# Image fills and image editing

The inspector's image fill row, the image-editing card (Settings, Adjustments and Curves tabs), image editing
mode (`Tool::ImageEdit`, the crop by handles and pan), and pictures arriving by drop or paste.
Part of §9.4.

## 9.4 The image fill row

- `panels::paint::PaintKind::Image` exists but is no picker kind (neither *Solid* nor in
  `PaintKind::GRADIENTS`; an image comes from a file dialog, not a colour wheel). `picker_body` returns
  early for an image.
- One line. The chip draws the thumbnail (`ui::Swatch::Picture`) over the checkerboard **at the brush's
  `ImageSampler::alpha`** — carried on the variant, a uniform white×alpha tint, because `thumbs` shares
  one texture per picture across layers. Grey ramp while a thumbnail is missing or deferred. The hex
  field is a plain "Image" label.
- **Clicking the chip or the label enters image editing** (tooltip *Edit this picture — crop it, move
  it, scale it*), through `MastikApp::begin_image_edit`. The door is
  `inspector::row_edits_the_mode_s_picture`, which compares the row's slot with
  `edit::croppable_fill`'s answer, so a second image fill's row is inert. A locked layer (or locked
  group, via `query::is_effectively_locked`) closes the door, with a third tooltip state naming the
  lock. The picker still opens from a locked layer's colour rows.
- The Fill/Fit/Crop/Tile strip and popover buttons live in the image-editing card, not the row. Mode
  writes go through `write_slot` and `paint::with_fit` everywhere, keeping the crop across a switch.

## The image-editing card (`inspector::image_edit_card`)

- A `POPOVER_W` `Foreground` `Area` in `menu_frame`, at `popover_anchor` taken from the **paint row's**
  rect, in the shared lane (`picker_lane_right`, which asks `edited_image`).
- **It goes with the mode**: it appears with the mode and goes when `Enter`, `Escape` or its ✕ leaves
  it. There is no click-away rule, since a canvas click is often the next crop drag. `MastikApp` keeps
  only `image_tab: ImageCard` (the last tab, kept across entries); which layer and paint is
  `canvas::edited_image`'s answer.
- **The card keeps one height: the Settings tab's.** The Settings body's height is measured where it
  is drawn and remembered in the context (`image_body_h_id`; `IMAGE_BODY_FALLBACK_H` until it has been
  drawn once; a test holds the constant to the measured body, since a card opened on another tab
  keeps it). The other tabs' bodies stop one reset row short of it, so their reset rows sit where
  Settings' last row ends. Nothing grows the popup: an egui popup can shrink but never grow back.
- **The tab strip is the header**: a three-cell `ui::segmented` (Settings, Adjustments, Curves) with `ImageCard::title` labels, then
  `ui::TABS_CLOSE_GAP` and `ui::close_button`, as the colour picker's header reads; the ✕ leaves the
  mode (`choose_tool(Tool::Select)`), keeping the crop made. Controls are `CELL` = 26; both tracks (this and Settings' mode strip) ask for `TRACK_CELL`,
  four less, since `ui::segmented` pads two each side.

### Settings tab

- **Framing modes** Fill / Fit / Crop / Tile — an override: Crop is what handles and pan do; the others
  re-frame inside the box the crop left and hide the ghost. The crop rectangle survives.
- **Aspect** eyebrow under Mode: a `ComboBox` of `CropShape` (*Free*, *Original*, *1:1*, *4:3*,
  *3:2*, *16:9*) and a `ui::field_button` (`icon::ARROWS_CLOCKWISE`, the reverse glyph) that swaps a
  two-sided ratio to portrait, its tooltip naming the shape it swaps to; dimmed with a reason for
  Free, Original and 1:1. The shape is **card state, not the document's** (`MastikApp::crop_aspect`,
  a `CropAspect` keyed by the picture, so a new picture starts Free): a crop's shape is already its
  box. *Original* reads the oriented picture, so it follows a turn. Picking one
  (`MastikApp::shape_crop`) reshapes the window at once to the largest box of that shape inside the
  box and the picture (`edit::crop_shaped`), about the box's centre, the picture held still and the
  mode forced to Crop: one undo step, *Crop*, as a handle drag is. Re-picking the shape in force
  reshapes again.
- **Orient** eyebrow over four `ui::field_button_sized`s splitting the row, the Transform card's
  quarters row in its order and glyphs: turn counter-clockwise (`icon::ROTATE_LEFT`), turn clockwise
  (`icon::ROTATE_RIGHT`), mirror left to right (`icon::FLIP_HORIZONTAL`), mirror top to bottom
  (`icon::FLIP_VERTICAL`). Actions, never lit; each tooltip says it moves the picture in its frame,
  not the layer. `MastikApp::turn_picture` writes `OrientOp::Turn`, once clockwise and three times
  counter-clockwise (the model's one turn), which carries the crop. When the box showed the
  whole picture at its own shape (`ImageRef::frames_whole_picture`: Fill or Fit, or Crop uncut, at
  the oriented aspect within 1%) the box turns too, width and height swapped about its centre
  (`edit::reshape_box`); any other box is a framing someone chose and stays. One undo step, *Turn
  picture*.
- **Scale** (`icon::RESIZE`) and **Tile** (`icon::IMAGE_TILE`) half-width fields, percent of source
  pixels (`1.0` = one unit per source pixel); each read by one mode and **dimmed, not hidden**,
  otherwise.
  - Scale is the crop zoom (no gesture can scale the picture). Floor `ImageRef::cover_scale`; ceiling
    `floor.max(10_000.0)` — the one inspector bound from the document. Writes via `image::crop_scaled`
    (uniform about the window centre); readout seeded from `cover_crop` outside Crop. Scrubs in
    hundredths (multiplicative); Tile in whole units.
- Then five actions in one run, the readout they act on below them.
- *Reset crop* puts the picture back as it was placed (`MastikApp::crop_reset_tx`): the framing
  fields at their defaults (mode, crop, tile scale, orientation; the adjustments are another tab's
  and stay), the card's Aspect back to Free, and the box set to the whole picture where it is drawn
  now, at its own shape, so nothing is cut or stretched and the picture does not move (under Tile,
  which draws no single picture, the box keeps its centre and width). One undo step, *Reset crop*.
  Dimmed with *The picture is shown whole, as it was placed*.
- Then *Replace…* (pick one file, embed it, **keep the framing** — the repair for a
  missing picture) and *Replace and reset* (framing fields to defaults, adjustments kept). Both decline
  for a selection slot and run after the `Area` closure (the dialog is modal); the card stays behind
  the dialog.
- Then the two actions that **rewrite a picture's bytes** — the only exceptions to "the original
  goes out", each its own undo step, run only from here, neither asking for confirmation (undo
  brings back every byte; the tooltips state the cost). Both dim with *The picture's pixels are
  missing — replace it first*. Each commits through `MastikApp::rewrite_tx`: the new entry is
  added, and the old one is removed in the same step once nothing names it (`io::unused_images`
  on the document as it would stand), so the file gets lighter at once.
  - *Reduce picture…* (`icon::CORNERS_IN`) opens a modal canvas card (`MastikApp::reduce_ui`, the
    Trace card's pattern): **Size** offered as multiples of the largest drawn size (1×, 2×, 3× as
    drawn, each only while smaller than the original, then *Original*, which only re-encodes;
    `pictures::reduce_sizes`; 2× by default, the original when 2× is not smaller), never
    upscaling; the drawn size is the largest scale along any axis, so a picture squeezed on one
    axis is not reduced below the size it is drawn on the other; **Format** JPEG or PNG (a picture
    with transparency offers PNG only); **Quality** (`icon::GAUGE`, 85) dimmed with its reason for
    PNG. *Now* and *After, about* sizes; the after figure is **measured**, encoded on a worker as
    the settings move, one encode at a time, and *Reduce* waits for it. It acts on the picture, not
    the layer: every layer and style showing it moves onto the smaller one (`edit::repoint_image`),
    since a reduce leaving the original for the others would not shrink the file. Crops,
    orientations and adjustments keep their meaning (the crop is in fractions); a tile scale, in
    units per source pixel, is multiplied by the old width over the new so each tile keeps its
    drawn size. Status *Reduced
    Hero photo — 4.8 MB to 0.6 MB*.
  - *Apply crop* (`icon::CROP`) keeps only the pixels inside the window, cut in the picture's own
    pixel space (`edit::crop_source_px`, through the orientation) so the mirrors, the turn and the
    adjustments stay live. The mode stays Crop, and the crop left is the part of the cut that was
    shown (`edit::crop_after_cut`): whole unless the cut was rounded outward to pixels, so the layer
    draws exactly what it drew, a box stretched against the cut included. This layer gets a new
    entry; others showing the original keep it. Decoded and encoded on a worker
    (`MastikApp::apply_crop`), landed by `poll_pictures` only on the document it was asked of; a
    second rewrite while one is on its worker is refused with *Still rewriting a picture — try
    again in a moment*. A JPEG is re-encoded at quality 92, anything else as PNG. Dimmed with *This picture has not
    been cropped* (the whole-crop sentinel), then *Only Crop keeps part of the picture — switch the
    mode to Crop first*. Status *Cropped Hero photo to 2400 × 1800 — 4.8 MB to 1.1 MB*. Linked-source entries (*Embed/Link*, *Relink…*) are not built — no linking is authored.
  A replacement drops the layer's photo credit (`credit::dropped_by_replace`, in `replace_image_with`),
  since the photo it credited is gone; an icon's credit stays.
- **Asset** eyebrow, last, over the **weight readout**, two fixed lines whatever they say
  (`pictures::Weight::lines`): format, pixels and stored size (*JPEG · 6000 × 4000 · 4.8 MB*), then
  how small it is drawn and on how many layers (*Drawn at 1/8 of its pixels · on 3 layers*, or
  *Drawn at 1:1 · on this layer only*). "Drawn at" is the **largest** drawn size across every layer
  showing the picture (`MastikApp::picture_weight`: world × framing, units per source pixel), so a
  picture drawn small in one place and full size in another is not flagged, and the zoom plays no
  part. The second line turns `color::warn()` under a quarter of its pixels across
  (`pictures::HEAVY_BELOW`). A picture whose bytes are not here reads *Missing · 2400 × 1600 when
  saved* (*Missing* with no entry at all).
- Actions go through `inspector::menu_action`, which shadows a dimmed row with an explicit
  `Sense::empty()` claimant so its tooltip still shows.
- Not here: *Original size* (resizes the node; it is `context-menus.md` §5.7's row), *Export
  original…* (removed; exporting is the Export panel), *Edit crop…*.

### Adjustments tab

- **Three folds**, one per `AdjustSection`, each headed by `ui::fold_rule` as the Chart card's are:
  **Light** (Exposure, Contrast, Highlights, Shadows, White point, Black point), **Colour** (Saturation,
  Temperature, Tint, Hue) and **Looks** (Fade, Greyscale, Invert, Duotone with its two ends). The heads
  have no tooltip. Each head carries a count (`AdjustSection::moved`, drawn as a card header's
  overrides): the amounts in that fold away from rest, so a folded section is never silently
  adjusted.
- **Folds are remembered per section** in `collapsed_panels` (`inspector::image_fold_key`) and open
  independently. A new session opens Light and folds Colour and Looks (`IMAGE_FOLDED`).
- **The folds scroll** in a body of fixed height between the tab strip and *Reset adjustments*, with no
  bar and `ui::scroll_fade_top` once scrolled; the strip and the reset stay put.
- One row per amount, in `Adjustment::ALL`'s order, at `ui::CARD_ROW_GAP`. Each is a
  `ui::value_field_metered` with the **word** as prefix, scrubbing `Adjustment::shown_scale` (percent,
  hue in degrees) over the model's `Adjustment::range`, written through `valve_slot` (one undo step
  per scrub).
- The meter (`ui::Meter`) is an accent wash across the field's ink. A two-sided amount is
  `Meter::Signed`, **growing from the middle** (negative left, positive right, ±1 reaching an edge)
  over a 1pt centre tick (`ui::METER_TICK_W`, from `ui::meter_wash()`) that is always drawn. A
  `0..=1` amount (white point, black point, fade, greyscale, invert, duotone) is `Meter::Amount`, **filling from the left edge
  across the whole field**, as an opacity reads, with no tick: no half of the field is dead. The number is right-aligned: signed
  via `inspector::signed_number` for a two-sided amount (hue with a `°` suffix), unsigned for a
  `0..=1` one.
- Tooltips are `Adjustment::hint`: White point *Turn the lightest tones white*, Black point *Turn the
  darkest tones black*, Fade *Lift black towards grey, for a muted print*. The undo step is the
  image fill's, as every slider's.
- A per-row reset is **dimmed, not absent**, at zero; *Reset adjustments* is a `menu_action` that puts
  every amount back, the duotone colours to their defaults and the curve to a straight line.
- **Duotone's ends** sit under its row whenever Looks is open (picking one before raising the amount
  is allowed): two half-width `slot_color_row`s (chip and hex, no opacity, its `alpha` switch off) on
  `PaintSlot::Duotone(fill_index, DuotoneEnd)`. That slot's read and write go through the image
  fill's own `PaintSlot::Fill` slot. Its picker closes when the card closes, leaves the Adjustments
  tab or folds Looks.

### Curves tab (`panels::image_curves`)

- **One master curve for R, G and B**, since the pipeline holds one table. The graph is the body's
  full width and as tall as the body allows after the input strip and the In/Out row, up to square; its ground is a
  `color::field()` rounded rect.
- Behind the curve: the picture's **luminance histogram** (`mastik_render::luminance_histogram`,
  square-root heights in faint ink, cached per picture in the context), quarter lines in
  `color::divider()`, and the straight line dashed.
- **The ends are the tone rows.** The black end's square sits at (black point, fade) and the white
  end's at (1 − white point, 1). Dragging the black end writes black point and fade, dragging the
  white end writes white point: the same numbers the Adjustments rows show. An end stops `MIN_GAP`
  short of the other.
- **The input strip** sits under the graph: a black-to-white ramp spanning the graph's input axis,
  corners masked as the gradient stop bar's are, and under it a triangle pointing up at each end's
  input, filled and edged as that end's square (the accent while held). A triangle slides its end
  across only, so the black end keeps its fade; the cursor is `ResizeHorizontal`, as on a gradient
  stop. A press takes the end nearest across within `HIT_R`; a click or a drag takes that end, so In
  and Out follow it. Each triangle's tooltip is its row's hint. The strip adds to the body's height,
  so the graph stays square when the body has room.
- **Inner points** (round): a click on the graph adds one where the curve already is, so nothing
  changes until it is moved (up to eight; a click on an end's side or beside a point adds none). A
  press that drags adds and moves in one step. A drag moves a point between its neighbours and
  between fade and white; dragged more than `OFF_GRAPH` past the graph's edge it goes, as does a
  double-click (`ui::double_clicked`). Points are stored between the ends (`ImageAdjust::curve`),
  so moving an end carries them.
- Tooltip *Click to add a point, drag it off to remove it*; the cursor is a grab hand over a handle
  and while dragging. The held handle is drawn in `color::accent_400()`.
- **In and Out** are half-width `ui::value_field`s for the held handle, in percent, written through
  `valve_slot`, so arrows step it and a typed value lands exactly. With nothing held they read `–`,
  dimmed with *Pick a point on the curve to set it*; the white end's Out is dimmed with *The white
  end always maps to white*.
- *Reset curve* is a `menu_action` at the reset row's place: black point, white point, fade and the
  points back to rest, dimmed with *The curve is already a straight line*.
- Every write is the image fill's own undo step, as the sliders' are; a drag is one step.

## Image editing mode (`Tool::ImageEdit`)

**Outside the mode, resizing a picture resizes it; inside it, the handles crop.** The layer's box is a
window onto its picture. There is no crop tool, rail button or `C` key.

- A `Tool` variant because routing, cursor, `chrome_hidden`, the Escape ladder and `Enter`'s toggle are
  keyed off `self.tool`.
- **Subject is derived**: `canvas::edited_image` — the tool plus a single selection whose first visible
  image fill (`edit::croppable_fill` → `Paint::image_fill`). Not stored.
- **Four doors**, all calling `MastikApp::begin_image_edit(id)` (selects, takes the tool, prints the
  status line): double-click on a picture (after the group step), `Enter` on a selected one, *Edit
  image* at the head of `context-menus.md` §5.7, and a click on a fill row's chip or label.
- **Lock**: `begin_image_edit` refuses a locked node (*"This layer is locked — unlock it to edit the
  picture"*), and `edited_image` carries the same guard (the panel can lock the row or move the
  selection without re-entering the tool). The menu row dims because the mode refuses. Text
  (`canvas::begin_edit_text`) and points refuse a locked layer the same way.

### The crop gestures

- A **handle drag** moves one edge with the picture nailed down in world space; a **drag on the
  picture** slides it under a fixed box. Neither scales the photograph. Side handles are live.
- `ImageRef::picture_box`: the whole photograph's rectangle from the transform the backends paint with.
  `edit::crop_resized`: moves the grabbed edge, bounded by that rectangle, and **holds the handle on its
  own side** (no mirroring). `image::crop_reframed`: re-reads the old mapping at the new box's corners
  so the picture stays still. Crop and frame stay in proportion — no stretch.
- `crop_resize_tx` aims `edit::resize_layer` at `crop_resized`'s corner and emits **one transaction**
  with the geometry and the `SetFills` together. The mode is forced to `Crop`. A crop is a resize, so
  the lock reaches it.
- **A held shape** (`canvas::crop_held_shape`): while the card's Aspect is not Free every handle holds
  it (`edit::crop_resized_held`) — a corner holds the far corner and follows the pointer's larger
  reach, a side moves its edge and grows the other axis about the middle, the picture's edge winning
  over the pointer. A picked shape holds whether Shift is down or not, since it is the shape the crop
  must end in; on Free, Shift holds the box's shape at the press, as Shift does on a resize. A side
  drag's box has no held edge, so it is written whole by `edit::reshape_box`.
- Both drags capture start values at the press and re-derive each frame (`session.set_preview`). The
  **commit is gated on travel** (`grab_world`, `gesture_travelled`, §9.3); an untravelled release
  clears the preview (`clear_gesture_preview`). The preview is ungated.

### Chrome

- **The offcut is real ink**: `canvas::image_ghost` builds a `RenderOverrides` **chrome ghost** — a
  `Rect` at `picture_box` filled with the uncropped picture (orientation and adjustments included) at
  `GHOST_ALPHA` × layer opacity, at the layer's index so it draws under it. Only in `Crop`.
- The chrome ghost lives in its own `RenderOverrides` field so `is_empty` (`has_gesture_preview`) stays
  false while the mode is merely open. `ghosts_of` chains it. Re-installed every frame at the top of
  `draw_canvas` (survives commits; `set_preview` replaces overrides wholesale).
- `chrome_hidden` removes the selection box; `canvas::draw_crop_box` (called from `draw_canvas`, shapes
  from the free `canvas::crop_chrome`) draws the window **dashed** — 3pt at 6/6 (`CROP_DASH_PX`,
  `CROP_GAP_PX`, `CROP_STROKE_PX`) — with hollow corner handles, plus the picture's solid outline when
  the ghost is live, both via `shown_crop`. Cursor: the open hand (`Bitmap::Pan`) over the
  picture, the closed one while panning it, `Bitmap::Resize` at the handle's angle on handles and
  for the whole of a handle drag, wherever the pointer goes.
  While a handle is held, thirds hairlines cross the window (`canvas::crop_thirds`, in the picture
  outline's `SELECT_DIM`); a pan does not show them.
- **Done pill** (`canvas::crop_exit_pill`) in the size badge's slot, same ground, ink, radius, gap and
  `badge_placement`. A `Foreground` `Area`; `wheel_input`'s exemption names its layer. While the card
  holds a shape the pill names it (*Done · 4:3*), so the shape shows where the drag happens.

## Pictures by drop and paste

- `take_dropped_images` runs every frame from `update`, whatever the tool, **unless
  `MastikApp::modal_is_up`**, reading `raw.dropped_files` (browser drags as bytes, file-manager drops
  as paths). It and *Place image* (`place_image_action`) hand the files to `crate::import` as
  `Incoming` (name, bytes or path); neither reads a file on the frame. *Replace image*
  (`replace_image_action`, `start_replace_image`), a clipboard picture (`paste_image`,
  `start_paste_image`) and pasted SVG markup (`paste_svg`, `start_paste_svg`) go through the same
  worker, each as its own `import::Door`: the frame only runs the dialog or reads the clipboard.
  A lone dropped `.json` is not a picture: it opens the design-tokens review instead
  (`styles-variables.md`, *Design tokens*), read on the frame since a tokens file is small.
- **An import is read off the frame** (`start_reading`, a `crate::job::Job`), so a large PDF, SVG
  or picture never freezes the window. The worker reads the bytes, encodes a clipboard picture to PNG
  (`app::rgba_to_png`), decodes pictures (`app::read_picture`) and builds SVG and PDF layers
  (`canvas::read_svg`, `canvas::read_pdf`) under a stand-in parent the landing replaces, minting ids
  from an `IdSource` of its own (`session::random_actor`); the session's `IdSource` stays the
  frame's. It registers the document's faces (`Resolved::text_fonts`) first, since a worker's text
  engine holds only Inter and an SVG's text naming a family the document uses would read as
  substituted. Meanwhile the status says *Importing logo.svg…*, *Importing 3 files…*, *Pasting image…*
  or *Pasting SVG…*.
- **`poll_imports` lands it** (editor frame, beside `poll_simplify`) in the order asked: one still
  reading holds every later one for its tab, so a quick second paste never lands under a slow first.
  It is held while a gesture is live (`gesture_live`: a drag, a pen run or a pencil stroke, whose end
  would write back a document from before the import or lose its stroke to the landing's tool
  change), and while a modal is up, which may be asking about the document as it stands. It lands only in the tab in
  front when it started (`Tabs::active`), and only while that tab is in front: a tab behind is never
  written to (`library.md`). An import whose tab has closed is dropped without a word, as is one
  started on the scratch session no tab shows, once a tab is active. `land_import` adopts the decoded pixels into the decode cache, then places as the door does,
  against the document at landing: a drop's pictures through `drop_images_at` at the drop's world
  point, *Place image*'s by arming the Image tool; SVG and PDF through `place_svg` / `place_pdf`
  (`place_vectors`). A pasted picture lands through `land_pasted_image`, a pasted SVG through
  `place_svg` (*Pasted SVG — 5 layers*), and markup that sniffed as SVG but will not read as a text
  layer (`paste_text`), the paste's last arm; *Replace image* through `replace_image_with`, only
  while its slot still holds the brush it held when asked (`Door::Replace::was`): a slot edited,
  emptied or deleted meanwhile is left alone and the status says the picture was not placed.
  `place_svg` measures the import laid out alone, on a scratch page standing in for the parent, so a
  landing never clones the document.
- Lands at the pointer, cascaded 24 units per file; at the viewport centre with no pointer over the
  canvas. **One transaction per drop** — placements built via `canvas::image_tx` against a document
  advanced per picture (index, free name, embedded-blob check); every picture made is selected.
- Windows: no drag event carries a position, so the pointer is the desktop cursor via `GetCursorPos`
  mapped against `ViewportInfo::inner_rect` (`canvas::os_cursor_desktop_px`), read before the event
  stream.
- A **single** picture dropped onto the **selected** shape replaces its image fill, keeping crop, mode,
  tile scale and orientation. `Ctrl+V` addresses the selection with no point; the context menu's
  *Paste here* is aimed like the drop. Otherwise a new layer, fitted like a click-placed one.
- `canvas::draw_drop_indicator` while files hover: outline round the target frame (`target_artboard`,
  or an inset canvas border), a pill naming the outcome, and the hover outline on a shape to be
  replaced.
- SVG is never rasterized: a dropped or placed `.svg` (sniffed by its bytes, `app::looks_like_svg`)
  is read by `svg_in::import` through `canvas::read_svg`, the reader a pasted SVG uses too: one
  group named for the file's stem, centred on the drop point (cascaded `DROP_CASCADE` per file) or
  where a paste would land for the *Place image* dialog, which takes `.svg` and imports at once
  rather than loading the Image tool. Its report (*Imported “logo.svg” — 5 layers*) follows the
  pictures' in one status, and the drop selects both.
- A PDF (its `%PDF-` header, `pdf_in::looks_like_pdf`) comes in by the same doors, the drop pill
  saying *Import PDF* (`canvas::read_pdf`, placed by `place_pdf`). `read_pdf` has `pdf_in::import`
  (hayro) draw the pages as SVG in the reading child process (`reader::pdf`, `library.md`), so a
  PDF that crashes hayro costs the import, not the app; `svg_in` reads the pages back on the worker.
  A child that dies without answering is refused as a PDF that does not read is
  (*“brochure.pdf” could not be read as PDF*). Each page becomes a
  white frame at the page's size — a frame even for one page, so the page keeps its paper and its
  edge — named *brochure*, or *brochure — page 2* of several, in a row `PDF_PAGE_GAP` apart on the
  page, centred where a paste would land: one step, the frames selected, *Imported brochure.pdf —
  340 layers on 12 pages*. Past `pdf_in::MAX_PAGES` the rest
  are left out and the report says so; a page that would rasterise or decode past
  `pdf_in`'s pixel budget is left out and reported (*skipped 1 page too large to draw*), and a PDF
  with no page left is refused by name. A password-protected PDF is
  refused by name. An Illustrator `.ai` saved with PDF compatibility is a PDF and comes in as one
  (*Place image* lists the extension, the drop pill counts it); one without is refused with the
  re-save that fixes it (`import::is_illustrator`).
- `Ctrl+V` over a picture-only clipboard produces no egui event, so `input::normal_mode` also reads the
  key **release**; `Action::Paste` tries the picture before the in-app layer paste.

## Traps

- Forcing the mode to `Crop` is a real write: an untravelled press-release would flip a `Fill` picture
  to `Crop` and leave an invisible undo step.
- Calling `draw_crop_box` from inside `draw_points` means it never runs (that only runs for the node
  tool); where a `&mut self` method is called from is not testable.
- Counting the chrome ghost as a preview makes every right-click "cancel" a gesture nobody started.
- Refusing only at the four doors misses the routes into the mode that bypass `begin_image_edit`.
