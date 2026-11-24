//! End-to-end play scenarios over small documents built here.

use crate::{Input, Notice, Player, Start, Take};
use mastik_core::kurbo::{Affine, Point, RoundedRectRadii, Size, Vec2};
use mastik_core::peniko::Color;
use mastik_core::prototype::{
    Action, Branch, Direction, Easing, Expr, Flow, Going, Input as Bound, LayerPlay, ModeScope,
    OverlayPosition, OverlaySettings, Pin, Reaction, ScrollAxis, Side, Snap, Timing, Transition,
    TransitionKind, Trigger, VarChange,
};
use mastik_core::variable::{CollectionId, TextVars};
use mastik_core::variant::{Interaction, VariantProp, VariantSet};
use mastik_core::{
    Cell, Document, Effect, EffectKind, IdSource, Keyed, Mode, ModeId, NodeId, NodeKind, Operation,
    Placement, Shadow, TextSizing, TextStyle, Transaction, Value, VarId, VarKind, Variable,
};

/// The player's own actor in every test.
const PLAYER: u64 = 0x91A7;

fn linear(kind: TransitionKind, ms: u32) -> Transition {
    Transition {
        kind,
        direction: Direction::Left,
        timing: Timing::Ease {
            curve: Easing::Linear,
            ms,
        },
    }
}

/// A document under construction: one page, one mode.
struct Fx {
    doc: Document,
    ids: IdSource,
    page: NodeId,
    mode: ModeId,
}

impl Fx {
    fn new() -> Fx {
        let mut ids = IdSource::new(0x7E57);
        let page = ids.mint();
        let mut doc = Document::new(page);
        let mode = ids.mint_mode();
        doc.apply(&Transaction(vec![Operation::AddMode {
            collection: CollectionId::FIRST,
            mode: Mode {
                id: mode,
                name: "Default".into(),
            },
            index: 0,
            cells: vec![],
        }]))
        .expect("a mode");
        Fx {
            doc,
            ids,
            page,
            mode,
        }
    }

    fn apply(&mut self, ops: Vec<Operation>) {
        let mut tx = Transaction(ops);
        mastik_core::propagate::owed(&self.doc, &mut tx, &mut self.ids);
        self.doc.apply(&tx).expect("the fixture applies");
    }

    fn node(&mut self, parent: NodeId, name: &str, kind: NodeKind, at: (f64, f64)) -> NodeId {
        let id = self.ids.mint();
        let index = self.doc.get(parent).map_or(0, |n| n.children().len());
        self.apply(vec![Operation::CreateNode {
            id,
            parent,
            index,
            kind,
            transform: Some(Affine::translate(at)),
            name: Some(name.into()),
        }]);
        id
    }

    /// A top-level frame `w` by `h` at `x` on the page.
    fn frame(&mut self, name: &str, x: f64, w: f64, h: f64) -> NodeId {
        let page = self.page;
        self.node(page, name, frame_kind(w, h), (x, 0.0))
    }

    fn rect(&mut self, parent: NodeId, name: &str, x: f64, y: f64, w: f64, h: f64) -> NodeId {
        let kind = NodeKind::Rect {
            size: Size::new(w, h),
            corner_radii: RoundedRectRadii::default(),
            corner_smoothing: 0.0,
        };
        self.node(parent, name, kind, (x, y))
    }

    /// Add a reaction to `id`, after any it has.
    fn on(&mut self, id: NodeId, trigger: Trigger, actions: Vec<Action>) {
        let mut reactions = self.doc.get(id).unwrap().reactions().to_vec();
        reactions.push(Keyed::new(
            self.ids.mint_item(),
            Reaction {
                trigger,
                actions: actions
                    .into_iter()
                    .map(|a| Keyed::new(self.ids.mint_item(), a))
                    .collect(),
            },
        ));
        self.apply(vec![Operation::SetReactions { id, reactions }]);
    }

    fn play_settings(&mut self, id: NodeId, play: LayerPlay) {
        self.apply(vec![Operation::SetPlay { id, play }]);
    }

    fn var(&mut self, name: &str, kind: VarKind, cell: Cell) -> VarId {
        let id = self.ids.mint_var();
        self.apply(vec![Operation::AddVariable {
            var: Variable {
                id,
                name: name.into(),
                kind,
                cells: [(self.mode, cell)].into(),
                scope: None,
                origin: None,
                forked: None,
                description: String::new(),
            },
        }]);
        id
    }

    fn play(&self, frame: NodeId) -> Player {
        Player::new(&self.doc, Start::Frame(frame), PLAYER).expect("plays")
    }
}

fn frame_kind(w: f64, h: f64) -> NodeKind {
    NodeKind::Artboard {
        size: Size::new(w, h),
        corner_radii: RoundedRectRadii::default(),
        corner_smoothing: 0.0,
    }
}

fn text_kind(content: &str) -> NodeKind {
    NodeKind::Text {
        content: content.into(),
        style: Box::new(TextStyle {
            font_family: "Inter".into(),
            font_size: 12.0,
            ..Default::default()
        }),
        spans: Default::default(),
        para_spans: Default::default(),
        paragraph: Default::default(),
        block: Default::default(),
        sizing: TextSizing::Auto,
        on_path: None,
        on_path_flip: false,
        on_path_offset: 0.0,
    }
}

fn click(p: &mut Player, x: f64, y: f64) {
    let at = Point::new(x, y);
    p.input(Input::PointerMove(at));
    p.input(Input::PointerDown(at));
    p.input(Input::PointerUp(at));
}

fn number(p: &Player, var: VarId) -> f64 {
    match p.doc.resolve_var(p.screen(), var) {
        Some(Value::Number(n)) => n,
        other => panic!("not a number: {other:?}"),
    }
}

/// Two frames, A and B, each 200 by 100; A's button (10,10 50×30) navigates to B
/// with `transition`.
fn two_screens(transition: Transition) -> (Fx, NodeId, NodeId, NodeId) {
    let mut fx = Fx::new();
    let a = fx.frame("A", 0.0, 200.0, 100.0);
    let b = fx.frame("B", 300.0, 200.0, 100.0);
    let button = fx.rect(a, "Button", 10.0, 10.0, 50.0, 30.0);
    fx.on(
        button,
        Trigger::Click,
        vec![Action::Navigate {
            to: b,
            transition,
            keep_scroll: false,
        }],
    );
    (fx, a, b, button)
}

// ── Navigation ──────────────────────────────────────────────────────────────

/// A click on a link dissolves to its frame: both frames drawn half way, the new
/// one at half opacity, then the new one alone; *Back* returns.
///
/// flip: `Player::show` not starting the screen animation → fails at "two layers
/// half way".
#[test]
fn a_click_navigates_with_a_dissolve() {
    let (fx, a, b, _) = two_screens(linear(TransitionKind::Dissolve, 300));
    let mut p = fx.play(a);
    click(&mut p, 20.0, 20.0);
    assert_eq!(p.screen(), b, "navigated");
    assert_eq!(p.history(), vec![a]);
    p.tick(150.0);
    let s = p.scene();
    let frames: Vec<_> = s.layers.iter().map(|l| (l.frame, l.opacity)).collect();
    assert_eq!(frames, vec![(a, 1.0), (b, 0.5)], "two layers half way");
    p.tick(150.0);
    let frames: Vec<_> = p.scene().layers.iter().map(|l| l.frame).collect();
    assert_eq!(frames, vec![b], "B alone once done");
    assert!(!p.is_moving());
    p.input(Input::Key {
        key: "x".into(),
        shift: false,
    });
    // Back from a reaction on B.
    let mut fx = fx;
    fx.on(b, Trigger::Click, vec![Action::Back]);
    let mut p = fx.play(a);
    click(&mut p, 20.0, 20.0);
    click(&mut p, 150.0, 80.0);
    assert_eq!(p.screen(), a, "back");
    assert!(p.history().is_empty());
}

/// A push moves both frames by the screen's width toward its direction.
///
/// flip: `screen_layers`' push arm moving only the incoming frame → fails at
/// "the outgoing frame moves".
#[test]
fn a_push_moves_both_frames() {
    let (fx, a, _, _) = two_screens(linear(TransitionKind::Push, 100));
    let mut p = fx.play(a);
    click(&mut p, 20.0, 20.0);
    p.tick(25.0);
    let s = p.scene();
    assert_eq!(
        s.layers[0].at,
        Point::new(-50.0, 0.0),
        "the outgoing frame moves"
    );
    assert_eq!(
        s.layers[1].at,
        Point::new(150.0, 0.0),
        "the incoming one enters"
    );
}

/// A push with no side moves to the left as one toward the left does, and
/// *Back* comes from the other side for both.
///
/// flip: `opposite` answering no side for no side → fails at "back from the
/// left".
#[test]
fn back_after_a_push_with_no_side_comes_from_the_other_side() {
    for side in [Direction::Left, Direction::None] {
        let mut push = linear(TransitionKind::Push, 100);
        push.direction = side;
        let (mut fx, a, b, _) = two_screens(push);
        fx.on(b, Trigger::Click, vec![Action::Back]);
        let mut p = fx.play(a);
        click(&mut p, 20.0, 20.0);
        p.tick(25.0);
        assert_eq!(p.scene().layers[1].at, Point::new(150.0, 0.0), "{side:?}");
        p.tick(100.0);
        click(&mut p, 150.0, 80.0);
        p.tick(25.0);
        assert_eq!(
            p.scene().layers[1].at,
            Point::new(-150.0, 0.0),
            "back from the left ({side:?})"
        );
    }
}

/// A navigation to a frame that is not there is skipped, and said once.
///
/// flip: `Player::notice_once` pushing every time → fails at "said once".
#[test]
fn a_dangling_target_is_skipped_and_said_once() {
    let (mut fx, a, b, _) = two_screens(Transition::INSTANT);
    fx.apply(vec![Operation::DeleteNode { id: b }]);
    let mut p = fx.play(a);
    click(&mut p, 20.0, 20.0);
    click(&mut p, 20.0, 20.0);
    assert_eq!(p.screen(), a, "stayed");
    let notices = p.take_notices();
    assert_eq!(notices.len(), 1, "said once: {notices:?}");
    assert!(matches!(notices[0], Notice::Missing { .. }));
}

/// A click on nothing flashes the hints, which fade.
///
/// flip: `pointer_up` never setting the flash → fails at "flashes".
#[test]
fn a_click_on_nothing_flashes_the_hints() {
    let (fx, a, _, button) = two_screens(Transition::INSTANT);
    let mut p = fx.play(a);
    let hot = p.hotspots();
    assert_eq!(hot.len(), 1);
    assert_eq!(hot[0].layer, button);
    assert_eq!(
        hot[0].rect,
        mastik_core::kurbo::Rect::new(10.0, 10.0, 60.0, 40.0)
    );
    click(&mut p, 150.0, 80.0);
    assert_eq!(p.scene().hint_flash, Some(1.0), "flashes");
    p.tick(300.0);
    assert_eq!(p.scene().hint_flash, Some(0.5));
    p.tick(300.0);
    assert_eq!(p.scene().hint_flash, None, "faded");
}

// ── Logic ───────────────────────────────────────────────────────────────────

/// A click counts up a variable; a conditional navigates once the count reaches
/// two, reading the variable by id in its expression.
///
/// flip: `run_action`'s conditional taking a branch whose condition is false →
/// fails at "not yet".
#[test]
fn a_conditional_reads_a_variable() {
    let mut fx = Fx::new();
    let a = fx.frame("A", 0.0, 200.0, 100.0);
    let c = fx.frame("C", 300.0, 200.0, 100.0);
    let count = fx.var("count", VarKind::Number, Cell::Number(0.0));
    let label = fx.var("label", VarKind::String, Cell::String(String::new()));
    let button = fx.rect(a, "Button", 10.0, 10.0, 50.0, 30.0);
    let when = Expr(format!("{{{}}} >= 2", count.0.to_wire()));
    let (then_id, else_id) = (fx.ids.mint_item(), fx.ids.mint_item());
    fx.on(
        button,
        Trigger::Click,
        vec![
            Action::SetVariable {
                var: count,
                change: VarChange::Increment { by: 1.0 },
            },
            Action::Conditional {
                branches: vec![Branch {
                    when,
                    actions: vec![Keyed::new(
                        then_id,
                        Action::Navigate {
                            to: c,
                            transition: Transition::INSTANT,
                            keep_scroll: false,
                        },
                    )],
                }],
                otherwise: vec![Keyed::new(
                    else_id,
                    Action::SetVariable {
                        var: label,
                        change: VarChange::Set {
                            to: Expr(format!("\"at \" + {{{}}}", count.0.to_wire())),
                        },
                    },
                )],
            },
        ],
    );
    let mut p = fx.play(a);
    click(&mut p, 20.0, 20.0);
    assert_eq!(number(&p, count), 1.0);
    assert_eq!(p.screen(), a, "not yet");
    assert_eq!(
        p.doc.resolve_var(a, label),
        Some(Value::String("at 1".into())),
        "otherwise ran"
    );
    click(&mut p, 20.0, 20.0);
    assert_eq!(p.screen(), c, "two reached");
    let row = p.values().into_iter().find(|r| r.var == count).unwrap();
    assert!(row.played);
    assert_eq!(row.value, Value::Number(2.0));
    assert_eq!(
        fx.doc.resolve_var(a, count),
        Some(Value::Number(0.0)),
        "the file untouched"
    );
}

/// A flow's starting values beat the modes', and a value set from the state
/// card fires *variable changes* reactions.
///
/// flip: `Player::changed` not firing the reactions → fails at "the reaction ran".
#[test]
fn a_flow_starts_values_and_a_change_fires_its_reaction() {
    let mut fx = Fx::new();
    let a = fx.frame("A", 0.0, 200.0, 100.0);
    let b = fx.frame("B", 300.0, 200.0, 100.0);
    let signed = fx.var("signed in", VarKind::Boolean, Cell::Boolean(false));
    fx.on(
        a,
        Trigger::VariableChanges { var: signed },
        vec![Action::Navigate {
            to: b,
            transition: Transition::INSTANT,
            keep_scroll: false,
        }],
    );
    let flow = fx.ids.mint_item();
    let page = fx.page;
    let mut flows = fx.doc.page(page).unwrap().flows.clone();
    flows.push(Keyed::new(
        flow,
        Flow {
            name: "Signed out".into(),
            start: a,
            modes: Vec::new(),
            values: vec![(signed, Cell::Boolean(true))],
        },
    ));
    fx.apply(vec![Operation::SetFlows { id: page, flows }]);
    let mut p = Player::new(
        &fx.doc,
        Start::Flow(mastik_core::prototype::FlowRef { page, flow }),
        PLAYER,
    )
    .unwrap();
    assert_eq!(
        p.doc.resolve_var(a, signed),
        Some(Value::Boolean(true)),
        "the flow's value"
    );
    assert_eq!(p.screen(), a, "starting values fire nothing");
    p.set_value(signed, Value::Boolean(false)).unwrap();
    assert_eq!(p.screen(), b, "the reaction ran");
    assert_eq!(
        p.set_value(signed, Value::Number(1.0)),
        Err(crate::ValueError::WrongKind)
    );
}

/// Hovering runs *while hovering* and leaving puts it back.
///
/// flip: `rehover` not undoing a left layer's whiles → fails at "put back".
#[test]
fn leaving_a_hover_undoes_it() {
    let mut fx = Fx::new();
    let a = fx.frame("A", 0.0, 200.0, 100.0);
    let over = fx.var("over", VarKind::Boolean, Cell::Boolean(false));
    let card = fx.rect(a, "Card", 10.0, 10.0, 50.0, 30.0);
    fx.on(
        card,
        Trigger::HoverWhile,
        vec![Action::SetVariable {
            var: over,
            change: VarChange::Toggle,
        }],
    );
    let mut p = fx.play(a);
    p.input(Input::PointerMove(Point::new(20.0, 20.0)));
    assert_eq!(p.doc.resolve_var(a, over), Some(Value::Boolean(true)), "on");
    p.input(Input::PointerMove(Point::new(150.0, 80.0)));
    assert_eq!(
        p.doc.resolve_var(a, over),
        Some(Value::Boolean(false)),
        "put back"
    );
    assert!(p.values().iter().all(|r| !r.played), "no play value left");
}

/// *While pressing* opens an overlay and changes an instance's state while the
/// press lasts, and release puts both back.
///
/// flip: `undo` emptying its `Undo::Overlays` arm → fails at "the overlay
/// closes"; its `Undo::State` arm → fails at "the state goes back".
#[test]
fn releasing_a_press_undoes_it() {
    let (mut fx, a, instance, [rest, _, pressed]) = button_set();
    let menu = fx.frame("Menu", 900.0, 80.0, 40.0);
    let peek = fx.rect(a, "Peek", 120.0, 50.0, 40.0, 40.0);
    fx.on(
        peek,
        Trigger::PressWhile,
        vec![
            Action::OpenOverlay {
                overlay: menu,
                settings: OverlaySettings::default(),
                transition: Transition::INSTANT,
            },
            Action::ChangeState {
                instance,
                to: pressed,
                transition: Transition::INSTANT,
            },
        ],
    );
    let mut p = fx.play(a);
    let link = |p: &Player| p.doc.get(instance).unwrap().link();
    let at = Point::new(130.0, 60.0);
    p.input(Input::PointerMove(at));
    p.input(Input::PointerDown(at));
    assert_eq!(p.overlays(), vec![menu], "open while pressed");
    assert_eq!(link(&p), Some(pressed));
    p.input(Input::PointerUp(at));
    assert!(p.overlays().is_empty(), "the overlay closes");
    assert_eq!(link(&p), Some(rest), "the state goes back");
}

/// *While hovering* navigates while the pointer stays where the layer was, and
/// leaving that place comes back to the screen with nothing left in the history.
///
/// flip: `undo` emptying its `Undo::Screen` arm, or `rehover` ending a while
/// only for a layer it saw leave (the navigation took the card from under the
/// pointer before the while was kept) → fails at "back on A".
#[test]
fn leaving_a_hover_that_navigated_comes_back() {
    let (mut fx, a, b, _) = two_screens(Transition::INSTANT);
    let card = fx.rect(a, "Card", 100.0, 50.0, 40.0, 40.0);
    fx.on(
        card,
        Trigger::HoverWhile,
        vec![Action::Navigate {
            to: b,
            transition: Transition::INSTANT,
            keep_scroll: false,
        }],
    );
    let mut p = fx.play(a);
    p.input(Input::PointerMove(Point::new(110.0, 60.0)));
    assert_eq!(p.screen(), b, "there while hovering");
    p.input(Input::PointerMove(Point::new(120.0, 70.0)));
    assert_eq!(p.screen(), b, "still there where the card was");
    p.input(Input::PointerMove(Point::new(190.0, 95.0)));
    assert_eq!(p.screen(), a, "back on A");
    assert!(p.history().is_empty());
}

/// A *while hovering* navigation comes back though *Back*'s history is full,
/// its oldest visit dropped as the hover navigates.
///
/// flip: `undo` comparing `Undo::Screen` with the history's length in place of
/// `Player::depth` → fails at "back on A".
#[test]
fn a_hover_navigation_comes_back_from_a_full_history() {
    let (mut fx, a, b, _) = two_screens(Transition::INSTANT);
    let card = fx.rect(a, "Card", 100.0, 50.0, 40.0, 40.0);
    fx.on(
        card,
        Trigger::HoverWhile,
        vec![Action::Navigate {
            to: b,
            transition: Transition::INSTANT,
            keep_scroll: false,
        }],
    );
    let mut p = fx.play(a);
    let visit = crate::player::Visit {
        frame: b,
        transition: Transition::INSTANT,
        keep: false,
    };
    p.history = vec![visit; crate::player::MAX_HISTORY];
    p.input(Input::PointerMove(Point::new(110.0, 60.0)));
    assert_eq!(p.screen(), b, "fixture: there while hovering");
    assert_eq!(
        p.history().len(),
        crate::player::MAX_HISTORY,
        "fixture: full"
    );
    p.input(Input::PointerMove(Point::new(190.0, 95.0)));
    assert_eq!(p.screen(), a, "back on A");
}

/// Leaving a *while hovering* puts its writes back newest first, so two writes
/// to one variable end at its start; an overlay it opened stays while the
/// pointer is on the layer's place and closes on leaving.
///
/// flip: `undo` putting back oldest first → fails at "back to its start";
/// emptying its `Undo::Overlays` arm → fails at "the overlay closes".
#[test]
fn leaving_a_hover_puts_back_newest_first() {
    let (mut fx, a, card, n) = one_card();
    let menu = fx.frame("Menu", 900.0, 80.0, 40.0);
    fx.on(
        card,
        Trigger::HoverWhile,
        vec![
            add(n, 1.0),
            add(n, 1.0),
            Action::OpenOverlay {
                overlay: menu,
                settings: OverlaySettings::default(),
                transition: Transition::INSTANT,
            },
        ],
    );
    let mut p = fx.play(a);
    p.input(Input::PointerMove(Point::new(20.0, 20.0)));
    assert_eq!(number(&p, n), 2.0, "fixture: both ran");
    assert_eq!(p.overlays(), vec![menu], "open while hovering");
    p.input(Input::PointerMove(Point::new(25.0, 25.0)));
    assert_eq!(p.overlays(), vec![menu], "still open on the card's place");
    p.input(Input::PointerMove(Point::new(190.0, 95.0)));
    assert_eq!(number(&p, n), 0.0, "back to its start");
    assert!(p.values().iter().all(|r| !r.played), "no play value left");
    assert!(p.overlays().is_empty(), "the overlay closes");
}

/// A scroll moves card B under a pointer still on card A's place: A's hover
/// lives on there, B's begins, and when A's ends it leaves B's value alone;
/// B's own end puts back what both found.
///
/// flip: `rehover` putting each ended while back without `hand_over_values`
/// → fails at "B's value stays".
#[test]
fn an_older_hover_ending_leaves_the_newer_ones_value() {
    let mut fx = Fx::new();
    let a = fx.frame("A", 0.0, 200.0, 100.0);
    fx.play_settings(
        a,
        LayerPlay {
            scroll: ScrollAxis::Vertical,
            ..LayerPlay::default()
        },
    );
    let first = fx.rect(a, "First", 10.0, 10.0, 100.0, 40.0);
    let second = fx.rect(a, "Second", 10.0, 60.0, 100.0, 80.0);
    fx.rect(a, "Long", 0.0, 150.0, 10.0, 400.0);
    let shown = fx.var("Preview", VarKind::String, Cell::String("none".into()));
    for (card, name) in [(first, "'A'"), (second, "'B'")] {
        fx.on(
            card,
            Trigger::HoverWhile,
            vec![Action::SetVariable {
                var: shown,
                change: VarChange::Set {
                    to: Expr(name.into()),
                },
            }],
        );
    }
    let mut p = fx.play(a);
    let preview = |p: &Player| p.doc.resolve_var(p.screen(), shown);
    let text = |s: &str| Some(Value::String(s.into()));
    p.input(Input::PointerMove(Point::new(20.0, 20.0)));
    assert_eq!(preview(&p), text("A"), "fixture: A hovered");
    p.input(Input::Wheel {
        at: Point::new(20.0, 20.0),
        delta: Vec2::new(0.0, 50.0),
    });
    p.input(Input::PointerMove(Point::new(21.0, 21.0)));
    assert_eq!(preview(&p), text("B"), "fixture: B under the pointer");
    p.input(Input::PointerMove(Point::new(20.0, 70.0)));
    assert_eq!(preview(&p), text("B"), "B's value stays");
    p.input(Input::PointerMove(Point::new(150.0, 95.0)));
    assert_eq!(preview(&p), text("none"), "both put back");
}

/// *Set variable* to a value of another kind is skipped with a notice; one that
/// does not change what the variable reads fires no *variable changes*; a
/// condition that is not true or false gives a notice and falls to otherwise.
///
/// flip: `same_kind` dropped → fails at "the wrong kind is said"; `set_play`
/// not comparing before and after → fails at "a no-op fires nothing".
#[test]
fn set_variable_and_conditional_skip_what_they_cannot_use() {
    let (mut fx, a, card, n) = one_card();
    let fired = fx.var("fired", VarKind::Number, Cell::Number(0.0));
    let other = fx.var("other", VarKind::Number, Cell::Number(0.0));
    fx.on(
        a,
        Trigger::VariableChanges { var: n },
        vec![add(fired, 1.0)],
    );
    let set = |to: &str| Action::SetVariable {
        var: n,
        change: VarChange::Set {
            to: Expr(to.into()),
        },
    };
    let branch = fx.ids.mint_item();
    let otherwise = fx.ids.mint_item();
    fx.on(
        card,
        Trigger::Click,
        vec![
            set("\"a word\""),
            set("0"),
            Action::Conditional {
                branches: vec![Branch {
                    when: Expr("1 + 1".into()),
                    actions: vec![Keyed::new(branch, add(other, 10.0))],
                }],
                otherwise: vec![Keyed::new(otherwise, add(other, 1.0))],
            },
        ],
    );
    let mut p = fx.play(a);
    click(&mut p, 20.0, 20.0);
    assert_eq!(number(&p, n), 0.0, "unchanged");
    let said = p
        .take_notices()
        .into_iter()
        .filter(|x| matches!(x, Notice::Expression { .. }))
        .count();
    assert_eq!(said, 2, "the wrong kind is said, and the condition");
    assert_eq!(number(&p, fired), 0.0, "a no-op fires nothing");
    assert_eq!(number(&p, other), 1.0, "otherwise ran");
}

/// A key trigger fires on its key, Shift-ed or not as written.
///
/// flip: `Player::key` ignoring `shift` → fails at "the Shift-ed key is not
/// taken".
#[test]
fn a_key_fires_its_reaction() {
    let mut fx = Fx::new();
    let a = fx.frame("A", 0.0, 200.0, 100.0);
    let n = fx.var("n", VarKind::Number, Cell::Number(0.0));
    fx.on(
        a,
        Trigger::Key {
            key: "ArrowRight".into(),
            shift: false,
        },
        vec![Action::SetVariable {
            var: n,
            change: VarChange::Increment { by: 1.0 },
        }],
    );
    let mut p = fx.play(a);
    assert!(p.input(Input::Key {
        key: "ArrowRight".into(),
        shift: false
    }));
    assert!(
        !p.input(Input::Key {
            key: "ArrowRight".into(),
            shift: true
        }),
        "the Shift-ed key is not taken"
    );
    assert_eq!(number(&p, n), 1.0, "plain only");
}

/// An `After` reaction runs at its time after its frame opens.
///
/// flip: `Player::tick` running a timer at the tick's end rather than at its due
/// → fails at "started at its due".
#[test]
fn a_timer_fires_at_its_time() {
    let mut fx = Fx::new();
    let a = fx.frame("A", 0.0, 200.0, 100.0);
    let b = fx.frame("B", 300.0, 200.0, 100.0);
    fx.on(
        a,
        Trigger::After { ms: 500 },
        vec![Action::Navigate {
            to: b,
            transition: linear(TransitionKind::Dissolve, 100),
            keep_scroll: false,
        }],
    );
    let mut p = fx.play(a);
    p.tick(499.0);
    assert_eq!(p.screen(), a, "not before");
    p.tick(51.0);
    assert_eq!(p.screen(), b);
    // The dissolve started at 500, not at the tick's 550.
    assert_eq!(p.scene().layers[1].opacity, 0.5, "started at its due");
}

/// An `After` does not fire once its frame has closed: a splash's timer is
/// dropped when the person taps through before it is due, and starts over
/// when they come back.
///
/// flip: `Player::on_close` keeping the closed frame's timers → fails at
/// "started over on coming back" (`fire_timer`'s shown check alone covers
/// "still on B").
#[test]
fn a_closed_frames_timer_does_not_fire() {
    let (mut fx, a, b, _) = two_screens(Transition::INSTANT);
    let c = fx.frame("C", 600.0, 200.0, 100.0);
    fx.on(
        a,
        Trigger::After { ms: 500 },
        vec![Action::Navigate {
            to: c,
            transition: Transition::INSTANT,
            keep_scroll: false,
        }],
    );
    let mut p = fx.play(a);
    p.tick(100.0);
    click(&mut p, 20.0, 20.0);
    assert_eq!(p.screen(), b);
    p.tick(100.0);
    assert_eq!(p.screen(), b, "still on B");
    p.back();
    assert_eq!(p.screen(), a, "back on A");
    // Back at 200: the first timer would have been due at 500.
    p.tick(350.0);
    assert_eq!(p.screen(), a, "started over on coming back");
    p.tick(150.0);
    assert_eq!(p.screen(), c);
}

/// A turned layer is hit in its box as drawn: a click in the corner of the
/// square around a diamond misses it, and one at its middle hits.
///
/// flip: `Player::walk` testing the layer's bounding box (`Player::rect`) →
/// fails at "the corner misses".
#[test]
fn a_turned_layer_is_hit_in_its_box() {
    let mut fx = Fx::new();
    let a = fx.frame("A", 0.0, 400.0, 400.0);
    let b = fx.frame("B", 500.0, 200.0, 100.0);
    let diamond = fx.rect(a, "Diamond", 0.0, 0.0, 100.0, 100.0);
    let centre = Point::new(200.0, 200.0);
    fx.apply(vec![Operation::SetTransform {
        id: diamond,
        transform: Affine::translate(centre.to_vec2())
            * Affine::rotate(std::f64::consts::FRAC_PI_4)
            * Affine::translate((-50.0, -50.0)),
    }]);
    fx.on(
        diamond,
        Trigger::Click,
        vec![Action::Navigate {
            to: b,
            transition: Transition::INSTANT,
            keep_scroll: false,
        }],
    );
    let mut p = fx.play(a);
    // The square around the diamond reaches 70.7 from its centre.
    click(&mut p, 200.0 - 65.0, 200.0 - 65.0);
    assert_eq!(p.screen(), a, "the corner misses");
    click(&mut p, 200.0, 200.0);
    assert_eq!(p.screen(), b, "the middle hits");
}

/// Two screens each navigating to the other *After 0* still let time move, one
/// turn a frame, and *Back*'s history keeps at most `MAX_HISTORY` visits.
///
/// flip: `on_open` dropping its floor under a timer → fails at "one turn a
/// frame"; `navigate` not trimming the history → fails at "the history is
/// bounded".
#[test]
fn a_loop_between_screens_after_nothing_waits_a_frame() {
    let mut fx = Fx::new();
    let a = fx.frame("A", 0.0, 200.0, 100.0);
    let b = fx.frame("B", 300.0, 200.0, 100.0);
    for (from, to) in [(a, b), (b, a)] {
        fx.on(
            from,
            Trigger::After { ms: 0 },
            vec![Action::Navigate {
                to,
                transition: Transition::INSTANT,
                keep_scroll: false,
            }],
        );
    }
    let mut p = fx.play(a);
    for _ in 0..10 {
        p.tick(16.0);
    }
    assert!(
        p.history().len() <= 11,
        "one turn a frame: {}",
        p.history().len()
    );
    for _ in 0..crate::player::MAX_HISTORY {
        p.tick(16.0);
    }
    assert_eq!(
        p.history().len(),
        crate::player::MAX_HISTORY,
        "the history is bounded"
    );
}

/// A screen waiting only on an *After* timer is moving but not animating, and
/// says when the timer is due; a transition animates.
///
/// flip: `Player::is_animating` counting the timers → fails at "waiting, not
/// animating".
#[test]
fn a_waiting_timer_is_due_not_animating() {
    let mut fx = Fx::new();
    let a = fx.frame("A", 0.0, 200.0, 100.0);
    let b = fx.frame("B", 300.0, 200.0, 100.0);
    fx.on(
        a,
        Trigger::After { ms: 3000 },
        vec![Action::Navigate {
            to: b,
            transition: linear(TransitionKind::Dissolve, 100),
            keep_scroll: false,
        }],
    );
    let mut p = fx.play(a);
    assert!(p.is_moving(), "fixture: the timer is pending");
    assert!(!p.is_animating(), "waiting, not animating");
    assert_eq!(p.next_due(), Some(3000.0), "due at 3 s");
    p.tick(1000.0);
    assert_eq!(p.next_due(), Some(3000.0), "still due at 3 s of play");
    p.tick(2050.0);
    assert_eq!(p.screen(), b, "fixture: the timer fired");
    assert!(p.is_animating(), "the dissolve animates");
}

/// Typing into an input text layer writes its variable, which the layer shows;
/// Backspace removes the last grapheme.
///
/// flip: `Player::prepare` not binding the layer's content → fails at "bound";
/// Backspace popping one `char` → fails at "one grapheme removed".
#[test]
fn typing_writes_the_inputs_variable() {
    let mut fx = Fx::new();
    let a = fx.frame("A", 0.0, 200.0, 100.0);
    let name = fx.var("name", VarKind::String, Cell::String(String::new()));
    let boxed = fx.node(a, "Box", frame_kind(120.0, 30.0), (10.0, 10.0));
    let field = fx.node(boxed, "Field", text_kind("Name"), (4.0, 4.0));
    fx.play_settings(
        field,
        LayerPlay {
            input: Some(Bound {
                var: name,
                property: None,
            }),
            ..LayerPlay::default()
        },
    );
    let mut p = fx.play(a);
    let vars: TextVars = p.doc.get(field).unwrap().text_vars();
    assert_eq!(
        vars[mastik_core::variable::TEXT_CONTENT],
        Some(name),
        "bound"
    );
    // The text is empty and has no width: the click lands on its box.
    click(&mut p, 100.0, 20.0);
    assert_eq!(p.focused(), Some(field));
    p.input(Input::Text("Ada".into()));
    p.input(Input::Text("m".into()));
    p.input(Input::Key {
        key: "Backspace".into(),
        shift: false,
    });
    assert_eq!(
        p.doc.resolve_var(a, name),
        Some(Value::String("Ada".into()))
    );
    // Backspace takes a whole grapheme: the thumb with its skin tone.
    p.input(Input::Text("👍🏽".into()));
    p.input(Input::Key {
        key: "Backspace".into(),
        shift: false,
    });
    assert_eq!(
        p.doc.resolve_var(a, name),
        Some(Value::String("Ada".into())),
        "one grapheme removed"
    );
    assert!(p.input(Input::Key {
        key: "Escape".into(),
        shift: false
    }));
    assert_eq!(p.focused(), None);
}

/// Two instances of a set `State: Rest, Focus, Hover` (tied to their states), each
/// variant holding a text input bound to one string variable: the first
/// instance low at the left of frame A, the second high at the right. Answers
/// the frame, the instances, their inputs and the Rest and Focus mains.
fn two_fields() -> (Fx, NodeId, [NodeId; 2], [NodeId; 2], [NodeId; 2]) {
    let mut fx = Fx::new();
    let a = fx.frame("A", 0.0, 300.0, 100.0);
    let name = fx.var("name", VarKind::String, Cell::String(String::new()));
    let set = fx.frame("Field", 600.0, 300.0, 100.0);
    let rest = fx.node(set, "Rest", frame_kind(80.0, 30.0), (0.0, 0.0));
    let focus = fx.node(set, "Focus", frame_kind(80.0, 30.0), (100.0, 0.0));
    let hover = fx.node(set, "Hover", frame_kind(80.0, 30.0), (200.0, 0.0));
    for main in [rest, focus, hover] {
        let text = fx.node(main, "Value", text_kind(""), (4.0, 4.0));
        fx.play_settings(
            text,
            LayerPlay {
                input: Some(Bound {
                    var: name,
                    property: None,
                }),
                ..LayerPlay::default()
            },
        );
    }
    let mut prop = VariantProp::new("State", vec!["Rest".into(), "Focus".into(), "Hover".into()]);
    prop.states
        .insert("Focus".into(), Interaction::FocusVisible);
    prop.states.insert("Hover".into(), Interaction::Hover);
    let mut ops: Vec<Operation> = [rest, focus, hover]
        .iter()
        .map(|id| Operation::SetComponent {
            id: *id,
            component: true,
        })
        .collect();
    ops.push(Operation::SetVariantSet {
        id: set,
        set: Some(VariantSet { props: vec![prop] }),
    });
    for (id, v) in [(rest, "Rest"), (focus, "Focus"), (hover, "Hover")] {
        ops.push(Operation::SetVariant {
            id,
            values: vec![v.into()],
        });
    }
    fx.apply(ops);
    let mut instances = [rest; 2];
    for (i, at) in [(10.0, 60.0), (200.0, 10.0)].into_iter().enumerate() {
        let (tx, made) = mastik_core::insert_subtrees(
            &fx.doc,
            &mut fx.ids,
            &[Placement {
                nodes: fx.doc.capture_subtree(rest).unwrap(),
                parent: a,
                index: None,
            }],
            Default::default(),
        );
        fx.apply(tx.0);
        fx.apply(vec![Operation::SetTransform {
            id: made[0],
            transform: Affine::translate(at),
        }]);
        instances[i] = made[0];
    }
    let inputs = instances.map(|i| fx.doc.get(i).unwrap().children()[0]);
    (fx, a, instances, inputs, [rest, focus])
}

fn key(p: &mut Player, key: &str, shift: bool) -> bool {
    p.input(Input::Key {
        key: key.into(),
        shift,
    })
}

/// Tab moves focus between a frame's inputs in reading order, round and back
/// with Shift; the instance holding the focused input shows its *Focus* state
/// and rests when focus leaves it.
///
/// flip: `tab_order` keeping the tree's order → fails at "the higher one first";
/// `set_focus` not showing the holder's state → fails at "shows focus".
#[test]
fn tab_moves_focus_in_reading_order_and_shows_focus() {
    let (fx, a, [low, high], [low_in, high_in], [rest, focus]) = two_fields();
    let mut p = fx.play(a);
    let link = |p: &Player, i: NodeId| p.doc.get(i).unwrap().link();
    assert!(key(&mut p, "Tab", false));
    assert_eq!(p.focused(), Some(high_in), "the higher one first");
    assert_eq!(link(&p, high), Some(focus), "shows focus");
    assert_eq!(link(&p, low), Some(rest));
    key(&mut p, "Tab", false);
    assert_eq!(p.focused(), Some(low_in));
    assert_eq!(link(&p, high), Some(rest), "rests when focus leaves");
    assert_eq!(link(&p, low), Some(focus));
    key(&mut p, "Tab", false);
    assert_eq!(p.focused(), Some(high_in), "round to the first");
    key(&mut p, "Tab", true);
    assert_eq!(p.focused(), Some(low_in), "back with Shift");
    p.input(Input::Text("Ada".into()));
    assert_eq!(
        p.doc.resolve_var(a, p.inputs[0].1.var),
        Some(Value::String("Ada".into())),
        "typing still reaches the field"
    );
    key(&mut p, "Escape", false);
    assert_eq!(p.focused(), None);
    assert_eq!(link(&p, low), Some(rest), "rests");
}

/// A click into a field shows its instance's *Focus* state, which hovering it
/// does not replace; a click elsewhere rests it.
///
/// flip: `rehover` showing hover over the focused instance → fails at "focus
/// stays".
#[test]
fn a_clicked_field_shows_focus_over_hover() {
    let (fx, a, [low, _], [low_in, _], [rest, focus]) = two_fields();
    let mut p = fx.play(a);
    let link = |p: &Player| p.doc.get(low).unwrap().link();
    click(&mut p, 40.0, 75.0);
    assert_eq!(p.focused(), Some(low_in));
    assert_eq!(link(&p), Some(focus), "shows focus");
    p.input(Input::PointerMove(Point::new(150.0, 50.0)));
    p.input(Input::PointerMove(Point::new(40.0, 75.0)));
    assert_eq!(link(&p), Some(focus), "focus stays");
    click(&mut p, 150.0, 50.0);
    assert_eq!(p.focused(), None);
    assert_eq!(link(&p), Some(rest));
}

// ── Overlays ────────────────────────────────────────────────────────────────

/// An overlay opens centred over its backdrop; *Back* closes it before it walks
/// the history; a click outside closes it.
///
/// flip: `run_action`'s `Back` popping the history even with an overlay open →
/// fails at "and leaves the screen"; `pointer_down` going on after closing the
/// overlay → fails at "the close was the click's use" (the hit outside an
/// overlay holds no layer, so nothing beneath could react either way).
#[test]
fn an_overlay_opens_closes_and_backs_out() {
    let mut fx = Fx::new();
    let a = fx.frame("A", 0.0, 200.0, 100.0);
    let b = fx.frame("B", 300.0, 200.0, 100.0);
    let menu = fx.frame("Menu", 600.0, 100.0, 50.0);
    let shade = Color::from_rgba8(0, 0, 0, 128);
    let open = fx.rect(b, "Open", 0.0, 0.0, 40.0, 40.0);
    fx.on(
        open,
        Trigger::Click,
        vec![Action::OpenOverlay {
            overlay: menu,
            settings: OverlaySettings {
                backdrop: Some(shade),
                ..OverlaySettings::default()
            },
            transition: Transition::INSTANT,
        }],
    );
    let close = fx.rect(menu, "Close", 0.0, 0.0, 20.0, 20.0);
    fx.on(close, Trigger::Click, vec![Action::Back]);
    let go = fx.rect(a, "Go", 0.0, 0.0, 40.0, 40.0);
    fx.on(
        go,
        Trigger::Click,
        vec![Action::Navigate {
            to: b,
            transition: Transition::INSTANT,
            keep_scroll: false,
        }],
    );
    // A layer on B under the point the outside click lands on.
    let fired = fx.var("fired", VarKind::Boolean, Cell::Boolean(false));
    let under = fx.rect(b, "Under", 160.0, 60.0, 40.0, 40.0);
    fx.on(
        under,
        Trigger::Click,
        vec![Action::SetVariable {
            var: fired,
            change: VarChange::Toggle,
        }],
    );
    let mut p = fx.play(a);
    click(&mut p, 10.0, 10.0);
    click(&mut p, 10.0, 10.0);
    assert_eq!(p.overlays(), vec![menu]);
    let top = p.scene().layers.last().cloned().unwrap();
    assert_eq!(top.at, Point::new(50.0, 25.0), "centred");
    assert_eq!(top.backdrop, Some(shade));
    // Close sits at the overlay's top-left.
    click(&mut p, 55.0, 30.0);
    assert!(p.overlays().is_empty(), "back closes the overlay");
    assert_eq!(p.screen(), b, "and leaves the screen");
    click(&mut p, 190.0, 90.0);
    assert_eq!(
        p.doc.resolve_var(b, fired),
        Some(Value::Boolean(true)),
        "fixture: the layer under the outside point reacts"
    );
    click(&mut p, 10.0, 10.0);
    // Outside the overlay: it closes, and the click goes no further.
    click(&mut p, 190.0, 90.0);
    assert!(p.overlays().is_empty(), "a click outside closes it");
    assert_eq!(p.screen(), b);
    assert_eq!(p.scene().hint_flash, None, "the close was the click's use");
    assert_eq!(
        p.doc.resolve_var(b, fired),
        Some(Value::Boolean(true)),
        "the layer beneath did not react"
    );
}

/// An overlay set not to close on an outside click stays open, and the click
/// reaches nothing beneath it; a layer inside it still reacts.
///
/// flip: `pointer_down` closing the top overlay whatever `close_outside` says →
/// fails at "stays open". Going on past a kept-open overlay instead of
/// returning changes only whether the hints flash, which nothing pins.
#[test]
fn an_overlay_kept_open_takes_an_outside_click() {
    let mut fx = Fx::new();
    let a = fx.frame("A", 0.0, 200.0, 100.0);
    let menu = fx.frame("Menu", 300.0, 100.0, 50.0);
    let fired = fx.var("fired", VarKind::Boolean, Cell::Boolean(false));
    let open = fx.rect(a, "Open", 0.0, 0.0, 40.0, 40.0);
    fx.on(
        open,
        Trigger::Click,
        vec![Action::OpenOverlay {
            overlay: menu,
            settings: OverlaySettings {
                close_outside: false,
                ..OverlaySettings::default()
            },
            transition: Transition::INSTANT,
        }],
    );
    let under = fx.rect(a, "Under", 160.0, 60.0, 40.0, 40.0);
    fx.on(
        under,
        Trigger::Click,
        vec![Action::SetVariable {
            var: fired,
            change: VarChange::Toggle,
        }],
    );
    let close = fx.rect(menu, "Close", 0.0, 0.0, 20.0, 20.0);
    fx.on(
        close,
        Trigger::Click,
        vec![Action::CloseOverlay {
            transition: Transition::INSTANT,
        }],
    );
    let mut p = fx.play(a);
    click(&mut p, 10.0, 10.0);
    assert_eq!(p.overlays(), vec![menu], "fixture: open");
    click(&mut p, 190.0, 90.0);
    assert_eq!(p.overlays(), vec![menu], "stays open");
    assert_eq!(
        p.doc.resolve_var(a, fired),
        Some(Value::Boolean(false)),
        "the layer beneath did not react"
    );
    // Close sits at the centred overlay's top-left.
    click(&mut p, 55.0, 30.0);
    assert!(p.overlays().is_empty(), "*Close overlay* closes it");
}

/// *Swap overlay* from a layer inside the top overlay replaces it, the new one
/// placed by the layer's corner as drawn before the swap; *Navigate* with an
/// overlay open closes it.
///
/// flip: `run_action`'s `SwapOverlay` reading the anchor after the pop → fails
/// at "by the button that swapped it"; `show` not closing the overlays → fails
/// at "navigating closes the overlay".
#[test]
fn swap_overlay_replaces_the_top_and_navigating_closes_it() {
    let mut fx = Fx::new();
    let a = fx.frame("A", 0.0, 200.0, 100.0);
    let b = fx.frame("B", 300.0, 200.0, 100.0);
    let menu = fx.frame("Menu", 600.0, 60.0, 40.0);
    let more = fx.frame("More", 800.0, 50.0, 30.0);
    let manual = OverlaySettings {
        position: OverlayPosition::Manual { x: 0.0, y: 24.0 },
        ..OverlaySettings::default()
    };
    let open = fx.rect(a, "Open", 120.0, 30.0, 40.0, 20.0);
    fx.on(
        open,
        Trigger::Click,
        vec![Action::OpenOverlay {
            overlay: menu,
            settings: manual,
            transition: Transition::INSTANT,
        }],
    );
    let next = fx.rect(menu, "Next", 10.0, 0.0, 20.0, 20.0);
    fx.on(
        next,
        Trigger::Click,
        vec![Action::SwapOverlay {
            overlay: more,
            settings: manual,
            transition: Transition::INSTANT,
        }],
    );
    let go = fx.rect(more, "Go", 0.0, 0.0, 20.0, 20.0);
    fx.on(
        go,
        Trigger::Click,
        vec![Action::Navigate {
            to: b,
            transition: Transition::INSTANT,
            keep_scroll: false,
        }],
    );
    let mut p = fx.play(a);
    click(&mut p, 130.0, 40.0);
    assert_eq!(p.overlays(), vec![menu], "fixture: open");
    let top = p.scene().layers.last().cloned().unwrap();
    assert_eq!(top.at, Point::new(120.0, 54.0), "fixture: by its button");
    // Next is at (130, 54) as drawn.
    click(&mut p, 135.0, 60.0);
    assert_eq!(p.overlays(), vec![more], "swapped");
    let top = p.scene().layers.last().cloned().unwrap();
    assert_eq!(
        top.at,
        Point::new(130.0, 78.0),
        "by the button that swapped it"
    );
    click(&mut p, 135.0, 85.0);
    assert_eq!(p.screen(), b, "fixture: navigated");
    assert!(p.overlays().is_empty(), "navigating closes the overlay");
}

/// *Close overlay* plays its own transition, and *Back* the reverse of the one
/// the overlay opened with: the closing overlay is drawn fading for the span.
///
/// flip: `close_top` ignoring the transition it is given → fails at "closes by
/// its own dissolve"; closing at once when given none → fails at "back reverses
/// the opening dissolve".
#[test]
fn closing_an_overlay_plays_its_transition() {
    let mut fx = Fx::new();
    let a = fx.frame("A", 0.0, 200.0, 100.0);
    let menu = fx.frame("Menu", 300.0, 100.0, 50.0);
    let open = |transition| Action::OpenOverlay {
        overlay: menu,
        settings: OverlaySettings::default(),
        transition,
    };
    let instant = fx.rect(a, "Instant", 0.0, 0.0, 20.0, 20.0);
    fx.on(instant, Trigger::Click, vec![open(Transition::INSTANT)]);
    let fading = fx.rect(a, "Fading", 0.0, 80.0, 20.0, 20.0);
    fx.on(
        fading,
        Trigger::Click,
        vec![open(linear(TransitionKind::Dissolve, 100))],
    );
    let close = fx.rect(menu, "Close", 0.0, 0.0, 20.0, 20.0);
    fx.on(
        close,
        Trigger::Click,
        vec![Action::CloseOverlay {
            transition: linear(TransitionKind::Dissolve, 100),
        }],
    );
    let back = fx.rect(menu, "Back", 80.0, 0.0, 20.0, 20.0);
    fx.on(back, Trigger::Click, vec![Action::Back]);
    let menu_opacity = |p: &Player| {
        p.scene()
            .layers
            .iter()
            .find(|l| l.frame == menu)
            .map(|l| l.opacity)
    };
    let mut p = fx.play(a);
    click(&mut p, 10.0, 10.0);
    assert_eq!(menu_opacity(&p), Some(1.0), "fixture: opened at once");
    // Close sits at the centred overlay's top-left.
    click(&mut p, 55.0, 30.0);
    assert!(p.overlays().is_empty());
    p.tick(50.0);
    let half = menu_opacity(&p).expect("closes by its own dissolve");
    assert!(half > 0.0 && half < 1.0, "half way: {half}");
    p.tick(100.0);
    assert_eq!(menu_opacity(&p), None, "gone at the end");

    click(&mut p, 10.0, 90.0);
    p.tick(200.0);
    assert_eq!(menu_opacity(&p), Some(1.0), "fixture: faded in");
    click(&mut p, 135.0, 30.0);
    assert!(p.overlays().is_empty(), "back closes it");
    p.tick(50.0);
    let half = menu_opacity(&p).expect("back reverses the opening dissolve");
    assert!(half > 0.0 && half < 1.0, "half way: {half}");
}

/// A manual overlay sits at its offset from the layer that opens it, and one
/// frame opened by two actions sits as each says.
///
/// flip: `Player::overlay_at` reading `Manual` from the screen's corner, not the
/// anchor → fails at "by its button".
#[test]
fn a_manual_overlay_sits_by_the_layer_that_opens_it() {
    let mut fx = Fx::new();
    let a = fx.frame("A", 0.0, 200.0, 100.0);
    let menu = fx.frame("Menu", 300.0, 60.0, 40.0);
    let button = fx.rect(a, "Button", 120.0, 30.0, 40.0, 20.0);
    let manual = OverlaySettings {
        position: OverlayPosition::Manual { x: 0.0, y: 24.0 },
        ..OverlaySettings::default()
    };
    fx.on(
        button,
        Trigger::Click,
        vec![Action::OpenOverlay {
            overlay: menu,
            settings: manual,
            transition: Transition::INSTANT,
        }],
    );
    let other = fx.rect(a, "Other", 0.0, 0.0, 20.0, 20.0);
    fx.on(
        other,
        Trigger::Click,
        vec![Action::OpenOverlay {
            overlay: menu,
            settings: OverlaySettings::default(),
            transition: Transition::INSTANT,
        }],
    );
    let mut p = fx.play(a);
    click(&mut p, 130.0, 40.0);
    let top = p.scene().layers.last().cloned().unwrap();
    assert_eq!(top.at, Point::new(120.0, 54.0), "by its button");
    p.restart();
    click(&mut p, 10.0, 10.0);
    let top = p.scene().layers.last().cloned().unwrap();
    assert_eq!(
        top.at,
        Point::new(70.0, 30.0),
        "centred, by the other action"
    );
}

// ── Scrolling ───────────────────────────────────────────────────────────────

/// *Scroll to* scrolls the nearest scrolling frame above its layer, held at its
/// end, never the screen; *Open URL* asks the shell for an `http` or `https`
/// address only, trimmed, and skips any other with a notice.
///
/// flip: `scroll_to` not clamping its offset → fails at "held at its end";
/// *Open URL* taking any scheme → fails at "only the web address".
#[test]
fn scroll_to_and_open_url_reach_only_what_they_may() {
    let mut fx = Fx::new();
    let a = fx.frame("A", 0.0, 200.0, 400.0);
    fx.play_settings(
        a,
        LayerPlay {
            scroll: ScrollAxis::Vertical,
            ..LayerPlay::default()
        },
    );
    let list = fx.node(a, "List", frame_kind(200.0, 100.0), (0.0, 0.0));
    fx.play_settings(
        list,
        LayerPlay {
            scroll: ScrollAxis::Vertical,
            ..LayerPlay::default()
        },
    );
    let cards: Vec<NodeId> = (0..4)
        .map(|i| fx.rect(list, "Card", 0.0, f64::from(i) * 50.0, 200.0, 50.0))
        .collect();
    let go = fx.rect(a, "Go", 0.0, 200.0, 40.0, 40.0);
    fx.on(
        go,
        Trigger::Click,
        vec![
            Action::ScrollTo {
                layer: cards[3],
                transition: Transition::INSTANT,
            },
            Action::OpenUrl {
                url: " HTTPS://x.test ".into(),
            },
            Action::OpenUrl {
                url: "javascript:alert(1)".into(),
            },
        ],
    );
    let mut p = fx.play(a);
    click(&mut p, 10.0, 210.0);
    assert_eq!(
        p.scroll_offset(list),
        Vec2::new(0.0, 100.0),
        "held at its end"
    );
    assert_eq!(p.scroll_offset(a), Vec2::ZERO, "not the screen");
    assert_eq!(
        p.take_requests(),
        vec![crate::Request::OpenUrl("HTTPS://x.test".into())],
        "only the web address"
    );
    assert!(matches!(
        p.take_notices().as_slice(),
        [Notice::Missing { .. }]
    ));
}

/// A vertical scrolling frame scrolls as far as its content reaches; a fixed
/// child stays, a sticky one stops at the top edge.
///
/// flip: `Player::clamp_scroll` not clamping to the content → fails at "as far as
/// the content".
#[test]
fn a_frame_scrolls_its_content_by_pin() {
    let mut fx = Fx::new();
    let a = fx.frame("A", 0.0, 200.0, 100.0);
    fx.play_settings(
        a,
        LayerPlay {
            scroll: ScrollAxis::Vertical,
            ..LayerPlay::default()
        },
    );
    let long = fx.rect(a, "Long", 0.0, 0.0, 200.0, 300.0);
    let bar = fx.rect(a, "Bar", 0.0, 0.0, 200.0, 10.0);
    let head = fx.rect(a, "Head", 0.0, 40.0, 200.0, 10.0);
    fx.play_settings(
        bar,
        LayerPlay {
            pin: Pin::Fixed,
            ..LayerPlay::default()
        },
    );
    fx.play_settings(
        head,
        LayerPlay {
            pin: Pin::Sticky,
            ..LayerPlay::default()
        },
    );
    let mut p = fx.play(a);
    p.input(Input::Wheel {
        at: Point::new(100.0, 50.0),
        delta: Vec2::new(0.0, 500.0),
    });
    assert_eq!(
        p.scroll_offset(a),
        Vec2::new(0.0, 200.0),
        "as far as the content"
    );
    let s = p.scene();
    let moved = |id| {
        s.patches
            .iter()
            .find(|x| x.node == id)
            .and_then(|x| x.transform)
            .map(|t| t.translation())
    };
    assert_eq!(moved(long), Some(Vec2::new(0.0, -200.0)));
    assert_eq!(moved(bar), None, "fixed stays");
    assert_eq!(
        moved(head),
        Some(Vec2::new(0.0, 0.0)),
        "sticky stops at the top"
    );
    // A drag scrolls back.
    p.input(Input::PointerMove(Point::new(100.0, 20.0)));
    p.input(Input::PointerDown(Point::new(100.0, 20.0)));
    p.input(Input::PointerMove(Point::new(100.0, 70.0)));
    p.input(Input::PointerUp(Point::new(100.0, 70.0)));
    assert_eq!(p.scroll_offset(a), Vec2::new(0.0, 150.0), "dragged");
}

/// A drag scrolls the nearest frame scrolling its way: up and down over a
/// sideways carousel inside a page scrolls the page, sideways the carousel.
///
/// flip: `pointer_move` taking the nearest scrolling frame whatever its axis →
/// fails at "the page scrolls".
#[test]
fn a_drag_scrolls_the_frame_that_scrolls_its_way() {
    let mut fx = Fx::new();
    let a = fx.frame("A", 0.0, 200.0, 100.0);
    fx.rect(a, "Long", 0.0, 0.0, 200.0, 300.0);
    let carousel = fx.node(a, "Carousel", frame_kind(200.0, 60.0), (0.0, 20.0));
    fx.rect(carousel, "Cards", 0.0, 0.0, 600.0, 60.0);
    for (id, scroll) in [
        (a, ScrollAxis::Vertical),
        (carousel, ScrollAxis::Horizontal),
    ] {
        fx.play_settings(
            id,
            LayerPlay {
                scroll,
                ..LayerPlay::default()
            },
        );
    }
    let mut p = fx.play(a);
    let drag = |p: &mut Player, from: (f64, f64), to: (f64, f64)| {
        p.input(Input::PointerMove(Point::new(from.0, from.1)));
        p.input(Input::PointerDown(Point::new(from.0, from.1)));
        p.input(Input::PointerMove(Point::new(to.0, to.1)));
        p.input(Input::PointerUp(Point::new(to.0, to.1)));
    };
    drag(&mut p, (100.0, 70.0), (100.0, 40.0));
    assert_eq!(p.scroll_offset(a), Vec2::new(0.0, 30.0), "the page scrolls");
    assert_eq!(p.scroll_offset(carousel), Vec2::ZERO);
    // The carousel now sits at -10..50 on screen.
    drag(&mut p, (150.0, 20.0), (110.0, 20.0));
    assert_eq!(
        p.scroll_offset(carousel),
        Vec2::new(40.0, 0.0),
        "the carousel"
    );
}

// ── Motion ──────────────────────────────────────────────────────────────────

/// Smart animate pairs layers by name: the paired one moves and resizes between
/// its two places, the outgoing pair is hidden, and the unpaired layers slide
/// toward the direction's side, the new one arriving and the gone one leaving.
///
/// flip: `Player::paths` keying by id instead of name → fails at "paired by name";
/// `Smart::between_frames` leaving `slide` unset → fails at "enters from the right".
#[test]
fn smart_animate_pairs_layers_by_name() {
    let mut fx = Fx::new();
    let a = fx.frame("A", 0.0, 200.0, 100.0);
    let b = fx.frame("B", 300.0, 200.0, 100.0);
    let box_a = fx.rect(a, "Box", 0.0, 0.0, 20.0, 20.0);
    let gone = fx.rect(a, "Gone", 150.0, 0.0, 20.0, 20.0);
    let box_b = fx.rect(b, "Box", 100.0, 40.0, 40.0, 20.0);
    let new = fx.rect(b, "New", 0.0, 60.0, 20.0, 20.0);
    fx.on(
        box_a,
        Trigger::Click,
        vec![Action::Navigate {
            to: b,
            transition: linear(TransitionKind::SmartAnimate, 100),
            keep_scroll: false,
        }],
    );
    let mut p = fx.play(a);
    click(&mut p, 5.0, 5.0);
    p.tick(50.0);
    let s = p.scene();
    let patch = |id| s.patches.iter().find(|x| x.node == id).cloned();
    let moved = patch(box_b).expect("patched");
    assert_eq!(
        moved.transform.map(|t| t.translation()),
        Some(Vec2::new(50.0, 20.0)),
        "paired by name"
    );
    assert_eq!(moved.size, Some(Size::new(30.0, 20.0)));
    // Toward the left: half way, the new one is half a screen right of its place.
    assert_eq!(
        patch(new)
            .and_then(|x| x.transform)
            .map(|t| t.translation()),
        Some(Vec2::new(100.0, 60.0)),
        "enters from the right"
    );
    assert_eq!(patch(new).and_then(|x| x.opacity), None, "not faded");
    assert_eq!(patch(box_a).and_then(|x| x.visible), Some(false), "hidden");
    assert_eq!(
        patch(gone)
            .and_then(|x| x.transform)
            .map(|t| t.translation()),
        Some(Vec2::new(50.0, 0.0)),
        "leaves to the left"
    );
    let order: Vec<_> = s.layers.iter().map(|l| l.frame).collect();
    assert_eq!(order, vec![b, a], "the outgoing frame over the end");
    p.tick(50.0);
    assert!(p.scene().patches.is_empty(), "nothing left once done");
}

/// Smart animate fades in a paired layer the start hides and the end shows,
/// as it fades in a new one.
///
/// flip: `Player::look` reading a hidden layer's own opacity → fails at "fades
/// in".
#[test]
fn smart_animate_fades_in_a_layer_shown_at_the_end() {
    let mut fx = Fx::new();
    let a = fx.frame("A", 0.0, 200.0, 100.0);
    let b = fx.frame("B", 300.0, 200.0, 100.0);
    let go = fx.rect(a, "Go", 0.0, 0.0, 20.0, 20.0);
    let hidden = fx.rect(a, "Check", 100.0, 40.0, 20.0, 20.0);
    fx.apply(vec![Operation::SetVisible {
        id: hidden,
        visible: false,
    }]);
    let check = fx.rect(b, "Check", 100.0, 40.0, 20.0, 20.0);
    fx.on(
        go,
        Trigger::Click,
        vec![Action::Navigate {
            to: b,
            transition: linear(TransitionKind::SmartAnimate, 100),
            keep_scroll: false,
        }],
    );
    let mut p = fx.play(a);
    click(&mut p, 5.0, 5.0);
    p.tick(50.0);
    let s = p.scene();
    let opacity = s
        .patches
        .iter()
        .find(|x| x.node == check)
        .and_then(|x| x.opacity);
    assert_eq!(opacity, Some(0.5), "fades in");
}

/// Smart animate with no side fades its unpaired layers in and out in place.
///
/// flip: `Smart::between_frames` sliding under `Direction::None` → fails at "fades in place".
#[test]
fn smart_animate_with_no_side_fades() {
    let mut fx = Fx::new();
    let a = fx.frame("A", 0.0, 200.0, 100.0);
    let b = fx.frame("B", 300.0, 200.0, 100.0);
    let go = fx.rect(a, "Go", 0.0, 0.0, 20.0, 20.0);
    let new = fx.rect(b, "New", 0.0, 60.0, 20.0, 20.0);
    let mut fade = linear(TransitionKind::SmartAnimate, 100);
    fade.direction = Direction::None;
    fx.on(
        go,
        Trigger::Click,
        vec![Action::Navigate {
            to: b,
            transition: fade,
            keep_scroll: false,
        }],
    );
    let mut p = fx.play(a);
    click(&mut p, 5.0, 5.0);
    p.tick(50.0);
    let s = p.scene();
    let n = s
        .patches
        .iter()
        .find(|x| x.node == new)
        .cloned()
        .expect("patched");
    assert_eq!(n.opacity, Some(0.5), "fades in place");
    assert_eq!(n.transform, None, "not moved");
}

/// A paired layer of the outgoing frame that holds a fading one, however deep,
/// carries it: drawn with its paint blank rather than hidden, while one that
/// holds none is hidden.
///
/// flip: `Smart::between_frames` marking no carriers → fails at "the card
/// carries its badge".
#[test]
fn smart_animate_carries_a_fading_layer_with_its_pair() {
    let mut fx = Fx::new();
    let a = fx.frame("A", 0.0, 200.0, 100.0);
    let b = fx.frame("B", 300.0, 200.0, 100.0);
    let go = fx.rect(a, "Go", 0.0, 0.0, 20.0, 20.0);
    let card_a = fx.node(a, "Card", frame_kind(80.0, 60.0), (40.0, 0.0));
    let row = fx.node(card_a, "Row", frame_kind(80.0, 20.0), (0.0, 0.0));
    fx.rect(row, "Badge", 0.0, 0.0, 10.0, 10.0);
    let tag_a = fx.rect(a, "Tag", 150.0, 0.0, 10.0, 10.0);
    let card_b = fx.node(b, "Card", frame_kind(80.0, 60.0), (40.0, 30.0));
    fx.node(card_b, "Row", frame_kind(80.0, 20.0), (0.0, 0.0));
    fx.rect(b, "Tag", 150.0, 0.0, 10.0, 10.0);
    let mut fade = linear(TransitionKind::SmartAnimate, 100);
    fade.direction = Direction::None;
    fx.on(
        go,
        Trigger::Click,
        vec![Action::Navigate {
            to: b,
            transition: fade,
            keep_scroll: false,
        }],
    );
    let mut p = fx.play(a);
    click(&mut p, 5.0, 5.0);
    p.tick(50.0);
    let s = p.scene();
    let patch = |id| s.patches.iter().find(|x| x.node == id).cloned();
    let card = patch(card_a).expect("the card is patched");
    assert!(
        card.visible != Some(false) && card.paint.is_some(),
        "the card carries its badge"
    );
    assert_eq!(
        patch(row).and_then(|r| r.visible),
        None,
        "and so does its row"
    );
    assert_eq!(
        patch(tag_a).and_then(|t| t.visible),
        Some(false),
        "a pair holding nothing is hidden"
    );
}

/// A bound opacity fades from what it is drawn at, its variable's percentage,
/// not the literal 0 its binding leaves: a layer fading in, one fading out and
/// a paired one alike; and a paired layer the end hides fades out, drawn.
///
/// flip: `Smart::patch_in` fading in by `Node::opacity` → fails at "from 60%";
/// `Player::look` reading `Node::opacity` → fails at "paired from 60%";
/// `patch_in` leaving the hidden end undrawn → fails at "drawn while it fades".
#[test]
fn a_bound_opacity_fades_from_what_it_draws() {
    let mut fx = Fx::new();
    let a = fx.frame("A", 0.0, 200.0, 100.0);
    let b = fx.frame("B", 300.0, 200.0, 100.0);
    let go = fx.rect(a, "Go", 0.0, 0.0, 20.0, 20.0);
    let badge = fx.rect(b, "Badge", 0.0, 60.0, 20.0, 20.0);
    let muted = fx.var("muted", VarKind::Number, Cell::Number(60.0));
    let bind = mastik_core::build::bind_opacity_ops(&fx.doc, badge, muted);
    fx.apply(bind);
    let gone = fx.rect(a, "Gone", 40.0, 60.0, 20.0, 20.0);
    let paired = fx.rect(a, "Pair", 80.0, 60.0, 20.0, 20.0);
    let pair = fx.rect(b, "Pair", 80.0, 60.0, 20.0, 20.0);
    for id in [gone, paired] {
        let bind = mastik_core::build::bind_opacity_ops(&fx.doc, id, muted);
        fx.apply(bind);
    }
    fx.rect(a, "Shut", 120.0, 60.0, 20.0, 20.0);
    let shut = fx.rect(b, "Shut", 120.0, 60.0, 20.0, 20.0);
    fx.apply(vec![Operation::SetVisible {
        id: shut,
        visible: false,
    }]);
    let mut fade = linear(TransitionKind::SmartAnimate, 100);
    fade.direction = Direction::None;
    fx.on(
        go,
        Trigger::Click,
        vec![Action::Navigate {
            to: b,
            transition: fade,
            keep_scroll: false,
        }],
    );
    let mut p = fx.play(a);
    click(&mut p, 5.0, 5.0);
    p.tick(50.0);
    let s = p.scene();
    let patch = |id| s.patches.iter().find(|x| x.node == id);
    let o = patch(badge).and_then(|x| x.opacity).expect("patched");
    assert!((o - 0.3).abs() < 1e-6, "from 60%: {o}");
    let o = patch(gone).and_then(|x| x.opacity).expect("patched");
    assert!((o - 0.3).abs() < 1e-6, "out from 60%: {o}");
    let o = patch(pair).and_then(|x| x.opacity).expect("patched");
    assert!((o - 0.8).abs() < 1e-6, "paired from 60%: {o}");
    let shut = patch(shut).expect("patched");
    assert!(
        shut.opacity.is_some_and(|o| (o - 0.5).abs() < 1e-6),
        "fades out: {:?}",
        shut.opacity
    );
    assert_eq!(shut.visible, Some(true), "drawn while it fades");
}

/// Smart animate pairs an instance of one variant with an instance of another
/// variant of the same set: the toggle moves between its places rather than
/// one leaving and the other arriving.
///
/// flip: `Player::paths` keying an instance by its main's lineage alone →
/// fails at "the variants pair".
#[test]
fn smart_animate_pairs_two_variants_of_one_set() {
    let mut fx = Fx::new();
    let a = fx.frame("A", 0.0, 200.0, 100.0);
    let b = fx.frame("B", 300.0, 200.0, 100.0);
    let set = fx.frame("Toggle", 600.0, 200.0, 100.0);
    let off = fx.node(set, "Off", frame_kind(40.0, 20.0), (0.0, 0.0));
    let on = fx.node(set, "On", frame_kind(40.0, 20.0), (100.0, 0.0));
    let mut ops: Vec<Operation> = [off, on]
        .iter()
        .map(|id| Operation::SetComponent {
            id: *id,
            component: true,
        })
        .collect();
    ops.push(Operation::SetVariantSet {
        id: set,
        set: Some(VariantSet {
            props: vec![VariantProp::new("On", vec!["Off".into(), "On".into()])],
        }),
    });
    for (id, v) in [(off, "Off"), (on, "On")] {
        ops.push(Operation::SetVariant {
            id,
            values: vec![v.into()],
        });
    }
    fx.apply(ops);
    let from = place(&mut fx, off, a, (10.0, 10.0));
    let to = place(&mut fx, on, b, (50.0, 40.0));
    let go = fx.rect(a, "Go", 150.0, 60.0, 20.0, 20.0);
    fx.on(
        go,
        Trigger::Click,
        vec![Action::Navigate {
            to: b,
            transition: linear(TransitionKind::SmartAnimate, 100),
            keep_scroll: false,
        }],
    );
    let mut p = fx.play(a);
    assert_eq!(p.doc.get(to).and_then(|n| n.link()), Some(on), "fixture");
    click(&mut p, 155.0, 65.0);
    p.tick(50.0);
    let s = p.scene();
    let patch = |id| s.patches.iter().find(|x| x.node == id).cloned();
    assert_eq!(
        patch(to).and_then(|x| x.transform).map(|t| t.translation()),
        Some(Vec2::new(30.0, 25.0)),
        "the variants pair"
    );
    assert_eq!(patch(from).and_then(|x| x.visible), Some(false), "hidden");
}

/// A spring-timed transition settles and ends.
///
/// flip: `advance_anims` never ending the screen animation → fails at "ended".
#[test]
fn a_spring_transition_settles() {
    let spring = Transition {
        kind: TransitionKind::MoveIn,
        direction: Direction::Up,
        timing: Timing::Spring {
            mass: 1.0,
            stiffness: 300.0,
            damping: 20.0,
        },
    };
    let (fx, a, b, _) = two_screens(spring);
    let mut p = fx.play(a);
    click(&mut p, 20.0, 20.0);
    p.tick(16.0);
    assert!(p.scene().layers[1].at.y > 0.0, "entering from below");
    p.tick(crate::motion::duration(&spring.timing));
    assert!(!p.is_moving(), "ended");
    assert_eq!(p.scene().layers.len(), 1);
    assert_eq!(p.scene().layers[0].frame, b);
}

// ── Components ──────────────────────────────────────────────────────────────

/// A set `State: Rest, Hover, Pressed` (Hover and Pressed tied to their states),
/// with an instance of Rest in frame A, and frame A itself.
fn button_set() -> (Fx, NodeId, NodeId, [NodeId; 3]) {
    let mut fx = Fx::new();
    let a = fx.frame("A", 0.0, 200.0, 100.0);
    let set = fx.frame("Button", 600.0, 300.0, 100.0);
    let rest = fx.node(set, "Rest", frame_kind(80.0, 30.0), (0.0, 0.0));
    let hover = fx.node(set, "Hover", frame_kind(80.0, 30.0), (100.0, 0.0));
    let pressed = fx.node(set, "Pressed", frame_kind(80.0, 30.0), (200.0, 0.0));
    let mut prop = VariantProp::new(
        "State",
        vec!["Rest".into(), "Hover".into(), "Pressed".into()],
    );
    prop.states.insert("Hover".into(), Interaction::Hover);
    prop.states.insert("Pressed".into(), Interaction::Active);
    let mut ops: Vec<Operation> = [rest, hover, pressed]
        .iter()
        .map(|id| Operation::SetComponent {
            id: *id,
            component: true,
        })
        .collect();
    ops.push(Operation::SetVariantSet {
        id: set,
        set: Some(VariantSet { props: vec![prop] }),
    });
    for (id, v) in [(rest, "Rest"), (hover, "Hover"), (pressed, "Pressed")] {
        ops.push(Operation::SetVariant {
            id,
            values: vec![v.into()],
        });
    }
    fx.apply(ops);
    let (tx, made) = mastik_core::insert_subtrees(
        &fx.doc,
        &mut fx.ids,
        &[Placement {
            nodes: fx.doc.capture_subtree(rest).unwrap(),
            parent: a,
            index: None,
        }],
        Default::default(),
    );
    fx.apply(tx.0);
    let instance = made[0];
    fx.apply(vec![Operation::SetTransform {
        id: instance,
        transform: Affine::translate((10.0, 10.0)),
    }]);
    (fx, a, instance, [rest, hover, pressed])
}

/// A component set's interaction states play with nothing authored: hover shows
/// Hover, a press Pressed, release Hover again, leaving Rest.
///
/// flip: `rehover` not showing the hover state → fails at "hover".
#[test]
fn interaction_states_play_by_themselves() {
    let (fx, a, instance, [rest, hover, pressed]) = button_set();
    let mut p = fx.play(a);
    let link = |p: &Player| p.doc.get(instance).unwrap().link();
    assert_eq!(link(&p), Some(rest));
    p.input(Input::PointerMove(Point::new(20.0, 20.0)));
    assert_eq!(link(&p), Some(hover), "hover");
    p.input(Input::PointerDown(Point::new(20.0, 20.0)));
    assert_eq!(link(&p), Some(pressed), "pressed");
    p.input(Input::PointerUp(Point::new(20.0, 20.0)));
    assert_eq!(link(&p), Some(hover), "released");
    p.input(Input::PointerMove(Point::new(150.0, 80.0)));
    assert_eq!(link(&p), Some(rest), "rest");
    assert_eq!(
        fx.doc.get(instance).unwrap().link(),
        Some(rest),
        "the file untouched"
    );
}

/// *Change state* switches an instance and, animated, tracks its layers by id.
///
/// flip: `change_state` not starting the state animation → fails at "animating".
#[test]
fn change_state_switches_the_instance() {
    let (mut fx, a, instance, [_, hover, _]) = button_set();
    let go = fx.rect(a, "Go", 150.0, 50.0, 40.0, 40.0);
    fx.on(
        go,
        Trigger::Click,
        vec![Action::ChangeState {
            instance,
            to: hover,
            transition: linear(TransitionKind::SmartAnimate, 100),
        }],
    );
    let mut p = fx.play(a);
    click(&mut p, 160.0, 60.0);
    assert_eq!(p.doc.get(instance).unwrap().link(), Some(hover));
    assert!(p.is_moving(), "animating");
    p.tick(100.0);
    assert!(!p.is_moving());
}

/// A state loops: each variant's *After* changes its instance to the other, and
/// the new state's timer starts with the change, for as long as the frame shows.
///
/// flip: `switch_to` not calling `rearm` → fails at "back to rest".
#[test]
fn a_state_loops_by_its_variants_after() {
    let (mut fx, a, instance, [rest, hover, _]) = button_set();
    for (from, to) in [(rest, hover), (hover, rest)] {
        fx.on(
            from,
            Trigger::After { ms: 100 },
            vec![Action::ChangeState {
                instance: from,
                to,
                transition: Transition::INSTANT,
            }],
        );
    }
    assert_eq!(
        fx.doc.get(instance).unwrap().reactions().len(),
        1,
        "the instance follows its main's reaction"
    );
    let mut p = fx.play(a);
    let link = |p: &Player| p.doc.get(instance).unwrap().link();
    p.tick(99.0);
    assert_eq!(link(&p), Some(rest), "not before");
    p.tick(1.0);
    assert_eq!(link(&p), Some(hover), "after");
    p.tick(100.0);
    assert_eq!(link(&p), Some(rest), "back to rest");
    // One long tick turns the loop five times over.
    p.tick(1000.0);
    assert_eq!(link(&p), Some(rest), "an even number of turns");
    assert!(p.is_moving(), "still looping");
    assert_eq!(p.take_notices(), vec![], "no loop notice");
}

/// Two states each changing to the other after nothing still let time move: a
/// change of state waits at least a frame before its *After* fires.
///
/// flip: `rearm` dropping its floor → `tick` spins to `MAX_FIRES` turns at time
/// zero and fails at "the frame's own timer fires at once".
#[test]
fn a_loop_after_nothing_waits_a_frame() {
    let (mut fx, a, instance, [rest, hover, _]) = button_set();
    for (from, to) in [(rest, hover), (hover, rest)] {
        fx.on(
            from,
            Trigger::After { ms: 0 },
            vec![Action::ChangeState {
                instance: from,
                to,
                transition: Transition::INSTANT,
            }],
        );
    }
    let mut p = fx.play(a);
    let link = |p: &Player| p.doc.get(instance).unwrap().link();
    p.tick(1.0);
    assert_eq!(link(&p), Some(hover), "the frame's own timer fires at once");
    p.tick(14.0);
    assert_eq!(link(&p), Some(hover), "one turn per sixteen ms");
    p.tick(1.0);
    assert_eq!(link(&p), Some(rest));
}

/// An input instance flips its boolean on a click and shows it by its *Checked*
/// variant; a value set from the state card switches it too, and Space flips it
/// once Tab has focused it.
///
/// flip: `Player::sync_inputs` skipping the switch → fails at "shows checked";
/// `key` not calling `Player::flip` → fails at "Space flips".
#[test]
fn a_checkbox_input_writes_and_shows_its_variable() {
    let mut fx = Fx::new();
    let a = fx.frame("A", 0.0, 200.0, 100.0);
    let set = fx.frame("Checkbox", 600.0, 200.0, 100.0);
    let off = fx.node(set, "Off", frame_kind(20.0, 20.0), (0.0, 0.0));
    let on = fx.node(set, "On", frame_kind(20.0, 20.0), (40.0, 0.0));
    fx.apply(vec![
        Operation::SetComponent {
            id: off,
            component: true,
        },
        Operation::SetComponent {
            id: on,
            component: true,
        },
        Operation::SetVariantSet {
            id: set,
            set: Some(VariantSet {
                props: vec![VariantProp::new("Checked", vec!["Off".into(), "On".into()])],
            }),
        },
        Operation::SetVariant {
            id: off,
            values: vec!["Off".into()],
        },
        Operation::SetVariant {
            id: on,
            values: vec!["On".into()],
        },
    ]);
    let (tx, made) = mastik_core::insert_subtrees(
        &fx.doc,
        &mut fx.ids,
        &[Placement {
            nodes: fx.doc.capture_subtree(off).unwrap(),
            parent: a,
            index: None,
        }],
        Default::default(),
    );
    fx.apply(tx.0);
    let boxed = made[0];
    let agreed = fx.var("agreed", VarKind::Boolean, Cell::Boolean(false));
    fx.play_settings(
        boxed,
        LayerPlay {
            input: Some(Bound {
                var: agreed,
                property: Some("Checked".into()),
            }),
            ..LayerPlay::default()
        },
    );
    let mut p = fx.play(a);
    click(&mut p, 5.0, 5.0);
    assert_eq!(p.doc.resolve_var(a, agreed), Some(Value::Boolean(true)));
    assert_eq!(p.doc.get(boxed).unwrap().link(), Some(on), "shows checked");
    p.set_value(agreed, Value::Boolean(false)).unwrap();
    assert_eq!(p.doc.get(boxed).unwrap().link(), Some(off), "and unchecked");
    // Tab reaches it, and Space flips it as a click does.
    assert!(key(&mut p, "Tab", false));
    assert_eq!(p.focused(), Some(boxed));
    assert!(key(&mut p, " ", false));
    assert_eq!(p.doc.get(boxed).unwrap().link(), Some(on), "Space flips");
}

/// An instance of `main` placed in `parent` at `at`.
fn place(fx: &mut Fx, main: NodeId, parent: NodeId, at: (f64, f64)) -> NodeId {
    let (tx, made) = mastik_core::insert_subtrees(
        &fx.doc,
        &mut fx.ids,
        &[Placement {
            nodes: fx.doc.capture_subtree(main).unwrap(),
            parent,
            index: None,
        }],
        Default::default(),
    );
    fx.apply(tx.0);
    fx.apply(vec![Operation::SetTransform {
        id: made[0],
        transform: Affine::translate(at),
    }]);
    made[0]
}

/// A set's *Disabled* variant takes no input: in an instance shown *Disabled*,
/// a key trigger, a checkbox's click and a text input's focus all do nothing,
/// while the same card shown *Default* beside it takes each.
///
/// flip: `Player::key` not skipping disabled layers → fails at "the key";
/// `pointer_up`'s checkbox not skipping them → fails at "the checkbox";
/// `pointer_down`'s focus not skipping them → fails at "the field".
#[test]
fn a_disabled_instance_takes_no_input() {
    let mut fx = Fx::new();
    let a = fx.frame("A", 0.0, 300.0, 100.0);
    let n = fx.var("n", VarKind::Number, Cell::Number(0.0));
    let agreed = fx.var("agreed", VarKind::Boolean, Cell::Boolean(false));
    let name = fx.var("name", VarKind::String, Cell::String(String::new()));
    // A checkbox set `Checked: Off, On`.
    let boxes = fx.frame("Checkbox", 600.0, 100.0, 50.0);
    let off = fx.node(boxes, "Off", frame_kind(20.0, 20.0), (0.0, 0.0));
    let on = fx.node(boxes, "On", frame_kind(20.0, 20.0), (40.0, 0.0));
    // A card set `State: Default, Disabled`, each variant holding a field, a
    // checkbox and a layer with a key trigger.
    let cards = fx.frame("Card", 600.0, 300.0, 100.0);
    let default = fx.node(cards, "Default", frame_kind(100.0, 60.0), (0.0, 0.0));
    let disabled = fx.node(cards, "Disabled", frame_kind(100.0, 60.0), (150.0, 0.0));
    let mut prop = VariantProp::new("State", vec!["Default".into(), "Disabled".into()]);
    prop.states.insert("Disabled".into(), Interaction::Disabled);
    let mut ops: Vec<Operation> = [off, on, default, disabled]
        .iter()
        .map(|id| Operation::SetComponent {
            id: *id,
            component: true,
        })
        .collect();
    ops.push(Operation::SetVariantSet {
        id: boxes,
        set: Some(VariantSet {
            props: vec![VariantProp::new("Checked", vec!["Off".into(), "On".into()])],
        }),
    });
    ops.push(Operation::SetVariantSet {
        id: cards,
        set: Some(VariantSet { props: vec![prop] }),
    });
    for (id, v) in [
        (off, "Off"),
        (on, "On"),
        (default, "Default"),
        (disabled, "Disabled"),
    ] {
        ops.push(Operation::SetVariant {
            id,
            values: vec![v.into()],
        });
    }
    fx.apply(ops);
    for main in [default, disabled] {
        let field = fx.node(main, "Value", text_kind(""), (4.0, 4.0));
        fx.play_settings(
            field,
            LayerPlay {
                input: Some(Bound {
                    var: name,
                    property: None,
                }),
                ..LayerPlay::default()
            },
        );
        let check = place(&mut fx, off, main, (60.0, 30.0));
        fx.play_settings(
            check,
            LayerPlay {
                input: Some(Bound {
                    var: agreed,
                    property: Some("Checked".into()),
                }),
                ..LayerPlay::default()
            },
        );
        let keyed = fx.rect(main, "Keyed", 0.0, 50.0, 10.0, 10.0);
        fx.on(
            keyed,
            Trigger::Key {
                key: "Enter".into(),
                shift: false,
            },
            vec![add(n, 1.0)],
        );
    }
    let off_card = place(&mut fx, disabled, a, (0.0, 0.0));
    let on_card = place(&mut fx, default, a, (150.0, 0.0));
    let mut p = fx.play(a);
    assert_eq!(
        p.doc.get(off_card).unwrap().link(),
        Some(disabled),
        "fixture"
    );

    key(&mut p, "Enter", false);
    assert_eq!(number(&p, n), 1.0, "the key reaches the Default card alone");
    click(&mut p, 65.0, 35.0);
    assert_eq!(
        p.doc.resolve_var(a, agreed),
        Some(Value::Boolean(false)),
        "the checkbox"
    );
    click(&mut p, 10.0, 25.0);
    assert_eq!(p.focused(), None, "the field");

    // The Default card takes each.
    click(&mut p, 215.0, 35.0);
    assert_eq!(p.doc.resolve_var(a, agreed), Some(Value::Boolean(true)));
    click(&mut p, 160.0, 25.0);
    let field = p.doc.get(on_card).unwrap().children()[0];
    assert_eq!(p.focused(), Some(field));
}

// ── Scripts ─────────────────────────────────────────────────────────────────

/// A script runs on the copy with its prints in the log; one that fails is
/// rolled back whole, and said.
///
/// flip: `run_script_source` keeping a failed run's changes → fails at "rolled
/// back".
#[cfg(feature = "scripts")]
#[test]
fn a_script_runs_on_the_copy_and_a_failed_one_rolls_back() {
    let (mut fx, a, _, button) = two_screens(Transition::INSTANT);
    let (good, bad) = (fx.ids.mint_script(), fx.ids.mint_script());
    let script = |id, name: &str, source: &str| Operation::AddScript {
        script: mastik_core::Script {
            id,
            name: name.into(),
            source: source.into(),
        },
        index: 0,
    };
    fx.apply(vec![
        script(good, "Rename", "Button.name = \"Done\"\nprint(\"renamed\")"),
        script(bad, "Break", "Button.name = \"Half\"\nerror(\"boom\")"),
    ]);
    let other = fx.rect(a, "Other", 100.0, 10.0, 40.0, 40.0);
    fx.on(
        other,
        Trigger::Click,
        vec![Action::RunScript { script: bad }],
    );
    fx.apply(vec![Operation::SetReactions {
        id: button,
        reactions: vec![],
    }]);
    fx.on(
        button,
        Trigger::Click,
        vec![Action::RunScript { script: good }],
    );
    let mut p = fx.play(a);
    assert!(p.has_scripts());
    click(&mut p, 110.0, 20.0);
    assert_eq!(p.doc.get(button).unwrap().name(), "Button", "rolled back");
    assert!(matches!(
        p.take_notices().as_slice(),
        [Notice::Script { .. }]
    ));
    click(&mut p, 20.0, 20.0);
    assert_eq!(p.doc.get(button).unwrap().name(), "Done", "ran");
    assert_eq!(p.log().last().map(String::as_str), Some("renamed"));
    assert_eq!(
        fx.doc.get(button).unwrap().name(),
        "Button",
        "the file untouched"
    );
}

/// A runaway script in play stops at a step count, never at a time, so every
/// shell and a take's replay stop it at the same point.
///
/// flip: `run_script_source` running under the held budget, whose clock stops
/// it first → fails at "stopped by its steps".
#[cfg(feature = "scripts")]
#[test]
fn a_runaway_script_in_play_stops_at_its_steps() {
    let (mut fx, a, _, button) = two_screens(Transition::INSTANT);
    let spin = fx.ids.mint_script();
    fx.apply(vec![
        Operation::AddScript {
            script: mastik_core::Script {
                id: spin,
                name: "Spin".into(),
                source: "local i = 0\nwhile true do i = i + 1 end".into(),
            },
            index: 0,
        },
        Operation::SetReactions {
            id: button,
            reactions: vec![],
        },
    ]);
    fx.on(
        button,
        Trigger::Click,
        vec![Action::RunScript { script: spin }],
    );
    let mut p = fx.play(a);
    click(&mut p, 20.0, 20.0);
    let said = match p.take_notices().as_slice() {
        [Notice::Script { message, .. }] => message.clone(),
        other => panic!("{other:?}"),
    };
    assert!(
        said.ends_with(&format!(
            "Stopped after {} steps",
            mastik_vm::PLAY_BUDGET.steps
        )),
        "stopped by its steps: {said}"
    );
}

/// A loop of host work — layer writes over a 200-layer frame, or `play.vars`
/// writes — spends the play budget's steps, though it takes few Luau steps,
/// and is rolled back; a take's replay stops it at the same point.
///
/// flip: `PLAY_BUDGET.host` at 0 → fails at "both runs refused" (both finish).
#[cfg(feature = "scripts")]
#[test]
fn host_work_in_a_play_script_spends_its_steps() {
    let (mut fx, a, _, button) = two_screens(Transition::INSTANT);
    let field = fx.frame("Field", 1000.0, 400.0, 400.0);
    let dots: Vec<NodeId> = (0..200).map(|_| fx.ids.mint()).collect();
    fx.apply(
        dots.iter()
            .enumerate()
            .map(|(i, id)| Operation::CreateNode {
                id: *id,
                parent: field,
                index: i,
                kind: NodeKind::Rect {
                    size: Size::new(2.0, 2.0),
                    corner_radii: RoundedRectRadii::default(),
                    corner_smoothing: 0.0,
                },
                transform: Some(Affine::translate((5.0, 5.0))),
                name: Some("Dot".into()),
            })
            .collect(),
    );
    let count = fx.var("Count", VarKind::Number, Cell::Number(0.0));
    let (sweep, tally) = (fx.ids.mint_script(), fx.ids.mint_script());
    let script = |id, name: &str, source: &str| Operation::AddScript {
        script: mastik_core::Script {
            id,
            name: name.into(),
            source: source.into(),
        },
        index: 0,
    };
    fx.apply(vec![
        script(sweep, "Sweep", "for i = 1, 100 do Field.children.x = i end"),
        script(
            tally,
            "Tally",
            "for i = 1, 100000 do play.vars.Count = i end",
        ),
        Operation::SetReactions {
            id: button,
            reactions: vec![],
        },
    ]);
    fx.on(
        button,
        Trigger::Click,
        vec![Action::RunScript { script: sweep }],
    );
    let other = fx.rect(a, "Other", 100.0, 10.0, 40.0, 40.0);
    fx.on(
        other,
        Trigger::Click,
        vec![Action::RunScript { script: tally }],
    );
    let stopped = format!("Stopped after {} steps", mastik_vm::PLAY_BUDGET.steps);
    let mut p = fx.play(a);
    p.record();
    p.tick(16.0);
    click(&mut p, 20.0, 20.0);
    p.tick(16.0);
    click(&mut p, 110.0, 20.0);
    p.tick(16.0);
    let said: Vec<String> = p
        .take_notices()
        .into_iter()
        .map(|n| match n {
            Notice::Script { message, .. } => message,
            other => panic!("{other:?}"),
        })
        .collect();
    assert_eq!(said.len(), 2, "both runs refused: {said:?}");
    assert!(
        said.iter().all(|m| m.ends_with(&stopped)),
        "stopped by their steps: {said:?}"
    );
    let last = *dots.last().unwrap();
    let x = |p: &Player| p.document().get(last).unwrap().transform().translation().x;
    assert_eq!(x(&p), 5.0, "the sweep rolled back");
    assert_eq!(number(&p, count), 0.0, "the tally rolled back");
    let take = p.stop_recording().unwrap();
    let mut replay = crate::Replay::new(&fx.doc, take, PLAYER).unwrap();
    replay.finish();
    let r = replay.player();
    assert_eq!(r.log(), p.log(), "the replay stops at the same step");
    assert_eq!(x(r), 5.0);
}

/// A script adding a row to a table in play makes the list over it again on
/// the copy, so the new row shows; the document keeps its one row.
///
/// flip: `run_script_source` not calling `follow_tables` → fails at "the list
/// follows".
#[cfg(feature = "scripts")]
#[test]
fn a_script_editing_a_table_redraws_the_list_over_it() {
    use mastik_core::chart::{Column, ColumnId, Table};
    use mastik_core::list::{DataSet, Source};
    let mut fx = Fx::new();
    let a = fx.frame("A", 0.0, 200.0, 200.0);
    let main = fx.frame("Row", 600.0, 180.0, 30.0);
    fx.apply(vec![Operation::SetComponent {
        id: main,
        component: true,
    }]);
    let item = row_instance(&mut fx, main, a);
    let set = DataSet {
        id: fx.ids.mint_data_set(),
        name: "rows".into(),
        table: Table {
            columns: vec![Column {
                id: ColumnId(0),
                name: "title".into(),
            }],
            rows: vec![vec!["A".into()]],
        },
        source: None,
        pictures: Vec::new(),
    };
    let l = listed(&mut fx, item, Source::New(set));
    let add = fx.ids.mint_script();
    fx.apply(vec![Operation::AddScript {
        script: mastik_core::Script {
            id: add,
            name: "Add".into(),
            source: "tables.rows:add({ title = \"B\" })".into(),
        },
        index: 0,
    }]);
    let button = fx.rect(a, "Add", 150.0, 150.0, 40.0, 40.0);
    fx.on(
        button,
        Trigger::Click,
        vec![Action::RunScript { script: add }],
    );
    let mut p = fx.play(a);
    let rows = |p: &Player| p.document().get(l).map_or(0, |n| n.children().len());
    assert_eq!(rows(&p), 1);
    click(&mut p, 160.0, 160.0);
    assert!(p.take_notices().is_empty());
    assert_eq!(rows(&p), 2, "the list follows");
    assert_eq!(
        fx.doc.get(l).unwrap().children().len(),
        1,
        "the file untouched"
    );
    assert_eq!(p.tables().len(), 1, "on the state card");
    p.rebase(&fx.doc);
    assert_eq!(rows(&p), 2, "kept through a re-clone");
}

/// A script reads a variable's play value through `play.vars` and writes one
/// as *Set variable* does: the write fires *When a variable changes*, and
/// `play.go` shows its frame once the run ends. A failed run takes its play
/// values back with the copy and navigates nowhere.
///
/// flip: `run_script_source` not calling `changed` for what the run wrote →
/// fails at "the change fires"; `run_script_source` not restoring `values` on
/// failure → fails at "taken back"; navigating before it returns on failure →
/// fails at "navigates nowhere".
#[cfg(feature = "scripts")]
#[test]
fn a_script_reads_and_writes_play_values_and_navigates() {
    let (mut fx, a, b, button) = two_screens(Transition::INSTANT);
    let count = fx.var("Count", VarKind::Number, Cell::Number(0.0));
    let seen = fx.var("Seen", VarKind::Number, Cell::Number(0.0));
    let (bump, bad) = (fx.ids.mint_script(), fx.ids.mint_script());
    let script = |id, name: &str, source: &str| Operation::AddScript {
        script: mastik_core::Script {
            id,
            name: name.into(),
            source: source.into(),
        },
        index: 0,
    };
    fx.apply(vec![
        script(
            bump,
            "Bump",
            "play.vars.Count = play.vars.Count + 1\nprint(play.vars.Count)\nplay.go(\"B\")",
        ),
        script(
            bad,
            "Bad",
            "play.vars.Count = 100\nplay.go(\"B\")\nerror(\"boom\")",
        ),
        Operation::SetReactions {
            id: button,
            reactions: vec![],
        },
    ]);
    fx.on(
        button,
        Trigger::Click,
        vec![Action::RunScript { script: bump }],
    );
    let other = fx.rect(a, "Other", 100.0, 10.0, 40.0, 40.0);
    fx.on(
        other,
        Trigger::Click,
        vec![Action::RunScript { script: bad }],
    );
    let watcher = fx.rect(a, "Watcher", 150.0, 60.0, 10.0, 10.0);
    fx.on(
        watcher,
        Trigger::VariableChanges { var: count },
        vec![Action::SetVariable {
            var: seen,
            change: VarChange::Increment { by: 1.0 },
        }],
    );
    let mut p = fx.play(a);
    p.set_value(count, Value::Number(5.0)).unwrap();
    assert_eq!(
        number(&p, seen),
        1.0,
        "fixture: the state card's write fires"
    );

    click(&mut p, 110.0, 20.0);
    assert!(matches!(
        p.take_notices().as_slice(),
        [Notice::Script { .. }]
    ));
    assert_eq!(number(&p, count), 5.0, "taken back");
    assert_eq!(number(&p, seen), 1.0, "a failed run fires nothing");
    assert_eq!(p.screen(), a, "a failed run navigates nowhere");

    click(&mut p, 20.0, 20.0);
    assert!(p.take_notices().is_empty());
    assert_eq!(
        p.log().last().map(String::as_str),
        Some("6"),
        "the play value read"
    );
    assert_eq!(number(&p, count), 6.0, "written as a play value");
    assert_eq!(number(&p, seen), 2.0, "the change fires");
    assert_eq!(p.screen(), b, "navigated once the run ended");
    assert_eq!(p.history(), vec![a]);
}

/// A *while hovering* script's `play.vars` write is put back on leaving, and a
/// write of the wrong kind is refused at its line, the value unchanged.
///
/// flip: `run_script_source` not recording `Undo::Value` for what the run
/// wrote → fails at "put back on leaving"; `value_of` taking any value →
/// fails at "refused".
#[cfg(feature = "scripts")]
#[test]
fn a_hover_script_write_comes_back_and_a_wrong_kind_is_refused() {
    let (mut fx, a, card, _) = one_card();
    let count = fx.var("Count", VarKind::Number, Cell::Number(0.0));
    let (bump, wrong) = (fx.ids.mint_script(), fx.ids.mint_script());
    let script = |id, name: &str, source: &str| Operation::AddScript {
        script: mastik_core::Script {
            id,
            name: name.into(),
            source: source.into(),
        },
        index: 0,
    };
    fx.apply(vec![
        script(bump, "Bump", "play.vars.Count = play.vars.Count + 1"),
        script(wrong, "Wrong", "play.vars.Count = \"x\""),
    ]);
    fx.on(
        card,
        Trigger::HoverWhile,
        vec![Action::RunScript { script: bump }],
    );
    let other = fx.rect(a, "Other", 100.0, 10.0, 40.0, 40.0);
    fx.on(
        other,
        Trigger::Click,
        vec![Action::RunScript { script: wrong }],
    );
    let mut p = fx.play(a);
    p.input(Input::PointerMove(Point::new(20.0, 20.0)));
    assert_eq!(number(&p, count), 1.0, "fixture: written while hovering");
    p.input(Input::PointerMove(Point::new(80.0, 80.0)));
    assert_eq!(number(&p, count), 0.0, "put back on leaving");
    assert!(p.take_notices().is_empty());

    click(&mut p, 110.0, 20.0);
    assert!(
        matches!(p.take_notices().as_slice(), [Notice::Script { .. }]),
        "refused"
    );
    assert_eq!(number(&p, count), 0.0);
}

/// A script fired inside a list's row reads that row through `play.row`, which
/// is nil outside any row.
///
/// flip: `Player::row_cells` reading the table's first row for every copy →
/// fails at "the second row".
#[cfg(feature = "scripts")]
#[test]
fn a_script_in_a_row_reads_its_row() {
    let (mut fx, a, item, _) = cart(&[["Shirt", "1"], ["Socks", "3"]]);
    let picked = fx.var("Picked", VarKind::String, Cell::String(String::new()));
    let pick = fx.ids.mint_script();
    fx.apply(vec![Operation::AddScript {
        script: mastik_core::Script {
            id: pick,
            name: "Pick".into(),
            source: "play.vars.Picked = play.row.Name .. \" x\" .. play.row.Qty".into(),
        },
        index: 0,
    }]);
    fx.on(
        item,
        Trigger::Click,
        vec![Action::RunScript { script: pick }],
    );
    // The rows made again, so each copy carries the reaction.
    let l = fx.doc.get(item).unwrap().parent().unwrap();
    mastik_core::list::regenerate_outside_history(&mut fx.doc, &[l]);
    let mut p = fx.play(a);
    // Rows at y 10 and 48.
    click(&mut p, 30.0, 60.0);
    assert!(p.take_notices().is_empty());
    assert_eq!(
        p.doc.resolve_var(p.screen(), picked),
        Some(Value::String("Socks x3".into())),
        "the second row"
    );
    let outside = fx.ids.mint_script();
    fx.apply(vec![Operation::AddScript {
        script: mastik_core::Script {
            id: outside,
            name: "Outside".into(),
            source: "print(play.row == nil)".into(),
        },
        index: 0,
    }]);
    let button = fx.rect(a, "Button", 150.0, 350.0, 40.0, 40.0);
    fx.on(
        button,
        Trigger::Click,
        vec![Action::RunScript { script: outside }],
    );
    let mut p = fx.play(a);
    click(&mut p, 160.0, 360.0);
    assert_eq!(
        p.log().last().map(String::as_str),
        Some("true"),
        "no row outside a list"
    );
}

// ── Live update ─────────────────────────────────────────────────────────────

/// An edit the copy takes keeps play where it is; one it refuses re-clones and
/// carries play state across by id; one that deletes the screen falls back to
/// the start, said once.
///
/// flip: `Player::reclone` not carrying the play values → fails at "value kept
/// across the re-clone".
#[test]
fn live_update_keeps_play_state() {
    let (mut fx, a, b, _) = two_screens(Transition::INSTANT);
    let n = fx.var("n", VarKind::Number, Cell::Number(0.0));
    let doomed = fx.rect(b, "Doomed", 100.0, 50.0, 10.0, 10.0);
    let mut p = fx.play(a);
    click(&mut p, 20.0, 20.0);
    p.set_value(n, Value::Number(7.0)).unwrap();
    // Taken: a new layer on B.
    let extra = fx.ids.mint();
    let tx = Transaction(vec![Operation::CreateNode {
        id: extra,
        parent: b,
        index: 0,
        kind: frame_kind(5.0, 5.0),
        transform: None,
        name: Some("Extra".into()),
    }]);
    fx.doc.apply(&tx).unwrap();
    p.apply_edit(&tx).unwrap();
    assert!(p.doc.get(extra).is_some(), "the edit reached the copy");
    assert_eq!((p.screen(), number(&p, n)), (b, 7.0), "play untouched");
    // Refused: play deleted a layer the edit renames.
    p.commit(Transaction(vec![Operation::DeleteNode { id: doomed }]))
        .unwrap();
    let tx = Transaction(vec![Operation::SetName {
        id: doomed,
        name: "Renamed".into(),
    }]);
    fx.doc.apply(&tx).unwrap();
    p.apply_edit(&tx).unwrap();
    assert_eq!(p.doc.get(doomed).unwrap().name(), "Renamed", "re-cloned");
    assert_eq!(p.screen(), b, "screen kept across the re-clone");
    assert_eq!(number(&p, n), 7.0, "value kept across the re-clone");
    assert!(p.take_notices().is_empty());
    // The screen deleted: back to the start, said once.
    let tx = Transaction(vec![Operation::DeleteNode { id: b }]);
    fx.doc.apply(&tx).unwrap();
    p.apply_edit(&tx).unwrap();
    assert_eq!(p.screen(), a, "fell back to the start");
    assert_eq!(
        p.take_notices(),
        vec![Notice::Fallback(crate::Fallback::Screen)]
    );
}

/// An undo the editor's journal hands over as a history step reaches the
/// player even when it puts back a value plain `apply` refuses: a layer whose
/// blur a file carried past the cap, deleted and undeleted.
///
/// flip: `Player::apply_restoring` following with plain `apply` → fails at
/// "the player follows".
#[test]
fn an_undo_putting_back_a_refused_value_reaches_the_player() {
    let (mut fx, a, _, _) = two_screens(Transition::INSTANT);
    let blurred = fx.rect(a, "Blurred", 100.0, 50.0, 10.0, 10.0);
    let past = mastik_core::effect::MAX_BLUR * 2.0;
    fx.doc
        .apply_restoring(&Transaction(vec![Operation::SetEffects {
            id: blurred,
            effects: mastik_core::keyed_by_position([mastik_core::Effect::new(
                mastik_core::EffectKind::LayerBlur { radius: past },
            )]),
        }]))
        .expect("a file's value, as the loader keeps it");
    let mut p = fx.play(a);
    let mut history = mastik_core::History::default();
    history.keep_journal(true);
    let delete = mastik_core::build::delete(&fx.doc, &[blurred]);
    history.commit(&mut fx.doc, delete).unwrap();
    history.undo(&mut fx.doc).unwrap();
    let journal = history.take_journal();
    assert_eq!(
        journal.iter().map(|a| a.restoring).collect::<Vec<_>>(),
        [false, true],
        "the undo is journaled as a history step"
    );
    for a in &journal {
        match a.restoring {
            true => p.apply_restoring(&a.tx),
            false => p.apply_edit(&a.tx),
        }
        .expect("the player follows");
    }
    assert!(p.base.get(blurred).is_some(), "the undo reached the player");
    assert!(p.document().get(blurred).is_some());
    assert!(p.base == fx.doc, "the player's document is the editor's");
}

/// The start screen deleted under an open overlay: the overlay closes as a
/// navigation would, and the frame fallen back to becomes the start, so a
/// take recorded now begins there and replays.
///
/// flip: `fall_back` not closing the overlays → fails at "the overlay closed";
/// not moving the start → fails at "recording restarted".
#[test]
fn a_deleted_start_screen_closes_overlays_and_moves_the_start() {
    let (mut fx, a, b, _) = two_screens(Transition::INSTANT);
    let menu = fx.frame("Menu", 900.0, 80.0, 40.0);
    let go = fx.rect(b, "Go", 150.0, 50.0, 40.0, 40.0);
    fx.on(
        go,
        Trigger::Click,
        vec![Action::OpenOverlay {
            overlay: menu,
            settings: OverlaySettings::default(),
            transition: Transition::INSTANT,
        }],
    );
    let mut p = fx.play(b);
    click(&mut p, 160.0, 60.0);
    assert_eq!(p.overlays(), vec![menu], "fixture: open");
    p.tick(2000.0);
    assert!(p.now() > 0.0, "fixture: time passed");
    assert_eq!(p.overlays(), vec![menu], "fixture: still open");

    let tx = Transaction(vec![Operation::DeleteNode { id: b }]);
    fx.doc.apply(&tx).unwrap();
    p.apply_edit(&tx).unwrap();
    assert_eq!(p.screen(), a, "fell back to the first frame");
    assert!(p.overlays().is_empty(), "the overlay closed");

    p.record();
    assert_eq!(p.now(), 0.0, "recording restarted");
    assert_eq!(p.screen(), a);
    let take = p.stop_recording().unwrap();
    assert!(crate::Replay::new(&fx.doc, take, PLAYER).is_ok(), "replays");
}

/// The start frame deleted while play is on another screen moves the start to
/// the first frame, and a deleted flow moves it to the frame the flow began
/// at, so *Restart* restarts and a take recorded then replays.
///
/// flip: `fall_back` not moving a start the edit took away → fails at
/// "restarted from the first frame"; moving a flow's to the first frame → fails
/// at "the flow's frame".
#[test]
fn a_deleted_start_frame_or_flow_moves_the_start() {
    let (mut fx, a, b, _) = two_screens(Transition::INSTANT);
    let c = fx.frame("C", 600.0, 200.0, 100.0);
    let mut p = fx.play(a);
    click(&mut p, 20.0, 20.0);
    p.tick(2000.0);
    assert_eq!(p.screen(), b, "fixture: navigated off the start");
    let tx = Transaction(vec![Operation::DeleteNode { id: a }]);
    fx.doc.apply(&tx).unwrap();
    p.apply_edit(&tx).unwrap();
    assert_eq!(p.screen(), b, "the screen stays");
    p.tick(500.0);
    p.restart();
    assert_eq!(
        (p.now(), p.screen()),
        (0.0, b),
        "restarted from the first frame"
    );
    p.tick(500.0);
    p.record();
    assert_eq!(p.now(), 0.0, "recording restarted");
    p.tick(300.0);
    let take = p.stop_recording().unwrap();
    assert_eq!(take.length, 300.0);
    assert!(crate::Replay::new(&fx.doc, take, PLAYER).is_ok(), "replays");

    // A flow starting at C, deleted while its frame stands.
    let flow = fx.ids.mint_item();
    let page = fx.page;
    let flows = vec![Keyed::new(
        flow,
        Flow {
            name: "Flow".into(),
            start: c,
            modes: Vec::new(),
            values: Vec::new(),
        },
    )];
    fx.apply(vec![Operation::SetFlows { id: page, flows }]);
    let start = Start::Flow(mastik_core::prototype::FlowRef { page, flow });
    let mut p = Player::new(&fx.doc, start, PLAYER).unwrap();
    let tx = Transaction(vec![Operation::SetFlows {
        id: page,
        flows: Vec::new(),
    }]);
    fx.doc.apply(&tx).unwrap();
    p.apply_edit(&tx).unwrap();
    assert_eq!(p.start(), Start::Frame(c), "the flow's frame");
    p.tick(500.0);
    p.restart();
    assert_eq!(
        (p.now(), p.screen()),
        (0.0, c),
        "restarted at the flow's frame"
    );
}

/// An edit the copy takes cannot write over play's own choices: a page's modes
/// edited keep the mode *Set mode* gave, while a pick in another collection
/// shows, and the screen resized keeps its play size.
///
/// flip: `Player::put_play_back` not putting modes back → fails at "the played
/// mode holds"; putting back the node's whole list → fails at "the other
/// collection's pick shows"; not refitting the screen → fails at "the play
/// size holds"; `follow` re-cloning every edit → fails at "taken, not re-cloned".
#[test]
fn a_clean_live_edit_keeps_set_modes_and_the_play_size() {
    use mastik_core::variable::Collection;
    let mut fx = Fx::new();
    let a = fx.frame("A", 0.0, 200.0, 100.0);
    let c = fx.frame("C", 600.0, 200.0, 100.0);
    let other = fx.rect(c, "Other", 10.0, 10.0, 40.0, 40.0);
    let (light, dark, n) = dark_mode(&mut fx);
    let (brand_a, brand_b) = (fx.ids.mint_mode(), fx.ids.mint_mode());
    let brand = fx.ids.mint_collection();
    let sorted = |mut m: Vec<ModeId>| {
        m.sort();
        m
    };
    fx.apply(vec![
        Operation::AddCollection {
            collection: Collection {
                id: brand,
                name: "Brand".into(),
                modes: vec![
                    Mode {
                        id: brand_a,
                        name: "A".into(),
                    },
                    Mode {
                        id: brand_b,
                        name: "B".into(),
                    },
                ],
                breakpoints: None,
            },
            index: 1,
        },
        Operation::SetModes {
            id: fx.page,
            modes: sorted(vec![light, brand_a]),
        },
    ]);
    let mut p = fx.play(a);
    let played = Size::new(400.0, 300.0);
    p.set_play_size(Some(played));
    p.input(Input::SetMode(dark));
    assert_eq!(number(&p, n), 2.0, "fixture: dark");
    let size = |p: &Player| p.document().get(a).and_then(|x| x.kind().box_size());
    p.take_dirty();

    let tx = Transaction(vec![
        Operation::SetModes {
            id: fx.page,
            modes: sorted(vec![light, brand_b]),
        },
        Operation::SetGeometry {
            id: a,
            geometry: mastik_core::op::GeometryPatch::Size(Size::new(200.0, 150.0)),
        },
    ]);
    fx.doc.apply(&tx).unwrap();
    p.apply_edit(&tx).unwrap();
    assert!(
        !p.take_dirty().contains(&other),
        "taken, not re-cloned: a re-clone dirties every layer"
    );
    assert_eq!(number(&p, n), 2.0, "the played mode holds");
    assert_eq!(p.modes()[0].current, dark);
    assert_eq!(
        p.document().get(fx.page).unwrap().modes(),
        sorted(vec![dark, brand_b]).as_slice(),
        "the other collection's pick shows"
    );
    assert_eq!(size(&p), Some(played), "the play size holds");
}

/// A *while hovering* *Set mode* gives the screen its mode while the pointer
/// is on the layer and puts the saved one back on leaving; play then claims
/// no mode, so a later live edit of the screen's modes shows.
///
/// flip: `undo` passing over `Undo::Modes` → fails at "the saved mode back";
/// the arm keeping play's record of the mode → fails at "play claims no
/// mode".
#[test]
fn a_hover_set_mode_comes_back_and_leaves_live_edits_alone() {
    let mut fx = Fx::new();
    let a = fx.frame("A", 0.0, 200.0, 100.0);
    let card = fx.rect(a, "Card", 10.0, 10.0, 50.0, 30.0);
    let (light, dark, n) = dark_mode(&mut fx);
    fx.apply(vec![Operation::SetModes {
        id: a,
        modes: vec![light],
    }]);
    fx.on(
        card,
        Trigger::HoverWhile,
        vec![Action::SetMode {
            mode: dark,
            scope: ModeScope::Screen,
        }],
    );
    let mut p = fx.play(a);
    p.input(Input::PointerMove(Point::new(20.0, 20.0)));
    assert_eq!(number(&p, n), 2.0, "dark while hovering");
    p.input(Input::PointerMove(Point::new(150.0, 80.0)));
    assert_eq!(number(&p, n), 1.0, "the saved mode back");
    assert!(p.modes.is_empty(), "play claims no mode");

    let tx = Transaction(vec![Operation::SetModes {
        id: a,
        modes: vec![dark],
    }]);
    fx.doc.apply(&tx).unwrap();
    p.apply_edit(&tx).unwrap();
    assert_eq!(number(&p, n), 2.0, "the edit shows");
}

/// A *Dark* mode added beside the first collection's light one, each variable
/// there reading in it as in light, and a number variable reading 1 in light
/// and 2 in dark. Answers light, dark and the variable.
fn dark_mode(fx: &mut Fx) -> (ModeId, ModeId, VarId) {
    let light = fx.mode;
    let dark = fx.ids.mint_mode();
    let cells = fx
        .doc
        .variables()
        .iter()
        .filter_map(|v| Some((v.id, v.cells.get(&light)?.clone())))
        .collect();
    fx.apply(vec![Operation::AddMode {
        collection: CollectionId::FIRST,
        mode: Mode {
            id: dark,
            name: "Dark".into(),
        },
        index: 1,
        cells,
    }]);
    let n = fx.ids.mint_var();
    fx.apply(vec![Operation::AddVariable {
        var: Variable {
            id: n,
            name: "n".into(),
            kind: VarKind::Number,
            cells: [(light, Cell::Number(1.0)), (dark, Cell::Number(2.0))].into(),
            scope: None,
            origin: None,
            forked: None,
            description: String::new(),
        },
    }]);
    (light, dark, n)
}

/// A refused edit's re-clone keeps an instance play switched, a mode *Set
/// mode* gave and an open overlay; a later edit deleting the overlay's frame
/// closes it and one deleting a played variable drops its value, each said,
/// while a variable whose starting value was edited keeps its play value.
///
/// flip: `Player::reclone` not re-switching the instances → fails at "the
/// state is re-switched"; not putting the modes back → fails at "the mode is
/// given again".
#[test]
fn a_live_edit_carries_states_modes_and_overlays_and_falls_back() {
    let (mut fx, a, instance, [_, hover, _]) = button_set();
    let (light, dark, n) = dark_mode(&mut fx);
    let mut zero = |name: &str| {
        let id = fx.ids.mint_var();
        fx.apply(vec![Operation::AddVariable {
            var: Variable {
                id,
                name: name.into(),
                kind: VarKind::Number,
                cells: [(light, Cell::Number(0.0)), (dark, Cell::Number(0.0))].into(),
                scope: None,
                origin: None,
                forked: None,
                description: String::new(),
            },
        }]);
        id
    };
    let (m, gone) = (zero("m"), zero("gone"));
    let doomed = fx.rect(a, "Doomed", 100.0, 80.0, 10.0, 10.0);
    let menu = fx.frame("Menu", 900.0, 80.0, 40.0);
    let go = fx.rect(a, "Go", 150.0, 50.0, 40.0, 40.0);
    fx.on(
        go,
        Trigger::Click,
        vec![
            Action::ChangeState {
                instance,
                to: hover,
                transition: Transition::INSTANT,
            },
            Action::OpenOverlay {
                overlay: menu,
                settings: OverlaySettings::default(),
                transition: Transition::INSTANT,
            },
        ],
    );
    let mut p = fx.play(a);
    click(&mut p, 160.0, 60.0);
    p.input(Input::SetMode(dark));
    p.set_value(m, Value::Number(5.0)).unwrap();
    p.set_value(gone, Value::Number(3.0)).unwrap();
    let link = |p: &Player| p.doc.get(instance).unwrap().link();
    assert_eq!(link(&p), Some(hover), "fixture: switched");
    assert_eq!(p.overlays(), vec![menu], "fixture: open");
    assert_eq!(number(&p, n), 2.0, "fixture: dark");

    // Refused: play deleted a layer the edit renames.
    p.commit(Transaction(vec![Operation::DeleteNode { id: doomed }]))
        .unwrap();
    let tx = Transaction(vec![Operation::SetName {
        id: doomed,
        name: "Renamed".into(),
    }]);
    fx.doc.apply(&tx).unwrap();
    p.apply_edit(&tx).unwrap();
    assert_eq!(p.doc.get(doomed).unwrap().name(), "Renamed", "re-cloned");
    assert_eq!(link(&p), Some(hover), "the state is re-switched");
    assert_eq!(number(&p, n), 2.0, "the mode is given again");
    assert_eq!(p.overlays(), vec![menu], "the overlay is kept");
    assert!(p.take_notices().is_empty());

    let tx = Transaction(vec![
        Operation::DeleteNode { id: menu },
        Operation::RemoveVariable { id: gone },
        Operation::SetVariableCell {
            id: m,
            mode: light,
            cell: Cell::Number(9.0),
        },
    ]);
    fx.doc.apply(&tx).unwrap();
    p.apply_edit(&tx).unwrap();
    assert!(p.overlays().is_empty(), "the overlay's frame went: closed");
    let notices = p.take_notices();
    assert!(notices.contains(&Notice::Fallback(crate::Fallback::Overlay)));
    assert!(notices.contains(&Notice::Fallback(crate::Fallback::Variable)));
    assert_eq!(number(&p, m), 5.0, "an edited start keeps the play value");
}

/// A document sent whole is played in place of the old one, keeping the
/// screen and the play values; a later one without the screen falls back.
///
/// flip: `Player::rebase` not cloning the copy again from the new document →
/// fails at "the new document is played".
#[test]
fn a_rebase_plays_the_new_document_where_play_was() {
    let (mut fx, a, b, _) = two_screens(Transition::INSTANT);
    let n = fx.var("n", VarKind::Number, Cell::Number(0.0));
    let mut p = fx.play(a);
    click(&mut p, 20.0, 20.0);
    p.set_value(n, Value::Number(7.0)).unwrap();
    assert_eq!(p.screen(), b, "fixture: on B");
    let extra = fx.ids.mint();
    fx.apply(vec![Operation::CreateNode {
        id: extra,
        parent: b,
        index: 0,
        kind: frame_kind(5.0, 5.0),
        transform: None,
        name: Some("Extra".into()),
    }]);
    p.rebase(&fx.doc);
    assert!(p.doc.get(extra).is_some(), "the new document is played");
    assert_eq!((p.screen(), number(&p, n)), (b, 7.0), "play kept");
    fx.apply(vec![Operation::DeleteNode { id: b }]);
    p.rebase(&fx.doc);
    assert_eq!(p.screen(), a, "fell back to the start");
}

// ── Takes ───────────────────────────────────────────────────────────────────

/// A take recorded at an uneven frame rate replays at sixty frames a second to
/// the same state and the same scene.
///
/// flip: `Recording` stamping each input with the next tick's time (the input
/// stamped `now + 7`) → fails at "the same scene".
#[test]
fn a_take_replays_to_the_same_state() {
    let (mut fx, a, b, _) = two_screens(linear(TransitionKind::Dissolve, 300));
    let n = fx.var("n", VarKind::Number, Cell::Number(0.0));
    fx.on(
        b,
        Trigger::After { ms: 250 },
        vec![Action::SetVariable {
            var: n,
            change: VarChange::Increment { by: 5.0 },
        }],
    );
    let mut p = fx.play(a);
    p.record();
    p.tick(13.0);
    p.input(Input::PointerMove(Point::new(20.0, 20.0)));
    p.tick(41.0);
    click(&mut p, 20.0, 20.0);
    for dt in [7.0, 33.0, 19.0, 101.0, 3.0, 60.0] {
        p.tick(dt);
    }
    p.set_value(n, Value::Number(2.0)).unwrap();
    p.tick(77.0);
    let take: Take = p.stop_recording().unwrap();
    let mut replay = crate::Replay::new(&fx.doc, take.clone(), PLAYER).unwrap();
    let frame = 1000.0 / 60.0;
    while replay.player().now() + frame <= take.length {
        replay.advance(frame);
    }
    replay.finish();
    assert!(replay.is_done());
    let r = replay.player();
    assert_eq!(r.now(), p.now(), "at the take's end");
    assert_eq!(r.screen(), p.screen());
    assert_eq!(r.history(), p.history());
    assert_eq!(number(r, n), number(&p, n));
    assert_eq!(r.scene(), p.scene(), "the same scene");
}

/// A take opens with the play size and hand it was recorded with, and records
/// a change of size: the replay lays the screen out as the run did, so a tap
/// that lands only at the play size lands again, and a finger hovers nothing.
///
/// flip: `Player::record` opening the take with no `PlaySize` → fails at "laid
/// out as recorded"; with no `Touch` → fails at "the replay's finger".
#[test]
fn a_take_replays_at_its_play_size_and_hand() {
    use mastik_core::container::{Display, LayoutItem};
    let mut fx = Fx::new();
    let a = fx.frame("A", 0.0, 200.0, 100.0);
    let bar = fx.rect(a, "Bar", 0.0, 0.0, 50.0, 20.0);
    fx.apply(vec![
        Operation::SetDisplay {
            id: a,
            display: Some(Display::Flex(Default::default())),
        },
        Operation::SetLayoutItem {
            id: bar,
            item: LayoutItem {
                grow: 1.0,
                ..Default::default()
            },
        },
    ]);
    let n = fx.var("n", VarKind::Number, Cell::Number(0.0));
    fx.on(bar, Trigger::Click, vec![add(n, 1.0)]);
    fx.on(bar, Trigger::HoverEnter, vec![add(n, 100.0)]);
    fx.on(
        a,
        Trigger::Key {
            key: "Enter".into(),
            shift: false,
        },
        vec![add(n, 10.0)],
    );
    let mut p = fx.play(a);
    p.set_play_size(Some(Size::new(400.0, 100.0)));
    p.input(Input::Touch(true));
    p.record();
    p.tick(20.0);
    click(&mut p, 300.0, 10.0);
    p.tick(400.0);
    p.set_play_size(Some(Size::new(500.0, 100.0)));
    p.input(Input::Key {
        key: "Enter".into(),
        shift: false,
    });
    p.tick(30.0);
    assert_eq!(
        number(&p, n),
        11.0,
        "fixture: the tap and key landed, no hover"
    );
    let take = p.stop_recording().unwrap();

    let mut replay = crate::Replay::new(&fx.doc, take, PLAYER).unwrap();
    assert_eq!(
        replay.player().scene().size,
        Size::new(400.0, 100.0),
        "laid out as recorded from the first frame"
    );
    replay.finish();
    let r = replay.player();
    assert_eq!(r.play_size(), p.play_size(), "the change of size replayed");
    assert_eq!(
        number(r, n),
        11.0,
        "the replay's finger taps and hovers nothing"
    );
    assert_eq!(r.scene(), p.scene());
}

// ── Gestures ────────────────────────────────────────────────────────────────

/// A frame 200 by 100 holding a card (10,10 50×30), and a number variable.
fn one_card() -> (Fx, NodeId, NodeId, VarId) {
    let mut fx = Fx::new();
    let a = fx.frame("A", 0.0, 200.0, 100.0);
    let card = fx.rect(a, "Card", 10.0, 10.0, 50.0, 30.0);
    let n = fx.var("n", VarKind::Number, Cell::Number(0.0));
    (fx, a, card, n)
}

fn add(var: VarId, by: f64) -> Action {
    Action::SetVariable {
        var,
        change: VarChange::Increment { by },
    }
}

/// A finger hovers nothing: *On hover* waits for a mouse, and a tap still
/// clicks.
///
/// flip: `rehover` hit-testing a finger as it does a mouse → fails at "a finger
/// does not hover".
#[test]
fn a_finger_fires_no_hover() {
    let (mut fx, a, card, n) = one_card();
    fx.on(card, Trigger::HoverEnter, vec![add(n, 1.0)]);
    fx.on(card, Trigger::Click, vec![add(n, 10.0)]);
    let mut p = fx.play(a);
    p.input(Input::Touch(true));
    let at = Point::new(20.0, 20.0);
    p.input(Input::PointerMove(at));
    p.input(Input::PointerDown(at));
    p.input(Input::PointerUp(at));
    p.tick(500.0);
    assert_eq!(number(&p, n), 10.0, "a finger does not hover, and taps");
    p.input(Input::Touch(false));
    assert_eq!(number(&p, n), 11.0, "the mouse over it hovers");
}

/// *On leave* runs once as the pointer leaves the layer, and again when it
/// leaves the player from over it.
///
/// flip: `Player::rehover` not firing *HoverLeave* for the layers left → fails at
/// "left once".
#[test]
fn leaving_a_layer_fires_on_leave() {
    let (mut fx, a, card, n) = one_card();
    fx.on(card, Trigger::HoverLeave, vec![add(n, 1.0)]);
    let mut p = fx.play(a);
    p.input(Input::PointerMove(Point::new(20.0, 20.0)));
    assert_eq!(number(&p, n), 0.0, "entering is not leaving");
    p.input(Input::PointerMove(Point::new(150.0, 80.0)));
    assert_eq!(number(&p, n), 1.0, "left once");
    p.input(Input::PointerMove(Point::new(20.0, 20.0)));
    p.input(Input::PointerLeave);
    assert_eq!(number(&p, n), 2.0, "leaving the player leaves the layer");
}

/// On a layer with a click and a double click, one click waits out the double
/// click's window and then runs; two quick clicks run the double click alone.
///
/// flip: `Player::click` firing the click at once → fails at "held until the
/// window passes".
#[test]
fn a_double_click_holds_the_click() {
    let (mut fx, a, card, n) = one_card();
    fx.on(card, Trigger::Click, vec![add(n, 1.0)]);
    fx.on(card, Trigger::DoubleClick, vec![add(n, 10.0)]);
    let mut p = fx.play(a);
    click(&mut p, 20.0, 20.0);
    assert_eq!(number(&p, n), 0.0, "held until the window passes");
    assert!(p.is_moving(), "a held click keeps the clock asked for");
    p.tick(300.0);
    assert_eq!(number(&p, n), 1.0, "the click runs once no second came");
    click(&mut p, 20.0, 20.0);
    p.tick(100.0);
    click(&mut p, 20.0, 20.0);
    p.tick(300.0);
    assert_eq!(number(&p, n), 11.0, "the double click, and no click");
}

/// A layer with a click and no double click runs its click at once.
#[test]
fn a_lone_click_is_not_held() {
    let (mut fx, a, card, n) = one_card();
    fx.on(card, Trigger::Click, vec![add(n, 1.0)]);
    let mut p = fx.play(a);
    click(&mut p, 20.0, 20.0);
    assert_eq!(number(&p, n), 1.0);
}

/// A long press fires at its time while held and spends the press, so no click
/// follows; a shorter press is a click.
///
/// flip: `Player::fire_gestures` not setting `spent` → fails at "no click after
/// a long press".
#[test]
fn a_long_press_fires_while_held_and_takes_the_click() {
    let (mut fx, a, card, n) = one_card();
    fx.on(card, Trigger::Click, vec![add(n, 1.0)]);
    fx.on(card, Trigger::LongPress { ms: 500 }, vec![add(n, 10.0)]);
    let mut p = fx.play(a);
    let at = Point::new(20.0, 20.0);
    p.input(Input::PointerMove(at));
    p.input(Input::PointerDown(at));
    p.tick(499.0);
    assert_eq!(number(&p, n), 0.0, "not yet");
    p.tick(2.0);
    assert_eq!(number(&p, n), 10.0, "fired while held");
    p.input(Input::PointerUp(at));
    assert_eq!(number(&p, n), 10.0, "no click after a long press");
    p.input(Input::PointerDown(at));
    p.tick(100.0);
    p.input(Input::PointerUp(at));
    p.tick(1000.0);
    assert_eq!(number(&p, n), 11.0, "a short press is a click");
}

/// The deepest layer with a click takes it and the frame beneath does not; a
/// held click runs at once when the next click lands on another layer; a drag
/// cancels a long press.
///
/// flip: `fire` running every holder in the chain → fails at "the deepest
/// only"; `click` dropping the held tap → fails at "the held click ran"; a
/// drag not clearing the press's long presses → fails at "no long press after
/// a drag".
#[test]
fn a_click_goes_to_the_deepest_and_a_held_one_is_not_lost() {
    let (mut fx, a, card, n) = one_card();
    let other = fx.rect(a, "Other", 100.0, 10.0, 50.0, 30.0);
    fx.on(card, Trigger::Click, vec![add(n, 1.0)]);
    fx.on(a, Trigger::Click, vec![add(n, 100.0)]);
    let mut p = fx.play(a);
    click(&mut p, 20.0, 20.0);
    assert_eq!(number(&p, n), 1.0, "the deepest only");

    fx.on(card, Trigger::DoubleClick, vec![add(n, 10.0)]);
    fx.on(other, Trigger::Click, vec![add(n, 1000.0)]);
    let mut p = fx.play(a);
    click(&mut p, 20.0, 20.0);
    p.tick(100.0);
    click(&mut p, 110.0, 20.0);
    assert_eq!(number(&p, n), 1001.0, "the held click ran");

    let (mut fx, a, card, n) = one_card();
    fx.on(card, Trigger::LongPress { ms: 500 }, vec![add(n, 10.0)]);
    let mut p = fx.play(a);
    let at = Point::new(20.0, 20.0);
    p.input(Input::PointerMove(at));
    p.input(Input::PointerDown(at));
    p.input(Input::PointerMove(Point::new(40.0, 20.0)));
    p.tick(600.0);
    assert_eq!(number(&p, n), 0.0, "no long press after a drag");
}

/// Two *variable changes* reactions that set each other's variable stop at the
/// depth limit with one `Loop` notice, and play goes on.
///
/// flip: `run_reaction` without its `MAX_DEPTH` guard → the stack overflows.
#[test]
fn reactions_setting_each_other_off_stop_with_one_notice() {
    let (mut fx, a, card, n) = one_card();
    let m = fx.var("m", VarKind::Number, Cell::Number(0.0));
    fx.on(a, Trigger::VariableChanges { var: n }, vec![add(m, 1.0)]);
    fx.on(a, Trigger::VariableChanges { var: m }, vec![add(n, 1.0)]);
    fx.on(card, Trigger::Click, vec![add(n, 1.0)]);
    let mut p = fx.play(a);
    click(&mut p, 20.0, 20.0);
    assert_eq!(p.take_notices(), vec![Notice::Loop], "said once");
    assert!(number(&p, n) > 1.0, "it ran until the limit");
    click(&mut p, 20.0, 20.0);
    assert!(number(&p, n) > 2.0, "play goes on");
}

/// A quick drag left fires *On swipe left* and not the right; a slow one is no
/// swipe; a drag the other way is left to the scroll.
///
/// flip: `side_of` reading the move's sign backwards → fails at "swiped left".
#[test]
fn a_swipe_goes_by_its_side_and_its_speed() {
    let (mut fx, a, card, n) = one_card();
    fx.play_settings(
        a,
        LayerPlay {
            scroll: ScrollAxis::Vertical,
            ..LayerPlay::default()
        },
    );
    fx.rect(a, "Long", 0.0, 50.0, 10.0, 300.0);
    fx.on(card, Trigger::Swipe { side: Side::Left }, vec![add(n, 1.0)]);
    fx.on(
        card,
        Trigger::Swipe { side: Side::Right },
        vec![add(n, 10.0)],
    );
    let mut p = fx.play(a);
    let drag = |p: &mut Player, from: (f64, f64), to: (f64, f64), ms: f64| {
        let mid = ((from.0 + to.0) / 2.0, (from.1 + to.1) / 2.0);
        p.input(Input::PointerMove(from.into()));
        p.input(Input::PointerDown(from.into()));
        p.input(Input::PointerMove(mid.into()));
        p.tick(ms);
        p.input(Input::PointerMove(to.into()));
        p.input(Input::PointerUp(to.into()));
    };
    drag(&mut p, (55.0, 20.0), (15.0, 22.0), 100.0);
    assert_eq!(number(&p, n), 1.0, "swiped left");
    drag(&mut p, (55.0, 20.0), (15.0, 22.0), 2000.0);
    assert_eq!(number(&p, n), 1.0, "a slow drag is no swipe");
    drag(&mut p, (20.0, 38.0), (22.0, 12.0), 100.0);
    assert_eq!(number(&p, n), 1.0, "up is no side the card takes");
    assert_eq!(
        p.scroll_offset(a),
        Vec2::new(0.0, 26.0),
        "the frame scrolled"
    );
}

/// A scroll trigger fires once as the offset crosses its point the way it
/// goes, a jump across included, and not while past it.
///
/// flip: `Player::check_scrolls` testing `now >= at` alone (while past, not the
/// crossing) → fails at "not again while past".
#[test]
fn a_scroll_trigger_fires_on_the_crossing() {
    let (mut fx, a, card, n) = one_card();
    fx.play_settings(
        a,
        LayerPlay {
            scroll: ScrollAxis::Vertical,
            ..LayerPlay::default()
        },
    );
    fx.rect(a, "Long", 0.0, 50.0, 10.0, 400.0);
    let past = |going| Trigger::ScrollPast {
        frame: a,
        at: 100.0,
        going,
    };
    fx.on(card, past(Going::Forward), vec![add(n, 1.0)]);
    fx.on(card, past(Going::Back), vec![add(n, 10.0)]);
    let mut p = fx.play(a);
    let wheel = |p: &mut Player, dy: f64| {
        p.input(Input::Wheel {
            at: Point::new(100.0, 50.0),
            delta: Vec2::new(0.0, dy),
        });
    };
    wheel(&mut p, 60.0);
    assert_eq!(number(&p, n), 0.0, "short of the point");
    wheel(&mut p, 60.0);
    assert_eq!(number(&p, n), 1.0, "crossed going down");
    wheel(&mut p, 60.0);
    assert_eq!(number(&p, n), 1.0, "not again while past");
    wheel(&mut p, -500.0);
    assert_eq!(number(&p, n), 11.0, "a jump back across fires once");
}

/// A scroll trigger watching a frame that is gone says so once, the first time
/// its screen scrolls, and fires nothing.
///
/// flip: `check_scrolls` skipping the gone frame silently → fails at "said once".
#[test]
fn a_scroll_trigger_on_a_gone_frame_says_so() {
    let (mut fx, a, card, n) = one_card();
    fx.play_settings(
        a,
        LayerPlay {
            scroll: ScrollAxis::Vertical,
            ..LayerPlay::default()
        },
    );
    fx.rect(a, "Long", 0.0, 50.0, 10.0, 400.0);
    let gone = IdSource::new(0xDEAD).mint();
    fx.on(
        card,
        Trigger::ScrollPast {
            frame: gone,
            at: 10.0,
            going: Going::Forward,
        },
        vec![add(n, 1.0)],
    );
    let mut p = fx.play(a);
    for _ in 0..2 {
        p.input(Input::Wheel {
            at: Point::new(100.0, 50.0),
            delta: Vec2::new(0.0, 60.0),
        });
    }
    let said = p
        .take_notices()
        .into_iter()
        .filter(|x| matches!(x, Notice::ScrollFrameGone { layer } if *layer == card))
        .count();
    assert_eq!(said, 1, "said once");
    assert_eq!(number(&p, n), 0.0, "fires nothing");
}

// ── Scroll kept, modes, snapping ────────────────────────────────────────────

/// A scrolling frame A (200 by 100, content 300 tall) and a frame B; the key
/// `B` goes to B, and the key `A` comes back to A keeping scroll when `keep`.
fn there_and_back(keep: bool) -> (Fx, NodeId, NodeId) {
    let mut fx = Fx::new();
    let a = fx.frame("A", 0.0, 200.0, 100.0);
    let b = fx.frame("B", 300.0, 200.0, 100.0);
    fx.play_settings(
        a,
        LayerPlay {
            scroll: ScrollAxis::Vertical,
            ..LayerPlay::default()
        },
    );
    let long = fx.rect(a, "Long", 0.0, 0.0, 200.0, 300.0);
    let back = fx.rect(b, "Back", 0.0, 0.0, 20.0, 20.0);
    let go = |to, keep_scroll| Action::Navigate {
        to,
        transition: Transition::INSTANT,
        keep_scroll,
    };
    let key = |k: &str| Trigger::Key {
        key: k.into(),
        shift: false,
    };
    fx.on(long, key("B"), vec![go(b, false)]);
    fx.on(back, key("A"), vec![go(a, keep)]);
    (fx, a, b)
}

/// *Navigate* with *Keep scroll position* opens its frame where it was left;
/// without, at its top; *Back* past a kept navigation keeps it too.
///
/// flip: `Player::show` clearing the offsets whatever `keep` says → fails at
/// "kept where it was left".
#[test]
fn navigate_keeps_scroll_when_asked() {
    for keep in [false, true] {
        let (fx, a, _) = there_and_back(keep);
        let mut p = fx.play(a);
        p.input(Input::Wheel {
            at: Point::new(100.0, 50.0),
            delta: Vec2::new(0.0, 80.0),
        });
        key(&mut p, "B", false);
        key(&mut p, "A", false);
        let want = if keep { 80.0 } else { 0.0 };
        assert_eq!(
            p.scroll_offset(a),
            Vec2::new(0.0, want),
            "kept where it was left (keep: {keep})"
        );
    }
}

/// *Set mode* switches what a variable reads for the run, everywhere or on the
/// screen alone, and the state card's row follows; a frame pinning its own
/// mode keeps it, the document is untouched, a restart starts from the saved
/// modes, and a mode that is not there is said once.
///
/// flip: `Player::set_mode_on` filtering by the wrong collection (keeping the
/// old mode beside the new) → fails at "reads the dark column"; `run_action`'s
/// `SetMode` setting *This screen* everywhere → fails at "set on the screen";
/// `restart` putting the played modes back → fails at "a restart reads
/// the saved modes".
#[test]
fn set_mode_switches_what_variables_read() {
    let mut fx = Fx::new();
    let a = fx.frame("A", 0.0, 200.0, 100.0);
    let light = fx.mode;
    let dark = fx.ids.mint_mode();
    fx.apply(vec![Operation::AddMode {
        collection: CollectionId::FIRST,
        mode: Mode {
            id: dark,
            name: "Dark".into(),
        },
        index: 1,
        cells: vec![],
    }]);
    let n = fx.ids.mint_var();
    fx.apply(vec![Operation::AddVariable {
        var: Variable {
            id: n,
            name: "n".into(),
            kind: VarKind::Number,
            cells: [(light, Cell::Number(1.0)), (dark, Cell::Number(2.0))].into(),
            scope: None,
            origin: None,
            forked: None,
            description: String::new(),
        },
    }]);
    let b = fx.frame("B", 300.0, 200.0, 100.0);
    let pinned = fx.frame("Pinned", 600.0, 200.0, 100.0);
    fx.apply(vec![Operation::SetModes {
        id: pinned,
        modes: vec![light],
    }]);
    let card = fx.rect(a, "Card", 10.0, 10.0, 50.0, 30.0);
    fx.on(
        card,
        Trigger::Click,
        vec![Action::SetMode {
            mode: dark,
            scope: ModeScope::Everywhere,
        }],
    );
    let here = fx.rect(a, "Here", 100.0, 10.0, 30.0, 30.0);
    fx.on(
        here,
        Trigger::Click,
        vec![Action::SetMode {
            mode: dark,
            scope: ModeScope::Screen,
        }],
    );
    let go = fx.rect(a, "Go", 100.0, 60.0, 30.0, 30.0);
    fx.on(
        go,
        Trigger::Click,
        vec![Action::Navigate {
            to: b,
            transition: Transition::INSTANT,
            keep_scroll: false,
        }],
    );
    let gone = fx.ids.mint_mode();
    let bad = fx.rect(a, "Bad", 150.0, 10.0, 30.0, 30.0);
    fx.on(
        bad,
        Trigger::Click,
        vec![Action::SetMode {
            mode: gone,
            scope: ModeScope::Everywhere,
        }],
    );

    // This screen.
    let mut p = fx.play(a);
    click(&mut p, 110.0, 20.0);
    assert_eq!(number(&p, n), 2.0, "the screen reads dark");
    assert_eq!(p.modes()[0].current, dark);
    assert!(p.modes()[0].screen_sets, "set on the screen");
    click(&mut p, 110.0, 70.0);
    assert_eq!(p.screen(), b, "fixture: navigated");
    assert_eq!(number(&p, n), 1.0, "B reads the saved mode");
    assert!(!p.modes()[0].screen_sets);

    // Everywhere.
    let mut p = fx.play(a);
    assert_eq!(number(&p, n), 1.0);
    assert_eq!(p.modes()[0].current, light);
    click(&mut p, 20.0, 20.0);
    assert_eq!(number(&p, n), 2.0, "reads the dark column");
    assert_eq!(p.modes()[0].current, dark, "the card follows");
    assert!(!p.modes()[0].screen_sets, "set on the page, not the screen");
    assert_eq!(
        p.doc.resolve_var(pinned, n),
        Some(Value::Number(1.0)),
        "a frame's own pin keeps it"
    );
    assert_eq!(
        fx.doc.resolve_var(a, n),
        Some(Value::Number(1.0)),
        "the document is untouched"
    );
    p.restart();
    assert_eq!(number(&p, n), 1.0, "a restart reads the saved modes");
    click(&mut p, 20.0, 20.0);
    p.input(Input::SetMode(light));
    assert_eq!(number(&p, n), 1.0, "a pick from the card");

    // A mode that is not there.
    let _ = p.take_notices();
    click(&mut p, 160.0, 20.0);
    click(&mut p, 160.0, 20.0);
    assert_eq!(number(&p, n), 1.0, "nothing set");
    assert_eq!(
        p.take_notices(),
        vec![Notice::Missing {
            layer: bad,
            action: fx.doc.get(bad).unwrap().reactions()[0].value.actions[0].id,
        }],
        "said once"
    );
}

/// A flow's starting mode is set everywhere as play starts, again on a
/// restart, and the state card lists a kit's collection after the document's.
///
/// flip: `Player::new` skipping the flow's modes → fails at "starts dark";
/// `Player::everywhere` naming the pages for a kit's mode, which a page cannot
/// hold → fails at "the kit's starting mode".
#[test]
fn a_flow_starts_at_its_modes_and_the_card_lists_kits() {
    const KIT: &str = "5eed0000000000000000000000000011";
    let mut fx = Fx::new();
    let a = fx.frame("A", 0.0, 200.0, 100.0);
    let light = fx.mode;
    let dark = fx.ids.mint_mode();
    fx.apply(vec![Operation::AddMode {
        collection: CollectionId::FIRST,
        mode: Mode {
            id: dark,
            name: "Dark".into(),
        },
        index: 1,
        cells: vec![],
    }]);
    let n = fx.ids.mint_var();
    fx.apply(vec![Operation::AddVariable {
        var: Variable {
            id: n,
            name: "n".into(),
            kind: VarKind::Number,
            cells: [(light, Cell::Number(1.0)), (dark, Cell::Number(2.0))].into(),
            scope: None,
            origin: None,
            forked: None,
            description: String::new(),
        },
    }]);
    let (k1, k2) = (fx.ids.mint_mode(), fx.ids.mint_mode());
    let kit_mode = |id, name: &str| mastik_core::variable::KitMode {
        id,
        name: name.into(),
        from: id,
        collection: "Density".into(),
    };
    fx.apply(vec![Operation::SetKitModes {
        kit: KIT.into(),
        modes: Some(mastik_core::variable::KitModes {
            name: Some("Brand".into()),
            modes: vec![kit_mode(k1, "Roomy"), kit_mode(k2, "Tight")],
            map: Default::default(),
        }),
    }]);
    let flow = fx.ids.mint_item();
    let page = fx.page;
    fx.apply(vec![Operation::SetFlows {
        id: page,
        flows: vec![Keyed::new(
            flow,
            Flow {
                name: "Dark".into(),
                start: a,
                modes: vec![dark, k2],
                values: Vec::new(),
            },
        )],
    }]);
    let mut p = Player::new(
        &fx.doc,
        Start::Flow(mastik_core::prototype::FlowRef { page, flow }),
        PLAYER,
    )
    .unwrap();
    assert_eq!(number(&p, n), 2.0, "starts dark");
    let rows = p.modes();
    assert_eq!(rows.len(), 2, "the document's collection and the kit's");
    assert_eq!(
        (rows[1].name.as_str(), rows[1].kit.as_deref()),
        ("Density", Some(KIT))
    );
    assert_eq!(rows[1].current, k2, "the kit's starting mode");
    p.input(Input::SetMode(light));
    p.restart();
    assert_eq!(number(&p, n), 2.0, "a restart starts the flow dark again");
}

/// A frame snapping to its children rests each one at its start, centre or end
/// as its alignment says, the frame's inset kept at both ends, every point held
/// within the scroll; snapping by pages rests every frame's width and at the end.
///
/// flip: *End* resting a child's end at the frame's edge (`b - size`) → fails at
/// "end"; *Pages* without its last point at the end → fails at "pages".
#[test]
fn snap_points_follow_the_alignment_and_the_pages() {
    use mastik_core::prototype::SnapAlign;
    let mut fx = Fx::new();
    let a = fx.frame("A", 0.0, 100.0, 100.0);
    for (x, w) in [
        (10.0, 40.0),
        (60.0, 20.0),
        (90.0, 60.0),
        (160.0, 40.0),
        (210.0, 40.0),
    ] {
        fx.rect(a, "Card", x, 0.0, w, 100.0);
    }
    let points = |fx: &mut Fx, snap, snap_align| {
        fx.play_settings(
            a,
            LayerPlay {
                scroll: ScrollAxis::Horizontal,
                snap,
                snap_align,
                ..LayerPlay::default()
            },
        );
        fx.play(a).snap_points(a, true)
    };
    assert_eq!(
        points(&mut fx, Snap::Children, SnapAlign::Start),
        [0.0, 50.0, 80.0, 150.0],
        "start"
    );
    assert_eq!(
        points(&mut fx, Snap::Children, SnapAlign::Centre),
        [0.0, 20.0, 70.0, 130.0, 150.0],
        "centre"
    );
    assert_eq!(
        points(&mut fx, Snap::Children, SnapAlign::End),
        [0.0, 60.0, 110.0, 150.0],
        "end"
    );
    assert_eq!(
        points(&mut fx, Snap::Pages, SnapAlign::Start),
        [0.0, 100.0, 150.0],
        "pages"
    );
}

/// A following layer is held to its range with a third of the overshoot as
/// give: its parent's box, an empty span when it is wider than its parent,
/// or its outermost points; let go, it stays held back into range, or returns.
///
/// flip: `GIVE` at 1.0 → fails at "a third past the parent".
#[test]
fn a_following_layer_keeps_to_its_range() {
    use mastik_core::prototype::{DragPoint, Follow, FollowRange, Release};
    let setup = |w: f64, range, release, points: Vec<(f64, f64)>| {
        let mut fx = Fx::new();
        let a = fx.frame("A", 0.0, 200.0, 100.0);
        let card = fx.rect(a, "Card", 10.0, 10.0, w, 30.0);
        let points = points
            .into_iter()
            .map(|(x, y)| {
                Keyed::new(
                    fx.ids.mint_item(),
                    DragPoint {
                        name: "P".into(),
                        x,
                        y,
                    },
                )
            })
            .collect();
        fx.play_settings(
            card,
            LayerPlay {
                follow: Some(Follow {
                    range,
                    release,
                    settle: Timing::Ease {
                        curve: Easing::Linear,
                        ms: 100,
                    },
                    points,
                    ..Follow::default()
                }),
                ..LayerPlay::default()
            },
        );
        (fx.play(a), card)
    };
    let drag = |p: &mut Player, by: Vec2| {
        let at = Point::new(20.0, 20.0);
        p.input(Input::PointerMove(at));
        p.input(Input::PointerDown(at));
        p.input(Input::PointerMove(at + by * 0.5));
        p.tick(500.0);
        p.input(Input::PointerMove(at + by));
    };
    let up = |p: &mut Player, by: Vec2| {
        p.input(Input::PointerUp(Point::new(20.0, 20.0) + by));
        p.tick(500.0);
    };

    // Its parent's box: x from -10 to 140.
    let (mut p, card) = setup(50.0, FollowRange::Parent, Release::Stays, vec![]);
    let by = Vec2::new(200.0, 0.0);
    drag(&mut p, by);
    assert_eq!(
        p.moved(card),
        Vec2::new(160.0, 0.0),
        "a third past the parent"
    );
    up(&mut p, by);
    assert_eq!(p.moved(card), Vec2::new(140.0, 0.0), "held back into range");

    // Wider than its parent: nowhere to go, but the give.
    let (mut p, card) = setup(300.0, FollowRange::Parent, Release::Stays, vec![]);
    drag(&mut p, Vec2::new(30.0, 0.0));
    assert_eq!(p.moved(card), Vec2::new(10.0, 0.0), "an empty span");

    // Between its outermost points, the drawn place among them.
    let (mut p, card) = setup(
        50.0,
        FollowRange::Points,
        Release::Returns,
        vec![(60.0, 0.0)],
    );
    let by = Vec2::new(-30.0, 0.0);
    drag(&mut p, by);
    assert_eq!(p.moved(card), Vec2::new(-10.0, 0.0), "past the drawn point");
    up(&mut p, by);
    assert_eq!(p.moved(card), Vec2::ZERO, "returns");
}

/// A following layer let go where it rests drops at once: under *Stays* inside
/// its range; under *Returns* once it is back, after the settle. Either way its
/// *Drop (anywhere)* runs.
///
/// flip: `follow_release` not dropping when the layer is already where it
/// rests → fails at "dropped at once".
#[test]
fn a_following_layer_drops_where_it_rests() {
    use mastik_core::prototype::{DropAt, Follow, FollowRange, Release};
    let setup = |release| {
        let (mut fx, a, card, n) = one_card();
        fx.play_settings(
            card,
            LayerPlay {
                follow: Some(Follow {
                    range: FollowRange::Parent,
                    release,
                    settle: Timing::Ease {
                        curve: Easing::Linear,
                        ms: 100,
                    },
                    ..Follow::default()
                }),
                ..LayerPlay::default()
            },
        );
        fx.on(
            card,
            Trigger::Drop {
                at: DropAt::Anywhere,
            },
            vec![add(n, 1.0)],
        );
        (fx.play(a), n)
    };
    let drag = |p: &mut Player| {
        let at = Point::new(20.0, 20.0);
        p.input(Input::PointerMove(at));
        p.input(Input::PointerDown(at));
        p.input(Input::PointerMove(at + Vec2::new(10.0, 0.0)));
        p.tick(500.0);
        p.input(Input::PointerMove(at + Vec2::new(20.0, 0.0)));
        p.input(Input::PointerUp(at + Vec2::new(20.0, 0.0)));
    };

    let (mut p, n) = setup(Release::Stays);
    drag(&mut p);
    assert_eq!(number(&p, n), 1.0, "dropped at once");

    let (mut p, n) = setup(Release::Returns);
    drag(&mut p);
    assert_eq!(number(&p, n), 0.0, "fixture: settling");
    p.tick(500.0);
    assert_eq!(number(&p, n), 1.0, "dropped once back");
}

/// A carousel 100 wide of four 100-wide cards, snapping to its children: a
/// wheel gesture moves one card, and a short quick drag one card.
///
/// flip: `Player::wheel` scrolling a snapping frame freely instead of calling
/// `snap_step` → fails at "one step per gesture". Taking out `snap_step`'s
/// guard on a step under way does not bite: a second notch steps from the
/// partial offset to the same card, so the guard only keeps the motion from
/// restarting.
#[test]
fn a_snapping_frame_rests_on_its_children() {
    let mut fx = Fx::new();
    let a = fx.frame("A", 0.0, 100.0, 100.0);
    fx.play_settings(
        a,
        LayerPlay {
            scroll: ScrollAxis::Horizontal,
            snap: Snap::Children,
            ..LayerPlay::default()
        },
    );
    for i in 0..4 {
        fx.rect(
            a,
            &format!("Card {i}"),
            f64::from(i) * 100.0,
            0.0,
            100.0,
            100.0,
        );
    }
    let mut p = fx.play(a);
    let wheel = |p: &mut Player, dx: f64| {
        p.input(Input::Wheel {
            at: Point::new(50.0, 50.0),
            delta: Vec2::new(dx, 0.0),
        });
    };
    wheel(&mut p, 5.0);
    wheel(&mut p, 5.0);
    p.tick(1000.0);
    assert_eq!(
        p.scroll_offset(a),
        Vec2::new(100.0, 0.0),
        "one step per gesture"
    );
    // A short drag leftward (content forward) moves on one card.
    p.input(Input::PointerMove(Point::new(60.0, 50.0)));
    p.input(Input::PointerDown(Point::new(60.0, 50.0)));
    p.input(Input::PointerMove(Point::new(50.0, 50.0)));
    p.tick(50.0);
    p.input(Input::PointerMove(Point::new(40.0, 50.0)));
    p.input(Input::PointerUp(Point::new(40.0, 50.0)));
    p.tick(1000.0);
    assert_eq!(p.scroll_offset(a), Vec2::new(200.0, 0.0), "a drag steps on");
    wheel(&mut p, -5.0);
    p.tick(1000.0);
    assert_eq!(p.scroll_offset(a), Vec2::new(100.0, 0.0), "and back");
}

/// Smart animate moves a paired layer's corner radius and its drop shadow
/// rather than jumping to the end's.
///
/// flip: `Smart::patch_in` leaving `radii` unset → fails at "the radius half
/// way".
#[test]
fn smart_animate_moves_radius_and_shadow() {
    let mut fx = Fx::new();
    let a = fx.frame("A", 0.0, 200.0, 100.0);
    let b = fx.frame("B", 300.0, 200.0, 100.0);
    let box_a = fx.rect(a, "Box", 0.0, 0.0, 40.0, 40.0);
    let box_b = fx.node(
        b,
        "Box",
        NodeKind::Rect {
            size: Size::new(40.0, 40.0),
            corner_radii: RoundedRectRadii::from_single_radius(20.0),
            corner_smoothing: 0.0,
        },
        (0.0, 0.0),
    );
    let mut shadow = |blur: f64| {
        vec![Keyed::new(
            fx.ids.mint_item(),
            Effect::new(EffectKind::DropShadow(Shadow {
                blur,
                ..Shadow::default()
            })),
        )]
    };
    let (sa, sb) = (shadow(0.0), shadow(10.0));
    fx.apply(vec![
        Operation::SetEffects {
            id: box_a,
            effects: sa,
        },
        Operation::SetEffects {
            id: box_b,
            effects: sb,
        },
    ]);
    fx.on(
        box_a,
        Trigger::Click,
        vec![Action::Navigate {
            to: b,
            transition: linear(TransitionKind::SmartAnimate, 100),
            keep_scroll: false,
        }],
    );
    let mut p = fx.play(a);
    click(&mut p, 5.0, 5.0);
    p.tick(50.0);
    let s = p.scene();
    let patch = s.patches.iter().find(|x| x.node == box_b).expect("patched");
    assert_eq!(
        patch.radii.map(|r| r.top_left),
        Some(10.0),
        "the radius half way"
    );
    let blur = patch.effects.as_ref().and_then(|e| match &e[0].value.kind {
        EffectKind::DropShadow(s) => Some(s.blur),
        _ => None,
    });
    assert_eq!(blur, Some(5.0), "the shadow's blur half way");
}

/// A sheet that follows the drag down, held to its axis, settles on the point
/// nearest where it is let go and then runs its drops: the one for that point
/// and the one for anywhere, not another point's.
///
/// flip: `Player::follow_release` settling at the drawn place for *Snaps* →
/// fails at "rests at Closed".
#[test]
fn a_following_layer_snaps_to_its_point_and_drops() {
    use mastik_core::prototype::{DragPoint, DropAt, Follow, Release};
    let (mut fx, a, sheet, n) = one_card();
    let (peek, closed) = (fx.ids.mint_item(), fx.ids.mint_item());
    let point = |name: &str, y| DragPoint {
        name: name.into(),
        x: 0.0,
        y,
    };
    fx.play_settings(
        sheet,
        LayerPlay {
            follow: Some(Follow {
                axis: ScrollAxis::Vertical,
                release: Release::Snaps,
                settle: Timing::Ease {
                    curve: Easing::Linear,
                    ms: 100,
                },
                points: vec![
                    Keyed::new(peek, point("Peek", 30.0)),
                    Keyed::new(closed, point("Closed", 60.0)),
                ],
                ..Follow::default()
            }),
            ..LayerPlay::default()
        },
    );
    fx.on(
        sheet,
        Trigger::Drop {
            at: DropAt::Anywhere,
        },
        vec![add(n, 1.0)],
    );
    fx.on(
        sheet,
        Trigger::Drop {
            at: DropAt::Point(closed),
        },
        vec![add(n, 10.0)],
    );
    fx.on(
        sheet,
        Trigger::Drop {
            at: DropAt::Point(peek),
        },
        vec![add(n, 100.0)],
    );
    let mut p = fx.play(a);
    p.input(Input::PointerMove(Point::new(20.0, 20.0)));
    p.input(Input::PointerDown(Point::new(20.0, 20.0)));
    p.input(Input::PointerMove(Point::new(25.0, 40.0)));
    p.tick(500.0);
    p.input(Input::PointerMove(Point::new(30.0, 72.0)));
    assert_eq!(p.moved(sheet), Vec2::new(0.0, 52.0), "held to its axis");
    p.input(Input::PointerUp(Point::new(30.0, 72.0)));
    assert_eq!(number(&p, n), 0.0, "no drop before it settles");
    p.tick(200.0);
    assert_eq!(p.moved(sheet), Vec2::new(0.0, 60.0), "rests at Closed");
    assert_eq!(number(&p, n), 11.0, "its point's drop and anywhere's");
    // It is drawn, and hit, where it rests.
    let s = p.scene();
    let shift = s
        .patches
        .iter()
        .find(|x| x.node == sheet)
        .and_then(|x| x.transform)
        .map(|t| t.translation());
    assert_eq!(shift, Some(Vec2::new(10.0, 70.0)));
    assert_eq!(p.hotspots()[0].rect.y0, 70.0, "hit where it rests");
}

/// A scroll effect follows its frame's offset linearly across its range and
/// holds its end past it: half way, half faded and half moved; past the end,
/// at the end.
///
/// flip: `Player::scroll_effect_patch` not clamping the progress → fails at
/// "held at its end".
#[test]
fn a_scroll_effect_follows_the_offset() {
    use mastik_core::prototype::{EffectEnds, ScrollEffect, ScrollProp};
    let (mut fx, a, hero, _) = one_card();
    fx.play_settings(
        a,
        LayerPlay {
            scroll: ScrollAxis::Vertical,
            ..LayerPlay::default()
        },
    );
    fx.rect(a, "Long", 0.0, 0.0, 10.0, 500.0);
    let ends = |prop, start, end| EffectEnds { prop, start, end };
    fx.play_settings(
        hero,
        LayerPlay {
            scroll_effect: Some(ScrollEffect {
                frame: None,
                from: 0.0,
                to: 160.0,
                props: vec![
                    ends(ScrollProp::Opacity, 1.0, 0.0),
                    ends(ScrollProp::MoveY, 0.0, 60.0),
                ],
            }),
            ..LayerPlay::default()
        },
    );
    let mut p = fx.play(a);
    let look = |p: &mut Player, dy: f64| {
        p.input(Input::Wheel {
            at: Point::new(100.0, 50.0),
            delta: Vec2::new(0.0, dy),
        });
        let s = p.scene();
        let x = s.patches.iter().find(|x| x.node == hero).cloned().unwrap();
        (x.opacity, x.transform.map(|t| t.translation()))
    };
    let (o, at) = look(&mut p, 80.0);
    assert_eq!(o, Some(0.5));
    // Placed at 10,10, scrolled up 80, moved down 30.
    assert_eq!(at, Some(Vec2::new(10.0, -40.0)), "half way");
    let (o, at) = look(&mut p, 160.0);
    assert_eq!(o, Some(0.0));
    // Scrolled up 240, moved down its whole 60.
    assert_eq!(at, Some(Vec2::new(10.0, -170.0)), "held at its end");
}

/// A scroll effect naming a frame follows that frame's offset, though no
/// scrolling frame holds the layer: a card beside a scrolling list fades as
/// the list scrolls.
///
/// flip: `Player::effect_now` ignoring `ScrollEffect::frame` → fails at "the
/// card follows the list" (no frame above the card scrolls, so no patch).
#[test]
fn a_scroll_effect_follows_the_frame_it_names() {
    use mastik_core::prototype::{EffectEnds, ScrollEffect, ScrollProp};
    let (mut fx, a, card, _) = one_card();
    let list = fx.node(a, "List", frame_kind(200.0, 50.0), (0.0, 50.0));
    fx.play_settings(
        list,
        LayerPlay {
            scroll: ScrollAxis::Vertical,
            ..LayerPlay::default()
        },
    );
    fx.rect(list, "Long", 0.0, 0.0, 10.0, 500.0);
    fx.play_settings(
        card,
        LayerPlay {
            scroll_effect: Some(ScrollEffect {
                frame: Some(list),
                from: 0.0,
                to: 100.0,
                props: vec![EffectEnds {
                    prop: ScrollProp::Opacity,
                    start: 1.0,
                    end: 0.0,
                }],
            }),
            ..LayerPlay::default()
        },
    );
    let mut p = fx.play(a);
    p.input(Input::Wheel {
        at: Point::new(100.0, 75.0),
        delta: Vec2::new(0.0, 50.0),
    });
    let s = p.scene();
    let o = s
        .patches
        .iter()
        .find(|x| x.node == card)
        .and_then(|x| x.opacity);
    assert_eq!(o, Some(0.5), "the card follows the list");
}

/// A play size lays the screen out again through its flex: a child that grows
/// fills the new width, kept across a restart and given back with `None`; the
/// document is untouched.
///
/// flip: `Player::fit_screen` returning before it commits → fails at "the
/// child fills the new width".
#[test]
fn a_play_size_lays_the_screen_out_again() {
    use mastik_core::container::{Display, LayoutItem};
    let mut fx = Fx::new();
    let a = fx.frame("A", 0.0, 200.0, 100.0);
    let bar = fx.rect(a, "Bar", 0.0, 0.0, 50.0, 20.0);
    fx.apply(vec![
        Operation::SetDisplay {
            id: a,
            display: Some(Display::Flex(Default::default())),
        },
        Operation::SetLayoutItem {
            id: bar,
            item: LayoutItem {
                grow: 1.0,
                ..Default::default()
            },
        },
    ]);
    let width = |p: &Player| {
        p.resolved()
            .used_kind(p.document(), bar)
            .and_then(|k| k.box_size())
            .unwrap()
            .width
    };
    let mut p = fx.play(a);
    assert_eq!(width(&p), 200.0, "fixture: the child fills the frame");
    assert!(p.set_play_size(Some(Size::new(400.0, 100.0))));
    assert_eq!(width(&p), 400.0, "the child fills the new width");
    assert_eq!(
        p.scene().size,
        Size::new(400.0, 100.0),
        "the screen is that size"
    );
    p.restart();
    assert_eq!(width(&p), 400.0, "kept across a restart");
    p.set_play_size(None);
    assert_eq!(width(&p), 200.0, "given back as drawn");
    let doc_w = fx
        .doc
        .get(a)
        .and_then(|n| n.kind().box_size())
        .unwrap()
        .width;
    assert_eq!(doc_w, 200.0, "the document is untouched");
}

/// A play size that reaches another mode of the collection picked by width
/// names it; the drawn width's own mode names nothing.
///
/// flip: `Player::width_pick` dropping its comparison with the drawn width's
/// mode → fails at "as drawn, nothing to say".
#[test]
fn a_play_width_names_the_mode_it_picks() {
    let mut fx = Fx::new();
    let a = fx.frame("A", 0.0, 200.0, 100.0);
    let wide = fx.ids.mint_mode();
    fx.apply(vec![Operation::AddMode {
        collection: CollectionId::FIRST,
        mode: Mode {
            id: wide,
            name: "Wide".into(),
        },
        index: 1,
        cells: vec![],
    }]);
    let by_width =
        mastik_core::variable::by_width_ops(&fx.doc, CollectionId::FIRST, true).expect("by width");
    fx.apply(by_width);
    let mut p = fx.play(a);
    assert_eq!(p.width_pick(), None, "as drawn, nothing to say");
    p.set_play_size(Some(Size::new(800.0, 100.0)));
    assert_eq!(
        p.width_pick(),
        Some(("Variables".to_string(), "Wide".to_string())),
        "800 reaches Wide at 768"
    );
    p.set_play_size(Some(Size::new(300.0, 100.0)));
    assert_eq!(p.width_pick(), None, "the drawn width's mode again");
}

/// A click finds a layer where its scroll effect has moved and shrunk it, not
/// where it is drawn.
///
/// flip: `Player::walk` not taking the point back through the effect → fails
/// at "missed where it is drawn".
#[test]
fn a_click_follows_a_scroll_effect() {
    use mastik_core::prototype::{EffectEnds, ScrollEffect, ScrollProp};
    let (mut fx, a, card, n) = one_card();
    fx.play_settings(
        a,
        LayerPlay {
            scroll: ScrollAxis::Vertical,
            ..LayerPlay::default()
        },
    );
    fx.rect(a, "Long", 0.0, 0.0, 10.0, 500.0);
    let ends = |prop, start, end| EffectEnds { prop, start, end };
    fx.play_settings(
        card,
        LayerPlay {
            pin: Pin::Fixed,
            scroll_effect: Some(ScrollEffect {
                frame: None,
                from: 0.0,
                to: 100.0,
                props: vec![
                    ends(ScrollProp::MoveX, 0.0, 100.0),
                    ends(ScrollProp::Scale, 1.0, 0.5),
                ],
            }),
            ..LayerPlay::default()
        },
    );
    fx.on(card, Trigger::Click, vec![add(n, 1.0)]);
    let mut p = fx.play(a);
    p.input(Input::Wheel {
        at: Point::new(100.0, 50.0),
        delta: Vec2::new(0.0, 100.0),
    });
    // Drawn at 10..60 × 10..40; moved 100 right and halved about its centre:
    // 122.5..147.5 × 17.5..32.5.
    click(&mut p, 20.0, 25.0);
    assert_eq!(number(&p, n), 0.0, "missed where it is drawn");
    click(&mut p, 115.0, 25.0);
    assert_eq!(number(&p, n), 0.0, "missed where it would be unshrunk");
    click(&mut p, 135.0, 25.0);
    assert_eq!(number(&p, n), 1.0, "hit where it moved to");
}

// ── Lists ───────────────────────────────────────────────────────────────────

/// An instance of a main `Row` (180×30) placed in `parent` at (10, 10).
fn row_instance(fx: &mut Fx, main: NodeId, parent: NodeId) -> NodeId {
    let (tx, made) = mastik_core::insert_subtrees(
        &fx.doc,
        &mut fx.ids,
        &[Placement {
            nodes: fx.doc.capture_subtree(main).unwrap(),
            parent,
            index: None,
        }],
        Default::default(),
    );
    fx.apply(tx.0);
    fx.apply(vec![Operation::SetTransform {
        id: made[0],
        transform: Affine::translate((10.0, 10.0)),
    }]);
    made[0]
}

/// A list made around `instance` over `source`, its rows made.
fn listed(fx: &mut Fx, instance: NodeId, source: mastik_core::list::Source) -> NodeId {
    let res = mastik_core::Resolved::rebuild(&fx.doc);
    let (tx, l) = mastik_core::list::make(&fx.doc, &res, &mut fx.ids, instance, source).unwrap();
    fx.apply(tx.0);
    mastik_core::list::regenerate_outside_history(&mut fx.doc, &[l]);
    l
}

/// A tap on a list's second row opens it before the row's navigation runs, so
/// the detail screen's *Opened row* list shows row 2, and a live edit keeps it.
/// flip: `fire` not calling `open_row` → fails at "row 2 opened"; `apply_edit`
/// not calling `reopen_rows` → fails at "kept through the editor's regeneration".
#[test]
fn a_tapped_row_is_opened_on_the_detail_screen() {
    use mastik_core::chart::{Column, ColumnId, Table};
    use mastik_core::list::{DataSet, List, Rows, Source};
    let mut fx = Fx::new();
    let a = fx.frame("A", 0.0, 200.0, 200.0);
    let b = fx.frame("B", 300.0, 200.0, 200.0);
    let main = fx.frame("Row", 600.0, 180.0, 30.0);
    fx.apply(vec![Operation::SetComponent {
        id: main,
        component: true,
    }]);
    let item = row_instance(&mut fx, main, a);
    fx.on(
        item,
        Trigger::Click,
        vec![Action::Navigate {
            to: b,
            transition: Transition::default(),
            keep_scroll: false,
        }],
    );
    let set = DataSet {
        id: fx.ids.mint_data_set(),
        name: "rows".into(),
        table: Table {
            columns: vec![Column {
                id: ColumnId(0),
                name: "title".into(),
            }],
            rows: vec![vec!["A".into()], vec!["B".into()], vec!["C".into()]],
        },
        source: None,
        pictures: Vec::new(),
    };
    let data = set.id;
    listed(&mut fx, item, Source::New(set));
    let detail = row_instance(&mut fx, main, b);
    let shown = listed(&mut fx, detail, Source::Existing(data));
    let opened = List {
        rows: Rows::Opened,
        ..fx.doc.get(shown).unwrap().list().unwrap().clone()
    };
    fx.apply(vec![Operation::SetList {
        id: shown,
        list: Some(Box::new(opened)),
    }]);
    mastik_core::list::regenerate_outside_history(&mut fx.doc, &[shown]);

    let mut p = fx.play(a);
    // Rows at y 10, 48 and 86, 30 high, in a list frame at (10, 10).
    click(&mut p, 30.0, 60.0);
    assert_eq!(p.screen(), b, "the row navigates");
    let l = p.document().get(shown).unwrap().list().unwrap();
    assert_eq!(l.opened, 1, "row 2 opened");
    assert_eq!(
        fx.doc.get(shown).unwrap().list().unwrap().opened,
        0,
        "the file untouched"
    );

    // A live edit that writes the list as the editor holds it (its own
    // regeneration), then one the copy refuses and re-clones over, keeps row 2.
    let as_saved = fx.doc.get(shown).unwrap().list().unwrap().clone();
    let tx = Transaction(vec![Operation::SetList {
        id: shown,
        list: Some(Box::new(as_saved)),
    }]);
    fx.doc.apply(&tx).unwrap();
    p.apply_edit(&tx).unwrap();
    let l = p.document().get(shown).unwrap().list().unwrap();
    assert_eq!(l.opened, 1, "kept through the editor's regeneration");
    p.rebase(&fx.doc);
    let l = p.document().get(shown).unwrap().list().unwrap();
    assert_eq!(l.opened, 1, "kept through a re-clone");
}

/// The state card lists the data set an *Opened row* list reads, and a row
/// picked there opens it as a tap would.
/// flip: `Input::OpenRow` not calling `open_data_row` → fails at "picked".
#[test]
fn a_row_picked_in_the_state_card_opens() {
    use mastik_core::chart::{Column, ColumnId, Table};
    use mastik_core::list::{DataSet, List, Rows, Source};
    let mut fx = Fx::new();
    let a = fx.frame("A", 0.0, 200.0, 200.0);
    let main = fx.frame("Row", 600.0, 180.0, 30.0);
    fx.apply(vec![Operation::SetComponent {
        id: main,
        component: true,
    }]);
    let item = row_instance(&mut fx, main, a);
    let set = DataSet {
        id: fx.ids.mint_data_set(),
        name: "rows".into(),
        table: Table {
            columns: vec![Column {
                id: ColumnId(0),
                name: "title".into(),
            }],
            rows: vec![vec!["A".into()], vec!["B".into()], vec!["C".into()]],
        },
        source: None,
        pictures: Vec::new(),
    };
    let data = set.id;
    let l = listed(&mut fx, item, Source::New(set));
    let opened = List {
        rows: Rows::Opened,
        ..fx.doc.get(l).unwrap().list().unwrap().clone()
    };
    fx.apply(vec![Operation::SetList {
        id: l,
        list: Some(Box::new(opened)),
    }]);

    let mut p = fx.play(a);
    let rows = p.opened_rows();
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].rows, ["A", "B", "C"]);
    assert_eq!(rows[0].current, 0);
    p.input(Input::OpenRow { data, row: 2 });
    assert_eq!(p.opened_rows()[0].current, 2, "picked");
    assert_eq!(p.opened_rows()[0].open, "C", "the open row's label");
}

/// A list filtered by a variable is drawn again on the copy when the variable
/// changes, and hides its template while no row passes.
/// flip: `changed` not calling `refilter` → fails at "narrowed".
#[test]
fn a_filter_reading_a_variable_narrows_the_list_in_play() {
    use mastik_core::chart::{Column, ColumnId, Table};
    use mastik_core::list::{DataSet, List, Source, Test, TestOp};
    use mastik_core::prototype::Expr;
    let mut fx = Fx::new();
    let a = fx.frame("A", 0.0, 200.0, 200.0);
    let main = fx.frame("Row", 600.0, 180.0, 30.0);
    fx.apply(vec![Operation::SetComponent {
        id: main,
        component: true,
    }]);
    let item = row_instance(&mut fx, main, a);
    let category = fx.var("Category", VarKind::String, Cell::String(String::new()));
    let set = DataSet {
        id: fx.ids.mint_data_set(),
        name: "products".into(),
        table: Table {
            columns: vec![Column {
                id: ColumnId(0),
                name: "kind".into(),
            }],
            rows: vec![
                vec!["Shirts".into()],
                vec!["Knits".into()],
                vec!["Shirts".into()],
            ],
        },
        source: None,
        pictures: Vec::new(),
    };
    let l = listed(&mut fx, item, Source::New(set));
    let mut filtered: List = fx.doc.get(l).unwrap().list().unwrap().clone();
    filtered.filter.tests.push(Test {
        column: ColumnId(0),
        op: TestOp::Is,
        value: Expr(format!("{{{}}}", category.0.to_wire())),
    });
    fx.apply(vec![Operation::SetList {
        id: l,
        list: Some(Box::new(filtered)),
    }]);
    mastik_core::list::regenerate_outside_history(&mut fx.doc, &[l]);
    let copies = |p: &crate::Player| p.document().get(l).unwrap().list().unwrap().copies.len();

    let mut p = fx.play(a);
    assert_eq!(copies(&p), 2, "an empty value passes every row");
    p.set_value(category, Value::String("Knits".into()))
        .unwrap();
    assert_eq!(copies(&p), 0, "narrowed to one row");
    assert!(p.document().get(item).unwrap().visible());
    p.set_value(category, Value::String("Hats".into())).unwrap();
    assert!(
        !p.document().get(item).unwrap().visible(),
        "no row passes: hidden in play"
    );
    assert_eq!(
        fx.doc.get(l).unwrap().list().unwrap().copies.len(),
        2,
        "the file untouched"
    );
}

/// *Set mode* on every page gives them their modes in one commit and makes a
/// filtered list again once, not once per page.
///
/// flip: `Player::set_mode_on` putting each node's modes apart → fails at "made
/// again once".
#[test]
fn set_mode_everywhere_remakes_a_filtered_list_once() {
    use mastik_core::chart::{Column, ColumnId, Table};
    use mastik_core::list::{DataSet, List, Source, Test, TestOp};
    let mut fx = Fx::new();
    let a = fx.frame("A", 0.0, 200.0, 200.0);
    let kind = fx.var("kind", VarKind::String, Cell::String(String::new()));
    let (_, dark, _) = dark_mode(&mut fx);
    for (i, name) in ["Two", "Three"].into_iter().enumerate() {
        let id = fx.ids.mint();
        fx.apply(vec![Operation::AddPage {
            id,
            index: i + 1,
            name: name.into(),
            ground: None,
            draft: false,
            modes: Vec::new(),
            flows: Vec::new(),
            play: Default::default(),
        }]);
    }
    let main = fx.frame("Row", 600.0, 180.0, 30.0);
    fx.apply(vec![Operation::SetComponent {
        id: main,
        component: true,
    }]);
    let item = row_instance(&mut fx, main, a);
    let set = DataSet {
        id: fx.ids.mint_data_set(),
        name: "products".into(),
        table: Table {
            columns: vec![Column {
                id: ColumnId(0),
                name: "kind".into(),
            }],
            rows: vec![vec!["".into()], vec!["".into()], vec!["".into()]],
        },
        source: None,
        pictures: Vec::new(),
    };
    let l = listed(&mut fx, item, Source::New(set));
    let mut filtered: List = fx.doc.get(l).unwrap().list().unwrap().clone();
    filtered.filter.tests.push(Test {
        column: ColumnId(0),
        op: TestOp::Is,
        value: Expr(format!("{{{}}}", kind.0.to_wire())),
    });
    fx.apply(vec![Operation::SetList {
        id: l,
        list: Some(Box::new(filtered)),
    }]);
    let mut p = fx.play(a);
    let copies = p.document().get(l).unwrap().list().unwrap().copies.len();
    assert_eq!(copies, 2, "fixture: three rows pass");
    let before = p.ids.mint().seq;
    p.input(Input::SetMode(dark));
    let minted = p.ids.mint().seq - before - 1;
    assert_eq!(p.modes()[0].current, dark, "fixture: set");
    assert_eq!(minted, copies as u64, "made again once");
}

/// A list over an empty table hides its template in play while the document
/// keeps it, and a live edit giving the table rows shows it again.
/// flip: `prepare` not calling `hide_empty_templates` → fails at "hidden in play".
#[test]
fn an_empty_list_draws_no_row_in_play() {
    use mastik_core::chart::{Column, ColumnId, Table};
    use mastik_core::list::{DataSet, Source};
    let mut fx = Fx::new();
    let a = fx.frame("A", 0.0, 200.0, 200.0);
    let main = fx.frame("Row", 600.0, 180.0, 30.0);
    fx.apply(vec![Operation::SetComponent {
        id: main,
        component: true,
    }]);
    let item = row_instance(&mut fx, main, a);
    let mut set = DataSet {
        id: fx.ids.mint_data_set(),
        name: "cart".into(),
        table: Table {
            columns: vec![Column {
                id: ColumnId(0),
                name: "title".into(),
            }],
            rows: Vec::new(),
        },
        source: None,
        pictures: Vec::new(),
    };
    listed(&mut fx, item, Source::New(set.clone()));
    assert!(fx.doc.get(item).unwrap().visible(), "kept on the canvas");

    let mut p = fx.play(a);
    assert!(!p.document().get(item).unwrap().visible(), "hidden in play");

    set.table.rows.push(vec!["A".into()]);
    p.apply_edit(&Transaction(vec![Operation::SetDataSet {
        id: set.id,
        set: Some(Box::new(set)),
    }]))
    .unwrap();
    assert!(
        p.document().get(item).unwrap().visible(),
        "shown once the table has rows"
    );
}

// ── Row actions ─────────────────────────────────────────────────────────────

/// `{<read>}` for a table read, as an expression stores it.
fn read(r: mastik_core::prototype::expr::Ref) -> Expr {
    Expr(format!("{{{}}}", r.wire()))
}

/// A frame A (200×400) with a list at (10, 10) over a table `Cart` (Name, Qty)
/// holding `rows`, its template an instance of a 180×30 main.
fn cart(rows: &[[&str; 2]]) -> (Fx, NodeId, NodeId, mastik_core::list::DataSetId) {
    use mastik_core::chart::{Column, ColumnId, Table};
    use mastik_core::list::{DataSet, Source};
    let mut fx = Fx::new();
    let a = fx.frame("A", 0.0, 200.0, 400.0);
    let main = fx.frame("Row", 600.0, 180.0, 30.0);
    fx.apply(vec![Operation::SetComponent {
        id: main,
        component: true,
    }]);
    let item = row_instance(&mut fx, main, a);
    let set = DataSet {
        id: fx.ids.mint_data_set(),
        name: "Cart".into(),
        table: Table {
            columns: vec![
                Column {
                    id: ColumnId(0),
                    name: "Name".into(),
                },
                Column {
                    id: ColumnId(1),
                    name: "Qty".into(),
                },
            ],
            rows: rows
                .iter()
                .map(|r| r.iter().map(|c| (*c).to_string()).collect())
                .collect(),
        },
        source: None,
        pictures: Vec::new(),
    };
    let data = set.id;
    listed(&mut fx, item, Source::New(set));
    (fx, a, item, data)
}

/// A cart with no script: *Add row* fills an empty table and its list, a
/// row's *Edit row* reads the row it is on, and *When a table changes* works a
/// total out again. The document's table stays empty, and the state card's
/// *Reset* puts the played one back.
///
/// flip: `write_table` not calling `table_changed` → fails at "the total
/// follows"; `Player::pick` reading `opened` for *This row* → fails at "row 2
/// edited".
#[test]
fn a_cart_adds_edits_and_totals_rows_in_play() {
    use mastik_core::chart::ColumnId;
    use mastik_core::prototype::expr::{Agg, Ref};
    use mastik_core::prototype::{CellWrite, RowEnd, WhichRow};
    let (mut fx, a, item, cart) = cart(&[]);
    let total = fx.var("Total", VarKind::Number, Cell::Number(0.0));
    let add = fx.rect(a, "Add", 150.0, 350.0, 40.0, 40.0);
    fx.on(
        add,
        Trigger::Click,
        vec![Action::AddRow {
            table: cart,
            at: RowEnd::End,
            cells: vec![
                CellWrite {
                    column: ColumnId(0),
                    to: Expr("'Shirt'".into()),
                },
                CellWrite {
                    column: ColumnId(1),
                    to: Expr("1".into()),
                },
            ],
        }],
    );
    fx.on(
        item,
        Trigger::Click,
        vec![Action::EditRow {
            table: cart,
            row: WhichRow::This,
            cells: vec![CellWrite {
                column: ColumnId(1),
                to: Expr(format!("{} + 1", read(Ref::Row(ColumnId(1))).0)),
            }],
        }],
    );
    fx.on(
        a,
        Trigger::TableChanges { table: cart },
        vec![Action::SetVariable {
            var: total,
            change: VarChange::Set {
                to: read(Ref::Total(cart, ColumnId(1), Agg::Sum)),
            },
        }],
    );
    let l = fx.doc.get(item).unwrap().parent().unwrap();
    let mut p = fx.play(a);
    let drawn = |p: &Player| {
        let n = p.document().get(l).unwrap();
        n.children()
            .iter()
            .filter(|c| p.document().get(**c).unwrap().visible())
            .count()
    };
    assert_eq!(drawn(&p), 0, "an empty cart draws nothing");

    click(&mut p, 160.0, 360.0);
    click(&mut p, 160.0, 360.0);
    assert!(p.take_notices().is_empty());
    let rows = |p: &Player| p.document().data_set(cart).unwrap().table.rows.clone();
    assert_eq!(rows(&p).len(), 2, "two rows added");
    assert_eq!(drawn(&p), 2, "the list follows");
    assert_eq!(number(&p, total), 2.0, "the total follows");

    // Rows at y 10 and 48: the second row's click edits the second row.
    click(&mut p, 30.0, 60.0);
    assert_eq!(
        rows(&p)[1],
        vec!["Shirt".to_string(), "2".into()],
        "row 2 edited"
    );
    assert_eq!(rows(&p)[0][1], "1");
    assert_eq!(number(&p, total), 3.0);
    assert!(
        fx.doc.data_set(cart).unwrap().table.rows.is_empty(),
        "the file untouched"
    );

    let card = p.tables();
    assert_eq!(card.len(), 1);
    assert_eq!((card[0].rows, card[0].in_file), (2, 0));
    p.input(Input::ResetTable(cart));
    assert!(rows(&p).is_empty(), "put back");
    assert_eq!(drawn(&p), 0);
    assert_eq!(number(&p, total), 0.0, "a reset is a change too");
    assert!(p.tables().is_empty());
}

/// A row action copying a cell writes it as written, a bare read in
/// parentheses too, and *Edit row* writing a row's cell back leaves it be; a
/// cell worked out from one is written as it shows.
///
/// flip: `Player::cell_text` writing every value through `format_value` →
/// fails at "copied as written".
#[test]
fn a_copied_cell_keeps_its_spelling() {
    use mastik_core::chart::ColumnId;
    use mastik_core::prototype::expr::{Ref, RowAt};
    use mastik_core::prototype::{CellWrite, RowEnd, WhichRow};
    let (mut fx, a, item, cart) = cart(&[["12.50", "007"]]);
    let first = |c| read(Ref::Cell(cart, RowAt::First, ColumnId(c)));
    let add = fx.rect(a, "Add", 150.0, 350.0, 40.0, 40.0);
    fx.on(
        add,
        Trigger::Click,
        vec![Action::AddRow {
            table: cart,
            at: RowEnd::End,
            cells: vec![
                CellWrite {
                    column: ColumnId(0),
                    to: first(0),
                },
                CellWrite {
                    column: ColumnId(1),
                    to: Expr(format!("({})", first(1).0)),
                },
            ],
        }],
    );
    fx.on(
        item,
        Trigger::Click,
        vec![Action::EditRow {
            table: cart,
            row: WhichRow::This,
            cells: vec![
                CellWrite {
                    column: ColumnId(0),
                    to: read(Ref::Row(ColumnId(0))),
                },
                CellWrite {
                    column: ColumnId(1),
                    to: Expr(format!("{} + 0", read(Ref::Row(ColumnId(1))).0)),
                },
            ],
        }],
    );
    let mut p = fx.play(a);
    let rows = |p: &Player| p.document().data_set(cart).unwrap().table.rows.clone();
    click(&mut p, 160.0, 360.0);
    assert!(p.take_notices().is_empty());
    assert_eq!(
        rows(&p)[1],
        vec!["12.50".to_string(), "007".into()],
        "copied as written"
    );
    click(&mut p, 30.0, 20.0);
    assert_eq!(
        rows(&p)[0],
        vec!["12.50".to_string(), "7".into()],
        "written back as it was; worked out as it shows"
    );
}

/// A row's reaction goes on after its first write makes the list again: a
/// second *Edit row* on *This row* writes the same row, and a second reaction
/// on the layer still runs.
///
/// flip: `run_actions` not looking its holder up again (`Player::still`) →
/// fails at "nothing skipped"; `fire` not doing so → fails at "the second
/// reaction runs".
#[test]
fn a_row_reaction_goes_on_after_its_first_write() {
    use mastik_core::chart::ColumnId;
    use mastik_core::prototype::expr::Ref;
    use mastik_core::prototype::{CellWrite, WhichRow};
    let (mut fx, a, item, cart) = cart(&[["Shirt", "1"], ["Hat", "1"]]);
    let n = fx.var("n", VarKind::Number, Cell::Number(0.0));
    fx.on(
        item,
        Trigger::Click,
        vec![
            Action::EditRow {
                table: cart,
                row: WhichRow::This,
                cells: vec![CellWrite {
                    column: ColumnId(1),
                    to: Expr(format!("{} + 1", read(Ref::Row(ColumnId(1))).0)),
                }],
            },
            Action::EditRow {
                table: cart,
                row: WhichRow::This,
                cells: vec![CellWrite {
                    column: ColumnId(0),
                    to: Expr("'Done'".into()),
                }],
            },
        ],
    );
    fx.on(item, Trigger::Click, vec![add(n, 1.0)]);
    let l = fx.doc.get(item).unwrap().parent().unwrap();
    mastik_core::list::regenerate_outside_history(&mut fx.doc, &[l]);
    let mut p = fx.play(a);
    let rows = |p: &Player| p.document().data_set(cart).unwrap().table.rows.clone();
    click(&mut p, 30.0, 60.0);
    assert_eq!(p.take_notices(), vec![], "nothing skipped");
    assert_eq!(
        rows(&p)[1],
        vec!["Done".to_string(), "2".into()],
        "both cells written"
    );
    assert_eq!(rows(&p)[0], vec!["Shirt".to_string(), "1".into()]);
    assert_eq!(number(&p, n), 1.0, "the second reaction runs");
}

/// A row reaction whose first write filters its row out of the list skips the
/// rest as `Missing`, never writing the row that now sits at its drawn place.
///
/// flip: `Player::still` finding the row by its drawn place, not its table row
/// → fails at "the next row untouched".
#[test]
fn a_row_filtered_out_by_its_write_skips_the_rest() {
    use mastik_core::chart::ColumnId;
    use mastik_core::list::{Filter, List, Test, TestOp};
    use mastik_core::prototype::{CellWrite, WhichRow};
    let (mut fx, a, item, cart) = cart(&[["A", "1"], ["B", "1"], ["C", "1"]]);
    let l = fx.doc.get(item).unwrap().parent().unwrap();
    let filtered = List {
        filter: Filter {
            any: false,
            tests: vec![Test {
                column: ColumnId(1),
                op: TestOp::Is,
                value: Expr("1".into()),
            }],
        },
        ..fx.doc.get(l).unwrap().list().unwrap().clone()
    };
    fx.apply(vec![Operation::SetList {
        id: l,
        list: Some(Box::new(filtered)),
    }]);
    let write = |column, to: &str| Action::EditRow {
        table: cart,
        row: WhichRow::This,
        cells: vec![CellWrite {
            column: ColumnId(column),
            to: Expr(to.into()),
        }],
    };
    fx.on(
        item,
        Trigger::Click,
        vec![write(1, "'0'"), write(0, "'Done'")],
    );
    mastik_core::list::regenerate_outside_history(&mut fx.doc, &[l]);
    let mut p = fx.play(a);
    let rows = |p: &Player| p.document().data_set(cart).unwrap().table.rows.clone();
    click(&mut p, 30.0, 60.0);
    assert_eq!(rows(&p)[1], vec!["B".to_string(), "0".into()], "fixture");
    assert_eq!(
        rows(&p)[2],
        vec!["C".to_string(), "1".into()],
        "the next row untouched"
    );
    assert!(
        matches!(p.take_notices().as_slice(), [Notice::Missing { .. }]),
        "the rest skipped"
    );
}

/// *Set variable* of a text variable from one cell that reads as a number
/// takes the cell as written, leading zeros and all.
///
/// flip: the `VarChange::Set` arm not taking `as_written` → fails at "set as
/// written" (refused as a number).
#[test]
fn a_text_variable_takes_a_numeric_cell_as_written() {
    use mastik_core::chart::ColumnId;
    use mastik_core::prototype::expr::{Ref, RowAt};
    let (mut fx, a, item, cart) = cart(&[["00731", "1"]]);
    let code = fx.var("Code", VarKind::String, Cell::String(String::new()));
    fx.on(
        item,
        Trigger::Click,
        vec![Action::SetVariable {
            var: code,
            change: VarChange::Set {
                to: read(Ref::Cell(cart, RowAt::First, ColumnId(0))),
            },
        }],
    );
    let mut p = fx.play(a);
    click(&mut p, 30.0, 20.0);
    assert_eq!(
        p.doc.resolve_var(p.screen(), code),
        Some(Value::String("00731".into())),
        "set as written"
    );
    assert_eq!(p.take_notices(), vec![]);
}

/// A list made again in play starts its new rows' *After* reactions, so a row
/// whose state loops goes on looping after a write regenerates the list.
///
/// flip: `regenerate_list` not calling `take_up_copies` → fails at "the new
/// row loops".
#[test]
fn a_regenerated_row_keeps_looping() {
    use mastik_core::chart::{Column, ColumnId, Table};
    use mastik_core::list::{DataSet, Source};
    use mastik_core::prototype::{CellWrite, RowEnd};
    let mut fx = Fx::new();
    let a = fx.frame("A", 0.0, 200.0, 400.0);
    let set = fx.frame("Row", 600.0, 400.0, 30.0);
    let one = fx.node(set, "One", frame_kind(180.0, 30.0), (0.0, 0.0));
    let two = fx.node(set, "Two", frame_kind(180.0, 30.0), (200.0, 0.0));
    let mut ops: Vec<Operation> = [one, two]
        .iter()
        .map(|id| Operation::SetComponent {
            id: *id,
            component: true,
        })
        .collect();
    ops.push(Operation::SetVariantSet {
        id: set,
        set: Some(VariantSet {
            props: vec![VariantProp::new("Look", vec!["One".into(), "Two".into()])],
        }),
    });
    for (id, v) in [(one, "One"), (two, "Two")] {
        ops.push(Operation::SetVariant {
            id,
            values: vec![v.into()],
        });
    }
    fx.apply(ops);
    for (from, to) in [(one, two), (two, one)] {
        fx.on(
            from,
            Trigger::After { ms: 100 },
            vec![Action::ChangeState {
                instance: from,
                to,
                transition: Transition::INSTANT,
            }],
        );
    }
    let item = row_instance(&mut fx, one, a);
    let data = DataSet {
        id: fx.ids.mint_data_set(),
        name: "Cards".into(),
        table: Table {
            columns: vec![Column {
                id: ColumnId(0),
                name: "Name".into(),
            }],
            rows: vec![vec!["1".into()], vec!["2".into()]],
        },
        source: None,
        pictures: Vec::new(),
    };
    let cards = data.id;
    let l = listed(&mut fx, item, Source::New(data));
    let more = fx.rect(a, "More", 150.0, 350.0, 40.0, 40.0);
    fx.on(
        more,
        Trigger::Click,
        vec![Action::AddRow {
            table: cards,
            at: RowEnd::End,
            cells: vec![CellWrite {
                column: ColumnId(0),
                to: Expr("'3'".into()),
            }],
        }],
    );
    let mut p = fx.play(a);
    let second = |p: &Player| {
        let copy = p.document().get(l).unwrap().list().unwrap().copies[0];
        p.document().get(copy).unwrap().link()
    };
    p.tick(100.0);
    assert_eq!(second(&p), Some(two), "fixture: the row loops");
    click(&mut p, 160.0, 360.0);
    assert_eq!(
        p.document().get(l).unwrap().list().unwrap().copies.len(),
        2,
        "fixture: the list made again"
    );
    assert_eq!(second(&p), Some(two), "a new row as the template shows");
    p.tick(100.0);
    assert_eq!(second(&p), Some(one), "the new row loops");
}

/// A row's *After 0* that writes its own table makes the list again each time
/// it fires; the new rows' timers wait a frame, so one tick fires a handful,
/// not the most a tick fires.
///
/// flip: `take_up_copies` arming at `now + ms` with no floor → fails at "a
/// handful a tick".
#[test]
fn a_row_after_writing_its_table_lets_time_move() {
    use mastik_core::chart::ColumnId;
    use mastik_core::prototype::expr::Ref;
    use mastik_core::prototype::{CellWrite, WhichRow};
    let (mut fx, a, item, cart) = cart(&[["A", "0"], ["B", "0"], ["C", "0"]]);
    fx.on(
        item,
        Trigger::After { ms: 0 },
        vec![Action::EditRow {
            table: cart,
            row: WhichRow::This,
            cells: vec![CellWrite {
                column: ColumnId(1),
                to: Expr(format!("{} + 1", read(Ref::Row(ColumnId(1))).0)),
            }],
        }],
    );
    let l = fx.doc.get(item).unwrap().parent().unwrap();
    mastik_core::list::regenerate_outside_history(&mut fx.doc, &[l]);
    let mut p = fx.play(a);
    let fired = |p: &Player| -> f64 {
        p.document()
            .data_set(cart)
            .unwrap()
            .table
            .rows
            .iter()
            .map(|r| r[1].parse::<f64>().unwrap())
            .sum()
    };
    p.tick(16.0);
    let once = fired(&p);
    assert!(once >= 1.0, "fixture: the rows fire");
    assert!(once <= 4.0, "a handful a tick: {once}");
}

/// *First row where…* finds its row by `{row.…}`; a row past the end skips the
/// action with a notice; *This row* outside a list over the table is missing;
/// a full table refuses *Add row* once, by name.
///
/// flip: `TableEnv::read` answering an empty cell for a row past the end →
/// fails at "a row past the end".
#[test]
fn row_actions_pick_their_row_and_say_why_they_skip() {
    use mastik_core::chart::ColumnId;
    use mastik_core::prototype::expr::{Ref, RowAt};
    use mastik_core::prototype::{CellWrite, RowEnd, WhichRow};
    let (mut fx, a, _, cart) = cart(&[["A", "1"], ["B", "1"], ["C", "1"]]);
    let button = |fx: &mut Fx, y: f64, action: Action| {
        let b = fx.rect(a, "B", 150.0, y, 40.0, 30.0);
        fx.on(b, Trigger::Click, vec![action]);
    };
    button(
        &mut fx,
        200.0,
        Action::DeleteRow {
            table: cart,
            row: WhichRow::FirstWhere {
                when: Expr(format!("{} == 'B'", read(Ref::Row(ColumnId(0))).0)),
            },
        },
    );
    button(
        &mut fx,
        250.0,
        Action::EditRow {
            table: cart,
            row: WhichRow::Opened,
            cells: vec![CellWrite {
                column: ColumnId(0),
                to: read(Ref::Cell(cart, RowAt::Nth(9), ColumnId(0))),
            }],
        },
    );
    button(
        &mut fx,
        300.0,
        Action::DeleteRow {
            table: cart,
            row: WhichRow::This,
        },
    );
    let mut p = fx.play(a);
    let names = |p: &Player| -> Vec<String> {
        let t = &p.document().data_set(cart).unwrap().table;
        t.rows.iter().map(|r| r[0].clone()).collect()
    };
    click(&mut p, 160.0, 210.0);
    assert_eq!(names(&p), ["A", "C"], "B deleted");
    click(&mut p, 160.0, 210.0);
    assert_eq!(names(&p), ["A", "C"], "no B left: skipped, nothing said");
    assert!(p.take_notices().is_empty());

    click(&mut p, 160.0, 260.0);
    let notices = p.take_notices();
    assert!(
        matches!(&notices[..], [Notice::Expression { error, .. }]
            if error.to_string() == "Cart has 2 rows, not 9"),
        "a row past the end: {notices:?}"
    );
    assert_eq!(names(&p), ["A", "C"]);

    click(&mut p, 160.0, 310.0);
    assert!(matches!(&p.take_notices()[..], [Notice::Missing { .. }]));

    // A full table: the most rows a table holds.
    let (mut fx, a, _, set) = self::cart(&[]);
    let mut full = fx.doc.data_set(set).unwrap().clone();
    full.table.rows = vec![vec![String::new(), String::new()]; mastik_core::chart::MAX_ROWS];
    fx.apply(vec![Operation::SetDataSet {
        id: set,
        set: Some(Box::new(full)),
    }]);
    let b = fx.rect(a, "Add", 150.0, 350.0, 40.0, 40.0);
    fx.on(
        b,
        Trigger::Click,
        vec![Action::AddRow {
            table: set,
            at: RowEnd::Start,
            cells: vec![],
        }],
    );
    let mut p = fx.play(a);
    click(&mut p, 160.0, 360.0);
    click(&mut p, 160.0, 360.0);
    assert!(
        matches!(&p.take_notices()[..], [Notice::TableFull { table, .. }] if table == "Cart"),
        "said once, by name"
    );
}

/// A live edit that leaves a played table's document rows alone keeps the
/// played rows, through a re-clone too; one that changes them gives the
/// document's back.
///
/// flip: `rebase` not calling `carry_tables` → fails at "kept through a
/// re-clone"; `Player::pick` reading *Opened row* as the last row → fails at
/// "the opened row, row 1, deleted".
#[test]
fn played_rows_survive_a_live_edit_that_leaves_their_table() {
    use mastik_core::prototype::{RowEnd, WhichRow};
    let (mut fx, a, _, cart) = cart(&[["A", "1"]]);
    let b = fx.rect(a, "Add", 150.0, 350.0, 40.0, 40.0);
    fx.on(
        b,
        Trigger::Click,
        vec![Action::AddRow {
            table: cart,
            at: RowEnd::End,
            cells: vec![],
        }],
    );
    let del = fx.rect(a, "Del", 150.0, 300.0, 40.0, 40.0);
    fx.on(
        del,
        Trigger::Click,
        vec![Action::DeleteRow {
            table: cart,
            row: WhichRow::Opened,
        }],
    );
    let mut p = fx.play(a);
    let len = |p: &Player| p.document().data_set(cart).unwrap().table.rows.len();
    click(&mut p, 160.0, 360.0);
    assert_eq!(len(&p), 2);
    let tx = Transaction(vec![Operation::SetName {
        id: b,
        name: "Add to cart".into(),
    }]);
    fx.doc.apply(&tx).unwrap();
    p.apply_edit(&tx).unwrap();
    assert_eq!(len(&p), 2, "kept through an edit elsewhere");
    p.rebase(&fx.doc);
    assert_eq!(len(&p), 2, "kept through a re-clone");

    let mut set = fx.doc.data_set(cart).unwrap().clone();
    set.table.rows.push(vec!["Z".into(), "9".into()]);
    set.table.rows.push(vec!["Y".into(), "9".into()]);
    let tx = Transaction(vec![Operation::SetDataSet {
        id: cart,
        set: Some(Box::new(set)),
    }]);
    fx.doc.apply(&tx).unwrap();
    p.apply_edit(&tx).unwrap();
    assert_eq!(len(&p), 3, "the document's rows again");
    assert!(p.tables().is_empty());
    let names = |p: &Player| -> Vec<String> {
        let t = &p.document().data_set(cart).unwrap().table;
        t.rows.iter().map(|r| r[0].clone()).collect()
    };
    click(&mut p, 160.0, 310.0);
    assert_eq!(names(&p), ["Z", "Y"], "the opened row, row 1, deleted");
    p.input(Input::OpenRow { data: cart, row: 1 });
    click(&mut p, 160.0, 310.0);
    assert_eq!(names(&p), ["Z"], "the row opened, row 2, deleted");
}
