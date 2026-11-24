//! *One PDF* holds one row's filled copy at a time: its peak memory does not
//! grow with the table. A binary of its own, since the counting allocator
//! counts every thread's allocations and another test running beside it would
//! move the peak.

use std::alloc::{GlobalAlloc, Layout, System};
use std::sync::atomic::{AtomicUsize, Ordering};

use mastik_core::chart::{Column, Table};
use mastik_core::kurbo::Size;
use mastik_core::list::DataSet;
use mastik_core::{
    Document, ExportFormat, ExportScale, ExportSpec, IdSource, NodeKind, Operation, PerRow,
    Resolved, RowsAs, Transaction,
};
use mastik_export::{PlanOptions, plan};

/// The system allocator, keeping the bytes live now and the most live since
/// the last [`reset_peak`].
struct Counting;

static LIVE: AtomicUsize = AtomicUsize::new(0);
static PEAK: AtomicUsize = AtomicUsize::new(0);

unsafe impl GlobalAlloc for Counting {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        // SAFETY: the caller's contract, passed on.
        let p = unsafe { System.alloc(layout) };
        if !p.is_null() {
            let now = LIVE.fetch_add(layout.size(), Ordering::Relaxed) + layout.size();
            PEAK.fetch_max(now, Ordering::Relaxed);
        }
        p
    }

    unsafe fn dealloc(&self, ptr: *mut u8, layout: Layout) {
        // SAFETY: the caller's contract, passed on.
        unsafe { System.dealloc(ptr, layout) };
        LIVE.fetch_sub(layout.size(), Ordering::Relaxed);
    }
}

#[global_allocator]
static ALLOC: Counting = Counting;

fn reset_peak() -> usize {
    let now = LIVE.load(Ordering::Relaxed);
    PEAK.store(now, Ordering::Relaxed);
    now
}

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

/// A frame `Badge` holding a text `Name`, written as one PDF over a table of
/// `rows`, beside a frame of `weight` texts no row fills: the document every
/// row's copy clones.
fn render_peak(rows: usize, weight: usize) -> (usize, Vec<u8>) {
    let mut ids = IdSource::new(0x3E3);
    let root = ids.mint();
    let mut doc = Document::new(root);
    let (badge, name, other) = (ids.mint(), ids.mint(), ids.mint());
    let table = ids.mint_data_set();
    let mut t = Table::default();
    let column = t.next_column_id();
    t.columns.push(Column {
        id: column,
        name: "Name".into(),
    });
    t.rows = (0..rows).map(|r| vec![format!("Person {r}")]).collect();
    let frame = |id, index, label: &str| Operation::CreateNode {
        id,
        parent: root,
        index,
        kind: NodeKind::Artboard {
            size: Size::new(400.0, 300.0),
            corner_radii: Default::default(),
            corner_smoothing: 0.0,
        },
        transform: None,
        name: Some(label.into()),
    };
    let mut ops = vec![
        frame(badge, 0, "Badge"),
        frame(other, 1, "Other"),
        Operation::CreateNode {
            id: name,
            parent: badge,
            index: 0,
            kind: text_kind("Placeholder"),
            transform: None,
            name: Some("Name".into()),
        },
    ];
    for i in 0..weight {
        ops.push(Operation::CreateNode {
            id: ids.mint(),
            parent: other,
            index: i,
            kind: text_kind("A line of text that every copy carries along"),
            transform: None,
            name: None,
        });
    }
    ops.push(Operation::SetDataSet {
        id: table,
        set: Some(Box::new(DataSet {
            id: table,
            name: "People".into(),
            table: t,
            source: None,
            pictures: Vec::new(),
        })),
    });
    ops.push(Operation::SetExports {
        id: badge,
        exports: mastik_core::keyed_by_position([ExportSpec {
            per_row: Some(PerRow {
                rows_as: RowsAs::Pages,
                ..PerRow::new(table)
            }),
            ..ExportSpec::new(ExportFormat::Pdf, ExportScale::Times(1.0))
        }]),
    });
    doc.apply(&Transaction(ops)).expect("the fixture");
    let res = Resolved::rebuild(&doc);
    let planned = plan(&doc, &res, &[badge], PlanOptions::default());
    assert_eq!(planned.len(), 1, "fixture: one PDF");
    let before = reset_peak();
    let bytes = mastik_export::plan::render(&doc, &res, &planned[0]).expect("a pdf");
    (PEAK.load(Ordering::Relaxed) - before, bytes)
}

/// Six times the rows costs well under twice the peak memory: the rows'
/// copies are not all held at once.
///
/// flip: `plan::row_pages` collecting every page's copy before writing any →
/// fails at "the peak grows with the rows".
#[test]
fn one_pdf_holds_one_rows_copy_at_a_time() {
    let (few, small) = render_peak(4, 200);
    let (many, large) = render_peak(24, 200);
    let pages = |b: &[u8]| {
        let text = String::from_utf8_lossy(b);
        text.matches("/Type/Page").count() - text.matches("/Type/Pages").count()
    };
    assert_eq!(
        (pages(&small), pages(&large)),
        (4, 24),
        "fixture: a page per row"
    );
    assert!(
        many < few * 2,
        "the peak grows with the rows: {few} bytes for 4 rows, {many} for 24"
    );
}
