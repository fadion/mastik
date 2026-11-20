//! End-to-end play scenarios over small documents built here.

use crate::{Input, Notice, Player, Start, Take};
use mastik_core::kurbo::{Affine, Point, RoundedRectRadii, Size, Vec2};
use mastik_core::peniko::Color;
use mastik_core::prototype::{
    Action, Branch, Direction, Easing, Expr, Flow, Input as Bound, LayerPlay, OverlaySettings, Pin,
    Reaction, ScrollAxis, Timing, Transition, TransitionKind, Trigger, VarChange,
};
use mastik_core::variable::{CollectionId, TextVars};
use mastik_core::variant::{Interaction, VariantProp, VariantSet};
use mastik_core::{
    Cell, Document, IdSource, Keyed, Mode, ModeId, NodeId, NodeKind, Operation, Placement,
    TextSizing, TextStyle, Transaction, Value, VarId, VarKind, Variable,
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
        vec![Action::Navigate { to: b, transition }],
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

/// An `After` reaction runs at its time after its frame opens, and not once the
/// frame has closed.
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

/// Typing into an input text layer writes its variable, which the layer shows.
///
/// flip: `Player::prepare` not binding the layer's content → fails at "bound".
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
    assert!(p.input(Input::Key {
        key: "Escape".into(),
        shift: false
    }));
    assert_eq!(p.focused(), None);
}

// ── Overlays ────────────────────────────────────────────────────────────────

/// An overlay opens centred over its backdrop; *Back* closes it before it walks
/// the history; a click outside closes it.
///
/// flip: `run_action`'s `Back` popping the history even with an overlay open →
/// fails at "and leaves the screen".
#[test]
fn an_overlay_opens_closes_and_backs_out() {
    let mut fx = Fx::new();
    let a = fx.frame("A", 0.0, 200.0, 100.0);
    let b = fx.frame("B", 300.0, 200.0, 100.0);
    let menu = fx.frame("Menu", 600.0, 100.0, 50.0);
    let shade = Color::from_rgba8(0, 0, 0, 128);
    fx.play_settings(
        menu,
        LayerPlay {
            overlay: OverlaySettings {
                backdrop: Some(shade),
                ..OverlaySettings::default()
            },
            ..LayerPlay::default()
        },
    );
    let open = fx.rect(b, "Open", 0.0, 0.0, 40.0, 40.0);
    fx.on(
        open,
        Trigger::Click,
        vec![Action::OpenOverlay {
            overlay: menu,
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
    click(&mut p, 10.0, 10.0);
    // Outside the overlay: it closes, and the click goes no further.
    click(&mut p, 190.0, 90.0);
    assert!(p.overlays().is_empty(), "a click outside closes it");
    assert_eq!(p.screen(), b);
}

// ── Scrolling ───────────────────────────────────────────────────────────────

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
/// variant; a value set from the state card switches it too.
///
/// flip: `Player::sync_inputs` skipping the switch → fails at "shows checked".
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
