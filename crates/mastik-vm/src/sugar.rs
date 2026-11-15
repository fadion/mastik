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

/// Whether `l` ends the statement before it at bracket depth 0: a `;`, a comment,
/// or a keyword a statement follows (`do`, `then`, `else`, `repeat`, `end`).
fn opens_statement(l: &Lexeme, src: &str) -> bool {
    let t = &src[l.range.clone()];
    match l.kind {
        Lx::Comment => true,
        Lx::Punct => t == ";",
        Lx::Keyword => matches!(t, "do" | "then" | "else" | "repeat" | "end"),
        _ => false,
    }
}

/// Whether `l` ends the expression before it at bracket depth 0: a `;`, a
/// comment, or a keyword that closes a block (`end`, `else`, `elseif`, `until`).
fn closes_expression(l: &Lexeme, src: &str) -> bool {
    let t = &src[l.range.clone()];
    match l.kind {
        Lx::Comment => true,
        Lx::Punct => t == ";",
        Lx::Keyword => matches!(t, "end" | "else" | "elseif" | "until"),
        _ => false,
    }
}

/// Rule 3: `<expr>.<prop> op= <rhs>` as an adjust of each layer by itself —
/// `+=` and `-=` by an amount, the others by a function of the layer's own value.
/// Each statement is rewritten by itself: the subject starts after the statement
/// boundary before it and the amount ends at the one after. Answers the line and
/// whether anything was rewritten.
fn compound(line: &str) -> (String, bool) {
    let lexemes = lex::lex(line);
    let mut depths = Vec::with_capacity(lexemes.len());
    let mut depth = 0i32;
    for l in &lexemes {
        if l.kind == Lx::Punct && matches!(&line[l.range.clone()], ")" | "]" | "}") {
            depth -= 1;
        }
        depths.push(depth);
        if l.kind == Lx::Punct && matches!(&line[l.range.clone()], "(" | "[" | "{") {
            depth += 1;
        }
    }
    let mut out = String::with_capacity(line.len() + 16);
    let mut copied = 0;
    let mut rewrote = false;
    let mut i = 0;
    while i < lexemes.len() {
        let l = &lexemes[i];
        let op = &line[l.range.clone()];
        if l.kind != Lx::Punct || depths[i] != 0 || !COMPOUND.contains(&op) || i < 3 {
            i += 1;
            continue;
        }
        let (prop, dot) = (&lexemes[i - 1], &lexemes[i - 2]);
        if prop.kind != Lx::Ident || dot.kind != Lx::Punct || &line[dot.range.clone()] != "." {
            i += 1;
            continue;
        }
        let start = (0..i - 2)
            .rev()
            .find(|&j| depths[j] == 0 && opens_statement(&lexemes[j], line))
            .map_or(0, |j| lexemes[j].range.end)
            .max(copied);
        let end_at = (i + 1..lexemes.len())
            .find(|&j| depths[j] == 0 && closes_expression(&lexemes[j], line))
            .unwrap_or(lexemes.len());
        let end = match end_at {
            n if n == i + 1 => l.range.end,
            n => lexemes[n - 1].range.end,
        };
        let subject = line[start..dot.range.start].trim();
        let rhs = line[l.range.end..end].trim();
        if subject.is_empty() || rhs.is_empty() {
            i += 1;
            continue;
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
        let lead = &line[start..dot.range.start];
        let lead_ws = &lead[..lead.len() - lead.trim_start().len()];
        out.push_str(&line[copied..start]);
        out.push_str(lead_ws);
        out.push_str(&format!(
            "{subject}:adjust({}, {by})",
            lua_string(&line[prop.range.clone()])
        ));
        copied = end;
        rewrote = true;
        i = end_at;
    }
    out.push_str(&line[copied..]);
    (out, rewrote)
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
            n.kind == Lx::Comment
                || (n.kind == Lx::Punct
                    && matches!(&line[n.range.clone()], ")" | "," | "}" | "]" | ";"))
                || (n.kind == Lx::Keyword
                    && matches!(&line[n.range.clone()], "end" | "else" | "elseif" | "until"))
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
