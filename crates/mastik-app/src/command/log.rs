//! The command log: the bar's history and the app's Info log. Every undo step is
//! listed as the command that would make it, so a person or an agent learns the
//! language by watching; a step with no command yet is a visible hole in what the
//! language covers.

use mastik_core::kurbo::{Affine, Vec2};
use mastik_core::{Document, NodeId, Operation, Resolved, Transaction};
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
}

/// A document's log for the session. Not saved.
#[derive(Default)]
pub(crate) struct Log {
    entries: VecDeque<Entry>,
    /// The newest step serial seen, and the revision the document was at then: a
    /// run that folds more edits into its step keeps the serial and moves the
    /// revision, and its line is rendered again.
    seen: Option<(u64, u64)>,
}

impl Log {
    /// The entries, oldest first.
    pub(crate) fn entries(&self) -> impl DoubleEndedIterator<Item = &Entry> + ExactSizeIterator {
        self.entries.iter()
    }

    /// Log a line typed in the bar or a script run, with the step it made, if any.
    pub(crate) fn push(&mut self, entry: Entry) {
        if let Some(serial) = entry.serial {
            self.seen = Some((serial, u64::MAX));
        }
        self.entries.push_back(entry);
        while self.entries.len() > KEEP {
            self.entries.pop_front();
        }
    }

    /// Note the undo step on top of `s`'s history if it is new, or re-render it if
    /// a run folded more into it; called once a frame.
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
        self.seen = Some((top.serial, rev));
        let line = render(&s.doc, &s.resolved, top.inverse);
        self.entries.push_back(Entry {
            luau: line.as_deref().map(luau_of),
            line,
            source: Source::Edit {
                label: top.note.label.clone(),
                glyph: top.note.glyph.clone(),
            },
            result: String::new(),
            failed: false,
            serial: Some(top.serial),
        });
        while self.entries.len() > KEEP {
            self.entries.pop_front();
        }
    }

    /// The newest line typed in the bar, for `.` to repeat.
    pub(crate) fn last_typed(&self) -> Option<&str> {
        self.entries
            .iter()
            .rev()
            .find(|e| e.source == Source::Bar && !e.failed)
            .and_then(|e| e.line.as_deref())
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
        }
    }
}

/// The Luau a rendered command is: each `;`-separated property line through the
/// bar's sugar, which for a rendered line is only subject elision and adjust.
fn luau_of(line: &str) -> String {
    line.split("; ")
        .map(|part| mastik_vm::desugar(part).luau)
        .collect::<Vec<_>>()
        .join("\n")
}

/// `s` as a Luau string literal.
pub(crate) fn quoted(s: &str) -> String {
    let mut out = String::with_capacity(s.len() + 2);
    out.push('"');
    for c in s.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            c => out.push(c),
        }
    }
    out.push('"');
    out
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

    #[test]
    fn a_name_is_quoted_with_its_quotes_escaped() {
        assert_eq!(quoted(r#"My "Frame""#), r#""My \"Frame\"""#);
    }
}
