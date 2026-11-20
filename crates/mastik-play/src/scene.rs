//! What a shell draws: the frames on screen and the patches over the copy.

use mastik_core::kurbo::{Affine, Point, Rect, RoundedRectRadii, Size};
use mastik_core::peniko::Color;
use mastik_core::{Effect, Keyed, NodeId, Paint};

/// One frame of play to draw, in **screen space**: the current screen frame's box
/// with its top-left at the origin, one unit a document pixel. The shell scales
/// and places that box (the page's *Scaling*), and maps pointer events back into
/// it.
#[derive(Clone, Debug, PartialEq)]
pub struct Scene {
    /// The page the screen frame is on.
    pub page: NodeId,
    /// The screen frame's size.
    pub size: Size,
    /// The frames to draw, bottom first: the screen (two of them while a
    /// transition runs), then each open overlay.
    pub layers: Vec<Layer>,
    /// Changes to draw over the copy's nodes this frame — scroll offsets, a
    /// transition's in-between values, a fade. A node with no patch draws as the
    /// copy holds it.
    pub patches: Vec<Patch>,
    /// While the hotspot hints flash after a click on nothing, how strongly, 1
    /// down to 0.
    pub hint_flash: Option<f32>,
}

/// One frame drawn whole: its subtree, clipped to its box, with the frame's
/// top-left at [`Self::at`].
#[derive(Clone, Debug, PartialEq)]
pub struct Layer {
    pub frame: NodeId,
    /// Where the frame's top-left is drawn, in screen space.
    pub at: Point,
    /// Multiplies the whole frame, the dissolve's and fade's factor.
    pub opacity: f32,
    /// A colour to cover everything beneath this layer with before drawing it —
    /// an overlay's backdrop, already faded by its transition.
    pub backdrop: Option<Color>,
    /// Whether this is an overlay rather than the screen.
    pub overlay: bool,
}

/// What one node draws with this frame instead of what the copy holds; each
/// `None` keeps the copy's. Shaped like the renderer's per-node override: a
/// local transform, a box size, a paint with literal brushes, an opacity and a
/// visibility.
#[derive(Clone, Debug, PartialEq)]
pub struct Patch {
    pub node: NodeId,
    /// The node's local transform, replacing where layout placed it.
    pub transform: Option<Affine>,
    /// The node's box size (a frame's, a rectangle's), replacing its used size.
    pub size: Option<Size>,
    /// The node's own opacity, replacing it.
    pub opacity: Option<f32>,
    pub visible: Option<bool>,
    /// The node's paint, its brushes literal.
    pub paint: Option<Paint>,
    /// The node's corner radii (a frame's, a rectangle's; a polygon's or star's
    /// one radius as the top-left).
    pub radii: Option<RoundedRectRadii>,
    /// The node's effect stack, its numbers literal.
    pub effects: Option<Vec<Keyed<Effect>>>,
}

impl Patch {
    /// A patch on `node` that changes nothing yet.
    pub fn new(node: NodeId) -> Self {
        Patch {
            node,
            transform: None,
            size: None,
            opacity: None,
            visible: None,
            paint: None,
            radii: None,
            effects: None,
        }
    }
}

/// A place the person can act on, for the hints.
#[derive(Clone, Debug, PartialEq)]
pub struct Hotspot {
    pub layer: NodeId,
    /// In screen space.
    pub rect: Rect,
}
