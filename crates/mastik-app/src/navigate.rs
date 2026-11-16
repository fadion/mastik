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
    /// The label book, lowercase label to layer: every label given since the
    /// document came forward, each kept by its layer (`deal`).
    labels: HashMap<String, NodeId>,
    /// The deal each layer was last in view for, by `tick`: the oldest out of
    /// view gives up its label first once the book is full.
    seen_at: HashMap<NodeId, u64>,
    /// Counts deals.
    tick: u64,
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
    /// Digits typed ahead of an arrow or `.`: how many times it runs.
    count: Option<u32>,
    /// `m` or `'` pressed, waiting for the mark's letter.
    marking: Option<MarkKey>,
    /// The name filter `/` opened, while it holds.
    filter: Option<Filter>,
}

/// What a mark's letter does once it comes.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum MarkKey {
    /// `m`: remember where things stand under the letter.
    Set,
    /// `'`: go back to what the letter remembers; `''` to before the last jump.
    Jump,
}

/// The name filter: only the layers whose drawn name holds `text` keep their
/// labels. While `typing`, every key types into it; after `Enter` the letters
/// pick among what it left.
struct Filter {
    text: String,
    typing: bool,
    /// The layers it leaves, in reading order, each with a label in the deal.
    matches: Vec<NodeId>,
}

/// Where the work stood: the page, the view, the selection and the entered
/// group. A mark holds one (`m` + letter), and a jump to one leaves the place
/// it left behind for `''`.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct Place {
    page: NodeId,
    camera: crate::view::Camera,
    selection: Vec<NodeId>,
    entered: Option<NodeId>,
}

/// One document's marks, kept with it for the sitting and never saved.
#[derive(Clone, Debug, Default)]
pub(crate) struct Marks {
    by_letter: HashMap<char, Place>,
    /// Where the last jump left from, for `''`.
    back: Option<Place>,
}

/// The most a count runs a key: a two-digit count.
const COUNT_MAX: u32 = 99;

impl Navigate {
    /// Drop whatever is half typed: a label's first letter, a count, a mark's
    /// prefix and the name filter.
    fn clear_pending(&mut self) {
        self.typed.clear();
        self.count = None;
        self.marking = None;
        self.filter = None;
    }

    /// Whether anything is half typed, which `Escape` clears before it gives
    /// the keys back to the field.
    fn pending(&self) -> bool {
        !self.typed.is_empty()
            || self.count.is_some()
            || self.marking.is_some()
            || self.filter.is_some()
    }
}

struct On {
    /// The tool to give back when the bar closes.
    prior: Tool,
    /// Whether the canvas has the keys (Pick) rather than the field (Type).
    picking: bool,
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

/// How hard a home-row letter is to reach: index fingers, then middle, ring, the
/// stretch and the pinkies.
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

/// The box in `boxes` nearest `from` toward `step`: of those whose centre lies
/// past `from`'s that way, the least gap along the step plus twice the gap
/// across it, so a layer in line beats a nearer one off to the side; ties go to
/// the centre most in line. `boxes` in reading order, which breaks any tie left.
fn nearest_toward(step: Step, from: Rect, boxes: &[(NodeId, Rect)]) -> Option<NodeId> {
    // Each box as (along, across): along grows toward the step.
    let axes = |r: Rect| -> ((f64, f64), (f64, f64)) {
        match step {
            Step::Right => ((r.x0, r.x1), (r.y0, r.y1)),
            Step::Left => ((-r.x1, -r.x0), (r.y0, r.y1)),
            Step::Down => ((r.y0, r.y1), (r.x0, r.x1)),
            Step::Up => ((-r.y1, -r.y0), (r.x0, r.x1)),
        }
    };
    let mid = |(a, b): (f64, f64)| (a + b) / 2.0;
    let (f_along, f_across) = axes(from);
    let score = |r: Rect| -> Option<(f64, f64)> {
        let (along, across) = axes(r);
        if mid(along) <= mid(f_along) + 0.5 {
            return None;
        }
        let gap = (along.0 - f_along.1).max(0.0);
        let side = (across.0 - f_across.1).max(f_across.0 - across.1).max(0.0);
        Some((gap + 2.0 * side, (mid(across) - mid(f_across)).abs()))
    };
    boxes
        .iter()
        .filter_map(|(id, r)| Some((*id, score(*r)?)))
        .min_by(|a, b| a.1.0.total_cmp(&b.1.0).then(a.1.1.total_cmp(&b.1.1)))
        .map(|(id, _)| id)
}

/// The letter `key` types, lowercase, for a mark's name.
fn letter(key: egui::Key) -> Option<char> {
    let name = key.name();
    let mut chars = name.chars();
    match (chars.next(), chars.next()) {
        (Some(c), None) if c.is_ascii_alphabetic() => Some(c.to_ascii_lowercase()),
        _ => None,
    }
}

/// The digit `key` types, for a count.
fn digit(key: egui::Key) -> Option<u32> {
    use egui::Key as K;
    [
        K::Num0,
        K::Num1,
        K::Num2,
        K::Num3,
        K::Num4,
        K::Num5,
        K::Num6,
        K::Num7,
        K::Num8,
        K::Num9,
    ]
    .iter()
    .position(|k| *k == key)
    .map(|d| d as u32)
}

/// Whether `id` and every layer above it up to its page are shown.
fn shown(doc: &mastik_core::Document, id: NodeId) -> bool {
    std::iter::successors(Some(id), |c| doc.get(*c).and_then(|n| n.parent()))
        .take_while(|c| !doc.is_page(*c))
        .all(|c| doc.get(c).is_some_and(|n| n.visible()))
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
        });
        self.navigate.clear_pending();
        self.navigate.dealt_for = None;
    }

    /// The bar closing: the tool from before comes back, unless another was
    /// chosen by hand meanwhile or a text edit took the keys (arming a tool
    /// would end it), and the selection stays.
    pub(crate) fn end_bar_session(&mut self) {
        self.navigate.clear_pending();
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
        if let Some(on) = self.navigate.on.as_mut() {
            on.picking = picking;
        }
        self.navigate.clear_pending();
    }

    /// Forget the layers the labels hold as another document comes forward,
    /// which may share their ids.
    pub(crate) fn forget_navigate_layers(&mut self) {
        self.navigate.labels.clear();
        self.navigate.seen_at.clear();
        self.navigate.deal.clear();
        self.navigate.clear_pending();
        self.navigate.dealt_for = None;
        self.navigate.seen = None;
    }

    /// One key of Pick's keymap. A count typed ahead runs an arrow or `.` that
    /// many times, an arrow stopping at the last layer that way; any other key
    /// spends it.
    pub(crate) fn nav_action(&mut self, ctx: &egui::Context, nav: Nav) {
        let times = self.navigate.count.take().unwrap_or(1);
        match nav {
            Nav::Step(step, extend) => {
                for _ in 0..times {
                    if !self.nav_step(step, extend) {
                        break;
                    }
                }
            }
            Nav::StepIn => self.step_in(),
            Nav::StepOut => self.step_out(),
            Nav::Matching => self.select_matching(),
            Nav::Fit => self.zoom_to(self.document_bounds()),
            Nav::Zoom => self.zoom_to(self.selection_bounds()),
            Nav::Repeat => {
                for _ in 0..times {
                    self.repeat_bar_line(ctx);
                }
            }
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

    /// Move to the current layer's nearest sibling that way on the canvas
    /// ([`nearest_toward`]); with `extend`, add it to the selection rather than
    /// replace it. The canvas, not the Layers panel: z-order often runs against
    /// what the eye sees, and `↓` should go down. Nothing that way stays put.
    /// Answers whether it moved.
    fn nav_step(&mut self, step: Step, extend: bool) -> bool {
        let doc = &self.session.doc;
        let target = match self.nav_current() {
            // Nothing to move from: the scope's first layer.
            None => self.reading_order(self.nav_scope()).first().copied(),
            Some(at) => doc.get(at).and_then(|n| n.parent()).and_then(|p| {
                let from = self.session.preview_world_bounds(at)?;
                let boxes: Vec<(NodeId, Rect)> = self
                    .reading_order(p)
                    .into_iter()
                    .filter(|id| *id != at)
                    .filter_map(|id| Some((id, self.session.preview_world_bounds(id)?)))
                    .collect();
                nearest_toward(step, from, &boxes)
            }),
        };
        if let Some(target) = target {
            self.nav_land(target, extend);
        }
        target.is_some()
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
        let shown = kids
            .iter()
            .copied()
            .filter(|id| doc.get(*id).is_some_and(|n| n.visible()))
            .collect();
        self.reading_sort(shown)
    }

    /// `ids` in reading order on the canvas, as [`Self::reading_order`] reads
    /// siblings; those with no box are left out.
    fn reading_sort(&self, ids: Vec<NodeId>) -> Vec<NodeId> {
        let mut boxes: Vec<(NodeId, Rect)> = ids
            .into_iter()
            .filter_map(|id| Some((id, self.session.preview_world_bounds(id)?)))
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
            Some(id) => {
                self.enter_container(id, None);
                // On the first child the eye reads, as `↓` walks on from it, not
                // the topmost in z-order the editor's Enter lands on.
                if let Some(first) = self.reading_order(id).first().copied() {
                    self.session.selection.set_one(first);
                }
            }
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
    /// when there is one. **A label belongs to its layer**: once given, a layer
    /// keeps it for as long as the document is open — through pans, zooms,
    /// entered groups and edits — so a label never changes under the eye. Labels
    /// are always two letters, unique across the document; a layer meeting its
    /// first deal takes the easiest free one, the newcomers in reading order.
    /// Only when all [`LABELS_MAX`] are taken is one moved ([`Self::free_label`]).
    /// A view with more than [`LABELS_MAX`] candidates labels only the children of
    /// the current layer.
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
        ids.truncate(LABELS_MAX);
        // Newcomers take the easy labels in the order the eye reads them.
        let ids = self.reading_sort(ids);
        self.navigate.tick += 1;
        let tick = self.navigate.tick;
        for id in &ids {
            self.navigate.seen_at.insert(*id, tick);
        }
        let deal: Vec<Dealt> = ids
            .iter()
            .filter_map(|id| {
                let label = self.book_label(*id, &ids)?;
                Some(Dealt {
                    label,
                    id: *id,
                    container: self.is_container(*id),
                })
            })
            .collect();
        self.navigate.deal = deal;
        // The filter's matches are read against the view just dealt.
        self.refilter();
    }

    /// Whether `id` is drawn with a container's dashed outline and dark label.
    fn is_container(&self, id: NodeId) -> bool {
        self.session.doc.get(id).is_some_and(|n| {
            matches!(
                n.kind(),
                NodeKind::Group | NodeKind::Boolean { .. } | NodeKind::Artboard { .. }
            )
        })
    }

    /// `id`'s label from the book, giving it one when it has none; `showing`
    /// are the layers in view, whose labels never move.
    fn book_label(&mut self, id: NodeId, showing: &[NodeId]) -> Option<String> {
        if let Some((k, _)) = self.navigate.labels.iter().find(|(_, v)| **v == id) {
            return Some(k.clone());
        }
        let label = self.free_label(showing)?;
        self.navigate.labels.insert(label.clone(), id);
        Some(label)
    }

    /// The easiest label no layer holds; with all of them held, the label of a
    /// deleted layer, else of the layer seen longest ago that is out of view
    /// (`showing`) and not named in the field's line. `None` only when every
    /// label is in view or in the line.
    fn free_label(&mut self, showing: &[NodeId]) -> Option<String> {
        if let Some(l) = label_pairs()
            .into_iter()
            .find(|l| !self.navigate.labels.contains_key(l))
        {
            return Some(l);
        }
        let line = self
            .command_bar
            .as_ref()
            .map(|b| b.text.clone())
            .unwrap_or_default();
        let held = words(&line);
        let doc = &self.session.doc;
        let seen = &self.navigate.seen_at;
        let (label, _) = self
            .navigate
            .labels
            .iter()
            .filter(|(k, v)| !held.contains(k.as_str()) && !showing.contains(v))
            .min_by_key(|(k, v)| {
                let age = if doc.contains(**v) {
                    seen.get(*v).copied().unwrap_or(0)
                } else {
                    0
                };
                (doc.contains(**v), age, (*k).clone())
            })
            .map(|(k, v)| (k.clone(), *v))?;
        self.navigate.labels.remove(&label);
        Some(label)
    }

    /// The label `id` wears, giving it one from the book when it has none, so a
    /// line can name a layer the deal did not reach. It is drawn with the deal's.
    pub(crate) fn label_for(&mut self, id: NodeId) -> Option<String> {
        if let Some((k, _)) = self.navigate.labels.iter().find(|(_, v)| **v == id) {
            return Some(k.clone());
        }
        let showing: Vec<NodeId> = self.navigate.deal.iter().map(|d| d.id).collect();
        let label = self.book_label(id, &showing)?;
        let container = self.is_container(id);
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

    /// In Pick, take the keys Pick reads itself out of this frame's events before
    /// the keymap sees them, one at a time in order ([`Self::pick_key`]): the
    /// labels' letters, a count's digits, a mark's `m`, `'` and letter, and the
    /// name filter's `/` and everything typed into it. Answers whether a key
    /// press was left for the keymap, which ends a count it did not use.
    pub(crate) fn leap_keys(&mut self, ctx: &egui::Context) -> bool {
        if !self.picking() || self.mode != Mode::Normal {
            return false;
        }
        let events = ctx.input_mut(|i| std::mem::take(&mut i.events));
        let mut kept = Vec::with_capacity(events.len());
        // The text a key spent here also types, read with its key.
        let mut skip: Option<&'static str> = None;
        let mut passed = false;
        for e in events {
            let spent = match &e {
                egui::Event::Text(t) if skip == Some(t.as_str()) => {
                    skip = None;
                    true
                }
                egui::Event::Text(t) => self.filter_text(t),
                egui::Event::Key {
                    key,
                    pressed: true,
                    modifiers,
                    ..
                } if !modifiers.command && !modifiers.alt => {
                    let spent = self.pick_key(*key, *modifiers, &mut skip);
                    passed |= !spent;
                    spent
                }
                _ => false,
            };
            if !spent {
                kept.push(e);
            }
        }
        ctx.input_mut(|i| i.events = kept);
        passed
    }

    /// After the keymap has read a frame of Pick's keys: a count the keys
    /// left over did not use ends with them.
    pub(crate) fn pick_keys_read(&mut self, passed: bool) {
        if passed {
            self.navigate.count = None;
        }
    }

    /// Text typed while the name filter is open goes into it.
    fn filter_text(&mut self, t: &str) -> bool {
        let Some(f) = self.navigate.filter.as_mut().filter(|f| f.typing) else {
            return false;
        };
        f.text.push_str(t);
        self.refilter();
        true
    }

    /// One key press in Pick, answering whether Pick spent it. In order: the
    /// filter while typed takes every key; a mark's prefix takes the next
    /// letter; `Escape` clears anything half typed; then the digits, `m`, `'`,
    /// `/` and the labels' letters. `skip` takes the text the key will also
    /// type, so it does not reach the filter it just opened.
    fn pick_key(
        &mut self,
        key: egui::Key,
        m: egui::Modifiers,
        skip: &mut Option<&'static str>,
    ) -> bool {
        use egui::Key as K;
        if let Some(f) = self.navigate.filter.as_mut().filter(|f| f.typing) {
            match key {
                K::Backspace if f.text.is_empty() => self.navigate.filter = None,
                K::Backspace => {
                    f.text.pop();
                    self.refilter();
                }
                K::Escape => self.navigate.filter = None,
                K::Enter => match *f.matches.as_slice() {
                    [one] => {
                        self.navigate.filter = None;
                        self.nav_land(one, m.shift);
                    }
                    [] => {}
                    _ => f.typing = false,
                },
                // Its text arrives as `Event::Text`.
                _ => {}
            }
            return true;
        }
        if let Some(mark) = self.navigate.marking.take() {
            if mark == MarkKey::Jump && key == K::Quote && !m.shift {
                *skip = Some("'");
                self.jump_back();
                return true;
            }
            if let Some(c) = letter(key) {
                match mark {
                    MarkKey::Set => self.set_mark(c),
                    MarkKey::Jump => self.jump_to_mark(c),
                }
                return true;
            }
            // Escape only cancels the prefix; any other key is read as itself.
            if key == K::Escape {
                return true;
            }
        }
        if key == K::Escape && self.navigate.pending() {
            self.navigate.clear_pending();
            return true;
        }
        if !m.shift
            && let Some(d) = digit(key)
            && (d > 0 || self.navigate.count.is_some())
        {
            let n = self.navigate.count.unwrap_or(0) * 10 + d;
            self.navigate.count = Some(n.min(COUNT_MAX));
            self.navigate.typed.clear();
            return true;
        }
        if key == K::Backspace && self.navigate.count.is_some() {
            self.navigate.count = self.navigate.count.map(|n| n / 10).filter(|n| *n > 0);
            return true;
        }
        if !m.shift && matches!(key, K::M | K::Quote) {
            self.navigate.typed.clear();
            self.navigate.count = None;
            self.navigate.marking = Some(match key {
                K::M => MarkKey::Set,
                _ => MarkKey::Jump,
            });
            *skip = Some(if key == K::M { "m" } else { "'" });
            return true;
        }
        if !m.shift && key == K::Slash {
            self.navigate.typed.clear();
            self.navigate.count = None;
            match self.navigate.filter.as_mut() {
                // `/` again goes back to typing what is there.
                Some(f) => f.typing = true,
                None => {
                    self.navigate.filter = Some(Filter {
                        text: String::new(),
                        typing: true,
                        matches: Vec::new(),
                    });
                }
            }
            *skip = Some("/");
            return true;
        }
        if key == K::Backspace && !self.navigate.typed.is_empty() {
            self.navigate.typed.pop();
            return true;
        }
        let Some(c) = home_letter(key) else {
            return false;
        };
        self.navigate.count = None;
        let typed = format!("{}{c}", self.navigate.typed);
        let open = |d: &&Dealt| self.filter_admits(d.id);
        let hit = self
            .navigate
            .deal
            .iter()
            .filter(open)
            .find(|d| d.label == typed);
        if let Some(id) = hit.map(|d| d.id) {
            self.navigate.typed.clear();
            self.navigate.filter = None;
            self.nav_land(id, m.shift);
        } else if self
            .navigate
            .deal
            .iter()
            .filter(open)
            .any(|d| d.label.starts_with(&typed))
        {
            self.navigate.typed = typed;
        } else {
            self.navigate.typed.clear();
        }
        true
    }

    /// Whether the name filter leaves `id`'s label: always with no filter or
    /// nothing typed in it.
    fn filter_admits(&self, id: NodeId) -> bool {
        self.navigate
            .filter
            .as_ref()
            .is_none_or(|f| f.text.is_empty() || f.matches.contains(&id))
    }

    /// The layers the name filter leaves: every one in view inside the scope,
    /// at any depth, shown and unlocked, whose name as drawn holds the text,
    /// case aside. Each is given a label in the deal if it has none, so the
    /// letters can pick it.
    fn refilter(&mut self) {
        let Some(text) = self.navigate.filter.as_ref().map(|f| f.text.to_lowercase()) else {
            return;
        };
        let mut ids = Vec::new();
        if !text.is_empty() {
            let doc = &self.session.doc;
            let scope = self.nav_scope();
            let view = self.session.camera.viewport(self.canvas_px).view;
            ids = mastik_core::nodes_in_view(&self.session.resolved, self.session.page, view)
                .into_iter()
                .filter(|id| *id != scope && mastik_core::is_within(doc, *id, scope))
                .filter(|id| shown(doc, *id) && !mastik_core::is_effectively_locked(doc, *id))
                .filter(|id| {
                    crate::private::layer(doc, *id)
                        .to_lowercase()
                        .contains(&text)
                })
                .collect();
            ids = self.reading_sort(ids);
            ids.truncate(LABELS_MAX);
        }
        for id in &ids {
            if !self.navigate.deal.iter().any(|d| d.id == *id) {
                let _ = self.label_for(*id);
            }
        }
        if let Some(f) = self.navigate.filter.as_mut() {
            f.matches = ids;
        }
    }

    /// Where things stand now, for a mark or for `''`.
    fn place(&self) -> Place {
        Place {
            page: self.session.page,
            camera: self.session.camera,
            selection: self.session.selection.ids().to_vec(),
            entered: self.entered_group,
        }
    }

    /// Go back to `place`: its page, view, entered group and what of its
    /// selection is still there. A page gone since says so and goes nowhere.
    fn go_to_place(&mut self, place: &Place) -> bool {
        if !self.session.doc.is_page(place.page) {
            self.session.info("That mark's page is gone");
            return false;
        }
        self.go_to_page(place.page);
        self.session.camera = place.camera;
        let doc = &self.session.doc;
        self.entered_group = place.entered.filter(|g| doc.contains(*g));
        let kept: Vec<NodeId> = place
            .selection
            .iter()
            .copied()
            .filter(|id| doc.contains(*id))
            .collect();
        self.session.selection.set(kept);
        // Deal for the view arrived at now, not once it has rested.
        self.navigate.dealt_for = None;
        true
    }

    /// `m` + `c`: remember where things stand under `c`.
    fn set_mark(&mut self, c: char) {
        let here = self.place();
        self.session.marks.by_letter.insert(c, here);
        self.session
            .info(format!("Mark {} set", c.to_ascii_uppercase()));
    }

    /// `'` + `c`: go to mark `c`, leaving where things stood for `''`.
    fn jump_to_mark(&mut self, c: char) {
        let Some(there) = self.session.marks.by_letter.get(&c).cloned() else {
            self.session
                .info(format!("No mark {}", c.to_ascii_uppercase()));
            return;
        };
        let here = self.place();
        if self.go_to_place(&there) {
            self.session.marks.back = Some(here);
        }
    }

    /// `''`: back to where the last jump left from, which then becomes the way
    /// back again.
    fn jump_back(&mut self) {
        let Some(there) = self.session.marks.back.clone() else {
            self.session.info("No jump to go back from");
            return;
        };
        let here = self.place();
        if self.go_to_place(&there) {
            self.session.marks.back = Some(here);
        }
    }

    /// What Pick has half typed, as the bar's field shows it at the line's
    /// right edge: a count, a mark's prefix, the name filter, a label's first
    /// letter.
    pub(crate) fn pick_typed(&self) -> String {
        let n = &self.navigate;
        if let Some(f) = &n.filter {
            return format!("/{}", f.text);
        }
        match n.marking {
            Some(MarkKey::Set) => return "M".to_owned(),
            Some(MarkKey::Jump) => return "'".to_owned(),
            None => {}
        }
        let count = n.count.map(|c| c.to_string()).unwrap_or_default();
        format!("{count}{}", n.typed.to_uppercase())
    }

    /// The bar's footer while it picks: the keys that mean something now.
    pub(crate) fn pick_hint(&self) -> &'static str {
        let n = &self.navigate;
        match (&n.filter, n.marking) {
            (Some(f), _) if f.typing => "Type a name · Enter picks · Esc clears",
            (Some(_), _) => "Labels pick what the name left · / edits it · Esc clears",
            (None, Some(MarkKey::Set)) => "A letter names the mark",
            (None, Some(MarkKey::Jump)) => "A letter goes to its mark · ' goes back",
            (None, None) => "Labels pick · Enter in · Backspace out · / name · m mark · ' jump",
        }
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
            // The name filter leaves only what it matched.
            .filter(|d| self.filter_admits(d.id))
            .collect();
        let matched = |d: &Dealt| d.label.starts_with(&typed);
        // What is picked wears the selection's blue, label and outline both, so
        // it stands apart from every other label at a glance.
        let chosen = |d: &Dealt| self.session.selection.contains(d.id);
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
                let hue = if chosen(d) {
                    color::SELECT
                } else {
                    color::leap()
                };
                outline(painter, *b, d.container, hue);
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
            let hue = if chosen(d) {
                color::SELECT
            } else if d.container {
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
    /// its squares — two letters each, the easiest in reading order.
    ///
    /// flip: `deal` giving the easiest labels nearest the current layer first,
    /// rather than in `reading_sort`'s order → fails at "in reading order".
    #[test]
    fn pick_deals_two_letter_labels_in_reading_order() {
        let ctx = egui::Context::default();
        let mut f = fixture(&ctx);
        tap(&ctx, &mut f.app, egui::Key::Backtick);
        assert_eq!(
            dealt(&f.app),
            HashSet::from([f.a, f.g, f.b]),
            "the group, not its squares"
        );
        let pairs = label_pairs();
        let got = [f.a, f.g, f.b].map(|id| label_of(&f.app, id));
        assert_eq!(got.as_slice(), &pairs[..3], "in reading order");
    }

    /// In a crowded view every label is still two letters, and the first letter
    /// narrows.
    ///
    /// flip: `leap_keys` not keeping the first letter → fails at "the first
    /// letter narrows".
    #[test]
    fn the_first_letter_of_a_label_narrows() {
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
            first.to_ascii_uppercase().to_string(),
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

    /// `Enter` steps into the picked group, on the child the eye reads first,
    /// and the labels follow onto its children; `Backspace` steps out, selecting
    /// the group, and they come back.
    ///
    /// flip: `step_out` leaving `entered_group` as it was → fails at "out of the
    /// group"; `step_in` keeping `enter_container`'s topmost child → fails at "on
    /// its top-left child".
    #[test]
    fn enter_steps_in_and_backspace_steps_out_and_the_labels_follow() {
        let ctx = egui::Context::default();
        let mut f = fixture(&ctx);
        tap(&ctx, &mut f.app, egui::Key::Backtick);
        let lg = label_of(&f.app, f.g);
        spell(&ctx, &mut f.app, &lg, egui::Modifiers::NONE);

        tap(&ctx, &mut f.app, egui::Key::Enter);
        assert_eq!(f.app.entered_group, Some(f.g), "into the group");
        // r2 is the topmost in z-order; r1 is to its left.
        assert_eq!(selected(&f.app), [f.r1], "on its top-left child");
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

    /// `←` and `→` step to the nearest sibling that way on the canvas, `→` onto a
    /// group without stepping into it; `↑` with nothing above stays; Shift adds.
    ///
    /// flip: `nearest_toward` dropping its "past the centre" test → fails at
    /// "nothing to the right of b".
    #[test]
    fn the_arrows_walk_siblings_and_shift_adds() {
        let ctx = egui::Context::default();
        let mut f = fixture(&ctx);
        tap(&ctx, &mut f.app, egui::Key::Backtick);
        // The row reads a, g, b left to right, though the panel lists b, g, a.
        tap(&ctx, &mut f.app, egui::Key::ArrowRight);
        assert_eq!(selected(&f.app), [f.b], "nothing to the right of b");
        tap(&ctx, &mut f.app, egui::Key::ArrowLeft);
        assert_eq!(selected(&f.app), [f.g], "the group, nearer than a");
        shift_tap(&ctx, &mut f.app, egui::Key::ArrowLeft);
        assert_eq!(selected(&f.app), [f.g, f.a], "and a, added");
        tap(&ctx, &mut f.app, egui::Key::ArrowRight);
        assert_eq!(selected(&f.app), [f.g], "a plain step selects alone");
        assert_eq!(
            f.app.entered_group, None,
            "→ onto a group does not enter it"
        );
        tap(&ctx, &mut f.app, egui::Key::ArrowUp);
        assert_eq!(selected(&f.app), [f.g], "nothing above the row");
    }

    /// Four squares in a grid: each arrow goes to the neighbour that way, and a
    /// layer in line beats a nearer one off the line.
    ///
    /// flip: `nearest_toward` scoring the gap along alone → fails at "in line,
    /// past the nearer one off to the side".
    #[test]
    fn the_arrows_move_across_a_grid_by_direction() {
        let r = |x: f64, y: f64| Rect::new(x, y, x + 20.0, y + 20.0);
        let id = |seq| NodeId { actor: 1, seq };
        let (tl, tr, bl, br) = (id(1), id(2), id(3), id(4));
        let boxes = [
            (tl, r(0.0, 0.0)),
            (tr, r(100.0, 0.0)),
            (bl, r(0.0, 100.0)),
            (br, r(100.0, 100.0)),
        ];
        let from = |me: NodeId| {
            let at = boxes.iter().find(|(i, _)| *i == me).unwrap().1;
            let rest: Vec<_> = boxes.iter().copied().filter(|(i, _)| *i != me).collect();
            move |s| nearest_toward(s, at, &rest)
        };
        assert_eq!(from(tl)(Step::Right), Some(tr));
        assert_eq!(from(tl)(Step::Down), Some(bl), "down, not along the row");
        assert_eq!(from(tl)(Step::Up), None);
        assert_eq!(from(tl)(Step::Left), None);
        assert_eq!(from(br)(Step::Up), Some(tr));
        assert_eq!(from(br)(Step::Left), Some(bl));
        // Just below and off to the right, against further down and in line.
        let near_side = (id(9), Rect::new(60.0, 30.0, 80.0, 50.0));
        let in_line = (id(10), Rect::new(0.0, 80.0, 20.0, 100.0));
        assert_eq!(
            nearest_toward(Step::Down, r(0.0, 0.0), &[near_side, in_line]),
            Some(id(10)),
            "in line, past the nearer one off to the side"
        );
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

    /// Pan until the view is dealt again.
    fn pan_and_rest(ctx: &egui::Context, app: &mut MastikApp, by: Vec2) {
        let before = app.navigate.dealt_for.map(|k| k.view);
        app.session.camera.pan_pixels(by);
        for _ in 0..20 {
            frame(ctx, app, Vec::new());
        }
        let after = app.navigate.dealt_for.map(|k| k.view);
        assert_ne!(before, after, "fixture: the moved view was dealt again");
    }

    /// A layer keeps its label through everything that deals again: a pan, a
    /// view crowded past nine, stepping into a group and out, closing and
    /// reopening the bar.
    ///
    /// flip: `deal` starting each deal from an empty book → fails at "through
    /// a pan".
    #[test]
    fn a_layer_keeps_its_label_through_every_deal() {
        let ctx = egui::Context::default();
        let mut f = fixture(&ctx);
        tap(&ctx, &mut f.app, egui::Key::Backtick);
        let la = label_of(&f.app, f.a);
        let lb = label_of(&f.app, f.b);
        let lg = label_of(&f.app, f.g);
        // `a` out of view, so `g` reads first; then back.
        pan_and_rest(&ctx, &mut f.app, Vec2::new(-300.0, 0.0));
        assert!(!dealt(&f.app).contains(&f.a), "fixture: a out of view");
        assert_eq!(label_of(&f.app, f.g), lg, "through a pan");
        pan_and_rest(&ctx, &mut f.app, Vec2::new(300.0, 0.0));
        assert_eq!(label_of(&f.app, f.a), la, "through a pan back");
        crowd(&ctx, &mut f);
        assert_eq!(label_of(&f.app, f.a), la, "through a crowded view");
        f.app.session.selection.set_one(f.g);
        tap(&ctx, &mut f.app, egui::Key::Enter);
        assert_eq!(f.app.entered_group, Some(f.g), "fixture: stepped in");
        tap(&ctx, &mut f.app, egui::Key::Backspace);
        assert_eq!(
            label_of(&f.app, f.a),
            la,
            "through a group stepped in and out"
        );
        f.app.end_bar_session();
        f.app.command_bar = None;
        frame(&ctx, &mut f.app, Vec::new());
        tap(&ctx, &mut f.app, egui::Key::Backtick);
        assert_eq!(label_of(&f.app, f.b), lb, "through the bar reopened");
    }

    /// With every label taken, a newcomer takes the label of the layer seen
    /// longest ago, never one in view or named in the field's line.
    ///
    /// flip: `free_label` not skipping the line's labels → fails at "not the
    /// line's".
    #[test]
    fn a_full_book_moves_only_a_label_out_of_view_and_out_of_the_line() {
        let ctx = egui::Context::default();
        let mut f = fixture(&ctx);
        bar_with(&ctx, &mut f, "");
        // The rest of the book on layers long gone from view: minted, never shown.
        let pairs = label_pairs();
        let taken: Vec<String> = f.app.leap_labels().keys().cloned().collect();
        let mut ids = mastik_core::IdSource::new(0xB00C);
        for l in pairs.iter().filter(|l| !taken.contains(l)) {
            let id = ids.mint();
            f.app.navigate.labels.insert(l.clone(), id);
            f.app.navigate.seen_at.insert(id, 0);
        }
        assert_eq!(
            f.app.leap_labels().len(),
            LABELS_MAX,
            "fixture: the book is full"
        );
        // Each absent layer is a deleted one, so all tie and the first by letter
        // goes first: the line holds that one.
        let held = pairs
            .iter()
            .filter(|l| !taken.contains(l))
            .min()
            .cloned()
            .expect("an absent layer's label");
        f.app.command_bar.as_mut().unwrap().text = format!("{held}.x = 1");
        let page = f.app.session.page;
        let fresh = f.app.session.ids.mint();
        f.app
            .session
            .commit(Transaction(vec![square(fresh, page, 3, 200.0, 0.0)]));
        let got = f.app.label_for(fresh).expect("a label");
        assert_ne!(got, held, "not the line's");
        assert!(!taken.contains(&got), "not one in view");
        assert_eq!(f.app.leap_labels().get(&got), Some(&fresh));
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

    /// The closed outlines in `hue` a frame of the label overlay alone paints,
    /// with the pointer on `pointed`.
    fn solid_in(
        ctx: &egui::Context,
        app: &MastikApp,
        pointed: Option<NodeId>,
        hue: egui::Color32,
    ) -> usize {
        let out = ctx.run_ui(egui::RawInput::default(), |ui| {
            app.draw_leap_labels(ui.painter(), app.canvas_rect, app.canvas_ppp, pointed);
        });
        let ink = egui::epaint::ColorMode::Solid(hue);
        out.shapes
            .iter()
            .filter(
                |c| matches!(&c.shape, egui::Shape::Path(p) if p.closed && p.stroke.color == ink),
            )
            .count()
    }

    /// The solid outlines, yellow and the picked layers' blue alike.
    fn solid(ctx: &egui::Context, app: &MastikApp, pointed: Option<NodeId>) -> usize {
        solid_in(ctx, app, pointed, color::leap()) + solid_in(ctx, app, pointed, color::SELECT)
    }

    /// The label pills a frame of the overlay paints on a ground of `hue`.
    fn pills_in(ctx: &egui::Context, app: &MastikApp, hue: egui::Color32) -> usize {
        let out = ctx.run_ui(egui::RawInput::default(), |ui| {
            app.draw_leap_labels(ui.painter(), app.canvas_rect, app.canvas_ppp, None);
        });
        out.shapes
            .iter()
            .filter(|c| matches!(&c.shape, egui::Shape::Rect(r) if r.fill == hue))
            .count()
    }

    /// The picked layer wears the selection's blue, its label and its outline,
    /// and the blue follows the pick; every other label stays yellow.
    ///
    /// flip: `draw_leap_labels` dropping its `chosen` hue for the label → fails
    /// at "the picked label is blue".
    #[test]
    fn the_picked_layer_wears_the_selection_blue() {
        let ctx = egui::Context::default();
        let mut f = fixture(&ctx);
        tap(&ctx, &mut f.app, egui::Key::Backtick);
        assert_eq!(selected(&f.app), [f.b], "fixture: b picked");
        assert_eq!(
            pills_in(&ctx, &f.app, color::SELECT),
            1,
            "the picked label is blue"
        );
        assert_eq!(
            solid_in(&ctx, &f.app, None, color::SELECT),
            1,
            "and its outline"
        );
        let la = label_of(&f.app, f.a);
        spell(&ctx, &mut f.app, &la, egui::Modifiers::SHIFT);
        assert_eq!(selected(&f.app), [f.b, f.a], "fixture: a added");
        assert_eq!(
            pills_in(&ctx, &f.app, color::SELECT),
            2,
            "both picked are blue"
        );
        tap(&ctx, &mut f.app, egui::Key::ArrowRight);
        assert_eq!(selected(&f.app), [f.g], "fixture: on to the group");
        assert_eq!(pills_in(&ctx, &f.app, color::SELECT), 1, "the blue follows");
        assert_eq!(
            solid_in(&ctx, &f.app, None, color::SELECT),
            0,
            "the group's outline is dashed, so no solid blue"
        );
        assert!(
            pills_in(&ctx, &f.app, color::leap()) >= 2,
            "a and b are back to yellow"
        );
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

    fn line(app: &MastikApp) -> String {
        app.command_bar
            .as_ref()
            .map(|b| b.text.clone())
            .unwrap_or_default()
    }

    /// A pick writes its current layer at the caret as it goes, replaces it as
    /// the pick moves on (a label, an arrow), and `` ` `` ends it with the label
    /// there. The pick selects as any pick does.
    ///
    /// flip: `pick_into_line` writing nothing → fails at "written as it goes".
    #[test]
    fn a_pick_writes_its_layer_into_the_line_as_it_goes() {
        let ctx = egui::Context::default();
        let mut f = fixture(&ctx);
        bar_with(&ctx, &mut f, ".x = ");
        tap(&ctx, &mut f.app, egui::Key::Backtick);
        assert!(f.app.picking(), "fixture: picking");
        assert_eq!(
            line(&f.app),
            ".x = ",
            "nothing written before the pick moves"
        );
        let la = label_of(&f.app, f.a);
        spell(&ctx, &mut f.app, &la, egui::Modifiers::NONE);
        frame(&ctx, &mut f.app, Vec::new());
        assert_eq!(line(&f.app), format!(".x = {la}"), "written as it goes");
        assert_eq!(selected(&f.app), [f.a], "and selected");
        tap(&ctx, &mut f.app, egui::Key::ArrowRight);
        frame(&ctx, &mut f.app, Vec::new());
        let lg = label_of(&f.app, f.g);
        assert_eq!(
            line(&f.app),
            format!(".x = {lg}"),
            "replaced as it moves on"
        );
        tap(&ctx, &mut f.app, egui::Key::Backtick);
        frame(&ctx, &mut f.app, Vec::new());
        assert!(!f.app.picking(), "`` ` `` ends the pick");
        assert_eq!(line(&f.app), format!(".x = {lg}"), "with the label there");
    }

    /// A click on the canvas while picking selects as it always does, and the
    /// pick writes what it selected.
    ///
    /// flip: `bar_alt_click` taking a plain press while picking → fails at
    /// "selected".
    #[test]
    fn a_click_while_picking_selects_and_writes() {
        let ctx = egui::Context::default();
        let mut f = fixture(&ctx);
        bar_with(&ctx, &mut f, ".x = ");
        tap(&ctx, &mut f.app, egui::Key::Backtick);
        let at = centre_of(&f.app, f.a);
        click(&ctx, &mut f.app, at, egui::Modifiers::NONE);
        assert_eq!(selected(&f.app), [f.a], "selected");
        let la = label_of(&f.app, f.a);
        assert_eq!(line(&f.app), format!(".x = {la}"), "and written");
        assert!(f.app.picking(), "the pick goes on");
    }

    /// The label that opens a line is its subject: a bare `.x` means that layer
    /// whatever is selected.
    ///
    /// flip: `line_subject` answering the selection always → fails at "a moved".
    #[test]
    fn the_label_that_opens_a_line_is_its_subject() {
        let ctx = egui::Context::default();
        let mut f = fixture(&ctx);
        bar_with(&ctx, &mut f, "");
        let la = label_of(&f.app, f.a);
        let (a0, b0) = (left_of(&f.app, f.a), left_of(&f.app, f.b));
        f.app
            .command_bar
            .as_mut()
            .unwrap()
            // The second statement's bare `.x` is the subject's.
            .write_at_caret(format!("{la}.x += 0; .x += 10"));
        frame(&ctx, &mut f.app, Vec::new());
        assert_eq!(selected(&f.app), [f.b], "fixture: b is selected");
        tap(&ctx, &mut f.app, egui::Key::Enter);
        assert_eq!(left_of(&f.app, f.a), a0 + 10.0, "a moved");
        assert_eq!(left_of(&f.app, f.b), b0, "b did not");
    }

    fn left_of(app: &MastikApp, id: NodeId) -> f64 {
        app.session.resolved.world_bounds(id).expect("a box").x0
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

    /// A square like `square`'s, named.
    fn named(id: NodeId, parent: NodeId, index: usize, x: f64, y: f64, name: &str) -> Operation {
        let mut op = square(id, parent, index, x, y);
        if let Operation::CreateNode { name: n, .. } = &mut op {
            *n = Some(name.to_owned());
        }
        op
    }

    /// The text a key types, sent with its key as a keyboard sends both.
    fn typed_keys(ctx: &egui::Context, app: &mut MastikApp, text: &str) {
        for c in text.chars() {
            let key = egui::Key::from_name(&c.to_ascii_uppercase().to_string())
                .or_else(|| egui::Key::from_name(&c.to_string()))
                .expect("a key for the character");
            frame(
                ctx,
                app,
                vec![
                    key_with(key, egui::Modifiers::NONE),
                    egui::Event::Text(c.to_string()),
                ],
            );
        }
    }

    /// Six squares in a row under the fixture's, in view; the first selected.
    fn row(ctx: &egui::Context, f: &mut F) -> Vec<NodeId> {
        let page = f.app.session.page;
        let ids: Vec<NodeId> = (0..6).map(|_| f.app.session.ids.mint()).collect();
        let ops = ids
            .iter()
            .enumerate()
            .map(|(i, id)| square(*id, page, 3, i as f64 * 30.0, 30.0))
            .collect();
        f.app.session.commit(Transaction(ops));
        f.app.zoom_to(Some(Rect::new(-20.0, -20.0, 340.0, 70.0)));
        f.app.session.selection.set_one(ids[0]);
        frame(ctx, &mut f.app, Vec::new());
        ids
    }

    /// A count runs the arrow that many times and stops at the last layer that
    /// way; a key the count does not go with spends it, so the next arrow
    /// moves once; `Escape` clears a count without leaving Pick.
    ///
    /// flip: `nav_action` ignoring the count → fails at "3 → moves three".
    #[test]
    fn a_count_runs_the_arrow_that_many_times() {
        let ctx = egui::Context::default();
        let mut f = fixture(&ctx);
        let ids = row(&ctx, &mut f);
        tap(&ctx, &mut f.app, egui::Key::Backtick);
        tap(&ctx, &mut f.app, egui::Key::Num3);
        assert_eq!(f.app.pick_typed(), "3", "the field shows the count");
        tap(&ctx, &mut f.app, egui::Key::ArrowRight);
        assert_eq!(selected(&f.app), [ids[3]], "3 → moves three");
        tap(&ctx, &mut f.app, egui::Key::Num1);
        tap(&ctx, &mut f.app, egui::Key::Num2);
        tap(&ctx, &mut f.app, egui::Key::ArrowLeft);
        assert_eq!(selected(&f.app), [ids[0]], "12 ← stops at the last");
        tap(&ctx, &mut f.app, egui::Key::Num2);
        tap(&ctx, &mut f.app, egui::Key::R);
        tap(&ctx, &mut f.app, egui::Key::ArrowRight);
        assert_eq!(selected(&f.app), [ids[1]], "a resting key spent the count");
        tap(&ctx, &mut f.app, egui::Key::Num2);
        tap(&ctx, &mut f.app, egui::Key::Escape);
        assert!(f.app.picking(), "Escape cleared the count and stayed");
        tap(&ctx, &mut f.app, egui::Key::ArrowRight);
        assert_eq!(selected(&f.app), [ids[2]], "and the next arrow moves once");
        tap(&ctx, &mut f.app, egui::Key::Num0);
        assert_eq!(f.app.pick_typed(), "", "0 starts no count");
    }

    /// `m` + a letter remembers the selection and the view; `'` + the letter
    /// goes back to them from anywhere, and `''` returns to where that jump
    /// left from. The letter is the mark's even when it is a label's letter.
    ///
    /// flip: `pick_key` reading a home-row letter as a label before a pending
    /// mark → fails at "the mark took the letter".
    #[test]
    fn a_mark_remembers_the_selection_and_the_view() {
        let ctx = egui::Context::default();
        let mut f = fixture(&ctx);
        f.app.session.selection.set_one(f.a);
        tap(&ctx, &mut f.app, egui::Key::Backtick);
        let marked_view = f.app.session.camera;
        tap(&ctx, &mut f.app, egui::Key::M);
        assert_eq!(f.app.pick_typed(), "M", "the field shows the prefix");
        tap(&ctx, &mut f.app, egui::Key::F);
        assert!(
            f.app.session.marks.by_letter.contains_key(&'f'),
            "the mark took the letter"
        );
        assert_eq!(f.app.pick_typed(), "", "and no label began");

        let lb = label_of(&f.app, f.b);
        spell(&ctx, &mut f.app, &lb, egui::Modifiers::NONE);
        f.app.session.camera.center.x += 500.0;
        let left_view = f.app.session.camera;
        tap(&ctx, &mut f.app, egui::Key::Quote);
        tap(&ctx, &mut f.app, egui::Key::F);
        assert_eq!(selected(&f.app), [f.a], "back to the mark's selection");
        assert_eq!(f.app.session.camera, marked_view, "and its view");

        tap(&ctx, &mut f.app, egui::Key::Quote);
        tap(&ctx, &mut f.app, egui::Key::Quote);
        assert_eq!(selected(&f.app), [f.b], "'' goes back");
        assert_eq!(f.app.session.camera, left_view);

        tap(&ctx, &mut f.app, egui::Key::Quote);
        tap(&ctx, &mut f.app, egui::Key::Z);
        assert_eq!(selected(&f.app), [f.b], "an unset mark goes nowhere");
        assert!(f.app.picking());
    }

    /// `/` and a name leave only the layers in view whose name holds it, at
    /// any depth inside the scope, with labels for them; `Enter` on one match
    /// selects it, on several hands the letters back to pick among them. The
    /// letters typed into the filter never reach the labels.
    ///
    /// flip: `refilter` matching only the dealt layers → fails at "a layer
    /// inside a group".
    #[test]
    fn the_name_filter_leaves_what_matches() {
        let ctx = egui::Context::default();
        let mut f = fixture(&ctx);
        let page = f.app.session.page;
        let (ghost, solid) = (f.app.session.ids.mint(), f.app.session.ids.mint());
        let inner = f.app.session.ids.mint();
        f.app.session.commit(Transaction(vec![
            named(ghost, page, 3, 0.0, 30.0, "Button ghost"),
            named(solid, page, 4, 60.0, 30.0, "Button solid"),
            named(inner, f.g, 0, 120.0, 0.0, "Ghost inner"),
        ]));
        f.app.zoom_to(Some(Rect::new(-20.0, -20.0, 340.0, 70.0)));
        frame(&ctx, &mut f.app, Vec::new());
        tap(&ctx, &mut f.app, egui::Key::Backtick);
        let before = selected(&f.app);

        typed_keys(&ctx, &mut f.app, "/gh");
        assert_eq!(selected(&f.app), before, "the letters typed no label");
        assert_eq!(f.app.pick_typed(), "/gh");
        let matches = |app: &MastikApp| app.navigate.filter.as_ref().unwrap().matches.clone();
        let mut got = matches(&f.app);
        got.sort();
        let mut want = vec![ghost, inner];
        want.sort();
        assert_eq!(got, want, "both ghosts, one of them a layer inside a group");

        tap(&ctx, &mut f.app, egui::Key::Enter);
        assert!(
            f.app.navigate.filter.as_ref().is_some_and(|f| !f.typing),
            "two matches: Enter hands the letters to the labels"
        );
        let li = label_of(&f.app, inner);
        spell(&ctx, &mut f.app, &li, egui::Modifiers::NONE);
        assert_eq!(selected(&f.app), [inner], "a label among the matches picks");
        assert!(f.app.navigate.filter.is_none(), "and ends the filter");

        typed_keys(&ctx, &mut f.app, "/solid");
        tap(&ctx, &mut f.app, egui::Key::Enter);
        assert_eq!(selected(&f.app), [solid], "one match: Enter picks it");

        typed_keys(&ctx, &mut f.app, "/x");
        tap(&ctx, &mut f.app, egui::Key::Escape);
        assert!(f.app.navigate.filter.is_none(), "Escape clears the filter");
        assert!(f.app.picking(), "and stays in Pick");
    }
}
