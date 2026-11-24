//! The browser player's arithmetic (`crates/mastik-web/src/shell.rs`), tested here:
//! `mastik-web` is its own workspace and builds only for the page, so its tests
//! cannot run there.

#[path = "../../mastik-web/src/shell.rs"]
mod shell;

use mastik_core::kurbo::{Affine, RoundedRectRadii, Size};
use mastik_core::prototype::{Flow, FlowRef, PlayExport};
use mastik_core::{Document, IdSource, Keyed, NodeId, NodeKind, Operation, Transaction};
use mastik_play::Start;

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
