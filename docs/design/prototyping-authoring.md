# Prototyping: authoring

Where a prototype is made: the inspector's Prototype tab, the connectors and the link knob on the
canvas, an instance's interactions, kit publishing, and what agents read and write. The saved model
it edits is `prototyping-model.md`; the player is `play.md`. Code:
`crates/mastik-app/src/panels/prototype.rs`, the knob's drag in `canvas.rs` (`Drag::Link`).

## The Prototype tab

- **A tab of the inspector, not a mode.** `MastikApp::inspector_tabs` heads every inspector state
  with `ui::segmented` *Design* | *Prototype*, a card of its own; the palette's *Prototype tab*
  (`ViewSwitch::PrototypeTab`, `icon::INTERACTION`) flips it. **No chord**: the keymap's chords
  are spent, and a prototyping chord would mean two things. Tools, selection and the layers panel
  are unchanged, so a frame can be nudged without leaving the tab. The tab is the window's
  (`MastikApp::prototype_tab`), never saved.
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
  the value held in `ProtoState::scrub` and commits once when the field lets go.

## Interactions

- **One block per reaction**: the trigger's dropdown, `+` (add an action) and `⋯` (*Duplicate*,
  *Delete*), then its actions top to bottom. The card's own `+` adds *On click* with no actions.
- **Triggers** are offered in two groups, the pointer's (*On click*, *On press*, *While
  pressing*, *On hover*, *While hovering*, *On drag*) and the rest (*Key…*, *After delay*, *When
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
  first variable or script. It opens its popover at once.
- **An action is one row**: a grip, its glyph, its verb, its target in accent ink, its
  transition as a chip (kind and duration, or *Spring*), and `×`. A row drags by its grip to
  another place in its own list (a reaction's, or one branch's), one *Move action* step; the
  grip is inert while the list holds one action. A target that is gone is drawn in the error
  colour behind the warning glyph (*Missing frame*, *Missing layer*, *Missing variable*, …), kept
  so it can be mended, and skipped while playing. A conditional's branches follow it as heads in
  mono accent (*if …*, *else if …*, *else*), each with its own `+`, their actions indented under
  them. A target is read as the layer holding the reaction reads it
  (`prototype::resolve_target`), so an instance's names its own layers.
- **The row's popover**, in the popover lane: the verb's head with *Move up* / *Move down* and
  close, the action's fields, then its transition. A screen action picks a top-level frame, page
  by page; *Scroll to* a layer of the holder's screen; *Change to* a variant of the instance's
  set; *Set variable* a variable, then *Set* / *Increment* / *Toggle* (*Increment* for a number,
  *Toggle* for a boolean) and its expression or step; *Conditional* one expression per branch,
  *Add else if* and a branch's `×`; *Open link* an address, refused unless `http` or `https`;
  *Run script* a document script by name.
- **Expressions show names.** The field writes a variable `{name}`
  (`Expr::with_names`) and stores its id (`Expr::from_names`), so a rename never breaks it. What
  does not read, names no variable, or (for *Set variable*'s value and a condition) comes to
  another kind than wanted (`Expr::kind`) is kept as typed, ringed, with the reason under the
  field (`ui::field_refusal`) and the range `ExprError::at` names underlined
  (`ui::field_underline`), and nothing is written until it reads.
- **Transitions** (`transition_fields`, also the page's *New links*): the kind; a direction
  segment of *None* and the four sides. The sides are live for *Move in*, *Move out*, *Push*
  and *Slide* (`TransitionKind::is_directional`) and for *Smart animate*; *None* is live for
  *Smart animate* alone, whose unpaired layers fade under it and slide in from a side otherwise
  (`play.md`, *Motion*); every other kind dims the whole segment. Picking *Smart animate* sets
  *None*; picking a move, push or slide while *None* is set sets the left side. Then *Curve* |
  *Spring*. A curve is an easing
  from the CSS set or *Custom curve* with its four cubic-bezier numbers (both X within 0..1), and a
  duration; a spring is mass, stiffness and damping. *Instant* shows the kind alone.

## The Prototype card

- **A frame**: *Scrolling* (*None*, *Vertical*, *Horizontal*, *Both*), the children pinned in it
  listed under it (*Top bar · Fixed*); *As an overlay*: its position (the centre, the top and
  bottom thirds' left, centre and right, or *Manual* with X and Y), *Close on a click outside*, *Darken behind it* (black at 40% when turned on), and
  while it is on the shade's colour (`ui::colour_text_field`, its alpha kept) and *Shade*, its
  opacity.
- **A child of a frame**: *In a scrolling frame*, *Scrolls* / *Fixed* / *Sticky*, dimmed with
  *The frame does not scroll* while its parent does not. The pin is set here, on the child's own
  card, rather than in the layers panel's menu.
- **An input**: a text layer binds a string variable; an instance a boolean one and the set's
  two-valued property that shows it. `×` unbinds.
- **The page** (nothing selected): *Device* (*None* or a phone or tablet frame template by name),
  *Scaling* (*Fit*, *Actual size*, *Fill width*) and *New links*, the transition a new link takes.
- ***Flows***: one row per flow, its name (a `ui::Draft`, committed on leaving the field) and its
  start among the page's frames, `×` to remove; `+` adds *Flow N* starting at the page's first
  frame, dimmed with no frame. Under each flow, its starting values: a row per value, the
  variable (a variable dropdown; a new pick starts at its first mode's value), the value typed as
  the variable's kind reads it (a number, text, `true` / `false`, a colour, or `{name}` for
  another variable of that kind; refused and ringed when it does not read) and `×`; *Add a
  starting value* adds the first variable not yet named, dimmed when every one is.

## On the canvas

- **Connectors** (`draw_prototype_links`), only while the Prototype tab is up and the chrome
  shows: from each layer's box (right edge) to each navigate or overlay target's box on the same
  page (`prototype::links_on`), a level-tangent curve with an arrowhead, the selection's in full
  accent and the rest dim. A link in a conditional or opening an overlay is dashed. A target that
  is gone is a short red stub ending in the warning glyph; one on another page a short stub. They
  are egui shapes over the rendered page, recomputed from the document, never in the scene.
- **The knob**: an accent dot `KNOB_GAP` right of the one selected layer's box, with a selecting
  tool. A drag from it (`Drag::Link`, ahead of the box handles, and in `chrome_claims`) draws the
  link to the pointer and rings the screen under it; released over a top-level frame other than
  the layer's own, it commits one *Navigate to* with the page's default transition as *Add
  interaction*, joining the layer's first click (`prototype::with_link`). A click on it links
  nothing and says how to use it; a drop elsewhere says where to drop.
- **Flow chips** (`flow_chips`), while the Prototype tab is up: each flow's name behind the
  `PLAY` glyph on an accent pill over its start frame's top-right corner, several on one frame
  stacked leftward. A click plays that flow over the canvas (`prototyping-player.md`). Private
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

- **The knob is a press target the canvas must know.** `link_knob_at` answers in
  `begin_select_drag`, `chrome_claims` and the click path; left out of any of them, a press on it
  starts a move, a guide, or a pick that drops the selection.
- **The Prototype tab must not draw the Design cards' state.** Switching closes the inspector's
  popovers and a key capture, since both are keyed to cards the other tab does not draw.
