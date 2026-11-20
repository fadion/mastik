//! A play `Scene` drawn through `mastik_render::play`: each layer's frame lands at
//! its place in screen space, wherever it sits on the page.

use mastik_core::Brush;
use mastik_core::kurbo::{Affine, Point, RoundedRectRadii, Size};
use mastik_core::peniko::Color;
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
