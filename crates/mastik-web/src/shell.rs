//! The page's arithmetic, apart from the browser so it builds and is tested on the
//! desktop (`mastik-export`'s `tests/web_shell.rs` includes this file): where the
//! screen goes under a *Scaling*, how a pointer maps back into screen space, how
//! a browser key is named the way a key trigger names it, where play opens, what
//! a notice says, and inspect mode's pick and measures.

use mastik_core::kurbo::{Point, Rect, Size, Vec2};
use mastik_core::measure;
use mastik_core::prototype::{FlowRef, Scaling};
use mastik_core::{Document, NodeId, NodeKind, subtree_nodes};
use mastik_play::{Notice, Start};

/// Every flow in the document, in page order, with its name.
pub fn flows(doc: &Document) -> Vec<(FlowRef, String)> {
    doc.pages()
        .iter()
        .flat_map(|p| {
            p.flows.iter().map(|f| {
                let r = FlowRef {
                    page: p.id,
                    flow: f.id,
                };
                (r, f.value.name.clone())
            })
        })
        .collect()
}

/// The first page's first frame, where a file with no flow opens.
pub fn first_frame(doc: &Document) -> Option<NodeId> {
    let page = doc.first_page();
    doc.get(page)?.children().iter().copied().find(|c| {
        doc.get(*c)
            .is_some_and(|n| matches!(n.kind(), NodeKind::Artboard { .. }))
    })
}

/// Where play opens: frame `asked` (a frame's id; a mirrored page's) when the
/// document holds that frame, else the export's opening flow among `flows`, else
/// the first flow, else the first page's first frame. With it, the index in
/// `flows` of the flow playing, none when a frame was asked. `None` when the
/// document has no frame to open.
pub fn opening(
    doc: &Document,
    flows: &[(FlowRef, String)],
    asked: &str,
) -> Option<(Start, Option<usize>)> {
    let opens = doc.play_export().opens;
    let flow = flows
        .iter()
        .position(|(r, _)| Some(*r) == opens)
        .or((!flows.is_empty()).then_some(0));
    let asked = NodeId::from_wire(asked).filter(|id| {
        doc.get(*id)
            .is_some_and(|n| matches!(n.kind(), NodeKind::Artboard { .. }))
    });
    match (asked, flow) {
        (Some(f), _) => Some((Start::Frame(f), None)),
        (None, Some(i)) => Some((Start::Flow(flows[i].0), Some(i))),
        (None, None) => Some((Start::Frame(first_frame(doc)?), None)),
    }
}

/// The device frame's bezel round the screen, in screen units.
pub const BEZEL: f64 = 14.0;

/// The gap a fitted device frame keeps from the window's edges, in CSS pixels.
pub const MARGIN: f64 = 24.0;

/// A screen whose shorter side is under this many units is a phone's, drawn with
/// a phone's rounder corners; at or over it, a tablet's.
const PHONE_SIDE: f64 = 600.0;

/// Where the page puts the screen, all in CSS pixels but `scale`.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Placement {
    /// CSS pixels per screen unit.
    pub scale: f64,
    /// The scrolling stage's size: the room, grown to hold a screen larger than
    /// it, so the page scrolls to the rest.
    pub stage: Size,
    /// The screen's top-left within the stage.
    pub at: Point,
    /// The screen's size.
    pub screen: Size,
    /// The device frame's bezel; 0 with none.
    pub bezel: f64,
    /// The screen's corner radius; 0 with no device frame.
    pub radius: f64,
}

/// Place a `screen` (screen units) in a `room` (CSS pixels) by `scaling`, inside
/// a device frame when `device`. *Fit* shows the whole device, larger or smaller
/// than life; *Actual size* is one CSS pixel a unit; *Fill width* fits the
/// width and lets the height run on, for the stage to scroll.
pub fn place(scaling: Scaling, screen: Size, room: Size, device: bool) -> Placement {
    let bezel = if device { BEZEL } else { 0.0 };
    let outer = Size::new(screen.width + 2.0 * bezel, screen.height + 2.0 * bezel);
    let margin = if device { MARGIN } else { 0.0 };
    let avail = Size::new(
        (room.width - 2.0 * margin).max(1.0),
        (room.height - 2.0 * margin).max(1.0),
    );
    let w = outer.width.max(1.0);
    let h = outer.height.max(1.0);
    let scale = match scaling {
        // Laid out at the room, a screen with no device fits it one to one;
        // a device's size is fitted.
        Scaling::Fit | Scaling::Responsive => (avail.width / w).min(avail.height / h),
        Scaling::ActualSize => 1.0,
        Scaling::FillWidth => avail.width / w,
    };
    let drawn = Size::new(outer.width * scale, outer.height * scale);
    let stage = Size::new(
        room.width.max(drawn.width + 2.0 * margin),
        room.height.max(drawn.height + 2.0 * margin),
    );
    let at = Point::new(
        ((stage.width - drawn.width) / 2.0 + bezel * scale).round(),
        ((stage.height - drawn.height) / 2.0 + bezel * scale).round(),
    );
    let radius = match device {
        false => 0.0,
        true if screen.width.min(screen.height) < PHONE_SIDE => 40.0 * scale,
        true => 18.0 * scale,
    };
    Placement {
        scale,
        stage,
        at,
        screen: Size::new(screen.width * scale, screen.height * scale),
        bezel: bezel * scale,
        radius,
    }
}

/// A point on the screen canvas, in CSS pixels from its top-left, in screen space.
pub fn to_screen(css: Point, scale: f64) -> Point {
    let s = if scale > 0.0 { scale } else { 1.0 };
    Point::new(css.x / s, css.y / s)
}

/// A wheel event's delta in screen units, positive into the content. `mode` is
/// the DOM's `deltaMode`: pixels (0), lines (1) or pages (2), a page being
/// `page` CSS pixels tall.
pub fn wheel_delta(dx: f64, dy: f64, mode: u32, scale: f64, page: f64) -> Vec2 {
    let unit = match mode {
        1 => 16.0,
        2 => page.max(1.0),
        _ => 1.0,
    };
    let s = if scale > 0.0 { scale } else { 1.0 };
    Vec2::new(dx * unit / s, dy * unit / s)
}

/// A DOM `KeyboardEvent.key` as a key trigger names it (egui's `Key::name`:
/// `"A"`, `"Space"`, `"Left"`, `"Comma"`), or `None` for a key a trigger never
/// names: a modifier alone, a dead key, a key the browser cannot name.
pub fn key_name(key: &str) -> Option<String> {
    let named = match key {
        "ArrowDown" => "Down",
        "ArrowLeft" => "Left",
        "ArrowRight" => "Right",
        "ArrowUp" => "Up",
        " " => "Space",
        ":" => "Colon",
        "," => "Comma",
        "-" => "Minus",
        "." => "Period",
        "+" => "Plus",
        "=" => "Equals",
        ";" => "Semicolon",
        "\\" => "Backslash",
        "/" => "Slash",
        "|" => "Pipe",
        "?" => "Questionmark",
        "!" => "Exclamationmark",
        "[" => "OpenBracket",
        "]" => "CloseBracket",
        "{" => "OpenCurlyBracket",
        "}" => "CloseCurlyBracket",
        "`" => "Backtick",
        "'" => "Quote",
        "Escape" | "Tab" | "Backspace" | "Enter" | "Insert" | "Delete" | "Home" | "End"
        | "PageUp" | "PageDown" | "Copy" | "Cut" | "Paste" => key,
        _ => {
            let mut chars = key.chars();
            return match (chars.next(), chars.next()) {
                (Some(c), None) if c.is_ascii_alphanumeric() => Some(c.to_ascii_uppercase().into()),
                (Some('F'), Some(_))
                    if key[1..].parse::<u8>().is_ok_and(|n| (1..=35).contains(&n)) =>
                {
                    Some(key.into())
                }
                _ => None,
            };
        }
    };
    Some(named.into())
}

/// What play's notice says, a sentence; a first character `!` marks one the
/// viewer is shown, the rest go to the console.
pub fn notice_text(n: Notice) -> String {
    match n {
        Notice::ScriptsOff => "!Scripts do not run in an exported prototype".into(),
        Notice::Script { name, message } => format!("Script {name} failed: {message}"),
        Notice::ScriptNeedsInput { name } => {
            format!("Script {name} asks for input, which an action cannot give")
        }
        Notice::Loop => "Reactions set each other off too deep; the chain was cut".into(),
        Notice::Missing { .. } => "An action's target is not there; it was skipped".into(),
        Notice::Expression { error, .. } => format!("An expression failed: {error}"),
        Notice::Fallback(f) => format!("Play fell back: {f:?}"),
        Notice::ScrollFrameGone { .. } => {
            "A scroll trigger watches a frame that is gone; it never fires".into()
        }
        Notice::TableFull { table, .. } => {
            let n = mastik_core::chart::MAX_ROWS;
            format!(
                "Skipped Add row: {table} holds {},{:03} rows, the most a table can",
                n / 1000,
                n % 1000
            )
        }
    }
}

/// The topmost layer of `frame` (the frame itself under its children) whose box,
/// by `rect`, holds `at`, leaving out a layer hidden itself or under a hidden
/// parent: inspect mode's pick.
pub fn topmost(
    doc: &Document,
    frame: NodeId,
    at: Point,
    rect: impl Fn(NodeId) -> Option<Rect>,
) -> Option<NodeId> {
    let hidden = |id: NodeId| doc.get(id).is_none_or(|n| !n.visible());
    let mut hit = None;
    for id in subtree_nodes(doc, &[frame]) {
        let shown = !std::iter::successors(Some(id), |n| doc.get(*n)?.parent())
            .take_while(|n| *n != frame)
            .any(hidden);
        if shown && rect(id).is_some_and(|r| r.contains(at)) {
            hit = Some(id);
        }
    }
    hit
}

/// The distances between boxes `a` and `b` as the canvas's `Alt` measure quotes
/// them, at `scale` CSS pixels per screen unit: each `"h"` or `"v"` (the line's
/// direction), where it sits, where it runs from and to, and its label, joined
/// by `|`.
pub fn measure_lines(a: Rect, b: Rect, scale: f64) -> Vec<String> {
    measure::between(a, b)
        .into_iter()
        .map(|m| {
            let axis = match m.axis {
                measure::Axis::Horizontal => "h",
                measure::Axis::Vertical => "v",
            };
            let label = measure::label(m.distance());
            format!(
                "{axis}|{}|{}|{}|{label}",
                m.at * scale,
                m.from * scale,
                m.to * scale
            )
        })
        .collect()
}

/// The device pixels per screen unit to draw a `screen` at: `scale`, lowered so
/// neither side of the drawing passes `side` pixels (WebGPU's texture limit) and
/// its area stays within `area` (Mobile Safari's canvas cap). Lowering costs only
/// sharpness; past either limit the canvas draws nothing at all.
pub fn draw_scale(screen: Size, scale: f64, side: u32, area: f64) -> f64 {
    let (w, h) = (screen.width.max(1.0), screen.height.max(1.0));
    let side = f64::from(side);
    scale
        .min(side / w)
        .min(side / h)
        .min((area / (w * h)).sqrt())
}

/// The text a DOM key types into a focused field: one printable character, or
/// `None` for a named key (`"Enter"`, `"Backspace"`).
pub fn typed(key: &str) -> Option<&str> {
    let mut chars = key.chars();
    match (chars.next(), chars.next()) {
        (Some(c), None) if !c.is_control() => Some(key),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const PHONE: Size = Size::new(390.0, 844.0);

    /// Fit with no device fills the room's tighter side and centres the other.
    #[test]
    fn fit_fills_the_tighter_side_and_centres_the_other() {
        let p = place(Scaling::Fit, PHONE, Size::new(1000.0, 422.0), false);
        assert_eq!(p.scale, 0.5);
        assert_eq!(p.screen, Size::new(195.0, 422.0));
        assert_eq!(p.at, Point::new(403.0, 0.0));
        assert_eq!(p.stage, Size::new(1000.0, 422.0));
        assert_eq!((p.bezel, p.radius), (0.0, 0.0));
    }

    /// Fit grows a small screen to the room as well as shrinking a large one.
    #[test]
    fn fit_grows_a_small_screen() {
        let p = place(
            Scaling::Fit,
            Size::new(100.0, 50.0),
            Size::new(400.0, 400.0),
            false,
        );
        assert_eq!(p.scale, 4.0);
    }

    /// The device frame's bezel and margin come out of the room before fitting,
    /// and the screen sits inside the bezel.
    #[test]
    fn a_device_frame_fits_the_whole_device() {
        let room = Size::new(2000.0, 844.0 + 2.0 * BEZEL + 2.0 * MARGIN);
        let p = place(Scaling::Fit, PHONE, room, true);
        assert_eq!(p.scale, 1.0);
        assert_eq!(p.bezel, BEZEL);
        assert_eq!(p.at.y, MARGIN + BEZEL);
        assert_eq!(p.radius, 40.0, "a phone's corners");
        let tablet = place(Scaling::Fit, Size::new(820.0, 1180.0), room, true);
        assert!(
            tablet.radius < 18.0 && tablet.radius > 0.0,
            "a tablet's corners, scaled"
        );
    }

    /// Actual size never scales, and a screen larger than the room grows the
    /// stage so the page scrolls to the rest.
    #[test]
    fn actual_size_grows_the_stage_past_the_room() {
        let p = place(Scaling::ActualSize, PHONE, Size::new(300.0, 500.0), false);
        assert_eq!(p.scale, 1.0);
        assert_eq!(p.stage, PHONE);
        assert_eq!(p.at, Point::ZERO);
    }

    /// Fill width fits the width and lets a tall screen run on below the room.
    #[test]
    fn fill_width_runs_the_height_on() {
        let p = place(Scaling::FillWidth, PHONE, Size::new(780.0, 600.0), false);
        assert_eq!(p.scale, 2.0);
        assert_eq!(p.stage, Size::new(780.0, 1688.0));
        assert_eq!(p.at, Point::ZERO);
    }

    /// A pointer maps back by the scale the screen was placed at.
    #[test]
    fn a_pointer_maps_back_into_screen_space() {
        assert_eq!(
            to_screen(Point::new(100.0, 50.0), 0.5),
            Point::new(200.0, 100.0)
        );
        assert_eq!(to_screen(Point::new(7.0, 9.0), 0.0), Point::new(7.0, 9.0));
    }

    /// Wheel deltas in lines and pages become pixels, then screen units.
    #[test]
    fn wheel_deltas_become_screen_units() {
        assert_eq!(wheel_delta(0.0, 100.0, 0, 2.0, 600.0), Vec2::new(0.0, 50.0));
        assert_eq!(wheel_delta(0.0, 3.0, 1, 1.0, 600.0), Vec2::new(0.0, 48.0));
        assert_eq!(wheel_delta(1.0, 0.0, 2, 1.0, 600.0), Vec2::new(600.0, 0.0));
    }

    /// Browser keys come out as a key trigger records them.
    #[test]
    fn browser_keys_are_named_as_triggers_name_them() {
        for (dom, name) in [
            ("a", "A"),
            ("A", "A"),
            ("7", "7"),
            (" ", "Space"),
            ("ArrowLeft", "Left"),
            ("Enter", "Enter"),
            (",", "Comma"),
            ("F5", "F5"),
        ] {
            assert_eq!(key_name(dom).as_deref(), Some(name), "{dom:?}");
        }
        for dom in ["Shift", "Control", "Dead", "Unidentified", "é", "F0", "Fn"] {
            assert_eq!(key_name(dom), None, "{dom:?}");
        }
    }

    /// A tall screen at a high density draws within the limits: a 1440 × 5000
    /// page at 2× keeps its side within 8192 and its area within the cap, while a
    /// phone screen keeps its full scale.
    /// flip: `draw_scale` returning `scale` → fails at "within the side".
    #[test]
    fn a_tall_screen_draws_within_the_limits() {
        let tall = Size::new(1440.0, 5000.0);
        let s = draw_scale(tall, 2.0, 8192, f64::INFINITY);
        assert!((tall.height * s).round() <= 8192.0, "within the side");
        assert!(s > 1.6, "lowered no more than it must: {s}");
        let s = draw_scale(tall, 2.0, 32_767, 16_777_216.0);
        assert!(
            tall.width * s * tall.height * s <= 16_777_216.0,
            "within the area"
        );
        assert_eq!(draw_scale(PHONE, 3.0, 8192, 16_777_216.0), 3.0, "a phone");
    }

    /// One printable character types; a named key does not.
    #[test]
    fn only_a_printable_character_types() {
        assert_eq!(typed("é"), Some("é"));
        assert_eq!(typed(" "), Some(" "));
        assert_eq!(typed("Enter"), None);
        assert_eq!(typed("\u{8}"), None);
    }
}
