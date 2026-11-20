//! The inspector's Prototype tab (`docs/design/prototyping-authoring.md`): a
//! layer's interactions, its play settings, and with nothing selected the page's
//! flows and prototype settings. Every edit is one transaction on the saved model
//! (`mastik_core::prototype`); nothing here plays.

use super::inspector::{FRAME_PRESETS, identity_card, identity_glyph, interacting};
use crate::app::MastikApp;
use crate::preview::Drag;
use crate::theme::{self, color, icon};
use crate::ui::{self, FieldButton, Prefix, Scrub};
use eframe::egui;
use mastik_core::id::NodeId;
use mastik_core::item::{ItemId, Keyed};
use mastik_core::prototype::{
    Action, Branch, Direction, Easing, Expr, Flow, Input, LayerPlay, OverlayPosition,
    OverlaySettings, PagePlay, Pin, Reaction, Scaling, ScrollAxis, Timing, Transition,
    TransitionKind, Trigger, VarChange,
};
use mastik_core::variable::{Cell, VarId, VarKind};
use mastik_core::{Document, NodeKind, Operation, Transaction};

/// The Prototype tab's state between frames: its open popover, a typed field's
/// draft, the key a key trigger waits for, and a scrubbed number in flight.
#[derive(Default)]
pub(crate) struct ProtoState {
    /// The action whose popover is open.
    pub open: Option<ActionAt>,
    /// A typed expression, address or flow name, until it commits.
    pub draft: Option<ui::Draft<DraftKey, String>>,
    /// The reaction whose *Key…* trigger waits for the next key.
    pub capture: Option<(NodeId, ItemId)>,
    /// A number field being scrubbed or typed: the saved model is never previewed
    /// (its operations draw nothing), so the field shows the value held here.
    pub scrub: Option<(egui::Id, f64)>,
    /// Where in a refused expression's typed text the fault is, for its field's
    /// underline.
    pub expr_fault: Option<(DraftKey, std::ops::Range<usize>)>,
    /// The action being dragged by its grip.
    pub dragging: Option<ActionAt>,
    /// The unselected layer wearing link dots, noted as the canvas draws so a
    /// press next frame aims at the dots it saw (`note_link_hover`).
    pub knob_hover: std::cell::Cell<Option<NodeId>>,
    /// A manual overlay's X or Y held in its field, so the canvas ghost follows
    /// the scrub before it commits.
    pub held_at: Option<(ItemId, mastik_core::kurbo::Vec2)>,
    /// The placed overlay's ghost as the canvas drew it last frame, in screen
    /// space: what a press aims at, and what keeps a click on it from closing
    /// the popover.
    pub ghost_rect: std::cell::Cell<Option<egui::Rect>>,
    /// Each connector as the canvas drew it last frame, in screen space, with
    /// the action it draws: what a click on one aims at.
    pub links_drawn: std::cell::RefCell<Vec<(ActionAt, Vec<egui::Pos2>)>>,
    /// The connector under the pointer as the canvas drew it, whose layer the
    /// hover outline rings (`link_hover`).
    pub link_hot: std::cell::Cell<Option<ActionAt>>,
}

/// One action of one layer's reaction, by item ids, nested ones included.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct ActionAt {
    pub node: NodeId,
    pub reaction: ItemId,
    pub action: ItemId,
}

/// What a typed field of the tab is for.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) enum DraftKey {
    /// A conditional's branch condition, or *Set variable*'s value (branch 0).
    Expr(ItemId, usize),
    /// *Open link*'s address.
    Url(ItemId),
    /// A flow's name.
    FlowName(ItemId),
    /// An overlay action's backdrop colour.
    Backdrop(ItemId),
    /// A flow's starting value, by its place in the flow's list.
    FlowValue(ItemId, usize),
}

/// Where in a reaction a new action goes.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) enum Place {
    /// The reaction's own list.
    Top,
    /// A conditional's branch, by index.
    Branch(ItemId, usize),
    /// A conditional's *else*.
    Otherwise(ItemId),
}

/// The kinds of action the add menu offers, in its groups' order.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum ActionKind {
    Navigate,
    Back,
    ScrollTo,
    OpenUrl,
    OpenOverlay,
    SwapOverlay,
    CloseOverlay,
    ChangeState,
    SetVariable,
    Conditional,
    RunScript,
}

impl ActionKind {
    /// The add menu's groups: where the player is, what floats over it, and the
    /// state of the copy it plays.
    const GROUPS: [(&'static str, &'static [ActionKind]); 3] = [
        (
            "Go",
            &[
                ActionKind::Navigate,
                ActionKind::Back,
                ActionKind::ScrollTo,
                ActionKind::OpenUrl,
            ],
        ),
        (
            "Overlay",
            &[
                ActionKind::OpenOverlay,
                ActionKind::SwapOverlay,
                ActionKind::CloseOverlay,
            ],
        ),
        (
            "State",
            &[
                ActionKind::ChangeState,
                ActionKind::SetVariable,
                ActionKind::Conditional,
                ActionKind::RunScript,
            ],
        ),
    ];

    fn of(a: &Action) -> Self {
        match a {
            Action::Navigate { .. } => Self::Navigate,
            Action::Back => Self::Back,
            Action::OpenOverlay { .. } => Self::OpenOverlay,
            Action::CloseOverlay { .. } => Self::CloseOverlay,
            Action::SwapOverlay { .. } => Self::SwapOverlay,
            Action::ScrollTo { .. } => Self::ScrollTo,
            Action::ChangeState { .. } => Self::ChangeState,
            Action::SetVariable { .. } => Self::SetVariable,
            Action::Conditional { .. } => Self::Conditional,
            Action::OpenUrl { .. } => Self::OpenUrl,
            Action::RunScript { .. } => Self::RunScript,
        }
    }

    /// The menu's and the popover head's words.
    pub(crate) fn label(self) -> &'static str {
        match self {
            Self::Navigate => "Navigate to",
            Self::Back => "Back",
            Self::ScrollTo => "Scroll to",
            Self::OpenUrl => "Open link",
            Self::OpenOverlay => "Open overlay",
            Self::SwapOverlay => "Swap overlay",
            Self::CloseOverlay => "Close overlay",
            Self::ChangeState => "Change to",
            Self::SetVariable => "Set variable",
            Self::Conditional => "Conditional",
            Self::RunScript => "Run script",
        }
    }

    pub(crate) fn glyph(self) -> &'static str {
        match self {
            Self::Navigate => icon::ARROW_RIGHT,
            Self::Back => icon::ARROW_LEFT,
            Self::ScrollTo => icon::ARROW_LINE_DOWN,
            Self::OpenUrl => icon::ARROW_SQUARE_OUT,
            Self::OpenOverlay => icon::OVERLAY_OPEN,
            Self::SwapOverlay => icon::OVERLAY_SWAP,
            Self::CloseOverlay => icon::OVERLAY_CLOSE,
            Self::ChangeState => icon::CHANGE_STATE,
            Self::SetVariable => icon::BRACKETS_CURLY,
            Self::Conditional => icon::CONDITIONAL,
            Self::RunScript => icon::FILE_CODE,
        }
    }
}

/// A trigger's words, as its block's head says them.
pub(crate) fn trigger_label(doc: &Document, t: &Trigger) -> String {
    match t {
        Trigger::Click => "On click".into(),
        Trigger::HoverEnter => "On hover".into(),
        Trigger::HoverWhile => "While hovering".into(),
        Trigger::Press => "On press".into(),
        Trigger::PressWhile => "While pressing".into(),
        Trigger::Drag => "On drag".into(),
        Trigger::Key { key, shift } if key.is_empty() => match shift {
            true => "Key Shift+…".into(),
            false => "Key …".into(),
        },
        Trigger::Key { key, shift: true } => format!("Key Shift+{key}"),
        Trigger::Key { key, .. } => format!("Key {key}"),
        Trigger::After { ms } => format!("After {}", duration_words(*ms)),
        Trigger::VariableChanges { var } => match doc.variables().get(*var) {
            Some(v) => format!("When {} changes", v.name),
            None => "When a missing variable changes".into(),
        },
        Trigger::FrameOpens => "When the frame opens".into(),
    }
}

/// `2 s` for whole seconds, `300 ms` otherwise.
fn duration_words(ms: u32) -> String {
    match ms {
        0 => "0 ms".into(),
        ms if ms % 1000 == 0 => format!("{} s", ms / 1000),
        ms => format!("{ms} ms"),
    }
}

/// The triggers the head's menu offers, grouped: the pointer's, then the rest.
/// `Key` is offered as *Key…*, which waits for the key.
const POINTER_TRIGGERS: [Trigger; 6] = [
    Trigger::Click,
    Trigger::Press,
    Trigger::PressWhile,
    Trigger::HoverEnter,
    Trigger::HoverWhile,
    Trigger::Drag,
];

fn same_trigger_kind(a: &Trigger, b: &Trigger) -> bool {
    std::mem::discriminant(a) == std::mem::discriminant(b)
}

/// The transition's two words on its action row: its kind and its timing.
pub(crate) fn transition_chip(t: &Transition) -> String {
    if t.kind == TransitionKind::Instant {
        return "Instant".into();
    }
    let kind = transition_short(t.kind);
    match t.timing {
        Timing::Spring { .. } => format!("{kind} · Spring"),
        Timing::Ease { ms, .. } => format!("{kind} · {ms} ms"),
    }
}

fn transition_short(k: TransitionKind) -> &'static str {
    match k {
        TransitionKind::Instant => "Instant",
        TransitionKind::Dissolve => "Dissolve",
        TransitionKind::MoveIn => "Move in",
        TransitionKind::MoveOut => "Move out",
        TransitionKind::Push => "Push",
        TransitionKind::Slide => "Slide",
        TransitionKind::SmartAnimate => "Smart",
    }
}

fn transition_label(k: TransitionKind) -> &'static str {
    match k {
        TransitionKind::SmartAnimate => "Smart animate",
        other => transition_short(other),
    }
}

const TRANSITION_KINDS: [TransitionKind; 7] = [
    TransitionKind::Instant,
    TransitionKind::Dissolve,
    TransitionKind::SmartAnimate,
    TransitionKind::MoveIn,
    TransitionKind::MoveOut,
    TransitionKind::Push,
    TransitionKind::Slide,
];

const DIRECTIONS: [(Direction, &str); 5] = [
    (Direction::None, "None"),
    (Direction::Left, "Left"),
    (Direction::Right, "Right"),
    (Direction::Up, "Up"),
    (Direction::Down, "Down"),
];

/// The CSS set, then the custom curve.
const EASINGS: [(Easing, &str); 6] = [
    (Easing::Linear, "Linear"),
    (Easing::Ease, "Ease"),
    (Easing::EaseIn, "Ease in"),
    (Easing::EaseOut, "Ease out"),
    (Easing::EaseInOut, "Ease in and out"),
    (Easing::Cubic([0.22, 1.0, 0.36, 1.0]), "Custom curve"),
];

/// The motion preview's height, its graph's inset from its ground, and the lane
/// under the graph where a dot moves as the transition would.
const PREVIEW_H: f32 = 96.0;
const PREVIEW_PAD: f32 = 8.0;
const PREVIEW_LANE_H: f32 = 12.0;
/// How long the playing dot rests at its end before it plays again.
const PREVIEW_REST_MS: f64 = 600.0;
/// How many pieces the preview's curve is drawn in.
const PREVIEW_STEPS: usize = 96;

/// A transition's timing drawn as its value over time in the accent, the target
/// dashed so a spring's overshoot shows, over a lane where a dot moves as the
/// transition would. Under the pointer both dots play in real time, rest at the
/// end and play again; a timing changed meanwhile starts them over.
fn motion_preview(
    ui: &mut egui::Ui,
    salt: impl std::hash::Hash + std::fmt::Debug,
    timing: Timing,
    w: f32,
) -> egui::Response {
    use mastik_play::motion::{duration, progress};
    let (rect, resp) = ui.allocate_exact_size(egui::vec2(w, PREVIEW_H), egui::Sense::hover());
    let p = ui.painter_at(rect);
    p.rect_filled(rect, egui::CornerRadius::same(ui::BUTTON_R), color::field());
    let total = duration(&timing).max(1.0);
    let at = |ms: f64| progress(&timing, ms).0;
    let samples: Vec<f64> = (0..=PREVIEW_STEPS)
        .map(|i| at(total * i as f64 / PREVIEW_STEPS as f64))
        .collect();
    // Room for an overshoot past the target, or under the start.
    let (lo, hi) = samples
        .iter()
        .fold((0.0_f64, 1.0_f64), |(lo, hi), v| (lo.min(*v), hi.max(*v)));
    let span = (hi - lo).max(1e-6);
    let graph = egui::Rect::from_min_max(
        rect.min + egui::Vec2::splat(PREVIEW_PAD),
        egui::pos2(
            rect.right() - PREVIEW_PAD,
            rect.bottom() - 2.0 * PREVIEW_PAD - PREVIEW_LANE_H,
        ),
    );
    let lane = egui::Rect::from_min_max(
        egui::pos2(graph.left(), graph.bottom() + PREVIEW_PAD),
        egui::pos2(graph.right(), rect.bottom() - PREVIEW_PAD),
    );
    let y_of = |v: f64| graph.bottom() - ((v - lo) / span) as f32 * graph.height();
    let x_of = |f: f64| graph.left() + f as f32 * graph.width();
    let lane_x = |v: f64| lane.left() + ((v - lo) / span) as f32 * lane.width();
    let rule = egui::Stroke::new(1.0, color::divider());
    let spring = matches!(timing, Timing::Spring { .. });
    p.extend(egui::Shape::dashed_line(
        &[
            egui::pos2(graph.left(), y_of(1.0)),
            egui::pos2(graph.right(), y_of(1.0)),
        ],
        rule,
        3.0,
        3.0,
    ));
    let line: Vec<egui::Pos2> = samples
        .iter()
        .enumerate()
        .map(|(i, v)| egui::pos2(x_of(i as f64 / PREVIEW_STEPS as f64), y_of(*v)))
        .collect();
    let ink = color::accent_400();
    p.add(egui::Shape::line(line, egui::Stroke::new(2.0, ink)));
    // The lane: a track from where it starts to where it lands; a spring's
    // landing ticked, dotted, as the point it overshoots.
    let mid = lane.center().y;
    p.line_segment(
        [egui::pos2(lane.left(), mid), egui::pos2(lane.right(), mid)],
        rule,
    );
    if spring {
        let x = lane_x(1.0);
        p.extend(egui::Shape::dotted_line(
            &[egui::pos2(x, lane.top()), egui::pos2(x, lane.bottom())],
            theme::text::faint(),
            3.0,
            0.75,
        ));
    }
    // Played under the pointer, from the time the pointer came or the timing changed.
    let key = ui.id().with(("motion-preview", salt));
    let now = ui.input(|i| i.time);
    let played = match resp.hovered() {
        true => {
            let kept = ui.ctx().data(|d| d.get_temp::<(f64, Timing)>(key));
            let start = match kept {
                Some((s, t)) if t == timing => s,
                _ => {
                    ui.ctx().data_mut(|d| d.insert_temp(key, (now, timing)));
                    now
                }
            };
            ui.ctx().request_repaint();
            Some(((now - start) * 1000.0 % (total + PREVIEW_REST_MS)).min(total))
        }
        false => {
            ui.ctx().data_mut(|d| d.remove::<(f64, Timing)>(key));
            None
        }
    };
    let edge = egui::Stroke::new(1.5, color::field());
    // A spring runs until it settles: said in the graph's lower right, which
    // its curve has left by then, over the curve where an undershoot reaches.
    // A curve's duration is in the field above.
    if spring {
        p.text(
            egui::pos2(graph.right(), graph.bottom()),
            egui::Align2::RIGHT_BOTTOM,
            format!("Settles in {} ms", total.round()),
            egui::FontId::proportional(ui::SMALL_PT),
            theme::text::muted(),
        );
    }
    match played {
        Some(ms) => {
            let v = at(ms);
            p.circle(
                egui::pos2(x_of(ms / total), y_of(v)),
                4.0,
                theme::text::strong(),
                edge,
            );
            p.circle(egui::pos2(lane_x(v), mid), 5.0, ink, edge);
        }
        None => {
            p.circle(egui::pos2(lane_x(0.0), mid), 5.0, ink, edge);
        }
    }
    resp
}

/// An overlay's positions, in the *Position* menu's order.
const OVERLAY_POSITIONS: [(OverlayPosition, &str); 8] = [
    (OverlayPosition::Centre, "Centre"),
    (OverlayPosition::TopLeft, "Top left"),
    (OverlayPosition::TopCentre, "Top centre"),
    (OverlayPosition::TopRight, "Top right"),
    (OverlayPosition::BottomLeft, "Bottom left"),
    (OverlayPosition::BottomCentre, "Bottom centre"),
    (OverlayPosition::BottomRight, "Bottom right"),
    (OverlayPosition::Manual { x: 0.0, y: 0.0 }, "Manual"),
];

fn easing_label(e: Easing) -> &'static str {
    EASINGS
        .iter()
        .find(|(x, _)| std::mem::discriminant(x) == std::mem::discriminant(&e))
        .map_or("Custom curve", |(_, l)| *l)
}

/// The transition an action takes when it moves the screen.
fn transition_of(a: &Action) -> Option<Transition> {
    match a {
        Action::Navigate { transition, .. }
        | Action::OpenOverlay { transition, .. }
        | Action::CloseOverlay { transition }
        | Action::SwapOverlay { transition, .. }
        | Action::ScrollTo { transition, .. }
        | Action::ChangeState { transition, .. } => Some(*transition),
        _ => None,
    }
}

fn with_transition(a: &Action, t: Transition) -> Action {
    let mut a = a.clone();
    match &mut a {
        Action::Navigate { transition, .. }
        | Action::OpenOverlay { transition, .. }
        | Action::CloseOverlay { transition }
        | Action::SwapOverlay { transition, .. }
        | Action::ScrollTo { transition, .. }
        | Action::ChangeState { transition, .. } => *transition = t,
        _ => {}
    }
    a
}

// --- the action list, by id ---------------------------------------------------

/// The action `id` in `list`, nested ones included.
pub(crate) fn find(list: &[Keyed<Action>], id: ItemId) -> Option<&Keyed<Action>> {
    list.iter().find_map(|k| {
        if k.id == id {
            return Some(k);
        }
        match &k.value {
            Action::Conditional {
                branches,
                otherwise,
            } => branches
                .iter()
                .find_map(|b| find(&b.actions, id))
                .or_else(|| find(otherwise, id)),
            _ => None,
        }
    })
}

/// [`find`], to change it.
pub(crate) fn find_mut(list: &mut [Keyed<Action>], id: ItemId) -> Option<&mut Keyed<Action>> {
    for k in list.iter_mut() {
        if k.id == id {
            return Some(k);
        }
        if let Action::Conditional {
            branches,
            otherwise,
        } = &mut k.value
        {
            for b in branches.iter_mut() {
                if let Some(hit) = find_mut(&mut b.actions, id) {
                    return Some(hit);
                }
            }
            if let Some(hit) = find_mut(otherwise, id) {
                return Some(hit);
            }
        }
    }
    None
}

/// The list `place` names in a reaction's `actions`.
pub(crate) fn list_at(
    actions: &mut Vec<Keyed<Action>>,
    place: Place,
) -> Option<&mut Vec<Keyed<Action>>> {
    let (cond, branch) = match place {
        Place::Top => return Some(actions),
        Place::Branch(c, i) => (c, Some(i)),
        Place::Otherwise(c) => (c, None),
    };
    match &mut find_mut(actions, cond)?.value {
        Action::Conditional {
            branches,
            otherwise,
        } => match branch {
            Some(i) => branches.get_mut(i).map(|b| &mut b.actions),
            None => Some(otherwise),
        },
        _ => None,
    }
}

/// Take the action `id` out of whichever list holds it; whether it was there.
pub(crate) fn remove(list: &mut Vec<Keyed<Action>>, id: ItemId) -> bool {
    if let Some(i) = list.iter().position(|k| k.id == id) {
        list.remove(i);
        return true;
    }
    list.iter_mut().any(|k| match &mut k.value {
        Action::Conditional {
            branches,
            otherwise,
        } => branches.iter_mut().any(|b| remove(&mut b.actions, id)) || remove(otherwise, id),
        _ => false,
    })
}

/// Move the action `id` one place up (`up`) or down within its own list; whether
/// it moved.
pub(crate) fn shift(list: &mut [Keyed<Action>], id: ItemId, up: bool) -> bool {
    if let Some(i) = list.iter().position(|k| k.id == id) {
        let j = match up {
            true => i.checked_sub(1),
            false => (i + 1 < list.len()).then_some(i + 1),
        };
        return match j {
            Some(j) => {
                list.swap(i, j);
                true
            }
            None => false,
        };
    }
    list.iter_mut().any(|k| match &mut k.value {
        Action::Conditional {
            branches,
            otherwise,
        } => branches.iter_mut().any(|b| shift(&mut b.actions, id, up)) || shift(otherwise, id, up),
        _ => false,
    })
}

/// Move the action `id` to `to` within its own list, `to` counted among the
/// list without it; whether it moved.
pub(crate) fn move_to(list: &mut Vec<Keyed<Action>>, id: ItemId, to: usize) -> bool {
    if let Some(i) = list.iter().position(|k| k.id == id) {
        let k = list.remove(i);
        let to = to.min(list.len());
        list.insert(to, k);
        return i != to;
    }
    list.iter_mut().any(|k| match &mut k.value {
        Action::Conditional {
            branches,
            otherwise,
        } => {
            branches.iter_mut().any(|b| move_to(&mut b.actions, id, to))
                || move_to(otherwise, id, to)
        }
        _ => false,
    })
}

/// Where the action `id` sits in its list and how long that list is.
fn position(list: &[Keyed<Action>], id: ItemId) -> Option<(usize, usize)> {
    if let Some(i) = list.iter().position(|k| k.id == id) {
        return Some((i, list.len()));
    }
    list.iter().find_map(|k| match &k.value {
        Action::Conditional {
            branches,
            otherwise,
        } => branches
            .iter()
            .find_map(|b| position(&b.actions, id))
            .or_else(|| position(otherwise, id)),
        _ => None,
    })
}

/// A copy of `list` with every item id (nested ones included) minted afresh, for
/// *Duplicate*: the door refuses a repeated id.
fn fresh_ids(list: &[Keyed<Action>], mint: &mut impl FnMut() -> ItemId) -> Vec<Keyed<Action>> {
    list.iter()
        .map(|k| {
            let value = match &k.value {
                Action::Conditional {
                    branches,
                    otherwise,
                } => Action::Conditional {
                    branches: branches
                        .iter()
                        .map(|b| Branch {
                            when: b.when.clone(),
                            actions: fresh_ids(&b.actions, mint),
                        })
                        .collect(),
                    otherwise: fresh_ids(otherwise, mint),
                },
                other => other.clone(),
            };
            Keyed::new(mint(), value)
        })
        .collect()
}

// --- expressions ----------------------------------------------------------------

/// An expression as its field shows it: each `{<actor-hex>:<seq>}` written as the
/// variable's name in braces, a variable that is gone left as its id.
pub(crate) fn expr_shown(doc: &Document, e: &Expr) -> String {
    e.with_names(&|v| doc.variables().get(v).map(|v| v.name.clone()))
}

/// What was typed into an expression field, as stored: each `{name}` written as
/// its variable's id. Refused, with the reason, when it does not read, names a
/// variable no one has, or (for *Set variable*, `want`) comes to another kind.
/// A refusal carries the byte range of the typed text it is about, when the
/// expression names one.
pub(crate) fn expr_stored(
    doc: &Document,
    text: &str,
    want: Option<VarKind>,
) -> Result<Expr, (String, Option<std::ops::Range<usize>>)> {
    let (e, error) = Expr::from_names(text, &|n| doc.variables().by_name(n).map(|v| v.id));
    if let Some(why) = error {
        return Err((capitalised(&why.to_string()), Some(why.at())));
    }
    if let Some(want) = want {
        // `kind` reads the stored form, whose ranges are not the typed text's.
        let got = e
            .kind(&|v| doc.variables().get(v).map(|v| v.kind))
            .map_err(|why| (capitalised(&why.to_string()), None))?;
        if got != want {
            return Err((
                format!(
                    "This comes to a {}, not a {}",
                    kind_word(got),
                    kind_word(want)
                ),
                None,
            ));
        }
    }
    Ok(e)
}

fn kind_word(k: VarKind) -> &'static str {
    match k {
        VarKind::Number => "number",
        VarKind::String => "string",
        VarKind::Boolean => "boolean",
        VarKind::Color => "colour",
    }
}

fn capitalised(s: &str) -> String {
    let mut c = s.chars();
    match c.next() {
        Some(f) => f.to_uppercase().chain(c).collect(),
        None => String::new(),
    }
}

/// Whether `url` is an address *Open link* takes: `http` or `https`.
pub(crate) fn is_link(url: &str) -> bool {
    let u = url.trim().to_ascii_lowercase();
    (u.starts_with("https://") && u.len() > 8) || (u.starts_with("http://") && u.len() > 7)
}

// --- the document, as the tab reads it -------------------------------------------

/// The top-level frames of every page, page by page: what *Navigate to* and the
/// overlay actions go to.
pub(crate) fn screens(doc: &Document) -> Vec<(NodeId, Vec<NodeId>)> {
    doc.pages()
        .iter()
        .map(|p| {
            let frames = doc
                .get(p.id)
                .map(|root| {
                    root.children()
                        .iter()
                        .copied()
                        .filter(|c| is_frame(doc, *c))
                        .collect()
                })
                .unwrap_or_default();
            (p.id, frames)
        })
        .collect()
}

pub(crate) fn is_frame(doc: &Document, id: NodeId) -> bool {
    doc.get(id)
        .is_some_and(|n| matches!(n.kind(), NodeKind::Artboard { .. }))
}

/// The top-level frame `id` is in (itself, for one), or `None` on open canvas.
pub(crate) fn screen_of(doc: &Document, id: NodeId) -> Option<NodeId> {
    let mut at = id;
    loop {
        let parent = doc.get(at)?.parent()?;
        if doc.page(parent).is_some() {
            return is_frame(doc, at).then_some(at);
        }
        at = parent;
    }
}

/// Every layer under `root`, depth first, `root` left out.
fn descendants(doc: &Document, root: NodeId) -> Vec<NodeId> {
    let mut out = Vec::new();
    let mut stack: Vec<NodeId> = doc
        .get(root)
        .map(|n| n.children().iter().rev().copied().collect())
        .unwrap_or_default();
    while let Some(id) = stack.pop() {
        out.push(id);
        if let Some(n) = doc.get(id) {
            stack.extend(n.children().iter().rev().copied());
        }
    }
    out
}

/// The instance *Change to* switches from a layer: the instance it is or is in,
/// when that instance's main is a variant of a set.
pub(crate) fn state_instance(doc: &Document, id: NodeId) -> Option<NodeId> {
    let root = mastik_core::component::instance_root(doc, id)?;
    let main = mastik_core::component::main_of(doc, root)?;
    mastik_core::variant::set_of(doc, main).map(|_| root)
}

/// The variants an instance may change to: its main's set's, in order.
fn states_of(doc: &Document, instance: NodeId) -> Vec<NodeId> {
    mastik_core::component::main_of(doc, instance)
        .and_then(|m| mastik_core::variant::set_of(doc, m))
        .map(|set| mastik_core::variant::variants(doc, set))
        .unwrap_or_default()
}

fn name(doc: &Document, id: NodeId) -> String {
    crate::private::layer(doc, id).into_owned()
}

/// What an action names, as its row writes it, and the word for it gone when
/// it does not exist. A target is read as the layer holding the reaction reads it.
fn target_words(doc: &Document, holder: NodeId, a: &Action) -> (String, Option<&'static str>) {
    let layer = |t: NodeId, gone: &'static str| {
        let t = mastik_core::prototype::resolve_target(doc, holder, t);
        match doc.get(t) {
            Some(_) => (name(doc, t), None),
            None => (gone.to_string(), Some(gone)),
        }
    };
    match a {
        Action::Navigate { to, .. } => layer(*to, "Missing frame"),
        Action::OpenOverlay { overlay, .. } | Action::SwapOverlay { overlay, .. } => {
            layer(*overlay, "Missing frame")
        }
        Action::ScrollTo { layer: l, .. } => layer(*l, "Missing layer"),
        Action::ChangeState { instance, to, .. } => {
            let inst = mastik_core::prototype::resolve_target(doc, holder, *instance);
            match (doc.get(inst), doc.get(*to)) {
                (Some(_), Some(_)) => (name(doc, *to), None),
                (None, _) => ("Missing instance".into(), Some("Missing instance")),
                (_, None) => ("Missing variant".into(), Some("Missing variant")),
            }
        }
        Action::SetVariable { var, change } => match doc.variables().get(*var) {
            Some(v) => {
                let how = match change {
                    VarChange::Set { to } => format!("= {}", expr_shown(doc, to)),
                    VarChange::Increment { by } if *by < 0.0 => format!("− {}", -by),
                    VarChange::Increment { by } => format!("+ {by}"),
                    VarChange::Toggle => "toggled".into(),
                };
                (format!("{} {how}", v.name), None)
            }
            None => ("Missing variable".into(), Some("Missing variable")),
        },
        Action::Conditional { branches, .. } => (
            branches
                .first()
                .map(|b| expr_shown(doc, &b.when))
                .unwrap_or_default(),
            None,
        ),
        Action::OpenUrl { url } => (url.clone(), None),
        Action::RunScript { script } => match doc.script(*script) {
            Some(s) => (s.name.clone(), None),
            None => ("Missing script".into(), Some("Missing script")),
        },
        Action::Back | Action::CloseOverlay { .. } => (String::new(), None),
    }
}

/// The transition a new link on `page` takes: the page's own default.
pub(crate) fn page_transition(doc: &Document, page: Option<NodeId>) -> Transition {
    page.and_then(|p| doc.page(p))
        .map_or_else(Transition::default, |p| p.play.transition)
}

/// A new action of `kind` for a reaction on `holder`, its target the likeliest
/// one: another screen, the holder's own instance, the first variable or script.
/// `None` where the document has nothing it could name.
pub(crate) fn new_action(doc: &Document, holder: NodeId, kind: ActionKind) -> Option<Action> {
    let transition = page_transition(doc, doc.page_of(holder));
    let own = screen_of(doc, holder);
    let other_screen = || {
        let all: Vec<NodeId> = screens(doc).into_iter().flat_map(|(_, f)| f).collect();
        // A main is a component, not a screen, so it is picked last.
        let main = |f: &NodeId| doc.get(*f).is_some_and(|n| n.component());
        all.iter()
            .copied()
            .find(|f| Some(*f) != own && !main(f))
            .or_else(|| all.iter().copied().find(|f| Some(*f) != own))
            .or_else(|| all.first().copied())
    };
    Some(match kind {
        ActionKind::Navigate => Action::Navigate {
            to: other_screen()?,
            transition,
        },
        ActionKind::Back => Action::Back,
        ActionKind::ScrollTo => Action::ScrollTo {
            layer: own.and_then(|s| descendants(doc, s).first().copied())?,
            transition,
        },
        ActionKind::OpenUrl => Action::OpenUrl {
            url: "https://".into(),
        },
        ActionKind::OpenOverlay => Action::OpenOverlay {
            overlay: other_screen()?,
            settings: OverlaySettings::default(),
            transition,
        },
        ActionKind::SwapOverlay => Action::SwapOverlay {
            overlay: other_screen()?,
            settings: OverlaySettings::default(),
            transition,
        },
        ActionKind::CloseOverlay => Action::CloseOverlay { transition },
        ActionKind::ChangeState => {
            let instance = state_instance(doc, holder)?;
            let main = mastik_core::component::main_of(doc, instance)?;
            let to = states_of(doc, instance)
                .into_iter()
                .find(|v| *v != main)
                .unwrap_or(main);
            Action::ChangeState {
                instance,
                to,
                transition,
            }
        }
        ActionKind::SetVariable => {
            let v = doc.variables().iter().next()?;
            let change = match v.kind {
                VarKind::Boolean => VarChange::Toggle,
                VarKind::Number => VarChange::Increment { by: 1.0 },
                VarKind::String | VarKind::Color => VarChange::Set {
                    to: Expr(String::new()),
                },
            };
            Action::SetVariable { var: v.id, change }
        }
        ActionKind::Conditional => Action::Conditional {
            branches: vec![Branch {
                when: Expr("true".into()),
                actions: Vec::new(),
            }],
            otherwise: Vec::new(),
        },
        ActionKind::RunScript => Action::RunScript {
            script: doc.scripts().first()?.id,
        },
    })
}

/// Why the add menu cannot offer `kind` under `trigger` on `holder`, or `None`
/// when it can.
pub(crate) fn action_refusal(
    doc: &Document,
    holder: NodeId,
    trigger: &Trigger,
    kind: ActionKind,
) -> Option<&'static str> {
    match kind {
        ActionKind::RunScript if !trigger.is_a_persons_act() => {
            Some("A script runs only when someone acts")
        }
        ActionKind::RunScript if doc.scripts().is_empty() => Some("This document has no scripts"),
        ActionKind::ChangeState if state_instance(doc, holder).is_none() => Some("Not a component"),
        ActionKind::SetVariable if doc.variables().iter().next().is_none() => {
            Some("This document has no variables")
        }
        ActionKind::ScrollTo if new_action(doc, holder, kind).is_none() => {
            Some("Only a layer in a frame can be scrolled to")
        }
        ActionKind::Navigate | ActionKind::OpenOverlay | ActionKind::SwapOverlay
            if new_action(doc, holder, kind).is_none() =>
        {
            Some("No frame to go to")
        }
        _ => None,
    }
}

/// The connectors of one action list of reaction `reaction` on `holder`, nested
/// lists' dashed.
fn layer_links(
    doc: &Document,
    holder: NodeId,
    reaction: ItemId,
    list: &[Keyed<Action>],
    dashed: bool,
    out: &mut Vec<Link>,
) {
    for k in list {
        match &k.value {
            Action::Navigate { to: t, .. }
            | Action::OpenOverlay { overlay: t, .. }
            | Action::SwapOverlay { overlay: t, .. } => {
                let overlay = !matches!(k.value, Action::Navigate { .. });
                out.push(Link {
                    from: holder,
                    to: mastik_core::prototype::resolve_target(doc, holder, *t),
                    dashed: dashed || overlay,
                    at: ActionAt {
                        node: holder,
                        reaction,
                        action: k.id,
                    },
                });
            }
            Action::Conditional {
                branches,
                otherwise,
            } => {
                for b in branches {
                    layer_links(doc, holder, reaction, &b.actions, true, out);
                }
                layer_links(doc, holder, reaction, otherwise, true, out);
            }
            _ => {}
        }
    }
}

/// One connector the canvas draws while the Prototype tab is up.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct Link {
    pub from: NodeId,
    /// The target as the holder reads it; it may be gone.
    pub to: NodeId,
    /// Inside a conditional, or opening an overlay.
    pub dashed: bool,
    /// The action it draws, which a click on it opens.
    pub at: ActionAt,
}

/// Every connector on `page`: each navigate or overlay action of each layer.
pub(crate) fn links_on(doc: &Document, page: NodeId) -> Vec<Link> {
    let mut out = Vec::new();
    for id in descendants(doc, page) {
        let Some(n) = doc.get(id) else { continue };
        for r in n.reactions() {
            layer_links(doc, id, r.id, &r.actions, false, &mut out);
        }
    }
    out
}

/// How far from a connector's line a press still takes it: wider than its ink,
/// so it need not be hit on the pixel.
const LINK_PICK: f32 = LINK_W / 2.0 + 5.0;
/// A hovered connector's weight.
const LINK_HOT_W: f32 = LINK_W + 1.5;

/// How far `p` is from the polyline `pts`.
fn polyline_distance(pts: &[egui::Pos2], p: egui::Pos2) -> f32 {
    match pts {
        [] => f32::INFINITY,
        [a] => a.distance(p),
        _ => pts
            .windows(2)
            .map(|w| {
                let (a, b) = (w[0], w[1]);
                let ab = b - a;
                let l = ab.length_sq();
                let t = if l > 0.0 {
                    ((p - a).dot(ab) / l).clamp(0.0, 1.0)
                } else {
                    0.0
                };
                p.distance(a + ab * t)
            })
            .fold(f32::INFINITY, f32::min),
    }
}

/// `reactions` on `id` with a click navigating to `to` added: to the layer's
/// first click reaction, or as a new one. What a canvas link dot's drop commits.
pub(crate) fn with_link(
    doc: &Document,
    id: NodeId,
    to: NodeId,
    mint: &mut impl FnMut() -> ItemId,
) -> Vec<Keyed<Reaction>> {
    let mut list = doc
        .get(id)
        .map(|n| n.reactions().to_vec())
        .unwrap_or_default();
    let action = Keyed::new(
        mint(),
        Action::Navigate {
            to,
            transition: page_transition(doc, doc.page_of(id)),
        },
    );
    match list.iter_mut().find(|r| r.trigger == Trigger::Click) {
        Some(r) => r.value.actions.push(action),
        None => list.push(Keyed::new(
            mint(),
            Reaction {
                trigger: Trigger::Click,
                actions: vec![action],
            },
        )),
    }
    list
}

// --- the canvas ---------------------------------------------------------------

/// A link dot's radius at rest and under the pointer: a size up from a shape's
/// corner dots, so the two handles do not read as one.
const KNOB_R: f32 = 5.0;
const KNOB_HOT_R: f32 = 6.0;
/// How far from a link dot's centre a press still takes it. Wider than its ink
/// and tested before the box's handles, so the dot on an edge is easy to take
/// from the resize there.
const KNOB_PICK: f32 = KNOB_R + 5.0;
/// A connector's weight, and its arrowhead's length and half-width.
const LINK_W: f32 = 3.0;
const HEAD_LEN: f32 = 12.0;
const HEAD_HALF: f32 = 6.0;
/// How far a missing or off-page target's stub reaches from its layer.
const STUB: f32 = 28.0;
/// The longest straight piece a connector's curve is drawn in, so it stays
/// smooth however far the canvas is zoomed in.
const CURVE_STEP: f32 = 3.0;

/// The screen box of a quad.
fn quad_box(q: [egui::Pos2; 4]) -> egui::Rect {
    egui::Rect::from_points(&q)
}

/// The link dots of a layer's screen quad: each edge's index, its middle, and
/// the outward direction a link leaves it by. A collapsed edge has none.
fn knob_dots(q: [egui::Pos2; 4]) -> Vec<(usize, egui::Pos2, egui::Vec2)> {
    let centre = q.iter().fold(egui::Vec2::ZERO, |s, p| s + p.to_vec2()) / 4.0;
    (0..4)
        .filter_map(|i| {
            let mid = q[i] + (q[(i + 1) % 4] - q[i]) * 0.5;
            let away = mid.to_vec2() - centre;
            (away.length() > 0.5).then(|| (i, mid, away.normalized()))
        })
        .collect()
}

/// How close the pointer comes to an edge before that edge's link dot shows:
/// wide enough to see it coming, so a box does not wear four dots at once.
/// Never less than [`KNOB_PICK`], so a dot that takes a press is one drawn.
const KNOB_REVEAL: f32 = 28.0;
const _: () = assert!(KNOB_REVEAL >= KNOB_PICK);

/// How far `p` is from edge `edge` of the quad `q`, as a segment.
fn edge_distance(q: [egui::Pos2; 4], edge: usize, p: egui::Pos2) -> f32 {
    let (a, b) = (q[edge], q[(edge + 1) % 4]);
    let ab = b - a;
    let l = ab.length_sq();
    let t = if l > 0.0 {
        ((p - a).dot(ab) / l).clamp(0.0, 1.0)
    } else {
        0.0
    };
    p.distance(a + ab * t)
}

/// A layer's box in world space, its corners in the screen quad's order, so an
/// edge's index names the same edge in both. Which edge a connector takes is
/// decided here, in world space, so zooming never changes the answer.
type WorldQuad = [mastik_core::kurbo::Point; 4];

fn quad_centre(q: WorldQuad) -> mastik_core::kurbo::Point {
    q[0].midpoint(q[2])
}

fn edge_mid(q: WorldQuad, edge: usize) -> mastik_core::kurbo::Point {
    q[edge].midpoint(q[(edge + 1) % 4])
}

/// The edge of `q` whose outward direction best matches `want(middle)`, the
/// way it should face from that edge's middle; none if every edge is collapsed.
fn facing_edge(
    q: WorldQuad,
    want: impl Fn(mastik_core::kurbo::Point) -> mastik_core::kurbo::Vec2,
) -> Option<usize> {
    let c = quad_centre(q);
    (0..4)
        .filter_map(|i| {
            let mid = edge_mid(q, i);
            let out = mid - c;
            let w = want(mid);
            (out.hypot() > 1e-9).then(|| {
                let score = match w.hypot() > 1e-9 {
                    true => out.normalize().dot(w.normalize()),
                    false => 0.0,
                };
                (i, score)
            })
        })
        .max_by(|a, b| a.1.total_cmp(&b.1))
        .map(|(i, _)| i)
}

/// The edges a connector leaves `from` by and enters `to` by: `from`'s edge
/// facing `to`'s centre, then `to`'s edge facing that edge's middle.
fn connector_edges(from: WorldQuad, to: WorldQuad) -> Option<(usize, usize)> {
    let target = quad_centre(to);
    let a = facing_edge(from, |mid| target - mid)?;
    let start = edge_mid(from, a);
    let b = facing_edge(to, |mid| start - mid)?;
    Some((a, b))
}

/// The edge a link to nothing on this page leaves `from` by: the one facing out
/// of its frame (whose centre is `frame`) along the axis it is further off that
/// centre, the way off the screen. Without a frame, or at its middle, the right.
fn stub_edge(from: WorldQuad, frame: Option<mastik_core::kurbo::Point>) -> Option<usize> {
    use mastik_core::kurbo::Vec2;
    let d = frame.map_or(Vec2::ZERO, |f| quad_centre(from) - f);
    let dir = match d.x.abs() >= d.y.abs() {
        true if d.x < 0.0 => Vec2::new(-1.0, 0.0),
        true => Vec2::new(1.0, 0.0),
        false if d.y < 0.0 => Vec2::new(0.0, -1.0),
        false => Vec2::new(0.0, 1.0),
    };
    facing_edge(from, |_| dir)
}

/// Edge `edge` of the screen quad `q`: its middle, and its outward direction
/// (rightward for a quad collapsed to a point).
fn screen_edge(q: [egui::Pos2; 4], edge: usize) -> (egui::Pos2, egui::Vec2) {
    let centre = q[0] + (q[2] - q[0]) * 0.5;
    let mid = q[edge] + (q[(edge + 1) % 4] - q[edge]) * 0.5;
    let away = mid - centre;
    match away.length() > 1e-3 {
        true => (mid, away.normalized()),
        false => (mid, egui::Vec2::X),
    }
}

/// The cubic from `a`, leaving along `out_a`, to `b`, arriving against `out_b`
/// (the outward direction of the side it enters; zero for a free end), in
/// pieces no longer than [`CURVE_STEP`].
fn curve(a: egui::Pos2, out_a: egui::Vec2, b: egui::Pos2, out_b: egui::Vec2) -> Vec<egui::Pos2> {
    let reach = ((b - a).length() * 0.4).max(24.0);
    let c1 = a + out_a * reach;
    let c2 = b + out_b * reach;
    let rough = (c1 - a).length() + (c2 - c1).length() + (b - c2).length();
    let n = ((rough / CURVE_STEP).ceil() as usize).clamp(8, 1024);
    (0..=n)
        .map(|i| {
            let t = i as f32 / n as f32;
            let u = 1.0 - t;
            let p = a.to_vec2() * (u * u * u)
                + c1.to_vec2() * (3.0 * u * u * t)
                + c2.to_vec2() * (3.0 * u * t * t)
                + b.to_vec2() * (t * t * t);
            p.to_pos2()
        })
        .collect()
}

/// Stroke `pts` as a connector `w` thick with an arrowhead on its last point. The
/// line stops at the head's base, so its weight never shows past the tip.
fn paint_link(
    painter: &egui::Painter,
    mut pts: Vec<egui::Pos2>,
    dashed: bool,
    ink: egui::Color32,
    w: f32,
) {
    let Some(&tip) = pts.last() else { return };
    let (dir, keep) = match pts.iter().rposition(|p| p.distance(tip) >= HEAD_LEN) {
        Some(i) => ((tip - pts[i]).normalized(), i + 1),
        None => ((tip - pts[0]).normalized(), 0),
    };
    if dir.length() < 0.5 {
        return;
    }
    let base = tip - dir * HEAD_LEN;
    pts.truncate(keep);
    if !pts.is_empty() {
        pts.push(base);
        let stroke = egui::Stroke::new(w, ink);
        match dashed {
            true => painter.extend(egui::Shape::dashed_line(&pts, stroke, 6.0, 4.0)),
            false => {
                painter.add(egui::Shape::line(pts, stroke));
            }
        }
    }
    let side = egui::vec2(-dir.y, dir.x);
    painter.add(egui::Shape::convex_polygon(
        vec![tip, base + side * HEAD_HALF, base - side * HEAD_HALF],
        ink,
        egui::Stroke::NONE,
    ));
}

/// A flow chip's height on the canvas, and the gap it keeps above its frame.
const FLOW_CHIP_H: f32 = 20.0;
const FLOW_CHIP_GAP: f32 = 6.0;
/// The air after a flow chip's words, to the pill's round end.
const FLOW_CHIP_PAD_R: f32 = 7.0;

impl MastikApp {
    /// The flow chips on the current page while the Prototype tab is up: each
    /// flow's name over its start frame's top-right corner, which plays it.
    /// Each chip's flow, rect and words; a frame starting several flows stacks
    /// them leftward.
    pub(crate) fn flow_chips(
        &self,
        ui: &egui::Ui,
        rect: egui::Rect,
        ppp: f32,
    ) -> Vec<(mastik_core::prototype::FlowRef, egui::Rect, String)> {
        if !self.prototype_tab {
            return Vec::new();
        }
        let doc = &self.session.doc;
        let page = self.session.page;
        let Some(info) = doc.page(page) else {
            return Vec::new();
        };
        let font = egui::FontId::proportional(ui::SMALL_PT);
        let mut out: Vec<(mastik_core::prototype::FlowRef, egui::Rect, String)> = Vec::new();
        for f in &info.flows {
            let Some(frame) = self.selection_quad(f.start, rect, ppp).map(quad_box) else {
                continue;
            };
            let words = crate::private::flow(&f.name, out.len()).into_owned();
            let text_w = ui.fonts_mut(|fx| {
                fx.layout_no_wrap(words.clone(), font.clone(), egui::Color32::WHITE)
                    .size()
                    .x
            });
            // The glyph's room on the left, the words, and air on the right.
            let w = text_w + 22.0 + FLOW_CHIP_PAD_R;
            // Leftward of any chip this frame already wears.
            let right = out
                .iter()
                .filter(|(r, ..)| {
                    info.flows
                        .iter()
                        .any(|x| x.id == r.flow && x.start == f.start)
                })
                .map(|(_, c, _)| c.left() - 4.0)
                .fold(frame.right(), f32::min);
            let chip = egui::Rect::from_min_max(
                egui::pos2(right - w, frame.top() - FLOW_CHIP_GAP - FLOW_CHIP_H),
                egui::pos2(right, frame.top() - FLOW_CHIP_GAP),
            );
            let r = mastik_core::prototype::FlowRef { page, flow: f.id };
            out.push((r, chip, words));
        }
        out
    }

    /// The flow whose chip is at `screen`.
    pub(crate) fn flow_chip_at(
        &self,
        ui: &egui::Ui,
        screen: egui::Pos2,
        rect: egui::Rect,
        ppp: f32,
    ) -> Option<mastik_core::prototype::FlowRef> {
        self.flow_chips(ui, rect, ppp)
            .into_iter()
            .find(|(_, c, _)| c.contains(screen))
            .map(|(r, ..)| r)
    }

    /// Draw the flow chips: the play glyph and the flow's name on a pill of
    /// the canvas badges' blue, lifted under the pointer as the crop's *Done* is.
    fn draw_flow_chips(&self, ui: &egui::Ui, painter: &egui::Painter, rect: egui::Rect, ppp: f32) {
        let hover = ui.ctx().pointer_hover_pos();
        let ink = ui::pill_ink(color::BADGE);
        for (_, chip, words) in self.flow_chips(ui, rect, ppp) {
            let lit = hover.is_some_and(|p| chip.contains(p));
            painter.rect_filled(
                chip,
                FLOW_CHIP_H / 2.0,
                match lit {
                    true => color::BADGE.blend(egui::Color32::from_white_alpha(26)),
                    false => color::BADGE,
                },
            );
            // text-ink: words on the chip's badge blue, in the badges' own ink.
            painter.text(
                egui::pos2(chip.left() + 13.0, chip.center().y),
                egui::Align2::CENTER_CENTER,
                icon::PLAY,
                theme::icon_font(10.0),
                ink,
            );
            painter.text(
                egui::pos2(chip.left() + 22.0, chip.center().y),
                egui::Align2::LEFT_CENTER,
                words,
                egui::FontId::proportional(ui::SMALL_PT),
                ink,
            );
        }
    }

    /// The layers wearing link dots, while the Prototype tab is up and a
    /// selecting tool is in hand: the one selected layer, then the hovered one.
    fn knob_layers(&self) -> Vec<NodeId> {
        if !self.prototype_tab || !self.tool.selects() {
            return Vec::new();
        }
        let mut out: Vec<NodeId> = self.session.selection.single().into_iter().collect();
        if let Some(id) = self.proto.knob_hover.get()
            && !out.contains(&id)
            && self.session.doc.get(id).is_some()
        {
            out.push(id);
        }
        out
    }

    /// The link dot at `screen`, the nearest within reach: its layer and the
    /// edge it sits on. Within reach of a dot is near its edge, so the dot
    /// answering is one drawn ([`KNOB_REVEAL`]).
    pub(crate) fn link_knob_at(
        &self,
        screen: egui::Pos2,
        rect: egui::Rect,
        ppp: f32,
    ) -> Option<(NodeId, usize)> {
        // A placed overlay's ghost lies over the page: a dot under it is not taken.
        if self.ghost_at(screen) {
            return None;
        }
        self.knob_layers().into_iter().find_map(|id| {
            knob_dots(self.selection_quad(id, rect, ppp)?)
                .into_iter()
                .map(|(edge, at, _)| (edge, at.distance(screen)))
                .filter(|(_, d)| *d <= KNOB_PICK)
                .min_by(|a, b| a.1.total_cmp(&b.1))
                .map(|(edge, _)| (id, edge))
        })
    }

    /// Note which unselected layer wears link dots this frame: the one under
    /// the pointer, or the one that wore them last frame while the pointer is on
    /// one of its dots, which sit half outside it.
    pub(crate) fn note_link_hover(
        &self,
        ui: &egui::Ui,
        rect: egui::Rect,
        ppp: f32,
        resp: &egui::Response,
    ) {
        let live = self.prototype_tab && self.tool.selects() && self.hover_chrome_live();
        let next = live
            .then(|| {
                let p = resp.hover_pos()?;
                let kept = self.proto.knob_hover.get().filter(|id| {
                    !self.session.selection.contains(*id)
                        && self.selection_quad(*id, rect, ppp).is_some_and(|q| {
                            knob_dots(q)
                                .iter()
                                .any(|(_, at, _)| at.distance(p) <= KNOB_PICK)
                        })
                });
                kept.or_else(|| self.hover_target(ui, rect, ppp, resp))
            })
            .flatten();
        self.proto.knob_hover.set(next);
    }

    /// The layer the hover outline rings for the Prototype tab: the one whose
    /// connector is under the pointer, which a click selects; else the one
    /// wearing link dots, while the pointer is on one of them outside it.
    pub(crate) fn link_hover(&self) -> Option<NodeId> {
        if !self.prototype_tab {
            return None;
        }
        self.proto
            .link_hot
            .get()
            .map(|a| a.node)
            .or_else(|| self.proto.knob_hover.get())
    }

    /// Whether a connector under `screen` answers the pointer: with a selecting
    /// tool, no gesture under way, and no link dot or placed overlay's ghost
    /// there, which take a press first.
    fn connector_live(&self, screen: egui::Pos2, rect: egui::Rect, ppp: f32) -> bool {
        self.tool.selects()
            && self.hover_chrome_live()
            && matches!(self.drag, Drag::None)
            && self.link_knob_at(screen, rect, ppp).is_none()
            && !self.ghost_at(screen)
    }

    /// The action whose connector is at `screen`, the nearest within
    /// [`LINK_PICK`] of the connectors drawn last frame.
    pub(crate) fn connector_at(
        &self,
        screen: egui::Pos2,
        rect: egui::Rect,
        ppp: f32,
    ) -> Option<ActionAt> {
        if !self.prototype_tab || !self.connector_live(screen, rect, ppp) {
            return None;
        }
        self.proto
            .links_drawn
            .borrow()
            .iter()
            .map(|(at, pts)| (*at, polyline_distance(pts, screen)))
            .filter(|(_, d)| *d <= LINK_PICK)
            .min_by(|a, b| a.1.total_cmp(&b.1))
            .map(|(at, _)| at)
    }

    /// A click on a connector: select the layer holding its action and open
    /// that action's popover, so it is edited where it lives.
    pub(crate) fn open_link(&mut self, at: ActionAt) {
        if self.session.selection.single() != Some(at.node) {
            self.session.selection.set(vec![at.node]);
        }
        self.proto.open = Some(at);
    }

    /// The connectors from each layer with a navigate or overlay action to its
    /// target, the selection's lit and the rest dim; a dangling one is a red
    /// stub with the warning glyph; each wears a dot where it leaves. Then the
    /// link dots near the pointer, and the link one pulls.
    pub(crate) fn draw_prototype_links(
        &self,
        ui: &egui::Ui,
        painter: &egui::Painter,
        rect: egui::Rect,
        ppp: f32,
    ) {
        if !self.prototype_tab {
            self.proto.ghost_rect.set(None);
            self.proto.link_hot.set(None);
            self.proto.links_drawn.borrow_mut().clear();
            return;
        }
        self.draw_placing(ui, painter, rect, ppp);
        self.draw_flow_chips(ui, painter, rect, ppp);
        let doc = &self.session.doc;
        let page = self.session.page;
        // Each connector's action, ink, line and warning glyph, before any is
        // painted, so the hovered one can be drawn last and heavier.
        let mut drawn: Vec<(Link, egui::Color32, Vec<egui::Pos2>, Option<egui::Pos2>)> = Vec::new();
        for link in links_on(doc, page) {
            let Some(from) = self.selection_quad(link.from, rect, ppp) else {
                continue;
            };
            let lit = self.session.selection.contains(link.from);
            // The selection's blue: full on the selection's own, softer on the rest.
            let blue = match lit {
                true => color::SELECT,
                false => color::SELECT_DIM,
            };
            let here = doc.get(link.to).is_some() && doc.page_of(link.to) == Some(page);
            let ends = here
                .then(|| self.world_quad(link.from).zip(self.world_quad(link.to)))
                .flatten()
                .and_then(|(a, b)| connector_edges(a, b))
                .zip(
                    here.then(|| self.selection_quad(link.to, rect, ppp))
                        .flatten(),
                );
            let (ink, pts, warning) = match ends {
                Some(((ea, eb), to)) => {
                    let (a, out_a) = screen_edge(from, ea);
                    let (b, out_b) = screen_edge(to, eb);
                    (blue, curve(a, out_a, b, out_b), None)
                }
                None => {
                    let frame = screen_of(doc, link.from)
                        .filter(|s| *s != link.from)
                        .and_then(|s| self.world_quad(s))
                        .map(quad_centre);
                    let edge = self
                        .world_quad(link.from)
                        .and_then(|q| stub_edge(q, frame))
                        // Edge 1 runs from the top-right corner down: the right.
                        .unwrap_or(1);
                    let (a, out) = screen_edge(from, edge);
                    let b = a + out * STUB;
                    let (ink, warning) = match doc.get(link.to) {
                        None => (color::error(), Some(b + out * 11.0)),
                        Some(_) => (blue, None),
                    };
                    (ink, vec![a, b], warning)
                }
            };
            drawn.push((link, ink, pts, warning));
        }
        let pointer = ui.ctx().pointer_latest_pos();
        let hot = pointer
            .filter(|p| self.connector_live(*p, rect, ppp))
            .and_then(|p| {
                drawn
                    .iter()
                    .enumerate()
                    .map(|(i, (_, _, pts, _))| (i, polyline_distance(pts, p)))
                    .filter(|(_, d)| *d <= LINK_PICK)
                    .min_by(|a, b| a.1.total_cmp(&b.1))
                    .map(|(i, _)| i)
            });
        let order = (0..drawn.len()).filter(|i| Some(*i) != hot).chain(hot);
        for i in order {
            let (link, ink, pts, warning) = &drawn[i];
            let lit = Some(i) == hot;
            // Under the pointer: the selection's full blue, a step heavier.
            let (ink, w) = match lit {
                true if *ink != color::error() => (color::SELECT, LINK_HOT_W),
                true => (*ink, LINK_HOT_W),
                false => (*ink, LINK_W),
            };
            if let Some(at) = warning {
                painter.text(
                    *at,
                    egui::Align2::CENTER_CENTER,
                    icon::WARNING,
                    theme::icon_font(12.0),
                    ink,
                );
            }
            paint_link(painter, pts.clone(), link.dashed, ink, w);
            // A dot where it leaves, so every connector has a starting point.
            if let Some(a) = pts.first() {
                painter.circle(*a, KNOB_R, color::POINT, egui::Stroke::new(1.5, ink));
            }
        }
        self.proto.link_hot.set(hot.map(|i| drawn[i].0.at));
        *self.proto.links_drawn.borrow_mut() = drawn
            .into_iter()
            .map(|(link, _, pts, _)| (link.at, pts))
            .collect();
        // The link being pulled, from the dot it left, and the screen it would land on.
        let pulled = match self.drag {
            Drag::Link { from, edge } => Some((from, edge)),
            _ => None,
        };
        if let Some((from, edge)) = pulled
            && let Some(p) = pointer
            && let Some(q) = self.selection_quad(from, rect, ppp)
        {
            let (at, out) = screen_edge(q, edge);
            let (b, out_b, target) = self.pulled_end(from, edge, p, rect, ppp);
            if let Some(t) = target {
                painter.add(egui::Shape::closed_line(
                    t.to_vec(),
                    egui::Stroke::new(2.0, color::SELECT),
                ));
            }
            paint_link(
                painter,
                curve(at, out, b, out_b),
                false,
                color::SELECT,
                LINK_W,
            );
        }
        // The dots near the pointer, filled where it would take one or one is
        // pulled, as a shape's dragged corner dot is.
        let hot = pulled.or_else(|| pointer.and_then(|p| self.link_knob_at(p, rect, ppp)));
        for id in self.knob_layers() {
            let Some(q) = self.selection_quad(id, rect, ppp) else {
                continue;
            };
            for (edge, at, _) in knob_dots(q) {
                let near = pointer.is_some_and(|p| edge_distance(q, edge, p) <= KNOB_REVEAL);
                if hot == Some((id, edge)) {
                    painter.circle(
                        at,
                        KNOB_HOT_R,
                        color::SELECT,
                        egui::Stroke::new(1.5, color::POINT),
                    );
                } else if near {
                    painter.circle(
                        at,
                        KNOB_R,
                        color::POINT,
                        egui::Stroke::new(1.5, color::SELECT),
                    );
                }
            }
        }
    }

    /// Where a link pulled from `from`'s `edge` ends with the pointer at `p`,
    /// and the direction it arrives against: over a screen it would land on,
    /// snapped to that screen's edge facing the pulled dot for as long as the
    /// pointer stays inside it (with the screen's quad, to ring); elsewhere,
    /// the pointer itself.
    fn pulled_end(
        &self,
        from: NodeId,
        edge: usize,
        p: egui::Pos2,
        rect: egui::Rect,
        ppp: f32,
    ) -> (egui::Pos2, egui::Vec2, Option<[egui::Pos2; 4]>) {
        let world = self.to_world(p, rect, ppp);
        let snapped = self.link_target(from, world).and_then(|t| {
            let start = edge_mid(self.world_quad(from)?, edge);
            let e = facing_edge(self.world_quad(t)?, |mid| start - mid)?;
            Some((self.selection_quad(t, rect, ppp)?, e))
        });
        match snapped {
            Some((t, e)) => {
                let (b, out_b) = screen_edge(t, e);
                (b, out_b, Some(t))
            }
            None => (p, egui::Vec2::ZERO, None),
        }
    }

    /// `id`'s box in world space, through the preview so it tracks a gesture.
    fn world_quad(&self, id: NodeId) -> Option<WorldQuad> {
        let local = self.session.preview_local_box(id)?;
        let world = self.session.preview_world_transform(id)?;
        Some(
            [
                // `local_quad`'s corners, in its order.
                (local.min_x(), local.min_y()),
                (local.max_x(), local.min_y()),
                (local.max_x(), local.max_y()),
                (local.min_x(), local.max_y()),
            ]
            .map(|(x, y)| world * mastik_core::kurbo::Point::new(x, y)),
        )
    }

    /// The screen a link from `from` dropped at `world` goes to: the top-level
    /// frame under it, unless that is the frame `from` is in.
    fn link_target(&self, from: NodeId, world: mastik_core::kurbo::Point) -> Option<NodeId> {
        let doc = &self.session.doc;
        let under = self.frame_at_world(world)?;
        let screen = screen_of(doc, under)?;
        (screen_of(doc, from) != Some(screen) && screen != from).then_some(screen)
    }

    /// A link dot's drop: one *Navigate to* the screen under the pointer, with
    /// the page's default transition, as one step.
    pub(crate) fn drop_link(&mut self, from: NodeId, world: mastik_core::kurbo::Point) {
        let Some(to) = self.link_target(from, world) else {
            self.session
                .info("Drop the link on another frame to link to it");
            return;
        };
        let session = &mut self.session;
        let list = with_link(&session.doc, from, to, &mut || session.ids.mint_item());
        let target = name(&self.session.doc, to);
        self.write_reactions(from, list, "Add interaction", icon::INTERACTION);
        self.session.info(format!("Linked to “{target}”"));
    }

    /// The overlay being placed by hand: while the Prototype tab is up and the
    /// open popover is an *Open overlay* or *Swap overlay* at *Manual* on a layer
    /// of this page. Its offset is the one being dragged or typed, else the saved.
    pub(crate) fn overlay_placing(&self) -> Option<Placing> {
        if !self.prototype_tab {
            return None;
        }
        let at = self.proto.open?;
        let doc = &self.session.doc;
        if doc.page_of(at.node) != Some(self.session.page) {
            return None;
        }
        let r = doc
            .get(at.node)?
            .reactions()
            .iter()
            .find(|r| r.id == at.reaction)?;
        let (overlay, settings) = match &find(&r.actions, at.action)?.value {
            Action::OpenOverlay {
                overlay, settings, ..
            }
            | Action::SwapOverlay {
                overlay, settings, ..
            } => (*overlay, *settings),
            _ => return None,
        };
        let OverlayPosition::Manual { x, y } = settings.position else {
            return None;
        };
        let frame = mastik_core::prototype::resolve_target(doc, at.node, overlay);
        if !is_frame(doc, frame) {
            return None;
        }
        let offset = match self.drag {
            Drag::OverlayAt { action, now, .. } if action == at => now,
            _ => self
                .proto
                .held_at
                .filter(|(a, _)| *a == at.action)
                .map_or(mastik_core::kurbo::Vec2::new(x, y), |(_, v)| v),
        };
        Some(Placing { at, frame, offset })
    }

    /// Where the overlay being placed sits in world space: its offset from the
    /// top-left of the opening layer's box, at the frame's drawn size.
    pub(crate) fn placing_rect(&self, p: &Placing) -> Option<mastik_core::kurbo::Rect> {
        let q = self.world_quad(p.at.node)?;
        let corner = q.iter().fold(q[0], |m, c| {
            mastik_core::kurbo::Point::new(m.x.min(c.x), m.y.min(c.y))
        });
        let size = self
            .session
            .resolved
            .used_kind(&self.session.doc, p.frame)?
            .box_size()?;
        Some(mastik_core::kurbo::Rect::from_origin_size(
            corner + p.offset,
            size,
        ))
    }

    /// The overlay being placed, drawn as itself at half strength over the page
    /// where it would open: the chrome ghost `draw_canvas` installs each frame.
    pub(crate) fn overlay_ghost(&self) -> Option<(NodeId, mastik_render::Ghost)> {
        let p = self.overlay_placing()?;
        let r = self.placing_rect(&p)?;
        let doc = &self.session.doc;
        let page = self.session.page;
        let mut node =
            mastik_render::ghost_as_drawn(doc, &self.session.resolved, p.frame, PLACING_ACTOR)?;
        node.transform = mastik_core::kurbo::Affine::translate(r.origin().to_vec2());
        node.opacity *= PLACING_ALPHA;
        Some((
            p.frame,
            mastik_render::Ghost {
                parent: page,
                index: doc.get(page)?.children().len(),
                node,
            },
        ))
    }

    /// Whether a press at `screen` takes the placed overlay's ghost, as it was
    /// drawn last frame.
    pub(crate) fn ghost_at(&self, screen: egui::Pos2) -> bool {
        self.prototype_tab
            && self
                .proto
                .ghost_rect
                .get()
                .is_some_and(|r| r.contains(screen))
            && self.overlay_placing().is_some()
    }

    /// The ghost's offset for a drag from `grab` to `world` that began at `from`,
    /// snapped as a moved layer's box is (Shift keeps it to one axis), and the
    /// marks that snap draws. Whole points, as the offset is stored.
    pub(crate) fn snapped_overlay(
        &self,
        from: mastik_core::kurbo::Vec2,
        grab: mastik_core::kurbo::Point,
        world: mastik_core::kurbo::Point,
        shift: bool,
    ) -> (mastik_core::kurbo::Vec2, crate::snap::Marks) {
        let raw = world - grab;
        let axes = crate::snap::move_axes(raw, shift);
        let delta = crate::snap::constrain(raw, axes);
        let whole =
            |v: mastik_core::kurbo::Vec2| mastik_core::kurbo::Vec2::new(v.x.round(), v.y.round());
        let Some(base) = self
            .overlay_placing()
            .and_then(|p| self.placing_rect(&Placing { offset: from, ..p }))
        else {
            return (whole(from + delta), crate::snap::Marks::default());
        };
        let snapped = crate::snap::snap_rect(base + delta, axes, &self.snapping(&[]));
        (whole(from + delta + snapped.adjust), snapped.marks)
    }

    /// The ghost's drop: the offset it was dragged to, in whole points, written
    /// as the action's manual position, one *Overlay* step.
    pub(crate) fn drop_overlay(&mut self, at: ActionAt, now: mastik_core::kurbo::Vec2) {
        let doc = &self.session.doc;
        let Some(n) = doc.get(at.node) else { return };
        let mut list = n.reactions().to_vec();
        let Some(r) = list.iter_mut().find(|r| r.id == at.reaction) else {
            return;
        };
        let Some(k) = find_mut(&mut r.value.actions, at.action) else {
            return;
        };
        let (Action::OpenOverlay { settings, .. } | Action::SwapOverlay { settings, .. }) =
            &mut k.value
        else {
            return;
        };
        let position = OverlayPosition::Manual {
            x: now.x.round(),
            y: now.y.round(),
        };
        if settings.position == position {
            return;
        }
        settings.position = position;
        self.write_reactions(at.node, list, "Overlay", icon::OVERLAY_OPEN);
    }

    /// The placed overlay's ring, over its ghost: the selection's blue, heavier
    /// under the pointer or while dragged. Notes the ghost's screen box for the
    /// next frame's press.
    fn draw_placing(&self, ui: &egui::Ui, painter: &egui::Painter, rect: egui::Rect, ppp: f32) {
        let shown = self.overlay_placing().and_then(|p| {
            let r = self.placing_rect(&p)?;
            let a = self.to_screen(r.origin(), rect, ppp);
            let b = self.to_screen(mastik_core::kurbo::Point::new(r.x1, r.y1), rect, ppp);
            Some(egui::Rect::from_two_pos(a, b))
        });
        self.proto.ghost_rect.set(shown);
        let Some(r) = shown else { return };
        let hot = matches!(self.drag, Drag::OverlayAt { .. })
            || ui.ctx().pointer_hover_pos().is_some_and(|p| r.contains(p));
        let w = if hot { 2.0 } else { 1.0 };
        painter.rect_stroke(
            r,
            0.0,
            egui::Stroke::new(w, color::SELECT),
            egui::StrokeKind::Outside,
        );
    }
}

/// The actor a placed overlay's ghost ids are minted under: ids no document
/// holds, read only by the scene walk.
const PLACING_ACTOR: u64 = 0x0FE4;
/// How strongly the placed overlay's ghost draws.
const PLACING_ALPHA: f32 = 0.5;

/// A manual overlay being placed on the canvas: the action placing it, the frame
/// it opens, and its offset from the opening layer's top-left.
#[derive(Clone, Copy, Debug)]
pub(crate) struct Placing {
    pub at: ActionAt,
    pub frame: NodeId,
    pub offset: mastik_core::kurbo::Vec2,
}

// --- the tab ------------------------------------------------------------------

/// The popover's width.
const POP_W: f32 = crate::app::POPOVER_W;
/// Between two controls on one line.
const COL: f32 = ui::CARD_COL_GAP;
/// A nested action's indent under its branch head.
const NEST: f32 = 16.0;

impl MastikApp {
    /// Show or put away the Prototype tab, and say so.
    pub(crate) fn set_prototype_tab(&mut self, on: bool) {
        if self.prototype_tab != on {
            self.close_inspector_popovers();
            self.proto.capture = None;
        }
        self.prototype_tab = on;
        self.session.info(match on {
            true => "Prototype tab",
            false => "Design tab",
        });
    }

    /// The inspector's head: *Design* | *Prototype*, a segmented control on a card
    /// of its own over every other card.
    pub(super) fn inspector_tabs(&mut self, ui: &mut egui::Ui) {
        let mut pick = None;
        ui.push_id("inspector-tabs", |ui| {
            ui::card(ui, |ui| {
                let w = ui.available_width();
                pick = ui::segmented_tipped(
                    ui,
                    w,
                    ui::SEGMENT_CELL_H,
                    2,
                    usize::from(self.prototype_tab),
                    |_| true,
                    |i| match i {
                        0 => "The layer's look and layout",
                        _ => "How the layer behaves when the prototype plays",
                    },
                    |p, i, r, on| ui::segment_label(p, r, ["Design", "Prototype"][i], on),
                );
            });
        });
        if let Some(i) = pick {
            let on = i == 1;
            if on != self.prototype_tab {
                self.close_inspector_popovers();
                self.proto.capture = None;
                self.prototype_tab = on;
            }
        }
    }

    /// The Prototype tab's cards for what is selected.
    pub(super) fn prototype_inspector(&mut self, ui: &mut egui::Ui) {
        if self
            .proto
            .open
            .is_some_and(|o| !self.session.selection.contains(o.node))
        {
            self.proto.open = None;
        }
        if self
            .proto
            .capture
            .is_some_and(|(n, _)| !self.session.selection.contains(n))
        {
            self.proto.capture = None;
        }
        match self.session.selection.single() {
            Some(id) if self.session.doc.get(id).is_some() => {
                self.proto_identity(ui, id);
                self.interactions_card(ui, id);
                self.layer_play_card(ui, id);
            }
            _ if self.session.selection.is_empty() => {
                self.page_header(ui);
                self.flows_card(ui);
                self.page_play_card(ui);
            }
            _ => {
                identity_card(ui, |ui| {
                    ui::selection_note(ui, "Prototype one layer at a time");
                });
            }
        }
    }

    /// The layer's identity in the Prototype tab: its glyph and name, and what it
    /// is, without the Design tab's rows.
    fn proto_identity(&mut self, ui: &mut egui::Ui, id: NodeId) {
        let doc = &self.session.doc;
        let Some(n) = doc.get(id) else { return };
        let glyph = super::layers_icon(n).0;
        let title = name(doc, id);
        let note = match n.kind() {
            NodeKind::Artboard { size, .. } if screen_of(doc, id) == Some(id) => {
                format!("{} × {}", size.width.round(), size.height.round())
            }
            _ if n.link().is_some()
                && mastik_core::component::instance_root(doc, id) == Some(id) =>
            {
                "Instance".into()
            }
            _ => String::new(),
        };
        identity_card(ui, |ui| {
            ui.horizontal(|ui| {
                ui.spacing_mut().item_spacing.x = 9.0;
                identity_glyph(ui, glyph, color::accent_300());
                ui.label(
                    egui::RichText::new(title)
                        .size(ui::HEAD_PT)
                        .color(theme::text::strong()),
                );
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    ui.label(
                        egui::RichText::new(note)
                            .size(ui::CAPTION_PT)
                            .color(theme::text::faint()),
                    );
                });
            });
        });
    }

    /// Write a layer's reactions as one step.
    pub(crate) fn write_reactions(
        &mut self,
        id: NodeId,
        reactions: Vec<Keyed<Reaction>>,
        label: &'static str,
        glyph: &'static str,
    ) {
        if let Err(why) = mastik_core::prototype::check(&reactions) {
            self.session.info(format!("Not changed: {why}"));
            return;
        }
        self.session.name_step(label, glyph);
        self.commit_edit(Transaction(vec![Operation::SetReactions { id, reactions }]));
    }

    /// The *Interactions* card: one block per reaction, its trigger at its head
    /// and its actions in order under it.
    fn interactions_card(&mut self, ui: &mut egui::Ui, id: NodeId) {
        let reactions = self
            .session
            .doc
            .get(id)
            .map(|n| n.reactions().to_vec())
            .unwrap_or_default();
        let source = self.source_list(id, |n| n.reactions());
        let mut next: Option<(Vec<Keyed<Reaction>>, &'static str, &'static str)> = None;
        let mut restore: Option<Vec<Keyed<Reaction>>> = None;
        let mut reset_one: Option<ItemId> = None;
        let add = self.panel(
            ui,
            "Interactions",
            Some(ui::HeadAction::new(icon::PLUS, "Add an interaction")),
            |app, ui| {
                let ghosts = source
                    .as_deref()
                    .map(|s| mastik_core::reset::removed_items(s, &reactions))
                    .unwrap_or_default();
                if reactions.is_empty() && ghosts.is_empty() {
                    ui::empty_note(ui, "No interactions");
                }
                let states = source
                    .as_ref()
                    .map(|s| mastik_core::reset::item_states(s, &reactions));
                for (i, r) in reactions.iter().enumerate() {
                    // A rule between one interaction's actions and the next one's trigger.
                    if i > 0 {
                        ui::card_rule(ui);
                    }
                    let state = states.as_ref().map(|s| s[i]);
                    let out = app.reaction_block(ui, id, &reactions, r, state);
                    if let Some(o) = out.write {
                        next = Some(o);
                    }
                    if out.reset {
                        reset_one = Some(r.id);
                    }
                }
                for gone in ghosts {
                    let label = trigger_label(&app.session.doc, &gone.trigger);
                    let lead = super::component::GhostLead::Glyph(icon::INTERACTION);
                    match super::component::ghost_row(ui, lead, &label) {
                        Some(ui::ResetOrPush::Reset) => {
                            restore = source.clone();
                        }
                        Some(ui::ResetOrPush::Push) => {
                            let list = Operation::SetReactions {
                                id,
                                reactions: Vec::new(),
                            };
                            app.push_item_removal(id, &list, gone.id);
                        }
                        None => {}
                    }
                }
            },
        );
        if add {
            let mut list = reactions.clone();
            list.push(Keyed::new(
                self.session.ids.mint_item(),
                Reaction {
                    trigger: Trigger::Click,
                    actions: Vec::new(),
                },
            ));
            self.write_reactions(id, list, "Add interaction", icon::INTERACTION);
            return;
        }
        if let (Some(item), Some(src)) = (reset_one, source.as_deref()) {
            let list = mastik_core::reset::reset_item(src, &reactions, item);
            self.session.name_step("Interactions", icon::INTERACTION);
            self.commit_reset(Transaction(vec![Operation::SetReactions {
                id,
                reactions: list,
            }]));
            return;
        }
        if let Some(list) = restore {
            self.session.name_step("Interactions", icon::INTERACTION);
            self.commit_edit(Transaction(vec![Operation::SetReactions {
                id,
                reactions: list,
            }]));
            return;
        }
        if let Some((list, label, glyph)) = next {
            self.write_reactions(id, list, label, glyph);
        }
    }

    /// One reaction's block: its trigger's row, then its actions.
    fn reaction_block(
        &mut self,
        ui: &mut egui::Ui,
        id: NodeId,
        all: &[Keyed<Reaction>],
        r: &Keyed<Reaction>,
        state: Option<mastik_core::reset::ItemState>,
    ) -> BlockOut {
        let mut out = BlockOut::default();
        let with = |f: &dyn Fn(&mut Reaction)| -> Vec<Keyed<Reaction>> {
            all.iter()
                .map(|k| match k.id == r.id {
                    true => k.map(|v| {
                        let mut v = v.clone();
                        f(&mut v);
                        v
                    }),
                    false => k.clone(),
                })
                .collect()
        };
        let h = ui::CONTROL_H;
        let capturing = self.proto.capture == Some((id, r.id));
        let mut add_place: Option<(Place, ActionKind)> = None;
        // --- the head: trigger, `+`, `⋯` ------------------------------------
        ui.horizontal(|ui| {
            ui.spacing_mut().item_spacing.x = COL;
            let slot = if state.is_some() { 14.0 + COL } else { 0.0 };
            let w = ui.available_width() - 2.0 * (h + COL) - slot;
            if capturing {
                if let Some(t) = self.key_capture(ui, w) {
                    out.write = Some((
                        with(&|v: &mut Reaction| v.trigger = t.clone()),
                        "Change trigger",
                        icon::KEY,
                    ));
                }
            } else if let Some(t) = self.trigger_combo(ui, id, r, w) {
                match t {
                    TriggerPick::Set(t) => {
                        // A script refused under the new trigger goes with it.
                        let keeps = r.actions.iter().all(|a| {
                            !matches!(a.value, Action::RunScript { .. }) || t.is_a_persons_act()
                        });
                        if keeps {
                            out.write = Some((
                                with(&|v: &mut Reaction| v.trigger = t.clone()),
                                "Change trigger",
                                icon::INTERACTION,
                            ));
                        } else {
                            self.session.info(
                                "A script runs only when someone acts: remove Run script first",
                            );
                        }
                    }
                    TriggerPick::Key => self.proto.capture = Some((id, r.id)),
                }
            }
            let plus = ui::field_button(ui, icon::PLUS, h, FieldButton::Off)
                .on_hover_text("Add an action");
            if let Some(k) = self.add_action_menu(&plus, id, &r.trigger) {
                add_place = Some((Place::Top, k));
            }
            let more =
                ui::field_button(ui, icon::DOTS_THREE, h, FieldButton::Off).on_hover_text("More");
            let mut dup = false;
            let mut delete = false;
            egui::Popup::menu(&more)
                .align(egui::RectAlign::BOTTOM_END)
                .gap(ui::MENU_GAP)
                .show(|ui| {
                    ui::popup_rows(ui);
                    let w = ui::menu_width(ui.ctx(), ["Duplicate", "Delete"], true);
                    ui.set_width(w);
                    dup = ui::menu_item(ui, icon::COPY, "Duplicate", ui::MENU_ROW_H).clicked();
                    ui::menu_sep(ui);
                    delete = ui::menu_row(
                        ui,
                        ui::MenuRow::new(icon::TRASH, "Delete").danger(true),
                        ui::MENU_ROW_H,
                    )
                    .clicked();
                });
            if dup {
                let mut mint = || self.session.ids.mint_item();
                let copy = Reaction {
                    trigger: r.trigger.clone(),
                    actions: fresh_ids(&r.actions, &mut mint),
                };
                let at = all
                    .iter()
                    .position(|k| k.id == r.id)
                    .map_or(all.len(), |i| i + 1);
                let mut list = all.to_vec();
                list.insert(at, Keyed::new(mint(), copy));
                out.write = Some((list, "Duplicate interaction", icon::COPY));
            }
            if delete {
                let list = all.iter().filter(|k| k.id != r.id).cloned().collect();
                out.write = Some((list, "Remove interaction", icon::TRASH));
            }
            if let Some(state) = state {
                let hot = ui.rect_contains_pointer(ui.min_rect());
                match super::component::item_slot_menu(ui, state, hot) {
                    Some(ui::ResetOrPush::Reset | ui::ResetOrPush::Push) => out.reset = true,
                    None => {}
                }
            }
        });
        // --- what the trigger needs: a delay, a variable ----------------------
        match &r.trigger {
            Trigger::After { ms } => {
                let mut v = f64::from(*ms);
                let resp_id = ui.id().with(("after", r.id));
                let held = self
                    .proto
                    .scrub
                    .filter(|(i, _)| *i == resp_id)
                    .map(|(_, v)| v);
                if let Some(x) = held {
                    v = x;
                }
                let resp = ui::value_field_suffixed(
                    ui,
                    egui::vec2(ui.available_width(), h),
                    Prefix::Text("Delay"),
                    Some(ui::Suffix {
                        text: "ms",
                        clickable: false,
                        tooltip: "",
                    }),
                    &mut v,
                    Scrub::whole(10.0).range(0.0..=600_000.0),
                    |d| d.custom_formatter(ui::number(0)),
                )
                .0
                .on_hover_text("How long after the frame opens the actions run");
                let ms_new = v.round().clamp(0.0, 600_000.0) as u32;
                let tx = match ms_new != *ms {
                    true => Transaction(vec![Operation::SetReactions {
                        id,
                        reactions: with(&|x: &mut Reaction| {
                            x.trigger = Trigger::After { ms: ms_new }
                        }),
                    }]),
                    false => Transaction::default(),
                };
                self.proto_valve(resp_id, v, &resp, "Change trigger", icon::INTERACTION, tx);
            }
            Trigger::VariableChanges { var } => {
                if let Some(v) = self.variable_combo(ui, ("when-var", r.id), Some(*var), None) {
                    out.write = Some((
                        with(&|x: &mut Reaction| x.trigger = Trigger::VariableChanges { var: v }),
                        "Change trigger",
                        icon::BRACKETS_CURLY,
                    ));
                }
            }
            _ => {}
        }
        // --- the actions -----------------------------------------------------
        let mut list_out = ListOut::default();
        self.action_rows(ui, id, r, &r.actions, 0.0, &mut list_out);
        if let Some(place) = list_out.add {
            add_place = Some(place);
        }
        if let Some((place, kind)) = add_place {
            match new_action(&self.session.doc, id, kind) {
                Some(a) => {
                    let item = self.session.ids.mint_item();
                    let mut v = r.value.clone();
                    if let Some(list) = list_at(&mut v.actions, place) {
                        list.push(Keyed::new(item, a));
                        out.write = Some((
                            with(&|x: &mut Reaction| *x = v.clone()),
                            "Add action",
                            kind.glyph(),
                        ));
                        // *Back* has nothing to set, so no popover opens for it.
                        if kind != ActionKind::Back {
                            self.proto.open = Some(ActionAt {
                                node: id,
                                reaction: r.id,
                                action: item,
                            });
                        }
                    }
                }
                None => self
                    .session
                    .info("Nothing in the document for that action to name"),
            }
        }
        if let Some((moved, to)) = list_out.reorder {
            let mut v = r.value.clone();
            if move_to(&mut v.actions, moved, to) {
                out.write = Some((
                    with(&|x: &mut Reaction| *x = v.clone()),
                    "Move action",
                    icon::DOTS_SIX_VERTICAL,
                ));
            }
        }
        if let Some(gone) = list_out.remove {
            let mut v = r.value.clone();
            remove(&mut v.actions, gone);
            out.write = Some((
                with(&|x: &mut Reaction| *x = v.clone()),
                "Remove action",
                icon::TRASH,
            ));
            if self.proto.open.is_some_and(|o| o.action == gone) {
                self.proto.open = None;
            }
        }
        if let (Some(head), Some(open)) = (list_out.head, self.proto.open)
            && open.node == id
            && open.reaction == r.id
            && let Some(edit) = self.action_popover(ui, &head, id, r, open.action)
        {
            out.write = Some((
                with(&|x: &mut Reaction| *x = edit.0.clone()),
                edit.1,
                edit.2,
            ));
        }
        out
    }

    /// The trigger's dropdown, the pointer's triggers first. *When the frame
    /// opens* is offered on a top-level frame only.
    fn trigger_combo(
        &mut self,
        ui: &mut egui::Ui,
        id: NodeId,
        r: &Keyed<Reaction>,
        w: f32,
    ) -> Option<TriggerPick> {
        let doc = &self.session.doc;
        let label = trigger_label(doc, &r.trigger);
        let a_screen = screen_of(doc, id) == Some(id);
        let first_var = doc.variables().iter().next().map(|v| v.id);
        let mut pick = None;
        let current = r.trigger.clone();
        ui.scope(|ui| {
            ui::combo_height(ui);
            egui::ComboBox::from_id_salt(("trigger", r.id))
                .icon(ui::combo_chevron)
                .width(w)
                .selected_text(ui::glyph_and_text(icon::INTERACTION, &label))
                .show_ui(ui, |ui| {
                    ui::menu_rows(ui);
                    ui::menu_eyebrow(ui, "Pointer");
                    for t in POINTER_TRIGGERS {
                        let on = same_trigger_kind(&t, &current);
                        if ui.selectable_label(on, trigger_label(doc, &t)).clicked() && !on {
                            pick = Some(TriggerPick::Set(t));
                        }
                    }
                    ui::menu_sep(ui);
                    ui::menu_eyebrow(ui, "Other");
                    let on = matches!(current, Trigger::Key { .. });
                    if ui.selectable_label(on, "Key…").clicked() {
                        pick = Some(TriggerPick::Key);
                    }
                    let on = matches!(current, Trigger::After { .. });
                    if ui.selectable_label(on, "After delay").clicked() && !on {
                        pick = Some(TriggerPick::Set(Trigger::After { ms: 1000 }));
                    }
                    let on = matches!(current, Trigger::VariableChanges { .. });
                    let row = ui.add_enabled_ui(first_var.is_some(), |ui| {
                        ui.selectable_label(on, "When a variable changes")
                    });
                    let row = row
                        .inner
                        .on_disabled_hover_text("This document has no variables");
                    if row.clicked()
                        && !on
                        && let Some(var) = first_var
                    {
                        pick = Some(TriggerPick::Set(Trigger::VariableChanges { var }));
                    }
                    let on = matches!(current, Trigger::FrameOpens);
                    let row = ui.add_enabled_ui(a_screen, |ui| {
                        ui.selectable_label(on, "When the frame opens")
                    });
                    let row = row.inner.on_disabled_hover_text("Only a frame opens");
                    if row.clicked() && !on {
                        pick = Some(TriggerPick::Set(Trigger::FrameOpens));
                    }
                });
        });
        pick
    }

    /// The *Key…* field: the next plain or Shift-ed key pressed is the trigger.
    /// `Escape` gives up; a chord with Ctrl or Alt is refused, those staying the
    /// editor's while playing.
    fn key_capture(&mut self, ui: &mut egui::Ui, w: f32) -> Option<Trigger> {
        let mut empty = String::new();
        let resp = ui::text_field(ui, egui::vec2(w, ui::CONTROL_H), &mut empty, "Press a key");
        if !resp.has_focus() && !resp.lost_focus() {
            resp.request_focus();
        }
        if resp.lost_focus() {
            self.proto.capture = None;
            return None;
        }
        let pressed = ui.input(|i| {
            i.events.iter().find_map(|e| match e {
                egui::Event::Key {
                    key,
                    pressed: true,
                    modifiers,
                    ..
                } => Some((*key, *modifiers)),
                _ => None,
            })
        });
        let (key, mods) = pressed?;
        if key == egui::Key::Escape {
            self.proto.capture = None;
            return None;
        }
        if mods.ctrl || mods.alt || mods.command || mods.mac_cmd {
            self.session.info(
                "A key trigger takes a plain or Shift-ed key: Ctrl and Alt stay the editor's",
            );
            return None;
        }
        self.proto.capture = None;
        ui.memory_mut(|m| m.surrender_focus(resp.id));
        Some(Trigger::Key {
            key: key.name().to_string(),
            shift: mods.shift,
        })
    }

    /// The add-action menu under `button`: the kinds in their groups, each one the
    /// layer cannot take dimmed with its reason.
    fn add_action_menu(
        &mut self,
        button: &egui::Response,
        id: NodeId,
        trigger: &Trigger,
    ) -> Option<ActionKind> {
        let doc = &self.session.doc;
        let mut picked = None;
        egui::Popup::menu(button)
            .align(egui::RectAlign::BOTTOM_END)
            .gap(ui::MENU_GAP)
            .show(|ui| {
                ui::popup_rows(ui);
                let labels = ActionKind::GROUPS
                    .iter()
                    .flat_map(|(_, kinds)| kinds.iter().map(|k| k.label()));
                ui.set_width(ui::menu_width(ui.ctx(), labels, true));
                for (i, (group, kinds)) in ActionKind::GROUPS.iter().enumerate() {
                    if i > 0 {
                        ui::menu_sep(ui);
                    }
                    ui::menu_eyebrow(ui, group);
                    for kind in kinds.iter() {
                        let why = action_refusal(doc, id, trigger, *kind);
                        let row = ui::menu_row(
                            ui,
                            ui::MenuRow::new(kind.glyph(), kind.label()).enabled(why.is_none()),
                            ui::MENU_ROW_H,
                        );
                        match why {
                            Some(why) => {
                                row.on_disabled_hover_text(why);
                            }
                            None if row.clicked() => picked = Some(*kind),
                            None => {}
                        }
                    }
                }
            });
        picked
    }

    /// A list of actions, each a row, a conditional's branches nested under it.
    fn action_rows(
        &mut self,
        ui: &mut egui::Ui,
        id: NodeId,
        r: &Keyed<Reaction>,
        list: &[Keyed<Action>],
        indent: f32,
        out: &mut ListOut,
    ) {
        // Each row's rect, for where a dragged one would land among them.
        let mut rows: Vec<(ItemId, egui::Rect)> = Vec::new();
        for k in list {
            let open = self.proto.open
                == Some(ActionAt {
                    node: id,
                    reaction: r.id,
                    action: k.id,
                });
            let (row, resp) = self.action_row(ui, id, k, indent, open, list.len() > 1);
            rows.push((k.id, resp.rect));
            if row.grabbed {
                self.proto.dragging = Some(ActionAt {
                    node: id,
                    reaction: r.id,
                    action: k.id,
                });
            }
            if row.open {
                self.proto.open = match open {
                    true => None,
                    false => Some(ActionAt {
                        node: id,
                        reaction: r.id,
                        action: k.id,
                    }),
                };
            }
            if row.remove {
                out.remove = Some(k.id);
            }
            if open {
                out.head = Some(resp);
            }
            if let Action::Conditional {
                branches,
                otherwise,
            } = &k.value
            {
                for (i, b) in branches.iter().enumerate() {
                    let word = if i == 0 { "if" } else { "else if" };
                    let text = format!("{word} {}", expr_shown(&self.session.doc, &b.when));
                    if let Some(kind) =
                        self.branch_head(ui, id, &r.trigger, (k.id, i), &text, indent + NEST)
                    {
                        out.add = Some((Place::Branch(k.id, i), kind));
                    }
                    self.action_rows(ui, id, r, &b.actions, indent + NEST, out);
                }
                if let Some(kind) = self.branch_head(
                    ui,
                    id,
                    &r.trigger,
                    (k.id, usize::MAX),
                    "else",
                    indent + NEST,
                ) {
                    out.add = Some((Place::Otherwise(k.id), kind));
                }
                self.action_rows(ui, id, r, otherwise, indent + NEST, out);
            }
        }
        // A drag held by one of this list's rows: the line where it would land,
        // and the move once it is let go.
        let Some(d) = self.proto.dragging else {
            return;
        };
        if d.node != id || d.reaction != r.id || !rows.iter().any(|(a, _)| *a == d.action) {
            return;
        }
        let (pointer, down) = ui.input(|i| (i.pointer.interact_pos(), i.pointer.primary_down()));
        let Some(p) = pointer else {
            return;
        };
        let others: Vec<&(ItemId, egui::Rect)> =
            rows.iter().filter(|(a, _)| *a != d.action).collect();
        let to = others.iter().filter(|(_, rr)| rr.center().y < p.y).count();
        if down {
            ui.ctx().set_cursor_icon(egui::CursorIcon::Grabbing);
            let y = match others.get(to) {
                Some((_, rr)) => rr.top() - 1.0,
                None => others.last().map_or(p.y, |(_, rr)| rr.bottom() + 1.0),
            };
            let x = rows
                .first()
                .map_or(ui.max_rect().x_range(), |(_, rr)| rr.x_range());
            ui.painter()
                .hline(x, y, egui::Stroke::new(2.0, color::accent()));
        } else {
            self.proto.dragging = None;
            out.reorder = Some((d.action, to));
        }
    }

    /// A branch's head under its conditional: its words, and a `+` adding an
    /// action to that branch.
    fn branch_head(
        &mut self,
        ui: &mut egui::Ui,
        id: NodeId,
        trigger: &Trigger,
        salt: (ItemId, usize),
        text: &str,
        indent: f32,
    ) -> Option<ActionKind> {
        let mut picked = None;
        ui.horizontal(|ui| {
            ui.spacing_mut().item_spacing.x = COL;
            ui.add_space(indent);
            let w = ui.available_width() - ui::MENU_ROW_H - COL;
            ui.allocate_ui_with_layout(
                egui::vec2(w, ui::MENU_ROW_H),
                egui::Layout::left_to_right(egui::Align::Center),
                |ui| {
                    ui.set_width(w);
                    ui.add(
                        egui::Label::new(
                            egui::RichText::new(text)
                                .family(egui::FontFamily::Monospace)
                                .size(ui::CAPTION_PT)
                                .color(color::accent_300()),
                        )
                        .truncate(),
                    );
                },
            );
            let plus = ui
                .push_id(salt, |ui| {
                    ui::icon_button(ui, icon::PLUS, ui::MENU_ROW_H, 13.0, false, true)
                })
                .inner
                .on_hover_text("Add an action to this branch");
            picked = self.add_action_menu(&plus, id, trigger);
        });
        picked
    }

    /// One action's row: its glyph, its verb, its target in accent ink (red and
    /// marked when it is gone), its timing as a chip, and `×`. A click opens its
    /// popover.
    fn action_row(
        &mut self,
        ui: &mut egui::Ui,
        id: NodeId,
        k: &Keyed<Action>,
        indent: f32,
        open: bool,
        reorderable: bool,
    ) -> (RowOut, egui::Response) {
        let doc = &self.session.doc;
        let kind = ActionKind::of(&k.value);
        let (target, missing) = target_words(doc, id, &k.value);
        let chip = transition_of(&k.value).map(|t| transition_chip(&t));
        let verb = match &k.value {
            Action::SetVariable { .. } => "Set",
            Action::Conditional { .. } => "If",
            _ => kind.label(),
        };
        let mut out = RowOut::default();
        let resp = ui
            .horizontal(|ui| {
                ui.spacing_mut().item_spacing.x = COL;
                ui.add_space(indent);
                let size = egui::vec2(ui.available_width(), ui::CONTROL_H);
                // The grip and × sit over the row's press and keep their own.
                let (_, resp) =
                    ui::field_row_pressable(ui, size, open, true, |ui, _| {
                        ui.spacing_mut().item_spacing.x = COL;
                        let g = ui::grip(ui, reorderable, size.y);
                        out.grabbed = g.drag_started();
                        let ink = match missing {
                            Some(_) => color::error(),
                            None => theme::text::muted(),
                        };
                        let glyph = if missing.is_some() {
                            icon::WARNING
                        } else {
                            kind.glyph()
                        };
                        ui.label(theme::icon_text(glyph, 14.0, ink));
                        ui.label(egui::RichText::new(verb).size(ui::SMALL_PT).color(
                            match missing {
                                Some(_) => color::error(),
                                None => theme::text::strong(),
                            },
                        ));
                        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                            if ui::icon_button(
                                ui,
                                icon::X,
                                ui::CLOSE_BUTTON_W,
                                ui::CLOSE_GLYPH_PT,
                                false,
                                true,
                            )
                            .on_hover_text("Remove")
                            .clicked()
                            {
                                out.remove = true;
                            }
                            // The timing as a faint note, as a menu row's accelerator.
                            if let Some(chip) = &chip {
                                ui.label(
                                    egui::RichText::new(chip)
                                        .size(ui::CAPTION_PT)
                                        .color(theme::text::faint()),
                                );
                            }
                            let ink = match missing {
                                Some(_) => color::error(),
                                None => color::accent_200(),
                            };
                            ui.add(
                                egui::Label::new(
                                    egui::RichText::new(&target).size(ui::SMALL_PT).color(ink),
                                )
                                .truncate(),
                            );
                        });
                    });
                let tip = match missing {
                    Some(gone) => format!(
                        "{gone}: skipped while playing. Pick another target, or undo the delete"
                    ),
                    None => format!("Edit {}", kind.label().to_lowercase()),
                };
                let resp = resp.on_hover_text(tip);
                if resp.clicked() {
                    out.open = true;
                }
                resp
            })
            .inner;
        (out, resp)
    }

    /// An action's popover, beside its row in the popover lane: what it names,
    /// then its transition. Answers the reaction rewritten, with the step's name.
    fn action_popover(
        &mut self,
        ui: &mut egui::Ui,
        head: &egui::Response,
        id: NodeId,
        r: &Keyed<Reaction>,
        action: ItemId,
    ) -> Option<(Reaction, &'static str, &'static str)> {
        let k = find(&r.actions, action)?.clone();
        let ctx = ui.ctx().clone();
        let over_overlay = ctx
            .pointer_interact_pos()
            .and_then(|p| ctx.layer_id_at(p))
            .is_some_and(|l| l.order >= egui::Order::Foreground);
        let anchor = Self::popover_anchor(&ctx, head.rect, POP_W);
        let mut close = false;
        let mut edit: Option<(Action, &'static str, &'static str)> = None;
        let mut moved: Option<bool> = None;
        let kind = ActionKind::of(&k.value);
        let place = position(&r.actions, action);
        let menu = egui::Area::new(egui::Id::new(("proto-action", action)))
            .order(egui::Order::Foreground)
            .fixed_pos(anchor)
            .constrain_to(ctx.input(|i| i.content_rect()).shrink(ui::SCREEN_MARGIN))
            .show(&ctx, |ui| {
                ui::hold_tab(ui);
                ui::menu_frame(ui::POPOVER_PAD).show(ui, |ui| {
                    let inner = ui::menu_inner_w(POP_W, ui::POPOVER_PAD);
                    ui.set_width(inner);
                    ui.spacing_mut().item_spacing.y = ui::POPOVER_SECTION_GAP;
                    // --- head: the verb, moves, close ---------------------------
                    ui.horizontal(|ui| {
                        ui.spacing_mut().item_spacing.x = 4.0;
                        ui.label(theme::icon_text(
                            kind.glyph(),
                            ui::CONTROL_GLYPH_PT,
                            theme::text::muted(),
                        ));
                        ui.label(ui::eyebrow(kind.label()));
                        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                            if ui::close_button(ui).clicked() {
                                close = true;
                            }
                            let (i, n) = place.unwrap_or((0, 1));
                            let down = ui.add_enabled_ui(i + 1 < n, |ui| {
                                ui::icon_button(
                                    ui,
                                    icon::ARROW_DOWN,
                                    ui::CLOSE_BUTTON_W,
                                    ui::CLOSE_GLYPH_PT,
                                    false,
                                    true,
                                )
                            });
                            if down.inner.on_hover_text("Move down").clicked() {
                                moved = Some(false);
                            }
                            let up = ui.add_enabled_ui(i > 0, |ui| {
                                // The ↑ glyph sits a point low in its box: raised
                                // a point to line up with ↓ and ×.
                                let (slot, _) = ui.allocate_exact_size(
                                    egui::Vec2::splat(ui::CLOSE_BUTTON_W),
                                    egui::Sense::hover(),
                                );
                                ui.put(
                                    slot.translate(egui::vec2(0.0, -1.0)),
                                    |ui: &mut egui::Ui| {
                                        ui::icon_button(
                                            ui,
                                            icon::ARROW_UP,
                                            ui::CLOSE_BUTTON_W,
                                            ui::CLOSE_GLYPH_PT,
                                            false,
                                            true,
                                        )
                                    },
                                )
                            });
                            if up.inner.on_hover_text("Move up").clicked() {
                                moved = Some(true);
                            }
                        });
                    });
                    if let Some(e) = self.action_fields(ui, id, &r.trigger, &k, inner) {
                        edit = Some(e);
                    }
                    if let Some(t) = transition_of(&k.value) {
                        ui::card_rule(ui);
                        if let Some(t2) = self.transition_fields(ui, ("tr", action), t, inner) {
                            edit = Some((
                                with_transition(&k.value, t2),
                                "Set transition",
                                icon::INTERACTION,
                            ));
                        }
                    }
                });
            })
            .response;
        let press_away = super::press_began_away(&ctx, menu.layer_id.id, menu.rect, head.rect);
        if close
            || super::dismissed_by_click(super::ClickAway {
                clicked: ctx.input(|i| i.pointer.button_clicked(egui::PointerButton::Primary)),
                over_overlay,
                on_menu: menu.contains_pointer(),
                on_head: head.contains_pointer(),
                // The overlay's ghost on the canvas is this popover's to move.
                on_picker: ctx.pointer_latest_pos().is_some_and(|p| self.ghost_at(p)),
                in_gesture: ctx.dragged_id().is_some() || self.gesture_cancelled,
                press_away,
            })
        {
            self.proto.open = None;
        }
        self.popover_heard = true;
        if ctx.input(|i| i.key_pressed(egui::Key::Escape)) {
            self.proto.open = None;
        }
        let mut v = r.value.clone();
        if let Some(up) = moved {
            shift(&mut v.actions, action, up);
            return Some((
                v,
                "Move action",
                if up { icon::ARROW_UP } else { icon::ARROW_DOWN },
            ));
        }
        let (a, label, glyph) = edit?;
        find_mut(&mut v.actions, action)?.value = a;
        Some((v, label, glyph))
    }

    /// The fields of the action `k`, its transition aside. Answers the action
    /// rewritten and the step's name.
    fn action_fields(
        &mut self,
        ui: &mut egui::Ui,
        id: NodeId,
        trigger: &Trigger,
        k: &Keyed<Action>,
        w: f32,
    ) -> Option<(Action, &'static str, &'static str)> {
        let doc = &self.session.doc;
        let h = ui::CONTROL_H;
        let mut out = None;
        match &k.value {
            Action::Navigate { to, transition } => {
                if let Some(t) = frame_combo(ui, doc, ("to", k.id), *to, id, w) {
                    out = Some((
                        Action::Navigate {
                            to: t,
                            transition: *transition,
                        },
                        "Change action",
                        icon::ARROW_RIGHT,
                    ));
                }
            }
            Action::OpenOverlay {
                overlay,
                settings,
                transition,
            }
            | Action::SwapOverlay {
                overlay,
                settings,
                transition,
            } => {
                let swap = matches!(k.value, Action::SwapOverlay { .. });
                let make = |overlay: NodeId, settings: OverlaySettings| match swap {
                    true => Action::SwapOverlay {
                        overlay,
                        settings,
                        transition: *transition,
                    },
                    false => Action::OpenOverlay {
                        overlay,
                        settings,
                        transition: *transition,
                    },
                };
                let glyph = ActionKind::of(&k.value).glyph();
                if let Some(t) = frame_combo(ui, doc, ("to", k.id), *overlay, id, w) {
                    out = Some((make(t, *settings), "Change action", glyph));
                }
                if let Some(s) = self.overlay_fields(ui, k.id, *settings, w) {
                    out = Some((make(*overlay, s), "Overlay", glyph));
                }
            }
            Action::ScrollTo { layer, transition } => {
                let options: Vec<NodeId> = screen_of(doc, id)
                    .map(|s| descendants(doc, s))
                    .unwrap_or_default();
                if let Some(t) = layer_combo(ui, doc, ("scroll", k.id), *layer, &options, w) {
                    out = Some((
                        Action::ScrollTo {
                            layer: t,
                            transition: *transition,
                        },
                        "Change action",
                        icon::ARROW_LINE_DOWN,
                    ));
                }
            }
            Action::ChangeState {
                instance,
                to,
                transition,
            } => {
                let inst = mastik_core::prototype::resolve_target(doc, id, *instance);
                let options = states_of(doc, inst);
                if let Some(t) = layer_combo(ui, doc, ("state", k.id), *to, &options, w) {
                    out = Some((
                        Action::ChangeState {
                            instance: *instance,
                            to: t,
                            transition: *transition,
                        },
                        "Change action",
                        icon::CHANGE_STATE,
                    ));
                }
                if doc.get(inst).is_some() {
                    ui::selection_note(ui, &format!("Switches “{}”", name(doc, inst)));
                }
            }
            Action::SetVariable { var, change } => {
                let kind = doc.variables().get(*var).map(|v| v.kind);
                if let Some(v) = self.variable_combo(ui, ("set-var", k.id), Some(*var), None) {
                    let new_kind = self.session.doc.variables().get(v).map(|x| x.kind);
                    let change = match (new_kind, change) {
                        (Some(VarKind::Boolean), VarChange::Increment { .. }) => VarChange::Toggle,
                        (Some(VarKind::Number), VarChange::Toggle) => {
                            VarChange::Increment { by: 1.0 }
                        }
                        (
                            Some(VarKind::String | VarKind::Color),
                            VarChange::Toggle | VarChange::Increment { .. },
                        ) => VarChange::Set {
                            to: Expr(String::new()),
                        },
                        (_, c) => c.clone(),
                    };
                    // Answered, with the rows below still drawn this frame.
                    out = Some((
                        Action::SetVariable { var: v, change },
                        "Change action",
                        icon::BRACKETS_CURLY,
                    ));
                }
                let cur = match change {
                    VarChange::Set { .. } => 0,
                    VarChange::Increment { .. } => 1,
                    VarChange::Toggle => 2,
                };
                let enabled = |i: usize| match i {
                    1 => kind == Some(VarKind::Number),
                    2 => kind == Some(VarKind::Boolean),
                    _ => true,
                };
                let pick = ui::segmented_tipped(
                    ui,
                    w,
                    ui::SEGMENT_CELL_H,
                    3,
                    cur,
                    enabled,
                    |i| match i {
                        1 => "Add to a number variable",
                        2 => "Flip a boolean variable",
                        _ => "Set it to what an expression comes to",
                    },
                    |p, i, r, on| ui::segment_label(p, r, ["Set", "Increment", "Toggle"][i], on),
                );
                if let Some(i) = pick.filter(|i| *i != cur) {
                    let change = match i {
                        1 => VarChange::Increment { by: 1.0 },
                        2 => VarChange::Toggle,
                        _ => VarChange::Set {
                            to: Expr(String::new()),
                        },
                    };
                    out = Some((
                        Action::SetVariable { var: *var, change },
                        "Change action",
                        icon::BRACKETS_CURLY,
                    ));
                }
                match change {
                    VarChange::Set { to } => {
                        if let Some(e) = self.expr_field(ui, DraftKey::Expr(k.id, 0), to, w, kind) {
                            out = Some((
                                Action::SetVariable {
                                    var: *var,
                                    change: VarChange::Set { to: e },
                                },
                                "Change action",
                                icon::BRACKETS_CURLY,
                            ));
                        }
                    }
                    VarChange::Increment { by } => {
                        let resp_id = ui.id().with(("by", k.id));
                        let mut v = self
                            .proto
                            .scrub
                            .filter(|(i, _)| *i == resp_id)
                            .map_or(*by, |(_, v)| v);
                        let resp = ui::value_field(
                            ui,
                            egui::vec2(w, h),
                            Prefix::Text("By"),
                            &mut v,
                            Scrub::fine(0.5, 2),
                            |d| d.custom_formatter(ui::number(2)),
                        )
                        .on_hover_text("How much is added; a negative number takes away");
                        // The reaction is rewritten by the caller; here the valve holds the number.
                        let changed = (v - by).abs() > f64::EPSILON;
                        self.proto_hold(resp_id, v, &resp);
                        if changed
                            && !resp.dragged()
                            && !resp.has_focus()
                            && !self.gesture_cancelled
                        {
                            out = Some((
                                Action::SetVariable {
                                    var: *var,
                                    change: VarChange::Increment { by: v },
                                },
                                "Change action",
                                icon::BRACKETS_CURLY,
                            ));
                        }
                    }
                    VarChange::Toggle => {}
                }
            }
            Action::Conditional {
                branches,
                otherwise,
            } => {
                let mut next_branches = branches.clone();
                let mut changed = false;
                let mut drop = None;
                for (i, b) in branches.iter().enumerate() {
                    let word = if i == 0 { "If" } else { "Else if" };
                    ui::labelled(ui, word, |ui| {
                        ui.horizontal(|ui| {
                            ui.spacing_mut().item_spacing.x = COL;
                            let fw = if branches.len() > 1 {
                                w - 18.0 - COL
                            } else {
                                w
                            };
                            if let Some(e) = self.expr_field(
                                ui,
                                DraftKey::Expr(k.id, i),
                                &b.when,
                                fw,
                                Some(VarKind::Boolean),
                            ) {
                                next_branches[i].when = e;
                                changed = true;
                            }
                            if branches.len() > 1
                                && ui::icon_button(ui, icon::X, 18.0, 13.0, false, true)
                                    .on_hover_text("Remove this branch")
                                    .clicked()
                            {
                                drop = Some(i);
                            }
                        })
                    });
                }
                if let Some(i) = drop {
                    next_branches.remove(i);
                    changed = true;
                }
                if ui::word_link(ui, "Add else if", false).clicked() {
                    next_branches.push(Branch {
                        when: Expr("true".into()),
                        actions: Vec::new(),
                    });
                    changed = true;
                }
                if changed {
                    out = Some((
                        Action::Conditional {
                            branches: next_branches,
                            otherwise: otherwise.clone(),
                        },
                        "Change action",
                        icon::CONDITIONAL,
                    ));
                }
                ui::selection_note(
                    ui,
                    "Numbers take + − × ÷; text joins with +; and, or, not and comparisons test. Write a variable as {name}",
                );
            }
            Action::OpenUrl { url } => {
                if let Some(u) = self.url_field(ui, k.id, url, w) {
                    out = Some((
                        Action::OpenUrl { url: u },
                        "Change action",
                        icon::ARROW_SQUARE_OUT,
                    ));
                }
            }
            Action::RunScript { script } => {
                let scripts: Vec<(mastik_core::script::ScriptId, String)> = doc
                    .scripts()
                    .iter()
                    .map(|s| (s.id, s.name.clone()))
                    .collect();
                let current = scripts
                    .iter()
                    .find(|(s, _)| s == script)
                    .map_or("Missing script".to_string(), |(_, n)| n.clone());
                let mut pick = None;
                ui.scope(|ui| {
                    ui::combo_height(ui);
                    egui::ComboBox::from_id_salt(("script", k.id))
                        .icon(ui::combo_chevron)
                        .width(w)
                        .selected_text(ui::glyph_and_text(icon::FILE_CODE, &current))
                        .show_ui(ui, |ui| {
                            ui::menu_rows(ui);
                            for (s, n) in &scripts {
                                if ui.selectable_label(s == script, n).clicked() && s != script {
                                    pick = Some(*s);
                                }
                            }
                        });
                });
                if let Some(s) = pick {
                    out = Some((
                        Action::RunScript { script: s },
                        "Change action",
                        icon::FILE_CODE,
                    ));
                }
                if !trigger.is_a_persons_act() {
                    ui::selection_note(ui, "A script runs only when someone acts");
                }
            }
            Action::Back => {
                ui::selection_note(ui, "Goes back a screen; with an overlay open, closes it");
            }
            Action::CloseOverlay { .. } => {
                ui::selection_note(ui, "Closes the overlay on top");
            }
        }
        out
    }

    /// A transition's rows: its kind, its direction (for the kinds that move one
    /// way), and its timing, a curve over a duration or a spring. Answers the
    /// transition changed: by a pick at once, by a number field once it lets go
    /// (held in [`ProtoState::scrub`] meanwhile).
    pub(crate) fn transition_fields(
        &mut self,
        ui: &mut egui::Ui,
        salt: impl std::hash::Hash + std::fmt::Debug + Copy,
        t: Transition,
        w: f32,
    ) -> Option<Transition> {
        let h = ui::CONTROL_H;
        let mut out = None;
        let mut kind_pick = None;
        ui.scope(|ui| {
            ui::combo_height(ui);
            egui::ComboBox::from_id_salt(("kind", salt))
                .icon(ui::combo_chevron)
                .width(w)
                .selected_text(ui::combo_text(transition_label(t.kind)))
                .show_ui(ui, |ui| {
                    ui::menu_rows(ui);
                    for k in TRANSITION_KINDS {
                        if ui
                            .selectable_label(k == t.kind, transition_label(k))
                            .clicked()
                            && k != t.kind
                        {
                            kind_pick = Some(k);
                        }
                    }
                })
                .response
                .on_hover_text("How the screen moves to the next");
        });
        // A pick is answered, but every row below still draws from `t` this
        // frame, so the popover is never one frame short of its rows.
        if let Some(k) = kind_pick {
            // Smart animate starts with no side (its new layers fade); a move
            // always has one.
            let direction = match k {
                TransitionKind::SmartAnimate => Direction::None,
                _ if t.direction == Direction::None => Direction::Left,
                _ => t.direction,
            };
            out = Some(Transition {
                kind: k,
                direction,
                ..t
            });
        }
        if t.kind == TransitionKind::Instant {
            return out;
        }
        let smart = t.kind == TransitionKind::SmartAnimate;
        // A dissolve goes no way, so it has no direction row.
        if t.kind.is_directional() || smart {
            // A move reads no side as left, and shows it so.
            let shown = match (t.direction, smart) {
                (Direction::None, false) => Direction::Left,
                (d, _) => d,
            };
            let cur = DIRECTIONS
                .iter()
                .position(|(d, _)| *d == shown)
                .unwrap_or(0);
            let pick = ui::segmented_tipped(
                ui,
                w,
                ui::SEGMENT_CELL_H,
                DIRECTIONS.len(),
                cur,
                |i| i != 0 || smart,
                |i| match (i, smart) {
                    (0, true) => "New layers fade in, gone ones fade out",
                    (0, false) => "A move, push or slide needs a side",
                    (_, true) => "The side new layers enter from",
                    _ => "The side it moves toward",
                },
                |p, i, r, on| ui::segment_label(p, r, DIRECTIONS[i].1, on),
            );
            if let Some(i) = pick.filter(|i| *i != cur) {
                out = Some(Transition {
                    direction: DIRECTIONS[i].0,
                    ..t
                });
            }
        }
        let spring = matches!(t.timing, Timing::Spring { .. });
        let pick = ui::segmented_tipped(
            ui,
            w,
            ui::SEGMENT_CELL_H,
            2,
            usize::from(spring),
            |_| true,
            |i| match i {
                0 => "An easing curve over a duration",
                _ => "A spring, which takes as long as it takes to settle",
            },
            |p, i, r, on| ui::segment_label(p, r, ["Curve", "Spring"][i], on),
        );
        match pick {
            Some(1) if !spring => {
                out = Some(Transition {
                    timing: Timing::Spring {
                        mass: 1.0,
                        stiffness: 320.0,
                        damping: 22.0,
                    },
                    ..t
                });
            }
            Some(0) if spring => {
                out = Some(Transition {
                    timing: Timing::Ease {
                        curve: Easing::EaseOut,
                        ms: 300,
                    },
                    ..t
                });
            }
            _ => {}
        }
        // A number's write: the transition with `set` applied to its timing.
        let mut numbers: Vec<(egui::Response, egui::Id, f64, Transition)> = Vec::new();
        match t.timing {
            Timing::Ease { curve, ms } => {
                let mut curve_pick = None;
                ui.horizontal(|ui| {
                    ui.spacing_mut().item_spacing.x = COL;
                    let half = (w - COL) / 2.0;
                    ui.scope(|ui| {
                        ui::combo_height(ui);
                        egui::ComboBox::from_id_salt(("curve", salt))
                            .icon(ui::combo_chevron)
                            .width(half)
                            .selected_text(ui::combo_text(easing_label(curve)))
                            .show_ui(ui, |ui| {
                                ui::menu_rows(ui);
                                for (e, l) in EASINGS {
                                    let on = easing_label(curve) == l;
                                    if ui.selectable_label(on, l).clicked() && !on {
                                        curve_pick = Some(e);
                                    }
                                }
                            });
                    });
                    let rid = ui.id().with(("ms", salt));
                    let mut v = self
                        .proto
                        .scrub
                        .filter(|(i, _)| *i == rid)
                        .map_or(f64::from(ms), |(_, v)| v);
                    let resp = ui::value_field_suffixed(
                        ui,
                        egui::vec2(half, h),
                        Prefix::Label("Duration"),
                        Some(ui::Suffix {
                            text: "ms",
                            clickable: false,
                            tooltip: "",
                        }),
                        &mut v,
                        Scrub::whole(5.0).range(0.0..=10_000.0),
                        |d| d.custom_formatter(ui::number(0)),
                    )
                    .0
                    .on_hover_text("How long it takes");
                    let ms2 = v.round().clamp(0.0, 10_000.0) as u32;
                    numbers.push((
                        resp,
                        rid,
                        v,
                        Transition {
                            timing: Timing::Ease { curve, ms: ms2 },
                            ..t
                        },
                    ));
                });
                if let Some(e) = curve_pick {
                    out = Some(Transition {
                        timing: Timing::Ease { curve: e, ms },
                        ..t
                    });
                }
                if let Easing::Cubic(c) = curve {
                    ui.horizontal(|ui| {
                        ui.spacing_mut().item_spacing.x = COL;
                        let q = (w - 3.0 * COL) / 4.0;
                        for (i, label) in ["X1", "Y1", "X2", "Y2"].into_iter().enumerate() {
                            let rid = ui.id().with(("cubic", salt, i));
                            let mut v = self
                                .proto
                                .scrub
                                .filter(|(x, _)| *x == rid)
                                .map_or(c[i], |(_, v)| v);
                            let range = if i % 2 == 0 { 0.0..=1.0 } else { -2.0..=3.0 };
                            let resp = ui::value_field(
                                ui,
                                egui::vec2(q, h),
                                Prefix::Text(label),
                                &mut v,
                                Scrub::fine(0.01, 2).range(range),
                                |d| d.custom_formatter(ui::number(2)),
                            )
                            .on_hover_text(
                                "A handle of the cubic-bezier curve; both X stay within 0 to 1",
                            );
                            let mut c2 = c;
                            c2[i] = v;
                            numbers.push((
                                resp,
                                rid,
                                v,
                                Transition {
                                    timing: Timing::Ease {
                                        curve: Easing::Cubic(c2),
                                        ms,
                                    },
                                    ..t
                                },
                            ));
                        }
                    });
                }
            }
            Timing::Spring {
                mass,
                stiffness,
                damping,
            } => {
                ui.horizontal(|ui| {
                    ui.spacing_mut().item_spacing.x = COL;
                    let q = (w - 2.0 * COL) / 3.0;
                    let parts = [
                        ("M", mass, "Mass: how heavy what moves is"),
                        ("S", stiffness, "Stiffness: how hard the spring pulls"),
                        ("D", damping, "Damping: how fast it stops bouncing"),
                    ];
                    for (i, (label, x, tip)) in parts.into_iter().enumerate() {
                        let rid = ui.id().with(("spring", salt, i));
                        let mut v = self
                            .proto
                            .scrub
                            .filter(|(y, _)| *y == rid)
                            .map_or(x, |(_, v)| v);
                        let resp = ui::value_field(
                            ui,
                            egui::vec2(q, h),
                            Prefix::Text(label),
                            &mut v,
                            Scrub::fine(if i == 0 { 0.05 } else { 1.0 }, 2).range(0.01..=10_000.0),
                            |d| d.custom_formatter(ui::number(2)),
                        )
                        .on_hover_text(tip);
                        let mut s = [mass, stiffness, damping];
                        s[i] = v.max(0.01);
                        numbers.push((
                            resp,
                            rid,
                            v,
                            Transition {
                                timing: Timing::Spring {
                                    mass: s[0],
                                    stiffness: s[1],
                                    damping: s[2],
                                },
                                ..t
                            },
                        ));
                    }
                });
            }
        }
        // The timing a field holds while it is scrubbed or typed, so the preview
        // follows it before it commits.
        let shown = numbers
            .iter()
            .find(|(r, ..)| r.dragged() || r.has_focus())
            .map_or(t.timing, |(.., next)| next.timing);
        for (resp, rid, v, next) in numbers {
            self.proto_hold(rid, v, &resp);
            let engaged = resp.dragged() || resp.has_focus();
            if next != t && !engaged && interacting(&resp) && !self.gesture_cancelled {
                out = Some(next);
            }
        }
        motion_preview(ui, salt, shown, w);
        out
    }

    /// An overlay action's rows: where it shows, *Close on a click outside*,
    /// *Darken behind it* and, while darkened, the shade's colour and strength.
    /// *Manual* adds X and Y, measured from the layer that opens it; the frame is
    /// placed by dragging its ghost on the canvas, the fields being for the last
    /// point. Answers as [`Self::transition_fields`] does.
    fn overlay_fields(
        &mut self,
        ui: &mut egui::Ui,
        action: ItemId,
        o: OverlaySettings,
        w: f32,
    ) -> Option<OverlaySettings> {
        let mut out = None;
        let mut numbers: Vec<(egui::Response, egui::Id, f64, OverlaySettings)> = Vec::new();
        ui::labelled(ui, "Position", |ui| {
            let same = |a: &OverlayPosition, b: &OverlayPosition| {
                std::mem::discriminant(a) == std::mem::discriminant(b)
            };
            let shown = OVERLAY_POSITIONS
                .iter()
                .find(|(p, _)| same(p, &o.position))
                .map_or("Centre", |(_, l)| *l);
            let mut pick = None;
            ui.scope(|ui| {
                ui::combo_height(ui);
                egui::ComboBox::from_id_salt(("overlay-at", action))
                    .icon(ui::combo_chevron)
                    .width(w)
                    .selected_text(ui::combo_text(shown))
                    .show_ui(ui, |ui| {
                        ui::menu_rows(ui);
                        for (p, l) in OVERLAY_POSITIONS {
                            let on = same(&p, &o.position);
                            if ui.selectable_label(on, l).clicked() && !on {
                                pick = Some(p);
                            }
                        }
                    })
                    .response
                    .on_hover_text("Where the frame shows over the screen");
            });
            if let Some(p) = pick {
                out = Some(OverlaySettings { position: p, ..o });
            }
            if let OverlayPosition::Manual { x, y } = o.position {
                ui.horizontal(|ui| {
                    ui.spacing_mut().item_spacing.x = COL;
                    let half = (w - COL) / 2.0;
                    let mut held = None;
                    for (i, (label, v0)) in [("X", x), ("Y", y)].into_iter().enumerate() {
                        let rid = ui.id().with(("overlay-xy", action, i));
                        let mut v = self
                            .proto
                            .scrub
                            .filter(|(r, _)| *r == rid)
                            .map_or(v0, |(_, v)| v);
                        let resp = ui::value_field(
                            ui,
                            egui::vec2(half, ui::CONTROL_H),
                            Prefix::Text(label),
                            &mut v,
                            Scrub::whole(0.5),
                            |d| d.custom_formatter(ui::number(1)),
                        )
                        .on_hover_text("From the top-left of the layer that opens it");
                        let at = match i {
                            0 => mastik_core::kurbo::Vec2::new(v, y),
                            _ => mastik_core::kurbo::Vec2::new(x, v),
                        };
                        if resp.dragged() || resp.has_focus() {
                            held = Some((action, at));
                        }
                        let position = OverlayPosition::Manual { x: at.x, y: at.y };
                        numbers.push((resp, rid, v, OverlaySettings { position, ..o }));
                    }
                    self.proto.held_at = held;
                });
                ui::selection_note(ui, "Drag the frame on the canvas to place it");
            }
        });
        if ui::switch_row(
            ui,
            "Close on a click outside",
            o.close_outside,
            ui::CONTROL_H,
        )
        .clicked()
        {
            out = Some(OverlaySettings {
                close_outside: !o.close_outside,
                ..o
            });
        }
        if ui::switch_row(ui, "Darken behind it", o.backdrop.is_some(), ui::CONTROL_H)
            .on_hover_text("Draw a shade over the screen beneath the overlay")
            .clicked()
        {
            out = Some(OverlaySettings {
                backdrop: match o.backdrop {
                    Some(_) => None,
                    None => Some(mastik_core::peniko::Color::from_rgba8(0, 0, 0, 102)),
                },
                ..o
            });
        }
        if let Some(c) = o.backdrop {
            // The shade's colour, typed as every colour field takes one; the
            // shade below keeps its own strength.
            let key = DraftKey::Backdrop(action);
            let ctx = ui.ctx().clone();
            let [r, g, b, _] = c.to_rgba8().to_u8_array();
            let shown = format!("{r:02x}{g:02x}{b:02x}");
            let mut text = ui::Draft::live(&mut self.proto.draft, &ctx, key)
                .map_or(shown.clone(), |d| d.text.clone());
            let ink = crate::colour_text::parse(&text).map_or(egui::Color32::TRANSPARENT, |t| {
                ui::color_to_egui(t.color(1.0))
            });
            let resp = ui::colour_text_field(ui, egui::vec2(w, ui::CONTROL_H), &mut text, ink)
                .on_hover_text("The colour of the shade behind the overlay");
            if let Some(typed) = ui::Draft::follow(&mut self.proto.draft, &resp, key, &text)
                && typed != shown
                && let Some(t) = crate::colour_text::parse(&typed)
            {
                out = Some(OverlaySettings {
                    backdrop: Some(t.color(c.components[3])),
                    ..o
                });
            }
            let rid = ui.id().with(("backdrop", action));
            let a0 = f64::from(c.components[3]) * 100.0;
            let mut v = self
                .proto
                .scrub
                .filter(|(r, _)| *r == rid)
                .map_or(a0, |(_, v)| v);
            let resp = ui::value_field_suffixed(
                ui,
                egui::vec2(w, ui::CONTROL_H),
                Prefix::Text("Shade"),
                Some(ui::Suffix {
                    text: "%",
                    clickable: false,
                    tooltip: "",
                }),
                &mut v,
                Scrub::whole(0.5).range(0.0..=100.0),
                |d| d.custom_formatter(ui::number(0)),
            )
            .0
            .on_hover_text("How dark the shade behind the overlay is");
            let mut c2 = c;
            c2.components[3] = (v.clamp(0.0, 100.0) / 100.0) as f32;
            numbers.push((
                resp,
                rid,
                v,
                OverlaySettings {
                    backdrop: Some(c2),
                    ..o
                },
            ));
        }
        for (resp, rid, v, next) in numbers {
            self.proto_hold(rid, v, &resp);
            let engaged = resp.dragged() || resp.has_focus();
            if next != o && !engaged && interacting(&resp) && !self.gesture_cancelled {
                out = Some(next);
            }
        }
        out
    }

    /// Hold a number field's value while it is scrubbed or typed, and let go once
    /// it is not. A right-click calls the scrub off (`cancel_gesture`): nothing
    /// is held until the button is let go.
    fn proto_hold(&mut self, rid: egui::Id, v: f64, resp: &egui::Response) {
        if self.gesture_cancelled {
            self.proto.scrub = None;
        } else if resp.dragged() || resp.has_focus() {
            self.proto.scrub = Some((rid, v));
        } else if self.proto.scrub.is_some_and(|(i, _)| i == rid) {
            self.proto.scrub = None;
        }
    }

    /// A number field over the saved model: held while engaged, then committed
    /// once through the valve.
    fn proto_valve(
        &mut self,
        rid: egui::Id,
        v: f64,
        resp: &egui::Response,
        label: &'static str,
        glyph: &'static str,
        tx: Transaction,
    ) {
        self.proto_hold(rid, v, resp);
        self.named_valve(label, glyph, resp, tx);
    }

    /// A variable's dropdown, by name, `kind` narrowing it; answers a new pick.
    fn variable_combo(
        &mut self,
        ui: &mut egui::Ui,
        salt: impl std::hash::Hash + std::fmt::Debug,
        current: Option<VarId>,
        kind: Option<VarKind>,
    ) -> Option<VarId> {
        let doc = &self.session.doc;
        let vars: Vec<(VarId, String)> = doc
            .variables()
            .iter()
            .filter(|v| kind.is_none_or(|k| v.kind == k))
            .map(|v| (v.id, v.name.clone()))
            .collect();
        let shown = match current {
            Some(c) => vars
                .iter()
                .find(|(v, _)| *v == c)
                .map_or("Missing variable".to_string(), |(_, n)| n.clone()),
            None => "None".into(),
        };
        let mut pick = None;
        let w = ui.available_width();
        ui.scope(|ui| {
            ui::combo_height(ui);
            egui::ComboBox::from_id_salt(salt)
                .icon(ui::combo_chevron)
                .width(w)
                .selected_text(ui::glyph_and_text(icon::BRACKETS_CURLY, &shown))
                .show_ui(ui, |ui| {
                    ui::menu_rows(ui);
                    if vars.is_empty() {
                        ui::menu_note(ui, "No variables of that kind");
                    }
                    for (v, n) in &vars {
                        let on = current == Some(*v);
                        if ui.selectable_label(on, n).clicked() && !on {
                            pick = Some(*v);
                        }
                    }
                });
        });
        pick
    }

    /// An expression's field: the source with variables by name, refused (kept,
    /// ringed, the reason under it) when it does not read. Answers what to store
    /// once it commits.
    fn expr_field(
        &mut self,
        ui: &mut egui::Ui,
        key: DraftKey,
        e: &Expr,
        w: f32,
        want: Option<VarKind>,
    ) -> Option<Expr> {
        let shown = expr_shown(&self.session.doc, e);
        let ctx = ui.ctx().clone();
        let live = ui::Draft::live(&mut self.proto.draft, &ctx, key);
        let refused = live.as_ref().and_then(|d| d.refused.clone());
        let mut text = live.map_or(shown.clone(), |d| d.text.clone());
        let resp = ui
            .scope(|ui| {
                if refused.is_some() {
                    ui.visuals_mut().widgets.inactive.bg_stroke = ui::refused_stroke();
                }
                ui::text_field(
                    ui,
                    egui::vec2(w, ui::CONTROL_H),
                    &mut text,
                    "An expression, such as {count} + 1",
                )
            })
            .inner;
        if let Some(why) = &refused {
            if let Some((k, at)) = &self.proto.expr_fault
                && *k == key
            {
                ui::field_underline(ui, &resp, ui::CONTROL_H, &text, at.clone());
            }
            ui::field_refusal(ui, why);
        }
        let typed = ui::Draft::follow(&mut self.proto.draft, &resp, key, &text)?;
        if typed == shown {
            return None;
        }
        match expr_stored(&self.session.doc, &typed, want) {
            Ok(e) => {
                self.proto.expr_fault = None;
                Some(e)
            }
            Err((why, at)) => {
                self.proto.expr_fault = at.map(|at| (key, at));
                self.proto.draft = Some(ui::Draft::new(&ctx, key, typed).refuse(why));
                None
            }
        }
    }

    /// *Open link*'s address, refused unless it is `http` or `https`.
    fn url_field(&mut self, ui: &mut egui::Ui, item: ItemId, url: &str, w: f32) -> Option<String> {
        let key = DraftKey::Url(item);
        let ctx = ui.ctx().clone();
        let live = ui::Draft::live(&mut self.proto.draft, &ctx, key);
        let refused = live.as_ref().and_then(|d| d.refused.clone());
        let mut text = live.map_or(url.to_string(), |d| d.text.clone());
        let resp = ui::text_field(ui, egui::vec2(w, ui::CONTROL_H), &mut text, "https://");
        if let Some(why) = &refused {
            ui::field_refusal(ui, why);
        }
        let typed = ui::Draft::follow(&mut self.proto.draft, &resp, key, &text)?;
        let typed = typed.trim().to_string();
        if typed == url {
            return None;
        }
        if !is_link(&typed) {
            self.proto.draft = Some(
                ui::Draft::new(&ctx, key, typed).refuse("Not an address a link can take".into()),
            );
            return None;
        }
        Some(typed)
    }

    /// The *Prototype* card of a layer: a frame's scroll and overlay defaults, a
    /// child's pin, and an input binding on a text or an instance.
    fn layer_play_card(&mut self, ui: &mut egui::Ui, id: NodeId) {
        let doc = &self.session.doc;
        let Some(n) = doc.get(id) else { return };
        let play = n.play().clone();
        let frame = matches!(n.kind(), NodeKind::Artboard { .. });
        let text = matches!(n.kind(), NodeKind::Text { .. });
        let instance =
            n.link().is_some() && mastik_core::component::instance_root(doc, id) == Some(id);
        let parent = n.parent().filter(|p| doc.page(*p).is_none());
        let parent_scrolls = parent
            .and_then(|p| doc.get(p))
            .is_some_and(|p| p.play().scroll != ScrollAxis::None);
        let bool_props: Vec<String> = match instance {
            true => mastik_core::component::main_of(doc, id)
                .and_then(|m| mastik_core::variant::set_of(doc, m))
                .and_then(|s| doc.get(s))
                .and_then(|s| s.set())
                .map(|s| {
                    s.props
                        .iter()
                        .filter(|p| p.values.len() == 2)
                        .map(|p| p.name.clone())
                        .collect()
                })
                .unwrap_or_default(),
            false => Vec::new(),
        };
        let pinned: Vec<(String, Pin)> = match frame {
            true => n
                .children()
                .iter()
                .filter_map(|c| doc.get(*c))
                .filter(|c| c.play().pin != Pin::Scrolls)
                .map(|c| (name(doc, c.id()), c.play().pin))
                .collect(),
            false => Vec::new(),
        };
        if !frame && parent.is_none() && !text && !instance {
            return;
        }
        let mut next: Option<(LayerPlay, &'static str)> = None;
        self.panel(ui, "Prototype", None, |app, ui| {
            let w = ui.available_width();
            if frame {
                ui::labelled(ui, "Scrolling", |ui| {
                    let cur = [ScrollAxis::None, ScrollAxis::Vertical, ScrollAxis::Horizontal, ScrollAxis::Both]
                        .iter()
                        .position(|a| *a == play.scroll)
                        .unwrap_or(0);
                    let pick = ui::segmented_tipped(
                        ui,
                        w,
                        ui::SEGMENT_CELL_H,
                        4,
                        cur,
                        |_| true,
                        |_| "The axis the frame scrolls on when its content overflows",
                        |p, i, r, on| ui::segment_label(p, r, ["None", "Vertical", "Horizontal", "Both"][i], on),
                    );
                    if let Some(i) = pick.filter(|i| *i != cur) {
                        let mut p = play.clone();
                        p.scroll = [ScrollAxis::None, ScrollAxis::Vertical, ScrollAxis::Horizontal, ScrollAxis::Both][i];
                        next = Some((p, "Scrolling"));
                    }
                    for (who, pin) in &pinned {
                        let word = if *pin == Pin::Fixed { "Fixed" } else { "Sticky" };
                        ui.label(
                            egui::RichText::new(format!("{who} · {word}"))
                                .size(ui::SMALL_PT)
                                .color(theme::text::dim()),
                        );
                    }
                });
            }
            if parent.is_some() {
                ui::labelled(ui, "In a scrolling frame", |ui| {
                    let cur = [Pin::Scrolls, Pin::Fixed, Pin::Sticky].iter().position(|p| *p == play.pin).unwrap_or(0);
                    let pick = ui::segmented_tipped(
                        ui,
                        w,
                        ui::SEGMENT_CELL_H,
                        3,
                        cur,
                        |_| parent_scrolls,
                        |i| match (parent_scrolls, i) {
                            (false, _) => "The frame does not scroll",
                            (true, 0) => "Moves with the content",
                            (true, 1) => "Stays where it is drawn",
                            _ => "Scrolls until it reaches its place, then stays",
                        },
                        |p, i, r, on| ui::segment_label(p, r, ["Scrolls", "Fixed", "Sticky"][i], on),
                    );
                    if let Some(i) = pick.filter(|i| *i != cur) {
                        let mut p = play.clone();
                        p.pin = [Pin::Scrolls, Pin::Fixed, Pin::Sticky][i];
                        next = Some((p, "Pin"));
                    }
                });
            }
            if text || instance {
                ui::labelled(ui, "Input", |ui| {
                    let kind = if text { VarKind::String } else { VarKind::Boolean };
                    let current = play.input.as_ref().map(|i| i.var);
                    ui.horizontal(|ui| {
                        ui.spacing_mut().item_spacing.x = COL;
                        let cw = if current.is_some() { w - 18.0 - COL } else { w };
                        ui.scope(|ui| {
                            ui.set_width(cw);
                            if let Some(v) = app.variable_combo(ui, ("input", id), current, Some(kind)) {
                                let mut p = play.clone();
                                let property = match text {
                                    true => None,
                                    false => p.input.as_ref().and_then(|i| i.property.clone()).or_else(|| bool_props.first().cloned()),
                                };
                                p.input = Some(Input { var: v, property });
                                next = Some((p, "Input"));
                            }
                        });
                        if current.is_some()
                            && ui::icon_button(ui, icon::X, 18.0, 13.0, false, true)
                                .on_hover_text("Stop being an input")
                                .clicked()
                        {
                            let mut p = play.clone();
                            p.input = None;
                            next = Some((p, "Input"));
                        }
                    });
                    if !text && let Some(input) = &play.input {
                        let shown = input.property.clone().unwrap_or_else(|| "None".into());
                        let mut pick = None;
                        ui.scope(|ui| {
                            ui::combo_height(ui);
                            egui::ComboBox::from_id_salt(("input-prop", id))
                                .icon(ui::combo_chevron)
                                .width(w)
                                .selected_text(ui::combo_text(&shown))
                                .show_ui(ui, |ui| {
                                    ui::menu_rows(ui);
                                    if bool_props.is_empty() {
                                        ui::menu_note(ui, "The set has no two-valued property");
                                    }
                                    for p in &bool_props {
                                        let on = input.property.as_deref() == Some(p);
                                        if ui.selectable_label(on, p).clicked() && !on {
                                            pick = Some(p.clone());
                                        }
                                    }
                                })
                                .response
                                .on_hover_text("The property that shows the variable, such as a checkbox's Checked");
                        });
                        if let Some(prop) = pick {
                            let mut p = play.clone();
                            p.input = Some(Input { var: input.var, property: Some(prop) });
                            next = Some((p, "Input"));
                        }
                    }
                    let note = match text {
                        true => "Typing into it while playing sets the variable",
                        false => "Ticking it while playing sets the variable",
                    };
                    ui::selection_note(ui, note);
                });
            }
        });
        if let Some((p, label)) = next {
            self.session.name_step(label, icon::INTERACTION);
            self.commit_edit(Transaction(vec![Operation::SetPlay { id, play: p }]));
        }
    }

    /// The page's *Flows*: one row per flow, its name and its start frame; `+`
    /// adds one starting at the page's first frame.
    fn flows_card(&mut self, ui: &mut egui::Ui) {
        let page = self.session.page;
        let doc = &self.session.doc;
        let flows = doc.page(page).map(|p| p.flows.clone()).unwrap_or_default();
        let frames: Vec<NodeId> = doc
            .get(page)
            .map(|r| {
                r.children()
                    .iter()
                    .copied()
                    .filter(|c| is_frame(doc, *c))
                    .collect()
            })
            .unwrap_or_default();
        let action = match frames.first() {
            Some(_) => ui::HeadAction::new(icon::PLUS, "Add a flow"),
            None => ui::HeadAction::inert(icon::PLUS, "Add a frame to start a flow from"),
        };
        let mut next: Option<(Vec<Keyed<Flow>>, &'static str)> = None;
        let add = self.panel(ui, "Flows", Some(action), |app, ui| {
            if flows.is_empty() {
                ui::empty_note(ui, "No flows");
            }
            for f in &flows {
                ui.horizontal(|ui| {
                    ui.spacing_mut().item_spacing.x = COL;
                    let w = ui.available_width() - 18.0 - COL;
                    let half = (w - COL) / 2.0;
                    if let Some(n) = app.flow_name_field(ui, f.id, &f.name, half) {
                        let list = flows
                            .iter()
                            .map(|x| match x.id == f.id {
                                true => x.map(|v| Flow {
                                    name: n.clone(),
                                    ..v.clone()
                                }),
                                false => x.clone(),
                            })
                            .collect();
                        next = Some((list, "Rename flow"));
                    }
                    let doc = &app.session.doc;
                    if let Some(s) =
                        layer_combo(ui, doc, ("flow-start", f.id), f.start, &frames, half)
                    {
                        let list = flows
                            .iter()
                            .map(|x| match x.id == f.id {
                                true => x.map(|v| Flow {
                                    start: s,
                                    ..v.clone()
                                }),
                                false => x.clone(),
                            })
                            .collect();
                        next = Some((list, "Change flow"));
                    }
                    if ui::icon_button(ui, icon::X, 18.0, 13.0, false, true)
                        .on_hover_text("Remove")
                        .clicked()
                    {
                        next = Some((
                            flows.iter().filter(|x| x.id != f.id).cloned().collect(),
                            "Remove flow",
                        ));
                    }
                });
                // The flow's starting values: each a variable and what it holds
                // when the flow starts, typed as the variable's kind takes it.
                let with_values = |values: Vec<(VarId, Cell)>| -> Vec<Keyed<Flow>> {
                    flows
                        .iter()
                        .map(|x| match x.id == f.id {
                            true => x.map(|v| Flow {
                                values: values.clone(),
                                ..v.clone()
                            }),
                            false => x.clone(),
                        })
                        .collect()
                };
                for (i, (var, cell)) in f.values.iter().enumerate() {
                    ui.horizontal(|ui| {
                        ui.spacing_mut().item_spacing.x = COL;
                        let w = ui.available_width() - 18.0 - COL;
                        let half = (w - COL) / 2.0;
                        let picked = ui
                            .allocate_ui(egui::vec2(half, ui::CONTROL_H), |ui| {
                                app.variable_combo(ui, ("flow-value", f.id, i), Some(*var), None)
                            })
                            .inner;
                        if let Some(v) = picked {
                            let mut values = f.values.clone();
                            let start = app
                                .session
                                .doc
                                .variables()
                                .get(v)
                                .and_then(|x| x.cells.values().next().cloned());
                            if let Some(c) = start {
                                values[i] = (v, c);
                                next = Some((with_values(values), "Change starting value"));
                            }
                        }
                        if let Some(c) = app.flow_value_field(ui, f.id, i, *var, cell, half) {
                            let mut values = f.values.clone();
                            values[i].1 = c;
                            next = Some((with_values(values), "Change starting value"));
                        }
                        if ui::icon_button(ui, icon::X, 18.0, 13.0, false, true)
                            .on_hover_text("Remove starting value")
                            .clicked()
                        {
                            let mut values = f.values.clone();
                            values.remove(i);
                            next = Some((with_values(values), "Remove starting value"));
                        }
                    });
                }
                // The first variable no starting value names yet, at its first
                // mode's value.
                let unused = app
                    .session
                    .doc
                    .variables()
                    .iter()
                    .find(|v| !f.values.iter().any(|(x, _)| *x == v.id))
                    .and_then(|v| Some((v.id, v.cells.values().next()?.clone())));
                if crate::settings::text_button(
                    ui,
                    "Add a starting value",
                    match unused {
                        Some(_) => ui::FieldButton::Off,
                        None => ui::FieldButton::Disabled,
                    },
                )
                .on_hover_text(match unused {
                    Some(_) => "Set a variable as the flow starts",
                    None => "Every variable has a starting value",
                })
                .clicked()
                    && let Some(v) = unused
                {
                    let mut values = f.values.clone();
                    values.push(v);
                    next = Some((with_values(values), "Add starting value"));
                }
            }
        });
        if add && let Some(start) = frames.first().copied() {
            let mut list = flows.clone();
            let mut n = list.len() + 1;
            while list.iter().any(|f| f.name == format!("Flow {n}")) {
                n += 1;
            }
            list.push(Keyed::new(
                self.session.ids.mint_item(),
                Flow {
                    name: format!("Flow {n}"),
                    start,
                    values: Vec::new(),
                },
            ));
            next = Some((list, "Add flow"));
        }
        if let Some((flows, label)) = next {
            self.session.name_step(label, icon::PLAY);
            self.commit_edit(Transaction(vec![Operation::SetFlows { id: page, flows }]));
        }
    }

    /// A flow's starting value for `var`, typed as its kind reads: a number, a
    /// text, `true` or `false`, a colour, or `{name}` for another variable of the
    /// same kind. Refused, kept and ringed, when it does not read.
    fn flow_value_field(
        &mut self,
        ui: &mut egui::Ui,
        flow: ItemId,
        index: usize,
        var: VarId,
        cell: &Cell,
        w: f32,
    ) -> Option<Cell> {
        let doc = &self.session.doc;
        let kind = doc.variables().get(var).map(|v| v.kind)?;
        let shown = match cell {
            Cell::Number(n) => format!("{n}"),
            Cell::String(s) => s.clone(),
            Cell::Boolean(b) => b.to_string(),
            Cell::Color(c) => {
                let [r, g, b, a] = c.to_rgba8().to_u8_array();
                match a {
                    255 => format!("#{r:02x}{g:02x}{b:02x}"),
                    _ => format!("#{r:02x}{g:02x}{b:02x}{a:02x}"),
                }
            }
            Cell::Alias(v) => format!(
                "{{{}}}",
                doc.variables().get(*v).map_or("?", |x| x.name.as_str())
            ),
        };
        let key = DraftKey::FlowValue(flow, index);
        let ctx = ui.ctx().clone();
        let live = ui::Draft::live(&mut self.proto.draft, &ctx, key);
        let refused = live.as_ref().and_then(|d| d.refused.clone());
        let mut text = live.map_or(shown.clone(), |d| d.text.clone());
        let resp = ui
            .scope(|ui| {
                if refused.is_some() {
                    ui.visuals_mut().widgets.inactive.bg_stroke = ui::refused_stroke();
                }
                ui::text_field(ui, egui::vec2(w, ui::CONTROL_H), &mut text, "Value")
            })
            .inner;
        if let Some(why) = &refused {
            resp.clone().on_hover_text(why);
        }
        let typed = ui::Draft::follow(&mut self.proto.draft, &resp, key, &text)?;
        if typed == shown {
            return None;
        }
        let t = typed.trim();
        let alias = t
            .strip_prefix('{')
            .and_then(|r| r.strip_suffix('}'))
            .and_then(|n| doc.variables().by_name(n))
            .filter(|v| v.kind == kind && v.id != var)
            .map(|v| Cell::Alias(v.id));
        let read = alias.or(match kind {
            VarKind::Number => t
                .parse::<f64>()
                .ok()
                .filter(|n| n.is_finite())
                .map(Cell::Number),
            VarKind::String => Some(Cell::String(typed.clone())),
            VarKind::Boolean => match t {
                "true" => Some(Cell::Boolean(true)),
                "false" => Some(Cell::Boolean(false)),
                _ => None,
            },
            VarKind::Color => crate::colour_text::parse(t).map(|c| Cell::Color(c.color(1.0))),
        });
        if read.is_none() {
            let why = format!("Not a {}", kind_word(kind));
            self.proto.draft = Some(ui::Draft::new(&ctx, key, typed).refuse(why));
        }
        read
    }

    fn flow_name_field(
        &mut self,
        ui: &mut egui::Ui,
        item: ItemId,
        current: &str,
        w: f32,
    ) -> Option<String> {
        let key = DraftKey::FlowName(item);
        let ctx = ui.ctx().clone();
        let mut text = ui::Draft::live(&mut self.proto.draft, &ctx, key)
            .map_or(current.to_string(), |d| d.text.clone());
        let resp = ui::text_field(ui, egui::vec2(w, ui::CONTROL_H), &mut text, "Flow name");
        let typed = ui::Draft::follow(&mut self.proto.draft, &resp, key, &text)?;
        let typed = typed.trim().to_string();
        (!typed.is_empty() && typed != current).then_some(typed)
    }

    /// The page's *Prototype* card: the device drawn round the screen, how the
    /// screen fits the player, and the transition a new link takes.
    fn page_play_card(&mut self, ui: &mut egui::Ui) {
        let page = self.session.page;
        let play = self
            .session
            .doc
            .page(page)
            .map(|p| p.play.clone())
            .unwrap_or_default();
        let mut next: Option<(PagePlay, &'static str)> = None;
        self.panel(ui, "Prototype", None, |app, ui| {
            let w = ui.available_width();
            ui::labelled(ui, "Device", |ui| {
                let shown = play.device.clone().unwrap_or_else(|| "None".into());
                let mut pick: Option<Option<String>> = None;
                ui.scope(|ui| {
                    ui::combo_height(ui);
                    egui::ComboBox::from_id_salt(("device", page))
                        .icon(ui::combo_chevron)
                        .width(w)
                        .selected_text(ui::glyph_and_text(icon::DEVICES, &shown))
                        .show_ui(ui, |ui| {
                            ui::menu_rows(ui);
                            if ui.selectable_label(play.device.is_none(), "None").clicked()
                                && play.device.is_some()
                            {
                                pick = Some(None);
                            }
                            for (group, entries) in FRAME_PRESETS
                                .iter()
                                .filter(|(g, _)| matches!(*g, "PHONE" | "TABLET"))
                            {
                                ui::menu_sep(ui);
                                ui::menu_eyebrow(ui, group);
                                for (n, pw, ph) in entries.iter() {
                                    let on = play.device.as_deref() == Some(*n);
                                    let row = ui.selectable_label(on, format!("{n}, {pw} × {ph}"));
                                    if row.clicked() && !on {
                                        pick = Some(Some((*n).to_string()));
                                    }
                                }
                            }
                        })
                        .response
                        .on_hover_text("The device drawn round the screen while playing");
                });
                if let Some(d) = pick {
                    next = Some((
                        PagePlay {
                            device: d,
                            ..play.clone()
                        },
                        "Device",
                    ));
                }
            });
            ui::labelled(ui, "Scaling", |ui| {
                let all = [Scaling::Fit, Scaling::ActualSize, Scaling::FillWidth];
                let cur = all.iter().position(|s| *s == play.scaling).unwrap_or(0);
                let pick = ui::segmented_tipped(
                    ui,
                    w,
                    ui::SEGMENT_CELL_H,
                    3,
                    cur,
                    |_| true,
                    |i| match i {
                        0 => "The whole screen fits the player",
                        1 => "One point to one pixel",
                        _ => "The screen's width fills the player",
                    },
                    |p, i, r, on| {
                        ui::segment_label(p, r, ["Fit", "Actual size", "Fill width"][i], on)
                    },
                );
                if let Some(i) = pick.filter(|i| *i != cur) {
                    next = Some((
                        PagePlay {
                            scaling: all[i],
                            ..play.clone()
                        },
                        "Scaling",
                    ));
                }
            });
            ui::labelled(ui, "New links", |ui| {
                if let Some(t) = app.transition_fields(ui, ("page-tr", page), play.transition, w) {
                    next = Some((
                        PagePlay {
                            transition: t,
                            ..play.clone()
                        },
                        "New links",
                    ));
                }
            });
        });
        if let Some((p, label)) = next {
            self.session.name_step(label, icon::INTERACTION);
            self.commit_edit(Transaction(vec![Operation::SetPagePlay {
                id: page,
                play: p,
            }]));
        }
    }
}

/// A dropdown of the screens, page by page; answers a new pick.
fn frame_combo(
    ui: &mut egui::Ui,
    doc: &Document,
    salt: impl std::hash::Hash + std::fmt::Debug,
    current: NodeId,
    holder: NodeId,
    w: f32,
) -> Option<NodeId> {
    let current = mastik_core::prototype::resolve_target(doc, holder, current);
    let shown = match doc.get(current) {
        Some(_) => name(doc, current),
        None => "Missing frame".into(),
    };
    let all = screens(doc);
    let paged = all.len() > 1;
    let mut pick = None;
    ui.scope(|ui| {
        ui::combo_height(ui);
        egui::ComboBox::from_id_salt(salt)
            .icon(ui::combo_chevron)
            .width(w)
            .selected_text(ui::glyph_and_text(icon::FRAME, &shown))
            .show_ui(ui, |ui| {
                ui::menu_rows(ui);
                let mut first = true;
                for (page, frames) in &all {
                    if paged && !frames.is_empty() {
                        if !first {
                            ui::menu_sep(ui);
                        }
                        first = false;
                        let p = crate::private::page(doc, *page).into_owned();
                        ui::menu_eyebrow(ui, &p);
                    }
                    for f in frames {
                        let on = *f == current;
                        if ui.selectable_label(on, name(doc, *f)).clicked() && !on {
                            pick = Some(*f);
                        }
                    }
                }
            });
    });
    pick
}

/// A dropdown of `options` by name; answers a new pick.
fn layer_combo(
    ui: &mut egui::Ui,
    doc: &Document,
    salt: impl std::hash::Hash + std::fmt::Debug,
    current: NodeId,
    options: &[NodeId],
    w: f32,
) -> Option<NodeId> {
    let shown = match doc.get(current) {
        Some(_) => name(doc, current),
        None => "Missing layer".into(),
    };
    let mut pick = None;
    ui.scope(|ui| {
        ui::combo_height(ui);
        egui::ComboBox::from_id_salt(salt)
            .icon(ui::combo_chevron)
            .width(w)
            .selected_text(ui::combo_text(&shown))
            .show_ui(ui, |ui| {
                ui::menu_rows(ui);
                for o in options {
                    let on = *o == current;
                    if ui.selectable_label(on, name(doc, *o)).clicked() && !on {
                        pick = Some(*o);
                    }
                }
            });
    });
    pick
}

enum TriggerPick {
    Set(Trigger),
    Key,
}

#[derive(Default)]
struct BlockOut {
    write: Option<(Vec<Keyed<Reaction>>, &'static str, &'static str)>,
    reset: bool,
}

#[derive(Default)]
struct ListOut {
    add: Option<(Place, ActionKind)>,
    remove: Option<ItemId>,
    head: Option<egui::Response>,
    /// An action dropped at a new place in its own list.
    reorder: Option<(ItemId, usize)>,
}

#[derive(Default)]
struct RowOut {
    open: bool,
    remove: bool,
    /// Its grip began a drag.
    grabbed: bool,
}

#[cfg(test)]
mod tests {
    use super::*;
    use mastik_core::kurbo::{Affine, Size, Vec2};
    use mastik_core::{IdSource, Placement};

    /// Two screens, *Home* (holding a rect *Add to cart*) and *Cart* to its right,
    /// and a main *Button* with an instance on *Home*: adopted into a headless app.
    struct F {
        app: MastikApp,
        home: NodeId,
        cart: NodeId,
        add: NodeId,
        main: NodeId,
        inst: NodeId,
    }

    fn fixture(ctx: &egui::Context) -> F {
        let mut app = MastikApp::headless(ctx);
        let mut ids = IdSource::new(0xA7);
        let root = ids.mint();
        let mut doc = Document::new(root);
        let [home, cart, add, main] = [(); 4].map(|_| ids.mint());
        let frame = |w, h| NodeKind::Artboard {
            size: Size::new(w, h),
            corner_radii: Default::default(),
            corner_smoothing: 0.0,
        };
        let create = |id, parent, kind, at: Option<Affine>, name: &str| Operation::CreateNode {
            id,
            parent,
            index: 0,
            kind,
            transform: at,
            name: Some(name.into()),
        };
        doc.apply(&Transaction(vec![
            create(home, root, frame(400.0, 800.0), None, "Home"),
            create(
                cart,
                root,
                frame(400.0, 800.0),
                Some(Affine::translate((600.0, 0.0))),
                "Cart",
            ),
            create(
                add,
                home,
                NodeKind::Rect {
                    size: Size::new(100.0, 40.0),
                    corner_radii: Default::default(),
                    corner_smoothing: 0.0,
                },
                Some(Affine::translate((20.0, 20.0))),
                "Add to cart",
            ),
            create(
                main,
                root,
                frame(80.0, 30.0),
                Some(Affine::translate((0.0, 1200.0))),
                "Button",
            ),
            Operation::SetComponent {
                id: main,
                component: true,
            },
        ]))
        .expect("the screens");
        let (tx, made) = mastik_core::insert_subtrees(
            &doc,
            &mut ids,
            &[Placement {
                nodes: doc.capture_subtree(main).unwrap(),
                parent: home,
                index: None,
            }],
            Vec2::new(20.0, 200.0),
        );
        doc.apply(&tx).expect("an instance");
        app.session.adopt_document(doc, None);
        F {
            app,
            home,
            cart,
            add,
            main,
            inst: made[0],
        }
    }

    /// The `n`th item id of a source of its own, the same on every call.
    fn item(n: u32) -> ItemId {
        let mut s = IdSource::new(0xEE);
        let mut id = s.mint();
        for _ in 1..n {
            id = s.mint();
        }
        ItemId(id)
    }

    fn keyed(n: u32, a: Action) -> Keyed<Action> {
        Keyed::new(item(n), a)
    }

    /// A conditional holding one action in its branch and one in its *else*.
    fn nested() -> Vec<Keyed<Action>> {
        vec![
            keyed(1, Action::Back),
            keyed(
                2,
                Action::Conditional {
                    branches: vec![Branch {
                        when: Expr("true".into()),
                        actions: vec![keyed(3, Action::Back), keyed(4, Action::Back)],
                    }],
                    otherwise: vec![keyed(5, Action::Back)],
                },
            ),
        ]
    }

    /// An action nested in a branch is found, moved within its own list and
    /// removed by id. flip: `shift` without its descent into branches → fails at
    /// "moves inside its branch".
    #[test]
    fn nested_actions_are_reached_by_id() {
        let mut list = nested();
        assert!(find(&list, item(5)).is_some(), "the else's action");
        assert!(shift(&mut list, item(4), true), "moves inside its branch");
        let Action::Conditional { branches, .. } = &list[1].value else {
            panic!("fixture: the conditional");
        };
        assert_eq!(branches[0].actions[0].id, item(4));
        assert!(
            !shift(&mut list, item(4), true),
            "the top of its list stays"
        );
        assert!(remove(&mut list, item(5)));
        assert!(find(&list, item(5)).is_none());
        let at = list_at(&mut list, Place::Otherwise(item(2))).expect("the else");
        assert!(at.is_empty(), "the else is empty once its action goes");
        assert_eq!(position(&list, item(3)), Some((1, 2)));
    }

    /// A dragged action lands at its drop place inside its own branch, counted
    /// without it; dropped where it was, nothing moves.
    /// flip: `move_to` without its descent into branches → fails at "lands inside its branch".
    #[test]
    fn a_dragged_action_lands_in_its_own_list() {
        let mut list = nested();
        assert!(move_to(&mut list, item(4), 0), "lands inside its branch");
        let Action::Conditional { branches, .. } = &list[1].value else {
            panic!("fixture: the conditional");
        };
        assert_eq!(branches[0].actions[0].id, item(4));
        assert!(!move_to(&mut list, item(4), 0), "dropped in place: no move");
        assert!(move_to(&mut list, item(1), 9), "past the end: last");
        assert_eq!(list.last().map(|k| k.id), Some(item(1)));
    }

    /// An expression that does not read is refused with the range of the typed
    /// text at fault, which the field underlines.
    #[test]
    fn a_refused_expression_names_where_it_fails() {
        let doc = Document::new(mastik_core::IdSource::new(1).mint());
        let (_, at) = expr_stored(&doc, "1 + )", None).expect_err("does not read");
        let at = at.expect("a syntax fault has a place");
        assert!(
            at.start >= 2 && at.start <= 4,
            "at the stray bracket: {at:?}"
        );
    }

    /// *Duplicate* mints every id afresh, nested ones too, so the door takes it.
    #[test]
    fn a_duplicate_takes_fresh_ids() {
        let list = nested();
        let mut n = 100;
        let copy = fresh_ids(&list, &mut || {
            n += 1;
            item(n)
        });
        let mut both = list.clone();
        both.extend(copy);
        let r = vec![Keyed::new(
            item(50),
            Reaction {
                trigger: Trigger::Click,
                actions: both,
            },
        )];
        assert!(mastik_core::prototype::check(&r).is_ok());
    }

    /// The field shows a variable by name and stores its id; an unknown name, an
    /// expression that does not read and a value of another kind are refused.
    /// flip: `expr_stored` without its kind check → fails at "a number is not a
    /// string".
    #[test]
    fn an_expression_reads_names_and_stores_ids() {
        let ctx = egui::Context::default();
        let mut f = fixture(&ctx);
        let s = &mut f.app.session;
        let col = s.doc.variables().first_collection().id;
        let (ops, var) = mastik_core::variable::add_variable_ops(
            s.doc.variables(),
            &mut s.ids,
            col,
            VarKind::Number,
            Some("cartCount"),
            None,
        )
        .expect("fixture: a number variable");
        assert!(s.commit(Transaction(ops)), "fixture: committed");
        let doc = &f.app.session.doc;
        let stored = expr_stored(doc, "{cartCount} + 1", Some(VarKind::Number)).expect("reads");
        assert_eq!(stored.0, format!("{{{}}} + 1", var.0.to_wire()));
        assert_eq!(expr_shown(doc, &stored), "{cartCount} + 1");
        assert!(
            expr_stored(doc, "{nope} + 1", None).is_err(),
            "an unknown name"
        );
        assert!(expr_stored(doc, "1 +", None).is_err(), "does not read");
        assert!(
            expr_stored(doc, "{cartCount} + 1", Some(VarKind::String)).is_err(),
            "a number is not a string"
        );
    }

    /// The add menu dims *Run script* under a trigger no person fires and *Change
    /// to* off a component's variant.
    #[test]
    fn the_add_menu_refuses_what_the_layer_cannot_take() {
        let ctx = egui::Context::default();
        let f = fixture(&ctx);
        let doc = &f.app.session.doc;
        let after = Trigger::After { ms: 500 };
        assert_eq!(
            action_refusal(doc, f.add, &after, ActionKind::RunScript),
            Some("A script runs only when someone acts")
        );
        assert_eq!(
            action_refusal(doc, f.add, &Trigger::Click, ActionKind::ChangeState),
            Some("Not a component")
        );
        assert_eq!(
            action_refusal(doc, f.add, &Trigger::Click, ActionKind::Navigate),
            None
        );
        let Some(Action::Navigate { to, .. }) = new_action(doc, f.add, ActionKind::Navigate) else {
            panic!("a navigate");
        };
        assert_eq!(to, f.cart, "another screen than the layer's own");
    }

    /// A link a dot makes takes the page's default transition and joins the
    /// layer's click. flip: `with_link` taking `Transition::default()` → fails at
    /// "the page's default".
    #[test]
    fn a_link_takes_the_pages_transition_and_joins_the_click() {
        let ctx = egui::Context::default();
        let mut f = fixture(&ctx);
        let page = f.app.session.doc.first_page();
        let smart = Transition {
            kind: TransitionKind::SmartAnimate,
            ..Transition::default()
        };
        assert!(
            f.app
                .session
                .commit(Transaction(vec![Operation::SetPagePlay {
                    id: page,
                    play: PagePlay {
                        transition: smart,
                        ..PagePlay::default()
                    },
                }]))
        );
        let mut n = 0;
        let mut mint = || {
            n += 1;
            item(900 + n)
        };
        let once = with_link(&f.app.session.doc, f.add, f.cart, &mut mint);
        assert!(
            f.app
                .session
                .commit(Transaction(vec![Operation::SetReactions {
                    id: f.add,
                    reactions: once,
                }]))
        );
        let twice = with_link(&f.app.session.doc, f.add, f.home, &mut mint);
        assert_eq!(twice.len(), 1, "one click reaction");
        assert_eq!(twice[0].actions.len(), 2, "the second link joins it");
        assert_eq!(
            transition_of(&twice[0].actions[0].value),
            Some(smart),
            "the page's default"
        );
    }

    /// Dropping a dot's link on another screen commits one *Navigate to* as one
    /// step; on the layer's own screen it links nothing. flip: `link_target`
    /// without its own-screen test → fails at "nothing on its own frame".
    #[test]
    fn dropping_a_link_on_a_screen_links_to_it() {
        let ctx = egui::Context::default();
        let mut f = fixture(&ctx);
        f.app.prototype_tab = true;
        f.app.session.selection.set_one(f.add);
        f.app
            .drop_link(f.add, mastik_core::kurbo::Point::new(100.0, 100.0));
        assert!(
            f.app.session.doc.get(f.add).unwrap().reactions().is_empty(),
            "nothing on its own frame"
        );
        f.app
            .drop_link(f.add, mastik_core::kurbo::Point::new(700.0, 100.0));
        let r = f.app.session.doc.get(f.add).unwrap().reactions().to_vec();
        assert!(matches!(
            r.as_slice(),
            [k] if matches!(k.actions.as_slice(), [a] if a.value == Action::Navigate { to: f.cart, transition: Transition::default() })
        ));
        assert!(f.app.session.undo());
        assert!(
            f.app.session.doc.get(f.add).unwrap().reactions().is_empty(),
            "one undo step"
        );
    }

    /// Each edge of the selected layer, and of the hovered one, wears a link dot
    /// at its middle that takes a press; the layer's middle does not. flip:
    /// `knob_layers` without the hovered layer → fails at "the hovered layer's".
    #[test]
    fn every_edge_wears_a_link_dot() {
        let ctx = egui::Context::default();
        let mut f = fixture(&ctx);
        let rect = egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(1200.0, 900.0));
        f.app.prototype_tab = true;
        f.app.session.selection.set_one(f.add);
        let q = f.app.selection_quad(f.add, rect, 1.0).expect("on screen");
        let dots = knob_dots(q);
        assert_eq!(dots.len(), 4, "one dot an edge");
        for (edge, at, out) in &dots {
            let off = *at + *out * (KNOB_PICK - 1.0);
            assert_eq!(f.app.link_knob_at(off, rect, 1.0), Some((f.add, *edge)));
        }
        let middle = quad_box(q).center();
        assert_eq!(
            f.app.link_knob_at(middle, rect, 1.0),
            None,
            "not its middle"
        );
        f.app.session.selection.clear();
        assert_eq!(
            f.app.link_knob_at(dots[0].1, rect, 1.0),
            None,
            "nothing to wear them"
        );
        f.app.proto.knob_hover.set(Some(f.add));
        assert_eq!(
            f.app.link_knob_at(dots[0].1, rect, 1.0),
            Some((f.add, dots[0].0)),
            "the hovered layer's"
        );
        f.app.prototype_tab = false;
        assert_eq!(
            f.app.link_knob_at(dots[0].1, rect, 1.0),
            None,
            "the Design tab's none"
        );
    }

    /// A box in world space, edges 0 to 3 its top, right, bottom and left.
    fn world_box(x: f64, y: f64, w: f64, h: f64) -> WorldQuad {
        use mastik_core::kurbo::Point;
        [
            Point::new(x, y),
            Point::new(x + w, y),
            Point::new(x + w, y + h),
            Point::new(x, y + h),
        ]
    }

    /// A connector leaves by the edge facing its target, a rotated layer's
    /// own edge included, and is drawn in short pieces however long it runs.
    /// flip: `curve` at a fixed 24 pieces → fails at "a short piece".
    #[test]
    fn a_connector_faces_its_target_and_stays_smooth() {
        let from = world_box(0.0, 0.0, 100.0, 40.0);
        let at = |x, y| world_box(x, y, 100.0, 40.0);
        assert_eq!(
            connector_edges(from, at(3000.0, 900.0)),
            Some((1, 3)),
            "right to left"
        );
        assert_eq!(
            connector_edges(from, at(-500.0, 0.0)),
            Some((3, 1)),
            "left to right"
        );
        assert_eq!(
            connector_edges(from, at(20.0, 300.0)),
            Some((2, 0)),
            "down to top"
        );
        // Turned 30°: the edge it leaves by is one of its own, the one facing right.
        let turn = mastik_core::kurbo::Affine::rotate_about(
            30f64.to_radians(),
            mastik_core::kurbo::Point::new(50.0, 20.0),
        );
        let turned = from.map(|p| turn * p);
        let (e, _) = connector_edges(turned, at(3000.0, 0.0)).expect("an edge");
        let out = edge_mid(turned, e) - quad_centre(turned);
        assert!(
            out.normalize().x > 0.8,
            "the turned layer's right-facing edge"
        );
        let a = egui::pos2(100.0, 20.0);
        let pts = curve(a, egui::Vec2::X, egui::pos2(70.0, 300.0), -egui::Vec2::Y);
        let long = curve(a, egui::Vec2::X, egui::pos2(3000.0, 920.0), -egui::Vec2::X);
        for p in [&pts, &long] {
            let worst = p
                .windows(2)
                .map(|w| w[0].distance(w[1]))
                .fold(0.0, f32::max);
            assert!(worst <= CURVE_STEP * 1.5, "a short piece: {worst}");
        }
    }

    /// A stub to nothing on the page leaves by the edge facing out of its
    /// frame, decided in world space so the same at every zoom. flip:
    /// `stub_edge` always answering the right → fails at "out the top".
    #[test]
    fn a_stub_leaves_out_of_its_frame() {
        let frame = Some(quad_centre(world_box(0.0, 0.0, 400.0, 800.0)));
        let layer = |x, y| world_box(x - 30.0, y - 10.0, 60.0, 20.0);
        assert_eq!(stub_edge(layer(200.0, 30.0), frame), Some(0), "out the top");
        assert_eq!(
            stub_edge(layer(40.0, 400.0), frame),
            Some(3),
            "out the left"
        );
        assert_eq!(
            stub_edge(layer(220.0, 780.0), frame),
            Some(2),
            "out the bottom"
        );
        assert_eq!(
            stub_edge(layer(200.0, 400.0), None),
            Some(1),
            "no frame: the right"
        );
    }

    /// A pulled link snaps to the facing side of a screen it would land on,
    /// keeping it while the pointer is anywhere inside, and follows the pointer
    /// out. flip: `pulled_end` answering the pointer over a screen → fails at
    /// "snapped to the screen's facing side".
    #[test]
    fn a_pulled_link_snaps_into_the_screen_under_it() {
        let ctx = egui::Context::default();
        let f = fixture(&ctx);
        let rect = egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(1200.0, 900.0));
        let q = f.app.selection_quad(f.add, rect, 1.0).expect("on screen");
        let (edge, mid, _) = knob_dots(q)[0];
        assert!(edge_distance(q, edge, mid) < 0.01, "the dot is on its edge");
        assert!(edge_distance(q, edge, quad_box(q).center()) > 0.0);
        let cq = f.app.selection_quad(f.cart, rect, 1.0).expect("the cart");
        let cart = quad_box(cq);
        // From the right edge's dot, to the cart's left edge, which faces it.
        let side = screen_edge(cq, 3);
        for p in [cart.center(), cart.right_center() - egui::vec2(4.0, 0.0)] {
            let (b, out_b, ring) = f.app.pulled_end(f.add, 1, p, rect, 1.0);
            assert_eq!((b, out_b), side, "snapped to the screen's facing side");
            assert_eq!(out_b, -egui::Vec2::X);
            assert!(ring.is_some(), "the screen is ringed");
        }
        let outside = cart.right_center() + egui::vec2(60.0, 0.0);
        let (b, out_b, ring) = f.app.pulled_end(f.add, 1, outside, rect, 1.0);
        assert_eq!(
            (b, out_b, ring),
            (outside, egui::Vec2::ZERO, None),
            "out, it follows"
        );
    }

    /// The canvas's connectors: one per navigate, dashed inside a conditional, a
    /// dangling one kept with its gone target.
    #[test]
    fn connectors_follow_the_actions() {
        let ctx = egui::Context::default();
        let mut f = fixture(&ctx);
        let gone = f.app.session.ids.mint();
        let t = Transition::default();
        let actions = vec![
            keyed(
                1,
                Action::Navigate {
                    to: f.cart,
                    transition: t,
                },
            ),
            keyed(
                2,
                Action::Conditional {
                    branches: vec![Branch {
                        when: Expr("true".into()),
                        actions: vec![keyed(
                            3,
                            Action::Navigate {
                                to: gone,
                                transition: t,
                            },
                        )],
                    }],
                    otherwise: Vec::new(),
                },
            ),
        ];
        assert!(
            f.app
                .session
                .commit(Transaction(vec![Operation::SetReactions {
                    id: f.add,
                    reactions: vec![Keyed::new(
                        item(10),
                        Reaction {
                            trigger: Trigger::Click,
                            actions,
                        },
                    )],
                }]))
        );
        let page = f.app.session.doc.first_page();
        let links = links_on(&f.app.session.doc, page);
        assert_eq!(
            links,
            vec![
                Link {
                    from: f.add,
                    to: f.cart,
                    dashed: false,
                    at: ActionAt {
                        node: f.add,
                        reaction: item(10),
                        action: item(1),
                    },
                },
                Link {
                    from: f.add,
                    to: gone,
                    dashed: true,
                    at: ActionAt {
                        node: f.add,
                        reaction: item(10),
                        action: item(3),
                    },
                },
            ]
        );
    }

    /// `action` on *Add to cart* as reaction `item(10)`'s one action `item(1)`,
    /// with the Prototype tab up.
    fn one_action(f: &mut F, action: Action) -> ActionAt {
        assert!(
            f.app
                .session
                .commit(Transaction(vec![Operation::SetReactions {
                    id: f.add,
                    reactions: vec![Keyed::new(
                        item(10),
                        Reaction {
                            trigger: Trigger::Click,
                            actions: vec![keyed(1, action)],
                        },
                    )],
                }]))
        );
        f.app.prototype_tab = true;
        ActionAt {
            node: f.add,
            reaction: item(10),
            action: item(1),
        }
    }

    /// A connector answers a press a few points off its line, not one well
    /// clear of it, and a click on it selects its layer with the action open.
    /// flip: `LINK_PICK` at the line's half-width → fails at "off the line".
    #[test]
    fn a_click_on_a_connector_opens_its_action() {
        let ctx = egui::Context::default();
        let mut f = fixture(&ctx);
        let to = f.cart;
        let at = one_action(
            &mut f,
            Action::Navigate {
                to,
                transition: Transition::default(),
            },
        );
        let rect = egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(1200.0, 900.0));
        let _ = ctx.run_ui(Default::default(), |ui| {
            let painter = ui.painter().clone();
            f.app.draw_prototype_links(ui, &painter, rect, 1.0);
        });
        let mid = {
            let drawn = f.app.proto.links_drawn.borrow();
            assert_eq!(drawn.len(), 1, "fixture: one connector drawn");
            drawn[0].1[drawn[0].1.len() / 2]
        };
        assert_eq!(
            f.app.connector_at(mid + egui::vec2(0.0, 6.0), rect, 1.0),
            Some(at),
            "off the line"
        );
        assert_eq!(
            f.app.connector_at(mid + egui::vec2(0.0, 14.0), rect, 1.0),
            None,
            "well clear of it"
        );
        f.app.session.selection.set(vec![f.cart]);
        f.app.open_link(at);
        assert_eq!(f.app.session.selection.single(), Some(f.add));
        assert_eq!(f.app.proto.open, Some(at));
    }

    /// A manual overlay's ghost sits at its offset from the opening layer's
    /// top-left, at half strength, wearing ids no document holds; a drop writes
    /// the offset in whole points.
    /// flip: `placing_rect` measuring from the page's origin → fails at "by the layer".
    #[test]
    fn a_manual_overlay_is_placed_by_its_ghost() {
        let ctx = egui::Context::default();
        let mut f = fixture(&ctx);
        let overlay = f.cart;
        let at = one_action(
            &mut f,
            Action::OpenOverlay {
                overlay,
                settings: OverlaySettings {
                    position: OverlayPosition::Manual { x: 0.0, y: 48.0 },
                    ..OverlaySettings::default()
                },
                transition: Transition::default(),
            },
        );
        assert!(f.app.overlay_placing().is_none(), "no popover, no ghost");
        f.app.session.selection.set(vec![f.add]);
        f.app.proto.open = Some(at);
        let p = f.app.overlay_placing().expect("placing");
        let r = f.app.placing_rect(&p).expect("a box");
        assert_eq!(
            (r.x0, r.y0, r.width(), r.height()),
            (20.0, 68.0, 400.0, 800.0),
            "by the layer"
        );
        let (_, ghost) = f.app.overlay_ghost().expect("a ghost");
        assert_eq!(ghost.node.transform.translation(), Vec2::new(20.0, 68.0));
        assert_eq!(ghost.node.opacity, PLACING_ALPHA);
        assert!(
            f.app.session.doc.get(ghost.node.id).is_none(),
            "its own ids"
        );
        f.app.drop_overlay(at, Vec2::new(10.4, 30.6));
        let n = f.app.session.doc.get(f.add).expect("the layer");
        let Action::OpenOverlay { settings, .. } = &n.reactions()[0].actions[0].value else {
            panic!("fixture: the overlay action");
        };
        assert_eq!(
            settings.position,
            OverlayPosition::Manual { x: 10.0, y: 31.0 }
        );
    }

    /// A dragged ghost snaps its box to a frame's corner as a moved layer does,
    /// and covers the layer under it: no hover, no link dot.
    /// flip: `snapped_overlay` answering `from + delta` unsnapped → fails at "the
    /// frame's corner"; `pointed_at` without its ghost check → fails at "covered".
    #[test]
    fn a_placed_overlay_snaps_and_covers_what_is_under_it() {
        let ctx = egui::Context::default();
        let mut f = fixture(&ctx);
        let overlay = f.cart;
        let at = one_action(
            &mut f,
            Action::OpenOverlay {
                overlay,
                settings: OverlaySettings {
                    position: OverlayPosition::Manual { x: 0.0, y: 0.0 },
                    ..OverlaySettings::default()
                },
                transition: Transition::default(),
            },
        );
        f.app.canvas_px = (1200, 900);
        f.app.session.camera.zoom = 1.0;
        f.app.session.camera.center = mastik_core::kurbo::Point::new(600.0, 450.0);
        f.app.proto.open = Some(at);
        // From (0, 48) the ghost's box is at (20, 68); dragged 2 and 1 short of
        // Home's top-left.
        let grab = mastik_core::kurbo::Point::new(100.0, 100.0);
        let (to, _) = f.app.snapped_overlay(
            Vec2::new(0.0, 48.0),
            grab,
            grab + Vec2::new(-18.0, -67.0),
            false,
        );
        assert_eq!(to, Vec2::new(-20.0, -20.0), "the frame's corner");
        // *Add to cart*, under the ghost at its saved (0, 0), answers the pointer
        // until the ghost is drawn.
        let rect = egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(1200.0, 900.0));
        let over = f
            .app
            .to_screen(mastik_core::kurbo::Point::new(60.0, 40.0), rect, 1.0);
        let mut seen = Vec::new();
        for frame in 0..4 {
            let input = egui::RawInput {
                screen_rect: Some(rect),
                events: vec![egui::Event::PointerMoved(over)],
                ..Default::default()
            };
            let _ = ctx.run_ui(input, |ui| {
                let resp = ui.interact(rect, ui.id().with("canvas"), egui::Sense::hover());
                let painter = ui.painter().clone();
                seen.push(f.app.pointed_at(ui, rect, 1.0, &resp));
                if frame >= 2 {
                    f.app.draw_placing(ui, &painter, rect, 1.0);
                }
            });
        }
        assert!(seen[2].is_some(), "fixture: a layer under the pointer");
        assert_eq!(seen[3], None, "covered");
        assert!(f.app.link_knob_at(over, rect, 1.0).is_none());
    }

    fn draw(app: &mut MastikApp, ctx: &egui::Context) -> egui::FullOutput {
        let input = egui::RawInput {
            screen_rect: Some(egui::Rect::from_min_size(
                egui::pos2(0.0, 0.0),
                egui::vec2(1400.0, 2400.0),
            )),
            ..Default::default()
        };
        ctx.run_ui(input, |ui| {
            ui.set_max_width(280.0);
            app.inspector_ui(ui)
        })
    }

    /// The tab shows the Prototype cards in place of the Design ones, and the
    /// page's flows with nothing selected.
    #[test]
    fn the_prototype_tab_shows_its_own_cards() {
        let ctx = egui::Context::default();
        let mut f = fixture(&ctx);
        f.app.session.selection.set_one(f.home);
        draw(&mut f.app, &ctx);
        assert!(
            f.app.cards_drawn.contains(&"Fill"),
            "fixture: the Design tab"
        );
        assert!(!f.app.cards_drawn.contains(&"Interactions"));
        f.app
            .set_view_switch(crate::input::ViewSwitch::PrototypeTab, true);
        f.app.cards_drawn.clear();
        draw(&mut f.app, &ctx);
        assert!(f.app.cards_drawn.contains(&"Interactions"));
        assert!(
            f.app.cards_drawn.contains(&"Prototype"),
            "a frame's scroll and overlay"
        );
        assert!(!f.app.cards_drawn.contains(&"Fill"));
        f.app.session.selection.clear();
        f.app.cards_drawn.clear();
        draw(&mut f.app, &ctx);
        assert!(f.app.cards_drawn.contains(&"Flows"), "the page's flows");
    }

    /// An instance whose reaction differs from its main's wears the override dot
    /// on that reaction's row, and the Interactions card counts it. flip:
    /// `card_of` answering `None` for `SetReactions` → fails at "the card counts it".
    #[test]
    fn an_overridden_reaction_wears_the_dot() {
        let ctx = egui::Context::default();
        let mut f = fixture(&ctx);
        let r = |a: Action| {
            vec![Keyed::new(
                item(77),
                Reaction {
                    trigger: Trigger::Click,
                    actions: vec![keyed(78, a)],
                },
            )]
        };
        assert!(
            f.app
                .session
                .commit(Transaction(vec![Operation::SetReactions {
                    id: f.main,
                    reactions: r(Action::Back),
                }]))
        );
        assert_eq!(
            f.app.session.doc.get(f.inst).unwrap().reactions().len(),
            1,
            "fixture: the instance follows its main"
        );
        f.app.prototype_tab = true;
        f.app.session.selection.set_one(f.inst);
        let clean = draw(&mut f.app, &ctx);
        let before = crate::ui::painted_override_dots(&clean).len();
        assert!(
            f.app
                .session
                .commit(Transaction(vec![Operation::SetReactions {
                    id: f.inst,
                    reactions: r(Action::OpenUrl {
                        url: "https://example.com".into(),
                    }),
                }]))
        );
        let out = draw(&mut f.app, &ctx);
        assert!(
            crate::ui::painted_override_dots(&out).len() > before,
            "the reaction's row wears the dot"
        );
        assert_eq!(
            f.app.card_override("Interactions").map(|(n, _)| n),
            Some(1),
            "the card counts it"
        );
    }
}
