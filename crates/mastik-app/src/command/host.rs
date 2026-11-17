//! The open document as the scripting VM sees it: reads through the session's
//! document, verbs answered with its id source, and each write committed at once
//! so later reads see it. A run's writes become one undo step ([`run_as_step`]).

use crate::session::{CommitError, EditorSession};
use mastik_core::verbs::{self, Ctx, Done};
use mastik_core::{Document, NodeId, Resolved, Transaction};
use serde_json::Value;

/// The session behind a run. A read-only host refuses every write, which is what
/// keeps a live read in the bar from changing anything.
pub(crate) struct SessionHost<'a> {
    pub(crate) s: &'a mut EditorSession,
    pub(crate) read_only: bool,
}

/// Why a read-only host refused a write.
pub(crate) const A_READ_WRITES_NOTHING: &str = "a read changes nothing";

impl mastik_vm::Host for SessionHost<'_> {
    fn doc(&self) -> &Document {
        &self.s.doc
    }

    fn resolved(&self) -> &Resolved {
        &self.s.resolved
    }

    fn page(&self) -> NodeId {
        self.s.page
    }

    fn verb(&mut self, name: &str, args: &Value) -> Result<Done, String> {
        if self.read_only {
            return Err(A_READ_WRITES_NOTHING.into());
        }
        let s = &mut *self.s;
        verbs::call(
            &mut Ctx {
                doc: &s.doc,
                res: &s.resolved,
                ids: &mut s.ids,
                images: None,
                contrast: None,
            },
            name,
            args,
        )
        .map_err(|e| e.to_string())
    }

    fn commit(&mut self, tx: Transaction) -> Result<(), String> {
        if self.read_only {
            return Err(A_READ_WRITES_NOTHING.into());
        }
        // Every write a run makes, a verb's or a property's, passes here, so this
        // one check keeps a kit's vendored component changing only with its kit.
        if let Some(id) = vault_touched(&self.s.doc, &tx) {
            return Err(format!(
                "“{}” is a kit's component, which changes only when the kit does",
                self.s
                    .doc
                    .get(id)
                    .map(|n| n.name().to_string())
                    .unwrap_or_default()
            ));
        }
        // The style browser's *Edit* is set aside, as for an agent: a script's
        // edit of a style's users means the same whatever panel is open.
        let held = self.s.style_edit.take();
        let applied = self.s.try_commit(tx);
        self.s.style_edit = held;
        applied.map_err(|e| match e {
            CommitError::Op(e) => e.to_string(),
            CommitError::ReadOnly => crate::compare::READ_ONLY.to_owned(),
            CommitError::Held => self
                .s
                .script_run
                .as_ref()
                .map_or_else(|| e.to_string(), |r| super::running::held(&r.name)),
        })
    }
}

/// What a run changed, counted from the steps it made: the layers made, changed
/// and deleted, and the document's scripts it touched, each only when not zero.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) struct Counts {
    pub(crate) made: usize,
    pub(crate) changed: usize,
    pub(crate) deleted: usize,
    pub(crate) scripts: usize,
}

impl Counts {
    /// `6 layers changed`, `12 made, 2 changed`, `1 script changed`; `None` when
    /// nothing changed.
    pub(crate) fn said(self) -> Option<String> {
        let mut parts = Vec::new();
        let layers = |n: usize| if n == 1 { "layer" } else { "layers" };
        let mut noun = true;
        for (n, verb) in [
            (self.made, "made"),
            (self.changed, "changed"),
            (self.deleted, "deleted"),
        ] {
            if n == 0 {
                continue;
            }
            parts.push(match noun {
                true => format!("{n} {} {verb}", layers(n)),
                false => format!("{n} {verb}"),
            });
            noun = false;
        }
        if self.scripts > 0 {
            let n = self.scripts;
            parts.push(format!(
                "{n} {} changed",
                if n == 1 { "script" } else { "scripts" }
            ));
        }
        (!parts.is_empty()).then(|| parts.join(", "))
    }
}

/// Count what the step whose inverse is `inverse` did: an inverse's delete is a
/// layer the step made, its create one the step deleted, and any other edit names
/// a layer it changed. A layer the step both made and deleted is not counted.
pub(crate) fn counts(inverse: &Transaction) -> Counts {
    use mastik_core::Operation as Op;
    let mut made = rustc_hash::FxHashSet::default();
    let mut deleted = rustc_hash::FxHashSet::default();
    let mut scripts = rustc_hash::FxHashSet::default();
    let mut scripts_added = rustc_hash::FxHashSet::default();
    let mut scripts_removed = rustc_hash::FxHashSet::default();
    for op in &inverse.0 {
        match op {
            Op::DeleteNode { id } => {
                made.insert(*id);
            }
            Op::CreateNode { id, .. } => {
                deleted.insert(*id);
            }
            Op::InsertSubtree { nodes, .. } => {
                if let Some(root) = nodes.first() {
                    deleted.insert(root.id());
                }
            }
            // The inverse of a removal adds the script back, and of an add removes it.
            Op::AddScript { script, .. } => {
                scripts_removed.insert(script.id);
            }
            Op::RemoveScript { id } => {
                scripts_added.insert(*id);
            }
            Op::SetScriptName { id, .. } | Op::SetScriptSource { id, .. } => {
                scripts.insert(*id);
            }
            _ => {}
        }
    }
    // A script the run added and removed again left the document as it was.
    let gone: Vec<_> = scripts_added
        .intersection(&scripts_removed)
        .copied()
        .collect();
    scripts.extend(scripts_added.union(&scripts_removed).copied());
    for id in &gone {
        scripts.remove(id);
    }
    let changed = inverse
        .subjects()
        .into_iter()
        .filter(|id| !made.contains(id) && !deleted.contains(id))
        .count();
    let both = made.intersection(&deleted).count();
    Counts {
        made: made.len() - both,
        changed,
        deleted: deleted.len() - both,
        scripts: scripts.len(),
    }
}

/// The first layer in `doc`'s kit vault that `tx` would change, or would put a
/// layer under: its subjects, and the parents its creates, inserts and moves name.
/// Reading a vendored main, as an instance does, is not a change to it.
fn vault_touched(doc: &Document, tx: &Transaction) -> Option<NodeId> {
    use mastik_core::Operation as Op;
    doc.vault()?;
    let parents = tx.0.iter().filter_map(|op| match op {
        Op::CreateNode { parent, .. } | Op::InsertSubtree { parent, .. } => Some(*parent),
        Op::Reparent { new_parent, .. } => Some(*new_parent),
        _ => None,
    });
    tx.subjects()
        .into_iter()
        .chain(parents)
        .find(|id| doc.in_vault(*id))
}

/// Run `f` against the session as one undo step named `label` with `glyph`: every
/// write it commits is joined into one step, and on an error every one is taken
/// back, so a failed run changes nothing — the redo stack included. Answers what
/// `f` answered and what the step did, with its serial.
pub(crate) fn run_as_step<T>(
    s: &mut EditorSession,
    label: &str,
    glyph: &'static str,
    f: impl FnOnce(&mut SessionHost<'_>) -> Result<T, mastik_vm::VmError>,
) -> Result<(T, Counts, Option<u64>), mastik_vm::VmError> {
    // A run never merges into the edit run before it.
    s.end_edit_run();
    let mark = s.mark();
    let depth = s.history.undo_depth();
    // Each commit clears the redo stack; a run that fails or lands nothing gives
    // it back below.
    let held_redo = s.history.take_redo();
    let ran = f(&mut SessionHost {
        s: &mut *s,
        read_only: false,
    });
    let made = s.history.undo_depth().saturating_sub(depth);
    match ran {
        Err(mut e) => {
            let mut whole = true;
            for _ in 0..made {
                if !s.take_back() {
                    whole = false;
                    break;
                }
            }
            if whole {
                s.rewind(&mark);
                s.history.restore_redo(held_redo);
            } else {
                e.message
                    .push_str(" (its changes could not all be taken back: undo the rest)");
            }
            s.end_edit_run();
            Err(e)
        }
        Ok(answer) => {
            if made == 0 {
                s.history.restore_redo(held_redo);
            }
            if made > 1 {
                s.history.join_last(made);
            }
            let mut counts = Counts::default();
            if made > 0 {
                if let Some(note) = s.history.current_note_mut() {
                    note.label = label.to_owned();
                    note.glyph = glyph.into();
                    note.origin = mastik_core::Origin::User;
                }
                if let Some(top) = s.history.done().next_back() {
                    counts = self::counts(top.inverse);
                }
            }
            s.end_edit_run();
            let serial = (made > 0).then(|| s.history.current()).flatten();
            Ok((answer, counts, serial))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::app::MastikApp;
    use mastik_core::kurbo::{RoundedRectRadii, Size};
    use mastik_core::{IdSource, NodeKind, Operation};

    /// A session holding one kit instance, its main vendored into the vault:
    /// the app, the instance and the main.
    fn with_kit_instance() -> (MastikApp, NodeId, NodeId) {
        let ctx = egui::Context::default();
        let mut app = MastikApp::headless(&ctx);
        let mut lids = IdSource::new(0xAD);
        let lroot = lids.mint();
        let mut kit = Document::new(lroot);
        let m = lids.mint();
        kit.apply(&Transaction(vec![
            Operation::CreateNode {
                id: m,
                parent: lroot,
                index: 0,
                kind: NodeKind::Artboard {
                    size: Size::new(40.0, 40.0),
                    corner_radii: RoundedRectRadii::default(),
                    corner_smoothing: 0.0,
                },
                transform: None,
                name: Some("Badge".into()),
            },
            Operation::SetComponent {
                id: m,
                component: true,
            },
        ]))
        .unwrap();
        let published = mastik_core::kit::freeze(&kit).unwrap();
        let root = app.session.doc.first_page();
        let (tx, inst) = mastik_core::kit::insert(
            &app.session.doc,
            &mut app.session.ids,
            "5eed0000000000000000000000000002",
            1,
            &published,
            m,
            root,
        )
        .unwrap();
        assert!(app.session.commit(tx));
        let main = app.session.doc.get(inst).unwrap().link().unwrap();
        assert!(app.session.doc.in_vault(main), "the fixture vendored it");
        (app, inst, main)
    }

    /// Run `script` with `selection` as one step; its error message, if any.
    fn run(app: &mut MastikApp, selection: &[NodeId], script: &str) -> Option<String> {
        let labels = Default::default();
        let env = mastik_vm::Env {
            selection,
            labels: &labels,
            params: Default::default(),
        };
        run_as_step(&mut app.session, "Edit", "", |h| {
            mastik_vm::run_script(h, &env, script)
        })
        .err()
        .map(|e| e.message)
    }

    /// A script's property writes and verbs on a kit's vendored component are
    /// refused, the main left as it was; the instance showing it still moves.
    ///
    /// flip: `commit` without `vault_touched` → fails at "refused on the main".
    #[test]
    fn a_script_never_changes_a_kits_component() {
        let (mut app, inst, main) = with_kit_instance();
        let before = app.session.doc.get(main).unwrap().clone();
        // The vault is never laid out, so `.x` and `.rotation` find no box and
        // write nothing; the main is checked unchanged below all the same.
        assert_eq!(
            run(&mut app, &[main], "sel.x = 10; sel.rotation = 15"),
            None
        );
        for script in [
            "sel.name = \"Mine\"",
            "sel.opacity = 0.5",
            "sel.visible = false",
            "sel.width = 5",
            "group()",
            "duplicate()",
        ] {
            let why = run(&mut app, &[main], script);
            assert!(
                why.as_deref()
                    .is_some_and(|w| w.contains("kit's component")),
                "refused on the main: {script}: {why:?}"
            );
        }
        assert_eq!(
            app.session.doc.get(main),
            Some(&before),
            "the main unchanged"
        );
        assert_eq!(
            run(&mut app, &[inst], "sel.x = 10"),
            None,
            "the instance moves"
        );
        assert_eq!(
            app.session.resolved.world_bounds(inst).map(|b| b.x0),
            Some(10.0)
        );
    }

    /// A headless app holding `n` rects on its first page.
    fn with_rects(n: usize) -> (MastikApp, Vec<NodeId>) {
        let ctx = egui::Context::default();
        let mut app = MastikApp::headless(&ctx);
        let page = app.session.doc.first_page();
        let ids: Vec<NodeId> = (0..n).map(|_| app.session.ids.mint()).collect();
        let ops = ids
            .iter()
            .enumerate()
            .map(|(i, id)| Operation::CreateNode {
                id: *id,
                parent: page,
                index: i,
                kind: NodeKind::Rect {
                    size: Size::new(10.0, 10.0),
                    corner_radii: RoundedRectRadii::default(),
                    corner_smoothing: 0.0,
                },
                transform: None,
                name: None,
            })
            .collect();
        assert!(app.session.commit(Transaction(ops)));
        (app, ids)
    }

    /// Run `script` over `selection` as one step; what it counted.
    fn counted(app: &mut MastikApp, selection: &[NodeId], script: &str) -> Counts {
        let labels = Default::default();
        let env = mastik_vm::Env {
            selection,
            labels: &labels,
            params: Default::default(),
        };
        run_as_step(&mut app.session, "Edit", "", |h| {
            mastik_vm::run_script(h, &env, script)
        })
        .map(|(_, counts, _)| counts)
        .expect("the script runs")
    }

    /// A run that adds a script and removes it again changed no script.
    /// flip: `counts` putting added and removed scripts in one set → fails at
    /// "no script changed".
    #[test]
    fn a_script_added_and_removed_is_not_counted() {
        let (mut app, _) = with_rects(0);
        let added = counted(&mut app, &[], "add_script{name = \"Tmp\", source = \"\"}");
        assert_eq!(added.scripts, 1, "the fixture adds one");
        let both = counted(
            &mut app,
            &[],
            "local s = add_script{name = \"Tmp\", source = \"\"}\nremove_script{script = s}",
        );
        assert_eq!(both.scripts, 0, "no script changed");
    }

    /// A run writing several layers, one step per write, names them all.
    /// flip: `History::join_last` keeping the oldest note's subjects alone →
    /// fails at "every layer".
    #[test]
    fn a_joined_run_names_every_layer_it_wrote() {
        let (mut app, ids) = with_rects(3);
        let counts = counted(&mut app, &ids, "for _, l in sel do l.name = \"Card\" end");
        assert_eq!(counts.changed, 3, "the fixture writes three");
        let top = app.session.history.done().next_back().unwrap();
        let mut subjects = top.note.subjects.clone();
        subjects.sort();
        let mut want = ids.clone();
        want.sort();
        assert_eq!(subjects, want, "every layer");
    }

    /// A run's write to a styled layer leaves the style browser's *Edit* alone:
    /// the style keeps its value, and *Edit* stays open.
    /// flip: `SessionHost::commit` without taking `style_edit` aside → fails at
    /// "the style keeps its value".
    #[test]
    fn a_script_leaves_the_open_style_edit_alone() {
        use mastik_core::peniko::Color;
        use mastik_core::style::{Style, StyleValue};
        use mastik_core::{Fill, keyed_by_position};
        let (mut app, ids) = with_rects(1);
        let style: mastik_core::style::StyleId = app.session.ids.mint_style();
        let red = keyed_by_position([Fill {
            brush: Color::from_rgba8(255, 0, 0, 255).into(),
            visible: true,
        }]);
        assert!(app.session.commit(Transaction(vec![
            Operation::SetFills {
                id: ids[0],
                fills: red.clone(),
            },
            Operation::AddStyle {
                style: Style {
                    id: style,
                    name: "Surface".into(),
                    value: StyleValue::Fills(red.clone()),
                    origin: None,
                    forked: None,
                    description: String::new(),
                },
            },
            Operation::SetFillStyle {
                id: ids[0],
                style: Some(style),
            },
        ])));
        app.session.style_edit = Some(style);
        counted(&mut app, &ids, "sel.fill = \"#00ff00\"");
        assert_eq!(
            app.session.doc.styles().get(style).map(|s| &s.value),
            Some(&StyleValue::Fills(red)),
            "the style keeps its value"
        );
        assert_eq!(app.session.style_edit, Some(style), "Edit stays open");
    }

    /// A run that lands no step keeps the person's redo.
    /// flip: `run_as_step` without restoring the redo stack when it made no
    /// step → fails at "redo kept".
    #[test]
    fn a_run_that_lands_nothing_keeps_redo() {
        let (mut app, ids) = with_rects(1);
        assert!(app.session.undo());
        assert_eq!(
            app.session.history.redo_depth(),
            1,
            "the fixture has a redo"
        );
        counted(&mut app, &ids, "print(#sel)");
        assert_eq!(app.session.history.redo_depth(), 1, "redo kept");
    }
}
