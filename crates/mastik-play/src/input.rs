//! What the person does: pointer, keys and text in screen space, hit-tested
//! against the shown frames, and the triggers they fire.

use crate::actions::Undo;
use crate::player::Player;
use crate::scene::Hotspot;
use mastik_core::kurbo::{Point, Vec2};
use mastik_core::prototype::{Pin, ScrollAxis, Trigger};
use mastik_core::variable::{Value, VarId};
use mastik_core::variant::{self, Interaction};
use mastik_core::{NodeId, NodeKind, subtree_nodes};

/// How far, in screen units, a press moves before it is a drag.
const DRAG_SLOP: f64 = 4.0;

/// One thing the person did, in screen space ([`crate::Scene`]).
#[derive(Clone, Debug, PartialEq)]
pub enum Input {
    PointerMove(Point),
    PointerDown(Point),
    PointerUp(Point),
    /// The pointer left the player.
    PointerLeave,
    /// A wheel or trackpad scroll by `delta`, positive into the content.
    Wheel {
        at: Point,
        delta: Vec2,
    },
    /// A key by name (`"A"`, `"Enter"`, `"ArrowLeft"`, `"Backspace"`), plain or
    /// Shift-ed. Chords with Ctrl or Alt are the editor's and never come here.
    Key {
        key: String,
        shift: bool,
    },
    /// Text typed, for the focused input.
    Text(String),
    /// A value set from the state card.
    SetValue {
        var: VarId,
        value: Value,
    },
}

/// A press in progress.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct Press {
    pub(crate) last: Point,
    pub(crate) start: Point,
    /// What was under the press, deepest first.
    pub(crate) chain: Vec<NodeId>,
    pub(crate) dragging: bool,
    /// The scrolling frame a drag scrolls, once it is one.
    pub(crate) scroller: Option<NodeId>,
}

/// A *while* reaction in force, and what to put back when it ends.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct While {
    pub(crate) node: NodeId,
    pub(crate) press: bool,
    pub(crate) undo: Vec<Undo>,
}

/// What is under a point: the overlay it is in (or outside of), and the layers
/// under it, deepest first and its frame last; empty outside the top overlay.
struct Hit {
    overlay: Option<usize>,
    chain: Vec<NodeId>,
}

impl Player {
    /// Take one input. Answers whether the prototype used it, so a shell can
    /// hand an unused Escape back to the editor. A take being recorded records it.
    pub fn input(&mut self, input: Input) -> bool {
        if let Some(r) = &mut self.recording {
            r.inputs.push((self.now, input.clone()));
        }
        match input {
            Input::PointerMove(p) => self.pointer_move(p),
            Input::PointerDown(p) => self.pointer_down(p),
            Input::PointerUp(p) => self.pointer_up(p),
            Input::PointerLeave => {
                self.pointer = None;
                self.rehover();
                true
            }
            Input::Wheel { at, delta } => self.wheel(at, delta),
            Input::Key { key, shift } => self.key(&key, shift),
            Input::Text(text) => self.text(&text),
            Input::SetValue { var, value } => {
                self.set_play(var, value);
                true
            }
        }
    }

    /// The places the person can act on in the frame that takes input (the top
    /// overlay, else the screen), for the hints.
    pub fn hotspots(&self) -> Vec<Hotspot> {
        let (frame, at) = match self.overlays.last() {
            Some(o) => (o.frame, self.overlay_at(o)),
            None => (self.screen, Point::ZERO),
        };
        let origin = self.origin(frame);
        let mut out = Vec::new();
        for id in subtree_nodes(&self.doc, &[frame]) {
            let Some(n) = self.doc.get(id) else { continue };
            let acts = n.reactions().iter().any(|r| {
                matches!(
                    r.value.trigger,
                    Trigger::Click
                        | Trigger::Press
                        | Trigger::PressWhile
                        | Trigger::Drag
                        | Trigger::HoverEnter
                        | Trigger::HoverWhile
                )
            });
            if !(acts || n.play().input.is_some()) || !self.doc.shown_through(id) {
                continue;
            }
            if let Some(r) = self.rect(id) {
                let shift = self.drawn_shift(id);
                out.push(Hotspot {
                    layer: id,
                    rect: r + (shift + (at - origin)),
                });
            }
        }
        out
    }

    // ── Hit testing ──────────────────────────────────────────────────────────

    /// What is under screen point `p`. Overlays are modal: a point outside the
    /// top one hits nothing beneath it.
    fn hit(&self, p: Point) -> Hit {
        if let Some((i, o)) = self.overlays.iter().enumerate().next_back() {
            let at = self.overlay_at(o);
            let size = self.size(o.frame);
            let inside = mastik_core::kurbo::Rect::from_origin_size(at, size).contains(p);
            let chain = match inside {
                true => self.chain(o.frame, p - at.to_vec2()),
                false => Vec::new(),
            };
            return Hit {
                overlay: Some(i),
                chain,
            };
        }
        Hit {
            overlay: None,
            chain: self.chain(self.screen, p),
        }
    }

    /// The layers of `frame` under `q` (in the frame's own screen space), deepest
    /// first, the frame last.
    fn chain(&self, frame: NodeId, q: Point) -> Vec<NodeId> {
        let w = self.origin(frame) + q.to_vec2();
        let mut out = Vec::new();
        self.walk(frame, w, Vec2::ZERO, &mut out);
        out
    }

    fn walk(&self, id: NodeId, w: Point, shift: Vec2, out: &mut Vec<NodeId>) -> bool {
        let Some(n) = self.doc.get(id) else {
            return false;
        };
        if !n.visible() {
            return false;
        }
        let rect = self.rect(id).map(|r| r + shift);
        let inside = rect.is_some_and(|r| r.contains(w));
        let clips = n.clip() || self.scrolls(id) != ScrollAxis::None;
        if clips && matches!(n.kind(), NodeKind::Artboard { .. }) && !inside {
            return false;
        }
        for c in n.children().iter().rev() {
            if self.walk(*c, w, shift + self.child_offset(id, *c), out) {
                out.push(id);
                return true;
            }
        }
        let hittable = !matches!(n.kind(), NodeKind::Group | NodeKind::Root);
        if inside && hittable {
            out.push(id);
            return true;
        }
        false
    }

    /// How far `child` of `parent` is drawn from where it is placed: a scrolling
    /// frame's offset, by the child's pin.
    pub(crate) fn child_offset(&self, parent: NodeId, child: NodeId) -> Vec2 {
        let Some(s) = self.scroll.get(&parent).copied() else {
            return Vec2::ZERO;
        };
        let pin = self
            .doc
            .get(child)
            .map(|c| c.play().pin)
            .unwrap_or_default();
        match pin {
            Pin::Scrolls => -s,
            Pin::Fixed => Vec2::ZERO,
            // Scrolls with the content until it meets the frame's edge, then stays.
            Pin::Sticky => {
                let at = self
                    .res
                    .used_local(&self.doc, child)
                    .map_or(Vec2::ZERO, |t| t.translation());
                Vec2::new(-s.x.min(at.x.max(0.0)), -s.y.min(at.y.max(0.0)))
            }
        }
    }

    /// How far `id` is drawn from where it is placed: every scrolling frame
    /// above it, by each one's pin.
    pub(crate) fn drawn_shift(&self, id: NodeId) -> Vec2 {
        let mut shift = Vec2::ZERO;
        let mut child = id;
        while let Some(parent) = self.doc.get(child).and_then(|n| n.parent()) {
            shift += self.child_offset(parent, child);
            child = parent;
        }
        shift
    }

    // ── Pointer ──────────────────────────────────────────────────────────────

    fn pointer_move(&mut self, p: Point) -> bool {
        self.pointer = Some(p);
        let mut used = false;
        if let Some(mut press) = self.press.take() {
            if !press.dragging && (p - press.start).hypot() > DRAG_SLOP {
                press.dragging = true;
                // A drag reaction takes the drag; else the nearest scrolling frame.
                if !self.fire(&press.chain, &|t| *t == Trigger::Drag) {
                    press.scroller = press
                        .chain
                        .iter()
                        .copied()
                        .find(|f| self.scrolls(*f) != ScrollAxis::None);
                }
                used = true;
            }
            if let Some(f) = press.scroller {
                used |= self.scroll_by(f, press.last - p);
            }
            press.last = p;
            self.press = Some(press);
        }
        self.rehover();
        used
    }

    fn pointer_down(&mut self, p: Point) -> bool {
        self.pointer = Some(p);
        let hit = self.hit(p);
        if let (Some(i), true) = (hit.overlay, hit.chain.is_empty()) {
            // Outside the top overlay.
            if self.overlays[i].settings.close_outside {
                self.close_top(None);
                return true;
            }
            return false;
        }
        let chain = hit.chain;
        // An input's text may be empty and so have no width: a click on the box
        // holding it focuses it too.
        let held = chain.first().and_then(|deepest| {
            let n = self.doc.get(*deepest)?;
            n.children()
                .iter()
                .copied()
                .find(|c| self.is_text_input(*c))
        });
        let focus = chain
            .iter()
            .copied()
            .find(|id| self.is_text_input(*id))
            .or(held);
        self.set_focus(focus);
        self.press = Some(Press {
            last: p,
            start: p,
            chain: chain.clone(),
            dragging: false,
            scroller: None,
        });
        let mut used = self.fire(&chain, &|t| *t == Trigger::Press);
        used |= self.fire_while(&chain, true);
        if let Some(id) = chain
            .iter()
            .copied()
            .find(|id| self.has_state(*id, Interaction::Active))
        {
            used |= self.show_state(id, Interaction::Active);
        }
        used || self.focus.is_some()
    }

    fn pointer_up(&mut self, p: Point) -> bool {
        self.pointer = Some(p);
        let Some(press) = self.press.take() else {
            return false;
        };
        let ended: Vec<_> = self.whiles.iter().filter(|w| w.press).cloned().collect();
        self.whiles.retain(|w| !w.press);
        for w in ended {
            self.undo(w.undo);
        }
        for id in press.chain.clone() {
            let hover = self.focus_shown != Some(id) && self.hovered.contains(&id);
            if self.auto.contains_key(&id) && !(hover && self.show_state(id, Interaction::Hover)) {
                self.rest(id);
            }
        }
        if press.dragging {
            return true;
        }
        let mut used = false;
        // An input instance flips its variable.
        let checkbox = press.chain.iter().find_map(|id| {
            let input = self.doc.get(*id)?.play().input.clone()?;
            input.property.is_some().then_some(input.var)
        });
        if let Some(var) = checkbox
            && let Some(Value::Boolean(b)) = self.doc.resolve_var(self.screen, var)
        {
            self.set_play(var, Value::Boolean(!b));
            used = true;
        }
        used |= self.fire(&press.chain, &|t| *t == Trigger::Click);
        if !used && self.focus.is_none() {
            self.flash = Some(self.now);
        }
        used
    }

    fn wheel(&mut self, at: Point, delta: Vec2) -> bool {
        let chain = self.hit(at).chain;
        for f in chain {
            let axis = self.scrolls(f);
            if axis != ScrollAxis::None && self.scroll_by(f, delta) {
                return true;
            }
        }
        false
    }

    /// Bring the hover state up to date with the pointer: layers it left end
    /// their hover, layers it entered start theirs.
    pub(crate) fn rehover(&mut self) {
        let now = match self.pointer {
            Some(p) => self.hit(p).chain,
            None => Vec::new(),
        };
        let left: Vec<NodeId> = self
            .hovered
            .iter()
            .copied()
            .filter(|id| !now.contains(id))
            .collect();
        let entered: Vec<NodeId> = now
            .iter()
            .copied()
            .filter(|id| !self.hovered.contains(id))
            .collect();
        self.hovered = now;
        for id in left {
            let ended: Vec<_> = self
                .whiles
                .iter()
                .filter(|w| !w.press && w.node == id)
                .cloned()
                .collect();
            self.whiles.retain(|w| w.press || w.node != id);
            for w in ended {
                self.undo(w.undo);
            }
            if self.auto.contains_key(&id) {
                self.rest(id);
            }
        }
        for id in entered {
            if self.disabled(id) || !self.hovered.contains(&id) {
                continue;
            }
            let reactions: Vec<_> = self
                .doc
                .get(id)
                .map(|n| n.reactions().to_vec())
                .unwrap_or_default();
            let authored = reactions
                .iter()
                .any(|r| matches!(r.value.trigger, Trigger::HoverEnter | Trigger::HoverWhile));
            for r in reactions {
                match r.value.trigger {
                    Trigger::HoverEnter => self.run_reaction(id, r.id, None),
                    Trigger::HoverWhile => {
                        let mut undo = Vec::new();
                        self.run_reaction(id, r.id, Some(&mut undo));
                        self.whiles.push(While {
                            node: id,
                            press: false,
                            undo,
                        });
                    }
                    _ => {}
                }
            }
            let pressed = self.press.as_ref().is_some_and(|p| p.chain.contains(&id));
            let focused = self.focus_shown == Some(id);
            if !authored && !pressed && !focused && self.has_state(id, Interaction::Hover) {
                self.show_state(id, Interaction::Hover);
            }
        }
    }

    // ── Keys and text ────────────────────────────────────────────────────────

    fn key(&mut self, key: &str, shift: bool) -> bool {
        if key == "Tab" {
            return self.tab(shift);
        }
        if let Some(field) = self.focus.filter(|f| self.is_text_input(*f)) {
            match key {
                "Escape" | "Enter" => self.set_focus(None),
                "Backspace" => self.edit_text(field, |s| {
                    s.pop();
                }),
                _ => {}
            }
            return true;
        }
        if let Some(input) = self.focus {
            match key {
                "Escape" => {
                    self.set_focus(None);
                    return true;
                }
                " " | "Space" | "Enter" if !shift => return self.flip(input),
                _ => {}
            }
        }
        let frame = self.overlays.last().map_or(self.screen, |o| o.frame);
        let mut fire = Vec::new();
        for id in subtree_nodes(&self.doc, &[frame]) {
            let Some(n) = self.doc.get(id) else { continue };
            for r in n.reactions() {
                if let Trigger::Key { key: k, shift: s } = &r.value.trigger
                    && k.eq_ignore_ascii_case(key)
                    && *s == shift
                {
                    fire.push((id, r.id));
                }
            }
        }
        let used = !fire.is_empty();
        for (id, reaction) in fire {
            self.run_reaction(id, reaction, None);
        }
        used
    }

    fn text(&mut self, text: &str) -> bool {
        let Some(field) = self.focus.filter(|f| self.is_text_input(*f)) else {
            return false;
        };
        let typed: String = text.chars().filter(|c| !c.is_control()).collect();
        self.edit_text(field, |s| s.push_str(&typed));
        true
    }

    fn edit_text(&mut self, field: NodeId, f: impl FnOnce(&mut String)) {
        let Some(input) = self.doc.get(field).and_then(|n| n.play().input.clone()) else {
            return;
        };
        let mut s = match self.doc.resolve_var(field, input.var) {
            Some(Value::String(s)) => s,
            _ => return,
        };
        f(&mut s);
        self.set_play(input.var, Value::String(s));
    }

    fn is_text_input(&self, id: NodeId) -> bool {
        self.doc.get(id).is_some_and(|n| {
            matches!(n.kind(), NodeKind::Text { .. })
                && n.play()
                    .input
                    .as_ref()
                    .is_some_and(|i| i.property.is_none())
        })
    }

    // ── Triggers ─────────────────────────────────────────────────────────────

    /// Run the reactions `want` picks on the deepest layer of `chain` that has
    /// one; a layer inside a disabled instance is passed over. Answers whether
    /// any ran.
    pub(crate) fn fire(&mut self, chain: &[NodeId], want: &dyn Fn(&Trigger) -> bool) -> bool {
        for &id in chain {
            if self.disabled(id) {
                continue;
            }
            let picked: Vec<_> = self
                .doc
                .get(id)
                .map(|n| {
                    n.reactions()
                        .iter()
                        .filter(|r| want(&r.value.trigger))
                        .map(|r| r.id)
                        .collect()
                })
                .unwrap_or_default();
            if picked.is_empty() {
                continue;
            }
            for r in picked {
                self.run_reaction(id, r, None);
            }
            return true;
        }
        false
    }

    /// [`Self::fire`] for the *while pressed* reactions, each recording what to
    /// put back on release.
    fn fire_while(&mut self, chain: &[NodeId], press: bool) -> bool {
        for &id in chain {
            if self.disabled(id) {
                continue;
            }
            let picked: Vec<_> = self
                .doc
                .get(id)
                .map(|n| {
                    n.reactions()
                        .iter()
                        .filter(|r| r.value.trigger == Trigger::PressWhile)
                        .map(|r| r.id)
                        .collect()
                })
                .unwrap_or_default();
            if picked.is_empty() {
                continue;
            }
            for r in picked {
                let mut undo = Vec::new();
                self.run_reaction(id, r, Some(&mut undo));
                self.whiles.push(While {
                    node: id,
                    press,
                    undo,
                });
            }
            return true;
        }
        false
    }

    // ── A component set's interaction states ─────────────────────────────────

    /// The main `instance` shows in `state`, its other properties kept: the
    /// variant of its set whose interaction-state value is tied to `state`.
    fn state_main(&self, instance: NodeId, state: Interaction) -> Option<NodeId> {
        if !variant::can_switch(&self.doc, instance) {
            return None;
        }
        let main = match self.auto.get(&instance) {
            Some(rest) => *rest,
            None => self.doc.get(instance)?.link()?,
        };
        let set = variant::set_of(&self.doc, main)?;
        let vs = self.doc.get(set)?.set()?;
        let prop = vs.interaction()?;
        let p = &vs.props[prop];
        let value = p.values.iter().find(|v| p.state_of(v) == Some(state))?;
        variant::switch_target(&self.doc, main, prop, value)
    }

    /// The interaction state `instance`'s variant is tied to, if any.
    fn current_state(&self, instance: NodeId) -> Option<Interaction> {
        let main = self.doc.get(instance)?.link()?;
        let set = variant::set_of(&self.doc, main)?;
        let vs = self.doc.get(set)?.set()?;
        let prop = vs.interaction()?;
        let value = self.doc.get(main)?.variant().get(prop)?;
        vs.props[prop].state_of(value)
    }

    fn has_state(&self, instance: NodeId, state: Interaction) -> bool {
        !self.disabled(instance) && self.state_main(instance, state).is_some()
    }

    /// Whether `id` sits in an instance shown in its *Disabled* state, which takes
    /// no input.
    pub(crate) fn disabled(&self, id: NodeId) -> bool {
        let mut at = Some(id);
        while let Some(x) = at {
            if self.current_state(x) == Some(Interaction::Disabled) {
                return true;
            }
            at = self.doc.get(x).and_then(|n| n.parent());
        }
        false
    }

    /// Show `instance` in `state` by itself, remembering the main to return to.
    fn show_state(&mut self, instance: NodeId, state: Interaction) -> bool {
        let Some(to) = self.state_main(instance, state) else {
            return false;
        };
        let Some(rest) = self.doc.get(instance).and_then(|n| n.link()) else {
            return false;
        };
        let rest = *self.auto.get(&instance).unwrap_or(&rest);
        if self.switch_to(instance, to) {
            self.auto.insert(instance, rest);
            return true;
        }
        false
    }

    /// Return `instance` from an interaction state to the main it rests on, or
    /// to its *Focus* state while it shows the focused input's.
    fn rest(&mut self, instance: NodeId) {
        if self.focus_shown == Some(instance)
            && self.show_state(instance, Interaction::FocusVisible)
        {
            return;
        }
        if let Some(rest) = self.auto.remove(&instance) {
            self.switch_to(instance, rest);
        }
    }

    // ── Focus ────────────────────────────────────────────────────────────────

    /// Give focus to `to` (none: take it away). The instance holding the
    /// focused input, or the input itself, shows its *Focus* state; the one
    /// that showed it before rests.
    pub(crate) fn set_focus(&mut self, to: Option<NodeId>) {
        self.focus = to;
        let holder = to.and_then(|f| self.focus_holder(f));
        if holder == self.focus_shown {
            return;
        }
        if let Some(old) = self.focus_shown.take() {
            self.rest(old);
        }
        if let Some(h) = holder
            && self.show_state(h, Interaction::FocusVisible)
        {
            self.focus_shown = Some(h);
        }
    }

    /// The nearest instance from `input` up whose set has a *Focus* state.
    fn focus_holder(&self, input: NodeId) -> Option<NodeId> {
        let mut at = Some(input);
        while let Some(x) = at {
            if self.has_state(x, Interaction::FocusVisible) {
                return Some(x);
            }
            at = self.doc.get(x).and_then(|n| n.parent());
        }
        None
    }

    /// Move focus to the next input of the frame that takes input in reading
    /// order (`back`: the one before), round to the first. Answers whether there
    /// was one to move to.
    fn tab(&mut self, back: bool) -> bool {
        let order = self.tab_order();
        if order.is_empty() {
            return false;
        }
        let at = self.focus.and_then(|f| order.iter().position(|x| *x == f));
        let n = order.len();
        let next = match (at, back) {
            (None, false) => 0,
            (None, true) => n - 1,
            (Some(i), false) => (i + 1) % n,
            (Some(i), true) => (i + n - 1) % n,
        };
        self.set_focus(Some(order[next]));
        true
    }

    /// The inputs of the frame that takes input (the top overlay, else the
    /// screen) that can take focus, in reading order: topmost, then leftmost,
    /// ties to the earlier layer. Hidden ones and those in a disabled instance
    /// are left out.
    fn tab_order(&self) -> Vec<NodeId> {
        let frame = self.overlays.last().map_or(self.screen, |o| o.frame);
        let mut out: Vec<(f64, f64, usize, NodeId)> = subtree_nodes(&self.doc, &[frame])
            .into_iter()
            .enumerate()
            .filter(|(_, id)| {
                self.inputs.iter().any(|(x, _)| x == id)
                    && self.doc.shown_through(*id)
                    && !self.disabled(*id)
            })
            .filter_map(|(i, id)| {
                let r = self.rect(id)? + self.drawn_shift(id);
                Some((r.y0, r.x0, i, id))
            })
            .collect();
        out.sort_by(|a, b| {
            a.0.total_cmp(&b.0)
                .then(a.1.total_cmp(&b.1))
                .then(a.2.cmp(&b.2))
        });
        out.into_iter().map(|(.., id)| id).collect()
    }

    /// Flip the boolean input instance `input` holds focus on, as a click does.
    fn flip(&mut self, input: NodeId) -> bool {
        let Some(var) = self
            .doc
            .get(input)
            .and_then(|n| n.play().input.clone())
            .filter(|i| i.property.is_some())
            .map(|i| i.var)
        else {
            return false;
        };
        let Some(Value::Boolean(b)) = self.doc.resolve_var(input, var) else {
            return false;
        };
        self.set_play(var, Value::Boolean(!b));
        true
    }
}
