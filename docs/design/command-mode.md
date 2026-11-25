# Command Mode (vm.md §1–§12)

Command Mode is the keyboard's way into the document: the **command bar** runs one line of the
scripting language against the selection (its **Type** focus) and names layers by leap labels
dealt over the canvas (its **Pick** focus), and **scripts** are whole programs in that language,
kept in the library folder, in a document or in a kit. The language is Luau in the headless crate
`mastik-vm`; the app's half is `command/` (the bar, its log, the host, the script editor),
`navigate.rs` (Pick and the labels) and `scripts/` (the library folder). The
decisions it rests on are `foundations.md` §13. Headings keep the retired `vm.md`'s numbers, so a
`vm.md §4` cited from code is found here.

## 1. The thesis

One sandboxed language serves three consumers: the operator typing at speed, the person automating
a repetitive build, and an agent writing edits. A loop is in-band, where a JSON tool call would
unroll it. The language is a front end and adds no mutation path of its own:

- Every write is a registry verb (`mastik_core::verbs`) or core's world-space facade
  (`build::move_by_world`, `edit::turned_to`), committed through the host's commit door
  (invariant 2).
- `NodeId`s are never reused, so a handle to a deleted layer raises "no such layer" and can never
  alias another one.
- What a read means across several layers is core's answer, as the inspector shows it
  (`PaintShown::Mixed`, `build::OPACITY_AGREEMENT`, `build::RADIUS_AGREEMENT`).

## 2. Luau (`mastik-vm`)

**Luau through `mlua`, pinned `=0.11.6` with `luau` and `vendored`.** The deciding reason is `+=`:
the bar's `.x += 15` needs compound assignment, which Lua 5.4 lacks. Rhai was declined because
models have seen little of it. `vendored` compiles Luau's C++ with the platform's compiler, so the
workspace needs a C++ toolchain (MSVC on Windows; `foundations.md` §2). The pin is exact because
the language rests on behaviour checked against this version (globals proxied through `__index`
under `Lua::sandbox`, a Rust `__index` on a per-evaluation environment, the interrupt firing at
calls and loop edges); a bump re-runs the crate's tests first.

**The crate is headless**: `mastik-core`, `serde_json`, `rustc-hash` and `mlua`, held to the
headless list by `deps_forbidden` (`foundations.md` §3). It reaches the document only through the
caller's `Host` (`doc`, `resolved`, `page`, `verb`, `commit`). Its surface: `Env` (selection, leap
labels, a script's input values), `Outcome` (printed lines, a read's value as shown and as
copied, layers created, the selection to set), `VmError { kind: Error | Budget | Stopped, message,
line, span, fix }`, `desugar`, `eval_read`, `read_line`, `run_line`, `run_script` (under the held
budget), `run_line_leashed` and `run_script_leashed` (under a `Leash`: a `Budget` and a stop
flag), the budgets (§8), `header`, `complete`, `tokens`, `names_used`, `as_read`, `quoted`,
`definitions`. The property tables stay inside the crate.

**One sandboxed state per evaluation.** Each evaluation makes a fresh `Lua`, turns on
`Lua::sandbox` (the global table read-only) and loads the chunk with an environment table of its
own whose `__index` is the resolver (§4). The resolver cannot sit on the globals, which the sandbox
has made read-only; the permitted standard library is read off the sandboxed globals with
`globals().get`. **Globals are strict**: the environment's `__newindex` refuses an assignment to an
undeclared name (*No global “n” — write `local n = …`*), and an unknown name raises rather than
reading as nil, so a typo is an error at its line, not a silent no-op later.

## 3. Two tiers, one runtime

**Tier 1** is the bar's line sugar; **tier 2** is Luau. The sugar is a few textual rewrites that
produce Luau (`desugar`), and the bar's footer shows the Luau it produced, so a terse line teaches
the language. A script gets no sugar: it is Luau as written.

The rewrites, in order, on one line:

1. **Verb call.** A first word that names a registry verb, followed by whitespace or the end and not
   by `=`, a compound operator or an opening `{`, `(` or `[`, is a call; the other words are its
   arguments. Numbers, `true`, `false`, `nil` and quoted strings stay themselves; any other word
   becomes a string. `align left` is `align("left")`, `set_visible false` is `set_visible(false)`.
   A bracket after the space is Luau's own call form and runs as written: `duplicate { dx = 7 }`,
   `align ("left")`. **Names are the registry's, with no aliases**: `dup 3` is not a verb and
   runs as written.
2. **Subject elision.** A `.name` in prefix position (the line's start, after an operator, an
   opening bracket, an interpolation's `{`, a comma, a keyword that a value follows, or a number or
   string, which Luau cannot index unless the string is a call's argument) is `sel.name`:
   `sk.x = .x + 5` is `sk.x = sel.x + 5`.
3. **Compound assignment adjusts each layer by itself.** `<expr>.<prop> op= <rhs>` is
   `<expr>:adjust("<prop>", by)`: `+=` adds, `-=` adds the negation, and `*=`, `/=`, `//=`, `%=`,
   `^=` map each layer's own value through a function, whose parameter is named apart from every
   name in `<rhs>`. Each statement on the line is rewritten by itself: `<expr>` starts after the
   `;`, comment or block keyword before it and `<rhs>` ends at the one after, or where a name or a
   statement keyword follows a complete expression, so `.x += 5; .y -= 2`, `.x += 5 .y += 3` and
   `for … do l.x += 5 end` all work. The footer says
   *each by itself*. This is
   the one place the tiers differ, and it is kept: in a script `sel.x += 15` is Luau's read of the
   shared value then a write, which raises on a selection that disagrees; a script spells the
   per-layer form `sel:adjust("x", 15)`.
4. **Percent.** A number followed directly by `%`, with nothing after it but a closing bracket, a
   comma, a `;`, a comment, a block's closing keyword or the end, is `percent(n)` (§5). `10 % 3` stays a remainder.

Anything else is Luau, verbatim. `LineKind` sorts the result: **Read** (no assignment, no verb
call, no writing method) or **Write**; only a read is evaluated as it is typed.

## 4. Name resolution

The environment's `__index` (`run::resolve`) answers an unknown name in this order:

1. `sel` (alias `selection`): the selection, always a collection.
2. `parent`, `prev`, `next`, `children`: relative to the selection.
3. **Leap labels**, two lowercase home-row letters, each bound to its layer for as long as the
   document is open (*Pick*). A label whose layer was deleted raises.
4. **Registry verbs**, by their registry names.
5. `print` (to the bar's result or the editor's output), `percent`, `random` (§8), `math`
   without `random` and `randomseed`, `table` without the deprecated `foreach`, `foreachi` and
   `getn`, `param` (a script's inputs, §10), `tables` (the document's tables, `lists.md`
   *Verbs*; answered before `run::resolve`, ahead of every name here), `play` (likewise, and only
   in a script a playing prototype runs, `play.md` *Run script*; anywhere else, `Host::play`
   being `None`, it is a layer's name when one has it and raises at its line when none does),
   and the allowlist `string`, `ipairs`,
   `pairs`, `select`, `tonumber`, `tostring`, `type`, `assert`, `error`. A name off the allowlist
   (`os`, `debug`, `require`, `load`, `pcall`, `setmetatable`…) is an unknown name.
6. **A layer name** on the current page, matched exactly. Two or more raise, naming the count and
   pointing to a leap label: names are not unique (`naming.rs`), so a name is a convenience, never
   the primary handle.
7. Otherwise it raises, naming the name and the nearest known one within two edits as a fix that
   `Tab` takes.

Verbs, the selection and the standard library are kept for the run; a relative name, a label and a
layer name are looked up on each use, since the run's own edits move them. **In a verb's
arguments**, a string naming a leap label, passed where the verb takes a layer, is that layer.

## 5. The subject, and mixed values

**A collection is a Lua table with one shared metatable**: read-only, its array part holding
one-layer collections (a one-layer collection holds itself), so `#sel`, `sel[1]` and `ipairs(sel)`
work as Lua's own; a weak registry table maps each collection to its `NodeId`s. `sel` is always a
collection, so a line behaves the same at any selection size.

- **A write broadcasts**, one layer after another, each measuring the document the one before left.
- **A read is the shared value, and raises when the layers disagree**, within the inspector's
  tolerances (`OPACITY_AGREEMENT` for opacity, `RADIUS_AGREEMENT` for every other number). The error
  lists up to four of the values and says how to go on: `.x += 20` in the bar, `sel:adjust("x", 20)`
  in a script, `sel[1].x` for one, `sel:shared("x")` to branch.
- **Methods**: `sel:shared(name)` (the value, or nil when they differ), `sel:adjust(name, by)` (a
  number added to each layer's own value, or a function mapping it), `sel:select()` (select these
  once the run ends).

**Properties** are one table, `props::PROPERTIES`, which the runtime, completion, the error
suggestions and the `.d.luau` file all read. Geometry is world space (`foundations.md` §13
decision 2):

| Property | Reads | Writes through |
| --- | --- | --- |
| `x`, `y` | the left and top of the layer's bounds on the page | `build::move_by_world`; refused on a layer its container lays out |
| `width`, `height` | the layer's own box | the `set_size` verb; `percent(n)` or `50%` sets the size's % mode (`set_layout_item`) only where the size field offers %, a resizable item of a flex or grid container, and is an error saying so elsewhere |
| `rotation` | degrees, clockwise on the page | `edit::turned_to` |
| `opacity`, `corner_radius`, `name`, `visible`, `locked`, `text`, `blend` | the layer's own, a bound opacity at its variable's value | the matching verb |
| `fill`, `stroke` | the one paint: a shown plain colour (and a stroke with every default but its colour) as `"#rrggbb"` (`"#rrggbbaa"` when not opaque), any other one paint — a gradient, a picture, a hidden or bound one — as its table, nil for none; several are no one value; a text drawn in its style colour reads that colour | `set_fills` / `set_strokes`, a colour or one paint's table; a text written has its style colour cleared so the fill shows |
| `fills`, `strokes`, `effects` | the whole list, bottom first, each item a table (a plain fill as its colour); a text drawn in its style colour reads that colour alone as its fills | `set_fills` / `set_strokes` / `set_effects`; nil or `{}` clears |
| `font_family`, `font_size`, `font_weight`, `italic`, `line_height`, `letter_spacing`, `text_case` | a text's character style over all its characters as drawn, a bound slot at its variable's value (no one value when its runs differ); a length as a number of px or a string such as `"1.4em"`, `line_height` nil for auto; nil on any other layer | `style_text` over the whole text, as the Text card writes it, so the runs that set the field give way; a length takes px, `"12px"`, `"0.05em"` or `"5%"` of the font size; refused on a layer that is not a text |
| `text_align` | a text's paragraph alignment (`"Start"`, `"Center"`…) | `set_paragraph_style`, the rest of the paragraph kept |
| `layout` | a frame's flex or grid layout as its table, `{ Flex = { … } }` or `{ Grid = { … } }`, nil for none | `set_display`; nil turns it off |
| `gap`, `padding`, `direction` | as laid out, a bound one at its variable's value: the layout's gap (no one value when its rows and columns differ), its padding (one number when the sides agree, else `{ top, right, bottom, left }`) and a flex layout's direction; nil without a layout | `set_display`, the rest of the layout kept; refused where nothing is laid out, and `direction` on a grid |
| `layout_item` | how the layer sizes and places itself in its parent's layout, as its table (`{}` for the defaults): as authored, a bound limit at its variable's value, never the width a frame with width queries is laid at (`Document::bound_item`) | `set_layout_item` |
| `reactions`, `play` | the layer's prototype reactions and settings, in the shapes `set_reactions` and `set_play` take (`verbs::play_value`), each transition whole, so `B.reactions = A.reactions` keeps A's transitions on a page whose default differs; a write sets the list as given, ids and targets as they are, so copying one layer's reactions onto others is the verb `copy_reactions(others, a)`, *Paste interactions* | `set_reactions` / `set_play`; nil clears |
| `flows` | the flows of the page the layer is on (`verbs::play_value`); a page's own layer reads its own | `set_flows` on the page the layers are on, refused over layers on two pages, since a flow starts on its own page's frame; nil clears |
| `id`, `kind`, `parent`, `children`, `prev`, `next` | read-only | — |

**Read and write round-trip**: a read gives what is drawn in the model's own terms — a bound
value at its variable's value, a text's style colour as its fill — and never a value layout
derives, so a read written back draws as it did and every write takes effect. A table reads in
the short forms the verbs read back to the same value (`verbs::shorten`: hex colours, a paint without its default fields, lengths as `"22px"`),
so `local f = sel.fills; sel.fills = f` changes nothing, and a read table edited in Lua writes
back (`local s = sel.strokes; s[1].width = 3; sel.strokes = s`). An empty list read from the
model keeps a list's mark, since Lua's `{}` is a list and an object alike. Enum words take any
case and dashes (`"row-reverse"` is `"RowReverse"`). `sel:adjust` steps any number, a px length
included.

A property no layer has raises with the one it most likely means as the fix
(`run::nearest_property`): for a name people guess (`alpha`, `colour`, `radius`, `weight`,
`tracking`, `shadow`) the property it means, else the nearest within two edits (*No property
“x” — did you mean y?*). With nothing near, it lists the properties a layer has (*No property
“text_style”; a layer has x, y, width, …*), so the next guess is not blind.

**A verb call**: a collection given first fills the verb's subject (its first layer parameter),
and so does a plain list of layers and collections (`{a, b}`), each layer once in its order
(`layers_of`), else the selection does; a verb of one layer runs once for each; the remaining
arguments fill the other parameters in order, and one plain table that is no such list is the
named form: a lone `{a, b}` is the subject, not named arguments. It answers what it created as a
collection, else the id of what it made, else nil. What the last creating verb made, or what
`:select()` named last, is selected when the run ends.

## 6. What is bound, and what is not

**Bound:** every registry verb, the properties through the world-space facade, and the selection.
The host lends no image sink and no contrast probe, so the verbs that need one (`add_image`,
`fix_contrast`) refuse. It refuses a write to a kit's vendored component, which changes only
when the kit does: the check is on each commit, so it covers a verb, a property write and MCP's
`run_script`, which takes raw ids, alike. Reading a vendored main, as an instance does, is fine.

**Not bound, by design:** gestures, tools as interaction, previews, `RenderOverrides` and egui
chrome. A script can only say a committed outcome; it starts where the edit valve ends.

## 7. Transactions, undo and failure

**A run is one undo step, and a failed run changes nothing.** `command::host::run_as_step` runs
it against a session — the live one for an agent's `run_script` (§11), a working copy on a worker
for every run the person starts (*Off the UI thread*, below):

1. It ends the edit run (a run never merges into a nudge before it), marks the session, and holds
   the redo stack (`History::take_redo`), which each commit clears.
2. The host commits each step to that session as it comes (`EditorSession::try_commit`, the
   commit door, with what components owe), so later reads see earlier writes: positioning each
   child from the previous one works. The style browser's *Edit* is set aside for each commit, as
   for an agent, so a script's edit means the same whatever panel is open.
3. On success it joins the steps the run made into one with `History::join_last` (each inverse was
   taken after its own step, so the latest goes first), names it after the script, the bar's verb
   or *Edit*, with the source's glyph, and ends the edit run again.
4. On an error, a spent budget or a stop it takes back every step it made (`take_back`), rewinds to
   the mark and gives the redo stack back; a run that made no step gives it back too.

**Off the UI thread** (`command::running`). Enter in the bar, a script from the editor or the
palette, and an `/ai` reply's script run on a worker (`MastikApp::begin_run`), so the window keeps
drawing:

- **The copy is the document as it stood**: the document, its layout (`Resolved`) and an id source
  at the session's next id, copied when the run starts. The worker registers every face the window's
  text engine holds first (`text::registered_font_data`), so a family the script sets measures as
  it would on the UI thread, and makes a scratch session of the copy (`EditorSession::scratch`),
  so the run commits through the same commit door as the live session, every pass included.
- **The document is held while it runs** (`EditorSession::script_run`): every door that changes it
  — a commit, an undo, a redo, a jump in the history, a revert — refuses, and the status line says
  *“Card grid” is running — Esc stops it*; an agent's write is refused with the same reason. The
  canvas still pans, zooms, hovers and selects, and pages still switch: none of that is an edit.
  Text typed into a document script's code pane meanwhile stays in the pane and is stored once
  the run lands or stops. A count before `.` runs each repetition once the one before it lands.
  Holding is the rule rather than merging the person's edits with the run's, because a script
  measures the document as it goes (a layer placed from the one before it, a layout read back)
  and an edit landing mid-run would make its step mean something it never measured. The hold is
  short: a run is a few seconds at most, and *Stop* ends it at once.
- **The step lands whole**: the worker undoes its joined step once on the copy, and the redo is
  the step as the copy's door settled it, passes run. The live session commits that as it stands
  (`EditorSession::commit_settled`, as a redo applies), moves its id source past the ids the copy
  minted, and the report follows as before (the log, the status line, the editor's output, what
  the run made selected). A run is landed only when no gesture is live and no modal is up. A
  document replaced while the run was out (read again from disk) drops the run, which stops it;
  should the revision still differ when it lands, nothing lands and it fails as *the document
  changed while it ran*.
- **A run that ends within 50 ms lands in the frame that started it** (`LANDS_INLINE`), so a quick
  line reads as it always has, with nothing shown running. Past that, the status bar carries
  *Running “Card grid”…* (the seconds once one has passed, `MastikApp::running_said`) with *Stop*
  beside it, a `ui::word_link` as the exports' *Cancel* is; the bar's footer, for a line or an
  `/ai` script it started, reads *Running · Esc stops it* led by the loading spinner, the seconds
  on its right. The editor's *Run* and the palette's *Run ‹name›* rows are disabled meanwhile with
  the hold's sentence, and any other start is refused with it.
- **`Escape` stops it** (`tend_script_run`, read before every other key reader while a run is in
  flight, a modal aside), as does *Stop*: the stop flag is set, the run is dropped at once and
  nothing lands; the worker ends at its next step unseen (§8). The report is the door's: *The line
  stopped — nothing changed* (in the bar's error line while it still holds the line), *“Card grid”
  stopped — nothing changed* in the editor's output and the status line, the editor left as it
  is. A run is one at a time per document; a tab put behind keeps its run, which lands when the
  tab is in front again, and closing the tab stops it.

An agent's `run_script` stays on the UI thread, against the live session, under the held budget
(§8): the agent waits for its answer in the frame, and a run of its own is short.

A step is counted from its inverse for the status line and the log: a `DeleteNode` in it is a
layer made, a `CreateNode` or `InsertSubtree` one deleted, any other subject one changed, and a
layer both made and deleted is not counted. A script op counts as a script changed.

## 8. Sandbox and limits

The realistic failure of a script the person wrote is a runaway loop; scripts also travel (in a
document, a kit, a shared `.luau` file), so the hostile case is real too. It is answered by two
things together: nothing runs a script but the person, by hand (§10), and every run is inside this
sandbox. What is left is an escape through a bug in Luau or `mlua`.

- `Lua::sandbox` on; names off the allowlist (§4) do not resolve.
- **No filesystem, network, clock or unseeded randomness.** A run is deterministic, as replayed
  operations are (§12 of `foundations.md`). Work that needs files, such as one design in twenty
  brand variations, is the shell's: it runs `mastik run` once per variant with that variant's
  `--param` values and `-o`, and hands a picture to a batch as `add_image`'s base64 bytes
  (`export.md`). Randomness is **seeded**: `random(seed)` (a number or
  a string) answers a generator, `math.random`'s three forms, whose draws the seed fixes —
  `rng()` a fraction from 0 up to 1, `rng(n)` a whole number from 1 to n, `rng(m, n)` one from m
  to n — so scatter, jitter and generated patterns come out the same each run and a new seed
  deals again. A seed is required. The generator is the crate's own (`random.rs`, xoshiro256**
  seeded through SplitMix64, a whole number seeding as itself and a string by its FNV-1a hash), so
  the numbers a seed draws never change with a dependency (`random_draws_are_fixed_by_their_seed`
  pins one).
- **Budgets**, counted by `set_interrupt` (Luau calls it at every call and loop iteration; the clock
  is read every 256th, and at once after a verb or commit, which may be slow) and
  `set_memory_limit` above what the fresh state holds. Printed output lives outside Lua's memory,
  so a run may print at most 10,000 lines (`PRINT_LINES`) and 1 MB (`PRINT_CAP`, each line counted
  one byte more, so empty lines count too); `print` builds its line against the room left and
  measures each string before copying it. A verb's arguments are built as JSON there too, walked
  in Rust where no interrupt fires, and Lua shares a string or a table by reference where the host
  copies it at every one. So one call's arguments hold at most 100,000 values (`ARG_VALUES_MAX`,
  a collection counting each of its layers) and 16 MB of strings (`ARG_BYTES_MAX`), both spent
  before each copy: a table that shares a subtable at every level, or repeats a long string or a
  collection, is refused, not expanded. A verb of one layer, or a `.name`/`.text` write, hands its
  argument to the host once per layer, so its bytes count once per layer; and the clock and the
  steps are read between those per-layer host calls, which no interrupt separates. A budget with
  no clock charges host work to its steps instead (`Budget::host`: per layer read or written, per
  operation committed, per `play` call, and a fraction per table cell copied, layer handle built
  or layer passed over in a bare name's lookup); the others charge none. The
  editor's output pane lays out only the rows in view, and cuts a printed line past 2,000
  characters:

  | Budget | Time | Steps | Memory |
  | --- | --- | --- | --- |
  | `WORKER_BUDGET`: Enter in the bar, a script, an `/ai` script, on a worker | 30 s | 6,000,000,000 | 64 MB |
  | `RUN_BUDGET`: an agent's `run_script`, holding the UI thread | 1 s | 200,000,000 | 64 MB |
  | `READ_BUDGET`: the bar's live read, every keystroke | 20 ms | 500,000 | 8 MB |
  | `PLAY_BUDGET`: a prototype's *Run script* in play, which reads no clock (`play.md`) | none | 5,000,000, host work 1,000 a unit | 64 MB |

  A spent budget is `ErrorKind::Budget`, and the run is taken back (§7). A run on a worker no
  longer freezes the window, and the person stops it with `Escape`, so its time budget is only the
  backstop for a runaway left running, thirty times the held one; the steps scale with it, and the
  memory, which is what the copy cannot spare, stays.
- **A stop flag** (`mastik_vm::Leash::stop`) ends a run from another thread: the interrupt reads it
  at every step, and the per-layer host calls of one write read it between them, so the run ends as
  `ErrorKind::Stopped` at its next call, loop edge or layer. A host call under way (one verb) is not
  interrupted; it finishes, unseen. A run cannot call egui or re-enter the bar.

## 9. The log

**Every undo step is listed as the command that would make it** (`command::log`), so the person or
an agent learns the language by watching, and an edit with no command is a visible hole in what the
language covers.

- One log per document (`EditorSession::command_log`), for the session, never saved; it keeps 200
  entries and the bar shows the last six while its field is empty.
- It holds lines typed in the bar (with their result), script runs (`run "Card grid"`), and each
  step made anywhere else, watched once a frame and rendered from the step's stored inverse
  (`history::StepView::inverse`) against the document after it: a pure translation as
  `.x += 5; .y -= 2`, opacity, name, visible and locked; paints as `.fill` (one plain paint) or
  `.fills`, `.strokes` likewise, `.effects`, `.blend`, `.layout_item`, a layout's `.gap`,
  `.padding` and `.direction` when nothing else in it changed (else the whole `.layout`), the
  character properties that changed (`.font_size`…) and `.text_align` (`mastik_vm::set_by`, which
  reads each value before and after from the inverse, so no copy of the document is made). A text
  style change no property names is a hole unless a fill set on the same text explains it, since
  a `.fill` write clears the style's colour. The layers a step set must agree on each value. The
  person's step with anything else
  reads *— no command yet —* beside its name, a hole in what the language covers; an agent's
  (`Origin::Mcp`) is left out, since it teaches no one the language. A run that folds more into
  its step renders its line again. Every step since the watcher last looked is logged, oldest
  first, so two agents' calls answered in one frame are both listed; one whose layers a later step
  of that frame also changed is a hole, since the document after it is gone.
- **An `/ai` request is one entry** (`Source::Ai`), so the assistant's steps read as the request
  that made them. Sending it (`Log::asked`) logs `/ai ‹request›`, *Asked*, and holds the entry
  open: while open, any agent's step (`Origin::Mcp`) folds into it, taking its serial and the
  step's name as its result. A step names the agent, never the chat it came from (an applied
  proposal's names its client), so any agent's counts. The person's own step closes it (and is its own row),
  as does any other entry logged or a question the panel sends of its own (`Log::close_ai`). The
  reply's end (`Log::ai_replied`) sets the result, while no step has folded in, to *Script
  proposed*, *Edit proposed* / *Edits proposed* or *Answered*. Running the reply's script
  (`Log::ai_ran`) gives the open entry the script as its Luau, the run's result and its step; with
  no open entry for that line it logs a new `/ai` entry. The row wears `icon::SPARKLE` and reads
  in the typed line's ink.
- A right-click on picked rows: *Copy as script* (*Copy N lines as script*) and *Open in editor*
  (a new unsaved script). A hole is written as a comment naming its step, an `/ai` entry that ran
  no script as the comment `-- /ai ‹request›`. The menu takes `↑`/`↓` and `Enter` as a context
  menu does, so nothing is recalled behind it. Rows are picked by their place in the log
  (`log::Entry::seq`), not by index, so an edit logged while the bar is open, which drops the
  oldest entry from a full log, leaves the same rows picked.

## 10. Where scripts live

**A script may be stored in a document; it is never run by the document on its own.** No script
runs on open or re-runs on any change: a script the document re-runs is the constraint engine
`foundations.md` §13 decision 1 forbids, however small the convenience. Opening a shared document
therefore runs no one else's code, and there is no warning on open: a run the person starts is
inside §8's sandbox, and warnings train people to click *Allow*. A prototype's *Run script* is such
a run: it fires only from a pointer or key trigger the person acts on while playing, never from a
frame opening or a variable changing, and it writes the player's copy, not the document
(`play.md`). A stored script is a tool shipped beside the work; the
document records what its run committed and never needs it.

| Place | Kept | Edited |
| --- | --- | --- |
| **Unsaved** | the editor's list, for the session | in place |
| **Library** | `.luau` files in `prefs::folder()/scripts` (`scripts::library`), shared and git-tracked as files | written 600 ms after typing rests |
| **This document** | `Document::scripts()` (`mastik_core::script`), saved as the optional `scripts` key (`io.md`) | each change a `SetScriptSource`, typing folding into one *Edit script* step |
| **Kit · name** | the kit's published state (`kit::freeze` carries its scripts), listed from its latest publish | read-only |

- **The library folder.** A script's name is its file's stem. A name finds the file with exactly
  that name, else the one file whose name differs only in case, as Windows and macOS compare file
  names; where a case-sensitive folder holds several (`Grid.luau` and `grid.luau`) and none exact,
  the name is refused rather than guessed. The editor follows, renames and deletes the open
  script only by its own file's exact name, so once that file is gone a neighbour differing in
  case alone is never taken for it. A `.luau` file that is not UTF-8 text is not listed, but
  its name stays taken: nothing is saved, renamed or written over it. So is any other name the
  system resolves to something the library does not list (a symlink, a directory, a name it
  compares differently): a name with no listed script is only ever created, never written
  through. `check_name` refuses a name
  that is not one plain file name on every platform, and a name ending `.d`. Writes are atomic, and
  a pending write is flushed on exit. A delete asks first (*Delete script?*), since it removes the
  file with no undo. `mastik.d.luau` (§10, *Types*) sits beside the scripts and is never listed as
  one.
- **A library script changed outside Mastik** (an outside editor, a `git pull`) is read again
  whenever the folder is (the window opening, the palette opening, a run), and while it is open as
  the open document's file is (`library::disk::Watch`: every 45 s and when the window gets the
  focus back). With no typing waiting to be written, the pane takes the file's text and the status
  line says *Reloaded “Grid”: it changed on disk*, as for a clean document. With typing waiting,
  nothing is written over the outside edit: the typing is saved as a new library script beside it
  (*Grid 2*), which the pane then shows, and the status line says so. The pending write at exit
  does the same, so neither text is lost.
- **Document scripts** change through `AddScript`, `RemoveScript`, `SetScriptName` and
  `SetScriptSource` (the two setters fold into an edit run) and the verbs `add_script`,
  `set_script`, `remove_script`.
- **Copies between a document and the library, never links**: *Save to library* and *Copy into this
  document* copy, and a script of the same name in both is listed twice. **A kit's scripts are the
  one link**: they follow the kit's publishes as its components do, the disclosure covering each
  changed text. A change to them is worth a publish (`kit::Changes::scripts`), and the publish
  popover says so. The exception is a kit script a vendored main's *Run script* names: play runs
  document scripts only, so vendoring and each update copy it in as a document script
  (`kit::carry_scripts`, `kits.md`). A library script is never copied in on its own; *Copy into
  this document* is how a reaction comes to name one.

**The first-run disclosure.** Before a document's or a kit's script first runs on this machine, a
box says who wrote it and what a script can and cannot do, and shows its source laid out as Luau
reads it (`mastik_vm::as_read`): a lone carriage return breaks the line, as it ends a comment for
Luau, and a control or zero-width character, or a direction control, shows as its `\u{…}` escape,
so no code hides on a comment's line or behind an invisible character. It is recorded in
the preferences (`Prefs::script_disclosed`), never in the file, so a sender cannot mark a script as
shown: the SHA-256 of the source shown, keyed by `prefs::disclosure_key` (the document's or kit's id
and the script's). A changed source is shown again. The person's own scripts (library, unsaved)
are not disclosed, and neither is a document script's text the person typed in the editor or
copied into the document. The form and the disclosure are modal: the keymap waits while they
are up.

**The header** is the run of `---` lines a script opens with: the first is its title, the rest its
description, and each `---@input name type = default -- Label {lo..hi}` an input Mastik asks for in
a form before the run, read in the script as `param.name` (an unknown one raises). The MCP
`run_script` takes the values as `params` and `mastik run` as `--param name=value`, each read as
its default is written and held to its range (`Header::params`); an input left out keeps its
default, and a name the header does not declare is refused. Types: `number`,
`integer`, `boolean`, `string`, `color` (`"#rrggbb"`, or `"#rrggbbaa"`), `choice(a, b, …)`. A missing default is the
type's zero (the range's low end, the first choice); only a number or an integer takes a range. A
header that does not parse fails the run at its line. The form draws each by its type: a
`ui::value_field`, a `ui::colour_text_field` (reading any colour `colour_text::parse` does, and
handing the script `#rrggbb[aa]`; *Run* refuses text that is no colour), a `ui::segmented` for up to three choices and a
dropdown past that, a `ui::switch_row` carrying the input's label, a `ui::text_field`. Values are
remembered per script for the session.

**The run.** Disclosure if owed, then the form if the header has inputs, then the run (§7).
**A syntax error holds every run**, before the disclosure or the form (`start_script_run`): Luau's
parser refuses the text, so a run could only fail. The status line says *Card grid has a syntax
error on line 4*, and when the editor is open on that script the caret goes to the error. No other
problem the check finds holds a run. The run itself is on a worker (§7, *Off the UI thread*).
Once it lands the status line says *Card grid · 6 layers changed*, *Card grid failed at line 4 —
nothing changed* or *Card grid stopped: … — nothing changed* (a spent budget); a failure opens the
editor on the script with its line marked. A run the person stopped says *“Card grid” stopped —
nothing changed* and leaves the editor as it is.

**The editor** (`command/editor.rs`) is in the app: *Tools ▸ Scripts…*, and the palette's *Scripts…*
and a *Run ‹name›* row per saved script. One floating, resizable window that leaves the canvas
live, placed as the dialog cards are (centred on the card lane, at `ui::dialog_top`) and movable
by its head: a list on the left (a
`ui::search_box`; sections *This document*, *Library*, *Kit · ‹name›*, *Unsaved*; *+ New script*,
a `ui::action_button`, and the folder button at its foot), the script's head (its name, where it
lives, its problem counts, the find and ⋯ toggles and *Run*), the code pane and an output pane
listing the last runs under timed rules, errors in the error ink. The list's rows are list rows (`MENU_ROW_H`,
`ui::list_row_ground`) in the palette's row layout, except that a row's note (*2 inputs*) keeps its
room and a long name is cut instead, shown whole in the row's tooltip
(`palette::row_texts_note_kept`); with no script open the pane is a
`ui::empty_state`. The body is laid out in fixed rects, so the window keeps the size it is given.
`Ctrl+Enter` runs, or puts the caret on the syntax error that holds the run. The loupe in the head
toggles find (`FieldButton::On` while it is open), as `Ctrl+F` from the code pane opens it. **Find
reads as the canvas Find, smaller**: its field (`find::field`) with *3 of 12* at its end (a warm
*0* for none), the previous and next arrows and ✕ beside it. Matches ignore case; each is washed in
the accent in the code and the current one ringed and scrolled into view. A new text starts at the
first match at or after the caret; `Enter` steps on and `Shift+Enter` back, wrapping, with the
keyboard kept in the field; `Escape` or ✕ closes find and selects the current match in the code.
The ⋯ menu, opened by the ⋯ (on only while it is the ⋯'s own menu) or by a
right-click on a list row, which opens that script first (`context-menus.md` §6.10): *Save to
library* · *Copy into this document* · *Duplicate*, a rule, *Rename* · *Show in folder* · *Delete*,
each left out where it can never apply. **Save to library moves an unsaved script** into the
library, open with its code focused, and leaves a document's or a kit's script where it is, copied.
**Rename types over the list row** (`ui::label_edit`, as a page's or a layer's): from the menu, a
double-click on the open script's row or `F2`/`Ctrl+R` on it; `Enter` and a press elsewhere keep
the typed name, `Escape` the old one. **The window holds the keyboard** from a press on it, or its
opening, until a press elsewhere (`Scripts::holds_keys`): the canvas takes no key meanwhile, only
undo, redo and save passing as past a focused button (`input::passing_keys`), and while the menu is
open it holds the keys as a context menu does, `Escape` closing it.
The code pane is `ui::code_editor`, inked from `mastik_vm::tokens` in `theme::code`'s syntax inks:
keywords, literals (numbers, `true`, `false`, `nil`), strings, the permitted standard library
(`math`, `ipairs`, and a listed member after `math.`, `string.` or `table.`, from `run::LIBRARY`),
and any other name right before a `(`; comments are `text::faint()` and Mastik's own names
`accent_300()`. Its
gutter's numbers stand on their lines' baselines. The pane reads as one text: a press in its air
under the text puts the caret at the end, and one beside a line number at that line's start. The
output pane's top edge drags (`ui::drag_edge`), from the code pane's least (80) down to the output's
head alone; the height is kept for the session.

**Editing keys** (`command/code_keys.rs`, pure edits of the text and a selection, taken out of the
frame's events before the text edit reads them and after the completion list has taken its own).
One indent step is two spaces; a line indented with tabs keeps them. `Enter` keeps the line's
indentation and goes a step in after a line ending in `then`, `do`, `else`, `repeat`, an open
bracket or a function's parameters, and between `(` and `)` puts the close on a line of its own.
Typing the last letter of `end`, `else`, `elseif` or `until` (or a lone `}`, `)`, `]`) as a line's
only word steps the line back to its block's depth, never deeper. `Tab` over several lines indents
them, otherwise it pads to the next step; `Shift+Tab` outdents the lines a selection touches.
`Ctrl+/` comments the lines with `-- ` at their least indentation, or uncomments them when all are
comments. `Alt+↑`/`↓` move the lines, `Shift+Alt+↑`/`↓` copy them. Brackets and quotes type their
close where one can stand (a quote not after a name, nor in a string or comment), a close typed
before the same character steps over it, a bracket or quote typed over a selection wraps it, and
`Backspace` between an empty pair takes both. `Ctrl+X` with nothing selected cuts the caret's line
with its newline (the last line takes the newline before it). A double-click held and dragged
selects whole words from the clicked one (`drag_words`).

**Types.** `mastik_vm::definitions` writes a `.d.luau` from the same tables as the bindings (the
`Layers` class, `sel` and the relative names, each verb with typed parameters, `param`), saved as
`mastik.d.luau` in the scripts folder when the editor opens (never from a headless app, whose
preferences are a throwaway), so luau-lsp in an outside editor completes and checks against it.
**In-app type checking is not built**: Luau's type checker is a separate analysis library that
`mlua` does not expose. Completion, the hint, hover and the check read the same tables and infer
no types: a property is offered after any `.`, whatever stands before it, and the check calls a
property missing only after an expression it can see holds layers.

**Completion in the editor** is `mastik_vm::complete_script` over the whole script, in a dropdown
hung `ui::MENU_GAP` under the word being completed (over it with no room below), drawing the bar's
rows (`command::completion_rows`). It offers the script's own names (*This script*: its `local`s,
functions, parameters and loop variables, and after `param.` the header's inputs), the language's
names, the verbs, *Luau* (the keywords and the permitted library, and a library's members after
`math.`, `string.` or `table.`, `play`'s members after `play.` and the variables after
`play.vars.`), and the page's layers; properties after `.` and methods after `:`.
A row's side note is its one-line doc; the library's members' and the permitted globals' come from
`run::LIB_DOCS`, a line and a signature for each `LIBRARY` member and `STDLIB` function, held to
those lists by `every_library_member_has_a_help_line`. Inside a choice parameter's quotes it offers
that parameter's words with their quotes (*Choices*), and at a key of a verb's table argument the
names not yet written. Leap labels are left out, since they last a session, and so are the bar's
bare argument words. It offers nothing else in a string, nor in a comment, on a number, or where a
name is being declared (after `local`, `function`, `for`). It opens and shuts as the bar's list does, by one
`command::suggest::Suggester`, and only while the code has the focus: `↑`/`↓` move, `Tab` takes
the lit row and `Enter` too unless the word already reads as it, when `Enter` breaks the line;
`Escape` shuts it for the word; `Ctrl+Space` opens it.

**Language help** (`mastik_vm::help`, drawn by `command/script_help.rs`) reads the tables the
runtime binds, so it cannot disagree with a run. Its cards are `ui::menu_frame(ui::POPOVER_PAD)`,
sized from their laid-out text each frame.

- **The parameter hint** (`signature_at`) is the call the caret is in: the nearest unclosed `(`
  whose callee is a verb, a collection method after `:` or a function the script declares (its
  first declaration), or a verb's table argument (`verb { … }`, `verb({ … })`). The argument is
  counted by top-level commas. A verb's subject (its `Node`/`Nodes` parameter) is left out, and a
  verb that takes layers adds *Acts on the selection.* to its doc. Lines: the signature in mono, the
  lit parameter `accent_200()` underlined in `accent_300()`, types faint; the lit parameter's
  `name · doc`, *No more arguments* past the last, or for a table *Named: dx is left.* / *All
  given.* with nothing lit; the doc, dim; for the script's own function, names only and *This
  script's function · line N*. None inside a comment. It hangs `ui::MENU_GAP` over the line the
  call opens on, its left edge at the bracket; with no room above, under the completion list or the
  caret's line. As in other code editors it opens on typing, never on a click or a caret moved
  into a call: typing the call's `(`, a `,` or a `{` opens it, as does `Ctrl+Shift+Space` in any
  call, and it stays while the caret is in that call and the code has the focus. Leaving the call
  or `Escape` with no list up shuts it. egui lets go of a focused field on `Escape` before any
  widget reads it, so when the list or the hint takes the key the editor hands the code its
  keyboard back.
- **Hover** (`hover_at`): once the pointer rests on a name (`ui::Resting`), a card under it
  (`ui::dropdown_place`, taking no pointer) says what it is, the word washed in the accent: a
  property (`card.opacity: number`, its doc, *Property · reads raise when the layers disagree*), a
  method, a `param.x` input (its type and default, its label, *This script's input, line N*), a
  library member or global (*Luau library*), the language's names (*Name*), a verb (*Verb*), the
  script's functions, or a layer named on the page (its kind glyph, *Frame on Home, 1440 × 820 ·
  outlined on the canvas*, outlined there as a hovered Layers row is; several of that name add that
  they raise at the run). Nothing for keywords, numbers, strings, plain locals or a local that
  shadows a layer, and no card while the completion list is up or `Ctrl` is held.
- **Problems** (`check`), found without running. If Luau's parser refuses the text, the syntax
  error alone, on the token after *got '…'* outside strings and comments or else the line's text
  (`<eof>` on the last line with text), a text that stops short in plain words (*Missing `end` to
  close the `do` on line 1*). Otherwise:

  | Ink | Problem |
  | --- | --- |
  | warning (`color::warn()`) | a name not declared anywhere in the script and not the language's, a verb, the permitted library or a layer on this page: *No name “x” — did you mean y?*, noted *Not a local, a library name or a layer on this page* |
  | error (`color::error()`) | a property no layer has, after an expression that holds layers (a collection name, a layer name, a local or loop variable over one, a `Layers` property, an index of one); a missing member of `math`, `string` or `table`; a missing `param` input; an assignment to a global or a non-local named function (*No global “x” — write `local x = …`*) |

  Unknown names are only warnings because a layer name resolves at the run, on the page the run
  sees. A field of the script's own table is never checked, nor a name in a type annotation (after
  a declared name's `:`, a function's return `:` or `::`); a typed name in a parameter or `local`
  list is declared as an untyped one is. The check runs once typing has rested
  `script_help::CHECK_AFTER` (600 ms, the library write's delay) and at once on opening a script;
  nothing is marked while the text differs from the text checked, and an edit to the document or a
  page switch with the text unchanged checks again at once, since the check reads the page's layer
  names. Nor is a name's problem marked under the caret (a syntax error is). `ui::code_editor` takes `ui::CodeMark`s: each inks its line's number and
  squiggles its span (`canvas::paint_squiggle_in`); a failed run's line is one more mark in the error
  ink. Resting on a squiggle or its line number shows the problem's card (the warning glyph in its
  ink, the message, a *Change to y* `ui::label_button` that edits the text, the note); it takes
  the pointer, stays while the pointer is on it and hides the hint. The head's counts (the warning
  glyph, in the icon font, in each ink with its count, each only when not zero, on an icon
  button's hover ground) say *1 error, 1 warning · click goes to the next*, and a click selects
  the next problem after the caret, wrapping. While a syntax error holds the run, the head's *Run* is disabled (*Fix the syntax error
  on line N to run · Ctrl+Enter puts the caret on it*), as are the palette's *Run ‹name›* rows.
- **Go to declaration** (`declaration_at`): the nearest declaration above the use, else the first
  (a function called above it); `param.x` goes to its `---@input` line. With `Ctrl` held over such
  a name it takes a dotted `accent_300()` underline and the arrow in place of the text's I-beam
  (never the hand, as nowhere in the chrome); `Ctrl`+click, or `F12` at
  the caret, selects the declaration and washes its line in the accent, fading over 600 ms,
  scrolling only when the line is out of view, to a third of the way down. Mastik's names, the
  library and keywords never underline, and neither does a declaration itself, which leads nowhere.

## 11. MCP and `/ai`

Reads stay typed tools, and so do single edits, for validated arguments and an error per call. A
document's stored scripts are written through `add_script`, `set_script` and `remove_script`
(`mcp.md` §8.4); MCP runs none of them. Bulk work goes through `run_script(source, nodes?,
layers?)`, which runs Luau as the script editor does (`run_as_step`, under `RUN_BUDGET`, no line
sugar), so a loop over many layers is one call and one undo step marked as the agent's: `nodes` is
`sel`, and `layers` binds names to layers as leap labels are bound. It answers what the run printed
(the last 100 lines), the layers it made and what it changed, leaves the person's selection alone,
and with `base_rev` is taken back whole when a layer it wrote changed since. `/ai` turns the
other way: the person asks the assistant from the bar, and applies what it proposes or runs the
script it wrote from there (*The command bar*).

## 12. Non-goals

In `non-goals.md`, *Command Mode*.

## Pick (`navigate.rs`)

**The bar has two focuses.** In **Type** (`:`) its field has the keys. In **Pick** the canvas has
them: `` ` `` from the editor or the palette's *Pick layers* opens the bar
on it, and `` ` `` switches to it from Type unless the caret is inside a string, where it is Luau's
own backtick. Picking belongs to the bar because it serves someone who has left the mouse to type
and must name layers without it; on its own, a keyboard walk never beats the mouse in a design
tool. It is a keymap, not a canvas state: `MastikApp::mode` stays `Normal`, and `keymap_mode` hands
`input::resolve` `Mode::Navigate` while the bar picks. Its sign is the bar's head row.

Opening the bar on either focus switches the canvas to the Select tool; closing it keeps the
selection and gives back the tool from before, unless another was chosen by hand meanwhile or a
text edit took the keys. A text edit started under the bar (a double-click on text) closes it.

| Key | Act |
| --- | --- |
| a label's letters | select its layer; `Shift` on the last adds it. Of two-letter labels the first narrows and `Backspace` takes it back (`leap_keys`, read before the table) |
| `1`–`9`, then `0`–`9` | a count, at most 99: the next arrow runs that many times, stopping at the last layer that way, and `.` repeats the line that many times. `0` starts none. Any other key spends it; `Backspace` drops its last digit |
| `/` | the name filter: what is typed next (every key, home row included) keeps only the layers in view inside the scope, at any depth, shown and unlocked, whose name as drawn holds it, case aside (`refilter`), each given a label if it had none. `Enter` on one match selects it; on several it hands the letters back to pick among them, and `/` types into the filter again. A label picked ends it; `Backspace` on an empty filter or `Escape` clears it |
| `m` + a letter | mark: the page, the view, the selection and the entered group under the letter (`navigate::Place`), kept with the document for the sitting (`EditorSession::marks`) and dropped when another document is read into it |
| `'` + a letter · `''` | go to the mark, leaving where things stood for `''`, which goes back to before the last jump and makes that the way back again. A mark whose page is gone says so; an unset letter says *No mark ‹L›* |
| `Enter` | into the one selected layer when it holds others (group, boolean, frame, instance root), by `enter_container`, on its first child in reading order (not the topmost in z-order the editor's Enter lands on); from anything else, back to Type |
| `Backspace` | out: with a group entered, select it and enter the rung above it (`group_chain(doc, g, None).last()`); else select the current layer's parent |
| `↑` `↓` `←` `→` | the nearest sibling that way on the canvas (`nearest_toward`): of the siblings whose centre lies past the current layer's that way, the least gap along the arrow plus twice the gap across it, so one in line beats a nearer one off to the side. The canvas, not the Layers panel's z-order, which often runs against what the eye sees. Nothing that way stays put; an arrow onto a group selects it without stepping in. With nothing selected, any arrow takes the scope's first layer in reading order. `Shift` adds |
| `*` (`Shift+8`) | the same layer in each repeat (`select_matching`) |
| `Shift+1` / `Shift+2` | zoom to fit / to the selection |
| `.` | run the last line in the log again as written (`Log::last_line`): a line that opens with a label acts on that label's layer again, as Enter did, and any other on the selection. The line is typed, or written for an edit made elsewhere, such as a drag's `.x += 24`; a script's run is no line, and neither is an `/ai` request, which would ask again |
| `` ` `` `:` `Escape` | back to Type; `Escape` first clears anything half typed (a count, a mark's prefix, the filter, a label's first letter) |

Every other editor key rests; the history chords and the cheatsheet's stay, and opened from Pick
the cheatsheet's *Pick layers* section leads.

**One pick, written as it goes.** A pick is the same wherever the caret is — the line's start or
mid-line: labels, arrows, `Enter`, `Backspace` and clicks on the canvas move and select as above.
Once the selection has moved off what it was when the pick began, the current layer's label stands
in the line at the caret the pick began from, replaced as the pick moves on (`pick_into_line`), so
the field shows the pick as a pill in real time; `` ` `` ends the pick with it there. So `` ` ``,
a label, `` ` ``, `.x = `, `` ` ``, a label, `` ` ``, `.x + 50` writes *FJ.x = DK.x + 50*.
**The label that opens a line is its subject** (`line_subject`): a bare `.x` anywhere in it, and a
verb's subject, mean that layer, however the selection moves after; a line that opens with no
label acts on the selection.

**The deal** is made when the bar opens and kept for its whole time open (`deal_tick`). It deals
again at once when the page or the entered group changes, and once the view or the document has
rested 0.15 s after moving (`REST_S`), never mid-press, so a drag ends before its layers are
relabelled. The candidates are what a plain click could reach in view, inside the entered group
when there is one (`leap_candidates`): hit tests sampled across the view and at each layer's
centre, each raised by `group_chain` as a click is. More than 81 label only the current layer's
children (its parent's when it has none), those in view.

**A label belongs to its layer.** Labels are always two home-row letters, easiest first
(`label_pairs`), and unique across the document. A layer meeting its first deal takes the easiest
free label, the newcomers in reading order, and keeps it for as long as the document is open —
through pans, zooms, entered groups, edits and the bar closed and reopened — so a label never
changes under the eye and a line can name layers from different frames and groups. One length
always is the price of that: mixed one- and two-letter labels cannot share a deal (`f` would
shadow `fj`), so a count crossing nine would relabel everything. Only when all 81 are taken does a
label move (`free_label`): a deleted layer's first, else that of the layer seen longest ago that is
out of view and not named in the field's line. `label_for` gives a label to a layer the deal did not
reach.

**What is drawn.** In Pick, every label. In Type, those the completion offers for the word at the
caret (a word of at most two home-row letters), else only the label of the layer under the pointer,
so it can be named. Typed letters keep outlines only on the labels they still match and fade the
rest. Each labelled layer is outlined on its bounds, outset 3: dashed with the dark label for a
group, boolean or frame, solid with the yellow label for any other layer, so *the group or the layer
inside it* is told by line and lightness, not hue. Every line is two inks, `LEAP_INK` inside and
`color::leap()` outside, so it reads on any artwork. **What is picked is blue**: a selected layer's
label and the outer line of its outline wear `color::SELECT`, the canvas's selection blue, in place
of the yellow or the dark label, so the layer a label just took stands apart from every other label
at a glance. In Pick the label of the layer under the pointer wears a ring in `color::SELECT`. They are egui chrome painted in `draw_canvas` over
everything a layer wears, not a `RenderOverrides` overlay.

## The command bar (`command/mod.rs`)

`:` opens it on Type (`Key::Colon`, shifted or not) from the editor whenever no text entry has the
keyboard, closing the palette and any menu; it needs no mode of its own. It is a sibling of the
palette's card: 640 wide, docked 24 above the canvas's foot always — it never jumps away from
the selection, which pulls the eye; the canvas pans to reveal what it covers. **It is a plain foreground `egui::Area`, not a modal**: the canvas under it
pans, zooms, hovers and selects, and a click there never moves the keys or closes the bar, so the
mouse moves and points while the keys name and run. It is placed from the height it works out that
frame, not anchored, since an anchor reads last frame's size and a growing list would push the foot
down a frame early; the list is drawn into exactly that height, and the area is not constrained to
the screen, since egui constrains by last frame's size and would shove the card up on the frame its
list shrinks. Its rows, section labels and footer are the palette's (`app-chrome.md`, *Shared
rules*): a row's name is never cut for its note.

- **The head row**, above the field (`CONTROL_H` in the card's inset, a rule under it): the focus as
  one icon toggle, a card head's action icon (`ui::icon_button`, `HEAD_ACTION_W`) wearing
  `icon::LEAP_LABEL` — a layer's outline with its label on the corner, what Pick draws — on while
  the bar picks and a click switching focus, so the row gives its width to the path; the path from the page to
  the current layer's parent, and on the right the subject with its kind's glyph (*Card*, *Card and
  2 more*, *Nothing selected*), so it is plain which layer a line acts on and where it sits.
- **The field**: the card's whole width from the head row's inset, with no prompt column;
  monospace at the palette field's size (`HEAD_PT`), a read's live value (`= 20`) at the right
  edge, and *Pick a layer and type a command* while empty. **Pills** say which layers the line
  names, in the leap labels' own look (`ui::pill_shapes`): a free name that is a label
  (`mastik_vm::marks`) is drawn as its pill, the subject's opening the line like any other, except
  the word the completion list is open on, which is still being typed. Each `.` rule 2 reads as the
  subject wears the subject's pill before it (its label, `SK +2` for a selection of more), so
  `fj.x = dk.x - .x` reads *FJ.x = DK.x − FJ.x*. The field's layouter lays a label's hidden letters
  out exactly as wide as its pill, so the caret at either end of a label stands at the pill's edge,
  never inside it. **Colour says which layer is which**: the subject wears the selection's blue,
  `color::SELECT`, as its label does on the canvas; every
  other layer named takes the next of `color::REFERENCE` (green, orange, teal, pink) in order of
  first mention, one hue per layer however often it is named, and is outlined on the canvas in that
  hue while the bar is open (`draw_line_references`), as a spreadsheet colours a formula's ranges.
  Hovering a pill names its layer and outlines it as a hovered Layers row; `Backspace` just after a
  label pill takes the whole label. The line's text stays plain Luau with the labels as words. In
  Pick it sleeps: the line is kept, dimmed and not focusable, the hint reads *Picking: type a
  label*, and the right edge shows in the leap ink what Pick has half typed (`pick_typed`): a
  count, `M` or `'` waiting for a mark's letter, `/` and the name filter, or a label's first letter.
- **The footer**: in Type, the desugared Luau, *each by itself* after a rule-3 line, and on the
  right *` picks*, or *Enter copies ‹value›* while a read shows one. An error replaces it with a
  red caption, the token it names squiggled, and *Tab fixes it* when it carries a fix. The token is
  found by the lexer, so a name or a syntax error's *got '…'* token in a string or a comment is
  never the one marked; a syntax error the token cannot place marks the line's text, and a line
  that stops short reads plainly (*Missing `)` to close `(`*, *Something is missing at the end*),
  as the editor's syntax error does. In Pick it
  lists the keys that mean something now (`pick_hint`: the filter's while it is open, a mark's
  while its letter is awaited), with *` types* on the right.
- **`Alt`-click** on a layer, in Type, writes its label into the line at the caret (a space first
  when it would run into the word before), leaving the selection alone: an argument by pointing.
  The press sits out to its release
  (`gesture_cancelled`), so it neither selects nor starts the `Alt` duplicate-drag.
- **The list above**, in Type only: the log while the field is empty, else completion, at most
  eight rows in sections *Names*, *Verbs*, *Properties*, *Labels*, *Layers*, *Choices* (a verb's next argument's
  words: a choice's words, and the names of the tables, variables, styles, scripts, modes or
  collections the argument takes, each with a note — *12 rows · 4 columns*, *number · Theme*,
  *fill style*, *3 lines*, *mode · Theme*, *4 variables · 2 modes* — the verbs taking a name in
  place of the id), the typed letters lit; an almost-match is a dim row naming what it meant. A whole-name
  match outranks a prefix, and a section whose best row matches better goes first, so a label or
  name typed out whole leads the list. The lit row's layer (a label's, or a layer's by name) is
  outlined on the canvas as a hovered Layers row is.
- **When completion shows** follows a code editor's suggest widget (VS Code's, Zed's): typing a
  name's letter (not a digit), `.` or `:`, or the space before a verb's argument opens it on that
  word; any other character shuts it; deleting keeps it while the word has letters (or sits right
  after a trigger); the caret leaving the word shuts it. `Escape` shuts it for that word, and typing
  on in the word leaves it shut; `Ctrl+Space` (Ctrl on every platform) opens it on the word at the
  caret. Taking a row shuts it. A list with no rows is not shown.
- **Keys**, in Type: `` ` `` picks, outside a string (*Pick*). While the list shows, `↑`/`↓` move
  through it, `Tab` takes the lit row and `Enter` takes it too unless the word already reads as the
  row, when `Enter` runs the line. Otherwise `↑`/`↓`
  recall typed lines and `/ai` requests, `↓` past the newest giving back what was being typed (a
  click on a log row also puts its line in the field, a script run's excepted); `Tab` takes an error's
  fix; `Enter` runs, on a worker under `WORKER_BUDGET` (§7, *Off the UI thread*): a read copies
  its value, a string without Luau's quotes so it pastes into a field as the text, and makes no
  step; a write is one step named after its verb, else *Edit*, with `TERMINAL_WINDOW`. It reports
  where it landed: the field clears if it still holds the line, a failed line stays in the field
  with its error, and a line no longer in the field reports on the status line. `Escape` closes the
  log's menu if it is open, else the list, else clears the text, then closes; while the bar's run
  is in flight it stops the run instead.
- **`/ai`**: a line reading `/ai` and a request (`ai_request`; `/aim` is not one) is a request to
  the assistant: an edit, a question, or a script. It is not Luau: no completion, no live read, and
  the footer reads *Enter asks ‹speaker›*. Typing the line starts the assistant program's lookup
  (`assistant_blocked`), so `Enter` seldom waits for it.
  - `Enter` sends it into the panel's chat (`assistant_ask_bar`, `assistant.md` §6), so the
    harness, session, sign-in and access level are the panel's: at *Propose* an edit comes back as
    proposals. The model is told to act with its tools as in the panel, or, when asked for a
    script, to answer with one `luau` block and change nothing; either way to close on one plain
    sentence under 80 characters of what it did or what the script does (`prompt::bar_request`).
    The selection goes as chips, and each leap label the request names is told to the model as the
    layer id it stands for, which a script reads by that label. A bare `/ai` says *Type a request
    after /ai*.
  - While the program is still being looked for (`Asked::Looking`), the bar holds the request
    (`Ai::Waiting`, footer *Looking for ‹program›…*, right *Esc clears*, led by the turning
    loading spinner in the dim ink) and asks again each frame,
    sending once it is found; the lookup starts only on the first ask, so refusing here would leave
    the bar stuck. Any other refusal shows on the error line (`Why::sentence`).
  - Once sent, the request is logged (§9). While the reply runs (`Ai::Asking`), `Enter` waits and
    the footer, led by the same spinner, reads *Asking ‹speaker› · the reply shows in the assistant
    panel*. The reply is found by its place in the thread, so it counts only while
    the message before it is still this `/ai` request: after a new chat, or with another
    document's thread in front, the error line says the reply is gone.
  - When the turn ends, `bar_reply` sorts it (`BarReply`), each carrying the reply's last sentence
    (for a script, the last one before its block), a trailing colon trimmed:

    | Reply | When | List row (`reply_row`) | Footer · right | `Enter` |
    | --- | --- | --- | --- | --- |
    | *Proposed* | a tool row carries a proposal id; checked first | the sentence, else *N proposed edits* | *Enter applies it/them · Tab opens the assistant panel* · *Esc clears* | applies each proposal as its own step through `assistant_apply`, as from its card (a stale one says so on the status line), then clears the line |
    | *Script* | the last `luau`/`lua` block, else a lone unlabelled one, from a reply that wrote nothing (no done call outside *Look*'s tools; `call` judged by the tool it ran) | one line shows its code in mono; a longer one the names its code calls and sets (`mastik_vm::names_used`) in mono, its sentence (else *N lines*) as the faint note; with no such names, the sentence alone | *Enter runs it as one step · Tab opens it in the editor* · *N lines*; at *Look*, *Enter opens it in the editor · the chat can only look* | runs it (below); at *Look*, opens it in the editor as `Tab` does |
    | *Reply* | anything else: edits it made itself, or an answer | the sentence, else *Done: the reply is in the assistant panel* | *Tab opens the assistant panel* · *Enter clears* | clears the line |

    A script is only taken from a reply that changed nothing, since one that edited has done the
    work and its code block shows how. A script runs on one key with the person's write access, and
    its sentence is the model's own account, which layer names or text it read could steer: so the
    row leads with names read from the code, and a chat at *Look*, which was given no right to edit,
    never has its script run from the bar; the person runs it from the editor, having read it.
    The list shows the one row under its section label
    (`reply_rows`), wearing `icon::SPARKLE` in the log's glyph ink; the editor and the panel are
    where the whole reply reads. While proposals wait to be applied, the footer leads with
    `icon::SPARKLE` in `color::warn()`; a script or a reply has no mark (`footer_marked`, `Mark`).
  - A script runs as a script runs (`run_ai_script`: on a worker, no line sugar,
    `WORKER_BUDGET`) on the selection, the bar's labels bound: one step named by the request (cut
    at 40 characters), wearing `icon::SPARKLE` and marked `Origin::Mcp { client: ‹speaker›, tool:
    "/ai" }` before it lands, as a proposal's Apply is, since the person runs what the agent wrote.
    The log gives it to the request's entry (`Log::ai_ran`, §9). A raise takes the run back (§7)
    and shows on the error line without a span, since the script's place is not the field's; the
    script stays proposed.
  - `Tab` closes the bar and opens a script in the editor as a new unsaved script, or any other
    reply in the assistant panel. Editing the request drops the request and its reply; its turn
    goes on in the panel.

## Traps

- **A live read is held read-only twice**: `run::Mode::Read` refuses every write and verb inside
  the runtime, and the bar's `SessionHost { read_only: true }` refuses `verb` and `commit`. Either
  alone looks sufficient; the second is what keeps a runtime bug from editing on a keystroke
  (`a_read_shows_its_value_and_writes_nothing`).
- **`join_last` puts the latest inverse first**: each inverse was taken against the state after its
  own step (`joined_steps_undo_and_redo_as_one`).
- **A failed run must take back every step it committed**, or a raise after a write leaves half a
  run in the document (`a_line_that_fails_changes_nothing`).
- **The disclosure record is a cryptographic hash of the source**, because the key and the source
  both come from the sender (`a_script_is_disclosed_per_key_and_source`).
- **An `Alt`-click under the bar must cancel its gesture** (`bar_alt_click` sets
  `gesture_cancelled`): the label is written either way, but without it the same press also selects
  and starts an `Alt` duplicate-drag (`an_alt_click_writes_the_label_into_the_line`).
- **The syntax hold compiles the text as it is now**, never reading the check's problems: those wait
  for typing to rest, so a run started inside that wait would get through
  (`a_syntax_error_holds_every_run`).
- **A landed run moves the session's id source past the copy's**: the copy minted the step's
  layers from the session's next id, so without `skip_to` the next layer the person makes takes an
  id the step already used (`a_run_lands_as_one_step`).
- **The hold and the revision check are two defences**: the hold keeps every commit out while a run
  is in flight, and the revision check catches what bypasses the commit door (a document read
  again). Either alone looks sufficient.
