//! Lists from data (`docs/design/lists.md`): a frame carrying a [`List`]
//! description repeats its first child, a component instance, once per row of
//! one of the document's [`DataSet`]s, each copy's component properties filled
//! from the row's cells.
//!
//! The copies are ordinary instances, saved and drawn as any layer is. They are
//! made again by [`regenerate`] whenever the template, the description or the
//! data set changes: in the app inside the edit's own undo step, so undo and
//! redo never meet a copy a later regeneration replaced; in play, on the
//! player's copy. A load draws the copies as saved.

use crate::chart::{ColumnId, DataLink, MAX_CELL_CHARS, MAX_COLUMNS, MAX_ROWS, Table};
use crate::document::Document;
use crate::id::{IdSource, NodeId};
use crate::item::ItemId;
use crate::op::{DirtySet, Operation, Transaction};
use crate::variant::{PropKind, PropValue};
use rustc_hash::FxHashSet;
use serde::{Deserialize, Deserializer, Serialize, Serializer};

/// The most rows a list draws, the template's included: each is an instance.
pub const MAX_DRAWN: usize = 200;
/// The longest a data set's name may be.
pub const MAX_NAME_CHARS: usize = 256;

/// A data set's identity in its document.
#[derive(Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct DataSetId(pub NodeId);

impl std::fmt::Debug for DataSetId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "data set {:?}", self.0)
    }
}

/// Written as the node id's wire form, `"<actor-hex>:<seq>"`.
impl Serialize for DataSetId {
    fn serialize<S: Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        s.serialize_str(&self.0.to_wire())
    }
}

impl<'de> Deserialize<'de> for DataSetId {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        let s = String::deserialize(d)?;
        NodeId::from_wire(&s)
            .map(DataSetId)
            .ok_or_else(|| serde::de::Error::custom(format!("bad id {s:?}")))
    }
}

impl IdSource {
    /// A fresh [`DataSetId`].
    pub fn mint_data_set(&mut self) -> DataSetId {
        DataSetId(self.mint())
    }
}

/// One table the document's lists read: its name, its rows, and the file it was
/// read from while linked.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct DataSet {
    pub id: DataSetId,
    pub name: String,
    pub table: Table,
    /// The file the table was read from, while linked. Never carried by a copy.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source: Option<DataLink>,
    /// The pictures the table's cells name: a cell's text (a path beside the
    /// data file) and the picture read from it into the document, filled where
    /// the file is read, since core reads no files. An image property's column
    /// fills from it, or from a cell naming a picture's id.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub pictures: Vec<(String, crate::ImageId)>,
}

impl DataSet {
    /// The picture `cell` names: one read for that cell's text, else the
    /// document's picture of that id.
    pub fn picture(&self, doc: &Document, cell: &str) -> Option<crate::ImageId> {
        let cell = cell.trim();
        if let Some((_, id)) = self.pictures.iter().find(|(c, _)| c == cell) {
            return Some(id.clone());
        }
        let id = crate::ImageId(cell.to_string());
        doc.image(&id).is_some().then_some(id)
    }

    /// The image columns: each with a cell naming a picture ([`Self::picture`]).
    pub fn image_columns(&self, doc: &Document) -> Vec<ColumnId> {
        self.table
            .columns
            .iter()
            .filter(|c| {
                self.table
                    .cells(c.id)
                    .any(|cell| self.picture(doc, cell).is_some())
            })
            .map(|c| c.id)
            .collect()
    }
}

/// Why a data set or a list may not be stored.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ListRule {
    /// Past the table's limits, a ragged row, a cell or name too long.
    BadTable,
    /// A column id twice.
    DuplicateColumn,
    /// A list on anything but a frame.
    NotAFrame,
    /// A copy named twice, or the list's own frame named as its copy.
    BadCopies,
    /// More filter tests or sort columns than a list holds.
    BadFilter,
}

impl std::fmt::Display for ListRule {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            ListRule::BadTable => "a data set's table is past its limits or ragged",
            ListRule::DuplicateColumn => "a data set names a column twice",
            ListRule::NotAFrame => "a list sits only on a frame",
            ListRule::BadCopies => "a list names a copy twice, or its own frame as a copy",
            ListRule::BadFilter => "a list holds at most 8 filter tests and 2 sort columns",
        })
    }
}

impl DataSet {
    /// Whether this data set may be stored: the chart table's limits and shape.
    pub fn validate(&self) -> Result<(), ListRule> {
        if self.name.chars().count() > MAX_NAME_CHARS {
            return Err(ListRule::BadTable);
        }
        validate_table(&self.table)
    }
}

/// The chart's rule for a table, which a data set shares.
pub fn validate_table(t: &Table) -> Result<(), ListRule> {
    if t.rows.len() > MAX_ROWS || t.columns.len() > MAX_COLUMNS {
        return Err(ListRule::BadTable);
    }
    let mut ids = FxHashSet::default();
    for c in &t.columns {
        if !ids.insert(c.id) {
            return Err(ListRule::DuplicateColumn);
        }
        if c.name.chars().count() > MAX_CELL_CHARS {
            return Err(ListRule::BadTable);
        }
    }
    for r in &t.rows {
        if r.len() != t.columns.len() || r.iter().any(|c| c.chars().count() > MAX_CELL_CHARS) {
            return Err(ListRule::BadTable);
        }
    }
    Ok(())
}

/// A list's description, `Node::list` on a frame. Its first child that is not a
/// copy is the template.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct List {
    /// The data set it reads. Over one the document lacks (a list pasted from
    /// another document) the list keeps the rows it has until a data set is
    /// picked.
    pub data: DataSetId,
    #[serde(default)]
    pub rows: Rows,
    /// Which column fills which of the template's properties.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub fill: Vec<Fill>,
    /// The copies made by the last regeneration, in row order after the
    /// template; written only by [`regenerate`].
    #[serde(default, skip_serializing_if = "Vec::is_empty", with = "wire_ids")]
    pub copies: Vec<NodeId>,
    /// The row an [`Rows::Opened`] list shows: 0 in the document, the row
    /// opened while playing.
    #[serde(default, skip_serializing_if = "is_zero")]
    pub opened: usize,
    /// Which rows pass, before the sort and the rows mode.
    #[serde(default, skip_serializing_if = "Filter::is_empty")]
    pub filter: Filter,
    /// The order the passing rows are drawn in; empty keeps the table's.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub sort: Vec<SortKey>,
}

fn is_zero(v: &usize) -> bool {
    *v == 0
}

mod wire_ids {
    use crate::id::NodeId;
    use serde::{Deserialize, Deserializer, Serialize, Serializer};

    pub fn serialize<S: Serializer>(ids: &[NodeId], s: S) -> Result<S::Ok, S::Error> {
        let wire: Vec<String> = ids.iter().map(|id| id.to_wire()).collect();
        wire.serialize(s)
    }

    pub fn deserialize<'de, D: Deserializer<'de>>(d: D) -> Result<Vec<NodeId>, D::Error> {
        Vec::<String>::deserialize(d)?
            .into_iter()
            .map(|id| {
                NodeId::from_wire(&id)
                    .ok_or_else(|| serde::de::Error::custom(format!("malformed node id {id:?}")))
            })
            .collect()
    }
}

/// Which rows a list draws.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Rows {
    /// Every row, up to [`MAX_DRAWN`].
    #[default]
    All,
    /// The first `n`.
    First(u32),
    /// The one row opened in play by a tap on a row of a list over the same data
    /// set: a detail screen. Row 1 on the canvas.
    Opened,
    /// One fixed row, counted from 1 and held by position, the same in play: a
    /// stat card or a featured item. Past the table's end, its last row.
    Row(u32),
}

impl Rows {
    /// Whether it shows one row of the table as it stands, which a filter and a
    /// sort do not apply to.
    pub fn is_one_row(self) -> bool {
        matches!(self, Rows::Opened | Rows::Row(_))
    }
}

/// The most tests a filter holds; mixed groups are a script.
pub const MAX_TESTS: usize = 8;
/// The most columns a sort compares by: one and a *Then by*.
pub const MAX_SORT: usize = 2;

/// Which rows of the table pass before the rows mode applies: tests on columns,
/// joined by all or any.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Filter {
    /// Any test passing passes the row; else every test must.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub any: bool,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub tests: Vec<Test>,
}

impl Filter {
    pub fn is_empty(&self) -> bool {
        self.tests.is_empty()
    }
}

/// One filter test: a column, a test word and a value.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Test {
    pub column: ColumnId,
    pub op: TestOp,
    /// Literal text when it names no `{…}`, else an expression in the stored
    /// form, its variables read in the mode the list's frame is in.
    #[serde(default, skip_serializing_if = "is_empty_expr")]
    pub value: crate::prototype::expr::Expr,
}

fn is_empty_expr(e: &crate::prototype::expr::Expr) -> bool {
    e.0.is_empty()
}

/// A filter test's word. Text tests ignore case; the number tests read both
/// sides with `chart::parse_number` and fail a cell that is not a number.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TestOp {
    Is,
    IsNot,
    Contains,
    StartsWith,
    Less,
    AtMost,
    More,
    AtLeast,
    Empty,
    NotEmpty,
}

impl TestOp {
    /// Every test, in the order the dropdown lists them.
    pub const ALL: [TestOp; 10] = [
        TestOp::Is,
        TestOp::IsNot,
        TestOp::Contains,
        TestOp::StartsWith,
        TestOp::Less,
        TestOp::AtMost,
        TestOp::More,
        TestOp::AtLeast,
        TestOp::Empty,
        TestOp::NotEmpty,
    ];

    /// The words the List card shows.
    pub fn words(self) -> &'static str {
        match self {
            TestOp::Is => "is",
            TestOp::IsNot => "is not",
            TestOp::Contains => "contains",
            TestOp::StartsWith => "starts with",
            TestOp::Less => "less than",
            TestOp::AtMost => "at most",
            TestOp::More => "more than",
            TestOp::AtLeast => "at least",
            TestOp::Empty => "is empty",
            TestOp::NotEmpty => "is not empty",
        }
    }

    /// Whether it compares numbers, offered on a number column only.
    pub fn is_number(self) -> bool {
        matches!(
            self,
            TestOp::Less | TestOp::AtMost | TestOp::More | TestOp::AtLeast
        )
    }

    /// Whether it reads a value; the two emptiness tests do not.
    pub fn takes_value(self) -> bool {
        !matches!(self, TestOp::Empty | TestOp::NotEmpty)
    }

    /// Whether `cell` passes against `value`. An empty value passes every row,
    /// so a search not yet typed into shows everything.
    pub fn passes(self, cell: &str, value: &str) -> bool {
        let (cell, value) = (cell.trim(), value.trim());
        match self {
            TestOp::Empty => return cell.is_empty(),
            TestOp::NotEmpty => return !cell.is_empty(),
            _ if value.is_empty() => return true,
            _ => {}
        }
        if self.is_number() {
            let (Some(c), Some(v)) = (
                crate::chart::parse_number(cell),
                crate::chart::parse_number(value),
            ) else {
                return false;
            };
            return match self {
                TestOp::Less => c < v,
                TestOp::AtMost => c <= v,
                TestOp::More => c > v,
                _ => c >= v,
            };
        }
        let (c, v) = (cell.to_lowercase(), value.to_lowercase());
        match self {
            TestOp::Is => c == v,
            TestOp::IsNot => c != v,
            TestOp::Contains => c.contains(&v),
            _ => c.starts_with(&v),
        }
    }
}

/// One column a list's rows are sorted by.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct SortKey {
    pub column: ColumnId,
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub descending: bool,
}

/// Two cells in the data window's *Sort ascending* order: a number column by
/// value, text ignoring case. Empty cells are not ordered here; callers put
/// them last whichever way the sort runs.
pub fn cell_order(x: &str, y: &str, numeric: bool) -> std::cmp::Ordering {
    let (x, y) = (x.trim(), y.trim());
    if numeric {
        let (p, q) = (crate::chart::parse_number(x), crate::chart::parse_number(y));
        p.partial_cmp(&q).unwrap_or(std::cmp::Ordering::Equal)
    } else {
        x.to_lowercase().cmp(&y.to_lowercase())
    }
}

/// `a` against `b` sorted by column `col`, empty cells last either way.
pub fn sort_order(a: &str, b: &str, numeric: bool, ascending: bool) -> std::cmp::Ordering {
    use std::cmp::Ordering;
    match (a.trim().is_empty(), b.trim().is_empty()) {
        (true, true) => Ordering::Equal,
        (true, false) => Ordering::Greater,
        (false, true) => Ordering::Less,
        _ => {
            let o = cell_order(a, b, numeric);
            if ascending { o } else { o.reverse() }
        }
    }
}

/// One column filling one property of the template.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Fill {
    pub target: Target,
    pub column: ColumnId,
}

/// What a column fills.
#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Target {
    /// A component property of the template's main, by its item id: a text
    /// takes the cell, a boolean [`cell_bool`]'s reading, an image the picture
    /// the cell names.
    Prop(ItemId),
    /// A variant property of the template's set, by name: the copy switches to
    /// the variant whose value the cell names.
    Variant(String),
}

impl List {
    /// A list over `data` filling nothing, every row drawn.
    pub fn new(data: DataSetId) -> List {
        List {
            data,
            rows: Rows::All,
            fill: Vec::new(),
            copies: Vec::new(),
            opened: 0,
            filter: Filter::default(),
            sort: Vec::new(),
        }
    }

    /// This list without the fills, filter tests and sort keys on a column
    /// `table` lacks. A deleted column's id can be minted again for a new
    /// column, which none of them should read; until then each reads nothing.
    pub fn on_columns_of(&self, table: &Table) -> List {
        let has = |c: ColumnId| table.column(c).is_some();
        let mut list = self.clone();
        list.fill.retain(|f| has(f.column));
        list.filter.tests.retain(|t| has(t.column));
        list.sort.retain(|k| has(k.column));
        list
    }

    /// Whether this description may be stored on `id`.
    pub fn validate(&self, id: NodeId) -> Result<(), ListRule> {
        let mut seen = FxHashSet::default();
        if !self.copies.iter().all(|c| *c != id && seen.insert(*c)) {
            return Err(ListRule::BadCopies);
        }
        if self.filter.tests.len() > MAX_TESTS || self.sort.len() > MAX_SORT {
            return Err(ListRule::BadFilter);
        }
        Ok(())
    }

    /// The variables its filter reads.
    pub fn filter_vars(&self) -> Vec<crate::variable::VarId> {
        self.filter
            .tests
            .iter()
            .flat_map(|t| t.value.vars())
            .collect()
    }

    /// The column filling `target`, if any.
    pub fn column_for(&self, target: &Target) -> Option<ColumnId> {
        self.fill
            .iter()
            .find(|f| f.target == *target)
            .map(|f| f.column)
    }

    /// The table rows this list draws, in order: the filter, then the sort,
    /// then the rows mode. A one-row mode shows a row of the table as it
    /// stands, so the filter and sort do not apply to it. `read` answers a
    /// variable a test's value reads.
    pub fn shown_rows(
        &self,
        table: &Table,
        read: &dyn Fn(crate::variable::VarId) -> Option<crate::variable::Value>,
    ) -> Vec<usize> {
        let len = table.rows.len();
        match self.rows {
            Rows::Opened | Rows::Row(_) if len == 0 => return Vec::new(),
            Rows::Opened => return vec![self.opened.min(len - 1)],
            Rows::Row(n) => return vec![(n.max(1) as usize - 1).min(len - 1)],
            _ => {}
        }
        let mut rows = self.passing(table, read);
        self.sort_rows(table, &mut rows);
        let cap = match self.rows {
            Rows::First(n) => (n as usize).min(MAX_DRAWN),
            _ => MAX_DRAWN,
        };
        rows.truncate(cap);
        rows
    }

    /// The rows of `table` passing the filter, in table order. A test whose
    /// column the table lacks passes every row; a value that does not evaluate
    /// reads as empty, which passes too.
    pub fn passing(
        &self,
        table: &Table,
        read: &dyn Fn(crate::variable::VarId) -> Option<crate::variable::Value>,
    ) -> Vec<usize> {
        let tests: Vec<(Option<usize>, TestOp, String)> = self
            .filter
            .tests
            .iter()
            .map(|t| (table.index_of(t.column), t.op, test_value(&t.value, read)))
            .collect();
        (0..table.rows.len())
            .filter(|r| {
                if tests.is_empty() {
                    return true;
                }
                let mut each = tests.iter().map(|(col, op, value)| {
                    col.is_none_or(|c| op.passes(&table.rows[*r][c], value))
                });
                if self.filter.any {
                    each.any(|p| p)
                } else {
                    each.all(|p| p)
                }
            })
            .collect()
    }

    /// `rows` put in the sort's order; stable, so equal rows keep table order.
    fn sort_rows(&self, table: &Table, rows: &mut [usize]) {
        let keys: Vec<(usize, bool, bool)> = self
            .sort
            .iter()
            .filter_map(|k| {
                let c = table.index_of(k.column)?;
                Some((c, table.is_numeric(k.column), !k.descending))
            })
            .collect();
        if keys.is_empty() {
            return;
        }
        rows.sort_by(|a, b| {
            keys.iter()
                .map(|(c, numeric, up)| {
                    sort_order(&table.rows[*a][*c], &table.rows[*b][*c], *numeric, *up)
                })
                .find(|o| o.is_ne())
                .unwrap_or(std::cmp::Ordering::Equal)
        });
    }

    /// Whether `other` describes the same list, whatever copies either holds.
    pub fn same_description(&self, other: &List) -> bool {
        self.data == other.data
            && self.rows == other.rows
            && self.fill == other.fill
            && self.opened == other.opened
            && self.filter == other.filter
            && self.sort == other.sort
    }
}

/// A test's value as text: a value naming no variable is literal, so `2024-01`
/// and `12` mean what they say; one naming any is an expression, read through
/// `read`, empty when it does not evaluate.
pub fn test_value(
    value: &crate::prototype::expr::Expr,
    read: &dyn Fn(crate::variable::VarId) -> Option<crate::variable::Value>,
) -> String {
    if crate::prototype::expr::var_tokens(&value.0).is_empty() {
        return value.0.clone();
    }
    value
        .eval(read)
        .map(|v| crate::prototype::expr::format_value(&v))
        .unwrap_or_default()
}

/// The table rows list `id` draws, its filter's variables read in the mode in
/// force at the list's frame. Empty when it is no list or its set is missing.
pub fn shown(doc: &Document, id: NodeId) -> Vec<usize> {
    let Some(list) = doc.get(id).and_then(|n| n.list()) else {
        return Vec::new();
    };
    let Some(set) = doc.data_set(list.data) else {
        return Vec::new();
    };
    list.shown_rows(&set.table, &|v| doc.resolve_var(id, v))
}

/// The template of list `id`: its first child that is not a copy.
pub fn template(doc: &Document, id: NodeId) -> Option<NodeId> {
    let n = doc.get(id)?;
    let list = n.list()?;
    n.children()
        .iter()
        .copied()
        .find(|c| !list.copies.contains(c))
}

/// The list `id` is the template or a copy of, and its place: 0 for the
/// template, then the copies in order.
pub fn row_of(doc: &Document, id: NodeId) -> Option<(NodeId, usize)> {
    let parent = doc.get(id)?.parent?;
    let list = doc.get(parent)?.list()?;
    if template(doc, parent) == Some(id) {
        return Some((parent, 0));
    }
    let i = list.copies.iter().position(|c| *c == id)?;
    Some((parent, i + 1))
}

/// The list row the layer `id` sits in — the list, its place among the drawn
/// rows and the data set's row it shows — walking up from `id`.
pub fn row_at(doc: &Document, id: NodeId) -> Option<(NodeId, usize, usize)> {
    let mut at = Some(id);
    while let Some(n) = at.and_then(|a| doc.get(a)) {
        if let Some((list, place)) = row_of(doc, n.id) {
            let row = *shown(doc, list).get(place)?;
            return Some((list, place, row));
        }
        at = n.parent;
    }
    None
}

/// The list whose copy holds `id` — `id` itself or a layer inside it — so a
/// copy is picked as its list, never edited apart from its template.
pub fn copy_owner(doc: &Document, id: NodeId) -> Option<NodeId> {
    let mut at = Some(id);
    while let Some(n) = at.and_then(|a| doc.get(a)) {
        if let Some(p) = n.parent
            && doc
                .get(p)
                .and_then(|p| p.list())
                .is_some_and(|l| l.copies.contains(&n.id))
        {
            return Some(p);
        }
        at = n.parent;
    }
    None
}

/// Refuse, in words, a write landing on `id` when it is in a list's copy: the
/// copies are made again from the template whenever the list regenerates, so
/// the edit would land and then be lost without a word. Every door an agent
/// writes through asks this, not `verbs::call` alone.
pub fn refuse_copy(doc: &Document, id: NodeId) -> Result<(), String> {
    let Some(list) = copy_owner(doc, id) else {
        return Ok(());
    };
    let template = template(doc, list).map_or_else(String::new, |t| format!(" `{}`", t.to_wire()));
    Err(format!(
        "`{}` is in a row list `{}` makes from its template{template}, and is made again \
         whenever the list regenerates: edit the template, or the list's data",
        id.to_wire(),
        list.to_wire()
    ))
}

/// Every list frame in the document, in id order.
pub fn lists(doc: &Document) -> Vec<NodeId> {
    let mut out: Vec<NodeId> = doc
        .node_map()
        .values()
        .filter(|n| n.list().is_some())
        .map(|n| n.id)
        .collect();
    out.sort();
    out
}

/// The `SetList` ops that follow variant set `set`'s property `old` being
/// renamed to `new`, or deleted when `new` is `None`: a variant fill names its
/// property, so each list whose template is an instance of the set has its
/// fill on `old` renamed or dropped.
pub fn variant_prop_ops(
    doc: &Document,
    set: NodeId,
    old: &str,
    new: Option<&str>,
) -> Vec<Operation> {
    let mut ops = Vec::new();
    for id in lists(doc) {
        let Some(list) = doc.get(id).and_then(|n| n.list()) else {
            continue;
        };
        let in_set = template(doc, id)
            .and_then(|t| crate::component::main_of(doc, t))
            .and_then(|m| crate::variant::set_of(doc, m))
            == Some(set);
        let on_old = |f: &Fill| matches!(&f.target, Target::Variant(n) if n == old);
        if !in_set || !list.fill.iter().any(on_old) {
            continue;
        }
        let mut next = list.clone();
        match new {
            Some(new) => next.fill.iter_mut().filter(|f| on_old(f)).for_each(|f| {
                f.target = Target::Variant(new.to_owned());
            }),
            None => next.fill.retain(|f| !on_old(f)),
        }
        ops.push(Operation::SetList {
            id,
            list: Some(Box::new(next)),
        });
    }
    ops
}

/// The lists an applied transaction `tx` left out of date: its description or
/// its data set written, or a layer of its template changed (`dirty`, the
/// transaction's outcome) — a main's edit reaches the template through the
/// propagation, so it counts too. Also a list filling a variant property whose
/// template's set gained a variant or changed its properties, and a list whose
/// filter reads a variable moved, itself or an ancestor, into another frame.
pub fn touched(doc: &Document, tx: &Transaction, dirty: &DirtySet) -> Vec<NodeId> {
    let all = lists(doc);
    if all.is_empty() {
        return Vec::new();
    }
    let mut out: FxHashSet<NodeId> = FxHashSet::default();
    let mut sets: FxHashSet<DataSetId> = FxHashSet::default();
    // A value or a mode changed: a filter reading a variable may pass other rows.
    let mut values = false;
    for op in &tx.0 {
        match op {
            Operation::SetList { id, .. } => {
                out.insert(*id);
            }
            Operation::SetDataSet { id, .. } => {
                sets.insert(*id);
            }
            Operation::InsertDataSet { set, .. } => {
                sets.insert(set.id);
            }
            Operation::SetVariable { .. }
            | Operation::SetVariableCell { .. }
            | Operation::RemoveVariable { .. }
            | Operation::RemoveMode { .. }
            | Operation::SetModes { .. }
            | Operation::SetQueries { .. }
            | Operation::SetBreakpoints { .. } => values = true,
            _ => {}
        }
    }
    if values {
        out.extend(all.iter().copied().filter(|l| {
            doc.get(*l)
                .and_then(|n| n.list())
                .is_some_and(|x| !x.rows.is_one_row() && !x.filter_vars().is_empty())
        }));
    }
    if !sets.is_empty() {
        for l in &all {
            if doc
                .get(*l)
                .and_then(|n| n.list())
                .is_some_and(|x| sets.contains(&x.data))
            {
                out.insert(*l);
            }
        }
    }
    for id in &dirty.0 {
        let mut at = *id;
        while let Some(n) = doc.get(at) {
            let Some(p) = n.parent else { break };
            if doc.get(p).is_some_and(|p| p.list().is_some()) {
                if template(doc, p) == Some(at) {
                    out.insert(p);
                }
                break;
            }
            at = p;
        }
    }
    // A variant set given a variant, a value or a property: a row naming it
    // may switch, though no layer under the list changed. And a layer moved:
    // a list under it may now be in another mode.
    let variant_sets: FxHashSet<NodeId> = dirty
        .0
        .iter()
        .flat_map(|d| [Some(*d), doc.get(*d).and_then(|n| n.parent)])
        .flatten()
        .collect();
    let moved: FxHashSet<NodeId> =
        tx.0.iter()
            .filter_map(|op| match op {
                Operation::Reparent { id, .. } => Some(*id),
                _ => None,
            })
            .collect();
    for l in &all {
        let Some(x) = doc.get(*l).and_then(|n| n.list()) else {
            continue;
        };
        let by_variant = x
            .fill
            .iter()
            .any(|f| matches!(f.target, Target::Variant(_)))
            && template(doc, *l)
                .and_then(|t| crate::component::main_of(doc, t))
                .and_then(|m| crate::variant::set_of(doc, m))
                .is_some_and(|s| variant_sets.contains(&s));
        let by_mode = !moved.is_empty()
            && !x.rows.is_one_row()
            && !x.filter_vars().is_empty()
            && std::iter::successors(Some(*l), |a| doc.get(*a).and_then(|n| n.parent))
                .any(|a| moved.contains(&a));
        if by_variant || by_mode {
            out.insert(*l);
        }
    }
    let mut out: Vec<NodeId> = out
        .into_iter()
        .filter(|l| doc.get(*l).is_some_and(|n| n.list().is_some()))
        .collect();
    out.sort();
    out
}

/// A cell read as a boolean: true, yes, on or 1 is shown; false, no, off, 0 or
/// empty is hidden; anything else is `None`, and the row takes the main's value.
pub fn cell_bool(cell: &str) -> Option<bool> {
    match cell.trim().to_ascii_lowercase().as_str() {
        "true" | "yes" | "on" | "1" | "y" => Some(true),
        "false" | "no" | "off" | "0" | "n" | "" => Some(false),
        _ => None,
    }
}

/// The variant properties the template `root`'s set offers, by name, with each
/// one's values.
pub fn variant_props(doc: &Document, root: NodeId) -> Vec<crate::variant::VariantProp> {
    crate::component::main_of(doc, root)
        .and_then(|m| crate::variant::set_of(doc, m))
        .and_then(|s| doc.get(s))
        .and_then(|s| s.set().cloned())
        .map(|s| s.props)
        .unwrap_or_default()
}

/// The rows of `list` whose cell in a variant fill names no value of its
/// property: (property name, the cells that name none), for the card's warning.
pub fn unmatched_variants(doc: &Document, list_id: NodeId) -> Vec<(String, Vec<String>)> {
    let mut out: Vec<(String, Vec<String>)> = Vec::new();
    for (name, _, _, cell) in unmatched_variant_cells(doc, list_id) {
        match out.iter_mut().find(|(n, _)| *n == name) {
            Some((_, cells)) if !cells.contains(&cell) => cells.push(cell),
            Some(_) => {}
            None => out.push((name, vec![cell])),
        }
    }
    out
}

/// Each cell of `list`'s drawn rows that fills a variant property and names no
/// value of it: (property name, row, column index, the cell trimmed), in fill
/// then row order, for the data window to mark.
pub fn unmatched_variant_cells(
    doc: &Document,
    list_id: NodeId,
) -> Vec<(String, usize, usize, String)> {
    let Some(list) = doc.get(list_id).and_then(|n| n.list()) else {
        return Vec::new();
    };
    let Some(set) = doc.data_set(list.data) else {
        return Vec::new();
    };
    let Some(t) = template(doc, list_id) else {
        return Vec::new();
    };
    let props = variant_props(doc, t);
    let rows = shown(doc, list_id);
    let mut out = Vec::new();
    for f in &list.fill {
        let Target::Variant(name) = &f.target else {
            continue;
        };
        let Some(p) = props.iter().find(|p| p.name == *name) else {
            continue;
        };
        let Some(col) = set.table.index_of(f.column) else {
            continue;
        };
        for r in &rows {
            let cell = set.table.rows[*r][col].trim();
            if !p.values.iter().any(|v| v == cell) {
                out.push((name.clone(), *r, col, cell.to_string()));
            }
        }
    }
    out
}

/// The cells of `list`'s drawn rows that fill an image property but name no
/// picture: (property name, the cells), for the card's warning.
pub fn unmatched_pictures(doc: &Document, list_id: NodeId) -> Vec<(String, Vec<String>)> {
    let Some(list) = doc.get(list_id).and_then(|n| n.list()) else {
        return Vec::new();
    };
    let (Some(set), Some(t)) = (doc.data_set(list.data), template(doc, list_id)) else {
        return Vec::new();
    };
    let props = crate::variant::instance_properties(doc, t);
    let rows = shown(doc, list_id);
    let mut out = Vec::new();
    for f in &list.fill {
        let Target::Prop(item) = &f.target else {
            continue;
        };
        let Some(p) = props
            .iter()
            .find(|p| p.id == *item && p.kind == PropKind::Image)
        else {
            continue;
        };
        let Some(col) = set.table.index_of(f.column) else {
            continue;
        };
        let mut missing: Vec<String> = Vec::new();
        for r in &rows {
            let cell = set.table.rows[*r][col].trim();
            if !cell.is_empty()
                && set.picture(doc, cell).is_none()
                && !missing.iter().any(|m| m == cell)
            {
                missing.push(cell.to_string());
            }
        }
        if !missing.is_empty() {
            out.push((p.name.clone(), missing));
        }
    }
    out
}

/// Each property of the template a column fills by default: one whose name
/// matches the column's, ignoring case, spaces, dashes and underscores. Text and
/// boolean component properties, then the set's variant properties.
pub fn fill_by_name(doc: &Document, template: NodeId, table: &Table) -> Vec<Fill> {
    let key = |s: &str| -> String {
        s.chars()
            .filter(|c| !matches!(c, ' ' | '-' | '_'))
            .flat_map(char::to_lowercase)
            .collect()
    };
    let column = |name: &str| {
        let k = key(name);
        table
            .columns
            .iter()
            .find(|c| key(&c.name) == k)
            .map(|c| c.id)
    };
    let mut out = Vec::new();
    for p in crate::variant::instance_properties(doc, template) {
        if matches!(p.kind, PropKind::Text | PropKind::Boolean | PropKind::Image)
            && let Some(c) = column(&p.name)
        {
            out.push(Fill {
                target: Target::Prop(p.id),
                column: c,
            });
        }
    }
    for p in variant_props(doc, template) {
        if let Some(c) = column(&p.name) {
            out.push(Fill {
                target: Target::Variant(p.name.clone()),
                column: c,
            });
        }
    }
    out
}

/// *New table*'s table for `template`: one column per text, boolean and image
/// property, then per variant property, each named after it, and one row
/// holding the instance's own values (`true`/`false` for a boolean, a
/// picture's id, the value a variant shows), so [`fill_by_name`] fills every
/// property from it.
pub fn seed_table(doc: &Document, template: NodeId) -> Table {
    let mut table = Table::default();
    let mut row = Vec::new();
    let mut push = |table: &mut Table, name: &str, value: String| {
        let id = table.next_column_id();
        table.columns.push(crate::chart::Column {
            id,
            name: name.chars().take(MAX_CELL_CHARS).collect(),
        });
        row.push(value.chars().take(MAX_CELL_CHARS).collect::<String>());
    };
    for p in crate::variant::instance_properties(doc, template) {
        if table.columns.len() >= MAX_COLUMNS {
            break;
        }
        let value = match crate::variant::property_state(doc, template, &p).map(|(v, _)| v) {
            Some(PropValue::Text(t)) if p.kind == PropKind::Text => t,
            Some(PropValue::Boolean(b)) if p.kind == PropKind::Boolean => b.to_string(),
            // A picture by its id, which a cell may name.
            Some(PropValue::Image(id)) if p.kind == PropKind::Image => id.0,
            None if matches!(p.kind, PropKind::Text | PropKind::Boolean | PropKind::Image) => {
                String::new()
            }
            _ => continue,
        };
        push(&mut table, &p.name, value);
    }
    let shown = crate::component::main_of(doc, template)
        .and_then(|m| doc.get(m))
        .map(|m| m.variant().to_vec())
        .unwrap_or_default();
    for (i, p) in variant_props(doc, template).into_iter().enumerate() {
        if table.columns.len() >= MAX_COLUMNS {
            break;
        }
        push(
            &mut table,
            &p.name,
            shown.get(i).cloned().unwrap_or_default(),
        );
    }
    table.rows.push(row);
    table
}

/// Where a new list's data comes from.
pub enum Source {
    /// A data set the document holds.
    Existing(DataSetId),
    /// A new data set, added in the same step.
    New(DataSet),
}

/// Why `id` cannot become a list's template, in the words the UI shows; `None`
/// when it can.
pub fn why_not(doc: &Document, id: NodeId) -> Option<&'static str> {
    let n = doc.get(id)?;
    if n.link().is_none() || crate::component::instance_root(doc, id) != Some(id) {
        return Some("Select an instance: a list fills a component's properties");
    }
    let mut at = n.parent;
    while let Some(p) = at.and_then(|p| doc.get(p)) {
        if p.component
            || p.set().is_some()
            || p.link().is_some()
            || p.list().is_some()
            || p.chart().is_some()
        {
            return Some("A list can't go inside a component, an instance or another list");
        }
        at = p.parent;
    }
    if doc.in_vault(id) {
        return Some("Select an instance on a page");
    }
    None
}

/// The list frames standing where a list may not: a list frame that is a main,
/// a set or an instance, or sits inside one, a chart, another list or the vault.
/// [`why_not`]'s rule over the whole document, so a list frame moved or wrapped
/// into a main is caught as *Make a list* catches an instance there.
pub fn misplaced(
    nodes: &rustc_hash::FxHashMap<NodeId, crate::node::Node>,
    vault: Option<NodeId>,
) -> Vec<NodeId> {
    let mut out: Vec<NodeId> = nodes
        .values()
        .filter(|n| n.list.is_some())
        .filter(|n| {
            if n.component || n.set.is_some() || n.link.is_some() {
                return true;
            }
            let mut at = n.parent.and_then(|p| nodes.get(&p));
            while let Some(p) = at {
                if p.component
                    || p.set.is_some()
                    || p.link.is_some()
                    || p.list.is_some()
                    || p.chart.is_some()
                    || Some(p.id) == vault
                {
                    return true;
                }
                at = p.parent.and_then(|q| nodes.get(&q));
            }
            false
        })
        .map(|n| n.id)
        .collect();
    out.sort();
    out
}

/// The list frames not holding exactly one instance besides their copies: a
/// list repeats its template, so a template deleted, wrapped, or joined by a
/// layer placed ahead of it would leave nothing, or the wrong layer, repeated.
pub fn bad_templates(nodes: &rustc_hash::FxHashMap<NodeId, crate::node::Node>) -> Vec<NodeId> {
    let mut out: Vec<NodeId> = nodes
        .values()
        .filter_map(|n| Some((n, n.list.as_ref()?)))
        .filter(|(n, list)| {
            let mut own = n.children().iter().filter(|c| !list.copies.contains(c));
            let one = own.next().and_then(|c| nodes.get(c));
            own.next().is_some() || !one.is_some_and(|t| t.link.is_some())
        })
        .map(|(n, _)| n.id)
        .collect();
    out.sort();
    out
}

/// *Make a list from data*: instance `id` wrapped in a new frame flowing top to
/// bottom with an 8 px gap and hugging its rows, named after its component, the
/// frame given a list over `source` whose columns fill the properties of the
/// same name. The rows are made when the transaction is committed
/// ([`touched`], [`regenerate`]). Answers the transaction and the new frame.
pub fn make(
    doc: &Document,
    res: &crate::resolve::Resolved,
    ids: &mut IdSource,
    id: NodeId,
    source: Source,
) -> Result<(Transaction, NodeId), String> {
    use crate::container::{AlignItems, Dimension, Display, Flex, FlexDirection};
    if let Some(why) = why_not(doc, id) {
        return Err(why.to_string());
    }
    let (mut tx, frame) = crate::build::frame(doc, res, ids, &[id]).map_err(|e| e.to_string())?;
    let name = crate::component::main_of(doc, id)
        .map(|m| crate::variant::set_of(doc, m).unwrap_or(m))
        .and_then(|m| doc.get(m))
        .map_or_else(|| "List".to_string(), |m| format!("{} list", m.name()));
    tx.0.push(Operation::SetName { id: frame, name });
    tx.0.push(Operation::SetDisplay {
        id: frame,
        display: Some(Display::Flex(Flex {
            direction: FlexDirection::Column,
            align_items: AlignItems::Start,
            row_gap: 8.0,
            ..Flex::default()
        })),
    });
    tx.0.push(Operation::SetLayoutItem {
        id: frame,
        item: crate::container::LayoutItem {
            width: Dimension::FitContent,
            height: Dimension::FitContent,
            ..Default::default()
        },
    });
    let (data, table) = match source {
        Source::Existing(d) => {
            let set = doc.data_set(d).ok_or("That data set is gone")?;
            (d, set.table.clone())
        }
        Source::New(set) => {
            let d = set.id;
            let table = set.table.clone();
            tx.0.push(Operation::SetDataSet {
                id: d,
                set: Some(Box::new(set)),
            });
            (d, table)
        }
    };
    let mut list = List::new(data);
    list.fill = fill_by_name(doc, id, &table);
    tx.0.push(Operation::SetList {
        id: frame,
        list: Some(Box::new(list)),
    });
    Ok((tx, frame))
}

/// *Detach list*: the description dropped from each of `ids`, once each, its
/// copies kept as plain instances.
pub fn detach(doc: &Document, ids: &[NodeId]) -> Transaction {
    let mut seen = rustc_hash::FxHashSet::default();
    Transaction(
        ids.iter()
            .filter(|id| seen.insert(**id))
            .filter(|id| doc.get(**id).is_some_and(|n| n.list().is_some()))
            .map(|id| Operation::SetList {
                id: *id,
                list: None,
            })
            .collect(),
    )
}

/// A data set's name for a file at `path`: its file name.
pub fn name_for_path(path: &std::path::Path) -> String {
    path.file_name()
        .map(|f| f.to_string_lossy().into_owned())
        .unwrap_or_else(|| "Data".to_string())
}

/// A name no data set in `doc` has: `wanted`, else numbered past it; at most
/// [`MAX_NAME_CHARS`] either way.
pub fn free_name(doc: &Document, wanted: &str) -> String {
    free_name_among(doc.data_sets(), wanted)
}

/// [`free_name`] among `sets`. `row` is always taken: an expression reads the
/// row a row action is on as `{row.<column>}`.
pub fn free_name_among(sets: &[DataSet], wanted: &str) -> String {
    let mut taken: FxHashSet<&str> = sets.iter().map(|d| d.name.as_str()).collect();
    taken.insert(crate::prototype::expr::ROW);
    crate::naming::free_or_numbered(&taken, wanted, MAX_NAME_CHARS)
}

/// Bring list `id`'s copies to what its description and data set make of its
/// template, handing each step to `apply` in order: the old copies taken out
/// and the new ones made, then each row's variant switched, then each row's
/// properties filled. Each step is settled by `propagate::owed` before it is
/// handed over, as a commit would. `false` when `id` is not a list, or its data
/// set or template is missing, and nothing was handed over.
pub fn regenerate<E>(
    doc: &mut Document,
    id: NodeId,
    ids: &mut IdSource,
    mut apply: impl FnMut(&mut Document, Transaction) -> Result<(), E>,
) -> Result<bool, E> {
    let mut step = |doc: &mut Document, ids: &mut IdSource, ops: Vec<Operation>| {
        if ops.is_empty() {
            return Ok(());
        }
        let mut tx = Transaction(ops);
        crate::propagate::owed(doc, &mut tx, ids);
        apply(doc, tx)
    };
    let Some(list) = doc.get(id).and_then(|n| n.list()).cloned() else {
        return Ok(false);
    };
    let Some(set) = doc.data_set(list.data).cloned() else {
        return Ok(false);
    };
    let Some(tpl) = template(doc, id) else {
        return Ok(false);
    };
    let rows = list.shown_rows(&set.table, &|v| doc.resolve_var(id, v));

    // The old copies out, then the new ones in after the template, the list
    // naming them in the same step.
    // Captured before any step is handed over, so `false` means nothing changed.
    let Some(captured) = doc.capture_subtree(tpl) else {
        return Ok(false);
    };
    let gone: Vec<Operation> = list
        .copies
        .iter()
        .filter(|c| doc.get(**c).is_some_and(|n| n.parent == Some(id)))
        .map(|c| Operation::DeleteNode { id: *c })
        .collect();
    step(doc, ids, gone)?;
    let at = doc
        .get(id)
        .and_then(|n| n.children().iter().position(|c| *c == tpl))
        .map_or(1, |i| i + 1);
    let placements: Vec<crate::build::Placement> = (1..rows.len())
        .map(|_| crate::build::Placement {
            nodes: captured.clone(),
            parent: id,
            index: Some(at),
        })
        .collect();
    let (tx, made) = crate::build::insert_subtrees(doc, ids, &placements, kurbo::Vec2::ZERO);
    let mut ops = tx.0;
    // What reads a column the set no longer has is dropped here, whichever door
    // removed the column, so a later column minted under its id is read by none.
    ops.push(Operation::SetList {
        id,
        list: Some(Box::new(List {
            copies: made.clone(),
            ..list.on_columns_of(&set.table)
        })),
    });
    step(doc, ids, ops)?;

    let roots: Vec<(NodeId, usize)> = std::iter::once(tpl)
        .chain(made.iter().copied())
        .zip(rows.iter().copied())
        .collect();
    let cell = |row: usize, col: ColumnId| -> Option<&str> {
        let i = set.table.index_of(col)?;
        set.table.rows.get(row)?.get(i).map(|c| c.trim())
    };

    // Each row switched to the variant its cells name, then its text, boolean
    // and image properties set.
    let mut switches = Vec::new();
    for (root, row) in &roots {
        switches.extend(variant_ops(doc, *root, &list.fill, &|c| cell(*row, c), ids));
    }
    step(doc, ids, switches)?;
    let mut props = Vec::new();
    for (root, row) in &roots {
        props.extend(property_ops(doc, *root, &list.fill, &set, &|c| {
            cell(*row, c)
        }));
    }
    step(doc, ids, props)?;
    Ok(true)
}

/// The ops switching instance `root` to the variant its row's cells (`cell`)
/// name under `fill`. A filled property whose cell names no value takes the
/// default variant's, not the one the copy was captured showing: that is row
/// 1's last fill, so it would follow history. Empty when nothing changes.
fn variant_ops<'a>(
    doc: &Document,
    root: NodeId,
    fill: &[Fill],
    cell: &dyn Fn(ColumnId) -> Option<&'a str>,
    ids: &mut IdSource,
) -> Vec<Operation> {
    let Some(main) = crate::component::main_of(doc, root) else {
        return Vec::new();
    };
    let Some(values) = doc.get(main).map(|m| m.variant().to_vec()) else {
        return Vec::new();
    };
    let props = variant_props(doc, root);
    let default: Option<Vec<String>> = crate::variant::set_of(doc, main)
        .and_then(|s| crate::kit::default_variant(doc, s))
        .and_then(|d| doc.get(d))
        .map(|d| d.variant().to_vec());
    let mut want = values.clone();
    for f in fill {
        let Target::Variant(name) = &f.target else {
            continue;
        };
        let (Some(i), Some(c)) = (props.iter().position(|p| p.name == *name), cell(f.column))
        else {
            continue;
        };
        let value = if props[i].values.iter().any(|v| v == c) {
            Some(c)
        } else {
            default.as_ref().and_then(|d| d.get(i)).map(String::as_str)
        };
        if let (Some(v), Some(w)) = (value, want.get_mut(i)) {
            *w = v.to_string();
        }
    }
    if want == values {
        return Vec::new();
    }
    crate::variant::set_of(doc, main)
        .and_then(|s| crate::variant::find_variant(doc, s, &want))
        .and_then(|to| crate::variant::switch(doc, root, to, ids))
        .map(|tx| tx.0)
        .unwrap_or_default()
}

/// The ops setting instance `root`'s text, boolean and image properties from
/// its row's cells (`cell`) under `fill`. A cell that reads as no value resets
/// the property to the main's, for [`variant_ops`]' reason.
fn property_ops<'a>(
    doc: &Document,
    root: NodeId,
    fill: &[Fill],
    set: &DataSet,
    cell: &dyn Fn(ColumnId) -> Option<&'a str>,
) -> Vec<Operation> {
    let offered = crate::variant::instance_properties(doc, root);
    let mut ops = Vec::new();
    for f in fill {
        let Target::Prop(item) = &f.target else {
            continue;
        };
        let (Some(p), Some(c)) = (offered.iter().find(|p| p.id == *item), cell(f.column)) else {
            continue;
        };
        let value = match p.kind {
            PropKind::Text => Some(PropValue::Text(c.to_string())),
            PropKind::Boolean => cell_bool(c).map(PropValue::Boolean),
            PropKind::Image => set.picture(doc, c).map(PropValue::Image),
            PropKind::Swap | PropKind::Nested => continue,
        };
        ops.extend(match value {
            Some(v) => crate::variant::set_property(doc, &[root], p, &v),
            None => crate::variant::reset_property(doc, &[root], p),
        });
    }
    ops
}

/// The charts reading data set `set`, in id order.
pub fn charts_over(doc: &Document, set: DataSetId) -> Vec<NodeId> {
    let mut out: Vec<NodeId> = doc
        .node_map()
        .values()
        .filter(|n| n.chart().is_some_and(|c| c.data == Some(set)))
        .map(|n| n.id)
        .collect();
    out.sort();
    out
}

/// The `SetChart`s that bring every chart an applied transaction `tx` left
/// behind its data set back onto that set's table ([`crate::chart::Chart::reading`]):
/// the charts it moved onto a set, those over a set it wrote, and any chart in
/// `dirty` reading a set — a pasted chart carries the table it was copied with.
/// A chart whose set is missing keeps its table.
pub fn chart_follow_ops(doc: &Document, tx: &Transaction, dirty: &DirtySet) -> Vec<Operation> {
    let mut charts: Vec<NodeId> = dirty
        .0
        .iter()
        .copied()
        .filter(|id| {
            doc.get(*id)
                .and_then(|n| n.chart())
                .is_some_and(|c| c.data.is_some())
        })
        .collect();
    for op in &tx.0 {
        match op {
            Operation::SetChart { id, chart: Some(_) } => charts.push(*id),
            Operation::SetDataSet { id, .. } => charts.extend(charts_over(doc, *id)),
            Operation::InsertDataSet { set, .. } => charts.extend(charts_over(doc, set.id)),
            _ => {}
        }
    }
    charts.sort();
    charts.dedup();
    charts
        .into_iter()
        .filter_map(|id| {
            let chart = doc.get(id)?.chart()?;
            let data = chart.data?;
            let set = doc.data_set(data)?;
            (chart.table != set.table).then(|| Operation::SetChart {
                id,
                chart: Some(Box::new(chart.reading(data, &set.table))),
            })
        })
        .collect()
}

/// The ops of `tx` that [`regenerate_in_history`] reads beside the dirty set: a
/// list's description, a data set, a chart's description, and a variable's
/// value, a mode in force or a move (which a list filter may read), taken
/// before `tx` moves into the history.
pub fn follow_ops(tx: &Transaction) -> Transaction {
    Transaction(
        tx.0.iter()
            .filter(|op| {
                matches!(
                    op,
                    Operation::SetList { .. }
                        | Operation::SetDataSet { .. }
                        | Operation::InsertDataSet { .. }
                        | Operation::SetChart { chart: Some(_), .. }
                        | Operation::SetVariable { .. }
                        | Operation::SetVariableCell { .. }
                        | Operation::RemoveVariable { .. }
                        | Operation::RemoveMode { .. }
                        | Operation::SetModes { .. }
                        | Operation::SetQueries { .. }
                        | Operation::SetBreakpoints { .. }
                        | Operation::Reparent { .. }
                )
            })
            .cloned()
            .collect(),
    )
}

/// After `history` committed a step from `tx` that changed `dirty`: every chart
/// over a data set brought onto its table ([`chart_follow_ops`]), then every
/// list it left out of date ([`touched`]) regenerated, each step committed and
/// joined onto that step, so one undo takes the edit and its rows back
/// together. The edit's run stays open, so a scrub or a held nudge on a
/// template is still one step. What the regenerations changed is added to `dirty`.
pub fn regenerate_in_history(
    history: &mut crate::history::History,
    doc: &mut Document,
    ids: &mut IdSource,
    tx: &Transaction,
    dirty: &mut DirtySet,
) -> Result<(), crate::op::OpError> {
    let run = history.current_run();
    let follow = chart_follow_ops(doc, tx, dirty);
    if !follow.is_empty() {
        let d = history.commit(doc, Transaction(follow))?;
        history.join_last(2);
        dirty.0.extend(d.0);
    }
    for l in touched(doc, tx, dirty) {
        regenerate(doc, l, ids, |doc, step| {
            let d = history.commit(doc, step)?;
            history.join_last(2);
            dirty.0.extend(d.0);
            Ok(())
        })?;
    }
    history.resume_run(run);
    Ok(())
}

/// Whether any list or chart reads data set `set`.
pub fn is_read(doc: &Document, set: DataSetId) -> bool {
    doc.node_map().values().any(|n| {
        n.list().is_some_and(|x| x.data == set) || n.chart().is_some_and(|c| c.data == Some(set))
    })
}

/// The actor a list's copies are minted under by
/// [`regenerate_outside_history`], past every id the document holds: one per
/// list, so the same document always regenerates to the same ids.
pub fn copy_actor(list: NodeId) -> u64 {
    let mut z = (list.actor ^ list.seq.rotate_left(32)) ^ 0x115D_0000_0000_0000;
    z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
    z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
    z ^= z >> 31;
    z.max(1)
}

/// Every list in `lists` regenerated straight onto `doc`, outside history: for
/// a document built without an editing session, such as a test's. Answers what
/// changed.
pub fn regenerate_outside_history(doc: &mut Document, lists: &[NodeId]) -> DirtySet {
    let mut all = DirtySet::default();
    for l in lists {
        let mut ids = IdSource::new(copy_actor(*l));
        crate::document::reserve_existing_ids(doc, &mut ids);
        let done = regenerate(doc, *l, &mut ids, |doc, tx| {
            doc.apply(&tx).map(|o| all.0.extend(o.dirty.0))
        });
        // A regeneration the rules refuse leaves the list as it was: a bug in
        // the generator, never the user's to see.
        if let Err(e) = done {
            debug_assert!(false, "a list's regeneration was refused: {e}");
        }
    }
    all
}

/// What one column fills when a frame is drawn once per row of a table.
#[derive(Clone, Debug, PartialEq)]
pub enum RowTarget {
    /// A property of an instance frame, as a list fills its template.
    Fill(Fill),
    /// A text layer, or a layer showing a picture, under a frame that is no
    /// instance. One inside a nested instance is filled as an override.
    Layer { node: NodeId, column: ColumnId },
}

/// What `table`'s columns fill in `frame` by default. On an instance, the
/// properties [`fill_by_name`] matches. On any other frame, each text layer and
/// layer showing a picture under it whose name matches a column by
/// [`crate::content_fill::column_key`], in reading order; hidden and locked
/// layers and what they hold are left out, as is a text bound to a variable.
pub fn row_targets(doc: &Document, frame: NodeId, table: &Table) -> Vec<RowTarget> {
    if crate::component::instance_root(doc, frame) == Some(frame) {
        return fill_by_name(doc, frame, table)
            .into_iter()
            .map(RowTarget::Fill)
            .collect();
    }
    let key = crate::content_fill::column_key;
    let column = |name: &str| {
        let k = key(name);
        table
            .columns
            .iter()
            .find(|c| key(&c.name) == k)
            .map(|c| c.id)
    };
    row_slots(doc, frame)
        .into_iter()
        .filter_map(|(slot, name)| match slot {
            RowSlot::Layer(node) => Some(RowTarget::Layer {
                node,
                column: column(&name)?,
            }),
            RowSlot::Fill(_) => None,
        })
        .collect()
}

/// Something a column can fill in a *One file per row* frame: what a *Fill*
/// fold lists a row for.
#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RowSlot {
    /// A property of an instance frame, as a list's [`Target`].
    Fill(Target),
    /// A text or picture layer under a frame that is no instance.
    Layer(NodeId),
}

/// A *Fill* fold's choice for one slot, overriding what matches it by name:
/// the column that fills it, or `None` for nothing.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct RowPick {
    pub slot: RowSlot,
    pub column: Option<ColumnId>,
}

/// Every slot a column can fill in `frame`, with its name, in the order a
/// *Fill* fold lists them. On an instance, its text, boolean and image
/// properties, then its set's variant properties; on any other frame, the
/// text layers (not one bound to a variable) and layers showing a picture
/// under it, in reading order, leaving out hidden and locked layers and what
/// they hold.
pub fn row_slots(doc: &Document, frame: NodeId) -> Vec<(RowSlot, String)> {
    if crate::component::instance_root(doc, frame) == Some(frame) {
        let mut out: Vec<(RowSlot, String)> = crate::variant::instance_properties(doc, frame)
            .into_iter()
            .filter(|p| matches!(p.kind, PropKind::Text | PropKind::Boolean | PropKind::Image))
            .map(|p| (RowSlot::Fill(Target::Prop(p.id)), p.name.clone()))
            .collect();
        out.extend(
            variant_props(doc, frame)
                .into_iter()
                .map(|p| (RowSlot::Fill(Target::Variant(p.name.clone())), p.name)),
        );
        return out;
    }
    let mut out = Vec::new();
    let mut stack: Vec<NodeId> = doc
        .get(frame)
        .map(|n| n.children().iter().rev().copied().collect())
        .unwrap_or_default();
    while let Some(id) = stack.pop() {
        let Some(n) = doc.get(id) else { continue };
        if !n.visible() || n.locked() {
            continue;
        }
        let fillable = match n.kind() {
            crate::node::NodeKind::Text { .. } => {
                n.text_vars()[crate::variable::TEXT_CONTENT].is_none()
            }
            _ => n.paint().image_fill().is_some(),
        };
        if fillable {
            out.push((RowSlot::Layer(id), n.name().to_string()));
        }
        stack.extend(n.children().iter().rev());
    }
    out
}

/// [`row_targets`] with a *Fill* fold's `picks` laid over it: a picked slot
/// takes its column, or nothing; a pick naming a slot `frame` no longer has
/// is passed over.
pub fn row_targets_with(
    doc: &Document,
    frame: NodeId,
    table: &Table,
    picks: &[RowPick],
) -> Vec<RowTarget> {
    let mut out = row_targets(doc, frame, table);
    if picks.is_empty() {
        return out;
    }
    let slots: Vec<RowSlot> = row_slots(doc, frame).into_iter().map(|(s, _)| s).collect();
    for pick in picks.iter().filter(|p| slots.contains(&p.slot)) {
        out.retain(|t| match (t, &pick.slot) {
            (RowTarget::Fill(f), RowSlot::Fill(target)) => f.target != *target,
            (RowTarget::Layer { node, .. }, RowSlot::Layer(id)) => node != id,
            _ => true,
        });
        let Some(column) = pick.column.filter(|c| table.index_of(*c).is_some()) else {
            continue;
        };
        out.push(match &pick.slot {
            RowSlot::Fill(target) => RowTarget::Fill(Fill {
                target: target.clone(),
                column,
            }),
            RowSlot::Layer(node) => RowTarget::Layer {
                node: *node,
                column,
            },
        });
    }
    out
}

/// What filling a frame from rows left as it was.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct RowFilled {
    /// Cells naming no picture the table or the document holds, whose layer or
    /// property kept its own: each once, by table, row (from 0) and column, so a
    /// report over several files counts a cell once however many files read it.
    pub missing_pictures: Vec<(DataSetId, usize, ColumnId)>,
    /// The rows (from 0) in which a shown text of the filled frame is cut, or
    /// runs outside its fixed box, once laid out with the row's cells. Set by
    /// whoever lays the copy out; [`fill_row`] leaves it empty.
    pub overflowing: Vec<usize>,
}

/// Fill `frame` in `doc` from row `row` (from 0) of `set` through `targets`
/// ([`row_targets`]), straight onto `doc` and outside history: for a throwaway
/// copy that is drawn once and dropped, as *One file per row* draws each file.
pub fn fill_row(
    doc: &mut Document,
    frame: NodeId,
    set: &DataSet,
    row: usize,
    targets: &[RowTarget],
) -> RowFilled {
    let mut ids = IdSource::new(copy_actor(frame));
    crate::document::reserve_existing_ids(doc, &mut ids);
    let cells = set.table.rows.get(row);
    let cell = |col: ColumnId| -> Option<&str> {
        let i = set.table.index_of(col)?;
        cells?.get(i).map(|c| c.trim())
    };
    let missing = |c: &str| !c.is_empty() && set.picture(doc, c).is_none();
    let note = |filled: &mut RowFilled, column: ColumnId| {
        let at = (set.id, row, column);
        if !filled.missing_pictures.contains(&at) {
            filled.missing_pictures.push(at);
        }
    };
    let fill: Vec<Fill> = targets
        .iter()
        .filter_map(|t| match t {
            RowTarget::Fill(f) => Some(f.clone()),
            RowTarget::Layer { .. } => None,
        })
        .collect();
    let images = crate::variant::instance_properties(doc, frame);
    let mut filled = RowFilled::default();
    for f in &fill {
        if let Target::Prop(item) = &f.target
            && images
                .iter()
                .any(|p| p.id == *item && p.kind == PropKind::Image)
            && cell(f.column).is_some_and(missing)
        {
            note(&mut filled, f.column);
        }
    }
    let mut layers = Vec::new();
    for t in targets {
        let RowTarget::Layer { node, column } = t else {
            continue;
        };
        let (Some(n), Some(c)) = (doc.get(*node), cell(*column)) else {
            continue;
        };
        match n.kind() {
            crate::node::NodeKind::Text {
                content,
                style,
                spans,
                para_spans,
                paragraph,
                ..
            } => layers.push(Operation::SetText {
                id: *node,
                content: c.to_string(),
                spans: spans.replaced(0, content.len(), c.len(), style),
                para_spans: para_spans.replaced(0, content.len(), c.len(), paragraph),
            }),
            _ if missing(c) => note(&mut filled, *column),
            _ => {
                // The picture alone: the fill's fit and crop stay the layer's.
                let (Some(picture), Some(i)) = (set.picture(doc, c), n.paint().image_fill()) else {
                    continue;
                };
                let mut fills = n.paint().fills.clone();
                if let crate::variable::BrushSource::Literal(crate::Brush::Image(b)) =
                    &mut fills[i].brush
                {
                    b.image.id = picture;
                }
                layers.push(Operation::SetFills { id: *node, fills });
            }
        }
    }
    let switches = variant_ops(doc, frame, &fill, &cell, &mut ids);
    apply_outside(doc, switches, &mut ids);
    let props = property_ops(doc, frame, &fill, set, &cell);
    apply_outside(doc, props, &mut ids);
    apply_outside(doc, layers, &mut ids);
    filled
}

/// `ops` applied straight onto `doc` with what they owe the document's other
/// copies, outside history. A refusal is a bug in the caller, never the user's.
fn apply_outside(doc: &mut Document, ops: Vec<Operation>, ids: &mut IdSource) {
    if ops.is_empty() {
        return;
    }
    let mut tx = Transaction(ops);
    crate::propagate::owed(doc, &mut tx, ids);
    let applied = doc.apply(&tx);
    debug_assert!(applied.is_ok(), "a row's fill was refused: {applied:?}");
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A table of `rows`, its columns named by the first row.
    fn table(head: &[&str], rows: &[&[&str]]) -> Table {
        let mut t = Table::default();
        for h in head {
            let id = t.next_column_id();
            t.columns.push(crate::chart::Column {
                id,
                name: (*h).into(),
            });
        }
        t.rows = rows
            .iter()
            .map(|r| r.iter().map(|c| (*c).to_string()).collect())
            .collect();
        t
    }

    fn no_vars(_: crate::variable::VarId) -> Option<crate::variable::Value> {
        None
    }

    /// `shown_rows` caps every mode at the table and at `MAX_DRAWN`.
    #[test]
    fn shown_rows_follow_the_mode() {
        let rows = |n: usize| table(&["A"], &vec![&["x"][..]; n]);
        let mut l = List::new(DataSetId(NodeId { actor: 1, seq: 1 }));
        assert_eq!(l.shown_rows(&rows(3), &no_vars), vec![0, 1, 2]);
        assert_eq!(l.shown_rows(&rows(500), &no_vars).len(), MAX_DRAWN);
        l.rows = Rows::First(2);
        assert_eq!(l.shown_rows(&rows(3), &no_vars), vec![0, 1]);
        l.rows = Rows::Opened;
        l.opened = 7;
        assert_eq!(l.shown_rows(&rows(3), &no_vars), vec![2]);
        assert!(l.shown_rows(&rows(0), &no_vars).is_empty());
        l.rows = Rows::Row(2);
        assert_eq!(l.shown_rows(&rows(3), &no_vars), vec![1]);
        l.rows = Rows::Row(9);
        assert_eq!(
            l.shown_rows(&rows(3), &no_vars),
            vec![2],
            "past the end: the last row"
        );
    }

    /// Filter, then sort, then the mode: *First 2* is the top two after the
    /// sort, and a one-row mode ignores both.
    #[test]
    fn a_filter_and_sort_run_before_the_mode() {
        let t = table(
            &["Name", "Category", "Price"],
            &[
                &["Scarf", "Knits", "28"],
                &["Linen shirt", "Shirts", "42"],
                &["Oxford shirt", "shirts", "58"],
                &["Camp shirt", "Shirts", ""],
            ],
        );
        let (cat, price) = (t.columns[1].id, t.columns[2].id);
        let mut l = List::new(DataSetId(NodeId { actor: 1, seq: 1 }));
        l.filter.tests.push(Test {
            column: cat,
            op: TestOp::Is,
            value: crate::prototype::expr::Expr("Shirts".into()),
        });
        l.sort.push(SortKey {
            column: price,
            descending: true,
        });
        assert_eq!(
            l.shown_rows(&t, &no_vars),
            vec![2, 1, 3],
            "empty price last"
        );
        l.rows = Rows::First(1);
        assert_eq!(l.shown_rows(&t, &no_vars), vec![2]);
        l.rows = Rows::Row(1);
        assert_eq!(
            l.shown_rows(&t, &no_vars),
            vec![0],
            "a fixed row ignores the filter"
        );
        l.rows = Rows::All;
        l.filter.tests[0].value.0.clear();
        assert_eq!(
            l.shown_rows(&t, &no_vars).len(),
            4,
            "an empty value passes all"
        );
    }

    /// *Any* passes a row passing one test, *All* only one passing both; a
    /// second sort key orders the rows the first ties; `validate` refuses a
    /// ninth test, a third key and a copy named twice.
    /// flip: `passing` ignoring `filter.any` → fails at "any"; `sort_rows`
    /// reading the first key alone → fails at "then by".
    #[test]
    fn any_joins_tests_and_then_by_breaks_ties() {
        let t = table(
            &["Name", "Category", "Price"],
            &[
                &["Scarf", "Knits", "28"],
                &["Linen shirt", "Shirts", "42"],
                &["Beanie", "Knits", "28"],
                &["Oxford shirt", "Shirts", "58"],
            ],
        );
        let (name, cat, price) = (t.columns[0].id, t.columns[1].id, t.columns[2].id);
        let test = |column, op, value: &str| Test {
            column,
            op,
            value: crate::prototype::expr::Expr(value.into()),
        };
        let mut l = List::new(DataSetId(NodeId { actor: 1, seq: 1 }));
        l.filter.tests = vec![
            test(cat, TestOp::Is, "Knits"),
            test(name, TestOp::Contains, "Oxford"),
        ];
        assert!(
            l.shown_rows(&t, &no_vars).is_empty(),
            "all: none passes both"
        );
        l.filter.any = true;
        assert_eq!(l.shown_rows(&t, &no_vars), vec![0, 2, 3], "any");
        l.filter = Filter::default();
        l.sort = vec![
            SortKey {
                column: price,
                descending: false,
            },
            SortKey {
                column: name,
                descending: false,
            },
        ];
        assert_eq!(l.shown_rows(&t, &no_vars), vec![2, 0, 1, 3], "then by");

        let frame = NodeId { actor: 1, seq: 9 };
        assert!(l.validate(frame).is_ok());
        let mut bad = l.clone();
        bad.filter.tests = vec![test(cat, TestOp::Is, "Knits"); MAX_TESTS + 1];
        assert_eq!(bad.validate(frame), Err(ListRule::BadFilter), "tests");
        let mut bad = l.clone();
        bad.sort.push(SortKey {
            column: cat,
            descending: true,
        });
        assert_eq!(bad.validate(frame), Err(ListRule::BadFilter), "keys");
        let mut bad = l.clone();
        let copy = NodeId { actor: 1, seq: 10 };
        bad.copies = vec![copy, copy];
        assert_eq!(bad.validate(frame), Err(ListRule::BadCopies), "twice");
        bad.copies = vec![frame];
        assert_eq!(bad.validate(frame), Err(ListRule::BadCopies), "itself");
    }

    /// A value naming a variable reads it; one naming none is literal text.
    #[test]
    fn a_test_value_is_literal_unless_it_names_a_variable() {
        use crate::prototype::expr::Expr;
        use crate::variable::{Value, VarId};
        let v = VarId(NodeId { actor: 9, seq: 4 });
        let read = move |x: VarId| (x == v).then(|| Value::String("Knits".into()));
        assert_eq!(test_value(&Expr("2024-01".into()), &read), "2024-01");
        let e = Expr(format!("{{{}}}", v.0.to_wire()));
        assert_eq!(test_value(&e, &read), "Knits");
        assert!(TestOp::Contains.passes("Linen Shirt", "shirt"));
        assert!(TestOp::AtLeast.passes("12", "12"));
        assert!(!TestOp::Less.passes("abc", "12"), "a word is no number");
        assert!(TestOp::Empty.passes("  ", "ignored"));
    }

    /// A boolean cell reads the usual spellings and leaves anything else alone.
    #[test]
    fn cells_read_as_booleans() {
        assert_eq!(cell_bool(" Yes"), Some(true));
        assert_eq!(cell_bool("0"), Some(false));
        assert_eq!(cell_bool(""), Some(false));
        assert_eq!(cell_bool("maybe"), None);
    }
}
