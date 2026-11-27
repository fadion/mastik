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

/// The fixture's frame made a main, with an instance of it beside: answers the
/// instance's path and its copy of *Leaf*.
fn with_instance(f: &mut Fixture) -> (NodeId, NodeId) {
    f.edit(vec![Operation::SetComponent {
        id: f.frame,
        component: true,
    }]);
    let root = f.doc.get(f.frame).and_then(|n| n.parent()).unwrap();
    let (tx, made) = mastik_core::insert_subtrees(
        &f.doc,
        &mut f.ids,
        &[mastik_core::Placement {
            nodes: f.doc.capture_subtree(f.frame).unwrap(),
            parent: root,
            index: None,
        }],
        Default::default(),
    );
    f.edit(tx.0);
    let copy_of = |src: NodeId| {
        mastik_core::build::subtree_nodes(&f.doc, &[made[0]])
            .into_iter()
            .find(|n| f.doc.get(*n).and_then(|n| n.link()) == Some(src))
            .expect("the instance's copy")
    };
    (copy_of(f.path), copy_of(f.leaf))
}

/// `ops` with everything the components owe them, applied: the commit's assembly.
fn commit(f: &mut Fixture, ops: Vec<Operation>) {
    let mut tx = Transaction(ops);
    mastik_core::propagate::owed(&f.doc, &mut tx, &mut f.ids);
    f.edit(tx.0);
}

fn link_of(f: &Fixture, id: NodeId) -> BrushLink {
    f.doc.get(id).and_then(|n| n.brush()).expect("the link")
}

/// A brushed path whose brush sits in the same main: each instance draws with
/// its own copy of the brush and shows no override for it.
#[test]
fn an_instance_draws_with_its_own_copy_of_the_brush() {
    let mut f = Fixture::new();
    f.link(Mode::Art(Art::DEFAULT));
    let (path, leaf) = with_instance(&mut f);
    assert_eq!(
        link_of(&f, path).of,
        leaf,
        "the instance names its own brush"
    );
    assert!(f.res.brush_ink(path).is_some());
    assert_eq!(
        mastik_core::reset::overrides(&f.doc, path),
        Vec::new(),
        "the instance's brush is no override"
    );
    // An untouched copy goes with its source.
    let main_path = f.path;
    commit(&mut f, vec![Operation::DeleteNode { id: main_path }]);
    assert!(f.doc.get(path).is_none(), "the instance's path went too");
}

/// The main's new brush options reach the instance, still naming the
/// instance's brush; options the instance set itself are kept, and their reset
/// puts the main's back on the instance's own brush.
#[test]
fn a_mains_brush_options_reach_its_instances() {
    let mut f = Fixture::new();
    f.link(Mode::Art(Art::DEFAULT));
    let (path, leaf) = with_instance(&mut f);
    let (main_path, main_leaf) = (f.path, f.leaf);
    let scatter = Mode::Scatter(Scatter::DEFAULT);
    commit(
        &mut f,
        vec![Operation::SetBrush {
            id: main_path,
            brush: Some(BrushLink::new(main_leaf, scatter)),
        }],
    );
    assert_eq!(link_of(&f, path), BrushLink::new(leaf, scatter), "follows");
    let seeded = Mode::Scatter(Scatter {
        seed: 9,
        ..Scatter::DEFAULT
    });
    commit(
        &mut f,
        vec![Operation::SetBrush {
            id: path,
            brush: Some(BrushLink::new(leaf, seeded)),
        }],
    );
    assert_eq!(mastik_core::reset::overrides(&f.doc, path).len(), 1);
    commit(
        &mut f,
        vec![Operation::SetBrush {
            id: main_path,
            brush: Some(BrushLink::new(main_leaf, Mode::Art(Art::DEFAULT))),
        }],
    );
    assert_eq!(link_of(&f, path), BrushLink::new(leaf, seeded), "kept");
    // Pushed, the main takes the options on its own brush.
    let instance = f.doc.get(path).and_then(|n| n.parent()).unwrap();
    let mut scratch = f.doc.clone();
    let mut ids = IdSource::new(0xC4);
    let kind = mastik_core::push::Kind::Fields;
    let mut tx = Transaction(mastik_core::push::push(&f.doc, kind, instance, &mut ids));
    mastik_core::propagate::owed(&scratch, &mut tx, &mut ids);
    scratch.apply(&tx).expect("the push applies");
    assert_eq!(
        scratch.get(main_path).and_then(|n| n.brush()),
        Some(BrushLink::new(main_leaf, seeded)),
        "the push names the main's brush"
    );
    let reset = mastik_core::reset::overrides(&f.doc, path)
        .pop()
        .expect("one override")
        .reset;
    commit(&mut f, vec![reset]);
    assert_eq!(
        link_of(&f, path),
        BrushLink::new(leaf, Mode::Art(Art::DEFAULT)),
        "the reset names the instance's brush"
    );
}

/// A copy of the brushed path pastes as its baked twin where *Leaf* is not,
/// naming the brush it baked, and as itself where it is.
#[test]
fn a_paste_where_the_brush_is_not_bakes_and_names_it() {
    let mut f = Fixture::new();
    f.link(Mode::Art(Art::DEFAULT));
    let copied = vec![f.doc.capture_subtree(f.path).unwrap()];
    let baked = live_brush::baked_copies(&f.doc, &f.res, &[f.path], &mut f.ids);
    let elsewhere = Document::new(IdSource::new(0xD1).mint());
    let (subtrees, names) = live_brush::fitted(&elsewhere, &copied, &baked);
    assert_eq!(names, vec!["Leaf".to_string()]);
    assert_ne!(subtrees, copied, "the twin is pasted");
    let (subtrees, names) = live_brush::fitted(&f.doc, &copied, &baked);
    assert!(names.is_empty(), "{names:?}");
    assert_eq!(subtrees, copied, "the link is pasted where its brush is");
}

/// Deleting the main's brush, as the app commits it (its users baked first,
/// then what the components owe), leaves no layer linked to nothing, though an
/// instance's path holds options of its own: its brush copy stays with it.
/// flip: `propagate_structure` without its spared-brush loop → fails at
/// "linked to nothing".
#[test]
fn deleting_a_mains_brush_leaves_no_instance_linked_to_nothing() {
    let mut f = Fixture::new();
    f.link(Mode::Scatter(Scatter::DEFAULT));
    let (path, leaf) = with_instance(&mut f);
    let own = Mode::Scatter(Scatter {
        seed: 9,
        ..Scatter::DEFAULT
    });
    commit(
        &mut f,
        vec![Operation::SetBrush {
            id: path,
            brush: Some(BrushLink::new(leaf, own)),
        }],
    );
    let main_leaf = f.leaf;
    let delete = Transaction(vec![Operation::DeleteNode { id: main_leaf }]);
    let mut ops = live_brush::baked_for_deletes(&f.doc, &f.res, &delete, &mut f.ids);
    ops.extend(delete.0);
    commit(&mut f, ops);
    let dangling: Vec<NodeId> = mastik_core::build::subtree_nodes(&f.doc, &[f.frame])
        .into_iter()
        .chain([path])
        .filter(|id| {
            f.doc
                .get(*id)
                .and_then(|n| n.brush())
                .is_some_and(|l| !f.doc.contains(l.of))
        })
        .collect();
    assert!(dangling.is_empty(), "linked to nothing: {dangling:?}");
    assert!(
        f.doc.contains(leaf) && link_of(&f, path).of == leaf,
        "the instance keeps the brush it draws with"
    );
}

/// The fixture's frame, brushed with *Leaf*, and a copy of it with each layer
/// named in `renames` renamed, combined into a set, with an instance of the
/// first. Answers the instance's root and the second variant.
fn two_variants(f: &mut Fixture, renames: &[(&str, &str)]) -> (NodeId, NodeId) {
    use mastik_core::variant;
    f.link(Mode::Art(Art::DEFAULT));
    let root = f.doc.get(f.frame).and_then(|n| n.parent()).unwrap();
    let (nodes, second) =
        mastik_core::remap_subtree(&f.doc.capture_subtree(f.frame).unwrap(), &mut f.ids).unwrap();
    let mut ops: Vec<Operation> = renames
        .iter()
        .map(|(from, to)| Operation::SetName {
            id: nodes.iter().find(|n| n.name() == *from).unwrap().id(),
            name: (*to).into(),
        })
        .collect();
    ops.insert(
        0,
        Operation::InsertSubtree {
            nodes,
            parent: root,
            index: 1,
        },
    );
    ops.extend([
        Operation::SetName {
            id: f.frame,
            name: "Card/On".into(),
        },
        Operation::SetName {
            id: second,
            name: "Card/Off".into(),
        },
    ]);
    f.edit(ops);
    for main in [f.frame, second] {
        f.edit(vec![Operation::SetComponent {
            id: main,
            component: true,
        }]);
    }
    let (tx, _) = variant::combine(&f.doc, &f.res, &mut f.ids, &[f.frame, second]).unwrap();
    commit(f, tx.0);
    let (tx, made) = mastik_core::insert_subtrees(
        &f.doc,
        &mut f.ids,
        &[mastik_core::Placement {
            nodes: f.doc.capture_subtree(f.frame).unwrap(),
            parent: root,
            index: None,
        }],
        Default::default(),
    );
    f.edit(tx.0);
    (made[0], second)
}

/// The instance rooted at `instance` switched to `to`, with what it owes.
fn switch_to(f: &mut Fixture, instance: NodeId, to: NodeId) {
    let tx = mastik_core::variant::switch(&f.doc, instance, to, &mut f.ids).expect("a switch");
    commit(f, tx.0);
}

/// The layer named `name` in the instance rooted at `instance`.
fn named_in(f: &Fixture, instance: NodeId, name: &str) -> NodeId {
    mastik_core::build::subtree_nodes(&f.doc, &[instance])
        .into_iter()
        .find(|id| f.doc.get(*id).is_some_and(|n| n.name() == name))
        .unwrap_or_else(|| panic!("no {name} in the instance"))
}

/// A switch to a variant draws the instance's path with the instance's copy of
/// that variant's brush, whether the switch keeps the brush layer (same name) or
/// copies the variant's in (another name).
/// flip: `rewrite`'s `at_copy` as the identity → fails at "Leaf: … own copy";
/// `copy_of_new` without the inserted copies → fails at "Petal: … own copy";
/// the spared-brush loop counting rebrushed paths → fails at "Petal: … goes with
/// the switch".
#[test]
fn a_variant_switch_draws_with_the_instances_own_brush() {
    for brush_name in ["Leaf", "Petal"] {
        let mut f = Fixture::new();
        let (instance, second) = two_variants(&mut f, &[("Leaf", brush_name)]);
        switch_to(&mut f, instance, second);
        let path = named_in(&f, instance, "Stroke line");
        let brush = named_in(&f, instance, brush_name);
        assert_eq!(
            f.doc.get(instance).and_then(|n| n.link()),
            Some(second),
            "{brush_name}: the instance shows the second variant"
        );
        assert_eq!(
            link_of(&f, path).of,
            brush,
            "{brush_name}: the path draws with the instance's own copy"
        );
        assert_eq!(
            mastik_core::reset::overrides(&f.doc, path),
            Vec::new(),
            "{brush_name}: the switched path's brush is no override"
        );
        let leaves = mastik_core::build::subtree_nodes(&f.doc, &[instance])
            .into_iter()
            .filter(|id| f.doc.get(*id).is_some_and(|n| n.name() == "Leaf"))
            .count();
        assert_eq!(
            leaves,
            usize::from(brush_name == "Leaf"),
            "{brush_name}: the old variant's brush goes with the switch"
        );
    }
}

/// A switch leaves no layer linked to nothing: an instance's path set to draw
/// with its own copy of *Bud*, a layer the target variant does not have, keeps
/// that copy, which an untouched counterpart with no match would lose.
/// flip: `rewrite` skipping its spared-brush loop → fails at "linked to nothing".
#[test]
fn a_variant_switch_keeps_a_brush_the_instance_still_draws_with() {
    let mut f = Fixture::new();
    let bud = f.ids.mint();
    f.edit(vec![Operation::CreateNode {
        id: bud,
        parent: f.frame,
        index: 2,
        kind: NodeKind::Ellipse {
            size: Size::new(40.0, 40.0),
            arc: Default::default(),
        },
        transform: Some(Affine::translate((300.0, 250.0))),
        name: Some("Bud".into()),
    }]);
    let (instance, second) = two_variants(&mut f, &[("Bud", "Bloom")]);
    let path = named_in(&f, instance, "Stroke line");
    let own_bud = named_in(&f, instance, "Bud");
    commit(
        &mut f,
        vec![Operation::SetBrush {
            id: path,
            brush: Some(BrushLink::new(own_bud, Mode::Art(Art::DEFAULT))),
        }],
    );
    switch_to(&mut f, instance, second);
    let of = link_of(&f, path).of;
    assert!(f.doc.contains(of), "the path is linked to nothing");
    assert_eq!(of, own_bud, "the path keeps the instance's Bud");
    assert!(f.res.brush_ink(path).is_some(), "the path still draws");
}
