//! *Run script*: a document script run by Command Mode's Luau over the copy.

use crate::actions::Undo;
use crate::player::{Notice, Player};
use mastik_core::script::ScriptId;
use mastik_core::{ItemId, NodeId};

#[cfg(feature = "scripts")]
mod host {
    use std::collections::BTreeMap;

    use mastik_core::variable::{Value, VarId, VarKind};
    use mastik_core::verbs::{self, Ctx, Done};
    use mastik_core::{Document, IdSource, NodeId, NodeKind, Resolved, Transaction};

    /// One variable a run gave a play value: its play value and what it read on
    /// the screen before the run's first write to it.
    pub(super) struct Written {
        pub(super) var: VarId,
        pub(super) was: Option<Value>,
        pub(super) read: Option<Value>,
    }

    /// The copy as a script sees it: verbs answered with the player's ids, each
    /// write committed at once so later reads see it, and the play state `play`
    /// reaches.
    pub(super) struct PlayHost<'a> {
        pub(super) doc: &'a mut Document,
        pub(super) res: &'a mut Resolved,
        pub(super) ids: &'a mut IdSource,
        pub(super) page: NodeId,
        pub(super) dirty: Vec<NodeId>,
        /// The tables the run wrote, for the lists and charts over them to
        /// follow once it ends.
        pub(super) tables: Vec<mastik_core::list::DataSetId>,
        pub(super) values: &'a mut BTreeMap<VarId, Value>,
        /// The layer whose reaction runs the script, where a variable is read.
        pub(super) holder: NodeId,
        pub(super) screen: NodeId,
        /// The cells of the row the reaction fired in, by column.
        pub(super) row: Option<Vec<(String, String)>>,
        pub(super) written: Vec<Written>,
        /// The frame `play.go` asked for last, shown once the run ends.
        pub(super) go: Option<NodeId>,
    }

    impl mastik_vm::Host for PlayHost<'_> {
        fn doc(&self) -> &Document {
            self.doc
        }

        fn resolved(&self) -> &Resolved {
            self.res
        }

        fn page(&self) -> NodeId {
            self.page
        }

        fn verb(&mut self, name: &str, args: &serde_json::Value) -> Result<Done, String> {
            verbs::call(
                &mut Ctx {
                    doc: self.doc,
                    res: self.res,
                    ids: self.ids,
                    images: None,
                    contrast: None,
                },
                name,
                args,
            )
            .map_err(|e| e.to_string())
        }

        fn commit(&mut self, mut tx: Transaction) -> Result<(), String> {
            mastik_core::propagate::owed(self.doc, &mut tx, self.ids);
            let out = self.doc.apply(&tx).map_err(|e| e.to_string())?;
            self.res.update(self.doc, &out.dirty);
            self.dirty.extend(out.dirty.0);
            for set in crate::player::tables_written(&tx) {
                if !self.tables.contains(&set) {
                    self.tables.push(set);
                }
            }
            Ok(())
        }

        fn play(&mut self) -> Option<&mut dyn mastik_vm::Play> {
            Some(self)
        }
    }

    impl PlayHost<'_> {
        /// The variable `name` names as an expression names it, or its id.
        fn var_id(&self, name: &str) -> Result<VarId, String> {
            let t = self.doc.variables();
            match t.ids_named(name).as_slice() {
                [one] => Ok(*one),
                [] => NodeId::from_wire(name)
                    .map(VarId)
                    .filter(|v| t.get(*v).is_some())
                    .ok_or_else(|| format!("No variable “{name}”")),
                _ => Err(format!(
                    "More than one variable is called “{name}”: rename one"
                )),
            }
        }

        /// Where a variable is read: the holder, or the screen once the run
        /// deleted it.
        fn reader(&self) -> NodeId {
            if self.doc.get(self.holder).is_some() {
                self.holder
            } else {
                self.screen
            }
        }
    }

    /// `v` as a script reads it: a colour as `#rrggbb`.
    fn value_json(v: &Value) -> serde_json::Value {
        match v {
            Value::Number(n) => serde_json::json!(n),
            Value::Boolean(b) => serde_json::json!(b),
            Value::String(s) => serde_json::json!(s),
            Value::Color(_) => {
                serde_json::json!(mastik_core::prototype::expr::format_value(v))
            }
        }
    }

    /// `v` as a value of a `kind` variable called `name`, or why not.
    fn value_of(kind: VarKind, name: &str, v: &serde_json::Value) -> Result<Value, String> {
        let wanted = match kind {
            VarKind::Number => "a number",
            VarKind::String => "a string",
            VarKind::Boolean => "true or false",
            VarKind::Color => "a colour like \"#ff7a1a\"",
        };
        let value = match (kind, v) {
            (VarKind::Number, serde_json::Value::Number(n)) => {
                n.as_f64().filter(|n| n.is_finite()).map(Value::Number)
            }
            (VarKind::String, serde_json::Value::String(s)) => Some(Value::String(s.clone())),
            (VarKind::Boolean, serde_json::Value::Bool(b)) => Some(Value::Boolean(*b)),
            (VarKind::Color, serde_json::Value::String(s)) => {
                mastik_core::peniko::color::parse_color(s.trim())
                    .ok()
                    .map(|c| Value::Color(c.to_alpha_color::<mastik_core::peniko::color::Srgb>()))
            }
            _ => None,
        };
        value.ok_or_else(|| format!("{name} holds {wanted}, not {v}"))
    }

    impl mastik_vm::Play for PlayHost<'_> {
        fn var(&self, name: &str) -> Result<serde_json::Value, String> {
            let id = self.var_id(name)?;
            self.doc
                .resolve_var(self.reader(), id)
                .map(|v| value_json(&v))
                .ok_or_else(|| format!("{name} reads no value here"))
        }

        fn set_var(&mut self, name: &str, value: serde_json::Value) -> Result<(), String> {
            let id = self.var_id(name)?;
            let kind = self
                .doc
                .variables()
                .kind_of(id)
                .ok_or_else(|| format!("No variable “{name}”"))?;
            // *Set variable* skips a variable that reads nothing at its layer.
            if self.doc.resolve_var(self.reader(), id).is_none() {
                return Err(format!("{name} reads no value here"));
            }
            let value = value_of(kind, name, &value)?;
            if !self.written.iter().any(|w| w.var == id) {
                self.written.push(Written {
                    var: id,
                    was: self.values.get(&id).cloned(),
                    read: self.doc.resolve_var(self.screen, id),
                });
            }
            self.values.insert(id, value);
            let dirty = self.doc.set_play_values(self.values.clone());
            self.res.update(self.doc, &dirty);
            self.dirty.extend(dirty.0);
            Ok(())
        }

        fn row(&self) -> Option<Vec<(String, String)>> {
            self.row.clone()
        }

        fn go(&mut self, frame: &str) -> Result<(), String> {
            let is_frame = |id: NodeId| {
                self.doc
                    .get(id)
                    .is_some_and(|n| matches!(n.kind(), NodeKind::Artboard { .. }))
            };
            let named: Vec<NodeId> = self
                .doc
                .get(self.page)
                .map(|p| p.children().to_vec())
                .unwrap_or_default()
                .into_iter()
                .filter(|id| is_frame(*id) && self.doc.get(*id).is_some_and(|n| n.name() == frame))
                .collect();
            let to = match named.as_slice() {
                [one] => *one,
                [] => NodeId::from_wire(frame)
                    .filter(|id| is_frame(*id))
                    .ok_or_else(|| format!("No frame “{frame}” on this page"))?,
                _ => {
                    return Err(format!(
                        "{} frames on this page are called “{frame}”: rename one",
                        named.len()
                    ));
                }
            };
            self.go = Some(to);
            Ok(())
        }
    }
}

impl Player {
    /// Run document script `script` on the copy, for the action `action` on
    /// `holder`. A run that fails is rolled back to the copy as it was before it;
    /// `print` goes to the player's log; the selection is empty. What the run
    /// did to play values is pushed to `undo`, as *Set variable* pushes it.
    pub(crate) fn run_script(
        &mut self,
        holder: NodeId,
        action: ItemId,
        script: ScriptId,
        undo: &mut Vec<Undo>,
    ) {
        let Some(s) = self.doc.script(script).cloned() else {
            self.notice_once(
                holder,
                action,
                Notice::Missing {
                    layer: holder,
                    action,
                },
            );
            return;
        };
        self.run_script_source(holder, &s.name, &s.source, undo);
    }

    /// The cells of the table row `holder` is drawn in, by column, when it
    /// sits in a list over a table.
    #[cfg(feature = "scripts")]
    fn row_cells(&self, holder: NodeId) -> Option<Vec<(String, String)>> {
        let (list, _, row) = mastik_core::list::row_at(&self.doc, holder)?;
        let data = self.doc.get(list)?.list()?.data;
        let table = &self.doc.data_set(data)?.table;
        let cells = table.rows.get(row)?;
        Some(
            table
                .columns
                .iter()
                .zip(cells)
                .map(|(c, cell)| (c.name.clone(), cell.clone()))
                .collect(),
        )
    }

    #[cfg(feature = "scripts")]
    fn run_script_source(
        &mut self,
        holder: NodeId,
        name: &str,
        source: &str,
        undo: &mut Vec<Undo>,
    ) {
        let asks = mastik_vm::header(source).is_ok_and(|h| !h.inputs.is_empty());
        if asks {
            self.notices
                .push(Notice::ScriptNeedsInput { name: name.into() });
            return;
        }
        // The copy has no history to take a failed run back from.
        let (doc, res, values) = (self.doc.clone(), self.res.clone(), self.values.clone());
        let labels = rustc_hash::FxHashMap::default();
        let env = mastik_vm::Env {
            selection: &[],
            labels: &labels,
            params: serde_json::Map::new(),
        };
        let row = self.row_cells(holder);
        let (page, screen) = (self.page, self.screen);
        let mut host = host::PlayHost {
            doc: &mut self.doc,
            res: &mut self.res,
            ids: &mut self.ids,
            page,
            dirty: Vec::new(),
            tables: Vec::new(),
            values: &mut self.values,
            holder,
            screen,
            row,
            written: Vec::new(),
            go: None,
        };
        // Steps and memory alone bound the run: a clock would make it depend
        // on more than its inputs, and a take's replay diverge.
        let leash = mastik_vm::Leash {
            budget: mastik_vm::PLAY_BUDGET,
            stop: None,
        };
        let run = mastik_vm::run_script_leashed(&mut host, &env, source, &leash);
        let dirty = std::mem::take(&mut host.dirty);
        let tables = std::mem::take(&mut host.tables);
        let written = std::mem::take(&mut host.written);
        let go = host.go.take();
        let ok = run.is_ok();
        match run {
            Ok(out) => {
                self.dirty.extend(dirty);
                self.log.extend(out.printed);
                let _ = self.res.settle(&mut self.doc);
                self.follow_tables(&tables);
            }
            Err(e) => {
                self.doc = doc;
                self.res = res;
                self.values = values;
                self.log.push(format!("{name}: {e}"));
                self.notices.push(Notice::Script {
                    name: name.into(),
                    message: e.to_string(),
                });
            }
        }
        // A script may have added or removed inputs, or moved the play values'
        // table under them.
        self.prepare();
        self.apply_values();
        if !ok {
            return;
        }
        // What *Set variable* does after its write, for each variable the run
        // wrote, then the navigation it asked for, as the actions would in turn.
        for w in written {
            undo.push(Undo::Value(w.var, w.was));
            if self.doc.resolve_var(self.screen, w.var) != w.read {
                self.changed(w.var);
            }
        }
        if let Some(to) = go.filter(|to| self.is_frame(*to)) {
            let transition = self
                .doc
                .page(self.page)
                .map(|p| p.play.transition)
                .unwrap_or_default();
            undo.push(Undo::Screen(self.depth()));
            self.navigate(to, transition, false);
        }
    }

    #[cfg(not(feature = "scripts"))]
    fn run_script_source(
        &mut self,
        _holder: NodeId,
        _name: &str,
        _source: &str,
        _undo: &mut Vec<Undo>,
    ) {
        if !self.notices.contains(&Notice::ScriptsOff) {
            self.notices.push(Notice::ScriptsOff);
        }
    }
}
