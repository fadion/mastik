//! Markers (`mastik_core::marker`): a stroke's heads are hit, bound and outlined
//! as the ink they draw.

use mastik_core::kurbo::{Point, Shape};
use mastik_core::{
    Document, IdSource, Marker, NodeId, NodeKind, Operation, Resolved, Stroke, Transaction,
    keyed_by_position,
};

/// The stroke's width; at 100% an arrow is three widths long and three across.
const W: f64 = 4.0;

/// A page holding a 100-long horizontal line stroked `W` wide, the stroke's end
/// wearing `end`.
fn line(end: Marker) -> (Document, IdSource, NodeId, NodeId) {
    let mut ids = IdSource::new(0x3A7);
    let page = ids.mint();
    let mut doc = Document::new(page);
    let id = ids.mint();
    doc.apply(&Transaction(vec![
        Operation::CreateNode {
            id,
            parent: page,
            index: 0,
            kind: NodeKind::Line {
                end: Point::new(100.0, 0.0),
            },
            transform: None,
            name: None,
        },
        Operation::SetStrokes {
            id,
            strokes: keyed_by_position([Stroke {
                width: W,
                end,
                ..Default::default()
            }]),
        },
    ]))
    .unwrap();
    (doc, ids, page, id)
}

/// A point on an arrowhead's wing, past the stroke's own band, hits the line.
/// flip: `geometry::contains_local` without its `markers_hit` → fails at "the
/// wing".
#[test]
fn a_click_on_the_head_hits_the_line() {
    // Eight back from the tip the head is four either side; the stroke is two.
    let wing = Point::new(92.0, 3.0);
    let (doc, _, page, id) = line(Marker::Arrow);
    let res = Resolved::rebuild(&doc);
    assert_eq!(
        mastik_core::hit_test(&res, &doc, page, wing, 0.0),
        vec![id],
        "the wing"
    );
    let (doc, _, page, _) = line(Marker::None);
    let res = Resolved::rebuild(&doc);
    assert!(
        mastik_core::hit_test(&res, &doc, page, wing, 0.0).is_empty(),
        "an unmarked line is not that wide"
    );
}

/// The ink box holds the head: a width and a half either side of the line,
/// where the stroke alone reaches half a width.
/// flip: `geometry::world_ink_of_parts` without its `marked_bounds` → fails at
/// "the head's reach".
#[test]
fn the_ink_box_holds_the_head() {
    let (doc, _, _, id) = line(Marker::Arrow);
    let res = Resolved::rebuild(&doc);
    let ink = res.ink_bounds(id).expect("ink");
    assert!((ink.y1 - 1.5 * W).abs() < 1e-6, "the head's reach: {ink:?}");
    assert!(
        ink.x1 <= 100.0 + W / 2.0 + 1e-6,
        "the tip is the end: {ink:?}"
    );
}

/// *Outline stroke* unites the head with the shaft: one path, the head's width
/// across, its tip the line's end.
/// flip: `build::stroke_ink` without its heads → fails at "the head is in the ink".
#[test]
fn outline_stroke_takes_the_head() {
    let (mut doc, mut ids, _, id) = line(Marker::Arrow);
    let res = Resolved::rebuild(&doc);
    let (tx, made) = mastik_core::build::outline_stroke(&doc, &res, &mut ids, id).unwrap();
    doc.apply(&tx).unwrap();
    let node = doc.get(made).expect("the outline");
    let NodeKind::Path { path, .. } = node.kind() else {
        panic!("{:?}", node.kind());
    };
    let b = path.bounding_box();
    assert!(
        (b.height() - 3.0 * W).abs() < 0.1,
        "the head is in the ink: {b:?}"
    );
    assert!((b.x1 - 100.0).abs() < 0.1, "the tip is the end: {b:?}");
    assert!(
        node.paint().strokes.is_empty(),
        "the outline is a fill, no stroke"
    );
}
