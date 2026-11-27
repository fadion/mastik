# Mastik — Roadmap

Open work only. **Now** is in the order it is taken up; **Next** is decided and follows it;
**Later** waits on a stated trigger;
**Considerations** are ideas not yet decided. Decided non-goals are in `non-goals.md`; built
work is described in `architecture.md`. Refs are file + function name, never line numbers. Read
the code before picking an item up — an item's premise can go stale while its ask still reads
true.

## Next · Editing parity

Mastik as a standalone tool: the effects, paints, media and path tools that would otherwise send
someone to another app or a plugin. Not in order. Two items share one prerequisite: the model's
`Brush` (`mastik-core` `image.rs`) is an alias of `peniko::Brush`, a closed enum of solid,
gradient and image, so a pattern fill and a mesh gradient each need Mastik's own paint enum in its
place, saved in the same shape so existing files load. It is matched in about 60 files.

- **Pattern fill.** A fill tiling any layer, frame or component, kept live as the source changes,
  with spacing, offset rows (a hexagonal layout) and scale. A tiled picture exists (`ImageFit::Tile`,
  drawn with `Extend::Repeat`). vello cannot take a scene as a brush, so the tile renders offscreen
  per zoom and feeds an image brush set to repeat, cached on the source's revision with damage
  following edits to it. Needs the paint enum above. SVG and PDF write a `<pattern>`; CSS a
  repeated SVG background; Penpot and Sketch a rasterised tile, reported. Large.
- **Animated GIF.** Plays on the canvas and in the player; still exports, goldens and code export
  take frame one. The decoder is compiled (`image`'s `AnimationDecoder` for GIF) and the file
  already stores the original bytes, so the save format does not change. `ImageStore` keeps frames
  and delays. One clock for every GIF: a frame is the time since the document opened, wrapped at
  the GIF's length, and after a draw the canvas asks for a repaint when the next visible GIF frame
  is due, none when no GIF is on screen (the canvas draws only when its `canvas::DrawnKey`
  changes). A *Pause animations* switch. Medium.
- **Video.** A muted, looping video as a fill, for hero sections and short feature clips.
  `images.md` rules video out today. H.264 in MP4 only at first: `openh264` (built with its
  decoder) and `mp4`, already used to encode a take (`mastik-export` `video.rs`), decode it; other
  containers and codecs need ffmpeg or each OS's media API. A hard cap on the source, then a
  transcode on import to a design preset (about 1920 px long side, 30 fps, 3–4 Mbps, audio
  dropped, a length cap), around 10–15 MB. Blockers: `ImageStore` holds stills and needs a
  time-indexed source; pictures are base64 in the JSON file, a third larger; the canvas repaints
  on the GIF clock; the exported player plays it through a `<video>` element whose frames reach
  the WebAssembly drawing; still exports take a poster frame. Check H.264 patent coverage first:
  Cisco's licence covers its prebuilt binary, not a build from source. Large.
- **Mesh gradients.** Mesh lines and colour points, blended across each patch (Coons patches). A
  mesh mode in the canvas's gradient editor (`gradient.rs`): click on a line to add one through the point,
  drag points and handles, pick a point's colour. peniko has linear, radial and sweep only, so the
  mesh renders, as *Noise* does, by evaluating the patches into an image cached per zoom, one
  function shared by the canvas and the PNG writer. krilla has no mesh shadings, so SVG, PDF and
  CSS get a picture, reported. PDF import already rasterises meshes (`pdf_in`). Needs the paint
  enum above. Large.
- **Image editing for quick fixes.** Not a retouching suite: no adjustment layers, painted masks or
  brushes.
  - *Levels*: a midtone gamma and output levels beside black point, white point and fade, in
    `ImageAdjust::tone_table`. Small.
  - *Curves per channel* and *Colour balance*: the one tone table becomes three
    (`ImageAdjust::pipeline`), which SVG's `feFuncR/G/B` express, so the canvas, PNG and SVG still
    agree. Small.
  - *Clone* and *Spot heal*, baked into the picture. A retouch mode on the picture whose dabs are
    previews with the mode's own undo; *Done* commits one rewritten picture through
    `MastikApp::rewrite_tx`, as *Reduce picture* and *Apply crop* do, the old entry dropped once
    nothing names it. It edits the original pixels, under the adjustments, mapping the brush
    through crop and orientation, and re-encodes in the picture's own format (a JPEG as JPEG).
    Spot heal is a Poisson blend of a small area, tens of milliseconds on the CPU. Medium.

## Later · Player

- **Draw only the room a tall screen shows.** The player renders a screen whole, and one past the
  device's largest texture side is scaled down and drawn softer (`design/prototyping-player.md`).
  Rendering only the part of the screen the stage shows, at full density, bounds both the texture
  and the per-frame cost. Needs a GPU-attended change and a check on a device.

## Later · Platform and dependencies

- **egui 0.36** is blocked by vello requiring wgpu 29 (check — last asked 2026-09-03). Re-derive
  `ui::menu_rows`' button-style arithmetic when it moves.

## Considerations

Ideas worth weighing, not yet decided or scheduled.

- **A lighter scripts player.** A file that runs a script carries about 2.6 MB more, gzipped,
  mostly the verb layer a script reaches the document through (`Host::verb` → `verbs::call`
  by name), a table of every verb that linking cannot thin. Scripts writing typed operations
  would let the page's build drop it, at the cost of scripts no longer going through the one
  verb registry that Command Mode and MCP share — which is worth more than the 2.6 MB
  (`design/prototyping-export.md` *The browser build*).

- **A control channel into the running app.** Loopback only: read UI state, drive tools with
  synthetic pointer and key events, and screenshot the window. Agents never launch the GUI because
  the window takes the user's mouse and keyboard; a window that accepts input only through the
  channel (hidden or offscreen) would let an agent check chrome against the real app instead of
  throwaway probe tests. It could grow from the local socket the window already listens on for
  open requests (`one_window`).
- **Multiplayer without a server.** The host's app holds the session and peers join by link, peer
  to peer over QUIC with NAT traversal and an end-to-end encrypted relay when no direct path exists.
  Edits merge per property, a delete beats an edit, and each person's undo takes back only their
  own edits; optional password, approval of each joiner, view-only access. §12 locks in the ids and
  transactions such a layer needs (components bend its fourth property, §15 D978) and leaves the
  protocol and conflict rules undesigned; this shape needs no hosting and no accounts. The cost:
  the session ends when the host closes the file.
- **A tolerant pixel diff.** A compare with anti-aliasing detection and SSIM that writes an HTML
  report. The PNG golden is absent because `vello_cpu`'s bytes vary with SIMD level and version
  (*Parked decisions*); a compare that forgives edge pixels and still catches a moved shape is what
  would unblock it. The same harness could hold approved screenshots of the chrome as regression
  baselines.
- **Background removal by a model.** The feature designers expect of an image card, but it brings
  a model and a native runtime, so it is a project of its own. The model must be MIT or Apache
  licensed: BiRefNet's lite variant (about 1024 px input, good on hair) cuts out a subject in one
  step; BRIA's RMBG is non-commercial without a licence. ONNX Runtime through `ort` with
  `load-dynamic` (DirectML, CoreML, CPU); BiRefNet's deformable convolutions rule out the pure-Rust
  runtimes and are the export's known snag. It runs in the reader child process, as PDFs do
  (`reader::pdf`), so a runtime or driver crash costs the action, not the app, and the main binary
  never links the runtime. Model and runtime are downloaded once when switched on in Settings,
  pinned by SHA-256 and cached per machine, as the icon sets are; nothing ships in the installer.
  The result is a matte stored as its own image entry and named by `ImageRef` (`matte`), so it
  shares the picture's framing, rides undo and save as any picture does, folds into the
  adjusted-pixel cache, and exports as an SVG luminance `<mask>` and CSS `mask-image`. The matte is
  scaled to full size by a guided filter over the photograph, since a plain upscale softens hair,
  and the card's *Shift edge* and *Feather* edit it without running the model again.
- **Gradient stops bound to a colour variable.** A colour binds a whole solid paint, never a
  gradient stop (`design/styles-variables.md` *Variables*), so a chart's *Fade* and a Sankey's
  *Gradient* flows draw flat on a bound colour and say so (`chart::generate::faded`,
  `flow_gradient`), and Colour review's merge writes a stop the variable's default-mode value
  (`color_review::Merged::unbound`). Binding a stop app-wide means a stop that holds a
  `BrushSource`-style binding, saved with a format bump and a migration and resolved wherever a
  paint is (`Document::resolved_paint`); every reader of a bound colour checked, about 130
  matches of `BrushSource::Bound` across core, render and export; the SVG, CSS, code, Sketch
  and DTCG writers writing a stop's variable; and a way to bind one stop in the gradient editor.
  It reverses the rule above. A chart-only fix that draws a fade or flow from the bound colour's
  value at the chart, redrawing when a variable's value changes as the ground stamp does
  (`Chart::grounded`), is about a day, but its parts and exports hold plain colours.

## Parked decisions

- Rich text model migration (`runs: Vec<(Range, TextStyle)>`, `architecture.md` §5.4).
- History cap / snapshot compaction for huge delete inverses.
- Font embedding for non-Google local fonts (`architecture.md` §5.4a).
- **Justify-all** — blocked upstream in parley (`align_impl`, `pub(crate)` internals); trigger is a
  parley patch (§15 D825).
- MCP transport hardening (auth, TCP/remote).
- `vello_hybrid` as a third backend if low-end GPUs demand it.
- **PNG goldens** — wait on an aarch64/NEON measurement (§15 D304); per-`vello_cpu` too (D414).
  `num_threads` is still a default.
- **`Ctrl+Alt` is `AltGr`** on many European layouts (`shortcuts.md` §12, §15 D987).
- **One font-availability question for panel and snapshot** — only worth it if the snapshot ever
  reads a live `FontService`.
