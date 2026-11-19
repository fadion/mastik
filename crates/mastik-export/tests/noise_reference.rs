//! The noise effect against the reference it is ported from: the SVG writer's
//! `feTurbulence` filter, drawn by `resvg`, an implementation of the
//! specification's own code that shares nothing with ours. The CPU backend's
//! raster of the same document must match it; `mastik-render`'s on-device suite
//! holds the GPU to the CPU (`tests/fx_gpu.rs`, `tests/gpu_effects.rs`).

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
