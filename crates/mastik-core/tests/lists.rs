//! Lists from data (`list`): a list made from an instance repeats it once per
//! row, each copy's properties filled from its row, and the rows follow an edit
//! of the template, the data or the description inside that edit's undo step.

use mastik_core::chart::{Column, ColumnId, Table};
use mastik_core::kurbo::Size;
use mastik_core::list::{self, DataSet, Rows, Source};
use mastik_core::verbs::{self, Ctx, Done};
use mastik_core::{
    Document, History, IdSource, NodeId, NodeKind, Operation, Resolved, StepNote, Transaction,
};
use serde_json::{Value, json};

/// A document with one page, the id source that made it, and the page.
fn blank() -> (Document, IdSource, NodeId) {
    let mut ids = IdSource::new(0x7E5B);
    let page = ids.mint();
    (Document::new(page), ids, page)
}

/// Run verb `name` and commit its edit as a script does; answers what it made.
fn run(doc: &mut Document, ids: &mut IdSource, name: &str, args: Value) -> Vec<NodeId> {
    let res = Resolved::rebuild(doc);
    let Done {
        mut tx, created, ..
    } = verbs::call(
        &mut Ctx {
            doc,
            res: &res,
            ids,
            images: None,
            contrast: None,
        },
        name,
        &args,
    )
    .unwrap_or_else(|e| panic!("{name}: {e}"));
    mastik_core::propagate::owed(doc, &mut tx, ids);
    doc.apply(&tx).unwrap_or_else(|e| panic!("{name}: {e}"));
    created
}

/// A main `Row` holding a text bound to the text property `Title` and a
/// rectangle bound to the boolean property `Badge`, and an instance of it on
/// the page; answers the main and the instance.
fn row_component(doc: &mut Document, ids: &mut IdSource, page: NodeId) -> (NodeId, NodeId) {
    let frame = serde_json::to_value(NodeKind::Artboard {
        size: Size::new(200.0, 40.0),
        corner_radii: Default::default(),
        corner_smoothing: 0.0,
    })
    .unwrap();
    let rect = serde_json::to_value(NodeKind::Rect {
        size: Size::new(10.0, 10.0),
        corner_radii: Default::default(),
        corner_smoothing: 0.0,
    })
    .unwrap();
    let main = run(
        doc,
        ids,
        "create_node",
        json!({ "parent": page.to_wire(), "kind": frame, "name": "Row" }),
    )[0];
    let text = run(
        doc,
        ids,
        "create_text",
        json!({ "parent": main.to_wire(), "text": "Name" }),
    )[0];
    let badge = run(
        doc,
        ids,
        "create_node",
        json!({ "parent": main.to_wire(), "kind": rect }),
    )[0];
    run(
        doc,
        ids,
        "make_component",
        json!({ "nodes": [main.to_wire()] }),
    );
    run(
        doc,
        ids,
        "define_property",
        json!({ "node": text.to_wire(), "kind": "text", "name": "Title" }),
    );
    run(
        doc,
        ids,
        "define_property",
        json!({ "node": badge.to_wire(), "kind": "boolean", "name": "Badge" }),
    );
    let inst = run(
        doc,
        ids,
        "create_instance",
        json!({ "main": main.to_wire(), "parent": page.to_wire(), "x": 400, "y": 0 }),
    )[0];
    (main, inst)
}

/// A data set of `rows`, each (title, badge), under columns `title` and `badge`.
fn data(ids: &mut IdSource, rows: &[(&str, &str)]) -> DataSet {
    DataSet {
        id: ids.mint_data_set(),
        name: "rows.csv".into(),
        table: Table {
            columns: vec![
                Column {
                    id: ColumnId(0),
                    name: "title".into(),
                },
                Column {
                    id: ColumnId(1),
                    name: "Badge".into(),
                },
            ],
            rows: rows
                .iter()
                .map(|(t, b)| vec![(*t).to_string(), (*b).to_string()])
                .collect(),
        },
        source: None,
        pictures: Vec::new(),
    }
}

/// Commit `tx` as the app does: one step, then the lists and charts it left
/// out of date brought up to it and joined onto it.
fn commit(history: &mut History, doc: &mut Document, ids: &mut IdSource, tx: Transaction) {
    let mut tx = tx;
    mastik_core::propagate::owed(doc, &mut tx, ids);
    let lists = list::follow_ops(&tx);
    let mut dirty = history.commit(doc, tx).expect("commits");
    list::regenerate_in_history(history, doc, ids, &lists, &mut dirty).expect("regenerates");
}

/// Regenerate list `l` once more as an edit of its description would, the
/// description left as it is.
fn regenerate_again(history: &mut History, doc: &mut Document, ids: &mut IdSource, l: NodeId) {
    let same = doc.get(l).unwrap().list().unwrap().clone();
    let tx = Transaction(vec![Operation::SetList {
        id: l,
        list: Some(Box::new(same)),
    }]);
    commit(history, doc, ids, tx);
}

/// The text each row of list `l` draws, the template first.
fn titles(doc: &Document, l: NodeId) -> Vec<String> {
    let n = doc.get(l).unwrap();
    let copies = n.list().unwrap().copies.clone();
    std::iter::once(list::template(doc, l).unwrap())
        .chain(copies)
        .map(|r| {
            mastik_core::build::subtree_nodes(doc, &[r])
                .into_iter()
                .find_map(|id| match doc.get(id)?.kind() {
                    NodeKind::Text { content, .. } => Some(content.clone()),
                    _ => None,
                })
                .unwrap_or_default()
        })
        .collect()
}

/// Whether the badge of each row of list `l` is shown, the template first.
fn badges(doc: &Document, l: NodeId) -> Vec<bool> {
    let n = doc.get(l).unwrap();
    let copies = n.list().unwrap().copies.clone();
    std::iter::once(list::template(doc, l).unwrap())
        .chain(copies)
        .map(|r| {
            mastik_core::build::subtree_nodes(doc, &[r])
                .into_iter()
                .filter(|id| matches!(doc.get(*id).unwrap().kind(), NodeKind::Rect { .. }))
                .any(|id| doc.shown(id))
        })
        .collect()
}

/// Making a list fills the template from row 1 and one copy per later row, by
/// the columns whose names match the properties; one undo takes the frame, the
/// data set and every row away, and redo brings them back. The frame's
/// children are the template then the copies in row order.
/// flip: `regenerate_in_history` skipping `join_last` → fails at "one undo";
/// `insert_subtrees` not shifting a later placement at the same index → fails
/// at "the frame draws the rows in order".
#[test]
fn a_list_repeats_its_template_per_row_and_undoes_whole() {
    let (mut doc, mut ids, page) = blank();
    let (_, inst) = row_component(&mut doc, &mut ids, page);
    let set = data(&mut ids, &[("Linen", "yes"), ("Wool", "no"), ("Tote", "1")]);
    let res = Resolved::rebuild(&doc);
    let (tx, l) = list::make(&doc, &res, &mut ids, inst, Source::New(set)).expect("makes");
    let mut history = History::default();
    let before = doc.clone();
    commit(&mut history, &mut doc, &mut ids, tx);

    assert_eq!(titles(&doc, l), ["Linen", "Wool", "Tote"]);
    assert_eq!(badges(&doc, l), [true, false, true]);
    assert_eq!(list::template(&doc, l), Some(inst));
    assert_eq!(doc.get(l).unwrap().name(), "Row list");
    let copies = doc.get(l).unwrap().list().unwrap().copies.clone();
    assert_eq!(copies.len(), 2);
    assert_eq!(
        doc.get(l).unwrap().children(),
        [inst, copies[0], copies[1]],
        "the frame draws the rows in order"
    );

    let after = doc.clone();
    assert!(history.undo(&mut doc).is_ok());
    assert!(doc == before, "one undo takes the list and its rows away");
    assert!(history.redo(&mut doc).is_ok());
    assert!(doc == after, "redo brings them back");
}

/// Editing the data set, the rows mode or the template regenerates the copies
/// in the same step, and undo puts the previous rows back.
#[test]
fn rows_follow_the_data_the_mode_and_the_template() {
    let (mut doc, mut ids, page) = blank();
    let (main, inst) = row_component(&mut doc, &mut ids, page);
    let mut set = data(&mut ids, &[("A", "1"), ("B", "1")]);
    let res = Resolved::rebuild(&doc);
    let (tx, l) = list::make(&doc, &res, &mut ids, inst, Source::New(set.clone())).unwrap();
    let mut history = History::default();
    commit(&mut history, &mut doc, &mut ids, tx);
    assert_eq!(titles(&doc, l), ["A", "B"]);

    set.table.rows.push(vec!["C".into(), "0".into()]);
    let edit = Transaction(vec![Operation::SetDataSet {
        id: set.id,
        set: Some(Box::new(set.clone())),
    }]);
    commit(&mut history, &mut doc, &mut ids, edit);
    assert_eq!(titles(&doc, l), ["A", "B", "C"]);

    let mut first = doc.get(l).unwrap().list().unwrap().clone();
    first.rows = Rows::First(1);
    let edit = Transaction(vec![Operation::SetList {
        id: l,
        list: Some(Box::new(first)),
    }]);
    commit(&mut history, &mut doc, &mut ids, edit);
    assert_eq!(titles(&doc, l), ["A"]);
    history.undo(&mut doc).unwrap();
    assert_eq!(
        titles(&doc, l),
        ["A", "B", "C"],
        "undo brings the rows back"
    );

    // A main's edit reaches the template through the propagation, and the
    // copies are made again from it.
    let rect = doc
        .get(main)
        .unwrap()
        .children()
        .iter()
        .copied()
        .find(|c| matches!(doc.get(*c).unwrap().kind(), NodeKind::Rect { .. }))
        .unwrap();
    let edit = Transaction(vec![Operation::SetOpacity {
        id: rect,
        opacity: 0.5,
    }]);
    commit(&mut history, &mut doc, &mut ids, edit);
    let copies = doc.get(l).unwrap().list().unwrap().copies.clone();
    for c in copies {
        let r = mastik_core::build::subtree_nodes(&doc, &[c])
            .into_iter()
            .find(|id| matches!(doc.get(*id).unwrap().kind(), NodeKind::Rect { .. }))
            .unwrap();
        assert_eq!(
            doc.get(r).unwrap().opacity(),
            0.5,
            "a copy follows the main"
        );
    }
    history.undo(&mut doc).unwrap();
    history.redo(&mut doc).unwrap();
    assert_eq!(titles(&doc, l), ["A", "B", "C"]);
}

/// A list filtered by a variable on the canvas follows an edit of the
/// variable's value in that edit's step, and undo puts its rows back; a sort
/// orders the rows that pass.
/// flip: `follow_ops` dropping the variable ops → fails at "follows the value".
#[test]
fn a_filter_follows_the_variable_it_reads() {
    use mastik_core::list::{SortKey, Test, TestOp};
    let (mut doc, mut ids, page) = blank();
    let (_, inst) = row_component(&mut doc, &mut ids, page);
    let set = data(
        &mut ids,
        &[("Linen", "yes"), ("Wool", "no"), ("Camp", "yes")],
    );
    let res = Resolved::rebuild(&doc);
    let (tx, l) = list::make(&doc, &res, &mut ids, inst, Source::New(set)).unwrap();
    let mut history = History::default();
    commit(&mut history, &mut doc, &mut ids, tx);
    let yes = serde_json::to_value(mastik_core::Cell::String("yes".into())).unwrap();
    run(
        &mut doc,
        &mut ids,
        "add_variable",
        json!({ "kind": "string", "name": "Badge", "value": yes }),
    );
    let var = doc.variables().by_name("Badge").unwrap().id;
    let mut filtered = doc.get(l).unwrap().list().unwrap().clone();
    filtered.filter.tests.push(Test {
        column: ColumnId(1),
        op: TestOp::Is,
        value: mastik_core::prototype::Expr(format!("{{{}}}", var.0.to_wire())),
    });
    filtered.sort.push(SortKey {
        column: ColumnId(0),
        descending: false,
    });
    let edit = Transaction(vec![Operation::SetList {
        id: l,
        list: Some(Box::new(filtered)),
    }]);
    commit(&mut history, &mut doc, &mut ids, edit);
    assert_eq!(titles(&doc, l), ["Camp", "Linen"]);

    let no = serde_json::to_value(mastik_core::Cell::String("no".into())).unwrap();
    let res = Resolved::rebuild(&doc);
    let done = verbs::call(
        &mut Ctx {
            doc: &doc,
            res: &res,
            ids: &mut ids,
            images: None,
            contrast: None,
        },
        "set_variable_value",
        &json!({ "variable": var.0.to_wire(), "value": no }),
    )
    .unwrap();
    commit(&mut history, &mut doc, &mut ids, done.tx);
    assert_eq!(titles(&doc, l), ["Wool"], "follows the value");
    history.undo(&mut doc).unwrap();
    assert_eq!(
        titles(&doc, l),
        ["Camp", "Linen"],
        "undo puts the rows back"
    );
}

/// A list filtered by a variable, moved into a frame in another mode, passes
/// that mode's rows in the move's own step.
/// flip: `touched` without its `by_mode` arm does not bite, since the move also
/// dirties the template, whose walk up finds the list; without both → fails at
/// "the frame's mode".
#[test]
fn a_filtered_list_moved_into_another_mode_follows_it() {
    use mastik_core::list::{Test, TestOp};
    let (mut doc, mut ids, page) = blank();
    let (_, inst) = row_component(&mut doc, &mut ids, page);
    let set = data(
        &mut ids,
        &[("Linen", "yes"), ("Wool", "no"), ("Camp", "yes")],
    );
    let res = Resolved::rebuild(&doc);
    let (tx, l) = list::make(&doc, &res, &mut ids, inst, Source::New(set)).unwrap();
    let mut history = History::default();
    commit(&mut history, &mut doc, &mut ids, tx);
    let yes = serde_json::to_value(mastik_core::Cell::String("yes".into())).unwrap();
    let no = serde_json::to_value(mastik_core::Cell::String("no".into())).unwrap();
    let frame = serde_json::to_value(NodeKind::Artboard {
        size: Size::new(600.0, 600.0),
        corner_radii: Default::default(),
        corner_smoothing: 0.0,
    })
    .unwrap();
    let made = verbs::run_script(
        &mut doc,
        &mut ids,
        Default::default(),
        &json!([
            { "verb": "add_variable", "args": { "kind": "string", "name": "Badge", "value": yes }, "as": "v" },
            { "verb": "add_mode", "args": { "name": "Plain" }, "as": "m" },
            { "verb": "set_variable_value", "args": { "variable": "@v", "mode": "@m", "value": no } },
            { "verb": "create_node", "args": { "parent": page.to_wire(), "kind": frame, "x": 2000, "y": 0 }, "as": "f" },
            { "verb": "set_frame_mode", "args": { "node": "@f", "mode": "@m" } },
        ]),
    )
    .unwrap_or_else(|e| panic!("{e}"));
    let var = doc.variables().by_name("Badge").unwrap().id;
    let f = NodeId::from_wire(&made[3][0]).unwrap();
    let mut filtered = doc.get(l).unwrap().list().unwrap().clone();
    filtered.filter.tests.push(Test {
        column: ColumnId(1),
        op: TestOp::Is,
        value: mastik_core::prototype::Expr(format!("{{{}}}", var.0.to_wire())),
    });
    let edit = Transaction(vec![Operation::SetList {
        id: l,
        list: Some(Box::new(filtered)),
    }]);
    commit(&mut history, &mut doc, &mut ids, edit);
    assert_eq!(titles(&doc, l), ["Linen", "Camp"], "the page's mode");

    let into = Transaction(vec![Operation::Reparent {
        id: l,
        new_parent: f,
        index: 0,
    }]);
    commit(&mut history, &mut doc, &mut ids, into);
    assert_eq!(titles(&doc, l), ["Wool"], "the frame's mode");
}

/// A boolean cell that reads as neither shown nor hidden gives its row the
/// main's value, not the one row 1 last filled into the template.
/// flip: `regenerate` skipping the property on an unread cell → fails at "made again".
#[test]
fn an_unread_boolean_cell_takes_the_mains_value() {
    let (mut doc, mut ids, page) = blank();
    let (_, inst) = row_component(&mut doc, &mut ids, page);
    let set = data(&mut ids, &[("A", "0"), ("B", "maybe")]);
    let res = Resolved::rebuild(&doc);
    let (tx, l) = list::make(&doc, &res, &mut ids, inst, Source::New(set)).unwrap();
    let mut history = History::default();
    commit(&mut history, &mut doc, &mut ids, tx);
    assert_eq!(badges(&doc, l), [false, true]);
    regenerate_again(&mut history, &mut doc, &mut ids, l);
    assert_eq!(badges(&doc, l), [false, true], "made again");
}

/// A list saves and loads with its data set, and a duplicated list keeps its
/// copies under the new ids.
#[test]
fn a_list_saves_and_duplicates() {
    let (mut doc, mut ids, page) = blank();
    let (_, inst) = row_component(&mut doc, &mut ids, page);
    let set = data(&mut ids, &[("A", "1"), ("B", "0")]);
    let res = Resolved::rebuild(&doc);
    let (tx, l) = list::make(&doc, &res, &mut ids, inst, Source::New(set)).unwrap();
    let mut history = History::default();
    commit(&mut history, &mut doc, &mut ids, tx);

    let bytes = mastik_core::io::save(&doc).expect("saves");
    let back = mastik_core::io::load(&bytes).expect("loads");
    assert!(back == doc, "a list round-trips through the file");

    let (dup, made) = mastik_core::build::duplicate(&doc, &mut ids, &[l], Default::default());
    doc.apply(&dup).unwrap();
    let copy = made[0];
    let c = doc.get(copy).unwrap().list().unwrap();
    assert_eq!(c.copies.len(), 1);
    assert_eq!(
        doc.get(c.copies[0]).unwrap().parent(),
        Some(copy),
        "the duplicate names its own copies"
    );
    assert_eq!(titles(&doc, copy), ["A", "B"]);
}

/// `get_tree` reads a list frame as its `list` and its template, without the
/// copies, and `insert_subtree` of that tree makes a list again, its rows
/// made from the data; a read cut by `depth` is refused by `insert_subtree`.
/// flip: `tree_to` skipping `collapse_lists` → fails at "no copies in the read";
/// `tree_ops` ignoring `list` → panics at the new frame's `list()`.
#[test]
fn a_list_read_by_get_tree_inserts_as_a_list() {
    let (mut doc, mut ids, page) = blank();
    let (_, inst) = row_component(&mut doc, &mut ids, page);
    let set = data(&mut ids, &[("Linen", "yes"), ("Wool", "no"), ("Tote", "1")]);
    let res = Resolved::rebuild(&doc);
    let (tx, l) = list::make(&doc, &res, &mut ids, inst, Source::New(set)).unwrap();
    let mut history = History::default();
    commit(&mut history, &mut doc, &mut ids, tx);
    assert_eq!(doc.get(l).unwrap().list().unwrap().copies.len(), 2);

    let read = verbs::tree(&doc, l).expect("the list's tree");
    assert_eq!(
        read["children"].as_array().unwrap().len(),
        1,
        "no copies in the read"
    );
    assert!(read["list"].is_object(), "the read names the list: {read}");
    assert!(read["list"].get("copies").is_none());

    let made = edit(
        &mut history,
        &mut doc,
        &mut ids,
        "insert_subtree",
        json!({ "parent": page.to_wire(), "tree": read }),
    );
    let again = made[0];
    let list = doc.get(again).unwrap().list().expect("a list again");
    assert_eq!(list.data, doc.get(l).unwrap().list().unwrap().data);
    assert_eq!(list.copies.len(), 2, "its rows are made from the data");
    assert_eq!(titles(&doc, again), ["Linen", "Wool", "Tote"]);

    let cut = verbs::tree_to(&doc, l, Some(0)).unwrap();
    assert_eq!(cut["omitted"], 1, "the template is left out: {cut}");
    assert_eq!(cut["id"], l.to_wire(), "the cut layer keeps its id");
    let res = Resolved::rebuild(&doc);
    let refused = verbs::call(
        &mut Ctx {
            doc: &mut doc,
            res: &res,
            ids: &mut ids,
            images: None,
            contrast: None,
        },
        "insert_subtree",
        &json!({ "parent": page.to_wire(), "tree": cut }),
    )
    .expect_err("a cut tree is refused")
    .to_string();
    assert!(refused.contains("omitted"), "{refused}");
}

/// A board holding the main and a list of its instance, read with `get_tree`
/// and inserted, fills the new list's rows from the new main's properties,
/// which `insert_subtree` mints afresh.
/// flip: `insert_subtree` skipping `list_fills_onto_new_props` → fails at
/// "rows filled". `referenced` keeping a properties layer's id does not bite
/// here: the instance's `copy_of` keeps the main's anyway.
#[test]
fn a_list_read_with_its_main_fills_its_rows() {
    let (mut doc, mut ids, page) = blank();
    let frame = serde_json::to_value(NodeKind::Artboard {
        size: Size::new(800.0, 400.0),
        corner_radii: Default::default(),
        corner_smoothing: 0.0,
    })
    .unwrap();
    let board = run(
        &mut doc,
        &mut ids,
        "create_node",
        json!({ "parent": page.to_wire(), "kind": frame, "name": "Board" }),
    )[0];
    let (_, inst) = row_component(&mut doc, &mut ids, board);
    let set = data(&mut ids, &[("Linen", "yes"), ("Wool", "no"), ("Tote", "1")]);
    let res = Resolved::rebuild(&doc);
    let (tx, _) = list::make(&doc, &res, &mut ids, inst, Source::New(set)).unwrap();
    let mut history = History::default();
    commit(&mut history, &mut doc, &mut ids, tx);

    let read = verbs::tree(&doc, board).expect("the board");
    let made = edit(
        &mut history,
        &mut doc,
        &mut ids,
        "insert_subtree",
        json!({ "parent": page.to_wire(), "tree": read }),
    );
    let new_list = doc
        .get(made[0])
        .unwrap()
        .children()
        .iter()
        .copied()
        .find(|c| doc.get(*c).unwrap().list().is_some())
        .expect("a list in the new board");
    assert_eq!(
        titles(&doc, new_list),
        ["Linen", "Wool", "Tote"],
        "rows filled"
    );
    assert_eq!(badges(&doc, new_list), [true, false, true]);
}

/// A verb naming a layer inside a list's copy is refused, pointing at the
/// template; the same verb on the template's layer runs.
/// flip: `verbs::call` skipping `refuse_list_copies` → fails at "refused".
#[test]
fn a_verb_on_a_copy_is_refused() {
    let (mut doc, mut ids, page) = blank();
    let (_, inst) = row_component(&mut doc, &mut ids, page);
    let set = data(&mut ids, &[("A", "1"), ("B", "0")]);
    let res = Resolved::rebuild(&doc);
    let (tx, l) = list::make(&doc, &res, &mut ids, inst, Source::New(set)).unwrap();
    let mut history = History::default();
    commit(&mut history, &mut doc, &mut ids, tx);
    let text_in = |doc: &Document, root: NodeId| {
        mastik_core::build::subtree_nodes(doc, &[root])
            .into_iter()
            .find(|id| matches!(doc.get(*id).unwrap().kind(), NodeKind::Text { .. }))
            .unwrap()
    };
    let copy = doc.get(l).unwrap().list().unwrap().copies[0];
    let call = |doc: &mut Document, ids: &mut IdSource, node: NodeId| {
        let res = Resolved::rebuild(doc);
        verbs::call(
            &mut Ctx {
                doc,
                res: &res,
                ids,
                images: None,
                contrast: None,
            },
            "set_opacity",
            &json!({ "nodes": [node.to_wire()], "opacity": 0.5 }),
        )
        .map(|_| ())
        .map_err(|e| e.to_string())
    };
    let in_copy = text_in(&doc, copy);
    let refused = call(&mut doc, &mut ids, in_copy).expect_err("refused");
    assert!(
        refused.contains(&inst.to_wire()),
        "names the template: {refused}"
    );
    let in_template = text_in(&doc, inst);
    call(&mut doc, &mut ids, in_template).expect("the template's layer runs");
}

/// A set of two variants, *Paid* and *Due* of its one property, and a list over
/// an instance of *Paid* whose column of the property's name reads Due, Paid,
/// Nope, committed in `history`; answers the mains, the set and the list.
fn variant_list(
    doc: &mut Document,
    ids: &mut IdSource,
    history: &mut History,
) -> (Vec<NodeId>, NodeId, NodeId) {
    let page = doc.first_page();
    let frame = serde_json::to_value(NodeKind::Artboard {
        size: Size::new(100.0, 40.0),
        corner_radii: Default::default(),
        corner_smoothing: 0.0,
    })
    .unwrap();
    let mut mains = Vec::new();
    for name in ["Paid", "Due"] {
        let m = run(
            doc,
            ids,
            "create_node",
            json!({ "parent": page.to_wire(), "kind": frame, "name": name }),
        )[0];
        run(
            doc,
            ids,
            "make_component",
            json!({ "nodes": [m.to_wire()] }),
        );
        mains.push(m);
    }
    let set = run(
        doc,
        ids,
        "combine_as_variants",
        json!({ "nodes": mains.iter().map(|m| m.to_wire()).collect::<Vec<_>>() }),
    )[0];
    let prop = doc.get(set).unwrap().set().unwrap().props[0].clone();
    let value_of = |m: NodeId, doc: &Document| doc.get(m).unwrap().variant()[0].clone();
    let (paid, due) = (value_of(mains[0], doc), value_of(mains[1], doc));
    let inst = run(
        doc,
        ids,
        "create_instance",
        json!({ "main": mains[0].to_wire(), "parent": page.to_wire(), "x": 400, "y": 0 }),
    )[0];
    let data = DataSet {
        id: ids.mint_data_set(),
        name: "bills".into(),
        table: Table {
            columns: vec![Column {
                id: ColumnId(0),
                name: prop.name.clone(),
            }],
            rows: vec![vec![due.clone()], vec![paid.clone()], vec!["Nope".into()]],
        },
        source: None,
        pictures: Vec::new(),
    };
    let res = Resolved::rebuild(doc);
    let (tx, l) = list::make(doc, &res, ids, inst, Source::New(data)).unwrap();
    commit(history, doc, ids, tx);
    (mains, set, l)
}

/// The main each row of list `l` is an instance of, the template first.
fn row_mains(doc: &Document, l: NodeId) -> Vec<Option<NodeId>> {
    std::iter::once(list::template(doc, l).unwrap())
        .chain(doc.get(l).unwrap().list().unwrap().copies.clone())
        .map(|r| mastik_core::component::main_of(doc, r))
        .collect()
}

/// A column named after a variant property switches each row to the variant
/// its cell names, and a cell naming no variant takes the default variant's
/// value, however many times the list was made before.
/// flip: `regenerate` skipping the variant pass → fails at "switches"; an
/// unmatched cell keeping the captured variant → fails at "the default variant".
#[test]
fn a_variant_column_switches_each_row() {
    let (mut doc, mut ids, _) = blank();
    let mut history = History::default();
    let (mains, set, l) = variant_list(&mut doc, &mut ids, &mut history);
    let prop = doc.get(set).unwrap().set().unwrap().props[0].clone();
    assert_eq!(
        row_mains(&doc, l),
        [Some(mains[1]), Some(mains[0]), Some(mains[0])],
        "each row switches to the variant its cell names"
    );
    assert_eq!(
        list::unmatched_variants(&doc, l),
        [(prop.name.clone(), vec!["Nope".to_string()])]
    );
    assert_eq!(
        list::unmatched_variant_cells(&doc, l),
        [(prop.name.clone(), 2, 0, "Nope".to_string())],
        "the cell the data window marks"
    );

    // Made again, the template now shows row 1's Due; the unmatched row
    // still takes the default variant, not the template's last fill.
    assert_eq!(mastik_core::kit::default_variant(&doc, set), Some(mains[0]));
    regenerate_again(&mut history, &mut doc, &mut ids, l);
    let copies = doc.get(l).unwrap().list().unwrap().copies.clone();
    assert_eq!(
        mastik_core::component::main_of(&doc, copies[1]),
        Some(mains[0]),
        "the default variant"
    );
}

/// Run verb `name` and commit it as the app does, the lists it left out of
/// date made again in the same step; answers what it made.
fn edit(
    history: &mut History,
    doc: &mut Document,
    ids: &mut IdSource,
    name: &str,
    args: Value,
) -> Vec<NodeId> {
    let res = Resolved::rebuild(doc);
    let Done { tx, created, .. } = verbs::call(
        &mut Ctx {
            doc,
            res: &res,
            ids,
            images: None,
            contrast: None,
        },
        name,
        &args,
    )
    .unwrap_or_else(|e| panic!("{name}: {e}"));
    commit(history, doc, ids, tx);
    created
}

/// Renaming a variant property renames the list fill naming it, so the rows
/// keep their variants through the next regeneration.
/// flip: `rename_property` without `list::variant_prop_ops` → fails at "the
/// fill follows".
#[test]
fn renaming_a_variant_property_keeps_the_list_filling_it() {
    let (mut doc, mut ids, _) = blank();
    let mut h = History::default();
    let (mains, set, l) = variant_list(&mut doc, &mut ids, &mut h);
    let old = doc.get(set).unwrap().set().unwrap().props[0].name.clone();
    let args = json!({ "node": set.to_wire(), "property": old, "name": "State" });
    edit(&mut h, &mut doc, &mut ids, "rename_variant_property", args);
    let fill = &doc.get(l).unwrap().list().unwrap().fill;
    assert_eq!(
        fill[0].target,
        list::Target::Variant("State".into()),
        "the fill follows"
    );
    regenerate_again(&mut h, &mut doc, &mut ids, l);
    assert_eq!(
        row_mains(&doc, l),
        [Some(mains[1]), Some(mains[0]), Some(mains[0])]
    );
}

/// Renaming a variant property renames a per-row export's *Fill* pick on it,
/// so the picked column still fills the property; deleting the property drops
/// the pick.
/// flip: `variant_prop_ops` without its `SetExports` pass → fails at "the pick
/// follows".
#[test]
fn renaming_a_variant_property_keeps_a_per_row_pick_on_it() {
    use mastik_core::export::{ExportSpec, PerRow};
    let (mut doc, mut ids, _) = blank();
    let mut h = History::default();
    let (mains, set, l) = variant_list(&mut doc, &mut ids, &mut h);
    let table = doc.get(l).unwrap().list().unwrap().data;
    let old = doc.get(set).unwrap().set().unwrap().props[0].name.clone();
    let page = doc.first_page();
    let args = json!({ "main": mains[0].to_wire(), "parent": page.to_wire(), "x": 0, "y": 400 });
    let frame = run(&mut doc, &mut ids, "create_instance", args)[0];
    let pick = |name: &str| list::RowPick {
        slot: list::RowSlot::Fill(list::Target::Variant(name.into())),
        column: Some(ColumnId(0)),
    };
    let mut per_row = PerRow::new(table);
    per_row.fill = vec![pick(&old)];
    let spec = ExportSpec {
        per_row: Some(per_row),
        ..ExportSpec::new(Default::default(), Default::default())
    };
    let exports = mastik_core::keyed_by_position([spec]);
    let tx = Transaction(vec![Operation::SetExports { id: frame, exports }]);
    commit(&mut h, &mut doc, &mut ids, tx);
    let picks = |doc: &Document| {
        doc.get(frame).unwrap().exports()[0]
            .value
            .per_row
            .as_ref()
            .unwrap()
            .fill
            .clone()
    };

    let args = json!({ "node": set.to_wire(), "property": old, "name": "State" });
    edit(&mut h, &mut doc, &mut ids, "rename_variant_property", args);
    assert_eq!(picks(&doc), vec![pick("State")], "the pick follows");
    let rows = &doc.data_set(table).unwrap().table;
    let targets = list::row_targets_with(&doc, frame, rows, &picks(&doc));
    assert!(
        targets.contains(&list::RowTarget::Fill(list::Fill {
            target: list::Target::Variant("State".into()),
            column: ColumnId(0),
        })),
        "the picked column fills it: {targets:?}"
    );

    let args = json!({ "node": set.to_wire(), "name": "Size" });
    edit(&mut h, &mut doc, &mut ids, "add_variant_property", args);
    let args = json!({ "node": set.to_wire(), "property": "State" });
    edit(&mut h, &mut doc, &mut ids, "delete_variant_property", args);
    assert!(picks(&doc).is_empty(), "the pick goes with its property");
}

/// A variant added to the template's set switches the row whose cell names it
/// in the same step, though no layer under the list changed.
/// flip: `touched` without its `by_variant` arm → fails at "switches at once".
#[test]
fn a_variant_added_to_the_set_switches_its_row_at_once() {
    let (mut doc, mut ids, _) = blank();
    let mut h = History::default();
    let (_, set, l) = variant_list(&mut doc, &mut ids, &mut h);
    let prop = doc.get(set).unwrap().set().unwrap().props[0].name.clone();
    let args = json!({ "node": set.to_wire(), "property": prop, "value": "Nope" });
    edit(&mut h, &mut doc, &mut ids, "add_variant_value", args);
    let new = edit(
        &mut h,
        &mut doc,
        &mut ids,
        "add_variant",
        json!({ "node": set.to_wire() }),
    )[0];
    assert_eq!(doc.get(new).unwrap().variant(), ["Nope"], "the fixture");
    assert_eq!(row_mains(&doc, l)[2], Some(new), "switches at once");
}

/// A list refuses a layer that is not an instance, and one inside a main.
#[test]
fn only_an_instance_outside_mains_becomes_a_list() {
    let (mut doc, mut ids, page) = blank();
    let (main, _) = row_component(&mut doc, &mut ids, page);
    assert!(
        list::why_not(&doc, main).is_some(),
        "a main is not an instance"
    );
    let inner = doc.get(main).unwrap().children()[0];
    assert!(list::why_not(&doc, inner).is_some());
}

/// A list frame moved into a main is refused after the move, *Create component*
/// on a frame holding a list says why, and a file holding a list inside a main
/// loads with that list dropped.
/// flip: `Document::apply` skipping `list::misplaced` → fails at "refused".
#[test]
fn a_list_frame_stays_out_of_mains() {
    let (mut doc, mut ids, page) = blank();
    let (_, inst) = row_component(&mut doc, &mut ids, page);
    let set = data(&mut ids, &[("A", "1"), ("B", "0")]);
    let res = Resolved::rebuild(&doc);
    let (tx, l) = list::make(&doc, &res, &mut ids, inst, Source::New(set)).unwrap();
    let mut history = History::default();
    commit(&mut history, &mut doc, &mut ids, tx);
    // Another main, so the move breaks only the list's rule, not a main holding
    // an instance of itself.
    let frame = serde_json::to_value(NodeKind::Artboard {
        size: Size::new(300.0, 300.0),
        corner_radii: Default::default(),
        corner_smoothing: 0.0,
    })
    .unwrap();
    let main = run(
        &mut doc,
        &mut ids,
        "create_node",
        json!({ "parent": page.to_wire(), "kind": frame, "name": "Card" }),
    )[0];
    run(
        &mut doc,
        &mut ids,
        "make_component",
        json!({ "nodes": [main.to_wire()] }),
    );

    let into_main = Transaction(vec![Operation::Reparent {
        id: l,
        new_parent: main,
        index: 0,
    }]);
    let err = doc.clone().apply(&into_main).expect_err("refused");
    assert!(matches!(err, mastik_core::OpError::ListPlacement(id) if id == l));

    assert_eq!(
        mastik_core::component::why_not_main(&doc, l),
        Some(mastik_core::component::NotMain::List)
    );

    // A file an earlier build saved with the list inside the main.
    let from = doc.get(l).unwrap().parent().unwrap().to_wire();
    let (l_w, main_w) = (l.to_wire(), main.to_wire());
    let bytes = mastik_core::io::save(&doc).expect("saves");
    let mut value: Value = serde_json::from_slice(&bytes).unwrap();
    let mut moved = 0;
    for node in value["nodes"].as_array_mut().unwrap() {
        let id = node["id"].as_str().unwrap().to_string();
        if id == from {
            node["children"]
                .as_array_mut()
                .unwrap()
                .retain(|c| c.as_str() != Some(l_w.as_str()));
            moved += 1;
        }
        if id == main_w {
            node["children"]
                .as_array_mut()
                .unwrap()
                .insert(0, json!(l_w));
            moved += 1;
        }
        if id == l_w {
            node["parent"] = json!(main_w);
            moved += 1;
        }
    }
    assert_eq!(moved, 3, "the fixture's file was rewritten");
    let back = mastik_core::io::load(&serde_json::to_vec(&value).unwrap()).expect("loads");
    assert_eq!(back.get(l).unwrap().parent(), Some(main));
    assert!(back.get(l).unwrap().list().is_none(), "the list is dropped");
}

/// A data set stays until deleted: removing it is refused while a list reads
/// it, detaching that list keeps it, and it is deleted once nothing reads it.
/// flip: `Document::apply` not refusing a read data set's removal → fails at
/// "refused while read".
#[test]
fn a_data_set_stays_until_deleted() {
    let (mut doc, mut ids, page) = blank();
    let (_, inst) = row_component(&mut doc, &mut ids, page);
    let set = data(&mut ids, &[("A", "1")]);
    let sid = set.id;
    let res = Resolved::rebuild(&doc);
    let (tx, l) = list::make(&doc, &res, &mut ids, inst, Source::New(set)).unwrap();
    let mut history = History::default();
    commit(&mut history, &mut doc, &mut ids, tx);
    let delete = || Transaction(vec![Operation::SetDataSet { id: sid, set: None }]);
    assert!(
        matches!(
            doc.clone().apply(&delete()),
            Err(mastik_core::OpError::DataSetRead(id)) if id == sid
        ),
        "refused while read"
    );
    let detach = list::detach(&doc, &[l]);
    commit(&mut history, &mut doc, &mut ids, detach);
    assert_eq!(doc.data_sets().len(), 1, "kept when its list goes");
    commit(&mut history, &mut doc, &mut ids, delete());
    assert!(doc.data_sets().is_empty(), "deleted once unread");
    history.undo(&mut doc).unwrap();
    assert_eq!(doc.data_sets().len(), 1, "undo brings it back");
}

/// Undoing a data set's removal puts it back at its place, not at the end, and
/// redo removes it again.
/// flip: `SetDataSet`'s removal inverting to an add-at-the-end → fails at
/// "back at its place".
#[test]
fn undoing_a_data_set_removal_restores_its_place() {
    let (mut doc, mut ids, _) = blank();
    let sets: Vec<DataSet> = (0..3).map(|_| data(&mut ids, &[("A", "1")])).collect();
    let order: Vec<_> = sets.iter().map(|s| s.id).collect();
    let add = sets
        .into_iter()
        .map(|s| Operation::SetDataSet {
            id: s.id,
            set: Some(Box::new(s)),
        })
        .collect();
    let mut history = History::default();
    commit(&mut history, &mut doc, &mut ids, Transaction(add));
    let ids_now = |doc: &Document| doc.data_sets().iter().map(|d| d.id).collect::<Vec<_>>();
    assert_eq!(ids_now(&doc), order);
    let delete = Transaction(vec![Operation::SetDataSet {
        id: order[0],
        set: None,
    }]);
    commit(&mut history, &mut doc, &mut ids, delete);
    assert_eq!(ids_now(&doc), order[1..]);
    history.undo(&mut doc).unwrap();
    assert_eq!(ids_now(&doc), order, "back at its place");
    history.redo(&mut doc).unwrap();
    assert_eq!(ids_now(&doc), order[1..], "redo removes it again");
}

/// Tables `[A, B, C]`: deletes table `doomed`, then table `then` (or adds a new
/// table when `then` is `None`), then reverts the first deletion. Answers the
/// tables after the revert, or its refusal, and the three tables' ids.
fn revert_a_table_deletion(
    doomed: usize,
    then: Option<usize>,
) -> (Result<Vec<list::DataSetId>, String>, Vec<list::DataSetId>) {
    let (mut doc, mut ids, _) = blank();
    let sets: Vec<DataSet> = (0..3).map(|_| data(&mut ids, &[("A", "1")])).collect();
    let order: Vec<_> = sets.iter().map(|s| s.id).collect();
    let put = |s: DataSet| Operation::SetDataSet {
        id: s.id,
        set: Some(Box::new(s)),
    };
    let delete = |id| Transaction(vec![Operation::SetDataSet { id, set: None }]);
    let mut history = History::default();
    let add = Transaction(sets.into_iter().map(put).collect());
    commit(&mut history, &mut doc, &mut ids, add);
    commit(&mut history, &mut doc, &mut ids, delete(order[doomed]));
    let first = history.last_serial();
    let later = match then {
        Some(i) => delete(order[i]),
        None => Transaction(vec![put(data(&mut ids, &[("A", "1")]))]),
    };
    commit(&mut history, &mut doc, &mut ids, later);
    let now = history
        .revert(&mut doc, first, StepNote::default())
        .map(|_| doc.data_sets().iter().map(|d| d.id).collect())
        .map_err(|e| e.to_string());
    (now, order)
}

/// Reverting an earlier table deletion under a later step puts the table back
/// where the replay puts it: when the later step removed a table before it (its
/// recorded index now past the end), when it removed the table between, and
/// when it added a table, which the replay redoes into the old index.
/// flip: `History::revert` applying the step's recorded `InsertDataSet` index
/// (`tables_placed_as` skipped) → fails at "past the end".
#[test]
fn reverting_a_table_deletion_under_a_later_one_puts_it_back() {
    let (now, o) = revert_a_table_deletion(2, Some(0));
    assert_eq!(now, Ok(vec![o[1], o[2]]), "past the end");
    let (now, o) = revert_a_table_deletion(1, Some(0));
    assert_eq!(now, Ok(vec![o[1], o[2]]), "between");
    let (now, o) = revert_a_table_deletion(2, None);
    let now = now.expect("a later table");
    assert_eq!(now.len(), 4, "the later table stays");
    assert_eq!(
        [now[0], now[1], now[3]],
        [o[0], o[1], o[2]],
        "after the later table, as the replay redoes it"
    );
}

/// Tables `[A, B, C, D]`: commits the step `first` builds from their ids, then
/// deletes B, then reverts the first step. Answers the tables after the revert,
/// or its refusal, and the four tables' ids.
fn revert_under_a_deletion(
    first: impl FnOnce(&[list::DataSetId], &mut IdSource) -> Transaction,
) -> (Result<Vec<list::DataSetId>, String>, Vec<list::DataSetId>) {
    let (mut doc, mut ids, _) = blank();
    let sets: Vec<DataSet> = (0..4).map(|_| data(&mut ids, &[("A", "1")])).collect();
    let order: Vec<_> = sets.iter().map(|s| s.id).collect();
    let add = sets
        .into_iter()
        .map(|s| Operation::SetDataSet {
            id: s.id,
            set: Some(Box::new(s)),
        })
        .collect();
    let mut history = History::default();
    commit(&mut history, &mut doc, &mut ids, Transaction(add));
    let step = first(&order, &mut ids);
    commit(&mut history, &mut doc, &mut ids, step);
    let serial = history.last_serial();
    let delete_b = Transaction(vec![Operation::SetDataSet {
        id: order[1],
        set: None,
    }]);
    commit(&mut history, &mut doc, &mut ids, delete_b);
    let now = history
        .revert(&mut doc, serial, StepNote::default())
        .map(|_| doc.data_sets().iter().map(|d| d.id).collect())
        .map_err(|e| e.to_string());
    (now, order)
}

/// Reverting a step that put back two tables places each after the one before
/// it, in either order the step deleted them; and a step that deleted a table
/// and inserted a new one ahead of it reverts to the replay's order.
/// flip: `tables_placed_as` not inserting into `present` → fails at "A and C";
/// its `SetDataSet` arm left out → fails at "the inserted table".
#[test]
fn reverting_a_step_of_several_tables_under_a_later_one_puts_each_back() {
    let gone = |id| Operation::SetDataSet { id, set: None };
    for flip in [false, true] {
        let (now, o) = revert_under_a_deletion(|o, _| {
            let mut ops = vec![gone(o[0]), gone(o[2])];
            if flip {
                ops.reverse();
            }
            Transaction(ops)
        });
        assert_eq!(now, Ok(vec![o[0], o[2], o[3]]), "A and C, reversed {flip}");
    }
    let (now, o) = revert_under_a_deletion(|o, ids| {
        Transaction(vec![
            gone(o[2]),
            Operation::InsertDataSet {
                set: Box::new(data(ids, &[("A", "1")])),
                index: 0,
            },
        ])
    });
    assert_eq!(now, Ok(vec![o[0], o[2], o[3]]), "the inserted table");
}

/// A picture `n` (its bytes `[n]`) added to the document; answers its id.
fn picture(doc: &mut Document, n: u8) -> mastik_core::ImageId {
    let id = mastik_core::ImageId(format!("sha256:{n:064x}"));
    doc.apply(&Transaction(vec![Operation::AddImage {
        id: id.clone(),
        entry: mastik_core::ImageEntry {
            source: mastik_core::ImageSource::Embedded(vec![n].into()),
            format: mastik_core::ImageFormat::Png,
            width: 4,
            height: 4,
        },
    }]))
    .unwrap();
    id
}

/// The picture each row of list `l` shows, the template first.
fn pictures(doc: &Document, l: NodeId) -> Vec<mastik_core::ImageId> {
    let copies = doc.get(l).unwrap().list().unwrap().copies.clone();
    std::iter::once(list::template(doc, l).unwrap())
        .chain(copies)
        .map(|r| {
            mastik_core::build::subtree_nodes(doc, &[r])
                .into_iter()
                .find_map(|id| {
                    let (_, i) = mastik_core::variant::image_of(doc.get(id)?)?;
                    Some(i.id.clone())
                })
                .unwrap()
        })
        .collect()
}

/// An image property drives the picture of its layer's image fill, keeping the
/// fill's framing; a list's column fills it from a cell naming a picture's id
/// or a cell the data set read a picture for, and a cell naming none takes the
/// main's picture, however many times the list was made before, and is reported.
/// flip: `regenerate` skipping the `PropKind::Image` arm → fails at "by id"; an
/// unmatched cell leaving the captured picture → fails at "made again".
#[test]
fn a_column_of_pictures_fills_an_image_property() {
    let (mut doc, mut ids, page) = blank();
    let (a, b, c) = (
        picture(&mut doc, 1),
        picture(&mut doc, 2),
        picture(&mut doc, 3),
    );
    let frame = serde_json::to_value(NodeKind::Artboard {
        size: Size::new(200.0, 40.0),
        corner_radii: Default::default(),
        corner_smoothing: 0.0,
    })
    .unwrap();
    let rect = serde_json::to_value(NodeKind::Rect {
        size: Size::new(20.0, 20.0),
        corner_radii: Default::default(),
        corner_smoothing: 0.0,
    })
    .unwrap();
    let main = run(
        &mut doc,
        &mut ids,
        "create_node",
        json!({ "parent": page.to_wire(), "kind": frame, "name": "Card" }),
    )[0];
    let thumb = run(
        &mut doc,
        &mut ids,
        "create_node",
        json!({ "parent": main.to_wire(), "kind": rect }),
    )[0];
    let mut fill = mastik_core::Fill {
        brush: mastik_core::BrushSource::Literal(mastik_core::image_brush(a.clone())),
        visible: true,
    };
    if let mastik_core::BrushSource::Literal(mastik_core::Brush::Image(img)) = &mut fill.brush {
        img.image.fit = mastik_core::ImageFit::Fit;
    }
    doc.apply(&Transaction(vec![Operation::SetFills {
        id: thumb,
        fills: vec![mastik_core::Keyed::new(ids.mint_item(), fill)],
    }]))
    .unwrap();
    run(
        &mut doc,
        &mut ids,
        "make_component",
        json!({ "nodes": [main.to_wire()] }),
    );
    run(
        &mut doc,
        &mut ids,
        "define_property",
        json!({ "node": thumb.to_wire(), "kind": "image", "name": "Photo" }),
    );
    let inst = run(
        &mut doc,
        &mut ids,
        "create_instance",
        json!({ "main": main.to_wire(), "parent": page.to_wire(), "x": 400, "y": 0 }),
    )[0];
    let set = DataSet {
        id: ids.mint_data_set(),
        name: "cards.csv".into(),
        table: Table {
            columns: vec![Column {
                id: ColumnId(0),
                name: "photo".into(),
            }],
            rows: vec![
                vec![b.0.clone()],
                vec!["img/c.png".into()],
                vec!["missing.png".into()],
            ],
        },
        source: None,
        pictures: vec![("img/c.png".into(), c.clone())],
    };
    let res = Resolved::rebuild(&doc);
    let (tx, l) = list::make(&doc, &res, &mut ids, inst, Source::New(set)).unwrap();
    let mut history = History::default();
    commit(&mut history, &mut doc, &mut ids, tx);

    let shown = pictures(&doc, l);
    assert_eq!(shown[0], b, "by id");
    assert_eq!(shown[1], c, "by the data set's read");
    assert_eq!(shown[2], a, "the main's own");
    regenerate_again(&mut history, &mut doc, &mut ids, l);
    assert_eq!(
        pictures(&doc, l),
        [b.clone(), c.clone(), a.clone()],
        "made again"
    );
    assert_eq!(
        list::unmatched_pictures(&doc, l),
        [("Photo".to_string(), vec!["missing.png".to_string()])]
    );
    let copy = doc.get(l).unwrap().list().unwrap().copies[0];
    let framed = mastik_core::build::subtree_nodes(&doc, &[copy])
        .into_iter()
        .find_map(|id| mastik_core::variant::image_of(doc.get(id)?).map(|(_, r)| r.fit))
        .unwrap();
    assert_eq!(framed, mastik_core::ImageFit::Fit, "the framing kept");
}

/// A chart over a data set takes the set's table at the commit that edits the
/// set, a renamed series kept by its column id beside one that was not, in the
/// edit's own undo step; a chart back on its own table keeps the rows and is
/// left alone by later edits of the set.
/// flip: `regenerate_in_history` skipping `chart_follow_ops` → fails at
/// "follows the edit"; `Chart::reading` matching by name on its own set →
/// fails at "kept through the rename".
#[test]
fn a_chart_reads_a_data_set_and_follows_its_edits() {
    let (mut doc, mut ids, page) = blank();
    let chart = run(
        &mut doc,
        &mut ids,
        "insert_chart",
        json!({ "parent": page.to_wire(), "kind": "column" }),
    )[0];
    let mut set = data(&mut ids, &[("Linen", "4"), ("Wool", "7")]);
    set.table.columns[0].name = "Category".into();
    set.table.columns[1].name = "Sales".into();
    set.table.columns.push(Column {
        id: ColumnId(2),
        name: "Returns".into(),
    });
    for r in &mut set.table.rows {
        r.push("1".into());
    }
    let mut history = History::default();
    let was = doc.get(chart).unwrap().chart().unwrap().clone();
    let onto = was.reading(set.id, &set.table);
    let sales = set.table.columns[1].id;
    let costs = set.table.columns[2].id;
    assert_eq!(
        onto.mapping.series,
        [sales, costs],
        "the number columns are guessed series"
    );
    commit(
        &mut history,
        &mut doc,
        &mut ids,
        Transaction(vec![
            Operation::SetDataSet {
                id: set.id,
                set: Some(Box::new(set.clone())),
            },
            Operation::SetChart {
                id: chart,
                chart: Some(Box::new(onto)),
            },
        ]),
    );
    assert_eq!(doc.get(chart).unwrap().chart().unwrap().data, Some(set.id));

    // A cell edited and the series column renamed in the set.
    let before = doc.clone();
    let mut edited = set.clone();
    edited.table.rows[1][1] = "9".into();
    edited.table.columns[1].name = "Units".into();
    commit(
        &mut history,
        &mut doc,
        &mut ids,
        Transaction(vec![Operation::SetDataSet {
            id: set.id,
            set: Some(Box::new(edited.clone())),
        }]),
    );
    let c = doc.get(chart).unwrap().chart().unwrap();
    assert_eq!(c.table, edited.table, "follows the edit");
    assert_eq!(
        c.mapping.series,
        [sales, costs],
        "the series kept through the rename"
    );
    assert!(history.undo(&mut doc).is_ok());
    assert!(
        doc == before,
        "one undo takes the edit and the chart's copy back"
    );
    assert!(history.redo(&mut doc).is_ok());

    // Back on its own table: it keeps the rows, and the set's edits pass it by.
    let own = mastik_core::chart::Chart {
        data: None,
        ..doc.get(chart).unwrap().chart().unwrap().clone()
    };
    commit(
        &mut history,
        &mut doc,
        &mut ids,
        Transaction(vec![Operation::SetChart {
            id: chart,
            chart: Some(Box::new(own)),
        }]),
    );
    let mut again = edited.clone();
    again.table.rows[0][1] = "1".into();
    commit(
        &mut history,
        &mut doc,
        &mut ids,
        Transaction(vec![Operation::SetDataSet {
            id: set.id,
            set: Some(Box::new(again)),
        }]),
    );
    let c = doc.get(chart).unwrap().chart().unwrap();
    assert_eq!(c.table, edited.table, "its own table, as it was");
}

/// A list frame holds one instance besides its copies: deleting the template,
/// or placing a layer ahead of it to become the template, is refused.
/// flip: `Document::apply` skipping `list::bad_templates` → fails at "deleting
/// the template".
#[test]
fn a_list_keeps_its_one_instance_template() {
    let (mut doc, mut ids, page) = blank();
    let (_, inst) = row_component(&mut doc, &mut ids, page);
    let set = data(&mut ids, &[("Linen", "yes"), ("Wool", "no")]);
    let res = Resolved::rebuild(&doc);
    let (tx, l) = list::make(&doc, &res, &mut ids, inst, Source::New(set)).expect("makes");
    let mut history = History::default();
    commit(&mut history, &mut doc, &mut ids, tx);
    assert_eq!(
        doc.get(l).unwrap().list().unwrap().copies.len(),
        1,
        "the fixture: one copy"
    );

    let delete = Transaction(vec![Operation::DeleteNode { id: inst }]);
    let err = doc
        .clone()
        .apply(&delete)
        .expect_err("deleting the template");
    assert!(matches!(err, mastik_core::OpError::ListTemplate(id) if id == l));

    let rect = serde_json::to_value(NodeKind::Rect {
        size: Size::new(10.0, 10.0),
        corner_radii: Default::default(),
        corner_smoothing: 0.0,
    })
    .unwrap();
    let stray = run(
        &mut doc,
        &mut ids,
        "create_node",
        json!({ "parent": page.to_wire(), "kind": rect }),
    )[0];
    let ahead = Transaction(vec![Operation::Reparent {
        id: stray,
        new_parent: l,
        index: 0,
    }]);
    let err = doc
        .clone()
        .apply(&ahead)
        .expect_err("a layer ahead of the template");
    assert!(matches!(err, mastik_core::OpError::ListTemplate(id) if id == l));
}

/// Edits of one shape on a list's template fold into one step, the rows they
/// regenerate included, and one undo takes them all back.
/// flip: `regenerate_in_history` skipping `resume_run` → fails at "one step".
#[test]
fn a_run_of_template_edits_is_one_step() {
    let (mut doc, mut ids, page) = blank();
    let (_, inst) = row_component(&mut doc, &mut ids, page);
    let set = data(&mut ids, &[("Linen", "yes"), ("Wool", "no")]);
    let res = Resolved::rebuild(&doc);
    let (tx, l) = list::make(&doc, &res, &mut ids, inst, Source::New(set)).expect("makes");
    let mut history = History::default();
    commit(&mut history, &mut doc, &mut ids, tx);
    let made = doc.clone();
    let first_copy = doc.get(l).unwrap().list().unwrap().copies[0];
    for name in ["Row A", "Row B"] {
        let tx = Transaction(vec![Operation::SetName {
            id: inst,
            name: name.into(),
        }]);
        let mut dirty = history
            .commit_into_run(&mut doc, tx.clone())
            .expect("commits");
        list::regenerate_in_history(&mut history, &mut doc, &mut ids, &tx, &mut dirty)
            .expect("regenerates");
    }
    let copy = doc.get(l).unwrap().list().unwrap().copies[0];
    assert_ne!(
        copy, first_copy,
        "the fixture: the edits regenerated the rows"
    );
    assert!(history.undo(&mut doc).is_ok());
    assert!(doc == made, "one step");
}

/// A pasted chart reading a data set takes the set's table at the paste,
/// though it was copied before the set was edited.
/// flip: `chart_follow_ops` ignoring `dirty` → fails at "the set's table".
#[test]
fn a_pasted_chart_takes_its_data_sets_table() {
    let (mut doc, mut ids, page) = blank();
    let chart = run(
        &mut doc,
        &mut ids,
        "insert_chart",
        json!({ "parent": page.to_wire(), "kind": "column" }),
    )[0];
    let mut set = data(&mut ids, &[("Linen", "4"), ("Wool", "7")]);
    set.table.columns[1].name = "Sales".into();
    let mut history = History::default();
    let onto = doc
        .get(chart)
        .unwrap()
        .chart()
        .unwrap()
        .reading(set.id, &set.table);
    commit(
        &mut history,
        &mut doc,
        &mut ids,
        Transaction(vec![
            Operation::SetDataSet {
                id: set.id,
                set: Some(Box::new(set.clone())),
            },
            Operation::SetChart {
                id: chart,
                chart: Some(Box::new(onto)),
            },
        ]),
    );
    let copied = doc.capture_subtree(chart).expect("captures");
    let mut edited = set.clone();
    edited.table.rows[1][1] = "9".into();
    commit(
        &mut history,
        &mut doc,
        &mut ids,
        Transaction(vec![Operation::SetDataSet {
            id: set.id,
            set: Some(Box::new(edited.clone())),
        }]),
    );
    let (paste, made) = mastik_core::build::insert_subtrees(
        &doc,
        &mut ids,
        &[mastik_core::build::Placement {
            nodes: copied,
            parent: page,
            index: None,
        }],
        mastik_core::kurbo::Vec2::ZERO,
    );
    commit(&mut history, &mut doc, &mut ids, paste);
    let pasted = doc.get(made[0]).unwrap().chart().unwrap();
    assert_eq!(
        pasted.data,
        Some(set.id),
        "the fixture: the copy reads the set"
    );
    assert_eq!(pasted.table, edited.table, "the set's table");
}
