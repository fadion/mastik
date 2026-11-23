//! The prototype player's engine (`docs/design/play.md`).
//!
//! A [`Player`] plays a copy of a document: it clones the document, changes the
//! copy only through its own door (`Document::apply` after `propagate::owed`,
//! with no history), and holds everything that exists only while a prototype
//! plays — the screen, *Back*'s history, overlays, play values, scroll offsets,
//! hover and press, timers, motion. A shell (the app's player, the exported
//! page) feeds it [`Input`]s in screen space and time through
//! [`Player::tick`], and draws its [`Scene`]: the copy, with patches over it.
//!
//! Headless and meant to build for the browser: no rendering or UI dependency,
//! and Luau only behind the default `scripts` feature.

#![deny(rustdoc::broken_intra_doc_links, rustdoc::invalid_html_tags)]
#![allow(rustdoc::private_intra_doc_links)]

mod actions;
mod anim;
mod drag;
mod input;
mod live;
pub mod motion;
mod player;
mod rows;
mod scene;
mod script;
mod take;

#[cfg(test)]
mod tests;

pub use input::Input;
pub use player::{
    Fallback, ModeRow, Notice, OpenedRow, Player, Request, Start, StartError, ValueError, ValueRow,
};
pub use rows::TableRow;
pub use scene::{Hotspot, Layer, Patch, Scene};
pub use take::{Replay, Take};
