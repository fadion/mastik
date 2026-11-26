# Foundations: scope, stack, workspace, milestones, readiness

What Mastik is and is not, the pinned stack, the crate graph and its dependency rules, milestone status, the seams reserved for multiplayer, and Command Mode's decisions. (Original §1–§3, §10, §12–§14.)

Mastik is a native, open-source 2D design tool (Figma-class scope, minimal v1) written in Rust, for Windows and Linux and as a release target for Apple Silicon Macs. The document model is API-first: the operation path that serves the UI also serves AI agents over MCP. Seams (identity, mutation path, renderer boundary, serialization) matter more than feature count. Items in `invariants.md` are non-negotiable; if a change would violate one, stop and surface the conflict.

## 1. Scope

"v1" names the phase in which the basic editing tools were finished, not a release tag; that phase is over.

### In scope

- Pages, each an infinite canvas of artboards (§5.5). Node tree per page: Root → Artboard → (Artboard | Group | Rect | Ellipse | Line | Path | Text)\*, with arbitrarily nested groups and frames.
- Layers panel: tree view, reorder, rename, visibility, lock.
- Shapes: rectangle and frame (corner radius and smoothing), ellipse (wedges and rings), polygon, star, line, freeform path.
- Text: plain text nodes plus rich text (mixed runs, OpenType feature UI, per-paragraph attributes, lists with nesting and markers, decoration skip-ink, text-on-path via `NodeKind::Text::on_path`), editable in-canvas (§5.4, §9.2).
- Fonts: all installed system fonts, the full Google Fonts library (on-demand download with local cache), and a bundled fallback (§5.4a).
- Paint: solid and linear, radial and angular gradient fills; strokes with width, join, cap, dash, mitre limit and alignment (inside/centre/outside, §6.3, since no backend has it natively); a rectangle's stroke on any of its four sides. Several fills and strokes per layer, painted and exported in list order.
- Masks: any layer with an outline can mask the run of siblings drawn above it in the same parent, by its outline, its own alpha or its brightness (`MaskMode`), drawn alike on the canvas and in every export.
- Effects: a repeatable ordered stack per layer (drop shadow, inner shadow, layer blur, sharpen, noise, texture, background blur, colour filters), meaningful on containers too. Drawn on the canvas, the raster export and the SVG file from one specification, `mastik_render::effects`, except a background blur, which SVG cannot express and leaves out, reported (§5.3a, §6.4, §7).
- Blend modes per layer: the sixteen CSS/SVG/PDF modes plus Pass through on frames and groups, drawn alike on the canvas and in every export (§5.3, §6.2, §7).
- Transform: translate/rotate/skew per node (local) about a movable per-node origin; resize edits geometry, not scale (§5.3, §5.6).
- Selection, move, resize, z-order; copy/paste/duplicate via `InsertSubtree` (§5.7).
- Bulk property edits over a selection (transform, opacity, blend mode, corner radius, paint), and recolouring by colour rather than by layer (§9.4).
- Container layout: CSS flexbox, CSS grid and absolute insets (§5.3c). Fixed layout algorithms re-run from specified values.
- Components and instances as linked copies, variants and component properties, instance swap (§5.3d); shared component libraries ("kits", §5.3e); variables and styles (§5.3f).
- Images: fills, raster import (dialog, drag-and-drop, paste), framing, image editing (crop, orientation, tile scale, per-pixel adjustments), export in all three writers, missing-image placeholder with *Replace…* (§5.5a, §7.1, §9.2).
- Pan/zoom viewport; R-tree culling for many artboards.
- Rulers and ruler guides, global or scoped to one frame; guides are document state (§5.5, §9.4).
- Focus mode: hides the panels, keeps the canvas and its rulers, Escape returns (§9.4).
- Undo/redo: unlimited, transactional, gesture-granular.
- Save/load: native versioned document format with deterministic bytes.
- Export: SVG (semantic), PNG and JPEG (CPU renderer, no GPU needed), JSON snapshot (AI handoff).
- MCP server (read/query/export, then write operations against the live document) and headless mode `mastik serve` (§8.2).
- Command Mode: the command bar (typing a line, picking layers by label) and scripts in sandboxed Luau, stored in the library, a document or a kit and never run by the document (§13, `command-mode.md`).
- Platforms: Windows, Linux, and Apple Silicon macOS. Nobody developing Mastik has a Mac; CI's macOS leg is its only compiler. v0.4.0 was the first release built by CI for every platform; no build has been run by hand on a Mac.

### Deferred (design for, do not build)

- Hyphenation: a decided non-goal for v1 (§15 D399, `non-goals.md`), declined on price.
- Persistent constraints in §13's sense (live relationships between arbitrary properties, a dependency graph). Layout insets are not this.
- Real-time multiplayer: a real future goal. Operation and identity model are designed not to preclude it (§5.2, §12); the sync layer is not built.
- Plugins.
- Linked-image loading (§5.5a). Other image-feature non-goals are in §15 D191.
- Web and mobile. The exported prototype player runs in a browser (`prototyping-export.md`); the editor does not.

### Non-goals

Not a general illustration app, not pixel painting, not CAD.

## 2. Stack

Versions are pinned in the root `Cargo.toml` `[workspace.dependencies]`; pre-1.0 rendering crates are pinned exactly (`=x.y.z`).

| Concern | Crate | Notes |
|---|---|---|
| Toolchain | Rust edition 2024, stable | `rust-toolchain.toml`. `crates/mastik-web` alone builds on a pinned nightly, in its own workspace (`prototyping-export.md`) |
| Windowing | `winit` | `ApplicationHandler` API |
| GPU | `wgpu` | |
| Interactive renderer | `vello` (~0.9) | GPU compute |
| Export/CI renderer | `vello_cpu` (sparse strips, `=0.2.0`) | CPU-only, deterministic; drives PNG export without a GPU. A raster is not stable across versions of this crate (see `testing.md`) |
| Geometry | `kurbo` | `BezPath`, `Affine`, `Rect`, `Point`, `Size` |
| Paint | `peniko` | `Brush`, `Color`, `Gradient`; use the `color` crate types it re-exports |
| Text layout and editing | `parley` 0.11 (bundles `fontique`) | CPU-only. Layout, shaping, and the `Cursor`/`Selection` model behind `core::text::TextEdit`. `system` feature OFF: core never scans OS fonts |
| Container layout | `taffy` (`=0.14.0`, core only) | Flexbox and grid. `default-features = false`, features `std`, `flexbox`, `grid`, `detailed_layout_info` (a laid grid's tracks). Used through its low-level traits over Mastik's own nodes, not `TaffyTree`. Computes in `f32`; results quantized to 1/64 px (§15 D867, D873, D875) |
| Spatial index | `rstar` | R-tree |
| Maps | `rustc-hash` | `FxHashMap` |
| Serialization | `serde` + `serde_json` | Versioned JSON (§5.11); binary later behind the same API |
| Errors | `thiserror` | |
| UI chrome | `egui` + `egui-wgpu` + `egui-winit` | §9.1 |
| Font acquisition (app only) | `fontdb`, `ureq`, `dirs`, `woff2-patched` | §5.4a: system enumeration, catalog/face downloads, cache dirs, woff2→sfnt for variable web fonts. None may appear in `mastik-core` |
| Scripting (`mastik-vm` only) | `mlua` (`=0.11.6`, features `luau`, `vendored`) | Command Mode's language (`command-mode.md` §2). `vendored` compiles Luau's C++, so **building the workspace needs a C++ toolchain** (MSVC on Windows). Pinned exactly in `mastik-vm/Cargo.toml`: the sandbox and interrupt behaviour the language rests on was checked against this version |

**Two renderers, fixed roles.** GPU Vello renders the interactive canvas; `vello_cpu` renders every PNG (export, CI, headless) so exported output is deterministic across machines of one architecture and one pinned `vello_cpu`. The export path pins the SIMD level (`cpu.rs` `export_settings`) and spells out every rasterizer setting (`raster_settings`) instead of defaulting them. A stroked path is the primitive that moves when the SIMD level is merely detected. Both backends sit behind `ScenePainter` (§6). We do not depend on Linebender's renderer-abstraction layers (`imaging`, AnyRender); re-evaluate if maintaining two backends gets costly.

## 3. Workspace layout and dependency rules

```
mastik/
├─ Cargo.toml                # [workspace], shared pinned deps
├─ rust-toolchain.toml
└─ crates/
   ├─ mastik-core/            # headless engine: model, ops, history, resolve, text layout, io
   ├─ mastik-render/          # ScenePainter + Vello GPU impl + vello_cpu impl
   ├─ mastik-export/          # SVG, PDF, PNG, JPEG, WebP, JSON snapshot; code export (HTML/CSS, React, Vue); export plan and zip; the agent's page reads (`context`)
   ├─ mastik-mcp/             # MCP adapter: the wire, the tool table and schemas, the queue
   ├─ mastik-ai/              # the assistant's agent harnesses: find, seal, spawn, decode
   ├─ mastik-vm/              # Command Mode's language: Luau sandbox, bar sugar, header, completion, `.d.luau`
   ├─ mastik-play/            # the prototype play engine: played copy, play state, triggers, motion, takes
   ├─ mastik-web/             # the exported page's player (wasm); its own workspace, excluded from this one
   └─ mastik-app/             # `mastik` binary: GUI shell, tools, panels, `serve`/`export` subcommands
      └─ src/
         ├─ mcp/             # the MCP host: engine and handlers, window and `serve` hosts, endpoint, proxy, grants
         ├─ assistant/       # the assistant panel, its thread and prompt
         ├─ command/         # the command bar, its log, the VM's session host, the script editor
         └─ scripts/         # the library folder of `.luau` scripts
```

The MCP host lives in the app because its handlers commit through `EditorSession`, which is the app's (`mcp.md`, *The commit door*); `mastik-mcp` holds only what a host and a client share.

Dependency graph (acyclic):

```
mastik-core   ← (no internal deps)
mastik-render ← mastik-core
mastik-export ← mastik-core, mastik-render
mastik-mcp    ← mastik-core
mastik-ai     ← (no internal deps)
mastik-vm     ← mastik-core
mastik-play   ← mastik-core, mastik-vm (behind `scripts`)
mastik-web    ← mastik-core, mastik-render (its own workspace)
mastik-app    ← mastik-core, mastik-render, mastik-export, mastik-mcp, mastik-ai, mastik-vm
```

- `mastik-export` needs `mastik-render` because the SVG writer calls `mastik_render::effects` (`filter_matrix`, `any_ink`) so there is one implementation of the effect arithmetic (§6.4), and PNG/JPEG use the CPU renderer plus `image` (JPEG encoder) and `zip`. Consequence: an SVG-only build still pulls in `mastik-render`, `vello`, `vello_cpu` and `wgpu`. Moving those two helpers into core would make SVG core-only but would separate the colour-matrix order from the shaders it specifies; this was declined (§15 D783).
- `mastik-mcp` declares no `mastik-export` dependency; it contains no rendering or UI dependency.

### Allowed-dependency rules

- `mastik-core` MUST NOT depend on `vello*`, `wgpu`, `winit`, `egui`, or any GPU/windowing/UI crate. It must build and pass all tests with no graphics context. Permitted: `kurbo`, `peniko`, `parley` (CPU-only text layout; needed for text bounds, hit-testing and the editing cursor), `rstar`, `serde`, `serde_json`, `rustc-hash`, `thiserror`, `base64` (embedded image bytes), `roxmltree` (XML half of SVG import), `regex` (batch rename's Find), `serde_path_to_error` and `serde_ignored` (a verb's model argument refused naming where it went wrong, and an unknown field refused rather than dropped), `skrifa` and `read-fonts` (font tables read directly), `flo_curves` (boolean solver), `i_overlay` (`=`-pinned; the polygon arithmetic behind *Offset path* and *Outline stroke*), `taffy` (`=`-pinned, features as above). This list must stay set-equal to `mastik-core/Cargo.toml`; `deps_forbidden.rs` holds a copy (`CORE_PERMITTED`) and fails when the manifest differs, while the transitive tree is still checked against a denylist.
- Only `mastik-render` may depend on `vello` and `vello_cpu`. `wgpu` is shared with `mastik-app` because the GPU backend takes wgpu types and eframe owns the device.
- Only `mastik-app` may depend on `egui`/`winit`.
- `mastik-mcp`, `mastik-ai`, `mastik-vm` and `mastik-play` contain no rendering or UI dependency. `mastik-vm` alone declares `mlua`, which makes it the workspace's one C++ build (no test holds that).

### Enforcement

`mastik-core/tests/deps_forbidden.rs` inspects the transitive `cargo tree`, read `--workspace` rather than `-p` (with `resolver = "3"` these resolve differently, and every compiling gate resolves workspace-wide). It is a table, `RULES`, one row per rule a dependency tree can express: `mastik-core`, `mastik-mcp`, `mastik-ai`, `mastik-vm` and `mastik-play` against the headless list; `mastik-render` and `mastik-export` against a second, UI-only list (separate because `glutin_wgl_sys` arrives legitimately under `wgpu-hal` there). The tree is walked once per row.

The vello rule is a separate test, `only_mastik_render_declares_vello`, which reads manifests: it is about what a crate may declare, not what a tree may contain. Neither test can express the other's rule.

## 10. Milestones

M0–M6 are complete. (M6 was deliberately deferred until the editor was feature-complete, so the tool schema would not encode editor gaps.)

| Milestone | Delivers | Exit criterion |
|---|---|---|
| M0 Scaffold | Five crates with stubs, trait signatures, dependency rules, pinned versions; CI (build, clippy, fmt, forbidden-dep check) | `cargo build` and `cargo test` green; CI runs on Windows, Linux and macOS |
| M1 Core model, ops, history, IO | §5 minus Resolved; Text node model | create/delete/insert-subtree/reparent/reorder; atomic rollback on invalid transactions; undo/redo restore identical ids and state; validation rejects the §5.7 error list; save→load round-trips; re-save is byte-identical; migration stub runs |
| M2 Resolved, queries, text layout | World transforms, stroke-expanded bounds, parley layout cache, R-tree, incremental update, exact hit-testing | transforms compose through nested groups; `hit_test` exact and topmost-first; `nodes_in_view` matches brute force; property test `update ≡ rebuild`; text bounds stable under the bundled font |
| M3 Export | SVG, snapshot, PNG (CPU), no GPU anywhere, reachable from `mastik export` | fixture docs covering every NodeKind and paint type, byte-stable in CI |
| M4 GPU renderer | `VelloGpuRenderer`, shared `scene.rs`, culling, colour verification | fixture renders on a real device; pixel test validates the colour choke point; out-of-view nodes absent from the built scene; feature-gated GPU smoke suite with perceptual diff |
| M5a App shell | winit + wgpu + egui, Vello canvas, pan/zoom, selection, move/resize/rotate with preview→commit, undo/redo, save/open | multi-artboard manipulation with clean one-gesture-one-undo history; smooth pan/zoom with culling |
| M5b Tools and panels | Shape tools, pen, text tool with in-canvas editing, font picker over `FontService`, layers panel, inspector, colour editor, copy/paste/duplicate, grouping, multi-select, snapping, unsaved-change protection | a real small design can be built start to finish; an uninstalled Google font downloads, caches, applies, and still renders offline from cache |
| M6 MCP and headless | Socket endpoint, stdio proxy, `mastik serve`; Phase 1–4 tools (§8.4), on either host | an external agent can query/export against a running GUI and `mastik serve`; its edits appear live, join shared undo, and survive save/load |

## 12. Multiplayer readiness (design now, build later)

What v1 locks in so a sync layer is possible without re-architecting:

1. **Caller-minted, actor-scoped ids** (§5.2): peers create nodes concurrently without collision or coordination.
2. **Ids inside operations** (§5.7): a transaction replays deterministically on any replica.
3. **Transactions as the unit of change**: the natural sync/OT/CRDT payload.
4. **No derived state in the document**: replicas converge by replaying ops; `Resolved` is local. Bends:
   - **Components** (§5.3d, D978): an instance is a stored copy its main's edits are written into at the commit. Replay still converges, but a main edit racing an instance's creation on two peers would need a resync on merge.
   - **Kits** (§5.3e): a vendored library main changes only by the commit that accepts an update; an accept racing an instance's creation is the same resync.
   - **Styles** (§5.3f, D1062): a styled layer's list is a copy of its style's, written at the commit; a style edit racing a bind is the same resync.
   - **Variables** (§5.3f) do not bend it: a bound field stores the binding and its value is resolved at read (a bound corner's literal is written 0, not its value). A kit's vendored variables and styles are the kits' bend, not a new one.

Not designed: conflict resolution, causality/ordering metadata, presence, network protocol.

## 13. Command Mode

A keyboard-driven way to walk the node tree and edit at typing speed, through the same verbs agents use over MCP. Built: the **command bar** with two focuses, **Type** (`:`, one line of Luau over the selection, one undo step) and **Pick** (`` ` ``, the canvas has the keys: leap labels select, the arrows walk the tree), and **scripts** (the editor, the library folder, scripts stored in documents and kits). The design is in `command-mode.md`; the keys in `shortcuts.md` §15. `/ai` in the bar is a door into the assistant: a request it carries out, proposes, or answers with a script the person runs as one step.

### Decisions made

1. **One-shot evaluation, not constraints.** Expressions are evaluated at commit and forgotten; `a.width = b.width - 5` copies a value. Persistent constraints are a separate future feature (a dependency graph); do not let the syntax grow into an accidental constraint engine. §5.3c's insets are not this: a fixed layout algorithm re-run from specified values. For the same reason a script may be stored in a document but is never run by it: no run on open, no re-run on a change (§15 D1100).
2. **World-space property projection.** The language speaks `x/y/width/height/rotation/opacity/fill…` in world space (Figma inspector semantics), projected onto local transforms and geometry ops by a facade in `mastik-core` (`build::move_by_world`, `edit::turned_to`), the same machinery as MCP's `place_at_world` (§8.4).
3. **Selection-relative addressing.** The selected node is the default subject (`.width = …`); `parent`, `prev`/`next` are relative; leap-label handles address other nodes. Lookup by layer name is a disambiguated fallback only (names are not unique).
4. **The editor stays the normal mode.** The bar needs no mode: `:` opens it from the editor whenever no text entry has the keyboard, and the canvas, panels and mouse stay live under it. Its Pick focus swaps only the keymap (`Mode::Navigate`, handed to `input::resolve` by `MastikApp::keymap_mode`); the bar's head row is its sign. Text editing stays `Mode::TextInsert`.

### Where it rests

- **Mode-aware input routing** (§9.3): `input::resolve` takes the keymap's mode, so the bar's Pick focus is one more arm of the same match.
- **The world-space facade in core** (decision 2), shared by the language and MCP.
- **Leap labels are egui chrome**, painted in `draw_canvas` over everything a layer wears, not a `RenderOverrides` overlay.

## 14. Deferred decisions

Parked decisions live in `roadmap.md` under *Parked decisions*. None blocks v1, and none may become an invariant by accident. If a parked decision starts constraining the design, it has stopped being parked and wants a section of its own.

## 15. Deviations

Moved to `decisions.md`, which keeps the section number: a comment citing "§15 D123" resolves there. Where the implementation differs from this design, fix the code or record the deviation there with a reason and a keep/fix verdict.

## Traps

- The `mastik-core` allowed-dependency list is set-equality with its manifest; a name listed but no longer declared bakes a permission for nothing.
- `--workspace` and `-p` resolve different feature sets (`resolver = "3"`), so dependency checks and anything pinned by type size depend on the spelling used.
- SVG export needs `mastik-render` (effects maths); budgeting an SVG-only build for core alone is wrong.
- `wgpu` is legitimately in `mastik-app`'s tree; the vello-only-in-render rule is enforced by manifest scan, not tree walk, because app and export reach vello through render.
- `vello_cpu` pins matter: a version bump moves stroked rasters; nothing else should move.
