//! The Distort card: the path bar's Distort button opens it beside the bar, where
//! Clean up and Offset open theirs. Three rows, *Roughen*, *Zig-zag* and *Pucker /
//! Bloat* ([`mastik_core::distort`]), each shown on the canvas while its button is
//! hovered or focused or a field of its scrubbed, with the points it adds marked,
//! and each committed by its button as an undo step of its own. With points
//! selected a filter acts on the segments between them.
//!
//! Its own card rather than more Clean up rows: Clean up makes a path tidier and
//! these make it rougher.

use crate::app::MastikApp;
use crate::theme::{self, icon};
use crate::ui::{self, FieldButton, Prefix, Scrub, Suffix};
use eframe::egui;
use mastik_core::distort::Filter;
use mastik_core::kurbo::Point;
use mastik_core::{NodeId, NodeKind, Transaction};

/// The card's width: a popover's.
const W: f32 = crate::app::POPOVER_W;

/// The card's numbers, kept while it is closed so it reopens on the last ones.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct DistortNumbers {
    /// The furthest *Roughen* moves a point, in px.
    pub(crate) rough_size: f64,
    /// How many points *Roughen* adds before moving them, `0..=100` %.
    pub(crate) detail: f64,
    /// A *Zig-zag* ridge's height, crest to trough, in px.
    pub(crate) zig_size: f64,
    /// How many ridges each segment gets.
    pub(crate) ridges: f64,
    /// *Pucker / Bloat*'s amount, `-100..=100` %.
    pub(crate) amount: f64,
    /// Whether the points Roughen and Zig-zag add are smooth.
    pub(crate) smooth: bool,
}

impl Default for DistortNumbers {
    fn default() -> Self {
        Self {
            rough_size: 6.0,
            detail: 40.0,
            zig_size: 8.0,
            ridges: 5.0,
            amount: 30.0,
            smooth: false,
        }
    }
}

/// One of the card's rows.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Row {
    Roughen,
    ZigZag,
    PuckerBloat,
}

/// The open card.
pub(crate) struct DistortCard {
    id: NodeId,
    /// The path as stored when the card last read it; any other state of it starts
    /// the card over.
    basis: NodeKind,
    /// Roughen's pattern: what the preview shows is what Apply bakes, and each
    /// Apply moves to the next.
    seed: u64,
    /// The row the canvas shows, and what it was built from.
    shown: Option<(Row, ShownKey)>,
    /// The points the shown row would add, in world space, which the canvas marks.
    pub(crate) adding: Vec<Point>,
}

/// What a shown row was built from: the numbers, the point selection and the seed.
type ShownKey = (DistortNumbers, Vec<mastik_core::edit::PointRef>, u64);

impl MastikApp {
    /// Open the Distort card on `id`, or close it. The Offset and Clean up cards,
    /// which hang in the same place, close.
    pub(crate) fn toggle_distort(&mut self, id: NodeId) {
        if self.distort.is_some() {
            return self.close_distort();
        }
        self.close_offset_card();
        self.close_cleanup();
        let Some(basis) = self
            .session
            .resolved
            .used_kind(&self.session.doc, id)
            .cloned()
        else {
            return;
        };
        if !matches!(basis, NodeKind::Path { .. }) {
            return;
        }
        self.distort = Some(DistortCard {
            id,
            basis,
            // From the history, so a card reopened after any edit roughens anew.
            seed: self.session.history.last_serial(),
            shown: None,
            adding: Vec::new(),
        });
    }

    /// Close the card, taking its preview off the canvas.
    pub(crate) fn close_distort(&mut self) {
        if let Some(card) = self.distort.take()
            && card.shown.is_some()
        {
            self.session.clear_gesture_preview();
        }
    }

    /// Whether the gesture preview is the card's row rather than a gesture.
    pub(crate) fn distort_preview_standing(&self) -> bool {
        self.drag.is_none() && self.distort.as_ref().is_some_and(|c| c.shown.is_some())
    }

    /// The points the card's shown row would add, for the canvas to mark.
    pub(crate) fn distort_adding(&self) -> &[Point] {
        self.distort.as_ref().map_or(&[], |c| &c.adding)
    }

    /// The filter `row` stands for at the card's numbers.
    fn distort_filter(&self, card: &DistortCard, row: Row) -> Filter {
        let n = self.distort_numbers;
        match row {
            Row::Roughen => Filter::Roughen {
                size: n.rough_size,
                detail: n.detail / 100.0,
                seed: card.seed,
            },
            Row::ZigZag => Filter::ZigZag {
                size: n.zig_size,
                ridges: n.ridges.round().max(1.0) as usize,
            },
            Row::PuckerBloat => Filter::PuckerBloat {
                amount: n.amount / 100.0,
            },
        }
    }

    /// What `row` would do to the card's path now, and the points it adds in world
    /// space; `None` when it would change nothing.
    fn distort_tx(&self, card: &DistortCard, row: Row) -> Option<(Transaction, Vec<Point>)> {
        let id = card.id;
        let (world, subpaths) = self.edited_subpaths(id)?;
        let selected = self.with_mirrored_points(id, &self.subject_points(id));
        let filter = self.distort_filter(card, row);
        let out = mastik_core::distort::distort(
            &subpaths,
            &selected,
            filter,
            self.distort_numbers.smooth,
        );
        let tx = self.rewrite_edit(id, world, out.subpaths);
        (!tx.0.is_empty()).then_some((tx, out.added))
    }

    /// Commit `row`, one undo step named for it.
    fn commit_distort(&mut self, card: &mut DistortCard, row: Row) {
        if card.shown.take().is_some() {
            self.session.clear_gesture_preview();
        }
        card.adding.clear();
        let Some((tx, _)) = self.distort_tx(card, row) else {
            return self.session.info("Nothing to change");
        };
        let label = match row {
            Row::Roughen => "Roughen",
            Row::ZigZag => "Zig-zag",
            Row::PuckerBloat if self.distort_numbers.amount < 0.0 => "Pucker",
            Row::PuckerBloat => "Bloat",
        };
        self.session.name_step(label, icon::DISTORT);
        if !self.session.commit(tx) {
            return;
        }
        // Pucker and Bloat keep every anchor; the others renumber them.
        if row != Row::PuckerBloat {
            self.points.clear();
        }
        if row == Row::Roughen {
            card.seed = card.seed.wrapping_add(1);
        }
    }

    /// The card beside the bar, its top on the Distort button's. The button, its ×,
    /// `Escape` and the end of the edit close it.
    pub(crate) fn distort_ui(&mut self, ctx: &egui::Context, bar: egui::Rect, button: egui::Rect) {
        let Some(mut card) = self.distort.take() else {
            return;
        };
        if self.edited_path() != Some(card.id) {
            self.distort = Some(card);
            return self.close_distort();
        }
        let now = self.session.resolved.used_kind(&self.session.doc, card.id);
        if now != Some(&card.basis) {
            let Some(kind) = now.cloned() else {
                self.distort = Some(card);
                return self.close_distort();
            };
            if card.shown.take().is_some() {
                self.session.clear_gesture_preview();
            }
            card.adding.clear();
            card.basis = kind;
        }
        let before = self.distort_numbers;
        let mut n = before;
        let points = self.subject_points(card.id).len();

        let mut hot: Option<Row> = None;
        let mut act: Option<Row> = None;
        let mut close = false;
        let mut scrubbing = false;
        let at = egui::pos2(bar.right() + crate::path_bar::GAP, button.top());
        egui::Area::new(egui::Id::new("distort-card"))
            .order(egui::Order::Foreground)
            .fixed_pos(at)
            .show(ctx, |ui| {
                ui::hold_tab(ui);
                ui::menu_frame(ui::POPOVER_PAD).show(ui, |ui| {
                    let w = ui::menu_inner_w(W, ui::POPOVER_PAD);
                    ui.set_width(w);
                    ui.spacing_mut().item_spacing.y = ui::POPOVER_SECTION_GAP;
                    close = ui::popover_head(ui, "Distort");
                    let bw = crate::settings::text_button_w(ui, "Apply");
                    let button = egui::vec2(bw, ui::CONTROL_H);
                    // Two fields share what the button leaves.
                    let half = egui::vec2((w - bw - ui::CARD_COL_GAP * 2.0) / 2.0, ui::CONTROL_H);
                    let whole = egui::vec2(w - bw - ui::CARD_COL_GAP, ui::CONTROL_H);
                    let px = Some(Suffix {
                        text: "px",
                        clickable: false,
                        tooltip: "",
                    });
                    // A row is shown while its button is hovered or focused, or a
                    // field of its scrubbed or typed in.
                    let mut row = |which: Row, fields: &[&egui::Response], b: egui::Response| {
                        let engaged = fields.iter().any(|f| f.dragged() || f.has_focus());
                        scrubbing |= fields.iter().any(|f| f.dragged());
                        if engaged || b.hovered() || b.has_focus() {
                            hot = Some(which);
                        }
                        if b.clicked() {
                            act = Some(which);
                        }
                    };

                    ui::labelled(ui, "Roughen", |ui| {
                        ui.horizontal(|ui| {
                            ui.spacing_mut().item_spacing.x = ui::CARD_COL_GAP;
                            let (size, _) = ui::value_field_suffixed(
                                ui,
                                half,
                                Prefix::Text("Size"),
                                px,
                                &mut n.rough_size,
                                Scrub::fine(0.1, 1).range(0.0..=200.0),
                                |d| d.custom_formatter(ui::number(1)),
                            );
                            let size = size.on_hover_text("The furthest a point moves");
                            let detail = ui::value_field(
                                ui,
                                half,
                                Prefix::Text("Detail"),
                                &mut n.detail,
                                Scrub::whole(0.5).range(0.0..=100.0),
                                |d| d.custom_formatter(|v, _| format!("{v:.0}%")),
                            )
                            .on_hover_text(
                                "How many points it adds before moving them — 0 moves only \
                                 the points there are",
                            );
                            let b = ui::label_button(ui, "Apply", button, FieldButton::Off)
                                .on_hover_text("Each Apply roughens differently");
                            row(Row::Roughen, &[&size, &detail], b);
                        });
                    });

                    ui::labelled(ui, "Zig-zag", |ui| {
                        ui.horizontal(|ui| {
                            ui.spacing_mut().item_spacing.x = ui::CARD_COL_GAP;
                            let (size, _) = ui::value_field_suffixed(
                                ui,
                                half,
                                Prefix::Text("Size"),
                                px,
                                &mut n.zig_size,
                                Scrub::fine(0.1, 1).range(0.0..=200.0),
                                |d| d.custom_formatter(ui::number(1)),
                            );
                            let size = size.on_hover_text("A ridge's height, crest to trough");
                            let ridges = ui::value_field(
                                ui,
                                half,
                                Prefix::Text("Ridges"),
                                &mut n.ridges,
                                Scrub::whole(0.1).range(1.0..=100.0),
                                |d| d.custom_formatter(ui::number(0)),
                            )
                            .on_hover_text("How many ridges each segment gets");
                            let b = ui::label_button(ui, "Apply", button, FieldButton::Off);
                            row(Row::ZigZag, &[&size, &ridges], b);
                        });
                    });

                    ui::labelled(ui, "Pucker / Bloat", |ui| {
                        ui.horizontal(|ui| {
                            ui.spacing_mut().item_spacing.x = ui::CARD_COL_GAP;
                            let amount = ui::value_field_metered(
                                ui,
                                whole,
                                Prefix::Text("Amount"),
                                ui::Meter::Signed((n.amount / 100.0) as f32),
                                &mut n.amount,
                                Scrub::whole(0.5).range(-100.0..=100.0),
                                |d| {
                                    d.custom_formatter(|v, _| match v > 0.0 {
                                        true => format!("+{v:.0}%"),
                                        false => format!("{v:.0}%"),
                                    })
                                },
                            )
                            .on_hover_text(
                                "Below 0 pulls each segment's middle toward the centre, above \
                                 0 pushes it out",
                            );
                            let b = ui::label_button(ui, "Apply", button, FieldButton::Off);
                            row(Row::PuckerBloat, &[&amount], b);
                        });
                    });

                    ui::card_rule(ui);
                    ui::labelled(ui, "New points", |ui| {
                        let current = usize::from(n.smooth);
                        if let Some(i) = ui::segmented(
                            ui,
                            w,
                            ui::SEGMENT_CELL_H,
                            2,
                            current,
                            |p, i, rect, on| {
                                ui::segment_label(p, rect, ["Corner", "Smooth"][i], on)
                            },
                        ) {
                            n.smooth = i == 1;
                        }
                    });
                    let note = match points {
                        0 => "Acts on the whole path. Select points to distort only between \
                              them."
                            .to_owned(),
                        k => format!("Acts between the {k} selected points"),
                    };
                    ui.label(
                        egui::RichText::new(note)
                            .size(ui::SMALL_PT)
                            .color(theme::text::muted()),
                    );
                });
            });

        // Armed after a right-click's cancel is answered, so the frame it lands on
        // cannot re-arm it.
        if scrubbing && !self.gesture_cancelled && self.in_flight.distort_scrub.is_none() {
            self.in_flight.distort_scrub = Some(before);
        }
        self.distort_numbers = n;
        if close {
            self.distort = Some(card);
            return self.close_distort();
        }
        if let Some(row) = act {
            self.commit_distort(&mut card, row);
            self.distort = Some(card);
            ctx.request_repaint();
            return;
        }
        self.show_distort_row(&mut card, hot);
        self.distort = Some(card);
    }

    /// Put `row`'s result on the canvas, built again only when what it shows
    /// changed, or take the card's off when `row` is `None`. Hands off while a
    /// canvas drag owns the preview.
    fn show_distort_row(&mut self, card: &mut DistortCard, row: Option<Row>) {
        if !self.drag.is_none() {
            card.shown = None;
            card.adding.clear();
            return;
        }
        let Some(row) = row else {
            if card.shown.take().is_some() {
                self.session.clear_gesture_preview();
            }
            card.adding.clear();
            return;
        };
        let key: ShownKey = (
            self.distort_numbers,
            self.subject_points(card.id),
            card.seed,
        );
        if card.shown.as_ref() == Some(&(row, key.clone())) && self.session.has_gesture_preview() {
            return;
        }
        match self.distort_tx(card, row) {
            Some((tx, added)) => {
                self.session.set_preview(&tx);
                card.adding = added;
            }
            None => {
                self.session.clear_gesture_preview();
                card.adding.clear();
            }
        }
        card.shown = Some((row, key));
    }

    /// Put back the numbers a right-click cancelled the card's scrub of.
    pub(crate) fn restore_distort_scrub(&mut self, n: DistortNumbers) {
        self.distort_numbers = n;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::path_bar::tests::{app_with_a_ring, stored};

    /// Roughen commits one undo step named for it, adds points, and the next
    /// Apply draws another pattern; Pucker keeps the anchor count and is named by
    /// its sign. flip: `commit_distort` not advancing the seed → fails at "another
    /// pattern".
    #[test]
    fn each_apply_is_its_own_named_step() {
        let ctx = egui::Context::default();
        let (mut app, id) = app_with_a_ring(&ctx);
        app.toggle_distort(id);
        let mut card = app.distort.take().expect("the card opens on a path");
        // The ring's sides are about 5 long: full detail adds one point to each.
        app.distort_numbers.detail = 100.0;
        let before = stored(&app, id).0;
        app.commit_distort(&mut card, Row::Roughen);
        let once = stored(&app, id).0;
        assert_ne!(once, before, "it roughened");
        assert!(
            once.elements().len() >= before.elements().len() + 64,
            "a point was added to each side"
        );
        let step = app.session.history.done().last().expect("a step");
        assert_eq!(step.note.label, "Roughen");
        app.session.undo();
        app.commit_distort(&mut card, Row::Roughen);
        assert_ne!(stored(&app, id).0, once, "another pattern");

        app.distort_numbers.amount = -40.0;
        let count = stored(&app, id).0.elements().len();
        app.commit_distort(&mut card, Row::PuckerBloat);
        assert_eq!(
            app.session.history.done().last().unwrap().note.label,
            "Pucker"
        );
        assert_eq!(stored(&app, id).0.elements().len(), count, "no point added");
    }
}
