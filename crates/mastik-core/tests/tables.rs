//! The table verbs (`verbs::tables`): a table named by id or name, edited one
//! step at a time with every list and chart over it following, and the edits a
//! linked, shared or read table refuses.

use mastik_core::chart::{DataLink, Table};
use mastik_core::kurbo::Size;
use mastik_core::list::{self, DataSetId, Rows};
use mastik_core::verbs::{self, Ctx, Done, VerbError};
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

/// What verb `name` answers on `doc`, committing nothing.
fn answer(doc: &Document, ids: &mut IdSource, name: &str, args: Value) -> Result<Done, VerbError> {
    let res = Resolved::rebuild(doc);
    verbs::call(
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
}

/// The words verb `name` is refused with.
fn refused(doc: &Document, ids: &mut IdSource, name: &str, args: Value) -> String {
    match answer(doc, ids, name, args) {
        Ok(_) => panic!("{name} is not refused"),
        Err(e) => e.to_string(),
    }
}

/// Run verb `name` and commit it as the app does, the lists and charts it left
/// behind brought up to it in the same step; answers the verb's answer.
fn run(
    history: &mut History,
    doc: &mut Document,
    ids: &mut IdSource,
    name: &str,
    args: Value,
) -> Done {
    let mut done = answer(doc, ids, name, args).unwrap_or_else(|e| panic!("{name}: {e}"));
    let mut tx = std::mem::take(&mut done.tx);
    mastik_core::propagate::owed(doc, &mut tx, ids);
    let follow = list::follow_ops(&tx);
    let mut dirty = history.commit(doc, tx).expect("commits");
    list::regenerate_in_history(history, doc, ids, &follow, &mut dirty).expect("regenerates");
    done
}

/// The first layer verb `name` created.
fn made(
    history: &mut History,
    doc: &mut Document,
    ids: &mut IdSource,
    name: &str,
    args: Value,
) -> NodeId {
    run(history, doc, ids, name, args).created[0]
}

/// A table `People` with columns Name and Age over `rows`; answers its id.
fn people(history: &mut History, doc: &mut Document, ids: &mut IdSource, rows: Value) -> DataSetId {
    let done = run(
        history,
        doc,
        ids,
        "add_table",
        json!({ "name": "People", "columns": ["Name", "Age"], "rows": rows }),
    );
    let id = NodeId::from_wire(done.made.as_deref().unwrap()).unwrap();
    DataSetId(id)
}

/// The cells of table `set`.
fn cells(doc: &Document, set: DataSetId) -> Vec<Vec<String>> {
    doc.data_set(set).unwrap().table.rows.clone()
}

/// A main `Row` whose text is the text property `Name`, and an instance of it
/// on the page; answers the instance.
fn row_instance(
    history: &mut History,
    doc: &mut Document,
    ids: &mut IdSource,
    page: NodeId,
) -> NodeId {
    let frame = serde_json::to_value(NodeKind::Artboard {
        size: Size::new(200.0, 40.0),
        corner_radii: Default::default(),
        corner_smoothing: 0.0,
    })
    .unwrap();
    let main = made(
        history,
        doc,
        ids,
        "create_node",
        json!({ "parent": page.to_wire(), "kind": frame, "name": "Row" }),
    );
    let text = made(
        history,
        doc,
        ids,
        "create_text",
        json!({ "parent": main.to_wire(), "text": "Name" }),
    );
    run(
        history,
        doc,
        ids,
        "make_component",
        json!({ "nodes": [main.to_wire()] }),
    );
    run(
        history,
        doc,
        ids,
        "define_property",
        json!({ "node": text.to_wire(), "kind": "text", "name": "Name" }),
    );
    made(
        history,
        doc,
        ids,
        "create_instance",
        json!({ "main": main.to_wire(), "parent": page.to_wire(), "x": 400, "y": 0 }),
    )
}

/// The text each row of list `l` draws, the template first.
fn titles(doc: &Document, l: NodeId) -> Vec<String> {
    let copies = doc.get(l).unwrap().list().unwrap().copies.clone();
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

/// `add_table` with nothing makes *New table*'s *Table 1*, one column and one
/// empty row; a taken name is numbered past and said.
/// flip: `add_table` not taking `list::free_name` → fails at "Table 2".
#[test]
fn add_table_makes_a_new_table() {
    let (mut doc, mut ids, _) = blank();
    let mut h = History::default();
    let first = run(&mut h, &mut doc, &mut ids, "add_table", json!({}));
    let id = DataSetId(NodeId::from_wire(first.made.as_deref().unwrap()).unwrap());
    let set = doc.data_set(id).unwrap();
    assert_eq!(set.name, "Table 1");
    assert_eq!(set.table.columns[0].name, "Column 1");
    assert_eq!(set.table.rows, vec![vec![String::new()]]);
    let second = run(&mut h, &mut doc, &mut ids, "add_table", json!({}));
    let id = NodeId::from_wire(second.made.as_deref().unwrap()).unwrap();
    assert_eq!(doc.data_set(DataSetId(id)).unwrap().name, "Table 2");
    assert!(second.said.unwrap().contains("Table 2"));
    let wide: Vec<String> = (0..65).map(|i| format!("C{i}")).collect();
    let why = refused(&doc, &mut ids, "add_table", json!({ "columns": wide }));
    assert!(why.contains("At most 64 columns"), "{why}");
}

/// `edit_table` sets cells, adds rows at a place, deletes rows and edits
/// columns in one undo step, rows numbered as the table stood, and says the
/// new row count; one undo takes it all back.
/// flip: `edit_table` putting the added rows after row `at` rather than before
/// it → fails at the rows.
#[test]
fn edit_table_edits_in_one_step() {
    let (mut doc, mut ids, _) = blank();
    let mut h = History::default();
    let set = people(
        &mut h,
        &mut doc,
        &mut ids,
        json!([["Ana", "31"], ["Ben", "40"], ["Cy", "22"]]),
    );
    let before = doc.clone();
    let done = run(
        &mut h,
        &mut doc,
        &mut ids,
        "edit_table",
        json!({
            "table": "People",
            "columns": { "rename": { "Age": "Years" }, "add": ["City"] },
            "set": [{ "row": 3, "column": "Years", "value": 23 }],
            "add": [{ "Name": "Dee", "City": "Oslo" }],
            "at": 2,
            "delete": [1],
        }),
    );
    assert_eq!(
        cells(&doc, set),
        vec![
            vec!["Dee", "", "Oslo"],
            vec!["Ben", "40", ""],
            vec!["Cy", "23", ""],
        ]
    );
    let t = &doc.data_set(set).unwrap().table;
    assert_eq!(t.columns[1].name, "Years");
    assert_eq!(done.said.as_deref(), Some("People: 3 rows"));
    assert!(h.undo(&mut doc).is_ok());
    assert!(doc == before, "one undo takes the edit back");
}

/// Every cell edit of a linked table is refused naming its file; a row past
/// the end is refused with the row count; a name matching no table, or two,
/// is refused listing the tables.
/// flip: `edit_table` without `refuse_linked` → fails at "unlink it first".
#[test]
fn table_edits_are_refused_in_words() {
    let (mut doc, mut ids, _) = blank();
    let mut h = History::default();
    let set = people(
        &mut h,
        &mut doc,
        &mut ids,
        json!([["Ana", "31"], ["Ben", "40"]]),
    );
    let row = json!({ "table": "People", "set": [{ "row": 14, "column": "Name", "value": "X" }] });
    let why = refused(&doc, &mut ids, "edit_table", row);
    assert!(why.contains("People has 2 rows, not 14"), "{why}");

    let why = refused(
        &doc,
        &mut ids,
        "edit_table",
        json!({ "table": "Nobody", "delete": [1] }),
    );
    assert!(
        why.contains("no table is called `Nobody`") && why.contains("`People`"),
        "{why}"
    );
    run(
        &mut h,
        &mut doc,
        &mut ids,
        "add_table",
        json!({ "name": "Twin" }),
    );
    let mut twin = doc.data_sets().last().unwrap().clone();
    twin.id = ids.mint_data_set();
    let tx = Transaction(vec![Operation::SetDataSet {
        id: twin.id,
        set: Some(Box::new(twin.clone())),
    }]);
    h.commit(&mut doc, tx).unwrap();
    let why = refused(&doc, &mut ids, "delete_table", json!({ "table": "Twin" }));
    assert!(
        why.contains("2 tables are called `Twin`: give its id"),
        "{why}"
    );
    assert!(
        why.contains(&twin.id.0.to_wire()),
        "lists the tables: {why}"
    );

    let mut linked = doc.data_set(set).unwrap().clone();
    linked.source = Some(DataLink {
        path: "C:/data/people.csv".into(),
        size: 0,
        modified: 0,
        header: None,
    });
    let tx = Transaction(vec![Operation::SetDataSet {
        id: set,
        set: Some(Box::new(linked)),
    }]);
    h.commit(&mut doc, tx).unwrap();
    let why = refused(
        &doc,
        &mut ids,
        "edit_table",
        json!({ "table": "People", "delete": [1] }),
    );
    assert!(
        why.contains("People is linked to people.csv: unlink it first"),
        "{why}"
    );
    run(
        &mut h,
        &mut doc,
        &mut ids,
        "unlink_table",
        json!({ "table": "People" }),
    );
    assert!(doc.data_set(set).unwrap().source.is_none());
    run(
        &mut h,
        &mut doc,
        &mut ids,
        "edit_table",
        json!({ "table": "People", "delete": [1] }),
    );
    assert_eq!(cells(&doc, set), vec![vec!["Ben", "40"]]);
}

/// A list made over a table by `make_list` follows `edit_table`, and
/// `delete_table` is refused while it reads the table, naming its readers.
/// flip: `delete_table` not counting readers → fails at "used by 1 list".
#[test]
fn a_list_follows_its_table_and_holds_it() {
    let (mut doc, mut ids, page) = blank();
    let mut h = History::default();
    let inst = row_instance(&mut h, &mut doc, &mut ids, page);
    let set = people(
        &mut h,
        &mut doc,
        &mut ids,
        json!([["Ana", "31"], ["Ben", "40"]]),
    );
    let l = made(
        &mut h,
        &mut doc,
        &mut ids,
        "make_list",
        json!({ "node": inst.to_wire(), "table": set.0.to_wire() }),
    );
    assert_eq!(titles(&doc, l), ["Ana", "Ben"]);
    let add = json!({ "table": "People", "add": [{ "Name": "Cy" }] });
    run(&mut h, &mut doc, &mut ids, "edit_table", add);
    assert_eq!(titles(&doc, l), ["Ana", "Ben", "Cy"]);

    let why = refused(&doc, &mut ids, "delete_table", json!({ "table": "People" }));
    assert!(
        why.contains("Delete table: used by 1 list. Point them at another table first."),
        "{why}"
    );
    run(
        &mut h,
        &mut doc,
        &mut ids,
        "add_table",
        json!({ "name": "Spare" }),
    );
    run(
        &mut h,
        &mut doc,
        &mut ids,
        "delete_table",
        json!({ "table": "Spare" }),
    );
    assert!(doc.data_sets().iter().all(|d| d.name != "Spare"));

    let why = refused(
        &doc,
        &mut ids,
        "make_list",
        json!({ "node": page.to_wire(), "table": "People" }),
    );
    assert!(
        why.contains("page is not a layer") || why.contains("Select an instance"),
        "{why}"
    );
}

/// `set_list` sets the rows mode, a filter reading a variable by name (stored
/// as its id) and a sort, and moves the list onto another table keeping the
/// fill on the column of its name.
/// flip: `set_list` storing the filter value as written → fails at "stored by id".
#[test]
fn set_list_edits_the_list_card() {
    let (mut doc, mut ids, page) = blank();
    let mut h = History::default();
    let inst = row_instance(&mut h, &mut doc, &mut ids, page);
    let set = people(
        &mut h,
        &mut doc,
        &mut ids,
        json!([["Ana", "31"], ["Ben", "40"], ["Cy", "22"]]),
    );
    let l = made(
        &mut h,
        &mut doc,
        &mut ids,
        "make_list",
        json!({ "node": inst.to_wire(), "table": "People", "fill": { "Name": "Name" } }),
    );
    let var = run(
        &mut h,
        &mut doc,
        &mut ids,
        "add_variable",
        json!({ "kind": "number", "name": "limit", "value": 35 }),
    )
    .made
    .unwrap();
    let args = json!({
        "node": l.to_wire(),
        "rows": { "first": 2 },
        "filter": { "tests": [{ "column": "Age", "op": "at most", "value": "{limit}" }] },
        "sort": [{ "column": "Age", "descending": true }],
    });
    run(&mut h, &mut doc, &mut ids, "set_list", args);
    let now = doc.get(l).unwrap().list().unwrap().clone();
    assert_eq!(now.rows, Rows::First(2));
    assert_eq!(
        now.filter.tests[0].value.0,
        format!("{{{var}}}"),
        "stored by id"
    );
    assert_eq!(titles(&doc, l), ["Ana", "Cy"]);

    let other = run(
        &mut h,
        &mut doc,
        &mut ids,
        "add_table",
        json!({ "name": "Staff", "columns": ["Role", "Name"], "rows": [["Lead", "Zoe"]] }),
    );
    let other = other.made.unwrap();
    run(
        &mut h,
        &mut doc,
        &mut ids,
        "set_list",
        json!({ "node": l.to_wire(), "table": other }),
    );
    assert_eq!(titles(&doc, l), ["Zoe"], "the fill moved onto `Name`");
    assert!(doc.get(l).unwrap().list().unwrap().filter.tests.is_empty());
    let _ = set;

    let why = refused(
        &doc,
        &mut ids,
        "set_list",
        json!({ "node": l.to_wire(), "rows": { "row": 0 } }),
    );
    assert!(why.contains("rows count from 1"), "{why}");
}

/// A chart reading a table other readers share refuses the bare `table` form,
/// naming them; `write = "table"` changes the table for every reader, and
/// `write = "chart"` gives the chart its own copy. A chart that is the table's
/// only reader writes it bare.
/// flip: `set_chart_data` writing the bare form whoever reads the table →
/// fails at the refusal.
#[test]
fn set_chart_data_on_a_shared_table() {
    let (mut doc, mut ids, page) = blank();
    let mut h = History::default();
    let inst = row_instance(&mut h, &mut doc, &mut ids, page);
    let set = people(
        &mut h,
        &mut doc,
        &mut ids,
        json!([["Ana", "31"], ["Ben", "40"]]),
    );
    let chart = made(
        &mut h,
        &mut doc,
        &mut ids,
        "insert_chart",
        json!({ "parent": page.to_wire(), "kind": "column" }),
    );
    run(
        &mut h,
        &mut doc,
        &mut ids,
        "set_chart_data",
        json!({ "node": chart.to_wire(), "data": "People" }),
    );
    assert_eq!(doc.get(chart).unwrap().chart().unwrap().data, Some(set));
    let table: Table = serde_json::from_value(json!({
        "columns": [{ "id": 0, "name": "Name" }, { "id": 1, "name": "Age" }],
        "rows": [["Ana", "32"], ["Ben", "41"], ["Cy", "5"]],
    }))
    .unwrap();

    // The chart is the table's only reader: the bare form writes the table.
    let bare = json!({ "node": chart.to_wire(), "table": table });
    run(&mut h, &mut doc, &mut ids, "set_chart_data", bare.clone());
    assert_eq!(cells(&doc, set).len(), 3);
    let c = doc.get(chart).unwrap().chart().unwrap();
    assert_eq!(
        (c.data, c.table.rows.len()),
        (Some(set), 3),
        "the chart follows"
    );

    made(
        &mut h,
        &mut doc,
        &mut ids,
        "make_list",
        json!({ "node": inst.to_wire(), "table": "People" }),
    );
    let why = refused(&doc, &mut ids, "set_chart_data", bare);
    assert!(
        why.contains(
            "This chart reads People, which 1 list also read. Pass write = \"table\" to change \
             People for all of them, or write = \"chart\" to give this chart its own copy."
        ),
        "{why}"
    );
    let mut fewer = table.clone();
    fewer.rows.truncate(1);
    let args = json!({ "node": chart.to_wire(), "table": fewer, "write": "table" });
    run(&mut h, &mut doc, &mut ids, "set_chart_data", args);
    assert_eq!(cells(&doc, set), vec![vec!["Ana", "32"]]);
    let args = json!({ "node": chart.to_wire(), "table": table, "write": "chart" });
    run(&mut h, &mut doc, &mut ids, "set_chart_data", args);
    let c = doc.get(chart).unwrap().chart().unwrap();
    assert_eq!((c.data, c.table.rows.len()), (None, 3), "its own copy");
    assert_eq!(cells(&doc, set).len(), 1, "the table is left");
}

/// `set_table` replaces the cells keeping the id of each column whose name
/// survives; `rename_table` numbers a taken name past.
/// flip: `set_table` without `take_ids_by_name` → fails at "kept by name".
#[test]
fn set_table_and_rename_table() {
    let (mut doc, mut ids, _) = blank();
    let mut h = History::default();
    let set = people(&mut h, &mut doc, &mut ids, json!([["Ana", "31"]]));
    let age = doc.data_set(set).unwrap().table.columns[1].id;
    let args = json!({ "table": "People", "columns": ["Age", "Name"], "rows": [["9", "Kim"]] });
    run(&mut h, &mut doc, &mut ids, "set_table", args);
    let t = &doc.data_set(set).unwrap().table;
    assert_eq!(t.columns[0].id, age, "kept by name");
    assert_eq!(t.rows, vec![vec!["9", "Kim"]]);
    let ragged = json!({ "table": "People", "columns": ["A"], "rows": [["1", "2"]] });
    let why = refused(&doc, &mut ids, "set_table", ragged);
    assert!(why.contains("row 1 has 2 cells, not 1"), "{why}");

    run(
        &mut h,
        &mut doc,
        &mut ids,
        "add_table",
        json!({ "name": "Staff" }),
    );
    let done = run(
        &mut h,
        &mut doc,
        &mut ids,
        "rename_table",
        json!({ "table": "People", "name": "Staff" }),
    );
    assert_eq!(doc.data_set(set).unwrap().name, "Staff 2");
    assert!(done.said.unwrap().contains("Staff 2"));
}
