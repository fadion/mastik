//! `group_steps`: the writes a client makes between `begin` and `end` joined
//! into one undo step, so one request of several calls is one step to undo.
//! Each write lands and shows at once; only the history is joined, at the end.
//! A group the client leaves open closes at its turn's end (the assistant's)
//! or when its connection closes.

use super::{Answer, Connection, Documents};
use crate::session::EditorSession;
use serde_json::{Value, json};
use std::path::PathBuf;

/// An open group: where it began and what its step is called.
pub(super) struct Group {
    document: PathBuf,
    /// The serial the history had minted at `begin`: the group's steps are
    /// the client's steps above it.
    after: u64,
    name: String,
    /// The client as its steps record it (`StepNote::agent`).
    agent: String,
    /// The name the client gave at `initialize`, for the joined step's origin.
    client: String,
}

/// The name a group's step takes when `begin` gives none.
const UNNAMED: &str = "group";

/// `group_steps(action, name?)`. `begin` closes a group still open first, so a
/// client that never ended its last one is not refused; `end` with none open
/// is refused, saying so.
pub(super) fn run(
    conn: &mut Connection,
    s: &mut EditorSession,
    path: &std::path::Path,
    agent: String,
    client: &str,
    args: &Value,
) -> Result<Answer, String> {
    match args.get("action").and_then(Value::as_str) {
        Some("begin") => {
            let closed = conn.group.take().map(|g| join(s, g));
            let name = args
                .get("name")
                .and_then(Value::as_str)
                .map(str::trim)
                .filter(|n| !n.is_empty())
                .unwrap_or(UNNAMED);
            conn.group = Some(Group {
                document: path.to_path_buf(),
                after: s.history.last_serial(),
                name: name.to_owned(),
                agent,
                client: client.to_owned(),
            });
            let mut answer = json!({ "begun": name });
            if let Some(closed) = closed {
                answer["closed"] = closed;
            }
            Ok(Answer::Json(answer))
        }
        Some("end") => match conn.group.take() {
            Some(g) => Ok(Answer::Json(join(s, g))),
            None => Err(
                "No group is open: `group_steps` with `begin` opens one, and a group left \
                 open closed at the turn's end"
                    .into(),
            ),
        },
        _ => Err("`action` is `begin` or `end`".into()),
    }
}

/// Close `group` on the document it began in, if the host still holds it, and
/// write the joined history where a tab behind or `serve` keeps it.
pub(super) fn close(docs: &mut impl Documents, group: Group) {
    let path = group.document.clone();
    let Some(s) = docs.session(&path) else {
        return;
    };
    if join(s, group)["joined"].as_u64().unwrap_or(0) > 1 {
        let _ = docs.committed(&path);
    }
}

/// Join the group's steps into one named by it: the client's steps above
/// `after` lying on top of the history, as `History::join_agent_run` decides;
/// the client's steps under a step of anyone else's stay apart (`separate`).
fn join(s: &mut EditorSession, group: Group) -> Value {
    let (joined, separate) = s.join_agent_run(group.after, &group.agent);
    if joined > 0
        && let Some(note) = s.history.current_note_mut()
    {
        note.label.clone_from(&group.name);
        note.origin = mastik_core::Origin::Mcp {
            client: group.client,
            tool: "group_steps".into(),
        };
        note.glyph = "".into();
    }
    let mut answer = json!({ "ended": group.name, "joined": joined });
    if separate > 0 {
        answer["separate"] = json!(separate);
    }
    answer
}

#[cfg(test)]
mod tests {
    use crate::mcp::Engine;
    use crate::mcp::tests::{One, call, keyed, ok, open, rect};
    use serde_json::json;

    fn begin(engine: &mut Engine, docs: &mut One, conn: u64, name: &str) {
        let args = json!({ "action": "begin", "name": name });
        ok(&call(engine, docs, conn, "group_steps", args));
    }

    /// The writes between `begin` and `end` land one by one and end as one step
    /// named by the group, which the agent's `undo` takes back whole.
    ///
    /// flip: `join` without `join_last` → fails at "one step".
    #[test]
    fn a_group_is_one_step_named_by_it() {
        let (mut engine, mut docs) = (Engine::default(), One::new());
        open(&mut engine, &mut docs, 1);
        let before = docs.session.history.undo_depth();
        let layers = docs.session.doc.len();
        begin(&mut engine, &mut docs, 1, "Add cards");
        rect(&mut engine, &mut docs, 1, "A");
        rect(&mut engine, &mut docs, 1, "B");
        assert_eq!(
            docs.session.doc.len(),
            layers + 2,
            "each write lands at once"
        );
        let end = json!({ "action": "end" });
        let out = call(&mut engine, &mut docs, 1, "group_steps", end.clone());
        assert_eq!(ok(&out)["joined"], 2);
        let h = &docs.session.history;
        assert_eq!(h.undo_depth(), before + 1, "one step");
        assert_eq!(h.done().last().unwrap().note.label, "Add cards");
        let undone = call(&mut engine, &mut docs, 1, "undo", json!({}));
        assert_eq!(ok(&undone)["undone"], "Add cards");
        assert_eq!(docs.session.doc.len(), layers, "both taken back");
        let again = call(&mut engine, &mut docs, 1, "group_steps", end);
        assert!(again.is_error, "no group is open");
    }

    /// A save inside a group's run marks no step once the run joins, since the
    /// file holds only the run's first writes; a save at its top moves to the
    /// joined step.
    ///
    /// flip: `EditorSession::join_agent_run` leaving `saved_step` alone → fails
    /// at "saved inside the run".
    #[test]
    fn a_join_keeps_the_saved_mark_true() {
        for at_top in [false, true] {
            let (mut engine, mut docs) = (Engine::default(), One::new());
            open(&mut engine, &mut docs, 1);
            begin(&mut engine, &mut docs, 1, "Cards");
            rect(&mut engine, &mut docs, 1, "A");
            let first = docs.session.history.current();
            if !at_top {
                docs.session.mark_saved("cards.mastik".into());
            }
            rect(&mut engine, &mut docs, 1, "B");
            if at_top {
                docs.session.mark_saved("cards.mastik".into());
            }
            let end = call(
                &mut engine,
                &mut docs,
                1,
                "group_steps",
                json!({ "action": "end" }),
            );
            assert_eq!(ok(&end)["joined"], 2);
            assert_eq!(docs.session.history.current(), first, "the oldest serial");
            assert_eq!(
                docs.session.saved_step(),
                at_top.then_some(first),
                "saved inside the run: {at_top}"
            );
        }
    }

    /// The person's step among the group's ends its run: only the agent's steps
    /// above it join, and the ones under it stay apart.
    ///
    /// flip: `join` counting every step above `after` → fails at `joined`.
    #[test]
    fn the_persons_step_splits_a_group() {
        let (mut engine, mut docs) = (Engine::default(), One::new());
        open(&mut engine, &mut docs, 1);
        begin(&mut engine, &mut docs, 1, "Cards");
        rect(&mut engine, &mut docs, 1, "A");
        let page = docs.session.doc.first_page();
        let id = docs.session.ids.mint();
        let kind = mastik_core::NodeKind::Rect {
            size: mastik_core::kurbo::Size::new(10.0, 10.0),
            corner_radii: mastik_core::kurbo::RoundedRectRadii::from_single_radius(0.0),
            corner_smoothing: 0.0,
        };
        let tx = mastik_core::Transaction(vec![mastik_core::Operation::CreateNode {
            id,
            parent: page,
            index: 0,
            kind,
            transform: None,
            name: Some("Theirs".into()),
        }]);
        docs.session.try_commit(tx).unwrap();
        rect(&mut engine, &mut docs, 1, "B");
        rect(&mut engine, &mut docs, 1, "C");
        let depth = docs.session.history.undo_depth();
        let out = call(
            &mut engine,
            &mut docs,
            1,
            "group_steps",
            json!({ "action": "end" }),
        );
        assert_eq!(ok(&out)["joined"], 2, "{:?}", out.structured);
        assert_eq!(ok(&out)["separate"], 1);
        assert_eq!(docs.session.history.undo_depth(), depth - 1);
        let labels: Vec<String> = docs
            .session
            .history
            .done()
            .map(|v| v.note.label.clone())
            .collect();
        assert_eq!(
            labels.last().map(String::as_str),
            Some("Cards"),
            "{labels:?}"
        );
    }

    /// A group left open closes when the client's connection does, and a keyed
    /// chat's when its turn ends.
    ///
    /// flip: `Engine::closed` leaving the group open → fails at "closed with the
    /// connection"; `Engine::turn_ended` doing nothing → fails at "closed with
    /// the turn".
    #[test]
    fn an_open_group_closes_with_the_connection_or_the_turn() {
        let (mut engine, mut docs) = (Engine::default(), One::new());
        open(&mut engine, &mut docs, 1);
        let depth = docs.session.history.undo_depth();
        begin(&mut engine, &mut docs, 1, "Left open");
        rect(&mut engine, &mut docs, 1, "A");
        rect(&mut engine, &mut docs, 1, "B");
        engine.closed(&mut docs, 1);
        let h = &docs.session.history;
        assert_eq!(h.undo_depth(), depth + 1, "closed with the connection");
        assert_eq!(h.done().last().unwrap().note.label, "Left open");

        let key = Some("harness");
        let args = json!({ "document": "a" });
        ok(&keyed(&mut engine, &mut docs, 2, key, "use_document", args));
        let args = json!({ "action": "begin", "name": "Turn" });
        ok(&keyed(&mut engine, &mut docs, 2, key, "group_steps", args));
        let page = docs.session.doc.first_page().to_wire();
        let kind = mastik_core::verbs::example("NodeKind").unwrap();
        for name in ["C", "D"] {
            let args = json!({ "parent": page, "kind": kind, "name": name });
            ok(&keyed(&mut engine, &mut docs, 2, key, "create_node", args));
        }
        engine.turn_ended(&mut docs, "harness");
        let h = &docs.session.history;
        assert_eq!(h.undo_depth(), depth + 2, "closed with the turn");
        assert_eq!(h.done().last().unwrap().note.label, "Turn");
    }
}
