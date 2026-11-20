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

/// What one exported file holds, before it is packed.
#[derive(Clone, Debug)]
pub struct Parts<'a> {
    pub player: Player<'a>,
    /// The pruned document, saved.
    pub document: Vec<u8>,
    pub fonts: Vec<FontFile>,
    /// The page's title: the flow the file opens on.
    pub title: String,
}

impl<'a> Parts<'a> {
    /// [`prune`] `doc`, lay the result out and gather its fonts. Fonts are those
    /// registered with this thread's shaping engine, so the caller registers the
    /// document's families first, as the canvas has.
    pub fn of(doc: &Document, player: Player<'a>) -> Result<Self, PackError> {
        let mut pruned = prune(doc)?;
        let res = Resolved::settled(&mut pruned);
        let fonts = fonts_used(&pruned, &res);
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
/// is larger), the device frame behind it, the hints canvas over it, and the
/// player bar along the foot.
const STYLE: &str = "<style>
html, body { margin: 0; height: 100%; background: #1e1e1e; color: #e6e6e6;
  font: 13px system-ui, sans-serif; }
body { display: flex; flex-direction: column; overflow: hidden; }
#stage { flex: 1; min-height: 0; overflow: auto; }
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

const BODY: &str = "<main id=\"stage\"><div id=\"room\"><div id=\"device\" hidden></div>\
<canvas id=\"screen\" width=\"1\" height=\"1\"></canvas>\
<canvas id=\"hints\" width=\"1\" height=\"1\"></canvas></div></main>\n\
<nav id=\"bar\" hidden><span id=\"name\"></span><select id=\"flows\" hidden \
aria-label=\"Flow\"></select><button id=\"restart\" type=\"button\">Restart</button>\
<button id=\"hint-switch\" type=\"button\" aria-pressed=\"false\" hidden>Hints</button></nav>\n\
<div id=\"status\">Opening the prototype…</div>\n<div id=\"toast\" hidden><span></span></div>\n";

/// Unpacks the payload, loads the player from its bytes, registers the fonts and
/// plays: each animation frame ticks the engine by the time since the last and
/// draws, and stops asking for frames while nothing moves and no input came. The
/// canvas's pointer, wheel and the window's keys go to the player in CSS pixels
/// from the canvas; an address play opens goes to a new tab from the input that
/// asked for it, while the browser still counts it the viewer's act. `?cpu` in
/// the address forces the CPU renderer. The outcome is left on `<body>` as
/// `data-backend` or `data-error`, for whoever checks the page.
const BOOT: &str = r#"(async () => {
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
    play(player);
    status.remove();
    document.body.dataset.backend = player.backend();
  } catch (e) {
    status.textContent = String(e && e.message || e);
    document.body.dataset.error = String(e && e.message || e);
    console.error(e);
  }

  function play(player) {
    const stage = $("stage"), room = $("room"), device = $("device");
    const hints = $("hints"), hintCtx = hints.getContext("2d");
    const bar = $("bar"), flowList = $("flows"), hintSwitch = $("hint-switch");
    let last = null, queued = false, hintsOn = false, hintsShown = false, placed = "";

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
      canvas.setPointerCapture(e.pointerId);
      player.pointer_down(...local(e));
      e.preventDefault();
      after();
    });
    canvas.addEventListener("pointermove", e => {
      const [x, y] = local(e);
      player.pointer_move(x, y);
      canvas.style.cursor = player.over_hotspot(x, y) ? "pointer" : "";
      after();
    });
    canvas.addEventListener("pointerup", e => {
      if (e.button !== 0) return;
      player.pointer_up(...local(e));
      after();
    });
    canvas.addEventListener("pointercancel", () => { player.pointer_leave(); after(); });
    canvas.addEventListener("pointerleave", e => {
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
      if (e.target instanceof Element && bar.contains(e.target)) return;
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
    }
    wake();
  }

  function unpack(bytes) {
    const magic = new TextDecoder().decode(bytes.subarray(0, 8));
    const view = new DataView(bytes.buffer, bytes.byteOffset, bytes.byteLength);
    if (magic !== "MSTKPLAY" || view.getUint32(8, true) !== 1) {
      throw new Error("The prototype's data is damaged or from another version.");
    }
    const out = { wasm: null, document: null, fonts: [] };
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
    }
    return out;
  }
})();
"#;
