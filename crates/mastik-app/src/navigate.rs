//! Pick: the command bar's second focus (`docs/design/command-mode.md`, *Pick*).
//!
//! The bar has two focuses. In Type its field has the keys; in Pick the canvas
//! has them: every layer a click could reach wears a leap label, a label's
//! letters select it, and the tree's steps take the keys no label uses. A
//! keymap rather than a canvas state: `MastikApp::mode` stays `Normal`, so the
//! canvas, the panels and the mouse behave as in the editor, and
//! [`MastikApp::keymap_mode`] hands `input::resolve` [`Mode::Navigate`] instead.
//!
//! The deal is the bar's whole time open: labels stay bound as handles lines
//! name ([`MastikApp::leap_labels`]), keep their letters as the view moves, and
//! a label a line already holds keeps its layer through every later deal.

use crate::app::MastikApp;
use crate::input::{Mode, Nav, Step};
use crate::theme::color;
use crate::tools::Tool;
use eframe::egui;
use mastik_core::kurbo::{Point, Rect};
use mastik_core::{NodeId, NodeKind};
use std::collections::{HashMap, HashSet};

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
pub(crate) const LABEL_PAD: egui::Vec2 = egui::vec2(4.0, 2.0);

/// The space a sliding label leaves after the one it slid past.
const LABEL_GAP: f32 = 2.0;

/// How strongly a label the first letter ruled out is drawn: still there, so
/// the eye does not lose its place, but plainly out.
const RULED_OUT: f32 = 0.25;

/// The ring round the label of the layer under the pointer, in Pick.
const RING: f32 = 1.5;

/// Points between the samples a deal hit-tests across the view: under the
/// smallest layer a click comfortably reaches. Each layer's centre is tested too,
/// for anything smaller.
const SAMPLE_PT: f64 = 8.0;

/// The most samples one deal takes; a wider view samples more sparsely.
const SAMPLES_MAX: f64 = 40_000.0;

/// How long a moved view or document rests before it is dealt again, so a pan
/// or a wheel deals once at its end rather than every frame of it.
const REST_S: f64 = 0.15;

/// Pick's state on the app. The labels outlive a deal: they are handles the
/// bar's lines read until the next deal replaces them.
#[derive(Default)]
pub(crate) struct Navigate {
    /// While the bar is open.
    on: Option<On>,
    /// The labels lines may name, lowercase: the deal's, and any a line in the
    /// field held when the deal was made.
    labels: HashMap<String, NodeId>,
    /// The last deal, nearest the current layer first when it was made: what the
    /// canvas draws.
    deal: Vec<Dealt>,
    /// The letters typed of a label in Pick.
    typed: String,
    /// What the deal was made for.
    dealt_for: Option<DealKey>,
    /// What the last frame saw, and since when, so a moving view deals once it
    /// rests.
    seen: Option<(DealKey, f64)>,
}

struct On {
    /// The tool to give back when the bar closes.
    prior: Tool,
    /// Whether the canvas has the keys (Pick) rather than the field (Type).
    picking: bool,
    /// Pick from the middle of a line, where a value goes: a label or a click
    /// writes its layer into the line and gives the keys back, the selection
    /// left alone.
    naming: bool,
}

/// What a deal depends on: the page, the scope a click picks in, the view and
/// the document.
#[derive(Clone, Copy, PartialEq)]
struct DealKey {
    page: NodeId,
    scope: Option<NodeId>,
    view: Rect,
    revision: u64,
}

struct Dealt {
    label: String,
    id: NodeId,
    /// A group, boolean or frame: a dashed outline and a dark label.
    container: bool,
}

/// The home-row letters, easiest first: index fingers, then middle, ring, the
/// stretch and the pinkies. A deal of nine or fewer spells its labels from these.
pub(crate) fn label_singles() -> Vec<String> {
    let mut by_ease = HOME_ROW;
    by_ease.sort_by_key(|c| cost(*c));
    by_ease.iter().map(|c| c.to_string()).collect()
}

/// How hard a home-row letter is to reach.
fn cost(c: char) -> u8 {
    match c {
        'f' | 'j' => 0,
        'd' | 'k' => 1,
        's' | 'l' => 2,
        'g' | 'h' => 3,
        _ => 4,
    }
}

/// Every two-letter label, easiest first: index fingers before pinkies, and a
/// pair that changes hands or repeats a letter before one that stays on a hand.
pub(crate) fn label_pairs() -> Vec<String> {
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

/// The labels `text` holds as words: a line in the field names them, so a deal
/// must not move them to other layers under it.
fn words(text: &str) -> HashSet<&str> {
    text.split(|c: char| !(c.is_ascii_alphanumeric() || c == '_'))
        .filter(|w| !w.is_empty())
        .collect()
}

impl MastikApp {
    /// The keymap `input::resolve` reads: Pick's while the bar picks and no text
    /// is being typed, else the mode's own.
    pub(crate) fn keymap_mode(&self) -> Mode {
        match self.mode {
            Mode::Normal if self.picking() => Mode::Navigate,
            mode => mode,
        }
    }

    /// Whether the bar is open on its Pick focus.
    pub(crate) fn picking(&self) -> bool {
        self.command_bar.is_some() && self.pick_focus()
    }

    /// The focus the bar's session holds, read while the bar itself is taken out
    /// to be drawn.
    pub(crate) fn pick_focus(&self) -> bool {
        self.navigate.on.as_ref().is_some_and(|o| o.picking)
    }

    /// The labels lines may name, lowercase (`"f"`, `"sk"`). A label whose layer
    /// has gone stays: the reader says there is no such layer.
    pub(crate) fn leap_labels(&self) -> &HashMap<String, NodeId> {
        &self.navigate.labels
    }

    /// `` ` `` and the palette's *Pick layers*: the bar on its
    /// Pick focus, opening it first when it is shut.
    pub(crate) fn pick_layers(&mut self) {
        self.open_command_bar();
        self.set_picking(true);
    }

    /// The bar opening: the canvas runs the select tool under it, whatever was
    /// armed, and a fresh deal is made this frame.
    pub(crate) fn begin_bar_session(&mut self) {
        let prior = self.tool;
        if self.tool != Tool::Select {
            self.choose_tool(Tool::Select);
        }
        self.navigate.on = Some(On {
            prior,
            picking: false,
            naming: false,
        });
        self.navigate.typed.clear();
        self.navigate.dealt_for = None;
    }

    /// The bar closing: the tool from before comes back, unless another was
    /// chosen by hand meanwhile or a text edit took the keys (arming a tool
    /// would end it), and the selection stays.
    pub(crate) fn end_bar_session(&mut self) {
        self.navigate.typed.clear();
        let Some(on) = self.navigate.on.take() else {
            return;
        };
        if self.tool == Tool::Select && on.prior != Tool::Select && self.text.is_none() {
            // A drawing tool drops the selection as it arms; closing keeps it.
            let kept = self.session.selection.clone();
            self.choose_tool(on.prior);
            self.session.selection = kept;
        }
    }

    /// Give the keys to the canvas (`true`, Pick) or to the field.
    pub(crate) fn set_picking(&mut self, picking: bool) {
        self.set_pick_focus(picking, false);
    }

    /// Give the keys to the canvas to name a layer in the line rather than
    /// select one (`naming`), or as [`Self::set_picking`] does.
    pub(crate) fn set_pick_focus(&mut self, picking: bool, naming: bool) {
        if let Some(on) = self.navigate.on.as_mut() {
            on.picking = picking;
            on.naming = picking && naming;
        }
        self.navigate.typed.clear();
    }

    /// Whether the bar picks to name a layer in its line.
    pub(crate) fn naming_pick(&self) -> bool {
        self.picking() && self.navigate.on.as_ref().is_some_and(|o| o.naming)
    }

    /// Write `id`'s label into the bar's line at the caret and give the keys
    /// back to the field: a naming pick's answer, by label or by click.
    pub(crate) fn name_in_line(&mut self, id: NodeId) {
        if let Some(label) = self.label_for(id)
            && let Some(bar) = self.command_bar.as_mut()
        {
            bar.write_at_caret(label);
        }
        self.set_picking(false);
    }

    /// Forget the layers the labels hold as another document comes forward,
    /// which may share their ids.
    pub(crate) fn forget_navigate_layers(&mut self) {
        self.navigate.labels.clear();
        self.navigate.deal.clear();
        self.navigate.typed.clear();
        self.navigate.dealt_for = None;
        self.navigate.seen = None;
    }

    /// One key of Pick's keymap.
    pub(crate) fn nav_action(&mut self, ctx: &egui::Context, nav: Nav) {
        match nav {
            Nav::Step(step, extend) => self.nav_step(step, extend),
            Nav::StepIn => self.step_in(),
            Nav::StepOut => self.step_out(),
            Nav::Matching => self.select_matching(),
            Nav::Fit => self.zoom_to(self.document_bounds()),
            Nav::Zoom => self.zoom_to(self.selection_bounds()),
            Nav::Repeat => self.repeat_bar_line(ctx),
            Nav::Type => self.set_picking(false),
        }
    }

    /// The layer the keys move from: the selection's last.
    pub(crate) fn nav_current(&self) -> Option<NodeId> {
        self.session
            .selection
            .ids()
            .last()
            .copied()
            .filter(|id| self.session.doc.contains(*id))
    }

    /// The parent rows of the Layers panel: the entered group's children, else the
    /// page's.
    fn nav_scope(&self) -> NodeId {
        self.entered_group
            .filter(|g| self.session.doc.contains(*g))
            .unwrap_or(self.session.page)
    }

    /// Move to the next or previous sibling of the current layer in reading
    /// order on the canvas ([`Self::reading_order`]); with `extend`, add it to
    /// the selection rather than replace it. The canvas, not the Layers panel:
    /// z-order often runs against what the eye sees, and `↓` should go down.
    fn nav_step(&mut self, step: Step, extend: bool) {
        let doc = &self.session.doc;
        let target = match self.nav_current() {
            // Nothing to move from: the scope's first layer.
            None => self.reading_order(self.nav_scope()).first().copied(),
            Some(at) => doc.get(at).and_then(|n| n.parent()).and_then(|p| {
                let order = self.reading_order(p);
                let i = order.iter().position(|c| *c == at)?;
                match step {
                    Step::Next => order.get(i + 1).copied(),
                    Step::Prev => i.checked_sub(1).map(|j| order[j]),
                }
            }),
        };
        if let Some(target) = target {
            self.nav_land(target, extend);
        }
    }

    /// `parent`'s visible children as the eye reads them on the canvas: rows top
    /// to bottom, each left to right. A layer joins a row when its centre falls
    /// within the height of the row's first layer. Children with no box (empty,
    /// hidden) are left out.
    fn reading_order(&self, parent: NodeId) -> Vec<NodeId> {
        let doc = &self.session.doc;
        let Some(kids) = doc.get(parent).map(|n| n.children()) else {
            return Vec::new();
        };
        let mut boxes: Vec<(NodeId, Rect)> = kids
            .iter()
            .filter(|id| doc.get(**id).is_some_and(|n| n.visible()))
            .filter_map(|id| Some((*id, self.session.preview_world_bounds(*id)?)))
            .collect();
        boxes.sort_by(|a, b| a.1.y0.total_cmp(&b.1.y0));
        let mut rows: Vec<Vec<(NodeId, Rect)>> = Vec::new();
        for (id, b) in boxes {
            let cy = b.center().y;
            match rows.last_mut() {
                Some(row) if cy >= row[0].1.y0 && cy <= row[0].1.y1 => row.push((id, b)),
                _ => rows.push(vec![(id, b)]),
            }
        }
        rows.into_iter()
            .flat_map(|mut row| {
                row.sort_by(|a, b| a.1.x0.total_cmp(&b.1.x0));
                row.into_iter().map(|(id, _)| id)
            })
            .collect()
    }

    /// Into the one selected layer when it holds others, as the editor's `Enter`
    /// steps into a group; from anything else, back to the field.
    fn step_in(&mut self) {
        let doc = &self.session.doc;
        let into = self.session.selection.single().filter(|id| {
            doc.get(*id).is_some_and(|n| {
                !n.children().is_empty()
                    && (matches!(
                        n.kind(),
                        NodeKind::Group | NodeKind::Boolean { .. } | NodeKind::Artboard { .. }
                    ) || mastik_core::component::instance_root(doc, *id) == Some(*id))
            })
        });
        match into {
            Some(id) => self.enter_container(id, None),
            None => self.set_picking(false),
        }
    }

    /// Out of the entered group, selecting it, to the group it sits in; with none
    /// entered, to the current layer's parent.
    fn step_out(&mut self) {
        let doc = &self.session.doc;
        if let Some(g) = self.entered_group.filter(|g| doc.contains(*g)) {
            // The chain is outermost first: its last is the rung just above.
            self.entered_group = mastik_core::group_chain(doc, g, None).last().copied();
            self.session.selection.set_one(g);
            return;
        }
        if let Some(p) = self
            .nav_current()
            .and_then(|c| doc.get(c))
            .and_then(|n| n.parent())
            .filter(|p| !doc.is_page(*p))
        {
            self.session.selection.set_one(p);
        }
    }

    /// Select `id` alone, or add it with `add`, and scroll it into view.
    fn nav_land(&mut self, id: NodeId, add: bool) {
        if add {
            self.session.selection.add(id);
        } else {
            self.session.selection.set_one(id);
        }
        if let Some(bounds) = self.session.preview_world_bounds(id) {
            self.reveal(bounds);
        }
    }

    /// Each frame the bar is up: deal again when the scope or the page changed,
    /// or once a moved view or an edited document has rested. Never mid-press,
    /// so a drag ends before its layers are relabelled.
    pub(crate) fn deal_tick(&mut self, ctx: &egui::Context) {
        if self.command_bar.is_none() {
            return;
        }
        let key = DealKey {
            page: self.session.page,
            scope: self.entered_group,
            view: self.session.camera.viewport(self.canvas_px).view,
            revision: self.session.revision(),
        };
        let now = ctx.input(|i| i.time);
        let since = match self.navigate.seen {
            Some((k, t)) if k == key => t,
            _ => now,
        };
        self.navigate.seen = Some((key, since));
        let due = match self.navigate.dealt_for {
            Some(k) if k == key => false,
            Some(k) if k.page == key.page && k.scope == key.scope => {
                let rested = now - since >= REST_S;
                if !rested {
                    ctx.request_repaint_after(std::time::Duration::from_secs_f64(REST_S));
                }
                rested && !ctx.input(|i| i.pointer.any_down())
            }
            _ => true,
        };
        if due {
            self.deal();
            self.navigate.dealt_for = Some(key);
        }
    }

    /// Deal labels to what a click could reach in view, inside the entered group
    /// when there is one. A layer keeps the label
    /// it had while the deal keeps its length; a label the field's line holds
    /// keeps its layer; the rest take the easiest free label, nearest the current
    /// layer first. Nine or fewer are one letter each, more are two. A view with
    /// more than [`LABELS_MAX`] labels only the children of the current layer.
    fn deal(&mut self) {
        let doc = &self.session.doc;
        let current = self.nav_current();
        let view = self.session.camera.viewport(self.canvas_px).view;
        // Inside an entered group, only its layers: stepping in narrows the deal.
        let mut ids = self.leap_candidates(view, self.entered_group);
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
        // Nearest the current layer first, so the easy labels land where the eye is.
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

        let line = self
            .command_bar
            .as_ref()
            .map(|b| b.text.clone())
            .unwrap_or_default();
        let held = words(&line);
        let pinned: HashMap<String, NodeId> = self
            .navigate
            .labels
            .iter()
            .filter(|(k, _)| held.contains(k.as_str()))
            .map(|(k, v)| (k.clone(), *v))
            .collect();
        let pool = match ids.len() <= HOME_ROW.len() {
            true => label_singles(),
            false => label_pairs(),
        };
        let len = pool[0].len();
        let had: HashMap<NodeId, &str> = self
            .navigate
            .deal
            .iter()
            .filter(|d| d.label.len() == len)
            .map(|d| (d.id, d.label.as_str()))
            .collect();
        let mut taken: HashSet<String> = pinned.keys().cloned().collect();
        let mut out: Vec<Option<String>> = vec![None; ids.len()];
        for (slot, id) in out.iter_mut().zip(&ids) {
            let kept = pinned
                .iter()
                .find(|(_, v)| *v == id)
                .map(|(k, _)| k.clone())
                .or_else(|| {
                    had.get(id)
                        .filter(|l| !taken.contains(**l))
                        .map(|l| l.to_string())
                });
            if let Some(l) = kept {
                taken.insert(l.clone());
                *slot = Some(l);
            }
        }
        let mut free = pool.into_iter().filter(|l| !taken.contains(l));
        let container = |id: NodeId| {
            doc.get(id).is_some_and(|n| {
                matches!(
                    n.kind(),
                    NodeKind::Group | NodeKind::Boolean { .. } | NodeKind::Artboard { .. }
                )
            })
        };
        let deal: Vec<Dealt> = ids
            .iter()
            .zip(out)
            .filter_map(|(id, l)| {
                Some(Dealt {
                    label: l.or_else(|| free.next())?,
                    id: *id,
                    container: container(*id),
                })
            })
            .collect();
        let mut labels = pinned;
        labels.extend(deal.iter().map(|d| (d.label.clone(), d.id)));
        self.navigate.labels = labels;
        self.navigate.deal = deal;
    }

    /// The label `id` wears, dealing it the next free one when it has none, so a
    /// line can name a layer the deal did not reach.
    pub(crate) fn label_for(&mut self, id: NodeId) -> Option<String> {
        if let Some((k, _)) = self.navigate.labels.iter().find(|(_, v)| **v == id) {
            return Some(k.clone());
        }
        let singles = self.navigate.deal.iter().all(|d| d.label.len() == 1);
        let taken = |l: &String| self.navigate.labels.contains_key(l);
        let label = singles
            .then(label_singles)
            .and_then(|p| p.into_iter().find(|l| !taken(l)))
            .or_else(|| label_pairs().into_iter().find(|l| !taken(l)))?;
        let container = self.session.doc.get(id).is_some_and(|n| {
            matches!(
                n.kind(),
                NodeKind::Group | NodeKind::Boolean { .. } | NodeKind::Artboard { .. }
            )
        });
        self.navigate.labels.insert(label.clone(), id);
        self.navigate.deal.push(Dealt {
            label: label.clone(),
            id,
            container,
        });
        Some(label)
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

    /// In Pick, take the home-row letters out of this frame's keys before the
    /// keymap reads them: a label's letters select it (with Shift, add it), the
    /// first of two narrows, and `Backspace` takes a first letter back.
    pub(crate) fn leap_keys(&mut self, ctx: &egui::Context) {
        if !self.picking() || self.mode != Mode::Normal {
            return;
        }
        let typing = !self.navigate.typed.is_empty();
        let presses: Vec<(egui::Key, egui::Modifiers)> = ctx.input_mut(|i| {
            let mut out = Vec::new();
            i.events.retain(|e| match e {
                egui::Event::Key {
                    key,
                    pressed: true,
                    modifiers,
                    ..
                } if !modifiers.command
                    && !modifiers.alt
                    && (home_letter(*key).is_some()
                        || (typing && *key == egui::Key::Backspace)) =>
                {
                    out.push((*key, *modifiers));
                    false
                }
                _ => true,
            });
            out
        });
        for (key, m) in presses {
            if key == egui::Key::Backspace {
                self.navigate.typed.pop();
                continue;
            }
            let Some(c) = home_letter(key) else { continue };
            let typed = format!("{}{c}", self.navigate.typed);
            let hit = self.navigate.deal.iter().find(|d| d.label == typed);
            if let Some(id) = hit.map(|d| d.id) {
                self.navigate.typed.clear();
                if self.naming_pick() {
                    // A naming pick ends with the first label it takes.
                    self.name_in_line(id);
                    break;
                }
                self.nav_land(id, m.shift);
            } else if self
                .navigate
                .deal
                .iter()
                .any(|d| d.label.starts_with(&typed))
            {
                self.navigate.typed = typed;
            } else {
                self.navigate.typed.clear();
            }
        }
    }

    /// The letters typed of a label in Pick, for the bar's field to show.
    pub(crate) fn pick_typed(&self) -> &str {
        &self.navigate.typed
    }

    /// The names from the page down to the current layer's parent, the bar's head
    /// path: where the selection sits, and so where the labels are.
    pub(crate) fn pick_path(&self) -> Vec<String> {
        let doc = &self.session.doc;
        let from = self
            .nav_current()
            .and_then(|c| doc.get(c))
            .and_then(|n| n.parent())
            .or(self.entered_group);
        let mut path: Vec<NodeId> =
            std::iter::successors(from, |id| doc.get(*id).and_then(|n| n.parent()))
                .take_while(|id| !doc.is_page(*id))
                .collect();
        path.push(self.session.page);
        path.reverse();
        path.iter()
            .map(|id| crate::private::layer(doc, *id).into_owned())
            .collect()
    }

    /// The labels and their outlines over the canvas: all of them in Pick; in
    /// Type, those the completion offers for the word being typed, else the one
    /// on `pointed`, the layer a click would take, so it can be named. Every
    /// outline first, then every label, so no outline crosses a label. Letters
    /// already typed narrow them: the outlines stay only on the labels they
    /// match, and the rest fade. A label that would cover one already placed
    /// slides right along its layer's top edge. In Pick the label on `pointed`
    /// wears a ring.
    pub(crate) fn draw_leap_labels(
        &self,
        painter: &egui::Painter,
        rect: egui::Rect,
        ppp: f32,
        pointed: Option<NodeId>,
    ) {
        let picking = self.picking();
        let (typed, only) = if picking {
            (self.navigate.typed.clone(), None)
        } else {
            let Some(bar) = self.command_bar.as_ref() else {
                return;
            };
            match bar.labels_typed() {
                Some(word) => (word, None),
                None => match pointed.filter(|p| self.navigate.deal.iter().any(|d| d.id == *p)) {
                    Some(p) => (String::new(), Some(p)),
                    None => return,
                },
            }
        };
        let deal: Vec<&Dealt> = self
            .navigate
            .deal
            .iter()
            .filter(|d| only.is_none_or(|o| o == d.id))
            .collect();
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
                outline(painter, *b, d.container, color::leap());
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
            let pill = crate::ui::canvas_pill_led(
                &p,
                at.min,
                egui::Align2::LEFT_TOP,
                &text,
                hue,
                LABEL_PAD,
                lead,
            );
            if picking && Some(d.id) == pointed {
                p.rect_stroke(
                    pill.expand(RING),
                    egui::CornerRadius::same(4),
                    egui::Stroke::new(RING, color::SELECT),
                    egui::StrokeKind::Outside,
                );
            }
        }
    }
}

impl MastikApp {
    /// While the bar is open, each layer its line names other than the
    /// selection, outlined in the hue its pill wears, so the line and the canvas
    /// say which layer is which. Under the labels.
    pub(crate) fn draw_line_references(&self, painter: &egui::Painter, rect: egui::Rect, ppp: f32) {
        let Some(bar) = self.command_bar.as_ref() else {
            return;
        };
        for (id, hue) in bar.references() {
            let Some(b) = self.session.preview_world_bounds(*id) else {
                continue;
            };
            let a = self.to_screen(Point::new(b.x0, b.y0), rect, ppp);
            let z = self.to_screen(Point::new(b.x1, b.y1), rect, ppp);
            outline(
                painter,
                egui::Rect::from_two_pos(a, z).expand(OUTSET),
                false,
                *hue,
            );
        }
    }
}

/// A labelled layer's outline: two inks a line, dark inside and `hue` outside
/// (the leap yellow for a label), so it reads on any artwork; dashed for a group
/// or frame.
fn outline(painter: &egui::Painter, b: egui::Rect, dashed: bool, hue: egui::Color32) {
    for (r, ink) in [(b, color::LEAP_INK), (b.expand(1.0), hue)] {
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
    use mastik_core::kurbo::{Affine, Size, Vec2};
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

    /// Ten more squares in a row under the fixture's, in view: more than one
    /// letter can label.
    fn crowd(ctx: &egui::Context, f: &mut F) {
        let page = f.app.session.page;
        let ops = (0..10)
            .map(|i| {
                let id = f.app.session.ids.mint();
                square(id, page, 3, i as f64 * 30.0, 30.0)
            })
            .collect();
        f.app.session.commit(Transaction(ops));
        f.app.zoom_to(Some(Rect::new(-20.0, -20.0, 340.0, 70.0)));
        frame(ctx, &mut f.app, Vec::new());
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
        frame_with(ctx, app, events, modifiers);
    }

    fn frame_with(
        ctx: &egui::Context,
        app: &mut MastikApp,
        events: Vec<egui::Event>,
        modifiers: egui::Modifiers,
    ) {
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

    /// A press and release of the primary button at `at` with `modifiers` held,
    /// the pointer moved there first, then frames for the release to settle.
    fn click(ctx: &egui::Context, app: &mut MastikApp, at: egui::Pos2, modifiers: egui::Modifiers) {
        let button = |pressed| egui::Event::PointerButton {
            pos: at,
            button: egui::PointerButton::Primary,
            pressed,
            modifiers,
        };
        frame_with(ctx, app, vec![egui::Event::PointerMoved(at)], modifiers);
        frame_with(ctx, app, vec![button(true)], modifiers);
        frame_with(ctx, app, vec![button(false)], modifiers);
        frame_with(ctx, app, Vec::new(), egui::Modifiers::NONE);
    }

    /// Where `id`'s centre is on screen.
    fn centre_of(app: &MastikApp, id: NodeId) -> egui::Pos2 {
        let b = app.session.preview_world_bounds(id).expect("bounds");
        app.to_screen(b.center(), app.canvas_rect, app.canvas_ppp)
    }

    fn selected(app: &MastikApp) -> Vec<NodeId> {
        app.session.selection.ids().to_vec()
    }

    fn dealt(app: &MastikApp) -> HashSet<NodeId> {
        app.navigate.deal.iter().map(|d| d.id).collect()
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

    /// Type a label's letters, the last with `last` held.
    fn spell(ctx: &egui::Context, app: &mut MastikApp, label: &str, last: egui::Modifiers) {
        let n = label.chars().count();
        for (i, c) in label.chars().enumerate() {
            let m = if i + 1 == n {
                last
            } else {
                egui::Modifiers::NONE
            };
            frame(ctx, app, vec![key_with(letter_key(c), m)]);
        }
    }

    /// `` ` `` opens the bar on Pick with the select tool under it; `Esc` gives
    /// the keys to the field, `` ` `` takes them back, and `Esc` on an empty line
    /// closes the bar with the tool from before and the selection kept.
    ///
    /// flip: `end_bar_session` not choosing `on.prior` → fails at "the tool from
    /// before".
    #[test]
    fn the_backtick_opens_the_bar_on_pick_and_closing_gives_the_tool_back() {
        let ctx = egui::Context::default();
        let mut f = fixture(&ctx);
        f.app.choose_tool(Tool::Rect);
        f.app.session.selection.set_one(f.b);

        tap(&ctx, &mut f.app, egui::Key::Backtick);
        assert!(f.app.picking(), "the backtick opened the bar on Pick");
        assert_eq!(f.app.keymap_mode(), Mode::Navigate);
        assert_eq!(f.app.mode, Mode::Normal, "a keymap, not a canvas mode");
        assert_eq!(f.app.tool, Tool::Select, "the canvas selects under the bar");

        tap(&ctx, &mut f.app, egui::Key::Escape);
        assert!(f.app.command_bar.is_some(), "Esc kept the bar");
        assert!(!f.app.picking(), "and gave the keys to the field");
        tap(&ctx, &mut f.app, egui::Key::Backtick);
        assert!(f.app.picking(), "the backtick from the field picks again");

        tap(&ctx, &mut f.app, egui::Key::Escape);
        tap(&ctx, &mut f.app, egui::Key::Escape);
        assert!(
            f.app.command_bar.is_none(),
            "Esc on an empty line closed it"
        );
        assert_eq!(f.app.tool, Tool::Rect, "the tool from before");
        assert_eq!(selected(&f.app), [f.b], "the selection kept");
    }

    /// Inside a string the backtick is Luau's: it types rather than picks.
    ///
    /// flip: `read_keys` ignoring `in_string` → fails at "typed, not picked".
    #[test]
    fn a_backtick_inside_a_string_types() {
        let ctx = egui::Context::default();
        let mut f = fixture(&ctx);
        f.app.open_command_bar();
        f.app.command_bar.as_mut().unwrap().text = ".name = \"a".into();
        frame(&ctx, &mut f.app, Vec::new());
        tap(&ctx, &mut f.app, egui::Key::Backtick);
        assert!(!f.app.picking(), "typed, not picked");
    }

    /// Pick deals at once to what a plain click could select — the group, not
    /// its squares — one letter each while nine or fewer, the current layer the
    /// easiest.
    ///
    /// flip: `deal` spelling from `label_pairs` whatever the count → fails at "one
    /// letter each".
    #[test]
    fn pick_deals_one_letter_labels_at_once() {
        let ctx = egui::Context::default();
        let mut f = fixture(&ctx);
        tap(&ctx, &mut f.app, egui::Key::Backtick);
        assert_eq!(
            dealt(&f.app),
            HashSet::from([f.a, f.g, f.b]),
            "the group, not its squares"
        );
        assert!(
            f.app.leap_labels().keys().all(|k| k.len() == 1),
            "one letter each: {:?}",
            f.app.leap_labels()
        );
        assert_eq!(label_of(&f.app, f.b), label_singles()[0], "b is current");
    }

    /// Past nine, every label is two letters, and the first letter narrows.
    ///
    /// flip: `leap_keys` not keeping the first letter → fails at "the first
    /// letter narrows".
    #[test]
    fn past_nine_labels_are_two_letters_and_the_first_narrows() {
        let ctx = egui::Context::default();
        let mut f = fixture(&ctx);
        crowd(&ctx, &mut f);
        tap(&ctx, &mut f.app, egui::Key::Backtick);
        assert!(f.app.navigate.deal.len() > 9, "fixture: more than nine");
        assert!(f.app.leap_labels().keys().all(|k| k.len() == 2));
        let label = label_of(&f.app, f.a);
        let first = label.chars().next().unwrap();
        tap(&ctx, &mut f.app, letter_key(first));
        assert_eq!(
            f.app.pick_typed(),
            first.to_string(),
            "the first letter narrows"
        );
        assert_eq!(selected(&f.app), [f.b], "one letter selects nothing");
        tap(&ctx, &mut f.app, egui::Key::Backspace);
        assert_eq!(f.app.pick_typed(), "", "Backspace takes it back");
        assert!(f.app.picking(), "and does not step out");
        spell(&ctx, &mut f.app, &label, egui::Modifiers::NONE);
        assert_eq!(selected(&f.app), [f.a]);
    }

    /// A label selects; with Shift it adds; the deal stays for the next pick.
    ///
    /// flip: `nav_land` ignoring `add` → fails at "added".
    #[test]
    fn a_label_selects_and_shift_adds() {
        let ctx = egui::Context::default();
        let mut f = fixture(&ctx);
        tap(&ctx, &mut f.app, egui::Key::Backtick);
        let la = label_of(&f.app, f.a);
        spell(&ctx, &mut f.app, &la, egui::Modifiers::NONE);
        assert_eq!(selected(&f.app), [f.a]);
        let lg = label_of(&f.app, f.g);
        spell(&ctx, &mut f.app, &lg, egui::Modifiers::SHIFT);
        assert_eq!(selected(&f.app), [f.a, f.g], "added");
        assert_eq!(f.app.navigate.deal.len(), 3, "the deal stays");
        assert!(f.app.picking());
    }

    /// `Enter` steps into the picked group and the labels follow onto its
    /// children; `Backspace` steps out, selecting the group, and they come back.
    ///
    /// flip: `step_out` leaving `entered_group` as it was → fails at "out of the
    /// group".
    #[test]
    fn enter_steps_in_and_backspace_steps_out_and_the_labels_follow() {
        let ctx = egui::Context::default();
        let mut f = fixture(&ctx);
        tap(&ctx, &mut f.app, egui::Key::Backtick);
        let lg = label_of(&f.app, f.g);
        spell(&ctx, &mut f.app, &lg, egui::Modifiers::NONE);

        tap(&ctx, &mut f.app, egui::Key::Enter);
        assert_eq!(f.app.entered_group, Some(f.g), "into the group");
        assert_eq!(selected(&f.app), [f.r2], "on its top child");
        assert_eq!(dealt(&f.app), HashSet::from([f.r1, f.r2]));

        tap(&ctx, &mut f.app, egui::Key::Backspace);
        assert_eq!(f.app.entered_group, None, "out of the group");
        assert_eq!(selected(&f.app), [f.g], "the group selected");
        assert_eq!(dealt(&f.app), HashSet::from([f.a, f.g, f.b]));
        assert!(f.app.picking());
    }

    /// `Enter` on a layer with nothing inside gives the keys back to the field.
    ///
    /// flip: `step_in` dispatching the editor's `Enter` → fails at "to the field".
    #[test]
    fn enter_on_a_leaf_goes_back_to_the_field() {
        let ctx = egui::Context::default();
        let mut f = fixture(&ctx);
        tap(&ctx, &mut f.app, egui::Key::Backtick);
        let la = label_of(&f.app, f.a);
        spell(&ctx, &mut f.app, &la, egui::Modifiers::NONE);
        tap(&ctx, &mut f.app, egui::Key::Enter);
        assert!(
            f.app.command_bar.is_some() && !f.app.picking(),
            "to the field"
        );
        assert_eq!(selected(&f.app), [f.a]);
    }

    /// `↓` and `↑` step through siblings in reading order on the canvas; Shift
    /// adds.
    ///
    /// flip: `nav_step` walking `children` as the panel does → fails at "nothing
    /// after b, the last read".
    #[test]
    fn the_arrows_walk_siblings_and_shift_adds() {
        let ctx = egui::Context::default();
        let mut f = fixture(&ctx);
        tap(&ctx, &mut f.app, egui::Key::Backtick);
        // The row reads a, g, b left to right, though the panel lists b, g, a.
        tap(&ctx, &mut f.app, egui::Key::ArrowDown);
        assert_eq!(selected(&f.app), [f.b], "nothing after b, the last read");
        tap(&ctx, &mut f.app, egui::Key::ArrowUp);
        assert_eq!(selected(&f.app), [f.g], "the one before b");
        shift_tap(&ctx, &mut f.app, egui::Key::ArrowUp);
        assert_eq!(selected(&f.app), [f.g, f.a], "and a, added");
        tap(&ctx, &mut f.app, egui::Key::ArrowDown);
        assert_eq!(selected(&f.app), [f.g], "a plain step selects alone");
    }

    /// `↓` goes down the canvas whatever the z-order: three squares stacked so
    /// the panel lists them bottom to top.
    ///
    /// flip: `nav_step` walking `children` as the panel does → fails at "down
    /// the canvas".
    #[test]
    fn the_arrows_read_the_canvas_not_the_layer_order() {
        let ctx = egui::Context::default();
        let mut f = fixture(&ctx);
        let page = f.app.session.page;
        let ids: Vec<NodeId> = (0..3).map(|_| f.app.session.ids.mint()).collect();
        // Each lower one above the last in z-order, so the panel lists them
        // bottom to top.
        let ops = ids
            .iter()
            .enumerate()
            .map(|(i, id)| square(*id, page, 3 + i, 200.0, 60.0 + i as f64 * 30.0))
            .collect();
        f.app.session.commit(Transaction(ops));
        f.app.session.selection.set_one(ids[0]);
        tap(&ctx, &mut f.app, egui::Key::Backtick);
        tap(&ctx, &mut f.app, egui::Key::ArrowDown);
        assert_eq!(selected(&f.app), [ids[1]], "down the canvas");
        tap(&ctx, &mut f.app, egui::Key::ArrowDown);
        assert_eq!(selected(&f.app), [ids[2]]);
    }

    /// The editor's keys rest in Pick: a tool letter, Delete, a digit, a letter
    /// no label uses; Ctrl+Z still undoes.
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
        assert_eq!(
            app.session.history.undo_depth(),
            depth + 1,
            "u is no key here"
        );
        frame(
            &ctx,
            app,
            vec![key_with(egui::Key::Z, egui::Modifiers::COMMAND)],
        );
        assert_eq!(app.session.history.undo_depth(), depth, "Ctrl+Z undid it");
        assert!(app.picking(), "and the bar still picks");
    }

    /// A moved view is dealt again once it rests, and a layer keeps its letter
    /// though the current layer, which orders a fresh deal, has changed.
    ///
    /// flip: `deal` ignoring `had` → fails at "a keeps its letter".
    #[test]
    fn labels_keep_their_letters_when_the_view_moves() {
        let ctx = egui::Context::default();
        let mut f = fixture(&ctx);
        tap(&ctx, &mut f.app, egui::Key::Backtick);
        let la = label_of(&f.app, f.a);
        spell(&ctx, &mut f.app, &la, egui::Modifiers::NONE);
        assert_ne!(la, label_singles()[0], "fixture: a was not the easiest");
        let before = f.app.navigate.dealt_for.map(|k| k.view);
        f.app.session.camera.pan_pixels(Vec2::new(30.0, 0.0));
        for _ in 0..20 {
            frame(&ctx, &mut f.app, Vec::new());
        }
        let after = f.app.navigate.dealt_for.map(|k| k.view);
        assert_ne!(before, after, "fixture: the moved view was dealt again");
        assert_eq!(label_of(&f.app, f.a), la, "a keeps its letter");
    }

    /// A label the field's line holds keeps its layer through a deal elsewhere,
    /// and no other layer takes it.
    ///
    /// flip: `deal` building `labels` without `pinned` → fails at "the line's label".
    #[test]
    fn a_label_written_in_the_line_keeps_its_layer() {
        let ctx = egui::Context::default();
        let mut f = fixture(&ctx);
        f.app.open_command_bar();
        frame(&ctx, &mut f.app, Vec::new());
        let la = label_of(&f.app, f.a);
        f.app.command_bar.as_mut().unwrap().text = format!("{la}.x = 5");
        f.app.entered_group = Some(f.g);
        f.app.session.selection.set_one(f.r1);
        frame(&ctx, &mut f.app, Vec::new());
        assert_eq!(
            dealt(&f.app),
            HashSet::from([f.r1, f.r2]),
            "fixture: dealt inside"
        );
        assert_eq!(f.app.leap_labels().get(&la), Some(&f.a), "the line's label");
        assert!(
            f.app.navigate.deal.iter().all(|d| d.label != la),
            "taken by nothing else"
        );
    }

    /// Over 81 candidates, only the current layer's children are labelled, with
    /// the current layer itself, which the bar's subject pill deals a label.
    ///
    /// flip: dropping the `LABELS_MAX` fallback in `deal` → fails at the set.
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
        frame(&ctx, &mut f.app, Vec::new());
        let view = f.app.session.camera.viewport(f.app.canvas_px).view;
        assert!(
            f.app.leap_candidates(view, None).len() > LABELS_MAX,
            "fixture: more candidates than labels"
        );
        tap(&ctx, &mut f.app, egui::Key::Backtick);
        assert_eq!(dealt(&f.app), HashSet::from([f.r1, f.r2, f.g]));
    }

    /// The yellow closed outlines a frame of the label overlay alone paints, with
    /// the pointer on `pointed`.
    fn solid(ctx: &egui::Context, app: &MastikApp, pointed: Option<NodeId>) -> usize {
        let out = ctx.run_ui(egui::RawInput::default(), |ui| {
            app.draw_leap_labels(ui.painter(), app.canvas_rect, app.canvas_ppp, pointed);
        });
        let yellow = egui::epaint::ColorMode::Solid(color::leap());
        out.shapes
            .iter()
            .filter(|c| matches!(&c.shape, egui::Shape::Path(p) if p.closed && p.stroke.color == yellow))
            .count()
    }

    /// In Pick every label shows, and after a first letter only those it still
    /// matches keep an outline; the group's are dashed.
    ///
    /// flip: `draw_leap_labels` outlining every dealt layer → fails at the count
    /// after the first letter.
    #[test]
    fn the_first_letter_keeps_outlines_only_on_what_it_matches() {
        let ctx = egui::Context::default();
        let mut f = fixture(&ctx);
        crowd(&ctx, &mut f);
        tap(&ctx, &mut f.app, egui::Key::Backtick);
        let all = solid(&ctx, &f.app, None);
        assert_eq!(
            all,
            f.app.navigate.deal.len() - 1,
            "every solid one; the group dashed"
        );
        let first = label_of(&f.app, f.a).chars().next().unwrap();
        tap(&ctx, &mut f.app, letter_key(first));
        let left = f
            .app
            .navigate
            .deal
            .iter()
            .filter(|d| d.label.starts_with(first) && d.id != f.g)
            .count();
        assert!(left < all, "fixture: the letter rules outlines out");
        assert_eq!(
            solid(&ctx, &f.app, None),
            left,
            "only what the letter still matches"
        );
    }

    /// In Type no label shows until one is offered, except the one on the layer
    /// under the pointer, so it can be named.
    ///
    /// flip: `draw_leap_labels` returning in Type when nothing is offered → fails
    /// at "the pointed layer's".
    #[test]
    fn in_type_the_pointed_layer_shows_its_label_alone() {
        let ctx = egui::Context::default();
        let mut f = fixture(&ctx);
        f.app.open_command_bar();
        frame(&ctx, &mut f.app, Vec::new());
        assert_eq!(
            solid(&ctx, &f.app, None),
            0,
            "none while nothing is offered"
        );
        assert_eq!(solid(&ctx, &f.app, Some(f.a)), 1, "the pointed layer's");
    }

    /// The canvas stays live under the bar: a click selects, and the bar keeps
    /// its focus.
    ///
    /// flip: `canvas_ui` skipping the tools' input while the bar is open → fails at
    /// "the click selected".
    #[test]
    fn a_click_under_the_open_bar_selects() {
        let ctx = egui::Context::default();
        let mut f = fixture(&ctx);
        f.app.open_command_bar();
        frame(&ctx, &mut f.app, Vec::new());
        let at = centre_of(&f.app, f.a);
        click(&ctx, &mut f.app, at, egui::Modifiers::NONE);
        assert_eq!(selected(&f.app), [f.a], "the click selected");
        assert!(f.app.command_bar.is_some(), "the bar stayed");
    }

    /// In Type an `Alt`-click writes the layer's label into the line and leaves
    /// the selection as it was.
    ///
    /// flip: `bar_alt_click` not setting `gesture_cancelled` → fails at "the
    /// selection untouched".
    #[test]
    fn an_alt_click_writes_the_label_into_the_line() {
        let ctx = egui::Context::default();
        let mut f = fixture(&ctx);
        f.app.open_command_bar();
        frame(&ctx, &mut f.app, Vec::new());
        f.app.command_bar.as_mut().unwrap().text = ".x = ".into();
        frame(&ctx, &mut f.app, Vec::new());
        let at = centre_of(&f.app, f.a);
        click(&ctx, &mut f.app, at, egui::Modifiers::ALT);
        let la = label_of(&f.app, f.a);
        let text = f.app.command_bar.as_ref().map(|b| b.text.clone());
        assert_eq!(text, Some(format!(".x = {la}")), "the label at the caret");
        assert_eq!(selected(&f.app), [f.b], "the selection untouched");
    }

    /// The bar open with `line` in its field, the caret at its end. Written as
    /// a pick writes, so no completion list opens on it.
    fn bar_with(ctx: &egui::Context, f: &mut F, line: &str) {
        f.app.open_command_bar();
        frame(ctx, &mut f.app, Vec::new());
        f.app
            .command_bar
            .as_mut()
            .unwrap()
            .write_at_caret(line.into());
        frame(ctx, &mut f.app, Vec::new());
        let text = f.app.command_bar.as_ref().map(|b| b.text.clone());
        assert_eq!(text.as_deref(), Some(line), "fixture: the line written");
    }

    /// `` ` `` where a value goes picks to name: a label writes its layer into
    /// the line and gives the keys back, the selection untouched.
    ///
    /// flip: `leap_keys` landing on the layer whatever the pick → fails at "the
    /// label at the caret".
    #[test]
    fn a_backtick_where_a_value_goes_names_a_layer_in_the_line() {
        let ctx = egui::Context::default();
        let mut f = fixture(&ctx);
        bar_with(&ctx, &mut f, ".x = ");
        tap(&ctx, &mut f.app, egui::Key::Backtick);
        assert!(f.app.naming_pick(), "picks to name");
        let la = label_of(&f.app, f.a);
        spell(&ctx, &mut f.app, &la, egui::Modifiers::NONE);
        frame(&ctx, &mut f.app, Vec::new());
        let text = f.app.command_bar.as_ref().map(|b| b.text.clone());
        assert_eq!(text, Some(format!(".x = {la}")), "the label at the caret");
        assert!(!f.app.picking(), "the keys back with the field");
        assert_eq!(selected(&f.app), [f.b], "the selection untouched");
    }

    /// `` ` `` after a value, or on an empty line, picks to select.
    ///
    /// flip: `pick_from_line` naming on any line with text → fails at "after a
    /// value".
    #[test]
    fn a_backtick_after_a_value_picks_to_select() {
        let ctx = egui::Context::default();
        let mut f = fixture(&ctx);
        bar_with(&ctx, &mut f, ".x = 20");
        tap(&ctx, &mut f.app, egui::Key::Backtick);
        assert!(f.app.picking() && !f.app.naming_pick(), "after a value");
        let la = label_of(&f.app, f.a);
        spell(&ctx, &mut f.app, &la, egui::Modifiers::NONE);
        assert_eq!(selected(&f.app), [f.a], "the label selects");
        let text = f.app.command_bar.as_ref().map(|b| b.text.clone());
        assert_eq!(text.as_deref(), Some(".x = 20"), "and the line is kept");
    }

    /// While the bar picks to name, a plain click on a layer names it too.
    ///
    /// flip: `bar_alt_click` asking for `Alt` while naming → fails at "the label
    /// at the caret".
    #[test]
    fn a_click_while_naming_writes_the_label_into_the_line() {
        let ctx = egui::Context::default();
        let mut f = fixture(&ctx);
        bar_with(&ctx, &mut f, ".x = ");
        tap(&ctx, &mut f.app, egui::Key::Backtick);
        assert!(f.app.naming_pick(), "fixture: picks to name");
        let at = centre_of(&f.app, f.a);
        click(&ctx, &mut f.app, at, egui::Modifiers::NONE);
        let la = label_of(&f.app, f.a);
        let text = f.app.command_bar.as_ref().map(|b| b.text.clone());
        assert_eq!(text, Some(format!(".x = {la}")), "the label at the caret");
        assert!(!f.app.picking(), "the keys back with the field");
        assert_eq!(selected(&f.app), [f.b], "the selection untouched");
    }

    /// `Backspace` just after a label in the line takes the whole label.
    ///
    /// flip: dropping the bar's whole-pill `Backspace` → fails at the text.
    #[test]
    fn backspace_takes_a_label_whole() {
        let ctx = egui::Context::default();
        let mut f = fixture(&ctx);
        crowd(&ctx, &mut f);
        f.app.open_command_bar();
        frame(&ctx, &mut f.app, Vec::new());
        let la = label_of(&f.app, f.a);
        assert_eq!(la.len(), 2, "fixture: a two-letter label");
        bar_with(&ctx, &mut f, ".x = ");
        // Written as a pick writes it: typed, it is a word still being completed.
        f.app
            .command_bar
            .as_mut()
            .unwrap()
            .write_at_caret(la.clone());
        frame(&ctx, &mut f.app, Vec::new());
        let text = f.app.command_bar.as_ref().map(|b| b.text.clone());
        assert_eq!(
            text,
            Some(format!(".x = {la}")),
            "fixture: the label written"
        );
        tap(&ctx, &mut f.app, egui::Key::Backspace);
        let text = f.app.command_bar.as_ref().map(|b| b.text.clone());
        assert_eq!(text.as_deref(), Some(".x = "));
    }

    /// `Backspace` at the line's start takes the subject's pill: the selection
    /// goes, and the bar stays open.
    ///
    /// flip: dropping the bar's subject-pill `Backspace` → fails at "the
    /// selection goes".
    #[test]
    fn backspace_at_the_start_takes_the_subject() {
        let ctx = egui::Context::default();
        let mut f = fixture(&ctx);
        bar_with(&ctx, &mut f, "");
        assert_eq!(selected(&f.app), [f.b], "fixture: b is the subject");
        tap(&ctx, &mut f.app, egui::Key::Backspace);
        assert!(selected(&f.app).is_empty(), "the selection goes");
        assert!(f.app.command_bar.is_some(), "the bar stays open");
    }

    /// The singles are the nine home-row letters, each once, index fingers first.
    #[test]
    fn the_singles_are_the_home_row_easiest_first() {
        let singles = label_singles();
        assert_eq!(singles.len(), HOME_ROW.len());
        assert_eq!(&singles[..2], ["f", "j"]);
        assert_eq!(singles.last().unwrap(), "a");
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
}
