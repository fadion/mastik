//! Command Mode's language: Luau, sandboxed, over the document.
//!
//! A bar line is rewritten into Luau by [`desugar`] and run by [`run_line`], or read
//! as it is typed by [`eval_read`]; a script runs as written by [`run_script`]. Each
//! reaches the document through the caller's [`Host`]: reads through its document,
//! writes as registry verbs whose edits the host commits one by one, so later reads
//! see earlier writes and the host can join them into one undo step.

#![deny(rustdoc::broken_intra_doc_links, rustdoc::invalid_html_tags)]
#![allow(rustdoc::private_intra_doc_links)]

use std::ops::Range;

use mastik_core::{Document, NodeId, Resolved};
use rustc_hash::FxHashMap;

mod complete;
mod defs;
mod header;
mod help;
mod lex;
mod props;
mod run;
mod sugar;

#[cfg(test)]
mod tests;

pub use complete::{Completion, Row, Section, complete, complete_script};
pub use defs::{Tok, as_read, definitions, names_used, tokens};
pub use header::{Header, Input, InputType, header};
pub use help::{
    ACTS_ON_SELECTION, Hover, Problem, Severity, SigParam, Signature, check, declaration_at,
    hover_at, signature_at, syntax_error,
};
pub use props::{METHODS, Method, PROPERTIES, PropType, Property};
pub use run::{Budget, READ_BUDGET, RUN_BUDGET};
pub use sugar::{Line, LineKind, Marks, desugar, invisible, marks, quoted};

/// What the language reaches the document through: the app's session, or a test's
/// plain document.
pub trait Host {
    fn doc(&self) -> &Document;
    fn resolved(&self) -> &Resolved;
    /// The current page, for layer-name lookup.
    fn page(&self) -> NodeId;
    /// Answer verb `name`'s edit (`verbs::call` with the host's `IdSource` and lent
    /// sinks), not committed.
    fn verb(
        &mut self,
        name: &str,
        args: &serde_json::Value,
    ) -> Result<mastik_core::verbs::Done, String>;
    /// Commit `tx` now (the host propagates what components owe and applies); later
    /// reads see it.
    fn commit(&mut self, tx: mastik_core::Transaction) -> Result<(), String>;
}

/// What a run sees besides the document: the selection, the leap labels bound
/// this session, and the script's input values.
pub struct Env<'a> {
    pub selection: &'a [NodeId],
    /// Lowercase leap labels (`f`, `sk`) to the layers they were shown on.
    pub labels: &'a FxHashMap<String, NodeId>,
    pub params: serde_json::Map<String, serde_json::Value>,
}

/// What a run did besides its edits, which the host already holds.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Outcome {
    /// Each `print`, and each line a verb said about its edit.
    pub printed: Vec<String>,
    /// A read line's value, or what a script returned, as the bar shows it.
    pub value: Option<String>,
    /// Every layer the run created, in order.
    pub created: Vec<NodeId>,
    /// What to select once the run ends: what the last creating verb made, or
    /// what `:select()` named last; `None` leaves the selection alone.
    pub selection: Option<Vec<NodeId>>,
}

/// Why a run stopped.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ErrorKind {
    /// The text is wrong or what it asked was refused.
    Error,
    /// A budget ran out: time, steps or memory.
    Budget,
}

/// A failed run: what went wrong and where, in the text the caller passed.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct VmError {
    pub kind: ErrorKind,
    pub message: String,
    /// The line, from 1, when known.
    pub line: Option<u32>,
    /// The byte range of the offending token in the text the caller passed.
    pub span: Option<Range<usize>>,
    /// What Tab replaces: a range of the text and its replacement.
    pub fix: Option<(Range<usize>, String)>,
}

impl std::fmt::Display for VmError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self.line {
            Some(n) => write!(f, "line {n}: {}", self.message),
            None => f.write_str(&self.message),
        }
    }
}

impl std::error::Error for VmError {}

/// Evaluate bar `line` for its value as it is typed, under [`READ_BUDGET`]: any
/// attempt to change the document is an error, so nothing changes. `None` for nil.
pub fn eval_read(host: &mut dyn Host, env: &Env, line: &str) -> Result<Option<String>, VmError> {
    let luau = desugar(line).luau;
    run::run(host, env, &luau, line, run::Mode::Read, true).map(|o| o.value)
}

/// Run bar `line` through [`desugar`] under [`RUN_BUDGET`]; a read line's value
/// lands in [`Outcome::value`].
pub fn run_line(host: &mut dyn Host, env: &Env, line: &str) -> Result<Outcome, VmError> {
    let desugared = desugar(line);
    let expression = desugared.kind == LineKind::Read;
    run::run(host, env, &desugared.luau, line, run::Mode::Bar, expression)
}

/// Run a whole script as written, with no line sugar, under [`RUN_BUDGET`].
pub fn run_script(host: &mut dyn Host, env: &Env, source: &str) -> Result<Outcome, VmError> {
    run::run(host, env, source, source, run::Mode::Script, false)
}

/// `n` as the bar shows it: a whole number bare, else at most three decimals
/// with the trailing zeros trimmed.
pub(crate) fn format_number(n: f64) -> String {
    if !n.is_finite() {
        return n.to_string();
    }
    let r = (n * 1000.0).round() / 1000.0;
    let s = format!("{r:.3}");
    let s = s.trim_end_matches('0').trim_end_matches('.');
    if s == "-0" { "0".into() } else { s.to_owned() }
}

/// A collection as the bar names it: "Card", "Card and 2 more", "no layers".
pub(crate) fn describe(doc: &Document, ids: &[NodeId]) -> String {
    let name = |id: NodeId| {
        doc.get(id)
            .map_or_else(|| id.to_wire(), |n| n.name().to_owned())
    };
    match ids {
        [] => "no layers".into(),
        [one] => name(*one),
        [first, rest @ ..] => format!("{} and {} more", name(*first), rest.len()),
    }
}

/// The names of the layers under `page` that Luau can spell as a bare name.
pub(crate) fn layer_names(doc: &Document, page: NodeId) -> Vec<String> {
    let mut out = Vec::new();
    let mut stack: Vec<NodeId> = doc
        .get(page)
        .map(|p| p.children().to_vec())
        .unwrap_or_default();
    while let Some(id) = stack.pop() {
        let Some(node) = doc.get(id) else { continue };
        let name = node.name();
        if name.starts_with(lex::ident_start)
            && name.chars().all(lex::ident_char)
            && !lex::KEYWORDS.contains(&name)
            && !out.iter().any(|n| n == name)
        {
            out.push(name.to_owned());
        }
        stack.extend(node.children());
    }
    out
}
