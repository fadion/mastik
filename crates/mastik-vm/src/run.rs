//! The Luau runtime: one sandboxed state per evaluation, an environment whose
//! `__index` resolves names in `command-mode.md` §4's order, layer collections as tables
//! with one shared metatable, verbs bound by their registry names, and budgets.
//! Every callback reaches the host through one `RefCell<State>`, never borrowed
//! across a call back into Lua (`adjust`'s function form), which would re-enter it.

use std::cell::{Cell, RefCell};
use std::rc::Rc;
use std::time::{Duration, Instant};

use mastik_core::NodeId;
use mastik_core::verbs::{self, ParamKind};
use mlua::{Lua, MultiValue, Table, Value, VmState};
use serde_json::{Map, json};

use crate::props::{self, PropType, Val};
use crate::{ErrorKind, Host, Outcome, VmError, describe, format_number};

/// How long, how many interrupt steps and how much memory an evaluation may take.
#[derive(Clone, Copy, Debug)]
pub struct Budget {
    pub time: Duration,
    /// Interrupts, which Luau raises at every call and loop iteration.
    pub steps: u64,
    /// Bytes beyond what the fresh state holds.
    pub memory: usize,
}

/// A run's budget: a line run with Enter, or a script.
pub const RUN_BUDGET: Budget = Budget {
    time: Duration::from_secs(1),
    steps: 200_000_000,
    memory: 64 << 20,
};

/// A live read's budget: what the bar evaluates as the line is typed, every
/// keystroke, so small enough not to stall a frame.
pub const READ_BUDGET: Budget = Budget {
    time: Duration::from_millis(20),
    steps: 500_000,
    memory: 8 << 20,
};

/// Where the text being run came from, which decides what it may do and how a
/// mixed read's hint is spelled.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Mode {
    /// A script: no sugar, so a hint names `sel:adjust`.
    Script,
    /// A bar line run with Enter: a hint names `.x += 20`.
    Bar,
    /// A bar line read as it is typed: any write is refused.
    Read,
}

/// The registry key of the metatable every collection shares.
const META: &str = "mastik.meta";
/// The registry key of the weak table from a collection to its layers.
const IDS: &str = "mastik.ids";

/// The permitted standard library, read from the sandboxed globals.
pub(crate) const STDLIB: &[&str] = &[
    "math", "string", "table", "ipairs", "pairs", "select", "tonumber", "tostring", "type",
    "assert", "error",
];

/// The members of the permitted libraries, for highlighting and completion
/// after `math.`, `string.` and `table.`; `math` without its random numbers. A
/// test holds each one to the sandbox, so a name here is one a script can call.
pub(crate) const LIBRARY: &[(&str, &[&str])] = &[
    (
        "math",
        &[
            "abs", "acos", "asin", "atan", "atan2", "ceil", "clamp", "cos", "cosh", "deg", "exp",
            "floor", "fmod", "frexp", "huge", "ldexp", "log", "log10", "max", "min", "modf",
            "noise", "pi", "pow", "rad", "round", "sign", "sin", "sinh", "sqrt", "tan", "tanh",
        ],
    ),
    (
        "string",
        &[
            "byte", "char", "find", "format", "gmatch", "gsub", "len", "lower", "match", "rep",
            "reverse", "split", "sub", "upper",
        ],
    ),
    (
        "table",
        &[
            "clear", "clone", "concat", "create", "find", "freeze", "insert", "isfrozen", "maxn",
            "move", "pack", "remove", "sort", "unpack",
        ],
    ),
];

/// The members `LIBRARY` lists for library `lib`, none for any other name.
pub(crate) fn library(lib: &str) -> &'static [&'static str] {
    LIBRARY
        .iter()
        .find(|(name, _)| *name == lib)
        .map_or(&[], |(_, members)| members)
}

/// One line on a library member or a permitted global: its parameters (`None`
/// for a value such as `math.pi`), what it answers and what it does.
#[derive(Clone, Copy, Debug)]
pub(crate) struct LibDoc {
    /// `math.floor`, or a global's bare name.
    pub path: &'static str,
    pub params: Option<&'static str>,
    pub returns: &'static str,
    pub doc: &'static str,
}

/// Shorthand for a [`LIB_DOCS`] row.
const fn lib(
    path: &'static str,
    params: Option<&'static str>,
    returns: &'static str,
    doc: &'static str,
) -> LibDoc {
    LibDoc {
        path,
        params,
        returns,
        doc,
    }
}

/// A line for every [`LIBRARY`] member and every [`STDLIB`] function, which
/// completion and hover help show; a test holds the two lists to each other.
pub(crate) const LIB_DOCS: &[LibDoc] = &[
    lib(
        "math.abs",
        Some("x: number"),
        "number",
        "The size of x, its sign dropped",
    ),
    lib(
        "math.acos",
        Some("x: number"),
        "number",
        "The angle whose cosine is x, in radians",
    ),
    lib(
        "math.asin",
        Some("x: number"),
        "number",
        "The angle whose sine is x, in radians",
    ),
    lib(
        "math.atan",
        Some("x: number"),
        "number",
        "The angle whose tangent is x, in radians",
    ),
    lib(
        "math.atan2",
        Some("y: number, x: number"),
        "number",
        "The angle of the point (x, y) from the x axis, in radians",
    ),
    lib(
        "math.ceil",
        Some("x: number"),
        "number",
        "The smallest whole number not below x",
    ),
    lib(
        "math.clamp",
        Some("x: number, min: number, max: number"),
        "number",
        "x held between min and max",
    ),
    lib(
        "math.cos",
        Some("x: number"),
        "number",
        "The cosine of x radians",
    ),
    lib(
        "math.cosh",
        Some("x: number"),
        "number",
        "The hyperbolic cosine of x",
    ),
    lib(
        "math.deg",
        Some("x: number"),
        "number",
        "x radians in degrees",
    ),
    lib(
        "math.exp",
        Some("x: number"),
        "number",
        "e raised to the power x",
    ),
    lib(
        "math.floor",
        Some("x: number"),
        "number",
        "The largest whole number not above x",
    ),
    lib(
        "math.fmod",
        Some("x: number, y: number"),
        "number",
        "The remainder of x / y, rounded toward zero",
    ),
    lib(
        "math.frexp",
        Some("x: number"),
        "(number, number)",
        "m and e such that x = m × 2^e, m from 0.5 up to 1",
    ),
    lib(
        "math.huge",
        None,
        "number",
        "Infinity, larger than any number",
    ),
    lib(
        "math.ldexp",
        Some("m: number, e: number"),
        "number",
        "m × 2^e",
    ),
    lib(
        "math.log",
        Some("x: number, base: number?"),
        "number",
        "The logarithm of x, natural unless base is given",
    ),
    lib(
        "math.log10",
        Some("x: number"),
        "number",
        "The base-10 logarithm of x",
    ),
    lib(
        "math.max",
        Some("...: number"),
        "number",
        "The largest of the numbers given",
    ),
    lib(
        "math.min",
        Some("...: number"),
        "number",
        "The smallest of the numbers given",
    ),
    lib(
        "math.modf",
        Some("x: number"),
        "(number, number)",
        "The whole part of x and its fraction",
    ),
    lib(
        "math.noise",
        Some("x: number, y: number?, z: number?"),
        "number",
        "Perlin noise at the point, from -1 to 1",
    ),
    lib(
        "math.pi",
        None,
        "number",
        "π, a circle's circumference over its diameter",
    ),
    lib(
        "math.pow",
        Some("x: number, y: number"),
        "number",
        "x raised to the power y",
    ),
    lib(
        "math.rad",
        Some("x: number"),
        "number",
        "x degrees in radians",
    ),
    lib(
        "math.round",
        Some("x: number"),
        "number",
        "x to the nearest whole number, halves away from zero",
    ),
    lib(
        "math.sign",
        Some("x: number"),
        "number",
        "-1, 0 or 1, by the sign of x",
    ),
    lib(
        "math.sin",
        Some("x: number"),
        "number",
        "The sine of x radians",
    ),
    lib(
        "math.sinh",
        Some("x: number"),
        "number",
        "The hyperbolic sine of x",
    ),
    lib(
        "math.sqrt",
        Some("x: number"),
        "number",
        "The square root of x",
    ),
    lib(
        "math.tan",
        Some("x: number"),
        "number",
        "The tangent of x radians",
    ),
    lib(
        "math.tanh",
        Some("x: number"),
        "number",
        "The hyperbolic tangent of x",
    ),
    lib(
        "string.byte",
        Some("s: string, i: number?, j: number?"),
        "...number",
        "The character codes of s from i to j",
    ),
    lib(
        "string.char",
        Some("...: number"),
        "string",
        "The text of these character codes",
    ),
    lib(
        "string.find",
        Some("s: string, pattern: string, init: number?, plain: boolean?"),
        "(number?, number?)",
        "Where pattern first matches in s, searching from init",
    ),
    lib(
        "string.format",
        Some("format: string, ...: any"),
        "string",
        "The values written into format's % slots",
    ),
    lib(
        "string.gmatch",
        Some("s: string, pattern: string"),
        "() -> string",
        "Each match of pattern in s, for a for loop",
    ),
    lib(
        "string.gsub",
        Some("s: string, pattern: string, repl: any, n: number?"),
        "(string, number)",
        "s with each match of pattern replaced, and how many were",
    ),
    lib(
        "string.len",
        Some("s: string"),
        "number",
        "The length of s in bytes",
    ),
    lib(
        "string.lower",
        Some("s: string"),
        "string",
        "s in lower case",
    ),
    lib(
        "string.match",
        Some("s: string, pattern: string, init: number?"),
        "string?",
        "The first match of pattern in s, or its captures",
    ),
    lib(
        "string.rep",
        Some("s: string, n: number, sep: string?"),
        "string",
        "s written n times, joined by sep",
    ),
    lib("string.reverse", Some("s: string"), "string", "s backwards"),
    lib(
        "string.split",
        Some("s: string, sep: string?"),
        "{string}",
        "s cut at each sep, a comma unless given",
    ),
    lib(
        "string.sub",
        Some("s: string, i: number, j: number?"),
        "string",
        "The part of s from i to j",
    ),
    lib(
        "string.upper",
        Some("s: string"),
        "string",
        "s in upper case",
    ),
    lib(
        "table.clear",
        Some("t: table"),
        "()",
        "Remove every entry of t",
    ),
    lib(
        "table.clone",
        Some("t: table"),
        "table",
        "A shallow copy of t",
    ),
    lib(
        "table.concat",
        Some("t: table, sep: string?, i: number?, j: number?"),
        "string",
        "The list's strings and numbers joined by sep",
    ),
    lib(
        "table.create",
        Some("n: number, value: any?"),
        "table",
        "A list of n entries, each value",
    ),
    lib(
        "table.find",
        Some("t: table, value: any, init: number?"),
        "number?",
        "The index of value in the list, or nil",
    ),
    lib(
        "table.freeze",
        Some("t: table"),
        "table",
        "Make t read-only, and answer it",
    ),
    lib(
        "table.insert",
        Some("t: table, pos: number?, value: any"),
        "()",
        "Add value to the list at pos, or at its end",
    ),
    lib(
        "table.isfrozen",
        Some("t: table"),
        "boolean",
        "Whether t is read-only",
    ),
    lib(
        "table.maxn",
        Some("t: table"),
        "number",
        "The largest number t is indexed by",
    ),
    lib(
        "table.move",
        Some("a: table, f: number, e: number, t: number, b: table?"),
        "table",
        "Copy a's entries f to e into b, or a, from index t",
    ),
    lib(
        "table.pack",
        Some("...: any"),
        "table",
        "The values given as a list, with their count in n",
    ),
    lib(
        "table.remove",
        Some("t: table, pos: number?"),
        "any",
        "Take out the entry at pos, or the last, and answer it",
    ),
    lib(
        "table.sort",
        Some("t: table, comp: ((any, any) -> boolean)?"),
        "()",
        "Sort the list in place, by comp when given",
    ),
    lib(
        "table.unpack",
        Some("t: table, i: number?, j: number?"),
        "...any",
        "The list's entries from i to j as separate values",
    ),
    lib(
        "ipairs",
        Some("t: table"),
        "iterator",
        "Each index and value of the list in order, for a for loop",
    ),
    lib(
        "pairs",
        Some("t: table"),
        "iterator",
        "Each key and value of t in no set order, for a for loop",
    ),
    lib(
        "select",
        Some("i: number | \"#\", ...: any"),
        "...any",
        "The values from the i-th on, or their count for \"#\"",
    ),
    lib(
        "tonumber",
        Some("v: any, base: number?"),
        "number?",
        "v as a number, or nil when it is not one",
    ),
    lib("tostring", Some("v: any"), "string", "v as text"),
    lib(
        "type",
        Some("v: any"),
        "string",
        "The name of v's type: \"number\", \"string\", \"table\"…",
    ),
    lib(
        "assert",
        Some("v: any, message: string?"),
        "any",
        "v when it is truthy, else stop the run with message",
    ),
    lib(
        "error",
        Some("message: any, level: number?"),
        "never",
        "Stop the run with message",
    ),
];

/// The line on `path`: `math.floor`, or a global's name.
pub(crate) fn lib_doc(path: &str) -> Option<&'static LibDoc> {
    LIB_DOCS.iter().find(|d| d.path == path)
}

/// Names the language gives besides the verbs, labels and layers: completion
/// and the unknown-name suggestions read them.
pub(crate) const NAMES: &[(&str, &str)] = &[
    ("sel", "The selection, always a collection"),
    ("selection", "The selection, always a collection"),
    ("parent", "The selection's shared parent"),
    ("prev", "The sibling below the one selected layer"),
    ("next", "The sibling above the one selected layer"),
    ("children", "The layers inside the selection"),
    ("param", "The script's inputs, by name"),
    ("percent", "`percent(50)` is the size `50%`"),
    ("print", "Show values in the output"),
];

/// The most a run may print, in bytes, each line counted one more for its own
/// keeping: the output is held in the host's memory, outside the Lua memory limit.
const PRINT_CAP: usize = 1 << 20;

/// The most lines a run may print: each is a row the editor's output keeps.
const PRINT_LINES: usize = 10_000;

/// Why a run stopped printing.
const PRINTED_TOO_MUCH: &str = "Stopped after printing more than 10,000 lines or 1 MB";

/// What the host and the run accumulate.
struct State<'h> {
    host: &'h mut dyn Host,
    mode: Mode,
    printed: Vec<String>,
    /// The bytes in `printed`, held to [`PRINT_CAP`].
    printed_bytes: usize,
    created: Vec<NodeId>,
    selection: Option<Vec<NodeId>>,
    /// Set by each host call, which may be slow: the interrupt then reads the
    /// clock at once rather than at its next 256th step.
    hosted: Rc<Cell<bool>>,
}

impl State<'_> {
    /// Commit `tx` unless it is empty.
    fn commit(&mut self, tx: mastik_core::Transaction) -> Result<(), String> {
        if tx.0.is_empty() {
            return Ok(());
        }
        self.hosted.set(true);
        self.host.commit(tx)
    }

    /// Keep `line` as printed output; `false` once the run has printed too much.
    fn say(&mut self, line: String) -> bool {
        // An empty line costs one, so no run prints without end.
        self.printed_bytes += line.len() + 1;
        if self.printed_bytes > PRINT_CAP || self.printed.len() >= PRINT_LINES {
            return false;
        }
        self.printed.push(line);
        true
    }

    /// Answer and commit verb `name`; what it created and the id of what else it made.
    fn apply(
        &mut self,
        name: &str,
        args: serde_json::Value,
    ) -> Result<(Vec<NodeId>, Option<String>), String> {
        self.hosted.set(true);
        let done = self.host.verb(name, &args)?;
        self.commit(done.tx)?;
        if let Some(said) = done.said
            && !self.say(said)
        {
            return Err(PRINTED_TOO_MUCH.into());
        }
        self.created.extend(&done.created);
        Ok((done.created, done.made))
    }
}

/// An error a callback raised, carried through Lua back to [`run`].
#[derive(Debug)]
struct Raised {
    kind: ErrorKind,
    message: String,
    line: Option<u32>,
    /// The identifier at fault, to place the error's span on.
    ident: Option<String>,
    /// What Tab replaces `ident` with.
    fix: Option<String>,
}

impl std::fmt::Display for Raised {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.message)
    }
}

impl std::error::Error for Raised {}

/// The line the innermost function of the script's own chunk is running; a
/// frame of mlua's (a bound verb's wrapper) has lines of its own and is skipped.
fn current_line(lua: &Lua) -> Option<u32> {
    (0..32)
        .find_map(|level| {
            lua.inspect_stack(level, |d| {
                let ours = d.source().short_src.as_deref() == Some("script");
                ours.then(|| d.current_line()).flatten()
            })
            .flatten()
        })
        .and_then(|l| u32::try_from(l).ok())
}

/// An error naming `message`, at the line running.
fn raise(lua: &Lua, message: impl Into<String>) -> mlua::Error {
    raise_at(lua, message, None, None)
}

/// [`raise`], placed on `ident` and offering `fix` for it.
fn raise_at(
    lua: &Lua,
    message: impl Into<String>,
    ident: Option<&str>,
    fix: Option<String>,
) -> mlua::Error {
    mlua::Error::external(Raised {
        kind: ErrorKind::Error,
        message: message.into(),
        line: current_line(lua),
        ident: ident.map(str::to_owned),
        fix,
    })
}

/// A budget's time as the error says it: "1 s", "20 ms".
fn duration(d: Duration) -> String {
    if d >= Duration::from_secs(1) {
        format!("{} s", format_number(d.as_secs_f64()))
    } else {
        format!("{} ms", d.as_millis())
    }
}

/// A layer in a percentage of its container, `percent(50)`.
struct Percent(f64);

/// Run `code` against the host, `original` being the text the caller passed
/// (for placing an error), as an expression first when `expression` is set.
pub(crate) fn run(
    host: &mut dyn Host,
    env: &crate::Env<'_>,
    code: &str,
    original: &str,
    mode: Mode,
    expression: bool,
) -> Result<Outcome, VmError> {
    let budget = if mode == Mode::Read {
        READ_BUDGET
    } else {
        RUN_BUDGET
    };
    let lua = Lua::new();
    let fail = |e: mlua::Error| convert(e, original, budget);
    lua.sandbox(true).map_err(fail)?;
    let steps = Rc::new(Cell::new(0u64));
    let hosted = Rc::new(Cell::new(false));
    let start = Instant::now();
    let after_host = hosted.clone();
    lua.set_interrupt(move |_| {
        let n = steps.get() + 1;
        steps.set(n);
        let clock = n.is_multiple_of(256) || after_host.replace(false);
        let over = if n > budget.steps {
            Some(format!("Stopped after {} steps", budget.steps))
        } else if clock && start.elapsed() > budget.time {
            Some(format!("Stopped after {}", duration(budget.time)))
        } else {
            None
        };
        match over {
            Some(message) => Err(mlua::Error::external(Raised {
                kind: ErrorKind::Budget,
                message,
                line: None,
                ident: None,
                fix: None,
            })),
            None => Ok(VmState::Continue),
        }
    });
    lua.set_memory_limit(lua.used_memory() + budget.memory)
        .map_err(fail)?;
    let state = RefCell::new(State {
        host,
        mode,
        printed: Vec::new(),
        printed_bytes: 0,
        created: Vec::new(),
        selection: None,
        hosted,
    });
    let outer = &state;
    let value = lua
        .scope(|scope| {
            // Copied into the body so each `move` callback holds the reference itself.
            let state: &RefCell<State<'_>> = outer;
            let weak = lua.create_table()?;
            weak.raw_set("__mode", "k")?;
            let ids = lua.create_table()?;
            ids.set_metatable(Some(weak))?;
            lua.set_named_registry_value(IDS, ids)?;
            let meta = lua.create_table()?;
            lua.set_named_registry_value(META, &meta)?;

            let shared = scope.create_function(move |lua, (t, name): (Table, String)| {
                let ids = ids_of(lua, &t)?;
                read_shared(lua, state, &ids, &name, false)
            })?;
            let adjust =
                scope.create_function(move |lua, (t, name, by): (Table, String, Value)| {
                    adjust(lua, state, &t, &name, by)
                })?;
            let select = scope.create_function(move |lua, t: Table| {
                let ids = ids_of(lua, &t)?;
                state.borrow_mut().selection = Some(ids);
                Ok(())
            })?;
            let methods = [("shared", shared), ("adjust", adjust), ("select", select)];
            meta.raw_set(
                "__index",
                scope.create_function(move |lua, (t, key): (Table, Value)| {
                    let Value::String(key) = key else {
                        return Ok(Value::Nil);
                    };
                    let key = key.to_str()?.to_owned();
                    if let Some((_, f)) = methods.iter().find(|(n, _)| *n == key) {
                        return Ok(Value::Function(f.clone()));
                    }
                    let ids = ids_of(lua, &t)?;
                    read_shared(lua, state, &ids, &key, true)
                })?,
            )?;
            meta.raw_set(
                "__newindex",
                scope.create_function(move |lua, (t, key, v): (Table, Value, Value)| {
                    let Value::String(key) = key else {
                        return Err(raise(lua, "A collection's layers cannot be replaced"));
                    };
                    let ids = ids_of(lua, &t)?;
                    write(lua, state, &ids, &key.to_str()?, v)
                })?,
            )?;
            meta.raw_set(
                "__tostring",
                scope.create_function(move |lua, t: Table| {
                    let ids = ids_of(lua, &t)?;
                    Ok(describe(state.borrow().host.doc(), &ids))
                })?,
            )?;
            meta.raw_set(
                "__eq",
                lua.create_function(|lua, (a, b): (Table, Table)| {
                    Ok(ids_of(lua, &a)? == ids_of(lua, &b)?)
                })?,
            )?;
            meta.raw_set("__metatable", "layers")?;

            let call_verb = scope.create_function(move |lua, args: MultiValue| {
                let mut args = args.into_iter();
                let name: String = match args.next() {
                    Some(Value::String(s)) => s.to_str()?.to_owned(),
                    _ => return Err(raise(lua, "a verb needs its name")),
                };
                call_verb(lua, state, env, &name, args.collect())
            })?;
            let print = scope.create_function(move |lua, args: MultiValue| {
                let line: Vec<String> = args.iter().map(|v| printed(lua, state, v)).collect();
                if state.borrow_mut().say(line.join("\t")) {
                    return Ok(());
                }
                Err(mlua::Error::external(Raised {
                    kind: ErrorKind::Budget,
                    message: PRINTED_TOO_MUCH.into(),
                    line: current_line(lua),
                    ident: None,
                    fix: None,
                }))
            })?;
            let percent = lua.create_function(|lua, n: f64| lua.create_any_userdata(Percent(n)))?;
            let params = params_table(&lua, scope, env)?;

            let cache = lua.create_table()?;
            let env_meta = lua.create_table()?;
            let st = state;
            env_meta.raw_set(
                "__index",
                scope.create_function(move |lua, (_, name): (Table, String)| {
                    if let Some(v) = cache.raw_get::<Option<Value>>(name.as_str())? {
                        return Ok(v);
                    }
                    let (v, keep) =
                        resolve(lua, st, env, &name, &call_verb, &print, &percent, &params)?;
                    if keep {
                        cache.raw_set(name.as_str(), v.clone())?;
                    }
                    Ok(v)
                })?,
            )?;
            env_meta.raw_set(
                "__newindex",
                lua.create_function(|lua, (_, name, _): (Table, String, Value)| {
                    let message = if crate::lex::KEYWORDS.contains(&name.as_str()) {
                        format!("`{name}` is a keyword")
                    } else {
                        format!("No global “{name}” — write `local {name} = …`")
                    };
                    Err::<(), _>(raise_at(lua, message, Some(&name), None))
                })?,
            )?;
            let env_table = lua.create_table()?;
            env_table.set_metatable(Some(env_meta))?;

            let function = if expression {
                lua.load(format!("return {code}"))
                    .set_name("=script")
                    .set_environment(env_table.clone())
                    .into_function()
                    .ok()
            } else {
                None
            };
            let function = match function {
                Some(f) => f,
                None => lua
                    .load(code)
                    .set_name("=script")
                    .set_environment(env_table)
                    .into_function()?,
            };
            let returned: MultiValue = function.call(())?;
            match returned.into_iter().next() {
                None => Ok(None),
                Some(v) => shown(&lua, state, &v),
            }
        })
        .map_err(fail)?;
    let state = state.into_inner();
    Ok(Outcome {
        printed: state.printed,
        value,
        created: state.created,
        selection: state.selection,
    })
}

/// The `param` table: the script's inputs, an unknown one raising.
fn params_table<'s>(
    lua: &Lua,
    scope: &'s mlua::Scope<'s, '_>,
    env: &'s crate::Env<'_>,
) -> mlua::Result<Table> {
    let t = lua.create_table()?;
    for (k, v) in &env.params {
        t.raw_set(k.as_str(), json_lua(lua, v)?)?;
    }
    let meta = lua.create_table()?;
    meta.raw_set(
        "__index",
        scope.create_function(move |lua, (_, name): (Table, String)| {
            let known: Vec<&str> = env.params.keys().map(String::as_str).collect();
            let near = nearest(&name, &known);
            let message = match &near {
                Some(n) => format!("No input “{name}” — did you mean {n}?"),
                None => format!("No input “{name}”"),
            };
            Err::<Value, _>(raise_at(lua, message, Some(&name), near))
        })?,
    )?;
    t.set_metatable(Some(meta))?;
    t.set_readonly(true);
    Ok(t)
}

/// A JSON value as Lua: objects and arrays as tables.
fn json_lua(lua: &Lua, v: &serde_json::Value) -> mlua::Result<Value> {
    Ok(match v {
        serde_json::Value::Null => Value::Nil,
        serde_json::Value::Bool(b) => Value::Boolean(*b),
        serde_json::Value::Number(n) => Value::Number(n.as_f64().unwrap_or(0.0)),
        serde_json::Value::String(s) => Value::String(lua.create_string(s)?),
        serde_json::Value::Array(list) => {
            let t = lua.create_table()?;
            for (i, item) in list.iter().enumerate() {
                t.raw_set(i + 1, json_lua(lua, item)?)?;
            }
            Value::Table(t)
        }
        serde_json::Value::Object(map) => {
            let t = lua.create_table()?;
            for (k, item) in map {
                t.raw_set(k.as_str(), json_lua(lua, item)?)?;
            }
            Value::Table(t)
        }
    })
}

/// A global `name`, in `command-mode.md` §4's order, and whether it may be kept for the
/// rest of the run (a relative name or a layer may change as the run edits).
#[allow(clippy::too_many_arguments)]
fn resolve(
    lua: &Lua,
    state: &RefCell<State<'_>>,
    env: &crate::Env<'_>,
    name: &str,
    call_verb: &mlua::Function,
    print: &mlua::Function,
    percent: &mlua::Function,
    params: &Table,
) -> mlua::Result<(Value, bool)> {
    if matches!(name, "sel" | "selection") {
        return Ok((Value::Table(coll(lua, env.selection)?), true));
    }
    if matches!(name, "parent" | "prev" | "next" | "children") {
        return Ok((read_shared(lua, state, env.selection, name, true)?, false));
    }
    if let Some(id) = env.labels.get(name) {
        if !state.borrow().host.doc().contains(*id) {
            return Err(raise_at(
                lua,
                format!("No layer “{name}”: the layer it labelled was deleted"),
                Some(name),
                None,
            ));
        }
        return Ok((Value::Table(coll(lua, &[*id])?), false));
    }
    if verbs::find(name).is_some() {
        return Ok((Value::Function(call_verb.bind(name)?), true));
    }
    match name {
        "print" => return Ok((Value::Function(print.clone()), true)),
        "percent" => return Ok((Value::Function(percent.clone()), true)),
        "math" => return Ok((Value::Table(safe_math(lua)?), true)),
        "param" => return Ok((Value::Table(params.clone()), true)),
        n if STDLIB.contains(&n) => return Ok((lua.globals().get(n)?, true)),
        _ => {}
    }
    let found = {
        let st = state.borrow();
        let page = st.host.page();
        named(st.host.doc(), page, name)
    };
    match found.as_slice() {
        [one] => Ok((Value::Table(coll(lua, &[*one])?), false)),
        [] => {
            let candidates = global_names(state, env);
            let refs: Vec<&str> = candidates.iter().map(String::as_str).collect();
            let near = nearest(name, &refs);
            let message = match &near {
                Some(n) => format!("No name “{name}” — did you mean {n}?"),
                None => format!("No name “{name}”"),
            };
            Err(raise_at(lua, message, Some(name), near))
        }
        many => Err(raise_at(
            lua,
            format!(
                "{} layers on this page are called “{name}” — name one with its leap label",
                many.len()
            ),
            Some(name),
            None,
        )),
    }
}

/// Every global name a script could have meant: the language's, the labels,
/// the verbs, the standard library and the page's layers.
fn global_names(state: &RefCell<State<'_>>, env: &crate::Env<'_>) -> Vec<String> {
    let st = state.borrow();
    let mut out: Vec<String> = NAMES.iter().map(|(n, _)| (*n).to_owned()).collect();
    out.extend(env.labels.keys().cloned());
    out.extend(verbs::VERBS.iter().map(|v| v.name.to_owned()));
    out.extend(STDLIB.iter().map(|s| (*s).to_owned()));
    out.extend(crate::layer_names(st.host.doc(), st.host.page()));
    out
}

/// The layers under `page` called exactly `name`.
pub(crate) fn named(doc: &mastik_core::Document, page: NodeId, name: &str) -> Vec<NodeId> {
    let mut out = Vec::new();
    let mut stack: Vec<NodeId> = doc
        .get(page)
        .map(|p| p.children().to_vec())
        .unwrap_or_default();
    while let Some(id) = stack.pop() {
        let Some(node) = doc.get(id) else { continue };
        if node.name() == name {
            out.push(id);
        }
        stack.extend(node.children());
    }
    out
}

/// `math` without its random numbers: a run is deterministic.
fn safe_math(lua: &Lua) -> mlua::Result<Table> {
    let math: Table = lua.globals().get("math")?;
    let safe = lua.create_table()?;
    for pair in math.pairs::<Value, Value>() {
        let (k, v) = pair?;
        if let Value::String(s) = &k
            && matches!(&*s.to_str()?, "random" | "randomseed")
        {
            continue;
        }
        safe.raw_set(k, v)?;
    }
    safe.set_readonly(true);
    Ok(safe)
}

/// The edit distance between `a` and `b`, in characters, ignoring case.
pub(crate) fn edits(a: &str, b: &str) -> usize {
    let a: Vec<char> = a.to_lowercase().chars().collect();
    let b: Vec<char> = b.to_lowercase().chars().collect();
    let mut row: Vec<usize> = (0..=b.len()).collect();
    for (i, ca) in a.iter().enumerate() {
        let mut diagonal = row[0];
        row[0] = i + 1;
        for (j, cb) in b.iter().enumerate() {
            let above = row[j + 1];
            row[j + 1] = (diagonal + usize::from(ca != cb))
                .min(row[j] + 1)
                .min(above + 1);
            diagonal = above;
        }
    }
    row[b.len()]
}

/// The name of `known` that `asked` most likely meant: within two edits, and
/// fewer than its own length.
pub(crate) fn nearest(asked: &str, known: &[&str]) -> Option<String> {
    known
        .iter()
        .filter(|k| **k != asked)
        .map(|k| (edits(asked, k), *k))
        .filter(|(d, _)| *d <= 2 && *d < asked.chars().count())
        .min_by_key(|(d, _)| *d)
        .map(|(_, k)| k.to_owned())
}

/// The property `asked` most likely meant: a guess listed for it, else the nearest.
pub(crate) fn nearest_property(asked: &str) -> Option<String> {
    if let Some((_, meant)) = props::GUESSES.iter().find(|(g, _)| *g == asked) {
        return Some((*meant).to_owned());
    }
    let names: Vec<&str> = props::PROPERTIES.iter().map(|p| p.name).collect();
    nearest(asked, &names)
}

/// The error for a property no layer has.
fn no_property(lua: &Lua, name: &str) -> mlua::Error {
    let near = nearest_property(name);
    let message = match &near {
        Some(n) => format!("No property “{name}” — did you mean {n}?"),
        None => format!("No property “{name}”"),
    };
    raise_at(lua, message, Some(name), near)
}

/// A collection of `ids`: a read-only table whose array part holds one-layer
/// collections, the one-layer collection holding itself.
pub(crate) fn coll(lua: &Lua, ids: &[NodeId]) -> mlua::Result<Table> {
    let meta: Table = lua.named_registry_value(META)?;
    let registry: Table = lua.named_registry_value(IDS)?;
    let t = lua.create_table_with_capacity(ids.len(), 0)?;
    if ids.len() == 1 {
        t.raw_set(1, &t)?;
    } else {
        for (i, id) in ids.iter().enumerate() {
            t.raw_set(i + 1, coll(lua, &[*id])?)?;
        }
    }
    t.set_metatable(Some(meta))?;
    registry.raw_set(&t, lua.create_any_userdata(ids.to_vec())?)?;
    t.set_readonly(true);
    Ok(t)
}

/// The layers of collection `t`, `None` for any other table.
fn layers_of(lua: &Lua, t: &Table) -> mlua::Result<Option<Vec<NodeId>>> {
    let registry: Table = lua.named_registry_value(IDS)?;
    match registry.raw_get::<Value>(t)? {
        Value::UserData(ud) => Ok(Some(ud.borrow::<Vec<NodeId>>()?.clone())),
        _ => Ok(None),
    }
}

/// The layers of collection `t`; raises for any other table.
fn ids_of(lua: &Lua, t: &Table) -> mlua::Result<Vec<NodeId>> {
    layers_of(lua, t)?.ok_or_else(|| raise(lua, "Not a collection of layers"))
}

/// A layer value as Lua.
fn val_lua(lua: &Lua, v: Val) -> mlua::Result<Value> {
    Ok(match v {
        Val::Nil | Val::Mixed(_) => Value::Nil,
        Val::Num(n) => Value::Number(n),
        Val::Str(s) => Value::String(lua.create_string(&s)?),
        Val::Bool(b) => Value::Boolean(b),
        Val::Layers(ids) => Value::Table(coll(lua, &ids)?),
    })
}

/// `prop` across `ids`: the value they share. `strict` raises where they differ
/// or there are none; otherwise those read as nil. `children` joins every
/// layer's; `prev` and `next` need one layer.
fn read_shared(
    lua: &Lua,
    state: &RefCell<State<'_>>,
    ids: &[NodeId],
    prop: &str,
    strict: bool,
) -> mlua::Result<Value> {
    let Some(property) = props::find(prop) else {
        return Err(no_property(lua, prop));
    };
    let st = state.borrow();
    let (doc, res) = (st.host.doc(), st.host.resolved());
    let read = |id| props::read(doc, res, id, prop).map_err(|why| raise(lua, why));
    if prop == "children" {
        let mut all = Vec::new();
        for id in ids {
            if let Val::Layers(kids) = read(*id)? {
                all.extend(kids);
            }
        }
        drop(st);
        return Ok(Value::Table(coll(lua, &all)?));
    }
    if matches!(prop, "prev" | "next") && ids.len() != 1 {
        if !strict {
            return Ok(Value::Nil);
        }
        return Err(raise(
            lua,
            format!(
                "`{prop}` is a sibling of one layer, and there are {} — sel[1].{prop} reads the first's",
                ids.len()
            ),
        ));
    }
    let Some((first, rest)) = ids.split_first() else {
        if !strict {
            return Ok(Value::Nil);
        }
        return Err(raise(
            lua,
            format!("No layers to read .{prop} of: nothing is selected"),
        ));
    };
    let mut values = vec![read(*first)?];
    for id in rest {
        let v = read(*id)?;
        if !values.iter().any(|seen| props::agree(prop, seen, &v)) {
            values.push(v);
        }
    }
    let single_mixed = match &values[0] {
        Val::Mixed(why) if values.len() == 1 => Some(why.clone()),
        _ => None,
    };
    if let Some(why) = single_mixed {
        if !strict {
            return Ok(Value::Nil);
        }
        let who = describe(doc, &ids[..1]);
        return Err(raise(lua, format!("“{who}” has no one {prop}: {why}")));
    }
    if values.len() == 1 {
        let v = values.remove(0);
        drop(st);
        return val_lua(lua, v);
    }
    if !strict {
        return Ok(Value::Nil);
    }
    let shown: Vec<String> = values.iter().take(4).map(|v| v.shown(doc)).collect();
    let more = if values.len() > 4 { ", …" } else { "" };
    let hint = match (property.ty, st.mode) {
        (PropType::Number, Mode::Script) => {
            format!("sel:adjust(\"{prop}\", 20) adds to each; sel[1].{prop} reads one")
        }
        (PropType::Number, _) => format!(".{prop} += 20 adds to each; sel[1].{prop} reads one"),
        _ => format!("sel[1].{prop} reads one; sel:shared(\"{prop}\") is nil when they differ"),
    };
    Err(raise(
        lua,
        format!(
            "The {} layers have {} {} ({}{more}) — {hint}",
            ids.len(),
            values.len(),
            property.plural,
            shown.join(", ")
        ),
    ))
}

/// Raise unless the run may change the document.
fn writable(lua: &Lua, state: &RefCell<State<'_>>) -> mlua::Result<()> {
    if state.borrow().mode == Mode::Read {
        return Err(raise(
            lua,
            "A live read changes nothing: press Enter to run the line",
        ));
    }
    Ok(())
}

/// The Lua type name of `v`, for a refusal.
fn type_name(v: &Value) -> &'static str {
    match v {
        Value::Table(_) => "a table",
        Value::String(_) => "a string",
        Value::Number(_) | Value::Integer(_) => "a number",
        Value::Boolean(_) => "a boolean",
        Value::Nil => "nil",
        Value::Function(_) => "a function",
        _ => "a value of another kind",
    }
}

/// `v` as a number, if it is one.
fn number(v: &Value) -> Option<f64> {
    match v {
        Value::Number(n) => Some(*n),
        #[allow(clippy::cast_precision_loss)]
        Value::Integer(i) => Some(*i as f64),
        _ => None,
    }
}

/// Write `v` to `prop` of every layer in `ids`, one layer after another so
/// each write measures the document the one before left.
fn write(
    lua: &Lua,
    state: &RefCell<State<'_>>,
    ids: &[NodeId],
    prop: &str,
    v: Value,
) -> mlua::Result<()> {
    let Some(property) = props::find(prop) else {
        return Err(no_property(lua, prop));
    };
    writable(lua, state)?;
    if !property.writable {
        let hint = if prop == "parent" {
            " — `reparent` moves a layer into another"
        } else {
            ""
        };
        return Err(raise(lua, format!("`.{prop}` is read-only{hint}")));
    }
    if ids.is_empty() {
        return Err(raise(
            lua,
            format!("No layers to set .{prop} on: nothing is selected"),
        ));
    }
    if let Value::UserData(ud) = &v
        && let Ok(p) = ud.borrow::<Percent>()
    {
        return write_percent(lua, state, ids, prop, p.0);
    }
    let fail = |why: String| raise(lua, why);
    let wrong = || {
        raise(
            lua,
            format!(
                "`.{prop}` is {}, not {}",
                article(property.ty),
                type_name(&v)
            ),
        )
    };
    let wires: Vec<serde_json::Value> = ids.iter().map(|id| json!(id.to_wire())).collect();
    let mut st = state.borrow_mut();
    match property.ty {
        PropType::Number => {
            let n = number(&v).ok_or_else(wrong)?;
            if !n.is_finite() {
                return Err(raise(lua, format!("`.{prop}` must be a finite number")));
            }
            match prop {
                "x" | "y" => {
                    for id in ids {
                        if let Some(why) = mastik_core::build::laid_out_refusal(st.host.doc(), *id)
                        {
                            return Err(fail(why));
                        }
                        let tx = {
                            let (doc, res) = (st.host.doc(), st.host.resolved());
                            let Some(b) = mastik_core::query::bounds(res, *id) else {
                                continue;
                            };
                            let delta = if prop == "x" {
                                mastik_core::kurbo::Vec2::new(n - b.x0, 0.0)
                            } else {
                                mastik_core::kurbo::Vec2::new(0.0, n - b.y0)
                            };
                            mastik_core::build::move_by_world(doc, res, &[*id], delta)
                                .map_err(|e| fail(e.to_string()))?
                        };
                        st.commit(tx).map_err(fail)?;
                    }
                }
                "width" | "height" => {
                    for id in ids {
                        let size = {
                            let (doc, res) = (st.host.doc(), st.host.resolved());
                            mastik_core::query::local_box(doc, res, *id).map(|b| b.size())
                        };
                        let Some(size) = size else {
                            return Err(fail(format!(
                                "“{}” has no box to size",
                                describe(st.host.doc(), &[*id])
                            )));
                        };
                        let (w, h) = if prop == "width" {
                            (n, size.height)
                        } else {
                            (size.width, n)
                        };
                        st.apply(
                            "set_size",
                            json!({ "node": id.to_wire(), "width": w, "height": h }),
                        )
                        .map_err(fail)?;
                    }
                }
                "rotation" => {
                    for id in ids {
                        let tx = {
                            let (doc, res) = (st.host.doc(), st.host.resolved());
                            mastik_core::edit::turned_to(doc, res, *id, n.to_radians())
                        };
                        if let Some(tx) = tx {
                            st.commit(tx).map_err(fail)?;
                        }
                    }
                }
                "opacity" => {
                    st.apply("set_opacity", json!({ "nodes": wires, "opacity": n }))
                        .map_err(fail)?;
                }
                "corner_radius" => {
                    for id in ids {
                        st.apply(
                            "set_geometry",
                            json!({ "node": id.to_wire(), "geometry": { "CornerRadius": n } }),
                        )
                        .map_err(fail)?;
                    }
                }
                _ => return Err(no_property(lua, prop)),
            }
        }
        PropType::Colour => {
            let paint = match &v {
                Value::Nil => json!([]),
                Value::String(s) => json!([s.to_str()?.to_owned()]),
                _ => return Err(wrong()),
            };
            if prop == "fill" {
                st.apply("set_fills", json!({ "nodes": wires, "fills": paint }))
                    .map_err(fail)?;
                // A text draws in its style's colour over its fills, so new fills clear it.
                let coloured: Vec<serde_json::Value> =
                    mastik_core::build::coloured_texts(st.host.doc(), ids)
                        .iter()
                        .map(|id| json!(id.to_wire()))
                        .collect();
                if !coloured.is_empty() {
                    st.apply(
                        "set_text_style",
                        json!({ "nodes": coloured, "style": { "color": null } }),
                    )
                    .map_err(fail)?;
                }
            } else {
                st.apply("set_strokes", json!({ "nodes": wires, "strokes": paint }))
                    .map_err(fail)?;
            }
        }
        PropType::Text => {
            let text = match &v {
                Value::String(s) => s.to_str()?.to_owned(),
                Value::Number(_) | Value::Integer(_) if prop == "name" || prop == "text" => {
                    format_number(number(&v).unwrap_or(0.0))
                }
                _ => return Err(wrong()),
            };
            match prop {
                "name" | "text" => {
                    let verb = if prop == "name" {
                        "set_name"
                    } else {
                        "set_text"
                    };
                    for id in ids {
                        st.apply(verb, json!({ "node": id.to_wire(), prop: text }))
                            .map_err(fail)?;
                    }
                }
                "blend" => {
                    st.apply("set_blend", json!({ "nodes": wires, "blend": text }))
                        .map_err(fail)?;
                }
                _ => return Err(no_property(lua, prop)),
            }
        }
        PropType::Bool => {
            let Value::Boolean(b) = v else {
                return Err(wrong());
            };
            let verb = if prop == "visible" {
                "set_visible"
            } else {
                "set_locked"
            };
            st.apply(verb, json!({ "nodes": wires, prop: b }))
                .map_err(fail)?;
        }
        PropType::Layers => return Err(raise(lua, format!("`.{prop}` is read-only"))),
    }
    Ok(())
}

/// "a number", "a colour": how a refusal names a property's type.
fn article(ty: PropType) -> &'static str {
    match ty {
        PropType::Number => "a number",
        PropType::Text => "a string",
        PropType::Bool => "true or false",
        PropType::Colour => "a colour like \"#ff7a1a\", or nil",
        PropType::Layers => "a collection",
    }
}

/// `width` or `height` as a percentage of the container, where the size field
/// offers `%`: a resizable layer its parent lays out.
fn write_percent(
    lua: &Lua,
    state: &RefCell<State<'_>>,
    ids: &[NodeId],
    prop: &str,
    p: f64,
) -> mlua::Result<()> {
    use mastik_core::NodeKind;
    use mastik_core::container::{Dimension, can_stretch};
    if !matches!(prop, "width" | "height") {
        return Err(raise(
            lua,
            format!("`.{prop}` takes no %: only a laid-out layer's width and height do"),
        ));
    }
    if !p.is_finite() {
        return Err(raise(lua, "A percentage must be a finite number"));
    }
    let mut st = state.borrow_mut();
    for id in ids {
        let item = {
            let doc = st.host.doc();
            let Some(node) = doc.get(*id) else {
                return Err(raise(
                    lua,
                    format!("No layer `{}`: it was deleted", id.to_wire()),
                ));
            };
            let resizable = match node.kind() {
                NodeKind::Group => node.display().is_some(),
                k => can_stretch(k),
            };
            if !(mastik_core::build::is_flex_item(doc, *id) && resizable) {
                return Err(raise(
                    lua,
                    format!(
                        "“{}” takes no % {prop}: % sizes a layer by the flex or grid container that lays it out",
                        describe(doc, &[*id])
                    ),
                ));
            }
            let mut item = *node.item();
            let d = Dimension::Percent(p.max(0.0));
            if prop == "width" {
                item.width = d;
            } else {
                item.height = d;
            }
            item
        };
        let item = serde_json::to_value(item).map_err(|e| raise(lua, e.to_string()))?;
        st.apply(
            "set_layout_item",
            json!({ "nodes": [id.to_wire()], "item": item }),
        )
        .map_err(|why| raise(lua, why))?;
    }
    Ok(())
}

/// `sel:adjust(name, by)`: each layer's own value of `name` plus `by`, or
/// mapped through `by` when it is a function.
fn adjust(
    lua: &Lua,
    state: &RefCell<State<'_>>,
    t: &Table,
    name: &str,
    by: Value,
) -> mlua::Result<()> {
    let ids = ids_of(lua, t)?;
    let Some(property) = props::find(name) else {
        return Err(no_property(lua, name));
    };
    if property.ty != PropType::Number {
        return Err(raise(
            lua,
            format!(
                "`adjust` changes a number, and `.{name}` is {}",
                article(property.ty)
            ),
        ));
    }
    writable(lua, state)?;
    if ids.is_empty() {
        return Err(raise(
            lua,
            format!("No layers to adjust .{name} on: nothing is selected"),
        ));
    }
    for id in ids {
        let now = {
            let st = state.borrow();
            props::read(st.host.doc(), st.host.resolved(), id, name)
                .map_err(|why| raise(lua, why))?
        };
        let Val::Num(now) = now else {
            let who = describe(state.borrow().host.doc(), &[id]);
            return Err(raise(lua, format!("“{who}” has no {name} to adjust")));
        };
        let next = match &by {
            Value::Function(f) => {
                let out: Value = f.call(now)?;
                number(&out).ok_or_else(|| {
                    raise(
                        lua,
                        format!(
                            "`adjust`'s function returned {}, not a number",
                            type_name(&out)
                        ),
                    )
                })?
            }
            v => {
                number(v).ok_or_else(|| {
                    raise(
                        lua,
                        format!(
                            "`adjust` takes a number or a function, not {}",
                            type_name(v)
                        ),
                    )
                })? + now
            }
        };
        write(lua, state, &[id], name, Value::Number(next))?;
    }
    Ok(())
}

/// Call verb `name` with Lua's arguments: a collection first fills the subject
/// (the first layer parameter), else the selection does; the rest fill the other
/// parameters in order; one plain table is the named arguments. Answers what it
/// created as a collection, the id of what else it made, or nil.
fn call_verb(
    lua: &Lua,
    state: &RefCell<State<'_>>,
    env: &crate::Env<'_>,
    name: &str,
    args: Vec<Value>,
) -> mlua::Result<Value> {
    if state.borrow().mode == Mode::Read {
        return Err(raise(
            lua,
            format!("A live read changes nothing: `{name}` runs when you press Enter"),
        ));
    }
    let verb = verbs::find(name).ok_or_else(|| raise(lua, format!("No verb “{name}”")))?;
    let subject = verb
        .params
        .iter()
        .position(|p| matches!(p.kind, ParamKind::Node | ParamKind::Nodes));
    let mut map = Map::new();
    let mut given: Option<Vec<NodeId>> = None;
    let named_form = match args.as_slice() {
        [Value::Table(t)] => layers_of(lua, t)?.is_none(),
        _ => false,
    };
    let fail = |why: String| raise(lua, format!("`{name}`: {why}"));
    if named_form {
        let Some(Value::Table(t)) = args.first() else {
            unreachable!("the named form is one table");
        };
        for pair in t.pairs::<String, Value>() {
            let (key, v) = pair?;
            let kind = verb.params.iter().find(|p| p.name == key).map(|p| p.kind);
            let json = to_json(lua, &v, kind, env, 0).map_err(fail)?;
            map.insert(key, json);
        }
    } else {
        let mut args = args.into_iter().peekable();
        if subject.is_some()
            && let Some(Value::Table(t)) = args.peek()
            && let Some(ids) = layers_of(lua, t)?
        {
            given = Some(ids);
            args.next();
        }
        let rest: Vec<Value> = args.collect();
        let params: Vec<_> = verb
            .params
            .iter()
            .enumerate()
            .filter(|(i, _)| Some(*i) != subject)
            .map(|(_, p)| p)
            .collect();
        if rest.len() > params.len() {
            let names: Vec<&str> = params.iter().map(|p| p.name).collect();
            return Err(fail(format!(
                "takes {} argument{} ({}), and {} were given",
                params.len(),
                if params.len() == 1 { "" } else { "s" },
                names.join(", "),
                rest.len()
            )));
        }
        for (p, v) in params.iter().zip(&rest) {
            if matches!(v, Value::Nil) {
                continue;
            }
            map.insert(
                p.name.to_owned(),
                to_json(lua, v, Some(p.kind), env, 0).map_err(fail)?,
            );
        }
    }
    // The subject, from the collection given or the selection.
    let mut runs: Vec<Map<String, serde_json::Value>> = vec![map.clone()];
    if let Some(i) = subject {
        let p = &verb.params[i];
        if !map.contains_key(p.name) {
            let ids = given.unwrap_or_else(|| env.selection.to_vec());
            if ids.is_empty() {
                if p.kind == ParamKind::Node && p.name == "parent" {
                    let page = state.borrow().host.page();
                    runs[0].insert(p.name.into(), json!(page.to_wire()));
                } else if p.required {
                    return Err(raise(
                        lua,
                        format!("Nothing is selected, and `{name}` needs {}", p.doc),
                    ));
                }
            } else if p.kind == ParamKind::Nodes {
                let wires: Vec<String> = ids.iter().map(|id| id.to_wire()).collect();
                runs[0].insert(p.name.into(), json!(wires));
            } else {
                // A verb of one layer runs once for each.
                runs = ids
                    .iter()
                    .map(|id| {
                        let mut m = map.clone();
                        m.insert(p.name.into(), json!(id.to_wire()));
                        m
                    })
                    .collect();
            }
        }
    }
    let mut created = Vec::new();
    let mut made = None;
    for args in runs {
        let (c, m) = state
            .borrow_mut()
            .apply(name, serde_json::Value::Object(args))
            .map_err(|why| raise(lua, why))?;
        created.extend(c);
        made = made.or(m);
    }
    // What the call made across every run is selected, not only the last run's.
    if !created.is_empty() {
        state.borrow_mut().selection = Some(created.clone());
    }
    if !created.is_empty() {
        return Ok(Value::Table(coll(lua, &created)?));
    }
    match made {
        Some(m) => Ok(Value::String(lua.create_string(&m)?)),
        None => Ok(Value::Nil),
    }
}

/// A Lua value as a verb's JSON argument of `kind`: a collection as its wire
/// forms, a leap label given for a layer as its layer, a table as a list or an object.
fn to_json(
    lua: &Lua,
    v: &Value,
    kind: Option<ParamKind>,
    env: &crate::Env<'_>,
    depth: usize,
) -> Result<serde_json::Value, String> {
    if depth > 32 {
        return Err("a table nests too deep (is it inside itself?)".into());
    }
    let lua_err = |e: mlua::Error| e.to_string();
    Ok(match v {
        Value::Nil => serde_json::Value::Null,
        Value::Boolean(b) => json!(b),
        Value::Integer(i) => json!(i),
        Value::Number(n) => {
            if !n.is_finite() {
                return Err(format!("{n} is not a finite number"));
            }
            if n.fract() == 0.0 && n.abs() < 9.0e15 {
                #[allow(clippy::cast_possible_truncation)]
                let whole = *n as i64;
                json!(whole)
            } else {
                json!(n)
            }
        }
        Value::String(s) => {
            let s = s.to_str().map_err(lua_err)?.to_owned();
            let layer = matches!(kind, Some(ParamKind::Node | ParamKind::Nodes));
            let s = match env.labels.get(&s) {
                Some(id) if layer => id.to_wire(),
                _ => s,
            };
            if kind == Some(ParamKind::Nodes) {
                json!([s])
            } else {
                json!(s)
            }
        }
        Value::Table(t) => {
            if let Some(ids) = layers_of(lua, t).map_err(lua_err)? {
                let wires: Vec<String> = ids.iter().map(|id| id.to_wire()).collect();
                return match (kind, wires.as_slice()) {
                    (Some(ParamKind::Node), [one]) => Ok(json!(one)),
                    (Some(ParamKind::Node), _) => Err(format!(
                        "takes one layer here, and the collection holds {}",
                        wires.len()
                    )),
                    (Some(ParamKind::Nodes) | None, _) => Ok(json!(wires)),
                    _ => Err("a collection of layers cannot be passed here".into()),
                };
            }
            let listy = matches!(kind, Some(ParamKind::Model(m)) if m.starts_with('['))
                || matches!(kind, Some(ParamKind::Nodes));
            if t.raw_len() > 0 || (listy && t.clone().pairs::<Value, Value>().next().is_none()) {
                let mut list = Vec::new();
                for item in t.clone().sequence_values::<Value>() {
                    let item = item.map_err(lua_err)?;
                    let inner = (kind == Some(ParamKind::Nodes)).then_some(ParamKind::Node);
                    list.push(to_json(lua, &item, inner, env, depth + 1)?);
                }
                serde_json::Value::Array(list)
            } else {
                let mut map = Map::new();
                for pair in t.clone().pairs::<Value, Value>() {
                    let (k, item) = pair.map_err(lua_err)?;
                    let key = match &k {
                        Value::String(s) => s.to_str().map_err(lua_err)?.to_owned(),
                        Value::Number(_) | Value::Integer(_) => {
                            format_number(number(&k).unwrap_or(0.0))
                        }
                        other => return Err(format!("a table key cannot be {}", type_name(other))),
                    };
                    map.insert(key, to_json(lua, &item, None, env, depth + 1)?);
                }
                serde_json::Value::Object(map)
            }
        }
        Value::UserData(ud) => match ud.borrow::<Percent>() {
            Ok(p) => json!(format!("{}%", format_number(p.0))),
            Err(_) => return Err("this value cannot be passed to a verb".into()),
        },
        other => return Err(format!("{} cannot be passed to a verb", type_name(other))),
    })
}

/// `v` as the bar shows a read's value: numbers with trimmed decimals, strings
/// quoted, a collection as "Card and 2 more"; nil as nothing.
fn shown(lua: &Lua, state: &RefCell<State<'_>>, v: &Value) -> mlua::Result<Option<String>> {
    Ok(Some(match v {
        Value::Nil => return Ok(None),
        Value::Boolean(b) => b.to_string(),
        Value::Number(_) | Value::Integer(_) => format_number(number(v).unwrap_or(0.0)),
        Value::String(s) => format!("\"{}\"", s.to_str()?),
        Value::Table(t) => match layers_of(lua, t)? {
            Some(ids) => describe(state.borrow().host.doc(), &ids),
            None => table_shown(lua, state, t)?,
        },
        Value::Function(_) => "function".into(),
        Value::UserData(ud) => match ud.borrow::<Percent>() {
            Ok(p) => format!("{}%", format_number(p.0)),
            Err(_) => "userdata".into(),
        },
        _ => "value".into(),
    }))
}

/// A plain table as `{1, 2, 3}`, its first few items only.
fn table_shown(lua: &Lua, state: &RefCell<State<'_>>, t: &Table) -> mlua::Result<String> {
    let n = t.raw_len();
    if n == 0 {
        return Ok(if t.clone().pairs::<Value, Value>().next().is_none() {
            "{}".into()
        } else {
            "{…}".into()
        });
    }
    let mut items = Vec::new();
    for item in t.clone().sequence_values::<Value>().take(5) {
        let item = item?;
        let s = match &item {
            Value::Table(_) => "{…}".to_owned(),
            other => shown(lua, state, other)?.unwrap_or_else(|| "nil".into()),
        };
        items.push(s);
    }
    if n > 5 {
        items.push("…".into());
    }
    Ok(format!("{{{}}}", items.join(", ")))
}

/// `v` as `print` writes it: strings bare, numbers in full.
fn printed(lua: &Lua, state: &RefCell<State<'_>>, v: &Value) -> String {
    match v {
        Value::Nil => "nil".into(),
        Value::String(s) => s.to_string_lossy(),
        Value::Number(n) => {
            if n.fract() == 0.0 && n.abs() < 1e15 {
                format_number(*n)
            } else {
                n.to_string()
            }
        }
        other => shown(lua, state, other)
            .ok()
            .flatten()
            .unwrap_or_else(|| "nil".into()),
    }
}

/// The `Raised` an error carries, through any callback wrapping.
fn raised(e: &mlua::Error) -> Option<&Raised> {
    match e {
        mlua::Error::CallbackError { cause, .. } => raised(cause),
        mlua::Error::ExternalError(inner) => inner.downcast_ref::<Raised>(),
        mlua::Error::WithContext { cause, .. } => raised(cause),
        _ => None,
    }
}

/// A Luau message's `script:<line>: ` prefix split off.
pub(crate) fn split_line(message: &str) -> (Option<u32>, String) {
    let message = message
        .split_once("\nstack traceback:")
        .map_or(message, |(m, _)| m);
    let rest = message.strip_prefix("script:");
    if let Some(rest) = rest
        && let Some((n, msg)) = rest.split_once(": ")
        && let Ok(n) = n.parse::<u32>()
    {
        return (Some(n), msg.to_owned());
    }
    (None, message.to_owned())
}

/// The byte range of the first whole-word `ident` on line `line` of `text`
/// (anywhere when the line is not known).
fn locate(text: &str, line: Option<u32>, ident: &str) -> Option<std::ops::Range<usize>> {
    let mut offset = 0;
    let mut scope = text;
    if let Some(n) = line {
        let mut start = 0;
        for (i, l) in text.split_inclusive('\n').enumerate() {
            if i + 1 == n as usize {
                offset = start;
                scope = l;
                break;
            }
            start += l.len();
        }
    }
    let bytes = scope.as_bytes();
    let mut from = 0;
    while let Some(p) = scope[from..].find(ident) {
        let at = from + p;
        let end = at + ident.len();
        let before = at.checked_sub(1).map(|i| bytes[i] as char);
        let after = bytes.get(end).map(|b| *b as char);
        if !before.is_some_and(crate::lex::ident_char) && !after.is_some_and(crate::lex::ident_char)
        {
            return Some(offset + at..offset + end);
        }
        from = end;
    }
    None
}

/// An `mlua` error as the caller's [`VmError`], placed in `text`.
fn convert(e: mlua::Error, text: &str, budget: Budget) -> VmError {
    if let Some(r) = raised(&e) {
        let span = r.ident.as_deref().and_then(|i| locate(text, r.line, i));
        return VmError {
            kind: r.kind,
            message: r.message.clone(),
            line: r.line,
            fix: r.fix.clone().zip(span.clone()).map(|(f, s)| (s, f)),
            span,
        };
    }
    let (kind, (line, message)) = match &e {
        mlua::Error::SyntaxError { message, .. } => (ErrorKind::Error, split_line(message)),
        mlua::Error::RuntimeError(message) => (ErrorKind::Error, split_line(message)),
        mlua::Error::MemoryError(_) => (
            ErrorKind::Budget,
            (
                None,
                format!("Stopped: the run used more than {} MB", budget.memory >> 20),
            ),
        ),
        mlua::Error::CallbackError { cause, .. } => {
            return convert((**cause).clone(), text, budget);
        }
        other => (ErrorKind::Error, split_line(&other.to_string())),
    };
    // Luau's own wording for two cases a script meets, said in the app's terms.
    let message = if message.contains("attempt to modify a readonly table") {
        "This table cannot be changed: a collection's layers, `param` and the library are \
         read-only"
            .to_owned()
    } else if message.contains("error in error handling") || message.contains("stack overflow") {
        "A function called itself too deeply".to_owned()
    } else {
        message
    };
    VmError {
        kind,
        message,
        line,
        span: None,
        fix: None,
    }
}
