//! Navigate: walking the layer tree from the keyboard, and leap labels (§13).
//!
//! A keymap rather than a canvas state: `MastikApp::mode` stays `Normal`, so the
//! canvas, the panels and the mouse behave as in the editor, and
//! [`MastikApp::keymap_mode`] hands `input::resolve` [`Mode::Navigate`] instead.
//! A text edit started from here runs in `Mode::TextInsert` and ends back here.
//!
//! Moves are structural: `j` and `k` are the Layers panel's row below and above,
//! so `j` steps back in z-order. Leap labels are the spatial instrument: two
//! home-row letters on each layer a click could reach, and the last labels dealt
//! stay readable as handles ([`MastikApp::leap_labels`]) after they hide.

use crate::app::MastikApp;
use crate::input::{Action, Mode, Nav, Step};
use crate::theme::{self, color, icon};
use crate::tools::Tool;
use eframe::egui;
use mastik_core::kurbo::{Point, Rect};
use mastik_core::{NodeId, NodeKind};
use std::collections::{HashMap, HashSet};

/// The Tools menu's and the palette's row while Navigate is on.
pub(crate) const STOP_NAVIGATE: &str = "Stop navigating";

/// The pill's hint after the path.
const HINT: &str = "f labels · : command · ` leaves";

/// The home row, the letters a label is spelled from.
const HOME_ROW: [char; 9] = ['a', 's', 'd', 'f', 'g', 'h', 'j', 'k', 'l'];

/// How many labels two home-row letters spell.
pub(crate) const LABELS_MAX: usize = HOME_ROW.len() * HOME_ROW.len();

/// How far each outline stands off its layer's bounds, in points.
const OUTSET: f32 = 3.0;

/// A group's or frame's dashed outline: the entered instance's dash, so the two
/// dashed boundaries on the canvas read alike.
const DASH: f32 = 4.0;
const GAP: f32 = 3.0;

/// The air in a label: a main's chip's, tighter than a badge's, since a view can
/// carry eighty of them.
const LABEL_PAD: egui::Vec2 = egui::vec2(4.0, 2.0);

/// The space a sliding label leaves after the one it slid past.
const LABEL_GAP: f32 = 2.0;

/// How strongly a label the first letter ruled out is drawn: still there, so
/// the eye does not lose its place, but plainly out.
const RULED_OUT: f32 = 0.25;

/// Points between the samples a deal hit-tests across the view: under the
/// smallest layer a click comfortably reaches. Each layer's centre is tested too,
/// for anything smaller.
const SAMPLE_PT: f64 = 8.0;

/// The most samples one deal takes; a wider view samples more sparsely.
const SAMPLES_MAX: f64 = 40_000.0;

/// Navigate's state on the app. The labels outlive the mode: they are handles
/// the command bar reads until the next deal replaces them.
#[derive(Default)]
pub(crate) struct Navigate {
    /// While the mode is on.
    on: Option<On>,
    /// The last labels dealt, lowercase.
    labels: HashMap<String, NodeId>,
    /// The last deal, nearest the current layer first, which is also who got
    /// the easy pairs: what the labels draw from.
    deal: Vec<Dealt>,
    /// The labels on the canvas in Navigate, while they show.
    shown: Option<Shown>,
}

struct On {
    /// The tool to give back on leaving.
    prior: Tool,
    /// The layer the keys move from, while it is still selected.
    current: Option<NodeId>,
    /// Per parent, the child last stood on: where `l` goes back to.
    visited: HashMap<NodeId, NodeId>,
}

/// The deal on the canvas in Navigate.
struct Shown {
    /// The first letter, once typed.
    typed: Option<char>,
}

struct Dealt {
    label: String,
    id: NodeId,
    /// A group, boolean or frame: a dashed outline and a dark label.
    container: bool,
}

/// Every two-letter label, easiest first: index fingers before pinkies, and a
/// pair that changes hands or repeats a letter before one that stays on a hand.
pub(crate) fn label_pairs() -> Vec<String> {
    let cost = |c: char| match c {
        'f' | 'j' => 0,
        'd' | 'k' => 1,
        's' | 'l' => 2,
        'g' | 'h' => 3,
        _ => 4,
    };
    let left = |c: char| "asdfg".contains(c);
    let mut by_ease = HOME_ROW;
    by_ease.sort_by_key(|c| cost(*c));
    let mut pairs: Vec<(u8, String)> = by_ease
        .iter()
        .flat_map(|a| by_ease.iter().map(move |b| (*a, *b)))
        .map(|(a, b)| {
            let stays = left(a) == left(b) && a != b;
            (cost(a) + cost(b) + u8::from(stays), format!("{a}{b}"))
        })
        .collect();
    pairs.sort_by_key(|(score, _)| *score);
    pairs.into_iter().map(|(_, s)| s).collect()
}

/// Whether `c` is a letter a label is spelled from.
pub(crate) fn is_home_letter(c: char) -> bool {
    HOME_ROW.contains(&c)
}

/// The home-row letter `key` types, if it is one.
fn home_letter(key: egui::Key) -> Option<char> {
    use egui::Key as K;
    Some(match key {
        K::A => 'a',
        K::S => 's',
        K::D => 'd',
        K::F => 'f',
        K::G => 'g',
        K::H => 'h',
        K::J => 'j',
        K::K => 'k',
        K::L => 'l',
        _ => return None,
    })
}

impl MastikApp {
    /// The keymap `input::resolve` reads: Navigate's while it is on and no text
    /// is being typed, else the mode's own.
    pub(crate) fn keymap_mode(&self) -> Mode {
        match self.mode {
            Mode::Normal if self.navigate.on.is_some() => Mode::Navigate,
            mode => mode,
        }
    }

    /// Whether Navigate is on, a text edit started from it included.
    pub(crate) fn navigating(&self) -> bool {
        self.navigate.on.is_some()
    }

    /// The last labels dealt, lowercase (`"sk"`), kept after they hide until the
    /// next `f` deals again. A label whose layer has gone stays: the reader says
    /// there is no such layer.
    pub(crate) fn leap_labels(&self) -> &HashMap<String, NodeId> {
        &self.navigate.labels
    }

    /// `` ` ``, the Tools menu's *Navigate* and the palette's: on with the select
    /// tool, or back to the editor.
    pub(crate) fn toggle_navigate(&mut self) {
        if self.navigating() {
            self.leave_navigate();
            return;
        }
        let prior = self.tool;
        if self.tool != Tool::Select {
            self.choose_tool(Tool::Select);
        }
        self.navigate.on = Some(On {
            prior,
            current: self.session.selection.ids().last().copied(),
            visited: HashMap::new(),
        });
    }

    /// Back to the editor with the selection kept and the tool it had before.
    pub(crate) fn leave_navigate(&mut self) {
        self.navigate.shown = None;
        let Some(on) = self.navigate.on.take() else {
            return;
        };
        if self.tool != on.prior {
            // A drawing tool drops the selection as it arms; leaving keeps it.
            let kept = self.session.selection.clone();
            self.choose_tool(on.prior);
            self.session.selection = kept;
        }
    }

    /// Forget the layers Navigate holds as another document comes forward, which
    /// may share their ids: the labels dealt and shown, and where the keys stand.
    /// The mode itself stays on.
    pub(crate) fn forget_navigate_layers(&mut self) {
        self.navigate.labels.clear();
        self.navigate.deal.clear();
        self.navigate.shown = None;
        if let Some(on) = self.navigate.on.as_mut() {
            on.current = None;
            on.visited.clear();
        }
    }

    /// One key of Navigate's keymap.
    pub(crate) fn nav_action(&mut self, ctx: &egui::Context, nav: Nav) {
        match nav {
            Nav::Step(step, extend) => self.nav_step(step, extend),
            Nav::Labels { inside } => self.deal_labels(inside),
            Nav::StepIn => self.dispatch(ctx, Action::Enter),
            Nav::Zoom => {
                let bounds = self
                    .nav_current()
                    .and_then(|id| self.session.preview_world_bounds(id));
                self.zoom_to(bounds);
            }
            Nav::Undo => self.dispatch(ctx, Action::Undo),
            Nav::Command => self.open_command_bar(),
            Nav::Repeat => self.repeat_bar_line(ctx),
            Nav::Leave => self.leave_navigate(),
        }
    }

    /// The layer the keys move from: the one last moved to while it stays
    /// selected, else the selection's last.
    pub(crate) fn nav_current(&self) -> Option<NodeId> {
        let sel = &self.session.selection;
        self.navigate
            .on
            .as_ref()
            .and_then(|on| on.current)
            .filter(|id| sel.contains(*id))
            .or_else(|| sel.ids().last().copied())
            .filter(|id| self.session.doc.contains(*id))
    }

    /// The parent rows of the Layers panel: the entered group's children, else the
    /// page's.
    fn nav_scope(&self) -> NodeId {
        self.entered_group
            .filter(|g| self.session.doc.contains(*g))
            .unwrap_or(self.session.page)
    }

    /// Move the current layer one step through the tree; with `extend`, add it
    /// to the selection rather than replace it.
    fn nav_step(&mut self, step: Step, extend: bool) {
        let doc = &self.session.doc;
        let parent_of = |id: NodeId| doc.get(id).and_then(|n| n.parent());
        let target = match self.nav_current() {
            // Nothing to move from: the scope's top row, for any step but out.
            None => match step {
                Step::Parent => None,
                _ => doc
                    .get(self.nav_scope())
                    .and_then(|n| n.children().last().copied()),
            },
            Some(at) => match step {
                Step::Parent => parent_of(at).filter(|p| !doc.is_page(*p)),
                Step::Child => {
                    let kids = doc.get(at).map(|n| n.children()).unwrap_or_default();
                    let back = self
                        .navigate
                        .on
                        .as_ref()
                        .and_then(|on| on.visited.get(&at))
                        .filter(|v| kids.contains(v));
                    back.or(kids.last()).copied()
                }
                // `children` is z-order with the topmost last, and the panel lists
                // it reversed: the row below is the index before.
                Step::Next | Step::Prev => parent_of(at).and_then(|p| {
                    let kids = doc.get(p)?.children();
                    let i = kids.iter().position(|c| *c == at)?;
                    match step {
                        Step::Next => i.checked_sub(1).map(|j| kids[j]),
                        _ => kids.get(i + 1).copied(),
                    }
                }),
            },
        };
        let Some(target) = target else {
            return;
        };
        self.nav_land(target, extend);
    }

    /// Make `id` the current layer: selected alone, or added with `add`, its
    /// parent remembering it for `l`, and scrolled into view.
    fn nav_land(&mut self, id: NodeId, add: bool) {
        let doc = &self.session.doc;
        let parent_of = |id: NodeId| doc.get(id).and_then(|n| n.parent());
        if let Some(on) = self.navigate.on.as_mut() {
            for at in [on.current, Some(id)].into_iter().flatten() {
                if let Some(p) = parent_of(at) {
                    on.visited.insert(p, at);
                }
            }
            on.current = Some(id);
        }
        if add {
            self.session.selection.add(id);
        } else {
            self.session.selection.set_one(id);
        }
        if let Some(bounds) = self.session.preview_world_bounds(id) {
            self.reveal(bounds);
        }
    }

    /// `f` and `F`: deal labels to the layers in view a click could reach, or with
    /// `inside` to those inside the current layer, and show them.
    fn deal_labels(&mut self, inside: bool) {
        if self.deal(inside, true) {
            self.navigate.shown = Some(Shown { typed: None });
        }
    }

    /// The command bar opening: deal labels to the layers in view as `f` does,
    /// for the bar to show while a label is being typed. Says nothing when there
    /// is nothing to label; the last deal then stands.
    pub(crate) fn deal_bar_labels(&mut self) {
        self.navigate.shown = None;
        self.deal(false, false);
    }

    /// Deal labels and keep them as the handles lines name; `false` when there
    /// was nothing to label, which `say` reports on the status line. A view with
    /// more than [`LABELS_MAX`] labels only the children of the current layer.
    fn deal(&mut self, inside: bool, say: bool) -> bool {
        let doc = &self.session.doc;
        let current = self.nav_current();
        let within = match (inside, current) {
            (true, Some(c)) if doc.get(c).is_some_and(|n| !n.children().is_empty()) => Some(c),
            (true, _) => {
                if say {
                    self.session.info("Nothing inside this layer to label");
                }
                return false;
            }
            (false, _) => None,
        };
        let view = self.session.camera.viewport(self.canvas_px).view;
        let mut ids = self.leap_candidates(view, within);
        if ids.len() > LABELS_MAX {
            let parent = current.and_then(|c| doc.get(c)).and_then(|n| n.parent());
            let holder = current
                .filter(|c| doc.get(*c).is_some_and(|n| !n.children().is_empty()))
                .or(parent)
                .unwrap_or_else(|| self.nav_scope());
            ids = doc
                .get(holder)
                .map(|n| n.children().to_vec())
                .unwrap_or_default()
                .into_iter()
                .filter(|id| {
                    self.session
                        .preview_world_bounds(*id)
                        .is_some_and(|b| b.overlaps(view))
                })
                .collect();
        }
        if ids.is_empty() {
            if say {
                self.session.info("No layers in view to label");
            }
            return false;
        }
        // Nearest the current layer first, so the easy pairs land where the eye is.
        let from = current
            .and_then(|c| self.session.preview_world_bounds(c))
            .map_or(view.center(), |b| b.center());
        let centre = |id: &NodeId| {
            self.session
                .preview_world_bounds(*id)
                .map_or(f64::INFINITY, |b| (b.center() - from).hypot2())
        };
        ids.sort_by(|a, b| centre(a).total_cmp(&centre(b)));
        ids.truncate(LABELS_MAX);
        let deal: Vec<Dealt> = ids
            .into_iter()
            .zip(label_pairs())
            .map(|(id, label)| Dealt {
                label,
                id,
                container: doc.get(id).is_some_and(|n| {
                    matches!(
                        n.kind(),
                        NodeKind::Group | NodeKind::Boolean { .. } | NodeKind::Artboard { .. }
                    )
                }),
            })
            .collect();
        self.navigate.labels = deal.iter().map(|d| (d.label.clone(), d.id)).collect();
        self.navigate.deal = deal;
        true
    }

    /// The layers in `view` a plain click could select, in the order first met:
    /// the canvas's hit test sampled across the view and at each layer's centre,
    /// each hit raised to what the click policy picks. With `within`, only the
    /// layers inside it, picked as if it were the entered group.
    pub(crate) fn leap_candidates(&self, view: Rect, within: Option<NodeId>) -> Vec<NodeId> {
        let doc = &self.session.doc;
        let mut step = SAMPLE_PT / self.points_per_world();
        let samples = (view.width() / step) * (view.height() / step);
        if samples > SAMPLES_MAX {
            step *= (samples / SAMPLES_MAX).sqrt();
        }
        let mut points: Vec<Point> = Vec::new();
        let mut y = view.y0 + step / 2.0;
        while y < view.y1 {
            let mut x = view.x0 + step / 2.0;
            while x < view.x1 {
                points.push(Point::new(x, y));
                x += step;
            }
            y += step;
        }
        points.extend(
            mastik_core::nodes_in_view(&self.session.resolved, self.session.page, view)
                .into_iter()
                .filter_map(|id| self.session.preview_world_bounds(id))
                .map(|b| b.center())
                .filter(|c| view.contains(*c)),
        );
        let mut seen = HashSet::new();
        let mut out = Vec::new();
        for p in points {
            let Some(leaf) = self.pick_leaf(p) else {
                continue;
            };
            let picked = match within {
                Some(g) if leaf == g || !mastik_core::is_within(doc, leaf, g) => continue,
                Some(g) => mastik_core::group_chain(doc, leaf, Some(g))
                    .first()
                    .copied(),
                None => mastik_core::group_chain(doc, leaf, self.entered_group)
                    .first()
                    .copied(),
            };
            let id = picked.unwrap_or(leaf);
            if seen.insert(id) {
                out.push(id);
            }
        }
        out
    }

    /// Read the keys while labels show, in place of the keymap; `false` when none
    /// show. The first letter narrows, the second selects (with Shift, adds),
    /// Backspace takes the first back and Escape hides them.
    pub(crate) fn leap_keys(&mut self, ctx: &egui::Context) -> bool {
        if self.navigate.shown.is_none() || self.keymap_mode() != Mode::Navigate {
            return false;
        }
        if ctx.egui_wants_keyboard_input() {
            return false;
        }
        let presses: Vec<(egui::Key, egui::Modifiers)> = ctx.input(|i| {
            i.events
                .iter()
                .filter_map(|e| match e {
                    egui::Event::Key {
                        key,
                        pressed: true,
                        modifiers,
                        ..
                    } => Some((*key, *modifiers)),
                    _ => None,
                })
                .collect()
        });
        for (key, m) in presses {
            let Some(shown) = self.navigate.shown.as_mut() else {
                break;
            };
            if key == egui::Key::Escape {
                self.navigate.shown = None;
                break;
            }
            if m.command || m.alt {
                continue;
            }
            if key == egui::Key::Backspace {
                shown.typed = None;
                continue;
            }
            let Some(c) = home_letter(key) else {
                continue;
            };
            match shown.typed {
                None => {
                    if self.navigate.deal.iter().any(|d| d.label.starts_with(c)) {
                        shown.typed = Some(c);
                    }
                }
                Some(first) => {
                    let label: String = [first, c].into_iter().collect();
                    let hit = self.navigate.deal.iter().find(|d| d.label == label);
                    if let Some(id) = hit.map(|d| d.id) {
                        self.navigate.shown = None;
                        self.nav_land(id, m.shift);
                        break;
                    }
                }
            }
        }
        true
    }

    /// The labels and their outlines over the canvas while they show — in
    /// Navigate after `f`, or while the command bar is offering labels for the
    /// word being typed: every outline first, then every label, so no outline
    /// crosses a label. Letters already typed narrow them: the outlines stay only
    /// on the labels they match, and the rest fade. A label that would cover one
    /// already placed slides right along its layer's top edge.
    pub(crate) fn draw_leap_labels(&self, painter: &egui::Painter, rect: egui::Rect, ppp: f32) {
        let typed: String = match &self.navigate.shown {
            Some(shown) if self.keymap_mode() == Mode::Navigate => {
                shown.typed.map(String::from).unwrap_or_default()
            }
            _ => match self.command_bar.as_ref().and_then(|b| b.labels_typed()) {
                Some(word) => word.to_owned(),
                None => return,
            },
        };
        let deal = &self.navigate.deal;
        let matched = |d: &Dealt| d.label.starts_with(&typed);
        let boxes: Vec<Option<egui::Rect>> = deal
            .iter()
            .map(|d| {
                let b = self.session.preview_world_bounds(d.id)?;
                let a = self.to_screen(Point::new(b.x0, b.y0), rect, ppp);
                let z = self.to_screen(Point::new(b.x1, b.y1), rect, ppp);
                Some(egui::Rect::from_two_pos(a, z).expand(OUTSET))
            })
            .collect();
        for (d, b) in deal.iter().zip(&boxes) {
            if let Some(b) = b
                && matched(d)
            {
                outline(painter, *b, d.container);
            }
        }
        let font = egui::FontId::proportional(crate::ui::BADGE_PT);
        let mut placed: Vec<egui::Rect> = Vec::new();
        for (d, b) in deal.iter().zip(&boxes) {
            let Some(b) = b else { continue };
            let text = d.label.to_uppercase();
            let size = painter
                .layout_no_wrap(text.clone(), font.clone(), egui::Color32::PLACEHOLDER)
                .size()
                + LABEL_PAD * 2.0;
            let mut at = egui::Rect::from_min_size(b.left_top(), size);
            for _ in 0..=placed.len() {
                let Some(hit) = placed.iter().find(|p| p.intersects(at)) else {
                    break;
                };
                if hit.right() + LABEL_GAP >= b.right() {
                    break;
                }
                at = at.translate(egui::vec2(hit.right() + LABEL_GAP - at.left(), 0.0));
            }
            placed.push(at);
            let hue = if d.container {
                color::LEAP_INK
            } else {
                color::leap()
            };
            let mut p = painter.clone();
            let lead = match matched(d) {
                true => typed.chars().count(),
                false => {
                    p.multiply_opacity(RULED_OUT);
                    0
                }
            };
            crate::ui::canvas_pill_led(
                &p,
                at.min,
                egui::Align2::LEFT_TOP,
                &text,
                hue,
                LABEL_PAD,
                lead,
            );
        }
    }

    /// The pill's words after NAVIGATE: the path to the current layer, a caret
    /// between names, then the hint.
    pub(crate) fn navigate_pill(&self, ui: &mut egui::Ui) {
        let doc = &self.session.doc;
        let mut path: Vec<NodeId> = std::iter::successors(self.nav_current(), |id| {
            doc.get(*id).and_then(|n| n.parent())
        })
        .take_while(|id| !doc.is_page(*id))
        .collect();
        path.reverse();
        let pt = crate::ui::SMALL_PT;
        for (i, id) in path.iter().enumerate() {
            if i > 0 {
                ui.label(theme::icon_text(
                    icon::CARET_RIGHT,
                    pt,
                    theme::text::faint(),
                ));
            }
            ui.label(
                egui::RichText::new(crate::private::layer(doc, *id))
                    .size(pt)
                    .color(theme::text::strong()),
            );
        }
        ui.label(
            egui::RichText::new(HINT)
                .size(pt)
                .color(theme::text::faint()),
        );
    }
}

/// A labelled layer's outline: two inks a line, dark inside and the leap yellow
/// outside, so it reads on any artwork; dashed for a group or frame.
fn outline(painter: &egui::Painter, b: egui::Rect, dashed: bool) {
    for (r, ink) in [(b, color::LEAP_INK), (b.expand(1.0), color::leap())] {
        let stroke = egui::Stroke::new(1.0, ink);
        let corners = [
            r.left_top(),
            r.right_top(),
            r.right_bottom(),
            r.left_bottom(),
        ];
        if dashed {
            let closed = [corners[0], corners[1], corners[2], corners[3], corners[0]];
            painter.extend(egui::Shape::dashed_line(&closed, stroke, DASH, GAP));
        } else {
            painter.add(egui::Shape::closed_line(corners.to_vec(), stroke));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use mastik_core::kurbo::{Affine, Size};
    use mastik_core::{Document, IdSource, Operation, Transaction, build};

    /// The fixture's layers, as the Layers panel lists them top to bottom: `b`,
    /// the group `g` holding `r2` over `r1`, then `a`.
    struct F {
        app: MastikApp,
        a: NodeId,
        g: NodeId,
        r1: NodeId,
        r2: NodeId,
        b: NodeId,
    }

    fn square(id: NodeId, parent: NodeId, index: usize, x: f64, y: f64) -> Operation {
        Operation::CreateNode {
            id,
            parent,
            index,
            kind: NodeKind::Rect {
                size: Size::new(20.0, 20.0),
                corner_radii: Default::default(),
                corner_smoothing: 0.0,
            },
            transform: Some(Affine::translate((x, y))),
            name: None,
        }
    }

    /// `a`, `r1`, `r2`, `b` in a row on the page, `r1` and `r2` grouped, `b`
    /// selected, after one frame has laid the canvas out and fitted the row in it.
    fn fixture(ctx: &egui::Context) -> F {
        crate::theme::install(ctx);
        let mut app = MastikApp::headless(ctx);
        let mut ids = IdSource::new(0x4A7);
        let root = ids.mint();
        let mut doc = Document::new(root);
        let [a, r1, r2, b] = [ids.mint(), ids.mint(), ids.mint(), ids.mint()];
        doc.apply(&Transaction(vec![
            square(a, root, 0, 0.0, 0.0),
            square(r1, root, 1, 100.0, 0.0),
            square(r2, root, 2, 140.0, 0.0),
            square(b, root, 3, 300.0, 0.0),
        ]))
        .expect("the row");
        app.session.adopt_document(doc, None);
        let (tx, g) = build::group(
            &app.session.doc,
            &app.session.resolved,
            &mut app.session.ids,
            &[r1, r2],
        )
        .expect("a group");
        app.session.commit(tx);
        app.session.selection.set_one(b);
        frame(ctx, &mut app, Vec::new());
        app.zoom_to(Some(Rect::new(-20.0, -20.0, 340.0, 40.0)));
        let kids = app.session.doc.get(app.session.page).unwrap().children();
        assert_eq!(kids, [a, g, b], "fixture: the page holds a, the group, b");
        assert_eq!(app.session.doc.get(g).unwrap().children(), [r1, r2]);
        F {
            app,
            a,
            g,
            r1,
            r2,
            b,
        }
    }

    /// One frame of the whole app, its modifiers the first key's.
    fn frame(ctx: &egui::Context, app: &mut MastikApp, events: Vec<egui::Event>) {
        let modifiers = events
            .iter()
            .find_map(|e| match e {
                egui::Event::Key { modifiers, .. } => Some(*modifiers),
                _ => None,
            })
            .unwrap_or_default();
        let input = egui::RawInput {
            screen_rect: Some(egui::Rect::from_min_size(
                egui::Pos2::ZERO,
                egui::vec2(1320.0, 820.0),
            )),
            modifiers,
            events,
            ..Default::default()
        };
        let mut f = eframe::Frame::_new_kittest();
        let _ = ctx.run_ui(input, |ui| eframe::App::ui(app, ui, &mut f));
    }

    fn key_with(key: egui::Key, modifiers: egui::Modifiers) -> egui::Event {
        egui::Event::Key {
            key,
            physical_key: None,
            pressed: true,
            repeat: false,
            modifiers,
        }
    }

    /// Press `key` alone, a frame of its own.
    fn tap(ctx: &egui::Context, app: &mut MastikApp, key: egui::Key) {
        frame(ctx, app, vec![key_with(key, egui::Modifiers::NONE)]);
    }

    fn shift_tap(ctx: &egui::Context, app: &mut MastikApp, key: egui::Key) {
        frame(ctx, app, vec![key_with(key, egui::Modifiers::SHIFT)]);
    }

    fn selected(app: &MastikApp) -> Vec<NodeId> {
        app.session.selection.ids().to_vec()
    }

    /// The label dealt to `id`.
    fn label_of(app: &MastikApp, id: NodeId) -> String {
        app.leap_labels()
            .iter()
            .find(|(_, v)| **v == id)
            .map(|(k, _)| k.clone())
            .unwrap_or_else(|| panic!("{id:?} has no label: {:?}", app.leap_labels()))
    }

    fn letter_key(c: char) -> egui::Key {
        egui::Key::from_name(&c.to_string()).expect("a letter key")
    }

    /// `` ` `` turns Navigate on with the select tool and the pill's keymap; `Esc`
    /// leaves with the selection kept and the tool from before; `` ` `` leaves too.
    ///
    /// flip: `leave_navigate` not giving back `on.prior` → fails at "the tool from
    /// before".
    #[test]
    fn the_backtick_toggles_navigate_and_leaving_gives_the_tool_back() {
        let ctx = egui::Context::default();
        let mut f = fixture(&ctx);
        f.app.choose_tool(Tool::Rect);
        f.app.session.selection.set_one(f.b);

        tap(&ctx, &mut f.app, egui::Key::Backtick);
        assert!(f.app.navigating(), "the backtick turned it on");
        assert_eq!(f.app.keymap_mode(), Mode::Navigate);
        assert_eq!(f.app.mode, Mode::Normal, "a keymap, not a canvas mode");
        assert_eq!(f.app.tool, Tool::Select);

        tap(&ctx, &mut f.app, egui::Key::Escape);
        assert!(!f.app.navigating(), "Esc left");
        assert_eq!(f.app.tool, Tool::Rect, "the tool from before");
        assert_eq!(selected(&f.app), [f.b], "the selection kept");

        tap(&ctx, &mut f.app, egui::Key::Backtick);
        assert!(f.app.navigating());
        tap(&ctx, &mut f.app, egui::Key::Backtick);
        assert!(!f.app.navigating(), "the backtick leaves as well");
        assert_eq!(f.app.tool, Tool::Rect);
    }

    /// `j` and `k` are the Layers panel's row below and above, `l` the top row
    /// inside or the child last visited, `h` the parent; the ends stay put.
    ///
    /// flip: `Step::Next` taking the index after rather than before → fails at
    /// "the row below b".
    /// flip: `Step::Child` ignoring `visited` → fails at "the child last visited".
    #[test]
    fn hjkl_walk_the_layers_panel_rows() {
        let ctx = egui::Context::default();
        let mut f = fixture(&ctx);
        tap(&ctx, &mut f.app, egui::Key::Backtick);
        let app = &mut f.app;

        tap(&ctx, app, egui::Key::K);
        assert_eq!(selected(app), [f.b], "nothing above the top row");
        tap(&ctx, app, egui::Key::J);
        assert_eq!(selected(app), [f.g], "the row below b");
        tap(&ctx, app, egui::Key::L);
        assert_eq!(selected(app), [f.r2], "the top row inside the group");
        tap(&ctx, app, egui::Key::ArrowDown);
        assert_eq!(selected(app), [f.r1], "the arrow is j's twin");
        tap(&ctx, app, egui::Key::ArrowUp);
        assert_eq!(selected(app), [f.r2]);
        tap(&ctx, app, egui::Key::J);
        tap(&ctx, app, egui::Key::H);
        assert_eq!(selected(app), [f.g], "h is the parent");
        tap(&ctx, app, egui::Key::H);
        assert_eq!(selected(app), [f.g], "the page is no layer to step to");
        tap(&ctx, app, egui::Key::L);
        assert_eq!(selected(app), [f.r1], "the child last visited");
        tap(&ctx, app, egui::Key::ArrowLeft);
        tap(&ctx, app, egui::Key::J);
        assert_eq!(selected(app), [f.a]);
        tap(&ctx, app, egui::Key::J);
        assert_eq!(selected(app), [f.a], "nothing below the bottom row");
        assert_eq!(app.tool, Tool::Select, "no tool letter answered");
    }

    /// Shift with a move adds the layer it lands on, and the next move goes on
    /// from there.
    ///
    /// flip: `nav_land` ignoring `add` → fails at "b and the group".
    #[test]
    fn shift_with_a_move_extends_the_selection() {
        let ctx = egui::Context::default();
        let mut f = fixture(&ctx);
        tap(&ctx, &mut f.app, egui::Key::Backtick);
        shift_tap(&ctx, &mut f.app, egui::Key::J);
        assert_eq!(selected(&f.app), [f.b, f.g], "b and the group");
        shift_tap(&ctx, &mut f.app, egui::Key::ArrowDown);
        assert_eq!(selected(&f.app), [f.b, f.g, f.a], "and a, below the group");
        tap(&ctx, &mut f.app, egui::Key::K);
        assert_eq!(selected(&f.app), [f.g], "a plain move selects alone");
    }

    /// The editor's keys rest in Navigate: a tool letter, Delete, a digit; `u`
    /// undoes and Ctrl+Z keeps its meaning.
    ///
    /// flip: `navigate_mode` reading the table as `Mode::Normal` → fails at "R rests".
    #[test]
    fn the_editors_keys_rest_and_undo_stays() {
        let ctx = egui::Context::default();
        let mut f = fixture(&ctx);
        tap(&ctx, &mut f.app, egui::Key::Backtick);
        let app = &mut f.app;

        tap(&ctx, app, egui::Key::R);
        assert_eq!(app.tool, Tool::Select, "R rests");
        tap(&ctx, app, egui::Key::Delete);
        tap(&ctx, app, egui::Key::Num5);
        assert!(app.session.doc.contains(f.b), "Delete rests");
        assert_eq!(
            app.session.doc.get(f.b).unwrap().opacity(),
            1.0,
            "a digit sets no opacity"
        );

        let depth = app.session.history.undo_depth();
        app.session.commit(Transaction(vec![Operation::SetName {
            id: f.b,
            name: "Renamed".into(),
        }]));
        tap(&ctx, app, egui::Key::U);
        assert_eq!(app.session.history.undo_depth(), depth, "u undid it");
        frame(
            &ctx,
            app,
            vec![key_with(
                egui::Key::Z,
                egui::Modifiers::COMMAND | egui::Modifiers::SHIFT,
            )],
        );
        assert_eq!(
            app.session.history.undo_depth(),
            depth + 1,
            "Ctrl+Shift+Z redid it"
        );
        frame(
            &ctx,
            app,
            vec![key_with(egui::Key::Z, egui::Modifiers::COMMAND)],
        );
        assert_eq!(app.session.history.undo_depth(), depth, "Ctrl+Z undid it");
        assert!(app.navigating(), "and Navigate is still on");
    }

    /// `Enter` steps into a group as the editor's does, and Navigate stays on.
    ///
    /// flip: `Nav::StepIn` dispatching `Action::Escape` → fails at "inside".
    #[test]
    fn enter_steps_in_as_the_editors_does() {
        let ctx = egui::Context::default();
        let mut f = fixture(&ctx);
        f.app.session.selection.set_one(f.g);
        tap(&ctx, &mut f.app, egui::Key::Backtick);
        tap(&ctx, &mut f.app, egui::Key::I);
        assert_eq!(selected(&f.app), [f.r2], "inside, on the top child");
        assert_eq!(f.app.entered_group, Some(f.g));
        assert!(f.app.navigating());
    }

    /// A text edit started from Navigate types in the text keymap, and its Escape
    /// comes back to Navigate rather than out of it.
    ///
    /// flip: `keymap_mode` answering Navigate whatever `mode` holds → fails at "the
    /// text keymap".
    #[test]
    fn a_text_edit_from_navigate_ends_back_in_navigate() {
        let ctx = egui::Context::default();
        let mut f = fixture(&ctx);
        let text = f.app.session.ids.mint();
        let parts = crate::fonts::FontService::default_text_parts();
        f.app
            .session
            .commit(Transaction(vec![Operation::CreateNode {
                id: text,
                parent: f.app.session.page,
                index: 3,
                kind: NodeKind::Text {
                    content: "Pro".into(),
                    style: Box::new(parts.style),
                    spans: parts.spans,
                    para_spans: parts.para_spans,
                    paragraph: parts.paragraph,
                    block: parts.block,
                    sizing: parts.sizing,
                    on_path: None,
                    on_path_flip: false,
                    on_path_offset: 0.0,
                },
                transform: Some(Affine::translate((200.0, 0.0))),
                name: None,
            }]));
        f.app.session.selection.set_one(text);
        tap(&ctx, &mut f.app, egui::Key::Backtick);

        tap(&ctx, &mut f.app, egui::Key::Enter);
        assert!(f.app.text.is_some(), "Enter opened the text");
        assert_eq!(f.app.keymap_mode(), Mode::TextInsert, "the text keymap");
        assert!(f.app.navigating(), "with Navigate still on beneath it");

        tap(&ctx, &mut f.app, egui::Key::Escape);
        assert!(f.app.text.is_none(), "Escape ended the edit");
        assert_eq!(f.app.keymap_mode(), Mode::Navigate, "and came back here");
        assert_eq!(selected(&f.app), [text]);
    }

    /// The pairs are the 81 two-letter home-row labels, each once, easiest first.
    ///
    /// flip: `label_pairs` sorting by the second letter's cost alone → fails at the
    /// first pair.
    #[test]
    fn the_labels_are_every_home_row_pair_easiest_first() {
        let pairs = label_pairs();
        assert_eq!(pairs.len(), LABELS_MAX);
        let unique: HashSet<&String> = pairs.iter().collect();
        assert_eq!(unique.len(), LABELS_MAX, "each pair once");
        assert!(
            pairs
                .iter()
                .all(|p| p.chars().count() == 2 && p.chars().all(|c| HOME_ROW.contains(&c)))
        );
        assert_eq!(
            &pairs[..4],
            ["ff", "fj", "jf", "jj"],
            "the index fingers lead"
        );
        assert_eq!(pairs.last().unwrap(), "aa", "both pinkies last");
    }

    /// `f` labels what a plain click could select: the group, not the squares
    /// inside it, and the nearest layer gets the easiest pair.
    ///
    /// flip: `leap_candidates` keeping the leaf rather than the chain's outermost →
    /// fails at "the group, not its squares".
    #[test]
    fn f_labels_what_a_click_could_reach() {
        let ctx = egui::Context::default();
        let mut f = fixture(&ctx);
        tap(&ctx, &mut f.app, egui::Key::Backtick);
        tap(&ctx, &mut f.app, egui::Key::F);
        let dealt: HashSet<NodeId> = f.app.leap_labels().values().copied().collect();
        assert_eq!(
            dealt,
            HashSet::from([f.a, f.g, f.b]),
            "the group, not its squares"
        );
        assert_eq!(label_of(&f.app, f.b), label_pairs()[0], "b is current");
        assert!(f.app.navigate.shown.is_some(), "and they show");
    }

    /// `F` labels inside the current layer only.
    ///
    /// flip: `deal_labels` passing `None` for `within` → fails at the set.
    #[test]
    fn shift_f_labels_inside_the_current_layer() {
        let ctx = egui::Context::default();
        let mut f = fixture(&ctx);
        f.app.session.selection.set_one(f.g);
        tap(&ctx, &mut f.app, egui::Key::Backtick);
        shift_tap(&ctx, &mut f.app, egui::Key::F);
        let dealt: HashSet<NodeId> = f.app.leap_labels().values().copied().collect();
        assert_eq!(dealt, HashSet::from([f.r1, f.r2]));
    }

    /// Over 81 candidates, only the current layer's children are labelled.
    ///
    /// flip: dropping the `LABELS_MAX` fallback in `deal_labels` → fails at the set
    /// (81 labels on the grid).
    #[test]
    fn over_81_candidates_label_only_the_children_of_the_current_layer() {
        let ctx = egui::Context::default();
        let mut f = fixture(&ctx);
        let page = f.app.session.page;
        let mut ops = Vec::new();
        for i in 0..90 {
            let id = f.app.session.ids.mint();
            let (x, y) = ((i % 15) as f64 * 22.0, 40.0 + (i / 15) as f64 * 22.0);
            ops.push(square(id, page, 3, x, y));
        }
        f.app.session.commit(Transaction(ops));
        f.app.zoom_to(Some(Rect::new(-20.0, -20.0, 340.0, 180.0)));
        f.app.session.selection.set_one(f.g);
        tap(&ctx, &mut f.app, egui::Key::Backtick);
        let view = f.app.session.camera.viewport(f.app.canvas_px).view;
        assert!(
            f.app.leap_candidates(view, None).len() > LABELS_MAX,
            "fixture: more candidates than labels"
        );
        tap(&ctx, &mut f.app, egui::Key::F);
        let dealt: HashSet<NodeId> = f.app.leap_labels().values().copied().collect();
        assert_eq!(dealt, HashSet::from([f.r1, f.r2]));
    }

    /// The first letter narrows the labels, the second selects; the labels hide
    /// and their map stays, Navigate on, until the next `f` deals again.
    ///
    /// flip: `leap_keys` clearing `labels` when it hides them → fails at "the map
    /// stays".
    /// flip: `leap_keys` not recording the first letter → fails at "the first
    /// letter narrows".
    #[test]
    fn two_letters_select_and_the_map_outlives_the_labels() {
        let ctx = egui::Context::default();
        let mut f = fixture(&ctx);
        tap(&ctx, &mut f.app, egui::Key::Backtick);
        tap(&ctx, &mut f.app, egui::Key::F);
        let label = label_of(&f.app, f.a);
        let mut letters = label.chars();
        let (first, second) = (letters.next().unwrap(), letters.next().unwrap());

        tap(&ctx, &mut f.app, letter_key(first));
        let typed = f.app.navigate.shown.as_ref().and_then(|s| s.typed);
        assert_eq!(typed, Some(first), "the first letter narrows");
        assert_eq!(selected(&f.app), [f.b], "one letter selects nothing");

        tap(&ctx, &mut f.app, letter_key(second));
        assert_eq!(selected(&f.app), [f.a], "the second selects");
        assert!(f.app.navigate.shown.is_none(), "and the labels hide");
        assert_eq!(f.app.leap_labels().len(), 3, "the map stays");
        assert_eq!(f.app.leap_labels().get(&label), Some(&f.a));
        assert!(f.app.navigating());

        tap(&ctx, &mut f.app, egui::Key::J);
        assert_eq!(selected(&f.app), [f.a], "the keys are the keymap's again");
        tap(&ctx, &mut f.app, egui::Key::Escape);
        assert!(!f.app.navigating());
        assert_eq!(f.app.leap_labels().len(), 3, "leaving keeps it too");
    }

    /// Shift on the last letter adds the layer; Escape hides the labels without
    /// leaving Navigate.
    ///
    /// flip: `nav_land` called with `false` from `leap_keys` → fails at "added".
    #[test]
    fn shift_on_the_last_letter_adds_and_escape_only_hides() {
        let ctx = egui::Context::default();
        let mut f = fixture(&ctx);
        tap(&ctx, &mut f.app, egui::Key::Backtick);
        tap(&ctx, &mut f.app, egui::Key::F);
        tap(&ctx, &mut f.app, egui::Key::Escape);
        assert!(f.app.navigate.shown.is_none(), "Escape hid them");
        assert!(f.app.navigating(), "and stayed in Navigate");

        tap(&ctx, &mut f.app, egui::Key::F);
        let label = label_of(&f.app, f.g);
        let mut letters = label.chars().map(letter_key);
        tap(&ctx, &mut f.app, letters.next().unwrap());
        shift_tap(&ctx, &mut f.app, letters.next().unwrap());
        assert_eq!(selected(&f.app), [f.b, f.g], "added");
    }

    /// After the first letter only the labels it still matches keep an outline:
    /// each outline is two closed lines, dark inside and yellow outside, and the
    /// group's are dashed.
    ///
    /// flip: `draw_leap_labels` outlining every dealt layer → fails at the count
    /// after the first letter.
    #[test]
    fn the_first_letter_keeps_outlines_only_on_what_it_matches() {
        let ctx = egui::Context::default();
        let mut f = fixture(&ctx);
        tap(&ctx, &mut f.app, egui::Key::Backtick);
        tap(&ctx, &mut f.app, egui::Key::F);
        // The yellow closed lines a frame of the overlay alone paints.
        let solid = |app: &MastikApp| {
            let out = ctx.run_ui(egui::RawInput::default(), |ui| {
                app.draw_leap_labels(ui.painter(), app.canvas_rect, app.canvas_ppp);
            });
            let yellow = egui::epaint::ColorMode::Solid(color::leap());
            out.shapes
                .iter()
                .filter(|c| {
                    matches!(&c.shape, egui::Shape::Path(p) if p.closed && p.stroke.color == yellow)
                })
                .count()
        };
        assert_eq!(solid(&f.app), 2, "a and b are solid; the group dashed");
        let first = label_of(&f.app, f.a).chars().next().unwrap();
        tap(&ctx, &mut f.app, letter_key(first));
        let left = f
            .app
            .leap_labels()
            .iter()
            .filter(|(k, v)| k.starts_with(first) && **v != f.g)
            .count();
        assert!(left < 2, "fixture: the letter rules a solid outline out");
        assert_eq!(solid(&f.app), left, "only what the letter still matches");
    }
}
