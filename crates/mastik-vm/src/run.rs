//! The Luau runtime: one sandboxed state per evaluation, an environment whose
//! `__index` resolves names in `command-mode.md` §4's order, layer collections as tables
//! with one shared metatable, verbs bound by their registry names, and budgets.
//! Every callback reaches the host through one `RefCell<State>`, never borrowed
//! across a call back into Lua (`adjust`'s function form), which would re-enter it.

use std::cell::{Cell, RefCell};
use std::rc::Rc;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
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

/// The budget of a run that holds the thread it is called on, which waits for it:
/// an agent's `run_script`, answered in the frame.
pub const RUN_BUDGET: Budget = Budget {
    time: Duration::from_secs(1),
    steps: 200_000_000,
    memory: 64 << 20,
};

/// The budget of a run on a worker, which a stop flag can end at any step: a
/// line run with Enter, a script. Only a runaway the person leaves running
/// meets it, so it is the held budget thirty times over, memory aside.
pub const WORKER_BUDGET: Budget = Budget {
    time: Duration::from_secs(30),
    steps: 6_000_000_000,
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
/// The registry key of the metatable an empty list read from the model wears,
/// so it is written back as a list: Lua's `{}` is a list and an object alike.
const LIST: &str = "mastik.list";

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
            "floor", "fmod", "frexp", "huge", "isfinite", "isinf", "isnan", "ldexp", "lerp", "log",
            "log10", "map", "max", "min", "modf", "noise", "pi", "pow", "rad", "round", "sign",
            "sin", "sinh", "sqrt", "tan", "tanh",
        ],
    ),
    (
        "string",
        &[
            "byte", "char", "find", "format", "gmatch", "gsub", "len", "lower", "match", "pack",
            "packsize", "rep", "reverse", "split", "sub", "unpack", "upper",
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
        "math.isfinite",
        Some("x: number"),
        "boolean",
        "Whether x is neither infinite nor NaN",
    ),
    lib(
        "math.isinf",
        Some("x: number"),
        "boolean",
        "Whether x is infinite",
    ),
    lib(
        "math.isnan",
        Some("x: number"),
        "boolean",
        "Whether x is NaN, not a number",
    ),
    lib(
        "math.ldexp",
        Some("m: number, e: number"),
        "number",
        "m × 2^e",
    ),
    lib(
        "math.lerp",
        Some("a: number, b: number, t: number"),
        "number",
        "The number t of the way from a to b: a at 0, b at 1",
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
        "math.map",
        Some("x: number, inmin: number, inmax: number, outmin: number, outmax: number"),
        "number",
        "x moved from the range inmin to inmax onto outmin to outmax",
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
        "string.pack",
        Some("fmt: string, ...: any"),
        "string",
        "The values packed into a binary string as fmt lays them out",
    ),
    lib(
        "string.packsize",
        Some("fmt: string"),
        "number",
        "The length in bytes of a string string.pack makes with fmt",
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
        "string.unpack",
        Some("fmt: string, s: string, pos: number?"),
        "...any",
        "The values string.pack packed into s with fmt",
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
    (
        "random",
        "`random(seed)`: a generator drawing the same numbers for the same seed",
    ),
    ("print", "Show values in the output"),
];

/// The most a run may print, in bytes, each line counted one more for its own
/// keeping: the output is held in the host's memory, outside the Lua memory limit.
const PRINT_CAP: usize = 1 << 20;

/// The most lines a run may print: each is a row the editor's output keeps.
const PRINT_LINES: usize = 10_000;

/// Why a run stopped printing.
const PRINTED_TOO_MUCH: &str = "Stopped after printing more than 10,000 lines or 1 MB";

#[cfg(test)]
thread_local! {
    /// The longest line `print` has built on this thread, for the test of its cap.
    pub(crate) static PRINT_BUILT: Cell<usize> = const { Cell::new(0) };
}

/// The most values one verb call's arguments may hold, a collection counting
/// one per layer. A table that shares a
/// subtable at each level is small in Lua but a tree as JSON, built in the host's
/// memory where no interrupt or memory limit reaches.
const ARG_VALUES_MAX: usize = 100_000;

/// The most string bytes one verb call hands the host, counted once for each
/// layer a verb of one layer runs on: Lua shares a string by reference, and the
/// host copies it at every reference.
const ARG_BYTES_MAX: usize = 16 << 20;

/// What one verb call's arguments may still hold, spent before each value is built.
struct ArgBudget {
    values: usize,
    bytes: usize,
}

impl ArgBudget {
    fn new() -> Self {
        Self {
            values: ARG_VALUES_MAX,
            bytes: ARG_BYTES_MAX,
        }
    }

    /// Spend `n` values.
    fn values(&mut self, n: usize) -> Result<(), String> {
        self.values = self.values.checked_sub(n).ok_or_else(|| {
            format!(
                "the arguments hold more than {ARG_VALUES_MAX} values (does a table repeat itself?)"
            )
        })?;
        Ok(())
    }

    /// Spend `n` bytes of strings.
    fn bytes(&mut self, n: usize) -> Result<(), String> {
        self.bytes = self.bytes.checked_sub(n).ok_or_else(|| {
            format!(
                "the arguments hold more than {} MB (does a table repeat a string?)",
                ARG_BYTES_MAX >> 20
            )
        })?;
        Ok(())
    }

    /// The bytes spent so far.
    fn spent(&self) -> usize {
        ARG_BYTES_MAX - self.bytes
    }
}

/// Why `bytes` handed to each of `layers` layers is refused, `None` within
/// [`ARG_BYTES_MAX`].
fn per_layer_refusal(bytes: usize, layers: usize) -> Option<String> {
    (bytes.saturating_mul(layers) > ARG_BYTES_MAX).then(|| {
        format!(
            "once for each of {layers} layers, the arguments hold more than {} MB",
            ARG_BYTES_MAX >> 20
        )
    })
}

/// A spent budget, stopping the run as the interrupt does.
fn out_of_budget(message: String) -> mlua::Error {
    mlua::Error::external(Raised {
        kind: ErrorKind::Budget,
        message,
        line: None,
        ident: None,
        fix: None,
    })
}

/// Whether `stop` is set.
fn stopped(stop: Option<&AtomicBool>) -> bool {
    stop.is_some_and(|s| s.load(Ordering::Relaxed))
}

/// The error a run stopped from outside ends with.
fn stopped_error() -> mlua::Error {
    mlua::Error::external(Raised {
        kind: ErrorKind::Stopped,
        message: "Stopped".into(),
        line: None,
        ident: None,
        fix: None,
    })
}

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
    /// When the run started and how long it may take, read between the host
    /// calls one Lua call makes for each layer, where no interrupt fires.
    start: Instant,
    time: Duration,
    /// Set from outside to stop the run ([`crate::Leash::stop`]).
    stop: Option<Arc<AtomicBool>>,
}

impl State<'_> {
    /// Stop with the time budget's error once the run has spent it, or as
    /// stopped once the stop flag is set.
    fn on_time(&self) -> mlua::Result<()> {
        if stopped(self.stop.as_deref()) {
            return Err(stopped_error());
        }
        if self.start.elapsed() > self.time {
            return Err(out_of_budget(format!(
                "Stopped after {}",
                duration(self.time)
            )));
        }
        Ok(())
    }

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
        args: &serde_json::Value,
    ) -> Result<(Vec<NodeId>, Option<String>), String> {
        self.hosted.set(true);
        let done = self.host.verb(name, args)?;
        self.commit(done.tx)?;
        if let Some(said) = done.said
            && !self.say(said)
        {
            return Err(PRINTED_TOO_MUCH.into());
        }
        self.created.extend(&done.created);
        Ok((done.created, done.made))
    }

    /// Clear the style colour of the texts among `ids` that have one: a text
    /// draws in it over its fills, so new fills would not show.
    fn clear_text_colour(&mut self, ids: &[NodeId]) -> Result<(), String> {
        let coloured: Vec<serde_json::Value> =
            mastik_core::build::coloured_texts(self.host.doc(), ids)
                .iter()
                .map(|id| json!(id.to_wire()))
                .collect();
        if coloured.is_empty() {
            return Ok(());
        }
        self.apply(
            "set_text_style",
            &json!({ "nodes": coloured, "style": { "color": null } }),
        )
        .map(|_| ())
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
pub(crate) fn raise(lua: &Lua, message: impl Into<String>) -> mlua::Error {
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
/// (for placing an error), as an expression first when `expression` is set,
/// under `leash` (a live read always under [`READ_BUDGET`]).
pub(crate) fn run(
    host: &mut dyn Host,
    env: &crate::Env<'_>,
    code: &str,
    original: &str,
    mode: Mode,
    expression: bool,
    leash: &crate::Leash,
) -> Result<Outcome, VmError> {
    let budget = if mode == Mode::Read {
        READ_BUDGET
    } else {
        leash.budget
    };
    let stop = leash.stop.clone();
    let lua = Lua::new();
    let fail = |e: mlua::Error| convert(e, original, budget);
    lua.sandbox(true).map_err(fail)?;
    let steps = Rc::new(Cell::new(0u64));
    let hosted = Rc::new(Cell::new(false));
    let start = Instant::now();
    let after_host = hosted.clone();
    let flag = stop.clone();
    lua.set_interrupt(move |_| {
        // Read at every step: one relaxed load, so a stop lands at the next
        // call or loop edge.
        if stopped(flag.as_deref()) {
            return Err(stopped_error());
        }
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
            Some(message) => Err(out_of_budget(message)),
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
        start,
        time: budget.time,
        stop,
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
            lua.set_named_registry_value(LIST, lua.create_table()?)?;

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
                // Built against the room left, a string measured before it is copied.
                let room = PRINT_CAP.saturating_sub(state.borrow().printed_bytes);
                let mut line = String::new();
                let mut fits = true;
                for (i, v) in args.iter().enumerate() {
                    let tab = usize::from(i > 0);
                    let len = match v {
                        Value::String(s) => s.as_bytes().len(),
                        _ => 0,
                    };
                    if line.len() + tab + len >= room {
                        fits = false;
                        break;
                    }
                    if i > 0 {
                        line.push('\t');
                    }
                    line.push_str(&printed(lua, state, v));
                    #[cfg(test)]
                    PRINT_BUILT.set(PRINT_BUILT.get().max(line.len()));
                }
                if fits && state.borrow_mut().say(line) {
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
                None => Ok((None, None)),
                Some(Value::String(s)) => {
                    let raw = s.to_str()?.to_owned();
                    Ok((Some(format!("\"{raw}\"")), Some(raw)))
                }
                Some(v) => {
                    let shown = shown(&lua, state, &v)?;
                    Ok((shown.clone(), shown))
                }
            }
        })
        .map_err(fail)?;
    let (value, copied) = value;
    let state = state.into_inner();
    Ok(Outcome {
        printed: state.printed,
        value,
        copied,
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

/// Whether `t` is an empty list read from the model ([`LIST`]).
fn is_list(lua: &Lua, t: &Table) -> mlua::Result<bool> {
    let list: Table = lua.named_registry_value(LIST)?;
    Ok(t.metatable().is_some_and(|m| m == list))
}

/// A JSON value as Lua: objects and arrays as tables, an empty array marked as
/// a list ([`LIST`]).
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
            if list.is_empty() {
                t.set_metatable(Some(lua.named_registry_value::<Table>(LIST)?))?;
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
        "random" => {
            let random = lua.create_function(crate::random::random)?;
            return Ok((Value::Function(random), true));
        }
        "math" | "table" => return Ok((Value::Table(safe_library(lua, name)?), true)),
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

/// Every global name a script could have meant: the check's
/// (`help::global_names`) and the leap labels.
fn global_names(state: &RefCell<State<'_>>, env: &crate::Env<'_>) -> Vec<String> {
    let st = state.borrow();
    let mut out = crate::help::global_names(st.host.doc(), st.host.page());
    out.extend(env.labels.keys().cloned());
    out
}

/// The layers under `page` called exactly `name`.
pub(crate) fn named(doc: &mastik_core::Document, page: NodeId, name: &str) -> Vec<NodeId> {
    crate::layers_under(doc, page)
        .filter(|id| doc.get(*id).is_some_and(|n| n.name() == name))
        .collect()
}

/// A read-only copy of library `name`: `math` without its random numbers, since
/// a run is deterministic (`random(seed)` draws numbers its seed fixes), and
/// `table` without the deprecated `foreach`,
/// `foreachi` and `getn`, so every member a script reaches is one [`LIBRARY`] lists.
fn safe_library(lua: &Lua, name: &str) -> mlua::Result<Table> {
    let library: Table = lua.globals().get(name)?;
    let left_out: &[&str] = match name {
        "math" => &["random", "randomseed"],
        "table" => &["foreach", "foreachi", "getn"],
        _ => &[],
    };
    let safe = lua.create_table()?;
    for pair in library.pairs::<Value, Value>() {
        let (k, v) = pair?;
        if let Value::String(s) = &k
            && left_out.contains(&&*s.to_str()?)
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
        // Nothing near: the names there are, so the next guess is not blind.
        None => {
            let mut names: Vec<&str> = Vec::new();
            for p in props::PROPERTIES {
                if !names.contains(&p.name) {
                    names.push(p.name);
                }
            }
            format!("No property “{name}”; a layer has {}", names.join(", "))
        }
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

/// `f` over the layers of collection `t`, lent rather than copied; `None` for
/// any other table.
fn with_layers<R>(lua: &Lua, t: &Table, f: impl FnOnce(&[NodeId]) -> R) -> mlua::Result<Option<R>> {
    let registry: Table = lua.named_registry_value(IDS)?;
    match registry.raw_get::<Value>(t)? {
        Value::UserData(ud) => Ok(Some(f(&ud.borrow::<Vec<NodeId>>()?))),
        _ => Ok(None),
    }
}

/// The layers of collection `t`, or of a plain list of collections (`{a, b}`,
/// each a layer or a collection), in its order and each once; `None` for any
/// other table.
fn layers_of(lua: &Lua, t: &Table) -> mlua::Result<Option<Vec<NodeId>>> {
    if let Some(ids) = with_layers(lua, t, <[NodeId]>::to_vec)? {
        return Ok(Some(ids));
    }
    let len = t.raw_len();
    if len == 0 || t.pairs::<Value, Value>().count() != len {
        return Ok(None);
    }
    let mut out: Vec<NodeId> = Vec::new();
    for v in t.sequence_values::<Value>() {
        let Value::Table(item) = v? else {
            return Ok(None);
        };
        let Some(ids) = with_layers(lua, &item, <[NodeId]>::to_vec)? else {
            return Ok(None);
        };
        for id in ids {
            if !out.contains(&id) {
                out.push(id);
            }
        }
    }
    Ok(Some(out))
}

/// The layers of collection or list `t` ([`layers_of`]); raises for any other
/// table.
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
        Val::Json(v) => json_lua(lua, &v)?,
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
    if MODELLED.contains(&prop)
        || (matches!(prop, "fill" | "stroke") && matches!(v, Value::Table(_)))
    {
        return write_model(lua, state, ids, property, v);
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
                        st.on_time()?;
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
                        st.on_time()?;
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
                            &json!({ "node": id.to_wire(), "width": w, "height": h }),
                        )
                        .map_err(fail)?;
                    }
                }
                "rotation" => {
                    for id in ids {
                        st.on_time()?;
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
                    st.apply("set_opacity", &json!({ "nodes": wires, "opacity": n }))
                        .map_err(fail)?;
                }
                "corner_radius" => {
                    for id in ids {
                        st.on_time()?;
                        st.apply(
                            "set_geometry",
                            &json!({ "node": id.to_wire(), "geometry": { "CornerRadius": n } }),
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
                st.apply("set_fills", &json!({ "nodes": wires, "fills": paint }))
                    .map_err(fail)?;
                st.clear_text_colour(ids).map_err(fail)?;
            } else {
                st.apply("set_strokes", &json!({ "nodes": wires, "strokes": paint }))
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
                    if let Some(why) = per_layer_refusal(text.len(), ids.len()) {
                        return Err(fail(format!("`.{prop}`: {why}")));
                    }
                    let verb = if prop == "name" {
                        "set_name"
                    } else {
                        "set_text"
                    };
                    // One argument, its layer swapped for each, rather than a copy per layer.
                    let mut args = json!({ "node": null, prop: text });
                    for id in ids {
                        st.on_time()?;
                        args["node"] = json!(id.to_wire());
                        st.apply(verb, &args).map_err(fail)?;
                    }
                }
                "blend" => {
                    st.apply("set_blend", &json!({ "nodes": wires, "blend": text }))
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
            st.apply(verb, &json!({ "nodes": wires, prop: b }))
                .map_err(fail)?;
        }
        PropType::Layers => return Err(raise(lua, format!("`.{prop}` is read-only"))),
        // Every one is `MODELLED`, written above.
        PropType::Table | PropType::Length | PropType::Sides => {
            return Err(no_property(lua, prop));
        }
    }
    Ok(())
}

/// "a number", "a colour": how a refusal names a property's type.
fn article(ty: PropType) -> &'static str {
    match ty {
        PropType::Number => "a number",
        PropType::Text => "a string",
        PropType::Bool => "true or false",
        PropType::Colour => "a colour like \"#ff7a1a\", a paint's table, or nil",
        PropType::Layers => "a collection",
        PropType::Table => "a table",
        PropType::Length => "a number of px or a length like \"0.05em\"",
        PropType::Sides => "a number or a table of four",
    }
}

/// The properties written as the model's own values, through [`write_model`].
const MODELLED: &[&str] = &[
    "fills",
    "strokes",
    "effects",
    "font_family",
    "font_size",
    "font_weight",
    "italic",
    "line_height",
    "letter_spacing",
    "text_case",
    "text_align",
    "layout",
    "direction",
    "gap",
    "padding",
    "layout_item",
];

/// `v` as plain JSON for a verb's argument of `kind`, with no leap labels to read.
fn plain_json(lua: &Lua, v: &Value, kind: Option<ParamKind>) -> Result<serde_json::Value, String> {
    let labels = rustc_hash::FxHashMap::default();
    let env = crate::Env {
        selection: &[],
        labels: &labels,
        params: Map::new(),
    };
    to_json(lua, v, kind, &env, 0, &mut ArgBudget::new())
}

/// An enum word as the model spells it, written in any case or with dashes:
/// `"row-reverse"` and `"ROW_REVERSE"` are `"RowReverse"`.
fn model_word(s: &str) -> String {
    s.split(['-', '_', ' '])
        .map(|w| {
            let mut c = w.chars();
            c.next().map_or_else(String::new, |f| {
                f.to_uppercase()
                    .chain(c.flat_map(char::to_lowercase))
                    .collect()
            })
        })
        .collect()
}

/// A length a script wrote, in the model's shape: a number is px, and a string
/// `"12px"`, `"0.05em"` or `"5%"` (of the font size, as the field shows it).
fn length_json(v: &Value) -> Option<serde_json::Value> {
    if let Some(n) = number(v) {
        return n.is_finite().then(|| json!({ "Px": n }));
    }
    let Value::String(s) = v else {
        return None;
    };
    let s = s.to_str().ok()?.trim().to_owned();
    let (n, unit) = if let Some(n) = s.strip_suffix("px") {
        (n, "Px")
    } else if let Some(n) = s.strip_suffix("em") {
        (n, "Em")
    } else if let Some(n) = s.strip_suffix('%') {
        let n: f64 = n.trim().parse().ok()?;
        return n.is_finite().then(|| json!({ "Em": n / 100.0 }));
    } else {
        (s.as_str(), "Px")
    };
    let n: f64 = n.trim().parse().ok()?;
    n.is_finite().then(|| json!({ unit: n }))
}

/// The character attribute `prop` set to `v`, as `style_text` takes it.
fn char_attr(prop: &str, v: &Value) -> Option<serde_json::Value> {
    let text = |v: &Value| match v {
        Value::String(s) => s.to_str().ok().map(|s| s.to_owned()),
        _ => None,
    };
    Some(match prop {
        "font_family" => json!({ "Family": text(v)? }),
        "font_size" => json!({ "Size": number(v).filter(|n| n.is_finite())? }),
        "font_weight" => {
            let w = number(v).filter(|n| n.is_finite() && *n >= 1.0 && *n <= 1000.0)?;
            json!({ "Weight": w.round() as u16 })
        }
        "italic" => match v {
            Value::Boolean(b) => json!({ "Italic": b }),
            _ => return None,
        },
        "line_height" => match v {
            Value::Nil => json!({ "LineHeight": null }),
            v => json!({ "LineHeight": length_json(v)? }),
        },
        "letter_spacing" => json!({ "LetterSpacing": length_json(v)? }),
        "text_case" => json!({ "Case": model_word(&text(v)?) }),
        _ => return None,
    })
}

/// Write a property whose value is the model's own (`MODELLED`, or a paint's
/// table for `.fill` and `.stroke`) through the verb the inspector's card uses.
fn write_model(
    lua: &Lua,
    state: &RefCell<State<'_>>,
    ids: &[NodeId],
    property: &props::Property,
    v: Value,
) -> mlua::Result<()> {
    let prop = property.name;
    let fail = |why: String| raise(lua, format!("`.{prop}`: {why}"));
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
    // Each layer `f` refuses, by name: a text's property on a frame, a gap where
    // nothing is laid out.
    let named = |st: &State<'_>, id: NodeId| describe(st.host.doc(), &[id]);
    match prop {
        "fill" | "fills" | "stroke" | "strokes" | "effects" => {
            let list_kind = match prop {
                "fill" | "fills" => "[Fill]",
                "stroke" | "strokes" => "[Stroke]",
                _ => "[Effect]",
            };
            let list = match &v {
                Value::Nil => json!([]),
                Value::Table(_) if matches!(prop, "fill" | "stroke") => {
                    json!([plain_json(lua, &v, None).map_err(fail)?])
                }
                Value::Table(_) => {
                    plain_json(lua, &v, Some(ParamKind::Model(list_kind))).map_err(fail)?
                }
                _ => return Err(wrong()),
            };
            match list_kind {
                "[Fill]" => {
                    // A text in its style colour reads that colour as its fill,
                    // so a read written back draws as it did with the colour
                    // cleared, and any write takes effect.
                    st.apply("set_fills", &json!({ "nodes": wires, "fills": list }))
                        .map_err(fail)?;
                    st.clear_text_colour(ids).map_err(fail)?;
                }
                "[Stroke]" => {
                    st.apply("set_strokes", &json!({ "nodes": wires, "strokes": list }))
                        .map_err(fail)?;
                }
                _ => {
                    st.apply("set_effects", &json!({ "nodes": wires, "effects": list }))
                        .map_err(fail)?;
                }
            }
        }
        "font_family" | "font_size" | "font_weight" | "italic" | "line_height"
        | "letter_spacing" | "text_case" => {
            let attr = char_attr(prop, &v).ok_or_else(wrong)?;
            for id in ids {
                st.on_time()?;
                if !matches!(
                    st.host.doc().get(*id).map(|n| n.kind()),
                    Some(mastik_core::NodeKind::Text { .. })
                ) {
                    return Err(fail(format!("“{}” is not a text", named(&st, *id))));
                }
                st.apply(
                    "style_text",
                    &json!({ "node": id.to_wire(), "attrs": [attr] }),
                )
                .map_err(fail)?;
            }
        }
        "text_align" => {
            let Value::String(s) = &v else {
                return Err(wrong());
            };
            let align = model_word(&s.to_str()?);
            for id in ids {
                st.on_time()?;
                let paragraph = match st.host.doc().get(*id).map(|n| n.kind()) {
                    Some(mastik_core::NodeKind::Text { paragraph, .. }) => {
                        serde_json::to_value(paragraph).map_err(|e| fail(e.to_string()))?
                    }
                    _ => return Err(fail(format!("“{}” is not a text", named(&st, *id)))),
                };
                let mut paragraph = paragraph;
                paragraph["align"] = json!(align);
                st.apply(
                    "set_paragraph_style",
                    &json!({ "nodes": [id.to_wire()], "paragraph": paragraph }),
                )
                .map_err(fail)?;
            }
        }
        "layout" => {
            let args = match &v {
                Value::Nil => json!({ "nodes": wires }),
                Value::Table(_) => json!({
                    "nodes": wires,
                    "display": plain_json(lua, &v, Some(ParamKind::Model("Display"))).map_err(fail)?,
                }),
                _ => return Err(wrong()),
            };
            st.apply("set_display", &args).map_err(fail)?;
        }
        "layout_item" => {
            let Value::Table(_) = &v else {
                return Err(wrong());
            };
            let item = plain_json(lua, &v, Some(ParamKind::Model("LayoutItem"))).map_err(fail)?;
            st.apply("set_layout_item", &json!({ "nodes": wires, "item": item }))
                .map_err(fail)?;
        }
        "direction" | "gap" | "padding" => {
            let (key, value) = match (prop, &v) {
                ("direction", Value::String(s)) => {
                    use mastik_core::container::FlexDirection;
                    // Its word, its CSS keyword or the name it reads as
                    // (`"RowReverse"`), in any case and with any separators.
                    let word = s.to_str()?;
                    let joined = model_word(&word);
                    let d = FlexDirection::from_word(&word)
                        .or_else(|| {
                            FlexDirection::ALL
                                .into_iter()
                                .find(|d| format!("{d:?}").eq_ignore_ascii_case(&joined))
                        })
                        .ok_or_else(|| {
                            fail(format!(
                                "no direction “{}”: \"Row\", \"Column\", \"RowReverse\" or \"ColumnReverse\"",
                                &*word
                            ))
                        })?;
                    ("direction", json!(d))
                }
                ("gap", v) if number(v).is_some_and(f64::is_finite) => ("gap", json!(number(v))),
                ("padding", v) if number(v).is_some_and(f64::is_finite) => {
                    let n = number(v);
                    ("padding", json!([n, n, n, n]))
                }
                ("padding", Value::Table(_)) => {
                    let sides =
                        plain_json(lua, &v, Some(ParamKind::Model("[f64]"))).map_err(fail)?;
                    ("padding", sides)
                }
                _ => return Err(wrong()),
            };
            for id in ids {
                st.on_time()?;
                let display = st.host.doc().get(*id).and_then(|n| n.display()).cloned();
                let Some(display) = display else {
                    return Err(fail(format!(
                        "“{}” lays out nothing — set its .layout first",
                        named(&st, *id)
                    )));
                };
                let grid = matches!(display, mastik_core::container::Display::Grid(_));
                if grid && key == "direction" {
                    return Err(fail(format!(
                        "“{}” is a grid, which has no direction",
                        named(&st, *id)
                    )));
                }
                let mut whole = serde_json::to_value(&display).map_err(|e| fail(e.to_string()))?;
                let inner = whole
                    .get_mut(if grid { "Grid" } else { "Flex" })
                    .and_then(serde_json::Value::as_object_mut)
                    .ok_or_else(|| fail("its layout reads in no known shape".into()))?;
                if key == "gap" {
                    inner.insert("row_gap".into(), value.clone());
                    inner.insert("column_gap".into(), value.clone());
                } else {
                    inner.insert(key.into(), value.clone());
                }
                st.apply(
                    "set_display",
                    &json!({ "nodes": [id.to_wire()], "display": whole }),
                )
                .map_err(fail)?;
            }
        }
        _ => return Err(no_property(lua, prop)),
    }
    Ok(())
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
        st.on_time()?;
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
            &json!({ "nodes": [id.to_wire()], "item": item }),
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
    if !matches!(property.ty, PropType::Number | PropType::Length) {
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

/// Call verb `name` with Lua's arguments: a collection or a list of layers first
/// fills the subject (the first layer parameter), else the selection does; the
/// rest fill the other parameters in order; one other plain table is the named
/// arguments. Answers what it
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
    let mut budget = ArgBudget::new();
    if named_form {
        let Some(Value::Table(t)) = args.first() else {
            unreachable!("the named form is one table");
        };
        for pair in t.pairs::<String, Value>() {
            let (key, v) = pair?;
            let kind = verb.params.iter().find(|p| p.name == key).map(|p| p.kind);
            let json = to_json(lua, &v, kind, env, 0, &mut budget).map_err(fail)?;
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
                to_json(lua, v, Some(p.kind), env, 0, &mut budget).map_err(fail)?,
            );
        }
    }
    // The subject, from the collection given or the selection. A verb of one layer
    // runs once for each, on one argument map with its layer swapped in each time.
    let mut each: Option<(&str, Vec<NodeId>)> = None;
    if let Some(i) = subject {
        let p = &verb.params[i];
        if !map.contains_key(p.name) {
            let ids = given.unwrap_or_else(|| env.selection.to_vec());
            if ids.is_empty() {
                if p.kind == ParamKind::Node && p.name == "parent" {
                    let page = state.borrow().host.page();
                    map.insert(p.name.into(), json!(page.to_wire()));
                } else if p.required {
                    return Err(raise(
                        lua,
                        format!("Nothing is selected, and `{name}` needs {}", p.doc),
                    ));
                }
            } else if p.kind == ParamKind::Nodes {
                let wires: Vec<String> = ids.iter().map(|id| id.to_wire()).collect();
                map.insert(p.name.into(), json!(wires));
            } else {
                if let Some(why) = per_layer_refusal(budget.spent(), ids.len()) {
                    return Err(fail(why));
                }
                each = Some((p.name, ids));
            }
        }
    }
    let mut args = serde_json::Value::Object(map);
    let runs = each.as_ref().map_or(1, |(_, ids)| ids.len());
    let mut created = Vec::new();
    let mut made = None;
    for run in 0..runs {
        // The host calls follow one another in Rust, where no interrupt fires.
        state.borrow().on_time()?;
        if let Some((key, ids)) = &each {
            args[*key] = json!(ids[run].to_wire());
        }
        let (c, m) = state
            .borrow_mut()
            .apply(name, &args)
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
/// Each value spends one of `budget`'s values, a collection one per layer and a
/// string its bytes, shared across the call's arguments and spent before the copy.
fn to_json(
    lua: &Lua,
    v: &Value,
    kind: Option<ParamKind>,
    env: &crate::Env<'_>,
    depth: usize,
    budget: &mut ArgBudget,
) -> Result<serde_json::Value, String> {
    if depth > 32 {
        return Err("a table nests too deep (is it inside itself?)".into());
    }
    budget.values(1)?;
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
            budget.bytes(s.as_bytes().len())?;
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
            let wires = with_layers(lua, t, |ids| {
                budget.values(ids.len())?;
                Ok::<_, String>(ids.iter().map(|id| id.to_wire()).collect::<Vec<_>>())
            });
            if let Some(wires) = wires.map_err(lua_err)? {
                let wires = wires?;
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
                || matches!(kind, Some(ParamKind::Nodes))
                || is_list(lua, t).map_err(lua_err)?;
            if t.raw_len() > 0 || (listy && t.clone().pairs::<Value, Value>().next().is_none()) {
                let mut list = Vec::new();
                for item in t.clone().sequence_values::<Value>() {
                    let item = item.map_err(lua_err)?;
                    let inner = (kind == Some(ParamKind::Nodes)).then_some(ParamKind::Node);
                    list.push(to_json(lua, &item, inner, env, depth + 1, budget)?);
                }
                serde_json::Value::Array(list)
            } else {
                let mut map = Map::new();
                for pair in t.clone().pairs::<Value, Value>() {
                    let (k, item) = pair.map_err(lua_err)?;
                    let key = match &k {
                        Value::String(s) => {
                            budget.bytes(s.as_bytes().len())?;
                            s.to_str().map_err(lua_err)?.to_owned()
                        }
                        Value::Number(_) | Value::Integer(_) => {
                            format_number(number(&k).unwrap_or(0.0))
                        }
                        other => return Err(format!("a table key cannot be {}", type_name(other))),
                    };
                    map.insert(key, to_json(lua, &item, None, env, depth + 1, budget)?);
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
            Err(_) => mlua::ObjectLike::to_string(ud).unwrap_or_else(|_| "userdata".into()),
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

/// The byte range of the first name `ident` on line `line` of `text` (anywhere
/// when the line is not known): a name the lexer reads, never the same letters
/// in a string or a comment, so a fix lands on the code that raised.
fn locate(text: &str, line: Option<u32>, ident: &str) -> Option<std::ops::Range<usize>> {
    let mut scope = 0..text.len();
    if let Some(n) = line {
        let mut start = 0;
        for (i, l) in text.split_inclusive('\n').enumerate() {
            if i + 1 == n as usize {
                scope = start..start + l.len();
                break;
            }
            start += l.len();
        }
    }
    crate::lex::lex(text)
        .into_iter()
        .filter(|l| l.kind == crate::lex::Lx::Ident)
        .map(|l| l.range)
        .find(|r| scope.contains(&r.start) && &text[r.clone()] == ident)
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
        mlua::Error::SyntaxError { message, .. } => {
            // Placed in the text as typed, the token it names squiggled.
            let (line, message) = split_line(message);
            let (line, span) = crate::help::syntax_place(text, line, &message);
            return VmError {
                kind: ErrorKind::Error,
                message: crate::help::plain_syntax(&message),
                line: Some(line),
                span: Some(span),
                fix: None,
            };
        }
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
