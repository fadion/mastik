//! `tables`: a script's handle on the document's tables, sugar over the table
//! verbs as `sel`'s properties are over the property verbs. Luau's own `table`
//! library keeps its name. A run reads a table once and then works on its own
//! copy; the writes are gathered and land at the run's end as one `set_table`
//! per table written, so a loop of writes is one edit and a run that fails
//! part-way writes no table.

use std::cell::RefCell;

use mastik_core::chart::Table as Cells;
use mastik_core::list::DataSetId;
use mlua::{IntoLua, Lua, Table, Value};
use serde_json::json;

use super::{State, raise, writable};

/// A table a run has touched: its cells as the run has written them, and
/// whether to write them back at its end.
pub(super) struct Working {
    pub(super) id: DataSetId,
    pub(super) cells: Cells,
    pub(super) dirty: bool,
}

/// The raw field a handle keeps its table's id in.
const ID: &str = "__table";

/// The table `text` names (its id or its name), raising the refusal
/// `find_table` gives.
fn find(lua: &Lua, state: &RefCell<State<'_>>, text: &str) -> mlua::Result<DataSetId> {
    let st = state.borrow();
    mastik_core::verbs::find_table(st.host.doc(), text)
        .map(|d| d.id)
        .map_err(|why| raise(lua, format!("No table “{text}”: {why}")))
}

/// Run `f` over table `id`'s working copy, read from the document on first use.
fn with_cells<R>(
    lua: &Lua,
    state: &RefCell<State<'_>>,
    id: DataSetId,
    f: impl FnOnce(&mut Working, &str) -> mlua::Result<R>,
) -> mlua::Result<R> {
    let mut st = state.borrow_mut();
    let name = match st.host.doc().data_set(id) {
        Some(d) => d.name.clone(),
        None => return Err(raise(lua, "This table was deleted")),
    };
    if !st.tables.iter().any(|w| w.id == id) {
        let cells = st.host.doc().data_set(id).map(|d| d.table.clone());
        st.tables.push(Working {
            id,
            cells: cells.unwrap_or_default(),
            dirty: false,
        });
    }
    let w = st.tables.iter_mut().find(|w| w.id == id);
    f(w.expect("pushed above"), &name)
}

/// Refuse a write to table `id` in a live read or while it is linked to a file.
fn editable(lua: &Lua, state: &RefCell<State<'_>>, id: DataSetId) -> mlua::Result<()> {
    writable(lua, state)?;
    let st = state.borrow();
    if let Some(d) = st.host.doc().data_set(id)
        && let Some(link) = &d.source
    {
        let file = std::path::Path::new(&link.path)
            .file_name()
            .map_or_else(|| link.path.clone(), |f| f.to_string_lossy().into_owned());
        return Err(raise(
            lua,
            format!("{} is linked to {file}: unlink it first", d.name),
        ));
    }
    Ok(())
}

/// A cell as a script writes it: text as it is, a number as Luau prints it,
/// a boolean as `true` or `false`, nil as empty.
fn cell_text(lua: &Lua, v: &Value) -> mlua::Result<String> {
    match v {
        Value::Nil => Ok(String::new()),
        Value::String(s) => Ok(s.to_str()?.to_owned()),
        Value::Integer(n) => Ok(n.to_string()),
        Value::Number(n) => Ok(crate::format_number(*n)),
        Value::Boolean(b) => Ok(b.to_string()),
        other => Err(raise(
            lua,
            format!(
                "A cell is text, a number or a boolean, not {}",
                other.type_name()
            ),
        )),
    }
}

/// Row `i` (from 1) as an index into `w`, or the refusal naming the count.
fn row_at(lua: &Lua, w: &Working, name: &str, i: i64) -> mlua::Result<usize> {
    let n = w.cells.rows.len();
    match usize::try_from(i) {
        Ok(i) if (1..=n).contains(&i) => Ok(i - 1),
        _ => Err(raise(
            lua,
            format!(
                "{name} has {n} {}, not {i}",
                if n == 1 { "row" } else { "rows" }
            ),
        )),
    }
}

/// Write `values` (column name to cell) over `row`, refusing a column `w`
/// lacks.
fn write_row(
    lua: &Lua,
    w: &mut Working,
    name: &str,
    row: usize,
    values: &Table,
) -> mlua::Result<()> {
    for pair in values.pairs::<String, Value>() {
        let (column, v) = pair?;
        let Some(c) = w.cells.columns.iter().position(|c| c.name == column) else {
            return Err(raise(lua, format!("{name} has no column “{column}”")));
        };
        w.cells.rows[row][c] = cell_text(lua, &v)?;
    }
    w.dirty = true;
    Ok(())
}

/// The handle on table `id`: a table carrying the id, wearing `meta`.
fn handle(lua: &Lua, meta: &Table, id: DataSetId) -> mlua::Result<Table> {
    let t = lua.create_table()?;
    t.raw_set(ID, id.0.to_wire())?;
    t.set_metatable(Some(meta.clone()))?;
    Ok(t)
}

/// The table a handle names.
fn id_of(lua: &Lua, t: &Table) -> mlua::Result<DataSetId> {
    let wire: String = t.raw_get(ID)?;
    mastik_core::NodeId::from_wire(&wire)
        .map(DataSetId)
        .ok_or_else(|| raise(lua, "Not a table"))
}

/// The `tables` global: indexed by a table's name or id for its handle, and
/// `tables.new(name, columns)` making one. Built inside the run's scope, its
/// callbacks holding `state`.
pub(super) fn install<'s>(
    lua: &Lua,
    scope: &'s mlua::Scope<'s, '_>,
    state: &'s RefCell<State<'_>>,
) -> mlua::Result<Table> {
    let meta = lua.create_table()?;
    let add = scope.create_function(move |lua, (t, values): (Table, Table)| {
        let id = id_of(lua, &t)?;
        editable(lua, state, id)?;
        with_cells(lua, state, id, |w, name| {
            w.cells
                .rows
                .push(vec![String::new(); w.cells.columns.len()]);
            let at = w.cells.rows.len() - 1;
            write_row(lua, w, name, at, &values)?;
            Ok(at + 1)
        })
    })?;
    let set = scope.create_function(move |lua, (t, i, values): (Table, i64, Table)| {
        let id = id_of(lua, &t)?;
        editable(lua, state, id)?;
        with_cells(lua, state, id, |w, name| {
            let at = row_at(lua, w, name, i)?;
            write_row(lua, w, name, at, &values)
        })
    })?;
    let remove = scope.create_function(move |lua, (t, i): (Table, i64)| {
        let id = id_of(lua, &t)?;
        editable(lua, state, id)?;
        with_cells(lua, state, id, |w, name| {
            let at = row_at(lua, w, name, i)?;
            w.cells.rows.remove(at);
            w.dirty = true;
            Ok(())
        })
    })?;
    let column = scope.create_function(move |lua, (t, column): (Table, String)| {
        let id = id_of(lua, &t)?;
        let cells = with_cells(lua, state, id, |w, name| {
            let Some(c) = w.cells.columns.iter().position(|c| c.name == column) else {
                return Err(raise(lua, format!("{name} has no column “{column}”")));
            };
            Ok(w.cells
                .rows
                .iter()
                .map(|r| r[c].clone())
                .collect::<Vec<_>>())
        })?;
        lua.create_sequence_from(cells)
    })?;
    let methods = [
        ("add", add),
        ("set", set),
        ("remove", remove),
        ("column", column),
    ];
    meta.raw_set(
        "__index",
        scope.create_function(move |lua, (t, key): (Table, Value)| {
            let id = id_of(lua, &t)?;
            match key {
                Value::Integer(i) => with_cells(lua, state, id, |w, name| {
                    let at = row_at(lua, w, name, crate::wide(i))?;
                    let row = lua.create_table()?;
                    for (c, cell) in w.cells.columns.iter().zip(&w.cells.rows[at]) {
                        row.raw_set(c.name.as_str(), cell.as_str())?;
                    }
                    Ok(Value::Table(row))
                }),
                Value::String(s) => match s.to_str()?.as_ref() {
                    "rows" => with_cells(lua, state, id, |w, _| w.cells.rows.len().into_lua(lua)),
                    "name" => with_cells(lua, state, id, |_, name| {
                        Ok(Value::String(lua.create_string(name)?))
                    }),
                    key => match methods.iter().find(|(n, _)| *n == key) {
                        Some((_, f)) => Ok(Value::Function(f.clone())),
                        None => Err(raise(
                            lua,
                            format!("A table has rows, name, a row by number, add, set, remove and column, not “{key}”"),
                        )),
                    },
                },
                _ => Err(raise(lua, "A table's rows are numbered from 1")),
            }
        })?,
    )?;
    meta.raw_set(
        "__len",
        scope.create_function(move |lua, t: Table| {
            let id = id_of(lua, &t)?;
            with_cells(lua, state, id, |w, _| Ok(w.cells.rows.len()))
        })?,
    )?;
    meta.raw_set(
        "__newindex",
        lua.create_function(|lua, _: (Table, Value, Value)| {
            Err::<(), _>(raise(lua, "Write a row with :set(row, { Column = value })"))
        })?,
    )?;
    meta.raw_set(
        "__tostring",
        scope.create_function(move |lua, t: Table| {
            let id = id_of(lua, &t)?;
            with_cells(lua, state, id, |w, name| {
                let n = w.cells.rows.len();
                Ok(format!(
                    "{name} ({n} {})",
                    if n == 1 { "row" } else { "rows" }
                ))
            })
        })?,
    )?;
    meta.raw_set("__metatable", "table")?;

    let root = lua.create_table()?;
    let made_meta = meta.clone();
    let new = scope.create_function(move |lua, (name, columns): (String, Vec<String>)| {
        writable(lua, state)?;
        let args = json!({ "name": name, "columns": columns, "rows": [] });
        let made = state
            .borrow_mut()
            .apply("add_table", &args)
            .map_err(|why| raise(lua, why))?
            .1;
        let id = made
            .and_then(|m| mastik_core::NodeId::from_wire(&m))
            .map(DataSetId)
            .ok_or_else(|| raise(lua, "The table was not made"))?;
        handle(lua, &made_meta, id)
    })?;
    let root_meta = lua.create_table()?;
    root_meta.raw_set(
        "__index",
        scope.create_function(move |lua, (_, key): (Table, String)| {
            if key == "new" {
                return Ok(Value::Function(new.clone()));
            }
            let id = find(lua, state, &key)?;
            Ok(Value::Table(handle(lua, &meta, id)?))
        })?,
    )?;
    root_meta.raw_set(
        "__newindex",
        lua.create_function(|lua, _: (Table, Value, Value)| {
            Err::<(), _>(raise(lua, "Make a table with tables.new(name, columns)"))
        })?,
    )?;
    root_meta.raw_set("__metatable", "tables")?;
    root.set_metatable(Some(root_meta))?;
    Ok(root)
}

/// Write each table the run changed back as one `set_table`, at the run's end.
pub(super) fn flush(lua: &Lua, state: &RefCell<State<'_>>) -> mlua::Result<()> {
    let written: Vec<Working> = {
        let mut st = state.borrow_mut();
        std::mem::take(&mut st.tables)
            .into_iter()
            .filter(|w| w.dirty)
            .collect()
    };
    for w in written {
        let columns: Vec<&str> = w.cells.columns.iter().map(|c| c.name.as_str()).collect();
        let args = json!({
            "table": w.id.0.to_wire(),
            "columns": columns,
            "rows": w.cells.rows,
        });
        state
            .borrow_mut()
            .apply("set_table", &args)
            .map_err(|why| raise(lua, why))?;
    }
    Ok(())
}
