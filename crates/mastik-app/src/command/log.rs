//! The command log: the bar's history and the app's Info log. Every undo step is
//! listed as the command that would make it, so a person or an agent learns the
//! language by watching; a step with no command yet is a visible hole in what the
//! language covers.

use mastik_core::kurbo::{Affine, Vec2};
use mastik_core::{Document, NodeId, Operation, Resolved, Transaction};
use mastik_vm::quoted;
use std::collections::VecDeque;

/// How many entries a document's log keeps; the bar shows the newest few.
const KEEP: usize = 200;

/// Where an entry came from.
#[derive(Clone, Debug, PartialEq)]
pub(crate) enum Source {
    /// A line typed in the bar.
    Bar,
    /// A script run by hand, by its name.
    Script(String),
    /// An `/ai` request typed in the bar, with the assistant's steps it led to.
    Ai,
    /// An edit made anywhere else, as its undo step names it, with the glyph its
    /// step wears.
    Edit {
        label: String,
        glyph: std::borrow::Cow<'static, str>,
    },
}

/// One logged line.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct Entry {
    /// The line as typed, or the command an edit renders as; `None` for an edit
    /// the log has no command for yet.
    pub(crate) line: Option<String>,
    /// The line's Luau, as the bar's footer showed it: what *Copy as script* writes.
    pub(crate) luau: Option<String>,
    pub(crate) source: Source,
    /// What came of it: a read's value, a count, an error.
    pub(crate) result: String,
    /// Whether the line failed, so it reads in the error ink.
    pub(crate) failed: bool,
    /// The undo step it made, so the watcher does not list that step twice.
    serial: Option<u64>,
    /// Its place in the log, counted from the session's first entry: what the
    /// bar picks rows by, since an index shifts as old entries fall off.
    seq: u64,
}

/// A document's log for the session. Not saved.
#[derive(Default)]
pub(crate) struct Log {
    entries: VecDeque<Entry>,
    /// The newest step serial seen, and the revision the document was at then: a
    /// run that folds more edits into its step keeps the serial and moves the
    /// revision, and its line is rendered again.
    seen: Option<(u64, u64)>,
    /// The newest entry is an `/ai` request still taking the assistant's steps
    /// as its own, until the person edits, the bar logs another line, or the
    /// panel asks another question.
    ai_open: bool,
    /// The next entry's place in the log.
    next_seq: u64,
}

impl Log {
    /// The entries, oldest first.
    pub(crate) fn entries(&self) -> impl DoubleEndedIterator<Item = &Entry> + ExactSizeIterator {
        self.entries.iter()
    }

    /// Log a line typed in the bar or a script run, with the step it made, if any.
    pub(crate) fn push(&mut self, entry: Entry) {
        self.ai_open = false;
        if let Some(serial) = entry.serial {
            self.seen = Some((serial, u64::MAX));
        }
        self.append(entry);
    }

    /// Add `entry` as the newest, numbered, the oldest dropped past [`KEEP`].
    fn append(&mut self, mut entry: Entry) {
        entry.seq = self.next_seq;
        self.next_seq += 1;
        self.entries.push_back(entry);
        while self.entries.len() > KEEP {
            self.entries.pop_front();
        }
    }

    /// Log `/ai request` as sent: the assistant's steps from here on are its
    /// own, not rows of their own with no command.
    pub(crate) fn asked(&mut self, request: &str) {
        self.push(Entry {
            line: Some(format!("/ai {request}")),
            luau: None,
            source: Source::Ai,
            result: "Asked".into(),
            failed: false,
            serial: None,
            seq: 0,
        });
        self.ai_open = true;
    }

    /// The open `/ai` request, while it takes the assistant's steps.
    fn open_ai(&mut self) -> Option<&mut Entry> {
        match self.ai_open {
            true => self.entries.back_mut().filter(|e| e.source == Source::Ai),
            false => None,
        }
    }

    /// What the open `/ai` request's reply came to, while no step of its says
    /// more.
    pub(crate) fn ai_replied(&mut self, result: &str) {
        if let Some(e) = self.open_ai()
            && e.serial.is_none()
        {
            e.result = result.to_owned();
        }
    }

    /// The `/ai` request's script ran: its entry takes the script, what came
    /// of it and its step, or a new entry does once the request is closed.
    pub(crate) fn ai_ran(
        &mut self,
        request: &str,
        script: String,
        result: String,
        serial: Option<u64>,
    ) {
        let line = format!("/ai {request}");
        match self.open_ai() {
            Some(e) if e.line.as_deref() == Some(line.as_str()) => {
                e.luau = Some(script);
                e.result = result;
                e.serial = serial.or(e.serial);
                if let Some(serial) = serial {
                    self.seen = Some((serial, u64::MAX));
                }
            }
            _ => {
                let mut entry = Entry::typed(&line, script, result, false, serial);
                entry.source = Source::Ai;
                self.push(entry);
            }
        }
    }

    /// The panel asked another question: the open `/ai` request takes no more
    /// steps.
    pub(crate) fn close_ai(&mut self) {
        self.ai_open = false;
    }

    /// Take the step on top of a history resumed from an earlier session as seen,
    /// so the log does not list it as an edit made now.
    pub(crate) fn seen_up_to(&mut self, serial: Option<u64>) {
        self.seen = serial.map(|s| (s, u64::MAX));
    }

    /// Note the undo steps made since the log last looked, or re-render the top
    /// one if a run folded more into it; called once a frame.
    pub(crate) fn watch(s: &mut crate::session::EditorSession) {
        let mut log = std::mem::take(&mut s.command_log);
        log.observe(s);
        s.command_log = log;
    }

    /// [`Self::watch`]'s work, on a log taken out of `s`.
    fn observe(&mut self, s: &crate::session::EditorSession) {
        let Some(top) = s.history.done().next_back() else {
            return;
        };
        let rev = s.revision();
        match self.seen {
            Some((serial, _)) if top.serial < serial => return,
            Some((serial, at)) if top.serial == serial => {
                if at == rev || at == u64::MAX {
                    if at == u64::MAX {
                        self.seen = Some((serial, rev));
                    }
                    return;
                }
                self.seen = Some((serial, rev));
                // A run grew: render its line again, if it is the newest.
                if let Some(last) = self.entries.back_mut()
                    && last.serial == Some(serial)
                    && matches!(last.source, Source::Edit { .. })
                {
                    last.line = render(&s.doc, &s.resolved, top.inverse);
                    last.luau = last.line.as_deref().map(luau_of);
                }
                return;
            }
            _ => {}
        }
        // Every step past the one last seen, oldest first: several can land in
        // one frame (two agents' calls). A fresh log takes only the top.
        let since = self.seen.map(|(serial, _)| serial);
        let mut new: Vec<_> = s
            .history
            .done()
            .rev()
            .take_while(|step| since.is_some_and(|seen| step.serial > seen))
            .collect();
        if new.is_empty() {
            new.push(top);
        }
        new.reverse();
        self.seen = Some((top.serial, rev));
        let subjects: Vec<Vec<NodeId>> = new.iter().map(|step| step.inverse.subjects()).collect();
        for (i, step) in new.iter().enumerate() {
            // A step is rendered against the document now, which a later step
            // of the same frame may have changed under it: then it is a hole.
            let overwritten = subjects[i + 1..]
                .iter()
                .any(|later| later.iter().any(|id| subjects[i].contains(id)));
            let line = match overwritten {
                true => None,
                false => render(&s.doc, &s.resolved, step.inverse),
            };
            self.log_step(step, line);
        }
    }

    /// Log `step`, rendered as `line`.
    fn log_step(&mut self, step: &mastik_core::StepView<'_>, line: Option<String>) {
        // An agent's step while an `/ai` request is open is that request's,
        // named by the step; the person's own edit closes the request. A step
        // carries the agent's name, never the chat it came from, so any agent's
        // counts.
        let agents = match &step.note.origin {
            mastik_core::Origin::Mcp { .. } => true,
            mastik_core::Origin::User => {
                self.ai_open = false;
                false
            }
        };
        if agents && let Some(e) = self.open_ai() {
            e.serial = Some(step.serial);
            e.result = step.note.label.clone();
            return;
        }
        // An agent's step with no command teaches no one the language: it is
        // left out, where the person's own shows as a hole to fill.
        if agents && line.is_none() {
            return;
        }
        self.append(Entry {
            luau: line.as_deref().map(luau_of),
            line,
            source: Source::Edit {
                label: step.note.label.clone(),
                glyph: step.note.glyph.clone(),
            },
            result: String::new(),
            failed: false,
            serial: Some(step.serial),
            seq: 0,
        });
    }

    /// The newest line that ran, typed in the bar or written for an edit made
    /// elsewhere, for `.` to repeat. A script's run is no line, and neither is an
    /// `/ai` request, which would ask again.
    pub(crate) fn last_line(&self) -> Option<&str> {
        self.entries
            .iter()
            .rev()
            .filter(|e| !e.failed && !matches!(e.source, Source::Script(_) | Source::Ai))
            .find_map(|e| e.line.as_deref())
    }
}

impl Entry {
    /// A bar line, its Luau, what came of it, and the step it made.
    pub(crate) fn typed(
        line: &str,
        luau: String,
        result: String,
        failed: bool,
        serial: Option<u64>,
    ) -> Self {
        Self {
            line: Some(line.to_owned()),
            luau: Some(luau),
            source: Source::Bar,
            result,
            failed,
            serial,
            seq: 0,
        }
    }

    /// A script run: `run "name"` and its report.
    pub(crate) fn script(name: &str, result: String, failed: bool, serial: Option<u64>) -> Self {
        let line = format!("run {}", quoted(name));
        Self {
            luau: None,
            line: Some(line),
            source: Source::Script(name.to_owned()),
            result,
            failed,
            serial,
            seq: 0,
        }
    }

    /// Its place in the log, which the bar picks rows by.
    pub(crate) fn seq(&self) -> u64 {
        self.seq
    }
}

/// The Luau a rendered command is: the line through the bar's sugar, which for a
/// rendered line is only subject elision and adjust, each `;`-separated
/// statement by itself.
fn luau_of(line: &str) -> String {
    mastik_vm::desugar(line).luau
}

/// `v` as the bar writes a number: at most two decimals, no trailing zeros.
pub(crate) fn number(v: f64) -> String {
    let r = (v * 100.0).round() / 100.0;
    let r = if r == 0.0 { 0.0 } else { r };
    let s = format!("{r:.2}");
    s.trim_end_matches('0').trim_end_matches('.').to_owned()
}

/// The command that would make the step whose inverse is `inverse`, read against
/// the document after it: each kind of operation the language can say, in the
/// property form, joined by `; `. `None` when any part has no command yet.
pub(crate) fn render(doc: &Document, res: &Resolved, inverse: &Transaction) -> Option<String> {
    let mut moves: Vec<Vec2> = Vec::new();
    let mut opacity: Vec<f32> = Vec::new();
    let mut names: Vec<String> = Vec::new();
    let mut visible: Vec<bool> = Vec::new();
    let mut locked: Vec<bool> = Vec::new();
    for op in &inverse.0 {
        match op {
            Operation::SetTransform { id, transform } => {
                moves.push(moved_by(doc, res, *id, *transform)?)
            }
            Operation::SetOpacity { id, .. } => opacity.push(doc.get(*id)?.opacity()),
            Operation::SetName { id, .. } => names.push(doc.get(*id)?.name().to_string()),
            Operation::SetVisible { id, .. } => visible.push(doc.get(*id)?.visible()),
            Operation::SetLocked { id, .. } => locked.push(doc.get(*id)?.locked()),
            _ => return None,
        }
    }
    let mut lines = Vec::new();
    if let Some(d) = one(&moves, |a, b| (*a - *b).hypot() < 1e-6) {
        for (prop, v) in [("x", d.x), ("y", d.y)] {
            if v.abs() >= 0.005 {
                let op = if v < 0.0 { "-=" } else { "+=" };
                lines.push(format!(".{prop} {op} {}", number(v.abs())));
            }
        }
    } else if !moves.is_empty() {
        return None;
    }
    if let Some(v) = one(&opacity, |a, b| (a - b).abs() < 1e-4) {
        lines.push(format!(".opacity = {}", number(f64::from(*v))));
    } else if !opacity.is_empty() {
        return None;
    }
    if let Some(v) = one(&names, |a, b| a == b) {
        lines.push(format!(".name = {}", quoted(v)));
    } else if !names.is_empty() {
        return None;
    }
    for (prop, list) in [("visible", &visible), ("locked", &locked)] {
        if let Some(v) = one(list, |a, b| a == b) {
            lines.push(format!(".{prop} = {v}"));
        } else if !list.is_empty() {
            return None;
        }
    }
    (!lines.is_empty()).then(|| lines.join("; "))
}

/// The one value every item of `list` agrees on; `None` for an empty list or one
/// that disagrees.
fn one<T>(list: &[T], same: impl Fn(&T, &T) -> bool) -> Option<&T> {
    let first = list.first()?;
    list.iter().all(|v| same(first, v)).then_some(first)
}

/// How far a transform edit moved `id` in world space, given the transform it
/// replaced; `None` when it did more than move it (a turn, a scale).
fn moved_by(doc: &Document, res: &Resolved, id: NodeId, before: Affine) -> Option<Vec2> {
    let node = doc.get(id)?;
    let parent = node
        .parent()
        .and_then(|p| res.world_transform(p))
        .unwrap_or(Affine::IDENTITY);
    let (old, new) = (parent * before, parent * node.transform());
    let [a, b, c, d, _, _] = old.as_coeffs();
    let [e, f, g, h, _, _] = new.as_coeffs();
    let same = [a - e, b - f, c - g, d - h].iter().all(|x| x.abs() < 1e-9);
    same.then(|| new.translation() - old.translation())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn numbers_are_written_short() {
        assert_eq!(number(15.0), "15");
        assert_eq!(number(0.6), "0.6");
        assert_eq!(number(12.345), "12.35");
        assert_eq!(number(-0.001), "0");
    }
}
