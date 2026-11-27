//! A live brush in the writers that cannot draw one live: SVG (and PDF, which
//! follows it) write the bake in the layer's place.

use mastik_core::apply_brush::{Art, Mode};
use mastik_core::kurbo::{Affine, BezPath, Size};
use mastik_core::live_brush::BrushLink;
use mastik_core::peniko::Color;
use mastik_core::{
    Brush, Document, Fill, IdSource, NodeKind, Operation, Resolved, Stroke, Transaction,
    keyed_by_position,
};

/// The SVG of the brushed path alone carries the brush's green, drawn along it,
/// and not the path's own stroke; the document is left as it was.
/// flip: `svg_framed` without `live_brush::baked_view` → fails at "the brush's
/// paint is written".
#[test]
fn the_svg_of_a_brushed_path_writes_its_bake() {
    let mut ids = IdSource::new(0xBA);
    let root = ids.mint();
    let mut doc = Document::new(root);
    let (path, leaf) = (ids.mint(), ids.mint());
    let mut line = BezPath::new();
    line.move_to((0.0, 100.0));
    line.line_to((400.0, 100.0));
    let stroke = Stroke {
        brush: Brush::Solid(Color::from_rgb8(0xd0, 0x20, 0x20)).into(),
        ..Stroke::default()
    };
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
            name: Some("Vine".into()),
        },
        Operation::SetStrokes {
            id: path,
            strokes: keyed_by_position([stroke]),
        },
        Operation::CreateNode {
            id: leaf,
            parent: root,
            index: 1,
            kind: NodeKind::Ellipse {
                size: Size::new(80.0, 20.0),
                arc: Default::default(),
            },
            transform: Some(Affine::translate((100.0, 250.0))),
            name: Some("Leaf".into()),
        },
        Operation::SetFills {
            id: leaf,
            fills: keyed_by_position([Fill {
                brush: Brush::Solid(Color::from_rgb8(0x40, 0xa0, 0x60)).into(),
                visible: true,
            }]),
        },
        Operation::SetBrush {
            id: path,
            brush: Some(BrushLink::new(leaf, Mode::Art(Art::DEFAULT))),
        },
    ]))
    .expect("build the fixture");
    let res = Resolved::rebuild(&doc);
    let svg = mastik_export::svg::svg_of(&doc, &res, &[path]).to_lowercase();
    assert!(
        svg.contains("40a060"),
        "the brush's paint is written:\n{svg}"
    );
    assert!(!svg.contains("d02020"), "the path's stroke is not:\n{svg}");
    assert!(
        doc.get(path).and_then(|n| n.brush()).is_some(),
        "the link stays"
    );
}
