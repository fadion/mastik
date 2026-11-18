//! Pictures of a noise fill for the chrome: the picker's preview and a paint
//! row's chip, drawn by the CPU pass the PNG export draws with, so the chrome
//! shows what the layer shows.

use mastik_core::Brush;
use mastik_render::scene::NoiseRamp;

/// A texture of `brush`'s noise fill, `size` points, one point one document unit
/// from the layer's top-left: what the layer draws at 100%. `None` for a brush
/// that is no noise fill.
///
/// Held in egui's memory under `owner`, one texture per use site, beside a key
/// of everything it is drawn from: a frame that changes nothing makes nothing,
/// and a changed noise replaces the old texture rather than piling up beside it.
pub fn texture(
    ctx: &egui::Context,
    owner: egui::Id,
    brush: &Brush,
    size: egui::Vec2,
) -> Option<egui::TextureId> {
    let ramp = NoiseRamp::of(brush)?;
    let ppp = ctx.pixels_per_point();
    let (w, h) = (
        (size.x * ppp).round().max(1.0) as usize,
        (size.y * ppp).round().max(1.0) as usize,
    );
    let key = key_of(&ramp, w, h, ppp);
    if let Some((k, tex)) = ctx.data(|d| d.get_temp::<(u64, egui::TextureHandle)>(owner))
        && k == key
    {
        return Some(tex.id());
    }
    let mut px = vec![255u8; w * h * 4];
    let mut surface = mastik_render::effects::Surface::new(&mut px, w, h);
    mastik_render::effects::noise_fill(
        &mut surface,
        &ramp.noise,
        ramp.from,
        ramp.to,
        mastik_core::kurbo::Affine::scale(1.0 / f64::from(ppp)),
    );
    let image = egui::ColorImage::from_rgba_premultiplied([w, h], &px);
    let tex = ctx.load_texture(
        format!("noise-{key:x}"),
        image,
        egui::TextureOptions::NEAREST,
    );
    let id = tex.id();
    ctx.data_mut(|d| d.insert_temp(owner, (key, tex)));
    Some(id)
}

/// A hash of every input a noise texture is drawn from.
fn key_of(r: &NoiseRamp, w: usize, h: usize, ppp: f32) -> u64 {
    use std::hash::{Hash, Hasher};
    let mut s = std::collections::hash_map::DefaultHasher::new();
    (r.noise.kind, r.noise.detail, r.noise.seed, w, h).hash(&mut s);
    (
        r.noise.scale.to_bits(),
        r.noise.stretch.to_bits(),
        ppp.to_bits(),
    )
        .hash(&mut s);
    for c in r.from.iter().chain(&r.to) {
        c.to_bits().hash(&mut s);
    }
    s.finish()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A preview is drawn once per input: the same noise answers the same
    /// texture, and a changed seed a new one in the same place.
    ///
    /// flip: `texture` ignoring the stored key → fails at "the same noise".
    #[test]
    fn a_preview_is_drawn_once_per_noise() {
        let ctx = egui::Context::default();
        let owner = egui::Id::new("probe");
        let brush = |seed| {
            Brush::Gradient(mastik_core::GradientBrush::noise(
                mastik_core::Noise {
                    seed,
                    ..Default::default()
                },
                mastik_core::peniko::Color::WHITE,
                mastik_core::peniko::Color::BLACK,
            ))
        };
        let size = egui::vec2(40.0, 20.0);
        let a = texture(&ctx, owner, &brush(1), size).expect("a noise fill");
        assert_eq!(
            texture(&ctx, owner, &brush(1), size),
            Some(a),
            "the same noise"
        );
        assert_ne!(texture(&ctx, owner, &brush(2), size), Some(a));
        assert_eq!(
            texture(
                &ctx,
                owner,
                &Brush::Solid(mastik_core::peniko::Color::WHITE),
                size
            ),
            None
        );
    }
}
