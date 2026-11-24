//! The prototype model: what a played document is made of.
//!
//! Everything here is **saved and authored**: it goes through the commit door, is
//! undoable, and is copied with its layers. What exists only while a prototype
//! runs (the current frame, play values, scroll offsets) belongs to the player and
//! never reaches this module.
//!
//! - A layer carries [`Reaction`]s (`Node::reactions`, a keyed list) and its
//!   [`LayerPlay`] settings (`Node::play`).
//! - A page carries its [`Flow`]s (`Page::flows`) and its [`PagePlay`] settings.
//! - The document carries its [`PlayExport`] settings.
//!
//! The core type is a *reaction*, not an *interaction*: `variant::Interaction`
//! already names a variant value's tied browser state.
//!
//! **Targets may dangle.** An action naming a deleted frame, layer, variable or
//! script stays in place, drawn as missing and skipped in play, so undo mends it;
//! neither the door nor the loader refuses one. A target inside a main is stored
//! as the main's id on every instance too (the instance's item equals the
//! main's, so it is no override) and is mapped to the instance's own layer when
//! read, by [`resolve_target`].

pub mod expr;

pub use expr::{Expr, ExprError};

use crate::chart::ColumnId;
use crate::document::Document;
use crate::id::NodeId;
use crate::item::{ItemId, Keyed};
use crate::list::DataSetId;
use crate::script::ScriptId;
use crate::variable::{Cell, ModeId, VarId};
use peniko::Color;
use serde::{Deserialize, Serialize};

/// A layer id in its wire form, `"<actor-hex>:<seq>"`, as every other saved id.
mod wire {
    use crate::id::NodeId;
    use serde::{Deserialize, Deserializer, Serializer};

    pub fn serialize<S: Serializer>(id: &NodeId, s: S) -> Result<S::Ok, S::Error> {
        s.serialize_str(&id.to_wire())
    }

    pub fn deserialize<'de, D: Deserializer<'de>>(d: D) -> Result<NodeId, D::Error> {
        let s = String::deserialize(d)?;
        NodeId::from_wire(&s).ok_or_else(|| serde::de::Error::custom(format!("bad id {s:?}")))
    }
}

/// An optional layer id in its wire form; skipped when `None`.
mod wire_opt {
    use crate::id::NodeId;
    use serde::{Deserializer, Serializer};

    pub fn serialize<S: Serializer>(id: &Option<NodeId>, s: S) -> Result<S::Ok, S::Error> {
        match id {
            Some(id) => super::wire::serialize(id, s),
            None => s.serialize_none(),
        }
    }

    pub fn deserialize<'de, D: Deserializer<'de>>(d: D) -> Result<Option<NodeId>, D::Error> {
        super::wire::deserialize(d).map(Some)
    }
}

/// One trigger on a layer and the actions it runs, in order.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Reaction {
    pub trigger: Trigger,
    /// Run top to bottom. Each keeps its own id for the editor's rows; an instance
    /// follows its main's reaction with this list as one value, so a copy that
    /// changed any action holds the whole list as its override.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub actions: Vec<Keyed<Action>>,
}

/// What starts a reaction.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub enum Trigger {
    /// A click, or a tap.
    Click,
    /// The pointer enters the layer; the actions run once.
    HoverEnter,
    /// While the pointer is over the layer; leaving it undoes what the actions did.
    HoverWhile,
    /// The pointer leaves the layer, or the player; the actions run once.
    HoverLeave,
    /// Two clicks in quick succession. On a layer that also has a click, the
    /// click waits until a second can no longer come.
    DoubleClick,
    /// The pointer goes down on the layer.
    Press,
    /// While the pointer is held down on the layer.
    PressWhile,
    /// The pointer held down on the layer for `ms` without dragging; the press
    /// is then spent, so no click follows.
    LongPress { ms: u32 },
    /// A drag starting on the layer; inside a scrolling frame it takes the drag
    /// from the scroll.
    Drag,
    /// A quick drag toward `side`, released. Asked before *Drag* and the
    /// scroll, so a card can take left and right while its list scrolls.
    Swipe { side: Side },
    /// A layer that follows the drag has settled after a release, at `at`.
    Drop {
        #[serde(default, skip_serializing_if = "is_default")]
        at: DropAt,
    },
    /// A scrolling frame's offset crosses `at` going `going`: once per crossing,
    /// a jump across it included.
    ScrollPast {
        #[serde(with = "wire")]
        frame: NodeId,
        at: f64,
        #[serde(default, skip_serializing_if = "is_default")]
        going: Going,
    },
    /// A key, plain or Shift-ed; Ctrl and Alt chords belong to the editor.
    Key {
        /// The key's name as the player reads it, egui's (`"A"`, `"Enter"`,
        /// `"Left"`, `"Space"`).
        key: String,
        #[serde(default, skip_serializing_if = "std::ops::Not::not")]
        shift: bool,
    },
    /// A time after the frame holding the layer opens.
    After { ms: u32 },
    /// The variable's play value changes.
    VariableChanges { var: VarId },
    /// The frame holding the layer opens.
    FrameOpens,
    /// A row action changed the table, whichever layer ran it.
    TableChanges { table: DataSetId },
}

impl Trigger {
    /// Whether a person's act fires it. Only such a trigger may run a script:
    /// under the others a script would run unasked, which the document never does.
    pub fn is_a_persons_act(&self) -> bool {
        !matches!(
            self,
            Trigger::After { .. }
                | Trigger::VariableChanges { .. }
                | Trigger::FrameOpens
                | Trigger::TableChanges { .. }
        )
    }

    /// The table it watches.
    pub fn table(&self) -> Option<DataSetId> {
        match self {
            Trigger::TableChanges { table } => Some(*table),
            _ => None,
        }
    }

    /// The layer the trigger watches, other than its holder: a scroll
    /// trigger's frame.
    pub fn layer_target(&self) -> Option<NodeId> {
        match self {
            Trigger::ScrollPast { frame, .. } => Some(*frame),
            _ => None,
        }
    }

    /// This trigger with the layer it watches passed through `f`.
    pub fn map_layers(&self, f: &mut impl FnMut(NodeId) -> NodeId) -> Trigger {
        match self {
            Trigger::ScrollPast { frame, at, going } => Trigger::ScrollPast {
                frame: f(*frame),
                at: *at,
                going: *going,
            },
            other => other.clone(),
        }
    }
}

/// The side a swipe goes toward.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum Side {
    #[default]
    Left,
    Right,
    Up,
    Down,
}

/// Which way a scroll must cross a scroll trigger's point. On a frame that
/// scrolls sideways, *forward* is rightward.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum Going {
    /// Into the content: down, or right.
    #[default]
    Forward,
    /// Back toward the start: up, or left.
    Back,
    Either,
}

/// One thing a reaction does.
///
/// Externally tagged, as the file's other enums are: an instance follows its
/// main field by field (`propagate::items`), and only a kind written as the
/// object's one key tells that merge the action became another.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub enum Action {
    /// Go to a frame, on any page.
    Navigate {
        #[serde(with = "wire")]
        to: NodeId,
        transition: Transition,
        /// Whether the destination's scrolling frames open where play last left
        /// them in this run, rather than at their top. *Back* to a screen keeps
        /// its offsets when the navigation that left it kept them.
        #[serde(default, skip_serializing_if = "std::ops::Not::not")]
        keep_scroll: bool,
    },
    /// Pop the navigation history; with an overlay open, close the top overlay.
    Back,
    /// Open a frame as an overlay, shown as `settings` say. The settings are the
    /// action's, not the frame's: one frame opened from two places may sit by each.
    OpenOverlay {
        #[serde(with = "wire")]
        overlay: NodeId,
        #[serde(default, skip_serializing_if = "is_default")]
        settings: OverlaySettings,
        transition: Transition,
    },
    /// Close the top overlay.
    CloseOverlay { transition: Transition },
    /// Replace the top overlay with another frame, shown as `settings` say.
    SwapOverlay {
        #[serde(with = "wire")]
        overlay: NodeId,
        #[serde(default, skip_serializing_if = "is_default")]
        settings: OverlaySettings,
        transition: Transition,
    },
    /// Scroll the frame holding `layer` until it is in view.
    ScrollTo {
        #[serde(with = "wire")]
        layer: NodeId,
        transition: Transition,
    },
    /// Switch an instance to another variant of its set; `to` is that variant's
    /// main.
    ChangeState {
        #[serde(with = "wire")]
        instance: NodeId,
        #[serde(with = "wire")]
        to: NodeId,
        transition: Transition,
    },
    /// Change a variable's play value.
    SetVariable { var: VarId, change: VarChange },
    /// Switch the collection `mode` belongs to to `mode`, for the rest of the
    /// run: a mode set in play is play state, never the document's.
    SetMode {
        mode: ModeId,
        #[serde(default, skip_serializing_if = "is_default")]
        scope: ModeScope,
    },
    /// The first branch whose condition holds runs its actions; else `otherwise`.
    Conditional {
        branches: Vec<Branch>,
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        otherwise: Vec<Keyed<Action>>,
    },
    /// Open an `http`/`https` address in the browser.
    OpenUrl { url: String },
    /// Run a document script on the played copy.
    RunScript { script: ScriptId },
    /// Add a row to a table of the played copy: each listed cell what its
    /// expression comes to, written as text, the others empty.
    AddRow {
        table: DataSetId,
        #[serde(default, skip_serializing_if = "is_default")]
        at: RowEnd,
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        cells: Vec<CellWrite>,
    },
    /// Write the listed cells of one row of a table of the played copy, each
    /// worked out from the row as it was before the edit.
    EditRow {
        table: DataSetId,
        row: WhichRow,
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        cells: Vec<CellWrite>,
    },
    /// Delete one row of a table of the played copy.
    DeleteRow { table: DataSetId, row: WhichRow },
}

/// Where *Add row* puts its row.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum RowEnd {
    #[default]
    End,
    Start,
}

/// Which row *Edit row* or *Delete row* acts on. No row number: a run deletes
/// rows as it goes, so a number would name a different row each time.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub enum WhichRow {
    /// The list row the reaction fired in, the row a tap there opens; only
    /// inside a list over the action's table.
    This,
    /// The row an *Opened row* list over the table shows.
    Opened,
    /// The first row whose test holds, `{row.<column>}` reading the row tried.
    /// None holding skips the action.
    FirstWhere { when: Expr },
}

/// One cell a row action writes.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct CellWrite {
    pub column: ColumnId,
    pub to: Expr,
}

impl Action {
    /// The table a row action names.
    pub fn table(&self) -> Option<DataSetId> {
        match self {
            Action::AddRow { table, .. }
            | Action::EditRow { table, .. }
            | Action::DeleteRow { table, .. } => Some(*table),
            _ => None,
        }
    }

    /// The tables this action names, nested actions' and its expressions'
    /// included.
    pub fn tables(&self) -> Vec<DataSetId> {
        let mut out = Vec::new();
        walk(self, &mut |a| {
            out.extend(a.table());
            for e in a.exprs() {
                out.extend(e.refs().iter().filter_map(|r| r.table()));
            }
        });
        out.sort();
        out.dedup();
        out
    }

    /// The expressions this action holds itself, not its nested actions'.
    pub fn exprs(&self) -> Vec<&Expr> {
        fn cells(c: &[CellWrite]) -> Vec<&Expr> {
            c.iter().map(|c| &c.to).collect()
        }
        fn row(r: &WhichRow) -> Vec<&Expr> {
            match r {
                WhichRow::FirstWhere { when } => vec![when],
                _ => vec![],
            }
        }
        match self {
            Action::SetVariable {
                change: VarChange::Set { to },
                ..
            } => vec![to],
            Action::Conditional { branches, .. } => branches.iter().map(|b| &b.when).collect(),
            Action::AddRow { cells: c, .. } => cells(c),
            Action::EditRow {
                row: r, cells: c, ..
            } => [row(r), cells(c)].concat(),
            Action::DeleteRow { row: r, .. } => row(r),
            _ => Vec::new(),
        }
    }

    /// The actions nested in this one, branch by branch, then `otherwise`.
    pub fn nested(&self) -> impl Iterator<Item = &Keyed<Action>> {
        let (branches, otherwise): (&[Branch], &[Keyed<Action>]) = match self {
            Action::Conditional {
                branches,
                otherwise,
            } => (branches, otherwise),
            _ => (&[], &[]),
        };
        branches
            .iter()
            .flat_map(|b| b.actions.iter())
            .chain(otherwise)
    }

    /// The layers this action names, nested actions' included.
    pub fn layer_targets(&self) -> Vec<NodeId> {
        let mut out = Vec::new();
        walk(self, &mut |a| match a {
            Action::Navigate { to, .. } => out.push(*to),
            Action::OpenOverlay { overlay, .. } | Action::SwapOverlay { overlay, .. } => {
                out.push(*overlay)
            }
            Action::ScrollTo { layer, .. } => out.push(*layer),
            Action::ChangeState { instance, to, .. } => out.extend([*instance, *to]),
            _ => {}
        });
        out
    }

    /// This action with every layer it names passed through `f`, nested actions'
    /// included.
    pub fn map_layers(&self, f: &mut impl FnMut(NodeId) -> NodeId) -> Action {
        let keyed = |list: &[Keyed<Action>], f: &mut dyn FnMut(NodeId) -> NodeId| {
            list.iter()
                .map(|k| k.map(|a| a.map_layers(&mut &mut *f)))
                .collect()
        };
        match self {
            Action::Navigate {
                to,
                transition,
                keep_scroll,
            } => Action::Navigate {
                to: f(*to),
                transition: *transition,
                keep_scroll: *keep_scroll,
            },
            Action::OpenOverlay {
                overlay,
                settings,
                transition,
            } => Action::OpenOverlay {
                overlay: f(*overlay),
                settings: *settings,
                transition: *transition,
            },
            Action::SwapOverlay {
                overlay,
                settings,
                transition,
            } => Action::SwapOverlay {
                overlay: f(*overlay),
                settings: *settings,
                transition: *transition,
            },
            Action::ScrollTo { layer, transition } => Action::ScrollTo {
                layer: f(*layer),
                transition: *transition,
            },
            Action::ChangeState {
                instance,
                to,
                transition,
            } => Action::ChangeState {
                instance: f(*instance),
                to: f(*to),
                transition: *transition,
            },
            Action::Conditional {
                branches,
                otherwise,
            } => Action::Conditional {
                branches: branches
                    .iter()
                    .map(|b| Branch {
                        when: b.when.clone(),
                        actions: keyed(&b.actions, f),
                    })
                    .collect(),
                otherwise: keyed(otherwise, f),
            },
            other => other.clone(),
        }
    }

    /// This action with every variable it reads or sets passed through `var`
    /// and every script it runs through `script`, nested actions' included.
    pub fn map_refs(
        &self,
        var: &mut dyn FnMut(VarId) -> VarId,
        script: &mut dyn FnMut(ScriptId) -> ScriptId,
    ) -> Action {
        match self {
            Action::SetVariable { var: v, change } => Action::SetVariable {
                var: var(*v),
                change: match change {
                    VarChange::Set { to } => VarChange::Set {
                        to: to.map_vars(var),
                    },
                    other => other.clone(),
                },
            },
            Action::RunScript { script: s } => Action::RunScript { script: script(*s) },
            Action::AddRow { table, at, cells } => Action::AddRow {
                table: *table,
                at: *at,
                cells: map_cells(cells, var),
            },
            Action::EditRow { table, row, cells } => Action::EditRow {
                table: *table,
                row: map_row(row, var),
                cells: map_cells(cells, var),
            },
            Action::DeleteRow { table, row } => Action::DeleteRow {
                table: *table,
                row: map_row(row, var),
            },
            Action::Conditional {
                branches,
                otherwise,
            } => {
                let branches = branches
                    .iter()
                    .map(|b| Branch {
                        when: b.when.map_vars(var),
                        actions: map_refs_in(&b.actions, var, script),
                    })
                    .collect();
                Action::Conditional {
                    branches,
                    otherwise: map_refs_in(otherwise, var, script),
                }
            }
            other => other.clone(),
        }
    }
}

fn map_cells(cells: &[CellWrite], var: &mut dyn FnMut(VarId) -> VarId) -> Vec<CellWrite> {
    cells
        .iter()
        .map(|c| CellWrite {
            column: c.column,
            to: c.to.map_vars(var),
        })
        .collect()
}

fn map_row(row: &WhichRow, var: &mut dyn FnMut(VarId) -> VarId) -> WhichRow {
    match row {
        WhichRow::FirstWhere { when } => WhichRow::FirstWhere {
            when: when.map_vars(var),
        },
        other => other.clone(),
    }
}

/// [`Action::map_refs`] over a list of actions.
fn map_refs_in(
    list: &[Keyed<Action>],
    var: &mut dyn FnMut(VarId) -> VarId,
    script: &mut dyn FnMut(ScriptId) -> ScriptId,
) -> Vec<Keyed<Action>> {
    list.iter()
        .map(|k| k.map(|a| a.map_refs(var, script)))
        .collect()
}

/// `f` on `action` and every action nested in it, depth first.
fn walk<'a>(action: &'a Action, f: &mut impl FnMut(&'a Action)) {
    f(action);
    for k in action.nested() {
        walk(&k.value, f);
    }
}

/// One `if` / `else if` of a [`Action::Conditional`].
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Branch {
    pub when: Expr,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub actions: Vec<Keyed<Action>>,
}

/// Where [`Action::SetMode`] sets its mode, as the editor would.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum ModeScope {
    /// On every page, so a frame that pins a mode of its own keeps it.
    #[default]
    Everywhere,
    /// On the screen shown, which beats the page.
    Screen,
}

/// How [`Action::SetVariable`] changes the value.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub enum VarChange {
    /// To what the expression comes to.
    Set { to: Expr },
    /// A number variable by `by`, which may be negative.
    Increment { by: f64 },
    /// A boolean variable to its opposite.
    Toggle,
}

/// How the screen gets from one state to the next.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct Transition {
    pub kind: TransitionKind,
    /// Read by the directional kinds only.
    #[serde(default)]
    pub direction: Direction,
    pub timing: Timing,
}

impl Transition {
    /// No animation at all.
    pub const INSTANT: Transition = Transition {
        kind: TransitionKind::Instant,
        direction: Direction::Left,
        timing: Timing::Ease {
            curve: Easing::EaseOut,
            ms: 0,
        },
    };
}

impl Default for Transition {
    /// What a new link takes on a page that names no default of its own.
    fn default() -> Self {
        Transition {
            kind: TransitionKind::Dissolve,
            direction: Direction::Left,
            timing: Timing::Ease {
                curve: Easing::EaseOut,
                ms: 300,
            },
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum TransitionKind {
    Instant,
    Dissolve,
    MoveIn,
    MoveOut,
    Push,
    Slide,
    /// Pair the layers present in both states by name and path and interpolate
    /// every animatable property.
    SmartAnimate,
}

impl TransitionKind {
    /// Whether [`Transition::direction`] means anything to it.
    pub fn is_directional(self) -> bool {
        matches!(
            self,
            TransitionKind::MoveIn
                | TransitionKind::MoveOut
                | TransitionKind::Push
                | TransitionKind::Slide
        )
    }
}

/// The side a directional transition moves toward.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum Direction {
    Left,
    Right,
    Up,
    Down,
    /// No side, the default: smart animate fades its unpaired layers rather than
    /// sliding them. A move, push or slide reads it as [`Direction::Left`].
    #[default]
    None,
}

/// How a transition moves through time: a curve over a duration, or a spring
/// that settles when it settles.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub enum Timing {
    Ease {
        curve: Easing,
        ms: u32,
    },
    Spring {
        mass: f64,
        stiffness: f64,
        damping: f64,
    },
}

impl Timing {
    /// Whether every number is finite and a spring's are positive.
    pub fn is_usable(&self) -> bool {
        match *self {
            Timing::Ease { curve, .. } => curve.is_usable(),
            Timing::Spring {
                mass,
                stiffness,
                damping,
            } => [mass, stiffness, damping]
                .iter()
                .all(|x| x.is_finite() && *x > 0.0),
        }
    }
}

/// The CSS easing set and a custom cubic Bézier.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub enum Easing {
    Linear,
    Ease,
    EaseIn,
    EaseOut,
    EaseInOut,
    /// `cubic-bezier(x1, y1, x2, y2)`; both x within 0..=1, as CSS requires.
    Cubic([f64; 4]),
}

impl Easing {
    fn is_usable(self) -> bool {
        match self {
            Easing::Cubic([x1, y1, x2, y2]) => {
                [x1, y1, x2, y2].iter().all(|v| v.is_finite())
                    && (0.0..=1.0).contains(&x1)
                    && (0.0..=1.0).contains(&x2)
            }
            _ => true,
        }
    }
}

/// A layer's prototype settings that are not reactions. The frame's fields are
/// inert on any other kind, and the child's are read only inside a scrolling frame.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct LayerPlay {
    /// On a frame: the axis it scrolls along when its content overflows.
    #[serde(default, skip_serializing_if = "is_default")]
    pub scroll: ScrollAxis,
    /// On a scrolling frame: where it comes to rest after a drag or the wheel.
    #[serde(default, skip_serializing_if = "is_default")]
    pub snap: Snap,
    /// On a frame that snaps to its children: which of a child's edges it
    /// brings to the frame's.
    #[serde(default, skip_serializing_if = "is_default")]
    pub snap_align: SnapAlign,
    /// On a child of a scrolling frame: whether it scrolls with the content.
    #[serde(default, skip_serializing_if = "is_default")]
    pub pin: Pin,
    /// The variable this layer is an input for, if any.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub input: Option<Input>,
    /// Whether, and how, the layer moves with a drag that starts on it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub follow: Option<Follow>,
    /// Properties that follow a scrolling frame's offset across a range.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub scroll_effect: Option<ScrollEffect>,
}

/// A layer's properties driven by a scrolling frame's offset: linear across
/// `from..to`, each holding its end value outside it. The scroll is the
/// timing, so there is no curve.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ScrollEffect {
    /// The frame whose offset it follows, read through [`resolve_target`];
    /// `None` follows the nearest scrolling frame above the layer.
    #[serde(default, skip_serializing_if = "Option::is_none", with = "wire_opt")]
    pub frame: Option<NodeId>,
    /// Where on the offset the effect starts and ends, in units along the
    /// frame's scroll (down, or across for a frame that scrolls sideways).
    pub from: f64,
    pub to: f64,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub props: Vec<EffectEnds>,
}

impl ScrollEffect {
    /// Whether every number is finite.
    pub fn is_usable(&self) -> bool {
        [self.from, self.to].iter().all(|x| x.is_finite())
            && self
                .props
                .iter()
                .all(|p| p.start.is_finite() && p.end.is_finite())
    }
}

/// One property of a [`ScrollEffect`], with its value at the start and end
/// of the range.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct EffectEnds {
    pub prop: ScrollProp,
    pub start: f64,
    pub end: f64,
}

/// What a scroll effect can drive.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum ScrollProp {
    /// 0–1, the layer's opacity in place of its own.
    Opacity,
    /// Units across, from where it is drawn.
    MoveX,
    /// Units down, from where it is drawn.
    MoveY,
    /// A factor about its centre, 1 as drawn.
    Scale,
    /// Degrees about its centre.
    Rotation,
}

impl ScrollProp {
    pub const ALL: [ScrollProp; 5] = [
        ScrollProp::Opacity,
        ScrollProp::MoveY,
        ScrollProp::MoveX,
        ScrollProp::Scale,
        ScrollProp::Rotation,
    ];

    /// Its value with no effect.
    pub fn rest(self) -> f64 {
        match self {
            ScrollProp::Opacity | ScrollProp::Scale => 1.0,
            _ => 0.0,
        }
    }
}

/// A layer that follows the drag: held to an axis and a range, and on release
/// staying, going back, or settling on a point.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Follow {
    #[serde(default, skip_serializing_if = "is_default")]
    pub axis: ScrollAxis,
    #[serde(default, skip_serializing_if = "is_default")]
    pub range: FollowRange,
    #[serde(default, skip_serializing_if = "is_default")]
    pub release: Release,
    /// How it settles after a release that moves it.
    #[serde(default = "settle_timing")]
    pub settle: Timing,
    /// The places it can rest, each an offset from where it is drawn. Where it
    /// is drawn is a point too, always the first, so it is not listed.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub points: Vec<Keyed<DragPoint>>,
    /// The name of the point where it is drawn, when renamed from
    /// [`DRAWN_POINT`].
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub drawn: Option<String>,
}

impl Default for Follow {
    fn default() -> Self {
        Follow {
            axis: ScrollAxis::Both,
            range: FollowRange::default(),
            release: Release::default(),
            settle: settle_timing(),
            points: Vec::new(),
            drawn: None,
        }
    }
}

impl Follow {
    /// The name of the point where it is drawn.
    pub fn drawn_name(&self) -> &str {
        self.drawn.as_deref().unwrap_or(DRAWN_POINT)
    }
}

fn settle_timing() -> Timing {
    Timing::Spring {
        mass: 1.0,
        stiffness: 300.0,
        damping: 30.0,
    }
}

/// How far a following layer may go.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum FollowRange {
    #[default]
    Free,
    /// Its box stays inside its parent's.
    Parent,
    /// Between its outermost points.
    Points,
}

/// What a following layer does when let go.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum Release {
    /// Stays where it is let go.
    #[default]
    Stays,
    /// Goes back to where it is drawn.
    Returns,
    /// Settles on the nearest point, or the next one a quick release throws it to.
    Snaps,
}

/// A named place a following layer can rest, as an offset from where it is
/// drawn.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct DragPoint {
    pub name: String,
    pub x: f64,
    pub y: f64,
}

/// The name of the point where a following layer is drawn, until renamed
/// ([`Follow::drawn`]).
pub const DRAWN_POINT: &str = "Start";

/// Where a *Drop* trigger asks the layer to have come to rest.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum DropAt {
    #[default]
    Anywhere,
    /// Where it is drawn: [`Follow::drawn_name`].
    Drawn,
    /// One of [`Follow::points`]; a removed one never matches.
    Point(ItemId),
}

impl LayerPlay {
    /// Whether it holds nothing but defaults.
    pub fn is_empty(&self) -> bool {
        *self == Self::default()
    }

    /// Whether its numbers are finite and its settle timing usable, as the
    /// door requires.
    pub fn is_usable(&self) -> bool {
        let follow = self.follow.as_ref().is_none_or(|f| {
            f.settle.is_usable()
                && f.points
                    .iter()
                    .all(|p| p.value.x.is_finite() && p.value.y.is_finite())
        });
        let effect = self.scroll_effect.as_ref().is_none_or(|e| e.is_usable());
        follow && effect
    }

    /// The layer it names: a scroll effect's frame.
    pub fn layer_target(&self) -> Option<NodeId> {
        self.scroll_effect.as_ref().and_then(|e| e.frame)
    }

    /// These settings with the layer they name passed through `f`.
    pub fn map_layers(&self, f: &mut impl FnMut(NodeId) -> NodeId) -> LayerPlay {
        let mut out = self.clone();
        if let Some(fr) = out.scroll_effect.as_mut().and_then(|e| e.frame.as_mut()) {
            *fr = f(*fr);
        }
        out
    }
}

fn is_default<T: Default + PartialEq>(v: &T) -> bool {
    *v == T::default()
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum ScrollAxis {
    #[default]
    None,
    Vertical,
    Horizontal,
    Both,
}

/// Where a scrolling frame comes to rest.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum Snap {
    /// Wherever it is let go.
    #[default]
    None,
    /// On its children's edges, as [`SnapAlign`] says.
    Children,
    /// On whole steps of its own width or height.
    Pages,
}

/// Which edge of a child a frame snapping to its children rests on.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum SnapAlign {
    /// The child's leading edge at the frame's leading inset.
    #[default]
    Start,
    Centre,
    /// The child's trailing edge at the frame's trailing inset.
    End,
}

/// How a scrolling frame's child behaves as its content scrolls.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum Pin {
    /// Moves with the content.
    #[default]
    Scrolls,
    /// Stays where it is drawn.
    Fixed,
    /// Scrolls until it reaches the place it is drawn at, then stays.
    Sticky,
}

/// How an overlay action shows its frame.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct OverlaySettings {
    #[serde(default)]
    pub position: OverlayPosition,
    /// Drawn over the frame beneath, `None` for none.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub backdrop: Option<Color>,
    /// Whether a click outside the overlay closes it.
    #[serde(default = "yes", skip_serializing_if = "is_true")]
    pub close_outside: bool,
}

impl Default for OverlaySettings {
    fn default() -> Self {
        OverlaySettings {
            position: OverlayPosition::default(),
            backdrop: None,
            close_outside: true,
        }
    }
}

impl OverlaySettings {
    /// Whether its position and backdrop are finite numbers.
    pub fn is_usable(&self) -> bool {
        let backdrop = self
            .backdrop
            .is_none_or(|c| c.components.iter().all(|x| x.is_finite()));
        let at = match self.position {
            OverlayPosition::Manual { x, y } => x.is_finite() && y.is_finite(),
            _ => true,
        };
        backdrop && at
    }
}

fn yes() -> bool {
    true
}

fn is_true(b: &bool) -> bool {
    *b
}

/// Where an overlay is placed over the screen.
#[derive(Clone, Copy, Debug, Default, PartialEq, Serialize, Deserialize)]
pub enum OverlayPosition {
    #[default]
    Centre,
    TopLeft,
    TopCentre,
    TopRight,
    BottomLeft,
    BottomCentre,
    BottomRight,
    /// The overlay's top-left this far from the top-left of the layer that opens
    /// it, so a menu stays by its button wherever the button (or an instance of
    /// it) sits.
    Manual {
        x: f64,
        y: f64,
    },
}

/// A layer bound as an input: typing or ticking during play writes the variable's
/// play value.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Input {
    pub var: VarId,
    /// On an instance: the boolean variant property that shows the variable (a
    /// checkbox's *Checked*). `None` on a text layer, which binds a string.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub property: Option<String>,
}

/// A named way into a page's prototype: a starting frame, starting modes set as
/// *Set mode* sets them everywhere (*Checkout, dark*), and starting values for
/// variables that beat their modes' values (*Checkout, signed in*).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Flow {
    pub name: String,
    #[serde(with = "wire")]
    pub start: NodeId,
    /// One mode per collection at most, of the document's or a kit's; a mode no
    /// longer there is passed over.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub modes: Vec<ModeId>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub values: Vec<(VarId, Cell)>,
}

/// A page's prototype settings.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct PagePlay {
    /// The device the screen plays on, by the player's preset name: its size is
    /// the screen's, and the export draws it round the screen. `None` plays the
    /// screen as drawn.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub device: Option<String>,
    #[serde(default, skip_serializing_if = "is_default")]
    pub scaling: Scaling,
    /// Whether the device is turned, the screen laid out at its size on its
    /// side. Inert with no device.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub landscape: bool,
    /// The transition a new link on this page takes.
    #[serde(default, skip_serializing_if = "is_default")]
    pub transition: Transition,
}

impl PagePlay {
    pub fn is_empty(&self) -> bool {
        *self == Self::default()
    }
}

/// How the screen is fitted to the player.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum Scaling {
    #[default]
    Fit,
    ActualSize,
    FillWidth,
    /// Laid out again at the player's size, through the frame's flex, grid and
    /// width queries; with a device, at the device's size (as every scaling
    /// is), fitted as *Fit*.
    Responsive,
}

impl PagePlay {
    /// The size the screen is laid out at, in a player whose room is `room`
    /// screen units, `turned` when it plays landscape; `None` as drawn. A
    /// device always gives its size, turned when turned, and the scaling only
    /// fits that screen to the room; with none, *Responsive* gives the room.
    /// Never larger than a sane screen.
    pub fn play_size(
        &self,
        scaling: Scaling,
        turned: bool,
        room: kurbo::Size,
    ) -> Option<kurbo::Size> {
        let device = self.device.as_deref().and_then(crate::presets::device_size);
        let size = match device {
            Some(d) if turned => Some(kurbo::Size::new(d.height, d.width)),
            Some(d) => Some(d),
            None if scaling == Scaling::Responsive => Some(room),
            None => None,
        }?;
        let side = |x: f64| x.round().clamp(1.0, 100_000.0);
        (size.width.is_finite() && size.height.is_finite())
            .then(|| kurbo::Size::new(side(size.width), side(size.height)))
    }

    /// Whether the page's device can turn: it has one.
    pub fn can_turn(&self) -> bool {
        self.device
            .as_deref()
            .and_then(crate::presets::device_size)
            .is_some()
    }
}

/// A flow named across pages.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct FlowRef {
    #[serde(with = "wire")]
    pub page: NodeId,
    pub flow: ItemId,
}

/// The HTML export's settings, kept with the document so a second export asks
/// nothing.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct PlayExport {
    /// The flows that go in; empty is every flow.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub flows: Vec<FlowRef>,
    /// The flow the file opens on; `None` is the first that goes in.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub opens: Option<FlowRef>,
    #[serde(default = "yes", skip_serializing_if = "is_true")]
    pub device_frame: bool,
    #[serde(default = "yes", skip_serializing_if = "is_true")]
    pub player_bar: bool,
    #[serde(default = "yes", skip_serializing_if = "is_true")]
    pub hotspot_hints: bool,
    /// `None` takes each page's own.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub scaling: Option<Scaling>,
    /// What a *Responsive* page does on a phone, which never draws the device
    /// frame: lay out at the phone and follow its turn, or keep the page's
    /// device and orientation, fitted.
    #[serde(default, skip_serializing_if = "OnPhone::is_follow")]
    pub on_phone: OnPhone,
}

/// [`PlayExport::on_phone`].
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum OnPhone {
    /// The screen laid out at the phone's size and orientation, again when it
    /// turns.
    #[default]
    Follow,
    /// The page's device and orientation, scaled to fit.
    Keep,
}

impl OnPhone {
    fn is_follow(&self) -> bool {
        *self == OnPhone::Follow
    }
}

impl Default for PlayExport {
    fn default() -> Self {
        PlayExport {
            flows: Vec::new(),
            opens: None,
            device_frame: true,
            player_bar: true,
            hotspot_hints: true,
            scaling: None,
            on_phone: OnPhone::Follow,
        }
    }
}

impl PlayExport {
    pub fn is_empty(&self) -> bool {
        *self == Self::default()
    }
}

/// Why the door refuses a layer's reactions.
#[derive(Clone, Debug, PartialEq, thiserror::Error)]
pub enum ReactionError {
    #[error("Run script needs a person's act to start it, not {0:?}")]
    ScriptUnasked(Trigger),
    #[error("a transition's timing is not usable")]
    BadTiming,
    #[error("an overlay's position and backdrop are finite numbers")]
    BadOverlay,
    #[error("two actions in one list share the id {0:?}")]
    DuplicateAction(ItemId),
    #[error("a scroll trigger's point is a finite number")]
    BadScrollPoint,
}

/// Checks a layer's reactions as the door takes them: a script only under a
/// person's act, usable timings, and unique action ids in each list. Targets are
/// never checked — they may dangle.
pub fn check(reactions: &[Keyed<Reaction>]) -> Result<(), ReactionError> {
    for r in reactions {
        if let Trigger::ScrollPast { at, .. } = r.trigger
            && !at.is_finite()
        {
            return Err(ReactionError::BadScrollPoint);
        }
        check_list(&r.trigger, &r.actions)?;
    }
    Ok(())
}

fn check_list(trigger: &Trigger, list: &[Keyed<Action>]) -> Result<(), ReactionError> {
    let mut ids: Vec<ItemId> = list.iter().map(|k| k.id).collect();
    ids.sort_unstable();
    if let Some(w) = ids.windows(2).find(|w| w[0] == w[1]) {
        return Err(ReactionError::DuplicateAction(w[0]));
    }
    for k in list {
        match &k.value {
            Action::RunScript { .. } if !trigger.is_a_persons_act() => {
                return Err(ReactionError::ScriptUnasked(trigger.clone()));
            }
            Action::Navigate { transition, .. }
            | Action::OpenOverlay { transition, .. }
            | Action::CloseOverlay { transition }
            | Action::SwapOverlay { transition, .. }
            | Action::ScrollTo { transition, .. }
            | Action::ChangeState { transition, .. }
                if !transition.timing.is_usable() =>
            {
                return Err(ReactionError::BadTiming);
            }
            Action::OpenOverlay { settings, .. } | Action::SwapOverlay { settings, .. }
                if !settings.is_usable() =>
            {
                return Err(ReactionError::BadOverlay);
            }
            Action::Conditional {
                branches,
                otherwise,
            } => {
                for b in branches {
                    check_list(trigger, &b.actions)?;
                }
                check_list(trigger, otherwise)?;
            }
            _ => {}
        }
    }
    Ok(())
}

/// Every item id in `reactions`: the reactions' own and their actions', nested
/// ones included — for `reserve_existing_ids`.
pub fn item_ids(reactions: &[Keyed<Reaction>]) -> impl Iterator<Item = ItemId> + '_ {
    reactions.iter().flat_map(|r| {
        let mut ids = vec![r.id];
        fn actions(list: &[Keyed<Action>], ids: &mut Vec<ItemId>) {
            for k in list {
                ids.push(k.id);
                let nested: Vec<_> = k.value.nested().cloned().collect();
                actions(&nested, ids);
            }
        }
        actions(&r.actions, &mut ids);
        ids
    })
}

/// `reactions` with every layer they name passed through `f`.
pub fn map_layers(
    reactions: &[Keyed<Reaction>],
    f: &mut impl FnMut(NodeId) -> NodeId,
) -> Vec<Keyed<Reaction>> {
    reactions
        .iter()
        .map(|r| {
            r.map(|r| Reaction {
                trigger: r.trigger.map_layers(f),
                actions: r
                    .actions
                    .iter()
                    .map(|a| a.map(|a| a.map_layers(f)))
                    .collect(),
            })
        })
        .collect()
}

/// `reactions` with every variable they read or set (a *variable changes*
/// trigger's too) passed through `var` and every script they run through
/// `script`.
pub fn map_refs(
    reactions: &[Keyed<Reaction>],
    var: &mut dyn FnMut(VarId) -> VarId,
    script: &mut dyn FnMut(ScriptId) -> ScriptId,
) -> Vec<Keyed<Reaction>> {
    reactions
        .iter()
        .map(|r| {
            r.map(|r| Reaction {
                trigger: match &r.trigger {
                    Trigger::VariableChanges { var: v } => {
                        Trigger::VariableChanges { var: var(*v) }
                    }
                    other => other.clone(),
                },
                actions: map_refs_in(&r.actions, var, script),
            })
        })
        .collect()
}

/// The variables `reactions` read or set and the scripts they run, each once,
/// in the order met.
pub fn refs(reactions: &[Keyed<Reaction>]) -> (Vec<VarId>, Vec<ScriptId>) {
    let (mut vars, mut scripts) = (Vec::new(), Vec::new());
    let _ = map_refs(
        reactions,
        &mut |v| {
            if !vars.contains(&v) {
                vars.push(v);
            }
            v
        },
        &mut |s| {
            if !scripts.contains(&s) {
                scripts.push(s);
            }
            s
        },
    );
    (vars, scripts)
}

/// The modes `reactions` set (*Set mode*), nested actions' included.
pub fn modes_set(reactions: &[Keyed<Reaction>]) -> Vec<ModeId> {
    let mut out = Vec::new();
    for k in reactions.iter().flat_map(|r| &r.value.actions) {
        walk(&k.value, &mut |a| {
            if let Action::SetMode { mode, .. } = a
                && !out.contains(mode)
            {
                out.push(*mode);
            }
        });
    }
    out
}

/// `reactions` with each mode a *Set mode* sets passed through `mode`, nested
/// actions' included.
pub fn map_modes(
    reactions: &[Keyed<Reaction>],
    mode: &mut dyn FnMut(ModeId) -> ModeId,
) -> Vec<Keyed<Reaction>> {
    reactions
        .iter()
        .map(|r| {
            r.map(|r| Reaction {
                trigger: r.trigger.clone(),
                actions: map_modes_in(&r.actions, mode),
            })
        })
        .collect()
}

fn map_modes_in(
    list: &[Keyed<Action>],
    mode: &mut dyn FnMut(ModeId) -> ModeId,
) -> Vec<Keyed<Action>> {
    list.iter()
        .map(|k| {
            k.map(|a| match a {
                Action::SetMode { mode: m, scope } => Action::SetMode {
                    mode: mode(*m),
                    scope: *scope,
                },
                Action::Conditional {
                    branches,
                    otherwise,
                } => Action::Conditional {
                    branches: branches
                        .iter()
                        .map(|b| Branch {
                            when: b.when.clone(),
                            actions: map_modes_in(&b.actions, mode),
                        })
                        .collect(),
                    otherwise: map_modes_in(otherwise, mode),
                },
                other => other.clone(),
            })
        })
        .collect()
}

/// `e` as its field shows it in `doc`: each variable and table read by name,
/// `{row.…}` read against `row`, a name that is gone left as its id.
pub fn expr_shown(doc: &Document, e: &Expr, row: Option<DataSetId>) -> String {
    let vars = doc.variables();
    let var_id = |n: &str| vars.ids_named(n);
    let var_name = |v: VarId| vars.expr_name(v);
    e.with_names_in(&expr::Names {
        var_id: &var_id,
        var_name: &var_name,
        tables: doc.data_sets(),
        row,
    })
}

/// The layer `target` names when read from a reaction on `holder`. A reaction on
/// a copy is its source's, so its target is first resolved in the source's world
/// (down the link chain, for a nested instance), then mapped to the layer of
/// `holder`'s copy linked to it. A target with no counterpart (a frame outside
/// the main, or a deleted one) is itself.
pub fn resolve_target(doc: &Document, holder: NodeId, target: NodeId) -> NodeId {
    let Some(source) = doc.get(holder).and_then(|n| n.link) else {
        return target;
    };
    let up = resolve_target(doc, source, target);
    // The root of the copy `holder` is part of: the top of its run of linked
    // ancestors.
    let mut root = holder;
    while let Some(p) = doc.get(root).and_then(|n| n.parent) {
        if doc.get(p).is_none_or(|p| p.link.is_none()) {
            break;
        }
        root = p;
    }
    crate::propagate::counterpart(doc, root, up).unwrap_or(up)
}
