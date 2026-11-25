# Mastik — Roadmap

Open work only. **Now** is in the order it is taken up; **Next** is decided and follows it;
**Later** waits on a stated trigger;
**Considerations** are ideas not yet decided. Decided non-goals are in `non-goals.md`; built
work is described in `architecture.md`. Refs are file + function name, never line numbers. Read
the code before picking an item up — an item's premise can go stale while its ask still reads
true.

## Now · Chart improvements

Charts that serve more than the default look: a background the chart follows, the card rows the
model hides or lacks, emphasis and annotation, house styles, and three kinds dashboards ask for.
In order: *Background and derived theme* is the model the later items lean on. Two rules hold for
all of it. **One save-format version bump and one MCP snapshot bump** cover the batch: new
`ChartKind` and `PartRole` values are plain serde enums an older reader cannot parse, and
`chart::Options` does not deny unknown fields, so an older reader would drop new options on
save. **Every new colour joins `Chart::brushes` and `brushes_mut`**, so it binds, detaches and
pastes with the rest.

- **Card rows.** Settings the model has or nearly has, each in the section that owns its subject.
  *Padding* and *Gap* (`Options::padding`, `gap`, today MCP-only) as Style's last row; no canvas
  handles, since padding is one value. *Text by role*: tick labels, values, legend and axis titles
  each a size (as now), a weight and a colour, unset meaning the chart's; Sankey node labels stay
  on the legend role, leader lines on the text ink. *Mark stroke* (`Solid` or `Outline`, width,
  colour whose unset chip is *Background* or *Series*) replaces `Options::outline`; Soft gains a
  background-coloured separator. Axis line and gridline widths as their own half-width fields under
  their colour rows (a colour row is one `field_row_pressable`). Legend swatch shape (Square,
  Circle, Line) and a *Label line ends* switch for lines and areas, a switch rather than a legend
  position because every preset writes `legend.position` (`Preset::apply`); line-end labels relax
  apart as Sankey nodes do. *Order* (Table order, Largest first, Smallest first) for columns and
  bars, by the first series shown or a stack's total, replacing the pie's *Sort by value*;
  `chart_data::cell_of` maps through it. *Totals* on stacked columns and bars (`PartRole::Total`),
  owned by Bold. Presets own the stroke, totals and the value role's weight, nothing else new.
- **Background and derived theme.** The background is the chart frame's top solid fill; the Chart
  card's *Background* row (Solid and Variables only; disabled, naming why, over a gradient or a
  fill style) edits it. *Theme* becomes Auto, Light or Dark (`Options::theme`, `dark: true` read
  as Dark), Auto for new charts. The text ink, axis and gridline colours, a series on the palette
  and a named mark on it become `Option`: unset, `chart::generate` derives them from a ground it is
  passed and the theme, Auto picking the theme by WCAG contrast and tinting grid (12%) and axes
  (30%) toward the ground. Nothing is rewritten at commit, so `with_dark` retires (its exact-match
  swap could not follow modes, width picks, the page ground or a moved parent); `with_palette` puts
  colours back to unset; the loader maps colours still at their theme's start value to unset. A
  transparent chart reads the nearest painted ancestor, else the page (`chart::ground_of`); settle
  regenerates when a ground stamp kept beside `generated` differs, and it joins `chart::stale`. The
  frame's own fill passes its binding through; an ancestor's passes as its literal colour, so the
  chart frame stays the only user of its variables. Hollow points and an *Inside ink: Auto* choice
  (text ink or ground, per mark) read the ground. A plot background (`Options::plot`, fill and
  radius) is the plot's `PartRole::Ground` rect, the progress ring's track moving to `Track`. A
  chart drawn inside a solid-filled frame starts with no fill.
- **Highlight, negatives and references.** A highlight names categories or series by name (a
  grouped scatter's groups have no column), others *Muted* (an unset colour derived from the ground)
  or *Faded*, with a *Highlighted only* values switch; doors are the context menu (*Highlight
  ‹name›*, *Clear highlight*, through `query::chart_part_at`) and a Series dropdown. Per chart,
  not per data set. *Colour by category* for one-series columns and bars, through `named_marks` as
  pies do. *Colour below zero*: one chart-wide colour for bars and for line and area runs split at
  their zero crossings. *References*: up to 16 lines or bands per chart on the value or category
  axis, at a typed value, a category, or a statistic (average, median, min, max) over the series
  shown or one; label with `{value}`, colour, width, dash, label place, *Take into the range*.
  Bands under the marks, lines over them. Their own section after Values, headed by a
  `ui::fold_rule` variant with a trailing `+`, rows opening a popover. Not on pies, Sankeys or
  radars.
- **Lollipops and waterfall.** Lollipops are `Bars.shape`: stem, head size, `PointShape`, filled or
  hollow, and *Connect series* for a dumbbell. Grouped only (`Chart::validate` refuses lollipops
  with stacking); not preset-owned. *Waterfall* is `ChartKind::Waterfall` (Column or Bar): a
  category and one value column of changes walked from zero, totals picked by category name plus
  an optional final total, signed value labels, dashed or solid connectors, and *Increase*,
  *Decrease*, *Total* colours defaulting to blue, orange and slate (green and red fail
  deuteranopia). A total row's own cell is ignored, and the card says when it disagrees with the
  walk. New glyph `CHART_WATERFALL`; palette aliases *waterfall*, *bridge*, *lollipop*,
  *dumbbell*.
- **Radar.** `ChartKind::Radar` (Lines or Filled), reading what a line chart reads, in a square
  centred in the plot as a pie's is. Polygon or circle rings from the value axis's ticks, spokes as
  category gridlines, a start angle, the Lines options; straight sides only; gaps always joined; 3
  to 36 categories. Labels anchored by angle, the square shrinking to fit them. New glyph
  `CHART_RADAR`. Landing it drops radar from the non-goal *Chart kinds past the seven*.
- **Fade and chart styles.** *Fade* (`Options::fade`): areas and bars from their colour to a
  share of it at the baseline, a generated linear gradient; a bound series draws flat until gradient
  stops can bind. *Chart styles*: a sixth style slot (`StyleValue::Chart`, `StyleRefs::chart`,
  `SetChartStyle`) under the slots' rules, carrying the look (options, palette with a stored
  `Options::palette_group`, theme choice, text, stroke, fade, totals) and never the kind, stacking,
  order, shape, data, series colours, highlight, references, axis titles or typed ranges. Styled,
  the card's look sections collapse into the styled capsule; *Flatten* unbinds the style; kits carry
  them. Needs *Background and derived theme* first. Every five-slot enumeration grows a sixth.
  Landing it removes the non-goal *Saved user presets*.

## Next · Editing parity

Mastik as a standalone tool: the effects, paints, media and path tools that would otherwise send
someone to another app or a plugin. Not in order. Two items share one prerequisite: the model's
`Brush` (`mastik-core` `image.rs`) is an alias of `peniko::Brush`, a closed enum of solid,
gradient and image, so a pattern fill and a mesh gradient each need Mastik's own paint enum in its
place, saved in the same shape so existing files load. It is matched in about 60 files.

- **Progressive blur.** A layer or background blur whose radius ramps across the layer along a
  gradient. Effects are our own passes, so nothing upstream blocks it: a new `EffectKind` (or a
  ramp on the two blurs), and a variable-radius pass in both backends, `effects::blur` and
  `fx.wgsl` (a blur pyramid blended by the ramp), reusing `scene::paint_backdrop` for the
  background form. SVG has no variable blur: stack blurred copies under gradient masks, or leave
  it out and report it as the background blur is. Medium.
- **Glass.** A background blur plus refraction at the edges, a specular rim and optional
  dispersion. The backdrop repaint and its damage (`scene::spread_through_backdrops`) exist; new is
  a displacement pass over the backdrop driven by distance to the outline, on both backends. The rim
  reuses inner shadow and stroke. SVG, PDF and the Penpot and Sketch writers leave it out and report
  it, as the background blur; CSS gets the blur and the rim only. Large.
- **Texture.** Grain that breaks a layer's edges, not only its fill (the *Noise* effect,
  `mastik-core` `noise.rs`, stays inside the alpha). A displacement of the layer's own drawing by
  a noise field, a stage-1 pass beside *Noise*. SVG expresses it (`feTurbulence` into
  `feDisplacementMap`, the graph `svg.rs` `noise_graph` already starts). Medium.
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
- **Separate start and end caps.** `kurbo::Stroke` and the outline engine (`build::stroke_polygons`)
  already take two caps; the renderer sets both from `Stroke.cap`. Missing are the model field
  and the SVG boundary, which has one `stroke-linecap`: a stroke with two caps exports as outlined
  ink, as a tight open stroke does (`tight_stroke::union_outline`). Small.
- **Variable width.** A width profile along the path, edited with handles on the path (pen
  pressure is a non-goal). No backend draws a varying width, so a profiled stroke is outlined to a fill:
  `build::stroke_ink` already lays one line at several widths (per-side strokes), and the profile
  replaces its constant-width step with an offset whose half-width follows arc length, written by
  us since i_overlay strokes at one width. An optional profile on `Stroke`, `width` kept as its
  maximum so `geometry::stroke_expansion` stays conservative; every reader (bounds, hit test,
  booleans, render, export) then takes the one cached ink path. Lost: an editable width in SVG,
  CSS borders, and round trips through Penpot and Sketch, which have no profiles. Medium to large.
- **Vector brushes.** Art and scatter brushes: another shape bent or repeated along the path.
  Nothing exists; the pieces do (kurbo arc length, `mastik-core` `warp.rs` and `distort.rs`). An
  *Apply brush* that bakes the result into paths comes first; a live brush follows, rebuilt when
  its source changes, on top of variable width's ink path. Large.
- **Curvature combs.** A switch, with a chord, that draws curvature teeth along the normals of the
  path whose points are being edited, joined at their tips, scaled to the zoom with an adjustable
  or fitted scale so a tight corner stays on screen. Canvas chrome only, never in the document;
  shows where curvature jumps at a smooth point. Small to medium.
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

## Next · MCP

- **`get_prototype` once per interaction, with its settings.** It lists an interaction once per
  list copy (a list of ten rows repeats each of its row's interactions ten times) and repeats
  every interaction for every flow that reaches its frame (`reads::prototype` over
  `flow_map::plans`, each plan's `written`). It also leaves out what an agent walking its flows
  needs: overlay settings (position, backdrop, close outside), a flow's start values and modes,
  input bindings, and tied variant states. Collapse list copies to one line *on each row of
  ‹list›*, list each frame's interactions once with the flows naming their frames, and add those
  settings to the words.
- **A list sorted by a date column.** Every cell is text, so a sort on *Due* puts *Dec 12*
  before *Dec 2*; a sort that reads dates (or a date column kind) would let a task list sort by
  due date.
- **`get_tree` with a depth, and list copies collapsed.** `get_tree` takes only `id`, and a
  frame holding lists answers every copy expanded, tens of KB. A `depth`, and list copies shown
  as their template plus a count, would keep a read small.
- **A visibility bound to a boolean's negation.** A boolean binds visibility only as it is, so
  an off/on pair (a switch's two looks) needs a second variable kept in step by every toggle. A
  negated binding, or a boolean choosing between two values, would drop it.

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
