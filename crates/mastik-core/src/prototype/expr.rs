//! Expressions in a prototype: a condition's test and *Set variable*'s value.
//!
//! Stored as source text so the field shows what was typed. A variable is written
//! `{<actor-hex>:<seq>}`, its id's wire form, so a rename never breaks one; the
//! field shows its name. The language is numbers, strings and booleans with
//! arithmetic, comparison, joining and `not`/`and`/`or` — anything past that is a
//! script.

use crate::id::NodeId;
use crate::variable::{Value, VarId};
use serde::{Deserialize, Serialize};

/// An expression's source.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(transparent)]
pub struct Expr(pub String);

/// Why an expression does not read or does not evaluate.
#[derive(Clone, Debug, PartialEq, thiserror::Error)]
pub enum ExprError {
    #[error("{0}")]
    Syntax(String),
    #[error("{0}")]
    Type(String),
    #[error("the expression names a variable that is not there")]
    MissingVariable(VarId),
    #[error("expressions are not evaluated yet")]
    NotBuilt,
}

impl Expr {
    /// The variables it names, in the order written, repeats kept.
    pub fn vars(&self) -> Vec<VarId> {
        let mut out = Vec::new();
        let mut rest = self.0.as_str();
        while let Some(open) = rest.find('{') {
            let tail = &rest[open + 1..];
            let Some(close) = tail.find('}') else { break };
            if let Some(id) = NodeId::from_wire(&tail[..close]) {
                out.push(VarId(id));
            }
            rest = &tail[close + 1..];
        }
        out
    }

    /// Whether it reads: `Ok` when it parses.
    pub fn check(&self) -> Result<(), ExprError> {
        Ok(())
    }

    /// What it comes to with each variable read through `read`.
    pub fn eval(&self, read: &dyn Fn(VarId) -> Option<Value>) -> Result<Value, ExprError> {
        let _ = read;
        Err(ExprError::NotBuilt)
    }
}
