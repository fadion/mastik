# Mastik — Roadmap

Open work only. **Now** is in the order it is taken up; **Later** waits on a stated trigger;
**Considerations** are ideas not yet decided. Decided non-goals are in `non-goals.md`; built
work is described in `architecture.md`. Refs are file + function name, never line numbers. Read
the code before picking an item up — an item's premise can go stale while its ask still reads
true.

## Now · Prototyping

A release of its own. The prototype is built (`prototyping.md` points to each part); what
remains is below.

- **A lighter scripts player.** A file that runs a script carries about 2.6 MB more, gzipped,
  mostly the verb layer a script reaches the document through (`Host::verb` → `verbs::call`
  by name). Scripts writing typed operations instead would let the page's build drop it
  (`design/prototyping-export.md` *The browser build*).
- **Lists from data.** A repeated row filled from a data file, on the chart's data model; its
  saved form is undecided (`prototyping.md`, *Still open*).
- **Device and phone mirroring.** The app serves the prototype player to the local network and
  shows a QR code; a phone on the same network plays the page and follows edits. Not hosting: it
  ends with the app, needs no account and never leaves the network.
- **Annotations in inspect mode.** The exported player's inspect panel does not show a picked
  layer's annotation (`Node::note`), which the packed document already carries.
- **A hover leave trigger.** *While hovering* puts back what its actions did when the pointer
  leaves, but nothing can start on leaving: closing a menu after a delay, a fade out.
- **Touch gestures as triggers.** Long press, double tap and a swipe with a direction. *Drag*
  fires on any direction past four units, so a swipe left and a swipe right cannot do different
  things.
- **A scroll position trigger.** Actions that run when a scrolling frame's offset crosses a
  value, in either direction: a header that shrinks, a button shown past a point. With it, a
  layer property driven by the offset across a range (parallax, a fading header).
- **Set a variable mode.** An action that switches the mode a collection reads, for the whole
  prototype or one frame: dark mode or another language mid-play. Today only a flow's starting
  values and *Set variable* change what variables read.
- **Keep scroll on navigate.** *Navigate* resets every scrolling frame inside its destination
  (`play.md`, *Actions*); an option on the action to keep the offsets, so a tab bar returning to
  a long list lands where it left.
- **A layer that follows the drag.** *Drag* is a trigger only: the layer under it does not move.
  Moving it, held to an axis and a range, released to a snap point or back, with actions on
  release, gives swipe to dismiss, a slider's thumb and a sheet pulled down.
- **Scroll snapping.** A scrolling frame that rests on its children's edges (or every page of its
  own size) after a drag or a wheel, so a carousel stops on a card.
- **Smart animate's other properties.** It interpolates transform, box size, opacity and solid
  fills (`play.md`, *Motion*); corner radii, strokes, gradients, shadows and blur jump. A card
  growing into a screen needs its radius and shadow to move.
- **Responsive playback.** *Fit*, *Fill width* and *Actual size* all scale the screen as drawn. A
  fourth scaling lays the frame out again at the player's width through its flex, grid and width
  queries, so a resized window or a phone of another size shows the real layout.
- **Landscape.** A page's device preset has one orientation; a setting to turn it, and a button
  in the player's bar to turn it mid-play.

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
- **`Ctrl+Alt` is `AltGr`** on many European layouts (`shortcuts.md` §12, §15 D987).
- **One font-availability question for panel and snapshot** — only worth it if the snapshot ever
  reads a live `FontService`.
