//! The player: the played copy, its play state, and the door it changes the copy
//! through.

use crate::anim::Anims;
use crate::input::{Press, Tap, While};
use crate::take::Recording;
use mastik_core::kurbo::{Point, Rect, Size, Vec2};
use mastik_core::prototype::{self, FlowRef, OverlaySettings, Transition, Trigger};
use mastik_core::variable::{Cell, ModeId, Value, VarId, VarKind};
use mastik_core::{
    Document, IdSource, ItemId, NodeId, NodeKind, OpError, Resolved, Transaction,
    reserve_existing_ids, subtree_nodes,
};
use rustc_hash::{FxHashMap, FxHashSet};
use std::collections::BTreeMap;

/// Where play starts: a page's flow, or a frame with no flow.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Start {
    Flow(FlowRef),
    Frame(NodeId),
}

/// Why play cannot start.
#[derive(Clone, Debug, PartialEq, Eq, thiserror::Error)]
pub enum StartError {
    #[error("the flow is not there")]
    NoFlow,
    #[error("the start frame is not there")]
    NoFrame,
}

/// Something the shell should say once, in its own corner.
#[derive(Clone, Debug, PartialEq)]
pub enum Notice {
    /// An action names a frame, layer, variable or script that is not there; it
    /// is skipped. Said once per action.
    Missing { layer: NodeId, action: ItemId },
    /// An expression does not read or evaluate; its action is skipped. Said once
    /// per action.
    Expression {
        layer: NodeId,
        action: ItemId,
        error: prototype::ExprError,
    },
    /// A script failed and what it did was rolled back.
    Script { name: String, message: String },
    /// A script asks for input, which an action cannot give; it was not run.
    ScriptNeedsInput { name: String },
    /// This player was built without scripts; *Run script* is skipped.
    ScriptsOff,
    /// An edit removed what play was showing or holding, and play fell back.
    Fallback(Fallback),
    /// Reactions set each other off past the depth the player follows.
    Loop,
    /// A scroll trigger watches a frame that is gone, so it never fires. Said
    /// once per reaction, the first time its screen scrolls.
    ScrollFrameGone { layer: NodeId },
}

/// What an edit took away from under play.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Fallback {
    /// The current frame: play went to the flow's start.
    Screen,
    /// An open overlay: it closed.
    Overlay,
    /// A variable: its play value went.
    Variable,
}

/// Something the engine cannot do itself and asks of the shell.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Request {
    /// Open an `http`/`https` address in the browser.
    OpenUrl(String),
}

/// One row of the state card: a variable and what it holds now.
#[derive(Clone, Debug, PartialEq)]
pub struct ValueRow {
    pub var: VarId,
    pub name: String,
    pub kind: VarKind,
    /// Its play value, else its value in the mode in force at the screen frame.
    pub value: Value,
    /// Whether play set it.
    pub played: bool,
    /// The kit it comes from, `None` for the document's own.
    pub kit: Option<String>,
}

/// One row of the state card's modes: a collection of two modes or more and
/// the mode play reads on the screen shown.
#[derive(Clone, Debug, PartialEq)]
pub struct ModeRow {
    pub name: String,
    /// The kit it comes from, by id; `None` for the document's own.
    pub kit: Option<String>,
    /// Its modes in order, by id and name.
    pub modes: Vec<(mastik_core::variable::ModeId, String)>,
    pub current: mastik_core::variable::ModeId,
    /// Whether the screen sets a mode of it itself, which a pick for every
    /// page does not reach.
    pub screen_sets: bool,
}

/// One data set in the state card's *Opened rows*.
#[derive(Clone, Debug, PartialEq)]
pub struct OpenedRow {
    pub data: mastik_core::list::DataSetId,
    pub name: String,
    /// Each row by its first cell.
    pub rows: Vec<String>,
    /// The row open now: 0 until a tap or a pick opens another.
    pub current: usize,
}

/// Why a value from the state card was not taken.
#[derive(Clone, Debug, PartialEq, Eq, thiserror::Error)]
pub enum ValueError {
    #[error("the variable is not there")]
    NoVariable,
    #[error("the value is not of the variable's kind")]
    WrongKind,
}

/// An overlay open over the screen.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct Overlay {
    pub(crate) frame: NodeId,
    pub(crate) settings: OverlaySettings,
    /// The top-left of the layer that opened it, in screen space as it was
    /// drawn then: where a manual position is measured from.
    pub(crate) anchor: Point,
    /// The transition it opened with, played in reverse when *Back* closes it.
    pub(crate) transition: Transition,
}

/// A frame *Back* returns to, and the transition that left it.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct Visit {
    pub(crate) frame: NodeId,
    pub(crate) transition: Transition,
    /// Whether the navigation that left it kept scroll, so *Back* does too.
    pub(crate) keep: bool,
}

/// An `After` reaction waiting for its time.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct Timer {
    pub(crate) due: f64,
    pub(crate) holder: NodeId,
    pub(crate) reaction: ItemId,
    /// The shown frame that holds it; the timer goes when the frame closes.
    pub(crate) frame: NodeId,
}

/// How deep reactions may set each other off (a variable change firing a
/// reaction that changes a variable…) before the player stops following.
pub(crate) const MAX_DEPTH: u32 = 16;

/// The shortest wait of an *After* started by a change of state, in
/// milliseconds: two states each changing to the other after nothing would
/// otherwise fire forever without time moving.
const MIN_LOOP_MS: f64 = 16.0;

/// How many timers one tick fires at most. A tick that spans many turns of a
/// loop (a hidden player catching up) stops there; the loop goes on from now.
const MAX_FIRES: usize = 1024;

/// A prototype being played: a copy of the document and everything that exists
/// only while it plays.
///
/// The copy changes only through [`Player::commit`] — `Document::apply` after
/// `propagate::owed`, as the editor's commit door — with no history; ids it
/// mints come from the player's own actor. Time comes in through
/// [`Player::tick`] alone, so a run is a function of its inputs and their times.
pub struct Player {
    /// The document as the editor holds it: every edit the shell passes lands
    /// here too, so a refused edit can re-clone from it.
    pub(crate) base: Document,
    pub(crate) doc: Document,
    pub(crate) res: Resolved,
    pub(crate) ids: IdSource,
    pub(crate) start: Start,
    pub(crate) page: NodeId,
    pub(crate) screen: NodeId,
    pub(crate) history: Vec<Visit>,
    pub(crate) overlays: Vec<Overlay>,
    pub(crate) values: BTreeMap<VarId, Value>,
    /// Scroll offsets by scrolling frame, each positive into the content.
    pub(crate) scroll: FxHashMap<NodeId, Vec2>,
    /// How far each layer that follows the drag has been moved from where it
    /// is drawn: play state, kept for the run like a scroll offset.
    pub(crate) moved: FxHashMap<NodeId, Vec2>,
    /// Layers whose settle finished, waiting for their *Drop* reactions.
    pub(crate) drops: Vec<(NodeId, prototype::DropAt)>,
    /// The offsets as the scroll triggers last saw them, to tell a crossing.
    pub(crate) scroll_seen: FxHashMap<NodeId, Vec2>,
    /// The variant main each instance play switched was switched to, carried
    /// across a re-clone.
    pub(crate) states: FxHashMap<NodeId, NodeId>,
    /// The modes *Set mode* gave each page or frame, carried across a re-clone.
    pub(crate) modes: FxHashMap<NodeId, Vec<mastik_core::variable::ModeId>>,
    /// The row play opened in each data set, opened again after a live edit:
    /// the edit's own regeneration, or a re-clone, puts every list back on row 1.
    pub(crate) opened: BTreeMap<mastik_core::list::DataSetId, usize>,
    /// The size each screen is laid out at in place of its own (a page's
    /// device, or the room under *Responsive*); `None` as drawn.
    pub(crate) play_size: Option<Size>,
    /// Instances showing an interaction state (hover, pressed) by themselves,
    /// with the main to return to.
    pub(crate) auto: FxHashMap<NodeId, NodeId>,
    /// The layers bound as inputs.
    pub(crate) inputs: Vec<(NodeId, prototype::Input)>,
    /// The layers with a scroll effect, found as play starts and after a live
    /// edit, so a frame's scene need not look for them.
    pub(crate) effects: Vec<NodeId>,
    pub(crate) pointer: Option<Point>,
    /// The layers under the pointer, deepest first.
    pub(crate) hovered: Vec<NodeId>,
    /// The pointer is a finger (`Input::Touch`), which hovers nothing.
    pub(crate) touch: bool,
    pub(crate) press: Option<Press>,
    /// A click waiting to see whether it becomes a double click.
    pub(crate) tap: Option<Tap>,
    /// The input holding focus: a text being typed into, or an input instance
    /// Tab reached. Set through `Player::set_focus` alone.
    pub(crate) focus: Option<NodeId>,
    /// The instance showing its *Focus* state for the focused input.
    pub(crate) focus_shown: Option<NodeId>,
    pub(crate) whiles: Vec<While>,
    pub(crate) timers: Vec<Timer>,
    /// Milliseconds since play started.
    pub(crate) now: f64,
    pub(crate) anims: Anims,
    pub(crate) notices: Vec<Notice>,
    pub(crate) noticed: FxHashSet<(NodeId, ItemId)>,
    pub(crate) log: Vec<String>,
    pub(crate) requests: Vec<Request>,
    pub(crate) dirty: FxHashSet<NodeId>,
    pub(crate) recording: Option<Recording>,
    /// When a click on nothing last flashed the hints.
    pub(crate) flash: Option<f64>,
    pub(crate) depth: u32,
}

impl Player {
    /// Start playing `doc` from `start`, minting any id from `actor`, which must
    /// be the player's own.
    pub fn new(doc: &Document, start: Start, actor: u64) -> Result<Player, StartError> {
        let (page, frame, modes, cells) = resolve_start(doc, start)?;
        let mut ids = IdSource::new(actor);
        reserve_existing_ids(doc, &mut ids);
        let mut copy = doc.clone();
        let res = Resolved::settled(&mut copy);
        let mut p = Player {
            base: doc.clone(),
            doc: copy,
            res,
            ids,
            start,
            page,
            screen: frame,
            history: Vec::new(),
            overlays: Vec::new(),
            values: BTreeMap::new(),
            scroll: FxHashMap::default(),
            scroll_seen: FxHashMap::default(),
            moved: FxHashMap::default(),
            drops: Vec::new(),
            states: FxHashMap::default(),
            modes: FxHashMap::default(),
            opened: BTreeMap::new(),
            play_size: None,
            auto: FxHashMap::default(),
            inputs: Vec::new(),
            effects: Vec::new(),
            pointer: None,
            hovered: Vec::new(),
            touch: false,
            press: None,
            tap: None,
            focus: None,
            focus_shown: None,
            whiles: Vec::new(),
            timers: Vec::new(),
            now: 0.0,
            anims: Anims::default(),
            notices: Vec::new(),
            noticed: FxHashSet::default(),
            log: Vec::new(),
            requests: Vec::new(),
            dirty: FxHashSet::default(),
            recording: None,
            flash: None,
            depth: 0,
        };
        p.prepare();
        // A flow's starting modes are set as *Set mode* sets them everywhere.
        for mode in modes {
            for id in p.everywhere(mode) {
                p.set_mode_on(id, mode);
            }
        }
        for (var, cell) in cells {
            let value = match cell {
                Cell::Alias(to) => p.doc.resolve_var(frame, to),
                Cell::Color(c) => Some(Value::Color(c)),
                Cell::Number(n) => Some(Value::Number(n)),
                Cell::Boolean(b) => Some(Value::Boolean(b)),
                Cell::String(s) => Some(Value::String(s)),
            };
            if let Some(v) = value {
                p.values.insert(var, v);
            }
        }
        p.apply_values();
        p.sync_all_inputs();
        p.on_open(frame);
        Ok(p)
    }

    /// Play again from the start, on a fresh copy of the document as it now is.
    /// A recording in progress stops.
    pub fn restart(&mut self) {
        let actor = self.ids.actor();
        let (size, touch) = (self.play_size, self.touch);
        if let Ok(fresh) = Player::new(&self.base, self.start, actor) {
            *self = fresh;
            self.set_play_size(size);
            // The pointer is the same hand after a restart.
            self.touch = touch;
        }
    }

    /// Lay each screen out at `size` from now on, in place of its own, through
    /// its frame's layout and width queries (a page's device, or the room
    /// under *Responsive*);
    /// `None` puts it back as drawn. The copy alone changes, never the
    /// document. Answers whether anything changed.
    pub fn set_play_size(&mut self, size: Option<Size>) -> bool {
        let size = size.filter(|s| {
            s.width.is_finite() && s.height.is_finite() && s.width >= 1.0 && s.height >= 1.0
        });
        if size == self.play_size {
            return false;
        }
        self.play_size = size;
        self.fit_screen(self.screen);
        self.rehover();
        true
    }

    /// The size each screen is laid out at, `None` as drawn.
    pub fn play_size(&self) -> Option<Size> {
        self.play_size
    }

    /// Give frame `id` of the copy the play size, or its own size back.
    pub(crate) fn fit_screen(&mut self, id: NodeId) {
        let own = self.base.get(id).and_then(|n| n.kind().box_size());
        let Some(want) = self.play_size.or(own) else {
            return;
        };
        let now = self.doc.get(id).and_then(|n| n.kind().box_size());
        if now.is_none() || now == Some(want) {
            return;
        }
        let tx = Transaction(vec![mastik_core::Operation::SetGeometry {
            id,
            geometry: mastik_core::op::GeometryPatch::Size(want),
        }]);
        let _ = self.commit(tx);
    }

    /// What play has made of the document — the copy to draw.
    pub fn document(&self) -> &Document {
        &self.doc
    }

    /// The copy's derived state, to draw with.
    pub fn resolved(&self) -> &Resolved {
        &self.res
    }

    /// Where play started.
    pub fn start(&self) -> Start {
        self.start
    }

    /// The frame on screen.
    pub fn screen(&self) -> NodeId {
        self.screen
    }

    /// The page the screen frame is on.
    pub fn page(&self) -> NodeId {
        self.page
    }

    /// The open overlays, bottom first.
    pub fn overlays(&self) -> Vec<NodeId> {
        self.overlays.iter().map(|o| o.frame).collect()
    }

    /// The frames *Back* walks, oldest first.
    pub fn history(&self) -> Vec<NodeId> {
        self.history.iter().map(|v| v.frame).collect()
    }

    /// A scrolling frame's offset into its content.
    pub fn scroll_offset(&self, frame: NodeId) -> Vec2 {
        self.scroll.get(&frame).copied().unwrap_or(Vec2::ZERO)
    }

    /// Milliseconds since play started.
    pub fn now(&self) -> f64 {
        self.now
    }

    /// The input holding focus, if any: a text being typed into, while plain
    /// keys are text, or an input instance, which Space and Enter flip. Escape
    /// leaves either; Tab and Shift+Tab move between the inputs.
    pub fn focused(&self) -> Option<NodeId> {
        self.focus
    }

    /// Whether anything will change without input: a transition, a timer, the
    /// hints' flash. A shell may stop ticking while this is false.
    pub fn is_moving(&self) -> bool {
        !self.anims.is_empty()
            || !self.timers.is_empty()
            || self.flash.is_some()
            || self.gesture_due().is_some()
    }

    /// Whether the document holds a *Run script* action anywhere, so the shell
    /// can show the scripts disclosure when play starts.
    pub fn has_scripts(&self) -> bool {
        self.doc.node_ids().any(|id| {
            self.doc.get(id).is_some_and(|n| {
                n.reactions().iter().any(|r| {
                    let mut any = false;
                    each_action(&r.value.actions, &mut |a| {
                        any |= matches!(a, prototype::Action::RunScript { .. })
                    });
                    any
                })
            })
        })
    }

    /// The notices raised since last asked, oldest first.
    pub fn take_notices(&mut self) -> Vec<Notice> {
        std::mem::take(&mut self.notices)
    }

    /// What scripts printed and how they failed, oldest first.
    pub fn log(&self) -> &[String] {
        &self.log
    }

    /// The requests made since last asked.
    pub fn take_requests(&mut self) -> Vec<Request> {
        std::mem::take(&mut self.requests)
    }

    /// Every node of the copy whose drawing changed since last asked: what the
    /// shell's renderer must redraw. A transition's patches are not in it.
    pub fn take_dirty(&mut self) -> Vec<NodeId> {
        self.dirty.drain().collect()
    }

    /// Advance play's clock by `ms`: due timers and gestures (a held click, a
    /// long press) fire at their own times, and transitions move on.
    pub fn tick(&mut self, ms: f64) {
        let until = self.now + ms.max(0.0);
        for _ in 0..MAX_FIRES {
            let next = self
                .timers
                .iter()
                .enumerate()
                .filter(|(_, t)| t.due <= until)
                .min_by(|a, b| a.1.due.total_cmp(&b.1.due))
                .map(|(i, t)| (i, t.due));
            let gesture = self.gesture_due().filter(|d| *d <= until);
            match (next, gesture) {
                (None, None) => break,
                (Some((_, t)), Some(g)) if g <= t => {
                    self.now = self.now.max(g);
                    self.advance_anims();
                    self.fire_gestures();
                }
                (None, Some(g)) => {
                    self.now = self.now.max(g);
                    self.advance_anims();
                    self.fire_gestures();
                }
                (Some((i, _)), _) => {
                    let timer = self.timers.remove(i);
                    self.now = self.now.max(timer.due);
                    self.advance_anims();
                    self.fire_timer(timer);
                }
            }
            self.fire_drops();
            self.check_scrolls();
        }
        self.now = until;
        self.advance_anims();
        self.fire_drops();
        self.check_scrolls();
        if self
            .flash
            .is_some_and(|f| self.now - f >= crate::anim::FLASH_MS)
        {
            self.flash = None;
        }
    }

    /// The state card's rows: every variable, its current value, and whether
    /// play set it.
    pub fn values(&self) -> Vec<ValueRow> {
        self.doc
            .variables()
            .iter()
            .filter_map(|v| {
                Some(ValueRow {
                    var: v.id,
                    name: v.name.clone(),
                    kind: v.kind,
                    value: self.doc.resolve_var(self.screen, v.id)?,
                    played: self.values.contains_key(&v.id),
                    kit: v.kit().map(str::to_owned),
                })
            })
            .collect()
    }

    /// The state card's modes: each collection with two modes or more, the
    /// document's then the kits', and the mode it reads on the screen shown.
    pub fn modes(&self) -> Vec<ModeRow> {
        let t = self.doc.variables();
        let screen_modes = self.doc.get(self.screen).map(|n| n.modes().to_vec());
        t.switchable()
            .into_iter()
            .filter_map(|s| {
                Some(ModeRow {
                    name: s.name.clone(),
                    kit: s.owner.kit().map(str::to_owned),
                    modes: s.modes.iter().map(|(m, n)| (*m, n.to_string())).collect(),
                    current: self.doc.mode_of_at(self.screen, s.owner)?,
                    screen_sets: screen_modes
                        .as_ref()
                        .is_some_and(|ms| ms.iter().any(|m| t.owner_of(*m) == Some(s.owner))),
                })
            })
            .collect()
    }

    /// The collection picked by width and the mode the screen's play width
    /// picks in it, by name, while that differs from the mode its drawn width
    /// picks: why a resized or turned screen changed. `None` when the screen
    /// does not pick by width (a pin, a component, no such collection).
    pub fn width_pick(&self) -> Option<(String, String)> {
        let picked = self.doc.width_pick(self.screen)?;
        if self.base.width_mode(self.screen) == Some(picked) {
            return None;
        }
        let c = self.doc.variables().by_width()?;
        let mode = c.modes.iter().find(|m| m.id == picked)?;
        Some((c.name.clone(), mode.name.clone()))
    }

    /// The state card's *Opened rows*: one per data set an *Opened row* list
    /// reads, in the document's order.
    pub fn opened_rows(&self) -> Vec<OpenedRow> {
        use mastik_core::list::{self, Rows};
        let lists: Vec<list::List> = list::lists(&self.doc)
            .into_iter()
            .filter_map(|l| self.doc.get(l)?.list().cloned())
            .filter(|l| l.rows == Rows::Opened)
            .collect();
        self.doc
            .data_sets()
            .iter()
            .filter_map(|d| {
                let l = lists.iter().find(|l| l.data == d.id)?;
                // The row by its first column, the cell a person reads it by.
                let rows = d
                    .table
                    .rows
                    .iter()
                    .map(|r| r.first().map(|c| c.trim().to_string()).unwrap_or_default())
                    .collect();
                Some(OpenedRow {
                    data: d.id,
                    name: d.name.clone(),
                    rows,
                    current: l.opened.min(d.table.rows.len().saturating_sub(1)),
                })
            })
            .collect()
    }

    /// Set a variable's play value from the state card. Reactions to the change
    /// fire, and a take being recorded records it.
    pub fn set_value(&mut self, var: VarId, value: Value) -> Result<(), ValueError> {
        let Some(kind) = self.doc.variables().kind_of(var) else {
            return Err(ValueError::NoVariable);
        };
        if kind_of(&value) != kind {
            return Err(ValueError::WrongKind);
        }
        self.input(crate::Input::SetValue { var, value });
        Ok(())
    }

    // ── The play door ────────────────────────────────────────────────────────

    /// Change the copy: what the components owe `tx` appended, then applied, and
    /// the derived state updated. No history, no autosave.
    pub(crate) fn commit(&mut self, mut tx: Transaction) -> Result<(), OpError> {
        mastik_core::propagate::owed(&self.doc, &mut tx, &mut self.ids);
        let out = self.doc.apply(&tx)?;
        self.res.update(&self.doc, &out.dirty);
        let settled = self.res.settle(&mut self.doc);
        self.dirty.extend(out.dirty.0);
        self.dirty.extend(settled.0);
        Ok(())
    }

    /// A tap inside a list's row opens that row of its data set: every list
    /// over the same data set showing *Opened row* is made again on the copy to
    /// show it (`mastik_core::list`). Nothing for a layer in no list row.
    pub(crate) fn open_row(&mut self, id: NodeId) {
        use mastik_core::list;
        let Some((at, _, row)) = list::row_at(&self.doc, id) else {
            return;
        };
        let Some(data) = self.doc.get(at).and_then(|n| n.list()).map(|l| l.data) else {
            return;
        };
        self.open_data_row(data, row);
    }

    /// Row `row` of data set `data` opened: every list over it showing *Opened
    /// row* made again on the copy to show it.
    pub(crate) fn open_data_row(&mut self, data: mastik_core::list::DataSetId, row: usize) {
        use mastik_core::list::{self, Rows};
        self.opened.insert(data, row);
        let targets: Vec<(NodeId, list::List)> = list::lists(&self.doc)
            .into_iter()
            .filter_map(|l| {
                let x = self.doc.get(l)?.list()?;
                (x.data == data && x.rows == Rows::Opened && x.opened != row)
                    .then(|| (l, x.clone()))
            })
            .collect();
        for (l, x) in targets {
            let tx = Transaction(vec![mastik_core::Operation::SetList {
                id: l,
                list: Some(Box::new(list::List { opened: row, ..x })),
            }]);
            if self.commit(tx).is_err() {
                continue;
            }
            let (doc, res, ids, dirty) =
                (&mut self.doc, &mut self.res, &mut self.ids, &mut self.dirty);
            // A regeneration the rules refuse leaves the list showing its last row.
            let _ = list::regenerate(doc, l, ids, |doc, tx| {
                let out = doc.apply(&tx)?;
                res.update(doc, &out.dirty);
                dirty.extend(out.dirty.0);
                Ok::<(), OpError>(())
            });
            let settled = self.res.settle(&mut self.doc);
            self.dirty.extend(settled.0);
        }
    }

    /// Each row play opened, opened again on the copy; a data set the copy no
    /// longer has is forgotten.
    pub(crate) fn reopen_rows(&mut self) {
        self.opened.retain(|d, _| self.doc.data_set(*d).is_some());
        let opened: Vec<_> = self.opened.iter().map(|(d, r)| (*d, *r)).collect();
        for (data, row) in opened {
            self.open_data_row(data, row);
        }
    }

    /// Put the play values into the copy's table and lay out what reads them.
    pub(crate) fn apply_values(&mut self) {
        let dirty = self.doc.set_play_values(self.values.clone());
        self.res.update(&self.doc, &dirty);
        self.dirty.extend(dirty.0);
    }

    /// What the copy needs before play: the inputs found, and each input text
    /// layer's content bound to its variable so typing shows.
    pub(crate) fn prepare(&mut self) {
        self.inputs = self
            .doc
            .node_ids()
            .filter_map(|id| Some((id, self.doc.get(id)?.play().input.clone()?)))
            .collect();
        self.inputs.sort_by_key(|(id, _)| *id);
        self.effects = self
            .doc
            .node_ids()
            .filter(|id| {
                self.doc
                    .get(*id)
                    .is_some_and(|n| n.play().scroll_effect.is_some())
            })
            .collect();
        self.effects.sort();
        let mut binds = Vec::new();
        for (id, input) in &self.inputs {
            let Some(n) = self.doc.get(*id) else { continue };
            let text = matches!(n.kind(), NodeKind::Text { .. });
            let string = self.doc.variables().kind_of(input.var) == Some(VarKind::String);
            let mut vars = n.text_vars();
            let slot = mastik_core::variable::TEXT_CONTENT;
            if input.property.is_none() && text && string && vars[slot] != Some(input.var) {
                vars[slot] = Some(input.var);
                binds.push(mastik_core::Operation::SetTextVars { id: *id, vars });
            }
        }
        if !binds.is_empty() {
            // A binding the door refuses leaves the layer showing its own text.
            let _ = self.commit(Transaction(binds));
        }
        self.hide_empty_templates();
    }

    /// A list over an empty table draws no row in play: its template, kept on
    /// the canvas to be edited, is hidden on the copy, and shown again as the
    /// document has it once the table has rows (a live edit). A list whose data
    /// set is missing keeps the rows it has.
    fn hide_empty_templates(&mut self) {
        use mastik_core::list;
        let mut shows = Vec::new();
        for l in list::lists(&self.doc) {
            let Some(x) = self.doc.get(l).and_then(|n| n.list()) else {
                continue;
            };
            let Some(t) = list::template(&self.doc, l) else {
                continue;
            };
            let Some(set) = self.doc.data_set(x.data) else {
                continue;
            };
            let want = !set.table.rows.is_empty() && self.base.get(t).is_none_or(|n| n.visible());
            if self.doc.get(t).is_some_and(|n| n.visible() != want) {
                shows.push(mastik_core::Operation::SetVisible {
                    id: t,
                    visible: want,
                });
            }
        }
        if !shows.is_empty() {
            // A refusal leaves the template drawn, as the canvas draws it.
            let _ = self.commit(Transaction(shows));
        }
    }

    /// Say `notice` about `action` on `layer` once.
    pub(crate) fn notice_once(&mut self, layer: NodeId, action: ItemId, notice: Notice) {
        if self.noticed.insert((layer, action)) {
            self.notices.push(notice);
        }
    }

    // ── Geometry ─────────────────────────────────────────────────────────────

    /// Whether `id` is a frame, which a navigation or an overlay can show.
    pub(crate) fn is_frame(&self, id: NodeId) -> bool {
        self.doc
            .get(id)
            .is_some_and(|n| matches!(n.kind(), NodeKind::Artboard { .. }))
    }

    /// The size `frame` is drawn at.
    pub(crate) fn size(&self, frame: NodeId) -> Size {
        self.res
            .used_kind(&self.doc, frame)
            .and_then(NodeKind::box_size)
            .or_else(|| self.res.world_bounds(frame).map(|r| r.size()))
            .unwrap_or(Size::ZERO)
    }

    /// `id`'s top-left in world space.
    pub(crate) fn origin(&self, id: NodeId) -> Point {
        self.res
            .world_transform(id)
            .map_or(Point::ZERO, |t| t * Point::ZERO)
    }

    /// The box `id` takes in world space, before any scroll: its own box for a
    /// layer that has one, else its bounds.
    pub(crate) fn rect(&self, id: NodeId) -> Option<Rect> {
        let kind = self.res.used_kind(&self.doc, id)?;
        match kind.box_size() {
            Some(size) => Some(
                self.res
                    .world_transform(id)?
                    .transform_rect_bbox(Rect::from_origin_size(Point::ZERO, size)),
            ),
            None => self.res.world_bounds(id),
        }
    }

    /// Where overlay `o` is drawn in screen space.
    pub(crate) fn overlay_at(&self, o: &Overlay) -> Point {
        use prototype::OverlayPosition as P;
        let (s, f) = (self.size(self.screen), self.size(o.frame));
        let (cx, cy) = ((s.width - f.width) / 2.0, (s.height - f.height) / 2.0);
        let (right, bottom) = (s.width - f.width, s.height - f.height);
        match o.settings.position {
            P::Centre => Point::new(cx, cy),
            P::TopLeft => Point::new(0.0, 0.0),
            P::TopCentre => Point::new(cx, 0.0),
            P::TopRight => Point::new(right, 0.0),
            P::BottomLeft => Point::new(0.0, bottom),
            P::BottomCentre => Point::new(cx, bottom),
            P::BottomRight => Point::new(right, bottom),
            P::Manual { x, y } => o.anchor + Vec2::new(x, y),
        }
    }

    /// `id`'s top-left in screen space as drawn now: in the shown frame holding
    /// it, moved by that frame's place (an overlay's) and by any scroll above it.
    /// The screen's origin when no shown frame holds it.
    pub(crate) fn drawn_corner(&self, id: NodeId) -> Point {
        let Some(frame) = self.frame_of(id) else {
            return Point::ZERO;
        };
        let at = match self.overlays.iter().find(|o| o.frame == frame) {
            Some(o) => self.overlay_at(o),
            None => Point::ZERO,
        };
        let corner = self.rect(id).map_or(self.origin(id), |r| r.origin());
        corner + self.drawn_shift(id) + (at - self.origin(frame))
    }

    /// The frames shown and acted on, bottom first: the screen, then each overlay.
    pub(crate) fn shown(&self) -> Vec<NodeId> {
        std::iter::once(self.screen)
            .chain(self.overlays.iter().map(|o| o.frame))
            .collect()
    }

    /// The shown frame holding `id`.
    pub(crate) fn frame_of(&self, id: NodeId) -> Option<NodeId> {
        let shown = self.shown();
        let mut at = Some(id);
        while let Some(x) = at {
            if shown.contains(&x) {
                return Some(x);
            }
            at = self.doc.get(x).and_then(|n| n.parent());
        }
        None
    }

    // ── Opening and closing frames ───────────────────────────────────────────

    /// A frame has opened: its *frame opens* reactions run and its timers start.
    pub(crate) fn on_open(&mut self, frame: NodeId) {
        let nodes = subtree_nodes(&self.doc, &[frame]);
        let mut opens = Vec::new();
        for id in nodes {
            let Some(n) = self.doc.get(id) else { continue };
            for r in n.reactions() {
                match r.value.trigger {
                    Trigger::FrameOpens => opens.push((id, r.id)),
                    Trigger::After { ms } => self.timers.push(Timer {
                        due: self.now + f64::from(ms),
                        holder: id,
                        reaction: r.id,
                        frame,
                    }),
                    _ => {}
                }
            }
        }
        for (id, reaction) in opens {
            self.run_reaction(id, reaction, None);
        }
    }

    /// A frame has closed: its timers go.
    pub(crate) fn on_close(&mut self, frame: NodeId) {
        self.timers.retain(|t| t.frame != frame);
    }

    /// `instance` has changed state: the timers its old state started go, and
    /// the *After* reactions of the state it shows now start. A variant whose
    /// *After* changes its instance to the next variant therefore loops for as
    /// long as its frame is shown.
    pub(crate) fn rearm(&mut self, instance: NodeId) {
        let nodes = subtree_nodes(&self.doc, &[instance]);
        self.timers.retain(|t| !nodes.contains(&t.holder));
        let Some(frame) = self.frame_of(instance) else {
            return;
        };
        for id in nodes {
            let Some(n) = self.doc.get(id) else { continue };
            for r in n.reactions() {
                if let Trigger::After { ms } = r.value.trigger {
                    self.timers.push(Timer {
                        due: self.now + f64::from(ms).max(MIN_LOOP_MS),
                        holder: id,
                        reaction: r.id,
                        frame,
                    });
                }
            }
        }
    }

    fn fire_timer(&mut self, timer: Timer) {
        if !self.shown().contains(&timer.frame) {
            return;
        }
        self.run_reaction(timer.holder, timer.reaction, None);
    }

    // ── Values ───────────────────────────────────────────────────────────────

    /// Set `var`'s play value. When it changes what the variable reads, the
    /// copy is laid out again, inputs bound to it follow, and *variable changes*
    /// reactions fire.
    pub(crate) fn set_play(&mut self, var: VarId, value: Value) {
        if self.doc.variables().kind_of(var) != Some(kind_of(&value)) {
            return;
        }
        let before = self.doc.resolve_var(self.screen, var);
        self.values.insert(var, value.clone());
        self.apply_values();
        if before.as_ref() != Some(&value) {
            self.changed(var);
        }
    }

    /// Put `var`'s play value back to `was` (none: its modes'), as a hover's end
    /// does.
    pub(crate) fn restore_value(&mut self, var: VarId, was: Option<Value>) {
        let before = self.doc.resolve_var(self.screen, var);
        match was {
            Some(v) => self.values.insert(var, v),
            None => self.values.remove(&var),
        };
        self.apply_values();
        if self.doc.resolve_var(self.screen, var) != before {
            self.changed(var);
        }
    }

    fn changed(&mut self, var: VarId) {
        self.sync_inputs(var);
        let mut fire = Vec::new();
        for frame in self.shown() {
            for id in subtree_nodes(&self.doc, &[frame]) {
                let Some(n) = self.doc.get(id) else { continue };
                for r in n.reactions() {
                    if r.value.trigger == (Trigger::VariableChanges { var }) {
                        fire.push((id, r.id));
                    }
                }
            }
        }
        for (id, reaction) in fire {
            self.run_reaction(id, reaction, None);
        }
    }

    /// Every input instance showing a variable switched to the variant for its
    /// value.
    pub(crate) fn sync_all_inputs(&mut self) {
        let vars: Vec<VarId> = self.inputs.iter().map(|(_, i)| i.var).collect();
        for var in vars {
            self.sync_inputs(var);
        }
    }
}

/// A start's page, its frame, and the cells its flow starts variables at.
type Started = (NodeId, NodeId, Vec<ModeId>, Vec<(VarId, Cell)>);

/// The page, the start frame and the starting cells of `start` in `doc`.
fn resolve_start(doc: &Document, start: Start) -> Result<Started, StartError> {
    let (frame, modes, cells) = match start {
        Start::Flow(r) => {
            let page = doc.page(r.page).ok_or(StartError::NoFlow)?;
            let flow = page
                .flows
                .iter()
                .find(|f| f.id == r.flow)
                .ok_or(StartError::NoFlow)?;
            let f = &flow.value;
            (f.start, f.modes.clone(), f.values.clone())
        }
        Start::Frame(f) => (f, Vec::new(), Vec::new()),
    };
    let is_frame = doc
        .get(frame)
        .is_some_and(|n| matches!(n.kind(), NodeKind::Artboard { .. }));
    let page = doc.page_of(frame).filter(|_| is_frame);
    let page = page.ok_or(StartError::NoFrame)?;
    Ok((page, frame, modes, cells))
}

/// The kind of variable `v` is a value of.
pub(crate) fn kind_of(v: &Value) -> VarKind {
    match v {
        Value::Color(_) => VarKind::Color,
        Value::Number(_) => VarKind::Number,
        Value::Boolean(_) => VarKind::Boolean,
        Value::String(_) => VarKind::String,
    }
}

/// `f` on every action in `list`, nested ones included.
pub(crate) fn each_action(
    list: &[mastik_core::Keyed<prototype::Action>],
    f: &mut impl FnMut(&prototype::Action),
) {
    for k in list {
        f(&k.value);
        let nested: Vec<_> = k.value.nested().cloned().collect();
        each_action(&nested, f);
    }
}
