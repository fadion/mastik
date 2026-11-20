//! A layer that follows the drag: moved by the pointer within its axis and
//! range, then let go to stay, go back, or settle on a point, and its *Drop*
//! reactions run once it has settled.

use crate::player::Player;
use mastik_core::NodeId;
use mastik_core::kurbo::Vec2;
use mastik_core::prototype::{DropAt, Follow, FollowRange, Release, ScrollAxis, Trigger};

/// Past its range a following layer moves at this share of the pointer's pace,
/// so the edge is felt.
const GIVE: f64 = 1.0 / 3.0;
/// A release within this many milliseconds of the press throws the layer…
const QUICK_MS: f64 = 300.0;
/// …carrying on at its pace for this long before the nearest point is picked.
const THROW_MS: f64 = 200.0;

/// A following layer settling after a release.
pub(crate) struct MoveAnim {
    pub(crate) layer: NodeId,
    pub(crate) from: Vec2,
    pub(crate) to: Vec2,
    pub(crate) start: f64,
    pub(crate) timing: mastik_core::prototype::Timing,
    /// Where it will have come to rest, for its *Drop* reactions.
    pub(crate) at: DropAt,
}

impl Player {
    /// The settings of `id` if it follows the drag and takes input.
    pub(crate) fn follow_of(&self, id: NodeId) -> Option<Follow> {
        if self.disabled(id) {
            return None;
        }
        self.doc.get(id)?.play().follow.clone()
    }

    /// The deepest layer of `chain` that follows the drag.
    pub(crate) fn follower(&self, chain: &[NodeId]) -> Option<NodeId> {
        chain
            .iter()
            .copied()
            .find(|id| self.follow_of(*id).is_some())
    }

    /// How far `id` has been moved from where it is drawn.
    pub fn moved(&self, id: NodeId) -> Vec2 {
        self.moved.get(&id).copied().unwrap_or(Vec2::ZERO)
    }

    /// The offsets `layer` may take, as a box `(min, max)`; unbounded when
    /// its range is free.
    fn range(&self, layer: NodeId, f: &Follow) -> (Vec2, Vec2) {
        let free = (Vec2::new(f64::MIN, f64::MIN), Vec2::new(f64::MAX, f64::MAX));
        match f.range {
            FollowRange::Free => free,
            FollowRange::Parent => {
                let parent = self.doc.get(layer).and_then(|n| n.parent());
                let (Some(r), Some(p)) = (self.rect(layer), parent.and_then(|p| self.rect(p)))
                else {
                    return free;
                };
                let span = |lo: f64, hi: f64| if lo <= hi { (lo, hi) } else { (0.0, 0.0) };
                let (x0, x1) = span(p.x0 - r.x0, p.x1 - r.x1);
                let (y0, y1) = span(p.y0 - r.y0, p.y1 - r.y1);
                (Vec2::new(x0, y0), Vec2::new(x1, y1))
            }
            FollowRange::Points => {
                let pts = points(f);
                let fold = |g: fn(f64, f64) -> f64, x: bool| {
                    pts.iter()
                        .map(|(_, v)| if x { v.x } else { v.y })
                        .fold(0.0, g)
                };
                (
                    Vec2::new(fold(f64::min, true), fold(f64::min, false)),
                    Vec2::new(fold(f64::max, true), fold(f64::max, false)),
                )
            }
        }
    }

    /// Move `layer` to follow a pointer `d` from where the press went down,
    /// from offset `start`.
    pub(crate) fn follow_to(&mut self, layer: NodeId, start: Vec2, d: Vec2) {
        let Some(f) = self.follow_of(layer) else {
            return;
        };
        let d = held_to(f.axis, d);
        let (lo, hi) = self.range(layer, &f);
        let give = |v: f64, lo: f64, hi: f64| {
            if v < lo {
                lo + (v - lo) * GIVE
            } else if v > hi {
                hi + (v - hi) * GIVE
            } else {
                v
            }
        };
        let to = start + d;
        let to = Vec2::new(give(to.x, lo.x, hi.x), give(to.y, lo.y, hi.y));
        self.anims.moves.retain(|a| a.layer != layer);
        self.moved.insert(layer, to);
    }

    /// Let `layer` go after a drag of `d` over `ms`: it stays (held back into
    /// its range), goes back, or settles on a point, then drops.
    pub(crate) fn follow_release(&mut self, layer: NodeId, d: Vec2, ms: f64) {
        let Some(f) = self.follow_of(layer) else {
            return;
        };
        let at = self.moved(layer);
        let (lo, hi) = self.range(layer, &f);
        let held = Vec2::new(at.x.clamp(lo.x, hi.x), at.y.clamp(lo.y, hi.y));
        let (to, drop) = match f.release {
            Release::Stays => (held, DropAt::Anywhere),
            Release::Returns => (Vec2::ZERO, DropAt::Anywhere),
            Release::Snaps => {
                let ahead = match ms <= QUICK_MS {
                    true => at + held_to(f.axis, d) / ms.max(1.0) * THROW_MS,
                    false => at,
                };
                points(&f)
                    .into_iter()
                    .min_by(|a, b| (a.1 - ahead).hypot().total_cmp(&(b.1 - ahead).hypot()))
                    .map(|(at, v)| (v, at))
                    .unwrap_or((Vec2::ZERO, DropAt::Drawn))
            }
        };
        self.anims.moves.retain(|a| a.layer != layer);
        if to == at {
            self.moved.insert(layer, to);
            self.dropped(layer, drop);
            return;
        }
        self.anims.moves.push(MoveAnim {
            layer,
            from: at,
            to,
            start: self.now,
            timing: f.settle,
            at: drop,
        });
    }

    /// `layer` has come to rest at `at`: its *Drop* reactions for anywhere, and
    /// for that point, run.
    pub(crate) fn dropped(&mut self, layer: NodeId, at: DropAt) {
        if self.disabled(layer) {
            return;
        }
        let picked: Vec<_> = self
            .doc
            .get(layer)
            .map(|n| {
                n.reactions()
                    .iter()
                    .filter(|r| match r.value.trigger {
                        Trigger::Drop { at: want } => want == DropAt::Anywhere || want == at,
                        _ => false,
                    })
                    .map(|r| r.id)
                    .collect()
            })
            .unwrap_or_default();
        for r in picked {
            self.run_reaction(layer, r, None);
        }
    }

    /// Run the drops of the settles that finished in the last advance.
    pub(crate) fn fire_drops(&mut self) {
        for (layer, at) in std::mem::take(&mut self.drops) {
            self.dropped(layer, at);
        }
    }
}

/// Every point a following layer can rest at, where it is drawn first.
fn points(f: &Follow) -> Vec<(DropAt, Vec2)> {
    std::iter::once((DropAt::Drawn, Vec2::ZERO))
        .chain(
            f.points
                .iter()
                .map(|p| (DropAt::Point(p.id), Vec2::new(p.value.x, p.value.y))),
        )
        .collect()
}

/// `d` held to `axis`; *None* reads as both.
fn held_to(axis: ScrollAxis, d: Vec2) -> Vec2 {
    match axis {
        ScrollAxis::Horizontal => Vec2::new(d.x, 0.0),
        ScrollAxis::Vertical => Vec2::new(0.0, d.y),
        ScrollAxis::None | ScrollAxis::Both => d,
    }
}
