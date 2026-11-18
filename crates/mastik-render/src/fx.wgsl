// The GPU effect passes — one entry point per step of `crate::effects::run`,
// which is the specification these are written against (§6.4, §15 D333).
//
// **Premultiplied throughout, straight at the two ends.** vello's GPU output is
// *straight* alpha and its image atlas takes straight alpha too, so `premul` and
// `unpremul` bracket the stack; everything between them works on premultiplied
// pixels exactly as `effects.rs` does. Blurring straight alpha mixes the colour
// of fully transparent pixels into their neighbours and haloes every soft edge,
// which is the whole reason the CPU reference is premultiplied.
//
// **Every store goes through `q`.** Writing an `rgba8unorm` storage texture
// quantises to 8 bits with rounding the spec leaves to the implementation, and
// the CPU reference rounds half-up. Doing the quantisation explicitly makes the
// value already representable, so the store cannot round it a second time and
// the two backends agree byte for byte rather than within a tolerance nobody
// measured.

struct Params {
    size: vec2<u32>,
    // A shadow's offset in device pixels, already rounded by the host.
    off: vec2<i32>,
    // Kernel half-width for a blur, box half-width for a spread.
    radius: i32,
    // bit 0: the pass runs horizontally. bit 1: inner (invert the silhouette).
    // bit 2: the spread grows rather than shrinks.
    flags: u32,
    // Where this layer sits inside the texture it is read from. Sibling effect
    // layers are rasterized into **one** packed texture and one vello pass (see
    // `fx_gpu::run`), so the ingest reads from an offset; every pass after it
    // works in the layer's own buffer and ignores this.
    src_origin: vec2<i32>,
    // A shadow's colour, straight, alpha carrying its opacity.
    tint: vec4<f32>,
    // `effects::filter_matrix`'s twenty coefficients, row-major, packed five to
    // a row of four.
    m: array<vec4<f32>, 5>,
    // The domain being *read*, where that is a different grid from the one being
    // written: the fine size for `coarsen`, the coarse size for `upsample`. Every
    // other pass reads and writes the same grid and ignores it.
    //
    // **Last rather than beside `size`, which is where it belongs by meaning.**
    // The struct's offsets are a hand-written contract with `fx_gpu::Params`, and
    // inserting a field in the middle moves every one of them — a change that
    // compiles on both sides and draws nonsense.
    src_size: vec2<u32>,
    // Where the layer's rectangle sits in the texture `unpremul` writes. A batch
    // of sibling layers writes its results into **one** destination packed like
    // its source (§15 D1003 (7), `fx_gpu::run_batch`), so the last pass stores at
    // an offset; every other pass writes a scratch slot at its own origin and
    // ignores this. Appended, for the reason on `src_size`.
    dst_origin: vec2<i32>,
    // Where this dispatch's kernel starts in `kernel`: one buffer holds every
    // kernel a batch uses, so a blur names its own stretch of it.
    k_off: u32,
    // A sharpen's amount, 1.0 for 100%. Appended, for the reason on `src_size`.
    amount: f32,
    // The noise pass's numbers (`fx_gpu::Params::noise`): [0] the lattice map's
    // origin and x step, [1] its y step and a grain's amount. Appended, for the
    // reason on `src_size`.
    nz: array<vec4<f32>, 4>,
    // Octaves, 1 for the fractal sum, the grain's blend mode
    // (`effects::blend_code`), 1 for a colour grain.
    nzi: vec4<u32>,
};

@group(0) @binding(0) var src_a: texture_2d<f32>;
@group(0) @binding(1) var src_b: texture_2d<f32>;
@group(0) @binding(2) var dst: texture_storage_2d<rgba8unorm, write>;
@group(0) @binding(3) var<uniform> p: Params;
@group(0) @binding(4) var<storage, read> kernel: array<f32>;

const HORIZONTAL: u32 = 1u;
const INNER: u32 = 2u;
const GROW: u32 = 4u;

fn inside(c: vec2<i32>) -> bool {
    return c.x >= 0 && c.y >= 0 && c.x < i32(p.size.x) && c.y < i32(p.size.y);
}

// Round half-up to the nearest 8-bit level, as the CPU reference does.
fn q(v: vec4<f32>) -> vec4<f32> {
    return floor(clamp(v, vec4<f32>(0.0), vec4<f32>(1.0)) * 255.0 + 0.5) / 255.0;
}

fn load_a(c: vec2<i32>) -> vec4<f32> {
    return textureLoad(src_a, c, 0);
}

// **Zero outside the buffer, never the edge pixel.** A layer's blur has nothing
// outside it, so clamping the edge would smear the outermost row outwards
// forever — `effects::convolve` says the same in its own words.
fn load_a_edge(c: vec2<i32>) -> vec4<f32> {
    if (!inside(c)) {
        return vec4<f32>(0.0);
    }
    return textureLoad(src_a, c, 0);
}

fn mval(i: u32) -> f32 {
    return p.m[i / 4u][i % 4u];
}

@compute @workgroup_size(8, 8)
fn premul(@builtin(global_invocation_id) gid: vec3<u32>) {
    let c = vec2<i32>(gid.xy);
    if (!inside(c)) { return; }
    let s = load_a(c + p.src_origin);
    textureStore(dst, c, q(vec4<f32>(s.rgb * s.a, s.a)));
}

@compute @workgroup_size(8, 8)
fn unpremul(@builtin(global_invocation_id) gid: vec3<u32>) {
    let c = vec2<i32>(gid.xy);
    if (!inside(c)) { return; }
    let s = load_a(c);
    if (s.a <= 0.0) {
        // Colour under a zero alpha is not a colour; dividing by it would make a
        // NaN that the clamp then turns into white.
        textureStore(dst, c + p.dst_origin, vec4<f32>(0.0));
        return;
    }
    textureStore(dst, c + p.dst_origin, q(vec4<f32>(s.rgb / s.a, s.a)));
}

@compute @workgroup_size(8, 8)
fn copy_pass(@builtin(global_invocation_id) gid: vec3<u32>) {
    let c = vec2<i32>(gid.xy);
    if (!inside(c)) { return; }
    textureStore(dst, c, load_a(c));
}

// `effects::apply_matrix`: un-premultiply, transform, re-premultiply. A colour
// matrix is defined on straight colour, and running it on premultiplied data
// makes every coefficient depend on the alpha beside it — a hue shift confined
// to soft edges, i.e. exactly where nobody looks for it.
@compute @workgroup_size(8, 8)
fn matrix(@builtin(global_invocation_id) gid: vec3<u32>) {
    let c = vec2<i32>(gid.xy);
    if (!inside(c)) { return; }
    let s = load_a(c);
    if (s.a <= 0.0) {
        textureStore(dst, c, vec4<f32>(0.0));
        return;
    }
    let straight = vec4<f32>(s.rgb / s.a, s.a);
    var o: vec4<f32>;
    for (var row = 0u; row < 4u; row = row + 1u) {
        o[row] = mval(row * 5u + 0u) * straight.r
            + mval(row * 5u + 1u) * straight.g
            + mval(row * 5u + 2u) * straight.b
            + mval(row * 5u + 3u) * straight.a
            + mval(row * 5u + 4u);
    }
    let oa = clamp(o.a, 0.0, 1.0);
    textureStore(dst, c, q(vec4<f32>(clamp(o.rgb, vec3<f32>(0.0), vec3<f32>(1.0)) * oa, oa)));
}

// One axis of a separable gaussian. The kernel comes from `effects::kernel` on
// the host, so the weights are the same numbers the CPU convolves with —
// including its truncated-then-normalized sum, which is what keeps a large flat
// fill from stepping down by the 0.3% of the tail beyond 3σ.
@compute @workgroup_size(8, 8)
fn blur(@builtin(global_invocation_id) gid: vec3<u32>) {
    let c = vec2<i32>(gid.xy);
    if (!inside(c)) { return; }
    let horizontal = (p.flags & HORIZONTAL) != 0u;
    let step = select(vec2<i32>(0, 1), vec2<i32>(1, 0), horizontal);
    var acc = vec4<f32>(0.0);
    let n = 2 * p.radius;
    for (var t = 0; t <= n; t = t + 1) {
        acc = acc + load_a_edge(c + step * (t - p.radius)) * kernel[p.k_off + u32(t)];
    }
    textureStore(dst, c, q(acc));
}

// `effects::sharpen`'s unsharp mask: `src_a` the layer, `src_b` its blur. The
// clamp is SVG's arithmetic `feComposite`'s, alpha to 0..1 and then each colour
// to the alpha, so a bright edge's overshoot rings as the exported file's does.
@compute @workgroup_size(8, 8)
fn sharpen(@builtin(global_invocation_id) gid: vec3<u32>) {
    let c = vec2<i32>(gid.xy);
    if (!inside(c)) { return; }
    let o = load_a(c);
    let v = o + p.amount * (o - textureLoad(src_b, c, 0));
    let a = clamp(v.a, 0.0, 1.0);
    textureStore(dst, c, q(vec4<f32>(clamp(v.rgb, vec3<f32>(0.0), vec3<f32>(a)), a)));
}

// `effects::coarsen`: one thread per `radius`×`radius` block, averaging it into
// a single texel of the coarse grid that lives in the destination's top-left
// corner. Dispatched over the coarse grid, so the total work is one read per
// source pixel however large the block is — a full-size dispatch reading k²
// texels each would be k² times the work and is the obvious way to write this.
//
// **Off the right and bottom edges reads as transparent and still divides by
// k²**, matching the reference: dividing a partial block by its covered count
// instead leaves a bright rim down two sides of every resampled shadow.
//
// 🚨 **The loop is bounded by the source, and that is the whole of D403's
// promise** (§15 D743). The sentence above — one read per source
// pixel however large the block is — holds only while `k ≤ min(w, h)`. Past that
// the coarse grid is 1×1, one thread iterates `k²` times over a buffer of `w·h`,
// and the pass **becomes** the full-size spelling D403 says it was not writing:
// **116.3 ms** measured on a 4070 Ti for one shadow at `k = 1334`, against
// **0.45 ms** with the bound — 258× — and the figure does not move when the
// buffer does. Running to
// `hi` skips exactly the taps the old `if` skipped, so no value changes — the
// divisor stays `k²`, which is what the transparent-off-edge rule requires.
//
// `min(p.radius, src)` before the addition rather than after: `base + p.radius`
// on a block factor near `i32`'s ceiling is a wrap, and the clamped form cannot
// exceed twice the buffer. The divisor is computed in `f32` for the same reason —
// `p.radius * p.radius` in `i32` wraps above 46 341, which `MAX_SHADOW_BLOCK`
// keeps the host from sending today and this shader should not depend on.
@compute @workgroup_size(8, 8)
fn coarsen(@builtin(global_invocation_id) gid: vec3<u32>) {
    let c = vec2<i32>(gid.xy);
    if (!inside(c)) { return; }
    let src = vec2<i32>(p.src_size);
    let base = c * p.radius;
    let hi = min(src, base + min(vec2<i32>(p.radius), src));
    var acc = vec4<f32>(0.0);
    for (var y = base.y; y < hi.y; y = y + 1) {
        for (var x = base.x; x < hi.x; x = x + 1) {
            acc = acc + textureLoad(src_a, vec2<i32>(x, y), 0);
        }
    }
    textureStore(dst, c, q(acc / (f32(p.radius) * f32(p.radius))));
}

// `effects::split`: the coarse index below `v` and the fraction past it, clamped
// into `0..n`. Written as its own function for the same reason the reference has
// one — the clamping at *both* ends is what stops the last row of a shadow
// stretching, and it is three lines that are easy to get subtly different.
fn split(v: f32, n: i32) -> vec2<f32> {
    if (v <= 0.0) { return vec2<f32>(0.0, 0.0); }
    let i = floor(v);
    if (i >= f32(n - 1)) { return vec2<f32>(f32(n - 1), 0.0); }
    return vec2<f32>(i, v - i);
}

// `effects::upsample`: read the coarse grid back at full resolution, bilinearly.
// A coarse sample stands at the centre of the block it averaged, `(k-1)/2` pixels
// in — the half-block the `- half` accounts for, and the one thing here that
// would show up as a shadow creeping sideways as the zoom changes rather than as
// anything obviously wrong.
@compute @workgroup_size(8, 8)
fn upsample(@builtin(global_invocation_id) gid: vec3<u32>) {
    let c = vec2<i32>(gid.xy);
    if (!inside(c)) { return; }
    let k = f32(p.radius);
    let half = (k - 1.0) / 2.0;
    let sx = split((f32(c.x) - half) / k, i32(p.src_size.x));
    let sy = split((f32(c.y) - half) / k, i32(p.src_size.y));
    let i0 = vec2<i32>(i32(sx.x), i32(sy.x));
    let i1 = min(i0 + vec2<i32>(1), vec2<i32>(p.src_size) - vec2<i32>(1));
    let t = vec2<f32>(sx.y, sy.y);
    let a = textureLoad(src_a, vec2<i32>(i0.x, i0.y), 0);
    let b = textureLoad(src_a, vec2<i32>(i1.x, i0.y), 0);
    let cc = textureLoad(src_a, vec2<i32>(i0.x, i1.y), 0);
    let d = textureLoad(src_a, vec2<i32>(i1.x, i1.y), 0);
    let top = a + (b - a) * t.x;
    let bot = cc + (d - cc) * t.x;
    textureStore(dst, c, q(top + (bot - top) * t.y));
}

// The silhouette a shadow is cast from: the graphic's alpha, shifted by the
// offset. Colour is discarded — a shadow is its own colour everywhere, and
// `tint` supplies it.
//
// 🚨 **An inner shadow is complemented at the `tint`, not here** (§15 D742) —
// `effects::shadow_layer` says why at length, and this is the half that has to
// agree with it. Inverting here made this pass's edge rule
// ("outside the buffer is fully casting") the opposite of `blur`'s, `spread`'s
// and `coarsen`'s, which all read outside as transparent; on a shape whose ink is
// its own bounding box the inverted silhouette is zero everywhere the buffer
// holds, so at offset `(0, 0)` an inner shadow drew nothing at all. Both backends
// agreed, which is why `fx_gpu`'s comparison stayed green over two
// implementations of the same wrong picture.
@compute @workgroup_size(8, 8)
fn silhouette(@builtin(global_invocation_id) gid: vec3<u32>) {
    let c = vec2<i32>(gid.xy);
    if (!inside(c)) { return; }
    let s = c - p.off;
    var a = 0.0;
    if (inside(s)) {
        a = textureLoad(src_a, s, 0).a;
    }
    textureStore(dst, c, q(vec4<f32>(0.0, 0.0, 0.0, a)));
}

// One axis of the box dilation `spread` performs. Boxy on purpose: SVG's
// `feMorphology` is boxy too, so the export and the canvas are boxy in the
// *same* way rather than one of them being round.
@compute @workgroup_size(8, 8)
fn spread(@builtin(global_invocation_id) gid: vec3<u32>) {
    let c = vec2<i32>(gid.xy);
    if (!inside(c)) { return; }
    let horizontal = (p.flags & HORIZONTAL) != 0u;
    let grow = (p.flags & GROW) != 0u;
    let step = select(vec2<i32>(0, 1), vec2<i32>(1, 0), horizontal);
    var best = select(1.0, 0.0, grow);
    for (var d = -p.radius; d <= p.radius; d = d + 1) {
        let t = c + step * d;
        // Outside the buffer reads as empty, matching the blur's transparent
        // edge: a silhouette does not continue past the region it was
        // rasterized into.
        var v = 0.0;
        if (inside(t)) {
            v = textureLoad(src_a, t, 0).a;
        }
        best = select(min(best, v), max(best, v), grow);
    }
    // Quantised through the same 8-bit ladder the CPU walks, so a min/max over
    // one backend's rounded values cannot pick a different neighbour.
    textureStore(dst, c, q(vec4<f32>(0.0, 0.0, 0.0, best)));
}

// The shadow's colour at the silhouette's alpha, times the colour's own — and
// **the complement for an inner shadow**, which is the step `silhouette` above no
// longer performs. `INNER` reaches this pass rather than that one now.
@compute @workgroup_size(8, 8)
fn tint(@builtin(global_invocation_id) gid: vec3<u32>) {
    let c = vec2<i32>(gid.xy);
    if (!inside(c)) { return; }
    let inner = (p.flags & INNER) != 0u;
    let v = load_a(c).a;
    let a = select(v, 1.0 - v, inner) * p.tint.a;
    textureStore(dst, c, q(vec4<f32>(p.tint.rgb * a, a)));
}

// Confine an inner shadow to the layer, or the inverted silhouette paints the
// whole buffer outside the shape. `src_b` is the graphic and its alpha is the
// shape.
@compute @workgroup_size(8, 8)
fn mask_in(@builtin(global_invocation_id) gid: vec3<u32>) {
    let c = vec2<i32>(gid.xy);
    if (!inside(c)) { return; }
    let k = textureLoad(src_b, c, 0).a;
    textureStore(dst, c, q(load_a(c) * k));
}

// Source-over, both sides premultiplied: `src_a` over `src_b`.
@compute @workgroup_size(8, 8)
fn over(@builtin(global_invocation_id) gid: vec3<u32>) {
    let c = vec2<i32>(gid.xy);
    if (!inside(c)) { return; }
    let s = load_a(c);
    let d = textureLoad(src_b, c, 0);
    textureStore(dst, c, q(s + d * (1.0 - s.a)));
}

// ---- noise: `mastik_core::noise::Lattice`, line for line ----------------------
//
// The lattice's tables sit in `kernel` from `k_off` (`Lattice::table`): 514
// selector entries, then each channel's 514 two-component gradients.

const NZ_LEN: u32 = 514u;

fn nz_sel(i: u32) -> u32 {
    return u32(kernel[p.k_off + i]);
}

fn nz_grad(ch: u32, b: u32) -> vec2<f32> {
    let at = p.k_off + NZ_LEN + (ch * NZ_LEN + b) * 2u;
    return vec2<f32>(kernel[at], kernel[at + 1u]);
}

fn nz_curve(t: f32) -> f32 {
    return t * t * (3.0 - 2.0 * t);
}

// `Lattice::noise2`.
fn noise2(ch: u32, x: f32, y: f32) -> f32 {
    let fx = floor(x);
    let fy = floor(y);
    let rx0 = x - fx;
    let ry0 = y - fy;
    let rx1 = rx0 - 1.0;
    let ry1 = ry0 - 1.0;
    let bx0 = u32(i32(fx) & 255);
    let by0 = u32(i32(fy) & 255);
    let bx1 = (bx0 + 1u) & 255u;
    let by1 = (by0 + 1u) & 255u;
    let i = nz_sel(bx0);
    let j = nz_sel(bx1);
    let b00 = nz_sel(i + by0);
    let b10 = nz_sel(j + by0);
    let b01 = nz_sel(i + by1);
    let b11 = nz_sel(j + by1);
    let sx = nz_curve(rx0);
    let sy = nz_curve(ry0);
    let g00 = nz_grad(ch, b00);
    let g10 = nz_grad(ch, b10);
    let g01 = nz_grad(ch, b01);
    let g11 = nz_grad(ch, b11);
    let u0 = rx0 * g00.x + ry0 * g00.y;
    let v0 = rx1 * g10.x + ry0 * g10.y;
    let a = u0 + sx * (v0 - u0);
    let u1 = rx0 * g01.x + ry1 * g01.y;
    let v1 = rx1 * g11.x + ry1 * g11.y;
    let b = u1 + sx * (v1 - u1);
    return a + sy * (b - a);
}

// `Lattice::level`: one channel of the octave sum as an 8-bit level.
fn nz_level(ch: u32, uv: vec2<f32>) -> f32 {
    let fractal = p.nzi.y == 1u;
    var x = uv.x;
    var y = uv.y;
    var ratio = 1.0;
    var sum = 0.0;
    for (var o = 0u; o < p.nzi.x; o = o + 1u) {
        let n = noise2(ch, x, y);
        if (fractal) {
            sum = sum + n / ratio;
        } else {
            sum = sum + abs(n) / ratio;
        }
        x = x * 2.0;
        y = y * 2.0;
        ratio = ratio * 2.0;
    }
    var v = sum * 255.0;
    if (fractal) {
        v = (sum * 255.0 + 255.0) / 2.0;
    }
    return floor(clamp(v, 0.0, 255.0) + 0.5);
}

// `noise::round_trip`: a level premultiplied by the noise's own alpha and read
// back straight, each step at 8 bits.
fn nz_round_trip(c: f32, a: f32) -> f32 {
    if (a == 0.0) {
        return 0.0;
    }
    let af = a / 255.0;
    let pre = floor(c * af + 0.5);
    return floor(min(pre / af + 0.5, 255.0));
}

// `LatticeMap::at`.
fn nz_at(c: vec2<i32>) -> vec2<f32> {
    let x = f32(c.x);
    let y = f32(c.y);
    return vec2<f32>(
        p.nz[0].x + x * p.nz[0].z + y * p.nz[1].x,
        p.nz[0].y + x * p.nz[0].w + y * p.nz[1].y,
    );
}

fn bl_screen(b: f32, s: f32) -> f32 {
    return b + s - b * s;
}

fn bl_hard_light(b: f32, s: f32) -> f32 {
    if (s <= 0.5) {
        return b * 2.0 * s;
    }
    return bl_screen(b, 2.0 * s - 1.0);
}

// `effects::blend`'s separable modes, one channel, by `effects::blend_code`.
fn bl_channel(m: u32, b: f32, s: f32) -> f32 {
    switch m {
        case 2u: { return min(b, s); }
        case 3u: { return b * s; }
        case 4u: {
            if (b >= 1.0) { return 1.0; }
            if (s <= 0.0) { return 0.0; }
            return 1.0 - min((1.0 - b) / s, 1.0);
        }
        case 5u: { return max(b, s); }
        case 6u: { return bl_screen(b, s); }
        case 7u: {
            if (b == 0.0) { return 0.0; }
            if (s >= 1.0) { return 1.0; }
            return min(b / (1.0 - s), 1.0);
        }
        case 8u: { return bl_hard_light(s, b); }
        case 9u: {
            if (s <= 0.5) {
                return b - (1.0 - 2.0 * s) * b * (1.0 - b);
            }
            var d = sqrt(b);
            if (b <= 0.25) {
                d = ((16.0 * b - 12.0) * b + 4.0) * b;
            }
            return b + (2.0 * s - 1.0) * (d - b);
        }
        case 10u: { return bl_hard_light(b, s); }
        case 11u: { return abs(b - s); }
        case 12u: { return b + s - 2.0 * b * s; }
        default: { return s; }
    }
}

fn bl_lum(c: vec3<f32>) -> f32 {
    return 0.3 * c.x + 0.59 * c.y + 0.11 * c.z;
}

fn bl_sat(c: vec3<f32>) -> f32 {
    return max(max(c.x, c.y), c.z) - min(min(c.x, c.y), c.z);
}

fn bl_set_lum(c0: vec3<f32>, l0: f32) -> vec3<f32> {
    let c = c0 + vec3<f32>(l0 - bl_lum(c0));
    let l = bl_lum(c);
    let n = min(min(c.x, c.y), c.z);
    let x = max(max(c.x, c.y), c.z);
    var o = c;
    if (n < 0.0) {
        o = vec3<f32>(l) + (o - vec3<f32>(l)) * l / (l - n);
    }
    if (x > 1.0) {
        o = vec3<f32>(l) + (o - vec3<f32>(l)) * (1.0 - l) / (x - l);
    }
    return o;
}

fn bl_set_sat(c: vec3<f32>, s: f32) -> vec3<f32> {
    let n = min(min(c.x, c.y), c.z);
    let x = max(max(c.x, c.y), c.z);
    if (x > n) {
        return (c - vec3<f32>(n)) * s / (x - n);
    }
    return vec3<f32>(0.0);
}

// `effects::blend`: backdrop `cb`, source `cs`, by `effects::blend_code`.
fn blend(m: u32, cb: vec3<f32>, cs: vec3<f32>) -> vec3<f32> {
    switch m {
        case 13u: { return bl_set_lum(bl_set_sat(cs, bl_sat(cb)), bl_lum(cb)); }
        case 14u: { return bl_set_lum(bl_set_sat(cb, bl_sat(cs)), bl_lum(cb)); }
        case 15u: { return bl_set_lum(cs, bl_lum(cb)); }
        case 16u: { return bl_set_lum(cb, bl_lum(cs)); }
        default: {
            return vec3<f32>(
                bl_channel(m, cb.x, cs.x),
                bl_channel(m, cb.y, cs.y),
                bl_channel(m, cb.z, cs.z),
            );
        }
    }
}

// `effects::grain`: the opaque noise blended over the layer, cut to its alpha,
// mixed in at the amount.
@compute @workgroup_size(8, 8)
fn grain(@builtin(global_invocation_id) gid: vec3<u32>) {
    let c = vec2<i32>(gid.xy);
    if (!inside(c)) { return; }
    let s = load_a(c);
    if (s.a <= 0.0) {
        textureStore(dst, c, s);
        return;
    }
    let uv = nz_at(c);
    let na = nz_level(3u, uv);
    var n = vec3<f32>(nz_round_trip(nz_level(0u, uv), na) / 255.0);
    if (p.nzi.w == 1u) {
        n = vec3<f32>(
            nz_round_trip(nz_level(0u, uv), na),
            nz_round_trip(nz_level(1u, uv), na),
            nz_round_trip(nz_level(2u, uv), na),
        ) / 255.0;
    }
    let a = s.a;
    let cb = clamp(s.rgb / a, vec3<f32>(0.0), vec3<f32>(1.0));
    let b = blend(p.nzi.z, cb, n);
    let over = a * ((1.0 - a) * n + a * b);
    let amount = p.nz[1].z;
    let v = amount * over + (1.0 - amount) * s.rgb;
    textureStore(dst, c, q(vec4<f32>(clamp(v, vec3<f32>(0.0), vec3<f32>(a)), a)));
}
