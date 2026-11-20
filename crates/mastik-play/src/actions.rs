//! Running a reaction's actions on the copy, and the moves they make: navigating,
//! overlays, scrolling, a change of state, a variable.

use crate::anim::{ScreenAnim, ScrollAnim, Smart};
use crate::player::{MAX_DEPTH, Notice, Overlay, Player, Request, Visit};
use mastik_core::kurbo::Vec2;
use mastik_core::prototype::{
    self, Action, Direction, ScrollAxis, Transition, TransitionKind, Trigger, VarChange,
};
use mastik_core::variable::{Value, VarId};
use mastik_core::{ItemId, Keyed, NodeId, subtree_nodes, variant};

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

    fn run_actions(
        &mut self,
        holder: NodeId,
        trigger: &Trigger,
        actions: &[Keyed<Action>],
        undo: &mut Vec<Undo>,
    ) {
        for k in actions {
            self.run_action(holder, trigger, k, undo);
        }
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
            Action::Navigate { to, transition } => {
                let Some(to) = self.target(holder, k.id, *to, true) else {
                    return;
                };
                undo.push(Undo::Screen(self.history.len()));
                self.navigate(to, *transition);
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
                transition,
            } => {
                let Some(frame) = self.target(holder, k.id, *overlay, true) else {
                    return;
                };
                undo.push(Undo::Overlays(self.overlays.len()));
                self.open_overlay(frame, *transition);
            }
            Action::CloseOverlay { transition } => {
                if !self.overlays.is_empty() {
                    self.close_top(Some(*transition));
                }
            }
            Action::SwapOverlay {
                overlay,
                transition,
            } => {
                let Some(frame) = self.target(holder, k.id, *overlay, true) else {
                    return;
                };
                undo.push(Undo::Overlays(self.overlays.len()));
                if let Some(top) = self.overlays.pop() {
                    self.on_close(top.frame);
                }
                self.open_overlay(frame, *transition);
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
                    (VarChange::Set { to }, _) => {
                        match to.eval(&|v| self.doc.resolve_var(holder, v)) {
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
                    match b.when.eval(&|v| self.doc.resolve_var(holder, v)) {
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
            }
        }
    }

    // ── Moves ────────────────────────────────────────────────────────────────

    /// Show `to` as the screen, closing every overlay, with `to` the frame *Back*
    /// leaves from.
    pub(crate) fn navigate(&mut self, to: NodeId, transition: Transition) {
        if to == self.screen {
            return;
        }
        self.history.push(Visit {
            frame: self.screen,
            transition,
        });
        self.show(to, transition);
    }

    /// Pop the history: the frame before, by the reverse of the transition that
    /// left it.
    pub(crate) fn back(&mut self) {
        if let Some(visit) = self.history.pop() {
            self.show(visit.frame, reversed(visit.transition));
        }
    }

    fn show(&mut self, to: NodeId, transition: Transition) {
        while let Some(o) = self.overlays.pop() {
            self.on_close(o.frame);
        }
        self.anims.closing.clear();
        self.anims.overlays.clear();
        let from = self.screen;
        // A scrolling frame inside the frame shown starts at its top.
        let inside = subtree_nodes(&self.doc, &[to]);
        self.scroll.retain(|k, _| !inside.contains(k));
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
        self.focus = None;
        self.on_close(from);
        self.on_open(to);
        self.rehover();
    }

    pub(crate) fn open_overlay(&mut self, frame: NodeId, transition: Transition) {
        let settings = self
            .doc
            .get(frame)
            .map(|n| n.play().overlay)
            .unwrap_or_default();
        self.overlays.push(Overlay {
            frame,
            settings,
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
            self.focus = None;
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
    let flip = |d: Direction| match d {
        Direction::Left => Direction::Right,
        Direction::Right => Direction::Left,
        Direction::Up => Direction::Down,
        Direction::Down => Direction::Up,
    };
    let kind = match t.kind {
        TransitionKind::MoveIn => TransitionKind::MoveOut,
        TransitionKind::MoveOut => TransitionKind::MoveIn,
        k => k,
    };
    let direction = if t.kind.is_directional() {
        flip(t.direction)
    } else {
        t.direction
    };
    Transition {
        kind,
        direction,
        timing: t.timing,
    }
}
