//! The Alt-hover measure — the distances between what is selected and what the
//! pointer is over, in either direction.
//!
//! Hold `Alt` with something selected and hover another layer, or a ruler guide,
//! and the app quotes the distances between the two on both axes. The geometry
//! is `mastik_core::measure`, which the exported player's inspect mode quotes
//! too; canvas.rs decides what to compare and draws what comes back.

pub use mastik_core::measure::*;

/// The air around a measure's label pill, tighter than `ui::PILL_PAD`: the
/// pill sits centred on the line it measures, so a short gap stays readable.
pub const PILL_PAD: eframe::egui::Vec2 = eframe::egui::vec2(4.0, 2.0);

#[cfg(test)]
mod tests {
    use super::*;
    use eframe::egui;

    /// The label pill stays narrow enough to centre on its own line: a measure
    /// shorter than the pill is covered by its label and only the end ticks show,
    /// a fine trade only while that is a tight gap. The bounds are for
    /// `ui::BADGE_PT` with `PILL_PAD`; re-check them if either moves.
    #[test]
    fn the_label_pill_is_narrow_enough_to_centre_on_its_own_line() {
        let ctx = egui::Context::default();
        crate::theme::install(&ctx);
        // One pass so the fonts exist to lay out with.
        let _ = ctx.run_ui(Default::default(), |_| {});

        let width = |text: &str| {
            ctx.fonts_mut(|f| {
                f.layout_no_wrap(
                    text.to_owned(),
                    egui::FontId::proportional(crate::ui::BADGE_PT),
                    egui::Color32::WHITE,
                )
                .size()
                .x
            }) + 2.0 * super::PILL_PAD.x
        };

        let two = width(&label(40.0));
        assert!((18.0..=23.0).contains(&two), "a two-digit pill is {two}px");
        // The widest labels a whole and a fractional number produce.
        let four = width(&label(1200.0));
        let frac = width(&label(40.25));
        assert!(four <= 32.0, "a four-digit pill is {four}px");
        assert!(frac <= 38.0, "a fractional pill is {frac}px");
        // One digit is the tight case, carried by the ticks.
        let one = width(&label(8.0));
        assert!(one <= 15.0, "a one-digit pill is {one}px");
    }
}
