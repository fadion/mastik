//! Live update: the editor's commits applied to the played copy as they land.

use crate::player::{Fallback, Notice, Player};
use mastik_core::{NodeId, OpError, Resolved, Transaction, variant};

impl Player {
    /// Apply a transaction the editor committed to its document.
    ///
    /// Ids are minted by their callers and operations are deterministic, so the
    /// copy takes the same transaction and stays the document plus what play did
    /// to it, play state untouched, and what the edit wrote over *Set mode*'s
    /// modes or the screen's play size put back. When the copy refuses it, because play
    /// changed something the edit also touches, the copy is cloned again from
    /// the document and play state carried across by id: the screen, history,
    /// overlays, play values, scroll offsets and each instance's state. Either
    /// way, each row play opened is opened again (`reopen_rows`). What the
    /// edit removed falls back, with one notice each.
    ///
    /// `Err` only when the transaction does not apply to the document the player
    /// started from either: the shell passed a commit of another document, or
    /// missed one. The player is unchanged then.
    pub fn apply_edit(&mut self, tx: &Transaction) -> Result<(), OpError> {
        let played = self.played_tables();
        self.base.apply(tx)?;
        match self.doc.apply(tx) {
            Ok(out) => {
                self.res.update(&self.doc, &out.dirty);
                let settled = self.res.settle(&mut self.doc);
                self.dirty.extend(out.dirty.0);
                self.dirty.extend(settled.0);
                self.put_play_back();
            }
            Err(_) => self.reclone(),
        }
        self.carry_tables(played);
        self.prepare();
        self.reopen_rows();
        self.fall_back();
        Ok(())
    }

    /// Play `doc` in place of the document the player started from, keeping
    /// the run's place as a refused live edit does: the copy is cloned again
    /// and play state carried across by id, what `doc` lacks falling back.
    /// For a shell that is sent the document whole rather than each commit (a
    /// phone mirroring the app, whose trimmed copy cannot take the editor's
    /// transactions).
    pub fn rebase(&mut self, doc: &mastik_core::Document) {
        let played = self.played_tables();
        self.base = doc.clone();
        self.reclone();
        self.carry_tables(played);
        self.prepare();
        self.reopen_rows();
        self.fall_back();
    }

    /// After an edit the copy took, what the edit wrote over play's own choices
    /// put back: each node's modes *Set mode* gave it, and the screen's play
    /// size. Edits are absolute, so a `SetModes` or a resize of the screen
    /// lands on the copy whole.
    fn put_play_back(&mut self) {
        let mut undone: Vec<_> = self
            .modes
            .iter()
            .filter(|(id, set)| {
                self.doc
                    .get(**id)
                    .is_some_and(|n| n.modes() != set.as_slice())
            })
            .map(|(id, set)| (*id, set.clone()))
            .collect();
        undone.sort();
        for (id, set) in undone {
            self.put_modes(id, set);
        }
        self.fit_screen(self.screen);
    }

    /// The copy cloned again from the document, with each instance play
    /// switched put back in its state and the play values set.
    fn reclone(&mut self) {
        self.doc = self.base.clone();
        self.res = Resolved::settled(&mut self.doc);
        self.dirty.extend(self.doc.node_ids());
        // Instances first, then values: the values' inputs may switch them again.
        let mut states: Vec<_> = self.states.iter().map(|(i, m)| (*i, *m)).collect();
        states.sort();
        self.states.clear();
        for (instance, main) in states {
            let there = self.doc.get(instance).is_some() && self.doc.get(main).is_some();
            if there && self.doc.get(instance).and_then(|n| n.link()) != Some(main) {
                if let Some(tx) = variant::switch(&self.doc, instance, main, &mut self.ids) {
                    // A state that no longer switches stays as the document has it.
                    let _ = self.commit(tx);
                }
                self.states.insert(instance, main);
            }
        }
        let mut modes: Vec<_> = self.modes.drain().collect();
        modes.sort();
        for (id, set) in modes {
            if self.doc.get(id).is_some() {
                self.put_modes(id, set);
            }
        }
        self.fit_screen(self.screen);
        // A settle the re-clone cuts short ends where it was going, and drops.
        for a in std::mem::take(&mut self.anims.moves) {
            self.moved.insert(a.layer, a.to);
            self.drops.push((a.layer, a.at));
        }
        self.anims = Default::default();
    }

    /// What an edit took away, fallen back from: the screen to the start, an
    /// overlay closed, a variable's play value dropped.
    fn fall_back(&mut self) {
        let vars = self.doc.variables();
        let before = self.values.len();
        self.values.retain(|v, _| vars.get(*v).is_some());
        if self.values.len() != before {
            self.notices.push(Notice::Fallback(Fallback::Variable));
        }
        self.apply_values();
        let open = self.overlays.len();
        let gone: Vec<_> = self
            .overlays
            .iter()
            .filter(|o| !self.is_frame(o.frame))
            .map(|o| o.frame)
            .collect();
        self.overlays.retain(|o| !gone.contains(&o.frame));
        for frame in gone {
            self.on_close(frame);
        }
        if self.overlays.len() != open {
            self.notices.push(Notice::Fallback(Fallback::Overlay));
        }
        self.history.retain(|v| self.doc.get(v.frame).is_some());
        self.scroll.retain(|f, _| self.doc.get(*f).is_some());
        self.scroll_seen.retain(|f, _| self.doc.get(*f).is_some());
        let follows = |doc: &mastik_core::Document, l: &NodeId| {
            doc.get(*l).is_some_and(|n| n.play().follow.is_some())
        };
        self.moved.retain(|l, _| follows(&self.doc, l));
        self.anims.moves.retain(|a| follows(&self.doc, &a.layer));
        self.drops.retain(|(l, _)| follows(&self.doc, l));
        self.states.retain(|i, _| self.doc.get(*i).is_some());
        self.auto.retain(|i, _| self.doc.get(*i).is_some());
        if self.focus_shown.is_some_and(|f| self.doc.get(f).is_none()) {
            self.focus_shown = None;
        }
        if self.focus.is_some_and(|f| self.doc.get(f).is_none()) {
            self.set_focus(None);
        }
        if !self.is_frame(self.screen) {
            let start = match self.start {
                crate::Start::Frame(f) => Some(f),
                crate::Start::Flow(r) => self
                    .doc
                    .page(r.page)
                    .and_then(|p| p.flows.iter().find(|f| f.id == r.flow))
                    .map(|f| f.value.start),
            };
            let first = || {
                self.doc.page_ids().find_map(|p| {
                    self.doc
                        .get(p)?
                        .children()
                        .iter()
                        .copied()
                        .find(|c| self.is_frame(*c))
                })
            };
            if let Some(to) = start.filter(|f| self.is_frame(*f)).or_else(first) {
                self.timers.clear();
                self.anims = Default::default();
                self.history.clear();
                self.screen = to;
                if let Some(page) = self.doc.page_of(to) {
                    self.page = page;
                }
                self.fit_screen(to);
                self.on_open(to);
            }
            self.notices.push(Notice::Fallback(Fallback::Screen));
        }
        self.timers.retain(|t| self.doc.get(t.holder).is_some());
        self.rehover();
    }
}
