# Prototyping: the saved model

What a prototype is made of in the document: a layer's reactions and play settings, a page's
flows and play settings, and the document's HTML-export settings. Everything here is authored,
goes through the commit door, is undoable, saved and copied with its layers. The engine that
plays it and holds play state is `play.md`, the export `prototyping-export.md`; the player's
shells are designed in `prototyping.md`. Code: `mastik-core/src/prototype.rs` and `prototype/expr.rs`.

## Where it lives

| Field | Type | Holds |
| --- | --- | --- |
| `Node::reactions` | `Vec<Keyed<Reaction>>` | The layer's triggers and the actions each runs, in order |
| `Node::play` | `LayerPlay` | A frame's scroll axis and overlay defaults; a child's pin; an input binding |
| `Page::flows` | `Vec<Keyed<Flow>>` | Named ways into the page's prototype |
| `Page::play` | `PagePlay` | Device preset name, scaling, the default transition for a new link |
| `Document::play_export` | `PlayExport` | Which flows the HTML export takes, which it opens on, device frame, player bar, hotspot hints, a scaling override |

- **The type is a *reaction*, not an *interaction*.** `variant::Interaction` already names a
  variant value's tied browser state.
- **A reaction** is a `Trigger` and an ordered `Vec<Keyed<Action>>`. Each action has its own item
  id, nested ones under a `Conditional` too.
- **Triggers**: `Click`, `HoverEnter`, `HoverWhile`, `Press`, `PressWhile`, `Drag`, `Key` (plain or
  Shift-ed), `After { ms }`, `VariableChanges { var }`, `FrameOpens`.
- **Actions**: `Navigate`, `Back`, `OpenOverlay`, `CloseOverlay`, `SwapOverlay`, `ScrollTo`,
  `ChangeState` (an instance and the variant main it switches to), `SetVariable` (`Set`,
  `Increment`, `Toggle`), `Conditional` (branches, then `otherwise`), `OpenUrl`, `RunScript`. Every
  action that moves the screen carries a `Transition`: a kind, a direction read only by the
  directional kinds, and a `Timing` (an easing curve over a duration, or a spring).
- **`LayerPlay`'s frame fields are inert on any other kind**, and its pin is read only inside a
  scrolling frame. An `Input` binds a variable; on an instance it names the boolean variant
  property that shows it, on a text layer none (it binds a string).
- A **flow** is a name, a start frame and starting variable cells, which beat the modes' values.
- **Expressions** (`prototype::Expr`: a condition, *Set variable*'s value) are stored as source text,
  a variable written `{<actor-hex>:<seq>}` so a rename never breaks one. The language is in
  `play.md` *Expressions*.

## Operations

`SetReactions`, `SetPlay` (a layer), `SetFlows`, `SetPagePlay` (a page) and `SetPlayExport` (the
document, subjectless as the script ops are) each write their field whole and invert to the old
value. None changes ink, and each is a no-op in `RenderOverrides::absorb`. `AddPage` carries a
page's flows and play settings, so the inverse of a removal brings them back.

**What the door refuses** (and the loader refuses the same in a file):

- *Run script* under a trigger no person fires — `After`, `VariableChanges`, `FrameOpens`
  (`Trigger::is_a_persons_act`; `ReactionError::ScriptUnasked`). Under those a script would run
  unasked, which the document never does.
- An unusable timing (`Timing::is_usable`: finite, a spring's numbers positive, a cubic's x within
  0..=1), on an action, a page's default transition, or a non-finite overlay position or backdrop
  (`LayerPlay::is_usable`; `OpError::BadPlay`).
- A repeated id: an action id within one list (`ReactionError::DuplicateAction`), a reaction id on
  a layer (`Node::check_item_ids`), a flow id on a page.

**Targets dangle by design.** Neither the door nor the loader checks that an action's target, a
flow's start, an input's variable or a named script exists. Play skips a missing one, and undo of
the delete mends it. A target has nothing to draw, so keeping it costs nothing. An input's
variable counts as a target here, not as a binding, which the loader does refuse when dangling
(§5.3f).

## Saved form

- Schema v20 (`io.md`); every field is skipped when empty or at its default.
- **The enums are externally tagged** (`{"Navigate":{…}}`, a unit variant as a bare string), as the
  file's other enums are: an instance follows its main field by field, and the merge
  (`propagate::variant_changed`) recognises a change of kind only as the object's one key changing.
- Layer ids inside reactions and flows are saved in wire form, as every other saved id.

## Instances

- **Reactions are the seventh keyed list.** A main's reactions reach its copies through
  `propagate::items`, reaction by reaction, as exports do; an instance overrides a reaction by
  holding a different item under the same id, and push to main and reset work per item. Within a
  matched reaction, fields follow as `propagate::fields` merges them, so the action list follows as
  one value. `SetPlay` follows field by field.
- **A target inside the main is stored as the main's id on every copy**, so the copy's reaction
  equals the main's and is no override. `prototype::resolve_target(doc, holder, target)` maps it to
  the holder's own layer when read, following the link chain for a nested instance; a target with
  no counterpart (outside the main, or deleted) is itself.

## Copies and clean up

- `remap_subtree` maps a target inside the copied template to its copy and keeps one outside it.
- `build::insert_subtrees_as` then maps targets across templates copied together, so frame A
  linking to frame B, both duplicated, links copy to copy. It skips linked layers, whose targets
  name their main's layers.
- `page::duplicate` points the copy's flows at its own frames.
- Clean up keeps a layer with reactions or play settings (`Kept::Prototype`, *In the prototype ·
  kept*): an empty layer with a click is a hotspot.

## Traps

- **The externally tagged enums are load-bearing.** An internally tagged or untagged `Action` would
  let the copy merge mix two kinds' fields. `targets_are_saved_as_wire_ids` pins the tag and the
  wire form.
- **Two remaps, each looking removable.** `remap_subtree` sees one template; the pass in
  `insert_subtrees_as` sees across them. `a_copy_remaps_targets_inside_the_template_only` and
  `duplicating_two_frames_links_the_copies_to_each_other` pin one each.
- **A linked copy undoes the first remap.** Every door that makes an instance node runs
  `remap_subtree` and then `Node::make_copy_of`, which puts the source's reactions back unmapped;
  without it a fresh instance's reactions would all be overrides and stop following the main.
  `an_instance_holds_its_mains_targets_and_reads_its_own_layers` pins it.
