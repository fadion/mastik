//! The image card's Curves tab: one master tone curve for R, G and B together,
//! drawn over the picture's luminance histogram.
//!
//! The curve's ends are the black point, white point and fade amounts the
//! Adjustments tab shows as rows, so both controls write the same three numbers.
//! Its inner points are `ImageAdjust::curve`, stored between those ends.

use super::inspector::menu_action;
use super::paint::{self, PaintSlot};
use crate::app::MastikApp;
use crate::theme::{self, color, icon};
use crate::ui::{self, Prefix, Scrub};
use eframe::egui;
use mastik_core::{Brush, CURVE_POINTS, ImageAdjust, ImageId, ImageRef, NodeId};
use mastik_render::HISTOGRAM_BINS;
use std::sync::Arc;

/// One thing on the graph a pointer can hold.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Handle {
    /// The black end: black point across, fade up.
    Black,
    /// The white end: white point across; it always maps to white.
    White,
    /// An inner point, by its index in `ImageAdjust::curve`.
    Inner(usize),
}

/// A drag on the graph: the handle held, and the adjustment it started from,
/// holding the point the press added when it began on the curve.
#[derive(Clone, Copy)]
struct Drag {
    handle: Handle,
    base: ImageAdjust,
}

/// The graph's inset from its ground, so an end's square at a corner is drawn whole.
const GRAPH_PAD: f32 = 5.0;
/// How near the pointer must be to a handle to take it.
const HIT_R: f32 = 7.0;
/// An end's square, a side.
const END_SIDE: f32 = 7.0;
/// An inner point's radius.
const POINT_R: f32 = 3.5;
/// The tick on the graph's bottom edge under each end's input.
const TICK_H: f32 = 4.0;
/// How far past the graph's edge a dragged inner point goes before it is removed.
const OFF_GRAPH: f32 = 16.0;
/// The least input gap a handle keeps from its neighbours, so points stay in
/// order and the ends never meet under a drag.
const MIN_GAP: f32 = 0.01;
/// The smallest the graph is drawn, however short the Settings tab.
const GRAPH_MIN_H: f32 = 80.0;
/// What the graph's tooltip says, on every handle and on the curve.
const GRAPH_TIP: &str = "Click to add a point, drag it off to remove it";

/// `[input, output]` in `0..=1` → a point in `plot`, black at the bottom left.
fn to_screen(plot: egui::Rect, p: [f32; 2]) -> egui::Pos2 {
    egui::pos2(
        plot.left() + p[0] * plot.width(),
        plot.bottom() - p[1] * plot.height(),
    )
}

/// A point in `plot` → `[input, output]`, unclamped.
fn from_screen(plot: egui::Rect, pos: egui::Pos2) -> [f32; 2] {
    [
        (pos.x - plot.left()) / plot.width().max(1.0),
        (plot.bottom() - pos.y) / plot.height().max(1.0),
    ]
}

/// Where `h` sits on the graph, or [`None`] for an inner point that is gone.
fn position(a: &ImageAdjust, h: Handle) -> Option<[f32; 2]> {
    let (black, white) = a.curve_ends();
    match h {
        Handle::Black => Some(black),
        Handle::White => Some(white),
        Handle::Inner(i) => a.curve.as_slice().get(i).map(|p| a.curve_shown(*p)),
    }
}

/// Every handle on `a`, inner points before the ends so a point lying on an end
/// is the one taken.
fn handles(a: &ImageAdjust) -> impl Iterator<Item = (Handle, [f32; 2])> + '_ {
    (0..a.curve.len())
        .map(Handle::Inner)
        .chain([Handle::Black, Handle::White])
        .filter_map(|h| position(a, h).map(|p| (h, p)))
}

/// The handle nearest `pos` within [`HIT_R`].
fn hit(a: &ImageAdjust, plot: egui::Rect, pos: egui::Pos2) -> Option<Handle> {
    handles(a)
        .map(|(h, p)| (h, to_screen(plot, p).distance(pos)))
        .filter(|(_, d)| *d <= HIT_R)
        .min_by(|x, y| x.1.total_cmp(&y.1))
        .map(|(h, _)| h)
}

/// `a` with a point added on the curve at input `x`, and its index, or [`None`]
/// when the curve is full or `x` is not clear of both ends and every point.
/// The point lands where the curve already is, so adding one changes nothing
/// until it is moved.
fn added(a: &ImageAdjust, x: f32) -> Option<(ImageAdjust, usize)> {
    let ([x0, _], [x1, _]) = a.curve_ends();
    if a.curve.len() >= CURVE_POINTS || x <= x0 + MIN_GAP || x >= x1 - MIN_GAP {
        return None;
    }
    if a.curve
        .as_slice()
        .iter()
        .any(|p| (a.curve_shown(*p)[0] - x).abs() < MIN_GAP)
    {
        return None;
    }
    let mut out = *a;
    let i = out.curve.insert(a.curve_stored([x, a.curve_at(x)]))?;
    Some((out, i))
}

/// `base` with `h` moved to `to` on the graph, held between its neighbours, or
/// with the inner point removed when `off` says it was dragged off the graph.
fn moved(base: &ImageAdjust, h: Handle, to: [f32; 2], off: bool) -> ImageAdjust {
    let mut a = *base;
    let ([bx, _], [wx, _]) = base.curve_ends();
    let clamp = |v: f32, lo: f32, hi: f32| if lo <= hi { v.clamp(lo, hi) } else { lo };
    match h {
        Handle::Black => {
            a.black_point = clamp(to[0], 0.0, wx - MIN_GAP).max(0.0);
            a.fade = to[1].clamp(0.0, 1.0);
        }
        Handle::White => {
            a.white_point = 1.0 - clamp(to[0], bx + MIN_GAP, 1.0).min(1.0);
        }
        Handle::Inner(i) => {
            if off {
                a.curve.remove(i);
                return a;
            }
            let pts = base.curve.as_slice();
            let left = i
                .checked_sub(1)
                .and_then(|j| pts.get(j))
                .map_or(bx, |p| base.curve_shown(*p)[0]);
            let right = pts.get(i + 1).map_or(wx, |p| base.curve_shown(*p)[0]);
            if right - left <= 2.0 * MIN_GAP {
                return a;
            }
            let x = to[0].clamp(left + MIN_GAP, right - MIN_GAP);
            let y = to[1].clamp(base.fade, 1.0);
            a.curve.set(i, base.curve_stored([x, y]));
        }
    }
    // A move that would put the points out of order, or onto an end, is no move:
    // the model refuses such a curve rather than draw it.
    let pts = a.curve.as_slice();
    let ordered =
        pts.iter().all(|p| p[0] > 0.0 && p[0] < 1.0) && pts.windows(2).all(|w| w[0][0] < w[1][0]);
    if ordered { a } else { *base }
}

/// The straight curve: every end at rest and no inner points.
fn straightened(a: &ImageAdjust) -> ImageAdjust {
    ImageAdjust {
        black_point: 0.0,
        white_point: 0.0,
        fade: 0.0,
        curve: mastik_core::CurvePoints::EMPTY,
        ..*a
    }
}

impl MastikApp {
    /// The Curves tab's body: the graph, the held handle's In and Out, and
    /// *Reset curve*. `body_h` is the height above the reset row, the same as the
    /// Adjustments tab's scrolled body, so the card keeps one height.
    pub(super) fn image_curves_tab(
        &mut self,
        ui: &mut egui::Ui,
        id: NodeId,
        slot: PaintSlot,
        brush: &Brush,
        img: &ImageRef,
        body_h: f32,
    ) {
        let ctx = ui.ctx().clone();
        let pick_id = egui::Id::new(("image-curve-pick", id));
        let drag_id = egui::Id::new(("image-curve-drag", id));
        let a = img.adjust;
        let mut drag = ctx.data(|d| d.get_temp::<Option<Drag>>(drag_id)).flatten();
        let mut pick = ctx
            .data(|d| d.get_temp::<Option<Handle>>(pick_id))
            .flatten()
            .filter(|h| position(&a, *h).is_some());
        let inner = ui.available_width();
        let gap = ui.spacing().item_spacing.y;
        let graph_h = (body_h - gap - ui::CONTROL_H).min(inner).max(GRAPH_MIN_H);
        let (rect, resp) =
            ui.allocate_exact_size(egui::vec2(inner, graph_h), egui::Sense::click_and_drag());
        let plot = rect.shrink(GRAPH_PAD);

        // A press that moves holds what it began on, or adds a point there.
        if resp.drag_started() {
            let origin = ctx
                .input(|i| i.pointer.press_origin())
                .or(resp.interact_pointer_pos());
            drag = origin.and_then(|o| match hit(&a, plot, o) {
                Some(handle) => Some(Drag { handle, base: a }),
                None => added(&a, from_screen(plot, o)[0]).map(|(base, i)| Drag {
                    handle: Handle::Inner(i),
                    base,
                }),
            });
            pick = drag.map(|d| d.handle);
        }
        let mut live = a;
        if let Some(d) = drag
            && (resp.dragged() || resp.drag_stopped())
        {
            if let Some(pos) = ctx.input(|i| i.pointer.latest_pos()) {
                let off = !rect.expand(OFF_GRAPH).contains(pos);
                live = moved(&d.base, d.handle, from_screen(plot, pos), off);
                if off && matches!(d.handle, Handle::Inner(_)) {
                    pick = None;
                }
            }
            self.valve_slot(
                &resp,
                id,
                slot,
                paint::map_image(brush, |r| r.adjust = live),
            );
            if resp.drag_stopped() {
                drag = None;
            }
        }
        // A double-click on a point removes it; a click takes a handle or adds a
        // point where the curve is.
        let pressed_at = resp.interact_pointer_pos();
        if ui::double_clicked(ui, &resp) {
            if let Some(Handle::Inner(i)) = pressed_at.and_then(|p| hit(&a, plot, p)) {
                let mut next = a;
                next.curve.remove(i);
                self.write_slot(id, slot, paint::map_image(brush, |r| r.adjust = next));
                pick = None;
            }
        } else if resp.clicked()
            && let Some(p) = pressed_at
        {
            match hit(&a, plot, p) {
                Some(h) => pick = Some(h),
                None => {
                    if let Some((next, i)) = added(&a, from_screen(plot, p)[0]) {
                        self.write_slot(id, slot, paint::map_image(brush, |r| r.adjust = next));
                        pick = Some(Handle::Inner(i));
                    }
                }
            }
        }
        if drag.is_some() {
            ctx.set_cursor_icon(egui::CursorIcon::Grabbing);
        } else if resp
            .hover_pos()
            .is_some_and(|p| hit(&live, plot, p).is_some())
        {
            ctx.set_cursor_icon(egui::CursorIcon::Grab);
        }
        paint_graph(
            ui,
            rect,
            plot,
            &live,
            pick,
            self.image_histogram(&ctx, &img.id).as_deref(),
        );
        resp.on_hover_text(GRAPH_TIP);

        // The held handle's In and Out, so arrows step it and a typed value lands
        // exactly. Dimmed with the reason when nothing is held.
        ui.horizontal(|ui| {
            ui.spacing_mut().item_spacing.x = ui::CARD_COL_GAP;
            let half = (inner - ui::CARD_COL_GAP) / 2.0;
            let at = pick.and_then(|h| position(&live, h));
            for (axis, word) in [(0, "In"), (1, "Out")] {
                let reason = match (pick, axis) {
                    (None, _) => Some("Pick a point on the curve to set it"),
                    (Some(Handle::White), 1) => Some("The white end always maps to white"),
                    _ => None,
                };
                let blank = at.is_none();
                let mut v = at.map_or(0.0, |p| f64::from(p[axis]) * 100.0);
                let field = ui::disable_unless(ui, reason.is_none(), |ui| {
                    ui::value_field(
                        ui,
                        egui::vec2(half, ui::CONTROL_H),
                        Prefix::Text(word),
                        &mut v,
                        Scrub::whole(0.25).range(0.0..=100.0),
                        |d| {
                            d.custom_formatter(move |n, r| match blank {
                                true => "–".into(),
                                false => ui::number(0)(n, r),
                            })
                        },
                    )
                })
                .inner;
                match (reason, pick, at) {
                    (None, Some(h), Some(mut to)) => {
                        to[axis] = (v / 100.0) as f32;
                        let next = moved(&a, h, to, false);
                        self.valve_slot(
                            &field,
                            id,
                            slot,
                            paint::map_image(brush, |r| r.adjust = next),
                        );
                    }
                    (Some(why), ..) => {
                        // A disabled widget reports no hover; a claimant over it does.
                        ui.interact(field.rect, field.id.with("why"), egui::Sense::empty())
                            .on_hover_text(why);
                    }
                    _ => {}
                }
            }
        });
        let used = graph_h + gap + ui::CONTROL_H;
        if body_h > used {
            ui.add_space(body_h - used);
        }
        ui.scope(|ui| {
            ui.spacing_mut().item_spacing.y = 2.0;
            let curved = straightened(&a) != a;
            if menu_action(
                ui,
                curved,
                icon::ARROW_COUNTER_CLOCKWISE,
                "Reset curve",
                &|on| match on {
                    true => "Put the curve back to a straight line".into(),
                    false => "The curve is already a straight line".into(),
                },
            ) {
                self.write_slot(
                    id,
                    slot,
                    paint::map_image(brush, |r| r.adjust = straightened(&a)),
                );
                pick = None;
            }
        });
        ctx.data_mut(|d| {
            d.insert_temp(drag_id, drag);
            d.insert_temp(pick_id, pick);
        });
    }

    /// The luminance histogram of picture `id`, counted once per picture and
    /// kept in the context, or [`None`] while it has no pixels.
    fn image_histogram(
        &self,
        ctx: &egui::Context,
        id: &ImageId,
    ) -> Option<Arc<[u32; HISTOGRAM_BINS]>> {
        let key = egui::Id::new(("image-histogram", &id.0));
        if let Some(h) = ctx.data(|d| d.get_temp::<Arc<[u32; HISTOGRAM_BINS]>>(key)) {
            return Some(h);
        }
        let data = self.canvas.images().get(id)?;
        let h = Arc::new(mastik_render::luminance_histogram(data));
        ctx.data_mut(|d| d.insert_temp(key, h.clone()));
        Some(h)
    }
}

/// The graph: its ground, the histogram, quarter lines, the straight line dashed
/// behind the curve, the curve, the ends' ticks and every handle, the held one
/// in the accent.
fn paint_graph(
    ui: &egui::Ui,
    rect: egui::Rect,
    plot: egui::Rect,
    a: &ImageAdjust,
    pick: Option<Handle>,
    histogram: Option<&[u32; HISTOGRAM_BINS]>,
) {
    let p = ui.painter_at(rect);
    p.rect_filled(rect, egui::CornerRadius::same(ui::BUTTON_R), color::field());
    if let Some(bins) = histogram {
        // Square-root heights, so a picture's few darkest pixels still show
        // beside a sky that fills half the frame.
        let top = bins.iter().copied().max().unwrap_or(0).max(1) as f32;
        let ink = theme::text::faint().gamma_multiply(0.5);
        let w = plot.width() / HISTOGRAM_BINS as f32;
        for (i, n) in bins.iter().enumerate() {
            let h = plot.height() * (*n as f32 / top).sqrt();
            if h > 0.0 {
                let x = plot.left() + i as f32 * w;
                p.rect_filled(
                    egui::Rect::from_min_max(
                        egui::pos2(x, plot.bottom() - h),
                        egui::pos2(x + w, plot.bottom()),
                    ),
                    0.0,
                    ink,
                );
            }
        }
    }
    let rule = egui::Stroke::new(1.0, color::divider());
    for q in [0.25, 0.5, 0.75] {
        p.line_segment([to_screen(plot, [q, 0.0]), to_screen(plot, [q, 1.0])], rule);
        p.line_segment([to_screen(plot, [0.0, q]), to_screen(plot, [1.0, q])], rule);
    }
    p.extend(egui::Shape::dashed_line(
        &[to_screen(plot, [0.0, 0.0]), to_screen(plot, [1.0, 1.0])],
        egui::Stroke::new(1.0, theme::text::faint()),
        3.0,
        3.0,
    ));
    let steps = (plot.width() / 2.0).max(2.0) as usize;
    let line: Vec<egui::Pos2> = (0..=steps)
        .map(|i| {
            let x = i as f32 / steps as f32;
            to_screen(plot, [x, a.curve_at(x)])
        })
        .collect();
    p.add(egui::Shape::line(
        line,
        egui::Stroke::new(1.5, theme::text::strong()),
    ));
    let (black, white) = a.curve_ends();
    for x in [black[0], white[0]] {
        let at = to_screen(plot, [x, 0.0]).x;
        p.line_segment(
            [
                egui::pos2(at, rect.bottom()),
                egui::pos2(at, rect.bottom() - TICK_H),
            ],
            egui::Stroke::new(1.0, theme::text::muted()),
        );
    }
    let edge = egui::Stroke::new(1.5, theme::text::strong());
    for (h, at) in handles(a) {
        let c = to_screen(plot, at);
        let fill = if pick == Some(h) {
            color::accent_400()
        } else {
            color::card()
        };
        match h {
            Handle::Inner(_) => {
                p.circle(c, POINT_R, fill, edge);
            }
            Handle::Black | Handle::White => {
                let sq = egui::Rect::from_center_size(c, egui::vec2(END_SIDE, END_SIDE));
                p.rect(
                    sq,
                    egui::CornerRadius::same(1),
                    fill,
                    edge,
                    egui::StrokeKind::Middle,
                );
            }
        }
    }
}

#[cfg(test)]
mod card_tests {
    //! The image card's three tabs, driven through `inspector_ui`.
    use super::*;
    use crate::panels::ImageCard;
    use mastik_core::kurbo::Size;
    use mastik_core::{NodeKind, Operation, Transaction};

    fn frame(
        app: &mut MastikApp,
        ctx: &egui::Context,
        events: Vec<egui::Event>,
    ) -> egui::FullOutput {
        let input = egui::RawInput {
            screen_rect: Some(egui::Rect::from_min_size(
                egui::Pos2::ZERO,
                egui::vec2(1200.0, 1600.0),
            )),
            events,
            ..Default::default()
        };
        ctx.run_ui(input, |ui| {
            ui.set_max_width(280.0);
            app.inspector_ui(ui)
        })
    }

    /// A rect filled with a picture, selected and in image editing.
    fn editing(ctx: &egui::Context) -> (MastikApp, NodeId) {
        crate::theme::install(ctx);
        let _ = ctx.run_ui(Default::default(), |_| {});
        let mut app = MastikApp::headless(ctx);
        let mut ids = mastik_core::IdSource::new(0xC2);
        let root = ids.mint();
        let rect = ids.mint();
        let mut doc = mastik_core::Document::new(root);
        doc.apply(&Transaction(vec![
            Operation::CreateNode {
                id: rect,
                parent: root,
                index: 0,
                kind: NodeKind::Rect {
                    size: Size::new(100.0, 100.0),
                    corner_radii: Default::default(),
                    corner_smoothing: 0.0,
                },
                transform: None,
                name: None,
            },
            Operation::SetFills {
                id: rect,
                fills: mastik_core::keyed_by_position([mastik_core::Fill {
                    brush: mastik_core::image_brush(ImageId("a-photo".into())).into(),
                    visible: true,
                }]),
            },
        ]))
        .unwrap();
        // A picture with pixels, so Settings shows its whole asset section and
        // Curves its histogram.
        let photo = ImageId("a-photo".into());
        doc.apply(&Transaction(vec![Operation::AddImage {
            id: photo.clone(),
            entry: mastik_core::ImageEntry {
                source: mastik_core::ImageSource::Embedded(vec![0u8; 16].into()),
                format: mastik_core::ImageFormat::Png,
                width: 4,
                height: 3,
            },
        }]))
        .unwrap();
        let ramp: Vec<u8> = (0..12u8)
            .flat_map(|i| [i * 20, i * 20, i * 20, 255])
            .collect();
        app.canvas.images_mut().adopt(
            &photo,
            mastik_core::peniko::ImageData {
                data: mastik_core::peniko::Blob::new(Arc::new(ramp)),
                format: mastik_core::peniko::ImageFormat::Rgba8,
                alpha_type: mastik_core::peniko::ImageAlphaType::Alpha,
                width: 4,
                height: 3,
            },
        );
        app.session.adopt_document(doc, None);
        app.session.selection.set_one(rect);
        app.tool = crate::tools::Tool::ImageEdit;
        assert_eq!(app.edited_image(), Some((rect, 0)), "fixture: editing");
        (app, rect)
    }

    /// The card's height after a few frames on `tab`.
    fn card_h(app: &mut MastikApp, ctx: &egui::Context, rect: NodeId, tab: ImageCard) -> f32 {
        app.image_tab = tab;
        for _ in 0..4 {
            frame(app, ctx, Vec::new());
        }
        ctx.memory(|m| m.area_rect(egui::Id::new(("image-edit", rect))))
            .expect("the card is open")
            .height()
    }

    /// Every tab is the Settings tab's height, folds shut or open, so switching
    /// tabs never resizes the card and the open folds scroll instead.
    ///
    /// flip: the Adjustments tab's scroll area without `min_scrolled_height` and
    /// shrinking in height (either alone holds it) → fails at "every fold shut".
    #[test]
    fn every_tab_keeps_the_settings_tabs_height() {
        let ctx = egui::Context::default();
        let (mut app, rect) = editing(&ctx);
        let settings = card_h(&mut app, &ctx, rect, ImageCard::Settings);
        assert!(
            ctx.data(|d| d.get_temp::<f32>(egui::Id::new("image-card-body-h")))
                .is_some(),
            "the Settings tab's body is measured where it is drawn, not guessed"
        );
        let body = ctx
            .data(|d| d.get_temp::<f32>(egui::Id::new("image-card-body-h")))
            .unwrap_or_default();
        assert!(
            (body - super::super::inspector::IMAGE_BODY_FALLBACK_H).abs() < 0.5,
            "the fallback a card opened on another tab keeps is the Settings body, {body}: \
             a popup can shrink but never grow back"
        );
        let first = card_h(&mut app, &ctx, rect, ImageCard::Adjustments);
        assert!(
            (first - settings).abs() < 0.5,
            "Adjustments, Colour and Looks folded: {first} against Settings' {settings}"
        );
        for section in mastik_core::AdjustSection::ALL {
            app.collapsed_panels
                .insert(crate::panels::image_fold_key(section));
        }
        let shut = card_h(&mut app, &ctx, rect, ImageCard::Adjustments);
        assert!(
            (shut - settings).abs() < 0.5,
            "Adjustments, every fold shut: {shut} against Settings' {settings}"
        );
        for key in crate::panels::IMAGE_FOLDED {
            app.collapsed_panels.remove(key);
        }
        let open = card_h(&mut app, &ctx, rect, ImageCard::Adjustments);
        assert!(
            (open - settings).abs() < 0.5,
            "Adjustments, every fold open: {open} against Settings' {settings}"
        );
        let curves = card_h(&mut app, &ctx, rect, ImageCard::Curves);
        assert!(
            (curves - settings).abs() < 0.5,
            "Curves: {curves} against Settings' {settings}"
        );
    }

    /// A click on the graph's empty middle adds a point on the curve, written to
    /// the picture's fill as one step.
    ///
    /// flip: `added` answering `None` → fails at "one point added".
    #[test]
    fn a_click_on_the_graph_adds_a_point() {
        let ctx = egui::Context::default();
        let (mut app, rect) = editing(&ctx);
        card_h(&mut app, &ctx, rect, ImageCard::Curves);
        let out = frame(&mut app, &ctx, Vec::new());
        // Found by shape, not by colour: the card fades in, scaling its fills.
        let inner = ui::menu_inner_w(crate::app::POPOVER_W, ui::POPOVER_PAD);
        let graph = out
            .shapes
            .iter()
            .find_map(|c| match &c.shape {
                egui::Shape::Rect(r)
                    if (r.rect.width() - inner).abs() < 0.5 && r.rect.height() >= GRAPH_MIN_H =>
                {
                    Some(r.rect)
                }
                _ => None,
            })
            .expect("the graph is drawn");
        let at = graph.center();
        let button = |pressed| egui::Event::PointerButton {
            pos: at,
            button: egui::PointerButton::Primary,
            pressed,
            modifiers: egui::Modifiers::NONE,
        };
        frame(&mut app, &ctx, vec![egui::Event::PointerMoved(at)]);
        frame(&mut app, &ctx, vec![button(true)]);
        frame(&mut app, &ctx, vec![button(false)]);
        frame(&mut app, &ctx, Vec::new());
        let fill = &app.session.doc.get(rect).unwrap().paint().fills;
        let adjust = fill
            .iter()
            .find_map(|f| paint::image_of(f.brush.drawn()).map(|r| r.adjust))
            .expect("still a picture");
        assert_eq!(adjust.curve.len(), 1, "one point added");
        let [x, _] = adjust.curve_shown(adjust.curve.as_slice()[0]);
        assert!((x - 0.5).abs() < 0.02, "where the click was: {x}");
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn plot() -> egui::Rect {
        egui::Rect::from_min_size(egui::pos2(10.0, 20.0), egui::vec2(200.0, 100.0))
    }

    /// Dragging the black end writes black point and fade, and the white end
    /// writes white point, the three numbers the Adjustments rows show.
    /// flip: `moved` writing `a.white_point = x` for the white end → fails at
    /// "white point 20".
    #[test]
    fn dragging_an_end_writes_its_rows() {
        let a = moved(&ImageAdjust::NEUTRAL, Handle::Black, [0.14, 0.3], false);
        assert!((a.black_point - 0.14).abs() < 1e-6 && (a.fade - 0.3).abs() < 1e-6);
        let a = moved(&ImageAdjust::NEUTRAL, Handle::White, [0.8, 0.5], false);
        assert!(
            (a.white_point - 0.2).abs() < 1e-6,
            "white point 20, got {}",
            a.white_point
        );
        assert_eq!(a.fade, 0.0, "the white end has no output to move");
    }

    /// An end is held short of the other end, so a drag never crosses them.
    #[test]
    fn an_end_stops_short_of_the_other() {
        let a = ImageAdjust {
            white_point: 0.5,
            ..ImageAdjust::NEUTRAL
        };
        let moved = moved(&a, Handle::Black, [0.9, 0.0], false);
        assert!(
            moved.black_point <= 0.5 - MIN_GAP + 1e-6,
            "the black end passed the white end: {}",
            moved.black_point
        );
    }

    /// A click on the curve adds a point where the curve already is, so the
    /// picture does not change until the point is moved; a full curve adds none.
    #[test]
    fn a_click_adds_a_point_on_the_curve() {
        let a = ImageAdjust {
            fade: 0.2,
            ..ImageAdjust::NEUTRAL
        };
        let (b, i) = added(&a, 0.5).expect("room on the curve");
        assert_eq!(i, 0);
        let [x, y] = b.curve_shown(b.curve.as_slice()[0]);
        assert!((x - 0.5).abs() < 1e-6 && (y - a.curve_at(0.5)).abs() < 1e-6);
        for t in [0.1, 0.3, 0.7, 0.9] {
            assert!(
                (b.curve_at(t) - a.curve_at(t)).abs() < 1e-5,
                "adding a point bent the curve at {t}"
            );
        }
        let mut full = a;
        for k in 0..CURVE_POINTS {
            full.curve.insert([0.1 + 0.1 * k as f32, 0.5]);
        }
        assert!(added(&full, 0.95).is_none(), "the curve holds eight");
        assert!(added(&a, 0.0).is_none(), "nothing is added on an end");
    }

    /// An inner point moves between its neighbours, and dragged off the graph it
    /// goes. flip: `moved` ignoring `off` → fails at "dragged off, it goes".
    #[test]
    fn an_inner_point_stays_between_its_neighbours() {
        let mut a = ImageAdjust::NEUTRAL;
        a.curve.insert([0.3, 0.3]);
        a.curve.insert([0.6, 0.6]);
        let b = moved(&a, Handle::Inner(0), [0.9, 0.5], false);
        let x = b.curve_shown(b.curve.as_slice()[0])[0];
        assert!(
            x <= 0.6 - MIN_GAP + 1e-6,
            "the point passed its neighbour: {x}"
        );
        assert!(b.pipeline().is_some(), "and the curve still draws");
        let gone = moved(&a, Handle::Inner(0), [0.3, 2.0], true);
        assert_eq!(gone.curve.len(), 1, "dragged off, it goes");
    }

    /// A press lands on the nearest handle within reach, and screen and graph
    /// coordinates round-trip.
    #[test]
    fn a_press_takes_the_nearest_handle() {
        let a = ImageAdjust {
            black_point: 0.1,
            ..ImageAdjust::NEUTRAL
        };
        let at = to_screen(plot(), [0.1, 0.0]);
        assert_eq!(
            hit(&a, plot(), at + egui::vec2(2.0, -2.0)),
            Some(Handle::Black)
        );
        assert_eq!(
            hit(&a, plot(), to_screen(plot(), [1.0, 1.0])),
            Some(Handle::White)
        );
        assert_eq!(hit(&a, plot(), to_screen(plot(), [0.5, 0.5])), None);
        let back = from_screen(plot(), to_screen(plot(), [0.25, 0.75]));
        assert!((back[0] - 0.25).abs() < 1e-5 && (back[1] - 0.75).abs() < 1e-5);
    }
}
