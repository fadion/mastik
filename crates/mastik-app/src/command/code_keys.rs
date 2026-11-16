//! The script editor's editing keys, as text edits: indenting on `Enter`, a
//! block's closing word stepping back as it is typed, `Tab` and `Shift+Tab`
//! over lines, `Ctrl+/` comments, moving and copying lines, and brackets and
//! quotes typed in pairs. Each is a pure function of the text and a selection,
//! so the window only maps keys onto them.

use std::ops::Range;

/// One step of indentation: two spaces. A line already indented with tabs
/// keeps them: `Enter` copies a line's own leading whitespace, and `Shift+Tab`
/// takes a tab as one step.
pub(crate) const INDENT: &str = "  ";

/// A selection in byte offsets: where it was started, and the caret. Equal
/// when nothing is selected.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct Sel {
    pub(crate) anchor: usize,
    pub(crate) caret: usize,
}

impl Sel {
    pub(crate) fn caret(at: usize) -> Self {
        Sel {
            anchor: at,
            caret: at,
        }
    }

    fn range(self) -> Range<usize> {
        self.anchor.min(self.caret)..self.anchor.max(self.caret)
    }

    /// `r` selected in this selection's direction.
    fn over(self, r: Range<usize>) -> Self {
        match self.anchor <= self.caret {
            true => Sel {
                anchor: r.start,
                caret: r.end,
            },
            false => Sel {
                anchor: r.end,
                caret: r.start,
            },
        }
    }

    /// This selection moved by `by` bytes.
    fn shifted(self, by: isize) -> Self {
        let at = |i: usize| i.saturating_add_signed(by);
        Sel {
            anchor: at(self.anchor),
            caret: at(self.caret),
        }
    }
}

/// Where the line holding byte `i` starts.
fn line_start(text: &str, i: usize) -> usize {
    text[..i].rfind('\n').map_or(0, |n| n + 1)
}

/// Where the line holding byte `i` ends, before its newline.
fn line_end(text: &str, i: usize) -> usize {
    text[i..].find('\n').map_or(text.len(), |n| i + n)
}

/// The leading spaces and tabs of `line`.
fn indent_of(line: &str) -> &str {
    &line[..line.len() - line.trim_start_matches([' ', '\t']).len()]
}

/// The whole lines `sel` touches, from the first's start to the last's end. A
/// selection that ends at a line's very start leaves that line out, as it
/// selects none of it.
fn lines_of(text: &str, sel: Sel) -> Range<usize> {
    let r = sel.range();
    let mut last = r.end;
    if r.end > r.start && line_start(text, r.end) == r.end {
        last -= 1;
    }
    line_start(text, r.start)..line_end(text, last)
}

/// The byte offset each line of `lines` starts at.
fn starts(text: &str, lines: Range<usize>) -> Vec<usize> {
    std::iter::once(lines.start)
        .chain(
            text[lines.clone()]
                .match_indices('\n')
                .map(|(i, _)| lines.start + i + 1),
        )
        .collect()
}

/// Whether `c` may stand in a name.
fn word_char(c: char) -> bool {
    c.is_alphanumeric() || c == '_'
}

/// Whether `line` opens a block whose body goes one step in: it ends with
/// `then`, `do`, `else`, `repeat`, an open bracket, or a function's parameters.
fn opens_block(line: &str) -> bool {
    let line = line.trim_end();
    if line.ends_with(['{', '(', '[']) {
        return true;
    }
    let ends_with_word = |w: &str| {
        line.strip_suffix(w)
            .is_some_and(|before| !before.ends_with(word_char))
    };
    if ["then", "do", "else", "repeat"]
        .into_iter()
        .any(ends_with_word)
    {
        return true;
    }
    // A function written whole on the line (`f(function() return 1 end)`) opens nothing.
    let mut words = line.split(|c: char| !word_char(c));
    line.ends_with(')') && words.clone().any(|w| w == "function") && !words.any(|w| w == "end")
}

/// `indent` one step shallower: a tab, or up to a step of spaces, off its end.
fn shallower(indent: &str) -> &str {
    if let Some(rest) = indent.strip_suffix('\t') {
        return rest;
    }
    let spaces = indent.len() - indent.trim_end_matches(' ').len();
    &indent[..indent.len() - spaces.min(INDENT.len())]
}

/// `Enter`: break the line at the caret (the selection replaced), the new line
/// indented as this one, one step further when this one opens a block. Between
/// an open bracket and its close, the close goes on a line of its own.
pub(crate) fn newline(text: &mut String, sel: Sel) -> Sel {
    let r = sel.range();
    text.replace_range(r.clone(), "");
    let at = r.start;
    let before = &text[line_start(text, at)..at];
    let base = indent_of(before).to_owned();
    let inner = match opens_block(before) {
        true => format!("{base}{INDENT}"),
        false => base.clone(),
    };
    let pair = matches!(
        (before.chars().next_back(), text[at..].chars().next()),
        (Some('('), Some(')')) | (Some('{'), Some('}')) | (Some('['), Some(']'))
    );
    let insert = match pair {
        true => format!("\n{inner}\n{base}"),
        false => format!("\n{inner}"),
    };
    text.insert_str(at, &insert);
    Sel::caret(at + 1 + inner.len())
}

/// `Tab`: a selection over more than one line indents each of its lines one
/// step (an empty line stays empty) and keeps them selected whole; otherwise
/// the selection is replaced by spaces to the next step.
pub(crate) fn indent(text: &mut String, sel: Sel) -> Sel {
    let r = sel.range();
    if !text[r.clone()].contains('\n') {
        text.replace_range(r.clone(), "");
        let column = text[line_start(text, r.start)..r.start].chars().count();
        let pad = INDENT.len() - column % INDENT.len();
        text.insert_str(r.start, &" ".repeat(pad));
        return Sel::caret(r.start + pad);
    }
    let lines = lines_of(text, sel);
    let mut added = 0;
    for s in starts(text, lines.clone()) {
        let at = s + added;
        if at == text.len() || text[at..].starts_with('\n') {
            continue;
        }
        text.insert_str(at, INDENT);
        added += INDENT.len();
    }
    sel.over(lines.start..lines.end + added)
}

/// `Shift+Tab`: each line the selection touches one step shallower. A
/// selection over more than one line keeps its lines selected whole; a caret
/// stays where it was in its line's text.
pub(crate) fn outdent(text: &mut String, sel: Sel) -> Sel {
    let lines = lines_of(text, sel);
    let many = text[sel.range()].contains('\n');
    let mut removed = 0;
    let mut before_caret = 0;
    for s in starts(text, lines.clone()) {
        let at = s - removed;
        let line = &text[at..line_end(text, at)];
        let indent = indent_of(line);
        let take = indent.len() - shallower(indent).len();
        text.replace_range(at + indent.len() - take..at + indent.len(), "");
        if !many && sel.caret >= s {
            before_caret += take.min(sel.caret - s);
        }
        removed += take;
    }
    match many {
        true => sel.over(lines.start..lines.end - removed),
        false => {
            let caret = sel.caret - before_caret;
            Sel::caret(caret.max(line_start(text, caret)))
        }
    }
}

/// `Ctrl+/`: comment the lines out with `-- ` at their least indentation, or,
/// when every non-blank one already starts with `--`, take that off. Blank
/// lines are left alone either way.
pub(crate) fn toggle_comment(text: &mut String, sel: Sel) -> Sel {
    let lines = lines_of(text, sel);
    let body = |text: &str, s: usize| text[s..line_end(text, s)].to_owned();
    let filled: Vec<usize> = starts(text, lines.clone())
        .into_iter()
        .filter(|s| !body(text, *s).trim().is_empty())
        .collect();
    if filled.is_empty() {
        return sel;
    }
    // Past the same indent the removal measures, so the mark is where it looks.
    let commented = filled.iter().all(|s| {
        let line = body(text, *s);
        line[indent_of(&line).len()..].starts_with("--")
    });
    let column = filled
        .iter()
        .map(|s| indent_of(&body(text, *s)).len())
        .min()
        .unwrap_or(0);
    let mut delta: isize = 0;
    let mut caret_delta: isize = 0;
    for s in filled {
        let at = s.saturating_add_signed(delta);
        let change: isize = match commented {
            true => {
                let indent = indent_of(&body(text, at)).len();
                let mark = at + indent;
                let len = match text[mark..].starts_with("-- ") {
                    true => 3,
                    false => 2,
                };
                text.replace_range(mark..mark + len, "");
                // A caret at or inside the mark lands where the mark was.
                let mark_was = s + indent;
                if sel.caret > mark_was {
                    caret_delta -= len.min(sel.caret - mark_was) as isize;
                }
                -(len as isize)
            }
            false => {
                text.insert_str(at + column, "-- ");
                if sel.caret >= s + column {
                    caret_delta += 3;
                }
                3
            }
        };
        delta += change;
    }
    match sel.range().is_empty() {
        true => sel.shifted(caret_delta),
        false => sel.over(lines.start..lines.end.saturating_add_signed(delta)),
    }
}

/// `Alt+↑` / `Alt+↓`: the selected lines swap with the line above or below,
/// the selection moving with them. `None` at the text's first or last line.
pub(crate) fn move_lines(text: &mut String, sel: Sel, up: bool) -> Option<Sel> {
    let lines = lines_of(text, sel);
    let block = text[lines.clone()].to_owned();
    if up {
        if lines.start == 0 {
            return None;
        }
        let above = line_start(text, lines.start - 1);
        let other = text[above..lines.start - 1].to_owned();
        text.replace_range(above..lines.end, &format!("{block}\n{other}"));
        Some(sel.shifted(-((lines.start - above) as isize)))
    } else {
        if lines.end == text.len() {
            return None;
        }
        let below = line_end(text, lines.end + 1);
        let other = text[lines.end + 1..below].to_owned();
        text.replace_range(lines.start..below, &format!("{other}\n{block}"));
        Some(sel.shifted((other.len() + 1) as isize))
    }
}

/// `Shift+Alt+↓` / `↑`: a copy of the selected lines below or above them, the
/// selection on the copy.
pub(crate) fn duplicate_lines(text: &mut String, sel: Sel, up: bool) -> Sel {
    let lines = lines_of(text, sel);
    let block = text[lines.clone()].to_owned();
    match up {
        true => {
            text.insert_str(lines.start, &format!("{block}\n"));
            sel
        }
        false => {
            text.insert_str(lines.end, &format!("\n{block}"));
            sel.shifted((block.len() + 1) as isize)
        }
    }
}

/// `Ctrl+X` with nothing selected: the caret's whole line taken out, its
/// newline with it, the caret at the start of the line that took its place.
/// Answers the caret and the text taken, ending in a newline, for the clipboard.
pub(crate) fn cut_line(text: &mut String, sel: Sel) -> (Sel, String) {
    let start = line_start(text, sel.caret);
    let end = line_end(text, sel.caret);
    let line = format!("{}\n", &text[start..end]);
    let cut = match (end < text.len(), start > 0) {
        // Its own newline goes with it.
        (true, _) => start..end + 1,
        // The last line takes the newline before it instead.
        (false, true) => start - 1..end,
        (false, false) => start..end,
    };
    let at = cut.start.min(start);
    text.replace_range(cut, "");
    (Sel::caret(line_start(text, at.min(text.len()))), line)
}

/// The close of an open bracket or quote.
fn close_of(c: char) -> Option<char> {
    match c {
        '(' => Some(')'),
        '[' => Some(']'),
        '{' => Some('}'),
        '"' => Some('"'),
        '\'' => Some('\''),
        _ => None,
    }
}

/// Whether the caret at `at` sits in a string or a comment on its line: an
/// odd count of a quote before it, or a `--` outside quotes.
fn in_string_or_comment(text: &str, at: usize) -> bool {
    caret_in(text, at) != In::Code
}

/// What the caret at `at` sits in on its line.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum In {
    Code,
    /// An open `"`, `'` or `` ` `` before it, a backslash escaping the next character.
    String,
    /// A `--` outside quotes before it.
    Comment,
}

/// [`In`] for the caret at `at` in `text`.
pub(crate) fn caret_in(text: &str, at: usize) -> In {
    let mut open: Option<char> = None;
    let mut escaped = false;
    let mut prev = '\0';
    for c in text[line_start(text, at)..at].chars() {
        match open {
            Some(_) if escaped => escaped = false,
            Some(_) if c == '\\' => escaped = true,
            Some(q) if c == q => open = None,
            Some(_) => {}
            None if c == '-' && prev == '-' => return In::Comment,
            None if matches!(c, '"' | '\'' | '`') => open = Some(c),
            None => {}
        }
        prev = c;
    }
    match open {
        Some(_) => In::String,
        None => In::Code,
    }
}

/// Typing `c` where pairs help, `None` where the text edit should type it as
/// it would. With a selection, a bracket or quote wraps it. With none: an open
/// bracket or quote types its close too when what follows is a space, a close
/// or the line's end (a quote also not after a name, nor in a string or
/// comment); a close or quote typed just before the same character steps over it.
pub(crate) fn type_char(text: &mut String, sel: Sel, c: char) -> Option<Sel> {
    let r = sel.range();
    if !r.is_empty() {
        let close = close_of(c)?;
        if text[r.clone()].contains('\n') && matches!(c, '"' | '\'') {
            return None;
        }
        text.insert(r.end, close);
        text.insert(r.start, c);
        let inner = r.start + c.len_utf8()..r.end + c.len_utf8();
        return Some(sel.over(inner));
    }
    let at = r.start;
    let next = text[at..].chars().next();
    let prev = text[..at].chars().next_back();
    if matches!(c, ')' | ']' | '}' | '"' | '\'') && next == Some(c) {
        return Some(Sel::caret(at + c.len_utf8()));
    }
    let close = close_of(c)?;
    let room = next.is_none_or(|n| n.is_whitespace() || matches!(n, ')' | ']' | '}' | ',' | ';'));
    let quote = matches!(c, '"' | '\'');
    if !room || (quote && (prev.is_some_and(word_char) || in_string_or_comment(text, at))) {
        return None;
    }
    text.insert(at, close);
    text.insert(at, c);
    Some(Sel::caret(at + c.len_utf8()))
}

/// `Backspace` between an empty pair, `()` or `""`, takes both; `None`
/// elsewhere, for the text edit's own.
pub(crate) fn backspace_pair(text: &mut String, sel: Sel) -> Option<Sel> {
    if sel.anchor != sel.caret {
        return None;
    }
    let at = sel.caret;
    let prev = text[..at].chars().next_back()?;
    let next = text[at..].chars().next()?;
    if close_of(prev) != Some(next) {
        return None;
    }
    text.replace_range(at - prev.len_utf8()..at + next.len_utf8(), "");
    Some(Sel::caret(at - prev.len_utf8()))
}

/// After a letter or a close is typed: when the caret's line now reads only
/// `end`, `else`, `elseif`, `until`, `}`, `)` or `]`, step it back to the
/// depth its block opened at — the line above's, or one shallower when that
/// line is a body line. Only ever shallower, so it is safe to run again.
pub(crate) fn dedent_closer(text: &mut String, caret: usize) -> Option<Sel> {
    let start = line_start(text, caret);
    if line_end(text, caret) != caret {
        return None;
    }
    let line = &text[start..caret];
    let word = line.trim_start_matches([' ', '\t']);
    if !matches!(word, "end" | "else" | "elseif" | "until" | "}" | ")" | "]") {
        return None;
    }
    let above = text[..start.saturating_sub(1)]
        .rsplit('\n')
        .find(|l| !l.trim().is_empty())?;
    let want = match opens_block(above) {
        true => indent_of(above),
        false => shallower(indent_of(above)),
    }
    .to_owned();
    let have = indent_of(line).len();
    if want.len() >= have {
        return None;
    }
    text.replace_range(start..start + have, &want);
    Some(Sel::caret(caret - have + want.len()))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// `Ctrl+X` on no selection takes the caret's line and its newline; the
    /// last line takes the newline before it.
    /// flip: `cut_line` keeping the newline → fails at "a middle line".
    #[test]
    fn cut_takes_the_caret_line_whole() {
        let mut t = "a = 1\nb = 2\nc = 3".to_owned();
        let (sel, cut) = cut_line(&mut t, Sel::caret(8));
        assert_eq!(
            (t.as_str(), cut.as_str()),
            ("a = 1\nc = 3", "b = 2\n"),
            "a middle line"
        );
        assert_eq!(sel, Sel::caret(6));
        let end = t.len();
        let (sel, cut) = cut_line(&mut t, Sel::caret(end));
        assert_eq!(
            (t.as_str(), cut.as_str()),
            ("a = 1", "c = 3\n"),
            "the last line"
        );
        assert_eq!(sel, Sel::caret(0));
        let (_, cut) = cut_line(&mut t, Sel::caret(2));
        assert_eq!((t.as_str(), cut.as_str()), ("", "a = 1\n"), "the only line");
    }

    /// `text` with `|` marking the caret, or `[` and `]` a selection whose
    /// caret is at `]`, as the text and the selection.
    fn at(marked: &str) -> (String, Sel) {
        if let Some(c) = marked.find('|') {
            return (marked.replacen('|', "", 1), Sel::caret(c));
        }
        let a = marked.find('[').expect("a caret or a selection");
        let text = marked.replacen('[', "", 1);
        let c = text.find(']').expect("the selection's end");
        (
            text.replacen(']', "", 1),
            Sel {
                anchor: a,
                caret: c,
            },
        )
    }

    /// `text` with `sel` marked as `at` reads it.
    fn mark(text: &str, sel: Sel) -> String {
        let mut out = text.to_owned();
        match sel.anchor == sel.caret {
            true => out.insert(sel.caret, '|'),
            false => {
                let (a, c) = (sel.anchor, sel.caret);
                out.insert(a.max(c), if c > a { ']' } else { '[' });
                out.insert(a.min(c), if c > a { '[' } else { ']' });
            }
        }
        out
    }

    fn run(marked: &str, f: impl FnOnce(&mut String, Sel) -> Sel) -> String {
        let (mut text, sel) = at(marked);
        let sel = f(&mut text, sel);
        mark(&text, sel)
    }

    /// `Enter` keeps the line's indentation, goes a step in after a block's
    /// opening, and puts a bracket's close on its own line.
    /// flip: `newline` ignoring `opens_block` → fails at "after then".
    #[test]
    fn enter_indents_as_the_line_and_its_block() {
        assert_eq!(run("  x = 1|", newline), "  x = 1\n  |");
        assert_eq!(run("if a then|", newline), "if a then\n  |", "after then");
        assert_eq!(
            run("  for _, l in ipairs(sel) do|", newline),
            "  for _, l in ipairs(sel) do\n    |"
        );
        assert_eq!(
            run("local f = function(a)|", newline),
            "local f = function(a)\n  |"
        );
        assert_eq!(
            run("print(a)|", newline),
            "print(a)\n|",
            "a call opens nothing"
        );
        assert_eq!(
            run("f(function() return 1 end)|", newline),
            "f(function() return 1 end)\n|"
        );
        assert_eq!(run("undo|", newline), "undo\n|", "`do` as a word only");
        assert_eq!(run("t = {|}", newline), "t = {\n  |\n}");
        assert_eq!(run("\tx|", newline), "\tx\n\t|", "tabs stay tabs");
        assert_eq!(run("a [b] c", newline), "a \n| c");
    }

    /// Typing a block's closing word steps the line back to its block's depth,
    /// once, and never deeper.
    /// flip: `dedent_closer` taking the above line's depth whatever it is →
    /// fails at "under a body line".
    #[test]
    fn a_closing_word_steps_back_to_its_block() {
        let dedent = |marked: &str| {
            let (mut text, sel) = at(marked);
            let sel = dedent_closer(&mut text, sel.caret).unwrap_or(sel);
            mark(&text, sel)
        };
        assert_eq!(
            dedent("if a then\n  x = 1\n  end|"),
            "if a then\n  x = 1\nend|",
            "under a body line"
        );
        assert_eq!(
            dedent("if a then\n  end|"),
            "if a then\nend|",
            "under the opening"
        );
        assert_eq!(
            dedent("if a then\n  x = 1\nelse|"),
            "if a then\n  x = 1\nelse|",
            "never deeper"
        );
        assert_eq!(
            dedent("if a then\n  x = 1\nelseif|"),
            "if a then\n  x = 1\nelseif|",
            "elseif after else: once"
        );
        assert_eq!(dedent("f(\n  a,\n  )|"), "f(\n  a,\n)|");
        assert_eq!(
            dedent("if a then\n  x = 1\n  ending|"),
            "if a then\n  x = 1\n  ending|",
            "a longer word"
        );
        assert_eq!(
            dedent("if a then\n  x = 1\n  end| -- c"),
            "if a then\n  x = 1\n  end| -- c",
            "caret at the line's end only"
        );
    }

    /// `Tab` and `Shift+Tab` indent and outdent every line a selection touches,
    /// keeping them selected; with no selection `Tab` pads to the next step.
    /// flip: `lines_of` keeping a line the selection only reaches the start of
    /// → fails at "not the line it stops at".
    #[test]
    fn tab_and_shift_tab_work_on_lines() {
        assert_eq!(run("x|", indent), "x |", "to the next step");
        assert_eq!(run("|x", indent), "  |x");
        assert_eq!(run("[a\nb]", indent), "[  a\n  b]");
        assert_eq!(
            run("[a\n\nb]", indent),
            "[  a\n\n  b]",
            "a blank line stays blank"
        );
        assert_eq!(
            run("[a\nb\n]c", indent),
            "[  a\n  b]\nc",
            "not the line it stops at"
        );
        assert_eq!(run("[  a\n    b]", outdent), "[a\n  b]");
        assert_eq!(run("    x|", outdent), "  x|", "a caret stays in its text");
        assert_eq!(run("\t\tx|", outdent), "\tx|", "a tab is a step");
        assert_eq!(run(" |x", outdent), "|x");
    }

    /// `Ctrl+/` comments lines at their least indentation and uncomments them
    /// when all are comments, leaving blank lines alone.
    #[test]
    fn ctrl_slash_toggles_comments() {
        assert_eq!(run("[  a\n    b]", toggle_comment), "[  -- a\n  --   b]");
        assert_eq!(run("[  -- a\n  --   b]", toggle_comment), "[  a\n    b]");
        assert_eq!(run("[a\n\nb]", toggle_comment), "[-- a\n\n-- b]");
        assert_eq!(run("x = 1|", toggle_comment), "-- x = 1|");
        assert_eq!(run("--x|", toggle_comment), "x|");
        assert_eq!(
            run("[-- a\nb]", toggle_comment),
            "[-- -- a\n-- b]",
            "mixed: comment all"
        );
    }

    /// Uncommenting keeps a caret at or inside the mark on its own line, and a
    /// line led by other whitespace is not read as commented.
    /// flip: `toggle_comment` shifting the caret by the whole mark from the line's
    /// indent → the first case's caret lands inside `→` and panics.
    #[test]
    fn uncommenting_keeps_the_caret_on_its_line() {
        assert_eq!(
            run("x→\n|-- y", toggle_comment),
            "x→\n|y",
            "a caret before the mark"
        );
        assert_eq!(
            run("x→\n-|- y", toggle_comment),
            "x→\n|y",
            "inside the mark"
        );
        assert_eq!(run("  -- y|", toggle_comment), "  y|", "after it");
        assert_eq!(
            run("\u{3000}-- x|", toggle_comment),
            "-- \u{3000}-- x|",
            "a full-width space is no indent"
        );
        assert_eq!(run("\u{c}-- x|", toggle_comment), "-- \u{c}-- x|");
    }

    /// Lines move and copy with their selection, and stop at the text's ends.
    #[test]
    fn lines_move_and_copy() {
        let mv = |up: bool| move |t: &mut String, s: Sel| move_lines(t, s, up).unwrap_or(s);
        assert_eq!(run("a\nb|", mv(true)), "b|\na");
        assert_eq!(run("a|\nb", mv(false)), "b\na|");
        assert_eq!(run("a|\nb", mv(true)), "a|\nb", "the first line stays");
        assert_eq!(run("x\n[a\nb]\ny", mv(true)), "[a\nb]\nx\ny");
        let dup = |up: bool| move |t: &mut String, s: Sel| duplicate_lines(t, s, up);
        assert_eq!(run("a|\nb", dup(false)), "a\na|\nb");
        assert_eq!(run("a|\nb", dup(true)), "a|\na\nb");
    }

    /// Brackets and quotes go in pairs where a pair can stand, step over their
    /// own close, wrap a selection, and come out in pairs.
    /// flip: `type_char` pairing a quote after a name → fails at "an apostrophe".
    #[test]
    fn brackets_and_quotes_pair() {
        let ty = |c: char| move |t: &mut String, s: Sel| type_char(t, s, c).unwrap_or(s);
        assert_eq!(run("f|", ty('(')), "f(|)");
        assert_eq!(run("f(|)", ty(')')), "f()|", "over its close");
        assert_eq!(run("|x", ty('(')), "|x", "not before a name");
        assert_eq!(run("print(|)", ty('"')), "print(\"|\")");
        assert_eq!(run("x = \"|\"", ty('"')), "x = \"\"|");
        assert_eq!(run("it|", ty('\'')), "it|", "an apostrophe");
        assert_eq!(run("-- a |", ty('"')), "-- a |", "in a comment");
        assert_eq!(run("[sel]", ty('(')), "([sel])");
        let bs = |t: &mut String, s: Sel| backspace_pair(t, s).unwrap_or(s);
        assert_eq!(run("f(|)", bs), "f|");
        assert_eq!(run("f(x|)", bs), "f(x|)", "not a pair");
    }
}
