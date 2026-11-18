//! `mastik serve` (§8.2): documents with no window, behind the MCP endpoint.
//!
//! Each document is an `EditorSession`, so a write takes the same commit door as
//! in the window; each commit is written to its file at once, since nobody is
//! there to save later. The process ends when a window starts, which takes over,
//! and after its last connection closes — [`KEYED_LINGER`] after, while it
//! remembers a keyed client. The asset tools read the sets and services the
//! person switched on in Settings, and a call waiting on a download is run
//! again as the window runs one ([`run_waiting`]).

use super::assets::{Desk, Pack};
use super::endpoint::{self, Host};
use super::system_fonts::SystemFonts;
use super::{Documents, Engine};
use crate::assets::panel::KitPack;
use crate::library::state::Library;
use crate::session::EditorSession;
use mastik_mcp::queue::{self, Request};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::mpsc::RecvTimeoutError;
use std::time::{Duration, Instant};

/// How long a `serve` nobody has connected to waits for its first client.
const FIRST_WAIT: Duration = Duration::from_secs(60);

/// How often it looks for a window to hand over to.
const WINDOW_EVERY: Duration = Duration::from_secs(2);

/// How long, after its last answer, it waits before handing over to a window.
const FLUSH_WAIT: Duration = Duration::from_secs(1);

/// How long it outlives its last connection while it remembers a keyed client:
/// a harness that starts a process per turn reconnects at the next turn, and
/// finds its document and revisions where it left them.
const KEYED_LINGER: Duration = Duration::from_secs(15 * 60);

/// How often waiting asset calls are run again when no download has landed,
/// as the window does.
const LATER_EVERY: Duration = Duration::from_millis(500);

/// How long the loop waits for a request while an asset call waits on a
/// download, so one that landed is answered soon after.
const LATER_POLL: Duration = Duration::from_millis(50);

/// The headless host's documents.
pub(crate) struct Headless {
    open: HashMap<PathBuf, EditorSession>,
    library: Option<PathBuf>,
    /// The library, scanned the first time a kit tool asks for it.
    scanned: Option<Library>,
    /// The installed fonts, read the first time a document or `list_fonts`
    /// needs them.
    fonts: Option<SystemFonts>,
    /// How many undo steps a library document keeps across opens; `0` keeps none.
    history: u64,
    /// The icon sets, kit packs and photo services the person switched on, as
    /// the preferences held them when `serve` started; a window that starts
    /// takes over.
    asset_prefs: crate::prefs::AssetPrefs,
    assets: crate::assets::Assets,
    /// What `Assets::pass` hands frames to; nothing draws here.
    ctx: egui::Context,
    /// The kit icon packs read, by kit id, with the file time they were read at.
    packs: HashMap<String, (u64, Option<Arc<KitPack>>)>,
}

impl Headless {
    pub(crate) fn new(library: Option<PathBuf>) -> Self {
        Self {
            open: HashMap::new(),
            library,
            scanned: None,
            fonts: None,
            history: crate::prefs::Prefs::default().history_size,
            asset_prefs: crate::prefs::AssetPrefs::default(),
            assets: crate::assets::Assets::default(),
            ctx: egui::Context::default(),
            packs: HashMap::new(),
        }
    }

    /// Register the installed families `path`'s document names, and lay its
    /// text out again when any landed: `serve`'s engine starts with Inter alone.
    fn load_fonts(&mut self, path: &Path) {
        let Some(session) = self.open.get_mut(path) else {
            return;
        };
        let fonts = self.fonts.get_or_insert_with(SystemFonts::load);
        if fonts.register_for(&session.doc) {
            session.fonts_changed();
        }
    }

    /// The library, scanned again.
    fn rescan(&mut self) {
        match (&mut self.scanned, &self.library) {
            (Some(lib), _) => lib.refresh(),
            (None, Some(root)) => self.scanned = Some(Library::open(root.clone())),
            (None, None) => {}
        }
    }
}

impl Documents for Headless {
    fn open(&mut self, wanted: &str) -> Result<PathBuf, String> {
        let open: Vec<PathBuf> = self.open.keys().cloned().collect();
        let path = super::resolve(wanted, &open, self.library.as_deref())?;
        // The open document's own spelling for the same file (a case difference on
        // Windows, a `..`), so one file never gets two sessions writing over each
        // other.
        if !self.open.contains_key(&path)
            && let Ok(want) = std::fs::canonicalize(&path)
            && let Some(same) = open
                .iter()
                .find(|p| std::fs::canonicalize(p).is_ok_and(|p| p == want))
        {
            return Ok(same.clone());
        }
        if !self.open.contains_key(&path) {
            // The undo history the person's window kept for these bytes is
            // resumed, so a later write keeps it (`Headless::save`).
            let keep = self.library.is_some() && self.history > 0;
            let root = self.library.as_deref().unwrap_or(path.as_path());
            let (doc, history, stamp) = crate::app::read_opened(&path, root, keep)
                .map_err(|e| format!("cannot open {}: {e}", path.display()))?;
            let mut session = EditorSession::new();
            session.adopt_document(doc, Some(path.clone()));
            session.disk = stamp;
            if let Some(history) = history {
                session.resume_history(history);
            }
            self.open.insert(path.clone(), session);
            self.load_fonts(&path);
        }
        Ok(path)
    }

    fn session(&mut self, path: &Path) -> Option<&mut EditorSession> {
        self.open.get_mut(path)
    }

    fn committed(&mut self, path: &Path) -> Result<(), String> {
        self.load_fonts(path);
        self.save(path)
    }

    fn library(&mut self) -> Option<&Library> {
        self.rescan();
        self.scanned.as_ref()
    }

    fn with_library(&mut self, path: &Path) -> Option<(&mut EditorSession, Option<&Library>)> {
        self.rescan();
        Some((self.open.get_mut(path)?, self.scanned.as_ref()))
    }

    fn save(&mut self, path: &Path) -> Result<(), String> {
        let session = self
            .open
            .get_mut(path)
            .ok_or_else(|| format!("{} is no longer open", path.display()))?;
        // A change made outside since `serve` read the file (a pull, a sync, a
        // merge) wins: nobody is here to choose, so this copy is dropped unwritten
        // and the next `use_document` reads the file as it is.
        if session.disk.as_mut().is_some_and(|s| s.check(path)) {
            self.open.remove(path);
            return Err(format!(
                "{} changed on disk since Mastik read it, so this edit was not written; \
                 call `use_document` to read it again",
                path.display()
            ));
        }
        let digest = crate::library::store::write_document_digest(path, &session.doc)
            .map_err(|e| format!("cannot write {}: {e}", path.display()))?;
        // A library document's kept history is tied to its file's bytes, so it
        // is written again beside them, as the window's write does.
        if let Some(root) = self.library.as_deref().filter(|r| path.starts_with(r))
            && self.history > 0
            && let Some(id) = session.doc.meta().id.clone()
        {
            let limit = usize::try_from(self.history).unwrap_or(usize::MAX);
            if let Err(e) =
                crate::library::history::write(root, &id, &digest, session.history.saved(limit))
            {
                log::warn!(target: "mastik", "keeping the undo history of {}: {e}", path.display());
            }
        }
        session.disk = crate::library::disk::Stamp::of(path, digest);
        session.mark_saved(path.to_path_buf());
        Ok(())
    }

    /// Nobody reads a status line here; the log keeps it.
    fn tell(&mut self, line: String) {
        log::info!(target: "mastik", "serve: {line}");
    }

    /// The installed families: `serve` downloads no web font.
    fn fonts(&mut self) -> Vec<String> {
        self.fonts
            .get_or_insert_with(SystemFonts::load)
            .families()
            .to_vec()
    }

    fn canvas(&mut self, _: &Path) -> Option<(u32, u32)> {
        None
    }

    fn desk(&mut self) -> Option<&mut dyn Desk> {
        Some(self)
    }
}

impl Desk for Headless {
    fn prefs(&self) -> &crate::prefs::AssetPrefs {
        &self.asset_prefs
    }

    fn assets(&mut self) -> &mut crate::assets::Assets {
        &mut self.assets
    }

    /// Never read here: each worker reads its service's key from the keychain,
    /// as the window's do before its own read lands.
    fn keys(&self) -> Option<std::collections::BTreeMap<String, String>> {
        None
    }

    /// Each switched-on pack read from its file on this thread, as the kit
    /// tools read kits here, and kept until the file changes.
    fn packs(&mut self, own: Option<&str>) -> Vec<(crate::kits::index::KitEntry, Pack)> {
        use crate::kits::index::Load;
        self.rescan();
        let Some(lib) = self.scanned.as_ref() else {
            return Vec::new();
        };
        let on = &self.asset_prefs.icon_sets;
        let kits: Vec<_> = crate::assets::sets::icon_packs(lib, own)
            .into_iter()
            .filter(|k| on.contains(&crate::assets::sets::pack_key(&k.id)))
            .collect();
        kits.into_iter()
            .filter_map(|k| {
                let held = self.packs.get(&k.id).filter(|(at, _)| *at == k.modified);
                let pack = match held {
                    Some((_, pack)) => pack.clone(),
                    None => {
                        let pack = match crate::kits::index::read(&k.path) {
                            (Load::Ready(Some(p)), version) => Some(Arc::new(KitPack::new(
                                k.clone(),
                                version.unwrap_or(k.version),
                                p,
                            ))),
                            (Load::Ready(None), _) => None,
                            // A failed read is not kept, so the next search reads again.
                            (Load::Failed | Load::Pending, _) => return Some((k, Pack::Failed)),
                        };
                        self.packs.insert(k.id.clone(), (k.modified, pack.clone()));
                        pack
                    }
                };
                // A kit that published nothing has no icons to offer.
                pack.map(|p| (k, Pack::Ready(p)))
            })
            .collect()
    }

    /// No canvas caches pixels here: the photo's bytes are in the document,
    /// which the CPU renderer decodes.
    fn adopt(&mut self, _: &mastik_core::ImageId, _: mastik_core::peniko::ImageData) {}
}

/// Run again the asset calls waiting on a download, as the window's
/// `drain_mcp` does: once a worker landed something since `last`, else every
/// [`LATER_EVERY`]. Answers whether it ran them.
fn run_waiting(
    engine: &mut Engine,
    docs: &mut Headless,
    last: &mut Option<(u64, Instant)>,
    now: Instant,
) -> bool {
    if !engine.has_later() {
        return false;
    }
    docs.assets.pass(&docs.ctx);
    let landed = docs.assets.landed_count;
    let due =
        last.is_none_or(|(count, at)| count != landed || now.duration_since(at) >= LATER_EVERY);
    if due {
        *last = Some((landed, now));
        engine.run_later(docs);
    }
    due
}

/// When `serve` ends: its own count of the connections that called, so one whose
/// every call was refused before the engine knew it still counts, and a probe
/// that never called does not.
#[derive(Default)]
struct Life {
    callers: std::collections::HashSet<mastik_mcp::queue::Conn>,
    served: bool,
    /// When the last caller left, while none is open.
    idle: Option<Instant>,
    answered: Option<Instant>,
}

impl Life {
    fn called(&mut self, conn: mastik_mcp::queue::Conn, now: Instant) {
        self.callers.insert(conn);
        self.served = true;
        self.idle = None;
        self.answered = Some(now);
    }

    /// A waiting call was answered: its reply is flushed before a hand-over too.
    fn answered(&mut self, now: Instant) {
        self.answered = Some(now);
    }

    fn closed(&mut self, conn: mastik_mcp::queue::Conn, now: Instant) {
        if self.callers.remove(&conn) && self.callers.is_empty() {
            self.idle = Some(now);
        }
    }

    /// Whether to end with nobody left: `linger` after the last caller, or
    /// [`FIRST_WAIT`] after `started` when nobody ever called.
    fn done(&self, started: Instant, linger: Duration, now: Instant) -> bool {
        if !self.callers.is_empty() {
            return false;
        }
        match self.idle {
            Some(since) => now.duration_since(since) >= linger,
            None => !self.served && now.duration_since(started) > FIRST_WAIT,
        }
    }

    /// Whether to hand over to a window: once the last answer has had time to
    /// reach its connection, since ending the process drops an unwritten reply.
    fn hand_over(&self, now: Instant) -> bool {
        self.callers.is_empty()
            || self
                .answered
                .is_none_or(|a| now.duration_since(a) > FLUSH_WAIT)
    }
}

/// The lock only one `serve` holds, `None` when another does. Without a run
/// folder, or a lock the system will not take, it runs unguarded.
fn one_serve() -> Option<Option<std::fs::File>> {
    let Some(dir) = endpoint::run_dir() else {
        return Some(None);
    };
    let file = std::fs::create_dir_all(&dir).and_then(|()| {
        std::fs::OpenOptions::new()
            .create(true)
            .truncate(false)
            .write(true)
            .open(dir.join("serve.lock"))
    });
    let Ok(file) = file else {
        return Some(None);
    };
    match file.try_lock() {
        Ok(()) => Some(Some(file)),
        Err(std::fs::TryLockError::WouldBlock) => None,
        Err(std::fs::TryLockError::Error(_)) => Some(None),
    }
}

/// Run `mastik serve` until a window takes over or the last client leaves.
pub(crate) fn run() -> Result<(), String> {
    // One `serve` per user: two proxies starting together would otherwise each
    // start one, and two sessions of one file overwrite each other's writes.
    // The lock lasts as long as this process; a second `serve` leaves at once,
    // and its proxy connects to the first.
    let _only = match one_serve() {
        Some(lock) => lock,
        None => return Ok(()),
    };
    let (to, from) = queue::queue();
    let _endpoint = endpoint::listen(Host::Serve, to, None)
        .map_err(|e| format!("mastik serve: cannot listen: {e}"))?;
    let prefs = crate::prefs::Prefs::load();
    let mut docs = Headless::new(crate::library::root(prefs.base_folder.as_deref()));
    docs.history = prefs.history_size;
    docs.asset_prefs = prefs.assets;
    let mut engine = Engine::default();
    let started = Instant::now();
    let mut looked = Instant::now();
    let mut life = Life::default();
    let mut window = false;
    let mut later_run = None;
    loop {
        let wait = match engine.has_later() {
            true => LATER_POLL,
            false => Duration::from_millis(500),
        };
        match from.recv_timeout(wait) {
            // One its connection gave up waiting for is not run.
            Ok(Request::Call(call)) if !call.take() => {}
            Ok(Request::Call(call)) => {
                let conn = call.conn;
                let reply = call.reply.clone();
                match engine.handle_or_wait(&mut docs, call) {
                    Some(out) => {
                        let _ = reply.send(out);
                    }
                    // Kept: `run_waiting` answers it on its own reply.
                    None => later_run = Some((docs.assets.landed_count, Instant::now())),
                }
                life.called(conn, Instant::now());
            }
            // A connection that never called, such as a liveness probe from
            // `endpoint::hosts` (this process's own among them), is no client
            // leaving: counting it ended `serve` before its first call.
            Ok(Request::Closed(conn)) => {
                if engine.closed(&mut docs, conn) {
                    let used: Vec<PathBuf> = engine.in_use().map(Path::to_path_buf).collect();
                    docs.open.retain(|p, _| used.contains(p));
                }
                life.closed(conn, Instant::now());
            }
            Err(RecvTimeoutError::Timeout) => {}
            Err(RecvTimeoutError::Disconnected) => return Ok(()),
        }
        if run_waiting(&mut engine, &mut docs, &mut later_run, Instant::now()) {
            life.answered(Instant::now());
        }
        let linger = match engine.remembers_clients() {
            true => KEYED_LINGER,
            false => Duration::ZERO,
        };
        if life.done(started, linger, Instant::now()) {
            return Ok(());
        }
        if !window && looked.elapsed() > WINDOW_EVERY {
            looked = Instant::now();
            window = endpoint::hosts().iter().any(|h| h.host == Host::Window);
        }
        // A call waiting on a download would lose its answer with the process.
        if window && !engine.has_later() && life.hand_over(Instant::now()) {
            return Ok(());
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use mastik_mcp::queue::Call;
    use serde_json::json;

    /// A caller whose calls were all refused still ends `serve` when it leaves;
    /// a probe that never called does not; nobody calling ends it after the
    /// first wait.
    ///
    /// flip: `Life::closed` ignoring a conn the engine never knew → fails at
    /// "the refused caller left".
    #[test]
    fn serve_ends_when_its_last_caller_leaves() {
        let t0 = Instant::now();
        let mut life = Life::default();
        life.closed(7, t0);
        assert!(!life.done(t0, Duration::ZERO, t0), "a probe is no client");
        assert!(
            life.done(t0, Duration::ZERO, t0 + FIRST_WAIT * 2),
            "nobody came"
        );
        life.called(1, t0);
        assert!(!life.done(t0, Duration::ZERO, t0), "a caller is open");
        assert!(!life.hand_over(t0), "its answer has not been flushed");
        assert!(life.hand_over(t0 + FLUSH_WAIT * 2));
        life.closed(1, t0);
        assert!(life.done(t0, Duration::ZERO, t0), "the refused caller left");
        assert!(!life.done(t0, KEYED_LINGER, t0), "a keyed client lingers");
    }

    /// An agent's write through `serve` is not written over a file changed on
    /// disk since it was read: the outside change stays, and the call is an
    /// error, since `serve` dropped the edit with its copy.
    ///
    /// flip: `Headless::save` without its stamp check → fails at "the clash is
    /// reported"; `Engine::answer` without its dropped-copy arm → fails there too.
    #[test]
    fn serve_does_not_write_over_a_change_on_disk() {
        let dir = std::env::temp_dir().join(format!("mastik-serve-clash-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let file = dir.join("brand.mastik");
        let mut ids = mastik_core::IdSource::new(crate::session::random_actor());
        let doc = mastik_core::Document::new(ids.mint());
        let page = doc.first_page().to_wire();
        std::fs::write(&file, mastik_core::io::save(&doc).unwrap()).unwrap();
        let mut docs = Headless::new(None);
        let mut engine = Engine::default();
        let mut run = |tool: &str, args: serde_json::Value| {
            let (reply, _answer) = std::sync::mpsc::sync_channel(1);
            engine.handle(&mut docs, Call::new(1, None, tool, args, reply))
        };
        let path = file.display().to_string();
        assert!(!run("use_document", json!({ "document": path })).is_error);
        let rename = |name: &str| json!({ "node": page, "name": name });
        assert!(!run("set_name", rename("Ours")).is_error, "written");
        // Another program writes the file; a second's wait lets its time move.
        std::thread::sleep(Duration::from_millis(1100));
        let outside = mastik_core::io::save(&mastik_core::Document::new(ids.mint())).unwrap();
        std::fs::write(&file, &outside).unwrap();
        let out = run("set_name", rename("Mine"));
        assert!(out.is_error, "the clash is reported: {:?}", out.content);
        assert!(
            out.structured.is_none(),
            "no revision for an edit that exists nowhere"
        );
        assert_eq!(std::fs::read(&file).unwrap(), outside, "kept");
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// `serve` resumes a library document's kept undo history and writes it
    /// again beside each write, so a later open still finds the steps.
    ///
    /// flip: `Headless::save` not writing the history → fails at "kept";
    /// `Headless::open` not resuming it → fails at "resumed".
    #[test]
    fn serve_keeps_a_library_documents_undo_history() {
        let root = crate::kits::picker::tests::temp_root("serve-history");
        let mut ids = mastik_core::IdSource::new(crate::session::random_actor());
        let mut doc = mastik_core::Document::new(ids.mint());
        let page = doc.first_page().to_wire();
        let path = crate::library::store::file_document(&root, None, "Plan", &mut doc).unwrap();
        let id = doc.meta().id.clone().expect("filed with an id");
        let kind = mastik_core::verbs::example("NodeKind").unwrap();
        let create = json!({ "parent": page, "kind": kind, "name": "Chart" });
        let document = json!({ "document": path.display().to_string() });
        let kept = || {
            let digest = crate::library::history::digest(&std::fs::read(&path).unwrap());
            crate::library::history::read(&root, &id, &digest).map_or(0, |h| h.undo.len())
        };
        // Two runs of `serve`, each opening the file afresh.
        for run in 1..=2 {
            let mut docs = Headless::new(Some(root.clone()));
            let mut engine = Engine::default();
            let mut call = |docs: &mut Headless, tool: &str, args: serde_json::Value| {
                let (reply, _answer) = std::sync::mpsc::sync_channel(1);
                let out = engine.handle(docs, Call::new(1, None, tool, args, reply));
                assert!(!out.is_error, "{tool}: {:?}", out.content);
            };
            call(&mut docs, "use_document", document.clone());
            if run == 2 {
                let depth = docs.open.values().next().unwrap().history.undo_depth();
                assert_eq!(depth, 1, "resumed");
            }
            call(&mut docs, "create_node", create.clone());
            assert_eq!(kept(), run, "kept");
        }
        let _ = std::fs::remove_dir_all(&root);
    }

    /// `serve` answers `search_assets` and `place_asset` from the sets and
    /// services the person switched on, and a photo search waiting on the
    /// network is answered once its page lands rather than refused.
    ///
    /// flip: `Headless::desk` answering `None` → fails at "an icon found";
    /// `run_waiting` not running the kept calls → fails at "the photo search
    /// answered".
    #[test]
    fn serve_answers_the_asset_tools_from_what_is_switched_on() {
        use crate::assets::SetState;
        use crate::assets::photos::{Orientation, Page, Photo};
        use crate::assets::sets::{self, IconSet, SetSpec};
        let dir = std::env::temp_dir().join(format!("mastik-serve-assets-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let file = dir.join("page.mastik");
        let mut ids = mastik_core::IdSource::new(crate::session::random_actor());
        let doc = mastik_core::Document::new(ids.mint());
        let page = doc.first_page().to_wire();
        std::fs::write(&file, mastik_core::io::save(&doc).unwrap()).unwrap();
        let mut docs = Headless::new(None);
        let spec = SetSpec::by_key("font-awesome").unwrap();
        let set = IconSet {
            icons: vec![sets::Icon {
                name: "bell".into(),
                variant: 0,
                body: r#"<path fill="currentColor" d="M0 0h32v32H0z"/>"#.into(),
                view: [0.0, 0.0, 32.0, 32.0],
                aliases: Vec::new(),
            }],
            grid: 32.0,
            variant_grids: Vec::new(),
        };
        docs.assets
            .sets
            .insert(spec.key, SetState::Ready(Arc::new(set)));
        docs.asset_prefs.icon_sets = vec![spec.key.into()];
        docs.asset_prefs.photo_services = vec!["unsplash".into()];
        let mut engine = Engine::default();
        let call = |engine: &mut Engine, docs: &mut Headless, tool: &str, args| {
            let (reply, answer) = std::sync::mpsc::sync_channel(1);
            (
                engine.handle_or_wait(docs, Call::new(1, None, tool, args, reply)),
                answer,
            )
        };
        let document = json!({ "document": file.display().to_string() });
        call(&mut engine, &mut docs, "use_document", document);

        let icons = json!({ "queries": ["bell"], "kind": "icons" });
        let out = call(&mut engine, &mut docs, "search_assets", icons)
            .0
            .unwrap();
        assert!(!out.is_error, "an icon found: {:?}", out.content);
        let found = out.structured.unwrap();
        let asset = found["results"][0]["assets"][0]["asset"].clone();
        assert_eq!(
            asset
                .as_str()
                .map(|a| a.starts_with("icon:font-awesome/bell/")),
            Some(true)
        );
        let place = json!({ "asset": asset, "parent": page });
        let out = call(&mut engine, &mut docs, "place_asset", place)
            .0
            .unwrap();
        assert!(!out.is_error, "placed: {:?}", out.content);
        let s = docs.open.values().next().unwrap();
        assert_eq!(s.history.done().last().unwrap().note.label, "Insert icon");

        let photos = json!({ "queries": ["coffee"], "kind": "photos" });
        let (out, answer) = call(&mut engine, &mut docs, "search_assets", photos);
        assert!(out.is_none(), "the search waits on the network: {out:?}");
        let mut last = Some((docs.assets.landed_count, Instant::now()));
        let photo = Photo {
            service: "unsplash",
            id: "x1".into(),
            size: Some((8, 6)),
            thumb: "https://i/thumb".into(),
            urls: [
                "https://i/small".into(),
                "https://i/regular".into(),
                "https://i/full".into(),
            ],
            title: "a coffee cup".into(),
            author: "Ana Ruiz".into(),
            author_url: "https://unsplash.com/@ana".into(),
            page_url: "https://unsplash.com/photos/x1".into(),
            licence: "Unsplash License".into(),
            licence_url: "https://unsplash.com/license".into(),
            report_url: None,
        };
        let thumb = photo.thumb.clone();
        let page = Page {
            photos: vec![photo],
            total: 1,
            more: false,
        };
        let key = ("unsplash", "coffee".to_owned(), Orientation::Any);
        docs.assets.agent_land_search(key, Ok(page));
        docs.assets
            .agent_land_thumb(&thumb, Some(vec![0xff, 0xd8, 0xff]));
        run_waiting(&mut engine, &mut docs, &mut last, Instant::now());
        let out = answer.try_recv().expect("the photo search answered");
        assert!(!out.is_error, "{:?}", out.content);
        let found = out.structured.unwrap();
        assert_eq!(
            found["results"][0]["assets"][0]["asset"],
            "photo:unsplash/x1"
        );
        assert!(!engine.has_later());
        let _ = std::fs::remove_dir_all(&dir);
    }
}
