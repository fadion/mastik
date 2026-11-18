//! OKLCH, the polar form of OKLab: the space the colour picker's OKLCH format
//! edits in, *Generate scale* steps lightness in, and `contrast::nearest` moves a
//! colour through.
//!
//! The document stores sRGB, so every colour made here goes back to sRGB on
//! the way out. One outside the sRGB gamut is mapped there by lowering its
//! chroma at the same lightness and hue (CSS Color 4's gamut mapping), never by
//! clipping each channel, which shifts hue and lightness.

use peniko::Color;

/// A colour as OKLCH: lightness 0–1, chroma from 0 (about 0.37 at the edge of
/// sRGB), and hue in degrees, 0–360.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Oklch {
    pub l: f64,
    pub c: f64,
    pub h: f64,
}

/// The chroma below which a colour reads as grey, its hue meaningless.
pub const GREY_CHROMA: f64 = 1e-4;

/// The most chroma the picker's plane and field offer, a little past the most
/// any sRGB colour has (about 0.32), so the edge sits inside the plane.
pub const MAX_CHROMA: f64 = 0.37;

impl Oklch {
    /// `c`'s straight sRGB as OKLCH; alpha is not part of it. A grey answers hue 0.
    pub fn of(c: Color) -> Self {
        let [r, g, b, _] = c.components;
        let [l, a, b] = oklab([r, g, b].map(f64::from));
        let chroma = (a * a + b * b).sqrt();
        let h = if chroma < GREY_CHROMA {
            0.0
        } else {
            b.atan2(a).to_degrees().rem_euclid(360.0)
        };
        Self { l, c: chroma, h }
    }

    /// This colour as straight sRGB, unclamped: a channel outside 0–1 is out of
    /// gamut.
    pub fn to_srgb(self) -> [f64; 3] {
        let h = self.h.to_radians();
        from_oklab([self.l, self.c * h.cos(), self.c * h.sin()])
    }

    /// Whether sRGB holds this colour.
    pub fn in_gamut(self) -> bool {
        fits(self.to_srgb())
    }

    /// The sRGB colour nearest this one by CSS Color 4's gamut mapping: chroma
    /// lowered by bisection until clipping what is left moves the colour less
    /// than a just-noticeable difference (ΔE OK 0.02). Lightness at or past 0 or 1
    /// is black or white.
    pub fn to_srgb_mapped(self) -> [f64; 3] {
        const JND: f64 = 0.02;
        const EPSILON: f64 = 0.0001;
        if self.l >= 1.0 {
            return [1.0; 3];
        }
        if self.l <= 0.0 {
            return [0.0; 3];
        }
        if self.in_gamut() {
            return clip(self.to_srgb());
        }
        let mut current = self;
        let mut clipped = clip(current.to_srgb());
        if delta_eok(clipped, current) < JND {
            return clipped;
        }
        let (mut min, mut max, mut min_in_gamut) = (0.0, self.c, true);
        while max - min > EPSILON {
            current.c = (min + max) / 2.0;
            if min_in_gamut && current.in_gamut() {
                min = current.c;
                continue;
            }
            clipped = clip(current.to_srgb());
            let e = delta_eok(clipped, current);
            if e < JND {
                if JND - e < EPSILON {
                    return clipped;
                }
                min_in_gamut = false;
                min = current.c;
            } else {
                max = current.c;
            }
        }
        clipped
    }

    /// [`Self::to_srgb_mapped`] as a colour at `alpha`.
    pub fn to_color(self, alpha: f32) -> Color {
        let [r, g, b] = self.to_srgb_mapped();
        Color::new([r as f32, g as f32, b as f32, alpha])
    }
}

/// The most chroma sRGB holds at lightness `l` and hue `h` (degrees): where
/// the gamut's edge crosses that line.
pub fn max_chroma(l: f64, h: f64) -> f64 {
    if !(0.0..=1.0).contains(&l) {
        return 0.0;
    }
    let (mut lo, mut hi) = (0.0, MAX_CHROMA);
    if (Oklch { l, c: hi, h }).in_gamut() {
        return hi;
    }
    for _ in 0..24 {
        let mid = (lo + hi) / 2.0;
        if (Oklch { l, c: mid, h }).in_gamut() {
            lo = mid;
        } else {
            hi = mid;
        }
    }
    lo
}

/// The steps of a colour scale, lightest first.
pub const SCALE_STEPS: [u16; 11] = [50, 100, 200, 300, 400, 500, 600, 700, 800, 900, 950];

/// The fewest steps a scale may have; the most is every one of [`SCALE_STEPS`].
pub const SCALE_MIN_STEPS: usize = 3;

/// The indices of [`SCALE_STEPS`] a scale of `count` steps keeps, `count`
/// clamped to [`SCALE_MIN_STEPS`]..=11. Eleven keep all; ten drop 950; nine and
/// fewer spread evenly over the hundreds, 100 to 900, so five are 100, 300 …
/// 900 and three 100, 500, 900. Each name keeps the full ramp's lightness.
pub fn scale_subset(count: usize) -> Vec<usize> {
    let all = SCALE_STEPS.len();
    match count.clamp(SCALE_MIN_STEPS, all) {
        n if n >= all - 1 => (0..n).collect(),
        n => {
            // 100 is index 1 and 900 index 9: eight gaps, rounded half up.
            let span = all - 3;
            (0..n)
                .map(|k| 1 + (2 * k * span + (n - 1)) / (2 * (n - 1)))
                .collect()
        }
    }
}

/// The index of `keep` nearest the step `anchor`, by lightness: where the
/// source lands when the steps it was on are no longer kept.
pub fn nearest_kept(keep: &[usize], anchor: usize) -> usize {
    keep.iter()
        .copied()
        .min_by_key(|i| i.abs_diff(anchor))
        .unwrap_or(anchor)
}

/// The OKLCH lightness of a scale's lightest and darkest steps.
const SCALE_LIGHTNESS: (f64, f64) = (0.97, 0.24);

/// How much of the source's chroma a scale's two end steps keep; the steps
/// between ease toward the source's own.
const SCALE_END_CHROMA: f64 = 0.3;

/// The lightness step `i` of [`SCALE_STEPS`] sits at on the even line, 97% at
/// 50 to 24% at 950.
pub fn step_lightness(i: usize) -> f64 {
    let (light, dark) = SCALE_LIGHTNESS;
    light + (dark - light) * i as f64 / (SCALE_STEPS.len() - 1) as f64
}

/// The index of the step whose even lightness is nearest `c`'s: where a scale
/// from `c` puts it unless told otherwise.
pub fn nearest_step(c: Color) -> usize {
    let l = Oklch::of(c).l;
    (0..SCALE_STEPS.len())
        .min_by(|a, b| {
            (step_lightness(*a) - l)
                .abs()
                .total_cmp(&(step_lightness(*b) - l).abs())
        })
        .unwrap_or(5)
}

/// A colour scale from `base`, which lands on step `anchor` (an index of
/// [`SCALE_STEPS`]): every step at `base`'s hue and alpha, the lightness even
/// from 97% to 24%, the chroma `base`'s at the anchor and eased toward the
/// ends, capped at each step's sRGB edge so the hue holds.
///
/// With `exact`, the anchor's step is `base` itself and the lightness runs
/// evenly to it from each end instead.
pub fn scale(base: Color, anchor: usize, exact: bool) -> [(u16, Color); SCALE_STEPS.len()] {
    let from = Oklch::of(base);
    let last = SCALE_STEPS.len() - 1;
    let anchor = anchor.min(last);
    let (light, dark) = SCALE_LIGHTNESS;
    let alpha = base.components[3];
    std::array::from_fn(|i| {
        if exact && i == anchor {
            return (SCALE_STEPS[i], base);
        }
        let l = match (exact, i.cmp(&anchor)) {
            (false, _) | (true, std::cmp::Ordering::Equal) => step_lightness(i),
            (true, std::cmp::Ordering::Less) => light + (from.l - light) * i as f64 / anchor as f64,
            (true, std::cmp::Ordering::Greater) => {
                from.l + (dark - from.l) * (i - anchor) as f64 / (last - anchor) as f64
            }
        };
        // How far toward its end of the scale this step is, 0 at the anchor.
        let side = if i < anchor { anchor } else { last - anchor };
        let t = if side == 0 {
            0.0
        } else {
            i.abs_diff(anchor) as f64 / side as f64
        };
        let ease = 1.0 - (1.0 - SCALE_END_CHROMA) * t * t;
        let c = (from.c * ease).min(max_chroma(l, from.h));
        let [r, g, b] = clip(Oklch { l, c, h: from.h }.to_srgb());
        (
            SCALE_STEPS[i],
            Color::new([r as f32, g as f32, b as f32, alpha]),
        )
    })
}

/// [`scale`] cut to the `keep` indices of [`SCALE_STEPS`] ([`scale_subset`]):
/// each kept step the colour the full ramp gives it, so a shorter scale's 500
/// is the eleven-step scale's 500.
pub fn scale_kept(base: Color, anchor: usize, exact: bool, keep: &[usize]) -> Vec<(u16, Color)> {
    let full = scale(base, anchor, exact);
    keep.iter().filter_map(|i| full.get(*i).copied()).collect()
}

/// Whether straight sRGB `x` lies in the gamut, give or take float error.
fn fits(x: [f64; 3]) -> bool {
    x.iter().all(|v| (-1e-6..=1.0 + 1e-6).contains(v))
}

fn clip(x: [f64; 3]) -> [f64; 3] {
    x.map(|v| v.clamp(0.0, 1.0))
}

/// ΔE OK between sRGB `rgb` and `lch`: their distance in OKLab.
fn delta_eok(rgb: [f64; 3], lch: Oklch) -> f64 {
    let p = oklab(rgb);
    let h = lch.h.to_radians();
    let q = [lch.l, lch.c * h.cos(), lch.c * h.sin()];
    (0..3).map(|k| (p[k] - q[k]).powi(2)).sum::<f64>().sqrt()
}

/// Straight sRGB to OKLab.
pub(crate) fn oklab(c: [f64; 3]) -> [f64; 3] {
    let lin = |x: f64| {
        if x <= 0.04045 {
            x / 12.92
        } else {
            ((x + 0.055) / 1.055).powf(2.4)
        }
    };
    let [r, g, b] = c.map(lin);
    let l = (0.4122214708 * r + 0.5363325363 * g + 0.0514459929 * b).cbrt();
    let m = (0.2119034982 * r + 0.6806995451 * g + 0.1073969566 * b).cbrt();
    let s = (0.0883024619 * r + 0.2817188376 * g + 0.6299787005 * b).cbrt();
    [
        0.2104542553 * l + 0.7936177850 * m - 0.0040720468 * s,
        1.9779984951 * l - 2.4285922050 * m + 0.4505937099 * s,
        0.0259040371 * l + 0.7827717662 * m - 0.8086757660 * s,
    ]
}

/// OKLab to straight sRGB, unclamped: a channel outside 0–1 is out of gamut.
pub(crate) fn from_oklab([l, a, b]: [f64; 3]) -> [f64; 3] {
    let l_ = (l + 0.3963377774 * a + 0.2158037573 * b).powi(3);
    let m_ = (l - 0.1055613458 * a - 0.0638541728 * b).powi(3);
    let s_ = (l - 0.0894841775 * a - 1.2914855480 * b).powi(3);
    let lin = [
        4.0767416621 * l_ - 3.3077115913 * m_ + 0.2309699292 * s_,
        -1.2684380046 * l_ + 2.6097574011 * m_ - 0.3413193965 * s_,
        -0.0041960863 * l_ - 0.7034186147 * m_ + 1.7076147010 * s_,
    ];
    lin.map(|x| {
        if x <= 0.0031308 {
            x * 12.92
        } else {
            1.055 * x.powf(1.0 / 2.4) - 0.055
        }
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// OKLab round-trips a colour.
    #[test]
    fn oklab_round_trips() {
        let c = [0.64, 0.63, 0.61];
        let back = from_oklab(oklab(c));
        for k in 0..3 {
            assert!((back[k] - c[k]).abs() < 1e-6, "{back:?}");
        }
    }

    /// OKLCH reads CSS's reference values: `#ff0000` is `oklch(62.8% 0.2577 29.23)`.
    #[test]
    fn red_reads_as_css_writes_it() {
        let red = Oklch::of(Color::from_rgb8(255, 0, 0));
        assert!((red.l - 0.6280).abs() < 1e-3, "{red:?}");
        assert!((red.c - 0.2577).abs() < 1e-3, "{red:?}");
        assert!((red.h - 29.23).abs() < 0.1, "{red:?}");
    }

    /// An out-of-gamut colour is mapped by lowering chroma: the lightness and
    /// hue it lands at stay near the asked (the last clip may move them by up
    /// to a just-noticeable difference), where clipping each channel moves them
    /// far.
    ///
    /// flip: `to_srgb_mapped` returning `clip(self.to_srgb())` at once → fails at
    /// the lightness assertion.
    #[test]
    fn out_of_gamut_lowers_chroma_and_keeps_hue() {
        let asked = Oklch {
            l: 0.7,
            c: 0.35,
            h: 250.0,
        };
        assert!(!asked.in_gamut());
        let got = Oklch::of(asked.to_color(1.0));
        assert!(got.c < asked.c && got.c > 0.05, "{got:?}");
        assert!((got.l - asked.l).abs() < 0.02, "{got:?}");
        assert!((got.h - asked.h).abs() < 5.0, "{got:?}");
    }

    /// The gamut's edge is where sRGB stops: just inside fits, just outside does not.
    #[test]
    fn max_chroma_is_the_edge() {
        for h in [0.0, 90.0, 145.0, 264.0] {
            let c = max_chroma(0.6, h);
            assert!((Oklch { l: 0.6, c, h }).in_gamut());
            assert!(
                !(Oklch {
                    l: 0.6,
                    c: c + 1e-3,
                    h
                })
                .in_gamut(),
                "{h}"
            );
        }
    }

    /// A scale off the exact source runs 97% to 24% in even lightness steps at
    /// the base's hue, its chroma highest at the anchor.
    ///
    /// flip: `scale` mapping by `to_color` rather than capping at `max_chroma` →
    /// fails at the hue assertion.
    #[test]
    fn a_scale_is_even_in_lightness_and_keeps_the_hue() {
        let base = Color::from_rgb8(0x33, 0x66, 0xcc);
        let from = Oklch::of(base);
        let anchor = nearest_step(base);
        let steps = scale(base, anchor, false);
        assert_eq!(steps.map(|s| s.0), SCALE_STEPS);
        let lch = steps.map(|(_, c)| Oklch::of(c));
        assert!((lch[0].l - 0.97).abs() < 0.005 && (lch[10].l - 0.24).abs() < 0.005);
        for w in lch.windows(2) {
            assert!((w[0].l - w[1].l - 0.073).abs() < 0.005, "{lch:?}");
        }
        // The palest steps hold too little chroma for a hue to mean much.
        for c in lch.iter().filter(|c| c.c > 0.02) {
            assert!((c.h - from.h).abs() < 1.0, "{c:?} against {from:?}");
        }
        assert!(lch.iter().all(|c| c.c <= lch[anchor].c + 1e-3), "{lch:?}");
    }

    /// With `exact`, the anchor's step is the source itself and the lightness
    /// still falls step by step through it.
    #[test]
    fn an_exact_scale_passes_through_its_source() {
        let base = Color::from_rgb8(0x2f, 0x6b, 0xff);
        let steps = scale(base, 5, true);
        assert_eq!(steps[5].1, base);
        let ls = steps.map(|(_, c)| Oklch::of(c).l);
        assert!(ls.windows(2).all(|w| w[0] > w[1]), "{ls:?}");
    }

    /// Every count keeps its steps lightest first and symmetric about 500
    /// below ten, and a kept step is the full ramp's colour.
    ///
    /// flip: `scale_subset` rounding down rather than half up → fails at the
    /// four-step set.
    #[test]
    fn a_shorter_scale_keeps_a_symmetric_subset() {
        let names = |n: usize| {
            scale_subset(n)
                .iter()
                .map(|i| SCALE_STEPS[*i])
                .collect::<Vec<_>>()
        };
        assert_eq!(names(3), [100, 500, 900]);
        assert_eq!(names(4), [100, 400, 600, 900]);
        assert_eq!(names(5), [100, 300, 500, 700, 900]);
        assert_eq!(names(8), [100, 200, 300, 400, 600, 700, 800, 900]);
        assert_eq!(names(9), [100, 200, 300, 400, 500, 600, 700, 800, 900]);
        assert_eq!(names(10).last(), Some(&900));
        assert_eq!(names(11), SCALE_STEPS);
        assert_eq!(names(1), names(3), "clamped");
        for n in SCALE_MIN_STEPS..=9 {
            let s = names(n);
            assert!(s.windows(2).all(|w| w[0] < w[1]), "{s:?}");
            for (a, b) in s.iter().zip(s.iter().rev()) {
                assert_eq!(a + b, 1000, "{n}: {s:?}");
            }
        }
        let base = Color::from_rgb8(0x2f, 0x6b, 0xff);
        let full = scale(base, 5, true);
        let kept = scale_kept(base, 5, true, &scale_subset(3));
        assert_eq!(kept, vec![full[1], full[5], full[9]]);
        assert_eq!(nearest_kept(&scale_subset(3), 3), 1);
        assert_eq!(nearest_kept(&scale_subset(3), 4), 5);
    }
}
