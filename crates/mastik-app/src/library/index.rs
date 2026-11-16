//! What each document holds, for the library to search inside it: page,
//! top-level frame, component and style names and every text's content.
//!
//! **Derived and per machine**, like covers (`super::cover`): the facts live in
//! [`crate::machine_dir::root`]`/index`, one JSON file per document keyed by id
//! *and* modification time, so a document edited elsewhere is read again and
//! nothing is ever written into the library. Delete the folder and it refills.
//!
//! **Read on a thread of its own, for every document**, not only the ones on
//! screen: a search is over the whole library. The cover worker renders only the
//! cards the grid shows and skips the load when its picture is cached, so it
//! cannot be the one to read them all; this worker loads each document once,
//! and the cache makes every later launch a file read.

use super::ids;
use super::scan::Entry;
use super::search::{self, Hit};
use eframe::egui;
use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

/// How many texts a document's facts keep, and how long each may be: enough for
/// a search to find a label or a paragraph, bounded for a document of thousands.
const MAX_TEXTS: usize = 2_000;
const MAX_TEXT_CHARS: usize = 400;

/// One page, as the index keeps it.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct PageFact {
    /// The page root's wire id, as `LocalIndex::last_page` names it.
    pub id: String,
    pub name: String,
    pub draft: bool,
}

/// What one document holds, as far as the library's search and cards read it.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Facts {
    pub pages: Vec<PageFact>,
    /// The names of the frames directly on a page.
    pub frames: Vec<String>,
    /// Every main component's name.
    pub components: Vec<String>,
    /// The document's own styles' names.
    pub styles: Vec<String>,
    /// Every text's content, cut to [`MAX_TEXT_CHARS`].
    pub texts: Vec<String>,
    /// The kits this document uses, each with the oldest version any of its
    /// copies came from: a vault main's `origin`, as `kits::users` counts it.
    pub kits: Vec<KitUse>,
}

/// One kit a document took components from.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct KitUse {
    /// The kit's `meta.id`.
    pub kit: String,
    pub version: u32,
}

/// What kind of thing a content match found, in the order a row names the
/// strongest.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum ReasonKind {
    Page,
    Frame,
    Component,
    Style,
    Text,
}

impl ReasonKind {
    /// The word a row's reason leads with.
    pub fn word(self) -> &'static str {
        match self {
            Self::Page => "page",
            Self::Frame => "frame",
            Self::Component => "component",
            Self::Style => "style",
            Self::Text => "text",
        }
    }
}

/// Why a document matched by what it holds: the kind, the text shown (a text
/// quoted and cut to the match) and the byte ranges of it to light.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Reason {
    pub kind: ReasonKind,
    pub text: String,
    pub hit: Hit,
}

impl Facts {
    /// Read a loaded document's facts.
    pub fn of(doc: &mastik_core::Document) -> Self {
        let mut facts = Facts::default();
        let mut stack = Vec::new();
        for page in doc.pages() {
            let Some(root) = doc.get(page.id) else {
                continue;
            };
            facts.pages.push(PageFact {
                id: page.id.to_wire(),
                name: root.name().to_string(),
                draft: page.draft,
            });
            for child in root.children() {
                if let Some(node) = doc.get(*child)
                    && matches!(node.kind(), mastik_core::NodeKind::Artboard { .. })
                {
                    facts.frames.push(node.name().to_string());
                }
            }
            stack.push(page.id);
        }
        while let Some(id) = stack.pop() {
            let Some(node) = doc.get(id) else {
                continue;
            };
            if node.component() {
                facts.components.push(node.name().to_string());
            }
            if let mastik_core::NodeKind::Text { content, .. } = node.kind()
                && facts.texts.len() < MAX_TEXTS
                && !content.trim().is_empty()
            {
                facts
                    .texts
                    .push(content.chars().take(MAX_TEXT_CHARS).collect());
            }
            stack.extend(node.children().iter().copied());
        }
        if let Some(vault) = doc.vault().and_then(|v| doc.get(v)) {
            for origin in vault
                .children()
                .iter()
                .filter_map(|t| doc.get(*t)?.origin())
            {
                match facts.kits.iter_mut().find(|k| k.kit == origin.kit) {
                    Some(k) => k.version = k.version.min(origin.version),
                    None => facts.kits.push(KitUse {
                        kit: origin.kit.clone(),
                        version: origin.version,
                    }),
                }
            }
        }
        facts.styles = doc
            .styles()
            .iter()
            .filter(|s| s.origin.is_none())
            .map(|s| s.name.clone())
            .collect();
        facts
    }

    /// The strongest reason this document answers `words`, or `None`: the first
    /// kind in [`ReasonKind`]'s order that has a match, and within it the best.
    pub fn reason(&self, words: &[String]) -> Option<Reason> {
        let kinds: [(ReasonKind, Vec<&str>); 5] = [
            (
                ReasonKind::Page,
                self.pages.iter().map(|p| p.name.as_str()).collect(),
            ),
            (
                ReasonKind::Frame,
                self.frames.iter().map(String::as_str).collect(),
            ),
            (
                ReasonKind::Component,
                self.components.iter().map(String::as_str).collect(),
            ),
            (
                ReasonKind::Style,
                self.styles.iter().map(String::as_str).collect(),
            ),
            (
                ReasonKind::Text,
                self.texts.iter().map(String::as_str).collect(),
            ),
        ];
        for (kind, names) in kinds {
            let best = names
                .into_iter()
                .filter_map(|n| Some((n, search::rank(n, words)?)))
                .min_by_key(|(_, h)| h.tier);
            if let Some((name, hit)) = best {
                return Some(match kind {
                    ReasonKind::Text => quoted(name, words, hit.tier),
                    kind => Reason {
                        kind,
                        text: name.to_string(),
                        hit,
                    },
                });
            }
        }
        None
    }
}

/// How much of a text a reason keeps before its first matched letter.
const QUOTE_LEAD: usize = 12;

/// A text reason: quoted, one line, cut to start a little before the match.
/// Ranked again against the one-line copy, since a multi-byte space (U+00A0,
/// U+3000) becomes one byte there and every offset after it moves.
fn quoted(text: &str, words: &[String], tier: search::Tier) -> Reason {
    let line: String = text
        .chars()
        .map(|c| if c.is_whitespace() { ' ' } else { c })
        .collect();
    let hit = search::rank(&line, words).unwrap_or(Hit {
        tier,
        lit: Vec::new(),
    });
    let first = hit.lit.first().map_or(0, |r| r.start).min(line.len());
    let first = match line.is_char_boundary(first) {
        true => first,
        false => 0,
    };
    // Cut on a character boundary at most `QUOTE_LEAD` characters before the match.
    let cut = line[..first]
        .char_indices()
        .rev()
        .nth(QUOTE_LEAD)
        .map_or(0, |(i, _)| i);
    let (lead, body) = match cut {
        0 => ("“", &line[..]),
        _ => ("“… ", &line[cut..]),
    };
    let shift = lead.len() as isize - cut as isize;
    let lit = hit
        .lit
        .iter()
        .filter(|r| r.start >= cut)
        .map(|r| (r.start as isize + shift) as usize..(r.end as isize + shift) as usize)
        .collect();
    Reason {
        kind: ReasonKind::Text,
        text: format!("{lead}{body}”"),
        hit: Hit { tier, lit },
    }
}

/// The shape [`Facts`] is cached in. Bump it when a field is added, so the cache
/// written by an older build reads as a miss rather than as facts without it.
const FORMAT: u32 = 1;

/// An index key: the document's id plus its file's modification time, for the
/// reason `super::cover`'s key gives, and [`FORMAT`]; also the cache file's
/// stem. `None` for a document with no minted id.
fn key(entry: &Entry) -> Option<String> {
    let id = entry.meta.id.as_deref().filter(|id| ids::is_minted(id))?;
    Some(format!("{id}-{}-f{FORMAT}", entry.modified))
}

/// The cache's directory; `None` under `cfg(test)`, so the suite never writes
/// into or sweeps the developer's own cache.
fn index_dir() -> Option<PathBuf> {
    if cfg!(test) {
        return None;
    }
    Some(crate::machine_dir::root()?.join("index"))
}

/// The reading thread and the answers coming back. A worker of its own, for
/// `super::cover::Renderer`'s reason: a load of hundreds of milliseconds must not
/// wait behind, or hold up, anything else.
struct Reader {
    tx: Option<std::sync::mpsc::Sender<(PathBuf, String)>>,
    done: std::sync::mpsc::Receiver<(String, Option<Facts>)>,
    in_flight: usize,
    handle: Option<std::thread::JoinHandle<()>>,
    /// Read before each job, so a drop waits on one read, not the backlog.
    cancel: Arc<AtomicBool>,
}

impl Reader {
    fn spawn(ctx: &egui::Context, dir: Option<PathBuf>) -> Option<Self> {
        let (tx, jobs) = std::sync::mpsc::channel::<(PathBuf, String)>();
        let (report, done) = std::sync::mpsc::channel();
        let ctx = ctx.clone();
        let cancel = Arc::new(AtomicBool::new(false));
        let cancelled = Arc::clone(&cancel);
        let handle = std::thread::Builder::new()
            .name("mastik-index".into())
            .spawn(move || {
                for (path, key) in jobs {
                    if cancelled.load(Ordering::Relaxed) {
                        return;
                    }
                    // A panic costs one document's facts and not the worker.
                    let facts = std::panic::catch_unwind(|| read(dir.as_deref(), &path, &key))
                        .ok()
                        .flatten();
                    if report.send((key, facts)).is_err() {
                        return;
                    }
                    ctx.request_repaint();
                }
            })
            .ok()?;
        Some(Self {
            tx: Some(tx),
            done,
            in_flight: 0,
            handle: Some(handle),
            cancel,
        })
    }
}

impl Drop for Reader {
    fn drop(&mut self) {
        self.cancel.store(true, Ordering::Relaxed);
        self.tx = None;
        if let Some(handle) = self.handle.take() {
            let _ = handle.join();
        }
    }
}

/// A document's facts: from the cache when it is there, by loading it if not.
/// `None` for a file that will not read or load.
fn read(dir: Option<&Path>, path: &Path, key: &str) -> Option<Facts> {
    let cached = dir.map(|d| d.join(format!("{key}.json")));
    if let Some(facts) = cached
        .as_ref()
        .and_then(|p| std::fs::read(p).ok())
        .and_then(|b| serde_json::from_slice::<Facts>(&b).ok())
    {
        return Some(facts);
    }
    let bytes = std::fs::read(path).ok()?;
    let doc = mastik_core::io::load(&bytes).ok()?;
    let facts = Facts::of(&doc);
    if let Some(p) = &cached {
        if let Some(d) = p.parent() {
            let _ = std::fs::create_dir_all(d);
        }
        // Best-effort and derived: a torn file reads as a miss and is rewritten.
        if let Ok(json) = serde_json::to_vec(&facts) {
            let _ = std::fs::write(p, json);
        }
    }
    Some(facts)
}

/// The library's content index.
///
/// [`Self::pass`] hands every document it has not read to the worker, once, and
/// takes what came back; nothing in the egui pass reads a file.
pub struct ContentIndex {
    /// By key; `None` is a document that would not load, remembered so it is not
    /// read again every pass.
    facts: HashMap<String, Option<Facts>>,
    queued: HashSet<String>,
    reader: Option<Reader>,
    dir: Option<PathBuf>,
}

impl Default for ContentIndex {
    fn default() -> Self {
        Self {
            facts: HashMap::new(),
            queued: HashSet::new(),
            reader: None,
            dir: index_dir(),
        }
    }
}

impl ContentIndex {
    /// Once per library pass: take the worker's answers, then send it every
    /// document in `entries` not yet read or asked for.
    pub fn pass(&mut self, ctx: &egui::Context, entries: &[Entry]) {
        self.drain();
        for entry in entries {
            let Some(key) = key(entry) else {
                continue;
            };
            if self.facts.contains_key(&key) || self.queued.contains(&key) {
                continue;
            }
            if self.reader.is_none() {
                self.reader = Reader::spawn(ctx, self.dir.clone());
            }
            let Some(reader) = self.reader.as_mut() else {
                return;
            };
            let sent = reader
                .tx
                .as_ref()
                .is_some_and(|tx| tx.send((entry.path.clone(), key.clone())).is_ok());
            if !sent {
                // The worker has gone; the next pass spawns another.
                self.reader = None;
                return;
            }
            reader.in_flight += 1;
            self.queued.insert(key);
        }
    }

    /// Take what the worker has finished.
    fn drain(&mut self) {
        let Some(reader) = self.reader.as_mut() else {
            return;
        };
        let mut gone = false;
        loop {
            match reader.done.try_recv() {
                Ok((key, facts)) => {
                    reader.in_flight = reader.in_flight.saturating_sub(1);
                    self.queued.remove(&key);
                    self.facts.insert(key, facts);
                }
                Err(std::sync::mpsc::TryRecvError::Empty) => break,
                Err(std::sync::mpsc::TryRecvError::Disconnected) => {
                    gone = true;
                    break;
                }
            }
        }
        if gone {
            self.queued.clear();
            self.reader = None;
        }
    }

    /// `entry`'s facts, once read; `None` before then and for a document that
    /// would not load.
    pub fn get(&self, entry: &Entry) -> Option<&Facts> {
        self.facts.get(&key(entry)?)?.as_ref()
    }

    /// Whether `entry` has been read, either way; a document with no key never
    /// will be and counts as read.
    pub fn is_read(&self, entry: &Entry) -> bool {
        key(entry).is_none_or(|k| self.facts.contains_key(&k))
    }

    /// How many of `entries` have been read, and how many there are.
    pub fn progress(&self, entries: &[Entry]) -> (usize, usize) {
        let read = entries.iter().filter(|e| self.is_read(e)).count();
        (read, entries.len())
    }

    /// The documents among `entries` that use the kit `kit` (its `meta.id`), each
    /// with the oldest version it took; `None` until every one has been read, so
    /// a count is never a part-count.
    pub fn kit_users<'a>(&self, kit: &str, entries: &'a [Entry]) -> Option<Vec<(&'a Entry, u32)>> {
        let mut out = Vec::new();
        for e in entries {
            if !self.is_read(e) {
                return None;
            }
            if e.meta.id.as_deref() == Some(kit) {
                continue;
            }
            if let Some(k) = self
                .get(e)
                .and_then(|f| f.kits.iter().find(|k| k.kit == kit))
            {
                out.push((e, k.version));
            }
        }
        Some(out)
    }

    /// The strongest reason `entry` answers `words` by what it holds.
    pub fn reason(&self, entry: &Entry, words: &[String]) -> Option<Reason> {
        self.get(entry)?.reason(words)
    }

    /// Forget every answer and the worker, after the base folder changes.
    pub fn clear(&mut self) {
        self.facts.clear();
        self.queued.clear();
        self.reader = None;
    }

    /// Delete cached facts no live document claims, as `Covers::sweep` does; an
    /// empty list sweeps nothing.
    pub fn sweep(&self, live: &[Entry]) {
        let Some(dir) = self.dir.as_deref() else {
            return;
        };
        if live.is_empty() {
            return;
        }
        let keep: HashSet<String> = live.iter().filter_map(key).collect();
        let Ok(files) = std::fs::read_dir(dir) else {
            return;
        };
        for file in files.flatten() {
            let path = file.path();
            let stem = path.file_stem().and_then(|s| s.to_str()).unwrap_or("");
            if path.extension().and_then(|e| e.to_str()) == Some("json") && !keep.contains(stem) {
                let _ = std::fs::remove_file(&path);
            }
        }
    }

    /// Wait for every document sent and take the answers. Plain backticks: this
    /// item is `cfg(test)`.
    #[cfg(test)]
    pub(crate) fn settle(&mut self) {
        while let Some(reader) = self.reader.as_mut() {
            if reader.in_flight == 0 {
                break;
            }
            match reader.done.recv() {
                Ok((key, facts)) => {
                    reader.in_flight -= 1;
                    self.queued.remove(&key);
                    self.facts.insert(key, facts);
                }
                Err(_) => {
                    self.reader = None;
                    self.queued.clear();
                }
            }
        }
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use mastik_core::kurbo::Size;
    use mastik_core::{Document, IdSource, NodeKind, Operation, Transaction};

    /// A page holding the frame *Wallet*, and in it a text.
    pub(crate) fn doc() -> Document {
        let mut ids = IdSource::new(0x1D3);
        let mut doc = Document::new(ids.mint());
        let page = doc.first_page();
        let frame = ids.mint();
        doc.apply(&Transaction(vec![
            Operation::CreateNode {
                id: frame,
                parent: page,
                index: 0,
                kind: NodeKind::Artboard {
                    size: Size::new(100.0, 100.0),
                    corner_radii: Default::default(),
                    corner_smoothing: 0.0,
                },
                transform: None,
                name: Some("Wallet".into()),
            },
            Operation::CreateNode {
                id: ids.mint(),
                parent: frame,
                index: 0,
                kind: crate::control_audit::text_kind("Pay now, safely"),
                transform: None,
                name: None,
            },
        ]))
        .expect("build");
        doc
    }

    /// Facts read the pages, the frames on them and every text.
    #[test]
    fn facts_read_pages_frames_and_texts() {
        let facts = Facts::of(&doc());
        assert_eq!(facts.pages.len(), 1);
        assert_eq!(facts.frames, ["Wallet"]);
        assert_eq!(facts.texts, ["Pay now, safely"]);
    }

    /// A reason names the strongest kind that matched: a frame over a text, and
    /// a text quoted with its matched words lit.
    /// flip: `reason` trying the kinds in reverse → fails at "the frame".
    #[test]
    fn a_reason_names_the_strongest_kind_and_quotes_a_text() {
        let facts = Facts {
            frames: vec!["Wallet sheet".into()],
            texts: vec!["Your wallet is empty".into(), "Pay now, safely".into()],
            ..Facts::default()
        };
        let r = facts.reason(&search::words("wallet")).unwrap();
        assert_eq!(
            (r.kind, r.text.as_str()),
            (ReasonKind::Frame, "Wallet sheet"),
            "the frame"
        );
        let r = facts.reason(&search::words("pay")).unwrap();
        assert_eq!(r.kind, ReasonKind::Text);
        assert_eq!(r.text, "“Pay now, safely”");
        assert_eq!(&r.text[r.hit.lit[0].clone()], "Pay");
        assert!(facts.reason(&search::words("checkout")).is_none());
    }

    /// A long text is cut to start shortly before its match, and the lit range
    /// follows the cut.
    #[test]
    fn a_long_text_is_cut_to_its_match() {
        let facts = Facts {
            texts: vec!["We sent a code to the address you gave; check your inbox".into()],
            ..Facts::default()
        };
        let r = facts.reason(&search::words("inbox")).unwrap();
        assert!(r.text.starts_with("“… "), "{}", r.text);
        assert_eq!(&r.text[r.hit.lit[0].clone()], "inbox");
    }

    /// A match after a multi-byte space lights the matched letters, cut or not.
    /// flip: `quoted` ranking the text rather than its one-line copy → fails at
    /// the no-break space's case.
    #[test]
    fn a_match_after_a_wide_space_lights_its_own_letters() {
        for (text, query) in [
            ("Prix\u{A0}: école", "école"),
            ("東京\u{3000}渋谷", "渋谷"),
            (
                "Une longue phrase avant\u{A0}: la réponse est là, école",
                "école",
            ),
        ] {
            let facts = Facts {
                texts: vec![text.into()],
                ..Facts::default()
            };
            let r = facts.reason(&search::words(query)).unwrap();
            assert_eq!(&r.text[r.hit.lit[0].clone()], query, "{text}");
        }
    }
}
