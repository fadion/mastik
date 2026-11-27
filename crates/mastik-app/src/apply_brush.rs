//! *Apply brush…* (`docs/design/app-booleans.md`, *Apply brush*): a popover in
//! the inspector's lane that bends a shape along a path or scatters it along it,
//! as a live brush on the path (`mastik_core::live_brush`) or baked into paths
//! (`mastik_core::apply_brush`), previewed live and committed as one *Apply
//! brush* step. *Edit brush…* reopens it on brushed layers.

use crate::app::MastikApp;
use crate::ui::{self, FieldButton, Prefix, Scrub};
use eframe::egui;
use mastik_core::apply_brush::{self, ApplyBrush, Art, Mode, Refusal, Scatter, Stretch, Turn};
use mastik_core::live_brush::{self, BrushLink};
use mastik_core::{IdSource, NodeId, Operation, Transaction};

/// The card's width: a popover's.
const W: f32 = crate::app::POPOVER_W;

/// The card's numbers, as the fields show them (percentages, the seed whole):
/// what a right-click puts back when it cancels a scrub of one.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct Numbers {
    width: f64,
    spacing: f64,
    size: f64,
    size_varies: f64,
    scatter: f64,
    seed: f64,
}

impl Default for Numbers {
    fn default() -> Self {
        let (a, s) = (Art::DEFAULT, Scatter::DEFAULT);
        Self {
            width: a.width * 100.0,
            spacing: s.spacing * 100.0,
            size: s.size * 100.0,
            size_varies: s.size_varies * 100.0,
            scatter: s.scatter * 100.0,
            seed: f64::from(s.seed),
        }
    }
}

/// The options *Edit brush*'s layers differ in, each shown as *Mixed* until it
/// is set; untouched, each layer keeps its own. `kind` is Art against Scatter,
/// and while it stands each layer keeps its whole mode.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub(crate) struct Mixed {
    /// The brush layer itself, until *Pick a shape…* names one.
    brush: bool,
    kind: bool,
    width: bool,
    middle: bool,
    flip: bool,
    spacing: bool,
    size: bool,
    size_varies: bool,
    scatter: bool,
    seed: bool,
    fixed: bool,
}

impl Mixed {
    /// What `modes` differ in, each option compared among the modes of its kind.
    fn of(modes: &[Mode]) -> Mixed {
        let arts: Vec<Art> = modes
            .iter()
            .filter_map(|m| match m {
                Mode::Art(a) => Some(*a),
                Mode::Scatter(_) => None,
            })
            .collect();
        let scatters: Vec<Scatter> = modes
            .iter()
            .filter_map(|m| match m {
                Mode::Scatter(s) => Some(*s),
                Mode::Art(_) => None,
            })
            .collect();
        fn differ<T, V: PartialEq>(all: &[T], f: impl Fn(&T) -> V) -> bool {
            all.windows(2).any(|w| f(&w[0]) != f(&w[1]))
        }
        Mixed {
            brush: false,
            kind: !arts.is_empty() && !scatters.is_empty(),
            width: differ(&arts, |a| a.width),
            middle: differ(&arts, |a| a.stretch),
            flip: differ(&arts, |a| a.flip),
            spacing: differ(&scatters, |s| s.spacing),
            size: differ(&scatters, |s| s.size),
            size_varies: differ(&scatters, |s| s.size_varies),
            scatter: differ(&scatters, |s| s.scatter),
            seed: differ(&scatters, |s| s.seed),
            fixed: differ(&scatters, |s| s.turn),
        }
    }

    /// The number fields `after` moved off `before` set, no longer mixed.
    fn settle(&mut self, before: Numbers, after: Numbers) {
        self.width &= before.width == after.width;
        self.spacing &= before.spacing == after.spacing;
        self.size &= before.size == after.size;
        self.size_varies &= before.size_varies == after.size_varies;
        self.scatter &= before.scatter == after.scatter;
        self.seed &= before.seed == after.seed;
    }
}

/// A value field's formatter showing *Mixed* in place of the number while
/// `mixed`, `plain` otherwise.
fn mixed_or(
    mixed: bool,
    plain: impl Fn(egui::DragValue<'_>) -> egui::DragValue<'_>,
) -> impl Fn(egui::DragValue<'_>) -> egui::DragValue<'_> {
    move |d| match mixed {
        true => d.custom_formatter(|_, _| ui::MIXED_WORD.into()),
        false => plain(d),
    }
}

/// The card's settings, kept closed so it reopens on the brush last applied.
#[derive(Default)]
pub(crate) struct BrushCard {
    pub(crate) open: bool,
    /// The path the brush runs along and the shape it is made of, taken from the
    /// selection when the card opens; the brush may be picked on the canvas.
    path: Option<NodeId>,
    brush: Option<NodeId>,
    /// *Scatter* rather than *Art*.
    pub(crate) scatter: bool,
    numbers: Numbers,
    middle: bool,
    flip: bool,
    fixed: bool,
    keep_path: bool,
    /// *Bake into paths*: a commit bakes the brush into plain paths rather than
    /// linking it. Off by default.
    bake: bool,
    /// The brushed layers *Edit brush* is open on; empty while the card applies
    /// a brush.
    pub(crate) editing: Vec<NodeId>,
    /// What the edited layers differ in; nothing while the card applies a brush.
    mixed: Mixed,
    /// Whether *Pick a shape…* named a brush since the card opened: only then
    /// does *Edit brush* relink the layers it edits.
    relinked: bool,
    /// Whether the canvas is waiting for a click on a shape to use as the brush.
    pub(crate) picking: bool,
    /// The selection the card opened on: another closes it.
    opened_on: Vec<NodeId>,
    /// The preview built for the key it was built from.
    built: Option<(Key, Transaction)>,
}

/// What a preview was built from: the revision, whether it links, the
/// application and what is still mixed.
type Key = (u64, bool, ApplyBrush, Mixed);

/// What a right-click puts back when it cancels a scrub: the numbers, and which
/// of them were mixed.
pub(crate) type Scrubbed = (Numbers, Mixed);

impl BrushCard {
    /// Put back the numbers a right-click cancelled the scrub of.
    pub(crate) fn restore_scrub(&mut self, (numbers, mixed): Scrubbed) {
        self.numbers = numbers;
        self.mixed = mixed;
    }

    /// The application the card describes, while it has a path and a brush.
    fn how(&self) -> Option<ApplyBrush> {
        Some(ApplyBrush {
            path: self.path?,
            brush: self.brush?,
            mode: self.mode(),
            keep_path: self.keep_path,
        })
    }

    /// Whether a commit links the brush rather than baking it.
    fn live(&self) -> bool {
        !self.editing.is_empty() || !self.bake
    }

    /// The layers a commit writes the link onto: the edited ones, or the path.
    fn targets(&self) -> Vec<NodeId> {
        match self.editing.is_empty() {
            true => self.path.into_iter().collect(),
            false => self.editing.clone(),
        }
    }

    /// The card's settings taken from `mode`, as *Edit brush* opens on a layer.
    fn load(&mut self, mode: Mode) {
        match mode {
            Mode::Art(a) => {
                self.scatter = false;
                self.numbers.width = a.width * 100.0;
                self.middle = a.stretch == Stretch::Middle;
                self.flip = a.flip;
            }
            Mode::Scatter(s) => {
                self.scatter = true;
                self.numbers.spacing = s.spacing * 100.0;
                self.numbers.size = s.size * 100.0;
                self.numbers.size_varies = s.size_varies * 100.0;
                self.numbers.scatter = s.scatter * 100.0;
                self.numbers.seed = f64::from(s.seed);
                self.fixed = s.turn == Turn::Fixed;
            }
        }
    }

    /// The options a commit writes on a layer whose own are `own`: the card's,
    /// but where they are still mixed, which keep the layer's own.
    fn mode_for(&self, own: Option<Mode>) -> Mode {
        fn pick<T>(mixed: bool, own: T, card: T) -> T {
            if mixed { own } else { card }
        }
        let (x, card) = (self.mixed, self.mode());
        match (own, card) {
            (Some(own), _) if x.kind => own,
            (Some(Mode::Art(o)), Mode::Art(c)) => Mode::Art(Art {
                width: pick(x.width, o.width, c.width),
                stretch: pick(x.middle, o.stretch, c.stretch),
                flip: pick(x.flip, o.flip, c.flip),
            }),
            (Some(Mode::Scatter(o)), Mode::Scatter(c)) => Mode::Scatter(Scatter {
                spacing: pick(x.spacing, o.spacing, c.spacing),
                size: pick(x.size, o.size, c.size),
                size_varies: pick(x.size_varies, o.size_varies, c.size_varies),
                scatter: pick(x.scatter, o.scatter, c.scatter),
                turn: pick(x.fixed, o.turn, c.turn),
                seed: pick(x.seed, o.seed, c.seed),
            }),
            _ => card,
        }
    }

    /// The brush and options the card says, as a link.
    fn mode(&self) -> Mode {
        let n = self.numbers;
        match self.scatter {
            false => Mode::Art(Art {
                width: n.width / 100.0,
                stretch: match self.middle {
                    true => Stretch::Middle,
                    false => Stretch::Whole,
                },
                flip: self.flip,
            }),
            true => Mode::Scatter(Scatter {
                spacing: n.spacing / 100.0,
                size: n.size / 100.0,
                size_varies: n.size_varies / 100.0,
                scatter: n.scatter / 100.0,
                turn: match self.fixed {
                    true => Turn::Fixed,
                    false => Turn::Along,
                },
                seed: n.seed.round().clamp(0.0, f64::from(u32::MAX)) as u32,
            }),
        }
    }

    /// The path the open card shows dashed on the canvas.
    pub(crate) fn shown_path(&self) -> Option<NodeId> {
        self.path.filter(|_| self.open)
    }
}

enum Act {
    Close,
    Commit,
    Pick,
    Swap,
    Scatter(bool),
    Middle(bool),
    Flip,
    Fixed(bool),
    KeepPath,
    Bake,
    BakeBrush,
    RemoveBrush,
}

/// What the status line says while the canvas waits for a shape.
pub(crate) const PICK_STATUS: &str = "Click a shape to use as the brush · Esc to cancel";

/// The undo step a commit makes.
const STEP: &str = "Apply brush";

/// The undo step *Edit brush*'s *Done* makes.
const EDIT_STEP: &str = "Edit brush";

/// The undo steps of the two acts on a live brush.
pub(crate) const BAKE_STEP: &str = "Bake brush";
pub(crate) const REMOVE_STEP: &str = "Remove brush";

/// The row's and the palette's sentence when the selection cannot take a brush.
pub(crate) const NEEDS: &str = "Select a path, and the shape to run along it";

/// The sentence for *Edit brush…* and its two acts over layers without a brush.
pub(crate) const NO_BRUSH: &str = "Select a layer drawn with a brush";

/// Why *Keep the path* is dimmed while the card links the brush.
const KEEPS_PATH: &str = "A live brush keeps the path: it is the layer";

/// *Edit brush*'s note in place of the options while its layers' brushes are of
/// both kinds.
const KINDS_DIFFER: &str = "These layers' brushes are Art and Scatter; pick one to set them all";

/// Why *Swap brush and path* is dimmed in *Edit brush*.
const OWN_PATH: &str = "A live brush's path is its own layer";

/// The Brush dropdown's face while the card has no brush, opened on a path alone.
const NONE: &str = "None";

/// Why *Apply* and *Swap brush and path* are dimmed while the card has no brush.
const NO_SHAPE: &str = "Pick a shape to use as the brush";

impl MastikApp {
    /// Why the selection cannot take a brush, if it cannot: the row's sentence.
    /// A live brush's tests, since *Apply brush* links by default; a bake's own
    /// are asked when it commits.
    pub(crate) fn brush_refusal(&self) -> Option<&'static str> {
        live_brush::selection_refusal(
            &self.session.doc,
            &self.session.resolved,
            self.session.selection.ids(),
        )
        .map(|r| r.says())
    }

    /// Why the selection offers no *Edit brush…*, if it does not.
    pub(crate) fn edit_brush_refusal(&self) -> Option<&'static str> {
        live_brush::brushed_in(&self.session.doc, self.session.selection.ids())
            .is_empty()
            .then_some(NO_BRUSH)
    }

    /// Why the card's own path and brush no longer fit, if they do not: a live
    /// link's tests, which hold for a bake's card too, or each edited layer's.
    /// A card with no brush yet is held to the path's half.
    fn card_refusal(&self) -> Option<Refusal> {
        let (doc, res) = (&self.session.doc, &self.session.resolved);
        let Some(brush) = self.brush.brush else {
            return match (self.brush.path, self.brush.editing.is_empty()) {
                (Some(path), true) => apply_brush::path_refusal(doc, res, path, false),
                _ => Some(Refusal::Count),
            };
        };
        for path in self.brush.targets() {
            if doc.get(path).and_then(|n| n.brush()).is_none() && !self.brush.editing.is_empty() {
                return Some(Refusal::Count);
            }
            if let Some(r) = live_brush::refusal(doc, res, path, brush) {
                return Some(r);
            }
        }
        None
    }

    /// Open the card on the two selected layers, or on one path with no brush
    /// yet for *Pick a shape…* to name, or say why it cannot.
    pub(crate) fn open_brush(&mut self) {
        if let Some(why) = self.brush_refusal() {
            return self.session.info(why);
        }
        let (path, brush) = match self.session.selection.ids() {
            [path] => (*path, None),
            ids => match apply_brush::pair(&self.session.doc, ids) {
                Ok((path, brush)) => (path, Some(brush)),
                Err(_) => return self.session.info(NEEDS),
            },
        };
        // One card in the popover lane at a time.
        self.close_inspector_popovers();
        self.close_repeat();
        self.close_blend();
        self.close_warp();
        let card = &mut self.brush;
        card.open = true;
        card.picking = false;
        card.editing.clear();
        card.mixed = Mixed::default();
        card.relinked = false;
        card.path = Some(path);
        card.brush = brush;
        card.opened_on = self.session.selection.ids().to_vec();
        card.built = None;
    }

    /// *Edit brush…*: the card on the selected brushed layers, headed *Edit
    /// brush* and filled with the first one's brush and options; an option the
    /// layers differ in shows *Mixed*.
    pub(crate) fn open_edit_brush(&mut self) {
        let doc = &self.session.doc;
        let layers = live_brush::brushed_in(doc, self.session.selection.ids());
        let Some(link) = layers.first().and_then(|id| doc.get(*id)?.brush()) else {
            return self.session.info(NO_BRUSH);
        };
        let links: Vec<BrushLink> = layers
            .iter()
            .filter_map(|id| doc.get(*id)?.brush())
            .collect();
        let modes: Vec<Mode> = links.iter().map(|l| l.mode).collect();
        self.close_inspector_popovers();
        self.close_repeat();
        self.close_blend();
        self.close_warp();
        let card = &mut self.brush;
        card.open = true;
        card.picking = false;
        card.path = layers.first().copied();
        card.brush = Some(link.of);
        // Each kind's fields from the first layer of that kind, so either kind
        // picked over a mixed one shows a layer's numbers; the first layer's last.
        let first_of = |scatter: bool| {
            modes
                .iter()
                .find(|m| matches!(m, Mode::Scatter(_)) == scatter)
        };
        for m in [first_of(false), first_of(true)].into_iter().flatten() {
            card.load(*m);
        }
        card.load(link.mode);
        card.mixed = Mixed {
            brush: links.iter().any(|l| l.of != link.of),
            ..Mixed::of(&modes)
        };
        card.editing = layers;
        card.relinked = false;
        card.opened_on = self.session.selection.ids().to_vec();
        card.built = None;
    }

    /// The links the card writes: each target linked to its brush with its
    /// options, an edited layer keeping whether its brush shows.
    fn card_links(&self) -> Vec<Operation> {
        let Some(brush) = self.brush.brush else {
            return Vec::new();
        };
        self.brush
            .targets()
            .into_iter()
            .map(|id| {
                let own = self.session.doc.get(id).and_then(|n| n.brush());
                let mode = self.brush.mode_for(own.map(|l| l.mode));
                // An edited layer keeps its own brush until *Pick a shape…*
                // names another.
                let of = match own {
                    Some(l) if !self.brush.relinked => l.of,
                    _ => brush,
                };
                Operation::SetBrush {
                    id,
                    brush: Some(BrushLink {
                        of,
                        mode,
                        visible: own.is_none_or(|l| l.visible),
                    }),
                }
            })
            .collect()
    }

    /// *Bake brush*: each selected brushed layer's brush baked into paths, one
    /// *Bake brush* step; the groups are selected.
    pub(crate) fn bake_brushes(&mut self) {
        let layers = live_brush::brushed_in(&self.session.doc, self.session.selection.ids());
        if layers.is_empty() {
            return self.session.info(NO_BRUSH);
        }
        let name = layers
            .first()
            .and_then(|id| self.session.doc.get(*id)?.brush())
            .map(|l| crate::private::layer(&self.session.doc, l.of).into_owned())
            .unwrap_or_default();
        let mut ops = Vec::new();
        let mut groups = Vec::new();
        for id in layers {
            let (doc, res) = (&self.session.doc, &self.session.resolved);
            if let Some((baked, group)) = live_brush::bake(doc, res, &mut self.session.ids, id) {
                ops.extend(baked);
                groups.extend(group);
            }
        }
        self.close_brush();
        if !self.session.has_step_name() {
            self.session
                .name_step(BAKE_STEP, crate::menu::Item::BakeBrush.spec().glyph);
        }
        if !self.commit_edit(Transaction(ops)) {
            return;
        }
        let paths: usize = groups
            .iter()
            .filter_map(|g| self.session.doc.get(*g))
            .map(|g| g.children().len())
            .sum();
        if !groups.is_empty() {
            self.session.selection.set(groups);
        }
        self.session.info(format!(
            "Baked {name} into {}",
            ui::count_as(paths, "path", "paths")
        ));
    }

    /// *Remove brush*: each selected layer's link dropped, its own strokes
    /// drawn again, one *Remove brush* step.
    pub(crate) fn remove_brushes(&mut self) {
        let layers = live_brush::brushed_in(&self.session.doc, self.session.selection.ids());
        if layers.is_empty() {
            return self.session.info(NO_BRUSH);
        }
        let ops = layers
            .iter()
            .map(|id| Operation::SetBrush {
                id: *id,
                brush: None,
            })
            .collect();
        self.close_brush();
        if !self.session.has_step_name() {
            self.session
                .name_step(REMOVE_STEP, crate::menu::Item::RemoveBrush.spec().glyph);
        }
        self.commit_edit(Transaction(ops));
    }

    /// Close the card and take its preview off the canvas.
    pub(crate) fn close_brush(&mut self) {
        if self.brush.open {
            self.brush.open = false;
            self.brush.picking = false;
            self.brush.built = None;
            self.brush.editing.clear();
            self.session.clear_gesture_preview();
        }
    }

    /// Whether the canvas waits for a click on a shape for the card's brush.
    pub(crate) fn brush_picking(&self) -> bool {
        self.brush.open && self.brush.picking
    }

    /// Stop waiting for a shape, the card as it was. Answers whether it was waiting.
    pub(crate) fn cancel_brush_pick(&mut self) -> bool {
        if !self.brush_picking() {
            return false;
        }
        self.brush.picking = false;
        self.session.info("");
        true
    }

    /// What is under `world` for the wait: the outline under the pointer, then the
    /// leaf.
    fn brush_hits(&self, world: mastik_core::kurbo::Point) -> [Option<NodeId>; 2] {
        [self.rail_at(world), self.pick_leaf(world)]
    }

    /// The shape under `world` the card could take as its brush: one that is not
    /// the path and has an outline to lay along it.
    pub(crate) fn brush_shape_at(&self, world: mastik_core::kurbo::Point) -> Option<NodeId> {
        let (doc, res) = (&self.session.doc, &self.session.resolved);
        self.brush_hits(world).into_iter().flatten().find(|id| {
            Some(*id) != self.brush.path && apply_brush::brush_refusal(doc, res, *id).is_none()
        })
    }

    /// A click while the canvas waits for a shape: a shape the card can take
    /// becomes the brush and the card comes back; the path itself is refused by
    /// name; anything else is passed over.
    pub(crate) fn brush_pick_at(&mut self, world: mastik_core::kurbo::Point) {
        if let Some(id) = self.brush_shape_at(world) {
            self.brush.brush = Some(id);
            self.brush.relinked = true;
            self.brush.picking = false;
            self.session.info("");
        } else if self.brush.path.is_some() && self.brush_hits(world).contains(&self.brush.path) {
            self.session.info(Refusal::Same.says());
        }
    }

    /// Swap the card's brush and path, or say why the swap cannot run.
    fn swap_brush(&mut self) {
        let (Some(path), Some(brush)) = (self.brush.path, self.brush.brush) else {
            return;
        };
        let (doc, res) = (&self.session.doc, &self.session.resolved);
        let refused = match self.brush.bake {
            true => apply_brush::refusal(doc, res, brush, path),
            false => live_brush::refusal(doc, res, brush, path),
        };
        match refused {
            Some(r) => self.session.info(r.says()),
            None => (self.brush.path, self.brush.brush) = (Some(brush), Some(path)),
        }
    }

    /// Whether the gesture preview is the card's result rather than a gesture.
    pub(crate) fn brush_preview_standing(&self) -> bool {
        self.brush.open && self.drag.is_none()
    }

    /// Build the preview again when what it shows changed, and keep it up between
    /// other gestures. It is the commit's own edit, minted from a copy of the
    /// counter so it spends no ids: the group, and the path hidden, since a
    /// preview can neither delete nor reparent.
    fn refresh_brush_preview(&mut self) {
        let Some(how) = self.brush.how() else {
            return;
        };
        if !self.drag.is_none() {
            return;
        }
        let key: Key = (
            self.session.revision(),
            self.brush.live(),
            how,
            self.brush.mixed,
        );
        if self.brush.built.as_ref().is_none_or(|(k, _)| *k != key) && self.brush.live() {
            // The links themselves: the walk draws a previewed link as committed.
            let tx = Transaction(self.card_links());
            self.session.set_preview(&tx);
            self.brush.built = Some((key, tx));
        } else if self.brush.built.as_ref().is_none_or(|(k, _)| *k != key) {
            let mut minter = IdSource::new(self.session.ids.actor());
            minter.skip_to(self.session.ids.next_seq());
            let (doc, res) = (&self.session.doc, &self.session.resolved);
            let mut tx = apply_brush::apply(doc, res, &mut minter, &how)
                .map(|(tx, _)| tx)
                .unwrap_or_default();
            tx.0.retain(|op| matches!(op, mastik_core::Operation::InsertSubtree { .. }));
            if !tx.0.is_empty() {
                tx.0.push(mastik_core::Operation::SetVisible {
                    id: how.path,
                    visible: false,
                });
            }
            self.session.set_preview(&tx);
            self.brush.built = Some((key, tx));
        } else if !self.session.has_gesture_preview()
            && let Some((_, tx)) = &self.brush.built
        {
            let tx = tx.clone();
            self.session.set_preview(&tx);
        }
    }

    /// Apply the brush as the card says, one *Apply brush* step; the new group is
    /// selected.
    pub(crate) fn commit_brush(&mut self) {
        let Some(how) = self.brush.how() else {
            // `Enter` reaches here past the dimmed *Apply*, and says what it says.
            return self.session.info(match self.brush.path {
                Some(_) => NO_SHAPE,
                None => NEEDS,
            });
        };
        if self.brush.live() {
            return self.commit_link(how);
        }
        let made = apply_brush::apply(
            &self.session.doc,
            &self.session.resolved,
            &mut self.session.ids,
            &how,
        );
        let (tx, group) = match made {
            Ok(made) => made,
            Err(r) => return self.session.info(r.says()),
        };
        let name = crate::private::layer(&self.session.doc, how.brush).into_owned();
        self.session.clear_gesture_preview();
        // The row's label less its ellipsis, which promises a card, not a step.
        if !self.session.has_step_name() {
            self.session
                .name_step(STEP, crate::menu::Item::ApplyBrush.spec().glyph);
        }
        if !self.commit_edit(tx) {
            return;
        }
        self.session.selection.set_one(group);
        let copies = self
            .session
            .doc
            .get(group)
            .map_or(0, |g| g.children().len() - usize::from(how.keep_path));
        self.session.info(match how.mode {
            Mode::Art(_) => format!("Brushed with {name}"),
            Mode::Scatter(_) => format!(
                "Scattered {} of {name}",
                ui::count_as(copies, "copy", "copies")
            ),
        });
        self.brush.open = false;
        self.brush.picking = false;
        self.brush.built = None;
    }

    /// Link the brush as the card says: *Apply brush* on the path, which stays
    /// selected, or *Edit brush*'s *Done* on the layers it edits.
    fn commit_link(&mut self, how: ApplyBrush) {
        if let Some(r) = self.card_refusal() {
            return self.session.info(r.says());
        }
        let editing = !self.brush.editing.is_empty();
        let targets = self.brush.targets();
        let tx = Transaction(self.card_links());
        let doc = &self.session.doc;
        let brush = crate::private::layer(doc, how.brush).into_owned();
        let path = crate::private::layer(doc, how.path).into_owned();
        self.session.clear_gesture_preview();
        if !self.session.has_step_name() {
            let (step, item) = match editing {
                true => (EDIT_STEP, crate::menu::Item::EditBrush),
                false => (STEP, crate::menu::Item::ApplyBrush),
            };
            self.session.name_step(step, item.spec().glyph);
        }
        self.brush.open = false;
        self.brush.picking = false;
        self.brush.built = None;
        if !self.commit_edit(tx) {
            return;
        }
        self.session.selection.set(targets.clone());
        self.session.info(match (how.mode, targets.len()) {
            (_, n @ 2..) => format!("Brushed {} with {brush}", ui::count(n, "layer")),
            (Mode::Art(_), _) => format!("Brushed {path} with {brush}"),
            (Mode::Scatter(_), _) => format!("Scattered {brush} along {path}"),
        });
    }

    /// Say [`PICK_STATUS`] again before it fades or once another message has
    /// replaced it, as *Blend shapes…*'s wait does, but leave a refused pick's
    /// sentence and an error up for their hold.
    fn keep_brush_pick_status(&mut self) {
        let status = self.session.status();
        let fading = status.at.elapsed() + std::time::Duration::from_secs(1)
            >= crate::app::status_hold(status);
        let other = status.text != PICK_STATUS
            && status.text != Refusal::Same.says()
            && status.kind != crate::session::StatusKind::Error;
        if other || fading {
            self.session.info(PICK_STATUS);
        }
    }

    /// Draw the card when it is open, keep its preview up, and act on this
    /// frame's clicks.
    pub(crate) fn brush_ui(&mut self, ctx: &egui::Context) {
        if !self.brush.open {
            return;
        }
        if self.card_refusal().is_some() || self.session.selection.ids() != self.brush.opened_on {
            return self.close_brush();
        }
        // A popover opened in the lane after the card takes its place.
        if self.inspector_popover_open() {
            return self.close_brush();
        }
        self.refresh_brush_preview();
        // The canvas has the pointer until a shape is clicked or Escape pressed, and
        // the status line says so for as long as the card is hidden.
        if self.brush.picking {
            self.keep_brush_pick_status();
            return;
        }
        let area = self.transform_popover_area(ctx, egui::Id::new("apply-brush"));
        let brushes_differ = self.brush.mixed.brush && !self.brush.relinked;
        let (brush_name, brush_glyph) = match self.brush.brush.and_then(|b| self.session.doc.get(b))
        {
            // Each layer keeps its own brush until a shape is picked.
            _ if brushes_differ => (ui::MIXED_WORD.to_owned(), ""),
            Some(n) => (
                crate::private::layer(&self.session.doc, n.id()).into_owned(),
                crate::panels::layers::kind_glyph(n.kind()).0,
            ),
            None => (String::new(), ""),
        };
        let card = &mut self.brush;
        // Before the fields: `value_field` applies this frame's drag delta inside
        // the call that reports the drag.
        let before = card.numbers;
        let before_mixed = card.mixed;
        let mixed = card.mixed;
        let mut scrubbing = false;
        let mut act = None;
        area.show(ctx, |ui| {
            ui::hold_tab(ui);
            ui::menu_frame(ui::POPOVER_PAD).show(ui, |ui| {
                let w = ui::menu_inner_w(W, ui::POPOVER_PAD);
                ui.set_width(w);
                ui.spacing_mut().item_spacing.y = ui::POPOVER_SECTION_GAP;
                let editing = !card.editing.is_empty();
                let head = if editing { "Edit brush" } else { "Apply brush" };
                if ui::popover_head(ui, head) {
                    act = Some(Act::Close);
                }
                ui::labelled(ui, "Brush", |ui| {
                    ui.scope(|ui| {
                        ui::combo_height(ui);
                        let shown = match card.brush.is_some() || brushes_differ {
                            true => format!("Shape · {brush_name}"),
                            false => NONE.to_owned(),
                        };
                        egui::ComboBox::from_id_salt("apply-brush-shape")
                            .icon(ui::combo_chevron)
                            .width(w)
                            .selected_text(ui::glyph_and_text(brush_glyph, &shown))
                            .show_ui(ui, |ui| {
                                ui::menu_rows(ui);
                                let row = ui::MENU_ROW_H;
                                // Already chosen: a click keeps it.
                                ui::menu_check(ui, &shown, true, row);
                                ui::menu_sep(ui);
                                if ui::menu_check(ui, "Pick a shape…", false, row) {
                                    act = Some(Act::Pick);
                                }
                                let swap = ui::menu_row(
                                    ui,
                                    ui::MenuRow::new("", "Swap brush and path")
                                        .checked(false)
                                        .enabled(!editing && card.brush.is_some()),
                                    row,
                                );
                                if swap.clicked() {
                                    act = Some(Act::Swap);
                                }
                                swap.on_disabled_hover_text(match editing {
                                    true => OWN_PATH,
                                    false => NO_SHAPE,
                                });
                                if editing {
                                    ui::menu_sep(ui);
                                    if ui::menu_check(ui, "Bake brush", false, row) {
                                        act = Some(Act::BakeBrush);
                                    }
                                    if ui::menu_check(ui, "Remove brush", false, row) {
                                        act = Some(Act::RemoveBrush);
                                    }
                                }
                            })
                            .response
                            .on_hover_text("The shape bent or repeated along the path");
                    });
                });
                // Mixed: nothing raised (an index past the last cell) and the
                // first cell's word traded for `ui::segment_mixed`'s dash.
                let segment = |ui: &mut egui::Ui,
                               words: [&'static str; 2],
                               on: bool,
                               mixed: bool|
                 -> Option<bool> {
                    ui::segmented(
                        ui,
                        w,
                        ui::SEGMENT_CELL_H,
                        2,
                        if mixed { 2 } else { usize::from(on) },
                        |p, i, r, lit| match mixed && i == 0 {
                            true => ui::segment_mixed(p, r),
                            false => ui::segment_label(p, r, words[i], lit),
                        },
                    )
                    .map(|i| i == 1)
                };
                if let Some(on) = segment(ui, ["Art", "Scatter"], card.scatter, mixed.kind) {
                    act = Some(Act::Scatter(on));
                }
                // Both modes' rows take Scatter's height, so the footer stays put
                // and the card keeps its size as the mode changes.
                let body_h = body_height(ui);
                ui.vertical(|ui| {
                    ui.set_width(w);
                    ui.set_min_height(body_h);
                    let fw = (w - ui::CARD_COL_GAP) / 2.0;
                    let n = &mut card.numbers;
                    let mut field = |ui: &mut egui::Ui,
                                     width: f32,
                                     value: &mut f64,
                                     mixed: bool,
                                     range: std::ops::RangeInclusive<f64>,
                                     tip: &str| {
                        let resp = ui::value_field(
                            ui,
                            egui::vec2(width, ui::CONTROL_H),
                            Prefix::Text(""),
                            value,
                            Scrub::whole(0.5).range(range),
                            mixed_or(mixed, crate::trace::percent),
                        );
                        scrubbing |= resp.dragged();
                        resp.on_hover_text(tip);
                    };
                    // Each layer keeps its own kind until one is picked.
                    if mixed.kind {
                        ui::selection_note(ui, KINDS_DIFFER);
                        return;
                    }
                    if !card.scatter {
                        ui::labelled(ui, "Width", |ui| {
                            field(
                                ui,
                                w,
                                &mut n.width,
                                mixed.width,
                                1.0..=1000.0,
                                "How tall the shape draws across the path",
                            );
                        });
                        ui::labelled(ui, "Stretch", |ui| {
                            let words = ["Whole shape", "Middle only"];
                            if let Some(on) = segment(ui, words, card.middle, mixed.middle) {
                                act = Some(Act::Middle(on));
                            }
                        });
                        let flip = match mixed.flip {
                            true => {
                                ui::switch_row_mixed(ui, "Flip across", crate::settings::SWITCH_H)
                            }
                            false => ui::switch_row(
                                ui,
                                "Flip across",
                                card.flip,
                                crate::settings::SWITCH_H,
                            ),
                        };
                        if flip
                            .on_hover_text("Mirror the shape over the path")
                            .clicked()
                        {
                            act = Some(Act::Flip);
                        }
                        return;
                    }
                    crate::repeat::pair(ui, |ui, left| match left {
                        true => ui::labelled(ui, "Spacing", |ui| {
                            field(
                                ui,
                                fw,
                                &mut n.spacing,
                                mixed.spacing,
                                10.0..=1000.0,
                                "From one copy to the next, of the shape's length",
                            );
                        }),
                        false => ui::labelled(ui, "Size", |ui| {
                            field(
                                ui,
                                fw,
                                &mut n.size,
                                mixed.size,
                                1.0..=1000.0,
                                "Each copy's size, of the shape's",
                            );
                        }),
                    });
                    crate::repeat::pair(ui, |ui, left| match left {
                        true => ui::labelled(ui, "Size varies", |ui| {
                            field(
                                ui,
                                fw,
                                &mut n.size_varies,
                                mixed.size_varies,
                                0.0..=100.0,
                                "How far a copy's size strays either way",
                            );
                        }),
                        false => ui::labelled(ui, "Scatter", |ui| {
                            field(
                                ui,
                                fw,
                                &mut n.scatter,
                                mixed.scatter,
                                0.0..=500.0,
                                "How far a copy strays off the path, of the shape's length",
                            );
                        }),
                    });
                    ui::labelled(ui, "Turn", |ui| {
                        let words = ["Along the path", "Fixed"];
                        if let Some(on) = segment(ui, words, card.fixed, mixed.fixed) {
                            act = Some(Act::Fixed(on));
                        }
                    });
                    ui::labelled(ui, "Seed", |ui| {
                        let resp = ui::value_field(
                            ui,
                            egui::vec2(w, ui::CONTROL_H),
                            Prefix::Text(""),
                            &mut n.seed,
                            Scrub::whole(0.25).range(0.0..=f64::from(u32::MAX)),
                            mixed_or(mixed.seed, |d| d.custom_formatter(ui::number(0))),
                        );
                        scrubbing |= resp.dragged();
                        resp.on_hover_text("Which sizes and offsets: another number scatters anew");
                    });
                });
                // Baking is an act in *Edit brush*, a row, not an option.
                if !editing {
                    if ui::switch_row(ui, "Bake into paths", card.bake, crate::settings::SWITCH_H)
                        .on_hover_text("Bake the brush into plain paths instead of linking it")
                        .clicked()
                    {
                        act = Some(Act::Bake);
                    }
                    let keep = ui::disable_unless(ui, card.bake, |ui| {
                        ui::switch_row(
                            ui,
                            "Keep the path",
                            card.keep_path && card.bake,
                            crate::settings::SWITCH_H,
                        )
                    })
                    .inner;
                    if keep.clicked() {
                        act = Some(Act::KeepPath);
                    }
                    if card.bake {
                        keep.on_hover_text("Keep the path in the group, hidden");
                    } else {
                        keep.on_disabled_hover_text(KEEPS_PATH);
                    }
                }
                let primary = if editing { "Done" } else { "Apply" };
                let (state, why) = match card.brush {
                    Some(_) => (FieldButton::Primary, None),
                    None => (FieldButton::Disabled, Some(NO_SHAPE)),
                };
                match crate::settings::footer_row_noted(
                    ui,
                    &[("Cancel", FieldButton::Off), (primary, state)],
                    why,
                ) {
                    Some(0) => act = Some(Act::Close),
                    Some(_) => act = Some(Act::Commit),
                    None => {}
                }
            });
        });
        // Armed after a right-click's cancel is answered, so the frame it lands on
        // cannot re-arm it.
        if scrubbing && !self.gesture_cancelled && self.in_flight.brush_scrub.is_none() {
            self.in_flight.brush_scrub = Some((before, before_mixed));
        }
        // A number typed or scrubbed is set on every layer.
        let after = self.brush.numbers;
        self.brush.mixed.settle(before, after);
        let Some(act) = act else {
            return;
        };
        match act {
            Act::Close => self.close_brush(),
            Act::Commit => self.commit_brush(),
            Act::Pick => {
                self.brush.picking = true;
                self.session.info(PICK_STATUS);
            }
            Act::Swap => self.swap_brush(),
            Act::Scatter(on) => {
                self.brush.scatter = on;
                self.brush.mixed.kind = false;
            }
            Act::Middle(on) => {
                self.brush.middle = on;
                self.brush.mixed.middle = false;
            }
            // A mixed switch turns on, as every mixed switch does.
            Act::Flip => {
                self.brush.flip = self.brush.mixed.flip || !self.brush.flip;
                self.brush.mixed.flip = false;
            }
            Act::Fixed(on) => {
                self.brush.fixed = on;
                self.brush.mixed.fixed = false;
            }
            Act::KeepPath => self.brush.keep_path = !self.brush.keep_path,
            Act::Bake => self.brush.bake = !self.brush.bake,
            Act::BakeBrush => self.bake_brushes(),
            Act::RemoveBrush => self.remove_brushes(),
        }
        ctx.request_repaint();
    }
}

/// The height both modes' rows take: Scatter's, the taller — four labelled rows.
fn body_height(ui: &egui::Ui) -> f32 {
    // A laid-out word, not the font's row height: a label takes its galley's
    // height, which egui rounds to the pixel.
    let font = egui::FontId::proportional(ui::EYEBROW_PT);
    let eyebrow = ui
        .painter()
        .layout_no_wrap("SEED".into(), font, crate::theme::text::faint())
        .size()
        .y;
    let labelled = eyebrow + ui::SECTION_LABEL_GAP + ui::CONTROL_H;
    4.0 * labelled + 3.0 * ui::POPOVER_SECTION_GAP
}

#[cfg(test)]
mod tests {
    use super::*;
    use mastik_core::kurbo::{Affine, BezPath, Point, Size};
    use mastik_core::{Document, NodeKind, Operation};

    /// A headless app holding a path from (0, 300) to (600, 300) under a 60 × 20
    /// ellipse named *Leaf*, both selected.
    fn path_and_leaf(ctx: &egui::Context) -> (MastikApp, NodeId, NodeId) {
        let mut app = MastikApp::headless(ctx);
        let mut ids = IdSource::new(1);
        let root = ids.mint();
        let mut doc = Document::new(root);
        let page = doc.first_page();
        let (path, leaf) = (ids.mint(), ids.mint());
        let mut line = BezPath::new();
        line.move_to((0.0, 300.0));
        line.line_to((600.0, 300.0));
        doc.apply(&Transaction(vec![
            Operation::CreateNode {
                id: path,
                parent: page,
                index: 0,
                kind: NodeKind::Path {
                    path: line,
                    corner_radii: Vec::new(),
                    corner_smoothing: 0.0,
                    mirror: Default::default(),
                },
                transform: None,
                name: Some("Line".into()),
            },
            Operation::CreateNode {
                id: leaf,
                parent: page,
                index: 1,
                kind: NodeKind::Ellipse {
                    size: Size::new(60.0, 20.0),
                    arc: Default::default(),
                },
                transform: Some(Affine::translate((100.0, 0.0))),
                name: Some("Leaf".into()),
            },
        ]))
        .expect("build the fixture");
        app.session.adopt_document(doc, None);
        app.canvas_px = (800, 600);
        app.session.selection.set(vec![path, leaf]);
        (app, path, leaf)
    }

    /// With *Bake into paths* on, the card's commit puts a *Brushed · Leaf*
    /// group in the path's place and selects it, keeps the brush, names the step
    /// *Apply brush*, and one undo puts the path back.
    /// flip: `commit_brush` naming the step by the row's label → fails at the step
    /// name.
    #[test]
    fn applying_lands_one_named_step_and_selects_the_group() {
        let ctx = egui::Context::default();
        let (mut app, path, leaf) = path_and_leaf(&ctx);
        app.open_brush();
        app.brush.bake = true;
        assert!(app.brush.open);
        app.refresh_brush_preview();
        assert!(app.session.has_gesture_preview(), "the result previews");
        app.commit_brush();
        let group = app
            .session
            .selection
            .single()
            .expect("the group is selected");
        let page = app.session.doc.first_page();
        assert_eq!(
            app.session.doc.get(page).unwrap().children(),
            &[group, leaf]
        );
        assert_eq!(app.session.doc.get(group).unwrap().name(), "Brushed · Leaf");
        assert!(app.session.doc.get(path).is_none(), "the path goes");
        assert_eq!(app.session.status().text, "Brushed with Leaf");
        let step = app.session.history.done().last().expect("a step");
        assert_eq!(step.note.label, "Apply brush", "the step name");
        app.undo();
        assert_eq!(app.session.doc.get(page).unwrap().children(), &[path, leaf]);
    }

    /// With *Keep the path* on, the path lands at the group's bottom, hidden and
    /// named *Path*; a scatter makes one path per copy and says how many.
    #[test]
    fn a_kept_path_sits_hidden_under_the_copies() {
        let ctx = egui::Context::default();
        let (mut app, path, _) = path_and_leaf(&ctx);
        app.open_brush();
        app.brush.bake = true;
        app.brush.keep_path = true;
        app.brush.scatter = true;
        app.commit_brush();
        let group = app.session.selection.single().expect("the group");
        let kids = app.session.doc.get(group).unwrap().children().to_vec();
        assert_eq!(kids[0], path);
        let kept = app.session.doc.get(path).unwrap();
        assert!(!kept.visible());
        assert_eq!(kept.name(), "Path");
        // 600 along at 72 a step: nine copies.
        assert_eq!(kids.len(), 10);
        assert_eq!(app.session.status().text, "Scattered 9 copies of Leaf");
    }

    /// *Cancel* takes the preview off and leaves the document as it was.
    /// flip: `close_brush` keeping the gesture preview → fails at "nothing shown".
    #[test]
    fn cancel_leaves_everything_as_it_was() {
        let ctx = egui::Context::default();
        let (mut app, path, leaf) = path_and_leaf(&ctx);
        let revision = app.session.revision();
        app.open_brush();
        app.refresh_brush_preview();
        assert!(app.session.has_gesture_preview());
        app.close_brush();
        assert!(!app.session.has_gesture_preview(), "nothing shown");
        assert_eq!(app.session.revision(), revision);
        let page = app.session.doc.first_page();
        assert_eq!(app.session.doc.get(page).unwrap().children(), &[path, leaf]);
    }

    /// A path alone opens the card with no brush: *None*, nothing previewed and
    /// *Apply* refused, until *Pick a shape…* names one; then it links as a pair's
    /// card does. flip: `open_brush` asking `apply_brush::pair` of one layer →
    /// fails at "the card opens".
    #[test]
    fn a_path_alone_opens_the_card_for_a_brush_to_be_picked() {
        let ctx = egui::Context::default();
        let (mut app, path, leaf) = path_and_leaf(&ctx);
        app.session.selection.set_one(path);
        assert_eq!(app.brush_refusal(), None, "one path can take a brush");
        let revision = app.session.revision();
        app.open_brush();
        assert!(
            app.brush.open && app.brush.brush.is_none(),
            "the card opens"
        );
        app.refresh_brush_preview();
        assert!(!app.session.has_gesture_preview(), "nothing drawn yet");
        app.commit_brush();
        assert_eq!(app.session.revision(), revision, "nothing to apply");
        assert_eq!(
            app.session.status().text,
            NO_SHAPE,
            "as the dimmed Apply says"
        );
        app.brush.picking = true;
        app.brush_pick_at(Point::new(130.0, 10.0));
        assert_eq!(app.brush.brush, Some(leaf), "the picked shape");
        app.commit_brush();
        let link = app.session.doc.get(path).and_then(|n| n.brush());
        assert_eq!(link.map(|l| l.of), Some(leaf), "linked");
    }

    /// More than two layers are refused with the row's sentence, and no card opens.
    #[test]
    fn a_selection_without_a_path_is_refused_by_name() {
        let ctx = egui::Context::default();
        let (mut app, path, leaf) = path_and_leaf(&ctx);
        let third = app.session.ids.mint();
        let page = app.session.doc.first_page();
        assert!(app.session.commit(Transaction(vec![Operation::CreateNode {
            id: third,
            parent: page,
            index: 2,
            kind: NodeKind::Rect {
                size: Size::new(10.0, 10.0),
                corner_radii: Default::default(),
                corner_smoothing: 0.0,
            },
            transform: None,
            name: None,
        }])));
        app.session.selection.set(vec![path, leaf, third]);
        assert_eq!(app.brush_refusal(), Some(NEEDS));
        app.open_brush();
        assert!(!app.brush.open);
    }

    /// *Pick a shape…* waits for a click on the canvas: the path is refused by
    /// name and the wait goes on, a shape becomes the brush and the card comes
    /// back; Escape calls a wait off and leaves the card open.
    /// flip: `brush_shape_at` without its path filter → fails at "the path is
    /// refused".
    #[test]
    fn picking_takes_a_shape_but_not_the_path() {
        let ctx = egui::Context::default();
        let (mut app, path, leaf) = path_and_leaf(&ctx);
        let other = app.session.ids.mint();
        let page = app.session.doc.first_page();
        assert!(app.session.commit(Transaction(vec![Operation::CreateNode {
            id: other,
            parent: page,
            index: 2,
            kind: NodeKind::Rect {
                size: Size::new(40.0, 40.0),
                corner_radii: Default::default(),
                corner_smoothing: 0.0,
            },
            transform: Some(Affine::translate((400.0, 0.0))),
            name: None,
        }])));
        app.session.selection.set(vec![path, leaf]);
        app.open_brush();
        app.brush.picking = true;
        app.brush_pick_at(Point::new(300.0, 300.0));
        assert!(
            app.brush_picking() && app.brush.brush == Some(leaf),
            "the path is refused"
        );
        assert_eq!(app.session.status().text, Refusal::Same.says());
        app.brush_pick_at(Point::new(420.0, 20.0));
        assert!(!app.brush_picking(), "back to the card");
        assert_eq!(app.brush.brush, Some(other));

        app.brush.picking = true;
        app.escape(&ctx);
        assert!(
            !app.brush_picking() && app.brush.open,
            "the wait called off, card open"
        );
        app.escape(&ctx);
        assert!(!app.brush.open, "the next press closes the card");
    }

    /// While *Pick a shape…* waits, the status line keeps saying how to get out
    /// once another message replaces it, but leaves a refused pick's sentence up.
    /// flip: `keep_brush_pick_status` re-saying over the refusal → fails at "the
    /// refusal stays".
    #[test]
    fn the_pick_wait_keeps_its_status() {
        let ctx = egui::Context::default();
        let (mut app, _, _) = path_and_leaf(&ctx);
        app.open_brush();
        app.brush.picking = true;
        app.session.info("Something else");
        let _ = ctx.run_ui(Default::default(), |ui| app.brush_ui(ui.ctx()));
        assert_eq!(app.session.status().text, PICK_STATUS, "said again");
        app.session.info(Refusal::Same.says());
        let _ = ctx.run_ui(Default::default(), |ui| app.brush_ui(ui.ctx()));
        assert_eq!(
            app.session.status().text,
            Refusal::Same.says(),
            "the refusal stays"
        );
    }

    /// *Swap brush and path* trades the two when the swapped pair fits.
    #[test]
    fn swapping_trades_brush_and_path() {
        let ctx = egui::Context::default();
        let (mut app, path, leaf) = path_and_leaf(&ctx);
        app.open_brush();
        app.swap_brush();
        assert_eq!((app.brush.path, app.brush.brush), (Some(leaf), Some(path)));
    }

    /// The card is as tall in Art as in Scatter, so the footer stays put.
    /// flip: `brush_ui` without the body's `set_min_height` → fails at the height.
    #[test]
    fn the_card_keeps_its_height_in_either_mode() {
        let ctx = egui::Context::default();
        let (mut app, _, _) = path_and_leaf(&ctx);
        let id = egui::Id::new("apply-brush");
        crate::repeat::tests::pass(&ctx, &mut app, Vec::new());
        app.open_brush();
        let height = |app: &mut MastikApp| {
            for _ in 0..3 {
                crate::repeat::tests::pass(&ctx, app, Vec::new());
            }
            ctx.memory(|m| m.area_rect(id)).expect("the card").height()
        };
        let art = height(&mut app);
        app.brush.scatter = true;
        let scatter = height(&mut app);
        assert_eq!(art, scatter);
    }

    /// A right-click mid-scrub of *Width* puts it back, and the card opens level
    /// with the Transform card.
    /// flip: `cancel_gesture` not restoring `brush_scrub` → fails at "the width is
    /// back".
    #[test]
    fn a_right_click_winds_a_width_scrub_back() {
        use crate::repeat::tests::scrub_then_right_click;
        let id = egui::Id::new("apply-brush");
        let ctx = egui::Context::default();
        let (mut app, _, _) = path_and_leaf(&ctx);
        crate::repeat::tests::pass(&ctx, &mut app, Vec::new());
        app.open_brush();
        let before = app.brush.numbers;
        scrub_then_right_click(
            &ctx,
            &mut app,
            id,
            "Width",
            |at| at,
            |app| {
                assert_ne!(app.brush.numbers, before, "the drag scrubbed the width");
                assert!(app.in_flight.brush_scrub.is_some(), "the scrub is armed");
            },
        );
        assert_eq!(app.brush.numbers, before, "the width is back");
        assert!(app.brush.open, "the right-click closed nothing");
        let card = ctx.memory(|m| m.area_rect(id)).expect("the card");
        let transform = app.transform_card.expect("the Transform card drawn");
        assert_eq!(card.top(), transform.top(), "level with the Transform card");
    }

    /// The preview spends no ids.
    /// flip: `refresh_brush_preview` minting from `session.ids` → fails at "no ids
    /// spent".
    #[test]
    fn the_preview_spends_no_ids() {
        let ctx = egui::Context::default();
        let (mut app, _, _) = path_and_leaf(&ctx);
        app.open_brush();
        app.brush.bake = true;
        let before = app.session.ids.next_seq();
        app.refresh_brush_preview();
        assert!(app.session.has_gesture_preview());
        assert_eq!(app.session.ids.next_seq(), before, "no ids spent");
    }

    /// By default the card links the brush: the path stays the layer, selected,
    /// in its place, drawn with the brush; the step is *Apply brush*, and the
    /// preview was the link itself.
    /// flip: `BrushCard::live` answering `false` for a fresh card → fails at
    /// "the preview draws the brush".
    #[test]
    fn applying_links_a_live_brush_by_default() {
        let ctx = egui::Context::default();
        let (mut app, path, leaf) = path_and_leaf(&ctx);
        app.open_brush();
        app.refresh_brush_preview();
        let previewed = app
            .session
            .render_inputs()
            .2
            .brush_of(path)
            .map(|b| b.is_some());
        assert_eq!(previewed, Some(true), "the preview draws the brush");
        app.commit_brush();
        let page = app.session.doc.first_page();
        assert_eq!(
            app.session.doc.get(page).unwrap().children(),
            &[path, leaf],
            "the path stays"
        );
        let link = app.session.doc.get(path).and_then(|n| n.brush());
        assert_eq!(link.map(|l| l.of), Some(leaf));
        assert_eq!(app.session.selection.single(), Some(path));
        assert_eq!(app.session.status().text, "Brushed Line with Leaf");
        let step = app.session.history.done().last().expect("a step");
        assert_eq!(step.note.label, STEP);
        assert!(app.session.resolved.brush_ink(path).is_some());
    }

    /// *Edit brush* opens on the layer's own options, *Done* writes them as an
    /// *Edit brush* step, and *Remove brush* drops the link as its own step.
    #[test]
    fn edit_brush_writes_the_options_and_remove_drops_them() {
        let ctx = egui::Context::default();
        let (mut app, path, leaf) = path_and_leaf(&ctx);
        app.open_brush();
        app.brush.scatter = true;
        app.brush.numbers.seed = 5.0;
        app.commit_brush();
        app.brush.scatter = false;
        app.brush.numbers.seed = 1.0;
        app.open_edit_brush();
        assert!(app.brush.open && !app.brush.editing.is_empty());
        assert!(app.brush.scatter, "the layer's mode, not the card's last");
        assert_eq!(app.brush.numbers.seed, 5.0, "the layer's seed");
        app.brush.numbers.seed = 9.0;
        app.commit_brush();
        let link = app.session.doc.get(path).and_then(|n| n.brush()).unwrap();
        assert!(matches!(link.mode, Mode::Scatter(s) if s.seed == 9));
        assert_eq!(link.of, leaf);
        let step = app.session.history.done().last().expect("a step");
        assert_eq!(step.note.label, EDIT_STEP);
        app.session.selection.set_one(path);
        app.remove_brushes();
        assert!(app.session.doc.get(path).and_then(|n| n.brush()).is_none());
        let step = app.session.history.done().last().expect("a step");
        assert_eq!(step.note.label, REMOVE_STEP);
    }

    /// [`path_and_leaf`] with a second path, *Other*, drawn with a brush *Dot*:
    /// the path linked to *Leaf* with `a`, *Other* to *Dot* with `b`, and *Edit
    /// brush* opened on the two. Answers the app, the paths and the brushes.
    fn two_brushed(ctx: &egui::Context, a: Mode, b: Mode) -> (MastikApp, [NodeId; 4]) {
        let (mut app, path, leaf) = path_and_leaf(ctx);
        let (other, dot) = (app.session.ids.mint(), app.session.ids.mint());
        let page = app.session.doc.first_page();
        let mut line = BezPath::new();
        line.move_to((0.0, 400.0));
        line.line_to((600.0, 400.0));
        assert!(app.session.commit(Transaction(vec![
            Operation::CreateNode {
                id: other,
                parent: page,
                index: 2,
                kind: NodeKind::Path {
                    path: line,
                    corner_radii: Vec::new(),
                    corner_smoothing: 0.0,
                    mirror: Default::default(),
                },
                transform: None,
                name: Some("Other".into()),
            },
            Operation::CreateNode {
                id: dot,
                parent: page,
                index: 3,
                kind: NodeKind::Ellipse {
                    size: Size::new(20.0, 20.0),
                    arc: Default::default(),
                },
                transform: None,
                name: Some("Dot".into()),
            },
            Operation::SetBrush {
                id: path,
                brush: Some(BrushLink::new(leaf, a)),
            },
            Operation::SetBrush {
                id: other,
                brush: Some(BrushLink::new(dot, b)),
            },
        ])));
        app.session.selection.set(vec![path, other]);
        app.open_edit_brush();
        (app, [path, other, leaf, dot])
    }

    fn link_of(app: &MastikApp, id: NodeId) -> BrushLink {
        app.session.doc.get(id).and_then(|n| n.brush()).unwrap()
    }

    /// *Edit brush* over two layers drawn with different brushes writes the
    /// card's options to both and leaves each its own brush.
    /// flip: `card_links` writing the card's brush to every edited layer →
    /// fails at "each keeps its own brush".
    #[test]
    fn edit_brush_over_two_layers_keeps_each_ones_brush() {
        let ctx = egui::Context::default();
        let art = Mode::Art(Art::DEFAULT);
        let (mut app, [path, other, leaf, dot]) = two_brushed(&ctx, art, art);
        assert!(app.brush.mixed.brush, "the brushes differ");
        app.brush.flip = true;
        app.commit_brush();
        assert_eq!(
            (link_of(&app, path).of, link_of(&app, other).of),
            (leaf, dot),
            "each keeps its own brush"
        );
        assert!(
            matches!(link_of(&app, other).mode, Mode::Art(a) if a.flip),
            "the options"
        );
    }

    /// *Edit brush* over two Scatter brushes that differ in their seed shows
    /// the seed *Mixed* and the spacing they share as it is; the spacing set,
    /// *Done* writes it to both and leaves each its own seed.
    /// flip: `mode_for` ignoring the mixed flags → fails at "each keeps its
    /// own seed".
    #[test]
    fn edit_brush_keeps_each_layers_mixed_options() {
        let ctx = egui::Context::default();
        let seeded = |seed| {
            Mode::Scatter(Scatter {
                seed,
                ..Scatter::DEFAULT
            })
        };
        let (mut app, [path, other, ..]) = two_brushed(&ctx, seeded(3), seeded(8));
        let x = app.brush.mixed;
        assert!(x.seed && !x.spacing && !x.kind, "{x:?}");
        let before = app.brush.numbers;
        app.brush.numbers.spacing = 300.0;
        app.brush.mixed.settle(before, app.brush.numbers);
        app.commit_brush();
        let scatter_of = |id| match link_of(&app, id).mode {
            Mode::Scatter(s) => (s.spacing, s.seed),
            Mode::Art(_) => panic!("still a scatter"),
        };
        assert_eq!(
            (scatter_of(path), scatter_of(other)),
            ((3.0, 3), (3.0, 8)),
            "each keeps its own seed"
        );
    }

    /// Art and Scatter brushes together: the kind is mixed and *Done* leaves
    /// each layer's whole mode alone; picking Scatter sets both to it.
    /// flip: `mode_for` ignoring the mixed flags → fails at "each keeps its own
    /// kind".
    #[test]
    fn edit_brush_over_two_kinds_keeps_each_ones_until_one_is_picked() {
        let ctx = egui::Context::default();
        let art = Mode::Art(Art {
            width: 2.0,
            ..Art::DEFAULT
        });
        let scatter = Mode::Scatter(Scatter::DEFAULT);
        let (mut app, [path, other, ..]) = two_brushed(&ctx, art, scatter);
        assert!(app.brush.mixed.kind);
        let links = app.card_links();
        let modes: Vec<_> = links
            .iter()
            .map(|op| match op {
                Operation::SetBrush { brush: Some(l), .. } => l.mode,
                _ => unreachable!("the card writes links"),
            })
            .collect();
        assert_eq!(modes, vec![art, scatter], "each keeps its own kind");
        app.brush.scatter = true;
        app.brush.mixed.kind = false;
        app.commit_brush();
        assert!(
            [path, other]
                .iter()
                .all(|id| matches!(link_of(&app, *id).mode, Mode::Scatter(_))),
            "both scatter"
        );
    }

    /// Deleting the brush bakes the layer drawn with it in the same step, and
    /// says so; one undo brings both back, linked.
    /// flip: `commit_inner` without `live_brush::baked_for_deletes` → fails at
    /// "the brushed path is baked".
    #[test]
    fn deleting_the_brush_bakes_its_user_in_one_step() {
        let ctx = egui::Context::default();
        let (mut app, path, leaf) = path_and_leaf(&ctx);
        app.open_brush();
        app.commit_brush();
        app.session.selection.set_one(leaf);
        let steps = app.session.history.done().len();
        app.delete_selection();
        assert_eq!(app.session.history.done().len(), steps + 1, "one step");
        assert!(app.session.doc.get(leaf).is_none());
        let page = app.session.doc.first_page();
        let top = app.session.doc.get(page).unwrap().children().to_vec();
        assert_eq!(top.len(), 1);
        assert_eq!(
            app.session.doc.get(top[0]).unwrap().name(),
            "Brushed · Leaf",
            "the brushed path is baked"
        );
        assert_eq!(
            app.session.status().text,
            "Deleted Leaf · baked the 1 layer it brushed"
        );
        app.undo();
        assert_eq!(
            app.session
                .doc
                .get(path)
                .and_then(|n| n.brush())
                .map(|l| l.of),
            Some(leaf)
        );
    }
}
