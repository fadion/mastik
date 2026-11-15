//! Document scripts: Luau source carried in a document under a name.
//!
//! **A document stores its scripts and never runs them.** Nothing in core runs
//! one, and nothing anywhere runs one on open or on a change: a run is started by
//! hand, in the app. A script moves between a document and the library as a
//! copy, never a link, so a script here has no origin to follow.
//!
//! The list is ordered — the order the document's script list shows — and
//! changed only by [`crate::Operation::AddScript`], [`crate::Operation::RemoveScript`],
//! [`crate::Operation::SetScriptName`] and [`crate::Operation::SetScriptSource`].

use crate::id::NodeId;
use serde::{Deserialize, Deserializer, Serialize, Serializer};

/// A script's identity: a [`NodeId`] under a newtype, minted from the session's
/// one [`crate::IdSource`] as a style's is, so `reserve_existing_ids` covers it.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Ord, PartialOrd)]
pub struct ScriptId(pub NodeId);

impl std::fmt::Debug for ScriptId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "script {:?}", self.0)
    }
}

/// Written as the node id's wire form, `"<actor-hex>:<seq>"`.
impl Serialize for ScriptId {
    fn serialize<S: Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        s.serialize_str(&self.0.to_wire())
    }
}

impl<'de> Deserialize<'de> for ScriptId {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        let s = String::deserialize(d)?;
        NodeId::from_wire(&s)
            .map(ScriptId)
            .ok_or_else(|| serde::de::Error::custom(format!("bad id {s:?}")))
    }
}

impl crate::IdSource {
    /// A fresh [`ScriptId`].
    pub fn mint_script(&mut self) -> ScriptId {
        ScriptId(self.mint())
    }
}

/// One stored script: its name in the script list and its Luau source, as typed.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Script {
    pub id: ScriptId,
    pub name: String,
    pub source: String,
}

/// The stem a new script's name is numbered from: `Untitled 1`, `Untitled 2`.
pub const UNTITLED: &str = "Untitled";

/// `wanted` if no script in `scripts` has that name, else its stem numbered past
/// the scripts that do — `Card grid` → `Card grid 2`, `Untitled 1` → `Untitled 2`.
pub fn free_name(scripts: &[Script], wanted: &str) -> String {
    let taken: rustc_hash::FxHashSet<&str> = scripts.iter().map(|s| s.name.as_str()).collect();
    if !taken.contains(wanted) {
        return wanted.to_owned();
    }
    match crate::naming::numbered(wanted) {
        Some((stem, n)) => crate::naming::free_name(&taken, stem, n.saturating_add(1)),
        None => crate::naming::free_name(&taken, wanted, 2),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn script(name: &str) -> Script {
        Script {
            id: ScriptId(NodeId { actor: 1, seq: 1 }),
            name: name.into(),
            source: String::new(),
        }
    }

    /// A free name is kept; a taken one is numbered past itself.
    #[test]
    fn a_taken_name_is_numbered() {
        let list = [script("Card grid"), script("Untitled 1")];
        assert_eq!(free_name(&list, "Tidy"), "Tidy");
        assert_eq!(free_name(&list, "Card grid"), "Card grid 2");
        assert_eq!(free_name(&list, "Untitled 1"), "Untitled 2");
    }
}
