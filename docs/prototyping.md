# Mastik — prototyping

> **Status: decided, not built.** This file holds the design for prototypes until it lands; as each
> part is built, its rules move to a file under `design/` and leave here.

A prototype is the document played: frames linked by interactions, driven by variables and
scripts, animated between states, shown beside the editor while it is edited, and exported as one
HTML file a client opens with a double-click. Figma's model of linked frames is the spine, so it
reads as designers and clients expect; the logic reaches ProtoPie's and Axure's depth through
variables, conditions and Luau; the output is a real player, not a slideshow of images.

## 1. What is saved and what is not

**Saved in the document** — authored, through `Document::apply`, undoable, copied with its layers
and inherited through components:

- **Interactions on a layer**: a trigger and an ordered list of actions. An action names its target
  by id (a frame, an overlay, a layer, a variable, a script) and carries its transition.
- **Flows on a page**: named starting frames, each with optional starting values for variables
  (*Checkout, signed in*).
- **A frame's prototype settings**: device frame, scroll axis, fixed and sticky children, and its
  defaults as an overlay (position, backdrop, close on a click outside).
- **Interactions on a component's main**: every instance has them; an instance may override them
  as it overrides any other field.
- **Variables** are the existing ones (colour, number, string, boolean). A variable's starting value
  is its value in the mode in force where the flow starts, unless the flow sets it.
- **Scripts** that *Run script* names are document scripts, so the export can carry them. A library
  or kit script is copied into the document to be used.
- **The export's settings**: which flow opens, scaling, whether the player shows its own chrome.

**Play state** — exists only while a prototype runs; never saved, never in history, gone when the
player closes: the current frame, the history *Back* walks, the open overlays, each variable's
current value, each instance's current state, scroll offsets, what is typed into a field or ticked,
hover and press, and any animation under way.

**Derived** — rebuilt when needed, never stored: smart animate's pairing of layers between two
frames, the flow map's connectors, and the copy's `Resolved`.

## 2. The player plays a copy

Starting a prototype clones the document. The player changes the clone with ordinary transactions
through `Document::apply` — setting a variable, switching an instance's state, a script's writes —
with no history, no autosave and no dirty mark, and drops it when it stops. Playing therefore never
touches the file, which is invariant 5 applied to a whole session instead of a gesture.

- Variable bindings, variant swaps, visibility and layout already resolve through `Resolved`, so a
  change of state is drawn by the same code that draws the canvas.
- *Run script* runs the same Luau as Command Mode; its one step lands on the copy instead of the
  document.
- `RenderOverrides` keeps its narrow job: the frames between two states while a transition, smart
  animate or spring runs, as patches over the copy's `Resolved`.
- Layers a script creates during play take ids from the player's own actor, so they never collide
  with the document's.
- The exported player has no original to protect: it loads the file and plays on what it loaded.
  The in-app player and the exported one share one engine and behave the same.

## 3. Live update

While the editor's document changes, the in-app player re-clones it on each commit and carries its
play state across by id: the current frame, overlays, variable values, instance states, scroll and
typed values survive. What the edit removed falls back — a deleted current frame to the flow's
start, a deleted variable's value to nothing, a removed overlay closes.

- Only commits reach the player: a drag, a pen path or text being typed shows once it lands, never
  as a preview.
- Undo and redo are commits like any other and update the player the same way.
- The player follows the document it was started from. Switching the editor to another tab leaves
  it playing that document; it stops when that document closes.
- Re-cloning and re-resolving per commit doubles the resolve work while the player is open. It is
  skipped while the player is hidden, and commits arriving faster than the player draws coalesce
  to the last.

## 4. Where the player shows

- **Over the canvas**: a panel floating above the canvas, sized to the device frame and movable. It
  takes the pointer and keys while it is hovered or focused; Escape hands focus back to the canvas.
  The editor stays usable around it.
- **Detached**: the same player in a window of its own (an egui viewport), for a second monitor or
  for sharing one window in a call while the design is edited in the other. It wears the app's own
  title bar (`chrome::Chrome::viewport`), remembers its position, and closes with the main window.
  Its keys are its own: a key pressed in it never reaches the editor's bindings.
- **Fullscreen**: present mode becomes the player shown fullscreen, on whichever window holds it, so
  a detached player can fill the second monitor while the main window keeps editing. A page with no
  flows plays its top-level frames in reading order, as present mode does now.

The canvas already renders a second document into its own texture for Compare (`compare.rs`); the
player draws its copy the same way, through its own `CanvasRenderer`, and egui shows that texture
in whichever viewport holds the player.

## 5. Interactions

- **Triggers**: click or tap, hover (while and on enter), press, drag, a key or gamepad button,
  after a delay, a variable changing, and the frame opening.
- **Actions**: navigate, back, open, close or swap an overlay, scroll to, change an instance's state,
  set a variable, a conditional (if, else if, else, on variables and expressions), open a URL, and
  run a script. One trigger runs several actions in order.
- **Interactive components**: a main's states (hover and press, which exist for code export, and
  any variant) switch live in the player, so a button behaves wherever it is used without a frame
  for each state.
- **Scrolling**: a frame scrolls on its axis when its content overflows; fixed children stay, sticky
  ones stick at their offset. Flex and grid layout reflows inside, as on the canvas.
- **Hotspot hints**: a click on nothing flashes what can be clicked.

## 6. Motion

- **Transitions**: instant, dissolve, move in and out, push, slide, each with a direction.
- **Smart animate**: layers present in both frames are paired by name and path (a main's lineage
  key for instance children) and every animatable property interpolates; an unpaired layer fades.
- **Timing**: easing curves (the CSS set and a custom cubic Bézier) and springs (mass, stiffness,
  damping), per transition.
- **States of a component** animate between each other the same way, and a state may loop (a
  spinner, a skeleton's shimmer).

## 7. Logic and data

- **Variables during play**: set, increment, toggle, and expressions over numbers, strings and
  booleans; a bound field redraws as the value changes.
- **Inputs**: a text field, a checkbox and a toggle hold real values, bound to variables.
- **Lists from data**: a repeated row filled from a data file, reusing the chart's data model
  (`charts.md`): a table read into rows, each row's cells bound to the row's layers.
- **Scripts**: *Run script* gives a prototype what the actions do not — a computed total, a filter, a
  validation — in the language Command Mode already has.

## 8. Export

- **One HTML file**: the player compiled to WebAssembly, the document, its fonts and its images
  embedded, so a client double-clicks one file with no folder beside it. The wasm loads from bytes
  inside the page, not by fetch, so it runs from `file://` without a server.
- **Rendering in the browser**: `mastik-core` and `mastik-render`'s vello backend on WebGPU, so the
  player draws exactly what the canvas draws.
- **Recordings**: a flow played to MP4 or GIF through the CPU backend, for a client who wants a video.
- **The flow map**: a PDF of the frames with their connectors, for print and review.

## 9. Not built

Recorded in `non-goals.md`: a keyframe timeline, a node graph of patches, several devices and
sensors talking to each other, Rive and Lottie export, comments in the player, and hosted links.

## 10. Open questions

1. **Luau in the browser.** `mlua` builds Luau's C++ VM with the platform's compiler and does not
   target `wasm32-unknown-unknown`, which wgpu and vello use on the web. Either Luau is built
   separately (Emscripten) and the two modules talk through the page, or *Run script* is limited in
   the exported player. Decide before the export is built.
2. **A browser without WebGPU.** Fall back to `vello_cpu` in the page, or refuse with a message
   naming the browsers that work. WebGPU from `file://` has to be checked in each browser the
   export claims.
3. **Fonts in the export.** Embedding a Google font is allowed; a local font may forbid it (the
   parked font-embedding question in `roadmap.md`). The export needs a rule and a message for a font
   it cannot carry.
4. **The editing surface**: where interactions are authored (a Prototype tab in the inspector, a
   mode, or connectors drawn on the canvas) and how the flow map shows on the canvas.
5. **Imported prototypes.** The Figma, Penpot and Sketch importers report prototype links as left
   out. Once interactions exist, the ones that map (navigate, overlay, back, the common
   transitions) can be read in, and the rest reported as now.
