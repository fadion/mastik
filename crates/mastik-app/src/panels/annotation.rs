//! The inspector's *Annotation* card: a layer's note, what the drawing cannot say
//! ("this list scrolls", "48 px touch target"). Specification rather than
//! conversation: one text per layer, no replies, authors or resolved state. The
//! canvas marks an annotated layer (`MastikApp::draw_annotation_marks`) and code
//! export writes it atop the layer's code.

use crate::app::MastikApp;
use crate::theme::{color, icon};
use crate::ui;
use eframe::egui;
use mastik_core::{NodeId, Operation, Transaction};

/// The mark's side on the canvas, in points.
const MARK: f32 = 16.0;

impl MastikApp {
    /// The card for the one selected layer `id`: its annotation, typed in a field
    /// that grows with its lines, committed on defocus.
    pub(crate) fn inspector_annotation(&mut self, ui: &mut egui::Ui, id: NodeId) {
        let Some(saved) = self.session.doc.get(id).map(|n| n.note().to_string()) else {
            return;
        };
        self.panel(ui, "Annotation", None, |app, ui| {
            // Private mode hides an annotation as it does a description.
            let private = crate::private::on();
            let mut text = match ui::Draft::live(&mut app.note_edit, ui.ctx(), id) {
                Some(d) => d.text.clone(),
                None if private => String::new(),
                None => saved.clone(),
            };
            let hint = match private {
                true => "Annotation hidden in private mode",
                false => "Add what the drawing can't say",
            };
            let mut area = ui::TextArea::new(hint, 1);
            area.id = Some(ui.id().with(("annotation", id)));
            let resp = ui
                .add_enabled_ui(!private, |ui| ui::text_area(ui, &mut text, area))
                .inner;
            if let Some(typed) = ui::Draft::follow(&mut app.note_edit, &resp, id, &text) {
                let text = typed.trim().to_string();
                if text != saved {
                    app.session.name_step("Annotation", icon::NOTE);
                    app.commit_edit(Transaction(vec![Operation::SetNote { id, text }]));
                }
            }
        });
    }

    /// The annotated layers on the current page with where their mark sits on the
    /// screen: just outside the layer's top-right corner.
    pub(crate) fn annotation_marks(&self, rect: egui::Rect, ppp: f32) -> Vec<(NodeId, egui::Rect)> {
        let doc = &self.session.doc;
        let page = self.session.page;
        let mut out = Vec::new();
        let mut stack: Vec<NodeId> = doc
            .get(page)
            .map(|n| n.children().to_vec())
            .unwrap_or_default();
        while let Some(id) = stack.pop() {
            let Some(n) = doc.get(id) else { continue };
            if !n.visible() {
                continue;
            }
            stack.extend(n.children().iter().copied());
            if n.note().is_empty() {
                continue;
            }
            let Some(q) = self.selection_quad(id, rect, ppp) else {
                continue;
            };
            let right = q.iter().map(|p| p.x).fold(f32::MIN, f32::max);
            let top = q.iter().map(|p| p.y).fold(f32::MAX, f32::min);
            let at = egui::Rect::from_min_size(
                egui::pos2(right + 3.0, top - MARK - 3.0),
                egui::vec2(MARK, MARK),
            );
            if rect.intersects(at) {
                out.push((id, at));
            }
        }
        out
    }

    /// Mark each annotated layer on the canvas; hovering a mark shows the note.
    pub(crate) fn draw_annotation_marks(
        &self,
        ui: &egui::Ui,
        painter: &egui::Painter,
        rect: egui::Rect,
        ppp: f32,
    ) {
        for (id, at) in self.annotation_marks(rect, ppp) {
            painter.rect_filled(at, 4.0, color::accent_a(230));
            // text-ink: a mark on the chip's solid accent.
            painter.text(
                at.center(),
                egui::Align2::CENTER_CENTER,
                icon::NOTE,
                crate::theme::icon_font(11.0),
                color::on_accent(),
            );
            let hover = ui.interact(
                at,
                ui.id().with(("annotation-mark", id)),
                egui::Sense::hover(),
            );
            if let Some(n) = self.session.doc.get(id) {
                let words = match crate::private::on() {
                    true => "Annotation hidden in private mode".to_string(),
                    false => n.note().to_string(),
                };
                hover.on_hover_text(words);
            }
        }
    }
}
