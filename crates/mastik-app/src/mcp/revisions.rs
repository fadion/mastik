//! What each connection was shown of a document: the revisions it looked at,
//! each layer's fingerprint at its latest looks, and what changed since, for
//! an answer's `changed`, a read's `since` and a write's `base_rev`.

use super::{Connection, index};
use crate::session::EditorSession;
use mastik_mcp::wire::{Content, Output};
use serde_json::{Value, json};
use std::collections::HashMap;
use std::rc::Rc;

/// How many of a connection's latest looks keep every layer's fingerprint, for
/// `since` and for naming what changed.
const KEPT: usize = 8;

/// How many of a connection's looks `base_rev` can be checked against. A look
/// past [`KEPT`] holds only the ids others changed, so an agent writing many
/// times off one read is not sent back to read again.
const LOOKS: usize = 1024;

/// What a connection was shown: a revision and, for the latest [`KEPT`], each
/// layer's fingerprint.
pub(super) struct Shown {
    rev: u64,
    layers: Option<Rc<HashMap<String, Layer>>>,
    /// The layers changed or removed since the look before by anyone but this
    /// client, so `base_rev` does not count the client's own steps against it.
    others: Vec<String>,
    /// The step on top of the history then, and the whole document's
    /// [`fingerprint`]: what `export_png`'s `since` rebuilds that document from
    /// and holds it to, kept for every look since a whole snapshot is not.
    top: Option<u64>,
    print: u64,
    /// Each data table's hash by its id, so `base_rev` holds for a table as
    /// for a layer.
    tables: HashMap<String, u64>,
}

/// Each of the document's data tables hashed, by its id.
pub(super) fn tables_of(s: &EditorSession) -> HashMap<String, u64> {
    use std::hash::{Hash as _, Hasher as _};
    s.doc
        .data_sets()
        .iter()
        .map(|d| {
            let mut h = std::hash::DefaultHasher::new();
            serde_json::to_string(d).unwrap_or_default().hash(&mut h);
            (d.id.0.to_wire(), h.finish())
        })
        .collect()
}

/// A layer as a connection saw it.
#[derive(Clone, PartialEq)]
pub(super) struct Layer {
    name: String,
    /// `None` on a page's root.
    parent: Option<String>,
    hash: u64,
}

/// The most layers one list of a `changed` report names. One deletion of a
/// frame lists every layer in it, which buried the answer it rode on; past
/// this, the report counts the rest, names the top-level layers they sit in,
/// and names the read that lists every one (`get_document` with `since` and
/// `brief`).
const CHANGED_LISTED: usize = 50;

/// What changed between two looks at a document.
#[derive(Default)]
pub(super) struct Delta {
    pub(super) added: Vec<String>,
    pub(super) changed: Vec<String>,
    pub(super) removed: Vec<String>,
}

impl Delta {
    fn is_empty(&self) -> bool {
        self.added.is_empty() && self.changed.is_empty() && self.removed.is_empty()
    }

    /// Each id with the name it has (or, removed, had), every one: what
    /// `get_document` with `since` and `brief` answers.
    fn to_json_whole(&self, now: &HashMap<String, Layer>, then: &HashMap<String, Layer>) -> Value {
        json!({
            "added": named(&self.added, now, usize::MAX),
            "changed": named(&self.changed, now, usize::MAX),
            "removed": named(&self.removed, then, usize::MAX),
        })
    }

    /// Each id with the name it has (or, removed, had), at most [`CHANGED_LISTED`] a
    /// list. Past that, `left_out` counts the rest of each list, names in `in` the
    /// top-level layers holding them (at most [`CHANGED_LISTED`] too), and in
    /// `read_all` the call that lists every one since `from`, the revision the
    /// report runs from.
    pub(super) fn to_json(
        &self,
        now: &HashMap<String, Layer>,
        then: &HashMap<String, Layer>,
        from: u64,
    ) -> Value {
        let name = |l: &Layer| json!(l.name);
        let mut report = json!({
            "added": named(&self.added, now, CHANGED_LISTED),
            "changed": named(&self.changed, now, CHANGED_LISTED),
            "removed": named(&self.removed, then, CHANGED_LISTED),
        });
        let past = |ids: &[String]| ids.len().saturating_sub(CHANGED_LISTED);
        if [&self.added, &self.changed, &self.removed]
            .iter()
            .all(|ids| past(ids) == 0)
        {
            return report;
        }
        let mut sections: Vec<String> = Vec::new();
        let left = [
            (&self.added, now),
            (&self.changed, now),
            (&self.removed, then),
        ];
        for (ids, from) in left {
            for id in ids.iter().skip(CHANGED_LISTED) {
                if let Some(top) = top_level(id, from)
                    && !sections.contains(&top)
                {
                    sections.push(top);
                }
            }
        }
        sections.sort();
        let sections: Vec<Value> = sections
            .iter()
            .take(CHANGED_LISTED)
            .map(|id| {
                let layer = now.get(id).or_else(|| then.get(id));
                json!({ "id": id, "name": layer.map_or(json!(""), name) })
            })
            .collect();
        report["left_out"] = json!({
            "added": past(&self.added),
            "changed": past(&self.changed),
            "removed": past(&self.removed),
            "in": sections,
            "read_all": { "tool": "get_document", "args": { "since": from, "brief": true } },
        });
        report
    }
}

/// The first `most` of `ids`, each with its name in `from`.
fn named(ids: &[String], from: &HashMap<String, Layer>, most: usize) -> Vec<Value> {
    ids.iter()
        .take(most)
        .map(|id| json!({ "id": id, "name": from.get(id).map_or("", |l| l.name.as_str()) }))
        .collect()
}

/// The layer of `from` that holds `id` (or is it) directly under a page.
fn top_level(id: &str, from: &HashMap<String, Layer>) -> Option<String> {
    let mut at = id;
    loop {
        let parent = from.get(at)?.parent.as_deref()?;
        if from.get(parent)?.parent.is_none() {
            return Some(at.to_owned());
        }
        at = parent;
    }
}

pub(super) fn delta(then: &HashMap<String, Layer>, now: &HashMap<String, Layer>) -> Delta {
    let mut d = Delta::default();
    for (id, layer) in now {
        match then.get(id) {
            None => d.added.push(id.clone()),
            Some(old) if old != layer => d.changed.push(id.clone()),
            Some(_) => {}
        }
    }
    d.removed = then
        .keys()
        .filter(|id| !now.contains_key(*id))
        .cloned()
        .collect();
    d.added.sort();
    d.changed.sort();
    d.removed.sort();
    d
}

/// What serde writes, fed straight to a hasher, so a fingerprint builds no
/// string.
struct HashWriter<'a>(&'a mut std::hash::DefaultHasher);

impl std::io::Write for HashWriter<'_> {
    fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
        std::hash::Hasher::write(self.0, bytes);
        Ok(bytes.len())
    }

    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

/// The whole document's fingerprint, from every layer's: the same for the same
/// layers in whatever order a map holds them.
pub(super) fn fingerprint(layers: &HashMap<String, Layer>) -> u64 {
    use std::hash::{Hash as _, Hasher as _};
    layers.iter().fold(0u64, |acc, (id, l)| {
        let mut h = std::hash::DefaultHasher::new();
        id.hash(&mut h);
        l.hash.hash(&mut h);
        acc.wrapping_add(h.finish())
    })
}

/// Every layer's fingerprint in a document no session holds.
pub(super) fn layers_of_doc(
    doc: &mastik_core::Document,
    res: &mastik_core::Resolved,
) -> HashMap<String, Layer> {
    layers(&mastik_export::snapshot(doc, res), doc)
}

/// The step on top of the history and the document's fingerprint at the
/// connection's look at `rev`, while it is among the looks it keeps.
pub(super) fn look_at(conn: &Connection, rev: u64) -> Option<(Option<u64>, u64)> {
    conn.shown
        .iter()
        .find(|s| s.rev == rev)
        .map(|s| (s.top, s.print))
}

/// Every layer's fingerprint: its snapshot entry, hashed with what a registry
/// verb writes on it that the entry leaves out — its note, and on a page's
/// root the page's settings (flows, play) — so `base_rev` sees them change.
fn layers(
    snapshot: &mastik_export::Snapshot,
    doc: &mastik_core::Document,
) -> HashMap<String, Layer> {
    use std::hash::{Hash as _, Hasher as _};
    let pages: HashMap<&str, &mastik_export::snapshot::PageSnapshot> =
        snapshot.pages.iter().map(|p| (p.id.as_str(), p)).collect();
    snapshot
        .nodes
        .iter()
        .map(|n| {
            let mut h = std::hash::DefaultHasher::new();
            let _ = serde_json::to_writer(HashWriter(&mut h), n);
            if let Some(p) = pages.get(n.id.as_str()) {
                let _ = serde_json::to_writer(HashWriter(&mut h), p);
            }
            let note = mastik_core::NodeId::from_wire(&n.id).and_then(|id| doc.get(id));
            note.map(|n| n.note()).unwrap_or_default().hash(&mut h);
            (
                n.id.clone(),
                Layer {
                    name: n.name.clone(),
                    parent: n.parent.clone(),
                    hash: h.finish(),
                },
            )
        })
        .collect()
}

/// A document state's snapshot and its layers' fingerprints, made once.
type Looked = (Rc<mastik_export::Snapshot>, Rc<HashMap<String, Layer>>);

thread_local! {
    /// The last state looked at, by its `(revision, drawn)`, both unique in the
    /// process: a call's catch-up, its read and the look after its write share
    /// one, and so do the clients of one document, rather than each building
    /// the whole snapshot on the person's UI thread.
    static LOOKED: std::cell::RefCell<Option<((u64, u64), Looked)>> =
        const { std::cell::RefCell::new(None) };
}

fn looked(s: &EditorSession) -> Looked {
    let at = (s.revision(), s.drawn());
    if let Some(l) =
        LOOKED.with_borrow(|c| c.as_ref().filter(|(k, _)| *k == at).map(|(_, l)| l.clone()))
    {
        return l;
    }
    let snap = mastik_export::snapshot(&s.doc, &s.resolved);
    let fingerprints = Rc::new(layers(&snap, &s.doc));
    let l = (Rc::new(snap), fingerprints);
    LOOKED.set(Some((at, l.clone())));
    l
}

pub(super) fn snapshot_of(s: &EditorSession) -> Rc<mastik_export::Snapshot> {
    looked(s).0
}

pub(super) fn layers_of(s: &EditorSession) -> Rc<HashMap<String, Layer>> {
    looked(s).1
}

/// Remember a look at `rev`, with the layers and tables `others` changed or
/// removed since the look before, `top`, the step on top of the history, and
/// `tables`, each table's hash ([`tables_of`]).
pub(super) fn remember(
    conn: &mut Connection,
    rev: u64,
    layers: Rc<HashMap<String, Layer>>,
    others: Vec<String>,
    top: Option<u64>,
    tables: HashMap<String, u64>,
) {
    if conn.shown.back().is_some_and(|s| s.rev == rev) {
        return;
    }
    let print = fingerprint(&layers);
    conn.shown.push_back(Shown {
        rev,
        layers: Some(layers),
        others,
        top,
        print,
        tables,
    });
    if let Some(old) = conn.shown.len().checked_sub(KEPT + 1) {
        conn.shown[old].layers = None;
    }
    while conn.shown.len() > LOOKS {
        conn.shown.pop_front();
    }
}

/// The fingerprints of the connection's last look, and its revision.
pub(super) fn last_layers(conn: &Connection) -> Option<(&HashMap<String, Layer>, u64)> {
    conn.shown
        .back()
        .and_then(|s| Some((s.layers.as_deref()?, s.rev)))
}

/// What changed since the connection's last look, and remember this one.
pub(super) fn catch_up(conn: &mut Connection, session: &EditorSession) -> Option<Value> {
    let rev = session.revision();
    if conn.shown.back()?.rev == rev {
        return None;
    }
    let now = layers_of(session);
    let (report, mut others) = match last_layers(conn) {
        Some((last, from)) => {
            let d = delta(last, &now);
            let report = (!d.is_empty()).then(|| d.to_json(&now, last, from));
            (report, d.changed.into_iter().chain(d.removed).collect())
        }
        None => (None, Vec::new()),
    };
    let tables = tables_of(session);
    if let Some(last) = conn.shown.back() {
        others.extend(
            last.tables
                .iter()
                .filter(|(id, hash)| tables.get(*id) != Some(*hash))
                .map(|(id, _)| id.clone()),
        );
    }
    remember(conn, rev, now, others, session.history.current(), tables);
    report
}

/// Put `rev` and, when there is one, `changed` on an answer.
pub(super) fn stamp(output: &mut Output, rev: u64, changed: Option<Value>) {
    match output.structured.as_mut() {
        Some(Value::Object(map)) => {
            map.insert("rev".into(), json!(rev));
            if let Some(c) = changed {
                map.insert("changed".into(), c);
            }
            // The first content is the structured answer's text; files follow it.
            let text = Content::Text(Value::Object(map.clone()).to_string());
            match output.content.first_mut() {
                Some(first @ Content::Text(_)) => *first = text,
                _ => output.content.insert(0, text),
            }
        }
        _ => {
            let mut note = json!({ "rev": rev });
            if let Some(c) = changed {
                note["changed"] = c;
            }
            output.content.push(Content::Text(note.to_string()));
        }
    }
}

/// The layers a read with `since` covers, so its answer names only those.
#[derive(Clone, Copy)]
pub(super) enum Scope<'a> {
    Document,
    /// One layer.
    Node(&'a str),
    /// A layer and everything under it.
    Subtree(&'a str),
    /// A layer and its own children; with no layer, the pages.
    Children(Option<&'a str>),
}

impl Scope<'_> {
    /// Whether `id`, as `layers` places it, is covered.
    fn covers(self, id: &str, layers: &HashMap<String, Layer>) -> bool {
        match self {
            Scope::Document => true,
            Scope::Node(root) => id == root,
            Scope::Subtree(root) => {
                let mut at = Some(id);
                while let Some(here) = at {
                    if here == root {
                        return true;
                    }
                    at = layers.get(here).and_then(|l| l.parent.as_deref());
                }
                false
            }
            Scope::Children(parent) => {
                parent == Some(id)
                    || layers
                        .get(id)
                        .is_some_and(|l| l.parent.as_deref() == parent)
            }
        }
    }
}

/// A read's `since`: `unchanged` when nothing `scope` covers moved, or the
/// covered layers changed and removed since that revision; `None` to answer in
/// full (no `since`, or one too old to diff).
pub(super) fn since(
    conn: &Connection,
    args: &Value,
    s: &EditorSession,
    scope: Scope<'_>,
) -> Result<Option<Value>, String> {
    let Some(since) = index(args, "since") else {
        return Ok(None);
    };
    if since == s.revision() {
        return Ok(Some(json!({ "unchanged": true })));
    }
    let Some(then) = conn
        .shown
        .iter()
        .find(|s| s.rev == since)
        .and_then(|s| s.layers.as_ref())
    else {
        return Ok(None);
    };
    let (snap, now) = looked(s);
    let d = delta(then, &now);
    let fresh: std::collections::HashSet<&str> = d
        .added
        .iter()
        .chain(&d.changed)
        .map(String::as_str)
        .filter(|id| scope.covers(id, &now))
        .collect();
    let removed: Vec<&String> = d
        .removed
        .iter()
        .filter(|id| scope.covers(id, then))
        .collect();
    if fresh.is_empty() && removed.is_empty() {
        return Ok(Some(json!({ "unchanged": true })));
    }
    let fresh: Vec<&mastik_export::snapshot::NodeSnapshot> = snap
        .nodes
        .iter()
        .filter(|n| fresh.contains(n.id.as_str()))
        .collect();
    Ok(Some(
        json!({ "since": since, "nodes": fresh, "removed": removed }),
    ))
}

/// `get_document`'s `since` with `brief`: every layer added, changed and removed
/// since that revision, by id and name, however many; `unchanged` when nothing
/// moved. Refused for a revision whose layers the connection no longer keeps,
/// since nothing can be diffed against it.
pub(super) fn since_brief(
    conn: &Connection,
    since: u64,
    s: &EditorSession,
) -> Result<Value, String> {
    if since == s.revision() {
        return Ok(json!({ "unchanged": true }));
    }
    let then = conn
        .shown
        .iter()
        .find(|x| x.rev == since)
        .and_then(|x| x.layers.as_ref())
        .ok_or_else(|| {
            format!(
                "`since` {since} is older than this connection keeps whole: read again with \
                 `get_outline` or `find_nodes`"
            )
        })?;
    let now = layers_of(s);
    let d = delta(then, &now);
    if d.is_empty() {
        return Ok(json!({ "unchanged": true }));
    }
    let mut answer = d.to_json_whole(&now, then);
    answer["since"] = json!(since);
    Ok(answer)
}

/// Refuse a write whose layers changed after `base`.
pub(super) fn stale(
    conn: &Connection,
    s: &EditorSession,
    base: u64,
    tx: &mastik_core::Transaction,
) -> Result<(), String> {
    if base == s.revision() {
        return Ok(());
    }
    let Some(at) = conn.shown.iter().position(|x| x.rev == base) else {
        return Err(format!(
            "`base_rev` {base} is older than this connection remembers: read again and retry"
        ));
    };
    // What others changed since `base`: every step but the client's own writes,
    // which are not news to it. `handle` caught up before this call, so the last
    // look is the document as the call found it.
    let others: std::collections::HashSet<&str> = conn
        .shown
        .iter()
        .skip(at + 1)
        .flat_map(|s| s.others.iter().map(String::as_str))
        .collect();
    // A table is no layer: one the call writes is named by `get_table`'s word.
    for op in &tx.0 {
        if let mastik_core::Operation::SetDataSet { id, .. } = op
            && others.contains(id.0.to_wire().as_str())
        {
            let name = s
                .doc
                .data_set(*id)
                .map_or_else(|| id.0.to_wire(), |d| d.name.clone());
            return Err(format!(
                "{name} changed since rev {base}. Read it again with get_table."
            ));
        }
    }
    // A delete writes away its whole subtree, so a layer inside it the person
    // changed counts as one the call writes.
    let deleted: Vec<mastik_core::NodeId> =
        tx.0.iter()
            .filter_map(|op| match op {
                mastik_core::Operation::DeleteNode { id } => Some(*id),
                _ => None,
            })
            .collect();
    // A run's inverses carry a delete as the subtree that puts it back, every
    // layer of it already gone from the document.
    let restored = tx.0.iter().flat_map(|op| match op {
        mastik_core::Operation::InsertSubtree { nodes, .. } => {
            nodes.iter().map(|n| n.id()).collect()
        }
        _ => Vec::new(),
    });
    let mut written = tx.subjects();
    for id in mastik_core::build::subtree_nodes(&s.doc, &deleted)
        .into_iter()
        .chain(restored)
    {
        if !written.contains(&id) {
            written.push(id);
        }
    }
    let moved: Vec<String> = written
        .iter()
        .map(|id| id.to_wire())
        .filter(|id| others.contains(id.as_str()))
        .collect();
    if moved.is_empty() {
        return Ok(());
    }
    // Each by the name the caller read, or the earliest look since that holds it.
    let names: Vec<String> = moved
        .iter()
        .map(|id| {
            let seen = conn
                .shown
                .iter()
                .skip(at)
                .find_map(|s| s.layers.as_ref()?.get(id));
            match seen {
                Some(l) => format!("{} ({id})", l.name),
                None => id.clone(),
            }
        })
        .collect();
    Err(format!(
        "Refused: changed since rev {base}: {}. Read them again and retry.",
        names.join(", ")
    ))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::mcp::Engine;
    use crate::mcp::tests::{One, call, ok, open, rect};

    /// Another connection's edit is reported as `changed` on this one's next
    /// call, once, and a connection is never told of its own write.
    ///
    /// flip: `catch_up` answering `None` always → fails at the second
    /// connection's `changed`.
    #[test]
    fn another_connections_edit_arrives_as_changed() {
        let (mut engine, mut docs) = (Engine::default(), One::new());
        open(&mut engine, &mut docs, 1);
        open(&mut engine, &mut docs, 2);
        let id = rect(&mut engine, &mut docs, 1, "Card");
        let mine = call(&mut engine, &mut docs, 1, "get_selection", json!({}));
        assert!(
            ok(&mine).get("changed").is_none(),
            "own write reported back"
        );
        let theirs = call(&mut engine, &mut docs, 2, "get_selection", json!({}));
        let changed = &ok(&theirs)["changed"];
        assert_eq!(changed["added"][0]["id"], json!(id));
        assert_eq!(changed["added"][0]["name"], "Card");
        let again = call(&mut engine, &mut docs, 2, "get_selection", json!({}));
        assert!(ok(&again).get("changed").is_none(), "reported twice");
    }

    /// The person deleting a frame of many layers is reported with each list cut
    /// at `CHANGED_LISTED`, the rest counted, and the frame named as where they were; a
    /// sibling frame's layers do not read as changed.
    ///
    /// flip: `Delta::to_json` naming every id → fails at the cut list's length.
    #[test]
    fn a_large_change_is_cut_and_names_its_sections() {
        let (mut engine, mut docs) = (Engine::default(), One::new());
        open(&mut engine, &mut docs, 1);
        let page = docs.session.doc.first_page();
        let frame = docs.session.doc.get(page).unwrap().children()[0];
        let args = json!({ "nodes": [frame.to_wire()] });
        let out = call(&mut engine, &mut docs, 1, "duplicate", args);
        let copy = ok(&out)["created"][0].as_str().unwrap().to_owned();
        let kind = mastik_core::NodeKind::Rect {
            size: mastik_core::kurbo::Size::new(120.0, 80.0),
            corner_radii: mastik_core::kurbo::RoundedRectRadii::from_single_radius(8.0),
            corner_smoothing: 0.0,
        };
        let ops = (0..CHANGED_LISTED + 5)
            .map(|i| mastik_core::Operation::CreateNode {
                id: docs.session.ids.mint(),
                parent: frame,
                index: i,
                kind: kind.clone(),
                transform: None,
                name: None,
            })
            .collect();
        docs.session
            .try_commit(mastik_core::Transaction(ops))
            .unwrap();
        ok(&call(&mut engine, &mut docs, 1, "get_selection", json!({})));
        let held = 1 + docs.session.doc.get(frame).unwrap().children().len();
        assert!(
            held > CHANGED_LISTED + 5,
            "the frame holds more than a list names"
        );
        let tx = mastik_core::Transaction(vec![mastik_core::Operation::DeleteNode { id: frame }]);
        docs.session.try_commit(tx).unwrap();
        let view = call(&mut engine, &mut docs, 1, "get_view", json!({}));
        let changed = &ok(&view)["changed"];
        assert_eq!(
            changed["removed"].as_array().map(Vec::len),
            Some(CHANGED_LISTED)
        );
        assert_eq!(
            changed["left_out"]["removed"],
            held - CHANGED_LISTED,
            "{changed}"
        );
        assert_eq!(changed["left_out"]["in"][0]["id"], json!(frame.to_wire()));
        let page = json!([{ "id": page.to_wire(), "name": "Page 1" }]);
        assert_eq!(
            changed["changed"], page,
            "only the page's children: {changed}"
        );
        assert!(
            !changed.to_string().contains(&copy),
            "the sibling is unchanged"
        );

        // flip: `Delta::to_json_whole` naming at most `CHANGED_LISTED` → fails at
        // "every layer".
        let read_all = changed["left_out"]["read_all"].clone();
        assert_eq!(read_all["tool"], "get_document");
        let all = call(
            &mut engine,
            &mut docs,
            1,
            "get_document",
            read_all["args"].clone(),
        );
        let all = ok(&all);
        assert_eq!(
            all["removed"].as_array().map(Vec::len),
            Some(held),
            "every layer: {all}"
        );
        assert_eq!(all["changed"], page);
    }

    /// `brief` needs `since`, and a revision the connection no longer keeps is
    /// refused rather than answered as nothing changed.
    #[test]
    fn a_brief_read_needs_a_revision_it_can_diff() {
        let (mut engine, mut docs) = (Engine::default(), One::new());
        open(&mut engine, &mut docs, 1);
        let out = call(
            &mut engine,
            &mut docs,
            1,
            "get_document",
            json!({ "brief": true }),
        );
        assert!(out.is_error, "no `since`");
        let args = json!({ "since": 1, "brief": true });
        let out = call(&mut engine, &mut docs, 1, "get_document", args);
        assert!(out.is_error, "a revision never shown");
        let rev = ok(&call(&mut engine, &mut docs, 1, "get_selection", json!({})))["rev"].clone();
        let args = json!({ "since": rev, "brief": true });
        let out = call(&mut engine, &mut docs, 1, "get_document", args);
        assert_eq!(ok(&out)["unchanged"], true);
    }

    /// A write whose layer changed after its `base_rev` is refused naming it; one
    /// at the current revision applies.
    ///
    /// flip: `stale` answering `Ok(())` always → fails at "refused".
    #[test]
    fn a_stale_write_is_refused() {
        let (mut engine, mut docs) = (Engine::default(), One::new());
        open(&mut engine, &mut docs, 1);
        open(&mut engine, &mut docs, 2);
        let id = rect(&mut engine, &mut docs, 1, "Button");
        let read = call(&mut engine, &mut docs, 2, "get_node", json!({ "id": id }));
        let base = ok(&read)["rev"].clone();
        let rename =
            |name: &str, base: &Value| json!({ "node": id, "name": name, "base_rev": base });
        ok(&call(
            &mut engine,
            &mut docs,
            1,
            "set_name",
            rename("Primary", &Value::Null),
        ));
        let late = call(&mut engine, &mut docs, 2, "set_name", rename("Mine", &base));
        assert!(late.is_error, "refused");
        let Content::Text(why) = &late.content[0] else {
            panic!("no text")
        };
        assert!(why.contains("Button"), "{why}");
        let read = call(&mut engine, &mut docs, 2, "get_node", json!({ "id": id }));
        let now = ok(&read)["rev"].clone();
        ok(&call(
            &mut engine,
            &mut docs,
            2,
            "set_name",
            rename("Mine", &now),
        ));
    }

    /// An `edit_table` whose table another client changed after its `base_rev`
    /// is refused naming the table; the client's own edits off the same read are
    /// not counted against it.
    ///
    /// flip: `catch_up` leaving the tables out of `others` → fails at "refused".
    #[test]
    fn a_stale_table_edit_is_refused() {
        let (mut engine, mut docs) = (Engine::default(), One::new());
        open(&mut engine, &mut docs, 1);
        open(&mut engine, &mut docs, 2);
        let args = json!({ "name": "People", "columns": ["Name"], "rows": [["Ana"], ["Ben"]] });
        ok(&call(&mut engine, &mut docs, 1, "add_table", args));
        let read = call(
            &mut engine,
            &mut docs,
            2,
            "get_table",
            json!({ "table": "People" }),
        );
        let base = ok(&read)["rev"].clone();
        let edit = |value: &str, base: &Value| {
            json!({ "table": "People", "set": [{ "row": 1, "column": "Name", "value": value }],
                "base_rev": base })
        };
        ok(&call(
            &mut engine,
            &mut docs,
            2,
            "edit_table",
            edit("Cy", &base),
        ));
        ok(&call(
            &mut engine,
            &mut docs,
            2,
            "edit_table",
            edit("Dee", &base),
        ));
        ok(&call(
            &mut engine,
            &mut docs,
            1,
            "edit_table",
            edit("Eve", &Value::Null),
        ));
        let late = call(&mut engine, &mut docs, 2, "edit_table", edit("Fay", &base));
        assert!(late.is_error, "refused");
        let Content::Text(why) = &late.content[0] else {
            panic!("no text")
        };
        let said = format!("People changed since rev {base}. Read it again with get_table.");
        assert!(why.contains(&said), "{why}");
    }

    /// A delete with a stale `base_rev` is refused when a layer inside what it
    /// removes changed, not only the layer it names.
    ///
    /// flip: `stale` checking `subjects()` alone → fails at "refused".
    #[test]
    fn a_stale_delete_checks_what_it_removes() {
        let (mut engine, mut docs) = (Engine::default(), One::new());
        open(&mut engine, &mut docs, 1);
        open(&mut engine, &mut docs, 2);
        let child = rect(&mut engine, &mut docs, 1, "Label");
        let out = call(
            &mut engine,
            &mut docs,
            1,
            "group",
            json!({ "nodes": [child] }),
        );
        let group = ok(&out)["created"][0].as_str().unwrap().to_owned();
        let read = call(
            &mut engine,
            &mut docs,
            2,
            "get_node",
            json!({ "id": group }),
        );
        let base = ok(&read)["rev"].clone();
        let rename = json!({ "node": child, "name": "Edited" });
        ok(&call(&mut engine, &mut docs, 1, "set_name", rename));
        let delete = json!({ "nodes": [group], "base_rev": base });
        let late = call(&mut engine, &mut docs, 2, "delete_node", delete);
        assert!(late.is_error, "refused");
        let id = mastik_core::NodeId::from_wire(&child).unwrap();
        assert!(docs.session.doc.get(id).is_some(), "the child stays");
    }

    /// A `set_note` or `set_page_play` whose layer or page another client
    /// changed after its `base_rev` is refused: the note and the page's
    /// settings are in the fingerprint though the snapshot entry lacks them.
    ///
    /// flip: `layers` without the note → fails at "the note is refused";
    /// without the page settings → fails at "the page is refused".
    #[test]
    fn a_stale_note_or_page_play_is_refused() {
        let (mut engine, mut docs) = (Engine::default(), One::new());
        open(&mut engine, &mut docs, 1);
        open(&mut engine, &mut docs, 2);
        let layer = rect(&mut engine, &mut docs, 1, "Card");
        let page = docs.session.doc.first_page().to_wire();
        let read = call(
            &mut engine,
            &mut docs,
            2,
            "get_node",
            json!({ "id": layer }),
        );
        let base = ok(&read)["rev"].clone();
        let note = json!({ "node": layer, "text": "Scrolls" });
        ok(&call(&mut engine, &mut docs, 1, "set_note", note));
        let late = json!({ "node": layer, "text": "Fixed", "base_rev": base });
        let late = call(&mut engine, &mut docs, 2, "set_note", late);
        assert!(late.is_error, "the note is refused");

        let read = call(&mut engine, &mut docs, 2, "get_node", json!({ "id": page }));
        let base = ok(&read)["rev"].clone();
        let play = mastik_core::verbs::example("PagePlay").unwrap();
        let args = json!({ "page": page, "play": play });
        ok(&call(&mut engine, &mut docs, 1, "set_page_play", args));
        let args = json!({ "page": page, "play": play, "base_rev": base });
        let late = call(&mut engine, &mut docs, 2, "set_page_play", args);
        assert!(late.is_error, "the page is refused");
    }

    /// `run_script` holds a stale `base_rev` to the same rule: a script deleting
    /// a group whose child changed is refused and nothing changes.
    ///
    /// flip: `stale` without the layers an inverse `InsertSubtree` restores →
    /// fails at "refused".
    #[test]
    fn a_stale_script_delete_checks_what_it_removes() {
        let (mut engine, mut docs) = (Engine::default(), One::new());
        open(&mut engine, &mut docs, 1);
        open(&mut engine, &mut docs, 2);
        let child = rect(&mut engine, &mut docs, 1, "Label");
        let out = call(
            &mut engine,
            &mut docs,
            1,
            "group",
            json!({ "nodes": [child] }),
        );
        let group = ok(&out)["created"][0].as_str().unwrap().to_owned();
        let read = call(
            &mut engine,
            &mut docs,
            2,
            "get_node",
            json!({ "id": group }),
        );
        let base = ok(&read)["rev"].clone();
        let rename = json!({ "node": child, "name": "Edited" });
        ok(&call(&mut engine, &mut docs, 1, "set_name", rename));
        let args = json!({ "source": "delete_node(sel)", "nodes": [group], "base_rev": base });
        let late = call(&mut engine, &mut docs, 2, "run_script", args);
        assert!(late.is_error, "refused: {:?}", late.content);
        let id = mastik_core::NodeId::from_wire(&child).unwrap();
        assert!(docs.session.doc.get(id).is_some(), "the child stays");
        let fresh = call(
            &mut engine,
            &mut docs,
            2,
            "run_script",
            json!({ "source": "delete_node(sel)", "nodes": [group] }),
        );
        ok(&fresh);
        assert!(docs.session.doc.get(id).is_none(), "the script deletes");
    }

    /// Writes off one read keep passing its `base_rev` long after the last few
    /// looks the connection keeps whole, and another client's change to the layer
    /// in between still refuses it.
    ///
    /// flip: `remember` dropping looks past `KEPT` → fails at "an old read still
    /// holds".
    #[test]
    fn a_base_rev_holds_across_many_own_writes() {
        let (mut engine, mut docs) = (Engine::default(), One::new());
        open(&mut engine, &mut docs, 1);
        open(&mut engine, &mut docs, 2);
        let id = rect(&mut engine, &mut docs, 1, "Card");
        let other = rect(&mut engine, &mut docs, 1, "Other");
        let read = call(&mut engine, &mut docs, 1, "get_selection", json!({}));
        let base = ok(&read)["rev"].clone();
        for i in 0..KEPT * 3 {
            let args = json!({ "node": id, "name": format!("Card {i}"), "base_rev": base });
            let out = call(&mut engine, &mut docs, 1, "set_name", args);
            assert!(!out.is_error, "an old read still holds: {:?}", out.content);
        }
        let args = json!({ "nodes": [other], "opacity": 0.5 });
        ok(&call(&mut engine, &mut docs, 2, "set_opacity", args));
        for _ in 0..KEPT * 2 {
            let args = json!({ "node": id, "name": "Card", "base_rev": base });
            ok(&call(&mut engine, &mut docs, 1, "set_name", args));
        }
        let args = json!({ "node": other, "name": "Mine", "base_rev": base });
        let late = call(&mut engine, &mut docs, 1, "set_name", args);
        assert!(late.is_error, "the other client's change refuses it");
        assert!(format!("{:?}", late.content).contains("Other"));
    }

    /// Two writes off one read apply: the client's own first write is no change
    /// against its `base_rev`. Another client's write in between still refuses.
    ///
    /// flip: `stale` counting every step, its own included → fails at "its own
    /// write is not news".
    #[test]
    fn a_clients_own_write_does_not_stale_its_base() {
        let (mut engine, mut docs) = (Engine::default(), One::new());
        open(&mut engine, &mut docs, 1);
        open(&mut engine, &mut docs, 2);
        let id = rect(&mut engine, &mut docs, 1, "Button");
        let read = call(&mut engine, &mut docs, 1, "get_node", json!({ "id": id }));
        let base = ok(&read)["rev"].clone();
        let rename = |name: &str| json!({ "node": id, "name": name, "base_rev": base });
        ok(&call(&mut engine, &mut docs, 1, "set_name", rename("One")));
        let second = call(&mut engine, &mut docs, 1, "set_name", rename("Two"));
        assert!(
            !second.is_error,
            "its own write is not news: {:?}",
            second.content
        );

        let other = json!({ "node": id, "name": "Theirs" });
        ok(&call(&mut engine, &mut docs, 2, "set_name", other));
        assert!(
            call(&mut engine, &mut docs, 1, "set_name", rename("Three")).is_error,
            "another client's write still refuses"
        );
    }

    /// One document state is snapshotted once, whoever looks; a change makes
    /// a new look.
    ///
    /// flip: `looked` without its cache → fails at "shared".
    #[test]
    fn a_document_state_is_snapshotted_once() {
        let (mut engine, mut docs) = (Engine::default(), One::new());
        open(&mut engine, &mut docs, 1);
        let first = snapshot_of(&docs.session);
        assert!(Rc::ptr_eq(&first, &snapshot_of(&docs.session)), "shared");
        rect(&mut engine, &mut docs, 1, "A");
        let after = snapshot_of(&docs.session);
        assert!(!Rc::ptr_eq(&first, &after), "a change looks again");
        assert_eq!(after.nodes.len(), first.nodes.len() + 1);
    }

    /// `since` on `get_node` and `list_children` answers only what the read
    /// covers: another layer's rename leaves one layer `unchanged`, and the
    /// parent's children come back in their own rows.
    ///
    /// flip: `since` ignoring its `scope` → fails at "the other layer's
    /// rename is not this one's".
    #[test]
    fn since_answers_only_the_layers_a_read_covers() {
        let (mut engine, mut docs) = (Engine::default(), One::new());
        open(&mut engine, &mut docs, 1);
        open(&mut engine, &mut docs, 2);
        let a = rect(&mut engine, &mut docs, 1, "A");
        let b = rect(&mut engine, &mut docs, 1, "B");
        let read = call(&mut engine, &mut docs, 2, "get_node", json!({ "id": a }));
        let base = ok(&read)["rev"].clone();
        let args = json!({ "node": b, "name": "Renamed" });
        ok(&call(&mut engine, &mut docs, 1, "set_name", args));

        let node = |engine: &mut Engine, docs: &mut One, id: &str| {
            let args = json!({ "id": id, "since": base });
            ok(&call(engine, docs, 2, "get_node", args)).clone()
        };
        let one = node(&mut engine, &mut docs, &a);
        assert_eq!(
            one["unchanged"], true,
            "the other layer's rename is not this one's: {one}"
        );
        let other = node(&mut engine, &mut docs, &b);
        let ids: Vec<&Value> = other["nodes"]
            .as_array()
            .unwrap()
            .iter()
            .map(|n| &n["id"])
            .collect();
        assert_eq!(ids, [&json!(b)], "{other}");

        let page = docs.session.doc.first_page().to_wire();
        let args = json!({ "id": page, "since": base });
        let rows = call(&mut engine, &mut docs, 2, "list_children", args);
        let rows = ok(&rows);
        assert_eq!(rows["since"], base, "{rows}");
        let names: Vec<&Value> = rows["children"]
            .as_array()
            .unwrap()
            .iter()
            .map(|c| &c["name"])
            .collect();
        assert!(
            names.contains(&&json!("Renamed")) && names.contains(&&json!("A")),
            "{rows}"
        );
        assert!(rows.get("nodes").is_none(), "in its own rows: {rows}");
    }
}
