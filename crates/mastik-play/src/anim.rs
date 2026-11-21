//! Motion between states — a screen's transition, an overlay's, a change of
//! state, a scroll — and the scene each frame draws.
//!
//! Smart animate resolves layout at both ends when it starts and interpolates
//! between them: nothing is laid out per frame.

use crate::motion::progress;
use crate::player::{Overlay, Player};
use crate::scene::{Layer, Patch, Scene};
use mastik_core::kurbo::{Affine, Point, RoundedRectRadii, Size, Vec2};
use mastik_core::peniko::Color;
use mastik_core::prototype::{
    Direction, ScrollAxis, ScrollProp, Timing, Transition, TransitionKind,
};
use mastik_core::{
    Brush, BrushSource, Effect, EffectKind, Keyed, NodeId, Paint, Shadow, subtree_nodes,
};
use rustc_hash::{FxHashMap, FxHashSet};

/// How long the hotspot hints flash after a click on nothing, in milliseconds.
pub(crate) const FLASH_MS: f64 = 600.0;
/// How far the screen beneath a slide moves, as a share of the screen.
const SLIDE_UNDER: f64 = 0.3;

/// Every motion under way.
#[derive(Default)]
pub(crate) struct Anims {
    pub(crate) screen: Option<ScreenAnim>,
    /// Overlays opening, by frame.
    pub(crate) overlays: Vec<OverlayAnim>,
    /// Overlays closed and still leaving, drawn above the open ones.
    pub(crate) closing: Vec<Closing>,
    pub(crate) states: Vec<StateAnim>,
    pub(crate) scrolls: Vec<ScrollAnim>,
    /// Layers that follow the drag, settling after a release.
    pub(crate) moves: Vec<crate::drag::MoveAnim>,
}

impl Anims {
    pub(crate) fn is_empty(&self) -> bool {
        self.screen.is_none()
            && self.overlays.is_empty()
            && self.closing.is_empty()
            && self.states.is_empty()
            && self.scrolls.is_empty()
            && self.moves.is_empty()
    }

    pub(crate) fn open(&mut self, frame: NodeId, transition: Transition, now: f64) {
        self.overlays.retain(|a| a.frame != frame);
        self.overlays.push(OverlayAnim {
            frame,
            transition,
            start: now,
        });
    }

    pub(crate) fn close(&mut self, overlay: Overlay, at: Point, transition: Transition, now: f64) {
        self.closing.push(Closing {
            overlay,
            at,
            transition,
            start: now,
        });
    }
}

/// The screen changing from one frame to another.
pub(crate) struct ScreenAnim {
    pub(crate) from: NodeId,
    pub(crate) to: NodeId,
    pub(crate) transition: Transition,
    pub(crate) start: f64,
    pub(crate) smart: Option<Smart>,
}

pub(crate) struct OverlayAnim {
    pub(crate) frame: NodeId,
    pub(crate) transition: Transition,
    pub(crate) start: f64,
}

pub(crate) struct Closing {
    pub(crate) overlay: Overlay,
    pub(crate) at: Point,
    pub(crate) transition: Transition,
    pub(crate) start: f64,
}

/// An instance changing state.
pub(crate) struct StateAnim {
    pub(crate) instance: NodeId,
    pub(crate) smart: Smart,
    pub(crate) start: f64,
    pub(crate) timing: Timing,
}

pub(crate) struct ScrollAnim {
    pub(crate) frame: NodeId,
    pub(crate) from: Vec2,
    pub(crate) to: Vec2,
    pub(crate) start: f64,
    pub(crate) timing: Timing,
}

/// One layer's look at one end of a smart animate.
#[derive(Clone, Debug)]
pub(crate) struct Look {
    /// Its transform relative to the animation's reference layer.
    rel: Affine,
    size: Option<Size>,
    opacity: f32,
    /// Its paint with literal brushes.
    paint: Paint,
    radii: Option<RoundedRectRadii>,
    /// Its effects with literal numbers.
    effects: Vec<Keyed<Effect>>,
}

/// One paired layer: where it goes from and to.
#[derive(Clone, Debug)]
struct Track {
    node: NodeId,
    /// The layer it is placed in, when that is tracked too.
    parent: Option<NodeId>,
    /// The parent's transform relative to the reference at the end, for a
    /// parent that is not tracked.
    parent_rel: Affine,
    /// The reference layer itself: its paint and opacity move, never its place.
    root: bool,
    from: Look,
    to: Look,
}

/// A smart animate: the paired layers' tracks, and what fades.
#[derive(Clone, Debug, Default)]
pub(crate) struct Smart {
    tracks: Vec<Track>,
    /// Layers of the end with no pair: they fade in, or slide in under `slide`.
    fade_in: Vec<NodeId>,
    /// Layers of the outgoing frame with no pair: they fade out, or slide out.
    fade_out: Vec<NodeId>,
    /// Paired layers of the outgoing frame, which the end's layers draw.
    hide: Vec<NodeId>,
    /// Paired layers of the outgoing frame that carry a fading layer: they
    /// move with their pair, their own paint blank. By pair.
    carry: Vec<(NodeId, NodeId)>,
    /// The outgoing frame, drawn with its own paint blank.
    blank: Option<NodeId>,
    /// Between frames, the screen's span toward the transition's side: the
    /// unpaired layers slide along it, the end's arriving from the far side and
    /// the outgoing frame's leaving toward it, instead of fading. Each layer's
    /// parent's look at its end, by layer, turns the shift into its own space.
    slide: Option<(Vec2, FxHashMap<NodeId, Affine>)>,
}

/// A layer's pairing key below a smart animate's frame: each step its name, or
/// for a layer of an instance the lineage key of the main layer it follows, and
/// which of its siblings with that key it is.
type Path = Vec<(String, usize)>;

impl Smart {
    /// Pair the layers of frames `a` and `b` by name and path, and capture both
    /// ends. The unpaired layers slide toward `direction`'s side.
    pub(crate) fn between_frames(p: &Player, a: NodeId, b: NodeId, direction: Direction) -> Smart {
        let paths_a = p.paths(a);
        let by_path: FxHashMap<&Path, NodeId> = paths_a.iter().map(|(id, k)| (k, *id)).collect();
        let mut pair: FxHashMap<NodeId, NodeId> = FxHashMap::default();
        let mut smart = Smart {
            blank: Some(a),
            ..Smart::default()
        };
        let mut faded: FxHashSet<NodeId> = FxHashSet::default();
        // Pre-order, so a parent's track is before its children's.
        for (id, path) in p.paths(b) {
            if id == b {
                smart.tracks.push(Track {
                    node: b,
                    parent: None,
                    parent_rel: Affine::IDENTITY,
                    root: true,
                    from: p.look(a, a),
                    to: p.look(b, b),
                });
                pair.insert(a, b);
                continue;
            }
            let parent = p.doc.get(id).and_then(|n| n.parent());
            match by_path.get(&path) {
                Some(&from) => {
                    pair.insert(from, id);
                    // The end's scroll (a kept one) is added over the tracks by
                    // `scroll_patches`, so the start is measured without it.
                    let mut start = p.look_scrolled(a, from);
                    start.rel = Affine::translate(-p.drawn_shift(id)) * start.rel;
                    smart.tracks.push(Track {
                        node: id,
                        parent: parent.filter(|q| *q != b && smart.tracked(*q)),
                        parent_rel: parent.map_or(Affine::IDENTITY, |q| p.look(b, q).rel),
                        root: false,
                        from: start,
                        to: p.look(b, id),
                    });
                }
                None => {
                    if !parent.is_some_and(|q| faded.contains(&q)) {
                        smart.fade_in.push(id);
                    }
                    faded.insert(id);
                }
            }
        }
        // The outgoing frame: unpaired layers fade, under a paired parent that
        // carries them or the frame itself; every other paired layer is hidden.
        let mut gone: FxHashSet<NodeId> = FxHashSet::default();
        for (id, _) in &paths_a {
            if *id == a {
                continue;
            }
            let parent = p.doc.get(*id).and_then(|n| n.parent());
            if pair.contains_key(id) {
                continue;
            }
            if !parent.is_some_and(|q| gone.contains(&q)) {
                smart.fade_out.push(*id);
            }
            gone.insert(*id);
        }
        for (id, _) in &paths_a {
            let Some(to) = pair.get(id).copied().filter(|_| *id != a) else {
                continue;
            };
            let carries = subtree_nodes(&p.doc, &[*id])
                .iter()
                .any(|x| smart.fade_out.contains(x));
            match carries {
                true => smart.carry.push((*id, to)),
                false => smart.hide.push(*id),
            }
        }
        // With no side, the unpaired layers fade.
        if direction == Direction::None {
            return smart;
        }
        let size = p.size(b);
        let d = toward(direction);
        let span = Vec2::new(d.x * size.width, d.y * size.height);
        let mut parents = FxHashMap::default();
        for (ids, frame) in [(&smart.fade_in, b), (&smart.fade_out, a)] {
            for id in ids {
                let parent = p.doc.get(*id).and_then(|n| n.parent()).unwrap_or(frame);
                parents.insert(*id, p.look(frame, parent).rel);
            }
        }
        smart.slide = Some((span, parents));
        smart
    }

    /// The looks of `instance`'s layers before it changes state.
    pub(crate) fn capture_state(p: &Player, instance: NodeId) -> FxHashMap<NodeId, Look> {
        let reference = p.reference(instance);
        subtree_nodes(&p.doc, &[instance])
            .into_iter()
            .map(|id| (id, p.look(reference, id)))
            .collect()
    }

    /// Pair `instance`'s layers after a change of state with `before` by id (the
    /// switch keeps them), its new layers fading in.
    pub(crate) fn after_state(
        p: &Player,
        instance: NodeId,
        before: FxHashMap<NodeId, Look>,
    ) -> Smart {
        let reference = p.reference(instance);
        let mut smart = Smart::default();
        let mut faded: FxHashSet<NodeId> = FxHashSet::default();
        for id in subtree_nodes(&p.doc, &[instance]) {
            let parent = p.doc.get(id).and_then(|n| n.parent());
            match before.get(&id) {
                Some(from) => smart.tracks.push(Track {
                    node: id,
                    parent: parent.filter(|q| smart.tracked(*q)),
                    parent_rel: parent.map_or(Affine::IDENTITY, |q| p.look(reference, q).rel),
                    root: false,
                    from: from.clone(),
                    to: p.look(reference, id),
                }),
                None => {
                    if !parent.is_some_and(|q| faded.contains(&q)) {
                        smart.fade_in.push(id);
                    }
                    faded.insert(id);
                }
            }
        }
        smart
    }

    /// An unpaired layer's local transform shifted `k` spans along the slide,
    /// or `None` when this smart animate fades instead.
    fn slid(&self, p: &Player, id: NodeId, k: f64) -> Option<Affine> {
        let (span, parents) = self.slide.as_ref()?;
        let parent = parents.get(&id).copied().unwrap_or(Affine::IDENTITY);
        let local = p.res.used_local(&p.doc, id).unwrap_or(Affine::IDENTITY);
        Some(parent.inverse() * Affine::translate(*span * k) * parent * local)
    }

    fn tracked(&self, id: NodeId) -> bool {
        self.tracks.iter().any(|t| t.node == id && !t.root)
    }

    /// The patches for the end's layers at progress `t`, into `out`.
    fn patch_in(&self, p: &Player, t: f64, out: &mut Patches) {
        let mut rels: FxHashMap<NodeId, Affine> = FxHashMap::default();
        let fade = t.clamp(0.0, 1.0) as f32;
        for tr in &self.tracks {
            let Some(n) = p.doc.get(tr.node) else {
                continue;
            };
            let patch = out.get(tr.node);
            if !tr.root {
                let rel = lerp_affine(tr.from.rel, tr.to.rel, t);
                rels.insert(tr.node, rel);
                let parent = tr
                    .parent
                    .and_then(|q| rels.get(&q).copied())
                    .unwrap_or(tr.parent_rel);
                patch.transform = Some(parent.inverse() * rel);
                if let (Some(a), Some(b)) = (tr.from.size, tr.to.size)
                    && a != b
                {
                    let v = a.to_vec2().lerp(b.to_vec2(), t);
                    patch.size = Some(Size::new(v.x.max(0.0), v.y.max(0.0)));
                }
            }
            if tr.from.opacity != tr.to.opacity {
                let o = tr.from.opacity + (tr.to.opacity - tr.from.opacity) * t as f32;
                patch.opacity = Some(o.clamp(0.0, 1.0));
            }
            let mut paint = tr.to.paint.clone();
            let mut painted = false;
            // Fill by fill: one that cannot move (an image, two kinds of
            // gradient) shows the end's at once, the rest still move.
            if tr.from.paint.fills.len() == paint.fills.len() && tr.from.paint.fills != paint.fills
            {
                for (f, a) in paint.fills.iter_mut().zip(&tr.from.paint.fills) {
                    if let (Some(x), Some(y)) = (a.brush.literal(), f.brush.literal())
                        && let Some(b) = lerp_brush(x, y, t, fade)
                    {
                        f.brush = BrushSource::Literal(b);
                    }
                }
                painted = true;
            }
            if tr.from.paint.strokes.len() == paint.strokes.len()
                && tr.from.paint.strokes != paint.strokes
            {
                for (s, a) in paint.strokes.iter_mut().zip(&tr.from.paint.strokes) {
                    s.width = mix(a.width, s.width, t).max(0.0);
                    if let (Some(x), Some(y)) = (a.brush.literal(), s.brush.literal())
                        && let Some(b) = lerp_brush(x, y, t, fade)
                    {
                        s.brush = BrushSource::Literal(b);
                    }
                }
                painted = true;
            }
            if painted {
                patch.paint = Some(paint);
            }
            if let (Some(a), Some(b)) = (tr.from.radii, tr.to.radii)
                && a != b
            {
                patch.radii = Some(RoundedRectRadii::new(
                    mix(a.top_left, b.top_left, t).max(0.0),
                    mix(a.top_right, b.top_right, t).max(0.0),
                    mix(a.bottom_right, b.bottom_right, t).max(0.0),
                    mix(a.bottom_left, b.bottom_left, t).max(0.0),
                ));
            }
            if tr.from.effects != tr.to.effects
                && let Some(effects) = lerp_effects(&tr.from.effects, &tr.to.effects, t)
            {
                patch.effects = Some(effects);
            }
            let _ = n;
        }
        for id in &self.fade_in {
            if let Some(n) = p.doc.get(*id) {
                match self.slid(p, *id, t - 1.0) {
                    Some(at) => out.get(*id).transform = Some(at),
                    None => out.get(*id).opacity = Some(n.opacity() * fade),
                }
            }
        }
        // The outgoing frame's carriers move as their pairs do.
        for (from, to) in &self.carry {
            let transform = out.map.get(to).and_then(|x| x.transform);
            let patch = out.get(*from);
            patch.transform = transform;
            patch.paint = Some(Paint::default());
        }
        for id in &self.hide {
            out.get(*id).visible = Some(false);
        }
        for id in &self.fade_out {
            if let Some(n) = p.doc.get(*id) {
                match self.slid(p, *id, t) {
                    Some(at) => out.get(*id).transform = Some(at),
                    None => out.get(*id).opacity = Some(n.opacity() * (1.0 - fade)),
                }
            }
        }
        if let Some(a) = self.blank {
            out.get(a).paint = Some(Paint::default());
        }
    }
}

/// The patches of one scene, by node.
#[derive(Default)]
struct Patches {
    map: FxHashMap<NodeId, Patch>,
}

impl Patches {
    fn get(&mut self, node: NodeId) -> &mut Patch {
        self.map.entry(node).or_insert_with(|| Patch::new(node))
    }
}

impl Player {
    /// The layer a change of state on `instance` is measured from: its parent.
    fn reference(&self, instance: NodeId) -> NodeId {
        self.doc
            .get(instance)
            .and_then(|n| n.parent())
            .unwrap_or(instance)
    }

    /// `id`'s look, measured from `reference`.
    pub(crate) fn look(&self, reference: NodeId, id: NodeId) -> Look {
        let world = |x| self.res.world_transform(x).unwrap_or(Affine::IDENTITY);
        let rel = world(reference).inverse() * world(id);
        let (opacity, paint, effects) = match self.doc.get(id) {
            Some(n) => (
                n.opacity(),
                self.doc.resolved_paint(id, n.paint()).into_owned(),
                self.doc.resolved_effects(id, n.effects()).into_owned(),
            ),
            None => (1.0, Paint::default(), Vec::new()),
        };
        let kind = self.res.used_kind(&self.doc, id);
        Look {
            rel,
            size: kind.and_then(|k| k.box_size()),
            opacity,
            paint,
            radii: kind.and_then(|k| k.corner_radii()),
            effects,
        }
    }

    /// [`Self::look`] where `id` is drawn: moved by the scrolling frames above it.
    fn look_scrolled(&self, reference: NodeId, id: NodeId) -> Look {
        let mut look = self.look(reference, id);
        look.rel = Affine::translate(self.drawn_shift(id)) * look.rel;
        look
    }

    /// Every layer of `frame` with its pairing path, pre-order, `frame` first
    /// with the empty path.
    fn paths(&self, frame: NodeId) -> Vec<(NodeId, Path)> {
        let mut out = vec![(frame, Vec::new())];
        let mut i = 0;
        while i < out.len() {
            let (id, path) = out[i].clone();
            i += 1;
            let Some(n) = self.doc.get(id) else { continue };
            let mut seen: FxHashMap<String, usize> = FxHashMap::default();
            let mut kids = Vec::new();
            for c in n.children() {
                let Some(child) = self.doc.get(*c) else {
                    continue;
                };
                let key = match child.link() {
                    Some(main) => format!(
                        "\u{1}{}",
                        self.doc.get(main).map_or(main, |m| m.lineage()).to_wire()
                    ),
                    None => child.name().to_owned(),
                };
                let k = seen.entry(key.clone()).or_insert(0);
                let mut p = path.clone();
                p.push((key, *k));
                *k += 1;
                kids.push((*c, p));
            }
            // Children right after their parent keeps the list pre-order.
            for (offset, kid) in kids.into_iter().enumerate() {
                out.insert(i + offset, kid);
            }
        }
        out
    }

    /// Move every motion on to now: scrolls write their offsets, and what has
    /// finished goes.
    pub(crate) fn advance_anims(&mut self) {
        let now = self.now;
        let done = |start: f64, timing: &Timing| progress(timing, now - start).1;
        let mut scrolled = Vec::new();
        for a in &self.anims.scrolls {
            let (t, finished) = progress(&a.timing, now - a.start);
            let at = if finished { a.to } else { a.from.lerp(a.to, t) };
            scrolled.push((a.frame, at));
        }
        self.scroll.extend(scrolled);
        self.anims.scrolls.retain(|a| !done(a.start, &a.timing));
        let mut settled = Vec::new();
        for a in &self.anims.moves {
            let (t, finished) = progress(&a.timing, now - a.start);
            let at = if finished { a.to } else { a.from.lerp(a.to, t) };
            settled.push((a.layer, at));
            if finished {
                self.drops.push((a.layer, a.at));
            }
        }
        self.moved.extend(settled);
        self.anims.moves.retain(|a| !done(a.start, &a.timing));
        if self
            .anims
            .screen
            .as_ref()
            .is_some_and(|a| done(a.start, &a.transition.timing))
        {
            self.anims.screen = None;
        }
        self.anims
            .overlays
            .retain(|a| !done(a.start, &a.transition.timing));
        self.anims
            .closing
            .retain(|a| !done(a.start, &a.transition.timing));
        self.anims.states.retain(|a| !done(a.start, &a.timing));
    }

    /// What to draw now.
    pub fn scene(&self) -> Scene {
        let mut patches = Patches::default();
        let mut layers = Vec::new();
        let size = self.size(self.screen);
        let mut outgoing = None;
        match &self.anims.screen {
            Some(a) if self.doc.get(a.from).is_some() => {
                let (t, _) = progress(&a.transition.timing, self.now - a.start);
                let (from, to) = screen_layers(a, t, size);
                layers.push(from);
                layers.push(to);
                if a.transition.kind == TransitionKind::MoveOut {
                    layers.swap(0, 1);
                }
                if let Some(smart) = &a.smart {
                    smart.patch_in(self, t, &mut patches);
                    // The outgoing frame draws over the end, its paired layers
                    // hidden and its own background blank.
                    layers.swap(0, 1);
                    outgoing = Some(a.from);
                }
            }
            _ => layers.push(Layer {
                frame: self.screen,
                at: Point::ZERO,
                opacity: 1.0,
                backdrop: None,
                overlay: false,
            }),
        }
        for o in &self.overlays {
            let at = self.overlay_at(o);
            let anim = self.anims.overlays.iter().find(|a| a.frame == o.frame);
            let t = anim.map_or(1.0, |a| {
                progress(&a.transition.timing, self.now - a.start).0
            });
            let kind = anim.map_or(TransitionKind::Instant, |a| a.transition.kind);
            let dir = anim.map_or(Direction::Left, |a| a.transition.direction);
            layers.push(overlay_layer(o, at, kind, dir, t, size));
        }
        for c in &self.anims.closing {
            let (t, _) = progress(&c.transition.timing, self.now - c.start);
            // Closing plays the opening backwards: from shown to gone.
            let kind = match c.transition.kind {
                TransitionKind::MoveOut => TransitionKind::MoveIn,
                k => k,
            };
            let dir = match c.transition.kind {
                TransitionKind::MoveOut => opposite(c.transition.direction),
                _ => c.transition.direction,
            };
            layers.push(overlay_layer(&c.overlay, c.at, kind, dir, 1.0 - t, size));
        }
        for s in &self.anims.states {
            let (t, _) = progress(&s.timing, self.now - s.start);
            s.smart.patch_in(self, t, &mut patches);
        }
        self.scroll_patches(outgoing, &mut patches);
        let mut patches: Vec<Patch> = patches.map.into_values().collect();
        patches.sort_by_key(|p| p.node);
        Scene {
            page: self.page,
            size,
            layers,
            patches,
            hint_flash: self
                .flash
                .map(|f| (1.0 - (self.now - f) / FLASH_MS).clamp(0.0, 1.0) as f32),
        }
    }

    /// Each scrolling frame's children moved by its offset, by their pins, and
    /// each layer that follows the drag by how far it was moved, then each
    /// scroll effect, over whatever else patched them. The outgoing frame of a
    /// smart animate is left alone: its layers move with their pairs.
    fn scroll_patches(&self, skip: Option<NodeId>, out: &mut Patches) {
        let mut frames: Vec<NodeId> = self.scroll.keys().copied().collect();
        frames.sort();
        for frame in frames {
            if skip.is_some_and(|s| self.doc.get(frame).is_some() && self.under(frame, s)) {
                continue;
            }
            let Some(n) = self.doc.get(frame) else {
                continue;
            };
            for c in n.children().to_vec() {
                let shift = self.scroll_shift(frame, c);
                if shift == Vec2::ZERO {
                    continue;
                }
                let placed = self
                    .res
                    .used_local(&self.doc, c)
                    .unwrap_or(Affine::IDENTITY);
                let patch = out.get(c);
                let local = patch.transform.unwrap_or(placed);
                patch.transform = Some(Affine::translate(shift) * local);
            }
        }
        // Layers that follow the drag, over the rest; the outgoing frame's
        // move with their pairs, as its scroll does.
        let outgoing = |id: NodeId| skip.is_some_and(|s| self.under(id, s));
        let mut moved: Vec<(NodeId, Vec2)> = self.moved.iter().map(|(k, v)| (*k, *v)).collect();
        moved.sort_by_key(|(id, _)| *id);
        for (id, off) in moved {
            if off == Vec2::ZERO || self.doc.get(id).is_none() || outgoing(id) {
                continue;
            }
            let placed = self
                .res
                .used_local(&self.doc, id)
                .unwrap_or(Affine::IDENTITY);
            let patch = out.get(id);
            let local = patch.transform.unwrap_or(placed);
            patch.transform = Some(Affine::translate(off) * local);
        }
        for id in self.effects.clone() {
            if !outgoing(id) {
                self.scroll_effect_patch(id, out);
            }
        }
    }

    /// `id`'s scroll effect at the nearest scrolling frame's offset above it:
    /// its opacity in place of its own, then a move, scale and turn about its
    /// centre over whatever placed it.
    fn scroll_effect_patch(&self, id: NodeId, out: &mut Patches) {
        let Some(e) = self.effect_now(id) else { return };
        let placed = self
            .res
            .used_local(&self.doc, id)
            .unwrap_or(Affine::IDENTITY);
        let centre = self
            .res
            .used_kind(&self.doc, id)
            .and_then(|k| k.box_size())
            .map_or(Point::ZERO, |s| Point::new(s.width / 2.0, s.height / 2.0));
        let patch = out.get(id);
        if let Some(o) = e.opacity {
            patch.opacity = Some(o.clamp(0.0, 1.0) as f32);
        }
        if e.transforms {
            let local = patch.transform.unwrap_or(placed);
            patch.transform = Some(e.affine(local * centre) * local);
        }
    }

    /// Where `id`'s scroll effect stands at the offset of the frame it names,
    /// or else the nearest scrolling frame above it; `None` without an effect,
    /// with a named frame gone, or with no scrolling frame above.
    pub(crate) fn effect_now(&self, id: NodeId) -> Option<EffectNow> {
        let n = self.doc.get(id)?;
        let e = n.play().scroll_effect.as_ref()?;
        let frame = match e.frame {
            Some(f) => {
                let f = mastik_core::prototype::resolve_target(&self.doc, id, f);
                self.doc.get(f).map(|_| f)?
            }
            None => {
                let mut at = n.parent();
                while let Some(f) = at.filter(|f| self.scrolls(*f) == ScrollAxis::None) {
                    at = self.doc.get(f).and_then(|n| n.parent());
                }
                at?
            }
        };
        let offset = self.scroll_offset(frame);
        let along = match self.scrolls(frame) {
            ScrollAxis::Horizontal => offset.x,
            _ => offset.y,
        };
        let t = match e.to - e.from {
            span if span.abs() < f64::EPSILON => f64::from(u8::from(along >= e.to)),
            span => ((along - e.from) / span).clamp(0.0, 1.0),
        };
        let value = |p: ScrollProp| {
            e.props
                .iter()
                .find(|x| x.prop == p)
                .map_or(p.rest(), |x| x.start + (x.end - x.start) * t)
        };
        let has = |p: ScrollProp| e.props.iter().any(|x| x.prop == p);
        Some(EffectNow {
            opacity: has(ScrollProp::Opacity).then(|| value(ScrollProp::Opacity)),
            shift: Vec2::new(value(ScrollProp::MoveX), value(ScrollProp::MoveY)),
            scale: value(ScrollProp::Scale),
            turn: value(ScrollProp::Rotation),
            transforms: [
                ScrollProp::MoveX,
                ScrollProp::MoveY,
                ScrollProp::Scale,
                ScrollProp::Rotation,
            ]
            .into_iter()
            .any(has),
        })
    }

    /// Whether `id` is `ancestor` or inside it.
    fn under(&self, id: NodeId, ancestor: NodeId) -> bool {
        let mut at = Some(id);
        while let Some(x) = at {
            if x == ancestor {
                return true;
            }
            at = self.doc.get(x).and_then(|n| n.parent());
        }
        false
    }
}

/// A scroll effect's values at the offset now.
#[derive(Clone, Copy, Debug)]
pub(crate) struct EffectNow {
    /// In place of the layer's own, when the effect drives it.
    pub opacity: Option<f64>,
    pub shift: Vec2,
    pub scale: f64,
    /// Degrees.
    pub turn: f64,
    /// Whether it moves, scales or turns the layer at all.
    pub transforms: bool,
}

impl EffectNow {
    /// The move, scale and turn about `centre`, in the space `centre` is in.
    pub fn affine(&self, centre: Point) -> Affine {
        let c = centre.to_vec2();
        Affine::translate(self.shift + c)
            * Affine::rotate(self.turn.to_radians())
            * Affine::scale(self.scale)
            * Affine::translate(-c)
    }
}

/// The side a directional transition moves toward, as a unit vector.
fn toward(d: Direction) -> Vec2 {
    match d {
        Direction::Left | Direction::None => Vec2::new(-1.0, 0.0),
        Direction::Right => Vec2::new(1.0, 0.0),
        Direction::Up => Vec2::new(0.0, -1.0),
        Direction::Down => Vec2::new(0.0, 1.0),
    }
}

fn opposite(d: Direction) -> Direction {
    match d {
        Direction::Left => Direction::Right,
        Direction::Right => Direction::Left,
        Direction::Up => Direction::Down,
        Direction::Down => Direction::Up,
        Direction::None => Direction::None,
    }
}

/// The outgoing and incoming screens at progress `t`, outgoing first.
fn screen_layers(a: &ScreenAnim, t: f64, size: Size) -> (Layer, Layer) {
    let span = Vec2::new(size.width, size.height);
    let d = toward(a.transition.direction);
    let along = |k: f64| Point::new(d.x * span.x * k, d.y * span.y * k);
    let layer = |frame, at, opacity: f64| Layer {
        frame,
        at,
        opacity: opacity.clamp(0.0, 1.0) as f32,
        backdrop: None,
        overlay: false,
    };
    let (from_at, to_at, to_opacity) = match a.transition.kind {
        TransitionKind::Instant | TransitionKind::SmartAnimate => (Point::ZERO, Point::ZERO, 1.0),
        TransitionKind::Dissolve => (Point::ZERO, Point::ZERO, t),
        TransitionKind::MoveIn => (Point::ZERO, along(t - 1.0), 1.0),
        TransitionKind::MoveOut => (along(t), Point::ZERO, 1.0),
        TransitionKind::Push => (along(t), along(t - 1.0), 1.0),
        TransitionKind::Slide => (along(t * SLIDE_UNDER), along(t - 1.0), 1.0),
    };
    (layer(a.from, from_at, 1.0), layer(a.to, to_at, to_opacity))
}

/// An overlay at `at`, `t` of the way through opening by `kind`.
fn overlay_layer(
    o: &Overlay,
    at: Point,
    kind: TransitionKind,
    dir: Direction,
    t: f64,
    screen: Size,
) -> Layer {
    let d = toward(dir);
    let off = Vec2::new(d.x * screen.width, d.y * screen.height) * (t - 1.0);
    let (at, opacity) = match kind {
        TransitionKind::Instant | TransitionKind::SmartAnimate => (at, 1.0),
        TransitionKind::Dissolve => (at, t),
        TransitionKind::MoveIn | TransitionKind::Push | TransitionKind::Slide => (at + off, 1.0),
        // An overlay moving out of view as it opens is still read as arriving.
        TransitionKind::MoveOut => (at - off, 1.0),
    };
    let fade = t.clamp(0.0, 1.0) as f32;
    Layer {
        frame: o.frame,
        at,
        opacity: opacity.clamp(0.0, 1.0) as f32,
        backdrop: o.settings.backdrop.map(|c| c.multiply_alpha(fade)),
        overlay: true,
    }
}

fn mix(a: f64, b: f64, t: f64) -> f64 {
    a + (b - a) * t
}

/// Two effect stacks at `t` between them: shadows' offsets, blurs, spreads and
/// colours and the blurs' radii move. `None` when the stacks are not the same
/// kinds in the same order, which then show the end's at once.
fn lerp_effects(a: &[Keyed<Effect>], b: &[Keyed<Effect>], t: f64) -> Option<Vec<Keyed<Effect>>> {
    if a.len() != b.len() {
        return None;
    }
    let shadow = |x: &Shadow, y: &Shadow| Shadow {
        offset: x.offset.lerp(y.offset, t),
        blur: mix(x.blur, y.blur, t).max(0.0),
        spread: mix(x.spread, y.spread, t),
        color: lerp_color(x.color, y.color, t.clamp(0.0, 1.0) as f32),
        ..y.clone()
    };
    a.iter()
        .zip(b)
        .map(|(x, y)| {
            let kind = match (&x.value.kind, &y.value.kind) {
                (EffectKind::DropShadow(p), EffectKind::DropShadow(q)) => {
                    EffectKind::DropShadow(shadow(p, q))
                }
                (EffectKind::InnerShadow(p), EffectKind::InnerShadow(q)) => {
                    EffectKind::InnerShadow(shadow(p, q))
                }
                (EffectKind::LayerBlur { radius: p }, EffectKind::LayerBlur { radius: q }) => {
                    EffectKind::LayerBlur {
                        radius: mix(*p, *q, t).max(0.0),
                    }
                }
                (
                    EffectKind::BackgroundBlur { radius: p },
                    EffectKind::BackgroundBlur { radius: q },
                ) => EffectKind::BackgroundBlur {
                    radius: mix(*p, *q, t).max(0.0),
                },
                (p, q) if std::mem::discriminant(p) == std::mem::discriminant(q) => q.clone(),
                _ => return None,
            };
            Some(y.map(|e| Effect {
                kind: kind.clone(),
                ..e.clone()
            }))
        })
        .collect()
}

/// Two brushes at `t` between them, colours at `fade`: two colours mix; two
/// gradients of one kind move their geometry, opacity and ramp, the ramps
/// sampled at every offset either has; a colour reads as a flat ramp of the
/// other's kind. `None` for anything else (an image, two kinds), which then
/// shows the end's at once.
fn lerp_brush(a: &Brush, b: &Brush, t: f64, fade: f32) -> Option<Brush> {
    use mastik_core::GradientBrush;
    use mastik_core::peniko::{ColorStop, Gradient, GradientKind};
    let flat = |c: Color, like: &GradientBrush| GradientBrush {
        gradient: Gradient {
            stops: [0.0, 1.0]
                .into_iter()
                .map(|offset| ColorStop {
                    offset,
                    color: c.into(),
                })
                .collect::<Vec<_>>()
                .as_slice()
                .into(),
            ..like.gradient.clone()
        },
        ..like.clone()
    };
    let (x, y) = match (a, b) {
        (Brush::Solid(x), Brush::Solid(y)) => return Some(Brush::Solid(lerp_color(*x, *y, fade))),
        (Brush::Gradient(x), Brush::Gradient(y)) => (x.clone(), y.clone()),
        (Brush::Solid(c), Brush::Gradient(y)) => (flat(*c, y), y.clone()),
        (Brush::Gradient(x), Brush::Solid(c)) => (x.clone(), flat(*c, x)),
        _ => return None,
    };
    let pt = |p: Point, q: Point| p.lerp(q, t);
    let m = |p: f32, q: f32| p + (q - p) * t as f32;
    let kind = match (x.gradient.kind, y.gradient.kind) {
        (GradientKind::Linear(p), GradientKind::Linear(q)) => {
            GradientKind::Linear(mastik_core::peniko::LinearGradientPosition {
                start: pt(p.start, q.start),
                end: pt(p.end, q.end),
            })
        }
        (GradientKind::Radial(p), GradientKind::Radial(q)) => {
            GradientKind::Radial(mastik_core::peniko::RadialGradientPosition {
                start_center: pt(p.start_center, q.start_center),
                start_radius: m(p.start_radius, q.start_radius).max(0.0),
                end_center: pt(p.end_center, q.end_center),
                end_radius: m(p.end_radius, q.end_radius).max(0.0),
            })
        }
        (GradientKind::Sweep(p), GradientKind::Sweep(q)) => {
            GradientKind::Sweep(mastik_core::peniko::SweepGradientPosition {
                center: pt(p.center, q.center),
                start_angle: m(p.start_angle, q.start_angle),
                end_angle: m(p.end_angle, q.end_angle),
            })
        }
        _ => return None,
    };
    let mut offsets: Vec<f32> = x
        .gradient
        .stops
        .iter()
        .chain(y.gradient.stops.iter())
        .map(|s| s.offset)
        .collect();
    offsets.sort_by(f32::total_cmp);
    offsets.dedup();
    let stops: Vec<ColorStop> = offsets
        .into_iter()
        .map(|o| ColorStop {
            offset: o,
            color: lerp_color(
                ramp_at(&x.gradient.stops, o),
                ramp_at(&y.gradient.stops, o),
                fade,
            )
            .into(),
        })
        .collect();
    Some(Brush::Gradient(GradientBrush {
        gradient: Gradient {
            kind,
            stops: stops.as_slice().into(),
            ..y.gradient
        },
        transform: lerp_affine(x.transform, y.transform, t),
        opacity: m(x.opacity, y.opacity).clamp(0.0, 1.0),
    }))
}

/// A ramp's colour at `offset`, straight between the stops either side; past
/// the ends, the end's.
fn ramp_at(stops: &[mastik_core::peniko::ColorStop], offset: f32) -> Color {
    let srgb = |s: &mastik_core::peniko::ColorStop| s.color.to_alpha_color();
    let Some(first) = stops.first() else {
        return Color::TRANSPARENT;
    };
    if offset <= first.offset {
        return srgb(first);
    }
    for w in stops.windows(2) {
        let (p, q) = (&w[0], &w[1]);
        if offset <= q.offset {
            let span = q.offset - p.offset;
            let k = match span > f32::EPSILON {
                true => (offset - p.offset) / span,
                false => 1.0,
            };
            return lerp_color(srgb(p), srgb(q), k);
        }
    }
    stops.last().map_or(Color::TRANSPARENT, srgb)
}

fn lerp_color(a: Color, b: Color, t: f32) -> Color {
    let (x, y) = (a.components, b.components);
    Color::new([
        x[0] + (y[0] - x[0]) * t,
        x[1] + (y[1] - x[1]) * t,
        x[2] + (y[2] - x[2]) * t,
        x[3] + (y[3] - x[3]) * t,
    ])
}

/// `a` to `b` at `t`, moving, turning and scaling rather than blending the
/// matrices: the linear part read as a rotation of a scale and a shear, each
/// interpolated, the angle the short way round.
pub(crate) fn lerp_affine(a: Affine, b: Affine, t: f64) -> Affine {
    if a == b {
        return a;
    }
    let parts = |m: Affine| {
        let [a, b, c, d, e, f] = m.as_coeffs();
        let sx = a.hypot(b);
        let angle = b.atan2(a);
        let (cos, sin) = (angle.cos(), angle.sin());
        let shear = c * cos + d * sin;
        let sy = -c * sin + d * cos;
        (angle, sx, sy, shear, e, f)
    };
    let (a0, sx0, sy0, k0, e0, f0) = parts(a);
    let (a1, sx1, sy1, k1, e1, f1) = parts(b);
    let mut turn = a1 - a0;
    if turn > std::f64::consts::PI {
        turn -= std::f64::consts::TAU;
    } else if turn < -std::f64::consts::PI {
        turn += std::f64::consts::TAU;
    }
    let mix = |x: f64, y: f64| x + (y - x) * t;
    let (angle, sx, sy, k) = (a0 + turn * t, mix(sx0, sx1), mix(sy0, sy1), mix(k0, k1));
    let (cos, sin) = (angle.cos(), angle.sin());
    Affine::new([
        sx * cos,
        sx * sin,
        k * cos - sy * sin,
        k * sin + sy * cos,
        mix(e0, e1),
        mix(f0, f1),
    ])
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A turn interpolates the angle, not the matrix, so the half-way shape keeps
    /// its size.
    ///
    /// flip: `lerp_affine` mixing the coefficients directly → fails at "keeps its
    /// scale".
    #[test]
    fn an_affine_turns_rather_than_shrinks() {
        let a = Affine::IDENTITY;
        let b = Affine::rotate(std::f64::consts::FRAC_PI_2);
        let mid = lerp_affine(a, b, 0.5);
        let [m0, m1, ..] = mid.as_coeffs();
        assert!((m0.hypot(m1) - 1.0).abs() < 1e-9, "keeps its scale");
        let end = lerp_affine(a, b, 1.0);
        for (x, y) in end.as_coeffs().iter().zip(b.as_coeffs()) {
            assert!((x - y).abs() < 1e-9, "reaches the end");
        }
        let moved = lerp_affine(
            Affine::translate((0.0, 0.0)),
            Affine::translate((10.0, 4.0)),
            0.5,
        );
        assert_eq!(moved.translation(), Vec2::new(5.0, 2.0));
    }

    fn linear(end: f64, stops: &[(f32, Color)]) -> Brush {
        use mastik_core::peniko::{ColorStop, Gradient};
        let stops: Vec<ColorStop> = stops
            .iter()
            .map(|(offset, c)| ColorStop {
                offset: *offset,
                color: (*c).into(),
            })
            .collect();
        Brush::Gradient(mastik_core::GradientBrush {
            gradient: Gradient {
                stops: stops.as_slice().into(),
                ..Gradient::new_linear((0.0, 0.0), (end, 0.0))
            },
            transform: Affine::IDENTITY,
            opacity: 1.0,
        })
    }

    /// Two linear ramps half way: the end moves half way, and a ramp of two
    /// stops against one of three is sampled at all three offsets.
    ///
    /// flip: `lerp_brush` keeping the start's offsets alone → fails at the
    /// offsets' `assert_eq`.
    #[test]
    fn a_gradient_moves_its_end_and_its_ramp() {
        let (black, white) = (Color::from_rgb8(0, 0, 0), Color::from_rgb8(255, 255, 255));
        let a = linear(100.0, &[(0.0, black), (1.0, black)]);
        let b = linear(200.0, &[(0.0, white), (0.5, black), (1.0, white)]);
        let Some(Brush::Gradient(g)) = lerp_brush(&a, &b, 0.5, 0.5) else {
            panic!("two linear ramps move");
        };
        let mastik_core::peniko::GradientKind::Linear(p) = g.gradient.kind else {
            panic!("still linear");
        };
        assert_eq!(p.end, Point::new(150.0, 0.0), "the end moves half way");
        let offsets: Vec<f32> = g.gradient.stops.iter().map(|s| s.offset).collect();
        assert_eq!(offsets, [0.0, 0.5, 1.0]);
        let grey = |i: usize| {
            g.gradient.stops[i]
                .color
                .to_alpha_color::<mastik_core::peniko::color::Srgb>()
                .components[0]
        };
        assert!((grey(0) - 0.5).abs() < 1e-6, "an end stop is half way");
        assert!(grey(1).abs() < 1e-6, "the middle stop is half way");
    }

    /// A colour against a gradient reads as a flat ramp, so it fades into it;
    /// a gradient against an image cannot move.
    #[test]
    fn a_colour_fades_into_a_gradient() {
        let red = Color::from_rgb8(255, 0, 0);
        let b = linear(100.0, &[(0.0, Color::from_rgb8(0, 0, 255)), (1.0, red)]);
        let Some(Brush::Gradient(g)) = lerp_brush(&Brush::Solid(red), &b, 0.5, 0.5) else {
            panic!("a colour and a ramp move");
        };
        let first = g.gradient.stops[0]
            .color
            .to_alpha_color::<mastik_core::peniko::color::Srgb>();
        assert!((first.components[0] - 0.5).abs() < 1e-6);
        assert!((first.components[2] - 0.5).abs() < 1e-6);
    }
}
