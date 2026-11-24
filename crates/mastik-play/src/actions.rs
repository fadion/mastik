//! Running a reaction's actions on the copy, and the moves they make: navigating,
//! overlays, scrolling, a change of state, a variable.

use crate::anim::{ScreenAnim, ScrollAnim, Smart};
use crate::player::{MAX_DEPTH, Notice, Overlay, Player, Request, Visit};
use mastik_core::kurbo::{Point, Vec2};
use mastik_core::prototype::{
    self, Action, ModeScope, OverlaySettings, Pin, ScrollAxis, Snap, SnapAlign, Transition,
    TransitionKind, Trigger, VarChange,
};

/// A drag on a snapping frame let go within this many milliseconds is thrown…
const QUICK_MS: f64 = 300.0;
/// …carrying on at its pace for this long before the nearest rest point is
/// picked.
const THROW_MS: f64 = 200.0;
use mastik_core::variable::{ModeId, Value, VarId};
use mastik_core::{
    ItemId, Keyed, NodeId, NodeKind, Operation, Transaction, subtree_nodes, variant,
};

/// Where a layer sits in a list's row, found again after the list is made
/// again with new copies ([`Player::row_path`]).
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct RowPath {
    list: NodeId,
    place: usize,
    steps: Vec<usize>,
}

/// What an action under a *while* trigger did, so ending the hover or the press
/// can put it back.
#[derive(Clone, Debug, PartialEq)]
pub(crate) enum Undo {
    /// A variable's play value before, `None` for none.
    Value(VarId, Option<Value>),
    /// An instance and the main it was linked to before.
    State(NodeId, NodeId),
    /// How many overlays were open before.
    Overlays(usize),
    /// How long the history was before a navigation.
    Screen(usize),
    /// A page's or frame's modes before a *Set mode*.
    Modes(NodeId, Vec<ModeId>),
}

impl Player {
    /// Run the reaction `reaction` on `holder`, if it is still there. With `undo`,
    /// what its actions do is recorded for a *while* trigger's end.
    pub(crate) fn run_reaction(
        &mut self,
        holder: NodeId,
        reaction: ItemId,
        undo: Option<&mut Vec<Undo>>,
    ) {
        let Some(r) = self
            .doc
            .get(holder)
            .and_then(|n| n.reactions().iter().find(|r| r.id == reaction))
            .map(|r| r.value.clone())
        else {
            return;
        };
        let mut scratch = Vec::new();
        let undo = undo.unwrap_or(&mut scratch);
        if self.depth >= MAX_DEPTH {
            if !self.notices.contains(&Notice::Loop) {
                self.notices.push(Notice::Loop);
            }
            return;
        }
        self.depth += 1;
        self.run_actions(holder, &r.trigger, &r.actions, undo);
        self.depth -= 1;
    }

    /// Run `actions` on `holder` in turn. A write to a table a list draws makes
    /// the list again with new copies, so a holder in a row is looked up at
    /// its place in that row before each action.
    fn run_actions(
        &mut self,
        holder: NodeId,
        trigger: &Trigger,
        actions: &[Keyed<Action>],
        undo: &mut Vec<Undo>,
    ) {
        let path = self.row_path(holder);
        let mut holder = holder;
        for k in actions {
            holder = self.still(holder, path.as_ref());
            self.run_action(holder, trigger, k, undo);
        }
    }

    /// Where `id` sits in a list's row: the list, the row's place among the
    /// drawn rows, and the child index at each step down from the row to `id`.
    /// `None` outside any row.
    pub(crate) fn row_path(&self, id: NodeId) -> Option<RowPath> {
        let mut steps = Vec::new();
        let mut at = id;
        loop {
            if let Some((list, place)) = mastik_core::list::row_of(&self.doc, at) {
                steps.reverse();
                return Some(RowPath { list, place, steps });
            }
            let parent = self.doc.get(at)?.parent()?;
            let i = self
                .doc
                .get(parent)?
                .children()
                .iter()
                .position(|c| *c == at)?;
            steps.push(i);
            at = parent;
        }
    }

    /// `id` while it is there, else the layer at `path` in the list as it is
    /// drawn now; `id` itself when neither is there.
    pub(crate) fn still(&self, id: NodeId, path: Option<&RowPath>) -> NodeId {
        if self.doc.get(id).is_some() {
            return id;
        }
        let found = path.and_then(|p| {
            let root = match p.place {
                0 => mastik_core::list::template(&self.doc, p.list)?,
                n => *self.doc.get(p.list)?.list()?.copies.get(n - 1)?,
            };
            p.steps
                .iter()
                .try_fold(root, |at, i| self.doc.get(at)?.children().get(*i).copied())
        });
        found.unwrap_or(id)
    }

    /// The layer `raw` names when read from a reaction on `holder`, or `None`
    /// with a notice when it is not there (or is not a frame, when one is wanted).
    fn target(
        &mut self,
        holder: NodeId,
        action: ItemId,
        raw: NodeId,
        frame: bool,
    ) -> Option<NodeId> {
        let id = prototype::resolve_target(&self.doc, holder, raw);
        let there = self.doc.get(id).is_some() && (!frame || self.is_frame(id));
        if !there {
            self.notice_once(
                holder,
                action,
                Notice::Missing {
                    layer: holder,
                    action,
                },
            );
            return None;
        }
        Some(id)
    }

    fn run_action(
        &mut self,
        holder: NodeId,
        trigger: &Trigger,
        k: &Keyed<Action>,
        undo: &mut Vec<Undo>,
    ) {
        match &k.value {
            Action::Navigate {
                to,
                transition,
                keep_scroll,
            } => {
                let Some(to) = self.target(holder, k.id, *to, true) else {
                    return;
                };
                undo.push(Undo::Screen(self.history.len()));
                self.navigate(to, *transition, *keep_scroll);
            }
            Action::Back => {
                if self.overlays.is_empty() {
                    self.back();
                } else {
                    self.close_top(None);
                }
            }
            Action::OpenOverlay {
                overlay,
                settings,
                transition,
            } => {
                let Some(frame) = self.target(holder, k.id, *overlay, true) else {
                    return;
                };
                undo.push(Undo::Overlays(self.overlays.len()));
                let anchor = self.drawn_corner(holder);
                self.open_overlay(frame, *settings, anchor, *transition);
            }
            Action::CloseOverlay { transition } => {
                if !self.overlays.is_empty() {
                    self.close_top(Some(*transition));
                }
            }
            Action::SwapOverlay {
                overlay,
                settings,
                transition,
            } => {
                let Some(frame) = self.target(holder, k.id, *overlay, true) else {
                    return;
                };
                undo.push(Undo::Overlays(self.overlays.len()));
                // Read before the top goes: the holder may be in it.
                let anchor = self.drawn_corner(holder);
                if let Some(top) = self.overlays.pop() {
                    self.on_close(top.frame);
                }
                self.open_overlay(frame, *settings, anchor, *transition);
            }
            Action::ScrollTo { layer, transition } => {
                let Some(layer) = self.target(holder, k.id, *layer, false) else {
                    return;
                };
                self.scroll_to(layer, *transition);
            }
            Action::ChangeState {
                instance,
                to,
                transition,
            } => {
                let Some(instance) = self.target(holder, k.id, *instance, false) else {
                    return;
                };
                let Some(to) = self.target(holder, k.id, *to, false) else {
                    return;
                };
                self.auto.remove(&instance);
                if let Some(was) = self.doc.get(instance).and_then(|n| n.link()) {
                    undo.push(Undo::State(instance, was));
                }
                self.change_state(instance, to, *transition);
            }
            Action::SetVariable { var, change } => {
                let Some(old) = self.doc.resolve_var(holder, *var) else {
                    self.notice_once(
                        holder,
                        k.id,
                        Notice::Missing {
                            layer: holder,
                            action: k.id,
                        },
                    );
                    return;
                };
                let new = match (change, old) {
                    (VarChange::Set { to }, old) => {
                        match self
                            .eval(holder, to, None)
                            .and_then(|v| same_kind(v, &old, to))
                        {
                            Ok(v) => v,
                            Err(error) => {
                                self.notice_once(
                                    holder,
                                    k.id,
                                    Notice::Expression {
                                        layer: holder,
                                        action: k.id,
                                        error,
                                    },
                                );
                                return;
                            }
                        }
                    }
                    (VarChange::Increment { by }, Value::Number(n)) => Value::Number(n + by),
                    (VarChange::Toggle, Value::Boolean(b)) => Value::Boolean(!b),
                    // An increment of a non-number, a toggle of a non-boolean: the
                    // door does not refuse one, and play skips it.
                    _ => return,
                };
                undo.push(Undo::Value(*var, self.values.get(var).cloned()));
                self.set_play(*var, new);
            }
            Action::Conditional {
                branches,
                otherwise,
            } => {
                for b in branches {
                    match self.eval(holder, &b.when, None) {
                        Ok(Value::Boolean(true)) => {
                            return self.run_actions(holder, trigger, &b.actions, undo);
                        }
                        Ok(Value::Boolean(false)) => {}
                        Ok(_) => self.notice_once(
                            holder,
                            k.id,
                            Notice::Expression {
                                layer: holder,
                                action: k.id,
                                error: prototype::ExprError::Type {
                                    at: 0..b.when.0.len(),
                                    msg: "a condition comes to true or false".into(),
                                },
                            },
                        ),
                        Err(error) => self.notice_once(
                            holder,
                            k.id,
                            Notice::Expression {
                                layer: holder,
                                action: k.id,
                                error,
                            },
                        ),
                    }
                }
                self.run_actions(holder, trigger, otherwise, undo);
            }
            Action::SetMode { mode, scope } => {
                let t = self.doc.variables();
                let known = t.collection_of_mode(*mode).is_some() || t.kit_mode_of(*mode).is_some();
                if !known {
                    self.notice_once(
                        holder,
                        k.id,
                        Notice::Missing {
                            layer: holder,
                            action: k.id,
                        },
                    );
                    return;
                }
                let at = match scope {
                    ModeScope::Everywhere => self.everywhere(*mode),
                    ModeScope::Screen => vec![self.screen],
                };
                for id in at {
                    if let Some(was) = self.set_mode_on(id, *mode) {
                        undo.push(Undo::Modes(id, was));
                    }
                }
            }
            Action::OpenUrl { url } => {
                let lower = url.trim().to_ascii_lowercase();
                if lower.starts_with("http://") || lower.starts_with("https://") {
                    self.requests.push(Request::OpenUrl(url.trim().to_owned()));
                } else {
                    self.notice_once(
                        holder,
                        k.id,
                        Notice::Missing {
                            layer: holder,
                            action: k.id,
                        },
                    );
                }
            }
            Action::RunScript { script } => {
                // The door refuses this already; a second defence for a copy an
                // edit reached past it.
                if !trigger.is_a_persons_act() {
                    return;
                }
                self.run_script(holder, k.id, *script);
            }
            Action::AddRow { table, at, cells } => self.add_row(holder, k.id, *table, *at, cells),
            Action::EditRow { table, row, cells } => {
                self.edit_row(holder, k.id, *table, row, cells)
            }
            Action::DeleteRow { table, row } => self.delete_row(holder, k.id, *table, row),
        }
    }

    /// Put back what a *while* trigger's actions did, newest first.
    pub(crate) fn undo(&mut self, undo: Vec<Undo>) {
        for u in undo.into_iter().rev() {
            match u {
                Undo::Value(var, was) => self.restore_value(var, was),
                Undo::State(instance, was) => {
                    self.switch_to(instance, was);
                }
                Undo::Overlays(n) => {
                    while self.overlays.len() > n {
                        self.close_top(None);
                    }
                }
                Undo::Screen(n) => {
                    if self.history.len() > n {
                        self.back();
                    }
                }
                Undo::Modes(id, was) => self.put_modes(id, was),
            }
        }
    }

    /// Where *Set mode* sets `mode` everywhere: every page, for one of the
    /// document's modes. A page holds none of a kit's, so a kit's goes on each
    /// top-level frame instead, passing over one that pins a mode of that kit
    /// collection in the document, as a frame's own pin beats the page's.
    pub(crate) fn everywhere(&self, mode: ModeId) -> Vec<NodeId> {
        let t = self.doc.variables();
        let pages = self.doc.page_ids();
        if t.mode(mode).is_some() {
            return pages.collect();
        }
        let Some(owner) = t.owner_of(mode) else {
            return Vec::new();
        };
        let bt = self.base.variables();
        let pins = |id: NodeId| {
            self.base.get(id).is_some_and(|n| {
                n.modes()
                    .iter()
                    .any(|m| bt.owner_of(*m).is_some_and(|o| o == owner))
            })
        };
        pages
            .filter_map(|p| self.doc.get(p))
            .flat_map(|root| root.children().iter().copied())
            .filter(|f| {
                self.doc
                    .get(*f)
                    .is_some_and(|n| matches!(n.kind(), NodeKind::Artboard { .. }))
                    && !pins(*f)
            })
            .collect()
    }

    /// Set `mode` on node `id` of the copy (a page's root or a frame) in place
    /// of any mode of its collection there. Answers the modes it held before,
    /// `None` when nothing changed.
    pub(crate) fn set_mode_on(&mut self, id: NodeId, mode: ModeId) -> Option<Vec<ModeId>> {
        let was = self.doc.get(id)?.modes().to_vec();
        let t = self.doc.variables();
        let mut modes: Vec<ModeId> = was
            .iter()
            .copied()
            .filter(|m| !mastik_core::variable::same_collection(t, *m, mode))
            .collect();
        modes.push(mode);
        modes.sort();
        if modes == was {
            return None;
        }
        self.put_modes(id, modes);
        Some(was)
    }

    /// Give node `id` of the copy exactly `modes`, remembered so a live edit's
    /// fresh copy gets them again.
    pub(crate) fn put_modes(&mut self, id: NodeId, modes: Vec<ModeId>) {
        let tx = Transaction(vec![Operation::SetModes {
            id,
            modes: modes.clone(),
        }]);
        if self.commit(tx).is_ok() {
            self.modes.insert(id, modes);
            // A filter's variable may read another value in the new mode.
            self.refilter(None);
        }
    }

    // ── Moves ────────────────────────────────────────────────────────────────

    /// Show `to` as the screen, closing every overlay, with `to` the frame *Back*
    /// leaves from. With `keep`, `to`'s scrolling frames open where they were
    /// last left.
    pub(crate) fn navigate(&mut self, to: NodeId, transition: Transition, keep: bool) {
        if to == self.screen {
            return;
        }
        self.history.push(Visit {
            frame: self.screen,
            transition,
            keep,
        });
        self.show(to, transition, keep);
    }

    /// Pop the history: the frame before, by the reverse of the transition that
    /// left it, at its offsets when that navigation kept them.
    pub(crate) fn back(&mut self) {
        if let Some(visit) = self.history.pop() {
            self.show(visit.frame, reversed(visit.transition), visit.keep);
        }
    }

    fn show(&mut self, to: NodeId, transition: Transition, keep: bool) {
        while let Some(o) = self.overlays.pop() {
            self.on_close(o.frame);
        }
        self.anims.closing.clear();
        self.anims.overlays.clear();
        let from = self.screen;
        // A scrolling frame inside the frame shown starts at its top, unless
        // kept; the offsets of every screen left stay for the run either way.
        if !keep {
            let inside = subtree_nodes(&self.doc, &[to]);
            self.scroll.retain(|k, _| !inside.contains(k));
            // Not a scroll back past a point: the frame opens at its top.
            self.scroll_seen.retain(|k, _| !inside.contains(k));
        }
        // Laid out at the play size before smart animate measures it.
        self.fit_screen(to);
        self.anims.screen = (transition.kind != TransitionKind::Instant).then(|| ScreenAnim {
            from,
            to,
            transition,
            start: self.now,
            smart: (transition.kind == TransitionKind::SmartAnimate)
                .then(|| Smart::between_frames(self, from, to, transition.direction)),
        });
        if let Some(page) = self.doc.page_of(to) {
            self.page = page;
        }
        self.screen = to;
        self.set_focus(None);
        self.on_close(from);
        self.on_open(to);
        self.rehover();
    }

    /// Open `frame` over the screen as `settings` say; `anchor` is the screen
    /// point a manual position is measured from.
    pub(crate) fn open_overlay(
        &mut self,
        frame: NodeId,
        settings: OverlaySettings,
        anchor: Point,
        transition: Transition,
    ) {
        self.overlays.push(Overlay {
            frame,
            settings,
            anchor,
            transition,
        });
        if transition.kind != TransitionKind::Instant {
            self.anims.open(frame, transition, self.now);
        }
        self.on_open(frame);
        self.rehover();
    }

    /// Close the top overlay, by `transition` or else the reverse of the one it
    /// opened with.
    pub(crate) fn close_top(&mut self, transition: Option<Transition>) {
        let Some(o) = self.overlays.pop() else { return };
        let transition = transition.unwrap_or_else(|| reversed(o.transition));
        self.anims.overlays.retain(|a| a.frame != o.frame);
        if transition.kind != TransitionKind::Instant {
            let at = self.overlay_at(&o);
            self.anims.close(o.clone(), at, transition, self.now);
        }
        self.on_close(o.frame);
        if self.focus.is_some_and(|f| self.frame_of(f).is_none()) {
            self.set_focus(None);
        }
        self.rehover();
    }

    /// Scroll the nearest scrolling frame holding `layer` until `layer`'s top-left
    /// is at its top-left, as far as the content allows.
    fn scroll_to(&mut self, layer: NodeId, transition: Transition) {
        let mut at = self.doc.get(layer).and_then(|n| n.parent());
        while let Some(f) = at {
            if self.scrolls(f) != ScrollAxis::None {
                break;
            }
            at = self.doc.get(f).and_then(|n| n.parent());
        }
        let Some(frame) = at else { return };
        let (Some(l), Some(f)) = (self.rect(layer), self.rect(frame)) else {
            return;
        };
        let to = self.clamp_scroll(frame, l.origin() - f.origin());
        let from = self.scroll_offset(frame);
        self.anims.scrolls.retain(|a| a.frame != frame);
        if transition.kind == TransitionKind::Instant {
            self.scroll.insert(frame, to);
        } else {
            self.anims.scrolls.push(ScrollAnim {
                frame,
                from,
                to,
                start: self.now,
                timing: transition.timing,
            });
        }
    }

    /// The axis `frame` scrolls along: its own when its content overflows.
    pub(crate) fn scrolls(&self, frame: NodeId) -> ScrollAxis {
        if !self.is_frame(frame) {
            return ScrollAxis::None;
        }
        self.doc
            .get(frame)
            .map_or(ScrollAxis::None, |n| n.play().scroll)
    }

    /// `offset` held within how far `frame`'s content reaches past its box, on
    /// the axes it scrolls.
    pub(crate) fn clamp_scroll(&self, frame: NodeId, offset: Vec2) -> Vec2 {
        let axis = self.scrolls(frame);
        let (Some(f), Some(n)) = (self.rect(frame), self.doc.get(frame)) else {
            return Vec2::ZERO;
        };
        let mut reach = f;
        for c in n.children() {
            let fixed = self
                .doc
                .get(*c)
                .is_some_and(|c| c.play().pin == prototype::Pin::Fixed);
            if let (false, Some(r)) = (fixed, self.rect(*c)) {
                reach = reach.union(r);
            }
        }
        let max = Vec2::new(reach.x1 - f.x1, reach.y1 - f.y1);
        let x = matches!(axis, ScrollAxis::Horizontal | ScrollAxis::Both);
        let y = matches!(axis, ScrollAxis::Vertical | ScrollAxis::Both);
        Vec2::new(
            if x {
                offset.x.clamp(0.0, max.x.max(0.0))
            } else {
                0.0
            },
            if y {
                offset.y.clamp(0.0, max.y.max(0.0))
            } else {
                0.0
            },
        )
    }

    /// The offsets `frame` comes to rest at along one axis (`x`: across),
    /// ascending; empty when it does not snap on that axis.
    pub(crate) fn snap_points(&self, frame: NodeId, x: bool) -> Vec<f64> {
        let (Some(n), Some(f)) = (self.doc.get(frame), self.rect(frame)) else {
            return Vec::new();
        };
        let axis = self.scrolls(frame);
        let on = match x {
            true => matches!(axis, ScrollAxis::Horizontal | ScrollAxis::Both),
            false => matches!(axis, ScrollAxis::Vertical | ScrollAxis::Both),
        };
        if !on {
            return Vec::new();
        }
        let far = self.clamp_scroll(frame, Vec2::new(f64::MAX, f64::MAX));
        let (max, size, origin) = match x {
            true => (far.x, f.width(), f.x0),
            false => (far.y, f.height(), f.y0),
        };
        let play = n.play();
        let mut out: Vec<f64> = match play.snap {
            Snap::None => return Vec::new(),
            Snap::Pages => {
                let mut v = Vec::new();
                let mut k = 0.0;
                while size > 0.0 && k < max && v.len() < 10_000 {
                    v.push(k);
                    k += size;
                }
                v.push(max);
                v
            }
            Snap::Children => {
                let spans: Vec<(f64, f64)> = n
                    .children()
                    .iter()
                    .filter(|c| {
                        self.doc
                            .get(**c)
                            .is_some_and(|c| c.visible() && c.play().pin != Pin::Fixed)
                    })
                    .filter_map(|c| self.rect(*c))
                    .map(|r| match x {
                        true => (r.x0 - origin, r.x1 - origin),
                        false => (r.y0 - origin, r.y1 - origin),
                    })
                    .collect();
                // The first child's lead is the frame's inset, on both ends.
                let inset = spans.iter().map(|s| s.0).fold(f64::INFINITY, f64::min);
                let inset = if inset.is_finite() {
                    inset.max(0.0)
                } else {
                    0.0
                };
                spans
                    .iter()
                    .map(|(a, b)| match play.snap_align {
                        SnapAlign::Start => a - inset,
                        SnapAlign::Centre => (a + b) / 2.0 - size / 2.0,
                        SnapAlign::End => b - (size - inset),
                    })
                    .collect()
            }
        };
        for v in &mut out {
            *v = v.clamp(0.0, max.max(0.0));
        }
        out.sort_by(f64::total_cmp);
        out.dedup_by(|a, b| (*a - *b).abs() < 0.5);
        out
    }

    /// Whether `frame` snaps on either axis.
    pub(crate) fn snaps(&self, frame: NodeId) -> bool {
        self.doc
            .get(frame)
            .is_some_and(|n| n.play().snap != Snap::None)
            && self.scrolls(frame) != ScrollAxis::None
    }

    /// One wheel step on a snapping frame: to the next rest point toward
    /// `delta`. A step under way swallows the rest of its gesture, so a
    /// trackpad's stream of small deltas moves one step. Answers whether it
    /// moved.
    pub(crate) fn snap_step(&mut self, frame: NodeId, delta: Vec2) -> bool {
        if self.anims.scrolls.iter().any(|a| a.frame == frame) {
            return true;
        }
        let at = self.scroll_offset(frame);
        let step = |pts: &[f64], from: f64, d: f64| -> f64 {
            match d.partial_cmp(&0.0) {
                Some(std::cmp::Ordering::Greater) => pts
                    .iter()
                    .copied()
                    .find(|p| *p > from + 0.5)
                    .unwrap_or(from),
                Some(std::cmp::Ordering::Less) => pts
                    .iter()
                    .rev()
                    .copied()
                    .find(|p| *p < from - 0.5)
                    .unwrap_or(from),
                _ => from,
            }
        };
        let (px, py) = (
            self.snap_points(frame, true),
            self.snap_points(frame, false),
        );
        let to = Vec2::new(
            if px.is_empty() {
                at.x
            } else {
                step(&px, at.x, delta.x)
            },
            if py.is_empty() {
                at.y
            } else {
                step(&py, at.y, delta.y)
            },
        );
        self.settle_scroll(frame, to)
    }

    /// Bring a snapping frame to rest after a drag that started at offset
    /// `start` and took `ms`: at least one step the way it was dragged, and
    /// further by how fast it was let go.
    pub(crate) fn snap_release(&mut self, frame: NodeId, start: Vec2, ms: f64) {
        let at = self.scroll_offset(frame);
        let pick = |pts: &[f64], s: f64, now: f64| -> f64 {
            if pts.is_empty() {
                return now;
            }
            let moved = now - s;
            // A quick release carries on for a while at its pace.
            let ahead = match ms <= QUICK_MS {
                true => now + moved / ms.max(1.0) * THROW_MS,
                false => now,
            };
            let near = pts
                .iter()
                .copied()
                .min_by(|a, b| (a - ahead).abs().total_cmp(&(b - ahead).abs()))
                .unwrap_or(now);
            let next = match moved.partial_cmp(&0.0) {
                Some(std::cmp::Ordering::Greater) => pts.iter().copied().find(|p| *p > s + 0.5),
                Some(std::cmp::Ordering::Less) => pts.iter().rev().copied().find(|p| *p < s - 0.5),
                _ => None,
            };
            match next {
                Some(n) if (near - s) * moved.signum() < (n - s) * moved.signum() => n,
                _ => near,
            }
        };
        let to = Vec2::new(
            pick(&self.snap_points(frame, true), start.x, at.x),
            pick(&self.snap_points(frame, false), start.y, at.y),
        );
        self.settle_scroll(frame, to);
    }

    /// Move `frame`'s offset to `to` by the page's new-link timing. Answers
    /// whether it will move.
    fn settle_scroll(&mut self, frame: NodeId, to: Vec2) -> bool {
        let from = self.scroll_offset(frame);
        if to == from {
            return false;
        }
        let timing = self
            .doc
            .page(self.page)
            .map_or(Transition::default().timing, |p| p.play.transition.timing);
        self.anims.scrolls.retain(|a| a.frame != frame);
        self.anims.scrolls.push(ScrollAnim {
            frame,
            from,
            to,
            start: self.now,
            timing,
        });
        true
    }

    /// Scroll `frame` by `delta`, as far as its content allows. Answers whether
    /// it moved.
    pub(crate) fn scroll_by(&mut self, frame: NodeId, delta: Vec2) -> bool {
        let was = self.scroll_offset(frame);
        let now = self.clamp_scroll(frame, was + delta);
        self.anims.scrolls.retain(|a| a.frame != frame);
        self.scroll.insert(frame, now);
        now != was
    }

    /// Switch `instance` to `to`, animated by `transition`.
    fn change_state(&mut self, instance: NodeId, to: NodeId, transition: Transition) {
        if self.doc.get(instance).and_then(|n| n.link()) == Some(to) {
            return;
        }
        let animate = transition.kind != TransitionKind::Instant;
        let before = animate.then(|| Smart::capture_state(self, instance));
        if !self.switch_to(instance, to) {
            return;
        }
        if let Some(before) = before {
            let smart = Smart::after_state(self, instance, before);
            self.anims.states.retain(|a| a.instance != instance);
            self.anims.states.push(crate::anim::StateAnim {
                instance,
                smart,
                start: self.now,
                timing: transition.timing,
            });
        }
    }

    /// Switch `instance` to the variant main `to` on the copy. Answers whether it
    /// switched.
    pub(crate) fn switch_to(&mut self, instance: NodeId, to: NodeId) -> bool {
        if self.doc.get(instance).and_then(|n| n.link()) == Some(to) {
            return true;
        }
        let Some(tx) = variant::switch(&self.doc, instance, to, &mut self.ids) else {
            return false;
        };
        let ok = self.commit(tx).is_ok();
        if ok {
            self.states.insert(instance, to);
            self.rearm(instance);
        }
        ok
    }

    /// Each input instance showing `var` switched to the variant for its value.
    pub(crate) fn sync_inputs(&mut self, var: VarId) {
        let bound: Vec<(NodeId, String)> = self
            .inputs
            .iter()
            .filter(|(_, i)| i.var == var)
            .filter_map(|(id, i)| Some((*id, i.property.clone()?)))
            .collect();
        for (id, property) in bound {
            let Some(Value::Boolean(on)) = self.doc.resolve_var(id, var) else {
                continue;
            };
            if let Some(main) = self.checked_main(id, &property, on) {
                self.switch_to(id, main);
            }
        }
    }

    /// The variant of `instance`'s set it shows with boolean property `property`
    /// at `on`, its other properties kept.
    fn checked_main(&self, instance: NodeId, property: &str, on: bool) -> Option<NodeId> {
        let main = self.doc.get(instance)?.link()?;
        let set = variant::set_of(&self.doc, main)?;
        let vs = self.doc.get(set)?.set()?;
        let prop = vs.props.iter().position(|p| p.name == property)?;
        let value = bool_value(&vs.props[prop].values, on)?;
        variant::switch_target(&self.doc, main, prop, &value)
    }
}

/// `v` when it is of `old`'s kind: a table's cell has its kind only in play, so
/// *Set variable* checks it here, as the field checks a variable's.
fn same_kind(
    v: Value,
    old: &Value,
    to: &mastik_core::prototype::Expr,
) -> Result<Value, prototype::ExprError> {
    use crate::player::kind_of;
    let word = |k| match k {
        mastik_core::variable::VarKind::Number => "a number",
        mastik_core::variable::VarKind::String => "text",
        mastik_core::variable::VarKind::Boolean => "true or false",
        mastik_core::variable::VarKind::Color => "a colour",
    };
    match kind_of(&v) == kind_of(old) {
        true => Ok(v),
        false => Err(prototype::ExprError::Type {
            at: 0..to.0.len(),
            msg: format!(
                "this comes to {}, not {}",
                word(kind_of(&v)),
                word(kind_of(old))
            ),
        }),
    }
}

/// The value of a boolean variant property that means `on`: a value named like
/// true or false, else the second value for true and the first for false.
fn bool_value(values: &[String], on: bool) -> Option<String> {
    let words: &[&str] = match on {
        true => &["true", "on", "yes", "checked"],
        false => &["false", "off", "no", "unchecked"],
    };
    values
        .iter()
        .find(|v| words.contains(&v.to_lowercase().as_str()))
        .or_else(|| match on {
            true => values.get(1),
            false => values.first(),
        })
        .cloned()
}

/// `t` played backwards: a move in becomes a move out from the other side, a
/// push or slide comes from the other side.
pub(crate) fn reversed(t: Transition) -> Transition {
    let kind = match t.kind {
        TransitionKind::MoveIn => TransitionKind::MoveOut,
        TransitionKind::MoveOut => TransitionKind::MoveIn,
        k => k,
    };
    let direction = if t.kind.is_directional() {
        crate::anim::opposite(t.direction)
    } else {
        t.direction
    };
    Transition {
        kind,
        direction,
        timing: t.timing,
    }
}
