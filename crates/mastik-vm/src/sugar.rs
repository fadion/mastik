//! Tier 1: the command bar's line sugar, a handful of textual rewrites that
//! produce Luau. The bar shows the Luau it produced, so a terse line teaches the
//! language rather than standing beside it. A script gets none of this.

use crate::lex::{self, Lexeme, Lx};

/// A bar line rewritten into Luau.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Line {
    /// The Luau that runs.
    pub luau: String,
    pub kind: LineKind,
    /// Whether a compound assignment became a per-layer adjust (rule 3), which
    /// the bar's footer says as "each by itself".
    pub each_by_itself: bool,
}

/// Whether a line only reads or may change something.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LineKind {
    /// No assignment and no verb call: the bar can show its value as it is typed.
    Read,
    /// An assignment, a verb call or a method that writes.
    Write,
}

/// The compound assignment operators rule 3 rewrites.
const COMPOUND: &[&str] = &["+=", "-=", "*=", "/=", "//=", "%=", "^="];

/// The methods of a collection that change something.
pub(crate) const WRITING_METHODS: &[&str] = &["adjust", "select"];

/// Rewrite one bar line into Luau: rule 1 (a verb and bare words is a call), else
/// rule 2 (a leading `.` is `sel.`), rule 3 (a compound assignment adjusts each
/// layer by itself) and a size's `50%` as `percent(50)`. Anything else is verbatim.
pub fn desugar(line: &str) -> Line {
    let (luau, each_by_itself) = match verb_call(line) {
        Some(call) => (call, false),
        None => {
            let elided = elide(line);
            let (adjusted, each) = compound(&elided);
            (percent(&adjusted), each)
        }
    };
    let kind = classify(&luau);
    Line {
        luau,
        kind,
        each_by_itself,
    }
}

/// Rule 1: `align left` as `align("left")` when the first word names a verb and
/// whitespace or the end follows it. Numbers, `true`, `false`, `nil` and quoted
/// strings stay themselves; every other word becomes a string.
fn verb_call(line: &str) -> Option<String> {
    let text = line.trim();
    let end = text
        .find(|c: char| !lex::ident_char(c))
        .unwrap_or(text.len());
    let name = &text[..end];
    if name.is_empty()
        || !text.starts_with(lex::ident_start)
        || mastik_core::verbs::find(name).is_none()
    {
        return None;
    }
    let rest = &text[end..];
    if !(rest.is_empty() || rest.starts_with(char::is_whitespace)) {
        return None;
    }
    let rest = rest.trim_start();
    // `group = 3` is an assignment and `align == x` a comparison, not arguments.
    if rest.starts_with('=') || COMPOUND.iter().any(|op| rest.starts_with(op)) {
        return None;
    }
    let args: Vec<String> = words(rest)
        .into_iter()
        .map(|w| {
            if w.starts_with(['"', '\''])
                || lex::is_number(w)
                || matches!(w, "true" | "false" | "nil")
            {
                w.to_owned()
            } else {
                lua_string(w)
            }
        })
        .collect();
    Some(format!("{name}({})", args.join(", ")))
}

/// `text` split at whitespace, a quoted string kept whole with its quotes.
fn words(text: &str) -> Vec<&str> {
    let b = text.as_bytes();
    let mut out = Vec::new();
    let mut i = 0;
    while i < b.len() {
        if b[i].is_ascii_whitespace() {
            i += 1;
            continue;
        }
        let start = i;
        if matches!(b[i], b'"' | b'\'') {
            let quote = b[i];
            i += 1;
            while i < b.len() && b[i] != quote {
                i += if b[i] == b'\\' { 2 } else { 1 };
            }
            i = (i + 1).min(b.len());
        } else {
            while i < b.len() && !b[i].is_ascii_whitespace() {
                i += 1;
            }
        }
        out.push(&text[start..i]);
    }
    out
}

/// `s` as a double-quoted Luau string literal.
pub(crate) fn lua_string(s: &str) -> String {
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

/// Whether the lexeme before a `.` puts that `.` in prefix position: the line's
/// start, an operator or an opening bracket or comma, or a keyword that a value follows.
fn prefix_after(prev: Option<&Lexeme>, src: &str) -> bool {
    let Some(prev) = prev else { return true };
    let text = &src[prev.range.clone()];
    match prev.kind {
        Lx::Punct => !matches!(text, ")" | "]" | "}" | "." | "..." | ":"),
        Lx::Keyword => !matches!(text, "end" | "true" | "false" | "nil"),
        _ => false,
    }
}

/// Rule 2: a `.name` in prefix position as `sel.name`.
fn elide(line: &str) -> String {
    let lexemes = lex::lex(line);
    let mut out = String::with_capacity(line.len() + 8);
    let mut copied = 0;
    for (i, l) in lexemes.iter().enumerate() {
        let dot = l.kind == Lx::Punct && &line[l.range.clone()] == ".";
        let named = lexemes
            .get(i + 1)
            .is_some_and(|n| n.kind == Lx::Ident && n.range.start == l.range.end);
        if dot && named && prefix_after(i.checked_sub(1).map(|p| &lexemes[p]), line) {
            out.push_str(&line[copied..l.range.start]);
            out.push_str("sel");
            copied = l.range.start;
        }
    }
    out.push_str(&line[copied..]);
    out
}

/// Rule 3: `<expr>.<prop> op= <rhs>` as an adjust of each layer by itself —
/// `+=` and `-=` by an amount, the others by a function of the layer's own value.
/// Answers the line and whether it was rewritten.
fn compound(line: &str) -> (String, bool) {
    let lexemes = lex::lex(line);
    let mut depth = 0i32;
    let mut at = None;
    for (i, l) in lexemes.iter().enumerate() {
        if l.kind != Lx::Punct {
            continue;
        }
        match &line[l.range.clone()] {
            "(" | "[" | "{" => depth += 1,
            ")" | "]" | "}" => depth -= 1,
            op if depth == 0 && COMPOUND.contains(&op) => {
                at = Some(i);
                break;
            }
            _ => {}
        }
    }
    let Some(i) = at else {
        return (line.to_owned(), false);
    };
    if i < 3 {
        return (line.to_owned(), false);
    }
    let (prop, dot) = (&lexemes[i - 1], &lexemes[i - 2]);
    if prop.kind != Lx::Ident || dot.kind != Lx::Punct || &line[dot.range.clone()] != "." {
        return (line.to_owned(), false);
    }
    let subject = line[..dot.range.start].trim();
    let name = &line[prop.range.clone()];
    let op = &line[lexemes[i].range.clone()];
    let rhs = line[lexemes[i].range.end..].trim();
    if subject.is_empty() || rhs.is_empty() {
        return (line.to_owned(), false);
    }
    let by = match op {
        "+=" => rhs.to_owned(),
        "-=" if lex::is_number(rhs) && !rhs.starts_with('-') => format!("-{rhs}"),
        "-=" => format!("-({rhs})"),
        _ => {
            let bare = &op[..op.len() - 1];
            format!("function(v) return v {bare} ({rhs}) end")
        }
    };
    (
        format!("{subject}:adjust({}, {by})", lua_string(name)),
        true,
    )
}

/// A size's `50%` as `percent(50)`: a number with `%` right after it and nothing
/// to take the remainder of, which Luau's `%` would otherwise need.
fn percent(line: &str) -> String {
    let lexemes = lex::lex(line);
    let mut out = String::with_capacity(line.len() + 16);
    let mut copied = 0;
    for (i, l) in lexemes.iter().enumerate() {
        if l.kind != Lx::Number {
            continue;
        }
        let Some(sign) = lexemes.get(i + 1) else {
            continue;
        };
        let is_percent = sign.kind == Lx::Punct
            && &line[sign.range.clone()] == "%"
            && sign.range.start == l.range.end;
        let ends = lexemes.get(i + 2).is_none_or(|n| {
            n.kind == Lx::Punct && matches!(&line[n.range.clone()], ")" | "," | "}" | "]" | ";")
        });
        if is_percent && ends {
            out.push_str(&line[copied..l.range.start]);
            out.push_str(&format!("percent({})", &line[l.range.clone()]));
            copied = sign.range.end;
        }
    }
    out.push_str(&line[copied..]);
    out
}

/// Whether `luau` only reads: no assignment, no verb call, no writing method.
pub(crate) fn classify(luau: &str) -> LineKind {
    let lexemes = lex::lex(luau);
    let text = |l: &Lexeme| &luau[l.range.clone()];
    for (i, l) in lexemes.iter().enumerate() {
        let after = |d: usize| lexemes.get(i + d).map(text);
        let before = i.checked_sub(1).map(|p| text(&lexemes[p]));
        match l.kind {
            Lx::Punct if text(l) == "=" || COMPOUND.contains(&text(l)) => {
                return LineKind::Write;
            }
            Lx::Punct if text(l) == ":" => {
                if after(1).is_some_and(|m| WRITING_METHODS.contains(&m)) {
                    return LineKind::Write;
                }
            }
            Lx::Ident
                if mastik_core::verbs::find(text(l)).is_some()
                    && !matches!(before, Some("." | ":"))
                    && after(1)
                        .is_some_and(|n| n == "(" || n == "{" || n.starts_with(['"', '\''])) =>
            {
                return LineKind::Write;
            }
            _ => {}
        }
    }
    LineKind::Read
}
