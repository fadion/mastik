//! Lists from data (`list`): a list made from an instance repeats it once per
//! row, each copy's properties filled from its row, and the rows follow an edit
//! of the template, the data or the description inside that edit's undo step.

use mastik_core::chart::{Column, ColumnId, Table};
use mastik_core::kurbo::Size;
use mastik_core::list::{self, DataSet, Rows, Source};
use mastik_core::verbs::{self, Ctx, Done};
use mastik_core::{
    Document, History, IdSource, NodeId, NodeKind, Operation, Resolved, Transaction,
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
/// data set and every row away, and redo brings them back.
/// flip: `regenerate_in_history` skipping `join_last` → fails at "one undo".
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

/// A column named after a variant property switches each row to the variant
/// its cell names, and a cell naming no variant leaves the row as the template
/// shows it.
/// flip: `regenerate` skipping the variant pass → fails at "switches".
#[test]
fn a_variant_column_switches_each_row() {
    let (mut doc, mut ids, page) = blank();
    let frame = serde_json::to_value(NodeKind::Artboard {
        size: Size::new(100.0, 40.0),
        corner_radii: Default::default(),
        corner_smoothing: 0.0,
    })
    .unwrap();
    let mut mains = Vec::new();
    for name in ["Paid", "Due"] {
        let m = run(
            &mut doc,
            &mut ids,
            "create_node",
            json!({ "parent": page.to_wire(), "kind": frame, "name": name }),
        )[0];
        run(
            &mut doc,
            &mut ids,
            "make_component",
            json!({ "nodes": [m.to_wire()] }),
        );
        mains.push(m);
    }
    let set = run(
        &mut doc,
        &mut ids,
        "combine_as_variants",
        json!({ "nodes": mains.iter().map(|m| m.to_wire()).collect::<Vec<_>>() }),
    )[0];
    let prop = doc.get(set).unwrap().set().unwrap().props[0].clone();
    let value_of = |m: NodeId, doc: &Document| doc.get(m).unwrap().variant()[0].clone();
    let (paid, due) = (value_of(mains[0], &doc), value_of(mains[1], &doc));
    let inst = run(
        &mut doc,
        &mut ids,
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
    let res = Resolved::rebuild(&doc);
    let (tx, l) = list::make(&doc, &res, &mut ids, inst, Source::New(data)).unwrap();
    let mut history = History::default();
    commit(&mut history, &mut doc, &mut ids, tx);
    let rows: Vec<NodeId> = std::iter::once(inst)
        .chain(doc.get(l).unwrap().list().unwrap().copies.clone())
        .collect();
    let shown: Vec<Option<NodeId>> = rows
        .iter()
        .map(|r| mastik_core::component::main_of(&doc, *r))
        .collect();
    assert_eq!(
        shown,
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
/// or a cell the data set read a picture for, and a cell naming none leaves the
/// template's picture and is reported.
/// flip: `regenerate` skipping the `PropKind::Image` arm → fails at "by id".
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
    assert_eq!(shown[2], a, "the template's own");
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
