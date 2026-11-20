# Prototyping: authoring

Where a prototype is made: prototype mode's inspector cards, the connectors and the link dots on
the canvas, an instance's interactions, kit publishing, and what agents read and write. The saved
model it edits is `prototyping-model.md`; the player is `play.md`. Code:
`crates/mastik-app/src/panels/prototype.rs`, a link dot's drag in `canvas.rs` (`Drag::Link`).

## Prototype mode

- **A workspace mode, not an inspector tab.** It changes the canvas (connectors, link dots, flow
  chips, the hover outline) as well as the inspector's cards, so it is a `ViewSwitch` beside
  outline, focus and private mode (`ViewSwitch::PrototypeMode`, *Prototype mode*). Its switch is
  an `icon::INTERACTION` icon button in the top bar, immediately left of *Play*, so authoring and
  playing are one spot; lit while on, its tooltip *Prototype mode*, or *Leave prototype mode*
  while lit; a click calls `set_prototype_mode`, which says *Prototype mode on* / *off* on the
  status line. The palette's row flips it too. **No chord and no menu**: the keymap's chords are
  spent, and a prototyping chord would mean two things. Tools, selection and the layers panel are
  unchanged, so a frame can be nudged without leaving the mode. The mode is the window's
  (`MastikApp::prototype_mode`), never saved.
- **What it shows.** One layer: its identity (glyph, name, a screen's size or *Instance*), the
  *Interactions* card and the *Prototype* card. Nothing selected: the page card, *Flows* and the
  page's *Prototype* card. Several layers: *Prototype one layer at a time*.
- **Every edit is one transaction** on the saved model through `commit_edit`, named as its
  neighbours: *Add interaction*, *Change trigger*, *Add action*, *Change action*, *Set
  transition*, *Move action*, *Remove action*, *Duplicate interaction*, *Remove interaction*,
  *Scrolling*, *Overlay*, *Pin*, *Input*, *Add flow*, *Rename flow*, *Change flow*, *Remove
  flow*, *Add starting value*, *Change starting value*, *Remove starting value*, *Device*,
  *Scaling*, *New links*. A list the door refuses (`prototype::check`) is not
  written and the status line says why.
- **Numbers are held, not previewed.** The model's operations draw nothing, so a scrubbed or typed
  number (a delay, a duration, a spring, a cubic's handles, an overlay's position or shade) shows
  the value held in `ProtoState::scrub` and commits once when the field lets go. A right-click
  during the scrub calls it off and the field shows the saved value again (`proto_hold`,
  `cancel_gesture`).

## Interactions

- **One block per reaction**: the trigger's dropdown, `+` (add an action) and `⋯` (*Duplicate*,
  *Delete*), then its actions top to bottom; a `ui::card_rule` parts one block from the next. The
  card's own `+` adds *On click* with no actions. The add and `⋯` menus are as wide as their
  longest row (`ui::menu_width`).
- **Triggers** are offered in two groups under `ui::menu_eyebrow`s, *Pointer* (*On click*, *On press*, *While
  pressing*, *On hover*, *While hovering*, *On drag*) and *Other* (*Key…*, *After delay*, *When
  a variable changes*, *When the frame opens*). *When the frame opens* is offered on a top-level
  frame only (*Only a frame opens*); *When a variable changes* needs a variable. *After delay*
  adds a delay field, *When a variable changes* a variable dropdown. Changing to a trigger no
  person fires is refused while the reaction holds *Run script*.
- ***Key…* captures the next key**: a focused field, *Press a key*, takes the next plain or
  `Shift`-ed key as the trigger (its `egui::Key` name). A `Ctrl` or `Alt` chord is refused, those
  staying the editor's while playing; `Escape` or a click away gives up.
- **The add menu** groups the actions by what they move: *Go* (*Navigate to*, *Back*, *Scroll
  to*, *Open link*), *Overlay* (*Open*, *Swap*, *Close overlay*), *State* (*Change to*, *Set
  variable*, *Conditional*, *Run script*). What the layer cannot take is dimmed with its reason
  (`prototype::action_refusal`): *Run script* under a trigger no person fires (*A script runs only
  when someone acts*) or with no document script; *Change to* off an instance of a set's variant
  (*Not a component*); *Set variable* with no variable; a screen action with no frame to go to.
  A new action names the likeliest target (`prototype::new_action`): another screen than the
  layer's own, mains last; the layer's own instance and a variant other than its current one; the
  first variable or script. It opens its popover at once, except *Back*, which has nothing to set.
- **An action is one row**, a `ui::field_row_pressable` that opens its popover (*Back*'s opens
  none, having nothing to set, and says what it does under the pointer): a grip, its
  glyph, its verb, its target in accent ink, its transition's timing (kind and duration, or
  *Spring*) as a faint caption note as a menu row's accelerator is, and `×`; the grip and `×` keep
  their own press. The verb and the target share what the timing and `×` leave, the verb first,
  each cut rather than run under the timing. The rows stand `NEST` in from their trigger. A row
  drags by its grip to
  another place in its own list (a reaction's, or one branch's), carried under the pointer with
  its branches from where it was held (`inspector::carried_unit`) while the others make room, as
  an Effects row is (`InFlight::action_drag`); the
  release is one *Move action* step, and a right-click or `Escape` puts it back. The
  grip is inert while the list holds one action. A target that is gone is drawn in the error
  colour behind the warning glyph (*Missing frame*, *Missing layer*, *Missing variable*, …), kept
  so it can be mended, and skipped while playing. A conditional's branches follow it as heads in
  mono accent (*if …*, *else if …*, *else*), each one caption line tall (`BRANCH_HEAD_H`), taking
  `BRANCH_TUCK` of the rows' gap back above and below, its `+` reaching to the close button's box,
  their actions indented under them. A target is read as the layer holding the reaction reads it
  (`prototype::resolve_target`), so an instance's names its own layers.
- **The row's popover**, in the popover lane: the verb's head and close (an action moves by its
  grip alone), the action's fields, then its transition. Opened by a click on a connector, it
  is not closed by that click (`ProtoState::opened_on_canvas`): the canvas runs first, so the
  popover draws on the click's own frame. A screen action picks a top-level frame, each
  page's under its eyebrow; *Scroll to* a layer of the holder's screen; *Change to* a variant of the instance's
  set; *Set variable* a variable, then *Set* / *Increment* / *Toggle* (*Increment* for a number,
  *Toggle* for a boolean) and its expression or step; *Conditional* one expression per branch
  (`ui::labelled` *If* / *Else if*), *Add else if* and a branch's `×`; *Open link* an address, refused unless `http` or `https`;
  *Run script* a document script by name.
- ***Open overlay* and *Swap overlay* show how the frame shows** (`overlay_fields`, one
  *Overlay* step): *Position* (the centre, the top and bottom thirds' left, centre and right, or
  *Manual*), *Close on a click outside*, *Darken behind it* (black at 40% when turned on) and,
  while it is on, the shade's colour (`ui::colour_text_field`, its alpha kept) and *Shade*, its
  opacity. *Manual* adds X and Y, from the top-left of the layer holding the action, and the
  note *Drag the frame on the canvas to place it*: the fields are for the last point, the canvas
  is where it is placed (*Placing an overlay*, below).
- **Expressions show names.** The field writes a variable `{name}`
  (`Expr::with_names`) and stores its id (`Expr::from_names`), so a rename never breaks it. What
  does not read, names no variable, or (for *Set variable*'s value and a condition) comes to
  another kind than wanted (`Expr::kind`) is kept as typed, ringed, with the reason on the
  warning glyph inside the field's right end (`ui::refusable_field`, as an address and a flow's
  starting value are refused) and the range `ExprError::at` names underlined
  (`ui::field_underline`), and nothing is written until it reads.
- **Transitions** (`transition_fields`, also the page's *New links*): the kind; for *Move in*,
  *Move out*, *Push* and *Slide* (`TransitionKind::is_directional`) and *Smart animate*, a
  direction segment of *None* and the four sides. *None* is live for *Smart animate* alone, whose
  unpaired layers fade under it and slide in from a side otherwise (`play.md`, *Motion*).
  *Dissolve* goes no way and has no direction row. Picking *Smart animate* sets *None*; picking a
  move, push or slide while *None* is set sets the left side. Then *Curve* | *Spring*. A curve is
  an easing from the CSS set or *Custom curve* with its four cubic-bezier numbers (both X within
  0..1), and a *Duration*; a spring is mass, stiffness and damping. Fields sharing a line stand
  `ui::CARD_COL_GAP` apart. Under them, **the motion preview** (`motion_preview`): the timing
  as its value over time on the Curves graph's ground, from `mastik_play::motion` so it is what
  plays: the curve in `accent_400`, the target dashed in the divider's ink and the range grown to
  show a spring's overshoot, and under it a lane where a dot moves as the transition would, a
  spring's landing ticked with a dotted line. A spring says *Settles in 640 ms* in the graph's
  lower right, which its curve has left by then; a curve's duration is the field above. Under
  the pointer a dot in the strong text ink plays along the curve and the lane's accent dot along
  the lane in real time, rests `PREVIEW_REST_MS` and plays again; a timing changed
  meanwhile, or a field scrubbed, starts it over from what is held; on the frame a field lets go
  it draws the timing being written, not the saved one. *Instant* shows the kind
  alone. A pick is answered without returning early: every row still draws that frame from the old value, so the popover is never one frame
  short of its rows (an egui popup that shrinks does not grow back).

## The Prototype card

- **A frame**: *Scrolling* (*None*, *Vertical*, *Horizontal*, *Both*), the children pinned in it
  listed under it (*Top bar · Fixed*). How a frame shows as an overlay is its opening action's,
  set in that action's popover.
- **A child of a frame**: *In a scrolling frame*, *Scrolls* / *Fixed* / *Sticky*, dimmed with
  *The frame does not scroll* while its parent does not. The pin is set here, on the child's own
  card, rather than in the layers panel's menu.
- **An input**: a text layer binds a string variable; an instance a boolean one and the set's
  two-valued property that shows it. `×` unbinds.
- **The page** (nothing selected): *Device* (*None*, then the phone and tablet frame templates by name, each group under its
  `ui::menu_eyebrow`),
  *Scaling* (*Fit*, *Actual size*, *Fill width*) and *New links*, the transition a new link takes.
- ***Flows***: one row per flow, its name (a `ui::Draft`, committed on leaving the field) and its
  start among the page's frames, a `ui::field_button` `×` to remove (as every removal in the
  tab is), each flow after the first under a `ui::card_rule`; `+` adds *Flow N* starting at the page's first
  frame, dimmed with no frame. Under each flow, its starting values: a row per value, the
  variable (a variable dropdown; a new pick starts at its first mode's value), the value typed as
  the variable's kind reads it (a number, text, `true` / `false`, a colour, or `{name}` for
  another variable of that kind; refused and ringed when it does not read) and `×`; *Add a
  starting value* adds the first variable not yet named, dimmed when every one is.

## On the canvas

- **Connectors** (`draw_prototype_links`), only in prototype mode and while the chrome
  shows: from each layer's box to each navigate or overlay target's box on the same page
  (`prototype::links_on`). A connector leaves by the middle of its layer's edge that faces the
  target's centre and enters by the middle of the target's edge facing that start
  (`connector_edges`), so a rotated layer's connector starts on its own edge, where its link dot
  is. The edges are picked from the boxes in world space (`world_quad`), so zooming never moves
  a connector to another side; the screen quad's edge of the same index is drawn
  (`screen_edge`). It is a curve tangent to both edges, sampled in pieces no longer than
  `CURVE_STEP` on screen so it stays smooth zoomed in. It is `LINK_W` thick in `color::SELECT`
  for the selection's links and `color::SELECT_DIM` for the rest, with a filled `HEAD_LEN` ×
  2·`HEAD_HALF` arrowhead; the line stops at the head's base so its weight never shows past the
  tip. The line says when it is followed (`LinkLine`): a navigate's solid, an overlay's dashed (6
  on, 4 off), and any in a conditional dotted, round dots the line's width a dash's length apart. A target that is
  gone is a short red stub ending in the warning glyph; one on another page a short stub in the
  connector's ink. A stub leaves by the edge facing out of the frame its layer is in
  (`stub_edge`, in world space: along the axis the layer's centre is further off the frame's, so
  a layer near its frame's middle never flips side as the view zooms; a layer outside any frame,
  or a screen of its own, leaves by its right), the glyph beyond its end the same way.
  Every connector and stub wears a dot where it leaves, a rest link dot's (`POINT`, 1.5 ring)
  ringed in its own ink, so each has a visible start. They are egui shapes over the rendered
  page, recomputed from the document, never in the scene.
- **Link dots** (`knob_dots`), with a selecting tool, on the one selected layer's screen quad and
  the hovered unselected one's: an edge shows the dot at its middle only while the pointer is
  within `KNOB_REVEAL` (28) of that edge as a segment (`edge_distance`), so a box never wears four
  dots at once. At rest a dot is `color::POINT` with a 1.5 `color::SELECT` ring at `KNOB_R`. The
  dot a press would take, or the one being pulled, is always drawn, grown to `KNOB_HOT_R` and
  filled `SELECT` with a `POINT` ring, as a shape's dragged corner dot is; `KNOB_R` sits a size
  above the corner dots so the two handles do not read as one. A press within `KNOB_PICK` of a dot
  takes it ahead of the box's resize handles (`link_knob_at`, after only the pivot), and the
  cursor stays the rest arrow over it; `KNOB_REVEAL >= KNOB_PICK` is asserted at compile time, so a
  dot that answers a press is one drawn. Pulling a hovered layer's dot selects that layer first,
  so the *Interactions* card shows the link. The pulled link leaves its dot along the edge's
  outward direction. While the pointer is anywhere over a screen it would land on (`link_target`),
  its end snaps to the middle of that screen's edge facing the pulled dot (`pulled_end`, by
  `facing_edge`) and the screen is ringed; elsewhere it ends at the pointer. Released
  over a top-level frame other than the layer's own, it commits one *Navigate to* with the page's
  default transition as *Add interaction*, joining the layer's first click
  (`prototype::with_link`). A click on a dot links nothing and says how to use it; a drop
  elsewhere says where to drop.
- **Which unselected layer wears dots** is noted each frame in `ProtoState::knob_hover` by
  `note_link_hover` (from `draw_canvas`): the hover target, or the layer that wore them last
  frame while the pointer is still on one of its dots, since they sit half outside it. The hover
  outline keeps ringing that layer meanwhile (`link_hover`). A press aims at the dots drawn last
  frame.
- **A connector is a press target** (`connector_at`): the pointer within `LINK_PICK` (5 past
  its half-width) of a connector's line, stub or head takes it, the nearest when several are
  near. Under the pointer it is drawn last, in `color::SELECT` (a dangling one keeps the error
  colour) and `LINK_HOT_W` thick, and the hover outline rings the layer holding it
  (`link_hover`), the layer a click selects. A click selects that layer and opens the
  connector's action's popover in its *Interactions* card (`open_link`), so the link is edited
  where it lives. Link dots and a placed overlay's ghost come first; a press-drag on a connector
  starts what it would start elsewhere. The connectors are hit as drawn last frame
  (`ProtoState::links_drawn`).
- **Placing an overlay** (`overlay_placing`): while an *Open overlay* or *Swap overlay* popover
  at *Manual* is open on a layer of this page, the frame it opens is drawn over the page at half
  strength (`PLACING_ALPHA`) at its place, a chrome ghost (`overlay_ghost`, installed by
  `draw_canvas` beside the crop's) built as the frame draws by `mastik_render::ghost_as_drawn`
  (layout's boxes, the cached text and boolean outlines, ids minted under `PLACING_ACTOR` so no
  lookup reaches the frame itself), ringed in `color::SELECT`, heavier under the pointer. A press
  on it drags it (`Drag::OverlayAt`), ahead of the box handles under it, its box snapping as a
  moved layer's does, Shift keeping it to one axis (`snapped_overlay`); the release writes the
  offset in whole points as one *Overlay* step (`drop_overlay`). It covers what is under it: no
  layer there is hovered or picked (`pointed_at`), and no link dot there is taken. A click on it
  does nothing and keeps the popover open. The ghost follows an X or Y field while it is scrubbed or typed
  (`ProtoState::held_at`).
- **Flow chips** (`flow_chips`), in prototype mode: each flow's name behind the
  `PLAY` glyph on a `color::BADGE` pill (the canvas badges' blue, `ui::pill_ink(BADGE)` ink) over
  its start frame's top-right corner, `FLOW_CHIP_PAD_R` (9) of air after the words, several on one
  frame stacked leftward. Under the pointer it lifts by a white wash at alpha 26, as the crop's
  *Done* pill does. A click plays that flow over the canvas (`prototyping-player.md`). Private
  mode masks the names.

## Components

- **Interactions are a keyed list on instances.** An instance's reaction rows wear the override
  slot (`component::item_slot_menu`): the dot when it differs from its main's, `INSTANCE_LOCAL`
  for the instance's own, ↺ under the pointer, and on a right-click *Reset to main* and *Push to
  main*, per item. A reaction the instance removed is a ghost row with *Restore* and *Push removal
  to main*. The card's header counts and resets them as every card does (`card_of`:
  `SetReactions` is *Interactions*, `SetPlay` *Prototype*).
- **Kit publishing** leaves out of a kit main every action whose target lies outside it
  (`kit::left_out`); the publish line names them (*· left out Card's Navigate to Checkout, whose
  target is outside the component*), in the app's popover as in the MCP's `publish_kit`
  (`kits.md`).

## Agents

`set` writes a layer's `reactions` and `play` and a page's `flows` and `play`; `get_node` and the
JSON snapshot read them, and `get_tree` and `insert_subtree` carry a layer's, a target inside
the tree remapped to the layer the tree makes (`mcp.md`, `export.md`). There is no play tool.

## Traps

- **A link dot and a placed overlay's ghost are press targets the canvas must know.**
  `link_knob_at` and `ghost_at` answer in `begin_select_drag`, `chrome_claims`, the Select tool's
  cursor and the click path; left out of any of them, a press there starts a resize, a move, a
  guide, or a pick that drops the selection (and with it the popover placing the ghost). `chrome_claims` must walk the same order as `begin_select_drag`
  (`a_guide_does_not_steal_a_press_the_chrome_claims`).
- **Prototype mode must not draw the Design cards' state.** Switching closes the inspector's
  popovers and a key capture, since both are keyed to cards the other mode does not draw.
