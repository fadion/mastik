# Mastik — prototyping

> **Status: built, three questions open.** Each built part's rules live in a file under
> `design/` (below). Still open: *Run script* in the exported page, lists from data, and whether a
> minimised main window keeps a detached player drawing.

A prototype is the document played: frames linked by interactions, driven by variables and
scripts, animated between states, shown beside the editor while it is edited, and exported as one
HTML file a client opens with a double-click. Figma's model of linked frames is the spine, so it
reads as designers and clients expect; the logic reaches ProtoPie's and Axure's depth through
variables, conditions and Luau; the output is a real player, not a slideshow of images.

## Where each part lives

| Part | File |
| --- | --- |
| The saved model: reactions, play settings, flows, the export settings, dangling targets, instances and copies | `design/prototyping-model.md` |
| The engine: the played copy, play values and state, triggers and actions, inputs and focus, scrolling, motion (smart animate's direction, states that loop), live update, takes and video, expressions | `design/play.md` |
| Drawing a frame of play on either backend | `design/render.md` *Play* |
| Authoring: the Prototype tab, connectors, the link knob, flow chips, instances, kit publishing, agents | `design/prototyping-authoring.md` |
| The player in the app: present mode, following the editor, over the canvas, detached, fullscreen, keys, the bar, the export card, video and the flow map | `design/prototyping-player.md` |
| The exported page: the browser build, the trim, fonts, the file, the page playing, inspect mode, the flow map, the release's player | `design/prototyping-export.md` |
| Scripts a reaction runs: document scripts; a kit's are copied in when vendored | `design/command-mode.md` §10, `design/kits.md` |
| Imported prototype links and flows (Figma, Penpot, Sketch) | `design/library.md` *Import files* |

**Derived** — rebuilt when needed, never stored: smart animate's pairing of layers between two
frames, the in-between frames, the flow map's connectors, hotspot rectangles, and the copy's
`Resolved`.

## Still open

### *Run script* in the exported page

The exported file should run its scripts in the same sandbox as the app, against its own copy.
Today the page plays without `scripts`: a *Run script* is skipped and the viewer told once
(`design/prototyping-export.md` *The browser build*). The page has no Luau because `mlua` builds
Luau's C++, and `wasm32-unknown-unknown` has no C/C++ toolchain or runtime to build it with;
moving the player to Emscripten is closed, since `wasm-bindgen` and wgpu's WebGPU backend need
`wasm32-unknown-unknown`.

- **Recommended:** Luau's C++ compiled by clang against a wasi-sdk sysroot (libc, libc++) into the
  same module. An estimated 0.6–0.9 MB more, gzipped, and CI needs a pinned wasi-sdk.
- **The alternative:** a second module built by Emscripten beside the player, which runs a script
  on a copy and hands back its operations.
- **Rejected:** a Lua written in Rust, which is not Luau, so a script would run differently in the
  page than in the app.

### Lists from data

A repeated row filled from a data file, reusing the chart's data model (`design/charts.md`).
Open: how a repeated row and its data file are saved, and how a row's layers bind a row's cells.

### A minimised main window

Whether an immediate viewport keeps a detached player drawing while the main window is
minimised.
