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

use crate::document::Document;
use crate::id::NodeId;
use crate::item::{ItemId, Keyed};
use crate::script::ScriptId;
use crate::variable::{Cell, VarId};
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
    /// The pointer goes down on the layer.
    Press,
    /// While the pointer is held down on the layer.
    PressWhile,
    /// A drag starting on the layer; inside a scrolling frame it takes the drag
    /// from the scroll.
    Drag,
    /// A key, plain or Shift-ed; Ctrl and Alt chords belong to the editor.
    Key {
        /// The key's name as the player reads it (`"A"`, `"Enter"`, `"ArrowLeft"`).
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
}

impl Trigger {
    /// Whether a person's act fires it. Only such a trigger may run a script:
    /// under the others a script would run unasked, which the document never does.
    pub fn is_a_persons_act(&self) -> bool {
        !matches!(
            self,
            Trigger::After { .. } | Trigger::VariableChanges { .. } | Trigger::FrameOpens
        )
    }
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
    },
    /// Pop the navigation history; with an overlay open, close the top overlay.
    Back,
    /// Open a frame as an overlay, placed by its own [`LayerPlay::overlay`].
    OpenOverlay {
        #[serde(with = "wire")]
        overlay: NodeId,
        transition: Transition,
    },
    /// Close the top overlay.
    CloseOverlay { transition: Transition },
    /// Replace the top overlay with another frame.
    SwapOverlay {
        #[serde(with = "wire")]
        overlay: NodeId,
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
}

impl Action {
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
            Action::Navigate { to, transition } => Action::Navigate {
                to: f(*to),
                transition: *transition,
            },
            Action::OpenOverlay {
                overlay,
                transition,
            } => Action::OpenOverlay {
                overlay: f(*overlay),
                transition: *transition,
            },
            Action::SwapOverlay {
                overlay,
                transition,
            } => Action::SwapOverlay {
                overlay: f(*overlay),
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
    /// On a frame: how it shows when opened as an overlay.
    #[serde(default, skip_serializing_if = "is_default")]
    pub overlay: OverlaySettings,
    /// On a child of a scrolling frame: whether it scrolls with the content.
    #[serde(default, skip_serializing_if = "is_default")]
    pub pin: Pin,
    /// The variable this layer is an input for, if any.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub input: Option<Input>,
}

impl LayerPlay {
    /// Whether it holds nothing but defaults.
    pub fn is_empty(&self) -> bool {
        *self == Self::default()
    }

    /// Whether every number in it is finite.
    pub fn is_usable(&self) -> bool {
        self.overlay.is_usable()
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

/// A frame's defaults as an overlay.
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
    fn is_usable(&self) -> bool {
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
    /// The overlay's top-left at this point of the screen.
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

/// A named way into a page's prototype: a starting frame, and starting values for
/// variables that beat their modes' values (*Checkout, signed in*).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Flow {
    pub name: String,
    #[serde(with = "wire")]
    pub start: NodeId,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub values: Vec<(VarId, Cell)>,
}

/// A page's prototype settings.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct PagePlay {
    /// The device drawn around the screen, by the player's preset name; `None`
    /// draws none.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub device: Option<String>,
    #[serde(default, skip_serializing_if = "is_default")]
    pub scaling: Scaling,
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
    #[error("two actions in one list share the id {0:?}")]
    DuplicateAction(ItemId),
}

/// Checks a layer's reactions as the door takes them: a script only under a
/// person's act, usable timings, and unique action ids in each list. Targets are
/// never checked — they may dangle.
pub fn check(reactions: &[Keyed<Reaction>]) -> Result<(), ReactionError> {
    for r in reactions {
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
                trigger: r.trigger.clone(),
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
