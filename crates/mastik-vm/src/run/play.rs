//! `play`: what a script a playing prototype runs reaches of the play state,
//! through the host's [`crate::Play`]. `play.vars` reads and writes variables'
//! play values by name, `play.row` is the row the reaction fired in, and
//! `play.go` asks for a frame to be shown once the run ends. The run binds
//! `play` only when its host has play state ([`crate::Host::play`]).

use std::cell::RefCell;

use mlua::{Lua, Table, Value};

use super::{State, json_lua, raise, writable};

/// `play`'s members with a line on each, for completion after `play.`.
pub(crate) const MEMBERS: &[(&str, &str)] = &[
    (
        "vars",
        "Variables by name: a read gives the play value, a write sets one",
    ),
    (
        "row",
        "The row the reaction fired in, by column; nil outside a list",
    ),
    ("go", "`play.go(\"Frame\")` shows a frame once the run ends"),
];

/// Run `f` over the host's play state, raising when it has none.
fn with_play<R>(
    lua: &Lua,
    state: &RefCell<State<'_>>,
    f: impl FnOnce(&mut dyn crate::Play) -> Result<R, String>,
) -> mlua::Result<R> {
    let mut st = state.borrow_mut();
    let play = st.host.play().ok_or_else(|| {
        raise(
            lua,
            "`play` is there only in a script a playing prototype runs",
        )
    })?;
    f(play).map_err(|why| raise(lua, why))
}

/// A value a script writes to a variable, as [`crate::Play::set_var`] takes it.
fn var_json(lua: &Lua, v: &Value) -> mlua::Result<serde_json::Value> {
    Ok(match v {
        Value::Number(n) => serde_json::json!(n),
        Value::Integer(n) => serde_json::json!(crate::wide(*n)),
        Value::String(s) => serde_json::json!(s.to_str()?.to_owned()),
        Value::Boolean(b) => serde_json::json!(b),
        other => {
            return Err(raise(
                lua,
                format!(
                    "A variable holds a number, a string, true or false, or a colour like \"#ff7a1a\", not {}",
                    super::type_name(other)
                ),
            ));
        }
    })
}

/// The `play` global: `vars`, `row` and `go`, every other key raising. Built
/// inside the run's scope, its callbacks holding `state`.
pub(super) fn install<'s>(
    lua: &Lua,
    scope: &'s mlua::Scope<'s, '_>,
    state: &'s RefCell<State<'_>>,
) -> mlua::Result<Table> {
    let vars = lua.create_table()?;
    let vars_meta = lua.create_table()?;
    vars_meta.raw_set(
        "__index",
        scope.create_function(move |lua, (_, name): (Table, String)| {
            let v = with_play(lua, state, |p| p.var(&name))?;
            json_lua(lua, &v)
        })?,
    )?;
    vars_meta.raw_set(
        "__newindex",
        scope.create_function(move |lua, (_, name, v): (Table, String, Value)| {
            writable(lua, state)?;
            let v = var_json(lua, &v)?;
            with_play(lua, state, |p| p.set_var(&name, v))
        })?,
    )?;
    vars_meta.raw_set("__metatable", "play.vars")?;
    vars.set_metatable(Some(vars_meta))?;

    let go = scope.create_function(move |lua, frame: String| {
        writable(lua, state)?;
        with_play(lua, state, |p| p.go(&frame))
    })?;

    let root = lua.create_table()?;
    let root_meta = lua.create_table()?;
    root_meta.raw_set(
        "__index",
        scope.create_function(move |lua, (_, key): (Table, String)| match key.as_str() {
            "vars" => Ok(Value::Table(vars.clone())),
            "go" => Ok(Value::Function(go.clone())),
            "row" => {
                let Some(cells) = with_play(lua, state, |p| Ok(p.row()))? else {
                    return Ok(Value::Nil);
                };
                let row = lua.create_table()?;
                for (column, cell) in &cells {
                    row.raw_set(column.as_str(), cell.as_str())?;
                }
                let meta = lua.create_table()?;
                meta.raw_set(
                    "__index",
                    lua.create_function(|lua, (_, column): (Table, String)| {
                        Err::<Value, _>(raise(lua, format!("The row has no column “{column}”")))
                    })?,
                )?;
                row.set_metatable(Some(meta))?;
                row.set_readonly(true);
                Ok(Value::Table(row))
            }
            other => Err(raise(
                lua,
                format!("`play` has vars, row and go, not “{other}”"),
            )),
        })?,
    )?;
    root_meta.raw_set(
        "__newindex",
        lua.create_function(|lua, _: (Table, Value, Value)| {
            Err::<(), _>(raise(lua, "Write a variable with play.vars.Name = value"))
        })?,
    )?;
    root_meta.raw_set("__metatable", "play")?;
    root.set_metatable(Some(root_meta))?;
    Ok(root)
}
