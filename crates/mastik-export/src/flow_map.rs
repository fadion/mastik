//! The flow map: a PDF of each flow's frames with their connectors, and every
//! interaction written out in words, for review and sign-off.
//!
//! Each flow on each page, in order, takes a map page and then as many pages of
//! words as its interactions need. The map is the flow's frames on its page as
//! they sit on the canvas, drawn by the PDF export's own path ([`crate::pdf`]),
//! with an arrow from each layer that moves the screen to the frame it shows
//! and a numbered badge on every layer with a reaction; the words pages list the
//! same numbers. A frame the flow reaches on another page is named, not drawn.

use std::fmt::Write as _;

use krilla_svg::SurfaceExt;
use mastik_core::kurbo::{Point, Rect};
use mastik_core::peniko::{Blob, FontData};
use mastik_core::prototype::{
    Action, Direction, Easing, Reaction, Timing, Transition, TransitionKind, Trigger, VarChange,
};
use mastik_core::{Document, ExportText, Keyed, NodeId, NodeKind, Resolved, subtree_nodes};

/// A words page's size, in points: A4.
const PAGE: (f64, f64) = (595.0, 842.0);
/// A words page's margin, in points.
const MARGIN: f64 = 48.0;
/// The words' size and line height, in points.
const TEXT_PT: f64 = 10.0;
const LINE_PT: f64 = 14.0;
/// How many characters a words line holds before it wraps: Inter at
/// [`TEXT_PT`] averages under 5.5 points a character across the page's width.
const WRAP: usize = 88;
/// The connectors' and badges' colour.
const INK: &str = "#1f6fff";

/// One reaction written out: its number on the map, the frame and layer that
/// hold it, and its words.
#[derive(Debug, Clone, PartialEq)]
pub struct Written {
    pub number: usize,
    pub frame: NodeId,
    pub layer: NodeId,
    pub text: String,
}

/// One flow as the map draws it: its frames in the order the flow reaches them,
/// starting frame first, and its reactions written out.
#[derive(Debug, Clone, PartialEq)]
pub struct FlowPlan {
    pub page: NodeId,
    pub name: String,
    pub frames: Vec<NodeId>,
    pub written: Vec<Written>,
}

/// Every flow of `doc`, page by page, each with the frames it reaches and its
/// reactions written out.
pub fn plans(doc: &Document) -> Vec<FlowPlan> {
    let mut out = Vec::new();
    for page in doc.pages() {
        for flow in &page.flows {
            let frames = reached(doc, flow.value.start);
            let mut written = Vec::new();
            for &frame in &frames {
                for layer in subtree_nodes(doc, &[frame]) {
                    let Some(n) = doc.get(layer) else { continue };
                    for r in n.reactions() {
                        written.push(Written {
                            number: written.len() + 1,
                            frame,
                            layer,
                            text: describe(doc, layer, &r.value),
                        });
                    }
                }
            }
            out.push(FlowPlan {
                page: page.id,
                name: flow.value.name.clone(),
                frames,
                written,
            });
        }
    }
    out
}

/// The flow map of `doc` as one PDF, or `None` when it has no flow or the
/// converter refuses a page.
pub fn flow_map(doc: &Document, res: &Resolved) -> Option<Vec<u8>> {
    let plans = plans(doc);
    if plans.is_empty() {
        return None;
    }
    let inter: Vec<FontData> = mastik_core::text::bundled_inter()
        .iter()
        .map(|b| FontData::new(Blob::from(b.to_vec()), 0))
        .collect();
    let mut document = krilla::Document::new();
    let mut outline = krilla::outline::Outline::new();
    let mut index = 0;
    for plan in &plans {
        let mut svgs = vec![(map_svg(doc, res, plan)?, plan.frames.clone())];
        for words in words_pages(doc, plan) {
            svgs.push((words, Vec::new()));
        }
        for (i, (svg, frames)) in svgs.iter().enumerate() {
            let shown = crate::pdf::shown_nodes(doc, frames);
            let mut fonts = crate::pdf::fonts_used(res, &shown);
            fonts.extend(inter.iter().cloned());
            let missed = std::sync::Mutex::new(Vec::new());
            let tree = crate::pdf::page_tree(svg, fonts, &missed)?;
            let size = krilla::geom::Size::from_wh(tree.size().width(), tree.size().height())?;
            let mut page = document.start_page_with(krilla::page::PageSettings::new(size));
            let mut surface = page.surface();
            surface.draw_svg(
                &tree,
                size,
                krilla_svg::SvgSettings {
                    embed_text: true,
                    filter_scale: crate::pdf::FILTER_SCALE,
                },
            );
            surface.finish();
            page.finish();
            if i == 0 {
                outline.push_child(krilla::outline::OutlineNode::new(
                    plan.name.clone(),
                    krilla::destination::XyzDestination::new(
                        index,
                        krilla::geom::Point::from_xy(0.0, 0.0),
                    ),
                ));
            }
            index += 1;
        }
    }
    document.set_outline(outline);
    document.finish().ok()
}

/// The frames a flow starting at `start` reaches, in the order met: every frame
/// a layer of a reached frame navigates to or opens as an overlay, breadth
/// first.
pub fn reached(doc: &Document, start: NodeId) -> Vec<NodeId> {
    let mut out = Vec::new();
    if is_frame(doc, start) {
        out.push(start);
    }
    let mut i = 0;
    while i < out.len() {
        let frame = out[i];
        i += 1;
        for (_, _, to) in moves(doc, frame) {
            if is_frame(doc, to) && !out.contains(&to) {
                out.push(to);
            }
        }
    }
    out
}

/// Every move out of `frame`: the layer holding it, whether it opens an
/// overlay, and the frame it shows, read as the holder reads its targets.
fn moves(doc: &Document, frame: NodeId) -> Vec<(NodeId, bool, NodeId)> {
    let mut out = Vec::new();
    for layer in subtree_nodes(doc, &[frame]) {
        let Some(n) = doc.get(layer) else { continue };
        for r in n.reactions() {
            each(&r.value.actions, &mut |a| {
                let (to, overlay) = match a {
                    Action::Navigate { to, .. } => (*to, false),
                    Action::OpenOverlay { overlay, .. } | Action::SwapOverlay { overlay, .. } => {
                        (*overlay, true)
                    }
                    _ => return,
                };
                let to = mastik_core::prototype::resolve_target(doc, layer, to);
                out.push((layer, overlay, to));
            });
        }
    }
    out
}

/// `f` on every action of `list` and those nested in it.
fn each(list: &[Keyed<Action>], f: &mut dyn FnMut(&Action)) {
    for k in list {
        f(&k.value);
        let nested: Vec<Keyed<Action>> = k.value.nested().cloned().collect();
        each(&nested, f);
    }
}

fn is_frame(doc: &Document, id: NodeId) -> bool {
    doc.get(id)
        .is_some_and(|n| matches!(n.kind(), NodeKind::Artboard { .. }))
}

// ── Words ───────────────────────────────────────────────────────────────────

/// `reaction` on `holder` in words: its trigger, then each action in order.
pub fn describe(doc: &Document, holder: NodeId, reaction: &Reaction) -> String {
    let actions = actions_words(doc, holder, &reaction.actions);
    format!("{}: {}", trigger_words(doc, &reaction.trigger), actions)
}

fn trigger_words(doc: &Document, t: &Trigger) -> String {
    match t {
        Trigger::Click => "On click".into(),
        Trigger::HoverEnter => "On hover".into(),
        Trigger::HoverWhile => "While hovering".into(),
        Trigger::HoverLeave => "On leave".into(),
        Trigger::DoubleClick => "On double click".into(),
        Trigger::Press => "On press".into(),
        Trigger::PressWhile => "While pressed".into(),
        Trigger::LongPress { .. } => "On long press".into(),
        Trigger::Drag => "On drag".into(),
        Trigger::Swipe { side } => format!("On swipe {}", side_word(*side)),
        Trigger::Drop { .. } => "On drop".into(),
        Trigger::ScrollPast { frame, at, .. } => {
            let name = doc.get(*frame).map_or("a missing frame", |n| n.name());
            format!("When {name} scrolls past {at}")
        }
        Trigger::Key { key, shift } => match shift {
            true => format!("On the key Shift+{key}"),
            false => format!("On the key {key}"),
        },
        Trigger::After { ms } => format!("After {ms} ms"),
        Trigger::VariableChanges { var } => format!("When {} changes", var_name(doc, *var)),
        Trigger::FrameOpens => "When the frame opens".into(),
    }
}

fn side_word(side: mastik_core::prototype::Side) -> &'static str {
    use mastik_core::prototype::Side;
    match side {
        Side::Left => "left",
        Side::Right => "right",
        Side::Up => "up",
        Side::Down => "down",
    }
}

fn actions_words(doc: &Document, holder: NodeId, list: &[Keyed<Action>]) -> String {
    if list.is_empty() {
        return "nothing".into();
    }
    list.iter()
        .map(|k| action_words(doc, holder, &k.value))
        .collect::<Vec<_>>()
        .join("; then ")
}

fn action_words(doc: &Document, holder: NodeId, a: &Action) -> String {
    let layer = |id: NodeId| {
        let id = mastik_core::prototype::resolve_target(doc, holder, id);
        layer_name(doc, holder, id)
    };
    match a {
        Action::Navigate { to, transition, .. } => {
            format!("go to {}{}", layer(*to), transition_words(transition))
        }
        Action::Back => "go back".into(),
        Action::OpenOverlay {
            overlay,
            transition,
            ..
        } => format!(
            "open {} as an overlay{}",
            layer(*overlay),
            transition_words(transition)
        ),
        Action::CloseOverlay { transition } => {
            format!("close the overlay{}", transition_words(transition))
        }
        Action::SwapOverlay {
            overlay,
            transition,
            ..
        } => format!(
            "swap the overlay for {}{}",
            layer(*overlay),
            transition_words(transition)
        ),
        Action::ScrollTo {
            layer: l,
            transition,
        } => {
            format!("scroll to {}{}", layer(*l), transition_words(transition))
        }
        Action::ChangeState {
            instance,
            to,
            transition,
        } => format!(
            "change {} to {}{}",
            layer(*instance),
            quoted(doc.get(*to).map(|n| n.name())),
            transition_words(transition)
        ),
        Action::SetVariable { var, change } => {
            let name = var_name(doc, *var);
            match change {
                VarChange::Set { to } => format!("set {name} to {}", expr_words(doc, to)),
                VarChange::Increment { by } if *by < 0.0 => {
                    format!("decrease {name} by {}", number(-by))
                }
                VarChange::Increment { by } => format!("increase {name} by {}", number(*by)),
                VarChange::Toggle => format!("toggle {name}"),
            }
        }
        Action::Conditional {
            branches,
            otherwise,
        } => {
            let mut out = String::new();
            for (i, b) in branches.iter().enumerate() {
                let head = if i == 0 { "if" } else { "; else if" };
                let _ = write!(
                    out,
                    "{head} {}, {}",
                    expr_words(doc, &b.when),
                    actions_words(doc, holder, &b.actions)
                );
            }
            if !otherwise.is_empty() {
                let _ = write!(
                    out,
                    "; otherwise, {}",
                    actions_words(doc, holder, otherwise)
                );
            }
            out
        }
        Action::SetMode { mode, scope } => {
            let t = doc.variables();
            let words = t
                .collection_of_mode(*mode)
                .and_then(|c| {
                    let m = c.modes.iter().find(|m| m.id == *mode)?;
                    Some((c.name.clone(), m.name.clone()))
                })
                .or_else(|| {
                    let (kit, m) = t.kit_mode_of(*mode)?;
                    Some((kit.to_owned(), m.name.clone()))
                });
            let here = match scope {
                mastik_core::prototype::ModeScope::Everywhere => "",
                mastik_core::prototype::ModeScope::Screen => " on this screen",
            };
            match words {
                Some((c, m)) => format!("set {c} to {}{here}", quoted(Some(&m))),
                None => "set a missing mode".into(),
            }
        }
        Action::OpenUrl { url } => format!("open {url}"),
        Action::RunScript { script } => format!(
            "run the script {}",
            quoted(
                doc.scripts()
                    .iter()
                    .find(|s| s.id == *script)
                    .map(|s| s.name.as_str())
            )
        ),
    }
}

/// How a move animates, as a parenthesis after it; nothing for *Instant*.
fn transition_words(t: &Transition) -> String {
    let kind = match t.kind {
        TransitionKind::Instant => return " (instantly)".into(),
        TransitionKind::Dissolve => "dissolve",
        TransitionKind::MoveIn => "move in",
        TransitionKind::MoveOut => "move out",
        TransitionKind::Push => "push",
        TransitionKind::Slide => "slide",
        TransitionKind::SmartAnimate => "smart animate",
    };
    let direction = match (t.kind.is_directional() || t.kind == TransitionKind::SmartAnimate)
        .then_some(t.direction)
    {
        Some(Direction::Left) => " left",
        Some(Direction::Right) => " right",
        Some(Direction::Up) => " up",
        Some(Direction::Down) => " down",
        Some(Direction::None) | None => "",
    };
    let timing = match t.timing {
        Timing::Ease { curve, ms } => {
            let curve = match curve {
                Easing::Linear => "linear".to_owned(),
                Easing::Ease => "ease".to_owned(),
                Easing::EaseIn => "ease in".to_owned(),
                Easing::EaseOut => "ease out".to_owned(),
                Easing::EaseInOut => "ease in and out".to_owned(),
                Easing::Cubic([a, b, c, d]) => format!(
                    "cubic-bezier({}, {}, {}, {})",
                    number(a),
                    number(b),
                    number(c),
                    number(d)
                ),
            };
            format!("{ms} ms {curve}")
        }
        Timing::Spring { .. } => "a spring".into(),
    };
    format!(" ({kind}{direction}, {timing})")
}

/// An expression as its field shows it: each variable by its name.
fn expr_words(doc: &Document, e: &mastik_core::prototype::Expr) -> String {
    e.with_names(&|v| doc.variables().get(v).map(|x| x.name.clone()))
}

fn var_name(doc: &Document, v: mastik_core::VarId) -> String {
    quoted(doc.variables().get(v).map(|x| x.name.as_str()))
}

/// A layer by its name in quotes, with its page where that is not the page of
/// `holder`; a missing one said so.
fn layer_name(doc: &Document, holder: NodeId, id: NodeId) -> String {
    let name = quoted(doc.get(id).map(|n| n.name()));
    let (here, there) = (doc.page_of(holder), doc.page_of(id));
    match there.filter(|p| Some(*p) != here).and_then(|p| doc.get(p)) {
        Some(page) => format!("{name} on the page {}", quoted(Some(page.name()))),
        None => name,
    }
}

fn quoted(name: Option<&str>) -> String {
    match name {
        Some(n) => format!("“{n}”"),
        None => "a missing layer".into(),
    }
}

/// A number written whole or with at most three decimals.
fn number(n: f64) -> String {
    let s = format!("{n:.3}");
    s.trim_end_matches('0').trim_end_matches('.').to_owned()
}

// ── Pages ───────────────────────────────────────────────────────────────────

/// The map page of `plan` as SVG: its frames on its page as the PDF export
/// draws them, framed with a margin and a title band, with the connectors and
/// badges over them.
fn map_svg(doc: &Document, res: &Resolved, plan: &FlowPlan) -> Option<String> {
    let here: Vec<NodeId> = plan
        .frames
        .iter()
        .copied()
        .filter(|f| doc.page_of(*f) == Some(plan.page))
        .collect();
    let extent = crate::extent(res, &here)?;
    let side = extent.width().max(extent.height());
    let margin = (side * 0.04).max(40.0);
    let unit = (side / 600.0).max(1.0);
    let view = Rect::new(
        extent.x0 - margin,
        extent.y0 - margin * 2.0,
        extent.x1 + margin,
        extent.y1 + margin,
    );
    let svg = crate::svg::svg_linking(
        doc,
        res,
        &here,
        Default::default(),
        ExportText::Live,
        Some(view),
        true,
    )
    .svg;
    let mut over = String::new();
    let _ = write!(
        over,
        "<defs><marker id=\"flow-map-arrow\" viewBox=\"0 0 10 10\" refX=\"9\" refY=\"5\" \
         markerWidth=\"6\" markerHeight=\"6\" orient=\"auto-start-reverse\">\
         <path d=\"M0 0L10 5L0 10z\" fill=\"{INK}\"/></marker></defs>"
    );
    let _ = write!(
        over,
        "<text x=\"{}\" y=\"{}\" font-family=\"Inter\" font-size=\"{}\" font-weight=\"600\" \
         fill=\"#111111\">{}</text>",
        view.x0 + margin,
        view.y0 + margin * 1.1,
        margin * 0.5,
        escape(&plan.name)
    );
    let rect = |id: NodeId| layer_rect(res, doc, id);
    // Each frame outlined and named above, as the canvas names it.
    for &frame in &here {
        let Some(r) = rect(frame) else { continue };
        let name = doc.get(frame).map_or("", |n| n.name());
        let _ = write!(
            over,
            "<rect x=\"{}\" y=\"{}\" width=\"{}\" height=\"{}\" fill=\"none\" stroke=\"#c8c8c8\" \
             stroke-width=\"{unit}\"/><text x=\"{}\" y=\"{}\" font-family=\"Inter\" \
             font-size=\"{}\" fill=\"#555555\">{}</text>",
            r.x0,
            r.y0,
            r.width(),
            r.height(),
            r.x0,
            r.y0 - unit * 6.0,
            unit * 12.0,
            escape(name)
        );
    }
    for &frame in &here {
        for (layer, overlay, to) in moves(doc, frame) {
            let (Some(from), Some(target)) = (rect(layer), rect(to)) else {
                continue;
            };
            if doc.page_of(to) != Some(plan.page) {
                continue;
            }
            let end = nearest(target, from.center());
            let start = nearest(from, end);
            let dash = match overlay {
                true => format!(" stroke-dasharray=\"{} {}\"", unit * 6.0, unit * 4.0),
                false => String::new(),
            };
            let _ = write!(
                over,
                "<path d=\"M{} {}L{} {}\" fill=\"none\" stroke=\"{INK}\" stroke-width=\"{}\"{dash} \
                 marker-end=\"url(#flow-map-arrow)\"/>",
                start.x,
                start.y,
                end.x,
                end.y,
                unit * 2.0
            );
        }
    }
    for w in &plan.written {
        if !here.contains(&w.frame) {
            continue;
        }
        let Some(r) = rect(w.layer) else { continue };
        let radius = unit * 9.0;
        let (x, y) = (r.x0, r.y0);
        let _ = write!(
            over,
            "<circle cx=\"{x}\" cy=\"{y}\" r=\"{radius}\" fill=\"{INK}\"/>\
             <text x=\"{x}\" y=\"{}\" font-family=\"Inter\" font-size=\"{}\" font-weight=\"600\" \
             text-anchor=\"middle\" fill=\"#ffffff\">{}</text>",
            y + radius * 0.4,
            radius * 1.1,
            w.number
        );
    }
    let at = svg.rfind("</svg>")?;
    Some(format!("{}{over}{}", &svg[..at], &svg[at..]))
}

/// The box `id` takes in world space: its own box for a layer with one, else
/// its bounds.
fn layer_rect(res: &Resolved, doc: &Document, id: NodeId) -> Option<Rect> {
    match res.used_kind(doc, id)?.box_size() {
        Some(size) => Some(
            res.world_transform(id)?
                .transform_rect_bbox(Rect::from_origin_size(Point::ZERO, size)),
        ),
        None => res.world_bounds(id),
    }
}

/// The point of `r` nearest `p`: `p` itself inside it, else on its edge.
fn nearest(r: Rect, p: Point) -> Point {
    Point::new(p.x.clamp(r.x0, r.x1), p.y.clamp(r.y0, r.y1))
}

/// The words pages of `plan` as SVG, each an A4 page of lines: the frames in
/// order, each with its numbered reactions, wrapped to the page.
fn words_pages(doc: &Document, plan: &FlowPlan) -> Vec<String> {
    let mut lines: Vec<(bool, String)> = vec![(true, plan.name.clone())];
    for &frame in &plan.frames {
        let name = doc.get(frame).map_or("", |n| n.name());
        let mut head = name.to_owned();
        if doc.page_of(frame) != Some(plan.page)
            && let Some(page) = doc.page_of(frame).and_then(|p| doc.get(p))
        {
            let _ = write!(head, " (on the page {})", quoted(Some(page.name())));
        }
        lines.push((false, String::new()));
        lines.push((true, head));
        let mut any = false;
        for w in plan.written.iter().filter(|w| w.frame == frame) {
            any = true;
            let who = doc.get(w.layer).map_or("", |n| n.name());
            let text = format!("{}. {who} — {}", w.number, w.text);
            for (i, part) in wrap(&text, WRAP).into_iter().enumerate() {
                let indent = if i == 0 { "" } else { "    " };
                lines.push((false, format!("{indent}{part}")));
            }
        }
        if !any {
            lines.push((false, "No interactions.".into()));
        }
    }
    let per_page = ((PAGE.1 - MARGIN * 2.0) / LINE_PT).floor() as usize;
    lines
        .chunks(per_page.max(1))
        .map(|chunk| {
            let mut svg = format!(
                "<svg xmlns=\"http://www.w3.org/2000/svg\" width=\"{w}\" height=\"{h}\" \
                 viewBox=\"0 0 {w} {h}\"><rect width=\"{w}\" height=\"{h}\" fill=\"#ffffff\"/>",
                w = PAGE.0,
                h = PAGE.1
            );
            for (i, (bold, text)) in chunk.iter().enumerate() {
                let weight = if *bold { 600 } else { 400 };
                let _ = write!(
                    svg,
                    "<text x=\"{MARGIN}\" y=\"{}\" font-family=\"Inter\" font-size=\"{TEXT_PT}\" \
                     font-weight=\"{weight}\" fill=\"#111111\" xml:space=\"preserve\">{}</text>",
                    MARGIN + LINE_PT * (i as f64 + 1.0),
                    escape(text)
                );
            }
            svg.push_str("</svg>");
            svg
        })
        .collect()
}

/// `text` broken at spaces into lines of at most `width` characters; a word
/// longer than that is a line of its own.
fn wrap(text: &str, width: usize) -> Vec<String> {
    let mut out = Vec::new();
    let mut line = String::new();
    for word in text.split(' ') {
        if !line.is_empty() && line.chars().count() + 1 + word.chars().count() > width {
            out.push(std::mem::take(&mut line));
        }
        if !line.is_empty() {
            line.push(' ');
        }
        line.push_str(word);
    }
    out.push(line);
    out
}

fn escape(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A line breaks at the last space that fits, and a long word stands alone.
    #[test]
    fn words_wrap_at_spaces() {
        assert_eq!(wrap("aa bb cc", 5), vec!["aa bb", "cc"]);
        assert_eq!(wrap("aaaaaaa b", 3), vec!["aaaaaaa", "b"]);
    }
}
