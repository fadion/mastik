//! The MCP host (§8): the tool handlers, run on the thread that owns the
//! documents, the window's or `serve`'s.
//!
//! [`Engine`] keeps each connection's current document and what it last showed
//! that connection, and answers each call against an [`EditorSession`] lent by
//! the host through [`Documents`]. Writes go through `EditorSession::try_commit`,
//! the door the app's own edits use.

pub(crate) mod access;
mod actions;
mod agent_log;
pub(crate) mod assets;
mod context;
mod describe;
mod diff;
pub(crate) mod endpoint;
mod group;
mod inspect;
pub(crate) mod kits;
pub(crate) mod propose;
pub(crate) mod proxy;
mod reads;
mod revisions;
pub(crate) mod serve;
mod set;
mod short;
pub(crate) mod system_fonts;
pub(crate) mod window;
mod writes;

use crate::library::state::Library;
use crate::session::EditorSession;
use describe::{describe_tools, no_such_tool};
use mastik_core::NodeId;
use mastik_mcp::queue::{Call, Conn};
use mastik_mcp::wire::{Content, Output};
use revisions::{
    Scope, Shown, catch_up, delta, last_layers, layers_of, remember, since, snapshot_of, stale,
    stamp,
};
use serde_json::{Map, Value, json};
use set::set;
use std::collections::{HashMap, VecDeque};
use std::path::{Path, PathBuf};
use writes::{READ_ONLY, batch, write};

/// What a host lends the engine: its documents, by absolute path.
pub(crate) trait Documents {
    /// Find the document `wanted` names and open it if it is not open,
    /// answering its absolute path, or why not (several candidates, none).
    fn open(&mut self, wanted: &str) -> Result<PathBuf, String>;
    /// The open document at `path`.
    fn session(&mut self, path: &Path) -> Option<&mut EditorSession>;
    /// The document at `path` changed through a tool: `serve` writes it, as the
    /// window does a tab behind. An error is why the edit is not on disk; a
    /// host that closed the document with it has dropped the edit too.
    fn committed(&mut self, path: &Path) -> Result<(), String>;
    /// The library as it now stands on disk, or `None` with no base folder.
    fn library(&mut self) -> Option<&Library>;
    /// The open document at `path` and the library together, for a kit tool
    /// that reads both.
    fn with_library(&mut self, path: &Path) -> Option<(&mut EditorSession, Option<&Library>)>;
    /// Write the document at `path` now, as the kit mark's changes are.
    fn save(&mut self, path: &Path) -> Result<(), String>;
    /// Put `line` on the status line, where a person would read it.
    fn tell(&mut self, line: String);
    /// Every font family a text here can be set in, sorted.
    fn fonts(&mut self) -> Vec<String>;
    /// The canvas's size in device pixels while the document at `path` is the
    /// tab in front of the person; `None` behind and in `serve`, where nobody
    /// looks at it.
    fn canvas(&mut self, path: &Path) -> Option<(u32, u32)>;
    /// A tool that may write is about to run on the document at `path`: the
    /// window ends the person's text edit there first, so the typing lands as
    /// its own step rather than being lost under the agent's.
    fn before_write(&mut self, _path: &Path) {}
    /// The assets browser's providers: the window's, or `serve`'s read from the
    /// preferences.
    fn desk(&mut self) -> Option<&mut dyn assets::Desk> {
        None
    }
}

/// The document `wanted` names: an absolute path; else a name or relative path
/// matched against the open documents, then the library's. Several matches and
/// none are errors the agent can show the person.
pub(crate) fn resolve(
    wanted: &str,
    open: &[PathBuf],
    library: Option<&Path>,
) -> Result<PathBuf, String> {
    let asked = Path::new(wanted);
    if asked.is_absolute() {
        return match asked.is_file() {
            true => Ok(asked.to_path_buf()),
            false => Err(format!("No file at {wanted}.")),
        };
    }
    let key = wanted
        .trim_end_matches(".mastik")
        .to_lowercase()
        .replace('\\', "/");
    let matches = |path: &Path, name: Option<&str>| {
        let p = path.to_string_lossy().to_lowercase().replace('\\', "/");
        let p = p.trim_end_matches(".mastik");
        p.ends_with(&format!("/{key}")) || name.is_some_and(|n| n.to_lowercase() == key)
    };
    let mut found: Vec<PathBuf> = open.iter().filter(|p| matches(p, None)).cloned().collect();
    if found.is_empty()
        && let Some(root) = library
    {
        found = crate::library::scan::scan(root)
            .into_iter()
            .filter(|e| matches(&e.path, e.meta.name.as_deref()))
            .map(|e| e.path)
            .collect();
    }
    found.sort();
    found.dedup();
    match found.len() {
        1 => Ok(found.remove(0)),
        0 => Err(format!(
            "No document called `{wanted}` is open or in the library. Ask the person for its path."
        )),
        _ => Err(format!(
            "Several documents match `{wanted}`; ask the person which: {}",
            found
                .iter()
                .map(|p| p.display().to_string())
                .collect::<Vec<_>>()
                .join(", ")
        )),
    }
}

/// One client's state: its current document and what it was last shown.
#[derive(Default)]
struct Connection {
    document: Option<PathBuf>,
    /// Newest last.
    shown: VecDeque<Shown>,
    /// When it last called, in [`Engine::tick`]s: the least recent keyed client
    /// is forgotten first.
    used: u64,
    /// The `group_steps` group it has open, on its current document.
    group: Option<group::Group>,
    /// Its exports packing on a worker, each answered to its call when done.
    exports: Vec<Exporting>,
}

/// An export call's file packing on a worker ([`reads::export_work`]).
struct Exporting {
    call: Call,
    done: std::sync::mpsc::Receiver<Result<Answer, String>>,
}

/// The tools whose file packs on a worker while the call waits.
const EXPORTS: [&str; 2] = ["export_prototype", "export_flow_map"];

/// Whether `call` is one of [`EXPORTS`], named or through `call`.
fn export_call(call: &Call) -> bool {
    let tool = match call.tool.as_str() {
        "call" => call.args.get("tool").and_then(Value::as_str),
        tool => Some(tool),
    };
    tool.is_some_and(|t| EXPORTS.contains(&t))
}

/// Whose state a call reads: a keyed client's, which outlives its connections,
/// or an unkeyed connection's, which ends with it.
#[derive(Clone, PartialEq, Eq, Hash)]
enum Who {
    Key(String),
    Conn(Conn),
}

/// Whose state `call` reads.
fn who(call: &Call) -> Who {
    match &call.key {
        Some(key) => Who::Key(key.clone()),
        None => Who::Conn(call.conn),
    }
}

/// The asset tool `call` runs, named or through `call`.
fn assets_tool(call: &Call) -> Option<&'static str> {
    let (tool, args) = match call.tool.as_str() {
        "call" => (
            call.args.get("tool").and_then(Value::as_str)?,
            call.args.get("args").unwrap_or(&Value::Null),
        ),
        tool => (tool, &call.args),
    };
    // A proposal that places an asset waits on its download too.
    if tool == "propose_edit" && !placements(args).is_empty() {
        return Some("propose_edit");
    }
    assets::TOOLS.iter().copied().find(|t| *t == tool)
}

/// A `propose_edit`'s `place_asset` calls: each one's number in `calls`, from
/// 1, and its arguments.
fn placements(args: &Value) -> Vec<(usize, Value)> {
    args.get("calls")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .enumerate()
        .filter(|(_, c)| propose::call_name(c) == Some(propose::PLACE))
        .map(|(i, c)| (i + 1, c.get("args").cloned().unwrap_or_else(|| json!({}))))
        .collect()
}

/// The client `call` comes from as its steps record it
/// ([`mastik_core::StepNote::agent`]): its key, or its connection in this
/// process, so an unkeyed client's steps stop being its own when it reconnects,
/// as its revisions do.
fn agent(call: &Call) -> String {
    match who(call) {
        Who::Key(key) => keyed_agent(&key),
        Who::Conn(conn) => format!("conn:{}:{conn}", std::process::id()),
    }
}

/// [`agent`] for the client that gave `key`.
fn keyed_agent(key: &str) -> String {
    format!("key:{key}")
}

/// How many keyed clients a host remembers after their connections close. A
/// harness that starts a process per turn comes back on a new connection each
/// turn with the same key.
const CLIENTS: usize = 32;

/// How many proposals a host keeps for the person to apply; the oldest goes
/// first.
const PROPOSALS: usize = 64;

/// The tool handlers and every client's state.
#[derive(Default)]
pub(crate) struct Engine {
    clients: HashMap<Who, Connection>,
    /// The connections open now, which `serve` waits on.
    live: std::collections::HashSet<Conn>,
    /// The key each open connection called with, so a keyed client's open
    /// group closes when its last connection does.
    keys: HashMap<Conn, String>,
    tick: u64,
    /// The levels the window granted its chats.
    grants: access::Grants,
    /// What the chats proposed, newest last.
    proposals: VecDeque<propose::Proposal>,
    /// The last proposal's id.
    proposed: u64,
    /// Where a granted chat's calls are written.
    log: agent_log::Log,
    /// Asset calls waiting on a download, run again as downloads land.
    later: Vec<Later>,
    /// Set while a call is answered, when it must wait for a download.
    waiting: bool,
    /// What a placement answered with a pending id finally answered, by that
    /// id, newest last.
    finished: VecDeque<(String, Output)>,
    /// The last pending id handed out.
    pending: u64,
}

/// An asset call waiting on the network.
struct Later {
    call: Call,
    /// When it was first taken.
    since: std::time::Instant,
    /// The id it was answered with once it waited past [`assets::WAIT`]; it
    /// keeps running, and its answer is kept under the id.
    pending: Option<String>,
}

/// How many finished placements are kept for `place_asset`'s `pending`.
const FINISHED: usize = 16;

impl Engine {
    /// Forget a connection that closed, and its state unless a key holds it.
    /// A group the client left open closes: an unkeyed client's with its
    /// connection, a keyed one's with the last connection it called on.
    /// Answers whether it had called: a connection that only looked whether the
    /// endpoint answers, as `endpoint::hosts` does, is no client leaving.
    pub(crate) fn closed(&mut self, docs: &mut impl Documents, conn: Conn) -> bool {
        let mut open = self
            .clients
            .remove(&Who::Conn(conn))
            .and_then(|mut c| c.group.take());
        if let Some(key) = self.keys.remove(&conn)
            && !self.keys.values().any(|k| *k == key)
        {
            open = open.or_else(|| {
                self.clients
                    .get_mut(&Who::Key(key))
                    .and_then(|c| c.group.take())
            });
        }
        if let Some(g) = open {
            group::close(docs, g);
        }
        self.live.remove(&conn)
    }

    /// The assistant's turn for the chat keyed `key` has ended: a group it left
    /// open closes, as one a disconnecting client leaves.
    pub(crate) fn turn_ended(&mut self, docs: &mut impl Documents, key: &str) {
        let open = self
            .clients
            .get_mut(&Who::Key(key.to_owned()))
            .and_then(|c| c.group.take());
        if let Some(g) = open {
            group::close(docs, g);
        }
    }

    /// The documents some client is using, for a host deciding what it may close.
    pub(crate) fn in_use(&self) -> impl Iterator<Item = &Path> {
        self.clients.values().filter_map(|c| c.document.as_deref())
    }

    /// Whether a keyed client's state is held, which a host keeps for its next
    /// connection.
    pub(crate) fn remembers_clients(&self) -> bool {
        self.clients.keys().any(|w| matches!(w, Who::Key(_)))
    }

    /// The state `call` reads and writes, made when new; the least recently
    /// used keyed client is forgotten past [`CLIENTS`].
    fn client(&mut self, call: &Call) -> &mut Connection {
        self.live.insert(call.conn);
        if let Some(key) = &call.key {
            self.keys.insert(call.conn, key.clone());
        }
        self.tick += 1;
        let who = who(call);
        if !self.clients.contains_key(&who) && matches!(who, Who::Key(_)) {
            let keyed = self
                .clients
                .iter()
                .filter(|(w, _)| matches!(w, Who::Key(_)));
            if keyed.clone().count() >= CLIENTS
                && let Some(oldest) = keyed.min_by_key(|(_, c)| c.used).map(|(w, _)| w.clone())
            {
                self.clients.remove(&oldest);
            }
        }
        let tick = self.tick;
        let client = self.clients.entry(who).or_default();
        client.used = tick;
        client
    }

    /// The levels granted to the window's chats, shared with its endpoint.
    pub(crate) fn grants(&self) -> access::Grants {
        self.grants.clone()
    }

    /// The proposal called `id`, while it is among the last [`PROPOSALS`].
    pub(crate) fn proposal(&self, id: u64) -> Option<&propose::Proposal> {
        self.proposals.iter().find(|p| p.id == id)
    }

    /// [`Self::handle_or_wait`] for a test that wants an answer now: a call that
    /// would wait on a download is refused.
    #[cfg(test)]
    pub(crate) fn handle(&mut self, docs: &mut impl Documents, call: Call) -> Output {
        self.handle_or_wait(docs, call).unwrap_or_else(|| {
            // The one `handle_or_wait` just kept.
            self.later.pop();
            Output::error("It is still downloading; call again shortly.")
        })
    }

    /// Answer one call, or `None` when it waits on a download: it is kept, and
    /// [`Self::run_later`] answers it on its own reply. A chat granted a level,
    /// by its key or by the process calling, acts under the chat's key, is
    /// refused a tool above its level, and has the call written to the agent
    /// log. A chat's key this window holds no grant for (a chat of an earlier
    /// run, or one ended long ago) is refused rather than answered as an
    /// ungranted client's.
    pub(crate) fn handle_or_wait(
        &mut self,
        docs: &mut impl Documents,
        call: Call,
    ) -> Option<Output> {
        // Only an asset call or an export waits, and the rest can carry megabytes
        // of arguments.
        let again = (assets_tool(&call).is_some() || export_call(&call)).then(|| call.again());
        self.waiting = false;
        let out = self.handle_now(docs, call);
        let (true, Some(again)) = (std::mem::take(&mut self.waiting), again) else {
            return Some(out);
        };
        self.later.push(Later {
            call: again,
            since: std::time::Instant::now(),
            pending: None,
        });
        None
    }

    /// Whether some call waits on a download.
    pub(crate) fn has_later(&self) -> bool {
        !self.later.is_empty()
    }

    /// Run again each call waiting on a download, answering those that no longer
    /// wait. A placement still waiting past [`assets::WAIT`] is answered with a
    /// pending id and keeps running; a search is answered as not done.
    pub(crate) fn run_later(&mut self, docs: &mut impl Documents) {
        for mut l in std::mem::take(&mut self.later) {
            let again = l.call.again();
            self.waiting = false;
            let out = self.handle_now(docs, l.call);
            let waits = std::mem::take(&mut self.waiting);
            if !waits {
                match l.pending {
                    None => {
                        let _ = again.reply.send(out);
                    }
                    Some(id) => {
                        self.finished.push_back((id, out));
                        if self.finished.len() > FINISHED {
                            self.finished.pop_front();
                        }
                    }
                }
                continue;
            }
            // An export waits for its worker, however long the packing takes.
            if l.pending.is_none()
                && assets_tool(&again).is_some()
                && l.since.elapsed() >= assets::WAIT
            {
                // The download goes on; a proposal asked again finds it.
                if assets_tool(&again) == Some("propose_edit") {
                    let _ = again.reply.send(Output::error(format!(
                        "The photo did not download within {} s, so nothing was proposed. It goes on downloading: propose again shortly.",
                        assets::WAIT.as_secs()
                    )));
                    continue;
                }
                if assets_tool(&again) == Some("search_assets") {
                    let _ = again.reply.send(Output::error(format!(
                        "The search did not answer within {} s: the service is slow or out of reach. Call again shortly.",
                        assets::WAIT.as_secs()
                    )));
                    continue;
                }
                self.pending += 1;
                let id = format!("p{}", self.pending);
                let _ = again.reply.send(Output::json(json!({
                    "pending": id,
                    "said": "The photo is still downloading and lands when it arrives; call `place_asset` with `pending` for what it made.",
                })));
                l.pending = Some(id);
            }
            l.call = again;
            self.later.push(l);
        }
    }

    /// [`Self::handle_or_wait`] with nothing kept: `self.waiting` says whether
    /// it waits.
    fn handle_now(&mut self, docs: &mut impl Documents, mut call: Call) -> Output {
        parse_json_text(&mut call);
        let Some((key, level)) = self.grants.resolve(call.key.as_deref(), call.pid) else {
            if call
                .key
                .as_deref()
                .is_some_and(crate::assistant::is_client_key)
            {
                return Output::error(access::CHAT_ENDED);
            }
            return self.answer(docs, call);
        };
        call.key = Some(key.clone());
        let tool = match call.tool.as_str() {
            "call" => call.args.get("tool").and_then(Value::as_str),
            tool => Some(tool),
        }
        .unwrap_or("call")
        .to_owned();
        let out = match level.map(|level| access::refused(level, &call)) {
            None => Output::error(access::CHAT_ENDED),
            Some(Some(why)) => Output::error(why),
            Some(None) => self.answer(docs, call),
        };
        let document = self
            .clients
            .get(&Who::Key(key.clone()))
            .and_then(|c| c.document.clone());
        self.log
            .write(&tool, document.as_deref(), &key, !out.is_error);
        out
    }

    /// Answer one call that its level allows, in short ids when the client
    /// asked for them and is no chat: the panel links, keeps and replays a
    /// chat's ids in full.
    fn answer<D: Documents>(&mut self, docs: &mut D, call: Call) -> Output {
        if !call.short_ids
            || call
                .key
                .as_deref()
                .is_some_and(crate::assistant::is_client_key)
        {
            return self.answer_ids(docs, call);
        }
        // Read against the document the call starts in, written against the
        // one it ends in: `use_document` may change it.
        let whose = who(&call);
        let known = |engine: &mut Self, docs: &mut D| {
            engine
                .clients
                .get(&whose)
                .and_then(|c| c.document.clone())
                .and_then(|path| docs.session(&path))
                .map(|s| short::known_actors(&s.doc, s.ids.actor()))
                .unwrap_or_default()
        };
        let before = known(self, docs);
        let mut call = call;
        short::expand_sources(&mut call.args, &before);
        let mut out = mastik_core::with_short_ids(&before, || self.answer_ids(docs, call));
        short::shorten(&mut out, &known(self, docs));
        out
    }

    /// [`Self::answer`], with ids as the client wrote them read already.
    fn answer_ids(&mut self, docs: &mut impl Documents, call: Call) -> Output {
        // `call` is the named tool's call itself, so it is checked and answered
        // as that tool, listed or not.
        let mut call = call;
        if call.tool == "call" {
            let Some(tool) = call.args.get("tool").and_then(Value::as_str) else {
                return Output::error("`call` needs `tool`, a tool's name");
            };
            if tool == "call" {
                return Output::error("`call` runs another tool, not itself");
            }
            let args = call.args.get("args").cloned().unwrap_or_else(|| json!({}));
            call.tool = tool.to_owned();
            call.args = args;
        }
        // Before the arguments are checked, which would refuse a name as no id.
        let path = self.client(&call).document.clone();
        if let Some(s) = path.and_then(|p| docs.session(&p))
            && let Err(why) = pages_by_name(s, &call.tool, &mut call.args)
        {
            return Output::error(why);
        }
        if let Err(why) = check_call(&call) {
            return Output::error(why);
        }
        if call.tool == "export_png"
            && call.key.is_some()
            && let Some(map) = call.args.as_object_mut()
        {
            map.entry("max").or_insert(json!(CHAT_PNG_MAX));
        }
        // Ids start from the launch's time, so a card saved in a thread before
        // a restart never names a proposal made after it.
        if self.proposed == 0 {
            self.proposed = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map_or(1, |d| d.as_millis() as u64 * 1000);
        }
        let next = self.proposed + 1;
        let conn = self.client(&call);
        if call.tool == "use_document" {
            let Some(wanted) = call.args.get("document").and_then(Value::as_str) else {
                return Output::error("`use_document` needs `document`");
            };
            return match docs.open(wanted) {
                Ok(path) => {
                    // A group's steps are one document's history.
                    if conn.document.as_deref() != Some(path.as_path())
                        && let Some(g) = conn.group.take()
                    {
                        group::close(docs, g);
                    }
                    let Some(session) = docs.session(&path) else {
                        return Output::error(format!("{} did not open", path.display()));
                    };
                    let snap = snapshot_of(session);
                    let rev = session.revision();
                    // The document already in use keeps what this client was
                    // shown, so `since`, `base_rev` and `changed` carry across a
                    // reconnect. The proxy's own restore reports no `changed`,
                    // leaving it for the agent's next call.
                    let mut changed = None;
                    if conn.document.as_deref() == Some(path.as_path()) {
                        if call.args.get("restore") != Some(&json!(true)) {
                            changed = catch_up(conn, session);
                        }
                    } else {
                        conn.document = Some(path.clone());
                        conn.shown.clear();
                        let top = session.history.current();
                        let tables = revisions::tables_of(session);
                        remember(conn, rev, layers_of(session), Vec::new(), top, tables);
                    }
                    let pages: Vec<Value> = snap
                        .pages
                        .iter()
                        .map(|p| json!({ "id": p.id, "name": p.name }))
                        .collect();
                    let mut answer = json!({
                        "rev": rev,
                        "document": path.display().to_string(),
                        "pages": pages,
                        "layers": snap.nodes.len(),
                    });
                    if let Some(c) = changed {
                        answer["changed"] = c;
                    }
                    Output::json(answer)
                }
                Err(why) => Output::error(why),
            };
        }
        if call.tool == "list_fonts" {
            return Output::json(fonts(docs, &call.args));
        }
        if call.tool == "describe_tools" {
            // `handle` wrote a granted chat's key onto its call.
            let level = self
                .grants
                .resolve(call.key.as_deref(), None)
                .and_then(|(_, level)| level);
            return Output::json(describe_tools(&call.args, level));
        }
        if kits::WITHOUT_DOCUMENT.contains(&call.tool.as_str()) {
            let answer = kits::without_document(docs, &call)
                .and_then(|v| read_fits(&call.tool, Answer::Json(v)));
            return match answer {
                Ok(Answer::Json(v)) => Output::json(v),
                Ok(_) => Output::error("an answer that is not JSON"),
                Err(why) => Output::error(why),
            };
        }
        if call.tool == "search_assets" {
            let own = conn
                .document
                .clone()
                .and_then(|p| docs.session(&p).and_then(|s| s.doc.meta().id.clone()));
            return match assets::search(docs, &call.args, own.as_deref()) {
                Ok(assets::Step::Done(a)) => output_of(Ok(a)),
                Ok(assets::Step::Wait) => {
                    self.waiting = true;
                    Output::error("waiting on a download")
                }
                Err(why) => Output::error(why),
            };
        }
        if call.tool == "place_asset" {
            if let Some(id) = call.args.get("pending").and_then(Value::as_str) {
                return self.pending_answer(id);
            }
            if assets::waits(docs, conn.document.clone().as_deref(), &call.args) {
                self.waiting = true;
                return Output::error("waiting on a download");
            }
        }
        let Some(path) = conn.document.clone() else {
            return Output::error(
                "No document yet: call `use_document` with the file the person named.",
            );
        };
        let canvas = match call.tool.as_str() {
            "get_view" => docs.canvas(&path),
            _ => None,
        };
        // Core cannot see the host's fonts: a family nobody can set text in would
        // draw every replaced layer in a fallback. Written in the host's spelling,
        // whether the call is `replace_font` or one step of a batch or proposal.
        let fonts_checked = match call.tool.as_str() {
            "replace_font" => host_font(docs, &mut call.args),
            "batch" | "propose_edit" => {
                // A batch's steps name their `tool`, a proposal's calls their
                // `verb` or `tool`.
                let list = match call.tool == "batch" {
                    true => "steps",
                    false => "calls",
                };
                let steps = call.args.get_mut(list).and_then(Value::as_array_mut);
                steps
                    .into_iter()
                    .flatten()
                    .try_for_each(|step| match propose::call_name(step) {
                        Some("replace_font") => match step.get_mut("args") {
                            Some(args) => host_font(docs, args),
                            None => Ok(()),
                        },
                        _ => Ok(()),
                    })
            }
            _ => Ok(()),
        };
        if let Err(why) = fonts_checked {
            return Output::error(why);
        }
        // A proposal's placements are made ready first, each photo downloaded,
        // so the proposal holds them and applying it reaches no network.
        let mut held = Vec::new();
        if call.tool == "propose_edit" {
            for (n, args) in placements(&call.args) {
                let ready = match assets::waits(docs, Some(&path), &args) {
                    true => Ok(assets::Holding::Wait),
                    false => assets::hold(docs, &path, &args),
                };
                match ready {
                    Ok(assets::Holding::Ready(h)) => held.push(*h),
                    Ok(assets::Holding::Wait) => {
                        self.waiting = true;
                        return Output::error("waiting on a download");
                    }
                    Err(why) => {
                        return Output::error(format!(
                            "call {n} (`place_asset`): {why}. Nothing was proposed."
                        ));
                    }
                }
            }
        }
        if mastik_mcp::schema::class(&call.tool) != mastik_mcp::schema::Class::Read {
            docs.before_write(&path);
        }
        let Some(session) = docs.session(&path) else {
            conn.document = None;
            conn.shown.clear();
            conn.group = None;
            return Output::error(format!(
                "{} is no longer open: call `use_document` again.",
                path.display()
            ));
        };
        // What moved since this connection last looked, the person's edits and
        // undos included, reported before the call runs.
        let before = session.revision();
        let changed = catch_up(conn, session);
        let mark = session.mark();
        session.mcp_agent = Some(agent(&call));
        let mut proposed = None;
        let mut held_redo = None;
        let mut wait = false;
        let answer = if call.tool == "place_asset" {
            match assets::place(docs, &path, &call) {
                Ok(assets::Step::Done(a)) => Ok(a),
                Ok(assets::Step::Wait) => {
                    wait = true;
                    Ok(Answer::Json(Value::Null))
                }
                Err(why) => Err(why),
            }
        } else if call.tool == "propose_edit" {
            let title = call.args["title"].as_str().unwrap_or_default();
            propose::calls(&call.args)
                .and_then(|calls| propose::check(session, &call.client, title, calls, held))
                .map(|mut p| {
                    p.id = next;
                    p.key.clone_from(&call.key);
                    p.document.clone_from(&path);
                    let answer = json!({ "proposal": p.id, "changes": p.changes });
                    proposed = Some(p);
                    Answer::Json(answer)
                })
                .map_err(|why| format!("{why}. Nothing was proposed."))
        } else if kits::is_kit_tool(&call.tool) {
            kits::run(docs, &path, &call)
        } else if actions::TOOLS.contains(&call.tool.as_str()) {
            actions::run(docs, &path, &call)
        } else if EXPORTS.contains(&call.tool.as_str()) {
            // Checked now, packed on a worker, answered when it is done: packing
            // the player would hold the window's frame for seconds.
            match conn.exports.iter().position(|e| e.call.is(&call)) {
                Some(i) => match conn.exports[i].done.try_recv() {
                    Ok(answer) => {
                        conn.exports.remove(i);
                        answer
                    }
                    Err(std::sync::mpsc::TryRecvError::Empty) => {
                        wait = true;
                        Ok(Answer::Json(Value::Null))
                    }
                    Err(std::sync::mpsc::TryRecvError::Disconnected) => {
                        conn.exports.remove(i);
                        Err("The export stopped before it finished".into())
                    }
                },
                None => match reads::export_work(&call.tool, session) {
                    Some(Ok(work)) => {
                        let (send, done) = std::sync::mpsc::channel();
                        std::thread::Builder::new()
                            .name("mastik-mcp-export".into())
                            .spawn(move || {
                                let _ = send.send(work());
                            })
                            .map_err(|e| format!("The export could not start: {e}"))
                            .map(|_| {
                                conn.exports.push(Exporting {
                                    call: call.again(),
                                    done,
                                });
                                wait = true;
                                Answer::Json(Value::Null)
                            })
                    }
                    Some(Err(why)) => Err(why),
                    None => Err(format!("`{}` is no export", call.tool)),
                },
            }
        } else if call.tool == "get_view" {
            Ok(Answer::Json(view(session, canvas)))
        } else if call.tool == "group_steps" {
            group::run(conn, session, &path, agent(&call), &call.client, &call.args)
        } else if matches!(call.tool.as_str(), "batch" | "set") {
            // Each step's commit clears the person's redo; one that ends refused,
            // or lands nothing, gives it back below.
            held_redo = Some(session.history.take_redo());
            run(conn, session, &call)
        } else {
            run(conn, session, &call)
        };
        let answer = answer.and_then(|a| read_fits(&call.tool, a));
        let Some(session) = docs.session(&path) else {
            return Output::error(format!("{} closed during the call", path.display()));
        };
        session.mcp_agent = None;
        // A refused `batch` or `set` takes back the steps it made, and leaves the
        // revision where it was, so no client sees a change that is not there.
        // Only these two: they change the document through the history alone,
        // which is what `rewind` checks.
        if answer.is_err() && matches!(call.tool.as_str(), "batch" | "set") {
            session.rewind(&mark);
        }
        if let Some(redo) = held_redo
            && (answer.is_err() || session.revision() == before)
        {
            session.history.restore_redo(redo);
        }
        let after = session.revision();
        let mut output = output_of(answer);
        if after != before {
            // The call's own edit is not news to the caller; an undo's or redo's
            // layers are, since it did not choose them.
            let now = layers_of(session);
            let own =
                last_layers(conn).map(|(then, from)| delta(then, &now).to_json(&now, then, from));
            let tables = revisions::tables_of(session);
            remember(
                conn,
                after,
                now,
                Vec::new(),
                session.history.current(),
                tables,
            );
            if matches!(call.tool.as_str(), "undo" | "redo")
                && let (Some(own), Some(Value::Object(map))) = (own, output.structured.as_mut())
            {
                map.insert("layers".into(), own);
            }
            if let Err(why) = docs.committed(&path) {
                // A host that dropped its copy rather than write it (`serve`,
                // finding the file changed outside) holds the edit nowhere.
                if docs.session(&path).is_none() {
                    return Output::error(why);
                }
                let line = format!("The edit is made but not written to the file: {why}");
                match output.structured.as_mut() {
                    Some(Value::Object(map)) => {
                        map.insert("unsaved".into(), json!(line));
                    }
                    _ => output.content.push(Content::Text(line)),
                }
            }
        }
        // A joined history is written where a tab behind or `serve` keeps it.
        if call.tool == "group_steps" && !output.is_error {
            let _ = docs.committed(&path);
        }
        stamp(&mut output, after, changed);
        if let Some(p) = proposed {
            self.proposed = p.id;
            self.proposals.push_back(p);
            if self.proposals.len() > PROPOSALS {
                self.proposals.pop_front();
            }
        }
        if wait {
            self.waiting = true;
        }
        output
    }

    /// `place_asset` with `pending`: what the placement made once it landed,
    /// answered once, or that it still downloads.
    fn pending_answer(&mut self, id: &str) -> Output {
        if let Some(i) = self.finished.iter().position(|(p, _)| p == id)
            && let Some((_, out)) = self.finished.remove(i)
        {
            return out;
        }
        if self.later.iter().any(|l| l.pending.as_deref() == Some(id)) {
            return Output::json(json!({ "pending": id, "said": "Still downloading." }));
        }
        Output::error(format!(
            "No placement is pending as `{id}`: its answer was read already, or it never was"
        ))
    }
}

/// A handler's answer as the client is sent it.
fn output_of(answer: Result<Answer, String>) -> Output {
    match answer {
        Ok(Answer::Json(v)) => Output::json(v),
        Ok(Answer::Image(bytes)) => Output {
            content: vec![Content::Image {
                bytes,
                mime: "image/png",
            }],
            structured: None,
            is_error: false,
            meta: None,
        },
        Ok(Answer::Files(report, files)) => {
            let mut output = Output::json(report);
            output.content.extend(files);
            output
        }
        Err(why) => Output::error(why),
    }
}

/// `replace_font`'s `to`, refused unless a family the host can set text in, and
/// written in the host's spelling.
fn host_font(docs: &mut impl Documents, args: &mut Value) -> Result<(), String> {
    let Some(to) = args.get("to").and_then(Value::as_str) else {
        return Ok(());
    };
    let to = to.trim().to_owned();
    let key = to.to_lowercase();
    match docs.fonts().into_iter().find(|f| f.to_lowercase() == key) {
        Some(family) => {
            args["to"] = json!(family);
            Ok(())
        }
        None => Err(format!(
            "`to`: no family called {to} can be set here; `list_fonts` names those that can"
        )),
    }
}

/// Refuse a tool no host serves, before anything else, and a table tool's
/// arguments as a verb's are refused: unknown, missing or of the wrong kind. A
/// `"12"` for a `since` read as absent answered in full without a word.
fn check_call(call: &Call) -> Result<(), String> {
    if mastik_core::verbs::find(&call.tool).is_some() {
        return Ok(());
    }
    let Some(tool) = mastik_mcp::schema::tool(&call.tool) else {
        return Err(no_such_tool(&call.tool));
    };
    let mut args = call.args.clone();
    // An empty selection clears it, where a layer list elsewhere needs one.
    if tool.name == "set_selection" && call.args.get("ids") == Some(&json!([])) {
        return Ok(());
    }
    // The proxy's restore after a reconnect, never listed to the agent. A
    // proxy started from an older install still sends `actors`, now unread.
    if tool.name == "use_document"
        && let Some(map) = args.as_object_mut()
    {
        map.remove("restore");
        map.remove("actors");
    }
    mastik_core::verbs::check_args(tool.name, tool.params, &args)
        .map(|_| ())
        .map_err(|e| e.to_string())
}

/// Parse the model parameters some clients send as JSON text, `call`'s own
/// `args` among them, so the level check and the handler read what the agent
/// meant; a verb's own `Args` does the same.
fn parse_json_text(call: &mut Call) {
    let parse = |map: &mut serde_json::Map<String, Value>, name: &str| {
        if let Some(Value::String(s)) = map.get(name)
            && let Ok(parsed) = serde_json::from_str::<Value>(s.trim())
            && (parsed.is_object() || parsed.is_array())
        {
            map.insert(name.to_owned(), parsed);
        }
    };
    let (tool, args) = if call.tool == "call" {
        let Some(map) = call.args.as_object_mut() else {
            return;
        };
        parse(map, "args");
        let Some(tool) = map.get("tool").and_then(Value::as_str).map(str::to_owned) else {
            return;
        };
        let Some(args) = map.get_mut("args") else {
            return;
        };
        (tool, args)
    } else {
        (call.tool.clone(), &mut call.args)
    };
    if mastik_core::verbs::find(&tool).is_some() {
        return;
    }
    let (Some(tool), Some(map)) = (mastik_mcp::schema::tool(&tool), args.as_object_mut()) else {
        return;
    };
    for p in tool.params {
        if let mastik_core::verbs::ParamKind::Model(model) = p.kind
            && !matches!(model, "PropValue" | "DtcgFile")
        {
            parse(map, p.name);
        }
    }
}

enum Answer {
    Json(Value),
    Image(Vec<u8>),
    /// A report and the files it lists.
    Files(Value, Vec<Content>),
}

fn node(args: &Value, name: &str) -> Result<Option<NodeId>, String> {
    match args.get(name) {
        None | Some(Value::Null) => Ok(None),
        Some(Value::String(s)) => NodeId::from_wire(s)
            .map(Some)
            .ok_or_else(|| format!("`{name}`: `{s}` is not a layer id")),
        Some(other) => Err(format!("`{name}`: expected a layer id, got {other}")),
    }
}

/// [`page_by_name`] for `tool`'s `args`, and for each step of a `batch` or call
/// of a `propose_edit`, its refusal naming the step.
fn pages_by_name(s: &EditorSession, tool: &str, args: &mut Value) -> Result<(), String> {
    page_by_name(s, args, false)?;
    let (list, what) = match tool {
        "batch" => ("steps", "step"),
        "propose_edit" => ("calls", "call"),
        _ => return Ok(()),
    };
    let steps = args.get_mut(list).and_then(Value::as_array_mut);
    for (n, step) in steps.into_iter().flatten().enumerate() {
        if let Some(args) = step.get_mut("args") {
            page_by_name(s, args, true).map_err(|why| format!("{what} {}: {why}", n + 1))?;
        }
    }
    Ok(())
}

/// A `page` argument given as a page's name, as an agent reads it in the
/// preamble or a tree, written as that page's id; one no page has, or several
/// do, is refused naming the pages with their ids. An id is left as it is, and
/// so is an `@` name in a batch step or proposal call (`labels`), where it is a
/// label for an earlier step's page; a direct call substitutes no labels.
fn page_by_name(s: &EditorSession, args: &mut Value, labels: bool) -> Result<(), String> {
    let Some(Value::String(name)) = args.get("page") else {
        return Ok(());
    };
    if NodeId::from_wire(name).is_some() || labels && name.starts_with('@') {
        return Ok(());
    }
    let pages: Vec<(NodeId, &str)> = s
        .doc
        .pages()
        .iter()
        .filter_map(|p| s.doc.get(p.id).map(|n| (p.id, n.name())))
        .collect();
    let wanted = name.trim();
    let matching: Vec<NodeId> = pages
        .iter()
        .filter(|(_, n)| n.eq_ignore_ascii_case(wanted))
        .map(|(id, _)| *id)
        .collect();
    if let [one] = matching[..] {
        args["page"] = json!(one.to_wire());
        return Ok(());
    }
    let listed: Vec<String> = pages
        .iter()
        .map(|(id, n)| format!("{n} {}", id.to_wire()))
        .collect();
    let why = match matching.len() {
        0 => "is no page's name or id",
        _ => "names several pages",
    };
    Err(format!(
        "`page`: `{name}` {why}; the pages are {}",
        listed.join(", ")
    ))
}

/// A list of layer ids, `None` when absent.
fn nodes(args: &Value, name: &str) -> Result<Option<Vec<NodeId>>, String> {
    match args.get(name) {
        None | Some(Value::Null) => Ok(None),
        Some(Value::Array(ids)) => ids
            .iter()
            .map(|v| {
                v.as_str()
                    .and_then(NodeId::from_wire)
                    .ok_or_else(|| format!("`{name}`: `{v}` is not a layer id"))
            })
            .collect::<Result<Vec<_>, _>>()
            .map(Some),
        Some(other) => Err(format!(
            "`{name}`: expected a list of layer ids, got {other}"
        )),
    }
}

fn required(args: &Value, name: &str) -> Result<NodeId, String> {
    node(args, name)?.ok_or_else(|| format!("needs `{name}`"))
}

fn index(args: &Value, name: &str) -> Option<u64> {
    args.get(name).and_then(Value::as_u64)
}

/// The page `id` names or sits on; for none, the page the person is on (the
/// first in a document nobody has open).
fn page_of(s: &EditorSession, id: Option<NodeId>) -> Result<NodeId, String> {
    match id {
        None => Ok(s.page),
        Some(id) if is_page(s, id) => Ok(id),
        Some(id) => s
            .doc
            .page_of(id)
            .ok_or_else(|| format!("no layer `{}`", id.to_wire())),
    }
}

/// The page an `export_png` `region` is a rectangle of: `id`'s when it names a
/// page, else the person's. A layer's `id` beside a `region` is refused.
fn region_page(s: &EditorSession, id: Option<NodeId>) -> Result<NodeId, String> {
    if let Some(id) = id.filter(|id| !is_page(s, *id)) {
        return Err(format!(
            "`{}` is a layer: `region` is a rectangle of a page, so pass the page \
             or no `id`; or export the layer alone without `region`",
            id.to_wire()
        ));
    }
    page_of(s, id)
}

/// `export_png`'s `region`, `[x, y, w, h]` in page coordinates; `None` when
/// absent. One with no area is refused: it has nothing to draw.
fn region(args: &Value) -> Result<Option<mastik_core::kurbo::Rect>, String> {
    let Some(v) = args.get("region").filter(|v| !v.is_null()) else {
        return Ok(None);
    };
    let n: Vec<f64> = v
        .as_array()
        .map(|a| a.iter().filter_map(Value::as_f64).collect())
        .unwrap_or_default();
    match n[..] {
        [x, y, w, h] if w > 0.0 && h > 0.0 => {
            Ok(Some(mastik_core::kurbo::Rect::new(x, y, x + w, y + h)))
        }
        [_, _, _, _] => Err("`region` needs a width and a height above zero".into()),
        _ => Err("`region` is [x, y, w, h]".into()),
    }
}

/// `run_exports`' `rows`, `{from, count}` from 1, as the rows from index
/// `from - 1` on; `None` when absent. The schema has checked its shape.
fn row_range(args: &Value) -> Option<std::ops::Range<usize>> {
    let v = args.get("rows").filter(|v| !v.is_null())?;
    let at = |k: &str| v.get(k).and_then(Value::as_u64).map(|n| n as usize);
    let (from, count) = (at("from")?.max(1), at("count")?);
    Some(from - 1..from - 1 + count)
}

/// What the person sees of the document: the page they are on, the camera's
/// centre and zoom, and, while it is the tab in front (`canvas`), the world
/// rectangle the canvas shows.
fn view(s: &EditorSession, canvas: Option<(u32, u32)>) -> Value {
    let name = s.doc.get(s.page).map(|p| p.name().to_owned());
    let c = s.camera;
    let mut answer = json!({
        "page": { "id": s.page.to_wire(), "name": name },
        "center": [c.center.x, c.center.y],
        "zoom": c.zoom,
        "in_front": canvas.is_some(),
    });
    if let Some(px) = canvas {
        let r = c.viewport(px).view;
        answer["visible"] = json!([r.x0, r.y0, r.width(), r.height()]);
    }
    answer
}

fn is_page(s: &EditorSession, id: NodeId) -> bool {
    s.doc.page_ids().any(|p| p == id)
}

/// The default longest side of `export_png`.
const PNG_MAX: u64 = 1600;

/// [`PNG_MAX`] for an assistant chat, whose pictures are read again on every
/// later step of its turn: about half the image tokens, still enough to judge
/// a page by.
const CHAT_PNG_MAX: u64 = 1000;

/// The longest answer any read gives, in bytes. Harnesses cut a tool
/// result past their own limit (Claude Code's is about 25,000 tokens), and a
/// snapshot cut mid-way reads as a smaller document; a refusal naming the size
/// and the narrower reads is what the model can act on.
const ANSWER_BUDGET: usize = 100_000;

/// Refuse `text` past [`ANSWER_BUDGET`], saying what to read `instead`.
fn within_budget(text: &str, what: &str, instead: &str) -> Result<(), String> {
    match text.len() {
        n if n <= ANSWER_BUDGET => Ok(()),
        n => Err(format!(
            "{what} is {} KB, past the {} KB a client is sure to read whole: {instead}",
            n / 1000,
            ANSWER_BUDGET / 1000
        )),
    }
}

/// What a read past the budget points to.
const NARROWER: &str =
    "reach the part you need with `find_nodes` or `get_outline`, then `get_node` on it";

/// A read's answer, refused past [`ANSWER_BUDGET`] with the narrower call that
/// tool takes. Every read passes here, behind any budget it applies itself;
/// `run_exports`, `export_prototype` and `export_flow_map` carry their files
/// under their own, and an image is bounded by its pixels, `export_png`'s two
/// included.
fn read_fits(tool: &str, answer: Answer) -> Result<Answer, String> {
    use mastik_mcp::schema::{Class, class};
    if class(tool) != Class::Read
        || matches!(
            tool,
            "run_exports" | "export_png" | "export_prototype" | "export_flow_map"
        )
    {
        return Ok(answer);
    }
    let size = match &answer {
        Answer::Json(v) => v.to_string().len(),
        Answer::Files(report, files) => {
            let bytes = files.iter().map(|f| match f {
                Content::File { bytes, .. } => bytes.len(),
                Content::Text(t) => t.len(),
                Content::Image { bytes, .. } => bytes.len(),
            });
            report.to_string().len() + bytes.sum::<usize>()
        }
        Answer::Image(_) => return Ok(answer),
    };
    if size <= ANSWER_BUDGET {
        return Ok(answer);
    }
    let instead = match tool {
        "find_like" => "pass a smaller `limit`",
        "list_components" | "list_variables" | "list_styles" | "list_documents" | "list_kits" => {
            "pass `query` to list the ones whose name holds it"
        }
        "export_tokens" => "read them in parts with `list_variables` and `list_styles`",
        "get_table" => "pass a smaller `count`, or `columns`",
        "check_layout" | "check_contrast" | "review_colours" | "export_code" => {
            "pass `ids` to read one part of the page"
        }
        _ => NARROWER,
    };
    Err(format!(
        "This answer is {} KB, past the {} KB a client is sure to read whole: {instead}",
        size / 1000,
        ANSWER_BUDGET / 1000
    ))
}

/// The most `export_png`'s `max` is taken at, and the longest side an
/// attached layer is drawn at for the assistant: a raster held in the person's
/// window, where an unbounded one could exhaust its memory and lose every tab.
pub(crate) const PNG_CEILING: u64 = 4096;

/// The viewport `export_png` draws `view` through, refused when it would pass
/// `max` pixels: the renderer floors its scale, so a huge box would otherwise
/// draw a raster of gigabytes on the person's UI thread.
fn png_viewport(
    what: &str,
    view: mastik_core::kurbo::Rect,
    scale: f64,
    max: f64,
) -> Result<mastik_render::Viewport, String> {
    let vp = mastik_export::png::viewport_at(view, scale);
    let side = f64::from(vp.pixel_size.0.max(vp.pixel_size.1));
    if !view.width().is_finite() || !view.height().is_finite() || side > max.ceil() + 1.0 {
        return Err(format!(
            "{what} is too large to draw within `max` pixels; export a smaller part"
        ));
    }
    Ok(vp)
}

fn run(conn: &Connection, s: &mut EditorSession, call: &Call) -> Result<Answer, String> {
    let args = &call.args;
    let json = |v: Value| Ok(Answer::Json(v));
    match call.tool.as_str() {
        "get_document" if args.get("brief") == Some(&json!(true)) => {
            let since = index(args, "since").ok_or(
                "`brief` lists what changed since a revision: pass `since`, a `rev` you were given",
            )?;
            let answer = revisions::since_brief(conn, since, s)?;
            within_budget(
                &answer.to_string(),
                "What changed",
                "read the sections `left_out.in` names with `get_outline`",
            )?;
            json(answer)
        }
        "get_document" => {
            let snap = snapshot_of(s);
            if let Some(since) = since(conn, args, s, Scope::Document)? {
                within_budget(&since.to_string(), "What changed", NARROWER)?;
                return json(since);
            }
            let whole = serde_json::to_value(&snap).map_err(|e| e.to_string())?;
            within_budget(&whole.to_string(), "The whole document", NARROWER)?;
            json(whole)
        }
        "get_tree" => {
            let id = required(args, "id")?;
            if is_page(s, id) {
                return Err(format!(
                    "`{}` is a page: `get_outline` names its layers, each a tree",
                    id.to_wire()
                ));
            }
            let tree = mastik_core::verbs::tree(&s.doc, id)
                .ok_or_else(|| format!("no layer `{}`", id.to_wire()))?;
            let answer = json!({ "tree": tree });
            within_budget(
                &answer.to_string(),
                "This tree",
                "read the layers inside it one `get_tree` at a time",
            )?;
            json(answer)
        }
        "get_node" => {
            let id = required(args, "id")?;
            if s.doc.get(id).is_none() {
                return Err(format!("no layer `{}`", id.to_wire()));
            }
            let id = id.to_wire();
            let snap = snapshot_of(s);
            let subtree = args.get("subtree").and_then(Value::as_bool) == Some(true);
            let scope = match subtree {
                true => Scope::Subtree(&id),
                false => Scope::Node(&id),
            };
            if let Some(since) = since(conn, args, s, scope)? {
                within_budget(&since.to_string(), "What changed", NARROWER)?;
                return json(since);
            }
            let by_id: HashMap<&str, &mastik_export::snapshot::NodeSnapshot> =
                snap.nodes.iter().map(|n| (n.id.as_str(), n)).collect();
            let root = by_id
                .get(id.as_str())
                .ok_or_else(|| format!("no layer `{id}`"))?;
            let mut nodes = vec![*root];
            if subtree {
                let mut i = 0;
                while i < nodes.len() {
                    for c in &nodes[i].children {
                        if let Some(n) = by_id.get(c.as_str()) {
                            nodes.push(n);
                        }
                    }
                    i += 1;
                }
            }
            let mut answer = json!({ "nodes": nodes });
            // A page's flows and prototype settings are its page entry's, read
            // with its root as `set` writes them.
            if let Some(p) = snap.pages.iter().find(|p| p.id == id)
                && let Some(root) = answer["nodes"].get_mut(0).and_then(Value::as_object_mut)
            {
                for (k, v) in [("flows", &p.flows), ("play", &p.play)] {
                    if let Some(v) = v {
                        root.insert(k.into(), v.clone());
                    }
                }
            }
            // Below the root, a layer's page and frame are the root's or the
            // root itself: written once, on the root.
            if let Some(Value::Array(list)) = answer.get_mut("nodes")
                && let Some((root, below)) = list.split_first_mut()
            {
                let frame = root.get("artboard").cloned();
                for n in below.iter_mut().filter_map(Value::as_object_mut) {
                    n.remove("page");
                    if n.get("artboard") == frame.as_ref() || n.get("artboard") == root.get("id") {
                        n.remove("artboard");
                    }
                }
            }
            within_budget(&answer.to_string(), "This subtree", NARROWER)?;
            json(answer)
        }
        "list_children" => {
            let id = node(args, "id")?;
            if let Some(id) = id
                && s.doc.get(id).is_none()
            {
                return Err(format!("no layer `{}`", id.to_wire()));
            }
            let snap = snapshot_of(s);
            let wire = id.map(NodeId::to_wire);
            // Answered in its own rows: whole when any of them, or the parent's
            // order of them, moved.
            let moved = match since(conn, args, s, Scope::Children(wire.as_deref()))? {
                Some(v) if v.get("unchanged").is_some() => return json(v),
                Some(v) => v.get("since").cloned(),
                None => None,
            };
            let rows = |children: Vec<Value>| match &moved {
                Some(since) => json!({ "since": since, "children": children }),
                None => json!({ "children": children }),
            };
            let Some(id) = wire else {
                let pages: Vec<Value> = snap
                    .pages
                    .iter()
                    .map(|p| json!({ "id": p.id, "name": p.name, "kind": "page" }))
                    .collect();
                return json(rows(pages));
            };
            let by_id: HashMap<&str, &mastik_export::snapshot::NodeSnapshot> =
                snap.nodes.iter().map(|n| (n.id.as_str(), n)).collect();
            let parent = by_id
                .get(id.as_str())
                .ok_or_else(|| format!("no layer `{id}`"))?;
            let children: Vec<Value> = parent
                .children
                .iter()
                .filter_map(|c| by_id.get(c.as_str()))
                .map(|n| json!({ "id": n.id, "name": n.name, "kind": n.kind }))
                .collect();
            json(rows(children))
        }
        "get_outline" => {
            let answer = outline_of(s, args)?;
            within_budget(
                &answer.to_string(),
                "This outline",
                "pass a smaller `depth` or `limit`, or outline one layer with `id`",
            )?;
            json(answer)
        }
        "find_nodes" => {
            let query = args
                .get("query")
                .and_then(Value::as_str)
                .ok_or("needs `query`")?
                .to_lowercase();
            let page = node(args, "page")?.map(NodeId::to_wire);
            let limit = index(args, "limit").unwrap_or(50) as usize;
            let snap = snapshot_of(s);
            let by_id: HashMap<&str, &mastik_export::snapshot::NodeSnapshot> =
                snap.nodes.iter().map(|n| (n.id.as_str(), n)).collect();
            let mut found = Vec::new();
            let mut more = false;
            for n in &snap.nodes {
                if page.as_ref().is_some_and(|p| *p != n.page) || n.id == n.page {
                    continue;
                }
                let text = match &n.geometry {
                    mastik_export::snapshot::Geometry::Text { content, .. } => content.as_str(),
                    _ => "",
                };
                if !(n.name.to_lowercase().contains(&query) || text.to_lowercase().contains(&query))
                {
                    continue;
                }
                if found.len() == limit {
                    more = true;
                    break;
                }
                let mut path = Vec::new();
                let mut up = n.parent.as_deref();
                while let Some(p) = up.and_then(|p| by_id.get(p)) {
                    path.push(p.name.clone());
                    up = p.parent.as_deref();
                }
                path.reverse();
                let mut entry = json!({ "id": n.id, "name": n.name, "kind": n.kind, "path": path });
                if let Some(id) = NodeId::from_wire(&n.id) {
                    entry["box"] = reads::bounds_box(s, id);
                    if s.selection
                        .ids()
                        .iter()
                        .any(|sel| reads::within(&s.doc, *sel, id))
                    {
                        entry["selected"] = json!(true);
                    }
                }
                found.push(entry);
            }
            json(json!({ "nodes": found, "more": more }))
        }
        "hit_test" => {
            let x = args.get("x").and_then(Value::as_f64).ok_or("needs `x`")?;
            let y = args.get("y").and_then(Value::as_f64).ok_or("needs `y`")?;
            let page = page_of(s, node(args, "page")?)?;
            let hits = mastik_core::query::hit_test(&s.resolved, &s.doc, page, (x, y).into(), 0.0);
            json(json!({ "ids": hits.iter().map(|h| h.to_wire()).collect::<Vec<_>>() }))
        }
        "get_bounds" => {
            let id = required(args, "id")?;
            let r = s
                .resolved
                .world_bounds(id)
                .ok_or_else(|| format!("`{}` has no bounds", id.to_wire()))?;
            json(json!({ "bounds": [r.x0, r.y0, r.width(), r.height()] }))
        }
        "get_selection" => json(json!({
            "ids": s.selection.ids().iter().map(|i| i.to_wire()).collect::<Vec<_>>()
        })),
        "set_selection" => {
            let ids = args
                .get("ids")
                .and_then(Value::as_array)
                .ok_or("needs `ids`")?
                .iter()
                .map(|v| {
                    v.as_str()
                        .and_then(NodeId::from_wire)
                        .filter(|id| s.doc.get(*id).is_some())
                        .ok_or_else(|| format!("`{v}` is no layer"))
                })
                .collect::<Result<Vec<_>, _>>()?;
            // As a person can select: layers of the page shown, never the page
            // itself or a kit's vendored component. The page is not switched,
            // which would move the person's view.
            if let Some(id) = ids
                .iter()
                .find(|id| **id == s.page || s.doc.page_of(**id) != Some(s.page))
            {
                return Err(format!(
                    "`{}` is not a layer of the page the person is on (`{}`); a selection \
                     never spans pages",
                    id.to_wire(),
                    s.page.to_wire()
                ));
            }
            let mut seen = std::collections::HashSet::new();
            let ids: Vec<NodeId> = ids.into_iter().filter(|id| seen.insert(*id)).collect();
            s.selection.set(ids);
            json(json!({}))
        }
        "export_svg" => {
            let id = node(args, "id")?;
            let svg = match id {
                Some(id) if !is_page(s, id) => {
                    mastik_export::svg::svg_of(&s.doc, &s.resolved, &[id])
                }
                _ => {
                    let page = page_of(s, id)?;
                    mastik_export::svg::svg(&s.doc, &s.resolved, page, None)
                }
            };
            within_budget(
                &svg,
                "The SVG",
                "export one layer with `id`, or look with `export_png`",
            )?;
            json(json!({ "svg": svg }))
        }
        "export_png" if index(args, "since").is_some() => {
            diff::export_png_since(conn, s, args, index(args, "since").unwrap_or_default())
        }
        "export_png" => {
            let id = node(args, "id")?;
            let max = index(args, "max").unwrap_or(PNG_MAX);
            if max > PNG_CEILING {
                return Err(format!(
                    "`max` is at most {PNG_CEILING}; export a part with `id` to see it closer"
                ));
            }
            let max = max.max(16) as f64;
            if let Some(region) = region(args)? {
                let page = region_page(s, id)?;
                let scale = (max / region.width().max(region.height())).min(2.0);
                let vp = png_viewport("`region`", region, scale, max)?;
                return Ok(Answer::Image(mastik_export::png::png(
                    &s.doc,
                    &s.resolved,
                    &vp,
                    page,
                )));
            }
            let png = match id {
                Some(id) if !is_page(s, id) => {
                    // The box `raster_of` frames: the ink, effects and unclipped
                    // children included, so `max` bounds what is drawn.
                    let r = mastik_export::extent(&s.resolved, &[id])
                        .ok_or_else(|| format!("`{}` has no bounds", id.to_wire()))?;
                    let scale = (max / r.width().max(r.height()).max(1.0)).min(2.0);
                    png_viewport("The layer", r, scale, max)?;
                    let opts = mastik_export::png::RasterOpts {
                        scale,
                        ..Default::default()
                    };
                    let r = mastik_export::png::raster_of(&s.doc, &s.resolved, &[id], &opts)
                        .ok_or("nothing to draw")?;
                    mastik_export::png::encode_rgba8(&r.rgba, r.width, r.height)
                }
                _ => {
                    let page = page_of(s, id)?;
                    let view = mastik_export::extent(&s.resolved, &[page])
                        .unwrap_or_else(|| mastik_core::kurbo::Rect::new(0.0, 0.0, 512.0, 512.0));
                    let scale = (max / view.width().max(view.height()).max(1.0)).min(1.0);
                    let vp = png_viewport("The page", view, scale, max)?;
                    mastik_export::png::png(&s.doc, &s.resolved, &vp, page)
                }
            };
            Ok(Answer::Image(png))
        }
        "undo" | "redo" if s.compare.is_some() => Err(READ_ONLY.to_owned()),
        "undo" => undo(s, &agent(call)),
        "redo" => {
            let step = own_step(s.history.undone().next(), &agent(call), "redo")?;
            json(json!({ "redone": s.redo().then_some(step).flatten() }))
        }
        "run_exports" => reads::run_exports(s, nodes(args, "ids")?, row_range(args)),
        "export_code" => reads::export_code(s, args, nodes(args, "ids")?),
        "list_components" => json(reads::named(reads::components(&s.doc), "components", args)),
        "list_variables" => json(reads::named(reads::variables(&s.doc), "variables", args)),
        "list_styles" => json(reads::named(reads::styles(&s.doc), "styles", args)),
        "list_tables" => json(reads::tables(&s.doc)),
        "get_table" => json(reads::table(&s.doc, args)?),
        "list_scripts" => json(reads::named(reads::scripts(&s.doc), "scripts", args)),
        "get_prototype" => json(reads::prototype(&s.doc)),
        tool if inspect::TOOLS.contains(&tool) => inspect::run(s, tool, args),
        tool if context::TOOLS.contains(&tool) => context::run(s, tool, args),
        "run_script" => run_script(conn, s, &call.client, args),
        "batch" => batch(conn, s, &call.client, args),
        "set" => set(conn, s, &call.client, args),
        tool => write(conn, s, &call.client, tool, args),
    }
}

/// `undo`: take back the latest step this client made and has not taken back.
/// On top of the history it is undone; under later steps, the person's or
/// another client's, it is taken back alone as a new step when those changed
/// other things (`EditorSession::revert`), and refused, naming why, when not.
fn undo(s: &mut EditorSession, agent: &str) -> Result<Answer, String> {
    let mine = |n: &mastik_core::StepNote| n.agent.as_deref() == Some(agent);
    let latest = s
        .history
        .done()
        .rev()
        .find(|v| mine(v.note) && v.note.reverts.is_none() && !s.history.is_reverted(v.serial))
        .map(|v| (v.serial, v.note.label.clone()));
    let top = s.history.done().next_back().map(|v| v.serial);
    let json = |v: Value| Ok(Answer::Json(v));
    match latest {
        Some((serial, label)) if Some(serial) == top => {
            json(json!({ "undone": s.undo().then_some(label) }))
        }
        Some((serial, label)) => match s.revert(serial) {
            Ok(()) => json(json!({ "undone": label, "under_later_steps": true })),
            Err(why) => Err(format!("Your step `{label}`: {why}. Nothing was changed")),
        },
        // The client's own *Undo* step on top is not undone: that would put
        // back the edit it took back.
        None if s
            .history
            .done()
            .next_back()
            .is_some_and(|v| mine(v.note) && v.note.reverts.is_some()) =>
        {
            json(json!({ "undone": null }))
        }
        None => {
            let step = own_step(s.history.done().next_back(), agent, "undo")?;
            json(json!({ "undone": s.undo().then_some(step).flatten() }))
        }
    }
}

/// An MCP call run straight on `s`, outside any client's
/// connection: `mastik run`'s two kinds of script and `mastik inspect`'s reads,
/// answered as the MCP answers them, without its answer budget. `args` carries
/// no `base_rev`, the one thing that reads a connection.
pub(crate) fn run_offline(
    s: &mut EditorSession,
    tool: &str,
    args: &Value,
) -> Result<Value, String> {
    const CLIENT: &str = "mastik";
    let conn = Connection::default();
    let answer = match tool {
        "batch" => writes::batch(&conn, s, CLIENT, args)?,
        "run_script" => run_script(&conn, s, CLIENT, args)?,
        "get_outline" => return outline_of(s, args),
        "design_context" => context::run(s, tool, args)?,
        "list_components" => {
            return Ok(reads::named(reads::components(&s.doc), "components", args));
        }
        "list_variables" => return Ok(reads::named(reads::variables(&s.doc), "variables", args)),
        "list_styles" => return Ok(reads::named(reads::styles(&s.doc), "styles", args)),
        "list_tables" => return Ok(reads::tables(&s.doc)),
        "get_prototype" => return Ok(reads::prototype(&s.doc)),
        other => return Err(format!("`{other}` is not run outside a connection")),
    };
    match answer {
        Answer::Json(v) => Ok(v),
        _ => Err(format!("`{tool}` answers JSON")),
    }
}

/// `get_outline`'s answer: the layers below `id`, else the current page, to
/// `depth` levels (2) and `limit` rows (200).
fn outline_of(s: &EditorSession, args: &Value) -> Result<Value, String> {
    let root = match node(args, "id")? {
        Some(id) if s.doc.get(id).is_none() => {
            return Err(format!("no layer `{}`", id.to_wire()));
        }
        Some(id) => id,
        None => s.page,
    };
    let depth = index(args, "depth").unwrap_or(2);
    let limit = index(args, "limit").unwrap_or(200) as usize;
    Ok(reads::outline(s, root, depth, limit))
}

/// The most printed lines `run_script` answers; the earlier ones are counted.
const PRINTED_LISTED: usize = 100;

/// `run_script`: Command Mode's Luau run as the script editor runs it
/// (`command::host::run_as_step`), one undo step named by the tool and marked
/// as the client's. A raise, a spent budget or a stale `base_rev` takes every
/// write back and keeps the person's redo. The selection is left alone.
fn run_script(
    conn: &Connection,
    s: &mut EditorSession,
    client: &str,
    args: &Value,
) -> Result<Answer, String> {
    let source = args["source"].as_str().unwrap_or_default();
    let selection = nodes(args, "nodes")?.unwrap_or_default();
    let mut labels = rustc_hash::FxHashMap::default();
    for (name, id) in args
        .get("layers")
        .and_then(Value::as_object)
        .into_iter()
        .flatten()
    {
        let id = id
            .as_str()
            .and_then(NodeId::from_wire)
            .ok_or_else(|| format!("`layers.{name}`: `{id}` is not a layer id"))?;
        labels.insert(name.clone(), id);
    }
    let header = mastik_vm::header(source).map_err(|e| format!("{e}. Nothing was changed."))?;
    let given = match args.get("params") {
        None | Some(Value::Null) => Map::new(),
        Some(Value::Object(given)) => given.clone(),
        Some(_) => return Err("`params` is an object, {\"<input>\": <value>}".into()),
    };
    let env = mastik_vm::Env {
        selection: &selection,
        labels: &labels,
        params: header
            .params(&given)
            .map_err(|e| format!("`params`: {e}. Nothing was changed."))?,
    };
    let base = index(args, "base_rev");
    let depth = s.history.undo_depth();
    let ran = crate::command::host::run_as_step(s, "run_script", "", |h| {
        let outcome = mastik_vm::run_script(h, &env, source)?;
        if let Some(base) = base {
            let made: Vec<mastik_core::Operation> =
                h.s.history
                    .done()
                    .skip(depth)
                    .flat_map(|v| v.inverse.0.clone())
                    .collect();
            stale(conn, h.s, base, &mastik_core::Transaction(made)).map_err(|why| {
                mastik_vm::VmError {
                    kind: mastik_vm::ErrorKind::Error,
                    message: why,
                    line: None,
                    span: None,
                    fix: None,
                }
            })?;
        }
        Ok(outcome)
    });
    let (outcome, counts, serial) = ran.map_err(|e| format!("{e}. Nothing was changed."))?;
    if serial.is_some()
        && let Some(note) = s.history.current_note_mut()
    {
        note.origin = mastik_core::Origin::Mcp {
            client: client.to_owned(),
            tool: "run_script".into(),
        };
        note.glyph = "".into();
    }
    let printed = &outcome.printed;
    let cut = printed.len().saturating_sub(PRINTED_LISTED);
    let mut answer = json!({
        "printed": printed[cut..],
        "created": outcome.created.iter().map(|id| id.to_wire()).collect::<Vec<_>>(),
        "said": counts.said().unwrap_or_else(|| "Nothing changed".into()),
    });
    if let Some(value) = outcome.value {
        answer["value"] = json!(value);
    }
    if cut > 0 {
        answer["printed_before"] = json!(cut);
    }
    Ok(Answer::Json(answer))
}

/// Refuse an agent's `undo` or `redo` of a step it did not make: the history is
/// the person's too, and one agent call must not take back their work or another
/// client's. A step is this client's when it carries its [`agent`], not merely
/// its name: two sessions of one harness share that. Answers the step's name as
/// the History panel lists it, `None` at the end of the history.
fn own_step(
    step: Option<mastik_core::StepView<'_>>,
    agent: &str,
    verb: &str,
) -> Result<Option<String>, String> {
    let Some(step) = step else {
        return Ok(None);
    };
    match &step.note.origin {
        _ if step.note.agent.as_deref() == Some(agent) => Ok(Some(step.note.label.clone())),
        mastik_core::Origin::Mcp { client: c, .. } => Err(format!(
            "The step to {verb} is `{}`, made by another client ({c}), not by you: nothing was changed",
            step.note.label
        )),
        mastik_core::Origin::User => Err(format!(
            "The step to {verb} is the person's `{}`: nothing was changed. Ask them, or reverse your own edit with a write",
            step.note.label
        )),
    }
}

/// `list_fonts`: the families whose name holds `query`, ignoring case, at most
/// `limit` (100 when absent), and how many matched in all.
fn fonts(docs: &mut impl Documents, args: &Value) -> Value {
    let query = args
        .get("query")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_lowercase();
    let limit = index(args, "limit").unwrap_or(100) as usize;
    let matched: Vec<String> = docs
        .fonts()
        .into_iter()
        .filter(|f| f.to_lowercase().contains(&query))
        .collect();
    json!({
        "families": matched.iter().take(limit).collect::<Vec<_>>(),
        "matched": matched.len(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::mpsc::sync_channel;

    /// One open document at `/a.mastik`.
    pub(in crate::mcp) struct One {
        pub(in crate::mcp) path: PathBuf,
        pub(in crate::mcp) session: EditorSession,
        committed: usize,
    }

    impl One {
        pub(in crate::mcp) fn new() -> Self {
            Self {
                path: PathBuf::from("/a.mastik"),
                session: EditorSession::new(),
                committed: 0,
            }
        }
    }

    impl Documents for One {
        fn open(&mut self, wanted: &str) -> Result<PathBuf, String> {
            match wanted {
                "a" => Ok(self.path.clone()),
                _ => Err(format!("no `{wanted}`")),
            }
        }
        fn session(&mut self, path: &Path) -> Option<&mut EditorSession> {
            (path == self.path).then_some(&mut self.session)
        }
        fn committed(&mut self, _: &Path) -> Result<(), String> {
            self.committed += 1;
            Ok(())
        }
        fn library(&mut self) -> Option<&Library> {
            None
        }
        fn with_library(&mut self, path: &Path) -> Option<(&mut EditorSession, Option<&Library>)> {
            self.session(path).map(|s| (s, None))
        }
        fn save(&mut self, _: &Path) -> Result<(), String> {
            Ok(())
        }
        fn tell(&mut self, _: String) {}
        fn fonts(&mut self) -> Vec<String> {
            vec!["Inter".into(), "Inter Tight".into(), "Lora".into()]
        }
        fn canvas(&mut self, path: &Path) -> Option<(u32, u32)> {
            (path == self.path).then_some((800, 600))
        }
    }

    pub(in crate::mcp) fn call(
        engine: &mut Engine,
        docs: &mut One,
        conn: Conn,
        tool: &str,
        args: Value,
    ) -> Output {
        keyed(engine, docs, conn, None, tool, args)
    }

    /// [`call`] from a client that gave `key`.
    pub(in crate::mcp) fn keyed(
        engine: &mut Engine,
        docs: &mut One,
        conn: Conn,
        key: Option<&str>,
        tool: &str,
        args: Value,
    ) -> Output {
        let (reply, answer) = sync_channel(1);
        let call = Call::new(conn, key.map(str::to_owned), tool, args, reply);
        if !export_call(&call) {
            return engine.handle(docs, call);
        }
        // An export waits for its worker: settle it as the window does.
        if let Some(out) = engine.handle_or_wait(docs, call) {
            return out;
        }
        let until = std::time::Instant::now() + std::time::Duration::from_secs(30);
        loop {
            engine.run_later(docs);
            if let Ok(out) = answer.try_recv() {
                return out;
            }
            assert!(std::time::Instant::now() < until, "the export settles");
            std::thread::yield_now();
        }
    }

    pub(in crate::mcp) fn ok(out: &Output) -> &Value {
        assert!(!out.is_error, "{:?}", out.content);
        out.structured.as_ref().unwrap()
    }

    pub(in crate::mcp) fn open(engine: &mut Engine, docs: &mut One, conn: Conn) {
        ok(&call(
            engine,
            docs,
            conn,
            "use_document",
            json!({ "document": "a" }),
        ));
    }

    /// Make a rectangle on the first page through the engine, answering its id.
    pub(in crate::mcp) fn rect(
        engine: &mut Engine,
        docs: &mut One,
        conn: Conn,
        name: &str,
    ) -> String {
        let page = docs.session.doc.first_page().to_wire();
        let kind = mastik_core::verbs::example("NodeKind").unwrap();
        let args = json!({ "parent": page, "kind": kind, "name": name });
        let out = call(engine, docs, conn, "create_node", args);
        ok(&out)["created"][0].as_str().unwrap().to_owned()
    }

    /// A new top-level frame named `name`: what a link or a flow goes to.
    pub(in crate::mcp) fn screen(
        engine: &mut Engine,
        docs: &mut One,
        conn: Conn,
        name: &str,
    ) -> String {
        let page = docs.session.doc.first_page().to_wire();
        let kind = json!({ "Artboard": { "size": { "width": 390, "height": 844 } } });
        let args = json!({ "parent": page, "kind": kind, "name": name });
        let out = call(engine, docs, conn, "create_node", args);
        ok(&out)["created"][0].as_str().unwrap().to_owned()
    }

    /// A client that asked for short ids is answered in them and writes with
    /// them, a name that reads like one is kept as written, and a fresh host
    /// with no memory of the client, having shown another actor first, reads
    /// an id the agent held as the same layer.
    ///
    /// flip: `Engine::answer` reading arguments against no actors → fails at
    /// the first `create_node`. The spelling's independence of order is
    /// pinned in `mastik_core`'s `the_short_form_spells_a_unique_start_of_the_actor`.
    #[test]
    fn short_ids_name_the_same_layer_on_any_connection() {
        fn short(engine: &mut Engine, docs: &mut One, tool: &str, args: Value) -> Output {
            let (reply, _answer) = sync_channel(1);
            let mut call = Call::new(1, Some("k".into()), tool, args, reply);
            call.short_ids = true;
            engine.handle(docs, call)
        }
        let (mut engine, mut docs) = (Engine::default(), One::new());
        let out = short(
            &mut engine,
            &mut docs,
            "use_document",
            json!({ "document": "a" }),
        );
        let page = ok(&out)["pages"][0]["id"].as_str().unwrap().to_owned();
        assert!(page.starts_with('n') && !page.contains(':'), "{page}");
        let kind = mastik_core::verbs::example("NodeKind").unwrap();
        let args = json!({ "parent": page, "kind": kind, "name": "Card" });
        let out = short(&mut engine, &mut docs, "create_node", args);
        let card = ok(&out)["created"][0].as_str().unwrap().to_owned();
        assert!(out.meta.is_none(), "no table travels");
        let s = &docs.session;
        let known = short::known_actors(&s.doc, s.ids.actor());
        let id = NodeId::from_short(&card, &known).expect("a short id");
        let args = json!({ "node": card, "name": "n1" });
        ok(&short(&mut engine, &mut docs, "set_name", args));
        assert_eq!(
            docs.session.doc.get(id).unwrap().name(),
            "n1",
            "a name stays as written"
        );
        // What it buys: an outline of ten layers in a fraction of the text.
        for i in 0..10 {
            let args = json!({ "parent": page, "kind": kind, "name": format!("R{i}") });
            ok(&short(&mut engine, &mut docs, "create_node", args));
        }
        let text = |out: &Output| match &out.content[0] {
            Content::Text(t) => t.len(),
            _ => 0,
        };
        let shown = text(&short(&mut engine, &mut docs, "get_outline", json!({})));
        open(&mut engine, &mut docs, 2);
        let full = text(&call(&mut engine, &mut docs, 2, "get_outline", json!({})));
        assert!(
            shown * 10 < full * 9,
            "short ids cut the outline: {shown} of {full}"
        );

        // Another actor's layer, shown first by a fresh host that never saw
        // this client: a table built in order would number the actors anew.
        let other = NodeId {
            actor: 0xabc,
            seq: 1,
        };
        let parent = NodeId::from_short(&page, &known).unwrap();
        let make = mastik_core::Transaction(vec![mastik_core::Operation::CreateNode {
            id: other,
            parent,
            index: 0,
            kind: mastik_core::NodeKind::Rect {
                size: mastik_core::kurbo::Size::new(10.0, 10.0),
                corner_radii: mastik_core::kurbo::RoundedRectRadii::default(),
                corner_smoothing: 0.0,
            },
            transform: None,
            name: Some("Theirs".into()),
        }]);
        docs.session.try_commit(make).unwrap();
        let mut next = Engine::default();
        let restore = json!({ "document": "a", "restore": true });
        ok(&short(&mut next, &mut docs, "use_document", restore));
        let theirs = short(&mut next, &mut docs, "get_node", json!({ "id": "abc:1" }));
        assert_eq!(ok(&theirs)["nodes"][0]["name"], "Theirs");
        let out = short(&mut next, &mut docs, "get_node", json!({ "id": card }));
        assert!(
            !out.is_error,
            "the next host reads the id: {:?}",
            out.content
        );
        assert_eq!(
            ok(&out)["nodes"][0]["id"],
            json!(card),
            "and writes it the same way"
        );
        assert_eq!(ok(&out)["nodes"][0]["name"], "n1");
    }

    /// A tool before `use_document` is refused; after, a write lands through the
    /// session as one undo step, and the host is told to write the file.
    #[test]
    fn a_write_needs_a_document_and_lands_as_one_step() {
        let (mut engine, mut docs) = (Engine::default(), One::new());
        assert!(call(&mut engine, &mut docs, 1, "get_document", json!({})).is_error);
        open(&mut engine, &mut docs, 1);
        let id = NodeId::from_wire(&rect(&mut engine, &mut docs, 1, "Header")).unwrap();
        assert_eq!(docs.session.doc.get(id).unwrap().name(), "Header");
        assert_eq!(docs.committed, 1);
        assert_eq!(docs.session.history.undo_depth(), 1);
    }

    /// Every listed tool that is not a verb reaches a handler: called with the
    /// least its parameters accept, none is answered as no tool, which a table
    /// tool missing from dispatch is (it falls through to the verbs). An unknown
    /// name is refused as one before anything else.
    ///
    /// flip: `run` without its `get_bounds` arm → fails at `get_bounds`.
    #[test]
    fn every_listed_tool_is_dispatched() {
        use mastik_core::verbs::ParamKind as K;
        let (mut engine, mut docs) = (Engine::default(), One::new());
        open(&mut engine, &mut docs, 1);
        let page = docs.session.doc.first_page().to_wire();
        let least = |kind: K| match kind {
            K::Node | K::Id(_) => json!(page),
            K::Nodes => json!([page]),
            K::Number | K::Index => json!(0),
            K::Bool => json!(false),
            K::Text => json!("x"),
            K::Choice(words) => json!(words[0]),
            K::Model("KitModeMap") => json!({}),
            K::Model("DtcgFile") => json!("{}"),
            K::Model(model) => mastik_core::verbs::example(model).unwrap_or(json!({})),
        };
        for tool in mastik_mcp::schema::TOOLS {
            // `call` answers as the tool it names, which its own test covers.
            if tool.name == "use_document" || tool.name == "call" {
                continue;
            }
            let args: serde_json::Map<String, Value> = tool
                .params
                .iter()
                .filter(|p| p.required)
                .map(|p| (p.name.to_owned(), least(p.kind)))
                .collect();
            let out = call(&mut engine, &mut docs, 1, tool.name, Value::Object(args));
            let text = format!("{:?}", out.content);
            assert!(
                !text.contains("no tool `") && !text.contains("No tool is called"),
                "`{}` is not dispatched: {text}",
                tool.name
            );
        }
        let none = call(&mut engine, &mut docs, 1, "paint_it_red", json!({}));
        assert!(format!("{:?}", none.content).contains("No tool is called"));
    }

    /// `set_selection` takes layers of the page shown, once each, and refuses
    /// the page itself, leaving the selection as it was.
    ///
    /// flip: `set_selection` without its page check → fails at the page's
    /// refusal.
    #[test]
    fn set_selection_takes_layers_of_the_page_shown() {
        let (mut engine, mut docs) = (Engine::default(), One::new());
        open(&mut engine, &mut docs, 1);
        let id = rect(&mut engine, &mut docs, 1, "Chip");
        let set = |e: &mut Engine, d: &mut One, ids: Value| {
            call(e, d, 1, "set_selection", json!({ "ids": ids }))
        };
        ok(&set(&mut engine, &mut docs, json!([id, id])));
        assert_eq!(docs.session.selection.ids().len(), 1, "once each");
        let page = docs.session.page.to_wire();
        assert!(set(&mut engine, &mut docs, json!([page])).is_error);
        assert_eq!(docs.session.selection.ids().len(), 1, "left as it was");
    }

    /// `get_view` answers the page the person is on and what the canvas shows,
    /// and a read with no page reads that page, not the first.
    ///
    /// flip: `page_of` answering the first page for none → fails at "on the
    /// person's page".
    #[test]
    fn get_view_answers_the_persons_page_and_reads_default_to_it() {
        let (mut engine, mut docs) = (Engine::default(), One::new());
        open(&mut engine, &mut docs, 1);
        let added = call(
            &mut engine,
            &mut docs,
            1,
            "add_page",
            json!({ "name": "Two" }),
        );
        let two = ok(&added)["created"][0].as_str().unwrap().to_owned();
        let page = NodeId::from_wire(&two).unwrap();
        docs.session.switch_page(page);
        let kind = mastik_core::verbs::example("NodeKind").unwrap();
        let args = json!({ "parent": two, "kind": kind });
        let made = call(&mut engine, &mut docs, 1, "create_node", args);
        let rect = ok(&made)["created"][0].clone();
        let view = call(&mut engine, &mut docs, 1, "get_view", json!({}));
        let view = ok(&view);
        assert_eq!(view["page"]["id"], json!(two));
        assert_eq!(view["page"]["name"], json!("Two"));
        let r = docs.session.camera.viewport((800, 600)).view;
        assert_eq!(view["visible"], json!([r.x0, r.y0, r.width(), r.height()]));
        // The example rectangle is 120 × 80 at the page's origin, where the first
        // page's frame also sits.
        let hit = call(
            &mut engine,
            &mut docs,
            1,
            "hit_test",
            json!({ "x": 10, "y": 10 }),
        );
        assert_eq!(ok(&hit)["ids"], json!([rect]), "on the person's page");
    }

    /// `get_tree` answers a layer in `insert_subtree`'s shape, which inserts back,
    /// and refuses a page.
    ///
    /// flip: `get_tree` without its page refusal → fails at "a page is refused".
    #[test]
    fn get_tree_answers_what_insert_subtree_takes() {
        let (mut engine, mut docs) = (Engine::default(), One::new());
        open(&mut engine, &mut docs, 1);
        let id = rect(&mut engine, &mut docs, 1, "Chip");
        let out = call(&mut engine, &mut docs, 1, "get_tree", json!({ "id": id }));
        let tree = ok(&out)["tree"].clone();
        assert_eq!(tree["name"], json!("Chip"));
        let page = docs.session.page.to_wire();
        let args = json!({ "parent": page, "tree": tree });
        ok(&call(&mut engine, &mut docs, 1, "insert_subtree", args));
        let read = call(&mut engine, &mut docs, 1, "get_tree", json!({ "id": page }));
        assert!(read.is_error, "a page is refused");
    }

    /// A board holding a main and its instance, read with `get_tree` in short ids
    /// and inserted back through the commit door, is a main and an instance of
    /// it again, not a plain copy.
    ///
    /// flip: `verbs::tree` dropping every `id` → fails at "an instance of the
    /// new main", the copy linked to the first main instead.
    #[test]
    fn get_tree_round_trips_a_main_and_its_instance() {
        fn short(engine: &mut Engine, docs: &mut One, tool: &str, args: Value) -> Output {
            let (reply, _answer) = sync_channel(1);
            let mut call = Call::new(1, Some("k".into()), tool, args, reply);
            call.short_ids = true;
            engine.handle(docs, call)
        }
        let (mut engine, mut docs) = (Engine::default(), One::new());
        let out = short(
            &mut engine,
            &mut docs,
            "use_document",
            json!({ "document": "a" }),
        );
        let page = ok(&out)["pages"][0]["id"].as_str().unwrap().to_owned();
        let frame = |w: f64| json!({ "Artboard": { "size": { "width": w, "height": 40 } } });
        let steps = json!([
            { "tool": "create_node", "args": { "parent": page, "kind": frame(400.0), "name": "Board" }, "as": "board" },
            { "tool": "create_node", "args": { "parent": "@board", "kind": frame(120.0), "name": "Button" }, "as": "main" },
            { "tool": "create_text", "args": { "parent": "@main", "text": "Go" } },
            { "tool": "make_component", "args": { "nodes": ["@main"] } },
            { "tool": "create_instance", "args": { "main": "@main", "parent": "@board", "x": 200, "y": 0 } },
        ]);
        let out = short(&mut engine, &mut docs, "batch", json!({ "steps": steps }));
        let board = ok(&out)["steps"][0]["created"][0]
            .as_str()
            .unwrap()
            .to_owned();
        let out = short(&mut engine, &mut docs, "get_tree", json!({ "id": board }));
        let tree = ok(&out)["tree"].clone();
        let link = tree["children"][1]["copy_of"].as_str().unwrap_or_default();
        assert!(
            link.starts_with('n') && !link.contains(':'),
            "short ids: {tree}"
        );
        let args = json!({ "parent": page, "tree": tree });
        let out = short(&mut engine, &mut docs, "insert_subtree", args);
        let s = &docs.session;
        let known = short::known_actors(&s.doc, s.ids.actor());
        let copy = NodeId::from_short(ok(&out)["created"][0].as_str().unwrap(), &known).unwrap();
        let kids = s.doc.get(copy).unwrap().children().to_vec();
        assert!(s.doc.get(kids[0]).unwrap().component(), "a main");
        assert_eq!(
            s.doc.get(kids[1]).unwrap().link(),
            Some(kids[0]),
            "an instance of the new main"
        );
    }

    /// `replace_font` takes a family the host can set, in the host's spelling,
    /// and refuses one it cannot.
    ///
    /// flip: `handle` without its family check → fails at "a family the host
    /// lacks is refused".
    #[test]
    fn replace_font_takes_only_a_family_the_host_can_set() {
        let (mut engine, mut docs) = (Engine::default(), One::new());
        open(&mut engine, &mut docs, 1);
        let page = docs.session.page.to_wire();
        let args = json!({ "parent": page, "text": "Hi" });
        let made = call(&mut engine, &mut docs, 1, "create_text", args);
        let t = NodeId::from_wire(ok(&made)["created"][0].as_str().unwrap()).unwrap();
        let family = |d: &One| match d.session.doc.get(t).unwrap().kind() {
            mastik_core::NodeKind::Text { style, .. } => style.font_family.clone(),
            _ => unreachable!(),
        };
        let from = family(&docs);
        let args = json!({ "from": from, "to": "Nowhere Sans" });
        let out = call(&mut engine, &mut docs, 1, "replace_font", args);
        assert!(out.is_error, "a family the host lacks is refused");
        // As a batch's step too.
        let step =
            json!({ "tool": "replace_font", "args": { "from": from, "to": "Nowhere Sans" } });
        let out = call(
            &mut engine,
            &mut docs,
            1,
            "batch",
            json!({ "steps": [step] }),
        );
        assert!(out.is_error, "a batch step is refused");
        assert_eq!(family(&docs), from);
        // And as a proposal's call.
        // flip: the check reading a proposal call's `tool` → fails here.
        let proposed = json!({ "title": "Swap", "calls": [{ "verb": "replace_font",
            "args": { "from": from, "to": "Nowhere Sans" } }] });
        let out = call(&mut engine, &mut docs, 1, "propose_edit", proposed);
        assert!(out.is_error, "a proposal's call is refused");
        let args = json!({ "from": from, "to": "lora" });
        ok(&call(&mut engine, &mut docs, 1, "replace_font", args));
        assert_eq!(family(&docs), "Lora", "in the host's spelling");
    }

    /// A connection that never called closing is no client leaving, so a host
    /// keeps its documents; one that called is.
    ///
    /// flip: `Engine::closed` answering `true` always → fails at the probe.
    #[test]
    fn a_probe_closing_is_no_client_leaving() {
        let (mut engine, mut docs) = (Engine::default(), One::new());
        open(&mut engine, &mut docs, 1);
        assert!(!engine.closed(&mut docs, 2), "a probe never called");
        assert!(engine.closed(&mut docs, 1), "a client that called left");
    }

    /// A table tool's model argument sent as JSON text is read as the structure
    /// it holds, by the handler as well as the check, and so is `call`'s own
    /// `args`.
    ///
    /// flip: `parse_json_text` leaving `call`'s `args` as text → fails at
    /// "call's args".
    #[test]
    fn json_text_arguments_reach_the_handler_parsed() {
        let (mut engine, mut docs) = (Engine::default(), One::new());
        open(&mut engine, &mut docs, 1);
        let id = rect(&mut engine, &mut docs, 1, "Box");
        let props = json!({ "name": "Card" }).to_string();
        let out = call(
            &mut engine,
            &mut docs,
            1,
            "set",
            json!({ "nodes": [id], "props": props }),
        );
        ok(&out);
        let node = mastik_core::NodeId::from_wire(&id).unwrap();
        assert_eq!(docs.session.doc.get(node).unwrap().name(), "Card", "props");
        let names = json!(["set_point"]).to_string();
        let out = call(
            &mut engine,
            &mut docs,
            1,
            "describe_tools",
            json!({ "names": names }),
        );
        assert_eq!(ok(&out)["tools"][0]["name"], "set_point", "names");
        // `call`'s own `args` as JSON text, and a model argument inside it.
        let args = json!({ "node": id, "name": "Renamed" }).to_string();
        let out = call(
            &mut engine,
            &mut docs,
            1,
            "call",
            json!({ "tool": "set_name", "args": args }),
        );
        ok(&out);
        assert_eq!(
            docs.session.doc.get(node).unwrap().name(),
            "Renamed",
            "call's args"
        );
        let args = json!({ "names": json!(["set"]).to_string() }).to_string();
        let out = call(
            &mut engine,
            &mut docs,
            1,
            "call",
            json!({ "tool": "describe_tools", "args": args }),
        );
        assert_eq!(ok(&out)["tools"][0]["name"], "set", "nested");
    }

    /// `call` answers as the tool it names, listed or not, and never names itself;
    /// `describe_tools` finds a tool by name and by a word.
    ///
    /// flip: `handle` leaving `call` unrewritten → fails at the call's `ok`, as no
    /// tool.
    #[test]
    fn call_runs_any_tool_and_describe_tools_finds_it() {
        let (mut engine, mut docs) = (Engine::default(), One::new());
        open(&mut engine, &mut docs, 1);
        let id = rect(&mut engine, &mut docs, 1, "Box");
        let out = call(
            &mut engine,
            &mut docs,
            1,
            "call",
            json!({ "tool": "set_name", "args": { "node": id, "name": "Called" } }),
        );
        ok(&out);
        let n = docs
            .session
            .doc
            .get(NodeId::from_wire(&id).unwrap())
            .unwrap();
        assert_eq!(n.name(), "Called", "renamed");
        assert!(call(&mut engine, &mut docs, 1, "call", json!({ "tool": "call" })).is_error);
        let found = call(
            &mut engine,
            &mut docs,
            1,
            "describe_tools",
            json!({ "names": ["set_point"], "query": "gradient" }),
        );
        let tools = ok(&found)["tools"].as_array().unwrap().clone();
        let names: Vec<&str> = tools.iter().filter_map(|t| t["name"].as_str()).collect();
        assert!(names.contains(&"set_point"), "{names:?}");
        assert!(tools.iter().all(|t| t["inputSchema"].is_object()));
    }

    /// A subtree names its page and frame once, on the root: a layer below keeps
    /// only a frame that is not the root's.
    ///
    /// flip: `get_node` writing every layer's `page` → fails at "said once".
    #[test]
    fn a_subtree_names_its_page_and_frame_once() {
        let (mut engine, mut docs) = (Engine::default(), One::new());
        open(&mut engine, &mut docs, 1);
        let page = docs.session.doc.first_page().to_wire();
        let tree = json!({ "name": "Card", "kind": { "Artboard": { "size": { "width": 200.0, "height": 100.0 },
            "corner_radii": { "top_left": 0.0, "top_right": 0.0, "bottom_right": 0.0, "bottom_left": 0.0 } } },
            "children": [{ "kind": mastik_core::verbs::example("NodeKind").unwrap() }] });
        let made = call(
            &mut engine,
            &mut docs,
            1,
            "insert_subtree",
            json!({ "parent": page, "tree": tree }),
        );
        let card = ok(&made)["created"][0].clone();
        let read = call(
            &mut engine,
            &mut docs,
            1,
            "get_node",
            json!({ "id": card, "subtree": true }),
        );
        let nodes = &ok(&read)["nodes"];
        assert_eq!(nodes[0]["page"], json!(page), "{nodes}");
        assert!(nodes[1].get("page").is_none(), "said once: {nodes}");
        assert!(nodes[1].get("artboard").is_none(), "{nodes}");
    }

    /// An agent's `undo` takes back its own step, and is refused over the person's
    /// step or another client's, changing nothing, though that client gave the
    /// same name at `initialize`.
    ///
    /// flip: `own_step` answering `Ok(())` always → fails at "the person's step is
    /// refused"; `own_step` judging by `Origin::Mcp`'s name → fails at "another
    /// client of the same name".
    #[test]
    fn undo_takes_back_only_the_callers_own_step() {
        let (mut engine, mut docs) = (Engine::default(), One::new());
        open(&mut engine, &mut docs, 1);
        open(&mut engine, &mut docs, 2);
        let id = NodeId::from_wire(&rect(&mut engine, &mut docs, 1, "Mine")).unwrap();
        let rename = mastik_core::Transaction(vec![mastik_core::Operation::SetName {
            id,
            name: "Theirs".into(),
        }]);
        docs.session.try_commit(rename).unwrap();
        let out = call(&mut engine, &mut docs, 1, "undo", json!({}));
        assert!(out.is_error, "the person's step is refused");
        assert_eq!(docs.session.history.undo_depth(), 2, "nothing changed");
        docs.session.undo();
        let other = call(&mut engine, &mut docs, 2, "undo", json!({}));
        assert!(other.is_error, "another client of the same name");
        assert_eq!(docs.session.history.undo_depth(), 1);
        ok(&call(&mut engine, &mut docs, 1, "undo", json!({})));
        assert_eq!(docs.session.history.undo_depth(), 0, "its own is undone");
    }

    /// `run_script` runs a loop over `nodes` and a layer named in `layers` as one
    /// undo step marked as the client's; a raise after a write, or a `base_rev`
    /// a layer it wrote moved past, changes nothing.
    ///
    /// flip: `run_script` dropping its `base_rev` check → fails at "stale".
    #[test]
    fn run_script_is_one_step_and_a_failure_changes_nothing() {
        let (mut engine, mut docs) = (Engine::default(), One::new());
        open(&mut engine, &mut docs, 1);
        let a = rect(&mut engine, &mut docs, 1, "A");
        let b = rect(&mut engine, &mut docs, 1, "B");
        let depth = docs.session.history.undo_depth();
        let source =
            "for i, l in ipairs(sel) do l.x = i * 100 end\ncard.name = \"Hero\"\nprint(#sel)";
        let args = json!({ "source": source, "nodes": [a, b], "layers": { "card": a } });
        let out = call(&mut engine, &mut docs, 1, "run_script", args);
        assert_eq!(ok(&out)["printed"], json!(["2"]));
        let id = |w: &str| NodeId::from_wire(w).unwrap();
        let left = |docs: &One, w: &str| {
            let r = docs.session.resolved.world_bounds(id(w)).unwrap();
            r.x0.round()
        };
        assert_eq!((left(&docs, &a), left(&docs, &b)), (100.0, 200.0));
        assert_eq!(docs.session.doc.get(id(&a)).unwrap().name(), "Hero");
        assert_eq!(docs.session.history.undo_depth(), depth + 1, "one step");
        let step = docs.session.history.done().next_back().unwrap();
        assert!(matches!(
            &step.note.origin,
            mastik_core::Origin::Mcp { tool, .. } if tool == "run_script"
        ));

        let args = json!({ "source": "sel.x = 5\nerror(\"boom\")", "nodes": [b] });
        let out = call(&mut engine, &mut docs, 1, "run_script", args);
        assert!(out.is_error);
        assert_eq!(left(&docs, &b), 200.0, "a raise takes the write back");
        assert_eq!(docs.session.history.undo_depth(), depth + 1);

        let rev = ok(&call(&mut engine, &mut docs, 1, "get_selection", json!({})))["rev"].clone();
        let rename = mastik_core::Transaction(vec![mastik_core::Operation::SetName {
            id: id(&b),
            name: "Theirs".into(),
        }]);
        docs.session.try_commit(rename).unwrap();
        let args = json!({ "source": "sel.opacity = 0.5", "nodes": [b], "base_rev": rev });
        let out = call(&mut engine, &mut docs, 1, "run_script", args);
        assert!(out.is_error, "stale");
        assert_eq!(docs.session.doc.get(id(&b)).unwrap().opacity(), 1.0);
    }

    /// Under the person's later step on another layer, an agent's `undo` takes
    /// back its own step alone, as a new step, and leaves theirs; a second
    /// `undo` reaches the step before, not the one it made, and is refused
    /// there since the person renamed that layer.
    ///
    /// flip: `undo` taking the latest own step without skipping a revert step →
    /// fails at "the second undo reaches the step before".
    #[test]
    fn undo_reaches_under_the_persons_step_on_another_layer() {
        let (mut engine, mut docs) = (Engine::default(), One::new());
        open(&mut engine, &mut docs, 1);
        let first = NodeId::from_wire(&rect(&mut engine, &mut docs, 1, "First")).unwrap();
        let mine = NodeId::from_wire(&rect(&mut engine, &mut docs, 1, "Mine")).unwrap();
        let theirs = mastik_core::Transaction(vec![mastik_core::Operation::SetName {
            id: first,
            name: "Theirs".into(),
        }]);
        docs.session.try_commit(theirs).unwrap();
        let out = call(&mut engine, &mut docs, 1, "undo", json!({}));
        assert_eq!(ok(&out)["under_later_steps"], true);
        assert!(
            docs.session.doc.get(mine).is_none(),
            "its step is taken back"
        );
        assert_eq!(docs.session.doc.get(first).unwrap().name(), "Theirs");
        assert_eq!(docs.session.history.undo_depth(), 4, "as a step of its own");
        let again = call(&mut engine, &mut docs, 1, "undo", json!({}));
        assert!(
            again.is_error && docs.session.doc.get(mine).is_none(),
            "the second undo reaches the step before: {:?}",
            again.content
        );
        assert_eq!(
            docs.session.history.undo_depth(),
            4,
            "which the rename keeps"
        );
    }

    /// With the client's only step taken back under the person's, a second
    /// `undo` leaves the *Undo* step it made alone rather than putting the
    /// edit back.
    ///
    /// flip: `undo`'s arm for its own *Undo* step on top removed → fails at
    /// `undone: null`.
    #[test]
    fn a_second_undo_never_undoes_its_own_revert() {
        let (mut engine, mut docs) = (Engine::default(), One::new());
        open(&mut engine, &mut docs, 1);
        let mine = NodeId::from_wire(&rect(&mut engine, &mut docs, 1, "Mine")).unwrap();
        let page = docs.session.page;
        let theirs = mastik_core::Transaction(vec![mastik_core::Operation::SetName {
            id: page,
            name: "Theirs".into(),
        }]);
        docs.session.try_commit(theirs).unwrap();
        let out = call(&mut engine, &mut docs, 1, "undo", json!({}));
        assert_eq!(ok(&out)["under_later_steps"], true);
        let depth = docs.session.history.undo_depth();
        let again = call(&mut engine, &mut docs, 1, "undo", json!({}));
        assert_eq!(ok(&again)["undone"], Value::Null);
        assert!(
            docs.session.doc.get(mine).is_none(),
            "the edit stays taken back"
        );
        assert_eq!(docs.session.history.undo_depth(), depth);
    }

    /// The person taking back an agent's step under later ones makes a step of
    /// their own, counted as a commit, which the agent's `undo` cannot reach.
    ///
    /// flip: `EditorSession::revert` keeping the reverted step's agent → fails
    /// at "the person's step".
    #[test]
    fn the_persons_revert_of_an_agent_step_is_theirs() {
        let (mut engine, mut docs) = (Engine::default(), One::new());
        open(&mut engine, &mut docs, 1);
        let mine = NodeId::from_wire(&rect(&mut engine, &mut docs, 1, "Mine")).unwrap();
        let serial = docs.session.history.done().next_back().unwrap().serial;
        let page = docs.session.page;
        let theirs = mastik_core::Transaction(vec![mastik_core::Operation::SetName {
            id: page,
            name: "Theirs".into(),
        }]);
        docs.session.try_commit(theirs).unwrap();
        let commits = docs.session.commits();
        docs.session.revert(serial).unwrap();
        assert!(docs.session.doc.get(mine).is_none());
        assert_eq!(docs.session.commits(), commits + 1, "a revert is a commit");
        let top = docs.session.history.done().next_back().unwrap();
        assert!(
            top.note.agent.is_none() && top.note.origin == mastik_core::Origin::User,
            "the person's step"
        );
        let undo = call(&mut engine, &mut docs, 1, "undo", json!({}));
        assert!(undo.is_error, "the agent cannot undo it");
        assert!(docs.session.doc.get(mine).is_none());
    }

    /// `undo` and `redo` answer the name of the step they moved, as the history
    /// lists it, and `undone: null` at the end of the history.
    ///
    /// flip: `own_step` answering `None` for the client's own step → fails at
    /// the undo's name.
    #[test]
    fn undo_and_redo_name_the_step() {
        let (mut engine, mut docs) = (Engine::default(), One::new());
        open(&mut engine, &mut docs, 1);
        let id = rect(&mut engine, &mut docs, 1, "Chip");
        let args = json!({ "node": id, "name": "Pill" });
        ok(&call(&mut engine, &mut docs, 1, "set_name", args));
        let label = docs
            .session
            .history
            .done()
            .next_back()
            .unwrap()
            .note
            .label
            .clone();
        assert!(!label.is_empty());
        let undo = call(&mut engine, &mut docs, 1, "undo", json!({}));
        assert_eq!(ok(&undo)["undone"], json!(label), "the undo's name");
        let redo = call(&mut engine, &mut docs, 1, "redo", json!({}));
        assert_eq!(ok(&redo)["redone"], json!(label));
        ok(&call(&mut engine, &mut docs, 1, "undo", json!({})));
        ok(&call(&mut engine, &mut docs, 1, "undo", json!({})));
        let none = call(&mut engine, &mut docs, 1, "undo", json!({}));
        assert_eq!(ok(&none)["undone"], Value::Null);
    }

    /// `find_nodes` finds by name, ignoring case, with its ancestors' names; a
    /// read with `since` at the current revision answers `unchanged`.
    #[test]
    fn find_reaches_a_layer_and_since_skips_an_unchanged_read() {
        let (mut engine, mut docs) = (Engine::default(), One::new());
        open(&mut engine, &mut docs, 1);
        rect(&mut engine, &mut docs, 1, "Home header");
        rect(&mut engine, &mut docs, 1, "Footer");
        let found = call(
            &mut engine,
            &mut docs,
            1,
            "find_nodes",
            json!({ "query": "HEADER" }),
        );
        let nodes = ok(&found)["nodes"].as_array().unwrap().clone();
        assert_eq!(nodes.len(), 1);
        assert_eq!(nodes[0]["name"], "Home header");
        assert_eq!(nodes[0]["path"].as_array().unwrap().len(), 1, "the page");
        let rev = ok(&found)["rev"].clone();
        let read = call(
            &mut engine,
            &mut docs,
            1,
            "get_document",
            json!({ "since": rev }),
        );
        assert_eq!(ok(&read)["unchanged"], true);
    }

    /// A 120 × 80 rectangle named `name` at (`x`, `y`) on the first page,
    /// answering its id.
    fn rect_at(engine: &mut Engine, docs: &mut One, name: &str, x: f64, y: f64) -> String {
        let page = docs.session.doc.first_page().to_wire();
        let kind = mastik_core::verbs::example("NodeKind").unwrap();
        let args = json!({
            "parent": page, "kind": kind, "name": name,
            "transform": [1.0, 0.0, 0.0, 1.0, x, y],
        });
        let out = call(engine, docs, 1, "create_node", args);
        ok(&out)["created"][0].as_str().unwrap().to_owned()
    }

    /// Two rectangles named alike, one grouped with a third: `Header` at the
    /// origin, then `Section` holding `Header` at (200, 50.04) and `Body`, on
    /// a page cleared of a new document's sample frame.
    fn two_headers(engine: &mut Engine, docs: &mut One) -> [String; 4] {
        open(engine, docs, 1);
        let sample: Vec<String> = docs
            .session
            .doc
            .get(docs.session.doc.first_page())
            .unwrap()
            .children()
            .iter()
            .map(|c| c.to_wire())
            .collect();
        ok(&call(
            engine,
            docs,
            1,
            "delete_node",
            json!({ "nodes": sample }),
        ));
        let top = rect_at(engine, docs, "Header", 0.0, 0.0);
        let inner = rect_at(engine, docs, "Header", 200.0, 50.04);
        let body = rect_at(engine, docs, "Body", 400.0, 0.0);
        let out = call(engine, docs, 1, "group", json!({ "nodes": [inner, body] }));
        let group = ok(&out)["created"][0].as_str().unwrap().to_owned();
        ok(&call(
            engine,
            docs,
            1,
            "set_name",
            json!({ "node": group, "name": "Section" }),
        ));
        [top, group, inner, body]
    }

    /// `get_outline` answers a page's layers as rows in paint order, a parent
    /// before its children, with world boxes to 0.1, cut at `depth` and `limit`
    /// with the rest counted, and the selection under its root.
    ///
    /// flip: `reads::outline` pushing children unreversed → fails at the row order.
    #[test]
    fn get_outline_maps_a_page_in_one_table() {
        let (mut engine, mut docs) = (Engine::default(), One::new());
        let [top, group, inner, body] = two_headers(&mut engine, &mut docs);
        assert_eq!(
            docs.session
                .doc
                .get(docs.session.page)
                .unwrap()
                .children()
                .len(),
            2,
            "the fixture's page holds the top header and the section"
        );
        ok(&call(
            &mut engine,
            &mut docs,
            1,
            "set_selection",
            json!({ "ids": [body] }),
        ));
        let out = call(&mut engine, &mut docs, 1, "get_outline", json!({}));
        let a = ok(&out);
        assert_eq!(
            a["columns"],
            json!(["id", "name", "kind", "x", "y", "w", "h", "depth"])
        );
        let rows = a["rows"].as_array().unwrap();
        let ids: Vec<&str> = rows.iter().map(|r| r[0].as_str().unwrap()).collect();
        assert_eq!(
            ids,
            [&top, &group, &inner, &body],
            "paint order, parent first"
        );
        assert_eq!(
            rows[2],
            json!([inner, "Header", "rect", 200, 50, 120, 80, 2])
        );
        assert_eq!(rows[1][7], 1);
        assert_eq!(a["more"], 0);
        assert_eq!(a["selected"], json!([body]));

        let shallow = call(
            &mut engine,
            &mut docs,
            1,
            "get_outline",
            json!({ "depth": 1, "limit": 1 }),
        );
        assert_eq!(ok(&shallow)["rows"].as_array().unwrap().len(), 1);
        assert_eq!(
            ok(&shallow)["more"],
            1,
            "the section, its children past depth"
        );

        let one = call(
            &mut engine,
            &mut docs,
            1,
            "get_outline",
            json!({ "id": top }),
        );
        assert_eq!(
            ok(&one)["rows"],
            json!([[top, "Header", "rect", 0, 0, 120, 80, 0]])
        );
        assert_eq!(ok(&one)["selected"], json!([]));
    }

    /// `get_outline` and `find_nodes` give a stroked layer's own box, where
    /// `get_node`'s `world_bounds` take in the stroke.
    ///
    /// flip: `reads::bounds_box` answering the world bounds → fails at the
    /// outline's row.
    #[test]
    fn the_outline_boxes_a_layer_without_its_stroke() {
        let (mut engine, mut docs) = (Engine::default(), One::new());
        open(&mut engine, &mut docs, 1);
        let id = rect_at(&mut engine, &mut docs, "Stroked", 10.0, 10.0);
        let strokes = mastik_core::keyed_by_position([mastik_core::Stroke {
            width: 3.0,
            ..Default::default()
        }]);
        let nid = NodeId::from_wire(&id).unwrap();
        let tx = mastik_core::Transaction(vec![mastik_core::Operation::SetStrokes {
            id: nid,
            strokes,
        }]);
        docs.session.try_commit(tx).unwrap();
        let node = call(&mut engine, &mut docs, 1, "get_node", json!({ "id": id }));
        let inked = &ok(&node)["nodes"][0]["world_bounds"];
        assert!(
            inked[2].as_f64() > Some(120.0),
            "the stroke is drawn: {inked}"
        );
        let out = call(
            &mut engine,
            &mut docs,
            1,
            "get_outline",
            json!({ "id": id }),
        );
        assert_eq!(
            ok(&out)["rows"],
            json!([[id, "Stroked", "rect", 10, 10, 120, 80, 0]])
        );
        let args = json!({ "query": "stroked" });
        let found = call(&mut engine, &mut docs, 1, "find_nodes", args);
        assert_eq!(ok(&found)["nodes"][0]["box"], json!([10, 10, 120, 80]));
    }

    /// `find_nodes` tells two layers of one name apart by their boxes, and marks
    /// the one selected and the group holding it.
    ///
    /// flip: `find_nodes` asking whether a match lies under the selection rather
    /// than holds it → fails at "the section holds the selection".
    #[test]
    fn find_nodes_gives_boxes_and_the_selection() {
        let (mut engine, mut docs) = (Engine::default(), One::new());
        let [top, group, inner, _] = two_headers(&mut engine, &mut docs);
        ok(&call(
            &mut engine,
            &mut docs,
            1,
            "set_selection",
            json!({ "ids": [inner] }),
        ));
        let out = call(
            &mut engine,
            &mut docs,
            1,
            "find_nodes",
            json!({ "query": "e" }),
        );
        let found = ok(&out)["nodes"].as_array().unwrap().clone();
        let by = |id: &str| found.iter().find(|n| n["id"] == id).unwrap().clone();
        assert_eq!(by(&top)["box"], json!([0, 0, 120, 80]));
        assert!(by(&top).get("selected").is_none());
        assert_eq!(by(&inner)["box"], json!([200, 50, 120, 80]));
        assert_eq!(by(&inner)["selected"], true);
        assert_eq!(
            by(&group)["selected"],
            true,
            "the section holds the selection"
        );
    }

    /// A `page` given as the page's name reads as its id, in any case, a batch
    /// step's too; one no page has is refused with the pages and their ids.
    ///
    /// flip: `page_by_name` returning before its lookup → fails at "by name";
    /// `pages_by_name` reading the call's own `page` alone → fails at "a batch
    /// step by name"; passing over an `@` name in a direct call → fails at "an
    /// @ name directly".
    #[test]
    fn a_page_is_found_by_its_name() {
        let (mut engine, mut docs) = (Engine::default(), One::new());
        two_headers(&mut engine, &mut docs);
        let page = docs.session.doc.first_page();
        let name = docs.session.doc.get(page).unwrap().name().to_uppercase();
        let out = call(
            &mut engine,
            &mut docs,
            1,
            "find_nodes",
            json!({ "query": "e", "page": name }),
        );
        assert!(!out.is_error, "by name: {:?}", out.content);
        assert!(!ok(&out)["nodes"].as_array().unwrap().is_empty());
        let out = call(
            &mut engine,
            &mut docs,
            1,
            "find_nodes",
            json!({ "query": "e", "page": "Site" }),
        );
        let why = format!("{:?}", out.content);
        assert!(
            out.is_error && why.contains(&page.to_wire()),
            "the pages named: {why}"
        );
        let draft = json!({ "steps": [
            { "tool": "set_page_draft", "args": { "page": name, "draft": true } },
        ] });
        let out = call(&mut engine, &mut docs, 1, "batch", draft);
        assert!(!out.is_error, "a batch step by name: {:?}", out.content);
        assert!(docs.session.doc.pages()[0].draft, "the page is a draft");

        // A direct call substitutes no labels, so an `@` name is a page's name.
        assert!(docs.session.commit(mastik_core::Transaction(vec![
            mastik_core::Operation::SetName {
                id: page,
                name: "@Mobile".into(),
            }
        ])));
        let out = call(
            &mut engine,
            &mut docs,
            1,
            "set_page_draft",
            json!({ "page": "@Mobile", "draft": false }),
        );
        assert!(!out.is_error, "an @ name directly: {:?}", out.content);
        assert!(!docs.session.doc.pages()[0].draft);
    }

    /// `export_png`'s `region` draws that rectangle of the page, up to 2×, and
    /// refuses a layer `id` and a rectangle with no area.
    ///
    /// flip: the region's scale capped at 1× as a page's is → fails at the size.
    #[test]
    fn export_png_draws_a_region_of_the_page() {
        let (mut engine, mut docs) = (Engine::default(), One::new());
        let [top, ..] = two_headers(&mut engine, &mut docs);
        // The example's solid fill alone; its image fill has no picture here.
        let fills = json!([mastik_core::verbs::example("[Fill]").unwrap()[0]]);
        ok(&call(
            &mut engine,
            &mut docs,
            1,
            "set_fills",
            json!({ "nodes": [top], "fills": fills }),
        ));
        let page = docs.session.page.to_wire();
        let mut look = |args: Value| {
            let out = call(&mut engine, &mut docs, 1, "export_png", args);
            assert!(!out.is_error, "{:?}", out.content);
            match &out.content[0] {
                Content::Image { bytes, .. } => image::load_from_memory(bytes).unwrap().to_rgba8(),
                other => panic!("not an image: {other:?}"),
            }
        };
        let on = look(json!({ "region": [10, 10, 100, 50], "max": 400 }));
        assert_eq!(on.dimensions(), (200, 100));
        assert_eq!(
            on.get_pixel(100, 50).0,
            [0x33, 0x66, 0xcc, 0xff],
            "the header"
        );
        let off = look(json!({ "id": page, "region": [1000, 1000, 100, 50] }));
        assert_ne!(off.get_pixel(100, 50).0, [0x33, 0x66, 0xcc, 0xff]);

        for refused in [
            json!({ "id": top, "region": [0, 0, 10, 10] }),
            json!({ "region": [0, 0, 0, 10] }),
        ] {
            let out = call(&mut engine, &mut docs, 1, "export_png", refused.clone());
            assert!(out.is_error, "{refused} is refused");
        }
    }

    /// `export_png` never draws past `max`: a vast region, and a `max` over the
    /// ceiling, are refused; a layer stops at 2× and a page at 1×; a layer's
    /// far shadow counts toward `max`.
    ///
    /// flip: `png_viewport` not checking the pixel size → fails at the vast region
    /// (or runs out of memory first); the layer arm measuring `world_bounds` →
    /// fails at "the ink stays within".
    #[test]
    fn export_png_stays_within_max() {
        let (mut engine, mut docs) = (Engine::default(), One::new());
        let [top, ..] = two_headers(&mut engine, &mut docs);
        for refused in [
            json!({ "region": [0, 0, 1e9, 1e9] }),
            json!({ "region": [0, 0, 1e9, 1e9], "max": 4096 }),
            json!({ "max": PNG_CEILING + 1 }),
        ] {
            let out = call(&mut engine, &mut docs, 1, "export_png", refused.clone());
            assert!(out.is_error, "{refused} is refused");
        }
        let mut size = |args: Value| {
            let out = call(&mut engine, &mut docs, 1, "export_png", args);
            assert!(!out.is_error, "{:?}", out.content);
            match &out.content[0] {
                Content::Image { bytes, .. } => image::load_from_memory(bytes)
                    .unwrap()
                    .to_rgba8()
                    .dimensions(),
                other => panic!("not an image: {other:?}"),
            }
        };
        let (w, h) = size(json!({ "id": top, "max": 4096 }));
        assert_eq!((w, h), (240, 160), "a 120×80 layer stops at 2×");
        let (w, h) = size(json!({ "max": 4096 }));
        assert_eq!(w, 520, "a page 520 wide stops at 1× ({w}×{h})");
        // A shadow far off the layer's box is drawn, so `max` counts it.
        let far = json!({ "nodes": [top], "effects": [{ "kind": "DropShadow", "color": "#000000",
                          "blur": 4, "offset": [3000, 3000], "spread": 0 }] });
        ok(&call(&mut engine, &mut docs, 1, "set_effects", far));
        let out = call(
            &mut engine,
            &mut docs,
            1,
            "export_png",
            json!({ "id": top, "max": 64 }),
        );
        let Some(Content::Image { bytes, .. }) = out.content.first() else {
            panic!("not an image: {:?}", out.content);
        };
        let (w, h) = image::load_from_memory(bytes)
            .unwrap()
            .to_rgba8()
            .dimensions();
        assert!(w.max(h) <= 65, "the ink stays within `max` ({w}×{h})");
    }

    /// An assistant chat's `export_png` with no `max` is drawn smaller than
    /// another client's, and one that asks for a size gets it.
    ///
    /// flip: the chat default left out of `answer_ids` → fails at "a chat".
    #[test]
    fn a_chat_looks_at_a_smaller_picture_by_default() {
        let (mut engine, mut docs) = (Engine::default(), One::new());
        let grants = engine.grants();
        grants.grant(
            "assistant-1",
            crate::mcp::access::Level::Edit,
            std::sync::Arc::new(|_| false),
        );
        let chat = Some("assistant-1");
        let doc = json!({ "document": "a" });
        ok(&keyed(&mut engine, &mut docs, 1, chat, "use_document", doc));
        open(&mut engine, &mut docs, 2);
        let big = rect(&mut engine, &mut docs, 2, "Big");
        let size = json!({ "nodes": [big], "props": { "size": [1200, 800] } });
        ok(&call(&mut engine, &mut docs, 2, "set", size));
        let mut width = |conn, key, args: Value| {
            let out = keyed(&mut engine, &mut docs, conn, key, "export_png", args);
            assert!(!out.is_error, "{:?}", out.content);
            match &out.content[0] {
                Content::Image { bytes, .. } => image::load_from_memory(bytes).unwrap().width(),
                other => panic!("not an image: {other:?}"),
            }
        };
        assert_eq!(
            width(2, None, json!({ "id": big })),
            PNG_MAX as u32,
            "another client"
        );
        assert_eq!(
            width(1, chat, json!({ "id": big })),
            CHAT_PNG_MAX as u32,
            "a chat"
        );
        assert_eq!(
            width(1, chat, json!({ "id": big, "max": 1400 })),
            1400,
            "a chat's own size"
        );
    }

    /// `run_exports` refuses a document with no settings, then renders a layer's
    /// settings as a named file it returns rather than writes.
    ///
    /// flip: `run_exports` dropping the `Content::File`s → fails at the file count.
    #[test]
    fn run_exports_returns_the_files_settings_ask_for() {
        let (mut engine, mut docs) = (Engine::default(), One::new());
        open(&mut engine, &mut docs, 1);
        let id = rect(&mut engine, &mut docs, 1, "Badge");
        assert!(call(&mut engine, &mut docs, 1, "run_exports", json!({})).is_error);
        let exports = mastik_core::verbs::example("[ExportSpec]").unwrap();
        ok(&call(
            &mut engine,
            &mut docs,
            1,
            "set_exports",
            json!({ "nodes": [id], "exports": exports }),
        ));
        let out = call(&mut engine, &mut docs, 1, "run_exports", json!({}));
        let listed = ok(&out)["files"].as_array().unwrap().clone();
        assert!(!listed.is_empty());
        assert_eq!(listed[0]["layer"], json!(id));
        let files: Vec<&String> = out
            .content
            .iter()
            .filter_map(|c| match c {
                Content::File { name, bytes, .. } if !bytes.is_empty() => Some(name),
                _ => None,
            })
            .collect();
        assert_eq!(files.len(), listed.len());
        assert!(files[0].contains("Badge"), "{files:?}");
    }

    /// The list reads narrow by name with `query`, `find_like` cuts at `limit`
    /// and counts the rest, and any read past the budget is refused naming the
    /// narrower call it takes.
    ///
    /// flip: `read_fits` answering `Ok` always → fails at "refused".
    #[test]
    fn list_reads_narrow_and_every_read_keeps_the_budget() {
        let (mut engine, mut docs) = (Engine::default(), One::new());
        open(&mut engine, &mut docs, 1);
        let a = rect(&mut engine, &mut docs, 1, "A");
        let b = rect(&mut engine, &mut docs, 1, "B");
        let fills = mastik_core::verbs::example("[Fill]").unwrap();
        let args = json!({ "nodes": [a, b], "fills": fills });
        ok(&call(&mut engine, &mut docs, 1, "set_fills", args));
        let args = json!({ "id": a, "by": "fill", "limit": 1 });
        let like = call(&mut engine, &mut docs, 1, "find_like", args);
        let like = ok(&like);
        assert_eq!(like["nodes"].as_array().unwrap().len(), 1, "{like}");
        assert_eq!(like["more"], 1, "{like}");

        let answer = json!({ "styles": [{ "name": "Brand/Blue" }, { "name": "Grey" }] });
        let kept = reads::named(answer, "styles", &json!({ "query": "blue" }));
        assert_eq!(kept["styles"].as_array().unwrap().len(), 1, "{kept}");

        let big = Answer::Json(json!({ "styles": "x".repeat(ANSWER_BUDGET) }));
        let refused = read_fits("list_styles", big);
        assert!(
            refused.as_ref().is_err_and(|why| why.contains("`query`")),
            "refused"
        );
        let write = Answer::Json(json!({ "made": "x".repeat(ANSWER_BUDGET) }));
        assert!(
            read_fits("set_name", write).is_ok(),
            "a write is never refused after the fact"
        );
    }

    /// A read the document makes too long is refused through the call itself,
    /// naming `query`.
    ///
    /// flip: `answer_ids` without its `read_fits` → fails at "refused".
    #[test]
    fn a_read_past_the_budget_is_refused_through_the_call() {
        let (mut engine, mut docs) = (Engine::default(), One::new());
        open(&mut engine, &mut docs, 1);
        let fills = mastik_core::keyed_by_position([mastik_core::Fill {
            brush: mastik_core::peniko::Color::from_rgba8(1, 2, 3, 255).into(),
            visible: true,
        }]);
        let pad = "x".repeat(200);
        let ops = (0..600)
            .map(|i| mastik_core::Operation::AddStyle {
                style: mastik_core::style::Style {
                    id: docs.session.ids.mint_style(),
                    name: format!("Style {i} {pad}"),
                    value: mastik_core::style::StyleValue::Fills(fills.clone()),
                    origin: None,
                    forked: None,
                    description: String::new(),
                },
            })
            .collect();
        assert!(docs.session.commit(mastik_core::Transaction(ops)));
        let out = call(&mut engine, &mut docs, 1, "list_styles", json!({}));
        let text = format!("{:?}", out.content);
        assert!(out.is_error && text.contains("`query`"), "refused");
        ok(&call(
            &mut engine,
            &mut docs,
            1,
            "list_styles",
            json!({ "query": "style 7 " }),
        ));
    }

    /// A row whose raster would pass the per-file pixel ceiling is refused by
    /// name before anything is drawn.
    ///
    /// flip: `run_exports` without its pixel check → fails at "refused". The
    /// 120×80 rectangle at 100× sits just past the ceiling, so the flip draws
    /// 384 MB, not gigabytes.
    #[test]
    fn run_exports_refuses_a_raster_past_the_ceiling() {
        let (mut engine, mut docs) = (Engine::default(), One::new());
        open(&mut engine, &mut docs, 1);
        let id = rect(&mut engine, &mut docs, 1, "Badge");
        let huge = vec![mastik_core::ExportSpec::new(
            mastik_core::ExportFormat::Png,
            mastik_core::ExportScale::Times(100.0),
        )];
        let args = json!({ "nodes": [id], "exports": huge });
        ok(&call(&mut engine, &mut docs, 1, "set_exports", args));
        let out = call(&mut engine, &mut docs, 1, "run_exports", json!({}));
        assert!(out.is_error, "refused");
        let text = format!("{:?}", out.content);
        assert!(
            text.contains("Badge@100x.png") && text.contains(&id),
            "{text}"
        );
    }

    /// `rows` keeps a *One file per row* setting to the rows asked for, each
    /// file saying its row; a run past the table's end says how many rows it has.
    ///
    /// flip: `run_exports` not keeping the plan to `rows` → fails at "row 2 alone";
    /// without its past-the-end check → fails at "the row count".
    #[test]
    fn run_exports_takes_a_run_of_rows() {
        let (mut engine, mut docs) = (Engine::default(), One::new());
        open(&mut engine, &mut docs, 1);
        let id = rect(&mut engine, &mut docs, 1, "Badge");
        let args = json!({ "name": "People", "columns": ["Name"], "rows": [["Ada"], ["Alan"], ["Grace"]] });
        let table = ok(&call(&mut engine, &mut docs, 1, "add_table", args))["table"].clone();
        let exports = json!([{ "format": "Png", "per_row": { "table": table } }]);
        let args = json!({ "nodes": [id], "exports": exports });
        ok(&call(&mut engine, &mut docs, 1, "set_exports", args));
        let all = call(&mut engine, &mut docs, 1, "run_exports", json!({}));
        assert_eq!(ok(&all)["files"].as_array().map(Vec::len), Some(3));
        let args = json!({ "rows": { "from": 2, "count": 1 } });
        let some = call(&mut engine, &mut docs, 1, "run_exports", args);
        let listed = ok(&some)["files"].as_array().unwrap().clone();
        assert_eq!(listed.len(), 1, "row 2 alone: {listed:?}");
        assert_eq!(listed[0]["row"], json!(2));
        assert_eq!(listed[0]["path"], json!("Badge/2.png"));
        let bad = json!({ "rows": { "from": 0, "count": 1 } });
        assert!(call(&mut engine, &mut docs, 1, "run_exports", bad).is_error);
        let past = json!({ "rows": { "from": 4, "count": 1 } });
        let past = call(&mut engine, &mut docs, 1, "run_exports", past);
        let said = format!("{:?}", past.content);
        assert!(past.is_error, "{said}");
        assert!(
            said.contains("3 rows") && said.contains("past its end"),
            "the row count: {said}"
        );
    }

    /// `export_code` answers files and the three report lists.
    #[test]
    fn export_code_answers_files_and_the_report() {
        let (mut engine, mut docs) = (Engine::default(), One::new());
        open(&mut engine, &mut docs, 1);
        let id = rect(&mut engine, &mut docs, 1, "Card");
        let out = call(
            &mut engine,
            &mut docs,
            1,
            "export_code",
            json!({ "target": "html", "ids": [id] }),
        );
        let report = ok(&out);
        assert!(report["files"].as_array().is_some_and(|f| !f.is_empty()));
        assert!(report.get("approximated").is_some());
        assert!(
            out.content
                .iter()
                .any(|c| matches!(c, Content::File { name, .. } if name.ends_with(".html")))
        );
        let bad = call(
            &mut engine,
            &mut docs,
            1,
            "export_code",
            json!({ "target": "svelte" }),
        );
        assert!(bad.is_error);
    }

    /// Two flows, `Browse` and `Checkout`, on the first page, and the page's id.
    fn two_flows(engine: &mut Engine, docs: &mut One) -> String {
        let a = screen(engine, docs, 1, "A");
        let b = screen(engine, docs, 1, "B");
        let page = docs.session.doc.first_page().to_wire();
        let props = json!({ "flows": [
            { "name": "Browse", "start": a },
            { "name": "Checkout", "start": b },
        ] });
        ok(&call(
            engine,
            docs,
            1,
            "set",
            json!({ "nodes": [page], "props": props }),
        ));
        page
    }

    /// The one file a file export answered, by name and bytes.
    fn only_file(out: &Output) -> (&str, &[u8]) {
        let files: Vec<(&str, &[u8])> = out
            .content
            .iter()
            .filter_map(|c| match c {
                Content::File { name, bytes, .. } => Some((name.as_str(), bytes.as_slice())),
                _ => None,
            })
            .collect();
        assert_eq!(files.len(), 1, "one file: {out:?}");
        files[0]
    }

    /// `export_prototype` refuses a document with no flow, then answers the HTML
    /// file the document's own export settings ask for: opening on the flow
    /// they name, in its name and title.
    ///
    /// flip: `html_name` not finding the flow `opens` names → fails at the
    /// file's name.
    #[test]
    fn export_prototype_answers_the_file_its_settings_open_on() {
        let (mut engine, mut docs) = (Engine::default(), One::new());
        open(&mut engine, &mut docs, 1);
        let none = call(&mut engine, &mut docs, 1, "export_prototype", json!({}));
        assert!(none.is_error, "no flow: {none:?}");
        two_flows(&mut engine, &mut docs);
        let page = docs.session.doc.first_page();
        let checkout = docs.session.doc.page(page).unwrap().flows[1].id;
        let opens = mastik_core::prototype::FlowRef {
            page,
            flow: checkout,
        };
        let settings = mastik_core::prototype::PlayExport {
            opens: Some(opens),
            ..docs.session.doc.play_export().clone()
        };
        docs.session
            .doc
            .apply(&mastik_core::Transaction(vec![
                mastik_core::Operation::SetPlayExport { settings },
            ]))
            .unwrap();
        let out = call(&mut engine, &mut docs, 1, "export_prototype", json!({}));
        let report = ok(&out);
        assert!(!report["settings"]["opens"].is_null(), "{report}");
        let (name, bytes) = only_file(&out);
        assert_eq!(name, "Prototype — Checkout.html");
        assert_eq!(report["files"][0]["path"], json!(name));
        let html = String::from_utf8_lossy(bytes);
        assert!(html.contains("<title>Checkout</title>"), "the title");
    }

    /// `export_prototype` is held by an interaction acting on a layer that is
    /// gone, naming the layer, as the export card is.
    ///
    /// flip: `export_prototype` not checking `holds` → fails at "held".
    #[test]
    fn export_prototype_is_held_by_a_broken_interaction() {
        let (mut engine, mut docs) = (Engine::default(), One::new());
        open(&mut engine, &mut docs, 1);
        two_flows(&mut engine, &mut docs);
        let a = rect(&mut engine, &mut docs, 1, "Button");
        let gone = screen(&mut engine, &mut docs, 1, "Gone");
        let props = json!({
            "reactions": [{ "trigger": "Click", "actions": [{ "Navigate": { "to": gone } }] }],
        });
        ok(&call(
            &mut engine,
            &mut docs,
            1,
            "set",
            json!({ "nodes": [a], "props": props }),
        ));
        ok(&call(
            &mut engine,
            &mut docs,
            1,
            "delete_node",
            json!({ "nodes": [gone] }),
        ));
        let out = call(&mut engine, &mut docs, 1, "export_prototype", json!({}));
        assert!(out.is_error, "held: {out:?}");
        let said = format!("{out:?}");
        assert!(said.contains(&a) && said.contains("gone"), "{said}");
    }

    /// `export_flow_map` refuses a document with no flow, then answers the PDF
    /// with what it drew differently.
    ///
    /// flip: `export_flow_map` answering the report without its file → fails
    /// at "one file".
    #[test]
    fn export_flow_map_answers_the_pdf() {
        let (mut engine, mut docs) = (Engine::default(), One::new());
        open(&mut engine, &mut docs, 1);
        let none = call(&mut engine, &mut docs, 1, "export_flow_map", json!({}));
        assert!(none.is_error, "no flow: {none:?}");
        two_flows(&mut engine, &mut docs);
        let out = call(&mut engine, &mut docs, 1, "export_flow_map", json!({}));
        assert!(ok(&out)["approximated"].is_array());
        let (name, bytes) = only_file(&out);
        assert_eq!(name, "Prototype — Flow map.pdf");
        assert!(bytes.starts_with(b"%PDF"), "a PDF");
    }

    /// `get_prototype` answers each flow with the frames its links reach and
    /// each interaction in words by its layer, and names what holds the export.
    ///
    /// flip: `prototype` answering only each flow's start → fails at "B is
    /// reached"; `holds` left out → fails at "the hold".
    #[test]
    fn get_prototype_reads_each_flow_and_what_holds_it() {
        let (mut engine, mut docs) = (Engine::default(), One::new());
        open(&mut engine, &mut docs, 1);
        two_flows(&mut engine, &mut docs);
        let page = docs.session.doc.first_page();
        let flows = docs.session.doc.page(page).unwrap().flows.clone();
        let (a, b) = (flows[0].start.to_wire(), flows[1].start.to_wire());
        let kind = mastik_core::verbs::example("NodeKind").unwrap();
        let args = json!({ "parent": a, "kind": kind, "name": "Next" });
        let next = ok(&call(&mut engine, &mut docs, 1, "create_node", args))["created"][0].clone();
        let gone = screen(&mut engine, &mut docs, 1, "Gone");
        let props = json!({ "reactions": [
            { "trigger": "Click", "actions": [{ "Navigate": { "to": b } }] },
            { "trigger": "DoubleClick", "actions": [{ "OpenOverlay": { "overlay": gone } }] },
        ] });
        ok(&call(
            &mut engine,
            &mut docs,
            1,
            "set",
            json!({ "nodes": [next], "props": props }),
        ));
        ok(&call(
            &mut engine,
            &mut docs,
            1,
            "delete_node",
            json!({ "nodes": [gone] }),
        ));
        let read = ok(&call(&mut engine, &mut docs, 1, "get_prototype", json!({}))).clone();
        let browse = &read["flows"][0];
        assert_eq!(browse["name"], "Browse", "{read}");
        let frames: Vec<&Value> = browse["frames"]
            .as_array()
            .unwrap()
            .iter()
            .map(|f| &f["id"])
            .collect();
        assert_eq!(frames, [&json!(a), &json!(b)], "B is reached: {read}");
        let first = &browse["interactions"][0];
        assert_eq!(
            (first["layer"].clone(), first["layer_name"].clone()),
            (next, json!("Next"))
        );
        assert!(
            first["does"].as_str().unwrap().contains("B"),
            "in words: {first}"
        );
        assert_eq!(
            read["holds"].as_array().map(Vec::len),
            Some(1),
            "the hold: {read}"
        );
    }

    /// An export's file packs on a worker: the call is kept waiting, not
    /// answered on the pass that starts it, and `run_later` answers it on its own
    /// reply once the worker is done; a refused export is answered at once.
    ///
    /// flip: the export arm skipped, so the export reaches no tool → fails at
    /// "kept waiting"; it dropping the worker's answer and waiting on → the
    /// call is answered "stopped", failing at `only_file`.
    #[test]
    fn an_export_waits_for_its_worker_and_is_answered_on_its_reply() {
        let (mut engine, mut docs) = (Engine::default(), One::new());
        open(&mut engine, &mut docs, 1);
        let start = |engine: &mut Engine, docs: &mut One| {
            let (reply, answer) = sync_channel(1);
            let call = Call::new(1, None, "export_flow_map", json!({}), reply);
            (engine.handle_or_wait(docs, call), answer)
        };
        let (refused, _) = start(&mut engine, &mut docs);
        assert!(
            refused.is_some_and(|o| o.is_error),
            "no flow: answered at once"
        );
        two_flows(&mut engine, &mut docs);
        let (now, answer) = start(&mut engine, &mut docs);
        assert!(now.is_none() && engine.has_later(), "kept waiting");
        let until = std::time::Instant::now() + std::time::Duration::from_secs(30);
        let out = loop {
            engine.run_later(&mut docs);
            if let Ok(out) = answer.try_recv() {
                break out;
            }
            assert!(std::time::Instant::now() < until, "answered");
            std::thread::yield_now();
        };
        assert!(only_file(&out).1.starts_with(b"%PDF"), "the PDF");
        assert!(!engine.has_later(), "nothing left waiting");
    }

    /// The three table reads list what the verbs made: a variable with its
    /// resolved value per mode, a style with its users, a main component with
    /// the properties it owns; cells and values in the short forms writes take.
    ///
    /// flip: `components` listing properties for sets alone → fails at "a plain
    /// main's properties are listed"; `variables` without `shorten` → fails at "a
    /// cell as set_variable_value takes it".
    #[test]
    fn the_lists_show_variables_styles_and_components() {
        let (mut engine, mut docs) = (Engine::default(), One::new());
        open(&mut engine, &mut docs, 1);
        let cell = mastik_core::verbs::example("Cell").unwrap();
        let args = json!({ "kind": "color", "name": "brand/blue", "value": cell });
        ok(&call(&mut engine, &mut docs, 1, "add_variable", args));
        let vars = call(&mut engine, &mut docs, 1, "list_variables", json!({}));
        let v = &ok(&vars)["variables"][0];
        assert_eq!(v["name"], "brand/blue");
        let resolved = v["resolved"].as_object().unwrap();
        assert_eq!(resolved.values().next(), Some(&json!("#3366ccff")));
        let cells = v["cells"].as_object().unwrap();
        assert_eq!(
            cells.values().next(),
            Some(&json!({ "color": "#3366cc" })),
            "a cell as set_variable_value takes it"
        );

        let id = rect(&mut engine, &mut docs, 1, "Chip");
        let value = mastik_core::verbs::example("StyleValue").unwrap();
        let args = json!({ "value": value, "name": "Surface", "nodes": [id] });
        ok(&call(&mut engine, &mut docs, 1, "add_style", args));
        let styles = call(&mut engine, &mut docs, 1, "list_styles", json!({}));
        let s = &ok(&styles)["styles"][0];
        assert_eq!(s["name"], "Surface");
        assert_eq!(s["slot"], "fills");
        assert_eq!(s["users"], 1);
        assert!(
            !s["value"].to_string().contains("components"),
            "a style's value in the short forms: {s}"
        );

        // A rectangle becomes a main inside a frame of its own.
        let made = call(
            &mut engine,
            &mut docs,
            1,
            "make_component",
            json!({ "nodes": [id] }),
        );
        let main = ok(&made)["created"][0].clone();
        // A main in no set owns its properties, which `set_instance_property`
        // then names.
        let args = json!({ "node": id, "kind": "boolean", "name": "Shown" });
        ok(&call(&mut engine, &mut docs, 1, "define_property", args));
        let comps = call(&mut engine, &mut docs, 1, "list_components", json!({}));
        let listed = ok(&comps)["components"].as_array().unwrap().clone();
        assert_eq!(listed.len(), 1);
        assert_eq!(listed[0]["kind"], "main");
        assert_eq!(listed[0]["id"], main);
        assert!(
            listed[0]["properties"].to_string().contains("Shown"),
            "a plain main's properties are listed: {}",
            listed[0]
        );
    }

    /// An instance reads back the property values it is set to and each copy the
    /// fields it overrides, in the words `set_instance_property` and
    /// `reset_overrides` take.
    ///
    /// flip: `component_summary` reading `overrides` off the instance root alone →
    /// fails at "the copy's opacity".
    #[test]
    fn an_instance_reads_back_its_properties_and_overrides() {
        let (mut engine, mut docs) = (Engine::default(), One::new());
        open(&mut engine, &mut docs, 1);
        let id = rect(&mut engine, &mut docs, 1, "Dot");
        let made = call(
            &mut engine,
            &mut docs,
            1,
            "make_component",
            json!({ "nodes": [id] }),
        );
        let main = ok(&made)["created"][0].clone();
        let args = json!({ "node": id, "kind": "boolean", "name": "Shown" });
        ok(&call(&mut engine, &mut docs, 1, "define_property", args));
        let page = docs.session.doc.first_page().to_wire();
        let args = json!({ "main": main, "parent": page, "x": 500, "y": 500 });
        let made = call(&mut engine, &mut docs, 1, "create_instance", args);
        let instance = ok(&made)["created"][0].clone();
        let args = json!({ "node": instance, "property": "Shown", "value": false });
        ok(&call(
            &mut engine,
            &mut docs,
            1,
            "set_instance_property",
            args,
        ));

        let read = |engine: &mut Engine, docs: &mut One, id: &Value| {
            let out = call(engine, docs, 1, "get_node", json!({ "id": id }));
            ok(&out)["nodes"][0].clone()
        };
        let root = read(&mut engine, &mut docs, &instance);
        let props = &root["component"]["properties"];
        assert_eq!(props[0]["name"], "Shown", "{root}");
        assert_eq!(props[0]["kind"], "boolean");
        assert_eq!(props[0]["value"], false);
        assert_eq!(props[0]["overridden"], true);

        let copy = root["children"][0].clone();
        let args = json!({ "nodes": [copy], "opacity": 0.5 });
        ok(&call(&mut engine, &mut docs, 1, "set_opacity", args));
        let node = read(&mut engine, &mut docs, &copy);
        let overrides = node["component"]["overrides"]
            .as_array()
            .cloned()
            .unwrap_or_default();
        assert!(
            overrides.contains(&json!("opacity")),
            "the copy's opacity: {overrides:?}"
        );
        assert!(overrides.contains(&json!("visible")), "{overrides:?}");
        let main_node = read(&mut engine, &mut docs, &main);
        assert!(main_node["component"].get("overrides").is_none());
    }

    /// `outline_text` turns a text the session laid out into a path, through the
    /// engine as an agent calls it.
    #[test]
    fn outline_text_turns_text_into_a_path() {
        let (mut engine, mut docs) = (Engine::default(), One::new());
        open(&mut engine, &mut docs, 1);
        let page = docs.session.doc.first_page().to_wire();
        let made = call(
            &mut engine,
            &mut docs,
            1,
            "create_text",
            json!({ "parent": page, "text": "Hello" }),
        );
        let text = ok(&made)["created"][0].clone();
        let out = call(
            &mut engine,
            &mut docs,
            1,
            "outline_text",
            json!({ "node": text }),
        );
        let path = NodeId::from_wire(ok(&out)["created"][0].as_str().unwrap()).unwrap();
        assert!(matches!(
            docs.session.doc.get(path).map(|n| n.kind()),
            Some(mastik_core::NodeKind::Path { .. })
        ));
    }

    /// A verb's line on what it did reaches the agent as `said`.
    ///
    /// flip: `write` dropping `done.said` → fails at the `said` assertion.
    #[test]
    fn a_verbs_said_line_reaches_the_agent() {
        let (mut engine, mut docs) = (Engine::default(), One::new());
        open(&mut engine, &mut docs, 1);
        let page = docs.session.doc.first_page().to_wire();
        ok(&call(
            &mut engine,
            &mut docs,
            1,
            "create_text",
            json!({ "parent": page, "text": "Log in" }),
        ));
        let out = call(
            &mut engine,
            &mut docs,
            1,
            "replace_text",
            json!({ "find": "Log in", "replace": "Sign in" }),
        );
        assert_eq!(ok(&out)["said"], "Replaced 1 match in 1 layer");
    }

    /// A fill list of one solid colour, as `set_fills` takes it.
    fn solid(r: u8, g: u8, b: u8) -> Value {
        use mastik_core::peniko::{Brush, Color};
        serde_json::to_value(vec![mastik_core::Fill {
            brush: Brush::Solid(Color::from_rgb8(r, g, b)).into(),
            visible: true,
        }])
        .unwrap()
    }

    /// `list_fonts` needs no document and filters by name, ignoring case.
    #[test]
    fn list_fonts_filters_by_name() {
        let (mut engine, mut docs) = (Engine::default(), One::new());
        let out = call(
            &mut engine,
            &mut docs,
            1,
            "list_fonts",
            json!({ "query": "inter" }),
        );
        assert_eq!(ok(&out)["families"], json!(["Inter", "Inter Tight"]));
        assert_eq!(ok(&out)["matched"], 2);
    }

    /// `find_like` by fill finds the layers painted the same, the source with
    /// them, and refuses a layer with nothing to match.
    ///
    /// flip: `shares` comparing fill counts only → fails at the two-layer count.
    #[test]
    fn find_like_matches_by_fill() {
        let (mut engine, mut docs) = (Engine::default(), One::new());
        open(&mut engine, &mut docs, 1);
        // One fill each, so only the colour tells C apart.
        let fills = solid(0, 128, 0);
        let ids: Vec<String> = ["A", "B", "C"]
            .iter()
            .map(|n| rect(&mut engine, &mut docs, 1, n))
            .collect();
        for id in &ids[..2] {
            ok(&call(
                &mut engine,
                &mut docs,
                1,
                "set_fills",
                json!({ "nodes": [id], "fills": fills }),
            ));
        }
        let red = solid(255, 0, 0);
        let set = call(
            &mut engine,
            &mut docs,
            1,
            "set_fills",
            json!({ "nodes": [ids[2]], "fills": red }),
        );
        ok(&set);
        let out = call(
            &mut engine,
            &mut docs,
            1,
            "find_like",
            json!({ "id": ids[0], "by": "fill" }),
        );
        let found: Vec<&str> = ok(&out)["nodes"]
            .as_array()
            .unwrap()
            .iter()
            .map(|n| n["name"].as_str().unwrap())
            .collect();
        assert_eq!(found, ["A", "B"]);
        let bare = rect(&mut engine, &mut docs, 1, "D");
        assert!(
            call(
                &mut engine,
                &mut docs,
                1,
                "find_like",
                json!({ "id": bare, "by": "stroke" })
            )
            .is_error
        );
    }

    /// `check_contrast` measures a text layer on the page and says whether it
    /// passes, listing a passing one only when asked for `all`.
    ///
    /// flip: `check_contrast` keeping every finding without `all` → fails at "a
    /// pass is not listed".
    #[test]
    fn check_contrast_measures_text() {
        let (mut engine, mut docs) = (Engine::default(), One::new());
        open(&mut engine, &mut docs, 1);
        let page = docs.session.doc.first_page().to_wire();
        ok(&call(
            &mut engine,
            &mut docs,
            1,
            "create_text",
            json!({ "parent": page, "text": "Hello" }),
        ));
        let out = call(&mut engine, &mut docs, 1, "check_contrast", json!({}));
        let quiet = ok(&out);
        assert_eq!(
            quiet["findings"],
            json!([]),
            "a pass is not listed: {quiet}"
        );
        assert!(quiet["measured"].as_u64() > Some(0), "but counted: {quiet}");
        let args = json!({ "all": true });
        let out = call(&mut engine, &mut docs, 1, "check_contrast", args);
        let report = ok(&out);
        let findings = report["findings"].as_array().unwrap();
        assert!(
            findings.iter().any(|f| f["trail"] == json!(["Hello"])
                || f["trail"].as_array().is_some_and(|t| t.len() == 1)),
            "{report}"
        );
        assert_eq!(report["failing"], 0, "black on white passes: {report}");
        assert!(
            call(
                &mut engine,
                &mut docs,
                1,
                "check_contrast",
                json!({ "level": "x" })
            )
            .is_error
        );
        // A text too faint to read is listed without `all`, and counted.
        // flip: `check_contrast` keeping passes in its default list → fails at
        // "only the failing one".
        let faint = call(
            &mut engine,
            &mut docs,
            1,
            "create_text",
            json!({ "parent": page, "text": "Faint", "y": 100 }),
        );
        let faint = ok(&faint)["created"][0].clone();
        let fills = json!({ "nodes": [faint], "fills": ["#eeeeee"] });
        ok(&call(&mut engine, &mut docs, 1, "set_fills", fills));
        let out = call(&mut engine, &mut docs, 1, "check_contrast", json!({}));
        let report = ok(&out);
        assert_eq!(report["failing"], 1, "{report}");
        let listed = report["findings"].as_array().unwrap();
        assert_eq!(listed.len(), 1, "only the failing one: {report}");
    }

    /// `review_colours` groups two near-identical fills, and `merge_colours`
    /// makes them one, after which the review has no group.
    #[test]
    fn review_colours_groups_what_merge_colours_merges() {
        let (mut engine, mut docs) = (Engine::default(), One::new());
        open(&mut engine, &mut docs, 1);
        for (name, r) in [("A", 0x80), ("B", 0x81)] {
            let id = rect(&mut engine, &mut docs, 1, name);
            ok(&call(
                &mut engine,
                &mut docs,
                1,
                "set_fills",
                json!({ "nodes": [id], "fills": solid(r, 0x33, 0x66) }),
            ));
        }
        let out = call(&mut engine, &mut docs, 1, "review_colours", json!({}));
        let groups = ok(&out)["groups"].as_array().unwrap().clone();
        assert_eq!(groups.len(), 1, "{groups:?}");
        let colours = groups[0]["colours"].as_array().unwrap();
        let (into, other) = (colours[0]["colour"].clone(), colours[1]["colour"].clone());
        ok(&call(
            &mut engine,
            &mut docs,
            1,
            "merge_colours",
            json!({ "colours": [other], "into": into }),
        ));
        let again = call(&mut engine, &mut docs, 1, "review_colours", json!({}));
        assert_eq!(ok(&again)["groups"], json!([]));
    }

    /// `import_tokens` adds a token file's variable in one undo step, and
    /// `export_tokens` writes it back out.
    ///
    /// flip: `import_tokens` skipping its commit → fails at the variable count.
    #[test]
    fn tokens_import_and_export() {
        let (mut engine, mut docs) = (Engine::default(), One::new());
        open(&mut engine, &mut docs, 1);
        let file = json!({ "brand": { "$type": "color", "$value": "#ff0000" } });
        let out = call(
            &mut engine,
            &mut docs,
            1,
            "import_tokens",
            json!({ "tokens": file }),
        );
        assert!(
            ok(&out)["added"].as_u64() >= Some(1),
            "{:?}",
            out.structured
        );
        assert!(
            docs.session
                .doc
                .variables()
                .iter()
                .any(|v| v.name == "brand")
        );
        assert_eq!(docs.session.history.undo_depth(), 1);
        let out = call(&mut engine, &mut docs, 1, "export_tokens", json!({}));
        let written = ok(&out)["tokens"].as_str().unwrap().to_owned();
        assert!(written.contains("\"brand\""), "{written}");
        assert!(
            call(
                &mut engine,
                &mut docs,
                1,
                "import_tokens",
                json!({ "tokens": "[1, 2]" })
            )
            .is_error
        );
    }

    /// A type style whose family the host lacks is said in the import's answer,
    /// and one in a family the host has is not.
    ///
    /// flip: `import_tokens` filtering nothing out by the host's fonts → fails at
    /// "Inter is the host's".
    #[test]
    fn tokens_import_says_a_family_the_host_lacks() {
        let (mut engine, mut docs) = (Engine::default(), One::new());
        open(&mut engine, &mut docs, 1);
        let ty = |family: &str| {
            json!({ "$type": "typography", "$value": {
            "fontFamily": family, "fontSize": "16px", "fontWeight": 400, "lineHeight": 1.4 } })
        };
        let file = json!({ "body": ty("Inter"), "display": ty("Nowhere Serif") });
        let out = call(
            &mut engine,
            &mut docs,
            1,
            "import_tokens",
            json!({ "tokens": file }),
        );
        let said = ok(&out)["said"].as_str().unwrap_or_default().to_owned();
        assert!(
            said.contains("`Nowhere Serif`"),
            "the missing family: {said}"
        );
        assert!(!said.contains("`Inter`"), "Inter is the host's: {said}");
    }

    /// A keyed client that closes its connection and comes back on another, as a
    /// harness starting a process per turn does, keeps its document, hears what
    /// the person changed in between as `changed`, and its `since` and `base_rev`
    /// still work. An unkeyed connection's state ends with it.
    ///
    /// flip: `Engine::closed` removing keyed state too → fails at the first call
    /// back, refused with *No document yet*.
    #[test]
    fn a_keyed_client_keeps_its_state_across_connections() {
        let (mut engine, mut docs) = (Engine::default(), One::new());
        let k = Some("chat-1");
        ok(&keyed(
            &mut engine,
            &mut docs,
            1,
            k,
            "use_document",
            json!({ "document": "a" }),
        ));
        let page = docs.session.doc.first_page().to_wire();
        let kind = mastik_core::verbs::example("NodeKind").unwrap();
        let made = keyed(
            &mut engine,
            &mut docs,
            1,
            k,
            "create_node",
            json!({ "parent": page, "kind": kind, "name": "Card" }),
        );
        let card = ok(&made)["created"][0].clone();
        let base = ok(&made)["rev"].clone();
        engine.closed(&mut docs, 1);

        // The person renames it while no connection is open.
        open(&mut engine, &mut docs, 9);
        ok(&call(
            &mut engine,
            &mut docs,
            9,
            "set_name",
            json!({ "node": card, "name": "Hero" }),
        ));

        let back = keyed(&mut engine, &mut docs, 2, k, "get_selection", json!({}));
        let back = ok(&back);
        assert_eq!(
            back["changed"]["changed"][0]["id"], card,
            "the document is remembered, and the rename reported: {back}"
        );
        let since = keyed(
            &mut engine,
            &mut docs,
            2,
            k,
            "get_document",
            json!({ "since": base }),
        );
        assert_eq!(ok(&since)["since"], base, "answered as a delta");
        let stale = keyed(
            &mut engine,
            &mut docs,
            2,
            k,
            "set_name",
            json!({ "node": card, "name": "Mine", "base_rev": base }),
        );
        let Content::Text(why) = &stale.content[0] else {
            panic!("no text")
        };
        assert!(
            stale.is_error && why.contains("Card"),
            "refused by the name it read: {why}"
        );

        engine.closed(&mut docs, 9);
        assert!(
            call(&mut engine, &mut docs, 10, "get_selection", json!({})).is_error,
            "a new unkeyed connection starts with no document"
        );
    }

    /// The proxy's restore of the document a keyed client already uses keeps its
    /// history and reports nothing, so the agent's own next call hears what
    /// changed; a plain `use_document` of it reports the change itself.
    ///
    /// flip: `use_document` clearing `shown` whatever document it names → fails
    /// at "the next call hears it".
    #[test]
    fn a_restore_leaves_the_change_for_the_agent() {
        let (mut engine, mut docs) = (Engine::default(), One::new());
        let k = Some("chat-2");
        let use_doc = |restore: bool| json!({ "document": "a", "restore": restore });
        ok(&keyed(
            &mut engine,
            &mut docs,
            1,
            k,
            "use_document",
            use_doc(false),
        ));
        engine.closed(&mut docs, 1);
        open(&mut engine, &mut docs, 9);
        rect(&mut engine, &mut docs, 9, "Badge");
        let restored = keyed(&mut engine, &mut docs, 2, k, "use_document", use_doc(true));
        assert!(ok(&restored).get("changed").is_none());
        let next = keyed(&mut engine, &mut docs, 2, k, "get_selection", json!({}));
        assert_eq!(
            ok(&next)["changed"]["added"][0]["name"],
            "Badge",
            "the next call hears it"
        );
        rect(&mut engine, &mut docs, 9, "Chip");
        let again = keyed(&mut engine, &mut docs, 2, k, "use_document", use_doc(false));
        assert_eq!(ok(&again)["changed"]["added"][0]["name"], "Chip");
    }

    /// Past [`CLIENTS`] keyed clients, the least recently used is forgotten.
    #[test]
    fn keyed_clients_are_bounded() {
        let (mut engine, mut docs) = (Engine::default(), One::new());
        for n in 0..=CLIENTS {
            let key = format!("k{n}");
            ok(&keyed(
                &mut engine,
                &mut docs,
                n as Conn,
                Some(&key),
                "use_document",
                json!({ "document": "a" }),
            ));
            engine.closed(&mut docs, n as Conn);
        }
        // The kept one first: asking for the forgotten one makes it anew,
        // forgetting the next oldest.
        ok(&keyed(
            &mut engine,
            &mut docs,
            99,
            Some("k1"),
            "get_selection",
            json!({}),
        ));
        assert!(
            keyed(
                &mut engine,
                &mut docs,
                99,
                Some("k0"),
                "get_selection",
                json!({})
            )
            .is_error,
            "the oldest was forgotten"
        );
    }

    /// Revisions start from the launch time, not from 1, so a `rev` another
    /// process handed out does not match one here; they stay exact in JavaScript.
    ///
    /// flip: `revision_seed` answering 1 → fails at "not a small counter".
    #[test]
    fn revisions_start_from_the_launch_time() {
        let rev = crate::session::next_revision();
        assert!(rev > 1_700_000_000_000_000, "not a small counter: {rev}");
        assert!(rev < 1 << 53);
    }

    /// A name matches an open document by file name, with or without the
    /// extension; an absolute path that is no file and an unknown name fail.
    #[test]
    fn a_document_is_found_by_name() {
        let open = [
            PathBuf::from("/work/dashboard.mastik"),
            PathBuf::from("/work/site.mastik"),
        ];
        assert_eq!(
            resolve("dashboard.mastik", &open, None),
            Ok(open[0].clone())
        );
        assert_eq!(resolve("Site", &open, None), Ok(open[1].clone()));
        assert!(resolve("nothing", &open, None).is_err());
        let missing = std::env::temp_dir().join("mastik-no-such-file.mastik");
        assert!(resolve(&missing.display().to_string(), &open, None).is_err());
    }
}
