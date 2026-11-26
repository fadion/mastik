# Effects

The effect model (`mastik_core::effect`, §5.3a) and the pixel passes that draw it (`mastik-render/src/effects.rs`, §6.4). The passes are the single specification read by the CPU backend, the GPU shaders and the SVG writer.

## 5.3a Effects (`effect.rs`)

```rust
pub struct Effect { pub kind: EffectKind, pub visible: bool, pub vars: EffectVars }
pub type EffectVars = [Option<VarId>; 4];   // X, Y, blur, spread (EFFECT_X/_Y/_BLUR/_SPREAD);
                                            // a blur's or glass's radius is the BLUR slot, a
                                            // progressive blur's end radius the SPREAD slot
impl Effect {
    pub fn numbers(&self) -> [Option<f64>; 4];  // None where the kind has no such number
    pub fn set_number(&mut self, slot: usize, x: f64);  // silent no-op where the kind lacks it;
                                    // a ramp's end held to 0..=MAX_BLUR here
    pub fn number_read(slot: usize, x: f64) -> f64;  // blur within 0..=MAX_BLUR; offset and spread as is
    pub fn with_progressive(&self, on: bool) -> Self; // Even <-> Progressive, bindings carried
}

pub enum EffectKind {
    DropShadow(Shadow),
    InnerShadow(Shadow),
    LayerBlur { radius: f64, ramp: Option<BlurRamp> },  // the RADIUS as typed, not the deviation;
                                    // with a ramp, the radius at its start
    Sharpen { amount: f64, radius: f64 },  // unsharp mask; amount 0..=MAX_SHARPEN (2.0), radius a blur's
    Noise(Noise),                   // noise blended with the layer, inside its alpha (mastik_core::noise)
    Texture(Texture),               // the layer's drawing moved by a noise field; runs first (mastik_core::noise)
    BackgroundBlur { radius: f64, ramp: Option<BlurRamp> },  // blurs what lies behind, inside the outline
    Glass(Glass),                   // backdrop blurred, bent and frosted, a lit rim over the fill; tag `glass`
    Filters(Filters),
}

pub struct BlurRamp {               // a progressive blur; skipped in the file when None
    pub end: f64,                   // the radius at `to`
    pub from: Point, pub to: Point, // on the layer's box normalised 0..1, (0,0) its top-left
    pub ease: RampEase,             // Linear | Smooth (smoothstep, the default)
}

pub struct Glass {                  // read through GlassWire: fields optional, held to range
    pub blur: f64,                  // a background blur's units, 0..=MAX_BLUR; 16
    pub frost: f64,                 // white veil, 0..=1; 10%
    pub bend: f64,                  // 0..=1; 30%
    pub depth: f64,                 // how far in the bend reaches, layer units, 0..=MAX_BLUR; 12
    pub dispersion: f64,            // 0..=1; 0
    pub rim: Rim,
}
pub struct Rim { pub angle: f64, pub width: f64, pub color: Color }  // 135°, 1, white at 60%

pub struct Shadow {
    pub offset: kurbo::Vec2,        // layer's own space, so it turns with the layer
    pub blur: f64,                  // radius
    pub spread: f64,                // dilate (or erode) the silhouette before blurring
    pub color: peniko::Color,       // straight sRGB; the ALPHA is the opacity
    pub blend: BlendMode,           // Normal by default, never PassThrough; absent from the file at Normal
}
impl Shadow { pub fn blends(&self) -> bool; }   // mode is not Normal

pub struct Filters { pub brightness: f64, pub contrast: f64, pub saturation: f64, pub hue: f64 }
                                    // factors, 1.0 neutral; hue in DEGREES, 0.0 neutral

pub struct Noise {                  // mastik_core::noise; serde tag `noise`
    pub size: [f64; 2],             // one grain's width and height in document units, NOISE_SIZE_RANGE (0.5..=50)
    pub density: f64,               // 0..=1, the share of the layer covered; 1 by default, absent from the file at 1
    pub seed: i32,
    pub blend: BlendMode,           // Normal by default, never PassThrough; absent from the file at Normal
    pub colors: [Color; 2],         // the low and high end; a transparent second is a one-colour noise;
                                    // black 40% -> black clear by default
    pub multi: bool,                // three channels of noise colour of its own, at `opacity`; colors unused
    pub opacity: f64,               // 0..=1, a multicolour noise's strength; 0.3 by default
}

pub struct Texture {                // mastik_core::noise; serde tag `texture`
    pub size: [f64; 2],             // one grain, as Noise's (NOISE_SIZE_RANGE); 2 x 2 by default
    pub amount: f64,                // how far the edge moves, document units, TEXTURE_AMOUNT_RANGE (0..=100); 4
    pub seed: i32,                  // 1 by default
    pub detail: u8,                 // fractal octaves, TEXTURE_DETAIL_RANGE (1..=5); 3
}

pub const BLUR_DEVIATION: f64 = 0.5;   // deviation = radius x this
pub const BLUR_CUTOFF: f64 = 3.0;      // deviations worth rasterizing (vello_common's number)
pub fn deviation(radius: f64) -> f64;  // what the three writers take
pub fn reach(radius: f64) -> f64;      // what bounds and culling use
pub fn stack_escape(effects: &[Effect]) -> Insets;
pub fn buffer_escape(effects: &[Effect]) -> Insets;          // stack_escape + where a sharpen, texture or glass reads
pub fn texture_reach(effects: &[Effect]) -> f64;             // visible textures' amounts, summed
pub fn backdrop_blur(effects: &[Effect]) -> Option<(f64, Option<BlurRamp>)>;  // visible background blurs as one
pub fn backdrop_radius(effects: &[Effect]) -> Option<f64>;   // backdrop_blur's larger radius
pub fn backdrop_reach(effects: &[Effect]) -> Option<f64>;    // how far past the outline the layer reads behind
pub fn ramps_on_box(effects: Cow<[Effect]>, bounds: Rect) -> Cow<[Effect]>;  // ramps into the layer's space
pub fn escaped(bounds: Rect, escape: Insets, world: Affine) -> Rect;   // local escape -> world box
```

- **Nine effects**, offered in `EffectKind::all_defaults`' order: drop shadow, inner shadow, layer blur, sharpen, noise, texture, background blur, glass and colour filters. All but the background blur and the glass draw alike on the canvas, the raster export and the SVG writer; SVG cannot express a backdrop, so the SVG writer leaves those two out and reports them (§7). A layer's own blend mode composites outside its effect layer, so its shadows blend with it (§5.3, §6.2).
- **A background blur shows what lies behind the layer, blurred, inside its outline** (`scene::paint_backdrop`, §6.2). It is no pass over the layer's own pixels, and is drawn behind everything the layer draws, so its place in the list matters only beside a glass (*Glass*) or another progressive one, since the first progressive one leads the ramp (*Progressive blur*). Its radius is a layer blur's, in the same units, bound through the same slot, carried by `retyped` and held to the same cap. Several visible ones compose into one radius, `backdrop_blur` = `sqrt(r1^2 + r2^2)`, since blurs in sequence convolve. A mask draws none, nor does any layer inside a mask group: their ink is drawn into the mask layer, where a backdrop would become mask, so `paint_node_pass` skips them (the walk counts the ink masks it is inside) and `spread_through_backdrops` skips any layer reading its backdrop with a mask among its ancestors.
- **Either blur can be progressive** (*Progressive blur*, below): a ramp, not a kind of its own. **A glass** is a background blur that also bends at the outline, frosts and lights a rim (*Glass*, below).
- **A sharpen is an unsharp mask over the layer's own drawing**: `original + amount × (original − blur)` per premultiplied channel, clamped as SVG's arithmetic `feComposite` clamps (alpha to 0–1, then each colour to the alpha), a pass of stage 1 beside the layer blur whose passes it reuses (§6.4). An effect rather than a picture's adjustment, since it reads neighbouring pixels, so it works on any layer. Amount is 0–200% (`MAX_SHARPEN`, refused past it by `Document::apply` and clamped on load with the radius); the radius is a layer blur's, in the same units, bound through the blur slot (the amount binds nothing). It shares nothing with the blurs on a retype: its radius is a pixel or two where a blur's is a dozen, so it starts on `DEFAULT_SHARPEN_RADIUS` (1.5) at 60%. Where the layer is transparent the mask is negative and clamps to nothing, so it escapes `ZERO`; but its blur reads past the pixel it draws, so an effect layer's buffer is clipped with `buffer_escape` (the stack's escape grown by each visible sharpen's reach), or a layer cut by the target would draw a bright rim along the cut.
- **Noise is an effect, not a fill**: it lies over the layer's base fill, and its colours are picked from ordinary swatches (a noise inside the colour picker could reach colours only through the eyedropper, since the picker cannot open inside itself). It is blended with the layer's own drawing, inside its alpha, a stage-1 pass in list order like the sharpen (§6.4, *Noise*). It lives in the layer's own space, so it turns and scales with the layer. Per pixel, so it escapes `ZERO`; it binds no number, shares nothing on a retype, and stays under level of detail as `Filters` do. It has ink only with some density and a visible colour (or, multicolour, some opacity). `Noise` deserializes through a wire type (`NoiseWire`) in which every field may be left out, every number is held to range and Pass through reads as Normal, so a file, a paste or an MCP write never holds a value out of range (`blur_within_cap` asks `n == n.clamped()`).
- **A texture moves the layer's own drawing** (children, fill, stroke) by a noise field, so its outline roughens, not only its fill (*Texture*, §6.4). **It runs before every other effect wherever its row sits**, so a shadow is cast from the rough edge and a noise speckles the rough shape: a shadow keeping the smooth edge under a rough shape looks broken, and a movable stage would make every other effect's order a question. It lives in the layer's own space as noise does. It escapes `uniform(amount)`, since a pixel moves at most `amount` on either axis, and reads as far again, so `buffer_escape` grows by the amount as by a sharpen's reach (a buffer cut by the target would otherwise pull transparency in along the cut). Level of detail drops it under a device pixel (render.md). It binds no number; a retype from noise carries the grain size and seed, from anything else the default. It has ink only with some amount. `Texture` reads through `TextureWire` as `Noise` through `NoiseWire`.
- **A shadow carries its own blend mode**, so a Multiply shadow darkens a coloured ground: a drop shadow blends with what lies under the layer, an inner one with the layer and what shows through it. A Normal container isolates its children, not its shadows; a layer in any other mode composites its shadows by that mode, so the layer's mode wins over the shadow's. One of the sixteen modes, never Pass through (a shadow is no container): `build::effect_is_finite` refuses it at both doors and the loader's `effect::clamp_blur` reads it as Normal. `EffectKind::retyped` carries it between drop and inner. The canvas draws such a stack in passes (§6.2); the SVG writer cannot (§7).
- **`visible` sits beside the payload** (as on `Fill`) so hiding a shadow does not throw away its tuned numbers.
- **A shadow's opacity is its colour's alpha**, with no second field. The rule is "one door": `paint::alpha_of` / `with_alpha` are the single pair that read and write a brush's alpha. (A gradient fill's opacity is its own field because scaling stops is not invertible through zero, §15 D767.)
- **A blur stores the radius the designer types**, not the gaussian deviation; `BLUR_DEVIATION` is the one place the two relate. CSS `blur(12px)` means deviation 6; SVG `feGaussianBlur` and both vello backends take the deviation. A radius is capped at `MAX_BLUR` (1,000), because the CPU backend's buffer grows with it: `Document::apply` refuses past it (`OpError::BlurTooLarge`, at `SetEffects` and `InsertSubtree`) outside a history step, a load or a clipboard read clamps to it (`NodeDto::into_node`, and an effects style in `into_document`), the SVG importer clamps to it, and a bound blur is read within it. A history step is exempt because it only puts back what the document held, so an undo never fails over the cap.
- **`Filters` is factors with 1.0 neutral, hue in degrees**, deliberately unlike `ImageAdjust` (`Adjustment::range`, zero neutral, hue in half turns; see images.md): these numbers go straight into `feColorMatrix` and the shader. `Filters::is_neutral` and `changed` answer "is the row doing anything" and "how many channels are off neutral"; a neutral row must emit no primitive.
- **`escape` is how far an effect's ink lands outside the geometry**, as `Insets` read outward; zero for a hidden effect. A drop shadow reaches `blur + spread` on the side the offset points at, and the same minus the offset on the other. An inner shadow and a `Filters` row escape `ZERO`, and so do a background blur and a glass: they read outside the outline but land only inside it, so `stack_escape`, `layer_blur_deviation` and `buffer_downscale` ignore them too. How far they read is `backdrop_reach`: the background blurs' composed larger radius reached, plus each visible glass's blur reach and depth, summed so it is never short; damage (`spread_through_backdrops`) and the backdrop's repaint use it. A progressive layer blur escapes by its larger end.
  - `escape` is where ink *lands*, not where passes must be *evaluated*. An inner shadow is cast from everything outside the layer, so its silhouette needs `blur + |spread| + |offset|` out. Both renderers get that free (outside the buffer reads as no layer); the SVG writer pads the filter region (§7), because outside a filter region every primitive is transparent black. Do not "fix" `escape` towards the pad: it would break every bound in §5.9.
- **`stack_escape` is a union taken per stage of what §6.4 composes.** Stage 1 is one isotropic reach: the visible textures' amounts summed (`texture_reach`; each moves the last one's result) plus the reach of the convolved deviation of all visible `LayerBlur`s, each at its larger end (`sigma = sqrt(s1^2 + s2^2)`); stage 2 is each shadow's escape grown by stage 1 on every side; the answer is the union. Composition is done in deviations and turned into a reach once (squaring reaches instead agrees only while `BLUR_CUTOFF` is constant).
- **`escaped` splits a local escape into an offset and a radius** rather than transforming four corners. A blur's reach is a disc; the rotated AABB of a square is up to sqrt(2) too big. The radius becomes the exact half-extents of the mapped ellipse (row Euclidean norm, not absolute row sum). It never shrinks a box.
- **Variables**: a shadow's X, Y, blur, spread, a blur's, a sharpen's or a glass's radius, and a progressive blur's end radius bind to number variables (§5.3f, §15 D1074), bindings in the item (`vars`) and the bound number's literal 0. Every reader (ink bounds, scene walk, ghost, SVG writer, snapshot) takes the stack through `Document::resolved_effects`, never `Node::effects` bare.
- Written by `Operation::SetEffects` (§5.7), cached into `Resolved::ink_bounds` (§5.9), drawn through `ScenePainter::push_effect_layer` (or `push_shadow_layer`, §6.2) by the passes below, a background blur and a glass by `scene::paint_backdrop` and a glass's rim by `scene::paint_rims`, exported as one `<filter>` per node (§7), saved as additive `NodeDto::effects` (§5.11), authored in the inspector's Effects card (§9.4).

## 6.4 The effect passes (`effects.rs`)

**One implementation of the pixel arithmetic, read by both backends.** `vello` 0.9 has no filters beyond `draw_blurred_rounded_rect`, and `vello_cpu` 0.2.0 implements only `GaussianBlur` and `DropShadow` as single-primitive graphs (no `ColorMatrix`, no `Composite`, so no colour filters and no inner shadow). Leaning on the library for the two it has would mean two blur implementations on two backends that §3 keeps identical. So `effects.rs` is the specification, the CPU backend is the reference, and `fx.wgsl` has one entry point per step of `effects::run`, in the same order, over the same premultiplied 8-bit values. The gaussian kernel is built on the host by `effects::kernel` and passed to the shader as a buffer. `tests/fx_gpu.rs` runs both over one buffer and compares bytes; the noise and texture passes are held within a tolerance instead (*Noise*, *Texture*, below).

`run` takes `scale` (device pixels per document unit on each axis) and `to_local`, the map from a buffer pixel's corner into the layer's own space (`effects::buffer_to_local`, which both backends call, so their noise and texture land on one lattice); both are carried on both backends' effect frames and the GPU's `EffectJob`.

**The CPU convolution may be reorganized for speed only if it stays byte-identical.** `effects::convolve` sums each pixel's taps in kernel order with the edge taps skipped, which pins the floating-point result; within that, the vertical pass accumulates whole rows, a pass past `THREADED_COST` splits its rows across threads (`for_row_bands`), and a shadow's silhouette, whose colour is zero until the tint, is coarsened, blurred and upsampled in alpha alone (`Channels::Alpha`). `the_blur_is_the_direct_convolution_byte_for_byte` holds it to the direct loop.

`mastik-export` calls `effects::filter_matrix` (once, from the `<filter>` emitter) and `effects::any_ink` (twice) across the crate boundary; that is why SVG needs `mastik-render`. Moving them into core was declined: it would split `filter_matrix`'s twenty coefficients (in the order `fx.wgsl` packs them) from the backend that holds them to account.

`Surface { px: &mut [u8], w, h }` is the seam: RGBA8, neither `vello_cpu::Pixmap` nor `wgpu::Texture`. Its constructor asserts the length (a stride off-by-one would silently shear the picture in release).

Two conventions:

- **Pixels are premultiplied.** Blurring straight alpha haloes soft edges.
- **Filters run in sRGB, not linear.** SVG defaults `color-interpolation-filters` to `linearRGB`, so every emitted filter says `sRGB` explicitly. Matching design tools matters more than physics, and the three writers agreeing matters more than either.

### Staging

`run` is two stages, and `fx_gpu` and the SVG writer (`svg::effect_def`) are arranged the same way. Every visible `Texture` runs first, in list order, wherever its row sits; then `Filters`, `LayerBlur`, `Sharpen` and `Noise` transform the layer's own appearance in list order; then every shadow is cast from *that* result and merged behind or over it. An inner shadow after a drop shadow must not take its silhouette from layer-plus-shadow. `effect::stack_escape` is the same decision for bounds: reordering the stages here means changing it too, and nothing fails if only one moves. A background blur and a glass are no stage: `run` and `fx_gpu` skip them and `any_ink` answers `false` for them, so a stack holding only those pushes no effect layer. The backdrop is drawn in a layer of its own (`scene::backdrop_chain`, *Glass*, §6.2), and a glass's rim in another.

`run_shadows` is `run` with the layer left out: the output starts empty instead of as the graphic, so only the shadows cast remain. It draws a blending shadow's own pass (§6.2); the GPU spells it `layer = false` (`FxFrame`, `EffectJob`, `fx_gpu::Item`), starting `out` with a zero `Tint` instead of a `Copy` of the graphic.

### Colour and geometry details

- `filter_matrix` composes a `Filters` row into the 4x5 `feColorMatrix` array in the order **brightness -> contrast -> saturation -> hue** (the panel's order; composition does not commute). Luminance weights are SVG's sRGB 0.213/0.715/0.072, not NTSC 0.299/0.587/0.114.
- `device_sigma` returns two deviations: a non-uniform scale (viewport, 2x export, skewed ancestor) blurs unevenly, and one averaged sigma would make a blur on a skewed layer round.
- `spread_alpha` is a box dilation (what `feMorphology` does) and takes two half-widths for the same reason. The factor is the ratio of the two column norms `push_effect_layer` computes, so it is unbounded; the bounds side (`escaped`) already maps the spread as an ellipse.
- `effects::spread_steps` caps the half-width at `extent - 1` per axis. Lossless, since both backends read outside the buffer as empty. Cost is still O(w*h*n); running the dilation on the coarse grid is an open decision.

### Inner shadow: complement at the tint

A shadow's offset turns and mirrors with the layer: both backends shift the silhouette by `effects::shadow_shift`, the offset carried by the linear part of the buffer→layer map's inverse and rounded to whole buffer pixels, which is the vector `effect::escaped` bounds. The silhouette both shadow kinds are cast from is the graphic's own alpha. `255 - v` happens once, in the last pass before the confine, never at the silhouette. Inverting first puts two edge rules in contradiction (the offset lookup treats outside as casting while convolve, spread and coarsen treat it as transparent), and on a shape whose ink equals its box at offset (0,0) the render equals no effect at all. The late complement needs no edge flag because every pass between commutes with it: blur with a normalized kernel, `coarsen` (box average), `upsample` (bilinear, weights sum to 1), and dilation-of-complement = erosion. A pass inserted between that is neither linear nor a max/min with matching identity breaks this. `spread_alpha`'s `grow` (`(spread > 0) != inner`) relies on it. The GPU sends `inner` to `Pass::Tint`, not `Pass::Silhouette`.

### Bounding the shadow blur

- `effect::shadow_downscale` / `effects::blur_coarse`: a shadow's blur sigma in device pixels is the deviation times zoom. The layer buffer may not be downscaled for a shadow (the artwork over it stays sharp), but the silhouette alpha field is: past twice the budget it is box-averaged into `ceil(sigma/24)`-pixel blocks, blurred at sigma/k, and read back bilinearly. The half-block offset is the whole read-back; get it wrong and shadows creep with zoom. Both backends take `k` from the one function, and the buffer size from `effects::effect_buffer`. The GPU spells it `coarsen`, a blur over the coarse grid, `upsample`, with the coarse grid in the corner of a full-size scratch texture.
- `coarsen` dispatches one thread per block, so total work is one read per source pixel only while `k <= min(w, h)`; both loops stop at the source edge, not the block edge, and the divisor stays `k^2` (transparent-off-edge).
- Caps: `effect::MAX_SHADOW_BLOCK` = 4096 and `effect::MAX_KERNEL_RADIUS` = 512, asked in `effects::kernel_radius` (the one function both blurs use). A document can hold an absurd deviation (`stdDeviation="1e30"` imports without report); unbounded, it hangs the cover worker and anything that joins it (library settings Save). `MAX_SHADOW_BLOCK` is now a correctness cap (`k as i32` would wrap negative and draw no shadow).

### Sharpen

`effects::sharpen` keeps the layer, blurs a copy through the layer blur's own passes, and combines the two per pixel with the clamp SVG's arithmetic `feComposite` applies (alpha to 0–1, colour to the alpha); `fx.wgsl`'s `sharpen` entry does the same over `src_a` (the layer, kept in the `O0` scratch slot, free until stage 2) and `src_b` (its blur), the amount in `Params::amount`. The clamp must be in all three writers: left at 1, a colour past its alpha survives into any later pass that mixes premultiplied pixels and rings differently. The blurred copy is only subtracted, so past the budget it is taken on the coarse grid by `effect::shadow_downscale`'s answer, as a shadow's silhouette is; it never licenses `buffer_downscale`, since the output stays sharp. The SVG writer emits `feGaussianBlur` (`result="fx{n}b"`) and `feComposite operator="arithmetic"` with `k2 = 1 + a`, `k3 = −a`, both at four places whatever the spec's precision and from one `a` rounded once, so they sum to 1; an `a` that rounds to 0 writes no sharpen, since the reader refuses a zero `k3` and the stack with it; `svg_in::sharpen_amount` reads that pair back as a sharpen, keyed on those exact coefficients (`k1 = k4 = 0`, the composite over the blur's input and result), so another tool's arithmetic composite is not misread as one. CSS has no unsharp mask: `css_blocker` sends the layer to inline SVG (*a sharpen*), which carries the filter. Penpot, Sketch and DTCG leave it out and report it.

### Noise

**The generator is SVG's `feTurbulence`, ported**: `mastik_core::noise::Lattice` is the specification's reference code (Park–Miller seeding, the 256-cell lattice, four channels, the octave sum, fractal or turbulence) in `f32`, and `fx.wgsl` repeats it line for line over the table `Lattice::table` uploads into the kernel buffer. The algorithm is specified, so the SVG writer's `<feTurbulence>` is the reference both backends are held to, not a third copy. Three details are what make them agree, and each looks removable:

- **The 8-bit premultiply round trip.** `Lattice::rgba` and `red_alpha` premultiply each level by the noise's own alpha and read it back straight, each step rounded to 8 bits, as every SVG renderer does before `feColorMatrix` reads its input.
- **Pixel corners.** `LatticeMap` maps a buffer pixel's corner, not its centre, into lattice space, since SVG renderers sample there; its origin is reduced modulo 256 cells in `f64`, so a layer far from the origin keeps its precision.
- **`NOISE_FREQUENCY` (0.85).** The base frequency is 0.85 / size on each axis, not 1 / size: the noise is zero on every lattice point, and at 1 every pixel corner of an unscaled layer on whole units is one, so the noise would vanish. It sums `NOISE_OCTAVES` (2) of fractal noise.

**Shading** (`Noise::shade`, read by `effects::noise`, `fx.wgsl`'s `noise_fx` and `svg::noise_graph`):

- **Stretch.** Each channel is stretched about mid-grey by `NOISE_CONTRAST` (3) into `u`, because the fractal sum sits within a quarter of its range and unstretched the two colours would meet as a faint mix of both.
- **Colour.** `u` (from red) picks between the two colours by a straight lerp, each component held at 8 bits as an SVG matrix's output is, for the colour laid on and for how much of it shows (`m`, the lerp of the alphas). A multicolour noise lays the three stretched channels at `opacity`.
- **Density** cuts by the noise's own alpha channel: a pixel is covered when its alpha level is at or above `Noise::density_floor`, which is read off a measured distribution of that channel (`alpha_share`, measured once, the same for every seed and size), so a 30% density covers about 30% of pixels. The SVG writer cuts with `feComponentTransfer` `feFuncA type="linear"` at slope `DENSITY_SLOPE` (1000), so the levels either side of the floor clamp to 0 and 1, then `in`.
- **Mix.** The colour is made opaque, blended over the layer by `blend` through the W3C blend function (`effects::blend`, all sixteen modes; `fx.wgsl`'s `blend` repeats it), cut to the layer's alpha, then mixed with the layer by `m` and clamped as an arithmetic `feComposite`; alpha unchanged. The SVG writer: `feTurbulence` (base frequency at nine places, since it multiplies every coordinate) → stretch `feColorMatrix` → colour and strength matrices → the density cut → `feBlend` → `feComposite operator="in"` the graphic, then `in` and `out` composites by the strength summed by an arithmetic `k2=1 k3=1`. `svg_in` reads no `feTurbulence` and skips such a filter whole, so the export reports a round-trip loss (*a noise, which this app does not read back, nor the rest of its effects with it*).

A stack holding a visible noise is filtered in the layer's own space (§7). Folded filters refuse a stack with a noise (§6.2).

**Tolerances, not bytes.** The shader may fuse a multiply and an add where the CPU rounds between them, so a lattice sum on a rounding boundary lands a level apart and `NOISE_CONTRAST`'s stretch can carry it to three. `tests/fx_gpu.rs` (`a_noise_matches_the_reference`, `a_multicolour_noise_matches_the_reference_in_every_mode`) holds the GPU within 3 levels premultiplied (`NOISE_TOLERANCE`), 8 for Hue, Saturation, Color and Luminosity (`SetSat` divides by a near-grey pixel's channel spread), with at most one channel in a hundred differing; `gpu_effects::both_backends_draw_the_same_noise` within 3. `mastik-export/tests/noise_reference.rs · the_raster_draws_what_the_svg_writer_writes` draws the SVG writer's file through `resvg` and holds the CPU raster to it within 2 levels, past 1 on at most 15% of channels, since `resvg` rounds each composite of the mix at 8 bits; it covers separable modes over a translucent and an opaque layer. `resvg` runs a turned or mirrored filter space unturned, so a turned or mirrored layer's raster is held to the unturned layer's noise turned (`a_turned_or_mirrored_noise_is_the_unturned_noise_turned`). A hue mode over a translucent layer turns by many levels on `resvg`'s 8-bit straight backdrop, so those modes are held against the GPU instead.

### Texture

`effects::texture` is `feDisplacementMap` over `feTurbulence`. The noise is the *Noise* generator on the same lattice and map (`LatticeMap`, pixel corners, `NOISE_FREQUENCY`), summing `detail` fractal octaves. Each pixel takes the layer's colour from where the noise's red level moves it across and its green level down, by `scale × (level/255 − ½)` layer units with `scale = 2 × amount`, so a pixel moves at most `amount`; the move is carried into the buffer by `effects::texture_step` (the inverse of `to_local`'s linear part, times `scale`; zero for a layer scaled flat). The moved pixel is read bilinearly between pixel centres, transparent outside the buffer. `fx.wgsl`'s `texture_fx` (`Pass::Texture`) repeats it, its parameters in noise's slots (the lattice map, `texture_step`, the octaves, fractal); `tests/fx_gpu.rs · a_texture_matches_the_reference` holds it to the CPU at the noise tolerance.

`svg::texture_graph` writes `feTurbulence type="fractalNoise"` (`numOctaves` = `detail`) into `feDisplacementMap scale="2·amount" xChannelSelector="R" yChannelSelector="G"`, and a stack holding a visible texture is filtered in the layer's own space as noise is (`svg::grained`, §7). `svg_in` reads no `feTurbulence`, so the export reports a round-trip loss (*a texture, which this app does not read back, nor the rest of its effects with it*). **`resvg` is no reference for `feDisplacementMap`**: it applies the scale twice and reads the map premultiplied where the specification reads it unpremultiplied. So `noise_reference.rs · a_texture_is_written_as_the_backends_draw_it` pins the written graph rather than comparing pixels, and the backends are held to each other.

### Progressive blur

**A mode of both blurs, not a kind**: `ramp: Some(BlurRamp)` on a layer or background blur, `radius` its start. A kind would have to choose layer or background, and the background form is the one people want for headers; a mode also keeps the radius when toggled. Two ends and an ease only, held at each end's radius past it (a gradient's Pad); a sharp middle with soft ends is two effects.

- **The ends are on the layer's box normalised**, `(0, 0)` its top-left, so a resize carries the ramp. `ramps_on_box` lays them on the layer's box in its own space before the passes (scene walk, SVG writer); a stack handed to a renderer is the one place they are not normalised. `RampEase::Smooth` is smoothstep `3t² − 2t³`, the default; `Linear` is the other.
- **The angle is measured on the normalised box** (`BlurRamp::framed`, `angle`): 0 runs left to right, 90 top to bottom, 45 corner to corner on any box, the same as a gradient's at 0/90/180/270. So the MCP reads and writes an angle without the layer's size. Editing the angle re-frames the ends through the box's centre (`turned`).
- **The end radius binds in the `EFFECT_SPREAD` slot**, the fourth number either blur has free. `set_number` holds it to `0..=MAX_BLUR`, since `number_read` cannot tell that slot from a shadow's spread. `Document::apply`, the loader and `effect_is_finite` check the end as they check the radius.
- **Turning even keeps the larger radius** (`EffectKind::with_progressive`), and `Effect::with_progressive` moves the end's binding to the radius when the end was the larger, else drops it. Turning progressive starts at the radius and runs down to 0 (`BlurRamp::down`). A ramp whose end equals its start draws as an even blur (`is_progressive`).
- **Bounds take the larger end**: `escape`, `stack_escape`, `backdrop_radius` and damage. `layer_blur_deviation`, which licenses `buffer_downscale`, takes the smaller end, how blurred the sharpest part is.
- **Several background blurs compose onto one ramp** (`backdrop_blur`): the first progressive one gives the ramp its ends, direction and ease; every other joins both ends at its larger radius, since two ramps in different directions do not compose into one.

**Drawing.** `effects::ramp_plan` builds a ladder of up to `MAX_RAMP_STEPS` (8) rungs at most `RAMP_STEP_SIGMA` (3 device σ) apart, from the smaller radius to the larger, the top rung the larger's own blur. `effects::progressive_blur` blurs the layer at each rung and mixes every pixel from the two rungs either side of its own radius (`ramp_position`: its place along the ramp, held to the ends, eased). A rung past the budget is blurred on the coarse grid, as a sharpen's copy is. Between rungs it is a mix of two blurs, not the blur of the radius between: an approximation both backends make alike. The GPU spells it `Pass::Ramp` (`fx.wgsl`'s `ramp`, its `ramp_position` written operation for operation as the CPU's), the plan in `Params::rp`, appended so the earlier fields keep their offsets (`PARAMS_BYTES` 272). `tests/fx_gpu.rs · a_progressive_blur_matches_the_reference` holds it to the CPU within one premultiplied level.

### Glass

**A glass is one effect**, not a background blur, an inner shadow and a stroke built up: people should not have to assemble glass from three parts. **No presets**: the defaults read as glass on a busy picture, and an effects style keeps a tuned one. A layer may carry more than one; a second works on what the first left. `Glass` reads through `GlassWire` (every field optional, the fractions held to `0..=1`, the lengths to `0..=MAX_BLUR`, a non-finite angle to the default), as `Noise` reads through `NoiseWire`.

- **Back to front**: the backdrop blurred, bent at the outline (with dispersion), veiled white at the frost; then the layer's own fill, children and a frame's border; then the rim, at the layer's opacity (`scene::paint_rims`). A translucent fill lets the backdrop show.
- **Its blur binds in `EFFECT_BLUR`.** A retype carries a blur's or shadow's radius into the glass's blur and back (a progressive blur's larger); from a sharpen, noise, texture or filters it starts at the glass's defaults. It escapes nothing; its reading reach (`reach(blur) + depth`) joins `backdrop_reach` and `buffer_escape`. Level of detail never drops it: a rim under a pixel still reads as the glass's edge.
- **The rim's light angle** is in degrees counter-clockwise from the layer's right, up positive: 90 above, 180 left, 135 (the default) top-left. This is not the convention of the ramp's and a gradient's angle field, which runs clockwise (90 top to bottom). The light turns with the layer (`effects::rim_light`).

**Drawing.** `scene::backdrop_chain` turns the stack into the backdrop's passes in list order: each run of background blurs as one layer blur (`backdrop_blur`), each glass as itself. With no glass this is the single blur a background blur always drew, through `push_effect_layer`, byte for byte. With one, `ScenePainter::push_glass_layer` takes a buffer twice as tall (`effects::glass_buffer`): the backdrop in the top half, the outline filled opaque in the bottom, only the top drawn back. `effects::run_glass` is the specification; `fx.wgsl`'s `glass_bend` and `glass_rim` (`fx_gpu`'s glass item) the port, held within 2/255 by `gpu_effects::both_backends_draw_the_same_glass`.

- **Bend**: the outline's alpha blurred to fade out `depth` in (`glass_field_radius`) gives an edge field and its outward normal. Each pixel reads the blurred backdrop moved outward by `bend × near × depth`, bilinearly, `near` 1 at the outline and 0 a depth in; red reads `dispersion/2` further and blue as much less. Then the frost: `v·(1 − frost) + frost`.
- **Rim**: the outline blurred by the rim's width; a band full to ¾ of the width in (`Φ(1.5)`) and gone by 1¼ (`Φ(2.5)`, `RIM_GONE`, `RIM_SPAN`), weighted by `(n·L)²`, so it is brightest where the edge faces the light and on the opposite side, and fades where it runs along it.
- **Beside a glass, a progressive background blur is drawn even at its larger radius** (`backdrop_chain`): `run_glass` and its GPU twin blur evenly. Without a glass the ramp is drawn whole.

### Tests

CPU pixel tests in `mastik-render/tests/effects.rs` (shadow colour/position, alpha falloff, blur past the layer edge, filters change colour not extent, spread, inner shadow within the ink and on all four edges at (0,0), a sharpen's rim and no rim where the target cuts it, no-ink stack equals no stack). The SVG suite reads the filter graph. Neither would notice if only the other were right; for noise, `noise_reference.rs` compares the two pictures.

## Traps

- `escape` (where ink lands) and the evaluation extent (where passes run) differ for inner shadows, sharpens, textures and glasses; do not unify them. A sharpen's, texture's or glass's reading reach is `buffer_escape`, for the buffer only, never the ink bounds.
- `run_glass` and `fx_gpu`'s glass item read a backdrop blur's `radius` and ignore its ramp; `backdrop_chain` evening a progressive run beside a glass at its larger radius is what keeps that right. Drop the evening and a glass beside a progressive blur silently draws the smaller start radius, on both backends alike, so no backend comparison catches it; `scene::tests::a_ramp_beside_a_glass_is_blurred_at_its_larger_radius` pins it.
- `stack_escape` and the staging in `run` are one decision in four places (`run`, `fx_gpu`, `svg::effect_def`, `stack_escape`). A texture runs first though its row may sit anywhere; folding it into the list-order loop compiles and moves every shadow back to the smooth edge (`effects.rs · a_texture_runs_first_wherever_its_row_sits`).
- Compositing order of the complement: any new pass between silhouette and tint must commute with `255 - v`.
- Do not store the blur deviation; store the radius.
- A lone alpha door (`paint::alpha_of`) is the invariant, not "no opacity field".
- `filter_matrix` coefficient order is shared with `fx.wgsl`.
- The noise generator follows `feTurbulence` as SVG renderers run it, not as it would be written fresh: the 8-bit premultiply round trip, sampling at pixel corners and `NOISE_FREQUENCY` below 1 all look removable and each moves the picture off the exported file (*Noise*).
