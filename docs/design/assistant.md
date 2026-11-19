# The assistant (ai.md §1–§7)

The assistant lets a person ask a coding agent about the design in front of them: what a layer is
doing, why a layout breaks at a width, how to name a set of variants, what to change to fix a
contrast failure. They ask in a panel at the tool rail's foot, attach layers from the canvas, and
read short answers that can point back at layers and propose edits. The harness layer is
`crates/mastik-ai`; the window's side is `mastik-app`'s `assistant` module and `mcp::{access,
propose, agent_log}`. Headings keep the numbers they had in `ai.md`, so an `ai.md §4` cited from
code is found here.

## 1. The thesis

**Mastik embeds no model and holds no provider key.** The person already pays for, and is signed in
to, a headless coding agent. Mastik starts that agent as a child process, points it at Mastik's own
MCP endpoint, and shows its turn in the assistant panel. The harness owns the model, the account,
the billing and the rate limits; Mastik owns what the agent can see and what it can change.

- **No keys in Mastik.** Sign-in is always the harness's own. Settings holds no secret, and nothing
  goes into the keychain for the assistant.
- **One way in.** The agent reads and writes through the same MCP tools an outside client gets, so
  its edits are ordinary transactions on `mcp.md` §8.3's queue, undoable like any other
  (invariant 10).
- **Mastik seals the agent.** A coding agent arrives with a shell, a file editor and a web fetcher.
  Mastik starts every harness with its built-in tools off and only Mastik's MCP tools allowed (§4).

## 2. Harnesses

A **harness** (`mastik_ai::Harness`) names a wire dialect, not a vendor: a fork that speaks its
parent's stream counts as the parent. Each has a persisted key, a label (*Claude Code*), a program
name (`claude`) and a speaker name (*Claude*) shown on its replies. An unknown saved key
(`Harness::from_key`) is no harness and reads as *Off*.

| Harness | Program | Shape | Resume | Stop |
| --- | --- | --- | --- | --- |
| Claude Code | `claude` | one process per conversation, turns as NDJSON on stdin | the process holds the thread; `--resume <id>` after a kill | interrupt control request |
| Codex | `codex` | one process per turn, prompt on stdin | `exec resume <id>` | kill |
| Antigravity | `agy` | one process per conversation, NDJSON on stdin | `--conversation <id>` after a kill | kill |
| OpenCode | `opencode` | one process per turn, prompt on stdin | `--session <id>` | kill |
| GitHub Copilot | `copilot` | one process per turn, prompt on stdin | `--resume=<id>` | kill |
| Cursor | `cursor-agent` | one process per turn, prompt on stdin | `--resume <id>` | kill |
| DeepSeek Harness | `dsh` | one process per turn, prompt on stdin | `--session-id <id>` | kill |

- **Behaviour is asked as a capability, never by identity**: persistent or per-turn, prompt on
  stdin, system prompt in the first turn, interrupt, effort levels, images, how it is sealed. Each
  capability is an exhaustive `match`, so a new harness is a compile error, not a quiet default.
- **Every harness streams JSON lines**, and no flag or stream shape is driven that was not measured
  against the real program. DeepSeek Harness's `--json` sends a step at a time rather than text
  deltas.
- **Images reach Claude Code, Codex, Copilot and OpenCode**: Claude Code as content blocks in the
  turn's line, the others as PNG files written into the session's working folder and named on the
  turn's command line, Codex's `--image`, Copilot's `--attachment` and OpenCode's `--file` (all in
  the `--flag=value` form). Inside the working folder, so neither Copilot's path check nor
  OpenCode's external-folder rule meets them. Copilot's and OpenCode's flags are read off their
  own `--help`, not yet a measured run. `Harness::takes_images` is false for Antigravity, Cursor
  and DeepSeek Harness, which have no way in for an image and are told in words instead (§4,
  *Pictures*).
- **DeepSeek Harness takes almost nothing as a flag.** A `--patch` overlay written into the
  session's own `DSH_HOME` carries the model, the effort, the seal (every built-in tool row
  disabled by id), the sign-in and Mastik's server. The overlay is checked with `--dump-config`
  before every turn, and a row left live refuses the turn. The sign-in is the person's: their dsh
  account, read from their own credentials file by path and never copied, or a `DEEPSEEK_API_KEY`
  already in their environment, passed through and never stored.
- **Copilot and DeepSeek Harness run with a home in the session's run folder**, so their own
  configuration stays out; it goes with the session, so a resume holds within a session and never
  across one. Copilot's login file is copied in, since a home without a credential store would be
  signed out.
- **Cursor runs with the person's own Cursor folder** and no home of its own: its restriction and
  Mastik's server are project files (`.cursor/cli.json`, `.cursor/mcp.json`) in a workspace inside
  the run folder, its working folder. So the person's MCP servers reach it (its seal sentence says
  so), and it records the server's approval and its chats in their folder, keyed by that workspace;
  when the session ends `cursor::forget` removes the entries that name the workspace exactly.
- **Cursor needs its server approved** (`cursor-agent mcp enable`) before the first turn; a failed
  approval refuses the turn.
- **One-time setup holds once it succeeds** (Antigravity's registration, Cursor's approval,
  OpenCode's read-back): a failure refuses only its turn, since a helper past its deadline or a busy
  settings lock passes, and the next turn tries again. An OpenCode configuration read and found
  leaking is the exception: it refuses every turn of the session.
- **OpenCode reads its configuration from the session**: `XDG_CONFIG_HOME` at the session's
  `opencode.json`, project config off, and `OPENCODE_TEST_HOME` at an empty folder, since
  `~/.opencode/opencode.json` is merged over everything and no option turns it off. Because it
  merges from places Mastik cannot list, `opencode debug config` is read back before the first turn,
  and another enabled server, or a tool or permission the sealed agent allows, refuses the session.
- **OpenCode's agent is sealed by a wildcard**: its permissions deny `*` and then allow `mastik_*`
  (OpenCode applies the last rule that matches), so a built-in a later build adds is denied too. The
  read-back requires that deny rather than auditing the tools it knows, since a denylist by name
  leaves a new tool live.
- **Antigravity is configured globally**: its server is registered in the person's own registry and
  its allow rules written into its `settings.json`, both installed with the session and withdrawn
  when it ends. Each session registers under a name of its own, `mastik` and eight hex digits
  (`antigravity_server_name`), and grants and withdraws rules for that name only, so it never adds,
  replaces or removes a registration or rule named `mastik` the person made. Its claim marker, named
  after it and created exclusively, records the owning Mastik and the `agy` path it registered with,
  and is removed only once the withdrawal held. While a session holds them, an `agy` the person
  runs finds Mastik's server registered and its tools pre-approved; no option scopes either to one
  process.
- **Each harness is driven through its own headless mode, not the Agent Client Protocol.** ACP
  would turn the dialects into one, but Claude Code and Codex speak it only through adapter
  packages, which the person would have to install and which may not run the program they signed in
  to. Not planned (`non-goals.md`).

### Finding and probing the program

- **Lookup order:** a path set in Settings wins outright; else an `MASTIK_<HARNESS>_BIN` variable,
  then each harness's known install folders (CLIs are often installed off the inherited `PATH`),
  then `PATH` with `PATHEXT`.
- **On Windows** npm's `.cmd` shims come last, since Rust refuses a multi-line argument to a batch
  file. Behind a shim, the real program is started directly. Every child is started with
  `CREATE_NO_WINDOW` and `NoDefaultCurrentDirectoryInExePath=1`, in a Job Object of its own nested
  in the session's, so *Stop* and a grace kill end the harness's whole process tree. Off Windows
  each child leads a process group of its own, and the group is ended when the child is reaped, so
  what it leaves behind (a server it did not reap) does not outlive it.
- **A helper** (the `--help` probe, `agy mcp …`, `cursor-agent mcp enable`) runs in a private empty
  folder and is ended, with its tree, past a deadline (`process::run_bounded`): 15 seconds for the
  probe, 30 for `agy`, 60 for `cursor-agent mcp enable`. The read-backs of a resolved
  configuration, OpenCode's `debug config` and DeepSeek Harness's `--dump-config`, run with the
  session's environment in its working folder and are ended past 30 seconds.
- **Mastik never installs a harness.** A missing one is said in the panel with the program it
  looked for.
- **A probe reads `--help` once per program path** and caches what it finds: which sealing flags
  exist, and so how sealed a session can be (§4). A probe that could not run is not cached, and
  `Found::answered` says so, since a probe that did not finish refused nothing.
  `mastik_ai::warm` runs it in the background only while the assistant is on.
- **`mastik_ai::sweep` runs at every launch** on a background thread (`assistant::at_launch`):
  leftovers carry their owner's pid and start time, and only those whose owner is gone are removed,
  Antigravity's registration and rules included.

### Model and effort

- **The model is free text**, passed verbatim; empty omits the flag. Each harness offers a few
  suggestions (`Harness::model_suggestions`) in the field's list and ghost, never as the permitted
  set.
- **Effort uses each harness's own words** (`Harness::effort_levels`), and a level not on its list
  is never sent. Codex, Copilot and Cursor have none, so the Effort row is hidden: Copilot's flag
  fails the turn on its default `auto` model, and Cursor's effort is part of the model id.

## 3. A turn

- **Events** (`mastik_ai::Event`): text deltas, tool calls, tool results (paired to their call by the
  harness's own id, never by recency, which mis-pairs parallel calls), the end of the turn with its
  duration and token counts, a context reading, the harness exiting first, a refusal, and
  *Stopped*. Thinking is not shown.
- **A context reading** (`Event::Context`, `mastik_ai::Reading { used, window }`, `context.rs`)
  is the prompt of the latest model call, cached tokens included: what the next call starts from.
  `Done`'s tokens are summed over the turn's calls, each of which reads the same context again, so
  they are never used for it. Where each harness says it:

  | Harness | Used | Window |
  | --- | --- | --- |
  | Claude Code | each `assistant` event's `message.usage` (input + cache read + cache creation), per call | the `result`'s `modelUsage.<model>.contextWindow`, the latest call's model else the largest; kept for later turns |
  | OpenCode | each `step_finish`'s tokens (input + cache read + cache write), per step | none |
  | DeepSeek | each `status` `step_end`'s usage, per step | none |
  | Codex | the rollout file's latest `token_count`, `last_token_usage.input_tokens` | its `model_context_window` |
  | Copilot | `$COPILOT_HOME/session-state/<session>/events.jsonl`, the session's own home: the latest `session.usage_record`'s `inputTokens` | its `maxPromptTokens` |
  | Antigravity | `~/.gemini/antigravity-cli/brain/<conversation>/.system_generated/logs/transcript_full.jsonl`, the latest `source: MODEL` step (input + cache read) | none |
  | Cursor | the turn's sum, read only on a turn with no tool call (one model call) | none |

  Codex's stream gives only the turn's sum, Copilot's no counts, and Antigravity's `result` a
  running total over the session, so those three are read from the harness's log at the turn's
  end and emitted just before `Done`. Codex files a thread under the day it started
  (`$CODEX_HOME` or `~/.codex`, `sessions/YYYY/MM/DD/rollout-*-<thread>.jsonl`) and a resumed turn
  appends to that file, so the days are searched newest first. A log is read from its last 512 KiB,
  and an id holding a path separator or a dot is never joined into a path.
- **The parser is stateful per harness:** it turns restated text into deltas, drops restated tool
  calls, and resets at the end of each turn. A line that is not JSON is kept as a diagnostic, and
  so is Codex's `error` while it reconnects: only `turn.failed` ends its turn as failed.
- **Stop** sends Claude Code its interrupt and kills every other harness's child with its whole
  tree (its own process group off Windows). An interrupt that
  has not ended the turn in ten seconds kills the process, and the next turn respawns it with
  `--resume <session id>`. Stop runs on the UI thread and never waits on the harness's stdin: the
  interrupt is written on a thread of its own, and while the turn's line is still being written
  the harness has not read the turn, so its process is killed at once. The turn ends only when the
  harness reports `Event::Stopped`; until then the send button reads *Stopping…*, disabled, and
  then a note says *Stopped. The next question resumes the session.* A conversation the harness
  had not yet named cannot be resumed (Copilot names it only as a turn ends, so a first turn
  stopped early has none): the session ends instead, the note says *Stopped. The next question
  starts a new session with the thread so far.*, and that session replays the prose.
- **A harness that exits before answering ends the session** as any ending does (the grant revoked
  at once, the rest off the UI thread): the next question starts a new one, with the thread's prose
  replayed as a restored conversation is.
- **A stale stream is discarded.** Every event is tagged with its turn and dropped once a newer turn
  has begun, and a session the person has left (switched document, changed settings) is dropped
  whole.
- **The working folder** is a fresh, exclusive, owner-only folder under the machine data folder's
  `run/` for each session, never the document's folder and never the app's working directory, so
  no planted agent settings file beside a document can configure the child. A session that cannot
  make one is refused.
- **The MCP server** in the harness's config is `mastik mcp-proxy --client <key> --document <path>`.
  The key is the session's, so a harness that starts a process per turn finds its document and
  revisions on its next connection, with `changed` reporting what the person did in between
  (`mcp.md`, *Revisions*).
- **The environment** marks the child (`MASTIK_AGENT_CHILD=1`) and clears the variables that would
  point a harness at another configuration or reopen its seal (`OPENCODE_PERMISSION` among them).
  Under the mark the `mastik` command line refuses everything but `mcp-proxy`, `serve` included, and
  the proxy connects only to the window, never starting `serve`: the window holds the chat's grant
  and `serve` none, so an agent there would act unchecked. A speed bump rather than a boundary.
  Codex and Cursor start an MCP server without the agent's environment, so their server config
  sets the mark itself, and the proxy also counts a chat's `--client` key (`assistant-…`) as the
  mark. Every child's `PWD` is its own working folder: OpenCode's runtime reads an inherited one
  as its working folder, which would put it in the shell's folder Mastik was started from.
- **The window must have its endpoint.** A chat refuses to start when the window could not open
  its MCP endpoint, since the chat's level lives in the window's grants.
- **A command line too long** for the platform is refused before spawning, naming the limit and
  what made it long (the system prompt, where Claude Code's probe found no file flag for it, say).

## 4. What the agent can see and do

### Sealing

Each harness is started with its built-in tools off, the person's other MCP servers excluded where
the harness allows it, and only Mastik's tools allowed. The mechanism differs per harness (flags, a
sandbox, a rules or permissions file, a config overlay), and each **fails closed**: a harness whose
restricting config cannot be written, or whose probe cannot confirm its restricting options, is
refused, not started open. The seal is graded — *Sealed* where built-in tools are off, *Restricted*
where a sandbox or rules file holds them — and the panel's head shows its sentence
(`seal_sentence`), naming the mechanism, saying where a harness cannot keep the person's own MCP
servers away (Antigravity, Cursor), and, at *Restricted* outside Windows, that the access level is
a rail rather than a boundary.

Never passed, whatever a harness offers: any skip-permissions, auto-approve, allow-all or yolo
flag. A headless harness cannot ask the person mid-turn, so its allow-list is exactly the tools the
access level lists and everything else is denied.

### Access levels

**Access** (`mcp::access::Level`) decides what the agent may do. Settings holds the default for a
new chat, and the panel's head switches it for the chat in front; a chat on the default follows
it when another is saved. Tools above the level are **left out of the tool list**, not listed and
refused: an agent that sees a tool plans around it.

**A switch keeps the session** (`assistant_regrant`, `Session::set_access`), so the agent keeps
what it has read:

- **The host's grant changes at once.** A call from then on is answered at the new level, a
  running turn's included, and `tools/list` lists the new level's tools. A running turn is not
  stopped.
- **The harness takes it from the next question.** Every later turn's command line carries the
  new allow-list, and that turn first rewrites what holds the grant outside it: Claude Code's
  system-prompt file, Cursor's `.cursor/cli.json`, OpenCode's `opencode.json` (read back again
  before the turn), Antigravity's rules in its `settings.json`. A rewrite that fails refuses the
  turn and is tried again with the next.
- **Per harness:** Codex (`-c` approvals), Copilot (`--available-tools`), OpenCode, Cursor and
  DeepSeek Harness (whose overlay names no Mastik tool) start a process per turn, so the next turn
  simply resumes its conversation under the new grant. Claude Code and Antigravity hold one
  process whose allow-list was fixed at its start: the next turn ends it and starts it again with
  `--resume` or `--conversation`, the same resume a Stop that had to kill uses, so the
  conversation and its reads carry on in the new process.
- **The agent is told.** The next turn's preamble opens with `access changed by the person, from
  now on:` and the level's paragraph from the system prompt (`prompt::level_rules`), since a harness that took the system prompt in the
  first turn still holds the old one; Claude Code's restarted process also reads the new system
  prompt.
- **Where it cannot be kept, it restarts as before**: a persistent harness whose running process
  has not yet named its conversation would lose it, so the session ends (a running turn with
  *Stopped: the access level changed.*) and the next question starts a new one with the prose
  replayed.

- **Look** — Phase 1's reads and exports (`mcp.md` §8.4), and `search_assets`. `export_png` is the
  agent's eyes.
- **Propose** (the default) — Look, plus `propose_edit` (*Proposals*), whose calls may include
  `place_asset`; Propose does not run `place_asset` itself, and `describe_tools` describes it
  there as a call to propose (`access::usable`).
- **Edit** — Look, plus the writes of Phases 2 and 3 and `place_asset`. Each call commits as its own
  undo step marked as the assistant's (`Origin::Mcp`). Edit does not list `propose_edit`.

Every level lists the core set within it (`access::listed`) plus `describe_tools` and `call`, and
reaches its other tools through `call`. Phase 4, the kits and the library, is never a chat's, but
for the assets browser's two tools (`mcp.md` §8.4, *The assets browser*).
`propose_edit` sits outside the phases, in `mastik_mcp::tools::ASSISTANT`.

**The host holds the level, not the harness.** The endpoint serves any process running as the
person, and a harness with a shell can start `mastik mcp-proxy` itself or open the pipe or socket
directly. So the window, which starts the harness, grants each session's level to the MCP host
(`access::Grants`) and the host enforces it on every call (`Engine::handle`):

- **By key.** A call carrying a session's key acts at its level; a tool above it is refused, by
  name, through `call` and as a `batch` step (`access::refused`), even though it was never listed.
- **By process.** The endpoint reads the connecting process at accept (`GetNamedPipeClientProcessId`
  on Windows, `SO_PEERCRED` on Linux, `LOCAL_PEERPID` on macOS, none on other Unixes) and every
  call carries it (`Call::pid`). A process the session owns acts under its key and level whatever
  key it sends. On Windows ownership is the session's Job Object, which every process it starts
  joins however it is launched; elsewhere the parent chain is walked, which a process that left its
  tree (`setsid`, a double fork) escapes.
- The window grants the key before spawning and again with the session's process owner once it
  starts. Ending the session revokes the grant at once, on the UI thread, and finishes the rest
  (withdrawing Antigravity's registration, a bounded wait) off it. A revoked grant is kept as
  *ended*, not removed, so a call still arriving from the chat is refused (*This chat has ended:
  nothing was changed.*) rather than served as an ungranted client's; the last 32 are kept. A
  chat's key the window holds no grant for (an earlier run's chat, after a relaunch, or one
  forgotten past those 32) is refused the same way, by its `assistant-` form.
- A granted call is written to the agent log (§7).

Left alone: `describe_tools` still describes tools above a chat's level, and a client no grant
covers is an ordinary MCP client and can call `propose_edit` too.

At the *Sealed* grade the agent has no shell, so it cannot reach the endpoint any other way. At
*Restricted*, *Look* and *Propose* are only as strong as the sandbox outside Windows: a safety rail
against an eager model, not a boundary against a hostile one. The seal's sentence says so.

### Proposals

- **`propose_edit(title, calls)`** takes `calls` as `{verb, args, as?}`, each a registry verb of
  Phases 2 or 3, `set`, or `place_asset`; a call may name its verb `tool`, as a `batch` step does,
  since an agent copies one shape for the other. A later call names what an earlier one made as
  `"@label"`, resolved on each run, since the trial run on the copy mints its own ids
  (`Proposal::labels`), and `"@page"` is the first page, as in a batch; a `place_asset` may fill
  or go into a layer named so, its target checked once the calls run (`assets::put_held`). A
  `page` given by name reads as that page's id in each call, as in a call of its own. They are checked by running them on a copy of the document (`mcp::propose::check`), so the
  person's document, its revision and their redo stay as they were. The answer is
  `{proposal, changes}`: the id, and what each call changes in its verb's own words. A refusal
  names the call and proposes nothing.
- **The thread keeps its proposals; the host keeps only its last 64, in memory.** As a
  `propose_edit` result arrives, the panel copies the proposal out of the host into the thread
  (`Assistant::proposals`), and the thread's file saves it with whether the person applied it
  (§6), so a card survives a restart, a tab switch and the host forgetting it. A tool row of
  `propose_edit` renders as its card; one whose proposal was not kept (too large to save, or saved
  before proposals were) says *This proposal was not kept; ask again to see it.*
- **An applied card is found again on load.** Step serials are minted afresh when a document's
  history is read back, so a saved card records only that it was applied; loading the thread
  finds its undo step again (`propose::applied_steps`): the done steps that are a `propose_edit`
  by the card's chat under its title, which the applied cards sharing that chat and title take
  one each in the order they were proposed. Found, the card offers *Undo* as before; when the
  steps found are not one per such card (one card's own step gone, another's left), none of
  them takes one and each reads *Applied as one step* with no *Undo*, since a guess could take
  back the wrong step.
- **A card read back without its photo or icon** (a thread's file never holds them) applies the
  host's copy while the host still holds one (`panel::held_copy`), so a tab switch keeps a
  placement's *Apply*; once the host has let it go, *Apply* says the asset is no longer at hand.
- **A card's *Undo* while the document takes none** (compare, a script running) is refused with
  the status line's reason, and the card stays applied.
- **The card** heads with the sparkle, the title and the count of changes, lists the changes, and
  ends in one of:
  - **Apply** — runs the calls again on the document as it is, as one undo step named by the title
    and marked as the assistant's (`Origin::Mcp`): the person applies it, the agent wrote it. It
    is refused, with nothing changed, when the calls now reach a layer the card did not list,
    other than ones they create (a `replace_text` with no `nodes` meeting a layer added since):
    *The proposal now also changes `<name>`, which it did not list.* A refused Apply keeps the
    person's redo (`propose::apply`). A text edit still open ends first, as its own step; when
    that edit changed a layer the proposal writes, Apply is refused as the card would now show it
    (*`<name>` changed after this was proposed. Nothing was changed.*). *Undo* ends an open text
    edit first too, so it never takes back the person's typing in place of the proposal.
  - **`<layer> changed after this was proposed`**, in the warning colour, with *Ask again*, which
    asks *`<name>` changed. Propose again.* with the proposal's written layers that still exist
    attached.
  - **Applied as one step**, with *Undo* while that step is done and not taken back
    (`propose::can_undo`): while it is the last step *Undo* undoes it; under later steps it takes
    the proposal back alone as a new step when they changed other things (`EditorSession::revert`,
    as an agent's `undo` does, `mcp.md` §8.3), and otherwise says on the status line which later
    step changed what it changed, and nothing moves.
  - **Made in another document**.

### Context sent with a turn

- **The system prompt** (`assistant::prompt::system`) says what Mastik is, the access level and its
  tools truthfully, how to propose an edit, how to read a scope (work in that frame, read it
  first, put new layers inside it, unless the question names another place), and how to answer,
  then the person's own instructions from Settings, which come last and so win.
- **Answers are short**, since the panel is narrow: one or two sentences, the answer first, no
  preamble, recap or offer of more. A choice is at most three options in an ` ```options ` block,
  one short line each, and a question. A layer is named by its id, so it renders as a link. A table
  or code only when asked for one or when the answer is code.
- **The prompt goes in the harness's system-prompt flag** where it has one, so it holds on every
  turn and survives the harness compacting its context; otherwise at the top of the conversation's
  first turn. Claude Code takes it as a file (`--append-system-prompt-file`, falling back to
  `--append-system-prompt` where its probe lacks the flag), keeping it and the replayed
  conversation off the length-limited, world-readable command line. Every per-turn harness,
  OpenCode included, takes each turn's prompt on stdin, the first turn's carrying the system
  prompt, so neither is on the command line. OpenCode
  would also wrap an argument holding a space in quotes, escaping the quotes inside it, before the
  model saw it.
- **Each turn opens with a preamble**, fenced as data: the revision; the page, its name and id,
  since every `page` parameter takes the id (and reads a unique page name as it, `mcp::page_by_name`,
  a miss answered with the pages and their ids); the selection; the
  layers added, changed and removed since the last turn, by id and name (forty at most, then how
  many more); the scope, as `scope: ‹name› ‹id› (kind, w×h); work in this frame unless the question
  names another place`; the attached layers; the pictures (*Pictures*); the access level when the
  person changed it since the last turn (*Access levels*); the person's last three questions, since a harness's
  own memory of earlier turns is unreliable; and `style: brief`. The scope line and `style: brief`
  go on every turn, since an instruction given once fades over a long thread. The page outline,
  names and ids only, goes only on a session's first turn, the scoped frame first when it is on
  the page in front, so the outline's cap never cuts it. "Since the last turn" means since the last
  turn the harness was sent: what the agent was shown moves on only when the send succeeds, and
  back again when the turn is refused before the harness reads it. A name
  in the preamble is written on one line with `<` and `>` as `‹` and `›`, so no layer or page name can
  close the `<mastik-turn>` block and write words the model would read as the person's.
- **The design is never sent whole.** The agent's earlier reads stay in its session, and the
  preamble says which went stale. Tool results' `changed` cannot do this alone: an agent answering
  from memory calls no tool.
- **A restored conversation** replays its prose into a fresh session's system prompt, with a line
  for each proposal the agent made, its title, its changes and whether the person applied it. No
  other tool call is replayed, since what a read returned may have gone stale. A thread another
  harness wrote is not replayed, and the chat says so.
- **Document content is data, never instructions.** Layer names, text content, component
  descriptions and variable names may come from someone else's file or kit; the system prompt and
  the MCP server's instructions say so.

### Attaching layers

- **Two doors attach, and only two**: *Ask the assistant*, the layer menu's last group
  (`menu::Group::Assistant`, `context-menus.md` §3), and the ask button on the selection. Each
  attaches the selection as **chips** above the composer, opens the panel and focuses the composer.
  The rail button, the palette's *Assistant* and `Ctrl+Shift+A` open the panel and attach nothing.
  The menu row is omitted while the assistant is *Off*, as the ask button is hidden, so someone who
  never turns it on sees neither; in private mode it is dimmed.
- **The ask button** is a small `icon::SPARKLE` button above the selection's top-right corner, its
  right edge on the selection's and its bottom 8 clear of it, one for the whole selection and the
  same size at every zoom. Where there is no room above it sits inside the corner, and it stays left
  of the inspector's cards. *View ▸ Show ask assistant* (`ViewSwitch::AskAssistant`, in the
  View menu's canvas group; per machine, on by default) hides it; the context menu's row stays. The
  canvas context menu's view rows leave it out.
- **An attached layer has its own ask button, lit**, in the same place over that layer, carrying the
  sparkle and its chip's number, whatever *Show ask assistant* says. It shows while the panel is
  open or the layer is selected, on its own page; the selection's button gives way to it once every
  selected layer is attached. A click on it detaches the layer: the button is a toggle.
- **The ask buttons, the scope button among them, hide** while the selection is dragged, resized or rotated, while text, a path
  or a picture is edited, in Present, focus and private mode, and while the assistant is *Off*. They pass the
  wheel and a pinch to the canvas beneath, as its other overlays do.
- **A chip is a reference, not a copy** (`ui::layer_chip`): the layer's id, name, kind and size go
  with the question, and the agent reads the rest through the tools, and a picture of it goes too
  (*Pictures*). A chip's × removes it; a chip whose layer is deleted shows struck through and is
  not sent.
- **While the panel is open, an attached layer wears a dashed accent outline** 5 outside it; its
  number is on its lit ask button. A closed panel leaves no outline. Hovering a chip outlines its
  layer; clicking one selects it and zooms to it.
- **The agent's reply can name layers.** A layer id in a reply the document holds renders as a
  link naming the layer, in the accent; hovering it outlines the layer and clicking it selects it
  and zooms to it, as a chip does.

### The scope

- **The person can pin the assistant to one top-level frame**: a frame whose parent is a page
  (`scope_candidate`), a main component among them. It is a hint, not a gate: the agent is told to
  work in the frame unless the question names another place, and nothing refuses a write outside
  it.
- **One scope at a time**; scoping another frame replaces it. It belongs to the document in
  front, per tab, and lives in memory only: switching tabs stashes it and brings it back, *New
  chat* keeps it, private mode keeps it, and closing the tab drops it.
- **Two doors**, neither of which attaches anything:
  - **The scope button** (`icon::SCOPE_FRAME`, `AskKind::Scope`): unlit over a selection that is
    exactly one top-level frame while *Show ask assistant* and *View ▸ Show assistant scope*
    (`ViewSwitch::AssistantScope`, last of the canvas group; per machine, on by default) are both
    on; lit over the scoped frame on its page while the panel is open or the frame is selected,
    whatever those switches say. It stands
    `SCOPE_GAP` (4) left of the ask button at that corner and level with it, or in the ask
    button's place when none stands there; where it would leave the canvas it is dropped and the
    ask button stays. Unlit, a click scopes the assistant and opens the panel with the composer
    focused (*Scope the assistant to this frame*); lit, it removes the scope (*Remove the scope*).
  - **The layer menu's *Scope the assistant here***, after *Ask the assistant* on a single
    top-level frame, reading *Remove the scope* on the scoped one (`context-menus.md` §3).
- **The composer leads its chip row with the scope** as a pill (`ui::layer_chip` with `scope`),
  parted from the chips by a `ui::rule`; `aside` names the frame's page, faint, when it is not
  the one in front. Hovering it outlines the frame; clicking it selects the frame and zooms to it
  (*The assistant works in ‹name›. Click to go to it.*); its × removes the scope. A deleted
  frame's pill is struck through and faint (*‹name› was deleted. Questions are not scoped.*) and
  is not sent. Chips leave with a question; the scope stays. The hint reads *Ask about ‹name›*,
  then *Ask a follow-up in ‹name›*.
- **Each question records its scope** (`Msg::Person::scope`, left out of the saved thread when
  there is none, so older threads load), shown in its bubble as a faint glyph and name above the
  chips. *Ask again*, *Regenerate* and an option resend in the question's own scope; a fresh
  send, a proposal's *Ask again* and the command bar's `/ai` use the current one, its name and
  size as they are now.
- **While the panel is open, the scoped frame's name tag** leads with the scope glyph and takes
  the selection's ink, a component set's tab included; a main component's chip and an instance's
  tag keep their violet, the glyph their mark. Private mode leaves it plain. It gets no outline, which would read as the
  dashed one an attached layer wears.
- **The scope's picture goes when the agent has not seen the frame as it is** (*Pictures*): on a
  session's first turn with the scope, after the scope moves to another frame, and after anything
  in the frame changes (`prompt::subtree_hash` over its layers). The MCP server knows nothing of
  the scope.

### Pictures

- **Every question pictures what it is about**: the scope when it is due (above), first, then
  each attached layer. The scoped frame also attached is pictured once, as the scope.
- **To a harness that takes images** (§2) each goes as a render at 1×, scaled down to 4096 on its
  longer side (`export_png`'s ceiling) since it is drawn on the UI thread (`chip_renders`), and the
  preamble names them in order (`pictures: one image per layer, in this order: …`), the scope
  marked as such, so the agent knows which image is which. A layer with nothing to draw is left
  out of both. A per-turn harness reads each from a file in its working folder; one that cannot
  be written refuses the turn (`Refusal::PictureNotWritten`), since the order would no longer hold.
- **To one that does not**, the preamble says no picture reaches it and describes each in a line
  (`prompt::sketch`): kind and size, its own fills, how many layers it holds by kind, and the
  first of its text, quoted, names written as the preamble writes names.
- What the agent was shown of the scope moves on only with a turn sent, and back when the turn is
  refused before the harness reads it, as the layer changes do.

## 5. Settings

An **Assistant** tab in Settings, after *Assets* (`app-canvas-chrome.md`, *Settings*), staged
and written on *Save changes* as the rest is. The rows, then an **Instructions** eyebrow over the text area. Stored in prefs (`AssistantPrefs`), per machine; none
of it is a secret.

- **Agent** — a dropdown, *Off* first. A new install starts off. An unknown saved key loads as off
  and is kept until the person picks.
- **Path** — the program's path, empty for found automatically. Settings looks it up for the agent
  and path on screen, saved or not, with its own lookup beside the panel's: a spinner after the
  label while it looks, then *Found …* in the accent ink or *Not found* in the error's. A typed
  path is probed only once it has stood unchanged for 700 ms (`TYPING_QUIET`), since the probe
  runs whatever the half-typed name names.
- **Model** — free text in a `ui::suggest_field` (*Its default* as the hint, *Suggested models*
  on the chevron): the harness's suggestions in its list and as a ghost `Tab` fills.
- **Effort** — the harness's own levels; hidden where it has none.
- **Access** — *Look*, *Propose*, *Edit*: the default for a new chat. Each cell's tooltip says
  what the level lets the agent do, in the panel's level menu's words.
- **Instructions** — free text added to every system prompt (`ui::text_area`), held at its six
  rows and scrolling past them, so the modal keeps its size.

With *Off*, a caption says *Off: nothing is started and nothing is sent…* and the rows below Agent
are disabled. Switching agent clears Path and Model and moves Effort to the nearest level the
new harness has, ties downward; each harness's last values come back within the session. A saved
change ends the session only when it changes what the agent was started with.

## 6. The panel

A panel at the rail's foot rather than a floating window, which has to be placed, ends up over the
layers being asked about, and is one more kind of surface.

- **The door** is the rail's last button, under *Insert components*, wearing `icon::SPARKLE`, History's mark
  for a step made over MCP: the same concept, an agent acting. (`CHAT` stays unused: a speech
  bubble reads as comments, a non-goal.) Its tooltip is *Assistant (Ctrl+Shift+A)*. The palette's
  *Assistant* row and the chord open it too.
- **It is a `ui::rail_popup`, taller than its siblings.** Its bottom-left is theirs
  (`MastikApp::rail_popup_anchor`); its top stops 50 below the canvas's top, leaving the top of the page in view, down to a floor that
  still holds the head, one line of thread and the composer. It is 400 wide, and its right edge
  drags between 360 and 640, the only rail panel with a drag edge, since tables and code need the
  room; the width is kept per machine. The edge lights as the layers panel's does: a hairline over
  the card's border, after the same dwell (`EdgeDwell`), the full accent while dragged.
- **It shares the place**: opening a sibling closes it, and opening it closes them. Closing keeps
  the thread, the draft and a running turn. While a turn runs out of sight the button wears an
  accent ring; a reply that arrived unseen wears an accent dot until the panel opens; the tooltip
  says which (*Assistant · reply waiting*).
- **The canvas stays live** behind it. `Escape` closes it. A panel switch, a page switch or
  `Escape` never stops a turn; a document switch, a settings change, turning the assistant off and
  private mode end the session, each with a note.
- **Private mode turns the assistant off** while it is on. The panel closes and the session ends;
  the rail button and the context menu's row are disabled with *Not available in private mode*;
  `Ctrl+Shift+A` answers on the status line; the ask buttons, the outlines and the scope's mark are
  not drawn. The thread, its chips and the scope are kept.
- **The head**: the harness's label, the seal's grade beside a lock, then the Access dropdown and
  *New chat* (a plus). *New chat* clears the thread at once, without asking, and ends a running
  turn.
- **The thread:** the person's turns right-aligned; the agent's full width, under the speaker name
  the turn was written by, in bold and kept with the turn. An empty thread shows the app's empty
  state saying how to attach layers. It scrolls with no bar.
- **A turn is never blank while it waits.** The agent's turn takes its place at the send, with
  `ui::paint_spinner`, *Starting Claude Code…* until the harness's first event, then *Thinking…*
  until the first text or tool call, and again once a tool has finished and no text has followed.
  The seconds count from the send. A harness that exits before answering leaves a note, *Claude
  Code stopped before answering (exit code 1).*, with *Ask again* and *Show details* (the
  diagnostics); a turn the harness ends as failed leaves *Claude Code ended the turn with an
  error.* the same way, whether or not it wrote a reason.
- **Replies render markdown**: paragraphs, lists, tables, code blocks with a language bar and
  *Copy*. A block in `luau` or `lua` (`markdown::is_luau`) also wears `icon::FILE_CODE`, the log's
  *Open in editor* glyph, left of *Copy*: *Open in script editor* opens it as a new unsaved script
  (`open_script_editor_with`). A loose list, its items parted by blank lines, is one list, and an ordered one is
  numbered from its first number. A reply's text is parsed once, not every frame (an egui
  `FrameCache` keyed by the text). A proposal renders as its card (§4).
- **An ` ```options ` block renders as a stack of `ui::field_button`s**; a click sends its line as
  the next question, with the same chips and scope. Only the last turn's options are live.
- **A tool call is a row**, 22 tall: its name in mono, its argument, a status dot and its result
  cut short. After a turn's third call the rest fold into *+N calls*.
- **Under each finished turn:** its time and tokens, then the share read back from the prompt
  cache where the harness reports it (`14 s · 3.1k tokens · 92% cached`; `850`, `3.1k`, `1.0M`; the
  tokens are summed over the turn's model steps, each of which reads the conversation again; cached
  input counts as input. Antigravity's `result` is a running total over the conversation, which a
  respawned process carries on, so the session keeps the totals reported so far and gives each turn
  its difference: `Harness::usage_is_running_total`), *Copy*, and on the last turn *Regenerate*
  (`icon::ARROW_CLOCKWISE`), which drops that turn and asks again. While nothing can be sent it is
  disabled, its tooltip the composer's reason, and a send that is not taken puts the turn back.
- **The context bar** (`context_bar`) is a `ui::level_bar` the composer's width, `CONTEXT_GAP`
  (6) above it so the two read as one group, the panel's 10 plus `CONTEXT_DROP` (4) above the
  bar. egui spaces an item from the next as it places it, so the gap is set before the bar is
  drawn. It shows only a share, so only once a reading has a window: not before the first
  reading, not during Claude Code's first turn (it names the window at the turn's end), and
  never for Antigravity, OpenCode, DeepSeek or Cursor, which name none; a reason in the
  composer's place takes it away too. Hidden, it takes no room. The numbers are its tooltip
  (`context_reading`, the history row's pattern: strong head, plain line, dim line): *Context
  35% full* / *90.4k of 258.4k tokens* / *What the agent's next step reads. A new chat starts
  empty.*, the last line from `ui::LEVEL_RED` *Near the model's limit: the agent may start
  forgetting the start of the chat. A new chat starts empty.* The reading lives on the live
  session (`Live::context`), so it goes when the session ends (*New chat*, a harness change, a
  level change that cannot regrant) and a resumed or reloaded thread shows no bar until its next
  call. A reading with no window keeps the window already named.
- **The composer** is a bare `ui::text_area` in a field ground with the scope and the chips above
  it and the hint and send button on its foot, all on one left edge; it grows to 15 lines, then
  scrolls.
  Enter sends, Shift+Enter breaks the line, and the send button becomes *Stop* (`icon::STOP`)
  while a turn runs. `Ctrl+↑`/`Ctrl+↓` step through earlier
  questions.
- **The command bar's `/ai` asks in this thread** (`assistant_ask_bar`, `command-mode.md` *The
  command bar*): the thread shows `/ai <request>` as the person's turn with the selection as its
  chips, while the model is sent `prompt::bar_request`: do what is asked with the tools as here,
  or, when asked for a script, answer with one ` ```luau ` block and change nothing; either way end
  on one plain sentence under 80 characters, which is all the bar shows.
  *Regenerate* and *Ask again* send the `/ai …` text again as an ordinary question.
- **When it cannot send**, the composer is replaced by the reason, and off wins over every other:
  the assistant is off (*Open settings*); private mode; the document is unsaved (*Save the document
  to ask about it.*); the program not found, naming what was looked for and where (*Look again*,
  *Set the path*); its probe not finished in time, which refuses nothing (*Look again*); the
  program not sealable (*Look again*, *Set the path*). *Look again* finds the program afresh and
  probes it again, for one installed or updated since; a program not found or a probe not finished
  is also looked for again each time the panel opens, while a probe's answer is kept.
- **Not signed in blocks nothing.** The harness's exit ends the session, and its note says to run
  the program once in a terminal to sign in, then ask again: *Ask again* starts a new session.
- **The app's own words** (a refused spawn, a stopped turn, a replay withheld) are notes beside a
  turn, never message text, so they are never sent back to the model.
- **One conversation per document**, kept per machine in `<machine data>/chats/<document id>.json`
  (by a hash of the path when the id is not a plain name), never in the `.mastik` file: a chat is the
  person's, not the work's. It is written through `atomic::write`; when the document's id or path
  changes, the old file is removed only once the new one is written. At most 100 messages, cut
  from the front to start at a question, with each *Ask again* re-pointed at its question's new
  place (one whose question was cut keeps its words and loses the button), and a turn still
  streaming is not saved. A tool row keeps its result as the row shows it (a line count, *image*,
  an error's first line), cut to 120 characters (`thread::RESULT_KEPT`); a call stopped before it
  answered saves as *(no result)*. The file also holds the proposals the kept cards show, newest
  first, each at most 64 KiB and 1 MiB in all as JSON (`thread::Saved`); a proposal past either
  bound, or whose card was cut, is left out. A file saved as a bare list of messages still loads.

## 7. Logging and privacy

- **Off sends nothing and starts no agent**, the probe included. A launch still sweeps what a
  closed Mastik's sessions left, which can run `agy mcp` to withdraw a dead session's own
  registration (its `mastik` and eight hex digits) from the person's Antigravity config, with the
  `agy` its marker recorded, else the one found now: a standing grant there is worse than one
  command. A marker whose withdrawal did not hold stays for the next launch.
- **What leaves the machine** is what the harness sends: the question, the attached layers' and
  the scope's references and pictures (or their descriptions), and whatever the agent reads
  through the tools. No document file is handed over, and
  no telemetry is sent by Mastik.
- **The agent log** (`mcp::agent_log`) is `agent_log.jsonl` in the machine data folder: one line
  `{at, tool, document, key, ok}` per granted call, refusals included, a call through `call` under
  the tool it named. It never records results. It rolls to `agent_log.jsonl.1` at 8 MiB; always on;
  never read back by the app.

## Traps

- **The level is granted twice, and both are needed.** The key grant before spawning covers the
  harness's proxy connecting before `Session::start` returns; the grant with the session's process
  owner covers a process that sends another key or none
  (`an_owned_process_gets_the_chats_level_whatever_its_key`).
- **An agent is kept off `serve` twice.** The proxy under `MASTIK_AGENT_CHILD` or a chat's key
  connects only to the window (`proxy::Machine::connect`, `pick_host`), and `main` refuses
  `mastik serve` under the mark, for an agent with a shell. Each looks redundant beside the other;
  `serve` holds no grant, so either gone lets an agent act unchecked. The key check is not
  redundant with the mark either: Codex and Cursor scrub their servers' environment, and only
  their config's `env` puts the mark back.
- **Copilot's `--available-tools` with an empty value fails open** (every tool, the shell
  included), so an empty grant names a tool nothing provides
  (`copilot_never_sends_an_empty_allowlist`).
- **Antigravity has no interrupt line**: it ignores an unknown stdin event silently, so a guessed
  interrupt would hang the turn rather than fail. It is killed and resumed with `--conversation`.
- **Antigravity's seal is `--mode plan`, not `--sandbox`.** The sandbox restricts its terminal
  only; in a folder the person trusts (their home is a common entry) its own `write_file` and
  `run_command` still act, which was measured writing a file outside the run folder. Plan mode
  refuses both and leaves Mastik's allowed tools working. Headless, a refused tool ends its turn,
  so the system prompt tells every agent to call Mastik's tools directly rather than look around.
- **A persistent process ended for a new grant still has a reader.** Its exit arrives while the
  turn that ended it is in flight, so the reader of an ended generation reports nothing, or it
  would end that turn as the harness having exited
  (`a_new_grant_restarts_claude_code_resuming_its_conversation`).
- **Cursor runs in its default mode.** `--mode ask` held back nothing the permissions file did not,
  and had the model refuse the edits a chat at *Edit* asked for.
- **DeepSeek Harness grows tool plugins.** The per-turn check refuses a profile with a tool row
  the overlay does not disable (`tool-agent-team` arrived this way); the row then joins
  `DSH_TOOL_ROWS`.
