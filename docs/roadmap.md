# Mastik — Roadmap

Open work only. **Now** is in the order it is taken up; **Later** waits on a stated trigger;
**Considerations** are ideas not yet decided. Decided non-goals are in `non-goals.md`; built
work is described in `architecture.md`. Refs are file + function name, never line numbers. Read
the code before picking an item up — an item's premise can go stale while its ask still reads
true.

## Now · Prototyping

A release of its own. The prototype is built (`prototyping.md` points to each part); what
remains is below.

- **One export dialog.** *Export prototype…* and *Export flow map…* are reached only from the
  palette, and a take only from the player bar's *Export the take as a video*. One card with an
  *HTML file* / *Video* / *Flow map* switch at its head, reached from the palette, the player
  bar and a flow chip's menu:
  - HTML: the size meter split into Player, Fonts, Images and Document (today one total from
    `Parts::estimate`); warnings before writing (an image far larger than it is drawn, with
    *Downscale to 2×*); the export held on a missing target or an expression that does not
    parse; the file named *document — flow*.
  - Video: the takes of this run listed (only the last is kept, `Player::take`), *Format*,
    *Size*, *Show clicks* and *Device frame* (`video.rs` `Options` has format, fps and scale),
    and a preview.
  - Flow map: which flows go in (`flow_map::plans` takes every flow).
- **The player's flow menu.** Changing flow while playing means stopping and clicking a flow
  chip, and the selected frame is played only when the document has no flow at all
  (`MastikApp::play`). A menu on the bar listing the page's flows and *From this frame*; the
  state card showing the frame shown and how many steps *Back* goes.
- **Polish.** Smaller gaps between the prototyping design and what is built:
  - The player bar: elapsed time while recording; the bar hiding in fullscreen after 2 s still
    and showing near the bottom edge; one edge pulse when a live edit lands.
  - The exported page: a fullscreen button on its bar, a loading bar, and a corner note that
    fades, *Drawn without the GPU; motion may be slower*, under the CPU fallback.
  - Connectors: *Back* links drawn above the frames; a frame's *After delay* leaving from its
    name strip; the first flow's chip and Flows row in the accent.
  - The interactions card: *Copy interaction* in the `⋯` menu; a selected interaction block,
    ringed, whose off-screen target the canvas scrolls to; a variable in an action row as a
    `binding::capsule`; `@` in an expression opening the variable list; *Copy from library…*
    under *Run script*; a main's interaction states saying Hover and Pressed play live.
  - Triggers and drag: a notice when a scroll trigger's frame is gone (`Notice` has none); hover
    triggers kept from firing on a touch screen; a scroll effect naming the frame it follows
    (`ScrollEffect` always takes the nearest); the drawn snap point (`DRAWN_POINT`) renamable.
- **Charts reading a data set.** A chart could read one of the document's data sets
  (`design/lists.md`) instead of its own table, so a chart and a list over one file update
  together.

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
