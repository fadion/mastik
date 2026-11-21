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
/// fails at "and leaves the screen".
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
/// screen alone, and the state card's row follows; the document is untouched
/// and a restart starts from the saved modes.
///
/// flip: `Player::set_mode_on` filtering by the wrong collection (keeping the
/// old mode beside the new) → fails at "reads the dark column".
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
    let card = fx.rect(a, "Card", 10.0, 10.0, 50.0, 30.0);
    fx.on(
        card,
        Trigger::Click,
        vec![Action::SetMode {
            mode: dark,
            scope: ModeScope::Everywhere,
        }],
    );
    let mut p = fx.play(a);
    assert_eq!(number(&p, n), 1.0);
    assert_eq!(p.modes()[0].current, light);
    click(&mut p, 20.0, 20.0);
    assert_eq!(number(&p, n), 2.0, "reads the dark column");
    assert_eq!(p.modes()[0].current, dark, "the card follows");
    assert!(!p.modes()[0].screen_sets, "set on the page, not the screen");
    assert_eq!(
        fx.doc.resolve_var(a, n),
        Some(Value::Number(1.0)),
        "the document is untouched"
    );
    p.input(Input::SetMode(light));
    assert_eq!(number(&p, n), 1.0, "a pick from the card");
    p.restart();
    assert_eq!(number(&p, n), 1.0, "a restart reads the saved modes");
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
/// the detail screen's *Opened row* list shows row 2.
/// flip: `fire` not calling `open_row` → fails at "row 2 opened".
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
