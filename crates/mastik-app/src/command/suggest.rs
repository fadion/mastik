//! The completion list's open-and-shut state, as a code editor's suggest
//! widget keeps it: the command bar and the script editor each hold one, so
//! the two lists open, follow and shut alike.

/// Whether the list is up, and for which word.
#[derive(Default, Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) enum Suggest {
    #[default]
    Off,
    /// Up for the word starting at this byte.
    On(usize),
    /// Escape shut it for the word starting at this byte: typing on in that
    /// word leaves it shut, as an editor's does.
    Dismissed(usize),
}

/// A completion list's state over one text field.
#[derive(Default)]
pub(crate) struct Suggester {
    pub(crate) state: Suggest,
    /// The highlighted row.
    pub(crate) highlight: usize,
    /// The text as the field left it last frame, to tell typing from a
    /// keystroke-free frame. The caller sets it before the field draws, and
    /// again after writing the text itself, so a written word is not typing.
    pub(crate) seen: String,
}

impl Suggester {
    /// Move the state on from what changed in `text` since last frame. Typing
    /// a name's letter, `.` or `:`, or the space before a verb's argument opens
    /// it; any other character shuts it; deleting keeps it while the word
    /// stays, and a caret that leaves the word shuts it. `invoke`
    /// (`Ctrl+Space`) opens it on the word at the caret whatever came before.
    pub(crate) fn follow(
        &mut self,
        text: &str,
        c: Option<&mastik_vm::Completion>,
        caret: usize,
        invoke: bool,
    ) {
        let start = c.map(|c| c.replace.start);
        if text != self.seen {
            self.highlight = 0;
            let grew = text.len() > self.seen.len();
            self.state = match (c, start) {
                (Some(c), Some(s)) if s <= caret && caret <= text.len() => {
                    let word = &text[s..caret];
                    let trigger = trigger_at(text, s, c);
                    match self.state {
                        Suggest::Dismissed(a) if a == s => Suggest::Dismissed(a),
                        _ if grew => {
                            let typed = text[..caret].chars().next_back();
                            let opens = match typed {
                                Some(ch) if ch.is_alphanumeric() || ch == '_' => {
                                    !word.starts_with(|d: char| d.is_ascii_digit())
                                }
                                _ => word.is_empty() && trigger,
                            };
                            match opens {
                                true => Suggest::On(s),
                                false => Suggest::Off,
                            }
                        }
                        Suggest::On(a) if a == s && (caret > s || trigger) => Suggest::On(a),
                        _ => Suggest::Off,
                    }
                }
                _ => Suggest::Off,
            };
        }
        if let Suggest::On(a) | Suggest::Dismissed(a) = self.state
            && Some(a) != start
        {
            self.state = Suggest::Off;
        }
        if invoke && let Some(s) = start {
            self.state = Suggest::On(s);
        }
    }

    /// Whether the list is up.
    pub(crate) fn on(&self) -> bool {
        matches!(self.state, Suggest::On(_))
    }

    /// Escape on an open list: shut it for this word.
    pub(crate) fn dismiss(&mut self) {
        if let Suggest::On(a) = self.state {
            self.state = Suggest::Dismissed(a);
        }
    }

    /// Take row `i` of `c` into `text` and shut the list; answers the caret's
    /// new byte offset, just past what was written.
    pub(crate) fn accept(
        &mut self,
        text: &mut String,
        c: &mastik_vm::Completion,
        i: usize,
    ) -> Option<usize> {
        let row = c.rows.get(i)?;
        let mut r = c.replace.clone();
        if r.end > text.len() {
            return None;
        }
        // A member written bracketed takes the place of its dot too.
        let insert = match &row.member {
            Some(m) => {
                if text.get(..r.start).is_some_and(|t| t.ends_with('.')) {
                    r.start -= 1;
                }
                m.as_str()
            }
            None => insert_of(row),
        };
        text.replace_range(r.clone(), insert);
        self.highlight = 0;
        self.state = Suggest::Off;
        Some(r.start + insert.len())
    }

    /// Move the highlight among `rows` rows: one down, or up when `up`.
    pub(crate) fn step(&mut self, rows: usize, up: bool) {
        if rows == 0 {
            return;
        }
        self.highlight = match up {
            true => (self.highlight + rows - 1) % rows,
            false => (self.highlight + 1) % rows,
        };
    }

    /// Whether taking the highlighted row would change `text`: on a word already
    /// whole, Enter is the field's own (it runs the line, or breaks it).
    pub(crate) fn changes(&self, text: &str, c: &mastik_vm::Completion) -> bool {
        c.rows
            .get(self.highlight)
            .is_some_and(|row| text.get(c.replace.clone()) != Some(insert_of(row)))
    }
}

/// Whether an empty word at `start` sits where a list opens on its own: just
/// after `.` or `:`, or at a verb's argument, whose choices `c` holds.
fn trigger_at(text: &str, start: usize, c: &mastik_vm::Completion) -> bool {
    let before = text[..start].chars().next_back();
    matches!(before, Some('.' | ':'))
        || (!c.rows.is_empty()
            && c.rows
                .iter()
                .all(|r| r.section == mastik_vm::Section::Choices))
}

/// What taking `row` writes: an almost-match's suggestion, else its label.
fn insert_of(row: &mastik_vm::Row) -> &str {
    match (&row.suggestion, row.dim) {
        (Some(s), true) => s,
        _ => &row.label,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A table whose name Luau cannot spell after a dot is picked bracketed over
    /// the dot; a plain name is picked as it is.
    /// flip: `complete`'s `members` setting no `member` → fails at "bracketed
    /// over the dot" with the bare name; `accept` leaving the dot → the same
    /// assertion, with `tables.[…]`.
    #[test]
    fn a_spaced_table_name_is_picked_bracketed() {
        let mut ids = mastik_core::IdSource::new(0x7E5B);
        let page = ids.mint();
        let mut doc = mastik_core::Document::new(page);
        for name in ["Pending orders", "People"] {
            let set = mastik_core::list::DataSet {
                id: ids.mint_data_set(),
                name: name.into(),
                table: Default::default(),
                source: None,
                pictures: Vec::new(),
            };
            let id = set.id;
            doc.apply(&mastik_core::Transaction(vec![
                mastik_core::Operation::SetDataSet {
                    id,
                    set: Some(Box::new(set)),
                },
            ]))
            .expect("a table");
        }
        let source = "local t = tables.Pe";
        let c = mastik_vm::complete_script(&doc, page, source, source.len());
        let pick = |label: &str| {
            let i = c.rows.iter().position(|r| r.label == label).unwrap();
            let mut text = source.to_owned();
            let at = Suggester::default().accept(&mut text, &c, i);
            (text, at)
        };
        let (text, at) = pick("Pending orders");
        assert_eq!(
            text, "local t = tables[\"Pending orders\"]",
            "bracketed over the dot"
        );
        assert_eq!(at, Some(text.len()), "the caret after it");
        assert_eq!(pick("People").0, "local t = tables.People");
    }
}
