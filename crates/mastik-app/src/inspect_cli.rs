//! `mastik inspect` — the MCP's reads of one document, as JSON on stdout:
//! `get_outline`, `design_context` and the `list_*` reads, answered by the MCP's
//! own code (`mcp::run_offline`) without its answer budget.

use crate::session::EditorSession;
use serde_json::{Value, json};

/// What `mastik inspect` takes, for its refusals and `help inspect`.
pub(crate) const USAGE: &str = "usage: mastik inspect <file.mastik> outline [--page <name|n>] [--layer <name|id>] \
     [--depth <n>] [--limit <n>]\n       \
     mastik inspect <file.mastik> context [--page <name|n>] [--layer <name|id>]\n       \
     mastik inspect <file.mastik> components|variables|styles [--query <text>]";

/// A parsed `mastik inspect` command line.
#[derive(Debug, PartialEq)]
pub(crate) struct Inspect {
    pub file: String,
    /// The MCP read it answers with.
    pub tool: &'static str,
    /// `--page`: the page read, by number from 1 or exact name.
    pub page: Option<String>,
    /// `--layer`: the layer read instead of the page, by id or exact name.
    pub layer: Option<String>,
    /// The read's own arguments: `depth`, `limit`, `query`.
    pub args: serde_json::Map<String, Value>,
}

/// `mastik inspect <file> <read> [options]`, `args` starting after `inspect`.
pub(crate) fn parse(args: &[String]) -> Result<Inspect, String> {
    let (Some(file), Some(read)) = (args.first(), args.get(1)) else {
        return Err(USAGE.to_string());
    };
    let tool = match read.as_str() {
        "outline" => "get_outline",
        "context" => "design_context",
        "components" => "list_components",
        "variables" => "list_variables",
        "styles" => "list_styles",
        other => return Err(format!("no read {other:?}\n{USAGE}")),
    };
    let on_page = matches!(tool, "get_outline" | "design_context");
    let mut inspect = Inspect {
        file: file.clone(),
        tool,
        page: None,
        layer: None,
        args: serde_json::Map::new(),
    };
    let mut i = 2;
    while i < args.len() {
        let flag = args[i].as_str();
        let takes = match flag {
            "--page" | "--layer" => on_page,
            "--depth" | "--limit" => tool == "get_outline",
            "--query" => !on_page,
            _ => false,
        };
        if !takes {
            return Err(format!("{read} takes no {flag}\n{USAGE}"));
        }
        i += 1;
        let value = args
            .get(i)
            .filter(|v| !v.starts_with('-'))
            .ok_or_else(|| format!("{flag} wants a value\n{USAGE}"))?
            .clone();
        match flag {
            "--page" => inspect.page = Some(value),
            "--layer" => inspect.layer = Some(value),
            "--query" => {
                inspect.args.insert("query".into(), json!(value));
            }
            _ => {
                let n: u64 = value
                    .parse()
                    .map_err(|_| format!("{flag} wants a whole number\n{USAGE}"))?;
                inspect.args.insert(flag[2..].into(), json!(n));
            }
        }
        i += 1;
    }
    Ok(inspect)
}

/// The read's answer, pretty-printed.
pub(crate) fn run(inspect: &Inspect) -> Result<String, String> {
    let file = &inspect.file;
    let bytes = std::fs::read(file).map_err(|e| format!("cannot read {file}: {e}"))?;
    let doc = mastik_core::io::load(&bytes).map_err(|e| format!("cannot load {file}: {e}"))?;
    let mut s = EditorSession::new();
    s.adopt_document(doc, None);
    s.page = crate::chosen_page(&s.doc, inspect.page.as_deref())?;
    let mut args = inspect.args.clone();
    if let Some(layer) = &inspect.layer {
        let ids =
            crate::chosen_layers(&s.doc, std::slice::from_ref(layer), inspect.page.as_deref())?;
        if let Some(id) = ids.first() {
            args.insert("id".into(), json!(id.to_wire()));
        }
    }
    let answer = crate::mcp::run_offline(&mut s, inspect.tool, &Value::Object(args))
        .map_err(|e| format!("{file}: {e}"))?;
    Ok(serde_json::to_string_pretty(&answer).unwrap_or_default())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn inspect(s: &[&str]) -> Result<Inspect, String> {
        parse(&s.iter().map(|a| a.to_string()).collect::<Vec<_>>())
    }

    /// Each read takes only the options its MCP read has.
    #[test]
    fn inspect_takes_each_reads_options() {
        let i = inspect(&["a.mastik", "outline", "--depth", "3", "--layer", "Card"]).unwrap();
        assert_eq!(i.tool, "get_outline");
        assert_eq!(i.args["depth"], json!(3));
        assert_eq!(i.layer.as_deref(), Some("Card"));
        let i = inspect(&["a.mastik", "styles", "--query", "body"]).unwrap();
        assert_eq!(i.args["query"], json!("body"));
        assert!(inspect(&["a.mastik", "styles", "--depth", "2"]).is_err());
        assert!(inspect(&["a.mastik", "outline", "--query", "x"]).is_err());
        assert!(inspect(&["a.mastik", "context", "--limit", "2"]).is_err());
        assert!(inspect(&["a.mastik", "outline", "--depth", "two"]).is_err());
        assert!(inspect(&["a.mastik", "fonts"]).is_err());
        assert!(inspect(&["a.mastik"]).is_err());
    }

    /// The outline is the MCP's, read from a file.
    #[test]
    fn inspect_outline_reads_the_page() {
        let dir = std::env::temp_dir().join(format!("mastik-inspect-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let mut ids = mastik_core::IdSource::new(1);
        let mut doc = mastik_core::Document::new(ids.mint());
        let page = doc.first_page();
        let card = ids.mint();
        doc.apply(&mastik_core::Transaction(vec![
            mastik_core::Operation::CreateNode {
                id: card,
                parent: page,
                index: 0,
                kind: mastik_core::NodeKind::Group,
                transform: None,
                name: Some("Card".into()),
            },
        ]))
        .unwrap();
        let file = dir.join("a.mastik");
        std::fs::write(&file, mastik_core::io::save(&doc).unwrap()).unwrap();
        let i = inspect(&[file.to_str().unwrap(), "outline"]).unwrap();
        let answer: Value = serde_json::from_str(&run(&i).unwrap()).unwrap();
        assert!(answer.to_string().contains("\"Card\""), "{answer}");
        let _ = std::fs::remove_dir_all(&dir);
    }
}
