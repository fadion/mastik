//! Prototype links read from other tools' files (`figma_in`, `penpot_in`,
//! `sketch_in`): the ones that map — a navigation, an overlay opened, swapped
//! or closed, back, a scroll to, an address, and the common transitions — made
//! into reactions once every layer is made; the rest counted as left out.
//!
//! A reader collects each layer's [`Link`]s with their targets by the file's
//! own ids, then [`reaction_ops`] turns them into `SetReactions` ops through
//! the reader's map of ids, a target it did not bring in counted and dropped.

use crate::foreign::{Tally, arr, num, str_of};
use crate::id::{IdSource, NodeId};
use crate::item::Keyed;
use crate::op::Operation;
use crate::prototype::{
    Action, Direction, Easing, Flow, Reaction, Timing, Transition, TransitionKind, Trigger,
};
use rustc_hash::FxHashMap;
use serde_json::Value;

/// One reaction read from a file: its trigger and its steps, in order.
pub(crate) struct Link {
    pub trigger: Trigger,
    pub steps: Vec<Step>,
}

/// One action of a [`Link`], a layer it names by the file's own id.
pub(crate) enum Step {
    Navigate(String, Transition),
    Overlay(String, Transition),
    Swap(String, Transition),
    ScrollTo(String, Transition),
    Close(Transition),
    Back,
    Url(String),
}

/// The report row for a link or a part of one that has no counterpart.
const LEFT_OUT: &str = "Prototype interactions";
/// The report row for a link to a layer the import did not bring in.
const NO_TARGET: &str = "Prototype links to layers not brought in";

/// The ops giving each holder its links as reactions, a target found through
/// `nodes`. A step whose target is not there is counted and dropped, and a
/// link left with no step with it.
pub(crate) fn reaction_ops(
    links: Vec<(NodeId, Vec<Link>)>,
    nodes: &FxHashMap<String, NodeId>,
    ids: &mut IdSource,
    tally: &mut Tally,
) -> Vec<Operation> {
    let mut ops = Vec::new();
    for (id, list) in links {
        let mut reactions = Vec::new();
        for link in list {
            let mut actions = Vec::new();
            for step in link.steps {
                let to = |fid: &str| nodes.get(fid).copied();
                let action = match step {
                    Step::Navigate(f, transition) => to(&f).map(|to| Action::Navigate {
                        to,
                        transition,
                        keep_scroll: false,
                    }),
                    Step::Overlay(f, transition) => to(&f).map(|overlay| Action::OpenOverlay {
                        overlay,
                        settings: Default::default(),
                        transition,
                    }),
                    Step::Swap(f, transition) => to(&f).map(|overlay| Action::SwapOverlay {
                        overlay,
                        settings: Default::default(),
                        transition,
                    }),
                    Step::ScrollTo(f, transition) => {
                        to(&f).map(|layer| Action::ScrollTo { layer, transition })
                    }
                    Step::Close(transition) => Some(Action::CloseOverlay { transition }),
                    Step::Back => Some(Action::Back),
                    Step::Url(url) => Some(Action::OpenUrl { url }),
                };
                match action {
                    Some(a) => actions.push(Keyed::new(ids.mint_item(), a)),
                    None => tally.count(NO_TARGET, 1, "", "Left out"),
                }
            }
            if !actions.is_empty() {
                reactions.push(Keyed::new(
                    ids.mint_item(),
                    Reaction {
                        trigger: link.trigger,
                        actions,
                    },
                ));
            }
        }
        if !reactions.is_empty() {
            ops.push(Operation::SetReactions { id, reactions });
        }
    }
    ops
}

/// The op giving `page` its flows, each a name and a start frame by the file's
/// own id; one whose frame was not brought in is left out.
pub(crate) fn flow_op(
    page: NodeId,
    starts: &[(String, String)],
    nodes: &FxHashMap<String, NodeId>,
    ids: &mut IdSource,
) -> Option<Operation> {
    let flows: Vec<Keyed<Flow>> = starts
        .iter()
        .filter_map(|(name, fid)| {
            Some(Keyed::new(
                ids.mint_item(),
                Flow {
                    name: name.clone(),
                    start: *nodes.get(fid)?,
                    modes: Vec::new(),
                    values: Vec::new(),
                },
            ))
        })
        .collect();
    (!flows.is_empty()).then_some(Operation::SetFlows { id: page, flows })
}

fn ease(kind: TransitionKind, direction: Direction, ms: f64, curve: Easing) -> Transition {
    Transition {
        kind,
        direction,
        timing: Timing::Ease {
            curve,
            ms: ms.clamp(0.0, 60_000.0).round() as u32,
        },
    }
}

// ── Figma ───────────────────────────────────────────────────────────────────

/// A Figma node's links: its `interactions`, or the older `transitionNodeID`
/// when it has none. What does not map is counted.
pub(crate) fn figma(n: &Value, tally: &mut Tally) -> Vec<Link> {
    let mut out = Vec::new();
    for i in arr(n, "interactions") {
        let Some(trigger) = i.get("trigger").and_then(figma_trigger) else {
            tally.count(LEFT_OUT, 1, "", "Left out");
            continue;
        };
        // `actions` now; a single `action` in files from before it.
        let actions: Vec<&Value> = match i.get("actions").and_then(Value::as_array) {
            Some(list) => list.iter().collect(),
            None => i.get("action").into_iter().collect(),
        };
        let mut steps = Vec::new();
        for a in actions {
            match figma_step(a) {
                Some(s) => steps.push(s),
                None => tally.count(LEFT_OUT, 1, "", "Left out"),
            }
        }
        if !steps.is_empty() {
            out.push(Link { trigger, steps });
        }
    }
    if out.is_empty()
        && let Some(to) = n.get("transitionNodeID").and_then(Value::as_str)
    {
        let curve = figma_curve(n.get("transitionEasing"));
        let timing = match curve {
            Some(Timing::Ease { curve, .. }) => Timing::Ease {
                curve,
                ms: num(n, "transitionDuration").max(0.0).round() as u32,
            },
            Some(spring) => spring,
            None => Timing::Ease {
                curve: Easing::EaseOut,
                ms: num(n, "transitionDuration").max(0.0).round() as u32,
            },
        };
        out.push(Link {
            trigger: Trigger::Click,
            steps: vec![Step::Navigate(
                to.to_owned(),
                Transition {
                    kind: TransitionKind::Dissolve,
                    direction: Direction::Left,
                    timing,
                },
            )],
        });
    }
    out
}

fn figma_trigger(t: &Value) -> Option<Trigger> {
    Some(match str_of(t, "type") {
        "ON_CLICK" => Trigger::Click,
        "ON_HOVER" => Trigger::HoverWhile,
        "ON_PRESS" => Trigger::PressWhile,
        "ON_DRAG" => Trigger::Drag,
        "MOUSE_ENTER" => Trigger::HoverEnter,
        "MOUSE_LEAVE" => Trigger::HoverLeave,
        "MOUSE_DOWN" => Trigger::Press,
        "AFTER_TIMEOUT" => Trigger::After {
            ms: (num(t, "timeout") * 1000.0).clamp(0.0, 3.6e6).round() as u32,
        },
        "ON_KEY_DOWN" => {
            let codes: Vec<u64> = arr(t, "keyCodes")
                .iter()
                .filter_map(Value::as_u64)
                .collect();
            let shift = codes.contains(&16);
            let keys: Vec<&u64> = codes.iter().filter(|c| **c != 16).collect();
            let [code] = keys[..] else { return None };
            Trigger::Key {
                key: key_name(*code)?,
                shift,
            }
        }
        _ => return None,
    })
}

/// A browser key code as the player names its key; `None` for one it does
/// not name.
fn key_name(code: u64) -> Option<String> {
    Some(match code {
        8 => "Backspace".into(),
        9 => "Tab".into(),
        13 => "Enter".into(),
        27 => "Escape".into(),
        32 => " ".into(),
        37 => "ArrowLeft".into(),
        38 => "ArrowUp".into(),
        39 => "ArrowRight".into(),
        40 => "ArrowDown".into(),
        48..=57 | 65..=90 => char::from_u32(code as u32)?.to_string(),
        _ => return None,
    })
}

fn figma_step(a: &Value) -> Option<Step> {
    match str_of(a, "type") {
        "BACK" => Some(Step::Back),
        "CLOSE" => Some(Step::Close(Transition::INSTANT)),
        "URL" => Some(Step::Url(str_of(a, "url").to_owned())),
        "NODE" => {
            let to = a.get("destinationId").and_then(Value::as_str)?.to_owned();
            let t = figma_transition(a.get("transition"))?;
            match str_of(a, "navigation") {
                "NAVIGATE" | "" => Some(Step::Navigate(to, t)),
                "OVERLAY" => Some(Step::Overlay(to, t)),
                "SWAP" => Some(Step::Swap(to, t)),
                "SCROLL_TO" => Some(Step::ScrollTo(to, t)),
                _ => None,
            }
        }
        _ => None,
    }
}

/// A Figma transition; none is instant. `None` for a kind with no counterpart.
fn figma_transition(t: Option<&Value>) -> Option<Transition> {
    let Some(t) = t.filter(|t| !t.is_null()) else {
        return Some(Transition::INSTANT);
    };
    let kind = match str_of(t, "type") {
        "DISSOLVE" => TransitionKind::Dissolve,
        "SMART_ANIMATE" => TransitionKind::SmartAnimate,
        "MOVE_IN" => TransitionKind::MoveIn,
        "MOVE_OUT" | "SLIDE_OUT" => TransitionKind::MoveOut,
        "PUSH" => TransitionKind::Push,
        "SLIDE_IN" => TransitionKind::Slide,
        _ => return None,
    };
    let direction = match str_of(t, "direction") {
        "RIGHT" => Direction::Right,
        "TOP" => Direction::Up,
        "BOTTOM" => Direction::Down,
        _ => Direction::Left,
    };
    let ms = num(t, "duration") * 1000.0;
    let timing = match figma_curve(t.get("easing")) {
        Some(Timing::Ease { curve, .. }) => Timing::Ease {
            curve,
            ms: ms.clamp(0.0, 60_000.0).round() as u32,
        },
        Some(spring) => spring,
        None => Timing::Ease {
            curve: Easing::EaseOut,
            ms: ms.clamp(0.0, 60_000.0).round() as u32,
        },
    };
    Some(Transition {
        kind,
        direction,
        timing,
    })
}

/// A Figma easing as a timing, its duration left at 0 for the caller; Figma's
/// named springs by their published mass, stiffness and damping.
fn figma_curve(e: Option<&Value>) -> Option<Timing> {
    let e = e?;
    let curve = |c| Some(Timing::Ease { curve: c, ms: 0 });
    let spring = |mass: f64, stiffness: f64, damping: f64| {
        Some(Timing::Spring {
            mass,
            stiffness,
            damping,
        })
    };
    // The older files write the easing as its word alone.
    let kind = e.as_str().unwrap_or_else(|| str_of(e, "type"));
    match kind {
        "LINEAR" => curve(Easing::Linear),
        "EASE_IN" => curve(Easing::EaseIn),
        "EASE_OUT" => curve(Easing::EaseOut),
        "EASE_IN_AND_OUT" => curve(Easing::EaseInOut),
        "CUSTOM_CUBIC_BEZIER" => {
            let b = e.get("easingFunctionCubicBezier")?;
            let c = [num(b, "x1"), num(b, "y1"), num(b, "x2"), num(b, "y2")];
            curve(Easing::Cubic(c))
        }
        "GENTLE" => spring(1.0, 100.0, 15.0),
        "QUICK" => spring(1.0, 300.0, 20.0),
        "BOUNCY" => spring(1.0, 600.0, 15.0),
        "SLOW" => spring(1.0, 80.0, 20.0),
        "CUSTOM_SPRING" => {
            let s = e.get("easingFunctionSpring")?;
            spring(num(s, "mass"), num(s, "stiffness"), num(s, "damping"))
        }
        _ => None,
    }
    .filter(Timing::is_usable)
}

// ── Penpot ──────────────────────────────────────────────────────────────────

/// `v`'s field `k`, written in camelCase or kebab-case.
fn either<'a>(v: &'a Value, camel: &str, kebab: &str) -> Option<&'a Value> {
    v.get(camel).or_else(|| v.get(kebab))
}

fn either_str<'a>(v: &'a Value, camel: &str, kebab: &str) -> &'a str {
    either(v, camel, kebab)
        .and_then(Value::as_str)
        .unwrap_or("")
        .trim_start_matches(':')
}

/// A Penpot shape's `interactions`. What does not map is counted.
pub(crate) fn penpot(shape: &Value, tally: &mut Tally) -> Vec<Link> {
    let mut out = Vec::new();
    for i in arr(shape, "interactions") {
        let trigger = match either_str(i, "eventType", "event-type") {
            "click" => Trigger::Click,
            "mouse-press" => Trigger::Press,
            "mouse-over" => Trigger::HoverWhile,
            "mouse-enter" => Trigger::HoverEnter,
            "mouse-leave" => Trigger::HoverLeave,
            "after-delay" => Trigger::After {
                ms: num(i, "delay").clamp(0.0, 3.6e6).round() as u32,
            },
            _ => {
                tally.count(LEFT_OUT, 1, "", "Left out");
                continue;
            }
        };
        let t = penpot_transition(either(i, "animation", "animation"));
        let to = || {
            i.get("destination")
                .and_then(Value::as_str)
                .map(str::to_owned)
        };
        let step = match either_str(i, "actionType", "action-type") {
            "navigate" => to().map(|d| Step::Navigate(d, t)),
            "open-overlay" | "toggle-overlay" => to().map(|d| Step::Overlay(d, t)),
            "close-overlay" => Some(Step::Close(t)),
            "prev-screen" => Some(Step::Back),
            "open-url" => Some(Step::Url(str_of(i, "url").to_owned())),
            _ => None,
        };
        match step {
            Some(s) => out.push(Link {
                trigger,
                steps: vec![s],
            }),
            None => tally.count(LEFT_OUT, 1, "", "Left out"),
        }
    }
    out
}

fn penpot_transition(a: Option<&Value>) -> Transition {
    let Some(a) = a.filter(|a| !a.is_null()) else {
        return Transition::INSTANT;
    };
    let kind = match (
        either_str(a, "animationType", "animation-type"),
        str_of(a, "way").trim_start_matches(':'),
    ) {
        ("dissolve", _) => TransitionKind::Dissolve,
        ("slide", "out") => TransitionKind::MoveOut,
        ("slide", _) => TransitionKind::MoveIn,
        ("push", _) => TransitionKind::Push,
        _ => return Transition::INSTANT,
    };
    let direction = match str_of(a, "direction").trim_start_matches(':') {
        "right" => Direction::Right,
        "up" => Direction::Up,
        "down" => Direction::Down,
        _ => Direction::Left,
    };
    let curve = match str_of(a, "easing").trim_start_matches(':') {
        "linear" => Easing::Linear,
        "ease" => Easing::Ease,
        "ease-in" => Easing::EaseIn,
        "ease-in-out" => Easing::EaseInOut,
        _ => Easing::EaseOut,
    };
    ease(kind, direction, num(a, "duration"), curve)
}

/// A Penpot page's flows, as names and start frames: `flows` on the page, a
/// list or a map by id, or under its `options` in older files.
pub(crate) fn penpot_flows(page: &Value) -> Vec<(String, String)> {
    let flows = page
        .get("flows")
        .or_else(|| page.get("options").and_then(|o| o.get("flows")));
    let list: Vec<&Value> = match flows {
        Some(Value::Array(a)) => a.iter().collect(),
        Some(Value::Object(m)) => m.values().collect(),
        _ => Vec::new(),
    };
    list.into_iter()
        .filter_map(|f| {
            let start = either(f, "startingFrame", "starting-frame")?.as_str()?;
            Some((str_of(f, "name").to_owned(), start.to_owned()))
        })
        .collect()
}

// ── Sketch ──────────────────────────────────────────────────────────────────

/// A Sketch layer's `flow` as a click: to its artboard, as an overlay when that
/// artboard is shown as one (`overlay`), or back. `None` for a flow to nowhere.
pub(crate) fn sketch(flow: &Value, overlay: &dyn Fn(&str) -> bool) -> Option<Link> {
    let to = str_of(flow, "destinationArtboardID");
    if to.is_empty() {
        return None;
    }
    // Sketch's animation types: none, then a slide from the right, the left,
    // the bottom and the top.
    let slide = |direction| ease(TransitionKind::MoveIn, direction, 300.0, Easing::EaseOut);
    let t = match flow.get("animationType").and_then(Value::as_i64) {
        Some(0) => slide(Direction::Left),
        Some(1) => slide(Direction::Right),
        Some(2) => slide(Direction::Up),
        Some(3) => slide(Direction::Down),
        _ => Transition::INSTANT,
    };
    let step = match to {
        "back" => Step::Back,
        _ if overlay(to) => Step::Overlay(to.to_owned(), t),
        _ => Step::Navigate(to.to_owned(), t),
    };
    Some(Link {
        trigger: Trigger::Click,
        steps: vec![step],
    })
}
