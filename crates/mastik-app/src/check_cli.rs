//! `mastik check` — the checks a CI step runs (§9): contrast, code, review,
//! health and layout, over one document or several, as text for a person or
//! `--json` for a script.
//!
//! Every check answers each document with its report and whether it passed, and
//! the run exits with the worst document's code: 0 when all passed, 1 when one
//! failed, 2 when one could not be read or checked.

use mastik_core::{Document, NodeId};
use serde_json::{Value, json};

const USAGE: &str = "usage: mastik check contrast <file.mastik>... [--page <name|n>] \
                     [--aaa|--apca] [--non-text] [--json]\n       \
                     mastik check code <file.mastik>... [--page <name|n>] [--json]\n       \
                     mastik check review <file.mastik>... [--page <name|n>] [--json]\n       \
                     mastik check health <file.mastik>... [--page <name|n>] [--min <percent>] [--json]\n       \
                     mastik check layout <file.mastik>... [--page <name|n>] [--json]";

/// A parsed `mastik check` command line.
#[derive(Debug, PartialEq)]
pub(crate) struct Check {
    pub what: What,
    /// The documents, in the order given; at least one.
    pub files: Vec<String>,
    /// `--page`, asked of every document: a page's number from 1, or its exact name.
    pub page: Option<String>,
    /// `--json`: one JSON array, an object per document, instead of text.
    pub json: bool,
}

/// Which check runs.
#[derive(Debug, PartialEq)]
pub(crate) enum What {
    /// Every text layer's contrast against what is drawn behind it
    /// (`mastik_export::contrast`).
    Contrast(mastik_export::contrast::Options),
    /// The page's HTML export in a headless browser, held to the canvas
    /// (`mastik_export::code::fidelity`).
    Code,
    /// The design review the MCP `review` read runs (`mastik_export::review`).
    Review,
    /// *System health*'s readings, failing when coverage is under `min` percent.
    Health { min: Option<u32> },
    /// The MCP `check_layout` read's placement lints
    /// (`mastik_export::context::check_layout`).
    Layout,
}

/// `mastik check <which> <files…> [options]`, `args` starting at `<which>`.
pub(crate) fn parse(args: &[String]) -> Result<Check, String> {
    use mastik_export::contrast::{Level, Options};
    let which = args.first().map(String::as_str);
    let contrast = match which {
        Some("contrast") => true,
        Some("code" | "review" | "health" | "layout") => false,
        _ => return Err(USAGE.to_string()),
    };
    let (mut files, mut page, mut options) = (Vec::new(), None, Options::default());
    let (mut min, mut json) = (None, false);
    let mut i = 1;
    while i < args.len() {
        match args[i].as_str() {
            "--json" => json = true,
            "--min" if which == Some("health") => {
                i += 1;
                min = Some(
                    args.get(i)
                        .and_then(|v| v.trim_end_matches('%').parse::<u32>().ok())
                        .filter(|m| *m <= 100)
                        .ok_or_else(|| format!("--min wants a percentage, 0 to 100\n{USAGE}"))?,
                );
            }
            level @ ("--aaa" | "--apca") if contrast => {
                if options.level != Level::Aa {
                    return Err(format!("one level at a time\n{USAGE}"));
                }
                options.level = if level == "--aaa" {
                    Level::Aaa
                } else {
                    Level::Apca
                };
            }
            "--non-text" if contrast => options.non_text = true,
            "--page" => {
                i += 1;
                let value = args.get(i).filter(|v| !v.starts_with('-'));
                page = Some(
                    value
                        .ok_or_else(|| format!("--page wants a name or number\n{USAGE}"))?
                        .clone(),
                );
            }
            other if other.starts_with('-') => {
                return Err(format!("unknown option {other}\n{USAGE}"));
            }
            other => files.push(other.to_string()),
        }
        i += 1;
    }
    if files.is_empty() {
        return Err(USAGE.to_string());
    }
    let what = match which {
        Some("code") => What::Code,
        Some("review") => What::Review,
        Some("health") => What::Health { min },
        Some("layout") => What::Layout,
        _ => What::Contrast(options),
    };
    Ok(Check {
        what,
        files,
        page,
        json,
    })
}

/// One document's answer: the report for a person, the same for a script, and
/// whether it passed.
struct Outcome {
    text: String,
    json: Value,
    passed: bool,
}

/// Run `check` over each of its documents, print the reports, and answer the
/// exit code: the worst document's.
///
/// One document prints its report alone, as the checks always have. Several
/// print each under its file's name, indented, then a count; a document that
/// cannot be read is named on stderr and the rest still run. `--json` prints one
/// array whatever the count, so a script reads one shape.
pub(crate) fn run(check: &Check) -> i32 {
    let several = check.files.len() > 1;
    let (mut worst, mut passed, mut failed, mut unreadable) = (0, 0, 0, 0);
    let mut answers = Vec::new();
    for (i, file) in check.files.iter().enumerate() {
        let code = match one(&check.what, file, check.page.as_deref(), i) {
            Ok(o) => {
                if check.json {
                    answers.push(with_file(file, o.passed, o.json));
                } else if several {
                    println!("{file}");
                    for line in o.text.lines() {
                        println!("  {line}");
                    }
                } else {
                    print!("{}", o.text);
                }
                match o.passed {
                    true => passed += 1,
                    false => failed += 1,
                }
                i32::from(!o.passed)
            }
            Err(msg) => {
                eprintln!("{msg}");
                if check.json {
                    answers.push(json!({ "file": file, "error": msg }));
                }
                unreadable += 1;
                2
            }
        };
        worst = worst.max(code);
    }
    if check.json {
        // `Value`'s own printer cannot fail.
        println!(
            "{}",
            serde_json::to_string_pretty(&answers).unwrap_or_default()
        );
    } else if several {
        println!("{}", tally(passed, failed, unreadable));
    }
    let _ = std::io::Write::flush(&mut std::io::stdout());
    worst
}

/// The closing count of a run over several documents.
fn tally(passed: usize, failed: usize, unreadable: usize) -> String {
    let mut out = format!(
        "{} documents: {passed} passed",
        passed + failed + unreadable
    );
    if failed > 0 {
        out.push_str(&format!(", {failed} failed"));
    }
    if unreadable > 0 {
        out.push_str(&format!(", {unreadable} unreadable"));
    }
    out
}

/// A document's JSON answer: its file and whether it passed, then the check's
/// own report.
fn with_file(file: &str, passed: bool, report: Value) -> Value {
    let mut answer = json!({ "file": file, "passed": passed });
    if let (Some(into), Value::Object(fields)) = (answer.as_object_mut(), report) {
        into.extend(fields);
    }
    answer
}

/// Check one document. `index` keeps `check code`'s kept folders apart when one
/// run checks several. Every refusal names the file, once.
fn one(what: &What, file: &str, page: Option<&str>, index: usize) -> Result<Outcome, String> {
    let bytes = std::fs::read(file).map_err(|e| format!("cannot read {file}: {e}"))?;
    let doc = mastik_core::io::load(&bytes).map_err(|e| format!("cannot load {file}: {e}"))?;
    match what {
        What::Contrast(options) => contrast(&doc, page, *options),
        What::Code => code(doc, page, index),
        What::Review => review(&doc, page),
        What::Health { min } => health(&doc, page, *min),
        What::Layout => layout(doc, page),
    }
    .map_err(|e| format!("{file}: {e}"))
}

/// The layers' names, joined, as a finding's line names them.
fn names(doc: &Document, ids: &[NodeId]) -> String {
    let names: Vec<&str> = ids
        .iter()
        .filter_map(|id| doc.get(*id).map(|n| n.name()))
        .collect();
    names.join(", ")
}

/// Give each entry of `list` that carries `ids` the layers' `names` beside
/// them, so a script that annotates a pull request need not look them up.
fn name_ids(doc: &Document, list: &mut Value) {
    for entry in list.as_array_mut().into_iter().flatten() {
        let ids: Vec<NodeId> = entry["ids"]
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(|v| v.as_str().and_then(NodeId::from_wire))
            .collect();
        let names: Vec<&str> = ids
            .iter()
            .filter_map(|id| doc.get(*id).map(|n| n.name()))
            .collect();
        entry["names"] = json!(names);
    }
}

/// "1 finding", "3 findings".
fn count(n: usize, one: &str, many: &str) -> String {
    format!("{n} {}", if n == 1 { one } else { many })
}

/// `check layout`: each placement issue on the page with its layers' names, and
/// whether there were none.
fn layout(mut doc: Document, page: Option<&str>) -> Result<Outcome, String> {
    let res = mastik_core::Resolved::settled(&mut doc);
    let page = crate::chosen_page(&doc, page)?;
    let mut answer = mastik_export::context::check_layout(&doc, &res, &[page]);
    name_ids(&doc, &mut answer["issues"]);
    let issues = answer["issues"].as_array().map_or(&[][..], Vec::as_slice);
    let mut text = String::new();
    for issue in issues {
        let names: Vec<&str> = issue["names"]
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(Value::as_str)
            .collect();
        text.push_str(&format!(
            "{}  {}: {}\n",
            issue["code"].as_str().unwrap_or_default(),
            names.join(", "),
            issue["message"].as_str().unwrap_or_default()
        ));
    }
    let more = answer["more"].as_u64().unwrap_or(0) as usize;
    if more > 0 {
        text.push_str(&format!("and {} not listed\n", count(more, "more", "more")));
    }
    let total = issues.len() + more;
    let checked = answer["checked"].as_u64().unwrap_or(0) as usize;
    text.push_str(&format!(
        "{} on {}\n",
        count(total, "issue", "issues"),
        count(checked, "layer", "layers")
    ));
    Ok(Outcome {
        text,
        json: answer,
        passed: total == 0,
    })
}

/// `check review`: every finding of the design review on the page, a line
/// each, and whether there were none.
fn review(doc: &Document, page: Option<&str>) -> Result<Outcome, String> {
    use mastik_export::review::{Options, Scope, review, to_json};
    let page = crate::chosen_page(doc, page)?;
    let res = mastik_core::Resolved::rebuild(doc);
    let scope = Scope::new(doc, &res, &[page]);
    let found = review(&scope, Options::default());
    let mut text = String::new();
    for f in &found {
        text.push_str(&format!(
            "{}  {}: {}",
            f.code,
            names(doc, &f.ids),
            f.message
        ));
        if let Some(instead) = &f.instead {
            text.push_str(&format!(" Instead: {instead}."));
        }
        text.push('\n');
    }
    text.push_str(&format!(
        "{} on {} layers\n",
        count(found.len(), "finding", "findings"),
        scope.layers.len()
    ));
    let mut json = to_json(&scope, &found);
    name_ids(doc, &mut json["findings"]);
    Ok(Outcome {
        text,
        json,
        passed: found.is_empty(),
    })
}

/// `check health`: the readings over the page asked for or every page, and
/// whether coverage reaches `min` (always, without one).
fn health(doc: &Document, page: Option<&str>, min: Option<u32>) -> Result<Outcome, String> {
    use mastik_export::review::{Options, Scope, every_page, health};
    let roots = match page {
        Some(_) => vec![crate::chosen_page(doc, page)?],
        None => {
            let first = doc
                .pages()
                .first()
                .map(|p| p.id)
                .ok_or("the file has no page")?;
            every_page(doc, first)
        }
    };
    let res = mastik_core::Resolved::rebuild(doc);
    let scope = Scope::new(doc, &res, &roots);
    let h = health(&scope, Options::default().step);
    Ok(Outcome {
        text: h.to_text(),
        json: h.to_json(doc),
        passed: min.is_none_or(|m| h.overall().unwrap_or(100) as u32 >= m),
    })
}

/// `check contrast`: the failures and skips, a count, and whether every
/// measured layer passed.
fn contrast(
    doc: &Document,
    page: Option<&str>,
    options: mastik_export::contrast::Options,
) -> Result<Outcome, String> {
    use mastik_export::contrast::check_with;
    let page = crate::chosen_page(doc, page)?;
    let level = options.level;
    let report = check_with(doc, page, options, None);
    let mut text = String::new();
    let mut failed = 0;
    for f in &report.findings {
        if f.passes() {
            continue;
        }
        failed += 1;
        let mode = f
            .mode
            .as_ref()
            .map(|m| format!(" in {m}"))
            .unwrap_or_default();
        // Rounded down, so a fail never prints as its threshold.
        text.push_str(&format!(
            "fail  {}: {}, needs {}{mode}\n",
            f.trail.join(" / "),
            level.format(f.ratio),
            level.format_needs(f.needs)
        ));
    }
    for s in &report.skipped {
        text.push_str(&format!("skip  {}: {}\n", s.trail.join(" / "), s.why));
    }
    text.push_str(&format!(
        "{} of {} {} pass {}\n",
        report.findings.len() - failed,
        report.findings.len(),
        if options.non_text {
            "layers"
        } else {
            "text layers"
        },
        level.label()
    ));
    let json = mastik_export::contrast::to_json(level, std::slice::from_ref(&report), false);
    Ok(Outcome {
        text,
        json,
        passed: failed == 0,
    })
}

/// `check code`: each drifted layer, the export's notes, and whether the page
/// draws as the canvas does. On a drift the page is kept in a temp folder and
/// named, so the screenshot can be compared with `cpu.png` by eye.
fn code(mut doc: Document, page: Option<&str>, index: usize) -> Result<Outcome, String> {
    use mastik_export::code::fidelity;
    let res = mastik_core::Resolved::settled(&mut doc);
    let page = crate::chosen_page(&doc, page)?;
    let origins = doc
        .get(page)
        .map(|r| r.children().to_vec())
        .unwrap_or_default();
    let browser = fidelity::browser().ok_or(
        "no headless browser found: install Chrome, Chromium or Edge, or set MASTIK_BROWSER",
    )?;
    let dir =
        std::env::temp_dir().join(format!("mastik-check-code-{}-{index}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).map_err(|e| format!("cannot make {}: {e}", dir.display()))?;
    let export = mastik_export::code::html::html_of(&doc, &res, &origins);
    let found = fidelity::check_export(&doc, &res, &origins, &export, &browser, &dir)?;
    let mut text = String::new();
    let mut drifted = Vec::new();
    for d in &found.drifted {
        let (how, entry) = match d.how {
            fidelity::How::Cell { error, at } => (
                format!("{error:.1}/255 at {at:?}"),
                json!({ "error": error, "at": [at.0, at.1] }),
            ),
            fidelity::How::Ink { misplaced } => (
                format!("{:.0}% of its ink is off the canvas's", misplaced * 100.0),
                json!({ "misplaced": misplaced }),
            ),
        };
        text.push_str(&format!("drift  {}: {how}\n", d.label));
        let mut entry = entry;
        entry["id"] = json!(d.id.to_wire());
        entry["label"] = json!(d.label);
        drifted.push(entry);
    }
    if found.unclaimed > 0 {
        text.push_str(&format!(
            "drift  {} cells where no layer has ink\n",
            found.unclaimed
        ));
    }
    for a in &export.approximated {
        text.push_str(&format!("noted  {a}\n"));
    }
    let holds = found.holds();
    let mut json = json!({
        "drifted": drifted,
        "unclaimed": found.unclaimed,
        "mean": found.mean,
        "noted": export.approximated,
    });
    if holds {
        let _ = std::fs::remove_dir_all(&dir);
        text.push_str(&format!(
            "the page draws as the canvas does (mean {:.2}/255)\n",
            found.mean
        ));
    } else {
        json["kept"] = json!(dir.display().to_string());
        text.push_str(&format!(
            "{} layers drawn differently, mean {:.2}/255; the page is kept in {}\n",
            found.drifted.len(),
            found.mean,
            dir.display()
        ));
    }
    Ok(Outcome {
        text,
        json,
        passed: holds,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use mastik_export::contrast::{Level, Options};

    fn check(s: &[&str]) -> Result<Check, String> {
        parse(&s.iter().map(|a| a.to_string()).collect::<Vec<_>>())
    }

    fn files(s: &[&str]) -> Vec<String> {
        s.iter().map(|f| f.to_string()).collect()
    }

    /// Every check takes documents, a page and `--json`; contrast's levels,
    /// `--non-text` and health's `--min` stay their own, and no document is refused.
    #[test]
    fn checks_take_documents_a_page_and_json() {
        assert_eq!(
            check(&["contrast", "a.mastik", "--page", "2", "--aaa"]),
            Ok(Check {
                what: What::Contrast(Options {
                    level: Level::Aaa,
                    non_text: false,
                    default_ground: None,
                }),
                files: files(&["a.mastik"]),
                page: Some("2".into()),
                json: false,
            })
        );
        assert_eq!(
            check(&[
                "contrast",
                "a.mastik",
                "b.mastik",
                "--apca",
                "--non-text",
                "--json"
            ]),
            Ok(Check {
                what: What::Contrast(Options {
                    level: Level::Apca,
                    non_text: true,
                    default_ground: None,
                }),
                files: files(&["a.mastik", "b.mastik"]),
                page: None,
                json: true,
            })
        );
        assert_eq!(
            check(&["health", "a.mastik", "--min", "80%"]).map(|c| c.what),
            Ok(What::Health { min: Some(80) })
        );
        assert_eq!(
            check(&["layout", "a.mastik", "--page", "Home", "b.mastik"]),
            Ok(Check {
                what: What::Layout,
                files: files(&["a.mastik", "b.mastik"]),
                page: Some("Home".into()),
                json: false,
            })
        );
        assert_eq!(check(&["code", "a.mastik"]).map(|c| c.what), Ok(What::Code));
        assert_eq!(
            check(&["review", "a.mastik"]).map(|c| c.what),
            Ok(What::Review)
        );
        for refused in [
            &["a.mastik"][..],
            &["contrast"],
            &["layout", "--json"],
            &["contrast", "a.mastik", "--page"],
            &["contrast", "a.mastik", "--aa"],
            &["contrast", "a.mastik", "--aaa", "--apca"],
            &["code", "a.mastik", "--aaa"],
            &["layout", "a.mastik", "--non-text"],
            &["review", "a.mastik", "--min", "80"],
            &["health", "a.mastik", "--min", "101"],
        ] {
            assert!(check(refused).is_err(), "{refused:?}");
        }
    }

    /// A white 300 × 100 frame named Card holding one text, Caption, of the given
    /// grey at `x`, saved into `dir` as `name`.
    fn card(dir: &std::path::Path, name: &str, grey: u8, x: f64) -> String {
        use mastik_core::{
            Brush, Fill, IdSource, NodeKind, Operation, TextSizing, TextStyle, Transaction,
        };
        let mut ids = IdSource::new(0xD5);
        let root = ids.mint();
        let (frame, text) = (ids.mint(), ids.mint());
        let mut doc = mastik_core::Document::new(root);
        let fill = |v: u8| {
            mastik_core::keyed_by_position([Fill {
                brush: Brush::Solid(mastik_core::peniko::Color::from_rgba8(v, v, v, 255)).into(),
                visible: true,
            }])
        };
        doc.apply(&Transaction(vec![
            Operation::CreateNode {
                id: frame,
                parent: root,
                index: 0,
                kind: NodeKind::Artboard {
                    size: mastik_core::kurbo::Size::new(300.0, 100.0),
                    corner_radii: mastik_core::kurbo::RoundedRectRadii::default(),
                    corner_smoothing: 0.0,
                },
                transform: None,
                name: Some("Card".into()),
            },
            Operation::SetFills {
                id: frame,
                fills: fill(255),
            },
            Operation::CreateNode {
                id: text,
                parent: frame,
                index: 0,
                kind: NodeKind::Text {
                    content: "Faint".into(),
                    style: Box::new(TextStyle {
                        font_family: "Inter".into(),
                        font_size: 16.0,
                        weight: 400,
                        ..Default::default()
                    }),
                    spans: Default::default(),
                    para_spans: Default::default(),
                    paragraph: Default::default(),
                    block: Default::default(),
                    sizing: TextSizing::Auto,
                    on_path: None,
                    on_path_flip: false,
                    on_path_offset: 0.0,
                },
                transform: Some(mastik_core::kurbo::Affine::translate((x, 10.0))),
                name: Some("Caption".into()),
            },
            Operation::SetFills {
                id: text,
                fills: fill(grey),
            },
        ]))
        .unwrap();
        let file = dir.join(name);
        std::fs::write(&file, mastik_core::io::save(&doc).unwrap()).unwrap();
        file.to_string_lossy().into_owned()
    }

    fn scratch(name: &str) -> std::path::PathBuf {
        let dir = std::env::temp_dir().join(format!("mastik-{name}-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    /// Light grey type on white fails and names the layer, in the text and in
    /// the JSON.
    #[test]
    fn check_contrast_reports_a_failing_layer() {
        let dir = scratch("contrast-test");
        let file = card(&dir, "c.mastik", 200, 10.0);
        let o = one(&What::Contrast(Options::default()), &file, None, 0).unwrap();
        assert!(!o.passed, "{}", o.text);
        assert!(o.text.contains("fail  Card / Caption"), "{}", o.text);
        assert!(o.text.contains("0 of 1 text layers pass AA"), "{}", o.text);
        assert_eq!(o.json["failing"], 1, "{}", o.json);
        assert_eq!(o.json["findings"][0]["trail"], json!(["Card", "Caption"]));
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// A text wholly outside the frame that clips it is a layout issue naming
    /// both layers; the same card with the text inside it has none.
    /// flip: `layout` checking no root → fails at the first `passed` assertion.
    #[test]
    fn check_layout_names_a_text_outside_its_frame() {
        let dir = scratch("layout-test");
        let gone = card(&dir, "gone.mastik", 0, 500.0);
        let o = one(&What::Layout, &gone, None, 0).unwrap();
        assert!(!o.passed, "{}", o.text);
        assert!(
            o.text.contains("outside_clip  Caption, Card: "),
            "{}",
            o.text
        );
        assert!(o.text.ends_with("1 issue on 2 layers\n"), "{}", o.text);
        assert_eq!(o.json["issues"][0]["names"], json!(["Caption", "Card"]));
        let inside = card(&dir, "inside.mastik", 0, 10.0);
        let o = one(&What::Layout, &inside, None, 0).unwrap();
        assert!(o.passed, "{}", o.text);
        assert_eq!(o.text, "0 issues on 2 layers\n");
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// A document that cannot be read is answered with its error, and a run's
    /// JSON carries each document under its file.
    #[test]
    fn a_document_is_answered_under_its_file() {
        let dir = scratch("check-files-test");
        let good = card(&dir, "good.mastik", 0, 10.0);
        let Err(missing) = one(&What::Layout, "missing.mastik", None, 0) else {
            panic!("a missing document is answered")
        };
        assert_eq!(missing.matches("missing.mastik").count(), 1, "{missing}");
        let Err(no_page) = one(&What::Layout, &good, Some("9"), 0) else {
            panic!("a page the document lacks is answered")
        };
        assert!(
            no_page.starts_with(&format!("{good}: no page")),
            "{no_page}"
        );
        let o = one(&What::Layout, &good, None, 0).unwrap();
        let answer = with_file(&good, o.passed, o.json);
        assert_eq!(answer["file"], json!(good));
        assert_eq!(answer["passed"], true);
        assert_eq!(answer["checked"], 2);
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// The closing count names only what happened.
    #[test]
    fn the_tally_counts_each_outcome() {
        assert_eq!(tally(2, 0, 0), "2 documents: 2 passed");
        assert_eq!(
            tally(1, 1, 1),
            "3 documents: 1 passed, 1 failed, 1 unreadable"
        );
    }
}
