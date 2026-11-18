//! A noise fill and a grain against the reference they are ported from: the
//! SVG writer's `feTurbulence` filters, drawn by `resvg`, an implementation of the
//! specification's own code that shares nothing with ours. The CPU backend's
//! raster of the same document must match it; `mastik-render`'s on-device suite
//! holds the GPU to the CPU (`tests/fx_gpu.rs`, `tests/gpu_effects.rs`).

use mastik_core::kurbo::{Affine, RoundedRectRadii, Size};
use mastik_core::peniko::Color;
use mastik_core::{
    Brush, Document, Effect, EffectKind, Fill, GradientBrush, Grain, IdSource, NodeId, NodeKind,
    Noise, NoiseKind, Operation, Resolved, Transaction, keyed_by_position,
};
use mastik_render::{ImageStore, VelloCpuRenderer, Viewport};

const W: u32 = 160;
const H: u32 = 60;

/// A board holding a noise-filled rectangle and a grained solid one, each on
/// whole units so neither rasterizer antialiases an edge inside the boxes read.
fn document(noise: Noise, grain: Grain) -> (Document, Resolved, NodeId, NodeId) {
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
        Operation::SetFills {
            id: a,
            fills: keyed_by_position([Fill {
                brush: Brush::Gradient(GradientBrush::noise(
                    noise,
                    Color::from_rgba8(223, 230, 255, 255),
                    Color::from_rgba8(31, 41, 82, 200),
                ))
                .into(),
                visible: true,
            }]),
        },
        Operation::SetFills {
            id: b,
            fills: keyed_by_position([Fill {
                brush: Brush::Solid(Color::from_rgba8(255, 138, 92, 255)).into(),
                visible: true,
            }]),
        },
        Operation::SetEffects {
            id: b,
            effects: keyed_by_position([Effect::new(EffectKind::Grain(grain))]),
        },
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
const NOISE_BOX: (u32, u32, u32, u32) = (9, 11, 71, 49);
const GRAIN_BOX: (u32, u32, u32, u32) = (89, 11, 151, 49);

/// How far the raster may stray from the reference, per box: the grain within
/// one level; the noise fill within three, past one on at most 15% of channels.
/// `resvg` rounds the fill's matrix output, its premultiply and its composite at
/// 8 bits each, where the backend rounds once, and a ramp to a translucent
/// colour stacks those three roundings. The share bounds what the worst cannot:
/// a shifted or wrong lattice moves most of the box by far more than a level.
const GRAIN_TOLERANCE: u8 = 1;
const NOISE_TOLERANCE: u8 = 3;
const SHARE: f64 = 0.15;

/// Each kind of noise, stretched, and a grain in a separable and a
/// non-separable mode, through the CPU backend and through the SVG writer as
/// `resvg` draws it.
///
/// flip: `Lattice::noise2`'s `b10` read as `sel(i + by1)` → fails at the first
/// case's noise box, by tens of levels over most of it.
#[test]
fn the_raster_draws_what_the_svg_writer_writes() {
    for (kind, stretch, blend, colour) in [
        (
            NoiseKind::FractalNoise,
            0.0,
            mastik_core::BlendMode::Overlay,
            false,
        ),
        (
            NoiseKind::Turbulence,
            0.5,
            mastik_core::BlendMode::SoftLight,
            true,
        ),
        (
            NoiseKind::FractalNoise,
            -1.0,
            mastik_core::BlendMode::Multiply,
            false,
        ),
    ] {
        let noise = Noise {
            kind,
            scale: 12.0,
            detail: 4,
            seed: 37,
            stretch,
        };
        let grain = Grain {
            amount: 0.6,
            size: 2.0,
            seed: 9,
            blend,
            colour,
        };
        let (doc, res, board, page) = document(noise, grain);
        let svg = mastik_export::svg::svg(&doc, &res, page, Some(board));
        assert!(
            svg.contains("<feTurbulence") && svg.contains(kind.svg_type()),
            "the writer writes the noise as feTurbulence:\n{svg}"
        );
        let (a, b) = (ours(&doc, &res, page), reference(&svg));
        let lit = (NOISE_BOX.1..NOISE_BOX.3)
            .map(|y| a[((y * W + 30) * 4) as usize])
            .collect::<Vec<_>>();
        assert!(
            lit.iter().max().unwrap() - lit.iter().min().unwrap() > 20,
            "the fixture draws noise, not a flat fill: {lit:?}"
        );
        for (name, area, tolerance) in [
            ("noise", NOISE_BOX, NOISE_TOLERANCE),
            ("grain", GRAIN_BOX, GRAIN_TOLERANCE),
        ] {
            let (worst, share) = differ(&a, &b, area);
            assert!(
                worst <= tolerance && share <= SHARE,
                "{kind:?}, {blend:?}: the {name} box differs from the reference by up \
                 to {worst}, {:.1}% of channels past one level",
                share * 100.0
            );
        }
    }
}
