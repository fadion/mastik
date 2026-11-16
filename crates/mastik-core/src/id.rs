//! Node identity (§5.2, invariant 3).
//!
//! `NodeId = (actor, seq)`: a random per-session `actor` plus a session-local
//! monotonic `seq`. Ids are minted by the caller (`IdSource`) and carried inside
//! operations — `apply` never allocates. This makes op replay deterministic,
//! the property future multiplayer needs. Ids are never reused after deletion.

use serde::{Deserialize, Serialize};

/// Stable, never-reused node identity. See module docs.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Ord, PartialOrd, Serialize, Deserialize)]
pub struct NodeId {
    pub actor: u64,
    pub seq: u64,
}

impl std::fmt::Debug for NodeId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        // Serialized/diagnostic form: "<actor-hex>:<seq>".
        write!(f, "{:x}:{}", self.actor, self.seq)
    }
}

impl NodeId {
    /// An id no document holds — `seq` 0 is never minted ([`IdSource::new`]).
    /// Asked as a place, it sits under no frame and on no page, so it reads every
    /// variable in the document's default mode.
    pub const NOWHERE: NodeId = NodeId { actor: 0, seq: 0 };

    /// Compact, diff-friendly on-disk form: `"<actor-hex>:<seq>"` (§5.2).
    pub fn to_wire(self) -> String {
        format!("{:x}:{}", self.actor, self.seq)
    }

    /// Parse the wire form. Returns `None` on malformed input. Inside
    /// [`with_short_ids`] it also reads the short form against those actors.
    pub fn from_wire(s: &str) -> Option<NodeId> {
        let Some((actor, seq)) = s.split_once(':') else {
            return SHORT_ACTORS.with(|t| NodeId::from_short(s, t.borrow().as_deref()?));
        };
        Some(NodeId {
            actor: u64::from_str_radix(actor, 16).ok()?,
            seq: seq.parse().ok()?,
        })
    }

    /// The short form an MCP client is shown, `n<prefix>_<seq>`: the shortest
    /// start of the actor's hex, at least [`SHORT_PREFIX`] digits, that no
    /// other of `actors` begins with. It depends only on the actors, never on
    /// what a client saw first, so an id an agent holds names the same layer on
    /// any connection. `None` for an actor `actors` lacks, or whose whole hex
    /// another begins with.
    pub fn to_short(self, actors: &[u64]) -> Option<String> {
        if !actors.contains(&self.actor) {
            return None;
        }
        let hex = format!("{:x}", self.actor);
        let others: Vec<String> = actors
            .iter()
            .filter(|a| **a != self.actor)
            .map(|a| format!("{a:x}"))
            .collect();
        (SHORT_PREFIX.min(hex.len())..=hex.len())
            .map(|n| &hex[..n])
            .find(|p| !others.iter().any(|o| o.starts_with(p)))
            .map(|p| format!("n{p}_{}", self.seq))
    }

    /// Read [`Self::to_short`]'s form against `actors`: a prefix exactly one of
    /// them begins with, and the seq in digits with no sign or leading zero, so
    /// one id has one spelling. A prefix two actors share is `None`, never a
    /// guess: a new actor can make an earlier spelling ambiguous.
    pub fn from_short(s: &str, actors: &[u64]) -> Option<NodeId> {
        let (prefix, seq) = s.strip_prefix('n')?.split_once('_')?;
        let seq = (!seq.is_empty()
            && seq.bytes().all(|b| b.is_ascii_digit())
            && (seq == "0" || !seq.starts_with('0')))
        .then(|| seq.parse::<u64>().ok())
        .flatten()?;
        if prefix.len() < SHORT_PREFIX && !actors.iter().any(|a| format!("{a:x}") == prefix) {
            return None;
        }
        let mut matching = actors
            .iter()
            .filter(|a| format!("{a:x}").starts_with(prefix));
        let actor = *matching.next()?;
        if matching.next().is_some() {
            return None;
        }
        Some(NodeId { actor, seq })
    }
}

/// The fewest hex digits of its actor a short id spells: with the handful of
/// actors a document has, two random ones rarely share three, and when they
/// do the spelling lengthens rather than names the wrong one.
pub const SHORT_PREFIX: usize = 3;

thread_local! {
    /// The actors [`NodeId::from_wire`] reads the short form against, set by
    /// [`with_short_ids`] while a call from an MCP client runs.
    static SHORT_ACTORS: std::cell::RefCell<Option<Vec<u64>>> =
        const { std::cell::RefCell::new(None) };
}

/// Run `f` with [`NodeId::from_wire`] also reading the short form against
/// `actors`, so every place an argument is read as an id takes it, and a text
/// that happens to read `n1a2b_1` is never touched. The actors before are put
/// back after, a panic included.
pub fn with_short_ids<R>(actors: &[u64], f: impl FnOnce() -> R) -> R {
    struct Restore(Option<Vec<u64>>);
    impl Drop for Restore {
        fn drop(&mut self) {
            let was = self.0.take();
            SHORT_ACTORS.with(|t| *t.borrow_mut() = was);
        }
    }
    let _restore = Restore(SHORT_ACTORS.with(|t| t.replace(Some(actors.to_vec()))));
    f()
}

/// Per-session id minter. One is shared by every writer in a session (GUI,
/// `serve`, MCP clients acting through them), so all ids in a session share an
/// `actor` and never collide. A random `actor` avoids cross-session collisions
/// without coordination.
pub struct IdSource {
    actor: u64,
    next_seq: u64,
}

impl IdSource {
    /// Create a source with a caller-provided random actor id.
    ///
    /// Randomness lives outside core (core has no clock/RNG policy); callers
    /// pass a random `u64`. `seq` starts at 1 so `0` is never a valid `seq`.
    pub fn new(actor: u64) -> Self {
        Self { actor, next_seq: 1 }
    }

    /// Mint the next unique id for this session.
    pub fn mint(&mut self) -> NodeId {
        let id = NodeId {
            actor: self.actor,
            seq: self.next_seq,
        };
        self.next_seq += 1;
        id
    }

    pub fn actor(&self) -> u64 {
        self.actor
    }

    /// The seq the next [`mint`](Self::mint) will use. Exposed for diagnostics
    /// and for [`crate::document::reserve_existing_ids`].
    pub fn next_seq(&self) -> u64 {
        self.next_seq
    }

    /// Move the counter forward so the next mint is at least `seq`. Never moves
    /// it backwards — `seq` is monotonic per actor, which is what guarantees a
    /// deleted node's id is never re-minted (§5.2).
    pub fn skip_to(&mut self, seq: u64) {
        self.next_seq = self.next_seq.max(seq);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mint_is_monotonic_and_unique() {
        let mut src = IdSource::new(0xABCD);
        let a = src.mint();
        let b = src.mint();
        assert_eq!(a.actor, 0xABCD);
        assert_eq!(a.seq, 1);
        assert_eq!(b.seq, 2);
        assert_ne!(a, b);
    }

    #[test]
    fn debug_form_is_actor_hex_colon_seq() {
        let id = NodeId {
            actor: 0x1a2b,
            seq: 42,
        };
        assert_eq!(format!("{id:?}"), "1a2b:42");
    }

    /// Pins what `NodeId::from_wire`, which reads untrusted bytes, accepts today.
    ///
    /// Five spellings (a leading `+` on either half, uppercase hex, leading zeroes
    /// on either half) are aliases `to_wire` never produces, and `"0:0"` names an
    /// id the minter never mints. Each alias parses to the same `NodeId` as its
    /// canonical form, so the duplicate check still catches a file spelling one
    /// node twice. Whether to reject them is an open ruling (it narrows what
    /// `io::load` accepts); moving a row from `accepted` to the rejected list makes
    /// that edit visible. `"1a2b:"` and `":42"` are already rejected: an empty
    /// half does not default to zero.
    #[test]
    fn from_wire_accepts_the_canonical_form_and_five_aliases_of_it() {
        let canonical = NodeId {
            actor: 0x1a2b,
            seq: 42,
        };
        // Every spelling that parses, and what it parses to. The first row is the
        // only one `to_wire` produces; the rest are aliases a hand-edited or
        // foreign file can carry.
        let accepted: [(&str, NodeId); 8] = [
            ("1a2b:42", canonical),
            ("+1a2b:42", canonical),
            ("1A2B:42", canonical),
            ("0001a2b:42", canonical),
            ("1a2b:+42", canonical),
            ("1a2b:042", canonical),
            ("0:0", NodeId { actor: 0, seq: 0 }),
            (
                "ffffffffffffffff:1",
                NodeId {
                    actor: u64::MAX,
                    seq: 1,
                },
            ),
        ];
        for (s, want) in accepted {
            assert_eq!(
                NodeId::from_wire(s),
                Some(want),
                "{s:?} is accepted today; tightening this is a ruling, not a repair"
            );
        }

        // And every spelling that does not. Whitespace is the one worth having: a
        // pretty-printer that indented a JSON string value would otherwise make
        // ids resolve by accident.
        for s in [
            " 1a2b:42",
            "1a2b:42 ",
            "-1:2",
            "1a2b:",
            ":42",
            "1a2b",
            "1a2b:42:1",
            "g:1",
            "1a2b:-1",
            "",
        ] {
            assert_eq!(NodeId::from_wire(s), None, "{s:?} must not parse");
        }

        // The round trip, in the direction that matters: what the writer produces
        // the reader reads back, and spells the same way again.
        for id in [
            canonical,
            NodeId { actor: 0, seq: 1 },
            NodeId {
                actor: u64::MAX,
                seq: u64::MAX,
            },
        ] {
            let wire = id.to_wire();
            assert_eq!(NodeId::from_wire(&wire), Some(id), "{wire} round-trips");
            assert_eq!(
                NodeId::from_wire(&wire).unwrap().to_wire(),
                wire,
                "and re-spells identically, which is what makes a diff readable"
            );
        }
    }

    /// The short form spells the shortest unique start of the actor's hex,
    /// whatever order the actors come in; it round-trips, is read only inside
    /// `with_short_ids`, reads a longer unique start too (what it was written
    /// as before another actor joined), and a prefix two actors share is read
    /// as no id.
    ///
    /// flip: `to_short` with a fixed three-digit prefix → fails at "lengthened
    /// past a shared start"; `from_short` taking the first match → fails at
    /// "ambiguous".
    #[test]
    fn the_short_form_spells_a_unique_start_of_the_actor() {
        let (a, b, c) = (
            0x48c9_9165_4e6b_8e81,
            0x7c3a_0000_0000_0001,
            0x48c9_7777_0000_0002,
        );
        let id = |actor, seq| NodeId { actor, seq };
        let two = [a, b];
        assert_eq!(id(a, 36).to_short(&two).as_deref(), Some("n48c_36"));
        assert_eq!(
            id(b, 7).to_short(&[b, a]).as_deref(),
            Some("n7c3_7"),
            "order is no part of it"
        );
        let three = [a, b, c];
        assert_eq!(
            id(a, 36).to_short(&three).as_deref(),
            Some("n48c99_36"),
            "lengthened past a shared start"
        );
        assert_eq!(id(0xcc, 1).to_short(&two), None, "an actor the list lacks");
        assert_eq!(
            id(0xcc, 1).to_short(&[0xcc]).as_deref(),
            Some("ncc_1"),
            "a short actor whole"
        );
        assert_eq!(
            NodeId::from_wire("n48c9_36"),
            None,
            "no actors, no short form"
        );
        with_short_ids(&two, || {
            assert_eq!(NodeId::from_wire("n48c_36"), Some(id(a, 36)));
            assert_eq!(NodeId::from_wire("n7c3_7"), Some(id(b, 7)));
            assert_eq!(
                NodeId::from_wire("n48c99_36"),
                Some(id(a, 36)),
                "a longer start"
            );
            assert_eq!(
                NodeId::from_wire("48c991654e6b8e81:36"),
                Some(id(a, 36)),
                "the full form still reads"
            );
            for s in [
                "n", "n36", "n48c_036", "n48_1", "n48c_", "n48c_+3", "N48c_1", "n48c_1 ", "n48cz_1",
            ] {
                assert_eq!(NodeId::from_wire(s), None, "{s:?} must not parse");
            }
        });
        with_short_ids(&three, || {
            assert_eq!(NodeId::from_wire("n48c_36"), None, "ambiguous");
            assert_eq!(NodeId::from_wire("n48c99_36"), Some(id(a, 36)));
        });
        assert_eq!(
            NodeId::from_wire("n48c9_36"),
            None,
            "the actors are put back after"
        );
    }

    /// `skip_to` never moves the counter backwards, which keeps a deleted node's id
    /// from being re-minted. `document::reserve_existing_ids` calls it with `seq + 1`
    /// for every id in a loaded file: a high id pushes the counter past it, a low
    /// one must not pull it back.
    ///
    /// flip: `self.next_seq = seq` in place of the `max` → fails at "a lower seq
    /// must not pull the counter back"; the other assertions stay green.
    #[test]
    fn skip_to_only_ever_moves_the_counter_forward() {
        let mut src = IdSource::new(0xABCD);
        assert_eq!(src.next_seq(), 1, "fixture: a fresh source starts at 1");
        src.mint();
        src.mint();
        assert_eq!(src.next_seq(), 3);

        src.skip_to(2);
        assert_eq!(
            src.next_seq(),
            3,
            "a lower seq must not pull the counter back — that is how a deleted \
             node's id gets re-minted"
        );
        src.skip_to(100);
        assert_eq!(src.next_seq(), 100, "and a higher one moves it");
        assert_eq!(
            src.mint().seq,
            100,
            "the next mint is the seq that was asked for"
        );

        // `reserve_existing_ids` passes `seq + 1`, which is what makes an id in
        // the file unreachable rather than merely equalled.
        let mut src = IdSource::new(0xABCD);
        let loaded_seq = 7u64;
        src.skip_to(loaded_seq + 1);
        assert!(
            src.mint().seq > loaded_seq,
            "a live id from the file can never be handed back"
        );
    }
}
