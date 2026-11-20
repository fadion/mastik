//! The prototype's HTML export: one file a client opens with a double-click
//! (`docs/design/prototyping-export.md`).
//!
//! - [`prune`] trims a document to the pages its chosen flows reach, with what
//!   those pages use, as a document the loader takes.
//! - [`Parts`] gathers what goes in: the player (`mastik-web`'s wasm and its
//!   glue), the pruned document, and every font file its text was laid out in.
//! - [`Parts::estimate`] is the file's size before writing; [`Parts::html`]
//!   writes it: the parts in one gzip stream, base64'd once into the page and
//!   unpacked there by `DecompressionStream`, the wasm instantiated from those
//!   bytes, so the file runs from `file://`.

use mastik_core::prototype::{FlowRef, PlayExport};
use mastik_core::take::{Keep, take};
use mastik_core::{Document, NodeId, Operation, Resolved, Transaction};
use std::collections::{BTreeSet, HashSet};
use std::io::Write;

/// The browser player as `mastik-web`'s build leaves it: the wasm and the
/// `wasm-bindgen --target no-modules` script that loads it.
#[derive(Clone, Copy, Debug)]
pub struct Player<'a> {
    pub wasm: &'a [u8],
    pub glue: &'a str,
}

/// Why a prototype cannot be exported.
#[derive(Debug, PartialEq)]
pub enum PackError {
    /// No chosen flow exists: the document has none, or every one the export
    /// settings name is gone.
    NoFlows,
    /// The trim was refused; the document's own rules, named.
    Prune(String),
    /// The pruned document did not save.
    Save(String),
}

impl std::fmt::Display for PackError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            PackError::NoFlows => f.write_str("the document has no flow to export"),
            PackError::Prune(e) => write!(f, "cannot trim the document to its flows: {e}"),
            PackError::Save(e) => write!(f, "cannot save the trimmed document: {e}"),
        }
    }
}

impl std::error::Error for PackError {}

/// The flows the export takes, in page order: [`PlayExport::flows`] where they
/// still exist, every flow when it names none. A named flow that is gone is
/// skipped, as play skips a missing target.
pub fn chosen_flows(doc: &Document) -> Vec<FlowRef> {
    let every = doc.pages().iter().flat_map(|p| {
        p.flows.iter().map(|f| FlowRef {
            page: p.id,
            flow: f.id,
        })
    });
    let named = &doc.play_export().flows;
    every
        .filter(|r| named.is_empty() || named.contains(r))
        .collect()
}

/// The pages a prototype of `flows` can show, in document order: each flow's own
/// page and its start's, then every page a layer on a reached page navigates to,
/// opens or swaps an overlay from, scrolls to or switches a state to, and every
/// page holding the main of an instance on a reached page. A target that dangles
/// reaches nothing.
pub fn reached_pages(doc: &Document, flows: &[FlowRef]) -> Vec<NodeId> {
    let mut reached: HashSet<NodeId> = HashSet::new();
    let mut queue: Vec<NodeId> = Vec::new();
    let mut reach = |page: Option<NodeId>, queue: &mut Vec<NodeId>| {
        if let Some(page) = page
            && reached.insert(page)
        {
            queue.push(page);
        }
    };
    for r in flows {
        let Some(page) = doc.page(r.page) else {
            continue;
        };
        reach(Some(r.page), &mut queue);
        if let Some(flow) = page.flows.iter().find(|f| f.id == r.flow) {
            reach(doc.page_of(flow.value.start), &mut queue);
        }
    }
    while let Some(page) = queue.pop() {
        let mut stack = vec![page];
        while let Some(id) = stack.pop() {
            let Some(node) = doc.get(id) else { continue };
            stack.extend(node.children().iter().copied());
            if let Some(main) = node.link() {
                reach(doc.page_of(main), &mut queue);
            }
            for reaction in node.reactions() {
                for action in &reaction.value.actions {
                    for target in action.value.layer_targets() {
                        let target = mastik_core::prototype::resolve_target(doc, id, target);
                        reach(doc.page_of(target), &mut queue);
                    }
                }
            }
        }
    }
    doc.page_ids().filter(|p| reached.contains(p)).collect()
}

/// `doc` trimmed to what a prototype of its chosen flows shows: the
/// [`reached_pages`] whole, with every style, variable, script and vendored main
/// (document-wide, so kept whole), and only the pictures those pages show. Each
/// kept page keeps only its chosen flows, and the export settings name them and
/// the flow the file opens on.
///
/// Trimmed through `take`, the door an import's trim uses, so the result passes
/// every rule a loaded document does.
pub fn prune(doc: &Document) -> Result<Document, PackError> {
    let flows = chosen_flows(doc);
    let opens = doc
        .play_export()
        .opens
        .filter(|o| flows.contains(o))
        .or_else(|| flows.first().copied())
        .ok_or(PackError::NoFlows)?;
    let pages = reached_pages(doc, &flows);
    let mut out = doc.clone();
    let mut ops: Vec<Operation> = pages
        .iter()
        .filter_map(|p| {
            let page = doc.page(*p)?;
            let kept: Vec<_> = page
                .flows
                .iter()
                .filter(|f| {
                    flows.contains(&FlowRef {
                        page: *p,
                        flow: f.id,
                    })
                })
                .cloned()
                .collect();
            (kept.len() != page.flows.len()).then_some(Operation::SetFlows {
                id: *p,
                flows: kept,
            })
        })
        .collect();
    ops.push(Operation::SetPlayExport {
        settings: PlayExport {
            flows,
            opens: Some(opens),
            ..doc.play_export().clone()
        },
    });
    out.apply(&Transaction(ops))
        .map_err(|e| PackError::Prune(e.to_string()))?;
    let keep = Keep {
        pages,
        ..Keep::all(&out)
    };
    take(&mut out, &keep).map_err(|e| PackError::Prune(e.to_string()))?;
    Ok(out)
}

/// One font file as the page registers it.
#[derive(Clone, Debug, PartialEq)]
pub struct FontFile {
    pub bytes: Vec<u8>,
    /// The family the canvas registered it under, when not its own name.
    pub alias: Option<String>,
}

/// Every font file `doc`'s text was laid out in, once each, as the file the
/// layout drew from: whole, whatever its licence, since a viewer types into
/// text fields. Fallbacks count, since a run is the face that drew it; the
/// bundled Inter does not, since the player carries it.
pub fn fonts_used(doc: &Document, res: &Resolved) -> Vec<FontFile> {
    let mut seen = HashSet::new();
    let mut out = Vec::new();
    let ids: BTreeSet<NodeId> = doc.node_ids().collect();
    for layout in ids.iter().filter_map(|id| res.text_layout(*id)) {
        for run in &layout.runs {
            if mastik_core::text::is_bundled(&run.font) || !seen.insert(run.font.data.id()) {
                continue;
            }
            out.push(FontFile {
                bytes: run.font.data.data().to_vec(),
                alias: mastik_core::text::font_alias(&run.font),
            });
        }
    }
    out
}

/// The longest CSS a layer carries for inspect mode. A frame's is its whole
/// subtree's rules, so a large screen's would weigh more than it is worth to
/// someone inspecting; past this the page says to copy it from the app.
pub const INSPECT_CSS_MAX: usize = 32 * 1024;

/// Each layer's CSS as *Copy as CSS* writes it ([`crate::code::css_of`]), for
/// every layer on `doc`'s pages but the pages themselves, in document order. A
/// layer whose CSS is longer than [`INSPECT_CSS_MAX`] is left out.
pub fn inspect_css(doc: &Document, res: &Resolved) -> Vec<(NodeId, String)> {
    let pages: Vec<NodeId> = doc.page_ids().collect();
    let layers = mastik_core::subtree_nodes(doc, &pages);
    layers
        .into_iter()
        .filter(|id| !pages.contains(id))
        .filter_map(|id| {
            let css = crate::code::css_of(doc, res, &[id]);
            (!css.is_empty() && css.len() <= INSPECT_CSS_MAX).then_some((id, css))
        })
        .collect()
}

/// One file a layer's export settings write, for inspect mode to offer.
#[derive(Clone, Debug, PartialEq)]
pub struct InspectFile {
    pub layer: NodeId,
    /// Its name as the export would write it, folders included.
    pub path: String,
    pub bytes: Vec<u8>,
}

/// Every file the layers' own export settings write ([`crate::plan`]), rendered
/// now, since the page carries no exporter.
pub fn inspect_files(doc: &Document, res: &Resolved) -> Vec<InspectFile> {
    let pages: Vec<NodeId> = doc.page_ids().collect();
    let mut out = Vec::new();
    for id in mastik_core::subtree_nodes(doc, &pages) {
        if doc.get(id).is_none_or(|n| n.exports().is_empty()) {
            continue;
        }
        for file in crate::plan::plan(doc, res, &[id], Default::default()) {
            if let Some(bytes) = crate::plan::render(doc, res, &file) {
                out.push(InspectFile {
                    layer: id,
                    path: file.path,
                    bytes,
                });
            }
        }
    }
    out
}

/// What one exported file holds, before it is packed.
#[derive(Clone, Debug)]
pub struct Parts<'a> {
    pub player: Player<'a>,
    /// The pruned document, saved.
    pub document: Vec<u8>,
    pub fonts: Vec<FontFile>,
    /// The page's title: the flow the file opens on.
    pub title: String,
    /// What inspect mode shows, written here because the page carries no
    /// exporter: [`inspect_css`] and [`inspect_files`].
    pub css: Vec<(NodeId, String)>,
    pub files: Vec<InspectFile>,
}

impl<'a> Parts<'a> {
    /// [`prune`] `doc`, lay the result out and gather its fonts and what inspect
    /// mode shows. Fonts are those registered with this thread's shaping engine,
    /// so the caller registers the document's families first, as the canvas has.
    pub fn of(doc: &Document, player: Player<'a>) -> Result<Self, PackError> {
        let mut pruned = prune(doc)?;
        let res = Resolved::settled(&mut pruned);
        let fonts = fonts_used(&pruned, &res);
        let css = inspect_css(&pruned, &res);
        let files = inspect_files(&pruned, &res);
        let title = pruned
            .play_export()
            .opens
            .and_then(|o| {
                let page = pruned.page(o.page)?;
                let flow = page.flows.iter().find(|f| f.id == o.flow)?;
                Some(flow.value.name.clone())
            })
            .unwrap_or_default();
        let document =
            mastik_core::io::save(&pruned).map_err(|e| PackError::Save(e.to_string()))?;
        Ok(Self {
            player,
            document,
            fonts,
            title,
            css,
            files,
        })
    }

    /// The file's size in bytes before it is written, for the export dialog.
    /// Packed at gzip's fastest level, which the file's own level only improves
    /// on, so the answer is a little over, never under.
    pub fn estimate(&self) -> u64 {
        self.page_len(self.stream(flate2::Compression::fast()).len())
    }

    /// The HTML file.
    pub fn html(&self) -> String {
        let stream = self.stream(flate2::Compression::best());
        let payload = base64_encode(&stream);
        let mut page = String::with_capacity(self.page_len(stream.len()) as usize);
        let glue = script_safe(self.player.glue);
        page.push_str(HEAD);
        page.push_str("<title>");
        page.push_str(&html_text(&self.title));
        page.push_str("</title>\n");
        page.push_str(STYLE);
        page.push_str("</head>\n<body>\n");
        page.push_str(BODY);
        page.push_str("<script id=\"payload\" type=\"application/octet-stream\">");
        page.push_str(&payload);
        page.push_str("</script>\n<script>\n");
        page.push_str(&glue);
        page.push_str("\n</script>\n<script>\n");
        page.push_str(BOOT);
        page.push_str("</script>\n</body>\n</html>\n");
        page
    }

    /// The page's length around a stream of `stream` bytes.
    fn page_len(&self, stream: usize) -> u64 {
        let fixed = HEAD.len()
            + STYLE.len()
            + BODY.len()
            + BOOT.len()
            + script_safe(self.player.glue).len()
            + html_text(&self.title).len()
            + 200;
        (fixed + stream.div_ceil(3) * 4) as u64
    }

    /// The parts in the container the page unpacks, gzipped.
    fn stream(&self, level: flate2::Compression) -> Vec<u8> {
        let mut gz = flate2::write::GzEncoder::new(Vec::new(), level);
        let container = container(self);
        gz.write_all(&container)
            .expect("writing to a Vec cannot fail");
        gz.finish().expect("writing to a Vec cannot fail")
    }
}

/// The container's first bytes, which the page checks before reading on.
pub const MAGIC: &[u8; 8] = b"MSTKPLAY";
/// The container's layout version, after [`MAGIC`].
pub const CONTAINER_VERSION: u32 = 1;

/// One entry's kind in the container.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(u8)]
pub enum Entry {
    Wasm = 1,
    Document = 2,
    /// A font file; its name is the alias, empty for none.
    Font = 3,
    /// A layer's CSS for inspect mode; its name is the layer's id.
    Css = 4,
    /// A file a layer's export settings write; its name is the layer's id, a
    /// newline, and the file's path.
    File = 5,
}

/// The parts laid end to end: [`MAGIC`], [`CONTAINER_VERSION`] as `u32` LE, then
/// per entry its [`Entry`] byte, its name's length (`u32` LE) and UTF-8, its
/// data's length (`u32` LE) and bytes. The page's `unpack` reads exactly this.
fn container(parts: &Parts<'_>) -> Vec<u8> {
    let mut out = Vec::new();
    out.extend_from_slice(MAGIC);
    out.extend_from_slice(&CONTAINER_VERSION.to_le_bytes());
    let mut entry = |kind: Entry, name: &str, data: &[u8]| {
        out.push(kind as u8);
        out.extend_from_slice(&(name.len() as u32).to_le_bytes());
        out.extend_from_slice(name.as_bytes());
        out.extend_from_slice(&(data.len() as u32).to_le_bytes());
        out.extend_from_slice(data);
    };
    entry(Entry::Wasm, "", parts.player.wasm);
    entry(Entry::Document, "", &parts.document);
    for font in &parts.fonts {
        entry(
            Entry::Font,
            font.alias.as_deref().unwrap_or(""),
            &font.bytes,
        );
    }
    for (id, css) in &parts.css {
        entry(Entry::Css, &id.to_wire(), css.as_bytes());
    }
    for file in &parts.files {
        let name = format!("{}\n{}", file.layer.to_wire(), file.path);
        entry(Entry::File, &name, &file.bytes);
    }
    out
}

/// Text made safe inside an element.
fn html_text(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
}

/// A script made safe inside `<script>`: no `</script` may close it early. It
/// can only appear inside a string literal, where `<\/` reads the same.
fn script_safe(js: &str) -> String {
    let lower = js.to_ascii_lowercase();
    let mut out = String::with_capacity(js.len());
    let mut last = 0;
    for (at, _) in lower.match_indices("</script") {
        out.push_str(&js[last..at]);
        out.push_str("<\\/");
        last = at + 2;
    }
    out.push_str(&js[last..]);
    out
}

fn base64_encode(bytes: &[u8]) -> String {
    use base64::Engine;
    base64::engine::general_purpose::STANDARD.encode(bytes)
}

const HEAD: &str = "<!doctype html>\n<html lang=\"en\">\n<head>\n<meta charset=\"utf-8\">\n\
<meta name=\"viewport\" content=\"width=device-width, initial-scale=1\">\n\
<meta name=\"generator\" content=\"Mastik\">\n";

/// The page's look: the stage the screen is placed on (scrolling when the screen
/// is larger), the device frame behind it, the hints canvas over it, inspect
/// mode's panel beside it, and the player bar along the foot.
const STYLE: &str = "<style>
html, body { margin: 0; height: 100%; background: #1e1e1e; color: #e6e6e6;
  font: 13px system-ui, sans-serif; }
body { display: flex; flex-direction: column; overflow: hidden; }
#main { flex: 1; min-height: 0; display: flex; }
#stage { flex: 1; min-width: 0; overflow: auto; }
#inspect { flex: none; width: 320px; overflow: auto; padding: 12px; box-sizing: border-box;
  background: #252527; border-left: 1px solid #333336; }
#inspect[hidden] { display: none; }
#inspect h2 { margin: 0 0 8px; font-size: 13px; font-weight: 600; overflow-wrap: anywhere; }
#inspect h3 { margin: 16px 0 6px; font-size: 11px; font-weight: 600; color: #9a9aa0;
  text-transform: uppercase; letter-spacing: 0.04em; display: flex; align-items: center; }
#inspect h3 button { margin-left: auto; text-transform: none; letter-spacing: 0; }
#inspect pre { margin: 0; padding: 8px; border-radius: 5px; background: #1b1b1d;
  font: 12px ui-monospace, Consolas, monospace; white-space: pre-wrap; overflow-wrap: anywhere; }
#inspect p { margin: 0; color: #9a9aa0; }
#inspect a { display: block; padding: 4px 0; color: #63b3f5; }
#inspect button { height: 22px; padding: 0 8px; border: 0; border-radius: 5px;
  background: #333336; color: #e6e6e6; font: inherit; font-size: 12px; cursor: pointer; }
#room { position: relative; }
#device { position: absolute; background: #0d0d0e;
  box-shadow: 0 0 0 1px #3a3a3d, 0 16px 48px rgba(0, 0, 0, 0.5); }
#screen, #hints { position: absolute; display: block; }
#screen { background: #fff; touch-action: none; }
#hints { pointer-events: none; }
#bar { flex: none; display: flex; align-items: center; gap: 6px; height: 40px;
  padding: 0 12px; background: #252527; border-top: 1px solid #333336; }
#bar[hidden] { display: none; }
#name { flex: 1; min-width: 0; overflow: hidden; text-overflow: ellipsis; white-space: nowrap; }
#bar button, #bar select { height: 26px; padding: 0 10px; border: 0; border-radius: 5px;
  background: #333336; color: #e6e6e6; font: inherit; cursor: pointer; }
#bar button:hover, #bar select:hover { background: #3d3d41; }
#bar button[aria-pressed=\"true\"] { background: #2b5f8f; }
#status, #toast { position: fixed; inset: auto 0 56px 0; text-align: center; pointer-events: none; }
#toast span { padding: 6px 12px; border-radius: 6px; background: #333336; }
#toast[hidden] { display: none; }
</style>
";

const BODY: &str = "<div id=\"main\"><main id=\"stage\"><div id=\"room\">\
<div id=\"device\" hidden></div>\
<canvas id=\"screen\" width=\"1\" height=\"1\"></canvas>\
<canvas id=\"hints\" width=\"1\" height=\"1\"></canvas></div></main>\
<aside id=\"inspect\" hidden></aside></div>\n\
<nav id=\"bar\" hidden><span id=\"name\"></span><select id=\"flows\" hidden \
aria-label=\"Flow\"></select><button id=\"restart\" type=\"button\">Restart</button>\
<button id=\"hint-switch\" type=\"button\" aria-pressed=\"false\" hidden>Hints</button>\
<button id=\"inspect-switch\" type=\"button\" aria-pressed=\"false\">Inspect</button></nav>\n\
<div id=\"status\">Opening the prototype…</div>\n<div id=\"toast\" hidden><span></span></div>\n";

/// Unpacks the payload, loads the player from its bytes, registers the fonts and
/// plays: each animation frame ticks the engine by the time since the last and
/// draws, and stops asking for frames while nothing moves and no input came. The
/// canvas's pointer, wheel and the window's keys go to the player in CSS pixels
/// from the canvas; an address play opens goes to a new tab from the input that
/// asked for it, while the browser still counts it the viewer's act. `?cpu` in
/// the address forces the CPU renderer. The outcome is left on `<body>` as
/// `data-backend` or `data-error`, for whoever checks the page.
const BOOT: &str = r##"(async () => {
  const $ = id => document.getElementById(id);
  const status = $("status");
  const canvas = $("screen");
  try {
    if (typeof DecompressionStream === "undefined") {
      throw new Error("This browser cannot open the prototype: it lacks DecompressionStream.");
    }
    const text = document.getElementById("payload").textContent;
    let packed;
    if (Uint8Array.fromBase64) {
      packed = Uint8Array.fromBase64(text);
    } else {
      const raw = atob(text);
      packed = new Uint8Array(raw.length);
      for (let i = 0; i < raw.length; i++) packed[i] = raw.charCodeAt(i);
    }
    const stream = new Blob([packed]).stream().pipeThrough(new DecompressionStream("gzip"));
    const bytes = new Uint8Array(await new Response(stream).arrayBuffer());
    const parts = unpack(bytes);
    await wasm_bindgen({ module_or_path: parts.wasm });
    for (const font of parts.fonts) wasm_bindgen.add_font(font.bytes, font.alias || undefined);
    const cpu = new URLSearchParams(location.search).has("cpu");
    const player = await wasm_bindgen.start(canvas, parts.document, cpu);
    play(player, parts);
    status.remove();
    document.body.dataset.backend = player.backend();
  } catch (e) {
    status.textContent = String(e && e.message || e);
    document.body.dataset.error = String(e && e.message || e);
    console.error(e);
  }

  function play(player, parts) {
    const stage = $("stage"), room = $("room"), device = $("device");
    const hints = $("hints"), hintCtx = hints.getContext("2d");
    const bar = $("bar"), flowList = $("flows"), hintSwitch = $("hint-switch");
    const panel = $("inspect"), inspectSwitch = $("inspect-switch");
    let last = null, queued = false, hintsOn = false, hintsShown = false, placed = "";
    // Inspect mode: interactions stop, and a click picks the layer under it.
    let inspecting = false, picked = "", over = "";

    const wake = () => {
      if (!queued) { queued = true; requestAnimationFrame(frame); }
    };
    function frame(t) {
      queued = false;
      const ms = last === null ? 0 : t - last;
      const p = player.frame(ms, stage.clientWidth, stage.clientHeight, devicePixelRatio);
      place(p);
      drawHints(p[10]);
      if (p[9]) { last = t; wake(); } else last = null;
    }
    function place(p) {
      const [, sw, sh, x, y, w, h, b, r] = p;
      const key = p.slice(1, 9).join() + "|" + devicePixelRatio;
      if (key === placed) return;
      placed = key;
      const px = v => v + "px";
      room.style.width = px(sw);
      room.style.height = px(sh);
      for (const c of [canvas, hints]) {
        Object.assign(c.style, { left: px(x), top: px(y), width: px(w), height: px(h), borderRadius: px(r) });
      }
      hints.width = Math.max(1, Math.round(w * devicePixelRatio));
      hints.height = Math.max(1, Math.round(h * devicePixelRatio));
      hintsShown = true;
      device.hidden = b === 0;
      Object.assign(device.style, {
        left: px(x - b), top: px(y - b), width: px(w + 2 * b), height: px(h + 2 * b), borderRadius: px(r + b),
      });
    }
    function drawHints(flash) {
      if (inspecting) return drawInspect();
      const alpha = player.hotspot_hints() ? (hintsOn ? 1 : flash) : 0;
      if (alpha <= 0 && !hintsShown) return;
      const d = devicePixelRatio;
      hintCtx.clearRect(0, 0, hints.width, hints.height);
      hintsShown = alpha > 0;
      if (!hintsShown) return;
      const r = player.hotspots();
      hintCtx.lineWidth = 2 * d;
      hintCtx.fillStyle = `rgba(99, 179, 245, ${0.2 * alpha})`;
      hintCtx.strokeStyle = `rgba(99, 179, 245, ${alpha})`;
      for (let i = 0; i < r.length; i += 4) {
        hintCtx.fillRect(r[i] * d, r[i + 1] * d, r[i + 2] * d, r[i + 3] * d);
        hintCtx.strokeRect(r[i] * d + d, r[i + 1] * d + d, r[i + 2] * d - 2 * d, r[i + 3] * d - 2 * d);
      }
    }
    // The picked layer's box, the hovered one's, and the distances between them
    // (or to the picked layer's parent) as the canvas's Alt measure draws them.
    function drawInspect() {
      const d = devicePixelRatio, c = hintCtx;
      c.clearRect(0, 0, hints.width, hints.height);
      hintsShown = true;
      const box = (id, width, dash) => {
        const b = id ? player.layer_box(id) : [];
        if (b.length !== 4) return;
        c.lineWidth = width * d;
        c.setLineDash(dash.map(v => v * d));
        c.strokeStyle = "#63b3f5";
        c.strokeRect(b[0] * d, b[1] * d, b[2] * d, b[3] * d);
      };
      if (over !== picked) box(over, 1, [4, 3]);
      box(picked, 2, []);
      c.setLineDash([]);
      if (!picked) return;
      c.font = `${11 * d}px system-ui, sans-serif`;
      c.textAlign = "center";
      c.textBaseline = "middle";
      for (const m of player.measures(picked, over === picked ? "" : over)) {
        const [axis, at, from, to, label] = m.split("|");
        const [a, f, t] = [Number(at) * d, Number(from) * d, Number(to) * d];
        const [x0, y0, x1, y1] = axis === "h" ? [f, a, t, a] : [a, f, a, t];
        c.strokeStyle = c.fillStyle = "#f26d6d";
        c.lineWidth = d;
        c.beginPath();
        c.moveTo(x0, y0);
        c.lineTo(x1, y1);
        c.stroke();
        const [mx, my] = [(x0 + x1) / 2, (y0 + y1) / 2];
        const w = c.measureText(label).width + 8 * d, h = 15 * d;
        c.beginPath();
        c.roundRect(mx - w / 2, my - h / 2, w, h, 4 * d);
        c.fill();
        c.fillStyle = "#2b0c0c";
        c.fillText(label, mx, my);
      }
    }
    // The panel for the picked layer: its name, its CSS as Copy as CSS writes
    // it, and the files its export settings write.
    function showPanel() {
      panel.replaceChildren();
      const el = (tag, text) => {
        const e = document.createElement(tag);
        if (text !== undefined) e.textContent = text;
        panel.append(e);
        return e;
      };
      if (!picked) {
        el("p", "Click a layer to inspect it.");
        return;
      }
      el("h2", player.layer_name(picked));
      const css = parts.css.get(picked);
      const head = el("h3", "CSS");
      if (css) {
        const copy = document.createElement("button");
        copy.type = "button";
        copy.textContent = "Copy";
        copy.addEventListener("click", () => navigator.clipboard.writeText(css).then(
          () => toast("Copied the CSS"), () => toast("The browser refused the clipboard")));
        head.append(copy);
        el("pre", css);
      } else {
        el("p", "Too long to carry in the file; copy it from Mastik.");
      }
      const files = parts.files.get(picked) || [];
      el("h3", "Export");
      if (!files.length) el("p", "No export settings.");
      for (const f of files) {
        const a = el("a", f.path);
        a.href = URL.createObjectURL(new Blob([f.bytes]));
        a.download = f.path.split("/").pop();
      }
    }
    function setInspecting(on) {
      inspecting = on;
      picked = over = "";
      inspectSwitch.setAttribute("aria-pressed", String(on));
      panel.hidden = !on;
      canvas.style.cursor = on ? "crosshair" : "";
      if (on) { player.pointer_leave(); showPanel(); }
      placed = "";
      after();
    }
    // Opened here, inside the event that asked, so the browser lets the tab open.
    function after() {
      for (const url of player.take_urls()) open(url, "_blank", "noopener");
      for (const n of player.take_notices()) {
        if (n.startsWith("!")) toast(n.slice(1)); else console.warn(n);
      }
      wake();
    }
    let toastTimer = 0;
    function toast(text) {
      const t = $("toast");
      t.firstChild.textContent = text;
      t.hidden = false;
      clearTimeout(toastTimer);
      toastTimer = setTimeout(() => { t.hidden = true; }, 4000);
    }
    const local = e => {
      const r = canvas.getBoundingClientRect();
      return [e.clientX - r.left, e.clientY - r.top];
    };

    canvas.addEventListener("pointerdown", e => {
      if (e.button !== 0) return;
      e.preventDefault();
      if (inspecting) {
        picked = player.layer_at(...local(e));
        showPanel();
        return after();
      }
      canvas.setPointerCapture(e.pointerId);
      player.pointer_down(...local(e));
      after();
    });
    canvas.addEventListener("pointermove", e => {
      const [x, y] = local(e);
      if (inspecting) {
        over = player.layer_at(x, y);
        return after();
      }
      player.pointer_move(x, y);
      canvas.style.cursor = player.over_hotspot(x, y) ? "pointer" : "";
      after();
    });
    canvas.addEventListener("pointerup", e => {
      if (e.button !== 0 || inspecting) return;
      player.pointer_up(...local(e));
      after();
    });
    canvas.addEventListener("pointercancel", () => { player.pointer_leave(); after(); });
    canvas.addEventListener("pointerleave", e => {
      if (inspecting) { over = ""; return after(); }
      if (canvas.hasPointerCapture(e.pointerId)) return;
      player.pointer_leave();
      after();
    });
    canvas.addEventListener("wheel", e => {
      const [x, y] = local(e);
      if (player.wheel(x, y, e.deltaX, e.deltaY, e.deltaMode, stage.clientHeight)) e.preventDefault();
      after();
    }, { passive: false });
    addEventListener("keydown", e => {
      if (e.ctrlKey || e.altKey || e.metaKey || e.isComposing) return;
      if (e.target instanceof Element && (bar.contains(e.target) || panel.contains(e.target))) return;
      if (inspecting) {
        if (e.key === "Escape") { picked = ""; showPanel(); after(); }
        return;
      }
      if (player.key(e.key, e.shiftKey)) e.preventDefault();
      after();
    });
    addEventListener("resize", wake);

    if (player.player_bar()) {
      bar.hidden = false;
      const flows = player.flows();
      if (flows.length > 1) {
        flows.forEach((name, i) => flowList.add(new Option(name, String(i))));
        flowList.value = String(player.flow());
        flowList.hidden = false;
      } else {
        $("name").textContent = flows[0] || "";
      }
      flowList.addEventListener("change", () => {
        player.play_flow(Number(flowList.value));
        flowList.blur();
        after();
      });
      $("restart").addEventListener("click", e => {
        player.restart();
        e.currentTarget.blur();
        after();
      });
      hintSwitch.hidden = !player.hotspot_hints();
      hintSwitch.addEventListener("click", () => {
        hintsOn = !hintsOn;
        hintSwitch.setAttribute("aria-pressed", String(hintsOn));
        hintSwitch.blur();
        wake();
      });
      inspectSwitch.addEventListener("click", () => {
        setInspecting(!inspecting);
        inspectSwitch.blur();
      });
    }
    wake();
  }

  function unpack(bytes) {
    const magic = new TextDecoder().decode(bytes.subarray(0, 8));
    const view = new DataView(bytes.buffer, bytes.byteOffset, bytes.byteLength);
    if (magic !== "MSTKPLAY" || view.getUint32(8, true) !== 1) {
      throw new Error("The prototype's data is damaged or from another version.");
    }
    const out = { wasm: null, document: null, fonts: [], css: new Map(), files: new Map() };
    let at = 12;
    const take = () => {
      const len = view.getUint32(at, true);
      const part = bytes.subarray(at + 4, at + 4 + len);
      at += 4 + len;
      return part;
    };
    while (at < bytes.length) {
      const kind = bytes[at++];
      const name = new TextDecoder().decode(take());
      const data = take();
      if (kind === 1) out.wasm = data;
      else if (kind === 2) out.document = data;
      else if (kind === 3) out.fonts.push({ bytes: data, alias: name });
      else if (kind === 4) out.css.set(name, new TextDecoder().decode(data));
      else if (kind === 5) {
        const [layer, path] = name.split("\n");
        if (!out.files.has(layer)) out.files.set(layer, []);
        out.files.get(layer).push({ path, bytes: data });
      }
    }
    return out;
  }
})();
"##;
