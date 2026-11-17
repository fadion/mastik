//! A symmetric path: a `NodeKind::Path` that says which of its own axes it is
//! mirrored about, or how many copies of it turn about a centre, so point
//! editing can keep it so.
//!
//! The whole outline is stored, every copy as real points; the rule only says
//! which symmetry the points have. Its place is not stored. A shape symmetric
//! about a vertical line has its points' bounds centred on that line, so the axis
//! is the centre of the path's control box ([`centre`]); a shape that turns onto
//! itself about a point has its anchors' mean there ([`radial_centre`]). Neither
//! can drift from the points. The rule is kept only while it holds ([`kept`]):
//! any write that leaves the path asymmetric drops it, and one that moves or
//! scales it carries it along.

use crate::edit::{PenAnchor, PenSubpath};
use crate::geometry::anchor_runs;
use kurbo::{Affine, BezPath, PathSeg, Point, Vec2};
use serde::{Deserialize, Serialize};

/// The fewest copies a radial path turns: two would be a half turn, which reads
/// as a mirror.
pub const MIN_COPIES: u8 = 3;
/// The most copies a radial path turns.
pub const MAX_COPIES: u8 = 12;

/// Which symmetry a path has: mirrored across the vertical line through its
/// centre, the horizontal one or both, or `copies` turned about a centre, each
/// mirrored too when `reflect`.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct PathMirror {
    #[serde(default, skip_serializing_if = "is_false")]
    pub vertical: bool,
    #[serde(default, skip_serializing_if = "is_false")]
    pub horizontal: bool,
    /// How many copies turn about the centre, the drawn one counted: 0 for none,
    /// else [`MIN_COPIES`] to [`MAX_COPIES`].
    #[serde(default, skip_serializing_if = "is_zero")]
    pub copies: u8,
    /// Under `copies`, each copy mirrored across a line through the centre too.
    #[serde(default, skip_serializing_if = "is_false")]
    pub reflect: bool,
}

fn is_false(b: &bool) -> bool {
    !*b
}

fn is_zero(n: &u8) -> bool {
    *n == 0
}

impl PathMirror {
    /// No axis: a plain path.
    pub const NONE: Self = Self {
        vertical: false,
        horizontal: false,
        copies: 0,
        reflect: false,
    };

    /// `copies` turned about a centre, each mirrored too when `reflect`.
    pub fn around(copies: u8, reflect: bool) -> Self {
        Self {
            copies,
            reflect,
            ..Self::NONE
        }
    }

    /// Whether the path has no symmetry to keep.
    pub fn is_none(&self) -> bool {
        !self.vertical && !self.horizontal && self.copies == 0
    }

    /// Whether copies turn about a centre.
    pub fn is_radial(&self) -> bool {
        self.copies > 0
    }
}

/// Where a path's axes cross, in its own space: the centre of its control box.
pub fn centre(path: &BezPath) -> Point {
    path.control_box().center()
}

/// Where a radial path's copies turn about, in its own space: the mean of its
/// anchors, which a turn of the set onto itself leaves where it is.
pub fn radial_centre(path: &BezPath) -> Point {
    let (sum, n) = anchor_runs(path)
        .iter()
        .flat_map(|r| r.points.iter())
        .fold((Vec2::ZERO, 0.0), |(s, n), p| (s + p.to_vec2(), n + 1.0));
    if n == 0.0 {
        return centre(path);
    }
    (sum / n).to_point()
}

/// The point `mirror`'s symmetry is about, in the path's own space.
pub fn origin(path: &BezPath, mirror: PathMirror) -> Point {
    match mirror.is_radial() {
        true => radial_centre(path),
        false => centre(path),
    }
}

/// `mirror` less any symmetry `path` no longer has, with `radii` (indexed as
/// `NodeKind::Path::corner_radii`) carried by it too.
pub fn kept(path: &BezPath, radii: &[f64], mirror: PathMirror) -> PathMirror {
    if mirror.is_none() {
        return mirror;
    }
    let c = centre(path);
    let radial = (MIN_COPIES..=MAX_COPIES).contains(&mirror.copies) && {
        let o = radial_centre(path);
        holds(
            path,
            radii,
            turn_about(o, std::f64::consts::TAU / f64::from(mirror.copies)),
        )
    };
    PathMirror {
        vertical: mirror.vertical && holds(path, radii, reflection(true, c.x)),
        horizontal: mirror.horizontal && holds(path, radii, reflection(false, c.y)),
        copies: if radial { mirror.copies } else { 0 },
        reflect: radial && mirror.reflect && reflection_axis(path, radii).is_some(),
    }
}

/// The reflection of `p` across the vertical line `x = c` or the horizontal `y = c`.
pub fn reflect(p: Point, vertical: bool, c: f64) -> Point {
    match vertical {
        true => Point::new(2.0 * c - p.x, p.y),
        false => Point::new(p.x, 2.0 * c - p.y),
    }
}

/// The reflection across the vertical line `x = c` or the horizontal `y = c`.
fn reflection(vertical: bool, c: f64) -> Affine {
    match vertical {
        true => Affine::new([-1.0, 0.0, 0.0, 1.0, 2.0 * c, 0.0]),
        false => Affine::new([1.0, 0.0, 0.0, -1.0, 0.0, 2.0 * c]),
    }
}

/// A turn by `angle` about `c`.
pub fn turn_about(c: Point, angle: f64) -> Affine {
    Affine::translate(c.to_vec2()) * Affine::rotate(angle) * Affine::translate(-c.to_vec2())
}

/// The reflection across the line through `c` at `angle` from the x axis.
pub fn reflect_through(c: Point, angle: f64) -> Affine {
    let (s, k) = (2.0 * angle).sin_cos();
    Affine::translate(c.to_vec2())
        * Affine::new([k, s, s, -k, 0.0, 0.0])
        * Affine::translate(-c.to_vec2())
}

/// The angle of a line through a radial path's centre it is mirrored across,
/// if it is: found by pairing its first anchor off the centre with each anchor
/// as far out, since a reflection carries that anchor onto one of them.
pub fn reflection_axis(path: &BezPath, radii: &[f64]) -> Option<f64> {
    let c = radial_centre(path);
    let b = path.control_box();
    let tol = 1e-6 * (1.0 + b.width().max(b.height()));
    let anchors: Vec<Point> = anchor_runs(path)
        .iter()
        .flat_map(|r| r.points.clone())
        .collect();
    let a0 = *anchors.iter().find(|p| (**p - c).hypot() > tol)?;
    let (r0, t0) = ((a0 - c).hypot(), (a0 - c).atan2());
    anchors
        .iter()
        .filter(|p| ((**p - c).hypot() - r0).abs() <= tol)
        .map(|p| (t0 + (*p - c).atan2()) / 2.0)
        .find(|angle| holds(path, radii, reflect_through(c, *angle)))
}

/// Whether every segment of `path` has its image under `map` in the path,
/// either way round, and every anchor's radius is its image's.
fn holds(path: &BezPath, radii: &[f64], map: Affine) -> bool {
    let b = path.control_box();
    let tol = 1e-6 * (1.0 + b.width().max(b.height()));
    let runs = anchor_runs(path);
    let segs: Vec<Vec<Point>> = runs
        .iter()
        .flat_map(|r| r.segs.iter().map(seg_points))
        .collect();
    let images: Vec<Vec<Point>> = segs
        .iter()
        .map(|s| s.iter().map(|p| map * *p).collect())
        .collect();
    if !matches_all(&segs, &images, tol, |a, b| same_seg(a, b, tol)) {
        return false;
    }
    // A lone anchor has no segment to carry it.
    let mut anchors = Vec::new();
    let mut k = 0;
    for run in &runs {
        for p in &run.points {
            anchors.push(vec![
                *p,
                Point::new(radii.get(k).copied().unwrap_or(0.0), 0.0),
            ]);
            k += 1;
        }
    }
    let mirrored: Vec<Vec<Point>> = anchors.iter().map(|a| vec![map * a[0], a[1]]).collect();
    matches_all(&anchors, &mirrored, tol, |a, b| {
        (a[0] - b[0]).hypot() <= tol && (a[1].x - b[1].x).abs() <= tol
    })
}

/// A segment's points, anchor to anchor.
fn seg_points(s: &PathSeg) -> Vec<Point> {
    match *s {
        PathSeg::Line(l) => vec![l.p0, l.p1],
        PathSeg::Quad(q) => vec![q.p0, q.p1, q.p2],
        PathSeg::Cubic(c) => vec![c.p0, c.p1, c.p2, c.p3],
    }
}

/// Whether two segments are one curve, either way round.
fn same_seg(a: &[Point], b: &[Point], tol: f64) -> bool {
    let near = |p: &Point, q: &Point| (*p - *q).hypot() <= tol;
    a.len() == b.len()
        && (a.iter().zip(b).all(|(p, q)| near(p, q))
            || a.iter().zip(b.iter().rev()).all(|(p, q)| near(p, q)))
}

/// Whether `items` and `images` pair off one to one under `same`, matched by
/// their centroids (which neither reversal nor order changes) within `tol`.
fn matches_all(
    items: &[Vec<Point>],
    images: &[Vec<Point>],
    tol: f64,
    same: impl Fn(&[Point], &[Point]) -> bool,
) -> bool {
    if items.len() != images.len() {
        return false;
    }
    let centroid = |s: &[Point]| {
        let n = s.len().max(1) as f64;
        let sum = s.iter().fold(Point::ZERO, |acc, p| acc + p.to_vec2());
        Point::new(sum.x / n, sum.y / n)
    };
    let mut pool: Vec<(Point, usize)> = images
        .iter()
        .enumerate()
        .map(|(i, s)| (centroid(s), i))
        .collect();
    pool.sort_by(|a, b| a.0.x.total_cmp(&b.0.x));
    let mut used = vec![false; pool.len()];
    for item in items {
        let at = centroid(item);
        let lo = pool.partition_point(|(p, _)| p.x < at.x - tol);
        let found = (lo..pool.len())
            .take_while(|&j| pool[j].0.x <= at.x + tol)
            .find(|&j| {
                !used[j] && (pool[j].0.y - at.y).abs() <= tol && same(item, &images[pool[j].1])
            });
        match found {
            Some(j) => used[j] = true,
            None => return false,
        }
    }
    true
}

// --- a mirrored path under point editing -----------------------------------

/// A handle offset reflected across a vertical or horizontal axis.
fn flip(v: Vec2, vertical: bool) -> Vec2 {
    match vertical {
        true => Vec2::new(-v.x, v.y),
        false => Vec2::new(v.x, -v.y),
    }
}

/// `a`'s image across the axis `x = c` (or `y = c`): `swap` when the run passes
/// its image the other way round, so the image's arriving handle is the image of
/// `a`'s leaving one.
fn image_of(a: &PenAnchor, vertical: bool, c: f64, swap: bool) -> PenAnchor {
    let (out, back) = match swap {
        true => (flip(a.back, vertical), flip(a.out, vertical)),
        false => (flip(a.out, vertical), flip(a.back, vertical)),
    };
    PenAnchor {
        at: reflect(a.at, vertical, c),
        out,
        back,
        radius: a.radius,
    }
}

/// Each anchor's image across one axis among `subs`, by flat index, with
/// whether the run passes it the other way round; `None` where no anchor sits
/// at the image.
///
/// The way round comes from the neighbours, not the handles: a corner has none
/// to tell by, and pulling one out of it must still land on the right side.
pub fn partners(
    subs: &[PenSubpath],
    vertical: bool,
    c: f64,
    tol: f64,
) -> Vec<Option<(usize, bool)>> {
    partners_by(subs, |p| reflect(p, vertical, c), true, tol)
}

/// [`partners`] under any isometry `map`, an `involution` (a reflection) pairing
/// anchors both ways: each anchor's image by flat index, and whether the run
/// passes it the other way round.
pub fn partners_by(
    subs: &[PenSubpath],
    map: impl Fn(Point) -> Point,
    involution: bool,
    tol: f64,
) -> Vec<Option<(usize, bool)>> {
    let flat: Vec<&PenAnchor> = subs.iter().flat_map(|s| &s.anchors).collect();
    // Each anchor's neighbours along its run, by flat index.
    let mut prev = Vec::with_capacity(flat.len());
    let mut next = Vec::with_capacity(flat.len());
    let mut start = 0;
    for s in subs {
        let n = s.anchors.len();
        for i in 0..n {
            let wrap = |j: usize| start + j % n;
            prev.push(match (i, s.closed) {
                (0, false) => None,
                (0, true) => Some(wrap(n - 1)),
                _ => Some(start + i - 1),
            });
            next.push(match (i + 1 == n, s.closed) {
                (true, false) => None,
                _ => Some(wrap(i + 1)),
            });
        }
        start += n;
    }
    // Pair one to one. Among anchors coincident at the image's place, take the one
    // whose neighbours sit at the images of this one's, either way round (an end
    // matching an end), so the two ends a split leaves pair with their own images.
    let near = |p: Point, q: Point| (p - q).hypot() <= tol;
    let fits = |x: Option<usize>, y: Option<usize>| match (x, y) {
        (None, None) => 1,
        (Some(x), Some(y)) => usize::from(near(map(flat[x].at), flat[y].at)),
        _ => 0,
    };
    let score = |a: usize, b: usize| {
        let along = fits(next[a], next[b]) + fits(prev[a], prev[b]);
        let against = fits(next[a], prev[b]) + fits(prev[a], next[b]);
        along.max(against)
    };
    // Anchors by x, so the ones at an image's place are a short range.
    let mut by_x: Vec<usize> = (0..flat.len()).collect();
    by_x.sort_by(|&p, &q| flat[p].at.x.total_cmp(&flat[q].at.x).then(p.cmp(&q)));
    // `at[a]` is a's image; `taken[b]` that b is already some anchor's. A
    // reflection pairs both ways at once; a turn only carries a forward.
    let mut at: Vec<Option<usize>> = vec![None; flat.len()];
    let mut taken = vec![false; flat.len()];
    for a in 0..flat.len() {
        if at[a].is_some() {
            continue;
        }
        let want = map(flat[a].at);
        let lo = by_x.partition_point(|&j| flat[j].at.x < want.x - tol);
        let mut here: Vec<usize> = by_x[lo..]
            .iter()
            .copied()
            .take_while(|&j| flat[j].at.x <= want.x + tol)
            .filter(|&b| !taken[b] && near(flat[b].at, want))
            .collect();
        here.sort_unstable();
        let mut best: Option<(usize, usize)> = None;
        for b in here {
            let s = score(a, b);
            // On a tie another anchor beats this one itself: two partners
            // dragged onto the axis must still come apart.
            let s = 2 * s + usize::from(b != a);
            if best.is_none_or(|(_, bs)| s > bs) {
                best = Some((b, s));
            }
        }
        if let Some((b, _)) = best {
            at[a] = Some(b);
            taken[b] = true;
            if involution {
                at[b] = Some(a);
                taken[a] = true;
            }
        }
    }
    (0..flat.len())
        .map(|a| {
            let p = at[a]?;
            let image = |j: Option<usize>| j.and_then(|j| at[j]);
            let swap = match (next[a], prev[a]) {
                (Some(_), _) => image(next[a]).is_some() && image(next[a]) == prev[p],
                (None, Some(_)) => image(prev[a]).is_some() && image(prev[a]) == next[p],
                (None, None) => false,
            };
            Some((p, swap))
        })
        .collect()
}

/// Whether point editing moved, reshaped or re-rounded anchor `a` into `b`.
fn changed(a: &PenAnchor, b: &PenAnchor, eps: f64) -> bool {
    (a.at - b.at).hypot() > eps
        || (a.out - b.out).hypot() > eps
        || (a.back - b.back).hypot() > eps
        || (a.radius - b.radius).abs() > eps
}

/// `edited`, made symmetric again across `mirror`'s axes through `at` after a
/// point edit that started from the symmetric `base`, both in the path's own
/// space. Where an anchor changed, its image takes the reflection of it; where
/// both did, the first in order wins. An anchor on an axis stays on it, its two
/// handles each other's image. An edit that changed how many anchors there are
/// is left alone: the model drops the axis if it no longer holds. So is one that
/// changed every anchor, or mapped them all by one affine map ([`one_map`]: a
/// point-box scale holding an edge still), and one that moved each
/// pair together and left the path symmetric about its own centre: the axis
/// follows a symmetric result and drops from any other. A lone anchor's edit that
/// happens to be symmetric about a new centre still takes its image along.
pub fn symmetric_edit(
    base: &[PenSubpath],
    edited: &mut [PenSubpath],
    mirror: PathMirror,
    at: Point,
) {
    if mirror.is_radial() {
        return radial_edit(base, edited, mirror, false);
    }
    let shape = |s: &[PenSubpath]| s.iter().map(|s| s.anchors.len()).collect::<Vec<_>>();
    if shape(base) != shape(edited) {
        return;
    }
    let size = base
        .iter()
        .flat_map(|s| &s.anchors)
        .map(|a| (a.at - at).hypot())
        .fold(0.0, f64::max);
    let tol = 1e-6 * (1.0 + size);
    let flat_base: Vec<PenAnchor> = base.iter().flat_map(|s| s.anchors.clone()).collect();
    let axes = [
        (true, at.x, mirror.vertical),
        (false, at.y, mirror.horizontal),
    ]
    .map(|(vertical, c, on)| on.then(|| (vertical, c, partners(base, vertical, c, tol))));
    let moved: Vec<bool> = flat_base
        .iter()
        .zip(edited.iter().flat_map(|s| &s.anchors))
        .map(|(a, b)| changed(a, b, tol))
        .collect();
    let together = axes.iter().flatten().all(|(_, _, pairs)| {
        pairs
            .iter()
            .enumerate()
            .all(|(a, p)| p.is_none_or(|(p, _)| moved[a] == moved[p]))
    });
    let flat_edited: Vec<PenAnchor> = edited.iter().flat_map(|s| s.anchors.clone()).collect();
    if moved.iter().all(|m| *m) || one_map(&flat_base, &flat_edited, tol) {
        return;
    }
    if together
        && kept(
            &crate::edit::pen_path(edited),
            &crate::edit::pen_radii(edited),
            mirror,
        ) == mirror
    {
        return;
    }
    remirror_pairs(&flat_base, edited, axes, tol);
}

/// Whether `edited` is `base` under one affine map, anchors and handles alike and
/// no radius changed, with `base` spanning both directions: a point-box scale or turn of every point,
/// which may hold one edge still and so not move every anchor. A base on one line
/// fixes no map, so a lone anchor's drag along it is not taken for one.
fn one_map(base: &[PenAnchor], edited: &[PenAnchor], tol: f64) -> bool {
    let Some(p0) = base.first().map(|a| a.at) else {
        return false;
    };
    let far = |key: &dyn Fn(Point) -> f64| {
        (0..base.len()).max_by(|i, j| key(base[*i].at).total_cmp(&key(base[*j].at)))
    };
    let Some(i1) = far(&|p| (p - p0).hypot2()) else {
        return false;
    };
    let u = base[i1].at - p0;
    let Some(i2) = far(&|p| u.cross(p - p0).abs()) else {
        return false;
    };
    let v = base[i2].at - p0;
    let det = u.cross(v);
    if det.abs() <= tol * (1.0 + u.hypot()) {
        return false;
    }
    let q0 = edited[0].at;
    let (eu, ev) = (edited[i1].at - q0, edited[i2].at - q0);
    // The linear part takes `u` to `eu` and `v` to `ev`.
    let inv = Affine::new([v.y / det, -u.y / det, -v.x / det, u.x / det, 0.0, 0.0]);
    let linear = Affine::new([eu.x, eu.y, ev.x, ev.y, 0.0, 0.0]) * inv;
    let map = Affine::translate(q0.to_vec2()) * linear * Affine::translate(-p0.to_vec2());
    base.iter().zip(edited).all(|(a, b)| {
        (map * a.at - b.at).hypot() <= tol
            && (a.radius - b.radius).abs() <= tol
            && (linear * a.out.to_point() - b.out.to_point()).hypot() <= tol
            && (linear * a.back.to_point() - b.back.to_point()).hypot() <= tol
    })
}

/// `edited` made symmetric across `mirror`'s axes through `at` whatever the edit
/// was: [`symmetric_edit`]'s rule without its exceptions, for a caller that changed
/// every anchor on purpose and wants each pair's first anchor to decide.
pub fn remirror(base: &[PenSubpath], edited: &mut [PenSubpath], mirror: PathMirror, at: Point) {
    if mirror.is_radial() {
        return radial_edit(base, edited, mirror, true);
    }
    let shape = |s: &[PenSubpath]| s.iter().map(|s| s.anchors.len()).collect::<Vec<_>>();
    if shape(base) != shape(edited) {
        return;
    }
    let size = base
        .iter()
        .flat_map(|s| &s.anchors)
        .map(|a| (a.at - at).hypot())
        .fold(0.0, f64::max);
    let tol = 1e-6 * (1.0 + size);
    let flat_base: Vec<PenAnchor> = base.iter().flat_map(|s| s.anchors.clone()).collect();
    let axes = [
        (true, at.x, mirror.vertical),
        (false, at.y, mirror.horizontal),
    ]
    .map(|(vertical, c, on)| on.then(|| (vertical, c, partners(base, vertical, c, tol))));
    remirror_pairs(&flat_base, edited, axes, tol);
}

/// One mirrored axis: whether it is vertical, where it lies, and its [`partners`].
type Axis = (bool, f64, Vec<Option<(usize, bool)>>);

/// Each axis's pairs made each other's image again in `edited`: where one anchor
/// changed its image takes its reflection, where both did the first wins, and an
/// axis anchor is held to its axis.
fn remirror_pairs(
    flat_base: &[PenAnchor],
    edited: &mut [PenSubpath],
    axes: [Option<Axis>; 2],
    tol: f64,
) {
    for (vertical, c, pairs) in axes.into_iter().flatten() {
        let mut flat: Vec<PenAnchor> = edited.iter().flat_map(|s| s.anchors.clone()).collect();
        let mut done = vec![false; flat.len()];
        for a in 0..flat.len() {
            let Some((p, swap)) = pairs[a] else { continue };
            if done[a] {
                continue;
            }
            done[a] = true;
            done[p] = true;
            let (ca, cp) = (
                changed(&flat_base[a], &flat[a], tol),
                changed(&flat_base[p], &flat[p], tol),
            );
            if a == p {
                if ca {
                    flat[a] = on_axis(&flat_base[a], &flat[a], vertical, c, swap, tol);
                }
            } else if ca {
                flat[p] = image_of(&flat[a], vertical, c, swap);
            } else if cp {
                flat[a] = image_of(&flat[p], vertical, c, swap);
            }
        }
        let mut it = flat.into_iter();
        for sub in edited.iter_mut() {
            for a in &mut sub.anchors {
                *a = it.next().expect("one anchor per anchor");
            }
        }
    }
}

/// An anchor on the axis after an edit: pulled back onto it, so it slides along
/// it, with the handle the edit moved deciding the other.
fn on_axis(
    was: &PenAnchor,
    now: &PenAnchor,
    vertical: bool,
    c: f64,
    swap: bool,
    tol: f64,
) -> PenAnchor {
    let mut a = *now;
    match vertical {
        true => a.at.x = c,
        false => a.at.y = c,
    }
    // A run crossing the axis meets its image here: each handle is the other's.
    // One running along the axis keeps both handles on it.
    match swap {
        true if (now.back - was.back).hypot() > tol && (now.out - was.out).hypot() <= tol => {
            a.out = flip(a.back, vertical);
        }
        true => a.back = flip(a.out, vertical),
        false => {
            let along = |v: Vec2| match vertical {
                true => Vec2::new(0.0, v.y),
                false => Vec2::new(v.x, 0.0),
            };
            a.out = along(a.out);
            a.back = along(a.back);
        }
    }
    a
}

// --- a radial path under point editing --------------------------------------

/// Every map but the identity that carries a radial path onto itself, about its
/// centre: the turns, and the reflections when each copy is mirrored, each with
/// whether it reverses the run. Empty when the path's rule no longer holds.
fn radial_group(subs: &[PenSubpath], mirror: PathMirror) -> Vec<(Affine, bool)> {
    let path = crate::edit::pen_path(subs);
    let radii = crate::edit::pen_radii(subs);
    let n = mirror.copies;
    if kept(&path, &radii, mirror).copies != n {
        return Vec::new();
    }
    let c = radial_centre(&path);
    let step = std::f64::consts::TAU / f64::from(n);
    let mut out: Vec<(Affine, bool)> = (1..n)
        .map(|k| (turn_about(c, step * f64::from(k)), false))
        .collect();
    if mirror.reflect
        && let Some(axis) = reflection_axis(&path, &radii)
    {
        out.extend((0..n).map(|k| (reflect_through(c, axis + step * f64::from(k) / 2.0), true)));
    }
    out
}

/// `a` carried by `map`, whose linear part turns its handles; `swap` when the run
/// passes the image the other way round.
fn carried(a: &PenAnchor, map: Affine, swap: bool) -> PenAnchor {
    let lin = |v: Vec2| {
        let [m0, m1, m2, m3, _, _] = map.as_coeffs();
        Vec2::new(m0 * v.x + m2 * v.y, m1 * v.x + m3 * v.y)
    };
    let (out, back) = match swap {
        true => (lin(a.back), lin(a.out)),
        false => (lin(a.out), lin(a.back)),
    };
    PenAnchor {
        at: map * a.at,
        out,
        back,
        radius: a.radius,
    }
}

/// [`symmetric_edit`] and [`remirror`] for a radial path: each orbit of anchors
/// made one again, the first anchor in order that the edit changed deciding the
/// rest. An anchor a turn holds still is on the centre and is held there; one a
/// reflection holds still is on its line and slides along it. `force` skips the
/// cases [`symmetric_edit`] leaves alone.
fn radial_edit(base: &[PenSubpath], edited: &mut [PenSubpath], mirror: PathMirror, force: bool) {
    let shape = |s: &[PenSubpath]| s.iter().map(|s| s.anchors.len()).collect::<Vec<_>>();
    if shape(base) != shape(edited) {
        return;
    }
    let group = radial_group(base, mirror);
    if group.is_empty() {
        return;
    }
    let c = radial_centre(&crate::edit::pen_path(base));
    let size = base
        .iter()
        .flat_map(|s| &s.anchors)
        .map(|a| (a.at - c).hypot())
        .fold(0.0, f64::max);
    let tol = 1e-6 * (1.0 + size);
    let pairs: Vec<Vec<Option<(usize, bool)>>> = group
        .iter()
        .map(|(map, reverses)| partners_by(base, |p| *map * p, *reverses, tol))
        .collect();
    let flat_base: Vec<PenAnchor> = base.iter().flat_map(|s| s.anchors.clone()).collect();
    let mut flat: Vec<PenAnchor> = edited.iter().flat_map(|s| s.anchors.clone()).collect();
    let moved: Vec<bool> = flat_base
        .iter()
        .zip(&flat)
        .map(|(a, b)| changed(a, b, tol))
        .collect();
    if !force {
        if moved.iter().all(|m| *m) || one_map(&flat_base, &flat, tol) {
            return;
        }
        let together = pairs.iter().all(|p| {
            p.iter()
                .enumerate()
                .all(|(a, q)| q.is_none_or(|(q, _)| moved[a] == moved[q]))
        });
        let path = crate::edit::pen_path(edited);
        if together && kept(&path, &crate::edit::pen_radii(edited), mirror) == mirror {
            return;
        }
    }
    let mut done = vec![false; flat.len()];
    for a in 0..flat.len() {
        if done[a] {
            continue;
        }
        let orbit = |a: usize| pairs.iter().filter_map(move |p| p[a].map(|(q, _)| q));
        // The first changed anchor of the orbit decides it.
        let src = std::iter::once(a)
            .chain(orbit(a))
            .filter(|j| moved[*j])
            .min();
        done[a] = true;
        for q in orbit(a) {
            done[q] = true;
        }
        let Some(src) = src else { continue };
        let now = flat[src];
        let mut held = now;
        for ((map, reverses), p) in group.iter().zip(&pairs) {
            if let Some((q, swap)) = p[src]
                && q == src
            {
                held = match reverses {
                    false => PenAnchor { at: c, ..held },
                    true => along_line(&flat_base[src], &held, *map, c, swap, tol),
                };
            }
        }
        flat[src] = held;
        for ((map, _), p) in group.iter().zip(&pairs) {
            if let Some((q, swap)) = p[src]
                && q != src
            {
                flat[q] = carried(&held, *map, swap);
            }
        }
    }
    let mut it = flat.into_iter();
    for sub in edited.iter_mut() {
        for a in &mut sub.anchors {
            *a = it.next().expect("one anchor per anchor");
        }
    }
}

/// An anchor on the line a reflection `map` holds still, after an edit: turned so
/// the line stands vertical through `c`, held to it by [`on_axis`], turned back.
fn along_line(
    was: &PenAnchor,
    now: &PenAnchor,
    map: Affine,
    c: Point,
    swap: bool,
    tol: f64,
) -> PenAnchor {
    // The line's angle is half the reflection's: its matrix is [cos 2t, sin 2t; ...].
    let [m0, m1, ..] = map.as_coeffs();
    let angle = m1.atan2(m0) / 2.0;
    let upright = turn_about(c, std::f64::consts::FRAC_PI_2 - angle);
    let there = |a: &PenAnchor| carried(a, upright, false);
    let held = on_axis(&there(was), &there(now), true, c.x, swap, tol);
    carried(&held, upright.inverse(), false)
}

#[cfg(test)]
mod tests {
    use super::*;

    const BOTH: PathMirror = PathMirror {
        vertical: true,
        horizontal: true,
        ..PathMirror::NONE
    };

    /// A heart-ish closed run, symmetric about x = 50 only.
    fn heart() -> BezPath {
        let mut p = BezPath::new();
        p.move_to((50.0, 20.0));
        p.curve_to((60.0, 0.0), (90.0, 10.0), (80.0, 40.0));
        p.line_to((50.0, 90.0));
        p.line_to((20.0, 40.0));
        p.curve_to((10.0, 10.0), (40.0, 0.0), (50.0, 20.0));
        p.close_path();
        p
    }

    /// The heart keeps its vertical axis and loses the horizontal one it never had.
    #[test]
    fn a_symmetric_path_keeps_the_axis_it_is_symmetric_about() {
        assert_eq!(kept(&heart(), &[], BOTH), VERTICAL);
    }

    /// Moving one anchor off its image's place drops the axis.
    #[test]
    fn an_asymmetric_edit_drops_the_axis() {
        let mut p = heart();
        if let Some(kurbo::PathEl::LineTo(q)) = p.elements_mut().get_mut(2) {
            q.x += 4.0;
        }
        assert!(kept(&p, &[], BOTH).is_none());
    }

    /// A scale baked into the points keeps the axis: it is wherever the centre
    /// went.
    #[test]
    fn a_baked_scale_keeps_the_axis() {
        let mut p = heart();
        p.apply_affine(Affine::scale_non_uniform(2.5, 0.5) * Affine::translate((7.0, 3.0)));
        assert!(kept(&p, &[], BOTH).vertical);
    }

    /// One rounded corner without its image's radius breaks the symmetry.
    #[test]
    fn radii_must_mirror_too() {
        // Anchors: (50,20) (80,40) (50,90) (20,40).
        assert!(kept(&heart(), &[0.0, 6.0, 0.0, 6.0], BOTH).vertical);
        assert!(!kept(&heart(), &[0.0, 6.0, 0.0, 0.0], BOTH).vertical);
    }

    const VERTICAL: PathMirror = PathMirror {
        vertical: true,
        ..PathMirror::NONE
    };

    /// An open run of corners through `points`.
    fn run(points: &[(f64, f64)]) -> PenSubpath {
        PenSubpath {
            anchors: points
                .iter()
                .map(|&(x, y)| PenAnchor::corner(Point::new(x, y)))
                .collect(),
            closed: false,
        }
    }

    /// A closed diamond about x = 0, clockwise from the top: top and bottom on the
    /// axis, right and left each other's image.
    fn diamond() -> Vec<PenSubpath> {
        let mut d = run(&[(0.0, -10.0), (10.0, 0.0), (0.0, 10.0), (-10.0, 0.0)]);
        d.closed = true;
        vec![d]
    }

    /// Whether the model would keep the vertical axis on `subs`.
    fn holds(subs: &[PenSubpath]) -> bool {
        let path = crate::edit::pen_path(subs);
        kept(&path, &crate::edit::pen_radii(subs), VERTICAL).vertical
    }

    /// A dragged anchor takes its image with it, mirrored.
    /// flip: `symmetric_edit` returning early → fails at "the image followed".
    #[test]
    fn a_moved_anchor_moves_its_image() {
        let base = diamond();
        let mut edited = base.clone();
        edited[0].anchors[1].at = Point::new(14.0, 2.0);
        symmetric_edit(&base, &mut edited, VERTICAL, Point::ZERO);
        assert_eq!(
            edited[0].anchors[3].at,
            Point::new(-14.0, 2.0),
            "the image followed"
        );
        assert!(holds(&edited));
    }

    /// The image's own edit wins when only it moved.
    #[test]
    fn an_edit_on_the_image_side_carries_back() {
        let base = diamond();
        let mut edited = base.clone();
        edited[0].anchors[3].radius = 4.0;
        symmetric_edit(&base, &mut edited, VERTICAL, Point::ZERO);
        assert_eq!(edited[0].anchors[1].radius, 4.0);
        assert!(holds(&edited));
    }

    /// An anchor on the axis is locked to it: dragged off, it slides along it.
    /// flip: `on_axis` keeping `now.at` → fails at "still on the axis".
    #[test]
    fn an_axis_anchor_slides_along_the_axis() {
        let base = diamond();
        let mut edited = base.clone();
        edited[0].anchors[0].at = Point::new(3.0, -12.0);
        symmetric_edit(&base, &mut edited, VERTICAL, Point::ZERO);
        assert_eq!(
            edited[0].anchors[0].at,
            Point::new(0.0, -12.0),
            "still on the axis"
        );
        assert!(holds(&edited));
    }

    /// A handle pulled out of a corner lands on its image's matching side, which
    /// the neighbours decide since a corner has no handle to tell by; on the axis,
    /// the other handle becomes its image.
    /// flip: `partners` answering `swap` false always → fails at the handle assertion.
    #[test]
    fn a_handle_pulled_from_a_corner_mirrors_onto_the_right_side() {
        let base = diamond();
        let mut edited = base.clone();
        edited[0].anchors[1].out = Vec2::new(0.0, 5.0);
        edited[0].anchors[0].out = Vec2::new(6.0, 0.0);
        symmetric_edit(&base, &mut edited, VERTICAL, Point::ZERO);
        assert_eq!(edited[0].anchors[3].back, Vec2::new(0.0, 5.0));
        assert_eq!(edited[0].anchors[3].out, Vec2::ZERO);
        assert_eq!(edited[0].anchors[0].back, Vec2::new(-6.0, 0.0));
        assert!(holds(&edited));
    }

    /// After *Split at point* at the diamond's right and left anchors, each new end
    /// pairs with its own image, so dragging the two starting ends moves each by the
    /// drag and its image with it, and the axis holds.
    /// flip: `partners` taking the first anchor at the image's place, paired or not
    /// → fails at "its image". Either half of the fix alone (no re-pairing, the
    /// neighbour score) still passes: each covers this case for the other.
    #[test]
    fn split_ends_pair_with_their_own_images() {
        let base = crate::edit::split_at_points(&diamond(), &[(0, 1), (0, 3)]).0;
        let at = |subs: &[PenSubpath], s: usize, a: usize| subs[s].anchors[a].at;
        // [R0, B, L0] and [L1, T, R1].
        assert_eq!(at(&base, 0, 0), Point::new(10.0, 0.0));
        assert_eq!(at(&base, 1, 0), Point::new(-10.0, 0.0));
        let mut edited = base.clone();
        for (s, a) in [(0, 0), (1, 0)] {
            edited[s].anchors[a].at += Vec2::new(2.0, -2.0);
        }
        symmetric_edit(&base, &mut edited, VERTICAL, Point::ZERO);
        assert_eq!(
            at(&edited, 0, 0),
            Point::new(12.0, -2.0),
            "the end dragged right moved right"
        );
        assert_eq!(at(&edited, 0, 2), Point::new(-12.0, -2.0), "its image");
        assert_eq!(at(&edited, 1, 2), Point::new(8.0, -2.0));
        assert!(holds(&edited));
    }

    /// Nudging every anchor moves the path and the axis follows; turning every
    /// anchor turns the path, which drops the axis, rather than patching it back
    /// onto the old one.
    /// flip: `symmetric_edit` without its whole-set return → fails at "turned as asked".
    #[test]
    fn moving_every_anchor_moves_the_path() {
        let base = diamond();
        let mut edited = base.clone();
        for a in &mut edited[0].anchors {
            a.at += Vec2::new(1.0, 0.0);
        }
        let want = edited.clone();
        symmetric_edit(&base, &mut edited, VERTICAL, Point::ZERO);
        assert_eq!(edited, want, "moved as asked");
        assert!(holds(&edited));
        let mut turned = base.clone();
        for a in &mut turned[0].anchors {
            a.at = Affine::rotate(0.5) * a.at;
        }
        let want = turned.clone();
        symmetric_edit(&base, &mut turned, VERTICAL, Point::ZERO);
        assert_eq!(turned, want, "turned as asked");
    }

    /// Two partners dragged onto the axis pair with each other, not with
    /// themselves, so dragging one off pulls them apart again.
    /// flip: `partners` preferring the anchor itself on a tie → fails at "pulled apart".
    #[test]
    fn partners_on_the_axis_can_be_pulled_apart() {
        let mut d = run(&[(0.0, -10.0), (0.0, 0.0), (0.0, 10.0), (0.0, 0.0)]);
        d.closed = true;
        let base = vec![d];
        let mut edited = base.clone();
        edited[0].anchors[1].at = Point::new(3.0, 0.0);
        symmetric_edit(&base, &mut edited, VERTICAL, Point::ZERO);
        assert_eq!(
            edited[0].anchors[1].at,
            Point::new(3.0, 0.0),
            "pulled apart"
        );
        assert_eq!(edited[0].anchors[3].at, Point::new(-3.0, 0.0));
        assert!(holds(&edited));
    }

    /// Moving a pair together into a shape symmetric about its new centre keeps it;
    /// a lone anchor's drag that happens to be symmetric about a new centre still
    /// takes its image along.
    #[test]
    fn a_symmetric_pair_edit_is_kept_and_a_lone_one_mirrored() {
        let base = diamond();
        let mut edited = base.clone();
        edited[0].anchors[1].at = Point::new(14.0, 0.0);
        edited[0].anchors[3].at = Point::new(-14.0, 0.0);
        let want = edited.clone();
        symmetric_edit(&base, &mut edited, VERTICAL, Point::ZERO);
        assert_eq!(edited, want);
        let line = vec![run(&[(-10.0, 0.0), (10.0, 0.0)])];
        let mut longer = line.clone();
        longer[0].anchors[1].at = Point::new(20.0, 0.0);
        symmetric_edit(&line, &mut longer, VERTICAL, Point::ZERO);
        assert_eq!(longer[0].anchors[0].at, Point::new(-20.0, 0.0));
    }

    /// Every point scaled from a side handle of the point box is one map of the
    /// whole set, so it is kept: the held edge stays and the far one moves by the
    /// drag, rather than the far one's image moving the held edge.
    /// flip: `symmetric_edit` without its `one_map` return → fails at "the held
    /// edge stays".
    #[test]
    fn scaling_every_point_from_a_side_keeps_the_held_edge() {
        let base = diamond();
        let scale = Affine::translate((-10.0, 0.0))
            * Affine::scale_non_uniform(1.5, 1.0)
            * Affine::translate((10.0, 0.0));
        let mut edited = base.clone();
        for a in &mut edited[0].anchors {
            a.at = scale * a.at;
        }
        let want = edited.clone();
        symmetric_edit(&base, &mut edited, VERTICAL, Point::ZERO);
        assert_eq!(
            edited[0].anchors[3].at,
            Point::new(-10.0, 0.0),
            "the held edge stays"
        );
        assert_eq!(edited, want, "scaled as asked");
        assert!(holds(&edited), "symmetric about its new centre");

        let tall = Affine::translate((0.0, -10.0))
            * Affine::scale_non_uniform(1.0, 2.0)
            * Affine::translate((0.0, 10.0));
        let mut edited = base.clone();
        for a in &mut edited[0].anchors {
            a.at = tall * a.at;
        }
        let want = edited.clone();
        symmetric_edit(&base, &mut edited, BOTH, Point::ZERO);
        assert_eq!(edited, want, "both axes: scaled as asked");
    }

    /// A five-pointed star about (100, 50): outer points at radius 40, inner at
    /// 16, from straight up, clockwise.
    fn star() -> Vec<PenSubpath> {
        let c = Point::new(100.0, 50.0);
        let pts: Vec<(f64, f64)> = (0..10)
            .map(|k| {
                let r = if k % 2 == 0 { 40.0 } else { 16.0 };
                let t = -std::f64::consts::FRAC_PI_2 + std::f64::consts::PI * f64::from(k) / 5.0;
                (c.x + r * t.cos(), c.y + r * t.sin())
            })
            .collect();
        let mut s = run(&pts);
        s.closed = true;
        vec![s]
    }

    const FIVE: PathMirror = PathMirror {
        copies: 5,
        reflect: true,
        ..PathMirror::NONE
    };

    /// A star keeps five copies and their mirrors about its anchors' mean; a
    /// pinwheel, turned but not mirrored, keeps the copies only; a moved point
    /// drops both, and a count the shape does not turn by is dropped.
    #[test]
    fn a_star_keeps_its_copies_and_a_moved_point_drops_them() {
        let path = crate::edit::pen_path(&star());
        let c = radial_centre(&path);
        assert!((c - Point::new(100.0, 50.0)).hypot() < 1e-9, "{c:?}");
        assert_eq!(kept(&path, &[], FIVE), FIVE);
        assert_eq!(kept(&path, &[], PathMirror::around(4, false)).copies, 0);

        let mut pinwheel = star();
        for (k, a) in pinwheel[0].anchors.iter_mut().enumerate() {
            if k % 2 == 1 {
                a.at = turn_about(Point::new(100.0, 50.0), 0.2) * a.at;
            }
        }
        let path = crate::edit::pen_path(&pinwheel);
        assert_eq!(kept(&path, &[], FIVE), PathMirror::around(5, false));

        let mut moved = star();
        moved[0].anchors[2].at.x += 3.0;
        assert!(kept(&crate::edit::pen_path(&moved), &[], FIVE).is_none());
    }

    /// Dragging one outer point of the star takes the four others round with it,
    /// turned, and their mirrors: the star stays a five-fold star.
    ///
    /// flip: `symmetric_edit` without its radial arm → fails at "turned".
    #[test]
    fn a_moved_point_of_a_star_turns_its_copies_with_it() {
        let base = star();
        let mut edited = base.clone();
        // The top point, straight up from the centre, pulled further out.
        edited[0].anchors[0].at = Point::new(100.0, 0.0);
        symmetric_edit(&base, &mut edited, FIVE, Point::ZERO);
        let c = Point::new(100.0, 50.0);
        let want = turn_about(c, std::f64::consts::TAU / 5.0) * Point::new(100.0, 0.0);
        assert!((edited[0].anchors[2].at - want).hypot() < 1e-6, "turned");
        let path = crate::edit::pen_path(&edited);
        assert_eq!(kept(&path, &[], FIVE), FIVE);
    }

    /// An outer point sits on a mirror line through the centre: dragged sideways
    /// it slides along that line instead, and every copy follows.
    #[test]
    fn a_point_on_a_mirror_line_slides_along_it() {
        let base = star();
        let mut edited = base.clone();
        edited[0].anchors[0].at = Point::new(104.0, 4.0);
        symmetric_edit(&base, &mut edited, FIVE, Point::ZERO);
        assert!(
            (edited[0].anchors[0].at - Point::new(100.0, 4.0)).hypot() < 1e-6,
            "{:?}",
            edited[0].anchors[0].at
        );
        let path = crate::edit::pen_path(&edited);
        assert_eq!(kept(&path, &[], FIVE), FIVE);
    }

    /// The rule's two new keys are left out of a save while unset, so a mirrored
    /// or plain path saves as it did, and read back when set.
    #[test]
    fn the_radial_keys_save_only_when_set() {
        assert_eq!(
            serde_json::to_string(&VERTICAL).unwrap(),
            r#"{"vertical":true}"#
        );
        let five = serde_json::to_string(&FIVE).unwrap();
        assert_eq!(five, r#"{"copies":5,"reflect":true}"#);
        assert_eq!(serde_json::from_str::<PathMirror>(&five).unwrap(), FIVE);
    }

    /// Both axes: one moved anchor takes all three images with it.
    #[test]
    fn both_axes_carry_an_edit_to_all_four_quarters() {
        let mut sq = run(&[(10.0, 10.0), (-10.0, 10.0), (-10.0, -10.0), (10.0, -10.0)]);
        sq.closed = true;
        let base = vec![sq];
        let mut edited = base.clone();
        edited[0].anchors[0].at = Point::new(12.0, 15.0);
        symmetric_edit(&base, &mut edited, BOTH, Point::ZERO);
        let ats: Vec<Point> = edited[0].anchors.iter().map(|a| a.at).collect();
        assert_eq!(
            ats,
            [(12.0, 15.0), (-12.0, 15.0), (-12.0, -15.0), (12.0, -15.0)]
                .map(|(x, y)| Point::new(x, y))
        );
    }
}
