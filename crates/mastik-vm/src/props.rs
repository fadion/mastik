//! The properties a layer collection reads and writes, in one table that the
//! runtime, completion, the error suggestions and the `.d.luau` generator all
//! read, and each property's value on one layer.

use mastik_core::build::{self, PaintShown};
use mastik_core::container::Display;
use mastik_core::typography::{CharAttr, CharAttrKind, Length};
use mastik_core::{Document, NodeId, NodeKind, Operation, Resolved};

/// What a property's value is.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PropType {
    Number,
    Text,
    Bool,
    /// One paint: a colour as `"#rrggbb"` or `"#rrggbbaa"`, any other paint as
    /// a table.
    Colour,
    /// A layer collection.
    Layers,
    /// A table in the model's own shape, as the verb writing it takes it.
    Table,
    /// A length: a number of px, or a string such as `"0.05em"`.
    Length,
    /// One number for every side, or four, top first and clockwise.
    Sides,
}

impl PropType {
    /// The type as a `.d.luau` file spells it.
    pub fn luau(self) -> &'static str {
        match self {
            PropType::Number => "number",
            PropType::Text => "string",
            PropType::Colour => "string | { [string]: any }",
            PropType::Bool => "boolean",
            PropType::Layers => "Layers",
            PropType::Table => "{ [any]: any }",
            PropType::Length => "number | string",
            PropType::Sides => "number | { number }",
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
        "The one fill: a colour as \"#rrggbb\", a gradient or picture as a table; nil for none",
    ),
    prop(
        "stroke",
        PropType::Colour,
        true,
        "strokes",
        "The one stroke: a colour as \"#rrggbb\", else a table; nil for none",
    ),
    prop(
        "fills",
        PropType::Table,
        true,
        "fill lists",
        "Every fill, bottom first: a plain colour as \"#rrggbb\", any other as a table",
    ),
    prop(
        "strokes",
        PropType::Table,
        true,
        "stroke lists",
        "Every stroke, bottom first, each a table: its brush, width, align, dashes",
    ),
    prop(
        "effects",
        PropType::Table,
        true,
        "effect lists",
        "Shadows and blurs, in order, each a table such as { DropShadow = { … } }",
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
        "font_family",
        PropType::Text,
        true,
        "font families",
        "A text's font family",
    ),
    prop(
        "font_size",
        PropType::Number,
        true,
        "font sizes",
        "A text's font size in px",
    ),
    prop(
        "font_weight",
        PropType::Number,
        true,
        "font weights",
        "A text's weight, 100 to 900: 400 regular, 700 bold",
    ),
    prop(
        "italic",
        PropType::Bool,
        true,
        "italics",
        "Whether a text is italic",
    ),
    prop(
        "line_height",
        PropType::Length,
        true,
        "line heights",
        "A text's line height: px as a number, \"1.4em\" of the font size; nil for auto",
    ),
    prop(
        "letter_spacing",
        PropType::Length,
        true,
        "letter spacings",
        "A text's letter spacing: px as a number, or \"0.05em\" of the font size",
    ),
    prop(
        "text_case",
        PropType::Text,
        true,
        "text cases",
        "A text's case: \"Original\", \"Upper\", \"Lower\" or \"Title\"",
    ),
    prop(
        "text_align",
        PropType::Text,
        true,
        "text alignments",
        "A text's alignment: \"Start\", \"End\", \"Left\", \"Center\", \"Right\" or \"Justify\"",
    ),
    prop(
        "layout",
        PropType::Table,
        true,
        "layouts",
        "How a frame lays out its children: { Flex = { … } } or { Grid = { … } }; nil for none",
    ),
    prop(
        "direction",
        PropType::Text,
        true,
        "directions",
        "A flex layout's direction: \"Row\", \"Column\", \"RowReverse\" or \"ColumnReverse\"",
    ),
    prop(
        "gap",
        PropType::Number,
        true,
        "gaps",
        "The gap between a flex or grid layout's children, rows and columns alike",
    ),
    prop(
        "padding",
        PropType::Sides,
        true,
        "paddings",
        "A flex or grid layout's padding: one number, or { top, right, bottom, left }",
    ),
    prop(
        "layout_item",
        PropType::Table,
        true,
        "layout items",
        "How a layer sizes and places itself in its parent's layout: grow, basis, align_self…",
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
    ("family", "font_family"),
    ("font", "font_family"),
    ("weight", "font_weight"),
    ("leading", "line_height"),
    ("tracking", "letter_spacing"),
    ("case", "text_case"),
    ("shadow", "effects"),
    ("shadows", "effects"),
    ("spacing", "gap"),
    ("display", "layout"),
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
    /// A value in the model's own shape, written short as the verbs read it
    /// back (`verbs::shorten`): a paint list, an effect, a layout.
    Json(serde_json::Value),
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
            Val::Json(v) => {
                let s = v.to_string();
                match s.char_indices().nth(40) {
                    Some((at, _)) => format!("{}…", &s[..at]),
                    None => s,
                }
            }
            Val::Mixed(_) => "mixed".into(),
        }
    }
}

/// `v`, the value of a field called `key`, in the short forms the verbs read
/// back to the same value: a colour as hex, a paint without its default fields.
fn short(key: &str, v: serde_json::Value) -> Val {
    let mut wrapped = serde_json::Value::Object(serde_json::Map::from_iter([(key.to_owned(), v)]));
    mastik_core::verbs::shorten(&mut wrapped);
    match wrapped {
        serde_json::Value::Object(mut map) => map.remove(key).map_or(Val::Nil, Val::Json),
        _ => Val::Nil,
    }
}

/// `items`, each serialized, in the short forms as the list field `key` holds them.
fn short_list(key: &str, items: Vec<serde_json::Result<serde_json::Value>>) -> Val {
    let list: Vec<serde_json::Value> = items.into_iter().filter_map(Result::ok).collect();
    short(key, serde_json::Value::Array(list))
}

/// A length as a script reads it: px as a number, else `"0.05em"`.
fn length(l: Length) -> Val {
    match l {
        Length::Px(n) => Val::Num(n),
        Length::Em(f) => Val::Str(format!("{f}em")),
    }
}

/// A unit variant's name as serde wrote it (`"Center"`, `"Upper"`).
fn variant(v: serde_json::Result<serde_json::Value>) -> Val {
    match v {
        Ok(serde_json::Value::String(s)) => Val::Str(s),
        _ => Val::Nil,
    }
}

/// One character attribute of text `id` over all its characters, as drawn (a
/// bound slot at its variable's value): the value they share, `Mixed` when runs
/// differ, nil for any other layer.
fn char_attr(doc: &Document, id: NodeId, kind: CharAttrKind) -> Val {
    let Some(drawn) = doc.resolved_kind(id) else {
        return Val::Nil;
    };
    let NodeKind::Text {
        content,
        style,
        spans,
        ..
    } = drawn.as_ref()
    else {
        return Val::Nil;
    };
    char_val(style, spans, content.len(), kind)
}

/// One character attribute over a text `len` bytes long with `style` and
/// `spans`: the value its characters share, `Mixed` when runs differ.
fn char_val(
    style: &mastik_core::typography::TextStyle,
    spans: &mastik_core::typography::CharSpans,
    len: usize,
    kind: CharAttrKind,
) -> Val {
    let mut values = spans.values_in(style, kind, 0..len);
    if values.len() != 1 {
        return Val::Mixed("its characters differ".into());
    }
    match values.remove(0) {
        CharAttr::Family(f) => Val::Str(f),
        CharAttr::Size(n) => Val::Num(n),
        CharAttr::Weight(w) => Val::Num(f64::from(w)),
        CharAttr::Italic(b) => Val::Bool(b),
        CharAttr::LineHeight(l) => l.map_or(Val::Nil, length),
        CharAttr::LetterSpacing(l) => length(l),
        CharAttr::Case(c) => variant(serde_json::to_value(c)),
        _ => Val::Nil,
    }
}

/// The two gaps of `d` as one number, or `Mixed` when they differ.
fn gap(d: &Display) -> Val {
    let (row, column) = match d {
        Display::Flex(f) => (f.row_gap, f.column_gap),
        Display::Grid(g) => (g.row_gap, g.column_gap),
    };
    if (row - column).abs() <= build::RADIUS_AGREEMENT {
        Val::Num(row)
    } else {
        Val::Mixed("its row and column gaps differ".into())
    }
}

/// The padding of `d`: one number when its sides agree, else the four.
fn padding(d: &Display) -> Val {
    let p = match d {
        Display::Flex(f) => f.padding,
        Display::Grid(g) => g.padding,
    };
    if p.iter()
        .all(|s| (s - p[0]).abs() <= build::RADIUS_AGREEMENT)
    {
        Val::Num(p[0])
    } else {
        Val::Json(serde_json::json!(p))
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

/// The character properties, each with the attribute it reads.
const CHAR_PROPS: [(&str, CharAttrKind); 7] = [
    ("font_family", CharAttrKind::Family),
    ("font_size", CharAttrKind::Size),
    ("font_weight", CharAttrKind::Weight),
    ("italic", CharAttrKind::Italic),
    ("line_height", CharAttrKind::LineHeight),
    ("letter_spacing", CharAttrKind::LetterSpacing),
    ("text_case", CharAttrKind::Case),
];

/// What the step holding `inverse` set, as `(property, value)` with the value in
/// Luau, read against the document after it: the writes a line would make for
/// the same edit. `None` when `inverse` is no property's edit, or when a value
/// has no one form (paints that differ inside a group, runs that differ), or the
/// edit changed a text attribute no property names. A text style edit that
/// changed only the colour answers empty: the colour a `.fill` write clears.
pub fn set_by(
    doc: &Document,
    res: &Resolved,
    inverse: &Operation,
) -> Option<Vec<(&'static str, String)>> {
    let now = |id: NodeId, prop: &'static str| -> Option<Vec<(&'static str, String)>> {
        Some(vec![(prop, literal(&read(doc, res, id, prop).ok()?)?)])
    };
    match inverse {
        Operation::SetFills { id, .. } => paints(doc, res, *id, "fill", "fills"),
        Operation::SetStrokes { id, .. } => paints(doc, res, *id, "stroke", "strokes"),
        Operation::SetEffects { id, .. } => now(*id, "effects"),
        Operation::SetBlend { id, .. } => now(*id, "blend"),
        Operation::SetLayoutItem { id, .. } => now(*id, "layout_item"),
        Operation::SetDisplay { id, display } => {
            let after = doc.get(*id)?.display();
            match (display, after) {
                (Some(before), Some(after)) => {
                    layout_parts(before, after).or_else(|| now(*id, "layout"))
                }
                (None, None) => None,
                _ => now(*id, "layout"),
            }
        }
        Operation::SetTextStyle { id, style, spans } => {
            let NodeKind::Text {
                content,
                style: now_style,
                spans: now_spans,
                ..
            } = doc.get(*id)?.kind()
            else {
                return None;
            };
            let before = (style, spans.as_ref().unwrap_or(now_spans));
            char_changes(content.len(), before, (now_style, now_spans))
        }
        Operation::SetTextSpans { id, spans } => {
            let NodeKind::Text {
                content,
                style,
                spans: now_spans,
                ..
            } = doc.get(*id)?.kind()
            else {
                return None;
            };
            char_changes(content.len(), (style, spans), (style, now_spans))
        }
        Operation::SetParagraphStyle { id, paragraph, .. } => {
            let NodeKind::Text {
                paragraph: now_p, ..
            } = doc.get(*id)?.kind()
            else {
                return None;
            };
            (paragraph.align != now_p.align).then_some(())?;
            now(*id, "text_align")
        }
        _ => None,
    }
}

/// A paint list as the line writes it: `.fill` when one plain paint reads as
/// that one value, else the whole `.fills`.
fn paints(
    doc: &Document,
    res: &Resolved,
    id: NodeId,
    one: &'static str,
    all: &'static str,
) -> Option<Vec<(&'static str, String)>> {
    match read(doc, res, id, one).ok()? {
        v @ (Val::Str(_) | Val::Json(_)) => Some(vec![(one, literal(&v)?)]),
        _ => Some(vec![(all, literal(&read(doc, res, id, all).ok()?)?)]),
    }
}

/// The character properties whose runs differ between `before` and `after` over
/// a text `len` bytes long, each with its value after. Empty when only the colour
/// changed (what a `.fill` write clears); `None` when a changed one reads as
/// several values, or when an attribute no property names changed too, which a
/// line naming the rest would not repeat.
fn char_changes(
    len: usize,
    before: (
        &mastik_core::typography::TextStyle,
        &mastik_core::typography::CharSpans,
    ),
    after: (
        &mastik_core::typography::TextStyle,
        &mastik_core::typography::CharSpans,
    ),
) -> Option<Vec<(&'static str, String)>> {
    let mut out = Vec::new();
    for kind in CharAttrKind::ALL {
        if kind == CharAttrKind::Color
            || attr_runs(before.0, before.1, len, kind) == attr_runs(after.0, after.1, len, kind)
        {
            continue;
        }
        let (prop, _) = CHAR_PROPS.iter().find(|(_, k)| *k == kind)?;
        out.push((*prop, literal(&char_val(after.0, after.1, len, kind))?));
    }
    Some(out)
}

/// One attribute's runs over a text `len` bytes long, neighbours holding one
/// value merged, so two span layouts that style the same characters alike agree.
fn attr_runs(
    style: &mastik_core::typography::TextStyle,
    spans: &mastik_core::typography::CharSpans,
    len: usize,
    kind: CharAttrKind,
) -> Vec<(std::ops::Range<usize>, CharAttr)> {
    if len == 0 {
        return spans
            .values_in(style, kind, 0..0)
            .into_iter()
            .map(|v| (0..0, v))
            .collect();
    }
    let mut out: Vec<(std::ops::Range<usize>, CharAttr)> = Vec::new();
    for (r, v) in spans.runs_of(style, kind, 0..len) {
        match out.last_mut() {
            Some((last, was)) if *was == v => last.end = r.end,
            _ => out.push((r, v)),
        }
    }
    out
}

/// The gap, padding and direction that differ between two layouts that differ in
/// nothing else, each with its value after (empty when nothing changed); `None`
/// when anything else changed, or one of them reads as several values.
fn layout_parts(before: &Display, after: &Display) -> Option<Vec<(&'static str, String)>> {
    let mut moved = before.clone();
    match (&mut moved, after) {
        (Display::Flex(m), Display::Flex(a)) => {
            m.row_gap = a.row_gap;
            m.column_gap = a.column_gap;
            m.padding = a.padding;
            m.direction = a.direction;
        }
        (Display::Grid(m), Display::Grid(a)) => {
            m.row_gap = a.row_gap;
            m.column_gap = a.column_gap;
            m.padding = a.padding;
        }
        _ => return None,
    }
    if moved != *after {
        return None;
    }
    let direction = |d: &Display| match d {
        Display::Flex(f) => variant(serde_json::to_value(f.direction)),
        Display::Grid(_) => Val::Nil,
    };
    let mut out = Vec::new();
    for (prop, read) in [
        ("gap", gap as fn(&Display) -> Val),
        ("padding", padding),
        ("direction", direction),
    ] {
        let now = read(after);
        if !agree(prop, &read(before), &now) {
            out.push((prop, literal(&now)?));
        }
    }
    Some(out)
}

/// `v` written as Luau that reads back as it; `None` for layers and for a value
/// that is several.
fn literal(v: &Val) -> Option<String> {
    Some(match v {
        Val::Nil => "nil".into(),
        Val::Num(n) => crate::format_number(*n),
        Val::Str(s) => crate::quoted(s),
        Val::Bool(b) => b.to_string(),
        Val::Json(j) => json_literal(j),
        Val::Layers(_) | Val::Mixed(_) => return None,
    })
}

/// A JSON value as a Luau table constructor: an array as a list, an object's
/// keys as fields (bracketed when not a name).
pub(crate) fn json_literal(v: &serde_json::Value) -> String {
    use serde_json::Value;
    match v {
        Value::Null => "nil".into(),
        Value::Bool(b) => b.to_string(),
        Value::Number(n) => n
            .as_f64()
            .map_or_else(|| n.to_string(), crate::format_number),
        Value::String(s) => crate::quoted(s),
        Value::Array(items) if items.is_empty() => "{}".into(),
        Value::Array(items) => {
            let items: Vec<String> = items.iter().map(json_literal).collect();
            format!("{{ {} }}", items.join(", "))
        }
        Value::Object(map) if map.is_empty() => "{}".into(),
        Value::Object(map) => {
            let fields: Vec<String> = map
                .iter()
                .map(|(k, v)| {
                    let name = k.chars().next().is_some_and(crate::lex::ident_start)
                        && k.chars().all(|c| c.is_ascii_alphanumeric() || c == '_')
                        && !crate::lex::KEYWORDS.contains(&k.as_str());
                    match name {
                        true => format!("{k} = {}", json_literal(v)),
                        false => format!("[{}] = {}", crate::quoted(k), json_literal(v)),
                    }
                })
                .collect();
            format!("{{ {} }}", fields.join(", "))
        }
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

/// The one paint of a list, `.fill` or `.stroke`: a shown solid colour as hex,
/// none as `Nil`, any other one paint (a gradient, a picture, a hidden or bound
/// one, a stroke's width) as its table in `list`'s short form, and several as
/// `Mixed`, read whole through `.fills` or `.strokes`.
fn paint_value(list: Val, what: &str) -> Val {
    match list {
        Val::Json(serde_json::Value::Array(mut items)) => match items.len() {
            0 => Val::Nil,
            1 => match items.remove(0) {
                serde_json::Value::String(hex) => Val::Str(hex),
                // A stroke with every default but its colour reads as the colour.
                serde_json::Value::Object(map)
                    if map.len() == 1 && map.get("brush").is_some_and(|b| b.is_string()) =>
                {
                    map.get("brush")
                        .and_then(|b| b.as_str())
                        .map_or(Val::Nil, |hex| Val::Str(hex.to_owned()))
                }
                one => Val::Json(one),
            },
            _ => Val::Mixed(format!("it has several {what}s — .{what}s reads them all")),
        },
        other => other,
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
        // The `f32` drawn (a bound one at its variable's value) read back as the
        // decimal it was written as.
        "opacity" => Val::Num((f64::from(doc.drawn_opacity(id)) * 1e6).round() / 1e6),
        "fill" => paint_value(read(doc, res, id, "fills")?, "fill"),
        "stroke" => paint_value(read(doc, res, id, "strokes")?, "stroke"),
        "fills" => match build::shared_fills(doc, &[id]) {
            PaintShown::Mixed => Val::Mixed("the layers inside it differ".into()),
            PaintShown::List(list) => short_list(
                "fills",
                list.iter()
                    .map(|f| serde_json::to_value(&f.value))
                    .collect(),
            ),
        },
        "strokes" => match build::shared_strokes(doc, &[id]) {
            PaintShown::Mixed => Val::Mixed("the layers inside it differ".into()),
            PaintShown::List(list) => short_list(
                "strokes",
                list.iter()
                    .map(|s| serde_json::to_value(&s.value))
                    .collect(),
            ),
        },
        "effects" => short_list(
            "effects",
            node.effects()
                .iter()
                .map(|e| serde_json::to_value(&e.value))
                .collect(),
        ),
        "font_family" => char_attr(doc, id, CharAttrKind::Family),
        "font_size" => char_attr(doc, id, CharAttrKind::Size),
        "font_weight" => char_attr(doc, id, CharAttrKind::Weight),
        "italic" => char_attr(doc, id, CharAttrKind::Italic),
        "line_height" => char_attr(doc, id, CharAttrKind::LineHeight),
        "letter_spacing" => char_attr(doc, id, CharAttrKind::LetterSpacing),
        "text_case" => char_attr(doc, id, CharAttrKind::Case),
        "text_align" => match node.kind() {
            NodeKind::Text { paragraph, .. } => variant(serde_json::to_value(paragraph.align)),
            _ => Val::Nil,
        },
        // The layout as laid out: a bound gap or padding at its variable's value.
        "layout" => match doc.resolved_display(id) {
            None => Val::Nil,
            Some(d) => serde_json::to_value(&*d).map_or(Val::Nil, |v| short("layout", v)),
        },
        "direction" => match doc.resolved_display(id).as_deref() {
            Some(Display::Flex(f)) => variant(serde_json::to_value(f.direction)),
            _ => Val::Nil,
        },
        "gap" => doc.resolved_display(id).map_or(Val::Nil, |d| gap(&d)),
        "padding" => doc.resolved_display(id).map_or(Val::Nil, |d| padding(&d)),
        "layout_item" => serde_json::to_value(doc.resolved_item(id))
            .map_or(Val::Nil, |v| short("layout_item", v)),
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
