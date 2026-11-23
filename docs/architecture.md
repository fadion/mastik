# Mastik — Architecture

Mastik is a native 2D design tool: a Rust workspace of `mastik-core` (the headless document
model), `mastik-render` (vello on GPU and CPU), `mastik-export` (SVG, PDF, PNG, HTML/CSS, React, Vue, JSON), `mastik-mcp`
(the agent adapter), `mastik-ai` (the assistant's agent harnesses), `mastik-vm` (Command Mode's Luau), `mastik-play` (the prototype play engine) and `mastik-app` (egui + wgpu), with `mastik-web` (the exported prototype's player in WebAssembly) a workspace of its own. This file is the index. The design is in
`docs/design/`, one file per subsystem. **Open only the file you need.** Headings keep the
old section numbers, so a `§5.3d` cited from code is found by grepping `docs/design/` for
`5.3d`.

Key decisions are in `decisions.md`, non-goals in `non-goals.md`, open work in
`roadmap.md`.

## Invariants (`design/invariants.md`)

1. Core is headless. 2. Single mutation path: `Document::apply(Transaction)`.
3. Identity is stable, never reused, and caller-minted. 4. The document is truth; everything
else is derived. 5. Uncommitted state never enters the document. 6. Local transforms only.
7. Colour space discipline. 8. Operations are total and validated. 9. Serialization is
versioned and deterministic. 10. MCP is a thin adapter. 11. Renderers sit behind one
shared walk. 12. The distribution identity never changes once shipped.

Read the file for the exact wording before relying on one.

## Design files

| File | § | Covers |
| --- | --- | --- |
| `foundations.md` | 1–3, 10, 12–14 | Scope, stack, workspace and dependency rules, milestones, multiplayer readiness, Command Mode's decisions, deferred decisions |
| `invariants.md` | 4 | The twelve invariants |
| `testing.md` | 11 | Testing strategy, goldens |
| **Core** | | |
| `core-model.md` | 5–5.3, 5.5 | Module layout, identity, the node model, the document, pages and guides |
| `core-ops.md` | 5.6–5.10 | Transform and resize semantics, operations, builders (`build.rs`), layer names, history, `Resolved`, queries |
| `effects.md` | 5.3a, 6.4 | Layer effects: model and render passes |
| `layout.md` | 5.3b, 5.3c | Layout grids; container layout (flexbox, grid, insets) |
| `components.md` | 5.3d | Components, instances, overrides, variants, swap, reset, push to main, interaction states, lineage keys, documentation links |
| `kits.md` | 5.3e | Kits: shared component libraries |
| `styles-variables.md` | 5.3f | Variables, collections and modes (a collection picked by width, width queries), scope, bindings, styles, design tokens import and the linked tokens file |
| `text.md` | 5.4, 5.4a | The text node and fonts, spell check |
| `images.md` | 5.5a | Images; the brush, gradient stops, ends and mixing |
| `io.md` | 5.11, 5.11a | Save format, schema migration, clipboard envelope, the library block |
| `prototyping-model.md` | — | The saved prototype: a layer's reactions and play settings, a page's flows, the export settings, their ops, dangling targets, instances and copies |
| `play.md` | — | The play engine (`mastik-play`): the played copy and its door, play values, play state, the shell's contract, triggers, actions, inputs, scrolling, a layer that follows the drag, a scroll effect, motion, live update, takes; the expression language |
| **Render, export, MCP** | | |
| `render.md` | 6.1–6.3 | The render boundary, `RenderOverrides`, backends |
| `export.md` | 7 | SVG, PDF, PNG, ICO and ICNS, code export (HTML/CSS, React, Vue), the Penpot and Sketch files, JSON snapshot, the CLI, export doors |
| `prototyping-export.md` | — | The prototype as one HTML file: the browser build (`mastik-web`, nightly, unwinding), the trim to the chosen flows, fonts, the packed file and its size estimate, the page playing and its inspect mode, the flow map PDF, where the CLI finds the player and how the release ships it |
| `mcp.md` | 8 | MCP topology, headless mode, concurrency, the tool list (a test parses §8.4), the assistant's tool and grants |
| `assistant.md` | ai.md 1–7 | The assistant: agent harnesses (`mastik-ai`), sealing, access levels, proposals, the panel, attaching layers, the frame scope, its settings and the agent log |
| `command-mode.md` | vm.md 1–12 | Command Mode: the Luau sandbox (`mastik-vm`), the bar's sugar, name resolution, collections and properties, one undo step per run, budgets, the log, where scripts live (library, document, kit), the disclosure, the header, the script editor and its language help, the command bar, its `/ai` requests and its Pick focus with leap labels |
| **App** | | |
| `app-chrome.md` | 9.1, 9.2 | UI chrome and panel layout, the dark and light themes, the icon font, the title bar and the tab strip, the layers tree, the Pages section and the current page, the History section |
| `app-dataflow.md` | 9.3 | The one-way valve: gestures, previews, the commit door |
| `tools.md` | 9.4 | The v1 tools, the tool rail and canvas interaction |
| `app-selection.md` | 9.4 | Selection on the canvas and in the layers panel |
| `app-create-gestures.md` | 9.4 | Creation gestures |
| `app-pen-and-nodes.md` | 9.4 | Pen, pencil and node tools, *Pixel-fit* |
| `app-booleans.md` | 9.4 | Booleans, flatten, outline, the shape builder and Divide, Blend shapes, masks |
| `app-frames-and-copies.md` | 9.4 | Frames on the canvas, reparenting, copies |
| `app-snapping.md` | 9.4 | Snapping |
| `app-rulers-guides.md` | 9.4 | Rulers and guides |
| `app-autopan.md` | 9.4 | Auto-pan |
| `app-canvas-chrome.md` | 9.4 | Canvas chrome, the minimap, the size preview, the contrast check mode, the vision simulation, top-bar menus, the command palette, batch rename, clean up, fill with content, the assets browser and credits, find and replace in text, colour review, system health, the shortcut cheatsheet, focus mode, the peek (`\` held), private mode, present mode |
| `app-compare.md` | 9.4 | Compare with a version: the read-only mode, its marks, bar, Then/Swipe and changes card |
| `app-image-editing.md` | 9.4 | Image fills and image editing |
| `charts.md` | — | Charts: a frame carrying a chart description, its generated parts, reading a data set, the Chart card, the data window, import and the linked data file, flatten, and binding its colours to variables |
| `lists.md` | — | Lists from data: a frame repeating an instance per row of a data set, the document's data sets, regeneration inside the edit's undo step, where a list may stand, picture columns, a list's filter and sort, the List card, the linked file, the data window over a data set, *Data tables*, a row opened in play |
| `inspector.md` | 9.4 | The inspector cards, an annotation's card and its mark on the canvas |
| `prototyping-authoring.md` | — | Prototype mode, the interactions and Prototype cards, connectors, the link dots and flow chips, instances' interactions, kit publishing, agents |
| `prototyping-player.md` | — | The player in the app: starting and stopping, present mode as the player, following the editor's commits, over the canvas, detached and fullscreen, its keys, bar, flow menu and state card, and the one export card (HTML file, video from a take, flow map) |
| `type-panel.md` | 9.4 | The Type panel and typography popup |
| `font-picker.md` | 9.4 | The font picker |
| `library.md` | 9.5 | The library: dashboard, projects, search and its content index, versions, trash, *Import files*; document tabs (open, park, close, next launch) |

Also in `docs/`: `shortcuts.md` (the keymap), `context-menus.md` (the menus), `prototyping.md`
(prototyping's aims, where each built part is described, and what is still open).

## Keeping this true

Each file states the design as it is now. When a change alters a rule, rewrite the passage
that states it, in place — no amendment notes, no history. When a file outgrows ~40 KB,
split it and add the row here.
