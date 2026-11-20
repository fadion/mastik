//! Expressions in a prototype: a condition's test and *Set variable*'s value.
//!
//! Stored as source text so the field shows what was typed. A variable is written
//! `{<actor-hex>:<seq>}`, its id's wire form, so a rename never breaks one; the
//! field shows `{<name>}` instead ([`Expr::with_names`], [`Expr::from_names`]).
//!
//! The language is numbers, strings and booleans (a colour variable may be read
//! and compared): `+ - * / %` on numbers, `+` joining when either side is a
//! string, `== != < <= > >=`, `not`/`and`/`or`, parentheses, and the literals
//! `12`, `0.5`, `"text"` or `'text'`, `true`, `false`. Anything past that is a
//! script. Every error carries the byte range of the source it is about, for the
//! field to underline.

use crate::id::NodeId;
use crate::variable::{Value, VarId, VarKind};
use serde::{Deserialize, Serialize};
use std::ops::Range;

/// An expression's source.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(transparent)]
pub struct Expr(pub String);

/// Why an expression does not read or does not evaluate. Each names the byte
/// range of the source it is about.
#[derive(Clone, Debug, PartialEq, thiserror::Error)]
pub enum ExprError {
    #[error("{msg}")]
    Syntax { at: Range<usize>, msg: String },
    #[error("{msg}")]
    Type { at: Range<usize>, msg: String },
    #[error("the expression names a variable that is not there")]
    MissingVariable { at: Range<usize>, var: VarId },
    #[error("no variable is named “{name}”")]
    UnknownName { at: Range<usize>, name: String },
}

impl ExprError {
    /// The byte range of the source the error is about.
    pub fn at(&self) -> Range<usize> {
        match self {
            ExprError::Syntax { at, .. }
            | ExprError::Type { at, .. }
            | ExprError::MissingVariable { at, .. }
            | ExprError::UnknownName { at, .. } => at.clone(),
        }
    }
}

impl Expr {
    /// The variables it names, in the order written, repeats kept.
    pub fn vars(&self) -> Vec<VarId> {
        var_tokens(&self.0)
            .into_iter()
            .filter_map(|(_, inner)| NodeId::from_wire(inner).map(VarId))
            .collect()
    }

    /// The same expression with each variable it names by id passed through
    /// `f`: what a copy into another document's variables writes.
    pub fn map_vars(&self, f: &mut dyn FnMut(VarId) -> VarId) -> Expr {
        let mut out = String::with_capacity(self.0.len());
        let mut last = 0;
        for (at, inner) in var_tokens(&self.0) {
            let Some(id) = by_wire(inner) else { continue };
            out.push_str(&self.0[last..at.start]);
            out.push('{');
            out.push_str(&f(id).0.to_wire());
            out.push('}');
            last = at.end;
        }
        out.push_str(&self.0[last..]);
        Expr(out)
    }

    /// Whether it reads: `Ok` when it parses and every `{…}` is an id.
    pub fn check(&self) -> Result<(), ExprError> {
        parse(&self.0, &by_wire).map(|_| ())
    }

    /// The kind it comes to, each variable's kind read through `kind_of`, without
    /// evaluating it: what the field checks *Set variable*'s value against.
    pub fn kind(&self, kind_of: &dyn Fn(VarId) -> Option<VarKind>) -> Result<VarKind, ExprError> {
        type_of(&parse(&self.0, &by_wire)?, kind_of)
    }

    /// What it comes to with each variable read through `read`.
    pub fn eval(&self, read: &dyn Fn(VarId) -> Option<Value>) -> Result<Value, ExprError> {
        eval(&parse(&self.0, &by_wire)?, read)
    }

    /// The source as the field shows it: each `{<id>}` that `name_of` names
    /// written `{<name>}`; an id it does not name is left as written.
    pub fn with_names(&self, name_of: &dyn Fn(VarId) -> Option<String>) -> String {
        let mut out = String::with_capacity(self.0.len());
        let mut last = 0;
        for (at, inner) in var_tokens(&self.0) {
            let Some(name) = NodeId::from_wire(inner).and_then(|id| name_of(VarId(id))) else {
                continue;
            };
            out.push_str(&self.0[last..at.start]);
            out.push('{');
            out.push_str(&name);
            out.push('}');
            last = at.end;
        }
        out.push_str(&self.0[last..]);
        out
    }

    /// The source to store for `text` typed with names: each `{<name>}` that
    /// `id_of` knows written as its id, an id written as one kept. Answers the
    /// stored form whatever happens, and the first error in `text` (positions in
    /// `text`), so a field can keep a draft and underline it. A name `id_of` does
    /// not know is kept as written and is the error.
    pub fn from_names(
        text: &str,
        id_of: &dyn Fn(&str) -> Option<VarId>,
    ) -> (Expr, Option<ExprError>) {
        let lookup = |inner: &str| id_of(inner.trim()).or_else(|| by_wire(inner));
        let mut out = String::with_capacity(text.len());
        let mut last = 0;
        for (at, inner) in var_tokens(text) {
            let Some(id) = lookup(inner) else { continue };
            out.push_str(&text[last..at.start]);
            out.push('{');
            out.push_str(&id.0.to_wire());
            out.push('}');
            last = at.end;
        }
        out.push_str(&text[last..]);
        let error = parse(text, &lookup).err();
        (Expr(out), error)
    }
}

/// Every `{…}` in `text`: its byte range, braces included, and what is inside.
/// An unclosed brace ends the list.
pub fn var_tokens(text: &str) -> Vec<(Range<usize>, &str)> {
    let mut out = Vec::new();
    let mut from = 0;
    while let Some(open) = text[from..].find('{').map(|i| i + from) {
        let Some(close) = text[open + 1..].find('}').map(|i| i + open + 1) else {
            break;
        };
        out.push((open..close + 1, &text[open + 1..close]));
        from = close + 1;
    }
    out
}

fn by_wire(inner: &str) -> Option<VarId> {
    NodeId::from_wire(inner.trim()).map(VarId)
}

/// `n` as a joined string shows it: a whole number bare, else at most three
/// decimals with the trailing zeros trimmed.
pub fn format_number(n: f64) -> String {
    if !n.is_finite() || n.abs() >= 1e15 {
        return n.to_string();
    }
    let r = (n * 1000.0).round() / 1000.0;
    let s = format!("{r:.3}");
    let s = s.trim_end_matches('0').trim_end_matches('.');
    if s == "-0" { "0".into() } else { s.to_owned() }
}

/// `v` as a joined string shows it; a colour as `#rrggbb`, or `#rrggbbaa` when
/// not opaque.
pub fn format_value(v: &Value) -> String {
    match v {
        Value::Number(n) => format_number(*n),
        Value::String(s) => s.clone(),
        Value::Boolean(b) => b.to_string(),
        Value::Color(c) => {
            let [r, g, b, a] = c.to_rgba8().to_u8_array();
            match a {
                255 => format!("#{r:02x}{g:02x}{b:02x}"),
                _ => format!("#{r:02x}{g:02x}{b:02x}{a:02x}"),
            }
        }
    }
}

// ── Reading ──────────────────────────────────────────────────────────────────

#[derive(Clone, Debug, PartialEq)]
enum Tok {
    Num(f64),
    Str(String),
    Bool(bool),
    Var(VarId),
    Word(&'static str),
    Sym(&'static str),
}

const SYMS: [&str; 13] = [
    "==", "!=", "<=", ">=", "<", ">", "+", "-", "*", "/", "%", "(", ")",
];

fn lex(
    src: &str,
    var: &dyn Fn(&str) -> Option<VarId>,
) -> Result<Vec<(Tok, Range<usize>)>, ExprError> {
    let syntax = |at: Range<usize>, msg: &str| ExprError::Syntax {
        at,
        msg: msg.into(),
    };
    let bytes = src.as_bytes();
    let mut out = Vec::new();
    let mut i = 0;
    while i < src.len() {
        let c = src[i..].chars().next().unwrap_or(' ');
        let start = i;
        if c.is_whitespace() {
            i += c.len_utf8();
            continue;
        }
        if c.is_ascii_digit() || (c == '.' && bytes.get(i + 1).is_some_and(u8::is_ascii_digit)) {
            while i < src.len() && (bytes[i].is_ascii_digit() || bytes[i] == b'.') {
                i += 1;
            }
            let n: f64 = src[start..i]
                .parse()
                .map_err(|_| syntax(start..i, "not a number"))?;
            out.push((Tok::Num(n), start..i));
            continue;
        }
        if c == '"' || c == '\'' {
            let mut s = String::new();
            i += 1;
            let mut closed = false;
            while let Some(ch) = src[i..].chars().next() {
                i += ch.len_utf8();
                if ch == c {
                    closed = true;
                    break;
                }
                if ch == '\\' {
                    let Some(next) = src[i..].chars().next() else {
                        break;
                    };
                    i += next.len_utf8();
                    s.push(if next == 'n' { '\n' } else { next });
                    continue;
                }
                s.push(ch);
            }
            if !closed {
                return Err(syntax(start..src.len(), "this text has no closing quote"));
            }
            out.push((Tok::Str(s), start..i));
            continue;
        }
        if c == '{' {
            let Some(close) = src[i + 1..].find('}').map(|k| k + i + 1) else {
                return Err(syntax(start..src.len(), "this variable has no closing }"));
            };
            let inner = &src[i + 1..close];
            let at = start..close + 1;
            let id = var(inner).ok_or_else(|| ExprError::UnknownName {
                at: at.clone(),
                name: inner.trim().to_owned(),
            })?;
            out.push((Tok::Var(id), at));
            i = close + 1;
            continue;
        }
        if c.is_alphabetic() || c == '_' {
            while let Some(ch) = src[i..].chars().next() {
                if !(ch.is_alphanumeric() || ch == '_') {
                    break;
                }
                i += ch.len_utf8();
            }
            let tok = match &src[start..i] {
                "true" => Tok::Bool(true),
                "false" => Tok::Bool(false),
                "not" => Tok::Word("not"),
                "and" => Tok::Word("and"),
                "or" => Tok::Word("or"),
                w => {
                    return Err(syntax(
                        start..i,
                        &format!(
                            "“{w}” is not a word expressions know; a variable is written in {{ }}"
                        ),
                    ));
                }
            };
            out.push((tok, start..i));
            continue;
        }
        let Some(sym) = SYMS.iter().find(|s| src[i..].starts_with(**s)) else {
            let end = i + c.len_utf8();
            let msg = match c {
                '=' => "compare with ==".to_owned(),
                '!' => "write not".to_owned(),
                '&' => "write and".to_owned(),
                '|' => "write or".to_owned(),
                _ => format!("“{c}” has no meaning here"),
            };
            return Err(syntax(start..end, &msg));
        };
        i += sym.len();
        out.push((Tok::Sym(sym), start..i));
    }
    Ok(out)
}

#[derive(Clone, Debug, PartialEq)]
enum Ast {
    Lit(Value),
    Var(VarId),
    Neg(Box<Node>),
    Not(Box<Node>),
    Bin(&'static str, Box<Node>, Box<Node>),
}

#[derive(Clone, Debug, PartialEq)]
struct Node {
    ast: Ast,
    at: Range<usize>,
}

struct Parser {
    toks: Vec<(Tok, Range<usize>)>,
    pos: usize,
    end: usize,
}

impl Parser {
    fn peek(&self) -> Option<&Tok> {
        self.toks.get(self.pos).map(|t| &t.0)
    }

    fn here(&self) -> Range<usize> {
        self.toks
            .get(self.pos)
            .map_or(self.end..self.end, |t| t.1.clone())
    }

    fn eat(&mut self, want: &Tok) -> bool {
        let hit = self.peek() == Some(want);
        if hit {
            self.pos += 1;
        }
        hit
    }

    fn binary(
        &mut self,
        ops: &[Tok],
        next: fn(&mut Parser) -> Result<Node, ExprError>,
    ) -> Result<Node, ExprError> {
        let mut left = next(self)?;
        while let Some(op) = self.peek().filter(|t| ops.contains(*t)).cloned() {
            self.pos += 1;
            let right = next(self)?;
            let name = match op {
                Tok::Word(w) | Tok::Sym(w) => w,
                _ => unreachable!("operators are words and symbols"),
            };
            left = Node {
                at: left.at.start..right.at.end,
                ast: Ast::Bin(name, Box::new(left), Box::new(right)),
            };
        }
        Ok(left)
    }

    fn or(&mut self) -> Result<Node, ExprError> {
        self.binary(&[Tok::Word("or")], Parser::and)
    }

    fn and(&mut self) -> Result<Node, ExprError> {
        self.binary(&[Tok::Word("and")], Parser::not)
    }

    fn not(&mut self) -> Result<Node, ExprError> {
        let start = self.here().start;
        if self.eat(&Tok::Word("not")) {
            let inner = self.not()?;
            return Ok(Node {
                at: start..inner.at.end,
                ast: Ast::Not(Box::new(inner)),
            });
        }
        self.compare()
    }

    fn compare(&mut self) -> Result<Node, ExprError> {
        const CMP: [Tok; 6] = [
            Tok::Sym("=="),
            Tok::Sym("!="),
            Tok::Sym("<"),
            Tok::Sym("<="),
            Tok::Sym(">"),
            Tok::Sym(">="),
        ];
        let left = self.add()?;
        let Some(Tok::Sym(op)) = self.peek().filter(|t| CMP.contains(*t)).cloned() else {
            return Ok(left);
        };
        self.pos += 1;
        let right = self.add()?;
        if self.peek().is_some_and(|t| CMP.contains(t)) {
            return Err(ExprError::Syntax {
                at: self.here(),
                msg: "compare two values at a time; join comparisons with and".into(),
            });
        }
        Ok(Node {
            at: left.at.start..right.at.end,
            ast: Ast::Bin(op, Box::new(left), Box::new(right)),
        })
    }

    fn add(&mut self) -> Result<Node, ExprError> {
        self.binary(&[Tok::Sym("+"), Tok::Sym("-")], Parser::mul)
    }

    fn mul(&mut self) -> Result<Node, ExprError> {
        self.binary(
            &[Tok::Sym("*"), Tok::Sym("/"), Tok::Sym("%")],
            Parser::unary,
        )
    }

    fn unary(&mut self) -> Result<Node, ExprError> {
        let start = self.here().start;
        if self.eat(&Tok::Sym("-")) {
            let inner = self.unary()?;
            return Ok(Node {
                at: start..inner.at.end,
                ast: Ast::Neg(Box::new(inner)),
            });
        }
        self.primary()
    }

    fn primary(&mut self) -> Result<Node, ExprError> {
        let at = self.here();
        let Some(tok) = self.peek().cloned() else {
            return Err(ExprError::Syntax {
                at,
                msg: "a value is missing here".into(),
            });
        };
        self.pos += 1;
        let ast = match tok {
            Tok::Num(n) => Ast::Lit(Value::Number(n)),
            Tok::Str(s) => Ast::Lit(Value::String(s)),
            Tok::Bool(b) => Ast::Lit(Value::Boolean(b)),
            Tok::Var(v) => Ast::Var(v),
            Tok::Sym("(") => {
                let inner = self.or()?;
                let close = self.here();
                if !self.eat(&Tok::Sym(")")) {
                    return Err(ExprError::Syntax {
                        at: at.start..close.end.max(at.end),
                        msg: "this ( is never closed".into(),
                    });
                }
                return Ok(Node {
                    at: at.start..close.end,
                    ast: inner.ast,
                });
            }
            _ => {
                return Err(ExprError::Syntax {
                    at,
                    msg: "a value is missing before this".into(),
                });
            }
        };
        Ok(Node { ast, at })
    }
}

fn parse(src: &str, var: &dyn Fn(&str) -> Option<VarId>) -> Result<Node, ExprError> {
    let toks = lex(src, var)?;
    if toks.is_empty() {
        return Err(ExprError::Syntax {
            at: 0..src.len(),
            msg: "the expression is empty".into(),
        });
    }
    let mut p = Parser {
        toks,
        pos: 0,
        end: src.len(),
    };
    let node = p.or()?;
    if p.pos < p.toks.len() {
        return Err(ExprError::Syntax {
            at: p.here(),
            msg: "an operator is missing before this".into(),
        });
    }
    Ok(node)
}

// ── Kinds and values ─────────────────────────────────────────────────────────

fn kind_word(k: VarKind) -> &'static str {
    match k {
        VarKind::Color => "a colour",
        VarKind::Number => "a number",
        VarKind::Boolean => "true or false",
        VarKind::String => "text",
    }
}

fn kind_of_value(v: &Value) -> VarKind {
    match v {
        Value::Color(_) => VarKind::Color,
        Value::Number(_) => VarKind::Number,
        Value::Boolean(_) => VarKind::Boolean,
        Value::String(_) => VarKind::String,
    }
}

/// The kind `op` comes to over `l` and `r`, or why it does not apply. The one
/// rule both [`type_of`] and [`eval`] follow.
fn bin_kind(op: &str, l: VarKind, r: VarKind) -> Result<VarKind, String> {
    use VarKind::*;
    match op {
        "+" if l == String || r == String => match (l, r) {
            (Color, _) | (_, Color) => Err("a colour cannot be joined to text".into()),
            _ => Ok(String),
        },
        "+" | "-" | "*" | "/" | "%" => match (l, r) {
            (Number, Number) => Ok(Number),
            _ => Err(format!(
                "{op} needs two numbers, not {} and {}",
                kind_word(l),
                kind_word(r)
            )),
        },
        "==" | "!=" if l == r => Ok(Boolean),
        "==" | "!=" => Err(format!(
            "{} cannot be compared with {}",
            kind_word(l),
            kind_word(r)
        )),
        "<" | "<=" | ">" | ">=" => match (l, r) {
            (Number, Number) | (String, String) => Ok(Boolean),
            _ => Err(format!(
                "{op} compares two numbers or two texts, not {} and {}",
                kind_word(l),
                kind_word(r)
            )),
        },
        "and" | "or" => match (l, r) {
            (Boolean, Boolean) => Ok(Boolean),
            _ => Err(format!("{op} needs true or false on both sides")),
        },
        _ => Err(format!("{op} is not an operator")),
    }
}

fn type_of(n: &Node, kind_of: &dyn Fn(VarId) -> Option<VarKind>) -> Result<VarKind, ExprError> {
    let wrong = |msg: String| ExprError::Type {
        at: n.at.clone(),
        msg,
    };
    match &n.ast {
        Ast::Lit(v) => Ok(kind_of_value(v)),
        Ast::Var(v) => kind_of(*v).ok_or(ExprError::MissingVariable {
            at: n.at.clone(),
            var: *v,
        }),
        Ast::Neg(inner) => match type_of(inner, kind_of)? {
            VarKind::Number => Ok(VarKind::Number),
            k => Err(wrong(format!("- needs a number, not {}", kind_word(k)))),
        },
        Ast::Not(inner) => match type_of(inner, kind_of)? {
            VarKind::Boolean => Ok(VarKind::Boolean),
            k => Err(wrong(format!(
                "not needs true or false, not {}",
                kind_word(k)
            ))),
        },
        Ast::Bin(op, l, r) => {
            let (l, r) = (type_of(l, kind_of)?, type_of(r, kind_of)?);
            bin_kind(op, l, r).map_err(wrong)
        }
    }
}

fn eval(n: &Node, read: &dyn Fn(VarId) -> Option<Value>) -> Result<Value, ExprError> {
    let wrong = |msg: String| ExprError::Type {
        at: n.at.clone(),
        msg,
    };
    match &n.ast {
        Ast::Lit(v) => Ok(v.clone()),
        Ast::Var(v) => read(*v).ok_or(ExprError::MissingVariable {
            at: n.at.clone(),
            var: *v,
        }),
        Ast::Neg(inner) => match eval(inner, read)? {
            Value::Number(x) => Ok(Value::Number(-x)),
            v => Err(wrong(format!(
                "- needs a number, not {}",
                kind_word(kind_of_value(&v))
            ))),
        },
        Ast::Not(inner) => match eval(inner, read)? {
            Value::Boolean(b) => Ok(Value::Boolean(!b)),
            v => Err(wrong(format!(
                "not needs true or false, not {}",
                kind_word(kind_of_value(&v))
            ))),
        },
        Ast::Bin(op @ ("and" | "or"), l, r) => {
            let Value::Boolean(a) = eval(l, read)? else {
                return Err(wrong(format!("{op} needs true or false on both sides")));
            };
            // Short-circuit: the right side is not read once the left decides.
            if (*op == "and" && !a) || (*op == "or" && a) {
                return Ok(Value::Boolean(a));
            }
            match eval(r, read)? {
                Value::Boolean(b) => Ok(Value::Boolean(b)),
                _ => Err(wrong(format!("{op} needs true or false on both sides"))),
            }
        }
        Ast::Bin(op, l, r) => {
            let (a, b) = (eval(l, read)?, eval(r, read)?);
            bin_kind(op, kind_of_value(&a), kind_of_value(&b)).map_err(&wrong)?;
            Ok(match (*op, a, b) {
                ("+", a @ Value::String(_), b) | ("+", a, b @ Value::String(_)) => {
                    Value::String(format_value(&a) + &format_value(&b))
                }
                (op, Value::Number(x), Value::Number(y)) => match op {
                    "+" => Value::Number(x + y),
                    "-" => Value::Number(x - y),
                    "*" => Value::Number(x * y),
                    "/" | "%" if y == 0.0 => return Err(wrong("division by zero".into())),
                    "/" => Value::Number(x / y),
                    "%" => Value::Number(x.rem_euclid(y)),
                    "==" => Value::Boolean(x == y),
                    "!=" => Value::Boolean(x != y),
                    "<" => Value::Boolean(x < y),
                    "<=" => Value::Boolean(x <= y),
                    ">" => Value::Boolean(x > y),
                    _ => Value::Boolean(x >= y),
                },
                (op, Value::String(x), Value::String(y)) => Value::Boolean(match op {
                    "==" => x == y,
                    "!=" => x != y,
                    "<" => x < y,
                    "<=" => x <= y,
                    ">" => x > y,
                    _ => x >= y,
                }),
                ("==", a, b) => Value::Boolean(a == b),
                (_, a, b) => Value::Boolean(a != b),
            })
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use peniko::Color;

    fn var(seq: u64) -> VarId {
        VarId(NodeId { actor: 0xab, seq })
    }

    fn wire(seq: u64) -> String {
        format!("{{{}}}", var(seq).0.to_wire())
    }

    fn run(src: &str) -> Result<Value, ExprError> {
        let read = |v: VarId| match v.0.seq {
            1 => Some(Value::Number(4.0)),
            2 => Some(Value::String("Ada".into())),
            3 => Some(Value::Boolean(true)),
            4 => Some(Value::Color(Color::from_rgba8(255, 0, 0, 255))),
            _ => None,
        };
        Expr(src.into()).eval(&read)
    }

    /// Operators bind tighter in the usual order: `*` over `+` over comparison
    /// over `not` over `and` over `or`.
    ///
    /// flip: `Parser::add` calling `unary` instead of `mul` → fails at "precedence".
    #[test]
    fn arithmetic_follows_the_usual_precedence() {
        assert_eq!(run("1 + 2 * 3"), Ok(Value::Number(7.0)), "precedence");
        assert_eq!(run("(1 + 2) * 3"), Ok(Value::Number(9.0)));
        assert_eq!(run("-2 * -3"), Ok(Value::Number(6.0)));
        assert_eq!(run("7 % 4"), Ok(Value::Number(3.0)));
        assert_eq!(run("10 - 4 - 3"), Ok(Value::Number(3.0)), "left to right");
        assert_eq!(run("not 1 > 2 and true or false"), Ok(Value::Boolean(true)));
        assert_eq!(
            run("false or not false and false"),
            Ok(Value::Boolean(false))
        );
    }

    /// Variables are read by id; `+` joins when either side is text.
    ///
    /// flip: `format_number` writing `4.0` for a whole number → fails at "joined".
    #[test]
    fn variables_read_and_text_joins() {
        let src = format!("\"Hi \" + {} + \", \" + {}", wire(2), wire(1));
        assert_eq!(run(&src), Ok(Value::String("Hi Ada, 4".into())), "joined");
        assert_eq!(run(&format!("{} * 2.5", wire(1))), Ok(Value::Number(10.0)));
        assert_eq!(
            run(&format!("{} == true", wire(3))),
            Ok(Value::Boolean(true))
        );
        assert_eq!(
            run(&format!("{} == {}", wire(4), wire(4))),
            Ok(Value::Boolean(true)),
            "colours compare"
        );
        assert_eq!(run("'a' < 'b'"), Ok(Value::Boolean(true)));
        assert_eq!(run("0.1 + 0.2 + ''"), Ok(Value::String("0.3".into())));
    }

    /// `and` does not read its right side once the left is false, so a missing
    /// variable there is no error.
    ///
    /// flip: `eval`'s short-circuit removed → fails at "short-circuit".
    #[test]
    fn and_and_or_short_circuit() {
        let src = format!("false and {}", wire(9));
        assert_eq!(run(&src), Ok(Value::Boolean(false)), "short-circuit");
        let src = format!("true and {}", wire(9));
        assert!(matches!(run(&src), Err(ExprError::MissingVariable { .. })));
    }

    /// Each error names the range of source it is about.
    ///
    /// flip: `eval`'s operator check reporting the left operand's range → fails at
    /// "the whole operation".
    #[test]
    fn errors_carry_the_range_they_are_about() {
        let e = Expr("1 + true".into()).check();
        assert_eq!(e, Ok(()), "it parses");
        let e = run("1 + true").unwrap_err();
        assert_eq!(e.at(), 0..8, "the whole operation");
        let e = Expr("1 +".into()).check().unwrap_err();
        assert_eq!(e.at(), 3..3, "the missing value is at the end");
        let e = Expr("(1 + 2".into()).check().unwrap_err();
        assert!(matches!(e, ExprError::Syntax { .. }));
        let e = Expr("1 = 1".into()).check().unwrap_err();
        assert_eq!(e.at(), 2..3);
        assert_eq!(e.to_string(), "compare with ==");
        let e = Expr("{nope} + 1".into()).check().unwrap_err();
        assert_eq!(
            e,
            ExprError::UnknownName {
                at: 0..6,
                name: "nope".into()
            }
        );
        let e = Expr("1 < 2 < 3".into()).check().unwrap_err();
        assert_eq!(e.at(), 6..7, "a chained comparison");
        assert!(Expr("".into()).check().is_err(), "empty");
        assert!(matches!(run("1 / 0"), Err(ExprError::Type { .. })));
    }

    /// The static kind follows the same rule as evaluation.
    ///
    /// flip: `bin_kind` answering `Number` for `+` with a string → fails at "join".
    #[test]
    fn the_kind_is_known_without_evaluating() {
        let kind_of = |v: VarId| match v.0.seq {
            1 => Some(VarKind::Number),
            2 => Some(VarKind::String),
            _ => None,
        };
        let k = |s: String| Expr(s).kind(&kind_of);
        assert_eq!(k(format!("{} + 1", wire(1))), Ok(VarKind::Number));
        assert_eq!(k(format!("{} + 1", wire(2))), Ok(VarKind::String), "join");
        assert_eq!(k(format!("{} > 1", wire(1))), Ok(VarKind::Boolean));
        assert!(matches!(
            k(format!("{} * 2", wire(2))),
            Err(ExprError::Type { .. })
        ));
        assert!(matches!(k(wire(7)), Err(ExprError::MissingVariable { .. })));
    }

    /// The field's names go to ids and back, and an unknown name is kept as typed
    /// and reported where it is.
    ///
    /// flip: `from_names` skipping the id write → fails at "stored by id".
    #[test]
    fn names_round_trip_through_ids() {
        let name_of = |v: VarId| (v.0.seq == 1).then(|| "cart/count".to_owned());
        let id_of = |n: &str| (n == "cart/count").then(|| var(1));
        let stored = Expr(format!("{} + 1", wire(1)));
        let shown = stored.with_names(&name_of);
        assert_eq!(shown, "{cart/count} + 1");
        let (back, err) = Expr::from_names(&shown, &id_of);
        assert_eq!(back, stored, "stored by id");
        assert_eq!(err, None);
        let (kept, err) = Expr::from_names("{cart/count} + {gone}", &id_of);
        assert_eq!(kept.0, format!("{} + {{gone}}", wire(1)), "kept as typed");
        assert_eq!(err.map(|e| e.at()), Some(15..21));
        assert_eq!(stored.vars(), vec![var(1)]);
    }
}
