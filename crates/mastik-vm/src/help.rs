//! The help the script editor gives while a script is written: the call the
//! caret is in ([`signature_at`]), what a name is ([`hover_at`]), the problems a
//! check finds without running ([`check`], [`syntax_error`]) and where one of the
//! script's own names is declared ([`declaration_at`]). Each reads the tables the
//! runtime binds, so help cannot disagree with what a run does.

use std::ops::Range;

use mastik_core::verbs::{self, ParamKind, Verb};
use mastik_core::{Document, NodeId};

use crate::complete::{Decl, declarations};
use crate::lex::{self, Lexeme, Lx};
use crate::props::{self, METHODS, PropType};
use crate::run::{self, NAMES, STDLIB};

/// Appended to a verb's doc when it takes layers: the subject is never typed.
pub const ACTS_ON_SELECTION: &str = "Acts on the selection.";

/// One parameter of a [`Signature`].
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SigParam {
    pub name: String,
    /// Its type as the `.d.luau` spells it, `?` on an optional one; `None` for
    /// the script's own functions, which write no types.
    pub ty: Option<String>,
}

/// The call the caret is in, as the parameter hint shows it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Signature {
    /// What is called, as written: `align`, `card:adjust`, `nudge`.
    pub name: String,
    pub params: Vec<SigParam>,
    /// A table of named arguments (`verb { … }`): braces, and nothing lit.
    pub named: bool,
    /// The parameter the caret is in; `None` past the last and in a table.
    pub active: Option<usize>,
    /// The line under the signature: the lit parameter's `name · doc`, `No more
    /// arguments`, or what a table argument has left.
    pub detail: Option<String>,
    /// The callee's doc, drawn dim.
    pub doc: Option<String>,
    /// The faint last line: where the script's own function is declared.
    pub note: Option<String>,
    /// The byte the call opens at (its `(` or `{`): the hint hangs over its line.
    pub open: usize,
}

/// What [`call_at`] found the caret in.
pub(crate) enum Call {
    /// A verb's positional argument `arg` (the subject left out), or past them.
    Verb {
        verb: &'static Verb,
        arg: usize,
        open: usize,
    },
    /// A verb's table of named arguments, with the names already written.
    Named {
        verb: &'static Verb,
        written: Vec<String>,
        open: usize,
    },
    /// A collection method, called on `owner` (as written, possibly empty).
    Method {
        method: &'static props::Method,
        owner: String,
        arg: usize,
        open: usize,
    },
    /// A function the script declares, with its parameters and its line.
    Script {
        name: String,
        params: Vec<String>,
        line: u32,
        arg: usize,
        open: usize,
    },
}

/// One open bracket while [`call_at`] walks.
struct Frame {
    at: usize,
    commas: usize,
    /// In a `{`, the names written as `name =` at its top level.
    keys: Vec<String>,
}

/// A verb's parameters without the subject, which a script never types.
fn typed_params(verb: &Verb) -> impl Iterator<Item = &'static verbs::Param> {
    verb.params
        .iter()
        .filter(|p| !matches!(p.kind, ParamKind::Node | ParamKind::Nodes))
}

/// Whether `verb` acts on layers it is given or the selection.
fn takes_layers(verb: &Verb) -> bool {
    verb.params
        .iter()
        .any(|p| matches!(p.kind, ParamKind::Node | ParamKind::Nodes))
}

/// Index of the lexeme holding byte `at` (or ending at it), if any.
fn lexeme_at(lexemes: &[Lexeme], at: usize) -> Option<usize> {
    lexemes
        .iter()
        .position(|l| l.range.start <= at && at < l.range.end)
        .or_else(|| lexemes.iter().position(|l| l.range.end == at))
}

/// The line, from 1, holding byte `at` of `source`.
fn line_of(source: &str, at: usize) -> u32 {
    let at = at.min(source.len());
    u32::try_from(source[..at].matches('\n').count() + 1).unwrap_or(u32::MAX)
}

/// The call the caret at byte `cursor` is in: the nearest unclosed `(` (or a
/// verb's `{`) before it whose callee is a verb, a collection method or a
/// function the script declares. Nothing inside a comment.
pub(crate) fn call_at(source: &str, cursor: usize) -> Option<Call> {
    let lexemes = lex::lex(source);
    if lexemes
        .iter()
        .any(|l| l.kind == Lx::Comment && l.range.start < cursor && cursor <= l.range.end)
    {
        return None;
    }
    let text = |i: usize| lexemes.get(i).map_or("", |l| &source[l.range.clone()]);
    let mut frames: Vec<Frame> = Vec::new();
    for (i, l) in lexemes.iter().enumerate() {
        if l.range.start >= cursor {
            break;
        }
        if l.kind != Lx::Punct {
            if l.kind == Lx::Ident
                && text(i + 1) == "="
                && matches!(text(i.wrapping_sub(1)), "{" | "," | ";")
                && let Some(f) = frames.last_mut()
                && text(f.at) == "{"
            {
                f.keys.push(text(i).to_owned());
            }
            continue;
        }
        match text(i) {
            "(" | "{" | "[" => frames.push(Frame {
                at: i,
                commas: 0,
                keys: Vec::new(),
            }),
            ")" | "}" | "]" => {
                frames.pop();
            }
            "," | ";" => {
                if let Some(f) = frames.last_mut() {
                    f.commas += 1;
                }
            }
            _ => {}
        }
    }
    let ident = |i: usize| lexemes.get(i).is_some_and(|l| l.kind == Lx::Ident);
    let decls = declarations(source, &lexemes, &(0..0));
    for f in frames.iter().rev() {
        let open = lexemes[f.at].range.start;
        let callee = f.at.wrapping_sub(1);
        let bare = ident(callee) && !matches!(text(callee.wrapping_sub(1)), "." | ":");
        match text(f.at) {
            "{" => {
                // `verb { … }` or `verb({ … })`.
                let verb_at = if text(callee) == "(" {
                    callee.wrapping_sub(1)
                } else {
                    callee
                };
                let plain = ident(verb_at) && !matches!(text(verb_at.wrapping_sub(1)), "." | ":");
                if plain
                    && let Some(verb) = verbs::find(text(verb_at))
                    && !shadowed(&decls, &lexemes, source, text(verb_at))
                {
                    return Some(Call::Named {
                        verb,
                        written: f.keys.clone(),
                        open,
                    });
                }
            }
            "(" if ident(callee) && text(callee.wrapping_sub(1)) == ":" => {
                if let Some(method) = METHODS.iter().find(|m| m.name == text(callee)) {
                    let owner_at = callee.wrapping_sub(2);
                    let owner = if ident(owner_at) { text(owner_at) } else { "" };
                    return Some(Call::Method {
                        method,
                        owner: owner.to_owned(),
                        arg: f.commas,
                        open,
                    });
                }
            }
            "(" if bare => {
                let name = text(callee);
                if let Some(d) = function_decl(&decls, &lexemes, source, name) {
                    return Some(Call::Script {
                        name: name.to_owned(),
                        params: function_params(&lexemes, source, d.at),
                        line: line_of(source, lexemes[d.at].range.start),
                        arg: f.commas,
                        open,
                    });
                }
                if let Some(verb) = verbs::find(name)
                    && !shadowed(&decls, &lexemes, source, name)
                {
                    return Some(Call::Verb {
                        verb,
                        arg: f.commas,
                        open,
                    });
                }
            }
            _ => {}
        }
    }
    None
}

/// Whether the script declares `name` itself, hiding Mastik's.
fn shadowed(decls: &[Decl], lexemes: &[Lexeme], source: &str, name: &str) -> bool {
    decls
        .iter()
        .any(|d| &source[lexemes[d.at].range.clone()] == name)
}

/// The first function the script declares as `name`.
fn function_decl(decls: &[Decl], lexemes: &[Lexeme], source: &str, name: &str) -> Option<Decl> {
    decls
        .iter()
        .find(|d| d.what == "function" && &source[lexemes[d.at].range.clone()] == name)
        .copied()
}

/// The parameter names of the function whose name is lexeme `at`.
fn function_params(lexemes: &[Lexeme], source: &str, at: usize) -> Vec<String> {
    let text = |i: usize| lexemes.get(i).map_or("", |l| &source[l.range.clone()]);
    let mut i = at + 1;
    if text(i) != "(" {
        return Vec::new();
    }
    i += 1;
    let mut out = Vec::new();
    while matches!(lexemes.get(i), Some(l) if l.kind == Lx::Ident) || text(i) == "..." {
        out.push(text(i).to_owned());
        if text(i + 1) != "," {
            break;
        }
        i += 2;
    }
    out
}

/// `params` split at their top-level commas: `a: x, b: (y, z) -> w` is two.
fn split_params(params: &str) -> Vec<SigParam> {
    let mut out = Vec::new();
    let mut depth = 0i32;
    let mut start = 0;
    let mut push = |s: &str| {
        let s = s.trim();
        if s.is_empty() {
            return;
        }
        let (name, ty) = s
            .split_once(':')
            .map_or((s, None), |(n, t)| (n.trim(), Some(t.trim().to_owned())));
        out.push(SigParam {
            name: name.to_owned(),
            ty,
        });
    };
    for (i, c) in params.char_indices() {
        match c {
            '(' | '{' => depth += 1,
            ')' | '}' => depth -= 1,
            ',' if depth == 0 => {
                push(&params[start..i]);
                start = i + 1;
            }
            _ => {}
        }
    }
    push(&params[start..]);
    out
}

/// A verb's parameter as the hint writes it.
fn verb_param(p: &verbs::Param) -> SigParam {
    let q = if p.required { "" } else { "?" };
    SigParam {
        name: p.name.to_owned(),
        ty: Some(format!("{}{q}", crate::defs::param_type(p.kind))),
    }
}

/// A verb's doc as help shows it: the subject's line appended when it takes layers.
pub(crate) fn verb_doc(verb: &Verb) -> String {
    if takes_layers(verb) {
        format!("{}. {ACTS_ON_SELECTION}", verb.doc.trim_end_matches('.'))
    } else {
        verb.doc.to_owned()
    }
}

/// The detail line for parameter `p` of `params`, or past the last.
fn lit_line(params: &[SigParam], docs: &[&str], arg: usize) -> (Option<usize>, String) {
    match params.get(arg) {
        Some(p) => {
            let doc = docs.get(arg).copied().unwrap_or_default();
            if doc.is_empty() {
                (Some(arg), p.name.clone())
            } else {
                (Some(arg), format!("{} · {doc}", p.name))
            }
        }
        None => (None, "No more arguments".to_owned()),
    }
}

/// The parameter hint for the caret at byte `cursor` of `source`.
pub fn signature_at(source: &str, cursor: usize) -> Option<Signature> {
    Some(match call_at(source, cursor)? {
        Call::Verb { verb, arg, open } => {
            let ps: Vec<&verbs::Param> = typed_params(verb).collect();
            let params: Vec<SigParam> = ps.iter().map(|p| verb_param(p)).collect();
            let docs: Vec<&str> = ps.iter().map(|p| p.doc).collect();
            let (active, detail) = lit_line(&params, &docs, arg);
            Signature {
                name: verb.name.to_owned(),
                params,
                named: false,
                active,
                detail: Some(detail),
                doc: Some(verb_doc(verb)),
                note: None,
                open,
            }
        }
        Call::Named {
            verb,
            written,
            open,
        } => {
            let params: Vec<SigParam> = typed_params(verb).map(verb_param).collect();
            let left: Vec<&str> = params
                .iter()
                .map(|p| p.name.as_str())
                .filter(|n| !written.iter().any(|w| w == n))
                .collect();
            let detail = match left.as_slice() {
                [] => "All given.".to_owned(),
                [one] => format!("Named: {one} is left."),
                many => format!("Named: {} are left.", many.join(", ")),
            };
            Signature {
                name: verb.name.to_owned(),
                params,
                named: true,
                active: None,
                detail: Some(detail),
                doc: Some(verb_doc(verb)),
                note: None,
                open,
            }
        }
        Call::Method {
            method,
            owner,
            arg,
            open,
        } => {
            let params = split_params(method.params);
            let (active, detail) = lit_line(&params, &[], arg);
            Signature {
                name: format!("{owner}:{}", method.name),
                params,
                named: false,
                active,
                detail: active.is_none().then_some(detail),
                doc: Some(method.doc.to_owned()),
                note: None,
                open,
            }
        }
        Call::Script {
            name,
            params,
            line,
            arg,
            open,
        } => {
            let params: Vec<SigParam> = params
                .into_iter()
                .map(|name| SigParam { name, ty: None })
                .collect();
            let (active, detail) = lit_line(&params, &[], arg);
            Signature {
                name,
                params,
                named: false,
                active,
                detail: active.is_none().then_some(detail),
                doc: None,
                note: Some(format!("This script's function · line {line}")),
                open,
            }
        }
    })
}

/// What a name is, as its hover card shows it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Hover {
    /// The name's bytes, washed while the card is up.
    pub range: Range<usize>,
    /// The first line: the name as written, with its parameters when it is called.
    pub title: String,
    /// Its type, after the title.
    pub ty: Option<String>,
    pub doc: Option<String>,
    /// The faint last line: what kind of name it is.
    pub kind: String,
    /// A layer name's layer, outlined on the canvas while the card is up. Its
    /// doc is the caller's to write, from the layer.
    pub layer: Option<NodeId>,
}

/// A library member's or global's title and type.
fn lib_title(d: &run::LibDoc) -> (String, Option<String>) {
    match d.params {
        Some(p) => (format!("{}({p})", d.path), Some(d.returns.to_owned())),
        None => (d.path.to_owned(), Some(d.returns.to_owned())),
    }
}

/// The type a name of the language has.
fn name_type(name: &str) -> Option<&'static str> {
    match name {
        "sel" | "selection" | "children" => Some("Layers"),
        "parent" | "prev" | "next" => Some("Layers?"),
        _ => None,
    }
}

/// A verb's signature as one line, positional.
fn verb_title(verb: &Verb) -> String {
    let params: Vec<String> = typed_params(verb)
        .map(|p| {
            let s = verb_param(p);
            format!("{}: {}", s.name, s.ty.unwrap_or_default())
        })
        .collect();
    format!("{}({})", verb.name, params.join(", "))
}

/// What the name at byte `at` of `source` is, read against `page`'s layers;
/// `None` for a keyword, a number, a string, a plain local or a name nothing
/// knows, whose card would only repeat the word.
pub fn hover_at(doc: &Document, page: NodeId, source: &str, at: usize) -> Option<Hover> {
    let lexemes = lex::lex(source);
    let i = lexemes
        .iter()
        .position(|l| l.range.start <= at && at < l.range.end)?;
    let l = &lexemes[i];
    if l.kind != Lx::Ident {
        return None;
    }
    let text = |j: usize| lexemes.get(j).map_or("", |l| &source[l.range.clone()]);
    let word = text(i);
    let range = l.range.clone();
    let owner = i
        .checked_sub(2)
        .filter(|o| lexemes[*o].kind == Lx::Ident)
        .map(text);
    let hover = |title: String, ty: Option<String>, doc: Option<String>, kind: String| Hover {
        range: range.clone(),
        title,
        ty,
        doc,
        kind,
        layer: None,
    };
    match i.checked_sub(1).map(text) {
        Some(".") => {
            if owner == Some("param") {
                let input = crate::header::header(source)
                    .ok()?
                    .inputs
                    .into_iter()
                    .find(|n| n.name == word)?;
                let default = match (&input.default, &input.ty) {
                    (serde_json::Value::Number(n), _) => {
                        crate::format_number(n.as_f64().unwrap_or_default())
                    }
                    (serde_json::Value::String(s), crate::InputType::Choice(_)) => s.clone(),
                    (serde_json::Value::String(s), _) => format!("{s:?}"),
                    (other, _) => other.to_string(),
                };
                let ty = format!("{} = {default}", input.ty.spelled());
                return Some(hover(
                    format!("param.{word}"),
                    Some(ty),
                    input.label.clone(),
                    format!("This script's input, line {}", input.line),
                ));
            }
            if let Some(lib) = owner.filter(|o| run::LIBRARY.iter().any(|(n, _)| n == o)) {
                let d = run::lib_doc(&format!("{lib}.{word}"))?;
                let (title, ty) = lib_title(d);
                return Some(hover(
                    title,
                    ty,
                    Some(d.doc.to_owned()),
                    "Luau library".into(),
                ));
            }
            let p = props::find(word)?;
            let title = owner.map_or_else(|| format!(".{word}"), |o| format!("{o}.{word}"));
            Some(hover(
                title,
                Some(p.ty.luau().to_owned()),
                Some(p.doc.to_owned()),
                "Property · reads raise when the layers disagree".into(),
            ))
        }
        Some(":") => {
            let m = METHODS.iter().find(|m| m.name == word)?;
            let title = format!("{}:{word}({})", owner.unwrap_or_default(), m.params);
            Some(hover(
                title,
                Some(m.returns.to_owned()),
                Some(m.doc.to_owned()),
                "Method".into(),
            ))
        }
        _ => {
            let decls = declarations(source, &lexemes, &(0..0));
            if let Some(d) = function_decl(&decls, &lexemes, source, word) {
                let params = function_params(&lexemes, source, d.at);
                let line = line_of(source, lexemes[d.at].range.start);
                return Some(hover(
                    format!("{word}({})", params.join(", ")),
                    None,
                    None,
                    format!("This script's function · line {line}"),
                ));
            }
            if shadowed(&decls, &lexemes, source, word) {
                return None;
            }
            if let Some((_, d)) = NAMES.iter().find(|(n, _)| *n == word) {
                return Some(hover(
                    word.to_owned(),
                    name_type(word).map(str::to_owned),
                    Some((*d).to_owned()),
                    "Name".into(),
                ));
            }
            if let Some(verb) = verbs::find(word) {
                return Some(hover(
                    verb_title(verb),
                    None,
                    Some(verb_doc(verb)),
                    "Verb".into(),
                ));
            }
            if STDLIB.contains(&word) {
                let d = run::lib_doc(word)?;
                let (title, ty) = lib_title(d);
                return Some(hover(
                    title,
                    ty,
                    Some(d.doc.to_owned()),
                    "Luau library".into(),
                ));
            }
            let found = run::named(doc, page, word);
            let first = *found.first()?;
            let kind = if found.len() > 1 {
                "A layer named on this page · two or more of that name raise at the run"
            } else {
                "A layer named on this page"
            };
            Some(Hover {
                layer: Some(first),
                ..hover(word.to_owned(), Some("Layers".into()), None, kind.into())
            })
        }
    }
}

/// How sure a [`Problem`] is.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum Severity {
    /// Certain: a run fails here.
    Error,
    /// Likely: a run decides, as a layer of that name may exist by then.
    Warning,
}

/// A problem the check finds without running.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Problem {
    pub severity: Severity,
    /// What a run would say.
    pub message: String,
    /// The line, from 1.
    pub line: u32,
    /// The bytes to squiggle.
    pub span: Range<usize>,
    /// The run's "did you mean": the range to replace and the name to write.
    pub fix: Option<(Range<usize>, String)>,
    /// A faint second line.
    pub note: Option<String>,
    /// Luau's parser refused the text: *Run* is held, and the mark stands even
    /// under the caret.
    pub syntax: bool,
}

/// The byte range of line `n` (from 1) of `source`, its newline left out.
fn line_range(source: &str, n: u32) -> Range<usize> {
    let mut start = 0;
    for (i, l) in source.split_inclusive('\n').enumerate() {
        if i + 1 == n as usize {
            return start..start + l.trim_end_matches(['\n', '\r']).len();
        }
        start += l.len();
    }
    source.len()..source.len()
}

/// `range` of `source` with its surrounding whitespace left out.
fn trimmed(source: &str, range: Range<usize>) -> Range<usize> {
    let s = &source[range.clone()];
    let start = range.start + (s.len() - s.trim_start().len());
    start..start + s.trim().len()
}

/// The syntax error in `source`, if Luau's parser finds one. Nothing runs.
pub fn syntax_error(source: &str) -> Option<Problem> {
    let lua = mlua::Lua::new();
    let e = lua.load(source).set_name("=script").into_function().err()?;
    let raw = match &e {
        mlua::Error::SyntaxError { message, .. } => message.clone(),
        other => other.to_string(),
    };
    let (line, message) = run::split_line(&raw);
    let (line, span) = syntax_place(source, line, &message);
    Some(Problem {
        severity: Severity::Error,
        message: plain_syntax(&message),
        line,
        span,
        fix: None,
        note: None,
        syntax: true,
    })
}

/// Where Luau's syntax error `message` on `line` of `source` belongs: the token
/// after its *got '…'*, read by the lexer on that line, or else the line's text;
/// `<eof>` lands past the text, so it marks the last line that has any.
pub(crate) fn syntax_place(source: &str, line: Option<u32>, message: &str) -> (u32, Range<usize>) {
    let lines = source.split_inclusive('\n').count().max(1);
    let mut line = line
        .unwrap_or(1)
        .clamp(1, u32::try_from(lines).unwrap_or(u32::MAX));
    while line > 1 && line_range(source, line).is_empty() {
        line -= 1;
    }
    let whole = trimmed(source, line_range(source, line));
    let token = message
        .rsplit_once("got '")
        .and_then(|(_, rest)| rest.split_once('\''))
        .map(|(t, _)| t)
        .filter(|t| !t.is_empty());
    let span = token
        .and_then(|t| {
            lex::lex(source)
                .into_iter()
                .filter(|l| !matches!(l.kind, Lx::String | Lx::Comment))
                .map(|l| l.range)
                .find(|r| whole.contains(&r.start) && &source[r.clone()] == t)
        })
        .unwrap_or(whole);
    (line, span)
}

/// Luau's syntax error `message` in plain words for the commonest cases, a
/// text that stops short: *Missing `end` to close the `for` on line 1*,
/// *Missing `)`*, *Something is missing at the end*. Any other message as Luau
/// words it.
pub(crate) fn plain_syntax(message: &str) -> String {
    if !message.ends_with("got <eof>") {
        return message.to_owned();
    }
    let quoted = |s: &str| {
        s.split_once('\'')
            .and_then(|(_, rest)| rest.split_once('\''))
            .map(|(q, rest)| (q.to_owned(), rest.to_owned()))
    };
    if let Some(rest) = message.strip_prefix("Expected ")
        && let Some((wanted, rest)) = quoted(rest)
    {
        // `(to close 'for' at line 1)` or `(to close '(' at column 6)`.
        let closes = rest
            .split_once("(to close ")
            .and_then(|(_, r)| quoted(r))
            .map(|(opened, r)| {
                let at = r
                    .trim_start()
                    .strip_prefix("at line ")
                    .and_then(|n| n.split(')').next())
                    .filter(|n| n.parse::<u32>().is_ok());
                match at {
                    Some(n) => format!(" to close the `{opened}` on line {n}"),
                    None => format!(" to close `{opened}`"),
                }
            })
            .unwrap_or_default();
        return format!("Missing `{wanted}`{closes}");
    }
    "Something is missing at the end".to_owned()
}

/// The names a bare word may be besides the script's own.
fn global_names(doc: &Document, page: NodeId) -> Vec<String> {
    let mut out: Vec<String> = NAMES.iter().map(|(n, _)| (*n).to_owned()).collect();
    out.extend(verbs::VERBS.iter().map(|v| v.name.to_owned()));
    out.extend(STDLIB.iter().map(|s| (*s).to_owned()));
    out.extend(crate::layer_names(doc, page));
    out
}

/// The language's names that hold layers.
const COLLECTIONS: &[&str] = &["sel", "selection", "parent", "prev", "next", "children"];

/// Every problem the check finds in `source` against `page`, in source order:
/// a syntax error alone when there is one; else names the check cannot find
/// (warnings), properties no layer has, library members and header inputs that
/// do not exist, and assignments to globals (errors). Nothing runs.
pub fn check(doc: &Document, page: NodeId, source: &str) -> Vec<Problem> {
    if let Some(p) = syntax_error(source) {
        return vec![p];
    }
    let lexemes = lex::lex(source);
    let text = |j: usize| lexemes.get(j).map_or("", |l| &source[l.range.clone()]);
    let ident = |j: usize| lexemes.get(j).is_some_and(|l| l.kind == Lx::Ident);
    let decls = declarations(source, &lexemes, &(0..0));
    let declared_at: Vec<usize> = decls.iter().map(|d| d.at).collect();
    let declared: Vec<&str> = decls.iter().map(|d| text(d.at)).collect();
    let typed = crate::complete::in_types(source, &lexemes, &decls);
    let layers = crate::layer_names(doc, page);
    let globals = global_names(doc, page);
    let inputs: Vec<String> = crate::header::header(source)
        .map(|h| h.inputs.into_iter().map(|i| i.name).collect())
        .unwrap_or_default();
    let is_global = |w: &str| {
        NAMES.iter().any(|(n, _)| *n == w)
            || verbs::find(w).is_some()
            || STDLIB.contains(&w)
            || w == "self"
    };
    // Locals that hold a collection: `local c = sel` and a loop over one.
    let mut aliases: Vec<&str> = Vec::new();
    for d in &decls {
        let at = d.at;
        let held = match d.what {
            "local" if text(at + 1) == "=" => Some(at + 2),
            "loop variable" if text(at.wrapping_sub(1)) == "," => {
                let mut j = at + 1;
                while text(j) != "in" && j < lexemes.len() {
                    j += 1;
                }
                (matches!(text(j + 1), "ipairs" | "pairs") && text(j + 2) == "(").then_some(j + 3)
            }
            _ => None,
        };
        if let Some(h) = held
            && ident(h)
            && !matches!(text(h + 1), "." | ":" | "(" | "[")
            && (COLLECTIONS.contains(&text(h))
                || (layers.iter().any(|l| l == text(h)) && !declared.contains(&text(h))))
        {
            aliases.push(text(at));
        }
    }
    // Whether the expression ending at lexeme `j` holds layers.
    let collection = |mut j: usize| -> bool {
        loop {
            if text(j) == "]" {
                let mut depth = 0;
                while j > 0 {
                    match text(j) {
                        "]" => depth += 1,
                        "[" => {
                            depth -= 1;
                            if depth == 0 {
                                break;
                            }
                        }
                        _ => {}
                    }
                    j -= 1;
                }
                j = j.wrapping_sub(1);
                continue;
            }
            if !ident(j) {
                return false;
            }
            let w = text(j);
            if text(j.wrapping_sub(1)) == "." {
                if !props::find(w).is_some_and(|p| p.ty == PropType::Layers) {
                    return false;
                }
                j = j.wrapping_sub(2);
                continue;
            }
            if text(j.wrapping_sub(1)) == ":" {
                return false;
            }
            return aliases.contains(&w)
                || (!declared.contains(&w)
                    && (COLLECTIONS.contains(&w) || layers.iter().any(|l| l == w)));
        }
    };
    let mut depth: Vec<&str> = Vec::new();
    let mut out = Vec::new();
    for (i, l) in lexemes.iter().enumerate() {
        match (l.kind, text(i)) {
            (Lx::Punct, "(" | "{" | "[") => depth.push(text(i)),
            (Lx::Punct, ")" | "}" | "]") => {
                depth.pop();
            }
            _ => {}
        }
        if l.kind != Lx::Ident || declared_at.contains(&i) || typed[i] {
            continue;
        }
        let word = text(i);
        let before = text(i.wrapping_sub(1));
        let line = line_of(source, l.range.start);
        let span = l.range.clone();
        let problem =
            |severity, message: String, fix: Option<String>, note: Option<&str>| Problem {
                severity,
                message,
                line,
                span: span.clone(),
                fix: fix.map(|f| (span.clone(), f)),
                note: note.map(str::to_owned),
                syntax: false,
            };
        match before {
            "." if i >= 2 => {
                let owner = text(i - 2);
                if ident(i - 2)
                    && text(i.wrapping_sub(3)) != "."
                    && text(i.wrapping_sub(3)) != ":"
                    && !declared.contains(&owner)
                {
                    if owner == "param" {
                        if !inputs.iter().any(|n| n == word) {
                            let known: Vec<&str> = inputs.iter().map(String::as_str).collect();
                            let near = run::nearest(word, &known);
                            let message = match &near {
                                Some(n) => format!("No input “{word}” — did you mean {n}?"),
                                None => format!("No input “{word}”"),
                            };
                            out.push(problem(Severity::Error, message, near, None));
                        }
                        continue;
                    }
                    let members = run::library(owner);
                    if !members.is_empty() || run::LIBRARY.iter().any(|(n, _)| *n == owner) {
                        if !members.contains(&word) {
                            let near = run::nearest(word, members);
                            let message = match &near {
                                Some(n) => format!("No “{word}” in {owner} — did you mean {n}?"),
                                None => format!("No “{word}” in {owner}"),
                            };
                            out.push(problem(Severity::Error, message, near, None));
                        }
                        continue;
                    }
                }
                if collection(i - 2) && props::find(word).is_none() {
                    let near = run::nearest_property(word);
                    let message = match &near {
                        Some(n) => format!("No property “{word}” — did you mean {n}?"),
                        None => format!("No property “{word}”"),
                    };
                    out.push(problem(Severity::Error, message, near, None));
                }
            }
            "." | ":" | "goto" => {}
            _ => {
                let key = text(i + 1) == "="
                    && matches!(before, "{" | "," | ";")
                    && depth.last() == Some(&"{");
                if key || declared.contains(&word) {
                    continue;
                }
                let assigned = text(i + 1) == "="
                    && depth.is_empty()
                    && !matches!(before, "," | "local" | "for");
                let named_function = before == "function" && text(i + 1) == "(";
                if assigned || named_function {
                    if !is_global(word) || named_function {
                        let message = format!("No global “{word}” — write `local {word} = …`");
                        out.push(problem(Severity::Error, message, None, None));
                    }
                    continue;
                }
                if is_global(word) || layers.iter().any(|n| n == word) {
                    continue;
                }
                let mut known: Vec<&str> = globals.iter().map(String::as_str).collect();
                known.extend(declared.iter().copied());
                let near = run::nearest(word, &known);
                let message = match &near {
                    Some(n) => format!("No name “{word}” — did you mean {n}?"),
                    None => format!("No name “{word}”"),
                };
                out.push(problem(
                    Severity::Warning,
                    message,
                    near,
                    Some("Not a local, a library name or a layer on this page"),
                ));
            }
        }
    }
    out
}

/// The script's own name at byte `at` and where it is declared: the nearest
/// declaration above the use (the one Luau reads there), else the first one,
/// for a function called above it; an input after `param.` goes to its
/// `---@input` line. `None` for any name the script does not declare.
pub fn declaration_at(source: &str, at: usize) -> Option<(Range<usize>, Range<usize>)> {
    let lexemes = lex::lex(source);
    let i = lexeme_at(&lexemes, at)?;
    let l = &lexemes[i];
    if l.kind != Lx::Ident {
        return None;
    }
    let text = |j: usize| lexemes.get(j).map_or("", |l| &source[l.range.clone()]);
    let word = text(i);
    match i.checked_sub(1).map(text) {
        Some(".") if i >= 2 && text(i - 2) == "param" => {
            let input = crate::header::header(source)
                .ok()?
                .inputs
                .into_iter()
                .find(|n| n.name == word)?;
            let line = line_range(source, input.line);
            let tail = &source[line.clone()];
            let at = tail.find("---@input")? + "---@input".len();
            let p = tail[at..].find(word)? + at;
            Some((l.range.clone(), line.start + p..line.start + p + word.len()))
        }
        Some("." | ":") => None,
        _ => {
            let decls = declarations(source, &lexemes, &(0..0));
            let named: Vec<&Decl> = decls.iter().filter(|d| text(d.at) == word).collect();
            let above = named
                .iter()
                .filter(|d| lexemes[d.at].range.start <= l.range.start)
                .max_by_key(|d| d.at);
            let d = above.or_else(|| named.first())?;
            Some((l.range.clone(), lexemes[d.at].range.clone()))
        }
    }
}

/// The range a pick replaces and the `(label, detail)` rows on offer.
pub(crate) type ArgumentRows = (Range<usize>, Vec<(String, String)>);

/// Completion at a verb's argument where the generic list would be wrong: a
/// choice parameter's words with their quotes when `cursor` is inside a string
/// (the string is replaced), and the names a table argument has left at a key.
pub(crate) fn argument_rows(
    source: &str,
    cursor: usize,
    string: Option<Range<usize>>,
) -> Option<ArgumentRows> {
    match (call_at(source, cursor)?, string) {
        (Call::Verb { verb, arg, .. }, Some(s)) => {
            let p = typed_params(verb).nth(arg)?;
            let ParamKind::Choice(words) = p.kind else {
                return None;
            };
            let quote = source[s.clone()].chars().next().unwrap_or('"');
            Some((
                s,
                words
                    .iter()
                    .map(|w| (format!("{quote}{w}{quote}"), p.doc.to_owned()))
                    .collect(),
            ))
        }
        (Call::Named { verb, written, .. }, None) => {
            let lexemes = lex::lex(source);
            let mut start = cursor;
            let bytes = source.as_bytes();
            while start > 0 && lex::ident_char(bytes[start - 1] as char) {
                start -= 1;
            }
            let before = lexemes
                .iter()
                .rev()
                .find(|l| l.range.end <= start)
                .map(|l| &source[l.range.clone()]);
            if !matches!(before, Some("{" | "," | ";")) {
                return None;
            }
            let typing = &source[start..cursor];
            let mut end = cursor;
            while end < bytes.len() && lex::ident_char(bytes[end] as char) {
                end += 1;
            }
            Some((
                start..end,
                typed_params(verb)
                    .filter(|p| !written.iter().any(|w| w == p.name && w != typing))
                    .map(|p| (p.name.to_owned(), p.doc.to_owned()))
                    .collect(),
            ))
        }
        _ => None,
    }
}
