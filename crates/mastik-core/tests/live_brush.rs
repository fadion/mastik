//! Live brushes (`live_brush.rs`): a path linked to a brush layer, drawn from the
//! bake's geometry and kept up to date by `Resolved` as either one changes.
//!
//! The fixture's path and brush sit in a frame translated by (50, 30), as in
//! `apply_brush.rs`'s tests.

use mastik_core::apply_brush::{Art, Mode, Refusal, Scatter};
use mastik_core::kurbo::{Affine, BezPath, Point, Shape, Size};
use mastik_core::live_brush::{self, BrushLink};
use mastik_core::peniko::Color;
use mastik_core::{
    Brush, Document, Fill, GeometryPatch, IdSource, NodeId, NodeKind, Operation, Resolved, Stroke,
    Transaction, keyed_by_position,
};

const GREEN: Color = Color::from_rgb8(0x40, 0xa0, 0x60);

struct Fixture {
    doc: Document,
    res: Resolved,
    ids: IdSource,
    frame: NodeId,
    path: NodeId,
    leaf: NodeId,
}

impl Fixture {
    /// A stroked path from (0, 100) to (400, 100) in the frame and a green 80 × 20
    /// ellipse named *Leaf* inside a group.
    fn new() -> Self {
        let mut ids = IdSource::new(0xB8);
        let root = ids.mint();
        let mut doc = Document::new(root);
        let (frame, path, group, leaf) = (ids.mint(), ids.mint(), ids.mint(), ids.mint());
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
            Operation::SetStrokes {
                id: path,
                strokes: keyed_by_position([Stroke::default()]),
            },
            Operation::CreateNode {
                id: group,
                parent: frame,
                index: 1,
                kind: NodeKind::Group,
                transform: None,
                name: None,
            },
            Operation::CreateNode {
                id: leaf,
                parent: group,
                index: 0,
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
        ]))
        .expect("build the fixture");
        let res = Resolved::rebuild(&doc);
        Fixture {
            doc,
            res,
            ids,
            frame,
            path,
            leaf,
        }
    }

    /// `ops` applied, the resolve brought up to date the way the app does it.
    fn edit(&mut self, ops: Vec<Operation>) {
        let out = self.doc.apply(&Transaction(ops)).expect("the edit applies");
        self.res.update(&self.doc, &out.dirty);
    }

    fn link(&mut self, mode: Mode) {
        let brush = Some(BrushLink::new(self.leaf, mode));
        self.edit(vec![Operation::SetBrush {
            id: self.path,
            brush,
        }]);
    }

    /// The live resolve agrees with one rebuilt from scratch on the brush.
    fn agrees(&self) {
        let fresh = Resolved::rebuild(&self.doc);
        assert_eq!(
            self.res.brush_ink(self.path).map(|i| (**i).clone()),
            fresh.brush_ink(self.path).map(|i| (**i).clone()),
            "the brush ink drifted from a rebuild"
        );
        assert_eq!(
            self.res.ink_bounds(self.path),
            fresh.ink_bounds(self.path),
            "the ink box drifted from a rebuild"
        );
    }
}

/// Linking a brush gives the path ink along its whole length and an ink box
/// that holds it, and its strokes give way.
#[test]
fn a_linked_brush_draws_along_the_path() {
    let mut f = Fixture::new();
    assert!(!live_brush::hides_strokes(&f.doc, f.path));
    f.link(Mode::Art(Art {
        width: 2.0,
        ..Art::DEFAULT
    }));
    assert!(live_brush::hides_strokes(&f.doc, f.path));
    let ink = f.res.brush_ink(f.path).expect("the brush draws");
    let b = ink.path.bounding_box();
    // In the path's own space: the line at y 100, the leaf 20 tall times 2.
    assert!(
        (b.x0 - 0.0).abs() < 0.5 && (b.x1 - 400.0).abs() < 0.5,
        "{b:?}"
    );
    assert!(
        (b.y0 - 80.0).abs() < 0.5 && (b.y1 - 120.0).abs() < 0.5,
        "{b:?}"
    );
    let ink_box = f.res.ink_bounds(f.path).expect("an ink box");
    assert!(ink_box.y0 <= 30.0 + 80.0 + 0.5, "{ink_box:?}");
    f.agrees();
}

/// Reshaping the brush, or moving the group it sits in, redraws its users;
/// the update agrees with a rebuild each time.
#[test]
fn editing_the_brush_redraws_its_users() {
    let mut f = Fixture::new();
    f.link(Mode::Art(Art::DEFAULT));
    let before = f.res.brush_ink(f.path).cloned().expect("ink");
    f.edit(vec![Operation::SetGeometry {
        id: f.leaf,
        geometry: GeometryPatch::Size(Size::new(80.0, 40.0)),
    }]);
    let after = f.res.brush_ink(f.path).cloned().expect("ink");
    assert_ne!(*before, *after, "a taller brush draws taller ink");
    f.agrees();
    // Moving the brush's group: a brush is read from its own box, so the ink
    // stays, and the resolve still agrees.
    let group = f.doc.get(f.leaf).and_then(|n| n.parent()).unwrap();
    f.edit(vec![Operation::SetTransform {
        id: group,
        transform: Affine::translate((-60.0, 10.0)),
    }]);
    let moved = f.res.brush_ink(f.path).cloned().expect("ink");
    let drift = moved
        .path
        .bounding_box()
        .union(after.path.bounding_box())
        .size()
        - after.path.bounding_box().size();
    assert!(drift.width.abs() < 1e-6 && drift.height.abs() < 1e-6);
    f.agrees();
    // Scatter, then a path edit.
    f.link(Mode::Scatter(Scatter::DEFAULT));
    let copies = f.res.brush_ink(f.path).map(|i| i.places.len()).unwrap_or(0);
    assert!(copies > 1, "scatter lays several copies");
    let mut longer = BezPath::new();
    longer.move_to((0.0, 100.0));
    longer.line_to((560.0, 100.0));
    f.edit(vec![Operation::SetGeometry {
        id: f.path,
        geometry: GeometryPatch::Path {
            path: longer,
            corner_radii: Vec::new(),
            mirror: None,
        },
    }]);
    let more = f.res.brush_ink(f.path).map(|i| i.places.len()).unwrap_or(0);
    assert!(more > copies, "a longer path takes more copies");
    f.agrees();
}

/// Hiding the link draws the path's strokes again and no ink.
#[test]
fn a_hidden_brush_gives_back_the_strokes() {
    let mut f = Fixture::new();
    let mut link = BrushLink::new(f.leaf, Mode::Art(Art::DEFAULT));
    link.visible = false;
    f.edit(vec![Operation::SetBrush {
        id: f.path,
        brush: Some(link),
    }]);
    assert!(!live_brush::hides_strokes(&f.doc, f.path));
    assert!(f.res.brush_ink(f.path).is_none());
    f.agrees();
}

/// A click on the brush's ink picks the path layer, never the brush.
#[test]
fn a_click_on_the_ink_picks_the_brushed_layer() {
    let mut f = Fixture::new();
    f.link(Mode::Art(Art {
        width: 2.0,
        ..Art::DEFAULT
    }));
    let page = f.doc.first_page();
    // 15 above the line's centre, in world space: inside the 40-tall ink.
    let hits = mastik_core::query::hit_test(&f.res, &f.doc, page, Point::new(250.0, 115.0), 0.0);
    assert_eq!(hits.first(), Some(&f.path), "{hits:?}");
}

/// Deleting the brush bakes every layer drawn with it in the same edit, so the
/// document never holds a link to nothing; undo brings the link back.
#[test]
fn deleting_the_brush_bakes_its_users() {
    let mut f = Fixture::new();
    f.link(Mode::Art(Art::DEFAULT));
    let delete = Transaction(vec![Operation::DeleteNode { id: f.leaf }]);
    let mut ops = live_brush::baked_for_deletes(&f.doc, &f.res, &delete, &mut f.ids);
    assert!(!ops.is_empty(), "the user is baked");
    ops.extend(delete.0);
    let out = f
        .doc
        .apply(&Transaction(ops))
        .expect("the bake and delete apply");
    assert!(
        f.doc.get(f.path).is_none(),
        "an unfilled path goes with its bake"
    );
    let group = f.doc.get(f.frame).unwrap().children()[0];
    let g = f.doc.get(group).unwrap();
    assert_eq!(g.name(), "Brushed · Leaf");
    assert!(
        f.doc
            .node_ids()
            .all(|id| f.doc.get(id).is_some_and(|n| n.brush().is_none()))
    );
    f.doc.apply(&out.inverse).expect("undo");
    assert_eq!(
        f.doc.get(f.path).and_then(|n| n.brush()).map(|l| l.of),
        Some(f.leaf)
    );
}

/// A brush can't be drawn along a path it brushes, nor sit in an instance; a
/// bake's mask and layout refusals do not hold for a live link.
#[test]
fn a_looping_link_is_refused() {
    let mut f = Fixture::new();
    f.link(Mode::Art(Art::DEFAULT));
    // The leaf drawn along the path, and now the path asked to brush the leaf.
    assert_eq!(
        live_brush::refusal(&f.doc, &f.res, f.leaf, f.path),
        Some(Refusal::Loop)
    );
    assert_eq!(
        live_brush::refusal(&f.doc, &f.res, f.path, f.leaf),
        None,
        "relinking the same pair is no loop"
    );
}

/// The save file keeps the link and its options.
#[test]
fn the_save_file_keeps_the_link() {
    let mut f = Fixture::new();
    let mode = Mode::Scatter(Scatter {
        seed: 7,
        ..Scatter::DEFAULT
    });
    f.link(mode);
    let bytes = mastik_core::io::save(&f.doc).expect("save");
    let back = mastik_core::io::load(&bytes).expect("load");
    let link = back.get(f.path).and_then(|n| n.brush()).expect("the link");
    assert_eq!(link, BrushLink::new(f.leaf, mode));
    let res = Resolved::rebuild(&back);
    assert!(res.brush_ink(f.path).is_some());
}
