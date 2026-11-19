//! `mastik run` — a script applied to a document headlessly, in either of the
//! MCP's two languages, picked by the file's extension: a `.luau` file is the
//! MCP `run_script`'s Command Mode Luau, anything else a JSON `batch`'s steps.
//!
//! Both run through the MCP's own code on an editor session
//! (`mcp::run_offline`), so a script an agent ran, or a batch it sent, replays
//! here unchanged. A refusal anywhere writes nothing.

use crate::session::EditorSession;
use serde_json::{Map, Value, json};

/// What `mastik run` takes, for its refusals and `help run`.
pub(crate) const USAGE: &str = "usage: mastik run <script.json> [--in <file.mastik>] -o <out.mastik>\n       \
     mastik run <script.luau> [--in <file.mastik>] [--select <id>]... [--layer <name>=<id>]... \
     [--param <input>=<value>]... -o <out.mastik>";

/// A parsed `mastik run` command line.
#[derive(Debug, PartialEq)]
pub(crate) struct Run {
    pub script: String,
    /// The document the script runs on; a new one without it.
    pub input: Option<String>,
    pub out: String,
    /// `--select`: the layers a Luau script finds selected.
    pub select: Vec<String>,
    /// `--layer name=id`: the labels a Luau script reaches layers by.
    pub layers: Vec<(String, String)>,
    /// `--param input=value`: values for the inputs a Luau script's header declares.
    pub params: Vec<(String, String)>,
}

impl Run {
    fn luau(&self) -> bool {
        std::path::Path::new(&self.script)
            .extension()
            .is_some_and(|e| e.eq_ignore_ascii_case("luau"))
    }
}

/// `mastik run <script> [options]`, `args` starting after `run`.
pub(crate) fn parse(args: &[String]) -> Result<Run, String> {
    let (mut script, mut input, mut out) = (None, None, None);
    let (mut select, mut layers, mut params) = (Vec::new(), Vec::new(), Vec::new());
    let mut i = 0;
    while i < args.len() {
        match args[i].as_str() {
            flag @ ("--in" | "-o" | "--out" | "--select" | "--layer" | "--param") => {
                i += 1;
                // A `--param` value may be empty, or a negative number.
                let value = args
                    .get(i)
                    .filter(|v| flag == "--param" || !v.starts_with('-'))
                    .ok_or_else(|| format!("{flag} wants a value\n{USAGE}"))?
                    .clone();
                let pair = |what: &str| {
                    value
                        .split_once('=')
                        .filter(|(n, v)| !n.is_empty() && (flag == "--param" || !v.is_empty()))
                        .map(|(n, v)| (n.to_owned(), v.to_owned()))
                        .ok_or_else(|| format!("{flag} wants <name>={what}\n{USAGE}"))
                };
                match flag {
                    "--in" => input = Some(value),
                    "--select" => select.push(value),
                    "--layer" => layers.push(pair("<id>")?),
                    "--param" => params.push(pair("<value>")?),
                    _ => out = Some(value),
                }
            }
            other if other.starts_with('-') => {
                return Err(format!("unknown option {other}\n{USAGE}"));
            }
            other if script.is_some() => {
                return Err(format!("one script at a time; also got {other}\n{USAGE}"));
            }
            other => script = Some(other.to_string()),
        }
        i += 1;
    }
    let (Some(script), Some(out)) = (script, out) else {
        return Err(USAGE.to_string());
    };
    let run = Run {
        script,
        input,
        out,
        select,
        layers,
        params,
    };
    if !run.luau() && !(run.select.is_empty() && run.layers.is_empty() && run.params.is_empty()) {
        return Err(format!(
            "--select, --layer and --param are a Luau script's; a batch names its layers \
             and values itself\n{USAGE}"
        ));
    }
    Ok(run)
}

/// Run the script and write the result atomically. Answers the lines for
/// stdout: a Luau script's printed lines, or a line per batch step that made
/// something; and the summary for stderr. An `-o` naming the script is refused;
/// naming `--in` writes the document in place.
pub(crate) fn run(run: &Run) -> Result<(Vec<String>, String), String> {
    let script = &run.script;
    if crate::same_file(std::path::Path::new(&run.out), std::path::Path::new(script)) {
        return Err(format!(
            "refusing to write the document over {script}, the script it runs"
        ));
    }
    let text = std::fs::read_to_string(script).map_err(|e| format!("cannot read {script}: {e}"))?;
    let mut s = EditorSession::new();
    let doc = match &run.input {
        Some(file) => {
            let bytes = std::fs::read(file).map_err(|e| format!("cannot read {file}: {e}"))?;
            mastik_core::io::load(&bytes).map_err(|e| format!("cannot load {file}: {e}"))?
        }
        None => mastik_core::Document::new(s.ids.mint()),
    };
    s.adopt_document(doc, None);
    let (lines, said) = match run.luau() {
        true => luau(&mut s, run, text)?,
        false => batch(&mut s, &text)?,
    };
    let out = &run.out;
    let bytes = mastik_core::io::save(&s.doc).map_err(|e| format!("cannot save: {e}"))?;
    crate::atomic::write(std::path::Path::new(out), &bytes)
        .map_err(|e| format!("cannot write {out}: {e}"))?;
    Ok((lines, format!("{said}; wrote {out}")))
}

/// A Luau script, as the MCP `run_script` runs it.
fn luau(s: &mut EditorSession, run: &Run, source: String) -> Result<(Vec<String>, String), String> {
    let layers: Map<String, Value> = run
        .layers
        .iter()
        .map(|(name, id)| (name.clone(), json!(id)))
        .collect();
    let params: Map<String, Value> = run
        .params
        .iter()
        .map(|(name, value)| (name.clone(), json!(value)))
        .collect();
    let args = json!({ "source": source, "nodes": run.select, "layers": layers, "params": params });
    let answer = crate::mcp::run_offline(s, "run_script", &args)
        .map_err(|e| format!("{}: {e}", run.script))?;
    let mut lines: Vec<String> = Vec::new();
    if let Some(before) = answer["printed_before"].as_u64() {
        lines.push(format!("({before} earlier lines not kept)"));
    }
    lines.extend(
        answer["printed"]
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(|l| l.as_str().map(str::to_owned)),
    );
    // The value as the command bar shows it: a string in its quotes.
    if let Some(value) = answer.get("value").and_then(Value::as_str) {
        lines.push(value.to_owned());
    }
    Ok((
        lines,
        answer["said"].as_str().unwrap_or_default().to_owned(),
    ))
}

/// A JSON batch: a list of `{ tool, args, as }` steps, or a whole `batch`
/// call's arguments (`{ "steps": [...] }`), as the MCP `batch` runs them.
fn batch(s: &mut EditorSession, text: &str) -> Result<(Vec<String>, String), String> {
    let script: Value =
        serde_json::from_str(text).map_err(|e| format!("the script is not JSON: {e}"))?;
    let steps = match script {
        Value::Array(_) => script,
        Value::Object(mut call) => call
            .remove("steps")
            .ok_or("a script is a list of steps, or an object holding `steps`")?,
        _ => return Err("a script is a list of steps, or an object holding `steps`".into()),
    };
    // A core script's `{ verb, args }` step, which a batch would refuse as a stray key.
    if let Some(n) = steps
        .as_array()
        .and_then(|list| list.iter().position(|step| step.get("verb").is_some()))
    {
        return Err(format!(
            "step {}: a step names its `tool`, as the MCP `batch` does, not a `verb`",
            n + 1
        ));
    }
    // Only the steps are kept: a saved call's `base_rev` named a revision of the
    // agent's open document, which this one is not.
    let answer = crate::mcp::run_offline(s, "batch", &json!({ "steps": steps }))?;
    let answers = answer["steps"].as_array().map_or(&[][..], Vec::as_slice);
    let lines = answers
        .iter()
        .enumerate()
        .filter_map(|(i, a)| {
            let made = made(a);
            (!made.is_empty()).then(|| format!("step {}: {}", i + 1, made.join(" ")))
        })
        .collect();
    let n = answers.len();
    Ok((
        lines,
        format!("{n} {}", if n == 1 { "step" } else { "steps" }),
    ))
}

/// What a step's answer says it made: its created layers, else its main, else
/// any other id it names as made.
fn made(answer: &Value) -> Vec<String> {
    let created: Vec<String> = answer["created"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|v| v.as_str().map(str::to_owned))
        .collect();
    if !created.is_empty() {
        return created;
    }
    ["main", "image", "variable", "style", "id"]
        .iter()
        .find_map(|k| answer.get(*k).and_then(Value::as_str))
        .map(|v| vec![v.to_owned()])
        .unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn args(s: &[&str]) -> Vec<String> {
        s.iter().map(|a| a.to_string()).collect()
    }

    /// The extension picks the language, and only a Luau script takes labels.
    #[test]
    fn run_takes_a_script_and_its_options() {
        let r = parse(&args(&[
            "s.luau", "--select", "1:2", "--layer", "card=1:3", "-o", "o.mastik",
        ]))
        .unwrap();
        assert!(r.luau());
        assert_eq!(r.select, ["1:2"]);
        assert_eq!(r.layers, [("card".to_owned(), "1:3".to_owned())]);
        let r = parse(&args(&["s.json", "--in", "a.mastik", "-o", "o.mastik"])).unwrap();
        assert!(!r.luau());
        assert!(parse(&args(&["s.json", "--select", "1:2", "-o", "o.mastik"])).is_err());
        assert!(parse(&args(&["s.luau", "--layer", "card", "-o", "o.mastik"])).is_err());
        assert!(parse(&args(&["s.luau"])).is_err(), "no -o");
        let r = parse(&args(&[
            "s.luau", "--param", "x=-4", "--param", "brand=", "-o", "o.mastik",
        ]))
        .unwrap();
        let pair = |n: &str, v: &str| (n.to_owned(), v.to_owned());
        assert_eq!(r.params, [pair("x", "-4"), pair("brand", "")]);
        assert!(parse(&args(&["s.json", "--param", "x=1", "-o", "o.mastik"])).is_err());
    }

    /// A Luau script's `--param` reaches it as `param`, read by its header's
    /// types, and the inputs not given keep their defaults.
    #[test]
    fn a_luau_script_reads_its_params() {
        let dir = scratch("luau");
        let script = dir.join("s.luau");
        let source = "---@input label string = Hi\n---@input n integer = 1 -- Copies {1..5}\n\
                      return param.label .. \" \" .. (param.n * 2)\n";
        std::fs::write(&script, source).unwrap();
        let out = dir.join("o.mastik");
        let r = parse(&args(&[
            script.to_str().unwrap(),
            "--param",
            "n=3",
            "-o",
            out.to_str().unwrap(),
        ]))
        .unwrap();
        let (lines, _) = run(&r).unwrap();
        assert_eq!(lines, ["\"Hi 6\""]);
        let bad = parse(&args(&[
            script.to_str().unwrap(),
            "--param",
            "n=9",
            "-o",
            "x.mastik",
        ]));
        assert!(run(&bad.unwrap()).unwrap_err().contains("outside"));
        let _ = std::fs::remove_dir_all(&dir);
    }

    fn scratch(name: &str) -> std::path::PathBuf {
        let dir = std::env::temp_dir().join(format!("mastik-run-{}-{name}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    /// A batch's steps run through the MCP's `batch`, `set` and `@label` included.
    #[test]
    fn a_batch_script_runs_set_and_labels() {
        let dir = scratch("batch");
        let script = dir.join("s.json");
        let steps = json!({ "steps": [
            { "tool": "create_text", "as": "card", "args": { "parent": "@page", "text": "Hi" } },
            { "tool": "set", "args": { "nodes": ["@card"], "props": { "name": "Renamed" } } },
        ]});
        std::fs::write(&script, steps.to_string()).unwrap();
        let out = dir.join("o.mastik");
        let r = Run {
            script: script.display().to_string(),
            input: None,
            out: out.display().to_string(),
            select: vec![],
            layers: vec![],
            params: vec![],
        };
        let (lines, said) = run(&r).unwrap();
        assert_eq!(lines.len(), 1, "{lines:?}");
        assert!(said.starts_with("2 steps"), "{said}");
        let doc = mastik_core::io::load(&std::fs::read(&out).unwrap()).unwrap();
        let page = doc.first_page();
        let names: Vec<&str> = doc
            .get(page)
            .unwrap()
            .children()
            .iter()
            .filter_map(|c| doc.get(*c).map(|n| n.name()))
            .collect();
        assert_eq!(names, ["Renamed"]);
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// An `-o` naming the script is refused and leaves the script, while one
    /// naming `--in` writes the document in place.
    ///
    /// flip: `run` without its same-file check → fails at `unwrap_err`, the run
    /// writing the document over the script.
    #[test]
    fn run_refuses_to_write_over_its_script() {
        let dir = scratch("self");
        let script = dir.join("s.json");
        let steps = r#"[{ "tool": "create_text", "args": { "parent": "@page", "text": "Hi" } }]"#;
        std::fs::write(&script, steps).unwrap();
        let doc = dir.join("d.mastik");
        let blank = mastik_core::io::save(&mastik_core::Document::new(
            mastik_core::IdSource::new(0x5E).mint(),
        ))
        .unwrap();
        std::fs::write(&doc, &blank).unwrap();
        let on = |out: &std::path::Path| Run {
            script: script.display().to_string(),
            input: Some(doc.display().to_string()),
            out: out.display().to_string(),
            select: vec![],
            layers: vec![],
            params: vec![],
        };
        let err = run(&on(&script)).unwrap_err();
        assert!(err.contains("refusing"), "{err}");
        assert_eq!(std::fs::read_to_string(&script).unwrap(), steps);
        run(&on(&doc)).unwrap();
        assert_ne!(std::fs::read(&doc).unwrap(), blank, "written in place");
        let _ = std::fs::remove_dir_all(&dir);
    }
}
