//! Short ids (§8.1a): a client that asked for them, never a chat, is shown
//! `n48c9_36` for `48c991654e6b8e81:36`, the shortest start of the actor's hex
//! (`NodeId::to_short`) that no other actor of the document begins with. The
//! spelling depends on the document alone, so an id an agent holds names the
//! same layer on a new connection or another host. Ids read back through
//! `NodeId::from_wire` inside `mastik_core::with_short_ids`, so only a place
//! read as an id takes the short form; the answer is rewritten on its way out.

use mastik_core::{Document, NodeId};
use mastik_mcp::wire::{Content, Output};
use serde_json::Value;

/// The actors an id may be written short for, sorted: those any id the
/// document holds was minted by (a layer's, a variable's, a style's, a
/// script's…), and the session's own. Actor 0, which a migration's item ids
/// carry, is left out, so a text reading `0:30` stays as written.
pub(crate) fn known_actors(doc: &Document, own: u64) -> Vec<u64> {
    let mut known: Vec<u64> = mastik_core::minted_ids(doc)
        .map(|id| id.actor)
        .chain([own])
        .filter(|a| *a != 0)
        .collect();
    known.sort_unstable();
    known.dedup();
    known
}

/// Rewrite every full id of a `known` actor in `out` to its short form. Image
/// and file contents keep their bytes. A text that is the structure's own
/// serialization is written again from the shortened structure, so it keeps the
/// document's words as stored; any other text is rewritten as flat text.
pub(crate) fn shorten(out: &mut Output, known: &[u64]) {
    let copy = out.structured.as_ref().map(Value::to_string);
    if let Some(v) = out.structured.as_mut() {
        shorten_value(v, known);
    }
    for c in &mut out.content {
        let Content::Text(text) = c else {
            continue;
        };
        if let Some(v) = &out.structured
            && copy.as_ref() == Some(text)
        {
            *text = v.to_string();
        } else if let Some(short) = shorten_text(text, known) {
            *text = short;
        }
    }
}

/// The keys whose values are the document's own words, never ids: a text, a
/// name, a description or a script's source stays as stored, so what an agent
/// copies back is what was there.
const WORDS: [&str; 4] = ["text", "name", "description", "source"];

/// A layer's `styles`, each slot (`text` among them) naming a style by its id,
/// so no slot's value is stored words.
fn shorten_style_links(v: &mut Value, known: &[u64]) {
    match v {
        Value::Object(map) => map.values_mut().for_each(|s| shorten_value(s, known)),
        other => shorten_value(other, known),
    }
}

fn shorten_value(v: &mut Value, known: &[u64]) {
    match v {
        Value::String(s) => {
            if let Some(short) = shorten_text(s, known) {
                *s = short;
            }
        }
        Value::Array(items) => items.iter_mut().for_each(|i| shorten_value(i, known)),
        Value::Object(map) => {
            let keys: Vec<String> = map.keys().cloned().collect();
            for key in keys {
                let Some(mut value) = map.remove(&key) else {
                    continue;
                };
                if key == "styles" {
                    shorten_style_links(&mut value, known);
                } else if !(WORDS.contains(&key.as_str()) && value.is_string()) {
                    shorten_value(&mut value, known);
                }
                let key = shorten_text(&key, known).unwrap_or(key);
                map.insert(key, value);
            }
        }
        _ => {}
    }
}

/// Write every script `source` in `args`, at any depth (a `batch` step's, a
/// `call`'s), with its short ids in full: a stored script runs outside MCP,
/// where only the full form reads.
pub(crate) fn expand_sources(args: &mut Value, known: &[u64]) {
    match args {
        Value::Array(items) => items.iter_mut().for_each(|i| expand_sources(i, known)),
        Value::Object(map) => {
            for (key, value) in map.iter_mut() {
                match value {
                    Value::String(s) if key == "source" => {
                        if let Some(full) = expand_text(s, known) {
                            *s = full;
                        }
                    }
                    _ => expand_sources(value, known),
                }
            }
        }
        _ => {}
    }
}

/// `text` with each short id of a `known` actor written in full, `None` when
/// none is. A short id is `n`, a hex run, `_` and a decimal run, standing
/// alone as [`shorten_text`]'s ids do.
fn expand_text(text: &str, known: &[u64]) -> Option<String> {
    let bytes = text.as_bytes();
    let word = |b: u8| b.is_ascii_alphanumeric() || b == b'_';
    let mut out = String::new();
    let mut copied = 0;
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] != b'n' || (i > 0 && word(bytes[i - 1])) {
            i += 1;
            continue;
        }
        let mut k = i + 1;
        while k < bytes.len() && word(bytes[k]) {
            k += 1;
        }
        if let Some(id) = NodeId::from_short(&text[i..k], known) {
            out.push_str(&text[copied..i]);
            out.push_str(&id.to_wire());
            copied = k;
        }
        i = k;
    }
    (copied > 0).then(|| {
        out.push_str(&text[copied..]);
        out
    })
}

/// `text` with each full id of a `known` actor written short, `None` when
/// none is. An id is a lowercase hex run, `:` and a decimal run, standing
/// alone: no letter, digit or `_` touches either end.
fn shorten_text(text: &str, known: &[u64]) -> Option<String> {
    let bytes = text.as_bytes();
    let word = |b: u8| b.is_ascii_alphanumeric() || b == b'_';
    let hex = |b: u8| b.is_ascii_digit() || (b'a'..=b'f').contains(&b);
    let mut out = String::new();
    let mut copied = 0;
    let mut i = 0;
    while i < bytes.len() {
        if !hex(bytes[i]) || (i > 0 && word(bytes[i - 1])) {
            i += 1;
            continue;
        }
        let start = i;
        let mut j = i;
        while j < bytes.len() && hex(bytes[j]) {
            j += 1;
        }
        let colon = j;
        let mut k = colon + 1;
        while k < bytes.len() && bytes[k].is_ascii_digit() {
            k += 1;
        }
        let whole = colon < bytes.len()
            && bytes[colon] == b':'
            && k > colon + 1
            && (k == bytes.len() || !word(bytes[k]));
        let id = whole.then(|| NodeId::from_wire(&text[start..k])).flatten();
        let Some(id) = id else {
            i = j.max(i + 1);
            continue;
        };
        let Some(short) = id.to_short(known) else {
            i = k;
            continue;
        };
        out.push_str(&text[copied..start]);
        out.push_str(&short);
        copied = k;
        i = k;
    }
    (copied > 0).then(|| {
        out.push_str(&text[copied..]);
        out
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Full ids of a known actor are written short wherever they stand, and
    /// anything else that reads like one is left alone.
    ///
    /// flip: `shorten_text` taking a run that a letter touches → fails at
    /// `"xaa:36" stays`.
    #[test]
    fn known_ids_are_written_short_and_nothing_else() {
        let known = [0xaa, 0xbb];
        let said = shorten_text("`aa:36` beside bb:7 and aa:36.", &known);
        assert_eq!(said.as_deref(), Some("`naa_36` beside nbb_7 and naa_36."));
        for s in [
            "10:30", "xaa:36", "aa:36x", "aa:", "AA:36", "aa:36_1", "cc:1",
        ] {
            assert_eq!(shorten_text(s, &known), None, "{s:?} stays");
        }
    }

    /// A script's source is stored with its ids in full, and a stored text,
    /// name or source is answered as stored, while an id beside them is
    /// written short.
    ///
    /// flip: `shorten_value` without its `WORDS` arm → fails at "a text stays";
    /// without its `styles` arm → fails at "a text style's link";
    /// `expand_sources` skipping a step → fails at "a batch step's source".
    #[test]
    fn stored_words_keep_their_ids_in_full() {
        let known = [0xaa];
        let mut args = serde_json::json!({
            "source": "set_name(\"naa_36\", \"Hero\") -- not nab_1 or xnaa_2",
            "steps": [{ "args": { "source": "print(naa_7)" } }],
        });
        expand_sources(&mut args, &known);
        assert_eq!(
            args["source"],
            "set_name(\"aa:36\", \"Hero\") -- not nab_1 or xnaa_2"
        );
        assert_eq!(
            args["steps"][0]["args"]["source"], "print(aa:7)",
            "a batch step's source"
        );
        let mut answer = serde_json::json!({
            "id": "aa:3",
            "text": "see aa:3",
            "name": "aa:4",
            "source": "x = aa:5",
        });
        shorten_value(&mut answer, &known);
        assert_eq!(answer["id"], "naa_3");
        assert_eq!(answer["text"], "see aa:3", "a text stays");
        assert_eq!(answer["name"], "aa:4");
        assert_eq!(answer["source"], "x = aa:5");
        let mut tree = serde_json::json!({ "styles": { "text": "aa:27", "fills": "aa:28" } });
        shorten_value(&mut tree, &known);
        assert_eq!(
            tree["styles"],
            serde_json::json!({ "text": "naa_27", "fills": "naa_28" }),
            "a text style's link"
        );
    }

    /// An answer's text copy, the half most harnesses hand the model, keeps a
    /// stored text's ids in full as its structure does; a free text (a refusal)
    /// is still written short.
    ///
    /// flip: `shorten` rewriting a structured answer's text copy as flat text →
    /// fails at "the text copy keeps the text".
    #[test]
    fn an_answers_text_copy_keeps_stored_words() {
        let known = [0xaa];
        let mut out = Output::json(serde_json::json!({ "text": "see aa:3", "id": "aa:3" }));
        shorten(&mut out, &known);
        let Content::Text(copy) = &out.content[0] else {
            panic!("a text copy");
        };
        assert!(
            copy.contains("\"see aa:3\""),
            "the text copy keeps the text: {copy}"
        );
        assert!(copy.contains("\"naa_3\""), "the id is short: {copy}");
        assert_eq!(out.structured.as_ref().unwrap()["id"], "naa_3");
        let mut refused = Output::error("no layer aa:3");
        shorten(&mut refused, &known);
        assert!(
            matches!(&refused.content[0], Content::Text(t) if t == "no layer naa_3"),
            "a free text is written short"
        );
    }

    /// An actor that minted only a style stays known after the session's own
    /// actor changes, so a short id for that style still reads.
    ///
    /// flip: `known_actors` reading the layers' ids alone → fails at "the
    /// style's actor".
    #[test]
    fn an_actor_known_only_by_a_style_stays_known() {
        let mut pages = mastik_core::IdSource::new(0xaa);
        let mut doc = Document::new(pages.mint());
        let mut styles = mastik_core::IdSource::new(0xbb);
        let style = styles.mint_style();
        doc.apply(&mastik_core::Transaction(vec![
            mastik_core::Operation::AddStyle {
                style: mastik_core::style::Style {
                    id: style,
                    name: "Surface".into(),
                    value: mastik_core::style::StyleValue::Fills(Vec::new()),
                    origin: None,
                    forked: None,
                    description: String::new(),
                },
            },
        ]))
        .unwrap();
        let known = known_actors(&doc, 0xcc);
        assert!(known.contains(&0xbb), "the style's actor: {known:x?}");
    }
}
