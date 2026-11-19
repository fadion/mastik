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
/// refused or a kit could not be checked (or, in a dry run, when one waits), 2
/// when a document or the library cannot be read.
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

/// One document: its line and its exit code, or why it could not be read. A
/// kit the document draws from that the library lacks makes the code 1, so a
/// wrong `--library` cannot pass for up to date.
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
    // `find` has already refused a `--kit` the library lacks.
    let unchecked = match only {
        Some(_) => None,
        None => unchecked(&s.doc, lib),
    };
    // The versions held, read before the update moves them.
    let held = crate::kits::updates::waiting(&s.doc, lib, false);
    let (line, code) = match prepare_accept(&mut s, lib, only.as_deref()) {
        Ok(None) => {
            return Ok(match unchecked {
                Some(u) => (u, 1),
                None => ("up to date".into(), 0),
            });
        }
        Err(why) => (not_accepted_words(why, "leaving out --kit"), 1),
        Ok(Some(mut accepted)) => {
            let before = tokens(&s.doc);
            // Committed in memory for a dry run too, to count what it changes.
            s.try_commit(std::mem::take(&mut accepted.tx))
                .map_err(|e| format!("cannot apply the update: {e:?}"))?;
            let taken = taken(&accepted, &held, &before, &tokens(&s.doc));
            if update.dry_run {
                (format!("would take {taken}"), 1)
            } else {
                let saved =
                    mastik_core::io::save(&s.doc).map_err(|e| format!("cannot save: {e}"))?;
                crate::atomic::write(std::path::Path::new(file), &saved)
                    .map_err(|e| format!("cannot write it: {e}"))?;
                (format!("took {taken}; wrote it"), 0)
            }
        }
    };
    Ok(match unchecked {
        Some(u) => (format!("{line}; {u}"), code.max(1)),
        None => (line, code),
    })
}

/// The kits `doc` draws from (its vault's mains, its variables and styles) that
/// the library does not hold at the version held or newer, as one clause;
/// `None` when it holds every one.
fn unchecked(doc: &mastik_core::Document, lib: &Library) -> Option<String> {
    let tops = doc
        .vault()
        .and_then(|v| doc.get(v))
        .map_or(&[][..], |v| v.children());
    let mains = tops
        .iter()
        .filter_map(|t| doc.get(*t).and_then(|n| n.origin()))
        .map(|o| (o.kit.as_str(), o.name.as_deref(), o.version));
    let vars = doc.variables().iter().filter_map(|v| v.origin.as_ref());
    let styles = doc.styles().iter().filter_map(|s| s.origin.as_ref());
    let tokens = vars
        .map(|o| (o.kit.as_str(), o.version))
        .chain(styles.map(|o| (o.kit.as_str(), o.version)));
    let mut held: Vec<(&str, Option<&str>, u32)> = Vec::new();
    for (kit, name, version) in mains.chain(tokens.map(|(k, v)| (k, None, v))) {
        match held.iter_mut().find(|h| h.0 == kit) {
            Some(h) => {
                h.1 = h.1.or(name);
                h.2 = h.2.min(version);
            }
            None => held.push((kit, name, version)),
        }
    }
    let library = crate::kits::index::kits_in(lib, doc.meta().id.as_deref());
    let root = lib.root.display();
    let missing: Vec<String> = held
        .iter()
        .filter_map(|&(kit, name, version)| {
            let named = name.map_or_else(|| format!("kit {kit}"), str::to_owned);
            match library.iter().find(|k| k.id == kit) {
                None => Some(format!("{named} is not in {root}")),
                Some(k) if k.version < version => Some(format!(
                    "{named} is at v{} in {root}, older than the v{version} held",
                    k.version
                )),
                Some(_) => None,
            }
        })
        .collect();
    (!missing.is_empty()).then(|| format!("not checked: {}", missing.join(", ")))
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

/// A document's variables and styles, as [`tokens`] reads them.
type Tokens = (Vec<mastik_core::Variable>, Vec<mastik_core::style::Style>);

/// The document's variables and styles, the kit version each was taken at left
/// out: an update moves every kit token's version, and only a new value is a
/// change.
fn tokens(doc: &mastik_core::Document) -> Tokens {
    let vars = doc.variables().iter().cloned().map(|mut v| {
        if let Some(o) = &mut v.origin {
            o.version = 0;
        }
        v
    });
    let styles = doc.styles().iter().cloned().map(|mut s| {
        if let Some(o) = &mut s.origin {
            o.version = 0;
        }
        s
    });
    (vars.collect(), styles.collect())
}

/// How many of `after`'s rows are new or changed, plus `before`'s gone.
fn moved<T: PartialEq, K: PartialEq>(before: &[T], after: &[T], key: impl Fn(&T) -> K) -> usize {
    let gone = before
        .iter()
        .filter(|b| !after.iter().any(|a| key(a) == key(b)));
    after.iter().filter(|a| !before.contains(a)).count() + gone.count()
}

/// *Atlas v3 → v4, Icons v1 → v2 (5 changed, 1 removed, 1 made local, 2
/// variables changed)*; the layers' *changed* is left out when only tokens moved.
fn taken(
    a: &Accepted,
    held: &[crate::kits::updates::Waiting],
    before: &Tokens,
    after: &Tokens,
) -> String {
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
    let vars = moved(&before.0, &after.0, |v| v.id);
    let styles = moved(&before.1, &after.1, |s| s.id);
    let mut counts = Vec::new();
    if !a.changed.is_empty() || vars + styles == 0 {
        counts.push(format!("{} changed", a.changed.len()));
    }
    if !a.removed.is_empty() {
        counts.push(format!("{} removed", a.removed.len()));
    }
    if !a.made_local.is_empty() {
        counts.push(format!("{} made local", a.made_local.len()));
    }
    for (n, one, many) in [(vars, "variable", "variables"), (styles, "style", "styles")] {
        if n > 0 {
            counts.push(format!("{n} {} changed", if n == 1 { one } else { many }));
        }
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

    /// A library holding Atlas — one component whose fill reads a colour
    /// variable, published at v1, then at v2 with only the variable's value
    /// changed — and Consumer, holding an instance of it at v1. Built through
    /// the MCP's tools on a `serve` host. Answers the root and Consumer's path.
    fn library(tag: &str) -> (std::path::PathBuf, String) {
        use crate::mcp::Engine;
        use crate::mcp::serve::Headless;
        use serde_json::{Value, json};
        let root = crate::kits::picker::tests::temp_root(tag);
        let mut engine = Engine::default();
        let mut docs = Headless::new(Some(root.clone()));
        let mut ok = |tool: &str, args: Value| -> Value {
            let (reply, _answer) = std::sync::mpsc::sync_channel(1);
            let call = mastik_mcp::queue::Call::new(1, None, tool, args, reply);
            let out = engine.handle(&mut docs, call);
            assert!(!out.is_error, "{tool}: {:?}", out.content);
            out.structured.unwrap()
        };
        let atlas = ok("create_document", json!({ "name": "Atlas" }))["path"].clone();
        let page = ok("use_document", json!({ "document": atlas }))["pages"][0]["id"].clone();
        let ink = ok("add_variable", json!({ "kind": "color", "name": "ink" }))["variable"].clone();
        let kind = mastik_core::verbs::example("NodeKind").unwrap();
        let rect = ok(
            "create_node",
            json!({ "parent": page, "kind": kind, "name": "Badge" }),
        )["created"][0]
            .clone();
        let fills = mastik_core::verbs::example("[Fill]").unwrap();
        ok("set_fills", json!({ "nodes": [rect], "fills": fills }));
        ok(
            "bind_variable",
            json!({ "nodes": [rect], "field": { "fill": 0 }, "variable": ink }),
        );
        let main = ok("make_component", json!({ "nodes": [rect] }))["created"][0].clone();
        ok("make_kit", json!({}));
        ok("publish_kit", json!({ "note": "First" }));

        let consumer = ok("create_document", json!({ "name": "Consumer" }))["path"].clone();
        let page = ok("use_document", json!({ "document": consumer }))["pages"][0]["id"].clone();
        ok(
            "insert_from_kit",
            json!({ "kit": "Atlas", "main": main, "parent": page }),
        );

        ok("use_document", json!({ "document": atlas }));
        ok(
            "set_variable_value",
            json!({ "variable": ink, "value": "#ff0000" }),
        );
        ok("publish_kit", json!({ "note": "Red" }));
        (root, consumer.as_str().unwrap().to_owned())
    }

    fn update(file: &str, library: &std::path::Path, dry_run: bool) -> Update {
        Update {
            files: vec![file.to_owned()],
            kit: None,
            library: Some(library.display().to_string()),
            page: None,
            dry_run,
        }
    }

    /// A dry run says what it would take, fails the gate and writes nothing; a
    /// real run takes it, writes it and passes; then the document is up to
    /// date. A variable-only update counts the variable.
    ///
    /// flip: `one` writing before its dry-run return → fails at "a dry run
    /// writes nothing"; `taken` counting layers only → fails at "1 variable".
    #[test]
    fn kit_update_dry_runs_takes_and_reports() {
        let (root, consumer) = library("cli-update");
        let lib = Library::open(root.clone());
        let bytes = std::fs::read(&consumer).unwrap();

        let (line, code) = one(&update(&consumer, &root, true), &lib, &consumer).unwrap();
        assert!(line.starts_with("would take Atlas v1 → v2 ("), "{line}");
        assert!(line.contains("1 variable changed"), "{line}");
        assert!(!line.contains("0 changed"), "{line}");
        assert_eq!(code, 1, "a waiting update fails the dry run");
        assert_eq!(
            std::fs::read(&consumer).unwrap(),
            bytes,
            "a dry run writes nothing"
        );

        let (line, code) = one(&update(&consumer, &root, false), &lib, &consumer).unwrap();
        assert!(line.starts_with("took Atlas v1 → v2 ("), "{line}");
        assert!(line.ends_with("; wrote it"), "{line}");
        assert_eq!(code, 0);
        let doc = mastik_core::io::load(&std::fs::read(&consumer).unwrap()).unwrap();
        let top = doc.get(doc.vault().unwrap()).unwrap().children()[0];
        assert_eq!(doc.get(top).unwrap().origin().unwrap().version, 2);

        let lib = Library::open(root.clone());
        let (line, code) = one(&update(&consumer, &root, true), &lib, &consumer).unwrap();
        assert_eq!((line.as_str(), code), ("up to date", 0));
        let missing = root.join("missing.mastik").display().to_string();
        let mut both = update(&consumer, &root, true);
        both.files.push(missing);
        assert_eq!(run(&both), 2, "the worst code of the documents");
        let _ = std::fs::remove_dir_all(&root);
    }

    /// A library that lacks a kit the document draws from is not "up to date":
    /// it names the kit and fails the gate.
    ///
    /// flip: `one` ignoring `unchecked` → fails at "not checked".
    #[test]
    fn kit_update_names_a_kit_the_library_lacks() {
        let (root, consumer) = library("cli-lacks");
        let empty = crate::kits::picker::tests::temp_root("cli-lacks-empty");
        let lib = Library::open(empty.clone());
        let (line, code) = one(&update(&consumer, &empty, true), &lib, &consumer).unwrap();
        assert!(line.starts_with("not checked: Atlas is not in"), "{line}");
        assert_eq!(code, 1);
        let _ = std::fs::remove_dir_all(&root);
        let _ = std::fs::remove_dir_all(&empty);
    }
}
