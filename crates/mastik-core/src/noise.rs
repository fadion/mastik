//! Noise: the parameters of a noise fill and of a grain effect, and the one
//! generator both draw with.
//!
//! The generator is SVG's `feTurbulence`, ported from the specification's own
//! reference code: the same lattice, gradients, seeding and octave sum, so the
//! SVG writer's `<feTurbulence>` is the reference both backends are held to. The
//! CPU backend and the app evaluate it through [`Lattice`]; `fx.wgsl` repeats
//! [`Lattice::noise2`] and [`Lattice::rgba`] line for line over the table
//! [`Lattice::table`] uploads.
//!
//! Noise lives in the layer's own units: one lattice cell is [`Noise::scale`]
//! document units, so zooming magnifies it and a 2× export draws the same
//! picture finer. It is seeded and stored, never random per draw.

use serde::{Deserialize, Serialize};

/// The smallest and largest cell a noise fill may have, in document units.
pub const NOISE_SCALE_RANGE: std::ops::RangeInclusive<f64> = 0.5..=2_000.0;

/// How many octaves a noise fill's [`Noise::detail`] may sum: 1 to 8. Past 8
/// each octave adds less than a level of an 8-bit pixel.
pub const MAX_DETAIL: u32 = 8;

/// How far [`Noise::stretch`] may pull a cell: ±400%, five times as wide or tall.
pub const MAX_STRETCH: f64 = 4.0;

/// The smallest and largest grain, in document units.
pub const GRAIN_SIZE_RANGE: std::ops::RangeInclusive<f64> = 0.5..=8.0;

/// The base frequency of a grain of size 1, in cycles per document unit. Not 1:
/// the noise is zero on every lattice point, and at 1 every pixel corner of an
/// unscaled layer on whole units is one, so the grain would vanish.
pub const GRAIN_FREQUENCY: f64 = 0.85;

/// How many octaves a grain sums: a second octave breaks up the lattice's
/// regularity, and finer ones fall below a pixel at any size the field allows.
pub const GRAIN_OCTAVES: u32 = 2;

/// Which of `feTurbulence`'s two sums a noise draws.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum NoiseKind {
    /// `type="fractalNoise"`: the signed sum, soft and cloudy. *Clouds* in the UI.
    #[default]
    FractalNoise,
    /// `type="turbulence"`: the sum of magnitudes, with sharp creases. *Veins* in
    /// the UI.
    Turbulence,
}

impl NoiseKind {
    /// Both, in the order the picker's segmented control shows them.
    pub const ALL: [NoiseKind; 2] = [NoiseKind::FractalNoise, NoiseKind::Turbulence];

    /// The word the picker shows, named for what each looks like.
    pub fn label(self) -> &'static str {
        match self {
            Self::FractalNoise => "Clouds",
            Self::Turbulence => "Veins",
        }
    }

    /// `feTurbulence`'s `type` attribute.
    pub fn svg_type(self) -> &'static str {
        match self {
            Self::FractalNoise => "fractalNoise",
            Self::Turbulence => "turbulence",
        }
    }
}

/// A noise fill's generator settings. The two colours it maps the noise onto
/// are the gradient's first and last stops (`crate::GradientBrush::noise`).
///
/// Read through [`NoiseWire`], which holds every number to its range, so a file,
/// a paste or an MCP write never holds a value the generator cannot draw.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
#[serde(from = "NoiseWire")]
pub struct Noise {
    pub kind: NoiseKind,
    /// The size of one cell in document units, `1 / baseFrequency`; held to
    /// [`NOISE_SCALE_RANGE`].
    pub scale: f64,
    /// Octaves, `1..=`[`MAX_DETAIL`].
    pub detail: u32,
    /// `feTurbulence`'s seed. Any whole number; the generator folds it into
    /// its own range as the specification does.
    pub seed: i32,
    /// How far the cells are pulled wide (positive) or tall (negative), a
    /// fraction: 1.0 makes a cell twice as wide. Held to ±[`MAX_STRETCH`].
    #[serde(default, skip_serializing_if = "is_zero")]
    pub stretch: f64,
}

fn is_zero(v: &f64) -> bool {
    *v == 0.0
}

/// A [`Noise`] as read, before its numbers are held to their ranges.
#[derive(Deserialize)]
struct NoiseWire {
    #[serde(default)]
    kind: NoiseKind,
    scale: f64,
    detail: u32,
    #[serde(default)]
    seed: i32,
    #[serde(default)]
    stretch: f64,
}

impl From<NoiseWire> for Noise {
    fn from(w: NoiseWire) -> Self {
        Noise {
            kind: w.kind,
            scale: w.scale,
            detail: w.detail,
            seed: w.seed,
            stretch: w.stretch,
        }
        .clamped()
    }
}

impl Default for Noise {
    /// The noise a freshly picked Noise tab starts on: soft clouds of 28-unit
    /// cells over four octaves, coarse enough to read as noise at 100%.
    fn default() -> Self {
        Self {
            kind: NoiseKind::FractalNoise,
            scale: 28.0,
            detail: 4,
            seed: 1,
            stretch: 0.0,
        }
    }
}

impl Noise {
    /// This noise with every number held to its range; `NaN` takes the default.
    pub fn clamped(self) -> Self {
        let d = Self::default();
        let held = |v: f64, r: std::ops::RangeInclusive<f64>, or: f64| {
            if v.is_nan() {
                or
            } else {
                v.clamp(*r.start(), *r.end())
            }
        };
        Self {
            kind: self.kind,
            scale: held(self.scale, NOISE_SCALE_RANGE, d.scale),
            detail: self.detail.clamp(1, MAX_DETAIL),
            seed: self.seed,
            stretch: held(self.stretch, -MAX_STRETCH..=MAX_STRETCH, 0.0),
        }
    }

    /// `feTurbulence`'s two base frequencies, in cycles per document unit: one
    /// over the cell's width and height after the stretch.
    pub fn base_frequency(&self) -> (f64, f64) {
        let c = self.clamped();
        let (w, h) = if c.stretch >= 0.0 {
            (c.scale * (1.0 + c.stretch), c.scale)
        } else {
            (c.scale, c.scale * (1.0 - c.stretch))
        };
        (1.0 / w, 1.0 / h)
    }

    /// The octaves to sum, held to range.
    pub fn octaves(&self) -> u32 {
        self.detail.clamp(1, MAX_DETAIL)
    }

    /// Whether this draws `feTurbulence`'s fractal sum.
    pub fn fractal(&self) -> bool {
        self.kind == NoiseKind::FractalNoise
    }
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

    /// [`Self::rgba`]'s first channel alone: a noise fill and a grey grain take
    /// their value from red.
    pub fn red(&self, x: f32, y: f32, octaves: u32, fractal: bool) -> u8 {
        let level = |c| self.level(c, x, y, octaves, fractal);
        round_trip(level(0), level(3))
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

/// A grain over a layer: noise blended with the layer's own pixels, inside its
/// alpha.
///
/// Read through [`GrainWire`], which holds its numbers to range.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
#[serde(from = "GrainWire")]
pub struct Grain {
    /// How strongly the grain shows, `0..=1`.
    pub amount: f64,
    /// The size of one grain in document units, held to [`GRAIN_SIZE_RANGE`].
    pub size: f64,
    pub seed: i32,
    /// How the grain mixes with the layer. Never Pass through.
    #[serde(default = "overlay", skip_serializing_if = "is_overlay")]
    pub blend: crate::BlendMode,
    /// Whether the grain takes three channels of colour rather than one grey.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub colour: bool,
}

fn overlay() -> crate::BlendMode {
    crate::BlendMode::Overlay
}

fn is_overlay(b: &crate::BlendMode) -> bool {
    *b == crate::BlendMode::Overlay
}

/// A [`Grain`] as read.
#[derive(Deserialize)]
struct GrainWire {
    amount: f64,
    size: f64,
    #[serde(default)]
    seed: i32,
    #[serde(default = "overlay")]
    blend: crate::BlendMode,
    #[serde(default)]
    colour: bool,
}

impl From<GrainWire> for Grain {
    fn from(w: GrainWire) -> Self {
        Grain {
            amount: w.amount,
            size: w.size,
            seed: w.seed,
            blend: w.blend,
            colour: w.colour,
        }
        .clamped()
    }
}

/// The amount a freshly added grain starts on, 18%: visible on a flat fill
/// without reading as damage.
pub const DEFAULT_GRAIN_AMOUNT: f64 = 0.18;

impl Default for Grain {
    fn default() -> Self {
        Self {
            amount: DEFAULT_GRAIN_AMOUNT,
            size: 1.0,
            seed: 1,
            blend: crate::BlendMode::Overlay,
            colour: false,
        }
    }
}

impl Grain {
    /// This grain with its numbers held to range and Pass through read as
    /// Overlay; `NaN` takes the default.
    pub fn clamped(self) -> Self {
        let d = Self::default();
        Self {
            amount: if self.amount.is_nan() {
                d.amount
            } else {
                self.amount.clamp(0.0, 1.0)
            },
            size: if self.size.is_nan() {
                d.size
            } else {
                self.size
                    .clamp(*GRAIN_SIZE_RANGE.start(), *GRAIN_SIZE_RANGE.end())
            },
            seed: self.seed,
            blend: if self.blend == crate::BlendMode::PassThrough {
                d.blend
            } else {
                self.blend
            },
            colour: self.colour,
        }
    }

    /// `feTurbulence`'s base frequency for this grain, the same on both axes:
    /// [`GRAIN_FREQUENCY`] over the size.
    pub fn base_frequency(&self) -> (f64, f64) {
        let f = GRAIN_FREQUENCY / self.clamped().size;
        (f, f)
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

    /// A noise's numbers are held to range when read, so a file's wild values
    /// draw a noise the generator can render.
    #[test]
    fn a_read_noise_is_held_to_range() {
        let n: Noise = serde_json::from_str(
            r#"{"kind":"turbulence","scale":1e9,"detail":40,"seed":3,"stretch":-9}"#,
        )
        .expect("reads");
        assert_eq!(n.scale, *NOISE_SCALE_RANGE.end());
        assert_eq!(n.detail, MAX_DETAIL);
        assert_eq!(n.stretch, -MAX_STRETCH);
        let g: Grain =
            serde_json::from_str(r#"{"amount":3,"size":0,"blend":"PassThrough"}"#).expect("reads");
        assert_eq!(
            (g.amount, g.size, g.blend),
            (1.0, 0.5, crate::BlendMode::Overlay)
        );
    }

    /// Stretch pulls cells wide when positive and tall when negative.
    #[test]
    fn stretch_pulls_one_axis() {
        let wide = Noise {
            stretch: 1.0,
            ..Noise::default()
        }
        .base_frequency();
        assert_eq!(wide, (1.0 / 56.0, 1.0 / 28.0));
        let tall = Noise {
            stretch: -1.0,
            ..Noise::default()
        }
        .base_frequency();
        assert_eq!(tall, (1.0 / 28.0, 1.0 / 56.0));
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
