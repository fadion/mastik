//! Completion for the bar and the editor: Mastik's own names only — verbs,
//! properties, the language's names, leap labels and the page's layers — read
//! from the tables the runtime binds, so the two cannot drift.

use std::ops::Range;

use mastik_core::verbs::{self, ParamKind};
use mastik_core::{Document, NodeId};
use rustc_hash::FxHashMap;

use crate::lex;
use crate::props::{METHODS, PROPERTIES};

/// Which list a completion row belongs to, in the order they are shown when
/// their best matches are alike (`by_best_section`).
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum Section {
    /// The language's own names: `sel`, `parent`, `param`…
    Names,
    /// A script's own: its locals, functions, parameters and loop variables,
    /// and its header's inputs after `param.`.
    Script,
    Verbs,
    /// A collection's properties and methods, after `.` or `:`.
    Properties,
    Labels,
    Layers,
    /// The words a verb's next argument takes.
    Choices,
    /// Luau's keywords and the permitted standard library, and a library's
    /// members after `math.`, `string.` or `table.`.
    Luau,
}

/// One completion row.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Row {
    pub section: Section,
    /// What Enter writes over [`Completion::replace`].
    pub label: String,
    /// A line on what it is.
    pub detail: String,
    /// The byte ranges of `label` the typed word matched, to draw bold.
    pub matched: Vec<Range<usize>>,
    /// An almost-match drawn dim: the typed word is no name, and `suggestion` is
    /// what it most likely meant.
    pub dim: bool,
    pub suggestion: Option<String>,
}

/// What to offer at a cursor: the range a pick replaces, and the rows.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Completion {
    pub replace: Range<usize>,
    pub rows: Vec<Row>,
}

/// How well `word` matches `name`: the whole name 0, a prefix 1, the start of a
/// `_`-separated word 2, a subsequence 3, with the matched byte ranges; `None`
/// when it does not.
#[expect(
    clippy::single_range_in_vec_init,
    reason = "a list of matched ranges that holds one"
)]
fn score(word: &str, name: &str) -> Option<(u8, Vec<Range<usize>>)> {
    if word.is_empty() {
        return Some((1, Vec::new()));
    }
    let (w, n) = (word.to_ascii_lowercase(), name.to_ascii_lowercase());
    if n == w {
        return Some((0, vec![0..w.len()]));
    }
    if n.starts_with(&w) {
        return Some((1, vec![0..w.len()]));
    }
    let starts = n
        .match_indices('_')
        .map(|(i, _)| i + 1)
        .filter(|s| n[*s..].starts_with(&w));
    if let Some(s) = starts.into_iter().next() {
        return Some((2, vec![s..s + w.len()]));
    }
    let mut ranges: Vec<Range<usize>> = Vec::new();
    let mut from = 0;
    for c in w.chars() {
        let at = from + n[from..].find(c)?;
        let end = at + c.len_utf8();
        match ranges.last_mut() {
            Some(r) if r.end == at => r.end = end,
            _ => ranges.push(at..end),
        }
        from = end;
    }
    Some((3, ranges))
}

/// The identifier `text` ends with, empty when it ends with none.
fn word_before(text: &str) -> &str {
    let from = text
        .char_indices()
        .rev()
        .take_while(|(_, c)| lex::ident_char(*c))
        .last()
        .map_or(text.len(), |(i, _)| i);
    &text[from..]
}

/// The document's tables `word` matches, each with its size (*12 rows · 4
/// columns*): after `tables.`, and for a verb's `table` argument.
fn table_names(doc: &Document, word: &str) -> Vec<Row> {
    let count = |n: usize, one: &str| format!("{n} {one}{}", if n == 1 { "" } else { "s" });
    rank(
        Section::Choices,
        word,
        doc.data_sets().iter().map(|d| {
            let size = format!(
                "{} \u{b7} {}",
                count(d.table.rows.len(), "row"),
                count(d.table.columns.len(), "column")
            );
            (d.name.as_str(), size)
        }),
    )
}

/// Rows of `section` for every `(name, detail)` `word` matches, best first.
fn rank<'a>(
    section: Section,
    word: &str,
    candidates: impl IntoIterator<Item = (&'a str, String)>,
) -> Vec<Row> {
    ranked(section, word, candidates)
        .into_iter()
        .map(|(_, row)| row)
        .collect()
}

/// [`rank`] with each row's [`score`].
fn ranked<'a>(
    section: Section,
    word: &str,
    candidates: impl IntoIterator<Item = (&'a str, String)>,
) -> Vec<(u8, Row)> {
    let mut scored: Vec<(u8, Row)> = candidates
        .into_iter()
        .filter_map(|(name, detail)| {
            let (r, matched) = score(word, name)?;
            Some((
                r,
                Row {
                    section,
                    label: name.to_owned(),
                    detail,
                    matched,
                    dim: false,
                    suggestion: None,
                },
            ))
        })
        .collect();
    scored.sort_by(|(ra, a), (rb, b)| {
        ra.cmp(rb)
            .then(a.label.len().cmp(&b.label.len()))
            .then(a.label.cmp(&b.label))
    });
    scored
}

/// Completion for `line` with the cursor at byte `cursor`: one verb's next
/// argument once a verb and a space are typed, a property after `.`, a method
/// after `:`, else the names a line can start with.
pub fn complete(
    doc: &Document,
    page: NodeId,
    labels: &FxHashMap<String, NodeId>,
    line: &str,
    cursor: usize,
) -> Completion {
    let mut cursor = cursor.min(line.len());
    while !line.is_char_boundary(cursor) {
        cursor -= 1;
    }
    let bytes = line.as_bytes();
    let mut start = cursor;
    while start > 0 && lex::ident_char(bytes[start - 1] as char) {
        start -= 1;
    }
    let mut end = cursor;
    while end < bytes.len() && lex::ident_char(bytes[end] as char) {
        end += 1;
    }
    let word = &line[start..cursor];
    let replace = start..end;
    if let Some(rows) = verb_argument(doc, line, start, word) {
        return Completion { replace, rows };
    }
    let before = line[..start].chars().next_back();
    let rows = match before {
        Some('.') if word_before(&line[..start - 1]) == "tables" => table_names(doc, word),
        Some('.') if !line[..start].ends_with("..") => properties(word),
        Some(':') => rank(
            Section::Properties,
            word,
            METHODS.iter().map(|m| (m.name, m.doc.to_owned())),
        ),
        _ => {
            let mut keys: Vec<(&String, &NodeId)> = labels.iter().collect();
            keys.sort();
            let layers = crate::layer_names(doc, page);
            by_best_section(vec![
                ranked(
                    Section::Names,
                    word,
                    crate::run::NAMES.iter().map(|(n, d)| (*n, (*d).to_owned())),
                ),
                ranked(
                    Section::Verbs,
                    word,
                    verbs::VERBS.iter().map(|v| (v.name, v.doc.to_owned())),
                ),
                ranked(
                    Section::Labels,
                    word,
                    keys.into_iter()
                        .map(|(k, id)| (k.as_str(), crate::describe(doc, &[*id]))),
                ),
                ranked(
                    Section::Layers,
                    word,
                    layers.iter().map(|n| (n.as_str(), "layer".to_owned())),
                ),
            ])
        }
    };
    Completion { replace, rows }
}

/// Ranked sections as one list, kept whole and in their order except that a
/// section with a better best match goes first: a word typed out whole (a
/// label, `sel`) leads the list rather than sitting under every verb that
/// merely starts with it.
fn by_best_section(mut sections: Vec<Vec<(u8, Row)>>) -> Vec<Row> {
    sections.sort_by_key(|rows| rows.first().map_or(u8::MAX, |(s, _)| *s));
    sections.into_iter().flatten().map(|(_, row)| row).collect()
}

/// The property rows for `word`, an almost-match (`.alpha`) first as a dim row
/// naming what it most likely meant.
fn properties(word: &str) -> Vec<Row> {
    let mut rows = rank(
        Section::Properties,
        word,
        PROPERTIES.iter().map(|p| (p.name, p.doc.to_owned())),
    );
    let exact = PROPERTIES.iter().any(|p| p.name == word);
    let prefixed = rows
        .iter()
        .any(|r| r.matched.first().is_some_and(|m| m.start == 0));
    if !word.is_empty()
        && !exact
        && !prefixed
        && let Some(meant) = crate::run::nearest_property(word)
    {
        rows.insert(
            0,
            Row {
                section: Section::Properties,
                label: word.to_owned(),
                detail: format!("No property “{word}” — did you mean {meant}?"),
                matched: Vec::new(),
                dim: true,
                suggestion: Some(meant),
            },
        );
    }
    rows
}

/// The choices of a verb's next argument when `line` is a verb then bare words
/// (rule 1) and the word being typed, starting at `start`, is an argument.
/// `None` when the line is not such a call; no rows when the argument takes no
/// fixed words. A `table` argument takes the document's table names.
fn verb_argument(doc: &Document, line: &str, start: usize, word: &str) -> Option<Vec<Row>> {
    let lead = line.len() - line.trim_start().len();
    let text = &line[lead..];
    let name_len = text
        .find(|c: char| !lex::ident_char(c))
        .unwrap_or(text.len());
    let verb = verbs::find(&text[..name_len])?;
    let name_end = lead + name_len;
    if start <= name_end || !line[name_end..].starts_with(char::is_whitespace) {
        return None;
    }
    let before = line[name_end..start].split_whitespace().count();
    let subject = verb
        .params
        .iter()
        .position(|p| matches!(p.kind, ParamKind::Node | ParamKind::Nodes));
    let param = verb
        .params
        .iter()
        .enumerate()
        .filter(|(i, _)| Some(*i) != subject)
        .map(|(_, p)| p)
        .nth(before)?;
    if param.name == "table" {
        return Some(table_names(doc, word));
    }
    let ParamKind::Choice(words) = param.kind else {
        return Some(Vec::new());
    };
    Some(rank(
        Section::Choices,
        word,
        words.iter().map(|w| (*w, param.doc.to_owned())),
    ))
}

/// Completion for a whole script with the cursor at byte `cursor`, as the
/// editor offers it: no rows inside a string or a comment, or where a new name
/// is being declared; after `.` a property, a header input after `param.` and a
/// member after a library's name; after `:` a method; else the script's own
/// names, the language's, the verbs, Luau's and the page's layers. No leap
/// labels, which last a session, and no verb sugar, which is the bar's.
pub fn complete_script(doc: &Document, page: NodeId, source: &str, cursor: usize) -> Completion {
    let mut cursor = cursor.min(source.len());
    while !source.is_char_boundary(cursor) {
        cursor -= 1;
    }
    let bytes = source.as_bytes();
    let mut start = cursor;
    while start > 0 && lex::ident_char(bytes[start - 1] as char) {
        start -= 1;
    }
    let mut end = cursor;
    while end < bytes.len() && lex::ident_char(bytes[end] as char) {
        end += 1;
    }
    let replace = start..end;
    let none = Completion {
        replace: replace.clone(),
        rows: Vec::new(),
    };
    let lexemes = lex::lex(source);
    let quoted = lexemes.iter().find(|l| {
        matches!(l.kind, lex::Lx::String | lex::Lx::Comment)
            && l.range.start < cursor
            && (cursor < l.range.end || (cursor == l.range.end && !closed(source, l)))
    });
    // Inside a choice parameter's quotes, its words with their quotes.
    if let Some(s) = quoted.filter(|l| l.kind == lex::Lx::String) {
        let quote = &source[s.range.start..s.range.start + 1];
        if quote != "["
            && let Some((replace, words)) =
                crate::help::argument_rows(source, cursor, Some(s.range.clone()))
        {
            let typed = &source[s.range.start + 1..cursor];
            let rows = ranked(
                Section::Choices,
                typed,
                words.iter().map(|(w, d)| (&w[1..w.len() - 1], d.clone())),
            )
            .into_iter()
            .map(|(_, mut row)| {
                row.label = format!("{quote}{}{quote}", row.label);
                for m in &mut row.matched {
                    *m = m.start + 1..m.end + 1;
                }
                row
            })
            .collect();
            return Completion { replace, rows };
        }
    }
    // A word starting with a digit is a number being typed.
    if quoted.is_some() || source[start..end].starts_with(|c: char| c.is_ascii_digit()) {
        return none;
    }
    // At a key of a verb's table argument, the names it has left.
    if let Some((replace, names)) = crate::help::argument_rows(source, cursor, None) {
        let word = &source[replace.start..cursor];
        let rows = rank(
            Section::Choices,
            word,
            names.iter().map(|(n, d)| (n.as_str(), d.clone())),
        );
        return Completion { replace, rows };
    }
    let text = |l: &lex::Lexeme| &source[l.range.clone()];
    let before: Vec<&lex::Lexeme> = lexemes
        .iter()
        .take_while(|l| l.range.end <= start)
        .collect();
    if before
        .last()
        .is_some_and(|l| matches!(text(l), "local" | "function" | "for"))
    {
        return none;
    }
    let word = &source[start..cursor];
    let owner = before.len().checked_sub(2).map(|i| text(before[i]));
    let rows = match before.last().map(|l| text(l)) {
        Some(".") if owner == Some("param") => {
            let inputs = crate::header::header(source)
                .map(|h| h.inputs)
                .unwrap_or_default();
            rank(
                Section::Script,
                word,
                inputs
                    .iter()
                    .map(|i| (i.name.as_str(), format!("input · {}", i.ty.spelled()))),
            )
        }
        Some(".") if owner == Some("tables") => table_names(doc, word),
        Some(".") if owner.is_some_and(|o| crate::run::STDLIB.contains(&o)) => {
            let lib = owner.unwrap_or_default();
            rank(
                Section::Luau,
                word,
                crate::run::library(lib).iter().map(|m| {
                    let doc = crate::run::lib_doc(&format!("{lib}.{m}"))
                        .map_or_else(|| format!("{lib} library"), |d| d.doc.to_owned());
                    (*m, doc)
                }),
            )
        }
        Some(".") => properties(word),
        Some(":") => rank(
            Section::Properties,
            word,
            METHODS.iter().map(|m| (m.name, m.doc.to_owned())),
        ),
        _ => {
            let declared = declared(source, &lexemes, &replace);
            let layers = crate::layer_names(doc, page);
            let luau = lex::KEYWORDS
                .iter()
                .map(|k| (*k, "keyword".to_owned()))
                .chain(crate::run::STDLIB.iter().map(|s| {
                    let doc = crate::run::lib_doc(s)
                        .map_or_else(|| "library".to_owned(), |d| d.doc.to_owned());
                    (*s, doc)
                }));
            by_best_section(vec![
                ranked(
                    Section::Script,
                    word,
                    declared.iter().map(|(n, d)| (n.as_str(), (*d).to_owned())),
                ),
                ranked(
                    Section::Names,
                    word,
                    crate::run::NAMES.iter().map(|(n, d)| (*n, (*d).to_owned())),
                ),
                ranked(
                    Section::Verbs,
                    word,
                    verbs::VERBS.iter().map(|v| (v.name, v.doc.to_owned())),
                ),
                ranked(Section::Luau, word, luau),
                ranked(
                    Section::Layers,
                    word,
                    layers.iter().map(|n| (n.as_str(), "layer".to_owned())),
                ),
            ])
        }
    };
    Completion { replace, rows }
}

/// Whether string or comment lexeme `l` ends on a closing mark, so a cursor
/// right after it is outside: a quoted string's quote, a long bracket's `]`.
/// A line comment never does; a cursor at its line's end is still in it.
fn closed(source: &str, l: &lex::Lexeme) -> bool {
    let s = &source[l.range.clone()];
    match s.as_bytes() {
        [b'-', b'-', ..] => s.starts_with("--[") && s.ends_with(']'),
        [q @ (b'"' | b'\'' | b'`'), .., last] => last == q && s.len() > 1,
        _ => s.ends_with(']'),
    }
}

/// The names `source` declares, each once in the order first declared, with
/// what each is: `local` names, functions, a function's parameters and loop
/// variables. The lexeme at `typing` is the word being typed, never a name.
fn declared(
    source: &str,
    lexemes: &[lex::Lexeme],
    typing: &Range<usize>,
) -> Vec<(String, &'static str)> {
    let mut out: Vec<(String, &'static str)> = Vec::new();
    for d in declarations(source, lexemes, typing) {
        let name = &source[lexemes[d.at].range.clone()];
        if !out.iter().any(|(n, _)| n == name) {
            out.push((name.to_owned(), d.what));
        }
    }
    out
}

/// One name a script declares: its lexeme's index and what it is.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct Decl {
    pub at: usize,
    /// "local", "function", "parameter" or "loop variable".
    pub what: &'static str,
}

/// The lexeme past a type annotation starting at `i`: a run of names, `nil`,
/// strings and bracketed parts joined by `|`, `&`, `?`, `->`, `.` and `<…>`; it
/// ends at the first lexeme outside its brackets that continues no type, so a
/// return type stops before the body's first statement.
fn past_type(source: &str, lexemes: &[lex::Lexeme], mut i: usize) -> usize {
    let mut depth = 0usize;
    // Whether a part is still owed: at the start and after a joining operator.
    let mut owed = true;
    while let Some(l) = lexemes.get(i) {
        let t = &source[l.range.clone()];
        let joins = matches!(t, "|" | "&" | "->" | ".");
        if depth == 0 && !owed && !joins && !matches!(t, "?" | "<") {
            return i;
        }
        match t {
            "(" | "{" | "[" | "<" => depth += 1,
            ")" | "}" | "]" | ">" if depth == 0 => return i,
            ")" | "}" | "]" | ">" => {
                depth -= 1;
                owed = depth > 0;
            }
            _ if depth > 0 => {}
            _ if joins => owed = true,
            "?" => {}
            "," | "=" => return i,
            _ if l.kind == lex::Lx::Keyword && t != "nil" => return i,
            _ => owed = false,
        }
        i += 1;
    }
    i
}

/// Whether each of `lexemes` is part of a type: after a declared name's `:`,
/// after a function's `)` and `:` (its return type), after `::`, a function's
/// generic `<…>`, or a whole `type` (or `export type`) declaration. The names in
/// a type are types, not values a run looks up.
pub(crate) fn in_types(source: &str, lexemes: &[lex::Lexeme], decls: &[Decl]) -> Vec<bool> {
    let text = |i: usize| lexemes.get(i).map_or("", |l| &source[l.range.clone()]);
    let ident = |i: usize| lexemes.get(i).is_some_and(|l| l.kind == lex::Lx::Ident);
    let mut out = vec![false; lexemes.len()];
    // Marks from `start` to the end of the type starting at `from`.
    let mut mark_from = |start: usize, from: usize| {
        let end = past_type(source, lexemes, from);
        out[start.min(end)..end].fill(true);
    };
    for d in decls {
        if text(d.at + 1) == ":" {
            mark_from(d.at + 2, d.at + 2);
        }
    }
    for i in 0..lexemes.len() {
        match text(i) {
            "::" => mark_from(i + 1, i + 1),
            "type" if ident(i + 1) && matches!(text(i + 2), "=" | "<") => {
                let mut eq = i + 2;
                if text(eq) == "<" {
                    eq = past_type(source, lexemes, eq);
                }
                if text(eq) == "=" {
                    let start = if text(i.wrapping_sub(1)) == "export" {
                        i - 1
                    } else {
                        i
                    };
                    mark_from(start, eq + 1);
                }
            }
            "function" => {
                let mut name = i + 1;
                while ident(name) && matches!(text(name + 1), "." | ":") {
                    name += 2;
                }
                let generic = if ident(name) { name + 1 } else { name };
                if text(generic) == "<" {
                    mark_from(generic, generic);
                }
                let Some(open) = (i + 1..lexemes.len()).find(|&j| text(j) == "(") else {
                    continue;
                };
                let mut depth = 0usize;
                let close = (open..lexemes.len()).find(|&j| {
                    match text(j) {
                        "(" => depth += 1,
                        ")" => depth -= 1,
                        _ => {}
                    }
                    depth == 0
                });
                if let Some(close) = close
                    && text(close + 1) == ":"
                {
                    mark_from(close + 2, close + 2);
                }
            }
            _ => {}
        }
    }
    out
}

/// Every declaration in `lexemes`, in source order: `local
/// function` and `function` names (a dotted or method name declares nothing),
/// a function's parameters, past any generic `<…>` (`self` is implicit, never listed) and loop
/// variables. The lexeme at `typing` is the word being typed, never a name.
pub(crate) fn declarations(
    source: &str,
    lexemes: &[lex::Lexeme],
    typing: &Range<usize>,
) -> Vec<Decl> {
    let text = |i: usize| lexemes.get(i).map_or("", |l| &source[l.range.clone()]);
    let ident = |i: usize| {
        lexemes
            .get(i)
            .is_some_and(|l| l.kind == lex::Lx::Ident && l.range != *typing)
    };
    let mut out: Vec<Decl> = Vec::new();
    // Names after `at`, separated by commas, as `local a, b` and `for k, v` list
    // them, each perhaps typed: `a: number` skips its type to the next comma.
    let list = |at: usize| {
        let mut names = Vec::new();
        let mut i = at;
        while ident(i) {
            names.push(i);
            i += 1;
            if text(i) == ":" {
                i = past_type(source, lexemes, i + 1);
            }
            if text(i) != "," {
                break;
            }
            i += 1;
        }
        names
    };
    for i in 0..lexemes.len() {
        let mut add = |at: usize, what| out.push(Decl { at, what });
        match text(i) {
            "local" if text(i + 1) == "function" => {
                if ident(i + 2) {
                    add(i + 2, "function");
                }
            }
            "local" => list(i + 1).into_iter().for_each(|n| add(n, "local")),
            "for" => list(i + 1)
                .into_iter()
                .for_each(|n| add(n, "loop variable")),
            "function" => {
                let mut open = i + 1;
                if ident(open) {
                    let first = open;
                    open += 1;
                    while matches!(text(open), "." | ":") && ident(open + 1) {
                        open += 2;
                    }
                    let plain = open == first + 1;
                    if text(open) == "<" {
                        open = past_type(source, lexemes, open);
                    }
                    if plain && text(open) == "(" && text(i.wrapping_sub(1)) != "local" {
                        add(first, "function");
                    }
                } else if text(open) == "<" {
                    open = past_type(source, lexemes, open);
                }
                if text(open) == "(" {
                    list(open + 1).into_iter().for_each(|n| add(n, "parameter"));
                }
            }
            _ => {}
        }
    }
    out
}
