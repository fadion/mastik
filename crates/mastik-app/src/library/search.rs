//! How the library ranks a name against a typed query: one rule for the search
//! dropdown, the results page and the command palette's documents, so the three
//! cannot drift apart.
//!
//! Every word of the query must match. Each word is ranked on its own — a prefix
//! of the name, the start of one of its words, or anywhere in it — and the name
//! takes its worst word's tier, so word order does not matter. A name equal to
//! the whole query is exact and beats them all. Callers break ties by recency.

use std::ops::Range;

/// How well a name answers a query, best first.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum Tier {
    /// The name is the query.
    Exact,
    /// Every word starts the name or is the name's own prefix.
    Prefix,
    /// Every word starts a word of the name.
    WordStart,
    /// Some word only appears inside the name.
    Substring,
}

/// A name's answer to a query: its tier and the byte ranges of the name to light.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Hit {
    pub tier: Tier,
    pub lit: Vec<Range<usize>>,
}

/// The query's words, case folded: whitespace-separated, empty when nothing but
/// spaces was typed.
pub fn words(query: &str) -> Vec<String> {
    query.split_whitespace().map(str::to_lowercase).collect()
}

/// Rank `name` against the query's `words` ([`words`]); `None` unless every word
/// is in the name. The lit ranges are empty for a name whose case folding moves
/// its byte offsets, which only exotic Unicode does.
pub fn rank(name: &str, words: &[String]) -> Option<Hit> {
    if words.is_empty() {
        return None;
    }
    let folded = name.to_lowercase();
    let offsets_kept = folded.len() == name.len();
    // Where each word of the name starts, by the name's own letters and digits.
    let starts: Vec<usize> = folded
        .char_indices()
        .filter(|&(i, c)| {
            c.is_alphanumeric()
                && folded[..i]
                    .chars()
                    .next_back()
                    .is_none_or(|p| !p.is_alphanumeric())
        })
        .map(|(i, _)| i)
        .collect();
    let mut worst = Tier::Exact;
    let mut lit = Vec::new();
    for w in words {
        let (tier, at) = if folded.starts_with(w.as_str()) {
            (Tier::Prefix, 0)
        } else if let Some(&at) = starts
            .iter()
            .find(|&&s| folded[s..].starts_with(w.as_str()))
        {
            (Tier::WordStart, at)
        } else {
            (Tier::Substring, folded.find(w.as_str())?)
        };
        worst = worst.max(tier);
        if offsets_kept {
            lit.push(at..at + w.len());
        }
    }
    // The whole query, read as a phrase, can do better than its worst word.
    if folded
        .split_whitespace()
        .eq(words.iter().map(String::as_str))
    {
        worst = Tier::Exact;
    } else if folded.starts_with(&words.join(" ")) {
        worst = worst.min(Tier::Prefix);
    }
    lit.sort_by_key(|r| r.start);
    // Two words lighting overlapping letters light their union.
    let mut merged: Vec<Range<usize>> = Vec::new();
    for r in lit {
        match merged.last_mut() {
            Some(last) if r.start <= last.end => last.end = last.end.max(r.end),
            _ => merged.push(r),
        }
    }
    Some(Hit {
        tier: worst,
        lit: merged,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tier(name: &str, q: &str) -> Option<Tier> {
        rank(name, &words(q)).map(|h| h.tier)
    }

    /// The ladder: exact, then prefix, then word start, then substring.
    #[test]
    fn a_name_is_ranked_exact_then_prefix_then_word_start_then_substring() {
        assert_eq!(tier("Checkout", "checkout"), Some(Tier::Exact));
        assert_eq!(tier("Checkout flow v2", "check"), Some(Tier::Prefix));
        assert_eq!(tier("Mobile checkout", "check"), Some(Tier::WordStart));
        assert_eq!(
            tier("Quickcheck onboarding", "check"),
            Some(Tier::Substring)
        );
        assert_eq!(tier("Wallet", "check"), None);
    }

    /// Every word must match, each ranked alone, the row taking its worst; order
    /// does not matter.
    /// flip: `worst.max(tier)` → `worst.min(tier)` → fails at the `mob check` tier.
    #[test]
    fn every_word_must_match_and_the_worst_word_decides() {
        assert_eq!(tier("Mobile checkout", "mob check"), Some(Tier::WordStart));
        assert_eq!(tier("Mobile checkout", "check mob"), Some(Tier::WordStart));
        assert_eq!(tier("Mobile checkout", "mob heck"), Some(Tier::Substring));
        assert_eq!(tier("Mobile checkout", "mob wallet"), None);
        // A phrase the name starts with is a prefix, whatever its last word.
        assert_eq!(tier("Checkout flow v2", "checkout fl"), Some(Tier::Prefix));
        assert_eq!(tier("Mobile checkout", "  "), None);
    }

    /// The letters each word matched are lit, as the palette lights its hits.
    #[test]
    fn the_matched_letters_are_lit() {
        let hit = rank("Mobile Checkout", &words("check mo")).unwrap();
        assert_eq!(hit.lit, vec![0..2, 7..12]);
        let hit = rank("Quickcheck", &words("check")).unwrap();
        assert_eq!(hit.lit, vec![5..10]);
    }
}
