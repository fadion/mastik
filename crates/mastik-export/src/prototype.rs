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
    /// The frame a mirror starts from is gone, or is not a page's frame.
    NoFrame,
    /// The trim was refused; the document's own rules, named.
    Prune(String),
    /// The pruned document did not save.
    Save(String),
}

impl std::fmt::Display for PackError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            PackError::NoFlows => f.write_str("the document has no flow to export"),
            PackError::NoFrame => f.write_str("the frame playing is gone"),
            PackError::Prune(e) => write!(f, "cannot trim the document to its flows: {e}"),
            PackError::Save(e) => write!(f, "cannot save the trimmed document: {e}"),
        }
    }
}

impl std::error::Error for PackError {}

/// The flows the export takes, in page order: [`PlayExport::flows`] where they
/// still exist, every flow when it names none that does. A named flow that is
/// gone is skipped, as play skips a missing target, and deleting a flow leaves
/// the list as it was, so a list naming only gone flows names none.
pub fn chosen_flows(doc: &Document) -> Vec<FlowRef> {
    let every: Vec<FlowRef> = doc
        .pages()
        .iter()
        .flat_map(|p| {
            p.flows.iter().map(|f| FlowRef {
                page: p.id,
                flow: f.id,
            })
        })
        .collect();
    let named = &doc.play_export().flows;
    let chosen: Vec<FlowRef> = every
        .iter()
        .filter(|r| named.contains(r))
        .copied()
        .collect();
    match chosen.is_empty() {
        true => every,
        false => chosen,
    }
}

/// The pages a prototype of `flows` can show, in document order: each flow's own
/// page and its start's, then every page a layer on a reached page navigates to,
/// opens or swaps an overlay from, scrolls to or switches a state to, and every
/// page holding the main of an instance on a reached page. A target that dangles
/// reaches nothing.
pub fn reached_pages(doc: &Document, flows: &[FlowRef]) -> Vec<NodeId> {
    let mut seeds = Vec::new();
    for r in flows {
        let Some(page) = doc.page(r.page) else {
            continue;
        };
        seeds.push(r.page);
        if let Some(flow) = page.flows.iter().find(|f| f.id == r.flow) {
            seeds.extend(doc.page_of(flow.value.start));
        }
    }
    reached_from(doc, &seeds)
}

/// The pages `seeds` reach, themselves included, in document order:
/// [`reached_pages`]' walk from pages rather than flows.
fn reached_from(doc: &Document, seeds: &[NodeId]) -> Vec<NodeId> {
    let mut reached: HashSet<NodeId> = HashSet::new();
    let mut queue: Vec<NodeId> = Vec::new();
    let mut reach = |page: Option<NodeId>, queue: &mut Vec<NodeId>| {
        if let Some(page) = page
            && reached.insert(page)
        {
            queue.push(page);
        }
    };
    for page in seeds {
        reach(Some(*page), &mut queue);
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

/// A layer the file would carry whose interaction cannot play as authored: the
/// export is held until it is fixed.
#[derive(Clone, Debug, PartialEq)]
pub struct Hold {
    pub layer: NodeId,
    pub why: HoldWhy,
}

/// What a [`Hold`] is about.
#[derive(Clone, Debug, PartialEq)]
pub enum HoldWhy {
    /// An action names a layer that is gone.
    Missing,
    /// An expression does not read, with the reason.
    Expression(String),
}

/// Every [`Hold`] on the pages the export of `doc`'s chosen flows carries, in
/// document order, each layer once per kind.
pub fn holds(doc: &Document) -> Vec<Hold> {
    use mastik_core::prototype::Action;
    fn exprs(action: &Action, out: &mut Vec<String>) {
        out.extend(
            action
                .exprs()
                .into_iter()
                .filter_map(|e| e.check().err())
                .map(|e| e.to_string()),
        );
        for k in action.nested() {
            exprs(&k.value, out);
        }
    }
    let mut out: Vec<Hold> = Vec::new();
    let mut push = |hold: Hold| {
        if !out.iter().any(|h| {
            h.layer == hold.layer
                && std::mem::discriminant(&h.why) == std::mem::discriminant(&hold.why)
        }) {
            out.push(hold);
        }
    };
    for page in reached_pages(doc, &chosen_flows(doc)) {
        let mut stack = vec![page];
        while let Some(id) = stack.pop() {
            let Some(node) = doc.get(id) else { continue };
            stack.extend(node.children().iter().rev().copied());
            for reaction in node.reactions() {
                for action in &reaction.value.actions {
                    let missing = action.value.layer_targets().into_iter().any(|t| {
                        let t = mastik_core::prototype::resolve_target(doc, id, t);
                        doc.get(t).is_none()
                    }) || action
                        .value
                        .tables()
                        .into_iter()
                        .any(|t| doc.data_set(t).is_none());
                    if missing {
                        push(Hold {
                            layer: id,
                            why: HoldWhy::Missing,
                        });
                    }
                    let mut errors = Vec::new();
                    exprs(&action.value, &mut errors);
                    if let Some(e) = errors.into_iter().next() {
                        push(Hold {
                            layer: id,
                            why: HoldWhy::Expression(e),
                        });
                    }
                }
            }
        }
    }
    out
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
    let settings = PlayExport {
        flows: flows.clone(),
        opens: Some(opens),
        ..doc.play_export().clone()
    };
    trim(doc, &flows, pages, settings, Vec::new())
}

/// Where a mirrored prototype starts: the flow the app's player plays, or the
/// frame when it plays none.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Opening {
    Flow(FlowRef),
    Frame(NodeId),
}

/// `doc` trimmed for a phone mirroring the app's player from `opening`: the
/// pages it reaches, that flow alone (none for a frame), and the page itself as
/// the phone shows it — no bar, no device frame, and under the export's
/// *Follow its turn* no device, so *Responsive* lays out at the phone's own
/// size and follows its turn. Pages keep their scaling.
pub fn prune_mirror(doc: &Document, opening: Opening) -> Result<Document, PackError> {
    let (flows, pages) = match opening {
        Opening::Flow(r) => {
            if doc
                .page(r.page)
                .is_none_or(|p| !p.flows.iter().any(|f| f.id == r.flow))
            {
                return Err(PackError::NoFlows);
            }
            (vec![r], reached_pages(doc, &[r]))
        }
        Opening::Frame(f) => {
            let page = doc
                .get(f)
                .and_then(|n| n.parent())
                .filter(|p| doc.is_page(*p))
                .ok_or(PackError::NoFrame)?;
            (Vec::new(), reached_from(doc, &[page]))
        }
    };
    let settings = PlayExport {
        opens: flows.first().copied(),
        flows: flows.clone(),
        device_frame: false,
        player_bar: false,
        ..doc.play_export().clone()
    };
    let follow = doc.play_export().on_phone == mastik_core::prototype::OnPhone::Follow;
    let unturned: Vec<Operation> = pages
        .iter()
        .filter(|_| follow)
        .filter_map(|p| {
            let play = &doc.page(*p)?.play;
            (play.device.is_some() || play.landscape).then(|| Operation::SetPagePlay {
                id: *p,
                play: mastik_core::prototype::PagePlay {
                    device: None,
                    landscape: false,
                    ..play.clone()
                },
            })
        })
        .collect();
    trim(doc, &flows, pages, settings, unturned)
}

/// `doc` with `pages` kept whole and the rest taken, each kept page keeping only
/// the `flows` named, the export settings set to `settings`, after `more`.
fn trim(
    doc: &Document,
    flows: &[FlowRef],
    pages: Vec<NodeId>,
    settings: PlayExport,
    more: Vec<Operation>,
) -> Result<Document, PackError> {
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
    ops.extend(more);
    ops.push(Operation::SetPlayExport { settings });
    out.apply(&Transaction(ops))
        .map_err(|e| PackError::Prune(e.to_string()))?;
    let keep = Keep {
        pages,
        ..Keep::all(&out)
    };
    take(&mut out, &keep).map_err(|e| PackError::Prune(e.to_string()))?;
    for_sharing(&mut out).map_err(|e| PackError::Prune(e.to_string()))?;
    Ok(out)
}

/// What a file handed to other people need not carry, taken from `doc`: the
/// tables nothing kept reads (with the pictures only they named), every linked
/// file's path on this machine, and the document's meta but its name.
fn for_sharing(doc: &mut Document) -> Result<(), mastik_core::OpError> {
    let unread = mastik_core::io::unread_data_sets(doc);
    let mut ops: Vec<Operation> = doc
        .data_sets()
        .iter()
        .filter_map(|s| match unread.contains(&s.id) {
            true => Some(Operation::SetDataSet {
                id: s.id,
                set: None,
            }),
            false => s.source.is_some().then(|| Operation::SetDataSet {
                id: s.id,
                set: Some(Box::new(mastik_core::list::DataSet {
                    source: None,
                    ..s.clone()
                })),
            }),
        })
        .collect();
    let ids: Vec<NodeId> = doc.node_ids().collect();
    for id in ids {
        if let Some(chart) = (doc.get(id).and_then(|n| n.chart())).filter(|c| c.source.is_some()) {
            ops.push(Operation::SetChart {
                id,
                chart: Some(Box::new(mastik_core::chart::Chart {
                    source: None,
                    ..chart.clone()
                })),
            });
        }
    }
    doc.apply(&Transaction(ops))?;
    let gone: Vec<Operation> = mastik_core::io::unused_images(doc)
        .into_iter()
        .map(|id| Operation::RemoveImage { id })
        .collect();
    doc.apply(&Transaction(gone))?;
    doc.set_tokens_link(None);
    doc.set_meta(mastik_core::meta::DocumentMeta {
        name: doc.meta().name.clone(),
        ..Default::default()
    });
    Ok(())
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
/// layer whose CSS is longer than [`INSPECT_CSS_MAX`] is kept empty, so the page
/// tells it from a layer with none, or one play made (a list's row), which are
/// left out.
pub fn inspect_css(doc: &Document, res: &Resolved) -> Vec<(NodeId, String)> {
    let pages: Vec<NodeId> = doc.page_ids().collect();
    let layers = mastik_core::subtree_nodes(doc, &pages);
    layers
        .into_iter()
        .filter(|id| !pages.contains(id))
        .filter_map(|id| {
            let css = crate::code::css_of(doc, res, &[id]);
            match css.len() {
                0 => None,
                n if n > INSPECT_CSS_MAX => Some((id, String::new())),
                _ => Some((id, css)),
            }
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

/// Every file the layers' own export settings write ([`crate::plan::plan`]), rendered
/// now, since the page carries no exporter. A *One file per row* setting offers
/// the layer's one file as it stands, not a file per row.
pub fn inspect_files(doc: &Document, res: &Resolved) -> Vec<InspectFile> {
    let pages: Vec<NodeId> = doc.page_ids().collect();
    let opts = crate::plan::PlanOptions {
        rows_unfilled: true,
        ..Default::default()
    };
    let mut out = Vec::new();
    for id in mastik_core::subtree_nodes(doc, &pages) {
        if doc.get(id).is_none_or(|n| n.exports().is_empty()) {
            continue;
        }
        for file in crate::plan::plan(doc, res, &[id], opts.clone()) {
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
    /// The pictures the pruned document carries, each with its stored bytes.
    pub pictures: Vec<(mastik_core::ImageId, u64)>,
}

/// What the file's size is made of, in bytes on disk: the four shares sum to
/// [`Parts::estimate`].
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Breakdown {
    /// The wasm player and its loader, with the page around them.
    pub player: u64,
    pub fonts: u64,
    pub images: u64,
    /// The document without its pictures.
    pub document: u64,
}

impl Breakdown {
    pub fn total(&self) -> u64 {
        self.player + self.fonts + self.images + self.document
    }
}

/// The length of `chunks` gzipped at the fastest level.
fn gz_len(chunks: &[&[u8]]) -> u64 {
    let mut gz = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::fast());
    for c in chunks {
        gz.write_all(c).expect("writing to a Vec cannot fail");
    }
    gz.finish().map_or(0, |v| v.len() as u64)
}

impl<'a> Parts<'a> {
    /// [`prune`] `doc`, lay the result out and gather its fonts and what inspect
    /// mode shows. Fonts are those registered with this thread's shaping engine,
    /// so the caller registers the document's families first, as the canvas has.
    pub fn of(doc: &Document, player: Player<'a>) -> Result<Self, PackError> {
        Self::pack(prune(doc)?, player, true)
    }

    /// The parts of the page a phone mirroring the app loads: `doc` trimmed by
    /// [`prune_mirror`] from `opening`, its fonts, and no inspect mode, which
    /// the page has no bar to switch on. [`Parts::mirror_html`] writes it.
    pub fn mirror(doc: &Document, player: Player<'a>, opening: Opening) -> Result<Self, PackError> {
        Self::pack(prune_mirror(doc, opening)?, player, false)
    }

    /// The parts of `pruned`, inspect mode's with `inspect`.
    fn pack(mut pruned: Document, player: Player<'a>, inspect: bool) -> Result<Self, PackError> {
        let res = Resolved::settled(&mut pruned);
        let fonts = fonts_used(&pruned, &res);
        let (css, files) = match inspect {
            true => (inspect_css(&pruned, &res), inspect_files(&pruned, &res)),
            false => (Vec::new(), Vec::new()),
        };
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
        let mut pictures: Vec<(mastik_core::ImageId, u64)> = pruned
            .images()
            .map(|(id, e)| (id.clone(), e.bytes().map_or(0, |b| b.len() as u64)))
            .collect();
        pictures.sort();
        Ok(Self {
            player,
            document,
            fonts,
            title,
            css,
            files,
            pictures,
        })
    }

    /// [`Parts::estimate`] split by what makes it. Each part is weighed by its
    /// own gzipped size (a picture by its stored bytes, which gzip does not
    /// shrink), and the total shared out by those weights.
    pub fn breakdown(&self) -> Breakdown {
        let total = self.estimate();
        let images: u64 = self.pictures.iter().map(|(_, n)| n).sum();
        let fonts: Vec<&[u8]> = self.fonts.iter().map(|f| f.bytes.as_slice()).collect();
        let weights = [
            gz_len(&[self.player.wasm, self.player.glue.as_bytes()]),
            gz_len(&fonts),
            images,
            gz_len(&[&self.document]).saturating_sub(images),
        ];
        let sum: u64 = weights.iter().sum::<u64>().max(1);
        let share = |w: u64| (u128::from(total) * u128::from(w) / u128::from(sum)) as u64;
        let (fonts, images, document) = (share(weights[1]), share(weights[2]), share(weights[3]));
        Breakdown {
            player: total - fonts - images - document,
            fonts,
            images,
            document,
        }
    }

    /// The file's size in bytes before it is written, for the export dialog.
    /// Packed at gzip's fastest level, which the file's own level only improves
    /// on, so the answer is a little over, never under.
    pub fn estimate(&self) -> u64 {
        self.page_len(self.stream(flate2::Compression::fast()).len())
    }

    /// The HTML file.
    pub fn html(&self) -> String {
        self.page(None)
    }

    /// The page a mirroring phone loads: [`Parts::html`] with the page's mirror
    /// mode on, starting at `frame` (a frame's id) when the app's player plays no
    /// flow, its document being the app's `version`. The page then listens on `events` beside it for new versions of
    /// the document, fetches each from `doc`, and reports its screen to
    /// `status`, all relative to the page's own address.
    pub fn mirror_html(&self, frame: Option<NodeId>, version: u64) -> String {
        let frame = frame.map(NodeId::to_wire).unwrap_or_default();
        self.page(Some(&format!(
            "window.MASTIK_MIRROR = {{ frame: \"{frame}\", version: {version} }};\n"
        )))
    }

    /// The page, with `mirror`'s script before the boot when given.
    fn page(&self, mirror: Option<&str>) -> String {
        // A mirrored page crosses a local network once, written while the
        // person waits for the code: the fast level, not the file's best.
        let level = match mirror {
            Some(_) => flate2::Compression::fast(),
            None => flate2::Compression::best(),
        };
        let stream = self.stream(level);
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
        if let Some(mirror) = mirror {
            page.push_str(mirror);
        }
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
#inspect p.note { color: #e6e6e6; white-space: pre-wrap; overflow-wrap: anywhere; }
#inspect a { display: block; padding: 4px 0; color: #63b3f5; }
#inspect button { height: 22px; padding: 0 8px; border: 0; border-radius: 5px;
  background: #333336; color: #e6e6e6; font: inherit; font-size: 12px; cursor: pointer; }
#room { position: relative; }
#device { position: absolute; background: #0d0d0e;
  box-shadow: 0 0 0 1px #3a3a3d, 0 16px 48px rgba(0, 0, 0, 0.5); }
#screen, #hints { position: absolute; display: block; }
#screen { background: #fff; touch-action: none; }
#hints { pointer-events: none; }
#typing { position: fixed; left: 0; top: 0; width: 1px; height: 1px; padding: 0; border: 0;
  opacity: 0; resize: none; font-size: 16px; pointer-events: none; }
#bar { flex: none; display: flex; align-items: center; gap: 6px; height: 40px;
  padding: 0 12px; background: #252527; border-top: 1px solid #333336; }
#bar[hidden] { display: none; }
#name { flex: 1; min-width: 0; overflow: hidden; text-overflow: ellipsis; white-space: nowrap; }
#bar button, #bar select { height: 26px; padding: 0 10px; border: 0; border-radius: 5px;
  background: #333336; color: #e6e6e6; font: inherit; cursor: pointer; }
#bar button:hover, #bar select:hover { background: #3d3d41; }
#bar button[aria-pressed=\"true\"] { background: #2b5f8f; }
#toast { position: fixed; inset: auto 0 56px 0; text-align: center; pointer-events: none; }
#status { position: fixed; inset: 0; display: flex; flex-direction: column; align-items: center;
  justify-content: center; gap: 12px; text-align: center; pointer-events: none; }
#title { font-size: 15px; font-weight: 600; }
#load { width: 200px; height: 4px; border-radius: 2px; background: #333336; overflow: hidden; }
#load div { width: 0; height: 100%; background: #6d8cd9; transition: width 0.15s; }
#toast span { padding: 6px 12px; border-radius: 6px; background: #333336; }
#toast[hidden] { display: none; }
#note { position: fixed; top: 12px; right: 12px; padding: 6px 10px; border-radius: 6px;
  background: #333336; font-size: 12px; pointer-events: none; transition: opacity 0.6s; }
#note[hidden] { display: none; }
#note.gone { opacity: 0; }
body.ended #room { opacity: 0.4; }
</style>
";

const BODY: &str = "<div id=\"main\"><main id=\"stage\"><div id=\"room\">\
<div id=\"device\" hidden></div>\
<canvas id=\"screen\" width=\"1\" height=\"1\"></canvas>\
<canvas id=\"hints\" width=\"1\" height=\"1\"></canvas></div></main>\
<aside id=\"inspect\" hidden></aside></div>\n\
<textarea id=\"typing\" aria-hidden=\"true\" tabindex=\"-1\" autocomplete=\"off\" \
autocorrect=\"off\" autocapitalize=\"off\" spellcheck=\"false\"></textarea>\n\
<nav id=\"bar\" hidden><span id=\"name\"></span><select id=\"flows\" hidden \
aria-label=\"Flow\"></select><button id=\"restart\" type=\"button\">Restart</button>\
<button id=\"hint-switch\" type=\"button\" aria-pressed=\"false\" hidden>Hotspots</button>\
<button id=\"turn\" type=\"button\" aria-pressed=\"false\" hidden>Turn</button>\
<button id=\"inspect-switch\" type=\"button\" aria-pressed=\"false\">Inspect</button>\
<button id=\"full\" type=\"button\" aria-pressed=\"false\" hidden>Fullscreen</button></nav>\n\
<div id=\"status\"><div id=\"title\"></div><div id=\"load\"><div></div></div>\
<p id=\"said\">Opening the prototype…</p></div>\n<div id=\"toast\" hidden><span></span></div>\n\
<div id=\"note\" hidden>Drawn without the GPU; motion may be slower</div>\n";

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
  // Set by the page a mirroring phone loads (`Parts::mirror_html`), never by a file.
  const mirrored = window.MASTIK_MIRROR || null;
  const status = $("status");
  const canvas = $("screen");
  // The loading bar: unpacking is most of the wait, then the player and fonts.
  $("title").textContent = document.title;
  const loaded = share => { $("load").firstChild.style.width = Math.round(share * 100) + "%"; };
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
    let read = 0;
    const counted = new TransformStream({
      transform(chunk, out) {
        read += chunk.length;
        loaded(0.8 * read / packed.length);
        out.enqueue(chunk);
      },
    });
    const stream = new Blob([packed]).stream().pipeThrough(counted)
      .pipeThrough(new DecompressionStream("gzip"));
    const bytes = new Uint8Array(await new Response(stream).arrayBuffer());
    const parts = unpack(bytes);
    await wasm_bindgen({ module_or_path: parts.wasm });
    loaded(0.9);
    for (const font of parts.fonts) wasm_bindgen.add_font(font.bytes, font.alias || undefined);
    loaded(0.95);
    const cpu = new URLSearchParams(location.search).has("cpu");
    const player = await wasm_bindgen.start(canvas, parts.document, cpu, mirrored ? mirrored.frame : "");
    loaded(1);
    // A phone: a coarse pointer on a small screen.
    player.set_on_phone(matchMedia("(pointer: coarse)").matches && Math.min(screen.width, screen.height) < 600);
    play(player, parts);
    status.remove();
    document.body.dataset.backend = player.backend();
    // Fallen back to the CPU, unasked: say so in a corner, then fade.
    if (player.backend() === "cpu" && !cpu && !mirrored) {
      const note = $("note");
      note.hidden = false;
      setTimeout(() => note.classList.add("gone"), 4000);
      setTimeout(() => { note.hidden = true; }, 4700);
    }
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
    // The panel for the picked layer: its name, its annotation, its CSS as Copy
    // as CSS writes it, and the files its export settings write.
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
      const note = player.layer_note(picked);
      if (note) {
        el("h3", "Annotation");
        el("p", note).className = "note";
      }
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
      } else if (css === "") {
        el("p", "Too long to carry in the file; copy it from Mastik.");
      } else {
        // A layer with no CSS, or one play made (a list's row).
        el("p", "No CSS for this layer here; inspect it in Mastik.");
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
      showTurn();
      wake();
    }

    // The turn button shows while the page shown has a device, pressed while
    // it plays on its side.
    function showTurn() {
      const turn = $("turn");
      turn.hidden = !player.player_bar() || !player.can_turn();
      turn.setAttribute("aria-pressed", String(player.turned()));
    }
    let toastTimer = 0;
    function toast(text, ms = 4000) {
      const t = $("toast");
      t.firstChild.textContent = text;
      t.hidden = false;
      clearTimeout(toastTimer);
      if (ms !== Infinity) toastTimer = setTimeout(() => { t.hidden = true; }, ms);
    }
    const local = e => {
      const r = canvas.getBoundingClientRect();
      return [e.clientX - r.left, e.clientY - r.top];
    };

    // A finger hovers nothing: the player is told which hand each event is.
    let touching = false;
    const hand = e => {
      const t = e.pointerType === "touch";
      if (t !== touching) { touching = t; player.set_touch(t); }
    };
    canvas.addEventListener("pointerdown", e => {
      if (e.button !== 0) return;
      e.preventDefault();
      hand(e);
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
      hand(e);
      const [x, y] = local(e);
      if (inspecting) {
        over = player.layer_at(x, y);
        return after();
      }
      player.pointer_move(x, y);
      canvas.style.cursor = player.over_hotspot(x, y) ? "pointer" : "";
      after();
    });
    // A canvas raises no on-screen keyboard: while a field has play's focus, the
    // hidden textarea holds the browser's, inside the tap so a phone allows it.
    const typing = $("typing");
    const followFocus = () => {
      const typingHere = document.activeElement === typing;
      if (player.focused() && !typingHere) typing.focus({ preventScroll: true });
      else if (!player.focused() && typingHere) typing.blur();
    };
    canvas.addEventListener("pointerup", e => {
      if (e.button !== 0 || inspecting) return;
      player.pointer_up(...local(e));
      followFocus();
      after();
    });
    // Text arrives as input events, whole once a composition ends (IME, and a
    // phone keyboard that composes); the textarea is kept empty.
    typing.addEventListener("beforeinput", e => {
      if (e.inputType === "insertCompositionText") return;
      e.preventDefault();
      if (e.inputType === "insertText" || e.inputType === "insertReplacementText") {
        if (e.data) player.type_text(e.data);
      } else if (e.inputType === "deleteContentBackward") {
        player.key("Backspace", false);
      } else if (e.inputType === "insertLineBreak" || e.inputType === "insertParagraph") {
        player.key("Enter", false);
      }
      followFocus();
      after();
    });
    typing.addEventListener("compositionend", e => {
      if (e.data) player.type_text(e.data);
      typing.value = "";
      followFocus();
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
      if (e.target === typing) {
        // A character comes as `beforeinput`; a named key is play's, never the
        // textarea's.
        if ([...e.key].length === 1 || e.key === "Unidentified" || e.key === "Process") return;
        e.preventDefault();
        player.key(e.key, e.shiftKey);
        followFocus();
        return after();
      }
      if (player.key(e.key, e.shiftKey)) e.preventDefault();
      followFocus();
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
      $("turn").addEventListener("click", e => {
        player.turn();
        e.currentTarget.blur();
        after();
      });
      showTurn();
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
      const full = $("full");
      full.hidden = !document.fullscreenEnabled;
      full.addEventListener("click", () => {
        if (document.fullscreenElement) document.exitFullscreen();
        else document.documentElement.requestFullscreen().catch(() => {});
        full.blur();
      });
      addEventListener("fullscreenchange", () => {
        full.setAttribute("aria-pressed", String(!!document.fullscreenElement));
        wake();
      });
    }
    if (mirrored) mirror();
    wake();

    // A phone mirroring the app: each newer version of the document is played
    // in place of the old where play is, the screen shown is reported, and once
    // the app stops sharing the last frame stays, dimmed, under a line saying so.
    function mirror() {
      // The id outlives the reload a page needing fonts takes, so the app counts
      // the phone once; it is taken as it is read, so a duplicated tab picks its own.
      let id = null;
      try {
        id = sessionStorage.getItem("mastik-phone");
        sessionStorage.removeItem("mastik-phone");
      } catch (_) {}
      if (!id) id = Array.from(crypto.getRandomValues(new Uint32Array(2))).join("-");
      let version = mirrored.version, screen = null, ended = false;
      const report = () => {
        const now = player.screen_name();
        if (ended || now === screen) return;
        screen = now;
        fetch("status", { method: "POST", body: JSON.stringify({ id, screen }) }).catch(() => {});
      };
      report();
      const reporting = setInterval(report, 500);
      const events = new EventSource("events?id=" + encodeURIComponent(id));
      // A blip or a sleep only drops the stream, and EventSource reconnects by
      // itself; the share has stopped on CLOSED (a dead token), on "stop", or
      // when reconnects keep failing while the page is in front.
      let failed = 0;
      const end = () => {
        if (ended) return;
        ended = true;
        events.close();
        clearInterval(reporting);
        document.body.classList.add("ended");
        toast("Mastik stopped sharing", Infinity);
      };
      events.onopen = () => { failed = 0; };
      events.onerror = () => {
        if (events.readyState === EventSource.CLOSED) return end();
        if (document.hidden) return;
        if (++failed >= 10) end();
      };
      events.onmessage = async e => {
        if (e.data === "stop") return end();
        if (e.data === "reload") {
          try { sessionStorage.setItem("mastik-phone", id); } catch (_) {}
          return location.reload();
        }
        const v = Number(e.data);
        if (!(v > version)) return;
        version = v;
        try {
          const r = await fetch("doc?v=" + v);
          if (!r.ok || v !== version) return;
          player.rebase(new Uint8Array(await r.arrayBuffer()));
          toast("Updated from Mastik", 1500);
          report();
          wake();
        } catch (err) {
          console.error(err);
        }
      };
    }
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
