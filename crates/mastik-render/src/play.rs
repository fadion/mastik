//! Drawing a prototype's play [`Scene`] through the one scene walk, so every shell
//! (the app's player, the exported page, a video from a take) draws a frame of
//! play the same way.
//!
//! The scene is drawn in **screen space**: the viewport's view is the screen
//! frame's box with its top-left at the origin. Each layer's frame is moved there
//! by an override on its own transform, drawn with [`scene::build_of`] inside a
//! layer clipped to the frame's box and faded by the layer's opacity, over the
//! layer's backdrop.

use crate::renderer::{RenderOverrides, Viewport};
use crate::scene::{self, ClipRule, ScenePainter};
use kurbo::{Affine, Rect, Shape};
use mastik_core::{Brush, Document, Resolved};
use mastik_play::Scene;

/// The viewport that draws `scene` at `scale` device pixels per screen unit: the
/// screen's box from the origin, at least one pixel each way.
pub fn viewport(scene: &Scene, scale: f64) -> Viewport {
    let px = |v: f64| ((v * scale).round() as u32).max(1);
    Viewport {
        view: Rect::from_origin_size((0.0, 0.0), scene.size),
        pixel_size: (px(scene.size.width), px(scene.size.height)),
    }
}

/// The preview `scene`'s patches make over the played copy, before any layer is
/// placed: what [`build`] takes as `patches`.
pub fn overrides(doc: &Document, res: &Resolved, scene: &Scene) -> RenderOverrides {
    RenderOverrides::from_play(doc, res, &scene.patches)
}

/// Walk `scene`'s layers bottom first and drive `painter`. `patches` is
/// [`overrides`] of the same scene; `vp` is [`viewport`]'s, or any view of the
/// screen space.
pub fn build<P: ScenePainter>(
    doc: &Document,
    res: &Resolved,
    vp: &Viewport,
    patches: &RenderOverrides,
    scene: &Scene,
    painter: &mut P,
) {
    let screen = Rect::from_origin_size((0.0, 0.0), scene.size);
    for layer in &scene.layers {
        let Some(node) = doc.get(layer.frame) else {
            continue;
        };
        if let Some(colour) = layer.backdrop {
            painter.fill_path(
                Affine::IDENTITY,
                &screen.to_path(0.1),
                &Brush::Solid(colour),
                None,
                ClipRule::NonZero,
            );
        }
        // The frame's parent is drawn nowhere here, so the frame's local
        // transform is chosen to put its top-left at `at` in screen space.
        let parent = node
            .parent()
            .and_then(|p| patches.world_transform(doc, res, p))
            .unwrap_or(Affine::IDENTITY);
        let mut ov = patches.clone();
        ov.place(
            doc,
            layer.frame,
            parent.inverse() * Affine::translate(layer.at.to_vec2()),
        );
        let size = ov
            .get(layer.frame)
            .and_then(|o| o.kind.as_ref())
            .or_else(|| res.used_kind(doc, layer.frame))
            .and_then(|k| k.box_size())
            .unwrap_or(scene.size);
        let clip = Rect::from_origin_size(layer.at, size).to_path(0.1);
        painter.push_layer(
            Affine::IDENTITY,
            Some(&clip),
            ClipRule::NonZero,
            layer.opacity,
        );
        scene::build_of(doc, res, vp, &ov, &[layer.frame], painter);
        painter.pop_layer();
    }
}
