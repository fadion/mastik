# Prototyping: the HTML export

How a prototype becomes one HTML file a client opens with a double-click: the browser build of
the player, the trim to the chosen flows, the fonts, and the packing. What is not built yet (the
export dialog, the player bar, the play engine in the page, *Run script* in the page) is in
`prototyping.md` §8 and §10. Code:
`crates/mastik-web` (the page's player), `mastik-export/src/prototype.rs` (the packer),
`mastik export --prototype` (the CLI).

## The browser build

The page runs `mastik-core` and `mastik-render` compiled to `wasm32-unknown-unknown`, drawing
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
  build fails at `mlua-sys`. Hosting it would take Emscripten or wasi-sdk (downloads beyond
  rustup), or a Luau built apart and loaded beside the player. Until then an exported file does
  not run *Run script*.
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
- The wasm is about 11.8 MB, 4.0 MB gzipped; it carries the bundled Inter, which the page
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
  kind byte (1 the wasm, 2 the document, 3 a font whose name is its alias), the name and the
  data, each behind a little-endian `u32` length. The packer writes it and the page's own
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

`mastik_web::start(canvas, document, cpu)` loads the document, settles it, and shows one
screen, `Player::screen`: the start frame of the flow the export opens on, fitted to the
window. That method is the seam the play engine fills: it will own the played copy and the
current screen, and the shell keeps drawing and the canvas.

## Where the CLI finds the player

`mastik export doc.mastik --prototype -o out.html` reads `mastik_web_bg.wasm` and
`mastik_web.js` from `MASTIK_PLAYER_DIR`, else `player/` beside the executable or one folder up
(`target/player`, where `build.sh` writes them). Shipping the player beside the release
executable is part of the release build, not yet done.
