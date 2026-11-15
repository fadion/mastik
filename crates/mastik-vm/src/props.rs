//! The properties a layer collection reads and writes, in one table that the
//! runtime, completion, the error suggestions and the `.d.luau` generator all
//! read, and each property's value on one layer.

use mastik_core::build::{self, PaintShown};
use mastik_core::{Brush, Document, NodeId, NodeKind, Resolved};

/// What a property's value is.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PropType {
    Number,
    Text,
    Bool,
    /// A colour as `"#rrggbb"` or `"#rrggbbaa"`.
    Colour,
    /// A layer collection.
    Layers,
}

impl PropType {
    /// The type as a `.d.luau` file spells it.
    pub fn luau(self) -> &'static str {
        match self {
            PropType::Number => "number",
            PropType::Text | PropType::Colour => "string",
            PropType::Bool => "boolean",
            PropType::Layers => "Layers",
        }
    }
}

/// One property of a layer collection.
#[derive(Clone, Copy, Debug)]
pub struct Property {
    pub name: &'static str,
    pub ty: PropType,
    pub doc: &'static str,
    pub writable: bool,
    /// How a disagreement counts it: "3 widths".
    pub plural: &'static str,
}

/// Shorthand for a [`PROPERTIES`] row.
const fn prop(
    name: &'static str,
    ty: PropType,
    writable: bool,
    plural: &'static str,
    doc: &'static str,
) -> Property {
    Property {
        name,
        ty,
        doc,
        writable,
        plural,
    }
}

/// Every property, in the order completion lists them. Geometry is world space:
/// `x`, `y` are the left and top of the layer's bounds on the page.
pub static PROPERTIES: &[Property] = &[
    prop(
        "x",
        PropType::Number,
        true,
        "x positions",
        "Left of the layer's bounds on the page",
    ),
    prop(
        "y",
        PropType::Number,
        true,
        "y positions",
        "Top of the layer's bounds on the page",
    ),
    prop(
        "width",
        PropType::Number,
        true,
        "widths",
        "Width of the layer's box; `50%` sizes a laid-out item by its container",
    ),
    prop(
        "height",
        PropType::Number,
        true,
        "heights",
        "Height of the layer's box; `50%` sizes a laid-out item by its container",
    ),
    prop(
        "rotation",
        PropType::Number,
        true,
        "rotations",
        "Angle in degrees, clockwise on the page",
    ),
    prop(
        "opacity",
        PropType::Number,
        true,
        "opacities",
        "Opacity, 0 to 1",
    ),
    prop(
        "fill",
        PropType::Colour,
        true,
        "fills",
        "The one solid fill as \"#rrggbb\"; nil for none",
    ),
    prop(
        "stroke",
        PropType::Colour,
        true,
        "strokes",
        "The one solid stroke as \"#rrggbb\"; nil for none",
    ),
    prop("name", PropType::Text, true, "names", "The layer's name"),
    prop(
        "visible",
        PropType::Bool,
        true,
        "visibilities",
        "Whether the layer is shown",
    ),
    prop(
        "locked",
        PropType::Bool,
        true,
        "lock states",
        "Whether the layer is locked",
    ),
    prop(
        "corner_radius",
        PropType::Number,
        true,
        "corner radii",
        "The radius of every corner",
    ),
    prop(
        "text",
        PropType::Text,
        true,
        "texts",
        "A text layer's characters",
    ),
    prop(
        "blend",
        PropType::Text,
        true,
        "blend modes",
        "How the layer blends with what is under it",
    ),
    prop(
        "id",
        PropType::Text,
        false,
        "ids",
        "The layer's id, \"<actor-hex>:<seq>\"",
    ),
    prop(
        "kind",
        PropType::Text,
        false,
        "kinds",
        "What the layer is: rect, ellipse, text, artboard, group…",
    ),
    prop(
        "parent",
        PropType::Layers,
        false,
        "parents",
        "The layer holding these",
    ),
    prop(
        "children",
        PropType::Layers,
        false,
        "",
        "The layers inside these, bottom first",
    ),
    prop(
        "prev",
        PropType::Layers,
        false,
        "",
        "The sibling below a single layer; nil at the bottom",
    ),
    prop(
        "next",
        PropType::Layers,
        false,
        "",
        "The sibling above a single layer; nil at the top",
    ),
];

/// A method of a layer collection: its name, its Luau signature after `self`, and a line.
pub struct Method {
    pub name: &'static str,
    pub params: &'static str,
    pub returns: &'static str,
    pub doc: &'static str,
}

/// Every method a collection takes as `sel:name(…)`.
pub static METHODS: &[Method] = &[
    Method {
        name: "shared",
        params: "name: string",
        returns: "any",
        doc: "The value every layer shares, or nil when they differ",
    },
    Method {
        name: "adjust",
        params: "name: string, by: number | (number) -> number",
        returns: "()",
        doc: "Add `by` to each layer's own value, or map each through a function",
    },
    Method {
        name: "select",
        params: "",
        returns: "()",
        doc: "Select these layers once the run ends",
    },
];

/// Names agents and designers guess for a property, with the one each means.
pub(crate) const GUESSES: &[(&str, &str)] = &[
    ("alpha", "opacity"),
    ("color", "fill"),
    ("colour", "fill"),
    ("background", "fill"),
    ("border", "stroke"),
    ("radius", "corner_radius"),
    ("border_radius", "corner_radius"),
    ("hidden", "visible"),
    ("angle", "rotation"),
    ("w", "width"),
    ("h", "height"),
    ("left", "x"),
    ("top", "y"),
    ("content", "text"),
    ("characters", "text"),
    ("label", "name"),
    ("type", "kind"),
];

/// The property called `name`.
pub fn find(name: &str) -> Option<&'static Property> {
    PROPERTIES.iter().find(|p| p.name == name)
}

/// One property's value on one layer.
#[derive(Clone, Debug, PartialEq)]
pub(crate) enum Val {
    Nil,
    Num(f64),
    Str(String),
    Bool(bool),
    Layers(Vec<NodeId>),
    /// The layer itself holds several values: a group whose shapes differ. Says why.
    Mixed(String),
}

impl Val {
    /// The value as the bar shows it in a list of disagreeing values.
    pub(crate) fn shown(&self, doc: &Document) -> String {
        match self {
            Val::Nil => "nil".into(),
            Val::Num(n) => crate::format_number(*n),
            Val::Str(s) => format!("\"{s}\""),
            Val::Bool(b) => b.to_string(),
            Val::Layers(ids) => crate::describe(doc, ids),
            Val::Mixed(_) => "mixed".into(),
        }
    }
}

/// How near two values of `prop` must be to read as one: the inspector's
/// tolerance for opacity, and the radius tolerance for every other number.
fn tolerance(prop: &str) -> f64 {
    if prop == "opacity" {
        f64::from(build::OPACITY_AGREEMENT)
    } else {
        build::RADIUS_AGREEMENT
    }
}

/// Whether `a` and `b` read as one value of `prop`.
pub(crate) fn agree(prop: &str, a: &Val, b: &Val) -> bool {
    match (a, b) {
        (Val::Num(a), Val::Num(b)) => (a - b).abs() <= tolerance(prop),
        (Val::Mixed(_), _) | (_, Val::Mixed(_)) => false,
        (a, b) => a == b,
    }
}

/// `kind` as the snapshot names it.
pub fn kind_name(kind: &NodeKind) -> &'static str {
    match kind {
        NodeKind::Root => "root",
        NodeKind::Artboard { .. } => "artboard",
        NodeKind::Group => "group",
        NodeKind::Rect { .. } => "rect",
        NodeKind::Ellipse { .. } => "ellipse",
        NodeKind::Polygon { .. } => "polygon",
        NodeKind::Star { .. } => "star",
        NodeKind::Line { .. } => "line",
        NodeKind::Spiral { .. } => "spiral",
        NodeKind::Arc { .. } => "arc",
        NodeKind::Path { .. } => "path",
        NodeKind::Text { .. } => "text",
        NodeKind::Slice { .. } => "slice",
        NodeKind::Boolean { .. } => "boolean",
    }
}

/// `rgba` as `"#rrggbb"`, or `"#rrggbbaa"` when it is not opaque.
pub(crate) fn hex(rgba: [u8; 4]) -> String {
    let [r, g, b, a] = rgba;
    if a == 255 {
        format!("#{r:02x}{g:02x}{b:02x}")
    } else {
        format!("#{r:02x}{g:02x}{b:02x}{a:02x}")
    }
}

/// The paint value of a list of `(brush, visible)` items: one solid colour as
/// hex, none as `Nil`, and anything else — a gradient, a picture, several, a
/// variable's colour — as `Mixed`.
fn paint_value(items: &[(&mastik_core::BrushSource, bool)], what: &str) -> Val {
    match items {
        [] => Val::Nil,
        [(_, false)] => Val::Mixed(format!("its {what} is hidden")),
        [(brush, true)] => match brush.literal() {
            Some(Brush::Solid(c)) => Val::Str(hex(c.to_rgba8().to_u8_array())),
            _ => Val::Mixed(format!("its {what} is not one solid colour")),
        },
        _ => Val::Mixed(format!("it has several {what}s")),
    }
}

/// `prop` of the layer `id`; `Err` when the layer no longer exists.
pub(crate) fn read(doc: &Document, res: &Resolved, id: NodeId, prop: &str) -> Result<Val, String> {
    let node = doc
        .get(id)
        .ok_or_else(|| format!("No layer `{}`: it was deleted", id.to_wire()))?;
    let num = |v: Option<f64>| v.map_or(Val::Nil, Val::Num);
    Ok(match prop {
        "x" => num(mastik_core::query::bounds(res, id).map(|b| b.x0)),
        "y" => num(mastik_core::query::bounds(res, id).map(|b| b.y0)),
        "width" => num(mastik_core::query::local_box(doc, res, id).map(|b| b.width())),
        "height" => num(mastik_core::query::local_box(doc, res, id).map(|b| b.height())),
        "rotation" => num(res.world_transform(id).map(|w| {
            let degrees = mastik_core::edit::shown_angle(node.kind(), w).to_degrees();
            if degrees.abs() < 1e-9 { 0.0 } else { degrees }
        })),
        // The stored `f32` read back as the decimal it was written as.
        "opacity" => Val::Num((f64::from(node.opacity()) * 1e6).round() / 1e6),
        "fill" => match build::shared_fills(doc, &[id]) {
            PaintShown::Mixed => Val::Mixed("the layers inside it differ".into()),
            PaintShown::List(list) => {
                let items: Vec<_> = list.iter().map(|f| (&f.brush, f.visible)).collect();
                paint_value(&items, "fill")
            }
        },
        "stroke" => match build::shared_strokes(doc, &[id]) {
            PaintShown::Mixed => Val::Mixed("the layers inside it differ".into()),
            PaintShown::List(list) => {
                let items: Vec<_> = list.iter().map(|s| (&s.brush, s.visible)).collect();
                paint_value(&items, "stroke")
            }
        },
        "name" => Val::Str(node.name().to_owned()),
        "visible" => Val::Bool(node.visible()),
        "locked" => Val::Bool(node.locked()),
        "corner_radius" => match node.kind().corner_radii() {
            None => Val::Nil,
            Some(r) => mastik_core::geometry::uniform_radius(&r)
                .map_or_else(|| Val::Mixed("its corners differ".into()), Val::Num),
        },
        "text" => match node.kind() {
            NodeKind::Text { content, .. } => Val::Str(content.clone()),
            _ => Val::Nil,
        },
        "blend" => Val::Str(node.blend().label().to_owned()),
        "id" => Val::Str(id.to_wire()),
        "kind" => Val::Str(kind_name(node.kind()).to_owned()),
        "parent" => Val::Layers(node.parent().into_iter().collect()),
        "children" => Val::Layers(node.children().to_vec()),
        "prev" | "next" => {
            let Some(siblings) = node.parent().and_then(|p| doc.get(p)).map(|p| p.children())
            else {
                return Ok(Val::Nil);
            };
            let at = siblings.iter().position(|s| *s == id);
            let other = at.and_then(|i| {
                if prop == "prev" {
                    i.checked_sub(1)
                } else {
                    Some(i + 1)
                }
            });
            other
                .and_then(|i| siblings.get(i))
                .map_or(Val::Nil, |s| Val::Layers(vec![*s]))
        }
        other => return Err(format!("No property “{other}”")),
    })
}
