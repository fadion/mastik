//! Short ids per client (§8.1a): a client other than a chat is shown `n36` for
//! `48c991654e6b8e81:36`, a fifth of the tokens, by a table of actors kept per
//! client in the order the client first saw them. Ids read back through
//! `NodeId::from_wire` inside `mastik_core::with_short_ids`, so only a place
//! read as an id takes the short form; the answer is rewritten on its way out.
//!
//! The proxy keeps the table from each answer's `_meta` and hands it back when
//! it restores the document on another host, so ids the agent holds still name
//! the same layers there.

use mastik_core::{Document, NodeId};
use mastik_mcp::wire::{Content, Output};
use serde_json::Value;
use std::collections::HashSet;

/// The actors an id in an answer may be written short for: those the
/// document's layers were minted by, and the session's own. Actor 0, which a
/// migration's item ids carry, is left out, so a text reading `0:30` stays as
/// written.
pub(crate) fn known_actors(doc: &Document, own: u64) -> HashSet<u64> {
    doc.node_ids()
        .map(|id| id.actor)
        .chain([own])
        .filter(|a| *a != 0)
        .collect()
}

/// Rewrite every full id of a `known` actor in `out` to its short form,
/// adding actors new to `table` at its end. Image and file contents keep
/// their bytes.
pub(crate) fn shorten(out: &mut Output, known: &HashSet<u64>, table: &mut Vec<u64>) {
    if let Some(v) = out.structured.as_mut() {
        shorten_value(v, known, table);
    }
    for c in &mut out.content {
        if let Content::Text(text) = c
            && let Some(short) = shorten_text(text, known, table)
        {
            *text = short;
        }
    }
}

fn shorten_value(v: &mut Value, known: &HashSet<u64>, table: &mut Vec<u64>) {
    match v {
        Value::String(s) => {
            if let Some(short) = shorten_text(s, known, table) {
                *s = short;
            }
        }
        Value::Array(items) => items
            .iter_mut()
            .for_each(|i| shorten_value(i, known, table)),
        Value::Object(map) => {
            let keys: Vec<String> = map.keys().cloned().collect();
            for key in keys {
                let Some(mut value) = map.remove(&key) else {
                    continue;
                };
                shorten_value(&mut value, known, table);
                let key = shorten_text(&key, known, table).unwrap_or(key);
                map.insert(key, value);
            }
        }
        _ => {}
    }
}

/// `text` with each full id of a `known` actor written short, `None` when
/// none is. An id is a lowercase hex run, `:` and a decimal run, standing
/// alone: no letter, digit or `_` touches either end.
fn shorten_text(text: &str, known: &HashSet<u64>, table: &mut Vec<u64>) -> Option<String> {
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
        let id = whole
            .then(|| NodeId::from_wire(&text[start..k]))
            .flatten()
            .filter(|id| known.contains(&id.actor) || table.contains(&id.actor));
        let Some(id) = id else {
            i = j.max(i + 1);
            continue;
        };
        if !table.contains(&id.actor) {
            table.push(id.actor);
        }
        let Some(short) = id.to_short(table) else {
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
    /// flip: `shorten_text` dropping the `known` filter → fails at "an unknown
    /// actor stays".
    #[test]
    fn known_ids_are_written_short_and_nothing_else() {
        let known: HashSet<u64> = [0xaa, 0xbb].into();
        let mut table = Vec::new();
        let said = shorten_text("`aa:36` beside bb:7 and aa:36.", &known, &mut table);
        assert_eq!(said.as_deref(), Some("`n36` beside n1_7 and n36."));
        assert_eq!(table, [0xaa, 0xbb]);
        for s in ["10:30", "xaa:36", "aa:36x", "aa:", "AA:36", "aa:36_1"] {
            assert_eq!(shorten_text(s, &known, &mut table), None, "{s:?} stays");
        }
        assert_eq!(table, [0xaa, 0xbb], "an unknown actor stays");
    }
}
