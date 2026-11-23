# Prototyping: the player in the app

The play engine's shell inside the editor (`crates/mastik-app/src/player.rs`). The engine and
its contract are `play.md`; the exported page's shell is `prototyping-export.md`. Every place
the player shows draws the same body: the screen, the hotspot hints and the notices, over a bar
of its commands.

## Starting and stopping

- **Play** is the top bar's play button (lit while a prototype plays; a click on it stops), the
  palette's *Play prototype*, and a flow's chip on the canvas. It starts on the selected
  top-level frame, else the current page's first flow, else the page's first frame in reading
  order, else the first flow on another page (`player::start_for`): what the person is looking
  at comes before a flow elsewhere.
- **Present mode is the player filling the window** when the document has a flow anywhere
  (`Ctrl+Alt+Enter`); with none, present mode steps through frames (`app-canvas-chrome.md`, *Present mode*). Present mode and
  *Play* are one act, so the `PLAY` glyph keeps one meaning.
- **Scripts first.** When any *Run script* names a document script this machine has not shown
  for its current source, a card lists each script's source before play starts, with *Play* and
  *Cancel*; *Play* marks them seen, as Command Mode's first-run disclosure does.
- A player's copy is the document it was started from. It stops when that document's tab
  closes.

## Following the editor

The session's `History` keeps a journal of every transaction it applies — commits, undos,
redos, reverts — while a player plays (`History::keep_journal`). Each pass the player takes the
journal and hands each transaction to `Player::apply_edit`, so only commits reach it, never a
gesture's preview, and commits arriving faster than it draws land together. A parked tab's
journal waits, and its player catches up when the tab is in front again. A document read in
again starts play over from the same start.

## Where it shows

- **Over the canvas**: a card the size of the screen at the page's *Fit*, at most 70% of the
  canvas, never narrower than `OVER_MIN_W` (380, the detached window's least width too) so the bar
  fits with the flow menu's head at its longest (`FLOW_HEAD_CHARS`, 14), moved by its head. The pointer drives the prototype while over the screen; a press in
  it gives the player the keyboard, a press on the canvas gives it back.
- **Detached**: the same body in an immediate viewport wearing the app's title bar
  (`chrome::Chrome::viewport`), where it last stood (`Prefs::player_window`). That position is
  read once as the window opens (`Playing::window_from`, cleared when the player's place
  changes) and is the one its viewport builder keeps giving, never the position read back each
  frame, which would fight a drag. Its keys are its
  own; its *Fullscreen* fills its own screen.
- **Fullscreen**: present mode's branch of `MastikApp::frame_body`, the editor hidden.

## Keys

While the player holds the keyboard, plain and Shift-ed keys and typed text go to the prototype
(`Input::Key` by egui's key name, the name a key trigger stores; `Input::Text`); chords with
Ctrl or Alt go to the editor's keymap, so `Ctrl+S` and `Ctrl+Z` work mid-play. An Escape the
prototype does not use gives the keyboard back; fullscreen, it leaves (present mode stops,
otherwise the player returns over the canvas).

## The body

- **Scaling**: a page's device always sets the screen's size, turned when turned, and the
  scaling only fits that screen to the room (`PagePlay::play_size`, `Player::set_play_size`).
  With no device, *Responsive* lays the screen out again at the room's size (one unit a point,
  so a resized window or fullscreen shows the real layout), and *Fit*, *Fill width* and *Actual
  size* play it as drawn, never wider than the room; *Responsive* places as *Fit*. Over the
  canvas the card is sized from the screen, so there a screen with no device plays as drawn.
- **Turning**: a page's device plays on its side when the page is turned or the bar's
  `TURN_DEVICE` button turned it for this run (`Playing::turned`, play state, never the
  document's); the screen is laid out at the turned size, never a rotated picture. The button
  sits before *Open in a window*, lit while turned, tipped *Turn to landscape* / *Turn to
  portrait*, dimmed with *Pick a device to turn* on a page with none.
- **The size readout**: when the play size changes after play's first (a window resized under
  *Responsive*, a turn), the bar shows it in units, `874 × 402`, faint at
  `CAPTION_PT` before the turn button, for a second after the last change, then fading
  (`Playing::readout`). Not over the canvas, whose card never resizes and whose bar has no room.
- **Play on a phone** (`phone.rs`): the bar's `PHONE` button, after Export, tipped *Play on a
  phone on this network*, starts sharing and opens its card in the side column, in the state
  card's place (the two never show together); while sharing it is lit and toggles the card, and
  the phones listening are counted after it in `ui::head_badge`. An Escape the prototype does
  not use closes the card and keeps sharing; the next gives the keyboard back.
  Sharing starts only from the button, is never remembered, and stops when the player closes
  (*Stopped sharing — the player closed*) or from the card's *Stop sharing* (Danger).
  - **The server** binds this computer's private-network address (the one its default route
    leaves from; none on a public one, *Not on a network*) on a free port, and answers only
    clients in the private, link-local or loopback ranges, under a fresh 128-bit token per
    share: the page, `doc` (the current version), `events` (a stream of versions, a heartbeat
    every 10 s) and `status` (a phone's screen). Hand-written on `std::net`: four fixed paths,
    a bounded head and body, nothing else served.
  - **The page** is the HTML export's (`Parts::mirror`, `prune_mirror`): the flow playing, or
    its frame, with no bar or device frame, and no device unless the export's *On a phone* keeps
    the page's, so *Responsive* lays out at the phone's size and follows its turn. An edit is sent once the document has sat still for 0.25 s as a new
    version the phone plays where it is (`Player::rebase`); one needing fonts the page lacks
    sends `reload`. The phone toasts *Updated from Mastik*, and when sharing stops dims its last
    frame under *Mastik stopped sharing*.
  - **The firewall.** The first share on Windows raises the system's prompt for the app, which
    the card's help line names on Windows (*allow it so the phone can reach this computer*),
    every share, so the card keeps one size. A declined prompt cannot be seen from the app: a
    phone that never arrives is the sign, so after 20 s with none the empty list says to allow
    Mastik on private networks in Windows Security (elsewhere, to check both are on one network).
  - **The card**: the QR code (`qrcode`, painted dark on light in either theme), one line of help,
    the link with *Copy* (*Copied the link*), *Following* — up to four rows of a dot (lit while
    the phone listens, grey while it sleeps, gone after a minute), its browser at its coarsest
    (*Android · Chrome*) and its frame, folding into `ui::more_row`, *No phone yet* when empty,
    the list one height whatever it holds — and *Stop sharing*. A share that cannot start shows
    its reason as an empty note with *Try again*.
- **The mode chip**: while the play width picks another mode of the collection picked by width
  than the drawn width does (`Player::width_pick`), a `ui::scope_chip` with `icon::DATABASE`
  names it, *‹Collection› · ‹Mode›*, after the title in the card's or window's head, and
  fullscreen, which has no head, in the bar after the readout: why the layout jumped is on screen.
  A pinned mode beats the width, so then there is none.
- **Hotspots**: each hotspot drawn in the canvas selection blue (`color::select_a`), a 2pt
  stroke over a fill at a fifth of its alpha, as the exported page draws them, while the bar's toggle is on, and flashing
  by `Scene::hint_flash` after a click on nothing. The player, the export card and the exported
  page all call the toggle *Hotspots*.
- **Notices** in the screen's top-left corner, each once, for four seconds, in the app's words.
- **The bar**: the flow menu, *Restart*, *Hotspots*, *State* (the state card), *Record a
  take*, Export (`EXPORT`: *Export as an HTML file or a flow map* with no take, opening the
  export card on *HTML file*; *Export a take as a video* once one exists, opening it on
  *Video*), *Play on a phone*, and from the right *Fullscreen* (`ARROWS_OUT`),
  *Open in a window* / *Back over the canvas* (`BROWSERS`, lit while detached) and, fullscreen,
  *Stop playing*. `PLAY` is playing and `ARROW_SQUARE_OUT` leaving the app, so neither is worn
  here. Its commands are buttons, not chords: the editor has spent the chords they would want.
- **Recording**: while a take records, the whole row cross-fades out over `RECORD_FADE`
  (0.18 s) and is disabled, so a faded button takes no click, and a recording row fades in at
  the bar's left: *Stop recording* (`STOP`, lit) and the time so far, `0:24` at `CAPTION_PT` in
  the danger ink. Stopping fades back.
- **The flow menu** leads the bar: a `ui::menu_head` naming what plays (the flow, else the page;
  cut at `FLOW_HEAD_CHARS`), tipped *Play another flow*. It lists the flows of the page playing
  under *Flows on ‹page›*, each with its start frame as the note and the one playing ticked, then
  *From the selected frame* on the top-level frame selected on the canvas's page, disabled with *Select a
  frame on the canvas* when there is none. A pick restarts the copy there (`replay_from`): a new
  copy, play state dropped as *Restart* drops it, a recording under way ended unkept.
- **Filling a screen** (fullscreen, or the detached window at its *Fullscreen*), the bar floats
  over the screen's foot and fades over `BAR_FADE` (0.3 s) once the pointer has been still for
  `BAR_HOLD` (2 s) (`bar_shown`). Moving or pressing wakes it; it stays while the pointer is
  within `BAR_WAKE_EDGE` (80) of the bottom, or a menu or the side column is open.
- **A live edit landing** pulses the screen's edge once: a 2pt accent ring fading over
  `PULSE_LEN` (0.6 s).
- **The state card** fills the side column (`SIDE_W`, 260): its rows sit in a vertical scroll
  area with the scroll bar hidden, inset `CANVAS_CARD_PAD` at the sides and half that top and
  bottom, `ui::CARD_ROW_GAP` apart, each section's eyebrow a further `CARD_ROW_GAP` off the rows
  above it. It opens with the frame shown (`screen_row`) and how far *Back* reaches, each
  open overlay then each step of history (*2 back*, or *No way back*). Then *Opened rows* when a
  list shows *Opened row*: one row per data
  set such a list reads, a dropdown of its rows (`lists.md`, *Play*). Then *Tables* when a row
  action has changed a table this run (`Player::tables`): its name, *2 rows · 0 in the file* in
  faint caption ink, *Show* (`icon::TABLE`, lit while shown) and *Reset* (`icon::ARROW_COUNTER_CLOCKWISE`,
  *Put Cart back as the document has it*, an `Input::ResetTable`). *Show* opens the data window
  read-only over the played table on the player's screen: the grid locked as a linked table's
  is, no toolbar, and a note line behind `icon::PLAY`, *Edited in play — edits here would not
  reach the document*. Then *Modes* when the
  document has a collection of two modes or more:
  one row per such collection, the document's then each kit's under the kit's eyebrow
  (`Player::modes`), a dropdown of its modes
  set on every page, dim with *This screen sets its own ‹Collection› mode* while the screen sets
  one. The variables follow under a *Variables* eyebrow with their values; a number or text is
  typed and set on leaving the field (`ui::Draft`, as every field commits; `Escape` drops it), a
  boolean is a `ui::switch_row` under its name, a colour shown. Each row but a switch is a
  `ui::label_row`: the name on the left, strong once play has set the value, cut short of the
  control, which takes `STATE_CONTROL_SHARE` of the row. A value or mode set there is an input
  like any other.
- **Private mode** masks the flow's and page's names in the card's head and the window's title.
- Its texture is the player's own `CanvasRenderer`, drawn whole through
  `mastik_render::play` whenever the scene or the copy changed.

## Exports from the player

One card, *Export prototype* (`prototype_export::ExportCard`), a modal `settings::canvas_card`
with a `ui::segmented` row at its head switching its `Output`: *HTML file*, *Video*, *Flow map*.
The body keeps the tallest height it has shown (`body_h`), so switching to a shorter output
never shrinks the card. Its doors (`open_export`): the palette's *Export prototype…* opens it on
*HTML file* and *Export flow map…* on *Flow map*; the bar's Export on *Video* with the newest
take picked once a take exists, else on *HTML file*; a flow chip's menu on *HTML file* opening on that flow, or on
*Flow map* holding it alone (`act_on_flow`). The HTML file and the flow map need a flow, so
without one the card does not open and the status line says to add one. Each output asks where
to write, offering a name led by the document's: `‹document› — ‹flow›.html` (the flow it opens
on), `‹document› — ‹take›.mp4` or `.gif`, `‹document› — Flow map.pdf`.

- **The flows** of the HTML file and of the flow map are a ticked list (`flow_ticks`), one
  `ui::tick_row` per flow wearing `PLAY` as the flow chips do, since a flow is something left out
  rather than a feature switched; the one left ticked alone is `Tick::Fixed`, tipped with why it
  cannot go.
- **HTML file**: the flows in the file (every flow is the empty list; the last one cannot be
  left out), the flow it opens on (a `ui::fitted_combo`, *None* when no flow is in), the device
  frame, player bar and *Hotspots*, the scaling (the page
  card's dropdown with *Page's own · ‹its scaling›* first, the default), and *On a phone*
  (`PlayExport::on_phone`: *Follow its turn*, the default, or *Keep the page's*, captioned as the
  scaling's rows are; always shown, so the card keeps its size, and disabled with *Only a
  Responsive page turns with the phone* unless the scaling resolves to *Responsive*). Changed
  settings are committed to the document when the file is written, as one step.
  - **The size meter**, measured on a worker before anything is written, once per change of
    settings, document or downscaled pictures: *On disk, about ‹size›*, a `ui::level_bar_parts`
    split into Player, Fonts, Images and Document (`Parts::breakdown`, `prototyping-export.md`
    *The file*), and under it a `ui::level_bar_legend` of each part's size.
  - Its warnings and notes are `ui::note_line`s, the design Export card's warning line.
  - **A heavy picture**: one the file carries that is drawn below `pictures::HEAVY_BELOW` (a
    quarter) of its pixels across is named by the first layer showing it, with *Downscale to 2×*:
    twice its drawn size, made in the export's copy only (`prototype_export::downscale`, a JPEG at
    85, or a PNG when it has see-through pixels). A note then says the document keeps the
    original, with *Keep the original* to take the downscale back.
  - **What holds the export** (`mastik_export::prototype::holds`): on the pages the file carries,
    a layer with an action naming a layer or table that is gone, or an expression that does not read
    (`Expr::check`), each named by its layer behind a warning glyph in the danger ink. While any is listed, *Export…* is
    disabled with *Fix the interactions named above first*; until the measure lands, with
    *Measuring the file and checking its interactions*, since the measure is what finds them.
- **Video**: the takes recorded this run (`Playing::takes`, each a `TakeRec` with what played and
  when), in a *Take* dropdown newest first, each noted with its length and clicks; side by side,
  `ui::CARD_COL_GAP` apart, *Format* (MP4,
  *H.264, small*, or GIF, *loops, plays anywhere*) and *Size* at 1×, 2× (the default) or 3× the
  screen, each with its pixel size, the device frame's bezel counted; *Show clicks* (on by
  default) and *Device frame*; and a still preview at the first click (`video::still`), or the
  start when there is none. The foot says the length at 30 frames a second. Written through
  `mastik_export::video` on an export worker (`play.md`, *Takes*). Without a take the body says
  to record one, and *Export…* is disabled.
- **Flow map**: which flows go in (empty is every flow; the last cannot be left out), then a
  `settings::caption_unled` saying what the map holds, written by
  `mastik_export::flow_map::flow_map_of` (`export_flow_map_to`).
