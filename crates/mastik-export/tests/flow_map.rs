//! The flow map: each flow's frames, and every interaction written out.

use mastik_core::kurbo::{Affine, RoundedRectRadii, Size};
use mastik_core::prototype::{
    Action, Branch, Direction, Easing, Expr, Flow, Reaction, Timing, Transition, TransitionKind,
    Trigger, VarChange,
};
use mastik_core::{
    Cell, Document, IdSource, Keyed, Mode, NodeId, NodeKind, Operation, Resolved, Transaction,
    VarId, VarKind, Variable,
};
use mastik_export::flow_map::{flow_map, plans};

fn frame(w: f64, h: f64) -> NodeKind {
    NodeKind::Artboard {
        size: Size::new(w, h),
        corner_radii: RoundedRectRadii::default(),
        corner_smoothing: 0.0,
    }
}

fn rect() -> NodeKind {
    NodeKind::Rect {
        size: Size::new(40.0, 20.0),
        corner_radii: RoundedRectRadii::default(),
        corner_smoothing: 0.0,
    }
}

fn create(
    id: NodeId,
    parent: NodeId,
    index: usize,
    kind: NodeKind,
    name: &str,
    x: f64,
) -> [Operation; 1] {
    [Operation::CreateNode {
        id,
        parent,
        index,
        kind,
        transform: Some(Affine::translate((x, 10.0))),
        name: Some(name.into()),
    }]
}

/// Frames Home, Cart and Sheet on one page and Help on a second: Home's Buy
/// goes to Cart, Cart's Pay opens Sheet as an overlay when `count` is above 0
/// and else counts it up, Sheet's Close goes back, and Home's Help link goes to
/// the other page. One flow, "Checkout", starts at Home; Lost is never reached.
fn shop() -> (Document, [NodeId; 5], VarId) {
    let mut ids = IdSource::new(0x5A);
    let page = ids.mint();
    let mut doc = Document::new(page);
    let other = ids.mint();
    let [home, cart, sheet, lost, help] = [(); 5].map(|_| ids.mint());
    let [buy, pay, close, link] = [(); 4].map(|_| ids.mint());
    let (mode, count) = (ids.mint_mode(), ids.mint_var());
    let ease = |kind, ms| Transition {
        kind,
        direction: Direction::Left,
        timing: Timing::Ease {
            curve: Easing::EaseOut,
            ms,
        },
    };
    let [open_id, count_id] = [(); 2].map(|_| ids.mint_item());
    let mut item = || ids.mint_item();
    let on = |item: &mut dyn FnMut() -> mastik_core::ItemId, trigger, actions: Vec<Action>| {
        let id = item();
        vec![Keyed::new(
            id,
            Reaction {
                trigger,
                actions: actions.into_iter().map(|a| Keyed::new(item(), a)).collect(),
            },
        )]
    };
    let mut ops = vec![
        Operation::AddPage {
            id: other,
            index: 1,
            name: "Support".into(),
            ground: None,
            draft: false,
            modes: Vec::new(),
            flows: Vec::new(),
            play: Default::default(),
        },
        Operation::AddMode {
            collection: mastik_core::variable::CollectionId::FIRST,
            mode: Mode {
                id: mode,
                name: "Default".into(),
            },
            index: 0,
            cells: vec![],
        },
        Operation::AddVariable {
            var: Variable {
                id: count,
                name: "count".into(),
                kind: VarKind::Number,
                cells: [(mode, Cell::Number(0.0))].into(),
                scope: None,
                origin: None,
                forked: None,
                description: String::new(),
            },
        },
    ];
    ops.extend(create(home, page, 0, frame(200.0, 300.0), "Home", 0.0));
    ops.extend(create(cart, page, 1, frame(200.0, 300.0), "Cart", 300.0));
    ops.extend(create(sheet, page, 2, frame(160.0, 100.0), "Sheet", 600.0));
    ops.extend(create(lost, page, 3, frame(100.0, 100.0), "Lost", 900.0));
    ops.extend(create(help, other, 0, frame(100.0, 100.0), "Help", 0.0));
    ops.extend(create(buy, home, 0, rect(), "Buy", 20.0));
    ops.extend(create(link, home, 1, rect(), "Help link", 100.0));
    ops.extend(create(pay, cart, 0, rect(), "Pay", 20.0));
    ops.extend(create(close, sheet, 0, rect(), "Close", 20.0));
    ops.extend([
        Operation::SetReactions {
            id: buy,
            reactions: on(
                &mut item,
                Trigger::Click,
                vec![Action::Navigate {
                    to: cart,
                    transition: ease(TransitionKind::Push, 300),
                    keep_scroll: false,
                }],
            ),
        },
        Operation::SetReactions {
            id: link,
            reactions: on(
                &mut item,
                Trigger::Click,
                vec![Action::Navigate {
                    to: help,
                    transition: Transition::INSTANT,
                    keep_scroll: false,
                }],
            ),
        },
        Operation::SetReactions {
            id: pay,
            reactions: on(
                &mut item,
                Trigger::Click,
                vec![Action::Conditional {
                    branches: vec![Branch {
                        when: Expr(format!("{{{}}} > 0", count.0.to_wire())),
                        actions: vec![Keyed::new(
                            open_id,
                            Action::OpenOverlay {
                                overlay: sheet,
                                settings: Default::default(),
                                transition: ease(TransitionKind::Dissolve, 200),
                            },
                        )],
                    }],
                    otherwise: vec![Keyed::new(
                        count_id,
                        Action::SetVariable {
                            var: count,
                            change: VarChange::Increment { by: 1.0 },
                        },
                    )],
                }],
            ),
        },
        Operation::SetReactions {
            id: close,
            reactions: on(
                &mut item,
                Trigger::Key {
                    key: "Escape".into(),
                    shift: false,
                },
                vec![Action::Back],
            ),
        },
        Operation::SetFlows {
            id: page,
            flows: vec![Keyed::new(
                item(),
                Flow {
                    name: "Checkout".into(),
                    start: home,
                    modes: Vec::new(),
                    values: vec![],
                },
            )],
        },
    ]);
    doc.apply(&Transaction(ops)).expect("the shop");
    (doc, [home, cart, sheet, lost, help], count)
}

/// A flow reaches the frames its links show, breadth first from its start and
/// across pages, and leaves out a frame nothing links to; each reaction is
/// written out in words, numbered in frame order.
///
/// flip: `reached` not following overlays → fails at "the overlay is reached".
#[test]
fn a_flow_reaches_its_frames_and_writes_each_interaction() {
    let (doc, [home, cart, sheet, _lost, help], _) = shop();
    let plans = plans(&doc);
    assert_eq!(plans.len(), 1);
    let plan = &plans[0];
    assert_eq!(plan.name, "Checkout");
    assert_eq!(
        plan.frames,
        vec![home, cart, help, sheet],
        "the overlay is reached, and the other page's frame; Lost is not"
    );
    let words: Vec<&str> = plan.written.iter().map(|w| w.text.as_str()).collect();
    assert_eq!(
        words,
        vec![
            "On click: go to “Cart” (push left, 300 ms ease out)",
            "On click: go to “Help” on the page “Support” (instantly)",
            "On click: if {count} > 0, open “Sheet” as an overlay (dissolve, 200 ms ease out); \
             otherwise, increase “count” by 1",
            "On the key Escape: go back",
        ]
    );
    let numbers: Vec<usize> = plan.written.iter().map(|w| w.number).collect();
    assert_eq!(numbers, vec![1, 2, 3, 4]);
}

/// The flow map is a PDF that reads back: a map page and a words page for the
/// one flow.
///
/// flip: `flow_map` leaving out the words pages → fails at "two pages".
#[test]
fn the_flow_map_is_a_pdf_of_a_map_and_its_words() {
    let (doc, ..) = shop();
    let res = Resolved::rebuild(&doc);
    let pdf = flow_map(&doc, &res).expect("a flow map");
    assert!(mastik_export::pdf_in::looks_like_pdf(&pdf));
    let read = mastik_export::pdf_in::import(pdf).expect("it reads back");
    assert_eq!(read.pages.len(), 2, "two pages");
    // The page's three frames span 760 by 300, with a 40-unit margin and a
    // title band twice that above.
    let map = read.pages[0].size;
    assert!(
        (map.width - 840.0).abs() < 1.0 && (map.height - 420.0).abs() < 1.0,
        "the map frames the flow's frames on its page: {map:?}"
    );
    assert!(
        read.pages[1].size.width < read.pages[1].size.height,
        "the words page is upright"
    );
}

/// A document with no flow has no flow map.
#[test]
fn no_flow_no_map() {
    let doc = Document::new(IdSource::new(1).mint());
    let res = Resolved::rebuild(&doc);
    assert!(flow_map(&doc, &res).is_none());
}

/// A flow whose start is gone, and one whose frames are all on another page,
/// get their words pages and no map page, and the other flows keep theirs.
///
/// flip: `flow_map_of` failing on a flow with nothing on its page → fails at
/// "one dangling flow loses the map".
#[test]
fn a_flow_with_nothing_on_its_page_keeps_the_others() {
    let (mut doc, [.., help], _) = shop();
    let mut ids = IdSource::new(0x77);
    let page = doc.pages().first().expect("the page");
    let (page_id, mut flows) = (page.id, page.flows.clone());
    for (name, start) in [("Gone", ids.mint()), ("Elsewhere", help)] {
        flows.push(Keyed::new(
            ids.mint_item(),
            Flow {
                name: name.into(),
                start,
                modes: Vec::new(),
                values: vec![],
            },
        ));
    }
    doc.apply(&Transaction(vec![Operation::SetFlows {
        id: page_id,
        flows,
    }]))
    .expect("the flows");
    assert_eq!(plans(&doc).len(), 3);
    let res = Resolved::rebuild(&doc);
    let pdf = flow_map(&doc, &res).expect("one dangling flow loses the map");
    let read = mastik_export::pdf_in::import(pdf).expect("it reads back");
    let upright: Vec<bool> = read
        .pages
        .iter()
        .map(|p| p.size.width < p.size.height)
        .collect();
    assert_eq!(
        upright,
        vec![false, true, true, true],
        "Checkout's map and words, then a words page each"
    );
}

/// A control character in a name the map writes is left out, so the page
/// still parses and the flow map is made.
///
/// flip: `flow_map`'s escaper keeping C0 controls → fails at "a control
/// character in a name".
#[test]
fn a_control_character_in_a_name_does_not_lose_the_map() {
    let (mut doc, [home, ..], _) = shop();
    doc.apply(&Transaction(vec![Operation::SetName {
        id: home,
        name: "Home\u{1}".into(),
    }]))
    .expect("the rename");
    assert_eq!(doc.get(home).map(|n| n.name()), Some("Home\u{1}"));
    let res = Resolved::rebuild(&doc);
    assert!(
        flow_map(&doc, &res).is_some(),
        "a control character in a name"
    );
}
