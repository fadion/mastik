//! A play `Scene` drawn through `mastik_render::play`: each layer's frame lands at
//! its place in screen space, wherever it sits on the page.

use mastik_core::Brush;
use mastik_core::kurbo::{Affine, Point, RoundedRectRadii, Size};
use mastik_core::peniko::{Color, Gradient};
use mastik_core::prototype::{
    Action, Direction, Easing, Reaction, Timing, Transition, TransitionKind, Trigger,
};
use mastik_core::{
    Document, Fill, IdSource, Keyed, NodeId, NodeKind, Operation, Transaction, keyed_by_position,
};
use mastik_play::{Input, Player, Start};
use mastik_render::{ImageStore, VelloCpuRenderer};

fn frame(doc: &mut Document, ids: &mut IdSource, page: NodeId, x: f64, rgb: [u8; 3]) -> NodeId {
    let id = ids.mint();
    let index = doc.get(page).map_or(0, |n| n.children().len());
    doc.apply(&Transaction(vec![
        Operation::CreateNode {
            id,
            parent: page,
            index,
            kind: NodeKind::Artboard {
                size: Size::new(100.0, 100.0),
                corner_radii: RoundedRectRadii::default(),
                corner_smoothing: 0.0,
            },
            transform: Some(Affine::translate((x, 0.0))),
            name: Some("Screen".into()),
        },
        Operation::SetFills {
            id,
            fills: keyed_by_position([Fill {
                brush: Brush::Solid(Color::from_rgba8(rgb[0], rgb[1], rgb[2], 255)).into(),
                visible: true,
            }]),
        },
    ]))
    .expect("a frame");
    id
}

fn pixel(p: &Player) -> [u8; 4] {
    let (rgba, w, _) = VelloCpuRenderer::new().render_play_to_rgba(
        p.document(),
        p.resolved(),
        &ImageStore::new(),
        &p.scene(),
        1.0,
    );
    let i = ((50 * w + 50) * 4) as usize;
    [rgba[i], rgba[i + 1], rgba[i + 2], rgba[i + 3]]
}

/// A frame far from the page's origin draws at the screen's origin, before and
/// after a navigation's move-in lands.
/// flip: `play::build` placing the frame at its own world position → fails at the blue check.
#[test]
fn each_layer_draws_at_its_place_in_screen_space() {
    let mut ids = IdSource::new(0x7E57);
    let page = ids.mint();
    let mut doc = Document::new(page);
    let a = frame(&mut doc, &mut ids, page, 0.0, [255, 0, 0]);
    let b = frame(&mut doc, &mut ids, page, 500.0, [0, 0, 255]);
    let navigate = Action::Navigate {
        to: b,
        transition: Transition {
            kind: TransitionKind::MoveIn,
            direction: Direction::Left,
            timing: Timing::Ease {
                curve: Easing::Linear,
                ms: 100,
            },
        },
        keep_scroll: false,
    };
    doc.apply(&Transaction(vec![Operation::SetReactions {
        id: a,
        reactions: vec![Keyed::new(
            ids.mint_item(),
            Reaction {
                trigger: Trigger::Click,
                actions: vec![Keyed::new(ids.mint_item(), navigate)],
            },
        )],
    }]))
    .expect("a reaction");

    let mut p = Player::new(&doc, Start::Frame(a), 0x91A7).expect("plays");
    assert_eq!(
        pixel(&p),
        [255, 0, 0, 255],
        "the start frame fills the screen"
    );
    let at = Point::new(50.0, 50.0);
    p.input(Input::PointerMove(at));
    p.input(Input::PointerDown(at));
    p.input(Input::PointerUp(at));
    p.tick(200.0);
    assert_eq!(p.screen(), b, "the click navigated");
    assert_eq!(
        pixel(&p),
        [0, 0, 255, 255],
        "the frame at x = 500 draws at the origin"
    );
}

/// A play patch on a layer's corner radii keeps its gradient on the box layout
/// gives it: a rect stored 50 wide, pinned left and right to a 100-wide screen,
/// draws the same ramp with and without the patch.
/// flip: the scene walk taking a laid-out override kind for the stored box →
/// fails at "the ramp spans the stretched box".
#[test]
fn a_radii_patch_keeps_a_stretched_gradient_stretched() {
    let mut ids = IdSource::new(0x7E57);
    let page = ids.mint();
    let mut doc = Document::new(page);
    let screen = frame(&mut doc, &mut ids, page, 0.0, [255, 0, 0]);
    let rect = ids.mint();
    let ramp = Gradient::new_linear(Point::new(0.0, 0.0), Point::new(50.0, 0.0)).with_stops([
        (0.0_f32, Color::from_rgba8(0, 0, 0, 255)),
        (1.0_f32, Color::from_rgba8(255, 255, 255, 255)),
    ]);
    let px = |v| Some(mastik_core::LengthPct::Px(v));
    doc.apply(&Transaction(vec![
        Operation::CreateNode {
            id: rect,
            parent: screen,
            index: 0,
            kind: NodeKind::Rect {
                size: Size::new(50.0, 100.0),
                corner_radii: RoundedRectRadii::default(),
                corner_smoothing: 0.0,
            },
            transform: None,
            name: None,
        },
        Operation::SetFills {
            id: rect,
            fills: keyed_by_position([Fill {
                brush: Brush::Gradient(ramp.into()).into(),
                visible: true,
            }]),
        },
        Operation::SetInsets {
            id: rect,
            insets: mastik_core::Insets {
                left: px(0.0),
                right: px(0.0),
                ..Default::default()
            },
        },
    ]))
    .expect("a stretched rect");
    let p = Player::new(&doc, Start::Frame(screen), 0x91A7).expect("plays");
    let draw = |scene: &mastik_play::Scene| {
        let (rgba, w, _) = VelloCpuRenderer::new().render_play_to_rgba(
            p.document(),
            p.resolved(),
            &ImageStore::new(),
            scene,
            1.0,
        );
        let i = ((50 * w + 75) * 4) as usize;
        rgba[i]
    };
    let plain = p.scene();
    let still = draw(&plain);
    assert!(
        (150..230).contains(&still),
        "fixture: three quarters along the stretched ramp, got {still}"
    );
    let mut patched = plain.clone();
    let mut radii = mastik_play::Patch::new(rect);
    radii.radii = Some(RoundedRectRadii::from_single_radius(4.0));
    patched.patches.push(radii);
    assert_eq!(draw(&patched), still, "the ramp spans the stretched box");
}

/// Counts the fills a walk emits, by the brush's colour.
#[derive(Default)]
struct Fills(Vec<Brush>);

impl mastik_render::scene::ScenePainter for Fills {
    fn has_image(&self, _id: &mastik_core::ImageId) -> bool {
        true
    }
    fn fill_path(
        &mut self,
        _t: Affine,
        _path: &mastik_core::kurbo::BezPath,
        brush: &Brush,
        _framing: Option<mastik_core::Framing>,
        _rule: mastik_render::scene::ClipRule,
    ) {
        self.0.push(brush.clone());
    }
    fn stroke_path(
        &mut self,
        _t: Affine,
        _path: &mastik_core::kurbo::BezPath,
        _s: &mastik_render::scene::StrokePaint<'_>,
    ) {
    }
    fn draw_text(&mut self, _t: Affine, _run: &mastik_render::scene::TextRun<'_>) {}
    fn push_layer(
        &mut self,
        _t: Affine,
        _clip: Option<&mastik_core::kurbo::BezPath>,
        _rule: mastik_render::scene::ClipRule,
        _opacity: f32,
    ) {
    }
    fn push_mask_layer(&mut self) {}
    fn push_luminance_mask_layer(&mut self) {}
    fn push_effect_layer(
        &mut self,
        _t: Affine,
        _bounds: mastik_core::kurbo::Rect,
        _effects: &[Keyed<mastik_core::Effect>],
    ) {
    }
    fn push_blend_layer(&mut self, _mode: mastik_core::BlendMode) {}
    fn pop_layer(&mut self) {}
}

/// A scrolled feed draws only the rows on the screen: rows scrolled past are
/// culled by their bounds carried through the placement and the scroll, and a
/// row straddling the screen's bottom edge still draws.
/// flip: `RenderOverrides::placed_bounds` answering `None` → fails at "only the
/// rows on the screen".
#[test]
fn a_scrolled_feed_draws_only_what_is_on_screen() {
    use mastik_core::kurbo::Vec2;
    use mastik_core::prototype::{LayerPlay, ScrollAxis};
    let mut ids = IdSource::new(0x7E57);
    let page = ids.mint();
    let mut doc = Document::new(page);
    // Far from the page's origin, so the placement moves it.
    let screen = frame(&mut doc, &mut ids, page, 700.0, [255, 255, 255]);
    let mut ops = vec![Operation::SetPlay {
        id: screen,
        play: LayerPlay {
            scroll: ScrollAxis::Vertical,
            ..LayerPlay::default()
        },
    }];
    // Forty rows 20 tall, each its own colour by its index in the red channel.
    for i in 0..40u8 {
        let id = ids.mint();
        ops.push(Operation::CreateNode {
            id,
            parent: screen,
            index: usize::from(i),
            kind: NodeKind::Rect {
                size: Size::new(100.0, 20.0),
                corner_radii: RoundedRectRadii::default(),
                corner_smoothing: 0.0,
            },
            transform: Some(Affine::translate((0.0, f64::from(i) * 20.0))),
            name: Some(format!("Row {i}")),
        });
        ops.push(Operation::SetFills {
            id,
            fills: keyed_by_position([Fill {
                brush: Brush::Solid(Color::from_rgba8(i, 0, 200, 255)).into(),
                visible: true,
            }]),
        });
    }
    doc.apply(&Transaction(ops)).expect("a feed");
    let mut p = Player::new(&doc, Start::Frame(screen), 0x91A7).expect("plays");
    // 210 down: rows 10 to 15 show, row 15 cut by the bottom edge.
    p.input(Input::Wheel {
        at: Point::new(50.0, 50.0),
        delta: Vec2::new(0.0, 210.0),
    });
    assert_eq!(
        p.scroll_offset(screen),
        Vec2::new(0.0, 210.0),
        "fixture: scrolled"
    );
    let scene = p.scene();
    let (doc, res) = (p.document(), p.resolved());
    let vp = mastik_render::play::viewport(&scene, 1.0);
    let patches = mastik_render::play::overrides(doc, res, &scene);
    let mut fills = Fills::default();
    mastik_render::play::build(doc, res, &vp, &patches, &scene, &mut fills);
    let rows: Vec<u8> = fills
        .0
        .iter()
        .filter_map(|b| match b {
            Brush::Solid(c) if c.to_rgba8().b == 200 => Some(c.to_rgba8().r),
            _ => None,
        })
        .collect();
    assert_eq!(
        rows,
        (10..=15).collect::<Vec<_>>(),
        "only the rows on the screen"
    );
}
