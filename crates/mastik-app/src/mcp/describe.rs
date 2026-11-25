//! `describe_tools` and the refusal of a tool no host serves, both searching
//! every tool's definition, listed or not.

use super::access;
use serde_json::{Value, json};

/// `describe_tools`: the full definition of each tool named in `names`, then of
/// those whose name holds `query`, then of those whose description alone does,
/// then of those holding each of its words, then of those whose name holds one,
/// at most [`DESCRIBED`]; with `brief`, every match's name and description only.
/// A prototype model's name (`Reaction`, `Action`, `LayerPlay`) is answered in
/// `models` with its fields or variants; any other name that is no tool is
/// answered in `not_found`. A chat at `level` is
/// offered only what that level runs; a tool named above it, or one `query`
/// matches that a higher level runs, is answered in `beyond_access` with the
/// level it needs.
pub(super) fn describe_tools(args: &Value, level: Option<access::Level>) -> Value {
    let within = |d: &Value| {
        let name = d["name"].as_str().unwrap_or_default();
        level.is_none_or(|l| access::usable(l, name))
    };
    let (all, beyond): (Vec<Value>, Vec<Value>) = every_definition().into_iter().partition(within);
    let names: Vec<&str> = args
        .get("names")
        .and_then(Value::as_array)
        .map(|n| n.iter().filter_map(Value::as_str).collect())
        .unwrap_or_default();
    let query = args
        .get("query")
        .and_then(Value::as_str)
        .map(str::to_lowercase);
    let brief = args.get("brief").and_then(Value::as_bool).unwrap_or(false);
    // A query of several words is also looked for word by word, since an agent
    // writes "unsplash photo" where no text holds the two together.
    let words: Vec<String> = query
        .as_deref()
        .map(|q| {
            q.split(|c: char| !c.is_alphanumeric())
                .filter(|w| w.len() > 1)
                .map(str::to_owned)
                .collect()
        })
        .unwrap_or_default();
    // 0 named, 1 a name holding the query, 2 a description holding it, 3 every
    // word in the name or description, 4 a word in the name.
    let rank = |d: &Value| {
        let name = d["name"].as_str().unwrap_or_default();
        if names.contains(&name) {
            return Some(0);
        }
        let q = query.as_deref()?;
        if name.contains(q) {
            return Some(1);
        }
        let about = d["description"].as_str().unwrap_or_default().to_lowercase();
        if about.contains(q) {
            return Some(2);
        }
        if words.len() > 1
            && words
                .iter()
                .all(|w| name.contains(w.as_str()) || about.contains(w.as_str()))
        {
            return Some(3);
        }
        (words.len() > 1 && words.iter().any(|w| name.contains(w.as_str()))).then_some(4)
    };
    let mut matching: Vec<(u8, &Value)> =
        all.iter().filter_map(|d| rank(d).map(|r| (r, d))).collect();
    matching.sort_by_key(|(r, _)| *r);
    // A named tool a higher level drops is neither beyond this chat's access nor
    // missing; it is left out of both.
    let higher = |n: &str| needs(n).is_none_or(|l| level.is_none_or(|at| l > at));
    // A model a schema names (`A [Reaction]`) is answered with its shape, since
    // no example shows every action or trigger.
    let models: serde_json::Map<String, Value> = names
        .iter()
        .filter_map(|n| Some(((*n).to_owned(), mastik_core::verbs::model_shape(n)?)))
        .collect();
    let (mut beyond_access, not_found): (Vec<&str>, Vec<&str>) = names
        .iter()
        .filter(|n| !all.iter().any(|d| d["name"] == **n))
        .filter(|n| !models.contains_key(**n))
        .filter(|n| !beyond.iter().any(|d| d["name"] == **n) || higher(n))
        .partition(|n| beyond.iter().any(|d| d["name"] == **n));
    // A query's matches above the level too, the ones a higher level runs: an
    // agent that searched for a thing it may not do would otherwise conclude
    // that no tool does it, and could not tell the person what would. A tool a
    // higher level drops (`propose_edit` at Edit) is no such thing.
    let mut above: Vec<(u8, &str)> = beyond
        .iter()
        .filter_map(|d| {
            let name = d["name"].as_str()?;
            let r = rank(d).filter(|&r| r > 0)?;
            needs(name)
                .is_some_and(|l| level.is_none_or(|at| l > at))
                .then_some((r, name))
        })
        .collect();
    above.sort_by_key(|(r, _)| *r);
    for (_, name) in above.into_iter().take(DESCRIBED) {
        if !beyond_access.contains(&name) {
            beyond_access.push(name);
        }
    }
    let (tools, more): (Vec<Value>, usize) = if brief {
        let tools = matching
            .iter()
            .map(|(_, d)| {
                let full = d["description"].as_str().unwrap_or_default();
                json!({ "name": d["name"], "description": gist(full) })
            })
            .collect();
        (tools, 0)
    } else {
        let more = matching.len().saturating_sub(DESCRIBED);
        let tools = matching
            .into_iter()
            .take(DESCRIBED)
            .map(|(_, d)| d.clone())
            .collect();
        (tools, more)
    };
    // A tool outside the core list is not one a client listing only the core can
    // call by name: it answers "no such tool" before Mastik hears of it.
    let unlisted: Vec<&str> = tools
        .iter()
        .filter_map(|d| d["name"].as_str())
        .filter(|n| !mastik_mcp::tools::CORE.contains(n))
        .collect();
    let mut answer = json!({ "tools": tools, "more": more });
    if !unlisted.is_empty() {
        answer["unlisted"] = json!({
            "tools": unlisted,
            "said": "Not in the core list: where your client lists only it, run these through `call`, as {\"tool\": <name>, \"args\": {…}}",
        });
    }
    if !models.is_empty() {
        answer["models"] = Value::Object(models);
    }
    if !not_found.is_empty() {
        answer["not_found"] = json!(not_found);
    }
    if !beyond_access.is_empty() {
        let entries: Vec<Value> = beyond_access
            .iter()
            .map(|name| match needs(name) {
                Some(l) => json!({ "name": name, "needs": l.label() }),
                None => json!({ "name": name, "needs": "no chat" }),
            })
            .collect();
        answer["beyond_access"] = json!(entries);
        answer["said"] = json!(
            "`beyond_access` runs only at the access it `needs`, above this chat's: \
             don't call it; tell the person, who can switch the chat's access."
        );
    }
    answer
}

/// The lowest access level that can use `tool` ([`access::usable`]), `None`
/// when no chat's can.
fn needs(tool: &str) -> Option<access::Level> {
    access::Level::ALL
        .into_iter()
        .find(|&l| access::usable(l, tool))
}

/// Every tool's `tools/list` entry, listed or not.
fn every_definition() -> Vec<Value> {
    use mastik_mcp::tools::{ASSISTANT, PHASE_1, PHASE_2, PHASE_3, PHASE_4};
    PHASE_1
        .iter()
        .chain(&PHASE_2)
        .chain(&PHASE_3)
        .chain(&PHASE_4)
        .chain(&ASSISTANT)
        .filter_map(|name| mastik_mcp::schema::definition(name))
        .collect()
}

/// Why `name` is refused as no tool, with the nearest tools by `describe_tools`'
/// search run on each of its words: a word in a tool's name counts twice one in
/// its description. `tools/list` names only the core set, so pointing there
/// alone does not find the rest.
pub(super) fn no_such_tool(name: &str) -> String {
    let words: Vec<String> = name
        .to_lowercase()
        .split(|c: char| !c.is_alphanumeric())
        .filter(|w| w.len() > 1)
        .map(str::to_owned)
        .collect();
    let mut scored: Vec<(usize, String, String)> = every_definition()
        .iter()
        .filter_map(|d| {
            let tool = d["name"].as_str()?;
            let about = d["description"].as_str().unwrap_or_default();
            let lower = about.to_lowercase();
            let score: usize = words
                .iter()
                .map(
                    |w| match (tool.contains(w.as_str()), lower.contains(w.as_str())) {
                        (true, _) => 2,
                        (false, true) => 1,
                        _ => 0,
                    },
                )
                .sum();
            (score > 0).then(|| (score, tool.to_owned(), gist(about)))
        })
        .collect();
    scored.sort_by(|a, b| b.0.cmp(&a.0).then(a.1.len().cmp(&b.1.len())));
    let nearest: Vec<String> = scored
        .iter()
        .take(NEAREST)
        .map(|(_, tool, about)| format!("`{tool}`: {about}"))
        .collect();
    match nearest.is_empty() {
        true => format!(
            "No tool is called `{name}`; `describe_tools` with a `query` finds one by a word."
        ),
        false => format!(
            "No tool is called `{name}`. Nearest: {} `describe_tools` with a `query` finds others.",
            nearest.join(" ")
        ),
    }
}

/// How many tools an unknown name's refusal suggests.
const NEAREST: usize = 3;

/// The longest `gist` a brief `describe_tools` gives, in characters.
const GIST: usize = 120;

/// A tool's description cut to what a brief listing needs to pick it: its first
/// sentence, and of a long one the words up to [`GIST`] characters.
fn gist(description: &str) -> String {
    let first = description
        .split_once(". ")
        .map_or(description, |(s, _)| s)
        .trim_end_matches('.');
    if first.chars().count() <= GIST {
        return format!("{first}.");
    }
    let cut: String = first.chars().take(GIST).collect();
    let at = cut.rfind(' ').unwrap_or(cut.len());
    format!("{}…", cut[..at].trim_end_matches([',', ':', ';']))
}

/// The most full definitions one `describe_tools` answers.
const DESCRIBED: usize = 12;

#[cfg(test)]
mod tests {
    use super::*;
    use crate::mcp::Engine;
    use crate::mcp::tests::{One, call, ok};

    /// `call` of a tool that does not exist suggests the nearest by the words of
    /// its name, and points at `describe_tools`.
    ///
    /// flip: `no_such_tool` scoring a description's word as nothing → fails at
    /// `set_geometry`, which only its description reaches.
    #[test]
    fn an_unknown_tool_suggests_the_nearest() {
        let (mut engine, mut docs) = (Engine::default(), One::new());
        let out = call(
            &mut engine,
            &mut docs,
            1,
            "call",
            json!({ "tool": "set_corner_radius", "args": {} }),
        );
        assert!(out.is_error);
        let text = format!("{:?}", out.content);
        assert!(text.contains("`set_geometry`"), "{text}");
        assert!(text.contains("describe_tools"), "{text}");
    }

    /// `describe_tools` answers the tools whose name holds the query before those
    /// whose description alone does, answers names and a short line each when
    /// `brief`, and names what it did not find.
    ///
    /// flip: `describe_tools` not sorting by rank → fails at the first tool's name,
    /// as `get_tree` leads; a brief listing the whole description → fails at "a
    /// brief line is short".
    #[test]
    fn describe_tools_ranks_name_matches_first() {
        let (mut engine, mut docs) = (Engine::default(), One::new());
        let found = call(
            &mut engine,
            &mut docs,
            1,
            "describe_tools",
            json!({ "query": "style" }),
        );
        let tools = ok(&found)["tools"].as_array().unwrap().clone();
        let named: Vec<bool> = tools
            .iter()
            .map(|t| t["name"].as_str().unwrap().contains("style"))
            .collect();
        assert!(named[0], "{tools:?}");
        assert!(
            named.windows(2).all(|w| w[0] || !w[1]),
            "name matches first: {named:?}"
        );
        let found = call(
            &mut engine,
            &mut docs,
            1,
            "describe_tools",
            json!({ "query": "style", "brief": true, "names": ["no_such_tool"] }),
        );
        let answer = ok(&found);
        let tools = answer["tools"].as_array().unwrap();
        assert!(tools.iter().any(|t| t["name"] == "add_style"), "{tools:?}");
        assert!(tools.iter().all(|t| t.get("inputSchema").is_none()));
        let longest = tools
            .iter()
            .map(|t| t["description"].as_str().unwrap().chars().count())
            .max();
        assert!(
            longest <= Some(GIST + 1),
            "a brief line is short: {longest:?}"
        );
        assert_eq!(answer["not_found"], json!(["no_such_tool"]), "not found");
    }

    /// A model a schema names is answered with its shape: every action with its
    /// fields, a struct's fields, the example beside them.
    ///
    /// flip: `describe_tools` without its `models` → fails at "the actions".
    #[test]
    fn describe_tools_answers_a_model_by_its_shape() {
        let (mut engine, mut docs) = (Engine::default(), One::new());
        let found = call(
            &mut engine,
            &mut docs,
            1,
            "describe_tools",
            json!({ "names": ["Action", "[Reaction]", "OverlaySettings"] }),
        );
        let answer = ok(&found);
        let models = &answer["models"];
        assert_eq!(
            models["Action"]["variants"]["EditRow"],
            json!(["table", "row", "cells"]),
            "the actions: {answer}"
        );
        assert!(
            models["Action"]["variants"]["Back"].is_null(),
            "a bare word"
        );
        assert!(models["[Reaction]"]["example"].is_array(), "the example");
        assert!(
            models["OverlaySettings"]["fields"]
                .to_string()
                .contains("close_outside"),
            "a struct's fields"
        );
        assert!(answer.get("not_found").is_none(), "{answer}");
    }

    /// A query of words no text holds together still finds the tools holding
    /// each word: "unsplash photo" finds the photo search.
    ///
    /// flip: `describe_tools` without its word-by-word rank → fails at "found".
    #[test]
    fn describe_tools_matches_a_query_word_by_word() {
        let (mut engine, mut docs) = (Engine::default(), One::new());
        let found = call(
            &mut engine,
            &mut docs,
            1,
            "describe_tools",
            json!({ "query": "unsplash photo", "brief": true }),
        );
        let tools = ok(&found)["tools"].as_array().unwrap().clone();
        assert!(
            tools.iter().any(|t| t["name"] == "search_assets"),
            "found: {tools:?}"
        );
    }

    /// A tool outside the core list is answered as unlisted, with how to run it,
    /// and a core tool is not.
    ///
    /// flip: `describe_tools` without its `unlisted` answer → fails at "boolean".
    #[test]
    fn describe_tools_names_what_is_unlisted() {
        let (mut engine, mut docs) = (Engine::default(), One::new());
        let found = call(
            &mut engine,
            &mut docs,
            1,
            "describe_tools",
            json!({ "names": ["boolean", "set"] }),
        );
        let unlisted = ok(&found)["unlisted"].clone();
        assert_eq!(unlisted["tools"], json!(["boolean"]), "boolean: {unlisted}");
        assert!(unlisted["said"].as_str().unwrap().contains("`call`"));
    }
}
