//! Markers: the arrowheads, dots and bars a stroke draws at an open outline's two
//! ends ([`crate::Stroke::start`], [`crate::Stroke::end`]).
//!
//! A marker is a shape of its own, filled with its stroke's paint, never stroke
//! ink, so a stroke stays ink at one width and its dash never cuts a head. Each
//! visible stroke draws its own pair, sized from its width and
//! [`crate::Stroke::marker_size`], just after its ink. The start sits at the first
//! subpath's start and the end at the last subpath's end, each only where that
//! subpath is open: where SVG's `marker-start` and `marker-end` land.
//!
//! The tip is the end point. A head that reaches back over the stroke
//! ([`Marker::Arrow`], [`Marker::OpenArrow`]) trims the stroke back under itself
//! ([`trimmed`]), so the line's cap never shows past the point.

use std::borrow::Cow;

use kurbo::{
    Affine, BezPath, Circle, ParamCurve, ParamCurveArclen, PathSeg, Point, Rect, Shape, Vec2,
};
use serde::{Deserialize, Serialize};

use crate::node::NodeKind;

/// How finely arc lengths along a stroked path are measured, in its own units.
const ARCLEN_ACCURACY: f64 = 1e-4;

/// How finely a marker's curves are built, in stroke widths.
const TOLERANCE: f64 = 1e-3;

/// A head's span at 100%, in stroke widths: an arrow is this long and this wide.
const SPAN: f64 = 3.0;

/// The marker sizes a stroke may take, as fractions: 25% to 1000%.
pub const SIZE_RANGE: std::ops::RangeInclusive<f64> = 0.25..=10.0;

/// `size` held to [`SIZE_RANGE`], a size that is not a number read as 100%.
pub fn clamped_size(size: f64) -> f64 {
    match size.is_finite() {
        true => size.clamp(*SIZE_RANGE.start(), *SIZE_RANGE.end()),
        false => 1.0,
    }
}

/// Each marker's word, in [`Marker::ALL`]'s order: what scripts and the readers
/// of other formats name a marker by.
pub const WORDS: [&str; 6] = ["none", "arrow", "open_arrow", "circle", "square", "bar"];

/// What one end of an open outline wears.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum Marker {
    /// The stroke just ends, as its cap draws it.
    #[default]
    None,
    /// A filled triangle whose tip is the end.
    Arrow,
    /// Two strokes meeting at the end, a chevron.
    OpenArrow,
    /// A dot centred on the end.
    Circle,
    /// A square centred on the end, turned with the outline.
    Square,
    /// A bar across the end.
    Bar,
}

impl Marker {
    /// Every marker, in the order a picker lists them.
    pub const ALL: [Marker; 6] = [
        Marker::None,
        Marker::Arrow,
        Marker::OpenArrow,
        Marker::Circle,
        Marker::Square,
        Marker::Bar,
    ];

    /// Whether this is [`Marker::None`]; what the save format leaves out.
    pub fn is_none(&self) -> bool {
        *self == Marker::None
    }

    /// The marker's name as the inspector shows it.
    pub fn label(self) -> &'static str {
        match self {
            Marker::None => "None",
            Marker::Arrow => "Arrow",
            Marker::OpenArrow => "Open arrow",
            Marker::Circle => "Circle",
            Marker::Square => "Square",
            Marker::Bar => "Bar",
        }
    }

    /// The marker's word, [`WORDS`]' entry for it.
    pub fn word(self) -> &'static str {
        WORDS[Self::ALL.iter().position(|m| *m == self).unwrap_or(0)]
    }

    /// The marker a [`WORDS`] entry names.
    pub fn from_word(word: &str) -> Option<Marker> {
        WORDS.iter().position(|w| *w == word).map(|i| Self::ALL[i])
    }

    /// How far back from the end the stroke stops, in stroke widths, at `size`
    /// and with a chevron's `join`: far enough that the stroke's end and its cap
    /// stay inside the head.
    pub fn trim(self, size: f64, join: kurbo::Join) -> f64 {
        match self {
            Marker::Arrow => SPAN * size / 2.0,
            Marker::OpenArrow => join_reach(join) + 0.5,
            Marker::None | Marker::Circle | Marker::Square | Marker::Bar => 0.0,
        }
    }

    /// The marker's outline in stroke widths, its end at the origin pointing along
    /// +x, at `size`. The open arrow and the bar are strokes a width thick with
    /// the stroke's `cap` and `join`, outlined. Wound as [`Circle`] is, so heads
    /// overlapping fill as one.
    pub fn outline(self, size: f64, cap: kurbo::Cap, join: kurbo::Join) -> Option<BezPath> {
        let half = SPAN * size / 2.0;
        let polygon = |points: &[(f64, f64)]| {
            let mut path = BezPath::new();
            for (i, &(x, y)) in points.iter().enumerate() {
                match i {
                    0 => path.move_to((x, y)),
                    _ => path.line_to((x, y)),
                }
            }
            path.close_path();
            path
        };
        let stroked = |line: BezPath| {
            let style = kurbo::Stroke::new(1.0).with_caps(cap).with_join(join);
            kurbo::stroke(line, &style, &kurbo::StrokeOpts::default(), TOLERANCE)
        };
        let path = match self {
            Marker::None => return None,
            Marker::Arrow => polygon(&[(0.0, 0.0), (-2.0 * half, half), (-2.0 * half, -half)]),
            Marker::OpenArrow => {
                // The join's outer corner on the end: the vertex sits back by its reach.
                let v = -join_reach(join);
                let mut line = BezPath::new();
                line.move_to((v - half, half));
                line.line_to((v, 0.0));
                line.line_to((v - half, -half));
                stroked(line)
            }
            Marker::Circle => Circle::new(Point::ZERO, half).to_path(TOLERANCE),
            Marker::Square => Rect::new(-half, -half, half, half).to_path(0.0),
            Marker::Bar => {
                let mut line = BezPath::new();
                line.move_to((0.0, -half));
                line.line_to((0.0, half));
                stroked(line)
            }
        };
        let wound = Circle::new(Point::ZERO, 1.0).to_path(TOLERANCE).area();
        Some(match path.area().signum() == wound.signum() {
            true => path,
            false => path.reverse_subpaths(),
        })
    }
}

/// How far a right-angled join's outer corner reaches past its vertex, in widths.
fn join_reach(join: kurbo::Join) -> f64 {
    let r = std::f64::consts::FRAC_1_SQRT_2;
    match join {
        kurbo::Join::Miter => r,
        kurbo::Join::Round => 0.5,
        kurbo::Join::Bevel => 0.5 * r,
    }
}

/// One stroke's markers and what shapes them, read off it by
/// [`crate::Stroke::markers`].
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Markers {
    pub start: Marker,
    pub end: Marker,
    /// [`crate::Stroke::marker_size`].
    pub size: f64,
    /// The stroke's cap and join, which the open arrow and the bar are drawn with.
    pub cap: kurbo::Cap,
    pub join: kurbo::Join,
}

impl Markers {
    /// Neither end marked.
    pub const NONE: Markers = Markers {
        start: Marker::None,
        end: Marker::None,
        size: 1.0,
        cap: kurbo::Cap::Butt,
        join: kurbo::Join::Miter,
    };

    /// Whether neither end is marked.
    pub fn is_none(&self) -> bool {
        self.start.is_none() && self.end.is_none()
    }
}

/// Whether a layer of `kind` can wear markers at all: a line, an arc, a spiral or
/// a path. Whether one is drawn is [`applies`]' question.
pub fn drawn_on(kind: &NodeKind) -> bool {
    matches!(
        kind,
        NodeKind::Line { .. }
            | NodeKind::Arc { .. }
            | NodeKind::Spiral { .. }
            | NodeKind::Path { .. }
    )
}

/// Whether a layer of `kind` has an end for a marker: a line, an arc, a spiral,
/// and a path whose first or last subpath is open.
pub fn applies(kind: &NodeKind) -> bool {
    match kind {
        NodeKind::Line { .. } | NodeKind::Arc { .. } | NodeKind::Spiral { .. } => true,
        NodeKind::Path { path, .. } => {
            let subs = crate::geometry::subpaths(path);
            let (first, last) = marked_ends(&subs);
            first.is_some() || last.is_some()
        }
        _ => false,
    }
}

/// One marker where it sits on a stroked path.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Placed {
    pub marker: Marker,
    /// Whether it sits on the outline's end rather than its start.
    pub at_end: bool,
    /// The outline's end, where the marker's own origin goes.
    pub tip: Point,
    /// The way the marker points, a unit vector: out of the outline along its
    /// tangent at the end.
    pub dir: Vec2,
    /// Where the trimmed stroke stops: `tip` for a marker that trims nothing.
    pub base: Point,
}

impl Placed {
    /// The marker's outline for a stroke `width` wide with `markers`, in the
    /// path's own space.
    pub fn ink(&self, width: f64, markers: Markers) -> Option<BezPath> {
        let mut path = self
            .marker
            .outline(markers.size, markers.cap, markers.join)?;
        path.apply_affine(self.transform(width));
        Some(path)
    }

    /// The marker's own space, stroke widths with its end at the origin, to the path's.
    pub fn transform(&self, width: f64) -> Affine {
        Affine::translate(self.tip.to_vec2())
            * Affine::rotate(self.dir.atan2())
            * Affine::scale(width)
    }
}

/// The subpaths of `subs` a start and an end marker sit on, as `(the first, the
/// last)`, each `None` when that subpath closes.
fn marked_ends(subs: &[BezPath]) -> (Option<usize>, Option<usize>) {
    let open = |i: usize| (!crate::geometry::is_closed(&subs[i])).then_some(i);
    match subs.len() {
        0 => (None, None),
        n => (open(0), open(n - 1)),
    }
}

/// The trims a subpath `length` long takes at its two ends, scaled down together
/// when they would meet.
fn fitted_trims(start: f64, end: f64, length: f64) -> (f64, f64) {
    let total = start + end;
    if total <= length || total <= 0.0 {
        (start, end)
    } else {
        let k = length / total;
        (start * k, end * k)
    }
}

/// The segment and parameter `d` along `segs` from their start.
fn from_start(segs: &[PathSeg], d: f64) -> Option<(usize, f64)> {
    let mut before = 0.0;
    for (i, seg) in segs.iter().enumerate() {
        let len = seg.arclen(ARCLEN_ACCURACY);
        if before + len >= d {
            let t = if len > 0.0 {
                seg.inv_arclen(d - before, ARCLEN_ACCURACY)
            } else {
                0.0
            };
            return Some((i, t));
        }
        before += len;
    }
    segs.len().checked_sub(1).map(|i| (i, 1.0))
}

/// The segment and parameter `d` back along `segs` from their end.
fn from_end(segs: &[PathSeg], d: f64) -> Option<(usize, f64)> {
    let mut after = 0.0;
    for (i, seg) in segs.iter().enumerate().rev() {
        let len = seg.arclen(ARCLEN_ACCURACY);
        if after + len >= d {
            let t = if len > 0.0 {
                seg.inv_arclen(len - (d - after), ARCLEN_ACCURACY)
            } else {
                1.0
            };
            return Some((i, t));
        }
        after += len;
    }
    (!segs.is_empty()).then_some((0, 0.0))
}

/// The way out of `segs` at their end (`at_end`) or start, a unit vector: the
/// tangent there, read off the nearest control point that is not the end itself,
/// so a retracted handle does not leave the head unturned.
fn tangent_out(segs: &[PathSeg], at_end: bool) -> Option<Vec2> {
    let points = |seg: &PathSeg| -> Vec<Point> {
        match *seg {
            PathSeg::Line(l) => vec![l.p0, l.p1],
            PathSeg::Quad(q) => vec![q.p0, q.p1, q.p2],
            PathSeg::Cubic(c) => vec![c.p0, c.p1, c.p2, c.p3],
        }
    };
    let mut all: Vec<Point> = segs.iter().flat_map(points).collect();
    if !at_end {
        all.reverse();
    }
    let end = *all.last()?;
    all.iter()
        .rev()
        .map(|p| end - *p)
        .find(|d| d.hypot() > 1e-12)
        .map(Vec2::normalize)
}

/// The length of `segs` end to end.
fn length(segs: &[PathSeg]) -> f64 {
    segs.iter().map(|s| s.arclen(ARCLEN_ACCURACY)).sum()
}

/// The markers `shape`, a stroke's laid path, wears for a stroke `width` wide.
pub fn placed(shape: &BezPath, width: f64, markers: Markers) -> Vec<Placed> {
    if markers.is_none() || !width.is_finite() || width <= 0.0 {
        return Vec::new();
    }
    let subs = crate::geometry::subpaths(shape);
    let (first, last) = marked_ends(&subs);
    let trim = |m: Marker| m.trim(markers.size, markers.join) * width;
    let mut out = Vec::new();
    let mut place = |i: usize, marker: Marker, at_end: bool| {
        if marker.is_none() {
            return;
        }
        let segs: Vec<PathSeg> = subs[i].segments().collect();
        let len = length(&segs);
        if segs.is_empty() || len <= 1e-9 {
            return;
        }
        let other = match (at_end, first == last) {
            (true, true) => trim(markers.start),
            (false, true) => trim(markers.end),
            _ => 0.0,
        };
        let (own, _) = fitted_trims(trim(marker), other, len);
        let Some(dir) = tangent_out(&segs, at_end) else {
            return;
        };
        let tip = match at_end {
            true => segs[segs.len() - 1].end(),
            false => segs[0].start(),
        };
        let base = match (own > 0.0, at_end) {
            (false, _) => tip,
            (true, true) => {
                from_end(&segs, own).map_or(tip, |(j, t)| kurbo::ParamCurve::eval(&segs[j], t))
            }
            (true, false) => {
                from_start(&segs, own).map_or(tip, |(j, t)| kurbo::ParamCurve::eval(&segs[j], t))
            }
        };
        out.push(Placed {
            marker,
            at_end,
            tip,
            dir,
            base,
        });
    };
    if let Some(i) = first {
        place(i, markers.start, false);
    }
    if let Some(i) = last {
        place(i, markers.end, true);
    }
    out
}

/// Every marker `shape` wears at `width`, as one path to fill non-zero.
pub fn ink(shape: &BezPath, width: f64, markers: Markers) -> Option<BezPath> {
    let mut out = BezPath::new();
    for p in placed(shape, width, markers) {
        if let Some(path) = p.ink(width, markers) {
            out.extend(path);
        }
    }
    (!out.is_empty()).then_some(out)
}

/// `shape` cut back under the heads that trim ([`Marker::trim`]) at `width`. A
/// subpath both heads would wholly cover is left out.
pub fn trimmed(shape: &BezPath, width: f64, markers: Markers) -> Cow<'_, BezPath> {
    let trim = |m: Marker| m.trim(markers.size, markers.join) * width;
    let (start, end) = (trim(markers.start), trim(markers.end));
    if !width.is_finite() || width <= 0.0 || (start <= 0.0 && end <= 0.0) {
        return Cow::Borrowed(shape);
    }
    let subs = crate::geometry::subpaths(shape);
    let (first, last) = marked_ends(&subs);
    if first.is_none() && last.is_none() {
        return Cow::Borrowed(shape);
    }
    let mut out = BezPath::new();
    for (i, sub) in subs.iter().enumerate() {
        let a = if first == Some(i) { start } else { 0.0 };
        let b = if last == Some(i) { end } else { 0.0 };
        if a <= 0.0 && b <= 0.0 {
            out.extend(sub.iter());
            continue;
        }
        let mut segs: Vec<PathSeg> = sub.segments().collect();
        let (a, b) = fitted_trims(a, b, length(&segs));
        if a + b >= length(&segs) - 1e-9 {
            continue;
        }
        if a > 0.0
            && let Some((j, t)) = from_start(&segs, a)
        {
            segs[j] = kurbo::ParamCurve::subsegment(&segs[j], t..1.0);
            segs.drain(..j);
        }
        if b > 0.0
            && let Some((j, t)) = from_end(&segs, b)
        {
            segs[j] = kurbo::ParamCurve::subsegment(&segs[j], 0.0..t);
            segs.truncate(j + 1);
        }
        let Some(first_seg) = segs.first() else {
            continue;
        };
        out.move_to(first_seg.start());
        for seg in &segs {
            out.push(seg.as_path_el());
        }
    }
    Cow::Owned(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn line(to: Point) -> BezPath {
        let mut p = BezPath::new();
        p.move_to(Point::ZERO);
        p.line_to(to);
        p
    }

    fn both(m: Marker) -> Markers {
        Markers {
            start: m,
            end: m,
            ..Markers::NONE
        }
    }

    /// An arrow's tip is the line's end, pointing out of it; at 100% it is three
    /// widths long and wide, and the stroke stops half a head short of the tip.
    #[test]
    fn an_arrow_sits_on_the_end_and_trims_the_stroke() {
        let path = line(Point::new(100.0, 0.0));
        let arrows = both(Marker::Arrow);
        let placed = placed(&path, 2.0, arrows);
        assert_eq!(placed.len(), 2);
        let (start, end) = (placed[0], placed[1]);
        assert_eq!(start.tip, Point::ZERO);
        assert!(
            (start.dir - Vec2::new(-1.0, 0.0)).hypot() < 1e-9,
            "{:?}",
            start.dir
        );
        assert!((end.tip - Point::new(100.0, 0.0)).hypot() < 1e-9);
        assert!(
            (end.dir - Vec2::new(1.0, 0.0)).hypot() < 1e-9,
            "{:?}",
            end.dir
        );
        let cut = trimmed(&path, 2.0, arrows);
        let b = cut.bounding_box();
        assert!(
            (b.x0 - 3.0).abs() < 1e-6 && (b.x1 - 97.0).abs() < 1e-6,
            "{b:?}"
        );
        assert!(
            (end.base - Point::new(97.0, 0.0)).hypot() < 1e-6,
            "{:?}",
            end.base
        );
        let ib = ink(&path, 2.0, arrows).expect("ink").bounding_box();
        assert!(
            ib.x0 >= -1e-9 && ib.x1 <= 100.0 + 1e-9,
            "the tips are the ends: {ib:?}"
        );
        assert!(
            (ib.height() - 6.0).abs() < 1e-9,
            "three widths across: {ib:?}"
        );
    }

    /// The size scales a head, and the open arrow's point is the end whatever the
    /// join.
    #[test]
    fn the_size_scales_a_head_and_a_chevron_points_at_the_end() {
        for join in [kurbo::Join::Miter, kurbo::Join::Round, kurbo::Join::Bevel] {
            let head = Marker::OpenArrow
                .outline(2.0, kurbo::Cap::Butt, join)
                .expect("drawn")
                .bounding_box();
            assert!(
                head.x1.abs() < 1e-3,
                "{join:?}: the point is the end: {head:?}"
            );
            assert!(
                (head.height() - 6.0).abs() < 0.8,
                "{join:?}: six widths at 200%: {head:?}"
            );
        }
    }

    /// Every outline winds the same way, so two heads overlapping fill as one.
    #[test]
    fn every_outline_winds_as_a_circle_does() {
        let wound = Circle::new(Point::ZERO, 1.0).to_path(TOLERANCE).area();
        for m in Marker::ALL.into_iter().filter(|m| !m.is_none()) {
            for cap in [kurbo::Cap::Butt, kurbo::Cap::Round, kurbo::Cap::Square] {
                let area = m
                    .outline(1.0, cap, kurbo::Join::Miter)
                    .expect("drawn")
                    .area();
                assert_eq!(area.signum(), wound.signum(), "{m:?} {cap:?}");
            }
        }
    }

    /// A closed outline has no end: nothing is placed and nothing trimmed.
    #[test]
    fn a_closed_outline_wears_none() {
        let square = Rect::new(0.0, 0.0, 10.0, 10.0).to_path(0.0);
        let arrows = both(Marker::Arrow);
        assert!(placed(&square, 1.0, arrows).is_empty());
        assert!(matches!(trimmed(&square, 1.0, arrows), Cow::Borrowed(_)));
    }

    /// A line shorter than its two heads loses its shaft, and the heads stay.
    #[test]
    fn a_line_shorter_than_its_heads_keeps_only_the_heads() {
        let path = line(Point::new(4.0, 0.0));
        let arrows = both(Marker::Arrow);
        assert!(trimmed(&path, 2.0, arrows).is_empty());
        assert_eq!(placed(&path, 2.0, arrows).len(), 2);
    }

    /// A head on a curve is turned to the curve's tangent at the end, not to the
    /// chord its trim cuts.
    #[test]
    fn a_head_on_a_curve_follows_the_tangent() {
        let mut path = BezPath::new();
        path.move_to(Point::ZERO);
        path.curve_to((50.0, 0.0), (100.0, 50.0), (100.0, 100.0));
        let markers = Markers {
            end: Marker::Arrow,
            ..Markers::NONE
        };
        let end = placed(&path, 4.0, markers)[0];
        assert!(
            (end.dir - Vec2::new(0.0, 1.0)).hypot() < 1e-9,
            "{:?}",
            end.dir
        );
        assert!(
            (end.tip - end.base).hypot() <= 6.0 + 1e-6,
            "trimmed by 1.5 widths"
        );
    }
}
