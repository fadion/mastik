//! `review`: the design checks `check_layout` leaves out, over the shown layers
//! under a page or some layers: layers overlapping, text on a busy ground,
//! spacing off its rhythm or off the scale, raw values where a variable or a
//! style fits, near-duplicate colours, radii that nearly agree, and low
//! contrast. Each check is a function of its own over a [`Scope`], so the MCP
//! `review` read and `mastik check review` run the same ones, and a *Design
//! system health* card can run any of them.

use crate::context::{own_box, shown_under};
use crate::contrast;
use mastik_core::kurbo::Rect;
use mastik_core::peniko::Color;
use mastik_core::variable::{Field, Value as VarValue};
use mastik_core::{Brush, Document, NodeId, NodeKind, Operation, Resolved, Transaction};
use mastik_render::{ImageStore, RenderOverrides, VelloCpuRenderer, Viewport};
use serde_json::{Value, json};
use std::collections::BTreeMap;

/// One thing a check found: a code naming the check, the layers it is about,
/// a line saying what is wrong and, where one fits, what to use instead.
#[derive(Clone, Debug, PartialEq)]
pub struct Finding {
    pub code: &'static str,
    pub ids: Vec<NodeId>,
    pub message: String,
    pub instead: Option<String>,
}

impl Finding {
    fn new(code: &'static str, ids: Vec<NodeId>, message: String) -> Self {
        Self {
            code,
            ids,
            message,
            instead: None,
        }
    }

    fn instead(self, instead: String) -> Self {
        Self {
            instead: Some(instead),
            ..self
        }
    }

    /// As the `review` read answers it.
    pub fn to_json(&self) -> Value {
        let mut v = json!({
            "code": self.code,
            "ids": self.ids.iter().map(|i| i.to_wire()).collect::<Vec<_>>(),
            "message": self.message,
        });
        if let Some(instead) = &self.instead {
            v["instead"] = json!(instead);
        }
        v
    }
}

/// What a review holds the layers to.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Options {
    /// The spacing scale's step in units: a gap or padding off a multiple of it
    /// is `off_scale`.
    pub step: f64,
    /// The standard `low_contrast` holds text to.
    pub level: contrast::Level,
}

impl Default for Options {
    fn default() -> Self {
        Self {
            step: 4.0,
            level: contrast::Level::Aa,
        }
    }
}

/// The layers a review covers: every shown layer under its roots, each root's
/// page, and the document they are in.
pub struct Scope<'a> {
    pub doc: &'a Document,
    pub res: &'a Resolved,
    pub roots: Vec<NodeId>,
    /// The shown layers under the roots, roots and pages included, in document
    /// order; a hidden layer's subtree and a mask are left out.
    pub layers: Vec<NodeId>,
}

impl<'a> Scope<'a> {
    pub fn new(doc: &'a Document, res: &'a Resolved, roots: &[NodeId]) -> Self {
        let layers = roots
            .iter()
            .filter(|r| doc.shown_through(**r))
            .flat_map(|r| shown_under(doc, *r))
            .collect();
        Self {
            doc,
            res,
            roots: roots.to_vec(),
            layers,
        }
    }

    /// The scope's layers that are not pages.
    fn own(&self) -> impl Iterator<Item = NodeId> + '_ {
        self.layers
            .iter()
            .copied()
            .filter(|id| !self.doc.is_page(*id))
    }

    /// `id`'s shown children a check compares, with their own boxes: not a mask
    /// or a slice, and not turned or skewed, whose box is not its outline.
    fn children(&self, id: NodeId) -> Vec<(NodeId, Rect)> {
        let Some(n) = self.doc.get(id) else {
            return Vec::new();
        };
        n.children()
            .iter()
            .copied()
            .filter(|c| self.doc.shown(*c) && self.doc.get(*c).is_some_and(|n| !n.mask()))
            .filter(|c| !matches!(self.kind(*c), Some(NodeKind::Slice { .. })))
            .filter(|c| upright(self.res, *c))
            .filter_map(|c| Some((c, own_box(self.doc, self.res, c)?)))
            .collect()
    }

    fn kind(&self, id: NodeId) -> Option<&NodeKind> {
        self.res.used_kind(self.doc, id)
    }
}

/// The most findings a review answers; past it, `more` counts the rest.
const LISTED: usize = 100;

/// The most layers a finding names; past it, its line counts them.
const NAMED: usize = 20;

/// The most texts whose ground is drawn to judge it busy.
const GROUNDS: usize = 150;

/// The longest side a text's ground is drawn at, in pixels.
const GROUND_PX: f64 = 240.0;

/// Every check, in the order the findings are listed.
pub fn review(scope: &Scope<'_>, options: Options) -> Vec<Finding> {
    let mut out = overlaps(scope);
    out.extend(busy_grounds(scope));
    out.extend(spacing(scope, options.step));
    out.extend(raw_values(scope));
    out.extend(near_duplicate_colours(scope));
    out.extend(radii(scope));
    out.extend(low_contrast(scope, options.level));
    out
}

/// What the `review` read answers: the first [`LISTED`] findings, how many
/// layers were looked at, and how many findings were left out.
pub fn to_json(scope: &Scope<'_>, findings: &[Finding]) -> Value {
    let mut answer = json!({
        "findings": findings.iter().take(LISTED).map(Finding::to_json).collect::<Vec<_>>(),
        "checked": scope.own().count(),
    });
    if findings.len() > LISTED {
        answer["more"] = json!(findings.len() - LISTED);
    }
    answer
}

/// A number as the lines write it: to two places, whole when it is whole.
fn num(v: f64) -> String {
    let r = (v * 100.0).round() / 100.0;
    match r.fract() == 0.0 {
        true => format!("{}", r as i64),
        false => format!("{r}"),
    }
}

/// A colour as `#rrggbb`, or `#rrggbbaa` when it is not opaque.
fn hex(c: Color) -> String {
    let [r, g, b, a] = c.to_rgba8().to_u8_array();
    match a {
        255 => format!("#{r:02x}{g:02x}{b:02x}"),
        _ => format!("#{r:02x}{g:02x}{b:02x}{a:02x}"),
    }
}

/// Whether `id` is drawn unturned and unskewed, so its box is its outline.
fn upright(res: &Resolved, id: NodeId) -> bool {
    res.world_transform(id).is_some_and(|w| {
        let [_, b, c, _, _, _] = w.as_coeffs();
        b.abs() < 1e-9 && c.abs() < 1e-9
    })
}

/// Whether `inner` lies within `outer`, a unit of slack on each side.
fn holds(outer: Rect, inner: Rect) -> bool {
    inner.x0 >= outer.x0 - 1.0
        && inner.y0 >= outer.y0 - 1.0
        && inner.x1 <= outer.x1 + 1.0
        && inner.y1 <= outer.y1 + 1.0
}

/// `ids`, at most [`NAMED`] of them.
fn named(ids: &[NodeId]) -> Vec<NodeId> {
    ids.iter().take(NAMED).copied().collect()
}

/// "1 layer", "3 layers".
fn layers(n: usize) -> String {
    match n {
        1 => "1 layer".into(),
        n => format!("{n} layers"),
    }
}

/// `overlap`: two siblings whose boxes share more than a sliver, neither holding
/// the other. Texts are `check_layout`'s (`text_overlap`, `text_covered`); a
/// background holds what lies on it and is passed over.
pub fn overlaps(scope: &Scope<'_>) -> Vec<Finding> {
    let mut out = Vec::new();
    for &parent in &scope.layers {
        let kids: Vec<(NodeId, Rect)> = scope
            .children(parent)
            .into_iter()
            .filter(|(c, _)| !matches!(scope.kind(*c), Some(NodeKind::Text { .. })))
            .collect();
        if kids.len() > 200 {
            continue;
        }
        for (i, &(a, ab)) in kids.iter().enumerate() {
            for &(b, bb) in &kids[i + 1..] {
                let x = ab.intersect(bb);
                let least = ab.area().min(bb.area());
                if x.width() <= 2.0
                    || x.height() <= 2.0
                    || x.area() <= 0.1 * least
                    || holds(ab, bb)
                    || holds(bb, ab)
                {
                    continue;
                }
                out.push(Finding::new(
                    "overlap",
                    vec![a, b],
                    format!(
                        "Two layers overlap by {} × {}, neither holding the other.",
                        num(x.width()),
                        num(x.height())
                    ),
                ));
            }
        }
    }
    out
}

/// `busy_ground`: a text whose ground is busy, as a photograph or a pattern is:
/// its box drawn without it, a tenth of the pixels or more changing sharply
/// from their neighbours. A gradient changes smoothly and is not one.
pub fn busy_grounds(scope: &Scope<'_>) -> Vec<Finding> {
    let (doc, res) = (scope.doc, scope.res);
    let texts: Vec<NodeId> = scope
        .own()
        .filter(|id| matches!(scope.kind(*id), Some(NodeKind::Text { .. })))
        .take(GROUNDS)
        .collect();
    if texts.is_empty() {
        return Vec::new();
    }
    let mut images = ImageStore::new();
    images.prepare(doc);
    let mut cpu = VelloCpuRenderer::new();
    let mut out = Vec::new();
    for id in texts {
        let (Some(page), Some(view)) = (doc.page_of(id), res.world_bounds(id)) else {
            continue;
        };
        if view.width() < 1.0 || view.height() < 1.0 {
            continue;
        }
        let scale = (GROUND_PX / view.width().max(view.height())).min(1.0);
        let vp = Viewport {
            view,
            pixel_size: (
                (view.width() * scale).ceil().max(1.0) as u32,
                (view.height() * scale).ceil().max(1.0) as u32,
            ),
        };
        let hide = Transaction(vec![Operation::SetOpacity { id, opacity: 0.0 }]);
        let Some(hidden) = RenderOverrides::from_transaction(doc, res, &hide) else {
            continue;
        };
        let (rgba, w, h) = cpu.render_subtrees_with(doc, res, &vp, &images, &[page], &hidden);
        let ground = doc.ground(page).components;
        let lum: Vec<f64> = rgba
            .as_chunks::<4>()
            .0
            .iter()
            .map(|p| {
                let a = f64::from(p[3]) / 255.0;
                let c: [f64; 3] = std::array::from_fn(|k| {
                    f64::from(p[k]) / 255.0 * a + f64::from(ground[k]) * (1.0 - a)
                });
                contrast::luminance(c)
            })
            .collect();
        if let Some(sharp) = sharp_share(&lum, w as usize, h as usize)
            && sharp >= 0.1
        {
            out.push(
                Finding::new(
                    "busy_ground",
                    vec![id],
                    format!(
                        "It sits on a busy ground: {}% of what is behind it changes sharply.",
                        (sharp * 100.0).round()
                    ),
                )
                .instead("a solid ground under it, or a calmer part of the picture".into()),
            );
        }
    }
    out
}

/// The share of pixels whose luminance differs by more than 0.08 from the
/// pixel right of it or below it; `None` for a raster too small to judge.
fn sharp_share(lum: &[f64], w: usize, h: usize) -> Option<f64> {
    if w < 4 || h < 4 || lum.len() < w * h {
        return None;
    }
    let mut sharp = 0usize;
    for y in 0..h - 1 {
        for x in 0..w - 1 {
            let at = lum[y * w + x];
            if (at - lum[y * w + x + 1]).abs() > 0.08 || (at - lum[(y + 1) * w + x]).abs() > 0.08 {
                sharp += 1;
            }
        }
    }
    Some(sharp as f64 / ((w - 1) * (h - 1)) as f64)
}

/// Whether `v` is a multiple of `step`, to a twentieth of a unit.
fn on_step(v: f64, step: f64) -> bool {
    step <= 0.0 || ((v / step).round() * step - v).abs() < 0.05
}

/// The scale's two values either side of `v`.
fn either_side(v: f64, step: f64) -> String {
    let lo = (v / step).floor() * step;
    format!("{} or {}", num(lo), num(lo + step))
}

/// `rhythm` and `off_scale`. A flex or grid container's gaps and padding off
/// the scale, those bound to a variable left to it; and between layers placed
/// by hand in one column or one row, a gap near the others but not one of them
/// (`rhythm`), and gaps off the scale.
pub fn spacing(scope: &Scope<'_>, step: f64) -> Vec<Finding> {
    const SLOTS: [&str; 6] = [
        "column gap",
        "row gap",
        "top padding",
        "right padding",
        "bottom padding",
        "left padding",
    ];
    let doc = scope.doc;
    let mut out = Vec::new();
    for &parent in &scope.layers {
        let Some(n) = doc.get(parent) else { continue };
        if let Some(display) = doc.resolved_display(parent) {
            let bound = n.spacing_vars();
            for (k, v) in display.spacing().into_iter().enumerate() {
                if v > 0.0 && bound[k].is_none() && !on_step(v, step) {
                    out.push(
                        Finding::new(
                            "off_scale",
                            vec![parent],
                            format!(
                                "Its {} is {}, off the {}-unit scale.",
                                SLOTS[k],
                                num(v),
                                num(step)
                            ),
                        )
                        .instead(either_side(v, step)),
                    );
                }
            }
            continue;
        }
        let mut kids = scope.children(parent);
        // A ground holding the rest is not one of the run.
        let all: Vec<Rect> = kids.iter().map(|(_, b)| *b).collect();
        kids.retain(|(_, b)| all.len() < 2 || kids_held(*b, &all) < all.len() - 1);
        if kids.len() < 2 {
            continue;
        }
        let Some(run) = run_of(&mut kids) else {
            continue;
        };
        let gaps: Vec<f64> = run
            .windows(2)
            .map(|w| ((w[1].1 - w[0].2) * 100.0).round() / 100.0)
            .collect();
        let mut sorted = gaps.clone();
        sorted.sort_by(f64::total_cmp);
        let median = sorted[sorted.len() / 2];
        let mut off: BTreeMap<i64, Vec<NodeId>> = BTreeMap::new();
        for (i, g) in gaps.iter().enumerate() {
            let pair = [run[i].0, run[i + 1].0];
            let apart = (g - median).abs();
            if gaps.len() >= 2 && apart > 0.5 && apart <= 4.0_f64.max(0.25 * median) {
                out.push(
                    Finding::new(
                        "rhythm",
                        pair.to_vec(),
                        format!(
                            "A gap of {} between them, where the others are {}.",
                            num(*g),
                            num(median)
                        ),
                    )
                    .instead(num(median)),
                );
                continue;
            }
            if *g > 0.5 && !on_step(*g, step) {
                let ids = off.entry((g * 100.0).round() as i64).or_default();
                for p in pair {
                    if !ids.contains(&p) {
                        ids.push(p);
                    }
                }
            }
        }
        for (g, ids) in off {
            let g = g as f64 / 100.0;
            out.push(
                Finding::new(
                    "off_scale",
                    named(&ids),
                    format!(
                        "A gap of {} between them, off the {}-unit scale.",
                        num(g),
                        num(step)
                    ),
                )
                .instead(either_side(g, step)),
            );
        }
    }
    out
}

/// How many of `all` lie within `b`.
fn kids_held(b: Rect, all: &[Rect]) -> usize {
    all.iter().filter(|o| **o != b && holds(b, **o)).count()
}

/// `kids` as one column or one row, each layer clear of the next: each with
/// where it starts and ends along the run, in order. `None` for layers placed
/// any other way.
fn run_of(kids: &mut [(NodeId, Rect)]) -> Option<Vec<(NodeId, f64, f64)>> {
    let along = |kids: &mut [(NodeId, Rect)], start: fn(&Rect) -> f64, end: fn(&Rect) -> f64| {
        kids.sort_by(|a, b| start(&a.1).total_cmp(&start(&b.1)));
        kids.windows(2)
            .all(|w| start(&w[1].1) >= end(&w[0].1) - 0.5)
            .then(|| kids.iter().map(|(id, b)| (*id, start(b), end(b))).collect())
    };
    along(kids, |r| r.y0, |r| r.y1).or_else(|| along(kids, |r| r.x0, |r| r.x1))
}

/// A raw value's matches, gathered per what fits it.
#[derive(Default)]
struct Raw {
    by: BTreeMap<String, (String, Vec<NodeId>)>,
}

impl Raw {
    fn add(&mut self, key: String, line: impl FnOnce() -> String, id: NodeId) {
        let entry = self.by.entry(key).or_insert_with(|| (line(), Vec::new()));
        if !entry.1.contains(&id) {
            entry.1.push(id);
        }
    }
}

/// `raw_value` and `raw_style`: a value written raw where a variable or a style
/// of the document holds exactly it, and the layer takes neither. A solid fill
/// or stroke against the colour variables that may bind it, a rectangle's or
/// frame's even radius against the number variables scoped to corners, a
/// layout's gaps and padding against those scoped to spacing, and a layer's
/// whole fill, stroke or effect list, or a text's type, against the styles.
pub fn raw_values(scope: &Scope<'_>) -> Vec<Finding> {
    let doc = scope.doc;
    let vars = doc.variables();
    let value = |v: &mastik_core::Variable| vars.resolve_opt(v.id, None);
    let colours: Vec<(&mastik_core::Variable, [u8; 4])> = vars
        .local()
        .filter_map(|v| match value(v) {
            Some(VarValue::Color(c)) => Some((v, c.to_rgba8().to_u8_array())),
            _ => None,
        })
        .collect();
    let numbers: Vec<(&mastik_core::Variable, f64)> = vars
        .local()
        .filter_map(|v| match value(v) {
            Some(VarValue::Number(n)) => Some((v, n)),
            _ => None,
        })
        .collect();
    let mut raw = Raw::default();
    for id in scope.own() {
        let Some(n) = doc.get(id) else { continue };
        let styles = n.styles();
        let paint = n.paint();
        let solid = |shown: bool, brush: &mastik_core::variable::BrushSource| match brush.literal()
        {
            Some(Brush::Solid(c)) if shown => Some(*c),
            _ => None,
        };
        let fills: Vec<Color> = paint
            .fills
            .iter()
            .filter_map(|i| solid(i.visible, &i.brush))
            .collect();
        let strokes: Vec<Color> = paint
            .strokes
            .iter()
            .filter_map(|i| solid(i.visible, &i.brush))
            .collect();
        for (solids, field, styled, what) in [
            (fills, Field::Fill, styles.fills.is_some(), "fill"),
            (strokes, Field::Stroke, styles.strokes.is_some(), "stroke"),
        ] {
            if styled {
                continue;
            }
            for c in solids {
                let bytes = c.to_rgba8().to_u8_array();
                if let Some((v, _)) = colours.iter().find(|(v, b)| *b == bytes && v.binds(field)) {
                    raw.add(
                        format!("v{}{what}", v.id.0.to_wire()),
                        || {
                            format!(
                                "A {what} of {} is the variable `{}`, written raw",
                                hex(c),
                                v.name
                            )
                        },
                        id,
                    );
                }
            }
        }
        if matches!(n.kind(), NodeKind::Rect { .. } | NodeKind::Artboard { .. })
            && n.corner_vars().iter().all(Option::is_none)
            && let Some(r) = scope.kind(id).and_then(NodeKind::corner_radii)
            && r.top_left > 0.0
            && [r.top_right, r.bottom_right, r.bottom_left]
                .iter()
                .all(|c| *c == r.top_left)
            && let Some((v, _)) = numbers
                .iter()
                .find(|(v, x)| *x == r.top_left && v.binds(Field::Corner))
        {
            raw.add(
                format!("v{}radius", v.id.0.to_wire()),
                || {
                    format!(
                        "A radius of {} is the variable `{}`, written raw",
                        num(r.top_left),
                        v.name
                    )
                },
                id,
            );
        }
        if let Some(d) = doc.resolved_display(id) {
            let bound = n.spacing_vars();
            for (k, x) in d.spacing().into_iter().enumerate() {
                if x <= 0.0 || bound[k].is_some() {
                    continue;
                }
                if let Some((v, _)) = numbers
                    .iter()
                    .find(|(v, y)| *y == x && v.binds(Field::Spacing))
                {
                    raw.add(
                        format!("v{}spacing", v.id.0.to_wire()),
                        || {
                            format!(
                                "A gap or padding of {} is the variable `{}`, written raw",
                                num(x),
                                v.name
                            )
                        },
                        id,
                    );
                }
            }
        }
        for style in doc.styles().iter().filter(|s| s.origin.is_none()) {
            use mastik_core::style::StyleValue as S;
            fn values<T>(list: &[mastik_core::Keyed<T>]) -> Vec<&T> {
                list.iter().map(|k| &k.value).collect()
            }
            let fits = match &style.value {
                S::Fills(f) => {
                    styles.fills.is_none() && !f.is_empty() && values(f) == values(&paint.fills)
                }
                S::Strokes(s) => {
                    styles.strokes.is_none() && !s.is_empty() && values(s) == values(&paint.strokes)
                }
                S::Effects(e) => {
                    styles.effects.is_none() && !e.is_empty() && values(e) == values(n.effects())
                }
                S::Text(t) => {
                    styles.text.is_none()
                        && match n.kind() {
                            NodeKind::Text { style: own, .. } => {
                                own.font_family == t.font_family
                                    && own.font_size == t.font_size
                                    && own.weight == t.weight
                                    && own.italic == t.italic
                                    && own.line_height == t.line_height
                                    && own.letter_spacing == t.letter_spacing
                            }
                            _ => false,
                        }
                }
                S::Layout(_) => false,
            };
            if fits {
                raw.add(
                    format!("s{}", style.id.0.to_wire()),
                    || format!("It is the style `{}`, written raw", style.name),
                    id,
                );
            }
        }
    }
    raw.by
        .into_iter()
        .map(|(key, (line, ids))| {
            let (code, instead) = match key.starts_with('s') {
                true => ("raw_style", "apply the style"),
                false => ("raw_value", "bind the variable"),
            };
            Finding::new(
                code,
                named(&ids),
                format!("{line} on {}.", layers(ids.len())),
            )
            .instead(instead.into())
        })
        .collect()
}

/// `near_duplicate_colours`: the colours painted under the roots that Colour
/// review groups as near-identical, which `merge_colours` makes one.
pub fn near_duplicate_colours(scope: &Scope<'_>) -> Vec<Finding> {
    use mastik_core::color_review::{Threshold, census, groups};
    let census = census(scope.doc, &scope.roots);
    groups(&census, Threshold::Close)
        .into_iter()
        .filter(|g| g.members.len() > 1)
        .map(|g| {
            let hexes: Vec<String> = g.members.iter().map(|m| hex(census[*m].color)).collect();
            let mut ids: Vec<NodeId> = Vec::new();
            for m in &g.members {
                for id in &census[*m].nodes {
                    if !ids.contains(id) {
                        ids.push(*id);
                    }
                }
            }
            let target = hex(census[g.default_target(&census)].color);
            Finding::new(
                "near_duplicate_colours",
                named(&ids),
                format!(
                    "{} colours are near-identical: {}, on {}.",
                    hexes.len(),
                    hexes.join(", "),
                    layers(ids.len())
                ),
            )
            .instead(format!("one colour, {target}: `merge_colours`"))
        })
        .collect()
}

/// `radius`: an even corner radius close to one more layers use but not it, 7
/// beside 8, which is rarely meant.
pub fn radii(scope: &Scope<'_>) -> Vec<Finding> {
    let mut by: BTreeMap<i64, Vec<NodeId>> = BTreeMap::new();
    for id in scope.own() {
        let Some(r) = scope.kind(id).and_then(NodeKind::corner_radii) else {
            continue;
        };
        let r = scope.doc.resolve_corners(id, r).unwrap_or(r);
        if r.top_left > 0.0
            && [r.top_right, r.bottom_right, r.bottom_left]
                .iter()
                .all(|c| (c - r.top_left).abs() < 1e-6)
        {
            by.entry((r.top_left * 100.0).round() as i64)
                .or_default()
                .push(id);
        }
    }
    let mut out = Vec::new();
    for (&v, ids) in &by {
        let r = v as f64 / 100.0;
        let near = by
            .iter()
            .filter(|(w, others)| **w != v && others.len() > ids.len())
            .map(|(w, others)| (*w as f64 / 100.0, others.len()))
            .filter(|(w, _)| (w - r).abs() <= 2.0_f64.max(0.2 * w))
            .max_by_key(|(_, n)| *n);
        if let Some((w, n)) = near {
            out.push(
                Finding::new(
                    "radius",
                    named(ids),
                    format!(
                        "A radius of {} on {}, beside {} on {}.",
                        num(r),
                        layers(ids.len()),
                        num(w),
                        layers(n)
                    ),
                )
                .instead(num(w)),
            );
        }
    }
    out
}

/// `low_contrast`: each shown text under the roots that fails `level` against
/// what is drawn behind it, in any mode: the contrast check's own measure.
pub fn low_contrast(scope: &Scope<'_>, level: contrast::Level) -> Vec<Finding> {
    let doc = scope.doc;
    let texts: Vec<NodeId> = scope
        .own()
        .filter(|id| matches!(scope.kind(*id), Some(NodeKind::Text { .. })))
        .collect();
    let mut pages: Vec<NodeId> = texts.iter().filter_map(|id| doc.page_of(*id)).collect();
    pages.sort();
    pages.dedup();
    let options = contrast::Options {
        level,
        ..Default::default()
    };
    let mut out = Vec::new();
    for page in pages {
        let report = contrast::check_with(doc, page, options, Some(&texts));
        for f in report.findings.iter().filter(|f| !f.passes()) {
            let mode = f
                .mode
                .as_ref()
                .map(|m| format!(" in {m}"))
                .unwrap_or_default();
            out.push(
                Finding::new(
                    "low_contrast",
                    vec![f.id],
                    format!(
                        "Its contrast is {}, where its size needs {}{mode}.",
                        level.format(f.ratio),
                        level.format_needs(f.needs)
                    ),
                )
                .instead("`fix_contrast`".into()),
            );
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use mastik_core::IdSource;

    /// A layer inserted at `x`, `y` in the layer `parent` labels.
    fn layer(parent: &str, label: &str, x: f64, y: f64, tree: Value) -> Value {
        let mut tree = tree;
        tree["transform"] = json!([1, 0, 0, 1, x, y]);
        json!({ "verb": "insert_subtree", "args": { "parent": parent, "tree": tree }, "as": label })
    }

    fn rect(w: f64, h: f64, fill: &str, radius: f64) -> Value {
        json!({ "kind": { "Rect": { "size": { "width": w, "height": h }, "corner_radii": radius } },
            "fills": [fill] })
    }

    fn frame(w: f64, h: f64, fill: &str) -> Value {
        json!({ "kind": { "Artboard": { "size": { "width": w, "height": h } } }, "fills": [fill] })
    }

    fn text(fill: &str) -> Value {
        json!({ "kind": { "Text": { "content": "Hello there", "sizing": "Auto",
            "style": { "font_size": 24 } } }, "fills": [fill] })
    }

    /// A page with one case of every check and a control beside most, and the
    /// layers each made, by label.
    fn fixture() -> (Document, std::collections::HashMap<String, NodeId>) {
        let mut ids = IdSource::new(0x5e7);
        let page = ids.mint();
        let mut doc = Document::new(page);
        let p = page.to_wire();
        let mut steps = vec![
            layer(&p, "column", 0.0, 0.0, frame(200.0, 400.0, "#ffffff")),
            // Gaps of 24, 22 and 24: the 22 breaks the rhythm.
            layer("@column", "c0", 0.0, 0.0, rect(100.0, 40.0, "#eeeeee", 8.0)),
            layer(
                "@column",
                "c1",
                0.0,
                64.0,
                rect(100.0, 40.0, "#eeeeee", 8.0),
            ),
            layer(
                "@column",
                "c2",
                0.0,
                126.0,
                rect(100.0, 40.0, "#eeeeee", 8.0),
            ),
            layer(
                "@column",
                "c3",
                0.0,
                190.0,
                rect(100.0, 40.0, "#eeeeee", 8.0),
            ),
            layer(&p, "pair", 300.0, 0.0, frame(200.0, 200.0, "#ffffff")),
            layer("@pair", "a", 0.0, 0.0, rect(100.0, 100.0, "#dddddd", 7.0)),
            layer("@pair", "b", 50.0, 50.0, rect(100.0, 100.0, "#dddddd", 7.0)),
            layer(&p, "row", 0.0, 500.0, frame(300.0, 100.0, "#ffffff")),
            json!({ "verb": "set_display", "args": { "nodes": ["@row"],
                "display": { "Flex": { "direction": "Row", "gap": 15 } } } }),
            layer("@row", "brand", 0.0, 0.0, rect(40.0, 40.0, "#3366cc", 0.0)),
            layer("@row", "near", 0.0, 0.0, rect(40.0, 40.0, "#3467cc", 0.0)),
            json!({ "verb": "add_variable", "args": { "kind": "color", "name": "Brand",
                "value": { "color": "#3366cc" } } }),
            layer(&p, "busy", 600.0, 0.0, frame(200.0, 100.0, "#ffffff")),
        ];
        for i in 0..20 {
            let stripe = rect(5.0, 100.0, "#000000", 0.0);
            steps.push(layer(
                "@busy",
                &format!("s{i}"),
                f64::from(i) * 10.0,
                0.0,
                stripe,
            ));
        }
        steps.extend([
            layer("@busy", "on_stripes", 10.0, 30.0, text("#ff0000")),
            layer(&p, "calm", 600.0, 200.0, frame(200.0, 100.0, "#ffffff")),
            layer("@calm", "readable", 10.0, 30.0, text("#111111")),
            layer(&p, "pale", 600.0, 400.0, frame(200.0, 100.0, "#ffffff")),
            layer("@pale", "faint", 10.0, 30.0, text("#eeeeee")),
        ]);
        let mut labels = std::collections::HashMap::new();
        let made = mastik_core::verbs::run_script(
            &mut doc,
            &mut ids,
            mastik_core::verbs::Lent::default(),
            &Value::Array(steps.clone()),
        )
        .expect("the fixture builds");
        for (step, made) in steps.iter().zip(made) {
            if let (Some(label), Some(first)) = (step["as"].as_str(), made.first()) {
                labels.insert(label.to_owned(), NodeId::from_wire(first).unwrap());
            }
        }
        labels.insert("page".into(), page);
        (doc, labels)
    }

    /// Every check finds its case and passes its control by.
    ///
    /// flip: `spacing` taking a rhythm's gap only within 0.5 of the others →
    /// fails at "rhythm"; `busy_grounds` judging busy at a half share → fails at
    /// "busy_ground". The fixture has no radius far from 8, so `radii`'s
    /// nearness test is not pinned here.
    #[test]
    fn every_check_finds_its_case() {
        let (doc, l) = fixture();
        let res = Resolved::rebuild(&doc);
        let scope = Scope::new(&doc, &res, &[l["page"]]);
        let found = review(&scope, Options::default());
        let with =
            |code: &str| -> Vec<&Finding> { found.iter().filter(|f| f.code == code).collect() };
        let names =
            |code: &str| -> Vec<Vec<NodeId>> { with(code).iter().map(|f| f.ids.clone()).collect() };
        assert_eq!(names("rhythm"), vec![vec![l["c1"], l["c2"]]], "rhythm");
        assert_eq!(names("overlap"), vec![vec![l["a"], l["b"]]], "overlap");
        assert!(
            names("off_scale").contains(&vec![l["row"]]),
            "the flex gap of 15: {found:?}"
        );
        assert!(
            with("raw_value").iter().any(|f| f.ids == vec![l["brand"]]),
            "the brand colour written raw: {found:?}"
        );
        assert!(
            !with("near_duplicate_colours").is_empty(),
            "#3366cc beside #3467cc"
        );
        assert_eq!(names("radius"), vec![vec![l["a"], l["b"]]], "7 beside 8");
        assert_eq!(
            names("busy_ground"),
            vec![vec![l["on_stripes"]]],
            "busy_ground"
        );
        let pale = names("low_contrast");
        assert!(pale.contains(&vec![l["faint"]]), "low_contrast: {found:?}");
        assert!(!pale.contains(&vec![l["readable"]]), "dark on white reads");
        let answer = to_json(&scope, &found);
        assert!(answer["checked"].as_u64().unwrap() > 20);
    }
}
