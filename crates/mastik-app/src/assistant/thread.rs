//! One document's conversation with the assistant: its messages, and where they
//! are kept. A chat is the person's, not the work's, so it lives in the machine
//! data folder keyed by the document's id, never in the `.mastik` file.

use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

use crate::mcp::propose::Proposal;

/// The most messages a saved thread keeps; the oldest go first.
pub(crate) const KEPT: usize = 100;

/// The longest tool result a saved row keeps, in characters. A row holds its
/// result already cut short ([`ToolRow::result`]); this bounds the file
/// whatever a row is given.
pub(crate) const RESULT_KEPT: usize = 120;

/// What a saved row whose call never answered reads as: a turn stopped
/// while the call ran.
pub(crate) const NO_RESULT: &str = "(no result)";

/// The largest proposal a saved thread keeps, as JSON: `calls` can carry a
/// whole subtree. A card whose proposal was left out says so.
pub(crate) const PROPOSAL_BYTES: usize = 64 * 1024;

/// The most bytes of proposals one saved thread keeps, newest first.
pub(crate) const PROPOSALS_BYTES: usize = 1024 * 1024;

/// A layer attached to a question: a reference the agent reads through the
/// tools, never a copy.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub(crate) struct Chip {
    /// The layer's wire id (`4:31`).
    pub id: String,
    pub name: String,
    /// The node kind's word (`frame`, `text`).
    pub kind: String,
    pub w: f64,
    pub h: f64,
}

/// A tool call's state on its row.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) enum ToolState {
    Running,
    Done,
    Failed,
}

/// One tool call in an agent's turn.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub(crate) struct ToolRow {
    /// The harness's own id, which pairs the result with its call.
    pub id: String,
    /// The bare tool name.
    pub name: String,
    /// The argument it was given, in the agent's terms: a layer's id or a query.
    pub arg: String,
    pub state: ToolState,
    /// The result cut short: a line count, *image*, or the error.
    pub result: String,
    /// The proposal a `propose_edit` call recorded.
    #[serde(default)]
    pub proposal: Option<u64>,
}

/// A part of an agent's turn, in the order it arrived.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub(crate) enum Part {
    Text(String),
    Tool(ToolRow),
}

/// A finished turn's footing.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub(crate) struct Stats {
    pub seconds: f64,
    pub tokens: Option<u64>,
    /// The part of `tokens` read back from the prompt cache, when the harness
    /// says; absent in threads saved before it was kept.
    #[serde(default)]
    pub cached: Option<u64>,
}

/// The app's own words beside a turn, never sent back to the model.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub(crate) enum Note {
    /// A line of plain fact: a stopped turn, a replay withheld.
    Said(String),
    /// The harness ended before answering. `question` is the person's message it
    /// was answering, for *Ask again*; `details` the lines that were not JSON.
    Failed {
        text: String,
        question: usize,
        details: String,
    },
}

/// One message in the thread.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub(crate) enum Msg {
    Person {
        text: String,
        chips: Vec<Chip>,
        /// The frame the question was asked in, which *Ask again* asks in too.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        scope: Option<Chip>,
    },
    Agent {
        /// The speaker the turn was written by, kept with it.
        speaker: String,
        /// The harness's key, so a restored thread knows whose prose it holds.
        harness: String,
        parts: Vec<Part>,
        /// `None` while it streams, or when it was stopped.
        stats: Option<Stats>,
    },
    Note(Note),
}

impl Msg {
    /// The agent's prose in this message, for *Copy* and a replay: its text
    /// parts as the paragraphs the panel draws them as, since the text on
    /// either side of a tool call is a part of its own.
    pub(crate) fn prose(&self) -> String {
        match self {
            Msg::Agent { parts, .. } => parts
                .iter()
                .filter_map(|p| match p {
                    Part::Text(t) => Some(t.trim()).filter(|t| !t.is_empty()),
                    Part::Tool(_) => None,
                })
                .collect::<Vec<_>>()
                .join("\n\n"),
            Msg::Person { text, .. } => text.clone(),
            Msg::Note(_) => String::new(),
        }
    }
}

/// A proposal a card in the thread shows, as the thread's file keeps it.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub(crate) struct Kept {
    pub proposal: Proposal,
    /// The person applied it from its card.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub applied: bool,
    /// The history depth of its applied step, which orders it among the cards
    /// of its title when the thread is loaded again.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub depth: Option<usize>,
}

/// A thread as its file holds it: the messages, and the proposals their
/// cards show.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub(crate) struct Saved {
    pub msgs: Vec<Msg>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub proposals: Vec<Kept>,
}

/// A file in either form: the thread with its proposals, or the bare list of
/// messages a thread was saved as before proposals were kept.
#[derive(Deserialize)]
#[serde(untagged)]
enum OnDisk {
    Saved(Saved),
    Msgs(Vec<Msg>),
}

/// `text` cut to `n` characters, with an ellipsis when cut.
fn cut(text: &str, n: usize) -> String {
    match text.char_indices().nth(n) {
        Some((i, _)) => format!("{}…", &text[..i]),
        None => text.to_owned(),
    }
}

/// The thread as saved: at most [`KEPT`] messages, nothing after a turn still
/// streaming, each tool result within [`RESULT_KEPT`], and the proposals the
/// kept cards show, newest first within [`PROPOSAL_BYTES`] each and
/// [`PROPOSALS_BYTES`] in all. `applied` answers for each card the person
/// applied, with its step's history depth when that step is known.
pub(crate) fn for_saving(
    msgs: &[Msg],
    streaming: Option<usize>,
    proposals: &[Proposal],
    applied: &dyn Fn(u64) -> Option<Option<usize>>,
) -> Saved {
    let end = streaming.map_or(msgs.len(), |i| i.min(msgs.len()));
    let mut kept: Vec<Msg> = msgs[..end]
        .iter()
        .map(|m| match m {
            Msg::Agent {
                speaker,
                harness,
                parts,
                stats,
            } => Msg::Agent {
                speaker: speaker.clone(),
                harness: harness.clone(),
                parts: parts
                    .iter()
                    .map(|p| match p {
                        // A call stopped before it answered has no result.
                        Part::Tool(t) if t.state == ToolState::Running => Part::Tool(ToolRow {
                            result: NO_RESULT.into(),
                            state: ToolState::Done,
                            ..t.clone()
                        }),
                        Part::Tool(t) => Part::Tool(ToolRow {
                            result: cut(&t.result, RESULT_KEPT),
                            ..t.clone()
                        }),
                        p => p.clone(),
                    })
                    .collect(),
                stats: *stats,
            },
            m => m.clone(),
        })
        .collect();
    let mut dropped = kept.len().saturating_sub(KEPT);
    // A thread opening on an answer or a note has lost its question; start at a
    // question.
    while kept
        .get(dropped)
        .is_some_and(|m| !matches!(m, Msg::Person { .. }))
    {
        dropped += 1;
    }
    kept.drain(..dropped);
    // *Ask again* names its question by index, which moved with the cut; one
    // whose question went keeps its words and loses the button.
    for m in &mut kept {
        if let Msg::Note(Note::Failed { text, question, .. }) = m {
            match question.checked_sub(dropped) {
                Some(q) => *question = q,
                None => *m = Msg::Note(Note::Said(std::mem::take(text))),
            }
        }
    }
    let shown: Vec<u64> = kept
        .iter()
        .flat_map(|m| match m {
            Msg::Agent { parts, .. } => parts.as_slice(),
            _ => &[],
        })
        .filter_map(|p| match p {
            Part::Tool(t) => t.proposal,
            Part::Text(_) => None,
        })
        .collect();
    let mut room = PROPOSALS_BYTES;
    let mut cards: Vec<Kept> = Vec::new();
    for id in shown.iter().rev() {
        let Some(p) = proposals.iter().find(|p| p.id == *id) else {
            continue;
        };
        if cards.iter().any(|k| k.proposal.id == *id) {
            continue;
        }
        let step = applied(*id);
        let card = Kept {
            proposal: p.clone(),
            applied: step.is_some(),
            depth: step.flatten(),
        };
        let size = serde_json::to_vec(&card).map_or(usize::MAX, |b| b.len());
        if size <= PROPOSAL_BYTES && size <= room {
            room -= size;
            cards.push(card);
        }
    }
    cards.reverse();
    Saved {
        msgs: kept,
        proposals: cards,
    }
}

/// The folder the chats live in.
fn dir() -> Option<PathBuf> {
    Some(crate::machine_dir::root()?.join("chats"))
}

/// The file a document's chat is kept in: by its library id, which survives a
/// rename, else by a hash of its path.
pub(crate) fn file_for(doc_id: Option<&str>, path: Option<&Path>) -> Option<PathBuf> {
    let name = match (doc_id, path) {
        (Some(id), _) if is_plain(id) => id.to_owned(),
        (_, Some(p)) => format!("p{:016x}", path_hash(p)),
        _ => return None,
    };
    Some(dir()?.join(format!("{name}.json")))
}

/// An id safe as a file name.
fn is_plain(id: &str) -> bool {
    !id.is_empty()
        && id.len() <= 64
        && id
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
}

/// FNV-1a over the path's text: stable across runs, unlike `DefaultHasher`.
fn path_hash(p: &Path) -> u64 {
    let mut h: u64 = 0xcbf2_9ce4_8422_2325;
    for b in p.to_string_lossy().as_bytes() {
        h ^= u64::from(*b);
        h = h.wrapping_mul(0x0100_0000_01b3);
    }
    h
}

/// Read a saved chat; an unreadable one reads as empty.
pub(crate) fn load(file: &Path) -> Saved {
    let read = std::fs::read(file)
        .ok()
        .and_then(|b| serde_json::from_slice::<OnDisk>(&b).ok());
    match read {
        Some(OnDisk::Saved(s)) => s,
        Some(OnDisk::Msgs(msgs)) => Saved {
            msgs,
            proposals: Vec::new(),
        },
        None => Saved::default(),
    }
}

/// Write a chat through the atomic writer, or remove its file when it is empty.
pub(crate) fn save(file: &Path, saved: &Saved) -> std::io::Result<()> {
    if saved.msgs.is_empty() {
        return match std::fs::remove_file(file) {
            Err(e) if e.kind() != std::io::ErrorKind::NotFound => Err(e),
            _ => Ok(()),
        };
    }
    let bytes = serde_json::to_vec(saved).map_err(std::io::Error::other)?;
    if let Some(parent) = file.parent() {
        std::fs::create_dir_all(parent)?;
    }
    crate::atomic::write(file, &bytes)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn agent(text: &str, tool: Option<&str>) -> Msg {
        let mut parts = vec![Part::Text(text.into())];
        if let Some(name) = tool {
            parts.push(Part::Tool(ToolRow {
                id: "t1".into(),
                name: name.into(),
                arg: "4:31".into(),
                state: ToolState::Done,
                result: "+24 lines".into(),
                proposal: None,
            }));
        }
        Msg::Agent {
            speaker: "Claude".into(),
            harness: "claude".into(),
            parts,
            stats: None,
        }
    }

    fn person(text: &str) -> Msg {
        Msg::Person {
            text: text.into(),
            chips: Vec::new(),
            scope: None,
        }
    }

    /// The text on either side of a tool call reads as two paragraphs.
    /// flip: `prose` joining with `""` → fails at the `assert_eq`.
    #[test]
    fn prose_parts_are_paragraphs() {
        let Msg::Agent { mut parts, .. } = agent("Let me look.", Some("get_node")) else {
            unreachable!()
        };
        parts.push(Part::Text("The frame is Fixed.\n".into()));
        let m = Msg::Agent {
            speaker: "Claude".into(),
            harness: "claude".into(),
            parts,
            stats: None,
        };
        assert_eq!(m.prose(), "Let me look.\n\nThe frame is Fixed.");
    }

    /// `for_saving` with no proposals.
    fn saving(msgs: &[Msg], streaming: Option<usize>) -> Vec<Msg> {
        for_saving(msgs, streaming, &[], &|_| None).msgs
    }

    /// The tool row of agent message `m` at part `i`.
    fn row_at(msgs: &[Msg], m: usize, i: usize) -> &ToolRow {
        let Msg::Agent { parts, .. } = &msgs[m] else {
            panic!("not an agent turn");
        };
        let Part::Tool(t) = &parts[i] else {
            panic!("not a tool row");
        };
        t
    }

    /// A saved thread keeps each call's result as its row shows it, cut to the
    /// bound, and a call stopped before it answered reads as having none.
    /// flip: `for_saving` keeping `t.result` uncut → fails at "cut to the bound".
    #[test]
    fn tool_results_are_saved_within_their_bound() {
        let mut long = agent("a", Some("get_node"));
        if let Msg::Agent { parts, .. } = &mut long
            && let Some(Part::Tool(t)) = parts.last_mut()
        {
            t.result = "x".repeat(RESULT_KEPT * 3);
        }
        let mut stopped = agent("b", Some("get_view"));
        if let Msg::Agent { parts, .. } = &mut stopped
            && let Some(Part::Tool(t)) = parts.last_mut()
        {
            t.state = ToolState::Running;
            t.result = String::new();
        }
        let msgs = [
            person("q"),
            agent("a", Some("get_node")),
            person("r"),
            long,
            person("s"),
            stopped,
        ];
        let saved = saving(&msgs, None);
        assert_eq!(row_at(&saved, 1, 1).result, "+24 lines", "kept as shown");
        assert_eq!(
            row_at(&saved, 3, 1).result.chars().count(),
            RESULT_KEPT + 1,
            "cut to the bound"
        );
        let t = row_at(&saved, 5, 1);
        assert_eq!((t.state, t.result.as_str()), (ToolState::Done, NO_RESULT));
    }

    /// A proposal for a test: `id`, titled `title`, its calls `bytes` long.
    fn proposal(id: u64, title: &str, bytes: usize) -> Proposal {
        Proposal {
            id,
            key: Some("assistant-1-1".into()),
            document: PathBuf::from("a.mastik"),
            title: title.into(),
            changes: vec!["set_name · Hero".into()],
            calls: vec![(
                "set_name".into(),
                serde_json::json!({ "node": "1:2", "name": "x".repeat(bytes) }),
            )],
            rev: 3,
            writes: Vec::new(),
            before: Vec::new(),
            client: "Claude".into(),
            held: Vec::new(),
            labels: Vec::new(),
        }
    }

    /// An agent turn whose one call proposed `id`.
    fn proposed(id: u64) -> Msg {
        let mut m = agent("Proposed.", Some("propose_edit"));
        if let Msg::Agent { parts, .. } = &mut m
            && let Some(Part::Tool(t)) = parts.last_mut()
        {
            t.proposal = Some(id);
        }
        m
    }

    /// A saved thread keeps the proposals its kept cards show, each with
    /// whether it was applied, and reads back as it was written; one over its
    /// own bound, one whose card was cut and one no card shows are left out.
    ///
    /// flip: `for_saving` without the `PROPOSAL_BYTES` check → fails at "too
    /// large"; keeping every proposal given → fails at "no card shows it".
    #[test]
    fn proposals_are_saved_with_their_cards_and_load_back() {
        let msgs = vec![
            person("q"),
            proposed(1),
            person("r"),
            proposed(2),
            person("s"),
            proposed(3),
        ];
        let given = [
            proposal(1, "Rename", 10),
            proposal(2, "Huge", PROPOSAL_BYTES),
            proposal(3, "Fade", 10),
            proposal(4, "Elsewhere", 10),
        ];
        let saved = for_saving(&msgs, None, &given, &|id| (id == 1).then_some(Some(4)));
        let ids: Vec<(u64, bool)> = saved
            .proposals
            .iter()
            .map(|k| (k.proposal.id, k.applied))
            .collect();
        assert_eq!(ids, [(1, true), (3, false)], "too large; no card shows it");

        let dir =
            std::env::temp_dir().join(format!("mastik-thread-{}", crate::session::random_actor()));
        let file = dir.join("t.json");
        save(&file, &saved).expect("saved");
        assert_eq!(load(&file), saved, "reads back as written");
        let _ = std::fs::remove_dir_all(&dir);

        // Past the message cap the first card goes, and its proposal with it.
        let mut many = vec![person("q"), proposed(1)];
        for i in 0..KEPT {
            many.push(person(&format!("q{i}")));
            many.push(agent("a", None));
        }
        let cut = for_saving(&many, None, &given, &|_| None);
        assert!(cut.proposals.is_empty(), "its card was cut");
    }

    /// A thread saved as a bare list of messages, before proposals were kept,
    /// still loads.
    /// flip: `load` dropping a list's messages → fails at the message.
    #[test]
    fn a_thread_saved_as_a_list_loads() {
        let dir = std::env::temp_dir().join(format!(
            "mastik-thread-old-{}",
            crate::session::random_actor()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        let file = dir.join("t.json");
        std::fs::write(&file, r#"[{"Person":{"text":"q","chips":[]}}]"#).unwrap();
        let loaded = load(&file);
        assert_eq!(loaded.msgs, vec![person("q")]);
        assert!(loaded.proposals.is_empty());
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// The oldest messages go past the cap, and the thread starts at a question.
    /// flip: dropping the `while` → fails at the first message.
    #[test]
    fn the_cap_drops_the_oldest_and_starts_on_a_question() {
        let mut msgs = Vec::new();
        for i in 0..60 {
            msgs.push(person(&format!("q{i}")));
            msgs.push(agent("a", None));
        }
        msgs.insert(1, Msg::Note(Note::Said("x".into())));
        let saved = saving(&msgs, None);
        assert!(saved.len() <= KEPT);
        assert!(matches!(saved[0], Msg::Person { .. }));
    }

    /// Past the cap, a failure note still names its own question.
    /// flip: `for_saving` not shifting `question` → fails at the question's text.
    #[test]
    fn the_cap_keeps_ask_again_on_its_question() {
        let mut msgs = Vec::new();
        for i in 0..70 {
            msgs.push(person(&format!("q{i}")));
            msgs.push(agent("a", None));
        }
        let asked = msgs.len();
        msgs.push(person("last"));
        msgs.push(Msg::Note(Note::Failed {
            text: "stopped".into(),
            question: asked,
            details: String::new(),
        }));
        let saved = saving(&msgs, None);
        let Some(Msg::Note(Note::Failed { question, .. })) = saved.last() else {
            panic!("the note went");
        };
        assert_eq!(saved[*question], person("last"));
    }

    /// A turn still streaming is not saved, nor anything after it.
    #[test]
    fn a_streaming_turn_is_not_saved() {
        let saved = saving(&[person("q"), agent("half", None)], Some(1));
        assert_eq!(saved, vec![person("q")]);
    }

    /// A thread saved before questions carried a scope still loads, and a
    /// question with none saves without the field. The load holds without
    /// `serde(default)` too, since serde reads a missing `Option` as `None`.
    /// flip: `scope` without `skip_serializing_if` → fails at the saved text.
    #[test]
    fn a_thread_without_scopes_loads() {
        let old = r#"[{"Person":{"text":"q","chips":[]}}]"#;
        let msgs: Vec<Msg> = serde_json::from_str(old).expect("loads");
        assert_eq!(msgs, vec![person("q")]);
        let saved = serde_json::to_string(&msgs).expect("saves");
        assert!(!saved.contains("scope"), "{saved}");
    }

    /// A document's chat file is named by its id, else by its path.
    #[test]
    fn the_file_is_named_by_id_then_path() {
        let by_id = file_for(Some("abc-123"), Some(Path::new("a.mastik")));
        let by_path = file_for(None, Some(Path::new("a.mastik")));
        let odd = file_for(Some("../x"), Some(Path::new("a.mastik")));
        if let (Some(a), Some(b), Some(c)) = (by_id, by_path, odd) {
            assert!(a.ends_with("abc-123.json"));
            assert_eq!(b, c, "an id unsafe as a name falls back to the path");
        }
        assert!(file_for(None, None).is_none());
    }
}
