//! *Apply brush* (`apply_brush.rs`): a shape bent or repeated along a path, baked.
//!
//! The fixture's path and brush sit in a frame translated by (50, 30), so the
//! world space the brush is bent in is never the parent's.

use mastik_core::apply_brush::{self, ApplyBrush, Art, Mode, Refusal, Scatter};
use mastik_core::kurbo::{Affine, BezPath, Size};
use mastik_core::peniko::Color;
use mastik_core::{
    Brush, Document, Fill, IdSource, NodeId, NodeKind, Operation, Resolved, Transaction,
    keyed_by_position,
};

const GREEN: Color = Color::from_rgb8(0x40, 0xa0, 0x60);

struct Fixture {
    doc: Document,
    ids: IdSource,
    frame: NodeId,
    path: NodeId,
    leaf: NodeId,
}

impl Fixture {
    /// A path from (0, 100) to (400, 100) in the frame, under a green 80 × 20
    /// ellipse named *Leaf*, and a rectangle over both.
    fn new() -> Self {
        let mut ids = IdSource::new(0xB7);
        let root = ids.mint();
        let mut doc = Document::new(root);
        let (frame, path, leaf, top) = (ids.mint(), ids.mint(), ids.mint(), ids.mint());
        let mut line = BezPath::new();
        line.move_to((0.0, 100.0));
        line.line_to((400.0, 100.0));
        doc.apply(&Transaction(vec![
            Operation::CreateNode {
                id: frame,
                parent: root,
                index: 0,
                kind: NodeKind::Artboard {
                    size: Size::new(600.0, 400.0),
                    corner_radii: Default::default(),
                    corner_smoothing: 0.0,
                },
                transform: Some(Affine::translate((50.0, 30.0))),
                name: None,
            },
            Operation::CreateNode {
                id: path,
                parent: frame,
                index: 0,
                kind: NodeKind::Path {
                    path: line,
                    corner_radii: Vec::new(),
                    corner_smoothing: 0.0,
                    mirror: Default::default(),
                },
                transform: None,
                name: Some("Stroke line".into()),
            },
            Operation::CreateNode {
                id: leaf,
                parent: frame,
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
                    brush: Brush::Solid(GREEN).into(),
                    visible: true,
                }]),
            },
            Operation::CreateNode {
                id: top,
                parent: frame,
                index: 2,
                kind: NodeKind::Rect {
                    size: Size::new(10.0, 10.0),
                    corner_radii: Default::default(),
                    corner_smoothing: 0.0,
                },
                transform: None,
                name: None,
            },
        ]))
        .expect("build the fixture");
        Fixture {
            doc,
            ids,
            frame,
            path,
            leaf,
        }
    }

    fn apply(&mut self, mode: Mode, keep_path: bool) -> Result<NodeId, Refusal> {
        let res = Resolved::rebuild(&self.doc);
        let how = ApplyBrush {
            path: self.path,
            brush: self.leaf,
            mode,
            keep_path,
        };
        let (tx, group) = apply_brush::apply(&self.doc, &res, &mut self.ids, &how)?;
        self.doc.apply(&tx).expect("the edit applies");
        Ok(group)
    }
}

/// Of two layers the lower is the path and the upper the brush, unless only the
/// upper can be run along; more than two, or neither, are refused.
#[test]
fn the_pair_is_lower_path_upper_brush() {
    let mut f = Fixture::new();
    assert_eq!(
        apply_brush::pair(&f.doc, &[f.leaf, f.path]),
        Ok((f.path, f.leaf))
    );
    let top = f.doc.get(f.frame).unwrap().children()[2];
    assert_eq!(
        apply_brush::pair(&f.doc, &[f.path, f.leaf, top]),
        Err(Refusal::Count)
    );
    // Text under a path: the path is the only one to run along, so it is the path.
    let text = f.ids.mint();
    f.doc
        .apply(&Transaction(vec![Operation::CreateNode {
            id: text,
            parent: f.frame,
            index: 0,
            kind: NodeKind::Text {
                content: "Hi".into(),
                style: Default::default(),
                spans: Default::default(),
                para_spans: Default::default(),
                paragraph: Default::default(),
                block: Default::default(),
                sizing: mastik_core::TextSizing::Auto,
                on_path: None,
                on_path_flip: false,
                on_path_offset: 0.0,
            },
            transform: None,
            name: None,
        }]))
        .expect("add a text");
    assert_eq!(
        apply_brush::pair(&f.doc, &[text, f.path]),
        Ok((f.path, text))
    );
    let res = Resolved::rebuild(&f.doc);
    assert_eq!(
        apply_brush::selection_refusal(&f.doc, &res, &[text, f.path]),
        Some(Refusal::Text)
    );
    assert_eq!(
        apply_brush::refusal(&f.doc, &res, f.path, f.path),
        Some(Refusal::Same)
    );
    assert_eq!(
        Refusal::Count.says(),
        "Select a path, and the shape to run along it"
    );
}

/// An art brush lands as a group named for the brush in the path's slot, one
/// path inside wearing the brush's fill and spanning the path in world space; the
/// path goes and the brush stays.
/// flip: `apply` writing the bent path in world rather than parent space → fails
/// at "spans the path".
#[test]
fn art_lands_a_group_in_the_paths_place() {
    let mut f = Fixture::new();
    let group = f.apply(Mode::Art(Art::DEFAULT), false).unwrap();
    let kids = f.doc.get(f.frame).unwrap().children().to_vec();
    assert_eq!(kids[0], group, "in the path's slot");
    assert!(f.doc.get(f.path).is_none(), "the path goes");
    assert!(kids.contains(&f.leaf), "the brush stays");
    let g = f.doc.get(group).unwrap();
    assert_eq!(g.name(), "Brushed · Leaf");
    let [made] = g.children() else {
        panic!("one bent path: {:?}", g.children());
    };
    let made = f.doc.get(*made).unwrap();
    assert_eq!(made.name(), "Leaf");
    assert_eq!(
        made.paint().fills[0].brush.literal(),
        Some(&Brush::Solid(GREEN))
    );
    let res = Resolved::rebuild(&f.doc);
    let b = res.world_bounds(made.id()).unwrap();
    assert!(
        (b.x0 - 50.0).abs() < 0.1 && (b.x1 - 450.0).abs() < 0.1,
        "spans the path: {b:?}"
    );
    assert!((b.center().y - 130.0).abs() < 0.1 && (b.height() - 20.0).abs() < 0.1);
}

/// With *Keep the path* the path sits at the group's bottom, hidden and named
/// *Path*, where it was drawn.
/// flip: `apply` leaving the kept path visible → fails at "hidden".
#[test]
fn a_kept_path_sits_hidden_in_the_group() {
    let mut f = Fixture::new();
    let group = f.apply(Mode::Art(Art::DEFAULT), true).unwrap();
    let kids = f.doc.get(group).unwrap().children();
    assert_eq!(kids.len(), 2);
    assert_eq!(kids[0], f.path);
    let path = f.doc.get(f.path).unwrap();
    assert!(!path.visible(), "hidden");
    assert_eq!(path.name(), "Path");
    let res = Resolved::rebuild(&f.doc);
    assert_eq!(
        res.world_transform(f.path),
        Some(Affine::translate((50.0, 30.0)))
    );
}

/// A scatter makes one path per copy, spaced along the path.
#[test]
fn scatter_makes_a_path_per_copy() {
    let mut f = Fixture::new();
    let sc = Scatter {
        spacing: 1.25,
        ..Scatter::DEFAULT
    };
    let group = f.apply(Mode::Scatter(sc), false).unwrap();
    // 400 along at 100 a step: five copies.
    assert_eq!(f.doc.get(group).unwrap().children().len(), 5);
}

/// A locked path is refused, the brush untouched.
#[test]
fn a_locked_path_is_refused() {
    let mut f = Fixture::new();
    f.doc
        .apply(&Transaction(vec![Operation::SetLocked {
            id: f.path,
            locked: true,
        }]))
        .unwrap();
    assert_eq!(
        f.apply(Mode::Art(Art::DEFAULT), false),
        Err(Refusal::Locked)
    );
}
