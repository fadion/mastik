//! A forgiving Luau lexer: enough to highlight, to find prefix positions for the
//! bar's sugar and to tell a read from a write. It never fails; text it cannot
//! place is one-character punctuation, and an unclosed string runs to its line's end.

use std::ops::Range;

/// What a lexeme is.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Lx {
    Ident,
    Keyword,
    Number,
    String,
    Comment,
    Punct,
}

/// One lexeme and its byte range in the source.
#[derive(Clone, Debug)]
pub(crate) struct Lexeme {
    pub kind: Lx,
    pub range: Range<usize>,
}

/// Luau's reserved words.
pub(crate) const KEYWORDS: &[&str] = &[
    "and", "break", "do", "else", "elseif", "end", "false", "for", "function", "if", "in", "local",
    "nil", "not", "or", "repeat", "return", "then", "true", "until", "while", "continue",
];

/// Operators of more than one character, longest first so a scan takes the longest.
const OPERATORS: &[&str] = &[
    "...", "..=", "//=", "..", "==", "~=", "<=", ">=", "//", "+=", "-=", "*=", "/=", "%=", "^=",
    "->", "::",
];

/// Whether `c` may start an identifier.
pub(crate) fn ident_start(c: char) -> bool {
    c.is_ascii_alphabetic() || c == '_'
}

/// Whether `c` may continue an identifier.
pub(crate) fn ident_char(c: char) -> bool {
    c.is_ascii_alphanumeric() || c == '_'
}

/// The lexemes of `src`, whitespace left out.
pub(crate) fn lex(src: &str) -> Vec<Lexeme> {
    let b = src.as_bytes();
    let mut out = Vec::new();
    // One entry per open interpolation in a backtick string: the depth of the
    // ordinary braces opened inside it, so its own `}` resumes the string.
    let mut interp: Vec<usize> = Vec::new();
    let mut i = 0;
    while i < b.len() {
        let c = b[i] as char;
        let start = i;
        let kind = if c.is_ascii_whitespace() {
            i += 1;
            continue;
        } else if src[i..].starts_with("--") {
            i += 2;
            match long_bracket(b, i) {
                Some(level) => i = close_long(b, i, level),
                None => i = line_end(b, i),
            }
            Lx::Comment
        } else if c == '"' || c == '\'' {
            i = close_quote(b, i);
            Lx::String
        } else if c == '`' || (c == '}' && interp.last() == Some(&0)) {
            // The code in a `{…}` is lexed as code: it runs.
            if c == '}' {
                interp.pop();
            }
            let (end, opens) = interp_section(b, i + 1);
            i = end;
            if opens {
                interp.push(0);
            }
            Lx::String
        } else if c == '[' && long_bracket(b, i).is_some() {
            let level = long_bracket(b, i).unwrap_or(0);
            i = close_long(b, i, level);
            Lx::String
        } else if c.is_ascii_digit()
            || (c == '.' && b.get(i + 1).is_some_and(|d| d.is_ascii_digit()))
        {
            i = number_end(b, i);
            Lx::Number
        } else if ident_start(c) {
            while i < b.len() && ident_char(b[i] as char) {
                i += 1;
            }
            if KEYWORDS.contains(&&src[start..i]) {
                Lx::Keyword
            } else {
                Lx::Ident
            }
        } else {
            if let Some(depth) = interp.last_mut() {
                match c {
                    '{' => *depth += 1,
                    '}' => *depth -= 1,
                    _ => {}
                }
            }
            let op = OPERATORS.iter().find(|op| src[i..].starts_with(**op));
            i += op.map_or_else(
                || src[i..].chars().next().map_or(1, char::len_utf8),
                |o| o.len(),
            );
            Lx::Punct
        };
        out.push(Lexeme {
            kind,
            range: start..i,
        });
    }
    out
}

/// The level of a long bracket (`[[`, `[==[`) opening at `i`, if one does.
fn long_bracket(b: &[u8], i: usize) -> Option<usize> {
    if b.get(i) != Some(&b'[') {
        return None;
    }
    let mut j = i + 1;
    while b.get(j) == Some(&b'=') {
        j += 1;
    }
    (b.get(j) == Some(&b'[')).then_some(j - i - 1)
}

/// The end of the long bracket of `level` opened at `i`: past its close, or the source's end.
fn close_long(b: &[u8], i: usize, level: usize) -> usize {
    let mut close = vec![b']'];
    close.extend(std::iter::repeat_n(b'=', level));
    close.push(b']');
    let from = i + level + 2;
    b.get(from..)
        .and_then(|rest| {
            rest.windows(close.len())
                .position(|w| w == close.as_slice())
        })
        .map_or(b.len(), |p| from + p + close.len())
}

/// The end of the line holding `i`, before its newline. A lone `\r` ends it too,
/// as it ends a comment or a string for Luau: code after one runs, so it must
/// not be read as part of the comment before it.
fn line_end(b: &[u8], i: usize) -> usize {
    b[i..]
        .iter()
        .position(|c| matches!(c, b'\n' | b'\r'))
        .map_or(b.len(), |p| i + p)
}

/// The end of the quoted string opening at `i` (`"` or `'`): past its closing
/// quote, or its line's end.
fn close_quote(b: &[u8], i: usize) -> usize {
    let quote = b[i];
    let mut j = i + 1;
    while j < b.len() {
        match b[j] {
            b'\\' => j += 2,
            b'\n' | b'\r' => return j,
            c if c == quote => return j + 1,
            _ => j += 1,
        }
    }
    b.len()
}

/// The end of an interpolated string's text section starting at `i`, just past
/// a backtick or a `}` closing an interpolation: past the closing backtick, past
/// a `{` that opens an interpolation (then `true`), or its line's end.
fn interp_section(b: &[u8], i: usize) -> (usize, bool) {
    let mut j = i;
    while j < b.len() {
        match b[j] {
            // `\u{…}` is an escape, its brace no interpolation.
            b'\\' if b.get(j + 1..j + 3) == Some(b"u{") => j += 3,
            b'\\' => j += 2,
            b'\n' | b'\r' => return (j, false),
            b'`' => return (j + 1, false),
            b'{' => return (j + 1, true),
            _ => j += 1,
        }
    }
    (b.len(), false)
}

/// The end of the number starting at `i`: digits, letters (a hex digit, an
/// exponent), `_`, `.`, and a sign right after a decimal exponent.
fn number_end(b: &[u8], i: usize) -> usize {
    let hex = b[i] == b'0' && matches!(b.get(i + 1), Some(b'x' | b'X'));
    let mut j = i;
    while j < b.len() {
        let c = b[j];
        let signed = matches!(c, b'+' | b'-')
            && j > i
            && if hex {
                matches!(b[j - 1], b'p' | b'P')
            } else {
                matches!(b[j - 1], b'e' | b'E')
            };
        // A `.` before another is the `..` operator, not a decimal point.
        let point = c == b'.' && b.get(j + 1) != Some(&b'.');
        if c.is_ascii_alphanumeric() || c == b'_' || signed || point {
            j += 1;
        } else {
            break;
        }
    }
    j
}

/// Whether `word` reads as a Luau number literal.
pub(crate) fn is_number(word: &str) -> bool {
    let w = word.strip_prefix('-').unwrap_or(word);
    if let Some(hex) = w.strip_prefix("0x").or_else(|| w.strip_prefix("0X")) {
        return !hex.is_empty() && hex.chars().all(|c| c.is_ascii_hexdigit() || c == '_');
    }
    if let Some(bin) = w.strip_prefix("0b").or_else(|| w.strip_prefix("0B")) {
        return !bin.is_empty() && bin.chars().all(|c| matches!(c, '0' | '1' | '_'));
    }
    let plain: String = w.chars().filter(|c| *c != '_').collect();
    w.starts_with(|c: char| c.is_ascii_digit() || c == '.')
        && !plain.eq_ignore_ascii_case("inf")
        && !plain.eq_ignore_ascii_case("nan")
        && plain.parse::<f64>().is_ok()
}
