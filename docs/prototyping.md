# Mastik — prototyping

> **Status: decided, not built.** This file holds the design for prototypes until it lands; as each
> part is built, its rules move to a file under `design/` and leave here.

A prototype is the document played: frames linked by interactions, driven by variables and
scripts, animated between states, shown beside the editor while it is edited, and exported as one
HTML file a client opens with a double-click. Figma's model of linked frames is the spine, so it
reads as designers and clients expect; the logic reaches ProtoPie's and Axure's depth through
variables, conditions and Luau; the output is a real player, not a slideshow of images.

## 1. What is saved and what is not

**Saved in the document** — authored, through the commit door, undoable, copied with its layers
and carried by instances as linked copies:

- **Interactions on a layer**: a keyed list, each item a trigger and an ordered list of actions with
  its own id. An action names its target by id (a frame on any page, an overlay, a layer, a
  variable, a script) and carries its transition. The core type is not called `Interaction`, which
  already names a variant value's tied browser state (`variant::Interaction`).
- **Flows on a page**: named starting frames, each with optional starting values for variables
  (*Checkout, signed in*).
- **A frame's prototype settings**: scroll axis and its defaults as an overlay (position, backdrop,
  close on a click outside). **Fixed** and **sticky** are a child layer's own properties, read only
  inside a scrolling frame.
- **A page's prototype settings**: the device drawn around the screen, the scaling (fit, actual
  size, fill width) and the default transition a new link takes.
- **Inputs**: a text layer marked as an input binds a string variable; a checkbox or toggle is an
  instance whose boolean variant property binds a boolean variable. Typing or ticking during play
  writes the variable's play value (below).
- **Variables** are the existing ones (colour, number, string, boolean). A variable's starting value
  is its value in the mode in force where it is read, unless the flow sets it.
- **Scripts** that *Run script* names are document scripts, so the export can carry them. A library
  or kit script is copied into the document to be used.
- **The export's settings**: which flows go in, which opens, whether the device frame, the player
  bar and hotspot hints show. The scaling defaults to the page's.

**Play state** — exists only while a prototype runs; never saved, never in history, gone when the
player closes: the current frame, the history *Back* walks, the open overlays, each variable's
current value, each instance's current state, scroll offsets, what is typed into a field or ticked,
hover and press, any animation under way, and recorded takes (§8).

**Derived** — rebuilt when needed, never stored: smart animate's pairing of layers between two
frames, the in-between frames, the flow map's connectors, hotspot rectangles, and the copy's
`Resolved`.

### Targets that dangle

A deleted target leaves its action in place, drawn as missing and skipped in play; undo brings both
back. The loader keeps a dangling interaction target, unlike a dangling variable binding, which it
refuses: the action has nothing to draw, so keeping it costs nothing and mends on undo. Pasting or
duplicating layers remaps targets inside the pasted set to the new ids; a target outside it keeps
its id in the same document and is missing in another.

### Interactions on components

Interactions are a field of the layer like any other, so a main's interactions reach its instances
as linked copies and an instance overrides one by holding a different item under the same id, with
the override dot and *Reset* every other override has. Push to main and *Reset* work per item.

- An action targeting a layer inside the main is remapped to the instance's counterpart, as
  `remap_subtree` maps lineage keys.
- A kit main carries only interactions whose targets are inside it (state changes, the kit's own
  variables). Publishing a kit whose main navigates or opens an overlay reports the action and
  leaves it out, since its target lives in the kit's document.
- A component set's interaction states (*Hover*, *Pressed*, *Focus*, *Disabled*), which exist for
  code export, switch live in the player by themselves: hovering an instance shows its *Hover*
  variant with no interaction authored. An authored interaction may change to any other variant.

## 2. The player plays a copy

Starting a prototype clones the document. The player changes the clone through a play door in core:
`Document::apply` with `propagate::owed`'s settle, as the commit door does, so instances follow a
main changed during play. It keeps no undo history, no autosave and no dirty mark, and drops the
clone when it stops. Playing therefore never touches the file, which is invariant 5 applied to a
whole session instead of a gesture.

- **Variable values are not transactions.** A variable holds one value per mode and the modes in
  force differ per layer, so a play value is an override keyed by variable that beats every mode
  when the copy resolves. Kit variables take play values the same way.
- **A change of state** (an instance switching variant) is a transaction on the copy, drawn by the
  same code that draws the canvas.
- **Run script** runs Command Mode's Luau with a `Host` over the copy. A run that fails is rolled back
  to a snapshot taken before it, since the copy has no history to take the step back from. During
  play `sel` is empty, a script's input form is not offered (a script needing input cannot be run
  from an action), and `print` goes to the player's own log.
- **Only a person's act runs a script.** *Run script* is offered under pointer and key triggers
  (click, press, hover, drag, a key) and refused under *After delay*, *When a variable changes* and
  *When the frame opens*, where it would run unasked, which the document never does (D1100). The
  in-app player shows Command Mode's first-run disclosure before the first script runs, when play
  starts; the exported file runs its scripts in the same sandbox, against its own copy.
- `RenderOverrides` keeps its narrow job: the frames between two states while a transition, smart
  animate or spring runs, as patches over the copy's `Resolved`.
- Layers created during play take ids from the player's own actor, so they never collide with the
  document's.
- The exported player has no original to protect: it loads the file and plays on what it loaded.
  The in-app player and the exported one share one engine and behave the same.

## 3. Live update

While the editor's document changes, the in-app player applies each committed transaction to its
copy as well. Ids are caller-minted and operations deterministic (invariants 3 and 8), so the copy
stays the document plus what play did to it, and play state survives untouched. Only when the copy
refuses a transaction, because play changed something the edit also touches, does the player
re-clone and carry its play state across by id.

- What the edit removed falls back: a deleted current frame to the flow's start, a deleted
  variable's play value goes, a removed overlay closes. The player says so once, in its own corner.
- Only commits reach the player (it follows `revision`, not `drawn`): a drag, a pen path or text
  being typed shows once it lands, never as a preview. Undo and redo are commits like any other.
- A variable whose starting value was edited keeps its play value; *Restart* takes the new one.
- The player follows the document it was started from. Switching the editor to another tab leaves
  it playing that document; it stops when that document closes.
- Commits arriving faster than the player draws coalesce; a hidden player catches up when shown.

## 4. Where the player shows

- **Over the canvas**: a panel floating above the canvas, sized to the device and movable. It takes
  the pointer while hovered and the keys only while focused (a click into it focuses it, a click on
  the canvas gives focus back). The editor stays usable around it.
- **Detached**: the same player in a window of its own (an immediate egui viewport), for a second
  monitor or for sharing one window in a call while the design is edited in the other. It wears the
  app's own title bar (`chrome::Chrome::viewport`), remembers its position, and closes with the main
  window. Its keys are its own.
- **Fullscreen**: the player filling a screen. In the main window this is present mode's own branch
  of `MastikApp::ui`, which hides the editor; a detached player instead takes its own window
  fullscreen, so the main window keeps editing on the other monitor. Present mode plays the current
  page's first flow; with no flows anywhere in the document it shows top-level frames page by page
  as it does now. Present mode and *Play* are one act, so the `PLAY` glyph keeps one meaning.

The canvas already renders other documents into textures of their own for Compare (`compare.rs`);
the player draws its copy the same way, through its own `CanvasRenderer`, and egui shows that
texture in whichever viewport holds the player.

**Keys.** While the player has focus, plain and Shift-ed keys belong to the prototype (its text
fields and key triggers); chords with Ctrl or Alt go to the editor as ever, so `Ctrl+S` and
`Ctrl+Z` work mid-play. A key trigger may capture only plain and Shift-ed keys. Escape leaves a
prototype's text field, then hands focus back to the canvas; the player closes from its bar. Prototyping adds no chord to the keymap. The inspector's Prototype tab is reached by its segmented
control and the palette; *Play* by the top bar's play button, a flow chip and the palette;
`Ctrl+Alt+Enter` stays present mode, now the fullscreen player. The player's commands (restart,
hints, detach, record) are buttons: the chords they would want are spent in the editor, and a chord
that means two things breaks the keymap's rule.

**Private mode** masks the page and flow names in the player bar and the detached window's title,
since sharing that window is private mode's case.

## 5. Interactions

- **Triggers**: click or tap, hover (while and on enter), press (on and while), drag, a key, after a
  delay, a variable changing, and the frame opening.
- **Actions**: navigate, back, open, close or swap an overlay, scroll to, change an instance's state,
  set a variable, a conditional (if, else if, else, on variables and expressions), open a URL, and
  run a script. One trigger runs several actions in order.
- **Back** pops the navigation history; with an overlay open it closes the top overlay instead.
- **Scrolling**: a frame scrolls on its axis when its content overflows; fixed children stay, sticky
  ones stick at their offset. A drag trigger inside a scrolling frame takes the drag from the
  scroll. Flex and grid layout reflows inside, as on the canvas.
- **Hotspot hints**: a click on nothing flashes what can be clicked.

## 6. Motion

- **Transitions**: instant, dissolve, move in and out, push, slide, each with a direction.
- **Smart animate**: layers present in both frames are paired by name and path (an instance's
  children by lineage key) and every animatable property interpolates. An unpaired layer fades, or
  under a directional transition enters from its side. Layout is resolved at both ends, never per
  frame.
- **Timing**: easing curves (the CSS set and a custom cubic Bézier) and springs (mass, stiffness,
  damping), per transition.
- **States of a component** animate between each other the same way, and a state may loop (a
  spinner, a skeleton's shimmer).

## 7. Logic and data

- **Variables during play**: set, increment, toggle, and expressions over numbers, strings and
  booleans (arithmetic, comparison, joining, `not`/`and`/`or`); a bound field redraws as the value
  changes. Anything past that is a script.
- **Inputs** (§1) hold real values through their variables.
- **Lists from data**: a repeated row filled from a data file, reusing the chart's data model
  (`charts.md`). Its saved form is §10's third question.
- **Scripts**: *Run script* gives a prototype what the actions do not — a computed total, a filter, a
  validation — in the language Command Mode already has.

## 8. Export

- **One HTML file**: the player compiled to WebAssembly, the document, its fonts and its images
  inside one compressed stream, base64'd once into the page and unpacked by the browser's
  `DecompressionStream`; the wasm loads from those bytes, not by fetch, so it runs from `file://`
  without a server. Most parts are already compressed (wasm aside, WOFF2 and JPEG), so the file is
  about 1.33 times its parts, and the dialog shows that size before writing.
- **What goes in**: the pages the chosen flows reach, following navigation across pages, with the
  overlays, components, fonts and images they use. The pruned document must still pass the loader.
- **Rendering in the browser**: `mastik-core` and `mastik-render`'s vello backend on WebGPU, so the
  player draws exactly what the canvas draws, falling back to `vello_cpu` in the page where WebGPU
  is missing.
- **Recordings**: the player's record button logs a run's inputs with their times (a take, play
  state). Export replays a take on a fresh copy through the CPU backend, frame by frame, to MP4 or
  GIF; nothing is captured from the screen.
- **The flow map**: a PDF of each flow's frames with their connectors and every interaction written
  out in words, for review and sign-off.

## 9. The agent surface

Interactions, flows and the prototype settings are document fields, so MCP's `get_node` and `set`
read and write them and the snapshot carries them. There is no play tool: an agent checks a
prototype by reading what it links, not by running it.

## 10. Open questions

1. **Building the player for the browser.** `mastik-core` refuses `panic = "abort"`
   (`boolean.rs`), because `catch_unwind` guards round `flo_curves` and `i_overlay`, and stable
   `wasm32-unknown-unknown` aborts on panic. `mlua` builds Luau's C++ VM and does not target that
   triple either. Options: nightly `-Zbuild-std` with wasm exception handling;
   `wasm32-unknown-emscripten` for everything, which unwinds and hosts Luau, if wgpu's WebGPU works
   there; or geometry pre-flattened at export so the guarded code never runs in the page, with
   Luau built apart.
2. **Fonts in the export.** Embedding a Google font is allowed; a local font may forbid it (the
   parked font-embedding question in `roadmap.md`). The export needs a rule for reading a font's
   embedding permission and the fallback it names.
3. **Lists from data**: how a repeated row and its data file are saved, and how a row's layers bind
   a row's cells.
4. **The video encoder**: a crate and its licence for MP4; GIF needs none.
5. **A minimised main window**: whether an immediate viewport keeps the detached player drawing.
6. **Imported prototypes.** The Figma, Penpot and Sketch importers report prototype links as left
   out. Once interactions exist, the ones that map (navigate, overlay, back, the common
   transitions) can be read in, and the rest reported as now.
