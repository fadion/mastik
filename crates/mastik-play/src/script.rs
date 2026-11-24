//! *Run script*: a document script run by Command Mode's Luau over the copy.

use crate::player::{Notice, Player};
use mastik_core::script::ScriptId;
use mastik_core::{ItemId, NodeId};

#[cfg(feature = "scripts")]
mod host {
    use mastik_core::verbs::{self, Ctx, Done};
    use mastik_core::{Document, IdSource, NodeId, Resolved, Transaction};

    /// The copy as a script sees it: verbs answered with the player's ids, each
    /// write committed at once so later reads see it.
    pub(super) struct PlayHost<'a> {
        pub(super) doc: &'a mut Document,
        pub(super) res: &'a mut Resolved,
        pub(super) ids: &'a mut IdSource,
        pub(super) page: NodeId,
        pub(super) dirty: Vec<NodeId>,
        /// The tables the run wrote, for the lists and charts over them to
        /// follow once it ends.
        pub(super) tables: Vec<mastik_core::list::DataSetId>,
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
    }
}

impl Player {
    /// Run document script `script` on the copy, for the action `action` on
    /// `holder`. A run that fails is rolled back to the copy as it was before it;
    /// `print` goes to the player's log; the selection is empty.
    pub(crate) fn run_script(&mut self, holder: NodeId, action: ItemId, script: ScriptId) {
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
        self.run_script_source(&s.name, &s.source);
    }

    #[cfg(feature = "scripts")]
    fn run_script_source(&mut self, name: &str, source: &str) {
        let asks = mastik_vm::header(source).is_ok_and(|h| !h.inputs.is_empty());
        if asks {
            self.notices
                .push(Notice::ScriptNeedsInput { name: name.into() });
            return;
        }
        // The copy has no history to take a failed run back from.
        let (doc, res) = (self.doc.clone(), self.res.clone());
        let labels = rustc_hash::FxHashMap::default();
        let env = mastik_vm::Env {
            selection: &[],
            labels: &labels,
            params: serde_json::Map::new(),
        };
        let page = self.page;
        let mut host = host::PlayHost {
            doc: &mut self.doc,
            res: &mut self.res,
            ids: &mut self.ids,
            page,
            dirty: Vec::new(),
            tables: Vec::new(),
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
    }

    #[cfg(not(feature = "scripts"))]
    fn run_script_source(&mut self, _name: &str, _source: &str) {
        if !self.notices.contains(&Notice::ScriptsOff) {
            self.notices.push(Notice::ScriptsOff);
        }
    }
}
