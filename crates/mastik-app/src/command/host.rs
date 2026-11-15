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
        // A kit's vendored component changes only when the kit does.
        if let Some(id) = vault_node(&s.doc, args) {
            return Err(format!(
                "“{}” is a kit's component, which changes only when the kit does",
                s.doc
                    .get(id)
                    .map(|n| n.name().to_string())
                    .unwrap_or_default()
            ));
        }
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
        self.s.try_commit(tx).map_err(|e| match e {
            CommitError::Op(e) => e.to_string(),
            CommitError::ReadOnly => crate::compare::READ_ONLY.to_owned(),
        })
    }
}

/// What a run changed, counted from the steps it made: the layers made, changed
/// and deleted, each only when it is not zero.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) struct Counts {
    pub(crate) made: usize,
    pub(crate) changed: usize,
    pub(crate) deleted: usize,
}

impl Counts {
    /// `6 layers changed`, `12 made, 2 changed`; `None` when nothing changed.
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
        (!parts.is_empty()).then(|| parts.join(", "))
    }
}

/// Count what the step whose inverse is `inverse` did: an inverse's delete is a
/// layer the step made, its create one the step deleted, and any other edit names
/// a layer it changed.
pub(crate) fn counts(inverse: &Transaction) -> Counts {
    use mastik_core::Operation as Op;
    let mut made = rustc_hash::FxHashSet::default();
    let mut deleted = rustc_hash::FxHashSet::default();
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
            _ => {}
        }
    }
    let changed = inverse
        .subjects()
        .into_iter()
        .filter(|id| !made.contains(id) && !deleted.contains(id))
        .count();
    Counts {
        made: made.len(),
        changed,
        deleted: deleted.len(),
    }
}

/// The first layer `args` names that is a kit's vendored component: `node`,
/// `other`, `parent` or one of `nodes`.
fn vault_node(doc: &Document, args: &Value) -> Option<NodeId> {
    let one = ["node", "other", "parent"]
        .into_iter()
        .filter_map(|k| args.get(k).and_then(Value::as_str));
    let many = args
        .get("nodes")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(Value::as_str);
    one.chain(many)
        .filter_map(NodeId::from_wire)
        .find(|id| doc.in_vault(*id))
}

/// Run `f` against the session as one undo step named `label` with `glyph`: every
/// write it commits is joined into one step, and on an error every one is taken
/// back, so a failed run changes nothing. Answers what `f` answered and what the
/// step did, with its serial.
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
    let ran = f(&mut SessionHost {
        s: &mut *s,
        read_only: false,
    });
    let made = s.history.undo_depth().saturating_sub(depth);
    match ran {
        Err(e) => {
            for _ in 0..made {
                s.take_back();
            }
            s.rewind(&mark);
            s.end_edit_run();
            Err(e)
        }
        Ok(answer) => {
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
