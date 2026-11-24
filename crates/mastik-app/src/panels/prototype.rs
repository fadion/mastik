//! Prototype mode's inspector cards (`docs/design/prototyping-authoring.md`): a
//! layer's interactions, its play settings, and with nothing selected the page's
//! flows and prototype settings. Every edit is one transaction on the saved model
//! (`mastik_core::prototype`); nothing here plays.

use super::inspector::{FRAME_PRESETS, carried_unit, identity_card, identity_glyph, interacting};
use super::paint::{self, PaintSlot};
use crate::app::MastikApp;
use crate::preview::Drag;
use crate::theme::{self, color, icon};
use crate::ui::{self, FieldButton, Prefix, Scrub};
use eframe::egui;
use mastik_core::id::NodeId;
use mastik_core::item::{ItemId, Keyed};
use mastik_core::list::DataSetId;
use mastik_core::prototype::expr::{Ref, Unread, table_kind};
use mastik_core::prototype::{
    Action, Branch, CellWrite, DRAWN_POINT, Direction, DragPoint, DropAt, Easing, EffectEnds, Expr,
    Flow, Follow, FollowRange, Going, Input, LayerPlay, ModeScope, OverlayPosition,
    OverlaySettings, PagePlay, Pin, Reaction, Release, RowEnd, Scaling, ScrollAxis, ScrollEffect,
    ScrollProp, Side, Snap, SnapAlign, Timing, Transition, TransitionKind, Trigger, VarChange,
    WhichRow,
};
use mastik_core::variable::{Cell, ModeId, ModeOwner, Switchable, VarId, VarKind};
use mastik_core::{Document, NodeKind, Operation, Transaction};

/// Prototype mode's state between frames: its open popover, a typed field's
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
    /// The unselected layer wearing link dots, noted as the canvas draws so a
    /// press next frame aims at the dots it saw (`note_link_hover`).
    pub knob_hover: std::cell::Cell<Option<NodeId>>,
    /// A manual overlay's X or Y held in its field, so the canvas ghost follows
    /// the scrub before it commits.
    pub held_at: Option<(ItemId, mastik_core::kurbo::Vec2)>,
    /// A snap point's X or Y held in its field, so its canvas ghost follows the
    /// scrub before it commits.
    pub held_point: Option<(ItemId, mastik_core::kurbo::Vec2)>,
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
    /// The open popover was opened by a click on a connector. The canvas runs
    /// before the inspector, so the popover first draws on that click's frame,
    /// where the click would read as one away from it and close it.
    pub opened_on_canvas: bool,
    /// The marks a drag can take (snap points' ghosts, scroll triggers'
    /// lines) as the canvas drew them last frame, in screen space, topmost
    /// last.
    pub marks_drawn: std::cell::RefCell<Vec<(ProtoMark, egui::Rect)>>,
    /// The interactions *Copy interactions* took, with the tab they were copied
    /// in: *Paste interactions* offers them on that tab's document only, since
    /// their targets are that document's layers.
    pub copied: Option<(Option<crate::tabs::TabId>, Vec<Reaction>)>,
    /// The interaction block picked by a click on one of its connectors, by its
    /// layer and reaction: ringed in the card among two or more, and on the
    /// canvas its connectors and scroll line are the layer's only ones drawn
    /// lit. Any other click on the canvas lets it go.
    pub block: Option<(NodeId, ItemId)>,
    /// A library script's copy the popover's edit names, committed by the
    /// next [`MastikApp::write_reactions`] in the same step as that edit.
    pub script_in: Option<Operation>,
}

/// One of prototype mode's marks on the canvas that a drag moves.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) enum ProtoMark {
    /// A snap point of a layer that follows the drag, drawn as a ghost of it.
    Point { node: NodeId, point: ItemId },
    /// The point a scroll trigger fires at, drawn as a line across its frame.
    Past { node: NodeId, reaction: ItemId },
}

/// One action of one layer's reaction, by item ids, nested ones included.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct ActionAt {
    pub node: NodeId,
    pub reaction: ItemId,
    pub action: ItemId,
}

/// An action dragged by its grip up or down its own list (a reaction's, or one
/// branch's), shown at its landing while it is carried, as an Effects row is.
/// `from` and `to` index that list; `slots` are its rows as last laid out.
#[derive(PartialEq)]
pub(crate) struct ActionDrag {
    pub at: ActionAt,
    pub from: usize,
    pub to: usize,
    pub slots: super::paint::Slots,
}

/// What a typed field of prototype mode is for.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub(crate) enum DraftKey {
    /// A conditional's branch condition, or *Set variable*'s value (branch 0).
    Expr(ItemId, usize),
    /// *Open link*'s address.
    Url(ItemId),
    /// A flow's name.
    FlowName(ItemId),
    /// A flow's starting value, by its place in the flow's list.
    FlowValue(ItemId, usize),
    /// A snap point's name: the layer, and the point (`None` where it is drawn).
    PointName(NodeId, Option<ItemId>),
    /// A list filter test's value: the list frame, and the test's place.
    ListTest(NodeId, usize),
}

/// What an expression field stores.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) enum ExprWant {
    /// An expression, of this kind when named.
    Kind(Option<VarKind>),
    /// Literal text when it names no `{…}`, else an expression: a list
    /// filter's value, where `2024-01` and `12` mean what they say.
    LiteralOr,
    /// An expression of any kind, or nothing: *Add row*'s cell, where an empty
    /// field writes an empty cell.
    OrEmpty,
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
    SetMode,
    Conditional,
    RunScript,
    AddRow,
    EditRow,
    DeleteRow,
}

impl ActionKind {
    /// The add menu's groups: where the player is, what floats over it, the
    /// state of the copy it plays, and the copy's tables.
    const GROUPS: [(&'static str, &'static [ActionKind]); 4] = [
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
                ActionKind::SetMode,
                ActionKind::Conditional,
                ActionKind::RunScript,
            ],
        ),
        (
            "Table",
            &[
                ActionKind::AddRow,
                ActionKind::EditRow,
                ActionKind::DeleteRow,
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
            Action::SetMode { .. } => Self::SetMode,
            Action::Conditional { .. } => Self::Conditional,
            Action::OpenUrl { .. } => Self::OpenUrl,
            Action::RunScript { .. } => Self::RunScript,
            Action::AddRow { .. } => Self::AddRow,
            Action::EditRow { .. } => Self::EditRow,
            Action::DeleteRow { .. } => Self::DeleteRow,
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
            Self::SetMode => "Set mode",
            Self::Conditional => "Conditional",
            Self::RunScript => "Run script",
            Self::AddRow => "Add row",
            Self::EditRow => "Edit row",
            Self::DeleteRow => "Delete row",
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
            Self::SetMode => icon::MODE,
            Self::Conditional => icon::CONDITIONAL,
            Self::RunScript => icon::FILE_CODE,
            Self::AddRow => icon::ROWS_PLUS_BOTTOM,
            Self::EditRow => icon::ROWS_PENCIL,
            Self::DeleteRow => icon::TRASH,
        }
    }
}

/// A trigger's words, as its block's head says them.
pub(crate) fn trigger_label(doc: &Document, holder: NodeId, t: &Trigger) -> String {
    match t {
        Trigger::Click => "On click".into(),
        Trigger::HoverEnter => "On hover".into(),
        Trigger::HoverWhile => "While hovering".into(),
        Trigger::HoverLeave => "On leave".into(),
        Trigger::DoubleClick => "On double click".into(),
        Trigger::Press => "On press".into(),
        Trigger::PressWhile => "While pressing".into(),
        Trigger::LongPress { .. } => "On long press".into(),
        Trigger::Drag => "On drag".into(),
        Trigger::Swipe { side } => format!("On swipe {}", side_word(*side)),
        Trigger::Drop { at } => match at {
            DropAt::Anywhere => "On drop anywhere".into(),
            DropAt::Drawn => format!("On drop at {}", drawn_point(doc, holder)),
            DropAt::Point(p) => match drag_point(doc, holder, *p) {
                Some(name) => format!("On drop at {name}"),
                None => "On drop at a missing snap point".into(),
            },
        },
        Trigger::ScrollPast {
            frame: id,
            at,
            going,
        } => {
            let frame = match doc.get(*id) {
                Some(n) => n.name().to_owned(),
                None => "a missing frame".into(),
            };
            let going = match going {
                Going::Forward => "",
                Going::Back if sideways(doc, *id) => " going left",
                Going::Back => " going up",
                Going::Either => " either way",
            };
            format!(
                "When {frame} scrolls past {}{going}",
                ui::number(0)(*at, 0..=0)
            )
        }
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
        Trigger::TableChanges { table } => match doc.data_set(*table) {
            Some(d) => format!("When {} changes", d.name),
            None => "When a missing table changes".into(),
        },
    }
}

/// Why the *Table* actions and *When a table changes* are dimmed.
const NO_TABLES: &str = "This document has no tables";

/// A table dropdown for a row action or *When a table changes*: the
/// document's tables, without *From a file…*, since what it names must already
/// be there. Answers the table picked.
fn row_table_combo(
    ui: &mut egui::Ui,
    doc: &Document,
    salt: impl std::hash::Hash + std::fmt::Debug,
    table: DataSetId,
    w: f32,
) -> Option<DataSetId> {
    use crate::list_data::{TablePick, TableRows, table_combo};
    let face = doc
        .data_set(table)
        .map_or_else(|| "Missing table".to_string(), |d| d.name.clone());
    let rows = TableRows {
        no_file: true,
        ..TableRows::default()
    };
    match table_combo(ui, salt, w, doc, &face, Some(table), &rows) {
        Some(TablePick::Set(t)) => Some(t),
        _ => None,
    }
}

/// Whether a reaction on `holder` fires in a row of a list over `table`: the
/// holder sits in one, or sits in a main one of whose instances does — what
/// *This row* needs.
pub(crate) fn in_list_over(doc: &Document, holder: NodeId, table: DataSetId) -> bool {
    use mastik_core::list;
    let in_row = |id: NodeId| {
        let mut at = Some(id);
        while let Some(n) = at.and_then(|a| doc.get(a)) {
            if let Some((l, _)) = list::row_of(doc, n.id())
                && doc
                    .get(l)
                    .and_then(|l| l.list())
                    .is_some_and(|l| l.data == table)
            {
                return true;
            }
            at = n.parent();
        }
        false
    };
    if in_row(holder) {
        return true;
    }
    let mut at = Some(holder);
    while let Some(n) = at.and_then(|a| doc.get(a)) {
        if n.component() {
            return mastik_core::component::instances_of(doc, n.id())
                .into_iter()
                .any(in_row);
        }
        at = n.parent();
    }
    false
}

/// The name of snap point `p` of the layer `holder` that follows the drag.
fn drag_point(doc: &Document, holder: NodeId, p: ItemId) -> Option<String> {
    let follow = doc.get(holder)?.play().follow.clone()?;
    let point = follow.points.iter().find(|k| k.id == p)?;
    Some(point.value.name.clone())
}

/// The name of the point where `holder` is drawn, as its snap points list it.
fn drawn_point(doc: &Document, holder: NodeId) -> String {
    doc.get(holder)
        .and_then(|n| n.play().follow.as_ref())
        .map_or(DRAWN_POINT, |f| f.drawn_name())
        .to_owned()
}

/// A swipe's side in a block head's words.
fn side_word(side: Side) -> &'static str {
    match side {
        Side::Left => "left",
        Side::Right => "right",
        Side::Up => "up",
        Side::Down => "down",
    }
}

/// Whether `frame` scrolls sideways only, so a scroll trigger on it reads
/// right and left rather than down and up.
fn sideways(doc: &Document, frame: NodeId) -> bool {
    doc.get(frame)
        .is_some_and(|n| n.play().scroll == ScrollAxis::Horizontal)
}

/// The scrolling frames a scroll trigger on `id` can watch: those at or above
/// it, nearest first, then the rest of its screen's.
fn scrolling_frames(doc: &Document, id: NodeId) -> Vec<NodeId> {
    let scrolls = |x: NodeId| {
        doc.get(x).is_some_and(|n| {
            matches!(n.kind(), NodeKind::Artboard { .. }) && n.play().scroll != ScrollAxis::None
        })
    };
    let mut out = Vec::new();
    let mut at = Some(id);
    while let Some(x) = at {
        if scrolls(x) {
            out.push(x);
        }
        at = doc.get(x).and_then(|n| n.parent());
    }
    if let Some(screen) = screen_of(doc, id) {
        for x in pickable_under(doc, screen) {
            if scrolls(x) && !out.contains(&x) {
                out.push(x);
            }
        }
    }
    out
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
const POINTER_TRIGGERS: [Trigger; 10] = [
    Trigger::Click,
    Trigger::DoubleClick,
    Trigger::Press,
    Trigger::PressWhile,
    Trigger::LongPress { ms: LONG_PRESS_MS },
    Trigger::HoverEnter,
    Trigger::HoverWhile,
    Trigger::HoverLeave,
    Trigger::Drag,
    Trigger::Swipe { side: Side::Left },
];

/// How long a new long press is held, in milliseconds.
const LONG_PRESS_MS: u32 = 500;
/// The span of the offset a new scroll effect covers, in units.
const NEW_EFFECT_SPAN: f64 = 160.0;

/// A scroll effect property's name, its field's unit, and the factor its
/// stored value is shown at (opacity and scale as percentages).
fn scroll_prop_words(p: ScrollProp) -> (&'static str, &'static str, f64) {
    match p {
        ScrollProp::Opacity => ("Opacity", "%", 100.0),
        ScrollProp::MoveY => ("Move Y", "px", 1.0),
        ScrollProp::MoveX => ("Move X", "px", 1.0),
        ScrollProp::Scale => ("Scale", "%", 100.0),
        ScrollProp::Rotation => ("Rotation", "°", 1.0),
    }
}

/// A new scroll-effect property's two ends: opacity fades out, so the effect
/// shows at once; the others start and end at rest, to be set.
fn new_effect_ends(p: ScrollProp) -> EffectEnds {
    match p {
        ScrollProp::Opacity => EffectEnds {
            prop: p,
            start: 1.0,
            end: 0.0,
        },
        _ => EffectEnds {
            prop: p,
            start: p.rest(),
            end: p.rest(),
        },
    }
}

/// *With the scroll*'s `+` and the menu of the properties it can add, as wide
/// as its longest word. Answers the one picked.
fn scroll_prop_menu(
    ui: &mut egui::Ui,
    props: &[ScrollProp],
    tip: impl FnOnce(egui::Response) -> egui::Response,
) -> Option<ScrollProp> {
    let plus = tip(ui::field_button(
        ui,
        icon::PLUS,
        ui::CONTROL_H,
        FieldButton::Off,
    ));
    let mut pick = None;
    // Hung as the add-action menu is, from its `+`'s right edge.
    egui::Popup::menu(&plus)
        .align(egui::RectAlign::BOTTOM_END)
        .gap(ui::MENU_GAP)
        .show(|ui| {
            ui::popup_rows(ui);
            let words = props.iter().map(|p| scroll_prop_words(*p).0);
            ui.set_width(ui::menu_width(ui.ctx(), words, false));
            for p in props {
                let row = ui::MenuRow::new("", scroll_prop_words(*p).0);
                if ui::menu_row(ui, row, ui::MENU_ROW_H).clicked() {
                    pick = Some(*p);
                }
            }
        });
    pick
}

/// How far apart new snap points are placed from where a layer is drawn, in
/// units: each new one a step further, to be moved where it belongs.
const NEW_POINT_STEP: f64 = 100.0;

/// The snap point `+` adds to `f`: *Point n*, n the first number no point is
/// named by, n steps from where the layer is drawn along its axis (across for
/// a horizontal one, down otherwise).
fn new_drag_point(f: &Follow) -> DragPoint {
    let taken = |n: usize| {
        f.points
            .iter()
            .any(|p| p.value.name == format!("Point {n}"))
    };
    let n = (1..).find(|n| !taken(*n)).unwrap_or(1);
    let step = NEW_POINT_STEP * n as f64;
    let (x, y) = match f.axis {
        ScrollAxis::Horizontal => (step, 0.0),
        _ => (0.0, step),
    };
    DragPoint {
        name: format!("Point {n}"),
        x,
        y,
    }
}

/// The glyph of a step the layer's Prototype card names `label`: a snap
/// point's own, as moving one on the canvas wears, else the interaction's.
fn play_step_glyph(label: &str) -> &'static str {
    match label {
        "Add snap point" | "Remove snap point" | "Rename snap point" => icon::CROSSHAIR_SIMPLE,
        _ => icon::INTERACTION,
    }
}
/// The offset a new scroll trigger fires at, in units.
const NEW_SCROLL_POINT: f64 = 100.0;

fn same_trigger_kind(a: &Trigger, b: &Trigger) -> bool {
    std::mem::discriminant(a) == std::mem::discriminant(b)
}

/// The transition's word on its action row: its kind alone, so the row keeps
/// room for its target; the timing is in the popover.
pub(crate) fn transition_chip(t: &Transition) -> &'static str {
    transition_short(t.kind)
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

/// A swipe's sides, as the transition's direction names them.
const SIDES: [(Side, &str); 4] = [
    (Side::Left, "Left"),
    (Side::Right, "Right"),
    (Side::Up, "Up"),
    (Side::Down, "Down"),
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

/// The shade overlay action `action` on `node` draws behind its frame, while
/// it darkens.
pub(crate) fn backdrop_of(
    doc: &mastik_core::Document,
    node: NodeId,
    action: ItemId,
) -> Option<mastik_core::peniko::Color> {
    let n = doc.get(node)?;
    let k = n
        .reactions()
        .iter()
        .find_map(|r| find(&r.value.actions, action))?;
    match &k.value {
        Action::OpenOverlay { settings, .. } | Action::SwapOverlay { settings, .. } => {
            settings.backdrop
        }
        _ => None,
    }
}

/// `node`'s reactions rewritten with overlay action `action`'s shade at `c`;
/// `None` when the action is gone or does not darken.
pub(crate) fn backdrop_tx(
    doc: &mastik_core::Document,
    node: NodeId,
    action: ItemId,
    c: mastik_core::peniko::Color,
) -> Option<Transaction> {
    let mut reactions = doc.get(node)?.reactions().to_vec();
    let k = reactions
        .iter_mut()
        .find_map(|r| find_mut(&mut r.value.actions, action))?;
    match &mut k.value {
        Action::OpenOverlay { settings, .. } | Action::SwapOverlay { settings, .. }
            if settings.backdrop.is_some() =>
        {
            settings.backdrop = Some(c);
        }
        _ => return None,
    }
    Some(Transaction(vec![Operation::SetReactions {
        id: node,
        reactions,
    }]))
}

/// The overlay shade `tx` gives one action, against the saved document: what
/// a picker drag on a [`PaintSlot::Backdrop`] holds, since `SetReactions`
/// draws nothing to preview.
///
/// [`PaintSlot::Backdrop`]: super::paint::PaintSlot::Backdrop
pub(crate) fn backdrop_held(
    doc: &mastik_core::Document,
    tx: &Transaction,
) -> Option<(NodeId, ItemId, mastik_core::peniko::Color)> {
    fn walk(list: &[Keyed<Action>], out: &mut Vec<(ItemId, mastik_core::peniko::Color)>) {
        for k in list {
            match &k.value {
                Action::OpenOverlay { settings, .. } | Action::SwapOverlay { settings, .. } => {
                    if let Some(c) = settings.backdrop {
                        out.push((k.id, c));
                    }
                }
                Action::Conditional {
                    branches,
                    otherwise,
                } => {
                    for b in branches {
                        walk(&b.actions, out);
                    }
                    walk(otherwise, out);
                }
                _ => {}
            }
        }
    }
    tx.0.iter().find_map(|op| {
        let Operation::SetReactions { id, reactions } = op else {
            return None;
        };
        let mut shades = Vec::new();
        for r in reactions {
            walk(&r.value.actions, &mut shades);
        }
        shades
            .into_iter()
            .find(|(a, c)| backdrop_of(doc, *id, *a).is_some_and(|was| was != *c))
            .map(|(a, c)| (*id, a, c))
    })
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

/// The pointer, where the canvas's own layer is the one on top: over a card or
/// popup lying on the canvas (the player over it, a menu) nothing on the canvas
/// lights, as `MastikApp::pointed_at` reads it.
fn canvas_pointer(ui: &egui::Ui) -> Option<egui::Pos2> {
    let p = ui.ctx().pointer_latest_pos()?;
    ui.ctx()
        .layer_id_at(p)
        .is_none_or(|l| l == ui.layer_id())
        .then_some(p)
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

/// What an expression field's names may read.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) enum Reads {
    /// Variables only: a list filter's value, read where no table is.
    Vars,
    /// Variables and tables, with the row action's table `{row.…}` reads.
    Tables(Option<DataSetId>),
}

impl Reads {
    fn row(self) -> Option<DataSetId> {
        match self {
            Reads::Vars => None,
            Reads::Tables(row) => row,
        }
    }
}

/// An expression as its field shows it: each variable and table read by name,
/// a name that is gone left as its id.
pub(crate) fn expr_shown(doc: &Document, e: &Expr) -> String {
    mastik_core::prototype::expr_shown(doc, e, None)
}

/// What was typed into an expression field, as stored: each `{name}` written as
/// the id of what it names. Refused, with the reason, when it does not read,
/// names what no one has, or (for *Set variable*, `want`) comes to another
/// kind; a table's cell has its kind only in play, so it is never refused for
/// one. A refusal carries the byte range of the typed text it is about, when
/// the expression names one.
pub(crate) fn expr_stored(
    doc: &Document,
    text: &str,
    want: Option<VarKind>,
    reads: Reads,
) -> Result<Expr, (String, Option<std::ops::Range<usize>>)> {
    let vars = doc.variables();
    let var_id = |n: &str| vars.by_name(n).map(|v| v.id);
    let var_name = |v: VarId| vars.get(v).map(|v| v.name.clone());
    let (e, error) = match reads {
        Reads::Vars => Expr::from_names(text, &var_id),
        Reads::Tables(row) => Expr::from_names_in(
            text,
            &mastik_core::prototype::expr::Names {
                var_id: &var_id,
                var_name: &var_name,
                tables: doc.data_sets(),
                row,
            },
        ),
    };
    if let Some(why) = error {
        return Err((capitalised(&why.to_string()), Some(why.at())));
    }
    if let Some(want) = want {
        // `kind` reads the stored form, whose ranges are not the typed text's.
        let got = e
            .kind(&|r| match r {
                Ref::Var(v) => vars.get(*v).map(|v| Some(v.kind)).ok_or(Unread::Missing),
                r => table_kind(doc.data_sets(), reads.row(), r),
            })
            .map_err(|why| (capitalised(&why.to_string()), None))?;
        if let Some(got) = got
            && got != want
        {
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

/// One row of the list `@` or `{` opens in an expression field.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct AtRow {
    /// The eyebrow it sits under.
    group: &'static str,
    label: String,
    /// The faint words after it: a table's row count, what a word reads.
    note: String,
    /// What replaces the opener and what follows it…
    insert: String,
    /// …in braces when this names something whole; else after the same opener,
    /// for the next step's list (a table's rows and columns, a column's totals).
    done: bool,
}

/// Where `text` ends naming something for the list under its field: the byte
/// offset of a `{` still open at its end (outside quotes, as `Expr` reads
/// them), else of its last `@`; and whether it is the brace.
pub(crate) fn name_opener(text: &str) -> Option<(usize, bool)> {
    let mut quote = None;
    let mut i = 0;
    while let Some(c) = text[i..].chars().next() {
        let next = i + c.len_utf8();
        i = match quote {
            Some(_) if c == '\\' => next + text[next..].chars().next().map_or(0, char::len_utf8),
            Some(q) if c == q => {
                quote = None;
                next
            }
            Some(_) => next,
            None if c == '"' || c == '\'' => {
                quote = Some(c);
                next
            }
            None if c == '{' => match text[next..].find('}') {
                Some(k) => next + k + 1,
                None => return Some((i, true)),
            },
            None => next,
        };
    }
    text.rfind('@').map(|at| (at, false))
}

/// What an expression's `text` may be naming after an open `{` or its last
/// `@` ([`name_opener`]): the variables whose names hold what follows it, then
/// (where `reads` takes tables) the tables; after `<table>.`, what the table
/// holds — its row count, its rows, its columns; after a row, its columns;
/// after a column, its totals. None when neither is open.
pub(crate) fn at_rows(doc: &Document, text: &str, reads: Reads) -> Vec<AtRow> {
    use mastik_core::prototype::expr::{Agg, ROW};
    let Some((at, brace)) = name_opener(text) else {
        return Vec::new();
    };
    // Inside a brace only `}` ends the name, so it may hold spaces and symbols.
    let query = match brace {
        true => text[at + 1..].trim_start(),
        false => &text[at + 1..],
    };
    if !brace && query.contains(|c: char| "{}()+-*/<>=!&|,\"".contains(c)) {
        return Vec::new();
    }
    // After `@` a space ends the name, unless a table's name goes on past it.
    let spaced = |name: &str| query.starts_with(name) || name.starts_with(query);
    let tables = matches!(reads, Reads::Tables(_));
    if !brace
        && query.contains(char::is_whitespace)
        && !(tables && doc.data_sets().iter().any(|t| spaced(&t.name)))
    {
        return Vec::new();
    }
    let row = |group, label: &str, note: String, insert: String, done| AtRow {
        group,
        label: label.to_owned(),
        note,
        insert,
        done,
    };
    let holds = |name: &str, q: &str| name.to_lowercase().contains(&q.to_lowercase());
    let mut out: Vec<AtRow> = doc
        .variables()
        .iter()
        .filter(|v| holds(&v.name, query))
        .map(|v| row("Variables", &v.name, String::new(), v.name.clone(), true))
        .collect();
    let Reads::Tables(row_table) = reads else {
        out.truncate(AT_LIST_MAX);
        return out;
    };
    let sets = doc.data_sets();
    let count = |n: usize| match n {
        1 => "1 row".to_string(),
        n => format!("{n} rows"),
    };
    // The columns of `t`, each named whole after `head` or opening its totals.
    let columns = |t: &mastik_core::list::DataSet, head: &str, rest: &str, whole: bool| {
        t.table
            .columns
            .iter()
            .filter(|c| holds(&c.name, rest))
            .map(|c| {
                let numbers = t.table.is_numeric(c.id);
                let note = match (numbers, whole) {
                    (true, false) => "# · sum min max avg",
                    (true, true) => "#",
                    (false, _) => "Aa",
                };
                let insert = match whole {
                    true => format!("{head}{}", c.name),
                    false => format!("{head}{}.", c.name),
                };
                row("Columns", &c.name, note.into(), insert, whole)
            })
            .collect::<Vec<_>>()
    };
    // `row.` and `<table>.` with what follows them.
    let row_set = row_table.and_then(|r| sets.iter().find(|d| d.id == r));
    if let (Some(t), Some(rest)) = (row_set, query.strip_prefix("row.")) {
        return columns(t, "row.", rest, true);
    }
    if let Some((t, rest)) = sets.iter().find_map(|t| {
        let rest = query.strip_prefix(t.name.as_str())?.strip_prefix('.')?;
        Some((t, rest))
    }) {
        let head = format!("{}.", t.name);
        // After a column: its totals.
        if let Some((c, q)) = rest.rsplit_once('.')
            && t.table.columns.iter().any(|x| x.name == c)
        {
            return Agg::ALL
                .iter()
                .filter(|a| a.word().starts_with(q))
                .map(|a| {
                    let insert = format!("{head}{c}.{}", a.word());
                    row("Totals", a.word(), String::new(), insert, true)
                })
                .collect();
        }
        // After a row: its columns.
        if let Some((sel, q)) = rest.split_once('.') {
            let ok = matches!(sel, "opened" | "first" | "last")
                || sel.parse::<u32>().is_ok_and(|n| n >= 1);
            return match ok {
                true => columns(t, &format!("{head}{sel}."), q, true),
                false => Vec::new(),
            };
        }
        let n = t.table.rows.len();
        let mut words = vec![
            row(
                "Rows",
                "rows",
                format!("{} · how many rows", count(n)),
                format!("{head}rows"),
                true,
            ),
            row(
                "Rows",
                "opened",
                "the opened row".into(),
                format!("{head}opened."),
                false,
            ),
            row(
                "Rows",
                "first",
                "row 1".into(),
                format!("{head}first."),
                false,
            ),
            row(
                "Rows",
                "last",
                format!("row {}", n.max(1)),
                format!("{head}last."),
                false,
            ),
        ];
        if let Ok(k) = rest.parse::<u32>()
            && k >= 1
        {
            words.push(row(
                "Rows",
                rest,
                format!("row {k}"),
                format!("{head}{k}."),
                false,
            ));
        }
        words.retain(|w| w.label.starts_with(rest));
        words.extend(columns(t, &head, rest, false));
        words.truncate(AT_LIST_MAX);
        return words;
    }
    if row_set.is_some() && holds(ROW, query) {
        out.push(row(
            "Tables",
            ROW,
            "the row acted on".into(),
            "row.".into(),
            false,
        ));
    }
    out.extend(sets.iter().filter(|t| holds(&t.name, query)).map(|t| {
        row(
            "Tables",
            &t.name,
            count(t.table.rows.len()),
            format!("{}.", t.name),
            false,
        )
    }));
    out.truncate(AT_LIST_MAX);
    out
}

/// `text` once `row` is picked from its list: the opener and what follows it
/// become `{name}`, or, for a step that names nothing whole yet (a table, a
/// row, a column before its total), the same opener and that step, so the next
/// step's list opens.
fn picked(text: &str, row: &AtRow) -> String {
    let (at, brace) = name_opener(text).unwrap_or((text.len(), false));
    let head = &text[..at];
    match (row.done, brace) {
        (true, _) => format!("{head}{{{}}}", row.insert),
        (false, true) => format!("{head}{{{}", row.insert),
        (false, false) => format!("{head}@{}", row.insert),
    }
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

// --- the document, as prototype mode reads it ------------------------------------

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

/// [`descendants`] a picker offers as a target: a list's copies left out,
/// since every regeneration remakes them under new ids and a picked copy would
/// turn into *Missing layer*; the template's layers stay.
fn pickable_under(doc: &Document, root: NodeId) -> Vec<NodeId> {
    descendants(doc, root)
        .into_iter()
        .filter(|x| mastik_core::list::copy_owner(doc, *x).is_none())
        .collect()
}

/// The instance *Change to* switches from a layer: the instance it is or is in,
/// when that instance's main is a variant of a set; or, inside a main variant,
/// that main, which each of its instances reads as itself (`resolve_target`):
/// how a state that moves on to the next is built.
pub(crate) fn state_instance(doc: &Document, id: NodeId) -> Option<NodeId> {
    if let Some(root) = mastik_core::component::instance_root(doc, id) {
        let main = mastik_core::component::main_of(doc, root)?;
        return mastik_core::variant::set_of(doc, main).map(|_| root);
    }
    let mut at = Some(id);
    while let Some(x) = at {
        let n = doc.get(x)?;
        if n.component() {
            return mastik_core::variant::set_of(doc, x).map(|_| x);
        }
        at = n.parent();
    }
    None
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

/// The states the player switches `id` into with nothing authored: Hover and
/// Pressed, where `id` is a main in a set (or an instance of one) whose
/// interaction-state property ties them, each with the value tied.
fn live_states(doc: &Document, id: NodeId) -> Vec<(&'static str, String)> {
    use mastik_core::variant::{self, Interaction};
    let Some(n) = doc.get(id) else {
        return Vec::new();
    };
    let main = match n.link() {
        Some(m) if mastik_core::component::instance_root(doc, id) == Some(id) => m,
        _ => id,
    };
    let Some(vs) = variant::set_of(doc, main).and_then(|s| doc.get(s)?.set()) else {
        return Vec::new();
    };
    let Some(p) = vs.interaction().map(|i| &vs.props[i]) else {
        return Vec::new();
    };
    [Interaction::Hover, Interaction::Active]
        .into_iter()
        .filter_map(|s| {
            let value = p.values.iter().find(|v| p.state_of(v) == Some(s))?;
            Some((s.label(), value.clone()))
        })
        .collect()
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
        // How it branches: its first condition is the branch head under it.
        Action::Conditional {
            branches,
            otherwise,
        } => {
            let n = branches.len();
            let ways = match n {
                1 => "1 branch".to_string(),
                n => format!("{n} branches"),
            };
            match otherwise.is_empty() {
                true => (ways, None),
                false => (format!("{ways} · else"), None),
            }
        }
        Action::SetMode { mode, scope } => match mode_words(doc, *mode) {
            Some((collection, mode)) => {
                let here = match scope {
                    ModeScope::Everywhere => "",
                    ModeScope::Screen => " · this screen",
                };
                (format!("{collection} to {mode}{here}"), None)
            }
            None => ("Missing mode".into(), Some("Missing mode")),
        },
        Action::OpenUrl { url } => (url.clone(), None),
        Action::RunScript { script } => match doc.script(*script) {
            Some(s) => (s.name.clone(), None),
            None => ("Missing script".into(), Some("Missing script")),
        },
        Action::AddRow { table, .. } => match doc.data_set(*table) {
            Some(d) => (d.name.clone(), None),
            None => ("Missing table".into(), Some("Missing table")),
        },
        Action::EditRow { table, row, .. } | Action::DeleteRow { table, row } => {
            match doc.data_set(*table) {
                None => ("Missing table".into(), Some("Missing table")),
                Some(_) if *row == WhichRow::This && !in_list_over(doc, holder, *table) => {
                    ("No list row".into(), Some("No list row"))
                }
                Some(d) => (format!("{} · {}", d.name, row_short(row)), None),
            }
        }
        Action::Back | Action::CloseOverlay { .. } => (String::new(), None),
    }
}

/// The row a row action acts on, as its row in the card says it.
fn row_short(row: &WhichRow) -> &'static str {
    match row {
        WhichRow::This => "This row",
        WhichRow::Opened => "Opened row",
        WhichRow::FirstWhere { .. } => "First where…",
    }
}

/// The collections *Set mode* and a flow's starting modes can switch: the
/// document's with two modes or more, then each kit's.
fn switchable_collections(doc: &Document) -> Vec<Switchable<'_>> {
    doc.variables().switchable()
}

/// A mode's collection and its own name, `None` for a mode not there.
fn mode_words(doc: &Document, mode: ModeId) -> Option<(String, String)> {
    let t = doc.variables();
    if let Some(c) = t.collection_of_mode(mode) {
        let m = c.modes.iter().find(|m| m.id == mode)?;
        return Some((c.name.clone(), m.name.clone()));
    }
    let s = t
        .switchable()
        .into_iter()
        .find(|s| s.modes.iter().any(|(m, _)| *m == mode))?;
    let name = s.modes.iter().find(|(m, _)| *m == mode)?.1.to_owned();
    Some((s.name, name))
}

/// The collection combo of *Set mode* and a flow's starting mode: the
/// document's collections, then each kit's under its name. Answers the pick.
fn collection_combo<'a>(
    ui: &mut egui::Ui,
    doc: &Document,
    salt: impl std::hash::Hash + std::fmt::Debug,
    w: f32,
    cols: &'a [Switchable<'a>],
    at: Option<&Switchable<'_>>,
) -> Option<&'a Switchable<'a>> {
    let shown = at.map_or("Missing collection".to_string(), |c| c.name.clone());
    let face = ui::glyph_and_text(icon::MODE, &shown);
    let mut pick = None;
    ui::fitted_combo(ui, salt, w, face, &shown, |ui| {
        ui::menu_rows(ui);
        let mut kit = None;
        for c in cols {
            if let Some(k) = c.owner.kit()
                && kit != Some(k)
            {
                kit = Some(k);
                ui::menu_eyebrow(ui, &super::binding::kit_name(doc, k));
            }
            let on = at.is_some_and(|x| x.owner == c.owner);
            if ui.selectable_label(on, &c.name).clicked() && !on {
                pick = Some(c);
            }
        }
    });
    pick
}

/// The scalings in menu order, each with the caption that tells them apart:
/// the scaled ones first, the one laid out again last.
const SCALINGS: [(Scaling, &str, &str); 4] = [
    (Scaling::Fit, "Fit", "the screen scaled to the room"),
    (
        Scaling::FillWidth,
        "Fill width",
        "scaled to the room's width",
    ),
    (Scaling::ActualSize, "Actual size", "one unit a pixel"),
    (
        Scaling::Responsive,
        "Responsive",
        "laid out at the room's size",
    ),
];

/// A scaling's name.
pub(crate) fn scaling_label(s: Scaling) -> &'static str {
    SCALINGS
        .iter()
        .find(|(x, ..)| *x == s)
        .map_or("Fit", |(_, l, _)| l)
}

/// A scaling dropdown, each row's caption in the accelerator's place. With
/// `page`, a first row *Page's own · ‹its scaling›* stands for `None` (the
/// export card's). Answers a pick that differs from `current`.
pub(crate) fn scaling_combo(
    ui: &mut egui::Ui,
    salt: impl std::hash::Hash + std::fmt::Debug,
    w: f32,
    page: Option<Scaling>,
    current: Option<Scaling>,
) -> Option<Option<Scaling>> {
    let own = page.map(|p| format!("Page's own · {}", scaling_label(p)));
    let shown = match (current, &own) {
        (Some(s), _) => scaling_label(s).to_owned(),
        (None, Some(o)) => o.clone(),
        (None, None) => "Fit".to_owned(),
    };
    let mut pick = None;
    ui::fitted_combo(ui, salt, w, ui::combo_text(&shown), &shown, |ui| {
        ui::menu_rows(ui);
        if let Some(o) = &own {
            let on = current.is_none();
            if ui::menu_row(ui, ui::MenuRow::new("", o).selected(on), ui::MENU_ROW_H).clicked()
                && !on
            {
                pick = Some(None);
            }
        }
        for (s, label, caption) in SCALINGS {
            let on = current == Some(s);
            let row = ui::MenuRow::new("", label).note(caption).selected(on);
            if ui::menu_row(ui, row, ui::MENU_ROW_H).clicked() && !on {
                pick = Some(Some(s));
            }
        }
    })
    .response
    .on_hover_text("How the screen fits the player");
    pick
}

/// The transition a new link on `page` takes: the page's own default.
pub(crate) fn page_transition(doc: &Document, page: Option<NodeId>) -> Transition {
    page.and_then(|p| doc.page(p))
        .map_or_else(Transition::default, |p| p.play.transition)
}

/// A new branch's condition: the first boolean variable, which reads as a
/// question still to be set, or `true` in a document with none.
fn first_condition(doc: &Document) -> Expr {
    doc.variables()
        .iter()
        .find(|v| v.kind == VarKind::Boolean)
        .map_or_else(|| Expr("true".into()), |v| keeps(v.id))
}

/// *First row where…*'s first test: the first column's cell against empty
/// text, which reads as a question still to be set; `true` with no column.
fn first_row_test(columns: &[(mastik_core::chart::ColumnId, String)]) -> Expr {
    match columns.first() {
        Some((c, _)) => Expr(format!("{{{}}} == ''", Ref::Row(*c).wire())),
        None => Expr("true".into()),
    }
}

/// *Flow n*, n the list's length on, the first number no flow is named by.
fn free_flow_name(flows: &[Keyed<Flow>]) -> String {
    let mut n = flows.len() + 1;
    while flows.iter().any(|f| f.name == format!("Flow {n}")) {
        n += 1;
    }
    format!("Flow {n}")
}

/// *Set variable*'s first value: the variable itself, which reads and changes
/// nothing until it is typed over. An empty one would refuse the export.
fn keeps(var: VarId) -> Expr {
    Expr(format!("{{{}}}", var.0.to_wire()))
}

/// A new action of `kind` for a reaction on `holder`, its target the likeliest
/// one: another screen, the holder's own instance, the first variable or script.
/// `None` where the document has nothing it could name.
pub(crate) fn new_action(doc: &Document, holder: NodeId, kind: ActionKind) -> Option<Action> {
    let transition = page_transition(doc, doc.page_of(holder));
    let own = screen_of(doc, holder);
    // The frames in the order a new target is sought: the holder's own page
    // first, from the screen after its own round to the one before it, then
    // the other pages', so a new link draws as a connector, not a stub.
    let other_screen = || {
        let page = doc.page_of(holder);
        let groups = screens(doc);
        let here: Vec<NodeId> = groups
            .iter()
            .filter(|(p, _)| Some(*p) == page)
            .flat_map(|(_, f)| f.iter().copied())
            .collect();
        let mut all: Vec<NodeId> = match own.and_then(|s| here.iter().position(|f| *f == s)) {
            Some(i) => here[i + 1..].iter().chain(&here[..i]).copied().collect(),
            None => here,
        };
        all.extend(
            groups
                .iter()
                .filter(|(p, _)| Some(*p) != page)
                .flat_map(|(_, f)| f.iter().copied()),
        );
        // A main is a component, not a screen, so it is picked last.
        let main = |f: &NodeId| doc.get(*f).is_some_and(|n| n.component());
        all.iter()
            .copied()
            .find(|f| Some(*f) != own && !main(f))
            .or_else(|| all.iter().copied().find(|f| Some(*f) != own))
            .or(own)
    };
    Some(match kind {
        ActionKind::Navigate => Action::Navigate {
            to: other_screen()?,
            transition,
            keep_scroll: false,
        },
        ActionKind::Back => Action::Back,
        // The first layer in the nearest frame that scrolls: one that can move.
        ActionKind::ScrollTo => Action::ScrollTo {
            layer: scrolling_frames(doc, holder)
                .into_iter()
                .find_map(|f| doc.get(f)?.children().first().copied())?,
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
                VarKind::String | VarKind::Color => VarChange::Set { to: keeps(v.id) },
            };
            Action::SetVariable { var: v.id, change }
        }
        ActionKind::SetMode => {
            // The first such collection, at the mode it does not read now.
            let c = switchable_collections(doc).into_iter().next()?;
            let now = own.and_then(|s| doc.mode_of_at(s, c.owner));
            let mode = c.modes.iter().find(|m| Some(m.0) != now)?.0;
            Action::SetMode {
                mode,
                scope: ModeScope::Everywhere,
            }
        }
        ActionKind::Conditional => Action::Conditional {
            branches: vec![Branch {
                when: first_condition(doc),
                actions: Vec::new(),
            }],
            otherwise: Vec::new(),
        },
        ActionKind::RunScript => Action::RunScript {
            script: doc.scripts().first()?.id,
        },
        // The table of the list the holder is in, else the first; the row the
        // holder's list row is, where it is in one.
        ActionKind::AddRow | ActionKind::EditRow | ActionKind::DeleteRow => {
            let sets = doc.data_sets();
            let table = sets
                .iter()
                .find(|d| in_list_over(doc, holder, d.id))
                .or_else(|| sets.first())?
                .id;
            let row = match in_list_over(doc, holder, table) {
                true => WhichRow::This,
                false => WhichRow::Opened,
            };
            match kind {
                ActionKind::AddRow => Action::AddRow {
                    table,
                    at: RowEnd::End,
                    cells: Vec::new(),
                },
                ActionKind::EditRow => Action::EditRow {
                    table,
                    row,
                    cells: Vec::new(),
                },
                _ => Action::DeleteRow { table, row },
            }
        }
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
        ActionKind::ChangeState if state_instance(doc, holder).is_none() => {
            Some("Only a component with variants changes state")
        }
        ActionKind::SetVariable if doc.variables().iter().next().is_none() => {
            Some("This document has no variables")
        }
        ActionKind::SetMode if switchable_collections(doc).is_empty() => {
            Some("No collection has a second mode")
        }
        ActionKind::AddRow | ActionKind::EditRow | ActionKind::DeleteRow
            if doc.data_sets().is_empty() =>
        {
            Some(NO_TABLES)
        }
        ActionKind::ScrollTo if new_action(doc, holder, kind).is_none() => {
            Some(match scrolling_frames(doc, holder).is_empty() {
                true => "No frame on the screen scrolls",
                false => "No frame that scrolls holds a layer",
            })
        }
        ActionKind::Navigate | ActionKind::OpenOverlay | ActionKind::SwapOverlay
            if new_action(doc, holder, kind).is_none() =>
        {
            Some("No frame to go to")
        }
        _ => None,
    }
}

/// The connectors of one action list of reaction `reaction` on `holder`; a
/// nested (`conditional`) list's are dotted.
fn layer_links(
    doc: &Document,
    holder: NodeId,
    reaction: ItemId,
    list: &[Keyed<Action>],
    conditional: bool,
    out: &mut Vec<Link>,
) {
    for k in list {
        match &k.value {
            Action::Navigate { to: t, .. }
            | Action::OpenOverlay { overlay: t, .. }
            | Action::SwapOverlay { overlay: t, .. } => {
                let line = match k.value {
                    _ if conditional => LinkLine::Dotted,
                    Action::Navigate { .. } => LinkLine::Solid,
                    _ => LinkLine::Dashed,
                };
                out.push(Link {
                    from: holder,
                    to: mastik_core::prototype::resolve_target(doc, holder, *t),
                    line,
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

/// One connector the canvas draws while prototype mode is on.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct Link {
    pub from: NodeId,
    /// The target as the holder reads it; it may be gone.
    pub to: NodeId,
    pub line: LinkLine,
    /// The action it draws, which a click on it opens.
    pub at: ActionAt,
}

/// How a connector's line is drawn: what it says about when it is followed.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) enum LinkLine {
    /// A navigate, always taken.
    Solid,
    /// An overlay opened or swapped over the screen.
    Dashed,
    /// Inside a conditional, taken only when its branch is: an overlay's too.
    Dotted,
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

/// What a canvas link dot's drop commits: `reactions` on `id` with a click
/// navigating to `to`. A first click reaction that already navigates is
/// pointed at `to`, as a new link replaces the old; else the navigation joins
/// that reaction, or a new one. Answers the list, the action's place, and
/// whether it was retargeted rather than added.
pub(crate) fn with_link(
    doc: &Document,
    id: NodeId,
    to: NodeId,
    mint: &mut impl FnMut() -> ItemId,
) -> (Vec<Keyed<Reaction>>, ActionAt, bool) {
    let mut list = doc
        .get(id)
        .map(|n| n.reactions().to_vec())
        .unwrap_or_default();
    let at = |reaction, action| ActionAt {
        node: id,
        reaction,
        action,
    };
    if let Some(r) = list.iter_mut().find(|r| r.trigger == Trigger::Click) {
        let rid = r.id;
        let old = r.value.actions.iter_mut().find_map(|k| match &mut k.value {
            Action::Navigate { to: t, .. } => Some((k.id, t)),
            _ => None,
        });
        if let Some((aid, t)) = old {
            *t = to;
            return (list, at(rid, aid), true);
        }
        let aid = mint();
        r.value
            .actions
            .push(Keyed::new(aid, navigate_to(doc, id, to)));
        return (list, at(rid, aid), false);
    }
    let (rid, aid) = (mint(), mint());
    list.push(Keyed::new(
        rid,
        Reaction {
            trigger: Trigger::Click,
            actions: vec![Keyed::new(aid, navigate_to(doc, id, to))],
        },
    ));
    (list, at(rid, aid), false)
}

/// A new *Navigate to* `to` from `holder`, at its page's default transition.
fn navigate_to(doc: &Document, holder: NodeId, to: NodeId) -> Action {
    Action::Navigate {
        to,
        transition: page_transition(doc, doc.page_of(holder)),
        keep_scroll: false,
    }
}

// --- the canvas ---------------------------------------------------------------

/// A link dot's radius at rest and under the pointer: a size up from a shape's
/// corner dots, so the two handles do not read as one.
const KNOB_R: f32 = 5.0;
const KNOB_HOT_R: f32 = 6.0;
/// How far after a frame's name tag the dot of a connector leaving from the
/// tag is centred.
const TAG_DOT_GAP: f32 = 4.0;
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
    cubic(a, a + out_a * reach, b + out_b * reach, b)
}

/// How far above the higher of the two frames a link going back crests.
const BACK_RISE: f32 = 32.0;

/// A link going back, from `a` to the top edge's middle `b`, arched so its crest
/// is at `crest` (screen y, above both frames): up, across, and down into `b`.
fn arch(a: egui::Pos2, b: egui::Pos2, crest: f32) -> Vec<egui::Pos2> {
    // A cubic's middle is (a + 3·c1 + 3·c2 + b) / 8: both controls at `y` put
    // its middle at the crest.
    let y = (8.0 * crest - a.y - b.y) / 6.0;
    cubic(a, egui::pos2(a.x, y), egui::pos2(b.x, y), b)
}

/// The cubic through `a` and `b` with controls `c1` and `c2`, in pieces no
/// longer than [`CURVE_STEP`].
fn cubic(a: egui::Pos2, c1: egui::Pos2, c2: egui::Pos2, b: egui::Pos2) -> Vec<egui::Pos2> {
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
    line: LinkLine,
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
        match line {
            LinkLine::Solid => {
                painter.add(egui::Shape::line(pts, stroke));
            }
            LinkLine::Dashed => painter.extend(egui::Shape::dashed_line(&pts, stroke, 6.0, 4.0)),
            // Round dots the line's width across, a dash's length apart.
            LinkLine::Dotted => {
                painter.extend(egui::Shape::dotted_line(&pts, ink, 6.0, w / 2.0));
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

/// The gap a flow chip keeps above its frame.
const FLOW_CHIP_GAP: f32 = 6.0;
/// The most variables the list `@` opens in an expression shows.
const AT_LIST_MAX: usize = 12;
/// The springs a timing offers by name, in the order the dropdown lists them.
/// *Quick* is the one a new spring takes, the drag settle's default too.
const SPRING_PRESETS: [(&str, Timing); 4] = [
    ("Gentle", spring(1.0, 120.0, 20.0)),
    ("Quick", spring(1.0, 300.0, 30.0)),
    ("Bouncy", spring(1.0, 400.0, 12.0)),
    ("Slow", spring(1.0, 80.0, 18.0)),
];
const DEFAULT_SPRING: usize = 1;

const fn spring(mass: f64, stiffness: f64, damping: f64) -> Timing {
    Timing::Spring {
        mass,
        stiffness,
        damping,
    }
}

/// How far under the layer that opens it a *Manual* overlay starts, in units.
const MANUAL_OVERLAY_GAP: f64 = 8.0;
/// The width of the accent edge on the page's first flow's name field in the
/// Flows card.
const FIRST_FLOW_BAR: f32 = 3.0;
/// How much of the badges' blue a chip other than the page's first flow's
/// keeps, so the flow Play opens stands out.
const FLOW_CHIP_QUIET: f32 = 0.55;

impl MastikApp {
    /// The flow chips on the current page while prototype mode is on: each
    /// flow's name over its start frame's top-right corner, which plays it.
    /// Each chip's flow, rect and words; a frame starting several flows stacks
    /// them leftward.
    pub(crate) fn flow_chips(
        &self,
        ui: &egui::Ui,
        rect: egui::Rect,
        ppp: f32,
    ) -> Vec<(mastik_core::prototype::FlowRef, egui::Rect, String)> {
        if !self.prototype_mode() {
            return Vec::new();
        }
        let doc = &self.session.doc;
        let page = self.session.page;
        let Some(info) = doc.page(page) else {
            return Vec::new();
        };
        let mut out: Vec<(mastik_core::prototype::FlowRef, egui::Rect, String)> = Vec::new();
        for f in &info.flows {
            let Some(frame) = self.selection_quad(f.start, rect, ppp).map(quad_box) else {
                continue;
            };
            let words = crate::private::flow(&f.name, out.len()).into_owned();
            let size = ui::canvas_pill_glyph_size(ui.ctx(), icon::PLAY, &words);
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
                egui::pos2(right - size.x, frame.top() - FLOW_CHIP_GAP - size.y),
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

    /// Draw the flow chips: the play glyph and the flow's name on a pill, lifted
    /// under the pointer as the crop's *Done* is. The page's first flow, which
    /// Play opens, wears the canvas badges' blue; the others a quieter
    /// [`FLOW_CHIP_QUIET`] of it.
    fn draw_flow_chips(&self, ui: &egui::Ui, painter: &egui::Painter, rect: egui::Rect, ppp: f32) {
        let hover = ui.ctx().pointer_hover_pos();
        let first = self
            .session
            .doc
            .page(self.session.page)
            .and_then(|p| p.flows.first().map(|f| f.id));
        for (r, chip, words) in self.flow_chips(ui, rect, ppp) {
            let lit = hover.is_some_and(|p| chip.contains(p));
            let ground = match Some(r.flow) == first {
                true => color::BADGE,
                false => color::BADGE.gamma_multiply(FLOW_CHIP_QUIET),
            };
            let hue = match lit {
                true => ground.blend(egui::Color32::from_white_alpha(26)),
                false => ground,
            };
            ui::canvas_pill_glyph(painter, chip, icon::PLAY, &words, hue);
        }
    }

    /// The layers wearing link dots, while prototype mode is on and a
    /// selecting tool is in hand: the one selected layer, then the hovered one.
    fn knob_layers(&self) -> Vec<NodeId> {
        if !self.prototype_mode() || !self.tool.selects() {
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
        let live = self.prototype_mode() && self.tool.selects() && self.hover_chrome_live();
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

    /// The layer the hover outline rings in prototype mode: the one whose
    /// connector is under the pointer, which a click selects; else the one
    /// wearing link dots, while the pointer is on one of them outside it.
    pub(crate) fn link_hover(&self) -> Option<NodeId> {
        if !self.prototype_mode() {
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
        if !self.prototype_mode() || !self.connector_live(screen, rect, ppp) {
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
        self.proto.opened_on_canvas = true;
        self.proto.block = Some((at.node, at.reaction));
    }

    /// Delete or Backspace while an action's popover is open: that action goes,
    /// as one *Remove action* step, rather than the layer holding it, which is
    /// what a click on its connector selected. Answers whether it took the key.
    pub(crate) fn remove_open_action(&mut self) -> bool {
        let Some(at) = self.proto.open.filter(|_| self.prototype_mode()) else {
            return false;
        };
        let Some(mut reactions) = self
            .session
            .doc
            .get(at.node)
            .map(|n| n.reactions().to_vec())
        else {
            return false;
        };
        let Some(r) = reactions.iter_mut().find(|r| r.id == at.reaction) else {
            return false;
        };
        if !remove(&mut r.value.actions, at.action) {
            return false;
        }
        self.proto.open = None;
        self.write_reactions(at.node, reactions, "Remove action", icon::TRASH);
        true
    }

    /// The interaction block of `node` picked on the canvas, while it still has it.
    fn picked_block(&self, node: NodeId) -> Option<ItemId> {
        let (n, r) = self.proto.block.filter(|(n, _)| *n == node)?;
        let has = self
            .session
            .doc
            .get(n)
            .is_some_and(|x| x.reactions().iter().any(|k| k.id == r));
        has.then_some(r)
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
        if !self.prototype_mode() {
            self.proto.ghost_rect.set(None);
            self.proto.link_hot.set(None);
            self.proto.links_drawn.borrow_mut().clear();
            self.proto.marks_drawn.borrow_mut().clear();
            return;
        }
        self.draw_proto_marks(ui, painter, rect, ppp);
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
            // A block picked in the card lights its own and dims the layer's others.
            let lit = self.session.selection.contains(link.from)
                && self
                    .picked_block(link.from)
                    .is_none_or(|b| b == link.at.reaction);
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
            // A frame's own *After delay* leaves from its name tag.
            let strip = self
                .leaves_from_tag(&link)
                .then(|| self.frame_label(ui, link.from, rect, ppp))
                .flatten()
                .map(|(r, _)| egui::pos2(r.right() + TAG_DOT_GAP, r.center().y));
            let (ink, pts, warning) = match ends {
                Some(((ea, eb), to)) => match self.goes_back(&link, rect, ppp) {
                    // Back across the page: up from the layer's top edge, over
                    // the frames, down onto the target's.
                    Some(crest) => {
                        let a = strip.unwrap_or_else(|| screen_edge(from, 0).0);
                        (blue, arch(a, screen_edge(to, 0).0, crest), None)
                    }
                    None => {
                        let (a, out_a) = match strip {
                            Some(a) => (a, egui::Vec2::X),
                            None => screen_edge(from, ea),
                        };
                        let (b, out_b) = screen_edge(to, eb);
                        (blue, curve(a, out_a, b, out_b), None)
                    }
                },
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
        let hot = canvas_pointer(ui)
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
            paint_link(painter, pts.clone(), link.line, ink, w);
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
                LinkLine::Solid,
                color::SELECT,
                LINK_W,
            );
        }
        // The dots near the pointer, filled where it would take one or one is
        // pulled, as a shape's dragged corner dot is.
        let hot =
            pulled.or_else(|| canvas_pointer(ui).and_then(|p| self.link_knob_at(p, rect, ppp)));
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

    /// How far past frame `id`'s name tag its connector's dot reaches: in
    /// prototype mode, while one of its own *After delay* links leaves from the
    /// tag; zero otherwise. What else hangs after the tag (an annotation's mark)
    /// starts beyond it.
    pub(crate) fn tag_dot_reach(&self, id: NodeId) -> f32 {
        if !self.prototype_mode() {
            return 0.0;
        }
        let from_tag = links_on(&self.session.doc, self.session.page)
            .iter()
            .any(|l| l.from == id && self.leaves_from_tag(l));
        match from_tag {
            // The dot's ring is 1.5 wide, half of it outside its radius.
            true => TAG_DOT_GAP + KNOB_R + 1.0,
            false => 0.0,
        }
    }

    /// Whether `link` is a top-level frame's own *After delay*, which leaves
    /// from the frame's name tag rather than an edge.
    fn leaves_from_tag(&self, link: &Link) -> bool {
        let doc = &self.session.doc;
        let Some(n) = doc.get(link.from) else {
            return false;
        };
        n.parent() == Some(self.session.page)
            && is_frame(doc, link.from)
            && n.reactions()
                .iter()
                .any(|r| r.id == link.at.reaction && matches!(r.trigger, Trigger::After { .. }))
    }

    /// When `link` goes back across the page, to a frame whose middle lies left
    /// of the frame it leaves, the screen y its arch crests at: [`BACK_RISE`]
    /// above the higher of the two frames' tops.
    fn goes_back(&self, link: &Link, rect: egui::Rect, ppp: f32) -> Option<f32> {
        let doc = &self.session.doc;
        let frame = screen_of(doc, link.from)?;
        let target = screen_of(doc, link.to).unwrap_or(link.to);
        if frame == target {
            return None;
        }
        let (fq, tq) = (self.world_quad(frame)?, self.world_quad(target)?);
        let left = fq.iter().map(|p| p.x).fold(f64::INFINITY, f64::min);
        if quad_centre(tq).x >= left {
            return None;
        }
        let (fs, ts) = (
            self.selection_quad(frame, rect, ppp)?,
            self.selection_quad(link.to, rect, ppp)?,
        );
        let top = fs
            .iter()
            .chain(ts.iter())
            .map(|p| p.y)
            .fold(f32::INFINITY, f32::min);
        Some(top - BACK_RISE)
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
        let (list, at, moved) = with_link(&session.doc, from, to, &mut || session.ids.mint_item());
        let target = name(&self.session.doc, to);
        let label = if moved {
            "Change action"
        } else {
            "Add interaction"
        };
        self.write_reactions(from, list, label, icon::INTERACTION);
        // Open where it can be set further, as a click on a connector does.
        if self.session.doc.get(from).is_some_and(|n| {
            n.reactions()
                .iter()
                .any(|r| r.id == at.reaction && find(&r.actions, at.action).is_some())
        }) {
            self.proto.open = Some(at);
        }
        self.session.info(format!("Linked to “{target}”"));
    }

    /// The overlay being placed by hand: while prototype mode is on and the
    /// open popover is an *Open overlay* or *Swap overlay* at *Manual* on a layer
    /// of this page. Its offset is the one being dragged or typed, else the saved.
    pub(crate) fn overlay_placing(&self) -> Option<Placing> {
        if !self.prototype_mode() {
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
        self.prototype_mode()
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

/// How far either side of a scroll trigger's line a press takes it, in screen
/// points: the line is a hairline, so it needs a band to be grabbed.
const PAST_REACH: f32 = 4.0;
/// The dash of prototype mode's marks: lines that stand for a place, not ink.
const MARK_DASH: f32 = 4.0;
const MARK_GAP: f32 = 3.0;

impl MastikApp {
    /// `id`'s box in world space, bounding its quad.
    fn world_bounds(&self, id: NodeId) -> Option<mastik_core::kurbo::Rect> {
        let q = self.world_quad(id)?;
        let r = mastik_core::kurbo::Rect::from_points(q[0], q[0]);
        Some(q.iter().fold(r, |r, p| r.union_pt(*p)))
    }

    /// The layer whose marks show: the one selected, in prototype mode.
    fn marked_layer(&self) -> Option<NodeId> {
        self.prototype_mode()
            .then(|| self.session.selection.single())
            .flatten()
    }

    /// The value `mark` holds: a snap point's offset, or a scroll trigger's
    /// point in `x`; a drag's under way, else one held in its field, else the
    /// saved one.
    pub(crate) fn proto_mark_value(&self, mark: ProtoMark) -> Option<mastik_core::kurbo::Vec2> {
        if let Drag::ProtoMark { mark: m, now, .. } = self.drag
            && m == mark
        {
            return Some(now);
        }
        if let ProtoMark::Point { point, .. } = mark
            && let Some((_, v)) = self.proto.held_point.filter(|(p, _)| *p == point)
        {
            return Some(v);
        }
        let doc = &self.session.doc;
        match mark {
            ProtoMark::Point { node, point } => {
                let f = doc.get(node)?.play().follow.as_ref()?;
                let p = &f.points.iter().find(|k| k.id == point)?.value;
                Some(mastik_core::kurbo::Vec2::new(p.x, p.y))
            }
            ProtoMark::Past { node, reaction } => {
                let n = doc.get(node)?;
                let r = n.reactions().iter().find(|r| r.id == reaction)?;
                match &r.value.trigger {
                    Trigger::ScrollPast { at, .. } => Some(mastik_core::kurbo::Vec2::new(*at, 0.0)),
                    _ => None,
                }
            }
        }
    }

    /// The selected layer's snap points as ghosts: each point's mark, its name,
    /// and the box the layer has there, in world space.
    fn snap_ghosts(&self) -> Vec<(ProtoMark, String, mastik_core::kurbo::Rect)> {
        let Some(id) = self.marked_layer() else {
            return Vec::new();
        };
        let doc = &self.session.doc;
        let (Some(f), Some(b)) = (
            doc.get(id).and_then(|n| n.play().follow.clone()),
            self.world_bounds(id),
        ) else {
            return Vec::new();
        };
        f.points
            .iter()
            .filter_map(|k| {
                let mark = ProtoMark::Point {
                    node: id,
                    point: k.id,
                };
                Some((mark, k.value.name.clone(), b + self.proto_mark_value(mark)?))
            })
            .collect()
    }

    /// The selected layer's scroll triggers as lines, only the picked block's
    /// when one is: each one's mark, its frame's box in world space, its
    /// point, and whether the frame scrolls sideways.
    fn past_lines(&self) -> Vec<(ProtoMark, mastik_core::kurbo::Rect, f64, bool)> {
        let Some(id) = self.marked_layer() else {
            return Vec::new();
        };
        let doc = &self.session.doc;
        let Some(n) = doc.get(id) else {
            return Vec::new();
        };
        let picked = self.picked_block(id);
        n.reactions()
            .iter()
            .filter(|r| picked.is_none_or(|b| b == r.id))
            .filter_map(|r| {
                let Trigger::ScrollPast { frame, .. } = &r.value.trigger else {
                    return None;
                };
                let frame = mastik_core::prototype::resolve_target(doc, id, *frame);
                let mark = ProtoMark::Past {
                    node: id,
                    reaction: r.id,
                };
                let at = self.proto_mark_value(mark)?.x;
                Some((mark, self.world_bounds(frame)?, at, sideways(doc, frame)))
            })
            .collect()
    }

    /// The selected scrolling frame's rest points as lines across it, in world
    /// space: each child's edge it rests on (by *Align*), or each whole page's
    /// edge through its content.
    fn snap_edges(&self) -> Vec<[mastik_core::kurbo::Point; 2]> {
        use mastik_core::kurbo::Point;
        let Some(id) = self.marked_layer() else {
            return Vec::new();
        };
        let doc = &self.session.doc;
        let Some(n) = doc.get(id) else {
            return Vec::new();
        };
        let play = n.play();
        let Some(fr) = self.world_bounds(id) else {
            return Vec::new();
        };
        if play.snap == Snap::None {
            return Vec::new();
        }
        let kids: Vec<mastik_core::kurbo::Rect> = n
            .children()
            .iter()
            .filter(|c| {
                doc.get(**c)
                    .is_some_and(|c| c.visible() && c.play().pin != Pin::Fixed)
            })
            .filter_map(|c| self.world_bounds(*c))
            .collect();
        let mut out = Vec::new();
        for across in [true, false] {
            let on = match across {
                true => matches!(play.scroll, ScrollAxis::Horizontal | ScrollAxis::Both),
                false => matches!(play.scroll, ScrollAxis::Vertical | ScrollAxis::Both),
            };
            if !on {
                continue;
            }
            let line = |at: f64| match across {
                true => [Point::new(at, fr.y0), Point::new(at, fr.y1)],
                false => [Point::new(fr.x0, at), Point::new(fr.x1, at)],
            };
            match play.snap {
                Snap::None => {}
                Snap::Children => {
                    for k in &kids {
                        let (a, b) = match across {
                            true => (k.x0, k.x1),
                            false => (k.y0, k.y1),
                        };
                        out.push(line(match play.snap_align {
                            SnapAlign::Start => a,
                            SnapAlign::Centre => (a + b) / 2.0,
                            SnapAlign::End => b,
                        }));
                    }
                }
                Snap::Pages => {
                    let (start, size) = match across {
                        true => (fr.x0, fr.width()),
                        false => (fr.y0, fr.height()),
                    };
                    let end = kids
                        .iter()
                        .map(|k| if across { k.x1 } else { k.y1 })
                        .fold(start + size, f64::max);
                    let mut at = start + size;
                    while size > 0.0 && at < end && out.len() < 200 {
                        out.push(line(at));
                        at += size;
                    }
                }
            }
        }
        out
    }

    /// Prototype mode's marks for the selected layer: a scrolling frame's rest
    /// edges; each snap point as a dashed ghost of the layer's box with its name;
    /// each scroll trigger's point as a dashed line across its frame, a knob at
    /// its leading end and the point on a tag at the other. Notes what a press
    /// can take for the next frame.
    fn draw_proto_marks(&self, ui: &egui::Ui, painter: &egui::Painter, rect: egui::Rect, ppp: f32) {
        let hover = canvas_pointer(ui);
        let dragged = match self.drag {
            Drag::ProtoMark { mark, .. } => Some(mark),
            _ => None,
        };
        let dashed = |pts: &[egui::Pos2], stroke| {
            painter.extend(egui::Shape::dashed_line(pts, stroke, MARK_DASH, MARK_GAP));
        };
        for [a, b] in self.snap_edges() {
            let pts = [self.to_screen(a, rect, ppp), self.to_screen(b, rect, ppp)];
            dashed(&pts, egui::Stroke::new(1.0, color::SELECT_DIM));
        }
        let mut drawn = Vec::new();
        for (mark, name, r) in self.snap_ghosts() {
            let s = self.world_rect_to_screen(r, rect, ppp);
            let hot = dragged == Some(mark) || hover.is_some_and(|p| s.contains(p));
            let stroke = egui::Stroke::new(if hot { 2.0 } else { 1.0 }, color::SELECT);
            let corners = [
                s.left_top(),
                s.right_top(),
                s.right_bottom(),
                s.left_bottom(),
                s.left_top(),
            ];
            dashed(&corners, stroke);
            let at = s.left_top() - egui::vec2(0.0, 2.0);
            ui::canvas_pill(painter, at, egui::Align2::LEFT_BOTTOM, &name, color::BADGE);
            drawn.push((mark, s));
        }
        for (mark, fr, at, side) in self.past_lines() {
            use mastik_core::kurbo::Point;
            let (a, b) = match side {
                true => (Point::new(fr.x0 + at, fr.y0), Point::new(fr.x0 + at, fr.y1)),
                false => (Point::new(fr.x0, fr.y0 + at), Point::new(fr.x1, fr.y0 + at)),
            };
            let (a, b) = (self.to_screen(a, rect, ppp), self.to_screen(b, rect, ppp));
            let band = egui::Rect::from_two_pos(a, b).expand(PAST_REACH);
            let hot = dragged == Some(mark) || hover.is_some_and(|p| band.contains(p));
            let stroke = egui::Stroke::new(if hot { 2.0 } else { 1.0 }, color::SELECT);
            dashed(&[a, b], stroke);
            // Painted as a link dot is, at rest and taken.
            match hot {
                true => painter.circle(
                    a,
                    KNOB_HOT_R,
                    color::SELECT,
                    egui::Stroke::new(1.5, color::POINT),
                ),
                false => painter.circle(
                    a,
                    KNOB_R,
                    color::POINT,
                    egui::Stroke::new(1.5, color::SELECT),
                ),
            };
            let (tag_at, align) = match side {
                true => (b + egui::vec2(0.0, 4.0), egui::Align2::CENTER_TOP),
                false => (b + egui::vec2(4.0, 0.0), egui::Align2::LEFT_CENTER),
            };
            let words = ui::number(0)(at, 0..=0);
            ui::canvas_pill(painter, tag_at, align, &words, color::BADGE);
            drawn.push((mark, band));
        }
        *self.proto.marks_drawn.borrow_mut() = drawn;
    }

    /// The mark a press at `screen` takes, as the canvas drew them last frame,
    /// the topmost first.
    pub(crate) fn proto_mark_at(&self, screen: egui::Pos2) -> Option<ProtoMark> {
        if !self.prototype_mode() {
            return None;
        }
        let drawn = self.proto.marks_drawn.borrow();
        let hit = drawn.iter().rev().find(|(_, r)| r.contains(screen));
        hit.map(|(m, _)| *m)
            .filter(|m| self.proto_mark_value(*m).is_some())
    }

    /// Where a mark dragged from `grab` to `world` goes from `from`, and the
    /// marks snapping draws. A ghost keeps to its layer's axis (Shift to the
    /// drag's, under *Both*) and snaps as a moved box does; a line moves along
    /// its frame's scroll from 0 and snaps to its frame's children's leading
    /// edges, Alt holding that off. Whole points, as both are stored.
    pub(crate) fn moved_proto_mark(
        &self,
        mark: ProtoMark,
        grab: mastik_core::kurbo::Point,
        from: mastik_core::kurbo::Vec2,
        world: mastik_core::kurbo::Point,
        shift: bool,
        alt: bool,
    ) -> (mastik_core::kurbo::Vec2, crate::snap::Marks) {
        use mastik_core::kurbo::Vec2;
        let raw = world - grab;
        let doc = &self.session.doc;
        let whole = |v: Vec2| Vec2::new(v.x.round(), v.y.round());
        match mark {
            ProtoMark::Point { node, .. } => {
                let axis = doc
                    .get(node)
                    .and_then(|n| n.play().follow.as_ref().map(|f| f.axis))
                    .unwrap_or_default();
                let axes = match axis {
                    ScrollAxis::Horizontal => (true, false),
                    ScrollAxis::Vertical => (false, true),
                    _ => crate::snap::move_axes(raw, shift),
                };
                let delta = crate::snap::constrain(raw, axes);
                let Some(b) = self.world_bounds(node) else {
                    return (whole(from + delta), crate::snap::Marks::default());
                };
                let snapped =
                    crate::snap::snap_rect(b + from + delta, axes, &self.snapping(&[node]));
                (
                    whole(from + delta + crate::snap::constrain(snapped.adjust, axes)),
                    snapped.marks,
                )
            }
            ProtoMark::Past { node, reaction } => {
                let frame = doc
                    .get(node)
                    .and_then(|n| n.reactions().iter().find(|r| r.id == reaction))
                    .and_then(|r| match &r.value.trigger {
                        Trigger::ScrollPast { frame, .. } => {
                            Some(mastik_core::prototype::resolve_target(doc, node, *frame))
                        }
                        _ => None,
                    });
                let Some((frame, fr)) = frame.and_then(|f| Some((f, self.world_bounds(f)?))) else {
                    return (from, crate::snap::Marks::default());
                };
                let side = sideways(doc, frame);
                let along = |p: mastik_core::kurbo::Point| if side { p.x } else { p.y };
                let start = if side { fr.x0 } else { fr.y0 };
                let mut at = from.x + if side { raw.x } else { raw.y };
                // To a child's leading edge, so *past the hero* is one drag.
                if !alt {
                    let reach = self.snapping(&[]).tolerance();
                    let edges = doc
                        .get(frame)
                        .map(|n| n.children().to_vec())
                        .unwrap_or_default();
                    let near = edges
                        .iter()
                        .filter_map(|c| self.world_bounds(*c))
                        .map(|b| along(b.origin()) - start)
                        .filter(|e| (e - at).abs() <= reach)
                        .min_by(|a, b| (a - at).abs().total_cmp(&(b - at).abs()));
                    if let Some(e) = near {
                        at = e;
                    }
                }
                (
                    Vec2::new(at.round().max(0.0), 0.0),
                    crate::snap::Marks::default(),
                )
            }
        }
    }

    /// A mark's drop: a snap point's new offset (*Move snap point*) or a scroll
    /// trigger's new point (*Change trigger*), one step.
    pub(crate) fn drop_proto_mark(&mut self, mark: ProtoMark, now: mastik_core::kurbo::Vec2) {
        let doc = &self.session.doc;
        match mark {
            ProtoMark::Point { node, point } => {
                let Some(mut play) = doc.get(node).map(|n| n.play().clone()) else {
                    return;
                };
                let Some(p) = play
                    .follow
                    .as_mut()
                    .and_then(|f| f.points.iter_mut().find(|k| k.id == point))
                else {
                    return;
                };
                if (p.value.x, p.value.y) == (now.x, now.y) {
                    return;
                }
                (p.value.x, p.value.y) = (now.x, now.y);
                self.session
                    .name_step("Move snap point", icon::CROSSHAIR_SIMPLE);
                self.commit_edit(Transaction(vec![Operation::SetPlay { id: node, play }]));
            }
            ProtoMark::Past { node, reaction } => {
                let Some(n) = doc.get(node) else { return };
                let mut list = n.reactions().to_vec();
                let Some(r) = list.iter_mut().find(|r| r.id == reaction) else {
                    return;
                };
                let Trigger::ScrollPast { at, .. } = &mut r.value.trigger else {
                    return;
                };
                if *at == now.x {
                    return;
                }
                *at = now.x;
                self.write_reactions(node, list, "Change trigger", icon::INTERACTION);
            }
        }
    }
}

/// A manual overlay being placed on the canvas: the action placing it, the frame
/// it opens, and its offset from the opening layer's top-left.
#[derive(Clone, Copy, Debug)]
pub(crate) struct Placing {
    pub at: ActionAt,
    pub frame: NodeId,
    pub offset: mastik_core::kurbo::Vec2,
}

// --- prototype mode -----------------------------------------------------------

/// The popover's width.
const POP_W: f32 = crate::app::POPOVER_W;
/// Between two controls on one line.
const COL: f32 = ui::CARD_COL_GAP;
/// An action's indent under its trigger, and a nested one's under its branch head.
const NEST: f32 = 16.0;
/// What *Back* does: its row's tooltip, since it has no popover to say it in.
const BACK_NOTE: &str = "Goes back a screen; with an overlay open, closes it";
/// A branch head's height, one line of its caption, and how much of the rows'
/// gap it takes back above and below, so it sits close to the rows it heads.
const BRANCH_HEAD_H: f32 = 14.0;
const BRANCH_TUCK: f32 = 4.0;
/// How far outside its rows the picked interaction block's ring sits: inside
/// the card's margin, and clear of the rule between two blocks.
const BLOCK_RING_GAP: f32 = 4.0;

impl MastikApp {
    /// Prototype mode is on for the document in front: the inspector shows its
    /// cards in place of the Design cards. Each tab keeps its own, and the
    /// library remembers it as it does the camera, so a document reopens in it.
    pub(crate) fn prototype_mode(&self) -> bool {
        self.session.prototype_mode
    }

    /// Turn prototype mode on or off for the document in front, say so, and
    /// remember it for the document. A view change: the file is untouched.
    pub(crate) fn set_prototype_mode(&mut self, on: bool) {
        if self.prototype_mode() != on {
            self.close_inspector_popovers();
            self.proto.capture = None;
            self.session.prototype_mode = on;
            self.remember_prototype_mode();
        }
        self.session.info(match on {
            true => "Prototype mode on",
            false => "Prototype mode off",
        });
    }

    /// Record the front document's prototype mode in the library's local index,
    /// by its id; a document never filed has none and is not remembered.
    pub(crate) fn remember_prototype_mode(&mut self) {
        let Some(id) = self.session.doc.meta().id.clone() else {
            return;
        };
        let local = &mut self.library.local;
        let changed = match self.session.prototype_mode {
            true => local.prototype_mode.insert(id),
            false => local.prototype_mode.remove(&id),
        };
        if changed {
            local.save();
        }
    }

    /// Put a just-opened document back in prototype mode if it was left in it
    /// here.
    pub(crate) fn reopen_prototype_mode(&mut self) {
        self.session.prototype_mode = self
            .session
            .doc
            .meta()
            .id
            .as_ref()
            .is_some_and(|id| self.library.local.prototype_mode.contains(id));
    }

    /// Prototype mode's cards for what is selected.
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
                self.live_states_card(ui, id);
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

    /// The layer's identity in prototype mode: its glyph and name, and what it
    /// is, without the Design cards' rows.
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
                // The note first, from the right, so a long name is cut in what
                // it leaves rather than widening the inspector.
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    ui.label(
                        egui::RichText::new(note)
                            .size(ui::CAPTION_PT)
                            .color(theme::text::faint()),
                    );
                    ui.with_layout(egui::Layout::left_to_right(egui::Align::Center), |ui| {
                        ui.add(
                            egui::Label::new(
                                egui::RichText::new(title)
                                    .size(ui::HEAD_PT)
                                    .color(theme::text::strong()),
                            )
                            .truncate(),
                        );
                    });
                });
            });
            self.flow_start_row(ui, id);
        });
    }

    /// Under a screen's name in prototype mode: the flows that start on it, or
    /// *Start a flow here*, which adds one there as the Flows card's `+` does
    /// from the page's first frame.
    fn flow_start_row(&mut self, ui: &mut egui::Ui, id: NodeId) {
        let doc = &self.session.doc;
        let Some(page) = doc.page_of(id) else { return };
        if doc.get(id).and_then(|n| n.parent()) != Some(page) || !is_frame(doc, id) {
            return;
        }
        let flows = doc.page(page).map(|p| p.flows.clone()).unwrap_or_default();
        let here: Vec<&str> = flows
            .iter()
            .filter(|f| f.start == id)
            .map(|f| f.name.as_str())
            .collect();
        if !here.is_empty() {
            let words = match here.as_slice() {
                [one] => format!("Starts the flow {one}"),
                many => format!("Starts the flows {}", many.join(", ")),
            };
            ui::selection_note(ui, &words);
            return;
        }
        if crate::settings::text_button(ui, "Start a flow here", FieldButton::Off)
            .on_hover_text("Add a flow that opens on this frame")
            .clicked()
        {
            let mut list = flows;
            list.push(Keyed::new(
                self.session.ids.mint_item(),
                Flow {
                    name: free_flow_name(&list),
                    start: id,
                    modes: Vec::new(),
                    values: Vec::new(),
                },
            ));
            self.session.name_step("Add flow", icon::INTERACTION);
            self.commit_edit(Transaction(vec![Operation::SetFlows {
                id: page,
                flows: list,
            }]));
        }
    }

    /// The *Interaction states* card of a main in a set, or an instance of one,
    /// whose set ties Hover or Pressed to a value: each plays live in the
    /// player with nothing authored, which the card says so no one writes
    /// *While hovering → Change to* for it.
    fn live_states_card(&mut self, ui: &mut egui::Ui, id: NodeId) {
        let rows = live_states(&self.session.doc, id);
        if rows.is_empty() {
            return;
        }
        self.panel(ui, "Interaction states", None, |_, ui| {
            for (state, value) in &rows {
                let w = ui.available_width();
                let rect = ui
                    .allocate_ui_with_layout(
                        egui::vec2(w, ui::CONTROL_H),
                        egui::Layout::left_to_right(egui::Align::Center),
                        |ui| {
                            ui.label(
                                egui::RichText::new(*state)
                                    .size(ui::CONTROL_PT)
                                    .color(theme::text::strong()),
                            );
                            ui.with_layout(
                                egui::Layout::right_to_left(egui::Align::Center),
                                |ui| {
                                    ui.label(
                                        egui::RichText::new("plays live")
                                            .size(ui::CAPTION_PT)
                                            .color(theme::text::faint()),
                                    );
                                },
                            );
                        },
                    )
                    .response
                    .rect;
                // A scope's response is never hovered; the row's own area is.
                ui.interact(
                    rect,
                    ui.id().with(("live-state", *state)),
                    egui::Sense::hover(),
                )
                .on_hover_text(format!(
                    "The player shows {value} while it is {}, with no interaction",
                    match *state {
                        "Hover" => "hovered",
                        _ => "pressed",
                    }
                ));
            }
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
        let script_in = self.proto.script_in.take();
        if let Err(why) = mastik_core::prototype::check(&reactions) {
            self.session.info(format!("Not changed: {why}"));
            return;
        }
        let mut ops: Vec<Operation> = script_in.into_iter().collect();
        match ops.is_empty() {
            true => self.session.name_step(label, glyph),
            false => self
                .session
                .name_step("Copy script into document", icon::FILE_CODE),
        }
        ops.push(Operation::SetReactions { id, reactions });
        self.commit_edit(Transaction(ops));
    }

    /// The layer *Copy interactions* lifts from: the key layer, or the lone one,
    /// when it has any interactions.
    pub(crate) fn interactions_source(&self) -> Option<NodeId> {
        let sel = &self.session.selection;
        let id = sel.key().or_else(|| sel.single())?;
        let has = self
            .session
            .doc
            .get(id)
            .is_some_and(|n| !n.reactions().is_empty());
        has.then_some(id)
    }

    /// Whether *Paste interactions* has something to paste in the tab in front.
    pub(crate) fn interactions_copied(&self) -> bool {
        self.proto
            .copied
            .as_ref()
            .is_some_and(|(tab, _)| *tab == self.tabs.active)
    }

    /// *Copy interactions*: keep the source layer's interactions, apart from the
    /// clipboard, for *Paste interactions* in this tab.
    pub(crate) fn copy_interactions(&mut self) {
        let Some(id) = self.interactions_source() else {
            return;
        };
        let list: Vec<Reaction> = self.session.doc.get(id).map_or_else(Vec::new, |n| {
            n.reactions().iter().map(|k| k.value.clone()).collect()
        });
        self.session.info(match list.len() {
            1 => "Copied 1 interaction".to_string(),
            n => format!("Copied {n} interactions"),
        });
        self.proto.copied = Some((self.tabs.active, list));
    }

    /// *Paste interactions*: add the copied interactions after each selected
    /// layer's own, with fresh item ids, as one step.
    pub(crate) fn paste_interactions(&mut self) {
        if !self.interactions_copied() {
            return;
        }
        let Some((_, copied)) = self.proto.copied.clone() else {
            return;
        };
        let ids = self.session.selection.ids().to_vec();
        let mut ops = Vec::new();
        for id in ids {
            let Some(node) = self.session.doc.get(id) else {
                continue;
            };
            let mut list = node.reactions().to_vec();
            for r in &copied {
                let mut mint = || self.session.ids.mint_item();
                let copy = Reaction {
                    trigger: r.trigger.clone(),
                    actions: fresh_ids(&r.actions, &mut mint),
                };
                list.push(Keyed::new(mint(), copy));
            }
            if let Err(why) = mastik_core::prototype::check(&list) {
                self.session.info(format!("Not pasted: {why}"));
                return;
            }
            ops.push(Operation::SetReactions {
                id,
                reactions: list,
            });
        }
        if ops.is_empty() {
            return;
        }
        self.session
            .name_step("Paste interactions", icon::INTERACTION);
        self.commit_edit(Transaction(ops));
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
                    ui::empty_note(
                        ui,
                        "Drag a dot on the layer's edge onto a frame, or add an interaction with +",
                    );
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
                    let top = ui.cursor().top();
                    let out = app.reaction_block(ui, id, &reactions, r, state);
                    let block = egui::Rect::from_x_y_ranges(
                        ui.max_rect().x_range(),
                        top..=ui.min_rect().bottom(),
                    );
                    // Marked only among several: with one there is nothing to
                    // tell it from.
                    if reactions.len() > 1 && app.picked_block(id) == Some(r.id) {
                        ui::paint_quiet_ring(
                            ui.painter(),
                            block.expand(BLOCK_RING_GAP),
                            ui::BUTTON_R,
                        );
                    }
                    if let Some(o) = out.write {
                        next = Some(o);
                    }
                    if out.reset {
                        reset_one = Some(r.id);
                    }
                }
                for gone in ghosts {
                    let label = trigger_label(&app.session.doc, id, &gone.trigger);
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
        // A drag whose list this card no longer draws lands nothing and ends at
        // the release, so it cannot stay in flight.
        if self.in_flight.action_drag.is_some() && !ui.input(|i| i.pointer.primary_down()) {
            self.in_flight.action_drag = None;
        }
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
        match next {
            Some((list, label, glyph)) => self.write_reactions(id, list, label, glyph),
            // A copy prepared this frame with no edit to carry it is dropped.
            None => self.proto.script_in = None,
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
            let slot = match state {
                Some(_) => super::component::ITEM_SLOT_W + COL,
                None => 0.0,
            };
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
                    let w = ui::menu_width(ui.ctx(), ["Duplicate", "Remove"], true);
                    ui.set_width(w);
                    // The glyph the app's own Duplicate wears.
                    dup =
                        ui::menu_item(ui, icon::COPY_SIMPLE, "Duplicate", ui::MENU_ROW_H).clicked();
                    ui::menu_sep(ui);
                    delete = ui::menu_row(
                        ui,
                        ui::MenuRow::new(icon::TRASH, "Remove").danger(true),
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
                out.write = Some((list, "Duplicate interaction", icon::COPY_SIMPLE));
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
            Trigger::LongPress { ms } => {
                let mut v = f64::from(*ms);
                let resp_id = ui.id().with(("hold", r.id));
                if let Some(x) = self.proto.scrub.filter(|(i, _)| *i == resp_id) {
                    v = x.1;
                }
                let resp = ui::value_field_suffixed(
                    ui,
                    egui::vec2(ui.available_width(), h),
                    Prefix::Text("Hold"),
                    Some(ui::Suffix {
                        text: "ms",
                        clickable: false,
                        tooltip: "",
                    }),
                    &mut v,
                    Scrub::whole(10.0).range(100.0..=10_000.0),
                    |d| d.custom_formatter(ui::number(0)),
                )
                .0
                .on_hover_text("How long the pointer is held down before the actions run");
                let ms_new = v.round().clamp(100.0, 10_000.0) as u32;
                let tx = match ms_new != *ms {
                    true => Transaction(vec![Operation::SetReactions {
                        id,
                        reactions: with(&|x: &mut Reaction| {
                            x.trigger = Trigger::LongPress { ms: ms_new }
                        }),
                    }]),
                    false => Transaction::default(),
                };
                self.proto_valve(resp_id, v, &resp, "Change trigger", icon::INTERACTION, tx);
            }
            Trigger::Drop { at } => {
                let at = *at;
                let follow = self
                    .session
                    .doc
                    .get(id)
                    .and_then(|n| n.play().follow.clone());
                // Only a layer that snaps rests at a point it can be told by.
                let snaps = follow.as_ref().is_some_and(|f| f.release == Release::Snaps);
                let mut options = vec![(DropAt::Anywhere, "Anywhere".to_string())];
                if snaps {
                    options.push((DropAt::Drawn, drawn_point(&self.session.doc, id)));
                    for p in follow.iter().flat_map(|f| &f.points) {
                        options.push((DropAt::Point(p.id), p.value.name.clone()));
                    }
                }
                // Named as the head names it, offered or not.
                let shown = match at {
                    DropAt::Anywhere => "Anywhere".to_string(),
                    DropAt::Drawn => drawn_point(&self.session.doc, id),
                    DropAt::Point(p) => drag_point(&self.session.doc, id, p)
                        .unwrap_or_else(|| "Missing snap point".into()),
                };
                let mut pick = None;
                // Bare, as a trigger's other choices are: the glyph says a point.
                let w = ui.available_width();
                let face = ui::glyph_and_text(icon::CROSSHAIR_SIMPLE, &shown);
                ui::fitted_combo(ui, ("drop-at", r.id), w, face, &shown, |ui| {
                    ui::menu_rows(ui);
                    for (o, n) in &options {
                        let row = ui::MenuRow::new("", n).selected(*o == at);
                        if ui::menu_row(ui, row, ui::MENU_ROW_H).clicked() && *o != at {
                            pick = Some(*o);
                        }
                    }
                })
                .response
                .on_hover_text("Where it is let go for the actions to run");
                if let Some(at) = pick {
                    out.write = Some((
                        with(&|x: &mut Reaction| x.trigger = Trigger::Drop { at }),
                        "Change trigger",
                        icon::INTERACTION,
                    ));
                }
            }
            Trigger::Swipe { side } => {
                let cur = SIDES.iter().position(|(s, _)| s == side).unwrap_or(0);
                let pick = ui::segmented_tipped(
                    ui,
                    ui.available_width(),
                    ui::SEGMENT_CELL_H,
                    SIDES.len(),
                    cur,
                    |_| true,
                    |_| "The side the swipe goes toward",
                    |p, i, r, on| ui::segment_label(p, r, SIDES[i].1, on),
                );
                if let Some(i) = pick.filter(|i| *i != cur) {
                    let side = SIDES[i].0;
                    out.write = Some((
                        with(&|x: &mut Reaction| x.trigger = Trigger::Swipe { side }),
                        "Change trigger",
                        icon::INTERACTION,
                    ));
                }
            }
            Trigger::ScrollPast { frame, at, going } => {
                let (frame, at, going) = (*frame, *at, *going);
                let w = ui.available_width();
                let options = scrolling_frames(&self.session.doc, id);
                let shown = mastik_core::prototype::resolve_target(&self.session.doc, id, frame);
                let doc = &self.session.doc;
                if let Some(f) = layer_combo(
                    ui,
                    doc,
                    ("past-frame", r.id),
                    Some(icon::FRAME),
                    shown,
                    &options,
                    w,
                ) {
                    out.write = Some((
                        with(&|x: &mut Reaction| {
                            x.trigger = Trigger::ScrollPast {
                                frame: f,
                                at,
                                going,
                            }
                        }),
                        "Change trigger",
                        icon::INTERACTION,
                    ));
                }
                let mut v = at;
                let resp_id = ui.id().with(("past", r.id));
                if let Some(x) = self.proto.scrub.filter(|(i, _)| *i == resp_id) {
                    v = x.1;
                }
                let resp = ui::value_field_suffixed(
                    ui,
                    egui::vec2(w, h),
                    Prefix::Text("Past"),
                    Some(ui::Suffix {
                        text: "px",
                        clickable: false,
                        tooltip: "",
                    }),
                    &mut v,
                    Scrub::whole(1.0).range(0.0..=100_000.0),
                    |d| d.custom_formatter(ui::number(0)),
                )
                .0
                .on_hover_text("How far the frame has scrolled into its content");
                let at_new = v.round().clamp(0.0, 100_000.0);
                let tx = match at_new != at {
                    true => Transaction(vec![Operation::SetReactions {
                        id,
                        reactions: with(&|x: &mut Reaction| {
                            x.trigger = Trigger::ScrollPast {
                                frame,
                                at: at_new,
                                going,
                            }
                        }),
                    }]),
                    false => Transaction::default(),
                };
                self.proto_valve(resp_id, v, &resp, "Change trigger", icon::INTERACTION, tx);
                let words = match sideways(&self.session.doc, shown) {
                    true => ["Right", "Left", "Either"],
                    false => ["Down", "Up", "Either"],
                };
                let goings = [Going::Forward, Going::Back, Going::Either];
                let cur = goings.iter().position(|g| *g == going).unwrap_or(0);
                let pick = ui::segmented_tipped(
                    ui,
                    w,
                    ui::SEGMENT_CELL_H,
                    3,
                    cur,
                    |_| true,
                    |i| match i {
                        0 => "When it scrolls into the content past the point",
                        1 => "When it scrolls back past the point",
                        _ => "Each time it crosses the point",
                    },
                    |p, i, r, on| ui::segment_label(p, r, words[i], on),
                );
                if let Some(i) = pick.filter(|i| *i != cur) {
                    let going = goings[i];
                    out.write = Some((
                        with(&|x: &mut Reaction| {
                            x.trigger = Trigger::ScrollPast { frame, at, going }
                        }),
                        "Change trigger",
                        icon::INTERACTION,
                    ));
                }
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
            Trigger::TableChanges { table } => {
                let w = ui.available_width();
                let doc = &self.session.doc;
                if let Some(t) = row_table_combo(ui, doc, ("when-table", r.id), *table, w) {
                    out.write = Some((
                        with(&|x: &mut Reaction| x.trigger = Trigger::TableChanges { table: t }),
                        "Change trigger",
                        icon::TABLE,
                    ));
                }
            }
            _ => {}
        }
        // --- the actions, one step in under their trigger ---------------------
        let mut list_out = ListOut::default();
        self.action_rows(ui, id, r, &r.actions, NEST, &mut list_out);
        if let Some(place) = list_out.add {
            add_place = Some(place);
        }
        if let Some((place, kind)) = add_place {
            // A document with no script yet names one still to be picked from
            // the library in the popover that opens.
            let made = new_action(&self.session.doc, id, kind).or_else(|| {
                (kind == ActionKind::RunScript).then(|| Action::RunScript {
                    script: self.session.ids.mint_script(),
                })
            });
            match made {
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
        let label = trigger_label(doc, id, &r.trigger);
        let a_screen = screen_of(doc, id) == Some(id);
        let first_var = doc.variables().iter().next().map(|v| v.id);
        let first_table = doc.data_sets().first().map(|d| d.id);
        let first_scroller = scrolling_frames(doc, id).first().copied();
        let mut pick = None;
        let current = r.trigger.clone();
        let face = ui::glyph_and_text(icon::INTERACTION, &label);
        // A row of the list, dimmed with its reason where it cannot apply.
        let row = |ui: &mut egui::Ui, label: &str, on: bool, why: Option<&str>| {
            let r = ui::menu_row(
                ui,
                ui::MenuRow::new("", label)
                    .selected(on)
                    .enabled(why.is_none()),
                ui::MENU_ROW_H,
            );
            match why {
                Some(w) => r.on_disabled_hover_text(w),
                None => r,
            }
        };
        ui::fitted_combo(ui, ("trigger", r.id), w, face, &label, |ui| {
            ui::menu_rows(ui);
            ui::menu_eyebrow(ui, "Pointer");
            for t in POINTER_TRIGGERS {
                let on = same_trigger_kind(&t, &current);
                // The menu names the gesture; its side is picked under the head.
                let label = match t {
                    Trigger::Swipe { .. } => "On swipe".into(),
                    _ => trigger_label(doc, id, &t),
                };
                if row(ui, &label, on, None).clicked() && !on {
                    pick = Some(TriggerPick::Set(t));
                }
            }
            let follows = doc.get(id).is_some_and(|n| n.play().follow.is_some());
            let on = matches!(current, Trigger::Drop { .. });
            let why = (!follows).then_some("Only a layer that follows the drag drops");
            if row(ui, "On drop", on, why).clicked() && !on {
                pick = Some(TriggerPick::Set(Trigger::Drop {
                    at: DropAt::Anywhere,
                }));
            }
            ui::menu_sep(ui);
            ui::menu_eyebrow(ui, "Other");
            let on = matches!(current, Trigger::Key { .. });
            if row(ui, "Key…", on, None).clicked() {
                pick = Some(TriggerPick::Key);
            }
            let on = matches!(current, Trigger::After { .. });
            if row(ui, "After delay", on, None).clicked() && !on {
                pick = Some(TriggerPick::Set(Trigger::After { ms: 1000 }));
            }
            let on = matches!(current, Trigger::VariableChanges { .. });
            let why = first_var
                .is_none()
                .then_some("This document has no variables");
            if row(ui, "When a variable changes", on, why).clicked()
                && !on
                && let Some(var) = first_var
            {
                pick = Some(TriggerPick::Set(Trigger::VariableChanges { var }));
            }
            let on = matches!(current, Trigger::TableChanges { .. });
            let why = first_table.is_none().then_some(NO_TABLES);
            if row(ui, "When a table changes", on, why).clicked()
                && !on
                && let Some(table) = first_table
            {
                pick = Some(TriggerPick::Set(Trigger::TableChanges { table }));
            }
            let on = matches!(current, Trigger::ScrollPast { .. });
            let why = first_scroller.is_none().then_some("No frame here scrolls");
            if row(ui, "When scrolled past", on, why).clicked()
                && !on
                && let Some(frame) = first_scroller
            {
                pick = Some(TriggerPick::Set(Trigger::ScrollPast {
                    frame,
                    at: NEW_SCROLL_POINT,
                    going: Going::Forward,
                }));
            }
            let on = matches!(current, Trigger::FrameOpens);
            let why = (!a_screen).then_some("Only a frame opens");
            if row(ui, "When the frame opens", on, why).clicked() && !on {
                pick = Some(TriggerPick::Set(Trigger::FrameOpens));
            }
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
                // With no script of its own, the document can still copy one in
                // from the library in the action's popover.
                let library = doc.scripts().is_empty() && !self.library_script_names().is_empty();
                for (i, (group, kinds)) in ActionKind::GROUPS.iter().enumerate() {
                    if i > 0 {
                        ui::menu_sep(ui);
                    }
                    ui::menu_eyebrow(ui, group);
                    for kind in kinds.iter() {
                        let why = match action_refusal(doc, id, trigger, *kind) {
                            Some(_)
                                if library
                                    && *kind == ActionKind::RunScript
                                    && trigger.is_a_persons_act() =>
                            {
                                None
                            }
                            why => why,
                        };
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
    /// A row dragged by its grip is carried under the pointer with its branches
    /// while the others make room where it would land, as an Effects row is; the
    /// release moves it, and a right-click or `Escape` (`cancel_gesture`) puts
    /// it back.
    fn action_rows(
        &mut self,
        ui: &mut egui::Ui,
        id: NodeId,
        r: &Keyed<Reaction>,
        list: &[Keyed<Action>],
        indent: f32,
        out: &mut ListOut,
    ) {
        let at = |action: ItemId| ActionAt {
            node: id,
            reaction: r.id,
            action,
        };
        let held = |d: &ActionDrag| {
            d.at.node == id
                && d.at.reaction == r.id
                && list.get(d.from).is_some_and(|k| k.id == d.at.action)
        };
        let n = list.len();
        // Advanced against last frame's slots, before the rows are laid out.
        let y = ui.ctx().input(|i| i.pointer.interact_pos()).map(|p| p.y);
        let moved = self
            .in_flight
            .action_drag
            .as_mut()
            .filter(|d| held(d))
            .map(|d| {
                if let Some(y) = y
                    && !d.slots.is_empty()
                {
                    d.to = paint::forward_landing(n, d.to.min(n - 1), d.slots.gap(y));
                }
                (d.from, d.to)
            });
        let carrying = moved.map(|(from, _)| from);
        let mut slots = paint::Slots::default();
        for index in paint::forward_order(n, moved) {
            let k = &list[index];
            let open = self.proto.open == Some(at(k.id));
            let carried = carrying == Some(index);
            let (_, rect) = carried_unit(ui, carried, ("action", k.id), |ui| {
                let (row, resp) = self.action_row(ui, id, k, indent, open, n > 1);
                if row.grabbed {
                    self.in_flight.action_drag = Some(ActionDrag {
                        at: at(k.id),
                        from: index,
                        to: index,
                        slots: paint::Slots::default(),
                    });
                    // Its popover's `Area` would not follow the row.
                    self.proto.open = None;
                }
                // *Back* has nothing to set, so its row opens no popover.
                if row.open && !matches!(k.value, Action::Back) {
                    self.proto.open = (!open).then(|| at(k.id));
                }
                if row.remove {
                    out.remove = Some(k.id);
                }
                if open && !carried {
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
            });
            slots.push(rect.top(), rect.bottom());
        }
        // The carried row's list keeps its slots for next frame, and its release
        // is read from the pointer: a carried row's responses are empty.
        let Some(d) = self.in_flight.action_drag.as_mut().filter(|d| held(d)) else {
            return;
        };
        d.slots = slots;
        if ui.ctx().input(|i| i.pointer.primary_down()) {
            ui.ctx().set_cursor_icon(egui::CursorIcon::Grabbing);
            ui.ctx().request_repaint();
            return;
        }
        if let Some(d) = self.in_flight.action_drag.take()
            && d.to != d.from
        {
            out.reorder = Some((d.at.action, d.to));
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
        // A head is a caption over the rows it heads, not a row of its own: one
        // line tall, the rows' gap tucked in on both sides, its `+` reaching
        // past the line to the close button's box.
        let h = BRANCH_HEAD_H;
        let reach = ((ui::CLOSE_BUTTON_W - h) / 2.0) as i8;
        ui.add_space(-BRANCH_TUCK);
        ui.horizontal(|ui| {
            ui.spacing_mut().item_spacing.x = COL;
            ui.add_space(indent);
            let w = ui.available_width() - h - COL;
            ui.allocate_ui_with_layout(
                egui::vec2(w, h),
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
                    ui::icon_button_padded(
                        ui,
                        icon::PLUS,
                        egui::Vec2::splat(h),
                        ui::CLOSE_GLYPH_PT,
                        false,
                        true,
                        egui::Margin::same(reach),
                    )
                })
                .inner
                .on_hover_text("Add an action to this branch");
            picked = self.add_action_menu(&plus, id, trigger);
        });
        ui.add_space(-BRANCH_TUCK);
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
            Action::SetVariable { .. } | Action::SetMode { .. } => "Set",
            Action::Conditional { .. } => "If",
            _ => kind.label(),
        };
        // A variable set wears its name as a capsule, as a bound field does,
        // and nothing after it: a value rarely fits, and the popover shows it.
        let (capsule, target) = match &k.value {
            Action::SetVariable { var, .. } if missing.is_none() => {
                match doc.variables().get(*var) {
                    Some(v) => (Some(v.name.clone()), String::new()),
                    None => (None, target),
                }
            }
            _ => (None, target),
        };
        let mut out = RowOut::default();
        let resp = ui
            .horizontal(|ui| {
                ui.spacing_mut().item_spacing.x = COL;
                ui.add_space(indent);
                let size = egui::vec2(ui.available_width(), ui::CONTROL_H);
                // The grip and × sit over the row's press and keep their own.
                // Laid out as an Effects row is: its gap, glyph and label sizes.
                let (_, resp) = ui::field_row_pressable(ui, size, open, true, |ui, _| {
                    ui.spacing_mut().item_spacing.x = ui::LIST_ROW_GAP;
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
                    ui.label(theme::icon_text(glyph, ui::CONTROL_GLYPH_PT, ink));
                    let verb =
                        egui::RichText::new(verb)
                            .size(ui::CONTROL_PT)
                            .color(match missing {
                                Some(_) => color::error(),
                                None => theme::text::strong(),
                            });
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
                        // On the target's baseline, as the verb is.
                        if let Some(chip) = chip {
                            let chip = egui::RichText::new(chip)
                                .size(ui::CAPTION_PT)
                                .color(theme::text::faint());
                            ui::baseline_label(ui, chip, ui::SMALL_PT, false);
                        }
                        let ink = match missing {
                            Some(_) => color::error(),
                            None => color::accent_200(),
                        };
                        // The verb and the target share what the timing and ×
                        // leave, the verb first; each is cut rather than run
                        // under the timing.
                        let room = ui.available_width();
                        let layout = egui::Layout::left_to_right(egui::Align::Center);
                        ui.with_layout(layout, |ui| {
                            let verb_w = egui::WidgetText::from(verb.clone())
                                .into_galley(
                                    ui,
                                    Some(egui::TextWrapMode::Extend),
                                    f32::INFINITY,
                                    egui::TextStyle::Body,
                                )
                                .size()
                                .x;
                            let h = ui.available_height();
                            ui.allocate_ui_with_layout(
                                egui::vec2(verb_w.min(room), h),
                                layout,
                                |ui| ui::baseline_label(ui, verb, ui::SMALL_PT, true),
                            );
                            if let Some(name) = &capsule {
                                let max = (ui.available_width() * 0.6).max(24.0);
                                let w = super::binding::capsule_width(ui, name, max);
                                let (r, _) = ui.allocate_exact_size(
                                    egui::vec2(w, ui.available_height()),
                                    egui::Sense::hover(),
                                );
                                super::binding::capsule(ui, r.left_center(), name, max);
                            }
                            // A target with no room for a letter before its `…`
                            // is left out: a lone `…` overflows onto the timing.
                            let text = egui::RichText::new(&target).size(ui::SMALL_PT).color(ink);
                            let width = |ui: &egui::Ui, s: &str| {
                                egui::WidgetText::from(egui::RichText::new(s).size(ui::SMALL_PT))
                                    .into_galley(
                                        ui,
                                        Some(egui::TextWrapMode::Extend),
                                        f32::INFINITY,
                                        egui::TextStyle::Body,
                                    )
                                    .size()
                                    .x
                            };
                            let least = width(ui, "W…").min(width(ui, &target));
                            if !target.is_empty() && ui.available_width() >= least {
                                ui.add(egui::Label::new(text).truncate());
                            }
                        });
                    });
                });
                let tip = match missing {
                    Some(gone) => format!(
                        "{gone}: skipped while playing. Pick another target, or undo the delete"
                    ),
                    None if kind == ActionKind::Back => BACK_NOTE.to_string(),
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
        let kind = ActionKind::of(&k.value);
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
                    if ui::popover_head_glyph(ui, Some(kind.glyph()), kind.label()) {
                        close = true;
                    }
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
        let opening_click = std::mem::take(&mut self.proto.opened_on_canvas);
        if close
            || (!opening_click
                && super::dismissed_by_click(super::ClickAway {
                    clicked: ctx.input(|i| i.pointer.button_clicked(egui::PointerButton::Primary)),
                    over_overlay,
                    on_menu: menu.contains_pointer(),
                    on_head: head.contains_pointer(),
                    // The overlay's ghost on the canvas is this popover's to move.
                    on_picker: ctx.pointer_latest_pos().is_some_and(|p| self.ghost_at(p)),
                    in_gesture: ctx.dragged_id().is_some() || self.gesture_cancelled,
                    press_away,
                }))
        {
            self.proto.open = None;
        }
        self.popover_heard = true;
        if ctx.input(|i| i.key_pressed(egui::Key::Escape)) {
            self.proto.open = None;
        }
        let mut v = r.value.clone();
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
            Action::Navigate {
                to,
                transition,
                keep_scroll,
            } => {
                if let Some(t) = frame_combo(ui, doc, ("to", k.id), *to, id, w) {
                    out = Some((
                        Action::Navigate {
                            to: t,
                            transition: *transition,
                            keep_scroll: *keep_scroll,
                        },
                        "Change action",
                        icon::ARROW_RIGHT,
                    ));
                }
                if ui::switch_row(ui, "Keep scroll position", *keep_scroll, h)
                    .on_hover_text("Open the frame scrolled where it was last left, not at its top")
                    .clicked()
                {
                    out = Some((
                        Action::Navigate {
                            to: *to,
                            transition: *transition,
                            keep_scroll: !*keep_scroll,
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
                if let Some(s) = self.overlay_fields(ui, id, k.id, *settings, w) {
                    out = Some((make(*overlay, s), "Overlay", glyph));
                }
            }
            Action::ScrollTo { layer, transition } => {
                let options: Vec<NodeId> = screen_of(doc, id)
                    .map(|s| pickable_under(doc, s))
                    .unwrap_or_default();
                if let Some(t) = layer_combo(ui, doc, ("scroll", k.id), None, *layer, &options, w) {
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
                if let Some(t) = layer_combo(ui, doc, ("state", k.id), None, *to, &options, w) {
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
            Action::SetMode { mode, scope } => {
                let (mode, scope) = (*mode, *scope);
                let cols = switchable_collections(doc);
                let col = cols.iter().find(|c| c.modes.iter().any(|m| m.0 == mode));
                let mut pick = None;
                ui::labelled(ui, "Collection", |ui| {
                    if let Some(c) = collection_combo(ui, doc, ("mode-col", k.id), w, &cols, col) {
                        // A new collection starts at the mode it does not read
                        // now, as a new action does.
                        let now = screen_of(doc, id).and_then(|s| doc.mode_of_at(s, c.owner));
                        pick = c.modes.iter().find(|m| Some(m.0) != now).map(|m| m.0);
                    }
                });
                let mode_name = col
                    .and_then(|c| c.modes.iter().find(|m| m.0 == mode))
                    .map_or("Missing mode".to_string(), |m| m.1.to_owned());
                ui::labelled(ui, "Mode", |ui| {
                    ui::fitted_combo(ui, ("mode-mode", k.id), w, &mode_name, &mode_name, |ui| {
                        ui::menu_rows(ui);
                        for (m, n) in col.map_or(&[][..], |c| &c.modes[..]) {
                            let on = *m == mode;
                            if ui.selectable_label(on, *n).clicked() && !on {
                                pick = Some(*m);
                            }
                        }
                    });
                });
                let mut scope_new = scope;
                ui::labelled(ui, "Where", |ui| {
                    let cur = usize::from(scope == ModeScope::Screen);
                    let got = ui::segmented_tipped(
                        ui,
                        w,
                        ui::SEGMENT_CELL_H,
                        2,
                        cur,
                        |_| true,
                        |i| match i {
                            0 => "On every page; a frame that sets its own mode keeps it",
                            _ => "On the screen shown, over the page's",
                        },
                        |p, i, r, on| ui::segment_label(p, r, ["Everywhere", "This screen"][i], on),
                    );
                    if let Some(i) = got.filter(|i| *i != cur) {
                        scope_new = [ModeScope::Everywhere, ModeScope::Screen][i];
                    }
                });
                if let Some(c) = col {
                    let by_width = match c.owner {
                        ModeOwner::Collection(id) => doc
                            .variables()
                            .collection(id)
                            .is_some_and(|x| x.breakpoints.is_some()),
                        ModeOwner::Kit(..) => false,
                    };
                    let note = match (by_width, scope_new) {
                        (true, _) => format!(
                            "{} picks its mode by width; setting it holds {mode_name} at any width",
                            c.name
                        ),
                        (false, ModeScope::Everywhere) => {
                            format!("Frames that set their own {} mode keep it", c.name)
                        }
                        (false, ModeScope::Screen) => String::new(),
                    };
                    if !note.is_empty() {
                        ui::selection_note(ui, &note);
                    }
                }
                if pick.is_some() || scope_new != scope {
                    out = Some((
                        Action::SetMode {
                            mode: pick.unwrap_or(mode),
                            scope: scope_new,
                        },
                        "Change action",
                        icon::MODE,
                    ));
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
                        ) => VarChange::Set { to: keeps(v) },
                        // A value written for another kind would not read.
                        (Some(_), VarChange::Set { .. }) if new_kind != kind => {
                            VarChange::Set { to: keeps(v) }
                        }
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
                        _ => VarChange::Set { to: keeps(*var) },
                    };
                    out = Some((
                        Action::SetVariable { var: *var, change },
                        "Change action",
                        icon::BRACKETS_CURLY,
                    ));
                }
                match change {
                    VarChange::Set { to } => {
                        if let Some(e) = self.expr_field(
                            ui,
                            DraftKey::Expr(k.id, 0),
                            to,
                            w,
                            ExprWant::Kind(kind),
                            Reads::Tables(None),
                        ) {
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
                                w - ui::CONTROL_H - COL
                            } else {
                                w
                            };
                            if let Some(e) = self.expr_field(
                                ui,
                                DraftKey::Expr(k.id, i),
                                &b.when,
                                fw,
                                ExprWant::Kind(Some(VarKind::Boolean)),
                                Reads::Tables(None),
                            ) {
                                next_branches[i].when = e;
                                changed = true;
                            }
                            if branches.len() > 1
                                && ui::field_button(ui, icon::X, ui::CONTROL_H, FieldButton::Off)
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
                if crate::settings::text_button(ui, "Add else if", FieldButton::Off)
                    .on_hover_text("Add a branch tried when the ones above are not")
                    .clicked()
                {
                    next_branches.push(Branch {
                        when: first_condition(&self.session.doc),
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
                    "Numbers take + − × ÷; text joins with +; and, or, not and comparisons test; type @ for a variable",
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
                let mut copy: Option<String> = None;
                let face = ui::glyph_and_text(icon::FILE_CODE, &current);
                let fly_at = ui.id().with(("script-library", k.id));
                let shown = ui::fitted_combo(ui, ("script", k.id), w, face, &current, |ui| {
                    ui::menu_rows(ui);
                    let mut elsewhere = false;
                    for (s, n) in &scripts {
                        let row = ui.selectable_label(s == script, n);
                        elsewhere |= row.hovered();
                        if row.clicked() && s != script {
                            pick = Some(*s);
                        }
                    }
                    // Then a library script, copied in first so the export
                    // carries it.
                    if !scripts.is_empty() {
                        ui::menu_sep(ui);
                    }
                    let names = self.library_script_names();
                    let mut open = ui.ctx().data(|d| d.get_temp::<egui::Rect>(fly_at));
                    let row = ui::submenu_row(
                        ui,
                        ui::MenuRow::new(icon::FOLDER_SIMPLE, "Copy from library")
                            .enabled(!names.is_empty()),
                        open.is_some(),
                    );
                    match names.is_empty() {
                        true => {
                            row.on_disabled_hover_text("The script library is empty");
                            open = None;
                        }
                        false if row.hovered() || row.clicked() => open = Some(row.rect),
                        false => {}
                    }
                    if elsewhere {
                        open = None;
                    }
                    if let Some(at) = open {
                        let ctx = ui.ctx().clone();
                        // As wide as its longest name, not the menu it hangs from.
                        let inner = ui::menu_width(&ctx, names.iter().map(String::as_str), false);
                        let fw = inner + 2.0 * (ui::FLYOUT_PAD + ui::MENU_BORDER);
                        ui::flyout(fly_at.with("area"), at, fw).show(
                            &ctx,
                            (names.len(), 0),
                            |ui| {
                                ui::menu_rows(ui);
                                for n in &names {
                                    let row = ui::MenuRow::new("", n);
                                    if ui::menu_row(ui, row, ui::MENU_ROW_H).clicked() {
                                        copy = Some(n.clone());
                                    }
                                }
                            },
                        );
                        if copy.is_some() {
                            open = None;
                            ui.close();
                        }
                    }
                    ui.ctx().data_mut(|d| match open {
                        Some(at) => {
                            d.insert_temp(fly_at, at);
                        }
                        None => d.remove::<egui::Rect>(fly_at),
                    });
                });
                if shown.inner.is_none() {
                    ui.ctx().data_mut(|d| d.remove::<egui::Rect>(fly_at));
                }
                if let Some(name) = copy
                    && let Some((s, add)) = self.copy_library_script(&name)
                    && s != *script
                {
                    self.proto.script_in = add;
                    pick = Some(s);
                }
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
                ui::selection_note(ui, BACK_NOTE);
            }
            Action::CloseOverlay { .. } => {
                ui::selection_note(ui, "Closes the overlay on top");
            }
            Action::AddRow { .. } | Action::EditRow { .. } | Action::DeleteRow { .. } => {
                let glyph = ActionKind::of(&k.value).glyph();
                out = self
                    .row_action_fields(ui, id, k, w)
                    .map(|a| (a, "Change action", glyph));
            }
        }
        out
    }

    /// A row action's fields: its table; *Add row*'s place and one field per
    /// column, an empty one writing an empty cell; *Edit row*'s and *Delete
    /// row*'s row, and *Edit row*'s cells, only the columns it writes. Answers
    /// the action rewritten.
    fn row_action_fields(
        &mut self,
        ui: &mut egui::Ui,
        id: NodeId,
        k: &Keyed<Action>,
        w: f32,
    ) -> Option<Action> {
        let doc = &self.session.doc;
        let (table, at, row, cells) = match &k.value {
            Action::AddRow { table, at, cells } => (*table, Some(*at), None, Some(cells.clone())),
            Action::EditRow { table, row, cells } => {
                (*table, None, Some(row.clone()), Some(cells.clone()))
            }
            Action::DeleteRow { table, row } => (*table, None, Some(row.clone()), None),
            _ => return None,
        };
        let make = |table: DataSetId, row: Option<WhichRow>, cells: Vec<CellWrite>| match (at, row)
        {
            (Some(at), _) => Action::AddRow { table, at, cells },
            (None, Some(row)) if matches!(k.value, Action::EditRow { .. }) => {
                Action::EditRow { table, row, cells }
            }
            (None, row) => Action::DeleteRow {
                table,
                row: row.unwrap_or(WhichRow::Opened),
            },
        };
        let columns: Vec<(mastik_core::chart::ColumnId, String)> = doc
            .data_set(table)
            .map(|d| {
                d.table
                    .columns
                    .iter()
                    .map(|c| (c.id, c.name.clone()))
                    .collect()
            })
            .unwrap_or_default();
        let table_name = doc
            .data_set(table)
            .map_or_else(|| "Missing table".to_string(), |d| d.name.clone());
        let in_list = in_list_over(doc, id, table);
        let mut next = None;
        ui::labelled(ui, "Table", |ui| {
            if let Some(t) = row_table_combo(ui, doc, ("row-table", k.id), table, w) {
                // Another table's columns are not these, and its list may not
                // hold this layer.
                let row = row.clone().map(|r| match r {
                    WhichRow::This if !in_list_over(doc, id, t) => WhichRow::Opened,
                    WhichRow::FirstWhere { .. } => WhichRow::Opened,
                    r => r,
                });
                next = Some(make(t, row, Vec::new()));
            }
        });
        if let Some(at) = at {
            ui::labelled(ui, "At", |ui| {
                let cur = usize::from(at == RowEnd::Start);
                let got = ui::segmented_tipped(
                    ui,
                    w,
                    ui::SEGMENT_CELL_H,
                    2,
                    cur,
                    |_| true,
                    |i| match i {
                        0 => "After the last row",
                        _ => "Before the first row",
                    },
                    |p, i, r, on| ui::segment_label(p, r, ["End", "Start"][i], on),
                );
                if let Some(i) = got.filter(|i| *i != cur) {
                    next = Some(Action::AddRow {
                        table,
                        at: [RowEnd::End, RowEnd::Start][i],
                        cells: cells.clone().unwrap_or_default(),
                    });
                }
            });
        }
        if let Some(row) = &row {
            let this_why =
                format!("This row: the interaction is not inside a list over {table_name}");
            ui::labelled(ui, "Row", |ui| {
                let face = row_short(row);
                ui::fitted_combo(ui, ("row-which", k.id), w, face, face, |ui| {
                    ui::menu_rows(ui);
                    let options = [
                        ("This row", "the list row tapped", WhichRow::This),
                        ("Opened row", "", WhichRow::Opened),
                        (
                            "First row where…",
                            "",
                            WhichRow::FirstWhere {
                                when: first_row_test(&columns),
                            },
                        ),
                    ];
                    for (label, note, pick) in options {
                        let on = std::mem::discriminant(row) == std::mem::discriminant(&pick);
                        let ok = pick != WhichRow::This || in_list;
                        let mut r = ui::MenuRow::new("", label).selected(on).enabled(ok);
                        if !note.is_empty() {
                            r = r.note(note);
                        }
                        let resp = ui::menu_row(ui, r, ui::MENU_ROW_H);
                        if !ok {
                            resp.on_disabled_hover_text(&this_why);
                        } else if resp.clicked() && !on {
                            next = Some(make(table, Some(pick), cells.clone().unwrap_or_default()));
                        }
                    }
                });
            });
            if *row == WhichRow::This && !in_list {
                ui::selection_note(ui, &this_why);
            }
            if let WhichRow::FirstWhere { when } = row {
                ui::labelled(ui, "Where", |ui| {
                    if let Some(e) = self.expr_field(
                        ui,
                        DraftKey::Expr(k.id, usize::MAX),
                        when,
                        w,
                        ExprWant::Kind(Some(VarKind::Boolean)),
                        Reads::Tables(Some(table)),
                    ) {
                        let row = WhichRow::FirstWhere { when: e };
                        next = Some(make(table, Some(row), cells.clone().unwrap_or_default()));
                    }
                });
                ui::selection_note(ui, "No row matches: the action is skipped");
            }
        }
        let Some(cells) = cells else {
            return next;
        };
        ui::card_rule(ui);
        let order = |c: &mut Vec<CellWrite>| {
            c.sort_by_key(|x| columns.iter().position(|(id, _)| *id == x.column));
        };
        if at.is_some() {
            // *Add row*: every column, an empty field an empty cell.
            for (i, (col, name)) in columns.iter().enumerate() {
                let now = cells
                    .iter()
                    .find(|c| c.column == *col)
                    .map_or_else(Expr::default, |c| c.to.clone());
                ui::labelled(ui, name, |ui| {
                    let key = DraftKey::Expr(k.id, i);
                    let reads = Reads::Tables(None);
                    if let Some(e) = self.expr_field(ui, key, &now, w, ExprWant::OrEmpty, reads) {
                        let mut c: Vec<CellWrite> =
                            cells.iter().filter(|c| c.column != *col).cloned().collect();
                        if !e.0.trim().is_empty() {
                            c.push(CellWrite {
                                column: *col,
                                to: e,
                            });
                        }
                        order(&mut c);
                        next = Some(make(table, None, c));
                    }
                });
            }
            if columns.is_empty() {
                ui::selection_note(ui, "The table has no columns");
            }
            return next;
        }
        // *Edit row*: the columns it writes, each removable, then *Add column*.
        let row = row.unwrap_or(WhichRow::Opened);
        let fw = w - ui::CONTROL_H - COL;
        for (i, c) in cells.iter().enumerate() {
            let name = columns
                .iter()
                .find(|(id, _)| *id == c.column)
                .map_or("Missing column", |(_, n)| n.as_str());
            ui::labelled(ui, name, |ui| {
                ui.horizontal(|ui| {
                    ui.spacing_mut().item_spacing.x = COL;
                    let key = DraftKey::Expr(k.id, i);
                    let want = ExprWant::Kind(None);
                    let reads = Reads::Tables(Some(table));
                    if let Some(e) = self.expr_field(ui, key, &c.to, fw, want, reads) {
                        let mut all = cells.clone();
                        all[i].to = e;
                        next = Some(make(table, Some(row.clone()), all));
                    }
                    if ui::field_button(ui, icon::X, ui::CONTROL_H, FieldButton::Off)
                        .on_hover_text("Leave this column as it is")
                        .clicked()
                    {
                        let mut all = cells.clone();
                        all.remove(i);
                        next = Some(make(table, Some(row.clone()), all));
                    }
                });
            });
        }
        let free = columns
            .iter()
            .find(|(id, _)| !cells.iter().any(|c| c.column == *id));
        let size = egui::vec2(w, ui::CONTROL_H);
        let resp = ui::disable_unless(ui, free.is_some(), |ui| {
            ui::action_button(ui, icon::PLUS, "Add column", FieldButton::Off, size)
        })
        .inner;
        let resp = match free {
            Some(_) => resp.on_hover_text("Write another of the row's cells"),
            None => resp.on_disabled_hover_text("The edit writes every column"),
        };
        if resp.clicked()
            && let Some((col, _)) = free
        {
            // It reads its own cell, so it changes nothing until typed over.
            let mut all = cells.clone();
            all.push(CellWrite {
                column: *col,
                to: Expr(format!("{{{}}}", Ref::Row(*col).wire())),
            });
            order(&mut all);
            next = Some(make(table, Some(row.clone()), all));
        }
        if cells.len() > 1 {
            ui::selection_note(
                ui,
                "Every cell is worked out from the row as it was before the edit",
            );
        }
        next
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
        let mut out = None;
        let mut kind_pick = None;
        let kind = transition_label(t.kind);
        ui::fitted_combo(ui, ("kind", salt), w, ui::combo_text(kind), kind, |ui| {
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
        if let Some(timing) = self.timing_fields(ui, salt, t.timing, w) {
            out = Some(Transition { timing, ..t });
        }
        out
    }

    /// A timing's rows: *Curve* or *Spring*, then a curve over a duration or a
    /// spring's three numbers, then the motion preview. Answers as
    /// [`Self::transition_fields`] does.
    pub(crate) fn timing_fields(
        &mut self,
        ui: &mut egui::Ui,
        salt: impl std::hash::Hash + std::fmt::Debug + Copy,
        t: Timing,
        w: f32,
    ) -> Option<Timing> {
        let h = ui::CONTROL_H;
        let mut out = None;
        let spring = matches!(t, Timing::Spring { .. });
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
            Some(1) if !spring => out = Some(SPRING_PRESETS[DEFAULT_SPRING].1),
            Some(0) if spring => {
                out = Some(Timing::Ease {
                    curve: Easing::EaseOut,
                    ms: 300,
                });
            }
            _ => {}
        }
        // A number's write: the timing with that number in it.
        let mut numbers: Vec<(egui::Response, egui::Id, f64, Timing)> = Vec::new();
        match t {
            Timing::Ease { curve, ms } => {
                let mut curve_pick = None;
                ui.horizontal(|ui| {
                    ui.spacing_mut().item_spacing.x = COL;
                    let half = (w - COL) / 2.0;
                    let shown = easing_label(curve);
                    let face = ui::combo_text(shown);
                    ui::fitted_combo(ui, ("curve", salt), half, face, shown, |ui| {
                        ui::menu_rows(ui);
                        for (e, l) in EASINGS {
                            let on = shown == l;
                            if ui.selectable_label(on, l).clicked() && !on {
                                curve_pick = Some(e);
                            }
                        }
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
                    numbers.push((resp, rid, v, Timing::Ease { curve, ms: ms2 }));
                });
                if let Some(e) = curve_pick {
                    out = Some(Timing::Ease { curve: e, ms });
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
                                Timing::Ease {
                                    curve: Easing::Cubic(c2),
                                    ms,
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
                // A named feel first; the numbers under it make a custom one.
                let shown = SPRING_PRESETS
                    .iter()
                    .find(|(_, p)| *p == t)
                    .map_or("Custom", |(l, _)| *l);
                let mut preset = None;
                ui::fitted_combo(
                    ui,
                    ("spring-feel", salt),
                    w,
                    ui::combo_text(shown),
                    shown,
                    |ui| {
                        ui::menu_rows(ui);
                        for (l, p) in SPRING_PRESETS {
                            let on = shown == l;
                            if ui.selectable_label(on, l).clicked() && !on {
                                preset = Some(p);
                            }
                        }
                    },
                )
                .response
                .on_hover_text("How the spring feels");
                if let Some(p) = preset {
                    out = Some(p);
                }
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
                            Timing::Spring {
                                mass: s[0],
                                stiffness: s[1],
                                damping: s[2],
                            },
                        ));
                    }
                });
            }
        }
        // The timing a field holds while it is scrubbed or typed, so the preview
        // follows it before it commits; on the frame it commits, the one written,
        // or the preview shows the saved timing for a frame.
        let held = numbers
            .iter()
            .find(|(r, ..)| r.dragged() || r.has_focus())
            .map(|(.., next)| *next);
        for (resp, rid, v, next) in numbers {
            self.proto_hold(rid, v, &resp);
            let engaged = resp.dragged() || resp.has_focus();
            if next != t && !engaged && interacting(&resp) && !self.gesture_cancelled {
                out = Some(next);
            }
        }
        let shown = held.or(out).unwrap_or(t);
        motion_preview(ui, salt, shown, w);
        out
    }

    /// An overlay action's rows: where it shows, *Close on a click outside*,
    /// *Darken behind it* and, while darkened, the shade's colour and strength.
    /// *Manual* adds X and Y, measured from the layer that opens it; the frame is
    /// placed by dragging its ghost on the canvas, the fields being for the last
    /// point. Answers as [`Self::transition_fields`] does, but for the shade,
    /// which `node`'s colour slot commits itself.
    fn overlay_fields(
        &mut self,
        ui: &mut egui::Ui,
        node: NodeId,
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
            let face = ui::combo_text(shown);
            ui::fitted_combo(ui, ("overlay-at", action), w, face, shown, |ui| {
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
            if let Some(p) = pick {
                // *Manual* starts just under the layer that opens it, not over it.
                let p = match p {
                    OverlayPosition::Manual { x, .. } => OverlayPosition::Manual {
                        x,
                        y: self
                            .world_bounds(node)
                            .map_or(0.0, |b| (b.height() + MANUAL_OVERLAY_GAP).round()),
                    },
                    other => other,
                };
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
            // The shade as a shadow's colour is: chip (the picker), hex, and its
            // strength as the opacity; it commits through its own slot.
            let slot = PaintSlot::Backdrop(action);
            let shown = match self.slot_brush(node, slot) {
                Some(mastik_core::Brush::Solid(live)) => live,
                _ => c,
            };
            self.slot_color_row(
                ui,
                node,
                slot,
                egui::vec2(w, ui::CONTROL_H),
                shown,
                true,
                "Edit the shade behind the overlay",
            );
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
        let face = ui::glyph_and_text(icon::BRACKETS_CURLY, &shown);
        ui::fitted_combo(ui, salt, w, face, &shown, |ui| {
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
        pick
    }

    /// An expression's field: the source with variables by name, refused (kept,
    /// ringed, the reason under it) when it does not read. Answers what to store
    /// once it commits.
    pub(super) fn expr_field(
        &mut self,
        ui: &mut egui::Ui,
        key: DraftKey,
        e: &Expr,
        w: f32,
        want: ExprWant,
        reads: Reads,
    ) -> Option<Expr> {
        let empty_ok = want == ExprWant::OrEmpty;
        let (want, literal) = match want {
            ExprWant::Kind(k) => (k, false),
            ExprWant::LiteralOr => (None, true),
            ExprWant::OrEmpty => (None, false),
        };
        let shown = mastik_core::prototype::expr_shown(&self.session.doc, e, reads.row());
        let ctx = ui.ctx().clone();
        let live = ui::Draft::live(&mut self.proto.draft, &ctx, key);
        let refused = live.as_ref().and_then(|d| d.refused.clone());
        let mut text = live.map_or(shown.clone(), |d| d.text.clone());
        // `@` or an open `{` asks for a variable or a table: while the field
        // holds one being named, its keys move through the list under it rather
        // than commit.
        let at_id = ui.id().with(("expr-at", key));
        let (pick_key, pressed_in) = self.at_list_keys(ui, at_id, &text, reads);
        let hint = match (literal, reads) {
            (true, _) => "A value, or @ for a variable",
            (false, Reads::Vars) => "An expression, such as {count} + 1, or @ for a variable",
            (false, Reads::Tables(_)) => "An expression, such as {count} + 1, or @ for a name",
        };
        let resp = ui::refusable_field(
            ui,
            egui::vec2(w, ui::CONTROL_H),
            &mut text,
            hint,
            refused.as_deref(),
        );
        if refused.is_some()
            && let Some((k, at)) = &self.proto.expr_fault
            && *k == key
        {
            ui::field_underline(ui, &resp, ui::CONTROL_H, &text, at.clone());
        }
        // A press on the list keeps the field's focus and its draft.
        if pressed_in && resp.lost_focus() {
            resp.request_focus();
            return None;
        }
        if let Some(row) = self.at_list(ui, at_id, &resp, &text, pick_key, reads) {
            let next = picked(&text, &row);
            let mut state = egui::TextEdit::load_state(&ctx, resp.id).unwrap_or_default();
            state
                .cursor
                .set_char_range(Some(egui::text::CCursorRange::one(
                    egui::text::CCursor::new(next.chars().count()),
                )));
            state.store(&ctx, resp.id);
            resp.request_focus();
            self.proto.draft = Some(ui::Draft::new(&ctx, key, next));
            return None;
        }
        let typed = ui::Draft::follow(&mut self.proto.draft, &resp, key, &text)?;
        if typed == shown {
            return None;
        }
        if (literal && mastik_core::prototype::expr::var_tokens(&typed).is_empty())
            || (empty_ok && typed.trim().is_empty())
        {
            self.proto.expr_fault = None;
            return Some(Expr(typed));
        }
        match expr_stored(&self.session.doc, &typed, want, reads) {
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

    /// Before an expression field draws: while its `text` names a variable after
    /// `@` or `{` and the field held focus last pass, take the arrows, Enter and Tab
    /// for the list. Answers the variable Enter or Tab picked, and whether the
    /// pointer pressed on the list this pass.
    fn at_list_keys(
        &self,
        ui: &egui::Ui,
        at_id: egui::Id,
        text: &str,
        reads: Reads,
    ) -> (Option<AtRow>, bool) {
        let shown: Option<(Vec<AtRow>, egui::Rect, egui::Id)> = ui
            .ctx()
            .data(|d| d.get_temp::<(egui::Rect, egui::Id)>(at_id))
            .map(|(r, f)| (at_rows(&self.session.doc, text, reads), r, f));
        let Some((names, list, field)) = shown else {
            return (None, false);
        };
        if names.is_empty() || !ui.ctx().memory(|m| m.has_focus(field)) {
            return (None, false);
        }
        let pressed_in = ui.input(|i| {
            i.pointer.any_pressed() && i.pointer.interact_pos().is_some_and(|p| list.contains(p))
        });
        let hi_id = at_id.with("hi");
        let mut hi: usize = ui.ctx().data(|d| d.get_temp(hi_id)).unwrap_or(0);
        let (down, up, enter) = ui.input_mut(|i| {
            (
                i.consume_key(egui::Modifiers::NONE, egui::Key::ArrowDown),
                i.consume_key(egui::Modifiers::NONE, egui::Key::ArrowUp),
                i.consume_key(egui::Modifiers::NONE, egui::Key::Enter)
                    || i.consume_key(egui::Modifiers::NONE, egui::Key::Tab),
            )
        });
        if down {
            hi = (hi + 1) % names.len();
        }
        if up {
            hi = (hi + names.len() - 1) % names.len();
        }
        hi = hi.min(names.len() - 1);
        ui.ctx().data_mut(|d| d.insert_temp(hi_id, hi));
        (enter.then(|| names[hi].clone()), pressed_in)
    }

    /// The list `@` or `{` opens under the expression field `resp`, its highlighted
    /// row lit, each group under its eyebrow; answers the row clicked, or
    /// `picked` by Enter or Tab.
    fn at_list(
        &self,
        ui: &egui::Ui,
        at_id: egui::Id,
        resp: &egui::Response,
        text: &str,
        picked: Option<AtRow>,
        reads: Reads,
    ) -> Option<AtRow> {
        let names = at_rows(&self.session.doc, text, reads);
        if names.is_empty() || !resp.has_focus() {
            ui.ctx()
                .data_mut(|d| d.remove::<(egui::Rect, egui::Id)>(at_id));
            return picked;
        }
        let hi: usize = ui.ctx().data(|d| d.get_temp(at_id.with("hi"))).unwrap_or(0);
        let mut clicked = None;
        let shown = egui::Popup::new(at_id, ui.ctx().clone(), resp.rect, ui.layer_id())
            .kind(egui::PopupKind::Menu)
            .layout(egui::Layout::top_down_justified(egui::Align::Min))
            .style(egui::containers::menu::menu_style)
            .open(true)
            .align(egui::RectAlign::BOTTOM_START)
            .gap(ui::MENU_GAP)
            .show(|ui| {
                ui::popup_rows(ui);
                ui.set_width(resp.rect.width());
                let mut group = None;
                for (i, n) in names.iter().enumerate() {
                    if group != Some(n.group) {
                        ui::menu_eyebrow(ui, n.group);
                        group = Some(n.group);
                    }
                    let mut row = ui::MenuRow::new("", &n.label).highlight(i == hi);
                    if !n.note.is_empty() {
                        row = row.note(&n.note);
                    }
                    if ui::menu_row(ui, row, ui::MENU_ROW_H).clicked() {
                        clicked = Some(n.clone());
                    }
                }
            });
        if let Some(s) = shown {
            ui.ctx()
                .data_mut(|d| d.insert_temp(at_id, (s.response.rect, resp.id)));
        }
        picked.or(clicked)
    }

    /// *Open link*'s address, refused unless it is `http` or `https`.
    fn url_field(&mut self, ui: &mut egui::Ui, item: ItemId, url: &str, w: f32) -> Option<String> {
        let key = DraftKey::Url(item);
        let ctx = ui.ctx().clone();
        let live = ui::Draft::live(&mut self.proto.draft, &ctx, key);
        let refused = live.as_ref().and_then(|d| d.refused.clone());
        let mut text = live.map_or(url.to_string(), |d| d.text.clone());
        let resp = ui::refusable_field(
            ui,
            egui::vec2(w, ui::CONTROL_H),
            &mut text,
            "https://",
            refused.as_deref(),
        );
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
        let pinned: Vec<(NodeId, String, Pin)> = match frame {
            true => n
                .children()
                .iter()
                .filter_map(|c| doc.get(*c))
                .filter(|c| c.play().pin != Pin::Scrolls)
                .map(|c| (c.id(), name(doc, c.id()), c.play().pin))
                .collect(),
            false => Vec::new(),
        };
        if !frame && parent.is_none() && !text && !instance {
            return;
        }
        let mut next: Option<(LayerPlay, &'static str)> = None;
        let mut select = None;
        self.panel(ui, "Prototype", None, |app, ui| {
            let w = ui.available_width();
            if frame && app.card_fold(ui, "Prototype scrolling", "Scrolling") {
                ui::labelled(ui, "Axis", |ui| {
                    let cur = [
                        ScrollAxis::None,
                        ScrollAxis::Vertical,
                        ScrollAxis::Horizontal,
                        ScrollAxis::Both,
                    ]
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
                        |p, i, r, on| {
                            ui::segment_label(
                                p,
                                r,
                                ["None", "Vertical", "Horizontal", "Both"][i],
                                on,
                            )
                        },
                    );
                    if let Some(i) = pick.filter(|i| *i != cur) {
                        let mut p = play.clone();
                        p.scroll = [
                            ScrollAxis::None,
                            ScrollAxis::Vertical,
                            ScrollAxis::Horizontal,
                            ScrollAxis::Both,
                        ][i];
                        next = Some((p, "Scrolling"));
                    }
                });
                // The children pinned in it, a list row each with its pin as the
                // row's note; a click selects the child, whose card sets the pin.
                if !pinned.is_empty() {
                    ui::labelled(ui, "Pinned", |ui| {
                        ui.spacing_mut().item_spacing.y = 0.0;
                        for (child, who, pin) in &pinned {
                            let word = if *pin == Pin::Fixed {
                                "Fixed"
                            } else {
                                "Sticky"
                            };
                            let row = ui::MenuRow::new("", who).note(word);
                            if ui::menu_row(ui, row, ui::MENU_ROW_H)
                                .on_hover_text("Select this layer")
                                .clicked()
                            {
                                select = Some(*child);
                            }
                        }
                    });
                }
                // Snap appears once the frame scrolls.
                if play.scroll != ScrollAxis::None {
                    ui::labelled(ui, "Snap", |ui| {
                        let snaps = [Snap::None, Snap::Children, Snap::Pages];
                        let cur = snaps.iter().position(|s| *s == play.snap).unwrap_or(0);
                        let pick = ui::segmented_tipped(
                            ui,
                            w,
                            ui::SEGMENT_CELL_H,
                            3,
                            cur,
                            |_| true,
                            |i| match i {
                                0 => "Rests wherever it is let go",
                                1 => "Rests on its children's edges",
                                _ => "Rests on whole steps of its own size",
                            },
                            |p, i, r, on| {
                                ui::segment_label(p, r, ["None", "Children", "Paging"][i], on)
                            },
                        );
                        if let Some(i) = pick.filter(|i| *i != cur) {
                            let mut p = play.clone();
                            p.snap = snaps[i];
                            next = Some((p, "Scrolling"));
                        }
                        // *Paging* has one answer, so it takes no edge.
                        if play.snap == Snap::Children {
                            let aligns = [SnapAlign::Start, SnapAlign::Centre, SnapAlign::End];
                            let cur = aligns
                                .iter()
                                .position(|a| *a == play.snap_align)
                                .unwrap_or(0);
                            let pick = ui::segmented_tipped(
                                ui,
                                w,
                                ui::SEGMENT_CELL_H,
                                3,
                                cur,
                                |_| true,
                                |i| match i {
                                    0 => "A child's leading edge rests at the frame's",
                                    1 => "A child rests centred in the frame",
                                    _ => "A child's trailing edge rests at the frame's",
                                },
                                |p, i, r, on| {
                                    ui::segment_label(p, r, ["Start", "Centre", "End"][i], on)
                                },
                            );
                            if let Some(i) = pick.filter(|i| *i != cur) {
                                let mut p = play.clone();
                                p.snap_align = aligns[i];
                                next = Some((p, "Scrolling"));
                            }
                        }
                    });
                }
            }
            if parent.is_some() && app.card_fold(ui, "Prototype pin", "In a scrolling frame") {
                let cur = [Pin::Scrolls, Pin::Fixed, Pin::Sticky]
                    .iter()
                    .position(|p| *p == play.pin)
                    .unwrap_or(0);
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
            }
            if parent.is_some() {
                app.scroll_effect_section(ui, id, &play, w, &mut next);
                app.dragging_section(ui, id, &play, w, &mut next);
            }
            if (text || instance) && app.card_fold(ui, "Prototype input", "Input") {
                ui.scope(|ui| {
                    ui.spacing_mut().item_spacing.y = ui::CARD_ROW_GAP;
                    let kind = if text {
                        VarKind::String
                    } else {
                        VarKind::Boolean
                    };
                    let current = play.input.as_ref().map(|i| i.var);
                    ui.horizontal(|ui| {
                        ui.spacing_mut().item_spacing.x = COL;
                        let cw = if current.is_some() {
                            w - ui::CONTROL_H - COL
                        } else {
                            w
                        };
                        ui.scope(|ui| {
                            ui.set_width(cw);
                            if let Some(v) =
                                app.variable_combo(ui, ("input", id), current, Some(kind))
                            {
                                let mut p = play.clone();
                                let property = match text {
                                    true => None,
                                    false => p
                                        .input
                                        .as_ref()
                                        .and_then(|i| i.property.clone())
                                        .or_else(|| bool_props.first().cloned()),
                                };
                                p.input = Some(Input { var: v, property });
                                next = Some((p, "Input"));
                            }
                        });
                        if current.is_some()
                            && ui::field_button(ui, icon::X, ui::CONTROL_H, FieldButton::Off)
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
                        let face = ui::combo_text(&shown);
                        ui::fitted_combo(ui, ("input-prop", id), w, face, &shown, |ui| {
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
                        .on_hover_text(
                            "The property that shows the variable, such as a checkbox's Checked",
                        );
                        if let Some(prop) = pick {
                            let mut p = play.clone();
                            p.input = Some(Input {
                                var: input.var,
                                property: Some(prop),
                            });
                            next = Some((p, "Input"));
                        }
                    }
                    // Unbound, the note says what binding would do.
                    let note = match (text, current.is_some()) {
                        (true, true) => "Typing into it while playing sets the variable",
                        (true, false) => "Bind a string variable to type into it while playing",
                        (false, true) => "Ticking it while playing sets the variable",
                        (false, false) => "Bind a boolean variable to tick it while playing",
                    };
                    ui::selection_note(ui, note);
                });
            }
        });
        if let Some(child) = select {
            self.session.selection.set_one(child);
            self.reveal_selection();
        }
        if let Some((p, label)) = next {
            self.session.name_step(label, play_step_glyph(label));
            self.commit_edit(Transaction(vec![Operation::SetPlay { id, play: p }]));
        }
    }

    /// The Prototype card's *Dragging*: whether the layer follows the drag, its
    /// axis and range, its snap points as offsets from where it is drawn (the
    /// drawn place first, renamable and never removed), its release, and how it
    /// settles. The points come before *Release*, whose *Snaps* waits on one.
    fn dragging_section(
        &mut self,
        ui: &mut egui::Ui,
        id: NodeId,
        play: &LayerPlay,
        w: f32,
        next: &mut Option<(LayerPlay, &'static str)>,
    ) {
        if !self.card_fold(ui, "Prototype dragging", "Dragging") {
            return;
        }
        ui.scope(|ui| {
            ui.spacing_mut().item_spacing.y = ui::CARD_ROW_GAP;
            let on = play.follow.is_some();
            if ui::switch_row(ui, "Follows the drag", on, ui::CONTROL_H)
                .on_hover_text("While playing, the layer moves with a drag that starts on it")
                .clicked()
            {
                let mut p = play.clone();
                p.follow = match on {
                    true => None,
                    false => Some(Follow::default()),
                };
                *next = Some((p, "Dragging"));
            }
            let Some(f) = play.follow.clone() else {
                return;
            };
            let with = |g: &dyn Fn(&mut Follow)| {
                let mut p = play.clone();
                if let Some(f) = &mut p.follow {
                    g(f);
                }
                p
            };
            // In *Scrolling*'s words and order; `None` reads as both, so it is
            // not offered.
            ui::labelled(ui, "Axis", |ui| {
                let axes = [
                    ScrollAxis::Vertical,
                    ScrollAxis::Horizontal,
                    ScrollAxis::Both,
                ];
                let cur = axes.iter().position(|a| *a == f.axis).unwrap_or(2);
                let pick = ui::segmented_tipped(
                    ui,
                    w,
                    ui::SEGMENT_CELL_H,
                    3,
                    cur,
                    |_| true,
                    |i| match i {
                        0 => "Moves up and down only",
                        1 => "Moves across only",
                        _ => "Moves any way it is dragged",
                    },
                    |p, i, r, on| {
                        ui::segment_label(p, r, ["Vertical", "Horizontal", "Both"][i], on)
                    },
                );
                if let Some(i) = pick.filter(|i| *i != cur) {
                    *next = Some((with(&|f| f.axis = axes[i]), "Dragging"));
                }
            });
            ui::labelled(ui, "Range", |ui| {
                let ranges = [FollowRange::Free, FollowRange::Parent, FollowRange::Points];
                let cur = ranges.iter().position(|r| *r == f.range).unwrap_or(0);
                let pick = ui::segmented_tipped(
                    ui,
                    w,
                    ui::SEGMENT_CELL_H,
                    3,
                    cur,
                    |_| true,
                    |i| match i {
                        0 => "Goes wherever it is dragged",
                        1 => "Its box stays inside its parent's",
                        _ => "Between its outermost snap points",
                    },
                    |p, i, r, on| {
                        ui::segment_label(p, r, ["Free", "Within parent", "Snap points"][i], on)
                    },
                );
                if let Some(i) = pick.filter(|i| *i != cur) {
                    *next = Some((with(&|f| f.range = ranges[i]), "Dragging"));
                }
            });
            ui::labelled(ui, "Snap points", |ui| {
                // Where it is drawn: renamed like the others, never removed.
                ui.horizontal(|ui| {
                    ui.spacing_mut().item_spacing.x = COL;
                    let name_w = w - ui::CONTROL_H - COL;
                    let tip = Some("The point where the layer is drawn");
                    if let Some(name) =
                        self.point_name_field(ui, (id, None), f.drawn_name(), name_w, tip)
                    {
                        let drawn = (name != DRAWN_POINT).then_some(name);
                        *next = Some((with(&|f| f.drawn = drawn.clone()), "Rename snap point"));
                    }
                    ui.add_enabled_ui(false, |ui| {
                        ui::field_button(ui, icon::X, ui::CONTROL_H, FieldButton::Off)
                    })
                    .inner
                    .on_disabled_hover_text("Where the layer is drawn is always a point");
                });
                let mut gone = None;
                for k in &f.points {
                    self.drag_point_row(ui, id, play, k, w, &mut gone);
                }
                if let Some(p) = gone {
                    *next = Some((
                        with(&|f| f.points.retain(|k| k.id != p)),
                        "Remove snap point",
                    ));
                }
                if ui::field_button(ui, icon::PLUS, ui::CONTROL_H, FieldButton::Off)
                    .on_hover_text("Add a snap point")
                    .clicked()
                {
                    let item = self.session.ids.mint_item();
                    let point = new_drag_point(&f);
                    *next = Some((
                        with(&|f| f.points.push(Keyed::new(item, point.clone()))),
                        "Add snap point",
                    ));
                }
            });
            ui::labelled(ui, "Release", |ui| {
                let releases = [Release::Stays, Release::Returns, Release::Snaps];
                let cur = releases.iter().position(|r| *r == f.release).unwrap_or(0);
                let has_points = !f.points.is_empty();
                let pick = ui::segmented_tipped(
                    ui,
                    w,
                    ui::SEGMENT_CELL_H,
                    3,
                    cur,
                    |i| i != 2 || has_points,
                    |i| match (i, has_points) {
                        (0, _) => "Stays where it is let go",
                        (1, _) => "Goes back to where it is drawn",
                        (_, false) => "Add a snap point first",
                        _ => {
                            "Settles on the nearest point, or the next a quick release throws it to"
                        }
                    },
                    |p, i, r, on| ui::segment_label(p, r, ["Stays", "Returns", "Snaps"][i], on),
                );
                if let Some(i) = pick.filter(|i| *i != cur) {
                    *next = Some((with(&|f| f.release = releases[i]), "Dragging"));
                }
            });
            // How it moves after a release: back into its range, home, or onto a
            // point. The transition's timing rows, without a kind or a side; dim
            // while nothing settles, free and staying where it is let go.
            let still = f.range == FollowRange::Free && f.release == Release::Stays;
            ui::labelled(ui, "Settle", |ui| {
                let fields = ui::disable_unless(ui, !still, |ui| {
                    self.timing_fields(ui, ("settle", id), f.settle, w)
                });
                if let Some(t) = fields.inner {
                    *next = Some((with(&|f| f.settle = t), "Dragging"));
                }
                if still {
                    ui.interact(
                        fields.response.rect,
                        ui.id().with(("settle-still", id)),
                        egui::Sense::hover(),
                    )
                    .on_hover_text(
                        "Nothing settles: it goes anywhere and stays where it is let go",
                    );
                }
            });
        });
    }

    /// The Prototype card's *With the scroll*: the scrolling frame the effect
    /// follows (the nearest above the layer unless one is picked), the range of
    /// its offset the effect spans, and a row per property with its value at
    /// each end. Dim on a layer whose screen has no scrolling frame.
    fn scroll_effect_section(
        &mut self,
        ui: &mut egui::Ui,
        id: NodeId,
        play: &LayerPlay,
        w: f32,
        next: &mut Option<(LayerPlay, &'static str)>,
    ) {
        if !self.card_fold(ui, "Prototype scroll effect", "With the scroll") {
            return;
        }
        let doc = &self.session.doc;
        // The layer's own scroll is not one it can follow.
        let options: Vec<NodeId> = scrolling_frames(doc, id)
            .into_iter()
            .filter(|f| *f != id)
            .collect();
        let above = |f: NodeId| {
            let mut at = doc.get(id).and_then(|n| n.parent());
            while let Some(x) = at {
                if x == f {
                    return true;
                }
                at = doc.get(x).and_then(|n| n.parent());
            }
            false
        };
        // Those above come first, nearest first.
        let nearest = options.first().copied().filter(|f| above(*f));
        let with = |e: Option<ScrollEffect>| {
            let mut p = play.clone();
            p.scroll_effect = e;
            p
        };
        ui.scope(|ui| {
            ui.spacing_mut().item_spacing.y = ui::CARD_ROW_GAP;
            let Some(e) = play.scroll_effect.clone() else {
                let pick = ui
                    .add_enabled_ui(!options.is_empty(), |ui| {
                        scroll_prop_menu(ui, &ScrollProp::ALL, |r| {
                            r.on_hover_text("Add a property that follows the scroll")
                                .on_disabled_hover_text("No frame on the screen scrolls")
                        })
                    })
                    .inner;
                if let Some(p) = pick {
                    *next = Some((
                        with(Some(ScrollEffect {
                            // The nearest frame above, or else the screen's first.
                            frame: match nearest {
                                Some(_) => None,
                                None => options.first().copied(),
                            },
                            from: 0.0,
                            to: NEW_EFFECT_SPAN,
                            props: vec![new_effect_ends(p)],
                        })),
                        "Add scroll effect",
                    ));
                }
                return;
            };
            let tx = |e: ScrollEffect| {
                Transaction(vec![Operation::SetPlay {
                    id,
                    play: with(Some(e)),
                }])
            };
            let shown = match e.frame {
                Some(f) => Some(mastik_core::prototype::resolve_target(
                    &self.session.doc,
                    id,
                    f,
                )),
                None => nearest,
            };
            let doc = &self.session.doc;
            let combo = ui::labelled(ui, "Frame", |ui| {
                layer_combo(
                    ui,
                    doc,
                    ("effect-frame", id),
                    Some(icon::FRAME),
                    shown,
                    &options,
                    w,
                )
            });
            if let Some(f) = combo {
                let mut e2 = e.clone();
                e2.frame = Some(f);
                *next = Some((with(Some(e2)), "Scroll effect"));
            }
            // A field `fw` wide over one number of the effect, `k` its shown
            // scale. A length scrubs at the Transform card's X and Y speed.
            let field = |app: &mut Self,
                         ui: &mut egui::Ui,
                         key: (usize, bool),
                         fw: f32,
                         prefix: &'static str,
                         suffix: &'static str,
                         value: f64,
                         k: f64,
                         set: &dyn Fn(&mut ScrollEffect, f64)| {
                let rid = ui.id().with(("effect", key));
                let mut v = value * k;
                if let Some(x) = app.proto.scrub.filter(|(i, _)| *i == rid) {
                    v = x.1;
                }
                let speed = if suffix == "px" { 0.5 } else { 1.0 };
                let resp = ui::value_field_suffixed(
                    ui,
                    egui::vec2(fw, ui::CONTROL_H),
                    Prefix::Text(prefix),
                    (!suffix.is_empty()).then_some(ui::Suffix {
                        text: suffix,
                        clickable: false,
                        tooltip: "",
                    }),
                    &mut v,
                    Scrub::whole(speed),
                    |d| d.custom_formatter(ui::number(0)),
                )
                .0;
                let v = v.round();
                let t = match (v / k - value).abs() > 1e-9 {
                    true => {
                        let mut e2 = e.clone();
                        set(&mut e2, v / k);
                        tx(e2)
                    }
                    false => Transaction::default(),
                };
                app.proto_valve(rid, v, &resp, "Scroll effect", icon::INTERACTION, t);
            };
            let half = (w - COL) / 2.0;
            ui.horizontal(|ui| {
                ui.spacing_mut().item_spacing.x = COL;
                field(
                    self,
                    ui,
                    (usize::MAX, false),
                    half,
                    "From",
                    "px",
                    e.from,
                    1.0,
                    &|e, v| e.from = v,
                );
                field(
                    self,
                    ui,
                    (usize::MAX, true),
                    half,
                    "To",
                    "px",
                    e.to,
                    1.0,
                    &|e, v| e.to = v,
                );
            });
            // A property under its name, its values at the effect's *From* and
            // *To* side by side, then its `×`.
            let mut gone = None;
            let pw = (w - ui::CONTROL_H - 2.0 * COL) / 2.0;
            for (i, p) in e.props.iter().enumerate() {
                let (word, suffix, k) = scroll_prop_words(p.prop);
                ui::labelled(ui, word, |ui| {
                    ui.horizontal(|ui| {
                        ui.spacing_mut().item_spacing.x = COL;
                        field(
                            self,
                            ui,
                            (i, false),
                            pw,
                            "From",
                            suffix,
                            p.start,
                            k,
                            &|e, v| e.props[i].start = v,
                        );
                        field(self, ui, (i, true), pw, "To", suffix, p.end, k, &|e, v| {
                            e.props[i].end = v
                        });
                        if ui::field_button(ui, icon::X, ui::CONTROL_H, FieldButton::Off)
                            .on_hover_text("Remove this property")
                            .clicked()
                        {
                            gone = Some(i);
                        }
                    });
                });
            }
            if let Some(i) = gone {
                let mut e2 = e.clone();
                e2.props.remove(i);
                let left = (!e2.props.is_empty()).then_some(e2);
                *next = Some((with(left), "Remove scroll effect"));
            }
            let missing: Vec<ScrollProp> = ScrollProp::ALL
                .into_iter()
                .filter(|p| !e.props.iter().any(|x| x.prop == *p))
                .collect();
            if !missing.is_empty()
                && let Some(p) =
                    scroll_prop_menu(ui, &missing, |r| r.on_hover_text("Add a property"))
            {
                let mut e2 = e.clone();
                e2.props.push(new_effect_ends(p));
                *next = Some((with(Some(e2)), "Add scroll effect"));
            }
        });
    }

    /// One snap point's row: its name, then its offset across and down.
    fn drag_point_row(
        &mut self,
        ui: &mut egui::Ui,
        id: NodeId,
        play: &LayerPlay,
        k: &Keyed<DragPoint>,
        w: f32,
        gone: &mut Option<ItemId>,
    ) {
        let set = |g: &dyn Fn(&mut DragPoint)| {
            let mut p = play.clone();
            if let Some(f) = &mut p.follow
                && let Some(x) = f.points.iter_mut().find(|x| x.id == k.id)
            {
                g(&mut x.value);
            }
            Transaction(vec![Operation::SetPlay { id, play: p }])
        };
        ui.horizontal(|ui| {
            ui.spacing_mut().item_spacing.x = COL;
            let name_w = w - ui::CONTROL_H - COL;
            if let Some(name) =
                self.point_name_field(ui, (id, Some(k.id)), &k.value.name, name_w, None)
            {
                self.session
                    .name_step("Rename snap point", icon::CROSSHAIR_SIMPLE);
                self.commit_edit(set(&|p| p.name = name.clone()));
            }
            if ui::field_button(ui, icon::X, ui::CONTROL_H, FieldButton::Off)
                .on_hover_text("Remove this snap point")
                .clicked()
            {
                *gone = Some(k.id);
            }
        });
        // A drag of its ghost on the canvas shows here as it goes.
        let mark = ProtoMark::Point {
            node: id,
            point: k.id,
        };
        let live = self
            .proto_mark_value(mark)
            .unwrap_or(mastik_core::kurbo::Vec2::new(k.value.x, k.value.y));
        ui.horizontal(|ui| {
            ui.spacing_mut().item_spacing.x = COL;
            let half = (w - COL) / 2.0;
            let mut held = None;
            for (axis, label) in [(true, "X"), (false, "Y")] {
                let rid = ui.id().with(("point", k.id, axis));
                let mut v = if axis { live.x } else { live.y };
                if let Some(x) = self.proto.scrub.filter(|(i, _)| *i == rid) {
                    v = x.1;
                }
                // The Transform card's X and Y scrub speed, whole units kept.
                let resp = ui::value_field_suffixed(
                    ui,
                    egui::vec2(half, ui::CONTROL_H),
                    Prefix::Text(label),
                    None,
                    &mut v,
                    Scrub::whole(0.5),
                    |d| d.custom_formatter(ui::number(0)),
                )
                .0;
                let v = v.round();
                if resp.dragged() || resp.has_focus() {
                    held = Some(match axis {
                        true => mastik_core::kurbo::Vec2::new(v, live.y),
                        false => mastik_core::kurbo::Vec2::new(live.x, v),
                    });
                }
                let tx = match v != (if axis { k.value.x } else { k.value.y }) {
                    true => set(&|p| match axis {
                        true => p.x = v,
                        false => p.y = v,
                    }),
                    false => Transaction::default(),
                };
                self.proto_valve(rid, v, &resp, "Move snap point", icon::CROSSHAIR_SIMPLE, tx);
            }
            match held {
                Some(v) => self.proto.held_point = Some((k.id, v)),
                None if self.proto.held_point.is_some_and(|(p, _)| p == k.id) => {
                    self.proto.held_point = None;
                }
                None => {}
            }
        });
    }

    /// A snap point's name field, `point` naming the layer and the point (`None`
    /// for where it is drawn): typed into a `ui::Draft`, so `Escape` gives the
    /// typing up. Answers a new, non-empty name on a committing defocus.
    fn point_name_field(
        &mut self,
        ui: &mut egui::Ui,
        point: (NodeId, Option<ItemId>),
        current: &str,
        w: f32,
        tip: Option<&str>,
    ) -> Option<String> {
        let key = DraftKey::PointName(point.0, point.1);
        let ctx = ui.ctx().clone();
        let mut text = ui::Draft::live(&mut self.proto.draft, &ctx, key)
            .map_or(current.to_string(), |d| d.text.clone());
        let mut resp = ui::text_field(ui, egui::vec2(w, ui::CONTROL_H), &mut text, "Name");
        if let Some(tip) = tip {
            resp = resp.on_hover_text(tip);
        }
        let typed = ui::Draft::follow(&mut self.proto.draft, &resp, key, &text)?;
        let typed = typed.trim().to_string();
        (!typed.is_empty() && typed != current).then_some(typed)
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
                ui::empty_note(
                    ui,
                    "A flow is where the exported prototype starts; add one with +",
                );
            }
            for (n, f) in flows.iter().enumerate() {
                // A rule between one flow's starting values and the next flow.
                if n > 0 {
                    ui::card_rule(ui);
                }
                let row = ui.horizontal(|ui| {
                    ui.spacing_mut().item_spacing.x = COL;
                    let w = ui.available_width() - ui::CONTROL_H - COL;
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
                    if let Some(s) = layer_combo(
                        ui,
                        doc,
                        ("flow-start", f.id),
                        Some(icon::FRAME),
                        f.start,
                        &frames,
                        half,
                    ) {
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
                    if ui::field_button(ui, icon::X, ui::CONTROL_H, FieldButton::Off)
                        .on_hover_text("Remove this flow")
                        .clicked()
                    {
                        next = Some((
                            flows.iter().filter(|x| x.id != f.id).cloned().collect(),
                            "Remove flow",
                        ));
                    }
                });
                // The flow Play opens first wears the accent on its name field's
                // leading edge, as its chip on the canvas wears the blue: the
                // field's own outline, kept to its left corners so it wraps them.
                if n == 0 {
                    let r = row.response.rect;
                    let field = egui::Rect::from_min_size(
                        r.left_top(),
                        egui::vec2(r.width(), ui::CONTROL_H),
                    );
                    let round = ui.visuals().widgets.inactive.corner_radius;
                    let strip = field.with_max_x(field.left() + f32::from(round.nw));
                    ui.painter().with_clip_rect(strip).rect_stroke(
                        field,
                        round,
                        egui::Stroke::new(FIRST_FLOW_BAR, color::accent_400()),
                        egui::StrokeKind::Inside,
                    );
                    ui.interact(
                        r.with_max_x(r.left() + 6.0),
                        ui.id().with("first-flow"),
                        egui::Sense::hover(),
                    )
                    .on_hover_text("Play opens this flow when no frame is selected");
                }
                if let Some(modes) = app.flow_modes(ui, f) {
                    let list = flows
                        .iter()
                        .map(|x| match x.id == f.id {
                            true => x.map(|v| Flow {
                                modes: modes.0.clone(),
                                ..v.clone()
                            }),
                            false => x.clone(),
                        })
                        .collect();
                    next = Some((list, modes.1));
                }
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
                        let w = ui.available_width() - ui::CONTROL_H - COL;
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
                        if ui::field_button(ui, icon::X, ui::CONTROL_H, FieldButton::Off)
                            .on_hover_text("Remove this starting value")
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
            list.push(Keyed::new(
                self.session.ids.mint_item(),
                Flow {
                    name: free_flow_name(&list),
                    start,
                    modes: Vec::new(),
                    values: Vec::new(),
                },
            ));
            next = Some((list, "Add flow"));
        }
        if let Some((flows, label)) = next {
            self.session.name_step(label, icon::INTERACTION);
            self.commit_edit(Transaction(vec![Operation::SetFlows { id: page, flows }]));
        }
    }

    /// A flow's starting modes: each a collection and the mode it reads as the
    /// flow starts, then *Add a starting mode*. Answers the new list and its
    /// undo name.
    fn flow_modes(
        &mut self,
        ui: &mut egui::Ui,
        f: &Keyed<Flow>,
    ) -> Option<(Vec<ModeId>, &'static str)> {
        let doc = &self.session.doc;
        let t = doc.variables();
        let cols = switchable_collections(doc);
        let mut out = None;
        for (i, mode) in f.modes.iter().enumerate() {
            let col = cols.iter().find(|c| c.modes.iter().any(|m| m.0 == *mode));
            ui.horizontal(|ui| {
                ui.spacing_mut().item_spacing.x = COL;
                let w = ui.available_width() - ui::CONTROL_H - COL;
                let half = (w - COL) / 2.0;
                ui.allocate_ui(egui::vec2(half, ui::CONTROL_H), |ui| {
                    // A collection another starting mode names is not offered
                    // twice: one mode per collection.
                    let free: Vec<Switchable> = cols
                        .iter()
                        .filter(|c| {
                            col.is_some_and(|x| x.owner == c.owner)
                                || !f.modes.iter().any(|m| t.owner_of(*m) == Some(c.owner))
                        })
                        .cloned()
                        .collect();
                    if let Some(c) =
                        collection_combo(ui, doc, ("flow-col", f.id, i), half, &free, col)
                    {
                        let now = doc.mode_of_at(f.start, c.owner);
                        if let Some(m) = c.modes.iter().find(|m| Some(m.0) != now) {
                            let mut modes = f.modes.clone();
                            modes[i] = m.0;
                            out = Some((modes, "Change starting mode"));
                        }
                    }
                });
                let shown = col
                    .and_then(|c| c.modes.iter().find(|m| m.0 == *mode))
                    .map_or("Missing mode".to_string(), |m| m.1.to_owned());
                ui::fitted_combo(ui, ("flow-mode", f.id, i), half, &shown, &shown, |ui| {
                    ui::menu_rows(ui);
                    for (m, n) in col.map_or(&[][..], |c| &c.modes[..]) {
                        let on = m == mode;
                        if ui.selectable_label(on, *n).clicked() && !on {
                            let mut modes = f.modes.clone();
                            modes[i] = *m;
                            out = Some((modes, "Change starting mode"));
                        }
                    }
                });
                if ui::field_button(ui, icon::X, ui::CONTROL_H, FieldButton::Off)
                    .on_hover_text("Remove this starting mode")
                    .clicked()
                {
                    let mut modes = f.modes.clone();
                    modes.remove(i);
                    out = Some((modes, "Remove starting mode"));
                }
            });
        }
        // The first collection no starting mode names yet, at the mode the
        // start frame does not read now.
        let unused = cols
            .iter()
            .find(|c| !f.modes.iter().any(|m| t.owner_of(*m) == Some(c.owner)))
            .and_then(|c| {
                let now = doc.mode_of_at(f.start, c.owner);
                c.modes.iter().find(|m| Some(m.0) != now).map(|m| m.0)
            });
        let why = match (cols.is_empty(), unused) {
            (true, _) => "No collection has a second mode",
            (false, None) => "Every collection has a starting mode",
            (false, Some(_)) => "Set a mode as the flow starts",
        };
        if crate::settings::text_button(
            ui,
            "Add a starting mode",
            match unused {
                Some(_) => ui::FieldButton::Off,
                None => ui::FieldButton::Disabled,
            },
        )
        .on_hover_text(why)
        .clicked()
            && let Some(m) = unused
        {
            let mut modes = f.modes.clone();
            modes.push(m);
            out = Some((modes, "Add starting mode"));
        }
        out
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
        let resp = ui::refusable_field(
            ui,
            egui::vec2(w, ui::CONTROL_H),
            &mut text,
            "Value",
            refused.as_deref(),
        );
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
                let face = ui::glyph_and_text(icon::DEVICES, &shown);
                let combo_w = w - ui::CONTROL_H - COL;
                ui.horizontal(|ui| {
                    ui.spacing_mut().item_spacing.x = COL;
                    ui::fitted_combo(ui, ("device", page), combo_w, face, &shown, |ui| {
                        ui::menu_rows(ui);
                        let none = ui::MenuRow::new("", "None").selected(play.device.is_none());
                        if ui::menu_row(ui, none, ui::MENU_ROW_H).clicked() && play.device.is_some()
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
                                // The size is the row's note, as Scaling's meanings are.
                                let size = format!("{pw} × {ph}");
                                let row = ui::MenuRow::new("", n).note(&size).selected(on);
                                let row = ui::menu_row(ui, row, ui::MENU_ROW_H);
                                if row.clicked() && !on {
                                    pick = Some(Some((*n).to_string()));
                                }
                            }
                        }
                    })
                    .response
                    .on_hover_text("The device drawn round the screen while playing");
                    // Beside the device, lit while turned: a screen with no device
                    // has no orientation.
                    let can = play.can_turn();
                    let state = match (can, play.landscape) {
                        (false, _) => FieldButton::Disabled,
                        (true, true) => FieldButton::On,
                        (true, false) => FieldButton::Off,
                    };
                    let tip = match (can, play.landscape) {
                        (false, _) => "Pick a device to turn",
                        (true, true) => "Turn to portrait",
                        (true, false) => "Turn to landscape",
                    };
                    let turn = ui::field_button(ui, icon::TURN_DEVICE, ui::CONTROL_H, state);
                    let turn = match can {
                        true => turn.on_hover_text(tip),
                        false => turn.on_disabled_hover_text(tip),
                    };
                    if can && turn.clicked() {
                        next = Some((
                            PagePlay {
                                landscape: !play.landscape,
                                ..play.clone()
                            },
                            "Turn device",
                        ));
                    }
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
                if let Some(s) = scaling_combo(ui, ("scaling", page), w, None, Some(play.scaling)) {
                    next = Some((
                        PagePlay {
                            scaling: s.unwrap_or_default(),
                            ..play.clone()
                        },
                        "Scaling",
                    ));
                }
            });
            // What is laid out other than as drawn, and a start frame that has
            // no layout to do it with; quiet when nothing differs.
            let doc = &app.session.doc;
            let device = play
                .device
                .as_deref()
                .and_then(mastik_core::presets::device_size);
            if let (Some(d), Some(name)) = (device, play.device.as_deref()) {
                let note = match play.landscape {
                    true => format!("{name}, turned: {} × {}", d.height, d.width),
                    false => format!("{name}: {} × {}", d.width, d.height),
                };
                ui::selection_note(ui, &note);
            }
            // A device lays the screen out at its size under every scaling.
            if play.can_turn() || play.scaling == Scaling::Responsive {
                let flows = doc.page(page).map(|p| p.flows.clone()).unwrap_or_default();
                let flat = flows.iter().find(|f| {
                    doc.get(f.value.start)
                        .is_some_and(|n| n.display().is_none())
                });
                if let Some(f) = flat {
                    ui::selection_note(
                        ui,
                        &format!(
                            "{}'s start frame has no layout; it keeps its children where drawn",
                            f.value.name
                        ),
                    );
                }
            }
            ui::labelled(ui, "Default transition", |ui| {
                if let Some(t) = app.transition_fields(ui, ("page-tr", page), play.transition, w) {
                    next = Some((
                        PagePlay {
                            transition: t,
                            ..play.clone()
                        },
                        "Default transition",
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
    let face = ui::glyph_and_text(icon::FRAME, &shown);
    ui::fitted_combo(ui, salt, w, face, &shown, |ui| {
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
    pick
}

/// A dropdown of `options` by name, its face led by `glyph` when given (a list
/// of frames wears `icon::FRAME`, as [`frame_combo`] does); answers a new pick.
fn layer_combo(
    ui: &mut egui::Ui,
    doc: &Document,
    salt: impl std::hash::Hash + std::fmt::Debug,
    glyph: Option<&str>,
    current: impl Into<Option<NodeId>>,
    options: &[NodeId],
    w: f32,
) -> Option<NodeId> {
    let current = current.into();
    let shown = match current {
        Some(c) if doc.get(c).is_some() => name(doc, c),
        Some(_) => "Missing layer".into(),
        None => "None".into(),
    };
    let mut pick = None;
    let face = match glyph {
        Some(g) => ui::glyph_and_text(g, &shown),
        None => ui::combo_text(&shown),
    };
    ui::fitted_combo(ui, salt, w, face, &shown, |ui| {
        ui::menu_rows(ui);
        for o in options {
            let on = Some(*o) == current;
            if ui.selectable_label(on, name(doc, *o)).clicked() && !on {
                pick = Some(*o);
            }
        }
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

    /// A picker's targets on a screen holding a list keep the template and
    /// leave out its copies, whose ids every regeneration replaces.
    /// flip: `pickable_under` without its `copy_owner` filter → fails at "no
    /// copy".
    #[test]
    fn a_picker_offers_no_list_copy() {
        use mastik_core::chart::{Column, ColumnId, Table};
        use mastik_core::list::{self, DataSet, Source};
        let ctx = egui::Context::default();
        let f = fixture(&ctx);
        let mut doc = f.app.session.doc.clone();
        let mut ids = IdSource::new(0xB1);
        let data = DataSet {
            id: ids.mint_data_set(),
            name: "Rows".into(),
            table: Table {
                columns: vec![Column {
                    id: ColumnId(0),
                    name: "A".into(),
                }],
                rows: vec![vec!["1".into()], vec!["2".into()]],
            },
            source: None,
            pictures: Vec::new(),
        };
        let res = mastik_core::Resolved::rebuild(&doc);
        let (tx, l) = list::make(&doc, &res, &mut ids, f.inst, Source::New(data)).unwrap();
        doc.apply(&tx).expect("the list");
        list::regenerate_outside_history(&mut doc, &[l]);
        let copy = doc.get(l).unwrap().list().unwrap().copies[0];
        let options = pickable_under(&doc, f.home);
        assert!(options.contains(&f.inst), "the template stays");
        assert!(!options.contains(&copy), "no copy");
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

    /// An action nested in a branch is found and removed by id.
    /// flip: `remove` without its descent into branches → fails at the removal.
    #[test]
    fn nested_actions_are_reached_by_id() {
        let mut list = nested();
        assert!(find(&list, item(5)).is_some(), "the else's action");
        assert!(remove(&mut list, item(5)));
        assert!(find(&list, item(5)).is_none());
        let at = list_at(&mut list, Place::Otherwise(item(2))).expect("the else");
        assert!(at.is_empty(), "the else is empty once its action goes");
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
        let (_, at) = expr_stored(&doc, "1 + )", None, Reads::Vars).expect_err("does not read");
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
        let stored =
            expr_stored(doc, "{cartCount} + 1", Some(VarKind::Number), Reads::Vars).expect("reads");
        assert_eq!(stored.0, format!("{{{}}} + 1", var.0.to_wire()));
        assert_eq!(expr_shown(doc, &stored), "{cartCount} + 1");
        assert!(
            expr_stored(doc, "{nope} + 1", None, Reads::Vars).is_err(),
            "an unknown name"
        );
        assert!(
            expr_stored(doc, "1 +", None, Reads::Vars).is_err(),
            "does not read"
        );
        assert!(
            expr_stored(doc, "{cartCount} + 1", Some(VarKind::String), Reads::Vars).is_err(),
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
            Some("Only a component with variants changes state")
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

    /// Delete with an action's popover open removes that action as one step and
    /// keeps the layer. flip: `dispatch` without its `remove_open_action` arm →
    /// fails at "the layer stays".
    #[test]
    fn delete_with_an_action_open_removes_the_action() {
        let ctx = egui::Context::default();
        let mut f = fixture(&ctx);
        let to = f.cart;
        let at = one_action(
            &mut f,
            Action::Navigate {
                to,
                transition: Transition::default(),
                keep_scroll: false,
            },
        );
        f.app.session.prototype_mode = true;
        f.app.open_link(at);
        f.app.dispatch(&ctx, crate::input::Action::Delete);
        let n = f.app.session.doc.get(f.add).expect("the layer stays");
        assert!(n.reactions()[0].actions.is_empty(), "the action goes");
        assert!(f.app.proto.open.is_none(), "its popover closes");
        assert!(f.app.session.undo());
        assert_eq!(
            f.app.session.doc.get(f.add).unwrap().reactions()[0]
                .actions
                .len(),
            1,
            "one step"
        );
    }

    /// A link a dot makes takes the page's default transition, and a second
    /// link points the click's navigation elsewhere rather than adding one.
    /// flip: `with_link` pushing a new navigation always → fails at "replaces".
    #[test]
    fn a_link_takes_the_pages_transition_and_replaces_the_clicks_navigation() {
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
        let (once, first, moved) = with_link(&f.app.session.doc, f.add, f.cart, &mut mint);
        assert!(!moved, "a new link");
        assert!(
            f.app
                .session
                .commit(Transaction(vec![Operation::SetReactions {
                    id: f.add,
                    reactions: once,
                }]))
        );
        let (twice, second, moved) = with_link(&f.app.session.doc, f.add, f.home, &mut mint);
        assert_eq!(twice.len(), 1, "one click reaction");
        assert_eq!(
            twice[0].actions.len(),
            1,
            "the second link replaces the first"
        );
        assert!(moved && second == first, "retargeted in place");
        let Action::Navigate { to, .. } = &twice[0].actions[0].value else {
            panic!("fixture: a navigation");
        };
        assert_eq!(*to, f.home);
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
        f.app.session.prototype_mode = true;
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
            [k] if matches!(k.actions.as_slice(), [a] if a.value == Action::Navigate { to: f.cart, transition: Transition::default(), keep_scroll: false })
        ));
        assert_eq!(
            f.app.proto.open.map(|at| at.action),
            Some(r[0].actions[0].id),
            "its popover opens"
        );
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
        f.app.session.prototype_mode = true;
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
        f.app.session.prototype_mode = false;
        assert_eq!(
            f.app.link_knob_at(dots[0].1, rect, 1.0),
            None,
            "design mode's none"
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

    /// The canvas's connectors: one per navigate, dotted inside a conditional, a
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
                    keep_scroll: false,
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
                                keep_scroll: false,
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
                    line: LinkLine::Solid,
                    at: ActionAt {
                        node: f.add,
                        reaction: item(10),
                        action: item(1),
                    },
                },
                Link {
                    from: f.add,
                    to: gone,
                    line: LinkLine::Dotted,
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
    /// with prototype mode on.
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
        f.app.session.prototype_mode = true;
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
                keep_scroll: false,
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

    /// A main in a set whose state property ties Hover lists Hover as playing
    /// live; untied, nothing; a plain layer, nothing.
    /// flip: `live_states` taking Pressed alone → fails at "tied";
    /// `state_instance` without its main-variant walk → fails at "a main variant".
    #[test]
    fn a_tied_hover_plays_live() {
        use mastik_core::variant::{Interaction, VariantProp, VariantSet};
        let ctx = egui::Context::default();
        let mut f = fixture(&ctx);
        let root = f.app.session.doc.first_page();
        let [set, rest, hover] = [(); 3].map(|_| f.app.session.ids.mint());
        let frame = |id, parent, index, name: &str| Operation::CreateNode {
            id,
            parent,
            index,
            kind: NodeKind::Artboard {
                size: Size::new(80.0, 30.0),
                corner_radii: Default::default(),
                corner_smoothing: 0.0,
            },
            transform: None,
            name: Some(name.into()),
        };
        let mut prop = VariantProp::new("State", vec!["Default".into(), "Hovered".into()]);
        let mut ops = vec![
            frame(set, root, 0, "Button"),
            frame(rest, set, 0, "Default"),
            frame(hover, set, 1, "Hovered"),
        ];
        for v in [rest, hover] {
            ops.push(Operation::SetComponent {
                id: v,
                component: true,
            });
        }
        ops.push(Operation::SetVariantSet {
            id: set,
            set: Some(VariantSet {
                props: vec![prop.clone()],
            }),
        });
        for (v, value) in [(rest, "Default"), (hover, "Hovered")] {
            ops.push(Operation::SetVariant {
                id: v,
                values: vec![value.into()],
            });
        }
        assert!(f.app.session.commit(Transaction(ops)));
        assert!(live_states(&f.app.session.doc, rest).is_empty(), "untied");
        prop.states.insert("Hovered".into(), Interaction::Hover);
        assert!(
            f.app
                .session
                .commit(Transaction(vec![Operation::SetVariantSet {
                    id: set,
                    set: Some(VariantSet { props: vec![prop] }),
                }]))
        );
        assert_eq!(
            live_states(&f.app.session.doc, rest),
            vec![("Hover", "Hovered".to_string())],
            "tied"
        );
        assert!(
            live_states(&f.app.session.doc, f.add).is_empty(),
            "a plain layer"
        );
        // A main variant changes its own state, to the set's next variant: what
        // a state that moves on is built from.
        let doc = &f.app.session.doc;
        assert_eq!(state_instance(doc, rest), Some(rest), "a main variant");
        let Some(Action::ChangeState { instance, to, .. }) =
            new_action(doc, rest, ActionKind::ChangeState)
        else {
            panic!("Change to on a main variant");
        };
        assert_eq!((instance, to), (rest, hover));
    }

    /// `@` in an expression lists the variables whose names hold what follows
    /// it, and nothing once the name is closed or no `@` is open.
    /// flip: `at_rows` ignoring the query → fails at "filtered".
    #[test]
    fn an_at_lists_the_variables_it_may_name() {
        use mastik_core::variable::{Cell, CollectionId, Mode, Variable};
        let ctx = egui::Context::default();
        let mut f = fixture(&ctx);
        let mode = f.app.session.ids.mint_mode();
        let [a, b] = [(); 2].map(|_| f.app.session.ids.mint_var());
        let number = |id, name: &str| Operation::AddVariable {
            var: Variable {
                id,
                name: name.into(),
                kind: VarKind::Number,
                cells: [(mode, Cell::Number(0.0))].into(),
                scope: None,
                origin: None,
                forked: None,
                description: String::new(),
            },
        };
        assert!(f.app.session.commit(Transaction(vec![
            Operation::AddMode {
                collection: CollectionId::FIRST,
                mode: Mode {
                    id: mode,
                    name: "Default".into(),
                },
                index: 0,
                cells: vec![],
            },
            number(a, "cartCount"),
            number(b, "signedIn"),
        ])));
        let names = |t: &str| -> Vec<String> {
            at_rows(&f.app.session.doc, t, Reads::Vars)
                .into_iter()
                .map(|r| r.label)
                .collect()
        };
        assert_eq!(names("1 + @"), vec!["cartCount", "signedIn"]);
        assert_eq!(names("1 + @cart"), vec!["cartCount"], "filtered");
        assert!(names("@cart + 1").is_empty(), "closed by a space");
        assert!(names("{cartCount} + 1").is_empty(), "no @");
    }

    /// An open `{` lists what `@` would, narrowing as the name is typed, spaces
    /// and all; `}` or a quote around it closes the list; a pick keeps the brace.
    /// flip: `name_opener` skipping the brace scan → fails at "opens on a brace".
    #[test]
    fn a_brace_lists_what_it_may_name() {
        use mastik_core::chart::{Column, ColumnId, Table};
        use mastik_core::list::DataSet;
        use mastik_core::variable::{Cell, CollectionId, Mode, Variable};
        let ctx = egui::Context::default();
        let mut f = fixture(&ctx);
        let mode = f.app.session.ids.mint_mode();
        let [a, b] = [(); 2].map(|_| f.app.session.ids.mint_var());
        let number = |id, name: &str| Operation::AddVariable {
            var: Variable {
                id,
                name: name.into(),
                kind: VarKind::Number,
                cells: [(mode, Cell::Number(0.0))].into(),
                scope: None,
                origin: None,
                forked: None,
                description: String::new(),
            },
        };
        let set = f.app.session.ids.mint_data_set();
        assert!(f.app.session.commit(Transaction(vec![
            Operation::AddMode {
                collection: CollectionId::FIRST,
                mode: Mode {
                    id: mode,
                    name: "Default".into(),
                },
                index: 0,
                cells: vec![],
            },
            number(a, "cart count"),
            number(b, "signedIn"),
            Operation::SetDataSet {
                id: set,
                set: Some(Box::new(DataSet {
                    id: set,
                    name: "Cart items".into(),
                    table: Table {
                        columns: vec![Column {
                            id: ColumnId(0),
                            name: "Line".into(),
                        }],
                        rows: vec![vec!["42".into()]],
                    },
                    source: None,
                    pictures: Vec::new(),
                })),
            },
        ])));
        let doc = &f.app.session.doc;
        let rows = |t: &str| at_rows(doc, t, Reads::Tables(None));
        let labels = |t: &str| -> Vec<String> { rows(t).into_iter().map(|r| r.label).collect() };
        assert_eq!(
            labels("1 + {"),
            vec!["cart count", "signedIn", "Cart items"],
            "opens on a brace"
        );
        assert_eq!(labels("{cart c"), vec!["cart count"], "a space narrows");
        assert!(labels("{cart count}").is_empty(), "closed by its brace");
        assert!(labels("\"{\" + 1").is_empty(), "a brace inside quotes");
        assert_eq!(labels("\"a}\" + {sig"), vec!["signedIn"], "after a quote");
        assert_eq!(
            labels("{Cart items.Line."),
            vec!["sum", "min", "max", "avg"]
        );
        // A pick writes the whole name in braces, or the next step after `{`.
        let pick = |t: &str, label: &str| {
            let row = rows(t).into_iter().find(|r| r.label == label).unwrap();
            picked(t, &row)
        };
        assert_eq!(pick("1 + {ca", "cart count"), "1 + {cart count}");
        assert_eq!(pick("1 + {Ca", "Cart items"), "1 + {Cart items.");
        assert_eq!(pick("1 + @ca", "cart count"), "1 + {cart count}", "@ still");
        assert_eq!(pick("@Ca", "Cart items"), "@Cart items.");
    }

    /// `@` lists the tables after the variables; a table's dot lists its count,
    /// its rows and its columns, a column's dot its totals, and a pick of the
    /// last step writes the whole read, which stores by ids.
    /// flip: `at_rows` leaving out the tables → fails at "tables after".
    #[test]
    fn an_at_lists_tables_and_then_what_they_hold() {
        use mastik_core::chart::{Column, ColumnId, Table};
        use mastik_core::list::DataSet;
        let ctx = egui::Context::default();
        let mut f = fixture(&ctx);
        let id = f.app.session.ids.mint_data_set();
        let set = DataSet {
            id,
            name: "Cart items".into(),
            table: Table {
                columns: vec![
                    Column {
                        id: ColumnId(0),
                        name: "Name".into(),
                    },
                    Column {
                        id: ColumnId(1),
                        name: "Line".into(),
                    },
                ],
                rows: vec![vec!["Shirt".into(), "42".into()]],
            },
            source: None,
            pictures: Vec::new(),
        };
        assert!(
            f.app
                .session
                .commit(Transaction(vec![Operation::SetDataSet {
                    id,
                    set: Some(Box::new(set)),
                }]))
        );
        let doc = &f.app.session.doc;
        let rows = |t: &str| at_rows(doc, t, Reads::Tables(None));
        let labels = |t: &str| -> Vec<String> { rows(t).into_iter().map(|r| r.label).collect() };
        assert_eq!(
            labels("@"),
            vec!["Cart items"],
            "tables after the variables"
        );
        assert_eq!(
            rows("@Cart i")[0].insert,
            "Cart items.",
            "a space inside a table's name"
        );
        assert!(
            at_rows(doc, "@", Reads::Vars).is_empty(),
            "a list filter reads no table"
        );
        assert_eq!(
            labels("@Cart items."),
            vec!["rows", "opened", "first", "last", "Name", "Line"]
        );
        let line = rows("@Cart items.Li").pop().unwrap();
        assert_eq!(
            (line.insert.as_str(), line.done),
            ("Cart items.Line.", false)
        );
        assert_eq!(
            labels("@Cart items.Line."),
            vec!["sum", "min", "max", "avg"]
        );
        let cell = rows("@Cart items.first.Na").pop().unwrap();
        assert_eq!(
            (cell.insert.as_str(), cell.done),
            ("Cart items.first.Name", true)
        );
        let typed = format!("{{{}}} + 1", rows("@Cart items.Line.s")[0].insert);
        let e = expr_stored(doc, &typed, Some(VarKind::Number), Reads::Tables(None)).unwrap();
        assert_eq!(expr_shown(doc, &e), "{Cart items.Line.sum} + 1");
        assert!(
            expr_stored(doc, "{row.Name}", None, Reads::Tables(None)).is_err(),
            "row outside a row action"
        );
        assert!(expr_stored(doc, "{row.Name}", None, Reads::Tables(Some(id))).is_ok());
        assert!(labels("@row.").is_empty() && !rows("@").iter().any(|r| r.label == "row"));
        let in_row = at_rows(doc, "@", Reads::Tables(Some(id)));
        assert_eq!(
            in_row[0].label, "row",
            "the acted-on row inside a row action"
        );
    }

    /// A new *Set variable* on a text variable reads before anything is typed,
    /// so it never refuses the export; a new condition starts at the first
    /// boolean variable. flip: `keeps` answering an empty expression → fails at
    /// "reads".
    #[test]
    fn new_set_variable_and_condition_read_as_added() {
        use mastik_core::variable::{Cell, CollectionId, Mode, Variable};
        let ctx = egui::Context::default();
        let mut f = fixture(&ctx);
        let mode = f.app.session.ids.mint_mode();
        let [text, flag] = [(); 2].map(|_| f.app.session.ids.mint_var());
        let var = |id, name: &str, kind, cell| Operation::AddVariable {
            var: Variable {
                id,
                name: name.into(),
                kind,
                cells: [(mode, cell)].into(),
                scope: None,
                origin: None,
                forked: None,
                description: String::new(),
            },
        };
        assert!(f.app.session.commit(Transaction(vec![
            Operation::AddMode {
                collection: CollectionId::FIRST,
                mode: Mode {
                    id: mode,
                    name: "Default".into(),
                },
                index: 0,
                cells: vec![],
            },
            var(text, "greeting", VarKind::String, Cell::String("Hi".into())),
            var(flag, "signedIn", VarKind::Boolean, Cell::Boolean(false)),
        ])));
        let doc = &f.app.session.doc;
        let Some(Action::SetVariable {
            change: VarChange::Set { to },
            ..
        }) = new_action(doc, f.add, ActionKind::SetVariable)
        else {
            panic!("a Set on the text variable");
        };
        assert!(to.check().is_ok(), "reads: {to:?}");
        let Some(Action::Conditional { branches, .. }) =
            new_action(doc, f.add, ActionKind::Conditional)
        else {
            panic!("a conditional");
        };
        assert_eq!(branches[0].when.vars(), vec![flag], "the first flag");
    }

    /// A frame's own *After delay* going back to a frame on its left leaves from
    /// its name tag and arches over both frames; a link going forward does not.
    /// flip: `goes_back` answering `None` → fails at "crests above the frames";
    /// `leaves_from_tag` false → fails at "from the name tag".
    #[test]
    fn a_link_going_back_arches_over_the_frames() {
        let ctx = egui::Context::default();
        let mut f = fixture(&ctx);
        let (home, cart) = (f.home, f.cart);
        assert!(
            f.app
                .session
                .commit(Transaction(vec![Operation::SetReactions {
                    id: cart,
                    reactions: vec![Keyed::new(
                        item(20),
                        Reaction {
                            trigger: Trigger::After { ms: 1000 },
                            actions: vec![keyed(
                                21,
                                Action::Navigate {
                                    to: home,
                                    transition: Transition::default(),
                                    keep_scroll: false,
                                },
                            )],
                        },
                    )],
                }]))
        );
        f.app.session.prototype_mode = true;
        // Panned down, so the frames' tops and the cart's name tag are on screen.
        f.app
            .session
            .camera
            .pan_pixels(mastik_core::kurbo::Vec2::new(0.0, 600.0));
        let rect = egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(1600.0, 1600.0));
        let mut tag = None;
        let _ = ctx.run_ui(Default::default(), |ui| {
            let painter = ui.painter().clone();
            f.app.draw_prototype_links(ui, &painter, rect, 1.0);
            tag = f.app.frame_label(ui, cart, rect, 1.0).map(|(r, _)| r);
        });
        let drawn = f.app.proto.links_drawn.borrow();
        let pts = &drawn
            .iter()
            .find(|(at, _)| at.reaction == item(20))
            .expect("drawn")
            .1;
        let top = f
            .app
            .selection_quad(home, rect, 1.0)
            .expect("home on screen")[0]
            .y;
        let crest = pts.iter().map(|p| p.y).fold(f32::INFINITY, f32::min);
        assert!(
            crest <= top - BACK_RISE + 1.0,
            "crests above the frames: {crest} vs {top}"
        );
        let tag = tag.unwrap_or_else(|| {
            panic!(
                "the cart's name tag: {:?}",
                f.app.selection_quad(cart, rect, 1.0)
            )
        });
        assert!(
            pts[0].distance(egui::pos2(tag.right() + 4.0, tag.center().y)) < 1.0,
            "from the name tag"
        );
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

    /// An overlay's shade is a colour slot: a picker drag is held and read back
    /// before it commits, and a write lands on that action alone as one step.
    /// flip: `preview_backdrop` not called from `preview_slot` → fails at "held".
    #[test]
    fn an_overlay_shade_is_a_colour_slot_the_picker_drags() {
        let ctx = egui::Context::default();
        let mut f = fixture(&ctx);
        let overlay = f.cart;
        let black = mastik_core::peniko::Color::from_rgba8(0, 0, 0, 102);
        let at = one_action(
            &mut f,
            Action::OpenOverlay {
                overlay,
                settings: OverlaySettings {
                    backdrop: Some(black),
                    ..OverlaySettings::default()
                },
                transition: Transition::default(),
            },
        );
        let slot = PaintSlot::Backdrop(at.action);
        let solid = |b: Option<mastik_core::Brush>| match b {
            Some(mastik_core::Brush::Solid(c)) => Some(c.to_rgba8().to_u8_array()),
            _ => None,
        };
        assert_eq!(solid(f.app.slot_brush(f.add, slot)), Some([0, 0, 0, 102]));
        let red = mastik_core::peniko::Color::from_rgba8(200, 0, 0, 51);
        assert!(
            f.app
                .preview_slot(f.add, slot, mastik_core::Brush::Solid(red))
        );
        assert_eq!(
            solid(f.app.slot_brush(f.add, slot)),
            Some([200, 0, 0, 51]),
            "held"
        );
        assert_eq!(
            backdrop_of(&f.app.session.doc, f.add, at.action),
            Some(black),
            "not yet saved"
        );
        f.app
            .write_slot(f.add, slot, mastik_core::Brush::Solid(red));
        assert!(f.app.in_flight.backdrop_paint.is_none(), "the buffer ends");
        assert_eq!(
            backdrop_of(&f.app.session.doc, f.add, at.action).map(|c| c.to_rgba8().to_u8_array()),
            Some([200, 0, 0, 51])
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

    /// A snap point's ghost sits at its offset from the layer's box, keeps to
    /// the layer's axis when dragged, and its drop moves the point.
    ///
    /// flip: `moved_proto_mark` not holding a ghost to its layer's axis → fails
    /// at "held to the axis".
    #[test]
    fn a_snap_point_is_placed_by_its_ghost() {
        let ctx = egui::Context::default();
        let mut f = fixture(&ctx);
        let point = f.app.session.ids.mint_item();
        let play = LayerPlay {
            follow: Some(Follow {
                axis: ScrollAxis::Vertical,
                points: vec![Keyed::new(
                    point,
                    DragPoint {
                        name: "Closed".into(),
                        x: 0.0,
                        y: 100.0,
                    },
                )],
                ..Follow::default()
            }),
            ..LayerPlay::default()
        };
        f.app
            .commit_edit(Transaction(vec![Operation::SetPlay { id: f.add, play }]));
        f.app.session.prototype_mode = true;
        f.app.session.selection.set(vec![f.add]);
        let ghosts = f.app.snap_ghosts();
        assert_eq!(ghosts.len(), 1);
        let r = ghosts[0].2;
        assert_eq!(
            (r.x0, r.y0, r.width(), r.height()),
            (20.0, 120.0, 100.0, 40.0)
        );
        let mark = ghosts[0].0;
        let grab = mastik_core::kurbo::Point::new(50.0, 130.0);
        let (to, _) = f.app.moved_proto_mark(
            mark,
            grab,
            Vec2::new(0.0, 100.0),
            grab + Vec2::new(30.0, 57.0),
            false,
            false,
        );
        assert_eq!(to, Vec2::new(0.0, 157.0), "held to the axis");
        f.app.drop_proto_mark(mark, to);
        let n = f.app.session.doc.get(f.add).unwrap();
        let p = &n.play().follow.as_ref().unwrap().points[0].value;
        assert_eq!((p.x, p.y), (0.0, 157.0));
    }

    /// A new snap point is the first free *Point n*, n steps along the layer's
    /// axis: across for a horizontal layer, down otherwise.
    /// flip: `new_drag_point` without its `Horizontal` arm → fails at "across".
    #[test]
    fn a_new_snap_point_steps_along_the_axis() {
        let follow = |axis, names: &[&str]| Follow {
            axis,
            points: names
                .iter()
                .enumerate()
                .map(|(i, n)| {
                    Keyed::new(
                        item(i as u32 + 1),
                        DragPoint {
                            name: (*n).into(),
                            x: 0.0,
                            y: 0.0,
                        },
                    )
                })
                .collect(),
            ..Follow::default()
        };
        let at = |p: DragPoint| (p.name, p.x, p.y);
        assert_eq!(
            at(new_drag_point(&follow(ScrollAxis::Horizontal, &[]))),
            ("Point 1".into(), 100.0, 0.0),
            "across"
        );
        assert_eq!(
            at(new_drag_point(&follow(ScrollAxis::Vertical, &[]))),
            ("Point 1".into(), 0.0, 100.0),
            "down"
        );
        assert_eq!(
            at(new_drag_point(&follow(ScrollAxis::Both, &["Point 1"]))),
            ("Point 2".into(), 0.0, 200.0),
            "the first free number, a step further"
        );
    }

    /// The drawn point's name field reads *Start* until renamed, keeps what is
    /// typed from frame to frame, gives it up on `Escape`, and commits it on
    /// `Enter`; typing *Start* back clears the rename.
    /// flip: `point_name_field` reading `resp.lost_focus()` with no `ui::Draft`
    /// → fails at "kept across frames"; taking the draft on any defocus, `Escape`
    /// included → fails at "Escape gives it up".
    #[test]
    fn a_snap_point_name_is_a_draft() {
        let ctx = egui::Context::default();
        crate::theme::install(&ctx);
        let mut f = fixture(&ctx);
        let play = LayerPlay {
            follow: Some(Follow::default()),
            ..LayerPlay::default()
        };
        f.app
            .commit_edit(Transaction(vec![Operation::SetPlay { id: f.add, play }]));
        f.app.session.prototype_mode = true;
        f.app.session.selection.set_one(f.add);
        let key = |key, modifiers| egui::Event::Key {
            key,
            physical_key: None,
            pressed: true,
            repeat: false,
            modifiers,
        };
        let frame = |app: &mut MastikApp, events: Vec<egui::Event>| {
            let input = egui::RawInput {
                screen_rect: Some(egui::Rect::from_min_size(
                    egui::Pos2::ZERO,
                    egui::vec2(1400.0, 2400.0),
                )),
                events,
                ..Default::default()
            };
            ctx.run_ui(input, |ui| {
                ui.set_max_width(280.0);
                app.inspector_ui(ui)
            })
        };
        let field_at = |out: &egui::FullOutput, text: &str| {
            out.shapes.iter().find_map(|s| match &s.shape {
                egui::Shape::Text(t) if t.galley.job.text == text => {
                    Some(t.galley.rect.translate(t.pos.to_vec2()).center())
                }
                _ => None,
            })
        };
        let drawn = |app: &MastikApp| {
            let n = app.session.doc.get(f.add).unwrap();
            n.play().follow.as_ref().unwrap().drawn.clone()
        };
        // Click into the field, select its text, and type over it.
        let type_over = |app: &mut MastikApp, at: egui::Pos2| {
            let press = |pressed| egui::Event::PointerButton {
                pos: at,
                button: egui::PointerButton::Primary,
                pressed,
                modifiers: Default::default(),
            };
            frame(app, vec![egui::Event::PointerMoved(at), press(true)]);
            frame(app, vec![press(false)]);
            frame(
                app,
                vec![
                    key(egui::Key::A, egui::Modifiers::COMMAND),
                    egui::Event::Text("Rest".into()),
                ],
            );
        };
        let mut out = frame(&mut f.app, vec![]);
        for _ in 0..3 {
            out = frame(&mut f.app, vec![]);
        }
        let at = field_at(&out, "Start").expect("the drawn point reads Start");
        type_over(&mut f.app, at);
        let out = frame(&mut f.app, vec![]);
        assert!(field_at(&out, "Rest").is_some(), "kept across frames");
        frame(
            &mut f.app,
            vec![key(egui::Key::Escape, egui::Modifiers::NONE)],
        );
        let out = frame(&mut f.app, vec![]);
        assert_eq!(drawn(&f.app), None, "Escape gives it up");
        assert!(field_at(&out, "Start").is_some(), "the old name is back");
        type_over(&mut f.app, at);
        frame(
            &mut f.app,
            vec![key(egui::Key::Enter, egui::Modifiers::NONE)],
        );
        assert_eq!(drawn(&f.app).as_deref(), Some("Rest"), "Enter commits");
        for _ in 0..3 {
            frame(&mut f.app, vec![]);
        }
        let out = frame(&mut f.app, vec![]);
        let at = field_at(&out, "Rest").expect("the new name shows");
        type_over(&mut f.app, at);
        frame(&mut f.app, vec![]);
        // Typed back over the select-all, the default name clears the rename.
        frame(
            &mut f.app,
            vec![
                key(egui::Key::A, egui::Modifiers::COMMAND),
                egui::Event::Text(DRAWN_POINT.into()),
                key(egui::Key::Enter, egui::Modifiers::NONE),
            ],
        );
        assert_eq!(drawn(&f.app), None, "the default name clears it");
    }

    /// Prototype mode is remembered by the library for the document, as the
    /// camera is, and comes back when it reopens; the document stays clean.
    ///
    /// flip: `set_prototype_mode` not calling `remember_prototype_mode` → fails
    /// at "remembered".
    #[test]
    fn prototype_mode_is_remembered_for_the_document() {
        let ctx = egui::Context::default();
        let mut f = fixture(&ctx);
        let mut meta = f.app.session.doc.meta().clone();
        meta.id = Some("proto-doc".into());
        f.app.session.doc.set_meta(meta);
        f.app.set_prototype_mode(true);
        assert!(!f.app.session.is_dirty(), "a view change, not an edit");
        assert!(
            f.app.library.local.prototype_mode.contains("proto-doc"),
            "remembered"
        );
        f.app.session.prototype_mode = false;
        f.app.reopen_prototype_mode();
        assert!(f.app.prototype_mode(), "reopens in it");
        f.app.set_prototype_mode(false);
        assert!(
            f.app.library.local.prototype_mode.is_empty(),
            "forgotten when left"
        );
    }

    /// The connectors' and marks' pointer is no pointer under a card lying on
    /// the canvas, such as the player over it.
    ///
    /// flip: `canvas_pointer` without its `layer_id_at` test → fails at "under
    /// the card".
    #[test]
    fn the_canvas_pointer_stops_at_a_card_over_it() {
        let ctx = egui::Context::default();
        let screen = egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(800.0, 600.0));
        let card = egui::Rect::from_min_size(egui::pos2(200.0, 200.0), egui::vec2(200.0, 200.0));
        let at = |p: egui::Pos2| {
            let mut got = None;
            for _ in 0..3 {
                let _ = ctx.run_ui(
                    egui::RawInput {
                        screen_rect: Some(screen),
                        events: vec![egui::Event::PointerMoved(p)],
                        ..Default::default()
                    },
                    |ui| {
                        egui::Area::new(egui::Id::new("card"))
                            .order(egui::Order::Foreground)
                            .fixed_pos(card.min)
                            .show(ui.ctx(), |ui| {
                                ui.allocate_exact_size(card.size(), egui::Sense::hover());
                            });
                        got = canvas_pointer(ui);
                    },
                );
            }
            got
        };
        let off = egui::pos2(100.0, 100.0);
        assert_eq!(at(off), Some(off), "off the card");
        assert_eq!(at(card.center()), None, "under the card");
    }

    /// *Paste interactions* appends every copied interaction to each selected
    /// layer with fresh item ids, as one undo step, and only in the tab it was
    /// copied in.
    ///
    /// flip: `paste_interactions` keeping the copied ids → fails at "fresh ids".
    #[test]
    fn copied_interactions_paste_onto_each_selected_layer() {
        let ctx = egui::Context::default();
        let mut f = fixture(&ctx);
        let on_click = |n| {
            Keyed::new(
                item(n),
                Reaction {
                    trigger: Trigger::Click,
                    actions: vec![keyed(n + 10, Action::Back)],
                },
            )
        };
        f.app.write_reactions(
            f.add,
            vec![on_click(1), on_click(2)],
            "Add interaction",
            icon::INTERACTION,
        );
        f.app.session.selection.set_one(f.home);
        assert!(f.app.interactions_source().is_none(), "Home has none");
        f.app.session.selection.set_one(f.add);
        f.app.copy_interactions();
        assert!(f.app.interactions_copied());
        f.app.session.selection.set_one(f.cart);
        f.app.paste_interactions();
        let pasted = f.app.session.doc.get(f.cart).unwrap().reactions().to_vec();
        assert_eq!(pasted.len(), 2, "both interactions");
        let own = f.app.session.doc.get(f.add).unwrap().reactions().to_vec();
        assert!(
            pasted.iter().all(|p| own
                .iter()
                .all(|o| o.id != p.id && o.value.actions[0].id != p.value.actions[0].id)),
            "fresh ids"
        );
        f.app.session.undo();
        assert!(
            f.app
                .session
                .doc
                .get(f.cart)
                .unwrap()
                .reactions()
                .is_empty(),
            "one step"
        );
        f.app.proto.copied.as_mut().unwrap().0 = None;
        assert!(
            !f.app.interactions_copied(),
            "another tab's copy is not offered"
        );
    }

    /// A scroll trigger's line moves along its frame's scroll and snaps to a
    /// child's top edge unless Alt is held; its drop changes the trigger. A
    /// snapping frame's rest edges are its children's leading edges.
    ///
    /// flip: `moved_proto_mark` skipping the children's edges → fails at
    /// "snapped to the layer's top".
    #[test]
    fn a_scroll_triggers_point_is_dragged_on_the_canvas() {
        let ctx = egui::Context::default();
        let mut f = fixture(&ctx);
        f.app.canvas_px = (1200, 900);
        f.app.session.camera.zoom = 1.0;
        let play = LayerPlay {
            scroll: ScrollAxis::Vertical,
            snap: Snap::Children,
            ..LayerPlay::default()
        };
        f.app
            .commit_edit(Transaction(vec![Operation::SetPlay { id: f.home, play }]));
        let reaction = f.app.session.ids.mint_item();
        let reactions = vec![Keyed::new(
            reaction,
            Reaction {
                trigger: Trigger::ScrollPast {
                    frame: f.home,
                    at: 240.0,
                    going: Going::Forward,
                },
                actions: Vec::new(),
            },
        )];
        f.app
            .write_reactions(f.add, reactions, "Add interaction", icon::INTERACTION);
        f.app.session.prototype_mode = true;
        f.app.session.selection.set(vec![f.add]);
        let lines = f.app.past_lines();
        assert_eq!(lines.len(), 1);
        let (mark, frame, at, side) = lines[0];
        assert_eq!((frame.y0, at, side), (0.0, 240.0, false));
        let grab = mastik_core::kurbo::Point::new(50.0, 240.0);
        let to = |alt| {
            let w = grab + Vec2::new(5.0, -218.0);
            f.app
                .moved_proto_mark(mark, grab, Vec2::new(240.0, 0.0), w, false, alt)
                .0
        };
        // *Add to cart* sits at y 20 in Home.
        assert_eq!(
            to(false),
            Vec2::new(20.0, 0.0),
            "snapped to the layer's top"
        );
        assert_eq!(to(true), Vec2::new(22.0, 0.0), "Alt holds it off");
        f.app.drop_proto_mark(mark, Vec2::new(20.0, 0.0));
        let n = f.app.session.doc.get(f.add).unwrap();
        assert!(matches!(
            n.reactions()[0].value.trigger,
            Trigger::ScrollPast { at, .. } if at == 20.0
        ));
        f.app.session.selection.set(vec![f.home]);
        let inst_top = f.app.world_bounds(f.inst).unwrap().y0;
        let mut edges: Vec<f64> = f.app.snap_edges().iter().map(|[a, _]| a.y).collect();
        edges.sort_by(f64::total_cmp);
        assert_eq!(edges, [20.0, inst_top], "each child's top");
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

    /// An action carried to the bottom of its list lands last on the release, in
    /// one step; a cancelled carry lands nothing.
    /// flip: `action_rows` leaving `to` at `from` (no `forward_landing`) → fails
    /// at "the first action is now last".
    #[test]
    fn an_action_dragged_to_the_bottom_lands_last() {
        let ctx = egui::Context::default();
        crate::theme::install(&ctx);
        let mut f = fixture(&ctx);
        let at = one_action(&mut f, Action::Back);
        let three = vec![Keyed::new(
            item(10),
            Reaction {
                trigger: Trigger::Click,
                actions: vec![
                    keyed(1, Action::Back),
                    keyed(2, Action::Back),
                    keyed(3, Action::Back),
                ],
            },
        )];
        assert!(
            f.app
                .session
                .commit(Transaction(vec![Operation::SetReactions {
                    id: f.add,
                    reactions: three,
                }]))
        );
        f.app.session.selection.set_one(f.add);
        let order = |app: &MastikApp| -> Vec<ItemId> {
            app.session.doc.get(f.add).unwrap().reactions()[0]
                .actions
                .iter()
                .map(|k| k.id)
                .collect()
        };
        let frame = |app: &mut MastikApp, events: Vec<egui::Event>| {
            let input = egui::RawInput {
                screen_rect: Some(egui::Rect::from_min_size(
                    egui::Pos2::ZERO,
                    egui::vec2(1400.0, 2400.0),
                )),
                events,
                ..Default::default()
            };
            let _ = ctx.run_ui(input, |ui| {
                ui.set_max_width(280.0);
                app.inspector_ui(ui)
            });
        };
        let low = egui::pos2(100.0, 2300.0);
        let button = |pressed| egui::Event::PointerButton {
            pos: low,
            button: egui::PointerButton::Primary,
            pressed,
            modifiers: Default::default(),
        };
        let carry = |app: &mut MastikApp| {
            for _ in 0..3 {
                frame(app, vec![]);
            }
            app.in_flight.action_drag = Some(ActionDrag {
                at,
                from: 0,
                to: 0,
                slots: paint::Slots::default(),
            });
            frame(app, vec![egui::Event::PointerMoved(low), button(true)]);
            frame(app, vec![]);
        };
        // Cancelled: the release lands nothing.
        carry(&mut f.app);
        assert!(f.app.cancel_gesture(&ctx), "a carried action is a gesture");
        frame(&mut f.app, vec![button(false)]);
        assert_eq!(order(&f.app), vec![item(1), item(2), item(3)], "put back");
        // Let go: the first action lands last, in one step.
        carry(&mut f.app);
        let depth = f.app.session.history.undo_depth();
        frame(&mut f.app, vec![button(false)]);
        assert!(f.app.in_flight.action_drag.is_none(), "the release ends it");
        assert_eq!(
            order(&f.app),
            vec![item(2), item(3), item(1)],
            "the first action is now last"
        );
        assert_eq!(f.app.session.history.undo_depth(), depth + 1, "one step");
    }

    /// A click on a connector opens its action's popover and the popover stays
    /// open, the action at the top of its reaction or inside a branch alike: the
    /// popover first draws on the click's own frame.
    /// flip: `action_popover` without its `opened_on_canvas` excuse → fails at
    /// "stays open".
    #[test]
    fn a_connectors_click_opens_a_popover_that_stays() {
        let ctx = egui::Context::default();
        crate::theme::install(&ctx);
        for nested_case in [false, true] {
            let mut f = fixture(&ctx);
            let to = f.cart;
            let nav = Action::Navigate {
                to,
                transition: Transition::default(),
                keep_scroll: false,
            };
            let action = match nested_case {
                false => nav,
                true => Action::Conditional {
                    branches: vec![Branch {
                        when: Expr("true".into()),
                        actions: vec![keyed(3, nav)],
                    }],
                    otherwise: vec![],
                },
            };
            let mut at = one_action(&mut f, action);
            if nested_case {
                at.action = item(3);
            }
            f.app.session.selection.set(vec![f.cart]);
            let away = egui::pos2(1000.0, 500.0);
            let ev = |pressed| egui::Event::PointerButton {
                pos: away,
                button: egui::PointerButton::Primary,
                pressed,
                modifiers: Default::default(),
            };
            for _ in 0..3 {
                draw(&mut f.app, &ctx);
            }
            let frame = |app: &mut MastikApp, events: Vec<egui::Event>| {
                let input = egui::RawInput {
                    screen_rect: Some(egui::Rect::from_min_size(
                        egui::Pos2::ZERO,
                        egui::vec2(1400.0, 2400.0),
                    )),
                    events,
                    ..Default::default()
                };
                let _ = ctx.run_ui(input, |ui| {
                    ui.set_max_width(280.0);
                    app.inspector_ui(ui)
                });
            };
            frame(&mut f.app, vec![egui::Event::PointerMoved(away), ev(true)]);
            // The canvas hears the click before the inspector draws.
            f.app.open_link(at);
            frame(&mut f.app, vec![ev(false)]);
            let mut seen = vec![f.app.proto.open];
            for _ in 0..3 {
                frame(&mut f.app, vec![]);
                seen.push(f.app.proto.open);
            }
            assert_eq!(seen, vec![Some(at); 4], "stays open, nested: {nested_case}");
            // A later click away still closes it.
            let far = egui::pos2(1350.0, 2300.0);
            let at_far = |pressed| egui::Event::PointerButton {
                pos: far,
                button: egui::PointerButton::Primary,
                pressed,
                modifiers: Default::default(),
            };
            frame(
                &mut f.app,
                vec![egui::Event::PointerMoved(far), at_far(true)],
            );
            frame(&mut f.app, vec![at_far(false)]);
            assert_eq!(f.app.proto.open, None, "a click away closes it");
        }
    }

    /// An action's verb is cut before its timing note rather than run under it.
    /// flip: `action_row` laying the verb out whole before the timing → fails at
    /// "ends before the timing".
    #[test]
    fn an_actions_verb_stops_short_of_its_timing() {
        let ctx = egui::Context::default();
        crate::theme::install(&ctx);
        let mut f = fixture(&ctx);
        let overlay = f.cart;
        one_action(
            &mut f,
            Action::OpenOverlay {
                overlay,
                settings: OverlaySettings::default(),
                transition: Transition::default(),
            },
        );
        f.app.session.selection.set_one(f.add);
        let mut out = draw(&mut f.app, &ctx);
        for _ in 0..2 {
            out = draw(&mut f.app, &ctx);
        }
        let ink = |starts: &str| {
            out.shapes.iter().find_map(|s| match &s.shape {
                egui::Shape::Text(t) if t.galley.job.text.starts_with(starts) => {
                    Some(t.galley.rect.translate(t.pos.to_vec2()))
                }
                _ => None,
            })
        };
        let verb = ink("Open overlay").expect("the verb");
        let timing = ink("Dissolve").expect("the timing");
        assert!(
            verb.right() <= timing.left(),
            "ends before the timing: {verb:?} {timing:?}"
        );
    }

    /// Prototype mode shows the Prototype cards in place of the Design ones, and
    /// the page's flows with nothing selected.
    #[test]
    fn prototype_mode_shows_its_own_cards() {
        let ctx = egui::Context::default();
        let mut f = fixture(&ctx);
        f.app.session.selection.set_one(f.home);
        draw(&mut f.app, &ctx);
        assert!(f.app.cards_drawn.contains(&"Fill"), "fixture: design mode");
        assert!(!f.app.cards_drawn.contains(&"Interactions"));
        f.app
            .set_view_switch(crate::input::ViewSwitch::PrototypeMode, true);
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

    /// An action row's verb, target and timing, set in three sizes, share one
    /// baseline.
    ///
    /// flip: `action_row` adding the verb as a plain label → fails at "the
    /// verb".
    #[test]
    fn an_action_rows_words_share_a_baseline() {
        let ctx = egui::Context::default();
        crate::theme::install(&ctx);
        let mut f = fixture(&ctx);
        let nav = navigate_to(&f.app.session.doc, f.add, f.cart);
        let on_click = Reaction {
            trigger: Trigger::Click,
            actions: vec![keyed(2, nav)],
        };
        f.app.write_reactions(
            f.add,
            vec![Keyed::new(item(1), on_click)],
            "Add interaction",
            icon::INTERACTION,
        );
        f.app.session.selection.set_one(f.add);
        f.app.session.prototype_mode = true;
        let mut out = draw(&mut f.app, &ctx);
        for _ in 0..3 {
            out = draw(&mut f.app, &ctx);
        }
        let baseline = |word: &str| {
            out.shapes.iter().find_map(|c| match &c.shape {
                egui::Shape::Text(t) if t.galley.job.text == word => {
                    Some(t.pos.y + ui::baseline_in(&t.galley))
                }
                _ => None,
            })
        };
        let target = baseline("Cart").expect("the target");
        let verb = baseline("Navigate to").expect("the verb");
        let timing = out
            .shapes
            .iter()
            .find_map(|c| match &c.shape {
                egui::Shape::Text(t) if t.galley.job.text.starts_with("Dissolve") => {
                    Some(t.pos.y + ui::baseline_in(&t.galley))
                }
                _ => None,
            })
            .expect("the timing");
        assert!((verb - target).abs() < 0.01, "the verb: {verb} vs {target}");
        assert!(
            (timing - target).abs() < 0.01,
            "the timing: {timing} vs {target}"
        );
    }

    /// A folded section's title sits midway between its hairline and the next,
    /// by its ink: the middle of its capitals, which stand `CAP` of the type
    /// size above the baseline.
    ///
    /// flip: `FOLD_ROW_TRIM` back at 2 → fails at "centred between its rules".
    #[test]
    fn a_folded_title_sits_midway_between_its_rules() {
        const CAP: f32 = 0.7;
        let ctx = egui::Context::default();
        crate::theme::install(&ctx);
        let mut f = fixture(&ctx);
        f.app.session.selection.set_one(f.add);
        f.app.session.prototype_mode = true;
        for k in [
            "Prototype pin",
            "Prototype scroll effect",
            "Prototype dragging",
        ] {
            f.app.collapsed_panels.insert(k);
        }
        let mut out = draw(&mut f.app, &ctx);
        for _ in 0..3 {
            out = draw(&mut f.app, &ctx);
        }
        let mut rules: Vec<f32> = out
            .shapes
            .iter()
            .filter_map(|c| match &c.shape {
                egui::Shape::LineSegment { points, stroke }
                    if stroke.color == theme::color::divider() && points[0].y == points[1].y =>
                {
                    Some(points[0].y)
                }
                _ => None,
            })
            .collect();
        rules.sort_by(f32::total_cmp);
        let ink_mid = out
            .shapes
            .iter()
            .find_map(|c| match &c.shape {
                egui::Shape::Text(t) if t.galley.job.text == "With the scroll" => {
                    let base = t.pos.y + ui::baseline_in(&t.galley);
                    Some(base - CAP * ui::SMALL_PT / 2.0)
                }
                _ => None,
            })
            .expect("a folded section between two");
        let above = rules
            .iter()
            .rev()
            .find(|y| **y < ink_mid)
            .expect("its rule");
        let below = rules.iter().find(|y| **y > ink_mid).expect("the next rule");
        let mid = (above + below) / 2.0;
        assert!(
            (ink_mid - mid).abs() <= 0.75,
            "centred between its rules: ink {ink_mid} vs {mid}"
        );
    }

    /// The Prototype card's first section takes no rule under the card's head;
    /// a later one is parted from the one above by a hairline over its title.
    ///
    /// flip: `fold_rule` ignoring where `section_head_full` noted the body
    /// starts → fails at "none under the head".
    #[test]
    fn only_a_later_section_is_ruled() {
        let ctx = egui::Context::default();
        let mut f = fixture(&ctx);
        f.app.session.selection.set_one(f.add);
        f.app.session.prototype_mode = true;
        let mut out = draw(&mut f.app, &ctx);
        for _ in 0..3 {
            out = draw(&mut f.app, &ctx);
        }
        let shapes: Vec<_> = out.shapes.iter().map(|c| &c.shape).collect();
        let title_y = |word: &str| {
            shapes.iter().find_map(|s| match s {
                egui::Shape::Text(t) if t.galley.job.text == word => Some(t.pos.y),
                _ => None,
            })
        };
        // A divider hairline within a title row's height above `y`.
        let ruled = |y: f32| {
            shapes.iter().any(|s| match s {
                egui::Shape::LineSegment { points, stroke } => {
                    stroke.color == theme::color::divider()
                        && points[0].y == points[1].y
                        && (y - points[0].y) > 0.0
                        && (y - points[0].y) < ui::FOLD_HEAD_H
                }
                _ => false,
            })
        };
        let first = title_y("In a scrolling frame").expect("a child's first section");
        let later = title_y("Dragging").expect("a later section");
        assert!(!ruled(first), "none under the head");
        assert!(ruled(later), "a rule over a later section");
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
        f.app.session.prototype_mode = true;
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
