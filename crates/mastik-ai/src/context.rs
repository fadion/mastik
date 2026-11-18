//! How full a conversation's context is, read where each harness says it.
//!
//! The figure is the prompt of the latest model call: what the next call
//! starts from. A turn's usage summed over its calls counts the same context
//! once per call, so it is never used. Claude Code, OpenCode and DeepSeek say
//! it in their stream (`stream.rs`); Codex, Copilot and Antigravity only in a
//! log they write beside it, read here once a turn is over. Cursor says it only
//! for a turn of one call. Only Claude Code, Codex and Copilot name the window.

use crate::harness::Harness;
use std::io::{Read as _, Seek as _, SeekFrom};
use std::path::{Path, PathBuf};

/// How full the context is after a model call.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Reading {
    /// Tokens in the prompt of the latest call, cached ones included.
    pub used: u64,
    /// The most the model takes, where the harness says.
    pub window: Option<u64>,
}

/// The tail of a log that is read: the latest record is near the end, and a
/// long session's log runs to megabytes.
const TAIL: u64 = 512 * 1024;

/// The reading in `harness`'s own log for conversation `id`, for the harnesses
/// that write one; `None` for the rest, or when the log has none yet.
pub(crate) fn from_log(
    harness: Harness,
    id: &str,
    env: &[(String, std::ffi::OsString)],
) -> Option<Reading> {
    let path = log_path(harness, id, env)?;
    let text = tail(&path)?;
    match harness {
        Harness::Codex => codex(&text),
        Harness::Copilot => copilot(&text),
        Harness::Antigravity => antigravity(&text),
        Harness::Claude | Harness::OpenCode | Harness::Cursor | Harness::DeepSeek => None,
    }
}

/// Where `harness` logs conversation `id`. Copilot's home is the session's own
/// (`COPILOT_HOME` in `env`); Codex's and Antigravity's are the person's.
fn log_path(harness: Harness, id: &str, env: &[(String, std::ffi::OsString)]) -> Option<PathBuf> {
    // An id is a file-name part: never one that could climb out of the folder.
    if id.is_empty() || id.contains(['/', '\\', '.']) {
        return None;
    }
    match harness {
        Harness::Codex => codex_rollout(&user_home("CODEX_HOME", ".codex")?.join("sessions"), id),
        Harness::Copilot => {
            let home = env
                .iter()
                .find(|(k, _)| k == "COPILOT_HOME")
                .map(|(_, v)| PathBuf::from(v))
                .or_else(|| user_home("COPILOT_HOME", ".copilot"))?;
            Some(home.join("session-state").join(id).join("events.jsonl"))
        }
        Harness::Antigravity => Some(
            crate::find::home()?
                .join(".gemini")
                .join("antigravity-cli")
                .join("brain")
                .join(id)
                .join(".system_generated")
                .join("logs")
                .join("transcript_full.jsonl"),
        ),
        Harness::Claude | Harness::OpenCode | Harness::Cursor | Harness::DeepSeek => None,
    }
}

/// A CLI's home: its variable when set, else `<home>/<dot>`.
fn user_home(var: &str, dot: &str) -> Option<PathBuf> {
    match std::env::var_os(var).filter(|v| !v.is_empty()) {
        Some(d) => Some(PathBuf::from(d)),
        None => Some(crate::find::home()?.join(dot)),
    }
}

/// Codex files a thread under the day it started, `YYYY/MM/DD/rollout-<time>-<id>.jsonl`,
/// and a resumed turn appends to it: the days are searched newest first.
fn codex_rollout(sessions: &Path, id: &str) -> Option<PathBuf> {
    let suffix = format!("-{id}.jsonl");
    let newest_first = |p: &Path| -> Vec<PathBuf> {
        let mut v: Vec<PathBuf> = std::fs::read_dir(p)
            .into_iter()
            .flatten()
            .flatten()
            .map(|e| e.path())
            .collect();
        v.sort_unstable_by(|a, b| b.cmp(a));
        v
    };
    for year in newest_first(sessions).iter().filter(|p| p.is_dir()) {
        for month in newest_first(year).iter().filter(|p| p.is_dir()) {
            for day in newest_first(month).iter().filter(|p| p.is_dir()) {
                if let Some(f) = newest_first(day).into_iter().find(|f| {
                    f.file_name()
                        .and_then(|n| n.to_str())
                        .is_some_and(|n| n.starts_with("rollout-") && n.ends_with(&suffix))
                }) {
                    return Some(f);
                }
            }
        }
    }
    None
}

/// The last [`TAIL`] bytes of `path` as text, the first, cut line dropped.
fn tail(path: &Path) -> Option<String> {
    let mut f = std::fs::File::open(path).ok()?;
    let len = f.metadata().ok()?.len();
    let from = len.saturating_sub(TAIL);
    f.seek(SeekFrom::Start(from)).ok()?;
    let mut bytes = Vec::new();
    f.read_to_end(&mut bytes).ok()?;
    let text = String::from_utf8_lossy(&bytes).into_owned();
    Some(match from {
        0 => text,
        _ => text
            .split_once('\n')
            .map_or(String::new(), |(_, rest)| rest.to_string()),
    })
}

/// The last line of `text` that `read` makes a reading of.
fn last(text: &str, read: impl Fn(&serde_json::Value) -> Option<Reading>) -> Option<Reading> {
    text.lines()
        .rev()
        .filter_map(|l| serde_json::from_str::<serde_json::Value>(l.trim()).ok())
        .find_map(|v| read(&v))
}

/// A Codex rollout's latest `token_count`: `last_token_usage` is the latest
/// call alone (`total_token_usage` is the thread's sum), and its input
/// includes the cached part.
pub(crate) fn codex(text: &str) -> Option<Reading> {
    last(text, |v| {
        let p = v.get("payload")?;
        (p.get("type")?.as_str()? == "token_count").then_some(())?;
        let info = p.get("info")?;
        Some(Reading {
            used: info.pointer("/last_token_usage/input_tokens")?.as_u64()?,
            window: info.get("model_context_window").and_then(|w| w.as_u64()),
        })
    })
}

/// A Copilot session's latest `session.usage_record`, one per model call; its
/// input includes the cached part, and `maxPromptTokens` is the prompt's limit.
pub(crate) fn copilot(text: &str) -> Option<Reading> {
    last(text, |v| {
        (v.get("type")?.as_str()? == "session.usage_record").then_some(())?;
        let u = v.pointer("/data/usage")?;
        Some(Reading {
            used: u.get("inputTokens")?.as_u64()?,
            window: u.get("maxPromptTokens").and_then(|w| w.as_u64()),
        })
    })
}

/// An Antigravity transcript's latest model step; its input leaves out what
/// was read from the cache. It names no window.
pub(crate) fn antigravity(text: &str) -> Option<Reading> {
    last(text, |v| {
        (v.get("source")?.as_str()? == "MODEL").then_some(())?;
        let at = |k: &str| v.get(k).and_then(|n| n.as_u64());
        Some(Reading {
            used: crate::stream::sum_some([at("input_tokens"), at("cache_read_tokens")])?,
            window: None,
        })
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Two of a captured Codex rollout's `token_count`s: three calls, then a
    /// resumed turn of one. Trimmed to the fields read.
    const CODEX: &str = concat!(
        r#"{"type":"event_msg","payload":{"type":"token_count","info":null}}"#,
        "\n",
        r#"{"type":"event_msg","payload":{"type":"token_count","info":{"total_token_usage":{"input_tokens":27379},"last_token_usage":{"input_tokens":13984,"cached_input_tokens":13056},"model_context_window":258400}}}"#,
        "\n",
        r#"{"type":"response_item","payload":{"type":"message"}}"#,
        "\n",
    );

    /// Codex's reading is the latest `token_count`'s last call, not the thread's total.
    /// flip: reading `/total_token_usage/input_tokens` → fails at the assertion.
    #[test]
    fn codex_reads_the_last_calls_usage() {
        assert_eq!(
            codex(CODEX),
            Some(Reading {
                used: 13984,
                window: Some(258_400)
            })
        );
        assert_eq!(
            codex(r#"{"payload":{"type":"token_count","info":null}}"#),
            None
        );
    }

    /// Copilot's reading is its latest usage record, window and all.
    #[test]
    fn copilot_reads_the_latest_usage_record() {
        let text = concat!(
            r#"{"type":"session.usage_record","data":{"usage":{"inputTokens":15055,"cacheReadTokens":14891,"maxPromptTokens":272000}}}"#,
            "\n",
            r#"{"type":"session.usage_record","data":{"usage":{"inputTokens":17337,"cacheReadTokens":9728,"maxPromptTokens":922000}}}"#,
            "\n",
            r#"{"type":"session.shutdown","data":{}}"#,
        );
        assert_eq!(
            copilot(text),
            Some(Reading {
                used: 17337,
                window: Some(922_000)
            })
        );
    }

    /// Antigravity's reading adds the cache to its input, from the latest model step.
    #[test]
    fn antigravity_adds_the_cache_to_the_latest_step() {
        let text = concat!(
            r#"{"step_index":5,"source":"MODEL","type":"PLANNER_RESPONSE","input_tokens":2268,"cache_read_tokens":8393}"#,
            "\n",
            r#"{"step_index":7,"source":"MODEL","type":"PLANNER_RESPONSE","input_tokens":2554,"cache_read_tokens":8646}"#,
            "\n",
            r#"{"step_index":8,"source":"USER","type":"USER_INPUT"}"#,
        );
        assert_eq!(
            antigravity(text),
            Some(Reading {
                used: 2554 + 8646,
                window: None
            })
        );
    }

    /// A Codex thread is found in the day it started, whichever day that was,
    /// and only by its own id.
    #[test]
    fn a_codex_rollout_is_found_by_its_thread() {
        let root =
            std::env::temp_dir().join(format!("mastik-ai-codex-{}", crate::run::random_tag()));
        let old = root.join("2026").join("10").join("02");
        let new = root.join("2026").join("11").join("18");
        std::fs::create_dir_all(&old).unwrap();
        std::fs::create_dir_all(&new).unwrap();
        let mine = old.join("rollout-2026-10-02T09-00-00-abc-123.jsonl");
        std::fs::write(&mine, CODEX).unwrap();
        std::fs::write(new.join("rollout-2026-11-18T09-00-00-def-456.jsonl"), "").unwrap();
        assert_eq!(codex_rollout(&root, "abc-123"), Some(mine.clone()));
        assert_eq!(codex_rollout(&root, "zzz"), None);
        let _ = std::fs::remove_dir_all(&root);
    }

    /// Copilot's log is under the session's own home, and an id that could
    /// climb out of its folder is never joined.
    #[test]
    fn copilot_reads_under_the_sessions_home_and_refuses_a_climbing_id() {
        let home =
            std::env::temp_dir().join(format!("mastik-ai-copilot-{}", crate::run::random_tag()));
        let dir = home.join("session-state").join("s-1");
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(
            dir.join("events.jsonl"),
            r#"{"type":"session.usage_record","data":{"usage":{"inputTokens":900,"maxPromptTokens":1000}}}"#,
        )
        .unwrap();
        let env = vec![("COPILOT_HOME".to_string(), home.clone().into_os_string())];
        assert_eq!(
            from_log(Harness::Copilot, "s-1", &env),
            Some(Reading {
                used: 900,
                window: Some(1000)
            })
        );
        assert_eq!(log_path(Harness::Copilot, "../s-1", &env), None);
        assert_eq!(log_path(Harness::Claude, "s-1", &env), None);
        let _ = std::fs::remove_dir_all(&home);
    }
}
