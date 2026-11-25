//! `review`: the design checks `check_layout` leaves out, over the shown layers
//! under a page or some layers: layers overlapping, text on a busy ground,
//! spacing off its rhythm or off the scale, raw values where a variable or a
//! style fits, near-duplicate colours, radii that nearly agree, low contrast,
//! and prototype targets that are gone. Each check is a function of its own
//! over a [`Scope`], so the MCP `review` read and `mastik check review` run the
//! same ones. [`health`] reads
//! some of them beside its coverage count, for the *System health* card, MCP's
//! `health` and `mastik check health` alike.

use crate::context::{own_box, shown_under};
use crate::contrast;
use mastik_core::kurbo::Rect;
use mastik_core::peniko::Color;
use mastik_core::variable::{Field, Value as VarValue};
use mastik_core::{Brush, Document, NodeId, NodeKind, Operation, Resolved, Transaction};
use mastik_render::{ImageStore, RenderOverrides, VelloCpuRenderer, Viewport};
use serde_json::{Value, json};
use std::collections::{BTreeMap, HashSet};

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
    out.extend(dangling_targets(scope));
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

/// Whether `id` is a boolean, or a group inside one through groups: its
/// children are operands, which a boolean reads a group's union of, and whose
/// shape the boolean paints.
pub(crate) fn makes_a_boolean(doc: &Document, id: NodeId) -> bool {
    let mut at = doc.get(id);
    while let Some(n) = at {
        match n.kind() {
            NodeKind::Boolean { .. } => return true,
            NodeKind::Group => at = n.parent().and_then(|p| doc.get(p)),
            _ => return false,
        }
    }
    false
}

/// `overlap`: two siblings whose boxes share more than a sliver, neither holding
/// the other. Texts are `check_layout`'s (`text_overlap`, `text_covered`); a
/// background holds what lies on it and is passed over, and a boolean's operands
/// (a group operand's members too) overlap to make its shape.
pub fn overlaps(scope: &Scope<'_>) -> Vec<Finding> {
    let mut out = Vec::new();
    for &parent in &scope.layers {
        if makes_a_boolean(scope.doc, parent) {
            continue;
        }
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
        // A boolean's operands are placed to make its shape, not laid out.
        if makes_a_boolean(doc, parent) {
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
    /// The colour a variable's fill or stroke key matched, as RGBA bytes.
    colour: BTreeMap<String, [u8; 4]>,
    /// Each key's layers as a set, so a layer is listed once.
    seen: HashSet<(String, NodeId)>,
}

impl Raw {
    fn add(&mut self, key: String, line: impl FnOnce() -> String, id: NodeId) {
        if self.seen.insert((key.clone(), id)) {
            self.by
                .entry(key)
                .or_insert_with(|| (line(), Vec::new()))
                .1
                .push(id);
        }
    }
}

/// `raw_value` and `raw_style`: a value written raw where a variable or a style
/// of the document holds exactly it, and the layer takes neither. A solid fill
/// or stroke against the colour variables that may bind it, a rectangle's or
/// frame's even radius and a layout's gaps and padding against the number
/// variables that may bind them (the best fit, [`fitting`]), and a layer's
/// whole fill, stroke or effect list, or a text's type, against the styles.
pub fn raw_values(scope: &Scope<'_>) -> Vec<Finding> {
    use mastik_core::color_review::{census, roots};
    // `merge_colours` with a colour as `into` binds to the one swatch of exactly
    // that colour bound in its scope, every page: so only when that swatch is
    // the variable's, at the colour the layers hold raw.
    let used = census(scope.doc, &roots(scope.doc, None));
    let mergeable = |var: mastik_core::VarId, raw: [u8; 4]| {
        let mut bound = used
            .iter()
            .filter(|o| !o.is_raw() && o.color.to_rgba8().to_u8_array() == raw);
        bound.next().is_some_and(|s| s.var == Some(var)) && bound.next().is_none()
    };
    let raw = raw_matches(scope);
    raw.by
        .into_iter()
        .map(|(key, (line, ids))| {
            // A colour's every raw use binds in one call where a merge into the
            // colour its variable carries binds to it.
            let colour = ["fill", "stroke"].iter().any(|what| {
                without_colour(&key)
                    .strip_prefix('v')
                    .and_then(|k| k.strip_suffix(what))
                    .and_then(NodeId::from_wire)
                    .zip(raw.colour.get(&key))
                    .is_some_and(|(id, c)| mergeable(mastik_core::VarId(id), *c))
            });
            let (code, instead) = match (key.starts_with('s'), colour) {
                (true, _) => ("raw_style", "apply the style"),
                (false, true) => (
                    "raw_value",
                    "bind the variable; `merge_colours` with this colour as both `colours` and `into` binds every raw use at once",
                ),
                (false, false) => ("raw_value", "bind the variable"),
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

/// The number variable worth `x` that may bind `field` and fits it best: one
/// scoped to it, then one whose name says the field (`radius/card` for a corner,
/// `space/md` for a gap), then any, since an unscoped variable binds every
/// number field. An unscoped one whose name says the other field
/// (`radius/card` for a gap) is no fit: its value matching is a coincidence.
/// A scope admitting both fields counts as none.
fn fitting<'v>(
    numbers: &[(&'v mastik_core::Variable, f64)],
    x: f64,
    field: Field,
) -> Option<&'v mastik_core::Variable> {
    const CORNER: &[&str] = &["radius", "radii", "corner", "round"];
    const SPACING: &[&str] = &["space", "spacing", "gap", "padding", "gutter", "inset"];
    let (words, other, other_field): (&[&str], &[&str], Option<Field>) = match field {
        Field::Corner => (CORNER, SPACING, Some(Field::Spacing)),
        Field::Spacing => (SPACING, CORNER, Some(Field::Corner)),
        _ => (&[], &[], None),
    };
    // A word starting with a root says it too: `spacer`, `rounded`, `gaps`.
    let names = |v: &mastik_core::Variable, words: &[&str]| {
        name_words(&v.name)
            .iter()
            .any(|part| words.iter().any(|w| part.starts_with(w)))
    };
    // A scope says what a variable is for only where it keeps the other field
    // out: one admitting every number field, as a token import writes, says nothing.
    let scoped =
        |v: &mastik_core::Variable| v.scope.is_some() && other_field.is_none_or(|f| !v.binds(f));
    numbers
        .iter()
        .filter(|(v, y)| *y == x && v.binds(field))
        .map(|(v, _)| *v)
        .filter(|v| scoped(v) || names(v, words) || !names(v, other))
        .min_by_key(|v| (!scoped(v), !names(v, words)))
}

/// The words of a variable's name, lowercased: split at `/ - _ .`, spaces and
/// a camel hump (`borderRadius`), so `round` starts no word of `background`.
fn name_words(name: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut word = String::new();
    let mut after_lower = false;
    for ch in name.chars() {
        let cut = matches!(ch, '/' | '-' | '_' | '.' | ' ');
        if (cut || (ch.is_uppercase() && after_lower)) && !word.is_empty() {
            out.push(std::mem::take(&mut word));
        }
        after_lower = !cut && (ch.is_lowercase() || ch.is_ascii_digit());
        if !cut {
            word.extend(ch.to_lowercase());
        }
    }
    if !word.is_empty() {
        out.push(word);
    }
    out
}

/// A [`raw_matches`] key less the `#rrggbb` a colour match ends in.
fn without_colour(key: &str) -> &str {
    key.split_once('#').map_or(key, |(k, _)| k)
}

/// [`raw_values`]' matches with every layer each names: keyed `s<style>` for a
/// style, `v<variable><what>` for a variable, a colour's followed by the colour.
fn raw_matches(scope: &Scope<'_>) -> Raw {
    let doc = scope.doc;
    let vars = doc.variables();
    // Each variable's colour or number in the modes `set` puts in force: a layer
    // matches what its variable draws there, which binding it would keep.
    type Values<'v> = (
        Vec<(&'v mastik_core::Variable, [u8; 4])>,
        Vec<(&'v mastik_core::Variable, f64)>,
    );
    let values_in = |set: &[mastik_core::variable::ModeId]| -> Values<'_> {
        let mut out: Values<'_> = (Vec::new(), Vec::new());
        for v in vars.local() {
            match vars.resolve_at(v.id, set) {
                Some(VarValue::Color(c)) => out.0.push((v, c.to_rgba8().to_u8_array())),
                Some(VarValue::Number(n)) => out.1.push((v, n)),
                _ => {}
            }
        }
        out
    };
    let unmoded = values_in(&[]);
    let mut raw = Raw::default();
    for id in scope.own() {
        let Some(n) = doc.get(id) else { continue };
        let modes = doc.modes_at(id);
        let moded = (!modes.is_empty()).then(|| values_in(&modes));
        let (colours, numbers) = moded.as_ref().unwrap_or(&unmoded);
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
                    // A variable matches other colours in other modes; each is
                    // its own finding, its line and merge that colour's.
                    let key = format!("v{}{what}{}", v.id.0.to_wire(), hex(c));
                    raw.colour.entry(key.clone()).or_insert(bytes);
                    raw.add(
                        key,
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
            && let Some(v) = fitting(numbers, r.top_left, Field::Corner)
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
                if let Some(v) = fitting(numbers, x, Field::Spacing) {
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
                            // The whole type, as applying the style writes it:
                            // everything but the colour and the language.
                            NodeKind::Text { style: own, .. } => {
                                mastik_core::style::type_part(own)
                                    == mastik_core::style::type_part(t)
                            }
                            _ => false,
                        }
                }
                S::Layout(_) => false,
                // A chart at the style's whole look, as applying it writes it.
                S::Chart(o) => {
                    styles.chart.is_none()
                        && n.chart().is_some_and(|c| c.options.look() == o.look())
                }
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
    raw
}

/// `near_duplicate_colours`: the colours painted under the roots that Colour
/// review groups as near-identical, which `merge_colours` makes one.
pub fn near_duplicate_colours(scope: &Scope<'_>) -> Vec<Finding> {
    use mastik_core::color_review::{Threshold, census, groups, would_change};
    let census = census(scope.doc, &scope.roots);
    groups(&census, Threshold::Close)
        .into_iter()
        .filter(|g| g.members.len() > 1)
        .filter(|g| would_change(scope.doc, &scope.roots, &census, g))
        .map(|g| {
            // A bound member is named by its variable: beside it, a raw colour of
            // the same hex would otherwise read as a colour listed twice.
            let label = |m: usize| {
                let swatch = &census[m];
                match swatch.var.and_then(|v| scope.doc.variables().get(v)) {
                    Some(v) => format!("{} (`{}`)", hex(swatch.color), v.name),
                    None => hex(swatch.color),
                }
            };
            let hexes: Vec<String> = g.members.iter().map(|m| label(*m)).collect();
            // The layers a merge changes: the raw members' but the target's, never
            // a layer already bound to it.
            let target = g.members[g.default_target(&census)];
            let mut ids: Vec<NodeId> = Vec::new();
            let mut seen: HashSet<NodeId> = HashSet::new();
            for m in g
                .members
                .iter()
                .filter(|m| **m != target && census[**m].is_raw())
            {
                ids.extend(census[*m].nodes.iter().filter(|id| seen.insert(**id)));
            }
            let target = label(target);
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

/// What the prototype on layer `id` names that the document no longer holds,
/// each kind once, in the order met: its reactions' layers, variables, scripts,
/// modes, tables and columns, a scroll effect's frame and an input's variable.
/// The targets are read through the model's own enumerations (`layer_targets`,
/// `prototype::refs`, `modes_set`, `Action::tables`), so a new action kind
/// that names something is checked once it is enumerated there.
fn gone_targets(doc: &Document, id: NodeId) -> Vec<&'static str> {
    use mastik_core::prototype::{self, Action, Expr, expr::Ref};
    let Some(n) = doc.get(id) else {
        return Vec::new();
    };
    let mut gone: Vec<&'static str> = Vec::new();
    let mut say = |what: &'static str| {
        if !gone.contains(&what) {
            gone.push(what);
        }
    };
    let layer_gone = |t: NodeId| doc.get(prototype::resolve_target(doc, id, t)).is_none();
    let column_gone = |table: mastik_core::list::DataSetId, c: mastik_core::chart::ColumnId| {
        doc.data_set(table)
            .is_some_and(|d| d.table.index_of(c).is_none())
    };
    // The columns an action's own cells and expressions read; `row` is the
    // table `{row.…}` reads.
    fn columns(
        a: &Action,
        out: &mut Vec<(mastik_core::list::DataSetId, mastik_core::chart::ColumnId)>,
    ) {
        let row = a.table();
        let cells: &[prototype::CellWrite] = match a {
            Action::AddRow { cells, .. } | Action::EditRow { cells, .. } => cells,
            _ => &[],
        };
        if let Some(t) = row {
            out.extend(cells.iter().map(|c| (t, c.column)));
        }
        let exprs: Vec<&Expr> = a.exprs();
        for r in exprs.iter().flat_map(|e| e.refs()) {
            match r {
                Ref::Cell(t, _, c) | Ref::Total(t, c, _) => out.push((t, c)),
                Ref::Row(c) => out.extend(row.map(|t| (t, c))),
                Ref::Var(_) | Ref::Rows(_) => {}
            }
        }
        for k in a.nested() {
            columns(&k.value, out);
        }
    }
    let reactions = n.reactions();
    for r in reactions {
        let trigger = &r.value.trigger;
        if trigger.layer_target().is_some_and(layer_gone) {
            say("a frame");
        }
        if trigger.table().is_some_and(|t| doc.data_set(t).is_none()) {
            say("a table");
        }
        for k in &r.value.actions {
            let a = &k.value;
            if a.layer_targets().into_iter().any(layer_gone) {
                say("a layer");
            }
            if a.tables().into_iter().any(|t| doc.data_set(t).is_none()) {
                say("a table");
            }
            let mut read = Vec::new();
            columns(a, &mut read);
            if read.into_iter().any(|(t, c)| column_gone(t, c)) {
                say("a column");
            }
        }
    }
    let vars = doc.variables();
    let (used, scripts) = prototype::refs(reactions);
    if used.iter().any(|v| vars.get(*v).is_none()) {
        say("a variable");
    }
    if scripts.iter().any(|s| doc.script(*s).is_none()) {
        say("a script");
    }
    let mode_gone = |m: &mastik_core::ModeId| {
        vars.collection_of_mode(*m).is_none() && vars.kit_mode_of(*m).is_none()
    };
    if prototype::modes_set(reactions).iter().any(mode_gone) {
        say("a mode");
    }
    let play = n.play();
    if play.layer_target().is_some_and(layer_gone) {
        say("a frame");
    }
    if play
        .input
        .as_ref()
        .is_some_and(|i| vars.get(i.var).is_none())
    {
        say("a variable");
    }
    gone
}

/// `dangling_target`: a layer whose prototype names a layer, variable, script,
/// mode, table or column that is gone, and a page flow whose start frame is.
/// Targets dangle by design, so undo mends them, and play skips what needs
/// one; this says where. A copy's reactions are its main's, said there.
pub fn dangling_targets(scope: &Scope<'_>) -> Vec<Finding> {
    let doc = scope.doc;
    let mut out = Vec::new();
    for &id in &scope.layers {
        if let Some(page) = doc.page(id) {
            for f in &page.flows {
                let start = f.value.start;
                let is_frame = doc
                    .get(start)
                    .is_some_and(|n| matches!(n.kind(), NodeKind::Artboard { .. }));
                if !is_frame {
                    out.push(Finding::new(
                        "dangling_target",
                        vec![id],
                        format!(
                            "The flow “{}” starts at a frame that is gone, so it cannot play.",
                            f.value.name
                        ),
                    ));
                }
            }
            continue;
        }
        if is_copy(doc, id) {
            continue;
        }
        let gone = gone_targets(doc, id);
        let said = match gone.as_slice() {
            [] => continue,
            [one] => format!("{one} that is gone; play skips what needs it"),
            [rest @ .., last] => format!(
                "{} and {last} that are gone; play skips what needs them",
                rest.join(", ")
            ),
        };
        out.push(Finding::new(
            "dangling_target",
            vec![id],
            format!("Its prototype names {said}."),
        ));
    }
    out
}

/// How many uses of one kind are bound to the system, of how many, and the
/// layers holding the raw ones.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Coverage {
    pub bound: usize,
    pub total: usize,
    pub raw: Vec<NodeId>,
}

impl Coverage {
    fn count(&mut self, id: NodeId, bound: bool) {
        self.total += 1;
        match bound {
            true => self.bound += 1,
            false if !self.raw.contains(&id) => self.raw.push(id),
            false => {}
        }
    }

    /// The bound share as a whole percentage, rounded down so 99.6% never
    /// reads as all of it; `None` when there is nothing to count.
    pub fn percent(&self) -> Option<usize> {
        (self.total > 0).then(|| self.bound * 100 / self.total)
    }

    fn to_json(&self) -> Value {
        json!({ "bound": self.bound, "of": self.total })
    }
}

/// A list of the things one *To tidy* row counts, with every layer it names.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Tidy {
    /// How many there are, in the row's own unit.
    pub count: usize,
    pub ids: Vec<NodeId>,
}

impl Tidy {
    fn of(ids: Vec<NodeId>) -> Self {
        Self {
            count: ids.len(),
            ids,
        }
    }
}

/// *System health*: how much of a scope's paint, type and spacing is bound to
/// variables and styles, and what is left to tidy — the card, MCP `health` and
/// `mastik check health` all read this one function, so their numbers agree.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Health {
    /// Every visible fill and stroke, bound to a colour variable or its whole
    /// list to a style; and every shadow's colour, which no variable binds,
    /// bound when the effect list takes a style.
    pub paint: Coverage,
    /// Every text layer, bound when it takes a text style.
    pub text: Coverage,
    /// Every non-zero gap, padding and corner radius, bound to a number variable.
    pub spacing: Coverage,
    /// Layers whose raw value is exactly a variable or a style; `count` is the
    /// layers, and the commonest match leads `matches`.
    pub raw_match: Tidy,
    /// The matches behind [`Self::raw_match`], commonest first: the variable or
    /// style each names, and the layers.
    pub matches: Vec<(Match, Vec<NodeId>)>,
    /// Groups of near-identical colours; `count` is the groups.
    pub near_duplicates: Tidy,
    /// Layers with a gap, padding or radius off the spacing scale, unbound.
    pub off_scale: Tidy,
    /// Layers detached from a component (`Node::detached_from`).
    pub detached: Tidy,
    /// Main components in the scope with no instance anywhere; a component
    /// set counts once, when none of its variants has one.
    pub unused_components: Tidy,
    /// Layers whose prototype names something gone ([`dangling_targets`]); a
    /// flow's start is said by the review alone, a page being no layer to select.
    pub dangling: Tidy,
    /// The document's own variables nothing reads, whatever the scope.
    pub unused_variables: Vec<mastik_core::VarId>,
    /// The document's own styles no layer takes, whatever the scope.
    pub unused_styles: Vec<mastik_core::style::StyleId>,
    /// The layers looked at, and the pages they are on.
    pub checked: usize,
    pub pages: usize,
}

/// What a raw value is exactly: a variable or a style of the document.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Match {
    Variable(mastik_core::VarId),
    Style(mastik_core::style::StyleId),
}

impl Health {
    /// Every bound use of the three kinds against every use, as a whole
    /// percentage; `None` for a scope with nothing to count.
    pub fn overall(&self) -> Option<usize> {
        let bound = self.paint.bound + self.text.bound + self.spacing.bound;
        let total = self.paint.total + self.text.total + self.spacing.total;
        (total > 0).then(|| bound * 100 / total)
    }

    /// How many things the *To tidy* rows count together.
    pub fn to_tidy(&self) -> usize {
        self.raw_match.count
            + self.near_duplicates.count
            + self.off_scale.count
            + self.detached.count
            + self.unused_components.count
            + self.dangling.count
            + self.unused_variables.len()
            + self.unused_styles.len()
    }

    /// As MCP's `health` answers it: the counts, and each row's layer ids, at
    /// most [`LISTED`] of them.
    pub fn to_json(&self, doc: &Document) -> Value {
        let ids = |ids: &[NodeId]| -> Vec<String> {
            ids.iter().take(LISTED).map(|i| i.to_wire()).collect()
        };
        let tidy = |t: &Tidy| json!({ "count": t.count, "ids": ids(&t.ids) });
        let var_name = |v: mastik_core::VarId| {
            doc.variables()
                .get(v)
                .map(|v| v.name.clone())
                .unwrap_or_default()
        };
        json!({
            "bound": {
                "paint": self.paint.to_json(),
                "type": self.text.to_json(),
                "spacing": self.spacing.to_json(),
                "percent": self.overall(),
            },
            "raw_match": tidy(&self.raw_match),
            "near_duplicates": tidy(&self.near_duplicates),
            "off_scale": tidy(&self.off_scale),
            "detached": tidy(&self.detached),
            "dangling": tidy(&self.dangling),
            "unused": {
                "components": tidy(&self.unused_components),
                "variables": self.unused_variables.iter().map(|v| var_name(*v)).collect::<Vec<_>>(),
                "styles": self.unused_styles.iter().filter_map(|s| doc.styles().get(*s)).map(|s| s.name.clone()).collect::<Vec<_>>(),
            },
            "checked": self.checked,
        })
    }

    /// As `mastik check health` prints it: one line per reading, then the sum.
    pub fn to_text(&self) -> String {
        let cov = |c: &Coverage| match c.percent() {
            Some(p) => format!("{p}% ({}/{})", c.bound, c.total),
            None => "none".to_string(),
        };
        let mut out = format!(
            "bound      paint {}  type {}  spacing {}\n",
            cov(&self.paint),
            cov(&self.text),
            cov(&self.spacing)
        );
        out.push_str(&format!("raw_match  {}\n", self.raw_match.count));
        out.push_str(&format!("near_dup   {}\n", self.near_duplicates.count));
        out.push_str(&format!("off_scale  {}\n", self.off_scale.count));
        out.push_str(&format!("detached   {}\n", self.detached.count));
        out.push_str(&format!("dangling   {}\n", self.dangling.count));
        out.push_str(&format!(
            "unused     components {}  variables {}  styles {}\n",
            self.unused_components.count,
            self.unused_variables.len(),
            self.unused_styles.len()
        ));
        let overall = self
            .overall()
            .map_or_else(|| "nothing to count".to_string(), |p| format!("{p}% bound"));
        out.push_str(&format!("{overall} · {} to tidy\n", self.to_tidy()));
        out
    }
}

/// The roots of an *All pages* reading: every page but drafts, `current` kept
/// even when a draft, as Clean up gathers them.
pub fn every_page(doc: &Document, current: NodeId) -> Vec<NodeId> {
    doc.pages()
        .iter()
        .filter(|p| !p.draft || p.id == current)
        .map(|p| p.id)
        .collect()
}

/// Whether `id` is a copy, whose values are its main's choices, counted there.
fn is_copy(doc: &Document, id: NodeId) -> bool {
    doc.get(id).is_some_and(|n| n.link().is_some())
}

/// [`Health::overall`] alone for `scope`: the coverage pass without the checks,
/// for a summary read more often than the card is open.
pub fn overall(scope: &Scope<'_>) -> Option<usize> {
    let mut h = Health::default();
    counts(scope, 4.0, &mut h);
    h.overall()
}

/// The coverage pass over `scope`'s own layers, copies left out: the three
/// [`Coverage`]s and the detached layers into `h`. Answers the layers whose
/// own spacing or radius is off the scale, unbound.
fn counts(scope: &Scope<'_>, step: f64, h: &mut Health) -> Vec<NodeId> {
    let doc = scope.doc;
    let own: Vec<NodeId> = scope.own().filter(|id| !is_copy(doc, *id)).collect();
    let mut off_scale: Vec<NodeId> = Vec::new();
    for &id in &own {
        let Some(n) = doc.get(id) else { continue };
        let styles = n.styles();
        let paint = n.paint();
        let counted =
            |b: &mastik_core::variable::BrushSource| !matches!(b.literal(), Some(Brush::Image(_)));
        for f in paint
            .fills
            .iter()
            .filter(|f| f.visible && counted(&f.brush))
        {
            h.paint
                .count(id, styles.fills.is_some() || f.brush.var().is_some());
        }
        for s in paint
            .strokes
            .iter()
            .filter(|s| s.visible && counted(&s.brush))
        {
            h.paint
                .count(id, styles.strokes.is_some() || s.brush.var().is_some());
        }
        for e in n.effects().iter().filter(|e| e.visible) {
            if matches!(
                e.kind,
                mastik_core::effect::EffectKind::DropShadow(_)
                    | mastik_core::effect::EffectKind::InnerShadow(_)
            ) {
                h.paint.count(id, styles.effects.is_some());
            }
        }
        if matches!(n.kind(), NodeKind::Text { .. }) {
            h.text.count(id, styles.text.is_some());
        }
        let mut off = false;
        if let Some(d) = doc.resolved_display(id) {
            let bound = n.spacing_vars();
            for (k, v) in d.spacing().into_iter().enumerate() {
                if v > 0.0 {
                    h.spacing.count(id, bound[k].is_some());
                    off |= bound[k].is_none() && !on_step(v, step);
                }
            }
        }
        if matches!(n.kind(), NodeKind::Rect { .. } | NodeKind::Artboard { .. })
            && let Some(r) = scope.kind(id).and_then(NodeKind::corner_radii)
        {
            let vars = n.corner_vars();
            let corners = [r.top_left, r.top_right, r.bottom_right, r.bottom_left];
            for (k, c) in corners.into_iter().enumerate() {
                if c > 0.0 {
                    h.spacing.count(id, vars[k].is_some());
                    off |= vars[k].is_none() && !on_step(c, step);
                }
            }
        }
        if off {
            off_scale.push(id);
        }
        if n.detached_from().is_some() {
            h.detached.ids.push(id);
        }
    }
    off_scale
}

/// [`Health`] for `scope`, the spacing scale's step being `step`.
pub fn health(scope: &Scope<'_>, step: f64) -> Health {
    let doc = scope.doc;
    let mut h = Health::default();
    let mut off_scale = counts(scope, step, &mut h);
    let mut seen: HashSet<NodeId> = off_scale.iter().copied().collect();
    // Gaps between layers placed by hand, the spacing check's own reading.
    for f in spacing(scope, step)
        .into_iter()
        .filter(|f| f.code == "off_scale")
    {
        for id in f.ids {
            if !is_copy(doc, id) && seen.insert(id) {
                off_scale.push(id);
            }
        }
    }
    h.off_scale = Tidy::of(off_scale);
    h.detached.count = h.detached.ids.len();

    let mut layers: Vec<NodeId> = Vec::new();
    let mut seen: HashSet<NodeId> = HashSet::new();
    let mut matches: Vec<(Match, Vec<NodeId>)> = Vec::new();
    for (key, (_, ids)) in raw_matches(scope).by {
        let ids: Vec<NodeId> = ids.into_iter().filter(|i| !is_copy(doc, *i)).collect();
        if ids.is_empty() {
            continue;
        }
        let Some(m) = match_of(doc, &key) else {
            continue;
        };
        layers.extend(ids.iter().filter(|id| seen.insert(**id)));
        // One row a variable, though each colour it matches is its own key.
        match matches.iter_mut().find(|(held, _)| *held == m) {
            Some((_, held)) => {
                for id in ids {
                    if !held.contains(&id) {
                        held.push(id);
                    }
                }
            }
            None => matches.push((m, ids)),
        }
    }
    matches.sort_by_key(|(_, ids)| std::cmp::Reverse(ids.len()));
    h.raw_match = Tidy::of(layers);
    h.matches = matches;

    let near = near_duplicate_colours(scope);
    h.near_duplicates = Tidy {
        count: near.len(),
        ids: near.into_iter().flat_map(|f| f.ids).collect(),
    };

    h.unused_components = Tidy::of(unused_components(scope));
    h.dangling = Tidy::of(
        dangling_targets(scope)
            .into_iter()
            .flat_map(|f| f.ids)
            .filter(|id| !doc.is_page(*id))
            .collect(),
    );
    h.unused_variables = mastik_core::tokens::unused_variables(doc);
    h.unused_styles = mastik_core::style::unused(doc);
    h.checked = scope.own().count();
    let mut pages: Vec<NodeId> = scope.own().filter_map(|id| doc.page_of(id)).collect();
    pages.sort();
    pages.dedup();
    h.pages = pages.len();
    h
}

/// The variable or style a [`raw_matches`] key names.
fn match_of(doc: &Document, key: &str) -> Option<Match> {
    let wire = NodeId::from_wire;
    match key.strip_prefix('s') {
        Some(rest) => {
            let id = wire(rest)?;
            doc.styles()
                .iter()
                .find(|s| s.id.0 == id)
                .map(|s| Match::Style(s.id))
        }
        None => {
            let rest = without_colour(key).strip_prefix('v')?;
            let id = ["fill", "stroke", "radius", "spacing"]
                .iter()
                .find_map(|what| rest.strip_suffix(what))
                .and_then(wire)?;
            doc.variables()
                .iter()
                .find(|v| v.id.0 == id)
                .map(|v| Match::Variable(v.id))
        }
    }
}

/// The scope's main components no instance anywhere shows, and the component
/// sets none of whose variants has one; a kit's mains are the kit's.
fn unused_components(scope: &Scope<'_>) -> Vec<NodeId> {
    let doc = scope.doc;
    let used = mastik_core::component::instances_by_main(doc);
    let has = |m: NodeId| used.get(&m).is_some_and(|i| !i.is_empty());
    let mut out = Vec::new();
    for id in scope.own() {
        let Some(n) = doc.get(id) else { continue };
        if doc.in_vault(id) {
            continue;
        }
        if n.set().is_some() {
            let variants: Vec<NodeId> = n
                .children()
                .iter()
                .copied()
                .filter(|c| doc.get(*c).is_some_and(|c| c.component()))
                .collect();
            if !variants.is_empty() && !variants.iter().any(|v| has(*v)) {
                out.push(id);
            }
            continue;
        }
        let in_set = n
            .parent()
            .and_then(|p| doc.get(p))
            .is_some_and(|p| p.set().is_some());
        if n.component() && !in_set && !has(id) {
            out.push(id);
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
            // A boolean's operands overlap to make its shape: no finding.
            layer(
                &p,
                "mark",
                300.0,
                300.0,
                json!({ "kind": { "Boolean": { "op": "Union" } }, "fills": ["#dddddd"] }),
            ),
            layer("@mark", "m1", 0.0, 0.0, rect(100.0, 100.0, "#dddddd", 0.0)),
            layer(
                "@mark",
                "m2",
                50.0,
                50.0,
                rect(100.0, 100.0, "#dddddd", 0.0),
            ),
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
    /// "busy_ground"; `overlaps` reading a boolean's operands → fails at
    /// "overlap". The fixture has no radius far from 8, so `radii`'s
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

    /// `health` counts uses bound against raw, a copy's at its main only, and
    /// finds the unused main, the unused variable, the detached layer and the
    /// raw match.
    ///
    /// flip: `counts` counting a copy's paint → fails at "paint"; `unused_components`
    /// ignoring instances → fails at "the used main"; `unused_variables` reading
    /// no layer → fails at "Brand is used".
    #[test]
    fn health_counts_bound_uses_and_what_to_tidy() {
        let mut ids = IdSource::new(0x4ea);
        let page = ids.mint();
        let mut doc = Document::new(page);
        let p = page.to_wire();
        let steps = vec![
            json!({ "verb": "add_variable", "args": { "kind": "color", "name": "Brand",
                "value": { "color": "#3366cc" } }, "as": "brand_var" }),
            json!({ "verb": "add_variable", "args": { "kind": "number", "name": "Idle",
                "value": { "number": 3 } }, "as": "idle_var" }),
            layer(&p, "bound", 0.0, 0.0, rect(40.0, 40.0, "#3366cc", 0.0)),
            json!({ "verb": "bind_variable", "args": { "nodes": ["@bound"],
                "field": { "Fill": 0 }, "variable": "@brand_var" } }),
            layer(&p, "raw", 100.0, 0.0, rect(40.0, 40.0, "#3366cc", 0.0)),
            layer(&p, "main", 200.0, 0.0, rect(40.0, 40.0, "#ff0000", 0.0)),
            json!({ "verb": "make_component", "args": { "nodes": ["@main"] }, "as": "used" }),
            json!({ "verb": "create_instance", "args": { "main": "@used", "parent": p,
                "x": 300, "y": 0 } }),
            json!({ "verb": "create_instance", "args": { "main": "@used", "parent": p,
                "x": 400, "y": 0 }, "as": "gone" }),
            json!({ "verb": "detach_instance", "args": { "nodes": ["@gone"] } }),
            layer(&p, "alone", 500.0, 0.0, rect(40.0, 40.0, "#00ff00", 0.0)),
            json!({ "verb": "make_component", "args": { "nodes": ["@alone"] }, "as": "lonely" }),
        ];
        let made = mastik_core::verbs::run_script(
            &mut doc,
            &mut ids,
            mastik_core::verbs::Lent::default(),
            &Value::Array(steps.clone()),
        )
        .expect("the fixture builds");
        let l: std::collections::HashMap<&str, NodeId> = steps
            .iter()
            .zip(made)
            .filter_map(|(s, m)| Some((s["as"].as_str()?, NodeId::from_wire(m.first()?)?)))
            .collect();
        let res = Resolved::rebuild(&doc);
        let scope = Scope::new(&doc, &res, &[page]);
        let h = health(&scope, 4.0);
        let gone = l["gone"];
        assert_eq!(h.detached.ids, vec![gone], "detached");
        // The bound rect, the raw one, and the rects of the used main, the
        // detached copy and the lonely main (their frames have no fill): the
        // live instance's paint is its main's, counted there.
        assert_eq!((h.paint.bound, h.paint.total), (1, 5), "paint: {h:?}");
        assert!(h.paint.raw.contains(&l["raw"]));
        assert!(
            h.raw_match.ids.contains(&l["raw"]),
            "the raw #3366cc is Brand: {h:?}"
        );
        assert!(matches!(h.matches.first(), Some((Match::Variable(_), _))));
        assert_eq!(h.unused_components.ids, vec![l["lonely"]], "the used main");
        let idle = doc
            .variables()
            .iter()
            .find(|v| v.name == "Idle")
            .unwrap()
            .id;
        assert_eq!(h.unused_variables, vec![idle], "Brand is used");
        assert_eq!(h.overall(), Some(20));
        let json = h.to_json(&doc);
        assert_eq!(json["detached"]["count"], 1);
        assert_eq!(json["unused"]["variables"], json!(["Idle"]));
    }

    /// A reaction whose frame, variable and script were deleted is said once
    /// for its layer, naming each kind; a flow whose start was deleted is said
    /// on its page; a reaction whose targets are all there is not. `health`
    /// counts the layer, not the page.
    ///
    /// flip: `gone_targets` not reading `prototype::refs` → fails at "the
    /// variable and the script"; `dangling_targets` passing over pages → fails
    /// at "the flow".
    #[test]
    fn dangling_prototype_targets_are_said() {
        let mut ids = IdSource::new(0xda9);
        let page = ids.mint();
        let mut doc = Document::new(page);
        let p = page.to_wire();
        let mut run = |doc: &mut Document, steps: Vec<Value>| {
            let made = mastik_core::verbs::run_script(
                doc,
                &mut ids,
                mastik_core::verbs::Lent::default(),
                &Value::Array(steps.clone()),
            )
            .expect("the fixture builds");
            steps
                .iter()
                .zip(made)
                .filter_map(|(s, m)| Some((s["as"].as_str()?.to_owned(), m.first()?.clone())))
                .collect::<std::collections::HashMap<String, String>>()
        };
        let l = run(
            &mut doc,
            vec![
                layer(&p, "a", 0.0, 0.0, frame(200.0, 200.0, "#ffffff")),
                layer(&p, "b", 300.0, 0.0, frame(200.0, 200.0, "#ffffff")),
                layer(&p, "c", 600.0, 0.0, frame(200.0, 200.0, "#ffffff")),
                layer("@a", "button", 10.0, 10.0, rect(40.0, 40.0, "#000000", 0.0)),
                layer("@a", "fine", 60.0, 10.0, rect(40.0, 40.0, "#000000", 0.0)),
                json!({ "verb": "add_variable", "args": { "kind": "boolean", "name": "On" },
                    "as": "on" }),
                json!({ "verb": "add_script", "args": { "name": "Go", "source": "print(1)" },
                    "as": "go" }),
            ],
        );
        let reactions = json!([{ "trigger": "click", "actions": [
            { "navigate": { "to": l["b"] } },
            { "SetVariable": { "var": l["on"], "change": "Toggle" } },
            { "RunScript": { "script": l["go"] } },
        ] }]);
        run(
            &mut doc,
            vec![
                json!({ "verb": "set_reactions", "args": { "nodes": [l["button"]],
                    "reactions": reactions } }),
                json!({ "verb": "set_reactions", "args": { "nodes": [l["fine"]],
                    "reactions": [{ "trigger": "click", "actions": [
                        { "navigate": { "to": l["c"] } }] }] } }),
                json!({ "verb": "set_flows", "args": { "page": p,
                    "flows": [{ "name": "Main", "start": l["b"] }] } }),
                json!({ "verb": "delete_node", "args": { "nodes": [l["b"]] } }),
                json!({ "verb": "remove_variable", "args": { "variable": l["on"] } }),
                json!({ "verb": "remove_script", "args": { "script": l["go"] } }),
            ],
        );
        let res = Resolved::rebuild(&doc);
        let scope = Scope::new(&doc, &res, &[page]);
        let found: Vec<Finding> = review(&scope, Options::default())
            .into_iter()
            .filter(|f| f.code == "dangling_target")
            .collect();
        let button = NodeId::from_wire(&l["button"]).unwrap();
        let on_button: Vec<&Finding> = found.iter().filter(|f| f.ids == vec![button]).collect();
        assert_eq!(on_button.len(), 1, "one finding for the layer: {found:?}");
        assert_eq!(
            on_button[0].message,
            "Its prototype names a layer, a variable and a script that are gone; play skips what needs them.",
            "the variable and the script"
        );
        assert!(
            found
                .iter()
                .any(|f| f.ids == vec![page] && f.message.contains("“Main”")),
            "the flow: {found:?}"
        );
        assert_eq!(found.len(), 2, "the layer whose target is there: {found:?}");
        let h = health(&scope, 4.0);
        assert_eq!(h.dangling.ids, vec![button], "the layer, not the page");
        assert_eq!(h.to_json(&doc)["dangling"]["count"], 1);
    }

    /// A component set none of whose variants is shown counts once, as the set;
    /// a set with one shown variant does not count, and neither do its unshown
    /// variants on their own.
    ///
    /// flip: `unused_components` reading `in_set` as false → fails at "a variant
    /// alone"; checking only the set's first variant → fails at "a set in use".
    #[test]
    fn unused_components_count_a_set_once() {
        let mut ids = IdSource::new(0x4ec);
        let page = ids.mint();
        let mut doc = Document::new(page);
        let p = page.to_wire();
        let main = |label: &str, x: f64| {
            vec![
                layer(&p, label, x, 0.0, frame(40.0, 40.0, "#ff0000")),
                json!({ "verb": "make_component", "args": { "nodes": [format!("@{label}")] } }),
            ]
        };
        let mut steps = Vec::new();
        for (label, x) in [("a1", 0.0), ("a2", 50.0), ("b1", 200.0), ("b2", 250.0)] {
            steps.extend(main(label, x));
        }
        steps.push(
            json!({ "verb": "combine_as_variants", "args": { "nodes": ["@a1", "@a2"] },
            "as": "idle_set" }),
        );
        steps.push(
            json!({ "verb": "combine_as_variants", "args": { "nodes": ["@b1", "@b2"] },
            "as": "used_set" }),
        );
        steps.push(
            json!({ "verb": "create_instance", "args": { "main": "@b2", "parent": p,
            "x": 400, "y": 0 } }),
        );
        let made = mastik_core::verbs::run_script(
            &mut doc,
            &mut ids,
            mastik_core::verbs::Lent::default(),
            &Value::Array(steps.clone()),
        )
        .expect("the fixture builds");
        let l: std::collections::HashMap<&str, NodeId> = steps
            .iter()
            .zip(made)
            .filter_map(|(s, m)| Some((s["as"].as_str()?, NodeId::from_wire(m.first()?)?)))
            .collect();
        let res = Resolved::rebuild(&doc);
        let scope = Scope::new(&doc, &res, &[page]);
        let unused = unused_components(&scope);
        assert!(unused.contains(&l["idle_set"]), "an idle set: {unused:?}");
        assert!(!unused.contains(&l["used_set"]), "a set in use: {unused:?}");
        for v in ["a1", "a2", "b1"] {
            assert!(
                !unused.contains(&l[v]),
                "a variant alone: {v} in {unused:?}"
            );
        }
        assert_eq!(unused.len(), 1, "{unused:?}");
    }

    /// A raw gap is offered the spacing variable of its value, not a radius one
    /// made first; a colour written raw beside its variable is named by that
    /// variable, which is the merge's target, and a text of its exact colour,
    /// which no merge changes, is no finding.
    ///
    /// flip: `fitting` taking the first equal variable → fails at "the gap's";
    /// without its filter on the other field's names, or counting a scope of
    /// every field as one → fails at "a radius offered for a gap";
    /// `near_duplicate_colours` reading `default_target` as a census index →
    /// fails at "the target"; `rewrite` counting a place written its own colour
    /// → fails at "a text of the variable's colour"; `raw_values` not telling a
    /// colour apart → fails at "a colour's one-call fix"; its `mergeable`
    /// answering true for a variable nothing paints → fails at "an unused
    /// variable".
    #[test]
    fn raw_values_name_the_variable_that_fits() {
        let mut ids = IdSource::new(0x4eb);
        let page = ids.mint();
        let mut doc = Document::new(page);
        let p = page.to_wire();
        let steps = vec![
            json!({ "verb": "add_variable", "args": { "kind": "color", "name": "Ink",
                "value": { "color": "#f2f4ff" } } }),
            json!({ "verb": "add_variable", "args": { "kind": "color", "name": "Ground",
                "value": { "color": "#0b1020" } }, "as": "ground" }),
            json!({ "verb": "add_variable", "args": { "kind": "number", "name": "radius/card",
                "value": { "number": 16 } } }),
            json!({ "verb": "add_variable", "args": { "kind": "number", "name": "space/md",
                "value": { "number": 16 } } }),
            layer(&p, "row", 0.0, 0.0, frame(300.0, 100.0, "#f2f4ff")),
            json!({ "verb": "set_display", "args": { "nodes": ["@row"],
                "display": { "Flex": { "direction": "Row", "gap": 16 } } } }),
            layer("@row", "bound", 0.0, 0.0, rect(40.0, 40.0, "#0b1020", 0.0)),
            json!({ "verb": "bind_variable", "args": { "nodes": ["@bound"],
                "field": { "Fill": 0 }, "variable": "@ground" } }),
            layer("@row", "raw", 0.0, 0.0, rect(40.0, 40.0, "#0b1020", 0.0)),
            json!({ "verb": "add_variable", "args": { "kind": "color", "name": "Leaf",
                "value": { "color": "#3f6212" } }, "as": "leaf" }),
            layer("@row", "leafy", 0.0, 0.0, rect(40.0, 40.0, "#3f6212", 0.0)),
            json!({ "verb": "bind_variable", "args": { "nodes": ["@leafy"],
                "field": { "Fill": 0 }, "variable": "@leaf" } }),
            // A text's own colour binds no variable, so a merge cannot change it.
            layer(
                &p,
                "note",
                0.0,
                200.0,
                json!({ "kind": { "Text": { "content": "Leaf",
                "style": { "font_size": 14, "color": "#3f6212" } } } }),
            ),
            json!({ "verb": "add_variable", "args": { "kind": "number", "name": "radius/xl",
                "value": { "number": 24 } }, "as": "xl" }),
            // As a token import writes it: a scope naming every number field.
            json!({ "verb": "set_scope", "args": { "variable": "@xl",
                "scope": ["size", "limits", "spacing", "corner", "strokeWidth", "effect", "type"] } }),
            layer(&p, "wide", 0.0, 300.0, frame(300.0, 100.0, "#f2f4ff")),
            json!({ "verb": "set_display", "args": { "nodes": ["@wide"],
                "display": { "Flex": { "direction": "Row", "gap": 24 } } } }),
            layer("@wide", "a", 0.0, 0.0, rect(40.0, 40.0, "#f2f4ff", 0.0)),
        ];
        let made = mastik_core::verbs::run_script(
            &mut doc,
            &mut ids,
            mastik_core::verbs::Lent::default(),
            &Value::Array(steps.clone()),
        )
        .expect("the fixture builds");
        let l: std::collections::HashMap<&str, NodeId> = steps
            .iter()
            .zip(made)
            .filter_map(|(s, m)| Some((s["as"].as_str()?, NodeId::from_wire(m.first()?)?)))
            .collect();
        let res = Resolved::rebuild(&doc);
        let scope = Scope::new(&doc, &res, &[page]);
        let raw = raw_values(&scope);
        let gap = raw
            .iter()
            .find(|f| f.message.starts_with("A gap"))
            .expect("the raw gap");
        assert!(gap.message.contains("`space/md`"), "the gap's: {gap:?}");
        assert_eq!(gap.instead.as_deref(), Some("bind the variable"));
        assert!(
            !raw.iter().any(|f| f.message.contains("`radius/xl`")),
            "a radius offered for a gap: {raw:?}"
        );
        let fill = |var: &str| {
            raw.iter()
                .find(|f| f.message.starts_with("A fill") && f.message.contains(var))
                .unwrap_or_else(|| panic!("the raw fill of {var}: {raw:?}"))
        };
        let ground = fill("`Ground`");
        assert!(
            ground
                .instead
                .as_deref()
                .is_some_and(|i| i.contains("`merge_colours`")),
            "a colour's one-call fix: {ground:?}"
        );
        // The advice holds: the merge it names binds the raw `Ground` fill.
        let mut merged = doc.clone();
        mastik_core::verbs::run_script(
            &mut merged,
            &mut ids,
            mastik_core::verbs::Lent::default(),
            &json!([{ "verb": "merge_colours",
                "args": { "colours": ["#0b1020ff"], "into": "#0b1020ff" } }]),
        )
        .expect("the merge the hint names");
        let res_m = Resolved::rebuild(&merged);
        let after = raw_values(&Scope::new(&merged, &res_m, &[page]));
        assert!(
            !after.iter().any(|f| f.message.contains("`Ground`")),
            "merged away: {after:?}"
        );
        // `Ink` paints nothing, so a merge would find no variable to bind to.
        assert_eq!(
            fill("`Ink`").instead.as_deref(),
            Some("bind the variable"),
            "an unused variable"
        );
        let near = near_duplicate_colours(&scope);
        let pair = near
            .iter()
            .find(|f| f.message.contains("#0b1020"))
            .expect("the raw and bound #0b1020");
        assert!(pair.message.contains("(`Ground`)"), "{pair:?}");
        assert_eq!(pair.ids, vec![l["raw"]], "only the layer a merge changes");
        assert!(
            !near.iter().any(|f| f.message.contains("#3f6212")),
            "a text of the variable's colour is no finding: {near:?}"
        );
        assert_eq!(
            pair.instead.as_deref(),
            Some("one colour, #0b1020 (`Ground`): `merge_colours`"),
            "the target"
        );
    }

    /// A raw match is only what binding or applying would keep as drawn: a
    /// colour against the variable in the mode in force at the layer, a text's
    /// whole type against a text style, and a name's words by their start, never
    /// inside them. One finding holds one raw colour, and a merge is offered only
    /// where it binds that colour to the variable.
    ///
    /// flip: `raw_matches` reading every layer in the default mode → fails at
    /// "the Light colour in a Dark frame"; its colour key without the colour →
    /// fails at "the Light colour in a Dark frame" too, `night_dark` joining
    /// `day`; the text arm comparing six fields → fails at "an uppercase twin";
    /// `fitting` matching inside a word → fails at "`background/blur` is no
    /// radius"; `fitting` matching whole words only → fails at the `size/12`
    /// lookup, the radius offered as `spacer`; `mergeable` taking any swatch of
    /// the variable → fails at "a variable bound only at half"; `health` keeping
    /// a row per colour key → fails at "health lists `Brand` once".
    #[test]
    fn raw_matches_hold_what_binding_or_applying_keeps() {
        let mut ids = IdSource::new(0x4ec);
        let page = ids.mint();
        let mut doc = Document::new(page);
        let p = page.to_wire();
        let steps = vec![
            json!({ "verb": "add_variable", "args": { "kind": "color", "name": "Brand",
                "value": { "color": "#3366cc" } }, "as": "brand" }),
            json!({ "verb": "add_mode", "args": { "name": "Dark" }, "as": "dark" }),
            json!({ "verb": "set_variable_value", "args": { "variable": "@brand",
                "mode": "@dark", "value": { "color": "#99bbff" } } }),
            layer(&p, "day", 0.0, 0.0, rect(40.0, 40.0, "#3366cc", 0.0)),
            layer(&p, "night", 100.0, 0.0, frame(200.0, 100.0, "#000000")),
            json!({ "verb": "set_frame_mode", "args": { "node": "@night", "mode": "@dark" } }),
            layer(
                "@night",
                "night_light",
                0.0,
                0.0,
                rect(40.0, 40.0, "#3366cc", 0.0),
            ),
            layer(
                "@night",
                "night_dark",
                50.0,
                0.0,
                rect(40.0, 40.0, "#99bbff", 0.0),
            ),
            // `Brand` bound once, so a merge of its Light colour binds to it.
            layer(&p, "bound", 0.0, 100.0, rect(40.0, 40.0, "#3366cc", 0.0)),
            json!({ "verb": "bind_variable", "args": { "nodes": ["@bound"],
                "field": { "Fill": 0 }, "variable": "@brand" } }),
            // Bound only at half its alpha: no swatch of the raw colour is it.
            json!({ "verb": "add_variable", "args": { "kind": "color", "name": "Half",
                "value": { "color": "#226622" } }, "as": "half" }),
            layer(&p, "halved", 0.0, 200.0, rect(40.0, 40.0, "#22662280", 0.0)),
            json!({ "verb": "bind_variable", "args": { "nodes": ["@halved"],
                "field": { "Fill": 0 }, "variable": "@half" } }),
            layer(
                &p,
                "half_raw",
                50.0,
                200.0,
                rect(40.0, 40.0, "#226622", 0.0),
            ),
            // Ahead of `size/12`, which it would tie with as a name of neither field.
            json!({ "verb": "add_variable", "args": { "kind": "number", "name": "spacer",
                "value": { "number": 12 } } }),
            json!({ "verb": "add_variable", "args": { "kind": "number", "name": "size/12",
                "value": { "number": 12 } } }),
            json!({ "verb": "add_variable", "args": { "kind": "number",
                "name": "background/blur", "value": { "number": 12 } } }),
            layer(
                &p,
                "rounded",
                100.0,
                200.0,
                rect(40.0, 40.0, "#ffffff", 12.0),
            ),
            layer(&p, "body", 0.0, 300.0, text("#111111")),
            json!({ "verb": "add_style", "args": { "from": "@body", "slot": "text",
                "name": "Body" }, "as": "body_style" }),
            layer(&p, "twin", 0.0, 350.0, text("#111111")),
            layer(&p, "loud", 0.0, 400.0, text("#111111")),
            json!({ "verb": "set_text_style", "args": { "nodes": ["@loud"],
                "style": { "case": "Upper" } } }),
            layer(&p, "card", 300.0, 0.0, rect(40.0, 40.0, "#abcdef", 0.0)),
            layer(
                &p,
                "card_twin",
                350.0,
                0.0,
                rect(40.0, 40.0, "#abcdef", 0.0),
            ),
            layer(
                &p,
                "card_styled",
                400.0,
                0.0,
                rect(40.0, 40.0, "#abcdef", 0.0),
            ),
            json!({ "verb": "add_style", "args": { "from": "@card", "slot": "fills",
                "name": "Card", "nodes": ["@card_styled"] } }),
        ];
        let made = mastik_core::verbs::run_script(
            &mut doc,
            &mut ids,
            mastik_core::verbs::Lent::default(),
            &Value::Array(steps.clone()),
        )
        .expect("the fixture builds");
        let l: std::collections::HashMap<&str, NodeId> = steps
            .iter()
            .zip(made)
            .filter_map(|(s, m)| Some((s["as"].as_str()?, NodeId::from_wire(m.first()?)?)))
            .collect();
        let res = Resolved::rebuild(&doc);
        let scope = Scope::new(&doc, &res, &[page]);
        let raw = raw_values(&scope);
        let of = |var: &str| -> &Finding {
            raw.iter()
                .find(|f| f.code == "raw_value" && f.message.contains(var))
                .unwrap_or_else(|| panic!("{var}: {raw:?}"))
        };
        let brand = of("A fill of #3366cc is the variable `Brand`");
        assert_eq!(
            brand.ids,
            vec![l["day"]],
            "the Light colour in a Dark frame: {brand:?}"
        );
        assert!(
            brand
                .instead
                .as_deref()
                .is_some_and(|i| i.contains("merge_colours")),
            "the Light colour merges into the bound one: {brand:?}"
        );
        let dark = of("A fill of #99bbff is the variable `Brand`");
        assert_eq!(
            dark.ids,
            vec![l["night_dark"]],
            "the Dark one, apart: {dark:?}"
        );
        assert_eq!(
            dark.instead.as_deref(),
            Some("bind the variable"),
            "no swatch of the Dark colour is bound: {dark:?}"
        );
        assert_eq!(
            of("`Half`").instead.as_deref(),
            Some("bind the variable"),
            "a variable bound only at half"
        );
        assert!(
            of("`size/12`").ids == vec![l["rounded"]]
                && !raw.iter().any(|f| f.message.contains("background/blur")),
            "`background/blur` is no radius: {raw:?}"
        );
        assert!(
            !raw.iter().any(|f| f.message.contains("`spacer`")),
            "`spacer` is no radius: {raw:?}"
        );
        let style = |name: &str| -> Vec<&Finding> {
            raw.iter()
                .filter(|f| f.code == "raw_style" && f.message.contains(name))
                .collect()
        };
        let card = style("`Card`");
        assert!(
            card.len() == 1
                && card[0].ids.contains(&l["card"])
                && card[0].ids.contains(&l["card_twin"])
                && !card[0].ids.contains(&l["card_styled"]),
            "the fill style's raw twins, not the styled one: {card:?}"
        );
        let styled = style("`Body`");
        assert_eq!(styled.len(), 1, "{styled:?}");
        assert!(
            styled[0].ids.contains(&l["twin"]) && styled[0].ids.contains(&l["body"]),
            "the plain twin: {styled:?}"
        );
        assert!(
            !styled[0].ids.contains(&l["loud"]),
            "an uppercase twin: {styled:?}"
        );
        let h = health(&scope, 4.0);
        assert!(
            h.matches.iter().any(|(m, ids)| *m
                == Match::Style(mastik_core::style::StyleId(l["body_style"]))
                && ids.contains(&l["twin"])),
            "health lists the style: {:?}",
            h.matches
        );
        let brand_rows: Vec<_> = h
            .matches
            .iter()
            .filter(|(m, _)| *m == Match::Variable(mastik_core::VarId(l["brand"])))
            .collect();
        assert!(
            brand_rows.len() == 1
                && brand_rows[0].1.contains(&l["day"])
                && brand_rows[0].1.contains(&l["night_dark"]),
            "health lists `Brand` once, both colours: {:?}",
            h.matches
        );
    }

    /// Hand-placed gaps off the scale are one finding per gap, naming every
    /// layer beside one, and `health` folds them and an off-scale radius into
    /// its count, each corner counted as spacing.
    /// A boolean's operands are its shape, so their gaps are not.
    /// flip: `health` without its fold of `spacing`'s findings → fails at "the
    /// row's layers"; `counts` without its corner arm → fails at "radius 6";
    /// `spacing` without its boolean skip → fails at "not the dots'".
    #[test]
    fn off_scale_gaps_and_radii_reach_health() {
        let mut ids = IdSource::new(0x4ee);
        let page = ids.mint();
        let mut doc = Document::new(page);
        let p = page.to_wire();
        let steps = vec![
            layer(&p, "row", 0.0, 0.0, frame(300.0, 100.0, "#ffffff")),
            layer("@row", "r0", 0.0, 0.0, rect(40.0, 40.0, "#eeeeee", 0.0)),
            layer("@row", "r1", 50.0, 0.0, rect(40.0, 40.0, "#eeeeee", 0.0)),
            layer("@row", "r2", 100.0, 0.0, rect(40.0, 40.0, "#eeeeee", 0.0)),
            layer(&p, "box", 0.0, 200.0, frame(100.0, 100.0, "#ffffff")),
            layer("@box", "soft", 0.0, 0.0, rect(40.0, 40.0, "#eeeeee", 6.0)),
            // A dotted mark: its operands' gaps are the shape, not a row.
            layer(
                &p,
                "dots",
                0.0,
                400.0,
                json!({ "kind": { "Boolean": { "op": "Union" } }, "fills": ["#dddddd"] }),
            ),
            layer("@dots", "d0", 0.0, 0.0, rect(40.0, 40.0, "#dddddd", 0.0)),
            layer("@dots", "d1", 50.0, 0.0, rect(40.0, 40.0, "#dddddd", 0.0)),
            layer("@dots", "d2", 100.0, 0.0, rect(40.0, 40.0, "#dddddd", 0.0)),
        ];
        let made = mastik_core::verbs::run_script(
            &mut doc,
            &mut ids,
            mastik_core::verbs::Lent::default(),
            &Value::Array(steps.clone()),
        )
        .expect("the fixture builds");
        let l: std::collections::HashMap<&str, NodeId> = steps
            .iter()
            .zip(made)
            .filter_map(|(s, m)| Some((s["as"].as_str()?, NodeId::from_wire(m.first()?)?)))
            .collect();
        let res = Resolved::rebuild(&doc);
        let scope = Scope::new(&doc, &res, &[page]);
        let row = [l["r0"], l["r1"], l["r2"]];
        assert_eq!(
            doc.get(l["dots"]).map(|n| n.children().len()),
            Some(3),
            "fixture: three operands"
        );
        let off: Vec<Finding> = spacing(&scope, 4.0)
            .into_iter()
            .filter(|f| f.code == "off_scale")
            .collect();
        assert_eq!(
            off.len(),
            1,
            "one gap of 10, the row's, not the dots': {off:?}"
        );
        assert!(row.iter().all(|id| off[0].ids.contains(id)), "{off:?}");
        let h = health(&scope, 4.0);
        assert!(
            row.iter().all(|id| h.off_scale.ids.contains(id)),
            "the row's layers: {h:?}"
        );
        assert!(h.off_scale.ids.contains(&l["soft"]), "radius 6: {h:?}");
        assert_eq!(h.spacing.total, 4, "four corners: {h:?}");
    }

    /// A group operand's members overlap to make the boolean's shape too.
    /// flip: `overlaps` passing over a boolean's direct children only → fails.
    #[test]
    fn a_group_operands_members_overlap_unremarked() {
        let mut ids = IdSource::new(0x4ed);
        let page = ids.mint();
        let mut doc = Document::new(page);
        let p = page.to_wire();
        let steps = vec![
            layer(
                &p,
                "mark",
                0.0,
                0.0,
                json!({ "kind": { "Boolean": { "op": "Union" } }, "fills": ["#dddddd"] }),
            ),
            layer("@mark", "g", 0.0, 0.0, json!({ "kind": "Group" })),
            layer("@g", "a", 0.0, 0.0, rect(100.0, 100.0, "#dddddd", 0.0)),
            layer("@g", "b", 50.0, 50.0, rect(100.0, 100.0, "#dddddd", 0.0)),
            layer("@mark", "c", 120.0, 0.0, rect(100.0, 100.0, "#dddddd", 0.0)),
        ];
        let made = mastik_core::verbs::run_script(
            &mut doc,
            &mut ids,
            mastik_core::verbs::Lent::default(),
            &Value::Array(steps.clone()),
        )
        .expect("the fixture builds");
        let g = steps
            .iter()
            .zip(made)
            .find(|(s, _)| s["as"] == "g")
            .and_then(|(_, m)| NodeId::from_wire(m.first()?))
            .unwrap();
        assert!(
            matches!(doc.get(g).map(|n| n.kind()), Some(NodeKind::Group))
                && doc.get(g).unwrap().children().len() == 2,
            "fixture: a group of two inside the boolean"
        );
        let res = Resolved::rebuild(&doc);
        let found = overlaps(&Scope::new(&doc, &res, &[page]));
        assert!(found.is_empty(), "{found:?}");
    }
}
