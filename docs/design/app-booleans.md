# Booleans, flatten, outline and masks in the identity row

Non-destructive boolean containers (`core::boolean`, `NodeKind::Boolean`), the destructive verbs
beside them (flatten, outline shape, outline stroke, convert text to path), the shape builder and
*Divide*, the knife, *Blend shapes*, the identity card's boolean row with its Mask/Group/Ungroup buttons, the
Mask card, and how a failed boolean draws. Part of §9.4.

## 9.4 Booleans

### Model

- `NodeKind::Boolean { op }` is a container. Children keep their transforms and geometry and stay
  selectable and editable; the combined outline is **derived**, cached in `Resolved` beside text
  layouts, invalidated children-first (booleans nest).
- `geometry::local_path` returns `None` for a boolean on purpose. Anything needing "this shape's own
  path" goes through a wrapper: `Resolved::local_path` (canvas; callers `mask_path` and
  `canvas::stroke_outlines`) and `svg::outline` (exporter). `stroke_outlines` has a `Boolean` arm for
  an empty or abandoned outline so it is not drawn as its operands.
- **Stroke alignment is granted by kind**: `geometry::stroke_align_applies` treats a boolean as closed,
  because `boolean::from_flo` closes every subpath and normalizes winding.

### Arithmetic

- `flo_curves` (kurbo has no path booleans), which keeps curves as curves; polygon clippers would
  flatten a union of two circles into a thousand-segment polyline in the file.
- **Coordinates are scaled by a constant into flo_curves and back** — `boolean::c` multiplies,
  `boolean::k` divides; they are the whole seam. flo_curves has absolute tolerances (e.g. a
  `CLOSE_DISTANCE` of 0.01) that would otherwise erase thin results. Scale and accuracy are
  **constants**, never zoom- or size-dependent, or the same shapes would combine differently by camera.
- `boolean::MAX_BOOL_COORD` (10¹⁵⁰) is refused at `evaluate`: past ~10³⁰⁴ (scaled) flo_curves hangs.
  `build::GEOMETRY_LIMIT` bounds each stored number, but operands are in world space, where
  nested transforms multiply.
- **Winding**: flo_curves is even-odd, Mastik fills non-zero. `from_flo` alternates each subpath's
  direction with its nesting depth, so the result means the same under either rule (and is safe for
  SVG). Nesting is counted from `representative_point`: the bbox centre if inside, else a horizontal
  scan line through all segments, taking the midpoint of the first confirmed span. A subpath whose
  point cannot be found is **kept**, not dropped.
- **Shared edges are spliced** (`boolean::spliced`, in `from_flo`): flo_curves leaves operands that
  share an edge as separate contours either side of it, which fill right and stroke a line through
  the result, so two contours walking one edge in opposite directions become one. flo_curves winds
  its contours either way, so `from_flo` fixes each direction first; only then do two contours
  either side of an edge walk it oppositely, and splicing keeps the directions right. Ends must match
  within `SEAM_EPS` (10⁻⁶), well under the thinnest sliver a boolean keeps, so a sliver's two sides
  are never taken for one edge; handles within `SEAM_HANDLE_EPS`, since flo_curves hands a split edge
  back with handles off by around 10⁻⁶. Segments are indexed once by their ends and linked round
  their loops, so each splice is a relink and a grid of thousands of tiles splices in linear time. A
  contour of no area (two touching circles' intersection)
  is dropped; a thin one is a shape and is kept.
- `Exclude` is `exclude_of`: all operand outlines concatenated and read **even-odd** (via
  `Node::fill_rule` deriving even-odd for an `Exclude`). It is not `sub(add(a,b), intersect(a,b))`,
  which returns the operands unchanged. An open contour is closed first, as its fill closes it.
  Read even-odd, an edge two contours both walk adds nothing whichever way each walks it, so
  `boolean::cancelled` takes out every such pair and walks what is left back into closed loops,
  following each segment's own successor where it survived; a rejoined loop may run either way along
  its parts, which an even-odd read does not see. A straight edge shared only in part is first cut at
  the other's ends (`boolean::split_overlaps`, grouping segments by the line they lie on); a curved
  edge shared in part is not cut, and keeps its seam.
- `mastik-core`'s `tests/boolean_suite.rs` checks shared, partly shared and nearly shared edges,
  touching corners, containment, identical operands and the shapes of `tests/common` under every
  operation against the definition, and for specks and stray edges.
- `boolean::fold_operands`: `Union` and `Intersect` fold as a **balanced tree** (N log N on disjoint
  operands, which is what icon masks are); `Subtract` keeps the left fold — it is not associative.

### Operations and operand order

- Children are stored bottom-first, so operand order is layer order. `Subtract` = bottom child minus
  everything above it.
- `build::boolean` refuses fewer than two operands and refuses text (no outline until converted), and
  **gives the result the bottom operand's paint** (otherwise it draws nothing).
- **The key layer** (`key: Option<NodeId>`) moves to index 0, others keep relative order — meaningful
  only for `Subtract`; inert for the commutative three. `build::mask` takes the same argument with the
  same reorder. Once the container exists the key is not stored: re-designating is an
  `Operation::Reorder` to index 0 (`canvas::designate_key`).
- `layers::subtract_base` marks `children[0]` of a `Subtract` with a faint `diamond` glyph in the
  row's badge slot (shared `RowBadge`, `BADGE_BASE`), derived every frame. Only `Subtract`.
- Badge slot priority: mask (`icon::MASK`) > element tag > live brush name > frame size badge >
  subtract base mark (the whole order is in `components.md`, *Layers*).
- **Switching the operation is a `GeometryPatch`**: children and paint stay, undo restores the previous
  op, re-picking is an empty transaction. Travels the normal preview→commit valve.
- The dropdown is **disabled, never hidden**, with one layer selected. Its enabled test is a dry run of
  `build::boolean`. `Ctrl+Alt+U/S/I/X` are the four commands.

### Drawing and preview

- **Operands never reach the canvas**: `scene::paint_node` and `scene::paint_ghost` stop at a boolean.
  The boolean's own fill and stroke govern the result; operands' paint is inert until released.
  The inspector offers Fill/Stroke via `NodeKind::takes_paint()`.
- Gestures re-derive the outline in the preview: `RenderOverrides` keeps its own boolean cache, filled
  once per preview build, **deepest from the root first**.
- Committed and previewed answers share `boolean::evaluate_node` over `boolean::Operands` — five
  lookups the caller supplies (`children_of`, `visible_of`, `kind_of`, transforms, nested outlines).
  Three views use it: `Resolved` (committed), `RenderOverrides` (pending gesture), and ghosts (captured
  subtrees the document never saw). `Resolved::operand_path(doc, id)` is the committed "what this
  subtree contributes as an operand, in its parent's space", over a `with_committed_view` callback.
- `kind_of` matters: a resize changes geometry, so `RenderOverrides` answers with `drawn_kind` (ghost,
  override, then the **used** kind).
- `RenderOverrides::reevaluate_booleans` seeds from: a moved node's parent chain; a ghost's parent
  itself (an Alt-drag copy inside a boolean joins it); and a parent the preview reorders.
  `operand_children` reads the walk's own order, `RenderOverrides::merged_children`: ghosts **at their
  `Ghost::index`** as the commit inserts them (the fold is order-sensitive beyond the first operand),
  starting from `RenderOverrides::children_of` where the preview reorders.
- `render::GhostNode::path` is evaluated once when the ghost subtree is built (`build_ghost`,
  children-first), never in the scene walk.
- **Everything that measures a boolean measures its result**: `query::local_box` and
  `session::preview_local_box` read the (previewed) outline first, so selection box, badge, handles and
  transform fields describe the visible shape.

### Behaviour in the editor

- **Name follows the operation** while it is still one of the four labels, with or without a number
  (`naming::is_boolean_label`); the number carries across. A user-typed name is never overwritten.
- **Selects like a group**: in the spatial index, hit-tested against its outline (`query::hit_test`),
  treated as a group by `group_chain`; double-click steps inside.
- Layers row draws `layers::op_glyph` (shared with the dropdown) and takes drops like a group
  (`layer_row`'s `is_container`), which is how a third operand is added.
- **Grouping inside a boolean re-parenthesises**: `boolean::operand_of` gives a `Group` the union of
  its contents, so `a ⊻ b ⊻ c` becomes `a ⊻ (b ∪ c)`. Intended.
- **SVG** writes the derived outline only, never the operands.
- **Release is `build::ungroup`**: the splice a group needs, folding the used transform into each child.
  Operands' own paint comes back into view; `Ctrl+Shift+G` and the identity row's Ungroup reach it.

## Flatten, Outline shape, Outline stroke, Convert to path

All four share `build::replace_with_path` (*Outline stroke* only when it replaces the layer), which carries what survives being redrawn: transform,
z-slot, paint, name, opacity, a blend mode other than Normal or Pass through, visibility and its
binding, effects, **moved pivot**, **mask flag and mode**, the fill, stroke and effects **style
links**, **export settings**, the **credit**, the **effective fill rule**
(derived even-odd for an `Exclude`, else stored), and the layer's place in a layout (insets when
authored, item properties when non-default). `display` is not carried (a path has no children).
Lock and the proportion lock are dropped deliberately. None of the four has a chord.

| Verb | Admits | Result |
| --- | --- | --- |
| `build::flatten` (`Ctrl+E`) | one boolean, one mirrored path, or two or more layers | one boolean → `Path` of its outline in place; one mirrored path (`build::is_mirrored_path`) → the same path with its mirror cleared, one `SetGeometry` (`flatten_mirror`), so point editing stops keeping it symmetric; a set → union in the parent's space at the topmost member's slot with the bottom member's paint (`build::flatten_union`, carries paint only) |
| `build::outline` (*Outline shape*) | one Rect/Ellipse/Polygon/Star/Line/Spiral/Arc/Path | `Path` of its own outline (`local_path`), radii, smoothing and an ellipse's arc baked in; cut at the **drawn** (used) size, via a `Resolved` lookup |
| `build::outline_stroke` (*Outline stroke*) | one Shape/Path/Boolean with a visible stroke | each visible stroke a `Path` filled with that stroke's brush (a bound brush stays bound), below |
| `build::outline_text` (*Convert to path*) | one Text | `Path` from `mastik_core::text::outline` (glyph coords already node-local) |

- Flatten refuses a single non-boolean other than a mirrored path (that is *Outline shape*'s verb — the two rows
  appear together only on a rounded mirrored path, where Flatten drops the mirror and *Outline shape*
  bakes the corners), text, `Artboard`, `Root`. Groups contribute their union, nested booleans their result,
  via `Resolved::operand_path`; `build::carried_paint` shared by all arms.
- Flatten's two refusals of an admissible selection: `OpError::EmptyGeometry` (operands cancel out —
  move a shape) and `OpError::BooleanAbandoned` (flo_curves unwound — moving won't help).
  `flatten_union` brackets `boolean::failures` across its own `evaluate`; `flatten_boolean` reads the
  cache, so asks `Resolved::boolean_failed`.
- The destruction is one `DeleteNode` over a node that still has children, so one undo restores it.
- `build::can_outline`: on a `Path`, live only if **some radius is `> 0`** (not "not every radius is
  `<= 0`" — `NaN` passes that). Row present and dimmed otherwise. Groups, booleans, artboards, text,
  root refused.
- `build::can_outline_text` asks `text::outline` itself (all-space text has glyphs but no contours).
  ~1µs per glyph, so the menu computes it once at open (`ContextMenu::text_outlineable`).

### Outline stroke

- **The geometry is the ink as drawn** (`build::stroke_ink`, mirroring `scene::paint_strokes`): the
  drawn width (`Document::resolved_paint`), per-side strokes, dash patterns
  (`geometry::dash_fit_pieces`, `resolved_dashes`, cut with `kurbo::dash`), each end's cap, joins
  and mitre limit, the path trimmed under its arrowheads and its markers united with the band into
  one shape. An inside or outside stroke is laid at twice the width, then intersected with or
  subtracted from the shape. A profiled stroke's band is `profile::ink`'s pieces (§5.3), laid along
  the whole trimmed path and flattened, in place of the per-width laying.
- **It is polygon arithmetic, as *Offset path* is** (`build::stroke_polygons`, then `refit`): each
  laid path flattened finer the wider the stroke, and finer for a thin one, stroked by i_overlay with
  bevels at the flattening's vertices, round joins and caps as asked, and each real corner's mitre
  added as its own wedge while it is within the limit. i_overlay's own mitre clips past the limit
  where a rendered stroke bevels, and flo_curves, which an earlier version united band pieces with,
  drops or inverts pieces that share an edge (a 50-radius circle stroked 110 wide lost a quarter).
  The union and the inside or outside cut are i_overlay's, read non-zero. The refit keeps every piece
  of ink, each dot of a half-unit dotted stroke included, down to a hundredth of the half-width
  squared.
- **A stroke that leaves no ink is refused, not removed.** If any visible stroke with a width yields
  nothing (a butt-capped stroke of a dot), the whole operation is `EmptyGeometry`, rather than the
  layer losing that stroke with nothing drawn in its place.
  `mastik-core`'s `tests/stroke_suite.rs` checks the closed shapes and open paths of
  `tests/common` at three widths with each join, cap and alignment, dashed (centred, inside and
  outside), dotted and thin on the catalogue shrunk, and per side on a rectangle, against an
  independent oracle.
- **Result shape.** A layer with no visible fill and one stroke is replaced by the path
  (`replace_with_path`, then the stroke's brush as its one fill, no strokes, non-zero). It drops the
  fill and stroke style links that call carries, since the commit would write a style's lists back
  over the ink: a stroke style would stroke the outline again. Otherwise the
  layer, its strokes removed, and the stroke paths above it (each named "‹layer› stroke") go into a new
  group in its slot, named as the layer; the group takes the layer's effects, visibility, opacity and a non-Normal blend mode,
  each binding moved with its literal, which composite over both (the group isolates the two parts and blends them as one layer): a hidden layer's strokes stay hidden,
  and a bound opacity survives a layer with no visible fill being deleted inside that group.
- Refused for frames (their children would be lost), groups and text, and for a mask the result would
  group: it would mask its own stroke paths and stop masking the siblings above it
  (`build::can_outline_stroke`). The
  layer-menu row dims without a visible stroke (`LayerState::stroke_outlineable`);
  `inspector::outline_stroke` commits and selects the result.

## The shape builder and Divide

`core::shape_builder` cuts the selection's closed shapes into the faces of their planar arrangement.
The shape builder tool merges or removes faces; *Divide* makes each face a path. What lands is
plain `Path` layers.

- **The arrangement** (`boolean::regions`) is built by refinement with flo_curves' `path_cut`, so
  curves stay curves (i_overlay would flatten them, as for booleans): each outline in turn cuts
  every face so far into the part inside it and the part outside, and adds the part of itself
  nothing covered yet. Each face is split into its connected pieces (`pieces`: one outline with the
  holes directly inside it) and lists the outlines covering it in ascending order
  (`Region::covers`), the last topmost. Faces under `REGION_MIN_AREA` (10⁻³ unit²) or a mean width
  (twice the area over the perimeter) under `REGION_MIN_WIDTH` (0.05 units) are dropped: two
  outlines sharing an edge leave slivers along it, and where each traces the edge with its own
  control points (a circle and a turned copy) the slivers pass any area floor. It runs under the
  booleans' unwind guard and failure counter (`unwind_guard`, which `guarded` wraps) and refuses
  coordinates past `MAX_BOOL_COORD`. Faces whose areas add up past the union of what the sources
  fill (a source read by its fill rule entering as its filled faces) overlap (the
  arithmetic can hand back a whole operand as the outside of a nearly coincident one), and
  `arrange` refuses them as `Abandoned`.
- **Fill rules.** A boolean operand's subpaths read as holes where they nest and as one where they
  cross. A source whose own fill rule reads them otherwise (same-way nested subpaths under
  non-zero, crossing ones under even-odd; `boolean::filled_faces`) enters the arrangement as its
  subpaths, and a face keeps it only where its rule fills (`filled_regions`); a face nothing fills
  is no face. Such a solid can come back as more than one face.
- **Sources** (`shape_builder::arrange`): visible with every ancestor, not effectively locked, not
  picture-filled, not a mask (it paints nothing, and a face cut from it would paint its fill and stop
  clipping), outside any boolean, any instance and any layout (a new or reshaped layer there would
  be laid out anew, moving it and its siblings); a rect, ellipse, polygon, star, boolean, or a path
  whose every subpath is closed. Selected groups are opened into their contents.
- **Cutters**: an open path, a line, an arc or a spiral under the same conditions
  (`Arrangement::cutters`) fills no face of its own; each of its subpaths, followed only where it
  meets the faces' box (halved until each part misses it or is near its size, then flattened to
  0.01 units, or a ten-millionth of the box where that is coarser, so a cutter reaching 10¹² units
  costs what one near the faces does; a part still far larger than the box after 64 halvings is
  followed by its chord), cuts every face it crosses through the knife's own surgery
  (`knife::pieces`, `cut_regions`), each piece covered as its face was. It is left as it is by every edit. Any other shown layer (text,
  pictures, frames, locked shapes) is left out, and `Arrangement::left_out` says so. The work is in
  world space, so sources under different parents cut each other as drawn. Refusals: `TooFew`
  (fewer than two sources, or one and no cutter), `NoOverlap` (no face covered twice and no face
  cut), `Abandoned`. `can_arrange` is the dim test: it counts sources and cutters and does no
  arithmetic.
- **Merge and remove** (`Arrangement::merge`, `remove`): every source covering a picked face
  becomes itself minus the picked faces: its outline minus their union, or, for a source that
  entered as its subpaths (*Fill rules*, above), the union of the faces it still fills, since its
  outline read as an operand would fill a hole its rule makes. A path is rewritten in place; any other kind, a live
  boolean included, is replaced by a path in its slot through `build::replace_with_path`, keeping
  its name and look (undo brings the boolean back live). A source left with nothing is deleted;
  a subtraction the guard caught refuses the whole edit with `BooleanAbandoned`, deleting nothing.
  Merge also adds a path, the faces' union, looking like the topmost source over the first face
  picked (`build::carried_look`: paint, opacity, blend, effects), named *Path N*, just above the
  topmost source it took from, in that source's parent.
- **Divide** (`shape_builder::divide`): one path per face, looking like and named after the topmost
  source over it, so nothing moves on screen, stacked by that source. They go into a new group
  named *Divided* just above the outermost selected layer holding the topmost source. The sources
  are deleted; a selected group left with nothing goes whole. When that group held every source,
  *Divided* stands in for it and takes its opacity, blend, effects, mask and place in a layout.
  Otherwise the opacity of every parent a face leaves is folded into the face, less that of the
  parents it lands under; a clip or an effect on such a parent is not carried. A face cannot be
  made more opaque than its source, so when one would land under parents fainter than its own,
  *Divided* goes instead just above the layer holding the topmost source under the sources'
  nearest common parent, where no face gains a parent. Each face takes the
  credit nearest it, on a source over it or a group going with that source.

### The tool (`shape_builder.rs`)

`Tool::ShapeBuilder` (`Shift+M`, glyph `shape-builder`) is the first member of the rail's
Builder slot, with the knife, above the eyedropper; it stays armed after each edit.

- **Arming needs two closed shapes, or one and a cutter** (`MastikApp::builder_slot_usable`
  on `shape_builder::can_arrange`, the test *Divide* dims on). Until then its rail member is
  drawn live, as the Knife beside it is, but its tooltip gives the reason and a click on it arms
  nothing, the palette's *Tool: Shape builder* row is dim, and `Shift+M` arms nothing; all three
  give *Shape builder needs overlapping shapes, or a shape and a line across it* (`shape_builder::UNAVAILABLE`), the
  key on the status line. Once armed, the button stays lit whatever the selection becomes.
- **Selection chrome** while armed is the selection's box alone, dim: no member outlines, which
  would bury the face under the pointer, and no handles, since every press builds.
- Faces are found once per selection and document revision (`MastikApp::builder_faces`), on the
  frame the tool arms; hovering only looks up the face under the pointer.
- **Hover**: a light white wash and a 2pt `color::POINT` edge on the face under the pointer.
- **Drag**: a 3pt trail in `color::POINT` with a dot at the press. Faces join as the trail enters
  them and never leave; the trail is sampled every 2 screen pt so a fast drag cannot jump a thin
  face. The gathered faces draw as one outline, their union, with a pill beside the pointer,
  *N regions*. Alt, read live so it can flip mid-drag, turns trail and edge `color::danger()`,
  hatches the faces at 45° and makes the pill *Remove N*; the cursor's badge goes from Plus to Minus.
- **Release** commits one undo step, *Shape builder*: a merge, or with Alt held a removal. A click
  is a drag over one face: it lifts that face out as its own path, and an Alt-click removes it.
  The merged path and every source still there are selected afterwards.
- Escape or a right-click drops the drag (`cancel_gesture`) and records nothing; a second Escape
  returns to Select.
- **Status line**: on arming *Drag across regions to merge them · Alt-drag to remove*, or the
  refusal *Select overlapping shapes, or a shape and a line across it, then drag across them* (`shape_builder::NEEDS`,
  also said by a press with nothing to build), or *The shapes could not be cut into regions*;
  during a drag *Release to merge N regions · Esc to cancel*, *remove* while Alt is held; after it
  *Merged N regions* or *Removed N regions*. The arming hint, these reports and *Divide*'s end in
  *— only unlocked shapes, paths and lines take part; the rest were left out* when the arrangement left part
  of the selection out (`left_out_note`).

### Divide's doors

`menu::Item::Divide`, glyph `divide`, no chord (a one-way conversion is not one keystroke away):
the context menu's *Boolean ▸* as a second group, the inspector's boolean dropdown under the rule
above *Flatten*, and the palette (aliases
boolean, split, cut, pieces). It dims with *Divide needs overlapping shapes, or a shape and a line across it*
(`menu::DIVIDE_NEEDS`) where `can_arrange` refuses, and is never checked: it leaves no live
operation. The *Divided* group is selected afterwards and the status line says *Divided into N
paths*.

## The knife

`Tool::Knife` (glyph `knife-cut`, a disc cut across; `knife` already means Slice) shares the
Builder slot with the shape builder, since both reshape what is drawn and neither draws. It has no
letter, `C` being held for comments, and is reached from the slot's row or the palette. It stays
armed after each cut.

- **What it cuts** (`core::knife::cut`): the selection's closed shapes, groups opened, or with
  nothing selected every unlocked visible closed shape on the page under the line — the
  shape builder's `takes_part`, so text, pictures, frames, masks, open paths, shapes in a hidden
  group and shapes a layout places are left alone. A shape inside an instance is not cut either:
  cutting it would break the link without saying so.
- **How** (`knife::pieces`): the outline is read as the boolean union reads it (holes wound
  against their ring); where the layer's fill rule reads its subpaths otherwise
  (`boolean::filled_faces`), the union of the faces it fills is cut instead (`parted`), so the cut
  follows only the edges the layer draws. The line's *chords*,
  the stretches between two crossings of the outline
  that run inside it, are cut along exactly by surgery on the rings rather than by a boolean
  (flo_curves mishandles operands sharing an edge, which every piece and the cut side would). A
  chord from a ring back to it splits it in two, each loop taking the holes inside it; a chord to
  a hole first joins the hole to the ring along it both ways (a keyhole), so a ring cut through
  both sides comes apart. A chord that crosses an earlier one is applied in parts, cut where it
  crosses (`split_at_crossings`), so a freehand line looping back over itself cuts along both
  passes. A line that stops inside a shape cuts nothing there; a chord between two points of one
  hole is left alone, and so is a run along the outline itself (a line down a shape's edge or the
  seam two shapes share), whose piece would have no area (`MIN_PIECE_AREA` drops any such piece too).
- **What lands**: each shape the line parts keeps its id, look, name and slot for its first piece,
  as a path (a primitive becomes one, `reshaped`); the other pieces land just above it, looking
  like it and named as it is (`placed_path` with its name). One undo step, *Knife*. With a
  selection, the selection becomes the uncut layers and every piece; with none, nothing is selected.
- **Drawing** (`crate::knife`): a press starts the line and the drag draws it, a point every 2
  screen pt; Shift makes it straight from the press, read live. It shows dashed in the measure red,
  each crossing it will cut at wears an anchor's marker (`paint_anchor`), and a pill beside its end
  counts *Cut N shapes* (`knife::preview`, worked out again when the line changes). The cursor is
  the crosshair.
- **Status line**: on arming *Drag through shapes to cut them · Shift for a straight cut*; after
  a cut *Cut 2 shapes into 4*; a line that parts nothing says *Nothing to cut here*, or *Detach the
  instance to cut it* when it crossed an instance's shape. Escape or a right-click drops the line
  (`cancel_gesture`).

## Blend shapes

`core::blend` makes the steps between two shapes as plain paths. It is computed once; nothing
stays live.

- **The pair** (`blend::pair`): two layers in the same parent, each a rect, ellipse,
  polygon, star, line, spiral, arc, path or boolean, answered bottom of the stack first. Of three
  layers, the one `Path` among them is the spine (`blend::spine_in`) and the other two are the
  pair; three with more than one path cannot say which is followed and are refused (`Count`), and
  *Pick a path…* chooses the spine instead. `blend::refusal` adds the outlines' test (`Parts`)
  and a selected spine's to `pair`'s, so the dimmed row and the card ask what a commit would. A
  `blend::Refusal`'s `says()` is the sentence both the dimmed row and a refused commit give:
  `Count` *Blend shapes needs two shapes, and a path to follow if you like* (also any other kind,
  a group or a frame), `Spine` *Blend shapes can only follow a path with some length to it* (not a
  path, one of the two shapes, or no length: `blend::spine_refusal`; a spine must also pass the
  shapes' `Locked`, `Inside`, `Mask` and `Layout` tests, since it moves), `Picture` *Blend shapes can't take a picture*, `Text` *Blend shapes can't take
  text*, `Parts` *Blend shapes needs two shapes made of the same number of parts, both closed or
  both open*, `Locked` *A layer to blend is locked*, `Inside` *Blend shapes can't work inside a
  boolean, a component or an instance* — the group lands beside the upper shape, and there it
  would be one more operand, or a layer the main does not have; `Parents` *Blend shapes needs two
  shapes in the same group or frame* — moved into the other's parent a shape would fall under
  that parent's clip, opacity, mode and layout, as `build::group` refuses across parents; `Mask`
  *Blend shapes can't take a mask* — in the group it would stop clipping what it clips, as
  `build::can_outline_stroke` refuses one; `MaskBetween` *Blend shapes needs two shapes with no
  mask between them* — the lower shape, moved into the group above that mask, would fall under its
  clip; `Layout` *Blend shapes can't work inside an auto layout* — the parent, or one above it
  through plain groups (`container::lays_out_above`), lays out, and the two moved into the group
  would leave the flow and lose their stretch, as Repeat refuses.
- **Outlines** are taken in world space (`world_transform` × `Resolved::local_path`, so a boolean
  gives its result) and split into subpaths, which pair one to one and must agree on closed or
  open; each becomes cubics. A closed second subpath is turned to run the way the first does (by
  the sign of the area it encloses, curves included, so a lens of two segments reads right), then *reverse* turns it back by hand; each closed one
  starts at the point of its outline nearest the chosen place on its own world box (`Blend::start`,
  a fraction of the box), that segment cut in two. An open one keeps its ends; *reverse* flips the
  second.
- **Pairing is by arc length** (`paired`): each run is cut at the other's anchors' fractions of
  length, so both have the same segments and keep their exact shape, and step k of n puts every
  control point k/(n+1) of the way. Pairing anchor by anchor twists a circle against a rounded
  square and shrinks the middle step.
- **Paint** (`Blend::paint`, on): each step takes the nearer shape's fills and strokes, resolved
  and literal, bindings cut. Where both shapes have as many fills (or strokes) and the one at a
  position is a solid colour in both, the colour is mixed channel by channel, alpha included;
  stroke widths and drawn opacity are mixed. Off, every step wears the first shape's paint and
  opacity. Stroke widths are first scaled into the steps' own units, so a stroke draws as wide as
  it did under a scaled shape, and a gradient is carried by the shape's map into the steps' space,
  with the shape's world box mapped onto the step's (`box_onto`, centre onto centre, each side
  scaled; `paint_box::carry_brush`), so it lies over each step as it lay over the shape wherever
  the step stands. Each step takes
  the nearer shape's fill rule, and with it that shape's windings: a step taking the upper shape's
  rule runs each of its runs the way the upper shape's own does, undoing the turn above, so holes
  stay holes. Effects and blend mode are not carried.
- **The result** (`blend::blend`): an `Operation::InsertSubtree` of a group named *Blend*
  (`GROUP_NAME`) holding `steps` (up to `MAX_STEPS`) `Path` children *Step 1…n*, in the upper
  shape's parent where the upper shape stood, then the two shapes moved into it from that same
  parent (`Reparent` and a `SetTransform` that keeps each where it was drawn): the lower shape at the bottom, the steps from
  it upwards, the upper shape on top. A blend is almost always used with its ends, so the group is
  the whole thing; *Ungroup* gives the parts back. The steps are built on a scratch `Document`, so
  each node takes every default a created node has, then captured whole.
- **Along a spine** (`Blend::spine`, its first subpath in world space): every item, the two
  shapes included, is built as on a straight line and then moved by its box's centre onto its
  place (`Spine::places`), the first shape on the spine's start and the second on its end, the
  steps evenly between **by arc length**. A closed spine shares its whole loop, the last item one
  gap short of the start, as a ring's copies are. `Blend::spine_reverse` runs it from its end.
  `Blend::turn` (off) turns each item about its place by the tangent's angle there less the
  tangent's at the start, so the first shape keeps the way it faces. A gradient is carried with the
  move. The spine stays its own layer, unchanged on the page, moved to stand directly over the
  group in its parent (`Reparent` and a `SetTransform` keeping its world transform), so the steps
  never hide it. Only a spine beside the shapes moves, and only when no mask lies between its slot
  and the group's; one in another parent, or past a mask, stays where it is, since the move would
  give it or take away a clip, an opacity or a mode.

### The card (`blend.rs`)

`menu::Item::BlendShapes`, *Blend shapes…*, glyph `blend-shapes` (a small circle growing into a
square through a rounded step), group Structure, no chord; palette aliases morph, tween, steps,
interpolate. *Blend* alone already names the blend mode in the inspector and its undo steps. In the
layer menu it follows *Flatten* whenever two or three layers are selected, dimmed with
`blend::refusal`'s sentence (`LayerState::blend_refusal`) or the lock sentence; otherwise the palette's *Not available
here* row says *Blend shapes needs two shapes, and a path to follow if you like* (`KIND_VERBS`).

- **The card** is an inspector-lane popover placed as the Repeat card is
  (`MastikApp::transform_popover_area`, level with the Transform card, `tools.md`): no modal, no
  click-away. Rows: head `ui::popover_head` *Blend shapes*; *Steps between* (`icon::DOTS_THREE`, 1
  to 200, default 5); *Along*, a dropdown wearing the path layer's glyph, of `ui::menu_check`
  rows: *A straight line* (the default), *Path · ‹name›* while one is chosen (ticked), a rule,
  *Pick a path…*, and while a path is chosen *Reverse the path*; the switch *Turn the steps with
  the path* (off), disabled on a straight line with *A straight line turns nothing* — both rows
  are there whatever is chosen, so the card keeps its height; the switch *Blend paint and
  opacity* (on); *Match points from*, a dropdown of
  the nine places (`canvas::NINE_POINTS`, the names the pivot's pill uses), default *Top left*; the
  switch *Reverse the second shape*; foot `settings::footer_row`, *Cancel*, primary *Blend*. Its
  settings live on the app and outlive a close. It holds the lane alone: opening it closes the
  inspector's popovers and the Repeat card, opening the Repeat card closes it, and so does an
  inspector popover opened after it. Find and Colour review are left open.
- **A right-click cancels a scrub** of *Steps between* wherever the pointer is
  (`InFlight::blend_scrub`, as the Repeat card's `repeat_scrub`).
- **The spine.** The card opens on the path selected with the two (`spine_in`), else on a
  straight line, and lets go of a spine that stops fitting (`spine_refusal`). *Pick a path…*
  hides the card and makes the canvas wait for a path (`BlendCard::picking`): the status says
  *Click a path to blend along · Esc to cancel*, said again before it fades or once another
  message replaces it, for as long as the wait lasts; the cursor is the crosshair, every press but an
  armed pan's goes to the pick (`blend_pick_at`), and the hover outline gives way to a 3pt accent
  ring round the path under the pointer the card could follow (`MastikApp::blend_spine_at`: the
  outline under the pointer, `rail_at`, then the leaf, `pick_leaf`, kept only if `spine_refusal`
  passes it); anything else is passed over. A click on one chooses it and brings the card back;
  `Escape` calls the wait off, a rung before the card's own. While the card is open its spine is
  drawn dashed in the accent (`canvas::draw_blend_spine`).
- **The preview** is the commit's own edit less the moves into the group and the spine's (a
  preview cannot reparent; the two shapes' `SetTransform`s are kept, which place them on the spine
  in their own parent, and the spine draws where it stands), minted from a copy of the id
  counter so it spends no ids; rebuilt when the revision, selection or settings change, and put back after another
  gesture's preview cleared it. A canvas drag owns the preview while it lasts.
- **Keys.** `Enter` blends (`dispatch` hands `Action::Enter` to the card while it is open);
  `Escape` cancels, the rung after the Repeat card. The card closes when another document comes to
  the front and when the selection stops being blendable.
- **Commit**: one undo step, *Blend shapes*; the *Blend* group is selected and the status line
  says *Blended in 5 steps*, or *Blended in 5 steps along ‹name›*.

## Apply brush

One shape, the brush, runs along a path: *Art* bends it along the whole path, *Scatter* repeats it
along it. By default it is a **live brush** (`core::live_brush`): the path stays the layer, linked
to the brush and the options, and the brush draws in place of its strokes, redrawn whenever the
path, the brush or the options change. *Bake into paths* instead computes it once into plain paths
(`core::apply_brush`). Both share one geometry.

- **The pair** (`apply_brush::pair`): of two selected layers the lower is the path and the upper
  the brush, unless only the upper has an outline to run along (a rect, ellipse, polygon, star,
  line, spiral, arc, path or boolean; `live_brush::runs_along`), when the two swap. More than two
  layers, or two with no outline between them, are `Count`, *Select a path, and the shape to run
  along it*. The refusals add the rest, each a `Refusal::says()` sentence the dimmed row and a
  refused commit both give: `Same` *A brush can't be the path it runs along*, `Text` *A brush
  can't be text*, `Picture` *A brush can't be a picture*, `Flat` *A brush needs some length to lay
  along the path* (a brush whose world box has no width), `Length` *The path has no length to run
  along*, and for the path Blend's tests: `Locked` *The path is locked*, `Inside` *Apply brush
  can't work inside a boolean, a component or an instance*. A bake (`apply_brush::refusal`) adds
  the two it owes the group that replaces the path: `Mask` *Apply brush can't take a mask*,
  `Layout` *Apply brush can't work inside an auto layout*. A live link (`live_brush::refusal`)
  drops those two and adds `BrushInInstance` *A brush can't be inside an instance* and `Loop` *A
  brush can't be drawn along a path it brushes* (the brush or anything inside it reaches the path
  through a chain of links). The brush is otherwise only read, so a locked brush or one elsewhere
  in the tree is fine. The path's half of these (`apply_brush::path_refusal(doc, res, path,
  bake)`: no outline, `Locked`, `Inside`, a bake's `Mask` and `Layout`, `Length`) stands alone for
  a card opened on one layer before its brush is picked; `live_brush::selection_refusal` of a
  one-layer selection is that, live.
- **Geometry** is in world space: the path's every subpath with length is a track, measured by
  a per-segment table of arc length at 32 steps of `t`. *Art* maps the brush's world box onto each
  track: `x` to arc length (the box's left at the start, its right at the end), `y` to an offset
  along the left-hand normal about the box's middle line, times *Width*; *Flip across* negates
  it. *Middle only* (`Stretch::Middle`) keeps the box's first and last fifth (`CAP`) at their own
  length times the width, shared between the two when the path is shorter, and stretches the
  middle. Each brush cubic is cut where its `x` crosses a bend in the map (a track anchor, a
  fifth's edge), then mapped as a cubic through the mapped ends with the mapped tangents there,
  halved until it stays within `TOLERANCE` (0.02) of the exact map at its quarters, so curves stay
  curves and a closed brush or one of several subpaths (a ring) bends whole. Where the path turns
  a corner the brush's two sides part; a straight line joins them. *Scatter* places a copy every
  *Spacing* × the brush's length along an open track, the leftover split between its ends, or a
  whole number round a closed one; each copy is the brush scaled about its box's centre by *Size*,
  varied ± *Size varies*, moved sideways off the path by up to ± *Scatter* × the brush's length,
  and turned with the path's tangent (*Along the path*) or not (*Fixed*). The two random draws
  per copy come from a splitmix stream seeded by *Seed* (`content_fill::Rng`), both drawn for
  every copy so one setting at zero keeps the other's run. More than `MAX_COPIES` (2,000) is
  refused, `TooMany`.
- **The result** (`apply_brush::apply`): an `Operation::InsertSubtree` of a group named
  *Brushed · ‹brush›* (`group_name`) in the path's slot of its parent, holding one path per copy
  (one for Art), each named after the brush and wearing its look: fills and strokes resolved and
  literal, stroke widths carried into the new units (an art stroke keeps its width however far
  the brush stretches; a scattered one scales with its copy), gradients carried (over the bent
  box for Art), and its fill rule, effects, drawn opacity and mode. Then the path is deleted, or
  with *Keep the path* moved to the group's bottom, hidden and named *Path*
  (`KEPT_PATH_NAME`), where it was drawn. The brush stays where it was. Built on a scratch
  `Document` as Blend's steps are. `apply_brush::apply` is `refusal` then `build`; a live brush's
  bake calls `build` alone, since it holds paths no bake is offered (a mask, in an auto layout).

### The live brush (`live_brush`)

- **The link** is `Node::brush`, a `BrushLink { of, mode, visible }`: the brush layer, the card's
  options (`apply_brush::Mode`), and whether the brush shows (hidden, the path's strokes draw
  again). It is written by `Operation::SetBrush`, refused (`OpError::BrushForKind`) on a kind
  `runs_along` refuses and for a link to the layer itself. Copies of a main follow it, a copy's
  own is an override, and reset puts back the main's; a brush inside the same main is read as each
  instance's own copy of it (`components.md`, *Overrides are found by comparing values*).
- **What draws** (`apply_brush::ink`): the bake's geometry kept in the path layer's own space, so
  moving the layer moves it. An `Ink` is a path, the places it is drawn at (the identity for Art,
  one map per copy for Scatter, whose path is the brush's own outline), the factor on the brush's
  stroke widths and the map carrying its gradients; `apply_brush::carried` lays the brush's paint
  on it for the bake, the walk and the hit test alike. The brush's whole look draws: fills,
  strokes, gradients, fill rule, effects, drawn opacity and blend. The path's fills still draw;
  its strokes give way (`live_brush::hides_strokes`) while the link is shown and names a layer
  outside the vault, even when the brush draws nothing, so a broken brush shows as broken
  (`live_brush::broken` gives the reason); a link to a layer that is gone draws the strokes.
  Hiding the brush layer does not change its users.
- **The link never names nothing.** Deleting a brush, or a layer holding one, bakes every layer
  it brushed that is not deleted too (`live_brush::orphaned`, `baked_for_deletes`), put in front of
  the transaction at the app's commit door (`EditorSession::commit_inner`, before
  `propagate::owed`), so it is one undo step; the status says *Deleted ‹name› · baked the N
  layers it brushed*. A copy carries, beside each subtree, a twin with every brush whose brush
  stayed behind baked (`live_brush::baked_copies`); a paste into a document without the brush
  takes the twin (`live_brush::fitted`, `MastikApp::clip_here`), one into the same document keeps
  the link, remapped when the brush came along. `fitted` answers the names of the brushes it baked
  (read off the *Brushed · ‹brush›* groups the twins hold and their originals do not), and the paste
  doors and *Replace with clipboard* append to their sentence *· ‹brush› isn't in this document, so
  its brush was baked* (*· N brushes aren't in this document, so they were baked*).
- **The bake** (`live_brush::bake`): the *Brushed · ‹brush›* group in the path's place, the path
  kept at the group's bottom, shown under its own name with its strokes and link dropped, while
  it has a visible fill, and deleted otherwise. A brush that draws nothing just drops the link.
- **Writers that cannot draw it live** take `live_brush::baked_view`, a copy of the document with
  the brushes baked (`export.md`).

### The card (`apply_brush.rs`)

`menu::Item::ApplyBrush`, *Apply brush…*, glyph `brush` (a brushstroke pointed at both ends),
group Structure, no chord; palette aliases art brush, scatter brush, along a path, bend. In the
layer menu it follows *Blend shapes…* whenever two or more layers are selected, or one with an
outline to run along (`LayerState::runs_along`) and no brush (a brushed one has *Edit brush…*), dimmed with
`live_brush::selection_refusal`'s sentence (`LayerState::brush_refusal`; a bake's own two are
said when it commits) or the lock sentence; otherwise the palette's *Not available here* row says
*Select a path, and the shape to run along it* (`KIND_VERBS`). Whenever the selection holds a
brushed layer (`LayerState::brushed`) three rows follow it, group Structure, no chords, dimmed only
by the lock: *Edit brush…* (`Item::EditBrush`, glyph `brush`; aliases brush options, live brush),
*Bake brush* (`brush-bake`, the brush's outline with a filled path point at each tip, the outline
view's language; expand brush, brush to paths) and *Remove brush* (`brush-slash`, the brush struck
through as `eye-slash` is `eye`; unlink brush, clear brush). Their palette sentence is *Select
a layer drawn with a brush*.

- **The card** sits in the inspector's popover lane as Blend's does, and holds it alone: opening
  it closes the inspector's popovers, Repeat, Blend shapes and the warp envelope, and each of
  those closes it. Rows: head *Apply brush*; *Brush*, a dropdown wearing the brush's kind glyph and
  reading *Shape · ‹name›*, of `ui::menu_check` rows: the brush (ticked), a rule, *Pick a shape…*,
  *Swap brush and path*; a segmented *Art | Scatter*; then the mode's rows in a body as tall as
  Scatter's, so the footer stays put: Art's *Width* (%, default 100), *Stretch* segmented *Whole
  shape | Middle only*, switch *Flip across*; Scatter's *Spacing* | *Size* (120%, 80%), *Size
  varies* | *Scatter* (30%, 20%) as pairs (`repeat::pair`), *Turn* segmented *Along the path |
  Fixed*, *Seed* (1); the switch *Bake into paths* (off), and under it *Keep the path*, dimmed
  while baking is off with *A live brush keeps the path: it is the layer*; foot *Cancel*, primary
  *Apply*. Its settings live on the app and outlive a close. A right-click cancels a scrub of any
  of its numbers (`InFlight::brush_scrub`).
- **Path and brush** are taken from the selection when the card opens. Opened on one layer, it
  has a path and no brush: the dropdown reads *None* (its ticked row too), nothing previews, and
  *Apply* and *Swap brush and path* are disabled with *Pick a shape to use as the brush*; once
  *Pick a shape…* names one it is the pair's card. The card closes when the
  selection changes, when the two stop fitting (the live link's tests, which a bake's card is held
  to as well), and when another document comes to the front. *Swap brush and path* trades them,
  or says why the swapped pair cannot run. *Pick a shape…* hides the card and makes the canvas wait as Blend's *Pick a path…* does
  (`BrushCard::picking`, `brush_pick_at`): the status says *Click a shape to use as the brush ·
  Esc to cancel*, kept up as Blend's is; the cursor is the crosshair, the hover outline gives way
  to a 3pt ring in the selection blue round the shape under the pointer the card could take
  (`MastikApp::brush_shape_at`: not the path, and passing `brush_refusal`). A click on one takes
  it and brings the card back; a click on the path says *A brush can't be the path it runs
  along*, left up for its hold while the wait goes on; anything else is passed over. While the
  card is open its path is drawn dashed in the selection blue (`canvas::draw_brush_path`).
- **The preview** of a live brush is the `SetBrush` itself, drawn as committed. A bake's is the
  commit's own group, minted from a copy of the id counter, with the path hidden (a preview can
  neither delete nor reparent). Either is rebuilt when the revision or the settings change and put
  back after another gesture's preview cleared it. *Cancel* leaves everything as it was.
- **Keys.** `Enter` applies while the card is open; `Escape` calls off the wait for a shape, then
  closes the card, the two rungs after Blend shapes'.
- **Commit**: one undo step, *Apply brush*. Live, the path stays selected and the status says
  *Brushed ‹path› with ‹brush›* (Art) or *Scattered ‹brush› along ‹path›* (Scatter). Baked, the
  group is selected and the status says *Brushed with ‹brush›*, or *Scattered 9 copies of
  ‹brush›*.
- **Edit brush** is the same card on the selected brushed layers, headed *Edit brush* and filled
  from the first one's link; *Done* (`Enter`) writes the card's options to every one of them as
  one *Edit brush* step, each keeping its own brush (unless *Pick a shape…* named another,
  `BrushCard::relinked`) and whether it shows. It has no bake switches.
- **What the layers differ in reads *Mixed*** (`apply_brush::Mixed`): the brush (the dropdown
  reads *Shape · Mixed* until *Pick a shape…* names one); the kind (the *Art | Scatter* segment
  shows `ui::segment_mixed`'s dash with nothing raised, and a `ui::selection_note`, *These layers'
  brushes are Art and Scatter; pick one to set them all*, stands in place of the option rows); and
  each option, compared among the layers of its kind (number fields read `ui::MIXED_WORD`, *Stretch*
  and *Turn* show the dash, *Flip across* is `ui::switch_row_mixed` and a click turns it on).
  Untouched, a mixed option keeps each layer's own value (`BrushCard::mode_for`); setting it
  (typing, a scrub, a click) writes it to every layer. A kind picked over a mixed one shows each
  kind's fields from the first layer of that kind. A right-click that cancels a scrub puts back the
  mixed flags with the numbers.
  The Brush dropdown dims *Swap brush and path* (*A live brush's path is its own layer*) and adds,
  under a rule, *Bake brush* and *Remove brush*. Its doors are the layer menu and the inspector's
  brush row (`inspector.md`, *Fill and Stroke*).
- **Bake brush** bakes each selected link (`live_brush::bake`) as one *Bake brush* step, selects
  the groups and says *Baked ‹brush› into N paths*. **Remove brush** drops each link, one *Remove
  brush* step.
- Width handles do not show on a brushed layer (`width_handles::edited_profile`): its strokes do
  not draw. For the same reason, while its points are edited a path whose live brush is shown is
  outlined in a 1 pt `color::SELECT` line (`canvas::draw_points`), so the path being edited can be
  seen; leaving point editing hides it.

## The identity card's boolean row

Four controls: the op dropdown plus three 28×28 icon buttons, `ACTION_GLYPH` 16.

- **Dropdown**: four ops, then under a hairline two actions, not states: *Divide* (dimmed with
  `DIVIDE_NEEDS` where `can_arrange` refuses) and *Flatten* (`stack-simple` glyph, as in the
  context menu). The six rows sit well under `ui::DROPDOWN_MAX_H`, so the list never scrolls.
  `inspector::flatten_selection` gates on the dropdown's own predicate, plus a lone mirrored path,
  so the keyboard path reports a useful sentence. `inspector::popup_rule` draws the hairline via `ui::hairline_in_column`, sized from
  `available_width`, with gaps `ABOVE` 5pt / `BELOW` 4pt and the layout's `item_spacing.y` subtracted
  back out with `add_space`.
- **Mask** (`icon::MASK`) is a state: lit while the target is a mask, click again releases.
  `Inspector::mask_action` → `Make { members, key }` or `Release(id)`, behind both look and action.
  - Make goes through `build::mask` (wraps two or more in a group; a lone layer is flagged in place).
    Which layer becomes the mask is `build::mask_target`'s answer only: the key if designated, else the
    bottom-most non-picture member, else the bottom-most, among members `build::can_be_mask` accepts.
  - `mask_action` dry-runs `build::mask` on a throwaway `IdSource`, so builder refusals dim the control.
  - Release reads either end: one selected mask, or one selected container holding one.
  - Requires one parent across the selection. Returns a `Result` because each refusal has its own
    tooltip — including refusing layers inside a `Boolean` (operands are not artwork), which is
    redundant for correctness (the dry run declines too) but load-bearing for the message.
  - Same verb on `Ctrl+Alt+M` and the context menu's *Use as mask*, which dims with the same sentence
    via `MastikApp::mask_refusal` → `menu::Context::mask_refused`.
- **Group / Ungroup** (`icon::SELECTION_PLUS`, `icon::SELECTION_SLASH`), gated by `inspector::group_actions`:
  - Ungroup live when something selected is a `Group` or `Boolean`; both buttons call
    `app::ungroup_selection`, which builds through `build::ungroup_all` — one transaction however many
    containers, freed layers selected.
  - Group is a dry run of `build::group`, plus: a selection that is exactly one container is refused.

## The Mask card

`inspector::inspector_mask`: directly under the identity card while the selection is a mask **or a
container holding one**. One row: a dropdown of *Shape*, *Alpha* or *Luminance* (`MaskMode::ALL`) and,
beside it, an *Invert* toggle (`ui::field_button_marked`, glyph `icon::MASK_INVERT`, tooltip *Invert —
show only what lies outside the mask*). Targets come from `inspector::mask_card_targets` (free
function): the selected layer if it is a mask, else its **direct** mask children. A list — two runs
report *Mixed* and a pick writes every target that would change, in one transaction. The toggle is On
only while every target is inverted; a click inverts them all, or turns them all back when all are
(undo step *Invert mask*). On an instance each control carries its own override mark (`sub_mark`, over
`SetMaskMode` and `SetMaskInverted`), and the component panel's reset groups both under *Mask*. There
is no "mask group" flag; it is derived from the children every time.

## When a boolean fails

- flo_curves can panic (intransitive comparator in `GraphPath::exterior_paths`) on any op. The unwind
  is caught at `boolean::evaluate`, which answers `None`; the boolean draws a placeholder, the
  document survives. A `#[cfg(panic = "abort")] compile_error!` sits beside the guard.
- `boolean::failures()` is a thread-local **count** read either side of a call (an empty intersection
  also answers `None`; a count composes across nested inner `evaluate`s). `Resolved` keeps the answer
  per node (`Resolved::boolean_failed`); the layers row takes the warning colour and the commit says so
  in the status bar.
- It draws §5.5a's missing-picture placeholder over `query::boolean_placeholder` (the operands'
  union); scene walk, hit test, `Resolved::recompute_bounds` and SVG all ask that one function, giving
  the node world bounds so it stays in the R-tree. Committed-only: during a gesture
  `RenderOverrides::boolean_path` always has an entry.
- Text cannot be an operand; a mask can be text (`Resolved::mask_path` takes glyph outlines).
  Nor can a slice, which has no outline and would lend the result its empty paint: `boolean` and
  Flatten refuse it, and the layers panel will not drop one into a boolean.

Performance: re-running the whole boolean per operand edit and per preview build is cheap enough with
the balanced fold; per-operand caching is not to be built. Pixel tests live in
`mastik-export/tests/boolean.rs` (a subtracted hole and an excluded overlap are each `alpha == 0`).

## Traps

- Asking `geometry::local_path` for a boolean gets `None` silently — wrong ring, centred aligned stroke,
  unmeasured dashes. Go through the wrappers.
- A preview evaluator that overrides only transforms moves operands without resizing them; `kind_of`
  must answer the drawn kind.
- `MAX_BOOL_COORD` guards a hang the `catch_unwind` cannot see; the panic guard is inert under
  `panic = "abort"`.
- Group's lone-container refusal looks redundant in front of a builder that says yes, and nothing
  tests the row's gating.
- `replace_with_path` must carry the effective fill rule, or a flattened `Exclude` becomes its union.
- *Outline stroke*'s ink must not be `kurbo::stroke`'s expansion, though that looks like the same
  thing in one call: read even-odd it double-counts the inner corners (a 100 square with a 10 stroke
  comes out 4100 instead of 4000). `outline_stroke_traces_the_ink_each_alignment_draws` and
  `stroke_ink_covers_what_a_non_zero_expansion_fills` (`mastik-core/tests/build.rs`) pin it.
- *Blend shapes* must pair by arc length, not by matching anchor counts, though the latter looks
  simpler. `the_middle_step_is_half_way_in_place_size_and_paint` (`mastik-core/tests/blend_shapes.rs`,
  a circle against a rounded square) checks the middle step's width.
- A live brush's ink is rebuilt by three tests in `Resolved::update`'s `rebrush` that each look
  covered by the dirty set: the users `sync_brushed` added, users whose brush the pass reached,
  and users whose layer's or brush's world transform moved off the entry's. A layout resizing the
  brush, or an ancestor moving it, dirties neither layer, so dropping either of the last two leaves
  stale ink.
- `cancel_gesture` must not take the Blend card's standing preview for a gesture
  (`blend_preview_standing`, beside `repeat_preview_standing`), or a right-click on the canvas
  cancels it instead of opening the menu.
