//! One conversation with a harness: its run folder and sealing configuration,
//! its children, and the events their streams decode into.
//!
//! Work runs on std threads: one per turn, plus a reader per persistent
//! process. Every event is tagged with the turn it belongs to and dropped if a
//! newer turn has begun, so a killed child's late output never lands in the
//! next turn.

use crate::context;
use crate::find::{Launch, NotFound, launch, probe};
use crate::harness::{self, ClaudeFlags, Harness, Seal, TurnSpec};
use crate::process::{Tracker, wait_bounded};
use crate::stream::{LineKind, Parsed, StreamParser};
use std::io::{BufRead, BufReader, Read, Write};
use std::path::{Path, PathBuf};
use std::process::{Child, ChildStdin, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{Receiver, Sender};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

/// How long a child gets to exit, after its turn or a Stop, before it is killed.
const EXIT_GRACE: Duration = Duration::from_secs(10);

/// How long a check that reads a harness's resolved configuration back may
/// take: OpenCode's `debug config`, DeepSeek Harness's `--dump-config`.
const HELPER_TIMEOUT: Duration = Duration::from_secs(30);

/// How much of a child's stderr is kept for the diagnostics of a failed turn.
const STDERR_KEPT: usize = 64 * 1024;

/// Variables that would point a harness at another configuration, or reopen
/// what its seal closed, cleared from every child: OpenCode's config levers
/// merge with the person's file rather than replacing it, `COPILOT_ALLOW_ALL`
/// approves every tool, and `DSH_TOOLS_MODE` swaps in a code runner the
/// overlay never sees. Sign-in variables are not cleared.
const INHERITED_ENV_TO_CLEAR: &[&str] = &[
    "OPENCODE_CONFIG",
    "OPENCODE_CONFIG_CONTENT",
    "OPENCODE_CONFIG_DIR",
    "OPENCODE_PERMISSION",
    "COPILOT_ALLOW_ALL",
    "COPILOT_ASSISTED_APPROVAL",
    "COPILOT_CUSTOM_INSTRUCTIONS_DIRS",
    "DSH_TOOLS_MODE",
    "DSH_TELEMETRY_OTLP_URL",
];

/// What a session is started with.
#[derive(Clone, Debug)]
pub struct Config {
    pub harness: Harness,
    /// From [`Found::path`](crate::Found::path).
    pub program: PathBuf,
    /// Free text; `""` omits it.
    pub model: String,
    /// One of the harness's own levels; `""` omits it.
    pub effort: String,
    /// The bare MCP tool names this chat's access level grants: exactly these
    /// are allowed, everything else denied.
    pub allowed_tools: Vec<String>,
    /// In the system-prompt flag where there is one, else at the top of the
    /// conversation's first turn.
    pub system_prompt: String,
    /// The one stdio server every harness registers, as `mastik`.
    pub mcp_exe: PathBuf,
    pub mcp_args: Vec<String>,
    /// The machine's run folder, under which each session makes its own.
    pub run_root: PathBuf,
}

/// One turn of the person's.
#[derive(Clone, Debug, Default)]
pub struct Turn {
    pub text: String,
    /// PNG bytes, dropped where the harness does not take images.
    pub images: Vec<Vec<u8>>,
}

/// Why a session or a turn did not start. `Display` is one sentence.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Refusal {
    NotFound(NotFound),
    /// The seal could not be established, or a turn's check found a live tool.
    Unsealed {
        harness: Harness,
        why: String,
    },
    NoRunFolder(String),
    /// The command line is longer than the platform takes; `what` names the
    /// part that made it so.
    TooLong {
        total: usize,
        limit: usize,
        unit: &'static str,
        what: &'static str,
    },
    Spawn(String),
    ConfigNotWritten(String),
    /// A turn is already running.
    Busy,
}

impl std::fmt::Display for Refusal {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Refusal::NotFound(n) => write!(
                f,
                "Mastik could not find `{}` in the {} places it looked; set its path in Settings \
                 if it is installed elsewhere.",
                n.program,
                n.looked_in.len()
            ),
            Refusal::Unsealed { why, .. } => f.write_str(why),
            Refusal::NoRunFolder(e) => write!(
                f,
                "Mastik could not make a private folder to run the agent in ({e}), so it did not \
                 start it."
            ),
            Refusal::TooLong {
                total,
                limit,
                unit,
                what,
            } => write!(
                f,
                "This agent takes {what} on its command line, which would be {total} {unit} \
                 long where it allows about {limit}; start a new chat or shorten it."
            ),
            Refusal::Spawn(e) => write!(f, "Mastik could not start the agent: {e}."),
            Refusal::ConfigNotWritten(e) => write!(
                f,
                "Mastik could not set up the configuration that restricts the agent ({e}), so it \
                 did not start it."
            ),
            Refusal::Busy => f.write_str("A turn is already running; wait for it or stop it."),
        }
    }
}

/// What a session reports, drained by [`Session::poll`].
#[derive(Clone, Debug, PartialEq)]
pub enum Event {
    /// The harness's first output this turn.
    Alive,
    /// A text delta.
    Text(String),
    /// `name` is bare for Mastik's tools (`get_node`); anything else keeps its
    /// own name.
    ToolCall {
        id: String,
        name: String,
        input: serde_json::Value,
    },
    /// Paired with its call by `id`.
    ToolResult {
        id: String,
        text: String,
        is_error: bool,
        image: bool,
    },
    Done {
        seconds: f64,
        input_tokens: Option<u64>,
        /// The part of `input_tokens` read back from the prompt cache.
        cached_tokens: Option<u64>,
        output_tokens: Option<u64>,
        is_error: bool,
    },
    /// How full the conversation's context is now. Sent as calls finish where
    /// the stream says, and just before `Done` where only a log does.
    Context(crate::Reading),
    /// The harness exited before the turn was done; `signed_out` when it said
    /// it was not signed in.
    Exited {
        code: Option<i32>,
        signed_out: bool,
        diagnostics: String,
    },
    /// The turn the person stopped has ended.
    Stopped,
    /// The turn was refused before the harness ran it.
    Refused(Refusal),
    /// A line the harness printed that was not JSON, kept for "Show details".
    Diagnostic(String),
}

/// One conversation with a harness. Dropping it ends its processes and removes
/// its run folder and any global registration.
pub struct Session {
    inner: Arc<Inner>,
    events: Receiver<Event>,
}

/// Slow setup owed before the first turn, run on the turn's thread.
enum Prep {
    None,
    Antigravity,
    CursorApproval,
    /// OpenCode merges configuration from places Mastik cannot list, so the
    /// configuration it resolves is read back before the first turn.
    OpenCodeCheck,
}

/// What a session's threads share.
struct Inner {
    harness: Harness,
    program: PathBuf,
    claude: ClaudeFlags,
    run_folder: PathBuf,
    /// Where children run: a clean folder, or Cursor's workspace.
    cwd: PathBuf,
    /// The variables the isolation sets.
    env: Vec<(String, std::ffi::OsString)>,
    base: TurnSpec,
    mcp_exe: String,
    mcp_args: Vec<String>,
    run_root: PathBuf,
    prep: Prep,
    tracker: Arc<Tracker>,
    events: Mutex<Sender<Event>>,
    wake: Arc<dyn Fn() + Send + Sync>,
    in_flight: AtomicBool,
    state: Mutex<State>,
}

/// A session's grant: the MCP tools it allows and the system prompt that says
/// so.
#[derive(Clone, Debug)]
struct Access {
    allowed: Vec<String>,
    system: String,
}

#[derive(Default)]
struct State {
    turn: u64,
    started: Option<Instant>,
    alive_sent: bool,
    stopping: bool,
    /// The conversation id the next turn or respawn resumes.
    resume: Option<String>,
    /// Input, cached and output tokens reported so far, for a harness whose
    /// usage is a running total over the conversation; kept here because a
    /// respawned process carries on counting from the conversation's start.
    reported: [Option<u64>; 3],
    /// The system prompt has not reached the conversation yet.
    owes_system: bool,
    prepared: Option<Result<(), Refusal>>,
    registration: Option<crate::antigravity::Registration>,
    /// The grant given since the session started ([`Session::set_access`]),
    /// which every later turn's command line takes; `None` while it holds the
    /// one it started with.
    access: Option<Access>,
    /// The files and the persistent process still hold the grant before
    /// `access`: the next turn brings them to it before it runs.
    access_owed: bool,
    /// The persistent process, or this turn's child.
    child: Option<Arc<Mutex<Child>>>,
    stdin: Option<Arc<Mutex<ChildStdin>>>,
    /// Bumped per persistent process, so a dead one's reader leaves its
    /// successor alone.
    generation: u64,
    closed: bool,
}

impl Session {
    /// Set up a session: probe the program, make the run folder, write the
    /// configuration that seals the harness. Children start with the first
    /// turn. `wake` is called whenever events arrive.
    pub fn start(config: Config, wake: Arc<dyn Fn() + Send + Sync>) -> Result<Session, Refusal> {
        let h = config.harness;
        if !config.program.is_file() {
            return Err(Refusal::NotFound(NotFound {
                program: h.program(),
                looked_in: vec![config.program.clone()],
            }));
        }
        let probed = probe(h, &config.program);
        if probed.seal == Seal::Unknown {
            return Err(Refusal::Unsealed {
                harness: h,
                why: harness::seal_sentence(h, Seal::Unknown),
            });
        }
        let run_folder = crate::run::make(&config.run_root).map_err(Refusal::NoRunFolder)?;
        let setup = Setup::write(&config, &run_folder, probed.isolate_config);
        let setup = match setup {
            Ok(s) => s,
            Err(e) => {
                let _ = std::fs::remove_dir_all(&run_folder);
                return Err(Refusal::ConfigNotWritten(e));
            }
        };
        let (tx, rx) = std::sync::mpsc::channel();
        let inner = Arc::new(Inner {
            harness: h,
            program: config.program.clone(),
            claude: probed.claude,
            run_folder,
            cwd: setup.cwd,
            env: setup.env,
            base: setup.base,
            mcp_exe: config.mcp_exe.to_string_lossy().into_owned(),
            mcp_args: config.mcp_args.clone(),
            run_root: config.run_root.clone(),
            prep: setup.prep,
            tracker: Tracker::new(),
            events: Mutex::new(tx),
            wake,
            in_flight: AtomicBool::new(false),
            state: Mutex::new(State {
                owes_system: h.is_persistent() && h.system_in_first_turn(),
                ..State::default()
            }),
        });
        let session = Session { inner, events: rx };
        // A persistent session's argv is fixed: refuse it now rather than at
        // the first turn. Built first, so a refusal's drop removes the folder.
        if h.is_persistent() {
            let argv = session.inner.session_argv(&session.inner.lock(), None);
            session
                .inner
                .check_argv(&argv, &session.inner.base.system)?;
        }
        Ok(session)
    }

    /// Change the session's grant to `allowed_tools`, with `system_prompt` the
    /// system prompt that states it, keeping the conversation. Every later
    /// turn's command line takes it, and the next turn first rewrites the files
    /// that hold it and starts a persistent harness's process again, resuming
    /// its conversation by id. A turn running now finishes under the old grant.
    ///
    /// Answers `false`, changing nothing, when the conversation cannot be kept:
    /// a persistent harness whose process has not named it, so starting the
    /// process again would start a new one.
    pub fn set_access(&mut self, allowed_tools: Vec<String>, system_prompt: String) -> bool {
        let mut st = self.inner.lock();
        if self.inner.harness.is_persistent() && st.stdin.is_some() && st.resume.is_none() {
            return false;
        }
        st.access = Some(Access {
            allowed: allowed_tools,
            system: system_prompt,
        });
        st.access_owed = true;
        true
    }

    /// Send one turn. Refused while one is running, or when its command line
    /// is too long; everything slower is reported as events.
    pub fn send(&mut self, turn: Turn) -> Result<(), Refusal> {
        let inner = &self.inner;
        if inner.in_flight.load(Ordering::Acquire) {
            return Err(Refusal::Busy);
        }
        let images = match inner.harness.takes_images() {
            true => turn.images,
            false => Vec::new(),
        };
        let mut st = inner.lock();
        // Per-turn: the argv is known now, so its length is checked now.
        let per_turn = (!inner.harness.is_persistent()).then(|| {
            let spec = inner.turn_spec(&st, &turn.text, st.resume.clone(), st.turn + 1, &images);
            let args = inner.with_prefix(harness::turn_args(inner.harness, &spec));
            (spec, args)
        });
        if let Some((_, args)) = &per_turn {
            inner.check_argv(args, inner.system(&st))?;
        }
        st.turn += 1;
        st.started = Some(Instant::now());
        st.alive_sent = false;
        st.stopping = false;
        let id = st.turn;
        drop(st);
        inner.in_flight.store(true, Ordering::Release);
        let inner = Arc::clone(inner);
        std::thread::spawn(move || match per_turn {
            Some((spec, args)) => inner.run_turn(id, spec, args),
            None => inner.send_persistent(id, turn.text, images),
        });
        Ok(())
    }

    /// Stop the running turn: Claude Code's interrupt, a kill otherwise. A
    /// child gets ten seconds to end before it is killed; the next turn
    /// resumes the conversation by id. Never waits on the child's stdin, since
    /// it runs on the UI thread: a turn's line still being written means the
    /// harness has not read the turn, so the child is killed at once, and the
    /// interrupt is written on a thread of its own.
    pub fn stop(&mut self) {
        let inner = &self.inner;
        if !inner.in_flight.load(Ordering::Acquire) {
            return;
        }
        let (id, child, stdin) = {
            let mut st = inner.lock();
            st.stopping = true;
            (st.turn, st.child.clone(), st.stdin.clone())
        };
        // Still setting up: the turn's thread sees `stopping` before it spawns.
        let Some(child) = child else {
            return;
        };
        let kill = |inner: &Inner, child: &Mutex<Child>| {
            if let Ok(mut c) = child.lock() {
                inner.tracker.kill(&mut c);
            }
        };
        let interrupt = match (inner.harness.interrupt_line(), stdin) {
            (Some(line), Some(stdin)) if stdin.try_lock().is_ok() => Some((line, stdin)),
            _ => None,
        };
        let Some((line, stdin)) = interrupt else {
            return kill(inner, &child);
        };
        let (writer, grace) = (Arc::clone(inner), Arc::clone(inner));
        let written_child = Arc::clone(&child);
        std::thread::spawn(move || {
            let written = stdin
                .lock()
                .is_ok_and(|mut s| s.write_all(line.as_bytes()).and_then(|_| s.flush()).is_ok());
            if !written {
                kill(&writer, &written_child);
            }
        });
        // Apart from the write, which a harness not reading its stdin can hold.
        std::thread::spawn(move || {
            let until = Instant::now() + EXIT_GRACE;
            while Instant::now() < until {
                std::thread::sleep(Duration::from_millis(100));
                if !grace.in_flight.load(Ordering::Acquire) || grace.lock().turn != id {
                    return;
                }
            }
            kill(&grace, &child);
        });
    }

    /// Every event that has arrived, without blocking.
    pub fn poll(&mut self) -> Vec<Event> {
        self.events.try_iter().collect()
    }

    /// A turn is in flight.
    pub fn running(&self) -> bool {
        self.inner.in_flight.load(Ordering::Acquire)
    }

    /// Whether the next turn carries on this conversation: the process holding
    /// it is still running, or the harness has named it for a resume. A turn
    /// stopped before a per-turn harness named its conversation (Copilot names
    /// it only as the turn ends) leaves nothing to carry on.
    pub fn resumable(&self) -> bool {
        let st = self.inner.lock();
        st.resume.is_some() || st.stdin.is_some()
    }

    /// Does `pid` belong to this session's harness? See [`Session::owner`].
    pub fn owns_process(&self, pid: u32) -> bool {
        self.inner.tracker.owns(pid)
    }

    /// The same question, detached from the session so another thread can ask
    /// it: every turn's children are covered, and it answers `false` once the
    /// session is dropped. On Windows the answer is the harness's Job Object;
    /// elsewhere the parent chain is walked, which is best effort.
    pub fn owner(&self) -> Arc<dyn Fn(u32) -> bool + Send + Sync> {
        let tracker = Arc::clone(&self.inner.tracker);
        Arc::new(move |pid| tracker.owns(pid))
    }
}

// Dropping a session can run `agy mcp remove` and scan Cursor's folder, so a
// caller may drop it on a thread of its own; this keeps that possible.
const _: fn() = || {
    fn is_send<T: Send>() {}
    is_send::<Session>();
};

impl Drop for Session {
    fn drop(&mut self) {
        let inner = &self.inner;
        let registration = {
            let mut st = inner.lock();
            st.closed = true;
            st.stdin = None;
            st.child = None;
            st.registration.take()
        };
        inner.tracker.end();
        drop(registration);
        if inner.harness == Harness::Cursor {
            crate::cursor::forget(std::slice::from_ref(&inner.cwd));
        }
        // Files of a process just ended can stay locked for a moment on Windows;
        // what is left is swept at the next launch.
        for _ in 0..10 {
            if std::fs::remove_dir_all(&inner.run_folder).is_ok() || !inner.run_folder.exists() {
                break;
            }
            std::thread::sleep(Duration::from_millis(100));
        }
    }
}

/// What [`Session::start`] writes for one harness.
struct Setup {
    cwd: PathBuf,
    env: Vec<(String, std::ffi::OsString)>,
    base: TurnSpec,
    prep: Prep,
}

impl Setup {
    /// Write the harness's sealing configuration into the run folder. `Err`
    /// refuses the session: for OpenCode, Copilot, Cursor and DeepSeek the
    /// files are the seal or the isolation, and a missing one fails open.
    fn write(config: &Config, folder: &Path, isolate_config: bool) -> Result<Setup, String> {
        let h = config.harness;
        let exe = config.mcp_exe.to_string_lossy().into_owned();
        let args = &config.mcp_args;
        let allowed = &config.allowed_tools;
        let path_str = |p: &Path| p.to_string_lossy().into_owned();
        let mkdir =
            |p: &Path| std::fs::create_dir_all(p).map_err(|e| format!("{}: {e}", p.display()));
        let write = |p: &Path, text: String| {
            std::fs::write(p, text).map_err(|e| format!("{}: {e}", p.display()))
        };
        let work = folder.join("work");
        let mut s = Setup {
            cwd: work.clone(),
            env: Vec::new(),
            base: TurnSpec {
                system: config.system_prompt.clone(),
                model: config.model.clone(),
                effort: config.effort.clone(),
                isolate_config,
                allowed_tools: allowed.clone(),
                ..TurnSpec::default()
            },
            prep: Prep::None,
        };
        match h {
            Harness::Claude => {
                mkdir(&work)?;
                let cfg = folder.join("mcp.json");
                write(&cfg, harness::claude_mcp_config_json(&exe, args))?;
                s.base.mcp_config = Some(path_str(&cfg));
                if !config.system_prompt.trim().is_empty() {
                    let sys = folder.join("system-prompt.md");
                    write(&sys, config.system_prompt.clone())?;
                    s.base.system_file = Some(path_str(&sys));
                }
            }
            Harness::Codex => {
                mkdir(&work)?;
                s.base.mcp_overrides = harness::codex_mcp_overrides(&exe, args, allowed);
            }
            Harness::Antigravity => {
                mkdir(&work)?;
                s.prep = Prep::Antigravity;
            }
            Harness::OpenCode => {
                mkdir(&work)?;
                // `XDG_CONFIG_HOME` names the parent of `opencode/`. Never
                // `XDG_DATA_HOME`: its sign-in lives there.
                let root = folder.join("opencode-config");
                let dir = root.join("opencode");
                mkdir(&dir)?;
                write(
                    &dir.join("opencode.json"),
                    harness::opencode_config_json(&exe, args, allowed),
                )?;
                // `~/.opencode/opencode.json` is merged over everything else and
                // no option turns it off; this variable moves only that home,
                // not the data folder its sign-in lives in.
                let home = folder.join("opencode-home");
                mkdir(&home)?;
                s.env
                    .push(("XDG_CONFIG_HOME".into(), root.into_os_string()));
                s.env
                    .push(("OPENCODE_DISABLE_PROJECT_CONFIG".into(), "1".into()));
                s.env
                    .push(("OPENCODE_TEST_HOME".into(), home.into_os_string()));
                s.prep = Prep::OpenCodeCheck;
            }
            Harness::Copilot => {
                mkdir(&work)?;
                let home = folder.join("copilot-home");
                mkdir(&home)?;
                carry_copilot_login(&home);
                let cfg = home.join("mastik-mcp.json");
                write(&cfg, harness::copilot_mcp_config_json(&exe, args))?;
                s.base.mcp_config = Some(path_str(&cfg));
                s.env.push(("COPILOT_HOME".into(), home.into_os_string()));
            }
            Harness::Cursor => {
                let ws = crate::cursor::workspace_in(folder);
                let dot = ws.join(".cursor");
                mkdir(&dot)?;
                write(
                    &dot.join("cli.json"),
                    harness::cursor_permissions_json(allowed),
                )?;
                write(
                    &dot.join("mcp.json"),
                    harness::cursor_mcp_config_json(&exe, args),
                )?;
                s.cwd = ws;
                s.prep = Prep::CursorApproval;
            }
            Harness::DeepSeek => {
                mkdir(&work)?;
                let home = folder.join("dsh-home");
                mkdir(&home)?;
                let credentials = user_home_of("DSH_HOME", ".dsh")
                    .unwrap_or_else(|| home.clone())
                    .join(".credentials.yaml");
                let provider = match std::env::var_os("DEEPSEEK_API_KEY")
                    .is_some_and(|k| !k.to_string_lossy().trim().is_empty())
                {
                    true => harness::DshProvider::ApiKey,
                    false => harness::DshProvider::Account,
                };
                let overlay = home.join("mastik-overlay.yml");
                write(
                    &overlay,
                    harness::dsh_overlay_yaml(&harness::DshOverlay {
                        exe: &exe,
                        args,
                        credentials: &credentials.to_string_lossy(),
                        provider,
                        model: &config.model,
                        effort: &config.effort,
                    }),
                )?;
                s.base.mcp_config = Some(path_str(&overlay));
                s.env.push(("DSH_HOME".into(), home.into_os_string()));
                s.env
                    .push(("DSH_PERMISSION_MODE".into(), "read-only".into()));
                // Off entirely: nothing is built to send.
                s.env.push(("DSH_TELEMETRY_MODE".into(), "DISABLED".into()));
            }
        }
        Ok(s)
    }
}

/// A CLI's own home: its variable when set, else `<home>/<dot>`.
fn user_home_of(var: &str, dot: &str) -> Option<PathBuf> {
    match std::env::var_os(var).filter(|v| !v.to_string_lossy().trim().is_empty()) {
        Some(d) => Some(PathBuf::from(d)),
        None => Some(crate::find::home()?.join(dot)),
    }
}

/// Copy the person's Copilot `config.json` into the session's home: without a
/// credential store (headless Linux) the token lives there, and an empty home
/// would be signed out. It holds the login and none of their configuration.
fn carry_copilot_login(home: &Path) {
    if let Some(user) = user_home_of("COPILOT_HOME", ".copilot") {
        let from = user.join("config.json");
        if from.is_file() {
            let _ = std::fs::copy(&from, home.join("config.json"));
        }
    }
}

impl Inner {
    fn lock(&self) -> std::sync::MutexGuard<'_, State> {
        self.state.lock().unwrap_or_else(|e| e.into_inner())
    }

    /// The tools allowed now: the grant given since, else the session's own.
    fn allowed<'a>(&'a self, st: &'a State) -> &'a [String] {
        st.access
            .as_ref()
            .map_or(&self.base.allowed_tools, |a| &a.allowed)
    }

    /// The system prompt now: the grant's, else the session's own.
    fn system<'a>(&'a self, st: &'a State) -> &'a str {
        st.access.as_ref().map_or(&self.base.system, |a| &a.system)
    }

    /// The session's spec under the grant it holds now.
    fn granted_spec(&self, st: &State) -> TurnSpec {
        let mut spec = self.base.clone();
        if let Some(a) = &st.access {
            spec.allowed_tools.clone_from(&a.allowed);
            spec.system.clone_from(&a.system);
            if self.harness == Harness::Codex {
                spec.mcp_overrides =
                    harness::codex_mcp_overrides(&self.mcp_exe, &self.mcp_args, &a.allowed);
            }
        }
        spec
    }

    /// The spec of one turn. Its images are written into the working folder,
    /// where Copilot's path check and OpenCode's external-folder rule find them
    /// inside the workspace.
    fn turn_spec(
        &self,
        st: &State,
        text: &str,
        resume: Option<String>,
        turn: u64,
        images: &[Vec<u8>],
    ) -> TurnSpec {
        let images = images
            .iter()
            .enumerate()
            .filter_map(|(i, png)| {
                let p = self.cwd.join(format!("mastik-turn-{turn}-{i}.png"));
                std::fs::write(&p, png).ok()?;
                Some(p.to_string_lossy().into_owned())
            })
            .collect();
        TurnSpec {
            prompt: text.to_string(),
            resume,
            images,
            ..self.granted_spec(st)
        }
    }

    fn session_argv(&self, st: &State, resume: Option<String>) -> Vec<String> {
        let spec = TurnSpec {
            resume,
            ..self.granted_spec(st)
        };
        self.with_prefix(harness::session_args(self.harness, &spec, self.claude))
    }

    /// Bring what holds the grant outside the command line to the one
    /// [`Session::set_access`] gave: Claude Code's system-prompt file, Cursor's
    /// permissions, OpenCode's configuration (read back again before the turn),
    /// Antigravity's rules. A persistent process still running under the old
    /// grant is ended, so the turn starts it again resuming its conversation.
    /// A failure leaves it owed, and refuses the turn.
    fn take_access(&self) -> Result<(), Refusal> {
        let mut st = self.lock();
        if !st.access_owed {
            return Ok(());
        }
        let Some(access) = st.access.clone() else {
            st.access_owed = false;
            return Ok(());
        };
        let write = |p: &Path, text: String| {
            std::fs::write(p, text)
                .map_err(|e| Refusal::ConfigNotWritten(format!("{}: {e}", p.display())))
        };
        match self.harness {
            Harness::Claude => {
                if let Some(file) = &self.base.system_file {
                    write(Path::new(file), access.system.clone())?;
                }
            }
            Harness::Cursor => write(
                &self.cwd.join(".cursor").join("cli.json"),
                harness::cursor_permissions_json(&access.allowed),
            )?,
            Harness::OpenCode => {
                write(
                    &self
                        .run_folder
                        .join("opencode-config")
                        .join("opencode")
                        .join("opencode.json"),
                    harness::opencode_config_json(&self.mcp_exe, &self.mcp_args, &access.allowed),
                )?;
                st.prepared = None;
            }
            // Not yet registered: the registration grants what is allowed now.
            Harness::Antigravity => {
                if let Some(reg) = &st.registration {
                    reg.regrant(&access.allowed)
                        .map_err(Refusal::ConfigNotWritten)?;
                }
            }
            Harness::Codex | Harness::Copilot | Harness::DeepSeek => {}
        }
        // A new generation first, so the ended process's reader leaves this
        // turn alone.
        if let Some(child) = st.child.take() {
            st.stdin = None;
            st.generation += 1;
            if let Ok(mut c) = child.lock() {
                self.tracker.kill(&mut c);
            }
        }
        st.access_owed = false;
        Ok(())
    }

    fn with_prefix(&self, args: Vec<String>) -> Vec<String> {
        let mut a = launch(self.harness, &self.program).prefix;
        a.extend(args);
        a
    }

    /// Refuse a command line the platform will not take, before spawning: the
    /// OS's own error afterwards names neither cause. On Windows it is measured
    /// as std will quote it, against `cmd.exe`'s shorter limit behind a shim.
    /// `system` is the system prompt the session holds now, named as the cause
    /// when it is what made the line long.
    fn check_argv(&self, args: &[String], system: &str) -> Result<(), Refusal> {
        let program = launch(self.harness, &self.program).program;
        let ext = program
            .extension()
            .map(|e| e.to_string_lossy().to_ascii_lowercase());
        let batch = cfg!(windows) && matches!(ext.as_deref(), Some("cmd" | "bat"));
        let (limit, unit) = match (cfg!(windows), batch) {
            (true, true) => (8_000, "characters"),
            (true, false) => (30_000, "characters"),
            (false, _) => (120 * 1024, "bytes"),
        };
        let units = |a: &str| match cfg!(windows) {
            true => quoted_len(a, batch),
            false => a.len() + 1,
        };
        let total: usize =
            units(&program.to_string_lossy()) + args.iter().map(|a| units(a)).sum::<usize>();
        if total > limit {
            let longest = args.iter().max_by_key(|a| a.len());
            let system = system.trim();
            // The system prompt is the cause when it is most of the longest
            // argument (Claude Code's flag, where it has no file form).
            let what = match longest {
                Some(a)
                    if !system.is_empty() && a.contains(system) && system.len() * 2 > a.len() =>
                {
                    "the system prompt, which carries the instructions in Settings and the \
                     conversation so far"
                }
                _ => "your message",
            };
            return Err(Refusal::TooLong {
                total,
                limit,
                unit,
                what,
            });
        }
        if batch && args.iter().any(|a| a.contains('\n') || a.contains('\r')) {
            return Err(Refusal::Spawn(format!(
                "{} resolved to a batch shim ({}), which Windows cannot give a multi-line \
                 argument; set the program path in Settings to the real executable",
                self.harness.label(),
                program.display()
            )));
        }
        Ok(())
    }

    /// Send `ev` if `turn` is still the current one and not yet over: what a
    /// child prints after its turn's end belongs to no turn.
    fn emit(&self, turn: u64, ev: Event) {
        let st = self.lock();
        if st.turn != turn || st.closed || !self.in_flight.load(Ordering::Acquire) {
            return;
        }
        if let Ok(tx) = self.events.lock() {
            let _ = tx.send(ev);
        }
        drop(st);
        (self.wake)();
    }

    /// End `turn` with `ev` (or `Stopped`, when the person stopped it).
    fn finish(&self, turn: u64, ev: Event) {
        let st = self.lock();
        if st.turn != turn || st.closed || !self.in_flight.load(Ordering::Acquire) {
            return;
        }
        let ev = match (st.stopping, &ev) {
            (true, Event::Done { .. } | Event::Exited { .. }) => Event::Stopped,
            _ => ev,
        };
        if let Ok(tx) = self.events.lock() {
            let _ = tx.send(ev);
        }
        self.in_flight.store(false, Ordering::Release);
        drop(st);
        (self.wake)();
    }

    fn seconds(&self, turn_ms: Option<u64>) -> f64 {
        match turn_ms {
            Some(ms) => ms as f64 / 1000.0,
            None => self
                .lock()
                .started
                .map_or(0.0, |t| t.elapsed().as_secs_f64()),
        }
    }

    /// Run the slow setup until it holds. A success stands for the session, and
    /// so does a configuration that would leak, which no retry changes; any
    /// other failure (a helper past its deadline, a busy settings lock) is tried
    /// again on the next turn.
    fn prepare(&self) -> Result<(), Refusal> {
        if let Some(done) = self.lock().prepared.clone() {
            return done;
        }
        let (result, stands) = match self.prep {
            Prep::None => (Ok(()), true),
            Prep::Antigravity => (self.register_antigravity(), false),
            Prep::CursorApproval => (self.approve_cursor(), false),
            Prep::OpenCodeCheck => self.opencode_check(),
        };
        if result.is_ok() || stands {
            self.lock().prepared = Some(result.clone());
        }
        result
    }

    /// Register Mastik's server with Antigravity for this session. A failed
    /// attempt withdraws what it wrote, so another can follow. It grants what
    /// the session allows now.
    fn register_antigravity(&self) -> Result<(), Refusal> {
        let allowed = self.allowed(&self.lock()).to_vec();
        crate::antigravity::Registration::install(
            launch(self.harness, &self.program),
            &self.program,
            &self.run_root,
            &self.mcp_exe,
            &self.mcp_args,
            &allowed,
        )
        .map(|reg| self.lock().registration = Some(reg))
        .map_err(Refusal::ConfigNotWritten)
    }

    /// Have Cursor approve Mastik's project server.
    fn approve_cursor(&self) -> Result<(), Refusal> {
        let args = crate::cursor::APPROVE_ARGS.map(String::from).to_vec();
        crate::cursor::approve(self.command(&self.with_prefix(args))).map_err(|e| {
            Refusal::ConfigNotWritten(format!("Cursor did not approve Mastik's server: {e}"))
        })
    }

    /// A command for this harness: launch prefix, cleared and set environment,
    /// the working folder, the child marker.
    fn command(&self, args: &[String]) -> std::process::Command {
        let l: Launch = launch(self.harness, &self.program);
        let mut c = std::process::Command::new(&l.program);
        c.args(args);
        for k in INHERITED_ENV_TO_CLEAR {
            c.env_remove(k);
        }
        c.envs(l.env.iter().map(|(k, v)| (k, v)));
        c.envs(self.env.iter().map(|(k, v)| (k, v)));
        c.env(crate::AGENT_CHILD_ENV, "1").current_dir(&self.cwd);
        c
    }

    /// Spawn into the session's tracker, all three pipes as asked.
    fn spawn(&self, args: &[String], stdin: bool) -> Result<Child, Refusal> {
        let mut c = self.command(args);
        c.stdin(match stdin {
            true => Stdio::piped(),
            false => Stdio::null(),
        })
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
        self.tracker
            .spawn(&mut c)
            .map_err(|e| Refusal::Spawn(format!("{}: {e}", self.program.display())))
    }

    /// DeepSeek's seal, re-earned before every turn against the composed
    /// profile: an update can mount a tool plugin the overlay never named.
    fn dsh_check(&self) -> Result<(), Refusal> {
        let Some(overlay) = self.base.mcp_config.as_deref() else {
            return Ok(());
        };
        let refuse = |why: String| Refusal::Unsealed {
            harness: Harness::DeepSeek,
            why,
        };
        let args = self.with_prefix(harness::dsh_dump_args(overlay));
        let mut child = self.spawn(&args, false)?;
        let stdout = child.stdout.take();
        let stderr = child.stderr.take();
        let out = collect(stdout, usize::MAX);
        let err = collect(stderr, STDERR_KEPT);
        let ok =
            wait_bounded(&self.tracker, &Mutex::new(child), HELPER_TIMEOUT).is_some_and(|c| c == 0);
        let dump = out.join().unwrap_or_default();
        if !ok || !harness::dsh_dump_is_headless(&dump) {
            let first = err.join().unwrap_or_default();
            let first = first
                .lines()
                .map(str::trim)
                .find(|l| !l.is_empty())
                .unwrap_or("no output");
            return Err(refuse(format!(
                "Mastik could not check that DeepSeek Harness is sealed ({first}), so it did not \
                 run the turn."
            )));
        }
        let rows = harness::dsh_unsealed_rows(&dump);
        match rows.is_empty() {
            true => Ok(()),
            false => Err(refuse(format!(
                "DeepSeek Harness would still have tools Mastik did not grant ({}), so the turn \
                 was refused.",
                rows.join(", ")
            ))),
        }
    }

    /// OpenCode's seal, read off the configuration it resolves with the
    /// session's environment: only Mastik's server, and the agent's tools off.
    /// The flag is whether the verdict stands: a configuration that was read
    /// does, one that could not be read is worth reading again.
    fn opencode_check(&self) -> (Result<(), Refusal>, bool) {
        let refuse = |why: String| Refusal::Unsealed {
            harness: Harness::OpenCode,
            why,
        };
        let mut c = self.command(&self.with_prefix(vec!["debug".into(), "config".into()]));
        c.stdin(Stdio::null());
        let out = crate::process::run_bounded(&mut c, HELPER_TIMEOUT);
        let text = match out {
            Ok(Some(h)) if h.status.success() => String::from_utf8_lossy(&h.stdout).into_owned(),
            _ => String::new(),
        };
        match harness::opencode_config_leaks(&text) {
            None => (
                Err(refuse(
                    "Mastik could not read the configuration OpenCode resolves, so it could not \
                     check that it is sealed, and did not start it."
                        .into(),
                )),
                false,
            ),
            Some(leaks) if leaks.is_empty() => (Ok(()), true),
            Some(leaks) => (
                Err(refuse(format!(
                    "OpenCode's own configuration would still give this session more than \
                     Mastik's tools ({}), so Mastik did not start it.",
                    leaks.join(", ")
                ))),
                true,
            ),
        }
    }

    /// One turn on a per-turn harness: setup, spawn, read to the end.
    fn run_turn(self: Arc<Self>, turn: u64, spec: TurnSpec, args: Vec<String>) {
        // The last turn's child is reaped; *Stop* during setup must not reach it.
        self.lock().child = None;
        // The grant first: OpenCode's configuration rewritten is read back.
        let refused = self
            .take_access()
            .and_then(|_| self.prepare())
            .and_then(|_| match self.harness {
                Harness::DeepSeek => self.dsh_check(),
                _ => Ok(()),
            });
        if let Err(r) = refused {
            return self.finish(turn, Event::Refused(r));
        }
        let stdin_text = harness::turn_stdin(self.harness, &spec);
        let child = {
            let mut st = self.lock();
            if st.stopping {
                drop(st);
                return self.finish(turn, Event::Stopped);
            }
            match self.spawn(&args, stdin_text.is_some()) {
                Ok(c) => {
                    let c = Arc::new(Mutex::new(c));
                    st.child = Some(Arc::clone(&c));
                    c
                }
                Err(r) => {
                    drop(st);
                    return self.finish(turn, Event::Refused(r));
                }
            }
        };
        let (stdout, stderr, stdin) = {
            let mut c = child.lock().unwrap_or_else(|e| e.into_inner());
            (c.stdout.take(), c.stderr.take(), c.stdin.take())
        };
        // Written, then closed: the CLI waits for end of input.
        if let (Some(text), Some(mut si)) = (stdin_text, stdin) {
            std::thread::spawn(move || {
                let _ = si.write_all(text.as_bytes());
            });
        }
        let stderr = collect(stderr, STDERR_KEPT);
        let mut reader = Reader::new(self.harness);
        let mut ended = false;
        if let Some(out) = stdout {
            for line in lossy_lines(out) {
                if self.read_line(turn, &mut reader, &line) && !ended {
                    ended = true;
                    // The turn is over; the child gets its grace to exit.
                    let (tracker, child) = (Arc::clone(&self.tracker), Arc::clone(&child));
                    std::thread::spawn(move || {
                        wait_bounded(&tracker, &child, EXIT_GRACE);
                    });
                }
            }
        }
        let code = wait_bounded(&self.tracker, &child, EXIT_GRACE);
        let stderr = stderr.join().unwrap_or_default();
        if !ended {
            let ev = self.exited(code, &reader.plain, &stderr);
            self.finish(turn, ev);
        }
    }

    /// Handle one stdout line of `turn`; `true` when it ended the turn.
    fn read_line(&self, turn: u64, reader: &mut Reader, line: &str) -> bool {
        let first = {
            let mut st = self.lock();
            let first = st.turn == turn && !st.alive_sent;
            st.alive_sent |= first;
            first
        };
        if first {
            self.emit(turn, Event::Alive);
        }
        let parsed = reader.parser.push(line);
        if reader.parser.last_line() == LineKind::Plain {
            let l = line.trim().to_string();
            reader.plain.push(l.clone());
            self.emit(turn, Event::Diagnostic(l));
        }
        let mut done = false;
        for p in parsed {
            match reader.translate(p) {
                Translated::Session(id) => self.lock().resume = Some(id),
                Translated::Event(ev) => self.emit(turn, ev),
                Translated::Done {
                    is_error,
                    duration_ms,
                    input_tokens,
                    cached_tokens,
                    output_tokens,
                } => {
                    let seconds = self.seconds(duration_ms);
                    let [input_tokens, cached_tokens, output_tokens] =
                        match self.harness.usage_is_running_total() {
                            true => {
                                let mut st = self.lock();
                                let (turn, reported) = this_turn(
                                    st.reported,
                                    [input_tokens, cached_tokens, output_tokens],
                                );
                                st.reported = reported;
                                turn
                            }
                            false => [input_tokens, cached_tokens, output_tokens],
                        };
                    // The log is written before the turn's end is printed.
                    let id = self.lock().resume.clone();
                    if let Some(r) =
                        id.and_then(|id| context::from_log(self.harness, &id, &self.env))
                    {
                        self.emit(turn, Event::Context(r));
                    }
                    self.finish(
                        turn,
                        Event::Done {
                            seconds,
                            input_tokens,
                            cached_tokens,
                            output_tokens,
                            is_error,
                        },
                    );
                    reader.reset();
                    done = true;
                }
            }
        }
        done
    }

    /// What a turn that ended without its terminal event says.
    fn exited(&self, code: Option<i32>, plain: &[String], stderr: &str) -> Event {
        let own: String = match self.harness.stderr_diagnostic_prefix() {
            Some(p) => stderr
                .lines()
                .filter(|l| l.trim_start().starts_with(p))
                .collect::<Vec<_>>()
                .join("\n"),
            None => stderr.to_string(),
        };
        let plain = plain.join("\n");
        let diagnostics = [own.trim(), plain.trim()]
            .into_iter()
            .filter(|s| !s.is_empty())
            .collect::<Vec<_>>()
            .join("\n");
        Event::Exited {
            code,
            signed_out: says_signed_out(&diagnostics),
            diagnostics,
        }
    }

    /// One turn on a persistent harness: start the process if there is none,
    /// then write the turn to it. Its reader thread does the rest.
    fn send_persistent(self: Arc<Self>, turn: u64, text: String, images: Vec<Vec<u8>>) {
        if let Err(r) = self.take_access().and_then(|_| self.prepare()) {
            return self.finish(turn, Event::Refused(r));
        }
        let (stdin, text) = {
            let mut st = self.lock();
            if st.stopping {
                drop(st);
                return self.finish(turn, Event::Stopped);
            }
            if st.stdin.is_none() {
                let args = self.session_argv(&st, st.resume.clone());
                if let Err(r) = self.check_argv(&args, self.system(&st)) {
                    drop(st);
                    return self.finish(turn, Event::Refused(r));
                }
                let mut child = match self.spawn(&args, true) {
                    Ok(c) => c,
                    Err(r) => {
                        drop(st);
                        return self.finish(turn, Event::Refused(r));
                    }
                };
                st.generation += 1;
                let (out, err, si) = (child.stdout.take(), child.stderr.take(), child.stdin.take());
                let child = Arc::new(Mutex::new(child));
                st.child = Some(Arc::clone(&child));
                st.stdin = si.map(|s| Arc::new(Mutex::new(s)));
                let (me, generation) = (Arc::clone(&self), st.generation);
                std::thread::spawn(move || me.read_persistent(generation, child, out, err));
            }
            // The system prompt rides the conversation's first turn, once.
            let system = self.system(&st).to_owned();
            let text = match st.owes_system && !system.trim().is_empty() {
                true => {
                    st.owes_system = false;
                    harness::prefixed_prompt(&system, &text)
                }
                false => text,
            };
            (st.stdin.clone(), text)
        };
        let line = self.harness.turn_line(&text, &images);
        let written = stdin.is_some_and(|s| {
            s.lock()
                .is_ok_and(|mut s| s.write_all(line.as_bytes()).and_then(|_| s.flush()).is_ok())
        });
        if !written {
            // The reader reports the exit; this only covers a pipe already gone.
            let child = self.lock().child.clone();
            if let Some(c) = child
                && let Ok(mut c) = c.lock()
            {
                self.tracker.kill(&mut c);
            }
        }
    }

    /// A persistent process's reader, for every turn the process serves.
    fn read_persistent(
        self: Arc<Self>,
        generation: u64,
        child: Arc<Mutex<Child>>,
        out: Option<std::process::ChildStdout>,
        err: Option<std::process::ChildStderr>,
    ) {
        let stderr = collect(err, STDERR_KEPT);
        let mut reader = Reader::new(self.harness);
        // A process a new grant ended (`take_access`) is no longer the
        // session's: nothing it says reaches the turn its successor serves.
        let current = |me: &Self| me.lock().generation == generation;
        if let Some(out) = out {
            for line in lossy_lines(out) {
                let turn = self.lock().turn;
                if !current(&self) {
                    continue;
                }
                if self.in_flight.load(Ordering::Acquire) {
                    self.read_line(turn, &mut reader, &line);
                } else if let Some(Parsed::Session(id)) = reader
                    .parser
                    .push(&line)
                    .into_iter()
                    .find(|p| matches!(p, Parsed::Session(_)))
                {
                    self.lock().resume = Some(id);
                }
            }
        }
        let code = wait_bounded(&self.tracker, &child, EXIT_GRACE);
        let (turn, current) = {
            let mut st = self.lock();
            let current = st.generation == generation;
            if current {
                st.child = None;
                st.stdin = None;
                // A respawn opens a new conversation unless it can resume one.
                st.owes_system |= self.harness.system_in_first_turn() && st.resume.is_none();
            }
            (st.turn, current)
        };
        if current && self.in_flight.load(Ordering::Acquire) {
            let ev = self.exited(code, &reader.plain, &stderr.join().unwrap_or_default());
            self.finish(turn, ev);
        }
    }
}

/// A stream's parser and the per-turn naming state over it.
struct Reader {
    parser: StreamParser,
    plain: Vec<String>,
    anon: u32,
    last_anon: Option<String>,
}

enum Translated {
    Event(Event),
    Session(String),
    Done {
        is_error: bool,
        duration_ms: Option<u64>,
        input_tokens: Option<u64>,
        cached_tokens: Option<u64>,
        output_tokens: Option<u64>,
    },
}

impl Reader {
    fn new(h: Harness) -> Reader {
        Reader {
            parser: StreamParser::new(h),
            plain: Vec::new(),
            anon: 0,
            last_anon: None,
        }
    }

    fn reset(&mut self) {
        self.plain.clear();
        self.last_anon = None;
    }

    /// A parsed event in the public vocabulary: Mastik's tools by their bare
    /// name, and a call with no id of its own given one its result can find.
    fn translate(&mut self, p: Parsed) -> Translated {
        match p {
            Parsed::Text(t) => Translated::Event(Event::Text(t)),
            Parsed::Context(r) => Translated::Event(Event::Context(r)),
            Parsed::Session(id) => Translated::Session(id),
            Parsed::ToolUse { id, name, input } => {
                let id = match id.filter(|i| !i.is_empty()) {
                    Some(i) => i,
                    None => {
                        self.anon += 1;
                        let i = format!("call-{}", self.anon);
                        self.last_anon = Some(i.clone());
                        i
                    }
                };
                Translated::Event(Event::ToolCall {
                    id,
                    name: bare_name(&name).to_string(),
                    input,
                })
            }
            Parsed::ToolResult {
                id,
                text,
                is_error,
                image,
            } => Translated::Event(Event::ToolResult {
                id: id
                    .filter(|i| !i.is_empty())
                    .or_else(|| self.last_anon.clone())
                    .unwrap_or_default(),
                text,
                is_error,
                image,
            }),
            Parsed::Done {
                is_error,
                duration_ms,
                input_tokens,
                cached_tokens,
                output_tokens,
            } => Translated::Done {
                is_error,
                duration_ms,
                input_tokens,
                cached_tokens,
                output_tokens,
            },
        }
    }
}

/// One turn's share of running totals `now`, given those reported before, and
/// the totals to keep. A count the turn did not report keeps its earlier total;
/// one below it is a count started over, so it is the turn's whole.
fn this_turn(
    before: [Option<u64>; 3],
    now: [Option<u64>; 3],
) -> ([Option<u64>; 3], [Option<u64>; 3]) {
    let mut turn = [None; 3];
    let mut kept = before;
    for i in 0..3 {
        if let Some(n) = now[i] {
            turn[i] = Some(match before[i] {
                Some(b) if b <= n => n - b,
                _ => n,
            });
            kept[i] = Some(n);
        }
    }
    (turn, kept)
}

/// `mcp__mastik__get_node` → `get_node`; any other name unchanged.
fn bare_name(qualified: &str) -> &str {
    qualified
        .strip_prefix(&format!("mcp__{}__", crate::MCP_SERVER))
        .filter(|b| !b.is_empty())
        .unwrap_or(qualified)
}

/// One argument's length in UTF-16 units on a Windows command line, with its
/// separator: quoted when it holds a blank (always, for a batch file), each `"`
/// escaped and the backslashes before it doubled, and for a batch file each
/// `%` spelled as the eight-character escape std writes for it.
fn quoted_len(a: &str, batch: bool) -> usize {
    let mut n = 1;
    let mut backslashes = 0;
    for c in a.chars() {
        match c {
            '\\' => {
                backslashes += 1;
                n += 1;
            }
            '"' => {
                n += backslashes + 2;
                backslashes = 0;
            }
            '%' if batch => {
                n += 8;
                backslashes = 0;
            }
            c => {
                n += c.len_utf16();
                backslashes = 0;
            }
        }
    }
    if batch || a.is_empty() || a.contains([' ', '\t']) {
        n += backslashes + 2;
    }
    n
}

/// A pipe's lines until it closes, each decoded lossily: a harness may print a
/// diagnostic in the console's code page, and one bad byte must not end the
/// turn's stream.
fn lossy_lines(pipe: impl Read) -> impl Iterator<Item = String> {
    let mut r = BufReader::new(pipe);
    std::iter::from_fn(move || {
        let mut buf = Vec::new();
        match r.read_until(b'\n', &mut buf) {
            Ok(0) | Err(_) => None,
            Ok(_) => {
                while buf.last().is_some_and(|b| *b == b'\n' || *b == b'\r') {
                    buf.pop();
                }
                Some(String::from_utf8_lossy(&buf).into_owned())
            }
        }
    })
}

/// Read a pipe to its end on a thread of its own, keeping at most `cap` bytes:
/// a child blocked on a full pipe never exits.
fn collect<R: Read + Send + 'static>(
    pipe: Option<R>,
    cap: usize,
) -> std::thread::JoinHandle<String> {
    std::thread::spawn(move || {
        let mut kept = Vec::new();
        if let Some(mut p) = pipe {
            let mut buf = [0u8; 8192];
            while let Ok(n) = p.read(&mut buf) {
                if n == 0 {
                    break;
                }
                let room = cap.saturating_sub(kept.len()).min(n);
                kept.extend_from_slice(&buf[..room]);
            }
        }
        String::from_utf8_lossy(&kept).into_owned()
    })
}

/// Does a harness's dying output say it is not signed in?
fn says_signed_out(diagnostics: &str) -> bool {
    let d = diagnostics.to_lowercase();
    [
        "not logged in",
        "please log in",
        "please login",
        "run /login",
        "login required",
        "log in to",
        "not signed in",
        "sign in to",
        "please sign in",
        "not authenticated",
        "unauthenticated",
        "authentication required",
        "failed to authenticate",
        "invalid api key",
        "oauth token has expired",
        "no credentials",
    ]
    .iter()
    .any(|p| d.contains(p))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Mastik's tools are named bare; anything else keeps its own name.
    #[test]
    fn names_are_bare_for_mastiks_tools_only() {
        assert_eq!(bare_name("mcp__mastik__get_node"), "get_node");
        assert_eq!(bare_name("mcp__other__get_node"), "mcp__other__get_node");
        assert_eq!(bare_name("shell"), "shell");
        assert_eq!(bare_name("mcp__mastik__"), "mcp__mastik__");
    }

    /// A call with no id is given one, and its result finds it.
    #[test]
    fn an_anonymous_call_and_its_result_pair() {
        let mut r = Reader::new(Harness::Antigravity);
        let Translated::Event(Event::ToolCall { id, .. }) = r.translate(Parsed::ToolUse {
            id: Some(String::new()),
            name: "x".into(),
            input: serde_json::Value::Null,
        }) else {
            panic!("a call");
        };
        let Translated::Event(Event::ToolResult { id: rid, .. }) =
            r.translate(Parsed::ToolResult {
                id: None,
                text: String::new(),
                is_error: false,
                image: false,
            })
        else {
            panic!("a result");
        };
        assert_eq!(id, rid);
    }

    /// A sign-in failure is recognised; an ordinary crash is not.
    #[test]
    fn signed_out_is_read_off_the_harness_words() {
        assert!(says_signed_out("Invalid API key · Please run /login"));
        assert!(says_signed_out("Error: Not logged in. Run `codex login`."));
        assert!(!says_signed_out("panicked at src/main.rs:3"));
    }

    /// Each refusal is one sentence.
    #[test]
    fn refusals_are_one_sentence() {
        let all = [
            Refusal::NotFound(NotFound {
                program: "claude",
                looked_in: vec![PathBuf::from("a")],
            }),
            Refusal::NoRunFolder("denied".into()),
            Refusal::TooLong {
                total: 40_000,
                limit: 30_000,
                unit: "characters",
                what: "your message",
            },
            Refusal::Spawn("x".into()),
            Refusal::ConfigNotWritten("x".into()),
            Refusal::Busy,
        ];
        for r in all {
            let s = r.to_string();
            assert!(s.ends_with('.'), "{s}");
            assert_eq!(s.matches(". ").count(), 0, "{s}");
        }
    }

    fn fake_program(dir: &Path) -> PathBuf {
        let p = dir.join(if cfg!(windows) { "fake.exe" } else { "fake" });
        std::fs::write(&p, "").unwrap();
        p
    }

    fn config(h: Harness, root: &Path, program: PathBuf) -> Config {
        Config {
            harness: h,
            program,
            model: String::new(),
            effort: String::new(),
            allowed_tools: vec!["get_node".into()],
            system_prompt: "sys".into(),
            mcp_exe: PathBuf::from("mastik"),
            mcp_args: vec!["mcp-proxy".into()],
            run_root: root.to_path_buf(),
        }
    }

    /// A missing program is not found, and an unprobeable one is refused as
    /// unsealed, before any folder is made.
    #[test]
    fn start_refuses_before_writing_anything() {
        let root =
            std::env::temp_dir().join(format!("mastik-ai-start-{}", crate::run::random_tag()));
        std::fs::create_dir_all(&root).unwrap();
        let wake: Arc<dyn Fn() + Send + Sync> = Arc::new(|| {});
        let missing = config(Harness::Codex, &root, root.join("nope"));
        assert!(matches!(
            Session::start(missing, wake.clone()),
            Err(Refusal::NotFound(_))
        ));
        // An empty file cannot run `--help`: the seal is unknown.
        let program = fake_program(&root);
        let r = Session::start(config(Harness::Copilot, &root, program), wake);
        assert!(matches!(r, Err(Refusal::Unsealed { .. })), "{:?}", r.err());
        let folders = std::fs::read_dir(&root)
            .unwrap()
            .flatten()
            .filter(|e| e.path().is_dir())
            .count();
        assert_eq!(folders, 0);
        let _ = std::fs::remove_dir_all(&root);
    }

    /// Every harness's setup writes its sealing files into the run folder, and
    /// the variables it sets are not among those cleared.
    #[test]
    fn every_setup_writes_its_seal_into_the_run_folder() {
        let root =
            std::env::temp_dir().join(format!("mastik-ai-setup-{}", crate::run::random_tag()));
        for h in Harness::ALL {
            let folder = crate::run::make(&root).unwrap();
            let program = fake_program(&root);
            let s = Setup::write(&config(h, &root, program), &folder, false).unwrap();
            assert!(s.cwd.starts_with(&folder), "{h:?}");
            assert!(s.cwd.is_dir(), "{h:?}");
            for (k, _) in &s.env {
                assert!(!INHERITED_ENV_TO_CLEAR.contains(&k.as_str()), "{h:?} {k}");
            }
            if let Some(cfg) = &s.base.mcp_config {
                assert!(Path::new(cfg).is_file(), "{h:?}");
            }
            match h {
                Harness::Cursor => {
                    assert!(s.cwd.join(".cursor").join("cli.json").is_file());
                }
                Harness::OpenCode => {
                    assert!(
                        folder
                            .join("opencode-config/opencode/opencode.json")
                            .is_file()
                    );
                }
                Harness::Codex => assert!(!s.base.mcp_overrides.is_empty()),
                _ => {}
            }
        }
        let _ = std::fs::remove_dir_all(&root);
    }

    /// Not a test: the agent the session tests start, by `MASTIK_AI_FAKE`'s mode,
    /// speaking each dialect from its captured shape.
    #[test]
    #[ignore = "started by the session tests as a fake harness"]
    fn fake_harness() {
        let Ok(mode) = std::env::var("MASTIK_AI_FAKE") else {
            return;
        };
        assert_eq!(std::env::var(crate::AGENT_CHILD_ENV).as_deref(), Ok("1"));
        let args: Vec<String> = std::env::args().collect();
        let say = |s: &str| {
            println!("{s}");
            let _ = std::io::stdout().flush();
        };
        match mode.as_str() {
            "codex" => {
                let mut prompt = String::new();
                let _ = std::io::stdin().read_to_string(&mut prompt);
                let resumed = args.iter().any(|a| a == "resume");
                say(r#"{"type":"thread.started","thread_id":"t-1"}"#);
                say(
                    r#"{"type":"item.completed","item":{"id":"i1","type":"mcp_tool_call","server":"mastik","tool":"get_node","arguments":{"id":"1:2"},"result":{"content":[{"type":"text","text":"a frame"}]},"status":"completed"}}"#,
                );
                let text = format!(
                    "{} {}",
                    if resumed { "again" } else { "first" },
                    prompt.trim()
                );
                say(
                    &serde_json::json!({"type":"item.completed","item":{"id":"i2","type":"agent_message","text":text}})
                        .to_string(),
                );
                say(r#"{"type":"turn.completed","usage":{"input_tokens":5,"output_tokens":2}}"#);
            }
            "code-page" => {
                let mut out = std::io::stdout();
                let _ = out.write_all(b"Fehler: Datei \xfcberall\r\n");
                say(
                    r#"{"type":"item.completed","item":{"id":"i2","type":"agent_message","text":"done"}}"#,
                );
                say(r#"{"type":"turn.completed","usage":{"input_tokens":5,"output_tokens":2}}"#);
            }
            "hang" => {
                say(r#"{"type":"thread.started","thread_id":"t-1"}"#);
                std::thread::sleep(Duration::from_secs(60));
            }
            // Antigravity's NDJSON: a turn whose text says `unnamed` never
            // names its conversation, and one that says `slow` never ends.
            "agy" => {
                let resumed = args.iter().any(|a| a == "--conversation");
                for line in std::io::stdin().lock().lines() {
                    let Ok(line) = line else { break };
                    let v: serde_json::Value = serde_json::from_str(&line).unwrap();
                    if v["event"] != "user" {
                        continue;
                    }
                    let text = v.pointer("/message/content").and_then(|t| t.as_str());
                    let text = text.unwrap_or_default();
                    if !text.contains("unnamed") {
                        say(r#"{"event":"init","conversation_id":"c-1"}"#);
                    }
                    let reply = format!("{} {text}", if resumed { "resumed" } else { "new" });
                    say(&serde_json::json!({"event":"step_update","step_update":{"step_type":"agent_response","text_delta":reply}}).to_string());
                    if !text.contains("slow") {
                        say(r#"{"event":"result","result":{"status":"SUCCESS"}}"#);
                    }
                }
            }
            // Antigravity's usage as it really reports it: every turn so far.
            "agy-totals" => {
                let mut n = 0;
                for line in std::io::stdin().lock().lines() {
                    let Ok(line) = line else { break };
                    if !line.contains(r#""event":"user""#) {
                        continue;
                    }
                    n += 1;
                    say(r#"{"event":"init","conversation_id":"c-1"}"#);
                    say(&serde_json::json!({"event":"result","result":{"status":"SUCCESS","num_turns":n,"usage":{"input_tokens":100 * n,"cache_read_tokens":10 * n,"output_tokens":5 * n}}}).to_string());
                }
            }
            // A turn that names its conversation only at its end, as Copilot's.
            "hang-unnamed" => {
                say(
                    r#"{"type":"item.completed","item":{"id":"i2","type":"agent_message","text":"Let me look."}}"#,
                );
                std::thread::sleep(Duration::from_secs(60));
            }
            // Copilot: one model call logged under its home, and the session
            // named only in the `result` that ends the turn.
            "copilot-log" => {
                let _ = std::io::stdin().read_to_string(&mut String::new());
                let home = PathBuf::from(std::env::var_os("COPILOT_HOME").unwrap());
                let dir = home.join("session-state").join("s-9");
                std::fs::create_dir_all(&dir).unwrap();
                std::fs::write(
                    dir.join("events.jsonl"),
                    r#"{"type":"session.usage_record","data":{"usage":{"inputTokens":900,"maxPromptTokens":1000}}}"#,
                )
                .unwrap();
                say(r#"{"type":"assistant.message","data":{"messageId":"m1","content":"hi"}}"#);
                say(r#"{"type":"result","sessionId":"s-9","exitCode":0}"#);
            }
            // A persistent harness still starting: it reads nothing on stdin.
            "deaf" => std::thread::sleep(Duration::from_secs(60)),
            "exit" => {
                say("Error: Not logged in. Run `codex login`.");
                std::process::exit(3);
            }
            // OpenCode's `debug config`, counted in the working folder: the
            // first read fails (`flaky`) or leaks (`leaky`), later ones are
            // sealed. A turn reads its prompt and ends without a word.
            "opencode-flaky" | "opencode-leaky" if args.iter().any(|a| a == "debug") => {
                let runs = Path::new("config-reads");
                let n: u32 = std::fs::read_to_string(runs)
                    .ok()
                    .and_then(|s| s.parse().ok())
                    .unwrap_or(0);
                std::fs::write(runs, (n + 1).to_string()).unwrap();
                match (mode.as_str(), n) {
                    ("opencode-flaky", 0) => std::process::exit(1),
                    ("opencode-leaky", 0) => {
                        say(r#"{"agent":{"mastik":{"permission":{"*":"deny","bash":"allow"}}}}"#)
                    }
                    _ => say(&harness::opencode_config_json(
                        "mastik",
                        &[],
                        &["get_node".into()],
                    )),
                }
            }
            "opencode-flaky" | "opencode-leaky" => {
                let _ = std::io::stdin().read_to_string(&mut String::new());
            }
            // Claude Code's stream; `claude-argv` answers with its command line
            // after the text.
            "claude" | "claude-argv" => {
                for line in std::io::stdin().lock().lines() {
                    let Ok(line) = line else { break };
                    let v: serde_json::Value = serde_json::from_str(&line).unwrap();
                    match v["type"].as_str() {
                        Some("user") => {
                            say(r#"{"type":"system","subtype":"init","session_id":"s-1"}"#);
                            let text = v
                                .pointer("/message/content/0/text")
                                .and_then(|t| t.as_str())
                                .unwrap_or("");
                            let argv = format!("{text} | {}", args.join(" "));
                            let text = match mode.as_str() {
                                "claude-argv" => argv.as_str(),
                                _ => text,
                            };
                            say(&serde_json::json!({"type":"stream_event","event":{"type":"content_block_delta","delta":{"type":"text_delta","text":text}}}).to_string());
                            if !text.contains("slow") {
                                say(r#"{"type":"result","is_error":false,"duration_ms":7}"#);
                            }
                        }
                        Some("control_request") => {
                            say(r#"{"type":"result","is_error":true,"result":"interrupted"}"#);
                        }
                        _ => {}
                    }
                }
            }
            _ => {}
        }
    }

    /// A session on a fake harness in `mode`, under a fresh run root.
    fn fake_session(h: Harness, mode: &str) -> (Session, PathBuf) {
        let root =
            std::env::temp_dir().join(format!("mastik-ai-fake-{}", crate::run::random_tag()));
        std::fs::create_dir_all(&root).unwrap();
        let program = root.join(format!("fake-{}", crate::run::random_tag()));
        std::fs::write(&program, "").unwrap();
        crate::find::fake::install(h, &program, mode);
        let wake: Arc<dyn Fn() + Send + Sync> = Arc::new(|| {});
        let s = Session::start(config(h, &root, program), wake).expect("a session");
        (s, root)
    }

    /// Every event of the running turn, waiting for its end.
    fn until_idle(s: &mut Session) -> Vec<Event> {
        let until = Instant::now() + Duration::from_secs(30);
        let mut evs = Vec::new();
        while s.running() && Instant::now() < until {
            std::thread::sleep(Duration::from_millis(20));
            evs.extend(s.poll());
        }
        evs.extend(s.poll());
        evs
    }

    /// A per-turn harness: alive, a call named bare and its result paired, the
    /// system prompt in the first turn only, the second turn resuming by id,
    /// and the run folder gone with the session.
    #[test]
    fn a_per_turn_session_runs_and_resumes() {
        let (mut s, root) = fake_session(Harness::Codex, "codex");
        s.send(Turn {
            text: "hello".into(),
            images: Vec::new(),
        })
        .unwrap();
        assert_eq!(s.send(Turn::default()), Err(Refusal::Busy));
        let evs = until_idle(&mut s);
        assert_eq!(evs.iter().filter(|e| **e == Event::Alive).count(), 1);
        assert!(evs.contains(&Event::ToolCall {
            id: "i1".into(),
            name: "get_node".into(),
            input: serde_json::json!({ "id": "1:2" }),
        }));
        assert!(evs.contains(&Event::ToolResult {
            id: "i1".into(),
            text: "a frame".into(),
            is_error: false,
            image: false,
        }));
        assert!(
            evs.contains(&Event::Text("first sys\n\nhello".into())),
            "{evs:?}"
        );
        assert!(matches!(
            evs.last(),
            Some(Event::Done {
                input_tokens: Some(5),
                is_error: false,
                ..
            })
        ));
        s.send(Turn {
            text: "more".into(),
            images: Vec::new(),
        })
        .unwrap();
        let evs = until_idle(&mut s);
        assert!(evs.contains(&Event::Text("again more".into())), "{evs:?}");
        let folder = s.inner.run_folder.clone();
        assert!(folder.is_dir());
        drop(s);
        assert!(!folder.exists());
        let _ = std::fs::remove_dir_all(&root);
    }

    /// A harness whose context is only in its log reports it just before the
    /// turn's end, read under the session's own home by the id the same last
    /// line names.
    /// flip: the `Done` arm's `from_log` call removed → fails at "read from its log".
    #[test]
    fn a_logged_context_arrives_before_done() {
        let (mut s, root) = fake_session(Harness::Copilot, "copilot-log");
        s.send(Turn {
            text: "hello".into(),
            images: Vec::new(),
        })
        .unwrap();
        let evs = until_idle(&mut s);
        let n = evs.len();
        assert!(n >= 2, "{evs:?}");
        assert_eq!(
            evs[n - 2],
            Event::Context(crate::Reading {
                used: 900,
                window: Some(1000)
            }),
            "read from its log, in {evs:?}"
        );
        assert!(matches!(evs[n - 1], Event::Done { .. }), "{evs:?}");
        drop(s);
        let _ = std::fs::remove_dir_all(&root);
    }

    /// A harness that reports running totals has each turn's own share in its
    /// `Done`, its cached input counted as input.
    /// flip: `usage_is_running_total` answering `false` → fails at "the second turn's own".
    #[test]
    fn running_totals_become_each_turns_own() {
        let (mut s, root) = fake_session(Harness::Antigravity, "agy-totals");
        let mut dones = Vec::new();
        for text in ["one", "two"] {
            s.send(Turn {
                text: text.into(),
                images: Vec::new(),
            })
            .unwrap();
            let evs = until_idle(&mut s);
            dones.extend(evs.into_iter().filter_map(|e| match e {
                Event::Done {
                    input_tokens,
                    cached_tokens,
                    output_tokens,
                    ..
                } => Some((input_tokens, cached_tokens, output_tokens)),
                _ => None,
            }));
        }
        let each = (Some(110), Some(10), Some(5));
        assert_eq!(
            dones,
            vec![each, each],
            "the second turn's own, not the sum"
        );
        drop(s);
        let _ = std::fs::remove_dir_all(&root);
    }

    /// A turn's share of running totals: the difference, the whole when the
    /// count started over, and an unreported count keeps its earlier total.
    #[test]
    fn a_turns_share_of_running_totals() {
        let (turn, kept) = this_turn([Some(100), Some(10), None], [Some(150), Some(4), Some(7)]);
        assert_eq!(turn, [Some(50), Some(4), Some(7)]);
        assert_eq!(kept, [Some(150), Some(4), Some(7)]);
        let (turn, kept) = this_turn(kept, [None, None, None]);
        assert_eq!(turn, [None, None, None], "a failed turn reports nothing");
        assert_eq!(kept, [Some(150), Some(4), Some(7)], "and keeps the totals");
    }

    /// Stop kills a per-turn child and ends the turn as stopped, well inside
    /// the grace; the process belonged to the session until then. The
    /// conversation carries on only when the harness named it before the stop.
    /// flip: `resumable` answering `true` always → fails at "nothing to carry on".
    #[test]
    fn stop_ends_a_hanging_turn() {
        for (mode, named) in [("hang", true), ("hang-unnamed", false)] {
            let (mut s, root) = fake_session(Harness::Codex, mode);
            let owner = s.owner();
            s.send(Turn::default()).unwrap();
            let until = Instant::now() + Duration::from_secs(30);
            let mut evs = Vec::new();
            while !evs.contains(&Event::Alive) && Instant::now() < until {
                std::thread::sleep(Duration::from_millis(20));
                evs.extend(s.poll());
            }
            let pid = s
                .inner
                .lock()
                .child
                .as_ref()
                .map(|c| c.lock().unwrap().id())
                .unwrap();
            assert!(owner(pid));
            let stopped = Instant::now();
            s.stop();
            let evs = until_idle(&mut s);
            assert!(stopped.elapsed() < EXIT_GRACE);
            assert_eq!(evs.last(), Some(&Event::Stopped));
            match named {
                true => assert!(s.resumable(), "{mode}: it carries on"),
                false => assert!(!s.resumable(), "{mode}: nothing to carry on"),
            }
            drop(s);
            assert!(!owner(pid), "a dropped session owns nothing");
            let _ = std::fs::remove_dir_all(&root);
        }
    }

    /// A harness that dies before its turn is done is reported with its words,
    /// recognised as signed out.
    #[test]
    fn an_early_exit_says_why() {
        let (mut s, root) = fake_session(Harness::Codex, "exit");
        s.send(Turn::default()).unwrap();
        let evs = until_idle(&mut s);
        let Some(Event::Exited {
            code,
            signed_out,
            diagnostics,
        }) = evs.last()
        else {
            panic!("{evs:?}");
        };
        assert_eq!(*code, Some(3));
        assert!(*signed_out);
        assert!(diagnostics.contains("Not logged in"));
        drop(s);
        let _ = std::fs::remove_dir_all(&root);
    }

    /// A setup that failed to run is tried again on the next turn, while a
    /// configuration found leaking refuses every turn without being read again.
    /// flip: `prepare` keeping every verdict → fails at "the second turn ran";
    /// keeping only a success → fails at the leak's second refusal.
    #[test]
    fn a_failed_setup_is_retried_and_a_leak_stands() {
        let turn = |s: &mut Session| {
            s.send(Turn::default()).unwrap();
            until_idle(s).pop()
        };
        let (mut s, root) = fake_session(Harness::OpenCode, "opencode-flaky");
        assert!(
            matches!(turn(&mut s), Some(Event::Refused(Refusal::Unsealed { .. }))),
            "the first read fails"
        );
        let second = turn(&mut s);
        assert!(
            matches!(second, Some(Event::Exited { code: Some(0), .. })),
            "the second turn ran: {second:?}"
        );
        drop(s);
        let _ = std::fs::remove_dir_all(&root);

        let (mut s, root) = fake_session(Harness::OpenCode, "opencode-leaky");
        for _ in 0..2 {
            assert!(matches!(
                turn(&mut s),
                Some(Event::Refused(Refusal::Unsealed { .. }))
            ));
        }
        let reads = std::fs::read_to_string(s.inner.cwd.join("config-reads")).unwrap();
        assert_eq!(reads, "1", "read once");
        drop(s);
        let _ = std::fs::remove_dir_all(&root);
    }

    /// A Windows command line is measured as std quotes it: an argument of
    /// quote marks, short when counted bare, is refused as too long and named.
    /// flip: `quoted_len` counting `"` as one unit → fails at the escaped-quote
    /// assertion, and with that removed at the `TooLong` match.
    #[cfg(windows)]
    #[test]
    fn a_command_line_is_measured_as_quoted() {
        assert_eq!(quoted_len("ab", false), 3);
        assert_eq!(quoted_len("a b", false), 6);
        assert_eq!(quoted_len(r#"a\"b"#, false), 7);
        assert_eq!(quoted_len(r"a b\", false), 8);
        assert_eq!(quoted_len("5%", true), 12);
        let (s, root) = fake_session(Harness::Codex, "codex");
        let r = s.inner.check_argv(&["\"".repeat(20_000)], "sys");
        assert!(
            matches!(
                r,
                Err(Refusal::TooLong {
                    what: "your message",
                    ..
                })
            ),
            "{r:?}"
        );
        drop(s);
        let _ = std::fs::remove_dir_all(&root);
    }

    /// A stdout line that is not UTF-8 is kept as a diagnostic, and the turn
    /// still reads on to its end.
    /// flip: `run_turn` reading `BufRead::lines` and breaking on an error →
    /// fails at the `Diagnostic` assertion.
    #[test]
    fn a_line_in_another_code_page_does_not_end_the_turn() {
        let (mut s, root) = fake_session(Harness::Codex, "code-page");
        s.send(Turn::default()).unwrap();
        let evs = until_idle(&mut s);
        assert!(
            evs.contains(&Event::Diagnostic("Fehler: Datei \u{fffd}berall".into())),
            "{evs:?}"
        );
        assert!(evs.contains(&Event::Text("done".into())), "{evs:?}");
        assert!(
            matches!(
                evs.last(),
                Some(Event::Done {
                    is_error: false,
                    ..
                })
            ),
            "{evs:?}"
        );
        drop(s);
        let _ = std::fs::remove_dir_all(&root);
    }

    /// A persistent harness keeps one process across turns, and Stop sends
    /// Claude Code its interrupt rather than killing it.
    #[test]
    fn a_persistent_session_holds_one_process() {
        let (mut s, root) = fake_session(Harness::Claude, "claude");
        s.send(Turn {
            text: "one".into(),
            images: Vec::new(),
        })
        .unwrap();
        let evs = until_idle(&mut s);
        assert!(evs.contains(&Event::Text("one".into())), "{evs:?}");
        let pid = s
            .inner
            .lock()
            .child
            .as_ref()
            .map(|c| c.lock().unwrap().id());
        s.send(Turn {
            text: "slow".into(),
            images: Vec::new(),
        })
        .unwrap();
        std::thread::sleep(Duration::from_millis(300));
        s.stop();
        let evs = until_idle(&mut s);
        assert_eq!(evs.last(), Some(&Event::Stopped), "{evs:?}");
        let after = s
            .inner
            .lock()
            .child
            .as_ref()
            .map(|c| c.lock().unwrap().id());
        assert_eq!(pid, after, "the interrupt kept the process");
        drop(s);
        let _ = std::fs::remove_dir_all(&root);
    }

    /// Antigravity, which Stop kills, is started again by the next turn: with
    /// `--conversation` and no second system prompt once the conversation was
    /// named, and as a new conversation told the system prompt when it was not.
    /// flip: `read_persistent` without its `owes_system` recomputation → fails
    /// at the unnamed case's reply.
    #[test]
    fn antigravity_starts_again_after_a_stop() {
        let say = |text: &str| Turn {
            text: text.into(),
            images: Vec::new(),
        };
        for (first, then) in [("slow", "resumed two"), ("slow unnamed", "new sys\n\ntwo")] {
            let (mut s, root) = fake_session(Harness::Antigravity, "agy");
            // Its registration edits the person's own Antigravity: held as done.
            s.inner.lock().prepared = Some(Ok(()));
            s.send(say(first)).unwrap();
            let until = Instant::now() + Duration::from_secs(30);
            let mut evs = Vec::new();
            while !evs.iter().any(|e| matches!(e, Event::Text(_))) && Instant::now() < until {
                std::thread::sleep(Duration::from_millis(20));
                evs.extend(s.poll());
            }
            let told = Event::Text(format!("new sys\n\n{first}"));
            assert!(evs.contains(&told), "{first}: the first turn: {evs:?}");
            s.stop();
            let evs = until_idle(&mut s);
            assert_eq!(evs.last(), Some(&Event::Stopped), "{first}");
            s.send(say("two")).unwrap();
            let evs = until_idle(&mut s);
            assert!(evs.contains(&Event::Text(then.into())), "{first}: {evs:?}");
            drop(s);
            let _ = std::fs::remove_dir_all(&root);
        }
    }

    /// Stop returns at once while a turn's line is still being written to a
    /// harness that is not reading it, and the turn ends as stopped.
    /// flip: `stop` taking the stdin lock on its caller's thread → fails at
    /// "stop returned at once", after the fake gives up a minute later.
    #[test]
    fn stop_does_not_wait_on_a_write_in_progress() {
        let (mut s, root) = fake_session(Harness::Claude, "deaf");
        // Far past any pipe's buffer, so the write blocks.
        s.send(Turn {
            text: "x".repeat(8 << 20),
            images: Vec::new(),
        })
        .unwrap();
        let until = Instant::now() + Duration::from_secs(30);
        while s.inner.lock().stdin.is_none() && Instant::now() < until {
            std::thread::sleep(Duration::from_millis(20));
        }
        std::thread::sleep(Duration::from_millis(300));
        let writing = s
            .inner
            .lock()
            .stdin
            .clone()
            .is_some_and(|i| i.try_lock().is_err());
        assert!(writing, "the fixture's write is in progress");
        let stopped = Instant::now();
        s.stop();
        assert!(
            stopped.elapsed() < Duration::from_secs(2),
            "stop returned at once: {:?}",
            stopped.elapsed()
        );
        let evs = until_idle(&mut s);
        assert_eq!(evs.last(), Some(&Event::Stopped), "{evs:?}");
        assert!(stopped.elapsed() < EXIT_GRACE);
        drop(s);
        let _ = std::fs::remove_dir_all(&root);
    }

    /// The text of a turn's replies, joined.
    fn said(evs: &[Event]) -> String {
        evs.iter()
            .filter_map(|e| match e {
                Event::Text(t) => Some(t.as_str()),
                _ => None,
            })
            .collect()
    }

    /// A new grant on Claude Code starts its process again on the next turn,
    /// resuming the conversation with the new tools allowed and the new system
    /// prompt in its file; the ended process's exit reaches no turn.
    ///
    /// flip: `take_access` not ending the process → fails at `--resume`, the
    /// turn answered by the old process; `read_persistent` reporting an ended
    /// generation's exit → fails there too, the turn ended before it ran.
    #[test]
    fn a_new_grant_restarts_claude_code_resuming_its_conversation() {
        let (mut s, root) = fake_session(Harness::Claude, "claude-argv");
        let turn = |s: &mut Session, text: &str| {
            s.send(Turn {
                text: text.into(),
                images: Vec::new(),
            })
            .unwrap();
            until_idle(s)
        };
        let pid = |s: &Session| {
            s.inner
                .lock()
                .child
                .as_ref()
                .map(|c| c.lock().unwrap().id())
        };
        let evs = turn(&mut s, "one");
        assert!(!said(&evs).contains("--resume"), "{evs:?}");
        let first = pid(&s);
        assert!(s.set_access(vec!["get_node".into(), "set_name".into()], "sys two".into()));
        let evs = turn(&mut s, "two");
        let text = said(&evs);
        assert_ne!(pid(&s), first, "a new process");
        assert!(text.contains("--resume s-1"), "{text}");
        assert!(text.contains("mcp__mastik__set_name"), "{text}");
        assert!(
            matches!(
                evs.last(),
                Some(Event::Done {
                    is_error: false,
                    ..
                })
            ),
            "the turn ended as done: {evs:?}"
        );
        let file = s.inner.base.system_file.clone().expect("a system file");
        assert_eq!(std::fs::read_to_string(file).unwrap(), "sys two");
        // Taken once: the turn after keeps the process.
        let kept = pid(&s);
        turn(&mut s, "three");
        assert_eq!(pid(&s), kept);
        drop(s);
        let _ = std::fs::remove_dir_all(&root);
    }

    /// A persistent harness whose running process has not named its
    /// conversation cannot take a new grant without losing it: refused, and
    /// nothing is owed.
    /// flip: `set_access` without its persistent check → fails at "refused".
    #[test]
    fn an_unnamed_persistent_conversation_refuses_a_new_grant() {
        let (mut s, root) = fake_session(Harness::Antigravity, "agy");
        s.inner.lock().prepared = Some(Ok(()));
        s.send(Turn {
            text: "unnamed".into(),
            images: Vec::new(),
        })
        .unwrap();
        until_idle(&mut s);
        assert!(s.inner.lock().stdin.is_some(), "fixture: the process runs");
        assert!(
            !s.set_access(vec!["set_name".into()], "sys".into()),
            "refused"
        );
        assert!(!s.inner.lock().access_owed);
        drop(s);
        let _ = std::fs::remove_dir_all(&root);
    }

    /// A per-turn harness keeps its conversation across a new grant, and the
    /// next turn's command line carries the new tools.
    /// flip: `granted_spec` leaving Codex's overrides at the old grant → fails
    /// at the approval.
    #[test]
    fn a_new_grant_keeps_a_per_turn_conversation() {
        let (mut s, root) = fake_session(Harness::Codex, "codex");
        s.send(Turn::default()).unwrap();
        until_idle(&mut s);
        assert!(s.set_access(vec!["get_node".into(), "set_name".into()], "sys".into()));
        let spec = s.inner.turn_spec(&s.inner.lock(), "", None, 9, &[]);
        assert!(
            spec.mcp_overrides
                .iter()
                .any(|o| o.contains("set_name={approval_mode=\"approve\"}")),
            "{:?}",
            spec.mcp_overrides
        );
        s.send(Turn {
            text: "more".into(),
            images: Vec::new(),
        })
        .unwrap();
        let evs = until_idle(&mut s);
        assert!(said(&evs).starts_with("again"), "resumed: {evs:?}");
        drop(s);
        let _ = std::fs::remove_dir_all(&root);
    }

    /// A new grant rewrites the files that hold it before the next turn:
    /// Cursor's permissions, OpenCode's configuration, which is read back
    /// again.
    /// flip: `take_access` skipping Cursor's file → fails at its tool.
    #[test]
    fn a_new_grant_rewrites_the_files_that_hold_it() {
        let (mut s, root) = fake_session(Harness::Cursor, "codex");
        assert!(s.set_access(vec!["set_name".into()], "sys".into()));
        s.inner.take_access().unwrap();
        let cli = std::fs::read_to_string(s.inner.cwd.join(".cursor").join("cli.json")).unwrap();
        assert!(cli.contains("set_name"), "{cli}");
        drop(s);
        let _ = std::fs::remove_dir_all(&root);

        let (mut s, root) = fake_session(Harness::OpenCode, "opencode-flaky");
        s.inner.lock().prepared = Some(Ok(()));
        assert!(s.set_access(vec!["set_name".into()], "sys".into()));
        s.inner.take_access().unwrap();
        let cfg = s
            .inner
            .run_folder
            .join("opencode-config/opencode/opencode.json");
        let cfg = std::fs::read_to_string(cfg).unwrap();
        assert!(cfg.contains("set_name"), "{cfg}");
        assert!(s.inner.lock().prepared.is_none(), "read back again");
        drop(s);
        let _ = std::fs::remove_dir_all(&root);
    }
}
