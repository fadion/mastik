//! Curvature combs: while a path's points are edited and the switch is on
//! (`Alt+C`, the path bar's comb button), teeth stand on the path's normals, each as
//! long as the curvature there, their tips joined by a line. Canvas chrome only:
//! nothing here reaches the document, so nothing here is an undo step.
//!
//! Teeth point away from the centre of curvature, so they swap sides where the
//! curve changes direction. At every point that is neither a corner nor a broken
//! pair, the two sides are judged: a tangent a hair off line, a curvature that
//! jumps or flips. A flagged join colours the teeth near it and rings the point,
//! and the Combs card counts them and steps through them with *Next*.

use crate::app::MastikApp;
use crate::input::ViewSwitch;
use crate::theme::{self, color, icon};
use crate::ui::{self, FieldButton, Prefix, Scrub};
use mastik_core::edit::{PenSubpath, PointRef, segment_cubic};
use mastik_core::kurbo::{CubicBez, ParamCurve, ParamCurveDeriv, Point, Rect, Vec2};

/// The card's width: the Offset card's, in whose place it hangs.
const CARD_W: f32 = 176.0;
/// Fitted mode's length for the longest tooth that is not a kink, in screen pt.
const FIT_PT: f64 = 60.0;
/// The longest a tooth is drawn, in screen pt, so a kink's tooth stays on screen.
const CLIP_PT: f64 = 2.0 * FIT_PT;
/// The ring round a flagged point: clear of the point's marker (`POINT_PX` 9)
/// and of a snap target's ring, so it reads as a third thing.
const RING_PT: f32 = 9.0;
const RING_STROKE_PT: f32 = 1.5;
/// How far into each segment beside a flagged join its ink reaches, as a
/// fraction of the segment.
const REACH: f64 = 0.25;

/// The teeth per segment the card's field takes, and a fresh install's.
pub(crate) const TEETH_MIN: u8 = 8;
pub(crate) const TEETH_MAX: u8 = 64;
pub(crate) const TEETH_DEFAULT: u8 = 24;

/// A tangent turning by more than this across a smooth point is *Not quite smooth*.
const SMOOTH_DEG: f64 = 0.5;
/// A tangent turning by more than this is a pair broken on purpose (`Alt` while
/// shaping), a corner in all but its handles, and is not judged.
const BROKEN_DEG: f64 = 10.0;
/// A curvature step, over the larger side, past which a join is amber, and red.
const WARN_JUMP: f64 = 0.15;
const DANGER_JUMP: f64 = 0.5;
/// A side whose curvature times the path's size is under this counts as straight:
/// two near-straight sides meet fine whatever their ratio, and a sign change
/// between them is no flip.
const STRAIGHT: f64 = 0.05;
/// Curvature times the segment's control polygon length past which a tooth
/// inside it is a kink, where the curve all but folds. Measured against the
/// segment, not the path: a tight round corner on a large path is drawn on
/// purpose, and a quarter circle's highest is about 1.6.
const KINK: f64 = 25.0;

/// What is wrong at a smooth point.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) enum Fault {
    /// The curvature steps by this fraction of the larger side, signed by which
    /// side is larger: positive where it grows along the path.
    Jump(f64),
    /// The curve bends one way into the point and the other way out of it.
    Flips,
    /// The handles are off line by more than half a degree.
    NotSmooth,
}

impl Fault {
    /// Whether the fault is red rather than amber.
    pub(crate) fn danger(self) -> bool {
        match self {
            Self::Jump(j) => j.abs() > DANGER_JUMP,
            Self::Flips | Self::NotSmooth => true,
        }
    }

    /// The ink the fault's teeth, ring and pill wear.
    fn ink(self) -> egui::Color32 {
        match self.danger() {
            true => color::danger(),
            false => color::warn(),
        }
    }

    /// The pill's words.
    pub(crate) fn label(self) -> String {
        match self {
            Self::Jump(j) => format!("Curvature {:+.0}%", j * 100.0),
            Self::Flips => "Curvature flips".into(),
            Self::NotSmooth => "Not quite smooth".into(),
        }
    }
}

/// One tooth: where it stands and the curvature vector there, pointing away from
/// the centre of curvature with length |κ| (per world unit).
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct Tooth {
    pub(crate) at: Point,
    pub(crate) k: Vec2,
    /// A tooth inside a segment where the curve all but folds ([`KINK`]).
    pub(crate) kink: bool,
}

/// One segment's teeth, at `t = i / n` for `i` in `0..=n`; `None` where the
/// curvature is undefined (an end whose handle is retracted).
pub(crate) struct SegmentComb {
    /// The anchors the segment leaves and reaches.
    pub(crate) from: PointRef,
    pub(crate) to: PointRef,
    pub(crate) bounds: Rect,
    pub(crate) teeth: Vec<Option<Tooth>>,
}

/// A flagged smooth point.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct Join {
    pub(crate) at: PointRef,
    pub(crate) point: Point,
    pub(crate) fault: Fault,
}

/// The comb of a whole path.
pub(crate) struct Comb {
    pub(crate) segments: Vec<SegmentComb>,
    /// The flagged joins, in path order.
    pub(crate) joins: Vec<Join>,
    /// The largest |κ| among the teeth that are not kinks, which Fitted scales
    /// to [`FIT_PT`]; zero on a path of straight lines.
    pub(crate) peak: f64,
}

impl Comb {
    /// The fault at `at`, if it is flagged.
    fn fault_at(&self, at: PointRef) -> Option<Fault> {
        self.joins.iter().find(|j| j.at == at).map(|j| j.fault)
    }

    /// The scale Fitted draws at, in world units squared (a tooth's world length
    /// is |κ| times it), for `ppw` screen pt per world unit.
    pub(crate) fn fitted(&self, ppw: f64) -> Option<f64> {
        (self.peak > 0.0 && ppw > 0.0).then(|| FIT_PT / (ppw * self.peak))
    }
}

/// The signed curvature of `c` at `t`, `None` where its speed vanishes.
fn curvature(c: &CubicBez, t: f64, tiny: f64) -> Option<(f64, Vec2)> {
    let d1 = c.deriv().eval(t).to_vec2();
    let d2 = c.deriv().deriv().eval(t).to_vec2();
    let speed = d1.hypot();
    (speed > tiny).then(|| (d1.cross(d2) / (speed * speed * speed), d1 / speed))
}

/// The direction `c` leaves its start in, or arrives at its end in, read from
/// the nearest control point that is not on the end: a retracted handle still
/// has a tangent.
fn end_tangent(c: &CubicBez, start: bool, tiny: f64) -> Option<Vec2> {
    let v = if start {
        [c.p1, c.p2, c.p3].map(|q| q - c.p0)
    } else {
        [c.p2, c.p1, c.p0].map(|q| c.p3 - q)
    };
    v.into_iter()
        .find(|v| v.hypot() > tiny)
        .map(Vec2::normalize)
}

/// Judge the join at an anchor whose incoming segment is `a` and outgoing `b`,
/// on a path `size` across.
fn judge(a: &CubicBez, b: &CubicBez, size: f64, tiny: f64) -> Option<Fault> {
    let (tin, tout) = (end_tangent(a, false, tiny)?, end_tangent(b, true, tiny)?);
    let turn = tin.cross(tout).atan2(tin.dot(tout)).abs().to_degrees();
    if turn > BROKEN_DEG {
        return None;
    }
    if turn > SMOOTH_DEG {
        return Some(Fault::NotSmooth);
    }
    let (kin, _) = curvature(a, 1.0, tiny)?;
    let (kout, _) = curvature(b, 0.0, tiny)?;
    let (nin, nout) = (kin * size, kout * size);
    if nin.abs() < STRAIGHT && nout.abs() < STRAIGHT {
        return None;
    }
    if nin * nout < 0.0 && nin.abs().min(nout.abs()) >= STRAIGHT {
        return Some(Fault::Flips);
    }
    let jump = (kout.abs() - kin.abs()) / kin.abs().max(kout.abs());
    (jump.abs() > WARN_JUMP).then_some(Fault::Jump(jump))
}

/// The comb of `subpaths` at `n` teeth per segment.
pub(crate) fn comb(subpaths: &[PenSubpath], n: usize) -> Comb {
    let n = n.max(1);
    let size = subpaths
        .iter()
        .flat_map(|s| &s.anchors)
        .flat_map(|a| [a.at, a.leaving(), a.arriving()])
        .map(|p| Rect::from_points(p, p))
        .reduce(|a, b| a.union(b))
        .map_or(0.0, |r| r.size().to_vec2().hypot());
    let tiny = size * 1e-9;
    let mut segments = Vec::new();
    let mut joins = Vec::new();
    let mut peak: f64 = 0.0;
    for (s, sub) in subpaths.iter().enumerate() {
        let len = sub.anchors.len();
        let count = match sub.closed {
            true if len > 1 => len,
            _ => len.saturating_sub(1),
        };
        let cubics: Vec<CubicBez> = (0..count)
            .filter_map(|a| segment_cubic(subpaths, (s, a)))
            .collect();
        if cubics.len() != count {
            continue;
        }
        for (a, c) in cubics.iter().enumerate() {
            let reach = (c.p1 - c.p0).hypot() + (c.p2 - c.p1).hypot() + (c.p3 - c.p2).hypot();
            let teeth = (0..=n)
                .map(|i| {
                    let t = i as f64 / n as f64;
                    let (k, tangent) = curvature(c, t, tiny)?;
                    let kink = i > 0 && i < n && k.abs() * reach > KINK;
                    if !kink {
                        peak = peak.max(k.abs());
                    }
                    // The left normal is (-ty, tx), the centre of curvature at
                    // `at + normal / k`, and the tooth points the other way.
                    let normal = Vec2::new(-tangent.y, tangent.x);
                    Some(Tooth {
                        at: c.eval(t),
                        k: -k * normal,
                        kink,
                    })
                })
                .collect();
            segments.push(SegmentComb {
                from: (s, a),
                to: (s, (a + 1) % len),
                bounds: Rect::from_points(c.p0, c.p3).union_pt(c.p1).union_pt(c.p2),
                teeth,
            });
        }
        // Every anchor with a segment on both sides that is not a corner.
        for (a, an) in sub.anchors.iter().enumerate() {
            let incoming = match (a, sub.closed) {
                (0, true) => count.checked_sub(1),
                (0, false) => None,
                _ => Some(a - 1),
            };
            let (Some(i), Some(out)) = (incoming, cubics.get(a)) else {
                continue;
            };
            if an.is_corner() || i == a {
                continue;
            }
            if let Some(fault) = judge(&cubics[i], out, size, tiny) {
                joins.push(Join {
                    at: (s, a),
                    point: an.at,
                    fault,
                });
            }
        }
    }
    Comb {
        segments,
        joins,
        peak,
    }
}

/// The session's half of the combs: the card and Fixed's scale. The switch, the
/// mode and the teeth are preferences (`prefs::CombPrefs`).
#[derive(Clone, Copy, Debug)]
pub(crate) struct CombState {
    /// Whether the card shows beside the path bar while combs are on; its × hides
    /// it and leaves combs on.
    pub(crate) card: bool,
    /// Fixed's scale at 100%, in world units squared: the fitted scale when Fixed
    /// was chosen, so a second curve is drawn against the first's scale.
    pub(crate) base: Option<f64>,
    /// Fixed's scale, as a percentage of `base`.
    pub(crate) pct: f64,
    /// Whether the teeth changed under a held field and are not saved yet.
    unsaved: bool,
}

impl Default for CombState {
    fn default() -> Self {
        Self {
            card: true,
            base: None,
            pct: 100.0,
            unsaved: false,
        }
    }
}

impl MastikApp {
    /// Whether combs are on (they draw only while points are edited).
    pub(crate) fn combs_on(&self) -> bool {
        self.prefs.combs.on
    }

    /// Turn combs on or off, saved at once. Turning them on shows the card, which
    /// hangs where the Offset, Clean up and Distort cards do, so those close.
    pub(crate) fn set_combs(&mut self, on: bool) {
        self.prefs.combs.on = on;
        self.prefs.save();
        if on {
            self.combs.card = true;
            self.close_offset_card();
            self.close_cleanup();
            self.close_distort();
        }
    }

    /// Whether the Combs card is up: combs on, the card not closed, a path's
    /// points being edited, and neither an envelope open nor a pen run in hand,
    /// where the teeth are not drawn either.
    pub(crate) fn comb_card_shown(&self) -> bool {
        self.combs_on()
            && self.combs.card
            && self.warp.is_none()
            && self.pen.is_none()
            && self.edited_path().is_some()
    }

    /// The edited path's comb, in world space through the preview, at the card's
    /// teeth.
    pub(crate) fn edited_comb(&self) -> Option<Comb> {
        let id = self.edited_path()?;
        let (_, subpaths) = self.preview_subpaths(id)?;
        let n = self.prefs.combs.teeth.clamp(TEETH_MIN, TEETH_MAX);
        Some(comb(&subpaths, usize::from(n)))
    }

    /// Screen pt per world unit.
    fn comb_ppw(&self, ppp: f32) -> f64 {
        self.session.camera.zoom / f64::from(ppp)
    }

    /// The scale `comb` draws at, in world units squared: Fitted's, or Fixed's
    /// percentage of the scale it was fixed at.
    fn comb_scale(&self, comb: &Comb, ppw: f64) -> Option<f64> {
        let fitted = comb.fitted(ppw);
        match self.prefs.combs.fixed {
            true => Some(self.combs.base.or(fitted)? * self.combs.pct / 100.0),
            false => fitted,
        }
    }

    /// Draw the edited path's comb, under its points and handles.
    pub(crate) fn draw_combs(&self, painter: &egui::Painter, rect: egui::Rect, ppp: f32) {
        if !self.combs_on() || self.warp.is_some() || self.pen.is_some() {
            return;
        }
        let Some(id) = self.edited_path() else { return };
        let Some(comb) = self.edited_comb() else {
            return;
        };
        let ppw = self.comb_ppw(ppp);
        let Some(scale) = self.comb_scale(&comb, ppw) else {
            return;
        };
        let reach = rect.expand(CLIP_PT as f32);
        let dim = egui::Stroke::new(1.0, color::SELECT_DIM);
        let tip_line = egui::Stroke::new(1.0, color::SELECT);
        for seg in &comb.segments {
            let corners = [
                Point::new(seg.bounds.x0, seg.bounds.y0),
                Point::new(seg.bounds.x1, seg.bounds.y1),
            ]
            .map(|p| self.to_screen(p, rect, ppp));
            if !reach.intersects(egui::Rect::from_two_pos(corners[0], corners[1])) {
                continue;
            }
            let (from, to) = (comb.fault_at(seg.from), comb.fault_at(seg.to));
            let last = seg.teeth.len().saturating_sub(1).max(1) as f64;
            // The ink of tooth `i`: a kink's red, or a flagged join's near it.
            let ink = |i: usize, tooth: &Tooth| {
                let t = i as f64 / last;
                let near = match (t <= REACH, t >= 1.0 - REACH) {
                    (true, _) if from.is_some() => from,
                    (_, true) if to.is_some() => to,
                    _ => None,
                };
                match (tooth.kink, near) {
                    (true, _) => Some(color::danger()),
                    (false, Some(f)) => Some(f.ink()),
                    (false, None) => None,
                }
            };
            let mut prev: Option<(egui::Pos2, Option<egui::Color32>)> = None;
            for (i, tooth) in seg.teeth.iter().enumerate() {
                let Some(tooth) = tooth else {
                    prev = None;
                    continue;
                };
                let base = self.to_screen(tooth.at, rect, ppp);
                let mut v = tooth.k * scale * ppw;
                let len = v.hypot();
                if len > CLIP_PT {
                    v *= CLIP_PT / len;
                }
                let tip = base + egui::vec2(v.x as f32, v.y as f32);
                let ink = ink(i, tooth);
                painter.line_segment([base, tip], ink.map_or(dim, |c| egui::Stroke::new(1.0, c)));
                if let Some((p, was)) = prev {
                    let stroke = ink.or(was).map_or(tip_line, |c| egui::Stroke::new(1.0, c));
                    painter.line_segment([p, tip], stroke);
                }
                prev = Some((tip, ink));
            }
        }
        let picked = self.subject_points(id);
        for j in &comb.joins {
            let at = self.to_screen(j.point, rect, ppp);
            painter.circle_stroke(
                at,
                RING_PT,
                egui::Stroke::new(RING_STROKE_PT, j.fault.ink()),
            );
            if picked.contains(&j.at) {
                ui::canvas_pill(
                    painter,
                    at + egui::vec2(0.0, RING_PT + 4.0),
                    egui::Align2::CENTER_TOP,
                    &j.fault.label(),
                    j.fault.ink(),
                );
            }
        }
    }

    /// Pick the next flagged join after the last picked point, in path order,
    /// and bring it into view.
    pub(crate) fn next_rough_join(&mut self) {
        let Some(id) = self.edited_path() else { return };
        let Some(comb) = self.edited_comb() else {
            return;
        };
        let from = self.subject_points(id).last().copied();
        let next = comb
            .joins
            .iter()
            .find(|j| from.is_none_or(|f| j.at > f))
            .or(comb.joins.first());
        if let Some(j) = next {
            self.points.set_one(id, j.at);
            self.reveal(Rect::from_points(j.point, j.point));
        }
    }

    /// The Combs card: beside the bar, its top on the comb button's.
    pub(crate) fn comb_card_ui(
        &mut self,
        ctx: &egui::Context,
        bar: egui::Rect,
        button: egui::Rect,
    ) {
        let comb = self.edited_comb();
        let rough = comb.as_ref().map_or(0, |c| c.joins.len());
        let ppw = self.comb_ppw(ctx.pixels_per_point());
        let fitted = comb.as_ref().and_then(|c| c.fitted(ppw));
        let fixed = self.prefs.combs.fixed;
        let mut pct = if fixed { self.combs.pct } else { 100.0 };
        let mut teeth = f64::from(self.prefs.combs.teeth.clamp(TEETH_MIN, TEETH_MAX));
        let (mut close, mut mode, mut next, mut teeth_held) = (false, None, false, false);
        let at = egui::pos2(bar.right() + crate::path_bar::GAP, button.top());
        egui::Area::new(egui::Id::new("comb-card"))
            .order(egui::Order::Foreground)
            .fixed_pos(at)
            .show(ctx, |ui| {
                ui::hold_tab(ui);
                ui::menu_frame(ui::POPOVER_PAD).show(ui, |ui| {
                    let w = ui::menu_inner_w(CARD_W, ui::POPOVER_PAD);
                    ui.set_width(w);
                    ui.spacing_mut().item_spacing.y = ui::POPOVER_SECTION_GAP;
                    close = ui::popover_head(ui, "Combs");
                    mode = ui::segmented(
                        ui,
                        w,
                        ui::SEGMENT_CELL_H,
                        2,
                        usize::from(fixed),
                        |p, i, r, on| ui::segment_label(p, r, ["Fitted", "Fixed"][i], on),
                    );
                    let size = egui::vec2(w, ui::CONTROL_H);
                    ui::labelled(ui, "Scale", |ui| {
                        ui.add_enabled_ui(fixed, |ui| {
                            ui::value_field(
                                ui,
                                size,
                                Prefix::Text(""),
                                &mut pct,
                                Scrub::whole(0.5).range(1.0..=1000.0),
                                |d| d.custom_formatter(|v, _| format!("{v:.0}%")),
                            )
                        })
                        .inner
                        .on_hover_text(
                            "How long the teeth are, against the scale they were fixed at",
                        )
                        .on_disabled_hover_text("Fitted keeps the longest tooth on screen");
                    });
                    teeth_held = ui::labelled(ui, "Teeth", |ui| {
                        let resp = ui::value_field(
                            ui,
                            size,
                            Prefix::Text(""),
                            &mut teeth,
                            Scrub::whole(0.2).range(f64::from(TEETH_MIN)..=f64::from(TEETH_MAX)),
                            |d| d.custom_formatter(ui::number(0)),
                        )
                        .on_hover_text("Teeth on each segment");
                        resp.dragged() || resp.has_focus()
                    });
                    // One row in either state, so the card keeps its height: an
                    // area shrinks but never grows back.
                    let (glyph, words, ink) = match rough {
                        0 => (
                            icon::CHECK_CIRCLE,
                            "Every join is smooth".to_owned(),
                            theme::text::faint(),
                        ),
                        k => (
                            icon::WARNING,
                            ui::count_as(k, "rough join", "rough joins"),
                            color::warn(),
                        ),
                    };
                    ui::note_line(ui, glyph, ink, &words, None);
                    let state = match rough {
                        0 => FieldButton::Disabled,
                        _ => FieldButton::Off,
                    };
                    let b = ui::label_button(ui, "Next", size, state);
                    next = rough > 0 && b.clicked();
                    match rough {
                        0 => b.on_hover_text("Every join is smooth"),
                        _ => b.on_hover_text("Pick the next rough join"),
                    };
                });
            });
        if close {
            self.combs.card = false;
            return;
        }
        if let Some(i) = mode {
            let fixed = i == 1;
            if fixed {
                self.combs.base = fitted;
                self.combs.pct = 100.0;
            }
            self.prefs.combs.fixed = fixed;
            self.prefs.save();
        } else if fixed && pct != self.combs.pct {
            self.combs.pct = pct;
        }
        // Drawn live, saved once the field lets go: `save` rewrites the whole file.
        let teeth = teeth
            .round()
            .clamp(f64::from(TEETH_MIN), f64::from(TEETH_MAX)) as u8;
        self.combs.unsaved |= teeth != self.prefs.combs.teeth;
        self.prefs.combs.teeth = teeth;
        if !teeth_held && std::mem::take(&mut self.combs.unsaved) {
            self.prefs.save();
        }
        if next {
            self.next_rough_join();
        }
    }

    /// The comb switch, flipped from the path bar or `Alt+C`.
    pub(crate) fn toggle_combs(&mut self) {
        self.set_view_switch(ViewSwitch::Combs, !self.combs_on());
    }

    /// Fix Fixed's scale on the edited path's fitted one when a session starts
    /// in Fixed with nothing fixed yet, so zooming does not refit it.
    pub(crate) fn settle_comb_base(&mut self, ppp: f32) {
        if self.combs_on() && self.prefs.combs.fixed && self.combs.base.is_none() {
            let ppw = self.comb_ppw(ppp);
            self.combs.base = self.edited_comb().and_then(|c| c.fitted(ppw));
        }
    }
}

#[cfg(test)]
mod tests;
