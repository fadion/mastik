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
/// and file contents keep their bytes.
pub(crate) fn shorten(out: &mut Output, known: &[u64]) {
    if let Some(v) = out.structured.as_mut() {
        shorten_value(v, known);
    }
    for c in &mut out.content {
        if let Content::Text(text) = c
            && let Some(short) = shorten_text(text, known)
        {
            *text = short;
        }
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
                shorten_value(&mut value, known);
                let key = shorten_text(&key, known).unwrap_or(key);
                map.insert(key, value);
            }
        }
        _ => {}
    }
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
