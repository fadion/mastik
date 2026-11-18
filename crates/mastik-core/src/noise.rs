//! Noise: the parameters of the noise effect and the generator it draws with.
//!
//! The generator is SVG's `feTurbulence`, ported from the specification's own
//! reference code: the same lattice, gradients, seeding and octave sum, so the
//! SVG writer's `<feTurbulence>` is the reference both backends are held to. The
//! CPU backend evaluates it through [`Lattice`] and [`Noise::shade`]; `fx.wgsl`
//! repeats [`Lattice::noise2`], [`Lattice::rgba`] and the shading line for line
//! over the table [`Lattice::table`] uploads.
//!
//! Noise lives in the layer's own units, so zooming magnifies it and a 2× export
//! draws the same picture finer. It is seeded and stored, never random per draw.

use peniko::Color;
use serde::{Deserialize, Serialize};

/// The smallest and largest noise grain, in document units, on either axis.
pub const NOISE_SIZE_RANGE: std::ops::RangeInclusive<f64> = 0.5..=50.0;

/// The base frequency of a grain of size 1, in cycles per document unit. Not 1:
/// the noise is zero on every lattice point, and at 1 every pixel corner of an
/// unscaled layer on whole units is one, so the noise would vanish.
pub const NOISE_FREQUENCY: f64 = 0.85;

/// How many octaves noise sums: a second octave breaks up the lattice's
/// regularity; more would only blur the grain into clouds.
pub const NOISE_OCTAVES: u32 = 2;

/// How far the noise's value is pulled from mid-grey before it picks between
/// the two colours: `feTurbulence`'s fractal sum sits mostly within a quarter of
/// its range, so unstretched the colours would meet as a faint mix of both.
/// `fx.wgsl`'s `NZ_CONTRAST` repeats it.
pub const NOISE_CONTRAST: f32 = 3.0;

/// The steepness of the step the SVG writer cuts density with, an
/// `feFuncA type="linear"`: steep enough that the levels either side of the
/// floor land past 0 and 1 and clamp there.
pub const DENSITY_SLOPE: f64 = 1000.0;

/// Noise over a layer, inside its alpha: two colours picked by the noise's
/// value, or three channels of it in colour (`multi`), blended with the layer.
///
/// Read through [`NoiseWire`], which fills what is left out with the default and
/// holds every number to its range, so a file, a paste or an MCP write never
/// holds a value the generator cannot draw.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
#[serde(from = "NoiseWire")]
pub struct Noise {
    /// One grain's width and height in document units, each held to
    /// [`NOISE_SIZE_RANGE`].
    pub size: [f64; 2],
    /// The share of the layer the noise covers, `0..=1`; at 1 it covers all of it.
    #[serde(skip_serializing_if = "is_one")]
    pub density: f64,
    /// `feTurbulence`'s seed. Any whole number; the generator folds it into its
    /// own range as the specification does.
    pub seed: i32,
    /// How the noise mixes with the layer. Never Pass through.
    #[serde(skip_serializing_if = "is_normal")]
    pub blend: crate::BlendMode,
    /// The colour at the noise's low end and at its high end. A transparent
    /// second colour is a one-colour noise.
    pub colors: [Color; 2],
    /// Set, the noise takes three channels of colour of its own at
    /// [`Self::opacity`], and `colors` wait unused.
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    pub multi: bool,
    /// How strongly a multicolour noise shows, `0..=1`.
    pub opacity: f64,
}

fn is_one(v: &f64) -> bool {
    *v == 1.0
}

fn is_normal(b: &crate::BlendMode) -> bool {
    *b == crate::BlendMode::Normal
}

/// A [`Noise`] as read: any field may be left out.
#[derive(Deserialize)]
#[serde(default)]
struct NoiseWire {
    size: [f64; 2],
    density: f64,
    seed: i32,
    blend: crate::BlendMode,
    colors: [Color; 2],
    multi: bool,
    opacity: f64,
}

impl Default for NoiseWire {
    fn default() -> Self {
        let d = Noise::default();
        Self {
            size: d.size,
            density: d.density,
            seed: d.seed,
            blend: d.blend,
            colors: d.colors,
            multi: d.multi,
            opacity: d.opacity,
        }
    }
}

impl From<NoiseWire> for Noise {
    fn from(w: NoiseWire) -> Self {
        Noise {
            size: w.size,
            density: w.density,
            seed: w.seed,
            blend: w.blend,
            colors: w.colors,
            multi: w.multi,
            opacity: w.opacity,
        }
        .clamped()
    }
}

impl Default for Noise {
    /// A fresh noise: one-unit black grain at 40% fading to clear, visible on
    /// any fill without reading as damage.
    fn default() -> Self {
        Self {
            size: [1.0, 1.0],
            density: 1.0,
            seed: 1,
            blend: crate::BlendMode::Normal,
            colors: [
                Color::from_rgba8(0, 0, 0, 102),
                Color::from_rgba8(0, 0, 0, 0),
            ],
            multi: false,
            opacity: 0.3,
        }
    }
}

impl Noise {
    /// This noise with its numbers held to range and Pass through read as
    /// Normal; `NaN` takes the default.
    pub fn clamped(self) -> Self {
        let d = Self::default();
        let held = |v: f64, r: std::ops::RangeInclusive<f64>, or: f64| match v.is_nan() {
            true => or,
            false => v.clamp(*r.start(), *r.end()),
        };
        let colour = |c: Color, or: Color| match c.components.iter().all(|v| v.is_finite()) {
            true => {
                let mut c = c;
                c.components = c.components.map(|v| v.clamp(0.0, 1.0));
                c
            }
            false => or,
        };
        Self {
            size: [
                held(self.size[0], NOISE_SIZE_RANGE, d.size[0]),
                held(self.size[1], NOISE_SIZE_RANGE, d.size[1]),
            ],
            density: held(self.density, 0.0..=1.0, d.density),
            seed: self.seed,
            blend: match self.blend {
                crate::BlendMode::PassThrough => d.blend,
                b => b,
            },
            colors: [
                colour(self.colors[0], d.colors[0]),
                colour(self.colors[1], d.colors[1]),
            ],
            multi: self.multi,
            opacity: held(self.opacity, 0.0..=1.0, d.opacity),
        }
    }

    /// Whether this noise changes any pixel: some density, and a colour or a
    /// multicolour opacity that shows.
    pub fn has_ink(&self) -> bool {
        self.density > 0.0
            && match self.multi {
                true => self.opacity > 0.0,
                false => self.colors.iter().any(|c| c.components[3] > 0.0),
            }
    }

    /// `feTurbulence`'s base frequencies, in cycles per document unit:
    /// [`NOISE_FREQUENCY`] over each axis's size.
    pub fn base_frequency(&self) -> (f64, f64) {
        let [w, h] = self.clamped().size;
        (NOISE_FREQUENCY / w, NOISE_FREQUENCY / h)
    }

    /// The lowest alpha level of the noise a pixel must reach to be covered, so
    /// that [`Self::density`]'s share of pixels is; `None` at full density, where
    /// no pixel is cut. The noise's alpha channel is not uniform, so the floor
    /// is read off its measured distribution ([`alpha_share`]).
    pub fn density_floor(&self) -> Option<u8> {
        let d = self.clamped().density;
        if d >= 1.0 {
            return None;
        }
        let share = alpha_share();
        (0..=255u8).rev().find(|&l| share[usize::from(l)] >= d)
    }

    /// One pixel of noise as it is drawn over the layer, from the four levels
    /// [`Lattice::rgba`] gives: the straight colour laid on, and how much of it
    /// shows, both held at 8 bits as an SVG renderer holds a matrix's output.
    /// `None` where [`Self::density_floor`] leaves the pixel uncovered.
    ///
    /// A one-colour noise reads only red and alpha, so the caller may pass
    /// [`Lattice::red_alpha`]'s two with green and blue as zero.
    pub fn shade(&self, rgba: [u8; 4], floor: Option<u8>) -> Option<([f32; 3], f32)> {
        if floor.is_some_and(|f| rgba[3] < f) {
            return None;
        }
        let q = |v: f32| (v.clamp(0.0, 1.0) * 255.0 + 0.5).floor() / 255.0;
        let k = NOISE_CONTRAST;
        let u = |c: u8| q(k * f32::from(c) / 255.0 + 0.5 - 0.5 * k);
        if self.multi {
            return Some(([u(rgba[0]), u(rgba[1]), u(rgba[2])], q(self.opacity as f32)));
        }
        let t = u(rgba[0]);
        let [from, to] = self.colors.map(|c| c.components);
        let mix = |i: usize| q(from[i] + (to[i] - from[i]) * t);
        Some(([mix(0), mix(1), mix(2)], mix(3)))
    }
}

/// `share[l]`, the share of pixels whose noise alpha level is `l` or above,
/// measured once over a field of the lattice: the distribution is the same for
/// every seed and every size, since both only move and scale the field.
fn alpha_share() -> &'static [f64; 256] {
    static SHARE: std::sync::OnceLock<[f64; 256]> = std::sync::OnceLock::new();
    SHARE.get_or_init(|| {
        let lattice = Lattice::new(1);
        let mut count = [0u64; 256];
        const SIDE: u32 = 256;
        for i in 0..SIDE {
            for j in 0..SIDE {
                // Steps off the lattice's grid, so no sample lands on a lattice
                // point, where the noise is zero.
                let (x, y) = (i as f32 * 0.618_034 + 0.25, j as f32 * 0.414_214 + 0.125);
                let a = lattice.level(3, x, y, NOISE_OCTAVES, true);
                count[usize::from(a)] += 1;
            }
        }
        let total = f64::from(SIDE * SIDE);
        let mut share = [0.0; 256];
        let mut above = 0u64;
        for l in (0..256).rev() {
            above += count[l];
            share[l] = above as f64 / total;
        }
        share
    })
}

/// The lattice size of the reference code, `BSize`.
const B_SIZE: usize = 0x100;
/// `BSize + BSize + 2`: the tables are doubled so `i + j` never wraps.
pub const B_LEN: usize = B_SIZE + B_SIZE + 2;
const RAND_M: i32 = 2_147_483_647;
const RAND_A: i32 = 16_807;
const RAND_Q: i32 = 127_773;
const RAND_R: i32 = 2_836;

/// How many `f32`s [`Lattice::table`] holds: the selector, then four channels of
/// two-component gradients.
pub const TABLE_LEN: usize = B_LEN + 4 * B_LEN * 2;

/// The reference code's Park–Miller step.
fn random(seed: i32) -> i32 {
    let r = RAND_A * (seed % RAND_Q) - RAND_R * (seed / RAND_Q);
    if r <= 0 { r + RAND_M } else { r }
}

/// One seed's lattice: `feTurbulence`'s `init`, its selector permutation and the
/// unit gradients of its four channels.
///
/// The gradients are normalised in `f64`, as the reference does, and stored as
/// `f32`, which is what both backends evaluate in.
#[derive(Clone, Debug)]
pub struct Lattice {
    selector: [u16; B_LEN],
    gradient: [[[f32; 2]; B_LEN]; 4],
}

impl Lattice {
    pub fn new(seed: i32) -> Self {
        let mut seed = seed;
        if seed <= 0 {
            seed = -(seed % (RAND_M - 1)) + 1;
        }
        if seed > RAND_M - 1 {
            seed = RAND_M - 1;
        }
        let mut selector = [0u16; B_LEN];
        let mut g = [[[0f64; 2]; B_LEN]; 4];
        for channel in &mut g {
            for (i, cell) in channel.iter_mut().enumerate().take(B_SIZE) {
                selector[i] = i as u16;
                for v in cell.iter_mut() {
                    seed = random(seed);
                    *v = f64::from((seed % (2 * B_SIZE as i32)) - B_SIZE as i32) / B_SIZE as f64;
                }
                let s = (cell[0] * cell[0] + cell[1] * cell[1]).sqrt();
                cell[0] /= s;
                cell[1] /= s;
            }
        }
        for i in (1..B_SIZE).rev() {
            let k = selector[i];
            seed = random(seed);
            let j = (seed % B_SIZE as i32) as usize;
            selector[i] = selector[j];
            selector[j] = k;
        }
        for i in 0..B_SIZE + 2 {
            selector[B_SIZE + i] = selector[i];
            for channel in &mut g {
                channel[B_SIZE + i] = channel[i];
            }
        }
        let mut gradient = [[[0f32; 2]; B_LEN]; 4];
        for (out, src) in gradient.iter_mut().zip(&g) {
            for (o, s) in out.iter_mut().zip(src) {
                *o = [s[0] as f32, s[1] as f32];
            }
        }
        Self { selector, gradient }
    }

    /// The tables as the GPU reads them: [`B_LEN`] selector entries, then the
    /// gradients channel by channel, cell by cell, `x` before `y`.
    pub fn table(&self) -> Vec<f32> {
        let mut t = Vec::with_capacity(TABLE_LEN);
        t.extend(self.selector.iter().map(|&s| f32::from(s)));
        for channel in &self.gradient {
            for cell in channel {
                t.extend_from_slice(cell);
            }
        }
        t
    }

    /// The reference code's `noise2` for `channel` at lattice point `(x, y)`.
    ///
    /// `floor` where the reference adds `PerlinN` and truncates: the two agree
    /// for every point the reference handles, and this one needs no offset.
    pub fn noise2(&self, channel: usize, x: f32, y: f32) -> f32 {
        let (fx, fy) = (x.floor(), y.floor());
        let (rx0, ry0) = (x - fx, y - fy);
        let (rx1, ry1) = (rx0 - 1.0, ry0 - 1.0);
        let bx0 = (fx as i32 & 0xff) as usize;
        let by0 = (fy as i32 & 0xff) as usize;
        let (bx1, by1) = ((bx0 + 1) & 0xff, (by0 + 1) & 0xff);
        let sel = |i: usize| self.selector[i] as usize;
        let (i, j) = (sel(bx0), sel(bx1));
        let (b00, b10, b01, b11) = (sel(i + by0), sel(j + by0), sel(i + by1), sel(j + by1));
        let s_curve = |t: f32| t * t * (3.0 - 2.0 * t);
        let lerp = |t: f32, a: f32, b: f32| a + t * (b - a);
        let (sx, sy) = (s_curve(rx0), s_curve(ry0));
        let g = &self.gradient[channel];
        let at = |b: usize, rx: f32, ry: f32| rx * g[b][0] + ry * g[b][1];
        let a = lerp(sx, at(b00, rx0, ry0), at(b10, rx1, ry0));
        let b = lerp(sx, at(b01, rx0, ry1), at(b11, rx1, ry1));
        lerp(sy, a, b)
    }

    /// The reference code's `turbulence` for `channel` at `(x, y)` in the first
    /// octave's lattice units, already scaled by the base frequency.
    pub fn turbulence(&self, channel: usize, x: f32, y: f32, octaves: u32, fractal: bool) -> f32 {
        let (mut x, mut y, mut ratio, mut sum) = (x, y, 1.0f32, 0.0f32);
        for _ in 0..octaves {
            let n = self.noise2(channel, x, y);
            let n = if fractal { n } else { n.abs() };
            sum += n / ratio;
            x *= 2.0;
            y *= 2.0;
            ratio *= 2.0;
        }
        sum
    }

    /// The pixel `feTurbulence` hands the next primitive at `(x, y)`, as an SVG
    /// renderer holds it: four 8-bit channels, then premultiplied by its own
    /// alpha and read back straight, as `feColorMatrix` reads its input.
    ///
    /// The round trip is kept because it is what every SVG renderer does to the
    /// noise: where the alpha channel is near zero the colour channels lose
    /// their precision, and a backend that skipped it would draw a different
    /// picture from the exported file.
    pub fn rgba(&self, x: f32, y: f32, octaves: u32, fractal: bool) -> [u8; 4] {
        let level = |c| self.level(c, x, y, octaves, fractal);
        let a = level(3);
        [
            round_trip(level(0), a),
            round_trip(level(1), a),
            round_trip(level(2), a),
            a,
        ]
    }

    /// [`Self::rgba`] with green and blue left zero: all a one-colour noise
    /// reads, at half the cost.
    pub fn red_alpha(&self, x: f32, y: f32, octaves: u32, fractal: bool) -> [u8; 4] {
        let level = |c| self.level(c, x, y, octaves, fractal);
        let a = level(3);
        [round_trip(level(0), a), 0, 0, a]
    }

    /// One channel of `feTurbulence`'s output as an 8-bit level, the reference's
    /// mapping of each sum onto `0..=255`.
    fn level(&self, channel: usize, x: f32, y: f32, octaves: u32, fractal: bool) -> u8 {
        let n = self.turbulence(channel, x, y, octaves, fractal);
        let v = if fractal {
            (n * 255.0 + 255.0) / 2.0
        } else {
            n * 255.0
        };
        (v.clamp(0.0, 255.0) + 0.5) as u8
    }
}

/// Level `c` premultiplied by alpha `a` and read back straight, each step
/// rounded to 8 bits as an SVG renderer's buffers round it.
fn round_trip(c: u8, a: u8) -> u8 {
    if a == 0 {
        return 0;
    }
    let af = f32::from(a) / 255.0;
    let pre = (f32::from(c) * af + 0.5) as u8;
    (f32::from(pre) / af + 0.5).min(255.0) as u8
}

/// Where a buffer's pixel lands in the first octave's lattice: `origin + px·dx
/// + py·dy`, the buffer→layer map scaled by the base frequencies.
///
/// The origin is reduced modulo the lattice's period (256 cells) in `f64` before
/// it becomes `f32`, so a layer far from the page's origin keeps the precision a
/// layer near it has. Doubling per octave keeps the reduction valid.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct LatticeMap {
    pub origin: [f32; 2],
    pub dx: [f32; 2],
    pub dy: [f32; 2],
}

impl LatticeMap {
    /// The map for a buffer whose pixel corner `(px, py)` lands at
    /// `to_local * (px, py)` in the layer's space, at `freq` cycles per unit.
    ///
    /// Pixel corners, not centres: an SVG renderer evaluates `feTurbulence` at
    /// each pixel's top-left corner.
    pub fn new(to_local: kurbo::Affine, freq: (f64, f64)) -> Self {
        let [a, b, c, d, e, f] = to_local.as_coeffs();
        let reduce = |v: f64| {
            let r = v.rem_euclid(B_SIZE as f64);
            if r.is_finite() { r as f32 } else { 0.0 }
        };
        Self {
            origin: [reduce(e * freq.0), reduce(f * freq.1)],
            dx: [(a * freq.0) as f32, (b * freq.1) as f32],
            dy: [(c * freq.0) as f32, (d * freq.1) as f32],
        }
    }

    /// The lattice point of pixel `(px, py)`, in the order `fx.wgsl` computes it.
    pub fn at(&self, px: u32, py: u32) -> (f32, f32) {
        let (x, y) = (px as f32, py as f32);
        (
            self.origin[0] + x * self.dx[0] + y * self.dy[0],
            self.origin[1] + x * self.dx[1] + y * self.dy[1],
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The lattice is the reference code's: the first gradient of seed 1 and
    /// the selector's head are what `feTurbulence`'s `init` computes, held here
    /// as numbers so a change to the seeding fails.
    #[test]
    fn the_lattice_is_the_reference_codes() {
        let l = Lattice::new(1);
        // seed 1: random(1) = 16807, 16807 % 512 - 256 = 167; random(16807) =
        // 282475249, % 512 - 256 = -15.
        let (x, y) = (167.0f64 / 256.0, -15.0f64 / 256.0);
        let s = (x * x + y * y).sqrt();
        assert_eq!(l.gradient[0][0], [(x / s) as f32, (y / s) as f32]);
        let t = l.table();
        assert_eq!(t.len(), TABLE_LEN);
        assert_eq!(t[B_LEN..B_LEN + 2], l.gradient[0][0]);
        // The doubled half repeats the first.
        assert_eq!(l.selector[B_SIZE], l.selector[0]);
    }

    /// Zero and negative seeds fold the way the reference folds them: 0 and 1
    /// are one lattice, and -5 is 6.
    #[test]
    fn seeds_fold_as_the_reference_folds_them() {
        let at = |s: i32| Lattice::new(s).noise2(0, 3.3, 7.7);
        assert_eq!(at(0), at(1));
        assert_eq!(at(-5), at(6));
        assert_ne!(at(1), at(2), "two seeds are two lattices");
    }

    /// The noise is zero on every lattice point and continuous across a cell
    /// edge, which is what makes an `f32` port agree with the `f64` reference.
    #[test]
    fn noise_vanishes_on_the_lattice_and_is_continuous() {
        let l = Lattice::new(7);
        assert_eq!(l.noise2(0, 5.0, 9.0), 0.0);
        let below = l.noise2(1, 4.999_99, 2.5);
        let above = l.noise2(1, 5.000_01, 2.5);
        assert!((below - above).abs() < 1e-3, "{below} against {above}");
    }

    /// A noise's numbers are held to range when read, and what is left out takes
    /// the default, so a file's wild values or an agent's bare `{}` draw a noise
    /// the generator can render.
    #[test]
    fn a_read_noise_is_held_to_range() {
        let n: Noise = serde_json::from_str(
            r#"{"size":[0,900],"density":3,"opacity":-1,"blend":"PassThrough"}"#,
        )
        .expect("reads");
        assert_eq!(n.size, [0.5, 50.0]);
        assert_eq!((n.density, n.opacity), (1.0, 0.0));
        assert_eq!(n.blend, crate::BlendMode::Normal);
        let bare: Noise = serde_json::from_str("{}").expect("reads");
        assert_eq!(bare, Noise::default());
        let json = serde_json::to_string(&Noise::default()).expect("writes");
        assert!(
            !json.contains("density") && !json.contains("blend") && !json.contains("multi"),
            "the defaults that are the common case stay out of the file: {json}"
        );
    }

    /// The density floor covers the share of pixels it names: measured over a
    /// field the floor was not read from, a 30% density covers about 30%.
    ///
    /// flip: `density_floor` searching upward (`(0..=255)` without `.rev()`) →
    /// fails at the 30% share, which comes out 100%.
    #[test]
    fn density_covers_its_share_of_pixels() {
        let n = Noise {
            density: 0.3,
            ..Noise::default()
        };
        let floor = n.density_floor();
        assert!(floor.is_some(), "a partial density cuts");
        assert_eq!(
            Noise::default().density_floor(),
            None,
            "full density never cuts"
        );
        let lattice = Lattice::new(42);
        let (mut covered, mut all) = (0u32, 0u32);
        for i in 0..200 {
            for j in 0..200 {
                let (x, y) = (i as f32 * 0.731 + 0.3, j as f32 * 0.537 + 0.2);
                let px = lattice.red_alpha(x, y, NOISE_OCTAVES, true);
                all += 1;
                covered += u32::from(n.shade(px, floor).is_some());
            }
        }
        let share = f64::from(covered) / f64::from(all);
        assert!((share - 0.3).abs() < 0.03, "30% density covered {share:.3}");
    }

    /// A one-colour noise lays the first colour at the low end and the second at
    /// the high end, stretched by the contrast so both ends are reached; a
    /// multicolour one takes its three channels at its opacity.
    #[test]
    fn shade_picks_between_the_two_colours() {
        let n = Noise {
            colors: [
                Color::from_rgba8(255, 0, 0, 255),
                Color::from_rgba8(0, 0, 255, 0),
            ],
            ..Noise::default()
        };
        assert_eq!(n.shade([0, 0, 0, 255], None), Some(([1.0, 0.0, 0.0], 1.0)));
        assert_eq!(
            n.shade([255, 0, 0, 255], None),
            Some(([0.0, 0.0, 1.0], 0.0))
        );
        let ([r, _, b], m) = n.shade([128, 0, 0, 255], None).expect("covered");
        assert!((r - 0.5).abs() < 0.02 && (b - 0.5).abs() < 0.02 && (m - 0.5).abs() < 0.02);
        let multi = Noise {
            multi: true,
            opacity: 0.5,
            ..Noise::default()
        };
        assert_eq!(
            multi
                .shade([0, 255, 128, 255], None)
                .map(|(c, m)| (c[0], c[1], m)),
            Some((0.0, 1.0, 128.0 / 255.0))
        );
        assert_eq!(
            n.shade([0, 0, 0, 10], Some(11)),
            None,
            "below the floor, uncovered"
        );
    }

    /// The map reduces a far origin into the lattice's period without moving
    /// the noise: a layer 256 cells along draws what one at the origin draws.
    #[test]
    fn a_far_origin_draws_what_the_origin_draws() {
        let near = LatticeMap::new(kurbo::Affine::translate((3.0, 0.0)), (0.5, 0.5));
        let far = LatticeMap::new(kurbo::Affine::translate((3.0 + 512.0, 0.0)), (0.5, 0.5));
        assert_eq!(near, far);
    }
}
