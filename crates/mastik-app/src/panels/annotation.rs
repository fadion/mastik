//! A layer's annotation, the last row of its identity card: its note, what the drawing cannot say
//! ("this list scrolls", "48 px touch target"). Specification rather than
//! conversation: one text per layer, no replies, authors or resolved state. The
//! canvas marks an annotated layer (`MastikApp::draw_annotation_marks`) and code
//! export writes it atop the layer's code.

use crate::app::MastikApp;
use crate::theme::{color, icon};
use crate::ui;
use eframe::egui;
use mastik_core::{NodeId, Operation, Transaction};

/// The mark's side on the canvas, in points: a frame's name tag's height,
/// beside which it sits.
const MARK: f32 = 14.0;
/// Between a mark and what it hangs off: a name tag's end, or a layer's corner.
const MARK_GAP: f32 = 4.0;

impl MastikApp {
    /// The identity card's last row for the one selected layer `id`: its
    /// annotation, typed in a field that grows with its lines, committed on
    /// defocus. Unlabelled, as a component's description is; the hint names it.
    pub(crate) fn annotation_field(&mut self, ui: &mut egui::Ui, id: NodeId) {
        let Some(saved) = self.session.doc.get(id).map(|n| n.note().to_string()) else {
            return;
        };
        // Private mode hides an annotation as it does a description.
        let private = crate::private::on();
        let mut text = match ui::Draft::live(&mut self.note_edit, ui.ctx(), id) {
            Some(d) => d.text.clone(),
            None if private => String::new(),
            None => saved.clone(),
        };
        let hint = match private {
            true => "Annotation hidden in private mode",
            false => "Add an annotation",
        };
        let mut area = ui::TextArea::new(hint, 1);
        area.id = Some(ui.id().with(("annotation", id)));
        let resp = ui
            .add_enabled_ui(!private, |ui| ui::text_area(ui, &mut text, area))
            .inner
            .on_hover_text("What the drawing can't say: marked on the canvas, and written above the layer's code");
        if let Some(typed) = ui::Draft::follow(&mut self.note_edit, &resp, id, &text) {
            let text = typed.trim().to_string();
            if text != saved {
                self.session.name_step("Annotation", icon::NOTE);
                self.commit_edit(Transaction(vec![Operation::SetNote { id, text }]));
            }
        }
    }

    /// The annotated layers on the current page whose mark shows, with where it
    /// sits on the screen. Every one while *Show annotations* is on; otherwise the
    /// one `hovered`, those selected, and one whose mark is under the pointer, so
    /// the pointer can travel from the layer onto its mark to read the note.
    ///
    /// A layer wearing a name tag (a top-level frame) has its mark right after the
    /// tag, clear of the buttons over the frame's top-right corner; any other just
    /// outside its top-right corner.
    pub(crate) fn annotation_marks(
        &self,
        ui: &egui::Ui,
        rect: egui::Rect,
        ppp: f32,
        hovered: Option<NodeId>,
    ) -> Vec<(NodeId, egui::Rect)> {
        let doc = &self.session.doc;
        let page = self.session.page;
        let pointer = ui.ctx().pointer_hover_pos();
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
            let at = match self.frame_label(ui, id, rect, ppp) {
                Some((tag, _)) => egui::Rect::from_min_size(
                    egui::pos2(tag.right() + MARK_GAP, tag.center().y - MARK / 2.0),
                    egui::vec2(MARK, MARK),
                ),
                None => {
                    let Some(q) = self.selection_quad(id, rect, ppp) else {
                        continue;
                    };
                    let right = q.iter().map(|p| p.x).fold(f32::MIN, f32::max);
                    let top = q.iter().map(|p| p.y).fold(f32::MAX, f32::min);
                    egui::Rect::from_min_size(
                        egui::pos2(right + MARK_GAP, top - MARK - MARK_GAP),
                        egui::vec2(MARK, MARK),
                    )
                }
            };
            let shown = self.show_annotations
                || hovered == Some(id)
                || self.session.selection.contains(id)
                || pointer.is_some_and(|p| at.contains(p));
            if shown && rect.intersects(at) {
                out.push((id, at));
            }
        }
        out
    }

    /// Mark the annotated layers [`Self::annotation_marks`] shows; hovering a
    /// mark shows the note.
    pub(crate) fn draw_annotation_marks(
        &self,
        ui: &egui::Ui,
        painter: &egui::Painter,
        rect: egui::Rect,
        ppp: f32,
        resp: &egui::Response,
    ) {
        let hovered = self.hover_target(ui, rect, ppp, resp);
        for (id, at) in self.annotation_marks(ui, rect, ppp, hovered) {
            painter.rect_filled(at, 4.0, color::accent_a(230));
            // text-ink: a mark on the chip's solid accent.
            painter.text(
                at.center(),
                egui::Align2::CENTER_CENTER,
                icon::NOTE,
                crate::theme::icon_font(10.0),
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

#[cfg(test)]
mod tests {
    use crate::app::MastikApp;
    use eframe::egui;
    use mastik_core::kurbo::{Affine, Size};
    use mastik_core::{Document, IdSource, NodeId, NodeKind, Operation, Transaction};

    /// A headless app on a page holding an annotated top-level frame with an
    /// annotated rect inside it.
    fn fixture(ctx: &egui::Context) -> (MastikApp, NodeId, NodeId) {
        let mut app = MastikApp::headless(ctx);
        let mut ids = IdSource::new(0xA1);
        let page = ids.mint();
        let [frame, rect] = [(); 2].map(|_| ids.mint());
        let mut doc = Document::new(page);
        doc.apply(&Transaction(vec![
            Operation::CreateNode {
                id: frame,
                parent: page,
                index: 0,
                kind: NodeKind::Artboard {
                    size: Size::new(300.0, 200.0),
                    corner_radii: Default::default(),
                    corner_smoothing: 0.0,
                },
                transform: Some(Affine::translate((100.0, 100.0))),
                name: Some("Onboarding".into()),
            },
            Operation::CreateNode {
                id: rect,
                parent: frame,
                index: 0,
                kind: NodeKind::Rect {
                    size: Size::new(40.0, 40.0),
                    corner_radii: Default::default(),
                    corner_smoothing: 0.0,
                },
                transform: Some(Affine::translate((20.0, 20.0))),
                name: None,
            },
            Operation::SetNote {
                id: frame,
                text: "Swipes left".into(),
            },
            Operation::SetNote {
                id: rect,
                text: "48 px touch target".into(),
            },
        ]))
        .expect("the page");
        app.session.adopt_document(doc, None);
        // World and screen as one, so the frame, its tag and the marks are on screen.
        app.session.camera.center = mastik_core::kurbo::Point::ZERO;
        (app, frame, rect)
    }

    /// The canvas the marks are laid out on.
    fn canvas() -> egui::Rect {
        egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(1000.0, 1000.0))
    }

    /// The marks shown, `hovered` standing for the canvas's hover.
    fn marks(
        ctx: &egui::Context,
        app: &MastikApp,
        hovered: Option<NodeId>,
    ) -> Vec<(NodeId, egui::Rect)> {
        let mut out = Vec::new();
        let _ = ctx.run_ui(Default::default(), |ui| {
            out = app.annotation_marks(ui, canvas(), 1.0, hovered);
        });
        out
    }

    /// A mark shows on the layer hovered or selected, and on every annotated
    /// layer only while *Show annotations* is on.
    ///
    /// flip: `annotation_marks` ignoring `show_annotations` → fails at "every
    /// mark with the switch on".
    #[test]
    fn marks_show_on_hover_and_selection_unless_switched_on() {
        let ctx = egui::Context::default();
        crate::theme::install(&ctx);
        let (mut app, frame, rect) = fixture(&ctx);
        let ids = |m: Vec<(NodeId, egui::Rect)>| m.into_iter().map(|m| m.0).collect::<Vec<_>>();
        assert!(marks(&ctx, &app, None).is_empty(), "none at rest");
        assert_eq!(
            ids(marks(&ctx, &app, Some(rect))),
            [rect],
            "the hovered one"
        );
        app.session.selection.set_one(frame);
        assert_eq!(ids(marks(&ctx, &app, None)), [frame], "the selected one");
        app.session.selection.clear();
        app.show_annotations = true;
        assert_eq!(
            marks(&ctx, &app, None).len(),
            2,
            "every mark with the switch on"
        );
    }

    /// A frame's mark sits right after its name tag, centred on it; a layer
    /// without a tag keeps its mark outside its top-right corner.
    ///
    /// flip: `annotation_marks` placing every mark at the corner → fails at
    /// "after the frame's name".
    #[test]
    fn a_frames_mark_follows_its_name_tag() {
        let ctx = egui::Context::default();
        crate::theme::install(&ctx);
        let (mut app, frame, rect) = fixture(&ctx);
        app.show_annotations = true;
        let mut tag = None;
        let mut corner = None;
        let _ = ctx.run_ui(Default::default(), |ui| {
            tag = app.frame_label(ui, frame, canvas(), 1.0).map(|(r, _)| r);
            corner = app.selection_quad(rect, canvas(), 1.0);
        });
        let tag = tag.expect("fixture: the frame wears a tag");
        let all = marks(&ctx, &app, None);
        let at = |id| all.iter().find(|m| m.0 == id).map(|m| m.1).expect("a mark");
        let frame_mark = at(frame);
        assert!(
            frame_mark.left() > tag.right() && frame_mark.left() - tag.right() < 8.0,
            "after the frame's name: {frame_mark:?} vs {tag:?}"
        );
        assert!(
            (frame_mark.center().y - tag.center().y).abs() < 0.5,
            "centred on the tag"
        );
        let q = corner.expect("fixture: the rect is on screen");
        let right = q.iter().map(|p| p.x).fold(f32::MIN, f32::max);
        assert!(at(rect).left() > right, "outside the rect's corner");
    }
}
