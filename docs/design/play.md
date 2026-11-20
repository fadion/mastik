# Prototyping: the play engine

The engine that plays a prototype: a copy of the document, the state that exists only while it
plays, the triggers and actions, motion, live update and takes. It is one crate, `mastik-play`,
headless, which both shells drive — the in-app player and the exported page — so the two behave
the same. The saved model it plays is `prototyping-model.md`; the HTML export is
`prototyping-export.md`; the shells and where the player shows are `prototyping.md`. Code: `crates/mastik-play`, the play values in
`mastik-core/src/variable.rs`, expressions in `mastik-core/src/prototype/expr.rs`.

## The copy and its door

- **`Player::new(doc, start, actor)` clones the document.** Play changes only the clone, through
  `Player::commit`: `propagate::owed`, then `Document::apply`, then `Resolved::update` and
  `settle` — the editor's commit door without history, autosave or a dirty mark. Instances follow
  a main changed in play because `owed` runs.
- **Ids minted in play come from the player's actor**, which the shell passes and must be the
  player's own, so a layer play makes never collides with the document's.
- **The player keeps a second, untouched copy** (`base`): every editor commit lands there too, so
  a refused live edit re-clones without asking the shell for the document, and *Restart* starts
  from the document as it now is.
- **Time comes in through `tick(ms)` alone.** Nothing reads a clock; every motion is a closed
  function of the time since it started, and a timer's actions run at its due time even when a
  tick passes it. A run is therefore a function of its inputs and their times, which is what lets
  a take replay to the same state.

## Play values

A play value is an override keyed by variable that **beats every mode** wherever the copy reads
it. It lives on the copy's variable table (`Variables::set_played`, through
`Document::set_play_values`), never in an operation, never saved, never undone.

- `Variables::resolve_at` reads it first at **each step** of its walk: a value on an alias wins
  over its target, an alias reaches its target's value, and a kit's variable takes one as the
  document's own do. Every drawing door (`resolve_var_in`, `resolve_brush_in`, the bound sizes,
  texts and visibility) goes through `resolve_at`, so the copy draws play values with no other
  change.
- A value of the wrong kind, a non-finite number, or one for a variable not there is left out.
- `set_play_values` answers every layer that reads a variable, for `Resolved::update`.
- A flow's starting values are play values from the start: a literal cell as it is, an alias
  read at the start frame.

## Play state

Held by the `Player`, never in the copy: the screen frame and its page; *Back*'s history (each
entry the frame left and the transition that left it); the open overlays with their settings and
opening transition; the play values; scroll offsets per scrolling frame; the main each instance
play switched was switched to (carried across a re-clone); instances showing an interaction state
by themselves, with the main to return to; the layers bound as inputs; the pointer, the layers
under it, a press in progress, the focused input; *while* reactions in force with what to put
back; pending timers; motions under way; notices, the script log and requests for the shell.

## The shell's contract

Everything is in **screen space**: the screen frame's box with its top-left at the origin, one
unit a document pixel. The shell scales and places that box by the page's *Scaling* and maps
pointer events back into it.

| Call | What it does |
| --- | --- |
| `Player::new(doc, Start::Flow(r) \| Start::Frame(f), actor)` | Start; `StartError` when the flow or frame is not there |
| `input(Input) -> bool` | Pointer move, down, up, leave; wheel; a key; text; a state-card value. Answers whether the prototype used it, so an unused Escape goes back to the editor |
| `tick(ms)` | Advance time: timers fire at their own times, motions move on |
| `scene() -> Scene` | What to draw now |
| `hotspots()` | The rectangles of the layers that take a person's act in the frame that takes input, for the hints |
| `values()`, `set_value(var, value)` | The state card's rows, and setting one (recorded in a take; reactions to the change fire) |
| `apply_edit(&tx)` | Live update: an editor commit |
| `take_notices()`, `take_requests()`, `log()` | What to say once; what to do (open a URL); scripts' prints and failures |
| `take_dirty()` | Nodes of the copy whose drawing changed, for the renderer |
| `document()`, `resolved()` | The copy to draw, and its derived state |
| `screen()`, `page()`, `overlays()`, `history()`, `scroll_offset(f)`, `focused()`, `now()` | Play state, read-only |
| `is_moving()` | Whether anything changes without input; a shell may stop ticking while false |
| `has_scripts()` | Whether any *Run script* exists, for the scripts disclosure at start |
| `restart()`, `record()`, `stop_recording() -> Take`, `Replay` | Restart; takes |

**A `Scene`** is the page, the screen size, the **layers** to draw bottom first, the **patches**
over the copy, and the hints' flash strength. A layer is a frame drawn whole and clipped to its
box, its top-left at `at`, with an opacity and, for an overlay, a backdrop colour to cover what is
beneath (already faded). Two screen layers are drawn while a transition runs; each open or
closing overlay adds one.

**A `Patch`** replaces, for this frame only, one node's local transform, box size, own opacity,
visibility or paint (with literal brushes) — the granularity of the renderer's per-node
override, so an adapter turns each into one `NodeOverride` (a transform, a kind with its box size
set, a paint, an opacity, a visibility) without `mastik-play` depending on the renderer. A
transform patch moves a node but layout is not run on the patch: smart animate has already laid
out both ends.

## Triggers

- A **pointer trigger fires on the deepest layer under the pointer that has one**, then stops;
  a layer inside an instance shown in its *Disabled* state is passed over. Click fires on release
  if the press did not become a drag, over what was under the press.
- **Hit testing is by box**, not ink: a layer with a box (a frame, a shape) by its box, any other
  by its bounds, each moved by the scrolling frames above it. A group is hit only through its
  children. A clipping or scrolling frame cuts off what is outside it. A click on the box that
  holds an input text focuses it, since an empty text has no width.
- **Overlays are modal.** A press outside the top overlay closes it when its *close when clicking
  outside* is on, and reaches nothing beneath either way.
- *Hover enter* runs once per layer the pointer enters; *while hovering* and *while pressed* record
  what their actions did and put it back, newest first, when the pointer leaves or the press ends:
  a variable's previous play value, an instance's previous main, overlays opened, a navigation
  (one *Back*).
- A **drag** is a press that moves more than four units. A drag reaction on the pressed chain
  takes it; otherwise the nearest scrolling frame in the chain scrolls with it.
- A **key** fires *key* reactions in the frame that takes input whose key matches (letters without
  regard to case) and whose Shift matches. While an input text is focused, keys are text: Backspace
  deletes, Enter and Escape leave the field. While an input instance is focused, Space and Enter
  flip it, Escape leaves it, and other keys fire as ever. Tab is never a key trigger (*Inputs*).
- *After* starts when its frame opens and is dropped when the frame closes. Inside an instance it
  starts again whenever play changes the instance's state, the old state's timers dropped, so a
  variant whose *After* changes its instance to the next variant is **a state that loops** (a
  spinner, a skeleton's shimmer) for as long as its frame shows. There is no loop setting: the
  loop is the variants' own reactions, which every instance follows and a kit publish keeps. An
  *After* restarted this way waits at least 16 ms, so two states changing to each other after
  nothing still let time move, and one `tick` fires at most 1024 timers, the loop going on from
  the tick's end.
- *Frame opens* runs when the screen or an overlay opens; *variable changes* runs on every shown
  frame's layers when a play value changes what the variable reads.
- Reactions setting each other off are followed sixteen deep, then cut with a `Loop` notice.

## Actions

- **Targets are read through `prototype::resolve_target`**, so a reaction a main gives its instance
  acts on the instance's own layer. A target not there — or not a frame where a frame is needed —
  is skipped with one `Missing` notice per action.
- **Navigate** closes every overlay, pushes the history and resets the scroll of the frames inside
  the destination. Navigating to the screen itself does nothing.
- **Back** closes the top overlay when one is open, by the reverse of its opening transition;
  otherwise it pops the history by the reverse of the transition that left that frame (a move in
  becomes a move out from the other side; a push or slide comes from the other side).
- **Open overlay** places the frame by its own `OverlaySettings`; **swap** replaces the top one;
  **close** closes it by the transition given.
- **Scroll to** scrolls the nearest scrolling frame holding the layer until the layer's top-left is
  at the frame's, as far as the content allows.
- **Change state** is `variant::switch` committed on the copy, so ids survive. An authored change
  on an instance ends any interaction state it was showing by itself.
- **Set variable** evaluates against the copy at the reaction's layer (play values included);
  increment applies to numbers and toggle to booleans, and anything else is skipped. A value that
  does not change what the variable reads fires nothing.
- **Conditional** runs the first branch whose condition is true, else `otherwise`; a condition
  that errs or is not true or false is skipped with a notice.
- **Open URL** asks the shell (`Request::OpenUrl`) for `http` and `https` addresses only.
- **Run script** runs the document script through `mastik-vm` on the copy, under a person's act
  only (the door's rule, checked again). The selection is empty, no labels, no input form — a
  script whose header asks for input is not run, with a notice — and `print` goes to the log. The
  copy and its resolve are snapshotted first and put back whole if the run fails. Behind the
  default `scripts` feature; without it the action is skipped with `ScriptsOff`.

## Inputs

- At start and after every live edit the player finds the layers bound as inputs. An **input
  text**'s content is bound to its string variable on the copy, so typing shows; text typed into
  the focused field writes the variable's play value.
- An **input instance** flips its boolean on a click, and every input instance follows its
  variable through its boolean variant property: the value named `true`/`on`/`yes`/`checked`
  (`false`/`off`/`no`/`unchecked` for false), else the property's second value for true and its
  first for false.
- **Focus** is held by one input at a time: an input text takes it on a click (or a click on the
  box holding it), and **Tab** and **Shift+Tab** move it through the inputs of the frame that takes
  input, input texts and input instances alike, in reading order — topmost, then leftmost, ties to
  the earlier layer — round from the last to the first. Hidden inputs and those in a *Disabled*
  instance are passed over. A click on anything else, a navigation, or closing the overlay that
  holds it takes focus away.

## A component set's interaction states

With nothing authored, an instance whose set has an interaction-state property shows its
*Hover* variant while hovered and its *Pressed* variant while pressed, returning to *Hover* on
release and to the variant it rests on when the pointer leaves. Only an instance linked to a
variant main, which `variant::can_switch` allows, plays them. An authored hover reaction on the
instance replaces the automatic hover. *Disabled* takes no input. The nearest instance holding
the focused input (the input itself, for an input instance) whose set has a *Focus* value shows
it while focus stays; hover does not replace it, a press shows *Pressed* and returns to *Focus*,
and it rests when focus leaves.

## Scrolling

A frame scrolls along its `LayerPlay::scroll` axis as far as its children reach past its box,
fixed children not counted. The wheel scrolls the deepest scrolling frame under the pointer that
can still move; a drag scrolls as above. Each child of a scrolling frame is drawn by its pin:
*scrolls* moves with the offset, *fixed* stays, *sticky* moves with the content until it meets the
frame's top or left edge and stays there. Scrolling is a patch on each child's transform over
whatever else patched it.

## Motion

- **Timing**: `motion::progress` for an easing curve over its duration — the CSS set as their
  published cubic Béziers, a custom cubic read by `x` (Newton's method, then bisection) — or a
  spring from rest, closed form for under-, critically and over-damped, settled once it stays
  within 0.001 of its target (found at 240 Hz, capped at ten seconds).
- **Screen transitions**, by the direction's side: *dissolve* fades the new frame in over the old;
  *move in* slides the new one over; *move out* slides the old one off, drawn on top; *push* moves
  both by the screen's size; *slide* moves the new one over while the old one moves 30% of the
  way. An overlay opens by the same kinds over its own place and closes by them in reverse; its
  backdrop fades with it.
- **Smart animate between frames** pairs layers by path below each frame: each step the layer's
  name, or for a layer of an instance the lineage key of the main layer it follows, and which of
  its siblings with that key it is — so a pair's parents are paired too. Both ends are laid out
  when it starts and never again. A paired layer's transform (moved, turned the short way, scaled
  and sheared, each interpolated), box size, opacity and solid fills interpolate; the frame's own
  paint does too. The unpaired layers move by the transition's direction, a screen's width or
  height: the end's slide in from the far side to their places, the outgoing frame's slide out
  toward the side, as *push* moves frames, none of them fading. The outgoing frame is drawn over
  the end with its background blank: its paired layers hidden, its unpaired ones leaving, and a
  paired layer that holds a leaving one moving with its pair, its own paint blank. The shift is
  measured in each layer's parent as it stands at its end.
- **A change of state** with any transition but *instant* animates as smart animate over the
  instance, paired by id (the switch keeps them), measured from the instance's parent, with no
  direction. Layers the new state lacks are gone at once; its new ones fade in. Siblings moved by the instance's new size
  jump.
- A new navigation replaces a screen transition under way.

## Live update

`apply_edit(tx)` takes each transaction the editor commits. It is applied to the base copy (an
`Err` there means the shell passed the wrong document and nothing changes), then to the played
copy. When the copy refuses it — play changed something the edit touches — the copy is cloned again
from the base and play state carried across by id: the screen, history, overlays, play values,
scroll offsets, and each instance re-switched to the main play left it on. Motions under way end.
Then what the edit removed falls back, one notice each: a deleted screen to the start frame (else
the first frame of the document), a deleted overlay closes, a deleted variable's play value goes.
A variable whose starting value was edited keeps its play value.

## Takes

`record()` restarts play and records each input with the time it arrived; `stop_recording()`
answers the `Take` (start, inputs, length). `Replay::new(doc, take, actor)` plays it on a fresh
copy; `advance(ms)` gives each input due in that span at its own time, so a replay stepped frame
by frame for a video reaches exactly the state the run had at each time.

**Video** (`mastik_export::video::video(doc, take, actor, options, progress)`) replays a take on
a fresh copy at a fixed rate (1 to 60 frames a second), a frame at every frame time from the start
to the take's length, each drawn by `VelloCpuRenderer::render_play_to_rgba` at the options' scale
(capped at 4096 pixels a side) over white, every frame cut or padded to the first one's size. A
GIF loops, each frame on its own palette, its delays carrying their rounding so they add up to
the take. An MP4 is H.264 from OpenH264 built from source (BSD-2-Clause; the C++ toolchain `mlua`
already needs builds it, and its patent licence covers only Cisco's own binaries), at even sizes,
muxed by the `mp4` crate with one sample a frame. `progress` hears each frame and stops the job by
answering false.

## Notices

`Missing` and `Expression` are said once per action; `Script` and `ScriptNeedsInput` per run;
`ScriptsOff` and `Loop` once; `Fallback` (screen, overlay, variable) once per live edit that caused
it.

## Expressions

`prototype::Expr` holds a condition or *Set variable*'s value as source text, a variable written
by id, `{<actor-hex>:<seq>}`.

- **Values**: numbers (`12`, `0.5`), text in double or single quotes with `\` escapes, `true`,
  `false`, and variables of any kind (a colour can be read and compared, not computed with).
- **Operators**, loosest first: `or`; `and`; `not`; one comparison (`==` `!=` `<` `<=` `>` `>=`, not
  chained); `+` `-`; `*` `/` `%`; unary `-`; parentheses. `+` joins when either side is text, a number
  written whole or with at most three decimals; `==` and `!=` need two of one kind; `<`-style
  comparisons take two numbers or two texts; `and`/`or`/`not` take booleans and short-circuit.
  Division by zero is an error.
- **Errors** carry the byte range they are about (`ExprError::at`): syntax, a type, a variable
  not there, a name no variable has.
- **The field** shows names: `with_names(name_of)` writes `{<name>}` for each id it can name, and
  `from_names(text, id_of)` turns a typed `{<name>}` (or an id) back into the stored form,
  answering the first error in the typed text so the draft is kept and underlined. `kind(kind_of)`
  answers the kind an expression comes to without evaluating it, for checking *Set variable*'s
  value against its variable. `var_tokens` lists the `{…}` spans for highlighting.

## Building for the browser

`mastik-play` depends on `mastik-core` and, behind `scripts`, `mastik-vm`; nothing renders or
draws (`deps_forbidden` holds it to the headless list). The page's player (`crates/mastik-web`)
runs it on a pinned nightly that unwinds (`prototyping-export.md` *The browser build*), without
`scripts`: `mlua` does not build for that target, so a *Run script* there is skipped with
`ScriptsOff`.
