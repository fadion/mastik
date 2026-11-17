//! *Distort*: three baked filters on a path's anchors, for the hand-drawn and
//! organic edge — *Roughen*, *Zig-zag* and *Pucker / Bloat*. Each acts on the
//! segments between the selected anchors, or on the whole path when none is
//! selected, and answers ordinary anchors, so nothing stays live and every writer
//! already draws the result.
//!
//! Sizes are in the anchors' own units, which the editor gives in world space.

use crate::content_fill::Rng;
use crate::edit::{PenAnchor, PenSubpath, PointRef};
use crate::kurbo::{CubicBez, ParamCurve, ParamCurveArclen, ParamCurveDeriv, Point, Rect, Vec2};

/// How many points *Roughen* adds per unit of edge at a detail of 1: one every
/// four units, a fine grain.
const ROUGHEN_DENSITY: f64 = 0.25;

/// The most points one segment gains, so a huge path at full detail stays a path.
const MAX_ADDED_PER_SEGMENT: usize = 2000;

/// Arc-length accuracy, in the anchors' units.
const ARCLEN_ACCURACY: f64 = 1e-3;

/// One filter and its numbers.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Filter {
    /// Every point moves up to `size` in a direction drawn from `seed`, after
    /// `detail` (`0..=1`) has added points along the edge to move.
    Roughen { size: f64, detail: f64, seed: u64 },
    /// Each segment gains `ridges` ridges `size` high, crest to trough, across it.
    ZigZag { size: f64, ridges: usize },
    /// Each segment's middle pulled toward the shape's centre (`amount < 0`, as far
    /// as the centre at `-1`) or pushed away from it (`> 0`); the anchors stay.
    PuckerBloat { amount: f64 },
}

/// What a filter made: the anchors, and the points it added, for the preview's
/// hollow dots.
#[derive(Clone, Debug, PartialEq)]
pub struct Distorted {
    pub subpaths: Vec<PenSubpath>,
    pub added: Vec<Point>,
}

/// `filter` over `subpaths`, on the segments both of whose anchors are in
/// `selected`, or on every segment when `selected` is empty. `smooth` gives the
/// points Roughen and Zig-zag add, and the anchors they move, smooth handles;
/// otherwise they are corners and the segments they touch run straight.
pub fn distort(
    subpaths: &[PenSubpath],
    selected: &[PointRef],
    filter: Filter,
    smooth: bool,
) -> Distorted {
    let whole = selected.is_empty();
    let picked = |at: PointRef| whole || selected.contains(&at);
    let mut rng = match filter {
        Filter::Roughen { seed, .. } => Rng::new(&[seed]),
        _ => Rng::new(&[0]),
    };
    // Pucker / Bloat aims at the centre of the whole drawing.
    let centre = subpaths
        .iter()
        .flat_map(|s| s.anchors.iter().map(|a| a.at))
        .fold(None, |r: Option<Rect>, p| {
            Some(r.map_or(Rect::from_points(p, p), |r| r.union_pt(p)))
        })
        .map_or(Point::ZERO, |r| r.center());
    let mut added = Vec::new();
    let out = subpaths
        .iter()
        .enumerate()
        .map(|(si, sub)| {
            let n = sub.anchors.len();
            let segs = match (sub.closed, n) {
                (_, 0 | 1) => 0,
                (true, _) => n,
                (false, _) => n - 1,
            };
            let in_scope = |i: usize| picked((si, i)) && picked((si, (i + 1) % n));
            if let Filter::PuckerBloat { amount } = filter {
                let mut anchors = sub.anchors.clone();
                for i in (0..segs).filter(|i| in_scope(*i)) {
                    let j = (i + 1) % n;
                    let seg = cubic(&sub.anchors[i], &sub.anchors[j]);
                    // Both controls move by `v`, which moves the middle by ¾ of it.
                    let v = (seg.eval(0.5) - centre) * amount * 4.0 / 3.0;
                    anchors[i].out += v;
                    anchors[j].back += v;
                }
                return PenSubpath {
                    anchors,
                    closed: sub.closed,
                };
            }
            // The anchors in order, each followed by the points its segment gains,
            // and whether each output segment was distorted.
            let mut anchors: Vec<PenAnchor> = Vec::new();
            let mut bent: Vec<bool> = Vec::new();
            for i in 0..n {
                let mut a = sub.anchors[i];
                if let Filter::Roughen { size, .. } = filter
                    && picked((si, i))
                    && (whole || (0..segs).any(|s| in_scope(s) && (s == i || (s + 1) % n == i)))
                {
                    a.at += jitter(&mut rng, size);
                }
                anchors.push(a);
                if i >= segs {
                    continue;
                }
                let scoped = in_scope(i);
                bent.push(scoped);
                if !scoped {
                    continue;
                }
                let seg = cubic(&sub.anchors[i], &sub.anchors[(i + 1) % n]);
                let len = seg.arclen(ARCLEN_ACCURACY);
                let at = |f: f64| seg.inv_arclen(len * f, ARCLEN_ACCURACY);
                let points: Vec<Point> = match filter {
                    Filter::Roughen { size, detail, .. } => {
                        let count = ((len * detail.clamp(0.0, 1.0) * ROUGHEN_DENSITY).round()
                            as usize)
                            .min(MAX_ADDED_PER_SEGMENT);
                        (1..=count)
                            .map(|k| {
                                seg.eval(at(k as f64 / (count + 1) as f64)) + jitter(&mut rng, size)
                            })
                            .collect()
                    }
                    Filter::ZigZag { size, ridges } => {
                        let pieces = (ridges.clamp(1, MAX_ADDED_PER_SEGMENT / 2)) * 2;
                        (1..pieces)
                            .map(|k| {
                                let t = at(k as f64 / pieces as f64);
                                let d = seg.deriv().eval(t).to_vec2();
                                let normal = match d.hypot() > 1e-12 {
                                    true => Vec2::new(-d.y, d.x) / d.hypot(),
                                    false => Vec2::ZERO,
                                };
                                let side = if k % 2 == 1 { 0.5 } else { -0.5 };
                                seg.eval(t) + normal * size * side
                            })
                            .collect()
                    }
                    Filter::PuckerBloat { .. } => Vec::new(),
                };
                added.extend(points.iter().copied());
                for p in points {
                    anchors.push(PenAnchor::corner(p));
                    bent.push(true);
                }
            }
            shape_handles(&mut anchors, &bent, sub.closed, smooth);
            PenSubpath {
                anchors,
                closed: sub.closed,
            }
        })
        .collect();
    Distorted {
        subpaths: out,
        added,
    }
}

/// The segment from `a` to `b` as a cubic.
fn cubic(a: &PenAnchor, b: &PenAnchor) -> CubicBez {
    CubicBez::new(a.at, a.leaving(), b.arriving(), b.at)
}

/// A move of at most `size` in a direction drawn from `rng`, evenly over the disc.
fn jitter(rng: &mut Rng, size: f64) -> Vec2 {
    let angle = rng.unit() * std::f64::consts::TAU;
    let reach = rng.unit().sqrt() * size;
    Vec2::new(angle.cos(), angle.sin()) * reach
}

/// The handles of every anchor a distorted segment touches: on the side facing
/// such a segment, none for a corner, or a Catmull-Rom tangent through the
/// neighbours for `smooth`. `bent[j]` is the segment from anchor `j` to the next.
fn shape_handles(anchors: &mut [PenAnchor], bent: &[bool], closed: bool, smooth: bool) {
    let n = anchors.len();
    if n < 2 {
        return;
    }
    let at: Vec<Point> = anchors.iter().map(|a| a.at).collect();
    for j in 0..n {
        let prev = match (j, closed) {
            (0, true) => Some(n - 1),
            (0, false) => None,
            _ => Some(j - 1),
        };
        let into = prev.is_some_and(|p| bent.get(p).copied().unwrap_or(false));
        let leaving = bent.get(j).copied().unwrap_or(false);
        if !into && !leaving {
            continue;
        }
        let before = prev.map(|p| at[p]);
        let after = (j + 1 < n || closed).then(|| at[(j + 1) % n]);
        let a = &mut anchors[j];
        a.radius = 0.0;
        if !smooth {
            if into {
                a.back = Vec2::ZERO;
            }
            if leaving {
                a.out = Vec2::ZERO;
            }
            continue;
        }
        match (into, leaving, before, after) {
            (true, true, Some(p), Some(q)) => {
                let t = (q - p) / 6.0;
                a.out = t;
                a.back = -t;
            }
            _ => {
                if into && let Some(p) = before {
                    a.back = (p - a.at) / 3.0 * 0.5;
                }
                if leaving && let Some(q) = after {
                    a.out = (q - a.at) / 3.0 * 0.5;
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A closed square of side 100 as four corners.
    fn square() -> Vec<PenSubpath> {
        vec![PenSubpath {
            anchors: [(0.0, 0.0), (100.0, 0.0), (100.0, 100.0), (0.0, 100.0)]
                .map(|p| PenAnchor::corner(p.into()))
                .to_vec(),
            closed: true,
        }]
    }

    /// Roughen moves every point no further than its size, adds about a point
    /// per four units at full detail, and is the same picture for the same seed.
    #[test]
    fn roughen_stays_within_its_size_and_follows_its_seed() {
        let rough = |seed| Filter::Roughen {
            size: 6.0,
            detail: 1.0,
            seed,
        };
        let a = distort(&square(), &[], rough(7), false);
        assert_eq!(a.added.len(), 4 * 25, "25 points on each 100-unit side");
        let sq = crate::kurbo::Rect::new(0.0, 0.0, 100.0, 100.0);
        for p in a.subpaths[0].anchors.iter().map(|a| a.at) {
            let off = sq.inflate(6.0 + 1e-9, 6.0 + 1e-9);
            assert!(off.contains(p), "{p:?} moved past its size");
        }
        assert_eq!(a, distort(&square(), &[], rough(7), false), "seeded");
        assert_ne!(
            a,
            distort(&square(), &[], rough(8), false),
            "another seed, another picture"
        );
    }

    /// Zig-zag gives each segment `ridges` crests, `size` from crest to trough,
    /// and leaves the anchors where they were. flip: the side as ±1 → fails at
    /// the height.
    #[test]
    fn zig_zag_puts_its_ridges_on_every_segment() {
        let z = distort(
            &square(),
            &[],
            Filter::ZigZag {
                size: 8.0,
                ridges: 5,
            },
            false,
        );
        let anchors = &z.subpaths[0].anchors;
        assert_eq!(anchors.len(), 4 + 4 * 9, "nine new points a side");
        assert_eq!(anchors[0].at, Point::new(0.0, 0.0), "the corner stays");
        // The top side runs along y = 0: its crests and troughs are 4 either side.
        let ys: Vec<f64> = anchors[1..10].iter().map(|a| a.at.y).collect();
        let (lo, hi) = ys
            .iter()
            .fold((f64::MAX, f64::MIN), |(l, h), y| (l.min(*y), h.max(*y)));
        assert!(
            (hi - lo - 8.0).abs() < 1e-6,
            "crest to trough is the size: {ys:?}"
        );
        assert!(anchors.iter().all(|a| a.is_corner()), "corner points");
        let smooth = distort(
            &square(),
            &[],
            Filter::ZigZag {
                size: 8.0,
                ridges: 5,
            },
            true,
        );
        assert!(
            smooth.subpaths[0].anchors[2].is_smooth() && !smooth.subpaths[0].anchors[2].is_corner()
        );
    }

    /// Pucker pulls each side's middle toward the centre and Bloat pushes it out;
    /// the anchors stay and none is added.
    #[test]
    fn pucker_and_bloat_move_the_middles_only() {
        let mid_of_top = |amount: f64| {
            let d = distort(&square(), &[], Filter::PuckerBloat { amount }, false);
            assert!(d.added.is_empty());
            let a = &d.subpaths[0].anchors;
            assert_eq!(a[0].at, Point::new(0.0, 0.0));
            cubic(&a[0], &a[1]).eval(0.5)
        };
        assert!(
            (mid_of_top(-1.0) - Point::new(50.0, 50.0)).hypot() < 1e-9,
            "to the centre"
        );
        assert!(
            (mid_of_top(0.5) - Point::new(50.0, -25.0)).hypot() < 1e-9,
            "half as far out"
        );
    }

    /// With points selected only the segments between them change.
    #[test]
    fn a_selection_narrows_the_filter() {
        let z = distort(
            &square(),
            &[(0, 0), (0, 1)],
            Filter::ZigZag {
                size: 8.0,
                ridges: 2,
            },
            false,
        );
        assert_eq!(z.added.len(), 3, "the top side only");
        assert_eq!(z.subpaths[0].anchors.len(), 7);
    }
}
