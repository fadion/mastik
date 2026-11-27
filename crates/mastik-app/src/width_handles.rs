//! The width handles on the canvas.
//!
//! **The open stroke popover is the mode**, as the open picker is for a gradient:
//! while it is open on a profiled stroke of the one selected layer under a
//! selecting tool, each point of the stroke's width profile is a dot on the path
//! with a diamond on either edge, and the selection box steps aside
//! (`MastikApp::gradient_handles_up`).
//!
//! Dragging a diamond widens both sides of its point, Alt only its own; Shift
//! holds the point's whole width to whole numbers. Dragging the dot slides the
//! point along the path, between its neighbours. A click on the bare path adds a
//! point there at the width already drawn, and a right-click on a point removes
//! it. Every edit keeps the stroke's `width` the widest it draws: the profile is
//! rescaled under it, so the weight field reads the widest point. Each gesture is
//! one *Stroke* step, as every stroke-popover edit is.

use crate::app::MastikApp;
use crate::input::Mode;
use crate::preview::Drag;
use crate::theme::{color, icon};
use eframe::egui;
use mastik_core::kurbo::{Affine, BezPath, Point, Vec2};
use mastik_core::profile::{self, WidthPoint, WidthProfile};
use mastik_core::{NodeId, Stroke};

/// A diamond's half-diagonal and a dot's radius, in screen points.
const DIAMOND_R: f32 = 5.0;
const DOT_R: f32 = 4.0;
/// How near a press must land to a handle to grab it, and to the path to add a
/// point there, in screen points.
const PICK_PX: f32 = 8.0;
const PATH_PICK_PX: f32 = 5.0;

/// Which side of the path a diamond sits on, as the path runs.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Side {
    Left,
    Right,
}

/// What a press on the width handles grabbed: point `i`'s dot, or one of its
/// diamonds.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Grab {
    Dot(usize),
    Edge(usize, Side),
}

impl Grab {
    fn point(self) -> usize {
        match self {
            Grab::Dot(i) | Grab::Edge(i, _) => i,
        }
    }
}

/// The handles on screen this frame.
struct Marks {
    id: NodeId,
    index: usize,
    world: Affine,
    /// The path the stroke is laid along, in the layer's space.
    path: BezPath,
    /// The stroke as stored.
    stroke: Stroke,
    /// Half the width the stroke is drawn at, its profile's 1.
    half: f64,
    /// Each point's dot, left diamond and right diamond.
    points: Vec<(egui::Pos2, egui::Pos2, egui::Pos2)>,
}

impl Marks {
    fn grab_at(&self, at: egui::Pos2) -> Option<Grab> {
        let near = |p: egui::Pos2| p.distance(at);
        let mut best: Option<(Grab, f32)> = None;
        for (i, (dot, left, right)) in self.points.iter().enumerate() {
            for (g, p) in [
                (Grab::Edge(i, Side::Left), *left),
                (Grab::Edge(i, Side::Right), *right),
                (Grab::Dot(i), *dot),
            ] {
                let d = near(p);
                if d <= PICK_PX && best.is_none_or(|(_, b)| d < b) {
                    best = Some((g, d));
                }
            }
        }
        best.map(|(g, _)| g)
    }
}

/// Point `i`'s place and its left normal, in the layer's space.
fn station(path: &BezPath, t: f64) -> Option<(Point, Vec2)> {
    let (p, dir) = profile::point_at(path, t)?;
    Some((p, Vec2::new(-dir.y, dir.x)))
}

/// `points` rescaled so the widest is 1, and the factor the width takes for
/// the drawing to stay put; unchanged with a factor of 1 when `held` (a bound
/// width cannot move) or when nothing is wide at all.
fn normalised(mut points: Vec<WidthPoint>, held: bool) -> (Vec<WidthPoint>, f64) {
    let widest = points
        .iter()
        .map(|p| p.left.max(p.right))
        .fold(0.0, f64::max);
    if held || widest <= 1e-9 {
        for p in &mut points {
            p.left = p.left.min(1.0);
            p.right = p.right.min(1.0);
        }
        return (points, 1.0);
    }
    for p in &mut points {
        p.left /= widest;
        p.right /= widest;
    }
    (points, widest)
}

impl MastikApp {
    /// The profiled stroke whose popover is open, when its width handles are on
    /// the canvas: its layer, its index and the stroke as stored. `None` unless a
    /// selecting tool is up, that layer is the whole selection, and the stroke
    /// draws with its profile there.
    pub(crate) fn edited_profile(&self) -> Option<(NodeId, usize, Stroke)> {
        if self.mode != Mode::Normal || !self.selecting() {
            return None;
        }
        let index = self.stroke_menu?;
        let [id] = self.session.selection.ids() else {
            return None;
        };
        // A live brush draws in place of the stroke the handles would shape.
        if mastik_core::live_brush::hides_strokes(&self.session.doc, *id) {
            return None;
        }
        let node = self.session.display_node(*id)?;
        let stroke = node.paint().strokes.get(index)?;
        stroke.drawn_profile(node.kind())?;
        Some((*id, index, stroke.value.clone()))
    }

    fn width_marks(&self, rect: egui::Rect, ppp: f32) -> Option<Marks> {
        let (id, index, stroke) = self.edited_profile()?;
        let world = self.session.preview_world_transform(id)?;
        let node = self.session.display_node(id)?;
        let kind = node.kind();
        let shape = mastik_core::geometry::local_path(kind)
            .or_else(|| self.session.resolved.boolean_path(id).cloned())?;
        let drawn = self.session.doc.resolved_paint(id, node.paint());
        let width = drawn.strokes.get(index).map_or(stroke.width, |s| s.width);
        let path =
            mastik_core::geometry::stroked_path(kind, &shape, width, stroke.markers()).into_owned();
        let half = width / 2.0;
        let to_s = |p: Point| self.to_screen(world * p, rect, ppp);
        let profile = stroke.profile.as_ref()?;
        let points = profile
            .points
            .iter()
            .filter_map(|wp| {
                let (p, n) = station(&path, wp.t)?;
                Some((
                    to_s(p),
                    to_s(p + n * wp.left * half),
                    to_s(p - n * wp.right * half),
                ))
            })
            .collect();
        Some(Marks {
            id,
            index,
            world,
            path,
            stroke,
            half,
            points,
        })
    }

    /// Where along the edited stroke's path the point nearest `screen` sits, if
    /// it is within [`PATH_PICK_PX`] on screen.
    fn width_path_at(
        &self,
        m: &Marks,
        screen: egui::Pos2,
        rect: egui::Rect,
        ppp: f32,
    ) -> Option<f64> {
        if !mastik_core::build::affine_is_invertible(m.world) {
            return None;
        }
        let local = m.world.inverse() * self.to_world(screen, rect, ppp);
        let (t, _) = profile::fraction_at(&m.path, local)?;
        let (p, _) = profile::point_at(&m.path, t)?;
        (self.to_screen(m.world * p, rect, ppp).distance(screen) <= PATH_PICK_PX).then_some(t)
    }

    /// Whether the width handles take a press at `screen`: a handle, or the
    /// path a click adds a point on. The guide ladder and the stroke popover's
    /// click-away both ask.
    pub(crate) fn width_claims(&self, screen: egui::Pos2, rect: egui::Rect, ppp: f32) -> bool {
        self.width_marks(rect, ppp).is_some_and(|m| {
            m.grab_at(screen).is_some() || self.width_path_at(&m, screen, rect, ppp).is_some()
        })
    }

    /// The cursor over the width handles: a grab over one, the arrow over the
    /// bare path, where the ghost dot is the affordance for adding a point.
    pub(crate) fn width_cursor(
        &self,
        screen: Option<egui::Pos2>,
        rect: egui::Rect,
        ppp: f32,
    ) -> Option<crate::cursor::Pointer> {
        if matches!(self.drag, Drag::WidthPoint { .. }) {
            return Some(crate::canvas::handle_cursor(true));
        }
        let m = self.width_marks(rect, ppp)?;
        let at = screen?;
        if self.selection_locked() {
            return None;
        }
        if m.grab_at(at).is_some() {
            return Some(crate::canvas::handle_cursor(false));
        }
        self.width_path_at(&m, at, rect, ppp)
            .map(|_| crate::canvas::arrow(None))
    }

    /// Start a drag on the width handle under `grab`. False when the press is
    /// not the handles'.
    pub(crate) fn begin_width_drag(
        &mut self,
        grab: egui::Pos2,
        rect: egui::Rect,
        ppp: f32,
    ) -> bool {
        if self.selection_locked() {
            return false;
        }
        let Some(m) = self.width_marks(rect, ppp) else {
            return false;
        };
        let Some(g) = m.grab_at(grab) else {
            return false;
        };
        self.drag = Drag::WidthPoint {
            id: m.id,
            index: m.index,
            grab: g,
            from: Box::new(m.stroke),
        };
        true
    }

    /// A click on the bare path adds a point there at the width drawn, so the
    /// stroke does not change. True when the click was the handles'.
    pub(crate) fn width_click(&mut self, screen: egui::Pos2, rect: egui::Rect, ppp: f32) -> bool {
        if self.selection_locked() {
            return false;
        }
        let Some(m) = self.width_marks(rect, ppp) else {
            return false;
        };
        if m.grab_at(screen).is_some() {
            return true;
        }
        let Some(t) = self.width_path_at(&m, screen, rect, ppp) else {
            return false;
        };
        let Some(profile) = m.stroke.profile.as_ref() else {
            return true;
        };
        let (left, right) = profile.at(t);
        let mut points = profile.points.clone();
        points.push(WidthPoint { t, left, right });
        points.sort_by(|a, b| a.t.total_cmp(&b.t));
        let next = Stroke {
            profile: Some(WidthProfile { points }),
            ..m.stroke.clone()
        };
        self.commit_width(m.id, m.index, next);
        true
    }

    /// A right-click on a point removes it while another is left. True when it
    /// landed on the handles, which claim it from the context menu either way.
    pub(crate) fn width_secondary(
        &mut self,
        screen: egui::Pos2,
        rect: egui::Rect,
        ppp: f32,
    ) -> bool {
        if self.selection_locked() {
            return false;
        }
        let Some(m) = self.width_marks(rect, ppp) else {
            return false;
        };
        let Some(g) = m.grab_at(screen) else {
            return false;
        };
        let Some(profile) = m.stroke.profile.as_ref() else {
            return true;
        };
        if profile.points.len() > 1 {
            let mut points = profile.points.clone();
            points.remove(g.point());
            let (points, k) = normalised(points, m.stroke.width_var.is_some());
            let next = Stroke {
                width: m.stroke.width * k,
                profile: Some(WidthProfile { points }),
                ..m.stroke.clone()
            };
            self.commit_width(m.id, m.index, next);
        }
        true
    }

    /// The stroke a width drag makes with the pointer at `world`, re-derived
    /// from `from`, the stroke at the press.
    fn width_drag_stroke(
        &self,
        grab: Grab,
        from: &Stroke,
        world: Point,
        shift: bool,
        alt: bool,
    ) -> Stroke {
        let (rect, ppp) = (self.canvas_rect, self.canvas_ppp);
        let Some(m) = self.width_marks(rect, ppp) else {
            return from.clone();
        };
        let Some(profile) = from.profile.as_ref() else {
            return from.clone();
        };
        if !mastik_core::build::affine_is_invertible(m.world) || m.half <= 0.0 {
            return from.clone();
        }
        // The press's half-width: the preview may have rescaled the stroke since.
        let held = from.width_var.is_some();
        let half = if held { m.half } else { from.width / 2.0 };
        let local = m.world.inverse() * world;
        let mut points = profile.points.clone();
        let i = grab.point();
        let Some(wp) = points.get(i).copied() else {
            return from.clone();
        };
        match grab {
            Grab::Dot(_) => {
                let lo = if i == 0 { 0.0 } else { points[i - 1].t };
                let hi = points.get(i + 1).map_or(1.0, |p| p.t);
                if let Some((t, _)) = profile::fraction_at(&m.path, local) {
                    points[i].t = t.clamp(lo, hi);
                }
            }
            Grab::Edge(_, side) => {
                let Some((p, n)) = station(&m.path, wp.t) else {
                    return from.clone();
                };
                let reach = match side {
                    Side::Left => (local - p).dot(n),
                    Side::Right => -(local - p).dot(n),
                }
                .max(0.0)
                    / half;
                let (own, other) = match side {
                    Side::Left => (wp.left, wp.right),
                    Side::Right => (wp.right, wp.left),
                };
                let other = match alt {
                    true => other,
                    false if own > 1e-9 => other * reach / own,
                    false => reach,
                };
                let (mut own, mut other) = (reach, other);
                if shift {
                    // The point's whole width, in units, to a whole number.
                    let whole = (own + other) * half;
                    let k = match whole > 1e-9 {
                        true => whole.round() / whole,
                        false => 0.0,
                    };
                    own *= k;
                    other *= k;
                }
                let (left, right) = match side {
                    Side::Left => (own, other),
                    Side::Right => (other, own),
                };
                points[i].left = left;
                points[i].right = right;
            }
        }
        let (points, k) = normalised(points, held);
        Stroke {
            width: from.width * k,
            profile: Some(WidthProfile { points }),
            ..from.clone()
        }
    }

    /// The transaction writing stroke `index` of `id` as `stroke`.
    fn width_tx(&self, id: NodeId, index: usize, stroke: Stroke) -> mastik_core::Transaction {
        let Some(node) = self.session.doc.get(id) else {
            return mastik_core::Transaction::default();
        };
        let mut strokes = node.paint().strokes.clone();
        match strokes.get_mut(index) {
            Some(s) => s.value = stroke,
            None => return mastik_core::Transaction::default(),
        }
        mastik_core::Transaction(vec![mastik_core::Operation::SetStrokes { id, strokes }])
    }

    fn commit_width(&mut self, id: NodeId, index: usize, stroke: Stroke) {
        let tx = self.width_tx(id, index, stroke);
        self.session.name_step("Stroke", icon::STROKE);
        self.session.commit(tx);
    }

    /// Preview a width drag; `world` is the pointer.
    pub(crate) fn update_width_drag(&mut self, world: Point, shift: bool, alt: bool) {
        let Drag::WidthPoint {
            id,
            index,
            grab,
            from,
        } = &self.drag
        else {
            return;
        };
        let (id, index, grab) = (*id, *index, *grab);
        let next = self.width_drag_stroke(grab, from, world, shift, alt);
        let tx = self.width_tx(id, index, next);
        self.session.set_preview(&tx);
    }

    /// Commit a width drag: one *Stroke* step for the whole gesture.
    pub(crate) fn finish_width_drag(
        &mut self,
        (id, index, grab): (NodeId, usize, Grab),
        from: &Stroke,
        world: Point,
        (shift, alt): (bool, bool),
    ) {
        let next = self.width_drag_stroke(grab, from, world, shift, alt);
        if next == *from {
            self.session.clear_preview();
            return;
        }
        self.commit_width(id, index, next);
    }

    /// The profile's dots and diamonds, the ghost dot where a click would add a
    /// point, and a drag's width beside the pointer.
    pub(crate) fn draw_width_handles(&self, painter: &egui::Painter, rect: egui::Rect, ppp: f32) {
        let Some(m) = self.width_marks(rect, ppp) else {
            return;
        };
        let under = egui::Stroke::new(3.0, egui::Color32::from_black_alpha(115));
        let line = egui::Stroke::new(1.0, egui::Color32::WHITE);
        let rim = egui::Stroke::new(1.5, color::SELECT);
        let pointer = painter.ctx().input(|i| i.pointer.latest_pos());
        if self.drag.is_none()
            && !self.selection_locked()
            && let Some(at) = pointer
            && m.grab_at(at).is_none()
            && let Some(t) = self.width_path_at(&m, at, rect, ppp)
            && let Some((p, _)) = profile::point_at(&m.path, t)
        {
            let ghost = self.to_screen(m.world * p, rect, ppp);
            painter.circle_filled(ghost, DOT_R, egui::Color32::WHITE.gamma_multiply(0.5));
            painter.circle_stroke(ghost, DOT_R, rim);
        }
        let diamond = |c: egui::Pos2| {
            let r = DIAMOND_R;
            egui::Shape::convex_polygon(
                vec![
                    c + egui::vec2(0.0, -r),
                    c + egui::vec2(r, 0.0),
                    c + egui::vec2(0.0, r),
                    c + egui::vec2(-r, 0.0),
                ],
                egui::Color32::WHITE,
                rim,
            )
        };
        for (dot, left, right) in &m.points {
            painter.line_segment([*left, *right], under);
            painter.line_segment([*left, *right], line);
            painter.add(diamond(*left));
            painter.add(diamond(*right));
            painter.circle_filled(*dot, DOT_R, egui::Color32::WHITE);
            painter.circle_stroke(*dot, DOT_R, rim);
        }
        if let (Drag::WidthPoint { grab, .. }, Some(at)) = (&self.drag, pointer)
            && let Some(wp) = m
                .stroke
                .profile
                .as_ref()
                .and_then(|p| p.points.get(grab.point()))
        {
            let whole = (wp.left + wp.right) * m.half;
            let text = format!("Width {}", crate::ui::number(2)(whole, 0..=2));
            let hue = crate::canvas::chrome_hue(&self.session.doc, m.id);
            crate::spacing::paint_label(painter, at, &text, None, hue);
        }
    }
}

/// The handles driven through the whole canvas with real pointer events. Plain
/// backticks: `cargo doc` cannot see this module.
#[cfg(test)]
mod tests {
    use super::*;
    use mastik_core::profile::Preset;
    use mastik_core::{Document, IdSource, NodeKind, Operation, Transaction};

    const RECT: egui::Rect = egui::Rect {
        min: egui::Pos2::ZERO,
        max: egui::Pos2::new(800.0, 600.0),
    };

    /// A selected line from (0, 0) to (200, 0) with a 20-wide *Taper both*
    /// stroke, its popover open, the camera on the line's middle at 100%.
    fn app_with_a_taper(ctx: &egui::Context) -> (MastikApp, NodeId) {
        crate::theme::install(ctx);
        let mut app = MastikApp::headless(ctx);
        let mut ids = IdSource::new(0x71D);
        let root = ids.mint();
        let line = ids.mint();
        let mut doc = Document::new(root);
        doc.apply(&Transaction(vec![
            Operation::CreateNode {
                id: line,
                parent: root,
                index: 0,
                kind: NodeKind::Line {
                    end: Point::new(200.0, 0.0),
                },
                transform: Some(Affine::IDENTITY),
                name: None,
            },
            Operation::SetStrokes {
                id: line,
                strokes: mastik_core::keyed_by_position([Stroke {
                    width: 20.0,
                    profile: Some(Preset::TaperBoth.profile()),
                    ..Stroke::default()
                }]),
            },
        ]))
        .expect("the line");
        app.session.adopt_document(doc, None);
        app.session.camera.center = Point::new(100.0, 0.0);
        app.session.camera.zoom = 1.0;
        app.session.selection.set_one(line);
        app.stroke_menu = Some(0);
        for _ in 0..2 {
            frame(ctx, &mut app, Vec::new(), Default::default());
        }
        (app, line)
    }

    fn frame(
        ctx: &egui::Context,
        app: &mut MastikApp,
        events: Vec<egui::Event>,
        modifiers: egui::Modifiers,
    ) {
        let time = ctx.input(|i| i.time) + 0.1;
        let _ = ctx.run_ui(
            egui::RawInput {
                screen_rect: Some(RECT),
                time: Some(time),
                modifiers,
                events,
                ..Default::default()
            },
            |ui| app.canvas_ui(ui),
        );
    }

    fn button(
        at: egui::Pos2,
        button: egui::PointerButton,
        pressed: bool,
        modifiers: egui::Modifiers,
    ) -> egui::Event {
        egui::Event::PointerButton {
            pos: at,
            button,
            pressed,
            modifiers,
        }
    }

    fn at(app: &MastikApp, w: Point) -> egui::Pos2 {
        app.to_screen(w, RECT, 1.0)
    }

    /// Press at world `from`, drag to `to` in eight steps holding `modifiers`,
    /// release.
    fn drag(
        ctx: &egui::Context,
        app: &mut MastikApp,
        from: Point,
        to: Point,
        modifiers: egui::Modifiers,
    ) {
        let start = at(app, from);
        let primary = egui::PointerButton::Primary;
        frame(ctx, app, vec![egui::Event::PointerMoved(start)], modifiers);
        frame(
            ctx,
            app,
            vec![button(start, primary, true, modifiers)],
            modifiers,
        );
        for step in 1..=8 {
            let p = at(app, from + (to - from) * (f64::from(step) / 8.0));
            frame(ctx, app, vec![egui::Event::PointerMoved(p)], modifiers);
        }
        let end = at(app, to);
        frame(
            ctx,
            app,
            vec![button(end, primary, false, modifiers)],
            modifiers,
        );
        frame(ctx, app, Vec::new(), Default::default());
    }

    /// Move onto world `w`, wait out the double-click window, press and release
    /// `which` there.
    fn click(ctx: &egui::Context, app: &mut MastikApp, w: Point, which: egui::PointerButton) {
        let p = at(app, w);
        frame(
            ctx,
            app,
            vec![egui::Event::PointerMoved(p)],
            Default::default(),
        );
        for _ in 0..40 {
            frame(ctx, app, Vec::new(), Default::default());
        }
        frame(
            ctx,
            app,
            vec![button(p, which, true, Default::default())],
            Default::default(),
        );
        frame(
            ctx,
            app,
            vec![button(p, which, false, Default::default())],
            Default::default(),
        );
        frame(ctx, app, Vec::new(), Default::default());
    }

    fn stroke(app: &MastikApp, id: NodeId) -> Stroke {
        app.session.doc.get(id).unwrap().paint().strokes[0]
            .value
            .clone()
    }

    /// Dragging the middle point's left diamond out to 20 widens both sides of
    /// it to 20, so the stroke's width becomes 40, the widest it draws, and the
    /// profile is rescaled under it; one *Stroke* step, the line unmoved.
    /// flip: removing `begin_width_drag`'s rung from the press → fails at "the
    /// line did not move".
    #[test]
    fn a_diamond_widens_both_sides_and_the_width_stays_the_widest() {
        let ctx = egui::Context::default();
        let (mut app, id) = app_with_a_taper(&ctx);
        assert!(app.edited_profile().is_some(), "the handles are up");
        let depth = app.session.history.undo_depth();
        drag(
            &ctx,
            &mut app,
            Point::new(100.0, 10.0),
            Point::new(100.0, 20.0),
            Default::default(),
        );
        let node = app.session.doc.get(id).unwrap();
        assert_eq!(node.transform(), Affine::IDENTITY, "the line did not move");
        let s = stroke(&app, id);
        assert!((s.width - 40.0).abs() < 1e-6, "the width: {}", s.width);
        let mid = s.profile.as_ref().unwrap().points[1];
        assert!(
            (mid.left - 1.0).abs() < 1e-6 && (mid.right - 1.0).abs() < 1e-6,
            "{mid:?}"
        );
        assert_eq!(app.session.history.undo_depth(), depth + 1, "one step");
        let step = app.session.history.done().last().expect("a step");
        assert_eq!(step.note.label, "Stroke");
    }

    /// With Alt only the grabbed side moves.
    /// flip: `width_drag_stroke` ignoring `alt` → fails at the right side.
    #[test]
    fn alt_widens_only_its_own_side() {
        let ctx = egui::Context::default();
        let (mut app, id) = app_with_a_taper(&ctx);
        drag(
            &ctx,
            &mut app,
            Point::new(100.0, 10.0),
            Point::new(100.0, 20.0),
            egui::Modifiers::ALT,
        );
        let s = stroke(&app, id);
        let mid = s.profile.as_ref().unwrap().points[1];
        assert!((s.width - 40.0).abs() < 1e-6, "the width: {}", s.width);
        assert!((mid.left - 1.0).abs() < 1e-6, "the left: {mid:?}");
        assert!(
            (mid.right - 0.5).abs() < 1e-6,
            "the right side kept its 10: {mid:?}"
        );
    }

    /// A click on the bare path adds a point at the width drawn there, so the
    /// stroke draws as it did; a right-click on it takes it away again.
    /// flip: removing `width_click`'s rung from the click → fails at "added".
    #[test]
    fn a_click_adds_a_point_and_a_right_click_removes_it() {
        let ctx = egui::Context::default();
        let (mut app, id) = app_with_a_taper(&ctx);
        let before = stroke(&app, id).profile.unwrap();
        click(
            &ctx,
            &mut app,
            Point::new(50.0, 0.0),
            egui::PointerButton::Primary,
        );
        let added = stroke(&app, id).profile.unwrap();
        assert_eq!(added.points.len(), 4, "added");
        let t = added.points[1].t;
        assert!((t - 0.25).abs() < 1e-3, "at a quarter: {t}");
        assert!(
            (added.at(t).0 - before.at(t).0).abs() < 1e-9,
            "drawn as it was"
        );
        assert_eq!(app.session.selection.ids(), [id], "still selected");
        click(
            &ctx,
            &mut app,
            Point::new(50.0, 0.0),
            egui::PointerButton::Secondary,
        );
        assert_eq!(stroke(&app, id).profile.unwrap().points.len(), 3, "removed");
    }

    /// Without the popover open the handles are down and a press on the line
    /// is the line's.
    #[test]
    fn the_handles_need_the_popover() {
        let ctx = egui::Context::default();
        let (mut app, _) = app_with_a_taper(&ctx);
        app.stroke_menu = None;
        assert!(app.edited_profile().is_none());
        assert!(!app.width_claims(at(&app, Point::new(100.0, 10.0)), RECT, 1.0));
    }
}
