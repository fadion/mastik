//! *Apply brush*: one shape, the brush, bent along a path (*Art*) or repeated
//! along it (*Scatter*), baked into plain paths in a group named
//! `Brushed · ‹brush›` that takes the path's place. Computed once; nothing stays
//! live. The brush stays where it is; the path goes, or sits hidden in the group.
//!
//! Art maps the brush's world box onto the path: its left-to-right onto arc
//! length, its height onto the path's normal about the box's middle line. Each
//! brush curve is cut where the map bends (a path's anchors, *Middle only*'s
//! fifths) and subdivided until a cubic through the mapped ends and tangents
//! stays within [`TOLERANCE`] of the exact map, so curves stay curves.

use crate::build;
use crate::kurbo::{
    Affine, BezPath, CubicBez, Line, ParamCurve, ParamCurveArclen, ParamCurveDeriv, PathEl,
    PathSeg, Point, Rect, Shape, Vec2,
};
use crate::node::NodeKind;
use crate::{Document, IdSource, NodeId, Operation, Resolved, Transaction};

/// What the group's name starts with, before ` · ‹brush›`.
pub const GROUP_PREFIX: &str = "Brushed";

/// The name the path takes inside the group when it is kept.
pub const KEPT_PATH_NAME: &str = "Path";

/// The most copies a scatter makes along a path.
pub const MAX_COPIES: usize = 2000;

/// The share of the brush's length at each end that *Middle only* keeps
/// unstretched.
pub const CAP: f64 = 0.2;

/// How far, in world units, a bent curve may stray from the exact map.
pub const TOLERANCE: f64 = 0.02;

/// How an art brush stretches along the path.
#[derive(Clone, Copy, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Stretch {
    /// The whole shape spans the path.
    Whole,
    /// The shape's first and last [`CAP`] keep their length (scaled with the
    /// width), so a brushstroke's ends keep their shape; the middle stretches.
    Middle,
}

/// How each scattered copy turns.
#[derive(Clone, Copy, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Turn {
    /// With the path's direction where it stands.
    Along,
    /// Not at all: every copy faces as the brush does.
    Fixed,
}

/// The brush bent along the whole path.
#[derive(Clone, Copy, Debug, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct Art {
    /// The brush's height across the path, as a share of its own: 1 draws it as
    /// tall as it is.
    pub width: f64,
    pub stretch: Stretch,
    /// Mirror the brush across the path.
    pub flip: bool,
}

impl Art {
    pub const DEFAULT: Art = Art {
        width: 1.0,
        stretch: Stretch::Whole,
        flip: false,
    };
}

/// The brush repeated along the path.
#[derive(Clone, Copy, Debug, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct Scatter {
    /// From one copy's middle to the next, as a share of the brush's length.
    pub spacing: f64,
    /// Each copy's size as a share of the brush's.
    pub size: f64,
    /// How far a copy's size may stray from `size`, either way, as a share of it.
    pub size_varies: f64,
    /// How far a copy may stray sideways off the path, either way, as a share of
    /// the brush's length.
    pub scatter: f64,
    pub turn: Turn,
    /// Which run of random sizes and offsets the copies take.
    pub seed: u32,
}

impl Scatter {
    pub const DEFAULT: Scatter = Scatter {
        spacing: 1.2,
        size: 0.8,
        size_varies: 0.3,
        scatter: 0.2,
        turn: Turn::Along,
        seed: 1,
    };
}

/// Which brush, and its settings.
#[derive(Clone, Copy, Debug, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Mode {
    Art(Art),
    Scatter(Scatter),
}

/// One application of a brush to a path.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ApplyBrush {
    /// The path the brush runs along.
    pub path: NodeId,
    /// The shape that is bent or repeated.
    pub brush: NodeId,
    pub mode: Mode,
    /// Keep the path in the group, hidden, rather than delete it.
    pub keep_path: bool,
}

/// Why a brush cannot be applied.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Refusal {
    /// Not two layers, or neither one a path to run along, or a brush that is
    /// no shape.
    Count,
    /// The brush is the path.
    Same,
    Picture,
    Text,
    /// The brush has no length to lay along the path.
    Flat,
    /// The path has no length to run along.
    Length,
    /// The path is locked, or in a locked group.
    Locked,
    /// The path sits in a boolean, a main or an instance, where a new group
    /// would change what that draws.
    Inside,
    /// The path is a mask: the group in its place would stop clipping.
    Mask,
    /// The path sits in an auto layout, which would lay the group out elsewhere.
    Layout,
    /// More than [`MAX_COPIES`] copies.
    TooMany,
    /// The brush is drawn along the path it would brush, through a chain of
    /// live brushes.
    Loop,
    /// The brush sits in an instance, whose layers follow their main.
    BrushInInstance,
}

impl Refusal {
    /// The sentence a dimmed row and a refused commit say.
    pub fn says(self) -> &'static str {
        match self {
            Refusal::Count => "Select a path, and the shape to run along it",
            Refusal::Same => "A brush can't be the path it runs along",
            Refusal::Picture => "A brush can't be a picture",
            Refusal::Text => "A brush can't be text",
            Refusal::Flat => "A brush needs some length to lay along the path",
            Refusal::Length => "The path has no length to run along",
            Refusal::Locked => "The path is locked",
            Refusal::Inside => {
                "Apply brush can't work inside a boolean, a component or an instance"
            }
            Refusal::Mask => "Apply brush can't take a mask",
            Refusal::Layout => "Apply brush can't work inside an auto layout",
            Refusal::TooMany => "The brush would make too many copies: space them out more",
            Refusal::Loop => "A brush can't be drawn along a path it brushes",
            Refusal::BrushInInstance => "A brush can't be inside an instance",
        }
    }
}

/// Whether `kind` has an outline a brush can run along or be made of.
pub(crate) fn outlined(kind: &NodeKind) -> bool {
    matches!(
        kind,
        NodeKind::Rect { .. }
            | NodeKind::Ellipse { .. }
            | NodeKind::Polygon { .. }
            | NodeKind::Star { .. }
            | NodeKind::Line { .. }
            | NodeKind::Spiral { .. }
            | NodeKind::Arc { .. }
            | NodeKind::Path { .. }
            | NodeKind::Boolean { .. }
    )
}

/// The path and the brush of a two-layer selection: the lower layer is the
/// path and the upper the brush, unless only the upper has an outline to run
/// along. Checks only the count and the kinds; [`refusal`] checks the rest.
pub fn pair(doc: &Document, selection: &[NodeId]) -> Result<(NodeId, NodeId), Refusal> {
    let members = build::in_document_order(doc, selection);
    let [lower, upper] = members.as_slice() else {
        return Err(Refusal::Count);
    };
    let runs_along = |id: NodeId| doc.get(id).is_some_and(|n| outlined(n.kind()));
    match (runs_along(*lower), runs_along(*upper)) {
        (true, _) => Ok((*lower, *upper)),
        (false, true) => Ok((*upper, *lower)),
        (false, false) => Err(Refusal::Count),
    }
}

/// Why the selection cannot take a brush, if it cannot: [`pair`]'s refusal, then
/// [`refusal`]'s for the pair it names.
pub fn selection_refusal(doc: &Document, res: &Resolved, selection: &[NodeId]) -> Option<Refusal> {
    match pair(doc, selection) {
        Ok((path, brush)) => refusal(doc, res, path, brush),
        Err(r) => Some(r),
    }
}

/// Why `brush` cannot be the shape a brush is made of, if it cannot.
pub fn brush_refusal(doc: &Document, res: &Resolved, brush: NodeId) -> Option<Refusal> {
    let node = doc.get(brush)?;
    if matches!(node.kind(), NodeKind::Text { .. }) {
        return Some(Refusal::Text);
    }
    if node.paint().image_fill().is_some() {
        return Some(Refusal::Picture);
    }
    if !outlined(node.kind()) {
        return Some(Refusal::Count);
    }
    match outline(doc, res, brush) {
        Some((_, bounds)) if bounds.width() > 1e-6 => None,
        _ => Some(Refusal::Flat),
    }
}

/// Why `brush` cannot be baked along `path`, if it cannot.
pub fn refusal(doc: &Document, res: &Resolved, path: NodeId, brush: NodeId) -> Option<Refusal> {
    pair_refusal(doc, res, path, brush, true)
}

/// [`refusal`]'s tests; `bake` adds the two that hold only because a bake puts a
/// group in the path's place: a mask stops clipping and a layout moves it.
pub(crate) fn pair_refusal(
    doc: &Document,
    res: &Resolved,
    path: NodeId,
    brush: NodeId,
    bake: bool,
) -> Option<Refusal> {
    if path == brush {
        return Some(Refusal::Same);
    }
    let Some(node) = doc.get(path) else {
        return Some(Refusal::Count);
    };
    if !outlined(node.kind()) {
        return Some(Refusal::Count);
    }
    if crate::is_effectively_locked(doc, path) {
        return Some(Refusal::Locked);
    }
    let mut up = node.parent();
    while let Some(p) = up.and_then(|p| doc.get(p)) {
        if matches!(p.kind(), NodeKind::Boolean { .. }) || p.component() || p.link().is_some() {
            return Some(Refusal::Inside);
        }
        up = p.parent();
    }
    if node.link().is_some() {
        return Some(Refusal::Inside);
    }
    if bake && node.mask() {
        return Some(Refusal::Mask);
    }
    if bake && crate::container::lays_out_above(&crate::resolve::DocView(doc), path) {
        return Some(Refusal::Layout);
    }
    if tracks(doc, res, path).is_empty() {
        return Some(Refusal::Length);
    }
    brush_refusal(doc, res, brush)
}

// --- geometry --------------------------------------------------------------------

/// One subpath as cubic segments, and whether it closes.
#[derive(Clone, Debug)]
pub(crate) struct Run {
    pub(crate) segs: Vec<CubicBez>,
    pub(crate) closed: bool,
}

fn cubic(seg: PathSeg) -> CubicBez {
    match seg {
        PathSeg::Line(Line { p0, p1 }) => {
            CubicBez::new(p0, p0.lerp(p1, 1.0 / 3.0), p0.lerp(p1, 2.0 / 3.0), p1)
        }
        PathSeg::Quad(q) => q.raise(),
        PathSeg::Cubic(c) => c,
    }
}

/// `path`'s subpaths as runs, dropping segments with no length.
pub(crate) fn runs(path: &BezPath) -> Vec<Run> {
    crate::geometry::subpaths(path)
        .into_iter()
        .filter_map(|sub| {
            let closed = matches!(sub.elements().last(), Some(PathEl::ClosePath));
            let segs: Vec<CubicBez> = sub
                .segments()
                .map(cubic)
                .filter(|c| c.p0.distance(c.p3) > 1e-9 || c.p0.distance(c.p1) > 1e-9)
                .collect();
            (!segs.is_empty()).then_some(Run { segs, closed })
        })
        .collect()
}

/// How finely a segment's arc length is measured.
const ACCURACY: f64 = 1e-4;

/// How many steps of `t` a segment's arc-length table holds.
const SAMPLES: usize = 32;

/// A path to run along, measured: each segment's arc length at evenly spaced
/// `t`, so a point at an arc length is found by table rather than by search.
#[derive(Clone, Debug)]
pub(crate) struct Track {
    segs: Vec<CubicBez>,
    /// The arc length each segment starts at.
    starts: Vec<f64>,
    /// Per segment, the arc length from its start at `t = k / SAMPLES`.
    tables: Vec<[f64; SAMPLES + 1]>,
    pub(crate) total: f64,
    pub(crate) closed: bool,
}

impl Track {
    /// `run` measured, or `None` when it has no length.
    pub(crate) fn new(run: Run) -> Option<Track> {
        let mut starts = Vec::with_capacity(run.segs.len());
        let mut tables = Vec::with_capacity(run.segs.len());
        let mut total = 0.0;
        for c in &run.segs {
            let mut table = [0.0; SAMPLES + 1];
            for k in 1..=SAMPLES {
                let (t0, t1) = ((k - 1) as f64 / SAMPLES as f64, k as f64 / SAMPLES as f64);
                table[k] = table[k - 1] + c.subsegment(t0..t1).arclen(ACCURACY);
            }
            starts.push(total);
            total += table[SAMPLES];
            tables.push(table);
        }
        (total > 1e-6).then_some(Track {
            segs: run.segs,
            starts,
            tables,
            total,
            closed: run.closed,
        })
    }

    /// The segment arc length `s` falls in, the last for `s` past the end.
    fn seg_of(&self, s: f64) -> usize {
        self.starts
            .partition_point(|start| *start <= s)
            .saturating_sub(1)
    }

    /// The `t` on segment `i` at arc length `s` along the whole track, clamped to
    /// the segment.
    fn t_at(&self, i: usize, s: f64) -> f64 {
        let table = &self.tables[i];
        let local = (s - self.starts[i]).clamp(0.0, table[SAMPLES]);
        let k = table.partition_point(|l| *l < local).clamp(1, SAMPLES);
        let (l0, l1) = (table[k - 1], table[k]);
        let f = match l1 - l0 > 1e-12 {
            true => (local - l0) / (l1 - l0),
            false => 0.0,
        };
        ((k - 1) as f64 + f) / SAMPLES as f64
    }

    /// The point on segment `i` at arc length `s`, and the unit tangent there.
    fn frame(&self, i: usize, s: f64) -> (Point, Vec2) {
        let c = self.segs[i];
        let t = self.t_at(i, s);
        let d = c.deriv();
        let mut v = d.eval(t).to_vec2();
        // A handle on its anchor has no derivative there: a step inside says
        // which way the segment leaves, and the chord when that is none either.
        if v.hypot2() < 1e-18 {
            v = d.eval(t.clamp(1e-3, 1.0 - 1e-3)).to_vec2();
        }
        if v.hypot2() < 1e-18 {
            v = c.p3 - c.p0;
        }
        let v = match v.hypot() > 1e-12 {
            true => v / v.hypot(),
            false => Vec2::new(1.0, 0.0),
        };
        (c.eval(t), v)
    }

    /// The point at arc length `s` and the unit tangent there.
    pub(crate) fn at(&self, s: f64) -> (Point, Vec2) {
        self.frame(self.seg_of(s), s)
    }
}

/// The left-hand normal of unit tangent `t`, in y-down space: a brush's lower
/// half (larger y) falls on this side.
fn normal(t: Vec2) -> Vec2 {
    Vec2::new(-t.y, t.x)
}

/// `id`'s outline in world space, as runs, and its box.
fn outline(doc: &Document, res: &Resolved, id: NodeId) -> Option<(Vec<Run>, Rect)> {
    let world = res.world_transform(id)? * res.local_path(doc, id)?;
    let runs = runs(&world);
    (!runs.is_empty()).then(|| (runs, world.bounding_box()))
}

/// `id`'s subpaths in world space that have length, measured.
fn tracks(doc: &Document, res: &Resolved, id: NodeId) -> Vec<Track> {
    let Some(world) = res
        .world_transform(id)
        .zip(res.local_path(doc, id))
        .map(|(w, p)| w * p)
    else {
        return Vec::new();
    };
    runs(&world).into_iter().filter_map(Track::new).collect()
}

/// The map of a brush's box onto one track for an art brush: where each `x`
/// lands along the track, piecewise linear through `knots`, and how far off the
/// track each `y` lands.
pub(crate) struct Bend<'a> {
    track: &'a Track,
    /// `(x, s)` pairs, `x` ascending: the brush's `x` and the arc length it maps to.
    knots: Vec<(f64, f64)>,
    /// The brush box's middle line.
    mid: f64,
    /// What a unit of `y` off the middle line moves along the normal.
    across: f64,
}

impl<'a> Bend<'a> {
    pub(crate) fn new(track: &'a Track, bounds: Rect, art: Art) -> Bend<'a> {
        let (x0, x1, len) = (bounds.x0, bounds.x1, track.total);
        let knots = match art.stretch {
            Stretch::Whole => vec![(x0, 0.0), (x1, len)],
            Stretch::Middle => {
                let cap_in = bounds.width() * CAP;
                let cap_out = (cap_in * art.width.abs()).min(len / 2.0);
                vec![
                    (x0, 0.0),
                    (x0 + cap_in, cap_out),
                    (x1 - cap_in, len - cap_out),
                    (x1, len),
                ]
            }
        };
        Bend {
            track,
            knots,
            mid: bounds.center().y,
            across: if art.flip { -art.width } else { art.width },
        }
    }

    /// The arc length the brush's `x` maps to.
    pub(crate) fn s_of(&self, x: f64) -> f64 {
        let k = &self.knots;
        let i = k.partition_point(|(kx, _)| *kx <= x).clamp(1, k.len() - 1);
        let ((xa, sa), (xb, sb)) = (k[i - 1], k[i]);
        match xb - xa > 1e-12 {
            true => sa + (sb - sa) * ((x - xa) / (xb - xa)).clamp(0.0, 1.0),
            false => sa,
        }
    }

    /// The brush's `x` values where the map bends: the knots inside the box and
    /// each anchor of the track.
    fn breaks(&self) -> Vec<f64> {
        let k = &self.knots;
        let mut out: Vec<f64> = k[1..k.len() - 1].iter().map(|(x, _)| *x).collect();
        for &s in &self.track.starts[1..] {
            for w in k.windows(2) {
                let ((xa, sa), (xb, sb)) = (w[0], w[1]);
                if sb - sa > 1e-12 && s > sa && s < sb {
                    out.push(xa + (xb - xa) * (s - sa) / (sb - sa));
                }
            }
        }
        out.sort_by(f64::total_cmp);
        out.dedup_by(|a, b| (*a - *b).abs() < 1e-9);
        out
    }

    /// Where brush point `p` lands, read on track segment `seg`.
    fn map(&self, p: Point, seg: usize) -> Point {
        let s = self.s_of(p.x);
        let (at, t) = self.track.frame(seg, s);
        at + normal(t) * ((p.y - self.mid) * self.across)
    }

    /// The segment a brush piece between two breaks lies over.
    fn seg_for(&self, c: &CubicBez) -> usize {
        self.track.seg_of(self.s_of(c.eval(0.5).x))
    }
}

/// `c` cut wherever its `x` crosses one of `breaks`.
fn cut_at_breaks(c: CubicBez, breaks: &[f64]) -> Vec<CubicBez> {
    let (p0, p1, p2, p3) = (c.p0.x, c.p1.x, c.p2.x, c.p3.x);
    let a = p3 - 3.0 * p2 + 3.0 * p1 - p0;
    let b = 3.0 * (p2 - 2.0 * p1 + p0);
    let cc = 3.0 * (p1 - p0);
    let mut ts: Vec<f64> = breaks
        .iter()
        .flat_map(|x| kurbo::common::solve_cubic(p0 - x, cc, b, a))
        .filter(|t| t.is_finite() && *t > 1e-9 && *t < 1.0 - 1e-9)
        .collect();
    ts.sort_by(f64::total_cmp);
    ts.dedup_by(|a, b| (*a - *b).abs() < 1e-9);
    let mut out = Vec::with_capacity(ts.len() + 1);
    let mut from = 0.0;
    for t in ts {
        out.push(c.subsegment(from..t));
        from = t;
    }
    out.push(c.subsegment(from..1.0));
    out
}

/// How many times a brush piece may be halved to meet [`TOLERANCE`].
const MAX_DEPTH: u32 = 10;

/// Brush piece `c`, lying over track segment `seg`, mapped by `bend` as cubics
/// through the mapped ends with the mapped tangents there, halved until each
/// stays within [`TOLERANCE`] of the exact map.
fn bend_piece(bend: &Bend, c: CubicBez, seg: usize, depth: u32, out: &mut Vec<CubicBez>) {
    let f = |t: f64| bend.map(c.eval(t), seg);
    const H: f64 = 1e-4;
    let (q0, q3) = (f(0.0), f(1.0));
    let d0 = (f(H) - q0) / H;
    let d1 = (q3 - f(1.0 - H)) / H;
    let q = CubicBez::new(q0, q0 + d0 / 3.0, q3 - d1 / 3.0, q3);
    let strays = [0.25, 0.5, 0.75]
        .iter()
        .any(|t| q.eval(*t).distance(f(*t)) > TOLERANCE);
    if strays && depth < MAX_DEPTH {
        bend_piece(bend, c.subsegment(0.0..0.5), seg, depth + 1, out);
        bend_piece(bend, c.subsegment(0.5..1.0), seg, depth + 1, out);
    } else {
        out.push(q);
    }
}

/// The brush's runs bent along one track, appended to `out`. Where the path turns
/// a corner the two sides of the brush part; a straight line joins them.
pub(crate) fn bend_onto(brush: &[Run], bounds: Rect, track: &Track, art: Art, out: &mut BezPath) {
    let bend = Bend::new(track, bounds, art);
    let breaks = bend.breaks();
    for run in brush {
        let mut cubics = Vec::new();
        for c in &run.segs {
            for piece in cut_at_breaks(*c, &breaks) {
                let seg = bend.seg_for(&piece);
                bend_piece(&bend, piece, seg, 0, &mut cubics);
            }
        }
        let Some(first) = cubics.first() else {
            continue;
        };
        out.move_to(first.p0);
        let mut at = first.p0;
        for q in &cubics {
            if q.p0.distance(at) > 1e-9 {
                out.line_to(q.p0);
            }
            out.curve_to(q.p1, q.p2, q.p3);
            at = q.p3;
        }
        if run.closed {
            out.close_path();
        }
    }
}

/// Where each scattered copy goes along `track`: the arc lengths of their
/// middles. An open track spreads them a step apart, the leftover split between
/// its ends; a closed one fits a whole number round its loop.
pub(crate) fn scatter_places(track: &Track, step: f64) -> Vec<f64> {
    let len = track.total;
    if track.closed {
        let n = ((len / step).round() as usize).max(1);
        (0..n).map(|k| len * k as f64 / n as f64).collect()
    } else {
        let n = (len / step + 1e-9).floor() as usize + 1;
        let lead = (len - (n - 1) as f64 * step) / 2.0;
        (0..n).map(|k| lead + k as f64 * step).collect()
    }
}

/// The world maps that take the brush, whose box is `bounds`, onto each copy of
/// a scatter along `tracks`, or [`Refusal::TooMany`].
pub(crate) fn scatter_maps(
    tracks: &[Track],
    bounds: Rect,
    sc: Scatter,
) -> Result<Vec<Affine>, Refusal> {
    let length = bounds.width();
    let step = (sc.spacing * length).max(length * 0.01);
    let places: Vec<(usize, f64)> = tracks
        .iter()
        .enumerate()
        .flat_map(|(i, t)| scatter_places(t, step).into_iter().map(move |s| (i, s)))
        .collect();
    if places.len() > MAX_COPIES {
        return Err(Refusal::TooMany);
    }
    let mut rng = crate::content_fill::Rng::new(&[u64::from(sc.seed)]);
    let centre = bounds.center();
    Ok(places
        .into_iter()
        .map(|(i, s)| {
            // Both draws for every copy, so one setting at zero keeps the other's run.
            let (r_size, r_side) = (rng.unit() * 2.0 - 1.0, rng.unit() * 2.0 - 1.0);
            let scale = (sc.size * (1.0 + sc.size_varies * r_size)).max(0.01);
            let (at, t) = tracks[i].at(s);
            let at = at + normal(t) * (sc.scatter * length * r_side);
            let angle = match sc.turn {
                Turn::Along => t.atan2(),
                Turn::Fixed => 0.0,
            };
            Affine::translate(at.to_vec2())
                * Affine::rotate(angle)
                * Affine::scale(scale)
                * Affine::translate(-centre.to_vec2())
        })
        .collect())
}

/// What a live brush draws along a layer (`crate::live_brush`): the bake's
/// geometry, kept in the path layer's own space so moving the layer moves it.
#[derive(Clone, Debug, PartialEq)]
pub struct Ink {
    /// Art's bent brush in the path layer's space, or Scatter's brush outline
    /// in the brush's own space.
    pub path: BezPath,
    /// Where `path` is drawn in the path layer's space: the identity for Art,
    /// one map per copy for Scatter.
    pub places: Vec<Affine>,
    /// What the brush's stroke widths are multiplied by for `path`'s units.
    pub widths: f64,
    /// The map carrying the brush's gradients from its own space onto `path`.
    pub gradients: Affine,
}

/// The ink a brush outlined by `brush_path` under `brush_world` draws along a
/// layer outlined by `layer_path` under `layer_world`, as [`apply`] would bake
/// it, or why it draws none.
pub fn ink(
    layer_world: Affine,
    layer_path: &BezPath,
    brush_world: Affine,
    brush_path: &BezPath,
    mode: Mode,
) -> Result<Ink, Refusal> {
    let brush_runs = runs(&(brush_world * brush_path.clone()));
    let bounds = (brush_world * brush_path.clone()).bounding_box();
    if brush_runs.is_empty() || bounds.width() <= 1e-6 {
        return Err(Refusal::Flat);
    }
    if layer_world.determinant().abs() < 1e-12 {
        return Err(Refusal::Length);
    }
    let tracks: Vec<Track> = runs(&(layer_world * layer_path.clone()))
        .into_iter()
        .filter_map(Track::new)
        .collect();
    if tracks.is_empty() {
        return Err(Refusal::Length);
    }
    let to_local = layer_world.inverse();
    let size = |w: Affine| w.determinant().abs().sqrt();
    match mode {
        Mode::Art(art) => {
            let mut world = BezPath::new();
            for track in &tracks {
                bend_onto(&brush_runs, bounds, track, art, &mut world);
            }
            let onto = box_onto(bounds, world.bounding_box());
            Ok(Ink {
                path: to_local * world,
                places: vec![Affine::IDENTITY],
                widths: size(brush_world) * size(to_local),
                gradients: to_local * onto * brush_world,
            })
        }
        Mode::Scatter(sc) => Ok(Ink {
            path: brush_path.clone(),
            places: scatter_maps(&tracks, bounds, sc)?
                .into_iter()
                .map(|m| to_local * m * brush_world)
                .collect(),
            widths: 1.0,
            gradients: Affine::IDENTITY,
        }),
    }
}

/// The map of world space taking box `from` onto box `to`: centre onto centre,
/// each side scaled to the other's, a side too thin to scale keeping its scale.
fn box_onto(from: Rect, to: Rect) -> Affine {
    let ratio = |a: f64, b: f64| match a > 1e-9 && b > 1e-9 {
        true => b / a,
        false => 1.0,
    };
    Affine::translate(to.center().to_vec2())
        * Affine::scale_non_uniform(
            ratio(from.width(), to.width()),
            ratio(from.height(), to.height()),
        )
        * Affine::translate(-from.center().to_vec2())
}

/// A resolved `paint` laid on new geometry: stroke widths times `k`, unbound,
/// and every literal gradient carried by `by`.
pub fn carried(mut paint: crate::Paint, k: f64, by: Affine) -> crate::Paint {
    let carry = |src: &mut crate::variable::BrushSource| {
        if let Some(b) = src.literal() {
            *src = crate::variable::BrushSource::Literal(crate::paint_box::carry_brush(b, by));
        }
    };
    for f in &mut paint.fills {
        carry(&mut f.value.brush);
    }
    for s in &mut paint.strokes {
        s.value.width *= k;
        s.value.width_var = None;
        carry(&mut s.value.brush);
    }
    paint
}

/// What one new path draws, as the brush did: its paint resolved and literal,
/// stroke widths scaled by `k` and gradients carried by `by` (the brush's local
/// space into the new path's), its fill rule, effects, opacity and mode.
fn look_ops(doc: &Document, brush: NodeId, id: NodeId, (k, by): (f64, Affine)) -> Vec<Operation> {
    let Some(node) = doc.get(brush) else {
        return Vec::new();
    };
    let paint = carried(doc.resolved_paint(brush, node.paint()).into_owned(), k, by);
    let mut ops = vec![
        Operation::SetFillRule {
            id,
            rule: node.fill_rule(),
        },
        Operation::SetFills {
            id,
            fills: paint.fills,
        },
        Operation::SetStrokes {
            id,
            strokes: paint.strokes,
        },
        Operation::SetEffects {
            id,
            effects: doc.resolved_effects(brush, node.effects()).into_owned(),
        },
    ];
    let opacity = doc.drawn_opacity(brush);
    if opacity != 1.0 {
        ops.push(Operation::SetOpacity { id, opacity });
    }
    if node.blend() != crate::BlendMode::default_for(node.kind()) {
        ops.push(Operation::SetBlend {
            id,
            blend: node.blend(),
        });
    }
    ops
}

/// The group's name for a brush named `brush`.
pub fn group_name(brush: &str) -> String {
    format!("{GROUP_PREFIX} · {brush}")
}

/// `how` applied: the [`Operation::InsertSubtree`] of the `Brushed · ‹brush›`
/// group where the path stands in its parent, holding the new paths, then the
/// path deleted, or moved into the group's bottom hidden and named
/// [`KEPT_PATH_NAME`]. Answers the edit and the group.
pub fn apply(
    doc: &Document,
    res: &Resolved,
    ids: &mut IdSource,
    how: &ApplyBrush,
) -> Result<(Transaction, NodeId), Refusal> {
    if let Some(r) = refusal(doc, res, how.path, how.brush) {
        return Err(r);
    }
    build(doc, res, ids, how)
}

/// [`apply`] without its refusals: a live brush's bake
/// (`crate::live_brush::bake`) holds a path no bake is offered (in an auto
/// layout, a mask), whose look it must keep all the same.
pub(crate) fn build(
    doc: &Document,
    res: &Resolved,
    ids: &mut IdSource,
    how: &ApplyBrush,
) -> Result<(Transaction, NodeId), Refusal> {
    let path_node = doc.get(how.path).ok_or(Refusal::Count)?;
    let brush_node = doc.get(how.brush).ok_or(Refusal::Count)?;
    let parent = path_node.parent().ok_or(Refusal::Count)?;
    let at = doc
        .get(parent)
        .and_then(|p| p.children().iter().position(|c| *c == how.path))
        .ok_or(Refusal::Count)?;
    let to_parent = res
        .world_transform(parent)
        .unwrap_or(Affine::IDENTITY)
        .inverse();
    let brush_world = res.world_transform(how.brush).ok_or(Refusal::Count)?;
    let (brush_runs, bounds) = outline(doc, res, how.brush).ok_or(Refusal::Flat)?;
    let tracks = tracks(doc, res, how.path);
    // How much larger a unit of the brush draws than a unit of the new paths.
    let size = |w: Affine| w.determinant().abs().sqrt();
    let unit = size(to_parent);

    // Each new path in world space, how much larger a unit of the brush draws in
    // it than in the world, and the map of the brush's local space onto it.
    let made: Vec<(BezPath, f64, Affine)> = match how.mode {
        Mode::Art(art) => {
            let mut world = BezPath::new();
            for track in &tracks {
                bend_onto(&brush_runs, bounds, track, art, &mut world);
            }
            // A gradient is laid over the bent box as it lay over the brush's; a
            // stroke keeps its width however far the brush stretches.
            let onto = box_onto(bounds, world.bounding_box());
            vec![(world, size(brush_world), onto * brush_world)]
        }
        Mode::Scatter(sc) => {
            let mut brush_path = BezPath::new();
            for run in &brush_runs {
                brush_path.move_to(run.segs[0].p0);
                for c in &run.segs {
                    brush_path.curve_to(c.p1, c.p2, c.p3);
                }
                if run.closed {
                    brush_path.close_path();
                }
            }
            scatter_maps(&tracks, bounds, sc)?
                .into_iter()
                .map(|m| {
                    (
                        m * brush_path.clone(),
                        size(m * brush_world),
                        m * brush_world,
                    )
                })
                .collect()
        }
    };

    // Built on a scratch document, so each node takes every default a created
    // one has, then carried over whole.
    let root = ids.mint();
    let mut scratch = Document::new(root);
    let page = scratch.first_page();
    let group = ids.mint();
    let mut ops = vec![Operation::CreateNode {
        id: group,
        parent: page,
        index: 0,
        kind: NodeKind::Group,
        transform: None,
        name: Some(group_name(brush_node.name())),
    }];
    for (index, (world, scale, local)) in made.into_iter().enumerate() {
        let id = ids.mint();
        ops.push(Operation::CreateNode {
            id,
            parent: group,
            index,
            kind: NodeKind::Path {
                path: to_parent * world,
                corner_radii: Vec::new(),
                corner_smoothing: 0.0,
                mirror: crate::symmetry::PathMirror::NONE,
            },
            transform: None,
            name: Some(brush_node.name().to_owned()),
        });
        let carry = (scale * unit, to_parent * local);
        ops.extend(look_ops(doc, how.brush, id, carry));
    }
    scratch
        .apply(&Transaction(ops))
        .map_err(|_| Refusal::Count)?;
    let nodes = scratch.capture_subtree(group).ok_or(Refusal::Count)?;
    let mut ops = vec![Operation::InsertSubtree {
        nodes,
        parent,
        index: at,
    }];
    if how.keep_path {
        // The group has no transform and stands in the path's parent, so the path
        // keeps its own transform inside it.
        ops.push(Operation::Reparent {
            id: how.path,
            new_parent: group,
            index: 0,
        });
        ops.push(Operation::SetVisible {
            id: how.path,
            visible: false,
        });
        ops.push(Operation::SetName {
            id: how.path,
            name: KEPT_PATH_NAME.into(),
        });
    } else {
        ops.push(Operation::DeleteNode { id: how.path });
    }
    Ok((Transaction(ops), group))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn line(from: (f64, f64), to: (f64, f64)) -> Track {
        let mut p = BezPath::new();
        p.move_to(from);
        p.line_to(to);
        Track::new(runs(&p).remove(0)).expect("a line has length")
    }

    fn rect_runs(r: Rect) -> Vec<Run> {
        runs(&r.to_path(0.1))
    }

    fn bent(brush: &[Run], bounds: Rect, track: &Track, art: Art) -> BezPath {
        let mut out = BezPath::new();
        bend_onto(brush, bounds, track, art, &mut out);
        out
    }

    /// Along a straight line twice its length a 100 × 20 box spans the line end to
    /// end and keeps its height about it; *Width* scales the height alone.
    /// flip: `Bend::new` leaving `across` at 1 → fails at "half as tall".
    #[test]
    fn art_spans_a_line_and_width_scales_across() {
        let track = line((0.0, 50.0), (200.0, 50.0));
        let bounds = Rect::new(10.0, 0.0, 110.0, 20.0);
        let brush = rect_runs(bounds);
        let full = bent(&brush, bounds, &track, Art::DEFAULT).bounding_box();
        assert!((full.x0 - 0.0).abs() < 1e-6 && (full.x1 - 200.0).abs() < 1e-6);
        assert!((full.y0 - 40.0).abs() < 1e-6 && (full.y1 - 60.0).abs() < 1e-6);
        let half = Art {
            width: 0.5,
            ..Art::DEFAULT
        };
        let half = bent(&brush, bounds, &track, half).bounding_box();
        assert!(
            (half.height() - 10.0).abs() < 1e-6,
            "half as tall: {half:?}"
        );
        assert!((half.width() - 200.0).abs() < 1e-6);
    }

    /// *Flip across* mirrors the brush over the path: a wedge whose point is on top
    /// has it underneath.
    /// flip: `Bend::new` ignoring `flip` → fails at "the point below".
    #[test]
    fn flip_mirrors_across_the_path() {
        let track = line((0.0, 0.0), (100.0, 0.0));
        let mut wedge = BezPath::new();
        wedge.move_to((0.0, 10.0));
        wedge.line_to((50.0, -10.0));
        wedge.line_to((100.0, 10.0));
        wedge.close_path();
        let bounds = wedge.bounding_box();
        let brush = runs(&wedge);
        // The anchor the wedge's point lands on, by its height.
        let tip = |art: Art| {
            let out = bent(&brush, bounds, &track, art);
            out.elements()
                .iter()
                .find_map(|el| match el {
                    PathEl::CurveTo(_, _, p) | PathEl::LineTo(p) if (p.x - 50.0).abs() < 1e-6 => {
                        Some(p.y)
                    }
                    _ => None,
                })
                .expect("an anchor at the point")
        };
        assert!(tip(Art::DEFAULT) < 0.0, "the point above");
        let flipped = Art {
            flip: true,
            ..Art::DEFAULT
        };
        assert!(tip(flipped) > 0.0, "the point below");
    }

    /// *Middle only* keeps the brush's first and last fifth at their length (times
    /// the width) and stretches only the middle.
    /// flip: `Bend::new` building `Stretch::Middle` like `Whole` → fails at "the
    /// cap keeps its length".
    #[test]
    fn middle_only_keeps_the_ends() {
        let track = line((0.0, 0.0), (300.0, 0.0));
        let bounds = Rect::new(0.0, 0.0, 100.0, 10.0);
        let middle = Art {
            stretch: Stretch::Middle,
            ..Art::DEFAULT
        };
        let bend = Bend::new(&track, bounds, middle);
        assert!(
            (bend.s_of(10.0) - 10.0).abs() < 1e-9,
            "the cap keeps its length"
        );
        assert!((bend.s_of(20.0) - 20.0).abs() < 1e-9);
        assert!((bend.s_of(90.0) - 290.0).abs() < 1e-9);
        assert!(
            (bend.s_of(50.0) - 150.0).abs() < 1e-9,
            "the middle stretches"
        );
        let whole = Bend::new(&track, bounds, Art::DEFAULT);
        assert!((whole.s_of(10.0) - 30.0).abs() < 1e-9);
        // A path shorter than both caps shares itself between them.
        let short = line((0.0, 0.0), (30.0, 0.0));
        let bend = Bend::new(&short, bounds, middle);
        assert!((bend.s_of(20.0) - 15.0).abs() < 1e-9);
    }

    /// Bent round a closed circle, every point of the result lies between the
    /// circle's radius less and plus half the brush's height, and the result is
    /// made of curves, not a polyline.
    #[test]
    fn art_round_a_circle_stays_in_its_band() {
        let circle = kurbo::Circle::new((0.0, 0.0), 100.0).to_path(1e-3);
        let track = Track::new(runs(&circle).remove(0)).unwrap();
        assert!(track.closed);
        let bounds = Rect::new(0.0, -5.0, 50.0, 5.0);
        let out = bent(&rect_runs(bounds), bounds, &track, Art::DEFAULT);
        let mut curves = 0;
        for seg in out.segments() {
            if matches!(seg, PathSeg::Cubic(_)) {
                curves += 1;
            }
            for k in 0..=8 {
                let r = seg.eval(k as f64 / 8.0).to_vec2().hypot();
                assert!(
                    (95.0 - 0.05..=105.0 + 0.05).contains(&r),
                    "within the band: {r}"
                );
            }
        }
        assert!(curves > 8, "curves, {curves} of them");
    }

    /// Bent over one strongly curved segment, the result keeps within half the
    /// brush's height of the segment: the map is met by halving brush pieces.
    /// flip: `bend_piece` never halving → fails at "within the band".
    #[test]
    fn a_tight_curve_is_met_by_halving() {
        use kurbo::ParamCurveNearest;
        let arch = CubicBez::new((0.0, 0.0), (0.0, -150.0), (200.0, -150.0), (200.0, 0.0));
        let track = Track::new(Run {
            segs: vec![arch],
            closed: false,
        })
        .unwrap();
        let bounds = Rect::new(0.0, -5.0, 100.0, 5.0);
        let out = bent(&rect_runs(bounds), bounds, &track, Art::DEFAULT);
        for seg in out.segments() {
            for k in 0..=16 {
                let q = seg.eval(k as f64 / 16.0);
                let d = arch.nearest(q, 1e-6).distance_sq.sqrt();
                assert!(d <= 5.0 + TOLERANCE * 2.0, "within the band: {d} at {q:?}");
            }
        }
    }

    /// A brush of two subpaths, a ring, bends into two subpaths.
    #[test]
    fn a_ring_bends_into_two_subpaths() {
        let track = line((0.0, 0.0), (300.0, 0.0));
        let mut ring = Rect::new(0.0, 0.0, 100.0, 40.0).to_path(0.1);
        ring.extend(Rect::new(20.0, 10.0, 80.0, 30.0).to_path(0.1));
        let bounds = ring.bounding_box();
        let out = bent(&runs(&ring), bounds, &track, Art::DEFAULT);
        let moves = out
            .elements()
            .iter()
            .filter(|e| matches!(e, PathEl::MoveTo(_)))
            .count();
        assert_eq!(moves, 2);
    }

    /// Where a path turns a corner the brush is cut there and read on each side's
    /// own segment: no point strays off the two legs' bands, and each leg carries
    /// its half of the brush's edge.
    /// flip: `Bend::breaks` without the track's anchors → fails at "the first leg
    /// carries its half".
    #[test]
    fn a_corner_cuts_the_brush() {
        let mut p = BezPath::new();
        p.move_to((0.0, 0.0));
        p.line_to((100.0, 0.0));
        p.line_to((100.0, 100.0));
        let track = Track::new(runs(&p).remove(0)).unwrap();
        let bounds = Rect::new(0.0, -5.0, 200.0, 5.0);
        let out = bent(&rect_runs(bounds), bounds, &track, Art::DEFAULT);
        let mut points = Vec::new();
        for seg in out.segments() {
            for k in 0..=32 {
                let q = seg.eval(k as f64 / 32.0);
                let on_first = q.x <= 100.0 + 1e-6 && q.y.abs() <= 5.0 + 1e-6;
                let on_second = (q.x - 100.0).abs() <= 5.0 + 1e-6 && q.y >= -1e-6;
                // The straight joins at the corner's outside.
                let join = q.x >= 100.0 - 1e-6 && q.y <= 1e-6;
                assert!(on_first || on_second || join, "off the legs: {q:?}");
                points.push(q);
            }
        }
        let near = |at: Point| points.iter().any(|q| q.distance(at) < 0.5);
        assert!(
            near(Point::new(50.0, -5.0)),
            "the first leg carries its half"
        );
        assert!(
            near(Point::new(105.0, 50.0)),
            "the second leg carries its half"
        );
    }

    /// A scatter along an open line spaces its copies a step apart, the leftover
    /// split between the ends; a closed track fits a whole number round it.
    #[test]
    fn scatter_places_spread_evenly() {
        let track = line((0.0, 0.0), (610.0, 0.0));
        let at = scatter_places(&track, 60.0);
        assert_eq!(at.len(), 11);
        assert!((at[0] - 5.0).abs() < 1e-9 && (at[10] - 605.0).abs() < 1e-9);
        let circle = kurbo::Circle::new((0.0, 0.0), 100.0).to_path(1e-3);
        let ring = Track::new(runs(&circle).remove(0)).unwrap();
        let at = scatter_places(&ring, 60.0);
        assert_eq!(at.len(), (ring.total / 60.0).round() as usize);
    }

    /// The same seed lays out the same copies; another seed moves them; with no
    /// variation every copy is `size` and sits on the path.
    /// flip: `scatter_maps` seeding from a constant → fails at "another seed".
    #[test]
    fn scatter_is_seeded() {
        let track = [line((0.0, 0.0), (600.0, 0.0))];
        let bounds = Rect::new(0.0, 0.0, 50.0, 20.0);
        let maps = |seed| {
            scatter_maps(
                &track,
                bounds,
                Scatter {
                    seed,
                    ..Scatter::DEFAULT
                },
            )
        };
        assert_eq!(maps(7).unwrap(), maps(7).unwrap());
        assert_ne!(maps(7).unwrap(), maps(8).unwrap(), "another seed");
        let calm = Scatter {
            size_varies: 0.0,
            scatter: 0.0,
            ..Scatter::DEFAULT
        };
        for m in scatter_maps(&track, bounds, calm).unwrap() {
            let b = m.transform_rect_bbox(bounds);
            assert!((b.width() - 40.0).abs() < 1e-6, "{b:?}");
            assert!(b.center().y.abs() < 1e-6);
        }
    }

    /// *Along the path* turns each copy with the path; *Fixed* leaves it facing as
    /// the brush does.
    #[test]
    fn scatter_turns_along_or_stays_fixed() {
        let track = [line((0.0, 0.0), (0.0, 600.0))];
        let bounds = Rect::new(0.0, 0.0, 50.0, 10.0);
        let calm = Scatter {
            size: 1.0,
            size_varies: 0.0,
            scatter: 0.0,
            ..Scatter::DEFAULT
        };
        let along = scatter_maps(&track, bounds, calm).unwrap()[0].transform_rect_bbox(bounds);
        assert!(along.height() > along.width(), "turned down the path");
        let fixed = Scatter {
            turn: Turn::Fixed,
            ..calm
        };
        let fixed = scatter_maps(&track, bounds, fixed).unwrap()[0].transform_rect_bbox(bounds);
        assert!(fixed.width() > fixed.height(), "facing as the brush does");
    }

    /// Too many copies are refused rather than made.
    #[test]
    fn too_many_copies_are_refused() {
        let track = [line((0.0, 0.0), (100_000.0, 0.0))];
        let bounds = Rect::new(0.0, 0.0, 10.0, 10.0);
        let r = scatter_maps(&track, bounds, Scatter::DEFAULT);
        assert_eq!(r, Err(Refusal::TooMany));
    }
}
