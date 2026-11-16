# Mastik — Roadmap

Open work only. **Now** is in the order it is taken up; **Later** waits on a stated trigger;
**Considerations** are ideas not yet decided. Decided non-goals are in `non-goals.md`; built
work is described in `architecture.md`. Refs are file + function name, never line numbers. Read
the code before picking an item up — an item's premise can go stale while its ask still reads
true.

## Now · Prototyping

A release of its own: the document played, with interactions, flows,
variables and scripts during play, smart animate and springs, a player over the canvas, detached
into its own window or fullscreen, and export as one HTML file running the player in WebAssembly.
The design and its open questions are in `prototyping.md`.

## Now · Image editing

Make the image-editing card feel complete without turning Mastik into a photo editor
(`design/app-image-editing.md`, `design/images.md`). The bound is that the canvas, the PNG export
and the SVG export all draw the same thing. An adjustment must stay a colour matrix plus a tone
table (`AdjustPipeline`), a neighbourhood filter goes in the effect stack with an SVG filter to
match, and only an explicit action changes a picture's stored bytes.

- **Quarter turn.** A *Rotate 90°* beside the Settings tab's two mirrors. `OrientOp::Turn` is
  built and carries the crop, but nothing writes it, so four of the eight orientations cannot be
  reached. A photo shot sideways is the commonest repair.
- **Crop aspect.** Presets in the card (Free, Original, 1:1, 4:3, 3:2, 16:9) and Shift on the crop
  handles holding the ratio, as Shift does on a resize (`edit::crop_resized`). A crop is usually
  made for a slot of known shape.
- **Black point, white point, fade.** Three more `Adjustment`s, each only a different tone table,
  so the SVG writer needs no new primitive. Fade (lifted blacks) is the muted look duotone almost
  covers.
- **Purge unused images, and what each picture weighs.** Every picture is embedded and nothing
  collects the table (`Document::images`), so a file grows with no way to see why. The op exists
  (`Operation::RemoveImage`); the command and a size readout in the card do not.
- **Sharpen.** An unsharp mask (original + k·(original − blur)) in the effect stack, reusing the
  blur passes in `fx.wgsl`; SVG writes it as `feGaussianBlur` plus an arithmetic `feComposite`.
- **Reduce the picture, apply the crop.** Explicit, destructive actions with their own undo step:
  re-encode at a chosen size or quality, or keep only the cropped pixels. A large photograph
  placed as a thumbnail is what makes files heavy. They are the exception to "the original goes
  out", so they never run on their own.
- **Curves.** An editor for the tone table `AdjustPipeline` already holds. A real piece of chrome,
  so it follows black and white point and is taken up only if those see use.

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
- **A centroid snap during a move.** A layer's visual centre (`mastik_core::centroid`) as a source
  in `snap_rect` while the Align card's visual-centre toggle is on. Worth weighing only once the
  toggle has seen use.
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

- Binary save format (the versioned JSON format ships).
- Rich text model migration (`runs: Vec<(Range, TextStyle)>`, `architecture.md` §5.4).
- History cap / snapshot compaction for huge delete inverses.
- Font embedding for non-Google local fonts (`architecture.md` §5.4a).
- **Justify-all** — blocked upstream in parley (`align_impl`, `pub(crate)` internals); trigger is a
  parley patch (§15 D825).
- **A stored, user-placed transform origin for a multi-selection** — unparks when a user asks for it.
- UI toolkit revisit (gpui / Xilem), `architecture.md` §9.1.
- MCP transport hardening (auth, TCP/remote).
- **Scripts reaching outside the document** — writing files, reading images; two shapes, neither
  ruled (`vm.md`).
- **ACP as the assistant's transport** — until Claude Code speaks it without an adapter
  (`design/assistant.md` §2).
- `vello_hybrid` as a third backend if low-end GPUs demand it.
- **PNG goldens** — wait on an aarch64/NEON measurement (§15 D304); per-`vello_cpu` too (D414).
  `num_threads` is still a default.
- **Layers panel `MIN_W = 210`** rests on one reading, not two (§15 D444): re-tighten or amend its prose.
- **Chrome hairline base** in `rulers::snap_across_axis` (guides) is unruled (§15 D479). The pixel
  grid no longer calls it: it places lines from its lattice, which
  `the_lattice_picks_the_pixels_the_snapped_lines_did` holds to the pixels `snap_across_axis` picks,
  so ruling the base means moving the lattice with it.
- **The Type panel's four `%` caps** are 200, inherited, not chosen (§15 D861).
- **Dashboard virtualisation** — deferred (§15 D862); not a one-line `show_rows` (rename field, ⋮ menu,
  `follow_selection` depend on drawn cards).
- **Components: five unruled questions** — a group instance's canvas label; a main-scoped guide not
  copied; a linked node moved into a nested instance losing its link; instance roots exempt from
  membership (§15 D978); a moved-out layer previewed on copies (§15 D979 (xiv)).
- **`Ctrl+Alt` is `AltGr`** on many European layouts (`shortcuts.md` §12, §15 D987).
- **One font-availability question for panel and snapshot** — only worth it if the snapshot ever
  reads a live `FontService`.
