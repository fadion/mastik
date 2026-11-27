//! *Apply brush…* (`docs/design/app-booleans.md`, *Apply brush*): a popover in
//! the inspector's lane that bends a shape along a path or scatters it along it
//! (`mastik_core::apply_brush`), previewed live and committed as one *Apply brush*
//! step.

use crate::app::MastikApp;
use crate::ui::{self, FieldButton, Prefix, Scrub};
use eframe::egui;
use mastik_core::apply_brush::{self, ApplyBrush, Art, Mode, Refusal, Scatter, Stretch, Turn};
use mastik_core::{IdSource, NodeId, Transaction};

/// The card's width: a popover's.
const W: f32 = crate::app::POPOVER_W;

/// The card's numbers, as the fields show them (percentages, the seed whole):
/// what a right-click puts back when it cancels a scrub of one.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct Numbers {
    width: f64,
    spacing: f64,
    size: f64,
    size_varies: f64,
    scatter: f64,
    seed: f64,
}

impl Default for Numbers {
    fn default() -> Self {
        let (a, s) = (Art::DEFAULT, Scatter::DEFAULT);
        Self {
            width: a.width * 100.0,
            spacing: s.spacing * 100.0,
            size: s.size * 100.0,
            size_varies: s.size_varies * 100.0,
            scatter: s.scatter * 100.0,
            seed: f64::from(s.seed),
        }
    }
}

/// The card's settings, kept closed so it reopens on the brush last applied.
#[derive(Default)]
pub(crate) struct BrushCard {
    pub(crate) open: bool,
    /// The path the brush runs along and the shape it is made of, taken from the
    /// selection when the card opens; the brush may be picked on the canvas.
    path: Option<NodeId>,
    brush: Option<NodeId>,
    /// *Scatter* rather than *Art*.
    pub(crate) scatter: bool,
    numbers: Numbers,
    middle: bool,
    flip: bool,
    fixed: bool,
    keep_path: bool,
    /// Whether the canvas is waiting for a click on a shape to use as the brush.
    pub(crate) picking: bool,
    /// The selection the card opened on: another closes it.
    opened_on: Vec<NodeId>,
    /// The preview built for the key it was built from.
    built: Option<(Key, Transaction)>,
}

/// What a preview was built from: the revision and the application.
type Key = (u64, ApplyBrush);

impl BrushCard {
    /// Put back the numbers a right-click cancelled the scrub of.
    pub(crate) fn restore_scrub(&mut self, numbers: Numbers) {
        self.numbers = numbers;
    }

    /// The application the card describes, while it has a path and a brush.
    fn how(&self) -> Option<ApplyBrush> {
        let n = self.numbers;
        let mode = match self.scatter {
            false => Mode::Art(Art {
                width: n.width / 100.0,
                stretch: match self.middle {
                    true => Stretch::Middle,
                    false => Stretch::Whole,
                },
                flip: self.flip,
            }),
            true => Mode::Scatter(Scatter {
                spacing: n.spacing / 100.0,
                size: n.size / 100.0,
                size_varies: n.size_varies / 100.0,
                scatter: n.scatter / 100.0,
                turn: match self.fixed {
                    true => Turn::Fixed,
                    false => Turn::Along,
                },
                seed: n.seed.round().clamp(0.0, f64::from(u32::MAX)) as u32,
            }),
        };
        Some(ApplyBrush {
            path: self.path?,
            brush: self.brush?,
            mode,
            keep_path: self.keep_path,
        })
    }

    /// The path the open card shows dashed on the canvas.
    pub(crate) fn shown_path(&self) -> Option<NodeId> {
        self.path.filter(|_| self.open)
    }
}

enum Act {
    Close,
    Commit,
    Pick,
    Swap,
    Scatter(bool),
    Middle(bool),
    Flip,
    Fixed(bool),
    KeepPath,
}

/// What the status line says while the canvas waits for a shape.
pub(crate) const PICK_STATUS: &str = "Click a shape to use as the brush · Esc to cancel";

/// The undo step a commit makes.
const STEP: &str = "Apply brush";

/// The row's and the palette's sentence when the selection cannot take a brush.
pub(crate) const NEEDS: &str = "Select a path, and the shape to run along it";

impl MastikApp {
    /// Why the selection cannot take a brush, if it cannot: the row's sentence.
    pub(crate) fn brush_refusal(&self) -> Option<&'static str> {
        apply_brush::selection_refusal(
            &self.session.doc,
            &self.session.resolved,
            self.session.selection.ids(),
        )
        .map(|r| r.says())
    }

    /// Why the card's own path and brush no longer fit, if they do not.
    fn card_refusal(&self) -> Option<Refusal> {
        let (Some(path), Some(brush)) = (self.brush.path, self.brush.brush) else {
            return Some(Refusal::Count);
        };
        apply_brush::refusal(&self.session.doc, &self.session.resolved, path, brush)
    }

    /// Open the card on the two selected layers, or say why it cannot.
    pub(crate) fn open_brush(&mut self) {
        if let Some(why) = self.brush_refusal() {
            return self.session.info(why);
        }
        let Ok((path, brush)) = apply_brush::pair(&self.session.doc, self.session.selection.ids())
        else {
            return self.session.info(NEEDS);
        };
        // One card in the popover lane at a time.
        self.close_inspector_popovers();
        self.close_repeat();
        self.close_blend();
        self.close_warp();
        let card = &mut self.brush;
        card.open = true;
        card.picking = false;
        card.path = Some(path);
        card.brush = Some(brush);
        card.opened_on = self.session.selection.ids().to_vec();
        card.built = None;
    }

    /// Close the card and take its preview off the canvas.
    pub(crate) fn close_brush(&mut self) {
        if self.brush.open {
            self.brush.open = false;
            self.brush.picking = false;
            self.brush.built = None;
            self.session.clear_gesture_preview();
        }
    }

    /// Whether the canvas waits for a click on a shape for the card's brush.
    pub(crate) fn brush_picking(&self) -> bool {
        self.brush.open && self.brush.picking
    }

    /// Stop waiting for a shape, the card as it was. Answers whether it was waiting.
    pub(crate) fn cancel_brush_pick(&mut self) -> bool {
        if !self.brush_picking() {
            return false;
        }
        self.brush.picking = false;
        self.session.info("");
        true
    }

    /// What is under `world` for the wait: the outline under the pointer, then the
    /// leaf.
    fn brush_hits(&self, world: mastik_core::kurbo::Point) -> [Option<NodeId>; 2] {
        [self.rail_at(world), self.pick_leaf(world)]
    }

    /// The shape under `world` the card could take as its brush: one that is not
    /// the path and has an outline to lay along it.
    pub(crate) fn brush_shape_at(&self, world: mastik_core::kurbo::Point) -> Option<NodeId> {
        let (doc, res) = (&self.session.doc, &self.session.resolved);
        self.brush_hits(world).into_iter().flatten().find(|id| {
            Some(*id) != self.brush.path && apply_brush::brush_refusal(doc, res, *id).is_none()
        })
    }

    /// A click while the canvas waits for a shape: a shape the card can take
    /// becomes the brush and the card comes back; the path itself is refused by
    /// name; anything else is passed over.
    pub(crate) fn brush_pick_at(&mut self, world: mastik_core::kurbo::Point) {
        if let Some(id) = self.brush_shape_at(world) {
            self.brush.brush = Some(id);
            self.brush.picking = false;
            self.session.info("");
        } else if self.brush.path.is_some() && self.brush_hits(world).contains(&self.brush.path) {
            self.session.info(Refusal::Same.says());
        }
    }

    /// Swap the card's brush and path, or say why the swap cannot run.
    fn swap_brush(&mut self) {
        let (Some(path), Some(brush)) = (self.brush.path, self.brush.brush) else {
            return;
        };
        let (doc, res) = (&self.session.doc, &self.session.resolved);
        match apply_brush::refusal(doc, res, brush, path) {
            Some(r) => self.session.info(r.says()),
            None => (self.brush.path, self.brush.brush) = (Some(brush), Some(path)),
        }
    }

    /// Whether the gesture preview is the card's result rather than a gesture.
    pub(crate) fn brush_preview_standing(&self) -> bool {
        self.brush.open && self.drag.is_none()
    }

    /// Build the preview again when what it shows changed, and keep it up between
    /// other gestures. It is the commit's own edit, minted from a copy of the
    /// counter so it spends no ids: the group, and the path hidden, since a
    /// preview can neither delete nor reparent.
    fn refresh_brush_preview(&mut self) {
        let Some(how) = self.brush.how() else {
            return;
        };
        if !self.drag.is_none() {
            return;
        }
        let key: Key = (self.session.revision(), how);
        if self.brush.built.as_ref().is_none_or(|(k, _)| *k != key) {
            let mut minter = IdSource::new(self.session.ids.actor());
            minter.skip_to(self.session.ids.next_seq());
            let (doc, res) = (&self.session.doc, &self.session.resolved);
            let mut tx = apply_brush::apply(doc, res, &mut minter, &how)
                .map(|(tx, _)| tx)
                .unwrap_or_default();
            tx.0.retain(|op| matches!(op, mastik_core::Operation::InsertSubtree { .. }));
            if !tx.0.is_empty() {
                tx.0.push(mastik_core::Operation::SetVisible {
                    id: how.path,
                    visible: false,
                });
            }
            self.session.set_preview(&tx);
            self.brush.built = Some((key, tx));
        } else if !self.session.has_gesture_preview()
            && let Some((_, tx)) = &self.brush.built
        {
            let tx = tx.clone();
            self.session.set_preview(&tx);
        }
    }

    /// Apply the brush as the card says, one *Apply brush* step; the new group is
    /// selected.
    pub(crate) fn commit_brush(&mut self) {
        let Some(how) = self.brush.how() else {
            return self.session.info(NEEDS);
        };
        let made = apply_brush::apply(
            &self.session.doc,
            &self.session.resolved,
            &mut self.session.ids,
            &how,
        );
        let (tx, group) = match made {
            Ok(made) => made,
            Err(r) => return self.session.info(r.says()),
        };
        let name = crate::private::layer(&self.session.doc, how.brush).into_owned();
        self.session.clear_gesture_preview();
        // The row's label less its ellipsis, which promises a card, not a step.
        if !self.session.has_step_name() {
            self.session
                .name_step(STEP, crate::menu::Item::ApplyBrush.spec().glyph);
        }
        if !self.commit_edit(tx) {
            return;
        }
        self.session.selection.set_one(group);
        let copies = self
            .session
            .doc
            .get(group)
            .map_or(0, |g| g.children().len() - usize::from(how.keep_path));
        self.session.info(match how.mode {
            Mode::Art(_) => format!("Brushed with {name}"),
            Mode::Scatter(_) => format!(
                "Scattered {} of {name}",
                ui::count_as(copies, "copy", "copies")
            ),
        });
        self.brush.open = false;
        self.brush.picking = false;
        self.brush.built = None;
    }

    /// Say [`PICK_STATUS`] again before it fades or once another message has
    /// replaced it, as *Blend shapes…*'s wait does, but leave a refused pick's
    /// sentence and an error up for their hold.
    fn keep_brush_pick_status(&mut self) {
        let status = self.session.status();
        let fading = status.at.elapsed() + std::time::Duration::from_secs(1)
            >= crate::app::status_hold(status);
        let other = status.text != PICK_STATUS
            && status.text != Refusal::Same.says()
            && status.kind != crate::session::StatusKind::Error;
        if other || fading {
            self.session.info(PICK_STATUS);
        }
    }

    /// Draw the card when it is open, keep its preview up, and act on this
    /// frame's clicks.
    pub(crate) fn brush_ui(&mut self, ctx: &egui::Context) {
        if !self.brush.open {
            return;
        }
        if self.card_refusal().is_some() || self.session.selection.ids() != self.brush.opened_on {
            return self.close_brush();
        }
        // A popover opened in the lane after the card takes its place.
        if self.inspector_popover_open() {
            return self.close_brush();
        }
        self.refresh_brush_preview();
        // The canvas has the pointer until a shape is clicked or Escape pressed, and
        // the status line says so for as long as the card is hidden.
        if self.brush.picking {
            self.keep_brush_pick_status();
            return;
        }
        let area = self.transform_popover_area(ctx, egui::Id::new("apply-brush"));
        let (brush_name, brush_glyph) = match self.brush.brush.and_then(|b| self.session.doc.get(b))
        {
            Some(n) => (
                crate::private::layer(&self.session.doc, n.id()).into_owned(),
                crate::panels::layers::kind_glyph(n.kind()).0,
            ),
            None => (String::new(), ""),
        };
        let card = &mut self.brush;
        // Before the fields: `value_field` applies this frame's drag delta inside
        // the call that reports the drag.
        let before = card.numbers;
        let mut scrubbing = false;
        let mut act = None;
        area.show(ctx, |ui| {
            ui::hold_tab(ui);
            ui::menu_frame(ui::POPOVER_PAD).show(ui, |ui| {
                let w = ui::menu_inner_w(W, ui::POPOVER_PAD);
                ui.set_width(w);
                ui.spacing_mut().item_spacing.y = ui::POPOVER_SECTION_GAP;
                if ui::popover_head(ui, "Apply brush") {
                    act = Some(Act::Close);
                }
                ui::labelled(ui, "Brush", |ui| {
                    ui.scope(|ui| {
                        ui.spacing_mut().interact_size.y = ui::CONTROL_H;
                        ui.spacing_mut().button_padding.y = 0.0;
                        let shown = format!("Shape · {brush_name}");
                        egui::ComboBox::from_id_salt("apply-brush-shape")
                            .icon(ui::combo_chevron)
                            .width(w)
                            .selected_text(ui::glyph_and_text(brush_glyph, &shown))
                            .show_ui(ui, |ui| {
                                ui::menu_rows(ui);
                                let row = ui::MENU_ROW_H;
                                // Already chosen: a click keeps it.
                                ui::menu_check(ui, &shown, true, row);
                                ui::menu_sep(ui);
                                if ui::menu_check(ui, "Pick a shape…", false, row) {
                                    act = Some(Act::Pick);
                                }
                                if ui::menu_check(ui, "Swap brush and path", false, row) {
                                    act = Some(Act::Swap);
                                }
                            })
                            .response
                            .on_hover_text("The shape bent or repeated along the path");
                    });
                });
                if let Some(i) = ui::segmented(
                    ui,
                    w,
                    ui::SEGMENT_CELL_H,
                    2,
                    usize::from(card.scatter),
                    |p, i, r, on| ui::segment_label(p, r, ["Art", "Scatter"][i], on),
                ) {
                    act = Some(Act::Scatter(i == 1));
                }
                // Both modes' rows take Scatter's height, so the footer stays put
                // and the card keeps its size as the mode changes.
                let body_h = body_height(ui);
                ui.vertical(|ui| {
                    ui.set_width(w);
                    ui.set_min_height(body_h);
                    let fw = (w - ui::CARD_COL_GAP) / 2.0;
                    let n = &mut card.numbers;
                    let mut field = |ui: &mut egui::Ui,
                                     width: f32,
                                     value: &mut f64,
                                     range: std::ops::RangeInclusive<f64>,
                                     tip: &str| {
                        let resp = ui::value_field(
                            ui,
                            egui::vec2(width, ui::CONTROL_H),
                            Prefix::Text(""),
                            value,
                            Scrub::whole(0.5).range(range),
                            crate::trace::percent,
                        );
                        scrubbing |= resp.dragged();
                        resp.on_hover_text(tip);
                    };
                    if !card.scatter {
                        ui::labelled(ui, "Width", |ui| {
                            field(
                                ui,
                                w,
                                &mut n.width,
                                1.0..=1000.0,
                                "How tall the shape draws across the path",
                            );
                        });
                        ui::labelled(ui, "Stretch", |ui| {
                            if let Some(i) = ui::segmented(
                                ui,
                                w,
                                ui::SEGMENT_CELL_H,
                                2,
                                usize::from(card.middle),
                                |p, i, r, on| {
                                    ui::segment_label(p, r, ["Whole shape", "Middle only"][i], on)
                                },
                            ) {
                                act = Some(Act::Middle(i == 1));
                            }
                        });
                        if ui::switch_row(ui, "Flip across", card.flip, crate::settings::SWITCH_H)
                            .on_hover_text("Mirror the shape over the path")
                            .clicked()
                        {
                            act = Some(Act::Flip);
                        }
                        return;
                    }
                    crate::repeat::pair(ui, |ui, left| match left {
                        true => ui::labelled(ui, "Spacing", |ui| {
                            field(
                                ui,
                                fw,
                                &mut n.spacing,
                                10.0..=1000.0,
                                "From one copy to the next, of the shape's length",
                            );
                        }),
                        false => ui::labelled(ui, "Size", |ui| {
                            field(
                                ui,
                                fw,
                                &mut n.size,
                                1.0..=1000.0,
                                "Each copy's size, of the shape's",
                            );
                        }),
                    });
                    crate::repeat::pair(ui, |ui, left| match left {
                        true => ui::labelled(ui, "Size varies", |ui| {
                            field(
                                ui,
                                fw,
                                &mut n.size_varies,
                                0.0..=100.0,
                                "How far a copy's size strays either way",
                            );
                        }),
                        false => ui::labelled(ui, "Scatter", |ui| {
                            field(
                                ui,
                                fw,
                                &mut n.scatter,
                                0.0..=500.0,
                                "How far a copy strays off the path, of the shape's length",
                            );
                        }),
                    });
                    ui::labelled(ui, "Turn", |ui| {
                        if let Some(i) = ui::segmented(
                            ui,
                            w,
                            ui::SEGMENT_CELL_H,
                            2,
                            usize::from(card.fixed),
                            |p, i, r, on| {
                                ui::segment_label(p, r, ["Along the path", "Fixed"][i], on)
                            },
                        ) {
                            act = Some(Act::Fixed(i == 1));
                        }
                    });
                    ui::labelled(ui, "Seed", |ui| {
                        let resp = ui::value_field(
                            ui,
                            egui::vec2(w, ui::CONTROL_H),
                            Prefix::Text(""),
                            &mut n.seed,
                            Scrub::whole(0.25).range(0.0..=f64::from(u32::MAX)),
                            |d| d.custom_formatter(ui::number(0)),
                        );
                        scrubbing |= resp.dragged();
                        resp.on_hover_text("Which sizes and offsets: another number scatters anew");
                    });
                });
                if ui::switch_row(
                    ui,
                    "Keep the path",
                    card.keep_path,
                    crate::settings::SWITCH_H,
                )
                .on_hover_text("Keep the path in the group, hidden")
                .clicked()
                {
                    act = Some(Act::KeepPath);
                }
                match crate::settings::footer_row(
                    ui,
                    &[
                        ("Cancel", FieldButton::Off),
                        ("Apply", FieldButton::Primary),
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
        if scrubbing && !self.gesture_cancelled && self.in_flight.brush_scrub.is_none() {
            self.in_flight.brush_scrub = Some(before);
        }
        let Some(act) = act else {
            return;
        };
        match act {
            Act::Close => self.close_brush(),
            Act::Commit => self.commit_brush(),
            Act::Pick => {
                self.brush.picking = true;
                self.session.info(PICK_STATUS);
            }
            Act::Swap => self.swap_brush(),
            Act::Scatter(on) => self.brush.scatter = on,
            Act::Middle(on) => self.brush.middle = on,
            Act::Flip => self.brush.flip = !self.brush.flip,
            Act::Fixed(on) => self.brush.fixed = on,
            Act::KeepPath => self.brush.keep_path = !self.brush.keep_path,
        }
        ctx.request_repaint();
    }
}

/// The height both modes' rows take: Scatter's, the taller — four labelled rows.
fn body_height(ui: &egui::Ui) -> f32 {
    // A laid-out word, not the font's row height: a label takes its galley's
    // height, which egui rounds to the pixel.
    let font = egui::FontId::proportional(ui::EYEBROW_PT);
    let eyebrow = ui
        .painter()
        .layout_no_wrap("SEED".into(), font, crate::theme::text::faint())
        .size()
        .y;
    let labelled = eyebrow + ui::SECTION_LABEL_GAP + ui::CONTROL_H;
    4.0 * labelled + 3.0 * ui::POPOVER_SECTION_GAP
}

#[cfg(test)]
mod tests {
    use super::*;
    use mastik_core::kurbo::{Affine, BezPath, Point, Size};
    use mastik_core::{Document, NodeKind, Operation};

    /// A headless app holding a path from (0, 300) to (600, 300) under a 60 × 20
    /// ellipse named *Leaf*, both selected.
    fn path_and_leaf(ctx: &egui::Context) -> (MastikApp, NodeId, NodeId) {
        let mut app = MastikApp::headless(ctx);
        let mut ids = IdSource::new(1);
        let root = ids.mint();
        let mut doc = Document::new(root);
        let page = doc.first_page();
        let (path, leaf) = (ids.mint(), ids.mint());
        let mut line = BezPath::new();
        line.move_to((0.0, 300.0));
        line.line_to((600.0, 300.0));
        doc.apply(&Transaction(vec![
            Operation::CreateNode {
                id: path,
                parent: page,
                index: 0,
                kind: NodeKind::Path {
                    path: line,
                    corner_radii: Vec::new(),
                    corner_smoothing: 0.0,
                    mirror: Default::default(),
                },
                transform: None,
                name: Some("Line".into()),
            },
            Operation::CreateNode {
                id: leaf,
                parent: page,
                index: 1,
                kind: NodeKind::Ellipse {
                    size: Size::new(60.0, 20.0),
                    arc: Default::default(),
                },
                transform: Some(Affine::translate((100.0, 0.0))),
                name: Some("Leaf".into()),
            },
        ]))
        .expect("build the fixture");
        app.session.adopt_document(doc, None);
        app.canvas_px = (800, 600);
        app.session.selection.set(vec![path, leaf]);
        (app, path, leaf)
    }

    /// The card's commit puts a *Brushed · Leaf* group in the path's place and
    /// selects it, keeps the brush, names the step *Apply brush*, and one undo
    /// puts the path back.
    /// flip: `commit_brush` naming the step by the row's label → fails at the step
    /// name.
    #[test]
    fn applying_lands_one_named_step_and_selects_the_group() {
        let ctx = egui::Context::default();
        let (mut app, path, leaf) = path_and_leaf(&ctx);
        app.open_brush();
        assert!(app.brush.open);
        app.refresh_brush_preview();
        assert!(app.session.has_gesture_preview(), "the result previews");
        app.commit_brush();
        let group = app
            .session
            .selection
            .single()
            .expect("the group is selected");
        let page = app.session.doc.first_page();
        assert_eq!(
            app.session.doc.get(page).unwrap().children(),
            &[group, leaf]
        );
        assert_eq!(app.session.doc.get(group).unwrap().name(), "Brushed · Leaf");
        assert!(app.session.doc.get(path).is_none(), "the path goes");
        assert_eq!(app.session.status().text, "Brushed with Leaf");
        let step = app.session.history.done().last().expect("a step");
        assert_eq!(step.note.label, "Apply brush", "the step name");
        app.undo();
        assert_eq!(app.session.doc.get(page).unwrap().children(), &[path, leaf]);
    }

    /// With *Keep the path* on, the path lands at the group's bottom, hidden and
    /// named *Path*; a scatter makes one path per copy and says how many.
    #[test]
    fn a_kept_path_sits_hidden_under_the_copies() {
        let ctx = egui::Context::default();
        let (mut app, path, _) = path_and_leaf(&ctx);
        app.open_brush();
        app.brush.keep_path = true;
        app.brush.scatter = true;
        app.commit_brush();
        let group = app.session.selection.single().expect("the group");
        let kids = app.session.doc.get(group).unwrap().children().to_vec();
        assert_eq!(kids[0], path);
        let kept = app.session.doc.get(path).unwrap();
        assert!(!kept.visible());
        assert_eq!(kept.name(), "Path");
        // 600 along at 72 a step: nine copies.
        assert_eq!(kids.len(), 10);
        assert_eq!(app.session.status().text, "Scattered 9 copies of Leaf");
    }

    /// *Cancel* takes the preview off and leaves the document as it was.
    /// flip: `close_brush` keeping the gesture preview → fails at "nothing shown".
    #[test]
    fn cancel_leaves_everything_as_it_was() {
        let ctx = egui::Context::default();
        let (mut app, path, leaf) = path_and_leaf(&ctx);
        let revision = app.session.revision();
        app.open_brush();
        app.refresh_brush_preview();
        assert!(app.session.has_gesture_preview());
        app.close_brush();
        assert!(!app.session.has_gesture_preview(), "nothing shown");
        assert_eq!(app.session.revision(), revision);
        let page = app.session.doc.first_page();
        assert_eq!(app.session.doc.get(page).unwrap().children(), &[path, leaf]);
    }

    /// More than two layers, and two with nothing to run along, are refused with
    /// the row's sentence, and no card opens.
    #[test]
    fn a_selection_without_a_path_is_refused_by_name() {
        let ctx = egui::Context::default();
        let (mut app, path, leaf) = path_and_leaf(&ctx);
        app.session.selection.set_one(leaf);
        app.open_brush();
        assert!(!app.brush.open);
        assert_eq!(app.session.status().text, NEEDS);
        let third = app.session.ids.mint();
        let page = app.session.doc.first_page();
        assert!(app.session.commit(Transaction(vec![Operation::CreateNode {
            id: third,
            parent: page,
            index: 2,
            kind: NodeKind::Rect {
                size: Size::new(10.0, 10.0),
                corner_radii: Default::default(),
                corner_smoothing: 0.0,
            },
            transform: None,
            name: None,
        }])));
        app.session.selection.set(vec![path, leaf, third]);
        assert_eq!(app.brush_refusal(), Some(NEEDS));
        app.open_brush();
        assert!(!app.brush.open);
    }

    /// *Pick a shape…* waits for a click on the canvas: the path is refused by
    /// name and the wait goes on, a shape becomes the brush and the card comes
    /// back; Escape calls a wait off and leaves the card open.
    /// flip: `brush_shape_at` without its path filter → fails at "the path is
    /// refused".
    #[test]
    fn picking_takes_a_shape_but_not_the_path() {
        let ctx = egui::Context::default();
        let (mut app, path, leaf) = path_and_leaf(&ctx);
        let other = app.session.ids.mint();
        let page = app.session.doc.first_page();
        assert!(app.session.commit(Transaction(vec![Operation::CreateNode {
            id: other,
            parent: page,
            index: 2,
            kind: NodeKind::Rect {
                size: Size::new(40.0, 40.0),
                corner_radii: Default::default(),
                corner_smoothing: 0.0,
            },
            transform: Some(Affine::translate((400.0, 0.0))),
            name: None,
        }])));
        app.session.selection.set(vec![path, leaf]);
        app.open_brush();
        app.brush.picking = true;
        app.brush_pick_at(Point::new(300.0, 300.0));
        assert!(
            app.brush_picking() && app.brush.brush == Some(leaf),
            "the path is refused"
        );
        assert_eq!(app.session.status().text, Refusal::Same.says());
        app.brush_pick_at(Point::new(420.0, 20.0));
        assert!(!app.brush_picking(), "back to the card");
        assert_eq!(app.brush.brush, Some(other));

        app.brush.picking = true;
        app.escape(&ctx);
        assert!(
            !app.brush_picking() && app.brush.open,
            "the wait called off, card open"
        );
        app.escape(&ctx);
        assert!(!app.brush.open, "the next press closes the card");
    }

    /// While *Pick a shape…* waits, the status line keeps saying how to get out
    /// once another message replaces it, but leaves a refused pick's sentence up.
    /// flip: `keep_brush_pick_status` re-saying over the refusal → fails at "the
    /// refusal stays".
    #[test]
    fn the_pick_wait_keeps_its_status() {
        let ctx = egui::Context::default();
        let (mut app, _, _) = path_and_leaf(&ctx);
        app.open_brush();
        app.brush.picking = true;
        app.session.info("Something else");
        let _ = ctx.run_ui(Default::default(), |ui| app.brush_ui(ui.ctx()));
        assert_eq!(app.session.status().text, PICK_STATUS, "said again");
        app.session.info(Refusal::Same.says());
        let _ = ctx.run_ui(Default::default(), |ui| app.brush_ui(ui.ctx()));
        assert_eq!(
            app.session.status().text,
            Refusal::Same.says(),
            "the refusal stays"
        );
    }

    /// *Swap brush and path* trades the two when the swapped pair fits.
    #[test]
    fn swapping_trades_brush_and_path() {
        let ctx = egui::Context::default();
        let (mut app, path, leaf) = path_and_leaf(&ctx);
        app.open_brush();
        app.swap_brush();
        assert_eq!((app.brush.path, app.brush.brush), (Some(leaf), Some(path)));
    }

    /// The card is as tall in Art as in Scatter, so the footer stays put.
    /// flip: `brush_ui` without the body's `set_min_height` → fails at the height.
    #[test]
    fn the_card_keeps_its_height_in_either_mode() {
        let ctx = egui::Context::default();
        let (mut app, _, _) = path_and_leaf(&ctx);
        let id = egui::Id::new("apply-brush");
        crate::repeat::tests::pass(&ctx, &mut app, Vec::new());
        app.open_brush();
        let height = |app: &mut MastikApp| {
            for _ in 0..3 {
                crate::repeat::tests::pass(&ctx, app, Vec::new());
            }
            ctx.memory(|m| m.area_rect(id)).expect("the card").height()
        };
        let art = height(&mut app);
        app.brush.scatter = true;
        let scatter = height(&mut app);
        assert_eq!(art, scatter);
    }

    /// A right-click mid-scrub of *Width* puts it back, and the card opens level
    /// with the Transform card.
    /// flip: `cancel_gesture` not restoring `brush_scrub` → fails at "the width is
    /// back".
    #[test]
    fn a_right_click_winds_a_width_scrub_back() {
        use crate::repeat::tests::scrub_then_right_click;
        let id = egui::Id::new("apply-brush");
        let ctx = egui::Context::default();
        let (mut app, _, _) = path_and_leaf(&ctx);
        crate::repeat::tests::pass(&ctx, &mut app, Vec::new());
        app.open_brush();
        let before = app.brush.numbers;
        scrub_then_right_click(
            &ctx,
            &mut app,
            id,
            "Width",
            |at| at,
            |app| {
                assert_ne!(app.brush.numbers, before, "the drag scrubbed the width");
                assert!(app.in_flight.brush_scrub.is_some(), "the scrub is armed");
            },
        );
        assert_eq!(app.brush.numbers, before, "the width is back");
        assert!(app.brush.open, "the right-click closed nothing");
        let card = ctx.memory(|m| m.area_rect(id)).expect("the card");
        let transform = app.transform_card.expect("the Transform card drawn");
        assert_eq!(card.top(), transform.top(), "level with the Transform card");
    }

    /// The preview spends no ids.
    /// flip: `refresh_brush_preview` minting from `session.ids` → fails at "no ids
    /// spent".
    #[test]
    fn the_preview_spends_no_ids() {
        let ctx = egui::Context::default();
        let (mut app, _, _) = path_and_leaf(&ctx);
        app.open_brush();
        let before = app.session.ids.next_seq();
        app.refresh_brush_preview();
        assert!(app.session.has_gesture_preview());
        assert_eq!(app.session.ids.next_seq(), before, "no ids spent");
    }
}
