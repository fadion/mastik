//! A stroke's width profile: how wide it is along its path, as fractions of its
//! width at points placed by arc length, and the ink a profiled stroke lays.
//!
//! No backend strokes a varying width, so a profiled stroke is drawn as a fill.
//! [`ink`] lays it as pieces each wound positive, to fill non-zero: the renderer
//! fills them as they are, and `build::stroke_ink` unites them into one shape for
//! export and *Outline stroke*. The stroke's `width` stays its widest, so every
//! reader that grows a box by the width stays conservative.

use kurbo::{
    BezPath, Cap, Join, ParamCurve, ParamCurveArclen, ParamCurveDeriv, ParamCurveNearest, PathSeg,
    Point, Vec2,
};
use serde::{Deserialize, Serialize};

/// One point of a [`WidthProfile`].
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct WidthPoint {
    /// Where along the path, as a fraction of its whole length from its start.
    pub t: f64,
    /// How far the stroke's left edge (left as the path runs) sits from the path,
    /// as a fraction of half the stroke's width: 1 is an even stroke's edge.
    pub left: f64,
    /// The right edge's, likewise.
    pub right: f64,
}

/// How a stroke's width runs along its path: points sorted by `t`, each side
/// eased between them without overshooting (monotone cubic), held flat before
/// the first and after the last. Never empty once [`Self::clamped`].
///
/// Saved as `{"points": […]}`; read from that, a bare list of points, or a
/// preset's name ([`Preset::named`]), the short forms MCP writes.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(try_from = "ProfileWire")]
pub struct WidthProfile {
    pub points: Vec<WidthPoint>,
}

/// Every form a [`WidthProfile`] is read from.
#[derive(Deserialize)]
#[serde(untagged)]
enum ProfileWire {
    Named(String),
    Points(Vec<WidthPoint>),
    Full { points: Vec<WidthPoint> },
}

impl TryFrom<ProfileWire> for WidthProfile {
    type Error = String;
    fn try_from(w: ProfileWire) -> Result<Self, String> {
        match w {
            ProfileWire::Named(name) => Preset::named(&name).map(Preset::profile).ok_or_else(|| {
                let names: Vec<&str> = Preset::ALL.iter().map(|p| p.key()).collect();
                format!(
                    "no width profile is named {name:?}: one of {}, or a list of {{t, left, right}} points",
                    names.join(", ")
                )
            }),
            ProfileWire::Points(points) | ProfileWire::Full { points } => Ok(WidthProfile { points }),
        }
    }
}

/// The profiles the stroke popover offers by name. A profile that matches none
/// is *Custom*.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Preset {
    TaperBoth,
    TaperStart,
    TaperEnd,
    Bulge,
}

impl Preset {
    pub const ALL: [Preset; 4] = [
        Preset::TaperBoth,
        Preset::TaperStart,
        Preset::TaperEnd,
        Preset::Bulge,
    ];

    /// The name the popover shows.
    pub fn label(self) -> &'static str {
        match self {
            Preset::TaperBoth => "Taper both",
            Preset::TaperStart => "Taper start",
            Preset::TaperEnd => "Taper end",
            Preset::Bulge => "Bulge",
        }
    }

    /// The name MCP and scripts write.
    pub fn key(self) -> &'static str {
        match self {
            Preset::TaperBoth => "taper_both",
            Preset::TaperStart => "taper_start",
            Preset::TaperEnd => "taper_end",
            Preset::Bulge => "bulge",
        }
    }

    /// The preset `name` names, as [`Self::key`] or [`Self::label`] in any case.
    pub fn named(name: &str) -> Option<Preset> {
        let name = name.trim().to_ascii_lowercase().replace([' ', '-'], "_");
        Preset::ALL.into_iter().find(|p| p.key() == name)
    }

    pub fn profile(self) -> WidthProfile {
        let even = |t: f64, w: f64| WidthPoint {
            t,
            left: w,
            right: w,
        };
        let points = match self {
            Preset::TaperBoth => vec![even(0.0, 0.0), even(0.5, 1.0), even(1.0, 0.0)],
            Preset::TaperStart => vec![even(0.0, 0.0), even(1.0, 1.0)],
            Preset::TaperEnd => vec![even(0.0, 1.0), even(1.0, 0.0)],
            Preset::Bulge => vec![even(0.0, 0.3), even(0.5, 1.0), even(1.0, 0.3)],
        };
        WidthProfile { points }
    }
}

impl WidthProfile {
    /// The preset this profile is, if it is one exactly.
    pub fn preset(&self) -> Option<Preset> {
        Preset::ALL.into_iter().find(|p| p.profile() == *self)
    }

    /// The profile with every point finite and within `0..=1`, sorted by `t`;
    /// `None` when no point is left.
    pub fn clamped(mut self) -> Option<WidthProfile> {
        self.points
            .retain(|p| p.t.is_finite() && p.left.is_finite() && p.right.is_finite());
        for p in &mut self.points {
            p.t = p.t.clamp(0.0, 1.0);
            p.left = p.left.clamp(0.0, 1.0);
            p.right = p.right.clamp(0.0, 1.0);
        }
        self.points.sort_by(|a, b| a.t.total_cmp(&b.t));
        (!self.points.is_empty()).then_some(self)
    }

    /// The profile of the same stroke along the reversed path: each point at
    /// `1 − t`, its left and right traded.
    pub fn reversed(&self) -> WidthProfile {
        let points = self
            .points
            .iter()
            .rev()
            .map(|p| WidthPoint {
                t: 1.0 - p.t,
                left: p.right,
                right: p.left,
            })
            .collect();
        WidthProfile { points }
    }

    /// The widest either side reaches, which the eased curve never passes.
    pub fn widest(&self) -> f64 {
        self.points
            .iter()
            .map(|p| p.left.max(p.right))
            .fold(0.0, f64::max)
    }

    /// Both sides at `t`, as fractions of the half-width.
    pub fn at(&self, t: f64) -> (f64, f64) {
        (
            eased(&self.points, t, |p| p.left),
            eased(&self.points, t, |p| p.right),
        )
    }
}

/// One side of `points` at `t`: Fritsch–Carlson monotone cubic Hermite, so an
/// eased stretch never bulges past the points either side of it.
fn eased(points: &[WidthPoint], t: f64, side: impl Fn(&WidthPoint) -> f64) -> f64 {
    let n = points.len();
    match n {
        0 => return 1.0,
        1 => return side(&points[0]),
        _ => {}
    }
    if t <= points[0].t {
        return side(&points[0]);
    }
    if t >= points[n - 1].t {
        return side(&points[n - 1]);
    }
    let x = |i: usize| points[i].t;
    let y = |i: usize| side(&points[i]);
    let secant = |i: usize| {
        let dx = x(i + 1) - x(i);
        if dx <= 1e-12 {
            0.0
        } else {
            (y(i + 1) - y(i)) / dx
        }
    };
    // The slope at each point: zero at an extremum, else the harmonic mean of
    // the secants either side, which keeps each piece monotone.
    let slope = |i: usize| -> f64 {
        if i == 0 {
            return secant(0);
        }
        if i == n - 1 {
            return secant(n - 2);
        }
        let (a, b) = (secant(i - 1), secant(i));
        if a * b <= 0.0 {
            0.0
        } else {
            let (wa, wb) = (
                2.0 * (x(i + 1) - x(i)) + (x(i) - x(i - 1)),
                (x(i + 1) - x(i)) + 2.0 * (x(i) - x(i - 1)),
            );
            (wa + wb) / (wa / a + wb / b)
        }
    };
    let i = (0..n - 1).find(|&i| t < x(i + 1)).unwrap_or(n - 2);
    let h = x(i + 1) - x(i);
    if h <= 1e-12 {
        return y(i + 1);
    }
    let s = (t - x(i)) / h;
    let (s2, s3) = (s * s, s * s * s);
    let (h00, h10, h01, h11) = (
        2.0 * s3 - 3.0 * s2 + 1.0,
        s3 - 2.0 * s2 + s,
        -2.0 * s3 + 3.0 * s2,
        s3 - s2,
    );
    (h00 * y(i) + h10 * h * slope(i) + h01 * y(i + 1) + h11 * h * slope(i + 1)).max(0.0)
}

/// Accuracy for the arc-length sums here, in path units.
const ARCLEN_ACC: f64 = 1e-4;

/// The segments of `path` that have any length, in order, each with whether it
/// starts a subpath.
fn segments(path: &BezPath) -> Vec<(PathSeg, bool)> {
    let mut out = Vec::new();
    for sub in crate::geometry::subpaths(path) {
        let mut first = true;
        for seg in sub.segments() {
            if seg.arclen(ARCLEN_ACC) > 1e-9 {
                out.push((seg, first));
                first = false;
            }
        }
    }
    out
}

/// `path`'s whole length, every subpath end to end.
pub fn length(path: &BezPath) -> f64 {
    segments(path)
        .iter()
        .map(|(s, _)| s.arclen(ARCLEN_ACC))
        .sum()
}

/// The point `t` of the way along `path`, and the unit direction it runs there.
pub fn point_at(path: &BezPath, t: f64) -> Option<(Point, Vec2)> {
    let segs = segments(path);
    let total: f64 = segs.iter().map(|(s, _)| s.arclen(ARCLEN_ACC)).sum();
    if total <= 0.0 {
        return None;
    }
    let mut left = t.clamp(0.0, 1.0) * total;
    for (k, (seg, _)) in segs.iter().enumerate() {
        let len = seg.arclen(ARCLEN_ACC);
        if left <= len || k == segs.len() - 1 {
            let u = seg.inv_arclen(left.min(len), ARCLEN_ACC);
            return Some((seg.eval(u), direction(*seg, u)));
        }
        left -= len;
    }
    None
}

/// Where along `path` (a fraction of its length) the point nearest `p` sits,
/// and how far `p` is from it.
pub fn fraction_at(path: &BezPath, p: Point) -> Option<(f64, f64)> {
    let segs = segments(path);
    let total: f64 = segs.iter().map(|(s, _)| s.arclen(ARCLEN_ACC)).sum();
    if total <= 0.0 {
        return None;
    }
    let mut before = 0.0;
    let mut best: Option<(f64, f64)> = None;
    for (seg, _) in &segs {
        let near = seg.nearest(p, 1e-6);
        let distance = near.distance_sq.max(0.0).sqrt();
        if best.is_none_or(|(_, d)| distance < d) {
            let along = before + seg.subsegment(0.0..near.t).arclen(ARCLEN_ACC);
            best = Some((along / total, distance));
        }
        before += seg.arclen(ARCLEN_ACC);
    }
    best
}

/// The unit direction `seg` runs at `u`, from its derivative, or from its chord
/// where a handle sits on its anchor.
fn direction(seg: PathSeg, u: f64) -> Vec2 {
    let d = match seg {
        PathSeg::Line(l) => l.p1 - l.p0,
        PathSeg::Quad(q) => q.deriv().eval(u).to_vec2(),
        PathSeg::Cubic(c) => c.deriv().eval(u).to_vec2(),
    };
    let d = if d.hypot() > 1e-9 {
        d
    } else {
        let (a, b) = (seg.eval((u - 1e-3).max(0.0)), seg.eval((u + 1e-3).min(1.0)));
        b - a
    };
    let len = d.hypot();
    if len > 0.0 {
        d / len
    } else {
        Vec2::new(1.0, 0.0)
    }
}

/// What a profiled stroke lays, besides its path.
#[derive(Clone, Copy, Debug)]
pub struct Laid<'a> {
    /// The width the profile's fractions are of.
    pub width: f64,
    pub profile: &'a WidthProfile,
    /// The start and end caps.
    pub caps: (Cap, Cap),
    pub join: Join,
    pub miter_limit: f64,
    /// The dash pattern, already resolved, and its offset.
    pub dashes: &'a [f64],
    pub dash_offset: f64,
}

/// One point the ink is laid through.
#[derive(Clone, Copy, Debug)]
struct Station {
    p: Point,
    /// The unit direction the path runs.
    dir: Vec2,
    /// Arc length from the subpath's start.
    s: f64,
    /// Whether this station starts a new segment at the one before's point, so
    /// a join goes between them rather than a band.
    joint: bool,
}

/// The ink `laid` lays along `path`, its chords within `tol` of the true edges:
/// pieces each wound positive, one band piece between each two stations, a join
/// at every corner and a cap at every open end, to fill non-zero.
pub fn ink(path: &BezPath, laid: &Laid<'_>, tol: f64) -> BezPath {
    let mut out = BezPath::new();
    let half = laid.width / 2.0;
    let tol = tol.max(1e-6);
    if !half.is_finite() || half <= 0.0 {
        return out;
    }
    let total = length(path);
    if total <= 0.0 {
        return out;
    }
    let reach = half * laid.profile.widest();
    let mut before = 0.0;
    for sub in crate::geometry::subpaths(path) {
        let closed = matches!(sub.elements().last(), Some(kurbo::PathEl::ClosePath));
        let stations = stations(&sub, reach, tol, total);
        let Some(last) = stations.last() else {
            continue;
        };
        let sub_len = last.s;
        let sides = |s: f64| {
            let (l, r) = laid.profile.at((before + s) / total);
            (l * half, r * half)
        };
        let runs = match laid.dashes.is_empty() {
            true => vec![(0.0, sub_len)],
            false => dash_runs(laid.dashes, laid.dash_offset, sub_len),
        };
        let whole = laid.dashes.is_empty() && closed;
        for (a, b) in runs {
            let run = window(&stations, a, b);
            lay_run(&mut out, &run, &sides, laid, tol, whole);
        }
        before += sub_len;
    }
    out
}

/// `sub`'s stations: each segment cut evenly enough that its chords and its
/// offsets `reach` out stay within `tol`, and that the width eases smoothly over
/// `total`; each segment's first station a joint.
fn stations(sub: &BezPath, reach: f64, tol: f64, total: f64) -> Vec<Station> {
    let mut out: Vec<Station> = Vec::new();
    let mut s0 = 0.0;
    for seg in sub.segments() {
        let len = seg.arclen(ARCLEN_ACC);
        if len <= 1e-9 {
            continue;
        }
        let n = steps(seg, reach, tol, len / total);
        let points: Vec<(Point, f64)> = (0..=n)
            .map(|i| {
                let u = i as f64 / n as f64;
                (seg.eval(u), u)
            })
            .collect();
        let chords: Vec<f64> = points
            .windows(2)
            .map(|w| (w[1].0 - w[0].0).hypot())
            .collect();
        let chord_total: f64 = chords.iter().sum::<f64>().max(1e-12);
        let mut along = 0.0;
        for (i, (p, u)) in points.iter().enumerate() {
            if i > 0 {
                along += chords[i - 1];
            }
            out.push(Station {
                p: *p,
                dir: direction(seg, *u),
                s: s0 + len * along / chord_total,
                joint: i == 0 && !out.is_empty(),
            });
        }
        s0 += len;
    }
    out
}

/// How many even steps `seg` takes: enough that its chords stay within `tol`
/// (a cubic strays at most an eighth of its second derivative's bound over the
/// steps squared), that a parallel `reach` out turns little enough per step,
/// and that a segment `share` of the whole path gets its share of 96 steps.
fn steps(seg: PathSeg, reach: f64, tol: f64, share: f64) -> usize {
    let by_share = (96.0 * share).ceil();
    let (bend, turn) = match seg {
        PathSeg::Line(_) => (0.0, 0.0),
        _ => {
            let c = seg.to_cubic();
            let d1 = c.p0.to_vec2() - 2.0 * c.p1.to_vec2() + c.p2.to_vec2();
            let d2 = c.p1.to_vec2() - 2.0 * c.p2.to_vec2() + c.p3.to_vec2();
            let bend = 6.0 * d1.hypot().max(d2.hypot());
            let legs = [c.p1 - c.p0, c.p2 - c.p1, c.p3 - c.p2];
            let legs: Vec<Vec2> = legs.into_iter().filter(|v| v.hypot() > 1e-9).collect();
            let turn: f64 = legs
                .windows(2)
                .map(|w| w[0].cross(w[1]).atan2(w[0].dot(w[1])).abs())
                .sum();
            (bend, turn)
        }
    };
    let by_flat = (bend / (8.0 * tol)).sqrt().ceil();
    let by_turn = (turn * (reach / (8.0 * tol)).sqrt()).ceil();
    by_share.max(by_flat).max(by_turn).clamp(1.0, 2048.0) as usize
}

/// The on-dash stretches of a subpath `len` long, as SVG lays the pattern:
/// starting `offset` into it, afresh on each subpath.
fn dash_runs(dashes: &[f64], offset: f64, len: f64) -> Vec<(f64, f64)> {
    let mut pattern = dashes.to_vec();
    if pattern.len() % 2 == 1 {
        pattern.extend_from_within(..);
    }
    let period: f64 = pattern.iter().sum();
    if !period.is_finite() || period <= 0.0 {
        return vec![(0.0, len)];
    }
    let mut out = Vec::new();
    let mut i = 0;
    // Into the pattern by the offset: `i` the entry it lands in, `phase` how far.
    let mut phase = offset.rem_euclid(period);
    for _ in 0..pattern.len() {
        if phase < pattern[i] {
            break;
        }
        phase -= pattern[i];
        i = (i + 1) % pattern.len();
    }
    let mut at = -phase;
    // Bounded so a pattern of tiny dashes on a long path cannot hang the frame.
    for _ in 0..100_000 {
        if at >= len {
            break;
        }
        let end = at + pattern[i];
        if i % 2 == 0 && end > 0.0 {
            out.push((at.max(0.0), end.min(len)));
        }
        at = end;
        i = (i + 1) % pattern.len();
    }
    out
}

/// The stations of `all` from arc length `a` to `b`, cut where those fall.
fn window(all: &[Station], a: f64, b: f64) -> Vec<Station> {
    let lerp = |x: &Station, y: &Station, s: f64| {
        let k = if y.s - x.s > 1e-12 {
            (s - x.s) / (y.s - x.s)
        } else {
            0.0
        };
        let dir = x.dir.lerp(y.dir, k);
        let len = dir.hypot();
        Station {
            p: x.p.lerp(y.p, k),
            dir: if len > 1e-12 { dir / len } else { x.dir },
            s,
            joint: false,
        }
    };
    let mut out = Vec::new();
    for w in all.windows(2) {
        let (x, y) = (&w[0], &w[1]);
        if y.s < a || x.s > b {
            continue;
        }
        if out.is_empty() {
            if x.s >= a {
                out.push(Station { joint: false, ..*x });
            } else {
                out.push(lerp(x, y, a));
            }
        }
        if y.s <= b {
            out.push(*y);
        } else {
            out.push(lerp(x, y, b));
            break;
        }
    }
    if out.is_empty()
        && let Some(only) = all.first()
    {
        out.push(Station {
            joint: false,
            ..*only
        });
    }
    out
}

/// The normal on `dir`'s left.
fn normal(dir: Vec2) -> Vec2 {
    Vec2::new(-dir.y, dir.x)
}

/// Lay one run of stations: bands between them, joins at joints, caps at its
/// two ends unless `whole` closes it, where a join goes instead.
fn lay_run(
    out: &mut BezPath,
    run: &[Station],
    sides: &impl Fn(f64) -> (f64, f64),
    laid: &Laid<'_>,
    tol: f64,
    whole: bool,
) {
    let Some((first, last)) = run.first().zip(run.last()) else {
        return;
    };
    for w in run.windows(2) {
        let (x, y) = (&w[0], &w[1]);
        let (xl, xr) = sides(x.s);
        if y.joint {
            join(out, x.p, x.dir, y.dir, (xl, xr), laid, tol);
            continue;
        }
        let (yl, yr) = sides(y.s);
        let (nx, ny) = (normal(x.dir), normal(y.dir));
        piece(
            out,
            &[x.p + nx * xl, y.p + ny * yl, y.p - ny * yr, x.p - nx * xr],
        );
    }
    if whole {
        join(out, last.p, last.dir, first.dir, sides(last.s), laid, tol);
        return;
    }
    cap(
        out,
        first.p,
        -first.dir,
        sides(first.s),
        laid.caps.0,
        tol,
        false,
    );
    cap(out, last.p, last.dir, sides(last.s), laid.caps.1, tol, true);
}

/// A join at `p` from direction `a` to `b`, on the outer side only: the bands
/// overlap on the inner one.
fn join(
    out: &mut BezPath,
    p: Point,
    a: Vec2,
    b: Vec2,
    (l, r): (f64, f64),
    laid: &Laid<'_>,
    tol: f64,
) {
    let cross = a.cross(b);
    if cross.abs() < 1e-12 && a.dot(b) > 0.0 {
        return;
    }
    // Turning toward the left normal puts the outer side on the right.
    let (sign, w) = if cross > 0.0 { (-1.0, r) } else { (1.0, l) };
    if w <= 0.0 {
        return;
    }
    let (na, nb) = (normal(a) * sign, normal(b) * sign);
    match laid.join {
        Join::Bevel => piece(out, &[p, p + na * w, p + nb * w]),
        Join::Round => {
            let mut fan = vec![p];
            fan.extend(arc(p, na, nb, w, tol));
            piece(out, &fan);
        }
        Join::Miter => {
            let mid = na + nb;
            let cos_half = (mid.hypot() / 2.0).clamp(0.0, 1.0);
            let ratio = if cos_half > 1e-9 {
                1.0 / cos_half
            } else {
                f64::INFINITY
            };
            if ratio <= laid.miter_limit && mid.hypot() > 1e-12 {
                let tip = p + mid / mid.hypot() * w * ratio;
                piece(out, &[p, p + na * w, tip, p + nb * w]);
            } else {
                piece(out, &[p, p + na * w, p + nb * w]);
            }
        }
    }
}

/// A cap at `p` facing `out_dir` out of the line, `(l, r)` its sides as the
/// line runs (so mirrored at the start), `at_end` saying which end it is.
fn cap(
    out: &mut BezPath,
    p: Point,
    out_dir: Vec2,
    (l, r): (f64, f64),
    cap: Cap,
    tol: f64,
    at_end: bool,
) {
    // The line's own left normal, whichever way the cap faces.
    let left = match at_end {
        true => normal(out_dir),
        false => -normal(out_dir),
    };
    match cap {
        Cap::Butt => {}
        Cap::Square => {
            let e = (l + r) / 2.0;
            piece(
                out,
                &[
                    p + left * l,
                    p + left * l + out_dir * e,
                    p - left * r + out_dir * e,
                    p - left * r,
                ],
            );
        }
        Cap::Round => {
            let mut fan = vec![p];
            if l > 0.0 {
                fan.extend(arc(p, left, out_dir, l, tol));
            }
            if r > 0.0 {
                fan.extend(arc(p, out_dir, -left, r, tol));
            }
            piece(out, &fan);
        }
    }
}

/// Points on the circle of radius `w` round `c` from unit `a` to unit `b` the
/// short way, both ends included, within `tol` of the arc.
fn arc(c: Point, a: Vec2, b: Vec2, w: f64, tol: f64) -> Vec<Point> {
    let sweep = a.cross(b).atan2(a.dot(b));
    let step = (8.0 * tol / w.max(1e-9)).sqrt().clamp(1e-3, 0.3);
    let n = (sweep.abs() / step).ceil().clamp(1.0, 256.0) as usize;
    let start = a.atan2();
    (0..=n)
        .map(|i| {
            let th = start + sweep * i as f64 / n as f64;
            c + Vec2::new(th.cos(), th.sin()) * w
        })
        .collect()
}

/// Push `points` as one closed piece, wound positive.
fn piece(out: &mut BezPath, points: &[Point]) {
    if points.len() < 3 {
        return;
    }
    let area: f64 = (0..points.len())
        .map(|i| {
            points[i]
                .to_vec2()
                .cross(points[(i + 1) % points.len()].to_vec2())
        })
        .sum();
    if area.abs() <= 1e-18 {
        return;
    }
    let mut it: Box<dyn Iterator<Item = &Point>> = match area > 0.0 {
        true => Box::new(points.iter()),
        false => Box::new(points.iter().rev()),
    };
    if let Some(p) = it.next() {
        out.move_to(*p);
    }
    for p in it {
        out.line_to(*p);
    }
    out.close_path();
}

#[cfg(test)]
mod tests {
    use super::*;
    use kurbo::Shape;

    fn line(len: f64) -> BezPath {
        let mut p = BezPath::new();
        p.move_to((0.0, 0.0));
        p.line_to((len, 0.0));
        p
    }

    fn laid<'a>(profile: &'a WidthProfile, dashes: &'a [f64]) -> Laid<'a> {
        Laid {
            width: 10.0,
            profile,
            caps: (Cap::Butt, Cap::Butt),
            join: Join::Miter,
            miter_limit: 4.0,
            dashes,
            dash_offset: 0.0,
        }
    }

    /// The area `ink` covers on a straight line, whose pieces never overlap.
    fn area(ink: &BezPath) -> f64 {
        crate::geometry::subpaths(ink)
            .iter()
            .map(|s| s.area())
            .sum()
    }

    /// An eased side passes through its points and never past them.
    /// flip: `eased`'s zero slope at an extremum → fails at "overshoots".
    #[test]
    fn the_ease_meets_its_points_and_never_overshoots() {
        let p = WidthProfile {
            points: vec![
                WidthPoint {
                    t: 0.0,
                    left: 0.2,
                    right: 0.2,
                },
                WidthPoint {
                    t: 0.3,
                    left: 1.0,
                    right: 1.0,
                },
                WidthPoint {
                    t: 0.7,
                    left: 0.9,
                    right: 0.9,
                },
                WidthPoint {
                    t: 1.0,
                    left: 0.0,
                    right: 0.0,
                },
            ],
        };
        for wp in &p.points {
            assert!((p.at(wp.t).0 - wp.left).abs() < 1e-9, "at {}", wp.t);
        }
        for i in 0..=1000 {
            let (l, _) = p.at(f64::from(i) / 1000.0);
            assert!((-1e-9..=1.0 + 1e-9).contains(&l), "overshoots: {l} at {i}");
        }
        assert_eq!(Preset::TaperBoth.profile().at(0.5), (1.0, 1.0));
        assert_eq!(Preset::TaperBoth.profile().at(0.0), (0.0, 0.0));
    }

    /// An even profile lays a butt stroke's band exactly; a taper to nothing at
    /// one end lays half of it.
    #[test]
    fn an_even_profile_lays_the_band_and_a_taper_half_of_it() {
        let even = WidthProfile {
            points: vec![WidthPoint {
                t: 0.0,
                left: 1.0,
                right: 1.0,
            }],
        };
        let full = area(&ink(&line(100.0), &laid(&even, &[]), 0.01));
        assert!((full - 1000.0).abs() < 1e-6, "{full}");
        let taper = Preset::TaperStart.profile();
        let half = area(&ink(&line(100.0), &laid(&taper, &[]), 0.01));
        assert!((half - 500.0).abs() < 1.0, "{half}");
    }

    /// Dashes cut the profiled band: a half-on pattern lays half of it, each
    /// dash wide where the whole band is.
    /// flip: `ink` ignoring `Laid::dashes` → fails at "dashed".
    #[test]
    fn dashes_are_cut_from_the_profiled_band() {
        let even = WidthProfile {
            points: vec![WidthPoint {
                t: 0.0,
                left: 1.0,
                right: 1.0,
            }],
        };
        let dashed = area(&ink(&line(100.0), &laid(&even, &[10.0, 10.0]), 0.01));
        assert!((dashed - 500.0).abs() < 1e-6, "dashed: {dashed}");
        let offset = Laid {
            dash_offset: 5.0,
            ..laid(&even, &[10.0, 10.0])
        };
        let shifted = area(&ink(&line(100.0), &offset, 0.01));
        assert!((shifted - 500.0).abs() < 1e-6, "shifted: {shifted}");
    }

    /// A square cap adds half the width at each open end; a round one a half
    /// disc.
    #[test]
    fn caps_close_the_open_ends() {
        let even = WidthProfile {
            points: vec![WidthPoint {
                t: 0.0,
                left: 1.0,
                right: 1.0,
            }],
        };
        let square = Laid {
            caps: (Cap::Square, Cap::Butt),
            ..laid(&even, &[])
        };
        let a = area(&ink(&line(100.0), &square, 0.01));
        assert!((a - 1050.0).abs() < 1e-6, "square start: {a}");
        let round = Laid {
            caps: (Cap::Butt, Cap::Round),
            ..laid(&even, &[])
        };
        let a = area(&ink(&line(100.0), &round, 0.001));
        let disc = std::f64::consts::PI * 25.0 / 2.0;
        assert!((a - 1000.0 - disc).abs() < 0.5, "round end: {a}");
    }

    /// A profile reads from a preset's name, a bare list or the saved object,
    /// and an unknown name is refused naming the presets.
    #[test]
    fn a_profile_reads_its_short_forms() {
        let named: WidthProfile = serde_json::from_str(r#""Taper end""#).unwrap();
        assert_eq!(named.preset(), Some(Preset::TaperEnd));
        let list: WidthProfile = serde_json::from_str(r#"[{"t":0,"left":1,"right":0.5}]"#).unwrap();
        assert_eq!(list.points.len(), 1);
        let saved = serde_json::to_string(&Preset::Bulge.profile()).unwrap();
        let back: WidthProfile = serde_json::from_str(&saved).unwrap();
        assert_eq!(back.preset(), Some(Preset::Bulge));
        let err = serde_json::from_str::<WidthProfile>(r#""wobbly""#).unwrap_err();
        assert!(err.to_string().contains("taper_both"), "{err}");
    }

    /// Clamping sorts the points and holds them to 0–1; no points is no profile.
    #[test]
    fn clamping_sorts_and_holds_the_points() {
        let p = WidthProfile {
            points: vec![
                WidthPoint {
                    t: 0.9,
                    left: 3.0,
                    right: -1.0,
                },
                WidthPoint {
                    t: -2.0,
                    left: 0.5,
                    right: 0.5,
                },
                WidthPoint {
                    t: f64::NAN,
                    left: 0.5,
                    right: 0.5,
                },
            ],
        }
        .clamped()
        .unwrap();
        assert_eq!(p.points.len(), 2);
        assert_eq!(p.points[0].t, 0.0);
        assert_eq!((p.points[1].left, p.points[1].right), (1.0, 0.0));
        assert!(WidthProfile { points: Vec::new() }.clamped().is_none());
    }

    /// A point placed by fraction is found again at that fraction.
    #[test]
    fn a_fraction_round_trips_through_the_path() {
        let mut p = BezPath::new();
        p.move_to((0.0, 0.0));
        p.curve_to((50.0, 80.0), (120.0, -40.0), (200.0, 10.0));
        p.line_to((260.0, 90.0));
        for t in [0.0, 0.25, 0.6, 1.0] {
            let (at, _) = point_at(&p, t).unwrap();
            let (back, d) = fraction_at(&p, at).unwrap();
            assert!((back - t).abs() < 1e-4 && d < 1e-6, "{t} → {back} ({d})");
        }
    }
}
