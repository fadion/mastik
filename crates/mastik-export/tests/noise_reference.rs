//! The noise effect against the reference it is ported from: the SVG writer's
//! `feTurbulence` filter, drawn by `resvg`, an implementation of the
//! specification's own code that shares nothing with ours. The CPU backend's
//! raster of the same document must match it; `mastik-render`'s on-device suite
//! holds the GPU to the CPU (`tests/fx_gpu.rs`, `tests/gpu_effects.rs`). The
//! sharpen is held to its `feGaussianBlur` and `feComposite` the same way, and
//! so are a leaned inner shadow and a sweep's wedges.

use mastik_core::kurbo::{Affine, RoundedRectRadii, Size};
use mastik_core::peniko::Color;
use mastik_core::{
    Brush, Document, Effect, EffectKind, Fill, IdSource, NodeId, NodeKind, Noise, Operation,
    Resolved, Transaction, keyed_by_position,
};
use mastik_render::{ImageStore, VelloCpuRenderer, Viewport};

const W: u32 = 160;
const H: u32 = 60;

/// A board holding a translucent rectangle and an opaque one, both under
/// `noise`, each on whole units so neither rasterizer antialiases an edge
/// inside the boxes read.
fn document(noise: Noise) -> (Document, Resolved, NodeId, NodeId) {
    let mut ids = IdSource::new(0x5EED);
    let root = ids.mint();
    let mut doc = Document::new(root);
    let (board, a, b) = (ids.mint(), ids.mint(), ids.mint());
    let rect = |id, at: (f64, f64)| Operation::CreateNode {
        id,
        parent: board,
        index: 0,
        kind: NodeKind::Rect {
            size: Size::new(64.0, 40.0),
            corner_radii: RoundedRectRadii::default(),
            corner_smoothing: 0.0,
        },
        transform: Some(Affine::translate(at)),
        name: None,
    };
    let solid = |id, c: Color| Operation::SetFills {
        id,
        fills: keyed_by_position([Fill {
            brush: Brush::Solid(c).into(),
            visible: true,
        }]),
    };
    let effects = |id| Operation::SetEffects {
        id,
        effects: keyed_by_position([Effect::new(EffectKind::Noise(noise))]),
    };
    doc.apply(&Transaction(vec![
        Operation::CreateNode {
            id: board,
            parent: root,
            index: 0,
            kind: NodeKind::Artboard {
                size: Size::new(f64::from(W), f64::from(H)),
                corner_radii: RoundedRectRadii::default(),
                corner_smoothing: 0.0,
            },
            transform: None,
            name: None,
        },
        rect(a, (8.0, 10.0)),
        rect(b, (88.0, 10.0)),
        Operation::SetFills {
            id: board,
            fills: keyed_by_position([]),
        },
        solid(a, Color::from_rgba8(31, 41, 82, 200)),
        solid(b, Color::from_rgba8(255, 138, 92, 255)),
        effects(a),
        effects(b),
    ]))
    .unwrap();
    let res = Resolved::rebuild(&doc);
    (doc, res, board, root)
}

/// The CPU backend's picture, premultiplied.
fn ours(doc: &Document, res: &Resolved, page: NodeId) -> Vec<u8> {
    let vp = Viewport {
        view: mastik_core::kurbo::Rect::new(0.0, 0.0, f64::from(W), f64::from(H)),
        pixel_size: (W, H),
    };
    let (px, ..) =
        VelloCpuRenderer::new().render_to_rgba(doc, res, &vp, &ImageStore::default(), page);
    px.as_chunks::<4>()
        .0
        .iter()
        .flat_map(|p| {
            let a = f32::from(p[3]) / 255.0;
            [0, 1, 2]
                .map(|c| (f32::from(p[c]) * a + 0.5) as u8)
                .into_iter()
                .chain([p[3]])
        })
        .collect()
}

/// The SVG writer's file drawn by `resvg`, premultiplied.
fn reference(svg: &str) -> Vec<u8> {
    let tree = resvg::usvg::Tree::from_str(svg, &resvg::usvg::Options::default())
        .expect("the writer's SVG parses");
    let mut pixmap = resvg::tiny_skia::Pixmap::new(W, H).unwrap();
    resvg::render(
        &tree,
        resvg::tiny_skia::Transform::identity(),
        &mut pixmap.as_mut(),
    );
    pixmap.data().to_vec()
}

/// The worst channel difference inside `(x0, y0)..(x1, y1)` and the share of
/// channels off by more than one level.
fn differ(a: &[u8], b: &[u8], (x0, y0, x1, y1): (u32, u32, u32, u32)) -> (u8, f64) {
    let (mut worst, mut over, mut n) = (0u8, 0usize, 0usize);
    for y in y0..y1 {
        for x in x0..x1 {
            for c in 0..4 {
                let i = ((y * W + x) * 4) as usize + c;
                let d = a[i].abs_diff(b[i]);
                worst = worst.max(d);
                over += usize::from(d > 1);
                n += 1;
            }
        }
    }
    (worst, over as f64 / n as f64)
}

/// The boxes' insides, a pixel in from each edge.
const TRANSLUCENT_BOX: (u32, u32, u32, u32) = (9, 11, 71, 49);
const OPAQUE_BOX: (u32, u32, u32, u32) = (89, 11, 151, 49);

/// How far the raster may stray from the reference: within two levels, past
/// one on at most 15% of channels. `resvg` rounds each composite of the mix at 8
/// bits where the backend rounds once. The share bounds what the worst cannot:
/// a shifted or wrong lattice moves most of the box by far more than a level.
const TOLERANCE: u8 = 2;
const SHARE: f64 = 0.15;

/// Two colours at Normal, cut by density; multicolour at Soft light; two
/// translucent colours stretched at Multiply. Each through the CPU backend and
/// through the SVG writer as `resvg` draws it. Separable modes only: a hue mode
/// over a translucent layer turns by many levels on `resvg`'s 8-bit straight
/// backdrop, which `tests/fx_gpu.rs` covers against the GPU instead.
///
/// flip: `Lattice::noise2`'s `b10` read as `sel(i + by1)` → fails at the first
/// case's boxes, by tens of levels over most of them.
#[test]
fn the_raster_draws_what_the_svg_writer_writes() {
    let base = Noise {
        size: [2.0, 2.0],
        seed: 9,
        ..Noise::default()
    };
    let cases = [
        Noise {
            density: 0.4,
            colors: [Color::from_rgba8(10, 20, 30, 255), Color::WHITE],
            ..base
        },
        Noise {
            multi: true,
            opacity: 0.7,
            blend: mastik_core::BlendMode::SoftLight,
            ..base
        },
        Noise {
            size: [1.0, 5.0],
            colors: [
                Color::from_rgba8(200, 20, 120, 180),
                Color::from_rgba8(20, 200, 220, 60),
            ],
            blend: mastik_core::BlendMode::Multiply,
            ..base
        },
    ];
    for noise in cases {
        let (doc, res, board, page) = document(noise);
        let svg = mastik_export::svg::svg(&doc, &res, page, Some(board));
        assert!(
            svg.contains("<feTurbulence"),
            "the writer writes the noise as feTurbulence:\n{svg}"
        );
        let (a, b) = (ours(&doc, &res, page), reference(&svg));
        let lit = (OPAQUE_BOX.0..OPAQUE_BOX.2)
            // Green: the orange's red is at its top, where Soft light leaves it.
            .map(|x| a[((30 * W + x) * 4 + 1) as usize])
            .collect::<Vec<_>>();
        assert!(
            lit.iter().max().unwrap() - lit.iter().min().unwrap() > 20,
            "the fixture draws noise, not a flat fill: {lit:?}"
        );
        for (name, area) in [("translucent", TRANSLUCENT_BOX), ("opaque", OPAQUE_BOX)] {
            let (worst, share) = differ(&a, &b, area);
            assert!(
                worst <= TOLERANCE && share <= SHARE,
                "{noise:?}: the {name} box differs from the reference by up to {worst}, \
                 {:.1}% of channels past one level",
                share * 100.0
            );
        }
    }
}

/// A board holding one opaque 40×24 rectangle placed by `transform`, carrying
/// `effects`.
fn single(transform: Affine, effects: Vec<Effect>) -> (Document, Resolved, NodeId, NodeId) {
    let mut ids = IdSource::new(0x5EED);
    let root = ids.mint();
    let mut doc = Document::new(root);
    let (board, a) = (ids.mint(), ids.mint());
    doc.apply(&Transaction(vec![
        Operation::CreateNode {
            id: board,
            parent: root,
            index: 0,
            kind: NodeKind::Artboard {
                size: Size::new(f64::from(W), f64::from(H)),
                corner_radii: RoundedRectRadii::default(),
                corner_smoothing: 0.0,
            },
            transform: None,
            name: None,
        },
        Operation::SetFills {
            id: board,
            fills: keyed_by_position([]),
        },
        Operation::CreateNode {
            id: a,
            parent: board,
            index: 0,
            kind: NodeKind::Rect {
                size: Size::new(40.0, 24.0),
                corner_radii: RoundedRectRadii::default(),
                corner_smoothing: 0.0,
            },
            transform: Some(transform),
            name: None,
        },
        Operation::SetFills {
            id: a,
            fills: keyed_by_position([Fill {
                brush: Brush::Solid(Color::from_rgba8(255, 138, 92, 255)).into(),
                visible: true,
            }]),
        },
        Operation::SetEffects {
            id: a,
            effects: keyed_by_position(effects),
        },
    ]))
    .unwrap();
    let res = Resolved::rebuild(&doc);
    (doc, res, board, root)
}

/// The CPU raster of `single` and the writer's file of it.
fn raster(transform: Affine, effects: Vec<Effect>) -> (Vec<u8>, String) {
    let (doc, res, board, page) = single(transform, effects);
    let svg = mastik_export::svg::svg(&doc, &res, page, Some(board));
    (ours(&doc, &res, page), svg)
}

fn noise() -> Effect {
    Effect::new(EffectKind::Noise(Noise {
        size: [2.0, 2.0],
        seed: 9,
        density: 0.4,
        colors: [Color::from_rgba8(10, 20, 30, 255), Color::WHITE],
        ..Noise::default()
    }))
}

fn shadow() -> Effect {
    Effect::new(EffectKind::DropShadow(mastik_core::Shadow {
        offset: mastik_core::kurbo::Vec2::new(6.0, 4.0),
        blur: 0.0,
        spread: 0.0,
        color: Color::from_rgba8(0, 0, 255, 255),
        blend: mastik_core::BlendMode::Normal,
    }))
}

/// `resvg` runs a filter whose user space is turned or mirrored in an unturned
/// one (Chromium turns it, as the specification says), so it is no reference for
/// the writer's layer-space filter. The raster is held to itself instead: noise
/// lives in the layer's space, so a turned or mirrored layer draws the unturned
/// layer's noise turned or mirrored, pixel for pixel. The writer's half is its
/// wrapper, which must carry the layer's whole transform.
/// flip: `effects::buffer_to_local` keeping only the translation → fails at "mirrored".
#[test]
fn a_turned_or_mirrored_noise_is_the_unturned_noise_turned() {
    let (plain, _) = raster(Affine::translate((8.0, 10.0)), vec![noise()]);
    // Noise is read at pixel corners (`LatticeMap::new`), so the corner each pixel
    // shares with the plain one's names the pair.
    // x 110..150, y 10..34: pixel (x, y) reads the plain one's (158 − x, y).
    let mirrored = Affine::translate((150.0, 10.0)) * Affine::scale_non_uniform(-1.0, 1.0);
    // x 46..70, y 10..50: pixel (x, y) reads the plain one's (y − 2, 80 − x).
    let turned = Affine::translate((70.0, 10.0)) * Affine::rotate(std::f64::consts::FRAC_PI_2);
    type Map = fn(u32, u32) -> (u32, u32);
    type Case<'a> = (&'a str, Affine, Map, (u32, u32, u32, u32), &'a str);
    let cases: [Case<'_>; 2] = [
        (
            "mirrored",
            mirrored,
            |x, y| (158 - x, y),
            (111, 11, 149, 33),
            r#"<g transform="matrix(-1 0 0 1 150 10)"><g filter="#,
        ),
        (
            "turned",
            turned,
            |x, y| (y - 2, 80 - x),
            (47, 11, 69, 49),
            r#"<g transform="matrix(0 1 -1 0 70 10)"><g filter="#,
        ),
    ];
    for (name, t, back, (x0, y0, x1, y1), wrapper) in cases {
        let (px, svg) = raster(t, vec![noise()]);
        assert!(
            svg.contains(wrapper),
            "{name}: the wrapper is the layer's transform:\n{svg}"
        );
        let mut worst = 0u8;
        for y in y0..y1 {
            for x in x0..x1 {
                let (u, v) = back(x, y);
                let (i, j) = (((y * W + x) * 4) as usize, ((v * W + u) * 4) as usize);
                for c in 0..4 {
                    worst = worst.max(px[i + c].abs_diff(plain[j + c]));
                }
            }
        }
        assert!(
            worst <= TOLERANCE,
            "{name}: off the plain noise by up to {worst}"
        );
    }
}

/// A shadow on a mirrored layer falls where the canvas casts it: the offset
/// (6, 4) mirrored points left. Without a noise the filter is in the parent's
/// unturned space and `resvg` draws it, read in the strip left of the rectangle;
/// with one, the filter is in the layer's space under the wrapper and writes the
/// offset as authored.
/// flip: `effect_def` writing the offset unturned → fails at "no noise".
#[test]
fn a_mirrored_shadow_falls_where_the_canvas_casts_it() {
    // x 110..150, y 10..34; the shadow x 104..144, y 14..38.
    let mirrored = Affine::translate((150.0, 10.0)) * Affine::scale_non_uniform(-1.0, 1.0);
    let (a, svg) = raster(mirrored, vec![shadow()]);
    let b = reference(&svg);
    let i = ((20 * W + 106) * 4) as usize;
    assert_eq!(
        &a[i..i + 4],
        &[0, 0, 255, 255],
        "the canvas casts the strip"
    );
    let (worst, share) = differ(&a, &b, (105, 15, 109, 37));
    assert!(
        worst <= TOLERANCE && share <= SHARE,
        "no noise: the shadow differs from the reference by up to {worst}, \
         {:.1}% past one level",
        share * 100.0
    );
    let (a, svg) = raster(mirrored, vec![noise(), shadow()]);
    assert_eq!(
        &a[i..i + 4],
        &[0, 0, 255, 255],
        "noise: the canvas casts the strip"
    );
    assert!(
        svg.contains(r#"<g transform="matrix(-1 0 0 1 150 10)"><g filter="#)
            && svg.contains(r#"dx="6" dy="4""#),
        "noise: the offset as authored, under the layer's transform:\n{svg}"
    );
}

/// An opaque sweep's wedges leave no seam where they are narrowest, near the
/// centre: every pixel inside the rectangle, which lies within 24 of it, is
/// opaque in `resvg`, the seam's row included.
/// flip: `sweep_def` reaching under the next wedge by `WEDGE_OVERLAP` alone →
/// fails at "a seam", on most of the rectangle; without the first wedge's band
/// behind its first edge → on the row above the seam.
#[test]
fn a_sweep_leaves_no_seam_near_its_centre() {
    // The rectangle at x 60..100, y 18..42, the sweep's centre at (80, 30).
    let (mut doc, _, board, page) = single(Affine::translate((60.0, 18.0)), vec![]);
    let rect = doc
        .node_ids()
        .find(|id| matches!(doc.get(*id).unwrap().kind(), NodeKind::Rect { .. }))
        .expect("the rectangle");
    let sweep = mastik_core::peniko::Gradient::new_sweep((20.0, 12.0), 0.0, std::f32::consts::TAU)
        .with_stops([
            (0.0_f32, Color::from_rgba8(255, 0, 0, 255)),
            (1.0_f32, Color::from_rgba8(0, 0, 255, 255)),
        ]);
    doc.apply(&Transaction(vec![Operation::SetFills {
        id: rect,
        fills: keyed_by_position([Fill {
            brush: Brush::Gradient(sweep.into()).into(),
            visible: true,
        }]),
    }]))
    .unwrap();
    let res = Resolved::rebuild(&doc);
    let svg = mastik_export::svg::svg(&doc, &res, page, Some(board));
    assert!(svg.contains("<pattern"), "the fixture: a sweep:\n{svg}");
    let px = reference(&svg);
    let mut thin = Vec::new();
    for y in 18..42 {
        for x in 60..100 {
            let a = px[((y * W + x) * 4 + 3) as usize];
            if a < 250 {
                thin.push((x, y, a));
            }
        }
    }
    assert!(thin.is_empty(), "a seam: {} pixels, {thin:?}", thin.len());
}

/// An inner shadow on a layer leaned past 45° is whole in the SVG: its offset is
/// written stretched by the lean, (0, 10) as (20, 10), and the filter region is
/// padded by that, not by the layer-space offset. Counted as pixels more than 40
/// levels off, which the slanted edges' anti-aliasing stays under.
/// flip: `working_pad` padding by the layer-space offset → fails at "leaned", on
/// the wedge at the top-left corner.
#[test]
fn a_leaned_inner_shadow_is_whole_in_the_svg() {
    let inner = Effect::new(EffectKind::InnerShadow(mastik_core::Shadow {
        offset: mastik_core::kurbo::Vec2::new(0.0, 10.0),
        blur: 0.0,
        spread: 0.0,
        color: Color::from_rgba8(255, 0, 0, 255),
        blend: mastik_core::BlendMode::Normal,
    }));
    let far_off = |a: &[u8], b: &[u8], (x0, y0, x1, y1): (u32, u32, u32, u32)| {
        let mut n = 0;
        for y in y0..y1 {
            for x in x0..x1 {
                let i = ((y * W + x) * 4) as usize;
                n += usize::from((0..4).any(|c| a[i + c].abs_diff(b[i + c]) > 40));
            }
        }
        n
    };
    // The plain rectangle at x 40..80, y 10..34; leaned, it reaches x 128.
    let plain = Affine::translate((40.0, 10.0));
    let (a, svg) = raster(plain, vec![inner.clone()]);
    assert_eq!(far_off(&a, &reference(&svg), (40, 10, 80, 34)), 0, "plain");
    let leaned = plain * Affine::skew(2.0, 0.0);
    let (a, svg) = raster(leaned, vec![inner]);
    assert!(
        svg.contains(r#"dx="20" dy="10""#),
        "the offset leaned:\n{svg}"
    );
    let i = ((11 * W + 45) * 4) as usize;
    assert_eq!(
        &a[i..i + 4],
        &[255, 0, 0, 255],
        "the canvas casts the top-left corner"
    );
    assert_eq!(
        far_off(&a, &reference(&svg), (40, 10, 128, 34)),
        0,
        "leaned"
    );
}

/// How far a sharpen may stray from the reference: `resvg`'s three boxes miss a
/// gaussian's tail by a level or so, and a sharpen of `amount` multiplies that by
/// `1 + amount`.
const SHARPEN_TOLERANCE: u8 = 5;

/// A sharpen draws what the writer's blur and arithmetic composite draw, across
/// the rectangle's edges where it rings. Only deviations `resvg` blurs by boxes of
/// an odd width (2.5 and 3.5): below 2 its IIR blur strays from a gaussian by
/// several levels on its own, and an even box shifts by half a pixel. The layer's
/// alpha clamps to 1 inside and 0 outside, so the band also holds the clamp order.
/// flip: `working_pad` without its sharpen arm → fails at "differs", by tens of
/// levels on the edge pixels.
#[test]
fn a_sharpen_draws_what_the_svg_writer_writes() {
    // The 40×24 rectangle at x 8..48, y 10..34, and the band round it.
    let band = (2, 4, 54, 40);
    for radius in [5.0, 7.0] {
        let (a, svg) = raster(
            Affine::translate((8.0, 10.0)),
            vec![Effect::new(EffectKind::Sharpen {
                amount: 1.5,
                radius,
            })],
        );
        assert!(
            svg.contains(r#"operator="arithmetic" k1="0" k2="2.5" k3="-1.5""#),
            "radius {radius}: the writer writes the sharpen as a composite:\n{svg}"
        );
        // Green just inside the left edge and mid-box: the edge overshoots.
        let green = |x: u32| a[((20 * W + x) * 4 + 1) as usize];
        assert!(
            green(8) > green(28) + 40,
            "radius {radius}: the fixture rings: {} against {}",
            green(8),
            green(28)
        );
        let (worst, share) = differ(&a, &reference(&svg), band);
        assert!(
            worst <= SHARPEN_TOLERANCE && share <= SHARE,
            "radius {radius}: the sharpen differs from the reference by up to {worst}, \
             {:.1}% of channels past one level",
            share * 100.0
        );
    }

    // Translucent, dark then light across a hard stop at x 28: the light side's
    // overshoot passes its alpha but not 1, so only a clamp to the alpha holds it.
    let (mut doc, _, board, page) = single(
        Affine::translate((8.0, 10.0)),
        vec![Effect::new(EffectKind::Sharpen {
            amount: 1.5,
            radius: 5.0,
        })],
    );
    let rect = doc
        .node_ids()
        .find(|id| matches!(doc.get(*id).unwrap().kind(), NodeKind::Rect { .. }))
        .expect("the rectangle");
    let (dark, light) = (
        Color::from_rgba8(40, 40, 40, 150),
        Color::from_rgba8(200, 200, 200, 150),
    );
    let two_tone = mastik_core::peniko::Gradient::new_linear((0.0, 0.0), (40.0, 0.0)).with_stops([
        (0.0, dark),
        (0.5, dark),
        (0.5, light),
        (1.0, light),
    ]);
    doc.apply(&Transaction(vec![
        Operation::SetFills {
            id: rect,
            fills: keyed_by_position([Fill {
                brush: Brush::Gradient(two_tone.into()).into(),
                visible: true,
            }]),
        },
        // Over a ground, where a colour past its alpha would add light.
        Operation::SetFills {
            id: board,
            fills: keyed_by_position([Fill {
                brush: Brush::Solid(Color::from_rgb8(90, 110, 130)).into(),
                visible: true,
            }]),
        },
    ]))
    .unwrap();
    let res = Resolved::rebuild(&doc);
    let svg = mastik_export::svg::svg(&doc, &res, page, Some(board));
    let a = ours(&doc, &res, page);
    let px = |x: u32| &a[((20 * W + x) * 4) as usize..((20 * W + x) * 4 + 4) as usize];
    assert!(
        px(28)[0] > px(40)[0] + 20,
        "the fixture: the light side overshoots: {:?} against {:?}",
        px(28),
        px(40)
    );
    let (worst, share) = differ(&a, &reference(&svg), band);
    assert!(
        worst <= SHARPEN_TOLERANCE && share <= SHARE,
        "two-tone: the sharpen differs from the reference by up to {worst}, \
         {:.1}% of channels past one level",
        share * 100.0
    );
}
