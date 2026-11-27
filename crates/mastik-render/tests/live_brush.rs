//! A live brush through the scene walk: the brush's ink drawn in place of the
//! path's strokes, committed and under a preview that writes the link.

use mastik_core::Brush;
use mastik_core::apply_brush::{Mode, Scatter};
use mastik_core::kurbo::{Affine, BezPath, Rect, Size};
use mastik_core::live_brush::BrushLink;
use mastik_core::peniko::Color;
use mastik_core::{
    Document, Fill, IdSource, NodeId, NodeKind, Operation, Resolved, Stroke, Transaction,
    keyed_by_position,
};
use mastik_render::scene::{self, ClipRule, ScenePainter, StrokePaint, TextRun};
use mastik_render::{RenderOverrides, Viewport};

const GREEN: Color = Color::from_rgb8(0x40, 0xa0, 0x60);

/// Counts the green fills (the brush's) and every stroke laid.
#[derive(Default)]
struct Counter {
    green: usize,
    strokes: usize,
}

impl ScenePainter for Counter {
    fn has_image(&self, _id: &mastik_core::ImageId) -> bool {
        true
    }
    fn fill_path(
        &mut self,
        _transform: Affine,
        _path: &BezPath,
        brush: &Brush,
        _framing: Option<mastik_core::Framing>,
        _rule: ClipRule,
    ) {
        if matches!(brush, Brush::Solid(c) if *c == GREEN) {
            self.green += 1;
        }
    }
    fn stroke_path(&mut self, _transform: Affine, _path: &BezPath, _stroke: &StrokePaint<'_>) {
        self.strokes += 1;
    }
    fn draw_text(&mut self, _transform: Affine, _run: &TextRun<'_>) {}
    fn push_layer(&mut self, _t: Affine, _clip: Option<&BezPath>, _rule: ClipRule, _o: f32) {}
    fn push_mask_layer(&mut self) {}
    fn pop_layer(&mut self) {}
}

/// A stroked curve from (0, 100) to (400, 100) and a green 40 × 20 ellipse. A
/// curve, since a straight stroke snaps to the device and is laid as fills.
fn fixture() -> (Document, NodeId, NodeId) {
    let mut ids = IdSource::new(0xB9);
    let root = ids.mint();
    let mut doc = Document::new(root);
    let (path, leaf) = (ids.mint(), ids.mint());
    let mut line = BezPath::new();
    line.move_to((0.0, 100.0));
    line.curve_to((130.0, 40.0), (270.0, 160.0), (400.0, 100.0));
    doc.apply(&Transaction(vec![
        Operation::CreateNode {
            id: path,
            parent: root,
            index: 0,
            kind: NodeKind::Path {
                path: line,
                corner_radii: Vec::new(),
                corner_smoothing: 0.0,
                mirror: Default::default(),
            },
            transform: None,
            name: None,
        },
        Operation::SetStrokes {
            id: path,
            strokes: keyed_by_position([Stroke::default()]),
        },
        Operation::CreateNode {
            id: leaf,
            parent: root,
            index: 1,
            kind: NodeKind::Ellipse {
                size: Size::new(40.0, 20.0),
                arc: Default::default(),
            },
            transform: Some(Affine::translate((100.0, 250.0))),
            name: Some("Leaf".into()),
        },
        Operation::SetFills {
            id: leaf,
            fills: keyed_by_position([Fill {
                brush: Brush::Solid(GREEN).into(),
                visible: true,
            }]),
        },
    ]))
    .expect("build the fixture");
    (doc, path, leaf)
}

fn link(path: NodeId, leaf: NodeId) -> Operation {
    let mode = Mode::Scatter(Scatter {
        size_varies: 0.0,
        scatter: 0.0,
        ..Scatter::DEFAULT
    });
    Operation::SetBrush {
        id: path,
        brush: Some(BrushLink::new(leaf, mode)),
    }
}

fn drawn(doc: &Document, res: &Resolved, ov: &RenderOverrides) -> Counter {
    let vp = Viewport {
        view: Rect::new(-50.0, -50.0, 550.0, 550.0),
        pixel_size: (600, 600),
    };
    let mut c = Counter::default();
    scene::build(doc, res, &vp, ov, doc.first_page(), &mut c);
    c
}

/// Linked, the path draws a copy of the leaf per place and no stroke; the leaf
/// itself still draws where it is.
/// flip: `paint_node_pass` keeping the strokes of a brushed layer → fails at
/// "the brush draws in place of the stroke".
#[test]
fn a_brushed_path_draws_the_brush_not_its_stroke() {
    let (mut doc, path, leaf) = fixture();
    let res = Resolved::rebuild(&doc);
    let before = drawn(&doc, &res, &RenderOverrides::default());
    assert_eq!(
        (before.green, before.strokes),
        (1, 1),
        "the leaf and the stroke"
    );
    doc.apply(&Transaction(vec![link(path, leaf)])).unwrap();
    let res = Resolved::rebuild(&doc);
    let copies = res.brush_ink(path).map(|i| i.places.len()).expect("ink");
    assert!(copies > 2, "{copies}");
    let after = drawn(&doc, &res, &RenderOverrides::default());
    assert_eq!(after.strokes, 0, "the brush draws in place of the stroke");
    assert_eq!(after.green, copies + 1, "a copy per place, and the leaf");
}

/// A preview writing the link draws what the commit will, from the preview's
/// own geometry.
/// flip: `from_transaction_following` without `rebrush` → fails at "the
/// preview draws the brush".
#[test]
fn a_previewed_link_draws_as_committed() {
    let (doc, path, leaf) = fixture();
    let res = Resolved::rebuild(&doc);
    let tx = Transaction(vec![link(path, leaf)]);
    let ov = RenderOverrides::from_transaction(&doc, &res, &tx).expect("previewable");
    let seen = drawn(&doc, &res, &ov);
    let mut committed = doc.clone();
    committed.apply(&tx).unwrap();
    let res2 = Resolved::rebuild(&committed);
    let want = drawn(&committed, &res2, &RenderOverrides::default());
    assert_eq!(seen.green, want.green, "the preview draws the brush");
    assert_eq!(seen.strokes, 0);
}

/// Outline mode traces the path, not the ink.
#[test]
fn outline_mode_traces_the_path_not_the_ink() {
    let (mut doc, path, leaf) = fixture();
    doc.apply(&Transaction(vec![link(path, leaf)])).unwrap();
    let res = Resolved::rebuild(&doc);
    let mut ov = RenderOverrides::default();
    ov.set_outline(Some(Color::BLACK));
    let c = drawn(&doc, &res, &ov);
    // Outline draws every outline as a hairline stroke: the path and the leaf.
    assert_eq!(c.strokes, 2, "the path and the leaf, no copies");
}

/// A brush that draws nothing (here, more copies than a scatter may make) still
/// takes the strokes' place, and a preview moving the path keeps it so.
/// flip: `RenderOverrides::rebrush` answering the layer's own strokes for a link
/// with no ink → fails at "the preview keeps the strokes hidden".
#[test]
fn a_brush_that_draws_nothing_keeps_the_strokes_hidden_in_a_preview() {
    let (mut doc, path, leaf) = fixture();
    let crowded = Mode::Scatter(Scatter {
        spacing: 0.0,
        ..Scatter::DEFAULT
    });
    doc.apply(&Transaction(vec![
        Operation::SetGeometry {
            id: leaf,
            geometry: mastik_core::GeometryPatch::Size(Size::new(4.0, 20.0)),
        },
        Operation::SetBrush {
            id: path,
            brush: Some(BrushLink::new(leaf, crowded)),
        },
    ]))
    .unwrap();
    let res = Resolved::rebuild(&doc);
    assert!(res.brush_ink(path).is_none(), "too many copies: no ink");
    assert_eq!(drawn(&doc, &res, &RenderOverrides::default()).strokes, 0);
    let tx = Transaction(vec![Operation::SetTransform {
        id: path,
        transform: Affine::translate((5.0, 0.0)),
    }]);
    let ov = RenderOverrides::from_transaction(&doc, &res, &tx).expect("previewable");
    assert_eq!(
        drawn(&doc, &res, &ov).strokes,
        0,
        "the preview keeps the strokes hidden"
    );
}
