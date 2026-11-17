//! Mirror while drawing (`docs/design/app-pen-and-nodes.md`, *Mirror while
//! drawing*): the pen and the pencil draw the other half of a new path as they go,
//! reflected across an axis through the frame being drawn in.
//!
//! The halves join where they meet the axis. A run that starts and ends on it
//! closes as one symmetric path; one end on it makes one open path joined there;
//! neither leaves two subpaths in the one layer. *Both axes* is the vertical rule
//! then the horizontal one over what that made, so a quarter drawn from axis to
//! axis closes into a whole.
//!
//! *Around a centre* turns copies of the run about a point instead ([`radial`]);
//! copies whose ends meet are welded.
//!
//! A path drawn mirrored is saved mirrored (`mastik_core::symmetry`), and point
//! editing keeps it so ([`mastik_core::symmetry::symmetric_edit`]), its axis points
//! locked to the axes;
//! a new run drawn inside it mirrors across its own axes ([`MastikApp::mirror_frame`]).

use crate::app::MastikApp;
use crate::preview::{PenSubpath, PointRef};
use crate::theme::{color, icon};
use crate::ui;
use eframe::egui;
use mastik_core::kurbo::{Affine, BezPath, Point};
use mastik_core::symmetry::{PathMirror, partners_by};
use mastik_core::{NodeId, NodeKind, Operation, Transaction};

/// The symmetry the pen and the pencil draw with: mirrored across one axis or
/// both, or copies turned about a centre.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) enum Axes {
    #[default]
    Off,
    Vertical,
    Horizontal,
    Both,
    /// Copies turned about a centre ([`Mirror::copies`]).
    Centre,
}

impl Axes {
    /// The flyout's rows, in order.
    pub(crate) const ALL: [Axes; 5] = [
        Axes::Off,
        Axes::Vertical,
        Axes::Horizontal,
        Axes::Both,
        Axes::Centre,
    ];

    pub(crate) fn label(self) -> &'static str {
        match self {
            Axes::Off => "Off",
            Axes::Vertical => "Vertical axis",
            Axes::Horizontal => "Horizontal axis",
            Axes::Both => "Both axes",
            Axes::Centre => "Around a centre",
        }
    }

    pub(crate) fn vertical(self) -> bool {
        matches!(self, Axes::Vertical | Axes::Both)
    }

    pub(crate) fn horizontal(self) -> bool {
        matches!(self, Axes::Horizontal | Axes::Both)
    }
}

/// How many copies a centre starts with.
const START_COPIES: u8 = 6;

/// The document's mirror: the axes, and where they cross once dragged there.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct Mirror {
    pub(crate) axes: Axes,
    /// `None` until an axis or the centre is dragged: the middle of the frame
    /// being drawn in, or of the view on open canvas.
    pub(crate) at: Option<Point>,
    /// Under [`Axes::Centre`], how many copies turn about it, the drawn one
    /// counted; kept while another choice is ticked.
    pub(crate) copies: u8,
}

impl Default for Mirror {
    fn default() -> Self {
        Self {
            axes: Axes::Off,
            at: None,
            copies: START_COPIES,
        }
    }
}

/// The symmetry a new run is drawn with, in the space its axes are
/// axis-aligned in: the world for the drawing switch, a symmetric path's own
/// space for its own.
#[derive(Clone, Copy, Debug)]
pub(crate) struct Frame {
    pub(crate) axes: Axes,
    /// Where the axes cross, or the centre, in this frame's space.
    pub(crate) at: Point,
    pub(crate) to_world: Affine,
    /// A symmetric path's own, which is where its points put it and so has no
    /// grab.
    pub(crate) own: bool,
    /// Under [`Axes::Centre`], the copies and whether each is mirrored.
    pub(crate) copies: u8,
    pub(crate) reflect: bool,
    /// Under [`Axes::Centre`], the angle of the first slice line from the centre:
    /// straight up for the switch; a mirror line of a path's own.
    pub(crate) phase: f64,
}

/// What a mirror grab moves: one axis, `true` for the vertical, or the centre.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Grip {
    Axis(bool),
    Centre,
}

/// The axes' grab pills, or the centre's grip, as last painted, which a press on
/// leaves to them: the pen, the pencil and the *On axis* chip stay out of a grip
/// and its drag.
#[derive(Clone, Debug, Default)]
pub(crate) struct Grips {
    /// Each grip's hit rect in screen pt, and what it moves.
    pub(crate) pills: Vec<(Grip, egui::Rect)>,
    /// What a grip drag is moving, while one is.
    pub(crate) held: Option<Grip>,
    /// Where the axes were when the held pill was pressed, which a right-click
    /// puts back (`MastikApp::cancel_gesture`). Kept across frames while held.
    pub(crate) from: Option<Option<Point>>,
}

/// Mark the path `tx` creates as mirrored across `mirror`; the model keeps only
/// the axes that hold.
pub(crate) fn mark_mirrored(tx: &mut Transaction, mirror: PathMirror) {
    for op in &mut tx.0 {
        if let Operation::CreateNode {
            kind: NodeKind::Path { mirror: m, .. },
            ..
        } = op
        {
            *m = mirror;
        }
    }
}

/// What a run drawn mirrored says when its frame's turn leaves the path no axis
/// to be saved mirrored by.
pub(crate) const SAVED_PLAIN: &str =
    "Mirrored, saved as a plain path: the frame is turned, so point edits will not mirror";

/// The symmetry `frame` drew with as the path's own, in a parent placed by
/// `parent_world`: a world axis that lies along a local one (the parent turned by
/// a quarter turn or none) is kept as that axis, and a centre's copies wherever
/// the parent keeps angles (turned, mirrored or scaled evenly). `true` beside it
/// when the rule could not be kept, for the status to say.
pub(crate) fn local_mirror(frame: &Frame, parent_world: Affine) -> (PathMirror, bool) {
    let axes = frame.axes;
    if axes == Axes::Centre {
        let [a, b, c, d, _, _] = parent_world.as_coeffs();
        let (u, v) = (a.hypot(b), c.hypot(d));
        let tilt = 1e-6 * u.max(v);
        let even = (u - v).abs() <= tilt && (a * c + b * d).abs() <= tilt * u.max(v);
        return match even {
            true => (PathMirror::around(frame.copies, frame.reflect), false),
            false => (PathMirror::NONE, true),
        };
    }
    let inv = parent_world.inverse();
    let mut out = PathMirror::NONE;
    let mut dropped = false;
    // The world line's direction in the parent: along local y keeps it vertical.
    let mut keep = |dir: mastik_core::kurbo::Vec2| {
        let d = inv * dir.to_point() - inv * Point::ZERO;
        let tilt = 1e-6 * d.hypot();
        if d.x.abs() <= tilt {
            out.vertical = true;
        } else if d.y.abs() <= tilt {
            out.horizontal = true;
        } else {
            dropped = true;
        }
    };
    if axes.vertical() {
        keep(mastik_core::kurbo::Vec2::new(0.0, 1.0));
    }
    if axes.horizontal() {
        keep(mastik_core::kurbo::Vec2::new(1.0, 0.0));
    }
    (out, dropped)
}

/// The reflection across the vertical line `x = c` or the horizontal `y = c`.
fn reflection(vertical: bool, c: f64) -> Affine {
    match vertical {
        true => Affine::new([-1.0, 0.0, 0.0, 1.0, 2.0 * c, 0.0]),
        false => Affine::new([1.0, 0.0, 0.0, -1.0, 0.0, 2.0 * c]),
    }
}

/// `drawn` and its mirror images across `axes` crossing at `at`, joined where an
/// end lies within `tol` of an axis (pulled onto it exactly).
pub(crate) fn mirrored(drawn: PenSubpath, axes: Axes, at: Point, tol: f64) -> Vec<PenSubpath> {
    let mut subs = vec![drawn];
    if axes.vertical() {
        subs = subs
            .into_iter()
            .flat_map(|s| across(s, true, at.x, tol))
            .collect();
    }
    if axes.horizontal() {
        subs = subs
            .into_iter()
            .flat_map(|s| across(s, false, at.y, tol))
            .collect();
    }
    subs
}

/// One subpath mirrored across one axis, by the joining rule.
fn across(mut s: PenSubpath, vertical: bool, c: f64, tol: f64) -> Vec<PenSubpath> {
    let reflect = reflection(vertical, c);
    let image = |s: &PenSubpath| {
        let mut one = [s.clone()];
        crate::tools::map_subpaths(&mut one, reflect);
        let [m] = one;
        m
    };
    let n = s.anchors.len();
    // A reflection turns the winding, so a closed image runs backwards to wind
    // with the run: opposite windings cancel a fill where the two overlap. One
    // already its own image (Both axes after the vertical rule closed it) adds
    // nothing but a coincident twin. Only an exact image counts, one to one with
    // handles: `tol` is the snapping reach, and a run merely within it of its image
    // is not symmetric, so it still takes the image.
    if s.closed {
        let mut m = image(&s);
        let size = s
            .anchors
            .iter()
            .map(|a| a.at.x.abs().max(a.at.y.abs()))
            .fold(0.0, f64::max);
        let exact = 1e-6 * (1.0 + size);
        let near =
            |u: mastik_core::kurbo::Vec2, v: mastik_core::kurbo::Vec2| (u - v).hypot() <= exact;
        let mut used = vec![false; n];
        let own = m.anchors.len() == n
            && m.anchors.iter().all(|a| {
                let twin = (0..n).find(|j| {
                    let b = &s.anchors[*j];
                    !used[*j]
                        && (a.at - b.at).hypot() <= exact
                        && ((near(a.out, b.out) && near(a.back, b.back))
                            || (near(a.out, b.back) && near(a.back, b.out)))
                });
                twin.inspect(|j| used[*j] = true).is_some()
            });
        if own {
            return vec![s];
        }
        m.reverse();
        return vec![s, m];
    }
    if n < 2 {
        let m = image(&s);
        return vec![s, m];
    }
    let off = |p: Point| match vertical {
        true => (p.x - c).abs(),
        false => (p.y - c).abs(),
    };
    let onto = |p: Point| match vertical {
        true => Point::new(c, p.y),
        false => Point::new(p.x, c),
    };
    let start_on = off(s.anchors[0].at) <= tol;
    let end_on = off(s.anchors[n - 1].at) <= tol;
    if start_on {
        s.anchors[0].at = onto(s.anchors[0].at);
    }
    if end_on {
        s.anchors[n - 1].at = onto(s.anchors[n - 1].at);
    }
    // The image run backwards: from the end's image to the start's.
    let mut back = image(&s);
    back.reverse();
    match (start_on, end_on) {
        // Wound with the run as a closed image is, for a fill added later.
        (false, false) => vec![s, back],
        (_, true) => {
            let mut anchors = s.anchors;
            let b = back.anchors;
            if let Some(last) = anchors.last_mut() {
                last.out = b[0].out;
            }
            anchors.extend_from_slice(&b[1..]);
            if start_on {
                // The image of the start comes round onto the start: one anchor.
                if let Some(tail) = anchors.pop() {
                    anchors[0].back = tail.back;
                }
            }
            vec![PenSubpath {
                anchors,
                closed: start_on,
            }]
        }
        (true, false) => {
            let mut anchors = back.anchors;
            if let Some(last) = anchors.last_mut() {
                last.out = s.anchors[0].out;
            }
            anchors.extend_from_slice(&s.anchors[1..]);
            vec![PenSubpath {
                anchors,
                closed: false,
            }]
        }
    }
}

/// The lines a centre's copies are cut along, as angles from it: one slice line
/// per copy from `frame.phase`, and under `reflect` a mirror line halfway
/// between each pair, the second of each pair returned.
pub(crate) fn slice_lines(frame: &Frame) -> Vec<(f64, bool)> {
    let n = frame.copies.max(mastik_core::symmetry::MIN_COPIES);
    let step = std::f64::consts::TAU / f64::from(n);
    let mut out: Vec<(f64, bool)> = (0..n)
        .map(|k| (frame.phase + step * f64::from(k), false))
        .collect();
    if frame.reflect {
        out.extend((0..n).map(|k| (frame.phase + step * (f64::from(k) + 0.5), true)));
    }
    out
}

/// `p` pulled onto `frame`'s centre or one of its lines within `tol`, and what it
/// landed on: `Some(true)` for the centre, `Some(false)` for a line.
pub(crate) fn snap_radial(p: Point, frame: &Frame, tol: f64) -> (Point, Option<bool>) {
    let c = frame.at;
    if (p - c).hypot() <= tol {
        return (c, Some(true));
    }
    for (angle, _) in slice_lines(frame) {
        let d = mastik_core::kurbo::Vec2::new(angle.cos(), angle.sin());
        let t = (p - c).dot(d);
        if t > 0.0 && d.cross(p - c).abs() <= tol {
            return (c + d * t, Some(false));
        }
    }
    (p, None)
}

/// `drawn` and its copies turned about `frame`'s centre, each mirrored too under
/// `reflect` (across the line halfway through the first slice, then turned),
/// an open run's ends within `tol` of the centre or a line pulled onto it. Copies
/// whose ends meet are welded into one run, and a run whose ends meet closes, so
/// a petal drawn from line to line closes and a ring drawn from slice to slice
/// comes round whole.
pub(crate) fn radial(mut drawn: PenSubpath, frame: &Frame, tol: f64) -> Vec<PenSubpath> {
    use mastik_core::symmetry::{reflect_through, turn_about};
    let c = frame.at;
    let n = frame.copies.max(mastik_core::symmetry::MIN_COPIES);
    let step = std::f64::consts::TAU / f64::from(n);
    let len = drawn.anchors.len();
    if !drawn.closed && len >= 2 {
        for i in [0, len - 1] {
            drawn.anchors[i].at = snap_radial(drawn.anchors[i].at, frame, tol).0;
        }
    }
    let size = drawn
        .anchors
        .iter()
        .map(|a| (a.at - c).hypot())
        .fold(0.0, f64::max);
    let exact = 1e-6 * (1.0 + size);
    let carry = |s: &PenSubpath, map: Affine| {
        let mut one = [s.clone()];
        crate::tools::map_subpaths(&mut one, map);
        let [m] = one;
        m
    };
    let mut base = vec![drawn.clone()];
    if frame.reflect {
        // A reflection turns the winding, so the image runs backwards to wind
        // with the run, as the mirror's does.
        let mut image = carry(&drawn, reflect_through(c, frame.phase + step / 2.0));
        image.reverse();
        base.push(image);
    }
    let mut pieces: Vec<PenSubpath> = Vec::new();
    for k in 0..n {
        let turn = turn_about(c, step * f64::from(k));
        for b in &base {
            let piece = carry(b, turn);
            // A closed run that is already its own copy comes once.
            let twin = |q: &PenSubpath| {
                q.closed
                    && q.anchors.len() == piece.anchors.len()
                    && piece
                        .anchors
                        .iter()
                        .all(|a| q.anchors.iter().any(|b| (a.at - b.at).hypot() <= exact))
            };
            if !(piece.closed && pieces.iter().any(twin)) {
                pieces.push(piece);
            }
        }
    }
    weld(pieces, exact)
}

/// Open runs whose ends meet within `exact` joined into one, either way round,
/// the join keeping the arriving handle of one and the leaving handle of the
/// other. A run whose two ends meet is closed first, and again after each join,
/// so petals that share the centre close one by one rather than chaining.
fn weld(mut pieces: Vec<PenSubpath>, exact: f64) -> Vec<PenSubpath> {
    let meet = |a: Point, b: Point| (a - b).hypot() <= exact;
    let close = |s: &mut PenSubpath| {
        let n = s.anchors.len();
        if !s.closed && n > 2 && meet(s.anchors[0].at, s.anchors[n - 1].at) {
            let tail = s.anchors.pop().expect("more than two");
            s.anchors[0].back = tail.back;
            s.closed = true;
        }
    };
    pieces.iter_mut().for_each(close);
    let first = |s: &PenSubpath| s.anchors.first().map(|a| a.at);
    let last = |s: &PenSubpath| s.anchors.last().map(|a| a.at);
    let join = |mut a: PenSubpath, b: PenSubpath| {
        if let (Some(end), Some(start)) = (a.anchors.last_mut(), b.anchors.first()) {
            end.out = start.out;
        }
        a.anchors.extend_from_slice(&b.anchors[1..]);
        a
    };
    'again: loop {
        for i in 0..pieces.len() {
            for j in 0..pieces.len() {
                let (a, b) = (&pieces[i], &pieces[j]);
                if i == j || a.closed || b.closed || a.anchors.len() < 2 || b.anchors.len() < 2 {
                    continue;
                }
                let (Some(a0), Some(a1), Some(b0), Some(b1)) =
                    (first(a), last(a), first(b), last(b))
                else {
                    continue;
                };
                // Only where two ends meet: where more do (copies drawn from the
                // centre), any pairing is a guess, and each copy closes on its own.
                let ends_at = |p: Point| {
                    pieces
                        .iter()
                        .filter(|s| !s.closed && s.anchors.len() >= 2)
                        .flat_map(|s| [first(s), last(s)])
                        .flatten()
                        .filter(|q| meet(*q, p))
                        .count()
                };
                let way = if meet(a1, b0) && ends_at(a1) == 2 {
                    0
                } else if meet(a1, b1) && ends_at(a1) == 2 {
                    1
                } else if meet(a0, b0) && ends_at(a0) == 2 {
                    2
                } else {
                    continue;
                };
                let (hi, lo) = (i.max(j), i.min(j));
                let one = pieces.remove(hi);
                let other = pieces.remove(lo);
                let (mut a, mut b) = if i > j { (one, other) } else { (other, one) };
                match way {
                    1 => b.reverse(),
                    2 => a.reverse(),
                    _ => {}
                }
                let mut joined = join(a, b);
                close(&mut joined);
                pieces.push(joined);
                continue 'again;
            }
        }
        break;
    }
    pieces
}

/// The maps that carry a symmetric path onto itself, about `at` (its
/// [`mastik_core::symmetry::origin`]), each with whether it is its own inverse:
/// the reflection across each axis it is mirrored on, or a centre's turns and,
/// when each copy is mirrored, its reflections. Applied one after another over a
/// growing set, they reach every image.
fn symmetry_maps(path: &BezPath, radii: &[f64], m: PathMirror, at: Point) -> Vec<(Affine, bool)> {
    use mastik_core::symmetry::{reflect_through, reflection_axis, turn_about};
    if !m.is_radial() {
        return [(true, at.x, m.vertical), (false, at.y, m.horizontal)]
            .into_iter()
            .filter(|(_, _, on)| *on)
            .map(|(vertical, c, _)| (reflection(vertical, c), true))
            .collect();
    }
    let step = std::f64::consts::TAU / f64::from(m.copies);
    let mut out: Vec<(Affine, bool)> = (1..m.copies)
        .map(|k| (turn_about(at, step * f64::from(k)), false))
        .collect();
    if m.reflect
        && let Some(axis) = reflection_axis(path, radii)
    {
        out.extend(
            (0..m.copies).map(|k| (reflect_through(at, axis + step * f64::from(k) / 2.0), true)),
        );
    }
    out
}

/// How near an axis, in screen pt, an end is taken as on it.
pub(crate) const ON_AXIS_PT: f64 = 8.0;

/// How far a mirrored path's own axis runs past its box, in screen pt.
const OWN_AXIS_OVERHANG: f64 = 16.0;

/// The grab pill at an axis's end, in screen pt.
const PILL: egui::Vec2 = egui::vec2(16.0, 10.0);

/// The centre's grip across, in screen pt.
const CENTRE_GRIP: f32 = 14.0;

/// Whether the primary button holds a mirror grip `resp`: only the left button
/// moves an axis or the centre, as it moves a layer.
fn primary_grab(ui: &egui::Ui, resp: &egui::Response) -> bool {
    resp.dragged_by(egui::PointerButton::Primary)
        || (resp.is_pointer_button_down_on() && ui.input(|i| i.pointer.primary_down()))
}

impl MastikApp {
    /// The switch the next new run follows: inside a path edit that edit's own,
    /// which starts off whatever the drawing switch says; otherwise the drawing
    /// switch, the document's.
    pub(crate) fn mirror(&self) -> Mirror {
        match self.edited_path() {
            Some(id) => match self.edit_mirror {
                Some((of, m)) if of == id => m,
                _ => Mirror::default(),
            },
            None => self.session.mirror,
        }
    }

    /// [`Self::mirror`] to write to; inside an edit it is that edit's, made off
    /// on the first write.
    pub(crate) fn mirror_mut(&mut self) -> &mut Mirror {
        let Some(id) = self.edited_path() else {
            return &mut self.session.mirror;
        };
        if self.edit_mirror.is_none_or(|(of, _)| of != id) {
            self.edit_mirror = Some((id, Mirror::default()));
        }
        &mut self.edit_mirror.as_mut().expect("just set").1
    }

    /// The axes and where they cross, while mirroring is on: the dragged point, or
    /// the middle of the frame under `near` (the run's first point while one is
    /// drawn), or of the view.
    pub(crate) fn mirror_axes(&self, near: Option<Point>) -> Option<(Axes, Point)> {
        let m = self.mirror();
        if m.axes == Axes::Off {
            return None;
        }
        let at =
            m.at.or_else(|| {
                let frame = self.run_frame(near)?;
                (frame != self.session.page)
                    .then(|| self.session.resolved.world_bounds(frame))
                    .flatten()
                    .map(|b| b.center())
            })
            .unwrap_or_else(|| self.view_middle());
        Some((m.axes, at))
    }

    /// The middle of the canvas past the rulers, in world space; the camera's
    /// centre before the canvas is first laid out.
    fn view_middle(&self) -> Point {
        let rect = self.canvas_rect;
        if !rect.is_finite() || !rect.is_positive() {
            return self.session.camera.center;
        }
        self.to_world(self.canvas_inner(rect).center(), rect, self.canvas_ppp)
    }

    /// The frame the switch's axes are drawn through, in world space: the one the
    /// run began in, else the one under `near`, when the axes cross inside it.
    /// `None` draws them down the whole view: on open canvas, or once they were
    /// dragged out of that frame.
    fn axis_frame(&self, near: Option<Point>, at: Point) -> Option<mastik_core::kurbo::Rect> {
        let frame = self.run_frame(near)?;
        if frame == self.session.page {
            return None;
        }
        self.session
            .resolved
            .world_bounds(frame)
            .filter(|b| b.contains(at))
    }

    /// The axis whose pill claims the pointer at `screen`, or whose pill is being
    /// dragged: `true` for the vertical. `screen` is `None` off the canvas.
    pub(crate) fn mirror_grip(&self, screen: Option<egui::Pos2>) -> Option<Grip> {
        self.mirror_grips.held.or_else(|| {
            let p = screen?;
            self.mirror_grips
                .pills
                .iter()
                .find(|(_, r)| r.contains(p))
                .map(|(grip, _)| *grip)
        })
    }

    /// The frame the run in flight lands in, else the one under `near`. A pen
    /// run's is the parent fixed at its first press, not the frame under its first
    /// anchor, which Ctrl can move into another frame.
    fn run_frame(&self, near: Option<Point>) -> Option<NodeId> {
        match &self.pen {
            Some(p) if p.editing.as_ref().is_none_or(|e| e.minted) => Some(p.parent),
            _ => Some(self.draw_target(self.drawing_from().or(near)?).0),
        }
    }

    /// Where the run being drawn began, pen or pencil.
    fn drawing_from(&self) -> Option<Point> {
        match (&self.pen, &self.pencil) {
            (Some(p), _) if p.editing.as_ref().is_none_or(|e| e.minted) => {
                p.anchors.first().map(|a| a.at)
            }
            (_, Some(s)) => s.origin(),
            _ => None,
        }
    }

    /// A mirrored path's own axes, where they cross in its space, and its world
    /// transform; `None` for a path that is not mirrored.
    pub(crate) fn path_mirror(&self, id: NodeId) -> Option<(PathMirror, Point, Affine)> {
        let s = &self.session;
        let NodeKind::Path { path, mirror, .. } = s.resolved.used_kind(&s.doc, id)? else {
            return None;
        };
        if mirror.is_none() {
            return None;
        }
        let world = self.session.resolved.world_transform(id)?;
        Some((*mirror, mastik_core::symmetry::origin(path, *mirror), world))
    }

    /// A radial path's own first slice line, as an angle from its centre: a line
    /// it is mirrored across, or the way to its first anchor off the centre.
    fn own_phase(&self, id: NodeId, c: Point) -> f64 {
        let s = &self.session;
        let Some(NodeKind::Path {
            path, corner_radii, ..
        }) = s.resolved.used_kind(&s.doc, id)
        else {
            return -std::f64::consts::FRAC_PI_2;
        };
        mastik_core::symmetry::reflection_axis(path, corner_radii)
            .or_else(|| {
                let b = path.control_box();
                let tol = 1e-6 * (1.0 + b.width().max(b.height()));
                mastik_core::geometry::anchor_runs(path)
                    .iter()
                    .flat_map(|r| r.points.clone())
                    .find(|p| (*p - c).hypot() > tol)
                    .map(|p| (p - c).atan2())
            })
            .unwrap_or(-std::f64::consts::FRAC_PI_2)
    }

    /// The mirror a new run follows, if any: inside a mirrored path the node tool
    /// edits, that path's own axes; otherwise the drawing switch's. Extending an
    /// end follows none.
    pub(crate) fn mirror_frame(&self, near: Option<Point>) -> Option<Frame> {
        let editing = match &self.pen {
            Some(p) => match &p.editing {
                Some(e) if !e.minted => return None,
                Some(e) => Some(e.id),
                None => None,
            },
            None => self.edited_path(),
        };
        if let Some(id) = editing
            && let Some((m, at, to_world)) = self.path_mirror(id)
        {
            let axes = match (m.is_radial(), m.vertical, m.horizontal) {
                (true, ..) => Axes::Centre,
                (_, true, true) => Axes::Both,
                (_, true, false) => Axes::Vertical,
                _ => Axes::Horizontal,
            };
            return Some(Frame {
                axes,
                at,
                to_world,
                own: true,
                copies: m.copies,
                reflect: m.reflect,
                phase: self.own_phase(id, at),
            });
        }
        // The node tool shows the switch's axes only once its pen is armed.
        if self.pen.is_none() && self.tool == crate::tools::Tool::Node && !self.pen_bias {
            return None;
        }
        let m = self.mirror();
        self.mirror_axes(near).map(|(axes, at)| Frame {
            axes,
            at,
            to_world: Affine::IDENTITY,
            own: false,
            copies: m.copies,
            reflect: false,
            phase: -std::f64::consts::FRAC_PI_2,
        })
    }

    /// How near an axis an end is taken as on it, in `frame`'s space.
    fn axis_tol(&self, frame: &Frame) -> f64 {
        let scale = frame.to_world.determinant().abs().sqrt().max(1e-9);
        ON_AXIS_PT / self.points_per_world() / scale
    }

    /// The new run `drawn` as the mirror makes it, in world space; just `drawn`
    /// with the mirror off.
    pub(crate) fn mirror_run(&self, drawn: PenSubpath) -> Vec<PenSubpath> {
        let near = drawn.anchors.first().map(|a| a.at);
        let Some(frame) = self.mirror_frame(near) else {
            return vec![drawn];
        };
        let tol = self.axis_tol(&frame);
        let mut local = [drawn];
        crate::tools::map_subpaths(&mut local, frame.to_world.inverse());
        let [local] = local;
        let mut subs = match frame.axes {
            Axes::Centre => radial(local, &frame, tol),
            axes => mirrored(local, axes, frame.at, tol),
        };
        crate::tools::map_subpaths(&mut subs, frame.to_world);
        subs
    }

    /// `p` pulled onto an axis it lies within [`ON_AXIS_PT`] of, and whether it
    /// was: where a pen anchor snaps.
    pub(crate) fn snap_to_axis(&self, p: Point) -> (Point, bool) {
        let Some(frame) = self.mirror_frame(Some(p)) else {
            return (p, false);
        };
        let tol = self.axis_tol(&frame);
        let (axes, at) = (frame.axes, frame.at);
        let local = frame.to_world.inverse() * p;
        if axes == Axes::Centre {
            let (q, on) = snap_radial(local, &frame, tol);
            return (frame.to_world * q, on.is_some());
        }
        let mut q = local;
        let mut on = false;
        if axes.vertical() && (local.x - at.x).abs() <= tol {
            q.x = at.x;
            on = true;
        }
        if axes.horizontal() && (local.y - at.y).abs() <= tol {
            q.y = at.y;
            on = true;
        }
        (frame.to_world * q, on)
    }

    /// `subpaths` (world), a point edit of `id` that started from `base`, written
    /// back through [`mastik_core::edit::rewrite_points`], which keeps a mirrored
    /// path symmetric.
    pub(crate) fn rewrite_points(
        &self,
        id: NodeId,
        world: Affine,
        base: &[PenSubpath],
        subpaths: Vec<PenSubpath>,
    ) -> Transaction {
        let s = &self.session;
        crate::tools::rewrite_points(&s.doc, &s.resolved, id, world, base, subpaths)
    }

    /// Where adding a point at `t` along the segment leaving `at` on `id` adds one,
    /// with that point's images on a mirrored path, in the order to insert them:
    /// later anchors first, and on one segment the further point first, the
    /// nearer then measured along what is left. A segment that crosses an axis is
    /// its own image; a point near its middle goes in once, at the middle.
    pub(crate) fn mirrored_inserts(
        &self,
        id: NodeId,
        at: PointRef,
        t: f64,
    ) -> Vec<(PointRef, f64)> {
        let Some((mirror, c, _)) = self.path_mirror(id) else {
            return vec![(at, t)];
        };
        let Some(NodeKind::Path {
            path, corner_radii, ..
        }) = self.session.resolved.used_kind(&self.session.doc, id)
        else {
            return vec![(at, t)];
        };
        let subs = crate::tools::pen_anchors(path, corner_radii);
        let refs: Vec<PointRef> = subs
            .iter()
            .enumerate()
            .flat_map(|(s, sub)| (0..sub.anchors.len()).map(move |i| (s, i)))
            .collect();
        let flat = |r: PointRef| refs.iter().position(|x| *x == r);
        let b = path.control_box();
        let tol = 1e-6 * (1.0 + b.width().max(b.height()));
        // Within this of the middle, a point on a segment that is its own image is
        // the one point there.
        const MIDDLE: f64 = 0.02;
        let mut segs: Vec<(PointRef, f64)> = vec![(at, t)];
        for (map, involution) in symmetry_maps(path, corner_radii, mirror, c) {
            let pairs = partners_by(&subs, |p| map * p, involution, tol);
            for (seg, t) in segs.clone() {
                let Some((start, end)) = crate::tools::segment_ends(&subs, seg) else {
                    continue;
                };
                let image = |r: PointRef| flat(r).and_then(|k| pairs[k]).map(|(j, _)| refs[j]);
                let (Some(p0), Some(p1)) = (image(start), image(end)) else {
                    continue;
                };
                let leaves = |from: PointRef, to: PointRef| {
                    crate::tools::segment_ends(&subs, from).is_some_and(|(_, e)| e == to)
                };
                let found = if leaves(p0, p1) {
                    Some((p0, t))
                } else if leaves(p1, p0) {
                    Some((p1, 1.0 - t))
                } else {
                    None
                };
                let Some((s2, t2)) = found else { continue };
                if s2 == seg && (t - 0.5).abs() <= MIDDLE {
                    for x in &mut segs {
                        if x.0 == seg {
                            x.1 = 0.5;
                        }
                    }
                    continue;
                }
                if !segs.iter().any(|(s, u)| *s == s2 && (u - t2).abs() <= 1e-9) {
                    segs.push((s2, t2));
                }
            }
        }
        // Later anchors first, so an insert does not renumber the ones still to go.
        segs.sort_by(|a, b| b.0.cmp(&a.0).then(b.1.total_cmp(&a.1)));
        segs.dedup_by(|a, b| a.0 == b.0 && (a.1 - b.1).abs() <= 1e-9);
        // On one segment each nearer point is measured along the left part the
        // previous insert left, which ends at that insert's parameter.
        let mut prev: Option<(PointRef, f64)> = None;
        segs.into_iter()
            .map(|(seg, t)| {
                let rel = match prev {
                    Some((s, u)) if s == seg && u > 0.0 => t / u,
                    _ => t,
                };
                prev = Some((seg, t));
                (seg, rel)
            })
            .collect()
    }

    /// The image of every point in `points` on `id` across the path's own axes,
    /// with the points themselves: what deleting them takes, so the path stays
    /// symmetric.
    pub(crate) fn with_mirrored_points(&self, id: NodeId, points: &[PointRef]) -> Vec<PointRef> {
        let mut out = points.to_vec();
        let Some((mirror, at, _)) = self.path_mirror(id) else {
            return out;
        };
        let NodeKind::Path {
            path, corner_radii, ..
        } = self
            .session
            .resolved
            .used_kind(&self.session.doc, id)
            .expect("a mirrored path")
        else {
            return out;
        };
        let subs = crate::tools::pen_anchors(path, corner_radii);
        let refs: Vec<PointRef> = subs
            .iter()
            .enumerate()
            .flat_map(|(s, sub)| (0..sub.anchors.len()).map(move |i| (s, i)))
            .collect();
        let tol = 1e-6 * (1.0 + path.control_box().width().max(path.control_box().height()));
        for (map, involution) in symmetry_maps(path, corner_radii, mirror, at) {
            let pairs = partners_by(&subs, |p| map * p, involution, tol);
            for p in out.clone() {
                let Some(k) = refs.iter().position(|r| *r == p) else {
                    continue;
                };
                if let Some((j, _)) = pairs[k]
                    && !out.contains(&refs[j])
                {
                    out.push(refs[j]);
                }
            }
        }
        out
    }

    /// The glyph the path bar's symmetry button wears: the mirror's, or a
    /// centre's petals under *Around a centre*.
    pub(crate) fn symmetry_glyph(&self) -> &'static str {
        match self.mirror().axes {
            Axes::Centre => icon::RADIAL_SYMMETRY,
            _ => icon::FLIP_HORIZONTAL,
        }
    }

    /// The words the status line keeps while the pen or the pencil mirrors.
    pub(crate) fn mirror_status(&self) -> Option<String> {
        let m = self.mirror();
        let axes = m.axes;
        let across = match axes {
            Axes::Off => return None,
            Axes::Vertical => "the vertical axis",
            Axes::Horizontal => "the horizontal axis",
            Axes::Both => "both axes",
            Axes::Centre => {
                return Some(format!(
                    "Repeating {} copies around the centre · draw from one line to the next to close the shape",
                    m.copies
                ));
            }
        };
        let close = match axes {
            Axes::Both => "draw from one axis to the other to close the shape",
            _ => "start and end on the axis to close the shape",
        };
        Some(format!("Mirroring across {across} · {close}"))
    }

    /// Fix the axes where a new run begins at `p` when none was dragged and no frame
    /// is under it, so the view's middle they default to stays put while the run
    /// is drawn, however the view then moves. *Reset axis* lets them follow again.
    pub(crate) fn pin_mirror_axes(&mut self, p: Point) {
        let m = self.mirror();
        if m.axes != Axes::Off && m.at.is_none() && self.draw_target(p).0 == self.session.page {
            self.mirror_mut().at = Some(self.view_middle());
        }
    }

    /// Whether the run in flight extends a path, which the mirror leaves alone; a
    /// new run inside the edited path does not.
    fn extending(&self) -> bool {
        self.pen
            .as_ref()
            .is_some_and(|p| p.editing.as_ref().is_some_and(|e| !e.minted))
            || self.pencil.as_ref().is_some_and(|s| s.extends())
    }

    /// Paint the axes and their grab pills while the pen or the pencil is armed and
    /// mirroring, and the mirrored half of the run being drawn at 45%. Dragging a
    /// pill moves its axis; a double-click puts the axes back. Under the node tool,
    /// a mirrored path's own axes, which have no pill.
    pub(crate) fn mirror_chrome(
        &mut self,
        ui: &egui::Ui,
        painter: &egui::Painter,
        rect: egui::Rect,
        ppp: f32,
    ) {
        let from = self.mirror_grips.from.take();
        self.mirror_grips = Grips::default();
        if !matches!(
            self.tool,
            crate::tools::Tool::Pen | crate::tools::Tool::Pencil | crate::tools::Tool::Node
        ) || self.extending()
        {
            return;
        }
        let hover = ui
            .ctx()
            .pointer_hover_pos()
            .map(|p| self.to_world(p, rect, ppp));
        let Some(mirror) = self.mirror_frame(hover) else {
            return;
        };
        let ink = color::accent_400();
        let line = egui::Stroke::new(1.0, ink);
        if mirror.own {
            self.own_axes_chrome(painter, rect, ppp, &mirror, line);
            self.mirrored_run_chrome(ui, painter, rect, ppp, hover);
            return;
        }
        let (axes, at) = (mirror.axes, mirror.at);
        // Through the frame being drawn in; down the whole view past the rulers
        // otherwise, so the pill is always where the axis is seen.
        let span = self
            .axis_frame(hover, at)
            .map(|b| self.world_rect_to_screen(b, rect, ppp))
            .unwrap_or_else(|| self.canvas_inner(rect));
        let at_s = self.to_screen(at, rect, ppp);
        if axes == Axes::Centre {
            self.centre_chrome(ui, painter, rect, ppp, &mirror, span, from);
            self.mirrored_run_chrome(ui, painter, rect, ppp, hover);
            return;
        }
        let mut pills = Vec::new();
        if axes.vertical() {
            let x = at_s.x;
            painter.extend(egui::Shape::dashed_line(
                &[egui::pos2(x, span.top()), egui::pos2(x, span.bottom())],
                line,
                6.0,
                4.0,
            ));
            pills.push((
                true,
                egui::Rect::from_center_size(egui::pos2(x, span.top() + 2.0 + PILL.y / 2.0), PILL),
            ));
        }
        if axes.horizontal() {
            let y = at_s.y;
            painter.extend(egui::Shape::dashed_line(
                &[egui::pos2(span.left(), y), egui::pos2(span.right(), y)],
                line,
                6.0,
                4.0,
            ));
            let r = egui::Rect::from_center_size(
                egui::pos2(span.left() + 2.0 + PILL.x / 2.0, y),
                egui::vec2(PILL.y, PILL.x),
            );
            pills.push((false, r));
        }
        for (vertical, r) in pills {
            painter.rect_filled(r, egui::CornerRadius::same(3), ink);
            let id = ui.id().with(("mirror-axis", vertical));
            let hit = r.expand(3.0);
            self.mirror_grips.pills.push((Grip::Axis(vertical), hit));
            let resp = ui
                .interact(hit, id, egui::Sense::click_and_drag())
                .on_hover_cursor(match vertical {
                    true => egui::CursorIcon::ResizeHorizontal,
                    false => egui::CursorIcon::ResizeVertical,
                })
                .on_hover_text("Drag to move the axis · double-click to put it back");
            if primary_grab(ui, &resp) {
                self.mirror_grips.held = Some(Grip::Axis(vertical));
                self.mirror_grips.from = Some(from.unwrap_or(self.mirror().at));
            }
            if ui::double_clicked(ui, &resp) {
                self.mirror_mut().at = None;
            } else if resp.dragged_by(egui::PointerButton::Primary)
                && let Some(p) = resp.interact_pointer_pos()
            {
                let w = self.to_world(p, rect, ppp);
                let mut moved = self.mirror().at.unwrap_or(at);
                match vertical {
                    true => moved.x = w.x,
                    false => moved.y = w.y,
                }
                self.mirror_mut().at = Some(moved);
            }
        }
        self.mirrored_run_chrome(ui, painter, rect, ppp, hover);
    }

    /// A mirrored path's own axes while the node tool edits it: dashed through its
    /// box a little past each end, turned with the layer. They are where the
    /// points put them, so they have no grab pill.
    fn own_axes_chrome(
        &self,
        painter: &egui::Painter,
        rect: egui::Rect,
        ppp: f32,
        mirror: &Frame,
        line: egui::Stroke,
    ) {
        let Some(id) = self
            .pen
            .as_ref()
            .and_then(|p| p.editing.as_ref().map(|e| e.id))
            .or_else(|| self.edited_path())
        else {
            return;
        };
        let Some(NodeKind::Path { path, .. }) =
            self.session.resolved.used_kind(&self.session.doc, id)
        else {
            return;
        };
        let b = path.control_box();
        // Past the box by a fixed screen distance, whatever the zoom.
        let over = OWN_AXIS_OVERHANG / self.points_per_world() / {
            mirror.to_world.determinant().abs().sqrt().max(1e-9)
        };
        let ends =
            |a: Point, b: Point| [a, b].map(|p| self.to_screen(mirror.to_world * p, rect, ppp));
        if mirror.axes == Axes::Centre {
            // Each line from the centre to the farthest corner of the box, and past.
            let c = mirror.at;
            let reach = [
                Point::new(b.x0, b.y0),
                Point::new(b.x1, b.y0),
                Point::new(b.x0, b.y1),
                Point::new(b.x1, b.y1),
            ]
            .iter()
            .map(|p| (*p - c).hypot())
            .fold(0.0, f64::max)
                + over;
            for (angle, mid) in slice_lines(mirror) {
                let d = mastik_core::kurbo::Vec2::new(angle.cos(), angle.sin());
                let [a, z] = ends(c, c + d * reach);
                let (on, off) = if mid { (1.0, 4.0) } else { (6.0, 4.0) };
                painter.extend(egui::Shape::dashed_line(&[a, z], line, on, off));
            }
            return;
        }
        if mirror.axes.vertical() {
            let x = mirror.at.x;
            let [a, z] = ends(Point::new(x, b.y0 - over), Point::new(x, b.y1 + over));
            painter.extend(egui::Shape::dashed_line(&[a, z], line, 6.0, 4.0));
        }
        if mirror.axes.horizontal() {
            let y = mirror.at.y;
            let [a, z] = ends(Point::new(b.x0 - over, y), Point::new(b.x1 + over, y));
            painter.extend(egui::Shape::dashed_line(&[a, z], line, 6.0, 4.0));
        }
    }

    /// The switch's centre while drawing around it: a dashed slice line from the
    /// centre to the edge of `span` (screen) per copy, and the centre's grip.
    /// Dragging the grip moves the centre. Under the pen a click puts a point on
    /// the centre and a double-click finishes the run, as anywhere else on the
    /// canvas; otherwise a double-click puts the centre back.
    #[allow(clippy::too_many_arguments)]
    fn centre_chrome(
        &mut self,
        ui: &egui::Ui,
        painter: &egui::Painter,
        rect: egui::Rect,
        ppp: f32,
        frame: &Frame,
        span: egui::Rect,
        from: Option<Option<Point>>,
    ) {
        let ink = color::accent_400();
        let line = egui::Stroke::new(1.0, ink);
        let c = self.to_screen(frame.at, rect, ppp);
        for (angle, mid) in slice_lines(frame) {
            let d = egui::vec2(angle.cos() as f32, angle.sin() as f32);
            // Where the ray leaves `span`: the nearest of the sides it heads for.
            let side = |p: f32, v: f32, lo: f32, hi: f32| match v {
                v if v > 1e-6 => (hi - p) / v,
                v if v < -1e-6 => (lo - p) / v,
                _ => f32::INFINITY,
            };
            let t = side(c.x, d.x, span.left(), span.right()).min(side(
                c.y,
                d.y,
                span.top(),
                span.bottom(),
            ));
            if !t.is_finite() || t <= 0.0 {
                continue;
            }
            let (on, off) = if mid { (1.0, 4.0) } else { (6.0, 4.0) };
            painter.extend(egui::Shape::dashed_line(&[c, c + d * t], line, on, off));
        }
        // The grip: an accent disc ringed and dotted in the canvas's handle white.
        painter.circle(
            c,
            CENTRE_GRIP / 2.0,
            color::accent(),
            egui::Stroke::new(1.5, color::POINT),
        );
        painter.circle_filled(c, 1.5, color::POINT);
        let hit = egui::Rect::from_center_size(c, egui::vec2(CENTRE_GRIP, CENTRE_GRIP)).expand(3.0);
        self.mirror_grips.pills.push((Grip::Centre, hit));
        // The pen places on a click; Ctrl holding the run's points places none.
        let ctrl = ui.input(|i| i.modifiers.ctrl || i.modifiers.command);
        let pen = (self.tool == crate::tools::Tool::Pen || self.pen.is_some())
            && !self.pen_holds_points(ctrl);
        let tip = match pen {
            true => "Click to put a point on the centre · drag to move the centre",
            false => "Drag to move the centre · double-click to put it back",
        };
        let resp = ui
            .interact(
                hit,
                ui.id().with("mirror-centre"),
                egui::Sense::click_and_drag(),
            )
            .on_hover_cursor(egui::CursorIcon::Move)
            .on_hover_text(tip);
        if primary_grab(ui, &resp) {
            self.mirror_grips.held = Some(Grip::Centre);
            self.mirror_grips.from = Some(from.unwrap_or(self.mirror().at));
        }
        if ui::double_clicked(ui, &resp) {
            if pen {
                // The pen's double-click: its second click planted a duplicate.
                if let Some(run) = self.pen.as_mut()
                    && run.anchors.len() > 1
                {
                    run.anchors.pop();
                }
                self.finish_pen(false);
            } else {
                self.mirror_mut().at = None;
            }
        } else if pen && resp.clicked_by(egui::PointerButton::Primary) {
            let centre = frame.to_world * frame.at;
            self.pen_press_exact(centre, false, Some(centre));
            // The press is already released, so no handle is being pulled.
            if let Some(run) = self.pen.as_mut() {
                run.shaping = false;
            }
        } else if resp.dragged_by(egui::PointerButton::Primary)
            && let Some(p) = resp.interact_pointer_pos()
        {
            self.mirror_mut().at = Some(self.to_world(p, rect, ppp));
        }
    }

    /// The other half of the run being drawn, at 45%, and the *On axis* mark where
    /// the next point would land on an axis.
    fn mirrored_run_chrome(
        &self,
        ui: &egui::Ui,
        painter: &egui::Painter,
        rect: egui::Rect,
        ppp: f32,
        hover: Option<Point>,
    ) {
        // The other half of the pen's run, as it will land.
        if let Some(pen) = self
            .pen
            .as_ref()
            .filter(|p| p.editing.as_ref().is_none_or(|e| e.minted))
            && pen.anchors.len() >= 2
        {
            let drawn = PenSubpath {
                anchors: pen.anchors.clone(),
                closed: false,
            };
            let ghost = egui::Stroke::new(
                ((crate::canvas::PEN_STROKE_WIDTH * self.session.camera.zoom) as f32 / ppp)
                    .max(1.0),
                ui::color_to_egui(crate::canvas::PEN_STROKE_COLOR).gamma_multiply(0.45),
            );
            // The drawn half is in the run too, under the pen's own full stroke.
            for sub in &self.mirror_run(drawn) {
                let outline = crate::tools::pen_outline(&sub.anchors, sub.closed);
                for line in self.flatten_to_screen(&outline, rect, ppp) {
                    painter.add(line.stroked(ghost));
                }
            }
        }
        // And the pencil's: its trail as the mirror makes it, joined where it will
        // join, in the trail's own line at 45%. The fit happens on release.
        if let Some(stroke) = self.pencil.as_ref().filter(|s| !s.extends())
            && stroke.samples().len() >= 2
        {
            let drawn = PenSubpath {
                anchors: stroke
                    .samples()
                    .iter()
                    .map(|p| crate::preview::PenAnchor::corner(*p))
                    .collect(),
                closed: false,
            };
            let ghost = egui::Stroke::new(1.0, color::SELECT.gamma_multiply(0.45));
            for sub in &self.mirror_run(drawn) {
                let mut pts: Vec<egui::Pos2> = sub
                    .anchors
                    .iter()
                    .map(|a| self.to_screen(a.at, rect, ppp))
                    .collect();
                if sub.closed
                    && let Some(first) = pts.first().copied()
                {
                    pts.push(first);
                }
                painter.add(egui::Shape::line(pts, ghost));
            }
        }
        // Where the next point would land on an axis: a ring there and *On axis*,
        // in the snaps' red. The node tool places points only with its pen, nothing
        // places one on a pill, which moves the axis instead, and no press places one
        // while Ctrl holds the run's points.
        let (shift, ctrl) =
            ui.input(|i| (i.modifiers.shift, i.modifiers.ctrl || i.modifiers.command));
        let placing =
            (self.tool != crate::tools::Tool::Node || self.pen.is_some() || self.pen_bias)
                && self.mirror_grip(ui.ctx().pointer_hover_pos()).is_none()
                && !((self.tool == crate::tools::Tool::Pen || self.pen.is_some())
                    && self.pen_holds_points(ctrl))
                && !(self.pen_bias && ctrl);
        if placing && let Some(on) = hover.and_then(|w| self.lands_on_axis(w, shift)) {
            let at = self.to_screen(on, rect, ppp);
            let centre = self.mirror_frame(Some(on)).is_some_and(|f| {
                f.axes == Axes::Centre
                    && (f.to_world * f.at - on).hypot() <= 1e-9 * (1.0 + on.to_vec2().hypot())
            });
            let name = if centre { "On centre" } else { "On axis" };
            crate::canvas::draw_snap_target(painter, at, name, false);
        } else if (self.tool == crate::tools::Tool::Pen || self.pen.is_some())
            && !self.pen_holds_points(ctrl)
            && self.mirror_grips.held.is_none()
            && self.mirror_grip(ui.ctx().pointer_hover_pos()) == Some(Grip::Centre)
            && let Some(f) = self.mirror_frame(None).filter(|f| f.axes == Axes::Centre)
        {
            // The pen's click on the centre's grip puts its point there.
            let at = self.to_screen(f.to_world * f.at, rect, ppp);
            crate::canvas::draw_snap_target(painter, at, "On centre", false);
        }
    }

    /// Where a point placed at `world` lands on a mirror axis, if it does: the pen's
    /// next anchor (snapped as its press would be), or where the pencil's stroke
    /// would end, pulled onto the axis by the commit.
    pub(crate) fn lands_on_axis(&self, world: Point, shift: bool) -> Option<Point> {
        let at = match self.tool {
            crate::tools::Tool::Pen | crate::tools::Tool::Node => self.pen_point(world, shift).0,
            _ => world,
        };
        let (on, hit) = self.snap_to_axis(at);
        hit.then_some(on)
    }

    /// The path bar's mirror button's flyout, the drawing bar's or the edit
    /// bar's, over the switch it shows ([`Self::mirror`]): the axes, ticked like
    /// the View menu's rows, and *Reset axis*.
    pub(crate) fn mirror_flyout(&mut self, ctx: &egui::Context, button: egui::Rect) {
        let id = egui::Id::new("mirror-flyout");
        let m = self.mirror();
        let current = m.axes;
        let centre = current == Axes::Centre;
        let mut pick = None;
        let mut reset = false;
        let mut copies = f64::from(m.copies);
        let mut scrubbed = false;
        // The five choices, the copies field (a labelled control, about two rows)
        // and *Reset*.
        let rows = Axes::ALL.len() + 2 + 1;
        let card = ui::flyout(id, button, 200.0)
            .order(egui::Order::Foreground)
            .show(ctx, (rows, 1), |ui| {
                for axes in Axes::ALL {
                    if ui::menu_check(ui, axes.label(), axes == current, ui::MENU_ROW_H) {
                        pick = Some(axes);
                    }
                }
                // Under *Around a centre*, indented to its label and set off from
                // the rows by an eyebrow's gap; disabled under the others, so the
                // flyout keeps one height.
                ui.add_space(ui::SECTION_LABEL_GAP);
                ui.horizontal(|ui| {
                    ui.add_space(ui::MENU_LABEL_X);
                    ui.vertical(|ui| {
                        ui::disable_unless(ui, centre, |ui| {
                            ui::labelled(ui, "Copies", |ui| {
                                let w = ui.available_width() - ui::MENU_ROW_INSET;
                                let size = egui::vec2(w, ui::CONTROL_H);
                                let resp = ui::value_field(
                                    ui,
                                    size,
                                    ui::Prefix::Icon(icon::RADIAL_SYMMETRY),
                                    &mut copies,
                                    ui::Scrub::whole(0.1).range(
                                        f64::from(mastik_core::symmetry::MIN_COPIES)
                                            ..=f64::from(mastik_core::symmetry::MAX_COPIES),
                                    ),
                                    |d| d.custom_formatter(ui::number(0)),
                                )
                                .on_hover_text(
                                    "How many copies turn about the centre, the drawn one counted",
                                )
                                .on_disabled_hover_text("Pick Around a centre to set its copies");
                                scrubbed = resp.dragged();
                            });
                        });
                    });
                });
                ui::menu_sep(ui);
                let placed = self.mirror().at.is_some();
                let (word, put) = match centre {
                    true => ("Reset centre", "The centre is where it starts"),
                    false => ("Reset axis", "The axis is where it starts"),
                };
                let row = ui::MenuRow {
                    enabled: placed,
                    ..ui::MenuRow::new("", word)
                };
                let resp = ui::menu_row(ui, row, ui::MENU_ROW_H);
                let resp = match placed {
                    true => resp,
                    false => resp.on_hover_text(put),
                };
                reset = placed && resp.clicked();
            });
        let copies = (copies.round() as u8).clamp(
            mastik_core::symmetry::MIN_COPIES,
            mastik_core::symmetry::MAX_COPIES,
        );
        if centre && copies != m.copies {
            self.mirror_mut().copies = copies;
        }
        // Armed after a right-click's cancel is answered, so the frame it lands on
        // cannot re-arm it.
        if scrubbed && !self.gesture_cancelled && self.in_flight.mirror_copies_scrub.is_none() {
            self.in_flight.mirror_copies_scrub = Some(m.copies);
        }
        if let Some(axes) = pick {
            self.mirror_mut().axes = axes;
            self.mirror_open = false;
        }
        if reset {
            self.mirror_mut().at = None;
            self.mirror_open = false;
        }
        // A right-click that called off a scrub is spent on it.
        let pressed_elsewhere = ctx.input(|i| i.pointer.any_pressed())
            && !self.gesture_cancelled
            && !card.contains_pointer()
            && !ctx
                .pointer_interact_pos()
                .is_some_and(|p| button.contains(p));
        if pressed_elsewhere {
            self.mirror_open = false;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::preview::PenAnchor;
    use mastik_core::kurbo::Vec2;

    fn run(points: &[(f64, f64)]) -> PenSubpath {
        PenSubpath {
            anchors: points
                .iter()
                .map(|&(x, y)| PenAnchor::corner(Point::new(x, y)))
                .collect(),
            closed: false,
        }
    }

    fn ats(s: &PenSubpath) -> Vec<(f64, f64)> {
        s.anchors.iter().map(|a| (a.at.x, a.at.y)).collect()
    }

    /// Starting and ending on the axis closes one symmetric path, the joins
    /// counted once.
    #[test]
    fn both_ends_on_the_axis_close_one_path() {
        let got = mirrored(
            run(&[(100.0, 10.0), (60.0, 50.0), (100.0, 90.0)]),
            Axes::Vertical,
            Point::new(100.0, 0.0),
            1.0,
        );
        assert_eq!(got.len(), 1);
        assert!(got[0].closed);
        assert_eq!(
            ats(&got[0]),
            [(100.0, 10.0), (60.0, 50.0), (100.0, 90.0), (140.0, 50.0)]
        );
    }

    /// One end on the axis joins the halves there into one open path; an end
    /// near enough is pulled onto it.
    #[test]
    fn one_end_on_the_axis_joins_one_open_path() {
        let got = mirrored(
            run(&[(60.0, 10.0), (99.5, 50.0)]),
            Axes::Vertical,
            Point::new(100.0, 0.0),
            1.0,
        );
        assert_eq!(got.len(), 1);
        assert!(!got[0].closed);
        assert_eq!(ats(&got[0]), [(60.0, 10.0), (100.0, 50.0), (140.0, 10.0)]);
        let got = mirrored(
            run(&[(100.0, 50.0), (60.0, 10.0)]),
            Axes::Vertical,
            Point::new(100.0, 0.0),
            1.0,
        );
        assert_eq!(ats(&got[0]), [(140.0, 10.0), (100.0, 50.0), (60.0, 10.0)]);
    }

    /// Neither end on it leaves two subpaths in the one layer.
    #[test]
    fn neither_end_on_the_axis_leaves_two_subpaths() {
        let got = mirrored(
            run(&[(60.0, 10.0), (80.0, 50.0)]),
            Axes::Vertical,
            Point::new(100.0, 0.0),
            1.0,
        );
        assert_eq!(got.len(), 2);
        assert_eq!(
            ats(&got[1]),
            [(120.0, 50.0), (140.0, 10.0)],
            "run backwards"
        );
    }

    /// A closed run crossing the axis gets an image wound its way, so a fill
    /// covers the overlap; under Both axes a half the vertical rule already closed
    /// is its own image and comes back once.
    /// flip: `across` pushing the closed image unreversed → fails at "the overlap".
    #[test]
    fn a_closed_run_and_its_image_wind_alike() {
        use mastik_core::kurbo::Shape as _;
        let mut square = run(&[(80.0, 0.0), (130.0, 0.0), (130.0, 40.0), (80.0, 40.0)]);
        square.closed = true;
        let got = mirrored(square, Axes::Vertical, Point::new(100.0, 0.0), 1.0);
        let path = mastik_core::edit::pen_path(&got);
        assert_eq!(path.winding(Point::new(125.0, 20.0)).abs(), 1, "once");
        assert_eq!(
            path.winding(Point::new(100.0, 20.0)).abs(),
            2,
            "the overlap"
        );

        let half = run(&[(100.0, 60.0), (70.0, 100.0), (100.0, 140.0)]);
        let got = mirrored(half, Axes::Both, Point::new(100.0, 100.0), 1.0);
        assert_eq!(got.len(), 1, "no coincident twin");
        let path = mastik_core::edit::pen_path(&got);
        assert_ne!(path.winding(Point::new(100.0, 100.0)), 0);
    }

    /// A closed run only roughly its own image, each anchor's image within the
    /// snapping reach of one of its anchors, still gets its image: only an exact
    /// one adds nothing.
    /// flip: `across` asking `own` within `tol` → fails at "an image".
    #[test]
    fn a_roughly_symmetric_closed_run_gets_its_image() {
        let mut kite = run(&[(100.0, 0.0), (130.0, 30.0), (100.0, 60.0), (75.0, 30.0)]);
        kite.closed = true;
        let got = mirrored(kite, Axes::Vertical, Point::new(100.0, 0.0), 8.0);
        assert_eq!(got.len(), 2, "an image");
        let image = ats(&got[1]);
        assert!(
            image.contains(&(70.0, 30.0)) && image.contains(&(125.0, 30.0)),
            "{image:?}"
        );
    }

    /// Both axes: a quarter drawn from one axis to the other closes into a whole.
    #[test]
    fn a_quarter_from_axis_to_axis_closes_under_both() {
        let got = mirrored(
            run(&[(100.0, 60.0), (70.0, 70.0), (60.0, 100.0)]),
            Axes::Both,
            Point::new(100.0, 100.0),
            1.0,
        );
        assert_eq!(got.len(), 1);
        assert!(got[0].closed);
        assert_eq!(got[0].anchors.len(), 8);
    }

    /// A curve's handle at the join leaves on the mirror of the one arriving, so
    /// the joined path is as smooth across the axis as it was drawn.
    #[test]
    fn the_join_carries_the_mirrored_handle() {
        let mut s = run(&[(60.0, 10.0), (100.0, 50.0)]);
        s.anchors[1].back = mastik_core::kurbo::Vec2::new(-10.0, -4.0);
        let got = mirrored(s, Axes::Vertical, Point::new(100.0, 0.0), 1.0);
        let join = got[0].anchors[1];
        assert_eq!(join.back, mastik_core::kurbo::Vec2::new(-10.0, -4.0));
        assert_eq!(join.out, mastik_core::kurbo::Vec2::new(10.0, -4.0));
    }

    /// A headless app with the pen armed and the vertical axis at x = 100, the view
    /// at 100% centred on the origin.
    fn mirroring_pen(ctx: &egui::Context) -> MastikApp {
        let mut app = MastikApp::headless(ctx);
        let mut ids = mastik_core::IdSource::new(1);
        app.session
            .adopt_document(mastik_core::Document::new(ids.mint()), None);
        app.canvas_px = (800, 600);
        app.session.camera.center = Point::ZERO;
        app.session.camera.zoom = 1.0;
        app.choose_tool(crate::tools::Tool::Pen);
        app.session.mirror = Mirror {
            axes: Axes::Vertical,
            at: Some(Point::new(100.0, 0.0)),
            ..Mirror::default()
        };
        app
    }

    /// A point within `ON_AXIS_PT` of the axis lands on it; one further off, or
    /// with the mirror off, lands nowhere the chip names.
    #[test]
    fn the_chip_names_a_point_that_lands_on_the_axis() {
        let ctx = egui::Context::default();
        let mut app = mirroring_pen(&ctx);
        assert_eq!(
            app.lands_on_axis(Point::new(104.0, 50.0), false),
            Some(Point::new(100.0, 50.0))
        );
        assert_eq!(app.lands_on_axis(Point::new(120.0, 50.0), false), None);
        app.session.mirror.axes = Axes::Off;
        assert_eq!(app.lands_on_axis(Point::new(104.0, 50.0), false), None);
    }

    /// With the pointer by the axis the canvas draws *On axis*; away from it, not.
    ///
    /// flip: `mirror_chrome` skipping the chip → fails at the first assertion.
    #[test]
    fn the_on_axis_chip_is_painted_by_the_axis_only() {
        let ctx = egui::Context::default();
        crate::theme::install(&ctx);
        let mut app = mirroring_pen(&ctx);
        let rect = egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(800.0, 600.0));
        let says_on_axis = |app: &mut MastikApp, x: f32| {
            let pointer = app.to_screen(Point::new(f64::from(x), 50.0), rect, 1.0);
            let mut out = None;
            for _ in 0..2 {
                let input = egui::RawInput {
                    screen_rect: Some(rect),
                    events: vec![egui::Event::PointerMoved(pointer)],
                    ..Default::default()
                };
                out = Some(ctx.run_ui(input, |ui| {
                    let painter = ui.painter().clone();
                    app.mirror_chrome(ui, &painter, rect, 1.0);
                }));
            }
            out.expect("ran")
                .shapes
                .iter()
                .any(|c| matches!(&c.shape, egui::Shape::Text(t) if t.galley.text() == "On axis"))
        };
        assert!(says_on_axis(&mut app, 103.0), "by the axis");
        assert!(!says_on_axis(&mut app, 160.0), "away from it");
    }

    /// On open canvas the axes default to the view's middle, and a run fixes them
    /// there, so a pan mid-run does not move them under the anchors.
    ///
    /// flip: `pin_mirror_axes` doing nothing → fails at the second assertion.
    #[test]
    fn a_run_on_open_canvas_pins_the_axes_where_the_view_was() {
        let ctx = egui::Context::default();
        let mut app = MastikApp::headless(&ctx);
        let mut ids = mastik_core::IdSource::new(1);
        app.session
            .adopt_document(mastik_core::Document::new(ids.mint()), None);
        app.session.mirror.axes = Axes::Vertical;
        let was = app.session.camera.center;
        assert_eq!(app.mirror_axes(None).map(|(_, at)| at), Some(was));
        app.pin_mirror_axes(Point::new(-5000.0, 0.0));
        app.session.camera.center = Point::new(was.x + 300.0, was.y);
        assert_eq!(app.mirror_axes(None).map(|(_, at)| at), Some(was));
    }

    /// Around a centre, five copies: a petal from the centre out and back lands
    /// as one path of five closed petals, saved as turning five times; dragging
    /// one anchor of one petal under the node tool moves the same anchor of every
    /// petal, so the rule still holds.
    ///
    /// flip: `local_mirror` answering `NONE` for a centre → fails at "saved
    /// around"; `symmetric_edit` without its radial arm → fails at "still five".
    #[test]
    fn the_pen_lands_a_radial_run_that_point_editing_keeps() {
        let ctx = egui::Context::default();
        let mut app = mirroring_pen(&ctx);
        app.session.mirror = Mirror {
            axes: Axes::Centre,
            at: Some(Point::new(100.0, 100.0)),
            copies: 5,
        };
        let page = app.session.page;
        app.pen = Some(crate::preview::PenState {
            parent: page,
            parent_world: Affine::IDENTITY,
            anchors: [(100.0, 100.0), (112.0, 70.0), (100.5, 99.6)]
                .map(|(x, y)| PenAnchor::corner(Point::new(x, y)))
                .to_vec(),
            shaping: false,
            editing: None,
        });
        app.finish_pen(false);
        let id = app.session.selection.single().expect("the path");
        let around = PathMirror::around(5, false);
        assert_eq!(mirror_of(&app, id), around, "saved around");
        let Some(NodeKind::Path { path, .. }) = app.session.doc.get(id).map(|n| n.kind()) else {
            panic!("a path");
        };
        let base = crate::tools::pen_anchors(path, &[]);
        assert_eq!(base.len(), 5, "five petals");
        assert!(base.iter().all(|s| s.closed));

        let world = app.session.resolved.world_transform(id).unwrap();
        let mut edited = base.clone();
        edited[0].anchors[1].at += mastik_core::kurbo::Vec2::new(6.0, -3.0);
        let tx = app.rewrite_points(id, world, &base, edited);
        assert!(app.commit_edit(tx));
        assert_eq!(mirror_of(&app, id), around, "still five");
    }

    /// The flyout is one height whichever choice is ticked, the centre's rows
    /// standing disabled under the others; the bar's button wears the petals
    /// under a centre.
    ///
    /// flip: `mirror_flyout` drawing the centre's rows only under a centre →
    /// fails at "one height".
    #[test]
    fn the_flyout_keeps_one_height_and_the_button_its_glyph() {
        use crate::repeat::tests::pass;
        let ctx = egui::Context::default();
        let mut app = mirroring_pen(&ctx);
        app.mirror_open = true;
        let height = |app: &mut MastikApp| {
            for _ in 0..4 {
                let _ = pass(&ctx, app, Vec::new());
            }
            ctx.memory(|m| m.area_rect(egui::Id::new("mirror-flyout")))
                .expect("the flyout is up")
                .height()
        };
        app.session.mirror.axes = Axes::Off;
        let off = height(&mut app);
        assert_eq!(app.symmetry_glyph(), icon::FLIP_HORIZONTAL);
        app.session.mirror.axes = Axes::Centre;
        let centre = height(&mut app);
        assert!((off - centre).abs() < 0.5, "one height: {off} vs {centre}");
        assert_eq!(app.symmetry_glyph(), icon::RADIAL_SYMMETRY);
    }

    /// A right-click mid-scrub of the flyout's copies puts the count back and
    /// leaves the flyout open, with the pointer on the flyout or out over the
    /// canvas.
    ///
    /// flip: `cancel_gesture` not restoring `mirror_copies_scrub` → fails at "the
    /// copies are back"; `mirror_flyout`'s press-elsewhere check without its
    /// `gesture_cancelled` → fails at "still open".
    #[test]
    fn a_right_click_winds_a_copies_scrub_back() {
        use crate::repeat::tests::scrub_then_right_click;
        for outside in [false, true] {
            let ctx = egui::Context::default();
            let mut app = mirroring_pen(&ctx);
            app.session.mirror.axes = Axes::Centre;
            app.mirror_open = true;
            scrub_then_right_click(
                &ctx,
                &mut app,
                egui::Id::new("mirror-flyout"),
                "Copies",
                |at| match outside {
                    true => egui::pos2(700.0, 500.0),
                    false => at,
                },
                |app| {
                    assert_ne!(app.mirror().copies, START_COPIES, "the drag scrubbed it");
                    assert!(
                        app.in_flight.mirror_copies_scrub.is_some(),
                        "the scrub is armed"
                    );
                },
            );
            assert!(app.mirror_open, "still open ({outside})");
            assert_eq!(
                app.mirror().copies,
                START_COPIES,
                "the copies are back ({outside})"
            );
        }
    }

    /// The pen's finish goes through the mirror: a run from the axis and back to it
    /// lands as one closed path, in one step.
    ///
    /// flip: `finish_pen` creating from the pen's anchors alone → fails at the closed assertion.
    #[test]
    fn the_pen_lands_the_mirrored_run_as_one_path() {
        let ctx = egui::Context::default();
        let mut app = MastikApp::headless(&ctx);
        let mut ids = mastik_core::IdSource::new(1);
        app.session
            .adopt_document(mastik_core::Document::new(ids.mint()), None);
        app.canvas_px = (800, 600);
        app.choose_tool(crate::tools::Tool::Pen);
        app.session.mirror = Mirror {
            axes: Axes::Vertical,
            at: Some(Point::new(100.0, 0.0)),
            ..Mirror::default()
        };
        let page = app.session.page;
        app.pen = Some(crate::preview::PenState {
            parent: page,
            parent_world: Affine::IDENTITY,
            anchors: [(100.0, 10.0), (60.0, 50.0), (100.0, 90.0)]
                .map(|(x, y)| PenAnchor::corner(Point::new(x, y)))
                .to_vec(),
            shaping: false,
            editing: None,
        });
        let depth = app.session.history.undo_depth();
        app.finish_pen(false);
        assert_eq!(app.session.history.undo_depth(), depth + 1);
        let id = app
            .session
            .selection
            .single()
            .expect("the path is selected");
        let Some(mastik_core::NodeKind::Path { path, .. }) =
            app.session.doc.get(id).map(|n| n.kind())
        else {
            panic!("a path");
        };
        let subs = crate::tools::pen_anchors(path, &[]);
        assert_eq!(subs.len(), 1, "one subpath");
        assert!(subs[0].closed, "closed on the axis");
        assert_eq!(subs[0].anchors.len(), 4);
    }

    /// A headless app holding a diamond the pen drew mirrored about x = 100 —
    /// top (100, 10), left (60, 50), bottom (100, 90), right (140, 50) — under
    /// the node tool.
    fn mirrored_diamond(ctx: &egui::Context) -> (MastikApp, NodeId) {
        let mut app = MastikApp::headless(ctx);
        let mut ids = mastik_core::IdSource::new(1);
        app.session
            .adopt_document(mastik_core::Document::new(ids.mint()), None);
        app.canvas_px = (800, 600);
        app.session.camera.center = Point::new(100.0, 50.0);
        app.session.camera.zoom = 1.0;
        app.choose_tool(crate::tools::Tool::Pen);
        app.session.mirror = Mirror {
            axes: Axes::Vertical,
            at: Some(Point::new(100.0, 0.0)),
            ..Mirror::default()
        };
        let page = app.session.page;
        app.pen = Some(crate::preview::PenState {
            parent: page,
            parent_world: Affine::IDENTITY,
            anchors: [(100.0, 10.0), (60.0, 50.0), (100.0, 90.0)]
                .map(|(x, y)| PenAnchor::corner(Point::new(x, y)))
                .to_vec(),
            shaping: false,
            editing: None,
        });
        app.finish_pen(false);
        let id = app.session.selection.single().expect("the path");
        app.session.mirror.axes = Axes::Off;
        app.choose_tool(crate::tools::Tool::Node);
        app.session.selection.set_one(id);
        (app, id)
    }

    /// The anchors of `id`, in world space, in order.
    fn anchors(app: &MastikApp, id: NodeId) -> Vec<Point> {
        let Some(NodeKind::Path {
            path, corner_radii, ..
        }) = app.session.doc.get(id).map(|n| n.kind())
        else {
            panic!("a path");
        };
        let world = app.session.resolved.world_transform(id).expect("placed");
        crate::tools::pen_anchors(path, corner_radii)
            .iter()
            .flat_map(|s| s.anchors.iter().map(|a| world * a.at))
            .collect()
    }

    fn mirror_of(app: &MastikApp, id: NodeId) -> PathMirror {
        match app.session.doc.get(id).map(|n| n.kind()) {
            Some(NodeKind::Path { mirror, .. }) => *mirror,
            _ => panic!("a path"),
        }
    }

    /// Where `p` sits among `id`'s anchors.
    fn index_of(app: &MastikApp, id: NodeId, p: Point) -> PointRef {
        let i = anchors(app, id)
            .iter()
            .position(|a| (*a - p).hypot() < 1e-6)
            .unwrap_or_else(|| panic!("an anchor at {p:?}"));
        (0, i)
    }

    fn has(app: &MastikApp, id: NodeId, p: Point) -> bool {
        anchors(app, id).iter().any(|a| (*a - p).hypot() < 1e-6)
    }

    /// A path drawn mirrored is saved mirrored, and its size badge says so.
    /// flip: `finish_pen` without `mark_mirrored` → fails at the first assertion.
    #[test]
    fn a_path_drawn_mirrored_is_saved_mirrored_and_badged() {
        let ctx = egui::Context::default();
        let (app, id) = mirrored_diamond(&ctx);
        assert!(mirror_of(&app, id).vertical);
        let marks = crate::canvas::size_badge_marks(&app.session.doc, &[id]);
        assert_eq!(
            marks,
            [crate::theme::icon::FLIP_HORIZONTAL],
            "the size badge"
        );
    }

    /// Dragging the left anchor of a mirrored path takes the right one with it,
    /// mirrored, live and committed; the path stays mirrored, and undo is one step.
    /// flip: `points_tx` writing through `tools::rewrite_path` → fails at "the image followed".
    #[test]
    fn a_point_drag_on_a_mirrored_path_mirrors_onto_the_image() {
        let ctx = egui::Context::default();
        let (mut app, id) = mirrored_diamond(&ctx);
        let left = index_of(&app, id, Point::new(60.0, 50.0));
        app.points.set_one(id, left);
        let tx = app.points_tx(id, Vec2::new(-10.0, 5.0));
        assert!(app.session.commit(tx));
        assert!(has(&app, id, Point::new(50.0, 55.0)));
        assert!(has(&app, id, Point::new(150.0, 55.0)), "the image followed");
        assert!(mirror_of(&app, id).vertical, "still mirrored");
        app.undo();
        assert!(has(&app, id, Point::new(140.0, 50.0)));
    }

    /// The top anchor sits on the axis: dragged sideways, it only slides along it.
    #[test]
    fn an_axis_point_is_locked_to_the_axis() {
        let ctx = egui::Context::default();
        let (mut app, id) = mirrored_diamond(&ctx);
        let top = index_of(&app, id, Point::new(100.0, 10.0));
        app.points.set_one(id, top);
        let tx = app.points_tx(id, Vec2::new(25.0, -4.0));
        assert!(app.session.commit(tx));
        assert!(has(&app, id, Point::new(100.0, 6.0)), "slid along the axis");
        assert!(mirror_of(&app, id).vertical);
    }

    /// Deleting a point deletes its image, and the path stays mirrored.
    /// flip: `delete_points` without `with_mirrored_points` → fails at "the image went".
    #[test]
    fn deleting_a_point_deletes_its_image() {
        let ctx = egui::Context::default();
        let (mut app, id) = mirrored_diamond(&ctx);
        let left = index_of(&app, id, Point::new(60.0, 50.0));
        app.points.set_one(id, left);
        app.delete_points();
        assert!(!has(&app, id, Point::new(140.0, 50.0)), "the image went");
        assert_eq!(anchors(&app, id).len(), 2);
    }

    /// A point added on one side is added on the other, and the new one is the
    /// one selected.
    /// flip: `insert_point` inserting only `(at, t)` → fails at the anchor count.
    #[test]
    fn adding_a_point_adds_its_image() {
        let ctx = egui::Context::default();
        let (mut app, id) = mirrored_diamond(&ctx);
        // On the top-left edge, a quarter of the way from the top.
        app.add_point_at(Point::new(90.0, 20.0));
        assert_eq!(anchors(&app, id).len(), 6);
        assert!(has(&app, id, Point::new(90.0, 20.0)));
        assert!(has(&app, id, Point::new(110.0, 20.0)), "and its image");
        assert!(mirror_of(&app, id).vertical);
        let picked: Vec<PointRef> = app.points.on(id).collect();
        assert_eq!(picked, vec![index_of(&app, id, Point::new(90.0, 20.0))]);
    }

    /// An edit that breaks the symmetry drops the axis, and undo brings it back.
    /// flip: the inverse `GeometryPatch::Path` carrying `mirror: None` → fails at "back".
    #[test]
    fn a_lopsided_edit_drops_the_axis_and_undo_restores_it() {
        let ctx = egui::Context::default();
        let (mut app, id) = mirrored_diamond(&ctx);
        let world = app.session.resolved.world_transform(id).unwrap();
        let (_, mut subs) = {
            let Some(NodeKind::Path {
                path, corner_radii, ..
            }) = app.session.doc.get(id).map(|n| n.kind())
            else {
                panic!()
            };
            let mut s = crate::tools::pen_anchors(path, corner_radii);
            crate::tools::map_subpaths(&mut s, world);
            ((), s)
        };
        subs[0].anchors[1].at.x -= 7.0;
        let tx = crate::tools::rewrite_path(id, world, &subs, app.drawn_path(id));
        assert!(app.session.commit(tx));
        assert!(mirror_of(&app, id).is_none(), "dropped");
        app.undo();
        assert!(mirror_of(&app, id).vertical, "back");
    }

    /// The node tool draws a mirrored path's own axis through it, and a plain
    /// path's none.
    /// flip: `mirror_frame` skipping the edited path's own axes → fails at "drawn".
    #[test]
    fn the_node_tool_draws_a_mirrored_paths_axis() {
        let ctx = egui::Context::default();
        crate::theme::install(&ctx);
        let (mut app, id) = mirrored_diamond(&ctx);
        let rect = egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(800.0, 600.0));
        let axis_x = app.to_screen(Point::new(100.0, 50.0), rect, 1.0).x;
        let dashes = |app: &mut MastikApp| {
            let out = ctx.run_ui(
                egui::RawInput {
                    screen_rect: Some(rect),
                    ..Default::default()
                },
                |ui| {
                    let painter = ui.painter().clone();
                    app.mirror_chrome(ui, &painter, rect, 1.0);
                },
            );
            out.shapes
                .iter()
                .filter(|c| match &c.shape {
                    egui::Shape::LineSegment { points, .. } => {
                        points.iter().all(|p| (p.x - axis_x).abs() < 0.5)
                    }
                    _ => false,
                })
                .count()
        };
        assert!(dashes(&mut app) > 0, "drawn");
        app.flatten_selection();
        assert!(!mirror_of(&app, id).vertical);
        assert_eq!(dashes(&mut app), 0, "a plain path has none");
    }

    /// Flatten takes a lone mirrored path, and leaves it plain and where it was.
    #[test]
    fn flatten_makes_a_mirrored_path_plain() {
        let ctx = egui::Context::default();
        let (mut app, id) = mirrored_diamond(&ctx);
        app.choose_tool(crate::tools::Tool::Select);
        app.session.selection.set_one(id);
        let before = anchors(&app, id);
        app.flatten_selection();
        assert!(mirror_of(&app, id).is_none());
        assert_eq!(anchors(&app, id), before);
    }

    /// A new run drawn inside a mirrored path is mirrored across the path's own
    /// axis, whatever the drawing switch says.
    #[test]
    fn a_new_run_inside_a_mirrored_path_mirrors_across_its_axis() {
        let ctx = egui::Context::default();
        let (mut app, id) = mirrored_diamond(&ctx);
        let world = app.session.resolved.world_transform(id).unwrap();
        let Some(NodeKind::Path {
            path, corner_radii, ..
        }) = app.session.doc.get(id).map(|n| n.kind())
        else {
            panic!()
        };
        let mut subpaths = crate::tools::pen_anchors(path, corner_radii);
        crate::tools::map_subpaths(&mut subpaths, world);
        let active = subpaths.len();
        subpaths.push(PenSubpath {
            anchors: Vec::new(),
            closed: false,
        });
        app.pen = Some(crate::preview::PenState {
            parent: app.session.page,
            parent_world: Affine::IDENTITY,
            anchors: [(70.0, 40.0), (80.0, 60.0)]
                .map(|(x, y)| PenAnchor::corner(Point::new(x, y)))
                .to_vec(),
            shaping: false,
            editing: Some(crate::preview::PenEdit {
                id,
                world,
                subpaths,
                active,
                from_start: false,
                minted: true,
            }),
        });
        app.finish_pen(false);
        assert!(has(&app, id, Point::new(130.0, 40.0)));
        assert!(has(&app, id, Point::new(120.0, 60.0)));
        assert!(mirror_of(&app, id).vertical, "and stays mirrored");
    }

    /// A pen anchor pressed near a mirrored path's own axis, in a new run inside
    /// it, lands on the axis as the *On axis* ring promises, past the snap
    /// distance (7 pt off, the snaps reach 6).
    /// flip: `pen_point_except` gating the axis on a run outside any path → fails
    /// at "on the axis".
    #[test]
    fn a_run_inside_a_mirrored_path_lands_on_its_axis() {
        let ctx = egui::Context::default();
        let (mut app, id) = mirrored_diamond(&ctx);
        let world = app.session.resolved.world_transform(id).unwrap();
        app.pen = Some(crate::preview::PenState {
            parent: app.session.page,
            parent_world: Affine::IDENTITY,
            anchors: vec![PenAnchor::corner(Point::new(80.0, 30.0))],
            shaping: false,
            editing: Some(crate::preview::PenEdit {
                id,
                world,
                subpaths: vec![PenSubpath {
                    anchors: Vec::new(),
                    closed: false,
                }],
                active: 0,
                from_start: false,
                minted: true,
            }),
        });
        let (at, _) = app.pen_point(Point::new(107.0, 33.0), false);
        assert_eq!(at.x, 100.0, "on the axis: {at:?}");
    }

    /// World axes become the path's own in a parent turned a quarter turn (the
    /// vertical lands horizontal), and are dropped, said, in one turned 30°.
    /// flip: `local_mirror` keeping the world axes as they are → fails at "a
    /// quarter turn".
    #[test]
    fn world_axes_are_saved_as_the_parents_own() {
        let quarter = Affine::rotate(std::f64::consts::FRAC_PI_2);
        let (m, dropped) = local_mirror(&switch(Axes::Vertical), quarter);
        assert!(
            m.horizontal && !m.vertical && !dropped,
            "a quarter turn: {m:?}"
        );
        let (m, dropped) =
            local_mirror(&switch(Axes::Vertical), Affine::rotate(30f64.to_radians()));
        assert!(m.is_none() && dropped);
        let (m, dropped) = local_mirror(&switch(Axes::Both), Affine::translate((5.0, 5.0)));
        assert!(m.vertical && m.horizontal && !dropped);
    }

    /// The switch's frame drawing with `axes`, six copies under a centre.
    fn switch(axes: Axes) -> Frame {
        Frame {
            axes,
            at: Point::ZERO,
            to_world: Affine::IDENTITY,
            own: false,
            copies: 6,
            reflect: false,
            phase: -std::f64::consts::FRAC_PI_2,
        }
    }

    /// A centre's copies are saved wherever the parent keeps angles, turned any
    /// way or scaled evenly, and dropped, said, in a parent stretched one way.
    /// flip: `local_mirror` treating a centre as axes → fails at "turned 30°".
    #[test]
    fn a_centre_is_kept_in_a_turned_parent_and_dropped_in_a_stretched_one() {
        let centre = switch(Axes::Centre);
        let turned = Affine::rotate(30f64.to_radians()) * Affine::scale(2.0);
        let (m, dropped) = local_mirror(&centre, turned);
        assert_eq!(m, PathMirror::around(6, false), "turned 30°");
        assert!(!dropped);
        let (m, dropped) = local_mirror(&centre, Affine::scale_non_uniform(2.0, 1.0));
        assert!(m.is_none() && dropped);
    }

    /// A petal drawn from the centre out and back to it, under six copies each
    /// mirrored, lands as six closed petals; drawn from one slice line to the
    /// next at one radius, as one closed ring; with neither end on a line, as six
    /// open runs. An end within reach of a line is pulled onto it.
    ///
    /// flip: `radial` without its `weld` → fails at "each closed at the centre".
    #[test]
    fn a_centre_repeats_the_run_and_welds_where_ends_meet() {
        let mut frame = switch(Axes::Centre);
        frame.at = Point::new(100.0, 100.0);
        let petal = run(&[(100.0, 100.0), (110.0, 60.0), (100.5, 99.6)]);
        let got = radial(petal, &frame, 1.0);
        assert_eq!(got.len(), 6, "six petals");
        assert!(got.iter().all(|s| s.closed), "each closed at the centre");

        let mut mirrored = frame;
        mirrored.reflect = true;
        // From the first slice line (straight up) to its mirror line, 30° on.
        let half = std::f64::consts::FRAC_PI_6;
        let end = Point::new(100.0 + 50.0 * half.sin(), 100.0 - 50.0 * half.cos());
        let arc = run(&[(100.0, 50.0), (110.0, 52.0), (end.x, end.y)]);
        let got = radial(arc, &mirrored, 1.0);
        assert_eq!(got.len(), 1, "one ring");
        assert!(got[0].closed);
        assert_eq!(got[0].anchors.len(), 24);

        let loose = run(&[(120.0, 60.0), (130.0, 70.0)]);
        let got = radial(loose, &frame, 1.0);
        assert_eq!(got.len(), 6);
        assert!(got.iter().all(|s| !s.closed));
    }

    /// The axes cross in the frame the run landed in at its first press, though
    /// Ctrl moved the first anchor into another frame.
    /// flip: `run_frame` reading the frame under the first anchor → fails at "A's
    /// middle".
    #[test]
    fn the_axes_stay_with_the_frame_the_run_began_in() {
        let ctx = egui::Context::default();
        let mut app = MastikApp::headless(&ctx);
        let page = app.session.page;
        let frame = |id, x: f64| mastik_core::Operation::CreateNode {
            id,
            parent: page,
            index: 0,
            kind: NodeKind::Artboard {
                size: mastik_core::kurbo::Size::new(200.0, 200.0),
                corner_radii: Default::default(),
                corner_smoothing: 0.0,
            },
            transform: Some(Affine::translate((x, 0.0))),
            name: None,
        };
        let (a, b) = (app.session.ids.mint(), app.session.ids.mint());
        assert!(app.session.commit(mastik_core::Transaction(vec![
            frame(a, 0.0),
            frame(b, 300.0)
        ])));
        app.session.mirror = Mirror {
            axes: Axes::Vertical,
            at: None,
            ..Mirror::default()
        };
        app.pen = Some(crate::preview::PenState {
            parent: a,
            parent_world: Affine::IDENTITY,
            anchors: [(350.0, 50.0), (60.0, 80.0)]
                .map(|(x, y)| PenAnchor::corner(Point::new(x, y)))
                .to_vec(),
            shaping: false,
            editing: None,
        });
        let (_, at) = app.mirror_axes(None).expect("mirroring");
        assert_eq!(at.x, 100.0, "A's middle");
    }

    const RECT: egui::Rect = egui::Rect {
        min: egui::Pos2::ZERO,
        max: egui::Pos2::new(800.0, 600.0),
    };

    /// One frame of the canvas alone, with these events.
    fn canvas_frame(
        ctx: &egui::Context,
        app: &mut MastikApp,
        events: Vec<egui::Event>,
    ) -> egui::FullOutput {
        let time = ctx.input(|i| i.time) + 0.1;
        ctx.run_ui(
            egui::RawInput {
                screen_rect: Some(RECT),
                time: Some(time),
                events,
                ..Default::default()
            },
            |ui| app.canvas_ui(ui),
        )
    }

    /// A few frames with the pointer at `p`, so hover settles; the last's output.
    fn hover(ctx: &egui::Context, app: &mut MastikApp, p: egui::Pos2) -> egui::FullOutput {
        for _ in 0..3 {
            canvas_frame(ctx, app, vec![egui::Event::PointerMoved(p)]);
        }
        canvas_frame(ctx, app, vec![egui::Event::PointerMoved(p)])
    }

    fn primary(pos: egui::Pos2, pressed: bool) -> egui::Event {
        egui::Event::PointerButton {
            pos,
            button: egui::PointerButton::Primary,
            pressed,
            modifiers: egui::Modifiers::NONE,
        }
    }

    /// The vertical pill's hit rect as last painted.
    fn pill(app: &MastikApp) -> egui::Rect {
        let pills = &app.mirror_grips.pills;
        assert_eq!(pills.len(), 1, "one pill: {pills:?}");
        pills[0].1
    }

    /// Over a frame the axis runs through it from its middle, the pill on its top
    /// edge; off it, down the view from the view's middle, the pill under the
    /// ruler — even with the view's middle inside the frame.
    ///
    /// flip: `axis_frame` taking the frame under the axes' crossing → fails at "off the frame".
    #[test]
    fn the_axis_runs_through_the_frame_under_the_pointer_or_down_the_view() {
        let ctx = egui::Context::default();
        crate::theme::install(&ctx);
        let mut app = MastikApp::headless(&ctx);
        let mut ids = mastik_core::IdSource::new(1);
        app.session
            .adopt_document(mastik_core::Document::new(ids.mint()), None);
        app.session.camera.center = Point::ZERO;
        app.session.camera.zoom = 1.0;
        let page = app.session.page;
        let frame = app.session.ids.mint();
        let tx = crate::tools::create_artboard(
            frame,
            page,
            0,
            Affine::IDENTITY,
            Point::new(-200.0, -100.0),
            Point::new(200.0, 100.0),
            mastik_core::peniko::Color::WHITE,
        );
        assert!(app.session.commit(tx));
        app.choose_tool(crate::tools::Tool::Pen);
        app.session.mirror.axes = Axes::Vertical;
        canvas_frame(&ctx, &mut app, Vec::new());

        let inside = app.to_screen(Point::new(50.0, 0.0), RECT, 1.0);
        hover(&ctx, &mut app, inside);
        let on = pill(&app);
        let top = app.to_screen(Point::new(0.0, -100.0), RECT, 1.0);
        assert!(
            (on.center().x - top.x).abs() < 0.5,
            "the frame's middle: {on:?}"
        );
        assert!((on.top() + 1.0 - top.y).abs() < 0.5, "on the frame: {on:?}");

        let outside = app.to_screen(Point::new(50.0, 250.0), RECT, 1.0);
        hover(&ctx, &mut app, outside);
        let off = pill(&app);
        let inner = app.canvas_inner(RECT);
        assert!(
            (off.top() + 1.0 - inner.top()).abs() < 0.5,
            "off the frame: {off:?}"
        );
        assert!(
            (off.center().x - inner.center().x).abs() < 0.5,
            "the view's middle: {off:?}"
        );
    }

    /// A press on the pill and its drag move the axis: the pen plants nothing, and
    /// the pill shows no *On axis* chip.
    ///
    /// flip: `normal_mode_input` without the pill's claim → fails at "the pen plants nothing".
    #[test]
    fn a_drag_on_the_pill_moves_the_axis_and_draws_nothing() {
        let ctx = egui::Context::default();
        crate::theme::install(&ctx);
        let mut app = mirroring_pen(&ctx);
        canvas_frame(&ctx, &mut app, Vec::new());
        let p = pill(&app).center();
        let out = hover(&ctx, &mut app, p);
        assert!(
            !out.shapes
                .iter()
                .any(|c| matches!(&c.shape, egui::Shape::Text(t) if t.galley.text() == "On axis")),
            "no chip on the pill"
        );
        let to = p + egui::vec2(40.0, 0.0);
        canvas_frame(&ctx, &mut app, vec![primary(p, true)]);
        for i in 1..=4 {
            let at = p + egui::vec2(10.0 * i as f32, 0.0);
            canvas_frame(&ctx, &mut app, vec![egui::Event::PointerMoved(at)]);
        }
        canvas_frame(&ctx, &mut app, vec![primary(to, false)]);
        assert!(app.pen.is_none(), "the pen plants nothing");
        let at = app.session.mirror.at.expect("dragged");
        assert!((at.x - 140.0).abs() < 0.5, "the axis moved: {at:?}");
    }

    /// A right-click during a pill drag puts the axis back where the drag found it,
    /// and the rest of the drag moves nothing.
    ///
    /// flip: `cancel_gesture` without the pill's arm → fails at "put back".
    #[test]
    fn a_right_click_mid_drag_puts_the_axis_back() {
        let ctx = egui::Context::default();
        crate::theme::install(&ctx);
        let mut app = mirroring_pen(&ctx);
        canvas_frame(&ctx, &mut app, Vec::new());
        let p = pill(&app).center();
        hover(&ctx, &mut app, p);
        canvas_frame(&ctx, &mut app, vec![primary(p, true)]);
        for i in 1..=3 {
            let at = p + egui::vec2(10.0 * i as f32, 0.0);
            canvas_frame(&ctx, &mut app, vec![egui::Event::PointerMoved(at)]);
        }
        let moved = app.session.mirror.at.expect("dragged");
        assert!(
            (moved.x - 130.0).abs() < 0.5,
            "the drag moved it: {moved:?}"
        );
        // As the app's frame does on a right press, ahead of the UI.
        assert!(app.cancel_gesture(&ctx));
        assert_eq!(
            app.session.mirror.at,
            Some(Point::new(100.0, 0.0)),
            "put back"
        );
        let on = p + egui::vec2(60.0, 0.0);
        canvas_frame(&ctx, &mut app, vec![egui::Event::PointerMoved(on)]);
        canvas_frame(&ctx, &mut app, vec![primary(on, false)]);
        assert_eq!(
            app.session.mirror.at,
            Some(Point::new(100.0, 0.0)),
            "and stays"
        );
        assert!(app.pen.is_none(), "the pen plants nothing");
    }

    /// A right-button drag on the centre's grip leaves the centre where it is; the
    /// left button moves it.
    ///
    /// flip: `centre_chrome` moving on `dragged()` instead of the primary's drag →
    /// fails at "the right button moves nothing".
    #[test]
    fn only_the_left_button_drags_the_centre() {
        let ctx = egui::Context::default();
        crate::theme::install(&ctx);
        let mut app = mirroring_pen(&ctx);
        app.session.mirror.axes = Axes::Centre;
        canvas_frame(&ctx, &mut app, Vec::new());
        let c = pill(&app).center();
        let start = app.session.mirror.at;
        let drag = |app: &mut MastikApp, button| {
            hover(&ctx, app, c);
            let press = |pressed, pos| egui::Event::PointerButton {
                pos,
                button,
                pressed,
                modifiers: egui::Modifiers::NONE,
            };
            canvas_frame(&ctx, app, vec![press(true, c)]);
            let mut at = c;
            for i in 1..=3 {
                at = c + egui::vec2(10.0 * i as f32, 0.0);
                canvas_frame(&ctx, app, vec![egui::Event::PointerMoved(at)]);
            }
            canvas_frame(&ctx, app, vec![press(false, at)]);
        };
        drag(&mut app, egui::PointerButton::Secondary);
        assert_eq!(
            app.session.mirror.at, start,
            "the right button moves nothing"
        );
        drag(&mut app, egui::PointerButton::Primary);
        let moved = app.session.mirror.at.expect("dragged");
        assert!(
            (moved.x - 130.0).abs() < 0.5,
            "the left button moves it: {moved:?}"
        );
    }

    /// Under the pen a click on the centre's grip plants an anchor exactly on the
    /// centre and leaves the centre where it is; a drag still moves it.
    ///
    /// flip: `centre_chrome` without its click arm → fails at "a point on the centre".
    #[test]
    fn a_pen_click_on_the_centre_grip_places_a_point_there() {
        let ctx = egui::Context::default();
        crate::theme::install(&ctx);
        let mut app = mirroring_pen(&ctx);
        app.session.mirror.axes = Axes::Centre;
        canvas_frame(&ctx, &mut app, Vec::new());
        // Off the centre by a few points, still on the grip.
        let c = pill(&app).center() + egui::vec2(4.0, -3.0);
        hover(&ctx, &mut app, c);
        canvas_frame(&ctx, &mut app, vec![primary(c, true)]);
        canvas_frame(&ctx, &mut app, vec![primary(c, false)]);
        let anchors: Vec<Point> = app
            .pen
            .as_ref()
            .map(|p| p.anchors.iter().map(|a| a.at).collect())
            .unwrap_or_default();
        assert_eq!(anchors, [Point::new(100.0, 0.0)], "a point on the centre");
        assert!(!app.pen.as_ref().unwrap().shaping, "no handle pulled");
        assert_eq!(
            app.session.mirror.at,
            Some(Point::new(100.0, 0.0)),
            "the centre stays"
        );

        // Past the double-click delay, so the drag's press is a new click.
        for _ in 0..10 {
            canvas_frame(&ctx, &mut app, Vec::new());
        }
        let c = pill(&app).center();
        hover(&ctx, &mut app, c);
        canvas_frame(&ctx, &mut app, vec![primary(c, true)]);
        let mut at = c;
        for i in 1..=3 {
            at = c + egui::vec2(10.0 * i as f32, 0.0);
            canvas_frame(&ctx, &mut app, vec![egui::Event::PointerMoved(at)]);
        }
        canvas_frame(&ctx, &mut app, vec![primary(at, false)]);
        let moved = app.session.mirror.at.expect("dragged");
        assert!((moved.x - 130.0).abs() < 0.5, "a drag moves it: {moved:?}");
        assert_eq!(
            app.pen.as_ref().map(|p| p.anchors.len()),
            Some(1),
            "and places nothing"
        );
    }

    /// A path edit's pen mirrors through the edit's own switch: off when the edit
    /// opens whatever the drawing switch says, and off again in the next edit,
    /// the drawing switch untouched.
    ///
    /// flip: `MastikApp::mirror` reading the drawing switch inside an edit → fails
    /// at "the edit starts off"; `path_bar` keeping the switch past the edit →
    /// fails at "the next edit starts off too".
    #[test]
    fn a_path_edits_pen_mirrors_through_its_own_switch() {
        let ctx = egui::Context::default();
        let mut app = mirroring_pen(&ctx);
        let page = app.session.page;
        app.pen = Some(crate::preview::PenState {
            parent: page,
            parent_world: Affine::IDENTITY,
            anchors: [(20.0, 10.0), (60.0, 50.0)]
                .map(|(x, y)| PenAnchor::corner(Point::new(x, y)))
                .to_vec(),
            shaping: false,
            editing: None,
        });
        // Drawn off the axis, so the path lands plain.
        app.session.mirror.axes = Axes::Off;
        app.finish_pen(false);
        let id = app.session.selection.single().expect("the path");
        assert!(app.path_mirror(id).is_none(), "fixture: a plain path");
        app.session.mirror.axes = Axes::Vertical;
        let edit = |app: &mut MastikApp| {
            app.choose_tool(crate::tools::Tool::Node);
            app.session.selection.set_one(id);
            app.choose_tool(crate::tools::Tool::Pen);
            assert!(app.pen_bias, "fixture: the edit's pen is armed");
        };
        edit(&mut app);
        assert_eq!(app.mirror().axes, Axes::Off, "the edit starts off");
        assert!(app.mirror_frame(None).is_none());
        app.mirror_mut().axes = Axes::Both;
        assert_eq!(app.mirror_frame(None).map(|f| f.axes), Some(Axes::Both));
        // Done, and the bar's frame that sees no edit.
        app.choose_tool(crate::tools::Tool::Select);
        let _ = ctx.run_ui(Default::default(), |ui| app.path_bar(ui.ctx()));
        edit(&mut app);
        assert_eq!(app.mirror().axes, Axes::Off, "the next edit starts off too");
        assert_eq!(
            app.session.mirror.axes,
            Axes::Vertical,
            "the drawing switch"
        );
    }

    /// Leaving the pen puts a dragged axis back; the pencil and the eyedropper's
    /// round trip keep it.
    ///
    /// flip: `choose_tool` without the reset → fails at "leaving the pen".
    #[test]
    fn leaving_the_pen_puts_the_axis_back() {
        let ctx = egui::Context::default();
        let mut app = mirroring_pen(&ctx);
        app.choose_tool(crate::tools::Tool::Pencil);
        assert!(app.session.mirror.at.is_some(), "the pencil keeps it");
        app.choose_tool(crate::tools::Tool::Eyedropper);
        app.choose_tool(crate::tools::Tool::Eyedropper);
        assert_eq!(app.tool, crate::tools::Tool::Pencil);
        assert!(
            app.session.mirror.at.is_some(),
            "the eyedropper's trip keeps it"
        );
        app.choose_tool(crate::tools::Tool::Select);
        assert_eq!(app.session.mirror.at, None, "leaving the pen");
        assert_eq!(app.session.mirror.axes, Axes::Vertical, "the axes stay on");
    }
}
