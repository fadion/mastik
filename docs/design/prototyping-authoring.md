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
  unchanged, so a frame can be nudged without leaving the mode.
- **The mode is the document's view, remembered as its camera is.** It lives on the document's
  session (`EditorSession::prototype_mode`), so each tab has its own, and the library's local
  index keeps the ids of documents left in it (`LocalIndex::prototype_mode`, beside `last_view`),
  so a document reopens in the mode it was left in (`reopen_prototype_mode`). Never in the file,
  so switching it leaves the document clean; a document never filed has no id and is not
  remembered until it is.
- **What it shows.** One layer: its identity (glyph, name, a screen's size or *Instance*), the
  *Interaction states* card when it applies, the *Interactions* card and the *Prototype* card. Nothing selected: the page card, *Flows* and the
  page's *Prototype* card. Several layers: *Prototype one layer at a time*.
- **A top-level frame's identity says which flows start on it** (`flow_start_row`): *Starts the
  flow Checkout* or *Starts the flows Checkout, Sign in* as a `ui::selection_note`, or with none a
  *Start a flow here* text button that adds *Flow n* (`free_flow_name`, as the Flows card's `+`
  names one) starting there, an *Add flow* step.
- **Every edit is one transaction** on the saved model through `commit_edit`, named as its
  neighbours: *Add interaction*, *Change trigger*, *Add action*, *Change action*, *Set
  transition*, *Move action*, *Remove action*, *Duplicate interaction*, *Paste interactions*,
  *Remove interaction*,
  *Scrolling*, *Overlay*, *Pin*, *Input*, *Add scroll effect*, *Scroll effect*, *Remove scroll
  effect*, *Dragging*, *Add snap point*, *Rename snap point*,
  *Move snap point*, *Remove snap point*, *Add flow*, *Rename flow*, *Change flow*, *Remove
  flow*, *Add starting mode*, *Change starting mode*, *Remove starting mode*, *Add starting
  value*, *Change starting value*, *Remove starting value*, *Device*, *Turn device*,
  *Scaling*, *Default transition*. A list the door refuses (`prototype::check`) is not
  written and the status line says why.
- **Numbers are held, not previewed.** The model's operations draw nothing, so a scrubbed or typed
  number (a delay, a duration, a spring, a cubic's handles, an overlay's position or shade) shows
  the value held in `ProtoState::scrub` and commits once when the field lets go. A right-click
  during the scrub calls it off and the field shows the saved value again (`proto_hold`,
  `cancel_gesture`).

## Interactions

- **The *Interaction states* card** (`live_states_card`) stands before *Interactions* on a main
  in a set, or an instance of one, whose interaction-state property ties *Hover* and/or
  *Pressed*: a row each, *plays live*, its tooltip naming the value shown. The player switches
  those states with nothing authored (`play.md`), and the card says so, so no one writes *While
  hovering → Change to* for it.
- **One block per reaction**: the trigger's dropdown, `+` (add an action) and `⋯` (*Duplicate*
  in `COPY_SIMPLE`, the glyph the app's own Duplicate wears, a rule, then *Remove*, a danger row
  in `TRASH`, as its *Remove interaction* step is named), then its actions top to bottom; a
  `ui::card_rule` parts one block from the next. The card's own `+` adds *On click* with no
  actions, which the block's head `+` then fills. With no interactions the card says *Drag a
  dot on the layer's edge onto a frame, or add an interaction with +*. The
  add and `⋯` menus are as wide as their longest row (`ui::menu_width`).
- **A picked block** (`ProtoState::block`). Only a click on one of its connectors picks a block
  (`open_link`), the connector being the interaction itself on the canvas; a press in the card
  picks nothing, and any other click on the canvas lets the pick go. While the layer has two or
  more interactions, the picked one wears `ui::paint_quiet_ring` `BLOCK_RING_GAP` (4) outside its
  rows; with one there is nothing to tell it from. The canvas lights that block's marks alone
  (*On the canvas*).
- **Copy and paste** are the layer context menu's, under *Copy/Paste as ▸* in prototype mode
  only (`context-menus.md`). *Copy interactions* keeps every interaction of the key or lone
  layer in `ProtoState::copied`, never saved and apart from the clipboard, with the tab it was
  copied in, and says *Copied N interactions*. *Paste interactions* is live in that tab only,
  since the copies' targets are that document's layers: it appends every copied interaction to
  each selected layer with fresh item ids, nested actions' too, as one *Paste interactions* step,
  or writes nothing and says why when the door refuses a layer's list. What was the source's own
  is re-homed (`rehomed`): a *Change to* of the source's instance switches the target's, which
  must be of the same set, and *On drop at* a source snap point names the target's point of that
  name; a target that has neither refuses the paste with the reason.
- **Triggers** are offered in two groups under `ui::menu_eyebrow`s, *Pointer* (*On click*, *On
  double click*, *On press*, *While pressing*, *On long press*, *On hover*, *While hovering*, *On
  leave*, *On drag*, *On swipe*, *On drop*) and *Other* (*Key…*, *After delay*, *When a variable
  changes*, *When a table changes*, *When scrolled past*, *When the frame opens*). *On drop* needs a layer that follows
  the drag (*Only a layer that follows the drag drops*); *When the frame opens* is offered on a
  top-level frame only (*Only a frame opens*); *When a variable changes* needs a variable; *When a
  table changes* a table (*This document has no tables*); *When scrolled
  past* needs a scrolling frame at or above the layer or on its screen (*No frame here scrolls*).
  Changing to a trigger no person fires is refused while the reaction holds *Run script*.
- **A trigger's fields sit under its head**, each change one *Change trigger* step:

  | Trigger | Fields | Head |
  | --- | --- | --- |
  | *After delay* | *Delay* (ms) | *After 2 s* |
  | *When a variable changes* | a variable dropdown | *When ‹variable› changes* |
  | *When a table changes* | the row actions' table dropdown | *When ‹table› changes* |
  | *On long press* | *Hold* (ms, new at 500, 100 to 10 000) | *On long press* |
  | *On swipe* | a *Left · Right · Up · Down* `ui::segmented` | *On swipe left* |
  | *On drop* | a dropdown behind `icon::CROSSHAIR_SIMPLE`, unlabelled as a trigger's other choices are (a number takes a prefix; a choice says what it holds by its glyph and tooltip): *Anywhere*, and under *Release* *Snaps* also the drawn point by its name (`drawn_point`) and each snap point; its face names the trigger's point as the head does, offered or not | *On drop anywhere*, *On drop at Closed*, *On drop at a missing snap point* |
  | *When scrolled past* | a frame dropdown (the scrolling frames at or above the layer, nearest first, then the rest of its screen's); *Past* (px, new at 100); which way, a *Down · Up · Either* `ui::segmented` (*Right · Left · Either* on a sideways frame) | *When Feed scrolls past 240*, *… going up*, *… either way* |
- ***Key…* captures the next key**: a focused field, *Press a key*, takes the next plain or
  `Shift`-ed key as the trigger (its `egui::Key` name). A `Ctrl` or `Alt` chord is refused, those
  staying the editor's while playing; `Escape` or a click away gives up.
- **The add menu** groups the actions by what they move: *Go* (*Navigate to*, *Back*, *Scroll
  to*, *Open link*), *Overlay* (*Open*, *Swap*, *Close overlay*), *State* (*Change to*, *Set
  variable*, *Set mode*, *Conditional*, *Run script*), *Table* (*Add row* in
  `ROWS_PLUS_BOTTOM`, *Edit row* in `ROWS_PENCIL` (U+F093, a table's rows with a pencil where
  `ROWS_PLUS_BOTTOM` has its plus; not `PENCIL_SIMPLE`, which renames), *Delete row* in
  `TRASH`). What the layer cannot take is dimmed with
  its reason (`prototype::action_refusal`): *Run script* under a trigger no person fires (*A
  script runs only when someone acts*) or with no script in the document or the script library;
  *Change to* outside an instance of a set's variant or a main variant (*Only a component with
  variants changes state*); *Set variable* with no variable; *Set mode* with no collection of two
  modes or more (*No collection has a second mode*); *Scroll to* with no scrolling frame to
  scroll (*No frame on the screen scrolls*), or whose scrolling frames hold no layer (*No frame
  that scrolls holds a layer*); a screen action with no frame to go to (*No frame to
  go to*); a *Table* action with no table (*This document has no tables*). A new action names
  the likeliest target (`prototype::new_action`):
  - a screen action: a frame on the holder's own page first, from the screen after its own round
    to the one before it, so the link draws as a connector rather than a stub; then other pages';
    mains last;
  - *Scroll to*: the first child of the nearest scrolling frame (`scrolling_frames`);
  - *Change to*: the layer's own instance (inside a main variant, that main) and a variant other
    than its current one;
  - *Set variable*: the first variable, a number incremented by 1, a boolean toggled, a text or
    colour one set to itself (`keeps`, `{name}`), which reads and changes nothing, so a fresh
    action never refuses the export; switching a toggle or an increment to such a variable, or
    picking *Set*, starts it the same way; a *Set*'s expression is kept when the new variable is
    of the same kind, and starts over the same way when it is not;
  - *Conditional*: one branch at the first boolean variable (`first_condition`), `true` with
    none, as *Add else if*'s new branch is;
  - *Run script*: the first script, or on a document with none a fresh script id, *Missing
    script* until one is copied in from the library;
  - *Set mode*: the first collection with two modes, at a mode the holder's screen does not read
    now, *Everywhere*;
  - a *Table* action: the table of a list the holder fires in (`in_list_over`: the holder sits in
    a row of a list over it, or in a main an instance of which does), else the first; *This row*
    there, else *Opened row*; *Add row* at the end with no cells.

  It opens its popover at once, except *Back*, which has nothing to set.
- **An action is one row**, a `ui::field_row_pressable` that opens its popover (*Back*'s opens
  none, having nothing to set, and says what it does under the pointer), laid out as an Effects
  row is (`ui::LIST_ROW_GAP`, the glyph at `CONTROL_GLYPH_PT`, the verb at `CONTROL_PT`): a grip,
  its glyph, its verb, its target in accent ink at `SMALL_PT`, its transition's kind alone (`transition_chip`; the
  timing and *Keeps scroll* are the popover's, for the room) as a faint caption note as a menu
  row's accelerator is, and `×`; the grip and `×` keep their own press. The verb and the target
  share what the kind and `×` leave, the verb first, each cut rather than run under the kind; a
  target with no room for a letter before its `…` is left out. *Set variable*'s row names its
  variable as a `binding::capsule`, as a bound field does, and not the value. A conditional's
  names its branch count (*1 branch*, *2 branches*, *… · else* while *else* holds actions), its
  conditions being the branch heads under it. A row action's names its table, and for *Edit
  row* and *Delete row* its row (*Cart · This row*, *· Opened row*, *· First where…*). The rows stand
  `NEST` in from their trigger. A row
  drags by its grip to
  another place in its own list (a reaction's, or one branch's), carried under the pointer with
  its branches from where it was held (`inspector::carried_unit`) while the others make room, as
  an Effects row is (`InFlight::action_drag`); the
  release is one *Move action* step, and a right-click or `Escape` puts it back. The
  grip is inert while the list holds one action. A target that is gone is drawn in the error
  colour behind the warning glyph (*Missing frame*, *Missing layer*, *Missing variable*, *Missing
  table*, …, and *No list row* for a *This row* action no longer inside a list over its table),
  kept so it can be mended, and skipped while playing. A conditional's branches follow it as heads in
  mono accent (*if …*, *else if …*, *else*), each one caption line tall (`BRANCH_HEAD_H`), taking
  `BRANCH_TUCK` of the rows' gap back above and below, its `+` reaching to the close button's box,
  their actions indented under them. A target is read as the layer holding the reaction reads it
  (`prototype::resolve_target`), so an instance's names its own layers.
- **The row's popover**, in the popover lane: the verb's head and close (an action moves by its
  grip alone), the action's fields, then its transition. Opened by a click on a connector, it
  is not closed by that click (`ProtoState::opened_on_canvas`): the canvas runs first, so the
  popover draws on the click's own frame. While it is open, `Delete` or `Backspace` removes its
  action as one *Remove action* step (`remove_open_action`, routed in `app::dispatch` ahead of
  `delete_selection`), not the layer that click selected. A screen action picks a top-level frame, each
  page's under its eyebrow, and *Navigate to* adds *Keep scroll position*, a `ui::switch_row`
  (*Back* has none: it keeps scroll as the navigation it undoes did); *Scroll to* a layer of the holder's screen; *Change to* a variant of the instance's
  set (`state_instance`; inside a main variant the instance is that main, which each of its
  instances reads as itself through `resolve_target`, so a looping state is authored on the
  main); *Set variable* a variable, then *Set* / *Increment* / *Toggle* (*Increment* for a number,
  *Toggle* for a boolean) and its expression or step; *Conditional* one expression per branch
  (`ui::labelled` *If* / *Else if*), *Add else if* (a `settings::text_button`, tipped *Add a
  branch tried when the ones above are not*) and a branch's `×`; *Open link* an address, refused unless `http` or `https`;
  *Run script* a document script by name, then (under a separator when there are any) *Copy from
  library*, a `ui::submenu_row` in `FOLDER_SIMPLE`, the script editor's *Save to library* glyph,
  dimmed with *The script library is empty*; its flyout lists the library's scripts by name, as
  wide as the longest (`ui::menu_width`).
  Picking one points the action at the document's copy (`MastikApp::copy_library_script`): a
  document script with that name and text, else the library's added under a free name and
  marked as the person's own, so its run shows no disclosure. The copy rides the action's edit
  (`ProtoState::script_in`), so both are one *Copy script into document* step. Kit scripts are
  not offered: they arrive with a kit's components.
- ***Set mode*'s popover**: *Collection* (`collection_combo`: the document's collections with two
  modes or more, then each kit's under the kit's `ui::menu_eyebrow`, `Variables::switchable`;
  a new pick starts at a mode the holder's screen does not read now), *Mode*, and *Where*
  (*Everywhere* · *This screen*), then a note: a by-width collection is held at the mode at any
  width; under *Everywhere*, frames that set their own mode keep it. Its row reads *Set Theme to
  Dark*, as *Set variable*'s reads *Set*, and *… · this screen* under *This screen*.
- **A row action's popover**: *Table* (`list_data::table_combo` with `TableRows::no_file`, which
  leaves out *From a file…*, since what it names must already be there; the same dropdown serves
  *When a table changes*). A new table drops the cells, and a row that cannot follow it (*First
  row where…*, or *This row* outside a list over it) becomes *Opened row*.
  - *Add row*: *At* (*End* · *Start*, a `ui::segmented`), then one `ui::labelled` expression field
    per column; an empty field writes an empty cell.
  - *Edit row* and *Delete row*: *Row* (*This row*, dimmed with *This row: the interaction is not
    inside a list over Cart* when it does not apply; *Opened row*; *First row where…*, which adds
    a *Where* test reading `{row.…}`, and the note *No row matches: the action is skipped*).
  - *Edit row* then lists only the columns it writes, each with `×` (*Leave this column as it
    is*), and *Add column*, whose new cell starts as `{row.<column>}`, which changes nothing until
    typed over; with two cells or more the note *Every cell is worked out from the row as it was
    before the edit*.
- ***Open overlay* and *Swap overlay* show how the frame shows** (`overlay_fields`, one
  *Overlay* step): *Position* (the centre, the top and bottom thirds' left, centre and right, or
  *Manual*), *Close on a click outside*, *Darken behind it* (black at 40% when turned on) and,
  while it is on, the shade as a shadow's colour row is (`slot_color_row` over
  `PaintSlot::Backdrop`: the chip opening the picker, the hex, the opacity as its strength), its
  own *Overlay* step; `SetReactions` draws no preview, so a picker drag is held in
  `InFlight::backdrop_paint`. *Manual* adds X and Y, from the top-left of the layer holding the
  action, and starts just under that layer (X 0, Y its height plus `MANUAL_OVERLAY_GAP`, 8), and the
  note *Drag the frame on the canvas to place it*: the fields are for the last point, the canvas
  is where it is placed (*Placing an overlay*, below).
- **Expressions show names.** The field writes a variable `{name}` and a table read
  `{Cart.rows}`, `{Products.3.Name}`, `{Cart.Line.sum}` or `{row.Qty}` (`Expr::with_names_in`)
  and stores ids (`Expr::from_names_in`), so a rename never breaks it (`play.md`,
  *Expressions*). What does not read, names nothing, or (for *Set variable*'s value and a
  condition) comes to another kind than wanted (`Expr::kind`; a cell has no kind until play, so
  it is never refused here) is kept as typed, ringed, with the reason on the
  warning glyph inside the field's right end (`ui::refusable_field`, as an address and a flow's
  starting value are refused) and the range `ExprError::at` names underlined
  (`ui::field_underline`), and nothing is written until it reads. Its hint ends *or @ for a
  name* (*or @ for a variable* where only variables are read, as in a list's filter): while the
  focused field ends in a `{` still open (outside quotes) and a name being typed, closed only by
  its `}`, or else in `@` and a name being typed (`at_rows`, `name_opener`: closed by a brace or
  an operator, and by whitespace unless a table's name may still go on past it, since *Table 1*
  holds one), a list opens under it of the variables whose names hold that text, then under *Tables* the
  tables with their row counts (`row`, the row acted on, first inside a row action), at most
  `AT_LIST_MAX` (12). After `<table>.` it lists *rows*, *opened*, *first*, *last*, a typed row
  number and its number columns, noted *# · sum min max avg* (a text column has no totals, and
  `{Table.Column}` alone reads nothing); after a row, every column; after a number column, its
  totals. The arrows move its light; Enter or Tab, or a click, replaces the
  opener and what follows with `{name}`, the caret after it, or, for a step that names nothing
  whole yet (a table, a row, a column before its total), with the same opener and that step and a
  dot, so the next step's list opens (`picked`).
  A press on the list keeps the field's focus and its draft.
- **Transitions** (`transition_fields`, also the page's *Default transition*): the kind; for *Move in*,
  *Move out*, *Push* and *Slide* (`TransitionKind::is_directional`) and *Smart animate*, a
  direction segment of *None* and the four sides. *None* is live for *Smart animate* alone, whose
  unpaired layers fade under it and slide in from a side otherwise (`play.md`, *Motion*).
  *Dissolve* goes no way and has no direction row. Picking *Smart animate* sets *None*; picking a
  move, push or slide while *None* is set sets the left side. Then *Curve* | *Spring*. A curve is
  an easing from the CSS set or *Custom curve* with its four cubic-bezier numbers (both X within
  0..1), and a *Duration*; a spring is a feel (`SPRING_PRESETS`: *Gentle* 1/120/20, *Quick*
  1/300/30, *Bouncy* 1/400/12, *Slow* 1/80/18; *Custom* shown when none matches) over its mass,
  stiffness and damping (*M*, *S*, *D*). Switching to *Spring* takes *Quick*, the drag settle's
  default too. Fields sharing a line stand
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

- **Each section of the card is headed by a `ui::fold_rule`** (`card_fold`, its fold kept in
  `collapsed_panels` as the Chart and List cards' are): *Scrolling*, *In a scrolling frame*,
  *With the scroll*, *Dragging*, *Input*. The fields inside a section are `ui::labelled`.
- **A frame**: *Scrolling*'s *Axis* (*None*, *Vertical*, *Horizontal*, *Both*), then *Pinned*,
  the children pinned in it, a `ui::menu_row` each with its pin as the row's note (*Top bar*,
  *Fixed*); a click selects the child, whose own card sets the pin. Once the frame scrolls,
  *Snap* (*None*, *Children*, *Paging*: not *Pages*, which names the document's pages), and under
  *Children* a second segmented row, *Start* · *Centre* · *End*; each change is a *Scrolling*
  step. The rest edges are drawn on the canvas while the frame is selected (*On
  the canvas*). How a frame shows as an overlay is its opening action's,
  set in that action's popover.
- **A child of a frame**: *In a scrolling frame*, *Scrolls* / *Fixed* / *Sticky*, dimmed with
  *The frame does not scroll* while its parent does not. The pin is set here, on the child's own
  card, rather than in the layers panel's menu.
- ***With the scroll***, on any layer below a page's top level (`scroll_effect_section`): `+`
  opens the menu of properties the later `+` does (`scroll_prop_menu`), and the pick adds an
  effect from 0 to 160 px holding it (*Add scroll effect*), following the
  nearest scrolling frame above the layer (`frame: None`) or, with none above, naming the
  screen's first; dimmed with *No frame on the screen scrolls* when no frame but the layer itself
  does. Then *Frame*, a dropdown of the scrolling frames at or above the layer, nearest first,
  then the rest of its screen's (`scrolling_frames`, the layer itself left out), its face led by
  `icon::FRAME` as every list of frames is (`layer_combo`'s glyph), showing the
  named frame or else the nearest; a pick names it, a *Scroll effect* step. Then *From* and *To*
  (px), and each property (*Opacity*, *Move Y*, *Move X*, *Scale*, *Rotation*) under its name as
  a `ui::labelled` eyebrow: its *From* and *To* fields, the values at the effect's two ends,
  opacity and scale in %, rotation in °, then `×` (*Remove this property*). A length scrubs at
  the Transform card's X and Y speed (`Scrub::whole(0.5)`), the rest at 1; all keep whole
  numbers. Each number is one *Scroll effect* step; `×` removes a property, and with the last
  the effect (*Remove scroll effect*); `+` (`scroll_prop_menu`, `ui::menu_row`s hung from its
  right edge as the add-action menu is) offers the properties not yet there (*Add scroll
  effect*), *Opacity* 100% to 0%, the others at rest at both ends (`new_effect_ends`).
- ***Dragging***, on any layer below a page's top level (`dragging_section`): *Follows the drag*,
  a `ui::switch_row`, then while it is on *Axis* (*Vertical* · *Horizontal* · *Both*, the words
  and order of *Scrolling*'s, less *None*) and *Range* (*Free* · *Within parent* · *Snap
  points*). Under *Snap points*, the drawn point's row: its name in a text field as the others'
  are (*Rename snap point*; *Start* typed back clears `drawn`), its `×` dimmed with *Where the
  layer is drawn is always a point*; then a row per point (`drag_point_row`): its name, `×`
  (*Remove this snap point*), and its X and Y from where the layer is drawn, scrubbed at the
  Transform card's speed and kept whole, live both ways: the fields follow a drag of its ghost
  (`proto_mark_value`), and the ghost a scrub or typing in them (`ProtoState::held_point`),
  before either commits; `+` adds *Point n* (`new_drag_point`), n the first number no point is
  named by, n × 100 along the layer's axis (across for *Horizontal*, down otherwise). A name is
  typed into a `ui::Draft` (`point_name_field`), so `Escape` gives it up. The points come
  before *Release* (*Stays* · *Returns* · *Snaps*), whose *Snaps* is dimmed with *Add a snap
  point first* while there is none, so the way to it is just above. Then *Settle*: the
  transition popover's timing rows and motion preview (`timing_fields`), without a kind or a
  side, for how the layer moves after a release, dimmed under *Free* and *Stays*, where nothing
  settles, with *Nothing settles: it goes anywhere and stays where it is let go*. Each change
  is a *Dragging* step; the snap points' own steps wear `icon::CROSSHAIR_SIMPLE`, as moving one
  on the canvas does (`play_step_glyph`).
- **An input**: a text layer binds a string variable; an instance a boolean one and the set's
  two-valued property that shows it. `×` unbinds. The note under it says what typing or
  ticking does while it is bound, and while it is not, what binding would do (*Bind a string
  variable to type into it while playing*).
- **The page** (nothing selected): *Device* (*None*, then the phone and tablet frame templates by name, each group under its
  `ui::menu_eyebrow`) with a `TURN_DEVICE` field button beside it, on while the device is
  turned (*Turn to landscape* / *Turn to portrait*, a *Turn device* step; dimmed with *Pick a
  device to turn*), then *Scaling* (`scaling_combo`, a dropdown whose rows carry their meaning
  in the accelerator's place: *Fit*, *Fill width*, *Actual size*, *Responsive*), and *Default transition*,
  the transition every new action takes, a dragged link's included. Under them a note says what is laid out other than as drawn
  (*iPhone 18 Pro: 402 × 874.*, or *iPhone 18 Pro, turned: 874 × 402.*) and, whenever the page has a device or is *Responsive*
  (either lays the screen out at a size of its own), names a flow whose start
  frame has no layout (*Checkout's start frame has no layout; it keeps its children where
  drawn.*); it is quiet when nothing differs.
- ***Flows***: one row per flow, its name (a `ui::Draft`, committed on leaving the field) and its
  start among the page's frames, a `ui::field_button` `×` to remove (as every removal in the
  tab is), each flow after the first under a `ui::card_rule`. The first row, the flow Play opens
  with no frame selected, wears `accent_400` on its name field's leading edge: the field's own
  rounded outline, `FIRST_FLOW_BAR` (3) thick, kept to its left corners so it wraps them, tipped
  *Play opens this flow when no frame is selected*. With none, the card says *A flow is where the exported prototype starts; add one with
  +*. `+` adds *Flow n* (`free_flow_name`: n the first number from the flow count plus one that
  no flow is named by) starting at the page's first frame, dimmed with no frame. Under each flow, its starting modes (`flow_modes`): a row per
  mode, its collection (as *Set mode*'s, less those another row names) and its mode, and `×`;
  *Add a starting mode* adds the first collection not yet named, at a mode the start frame does
  not read now, dimmed when none is left. Then its starting values: a row per value, the
  variable (a variable dropdown, the ones another row sets shown disabled; a new pick starts at
  its value in its collection's first mode), the value typed as
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
  `CURVE_STEP` on screen so it stays smooth zoomed in. A link going back, its target frame's
  centre left of its own frame's left edge (`goes_back`), instead leaves its layer's top edge and
  arches over both frames, cresting `BACK_RISE` (32) above the higher top, down onto the target's
  top edge (`arch`). A top-level frame's own *After delay* leaves from its name tag
  (`leaves_from_tag`), forward or back. It is `LINK_W` thick in `color::SELECT`
  for the selection's links (only the picked block's, while one of the layer's is picked) and
  `color::SELECT_DIM` for the rest, with a filled `HEAD_LEN` ×
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
  default transition (`prototype::with_link`): when the layer's first *On click* already holds a
  *Navigate to*, that one is pointed at the new frame (*Change action*) rather than a second
  added; else the navigation joins that click, or makes one (*Add interaction*). The action's
  popover then opens (`ProtoState::open`), without picking its block. A click on a dot links
  nothing and says how to use it; a drop elsewhere says where to drop.
- **Which unselected layer wears dots** is noted each frame in `ProtoState::knob_hover` by
  `note_link_hover` (from `draw_canvas`): the hover target, or the layer that wore them last
  frame while the pointer is still on one of its dots, since they sit half outside it. The hover
  outline keeps ringing that layer meanwhile (`link_hover`). A press aims at the dots drawn last
  frame.
- **A connector is a press target** (`connector_at`): the pointer within `LINK_PICK` (5 past
  its half-width) of a connector's line, stub or head takes it, the nearest when several are
  near. Under the pointer it is drawn last, in `color::SELECT` (a dangling one keeps the error
  colour) and `LINK_HOT_W` thick, and the hover outline rings the layer holding it
  (`link_hover`), the layer a click selects. A click selects that layer, picks the action's block
  and opens the action's popover in its *Interactions* card (`open_link`), so the link is edited
  where it lives; it does not pan the canvas. Link dots and a placed overlay's ghost come first;
  a press-drag on a connector starts what it would start elsewhere. The connectors are hit as
  drawn last frame (`ProtoState::links_drawn`).
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
- **The selected layer's marks** (`draw_proto_marks`), in prototype mode, under the placed
  overlay: a scrolling frame that snaps shows its rest edges as dashed `color::SELECT_DIM` lines
  across it (`snap_edges`: each child's leading edge, centre or trailing edge by *Align*, or each
  whole page's edge through its content). A layer that follows the drag shows each snap point
  as a dashed `color::SELECT` outline of its box at the point's offset with the point's name on
  a `color::BADGE` pill above (`snap_ghosts`); the drawn point is the layer itself. A scroll
  trigger (only the picked block's, while one is picked) shows its point as a dashed line across
  its frame at that offset from the frame's top (from its left, sideways), a knob painted as a
  link dot is (`KNOB_R`, `POINT` ringed in `SELECT`; taken, `KNOB_HOT_R` the other way round)
  at its leading end and the number on a pill at the other (`past_lines`); a horizontal frame's
  runs top to bottom. Each is
  heavier under the pointer or while dragged, and a press on one (a ghost's box, or within
  `PAST_REACH` of a line) drags it (`Drag::ProtoMark`), after the placed overlay and ahead of
  the box handles, the cursor the rest arrow (`moved_proto_mark`): a ghost keeps to its layer's
  axis (Shift to the drag's under *Both*) and snaps as a moved box does; a line moves along the
  scroll from 0 and snaps to its frame's children's leading edges, Alt holding that off. Both
  are whole points; the release writes one *Move snap point* or *Change trigger* step
  (`drop_proto_mark`), and a cancel writes nothing. The marks are hit as drawn last frame
  (`ProtoState::marks_drawn`); the card's fields still hold the numbers.
- **Only the canvas's own pointer lights them.** Connectors, link dots and marks read the
  pointer through `canvas_pointer`, which gives none where another layer is on top (the player
  over the canvas, a menu), as `pointed_at` does for the hover outline.
- **Flow chips** (`flow_chips`), in prototype mode: each flow's name behind the
  `PLAY` glyph on the canvas's own pill (`ui::canvas_pill_glyph`: `BADGE_PT`, `PILL_PAD`, 4pt
  corners) over its start frame's top-right corner, several on one frame stacked leftward. The
  page's first flow, the one Play opens with no frame selected, wears `color::BADGE` (the canvas badges' blue); the others
  `FLOW_CHIP_QUIET` (0.55) of it. Under the pointer it lifts by a white wash at alpha 26, as the
  crop's *Done* pill does. A click plays that flow over the canvas (`prototyping-player.md`); a
  right-click opens its menu (`context-menus.md` §6.12). Private mode masks the names.

## Components

- **Interactions are a keyed list on instances.** An instance's reaction rows wear the override
  slot (`component::item_slot_menu`): the dot when it differs from its main's, `INSTANCE_LOCAL`
  for the instance's own, ↺ under the pointer, and on a right-click *Reset to main* and *Push to
  main*, per item. A reaction the instance removed is a ghost row with *Restore* and *Push removal
  to main*. The card's header counts and resets them as every card does (`card_of`:
  `SetReactions` is *Interactions*, `SetPlay` *Prototype*).
- **Kit publishing** leaves out of a kit main every action whose target lies outside it, and
  every reaction whose scroll trigger watches a frame outside it (`kit::left_out`); a scroll
  effect naming a frame outside follows the nearest one above instead; the publish line names
  what is left out (*· left out Card's Navigate to Checkout, whose
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
