# Prototyping: the play engine

The engine that plays a prototype: a copy of the document, the state that exists only while it
plays, the triggers and actions, motion, live update and takes. It is one crate, `mastik-play`,
headless, which both shells drive — the in-app player and the exported page — so the two behave
the same. The saved model it plays is `prototyping-model.md`; the HTML export is
`prototyping-export.md`; the in-app shell and where the player shows are
`prototyping-player.md`. Code: `crates/mastik-play`, the play values in
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
entry the frame left, the transition that left it and whether that navigation kept scroll); the
open overlays with their settings and opening transition; the play values; scroll offsets per
scrolling frame and how far each layer that follows the drag was moved, both kept for the run;
the settles that ended with their drops still to run; the main each instance play switched was switched to and the
modes *Set mode* gave each page or frame (both carried across a re-clone); instances showing an interaction state
by themselves, with the main to return to; the layers bound as inputs; the pointer, the layers
under it, a press in progress with its long presses due, a click held for a double click, the
focused input; *while* reactions in force with what to put back; the scroll offsets the scroll
triggers last saw; pending timers; motions under way; notices, the script log and requests for the shell.

## The shell's contract

Everything is in **screen space**: the screen frame's box with its top-left at the origin, one
unit a document pixel. The shell scales and places that box by the page's *Scaling* and maps
pointer events back into it.

| Call | What it does |
| --- | --- |
| `Player::new(doc, Start::Flow(r) \| Start::Frame(f), actor)` | Start; `StartError` when the flow or frame is not there |
| `input(Input) -> bool` | Pointer move, down, up, leave; wheel; a key; text; a state-card value or mode. Answers whether the prototype used it, so an unused Escape goes back to the editor |
| `tick(ms)` | Advance time: timers, held clicks and long presses fire at their own times, motions move on |
| `scene() -> Scene` | What to draw now |
| `hotspots()` | The rectangles of the layers that take a person's act in the frame that takes input, for the hints |
| `values()`, `set_value(var, value)` | The state card's rows, and setting one (recorded in a take; reactions to the change fire) |
| `modes()` | The state card's modes (`ModeRow`): each collection with two modes or more, the document's then each kit's, the mode the screen reads, and whether the screen sets one itself. A pick is `Input::SetMode`, set everywhere and recorded in a take |
| `set_play_size(size)`, `play_size()` | Lay every screen out at `size` in place of its own (*Responsive*, a turned device), `None` as drawn: the copy's frame is given that size (`fit_screen`, a `SetGeometry` on the copy) as play starts, before a navigation's smart animate measures it, after a live edit and across a restart, so its flex, grid and width queries answer as on a real screen of that size. A frame with no layout keeps its children where drawn. Overlays keep their own size |
| `width_pick()` | The collection picked by width and the mode the screen's play width picks, by name, while it differs from the one its drawn width picks (the base document's `width_mode`); `None` where the screen does not pick by width (`Document::width_pick`: a pin, a component). The player's mode chip |
| `rebase(&doc)` | Play `doc` in place of the document the player started from, keeping the run's place as a refused live edit does (the copy cloned again, play state carried by id, what `doc` lacks falling back): for a shell sent the document whole, a phone mirroring the app, whose trimmed copy cannot take the editor's transactions |
| `apply_edit(&tx)` | Live update: an editor commit |
| `take_notices()`, `take_requests()`, `log()` | What to say once; what to do (open a URL); scripts' prints and failures |
| `take_dirty()` | Nodes of the copy whose drawing changed, for the renderer |
| `document()`, `resolved()` | The copy to draw, and its derived state |
| `screen()`, `page()`, `overlays()`, `history()`, `scroll_offset(f)`, `moved(id)`, `focused()`, `now()` | Play state, read-only |
| `is_moving()` | Whether anything changes without input; a shell may stop ticking while false |
| `has_scripts()` | Whether any *Run script* exists, for the scripts disclosure at start |
| `restart()`, `record()`, `stop_recording() -> Take`, `Replay` | Restart; takes |

**A `Scene`** is the page, the screen size, the **layers** to draw bottom first, the **patches**
over the copy, and the hints' flash strength. A layer is a frame drawn whole and clipped to its
box, its top-left at `at`, with an opacity and, for an overlay, a backdrop colour to cover what is
beneath (already faded). Two screen layers are drawn while a transition runs; each open or
closing overlay adds one.

**A `Patch`** replaces, for this frame only, one node's local transform, box size, corner radii,
own opacity, visibility, paint (with literal brushes) or effect stack (with literal numbers) — the
granularity of the renderer's per-node override, so `RenderOverrides::from_play` turns each into
one `NodeOverride` (a transform, a kind with its box size and radii set, a paint, effects, an
opacity, a visibility) without `mastik-play` depending
on the renderer; `mastik_render::play` draws the scene on either backend (`render.md`, *Play*). A
transform patch moves a node but layout is not run on the patch: smart animate has already laid
out both ends.

## Triggers

- A **pointer trigger fires on the deepest layer under the pointer that has one**, then stops;
  a layer inside an instance shown in its *Disabled* state is passed over. Click fires on release
  if the press did not become a drag and no long press spent it, over what was under the press.
- **A click goes to the deepest layer with *On click* or *On double click*.** When that layer has
  a double click, the click is held `DOUBLE_MS` (250 ms): a second click on the same layer within
  it fires the double click and no click; otherwise the held click runs as a click on what was
  under the first (at once, if the next click lands elsewhere). A layer with only *On click* is
  never delayed, so only a layer that asks for a double click pays for it.
- **A long press** is due its `ms` after the press on the deepest layer with one, and fires while
  the press is held and has not become a drag; it spends the press, so no click follows. A drag
  cancels it. Held clicks and long presses are timed through `tick` as *After* timers are, and
  `is_moving` stays true while one waits.
- **Hit testing is by box**, not ink: a layer with a box (a frame, a shape) by its box, any other
  by its bounds, each moved by the scrolling frames above it and by how far it and the layers
  above it were dragged (`child_offset`), then taken back through a scroll effect's move, turn
  and scale (`walk`, from the box's centre as drawn). A group is hit only through its
  children. A clipping or scrolling frame cuts off what is outside it. A click on the box that
  holds an input text focuses it, since an empty text has no width.
- **Overlays are modal.** A press outside the top overlay closes it when its *close when clicking
  outside* is on, and reaches nothing beneath either way.
- *Hover enter* runs once per layer the pointer enters, *On leave* once per layer it leaves (the
  player included); *while hovering* and *while pressed* record what their actions did and put it
  back, newest first, when the pointer leaves or the press ends: a variable's previous play value,
  an instance's previous main, overlays opened, a navigation (one *Back*). *On leave* fires only
  for a layer still in a shown frame — one that went with its frame is gone, not left — and not
  in a *Disabled* instance.
- A **drag** is a press that moves more than four units. A swipe reaction anywhere in the pressed
  chain toward the side the move is going (its longer axis) takes it whole. Otherwise the deepest
  layer in the chain that follows the drag moves with it (*A layer that follows the drag*) and a
  drag reaction on the chain fires as well; the nearest scrolling frame in the chain scrolls with
  it only when neither took it. So a card can take left and right while its list scrolls up and
  down.
- **A swipe** holds the drag: nothing fires and nothing scrolls while it moves. On release, the
  side is read again from the whole move, and the deepest layer with a swipe toward it fires if
  the move went at least 30 units that way, more along than across, and was released within
  300 ms or averaged 0.5 units a millisecond — a slow drag ending to one side is no swipe.
- ***On drop*** (`Trigger::Drop`) fires on a layer that follows the drag once it has come to rest
  after a release: when its settle ends in `tick` (`Player::drops`, run after the motions
  advance), or at once when the release moves it nowhere. The reactions for *anywhere* run, and
  those for the point it reached (`DropAt::Drawn` or a `DropAt::Point`); *Stays* and *Returns*
  reach only *anywhere*. A trigger whose point is gone never fires.
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
- **A scroll trigger** (`ScrollPast`) on a shown frame's layer, outside a *Disabled* instance,
  fires when its frame (read through `resolve_target`) crosses `at` the way it asks: forward when the offset was below `at` and is
  now at or past it, back when it was at or past and is now below. A sideways frame's horizontal
  offset is read, any other's vertical. It is checked after every input and every `tick` step
  against the offsets last seen, so a jump across (*Scroll to*, a wheel) fires once and resting
  past the point fires nothing. A navigation that does not keep scroll forgets the destination's
  seen offsets with its scroll, so reopening at the top is not a crossing back. A trigger whose frame is gone never fires.
- Reactions setting each other off are followed sixteen deep, then cut with a `Loop` notice.

## Actions

- **Targets are read through `prototype::resolve_target`**, so a reaction a main gives its instance
  acts on the instance's own layer. A target not there — or not a frame where a frame is needed —
  is skipped with one `Missing` notice per action.
- **Navigate** closes every overlay and pushes the history. The player keeps every screen's scroll
  offsets for the run; `Player::show` resets those of the frames inside the destination unless
  the action keeps scroll (`keep_scroll`), and *Back* to a screen keeps them when the navigation
  that left it did (`Visit::keep`). Navigating to the screen itself does nothing.
- **Back** closes the top overlay when one is open, by the reverse of its opening transition;
  otherwise it pops the history by the reverse of the transition that left that frame (a move in
  becomes a move out from the other side; a push or slide comes from the other side).
- **Open overlay** places the frame by the action's `OverlaySettings`; **swap** replaces the top
  one, placed by its own action's; **close** closes it by the transition given. A manual position
  is measured from the opening layer's top-left as drawn when it opens (`Player::drawn_corner`:
  in its shown frame, moved by an overlay's place, the scrolls above it and any drag it follows),
  which the overlay
  keeps (`Overlay::anchor`); a swap reads it before the top overlay goes, since the layer may be
  in it.
- **Scroll to** scrolls the nearest scrolling frame holding the layer until the layer's top-left is
  at the frame's, as far as the content allows.
- **Change state** is `variant::switch` committed on the copy, so ids survive. An authored change
  on an instance ends any interaction state it was showing by itself.
- **Set variable** evaluates against the copy at the reaction's layer (play values included);
  increment applies to numbers and toggle to booleans, and anything else is skipped. A value that
  does not change what the variable reads fires nothing.
- **Set mode** commits `SetModes` on the copy in place of any mode of the same collection there:
  on every page root for *Everywhere*, so a frame that sets its own mode keeps it and a by-width
  collection is held at that mode; on the screen frame for *This screen*. A page holds no kit's
  mode, so *Everywhere* puts a kit's on each top-level frame instead, passing over one that pins
  a mode of that kit collection in the document (`Player::everywhere`). The player remembers
  what it gave each node (`put_modes`), so a live edit's re-clone puts it back; a *while* trigger
  undoes it (`Undo::Modes`). A mode in neither the document's collections nor a kit's is skipped
  with `Missing`. Nothing animates. *Restart* and a new flow start from the document's own modes
  and the flow's starting modes, which play sets as *Everywhere* does before anything opens.
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

**Snapping** (`LayerPlay::snap`): a frame that snaps rests on points along each axis it scrolls
(`Player::snap_points`), clamped to its scroll range.

- *Pages* rests on whole steps of the frame's size, and on the far end.
- *Children* rests on each visible, non-fixed child, by `snap_align`: *Start* brings its leading
  edge to the frame's inset, *End* its trailing edge to the inset from the far side, *Centre* its
  centre to the frame's. The inset is the first child's lead, used on both ends.
- A wheel gesture moves one point (`snap_step`); while that step moves, further wheel events on
  the frame are swallowed, so a trackpad's stream of deltas is one step.
- A drag scrolls freely; its release (`snap_release`) moves at least one point the way it went.
  Let go within 300 ms, it is carried 200 ms on at its pace and rests on the point nearest there.
- The settle runs on the page's new-link timing.

## A layer that follows the drag

A layer whose `LayerPlay::follow` is set moves with a drag that starts on it (`drag.rs`); one in
a *Disabled* instance does not. Its places are offsets from where it is drawn.

- **While dragged** it moves by the pointer's travel since the press, held to its axis (*None*
  reads as both) and to its range: *Free* has none; *Parent* keeps its box inside its parent's,
  and still along an axis where its box is the larger; *Points* keeps it between its outermost points
  on each axis, the drawn place counting as one. Past its range it moves at a third of the
  pointer's pace (`GIVE`), so the edge is felt.
- **Let go**, *Stays* holds it back into its range, *Returns* takes it to where it is drawn, and
  *Snaps* to the point nearest where it is let go — or, let go within 300 ms of the press, nearest
  where it would be after carrying on 200 ms at its pace, as a snapping frame's drag does. The
  drawn place is always a point. It moves there on `Follow::settle`, then drops (*On drop*,
  *Triggers*).
- **How far it moved** (`Player::moved`) is drawn as a translation patch over whatever else
  patched the layer, after the scroll's (`scroll_patches`), and counted in hit testing, the
  hotspots and `drawn_shift` through `child_offset`, so it is hit, and an overlay is placed from
  it, where it is drawn.

## A scroll effect

A layer whose `LayerPlay::scroll_effect` is set changes with the offset of the nearest scrolling
frame above it: the vertical offset, or the horizontal one for a frame that scrolls sideways only.

- **Progress** is linear across `from..to` and held at its ends outside it; a range of no length
  steps at `to`. Each listed property goes from its start to its end value; one not listed stays
  at rest.
- **Drawn** as a patch after the scroll's and the drag's (`scroll_effect_patch`): the opacity, clamped
  to 0–1, in place of the layer's own, then a move, and a turn and scale about the layer's centre,
  over whatever placed it.
- **Hit testing follows it** (`effect_now`, shared with the patch): a point is taken back through
  the move, turn and scale before it is tested against the layer and what it holds. The
  hotspot hints stay where the layer is placed.
- The layers with an effect are listed once (`Player::effects`, by `prepare`) as play starts and
  after a live edit, so a frame's scene does not look for them.

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
  and sheared, each interpolated), box size, opacity and corner radii (a polygon's or star's one)
  interpolate; so do fills and strokes, brush by brush, when both ends have as many (and stroke
  widths), and effects when both stacks are the same kinds in the same order (shadows' offset,
  blur, spread and colour; layer and background blur radii). Two gradients of one kind move
  their geometry, transform and opacity, their ramps sampled at every offset either has; a
  colour against a gradient reads as a flat ramp of its kind (`lerp_brush`). An image, two
  kinds of gradient, a stack that differs and other effect kinds jump to the end's. The frame's
  own paint interpolates too. A
  pair's start is measured where it is drawn less the end's scroll, which `scroll_patches` adds
  over the tracks, so a kept scroll is not counted twice. With no side (`Direction::None`) the unpaired layers fade: the end's in, the
  outgoing frame's out. Under a named side they move by it, a screen's width or height: the
  end's slide in from the far side to their places, the outgoing frame's slide out toward the
  side, as *push* moves frames, none of them fading. A move, push or slide has no *None* and
  reads one as the left side. The outgoing frame is drawn over
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
scroll offsets, each instance re-switched to the main play left it on, and the modes *Set mode*
gave. Motions under way end; a drag's settle ends where it was going and its drops run. After any
edit, how far a layer was dragged, its settle and its pending drops are kept only while the layer
still follows the drag.
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
runs it on a pinned nightly that unwinds (`prototyping-export.md` *The browser build*). The plain
player is built without `scripts` and skips a *Run script* with `ScriptsOff`; the scripts
player, packed only into a file that runs a script, is built with it, Luau's C++ compiled by
wasi-sdk.
