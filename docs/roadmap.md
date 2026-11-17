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

- **Inspect mode in the exported player.** A switch in the HTML export's player that stops
  interactions and lets the viewer pick layers: a layer's CSS as *Copy as CSS* writes it, distances
  to its neighbours as the canvas's `Alt` measure quotes them (`measure.rs`), and its export
  settings' files to download. It gives a developer a handoff with no server, from the file the
  designer already sends.
- **Annotations.** Notes pinned to a layer that say what the drawing cannot ("this list scrolls",
  "48 px touch target"), shown on the canvas, in inspect mode and atop the layer's code in code
  export. They are specification, not conversation: no replies, no authors, no resolved state,
  which keeps them apart from the comment tool `non-goals.md` declines.

## Now · Misc

Items run cheapest first.

- **More warp presets.** Wave, Rise, Squeeze, Fisheye and Twist beside Arc, Flag and Bulge
  (`warp::Preset`): each is one map function into the existing refit, and the result stays baked
  paths.
- **Linear and grid repeat.** `Line` and `Grid` beside the ring in *Repeat around origin*
  (`mastik_core::repeat`, `Ring`, `Place`), reusing its group and linked-copy placement:
  step-and-repeat, with copies optionally linked as instances.
- **Honour an imported picture's colour profile.** `mastik_render::images::decode` reads no ICC
  profile, so a Display P3 phone photo or an Adobe RGB JPEG draws as if its numbers were sRGB,
  desaturated on the canvas and in every export. Convert to sRGB at decode with the profile `image`
  exposes and a pure-Rust colour engine (`moxcms`); the stored bytes stay the original. Check it
  against a real P3 photo first.
- **The assets browser through MCP.** `search_assets` (icons or photos, several queries at once,
  set and variant optional; icons by name, aliases and licence, photos with alt text, credit and a
  small thumbnail as image content) and `place_asset` (at a point, a size, or filling layers),
  through the panel's own placement so the credit, Unsplash's download report and the undo names
  come with it. Only sets and services the person has switched on; an agent never switches one on,
  since that reaches the network. Searching is a read (Look); placing needs Propose or Edit. A photo
  lands after its download, inside the call's 30 s queue limit or as a pending id. Photo searches
  are cached briefly, since an hourly limit is the person's. Library kits shown in the Icons tab
  (`sets::icon_packs`) are included.
- **Arrowheads on lines.** A start and an end decoration on `Line`, `Arc` and open `Path` (none,
  arrow, open arrow, circle, square, bar), sized from the stroke width and drawn as their own
  shapes, not as stroke ink, so a `Stroke` stays ink at one width. Hit-testing, `ink_bounds` and
  *Outline stroke* include them. SVG writes `<marker>`; code export writes the SVG fallback. The
  Figma, Sketch and Penpot readers and writers map their own arrow kinds.
- **OKLCH.** An entry and display mode in the colour picker beside Hex, RGB, HSL and HSB, with the
  plane drawn in OKLCH and the sRGB gamut's edge shown on it. Storage stays sRGB. An out-of-gamut
  pick is mapped by lowering chroma (CSS Color 4's method), not by clipping channels; the picker
  keeps the person's L, C and H while it is open, unsaved, so a hue drag near the edge does not
  drift chroma. With it, *Generate scale*: a 50–950 ramp of colour variables from one colour, even
  in lightness, from the variables browser, MCP and Luau. `contrast::nearest` already moves colours
  in OKLCH.
- **Blend shapes along a spine.** *Blend shapes* places its steps on a straight line
  (`mastik_core::blend::blend`); map them onto a chosen path instead, optionally turned to its
  tangent.
- **N-fold radial symmetry for the pen and pencil.** `symmetry::PathMirror` holds a vertical and a
  horizontal axis; a rotational order (3–12 copies about the centre, optionally mirrored) needs a
  model change and its own rule for what the path stores, as the mirror's flags have.
- **Design system health.** One card for a document: how much paint, type and spacing is bound to
  variables and styles against raw values, detached instances, unused styles, variables and
  components, near-duplicate colours, and kits waiting on updates. Most of the readings exist
  (colour review, clean up, `kits_waiting`); the card and the coverage count do not. Its checks are
  `mastik_export::review`'s, the ones MCP's `review` and `mastik check review` run.
- **Noise and grain.** A noise fill, and a grain effect over a layer. `non-goals.md` declined noise
  as one generator written three times to agree to the pixel; SVG's `feTurbulence` is specified to
  the algorithm, so porting that one algorithm to WGSL and the CPU backend makes the SVG writer the
  reference rather than a third copy.
- **Device and phone mirroring.** The app serves the prototype player to the local network and
  shows a QR code; a phone on the same network plays the page and follows edits. Not hosting: it
  ends with the app, needs no account and never leaves the network. Waits on the player
  (*Now · Prototyping*).

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
- **Reading a sharpen back from SVG.** The export writes a sharpen as `feGaussianBlur` plus an
  arithmetic `feComposite` (k2 = 1 + a, k3 = −a), and `svg_in` does not recognise the pair, so a
  Mastik SVG re-imported loses it. Worth it only if that round trip matters; matching the pattern
  risks misreading other tools' filters, so it would key on the exact coefficients.
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
