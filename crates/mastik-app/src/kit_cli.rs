//! `mastik kit update` — the MCP's `accept_kit_updates` over many documents at
//! once, against the library's kits or a folder of them, so a kits repository
//! can bring its consumers up to date from CI.
//!
//! Every waiting kit's update is taken (or one kit's, `--kit`), through the
//! same transaction the app's *Accept all* commits, and each document is written
//! atomically over itself. A *Later* told in the app is a person's, on one
//! machine, and does not hold the command back.

use crate::library::state::Library;
use crate::mcp::kits::{Accepted, not_accepted_words, prepare_accept, real_name};
use crate::session::EditorSession;

/// What `mastik kit` takes, for its refusals and `help kit`.
pub(crate) const USAGE: &str = "usage: mastik kit update <file.mastik>... [--kit <name|id>] \
                                [--library <folder>] [--page <name|n>] [--dry-run]";

/// A parsed `mastik kit update` command line.
#[derive(Debug, PartialEq)]
pub(crate) struct Update {
    /// The documents, in the order given; at least one.
    pub files: Vec<String>,
    /// `--kit`: only this kit's update, by name or id.
    pub kit: Option<String>,
    /// `--library`: the folder the kits are found in, instead of the app's library.
    pub library: Option<String>,
    /// `--page`: where mains made local are stacked; the first page that is not a draft else.
    pub page: Option<String>,
    /// `--dry-run`: say what would be taken, write nothing, and fail when anything waits.
    pub dry_run: bool,
}

/// `mastik kit update <files…> [options]`, `args` starting at `update`.
pub(crate) fn parse(args: &[String]) -> Result<Update, String> {
    if args.first().map(String::as_str) != Some("update") {
        return Err(USAGE.to_string());
    }
    let mut update = Update {
        files: Vec::new(),
        kit: None,
        library: None,
        page: None,
        dry_run: false,
    };
    let mut i = 1;
    while i < args.len() {
        match args[i].as_str() {
            "--dry-run" => update.dry_run = true,
            flag @ ("--kit" | "--library" | "--page") => {
                i += 1;
                let value = args
                    .get(i)
                    .filter(|v| !v.starts_with('-'))
                    .ok_or_else(|| format!("{flag} wants a value\n{USAGE}"))?
                    .clone();
                let slot = match flag {
                    "--kit" => &mut update.kit,
                    "--library" => &mut update.library,
                    _ => &mut update.page,
                };
                if slot.replace(value).is_some() {
                    return Err(format!("{flag} once\n{USAGE}"));
                }
            }
            other if other.starts_with('-') => {
                return Err(format!("unknown option {other}\n{USAGE}"));
            }
            other => update.files.push(other.to_string()),
        }
        i += 1;
    }
    if update.files.is_empty() {
        return Err(USAGE.to_string());
    }
    Ok(update)
}

/// Run `update` over each document, a line each, and answer the exit code: 0
/// when every document is up to date or was updated, 1 when an update was
/// refused (or, in a dry run, when one waits), 2 when a document or the library
/// cannot be read.
pub(crate) fn run(update: &Update) -> i32 {
    let root = match &update.library {
        Some(folder) => Some(std::path::PathBuf::from(folder)),
        None => crate::library::root(crate::prefs::Prefs::load().base_folder.as_deref()),
    };
    let Some(root) = root.filter(|r| crate::library::scan::readable(r)) else {
        eprintln!("cannot read the library folder; name one with --library");
        return 2;
    };
    let lib = Library::open(root);
    let mut worst = 0;
    for file in &update.files {
        let code = match one(update, &lib, file) {
            Ok((line, code)) => {
                println!("{file}: {line}");
                code
            }
            Err(msg) => {
                eprintln!("{file}: {msg}");
                2
            }
        };
        worst = worst.max(code);
    }
    let _ = std::io::Write::flush(&mut std::io::stdout());
    worst
}

/// One document: its line and its exit code, or why it could not be read.
fn one(update: &Update, lib: &Library, file: &str) -> Result<(String, i32), String> {
    let bytes = std::fs::read(file).map_err(|e| format!("cannot read it: {e}"))?;
    let doc = mastik_core::io::load(&bytes).map_err(|e| format!("cannot load it: {e}"))?;
    let mut s = EditorSession::new();
    s.adopt_document(doc, None);
    s.page = crate::chosen_page(&s.doc, update.page.as_deref())?;
    let only = match &update.kit {
        Some(wanted) => Some(find(lib, &s, wanted)?),
        None => None,
    };
    // The versions held, read before the update moves them.
    let held = crate::kits::updates::waiting(&s.doc, lib, false);
    let accepted = match prepare_accept(&mut s, lib, only.as_deref()) {
        Ok(Some(a)) => a,
        Ok(None) => return Ok(("up to date".into(), 0)),
        Err(why) => return Ok((not_accepted_words(why, "leaving out --kit"), 1)),
    };
    let taken = taken(&accepted, &held);
    if update.dry_run {
        return Ok((format!("would take {taken}"), 1));
    }
    s.try_commit(accepted.tx)
        .map_err(|e| format!("cannot apply the update: {e:?}"))?;
    let saved = mastik_core::io::save(&s.doc).map_err(|e| format!("cannot save: {e}"))?;
    crate::atomic::write(std::path::Path::new(file), &saved)
        .map_err(|e| format!("cannot write it: {e}"))?;
    Ok((format!("took {taken}; wrote it"), 0))
}

/// The kit `wanted` names among those the library holds, by id or by its
/// file's name, ignoring case.
fn find(lib: &Library, s: &EditorSession, wanted: &str) -> Result<String, String> {
    let kits = crate::kits::index::kits_in(lib, s.doc.meta().id.as_deref());
    if let Some(k) = kits.iter().find(|k| k.id == wanted) {
        return Ok(k.id.clone());
    }
    let named: Vec<_> = kits
        .iter()
        .filter(|k| real_name(lib, k).eq_ignore_ascii_case(wanted.trim()))
        .collect();
    match named.as_slice() {
        [one] => Ok(one.id.clone()),
        [] => Err(format!("no kit {wanted:?} in {}", lib.root.display())),
        many => Err(format!(
            "several kits are called {wanted:?}; name one by id: {}",
            many.iter()
                .map(|k| k.id.as_str())
                .collect::<Vec<_>>()
                .join(", ")
        )),
    }
}

/// *Atlas v3 → v4, Icons v1 → v2 (5 changed, 1 removed, 1 made local)*.
fn taken(a: &Accepted, held: &[crate::kits::updates::Waiting]) -> String {
    let kits: Vec<String> = a
        .kits
        .iter()
        .map(
            |(id, name, to)| match held.iter().find(|w| &w.kit.id == id) {
                Some(w) => format!("{name} v{} → v{to}", w.held),
                None => format!("{name} v{to}"),
            },
        )
        .collect();
    let mut counts = vec![format!("{} changed", a.changed.len())];
    if !a.removed.is_empty() {
        counts.push(format!("{} removed", a.removed.len()));
    }
    if !a.made_local.is_empty() {
        counts.push(format!("{} made local", a.made_local.len()));
    }
    format!("{} ({})", kits.join(", "), counts.join(", "))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn kit(s: &[&str]) -> Result<Update, String> {
        parse(&s.iter().map(|a| a.to_string()).collect::<Vec<_>>())
    }

    /// The command line takes documents and each option once.
    #[test]
    fn kit_update_takes_documents_and_options() {
        let u = kit(&[
            "update",
            "a.mastik",
            "b.mastik",
            "--kit",
            "Atlas",
            "--dry-run",
        ])
        .unwrap();
        assert_eq!(u.files, ["a.mastik", "b.mastik"]);
        assert_eq!(u.kit.as_deref(), Some("Atlas"));
        assert!(u.dry_run);
        assert!(kit(&["update"]).is_err(), "no document");
        assert!(kit(&["publish", "a.mastik"]).is_err(), "only update");
        assert!(kit(&["update", "a.mastik", "--kit"]).is_err(), "a value");
        assert!(kit(&["update", "a.mastik", "--page", "1", "--page", "2"]).is_err());
        assert!(kit(&["update", "a.mastik", "--force"]).is_err());
    }
}
