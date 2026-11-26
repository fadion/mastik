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
triggers last saw; pending timers; motions under way; notices, the script log and requests for the shell;
each table play wrote (a row action or a script, noted in `follow_tables`), with the document's
table as it stood at the first write.

## The shell's contract

Everything is in **screen space**: the screen frame's box with its top-left at the origin, one
unit a document pixel. The shell scales and places that box by the page's *Scaling* and maps
pointer events back into it.

| Call | What it does |
| --- | --- |
| `Player::new(doc, Start::Flow(r) \| Start::Frame(f), actor)` | Start; `StartError` when the flow or frame is not there |
| `input(Input) -> bool` | Pointer move, down, up, leave; wheel; a key; text; a state-card value or mode; `Touch(bool)`, whether the pointer is a finger; `PlaySize`, as `set_play_size`. Answers whether the prototype used it, so an unused Escape goes back to the editor |
| `tick(ms)` | Advance time: timers, held clicks and long presses fire at their own times, motions move on |
| `scene() -> Scene` | What to draw now |
| `hotspots()` | The rectangles of the layers that take a person's act in the frame that takes input, for the hints |
| `values()`, `set_value(var, value)` | The state card's rows, and setting one (recorded in a take; reactions to the change fire) |
| `modes()` | The state card's modes (`ModeRow`): each collection with two modes or more, the document's then each kit's, the mode the screen reads, and whether the screen sets one itself. A pick is `Input::SetMode`, set everywhere and recorded in a take |
| `opened_rows()` | The state card's *Opened rows* (`OpenedRow`): each data set an *Opened row* list reads, its rows by first cell and the open one. A pick is `Input::OpenRow`, opened as a tap opens a row and recorded in a take (`lists.md`, *Play*) |
| `tables()` | The state card's *Tables* (`TableRow`): each table play wrote this run, its rows in play and in the document. `Input::ResetTable` puts one back as the document has it, recorded in a take |
| `set_play_size(size)`, `play_size()` | Lay every screen out at `size` in place of its own (a page's device, or the room under *Responsive*; `PagePlay::play_size`), `None` as drawn: the copy's frame is given that size (`fit_screen`, a `SetGeometry` on the copy) as play starts, before a navigation's smart animate measures it, after a live edit and across a restart, so its flex, grid and width queries answer as on a real screen of that size. A frame with no layout keeps its children where drawn. Overlays keep their own size |
| `width_pick()` | The collection picked by width and the mode the screen's play width picks, by name, while it differs from the one its drawn width picks (the base document's `width_mode`); `None` where the screen does not pick by width (`Document::width_pick`: a pin, a component). The player's mode chip |
| `rebase(&doc)` | Play `doc` in place of the document the player started from, keeping the run's place as a refused live edit does (the copy cloned again, play state carried by id, what `doc` lacks falling back): for a shell sent the document whole, a phone mirroring the app, whose trimmed copy cannot take the editor's transactions |
| `apply_edit(&tx)` | Live update: an editor commit |
| `take_notices()`, `take_requests()`, `log()` | What to say once; what to do (open a URL); scripts' prints and failures |
| `take_dirty()` | Nodes of the copy whose drawing changed, for the renderer |
| `document()`, `resolved()` | The copy to draw, and its derived state |
| `screen()`, `page()`, `overlays()`, `history()`, `scroll_offset(f)`, `moved(id)`, `focused()`, `now()` | Play state, read-only |
| `is_moving()` | Whether anything changes without input; a shell may stop ticking while false |
| `is_animating()`, `next_due()` | Whether a transition or the hints' flash changes the screen every frame now; else when, in play's clock, the next timer or held gesture is due, so a shell waiting on one draws once then, not every frame |
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
  A pointer reaction firing inside a list's row opens that row before its actions run
  (`lists.md`, *Play*).
- **A click goes to the deepest layer with *On click* or *On double click*.** When that layer has
  a double click, the click is held `DOUBLE_MS` (250 ms): a second click on the same layer within
  it fires the double click and no click; otherwise the held click runs as a click on what was
  under the first (at once, if the next click lands elsewhere). A layer with only *On click* is
  never delayed, so only a layer that asks for a double click pays for it.
- **A long press** is due its `ms` after the press on the deepest layer with one, and fires while
  the press is held and has not become a drag; it spends the press, so no click follows. A drag
  cancels it. Held clicks and long presses are timed through `tick` as *After* timers are, and
  `is_moving` stays true while one waits.
- **Hit testing is by box**, not ink: a layer with a box (a frame, a shape) by its box, turned
  with it (`Player::hits`, so a click in the corner of the square around a diamond misses), any
  other by its bounds, each moved by the scrolling frames above it and by how far it and the layers
  above it were dragged (`child_offset`), then taken back through a scroll effect's move, turn
  and scale (`walk`, from the box's centre as drawn). A group is hit only through its
  children. A clipping or scrolling frame cuts off what is outside it. A click on the box that
  holds an input text focuses it, since an empty text has no width.
- **Overlays are modal.** A press outside the top overlay closes it when its *close when clicking
  outside* is on, and reaches nothing beneath either way.
- *Hover enter* runs once per layer the pointer enters, *On leave* once per layer it leaves (the
  player included); *while hovering* and *while pressed* record what their actions did and put it
  back, newest first, when the pointer leaves or the press ends: a variable's previous play value,
  an instance's previous main, overlays opened, a navigation (one *Back*), a *Set mode*'s mode of
  its collection. A hover lasts while the
  layer is under the pointer or the pointer stays where the layer was when it began
  (`While::area`), so a *while hovering* navigation or modal overlay, which takes the layer from
  under the pointer, ends when the pointer moves off that place. Whiles that end together, the
  hovers a move ends or the presses a release ends, are put back newest first. One that ends while
  a later one that changed the same variable, instance or collection's mode on a node goes on
  hands what it found to that one instead of writing over it (`hand_over`), the oldest value
  winning: after a scroll moves card B under a pointer still on card A's place, A's end leaves
  B's value or mode, and B's end puts back what A found. *On leave* fires only
  for a layer still in a shown frame — one that went with its frame is gone, not left — and not
  in a *Disabled* instance.
- **A finger hovers nothing.** While the shell says the pointer is a finger (`Input::Touch(true)`),
  hovering reads as if the pointer were gone (`rehover`), so no hover trigger or *Hover* state
  fires before a tap, as on a phone; switching hands hovers again from where the pointer is. The
  shell tells it from each event: the app's player from egui's touch events against plain pointer
  moves, the exported page from `pointerType` (`set_touch` in `mastik-web`). `Touch` is recorded
  in a take like any input, and kept across `restart`.
- A **drag** is a press that moves more than four units. A swipe reaction anywhere in the pressed
  chain toward the side the move is going (its longer axis) takes it whole. Otherwise the deepest
  layer in the chain that follows the drag moves with it (*A layer that follows the drag*) and a
  drag reaction on the chain fires as well; the nearest scrolling frame in the chain that scrolls
  the way the drag goes (its longer axis at the slop), else the nearest scrolling frame, scrolls
  with it only when neither took it. So a card can take left and right while its list scrolls up
  and down, and a vertical drag on a horizontal carousel scrolls the page beneath it.
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
  *After* restarted this way, or started by a frame a timer's reaction opened, waits at least
  16 ms, so two states or two screens changing to each other after nothing still let time move,
  and one `tick` fires at most 1024 timers, the loop going on from the tick's end.
- *Frame opens* runs when the screen or an overlay opens; *variable changes* runs on every shown
  frame's layers when a play value changes what the variable reads; *table changes* runs on every
  shown frame's layers after each write that changes the table's rows: a row action, or a
  *Reset* from the state card. A script's write does not fire it.
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
- **Navigate** closes every overlay and pushes the history, which keeps the last 1000 visits
  (`MAX_HISTORY`); a *while* trigger's navigation is recorded by how deep the run had gone
  (`Player::depth`, which counts the visits trimmed), so it still comes back once the history is
  full. The player keeps every screen's scroll
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
  increment applies to numbers and toggle to booleans, and anything else is skipped. A value of
  another kind than the variable's is skipped with an `Expression` notice: a table's cell has its
  kind only in play, so the check is made here as well as in the field. A string variable set
  from one cell read alone takes the cell as written (`TableEnv::written`), as a row action's cell
  does, so `00731` is not refused as a number. A value that
  does not change what the variable reads fires nothing.
- **Set mode** commits `SetModes` on the copy in place of any mode of the same collection there:
  on every page root for *Everywhere*, so a frame that sets its own mode keeps it and a by-width
  collection is held at that mode; on the screen frame for *This screen*. A page holds no kit's
  mode, so *Everywhere* puts a kit's on each top-level frame instead, passing over one that pins
  a mode of that kit collection in the document (`Player::everywhere`). The player remembers
  the mode it gave each node in each collection it set, and no other (`Player::modes`), so a live
  edit puts those back, re-cloned or not, while the edit's pick in any other collection shows
  (`modes_owed`). A *while* trigger's end puts back that collection's mode and what the player
  remembered before (`Undo::Modes`), so it claims no mode it has given up. A mode in neither the document's collections nor a kit's is skipped
  with `Missing`. Nothing animates. *Restart* and a new flow start from the document's own modes
  and the flow's starting modes, which play sets as *Everywhere* does before anything opens.
- **Conditional** runs the first branch whose condition is true, else `otherwise`; a condition
  that errs or is not true or false is skipped with a notice.
- **Open URL** asks the shell (`Request::OpenUrl`) for `http` and `https` addresses only.
- **Run script** runs the document script through `mastik-vm` on the copy, under a person's act
  only (the door's rule, checked again). The selection is empty, no labels, no input form — a
  script whose header asks for input is not run, with a notice — and `print` goes to the log.
  The run's host answers `Host::play`, so the script reaches the play state as `play`
  (`script.rs`, `PlayHost`): `play.vars.Name` reads what the variable reads at the reaction's
  layer, its play value first, and `play.vars.Name = v` gives it a play value as *Set variable*
  does, a variable named as an expression names it and a value of another kind refused at its
  line; the write is seen by the run's later reads at once. Once the run ends, each variable it
  wrote whose reading changed has *Set variable*'s follow-up (`Player::changed`: inputs, filters,
  *When a variable changes*), and a *while* trigger's end puts its value back (`Undo::Value`).
  `play.row` is the row the reaction fired in, its cells by column name, when the holder sits in
  a list over a table (`list::row_at`, as *This row* finds it), read-only and nil elsewhere.
  `play.go("Frame")` names a top-level frame of the current page (or a frame by id), and the
  player navigates there after the run with the page's transition, as *Navigate* would, the last
  call winning. The exported page's scripts player runs the same `Player`, so it reaches the same
  `play`. A run goes under `PLAY_BUDGET`, steps and memory with no clock, so the app, the
  exported page and a take's replay stop a runaway at the same step. Host work, which runs in
  Rust between interrupts, is charged to the steps (`Budget::host`): each layer a call reads or
  writes, each operation committed and each `play` call costs 1,000, and each table cell copied
  or layer passed over when a bare name is looked up costs 10 and each layer handle built 100.
  So a loop of layer or `play.vars` writes, of `children` reads or of a table's `column` ends at
  the budget rather than holding the UI thread for minutes. A bare name's lookup is kept until the
  run's next write. The memory
  limit counts the allocator's bytes, which depend on the target's pointer width, so a run near it
  may stop on the desktop and finish in the exported page (wasm32); that is the one bound not
  shared, and a take's replay on the same machine still agrees. The copy, its resolve and
  the play values are snapshotted first and put back whole if the run fails, which then fires no
  change and navigates nowhere. A run that
  wrote a table ends with `Player::follow_tables` over the tables it wrote: every chart over one
  brought onto its table, every list over one made again on the copy, as the editor follows a
  table edit, and each list's template hidden or shown as it now draws rows or none (`lists.md`,
  *Play*); `Player::commit` does the same for any transaction writing a table. Behind the
  default `scripts` feature; without it the action is skipped with `ScriptsOff`.
- **Add row, Edit row and Delete row** (`rows.rs`) write the copy's table through
  `Player::commit`, so its lists and charts follow; nothing reaches the document, the save or
  undo. A linked table is edited in play like any other: refusing an edit to a linked table is
  the document's rule, not the played copy's. Each cell is worked out from the row as it was
  before the edit, written as text and cut at the cell limit. A cell that is one cell read alone
  (`{Products.opened.Price}`, `{row.Qty}`) copies that cell as written, since read as a number
  `12.50` or `007` would lose its spelling (`TableEnv::written`); a cell that does not evaluate skips
  the action with an `Expression` notice, and a column the table lacks is passed over. *This
  row* is `list::row_at` on the holder and must be a row of a list over the action's table, else
  `Missing`; *Opened row* is the row the data set has open, row 1 until one is opened; *First
  row where…* tries each row with `{row.…}` reading it, and none holding skips the action
  silently, as an empty table does. *Add row* on a table at the row limit is skipped with
  `TableFull`. A missing table is `Missing`. After each write, *When a table changes* reactions
  fire (*Triggers*). *Restart* puts every table back.
  A write makes its lists again with new copies, so the rest of the reaction, and the layer's
  further reactions, run on the layer at the holder's place in the row drawing the same table row
  (`Player::row_path`, `Player::still`). *Add row at start* and *Delete row* move the table rows
  after them, and the row is followed through the move. Once the row is deleted, or a filter or a
  sort takes it out of the list, the actions left that act on the row (*This row*, *Change to*,
  *Set mode*, *Scroll to*, *Conditional*, *Run script*) are skipped with `Missing` and the rest
  still run, so *Delete row; Back* goes back; the reactions left do not run. Nothing lands on the
  row now drawn in its place. A list made again in play takes up its new copies as a frame's
  opening does (`take_up_copies`): their inputs and scroll effects are found, and in a shown frame
  their *After* reactions start, at least `MIN_LOOP_MS` out, so a looping row state goes on
  looping and a row *After* that writes its own table lets time move. Every copy's *After* starts
  again from then, one that already ran included.

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

A layer whose `LayerPlay::scroll_effect` is set changes with the offset of its frame: the one it
names, read through `resolve_target`, or else the nearest scrolling frame above it; the vertical
offset, or the horizontal one for a frame that scrolls sideways only. A named frame that is gone,
or no scrolling frame above, means no effect (`effect_now`).

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
  name, or for an instance the lineage key of its main — of the main's set when the main is a
  variant, since variant roots share no lineage, so a toggle showing *Off* in one frame and *On*
  in the next morphs — and which of its siblings with that key it is, so a pair's parents are
  paired too. Both ends are laid out
  when it starts and never again. A paired layer's transform (moved, turned the short way, scaled
  and sheared, each interpolated; a mirror at one end only is a scale through zero, not half a
  turn), box size, opacity (a side that hides the layer reads as 0, so one shown only at the end
  fades in, and one the end hides is drawn while it fades out) and corner radii (a polygon's or
  star's one)
  interpolate; so do fills and strokes, brush by brush, when both ends have as many (and stroke
  widths; colours mix premultiplied), and effects when both stacks are the same kinds in the same order (shadows' offset,
  blur, spread and colour; layer and background blur radii, and a ramp's end radius and ends, the
  ease the end's, an even blur moving as a ramp that holds its radius; every glass number and its
  rim colour, held to range). Two gradients of one kind move
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
copy. Edits are absolute, so one the copy takes can write over play's own choices: after it, each
node whose modes no longer hold what *Set mode* gave it is given those modes again, its other
collections' modes as the edit left them, and the screen its play size (`put_play_back`). When the copy refuses it — play changed something the edit touches — the copy is cloned again
from the base and play state carried across by id: the screen, history, overlays, play values,
scroll offsets, each instance re-switched to the main play left it on, and the modes *Set mode*
gave. Motions under way end; a drag's settle ends where it was going and its drops run. After any
edit, how far a layer was dragged, its settle and its pending drops are kept only while the layer
still follows the drag.
Then what the edit removed falls back, one notice each: a deleted screen to the start frame (else
the first frame of the document), a deleted overlay closes, a deleted variable's play value goes.
A start the edit took away moves first, without a notice, so *Restart* and *Record* begin where the
run now can: a deleted flow's to the frame it began at, a deleted start frame's to the first frame
of the document. `record()` records nothing when play cannot restart.
A variable whose starting value was edited keeps its play value.
A table play wrote, by a row action or a script, is taken before the edit (`played_tables`) and afterwards
(`carry_tables`, through a re-clone or `rebase` too) gets its played rows back while its document
table is still the one the first write started from; one the edit changed is the document's again,
its played rows dropped rather than replayed onto the new table.

## Takes

`record()` restarts play and records each input with the time it arrived; `stop_recording()`
answers the `Take` (start, inputs, length). The take opens with the play size and the hand the
restart kept (`Input::PlaySize`, `Input::Touch`), and `set_play_size` records each change of
size as `PlaySize`, so a replay lays each screen out and hovers as the run did.
`Replay::new(doc, take, actor)` plays it on a fresh copy, giving that opening size and hand at
once so its first frame is laid out as the run's; `advance(ms)` gives each input due in that span at its own time, so a replay stepped frame
by frame for a video reaches exactly the state the run had at each time.

**Video** (`mastik_export::video::video(doc, take, actor, options, progress)`) replays a take on
a fresh copy at a fixed rate (1 to 60 frames a second), a frame at every frame time from the start
to the take's length, each drawn by `VelloCpuRenderer::render_play_to_rgba` over white at one
scale for the whole video: the options' scale, lowered for the first screen so its frame, bezel
included, is at most 4096 pixels a side and, as an MP4, within 3840 × 2160 either way round
(OpenH264 refuses a larger frame) and at most H.264's largest frame (36,864
macroblocks, and level 5.2's macroblocks a second at the frame rate), since OpenH264 labels a
larger one 5.1 anyway and a decoder that keeps to the level refuses it (`frame_size`, which the
export card shows). Every frame is cut or padded to the first one's size. With `clicks`, a ring
in the accent widens and fades over `CLICK_MS` (500 ms) where each press landed; with `device`,
the screen sits in the exported page's device frame (`in_device`: a `BEZEL` of 14 units, screen
corners 40 under 600 units on the short side, else 18) on white. `still` draws one such frame,
for the export card's preview. A GIF loops, each frame on its own palette, its delays carrying
their rounding so they add up to the take; a frame the same as the one before adds its delay to
that one rather than being written again, so a still stretch costs one frame. An MP4 is H.264
from OpenH264 built from source (BSD-2-Clause; the C++ toolchain `mlua` already needs builds it,
and its patent licence covers only Cisco's own binaries), at even sizes, muxed by the `mp4` crate
with one sample a frame. `progress` hears each frame and stops the job by answering false.
`video_to` writes into any `Write + Seek` as it goes, a GIF frame by frame, so the app streams a
take into a `.part` file beside the chosen one and renames it once whole: a long take is never
held in memory, and a cancel or failure leaves the chosen path as it was.

## Notices

`Missing`, `Expression` and `TableFull` (*Add row* on a table holding the most rows a table
can) are said once per action; `Script` and `ScriptNeedsInput` per run;
`ScriptsOff` and `Loop` once; `Fallback` (screen, overlay, variable) once per live edit that caused
it; `ScrollFrameGone { layer }`, a scroll trigger whose frame is gone and so never fires, once per
reaction, the first time its screen scrolls. The app and the page word it *A scroll trigger
watches a frame that is gone; it never fires*; the page, as with every notice but `ScriptsOff`,
says it to the console.

## Expressions

`prototype::Expr` holds a condition, *Set variable*'s value, a row action's cell or a *First row
where…* test as source text, everything it names stored by id (`expr::Ref`,
`prototyping-model.md`).

- **Values**: numbers (`12`, `0.5`), text in double or single quotes with `\` escapes, `true`,
  `false`, variables of any kind (a colour can be read and compared, not computed with), and
  table reads. The field shows them `{Cart.rows}`, `{Products.3.Name}`,
  `{Products.opened.Price}`, `{Cart.Line.sum}` and `{row.Qty}`.
- **Table reads in play** (`TableEnv::read`): a count is a number; a cell is a number when
  `chart::parse_number` reads it, else text; an aggregate takes the column's number cells and
  skips the rest, an empty column summing to 0 and min, max or avg of no numbers an error. Errors
  say why: *Products has 2 rows, not 3*, *Cart has no rows* (first, last or opened of an empty
  table), *row is read only inside a row action*. `opened` is the row the data set has open, row
  1 until a tap or the state card opens another. A list's filter value reads variables only; a
  table read there does not evaluate and passes.
- **Operators**, loosest first: `or`; `and`; `not`; one comparison (`==` `!=` `<` `<=` `>` `>=`, not
  chained); `+` `-`; `*` `/` `%`; unary `-`; parentheses. `+` joins when either side is text, a number
  written whole or with at most three decimals; `==` and `!=` need two of one kind, save a number
  and a text, which are equal when the text reads as that number (`parse_number`), so a column
  mixing sizes `8` and `M` tests either way; `<`-style
  comparisons take two numbers or two texts; `and`/`or`/`not` take booleans and short-circuit.
  Division by zero is an error.
- **Depth**: parentheses, `not`s and unary `-`s nest at most 16 deep (`MAX_NEST`), and the tree
  goes at most 64 operators deep, a chain counted (`MAX_HEIGHT`). Past either the expression is
  a syntax error, so one from a file or an agent cannot overflow the stack as it is parsed,
  kinded, evaluated or dropped.
- **Errors** carry the byte range they are about (`ExprError::at`): syntax, a type, a variable
  not there, a name nothing has, a table or column not there (`MissingTable`).
- **The field** shows names: `with_names_in(&Names)` writes `{<name>}` for each id it can name, and
  `from_names_in(text, &Names)` turns a typed `{<name>}` (or an id) back into the stored form,
  answering the first error in the typed text so the draft is kept and underlined
  (`with_names` and `from_names` are the variables-only forms a list filter uses). A typed name
  is resolved against the real table and column names (`Names::resolve`), since a name may
  hold a dot (an imported table is named after its file, `products.csv`): every reading is
  collected, and a name that reads two ways is refused with *pick it from the list { opens*. A variable
  name is looked up among every variable, the kits' among them (`Variables::ids_named`); a name two
  variables share is refused at both doors rather than given to the first, and is shown by its id
  (`Variables::expr_name`), which reads back; so is a name holding `}` or leading or trailing
  space, which the braces cannot hold. A brace inside quotes opens no name, for
  `var_tokens` as for the parse. `row` is
  reserved: no table takes it (`list::free_name`; the loader renames one). `kind(kind_of)`
  answers the kind an expression comes to without evaluating it, `None` when it is known only in
  play (`table_kind`: a count and a total are numbers, a cell has no kind until play), for
  checking *Set variable*'s value and a condition, so authoring never refuses a cell for its
  kind. `var_tokens` lists the `{…}` spans for highlighting; `prototype::expr_shown` shows an
  expression by name for the flow map and the app.

## Building for the browser

`mastik-play` depends on `mastik-core` and, behind `scripts`, `mastik-vm`; nothing renders or
draws (`deps_forbidden` holds it to the headless list). The page's player (`crates/mastik-web`)
runs it on a pinned nightly that unwinds (`prototyping-export.md` *The browser build*). The plain
player is built without `scripts` and skips a *Run script* with `ScriptsOff`; the scripts
player, packed only into a file that runs a script, is built with it, Luau's C++ compiled by
wasi-sdk.
