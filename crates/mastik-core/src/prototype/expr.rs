//! Expressions in a prototype: a condition's test, *Set variable*'s value and a
//! row action's cells.
//!
//! Stored as source text so the field shows what was typed. A variable is written
//! `{<actor-hex>:<seq>}`, its id's wire form, so a rename never breaks one; the
//! field shows `{<name>}` instead ([`Expr::with_names`], [`Expr::from_names`]).
//! A table is read the same way, by ids ([`Ref`]): the field shows
//! `{Cart.rows}`, `{Products.3.Name}`, `{Cart.Line.sum}` or `{row.Qty}`, and
//! typed names resolve against the real table and column names ([`Names`]),
//! since a table, a column and a variable may each contain a dot.
//!
//! The language is numbers, strings and booleans (a colour variable may be read
//! and compared): `+ - * / %` on numbers, `+` joining when either side is a
//! string, `== != < <= > >=`, `not`/`and`/`or`, parentheses, and the literals
//! `12`, `0.5`, `"text"` or `'text'`, `true`, `false`. Anything past that is a
//! script. Every error carries the byte range of the source it is about, for the
//! field to underline.

use crate::chart::{ColumnId, parse_number};
use crate::id::NodeId;
use crate::list::{DataSet, DataSetId};
use crate::variable::{Value, VarId, VarKind};
use serde::{Deserialize, Serialize};
use std::ops::Range;

/// What a `{…}` in an expression names.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Ref {
    Var(VarId),
    /// How many rows the table has: `{<table>:rows}`.
    Rows(DataSetId),
    /// One cell: `{<table>:<row>:<column>}`.
    Cell(DataSetId, RowAt, ColumnId),
    /// A column's number cells taken together: `{<table>:<column>:<agg>}`.
    Total(DataSetId, ColumnId, Agg),
    /// A cell of the row a row action is on — the one *Edit row* writes, or
    /// the one a *First row where…* test is looking at: `{row:<column>}`.
    Row(ColumnId),
}

/// Which row of a table a [`Ref::Cell`] reads.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum RowAt {
    /// The row an *Opened row* list over the table shows: row 1 until a tap
    /// opens another.
    Opened,
    First,
    Last,
    /// Counted from 1, as the data window's gutter counts.
    Nth(u32),
}

impl RowAt {
    /// Its word in an expression, stored and shown alike.
    pub fn word(self) -> String {
        match self {
            RowAt::Opened => "opened".into(),
            RowAt::First => "first".into(),
            RowAt::Last => "last".into(),
            RowAt::Nth(n) => n.to_string(),
        }
    }

    fn from_word(w: &str) -> Option<RowAt> {
        match w {
            "opened" => Some(RowAt::Opened),
            "first" => Some(RowAt::First),
            "last" => Some(RowAt::Last),
            n => n.parse().ok().filter(|n| *n >= 1).map(RowAt::Nth),
        }
    }
}

/// How a [`Ref::Total`] takes a column's cells together.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Agg {
    Sum,
    Min,
    Max,
    Avg,
}

impl Agg {
    pub const ALL: [Agg; 4] = [Agg::Sum, Agg::Min, Agg::Max, Agg::Avg];

    /// Its word in an expression, stored and shown alike.
    pub fn word(self) -> &'static str {
        match self {
            Agg::Sum => "sum",
            Agg::Min => "min",
            Agg::Max => "max",
            Agg::Avg => "avg",
        }
    }

    fn from_word(w: &str) -> Option<Agg> {
        Agg::ALL.into_iter().find(|a| a.word() == w)
    }
}

impl Ref {
    /// The stored form, without its braces.
    pub fn wire(&self) -> String {
        match self {
            Ref::Var(v) => v.0.to_wire(),
            Ref::Rows(t) => format!("{}:rows", t.0.to_wire()),
            Ref::Cell(t, at, c) => format!("{}:{}:{}", t.0.to_wire(), at.word(), c.0),
            Ref::Total(t, c, a) => format!("{}:{}:{}", t.0.to_wire(), c.0, a.word()),
            Ref::Row(c) => format!("row:{}", c.0),
        }
    }

    /// What a stored `{…}` names, `None` when it is no stored form.
    pub fn from_wire(inner: &str) -> Option<Ref> {
        let s = inner.trim();
        let table = |a: &str, q: &str| NodeId::from_wire(&format!("{a}:{q}")).map(DataSetId);
        let column = |c: &str| c.parse().ok().map(ColumnId);
        match s.split(':').collect::<Vec<_>>().as_slice() {
            ["row", c] => Some(Ref::Row(column(c)?)),
            [_] | [_, _] => NodeId::from_wire(s).map(|id| Ref::Var(VarId(id))),
            [a, q, "rows"] => Some(Ref::Rows(table(a, q)?)),
            [a, q, c, agg] if Agg::from_word(agg).is_some() => {
                Some(Ref::Total(table(a, q)?, column(c)?, Agg::from_word(agg)?))
            }
            [a, q, at, c] => Some(Ref::Cell(table(a, q)?, RowAt::from_word(at)?, column(c)?)),
            _ => None,
        }
    }

    /// The table it reads, `None` for a variable or `row`.
    pub fn table(&self) -> Option<DataSetId> {
        match self {
            Ref::Rows(t) | Ref::Cell(t, ..) | Ref::Total(t, ..) => Some(*t),
            Ref::Var(_) | Ref::Row(_) => None,
        }
    }
}

/// Why a [`Ref`] gives no value or kind.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Unread {
    /// The variable, table or column it names is not there.
    Missing,
    /// It is there but cannot be read now, and why (*Cart has no rows*).
    Why(String),
}

/// Why a typed name names nothing.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Lookup {
    Unknown,
    Why(String),
}

/// Why a typed name two variables share is refused: no list can tell them
/// apart by name.
fn shared_name(name: &str) -> Lookup {
    Lookup::Why(format!(
        "two variables are named “{name}”; rename one to read it here"
    ))
}

/// What typed names are read against: the document's variables and tables,
/// and the table `row` reads inside a row action.
pub struct Names<'a> {
    /// Every variable a typed name may mean (`Variables::ids_named`).
    pub var_id: &'a dyn Fn(&str) -> Vec<VarId>,
    /// A variable's name as shown, `None` to keep its id
    /// (`Variables::expr_name`).
    pub var_name: &'a dyn Fn(VarId) -> Option<String>,
    pub tables: &'a [DataSet],
    /// The row action's table: what `{row.<column>}` reads. `None` outside one.
    pub row: Option<DataSetId>,
}

/// The name `row` reads the acted-on row by; no table may take it.
pub const ROW: &str = "row";

impl Names<'_> {
    fn table(&self, id: DataSetId) -> Option<&DataSet> {
        self.tables.iter().find(|d| d.id == id)
    }

    /// `r` as the field shows it, inside the braces; `None` when what it names
    /// is not there.
    pub fn name(&self, r: &Ref) -> Option<String> {
        let col = |t: &DataSet, c: ColumnId| t.table.column(c).map(|c| c.name.clone());
        match *r {
            Ref::Var(v) => (self.var_name)(v),
            Ref::Rows(t) => Some(format!("{}.rows", self.table(t)?.name)),
            Ref::Cell(t, at, c) => {
                let t = self.table(t)?;
                Some(format!("{}.{}.{}", t.name, at.word(), col(t, c)?))
            }
            Ref::Total(t, c, a) => {
                let t = self.table(t)?;
                Some(format!("{}.{}.{}", t.name, col(t, c)?, a.word()))
            }
            Ref::Row(c) => Some(format!("{ROW}.{}", col(self.table(self.row?)?, c)?)),
        }
    }

    /// What a typed `{…}` names: a variable by name, a table read by its
    /// table's and column's names, or a stored form. A name that reads two
    /// ways is refused rather than guessed.
    pub fn resolve(&self, inner: &str) -> Result<Ref, Lookup> {
        let s = inner.trim();
        let mut found: Vec<Ref> = Vec::new();
        fn push(found: &mut Vec<Ref>, r: Ref) {
            if !found.contains(&r) {
                found.push(r);
            }
        }
        match (self.var_id)(s).as_slice() {
            [] => {}
            [v] => push(&mut found, Ref::Var(*v)),
            _ => return Err(shared_name(s)),
        }
        if let Some(r) = Ref::from_wire(s) {
            push(&mut found, r);
        }
        let col = |t: &DataSet, name: &str| {
            t.table
                .columns
                .iter()
                .find(|c| c.name == name)
                .map(|c| c.id)
        };
        let mut hint = None;
        if let Some(rest) = s.strip_prefix(ROW).and_then(|r| r.strip_prefix('.')) {
            match self.row.and_then(|t| self.table(t)) {
                Some(t) => match col(t, rest) {
                    Some(c) => push(&mut found, Ref::Row(c)),
                    None => hint = Some(format!("{} has no column “{rest}”", t.name)),
                },
                None => hint = Some("row is read only inside a row action".into()),
            }
        }
        for t in self.tables {
            if s == t.name {
                hint = Some(format!(
                    "“{s}” is a table; read it with a dot, as {{{s}.rows}}"
                ));
                continue;
            }
            let Some(rest) = s
                .strip_prefix(t.name.as_str())
                .and_then(|r| r.strip_prefix('.'))
            else {
                continue;
            };
            let before = found.len();
            if rest == "rows" {
                push(&mut found, Ref::Rows(t.id));
            }
            if let Some((at, c)) = rest.split_once('.')
                && let (Some(at), Some(c)) = (RowAt::from_word(at), col(t, c))
            {
                push(&mut found, Ref::Cell(t.id, at, c));
            }
            if let Some((c, a)) = rest.rsplit_once('.')
                && let (Some(c), Some(a)) = (col(t, c), Agg::from_word(a))
            {
                push(&mut found, Ref::Total(t.id, c, a));
            }
            if found.len() == before {
                hint.get_or_insert_with(|| format!("{} holds no “{rest}”", t.name));
            }
        }
        match found.as_slice() {
            [one] => Ok(*one),
            [] => Err(hint.map_or(Lookup::Unknown, Lookup::Why)),
            _ => Err(Lookup::Why(format!(
                "“{s}” reads two ways; pick it from the @ list"
            ))),
        }
    }
}

/// Where a table read looks in play: the tables, the row each one's *Opened
/// row* lists show, and the row a row action is on.
pub struct TableEnv<'a> {
    pub tables: &'a [DataSet],
    pub opened: &'a dyn Fn(DataSetId) -> usize,
    /// The row action's table and the cells of the row it is on.
    pub row: Option<(DataSetId, &'a [String])>,
}

/// A cell as an expression reads it: a number when it parses as one, else text.
pub fn cell_value(cell: &str) -> Value {
    parse_number(cell).map_or_else(|| Value::String(cell.to_owned()), Value::Number)
}

/// How many rows, said as a count: *1 row*, *3 rows*.
fn rows_word(n: usize) -> String {
    match n {
        1 => "1 row".into(),
        n => format!("{n} rows"),
    }
}

impl TableEnv<'_> {
    /// What table read `r` comes to now. A variable is not a table read and
    /// reads as missing here.
    pub fn read(&self, r: &Ref) -> Result<Value, Unread> {
        let set = |t: DataSetId| {
            self.tables
                .iter()
                .find(|d| d.id == t)
                .ok_or(Unread::Missing)
        };
        let at = |d: &DataSet, c: ColumnId| d.table.index_of(c).ok_or(Unread::Missing);
        match *r {
            Ref::Var(_) => Err(Unread::Missing),
            Ref::Rows(t) => Ok(Value::Number(set(t)?.table.rows.len() as f64)),
            Ref::Cell(..) | Ref::Row(_) => Ok(cell_value(self.written(r)?.unwrap_or_default())),
            Ref::Total(t, c, agg) => {
                let d = set(t)?;
                at(d, c)?;
                let nums: Vec<f64> = d.table.cells(c).filter_map(parse_number).collect();
                if nums.is_empty() && agg != Agg::Sum {
                    let col = d.table.column(c).map_or("", |c| c.name.as_str());
                    return Err(Unread::Why(format!(
                        "{} of no numbers: {} has no numbers in {col}",
                        agg.word(),
                        d.name
                    )));
                }
                let n = nums.len() as f64;
                Ok(Value::Number(match agg {
                    Agg::Sum => nums.iter().sum(),
                    Agg::Min => nums.iter().copied().fold(f64::INFINITY, f64::min),
                    Agg::Max => nums.iter().copied().fold(f64::NEG_INFINITY, f64::max),
                    Agg::Avg => nums.iter().sum::<f64>() / n,
                }))
            }
        }
    }

    /// The text of the cell read `r` names, as written; `None` when `r` reads
    /// no one cell (a count, a total, a variable). A row action copying a cell
    /// writes this, so `12.50` and `007` keep their spelling.
    pub fn written(&self, r: &Ref) -> Result<Option<&str>, Unread> {
        let set = |t: DataSetId| {
            self.tables
                .iter()
                .find(|d| d.id == t)
                .ok_or(Unread::Missing)
        };
        let at = |d: &DataSet, c: ColumnId| d.table.index_of(c).ok_or(Unread::Missing);
        match *r {
            Ref::Cell(t, which, c) => {
                let d = set(t)?;
                let i = at(d, c)?;
                let n = d.table.rows.len();
                let empty = || Unread::Why(format!("{} has no rows", d.name));
                let row = match which {
                    RowAt::First if n > 0 => 0,
                    RowAt::Last if n > 0 => n - 1,
                    RowAt::Opened if n > 0 => (self.opened)(t).min(n - 1),
                    RowAt::Nth(k) if (k as usize) <= n => k as usize - 1,
                    RowAt::Nth(k) => {
                        return Err(Unread::Why(format!(
                            "{} has {}, not {k}",
                            d.name,
                            rows_word(n)
                        )));
                    }
                    _ => return Err(empty()),
                };
                Ok(Some(d.table.rows[row][i].as_str()))
            }
            Ref::Row(c) => {
                let Some((t, cells)) = self.row else {
                    return Err(Unread::Why("row is read only inside a row action".into()));
                };
                let i = at(set(t)?, c)?;
                Ok(Some(cells.get(i).map_or("", String::as_str)))
            }
            Ref::Var(_) | Ref::Rows(_) | Ref::Total(..) => Ok(None),
        }
    }
}

/// The kind table read `r` comes to without evaluating it: a count and a total
/// are numbers, and a cell is a number or text by its data in play, so it has
/// no kind while authoring. Missing when its table or column is not in
/// `tables`; `row` is refused outside a row action (`row: None`).
pub fn table_kind(
    tables: &[DataSet],
    row: Option<DataSetId>,
    r: &Ref,
) -> Result<Option<VarKind>, Unread> {
    let has = |t: DataSetId, c: Option<ColumnId>| {
        tables
            .iter()
            .find(|d| d.id == t)
            .is_some_and(|d| c.is_none_or(|c| d.table.index_of(c).is_some()))
    };
    let there = match *r {
        Ref::Var(_) => false,
        Ref::Rows(t) => has(t, None),
        Ref::Cell(t, _, c) | Ref::Total(t, c, _) => has(t, Some(c)),
        Ref::Row(c) => match row {
            Some(t) => has(t, Some(c)),
            None => return Err(Unread::Why("row is read only inside a row action".into())),
        },
    };
    match (there, r) {
        (false, _) => Err(Unread::Missing),
        (true, Ref::Rows(_) | Ref::Total(..)) => Ok(Some(VarKind::Number)),
        (true, _) => Ok(None),
    }
}

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
    #[error("the expression names a table or column that is not there")]
    MissingTable { at: Range<usize> },
}

impl ExprError {
    /// The byte range of the source the error is about.
    pub fn at(&self) -> Range<usize> {
        match self {
            ExprError::Syntax { at, .. }
            | ExprError::Type { at, .. }
            | ExprError::MissingVariable { at, .. }
            | ExprError::UnknownName { at, .. }
            | ExprError::MissingTable { at } => at.clone(),
        }
    }

    fn unread(why: Unread, r: &Ref, at: &Range<usize>) -> ExprError {
        let at = at.clone();
        match (why, r) {
            (Unread::Missing, Ref::Var(var)) => ExprError::MissingVariable { at, var: *var },
            (Unread::Missing, _) => ExprError::MissingTable { at },
            (Unread::Why(msg), _) => ExprError::Type { at, msg },
        }
    }
}

impl Expr {
    /// The variables it names, in the order written, repeats kept.
    pub fn vars(&self) -> Vec<VarId> {
        self.refs()
            .into_iter()
            .filter_map(|r| match r {
                Ref::Var(v) => Some(v),
                _ => None,
            })
            .collect()
    }

    /// Everything it names by id, in the order written, repeats kept.
    pub fn refs(&self) -> Vec<Ref> {
        var_tokens(&self.0)
            .into_iter()
            .filter_map(|(_, inner)| Ref::from_wire(inner))
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

    /// Whether it reads: `Ok` when it parses and every `{…}` is a stored form.
    pub fn check(&self) -> Result<(), ExprError> {
        parse(&self.0, &stored).map(|_| ())
    }

    /// The kind it comes to without evaluating it, each name's kind read
    /// through `kind_of` (`None`: known only in play, as a table's cell): what
    /// the field checks *Set variable*'s value against.
    pub fn kind(
        &self,
        kind_of: &dyn Fn(&Ref) -> Result<Option<VarKind>, Unread>,
    ) -> Result<Option<VarKind>, ExprError> {
        type_of(&parse(&self.0, &stored)?, kind_of)
    }

    /// What it comes to with each variable read through `read`; a table read
    /// is missing.
    pub fn eval(&self, read: &dyn Fn(VarId) -> Option<Value>) -> Result<Value, ExprError> {
        self.eval_with(&|r| match r {
            Ref::Var(v) => read(*v).ok_or(Unread::Missing),
            _ => Err(Unread::Missing),
        })
    }

    /// What it comes to with everything it names read through `read`.
    pub fn eval_with(
        &self,
        read: &dyn Fn(&Ref) -> Result<Value, Unread>,
    ) -> Result<Value, ExprError> {
        eval(&parse(&self.0, &stored)?, read)
    }

    /// The one thing it names when it is nothing but that read (`{Products.opened.Price}`,
    /// parentheses aside): what a row action copies as written.
    pub fn bare_ref(&self) -> Option<Ref> {
        match parse(&self.0, &stored).ok()?.ast {
            Ast::Ref(r) => Some(r),
            _ => None,
        }
    }

    /// The source as the field shows it: each `{<id>}` that `name_of` names
    /// written `{<name>}`; an id it does not name is left as written.
    pub fn with_names(&self, name_of: &dyn Fn(VarId) -> Option<String>) -> String {
        self.shown(&|r| match r {
            Ref::Var(v) => name_of(*v),
            _ => None,
        })
    }

    /// [`Self::with_names`] with tables named too.
    pub fn with_names_in(&self, names: &Names) -> String {
        self.shown(&|r| names.name(r))
    }

    /// Each id `name_of` names written as its name; a name holding `}` or
    /// leading or trailing space would not read back, so it keeps its id.
    fn shown(&self, name_of: &dyn Fn(&Ref) -> Option<String>) -> String {
        let mut out = String::with_capacity(self.0.len());
        let mut last = 0;
        for (at, inner) in var_tokens(&self.0) {
            let Some(name) = Ref::from_wire(inner)
                .and_then(|r| name_of(&r))
                .filter(|n| !n.contains('}') && n.trim() == n)
            else {
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
    pub fn from_names(text: &str, id_of: &dyn Fn(&str) -> Vec<VarId>) -> (Expr, Option<ExprError>) {
        Self::stored_from(text, &|inner| match id_of(inner.trim()).as_slice() {
            [v] => Ok(Ref::Var(*v)),
            [] => by_wire(inner).map(Ref::Var).ok_or(Lookup::Unknown),
            _ => Err(shared_name(inner.trim())),
        })
    }

    /// [`Self::from_names`] with table reads resolved too ([`Names::resolve`]).
    pub fn from_names_in(text: &str, names: &Names) -> (Expr, Option<ExprError>) {
        Self::stored_from(text, &|inner| names.resolve(inner))
    }

    fn stored_from(
        text: &str,
        lookup: &dyn Fn(&str) -> Result<Ref, Lookup>,
    ) -> (Expr, Option<ExprError>) {
        let mut out = String::with_capacity(text.len());
        let mut last = 0;
        for (at, inner) in var_tokens(text) {
            let Ok(r) = lookup(inner) else { continue };
            out.push_str(&text[last..at.start]);
            out.push('{');
            out.push_str(&r.wire());
            out.push('}');
            last = at.end;
        }
        out.push_str(&text[last..]);
        let error = parse(text, lookup).err();
        (Expr(out), error)
    }
}

/// A stored `{…}`'s lookup: what [`Ref::from_wire`] reads.
fn stored(inner: &str) -> Result<Ref, Lookup> {
    Ref::from_wire(inner).ok_or(Lookup::Unknown)
}

/// Every `{…}` in `text` outside quoted text: its byte range, braces included,
/// and what is inside. Quotes and `\` escapes are read as `lex` reads them, so
/// the stored form and the parse agree on which braces are names. An unclosed
/// brace or quote ends the list.
pub fn var_tokens(text: &str) -> Vec<(Range<usize>, &str)> {
    let mut out = Vec::new();
    let mut i = 0;
    while let Some(c) = text[i..].chars().next() {
        let start = i;
        i += c.len_utf8();
        if c == '"' || c == '\'' {
            loop {
                let Some(ch) = text[i..].chars().next() else {
                    return out;
                };
                i += ch.len_utf8();
                if ch == '\\' {
                    i += text[i..].chars().next().map_or(0, char::len_utf8);
                } else if ch == c {
                    break;
                }
            }
        } else if c == '{' {
            let Some(close) = text[i..].find('}').map(|k| k + i) else {
                break;
            };
            out.push((start..close + 1, &text[i..close]));
            i = close + 1;
        }
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
    Ref(Ref),
    Word(&'static str),
    Sym(&'static str),
}

const SYMS: [&str; 13] = [
    "==", "!=", "<=", ">=", "<", ">", "+", "-", "*", "/", "%", "(", ")",
];

fn lex(
    src: &str,
    var: &dyn Fn(&str) -> Result<Ref, Lookup>,
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
            let r = var(inner).map_err(|why| match why {
                Lookup::Unknown => ExprError::UnknownName {
                    at: at.clone(),
                    name: inner.trim().to_owned(),
                },
                Lookup::Why(msg) => ExprError::Syntax {
                    at: at.clone(),
                    msg,
                },
            })?;
            out.push((Tok::Ref(r), at));
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
    Ref(Ref),
    Neg(Box<Node>),
    Not(Box<Node>),
    Bin(&'static str, Box<Node>, Box<Node>),
}

#[derive(Clone, Debug, PartialEq)]
struct Node {
    ast: Ast,
    at: Range<usize>,
    /// How many nodes deep the tree under it goes, itself counted.
    height: usize,
}

/// The deepest an expression's parentheses, `not`s and unary `-`s may nest. The
/// parser recurses about a dozen frames per level, so this keeps a hostile
/// expression from a file or an agent from overflowing the stack.
const MAX_NEST: usize = 16;

/// The deepest an expression's tree may go, operators chained one after
/// another counted. [`eval`], [`type_of`] and dropping a tree recurse once per
/// level, so this bounds them as [`MAX_NEST`] bounds the parser.
const MAX_HEIGHT: usize = 64;

struct Parser {
    toks: Vec<(Tok, Range<usize>)>,
    pos: usize,
    end: usize,
    /// How many parentheses, `not`s and unary `-`s are open where it reads.
    nest: usize,
}

/// A node over `ast`, refused when it would go deeper than [`MAX_HEIGHT`].
fn node(at: Range<usize>, ast: Ast) -> Result<Node, ExprError> {
    let height = 1 + match &ast {
        Ast::Lit(_) | Ast::Ref(_) => 0,
        Ast::Neg(n) | Ast::Not(n) => n.height,
        Ast::Bin(_, l, r) => l.height.max(r.height),
    };
    if height > MAX_HEIGHT {
        return Err(ExprError::Syntax {
            at,
            msg: format!("an expression goes at most {MAX_HEIGHT} operators deep"),
        });
    }
    Ok(Node { ast, at, height })
}

impl Parser {
    /// Opens one more level of nesting at `at`, refused past [`MAX_NEST`]; the
    /// caller closes it with `self.nest -= 1` once the level is read.
    fn open(&mut self, at: Range<usize>) -> Result<(), ExprError> {
        if self.nest >= MAX_NEST {
            return Err(ExprError::Syntax {
                at,
                msg: format!("an expression nests at most {MAX_NEST} deep"),
            });
        }
        self.nest += 1;
        Ok(())
    }

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
            left = node(
                left.at.start..right.at.end,
                Ast::Bin(name, Box::new(left), Box::new(right)),
            )?;
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
        let here = self.here();
        if self.eat(&Tok::Word("not")) {
            self.open(here.clone())?;
            let inner = self.not()?;
            self.nest -= 1;
            return node(here.start..inner.at.end, Ast::Not(Box::new(inner)));
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
        node(
            left.at.start..right.at.end,
            Ast::Bin(op, Box::new(left), Box::new(right)),
        )
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
        let here = self.here();
        if self.eat(&Tok::Sym("-")) {
            self.open(here.clone())?;
            let inner = self.unary()?;
            self.nest -= 1;
            return node(here.start..inner.at.end, Ast::Neg(Box::new(inner)));
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
            Tok::Ref(r) => Ast::Ref(r),
            Tok::Sym("(") => {
                self.open(at.clone())?;
                let inner = self.or()?;
                self.nest -= 1;
                let close = self.here();
                if !self.eat(&Tok::Sym(")")) {
                    return Err(ExprError::Syntax {
                        at: at.start..close.end.max(at.end),
                        msg: "this ( is never closed".into(),
                    });
                }
                return Ok(Node {
                    at: at.start..close.end,
                    ..inner
                });
            }
            _ => {
                return Err(ExprError::Syntax {
                    at,
                    msg: "a value is missing before this".into(),
                });
            }
        };
        node(at, ast)
    }
}

fn parse(src: &str, var: &dyn Fn(&str) -> Result<Ref, Lookup>) -> Result<Node, ExprError> {
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
        nest: 0,
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

const KINDS: [VarKind; 4] = [
    VarKind::Number,
    VarKind::String,
    VarKind::Boolean,
    VarKind::Color,
];

/// [`bin_kind`] where a side may be known only in play (`None`): it applies
/// when some kind that side could come to makes it apply, and comes to the
/// kind every such pairing agrees on, else to one known only in play.
fn bin_kind_maybe(
    op: &str,
    l: Option<VarKind>,
    r: Option<VarKind>,
) -> Result<Option<VarKind>, String> {
    let sides = |k: Option<VarKind>| k.map_or(KINDS.to_vec(), |k| vec![k]);
    let mut first_err = None;
    let mut got: Vec<VarKind> = Vec::new();
    for a in sides(l) {
        for b in sides(r) {
            match bin_kind(op, a, b) {
                Ok(k) if !got.contains(&k) => got.push(k),
                Ok(_) => {}
                Err(e) => {
                    first_err.get_or_insert(e);
                }
            }
        }
    }
    match got.as_slice() {
        [] => Err(first_err.unwrap_or_default()),
        [k] => Ok(Some(*k)),
        _ => Ok(None),
    }
}

fn type_of(
    n: &Node,
    kind_of: &dyn Fn(&Ref) -> Result<Option<VarKind>, Unread>,
) -> Result<Option<VarKind>, ExprError> {
    let wrong = |msg: String| ExprError::Type {
        at: n.at.clone(),
        msg,
    };
    match &n.ast {
        Ast::Lit(v) => Ok(Some(kind_of_value(v))),
        Ast::Ref(r) => kind_of(r).map_err(|why| ExprError::unread(why, r, &n.at)),
        Ast::Neg(inner) => match type_of(inner, kind_of)? {
            None | Some(VarKind::Number) => Ok(Some(VarKind::Number)),
            Some(k) => Err(wrong(format!("- needs a number, not {}", kind_word(k)))),
        },
        Ast::Not(inner) => match type_of(inner, kind_of)? {
            None | Some(VarKind::Boolean) => Ok(Some(VarKind::Boolean)),
            Some(k) => Err(wrong(format!(
                "not needs true or false, not {}",
                kind_word(k)
            ))),
        },
        Ast::Bin(op, l, r) => {
            let (l, r) = (type_of(l, kind_of)?, type_of(r, kind_of)?);
            bin_kind_maybe(op, l, r).map_err(wrong)
        }
    }
}

/// What `n` comes to. Each node's work is done in [`unary_op`] and
/// [`binary_op`], outside this recursive frame, so the frame stays small: a
/// debug build otherwise gives it kilobytes, and [`MAX_HEIGHT`] of them
/// overflow a small stack.
fn eval(n: &Node, read: &dyn Fn(&Ref) -> Result<Value, Unread>) -> Result<Value, ExprError> {
    match &n.ast {
        Ast::Lit(v) => Ok(v.clone()),
        Ast::Ref(r) => read(r).map_err(|why| ExprError::unread(why, r, &n.at)),
        Ast::Neg(inner) => unary_op("-", eval(inner, read)?, &n.at),
        Ast::Not(inner) => unary_op("not", eval(inner, read)?, &n.at),
        Ast::Bin(op @ ("and" | "or"), l, r) => match eval(l, read)? {
            // Short-circuit: the right side is not read once the left decides.
            Value::Boolean(a) if a == (*op == "or") => Ok(Value::Boolean(a)),
            a @ Value::Boolean(_) => binary_op(op, a, eval(r, read)?, &n.at),
            a => binary_op(op, a, Value::Boolean(false), &n.at),
        },
        Ast::Bin(op, l, r) => {
            let a = eval(l, read)?;
            binary_op(op, a, eval(r, read)?, &n.at)
        }
    }
}

/// `-` or `not` over `a`, the error naming `at`: one node's work in [`eval`].
fn unary_op(op: &str, a: Value, at: &Range<usize>) -> Result<Value, ExprError> {
    let msg = match (op, a) {
        ("-", Value::Number(x)) => return Ok(Value::Number(-x)),
        ("not", Value::Boolean(b)) => return Ok(Value::Boolean(!b)),
        ("-", v) => format!("- needs a number, not {}", kind_word(kind_of_value(&v))),
        (_, v) => format!(
            "not needs true or false, not {}",
            kind_word(kind_of_value(&v))
        ),
    };
    Err(ExprError::Type {
        at: at.clone(),
        msg,
    })
}

/// `op` over `a` and `b`, the error naming `at`: one node's work in [`eval`].
/// An `and` or `or` comes here only once its left side has not decided it.
fn binary_op(op: &str, a: Value, b: Value, at: &Range<usize>) -> Result<Value, ExprError> {
    let wrong = |msg: String| ExprError::Type {
        at: at.clone(),
        msg,
    };
    if op == "and" || op == "or" {
        return match (a, b) {
            (Value::Boolean(_), Value::Boolean(b)) => Ok(Value::Boolean(b)),
            _ => Err(wrong(format!("{op} needs true or false on both sides"))),
        };
    }
    bin_kind(op, kind_of_value(&a), kind_of_value(&b)).map_err(&wrong)?;
    Ok(match (op, a, b) {
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

    /// A hostile nesting is a syntax error, not a stack overflow, and an
    /// expression at the limits parses, kinds and evaluates on a small stack.
    ///
    /// flip: `Parser::open` never refusing → the thread overflows its stack at
    /// "parens", aborting the run.
    #[test]
    fn deep_nesting_is_refused_before_it_overflows() {
        let small = std::thread::Builder::new().stack_size(256 * 1024);
        small
            .spawn(|| {
                let deep = 100_000;
                let refused = |src: String, what: &str| {
                    let e = Expr(src).check();
                    assert!(matches!(e, Err(ExprError::Syntax { .. })), "{what}: {e:?}");
                };
                refused("(".repeat(deep) + "1" + &")".repeat(deep), "parens");
                refused("not ".repeat(deep) + "true", "nots");
                refused("- ".repeat(deep) + "1", "negations");
                refused("1".to_owned() + &" + 1".repeat(deep), "a chain");
                refused(
                    "(".repeat(MAX_NEST + 1) + "1" + &")".repeat(MAX_NEST + 1),
                    "one past",
                );
                refused("1".to_owned() + &" + 1".repeat(MAX_HEIGHT), "one op past");

                let nested = "(-".repeat(MAX_NEST / 2) + "1" + &")".repeat(MAX_NEST / 2);
                assert_eq!(run(&nested), Ok(Value::Number(1.0)), "at the nesting limit");
                let chain = "1".to_owned() + &" + 1".repeat(MAX_HEIGHT - 1);
                assert_eq!(run(&chain), Ok(Value::Number(MAX_HEIGHT as f64)));
                let kind = Expr(chain).kind(&|_| Ok(None));
                assert_eq!(kind, Ok(Some(VarKind::Number)), "at the height limit");
            })
            .expect("a thread")
            .join()
            .expect("no panic");
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
        let kind_of = |r: &Ref| match r {
            Ref::Var(v) if v.0.seq == 1 => Ok(Some(VarKind::Number)),
            Ref::Var(v) if v.0.seq == 2 => Ok(Some(VarKind::String)),
            _ => Err(Unread::Missing),
        };
        let k = |s: String| Expr(s).kind(&kind_of);
        assert_eq!(k(format!("{} + 1", wire(1))), Ok(Some(VarKind::Number)));
        assert_eq!(
            k(format!("{} + 1", wire(2))),
            Ok(Some(VarKind::String)),
            "join"
        );
        assert_eq!(k(format!("{} > 1", wire(1))), Ok(Some(VarKind::Boolean)));
        assert!(matches!(
            k(format!("{} * 2", wire(2))),
            Err(ExprError::Type { .. })
        ));
        assert!(matches!(k(wire(7)), Err(ExprError::MissingVariable { .. })));
    }

    /// The field's names go to ids and back, an unknown name is kept as typed
    /// and reported where it is, and a name holding `}` is shown by id.
    ///
    /// flip: `from_names` skipping the id write → fails at "stored by id";
    /// `Expr::shown` writing every name → fails at "kept as its id".
    #[test]
    fn names_round_trip_through_ids() {
        let name_of = |v: VarId| (v.0.seq == 1).then(|| "cart/count".to_owned());
        let id_of = |n: &str| (n == "cart/count").then(|| var(1)).into_iter().collect();
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

        // A name the braces cannot hold is shown by id, which reads back.
        let odd = |_: VarId| Some("Price {EUR}".to_owned());
        let shown = stored.with_names(&odd);
        assert_eq!(shown, format!("{} + 1", wire(1)), "kept as its id");
        let (back, err) = Expr::from_names(&shown, &|_| Vec::new());
        assert_eq!((back, err), (stored, None));
    }

    /// A brace inside quotes opens no name, for the stored form as for the
    /// parse: the name after it is stored by id, read by `refs` and mapped.
    ///
    /// flip: `var_tokens` not skipping quoted text → fails at "stored by id".
    #[test]
    fn a_quoted_brace_opens_no_name() {
        let id_of = |n: &str| (n == "Total").then(|| var(1)).into_iter().collect();
        let (e, err) = Expr::from_names(r#""{" + {Total} + '\'}'"#, &id_of);
        assert_eq!(err, None);
        assert_eq!(
            e.0,
            format!(r#""{{" + {} + '\'}}'"#, wire(1)),
            "stored by id"
        );
        assert_eq!(e.refs(), vec![Ref::Var(var(1))]);
        let mapped = e.map_vars(&mut |_| var(2));
        assert_eq!(mapped.vars(), vec![var(2)], "mapped");
        assert_eq!(run(&e.0), Ok(Value::String("{4'}".into())));
        assert!(var_tokens("'{x}").is_empty(), "an unclosed quote ends it");
    }

    fn set(seq: u64, name: &str, cols: &[&str], rows: &[&[&str]]) -> DataSet {
        use crate::chart::{Column, Table};
        DataSet {
            id: DataSetId(NodeId { actor: 0xcd, seq }),
            name: name.into(),
            table: Table {
                columns: cols
                    .iter()
                    .enumerate()
                    .map(|(i, n)| Column {
                        id: ColumnId(i as u32),
                        name: (*n).into(),
                    })
                    .collect(),
                rows: rows
                    .iter()
                    .map(|r| r.iter().map(|c| (*c).to_string()).collect())
                    .collect(),
            },
            source: None,
            pictures: Vec::new(),
        }
    }

    fn shop() -> Vec<DataSet> {
        vec![
            set(
                1,
                "products.csv",
                &["Name", "Price"],
                &[&["Shirt", "42"], &["Scarf", "28"]],
            ),
            set(2, "Cart", &["Name", "Line", "rows"], &[]),
        ]
    }

    /// A typed table read resolves by the real names, longest match, and is
    /// stored by ids that read back to the same text.
    ///
    /// flip: `Names::resolve` skipping its `Ref::Total` arm → fails at "a total".
    #[test]
    fn table_reads_resolve_by_name_and_store_ids() {
        let sets = shop();
        let vid = |n: &str| (n == "Total").then(|| var(1)).into_iter().collect();
        let vname = |v: VarId| (v == var(1)).then(|| "Total".to_owned());
        let names = Names {
            var_id: &vid,
            var_name: &vname,
            tables: &sets,
            row: Some(sets[1].id),
        };
        let typed = "{products.csv.2.Name} + {Cart.Line.sum} + {Cart.rows} + {row.Line} + {Total}";
        let (e, err) = Expr::from_names_in(typed, &names);
        assert_eq!(err, None);
        let refs = e.refs();
        assert_eq!(refs[0], Ref::Cell(sets[0].id, RowAt::Nth(2), ColumnId(0)));
        assert_eq!(
            refs[1],
            Ref::Total(sets[1].id, ColumnId(1), Agg::Sum),
            "a total"
        );
        assert_eq!(refs[2], Ref::Rows(sets[1].id));
        assert_eq!(refs[3], Ref::Row(ColumnId(1)));
        assert_eq!(e.with_names_in(&names), typed, "shown as typed");
        assert_eq!(e.vars(), vec![var(1)]);
        assert_eq!(Expr(e.0.clone()).check(), Ok(()), "the stored form reads");

        // A column called `rows` and the count read two ways.
        let (_, err) = Expr::from_names_in("{Cart.rows}", &Names { row: None, ..names });
        assert_eq!(
            err, None,
            "Cart.rows alone is the count: `rows` needs an aggregate"
        );
        let (_, err) = Expr::from_names_in("{Cart.first.rows}", &names);
        assert_eq!(err, None);
        let (_, err) = Expr::from_names_in("{Cart}", &names);
        assert!(err.unwrap().to_string().contains("is a table"));
        let (_, err) = Expr::from_names_in("{row.Name}", &Names { row: None, ..names });
        assert_eq!(
            err.unwrap().to_string(),
            "row is read only inside a row action"
        );
    }

    /// A name that reads two ways is refused rather than guessed, a name two
    /// variables share among them.
    ///
    /// flip: `Names::resolve` answering its first candidate → fails at "two ways";
    /// answering the first variable → fails at "shared".
    #[test]
    fn a_name_reading_two_ways_is_refused() {
        let sets = shop();
        let vid = |n: &str| (n == "Cart.rows").then(|| var(1)).into_iter().collect();
        let vname = |_: VarId| None;
        let names = Names {
            var_id: &vid,
            var_name: &vname,
            tables: &sets,
            row: None,
        };
        let (_, err) = Expr::from_names_in("{Cart.rows} + 1", &names);
        let err = err.expect("two ways");
        assert!(
            err.to_string().contains("pick it from the @ list"),
            "two ways"
        );
        assert_eq!(err.at(), 0..11);

        // Two variables of one name: refused at both doors, never the first.
        let both = |n: &str| match n {
            "count" => vec![var(1), var(2)],
            _ => Vec::new(),
        };
        let names = Names {
            var_id: &both,
            ..names
        };
        let (_, err) = Expr::from_names_in("{count} + 1", &names);
        assert!(err.unwrap().to_string().contains("two variables"), "shared");
        let (_, err) = Expr::from_names("{count} + 1", &both);
        assert!(err.unwrap().to_string().contains("two variables"));
    }

    /// Each form evaluates in play, and one that cannot says why.
    ///
    /// flip: `TableEnv::read` counting `Nth` from 0 → fails at "row 2".
    #[test]
    fn table_reads_evaluate_and_explain() {
        let sets = shop();
        let opened = |_| 1;
        let row = ["Scarf".to_string(), "28".to_string(), String::new()];
        let env = TableEnv {
            tables: &sets,
            opened: &opened,
            row: Some((sets[1].id, &row)),
        };
        let (p, c) = (sets[0].id, sets[1].id);
        let e = |r: Ref| env.read(&r);
        assert_eq!(
            e(Ref::Cell(p, RowAt::Nth(2), ColumnId(0))),
            Ok(Value::String("Scarf".into())),
            "row 2"
        );
        assert_eq!(
            e(Ref::Cell(p, RowAt::Opened, ColumnId(1))),
            Ok(Value::Number(28.0))
        );
        assert_eq!(
            e(Ref::Total(p, ColumnId(1), Agg::Sum)),
            Ok(Value::Number(70.0))
        );
        assert_eq!(
            e(Ref::Total(p, ColumnId(1), Agg::Avg)),
            Ok(Value::Number(35.0))
        );
        assert_eq!(
            e(Ref::Total(p, ColumnId(0), Agg::Sum)),
            Ok(Value::Number(0.0))
        );
        assert_eq!(e(Ref::Rows(c)), Ok(Value::Number(0.0)));
        assert_eq!(
            e(Ref::Total(c, ColumnId(1), Agg::Sum)),
            Ok(Value::Number(0.0)),
            "empty sums to 0"
        );
        assert_eq!(e(Ref::Row(ColumnId(1))), Ok(Value::Number(28.0)));
        let why = |r: Ref| match e(r) {
            Err(Unread::Why(m)) => m,
            other => panic!("{other:?}"),
        };
        assert_eq!(
            why(Ref::Cell(p, RowAt::Nth(3), ColumnId(0))),
            "products.csv has 2 rows, not 3"
        );
        assert_eq!(
            why(Ref::Cell(c, RowAt::First, ColumnId(0))),
            "Cart has no rows"
        );
        assert_eq!(
            why(Ref::Total(c, ColumnId(1), Agg::Min)),
            "min of no numbers: Cart has no numbers in Line"
        );
        assert_eq!(
            e(Ref::Cell(p, RowAt::First, ColumnId(9))),
            Err(Unread::Missing)
        );

        // Through an expression, the reason lands on the token.
        let x = Expr(format!(
            "1 + {{{}}}",
            Ref::Cell(c, RowAt::First, ColumnId(0)).wire()
        ));
        let err = x.eval_with(&|r| env.read(r)).unwrap_err();
        assert_eq!(err.to_string(), "Cart has no rows");
        assert_eq!(err.at().start, 4);
    }

    /// A count and a total are numbers while authoring; a cell has no kind
    /// until play, so it is never refused for one.
    ///
    /// flip: `bin_kind_maybe` erring whenever a side is unknown → fails at "a cell".
    #[test]
    fn a_cell_has_no_kind_until_play() {
        let sets = shop();
        let p = sets[0].id;
        let k = |src: String| Expr(src).kind(&|r| table_kind(&sets, None, r));
        let cell = format!("{{{}}}", Ref::Cell(p, RowAt::First, ColumnId(1)).wire());
        let rows = format!("{{{}}}", Ref::Rows(p).wire());
        assert_eq!(
            k(format!("{cell} * 2")),
            Ok(Some(VarKind::Number)),
            "a cell"
        );
        assert_eq!(k(format!("{cell} + 1")), Ok(None), "a sum or a join");
        assert_eq!(k(format!("{rows} + 1")), Ok(Some(VarKind::Number)));
        assert_eq!(k(format!("{cell} == 'x'")), Ok(Some(VarKind::Boolean)));
        assert!(k(format!("{rows} and true")).is_err());
        let gone = format!(
            "{{{}}}",
            Ref::Rows(DataSetId(NodeId { actor: 1, seq: 1 })).wire()
        );
        assert!(matches!(k(gone), Err(ExprError::MissingTable { .. })));
        let row = format!("{{{}}}", Ref::Row(ColumnId(0)).wire());
        assert!(
            matches!(k(row), Err(ExprError::Type { .. })),
            "row outside a row action"
        );
    }
}
