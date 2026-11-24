//! The verbs over the document's tables (`list::DataSet`): their cells and names,
//! the lists made from them and the charts reading them. A table is named by its
//! id or its name, rows count from 1, and every cell is text. An edit is one
//! `SetDataSet`; the commit regenerates every list and chart reading the table.

use super::{Args, Ctx, Done, VerbError, plain, plural};
use crate::chart::{Column, ColumnId, MAX_CELL_CHARS, MAX_COLUMNS, MAX_ROWS, Table};
use crate::document::Document;
use crate::id::NodeId;
use crate::list::{self, DataSet, DataSetId, List, Rows, Target, TestOp};
use crate::op::Operation;
use crate::variant::PropKind;
use serde::Deserialize;
use std::collections::BTreeMap;

/// A cell as a caller sends it: text, or a number or true/false, kept as written.
#[derive(Deserialize)]
#[serde(untagged)]
pub(super) enum CellIn {
    Text(String),
    Number(serde_json::Number),
    Bool(bool),
}

impl CellIn {
    fn text(self) -> String {
        match self {
            CellIn::Text(s) => s,
            CellIn::Number(n) => n.to_string(),
            CellIn::Bool(b) => b.to_string(),
        }
    }
}

/// A column as a caller names it: by its name, or by its id as `get_table` gives it.
#[derive(Deserialize)]
#[serde(untagged)]
pub(super) enum ColumnRef {
    Id(u32),
    Name(String),
}

/// One cell `edit_table` writes.
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct CellEdit {
    row: usize,
    column: ColumnRef,
    value: CellIn,
}

/// `edit_table`'s `columns`: names to add, renames old to new, names to delete.
#[derive(Deserialize, Default)]
#[serde(deny_unknown_fields)]
pub(super) struct ColumnEdits {
    #[serde(default)]
    add: Vec<String>,
    #[serde(default)]
    rename: BTreeMap<String, String>,
    #[serde(default)]
    delete: Vec<String>,
}

/// `set_list`'s `filter`, its columns by name.
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct FilterIn {
    #[serde(default)]
    any: bool,
    #[serde(default)]
    tests: Vec<TestIn>,
}

/// One filter test as a caller writes it.
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct TestIn {
    column: ColumnRef,
    op: String,
    #[serde(default)]
    value: Option<CellIn>,
}

/// One sort column as a caller writes it.
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct SortIn {
    column: ColumnRef,
    #[serde(default)]
    descending: bool,
}

/// The table `text` names: the one of that id, else the one of that name. A name
/// matching none, or more than one, is refused listing the tables.
pub fn find_table<'d>(doc: &'d Document, text: &str) -> Result<&'d DataSet, String> {
    if let Some(id) = NodeId::from_wire(text)
        && let Some(set) = doc.data_set(DataSetId(id))
    {
        return Ok(set);
    }
    let named: Vec<&DataSet> = doc.data_sets().iter().filter(|d| d.name == text).collect();
    match named.as_slice() {
        [one] => Ok(one),
        [] => Err(format!(
            "no table is called `{text}`; {}",
            tables_listed(doc)
        )),
        _ => Err(format!(
            "{} tables are called `{text}`: give its id; {}",
            named.len(),
            tables_listed(doc)
        )),
    }
}

/// The document's tables by name and id, for a refusal.
fn tables_listed(doc: &Document) -> String {
    const LISTED: usize = 12;
    let sets = doc.data_sets();
    if sets.is_empty() {
        return "the document has no tables".into();
    }
    let named: Vec<String> = sets
        .iter()
        .take(LISTED)
        .map(|d| format!("`{}` ({})", d.name, d.id.0.to_wire()))
        .collect();
    match sets.len().saturating_sub(LISTED) {
        0 => format!("the tables are {}", named.join(", ")),
        more => format!("the tables are {} and {more} more", named.join(", ")),
    }
}

/// The table parameter `param` names.
fn table_of<'d>(
    doc: &'d Document,
    a: &Args<'_>,
    param: &'static str,
) -> Result<&'d DataSet, VerbError> {
    let text = a.text(param).ok_or(VerbError::Missing {
        verb: a.verb.name,
        param,
    })?;
    find_table(doc, text).map_err(|why| a.invalid(param, why))
}

/// The lists and the charts reading table `set`, each in id order.
pub fn readers(doc: &Document, set: DataSetId) -> (Vec<NodeId>, Vec<NodeId>) {
    let lists = list::lists(doc)
        .into_iter()
        .filter(|l| {
            doc.get(*l)
                .and_then(|n| n.list())
                .is_some_and(|x| x.data == set)
        })
        .collect();
    (lists, list::charts_over(doc, set))
}

/// A table's readers in words, as *Data tables* says them: *2 lists and 1 chart*.
pub fn readers_words(lists: usize, charts: usize) -> String {
    let mut out = Vec::new();
    if lists > 0 {
        out.push(plural(lists, "list", "lists"));
    }
    if charts > 0 {
        out.push(plural(charts, "chart", "charts"));
    }
    out.join(" and ")
}

/// Refuse an edit of a linked table's cells: the next *Update* would read the
/// file over it.
fn refuse_linked(a: &Args<'_>, param: &'static str, set: &DataSet) -> Result<(), VerbError> {
    let Some(link) = &set.source else {
        return Ok(());
    };
    let file = link.path.rsplit(['/', '\\']).next().unwrap_or(&link.path);
    Err(a.invalid(
        param,
        format!("{} is linked to {file}: unlink it first", set.name),
    ))
}

/// `table`, refused past the table limits, in words a caller can act on.
fn within_limits(a: &Args<'_>, param: &'static str, table: &Table) -> Result<(), VerbError> {
    if table.columns.len() > MAX_COLUMNS {
        return Err(a.invalid(param, format!("At most {MAX_COLUMNS} columns")));
    }
    if table.rows.len() > MAX_ROWS {
        return Err(a.invalid(param, format!("At most {MAX_ROWS} rows")));
    }
    if let Some(c) = table
        .columns
        .iter()
        .find(|c| c.name.chars().count() > MAX_CELL_CHARS)
    {
        let why = format!(
            "column `{}…`: a name holds at most {MAX_CELL_CHARS} characters",
            c.name.chars().take(24).collect::<String>()
        );
        return Err(a.invalid(param, why));
    }
    for (r, row) in table.rows.iter().enumerate() {
        if let Some(c) = row.iter().position(|c| c.chars().count() > MAX_CELL_CHARS) {
            let column = table.columns.get(c).map_or("", |c| c.name.as_str());
            let why = format!(
                "row {}, `{column}`: a cell holds at most {MAX_CELL_CHARS} characters",
                r + 1
            );
            return Err(a.invalid(param, why));
        }
    }
    list::validate_table(table).map_err(|rule| a.invalid(param, rule))
}

/// The column of `table` (called `name` in refusals) that `col` names.
fn column_of(
    a: &Args<'_>,
    param: &'static str,
    name: &str,
    table: &Table,
    col: &ColumnRef,
) -> Result<ColumnId, VerbError> {
    let found = match col {
        ColumnRef::Name(n) => table.columns.iter().find(|c| c.name == *n),
        ColumnRef::Id(id) => table.column(ColumnId(*id)),
    };
    found.map(|c| c.id).ok_or_else(|| {
        let said = match col {
            ColumnRef::Name(n) => format!("`{n}`"),
            ColumnRef::Id(id) => format!("column id {id}"),
        };
        a.invalid(
            param,
            format!("{said} is no column of {name}; {}", columns_listed(table)),
        )
    })
}

/// The columns of `table` by name, for a refusal.
fn columns_listed(table: &Table) -> String {
    match table.columns.is_empty() {
        true => "it has none".into(),
        false => {
            let names: Vec<String> = table
                .columns
                .iter()
                .map(|c| format!("`{}`", c.name))
                .collect();
            format!("its columns are {}", names.join(", "))
        }
    }
}

/// Row `row`, counted from 1, as an index into `set`'s rows, or why it is none.
fn row_index(
    a: &Args<'_>,
    param: &'static str,
    set: &DataSet,
    row: usize,
) -> Result<usize, VerbError> {
    let len = set.table.rows.len();
    match row {
        0 => Err(a.invalid(param, "rows count from 1")),
        r if r > len => Err(a.invalid(
            param,
            format!("{} has {}, not {r}", set.name, plural(len, "row", "rows")),
        )),
        r => Ok(r - 1),
    }
}

/// `set` with `table` in place of its own, and a line saying its row count.
fn written(set: &DataSet, table: Table) -> Done {
    let said = format!("{}: {}", set.name, plural(table.rows.len(), "row", "rows"));
    Done::ops(vec![Operation::SetDataSet {
        id: set.id,
        set: Some(Box::new(DataSet {
            table,
            ..set.clone()
        })),
    }])
    .saying(said)
}

/// A table of `names` over `rows`, each column named apart from those before it
/// and numbered by place; a row of another width is refused. A name is trimmed
/// unless `old` has a column of exactly that name, so a table sent back as it
/// was read keeps a name with edge spaces, and with it the column's id.
fn table_from(
    a: &Args<'_>,
    names: Vec<String>,
    rows: Vec<Vec<CellIn>>,
    old: Option<&Table>,
) -> Result<Table, VerbError> {
    let mut table = Table::default();
    for name in names {
        let kept = old.is_some_and(|o| o.columns.iter().any(|c| c.name == name));
        let name = table.free_column_name(if kept { &name } else { name.trim() }, None);
        let id = table.next_column_id();
        table.columns.push(Column { id, name });
    }
    for (i, row) in rows.into_iter().enumerate() {
        if row.len() != table.columns.len() {
            let why = format!(
                "row {} has {}, not {}",
                i + 1,
                plural(row.len(), "cell", "cells"),
                table.columns.len()
            );
            return Err(a.invalid("rows", why));
        }
        table.rows.push(row.into_iter().map(CellIn::text).collect());
    }
    Ok(table)
}

/// A name for a table: trimmed and cut to the longest a name may be.
fn clipped(name: &str) -> String {
    name.trim().chars().take(list::MAX_NAME_CHARS).collect()
}

/// `add_table`: a new table, *Table N* when unnamed; with no columns *Column 1*
/// and one empty row, as *New table* makes. Answers its id.
pub(super) fn add_table(ctx: &mut Ctx<'_>, a: &Args<'_>) -> Result<Done, VerbError> {
    let wanted = a.text("name").map(clipped).unwrap_or_default();
    let wanted = match wanted.is_empty() {
        true => "Table 1".to_owned(),
        false => wanted,
    };
    let name = list::free_name(ctx.doc, &wanted);
    let columns = a.model_opt::<Vec<String>>("columns")?;
    let rows = a.model_opt::<Vec<Vec<CellIn>>>("rows")?;
    let table = match (columns, rows) {
        (None, Some(_)) => return Err(a.invalid("rows", "give `columns` with `rows`")),
        (None, None) => Table {
            columns: vec![Column {
                id: ColumnId(0),
                name: "Column 1".into(),
            }],
            rows: vec![vec![String::new()]],
        },
        (Some(names), rows) => table_from(a, names, rows.unwrap_or_default(), None)?,
    };
    within_limits(a, "columns", &table)?;
    let id = ctx.ids.mint_data_set();
    let made = Done::making(
        vec![Operation::SetDataSet {
            id,
            set: Some(Box::new(DataSet {
                id,
                name: name.clone(),
                table,
                source: None,
                pictures: Vec::new(),
            })),
        }],
        id.0.to_wire(),
    );
    Ok(match name == wanted {
        true => made,
        false => made.saying(format!("`{wanted}` is taken: named it `{name}`")),
    })
}

/// `edit_table`: cells set, rows added and deleted, columns added, renamed and
/// deleted, in one step. Rows are numbered as the table stands before the call;
/// `set` and `add` name columns as they stand after `columns`. Column deletes go
/// before renames, so a rename may take a deleted or renamed-away name.
pub(super) fn edit_table(ctx: &mut Ctx<'_>, a: &Args<'_>) -> Result<Done, VerbError> {
    let set = table_of(ctx.doc, a, "table")?;
    if ["set", "add", "delete", "columns"]
        .iter()
        .all(|p| a.get(p).is_none())
    {
        return Err(VerbError::Nothing(
            "`edit_table` changes `set`, `add`, `delete` or `columns`",
        ));
    }
    refuse_linked(a, "table", set)?;
    let mut t = set.table.clone();
    let len = t.rows.len();
    if let Some(edits) = a.model_opt::<ColumnEdits>("columns")? {
        let named = |n: &String| {
            column_of(
                a,
                "columns",
                &set.name,
                &set.table,
                &ColumnRef::Name(n.clone()),
            )
        };
        let renames = edits
            .rename
            .iter()
            .map(|(old, new)| Ok((named(old)?, new)))
            .collect::<Result<Vec<_>, VerbError>>()?;
        let deletes = edits
            .delete
            .iter()
            .map(named)
            .collect::<Result<Vec<_>, VerbError>>()?;
        for id in deletes {
            if let Some(i) = t.index_of(id) {
                t.columns.remove(i);
                t.rows.iter_mut().for_each(|r| {
                    r.remove(i);
                });
            }
        }
        // The renames are named together, against the columns neither deleted
        // nor renamed, so a rename may take a name freed in the same call.
        let mut taken: rustc_hash::FxHashSet<String> = t
            .columns
            .iter()
            .filter(|c| renames.iter().all(|(id, _)| *id != c.id))
            .map(|c| c.name.clone())
            .collect();
        for (id, new) in renames {
            let Some(i) = t.index_of(id) else { continue };
            let free: rustc_hash::FxHashSet<&str> = taken.iter().map(String::as_str).collect();
            let name = crate::chart::free_among(&free, new.trim());
            taken.insert(name.clone());
            t.columns[i].name = name;
        }
        for name in &edits.add {
            if t.columns.len() >= MAX_COLUMNS {
                return Err(a.invalid("columns", format!("At most {MAX_COLUMNS} columns")));
            }
            let name = t.free_column_name(name.trim(), None);
            // Past the ids the table had before the call too: a column deleted
            // in it is still read by the lists until they regenerate.
            let id = ColumnId(t.next_column_id().0.max(set.table.next_column_id().0));
            t.columns.push(Column { id, name });
            t.rows.iter_mut().for_each(|r| r.push(String::new()));
        }
    }
    for edit in a.model_opt::<Vec<CellEdit>>("set")?.unwrap_or_default() {
        let r = row_index(a, "set", set, edit.row)?;
        let c = column_of(a, "set", &set.name, &t, &edit.column)?;
        let i = t.index_of(c).unwrap_or_default();
        t.rows[r][i] = edit.value.text();
    }
    let mut gone = rustc_hash::FxHashSet::default();
    for row in a.model_opt::<Vec<usize>>("delete")?.unwrap_or_default() {
        gone.insert(row_index(a, "delete", set, row)?);
    }
    let mut added = Vec::new();
    for row in a
        .model_opt::<Vec<BTreeMap<String, CellIn>>>("add")?
        .unwrap_or_default()
    {
        let mut cells = vec![String::new(); t.columns.len()];
        for (name, value) in row {
            let c = column_of(a, "add", &set.name, &t, &ColumnRef::Name(name))?;
            cells[t.index_of(c).unwrap_or_default()] = value.text();
        }
        added.push(cells);
    }
    let at = match a.index("at") {
        None => len,
        Some(0) => return Err(a.invalid("at", "rows count from 1")),
        Some(n) if n > len + 1 => {
            let why = format!(
                "{} has {}, not {}",
                set.name,
                plural(len, "row", "rows"),
                n - 1
            );
            return Err(a.invalid("at", why));
        }
        Some(n) => n - 1,
    };
    let mut rows = Vec::with_capacity(len + added.len());
    for (i, row) in std::mem::take(&mut t.rows).into_iter().enumerate() {
        if i == at {
            rows.append(&mut added);
        }
        if !gone.contains(&i) {
            rows.push(row);
        }
    }
    rows.append(&mut added);
    t.rows = rows;
    within_limits(a, "table", &t)?;
    Ok(written(set, t))
}

/// `set_table`: the whole table replaced, its id kept; a column keeps its id
/// where the old table has one of its name, so the fills reading it hold.
pub(super) fn set_table(ctx: &mut Ctx<'_>, a: &Args<'_>) -> Result<Done, VerbError> {
    let set = table_of(ctx.doc, a, "table")?;
    refuse_linked(a, "table", set)?;
    let mut t = table_from(a, a.model("columns")?, a.model("rows")?, Some(&set.table))?;
    t.take_ids_by_name(&set.table);
    within_limits(a, "rows", &t)?;
    Ok(written(set, t))
}

/// `rename_table`: undo step *Rename table*; a name another table has is
/// numbered past it.
pub(super) fn rename_table(ctx: &mut Ctx<'_>, a: &Args<'_>) -> Result<Done, VerbError> {
    let set = table_of(ctx.doc, a, "table")?;
    let wanted = clipped(a.text("name").unwrap_or_default());
    if wanted.is_empty() {
        return Err(a.invalid("name", "a table needs a name"));
    }
    if wanted == set.name {
        return Err(VerbError::Nothing("The table already has that name"));
    }
    let name = list::free_name(ctx.doc, &wanted);
    let done = Done::ops(vec![Operation::SetDataSet {
        id: set.id,
        set: Some(Box::new(DataSet {
            name: name.clone(),
            ..set.clone()
        })),
    }]);
    Ok(match name == wanted {
        true => done,
        false => done.saying(format!("`{wanted}` is taken: named it `{name}`")),
    })
}

/// `delete_table`: refused while a list or chart reads the table. A linked
/// table's file is never touched.
pub(super) fn delete_table(ctx: &mut Ctx<'_>, a: &Args<'_>) -> Result<Done, VerbError> {
    let set = table_of(ctx.doc, a, "table")?;
    let (lists, charts) = readers(ctx.doc, set.id);
    if !lists.is_empty() || !charts.is_empty() {
        let why = format!(
            "Delete table: used by {}. Point them at another table first.",
            readers_words(lists.len(), charts.len())
        );
        return Err(a.invalid("table", why));
    }
    Ok(Done::ops(vec![Operation::SetDataSet {
        id: set.id,
        set: None,
    }])
    .saying(format!("Deleted {}", set.name)))
}

/// `unlink_table`: the table keeps its cells and drops its file, as the card's
/// *Unlink*.
pub(super) fn unlink_table(ctx: &mut Ctx<'_>, a: &Args<'_>) -> Result<Done, VerbError> {
    let set = table_of(ctx.doc, a, "table")?;
    if set.source.is_none() {
        let why = format!("{} is not linked to a file", set.name);
        return Err(a.invalid("table", why));
    }
    Ok(Done::ops(vec![Operation::SetDataSet {
        id: set.id,
        set: Some(Box::new(DataSet {
            source: None,
            ..set.clone()
        })),
    }]))
}

/// The fills `param` maps, property name to column name, for `template` over
/// `set`'s table, in the template's property order.
fn fills_named(
    doc: &Document,
    a: &Args<'_>,
    param: &'static str,
    template: NodeId,
    set: &DataSet,
) -> Result<Vec<list::Fill>, VerbError> {
    let map: BTreeMap<String, String> = a.model(param)?;
    let props: Vec<(String, Target)> = crate::variant::instance_properties(doc, template)
        .into_iter()
        .filter(|p| matches!(p.kind, PropKind::Text | PropKind::Boolean | PropKind::Image))
        .map(|p| (p.name.clone(), Target::Prop(p.id)))
        .chain(
            list::variant_props(doc, template)
                .into_iter()
                .map(|p| (p.name.clone(), Target::Variant(p.name))),
        )
        .collect();
    for prop in map.keys() {
        if !props.iter().any(|(n, _)| n == prop) {
            let names: Vec<String> = props.iter().map(|(n, _)| format!("`{n}`")).collect();
            let listed = match names.is_empty() {
                true => "it fills none".to_owned(),
                false => format!("it fills {}", names.join(", ")),
            };
            return Err(a.invalid(
                param,
                format!("`{prop}` is no property the template's rows fill; {listed}"),
            ));
        }
    }
    props
        .into_iter()
        .filter_map(|(name, target)| Some((map.get(&name)?, target)))
        .map(|(col, target)| {
            let column = column_of(
                a,
                param,
                &set.name,
                &set.table,
                &ColumnRef::Name(col.clone()),
            )?;
            Ok(list::Fill { target, column })
        })
        .collect()
}

/// `make_list`: instance `node` made a list over a table, as *Make a list from
/// data*; its properties filled by `fill`, else by the columns of their names.
pub(super) fn make_list(ctx: &mut Ctx<'_>, a: &Args<'_>) -> Result<Done, VerbError> {
    let id = a.layer(ctx.doc, "node")?;
    super::node(ctx, id)?;
    let set = table_of(ctx.doc, a, "table")?;
    let fill = match a.get("fill") {
        Some(_) => Some(fills_named(ctx.doc, a, "fill", id, set)?),
        None => None,
    };
    let (mut tx, frame) = list::make(
        ctx.doc,
        ctx.res,
        ctx.ids,
        id,
        list::Source::Existing(set.id),
    )
    .map_err(|why| a.invalid("node", why))?;
    if let Some(fill) = fill {
        for op in &mut tx.0 {
            if let Operation::SetList { list: Some(l), .. } = op {
                l.fill.clone_from(&fill);
            }
        }
    }
    let rows = set.table.rows.len().min(list::MAX_DRAWN);
    Ok(Done::made(tx, vec![frame]).saying(format!(
        "A list of {} from {}",
        plural(rows, "row", "rows"),
        set.name
    )))
}

/// `now`, list `id`'s description, moved onto `set`, as the List card's table
/// dropdown moves it: a fill keeps its column where `set` has one of that name,
/// a property left unfilled takes the column of its own name. The filter's and
/// the sort's columns move by name too, dropped where `set` has none. The card
/// and `set_list` both move a list through it.
pub fn moved_onto(doc: &Document, id: NodeId, now: &List, set: &DataSet) -> List {
    let old = doc.data_set(now.data).map(|d| &d.table);
    let onto = |c: ColumnId| -> Option<ColumnId> {
        let name = &old?.column(c)?.name;
        set.table
            .columns
            .iter()
            .find(|x| x.name == *name)
            .map(|x| x.id)
    };
    let mut fill: Vec<list::Fill> = now
        .fill
        .iter()
        .filter_map(|f| {
            Some(list::Fill {
                target: f.target.clone(),
                column: onto(f.column)?,
            })
        })
        .collect();
    if let Some(t) = list::template(doc, id) {
        for f in list::fill_by_name(doc, t, &set.table) {
            if !fill.iter().any(|g| g.target == f.target) {
                fill.push(f);
            }
        }
    }
    let mut filter = now.filter.clone();
    filter.tests = now
        .filter
        .tests
        .iter()
        .filter_map(|t| {
            Some(list::Test {
                column: onto(t.column)?,
                ..t.clone()
            })
        })
        .collect();
    let sort = now
        .sort
        .iter()
        .filter_map(|k| {
            Some(list::SortKey {
                column: onto(k.column)?,
                ..*k
            })
        })
        .collect();
    List {
        data: set.id,
        fill,
        filter,
        sort,
        ..now.clone()
    }
}

/// The filter test word `word` names: its snake_case form or the List card's
/// words, in any case.
fn test_op(word: &str) -> Option<TestOp> {
    TestOp::ALL.into_iter().find(|op| {
        let snake = serde_json::to_value(op)
            .ok()
            .and_then(|v| v.as_str().map(str::to_owned))
            .unwrap_or_default();
        plain(word) == plain(&snake) || plain(word) == plain(op.words())
    })
}

/// `set_list`: the List card's fields of a list, any of them: the table it
/// reads, which rows it draws, which column fills which property, its filter and
/// its sort, columns named in the table it reads after the call.
pub(super) fn set_list(ctx: &mut Ctx<'_>, a: &Args<'_>) -> Result<Done, VerbError> {
    let doc = ctx.doc;
    let given = a.layer(doc, "node")?;
    super::node(ctx, given)?;
    let id = match doc.get(given).and_then(|n| n.list()) {
        Some(_) => given,
        None => match list::row_of(doc, given) {
            Some((l, 0)) => l,
            _ => return Err(a.invalid("node", "not a list: `make_list` makes one")),
        },
    };
    let Some(now) = doc.get(id).and_then(|n| n.list()).cloned() else {
        return Err(a.invalid("node", "not a list"));
    };
    if ["table", "rows", "fill", "filter", "sort"]
        .iter()
        .all(|p| a.get(p).is_none())
    {
        return Err(VerbError::Nothing(
            "`set_list` changes `table`, `rows`, `fill`, `filter` or `sort`",
        ));
    }
    let (mut next, set) = match a.get("table") {
        Some(_) => {
            let set = table_of(doc, a, "table")?;
            (moved_onto(doc, id, &now, set), Some(set))
        }
        None => (now.clone(), doc.data_set(now.data)),
    };
    let set_for = |param: &'static str| {
        set.ok_or_else(|| a.invalid(param, "the list's table is gone: pass `table`"))
    };
    if let Some(rows) = a.model_opt::<Rows>("rows")? {
        match rows {
            Rows::First(n) if n == 0 || n as usize > list::MAX_DRAWN => {
                let why = format!("`first` is 1 to {}", list::MAX_DRAWN);
                return Err(a.invalid("rows", why));
            }
            Rows::Row(0) => return Err(a.invalid("rows", "rows count from 1")),
            _ => {}
        }
        next.rows = rows;
    }
    if a.get("fill").is_some() {
        let template =
            list::template(doc, id).ok_or_else(|| a.invalid("node", "the list has no template"))?;
        next.fill = fills_named(doc, a, "fill", template, set_for("fill")?)?;
    }
    if let Some(f) = a.model_opt::<FilterIn>("filter")? {
        let set = set_for("filter")?;
        if f.tests.len() > list::MAX_TESTS {
            let why = format!("a filter holds at most {} tests", list::MAX_TESTS);
            return Err(a.invalid("filter", why));
        }
        let mut tests = Vec::new();
        for t in f.tests {
            let column = column_of(a, "filter", &set.name, &set.table, &t.column)?;
            let op = test_op(&t.op).ok_or_else(|| {
                let words: Vec<String> = TestOp::ALL
                    .iter()
                    .filter_map(|o| serde_json::to_value(o).ok())
                    .filter_map(|v| v.as_str().map(|w| format!("`{w}`")))
                    .collect();
                a.invalid(
                    "filter",
                    format!("`{}` is no test; the tests are {}", t.op, words.join(", ")),
                )
            })?;
            let text = match op.takes_value() {
                true => t.value.map(CellIn::text).unwrap_or_default(),
                false => String::new(),
            };
            let value = match crate::prototype::expr::var_tokens(&text).is_empty() {
                true => crate::prototype::expr::Expr(text),
                false => {
                    let vars = doc.variables();
                    let (e, error) = crate::prototype::expr::Expr::from_names(&text, &|n| {
                        vars.by_name(n).map(|v| v.id)
                    });
                    if let Some(error) = error {
                        return Err(a.invalid("filter", format!("`{text}`: {error}")));
                    }
                    e
                }
            };
            tests.push(list::Test { column, op, value });
        }
        next.filter = list::Filter { any: f.any, tests };
    }
    if let Some(keys) = a.model_opt::<Vec<SortIn>>("sort")? {
        let set = set_for("sort")?;
        if keys.len() > list::MAX_SORT {
            let why = format!("a sort compares at most {} columns", list::MAX_SORT);
            return Err(a.invalid("sort", why));
        }
        next.sort = keys
            .into_iter()
            .map(|k| {
                Ok(list::SortKey {
                    column: column_of(a, "sort", &set.name, &set.table, &k.column)?,
                    descending: k.descending,
                })
            })
            .collect::<Result<_, VerbError>>()?;
    }
    next.validate(id).map_err(|rule| a.invalid("node", rule))?;
    Ok(Done::ops(vec![Operation::SetList {
        id,
        list: Some(Box::new(next)),
    }]))
}

/// `set_chart_data`: a chart pointed at a table of the document's (`data`), or
/// given new cells (`table`). On a chart reading a table the cells go into that
/// table, so every reader follows, unless `write` is `chart`, which gives the
/// chart its own copy; a table other lists or charts read too is written only
/// when `write` says which.
pub(super) fn set_chart_data(ctx: &mut Ctx<'_>, a: &Args<'_>) -> Result<Done, VerbError> {
    let (id, chart) = super::chart_of(ctx, a)?;
    let own = |table: Table| -> Result<Done, VerbError> {
        let mut chart = chart.with_table(table);
        chart.source = None;
        chart.data = None;
        super::chart_valid(a, &chart)?;
        Ok(Done::ops(vec![Operation::SetChart {
            id,
            chart: Some(Box::new(chart)),
        }]))
    };
    let table = a.model_opt::<Table>("table")?;
    match (a.get("data"), table) {
        (Some(_), Some(_)) => Err(a.invalid("data", "give `data` or `table`, not both")),
        (None, None) => Err(VerbError::Nothing(
            "`set_chart_data` takes `table` or `data`",
        )),
        (Some(_), None) => {
            if a.get("write").is_some() {
                return Err(a.invalid("write", "`write` goes with `table`"));
            }
            let set = table_of(ctx.doc, a, "data")?;
            let next = chart.reading(set.id, &set.table);
            super::chart_valid(a, &next)?;
            Ok(Done::ops(vec![Operation::SetChart {
                id,
                chart: Some(Box::new(next)),
            }]))
        }
        (None, Some(table)) => {
            let Some(set) = chart.data.and_then(|d| ctx.doc.data_set(d)) else {
                if a.choice("write") == Some("table") {
                    let why = "this chart has a table of its own: leave `write` out";
                    return Err(a.invalid("write", why));
                }
                return own(table);
            };
            let into_set = match a.choice("write") {
                Some("chart") => false,
                Some(_) => true,
                None => {
                    let (lists, charts) = readers(ctx.doc, set.id);
                    let charts = charts.iter().filter(|c| **c != id).count();
                    if !lists.is_empty() || charts > 0 {
                        let others = readers_words(lists.len(), charts);
                        let name = &set.name;
                        return Err(a.invalid(
                            "table",
                            format!(
                                "This chart reads {name}, which {others} also read. Pass write = \
                                 \"table\" to change {name} for all of them, or write = \"chart\" \
                                 to give this chart its own copy."
                            ),
                        ));
                    }
                    true
                }
            };
            if !into_set {
                return own(table);
            }
            refuse_linked(a, "table", set)?;
            let mut table = table;
            table.name_columns_apart();
            table.take_ids_by_name(&set.table);
            within_limits(a, "table", &table)?;
            super::chart_valid(a, &chart.reading(set.id, &table))?;
            Ok(written(set, table))
        }
    }
}
