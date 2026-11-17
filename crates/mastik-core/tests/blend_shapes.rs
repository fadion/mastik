//! *Blend shapes* (`blend.rs`): the steps between two shapes.
//!
//! The fixture is a circle and a rounded square on a frame translated by (50, 30),
//! so the world space the outlines are matched in is never the parent's.

use mastik_core::blend::{self, Blend, Refusal};
use mastik_core::kurbo::{Affine, Point, RoundedRectRadii, Shape, Size, Vec2};
use mastik_core::peniko::Color;
use mastik_core::{
    Brush, Document, Fill, IdSource, NodeId, NodeKind, Operation, Resolved, Transaction,
    keyed_by_position,
};

const BLUE: Color = Color::from_rgb8(0x60, 0x80, 0xd0);
const ORANGE: Color = Color::from_rgb8(0xd0, 0xa0, 0x50);

struct Fixture {
    doc: Document,
    ids: IdSource,
    frame: NodeId,
    circle: NodeId,
    square: NodeId,
}

fn filled(color: Color) -> Vec<mastik_core::Keyed<Fill>> {
    keyed_by_position([Fill {
        brush: Brush::Solid(color).into(),
        visible: true,
    }])
}

impl Fixture {
    /// A blue circle r 30 centred on (110, 260) under an orange 166-square at
    /// (530, 176) with corners of 16, the circle at half opacity.
    fn new() -> Self {
        let mut ids = IdSource::new(0xB1);
        let root = ids.mint();
        let mut doc = Document::new(root);
        let (frame, circle, square) = (ids.mint(), ids.mint(), ids.mint());
        doc.apply(&Transaction(vec![
            Operation::CreateNode {
                id: frame,
                parent: root,
                index: 0,
                kind: NodeKind::Artboard {
                    size: Size::new(800.0, 420.0),
                    corner_radii: Default::default(),
                    corner_smoothing: 0.0,
                },
                transform: Some(Affine::translate((50.0, 30.0))),
                name: None,
            },
            Operation::CreateNode {
                id: circle,
                parent: frame,
                index: 0,
                kind: NodeKind::Ellipse {
                    size: Size::new(60.0, 60.0),
                    arc: Default::default(),
                },
                transform: Some(Affine::translate((80.0, 230.0))),
                name: Some("Circle".into()),
            },
            Operation::SetFills {
                id: circle,
                fills: filled(BLUE),
            },
            Operation::SetOpacity {
                id: circle,
                opacity: 0.5,
            },
            Operation::CreateNode {
                id: square,
                parent: frame,
                index: 1,
                kind: NodeKind::Rect {
                    size: Size::new(166.0, 166.0),
                    corner_radii: RoundedRectRadii::from_single_radius(16.0),
                    corner_smoothing: 0.0,
                },
                transform: Some(Affine::translate((530.0, 176.0))),
                name: Some("Square".into()),
            },
            Operation::SetFills {
                id: square,
                fills: filled(ORANGE),
            },
        ]))
        .expect("build the fixture");
        Self {
            doc,
            ids,
            frame,
            circle,
            square,
        }
    }

    fn blend(&mut self, b: Blend) -> NodeId {
        let res = Resolved::rebuild(&self.doc);
        let (tx, group) = blend::blend(
            &self.doc,
            &res,
            &mut self.ids,
            &[self.square, self.circle],
            b,
        )
        .expect("a blend");
        self.doc.apply(&tx).expect("the blend applies");
        group
    }
}

const FIVE: Blend = Blend::DEFAULT;

/// The steps between the two shapes, `steps` of them, inside the group.
fn steps_of(f: &Fixture, group: NodeId) -> Vec<NodeId> {
    let kids = f.doc.get(group).unwrap().children();
    kids[1..kids.len() - 1].to_vec()
}

/// A group named *Blend* takes the two shapes and five plain paths between them,
/// the circle lowest, the steps running up to the square on top, each shape
/// still drawn where it was.
/// flip: `blend` without its `Reparent`s → fails at "the group is the whole blend".
#[test]
fn the_group_holds_both_shapes_and_the_steps_between() {
    let mut f = Fixture::new();
    let before = Resolved::rebuild(&f.doc);
    let was = [f.circle, f.square].map(|id| before.world_transform(id).unwrap());
    let group = f.blend(FIVE);
    assert_eq!(
        f.doc.get(f.frame).unwrap().children(),
        &[group],
        "the group is the whole blend"
    );
    let g = f.doc.get(group).unwrap();
    assert_eq!(g.name(), "Blend");
    let kids = g.children();
    assert_eq!(kids.len(), 7);
    assert_eq!((kids[0], kids[6]), (f.circle, f.square));
    for (k, step) in steps_of(&f, group).iter().enumerate() {
        let n = f.doc.get(*step).unwrap();
        assert!(matches!(n.kind(), NodeKind::Path { .. }));
        assert_eq!(n.name(), format!("Step {}", k + 1));
    }
    let after = Resolved::rebuild(&f.doc);
    for (id, w) in [f.circle, f.square].into_iter().zip(was) {
        let now = after.world_transform(id).unwrap();
        assert!(
            now.as_coeffs()
                .iter()
                .zip(w.as_coeffs())
                .all(|(a, b)| (a - b).abs() < 1e-9),
            "drawn where it was"
        );
    }
}

/// A shape in another group is refused, by the row and the commit alike: moved into
/// the other's group it would leave its own parent's clip, opacity and layout.
/// flip: `pair` without its common-parent test → fails at the refusal.
#[test]
fn shapes_in_different_parents_are_refused() {
    let mut f = Fixture::new();
    let holder = f.ids.mint();
    f.doc
        .apply(&Transaction(vec![
            Operation::CreateNode {
                id: holder,
                parent: f.frame,
                index: 0,
                kind: NodeKind::Group,
                transform: Some(Affine::translate((20.0, 10.0))),
                name: None,
            },
            Operation::Reparent {
                id: f.circle,
                new_parent: holder,
                index: 0,
            },
        ]))
        .unwrap();
    assert_eq!(
        blend::pair(&f.doc, &[f.circle, f.square]),
        Err(Refusal::Parents)
    );
    let res = Resolved::rebuild(&f.doc);
    let made = blend::blend(&f.doc, &res, &mut f.ids, &[f.circle, f.square], FIVE);
    assert_eq!(made.err(), Some(Refusal::Parents));
}

/// A mask is refused: in the group it would stop clipping what it clips.
/// flip: `pair` without its mask test → fails at the refusal.
#[test]
fn a_mask_is_refused() {
    let mut f = Fixture::new();
    f.doc
        .apply(&Transaction(vec![Operation::SetMask {
            id: f.circle,
            mask: true,
        }]))
        .unwrap();
    assert_eq!(
        blend::pair(&f.doc, &[f.circle, f.square]),
        Err(Refusal::Mask)
    );
}

/// An open line against a closed circle pairs as far as `pair` goes, and is refused
/// by `refusal`, which the row asks, with the sentence a commit gives.
/// flip: `refusal` answering `pair` alone → fails at "the row is dimmed".
#[test]
fn open_against_closed_is_refused_before_the_commit() {
    let mut f = Fixture::new();
    let line = f.ids.mint();
    f.doc
        .apply(&Transaction(vec![Operation::CreateNode {
            id: line,
            parent: f.frame,
            index: 2,
            kind: NodeKind::Line {
                end: Point::new(100.0, 0.0),
            },
            transform: Some(Affine::translate((300.0, 50.0))),
            name: None,
        }]))
        .unwrap();
    let res = Resolved::rebuild(&f.doc);
    assert!(blend::pair(&f.doc, &[f.circle, line]).is_ok());
    assert_eq!(
        blend::refusal(&f.doc, &res, &[f.circle, line]),
        Some(Refusal::Parts),
        "the row is dimmed"
    );
    let made = blend::blend(&f.doc, &res, &mut f.ids, &[f.circle, line], FIVE);
    assert_eq!(made.err(), Some(Refusal::Parts));
    assert_eq!(blend::refusal(&f.doc, &res, &[f.circle, f.square]), None);
}

/// A gradient on each shape, in a frame that scales, lies over every step as it
/// lay over the nearer shape: its ends fall at the same fractions of the step's
/// world box as of the shape's, however far the step stands from the shape.
/// flip: `blend` carrying each brush by the shape's own map alone, not onto the
/// step's box (`box_onto`) → fails at step 1's ramp.
#[test]
fn a_gradient_is_carried_into_each_steps_space() {
    use mastik_core::peniko::{Gradient, GradientKind};
    let mut f = Fixture::new();
    f.doc
        .apply(&Transaction(vec![
            Operation::SetTransform {
                id: f.frame,
                transform: Affine::translate((50.0, 30.0)) * Affine::scale(2.0),
            },
            Operation::SetFills {
                id: f.circle,
                fills: keyed_by_position([Fill {
                    brush: Brush::Gradient(Gradient::new_linear((0.0, 0.0), (60.0, 60.0)).into())
                        .into(),
                    visible: true,
                }]),
            },
            // Both gradients, so the steps either side of the middle each carry one.
            Operation::SetFills {
                id: f.square,
                fills: keyed_by_position([Fill {
                    brush: Brush::Gradient(Gradient::new_linear((0.0, 0.0), (166.0, 0.0)).into())
                        .into(),
                    visible: true,
                }]),
            },
        ]))
        .unwrap();
    // The ramp's ends as fractions of the layer's world box.
    let ends = |doc: &Document, res: &Resolved, id: NodeId| {
        let Some(Brush::Gradient(g)) = doc.get(id).unwrap().paint().fills[0].brush.literal() else {
            panic!("a gradient");
        };
        let GradientKind::Linear(line) = g.gradient.kind else {
            panic!("linear");
        };
        let world = res.world_transform(id).unwrap();
        let bounds = (world * res.local_path(doc, id).unwrap()).bounding_box();
        let at = |p: Point| {
            let w = world * g.transform * p;
            Vec2::new(
                (w.x - bounds.x0) / bounds.width(),
                (w.y - bounds.y0) / bounds.height(),
            )
        };
        (at(line.start), at(line.end))
    };
    let before = Resolved::rebuild(&f.doc);
    let (circle, square) = (
        ends(&f.doc, &before, f.circle),
        ends(&f.doc, &before, f.square),
    );
    let group = f.blend(FIVE);
    let after = Resolved::rebuild(&f.doc);
    for (k, step) in steps_of(&f, group).into_iter().enumerate() {
        // Steps 1 and 2 are nearer the circle, 3 to 5 the square.
        let want = if k < 2 { circle } else { square };
        let got = ends(&f.doc, &after, step);
        assert!(
            (got.0 - want.0).hypot() < 1e-6 && (got.1 - want.1).hypot() < 1e-6,
            "step {}'s ramp: {got:?} vs {want:?}",
            k + 1
        );
    }
}

/// An even-odd ring wound one way blended with a non-zero ring whose hole is
/// counter-wound keeps a hole in every step: each step takes the nearer shape's
/// rule and that shape's windings with it.
/// flip: `blend` stepping the upper half from the runs as turned → fails at "a hole
/// in step 3", the first to take the upper shape's rule.
#[test]
fn every_step_keeps_the_hole_both_ends_have() {
    use mastik_core::FillRule;
    use mastik_core::kurbo::BezPath;
    let square = |p: &mut BezPath, c: (f64, f64), r: f64, clockwise: bool| {
        let mut corners = [(-r, -r), (r, -r), (r, r), (-r, r)];
        if !clockwise {
            corners.reverse();
        }
        p.move_to((c.0 + corners[0].0, c.1 + corners[0].1));
        for (x, y) in &corners[1..] {
            p.line_to((c.0 + x, c.1 + y));
        }
        p.close_path();
    };
    let mut a_path = BezPath::new();
    square(&mut a_path, (100.0, 100.0), 50.0, true);
    square(&mut a_path, (100.0, 100.0), 20.0, true);
    let mut b_path = BezPath::new();
    square(&mut b_path, (400.0, 100.0), 50.0, true);
    square(&mut b_path, (400.0, 100.0), 20.0, false);
    let mut f = Fixture::new();
    let (a, b) = (f.ids.mint(), f.ids.mint());
    let path = |id, index, path: BezPath| Operation::CreateNode {
        id,
        parent: f.frame,
        index,
        kind: NodeKind::Path {
            path,
            corner_radii: Vec::new(),
            corner_smoothing: 0.0,
            mirror: Default::default(),
        },
        transform: None,
        name: None,
    };
    f.doc
        .apply(&Transaction(vec![
            path(a, 2, a_path),
            path(b, 3, b_path),
            Operation::SetFills {
                id: a,
                fills: filled(BLUE),
            },
            Operation::SetFills {
                id: b,
                fills: filled(ORANGE),
            },
            Operation::SetFillRule {
                id: a,
                rule: FillRule::EvenOdd,
            },
        ]))
        .unwrap();
    let res = Resolved::rebuild(&f.doc);
    let (tx, group) = blend::blend(&f.doc, &res, &mut f.ids, &[a, b], FIVE).expect("a blend");
    f.doc.apply(&tx).unwrap();
    let after = Resolved::rebuild(&f.doc);
    for (k, step) in steps_of(&f, group).into_iter().enumerate() {
        let n = f.doc.get(step).unwrap();
        let NodeKind::Path { path, .. } = n.kind() else {
            panic!("a path");
        };
        let world = after.world_transform(step).unwrap();
        // The middle of the step, in its own space.
        let mid = world.inverse() * Point::new(100.0 + 300.0 * (k + 1) as f64 / 6.0 + 50.0, 130.0);
        let w = path.winding(mid);
        let filled = match n.fill_rule() {
            FillRule::NonZero => w != 0,
            FillRule::EvenOdd => w % 2 != 0,
        };
        assert!(!filled, "a hole in step {}: winding {w}", k + 1);
    }
}

/// Step k of n sits k / (n + 1) of the way between the two centres, and grows
/// from one size to the other: the middle step of five is half way.
#[test]
fn the_middle_step_is_half_way_in_place_size_and_paint() {
    let mut f = Fixture::new();
    let group = f.blend(FIVE);
    let res = Resolved::rebuild(&f.doc);
    let middle = steps_of(&f, group)[2];
    let world = res.world_transform(middle).unwrap() * res.local_path(&f.doc, middle).unwrap();
    let b = world.bounding_box();
    // Centres (160, 290) and (663, 289) in world space; sizes 60 and 166.
    let want = Point::new((160.0 + 663.0) / 2.0, (290.0 + 289.0) / 2.0);
    assert!(
        (b.center() - want).hypot() < 0.5,
        "{:?} vs {want:?}",
        b.center()
    );
    assert!((b.width() - 113.0).abs() < 0.5, "{}", b.width());

    let n = f.doc.get(middle).unwrap();
    let Some(Brush::Solid(c)) = n.paint().fills[0].brush.literal() else {
        panic!("a solid fill");
    };
    let half = |x: u8, y: u8| (f32::from(x) + f32::from(y)) / 2.0 / 255.0;
    let [r, g, bl, _] = c.components;
    assert!((r - half(0x60, 0xd0)).abs() < 1e-3);
    assert!((g - half(0x80, 0xa0)).abs() < 1e-3);
    assert!((bl - half(0xd0, 0x50)).abs() < 1e-3);
    assert!((n.opacity() - 0.75).abs() < 1e-6, "opacity mixed");
}

/// Off, every step wears the first shape's paint and opacity.
#[test]
fn without_paint_every_step_wears_the_first_shapes() {
    let mut f = Fixture::new();
    let group = f.blend(Blend {
        paint: false,
        ..FIVE
    });
    for step in steps_of(&f, group) {
        let n = f.doc.get(step).unwrap();
        assert_eq!(
            n.paint().fills[0].brush.literal(),
            Some(&Brush::Solid(BLUE))
        );
        assert_eq!(n.opacity(), 0.5);
    }
}

/// Matching from another corner starts the circle's outline elsewhere, so the
/// steps differ; the first step's first anchor sits by the chosen corner.
#[test]
fn match_points_from_moves_where_the_outlines_pair_up() {
    let first_anchor = |start: Vec2| {
        let mut f = Fixture::new();
        let group = f.blend(Blend { start, ..FIVE });
        let step = steps_of(&f, group)[0];
        let NodeKind::Path { path, .. } = f.doc.get(step).unwrap().kind() else {
            panic!("a path");
        };
        match path.elements()[0] {
            mastik_core::kurbo::PathEl::MoveTo(p) => p,
            _ => panic!("starts with a move"),
        }
    };
    let top_left = first_anchor(Vec2::new(0.0, 0.0));
    let bottom_right = first_anchor(Vec2::new(1.0, 1.0));
    assert!(bottom_right.x > top_left.x && bottom_right.y > top_left.y);
}

/// A locked shape, and two operands of a boolean, are refused: in the boolean the
/// group would be one more operand.
#[test]
fn locked_shapes_and_boolean_operands_are_refused() {
    let mut f = Fixture::new();
    f.doc
        .apply(&Transaction(vec![Operation::SetLocked {
            id: f.square,
            locked: true,
        }]))
        .unwrap();
    assert_eq!(
        blend::pair(&f.doc, &[f.circle, f.square]),
        Err(Refusal::Locked)
    );

    let mut f = Fixture::new();
    let boolean = f.ids.mint();
    f.doc
        .apply(&Transaction(vec![
            Operation::CreateNode {
                id: boolean,
                parent: f.frame,
                index: 2,
                kind: NodeKind::Boolean {
                    op: mastik_core::node::BoolOp::Union,
                },
                transform: None,
                name: None,
            },
            Operation::Reparent {
                id: f.circle,
                new_parent: boolean,
                index: 0,
            },
            Operation::Reparent {
                id: f.square,
                new_parent: boolean,
                index: 1,
            },
        ]))
        .unwrap();
    assert_eq!(
        blend::pair(&f.doc, &[f.circle, f.square]),
        Err(Refusal::Inside)
    );
}

/// Text, pictures and any count but two are refused by name.
#[test]
fn text_pictures_and_other_counts_are_refused() {
    let mut f = Fixture::new();
    assert_eq!(blend::pair(&f.doc, &[f.circle]), Err(Refusal::Count));
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
        .unwrap();
    assert_eq!(blend::pair(&f.doc, &[f.circle, text]), Err(Refusal::Text));
    assert_eq!(Refusal::Text.says(), "Blend shapes can't take text");

    let picture = mastik_core::image_brush(mastik_core::ImageId("photo".into()));
    f.doc
        .apply(&Transaction(vec![Operation::SetFills {
            id: f.square,
            fills: keyed_by_position([Fill {
                brush: picture.into(),
                visible: true,
            }]),
        }]))
        .unwrap();
    assert_eq!(
        blend::pair(&f.doc, &[f.circle, f.square]),
        Err(Refusal::Picture)
    );
}

/// Two shapes inside a main are refused: the group would be a layer its instances
/// do not have.
/// flip: `pair` without its `component()` test → fails at the refusal.
#[test]
fn shapes_inside_a_main_are_refused() {
    let mut f = Fixture::new();
    f.doc
        .apply(&Transaction(vec![Operation::SetComponent {
            id: f.frame,
            component: true,
        }]))
        .unwrap();
    assert_eq!(
        blend::pair(&f.doc, &[f.circle, f.square]),
        Err(Refusal::Inside)
    );
}

/// An instance, and two shapes inside one, are refused: the group would move
/// the instance, or be a layer its main does not have.
/// flip: `pair` without its `node.link()` test → fails at "an instance
/// itself"; without both link tests → fails at "inside an instance" too.
#[test]
fn an_instance_and_shapes_inside_one_are_refused() {
    let mut f = Fixture::new();
    let (main, e1, e2) = (f.ids.mint(), f.ids.mint(), f.ids.mint());
    let dot = |id, index, x: f64| Operation::CreateNode {
        id,
        parent: main,
        index,
        kind: NodeKind::Ellipse {
            size: Size::new(10.0, 10.0),
            arc: Default::default(),
        },
        transform: Some(Affine::translate((x, 0.0))),
        name: None,
    };
    f.doc
        .apply(&Transaction(vec![
            Operation::CreateNode {
                id: main,
                parent: f.frame,
                index: 2,
                kind: NodeKind::Group,
                transform: None,
                name: None,
            },
            dot(e1, 0, 0.0),
            dot(e2, 1, 40.0),
            Operation::SetComponent {
                id: main,
                component: true,
            },
        ]))
        .unwrap();
    let place = mastik_core::Placement {
        nodes: f.doc.capture_subtree(main).unwrap(),
        parent: f.frame,
        index: None,
    };
    let (tx, made) = mastik_core::insert_subtrees(&f.doc, &mut f.ids, &[place], Default::default());
    f.doc.apply(&tx).unwrap();
    let instance = made[0];
    let copies = f.doc.get(instance).unwrap().children().to_vec();
    assert!(
        f.doc.get(instance).unwrap().link().is_some() && copies.len() == 2,
        "the fixture: an instance of two shapes"
    );
    assert_eq!(
        blend::pair(&f.doc, &[f.circle, instance]),
        Err(Refusal::Inside),
        "an instance itself"
    );
    assert_eq!(
        blend::pair(&f.doc, &copies),
        Err(Refusal::Inside),
        "inside an instance"
    );
}

/// Two shapes in an auto layout are refused, directly or through a plain group:
/// moved into the *Blend* group they would leave the flow and lose their stretch.
/// flip: `pair` without its layout test → fails at "in a row"; asking only the
/// direct parent → fails at "in a group in a row".
#[test]
fn shapes_in_an_auto_layout_are_refused() {
    let mut f = Fixture::new();
    let flex = Some(mastik_core::container::Display::Flex(Default::default()));
    f.doc
        .apply(&Transaction(vec![Operation::SetDisplay {
            id: f.frame,
            display: flex,
        }]))
        .unwrap();
    assert_eq!(
        blend::pair(&f.doc, &[f.circle, f.square]),
        Err(Refusal::Layout),
        "in a row"
    );
    let res = Resolved::rebuild(&f.doc);
    let made = blend::blend(&f.doc, &res, &mut f.ids, &[f.circle, f.square], FIVE);
    assert_eq!(made.err(), Some(Refusal::Layout));

    let res = Resolved::rebuild(&f.doc);
    let (tx, _) =
        mastik_core::build::group(&f.doc, &res, &mut f.ids, &[f.circle, f.square]).unwrap();
    f.doc.apply(&tx).unwrap();
    assert_eq!(
        blend::pair(&f.doc, &[f.circle, f.square]),
        Err(Refusal::Layout),
        "in a group in a row"
    );
}

/// A mask between the two shapes in their parent is refused: the lower shape,
/// moved into the group above it, would fall under its clip.
/// flip: `pair` without its test of the siblings between → fails at the refusal.
#[test]
fn a_mask_between_the_shapes_is_refused() {
    let mut f = Fixture::new();
    let mask = f.ids.mint();
    f.doc
        .apply(&Transaction(vec![
            Operation::CreateNode {
                id: mask,
                parent: f.frame,
                index: 1,
                kind: NodeKind::Rect {
                    size: Size::new(100.0, 100.0),
                    corner_radii: Default::default(),
                    corner_smoothing: 0.0,
                },
                transform: None,
                name: None,
            },
            Operation::SetMask {
                id: mask,
                mask: true,
            },
        ]))
        .unwrap();
    assert_eq!(
        f.doc.get(f.frame).unwrap().children(),
        &[f.circle, mask, f.square],
        "the fixture: the mask stands between"
    );
    assert_eq!(
        blend::pair(&f.doc, &[f.circle, f.square]),
        Err(Refusal::MaskBetween)
    );
    f.doc
        .apply(&Transaction(vec![Operation::SetMask {
            id: mask,
            mask: false,
        }]))
        .unwrap();
    assert!(blend::pair(&f.doc, &[f.circle, f.square]).is_ok());
}

/// *Reverse the second shape* runs it the other way: the middle step's first
/// anchor stays, and its second comes from the square's other side.
/// flip: `matched` ignoring `Blend::reverse` → fails at "a different step".
#[test]
fn reverse_runs_the_second_shape_the_other_way() {
    let first = |reverse: bool| {
        let mut f = Fixture::new();
        let group = f.blend(Blend { reverse, ..FIVE });
        let NodeKind::Path { path, .. } = f.doc.get(steps_of(&f, group)[2]).unwrap().kind() else {
            panic!("a path");
        };
        path.clone()
    };
    let (plain, reversed) = (first(false), first(true));
    assert_ne!(plain, reversed, "a different step");
    let start = |p: &mastik_core::kurbo::BezPath| match p.elements()[0] {
        mastik_core::kurbo::PathEl::MoveTo(p) => p,
        _ => panic!("starts with a move"),
    };
    assert!(
        (start(&plain) - start(&reversed)).hypot() < 1e-6,
        "the first anchors still pair"
    );
}

/// Where the two shapes hold different numbers of fills, a step takes the
/// nearer shape's list unmixed.
/// flip: `look_between` mixing without its equal-count test → panics indexing
/// past the shorter list (or, read with `get`, fails at the step's colour).
#[test]
fn fills_of_different_counts_are_not_mixed() {
    let mut f = Fixture::new();
    let two = keyed_by_position([ORANGE, ORANGE].map(|c| Fill {
        brush: Brush::Solid(c).into(),
        visible: true,
    }));
    f.doc
        .apply(&Transaction(vec![Operation::SetFills {
            id: f.square,
            fills: two,
        }]))
        .unwrap();
    let group = f.blend(FIVE);
    let first = f.doc.get(steps_of(&f, group)[0]).unwrap();
    let colours: Vec<_> = first
        .paint()
        .fills
        .iter()
        .map(|x| x.value.brush.literal().cloned())
        .collect();
    assert_eq!(
        colours,
        vec![Some(Brush::Solid(BLUE))],
        "the circle's one fill"
    );
}

/// No steps, or more than the most, is refused.
#[test]
fn the_step_count_is_bounded() {
    let mut f = Fixture::new();
    let res = Resolved::rebuild(&f.doc);
    for steps in [0, blend::MAX_STEPS + 1] {
        let made = blend::blend(
            &f.doc,
            &res,
            &mut f.ids,
            &[f.circle, f.square],
            Blend { steps, ..FIVE },
        );
        assert_eq!(made.err(), Some(Refusal::Count), "{steps}");
    }
}

/// Of two shapes that each draw at twice their own units, a step's stroke draws as
/// wide as theirs did: its width times its world scale is their drawn width.
/// flip: `blend` carrying widths with a factor of 1 → fails at the width.
#[test]
fn a_stroke_draws_as_wide_from_scaled_shapes() {
    use mastik_core::Stroke;
    let mut f = Fixture::new();
    let stroked = |id| Operation::SetStrokes {
        id,
        strokes: keyed_by_position([Stroke {
            brush: Brush::Solid(BLUE).into(),
            width: 4.0,
            ..Default::default()
        }]),
    };
    let doubled = |id, at: (f64, f64)| Operation::SetTransform {
        id,
        transform: Affine::translate(at) * Affine::scale(2.0),
    };
    f.doc
        .apply(&Transaction(vec![
            doubled(f.circle, (80.0, 230.0)),
            doubled(f.square, (430.0, 76.0)),
            stroked(f.circle),
            stroked(f.square),
        ]))
        .unwrap();
    let group = f.blend(FIVE);
    let res = Resolved::rebuild(&f.doc);
    for step in steps_of(&f, group) {
        let width = f.doc.get(step).unwrap().paint().strokes[0].width;
        let scale = res.world_transform(step).unwrap().determinant().sqrt();
        assert!((width * scale - 8.0).abs() < 1e-9, "{width} × {scale}");
    }
}

impl Fixture {
    /// A path in the frame, in its coordinates, above the two shapes.
    fn spine(&mut self, path: mastik_core::kurbo::BezPath) -> NodeId {
        let id = self.ids.mint();
        self.doc
            .apply(&Transaction(vec![Operation::CreateNode {
                id,
                parent: self.frame,
                index: 2,
                kind: NodeKind::Path {
                    path,
                    corner_radii: Vec::new(),
                    corner_smoothing: 0.0,
                    mirror: Default::default(),
                },
                transform: None,
                name: Some("Spine".into()),
            }]))
            .unwrap();
        id
    }
}

/// The frame-local polyline through `points`, closed when `closed`.
fn polyline(points: &[(f64, f64)], closed: bool) -> mastik_core::kurbo::BezPath {
    let mut p = mastik_core::kurbo::BezPath::new();
    p.move_to(points[0]);
    for q in &points[1..] {
        p.line_to(*q);
    }
    if closed {
        p.close_path();
    }
    p
}

/// Each of the group's layers' world centres, the circle first and the square last.
fn centres(f: &Fixture, group: NodeId) -> Vec<Point> {
    let res = Resolved::rebuild(&f.doc);
    f.doc
        .get(group)
        .unwrap()
        .children()
        .iter()
        .map(|id| res.world_bounds(*id).unwrap().center())
        .collect()
}

fn close_to(a: Point, b: Point) -> bool {
    a.distance(b) < 1e-3
}

/// The angle a layer's world transform turns by, in degrees.
fn turned(f: &Fixture, id: NodeId) -> f64 {
    let c = Resolved::rebuild(&f.doc)
        .world_transform(id)
        .unwrap()
        .as_coeffs();
    c[1].atan2(c[0]).to_degrees()
}

/// Along an open spine the circle stands on its start and the square on its end,
/// by their centres, with the steps evenly spaced between by arc length; the spine
/// stays its own layer, where it was on the page, directly under the group.
/// flip: `places` spacing over `count` gaps on an open spine → fails at "the square
/// on the end".
#[test]
fn an_open_spine_puts_the_shapes_on_its_ends_and_spaces_the_steps_evenly() {
    let mut f = Fixture::new();
    // An L: 300 along, then 300 down, so the middle item sits on the corner.
    let spine = f.spine(polyline(
        &[(100.0, 50.0), (400.0, 50.0), (400.0, 350.0)],
        false,
    ));
    let before = Resolved::rebuild(&f.doc).world_bounds(spine);
    let group = f.blend(Blend {
        spine: Some(spine),
        ..FIVE
    });
    let at = centres(&f, group);
    assert_eq!(at.len(), 7);
    // The frame stands at (50, 30); the spine is 600 long, so each gap is 100.
    let world = |x: f64, y: f64| Point::new(x + 50.0, y + 30.0);
    assert!(
        close_to(at[0], world(100.0, 50.0)),
        "the circle on the start: {at:?}"
    );
    assert!(
        close_to(at[6], world(400.0, 350.0)),
        "the square on the end: {at:?}"
    );
    assert!(
        close_to(at[3], world(400.0, 50.0)),
        "the middle step on the corner: {at:?}"
    );
    assert!(close_to(at[1], world(200.0, 50.0)), "a gap of 100: {at:?}");
    assert_eq!(
        f.doc.get(f.frame).unwrap().children(),
        &[spine, group],
        "the spine directly under the group"
    );
    assert_eq!(
        Resolved::rebuild(&f.doc).world_bounds(spine),
        before,
        "and unchanged on the page"
    );
    assert!(
        turned(&f, f.square).abs() < 1e-9,
        "turning is off by default"
    );
}

/// Turned, each item takes the spine's direction relative to its start: the
/// circle stays as it faced, and the square, on the end of an L whose second
/// leg runs down, turns a quarter.
/// flip: `places` measuring each angle without subtracting the start's → fails
/// at "the circle keeps".
#[test]
fn turning_follows_the_tangent_from_where_the_spine_starts() {
    let mut f = Fixture::new();
    // Starting downwards, so an absolute angle would turn the circle too.
    let spine = f.spine(polyline(
        &[(100.0, 50.0), (100.0, 350.0), (400.0, 350.0)],
        false,
    ));
    f.blend(Blend {
        spine: Some(spine),
        turn: true,
        ..FIVE
    });
    assert!(
        turned(&f, f.circle).abs() < 1e-6,
        "the circle keeps its facing"
    );
    assert!(
        (turned(&f, f.square) + 90.0).abs() < 1e-6,
        "the square turns from down to right: {}",
        turned(&f, f.square)
    );
}

/// On a closed spine the items share the whole loop, the square one gap short of
/// the start; *Reverse the path* runs them round the other way.
/// flip: `places` treating a closed spine as open → fails at "one gap short".
#[test]
fn a_closed_spine_spaces_the_items_round_the_loop() {
    let mut f = Fixture::new();
    // A 280-square: 1120 round, so seven items stand 160 apart.
    let ring = polyline(
        &[(100.0, 50.0), (380.0, 50.0), (380.0, 330.0), (100.0, 330.0)],
        true,
    );
    let spine = f.spine(ring);
    let world = |x: f64, y: f64| Point::new(x + 50.0, y + 30.0);
    let group = f.blend(Blend {
        spine: Some(spine),
        ..FIVE
    });
    let at = centres(&f, group);
    assert!(close_to(at[0], world(100.0, 50.0)), "{at:?}");
    assert!(
        close_to(at[6], world(100.0, 210.0)),
        "one gap short of the start: {at:?}"
    );

    let mut g = Fixture::new();
    let spine = g.spine(polyline(
        &[(100.0, 50.0), (380.0, 50.0), (380.0, 330.0), (100.0, 330.0)],
        true,
    ));
    let group = g.blend(Blend {
        spine: Some(spine),
        spine_reverse: true,
        ..FIVE
    });
    let at = centres(&g, group);
    assert!(
        close_to(at[1], world(100.0, 210.0)),
        "the other way round: {at:?}"
    );
}

/// Three layers where one is a path: that path is the spine, the other two are
/// blended, and the row says nothing against it. Two paths among the three cannot
/// tell which is followed and are refused.
/// flip: `pair` keeping the spine among the members → fails at "two shapes and a
/// spine".
#[test]
fn a_path_selected_with_the_two_is_the_spine() {
    let mut f = Fixture::new();
    let spine = f.spine(polyline(&[(100.0, 50.0), (400.0, 50.0)], false));
    let three = [f.square, spine, f.circle];
    assert_eq!(blend::spine_in(&f.doc, &three), Some(spine));
    assert_eq!(
        blend::pair(&f.doc, &three),
        Ok((f.circle, f.square)),
        "two shapes and a spine"
    );
    let res = Resolved::rebuild(&f.doc);
    assert_eq!(blend::refusal(&f.doc, &res, &three), None);

    let other = f.spine(polyline(&[(100.0, 80.0), (400.0, 80.0)], false));
    let paths = [f.circle, spine, other];
    assert_eq!(blend::spine_in(&f.doc, &paths), None);
    assert_eq!(blend::pair(&f.doc, &paths), Err(Refusal::Count));
    assert_eq!(
        Refusal::Count.says(),
        "Blend shapes needs two shapes, and a path to follow if you like"
    );
}

/// A spine that is one of the two shapes, is not a path, or has no length is
/// refused.
#[test]
fn a_spine_must_be_a_path_with_length_apart_from_the_two() {
    let mut f = Fixture::new();
    let dot = f.spine(polyline(&[(100.0, 50.0), (100.0, 50.0)], false));
    let res = Resolved::rebuild(&f.doc);
    let pair = (f.circle, f.square);
    for (spine, why) in [
        (f.circle, "a shape"),
        (f.frame, "a frame"),
        (dot, "no length"),
    ] {
        assert_eq!(
            blend::spine_refusal(&f.doc, &res, spine, pair),
            Some(Refusal::Spine),
            "{why}"
        );
    }
}
