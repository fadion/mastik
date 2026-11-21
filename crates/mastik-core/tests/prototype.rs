//! The prototype model on a layer (`prototype`): its reactions through the
//! commit door, undo, the save format and a copy.

use mastik_core::kurbo::{Affine, RoundedRectRadii, Size};
use mastik_core::prototype::{Action, Reaction, ReactionError, Transition, Trigger};
use mastik_core::{
    Document, History, IdSource, Keyed, NodeId, NodeKind, OpError, Operation, Transaction, io,
    remap_subtree,
};

fn frame(id: NodeId, parent: NodeId, index: usize, name: &str) -> Operation {
    Operation::CreateNode {
        id,
        parent,
        index,
        kind: NodeKind::Artboard {
            size: Size::new(100.0, 100.0),
            corner_radii: RoundedRectRadii::default(),
            corner_smoothing: 0.0,
        },
        transform: Some(Affine::IDENTITY),
        name: Some(name.into()),
    }
}

/// A page holding two frames, `a` and `b`.
fn two_frames() -> (Document, IdSource, NodeId, NodeId) {
    let mut ids = IdSource::new(0x9A);
    let page = ids.mint();
    let mut doc = Document::new(page);
    let (a, b) = (ids.mint(), ids.mint());
    doc.apply(&Transaction(vec![
        frame(a, page, 0, "A"),
        frame(b, page, 1, "B"),
    ]))
    .expect("seed");
    (doc, ids, a, b)
}

fn on(ids: &mut IdSource, trigger: Trigger, actions: Vec<Action>) -> Keyed<Reaction> {
    Keyed::new(
        ids.mint_item(),
        Reaction {
            trigger,
            actions: actions
                .into_iter()
                .map(|a| Keyed::new(ids.mint_item(), a))
                .collect(),
        },
    )
}

fn navigate(to: NodeId) -> Action {
    Action::Navigate {
        to,
        transition: Transition::default(),
        keep_scroll: false,
    }
}

fn set(id: NodeId, reactions: Vec<Keyed<Reaction>>) -> Transaction {
    Transaction(vec![Operation::SetReactions { id, reactions }])
}

fn round_trip(doc: &Document) -> Document {
    io::load(&io::save(doc).expect("saves")).expect("loads")
}

/// A layer's reactions save and load as they were.
///
/// flip: `NodeDto::from_node` writing no reactions → fails at "loaded as saved".
#[test]
fn a_layers_reactions_round_trip_through_the_file() {
    let (mut doc, mut ids, a, b) = two_frames();
    let reactions = vec![
        on(&mut ids, Trigger::Click, vec![navigate(b)]),
        on(
            &mut ids,
            Trigger::Key {
                key: "Enter".into(),
                shift: true,
            },
            vec![Action::Back],
        ),
    ];
    doc.apply(&set(a, reactions.clone())).expect("taken");
    assert_eq!(doc.get(a).unwrap().reactions(), reactions, "applied");
    let loaded = round_trip(&doc);
    assert_eq!(
        loaded.get(a).unwrap().reactions(),
        reactions,
        "loaded as saved"
    );
}

/// Undoing a `SetReactions` puts back the list it replaced.
///
/// flip: `op_set_reactions` answering the new list as its inverse → fails at
/// "undo restores".
#[test]
fn undoing_set_reactions_restores_the_old_list() {
    let (mut doc, mut ids, a, b) = two_frames();
    let first = vec![on(&mut ids, Trigger::Click, vec![navigate(b)])];
    let second = vec![on(&mut ids, Trigger::HoverEnter, vec![Action::Back])];
    let mut history = History::new();
    history.commit(&mut doc, set(a, first.clone())).unwrap();
    history.commit(&mut doc, set(a, second.clone())).unwrap();
    assert_eq!(doc.get(a).unwrap().reactions(), second, "the second list");
    history.undo(&mut doc).unwrap();
    assert_eq!(doc.get(a).unwrap().reactions(), first, "undo restores");
}

/// A script under a trigger no person fires is refused at the door, and the
/// layer keeps what it had.
///
/// flip: `op_set_reactions` without its `prototype::check` → fails at "refused".
#[test]
fn a_script_under_a_timer_is_refused_by_the_door() {
    let (mut doc, mut ids, a, _) = two_frames();
    let script = ids.mint_script();
    let timed = vec![on(
        &mut ids,
        Trigger::After { ms: 500 },
        vec![Action::RunScript { script }],
    )];
    let refused = doc.apply(&set(a, timed));
    assert!(
        matches!(
            refused,
            Err(OpError::BadReaction(ReactionError::ScriptUnasked(
                Trigger::After { ms: 500 }
            )))
        ),
        "refused: {refused:?}"
    );
    assert!(doc.get(a).unwrap().reactions().is_empty(), "unchanged");
}

/// A target naming no layer in the document is saved and loaded as it is.
///
/// flip: the loader refusing a reaction whose target is missing → fails at
/// "loads".
#[test]
fn a_dangling_target_survives_the_file() {
    let (mut doc, mut ids, a, _) = two_frames();
    let gone = ids.mint();
    assert!(doc.get(gone).is_none(), "the target is no layer");
    let reactions = vec![on(&mut ids, Trigger::Click, vec![navigate(gone)])];
    doc.apply(&set(a, reactions.clone())).expect("taken");
    let loaded = round_trip(&doc);
    assert_eq!(loaded.get(a).unwrap().reactions(), reactions, "kept");
}

/// A copied layer's target inside the copied set names the copy; one outside it
/// keeps its id.
///
/// flip: `remap_subtree` cloning `reactions` unmapped → fails at the last
/// assertion, "follows the copy".
#[test]
fn a_copy_remaps_targets_inside_the_template_only() {
    let (mut doc, mut ids, a, b) = two_frames();
    let (button, panel) = (ids.mint(), ids.mint());
    doc.apply(&Transaction(vec![
        frame(button, a, 0, "Button"),
        frame(panel, a, 1, "Panel"),
    ]))
    .expect("seed");
    let reactions = vec![on(
        &mut ids,
        Trigger::Click,
        vec![
            Action::ScrollTo {
                layer: panel,
                transition: Transition::INSTANT,
            },
            navigate(b),
        ],
    )];
    doc.apply(&set(button, reactions)).expect("taken");

    let template = doc.capture_subtree(a).expect("captured");
    let (copies, _) = remap_subtree(&template, &mut ids).expect("remapped");
    let new_panel = copies.iter().find(|n| n.name() == "Panel").unwrap().id();
    let new_button = copies.iter().find(|n| n.name() == "Button").unwrap();
    assert_ne!(new_panel, panel, "the panel was copied");
    let targets: Vec<NodeId> = new_button.reactions()[0]
        .actions
        .iter()
        .flat_map(|k| k.value.layer_targets())
        .collect();
    assert_eq!(
        targets,
        [new_panel, b],
        "follows the copy; outside it, kept"
    );
}

/// A target and a flow's start are saved in the id's wire form, as every other id.
///
/// flip: `Action::Navigate`'s `to` without `serde(with = "wire")` → fails at
/// "wire form".
#[test]
fn targets_are_saved_as_wire_ids() {
    let (_, _, _, b) = two_frames();
    let json = serde_json::to_string(&navigate(b)).unwrap();
    assert!(
        json.contains(&format!("\"{}\"", b.to_wire())),
        "wire form: {json}"
    );
    assert!(json.starts_with("{\"Navigate\""), "tagged by kind: {json}");
}

/// Duplicating two frames together, a link from one to the other names the
/// other's copy.
///
/// flip: `insert_subtrees_as` without its cross-template pass → fails at "the
/// copy of B".
#[test]
fn duplicating_two_frames_links_the_copies_to_each_other() {
    let (mut doc, mut ids, a, b) = two_frames();
    doc.apply(&set(
        a,
        vec![on(&mut ids, Trigger::Click, vec![navigate(b)])],
    ))
    .expect("taken");
    let (tx, created) =
        mastik_core::build::duplicate(&doc, &mut ids, &[a, b], mastik_core::kurbo::Vec2::ZERO);
    doc.apply(&tx).expect("duplicated");
    let [a2, b2] = created[..] else {
        panic!("two copies: {created:?}")
    };
    let (a2, b2) = if doc.get(a2).unwrap().name().starts_with('A') {
        (a2, b2)
    } else {
        (b2, a2)
    };
    let targets = doc.get(a2).unwrap().reactions()[0].actions[0].layer_targets();
    assert_eq!(targets, [b2], "the copy of B");
    assert_eq!(
        doc.get(a).unwrap().reactions()[0].actions[0].layer_targets(),
        [b],
        "the original keeps B"
    );
}

/// A fresh instance holds its main's reactions as they are, so none is an
/// override, and a target inside the main reads as the instance's own layer.
///
/// flip: `Node::make_copy_of` not restoring the source's reactions → fails at
/// "the main's own list".
#[test]
fn an_instance_holds_its_mains_targets_and_reads_its_own_layers() {
    let (mut doc, mut ids, a, _) = two_frames();
    let page = doc.pages()[0].id;
    let (button, panel) = (ids.mint(), ids.mint());
    doc.apply(&Transaction(vec![
        frame(button, a, 0, "Button"),
        frame(panel, a, 1, "Panel"),
        Operation::SetComponent {
            id: a,
            component: true,
        },
    ]))
    .expect("a main");
    let reactions = vec![on(
        &mut ids,
        Trigger::Click,
        vec![Action::ScrollTo {
            layer: panel,
            transition: Transition::INSTANT,
        }],
    )];
    doc.apply(&set(button, reactions.clone())).expect("taken");
    let (tx, instance) =
        mastik_core::build::place_instance(&doc, &mut ids, a, page, None).expect("placed");
    doc.apply(&tx).expect("instance");
    let copy = |name: &str| {
        mastik_core::build::subtree_nodes(&doc, &[instance])
            .into_iter()
            .find(|id| doc.get(*id).unwrap().name() == name)
            .unwrap()
    };
    let (copy_button, copy_panel) = (copy("Button"), copy("Panel"));
    assert_eq!(
        doc.get(copy_button).unwrap().reactions(),
        reactions,
        "the main's own list"
    );
    assert_eq!(
        mastik_core::prototype::resolve_target(&doc, copy_button, panel),
        copy_panel,
        "read as the instance's panel"
    );
    assert_eq!(
        mastik_core::prototype::resolve_target(&doc, button, panel),
        panel,
        "the main reads its own"
    );
}

/// A transaction refused after a `SetPlayExport` leaves the export settings as
/// they were.
///
/// flip: `InPlace`'s tables without `play_export` → fails at "rolled back" (or
/// the debug rollback check panics first).
#[test]
fn a_refused_transaction_rolls_back_the_export_settings() {
    let (mut doc, mut ids, a, _) = two_frames();
    let script = ids.mint_script();
    let settings = mastik_core::prototype::PlayExport {
        player_bar: false,
        ..Default::default()
    };
    let refused = doc.apply(&Transaction(vec![
        Operation::SetPlayExport { settings },
        set(
            a,
            vec![on(
                &mut ids,
                Trigger::FrameOpens,
                vec![Action::RunScript { script }],
            )],
        )
        .0
        .remove(0),
    ]));
    assert!(refused.is_err(), "the second op is refused");
    assert!(doc.play_export().player_bar, "rolled back");
}

/// A duplicated page's flows start on the page's own copies.
///
/// flip: `page::duplicate` without remapping `start` → fails at "starts on the
/// copy".
#[test]
fn a_duplicated_pages_flow_starts_on_its_copy() {
    let (mut doc, mut ids, a, _) = two_frames();
    let page = doc.pages()[0].id;
    let flow = mastik_core::prototype::Flow {
        name: "Checkout".into(),
        start: a,
        modes: Vec::new(),
        values: Vec::new(),
    };
    doc.apply(&Transaction(vec![Operation::SetFlows {
        id: page,
        flows: vec![Keyed::new(ids.mint_item(), flow)],
    }]))
    .expect("flows");
    let (tx, copy) = mastik_core::page::duplicate(&doc, &mut ids, page).expect("a page");
    doc.apply(&tx).expect("duplicated");
    let start = doc.page(copy).unwrap().flows[0].start;
    assert_ne!(start, a, "starts on the copy");
    assert_eq!(
        doc.get(start).unwrap().parent(),
        Some(copy),
        "on the new page"
    );
}

/// The size a page's screen plays at: a device's size, turned, when turned or
/// under *Responsive*; the room under *Responsive* with no device; else as
/// drawn.
///
/// flip: `PagePlay::play_size` not swapping a turned device's sides → fails at
/// "the phone on its side".
#[test]
fn a_page_plays_at_its_device_turned_or_its_room() {
    use mastik_core::prototype::{PagePlay, Scaling};
    let room = Size::new(1000.4, 700.0);
    let phone = PagePlay {
        device: Some("iPhone 16".into()),
        ..PagePlay::default()
    };
    assert!(phone.can_turn());
    assert_eq!(phone.play_size(Scaling::Fit, false, room), None, "as drawn");
    assert_eq!(
        phone.play_size(Scaling::Fit, true, room),
        Some(Size::new(852.0, 393.0)),
        "the phone on its side"
    );
    assert_eq!(
        phone.play_size(Scaling::Responsive, false, room),
        Some(Size::new(393.0, 852.0)),
        "the phone's size"
    );
    let bare = PagePlay::default();
    assert!(!bare.can_turn());
    assert_eq!(
        bare.play_size(Scaling::Responsive, true, room),
        Some(Size::new(1000.0, 700.0)),
        "the room, in whole units; nothing to turn"
    );
    assert_eq!(bare.play_size(Scaling::FillWidth, false, room), None);
    let unknown = PagePlay {
        device: Some("Phone from the future".into()),
        ..PagePlay::default()
    };
    assert!(!unknown.can_turn(), "a device no preset names");
}
