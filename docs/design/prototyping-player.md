# Prototyping: the player in the app

The play engine's shell inside the editor (`crates/mastik-app/src/player.rs`). The engine and
its contract are `play.md`; the exported page's shell is `prototyping-export.md`. Every place
the player shows draws the same body: the screen, the hotspot hints and the notices, over a bar
of its commands.

## Starting and stopping

- **Play** is the top bar's play button (lit while a prototype plays; a click on it stops), the
  palette's *Play prototype*, and a flow's chip on the canvas. It starts on the current page's
  first flow, else the first flow on any page, else the selected top-level frame, else the
  page's first frame in reading order (`player::start_for`).
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
  canvas, moved by its head. The pointer drives the prototype while over the screen; a press in
  it gives the player the keyboard, a press on the canvas gives it back.
- **Detached**: the same body in an immediate viewport wearing the app's title bar
  (`chrome::Chrome::viewport`), where it last stood (`Prefs::player_window`). Its keys are its
  own; its *Fullscreen* fills its own screen.
- **Fullscreen**: present mode's branch of `MastikApp::frame_body`, the editor hidden.

## Keys

While the player holds the keyboard, plain and Shift-ed keys and typed text go to the prototype
(`Input::Key` by egui's key name, the name a key trigger stores; `Input::Text`); chords with
Ctrl or Alt go to the editor's keymap, so `Ctrl+S` and `Ctrl+Z` work mid-play. An Escape the
prototype does not use gives the keyboard back; fullscreen, it leaves (present mode stops,
otherwise the player returns over the canvas).

## The body

- **Scaling** by the page's *Fit*, *Fill width* or *Actual size*, never wider than the room.
  *Responsive* lays the screen out again at the room's size (one unit a point, so a resized
  window or fullscreen shows the real layout) or, with a device, at the device's size fitted as
  *Fit* (`PagePlay::play_size`, `Player::set_play_size`). Over the canvas the card is sized
  from the screen, so there a screen with no device plays as drawn.
- **Turning**: a page's device plays on its side when the page is turned or the bar's
  `ROTATE_RIGHT` button turned it for this run (`Playing::turned`, play state, never the
  document's); the screen is laid out at the turned size, never a rotated picture. The button
  sits before *Open in a window*, lit while turned, tipped *Turn to landscape* / *Turn to
  portrait*, dimmed with *Pick a device to turn* on a page with none.
- **The size readout**: when the play size changes after play's first (a window resized under
  *Responsive*, a turn while one is laid out), the bar shows it in units, `852 × 393`, faint at
  `CAPTION_PT` before the turn button, for a second after the last change, then fading
  (`Playing::readout`). Not over the canvas, whose card never resizes and whose bar has no room.
- **Play on a phone** (`phone.rs`): the bar's `PHONE` button, after *Record*, tipped *Play on a
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
- **Hints**: each hotspot outlined in the accent while the bar's toggle is on, and flashing by
  `Scene::hint_flash` after a click on nothing.
- **Notices** in the screen's top-left corner, each once, for four seconds, in the app's words.
- **The bar**: *Restart*, *Show hotspots*, *Variables* (the state card), *Record a take* /
  *Stop recording*, *Export the take as a video* once a take exists, *Open in a window* / *Back
  over the canvas*, *Fullscreen*, and, fullscreen, *Stop playing*. Its commands are buttons, not
  chords: the editor has spent the chords they would want.
- **The state card** heads with *Modes* when the document has a collection of two modes or more:
  one row per such collection, the document's then each kit's under the kit's eyebrow
  (`Player::modes`), a dropdown of its modes
  set on every page, dim with *This screen sets its own ‹Collection› mode* while the screen sets
  one. The variables follow under a *Values* eyebrow with their values; a number or text is
  typed, a boolean switched, a colour shown. A value or mode set there is an input like any other.
- **Private mode** masks the flow's and page's names in the card's head and the window's title.
- Its texture is the player's own `CanvasRenderer`, drawn whole through
  `mastik_render::play` whenever the scene or the copy changed.

## Exports from the player

- **The export card** (*Export prototype…*): the flows in the file (every flow is the empty
  list; the last one cannot be left out), the flow it opens on, the device frame, player bar and
  hints, the scaling (the page card's dropdown with *Page's own · ‹its scaling›* first, the
  default), *On a phone* (`PlayExport::on_phone`: *Follow its turn*, the default, or *Keep the
  page's*, captioned as the scaling's rows are; always shown, so the card keeps its size, and
  disabled with *Only a Responsive page turns with the phone* unless the scaling resolves to
  *Responsive*), and the file's size measured on a worker before
  anything is written (`Parts::estimate`). Changed settings are committed to the document when
  the file is written, as one step.
- **Video**: a recorded take is written as MP4 or GIF by the name given, at 30 frames a second
  and two pixels a unit, through `mastik_export::video` on an export worker.
- **The flow map** (*Export flow map…*): `mastik_export::flow_map`, a PDF.
