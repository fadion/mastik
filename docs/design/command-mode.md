# Command Mode (vm.md §1–§12)

Command Mode is the keyboard's way into the document: **Navigate** walks the layer tree and deals
leap labels, the **command bar** runs one line of the scripting language against the selection, and
**scripts** are whole programs in that language, kept in the library folder, in a document or in a
kit. The language is Luau in the headless crate `mastik-vm`; the app's half is `command/` (the bar,
its log, the host, the script editor), `navigate.rs` and `scripts/` (the library folder). The
decisions it rests on are `foundations.md` §13. Headings keep `vm.md`'s numbers, so a `vm.md §4`
cited from code is found here; `vm.md` keeps only what is still open.

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
labels, a script's input values), `Outcome` (printed lines, a read's value, layers created, the
selection to set), `VmError { kind: Error | Budget, message, line, span, fix }`, `desugar`,
`eval_read`, `run_line`, `run_script`, `header`, `complete`, `tokens`, `names_used`, `definitions`,
`PROPERTIES`, `RUN_BUDGET`, `READ_BUDGET`.

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
   by `=` or a compound operator, is a call; the other words are its arguments. Numbers, `true`,
   `false`, `nil` and quoted strings stay themselves; any other word becomes a string. `align left`
   is `align("left")`, `duplicate 3` is `duplicate(3)`. **Names are the registry's, with no
   aliases**: `dup 3` is not a verb and runs as written.
2. **Subject elision.** A `.name` in prefix position (the line's start, after an operator, an
   opening bracket, a comma or a keyword that a value follows) is `sel.name`: `sk.x = .x + 5` is
   `sk.x = sel.x + 5`.
3. **Compound assignment adjusts each layer by itself.** `<expr>.<prop> op= <rhs>` is
   `<expr>:adjust("<prop>", by)`: `+=` adds, `-=` adds the negation, and `*=`, `/=`, `//=`, `%=`,
   `^=` map each layer's own value through a function. Each statement on the line is rewritten by
   itself: `<expr>` starts after the `;`, comment or block keyword before it and `<rhs>` ends at
   the one after, so `.x += 5; .y -= 2` and `for … do l.x += 5 end` both work. The footer says
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
3. **Leap labels**, two lowercase letters, bound to the layers Navigate last dealt them to. A label
   whose layer was deleted raises.
4. **Registry verbs**, by their registry names.
5. `print` (to the bar's result or the editor's output), `percent`, `math` without `random` and
   `randomseed`, `param` (a script's inputs, §10), and the allowlist `string`, `table`, `ipairs`,
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
| `opacity`, `corner_radius`, `name`, `visible`, `locked`, `text`, `blend` | the layer's own | the matching verb |
| `fill`, `stroke` | the one solid paint as `"#rrggbb"` (`"#rrggbbaa"` when not opaque), nil for none; anything else is no one value | `set_fills` / `set_strokes`; a text's style colour is cleared so the fill shows |
| `id`, `kind`, `parent`, `children`, `prev`, `next` | read-only | — |

A name people guess (`alpha`, `colour`, `radius`) raises with the property it means as the fix.

**A verb call**: a collection given first fills the verb's subject (its first layer parameter),
else the selection does; a verb of one layer runs once for each; the remaining arguments fill the
other parameters in order, and one plain table is the named form. It answers what it created as a
collection, else the id of what it made, else nil. What the last creating verb made, or what
`:select()` named last, is selected when the run ends.

## 6. What is bound, and what is not

**Bound:** every registry verb, the properties through the world-space facade, and the selection.
The host lends no image sink and no contrast probe, so the verbs that need one (`add_image`,
`fix_contrast`) refuse, and it refuses a write to a kit's vendored component, which changes only
when the kit does.

**Not bound, by design:** gestures, tools as interaction, previews, `RenderOverrides` and egui
chrome. A script can only say a committed outcome; it starts where the edit valve ends.

## 7. Transactions, undo and failure

**A run is one undo step, and a failed run changes nothing.** There is no working copy:

1. `command::host::run_as_step` ends the edit run (a run never merges into a nudge before it) and
   marks the session, and holds the redo stack (`History::take_redo`), which each commit clears.
2. The host commits each step to the live session as it comes (`EditorSession::try_commit`, the
   commit door, with what components owe), so later reads see earlier writes: positioning each
   child from the previous one works. The style browser's *Edit* is set aside for each commit, as
   for an agent, so a script's edit means the same whatever panel is open.
3. On success it joins the steps the run made into one with `History::join_last` (each inverse was
   taken after its own step, so the latest goes first), names it after the script, the bar's verb
   or *Edit*, with the source's glyph, and ends the edit run again.
4. On an error or a spent budget it takes back every step it made (`take_back`), rewinds to the
   mark and gives the redo stack back; a run that made no step gives it back too.

A step is counted from its inverse for the status line and the log: a `DeleteNode` in it is a
layer made, a `CreateNode` or `InsertSubtree` one deleted, any other subject one changed, and a
layer both made and deleted is not counted. A script op counts as a script changed.

## 8. Sandbox and limits

The realistic failure of a script the person wrote is a runaway loop; scripts also travel (in a
document, a kit, a shared `.luau` file), so the hostile case is real too. It is answered by two
things together: nothing runs a script but the person, by hand (§10), and every run is inside this
sandbox. What is left is an escape through a bug in Luau or `mlua`.

- `Lua::sandbox` on; names off the allowlist (§4) do not resolve.
- **No filesystem, network, clock or randomness.** A run is deterministic, as replayed operations
  are (§12 of `foundations.md`).
- **Budgets**, counted by `set_interrupt` (Luau calls it at every call and loop iteration; the clock
  is read every 256th, and at once after a verb or commit, which may be slow) and
  `set_memory_limit` above what the fresh state holds. Printed output lives outside Lua's memory,
  so a run may print at most 1 MB (`PRINT_CAP`):

  | Budget | Time | Steps | Memory |
  | --- | --- | --- | --- |
  | `RUN_BUDGET`: Enter in the bar, a script | 1 s | 200,000,000 | 64 MB |
  | `READ_BUDGET`: the bar's live read, every keystroke | 20 ms | 500,000 | 8 MB |

  A spent budget is `ErrorKind::Budget`, and the run is taken back (§7).
- **A run happens on the UI thread, in the frame that starts it, holding the session**: no progress,
  no stop key, no yielding across frames. It cannot call egui or re-enter the bar. The budgets are
  what stops a runaway.

## 9. The log

**Every undo step is listed as the command that would make it** (`command::log`), so the person or
an agent learns the language by watching, and an edit with no command is a visible hole in what the
language covers.

- One log per document (`EditorSession::command_log`), for the session, never saved; it keeps 200
  entries and the bar shows the last six while its field is empty.
- It holds lines typed in the bar (with their result), script runs (`run "Card grid"`), and each
  step made anywhere else, watched once a frame and rendered from the step's stored inverse
  (`history::StepView::inverse`) against the document after it: a pure translation as
  `.x += 5; .y -= 2`, opacity, name, visible and locked. Anything else reads *— no command yet —*
  beside the step's name. A run that folds more into its step renders its line again.
- A right-click on picked rows: *Copy as script* (*Copy N lines as script*) and *Open in editor*
  (a new unsaved script). A hole is written as a comment naming its step.

## 10. Where scripts live

**A script may be stored in a document; it is never run by the document.** No script runs on open
or re-runs on any change: a script the document re-runs is the constraint engine `foundations.md`
§13 decision 1 forbids, however small the convenience. Opening a shared document therefore runs no
one else's code, and there is no warning on open: a run the person starts is inside §8's sandbox,
and warnings train people to click *Allow*. A stored script is a tool shipped beside the work; the
document records what its run committed and never needs it.

| Place | Kept | Edited |
| --- | --- | --- |
| **Unsaved** | the editor's list, for the session | in place |
| **Library** | `.luau` files in `prefs::folder()/scripts` (`scripts::library`), shared and git-tracked as files | written 600 ms after typing rests |
| **This document** | `Document::scripts()` (`mastik_core::script`), saved as the optional `scripts` key (`io.md`) | each change a `SetScriptSource`, typing folding into one *Edit script* step |
| **Kit · name** | the kit's published state (`kit::freeze` carries its scripts), listed from its latest publish | read-only |

- **The library folder.** A script's name is its file's stem, compared without case as Windows and
  macOS compare file names; `check_name` refuses a name that is not one plain file name on every
  platform, and a name ending `.d`. Writes are atomic, and a pending write is flushed on exit. A
  delete asks first (*Delete script?*), since it removes the file with no undo. `mastik.d.luau`
  (§10, *Types*) sits beside the scripts and is never listed as one.
- **Document scripts** change through `AddScript`, `RemoveScript`, `SetScriptName` and
  `SetScriptSource` (the two setters fold into an edit run) and the verbs `add_script`,
  `set_script`, `remove_script`.
- **Copies between a document and the library, never links**: *Save to library* and *Copy into this
  document* copy, and a script of the same name in both is listed twice. **A kit's scripts are the
  one link**: they follow the kit's publishes as its components do, the disclosure covering each
  changed text. A change to them is worth a publish (`kit::Changes::scripts`), and the publish
  popover says so.

**The first-run disclosure.** Before a document's or a kit's script first runs on this machine, a
box says who wrote it and what a script can and cannot do, and shows its source. It is recorded in
the preferences (`Prefs::script_disclosed`), never in the file, so a sender cannot mark a script as
shown: the SHA-256 of the source shown, keyed by `prefs::disclosure_key` (the document's or kit's id
and the script's). A changed source is shown again. The person's own scripts (library, unsaved)
are not disclosed, and neither is a document script's text the person typed in the editor or
copied into the document. The form and the disclosure are modal: the keymap waits while they
are up.

**The header** is the run of `---` lines a script opens with: the first is its title, the rest its
description, and each `---@input name type = default -- Label {lo..hi}` an input Mastik asks for in
a form before the run, read in the script as `param.name` (an unknown one raises). Types: `number`,
`integer`, `boolean`, `string`, `color` (`"#rrggbb"`, or `"#rrggbbaa"`), `choice(a, b, …)`. A missing default is the
type's zero (the range's low end, the first choice); only a number or an integer takes a range. A
header that does not parse fails the run at its line. The form draws each by its type: a
`ui::value_field`, a `ui::colour_text_field`, a `ui::segmented` for up to three choices and a
dropdown past that, a `ui::switch_row` carrying the input's label, a `ui::text_field`. Values are
remembered per script for the session.

**The run.** Disclosure if owed, then the form if the header has inputs, then the run (§7). The
status line says *Card grid · 6 layers changed*, *Card grid failed at line 4 — nothing changed* or
*Card grid stopped: … — nothing changed*; a failure opens the editor on the script with its line
marked.

**The editor** (`command/editor.rs`) is in the app: *Tools ▸ Scripts…*, and the palette's *Scripts…*
and a *Run ‹name›* row per saved script. One floating, resizable window: a list on the left (a
search field; sections *This document*, *Library*, *Kit · ‹name›*, *Unsaved*; *New script* and the
folder button at its foot), the script's head (its name, renamed in place, where it lives, find,
the ⋯ menu and *Run*), the code pane and an output pane listing the last runs under timed rules,
errors in the error ink. The list's rows are list rows (`MENU_ROW_H`, `ui::list_row_ground`) in the
palette's row layout; with no script open the pane is a `ui::empty_state`. The body is laid out in
fixed rects, so the window keeps the size it is given. `Ctrl+Enter` runs. `Ctrl+F`, from the code pane, opens find: Enter
selects the next match, wrapping to the top, and `Ctrl+F` then Enter goes on from there. The ⋯ menu: *Save to library* ·
*Copy into this document* · *Duplicate* · *Rename* · *Show in folder* · *Delete* (not on a kit's).
The code pane is `ui::code_editor`, inked from `mastik_vm::tokens`: keywords `star()`, numbers
`saved()`, strings `warn()`, comments `text::faint()`, Mastik's own names `accent_300()`.

**Types.** `mastik_vm::definitions` writes a `.d.luau` from the same tables as the bindings (the
`Layers` class, `sel` and the relative names, each verb with typed parameters, `param`), saved as
`mastik.d.luau` in the scripts folder when the editor opens (never from a headless app, whose
preferences are a throwaway), so luau-lsp in an outside editor completes and checks against it.
**In-app type checking is not built**: Luau's type checker is a separate analysis library that
`mlua` does not expose. Completion in the bar and the editor covers Mastik's API only, from the
same tables.

## 11. MCP and `/ai`

Reads stay typed tools, and so do single edits, for validated arguments and an error per call. A
document's stored scripts are written through `add_script`, `set_script` and `remove_script`
(`mcp.md` §8.4); MCP runs none. A tool that runs the language for bulk work is open work
(`roadmap.md`). `/ai` is unbuilt (`ai.md`).

## 12. Non-goals

In `non-goals.md`, *Command Mode*.

## Navigate (`navigate.rs`)

**A keymap, not a canvas state.** `` ` `` (the Tools menu's *Navigate*, the palette's) turns it on
with the Select tool and off again; `MastikApp::mode` stays `Normal`, so the canvas, the panels and
the mouse behave as in the editor, and `keymap_mode` hands `input::resolve` `Mode::Navigate` in
place of the editor's keys. A text edit started from it runs in `Mode::TextInsert` and ends back in
it. Leaving keeps the selection and gives back the tool it had. While on, the Tools menu's and the
palette's row read *Stop navigating*.

| Key | Act |
| --- | --- |
| `h` `←` / `l` `→` | parent / child (the child last stood on, else the topmost) |
| `j` `↓` / `k` `↑` | the Layers panel's row below / above: structural, so `j` steps back in z-order |
| `Shift` + a step | add the layer to the selection |
| `f` / `Shift+F` | labels on the layers in view / inside the current layer |
| `Enter` `i` | step in, as the editor's `Enter` |
| `:` / `.` | the command bar / run the last bar line again |
| `Shift+2` | zoom to the layer |
| `u` | undo (and the history chords) |
| `` ` `` `Escape` | leave |

Every other editor key rests; the cheatsheet's chord stays, and opened from here its *Navigate*
section leads. **The sign is the status card at the canvas's bottom left**: *NAVIGATE* in
`color::warn()`, its only sign, then the path to the current layer and *f labels · : command · `
leaves*.

**Leap labels** are the spatial instrument. Two home-row letters each (81 pairs, the easiest nearest
the current layer); the candidates are what a plain click could reach, found by hit tests sampled
across the view and at each layer's centre, each raised by `group_chain` as a click is. More than
81 label only the children of the current layer (its parent's when it has none), those in view. The
first letter keeps outlines only on the labels it still matches and fades the rest; the second
selects (with `Shift`, adds); `Backspace` takes the first back, `Escape` hides them. Each labelled
layer is outlined on its bounds, outset 3: dashed with the dark label for a group, boolean or
frame, solid with the yellow label for any other layer, so *the group or the layer inside it* is
told by line and lightness, not hue. Every line is two inks, `LEAP_INK` inside and `color::leap()`
outside, so it reads on any artwork. They are egui chrome painted in `draw_canvas` over everything
a layer wears, not a `RenderOverrides` overlay. **Labels outlive the deal**: the last labels dealt
stay bound as handles for the bar and scripts until the next deal. **Opening the command bar deals
too**, to the layers in view as `f` does, quietly, so a line can name what is on screen; the bar
shows them only while its completion offers labels for the word at the caret (a word of at most two
home-row letters), narrowed by the letters typed as Navigate's first letter narrows them.

## The command bar (`command/mod.rs`)

`:` opens it (`Key::Colon`, shifted or not) from the editor or Navigate whenever no text entry has
the keyboard, closing the palette and any menu; it needs no mode of its own. It is a sibling of the
palette's card: 640 wide, docked 24 above the canvas's foot, or 24 below its top when the selection
reaches into that band. It is placed from the height it works out that frame, not anchored, since
an anchor reads last frame's size and a growing list would push the foot down a frame early; the
list is drawn into exactly that height, and the area is not constrained to the screen, since egui
constrains by last frame's size and would shove the card up on the frame its list shrinks. Its
rows, section labels and footer are the palette's (`app-chrome.md`, *Shared rules*): a row's name
is never cut for its note.

- **The field**: monospace at the palette field's size (`HEAD_PT`), a `:` prompt in the accent, a
  read's live value (`= 20`) at the right edge.
- **The footer**: the desugared Luau and the subject (*Card*, *Card and 2 more*, *Nothing
  selected*); *each by itself* after a rule-3 line. An error replaces it with a red caption, the
  token it names squiggled, and *Tab fixes it* when it carries a fix.
- **The list above**: the log while the field is empty, else completion, at most eight rows in
  sections *Names*, *Verbs*, *Properties*, *Labels*, *Layers*, *Choices* (a verb's next argument's
  words), the typed letters lit; an almost-match is a dim row naming what it meant. A whole-name
  match outranks a prefix, and a section whose best row matches better goes first, so a label or
  name typed out whole leads the list. The lit row's layer (a label's, or a layer's by name) is
  outlined on the canvas as a hovered Layers row is.
- **When completion shows** follows a code editor's suggest widget (VS Code's, Zed's): typing a
  name's letter (not a digit), `.` or `:`, or the space before a verb's argument opens it on that
  word; any other character shuts it; deleting keeps it while the word has letters (or sits right
  after a trigger); the caret leaving the word shuts it. `Escape` shuts it for that word, and typing
  on in the word leaves it shut; `Ctrl+Space` (Ctrl on every platform) opens it on the word at the
  caret. Taking a row shuts it. A list with no rows is not shown.
- **Keys**: while the list shows, `↑`/`↓` move through it, `Tab` takes the lit row and `Enter` takes
  it too unless the word already reads as the row, when `Enter` runs the line. Otherwise `↑`/`↓`
  recall typed lines, `↓` past the newest giving back what was being typed; `Tab` takes an error's
  fix; `Enter` runs (a read copies its value and makes no step; a write is one step named after its
  verb, else *Edit*, with `TERMINAL_WINDOW`). `Escape` closes the log's menu if it is open, else the
  list, else clears the text, then closes.
  A failed line stays in the field with its error.

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
