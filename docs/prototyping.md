# Mastik — prototyping

> **Status: decided, partly built.** This file holds the parts of prototyping not yet built; as
> each part is built, its rules move to a file under `design/` and leave a pointer here. Built: the
> saved model (`design/prototyping-model.md`), the play engine (`design/play.md`), authoring
> (`design/prototyping-authoring.md`) and the HTML export's browser build, trim and file
> (`design/prototyping-export.md`). Still to build: the player in the app, the engine in the
> exported page with the export dialog, video from takes and the flow map.

A prototype is the document played: frames linked by interactions, driven by variables and
scripts, animated between states, shown beside the editor while it is edited, and exported as one
HTML file a client opens with a double-click. Figma's model of linked frames is the spine, so it
reads as designers and clients expect; the logic reaches ProtoPie's and Axure's depth through
variables, conditions and Luau; the output is a real player, not a slideshow of images.

## 1. What is saved and what is not

**Saved in the document**: built, in `design/prototyping-model.md`. Variables' play values,
starting values and inputs during play are built in `design/play.md`. Still to build around them:

- **Scripts** that *Run script* names are document scripts, so the export can carry them. A library
  or kit script is copied into the document to be used.
- **The export's scaling** defaults to the page's when the export names none.

**Play state** is built: `design/play.md` *Play state* and *Takes*.

**Derived** — rebuilt when needed, never stored: smart animate's pairing of layers between two
frames, the in-between frames, the flow map's connectors, hotspot rectangles, and the copy's
`Resolved`.

### Targets that dangle

Built: the model keeps them and copies remap them (`design/prototyping-model.md`), the Prototype
tab draws them as missing (`design/prototyping-authoring.md`), and play skips them
(`design/play.md`).

### Interactions on components

Built: the model (`design/prototyping-model.md` *Instances*), the override slot and kit publishing
(`design/prototyping-authoring.md` *Components*), and *Hover*, *Pressed* and *Disabled* switching
live in play (`design/play.md`). Still to build: *Focus* switching live as the others do, and Tab
moving between a prototype's inputs.

## 2. The player plays a copy

Built: the copy, its play door, play values, a change of state, *Run script* with its rollback,
the patches and the player's own actor are in `design/play.md`. Still to build:

- The in-app player shows Command Mode's first-run disclosure before the first script runs, when
  play starts (`Player::has_scripts` answers whether it is owed).
- The exported file runs its scripts in the same sandbox, against its own copy (§10, question 5).

## 3. Live update

Built in the engine: applying each commit to the copy, re-cloning on a refusal, the fallbacks and
their notices, and an edited starting value keeping its play value (`design/play.md` *Live
update*). Still to build, in the shell:

- The fallback notices show once, in the player's own corner.
- Only commits reach the player (it follows `revision`, not `drawn`): a drag, a pen path or text
  being typed shows once it lands, never as a preview. Undo and redo are commits like any other.
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
prototype's text field, then hands focus back to the canvas; the player closes from its bar.
Prototyping adds no chord to the keymap (the Prototype tab's reach is in
`design/prototyping-authoring.md`). *Play* is reached by the top bar's play button, a flow chip
and the palette; `Ctrl+Alt+Enter` stays present mode, now the fullscreen player. The player's commands (restart,
hints, detach, record) are buttons: the chords they would want are spent in the editor, and a chord
that means two things breaks the keymap's rule.

**Private mode** masks the page and flow names in the player bar and the detached window's title,
since sharing that window is private mode's case.

## 5. Interactions

Built: the triggers, actions, *Back*, scrolling and the hotspot hints' flash are in
`design/play.md`, their authoring in `design/prototyping-authoring.md`. A sticky child sticks at
the scrolling frame's edge (`design/play.md` *Scrolling*). The shell still draws the hints (§4).

## 6. Motion

Built: transitions, smart animate, timing and states animating, in `design/play.md` *Motion*.
Still to build:

- **Smart animate's direction**: under a direction, an unpaired layer enters from its side. The
  Prototype tab offers the direction; the engine fades every unpaired layer.
- **A state that loops** (a spinner, a skeleton's shimmer).

## 7. Logic and data

Built: variables during play, expressions, inputs and scripts, in `design/play.md`. Still to
build:

- **Lists from data**: a repeated row filled from a data file, reusing the chart's data model
  (`charts.md`). Its saved form is §10's first question.

## 8. Export

Built, in `design/prototyping-export.md`: the one HTML file and its size estimate, what goes in,
fonts in whole, and the browser build drawing the opening frame through WebGPU or `vello_cpu`.
Takes are recorded and replayed by the engine (`design/play.md` *Takes*). Still to build:

- **The page plays.** The engine runs in the page, with the device frame, player bar, hotspot
  hints and scaling (§1) the export's settings name. The exported player has no original to
  protect: it loads the file and plays on what it loaded, with the same engine as the in-app
  player, so the two behave the same.
- **The export dialog**, showing the file's size (`Parts::estimate`) before writing.
- **Video from a take**: the player's record button, and an export that replays a take on a fresh
  copy through the CPU backend, frame by frame, to MP4 or GIF; nothing is captured from the
  screen.
- **The flow map**: a PDF of each flow's frames with their connectors and every interaction written
  out in words, for review and sign-off.

## 9. The agent surface

Built: `design/prototyping-authoring.md` *Agents*.

## 10. Open questions

1. **Lists from data**: how a repeated row and its data file are saved, and how a row's layers bind
   a row's cells.
2. **The video encoder**: a crate and its licence for MP4; GIF needs none.
3. **A minimised main window**: whether an immediate viewport keeps the detached player drawing.
4. **Imported prototypes.** The Figma, Penpot and Sketch importers report prototype links as left
   out. Now that interactions exist, the ones that map (navigate, overlay, back, the common
   transitions) can be read in, and the rest reported as now.
5. **Run script in the exported file.** §2 promises it, but the page has no Luau: `mlua` builds
   Luau's C++, which `wasm32-unknown-unknown` cannot compile (`design/prototyping-export.md`).
   Options: Emscripten, wasi-sdk, or a Luau built apart and loaded beside the player.
