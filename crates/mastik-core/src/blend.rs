//! *Blend shapes*: the steps between two shapes, as plain paths with their outline,
//! paint and opacity interpolated, in a group named *Blend* that takes the two
//! shapes as well, one at each end.
//!
//! Computed once; nothing stays live. Both outlines are taken in world space and
//! cut into the same number of cubic segments, so step `k` of `n` is every control
//! point a fraction `k / (n + 1)` of the way from the first shape to the second.
//! Along a spine each, the two shapes included, then moves by its centre onto
//! its place, spaced evenly by arc length, and may turn with the spine.

use crate::build;
use crate::kurbo::{
    Affine, BezPath, CubicBez, Line, ParamCurve, ParamCurveArclen, ParamCurveDeriv,
    ParamCurveNearest, PathSeg, Point, Rect, Shape, Vec2,
};
use crate::node::NodeKind;
use crate::{Brush, Document, IdSource, NodeId, Operation, Resolved, Transaction};

/// The group the steps land in.
pub const GROUP_NAME: &str = "Blend";

/// The most steps a blend makes between its two shapes.
pub const MAX_STEPS: usize = 200;

/// How the two shapes are blended.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Blend {
    /// How many shapes go between the two.
    pub steps: usize,
    /// Fills, strokes and opacity move from one to the other; off, every step
    /// wears the first shape's.
    pub paint: bool,
    /// Where on each shape's box the first anchors pair up, as a fraction of the
    /// box: `(0, 0)` its top-left, `(0.5, 0.5)` its middle.
    pub start: Vec2,
    /// Run the second shape's outline the other way round.
    pub reverse: bool,
    /// The path the steps follow, from the first shape at its start to the second
    /// at its end; `None` for a straight line, or the path selected with the two.
    pub spine: Option<NodeId>,
    /// Run the spine from its end instead.
    pub spine_reverse: bool,
    /// Turn each step, the two shapes included, with the spine's tangent, measured
    /// from the tangent at its start. Inert on a straight line.
    pub turn: bool,
}

impl Blend {
    /// Five steps with paint blended, outlines paired from the top-left, on a
    /// straight line.
    pub const DEFAULT: Blend = Blend {
        steps: 5,
        paint: true,
        start: Vec2::ZERO,
        reverse: false,
        spine: None,
        spine_reverse: false,
        turn: false,
    };
}

/// Why two layers cannot be blended.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Refusal {
    /// Not two shapes, or two shapes and one path to follow.
    Count,
    /// The path to follow is not a path, is one of the two shapes, or has no length.
    Spine,
    Picture,
    Text,
    /// One shape is closed and the other open, or they have different numbers of
    /// parts, so their outlines do not pair.
    Parts,
    /// A shape is locked, or in a locked group.
    Locked,
    /// A shape sits in a boolean, a main or an instance, where a new group would
    /// change what that draws.
    Inside,
    /// The two sit in different groups or frames: moving one into the other's
    /// would put it under that one's clip, opacity, mode and layout.
    Parents,
    /// A shape is a mask: in the group it would stop clipping what it clips.
    Mask,
    /// A mask stands between the two in their parent: the lower shape, moved
    /// into the group above it, would fall under its clip.
    MaskBetween,
    /// The two sit in an auto layout, directly or through plain groups: moved
    /// into the group they would leave the flow and lose what it gave them.
    Layout,
}

impl Refusal {
    /// The sentence a dimmed row and a refused commit say.
    pub fn says(self) -> &'static str {
        match self {
            Refusal::Count => "Blend shapes needs two shapes, and a path to follow if you like",
            Refusal::Spine => "Blend shapes can only follow a path with some length to it",
            Refusal::Picture => "Blend shapes can't take a picture",
            Refusal::Text => "Blend shapes can't take text",
            Refusal::Parts => {
                "Blend shapes needs two shapes made of the same number of parts, both closed or both open"
            }
            Refusal::Locked => "A layer to blend is locked",
            Refusal::Inside => {
                "Blend shapes can't work inside a boolean, a component or an instance"
            }
            Refusal::Parents => "Blend shapes needs two shapes in the same group or frame",
            Refusal::Mask => "Blend shapes can't take a mask",
            Refusal::MaskBetween => "Blend shapes needs two shapes with no mask between them",
            Refusal::Layout => "Blend shapes can't work inside an auto layout",
        }
    }
}

/// The path among three selected layers that the other two follow: the one
/// [`NodeKind::Path`] of the three. `None` for any other selection, or when more
/// than one of the three is a path and which is the spine cannot be told.
pub fn spine_in(doc: &Document, selection: &[NodeId]) -> Option<NodeId> {
    let members = build::in_document_order(doc, selection);
    let [_, _, _] = members.as_slice() else {
        return None;
    };
    let mut paths = members.iter().filter(|id| {
        doc.get(**id)
            .is_some_and(|n| matches!(n.kind(), NodeKind::Path { .. }))
    });
    match (paths.next(), paths.next()) {
        (Some(spine), None) => Some(*spine),
        _ => None,
    }
}

/// Whether `id` may be moved by a blend: not locked, not a mask, and not in a
/// boolean, a main or an instance, where a new or moved layer would change what
/// that draws.
fn movable(doc: &Document, id: NodeId) -> Result<&crate::Node, Refusal> {
    let node = doc.get(id).ok_or(Refusal::Count)?;
    if crate::is_effectively_locked(doc, id) {
        return Err(Refusal::Locked);
    }
    let mut up = node.parent();
    while let Some(p) = up.and_then(|p| doc.get(p)) {
        if matches!(p.kind(), NodeKind::Boolean { .. }) || p.component() || p.link().is_some() {
            return Err(Refusal::Inside);
        }
        up = p.parent();
    }
    if node.link().is_some() {
        return Err(Refusal::Inside);
    }
    if node.mask() {
        return Err(Refusal::Mask);
    }
    Ok(node)
}

/// The two shapes `selection` blends, bottom of the stack first, or why not. Of
/// three layers, the path [`spine_in`] names is left out: it is followed, not
/// blended.
pub fn pair(doc: &Document, selection: &[NodeId]) -> Result<(NodeId, NodeId), Refusal> {
    let mut members = build::in_document_order(doc, selection);
    if let Some(spine) = spine_in(doc, selection) {
        members.retain(|m| *m != spine);
    }
    let [a, b] = members.as_slice() else {
        return Err(Refusal::Count);
    };
    for id in [a, b] {
        let node = movable(doc, *id)?;
        if node.paint().image_fill().is_some() {
            return Err(Refusal::Picture);
        }
        match node.kind() {
            NodeKind::Text { .. } => return Err(Refusal::Text),
            NodeKind::Rect { .. }
            | NodeKind::Ellipse { .. }
            | NodeKind::Polygon { .. }
            | NodeKind::Star { .. }
            | NodeKind::Line { .. }
            | NodeKind::Spiral { .. }
            | NodeKind::Arc { .. }
            | NodeKind::Path { .. }
            | NodeKind::Boolean { .. } => {}
            _ => return Err(Refusal::Count),
        }
    }
    // Grouping moves both into the upper one's parent, as `build::group` refuses
    // to across parents.
    let parent = doc.get(*a).and_then(|n| n.parent());
    if parent != doc.get(*b).and_then(|n| n.parent()) {
        return Err(Refusal::Parents);
    }
    if crate::container::lays_out_above(&crate::resolve::DocView(doc), *a) {
        return Err(Refusal::Layout);
    }
    // A mask clips the siblings above it, so one between the two would start
    // clipping the lower shape once the group above it holds that shape.
    let siblings = parent
        .and_then(|p| doc.get(p))
        .map_or(&[][..], |p| p.children());
    let at = |id: NodeId| siblings.iter().position(|c| *c == id);
    if let (Some(i), Some(j)) = (at(*a), at(*b))
        && siblings[i.min(j) + 1..i.max(j)]
            .iter()
            .any(|s| doc.get(*s).is_some_and(|n| n.mask()))
    {
        return Err(Refusal::MaskBetween);
    }
    Ok((*a, *b))
}

/// Why `selection` cannot be blended, if it cannot: [`pair`]'s refusals and the
/// outlines' (open against closed, a different number of parts), so a dimmed row
/// says what a commit would.
pub fn refusal(doc: &Document, res: &Resolved, selection: &[NodeId]) -> Option<Refusal> {
    let (a, b) = match pair(doc, selection) {
        Ok(pair) => pair,
        Err(r) => return Some(r),
    };
    if let Some(id) = spine_in(doc, selection)
        && let Err(r) = spine(doc, res, id, (a, b), false)
    {
        return Some(r);
    }
    outlines(doc, res, a, b).err()
}

/// Why `id` cannot be the path a blend of `a` and `b` follows, if it cannot.
pub fn spine_refusal(
    doc: &Document,
    res: &Resolved,
    id: NodeId,
    (a, b): (NodeId, NodeId),
) -> Option<Refusal> {
    spine(doc, res, id, (a, b), false).err()
}

/// The path a blend follows, in world space: its first subpath, measured.
struct Spine {
    run: Run,
    /// Each segment's arc length.
    lengths: Vec<f64>,
    total: f64,
}

/// `id` as the spine of a blend of `a` and `b`, run from its end when `reverse`.
/// It moves to stand under the blend, so it must be movable as the shapes are.
fn spine(
    doc: &Document,
    res: &Resolved,
    id: NodeId,
    (a, b): (NodeId, NodeId),
    reverse: bool,
) -> Result<Spine, Refusal> {
    if id == a || id == b {
        return Err(Refusal::Spine);
    }
    let node = movable(doc, id)?;
    if !matches!(node.kind(), NodeKind::Path { .. }) {
        return Err(Refusal::Spine);
    }
    if crate::container::lays_out_above(&crate::resolve::DocView(doc), id) {
        return Err(Refusal::Layout);
    }
    let world = res.world_transform(id).ok_or(Refusal::Spine)?
        * res.local_path(doc, id).ok_or(Refusal::Spine)?;
    let run = runs(&world).into_iter().next().ok_or(Refusal::Spine)?;
    let run = if reverse { reversed(&run) } else { run };
    let lengths: Vec<f64> = run.segs.iter().map(|c| c.arclen(ACCURACY)).collect();
    let total: f64 = lengths.iter().sum();
    if total < 1e-6 {
        return Err(Refusal::Spine);
    }
    Ok(Spine {
        run,
        lengths,
        total,
    })
}

impl Spine {
    /// The point `s` along the spine and the angle of its tangent there.
    fn at(&self, s: f64) -> (Point, f64) {
        let mut from = 0.0;
        let last = self.run.segs.len() - 1;
        for (i, (c, l)) in self.run.segs.iter().zip(&self.lengths).enumerate() {
            if s <= from + l || i == last {
                let t = match *l > 1e-12 {
                    true => c.inv_arclen((s - from).clamp(0.0, *l), ACCURACY),
                    false => 0.0,
                };
                let d = c.deriv().eval(t).to_vec2();
                // A handle on its anchor has no derivative there: the chord says
                // which way the segment runs.
                let d = match d.hypot2() > 1e-18 {
                    true => d,
                    false => c.p3 - c.p0,
                };
                return (c.eval(t), d.atan2());
            }
            from += l;
        }
        (self.run.segs[0].p0, 0.0)
    }

    /// Where each of `count` items stands, first to last: evenly by arc length
    /// from end to end, or over a closed spine's whole loop with the last one gap
    /// short of the start, as a ring's copies are. Each with its turn from the
    /// first, zero unless `turn`.
    fn places(&self, count: usize, turn: bool) -> Vec<(Point, f64)> {
        let gaps = match self.run.closed {
            true => count,
            false => count.saturating_sub(1).max(1),
        };
        let at: Vec<(Point, f64)> = (0..count)
            .map(|k| self.at(self.total * k as f64 / gaps as f64))
            .collect();
        let first = at.first().map_or(0.0, |(_, a)| *a);
        at.into_iter()
            .map(|(p, a)| (p, if turn { a - first } else { 0.0 }))
            .collect()
    }
}

/// The map of world space that takes something centred on `centre` to stand on
/// `place`, turned by its angle about it.
fn placed(centre: Point, (place, angle): (Point, f64)) -> Affine {
    Affine::rotate_about(angle, place) * Affine::translate(place - centre)
}

/// One subpath as cubic segments, and whether it closes.
#[derive(Clone, Debug)]
struct Run {
    segs: Vec<CubicBez>,
    closed: bool,
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
fn runs(path: &BezPath) -> Vec<Run> {
    crate::geometry::subpaths(path)
        .into_iter()
        .filter_map(|sub| {
            let closed = matches!(sub.elements().last(), Some(kurbo::PathEl::ClosePath));
            let segs: Vec<CubicBez> = sub
                .segments()
                .map(cubic)
                .filter(|c| c.p0.distance(c.p3) > 1e-9 || c.p0.distance(c.p1) > 1e-9)
                .collect();
            (!segs.is_empty()).then_some(Run { segs, closed })
        })
        .collect()
}

/// The signed area a closed run encloses, curves included: its sign is which
/// way it turns, even for a lens of two segments whose anchors enclose nothing.
fn turning(run: &Run) -> f64 {
    let mut path = BezPath::new();
    path.move_to(run.segs[0].p0);
    for c in &run.segs {
        path.curve_to(c.p1, c.p2, c.p3);
    }
    path.close_path();
    path.area()
}

fn reversed(run: &Run) -> Run {
    Run {
        segs: run
            .segs
            .iter()
            .rev()
            .map(|c| CubicBez::new(c.p3, c.p2, c.p1, c.p0))
            .collect(),
        closed: run.closed,
    }
}

/// How finely an arc length is measured.
const ACCURACY: f64 = 1e-3;

/// A closed run started at the point of its outline nearest `at`, the segment
/// there cut in two.
fn started_near(run: &Run, at: Point) -> Run {
    let (i, t) = run
        .segs
        .iter()
        .enumerate()
        .map(|(i, c)| (i, c.nearest(at, ACCURACY)))
        .min_by(|(_, a), (_, b)| a.distance_sq.total_cmp(&b.distance_sq))
        .map_or((0, 0.0), |(i, n)| (i, n.t));
    let c = run.segs[i];
    let mut segs = Vec::with_capacity(run.segs.len() + 1);
    if t < 1.0 - 1e-9 {
        segs.push(c.subsegment(t..1.0));
    }
    segs.extend_from_slice(&run.segs[i + 1..]);
    segs.extend_from_slice(&run.segs[..i]);
    if t > 1e-9 {
        segs.push(c.subsegment(0.0..t));
    }
    Run {
        segs,
        closed: run.closed,
    }
}

/// Where each anchor after the first falls along `run`, as a fraction of its
/// length; the last is 1.
fn fractions(run: &Run) -> Vec<f64> {
    let lengths: Vec<f64> = run.segs.iter().map(|c| c.arclen(ACCURACY)).collect();
    let total: f64 = lengths.iter().sum::<f64>().max(1e-9);
    let mut at = 0.0;
    lengths
        .iter()
        .map(|l| {
            at += l;
            at / total
        })
        .collect()
}

/// `run` cut wherever `cuts` (fractions of its length, ascending) fall inside a
/// segment, so every fraction becomes an anchor and its own anchors stay.
fn cut_at(run: &Run, cuts: &[f64]) -> Run {
    let lengths: Vec<f64> = run.segs.iter().map(|c| c.arclen(ACCURACY)).collect();
    let total: f64 = lengths.iter().sum::<f64>().max(1e-9);
    let mut segs = Vec::new();
    let mut from = 0.0;
    for (c, l) in run.segs.iter().zip(&lengths) {
        let to = from + l / total;
        let mut t0 = 0.0;
        for &f in cuts.iter().filter(|f| **f > from + 1e-9 && **f < to - 1e-9) {
            let t = c.inv_arclen((f - from) * total, ACCURACY);
            segs.push(c.subsegment(t0..t));
            t0 = t;
        }
        segs.push(c.subsegment(t0..1.0));
        from = to;
    }
    Run {
        segs,
        closed: run.closed,
    }
}

/// `x` and `y` cut at each other's anchors, by fraction of length: the same
/// number of segments, and a corner of one meets the point as far round the other.
fn paired(x: &Run, y: &Run) -> (Run, Run) {
    let mut cuts: Vec<f64> = fractions(x).into_iter().chain(fractions(y)).collect();
    cuts.sort_by(f64::total_cmp);
    cuts.dedup_by(|a, b| (*a - *b).abs() < 1e-6);
    let (x, y) = (cut_at(x, &cuts), cut_at(y, &cuts));
    // Rounding at a near-coincident pair can leave one a segment short; pad it
    // with a point segment at its end rather than mismatch.
    let pad = |mut r: Run, n: usize| {
        while r.segs.len() < n {
            let p = r.segs.last().map_or(Point::ZERO, |c| c.p3);
            r.segs.push(CubicBez::new(p, p, p, p));
        }
        r
    };
    let n = x.segs.len().max(y.segs.len());
    (pad(x, n), pad(y, n))
}

/// The outline between `a` and `b` at `t`, segment by segment.
fn between(a: &[Run], b: &[Run], t: f64) -> BezPath {
    let mut out = BezPath::new();
    for (ra, rb) in a.iter().zip(b) {
        let lerp = |p: Point, q: Point| p.lerp(q, t);
        let first = (ra.segs[0], rb.segs[0]);
        out.move_to(lerp(first.0.p0, first.1.p0));
        for (ca, cb) in ra.segs.iter().zip(&rb.segs) {
            out.curve_to(lerp(ca.p1, cb.p1), lerp(ca.p2, cb.p2), lerp(ca.p3, cb.p3));
        }
        if ra.closed {
            out.close_path();
        }
    }
    out
}

/// One shape's outline in world space, as runs, and its box.
type Outline = (Vec<Run>, Rect);

/// The two shapes' outlines in world space, or [`Refusal::Parts`] when their runs
/// do not pair: a different number, or one closed against one open.
fn outlines(
    doc: &Document,
    res: &Resolved,
    a: NodeId,
    b: NodeId,
) -> Result<(Outline, Outline), Refusal> {
    let outline = |id: NodeId| -> Option<Outline> {
        let world = res.world_transform(id)? * res.local_path(doc, id)?;
        let bounds = world.bounding_box();
        Some((runs(&world), bounds))
    };
    let (ra, box_a) = outline(a).ok_or(Refusal::Count)?;
    let (rb, box_b) = outline(b).ok_or(Refusal::Count)?;
    if ra.is_empty()
        || ra.len() != rb.len()
        || ra.iter().zip(&rb).any(|(x, y)| x.closed != y.closed)
    {
        return Err(Refusal::Parts);
    }
    Ok(((ra, box_a), (rb, box_b)))
}

/// The two outlines' runs, paired, and for each pair whether the second shape's
/// run now goes against its own direction.
type Matched = (Vec<Run>, Vec<Run>, Vec<bool>);

/// The two outlines, in world space, paired run for run and cut to the same
/// segments, ready to be stepped between.
fn matched(
    doc: &Document,
    res: &Resolved,
    a: NodeId,
    b: NodeId,
    blend: Blend,
) -> Result<Matched, Refusal> {
    let ((ra, box_a), (rb, box_b)) = outlines(doc, res, a, b)?;
    let corner = |r: Rect| {
        Point::new(
            r.x0 + r.width() * blend.start.x,
            r.y0 + r.height() * blend.start.y,
        )
    };
    let (mut out_a, mut out_b, mut turned) = (Vec::new(), Vec::new(), Vec::new());
    for (x, y) in ra.iter().zip(&rb) {
        let (mut x, mut y) = (x.clone(), y.clone());
        let mut against = blend.reverse;
        if x.closed {
            // Turned the same way as the first, so the steps do not pass through
            // themselves; *Reverse* still turns it back by hand.
            if turning(&x).signum() != turning(&y).signum() {
                y = reversed(&y);
                against = !against;
            }
            if blend.reverse {
                y = reversed(&y);
            }
            x = started_near(&x, corner(box_a));
            y = started_near(&y, corner(box_b));
        } else if blend.reverse {
            y = reversed(&y);
        }
        let (x, y) = paired(&x, &y);
        out_a.push(x);
        out_b.push(y);
        turned.push(against);
    }
    Ok((out_a, out_b, turned))
}

/// `a` and `b` mixed `t` of the way, channel by channel, alpha included.
fn mix(a: crate::peniko::Color, b: crate::peniko::Color, t: f32) -> crate::peniko::Color {
    let (p, q) = (a.components, b.components);
    crate::peniko::Color::new(std::array::from_fn(|i| p[i] + (q[i] - p[i]) * t))
}

/// How one shape's paint is carried into the steps' own space: the factor its
/// stroke widths grow by, and the map from its local space to the steps'.
type Carry = (f64, Affine);

/// The look of step `t`: each fill and stroke the two share by position mixed
/// where both are solid colours, else the nearer shape's; widths and opacity
/// mixed. Literal throughout, as the steps are unbound. Each shape's [`Carry`]
/// brings its stroke widths and gradients into the steps' units, so a stroke draws
/// as wide as it did and a gradient lies over the step as it lay over the shape.
fn look_between(
    doc: &Document,
    (a, b): (NodeId, NodeId),
    t: f64,
    paint: bool,
    carry: (Carry, Carry),
) -> (crate::Paint, f32) {
    let (na, nb) = (doc.get(a), doc.get(b));
    let resolved = |n: Option<&crate::Node>, id, (k, by): Carry| {
        let mut p = n
            .map(|n| doc.resolved_paint(id, n.paint()).into_owned())
            .unwrap_or_default();
        let carried = |src: &mut crate::variable::BrushSource| {
            if let Some(brush) = src.literal() {
                *src =
                    crate::variable::BrushSource::Literal(crate::paint_box::carry_brush(brush, by));
            }
        };
        for f in &mut p.fills {
            carried(&mut f.value.brush);
        }
        for s in &mut p.strokes {
            s.value.width *= k;
            s.value.width_var = None;
            carried(&mut s.value.brush);
        }
        p
    };
    let (pa, pb) = (resolved(na, a, carry.0), resolved(nb, b, carry.1));
    let (oa, ob) = (doc.drawn_opacity(a), doc.drawn_opacity(b));
    if !paint {
        return (pa, oa);
    }
    let t32 = t as f32;
    let near = if t < 0.5 { &pa } else { &pb };
    let mut out = near.clone();
    let solid = |s: &crate::variable::BrushSource| match s.literal() {
        Some(Brush::Solid(c)) => Some(*c),
        _ => None,
    };
    if pa.fills.len() == pb.fills.len() {
        for (i, f) in out.fills.iter_mut().enumerate() {
            if let (Some(x), Some(y)) = (solid(&pa.fills[i].brush), solid(&pb.fills[i].brush)) {
                f.value.brush = crate::variable::BrushSource::Literal(Brush::Solid(mix(x, y, t32)));
            }
        }
    }
    if pa.strokes.len() == pb.strokes.len() {
        for (i, s) in out.strokes.iter_mut().enumerate() {
            let (x, y) = (&pa.strokes[i], &pb.strokes[i]);
            s.value.width = x.width + (y.width - x.width) * t;
            if let (Some(cx), Some(cy)) = (solid(&x.brush), solid(&y.brush)) {
                s.value.brush =
                    crate::variable::BrushSource::Literal(Brush::Solid(mix(cx, cy, t32)));
            }
        }
    }
    (out, oa + (ob - oa) * t32)
}

/// The map of world space taking box `from` onto box `to`: centre onto centre,
/// each side scaled to the other's. A side too thin to scale from or to keeps
/// its scale, so a line's box maps without a singular transform.
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

/// The blend of `selection`'s two shapes: the [`Operation::InsertSubtree`] that
/// adds the *Blend* group where the upper of the two stands in its parent, then
/// the two moved into it around the steps, first shape lowest. Answers the edit
/// and the group.
pub fn blend(
    doc: &Document,
    res: &Resolved,
    ids: &mut IdSource,
    selection: &[NodeId],
    blend: Blend,
) -> Result<(Transaction, NodeId), Refusal> {
    let (a, b) = pair(doc, selection)?;
    if blend.steps == 0 || blend.steps > MAX_STEPS {
        return Err(Refusal::Count);
    }
    let (ra, rb, turned) = matched(doc, res, a, b, blend)?;
    // The steps that take the second shape's fill rule run each of its turned runs
    // its own way again, so the rule meets the windings it was written for.
    let unturned = |runs: &[Run]| -> Vec<Run> {
        runs.iter()
            .zip(&turned)
            .map(|(r, t)| if *t { reversed(r) } else { r.clone() })
            .collect()
    };
    let (ra_b, rb_b) = (unturned(&ra), unturned(&rb));
    // Along a spine, item `k` (the first shape 0, the second `steps + 1`) is moved
    // from where the straight blend puts it to stand, by its centre, on its place.
    let followed = blend
        .spine
        .map(|s| spine(doc, res, s, (a, b), blend.spine_reverse))
        .transpose()?;
    let places = followed
        .as_ref()
        .map(|s| s.places(blend.steps + 2, blend.turn));
    let place_of = |k: usize, centre: Point| {
        places
            .as_ref()
            .map_or(Affine::IDENTITY, |p| placed(centre, p[k]))
    };
    let upper = doc.get(b).ok_or(Refusal::Count)?;
    let parent = upper.parent().ok_or(Refusal::Count)?;
    let at = doc
        .get(parent)
        .and_then(|p| p.children().iter().position(|c| *c == b))
        .ok_or(Refusal::Count)?;
    let parent_world = res.world_transform(parent).unwrap_or(Affine::IDENTITY);
    let to_parent = parent_world.inverse();
    // How much larger a unit of each shape draws than a unit of the steps.
    let size = |w: Affine| w.determinant().abs().sqrt();
    let world_box = |id: NodeId| {
        let world = res.world_transform(id).unwrap_or(Affine::IDENTITY);
        let bounds = res.local_path(doc, id).map(|p| (world * p).bounding_box());
        (world, bounds)
    };
    let (shape_a, shape_b) = (world_box(a), world_box(b));
    // Each shape's paint into a step's space, laid over the step's world box as
    // it lay over the shape's, so a gradient moves and sizes with the step, and
    // then placed with it.
    let carry = |(world, bounds): (Affine, Option<Rect>), step: Rect, place: Affine| -> Carry {
        let onto = bounds.map_or(Affine::IDENTITY, |from| box_onto(from, step));
        (
            size(world) / size(parent_world).max(1e-12),
            to_parent * place * onto * world,
        )
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
        name: Some(GROUP_NAME.into()),
    }];
    for k in 1..=blend.steps {
        let t = k as f64 / (blend.steps + 1) as f64;
        let step = ids.mint();
        // The nearer shape's rule, so a shape with holes keeps them.
        let near = if t < 0.5 || !blend.paint { a } else { b };
        let world = match near == b {
            true => between(&ra_b, &rb_b, t),
            false => between(&ra, &rb, t),
        };
        let step_box = world.bounding_box();
        let place = place_of(k, step_box.center());
        let path = to_parent * place * world;
        ops.push(Operation::CreateNode {
            id: step,
            parent: group,
            index: k - 1,
            kind: NodeKind::Path {
                path,
                corner_radii: Vec::new(),
                corner_smoothing: 0.0,
                mirror: crate::symmetry::PathMirror::NONE,
            },
            transform: None,
            name: Some(format!("Step {k}")),
        });
        let carry = (
            carry(shape_a, step_box, place),
            carry(shape_b, step_box, place),
        );
        let (paint, opacity) = look_between(doc, (a, b), t, blend.paint, carry);
        if let Some(rule) = doc.get(near).map(|n| n.fill_rule()) {
            ops.push(Operation::SetFillRule { id: step, rule });
        }
        ops.push(Operation::SetFills {
            id: step,
            fills: paint.fills,
        });
        ops.push(Operation::SetStrokes {
            id: step,
            strokes: paint.strokes,
        });
        if opacity != 1.0 {
            ops.push(Operation::SetOpacity { id: step, opacity });
        }
    }
    scratch
        .apply(&Transaction(ops))
        .map_err(|_| Refusal::Parts)?;
    let nodes = scratch.capture_subtree(group).ok_or(Refusal::Parts)?;
    let mut ops = vec![Operation::InsertSubtree {
        nodes,
        parent,
        index: at,
    }];
    // The two shapes join their steps, the first under them and the second over,
    // each where it stood or on its end of the spine: the group sits in the upper
    // shape's parent with no transform of its own.
    for (id, index) in [(a, 0), (b, blend.steps + 1)] {
        let world = res.world_transform(id).ok_or(Refusal::Count)?;
        let centre = world_box(id).1.map_or(Point::ZERO, |r| r.center());
        ops.push(Operation::Reparent {
            id,
            new_parent: group,
            index,
        });
        ops.push(Operation::SetTransform {
            id,
            transform: to_parent * place_of(index, centre) * world,
        });
    }
    // The spine stays its own layer, unchanged on the page, directly under the
    // group: the list as it stands once the group is in and the shapes out.
    if let Some(s) = blend.spine {
        let mut list: Vec<NodeId> = doc
            .get(parent)
            .map(|p| p.children().to_vec())
            .unwrap_or_default();
        list.insert(at, group);
        list.retain(|c| *c != a && *c != b && *c != s);
        let below = list
            .iter()
            .position(|c| *c == group)
            .ok_or(Refusal::Count)?;
        let world = res.world_transform(s).ok_or(Refusal::Spine)?;
        ops.push(Operation::Reparent {
            id: s,
            new_parent: parent,
            index: below,
        });
        ops.push(Operation::SetTransform {
            id: s,
            transform: to_parent * world,
        });
    }
    Ok((Transaction(ops), group))
}
