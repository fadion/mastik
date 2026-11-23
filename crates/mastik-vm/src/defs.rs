//! What reads a script's text without running it: the editor's highlighting, the
//! API names a script spells, and the `.d.luau` definitions an outside editor
//! completes from. All read the same tables the runtime binds.

use std::fmt::Write as _;
use std::ops::Range;

use mastik_core::verbs::{self, ParamKind};

use crate::lex::{self, Lx};
use crate::props::{self, METHODS, PROPERTIES};

/// A highlighted token's kind.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Tok {
    Keyword,
    /// `true`, `false` and `nil`: values, though Luau reserves them.
    Constant,
    Number,
    String,
    Comment,
    /// Mastik's own names: `sel`, `param`, the relative names, the verbs, and a
    /// property or method after a `.` or `:`.
    Api,
    /// The permitted standard library: `math`, `ipairs`…, and a member after
    /// `math.`, `string.` or `table.`.
    Builtin,
    /// Any other name called or defined: the name right before a `(`.
    Function,
    Plain,
}

/// Whether lexeme `i` of `lexemes` is the standard library's: a permitted
/// global, or a listed member after its library and a `.`.
fn is_builtin(src: &str, lexemes: &[lex::Lexeme], i: usize) -> bool {
    let word = &src[lexemes[i].range.clone()];
    let text = |j: usize| &src[lexemes[j].range.clone()];
    match i.checked_sub(1).map(text) {
        Some(".") => i
            .checked_sub(2)
            .is_some_and(|j| crate::run::library(text(j)).contains(&word)),
        Some(":") => false,
        _ => crate::run::STDLIB.contains(&word),
    }
}

/// The names that are Mastik's wherever they stand as a bare name.
const API_GLOBALS: &[&str] = &[
    "sel",
    "selection",
    "param",
    "parent",
    "prev",
    "next",
    "children",
    "percent",
    "random",
    "tables",
];

/// Whether lexeme `i` of `lexemes` is one of Mastik's names.
fn is_api(src: &str, lexemes: &[lex::Lexeme], i: usize) -> bool {
    let l = &lexemes[i];
    if l.kind != Lx::Ident {
        return false;
    }
    let word = &src[l.range.clone()];
    let before = i.checked_sub(1).map(|p| &src[lexemes[p].range.clone()]);
    match before {
        Some(".") => props::find(word).is_some(),
        Some(":") => METHODS.iter().any(|m| m.name == word),
        _ => API_GLOBALS.contains(&word) || verbs::find(word).is_some(),
    }
}

/// The tokens of `source` for highlighting, whitespace left out.
pub fn tokens(source: &str) -> Vec<(Range<usize>, Tok)> {
    let lexemes = lex::lex(source);
    lexemes
        .iter()
        .enumerate()
        .map(|(i, l)| {
            let calls = lexemes
                .get(i + 1)
                .is_some_and(|n| &source[n.range.clone()] == "(");
            let tok = match l.kind {
                Lx::Keyword if matches!(&source[l.range.clone()], "true" | "false" | "nil") => {
                    Tok::Constant
                }
                Lx::Keyword => Tok::Keyword,
                Lx::Number => Tok::Number,
                Lx::String => Tok::String,
                Lx::Comment => Tok::Comment,
                Lx::Ident if is_api(source, &lexemes, i) => Tok::Api,
                Lx::Ident if is_builtin(source, &lexemes, i) => Tok::Builtin,
                Lx::Ident if calls => Tok::Function,
                Lx::Ident | Lx::Punct => Tok::Plain,
            };
            (l.range.clone(), tok)
        })
        .collect()
}

/// The API names `source` spells, each once, in the order they first appear.
pub fn names_used(source: &str) -> Vec<String> {
    let lexemes = lex::lex(source);
    let mut out: Vec<String> = Vec::new();
    for i in 0..lexemes.len() {
        if is_api(source, &lexemes, i) {
            let word = &source[lexemes[i].range.clone()];
            if !out.iter().any(|w| w == word) {
                out.push(word.to_owned());
            }
        }
    }
    out
}

/// `source` laid out as Luau reads it, for a person to check before it runs: a
/// lone `\r` breaks the line, as it ends a comment or a string for Luau, and any
/// other control character, or one drawn as nothing ([`crate::invisible`]),
/// shows as its `\u{…}` escape. Each escape is ASCII and keeps the lexeme it
/// stands in, so the view highlights as the source does.
pub fn as_read(source: &str) -> String {
    let mut out = String::with_capacity(source.len());
    let mut chars = source.chars().peekable();
    while let Some(c) = chars.next() {
        match c {
            '\r' if chars.peek() == Some(&'\n') => {}
            '\r' => out.push('\n'),
            '\n' | '\t' => out.push(c),
            c if c.is_control() || crate::invisible(c) => {
                let _ = write!(out, "\\u{{{:x}}}", c as u32);
            }
            c => out.push(c),
        }
    }
    out
}

/// A parameter's Luau type.
pub(crate) fn param_type(kind: ParamKind) -> String {
    match kind {
        ParamKind::Node | ParamKind::Nodes => "Layers".into(),
        ParamKind::Number | ParamKind::Index => "number".into(),
        ParamKind::Bool => "boolean".into(),
        ParamKind::Text | ParamKind::Id(_) => "string".into(),
        ParamKind::Choice(words) => words
            .iter()
            .map(|w| format!("{:?}", w))
            .collect::<Vec<_>>()
            .join(" | "),
        ParamKind::Model(_) => "any".into(),
    }
}

/// `doc` as Luau comment lines at `indent`.
fn comment(out: &mut String, indent: &str, doc: &str) {
    for line in doc.lines() {
        let _ = writeln!(out, "{indent}--- {line}");
    }
}

/// A `.d.luau` definitions file for luau-lsp: the `Layers` collection with its
/// properties and methods, `sel` and the relative names, each verb with typed
/// parameters (positional, the subject left to the selection, or one table of
/// named arguments), and `param`.
pub fn definitions() -> String {
    let mut out = String::new();
    out.push_str("-- Mastik Command Mode API, generated by mastik-vm. Do not edit.\n\n");
    out.push_str(
        "--- A collection of layers: `#sel`, `sel[1]` and `ipairs(sel)` work; a write sets\n",
    );
    out.push_str("--- every layer, a read raises when they disagree.\n");
    out.push_str("declare class Layers\n");
    for p in PROPERTIES {
        comment(&mut out, "    ", p.doc);
        // Nil where a layer has none: a text's style on a frame, a gap where
        // nothing is laid out.
        let optional = if matches!(
            p.name,
            "prev"
                | "next"
                | "parent"
                | "fill"
                | "stroke"
                | "font_family"
                | "font_size"
                | "font_weight"
                | "italic"
                | "line_height"
                | "letter_spacing"
                | "text_case"
                | "text_align"
                | "layout"
                | "direction"
                | "gap"
                | "padding"
        ) {
            "?"
        } else {
            ""
        };
        let _ = writeln!(out, "    {}: {}{optional}", p.name, p.ty.luau());
    }
    for m in METHODS {
        comment(&mut out, "    ", m.doc);
        let sep = if m.params.is_empty() { "" } else { ", " };
        let _ = writeln!(
            out,
            "    function {}(self{sep}{}): {}",
            m.name, m.params, m.returns
        );
    }
    out.push_str("end\n\n");
    for (name, doc) in crate::run::NAMES {
        let ty = match *name {
            "sel" | "selection" | "children" => "Layers",
            "parent" | "prev" | "next" => "Layers?",
            "param" => "{ [string]: any }",
            "tables" => "{ [string]: any }",
            _ => continue,
        };
        comment(&mut out, "", doc);
        let _ = writeln!(out, "declare {name}: {ty}");
    }
    comment(
        &mut out,
        "",
        "`percent(50)` is the size `50%`, for a laid-out layer's width or height",
    );
    out.push_str("declare function percent(n: number): any\n\n");
    out.push_str(
        "--- Numbers drawn from a seed: `rng()` a fraction from 0 to 1, `rng(n)` a whole\n\
         --- number from 1 to n, `rng(m, n)` one from m to n. The same seed draws the same.\n\
         export type Random = ((() -> number) & ((n: number) -> number) & ((m: number, n: number) -> number))\n",
    );
    comment(
        &mut out,
        "",
        "`random(seed)`: a generator drawing the same numbers for the same seed",
    );
    out.push_str("declare function random(seed: number | string): Random\n\n");
    for verb in verbs::VERBS {
        let subject = verb
            .params
            .iter()
            .position(|p| matches!(p.kind, ParamKind::Node | ParamKind::Nodes));
        let positional: Vec<String> = verb
            .params
            .iter()
            .enumerate()
            .filter(|(i, _)| Some(*i) != subject)
            .map(|(_, p)| {
                let q = if p.required { "" } else { "?" };
                format!("{}: ({}){q}", p.name, param_type(p.kind))
            })
            .collect();
        let named: Vec<String> = verb
            .params
            .iter()
            .map(|p| {
                let q = if p.required && Some(p.name) != subject.map(|i| verb.params[i].name) {
                    ""
                } else {
                    "?"
                };
                format!("{}: ({}){q}", p.name, param_type(p.kind))
            })
            .collect();
        comment(&mut out, "", verb.doc);
        for p in verb.params {
            let _ = writeln!(out, "--- @param {} {}", p.name, p.doc);
        }
        let ret = "Layers | string | nil";
        let mut forms = vec![format!("(({}) -> {ret})", positional.join(", "))];
        if subject.is_some() {
            let mut with = vec!["subject: Layers".to_owned()];
            with.extend(positional.iter().cloned());
            forms.push(format!("(({}) -> {ret})", with.join(", ")));
        }
        forms.push(format!("((args: {{ {} }}) -> {ret})", named.join(", ")));
        let _ = writeln!(out, "declare {}: {}\n", verb.name, forms.join(" & "));
    }
    out
}
