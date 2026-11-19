//! *Blend shapes…* (`docs/design/app-booleans.md`, *Blend shapes*): a popover in
//! the inspector's lane that makes the steps between two shapes (`mastik_core::blend`),
//! previewed live and committed as one *Blend shapes* step.

use crate::app::MastikApp;
use crate::theme::icon;
use crate::ui::{self, FieldButton, Prefix, Scrub};
use eframe::egui;
use mastik_core::blend::{self, Blend};
use mastik_core::kurbo::Vec2;
use mastik_core::{IdSource, NodeId, Transaction};

/// The card's width: a popover's.
const W: f32 = crate::app::POPOVER_W;

/// The card's settings, kept closed so it reopens on the blend last made.
pub(crate) struct BlendCard {
    pub(crate) open: bool,
    steps: f64,
    paint: bool,
    /// Which of the nine places the outlines pair up from (`canvas::NINE_POINTS`).
    start: usize,
    reverse: bool,
    /// The path the steps follow, `None` for a straight line. Taken from the
    /// selection when the card opens, or picked on the canvas.
    spine: Option<NodeId>,
    spine_reverse: bool,
    turn: bool,
    /// Whether the canvas is waiting for a click on a path to follow.
    pub(crate) picking: bool,
    /// The preview built for the key it was built from.
    built: Option<(Key, Transaction)>,
}

impl Default for BlendCard {
    fn default() -> Self {
        Self {
            open: false,
            steps: 5.0,
            paint: true,
            start: 0,
            reverse: false,
            spine: None,
            spine_reverse: false,
            turn: false,
            picking: false,
            built: None,
        }
    }
}

/// What a preview was built from: the revision, the selection and the settings.
type Key = (u64, Vec<NodeId>, Blend);

impl BlendCard {
    /// Put back the step count a right-click cancelled the scrub of.
    pub(crate) fn restore_scrub(&mut self, steps: f64) {
        self.steps = steps;
    }

    fn blend(&self) -> Blend {
        let (col, row) = (self.start % 3, self.start / 3);
        Blend {
            steps: (self.steps.round() as usize).clamp(1, blend::MAX_STEPS),
            paint: self.paint,
            start: Vec2::new(col as f64 / 2.0, row as f64 / 2.0),
            reverse: self.reverse,
            spine: self.spine,
            spine_reverse: self.spine_reverse,
            turn: self.turn,
        }
    }

    /// The path the open card shows dashed on the canvas, if it follows one.
    pub(crate) fn shown_spine(&self) -> Option<NodeId> {
        self.spine.filter(|_| self.open)
    }
}

enum Act {
    Close,
    Commit,
    Paint,
    Reverse,
    Start(usize),
    Straight,
    Pick,
    SpineReverse,
    Turn,
}

/// What the status line says while the canvas waits for a path.
pub(crate) const PICK_STATUS: &str = "Click a path to blend along · Esc to cancel";

/// What the switch says on a straight line.
const STRAIGHT_TURNS_NOTHING: &str = "A straight line turns nothing";

impl MastikApp {
    /// Why the selection cannot be blended, if it cannot: the row's sentence.
    fn blend_refusal(&self) -> Option<&'static str> {
        blend::refusal(
            &self.session.doc,
            &self.session.resolved,
            self.session.selection.ids(),
        )
        .map(|r| r.says())
    }

    /// Open the card on the two selected shapes, or say why it cannot.
    pub(crate) fn open_blend(&mut self) {
        if let Some(why) = self.blend_refusal() {
            return self.session.info(why);
        }
        // One card in the popover lane at a time.
        self.close_inspector_popovers();
        self.close_repeat();
        self.close_warp();
        self.blend.open = true;
        self.blend.picking = false;
        self.blend.spine = blend::spine_in(&self.session.doc, self.session.selection.ids());
        self.blend.built = None;
    }

    /// Close the card and take its steps off the canvas.
    pub(crate) fn close_blend(&mut self) {
        if self.blend.open {
            self.blend.open = false;
            self.blend.picking = false;
            self.blend.built = None;
            self.session.clear_gesture_preview();
        }
    }

    /// Whether the canvas waits for a click on a path for the card to follow.
    pub(crate) fn blend_picking(&self) -> bool {
        self.blend.open && self.blend.picking
    }

    /// Stop waiting for a path, the card as it was. Answers whether it was waiting.
    pub(crate) fn cancel_blend_pick(&mut self) -> bool {
        if !self.blend_picking() {
            return false;
        }
        self.blend.picking = false;
        self.session.info("");
        true
    }

    /// The path under `world` the card could follow: a path, not one of the two
    /// shapes, that the blend could move under its group.
    pub(crate) fn blend_spine_at(&self, world: mastik_core::kurbo::Point) -> Option<NodeId> {
        let (doc, res) = (&self.session.doc, &self.session.resolved);
        let pair = blend::pair(doc, self.session.selection.ids()).ok()?;
        [self.rail_at(world), self.pick_leaf(world)]
            .into_iter()
            .flatten()
            .find(|id| blend::spine_refusal(doc, res, *id, pair).is_none())
    }

    /// A click while the canvas waits for a path: a path the card can follow is
    /// taken and the card comes back; anything else is passed over.
    pub(crate) fn blend_pick_at(&mut self, world: mastik_core::kurbo::Point) {
        if let Some(id) = self.blend_spine_at(world) {
            self.blend.spine = Some(id);
            self.blend.picking = false;
            self.session.info("");
        }
    }

    /// Whether the gesture preview is the card's steps rather than a gesture.
    pub(crate) fn blend_preview_standing(&self) -> bool {
        self.blend.open && self.drag.is_none()
    }

    /// Build the preview again when what it shows changed, and keep it up between
    /// other gestures. It is the commit's own edit, minted from a copy of the
    /// counter so it spends no ids.
    fn refresh_blend_preview(&mut self) {
        // A path deleted, locked or made one of the two under the card is let go.
        if let Some(s) = self.blend.spine {
            let (doc, res) = (&self.session.doc, &self.session.resolved);
            let fits = blend::pair(doc, self.session.selection.ids())
                .is_ok_and(|pair| blend::spine_refusal(doc, res, s, pair).is_none());
            if !fits {
                self.blend.spine = None;
            }
        }
        let b = self.blend.blend();
        let ids = self.session.selection.ids().to_vec();
        let key: Key = (self.session.revision(), ids.clone(), b);
        if !self.drag.is_none() {
            return;
        }
        if self.blend.built.as_ref().is_none_or(|(k, _)| *k != key) {
            let mut minter = IdSource::new(self.session.ids.actor());
            minter.skip_to(self.session.ids.next_seq());
            let (doc, res) = (&self.session.doc, &self.session.resolved);
            let mut tx = blend::blend(doc, res, &mut minter, &ids, b)
                .map(|(tx, _)| tx)
                .unwrap_or_default();
            // The steps, and the two shapes moved onto the spine: a preview cannot
            // reparent, and in the group they stand where their new transforms
            // put them in their own parent. The spine stays where it is drawn.
            tx.0.retain(|op| match op {
                mastik_core::Operation::InsertSubtree { .. } => true,
                mastik_core::Operation::SetTransform { id, .. } => Some(*id) != b.spine,
                _ => false,
            });
            self.session.set_preview(&tx);
            self.blend.built = Some((key, tx));
        } else if !self.session.has_gesture_preview()
            && let Some((_, tx)) = &self.blend.built
        {
            let tx = tx.clone();
            self.session.set_preview(&tx);
        }
    }

    /// Blend the two shapes as the card says, one *Blend shapes* step; the group,
    /// which takes the two shapes and the steps between, is selected.
    pub(crate) fn commit_blend(&mut self) {
        let b = self.blend.blend();
        let ids = self.session.selection.ids().to_vec();
        let made = blend::blend(
            &self.session.doc,
            &self.session.resolved,
            &mut self.session.ids,
            &ids,
            b,
        );
        let (tx, group) = match made {
            Ok(made) => made,
            Err(r) => return self.session.info(r.says()),
        };
        self.session.clear_gesture_preview();
        self.name_step_as(crate::menu::Item::BlendShapes);
        if !self.commit_edit(tx) {
            return;
        }
        self.session.selection.set_one(group);
        let steps = ui::count_as(b.steps, "step", "steps");
        self.session.info(match b.spine {
            Some(s) => format!(
                "Blended in {steps} along {}",
                crate::private::layer(&self.session.doc, s)
            ),
            None => format!("Blended in {steps}"),
        });
        self.blend.open = false;
        self.blend.picking = false;
        self.blend.built = None;
    }

    /// Say [`PICK_STATUS`] again before it fades or once another message has
    /// replaced it: while the wait lasts the card is hidden and the canvas takes
    /// no other press, so the status line is all that says how to get out.
    fn keep_pick_status(&mut self) {
        let status = self.session.status();
        let fading = status.at.elapsed() + std::time::Duration::from_secs(1)
            >= crate::app::status_hold(status);
        if status.text != PICK_STATUS || fading {
            self.session.info(PICK_STATUS);
        }
    }

    /// Draw the card when it is open, keep its preview up, and act on this
    /// frame's clicks.
    pub(crate) fn blend_ui(&mut self, ctx: &egui::Context) {
        if !self.blend.open {
            return;
        }
        if self.blend_refusal().is_some() {
            return self.close_blend();
        }
        // A popover opened in the lane after the card takes its place.
        if self.inspector_popover_open() {
            return self.close_blend();
        }
        self.refresh_blend_preview();
        // The canvas has the pointer until a path is clicked or Escape pressed, and
        // the status line says so for as long as the card is hidden.
        if self.blend.picking {
            self.keep_pick_status();
            return;
        }
        let area = self.transform_popover_area(ctx, egui::Id::new("blend-shapes"));
        let spine_name = self
            .blend
            .spine
            .map(|s| crate::private::layer(&self.session.doc, s).into_owned());
        let path_glyph = crate::panels::layers::kind_glyph(&mastik_core::NodeKind::Path {
            path: Default::default(),
            corner_radii: Vec::new(),
            corner_smoothing: 0.0,
            mirror: Default::default(),
        })
        .0;
        let card = &mut self.blend;
        // Before the field: `value_field` applies this frame's drag delta inside
        // the call that reports the drag.
        let before = card.steps;
        let mut scrubbing = false;
        let mut act = None;
        area.show(ctx, |ui| {
            ui::hold_tab(ui);
            ui::menu_frame(ui::POPOVER_PAD).show(ui, |ui| {
                let w = ui::menu_inner_w(W, ui::POPOVER_PAD);
                ui.set_width(w);
                ui.spacing_mut().item_spacing.y = ui::POPOVER_SECTION_GAP;
                if ui::popover_head(ui, "Blend shapes") {
                    act = Some(Act::Close);
                }
                ui::labelled(ui, "Steps between", |ui| {
                    let resp = ui::value_field(
                        ui,
                        egui::vec2(w, ui::CONTROL_H),
                        Prefix::Icon(icon::DOTS_THREE),
                        &mut card.steps,
                        Scrub::whole(0.1).range(1.0..=blend::MAX_STEPS as f64),
                        |d| d.custom_formatter(ui::number(0)),
                    );
                    scrubbing = resp.dragged();
                    resp.on_hover_text("How many shapes go between the two")
                });
                ui::labelled(ui, "Along", |ui| {
                    ui.scope(|ui| {
                        ui.spacing_mut().interact_size.y = ui::CONTROL_H;
                        ui.spacing_mut().button_padding.y = 0.0;
                        let shown = match &spine_name {
                            Some(name) => format!("Path · {name}"),
                            None => "A straight line".to_owned(),
                        };
                        egui::ComboBox::from_id_salt("blend-along")
                            .icon(ui::combo_chevron)
                            .width(w)
                            .selected_text(ui::glyph_and_text(path_glyph, &shown))
                            .show_ui(ui, |ui| {
                                ui::menu_rows(ui);
                                let row = ui::MENU_ROW_H;
                                if ui::menu_check(ui, "A straight line", spine_name.is_none(), row)
                                {
                                    act = Some(Act::Straight);
                                }
                                if let Some(name) = &spine_name {
                                    // Already chosen: a click keeps it.
                                    ui::menu_check(ui, &format!("Path · {name}"), true, row);
                                }
                                ui::menu_sep(ui);
                                if ui::menu_check(ui, "Pick a path…", false, row) {
                                    act = Some(Act::Pick);
                                }
                                if spine_name.is_some()
                                    && ui::menu_check(
                                        ui,
                                        "Reverse the path",
                                        card.spine_reverse,
                                        row,
                                    )
                                {
                                    act = Some(Act::SpineReverse);
                                }
                            })
                            .response
                            .on_hover_text(
                                "What the steps follow from the first shape to the second",
                            );
                    });
                });
                let along = spine_name.is_some();
                let turn = ui::disable_unless(ui, along, |ui| {
                    ui::switch_row(
                        ui,
                        "Turn the steps with the path",
                        card.turn && along,
                        crate::settings::SWITCH_H,
                    )
                })
                .inner
                .on_disabled_hover_text(STRAIGHT_TURNS_NOTHING);
                if turn.clicked() {
                    act = Some(Act::Turn);
                }
                if ui::switch_row(
                    ui,
                    "Blend paint and opacity",
                    card.paint,
                    crate::settings::SWITCH_H,
                )
                .clicked()
                {
                    act = Some(Act::Paint);
                }
                ui::labelled(ui, "Match points from", |ui| {
                    ui.scope(|ui| {
                        ui.spacing_mut().interact_size.y = ui::CONTROL_H;
                        ui.spacing_mut().button_padding.y = 0.0;
                        let mut start = card.start;
                        egui::ComboBox::from_id_salt("blend-start")
                            .icon(ui::combo_chevron)
                            .width(w)
                            .selected_text(crate::canvas::NINE_POINTS[start])
                            .show_ui(ui, |ui| {
                                ui::menu_rows(ui);
                                for (i, name) in crate::canvas::NINE_POINTS.iter().enumerate() {
                                    ui.selectable_value(&mut start, i, *name);
                                }
                            })
                            .response
                            .on_hover_text("Where on each shape the first points pair up");
                        if start != card.start {
                            act = Some(Act::Start(start));
                        }
                    });
                });
                if ui::switch_row(
                    ui,
                    "Reverse the second shape",
                    card.reverse,
                    crate::settings::SWITCH_H,
                )
                .on_hover_text("Run the second outline the other way, for a blend that twists")
                .clicked()
                {
                    act = Some(Act::Reverse);
                }
                match crate::settings::footer_row(
                    ui,
                    &[
                        ("Cancel", FieldButton::Off),
                        ("Blend", FieldButton::Primary),
                    ],
                ) {
                    Some(0) => act = Some(Act::Close),
                    Some(_) => act = Some(Act::Commit),
                    None => {}
                }
            });
        });
        // Armed after a right-click's cancel is answered, so the frame it lands on
        // cannot re-arm it.
        if scrubbing && !self.gesture_cancelled && self.in_flight.blend_scrub.is_none() {
            self.in_flight.blend_scrub = Some(before);
        }
        let Some(act) = act else {
            return;
        };
        match act {
            Act::Close => self.close_blend(),
            Act::Commit => self.commit_blend(),
            Act::Paint => self.blend.paint = !self.blend.paint,
            Act::Reverse => self.blend.reverse = !self.blend.reverse,
            Act::Start(i) => self.blend.start = i,
            Act::Straight => self.blend.spine = None,
            Act::Pick => {
                self.blend.picking = true;
                self.session.info(PICK_STATUS);
            }
            Act::SpineReverse => self.blend.spine_reverse = !self.blend.spine_reverse,
            Act::Turn if self.blend.spine.is_some() => self.blend.turn = !self.blend.turn,
            Act::Turn => {}
        }
        ctx.request_repaint();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use mastik_core::kurbo::{Affine, Size};
    use mastik_core::{Document, NodeKind, Operation};

    /// A headless app holding a circle under a square on the page, both selected.
    fn two_shapes(ctx: &egui::Context) -> (MastikApp, NodeId, NodeId) {
        let mut app = MastikApp::headless(ctx);
        let mut ids = IdSource::new(1);
        let root = ids.mint();
        let mut doc = Document::new(root);
        let page = doc.first_page();
        let (circle, square) = (ids.mint(), ids.mint());
        doc.apply(&Transaction(vec![
            Operation::CreateNode {
                id: circle,
                parent: page,
                index: 0,
                kind: NodeKind::Ellipse {
                    size: Size::new(60.0, 60.0),
                    arc: Default::default(),
                },
                transform: Some(Affine::translate((0.0, 0.0))),
                name: None,
            },
            Operation::CreateNode {
                id: square,
                parent: page,
                index: 1,
                kind: NodeKind::Rect {
                    size: Size::new(160.0, 160.0),
                    corner_radii: Default::default(),
                    corner_smoothing: 0.0,
                },
                transform: Some(Affine::translate((400.0, 0.0))),
                name: None,
            },
        ]))
        .expect("build the fixture");
        app.session.adopt_document(doc, None);
        app.canvas_px = (800, 600);
        app.session.selection.set(vec![circle, square]);
        (app, circle, square)
    }

    /// The card's commit lands the group with both shapes in it at its ends,
    /// selects it, says how many steps, and is one undo step.
    #[test]
    fn blending_lands_one_step_and_selects_the_group() {
        let ctx = egui::Context::default();
        let (mut app, circle, square) = two_shapes(&ctx);
        app.open_blend();
        assert!(app.blend.open);
        app.refresh_blend_preview();
        assert!(app.session.has_gesture_preview(), "the steps preview");
        app.commit_blend();
        let group = app
            .session
            .selection
            .single()
            .expect("the group is selected");
        let page = app.session.doc.first_page();
        assert_eq!(app.session.doc.get(page).unwrap().children(), &[group]);
        let kids = app.session.doc.get(group).unwrap().children();
        assert_eq!((kids[0], kids[kids.len() - 1]), (circle, square));
        assert_eq!(app.session.status().text, "Blended in 5 steps");
        app.undo();
        assert_eq!(
            app.session.doc.get(page).unwrap().children(),
            &[circle, square]
        );
    }

    /// The card opens in the popover lane, and a right-click mid-scrub of the
    /// steps puts them back, with the pointer on the card or out over the canvas.
    /// flip: `cancel_gesture` not restoring `blend_scrub` → fails at "the steps are back".
    #[test]
    fn a_right_click_winds_a_steps_scrub_back() {
        use crate::repeat::tests::scrub_then_right_click;
        let id = egui::Id::new("blend-shapes");
        for outside in [false, true] {
            let ctx = egui::Context::default();
            let (mut app, _, _) = two_shapes(&ctx);
            crate::repeat::tests::pass(&ctx, &mut app, Vec::new());
            app.open_blend();
            let before = app.blend.steps;
            scrub_then_right_click(
                &ctx,
                &mut app,
                id,
                "Steps between",
                |at| match outside {
                    true => egui::pos2(200.0, 400.0),
                    false => at,
                },
                |app| {
                    assert_ne!(app.blend.steps, before, "the drag scrubbed the steps");
                    assert!(app.in_flight.blend_scrub.is_some(), "the scrub is armed");
                },
            );
            assert_eq!(app.blend.steps, before, "the steps are back ({outside})");
            assert!(app.blend.open, "the right-click closed nothing");
            let card = ctx.memory(|m| m.area_rect(id)).expect("the card");
            let transform = app.transform_card.expect("the Transform card drawn");
            assert_eq!(card.top(), transform.top(), "level with the Transform card");
        }
    }

    /// One shape is refused with the row's own sentence, and no card opens.
    #[test]
    fn one_shape_is_refused_by_name() {
        let ctx = egui::Context::default();
        let (mut app, circle, _) = two_shapes(&ctx);
        app.session.selection.set_one(circle);
        app.open_blend();
        assert!(!app.blend.open);
        assert_eq!(
            app.session.status().text,
            "Blend shapes needs two shapes, and a path to follow if you like"
        );
    }

    /// `two_shapes` with a path named *Spine* from (0, 300) to (560, 300) above
    /// them, unselected.
    fn with_spine(ctx: &egui::Context) -> (MastikApp, NodeId, NodeId, NodeId) {
        let (mut app, circle, square) = two_shapes(ctx);
        let spine = app.session.ids.mint();
        let page = app.session.doc.first_page();
        let mut path = mastik_core::kurbo::BezPath::new();
        path.move_to((0.0, 300.0));
        path.line_to((560.0, 300.0));
        assert!(app.session.commit(Transaction(vec![Operation::CreateNode {
            id: spine,
            parent: page,
            index: 2,
            kind: NodeKind::Path {
                path,
                corner_radii: Vec::new(),
                corner_smoothing: 0.0,
                mirror: Default::default(),
            },
            transform: None,
            name: Some("Spine".into()),
        }])));
        app.session.selection.set(vec![circle, square]);
        (app, circle, square, spine)
    }

    /// A path selected with the two is the spine the card opens on; the commit
    /// says what it followed, and the spine stands over the group.
    /// flip: `open_blend` leaving the spine unset → fails at "the spine is chosen".
    #[test]
    fn a_path_selected_with_the_two_is_followed() {
        let ctx = egui::Context::default();
        let (mut app, circle, square, spine) = with_spine(&ctx);
        app.session.selection.set(vec![circle, square, spine]);
        app.open_blend();
        assert!(app.blend.open);
        assert_eq!(app.blend.spine, Some(spine), "the spine is chosen");
        app.commit_blend();
        let group = app.session.selection.single().expect("the group");
        assert_eq!(app.session.status().text, "Blended in 5 steps along Spine");
        let page = app.session.doc.first_page();
        assert_eq!(
            app.session.doc.get(page).unwrap().children(),
            &[group, spine]
        );
    }

    /// *Pick a path…* waits for a click on the canvas: a shape is passed over, a
    /// path is taken and the card comes back; Escape calls the wait off and leaves
    /// the card open.
    /// flip: `blend_spine_at` without its `spine_refusal` filter → fails at "a shape
    /// is passed over".
    #[test]
    fn picking_a_path_takes_only_a_path() {
        use mastik_core::kurbo::Point;
        let ctx = egui::Context::default();
        let (mut app, _, _, spine) = with_spine(&ctx);
        app.open_blend();
        app.blend.picking = true;
        app.session.info(PICK_STATUS);
        assert!(app.blend_picking());
        app.blend_pick_at(Point::new(30.0, 30.0));
        assert!(
            app.blend_picking() && app.blend.spine.is_none(),
            "a shape is passed over"
        );
        app.blend_pick_at(Point::new(280.0, 300.0));
        assert!(!app.blend_picking(), "back to the card");
        assert_eq!(app.blend.spine, Some(spine));

        app.blend.picking = true;
        app.escape(&ctx);
        assert!(
            !app.blend_picking() && app.blend.open,
            "the wait called off, card open"
        );
        assert_eq!(app.blend.spine, Some(spine), "the path kept");
    }

    /// While *Pick a path…* waits, the status line keeps saying how to get out,
    /// even once another message has replaced it. flip: `blend_ui` not calling
    /// `keep_pick_status` → fails at "said again".
    #[test]
    fn the_pick_wait_keeps_its_status() {
        let ctx = egui::Context::default();
        let (mut app, _, _, _) = with_spine(&ctx);
        app.open_blend();
        app.blend.picking = true;
        app.session.info("Something else");
        let _ = ctx.run_ui(Default::default(), |ui| app.blend_ui(ui.ctx()));
        assert_eq!(app.session.status().text, PICK_STATUS, "said again");
    }

    /// The card is as tall on a straight line as along a path: the *Along* row and
    /// the turn switch are there either way.
    /// flip: the turn switch drawn only along a path → fails at the height.
    #[test]
    fn the_card_keeps_its_height_whatever_it_follows() {
        let ctx = egui::Context::default();
        let (mut app, _, _, spine) = with_spine(&ctx);
        let id = egui::Id::new("blend-shapes");
        crate::repeat::tests::pass(&ctx, &mut app, Vec::new());
        app.open_blend();
        let height = |app: &mut MastikApp| {
            for _ in 0..3 {
                crate::repeat::tests::pass(&ctx, app, Vec::new());
            }
            ctx.memory(|m| m.area_rect(id)).expect("the card").height()
        };
        let straight = height(&mut app);
        app.blend.spine = Some(spine);
        let along = height(&mut app);
        assert_eq!(straight, along);
    }

    /// A line and a circle, whose outlines do not pair, are refused before the card
    /// opens, with the sentence a commit would give.
    /// flip: `blend_refusal` asking `blend::pair` alone → fails at "no card".
    #[test]
    fn open_against_closed_is_refused_before_the_card() {
        let ctx = egui::Context::default();
        let (mut app, circle, _) = two_shapes(&ctx);
        let line = app.session.ids.mint();
        let page = app.session.doc.first_page();
        assert!(app.session.commit(Transaction(vec![Operation::CreateNode {
            id: line,
            parent: page,
            index: 2,
            kind: NodeKind::Line {
                end: mastik_core::kurbo::Point::new(100.0, 0.0),
            },
            transform: Some(Affine::translate((200.0, 300.0))),
            name: None,
        }])));
        app.session.selection.set(vec![circle, line]);
        app.open_blend();
        assert!(!app.blend.open, "no card");
        assert_eq!(app.session.status().text, blend::Refusal::Parts.says());
    }

    /// The preview is the commit's own edit, minted from a copy of the counter: it
    /// spends no ids.
    /// flip: `refresh_blend_preview` minting from `session.ids` → fails at "no ids spent".
    #[test]
    fn the_preview_spends_no_ids() {
        let ctx = egui::Context::default();
        let (mut app, _, _) = two_shapes(&ctx);
        app.open_blend();
        let before = app.session.ids.next_seq();
        app.refresh_blend_preview();
        assert!(app.session.has_gesture_preview());
        assert_eq!(app.session.ids.next_seq(), before, "no ids spent");
    }
}
