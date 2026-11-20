# Prototyping: the HTML export

How a prototype becomes one HTML file a client opens with a double-click: the browser build of
the player, the trim to the chosen flows, the fonts, the packing, and the page playing. The app's export
card is `prototyping-player.md`; *Run script* in the page is open (`prototyping.md`).
Code: `crates/mastik-web` (the page's player), `mastik-export/src/prototype.rs` (the packer and
the page's script), `mastik export --prototype` (the CLI).

## The browser build

The page runs `mastik-core`, `mastik-play` (without `scripts`) and `mastik-render` compiled to
`wasm32-unknown-unknown`, drawing
with vello on WebGPU, else with `vello_cpu` into a 2D canvas. Both are the canvas's own
renderer, so the page draws what the editor draws.

- **The page unwinds.** `mastik-core` refuses `panic = "abort"`, because `catch_unwind` round
  `flo_curves` and `i_overlay` is what keeps degenerate geometry from taking the process down,
  and a page is no different. Stable Rust's wasm32 std is built for abort and will not link a
  crate built to unwind, so the player is built on a pinned nightly with `-Zbuild-std` and
  `-C panic=unwind -C target-feature=+exception-handling` (wasm exception handling, which
  current Chrome, Edge, Firefox and Safari run). A `catch_unwind` round a panic returns there
  as on the desktop; on stable, the same crate fails to link against std.
- **`crates/mastik-web` is its own workspace**, excluded from the root one, with its own
  `rust-toolchain.toml` (the nightly), `.cargo/config.toml` (target, flags, `build-std`, a
  target dir under `target/web`) and `Cargo.lock`. Cargo reads those only when run in that
  folder, so the nightly never reaches the app's build, and `cargo test --workspace` neither
  builds nor checks it. `build.sh` there builds it.
- **wasm-bindgen is held at 0.2.108** (with `js-sys`/`web-sys` 0.3.85 and
  `wasm-bindgen-futures` 0.4.58, which move with it). From 0.2.126 a closure handed to
  JavaScript must be `UnwindSafe` under `panic=unwind`, and wgpu 29's WebGPU backend passes ones
  that are not; 0.2.108 is the oldest wgpu 29 takes. `wasm-bindgen-cli` must be that exact
  version. The futures crate still wants an exported `async fn`'s future unwind safe, so
  `mastik_web::start` awaits the GPU setup before it holds a `Resolved`.
- **Luau is not in the page.** `mlua` builds Luau's C++ with `cc`, which needs a C++
  compiler and a C/C++ runtime for the target; `wasm32-unknown-unknown` has neither, and the
  build fails at `mlua-sys`. Moving the player to `wasm32-unknown-emscripten`, which `mlua`
  supports, is closed: `wasm-bindgen` and wgpu's WebGPU backend need `wasm32-unknown-unknown`.
  What remains is Luau's C++ compiled by clang against wasi-sdk's sysroot (libc, libc++) into
  the same module, or a second module beside the player that runs a script on a copy and hands
  back its operations; `prototyping.md` holds the question. Until then the page plays
  without `scripts` and *Run script* is skipped with a notice.
- **What the player's code may not use**, because the page has none of it: threads
  (`std::thread::spawn` fails; `available_parallelism` errs, which the CPU effects' banding
  already reads as one), `std::time::Instant`/`SystemTime` (they panic; time comes from
  JavaScript), the file system, and blocking waits (`pollster`, a GPU `poll(wait)`): the page
  is one thread that must return to the browser.
- **WebGPU or not is decided before the canvas is claimed**: `navigator.gpu`, an adapter and a
  device are asked for first, and the canvas gets a WebGPU context only once all three
  answered, since a canvas that has one can never get a 2D one. `?cpu` in the page's address
  forces the CPU renderer.
- vello draws into an `Rgba8Unorm` storage texture, copied onto the canvas with wgpu's
  `TextureBlitter`: a canvas texture cannot be a storage target. The canvas takes a format
  that is not `-srgb`, since vello's output is already sRGB-encoded.

### What holds

Measured on Windows with Chrome and Edge (the in-app browser pane, and the two headless over
the DevTools protocol):

- The sample file renders the opening frame through WebGPU and through `?cpu`, over HTTP and
  opened from `file://`; a `file://` page is a secure context, so WebGPU is offered there.
  Both match `mastik export --png` of the same page.
- A panic caught by `catch_unwind` in a module built this way returns its fallback value
  (run under Node).
- A face the CLI found installed (Georgia) is embedded and drawn in the page.
- A file of two frames, a scrolling list, a push, an overlay and *Back* plays in the in-app
  pane through WebGPU and through `?cpu` over HTTP: the wheel scrolls the list, the push draws
  both frames mid-way, the overlay opens and closes, hints show, and no animation frame is
  asked for while play is still.
- The wasm is about 12.9 MB, 4.3 MB gzipped; it carries the bundled Inter, which the page
  never has to embed again.

## What goes in

`prototype::prune` trims the document to the flows the export takes:

- **The flows**: `PlayExport::flows` where they still exist, every flow when it names none
  (`chosen_flows`). The file opens on `PlayExport::opens` when it is among them, else the
  first. With no flow at all there is nothing to export.
- **The pages, whole** (`reached_pages`): each flow's page and its start's page, then every
  page a layer on a reached page navigates to, opens or swaps an overlay from, scrolls to or
  switches a state to (each target read through `resolve_target`, so an instance's reaction
  reaches its own layers), and every page holding the main of an instance on a reached page.
  Pages go in whole because everything on a reached page is reachable by the person playing
  it.
- **Everything document-wide stays**: styles, variables, scripts, the vault's vendored mains.
  They are small, and a variable or style is reached through bindings the trim would have to
  re-derive.
- **Pictures** go with the pages that showed them.
- Each kept page keeps only its chosen flows, and `PlayExport` names the chosen flows and the
  one the file opens on, so the page needs nothing else to start.

The trim runs through `take::take`, the door an import's trim uses (*Delete page* per page,
detaching instances of a main on a page that goes), so the pruned document passes the loader.

## Fonts

Every font file the pruned document's text was laid out in goes in whole, whatever its licence,
as the file the canvas drew from: the runs' faces, read off the resolved layouts, fallbacks
included. A face registered under another family's name (`text::register_fonts_as`) carries
that name, and the page registers it under it again. The bundled Inter is left out; the player
carries it. Whole files, not subsets, because a viewer types into text fields.

The fonts are those registered with the exporting thread's shaping engine. The CLI registers
the installed faces of the families the document names first (`SystemFonts::register_for`),
as `serve` does.

## The file

- **One gzip stream** holds a container: `MSTKPLAY`, a `u32` version (1), then per entry a
  kind byte (1 the wasm, 2 the document, 3 a font whose name is its alias, 4 a layer's CSS
  named by its id, 5 a file of a layer's export settings named by its id, a newline and its
  path), the name and the data, each behind a little-endian `u32` length. The packer writes it and the page's own
  script reads it; nothing else does.
- The stream is **base64'd once** into a `<script type="application/octet-stream">`, decoded
  in the page, unpacked with `DecompressionStream("gzip")`, and the wasm is instantiated from
  those bytes, never fetched: the file runs from `file://`.
- The player's `wasm-bindgen --target no-modules` loader is inlined as a classic script, any
  `</script` in it written `<\/script`.
- The page leaves its outcome on `<body>`: `data-backend` (`webgpu` or `cpu`) or
  `data-error`, and shows the error in place of the prototype.
- **The size before writing** is `Parts::estimate`: the parts packed at gzip's fastest level,
  base64'd, plus the page; the file is packed at the best level, so the estimate is a little
  over, never under.

## The player in the page

`mastik_web::start(canvas, document, cpu)` loads the document and starts `mastik-play`'s
`Player` on the flow the export opens on (`PlayExport::opens`, else the first flow, else the
first page's first frame). The page plays on what it loaded: there is no original to protect,
and the engine is the in-app player's, so the two behave the same. The shell keeps only the
canvas, the drawing and the page around them.

- **Each animation frame** `Player::frame(ms, room, dpr)` ticks the engine by the time since
  the last frame, places the screen, and draws `scene()` through `mastik_render::play` — on
  either backend, the canvas's backing store sized to the scene at the placement's scale times
  the device pixel ratio. A frame whose scene, dirty nodes and pixel size are unchanged is not
  drawn. The page asks for the next frame only while `is_moving()`; an input or a resize
  asks for one, the first ticking by 0.
- **Placing** (`shell::place`): the scaling is `PlayExport::scaling`, else the screen's page's
  own. *Fit* scales the whole device to the room, up or down; *Actual size* is one CSS pixel
  a unit; *Fill width* fits the width. A screen larger than the room grows the stage, which
  scrolls. The device frame, when the export shows it and the page names a device, is a dark
  bezel 14 units wide round the screen, 24 CSS pixels from the window's edges under *Fit*;
  a screen under 600 units on its short side wears a phone's corners (40), else a tablet's (18).
- **Input** goes to the engine in screen space, mapped back by the placement's scale: the
  canvas's pointer down, move, up, leave and cancel (captured during a press), the wheel
  (deltas in lines or pages made pixels; the page scrolls when no frame used it), and the
  window's keys without Ctrl, Alt or Meta. A key is named as a key trigger records it (egui's
  `Key::name`: `Left`, `Space`, `Comma`); while an input is focused a printable key is text.
- **Addresses** play asks to open (`take_requests`) open in a new tab from inside the input
  handler that caused them, while the browser still counts the act the viewer's. Notices go to
  the console; `ScriptsOff` is shown to the viewer once.
- **The player bar**, when the export shows it, runs along the foot: the flow's name (a list
  of the file's flows when it holds several, switching plays that flow from its start),
  *Restart*, *Hints* when the export shows hotspot hints, and *Inspect*.
- **Hotspot hints** are drawn on a 2D canvas over the screen: `hotspots()` as rectangles in the
  selection blue, held while *Hints* is on, else at `Scene::hint_flash`'s strength after a click
  on nothing. The pointer is a hand over a hotspot.
- **Inspect mode**, the bar's *Inspect* switch, stops interactions (the pointer leaves the
  prototype; keys go nowhere but Escape, which drops the pick) and opens a panel beside the
  stage. A click picks the topmost visible layer under it in the frame on top, by box
  (`Player::layer_at`); the picked layer is outlined, the hovered one dashed, and the distances
  between them — or to the picked layer's parent while nothing else is hovered — are drawn as
  the canvas's `Alt` measure quotes them (`mastik_core::measure::between`, its labels and
  colours). The panel shows the layer's name, its CSS as *Copy as CSS* writes it with a *Copy*
  button, and the files its export settings write, to download. The page has no exporter, so
  both are made when the file is: `prototype::inspect_css` for every layer (a layer whose CSS
  is over 32 KB is left out, and the panel says to copy it from Mastik) and
  `prototype::inspect_files`, rendered through `plan`. Boxes are read off the played copy's
  layout, moved by scroll offsets; a sticky or fixed child is boxed where it would scroll to.
- The page's arithmetic (`shell.rs`: placing, the pointer's and wheel's mapping, key names)
  builds on the desktop too, and `mastik-export`'s `tests/web_shell.rs` includes and tests it;
  `tests/prototype.rs` checks the page's script calls only what the player exports.

## The flow map

The prototype's other file is the flow map (`mastik_export::flow_map`; the player's *Export flow
map…*, `mastik export --flow-map`): a PDF giving each flow a page of its frames as they sit on
the canvas, drawn by the PDF export's path, with an arrow from each layer that moves the screen
and a numbered badge on each layer with a reaction, then pages writing those reactions out in
words, for review and sign-off.

## Where the CLI finds the player

`mastik export doc.mastik --prototype -o out.html` reads `mastik_web_bg.wasm` and
`mastik_web.js` from `MASTIK_PLAYER_DIR`, else `player/` beside the executable or one folder up
(`target/player`, where `build.sh` writes them), else `../share/mastik/player` or
`../Resources/player` from the executable's folder.

The release ships it on every platform: the Release workflow's `player` job runs `build.sh`
once on the pinned nightly and hands the folder to each leg, which puts it in `player/` beside
the executable (the Windows zip and installer, the Linux archive and AppImage), in
`Contents/Resources/player` in the macOS bundle, and in `/usr/share/mastik/player` in the .deb
and .rpm (`packaging/linux/stage-payload.sh`, from `MASTIK_PLAYER_DIR` or `target/player`).
