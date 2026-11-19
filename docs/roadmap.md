# Mastik — Roadmap

Open work only. **Now** is in the order it is taken up; **Later** waits on a stated trigger;
**Considerations** are ideas not yet decided. Decided non-goals are in `non-goals.md`; built
work is described in `architecture.md`. Refs are file + function name, never line numbers. Read
the code before picking an item up — an item's premise can go stale while its ask still reads
true.

## Now · Misc

- **A centroid snap during a move.** While the Align card's visual-centre toggle is on, a moved
  layer's visual centre (`mastik_core::centroid`) is a snap source in `snap::snap_rect` beside its
  box's centre, so a move snaps by the same centre the toggle aligns by.
- **Reading a sharpen back from SVG.** The export writes a sharpen as `feGaussianBlur` plus an
  arithmetic `feComposite` (k2 = 1 + a, k3 = −a), and `svg_in` does not recognise the pair, so a
  Mastik SVG re-imported loses it. Key on the exact coefficients, so other tools' filters are not
  misread as a sharpen.
- **A group instance's canvas tag.** A group instance sitting on its page wears the tag a frame
  instance does, drawn only while it is selected or entered; today `app::wears_tag` gives a group a
  tag only as a main, so a group instance shows one only while entered.
- **A placed transform origin for a multi-selection.** One layer, a group among them, takes a
  placed pivot (`SetPivot`, `tools.md` *Transform origin*); several always turn about their union
  centre (`Handles::pivot`). The marker and the origin fields are offered over several layers
  too, the origin held by the session for that exact selection (a selection is not saved), and
  read by everything `Handles::pivot` feeds.

## Now · Prototyping

A release of its own: the document played, with interactions, flows,
variables and scripts during play, smart animate and springs, a player over the canvas, detached
into its own window or fullscreen, and export as one HTML file running the player in WebAssembly.
The design and its open questions are in `prototyping.md`.

- **Inspect mode in the exported player.** A switch in the HTML export's player that stops
  interactions and lets the viewer pick layers: a layer's CSS as *Copy as CSS* writes it, distances
  to its neighbours as the canvas's `Alt` measure quotes them (`measure.rs`), and its export
  settings' files to download. It gives a developer a handoff with no server, from the file the
  designer already sends.
- **Device and phone mirroring.** The app serves the prototype player to the local network and
  shows a QR code; a phone on the same network plays the page and follows edits. Not hosting: it
  ends with the app, needs no account and never leaves the network.
- **Annotations.** Notes pinned to a layer that say what the drawing cannot ("this list scrolls",
  "48 px touch target"), shown on the canvas, in inspect mode and atop the layer's code in code
  export. They are specification, not conversation: no replies, no authors, no resolved state,
  which keeps them apart from the comment tool `non-goals.md` declines.

## Later · Platform and dependencies

- **egui 0.36** is blocked by vello requiring wgpu 29 (check — last asked 2026-09-03). Re-derive
  `ui::menu_rows`' button-style arithmetic when it moves.

## Considerations

Ideas worth weighing, not yet decided or scheduled.

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
- **Chrome hairline base** in `rulers::snap_across_axis` (guides) is unruled (§15 D479). The pixel
  grid no longer calls it: it places lines from its lattice, which
  `the_lattice_picks_the_pixels_the_snapped_lines_did` holds to the pixels `snap_across_axis` picks,
  so ruling the base means moving the lattice with it.
- **`Ctrl+Alt` is `AltGr`** on many European layouts (`shortcuts.md` §12, §15 D987).
- **One font-availability question for panel and snapshot** — only worth it if the snapshot ever
  reads a live `FontService`.
