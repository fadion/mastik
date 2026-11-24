//! How a transition moves through time: easing curves, cubic Béziers and springs.
//!
//! Every answer is a closed function of the time since the motion started, never
//! a step integrated frame by frame, so a take replayed at another frame rate
//! reaches the same state at the same time.

use mastik_core::prototype::{Easing, Timing};

/// A spring is settled once it stays within this of its target.
const SETTLED: f64 = 1e-3;
/// The longest a spring is let run, in seconds, however loosely it is damped.
const SPRING_CAP: f64 = 10.0;

/// Where a motion with `timing` is `ms` after it started: its progress from 0 to
/// 1 (a spring may overshoot), and whether it has finished.
pub fn progress(timing: &Timing, ms: f64) -> (f64, bool) {
    match *timing {
        Timing::Ease { curve, ms: length } => {
            if length == 0 || ms >= f64::from(length) {
                return (1.0, true);
            }
            (ease(curve, (ms / f64::from(length)).max(0.0)), false)
        }
        Timing::Spring {
            mass,
            stiffness,
            damping,
        } => {
            let s = Spring::new(mass, stiffness, damping);
            let t = ms.max(0.0) / 1000.0;
            if t >= s.settle {
                return (1.0, true);
            }
            (1.0 + s.offset(t), false)
        }
    }
}

/// How long a motion with `timing` runs, in milliseconds.
pub fn duration(timing: &Timing) -> f64 {
    match *timing {
        Timing::Ease { ms, .. } => f64::from(ms),
        Timing::Spring {
            mass,
            stiffness,
            damping,
        } => Spring::new(mass, stiffness, damping).settle * 1000.0,
    }
}

/// `curve` at `t` in 0..=1.
pub fn ease(curve: Easing, t: f64) -> f64 {
    let t = t.clamp(0.0, 1.0);
    match curve {
        Easing::Linear => t,
        Easing::Ease => cubic_bezier([0.25, 0.1, 0.25, 1.0], t),
        Easing::EaseIn => cubic_bezier([0.42, 0.0, 1.0, 1.0], t),
        Easing::EaseOut => cubic_bezier([0.0, 0.0, 0.58, 1.0], t),
        Easing::EaseInOut => cubic_bezier([0.42, 0.0, 0.58, 1.0], t),
        Easing::Cubic(p) => cubic_bezier(p, t),
    }
}

/// CSS `cubic-bezier(x1, y1, x2, y2)` at `x`: the curve's parameter found for
/// `x` by Newton's method, falling back to bisection, then its `y`.
pub fn cubic_bezier([x1, y1, x2, y2]: [f64; 4], x: f64) -> f64 {
    if x <= 0.0 {
        return 0.0;
    }
    if x >= 1.0 {
        return 1.0;
    }
    let along = |p1: f64, p2: f64, t: f64| {
        let u = 1.0 - t;
        3.0 * u * u * t * p1 + 3.0 * u * t * t * p2 + t * t * t
    };
    let slope = |p1: f64, p2: f64, t: f64| {
        let u = 1.0 - t;
        3.0 * u * u * p1 + 6.0 * u * t * (p2 - p1) + 3.0 * t * t * (1.0 - p2)
    };
    let mut t = x;
    for _ in 0..8 {
        let dx = along(x1, x2, t) - x;
        if dx.abs() < 1e-7 {
            return along(y1, y2, t);
        }
        let d = slope(x1, x2, t);
        if d.abs() < 1e-6 {
            break;
        }
        t -= dx / d;
    }
    let (mut lo, mut hi) = (0.0, 1.0);
    t = x;
    for _ in 0..60 {
        let at = along(x1, x2, t);
        if (at - x).abs() < 1e-7 {
            break;
        }
        if at < x {
            lo = t;
        } else {
            hi = t;
        }
        t = (lo + hi) / 2.0;
    }
    along(y1, y2, t)
}

/// A damped spring pulling from 0 to 1, from rest.
#[derive(Clone, Copy, Debug)]
struct Spring {
    /// The undamped angular frequency, per second.
    w0: f64,
    /// The damping ratio: under 1 overshoots, 1 is critical, over 1 creeps.
    zeta: f64,
    /// When it settles, in seconds.
    settle: f64,
}

impl Spring {
    fn new(mass: f64, stiffness: f64, damping: f64) -> Self {
        let w0 = (stiffness / mass).sqrt();
        let zeta = damping / (2.0 * (stiffness * mass).sqrt());
        let mut s = Spring {
            w0,
            zeta,
            settle: SPRING_CAP,
        };
        // The last moment, sampled at 240 Hz, it is still away from the target.
        let step = 1.0 / 240.0;
        let samples = (SPRING_CAP / step) as usize;
        let away = |k: usize| s.offset(k as f64 * step).abs() >= SETTLED;
        let last = if zeta >= 1.0 - 1e-9 {
            // From rest a spring that does not overshoot only closes in, so
            // the samples still away come first: halve to the last of them.
            let (mut lo, mut hi) = (0, samples + 1);
            while lo < hi {
                let mid = (lo + hi) / 2;
                if away(mid) { lo = mid + 1 } else { hi = mid }
            }
            lo.checked_sub(1)
        } else {
            // Past where the envelope `a·e^(−ζω₀t)` is under the threshold no
            // sample is away, so the scan back starts there.
            let wd = w0 * (1.0 - zeta * zeta).sqrt();
            let a = (1.0 + (zeta * w0 / wd).powi(2)).sqrt();
            let quiet = (a / SETTLED).ln() / (zeta * w0);
            let from = match quiet.is_finite() && quiet >= 0.0 {
                true => ((quiet / step).ceil() as usize + 1).min(samples),
                false => samples,
            };
            (0..=from).rev().find(|k| away(*k))
        };
        s.settle = last.map_or(0.0, |k| ((k + 1) as f64 * step).min(SPRING_CAP));
        s
    }

    /// The distance from the target `t` seconds in; -1 at the start.
    fn offset(&self, t: f64) -> f64 {
        let (w0, z) = (self.w0, self.zeta);
        let x0 = -1.0;
        if (z - 1.0).abs() < 1e-9 {
            (-w0 * t).exp() * (x0 + w0 * x0 * t)
        } else if z < 1.0 {
            let wd = w0 * (1.0 - z * z).sqrt();
            (-z * w0 * t).exp() * (x0 * (wd * t).cos() + (z * w0 * x0) / wd * (wd * t).sin())
        } else {
            let root = (z * z - 1.0).sqrt();
            let (r1, r2) = (-w0 * (z - root), -w0 * (z + root));
            let c2 = -r1 * x0 / (r2 - r1);
            let c1 = x0 - c2;
            c1 * (r1 * t).exp() + c2 * (r2 * t).exp()
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The CSS presets hit their published midpoints, and a custom curve is read
    /// by `x`, not by its parameter.
    ///
    /// flip: `cubic_bezier` answering `along(y1, y2, x)` (the parameter taken as
    /// `x`) → fails at "ease-in at one half".
    #[test]
    fn the_css_curves_read_by_x() {
        // `ease-in` at x = 0.5 is about 0.3153 in every browser.
        let y = ease(Easing::EaseIn, 0.5);
        assert!((y - 0.3153).abs() < 1e-3, "ease-in at one half: {y}");
        assert_eq!(ease(Easing::Linear, 0.25), 0.25);
        assert!(
            (ease(Easing::EaseInOut, 0.5) - 0.5).abs() < 1e-6,
            "symmetric"
        );
        assert_eq!(ease(Easing::Ease, 1.0), 1.0);
        let y = cubic_bezier([0.0, 0.0, 1.0, 1.0], 0.3);
        assert!((y - 0.3).abs() < 1e-6, "a straight cubic is linear: {y}");
    }

    /// An underdamped spring overshoots and settles; an overdamped one never
    /// passes its target; both finish.
    ///
    /// flip: `Spring::offset`'s underdamped arm without its sine term → fails at
    /// "starts at rest".
    #[test]
    fn springs_overshoot_when_underdamped_and_settle() {
        let bouncy = Timing::Spring {
            mass: 1.0,
            stiffness: 200.0,
            damping: 8.0,
        };
        let (start, _) = progress(&bouncy, 0.0);
        let (soon, _) = progress(&bouncy, 1.0);
        assert!(start.abs() < 1e-9, "begins at 0");
        assert!(soon < 1e-3, "starts at rest: {soon}");
        let peak = (0..1000)
            .map(|ms| progress(&bouncy, f64::from(ms)).0)
            .fold(0.0, f64::max);
        assert!(peak > 1.05, "overshoots: {peak}");
        let length = duration(&bouncy);
        assert_eq!(progress(&bouncy, length + 1.0), (1.0, true), "settles");
        assert!(length < 10_000.0, "within the cap: {length}");

        let heavy = Timing::Spring {
            mass: 1.0,
            stiffness: 100.0,
            damping: 40.0,
        };
        let peak = (0..5000)
            .map(|ms| progress(&heavy, f64::from(ms)).0)
            .fold(0.0, f64::max);
        assert!(peak <= 1.0, "never passes: {peak}");
        assert!(progress(&heavy, duration(&heavy) + 1.0).1, "finishes");
    }

    /// An eased motion is done at its length and not before.
    ///
    /// flip: `progress` answering done at `ms >= length / 2` → fails at "half way".
    #[test]
    fn an_eased_motion_ends_at_its_length() {
        let t = Timing::Ease {
            curve: Easing::Linear,
            ms: 200,
        };
        assert_eq!(progress(&t, 100.0), (0.5, false), "half way");
        assert_eq!(progress(&t, 200.0), (1.0, true));
        let none = Timing::Ease {
            curve: Easing::Linear,
            ms: 0,
        };
        assert_eq!(progress(&none, 0.0), (1.0, true), "instant");
    }

    /// A spring's settle time is the last 240 Hz sample still away from its
    /// target, the same as scanning every sample back from the cap, for loose,
    /// bouncy, critical and heavy springs.
    ///
    /// flip: `Spring::new`'s envelope without its sine term's amplitude (`a`
    /// at 1) → fails at the near-critical `1 100 18` spring's settle.
    #[test]
    fn a_springs_settle_is_found_without_scanning_every_sample() {
        for (mass, stiffness, damping) in [
            (1.0, 200.0, 8.0),
            (1.0, 100.0, 2.0),
            (1.0, 300.0, 1.0),
            (1.0, 100.0, 18.0),
            (1.0, 100.0, 19.5),
            (1.0, 100.0, 19.9),
            (2.0, 100.0, 28.284_271_247_461_9),
            (1.0, 100.0, 20.0),
            (1.0, 100.0, 40.0),
            (1.0, 400.0, 0.0),
        ] {
            let s = Spring::new(mass, stiffness, damping);
            let step = 1.0 / 240.0;
            let samples = (SPRING_CAP / step) as usize;
            let every = (0..=samples)
                .rev()
                .find(|k| s.offset(*k as f64 * step).abs() >= SETTLED)
                .map_or(0.0, |k| ((k + 1) as f64 * step).min(SPRING_CAP));
            assert_eq!(s.settle, every, "{mass} {stiffness} {damping}");
        }
    }
}
