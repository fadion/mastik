//! *One file per row*: an export row reading a table plans a file per row and
//! draws each from a throwaway copy filled from its row.

use mastik_core::chart::{Column, ColumnId, Table};
use mastik_core::kurbo::Size;
use mastik_core::list::{DataSet, DataSetId, RowTarget};
use mastik_core::{
    Document, ExportFormat, ExportScale, ExportSpec, Fill, IdSource, NodeId, NodeKind, Operation,
    PerRow, Resolved, Transaction,
};
use mastik_export::{PlanOptions, plan};

fn text_kind(content: &str) -> NodeKind {
    NodeKind::Text {
        content: content.into(),
        style: Box::new(mastik_core::TextStyle {
            font_family: "Inter".into(),
            font_size: 12.0,
            ..Default::default()
        }),
        spans: Default::default(),
        para_spans: Default::default(),
        paragraph: Default::default(),
        block: Default::default(),
        sizing: mastik_core::TextSizing::Auto,
        on_path: None,
        on_path_flip: false,
        on_path_offset: 0.0,
    }
}

/// The fixture's layers and table.
struct Fixture {
    doc: Document,
    res: Resolved,
    frame: NodeId,
    name: NodeId,
    talk: NodeId,
    date: NodeId,
    photo: NodeId,
    table: DataSetId,
    columns: Vec<ColumnId>,
}

/// A frame `Certificate` holding a text `Name`, a locked text `Talk`, a text
/// `Date` no column names and a rect `photo` showing a picture, over a table
/// `Speakers` (`Name`, `talk`, `Photo`) of `rows`.
fn fixture(rows: &[[&str; 3]]) -> Fixture {
    let mut ids = IdSource::new(0x9E5);
    let root = ids.mint();
    let mut doc = Document::new(root);
    let (frame, name, talk, date, photo) =
        (ids.mint(), ids.mint(), ids.mint(), ids.mint(), ids.mint());
    let table = ids.mint_data_set();
    let mut t = Table::default();
    for h in ["Name", "talk", "Photo"] {
        let id = t.next_column_id();
        t.columns.push(Column { id, name: h.into() });
    }
    t.rows = rows
        .iter()
        .map(|r| r.iter().map(|c| c.to_string()).collect())
        .collect();
    let columns = t.columns.iter().map(|c| c.id).collect();
    let text = |id, index, content: &str, label: &str| Operation::CreateNode {
        id,
        parent: frame,
        index,
        kind: text_kind(content),
        transform: None,
        name: Some(label.into()),
    };
    doc.apply(&Transaction(vec![
        Operation::CreateNode {
            id: frame,
            parent: root,
            index: 0,
            kind: NodeKind::Artboard {
                size: Size::new(400.0, 300.0),
                corner_radii: Default::default(),
                corner_smoothing: 0.0,
            },
            transform: None,
            name: Some("Certificate".into()),
        },
        text(name, 0, "Placeholder", "Name"),
        text(talk, 1, "Talk title", "Talk"),
        Operation::SetLocked {
            id: talk,
            locked: true,
        },
        text(date, 2, "1 May", "Date"),
        Operation::CreateNode {
            id: photo,
            parent: frame,
            index: 3,
            kind: NodeKind::Rect {
                size: Size::new(40.0, 40.0),
                corner_radii: Default::default(),
                corner_smoothing: 0.0,
            },
            transform: None,
            name: Some("photo".into()),
        },
        Operation::SetFills {
            id: photo,
            fills: mastik_core::keyed_by_position([Fill {
                brush: mastik_core::image_brush(mastik_core::ImageId("seal".into())).into(),
                visible: true,
            }]),
        },
        Operation::SetDataSet {
            id: table,
            set: Some(Box::new(DataSet {
                id: table,
                name: "Speakers".into(),
                table: t,
                source: None,
                pictures: Vec::new(),
            })),
        },
    ]))
    .expect("the fixture");
    let res = Resolved::rebuild(&doc);
    Fixture {
        doc,
        res,
        frame,
        name,
        talk,
        date,
        photo,
        table,
        columns,
    }
}

/// A 2× PNG row reading `table`, named by `name_by`.
fn row_spec(table: DataSetId, name_by: Option<ColumnId>) -> ExportSpec {
    ExportSpec {
        per_row: Some(PerRow {
            name_by,
            ..PerRow::new(table)
        }),
        ..ExportSpec::new(ExportFormat::Png, ExportScale::Times(2.0))
    }
}

fn set_exports(f: &mut Fixture, spec: ExportSpec) {
    f.doc
        .apply(&Transaction(vec![Operation::SetExports {
            id: f.frame,
            exports: mastik_core::keyed_by_position([spec]),
        }]))
        .expect("the exports");
    f.res = Resolved::rebuild(&f.doc);
}

fn paths(f: &Fixture) -> Vec<String> {
    plan(&f.doc, &f.res, &[f.frame], PlanOptions::default())
        .into_iter()
        .map(|p| p.path)
        .collect()
}

/// Twelve rows numbered to two places in the layer's folder, a file each,
/// filled from its own row.
///
/// flip: `plan::row_number` without its padding → fails at `Certificate/1@2x.png`.
#[test]
fn a_row_spec_plans_a_numbered_file_per_row_in_the_layers_folder() {
    let rows: Vec<[&str; 3]> = (0..12).map(|_| ["Ada", "", ""]).collect();
    let mut f = fixture(&rows);
    let spec = row_spec(f.table, None);
    set_exports(&mut f, spec);
    let planned = plan(&f.doc, &f.res, &[f.frame], PlanOptions::default());
    let want: Vec<String> = (1..=12)
        .map(|n| format!("Certificate/{n:02}@2x.png"))
        .collect();
    let got: Vec<String> = planned.iter().map(|p| p.path.clone()).collect();
    assert_eq!(got, want);
    let rows: Vec<Option<usize>> = planned.iter().map(|p| p.row).collect();
    assert_eq!(rows, (0..12).map(Some).collect::<Vec<_>>(), "a row each");
}

/// *Name by* a column names each file by its cell, sanitized; an empty cell
/// falls back to the row number and two equal cells are told apart.
///
/// flip: `plan::row_files` ignoring `name_by` → fails at the names.
#[test]
fn name_by_names_files_by_a_cell_and_falls_back_to_the_row_number() {
    let mut f = fixture(&[
        ["Ada Lovelace", "", ""],
        ["", "", ""],
        ["Grace: Hopper", "", ""],
        ["Ada Lovelace", "", ""],
    ]);
    let spec = row_spec(f.table, Some(f.columns[0]));
    set_exports(&mut f, spec);
    assert_eq!(
        paths(&f),
        [
            "Certificate/Ada Lovelace@2x.png",
            "Certificate/2@2x.png",
            "Certificate/Grace- Hopper@2x.png",
            "Certificate/Ada Lovelace@2x (2).png",
        ]
    );
}

/// A table gone from the document leaves the layer one file; a table with no
/// rows plans none.
///
/// flip: `plan` treating a missing table as no rows → fails at "one file".
#[test]
fn a_gone_table_plans_one_file_and_an_empty_one_none() {
    let mut f = fixture(&[]);
    let spec = row_spec(f.table, None);
    set_exports(&mut f, spec);
    assert!(paths(&f).is_empty(), "no rows, no file");
    let gone = DataSetId(IdSource::new(0xD1E).mint());
    set_exports(&mut f, row_spec(gone, None));
    assert_eq!(paths(&f), ["Certificate@2x.png"], "one file");
}

/// A plain frame fills its unlocked, shown text and picture layers named like
/// a column, ignoring case, spaces, dashes and underscores; the locked `Talk`
/// and the unnamed `Date` are left out, and so is a hidden layer.
///
/// flip: `list::row_targets` not skipping locked layers → fails at the targets.
#[test]
fn a_plain_frame_fills_the_layers_named_like_a_column() {
    let mut f = fixture(&[["Ada", "Engines", "ada.jpg"]]);
    let table = f.doc.data_set(f.table).expect("the table").table.clone();
    let targets = mastik_core::list::row_targets(&f.doc, f.frame, &table);
    assert_eq!(
        targets,
        [
            RowTarget::Layer {
                node: f.name,
                column: f.columns[0]
            },
            RowTarget::Layer {
                node: f.photo,
                column: f.columns[2]
            },
        ]
    );
    let _ = (f.talk, f.date);
    f.doc
        .apply(&Transaction(vec![Operation::SetVisible {
            id: f.name,
            visible: false,
        }]))
        .expect("hide");
    let targets = mastik_core::list::row_targets(&f.doc, f.frame, &table);
    assert_eq!(targets.len(), 1, "a hidden layer is left out: {targets:?}");
}

/// A row's SVG shows its cell in the text named `Name`, the next row's its
/// own, and the document keeps its placeholder; a cell naming no picture is
/// counted and leaves the layer's fill.
///
/// flip: `plan::render` drawing `file.row`'s layer unfilled → fails at "row 1's name".
#[test]
fn a_rows_file_is_drawn_from_a_filled_copy_and_the_document_is_untouched() {
    let mut f = fixture(&[["Ada Lovelace", "", "nowhere.jpg"], ["Alan Turing", "", ""]]);
    let spec = ExportSpec {
        per_row: Some(PerRow::new(f.table)),
        ..ExportSpec::new(ExportFormat::Svg, ExportScale::Times(1.0))
    };
    set_exports(&mut f, spec.clone());
    let planned = plan(&f.doc, &f.res, &[f.frame], PlanOptions::default());
    let svg = |i: usize| {
        let bytes = mastik_export::plan::render(&f.doc, &f.res, &planned[i]).expect("bytes");
        String::from_utf8(bytes).expect("utf-8")
    };
    let (first, second) = (svg(0), svg(1));
    assert!(first.contains("Ada Lovelace"), "row 1's name: {first}");
    assert!(!first.contains("Placeholder"), "{first}");
    assert!(second.contains("Alan Turing"), "row 2's name: {second}");
    let Some(NodeKind::Text { content, .. }) = f.doc.get(f.name).map(|n| n.kind().clone()) else {
        panic!("the text");
    };
    assert_eq!(content, "Placeholder", "the document keeps its own text");
    let noted = |i: usize| mastik_export::plan::render_noted(&f.doc, &f.res, &planned[i]).1;
    assert_eq!(
        (noted(0).missing_pictures, noted(1).missing_pictures),
        (1, 0),
        "the report hears of row 1's missing picture"
    );

    let (copy, _, filled) =
        mastik_export::plan::filled_copy(&f.doc, f.frame, &spec, 0).expect("a table");
    assert_eq!(filled.missing_pictures, 1, "nowhere.jpg names no picture");
    assert_eq!(
        copy.get(f.photo).map(|n| n.paint().fills.clone()),
        f.doc.get(f.photo).map(|n| n.paint().fills.clone()),
        "and the layer keeps its fill"
    );
}

/// A PDF row writing its rows as pages plans one file, named as the layer's
/// one file is, of a page per row, each bookmarked; an empty table plans none.
///
/// flip: `plan::row_pages` drawing only row 0 → fails at the page count.
#[test]
fn one_pdf_holds_a_page_per_row() {
    let mut f = fixture(&[
        ["Ada Lovelace", "", ""],
        ["Alan Turing", "", ""],
        ["Grace Hopper", "", ""],
    ]);
    let spec = ExportSpec {
        per_row: Some(PerRow {
            name_by: Some(f.columns[0]),
            rows_as: mastik_core::RowsAs::Pages,
            ..PerRow::new(f.table)
        }),
        ..ExportSpec::new(ExportFormat::Pdf, ExportScale::Times(1.0))
    };
    set_exports(&mut f, spec.clone());
    let planned = plan(&f.doc, &f.res, &[f.frame], PlanOptions::default());
    let named: Vec<_> = planned.iter().map(|p| (p.path.as_str(), p.row)).collect();
    assert_eq!(named, [("Certificate.pdf", None)]);
    let bytes = mastik_export::plan::render(&f.doc, &f.res, &planned[0]).expect("a pdf");
    let text = String::from_utf8_lossy(&bytes);
    let pages = text.matches("/Type/Page").count() - text.matches("/Type/Pages").count();
    assert_eq!(pages, 3, "a page per row");
    assert_eq!(text.matches("/Dest").count(), 3, "a bookmark each");

    let mut empty = fixture(&[]);
    set_exports(&mut empty, spec);
    assert!(paths(&empty).is_empty(), "no rows, no file");
}

/// A row whose name no longer fits its fixed box is marked as overflowing;
/// a short one is not.
///
/// flip: `plan::filled_copy` leaving `overflowing` empty → fails at "the long name".
#[test]
fn a_row_whose_text_outgrows_its_fixed_box_is_marked() {
    let long = "Ada Augusta King, Countess of Lovelace, who wrote the first program";
    let mut f = fixture(&[["Ada", "", ""], [long, "", ""]]);
    f.doc
        .apply(&Transaction(vec![Operation::SetGeometry {
            id: f.name,
            geometry: mastik_core::GeometryPatch::TextSizing(mastik_core::TextSizing::Fixed(
                Size::new(60.0, 16.0),
            )),
        }]))
        .expect("a fixed box");
    let spec = row_spec(f.table, None);
    set_exports(&mut f, spec.clone());
    let overflows = |row: usize| {
        let filled = mastik_export::plan::filled_copy(&f.doc, f.frame, &spec, row)
            .expect("a table")
            .2;
        filled.overflowing == [row]
    };
    assert!(!overflows(0), "a short name fits");
    assert!(overflows(1), "the long name");
}
