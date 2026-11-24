//! The browser player's arithmetic (`crates/mastik-web/src/shell.rs`), tested here:
//! `mastik-web` is its own workspace and builds only for the page, so its tests
//! cannot run there.

#[path = "../../mastik-web/src/shell.rs"]
mod shell;

use mastik_core::kurbo::{Affine, Point, Rect, RoundedRectRadii, Size};
use mastik_core::prototype::{Flow, FlowRef, PlayExport};
use mastik_core::{Document, IdSource, Keyed, NodeId, NodeKind, Operation, Transaction};
use mastik_play::{Notice, Start};

/// A frame (`frame`), else a group, first under `parent`.
fn layer(id: NodeId, parent: NodeId, frame: bool) -> Operation {
    let kind = if frame {
        NodeKind::Artboard {
            size: Size::new(100.0, 80.0),
            corner_radii: RoundedRectRadii::default(),
            corner_smoothing: 0.0,
        }
    } else {
        NodeKind::Group
    };
    Operation::CreateNode {
        id,
        parent,
        index: 0,
        kind,
        transform: Some(Affine::IDENTITY),
        name: None,
    }
}

/// A document whose first page holds `home` and a group, and whose second page
/// holds `next` and, with `flows`, the flows *One* and *Two* starting there.
struct Opened {
    doc: Document,
    home: NodeId,
    group: NodeId,
    one: FlowRef,
    two: FlowRef,
}

fn opened(flows: bool) -> Opened {
    let mut ids = IdSource::new(0x0E);
    let first = ids.mint();
    let mut doc = Document::new(first);
    let [second, home, group, next] = [ids.mint(), ids.mint(), ids.mint(), ids.mint()];
    let [one_id, two_id] = [ids.mint_item(), ids.mint_item()];
    let flow = |id, name: &str| {
        Keyed::new(
            id,
            Flow {
                name: name.into(),
                start: next,
                modes: Vec::new(),
                values: Vec::new(),
            },
        )
    };
    let mut ops = vec![
        Operation::AddPage {
            id: second,
            index: 1,
            name: "Second".into(),
            ground: None,
            draft: false,
            modes: Vec::new(),
            flows: Vec::new(),
            play: Default::default(),
        },
        // At index 0 after `home`, so the group is the page's first child.
        layer(home, first, true),
        layer(group, first, false),
        layer(next, second, true),
    ];
    if flows {
        ops.push(Operation::SetFlows {
            id: second,
            flows: vec![flow(one_id, "One"), flow(two_id, "Two")],
        });
    }
    doc.apply(&Transaction(ops)).expect("seed");
    Opened {
        doc,
        home,
        group,
        one: FlowRef {
            page: second,
            flow: one_id,
        },
        two: FlowRef {
            page: second,
            flow: two_id,
        },
    }
}

fn opens(doc: &mut Document, flow: FlowRef) {
    doc.apply(&Transaction(vec![Operation::SetPlayExport {
        settings: PlayExport {
            opens: Some(flow),
            ..Default::default()
        },
    }]))
    .expect("export settings");
}

/// Play opens on the asked frame over any flow, else the export's opening
/// flow, else the first flow; an asked id that is no frame is ignored.
///
/// flip: `opening` ignoring `opens` → fails at "the export's opening flow".
#[test]
fn an_asked_frame_wins_then_the_opening_flow_then_the_first() {
    let mut o = opened(true);
    let flows = shell::flows(&o.doc);
    assert_eq!(
        flows
            .iter()
            .map(|(r, n)| (*r, n.as_str()))
            .collect::<Vec<_>>(),
        [(o.one, "One"), (o.two, "Two")]
    );
    assert_eq!(
        shell::opening(&o.doc, &flows, ""),
        Some((Start::Flow(o.one), Some(0))),
        "the first flow"
    );
    opens(&mut o.doc, o.two);
    assert_eq!(
        shell::opening(&o.doc, &flows, ""),
        Some((Start::Flow(o.two), Some(1))),
        "the export's opening flow"
    );
    assert_eq!(
        shell::opening(&o.doc, &flows, &o.home.to_wire()),
        Some((Start::Frame(o.home), None)),
        "the asked frame"
    );
    assert_eq!(
        shell::opening(&o.doc, &flows, &o.group.to_wire()),
        Some((Start::Flow(o.two), Some(1))),
        "a group is no frame to ask for"
    );
}

/// An opening flow the export no longer holds falls back to the first flow, and
/// a document with no flow opens on its first page's first frame.
///
/// flip: `first_frame` taking the first child whatever its kind → fails at "the
/// first page's first frame".
#[test]
fn with_no_flow_play_opens_on_the_first_pages_first_frame() {
    let mut o = opened(true);
    let gone = FlowRef {
        page: o.one.page,
        flow: IdSource::new(0x77).mint_item(),
    };
    opens(&mut o.doc, gone);
    let flows = shell::flows(&o.doc);
    assert_eq!(
        shell::opening(&o.doc, &flows, ""),
        Some((Start::Flow(o.one), Some(0))),
        "an opening flow that is gone"
    );
    let o = opened(false);
    let flows = shell::flows(&o.doc);
    assert!(flows.is_empty());
    assert_eq!(
        shell::opening(&o.doc, &flows, ""),
        Some((Start::Frame(o.home), None)),
        "the first page's first frame"
    );
}

/// Inspect mode picks the topmost shown layer under the point: the last in
/// paint order whose box holds it, never one hidden or under a hidden parent,
/// the frame itself under its children, and nothing outside every box.
///
/// flip: `topmost` dropping the hidden-ancestor walk → fails at "under a hidden
/// parent".
#[test]
fn inspect_picks_the_topmost_shown_layer() {
    let o = opened(false);
    let mut doc = o.doc;
    let mut ids = IdSource::new(0x0F);
    let [a, b, c] = [ids.mint(), ids.mint(), ids.mint()];
    let child = |id, parent, index| Operation::CreateNode {
        id,
        parent,
        index,
        kind: NodeKind::Group,
        transform: Some(Affine::IDENTITY),
        name: None,
    };
    doc.apply(&Transaction(vec![
        child(a, o.home, 0),
        child(b, o.home, 1),
        child(c, b, 0),
    ]))
    .expect("children");
    let rect = |id: NodeId| {
        let inside = [o.home, a, b, c].contains(&id);
        inside.then(|| Rect::new(0.0, 0.0, 50.0, 50.0))
    };
    let at = Point::new(10.0, 10.0);
    assert_eq!(
        shell::topmost(&doc, o.home, at, rect),
        Some(c),
        "the topmost"
    );
    assert_eq!(
        shell::topmost(&doc, o.home, Point::new(90.0, 90.0), rect),
        None,
        "outside every box"
    );
    doc.apply(&Transaction(vec![Operation::SetVisible {
        id: b,
        visible: false,
    }]))
    .expect("hide");
    assert_eq!(
        shell::topmost(&doc, o.home, at, rect),
        Some(a),
        "under a hidden parent"
    );
    let only_frame = |id: NodeId| (id == o.home).then(|| Rect::new(0.0, 0.0, 50.0, 50.0));
    assert_eq!(
        shell::topmost(&doc, o.home, at, only_frame),
        Some(o.home),
        "the frame itself"
    );
}

/// A full table's notice quotes the row cap with its thousands comma, and only
/// the scripts notice is the viewer's.
#[test]
fn notices_read_as_the_viewer_or_the_console_sees_them() {
    let mut ids = IdSource::new(0x10);
    let full = shell::notice_text(Notice::TableFull {
        layer: ids.mint(),
        action: ids.mint_item(),
        table: "Orders".into(),
    });
    let n = mastik_core::chart::MAX_ROWS;
    assert_eq!(
        full,
        format!(
            "Skipped Add row: Orders holds {},{:03} rows, the most a table can",
            n / 1000,
            n % 1000
        )
    );
    assert!(
        shell::notice_text(Notice::ScriptsOff).starts_with('!'),
        "shown"
    );
    assert!(
        !shell::notice_text(Notice::Loop).starts_with('!'),
        "the console's"
    );
}

/// Two boxes side by side measure their gap as a horizontal line, scaled to CSS
/// pixels, labelled in screen units.
#[test]
fn measures_are_scaled_lines_with_screen_labels() {
    let a = Rect::new(0.0, 0.0, 10.0, 10.0);
    let b = Rect::new(30.0, 0.0, 40.0, 10.0);
    let lines = shell::measure_lines(a, b, 2.0);
    assert!(lines.contains(&"h|10|20|60|20".to_string()), "{lines:?}");
}
