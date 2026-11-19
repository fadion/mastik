# Mastik — Decided non-goals

What is deliberately not being built. An item here is decided and not open work; do not re-report
it as a gap. Open work is in `roadmap.md`, and multiplayer is one of its considerations.

## Scope

- **Plugins.**
- **Multiplayer** — kept possible by the model (`architecture.md` §12), not built.
- **A comment tool.**
- **A unit system** — a screen tool's units are all px at 1x; density belongs to export (§15 D358).
- **Custom key bindings** — the keymap is fixed (`shortcuts.md`).
- **A UI scale setting.**
- **Moving the UI to another toolkit** (gpui, Xilem) — egui composites the wgpu canvas directly and serves the chrome (`app-chrome.md`).

## Canvas and interaction

- **The eyedropper sampling outside the window** — it samples the canvas and the app's own chrome
  (`tools.md`, *The eyedropper*); off-window would need a capture path per OS and a conversion from
  the display's colour space.
- **A compare's Swipe handle following a hovered pointer** — the handle moves only when dragged (`app-compare.md`).
- **A modifier's badge painted beside the pointer as canvas chrome** — tried and declined; the badge is drawn into the cursor itself (`tools.md`, *Canvas cursors*).
- **Nudging the transform origin from the keyboard** — the origin pills take typed values (§15 D308).
- **Aligning a multi-selection to its container** — select and align one at a time; mixed parents have no one container (§15 D309).
- **A lock blocking inspector edits** — a lock guards against canvas accidents, not deliberate panel edits (§15 D323).
- **Paste at the pointer for `Ctrl+V`** — `Ctrl+V` keeps its fixed offset; *Paste here* is the pointer-aimed one (§15 D300).
- **A frame's *Background…* context-menu row** — the Fill row already does it (§15 D800).
- **A cached local box on `Resolved`** — the chrome's six walks a frame are an accepted cost, and a gesture would bypass the cache (§15 D778).
- **Retained per-frame scenes or effect results kept across frames** — a pan, a preview and an edit already redraw only what they changed (`render.md` §6.3); the caches would serve only a full redraw at an unchanged scale, never the zoom gesture.
- **A rotation handle or mid-edge squares** — sides still resize (§15 D14).
- **A pixel grid drawn coarser than one unit** — the drawn grid matches the 1px lattice snapping uses; a coarser one has no workflow that needs it.
- **A user-draggable ruler origin** — the rulers would disagree with the world-space inspector; rulers re-origin only on a lone selected frame (§15 D137, D36).
- **Middle-button pan over the origin fields** (§15 D254).
- **`Tab` held inside a menu or a dropdown's list** — a desktop menu's keys are its arrows, `Enter` and `Escape`, and `Tab` does not cycle it; popovers, dialogs and cards hold `Tab`, menus do not (`app-chrome.md`, *Two kinds of focus*).
- **One insertion line for the layers tree and the paint panels' reorder** — a gap cannot name a parent (§15 D306).
- **A resizable inspector column** — its width is a function of the design (§15 D351).
- **Unique layer names** — names only need to be displayable (§15 D127). Batch rename makes no name unique, and its preview does not flag duplicates.
- **Batch renaming pages, styles or variables** — the box renames layers only.
- **Letter-case switches in batch rename** (*Title Case*, *UPPER*, …) — *Keep case* carries a match's case, and nothing else changes it.
- **A grid's `baseline` alignment in the cards** (§15 D919).
- **A polygonal (click-by-click) lasso, a contain-only lasso rule, or shape-testing for the box marquee** — the lasso touches shapes, the box keeps its bounds rule.
- **On the size preview: an act on a tile's click, a ground colour of one's own, or a separate drawing per size** — the card shows the layer as it is, at fixed sizes on fixed grounds (`app-canvas-chrome.md`, *The size preview*).
- **On a loose selection's spacing handles: bands over uneven gaps, a negative gap, a value typed on a band, or a Gap field in the Align card** — Distribute or Tidy up evens uneven gaps, after which the bands appear.
- **A group hovered or clicked anywhere in its box** — a group is hit only where a child draws, so the gaps inside it fall through to what is behind: a large group would otherwise catch every click over its empty space, and design tools of this class hit groups the same way. The hover ring traces the group's box for legibility only; a section that should take clicks across its whole box is a frame (`core-ops.md`, `contains_local`).

## Booleans and paths

- **A per-operand boolean cache** — measured, nothing to win (§15 D239).
- **`Flatten` on a lone non-boolean** other than a mirrored path, which it makes plain — *Outline shape* (`build::outline`) is that verb (§15 D230).
- **Pressure, tilt, variable width, brushes, an eraser or a smoothing setting on the pencil** — it makes geometry, not ink; *Simplify path* already smooths after the fact.
- **Vector networks** — a model change with no half-step, since every reader and export takes a `BezPath`; the shape builder, point snapping and path cutting cover the same needs with plain paths.

## Effects and paint

- **Variable-width profiles and separate start/end caps** — a `Stroke` is ink at one width along an outline (§15 D126). A line's arrowheads are markers, shapes of their own in the stroke's paint, not ink (`core-model.md`, `Stroke`).
- **A cap on the number of fills or strokes** (§15 D133).
- **A pool of reusable effect-pass renderers** — the build cost is a one-off hitch, and a pool conflicts with a correctness rule (§15 D779).
- **Reading `Filters` (brightness/contrast) back from SVG** — the combined matrix doesn't identify its inputs (§15 D411).
- **Wide-gamut colour (Display P3)** — the document stores sRGB. A P3 document would need a tag or a conversion in every export, every colour calculation (contrast, vision simulation, colour review) and every copy between documents, and a display path per OS that wgpu does not expose; most viewers of the output would see sRGB anyway. OKLCH is a way to pick sRGB colours, its picks past the gamut mapped into it (`app-dataflow.md`, *Colour picker*).
- **Diamond gradients** — Figma-only; peniko has no such kind, so the model would need its own gradient type for it. The Figma import reads one as radial and says so.
- **A CSS display format in the picker** — CSS colours are typed or pasted into any colour field; the value row shows Hex, RGB, HSL, HSB or OKLCH.
- **Setting one channel across layers that disagree** — over a *Mixed* row the value row is the text field, which sets the whole colour.
- **Pattern fills (a layer or component tiled as a fill)** — niche outside illustration, a tiled image fill covers the common case, and CSS can only repeat an image.

## Text

- **Rich text runs** (check — character spans exist; this may mean only a separate runs model).
- **Vertical writing mode** — and its OpenType tags stay withheld (§15 D279).
- **Hyphenation** — declined on price, not blocked (§15 D399).
- **Tab stops, columns, widow/orphan control** — own builds nobody has asked for (§15 D825).

## SVG import

- **`<foreignObject>`** — HTML; nothing in the model draws it; reported as skipped (§15 D813).
- **Rejoining a multi-line paragraph** — the signal is indistinguishable from stacked labels (§15 D826).

## Images

- **Formats beyond PNG, JPEG, WebP and GIF** (TIFF, BMP, …) (§15 D191, D600).
- **Linked images** — *Embed*/*Link*, *Relink…* and *Embed all*; no gesture authors a linked source (§15 D819).

## Library and files

- **Built-in design-system kits** (Material and the like) shipped inside the app — kits are offered for download on the Mastik website and land in the library as any kit does, which leaves room for kits people contribute.
- **A multiple selection in the library** — every verb takes one document (§15 D383).
- **Rollback after a partly-failed migration** — the retry is the recovery (§15 D814).
- **Per-file "missing" marks** — the unit that goes missing is the whole library (§15 D384).
- **Disabling or warning-styling *Open* on an unreadable card** — the mark already says it (§15 D818).
- **Detecting OneDrive's machine-name conflict copies** — indistinguishable from a typed name (§15 D375).
- **Queueing `Ctrl+S` and document-leaving transitions on the writer thread** — they read the result on the next line (§15 D393).
- **A "remember last page" dashboard option** — the default page is a Settings preference (§15 D749).
- **A binary save format** — the versioned JSON format is the file; it diffs, merges and recovers by hand, and nothing has asked for a smaller or faster one.
- **Reading Figma's `.fig` binary** — the format is unpublished, reading it reverse-engineers another company's product, and it breaks when Figma changes it; Figma files come in by link through the documented REST API.
- **EPS import** — PostScript is a program, and reading it needs an interpreter (Ghostscript, AGPL); PDF and AI import cover the vector files that matter.
- **Choosing an icon kit's variant in the Icons tab** — a set can have several properties, and the tab would have to enforce one or grow a picker per property; the tab places the default variant as an instance and its Instance card switches variants.
- **Recolouring an icon kit's icons on placement** — an icon lands in the kit's own colours; fill overrides on the instance recolour it.

## Components and layout

- **Pairing a state's layer with rest's by hand** — states pair by lineage key, then name path (§15 D982); an unpaired layer is fixed on the layer itself, where the Variants card's *Select it* leads.
- **A verb that un-makes a component** — delete the main, which detaches its instances, or detach an instance.
- **A main's guides on its instances** — a guide is a working aid for drawing the main, not part of the component (`components.md`).
- **Clean up removing layout from one-child frames** — the layout still pads, aligns and hugs, and code export writes it as flex.
- **CSS block, inline and float layout, `calc`, `order`, and margins other than `auto`** — layout is flexbox and grid; gap and padding stand in for margins, and the layers panel's order for `order`.

## Styles and variables

- **Binding a `Path`'s corner radii** — unbindable by decision (§15 D1057).
- **Editing a style with no users** — *Edit* works through a style's users (§15 D1063).
- **Per-node overrides keyed by breakpoint** — every reader would take them; a collection picked by width carries breakpoints instead.
- **A kit's collections in the variables browser's collection picker** — a kit's variables list under the document collection they map onto, and its modes are chosen through that mapping.

## Export

- **Cutting a frame into a grid of slices, nine-slice scaling, and a slice bound to a layer's bounds** — a layer's own Export already exports its bounds.
- **A web favicon bundle, BMP entries in an ICO, and a Linux icon set** — an icon file holds PNG entries, in the Favicon, Windows and macOS sets (`export.md`, *Icon files*).

## Code export

- **Native targets** (SwiftUI, Compose, Flutter, Floem) — an agent porting the HTML does it better (§15 D1098).
- **Tailwind and CSS-in-JS in v1** (§15 D1098).
- **Code layers, and importing HTML/CSS or framework code as layers** — too complex fully, messy partly; SVG import is untouched (§15 D1099).
- **The code fidelity check in the app** — it needs an external browser and cannot check React or Vue; `mastik check code` stays the way to run it.
- **Compiling the React and Vue output in CI** (`tsc`, `@vue/compiler-sfc`) — a Node toolchain in CI for a syntax check; the fidelity tests draw the HTML and the component walk.
- **One `<main>` across a page and the components it uses in React and Vue** — a component is one file for every page using it, so each file counts its own main; a page's main beside a use holding one writes two.

## Prototyping

- **A keyframe timeline** — transitions, smart animate, springs and looping states cover an app's motion; a timeline is a motion tool (`prototyping.md`).
- **A node graph of patches** — triggers, actions, conditions and Luau scripts carry the logic without a second language.
- **Several devices and sensors talking to each other** — one player, one screen.
- **Gamepad triggers** — neither winit nor egui delivers gamepad input, and a prototype of an app is driven by pointer and keys.
- **Rive and Lottie export** — the player is the prototype's output; motion assets are another product.
- **Comments in the player** — the comment tool is a non-goal, in the player as on the canvas.
- **Hosted links** — the export is one HTML file the designer sends or hosts anywhere; Mastik runs no server.

## Charts

- **Chart kinds past the seven** (radar, heatmap, treemap, funnel, gauge, candlestick, maps) — a flattened chart or plain shapes draw them (`charts.md`).
- **Sankey loops** (flows back to a node already passed) — refused with the loop named; a loop needs ribbons that turn backwards.
- **Combo charts and a second value axis** — two charts stacked in one frame do it without a dual-scale design.
- **A time scale** — the category axis is categorical; dates read as labels in table order.
- **Editing one part of a live chart** — series and value options cover the tweaks; past them, flatten.
- **A chart inside a component, an instance or a kit** — a resized instance would regenerate parts its main lacks; flatten first.
- **Data from a URL, formulas in the table, and data bound to variables** — the table is values, from typing, pasting or a local file.
- **Interactivity** (hover readouts, tooltips, animation) — a chart is a picture of its data, on the canvas and in code export.
- **A chart's own title or caption** — a text layer beside it does that.
- **Saved user presets** — duplicate a styled chart and replace its data.
- **Refreshing a linked file without a click** — an opened document's link can name any file on the machine (`charts.md`, *Import and the linked file*).

## The assistant

- **An embedded model, or a provider API key held by Mastik** — the harness holds the model, the account and the billing (`design/assistant.md` §1).
- **An approval prompt inside a turn** — a headless harness cannot pause for one; the access level is the approval.
- **The agent's own tools** (shell, files, web) inside the assistant.
- **Kits and the library through the assistant** — Phase 4's kit and library tools are never a chat's (the assets browser's two are).
- **A chat saved in the document** — a chat is the person's, not the work's.
- **The Agent Client Protocol as the assistant's transport** — Claude Code and Codex speak it only through adapters the person would install; each harness is driven through its own headless mode (`design/assistant.md`).
- **Gemini CLI as a harness** — Google withdrew its OAuth sign-in for individuals in favour of Antigravity, which is driven.

## Command Mode

- **Constraints or reactivity** — an expression is evaluated once and forgotten (`foundations.md` §13 decision 1).
- **A script run by the document** — on open, on a change, or on any trigger but the person's own (§15 D1100); a script the document re-runs is a constraint engine.
- **Links between a document's scripts and the library's** — they exchange copies; a kit's scripts are the one link (`command-mode.md` §10).
- **Plugins or UI extension from a script** — a script edits the document; panels, tools and chrome are a different feature with a different security design.
- **Async or coroutines inside a script** — a script cannot yield to the host; a run, on a worker or not, lands as one step (`command-mode.md` §7, §8).
- **Binding the interaction layer** — gestures, previews and `RenderOverrides` have no scriptable form; a script says a committed outcome (`command-mode.md` §6).
- **Keyboard navigation as a mode of its own** — walking layers by key never beats the mouse in a design tool; picking layers by label is the command bar's Pick focus, for naming layers while typing (`command-mode.md`, *Pick*).

## Tests and goldens

- **`f32_pipeline` for `OptimizeQuality`, and keeping strokes out of the PNG fixture** (§15 D304).
