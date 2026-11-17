//! The assistant on the canvas: the ask buttons over the selection and over
//! each attached layer, the scope button beside them over a top-level frame,
//! and while the panel is open a dashed outline on each attached layer.

use crate::app::MastikApp;
use crate::preview::Drag;
use crate::theme::{self, color, icon};
use mastik_core::NodeId;
use mastik_core::kurbo::{Point, Rect};

/// The ask button's height, and its width without a number; the same at every
/// zoom.
pub(crate) const ASK: f32 = 26.0;
/// Clear air between the button and its layer's top edge.
const ASK_GAP: f32 = 8.0;
/// How far outside its layer an attached layer's outline runs.
const MARK_OUT: f32 = 5.0;
/// A lit button's inset either side of its sparkle and number.
const ASK_PAD: f32 = 7.0;
/// Between a lit button's sparkle and its number.
const ASK_NUM_GAP: f32 = 4.0;
/// The sparkle's size on the button.
const ASK_GLYPH_PT: f32 = 14.0;
/// Between the scope button and the ask button on its right.
const SCOPE_GAP: f32 = 4.0;

/// What an ask button stands for, and so what a click on it does.
#[derive(Clone, Debug, PartialEq)]
pub(crate) enum AskKind {
    /// The selection's: attaches it.
    Ask,
    /// An attached layer's, lit with its chip's number: detaches it.
    Attached(String, usize),
    /// A top-level frame's scope button: scopes the assistant to it, or, lit
    /// on the scoped frame, removes the scope.
    Scope { wire: String, lit: bool },
}

/// One ask button: where it sits and what a click does.
pub(crate) struct AskButton {
    pub(crate) place: egui::Rect,
    pub(crate) kind: AskKind,
}

impl AskButton {
    /// The button's area, whose layer the canvas hands the wheel back from.
    pub(crate) fn area_id(&self) -> egui::Id {
        match &self.kind {
            AskKind::Ask => egui::Id::new("assistant-ask"),
            AskKind::Attached(wire, _) => egui::Id::new(("assistant-ask", wire.as_str())),
            AskKind::Scope { wire, .. } => egui::Id::new(("assistant-scope", wire.as_str())),
        }
    }

    /// Lit: it stands for something already attached or scoped.
    fn lit(&self) -> bool {
        match &self.kind {
            AskKind::Ask => false,
            AskKind::Attached(..) => true,
            AskKind::Scope { lit, .. } => *lit,
        }
    }
}

impl MastikApp {
    /// A world rect on screen.
    fn screen_rect(&self, r: Rect, ppp: f32) -> egui::Rect {
        let a = self.to_screen(Point::new(r.x0, r.y0), self.canvas_rect, ppp);
        let b = self.to_screen(Point::new(r.x1, r.y1), self.canvas_rect, ppp);
        egui::Rect::from_two_pos(a, b)
    }

    /// Whether ask buttons may show at all: the assistant on, and nothing the
    /// buttons would get in the way of.
    fn ask_live(&self, ctx: &egui::Context) -> bool {
        self.prefs.assistant.harness().is_some()
            && !self.prefs.private
            && self.presenting.is_none()
            && !self.focus
            && self.text.is_none()
            && self.edited_path().is_none()
            && self.edited_image().is_none()
            && matches!(self.drag, Drag::None)
            && ctx.dragged_id().is_none()
            && !self.session.has_gesture_preview()
    }

    /// An ask button `w` wide for layer bounds `sel` on screen: its right edge
    /// on the layer's, its bottom [`ASK_GAP`] above it; inside the corner where
    /// there is no room above; left of the inspector's cards.
    fn ask_button_place(&self, sel: egui::Rect, screen: egui::Rect, w: f32) -> egui::Rect {
        let column = Self::inspector_column(screen);
        let right = sel.right().min(column.left() - ASK_GAP);
        let mut top = sel.top() - ASK_GAP - ASK;
        if top < self.canvas_rect.top() + ASK_GAP {
            top = sel.top() + ASK_GAP;
        }
        egui::Rect::from_min_size(egui::pos2(right - w, top), egui::vec2(w, ASK))
    }

    /// A lit button's width, carrying number `n`.
    fn ask_lit_w(ctx: &egui::Context, n: usize) -> f32 {
        let num = ctx.fonts_mut(|f| {
            f.layout_no_wrap(
                n.to_string(),
                egui::FontId::proportional(crate::ui::SMALL_PT),
                egui::Color32::PLACEHOLDER,
            )
            .size()
            .x
        });
        ASK_PAD + ASK_GLYPH_PT + ASK_NUM_GAP + num + ASK_PAD
    }

    /// The ask buttons this frame. Each attached layer on this page has a lit
    /// one carrying its chip's number while the panel is open or the layer is
    /// selected, whatever *Show ask assistant* says. The selection has an unlit
    /// one while that switch is on and some selected layer is not attached; a
    /// lit button it would cover gives way. The scope button follows
    /// [`Self::scope_button`].
    pub(crate) fn ask_buttons(&self, ctx: &egui::Context) -> Vec<AskButton> {
        let mut out = Vec::new();
        if !self.ask_live(ctx) {
            return out;
        }
        let ppp = ctx.pixels_per_point();
        let screen = ctx.input(|i| i.content_rect());
        let unlit = (self.show_ask && !self.selection_attached())
            .then(|| self.selection_bounds())
            .flatten()
            .map(|b| self.ask_button_place(self.screen_rect(b, ppp), screen, ASK))
            .filter(|p| self.canvas_rect.contains_rect(*p));
        for (n, c) in self.assistant.chips.iter().enumerate() {
            let Some(id) = NodeId::from_wire(&c.id) else {
                continue;
            };
            if !(self.assistant.open || self.session.selection.contains(id))
                || self.session.doc.page_of(id) != Some(self.session.page)
            {
                continue;
            }
            let Some(b) = self.session.preview_world_bounds(id) else {
                continue;
            };
            let w = Self::ask_lit_w(ctx, n + 1);
            let place = self.ask_button_place(self.screen_rect(b, ppp), screen, w);
            if !self.canvas_rect.contains_rect(place) || unlit.is_some_and(|u| u.intersects(place))
            {
                continue;
            }
            out.push(AskButton {
                place,
                kind: AskKind::Attached(c.id.clone(), n + 1),
            });
        }
        if let Some(place) = unlit {
            out.push(AskButton {
                place,
                kind: AskKind::Ask,
            });
        }
        // Lit over the scoped frame on this page while the panel is open or the
        // frame is selected, whatever the View switches say; unlit over a
        // selected top-level frame while *Show ask assistant* and *Show
        // assistant scope* are both on.
        let scoped = self
            .assistant
            .scope
            .as_ref()
            .and_then(|c| NodeId::from_wire(&c.id))
            .filter(|id| self.session.doc.get(*id).is_some())
            .filter(|id| self.session.doc.page_of(*id) == Some(self.session.page));
        if let Some(id) = scoped
            && (self.assistant.open || self.session.selection.contains(id))
            && let Some(b) = self.scope_button(id, true, ppp, screen, &out)
        {
            out.push(b);
        }
        if self.show_ask
            && self.show_scope
            && let Some(id) = self.scope_candidate().filter(|id| Some(*id) != scoped)
            && let Some(b) = self.scope_button(id, false, ppp, screen, &out)
        {
            out.push(b);
        }
        out
    }

    /// Frame `id`'s scope button, left of the ask button standing at the same
    /// corner. Where it would leave the canvas it is dropped, and the ask
    /// button stays.
    fn scope_button(
        &self,
        id: NodeId,
        lit: bool,
        ppp: f32,
        screen: egui::Rect,
        others: &[AskButton],
    ) -> Option<AskButton> {
        let wire = id.to_wire();
        // The ask button at this frame's corner: its own lit one, or the
        // selection's when the frame is the selection.
        let selected = self.session.selection.ids() == [id];
        let beside = others.iter().find(|b| match &b.kind {
            AskKind::Attached(w, _) => *w == wire,
            AskKind::Ask => selected,
            AskKind::Scope { .. } => false,
        });
        let place = match beside {
            Some(b) => egui::Rect::from_min_size(
                egui::pos2(b.place.left() - SCOPE_GAP - ASK, b.place.top()),
                egui::vec2(ASK, ASK),
            ),
            None => {
                let b = self.session.preview_world_bounds(id)?;
                self.ask_button_place(self.screen_rect(b, ppp), screen, ASK)
            }
        };
        self.canvas_rect.contains_rect(place).then_some(AskButton {
            place,
            kind: AskKind::Scope { wire, lit },
        })
    }

    /// Whether `layer` is an ask button's, which hands the wheel to the canvas.
    pub(crate) fn is_ask_layer(&self, ctx: &egui::Context, layer: egui::LayerId) -> bool {
        layer.order == egui::Order::Middle
            && self
                .ask_buttons(ctx)
                .iter()
                .any(|b| b.area_id() == layer.id)
    }

    /// A click on an ask button: the selection's attaches it and opens the
    /// panel; a lit one's detaches its layer, a toggle. The scope button
    /// scopes the assistant to its frame, or, lit, removes the scope.
    pub(crate) fn ask_button_clicked(&mut self, kind: AskKind) {
        match kind {
            AskKind::Ask => self.ask_assistant(),
            AskKind::Attached(wire, _) => self.assistant.chips.retain(|c| c.id != wire),
            AskKind::Scope { lit: true, .. } => self.assistant.scope = None,
            AskKind::Scope { wire, lit: false } => {
                if let Some(id) = NodeId::from_wire(&wire) {
                    self.scope_assistant_to(id);
                }
            }
        }
    }

    /// Draw the ask buttons.
    pub(crate) fn assistant_canvas_ui(&mut self, ctx: &egui::Context) {
        let mut clicked = None;
        for b in self.ask_buttons(ctx) {
            let lit = b.lit();
            let resp = egui::Area::new(b.area_id())
                .order(egui::Order::Middle)
                .fixed_pos(b.place.min)
                .show(ctx, |ui| {
                    let resp = crate::ui::float_button(ui, b.place.size(), lit, |p, r, ink| {
                        let glyph = theme::icon_font(ASK_GLYPH_PT);
                        match &b.kind {
                            AskKind::Ask | AskKind::Scope { .. } => {
                                let g = match b.kind {
                                    AskKind::Ask => icon::SPARKLE,
                                    _ => icon::SCOPE_FRAME,
                                };
                                p.text(r.center(), egui::Align2::CENTER_CENTER, g, glyph, ink);
                            }
                            AskKind::Attached(_, n) => {
                                let x = r.left() + ASK_PAD;
                                p.text(
                                    egui::pos2(x + ASK_GLYPH_PT / 2.0, r.center().y),
                                    egui::Align2::CENTER_CENTER,
                                    icon::SPARKLE,
                                    glyph,
                                    ink,
                                );
                                p.text(
                                    egui::pos2(x + ASK_GLYPH_PT + ASK_NUM_GAP, r.center().y),
                                    egui::Align2::LEFT_CENTER,
                                    n.to_string(),
                                    egui::FontId::proportional(crate::ui::SMALL_PT),
                                    ink,
                                );
                            }
                        }
                    });
                    resp.on_hover_text(match &b.kind {
                        AskKind::Ask => "Ask the assistant",
                        AskKind::Attached(..) => "Detach from the assistant",
                        AskKind::Scope { lit: false, .. } => "Scope the assistant to this frame",
                        AskKind::Scope { lit: true, .. } => "Remove the scope",
                    })
                })
                .inner;
            if resp.clicked() {
                clicked = Some(b.kind);
            }
        }
        if let Some(kind) = clicked {
            self.ask_button_clicked(kind);
        }
    }

    /// A dashed accent outline [`MARK_OUT`] outside each attached layer; the
    /// layer a chip under the pointer names is outlined solid.
    pub(crate) fn draw_assistant_marks(&self, painter: &egui::Painter, rect: egui::Rect, ppp: f32) {
        if !self.assistant.open || self.prefs.private {
            return;
        }
        let p = painter.with_clip_rect(rect);
        for c in &self.assistant.chips {
            let Some(id) = NodeId::from_wire(&c.id)
                .filter(|id| self.session.doc.page_of(*id) == Some(self.session.page))
            else {
                continue;
            };
            let Some(r) = self.session.preview_world_bounds(id) else {
                continue;
            };
            let r = self.screen_rect(r, ppp).expand(MARK_OUT);
            let stroke = egui::Stroke::new(1.5, color::accent_400());
            for side in [
                [r.left_top(), r.right_top()],
                [r.right_top(), r.right_bottom()],
                [r.right_bottom(), r.left_bottom()],
                [r.left_bottom(), r.left_top()],
            ] {
                p.extend(egui::Shape::dashed_line(&side, stroke, 5.0, 3.0));
            }
        }
        // A layer on another page has no place here to outline.
        if let Some(wire) = &self.assistant.hovered
            && let Some(id) = NodeId::from_wire(wire)
            && self.session.doc.page_of(id) == Some(self.session.page)
            && let Some(r) = self.session.preview_world_bounds(id)
        {
            let r = self.screen_rect(r, ppp);
            p.rect_stroke(
                r,
                0.0,
                egui::Stroke::new(1.5, color::accent()),
                egui::StrokeKind::Outside,
            );
        }
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;

    /// The ask buttons for `app` in a 2000×1400 window.
    fn buttons(ctx: &egui::Context, app: &MastikApp) -> Vec<(AskKind, egui::Rect)> {
        let mut out = Vec::new();
        for _ in 0..2 {
            let _ = ctx.run_ui(
                egui::RawInput {
                    screen_rect: Some(egui::Rect::from_min_size(
                        egui::Pos2::ZERO,
                        egui::vec2(2000.0, 1400.0),
                    )),
                    ..Default::default()
                },
                |ui| {
                    out = app
                        .ask_buttons(ui.ctx())
                        .into_iter()
                        .map(|b| (b.kind, b.place))
                        .collect();
                },
            );
        }
        out
    }

    /// An attached layer has one ask button, lit and numbered, which shows with
    /// *Show ask assistant* off and detaches the layer when clicked.
    /// flip: `ask_buttons` keeping the selection's button once it is attached →
    /// fails at the single lit button.
    #[test]
    fn an_attached_layer_has_one_lit_button_that_detaches_it() {
        let ctx = egui::Context::default();
        let mut app = MastikApp::headless(&ctx);
        let id = crate::palette::tests::add_rect(&mut app);
        app.session.selection.set_one(id);
        app.prefs.assistant.agent = "claude".into();
        app.canvas_rect =
            egui::Rect::from_min_size(egui::pos2(100.0, 100.0), egui::vec2(1600.0, 1200.0));
        app.canvas_px = (1600, 1200);
        assert!(
            app.ask_live(&ctx),
            "nothing in the fixture hides the buttons"
        );
        let shown = buttons(&ctx, &app);
        assert_eq!(shown.len(), 1, "the selection's button: {shown:?}");
        assert_eq!(shown[0].0, AskKind::Ask, "unlit before it is attached");

        app.ask_button_clicked(AskKind::Ask);
        assert_eq!(app.assistant.chips.len(), 1, "a click attaches");
        app.show_ask = false;
        let shown = buttons(&ctx, &app);
        assert_eq!(
            shown.iter().map(|b| b.0.clone()).collect::<Vec<_>>(),
            vec![AskKind::Attached(id.to_wire(), 1)],
            "one button, lit with the chip's number, despite the switch"
        );

        app.ask_button_clicked(AskKind::Attached(id.to_wire(), 1));
        assert!(
            app.assistant.chips.is_empty(),
            "a lit button's click detaches"
        );
        assert!(buttons(&ctx, &app).is_empty(), "the switch is off");
    }

    /// Image editing hides the ask button, as editing a path does: the crop's
    /// handles and Done pill are the layer's chrome while it lasts.
    /// flip: `ask_live` without its `edited_image` guard → fails at "no button
    /// while the picture is edited".
    #[test]
    fn image_editing_hides_the_ask_button() {
        use mastik_core::{Operation, Transaction};
        let ctx = egui::Context::default();
        let mut app = MastikApp::headless(&ctx);
        let id = crate::palette::tests::add_rect(&mut app);
        app.session
            .try_commit(Transaction(vec![Operation::SetFills {
                id,
                fills: mastik_core::keyed_by_position([mastik_core::Fill {
                    brush: mastik_core::image_brush(mastik_core::ImageId("a-photo".into())).into(),
                    visible: true,
                }]),
            }]))
            .expect("a picture fill");
        app.session.selection.set_one(id);
        app.prefs.assistant.agent = "claude".into();
        app.canvas_rect =
            egui::Rect::from_min_size(egui::pos2(100.0, 100.0), egui::vec2(1600.0, 1200.0));
        app.canvas_px = (1600, 1200);
        assert_eq!(buttons(&ctx, &app).len(), 1, "fixture: the button shows");
        app.tool = crate::tools::Tool::ImageEdit;
        assert!(app.edited_image().is_some(), "fixture: editing the picture");
        assert!(
            buttons(&ctx, &app).is_empty(),
            "no button while the picture is edited"
        );
    }

    /// A frame named `name`, 400×300 at the origin, as the last child of
    /// `parent`.
    pub(crate) fn add_frame(app: &mut MastikApp, parent: NodeId, name: &str) -> NodeId {
        use mastik_core::{NodeKind, Operation, Transaction};
        let s = &mut app.session;
        let id = s.ids.mint();
        let index = s.doc.get(parent).map_or(0, |n| n.children().len());
        assert!(s.commit(Transaction(vec![Operation::CreateNode {
            id,
            parent,
            index,
            kind: NodeKind::Artboard {
                size: mastik_core::kurbo::Size::new(400.0, 300.0),
                corner_radii: mastik_core::kurbo::RoundedRectRadii::default(),
                corner_smoothing: 0.0,
            },
            transform: None,
            name: Some(name.into()),
        }])));
        id
    }

    /// An app with the assistant on and a canvas laid out, for the buttons.
    fn scoping_app(ctx: &egui::Context) -> MastikApp {
        let mut app = MastikApp::headless(ctx);
        app.prefs.assistant.agent = "claude".into();
        app.canvas_rect =
            egui::Rect::from_min_size(egui::pos2(100.0, 100.0), egui::vec2(1600.0, 1200.0));
        app.canvas_px = (1600, 1200);
        app
    }

    /// A selected top-level frame has a scope button left of its ask button,
    /// level with it; a click scopes the assistant to the frame and opens the
    /// panel, and the button is lit then. A click on the lit one removes it.
    /// flip: `scope_button` placed by `ask_button_place` alone → fails at "left of".
    #[test]
    fn a_top_level_frame_has_a_scope_button_beside_its_ask_button() {
        let ctx = egui::Context::default();
        let mut app = scoping_app(&ctx);
        let page = app.session.page;
        let home = add_frame(&mut app, page, "Home");
        app.session.selection.set_one(home);
        let shown = buttons(&ctx, &app);
        let ask = shown.iter().find(|b| b.0 == AskKind::Ask).expect("ask").1;
        let (kind, scope) = shown
            .iter()
            .find(|b| matches!(b.0, AskKind::Scope { .. }))
            .cloned()
            .unwrap_or_else(|| panic!("a scope button: {shown:?}"));
        assert_eq!(
            kind,
            AskKind::Scope {
                wire: home.to_wire(),
                lit: false
            }
        );
        assert_eq!(scope.top(), ask.top(), "level with the ask button");
        assert!(
            (ask.left() - SCOPE_GAP - scope.right()).abs() < 0.01,
            "left of the ask button, {SCOPE_GAP} apart: {scope:?} {ask:?}"
        );

        app.ask_button_clicked(kind);
        assert!(app.scoped_to(home), "a click scopes");
        assert!(app.assistant.open, "and opens the panel");
        assert!(app.assistant.chips.is_empty(), "attaching nothing");
        let lit = AskKind::Scope {
            wire: home.to_wire(),
            lit: true,
        };
        assert!(
            buttons(&ctx, &app).iter().any(|b| b.0 == lit),
            "lit once scoped"
        );

        // With the panel open, the lit button stays over its frame unselected.
        app.session.selection.clear();
        assert!(buttons(&ctx, &app).iter().any(|b| b.0 == lit));

        app.ask_button_clicked(lit);
        assert!(app.assistant.scope.is_none(), "the lit button removes it");
    }

    /// A layer inside a frame, or two frames, get no scope button: a scope is
    /// one screen.
    /// flip: `scope_candidate` without its page-parent test → fails at "nested".
    #[test]
    fn only_one_top_level_frame_offers_a_scope() {
        let ctx = egui::Context::default();
        let mut app = scoping_app(&ctx);
        let page = app.session.page;
        let home = add_frame(&mut app, page, "Home");
        let hero = add_frame(&mut app, home, "Hero");
        let about = add_frame(&mut app, page, "About");
        let scope_buttons = |app: &MastikApp| {
            buttons(&ctx, app)
                .into_iter()
                .filter(|b| matches!(b.0, AskKind::Scope { .. }))
                .count()
        };
        app.session.selection.set_one(hero);
        assert_eq!(scope_buttons(&app), 0, "nested");
        app.session.selection.set_one(home);
        app.session.selection.add(about);
        assert_eq!(scope_buttons(&app), 0, "two frames");
        app.session.selection.set_one(about);
        assert_eq!(scope_buttons(&app), 1, "the fixture can show one");
    }

    /// *View ▸ Show assistant scope* off hides the unlit scope button and leaves
    /// the ask button.
    /// flip: `ask_buttons` not reading `show_scope` → fails at "the switch hides it".
    #[test]
    fn the_scope_switch_hides_the_scope_button() {
        let ctx = egui::Context::default();
        let mut app = scoping_app(&ctx);
        let page = app.session.page;
        let home = add_frame(&mut app, page, "Home");
        app.session.selection.set_one(home);
        let count = |app: &MastikApp, scope: bool| {
            buttons(&ctx, app)
                .into_iter()
                .filter(|b| matches!(b.0, AskKind::Scope { .. }) == scope)
                .count()
        };
        assert_eq!(count(&app, true), 1, "the control: on by default");
        app.set_view_switch(crate::input::ViewSwitch::AssistantScope, false);
        assert_eq!(count(&app, true), 0, "the switch hides it");
        assert_eq!(count(&app, false), 1, "and keeps the ask button");
    }

    /// A link hovered in a reply outlines its layer on this page, and nothing
    /// for a layer on another page.
    /// flip: `draw_assistant_marks` without its page check on the hovered layer →
    /// fails at "nothing for another page".
    #[test]
    fn a_hovered_layer_on_another_page_is_not_outlined_here() {
        let ctx = egui::Context::default();
        let mut app = MastikApp::headless(&ctx);
        let id = crate::palette::tests::add_rect(&mut app);
        app.assistant.open = true;
        app.assistant.hovered = Some(id.to_wire());
        let rect = egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(1600.0, 1200.0));
        let marks = |app: &MastikApp| {
            let out = ctx.run_ui(egui::RawInput::default(), |ui| {
                app.draw_assistant_marks(ui.painter(), rect, 1.0);
            });
            out.shapes.len()
        };
        let here = marks(&app);
        assert!(here > 0, "outlined on its page");
        app.add_page();
        assert_ne!(app.session.doc.page_of(id), Some(app.session.page));
        assert_eq!(marks(&app), 0, "nothing for another page");
    }
}
