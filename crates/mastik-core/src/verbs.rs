//! The verb registry: every document edit a caller outside the canvas can name — an
//! MCP tool, a Command Mode line, a headless script — with typed parameters, in one
//! list. A verb reads the document and answers a [`Transaction`] and the ids it minted;
//! the caller commits it, as every edit is committed.
//!
//! Names are the public ones MCP and Command Mode use, so they are spelled for an
//! agent (`set_fills`, `boolean`), not after the builder behind them. Arguments
//! arrive as a JSON object; nodes as their wire form (`"<actor-hex>:<seq>"`), model
//! values (a fill list, a transform) in their serde form.

use crate::build::{self, Axis, Edge, ZMove};
use crate::container::{Display, Insets, LayoutItem};
use crate::document::Document;
use crate::effect::Effect;
use crate::export::ExportSpec;
use crate::id::{IdSource, NodeId};
use crate::item::{Keyed, PastTheEnd, rekey_by_position};
use crate::node::{
    BlendMode, BlockStyle, BoolOp, CharSpans, Fill, NodeKind, ParagraphStyle, Pivot, Stroke,
    TextSizing, TextStyle,
};
use crate::op::{GeometryPatch, OpError, Operation, Transaction};
use crate::resolve::Resolved;
use crate::style::{StyleId, StyleSlot, StyleValue};
use crate::svg_in::ImageSink;
use crate::typography::{CharAttr, Length, ParaAttr};
use crate::variable::{Cell, CollectionId, ModeId, VarId, VarKind};
use kurbo::Affine;
use rustc_hash::FxHashSet;
use serde_json::{Map, Value};

mod loose;
mod tables;

pub use tables::{find_table, moved_onto, readers, readers_words};

/// What a parameter takes, for checking an argument before a verb runs and for
/// describing the verb to a caller (an MCP schema, Command Mode's completion).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ParamKind {
    /// One node, by its wire form.
    Node,
    /// One or more nodes, as a list of wire forms.
    Nodes,
    Number,
    /// A whole number, zero or more.
    Index,
    Bool,
    Text,
    /// One of these words.
    Choice(&'static [&'static str]),
    /// A model value in its serde form; the name says which type.
    Model(&'static str),
    /// The id of a thing that is not a layer, in the node wire form; the name says
    /// which: a variable, a mode, a collection, a style.
    Id(&'static str),
}

/// One parameter of a [`Verb`].
#[derive(Clone, Copy, Debug)]
pub struct Param {
    pub name: &'static str,
    pub kind: ParamKind,
    pub required: bool,
    pub doc: &'static str,
}

/// A required parameter.
const fn req(name: &'static str, kind: ParamKind, doc: &'static str) -> Param {
    Param {
        name,
        kind,
        required: true,
        doc,
    }
}

/// An optional parameter.
const fn opt(name: &'static str, kind: ParamKind, doc: &'static str) -> Param {
    Param {
        name,
        kind,
        required: false,
        doc,
    }
}

/// What a verb reads and mints with.
pub struct Ctx<'a> {
    pub doc: &'a Document,
    pub res: &'a Resolved,
    pub ids: &'a mut IdSource,
    /// What hashes and sizes a picture's bytes, which core cannot do; `None`
    /// refuses `add_image`.
    pub images: Option<&'a mut dyn ImageSink>,
    /// What measures contrast against a rendered backdrop, which core cannot do;
    /// `None` refuses `fix_contrast`.
    pub contrast: Option<&'a mut dyn crate::contrast::Probe>,
}

/// What a caller lends a script's verbs that core cannot do itself.
#[derive(Default)]
pub struct Lent<'a> {
    pub images: Option<&'a mut dyn ImageSink>,
    pub contrast: Option<&'a mut dyn crate::contrast::Probe>,
}

/// A verb's answer: the edit to commit, the nodes it creates, in order, the id
/// of what else it made (a picture, a variable, a mode, a style) as a caller
/// names it, and a line on what it did.
#[derive(Debug, Default)]
pub struct Done {
    pub tx: Transaction,
    pub created: Vec<NodeId>,
    pub made: Option<String>,
    /// A line on what the edit did that its caller cannot read off the document:
    /// how many matches a replace wrote, what an import left out.
    pub said: Option<String>,
}

impl Done {
    fn edit(tx: Transaction) -> Self {
        Self {
            tx,
            ..Self::default()
        }
    }

    fn ops(ops: Vec<Operation>) -> Self {
        Self::edit(Transaction(ops))
    }

    fn made(tx: Transaction, created: Vec<NodeId>) -> Self {
        Self {
            tx,
            created,
            ..Self::default()
        }
    }

    /// `self`, with `said` saying what it did.
    fn saying(self, said: String) -> Self {
        Self {
            said: Some(said),
            ..self
        }
    }

    /// `ops`, which make the thing whose id is `made`.
    fn making(ops: Vec<Operation>, made: String) -> Self {
        Self {
            made: Some(made),
            ..Self::ops(ops)
        }
    }
}

/// Why a verb did not answer an edit.
#[derive(thiserror::Error, Debug)]
pub enum VerbError {
    #[error("no verb is called `{0}`")]
    Unknown(String),
    #[error("`{verb}` needs `{param}`")]
    Missing {
        verb: &'static str,
        param: &'static str,
    },
    #[error("`{verb}` takes no `{param}`; it takes {takes}")]
    Unexpected {
        verb: &'static str,
        param: String,
        /// The parameters it does take, written for the refusal.
        takes: String,
    },
    #[error("`{param}` of `{verb}`: {why}")]
    Invalid {
        verb: &'static str,
        param: &'static str,
        why: String,
    },
    #[error("the arguments of `{0}` must be an object")]
    NotAnObject(&'static str),
    #[error(transparent)]
    Refused(#[from] OpError),
    #[error("{0}")]
    Nothing(&'static str),
}

/// One verb: its public name, a line on what it does, its parameters, and the
/// function that answers it.
pub struct Verb {
    pub name: &'static str,
    pub doc: &'static str,
    pub params: &'static [Param],
    run: fn(&mut Ctx<'_>, &Args<'_>) -> Result<Done, VerbError>,
}

impl std::fmt::Debug for Verb {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Verb").field("name", &self.name).finish()
    }
}

/// A verb's arguments, checked against its parameters.
pub struct Args<'a> {
    verb: &'static Verb,
    map: &'a Map<String, Value>,
}

/// Parameter `p`'s value `v` with its short forms expanded, as it is read; a tree's
/// `like`s left out, since only the verb can see the layers they name. A design
/// tokens file is read as written: its `$value`s are its own grammar.
fn loosened<'v>(p: &Param, v: &'v Value) -> std::borrow::Cow<'v, Value> {
    match p.kind {
        ParamKind::Model("DtcgFile") => std::borrow::Cow::Borrowed(v),
        ParamKind::Model(model) => {
            let mut v = v.clone();
            if model == "Tree" {
                fn unlike(v: &mut Value) {
                    if let Value::Object(map) = v {
                        map.remove("like");
                        if let Some(Value::Array(children)) = map.get_mut("children") {
                            children.iter_mut().for_each(unlike);
                        }
                    }
                }
                unlike(&mut v);
            }
            // Some clients send a parameter with no schema type as JSON text.
            if let Value::String(s) = &v
                && model != "PropValue"
                && let Ok(parsed) = serde_json::from_str::<Value>(s.trim())
                && (parsed.is_object() || parsed.is_array() || parsed.is_string())
            {
                v = parsed;
            }
            if as_written(model) {
                return std::borrow::Cow::Owned(v);
            }
            // A short form written wrong is left as written, for the check to
            // refuse with its reason.
            let written = v.clone();
            if loosen_model(model, model_key(model, p.name), &mut v).is_err() {
                return std::borrow::Cow::Owned(written);
            }
            // What `settle` puts right reaches the verb, not only the check.
            if let Ok(settled) = settled_model(model, &v) {
                v = settled;
            }
            std::borrow::Cow::Owned(v)
        }
        _ => std::borrow::Cow::Borrowed(v),
    }
}

/// `value` checked against the parameters of the tool called `name`: an object
/// holding every required one, no other, each of its kind. MCP's own tools, which
/// are not verbs, are checked by it too.
pub fn check_args<'a>(
    name: &'static str,
    params: &[Param],
    value: &'a Value,
) -> Result<&'a Map<String, Value>, VerbError> {
    let map = match value {
        Value::Object(map) => map,
        Value::Null if params.iter().all(|p| !p.required) => {
            static EMPTY: std::sync::OnceLock<Map<String, Value>> = std::sync::OnceLock::new();
            EMPTY.get_or_init(Map::new)
        }
        _ => return Err(VerbError::NotAnObject(name)),
    };
    if let Some(extra) = map.keys().find(|k| !params.iter().any(|p| p.name == *k)) {
        let takes: Vec<String> = params.iter().map(|p| format!("`{}`", p.name)).collect();
        return Err(VerbError::Unexpected {
            verb: name,
            param: extra.clone(),
            takes: match takes.as_slice() {
                [] => "nothing".into(),
                _ => takes.join(", "),
            },
        });
    }
    for p in params {
        match map.get(p.name) {
            None | Some(Value::Null) if p.required => {
                return Err(VerbError::Missing {
                    verb: name,
                    param: p.name,
                });
            }
            None | Some(Value::Null) => {}
            Some(v) => check_kind(p.kind, &loosened(p, v)).map_err(|why| VerbError::Invalid {
                verb: name,
                param: p.name,
                why,
            })?,
        }
    }
    Ok(map)
}

impl<'a> Args<'a> {
    /// `value` checked against `verb`'s parameters: an object holding every required
    /// one, no other, each of its kind.
    fn check(verb: &'static Verb, value: &'a Value) -> Result<Self, VerbError> {
        let map = check_args(verb.name, verb.params, value)?;
        Ok(Self { verb, map })
    }

    fn invalid(&self, param: &'static str, why: impl ToString) -> VerbError {
        VerbError::Invalid {
            verb: self.verb.name,
            param,
            why: why.to_string(),
        }
    }

    fn get(&self, name: &'static str) -> Option<&Value> {
        self.map.get(name).filter(|v| !v.is_null())
    }

    /// Whether `name` was given as `null`, which some parameters read as
    /// "clear it".
    fn nulled(&self, name: &'static str) -> bool {
        self.map.get(name).is_some_and(Value::is_null)
    }

    /// The `Node` parameter `name`.
    pub fn node(&self, name: &'static str) -> Result<NodeId, VerbError> {
        let v = self.get(name).ok_or(VerbError::Missing {
            verb: self.verb.name,
            param: name,
        })?;
        wire(v).map_err(|why| self.invalid(name, why))
    }

    /// The `Node` parameter `name`, refused when it names a page: a page has no
    /// placement, opacity, eye, lock, blend or export a user can see or clear.
    pub fn layer(&self, doc: &Document, name: &'static str) -> Result<NodeId, VerbError> {
        let id = self.node(name)?;
        match doc.is_page(id) {
            true => Err(self.invalid(name, "a page is not a layer")),
            false => Ok(id),
        }
    }

    /// The `Nodes` parameter `name`, refused when any names a page, as [`Self::layer`].
    pub fn layers(&self, doc: &Document, name: &'static str) -> Result<Vec<NodeId>, VerbError> {
        let ids = self.nodes(name)?;
        match ids.iter().any(|id| doc.is_page(*id)) {
            true => Err(self.invalid(name, "a page is not a layer")),
            false => Ok(ids),
        }
    }

    /// The `Nodes` parameter `name`.
    pub fn nodes(&self, name: &'static str) -> Result<Vec<NodeId>, VerbError> {
        let Some(Value::Array(list)) = self.get(name) else {
            return Err(self.invalid(name, "expected a list of node ids"));
        };
        list.iter()
            .map(|v| wire(v).map_err(|why| self.invalid(name, why)))
            .collect()
    }

    /// The `Number` parameter `name`, or `None` when an optional one is absent.
    pub fn number(&self, name: &'static str) -> Option<f64> {
        self.get(name).and_then(Value::as_f64)
    }

    /// The `Index` parameter `name`, or `None` when an optional one is absent.
    pub fn index(&self, name: &'static str) -> Option<usize> {
        self.get(name)
            .and_then(Value::as_u64)
            .and_then(|n| usize::try_from(n).ok())
    }

    /// The `Bool` parameter `name`, or `None` when an optional one is absent.
    pub fn bool(&self, name: &'static str) -> Option<bool> {
        self.get(name).and_then(Value::as_bool)
    }

    /// The `Text` parameter `name`, or `None` when an optional one is absent.
    pub fn text(&self, name: &'static str) -> Option<&str> {
        self.get(name).and_then(Value::as_str)
    }

    /// The `Choice` parameter `name`, or `None` when an optional one is absent.
    pub fn choice(&self, name: &'static str) -> Option<&str> {
        self.text(name)
    }

    /// The `Model` parameter `name`, deserialized from its short forms (hex
    /// colours, paints without their default fields) as well as its own shape.
    pub fn model<T: serde::de::DeserializeOwned>(
        &self,
        name: &'static str,
    ) -> Result<T, VerbError> {
        let v = self.get(name).ok_or(VerbError::Missing {
            verb: self.verb.name,
            param: name,
        })?;
        let v = match self.verb.params.iter().find(|p| p.name == name) {
            Some(p) => loosened(p, v).into_owned(),
            None => v.clone(),
        };
        serde_json::from_value(v).map_err(|e| self.invalid(name, e))
    }

    /// Parameter `name` with its short forms expanded and its words put right, as
    /// JSON: for a patch the verb merges itself.
    fn loose(&self, name: &'static str) -> Option<Value> {
        let v = self.get(name)?;
        Some(match self.verb.params.iter().find(|p| p.name == name) {
            Some(p) => loosened(p, v).into_owned(),
            None => v.clone(),
        })
    }

    /// The `Id` parameter `name`, as the id type it names.
    pub fn id<T: serde::de::DeserializeOwned>(&self, name: &'static str) -> Result<T, VerbError> {
        self.model(name)
    }

    /// The `Id` parameter `name` when given.
    pub fn id_opt<T: serde::de::DeserializeOwned>(
        &self,
        name: &'static str,
    ) -> Result<Option<T>, VerbError> {
        self.model_opt(name)
    }

    /// The `Model` parameter `name` when given.
    pub fn model_opt<T: serde::de::DeserializeOwned>(
        &self,
        name: &'static str,
    ) -> Result<Option<T>, VerbError> {
        match self.get(name) {
            None => Ok(None),
            Some(_) => self.model(name).map(Some),
        }
    }
}

/// A node id from its wire form.
fn wire(v: &Value) -> Result<NodeId, String> {
    let s = v.as_str().ok_or("expected a node id")?;
    NodeId::from_wire(s).ok_or_else(|| format!("`{s}` is not a node id"))
}

/// Whether `v` is of `kind`, or why not.
fn check_kind(kind: ParamKind, v: &Value) -> Result<(), String> {
    match kind {
        ParamKind::Node => wire(v).map(|_| ()),
        ParamKind::Nodes => match v {
            Value::Array(list) if !list.is_empty() => {
                list.iter().try_for_each(|v| wire(v).map(|_| ()))
            }
            _ => Err("expected a non-empty list of node ids".into()),
        },
        ParamKind::Number => match v.as_f64() {
            Some(n) if n.is_finite() => Ok(()),
            _ => Err("expected a number".into()),
        },
        ParamKind::Index => v
            .as_u64()
            .map(|_| ())
            .ok_or_else(|| "expected a whole number".into()),
        ParamKind::Bool => v
            .as_bool()
            .map(|_| ())
            .ok_or_else(|| "expected true or false".into()),
        ParamKind::Text => v
            .as_str()
            .map(|_| ())
            .ok_or_else(|| "expected a string".into()),
        ParamKind::Choice(words) => match v.as_str() {
            Some(s) if words.contains(&s) => Ok(()),
            _ => Err(format!("expected one of {}", words.join(", "))),
        },
        ParamKind::Model(model) => check_model(model, v),
        ParamKind::Id(what) => wire(v)
            .map(|_| ())
            .map_err(|_| format!("expected a {what}'s id")),
    }
}

/// Whether `v`, in its own shape or the short forms, reads as the model type
/// `model` names (a [`ParamKind::Model`]'s name), or why not.
pub fn check_model(model: &str, v: &Value) -> Result<(), String> {
    settled_model(model, v).map(|_| ())
}

/// [`loose::loosen_strict`] for a value of the model type `model`, as the field
/// `key`. A chart's own `text` and `padding` are not a text style or a frame's
/// padding, the layer short forms of those names: only what is inside them loosens.
/// A chart style's value loosens as chart options, and every chart value takes
/// its colours as `"#rrggbb"`.
fn loosen_model(model: &str, key: &str, v: &mut Value) -> Result<(), String> {
    match model {
        "ChartOptions" => *v = chart_hex(&dark_as_theme(v)),
        "ChartHighlight" | "[ChartReference]" => *v = chart_hex(v),
        "StyleValue" => {
            if let Some(map) = v.as_object_mut()
                && let Some(mut chart) = map.remove("chart")
            {
                loosen_model("ChartOptions", "options", &mut chart)?;
                map.insert("chart".into(), chart);
                return Ok(());
            }
        }
        _ => {}
    }
    let own: Vec<(&str, Value)> = match (model, v.as_object_mut()) {
        ("ChartOptions", Some(map)) => ["text", "padding"]
            .into_iter()
            .filter_map(|k| map.remove(k).map(|v| (k, v)))
            .collect(),
        _ => Vec::new(),
    };
    loose::loosen_strict(key, v)?;
    for (k, mut field) in own {
        if let Value::Object(inner) = &mut field {
            for (name, value) in inner.iter_mut() {
                loose::loosen_strict(name, value)?;
            }
        }
        if let Some(map) = v.as_object_mut() {
            map.insert(k.to_owned(), field);
        }
    }
    Ok(())
}

/// `v` in the model's own shape: its short forms expanded and the slips
/// [`settle`] puts right put right, or why it does not read as the model.
fn settled_model(model: &str, v: &Value) -> Result<Value, String> {
    let mut loose = v.clone();
    if !as_written(model) {
        loosen_model(model, model_key(model, ""), &mut loose)?;
    }
    let v = &loose;
    macro_rules! reads {
        ($t:ty) => {
            settle::<$t>(model, v.clone())
        };
    }
    let checked = |r: Result<(), String>| r.map(|()| v.clone());
    match model {
        "Affine" => reads!(Affine),
        "BlockStyle" => reads!(BlockStyle),
        "CharSpans" => reads!(CharSpans),
        "Display" => reads!(Display),
        "GeometryPatch" => reads!(GeometryPatch),
        "Insets" => reads!(Insets),
        "LayoutItem" => reads!(LayoutItem),
        "NodeKind" => reads!(NodeKind),
        "ParagraphStyle" => reads!(ParagraphStyle),
        "Pivot" => reads!(Pivot),
        "TextStyle" => reads!(TextStyle),
        "[CharAttr]" => reads!(Vec<CharAttr>),
        "[ParaAttr]" => reads!(Vec<ParaAttr>),
        "Cell" => reads!(Cell),
        "StyleValue" => reads!(StyleInput),
        "BoundField" => reads!(build::BoundField),
        "Color" => reads!(peniko::Color),
        "[Hex]" => reads!(Vec<String>),
        "Scope" => reads!(Vec<String>),
        "Tree" => reads!(Tree),
        "[Effect]" => reads!(Vec<Effect>),
        "[ExportSpec]" => reads!(Vec<ExportSpec>),
        "[Fill]" => reads!(Vec<Fill>),
        "[Stroke]" => reads!(Vec<Stroke>),
        "[LayoutGrid]" => reads!(Vec<crate::layout::LayoutGrid>),
        "[Guide]" => reads!(Vec<GuideIn>),
        "Quad" => reads!([[f64; 2]; 4]),
        "RepeatLine" => reads!(crate::repeat::Line),
        "RepeatGrid" => reads!(crate::repeat::Grid),
        "ChartTable" => reads!(crate::chart::Table),
        "[Name]" => reads!(Vec<String>),
        // Checked with stand-in ids for snap points that leave them out; the verb
        // fills in its own.
        "LayerPlay" => {
            let mut p = v.clone();
            let mut n = 0;
            play_in(&mut p, &mut || {
                n += 1;
                crate::item::ItemId::positional(n)
            });
            settle::<crate::prototype::LayerPlay>(model, p).map(|_| v.clone())
        }
        "PlayExport" => reads!(crate::prototype::PlayExport),
        // A transition in part completed from a new page's, which is what the
        // verb reads too.
        "PagePlay" => {
            let mut p = v.clone();
            if let Some(t) = p.get_mut("transition") {
                let base = serde_json::to_value(crate::prototype::Transition::default())
                    .unwrap_or_default();
                *t = transition_in(Some(t.take()), &base);
            }
            settle::<crate::prototype::PagePlay>(model, p)
        }
        // Checked with stand-in ids and a new page's transition where the list
        // leaves them out; the verb fills in its own (`keyed_arg`).
        "[Reaction]" | "[Flow]" => {
            let mut p = v.clone();
            let mut n = 0;
            let mut mint = || {
                n += 1;
                crate::item::ItemId::positional(n)
            };
            let read = match model {
                "[Flow]" => {
                    ids_in(&mut p, &mut mint);
                    settle::<Vec<Keyed<crate::prototype::Flow>>>(model, p)
                }
                _ => {
                    let base = serde_json::to_value(crate::prototype::Transition::default())
                        .unwrap_or_default();
                    reactions_in(&mut p, &base, &mut mint)
                        .and_then(|()| settle::<Vec<Keyed<crate::prototype::Reaction>>>(model, p))
                }
            };
            read.map(|_| v.clone())
        }
        // Patches, whose nulls reset what they name (`patched`): read without
        // them, kept with them.
        "ChartMapping" => {
            checked(read_model::<crate::chart::Mapping>(&without_nulls(v)).map(|_| ()))
        }
        "ChartOptions" => {
            settle::<crate::chart::Options>(model, without_nulls(v)).map(|mut settled| {
                nulls_back(&mut settled, v);
                settled
            })
        }
        // Whole values, each replacing the chart's own.
        "ChartHighlight" => reads!(crate::chart::Highlight),
        "[ChartReference]" => reads!(Vec<crate::chart::Reference>),
        "TableRows" => checked(read_model::<Vec<Vec<tables::CellIn>>>(v).map(|_| ())),
        "[ColumnName]" => checked(read_model::<Vec<String>>(v).map(|_| ())),
        "[TableCell]" => checked(read_model::<Vec<tables::CellEdit>>(v).map(|_| ())),
        "[TableRow]" => checked(
            read_model::<Vec<std::collections::BTreeMap<String, tables::CellIn>>>(v).map(|_| ()),
        ),
        "[RowNumber]" => checked(read_model::<Vec<usize>>(v).map(|_| ())),
        "TableColumns" => checked(read_model::<tables::ColumnEdits>(v).map(|_| ())),
        "ListRows" => checked(read_model::<crate::list::Rows>(v).map(|_| ())),
        "ListFill" => {
            checked(read_model::<std::collections::BTreeMap<String, String>>(v).map(|_| ()))
        }
        "ListFilter" => checked(read_model::<tables::FilterIn>(v).map(|_| ())),
        "ListSort" => checked(read_model::<Vec<tables::SortIn>>(v).map(|_| ())),
        other => checked(check_other(other, v)),
    }
}

/// Whether a model is read as written, no short form loosened: a design tokens
/// file's `$value`s are its own grammar, and a table's cells and column names are
/// the person's words, which a short form keyed by a field's name would rewrite.
fn as_written(model: &str) -> bool {
    matches!(
        model,
        "DtcgFile"
            | "TableRows"
            | "[ColumnName]"
            | "[TableCell]"
            | "[TableRow]"
            | "[RowNumber]"
            | "TableColumns"
            | "ListRows"
            | "ListFill"
            | "ListFilter"
            | "ListSort"
    )
}

/// Whether `v` is the model `model` names that no serde type reads, or why not.
fn check_other(model: &str, v: &Value) -> Result<(), String> {
    match model {
        // A list of node ids that may be empty, where `Nodes` may not.
        "[Node]" => match v {
            Value::Array(list) => list.iter().try_for_each(|v| wire(v).map(|_| ())),
            _ => Err("expected a list of node ids".into()),
        },
        "PropValue" => match v {
            Value::Bool(_) | Value::String(_) => Ok(()),
            _ => Err("expected true, false or a string".into()),
        },
        // MCP's own tools: their handlers read the rest.
        "ChartColours"
            if v.as_object().is_some_and(|m| {
                m.values()
                    .all(|c| c.is_string() || c.is_null() || c.is_object())
            }) =>
        {
            Ok(())
        }
        "ChartSeries"
            if v.as_object()
                .is_some_and(|m| m.values().all(Value::is_object)) =>
        {
            Ok(())
        }
        "ChartSeries" => Err("expected an object of series styles by series name".into()),
        "ChartColours" => Err(
            "expected an object of colours by series or mark name, null for the palette's".into(),
        ),
        "KitModeMap" if v.is_object() => Ok(()),
        "KitModeMap" => Err("expected an object of mode ids".into()),
        "DtcgFile" if v.is_object() || v.is_string() => Ok(()),
        "DtcgFile" => Err("expected the tokens file, as text or as an object".into()),
        "Steps" if v.as_array().is_some_and(|s| s.iter().all(Value::is_object)) => Ok(()),
        "Steps" => Err("expected a list of steps, each an object".into()),
        "Version" if v.is_u64() || v.is_string() => Ok(()),
        "Version" => Err("expected a version's `at` or its name".into()),
        "Region"
            if v.as_array()
                .is_some_and(|r| r.len() == 4 && r.iter().all(Value::is_number)) =>
        {
            Ok(())
        }
        "Region" => Err("expected [x, y, w, h]".into()),
        "RowRange"
            if v.as_object().is_some_and(|m| {
                ["from", "count"]
                    .iter()
                    .all(|k| m.get(*k).and_then(Value::as_u64).is_some_and(|n| n >= 1))
            }) =>
        {
            Ok(())
        }
        "RowRange" => Err("expected {from, count}, each a whole number from 1".into()),
        "Props" | "Args" if v.is_object() => Ok(()),
        "Props" | "Args" => Err("expected an object".into()),
        _ => Err(format!("no model type is called `{model}`")),
    }
}

/// `v` read as a `T`, refused naming where it went wrong: the path of the value
/// serde refused, or of a field `T` has no place for, which serde alone would
/// drop without a word (`Flex { gap }` laid out with no gap).
pub(crate) fn read_model<T: serde::de::DeserializeOwned>(v: &Value) -> Result<T, String> {
    attempt(v).map_err(|miss| miss.said(v))
}

/// Where and why a value did not read as its model type.
struct Miss {
    steps: Vec<Option<Step>>,
    /// A field the type has no place for, rather than a value serde refused.
    unknown_field: bool,
    why: String,
}

impl Miss {
    /// The refusal as [`read_model`] words it, its path written against `v`.
    fn said(&self, v: &Value) -> String {
        let path = json_path(v, self.steps.clone());
        match (path.is_empty(), self.unknown_field) {
            (_, true) => format!("at `{path}`: no such field"),
            (true, false) => self.why.clone(),
            (false, false) => format!("at `{path}`: {}", self.why),
        }
    }
}

/// `v` read as a `T`, or where and why not.
fn attempt<T: serde::de::DeserializeOwned>(v: &Value) -> Result<T, Miss> {
    let mut unknown = Vec::new();
    let mut note = |path: serde_ignored::Path<'_>| unknown.push(ignored_steps(&path));
    let de = serde_ignored::Deserializer::new(v, &mut note);
    let t = serde_path_to_error::deserialize(de).map_err(|e| {
        let steps = e.path().iter().map(|s| {
            use serde_path_to_error::Segment;
            match s {
                Segment::Seq { index } => Some(Step::Index(*index)),
                Segment::Map { key } => Some(Step::Key(key.clone())),
                Segment::Enum { variant } => Some(Step::Key(variant.clone())),
                Segment::Unknown => None,
            }
        });
        let miss = Miss {
            steps: steps.collect(),
            unknown_field: false,
            why: e.into_inner().to_string(),
        };
        brush_miss(v, &miss).unwrap_or(miss)
    })?;
    match unknown.into_iter().next().or_else(|| unseen_field(v)) {
        None => Ok(t),
        Some(steps) => Err(Miss {
            steps,
            unknown_field: true,
            why: String::new(),
        }),
    }
}

/// A paint's `brush` refused as the untagged `BrushSource` refuses it, which names
/// neither the field nor the variant, read again as a brush alone for serde's own
/// path and reason: `None` where the miss is not that or the brush reads.
fn brush_miss(v: &Value, miss: &Miss) -> Option<Miss> {
    if !matches!(miss.steps.last(), Some(Some(Step::Key(k))) if k == "brush")
        || !miss.why.contains("untagged enum")
    {
        return None;
    }
    let mut at = v;
    for step in &miss.steps {
        at = match (step, at) {
            (Some(Step::Key(k)), Value::Object(map)) => map.get(k)?,
            (Some(Step::Index(i)), Value::Array(list)) => list.get(*i)?,
            _ => return None,
        };
    }
    if at.get("var").is_some() {
        return None;
    }
    let inner = attempt::<crate::Brush>(at).err()?;
    Some(Miss {
        steps: [miss.steps.clone(), inner.steps].concat(),
        ..inner
    })
}

/// A noise effect's kind's fields, its tag first: `mastik_core::Noise`'s, which
/// serde reads through a wire type that hides them.
const NOISE_KIND_FIELDS: &[&str] = &[
    "type", "size", "density", "seed", "blend", "colors", "multi", "opacity",
];

/// A texture effect's kind's fields, its tag first, for [`NOISE_KIND_FIELDS`]'
/// reason.
const TEXTURE_KIND_FIELDS: &[&str] = &["type", "size", "amount", "seed", "detail"];

/// A glass effect's kind's fields, its tag first, for [`NOISE_KIND_FIELDS`]'
/// reason.
const GLASS_KIND_FIELDS: &[&str] = &[
    "type",
    "blur",
    "frost",
    "bend",
    "depth",
    "dispersion",
    "rim",
];

/// The path of a field `serde_ignored` cannot see: one inside a gradient brush,
/// whose `#[serde(flatten)]` drops what it does not read; inside an untagged
/// grid track (`minmax` or `repeat`); inside an effect's internally tagged
/// kind, its shadow's colour included; or inside a solid brush's colour, which
/// the untagged `BrushSource` buffers. `None` when there is none.
fn unseen_field(v: &Value) -> Option<Vec<Option<Step>>> {
    /// A colour's fields as a `DynamicColor` writes them, which a hex is
    /// loosened into: a plain colour's components among them.
    fn colour_fields() -> Option<Vec<&'static str>> {
        struct_fields::<peniko::color::DynamicColor>().map(<[_]>::to_vec)
    }
    /// An effect kind's fields by its `type`, the tag among them; `None` for a
    /// map that is no effect kind.
    fn effect_fields(map: &serde_json::Map<String, Value>) -> Option<Vec<&'static str>> {
        let fields = match map.get("type")?.as_str()? {
            "drop_shadow" | "inner_shadow" => struct_fields::<crate::effect::Shadow>()?.to_vec(),
            "layer_blur" | "background_blur" => vec!["radius", "ramp"],
            "sharpen" => vec!["amount", "radius"],
            "noise" => NOISE_KIND_FIELDS[1..].to_vec(),
            "texture" => TEXTURE_KIND_FIELDS[1..].to_vec(),
            "glass" => GLASS_KIND_FIELDS[1..].to_vec(),
            "filters" => struct_fields::<crate::effect::Filters>()?.to_vec(),
            _ => return None,
        };
        Some([vec!["type"], fields].concat())
    }
    /// The key a shadow's colour is walked under, one no agent writes.
    const COLOUR: &str = "\0colour";
    fn walk(v: &Value, key: &str, at: &mut Vec<Option<Step>>) -> Option<Vec<Option<Step>>> {
        match v {
            Value::Object(map) => {
                let effect = if key == "kind" {
                    effect_fields(map)
                } else {
                    None
                };
                let allowed: Option<Vec<&str>> = match key {
                    _ if effect.is_some() => effect.clone(),
                    "Solid" | COLOUR => colour_fields(),
                    "Gradient" => struct_fields::<peniko::Gradient>().map(|f| {
                        let mut f = f.to_vec();
                        f.extend(["transform", "opacity"]);
                        f
                    }),
                    "track" if map.contains_key("repeat") => Some(vec!["repeat", "tracks"]),
                    "track" if map.contains_key("min") || map.contains_key("max") => {
                        Some(vec!["min", "max"])
                    }
                    _ => None,
                };
                if let Some(allowed) = allowed
                    && let Some(extra) = map.keys().find(|k| !allowed.contains(&k.as_str()))
                {
                    let mut path = at.clone();
                    path.push(Some(Step::Key(extra.clone())));
                    return Some(path);
                }
                for (k, child) in map {
                    at.push(Some(Step::Key(k.clone())));
                    let child_key = match k.as_str() {
                        "color" if effect.is_some() => COLOUR,
                        k => k,
                    };
                    let found = walk(child, child_key, at);
                    at.pop();
                    if found.is_some() {
                        return found;
                    }
                }
                None
            }
            Value::Array(items) => {
                let item_key = match key {
                    "columns" | "rows" | "tracks" => "track",
                    // A noise's two colours, walked as a shadow's one.
                    "colors" => COLOUR,
                    _ => "",
                };
                for (i, item) in items.iter().enumerate() {
                    at.push(Some(Step::Index(i)));
                    let found = walk(item, item_key, at);
                    at.pop();
                    if found.is_some() {
                        return found;
                    }
                }
                None
            }
            _ => None,
        }
    }
    walk(v, "", &mut Vec::new())
}

/// `v` as a `T` reads it once the slips an agent makes are put right: an enum
/// word in another case or in kebab-case (`center`, `space-between`), a field in
/// camelCase or kebab-case (`fontSize`). Refused with the form to write when a
/// slip is not one of those.
fn settle<T: serde::de::DeserializeOwned>(model: &str, mut v: Value) -> Result<Value, String> {
    // Each repair fixes one place, so a value has at most a handful.
    for _ in 0..64 {
        match attempt::<T>(&v) {
            Ok(_) => return Ok(v),
            Err(miss) => {
                if !repair(&mut v, &miss) {
                    return Err(explain(model, &v, &miss));
                }
            }
        }
    }
    read_model::<T>(&v).map(|_| v)
}

/// A word reduced to what tells variants and fields apart: no case, no `-`, `_`
/// or spaces, no quotes a client wrapped it in.
fn plain(word: &str) -> String {
    word.chars()
        .filter(|c| !matches!(c, '-' | '_' | ' ' | '"'))
        .flat_map(char::to_lowercase)
        .collect()
}

/// The words of the model's own variants an agent writes by another name.
fn alias(plain: &str) -> &str {
    match plain {
        "frame" => "artboard",
        // Hug contents, as Figma and CSS's `fit-content` name a size that follows them.
        "hug" | "hugcontents" | "fit" => "fitcontent",
        other => other,
    }
}

/// `v`'s value at `steps`, an unnamed step taken as an object's one key.
fn walk_mut<'v>(mut at: &'v mut Value, steps: &[Option<Step>]) -> Option<&'v mut Value> {
    for step in steps {
        at = match (step, at) {
            (Some(Step::Key(k)), Value::Object(map)) => map.get_mut(k)?,
            (Some(Step::Index(i)), Value::Array(list)) => list.get_mut(*i)?,
            (None, Value::Object(map)) if map.len() == 1 => map.values_mut().next()?,
            _ => return None,
        };
    }
    Some(at)
}

/// Put right the slip `miss` names in `v`; whether anything changed.
fn repair(v: &mut Value, miss: &Miss) -> bool {
    if miss.unknown_field {
        let Some((Some(Step::Key(key)), above)) = miss.steps.split_last() else {
            return false;
        };
        let snake = loose::snake(&key.replace(['-', ' '], "_")).replace("__", "_");
        let Some(Value::Object(map)) = walk_mut(v, above) else {
            return false;
        };
        // The written field wins over one the short forms filled in by default.
        if snake == *key {
            return false;
        }
        let Some(value) = map.remove(key) else {
            return false;
        };
        map.insert(snake, value);
        return true;
    }
    let Some(rest) = miss.why.strip_prefix("unknown variant `") else {
        return false;
    };
    // serde writes the refused word and then the ones it takes, each in backticks.
    let mut words = rest.split('`').step_by(2);
    let Some(bad) = words.next() else {
        return false;
    };
    // The word as written first, and its alias only when no variant is that word:
    // an alias is not told which enum it repairs (`fit` is an image fit's own).
    let words: Vec<&str> = words.collect();
    let one = |wanted: &str| {
        let mut hits = words.iter().filter(|w| plain(w) == wanted);
        match (hits.next(), hits.next()) {
            (Some(good), None) => Some(*good),
            _ => None,
        }
    };
    let written = plain(bad);
    let Some(good) = one(&written).or_else(|| one(alias(&written))) else {
        return false;
    };
    match walk_mut(v, &miss.steps) {
        Some(at @ Value::String(_)) if at.as_str() == Some(bad) => {
            *at = Value::from(good);
            true
        }
        Some(Value::Object(map)) if map.contains_key(bad) => {
            let inner = map.remove(bad).unwrap_or(Value::Null);
            map.insert(good.to_owned(), inner);
            true
        }
        _ => false,
    }
}

/// The refusal for a slip [`repair`] cannot put right, with the form to write:
/// the variant's own form when one was written without its fields, the fields
/// there are when one is unknown, else the model's example.
fn explain(model: &str, v: &Value, miss: &Miss) -> String {
    let said = miss.said(v);
    let compact = |e: &Value| serde_json::to_string(e).unwrap_or_default();
    let named = match miss.steps.last() {
        Some(Some(Step::Key(k))) => Some(k.clone()),
        _ => walk_ref(v, &miss.steps)
            .and_then(Value::as_str)
            .map(str::to_owned),
    };
    // A flex or grid frame places its children itself, so a transform alone is
    // dropped there: the way out of the flow is the CSS one, insets.
    const OUT_OF_FLOW: &str = "inside a flex or grid frame a child leaves the flow, as CSS \
         `position: absolute` does, by its `insets`, as `{\"top\": 12, \"right\": \"5%\"}` \
         (`set_insets` on a layer already there)";
    if model == "Tree" && matches!(named.as_deref(), Some("at" | "x" | "y" | "position")) {
        return format!(
            "{said}; a tree places a child by `transform`, as [1, 0, 0, 1, x, y], \
             and the root by `insert_subtree`'s `x` and `y`; {OUT_OF_FLOW}"
        );
    }
    if model == "LayoutItem" && matches!(named.as_deref(), Some("position" | "absolute")) {
        return format!("{said}; a layout item has no position: {OUT_OF_FLOW}");
    }
    if miss.unknown_field
        && named.as_deref() == Some("sizing")
        && matches!(model, "TextStyle" | "BlockStyle" | "ParagraphStyle")
    {
        return format!(
            "{said}; a text's sizing is its geometry, as `set`'s {{\"geometry\": {{\"TextSizing\": \"Auto\"}}}}"
        );
    }
    // `fill` has no one meaning, so it is refused; the refusal names what does it.
    let word = miss
        .why
        .strip_prefix("unknown variant `")
        .and_then(|w| w.split('`').next())
        .map(str::to_lowercase);
    if matches!(named.as_deref(), Some("width" | "height"))
        && matches!(word.as_deref(), Some("fill" | "stretch" | "fill_container"))
    {
        return format!(
            "{said}; a layer fills its parent across the flow by {{\"Percent\": 100}} (a frame's \
             `Auto` is its stored size, which a stretch never changes) and along it by `grow`: 1"
        );
    }
    if model == "Tree"
        && (miss.why.starts_with("unknown field `corner_radius`")
            || miss.why.starts_with("unknown field `corner_radii`"))
    {
        return format!(
            "{said}; a tree's corners are part of its `kind`, as \
             `{{\"Artboard\": {{\"size\": {{\"width\": 320, \"height\": 200}}, \"corner_radii\": 20}}}}`"
        );
    }
    if miss.unknown_field {
        let above = &miss.steps[..miss.steps.len().saturating_sub(1)];
        // The nearest field holding a model of its own answers for the place.
        let from = (0..above.len()).rev().find_map(|i| {
            let key = step_key(v, &miss.steps, i)?;
            field_model(model, key).map(|m| (m, i + 1))
        });
        let (model, from) = from.unwrap_or((model, 0));
        let below = &above[from..];
        let whole = model_fields(model, below, walk_ref(v, &above[..from]));
        // The model's example shows some fields filled in, in the short forms
        // (`gap` for `row_gap` and `column_gap`); the type's own list adds the rest.
        // Not inside a noise's or a glass's kind: the example's kinds are others,
        // whose fields they lack.
        let own_kind = model == "[Effect]" && below.len() == 2 && whole.is_some();
        let mut fields = example(model)
            .filter(|_| !own_kind)
            .and_then(|e| walk_ref(&e, below).cloned())
            .and_then(|e| match e {
                Value::Object(map) => Some(map.keys().cloned().collect::<Vec<_>>()),
                _ => None,
            })
            .unwrap_or_default();
        for f in whole.unwrap_or_default() {
            if !fields.iter().any(|k| k == f) {
                fields.push((*f).to_owned());
            }
        }
        return match (fields.is_empty(), whole.is_some()) {
            (true, _) => said,
            (false, true) => format!("{said}; the fields there are `{}`", fields.join("`, `")),
            (false, false) => format!("{said}; fields there include `{}`", fields.join("`, `")),
        };
    }
    let without_fields = [
        "unit variant",
        "expected struct variant",
        "expected newtype variant",
    ]
    .iter()
    .any(|w| miss.why.contains(w));
    if without_fields && let Some(form) = named.as_deref().and_then(|n| kind_example(&plain(n))) {
        return format!("{said}; write it with its fields, as `{}`", compact(&form));
    }
    // A word written as a key with a value (`{"visible": true}`) is the word alone.
    if miss.why.contains("expected unit")
        && let Some(word) = named.as_deref()
    {
        return format!("{said}; `{word}` takes no value: write it as the word \"{word}\"");
    }
    // A prototype struct or variant missing a field lists them all, as no example
    // shows every action.
    if miss.why.contains("missing field")
        && matches!(model, "[Reaction]" | "[Flow]" | "LayerPlay" | "PagePlay")
        && let Some(fields) = play_fields(&miss.steps)
    {
        return format!("{said}; the fields there are `{}`", fields.join("`, `"));
    }
    // A kind missing a field is shown that kind, not the model's example of another.
    // Kinds alone, named as the model spells a variant: a style's `text` is no kind.
    if miss.why.contains("missing field")
        && let Some(n) = named
            .as_deref()
            .filter(|n| n.starts_with(char::is_uppercase))
        && let Some(form) = kind_example(&plain(n))
    {
        return format!("{said}; for example `{}`", compact(&form));
    }
    // The example of the nearest field holding a model of its own: a fill written
    // wrong inside a tree is shown fills, not the whole tree.
    let nearest = (0..miss.steps.len())
        .rev()
        .find_map(|i| field_model(model, step_key(v, &miss.steps, i)?))
        .unwrap_or(model);
    // A style's chart holds the look alone, which `ChartOptions`'s example is not.
    let example = match (model, nearest) {
        ("StyleValue", "ChartOptions") => Some(chart_look_example()),
        _ => example(nearest),
    };
    match example {
        Some(e) if miss.why.contains("single key") => {
            format!("{said}: one key naming the variant, as `{}`", compact(&e))
        }
        Some(e) if miss.why.contains("unknown variant") || miss.why.contains("missing field") => {
            format!("{said}; for example `{}`", compact(&e))
        }
        _ => said,
    }
}

/// Every field serde reads at `below` inside a value of `model`, whose value
/// is `at`: the whole list, where an example shows only some. `None` where the
/// place is not a struct of the model's own.
fn model_fields(
    model: &str,
    below: &[Option<Step>],
    at: Option<&Value>,
) -> Option<&'static [&'static str]> {
    use crate::container::{Flex, Grid};
    match (model, below) {
        ("[Stroke]", [Some(Step::Index(_))]) => struct_fields::<Stroke>(),
        ("[Fill]", [Some(Step::Index(_))]) => struct_fields::<Fill>(),
        ("[Effect]", [Some(Step::Index(_))]) => struct_fields::<Effect>(),
        ("[Effect]", [Some(Step::Index(i)), Some(Step::Key(k))]) if k == "kind" => {
            let kind = at?.get(*i)?.get("kind")?;
            match kind.get("type")?.as_str()? {
                "noise" => Some(NOISE_KIND_FIELDS),
                "texture" => Some(TEXTURE_KIND_FIELDS),
                "glass" => Some(GLASS_KIND_FIELDS),
                _ => None,
            }
        }
        ("ParagraphStyle", []) => struct_fields::<ParagraphStyle>(),
        ("TextStyle", []) => struct_fields::<TextStyle>(),
        ("BlockStyle", []) => struct_fields::<BlockStyle>(),
        ("LayoutItem", []) => struct_fields::<crate::container::LayoutItem>(),
        ("Insets", []) => struct_fields::<Insets>(),
        ("ChartOptions", []) => struct_fields::<crate::chart::Options>(),
        ("Display", [_]) => match at.and_then(Value::as_object).map(|m| m.keys().next()) {
            Some(Some(k)) if k == "Flex" => struct_fields::<Flex>(),
            Some(Some(k)) if k == "Grid" => struct_fields::<Grid>(),
            _ => None,
        },
        // A trigger or action is settled alone, its variant the value's one key.
        ("[Reaction]", []) => match at.and_then(Value::as_object) {
            Some(map) if map.len() == 1 => {
                let kind = map.keys().next()?.clone();
                play_fields(&[Some(Step::Key(kind))])
            }
            _ => None,
        },
        ("[Reaction]" | "[Flow]" | "LayerPlay" | "PagePlay", _) => play_fields(below),
        _ => None,
    }
}

/// Every field serde reads at the place `below` names inside a prototype model,
/// by the last key on the way there: a struct the prototype holds under that
/// key, or a trigger's or action's variant.
fn play_fields(below: &[Option<Step>]) -> Option<&'static [&'static str]> {
    use crate::prototype as p;
    let key = below.iter().rev().find_map(|s| match s {
        Some(Step::Key(k)) => Some(k.as_str()),
        _ => None,
    })?;
    match key {
        "settings" => struct_fields::<p::OverlaySettings>(),
        "transition" => struct_fields::<p::Transition>(),
        "follow" => struct_fields::<p::Follow>(),
        "scroll_effect" => struct_fields::<p::ScrollEffect>(),
        "input" => struct_fields::<p::Input>(),
        "cells" => struct_fields::<p::CellWrite>(),
        "branches" => struct_fields::<p::Branch>(),
        k => variant_fields::<p::Action>(k)
            .or_else(|| variant_fields::<p::Trigger>(k))
            .or_else(|| variant_fields::<p::VarChange>(k))
            .or_else(|| variant_fields::<p::WhichRow>(k)),
    }
}

/// The field names a derived `Deserialize` reads `T` by, which serde hands to
/// `deserialize_struct`; `None` for a type read any other way.
fn struct_fields<T: serde::de::DeserializeOwned>() -> Option<&'static [&'static str]> {
    use serde::de::{Deserializer, Visitor};
    use std::cell::Cell;

    /// A deserializer that only notes the fields it is asked for, then stops.
    struct Capture(Cell<Option<&'static [&'static str]>>);
    #[derive(Debug)]
    struct Stop;
    impl std::fmt::Display for Stop {
        fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
            f.write_str("stop")
        }
    }
    impl std::error::Error for Stop {}
    impl serde::de::Error for Stop {
        fn custom<M: std::fmt::Display>(_: M) -> Self {
            Stop
        }
    }
    impl<'de> Deserializer<'de> for &Capture {
        type Error = Stop;
        fn deserialize_any<V: Visitor<'de>>(self, _: V) -> Result<V::Value, Stop> {
            Err(Stop)
        }
        fn deserialize_struct<V: Visitor<'de>>(
            self,
            _: &'static str,
            fields: &'static [&'static str],
            _: V,
        ) -> Result<V::Value, Stop> {
            self.0.set(Some(fields));
            Err(Stop)
        }
        serde::forward_to_deserialize_any! {
            bool i8 i16 i32 i64 i128 u8 u16 u32 u64 u128 f32 f64 char str string
            bytes byte_buf option unit unit_struct newtype_struct seq tuple
            tuple_struct map enum identifier ignored_any
        }
    }
    let capture = Capture(Cell::new(None));
    let _ = T::deserialize(&capture);
    capture.0.get()
}

/// The field names the enum `T`'s struct variant `variant` is read by, as
/// [`struct_fields`] reads a struct's; `None` for a variant of another shape or
/// no such variant.
pub fn variant_fields<T: serde::de::DeserializeOwned>(
    variant: &str,
) -> Option<&'static [&'static str]> {
    enum_capture::<T>(variant).1
}

/// The variant names the enum `T` is read by; `None` for a type that is no
/// enum.
fn enum_variants<T: serde::de::DeserializeOwned>() -> Option<&'static [&'static str]> {
    enum_capture::<T>("").0
}

/// The enum `T`'s variant names, and the fields of its struct variant `variant`,
/// as a derived `Deserialize` hands them to its deserializer.
#[allow(clippy::type_complexity)]
fn enum_capture<T: serde::de::DeserializeOwned>(
    variant: &str,
) -> (
    Option<&'static [&'static str]>,
    Option<&'static [&'static str]>,
) {
    use serde::de::{
        DeserializeSeed, Deserializer, EnumAccess, IntoDeserializer, VariantAccess, Visitor,
    };
    use std::cell::Cell;

    #[derive(Debug)]
    struct Stop;
    impl std::fmt::Display for Stop {
        fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
            f.write_str("stop")
        }
    }
    impl std::error::Error for Stop {}
    impl serde::de::Error for Stop {
        fn custom<M: std::fmt::Display>(_: M) -> Self {
            Stop
        }
    }
    /// Notes the enum's variants, hands it `variant` by name, then notes that
    /// variant's fields and stops.
    struct Capture<'a> {
        variant: &'a str,
        variants: Cell<Option<&'static [&'static str]>>,
        fields: Cell<Option<&'static [&'static str]>>,
    }
    impl<'de> Deserializer<'de> for &Capture<'_> {
        type Error = Stop;
        fn deserialize_any<V: Visitor<'de>>(self, _: V) -> Result<V::Value, Stop> {
            Err(Stop)
        }
        fn deserialize_enum<V: Visitor<'de>>(
            self,
            _: &'static str,
            variants: &'static [&'static str],
            visitor: V,
        ) -> Result<V::Value, Stop> {
            self.variants.set(Some(variants));
            visitor.visit_enum(self)
        }
        serde::forward_to_deserialize_any! {
            bool i8 i16 i32 i64 i128 u8 u16 u32 u64 u128 f32 f64 char str string
            bytes byte_buf option unit unit_struct newtype_struct seq tuple
            tuple_struct map struct identifier ignored_any
        }
    }
    impl<'de> EnumAccess<'de> for &Capture<'_> {
        type Error = Stop;
        type Variant = Self;
        fn variant_seed<S: DeserializeSeed<'de>>(self, seed: S) -> Result<(S::Value, Self), Stop> {
            let name: serde::de::value::StrDeserializer<'_, Stop> =
                self.variant.into_deserializer();
            Ok((seed.deserialize(name)?, self))
        }
    }
    impl<'de> VariantAccess<'de> for &Capture<'_> {
        type Error = Stop;
        fn unit_variant(self) -> Result<(), Stop> {
            Err(Stop)
        }
        fn newtype_variant_seed<S: DeserializeSeed<'de>>(self, _: S) -> Result<S::Value, Stop> {
            Err(Stop)
        }
        fn tuple_variant<V: Visitor<'de>>(self, _: usize, _: V) -> Result<V::Value, Stop> {
            Err(Stop)
        }
        fn struct_variant<V: Visitor<'de>>(
            self,
            fields: &'static [&'static str],
            _: V,
        ) -> Result<V::Value, Stop> {
            self.fields.set(Some(fields));
            Err(Stop)
        }
    }
    let capture = Capture {
        variant,
        variants: Cell::new(None),
        fields: Cell::new(None),
    };
    let _ = T::deserialize(&capture);
    (capture.variants.get(), capture.fields.get())
}

/// The shape of the prototype model `name` (`Reaction`, `Action`, `Trigger`,
/// `Flow`, `LayerPlay` and the structs and enums they hold), for
/// `describe_tools`: a struct's `fields`, or an enum's `variants`, each with its
/// fields or `null` for a word or a single value; with the model's example where
/// it has one. An action's or trigger's own name (`EditRow`) answers its fields.
/// `None` for a name that is none of these.
pub fn model_shape(name: &str) -> Option<Value> {
    use crate::prototype as p;
    use serde_json::json;
    fn variants<T: serde::de::DeserializeOwned>() -> Option<Value> {
        let names = enum_variants::<T>()?;
        let map: serde_json::Map<String, Value> = names
            .iter()
            .map(|v| ((*v).to_owned(), json!(variant_fields::<T>(v))))
            .collect();
        Some(json!({ "variants": map }))
    }
    fn fields<T: serde::de::DeserializeOwned>() -> Option<Value> {
        Some(json!({ "fields": struct_fields::<T>()? }))
    }
    let bare = name.trim_start_matches('[').trim_end_matches(']');
    let mut shape = match bare {
        "Reaction" => fields::<p::Reaction>(),
        "Trigger" => variants::<p::Trigger>(),
        "Action" => variants::<p::Action>(),
        "VarChange" => variants::<p::VarChange>(),
        "WhichRow" => variants::<p::WhichRow>(),
        "CellWrite" => fields::<p::CellWrite>(),
        "Branch" => fields::<p::Branch>(),
        "Transition" => fields::<p::Transition>(),
        "OverlaySettings" => fields::<p::OverlaySettings>(),
        "Flow" => fields::<p::Flow>(),
        "LayerPlay" => fields::<p::LayerPlay>(),
        "Follow" => fields::<p::Follow>(),
        "ScrollEffect" => fields::<p::ScrollEffect>(),
        "Input" => fields::<p::Input>(),
        "PagePlay" => fields::<p::PagePlay>(),
        "PlayExport" => fields::<p::PlayExport>(),
        "TransitionKind" => variants::<p::TransitionKind>(),
        "Direction" => variants::<p::Direction>(),
        "Timing" => variants::<p::Timing>(),
        "Easing" => variants::<p::Easing>(),
        "OverlayPosition" => variants::<p::OverlayPosition>(),
        "Side" => variants::<p::Side>(),
        "ScrollAxis" => variants::<p::ScrollAxis>(),
        "Snap" => variants::<p::Snap>(),
        "SnapAlign" => variants::<p::SnapAlign>(),
        "Pin" => variants::<p::Pin>(),
        "FollowRange" => variants::<p::FollowRange>(),
        "Release" => variants::<p::Release>(),
        // An action's or trigger's own name answers its fields.
        v => {
            let of = |names: Option<&[&str]>| names.is_some_and(|n| n.contains(&v));
            let (of, fields) = if of(enum_variants::<p::Action>()) {
                ("Action", variant_fields::<p::Action>(v))
            } else if of(enum_variants::<p::Trigger>()) {
                ("Trigger", variant_fields::<p::Trigger>(v))
            } else {
                return None;
            };
            return Some(json!({ "variant_of": of, "fields": fields }));
        }
    }?;
    let model = match bare {
        "Reaction" | "Flow" => format!("[{bare}]"),
        _ => bare.to_owned(),
    };
    if let Some(e) = example(&model) {
        shape["example"] = e;
    }
    Some(shape)
}

/// The model a field of that name holds, wherever it sits inside `model`.
fn field_model(model: &str, key: &str) -> Option<&'static str> {
    Some(match key {
        // Elsewhere a layer's `chart` is the whole chart, not its options.
        "chart" if model == "StyleValue" => "ChartOptions",
        "fills" => "[Fill]",
        "strokes" => "[Stroke]",
        "effects" => "[Effect]",
        "display" => "Display",
        "item" => "LayoutItem",
        "style" | "text_style" => "TextStyle",
        "paragraph" => "ParagraphStyle",
        "block" => "BlockStyle",
        "insets" => "Insets",
        _ => return None,
    })
}

/// The key `steps[i]` goes through in `v`: a field's name, or for an enum's
/// variant, which serde leaves unnamed, the one key of the object there.
fn step_key<'v>(v: &'v Value, steps: &'v [Option<Step>], i: usize) -> Option<&'v str> {
    match steps.get(i)? {
        Some(Step::Key(k)) => Some(k),
        Some(Step::Index(_)) => None,
        None => match walk_ref(v, &steps[..i])? {
            Value::Object(map) if map.len() == 1 => map.keys().next().map(String::as_str),
            _ => None,
        },
    }
}

/// `v`'s value at `steps`, as [`walk_mut`] finds it.
fn walk_ref<'v>(mut at: &'v Value, steps: &[Option<Step>]) -> Option<&'v Value> {
    for step in steps {
        at = match (step, at) {
            (Some(Step::Key(k)), Value::Object(map)) => map.get(k)?,
            (Some(Step::Index(i)), Value::Array(list)) => list.get(*i)?,
            (None, Value::Object(map)) if map.len() == 1 => map.values().next()?,
            _ => return None,
        };
    }
    Some(at)
}

/// Component properties as a refusal lists them: each name quoted, with its kind.
fn listed_props(props: Vec<&crate::variant::Property>) -> String {
    props
        .iter()
        .map(|p| {
            let kind = format!("{:?}", p.kind).to_lowercase();
            format!("\"{}\" ({kind})", p.name)
        })
        .collect::<Vec<_>>()
        .join(", ")
}

/// Move `id`, which `tx` inserts, so its box's left and top land on the verb's
/// `x` and `y` in its parent; an edge not given stays where it is.
fn place_corner(doc: &Document, a: &Args<'_>, tx: &mut Transaction, id: NodeId) {
    let (x, y) = (a.number("x"), a.number("y"));
    if (x.is_some() || y.is_some())
        && let Some(op) = build::cornered_at(doc, tx, id, x, y)
    {
        tx.0.push(op);
    }
}

/// A layer kind written with its fields, by the kind's name reduced by
/// [`plain`]: what an agent that wrote `"Ellipse"` alone is shown.
fn kind_example(plain: &str) -> Option<Value> {
    use serde_json::json;
    let size = json!({ "width": 120, "height": 80 });
    Some(match alias(plain) {
        "artboard" => json!({ "Artboard": { "size": size } }),
        "rect" => json!({ "Rect": { "size": size, "corner_radii": 8 } }),
        "ellipse" => json!({ "Ellipse": { "size": size } }),
        "polygon" => json!({ "Polygon": { "size": size, "sides": 6 } }),
        "star" => json!({ "Star": { "size": size, "points": 5 } }),
        "line" => json!({ "Line": { "end": [120, 0] } }),
        "arc" => json!({ "Arc": { "end": [120, 0], "sweep": 90 } }),
        "spiral" => json!({ "Spiral": { "size": size, "turns": 3 } }),
        "path" => json!({ "Path": { "path": "M0,0 L120,0 L60,80 Z" } }),
        "text" => json!({ "Text": { "content": "Hello", "style": { "font_size": 16 } } }),
        _ => return None,
    })
}

/// One step down a JSON value: a member by its key or an item by its place.
#[derive(Clone)]
enum Step {
    Key(String),
    Index(usize),
}

/// The steps of a path `serde_ignored` reports, `None` where it cannot name one
/// (an enum's variant).
fn ignored_steps(path: &serde_ignored::Path<'_>) -> Vec<Option<Step>> {
    use serde_ignored::Path;
    let (parent, step) = match path {
        Path::Root => return Vec::new(),
        Path::Seq { parent, index } => (*parent, Some(Step::Index(*index))),
        Path::Map { parent, key } => (*parent, Some(Step::Key(key.clone()))),
        Path::Some { parent } | Path::NewtypeStruct { parent } => return ignored_steps(parent),
        Path::NewtypeVariant { parent } => (*parent, None),
    };
    let mut steps = ignored_steps(parent);
    steps.push(step);
    steps
}

/// `steps` written against `v` as `children[1].kind.Text`, an unnamed step taken
/// as the one key of an externally tagged enum, which is how the value spells it.
fn json_path(v: &Value, steps: Vec<Option<Step>>) -> String {
    let mut out = String::new();
    let mut at = Some(v);
    for step in steps {
        let step = match step {
            Some(s) => s,
            None => match at.and_then(Value::as_object) {
                Some(map) if map.len() == 1 => {
                    Step::Key(map.keys().next().cloned().unwrap_or_default())
                }
                _ => continue,
            },
        };
        match &step {
            Step::Key(k) => {
                if !out.is_empty() {
                    out.push('.');
                }
                out.push_str(k);
                at = at.and_then(|v| v.get(k));
            }
            Step::Index(i) => {
                out.push_str(&format!("[{i}]"));
                at = at.and_then(|v| v.get(*i));
            }
        }
    }
    out
}

/// `v` with every null member of an object left out.
fn without_nulls(v: &Value) -> Value {
    match v {
        Value::Object(map) => Value::Object(
            map.iter()
                .filter(|(_, v)| !v.is_null())
                .map(|(k, v)| (k.clone(), without_nulls(v)))
                .collect(),
        ),
        other => other.clone(),
    }
}

/// A value of the model type `model` names, in the form a parameter of it takes:
/// what a caller is shown, since a serde form cannot be guessed (a rectangle
/// needs its `corner_radii`, an image brush its `sampler`). `None` for a name no
/// parameter uses.
pub fn example(model: &str) -> Option<Value> {
    use crate::effect::{EffectKind, Shadow};
    use crate::export::{ExportFormat, ExportScale};
    use crate::typography::CharAttr;
    use kurbo::{Size, Vec2};
    use peniko::Color;

    fn json<T: serde::Serialize>(v: &T) -> Value {
        serde_json::to_value(v).unwrap_or_default()
    }
    let rect = NodeKind::Rect {
        size: Size::new(120.0, 80.0),
        corner_radii: kurbo::RoundedRectRadii::from_single_radius(8.0),
        corner_smoothing: 0.0,
    };
    let fill = Fill {
        brush: peniko::Brush::Solid(Color::from_rgb8(0x33, 0x66, 0xcc)).into(),
        visible: true,
    };
    let gradient_fill = Fill {
        brush: peniko::Brush::Gradient(crate::GradientBrush {
            gradient: peniko::Gradient::new_linear((0.0, 0.0), (0.0, 80.0)).with_stops(
                [
                    Color::from_rgb8(0x33, 0x66, 0xcc),
                    Color::from_rgba8(0x33, 0x66, 0xcc, 0),
                ]
                .as_slice(),
            ),
            transform: Affine::IDENTITY,
            opacity: 1.0,
        })
        .into(),
        visible: true,
    };
    let image_fill = Fill {
        brush: crate::image::image_brush(crate::image::ImageId("<add_image's id>".into())).into(),
        visible: true,
    };
    Some(match model {
        "Affine" => json(&Affine::translate((40.0, 24.0))),
        "BlockStyle" => json(&BlockStyle::default()),
        "CharSpans" => {
            let mut spans = CharSpans::default();
            spans.set(0..5, CharAttr::Weight(700), &TextStyle::default());
            json(&spans)
        }
        // Filled in, since a default flex writes as `{}` and shows no field.
        "Display" => serde_json::json!({ "Flex": {
            "direction": "Row",
            "align_items": "Center",
            "justify_content": "SpaceBetween",
            "gap": 12,
            "padding": [16, 24],
        } }),
        "GeometryPatch" => json(&GeometryPatch::Size(Size::new(200.0, 100.0))),
        // Filled in, since a default one writes as `{}`.
        "Insets" => serde_json::json!({ "top": { "Px": 12 }, "right": { "Px": 12 } }),
        "LayoutItem" => serde_json::json!({
            "width": { "Px": 320 },
            "height": "Auto",
            "grow": 1,
            "align_self": "Center",
        }),
        "NodeKind" => json(&rect),
        // Filled in, since a default one writes as `{}`.
        "ParagraphStyle" => serde_json::json!({ "align": "Center", "spacing": "12px" }),
        "Pivot" => json(&Pivot::Normalized(Vec2::new(0.5, 0.5))),
        "TextStyle" => serde_json::json!({
            "font_family": "Inter",
            "font_size": 16,
            "weight": 600,
            "italic": false,
            "line_height": "22px",
            "letter_spacing": "-0.2px",
            "color": "#12352a",
            "case": "Upper",
        }),
        "[CharAttr]" => json(&vec![
            CharAttr::Weight(700),
            CharAttr::Size(24.0),
            CharAttr::LetterSpacing(Length::Em(0.02)),
        ]),
        "Cell" => json(&Cell::Color(Color::from_rgb8(0x33, 0x66, 0xcc))),
        "StyleValue" => serde_json::json!({ "fills": [json(&fill)] }),
        "BoundField" => json(&build::BoundField::Fill(0)),
        "Color" => json(&Color::from_rgb8(0x1f, 0x3a, 0x5f)),
        "[Hex]" => serde_json::json!(["#203b5eff", "#1e395fff"]),
        "[Node]" => serde_json::json!(["7e5b:12", "7e5b:40"]),
        "Props" => serde_json::json!({
            "name": "Title",
            "fills": ["#12352a"],
            "size": { "width": 320.0, "height": 48.0 },
            "at": [120.0, 96.0],
        }),
        "Scope" => serde_json::json!(["corner", "spacing"]),
        "PropValue" => Value::Bool(true),
        "[ParaAttr]" => json(&vec![
            ParaAttr::Spacing(Length::Px(12.0)),
            ParaAttr::IndentStart(Length::Px(24.0)),
        ]),
        // Written in the short forms, so the example shows them.
        "Tree" => serde_json::json!({
            "kind": { "Artboard": { "size": { "width": 320, "height": 200 } } },
            "name": "Card",
            "fills": ["#ffffff"],
            "display": { "Flex": { "direction": "Column", "gap": 12, "padding": 16 } },
            "children": [
                {
                    "kind": { "Rect": { "size": { "width": 288, "height": 96 }, "corner_radii": 8 } },
                    "name": "Picture",
                    "fills": ["#3366cc"],
                    "effects": [{ "kind": { "type": "drop_shadow", "offset": [0, 6], "blur": 14, "spread": 0, "color": "#12352a2e" } }],
                },
                {
                    "kind": { "Text": {
                        "content": "Hello",
                        "sizing": { "AutoHeight": 288 },
                        "paragraph": { "align": "Center" },
                        "style": { "font_family": "Inter", "font_size": 16, "weight": 600, "line_height": "22px" },
                    } },
                    "fills": ["#12352a"],
                },
            ],
        }),
        "[Effect]" => json(&vec![
            Effect::new(EffectKind::DropShadow(Shadow::default())),
            Effect::new(EffectKind::BackgroundBlur {
                radius: 24.0,
                ramp: Some(crate::effect::BlurRamp::down(0.0)),
            }),
        ]),
        "[ExportSpec]" => json(&vec![ExportSpec::new(
            ExportFormat::Png,
            ExportScale::default(),
        )]),
        "[Fill]" => json(&vec![fill, gradient_fill, image_fill]),
        "[Stroke]" => json(&vec![Stroke::default()]),
        "[LayoutGrid]" => serde_json::json!([
            { "axis": "Columns", "count": 4, "gutter": 24, "margin": 32 },
            { "axis": "Rows", "count": 8, "align": "Start", "size": 64, "visible": false },
        ]),
        "[Guide]" => serde_json::json!([
            { "axis": "vertical", "position": 160 },
            { "axis": "horizontal", "position": 80, "color": "#0a84ff" },
        ]),
        "Quad" => serde_json::json!([[0, 0], [200, -20], [200, 120], [0, 100]]),
        "RepeatLine" => serde_json::json!({ "count": 4, "gap": 8, "down": true }),
        "RepeatGrid" => serde_json::json!({ "cols": 3, "rows": 2, "gap": [8, 8] }),
        "ChartTable" => json(&crate::chart::Chart::sample(crate::chart::ChartKind::Column).table),
        "ChartMapping" => {
            json(&crate::chart::Chart::sample(crate::chart::ChartKind::Column).mapping)
        }
        "ChartOptions" => serde_json::json!({
            "bars": { "stacking": "Stacked", "radius": 4.0 },
            "legend": { "position": "Bottom" },
        }),
        "ChartColours" => serde_json::json!({ "Revenue": "#3b82f6" }),
        "ChartSeries" => serde_json::json!({ "Revenue": { "dash": "Dashed", "point": "Square" } }),
        "ChartHighlight" => serde_json::json!({
            "target": { "Categories": ["Apr"] }, "others": "Muted",
        }),
        "[ChartReference]" => serde_json::json!([
            { "id": 1, "at": { "Value": 2600 }, "label": "Target {value}", "colour": "#c0392b" },
            { "id": 2, "at": { "Stat": { "stat": "Average", "of": null } }, "label": "Average" },
        ]),
        "[Reaction]" => serde_json::json!([
            { "trigger": "Click", "actions": [{ "Navigate": { "to": "7e5b:40" } }] },
            { "trigger": "HoverWhile", "actions": [{ "ChangeState": {
                "instance": "7e5b:12", "to": "7e5b:31", "transition": "SmartAnimate" } }] },
            { "trigger": "Click", "actions": [{ "OpenOverlay": { "overlay": "7e5b:52",
                "settings": { "position": "BottomCentre" } } }] },
            { "trigger": "Click", "actions": [{ "Conditional": {
                "branches": [{ "when": "{Count} < 9", "actions": [{ "SetVariable": {
                    "var": "7e5b:9", "change": { "Set": { "to": "{Count} + 1" } } } }] }],
                "otherwise": ["Back"] } }] },
        ]),
        "LayerPlay" => serde_json::json!({
            "scroll": "Horizontal", "snap": "Children", "snap_align": "Centre",
            "follow": { "axis": "Vertical", "range": "Parent", "release": "Returns" },
            "scroll_effect": { "from": 0, "to": 200,
                "props": [{ "prop": "Opacity", "start": 1, "end": 0 }] },
        }),
        "PlayExport" => serde_json::json!({
            "flows": [{ "page": "7e5b:1", "flow": "7e5b:20" }],
            "device_frame": false, "scaling": "Fit",
        }),
        "[Flow]" => serde_json::json!([{ "name": "Checkout", "start": "7e5b:40" }]),
        "PagePlay" => serde_json::json!({
            "scaling": "Fit",
            "transition": { "kind": "Push", "direction": "Right" },
        }),
        "TableRows" => serde_json::json!([["Linen", "12"], ["Wool", "18"]]),
        "[ColumnName]" => serde_json::json!(["Name", "Price"]),
        "[TableCell]" => serde_json::json!([{ "row": 1, "column": "Price", "value": "14" }]),
        "[TableRow]" => serde_json::json!([{ "Name": "Silk", "Price": "30" }]),
        "[RowNumber]" => serde_json::json!([3]),
        "TableColumns" => serde_json::json!({
            "add": ["Stock"], "rename": { "Price": "Price (€)" }, "delete": ["Notes"],
        }),
        "ListRows" => serde_json::json!({ "first": 3 }),
        "ListFill" => serde_json::json!({ "Title": "Name" }),
        "ListFilter" => serde_json::json!({
            "tests": [{ "column": "Price", "op": "less", "value": "20" }],
        }),
        "ListSort" => serde_json::json!([{ "column": "Price", "descending": true }]),
        _ => return None,
    })
    .map(|mut v| {
        // Shown in the short forms, which are the ones worth writing.
        if !as_written(model) {
            loose::tighten(model_key(model, ""), &mut v);
        }
        v
    })
}

/// Values of `model` shown after `example`'s, where one value cannot show every
/// form: a style's value is paints, a text or layout style, or a chart's look.
pub fn more_examples(model: &str) -> Vec<Value> {
    match model {
        "StyleValue" => vec![
            serde_json::json!({ "chart": chart_look_example() }),
            serde_json::json!({ "effects": [
                { "type": "background_blur", "radius": 24,
                  "ramp": { "end": 0, "angle": 90, "ease": "smooth" } },
                { "type": "texture", "size": 2, "amount": 4, "seed": 1, "detail": 3 },
                { "type": "glass", "blur": 16, "frost": 0.1, "bend": 0.3, "depth": 12,
                  "rim": { "angle": 135, "width": 1, "color": "#ffffff99" } },
            ] }),
        ],
        _ => Vec::new(),
    }
}

/// A chart style's value under its `chart` key: the look alone, so no stacking,
/// unlike `ChartOptions`'s example.
fn chart_look_example() -> Value {
    serde_json::json!({
        "bars": { "radius": 4.0 },
        "legend": { "position": "Bottom" },
    })
}

/// The field name the short forms read a model's value under: its own kind's for
/// a colour or paint list, whatever its parameter is called, else the parameter's.
fn model_key<'a>(model: &str, param: &'a str) -> &'a str {
    match model {
        "Color" => "color",
        "TextStyle" => "style",
        "[Fill]" => "fills",
        "[Stroke]" => "strokes",
        "[Effect]" => "effects",
        "[LayoutGrid]" => "grids",
        "Insets" => "insets",
        "BoundField" => "field",
        _ => param,
    }
}

/// `n` with the noun that counts it: "1 match", "3 matches".
fn plural(n: usize, one: &str, many: &str) -> String {
    format!("{n} {}", if n == 1 { one } else { many })
}

/// The verb called `name`.
pub fn find(name: &str) -> Option<&'static Verb> {
    VERBS.iter().find(|v| v.name == name)
}

/// Check `args` against the verb called `name` and answer its edit.
pub fn call(ctx: &mut Ctx<'_>, name: &str, args: &Value) -> Result<Done, VerbError> {
    let verb = find(name).ok_or_else(|| VerbError::Unknown(name.to_owned()))?;
    let args = by_names(ctx.doc, verb, args)?;
    let args = Args::check(verb, &args)?;
    refuse_list_copies(ctx.doc, verb, &args)?;
    (verb.run)(ctx, &args)
}

/// The verbs whose `node` or `nodes` is only read from, so a list's copy may be
/// named: a style or a component made from its look, a push of its overrides.
const READS_ITS_NODE: &[&str] = &["add_style", "duplicate_as_component", "push_to_main"];

/// Refuse a write that names a layer of a list's copy as its `node`, `nodes` or
/// `parent`: the copies are made again from the template whenever the list
/// regenerates, so the edit would land and then be lost without a word.
fn refuse_list_copies(
    doc: &Document,
    verb: &'static Verb,
    args: &Args<'_>,
) -> Result<(), VerbError> {
    if READS_ITS_NODE.contains(&verb.name) {
        return Ok(());
    }
    for p in verb.params {
        if !matches!(p.name, "node" | "nodes" | "parent") || args.get(p.name).is_none() {
            continue;
        }
        let ids = match p.kind {
            ParamKind::Node => vec![args.node(p.name)?],
            ParamKind::Nodes => args.nodes(p.name)?,
            _ => continue,
        };
        for id in ids {
            crate::list::refuse_copy(doc, id).map_err(|why| args.invalid(p.name, why))?;
        }
    }
    Ok(())
}

/// `args` with each variable, style, collection, mode or script given by its name
/// taken as its id, and a variable's `value` given bare (`"#ff7a1a"`, `16`)
/// written as the cell of the variable's kind.
fn by_names(doc: &Document, verb: &'static Verb, args: &Value) -> Result<Value, VerbError> {
    let mut args = args.clone();
    let Value::Object(map) = &mut args else {
        return Ok(args);
    };
    for p in verb.params {
        let ParamKind::Id(what) = p.kind else {
            continue;
        };
        let Some(Value::String(name)) = map.get(p.name) else {
            continue;
        };
        if NodeId::from_wire(name).is_some() {
            continue;
        }
        let id = id_named(doc, what, name).map_err(|why| VerbError::Invalid {
            verb: verb.name,
            param: p.name,
            why,
        })?;
        map.insert(p.name.into(), id);
    }
    for p in verb.params {
        let named: fn(&Document, &mut Value) -> Result<(), String> = match p.kind {
            ParamKind::Model("[Reaction]") => reaction_names,
            ParamKind::Model("[Flow]") => flow_names,
            ParamKind::Model("LayerPlay") => play_names,
            _ => continue,
        };
        let Some(v) = map.get_mut(p.name) else {
            continue;
        };
        // Some clients send a parameter with no schema type as JSON text.
        if let Value::String(s) = v
            && let Ok(parsed) = serde_json::from_str::<Value>(s.trim())
        {
            *v = parsed;
        }
        named(doc, v).map_err(|why| VerbError::Invalid {
            verb: verb.name,
            param: p.name,
            why,
        })?;
    }
    if let Some(value) = map.get("value")
        && !value.is_object()
        && !value.is_null()
        && verb
            .params
            .iter()
            .any(|p| p.kind == ParamKind::Model("Cell"))
    {
        let kind = match map.get("kind").and_then(Value::as_str) {
            Some(k) => Some(k.to_owned()),
            None => map
                .get("variable")
                .and_then(|v| serde_json::from_value::<crate::variable::VarId>(v.clone()).ok())
                .and_then(|id| doc.variables().get(id))
                .and_then(|var| serde_json::to_value(var.kind).ok())
                .and_then(|k| k.as_str().map(str::to_owned)),
        };
        if let Some(kind) = kind {
            let cell = serde_json::json!({ kind: value.clone() });
            map.insert("value".into(), cell);
        }
    }
    Ok(args)
}

/// `field`, a variable, mode or script (`what`) given by its name, as its id.
/// A string shaped like an id (`16:9`) is kept as one when that id exists or
/// nothing has it as a name, since a target may dangle.
fn id_field(doc: &Document, what: &str, field: &mut Value) -> Result<(), String> {
    let Value::String(name) = field else {
        return Ok(());
    };
    if let Some(id) = NodeId::from_wire(name) {
        let items = named_items(doc, what)?;
        let held = |v: &Value| v.as_str().and_then(NodeId::from_wire) == Some(id);
        if items.iter().any(|(_, v)| held(v)) || !items.iter().any(|(n, _)| n == name) {
            return Ok(());
        }
    }
    *field = id_named(doc, what, name)?;
    Ok(())
}

/// The list of reactions `v` with each variable, mode, script, table and column
/// its triggers and actions name by name written as its id, as a top-level id
/// parameter takes a name; or the first that names nothing, by its path.
fn reaction_names(doc: &Document, v: &mut Value) -> Result<(), String> {
    for (i, r) in v.as_array_mut().into_iter().flatten().enumerate() {
        if let Some(t) = r.get_mut("trigger") {
            tagged_names(doc, t, &format!("[{i}].trigger"))?;
        }
        if let Some(actions) = r.get_mut("actions") {
            action_names(doc, actions, &format!("[{i}].actions"))?;
        }
    }
    Ok(())
}

/// [`reaction_names`] over a list of actions at `path`, a conditional's too.
fn action_names(doc: &Document, v: &mut Value, path: &str) -> Result<(), String> {
    for (j, a) in v.as_array_mut().into_iter().flatten().enumerate() {
        tagged_names(doc, a, &format!("{path}[{j}]"))?;
    }
    Ok(())
}

/// [`reaction_names`] over one trigger or action at `at`, its kind in any case.
fn tagged_names(doc: &Document, v: &mut Value, at: &str) -> Result<(), String> {
    let Value::Object(map) = v else {
        return Ok(());
    };
    for (kind, body) in map.iter_mut() {
        let Value::Object(fields) = body else {
            continue;
        };
        let at = format!("{at}.{kind}");
        let field = |name: &str| format!("at `{at}.{name}`");
        let what = match plain(kind).as_str() {
            "variablechanges" | "setvariable" => Some(("var", "variable")),
            "setmode" => Some(("mode", "mode")),
            "runscript" => Some(("script", "script")),
            _ => None,
        };
        if let Some((name, what)) = what
            && let Some(f) = fields.get_mut(name)
        {
            id_field(doc, what, f).map_err(|why| format!("{}: {why}", field(name)))?;
        }
        if let Some(t) = fields.get_mut("table") {
            // As `id_field`: an id-shaped name is an id unless only a name matches.
            if let Value::String(name) = t
                && NodeId::from_wire(name).is_none_or(|id| {
                    doc.data_set(crate::list::DataSetId(id)).is_none()
                        && doc.data_sets().iter().any(|d| d.name == *name)
                })
            {
                let set = tables::find_table(doc, name)
                    .map_err(|why| format!("{}: {why}", field("table")))?;
                *t = serde_json::to_value(set.id).unwrap_or_default();
            }
            let set = serde_json::from_value::<crate::list::DataSetId>(t.clone())
                .ok()
                .and_then(|id| doc.data_set(id));
            for (c, cell) in fields
                .get_mut("cells")
                .and_then(Value::as_array_mut)
                .into_iter()
                .flatten()
                .enumerate()
            {
                if let (Some(set), Some(Value::String(name))) = (set, cell.get("column")) {
                    let column = set
                        .table
                        .columns
                        .iter()
                        .find(|col| col.name == *name)
                        .ok_or_else(|| {
                            format!(
                                "{}: `{}` has no column `{name}`",
                                field(&format!("cells[{c}].column")),
                                set.name
                            )
                        })?;
                    cell["column"] = serde_json::to_value(column.id).unwrap_or_default();
                }
            }
        }
        for (b, branch) in fields
            .get_mut("branches")
            .and_then(Value::as_array_mut)
            .into_iter()
            .flatten()
            .enumerate()
        {
            if let Some(actions) = branch.get_mut("actions") {
                action_names(doc, actions, &format!("{at}.branches[{b}].actions"))?;
            }
        }
        if let Some(otherwise) = fields.get_mut("otherwise") {
            action_names(doc, otherwise, &format!("{at}.otherwise"))?;
        }
    }
    Ok(())
}

/// The list of flows `v` with each starting mode and each starting value's
/// variable given by its name written as its id, `values` written as an object
/// of values by variable taken as its pairs, and a value given bare (`"Fix"`,
/// `true`) written as the cell of its variable's kind.
fn flow_names(doc: &Document, v: &mut Value) -> Result<(), String> {
    for (i, flow) in v.as_array_mut().into_iter().flatten().enumerate() {
        if let Some(values) = flow.get_mut("values")
            && let Value::Object(map) = values
        {
            let pairs = map
                .iter()
                .map(|(var, value)| serde_json::json!([var, value]))
                .collect();
            *values = Value::Array(pairs);
        }
        for (m, mode) in flow
            .get_mut("modes")
            .and_then(Value::as_array_mut)
            .into_iter()
            .flatten()
            .enumerate()
        {
            id_field(doc, "mode", mode).map_err(|why| format!("at `[{i}].modes[{m}]`: {why}"))?;
        }
        for (n, pair) in flow
            .get_mut("values")
            .and_then(Value::as_array_mut)
            .into_iter()
            .flatten()
            .enumerate()
        {
            if let Some(var) = pair.get_mut(0) {
                id_field(doc, "variable", var)
                    .map_err(|why| format!("at `[{i}].values[{n}]`: {why}"))?;
            }
            if let Some([var, value]) = pair.as_array_mut().map(Vec::as_mut_slice)
                && let Some(cell) = bare_cell(doc, var, value)
            {
                *value = cell;
            }
        }
    }
    Ok(())
}

/// A layer's play settings `v` with its input's variable given by its name
/// written as its id.
fn play_names(doc: &Document, v: &mut Value) -> Result<(), String> {
    match v.pointer_mut("/input/var") {
        Some(var) => id_field(doc, "variable", var).map_err(|why| format!("at `input.var`: {why}")),
        None => Ok(()),
    }
}

/// `value` given bare (`"#ff7a1a"`, `16`, `true`) as the cell of the kind of the
/// variable `var`; `None` for a value already a cell, or a variable not there.
fn bare_cell(doc: &Document, var: &Value, value: &Value) -> Option<Value> {
    if value.is_object() || value.is_null() {
        return None;
    }
    let kind = serde_json::from_value::<crate::variable::VarId>(var.clone())
        .ok()
        .and_then(|id| doc.variables().get(id))
        .and_then(|var| serde_json::to_value(var.kind).ok())?;
    Some(serde_json::json!({ kind.as_str()?: value.clone() }))
}

/// The id of the `what` (variable, style, collection, mode, script) called `name`, or
/// why there is not exactly one.
fn id_named(doc: &Document, what: &str, name: &str) -> Result<Value, String> {
    let named = named_items(doc, what)?;
    let hits: Vec<&Value> = named
        .iter()
        .filter(|(n, _)| n == name)
        .map(|(_, id)| id)
        .collect();
    match hits.as_slice() {
        [id] => Ok((*id).clone()),
        [] => {
            let mut names: Vec<&str> = named.iter().map(|(n, _)| n.as_str()).collect();
            names.sort_unstable();
            names.dedup();
            let more = names.len().saturating_sub(12);
            names.truncate(12);
            let list = match names.is_empty() {
                true => "there are none".to_owned(),
                false if more > 0 => format!("they are `{}` and {more} more", names.join("`, `")),
                false => format!("they are `{}`", names.join("`, `")),
            };
            Err(format!("no {what} has the id or name `{name}`; {list}"))
        }
        _ => Err(format!(
            "{} {what}s are named `{name}`; give its id",
            hits.len()
        )),
    }
}

/// Every `what` (variable, style, collection, mode, script) by name and wire id.
fn named_items(doc: &Document, what: &str) -> Result<Vec<(String, Value)>, String> {
    let vars = doc.variables();
    Ok(match what {
        "variable" => vars
            .iter()
            .map(|v| (v.name.clone(), serde_json::to_value(v.id)))
            .map(|(n, id)| (n, id.unwrap_or_default()))
            .collect(),
        "style" => doc
            .styles()
            .iter()
            .map(|s| {
                (
                    s.name.clone(),
                    serde_json::to_value(s.id).unwrap_or_default(),
                )
            })
            .collect(),
        "collection" => vars
            .collections()
            .iter()
            .map(|c| {
                (
                    c.name.clone(),
                    serde_json::to_value(c.id).unwrap_or_default(),
                )
            })
            .collect(),
        "mode" => vars
            .modes()
            .map(|m| {
                (
                    m.name.clone(),
                    serde_json::to_value(m.id).unwrap_or_default(),
                )
            })
            .collect(),
        "script" => doc
            .scripts()
            .iter()
            .map(|s| {
                (
                    s.name.clone(),
                    serde_json::to_value(s.id).unwrap_or_default(),
                )
            })
            .collect(),
        _ => return Err(format!("expected a {what}'s id")),
    })
}

const NODE: ParamKind = ParamKind::Node;
const NODES: ParamKind = ParamKind::Nodes;
const BOOL: ParamKind = ParamKind::Bool;
const TEXT: ParamKind = ParamKind::Text;
const INDEX: ParamKind = ParamKind::Index;

const BOOL_OPS: &[&str] = &["union", "subtract", "intersect", "exclude"];
const EDGES: &[&str] = &["left", "center", "right", "top", "middle", "bottom"];
const AXES: &[&str] = &["horizontal", "vertical"];
const VAR_KINDS: &[&str] = &["color", "number", "boolean", "string"];
const RESETS: &[&str] = &["all", "fields", "children", "order"];
const PUSHES: &[&str] = &["all", "properties", "fields", "layers", "order", "removals"];
const LEVELS: &[&str] = &["aa", "aaa", "apca"];
const PROP_KINDS: &[&str] = &["boolean", "text", "swap", "image"];
/// The mask modes by the word a verb takes, in [`crate::node::MaskMode::ALL`]'s order.
const MASK_MODES: &[&str] = &["shape", "alpha", "luminance"];
/// The HTML elements by the tag a verb takes them as.
const ELEMENTS: &[&str] = &[
    "main", "section", "article", "aside", "header", "footer", "nav", "ul", "button", "a", "h1",
    "h2", "h3", "h4", "h5", "h6",
];
/// The browser states a variant value can be tied to, as `variant::Interaction`
/// writes them.
const STATES: &[&str] = &["hover", "focus-visible", "active", "disabled"];
const STYLE_SLOTS: &[&str] = &["fills", "strokes", "effects", "text", "layout", "chart"];

/// The style slot a [`STYLE_SLOTS`] word names.
fn slot_of(word: &str) -> StyleSlot {
    match word {
        "strokes" => StyleSlot::Strokes,
        "effects" => StyleSlot::Effects,
        "text" => StyleSlot::Text,
        "layout" => StyleSlot::Layout,
        "chart" => StyleSlot::Chart,
        _ => StyleSlot::Fills,
    }
}
const MOVES: &[&str] = &["front", "forward", "backward", "back"];
/// The blend modes by [`BlendMode::label`], as the snapshot names them, in
/// [`BlendMode::ALL`]'s order.
pub const BLENDS: &[&str] = &[
    "Pass through",
    "Normal",
    "Darken",
    "Multiply",
    "Color burn",
    "Lighten",
    "Screen",
    "Color dodge",
    "Overlay",
    "Soft light",
    "Hard light",
    "Difference",
    "Exclusion",
    "Hue",
    "Saturation",
    "Color",
    "Luminosity",
];

/// The layer named `id`, or [`OpError::NoSuchNode`].
fn node<'d>(ctx: &Ctx<'d>, id: NodeId) -> Result<&'d crate::node::Node, VerbError> {
    ctx.doc
        .get(id)
        .ok_or(VerbError::Refused(OpError::NoSuchNode(id)))
}

/// The layers a style verb's `nodes` names, each checked to exist: a paint
/// slot's [`crate::style::targets`] drops a missing id, which would otherwise
/// answer as done with nothing bound.
fn style_nodes(ctx: &Ctx<'_>, a: &Args<'_>) -> Result<Vec<NodeId>, VerbError> {
    let nodes = a.nodes("nodes")?;
    for id in &nodes {
        node(ctx, *id)?;
    }
    Ok(nodes)
}

/// `set_point`: one anchor of a path edited as the inspector's point fields edit
/// it, read and written in the layer's own space. A coordinate left out keeps
/// its value, a handle is placed after the point moves, and one handle given
/// mirrors the other only on a smooth point. The whole path is written back
/// through `edit::rewrite_points`, which keeps a mirrored path symmetric.
fn set_point(ctx: &mut Ctx<'_>, a: &Args<'_>) -> Result<Done, VerbError> {
    use crate::edit::Side;
    const EDITS: [&str; 7] = ["x", "y", "in_x", "in_y", "out_x", "out_y", "radius"];
    let id = a.layer(ctx.doc, "node")?;
    node(ctx, id)?;
    let (world, base) = crate::edit::path_subpaths(ctx.doc, ctx.res, id)
        .ok_or_else(|| a.invalid("node", "not a path"))?;
    let sub = a.index("subpath").unwrap_or(0);
    let count = base
        .get(sub)
        .ok_or_else(|| a.invalid("subpath", format!("the path has {} subpaths", base.len())))?
        .anchors
        .len();
    let at = a.index("point").unwrap_or(0);
    if at >= count {
        return Err(a.invalid("point", format!("subpath {sub} has {count} points")));
    }
    if EDITS.iter().all(|p| a.number(p).is_none()) {
        return Err(VerbError::Nothing(
            "Give the point a place, a handle or a radius",
        ));
    }
    let radius = a.number("radius");
    if radius.is_some_and(|r| r < 0.0) {
        return Err(a.invalid("radius", "a radius is not negative"));
    }
    let to_local = world.inverse();
    // Absent coordinates fall back to `now`, in the layer's space.
    let place = |x: &'static str, y: &'static str, now: kurbo::Point| {
        let now = to_local * now;
        (a.number(x).is_some() || a.number(y).is_some()).then(|| {
            world * kurbo::Point::new(a.number(x).unwrap_or(now.x), a.number(y).unwrap_or(now.y))
        })
    };
    let mut subpaths = base.clone();
    let an = &mut subpaths[sub].anchors[at];
    if let Some(p) = place("x", "y", an.at) {
        an.at = p;
    }
    let smooth = an.is_smooth();
    let aim_in = place("in_x", "in_y", an.arriving());
    let aim_out = place("out_x", "out_y", an.leaving());
    match (aim_in, aim_out) {
        (Some(i), Some(o)) => {
            an.back = i - an.at;
            an.out = o - an.at;
        }
        (Some(i), None) => an.pull_side(Side::Back, i, !smooth),
        (None, Some(o)) => an.pull_side(Side::Out, o, !smooth),
        (None, None) => {}
    }
    if let Some(r) = radius {
        an.radius = r;
    }
    Ok(Done::edit(crate::edit::rewrite_points(
        ctx.doc, ctx.res, id, world, &base, subpaths,
    )))
}

/// Whether `patch` writes the drawn outline of the rounded path `id` back as its
/// points, which would bake each fillet into curves and round it again.
///
/// The snapshot's `svg` is that outline, so it is the likely mistake. A patch is
/// caught by any one point the outline has only because a corner is rounded, so
/// the outline written back with an edit is caught too. Four decimal places is the
/// snapshot's precision; the tolerance sits above it.
fn bakes_rounding(ctx: &Ctx<'_>, id: NodeId, patch: &GeometryPatch) -> bool {
    const NEAR: f64 = 1e-3;
    let GeometryPatch::Path { path: written, .. } = patch else {
        return false;
    };
    let Some(
        kind @ NodeKind::Path {
            path, corner_radii, ..
        },
    ) = ctx.res.used_kind(ctx.doc, id)
    else {
        return false;
    };
    if corner_radii.iter().all(|r| *r <= 0.0) {
        return false;
    }
    let points = |p: &kurbo::BezPath, radii: &[f64]| -> Vec<kurbo::Point> {
        crate::edit::pen_anchors(p, radii)
            .iter()
            .flat_map(|s| s.anchors.iter().map(|a| a.at))
            .collect()
    };
    let near =
        |list: &[kurbo::Point], p: kurbo::Point| list.iter().any(|q| (*q - p).hypot() < NEAR);
    let anchors = points(path, corner_radii);
    let drawn = crate::geometry::local_path(kind).unwrap_or_default();
    let fillets: Vec<kurbo::Point> = points(&drawn, &[])
        .into_iter()
        .filter(|p| !near(&anchors, *p))
        .collect();
    points(written, &[]).into_iter().any(|p| near(&fillets, p))
}

/// A path patch written without `corner_radii` keeps the rounded path's radii
/// while the outline has as many points, and is refused naming them when it does
/// not: the short forms read radii left out as none, which would clear them.
fn keep_radii(
    ctx: &Ctx<'_>,
    a: &Args<'_>,
    id: NodeId,
    patch: &mut GeometryPatch,
) -> Result<(), VerbError> {
    let GeometryPatch::Path {
        path: written,
        corner_radii,
        ..
    } = patch
    else {
        return Ok(());
    };
    let named = match a.get("geometry") {
        Some(Value::Object(g)) => g
            .get("Path")
            .and_then(Value::as_object)
            .is_some_and(|p| p.contains_key("corner_radii")),
        Some(Value::String(s)) => s.contains("corner_radii"),
        _ => false,
    };
    let Some(NodeKind::Path {
        path,
        corner_radii: own,
        ..
    }) = ctx.doc.get(id).map(|n| n.kind())
    else {
        return Ok(());
    };
    if named || own.iter().all(|r| *r <= 0.0) {
        return Ok(());
    }
    let count = |p: &kurbo::BezPath| -> usize {
        crate::edit::pen_anchors(p, &[])
            .iter()
            .map(|s| s.anchors.len())
            .sum()
    };
    if count(written) != count(path) {
        return Err(a.invalid(
            "geometry",
            "the path's corners are rounded and the outline has another number of points; give `corner_radii`, one per point, to say which keep their rounding",
        ));
    }
    *corner_radii = own.clone();
    Ok(())
}

/// A style's value as a caller writes it: the lists as the single-layer verbs take
/// them, with no item ids, which the verb keys.
#[derive(serde::Deserialize, Debug)]
#[serde(rename_all = "lowercase", deny_unknown_fields)]
pub enum StyleInput {
    Fills(Vec<Fill>),
    Strokes(Vec<Stroke>),
    Effects(Vec<Effect>),
    Text(Box<TextStyle>),
    Layout(Box<crate::style::LayoutStyle>),
    /// A chart's options, as `set_chart` writes them whole.
    Chart(Box<crate::chart::Options>),
}

/// `input` as a style holds it, each list position keeping the item id `own`'s
/// list holds there (fresh ids past it, or for a new style), a text style
/// with its colour and language cleared (`style::type_part`), and a chart
/// style with its options' own fields at their defaults (`Options::look`).
fn style_value(ctx: &mut Ctx<'_>, input: StyleInput, own: Option<&StyleValue>) -> StyleValue {
    match (input, own) {
        (StyleInput::Fills(v), Some(StyleValue::Fills(o))) => StyleValue::Fills(rekeyed(ctx, o, v)),
        (StyleInput::Fills(v), _) => StyleValue::Fills(rekeyed(ctx, &[], v)),
        (StyleInput::Strokes(v), Some(StyleValue::Strokes(o))) => {
            StyleValue::Strokes(rekeyed(ctx, o, v))
        }
        (StyleInput::Strokes(v), _) => StyleValue::Strokes(rekeyed(ctx, &[], v)),
        (StyleInput::Effects(v), Some(StyleValue::Effects(o))) => {
            StyleValue::Effects(rekeyed(ctx, o, v))
        }
        (StyleInput::Effects(v), _) => StyleValue::Effects(rekeyed(ctx, &[], v)),
        (StyleInput::Text(t), _) => StyleValue::Text(Box::new(crate::style::type_part(&t))),
        (StyleInput::Layout(l), _) => StyleValue::Layout(l),
        (StyleInput::Chart(o), _) => StyleValue::Chart(Box::new(o.look())),
    }
}

/// The style the `style` argument names, the document's own.
fn own_style<'d>(ctx: &Ctx<'d>, a: &Args<'_>) -> Result<&'d crate::style::Style, VerbError> {
    let id: StyleId = a.id("style")?;
    let st = ctx
        .doc
        .styles()
        .get(id)
        .ok_or_else(|| a.invalid("style", "no style has that id"))?;
    if st.origin.is_some() {
        return Err(a.invalid("style", "a kit's style is read-only here"));
    }
    Ok(st)
}

/// The stored script the `script` argument names.
fn own_script(ctx: &Ctx<'_>, a: &Args<'_>) -> Result<crate::script::ScriptId, VerbError> {
    let id: crate::script::ScriptId = a.id("script")?;
    ctx.doc
        .script(id)
        .map(|s| s.id)
        .ok_or_else(|| a.invalid("script", "no script has that id"))
}

/// The `name` argument trimmed, refused when nothing is left.
fn script_name(a: &Args<'_>) -> Result<String, VerbError> {
    let name = a.text("name").map(str::trim).unwrap_or_default();
    if name.is_empty() {
        return Err(a.invalid("name", "a script needs a name"));
    }
    Ok(name.to_owned())
}

/// The variable the `variable` argument names, the document's own: a kit's is
/// written by an update only.
fn own_variable(ctx: &Ctx<'_>, a: &Args<'_>) -> Result<VarId, VerbError> {
    let id: VarId = a.id("variable")?;
    match ctx.doc.variables().get(id) {
        None => Err(a.invalid("variable", "no variable has that id")),
        Some(v) if v.origin.is_some() => {
            Err(a.invalid("variable", "a kit's variable is read-only here"))
        }
        Some(_) => Ok(id),
    }
}

/// The mode the `mode` argument names, the document's own: a kit's is written by
/// an update only.
fn own_mode(ctx: &Ctx<'_>, a: &Args<'_>) -> Result<ModeId, VerbError> {
    let mode: ModeId = a.id("mode")?;
    let table = ctx.doc.variables();
    if table.kit_mode_of(mode).is_some() {
        return Err(a.invalid("mode", "a kit's mode is read-only here"));
    }
    match table.mode(mode) {
        None => Err(a.invalid("mode", "no mode has that id")),
        Some(_) => Ok(mode),
    }
}

/// The collection the `collection` argument names, or `None` when it is not
/// given; refused when no collection has that id.
fn collection_arg(ctx: &Ctx<'_>, a: &Args<'_>) -> Result<Option<CollectionId>, VerbError> {
    match a.id_opt::<CollectionId>("collection")? {
        Some(c) if ctx.doc.variables().collection(c).is_none() => {
            Err(a.invalid("collection", "no collection has that id"))
        }
        c => Ok(c),
    }
}

/// The collection the required `collection` argument names.
fn collection_req(ctx: &Ctx<'_>, a: &Args<'_>) -> Result<CollectionId, VerbError> {
    collection_arg(ctx, a)?.ok_or(VerbError::Missing {
        verb: a.verb.name,
        param: "collection",
    })
}

/// `name` trimmed as a collection's, refused when empty or another collection
/// than `own` has it.
fn collection_name(
    ctx: &Ctx<'_>,
    a: &Args<'_>,
    name: &str,
    own: Option<CollectionId>,
) -> Result<String, VerbError> {
    let name = name.trim();
    if name.is_empty() {
        return Err(a.invalid("name", "a collection needs a name"));
    }
    let taken = ctx
        .doc
        .variables()
        .collections()
        .iter()
        .any(|c| Some(c.id) != own && c.name == name);
    match taken {
        true => Err(a.invalid("name", format!("{name} already exists"))),
        false => Ok(name.to_owned()),
    }
}

/// Why layer `id`'s element and link are not its own to set, as the identity
/// card's Element row dims: a layer inside an instance takes its main's, and a
/// shape takes none.
fn element_refusal(ctx: &Ctx<'_>, a: &Args<'_>, id: NodeId) -> Result<(), VerbError> {
    let n = node(ctx, id)?;
    let main = n
        .link()
        .and_then(|_| crate::component::instance_root(ctx.doc, id))
        .and_then(|r| crate::component::main_of(ctx.doc, r))
        .and_then(|m| ctx.doc.get(m));
    if let Some(m) = main {
        let why = format!("set on the main component, “{}”", m.name());
        return Err(a.invalid("node", why));
    }
    match crate::element::kind_refusal(n.kind()) {
        Some(why) => Err(a.invalid("node", why)),
        None => Ok(()),
    }
}

/// The main component or set the `node` argument names, which alone carry a
/// description and a documentation link of their own.
fn component_owner(ctx: &Ctx<'_>, a: &Args<'_>) -> Result<NodeId, VerbError> {
    let id = a.layer(ctx.doc, "node")?;
    let n = node(ctx, id)?;
    match n.component() || n.set().is_some() {
        true => Ok(id),
        false => Err(a.invalid("node", "neither a main component nor a component set")),
    }
}

/// What a variant verb says for a property name that is empty or taken.
const PROP_NAME_TAKEN: &str = "a property needs a name no other property of the set or main has";
/// What a variant verb says for a value name that is empty or taken.
const VALUE_NAME_TAKEN: &str = "a value needs a name no other value of the property has";

/// The layers the `nodes` argument names, each checked to exist: a bulk paint
/// write drops a missing id, which would otherwise answer as done.
fn existing_layers(ctx: &Ctx<'_>, a: &Args<'_>) -> Result<Vec<NodeId>, VerbError> {
    let ids = a.layers(ctx.doc, "nodes")?;
    for id in &ids {
        node(ctx, *id)?;
    }
    Ok(ids)
}

/// `tx` as the verb's edit, refused as `why` when it writes nothing, so an agent
/// is never told of a write that did not land.
fn nothing_if_empty(tx: Transaction, why: &'static str) -> Result<Done, VerbError> {
    match tx.0.is_empty() {
        true => Err(VerbError::Nothing(why)),
        false => Ok(Done::edit(tx)),
    }
}

/// The layers the `nodes` argument names, refused when none of them reaches a
/// layer that takes paint (`build::paint_targets`): an empty group, or slices,
/// would otherwise answer as done with nothing painted.
fn painted(ctx: &Ctx<'_>, a: &Args<'_>) -> Result<Vec<NodeId>, VerbError> {
    let ids = existing_layers(ctx, a)?;
    match build::paint_targets(ctx.doc, &ids).is_empty() {
        true => Err(a.invalid(
            "nodes",
            "none of them takes paint: a group passes paint to the shapes inside it, and holds none",
        )),
        false => Ok(ids),
    }
}

/// The text layers the `nodes` argument names; any other layer is refused.
fn texts(ctx: &Ctx<'_>, a: &Args<'_>) -> Result<Vec<NodeId>, VerbError> {
    let ids = existing_layers(ctx, a)?;
    match ids.iter().find(|id| {
        !matches!(
            ctx.doc.get(**id).map(|n| n.kind()),
            Some(NodeKind::Text { .. })
        )
    }) {
        Some(id) => Err(a.invalid("nodes", format!("`{}` is not a text layer", id.to_wire()))),
        None => Ok(ids),
    }
}

/// The component set the `node` argument names: the set, or one of its variants.
fn set_arg(ctx: &Ctx<'_>, a: &Args<'_>) -> Result<NodeId, VerbError> {
    let id = a.layer(ctx.doc, "node")?;
    node(ctx, id)?;
    match crate::variant::is_set(ctx.doc, id) {
        true => Ok(id),
        false => crate::variant::set_of(ctx.doc, id)
            .ok_or_else(|| a.invalid("node", "neither a component set nor a variant")),
    }
}

/// The variant property of `set` the `property` argument names, and its place.
fn variant_prop<'d>(
    ctx: &Ctx<'d>,
    a: &Args<'_>,
    set: NodeId,
) -> Result<(usize, &'d crate::variant::VariantProp), VerbError> {
    let vs = node(ctx, set)?
        .set()
        .ok_or_else(|| a.invalid("node", "not in a component set"))?;
    let name = a.text("property").unwrap_or_default();
    vs.props
        .iter()
        .enumerate()
        .find(|(_, p)| p.name == name)
        .ok_or_else(|| {
            let names: Vec<&str> = vs.props.iter().map(|p| p.name.as_str()).collect();
            a.invalid(
                "property",
                format!("the set's properties are {}", names.join(", ")),
            )
        })
}

/// The place of the value of `p` the `value` argument names.
fn variant_value(a: &Args<'_>, p: &crate::variant::VariantProp) -> Result<usize, VerbError> {
    let name = a.text("value").unwrap_or_default();
    p.values.iter().position(|v| v == name).ok_or_else(|| {
        a.invalid(
            "value",
            format!("{}'s values are {}", p.name, p.values.join(", ")),
        )
    })
}

/// The owner of the component property the `property` argument names — the
/// main or set the `node` argument names, or a variant's set — and the property.
/// A showing of nested properties is nameless, and never named here.
fn component_prop(
    ctx: &Ctx<'_>,
    a: &Args<'_>,
) -> Result<(NodeId, Keyed<crate::variant::Property>), VerbError> {
    let id = a.layer(ctx.doc, "node")?;
    node(ctx, id)?;
    let owner = crate::variant::owner_of_main(ctx.doc, id)
        .ok_or_else(|| a.invalid("node", "neither a main component nor a component set"))?;
    let props = node(ctx, owner)?.props();
    let name = a.text("property").unwrap_or_default();
    let named = |p: &&Keyed<crate::variant::Property>| {
        p.kind != crate::variant::PropKind::Nested && !p.name.is_empty()
    };
    props
        .iter()
        .filter(named)
        .find(|p| p.name == name)
        .map(|p| (owner, p.clone()))
        .ok_or_else(|| {
            let names: Vec<&crate::variant::Property> =
                props.iter().filter(named).map(|p| &**p).collect();
            a.invalid(
                "property",
                match names.as_slice() {
                    [] => "it has no component properties".to_owned(),
                    _ => format!("its properties are {}", listed_props(names)),
                },
            )
        })
}

/// What `make_component` says for a rule it refused by.
fn not_main(why: crate::component::NotMain) -> &'static str {
    use crate::component::NotMain;
    match why {
        NotMain::Kind => "only a frame or a group can become a component",
        NotMain::Already => "already a component",
        NotMain::Inside => "a component cannot sit inside a component or an instance",
        NotMain::Instance => "an instance cannot become a component; detach it first",
        NotMain::Holds => "a component cannot hold another component or a component set",
        NotMain::SetKind => {
            "a component set holds one kind of layer; its variants are another kind"
        }
        NotMain::List => "a list can't be part of a component; detach it first with `detach_list`",
    }
}

/// The text layer the `node` argument names, its styling and its text.
fn text_of<'d>(
    ctx: &Ctx<'d>,
    a: &Args<'_>,
) -> Result<(build::TextStyling<'d>, &'d str), VerbError> {
    build::TextStyling::of(node(ctx, a.node("node")?)?)
        .ok_or_else(|| a.invalid("node", "not a text layer"))
}

/// The byte range of `content` the `start` and `end` arguments name in characters,
/// and whether it is all of it.
fn char_range(a: &Args<'_>, content: &str) -> Result<(std::ops::Range<usize>, bool), VerbError> {
    let chars = content.chars().count();
    let (start, end) = (
        a.index("start").unwrap_or(0),
        a.index("end").unwrap_or(chars),
    );
    if start > end || end > chars {
        let why = format!("the text has {chars} characters");
        return Err(a.invalid(if end > chars { "end" } else { "start" }, why));
    }
    let byte = |n: usize| {
        content
            .char_indices()
            .nth(n)
            .map_or(content.len(), |(b, _)| b)
    };
    Ok((byte(start)..byte(end), start == 0 && end == chars))
}

/// `spans`, written in characters as the snapshot reads runs, as the byte ranges
/// of `content` the model keeps; one past the text is refused.
fn spans_in_bytes(
    a: &Args<'_>,
    name: &'static str,
    content: &str,
    spans: CharSpans,
    style: &TextStyle,
) -> Result<CharSpans, VerbError> {
    let chars = content.chars().count();
    let byte = |n: usize| {
        content
            .char_indices()
            .nth(n)
            .map_or(content.len(), |(b, _)| b)
    };
    let mut out = Vec::new();
    for s in spans.as_slice() {
        if s.start > s.end || s.end > chars {
            let why = format!("a run ends past the text, which has {chars} characters");
            return Err(a.invalid(name, why));
        }
        out.push(crate::typography::Span {
            start: byte(s.start),
            end: byte(s.end),
            attr: s.attr.clone(),
        });
    }
    Ok(CharSpans::from_parts(out, style))
}

/// A page's `name` trimmed, as the pages list keeps it; an empty one is refused.
fn page_name<'n>(a: &Args<'_>, name: &'n str) -> Result<&'n str, VerbError> {
    match name.trim() {
        "" => Err(a.invalid("name", "a page's name is not empty")),
        t => Ok(t),
    }
}

/// One op per layer of the `nodes` argument, built by `op`; a page is refused.
fn each(
    doc: &Document,
    args: &Args<'_>,
    op: impl Fn(NodeId) -> Operation,
) -> Result<Done, VerbError> {
    Ok(Done::ops(
        args.layers(doc, "nodes")?.into_iter().map(op).collect(),
    ))
}

/// A paint list written whole, each position keeping the item id the node's own
/// list holds there.
fn rekeyed<T>(ctx: &mut Ctx<'_>, own: &[Keyed<T>], values: Vec<T>) -> Vec<Keyed<T>> {
    rekey_by_position(own, values, &mut PastTheEnd::default(), ctx.ids)
}

/// Every verb, in the order a caller is shown them.
pub static VERBS: &[Verb] = &[
    // --- structure ---------------------------------------------------------
    Verb {
        name: "create_node",
        doc: "Create a layer of `kind` in `parent`, on top unless `index` says where, with no fills",
        params: &[
            req("parent", NODE, "the frame, group or page to create it in"),
            req(
                "kind",
                ParamKind::Model("NodeKind"),
                "what the layer is, with its geometry",
            ),
            opt(
                "index",
                INDEX,
                "its place among the parent's children, 0 the bottom",
            ),
            opt(
                "transform",
                ParamKind::Model("Affine"),
                "its placement in the parent",
            ),
            opt("x", ParamKind::Number, "its box's left in the parent"),
            opt("y", ParamKind::Number, "its box's top in the parent"),
            opt("name", TEXT, "its name"),
        ],
        run: |ctx, a| {
            let parent = a.node("parent")?;
            let count = node(ctx, parent)?.children().len();
            let id = ctx.ids.mint();
            let mut tx = Transaction(vec![Operation::CreateNode {
                id,
                parent,
                index: a.index("index").unwrap_or(count).min(count),
                kind: a.model::<NodeKind>("kind")?,
                transform: a.model_opt::<Affine>("transform")?,
                name: a.text("name").map(str::to_owned),
            }]);
            place_corner(ctx.doc, a, &mut tx, id);
            Ok(Done::made(tx, vec![id]))
        },
    },
    Verb {
        name: "create_text",
        doc: "Create a text layer in `parent`, its box sized to the text; answers it",
        params: &[
            req("parent", NODE, "the frame, group or page to create it in"),
            req(
                "text",
                TEXT,
                "what it says; a line break starts a paragraph",
            ),
            opt(
                "x",
                ParamKind::Number,
                "its box's left edge in the parent; 0 unless given",
            ),
            opt(
                "y",
                ParamKind::Number,
                "its box's top edge in the parent; 0 unless given",
            ),
            opt(
                "width",
                ParamKind::Number,
                "the width a longer line wraps at; none unless given",
            ),
            opt(
                "index",
                INDEX,
                "its place among the parent's children, 0 the bottom",
            ),
            opt("name", TEXT, "its name"),
        ],
        run: |ctx, a| {
            let parent = a.node("parent")?;
            let count = node(ctx, parent)?.children().len();
            let wrap = a.number("width");
            if wrap.is_some_and(|w| w <= 0.0) {
                return Err(a.invalid("width", "a width is more than 0"));
            }
            let (kind, _, offset) = crate::edit::text_layer_kind(
                a.text("text").unwrap_or_default(),
                crate::TextParts::default(),
                wrap,
            );
            let at = kurbo::Vec2::new(a.number("x").unwrap_or(0.0), a.number("y").unwrap_or(0.0));
            let id = ctx.ids.mint();
            let tx = Transaction(vec![Operation::CreateNode {
                id,
                parent,
                index: a.index("index").unwrap_or(count).min(count),
                kind,
                transform: Some(Affine::translate(at - offset)),
                name: a.text("name").map(str::to_owned),
            }]);
            Ok(Done::made(tx, vec![id]))
        },
    },
    Verb {
        name: "delete_node",
        doc: "Delete layers, with their frames' guides; instances of a deleted main are detached",
        params: &[req("nodes", NODES, "the layers to delete")],
        run: |ctx, a| {
            let ids = a.nodes("nodes")?;
            // A missing layer is refused here; every layer that exists is
            // deleted, so the edit is never empty.
            for id in &ids {
                node(ctx, *id)?;
            }
            Ok(Done::edit(build::delete(ctx.doc, &ids)))
        },
    },
    Verb {
        name: "duplicate",
        doc: "Copy layers, each copy one step above its original; answers the copies",
        params: &[
            req("nodes", NODES, "the layers to copy"),
            opt(
                "dx",
                ParamKind::Number,
                "how far right to move the copies, 0 unless given",
            ),
            opt(
                "dy",
                ParamKind::Number,
                "how far down to move the copies, 0 unless given",
            ),
        ],
        run: |ctx, a| {
            let offset =
                kurbo::Vec2::new(a.number("dx").unwrap_or(0.0), a.number("dy").unwrap_or(0.0));
            let nodes = a.nodes("nodes")?;
            let (tx, created) = build::duplicate(ctx.doc, ctx.ids, &nodes, offset);
            if created.is_empty() {
                return Err(VerbError::Nothing("none of the layers can be copied"));
            }
            // A main's copy is an instance; one in a set is no variant, which
            // agents expected.
            let main = nodes
                .iter()
                .any(|n| ctx.doc.get(*n).is_some_and(|n| n.component()));
            let done = Done::made(tx, created);
            Ok(match main {
                true => done.saying(
                    "a main's copy is an instance of it: `add_variant` copies a variant as a new \
                     one, and `duplicate_as_component` copies a main as a main"
                        .into(),
                ),
                false => done,
            })
        },
    },
    Verb {
        name: "insert_subtree",
        doc: "Create a whole tree of layers in `parent` in one edit; answers its layers, root first. Copy a layer's shape from `get_tree`",
        params: &[
            req("parent", NODE, "the frame, group or page to create it in"),
            req(
                "tree",
                ParamKind::Model("Tree"),
                "the root layer, its properties and its `children`, bottom first; a child is placed by `transform` [1, 0, 0, 1, x, y]; a `copy_of` layer takes what it leaves out, `kind` and `children` too, from its main, so write only overrides; a fill binds as {\"brush\": {\"var\": <id>}}, a style as \"styles\": {\"text\": <id>}; a layer's `like` names one whose fills, strokes, effects, opacity and blend it takes where it gives none, a text also a text's style and paragraph",
            ),
            opt(
                "index",
                INDEX,
                "the root's place among the parent's children, 0 the bottom",
            ),
            opt(
                "x",
                ParamKind::Number,
                "the root's box's left in the parent",
            ),
            opt("y", ParamKind::Number, "the root's box's top in the parent"),
        ],
        run: |ctx, a| {
            let parent = a.node("parent")?;
            let count = node(ctx, parent)?.children().len();
            let index = a.index("index").unwrap_or(count).min(count);
            let mut raw = a.get("tree").cloned().ok_or(VerbError::Missing {
                verb: a.verb.name,
                param: "tree",
            })?;
            if let Value::String(s) = &raw
                && let Ok(parsed) = serde_json::from_str::<Value>(s)
            {
                raw = parsed;
            }
            expand_like(ctx.doc, &mut raw).map_err(|why| a.invalid("tree", why))?;
            expand_copies(ctx.doc, &mut raw);
            loose::loosen("tree", &mut raw);
            let raw = settle::<Tree>("Tree", raw).map_err(|why| a.invalid("tree", why))?;
            let tree: Tree = serde_json::from_value(raw).map_err(|e| a.invalid("tree", e))?;
            let mut done = Done::default();
            let mut minted = Minted::default();
            mint_tree(ctx.ids, &tree, &mut done.created, &mut minted);
            let created = done.created.clone();
            let mut late = Late::new();
            let mut ids = created.iter();
            tree_ops(
                ctx, a, &tree, parent, index, &mut ids, &minted, &mut done, &mut late,
            )?;
            list_fills_onto_new_props(ctx.doc, &minted, &mut late);
            done.tx.0.extend(late);
            if let Some(&root) = done.created.first() {
                place_corner(ctx.doc, a, &mut done.tx, root);
            }
            Ok(done)
        },
    },
    Verb {
        name: "import_svg",
        doc: "Read SVG markup into layers, in one new group in `parent`; answers the group",
        params: &[
            req("parent", NODE, "the frame, group or page to place it in"),
            req("svg", TEXT, "the markup, one `<svg>` element"),
            opt(
                "x",
                ParamKind::Number,
                "how far right of the parent's origin it goes, 0 unless given",
            ),
            opt(
                "y",
                ParamKind::Number,
                "how far down from the parent's origin it goes, 0 unless given",
            ),
            opt("name", TEXT, "the group's name"),
            opt(
                "index",
                INDEX,
                "the group's place among the parent's children, 0 the bottom; on top unless given",
            ),
        ],
        run: |ctx, a| {
            let parent = a.node("parent")?;
            let count = node(ctx, parent)?.children().len();
            let index = a.index("index").unwrap_or(count).min(count);
            let svg = a.text("svg").unwrap_or_default();
            // Reborrowed to the one lifetime `import` asks of both; `as_deref_mut`
            // keeps the sink's own, and the borrow checker refuses the call.
            let images: Option<&mut dyn ImageSink> = match &mut ctx.images {
                Some(s) => Some(&mut **s),
                None => None,
            };
            let mut out = crate::svg_in::import(svg, &mut *ctx.ids, parent, index, images)
                .map_err(|e| a.invalid("svg", e))?;
            if out.shapes == 0 {
                return Err(VerbError::Nothing("the markup draws nothing"));
            }
            let root = out.root;
            let shift =
                Affine::translate((a.number("x").unwrap_or(0.0), a.number("y").unwrap_or(0.0)));
            if let Some(Operation::CreateNode {
                transform, name, ..
            }) = out
                .tx
                .0
                .iter_mut()
                .find(|op| matches!(op, Operation::CreateNode { id, .. } if *id == root))
            {
                *transform = Some(shift * transform.unwrap_or(Affine::IDENTITY));
                if let Some(given) = a.text("name") {
                    *name = Some(given.to_owned());
                }
            }
            // As the app's import says it: what is missing, then what is there
            // and slightly wrong.
            out.skipped.sort();
            out.approximated.sort();
            let mut said = Vec::new();
            if !out.skipped.is_empty() {
                said.push(format!("Left out: {}", out.skipped.join(", ")));
            }
            if !out.approximated.is_empty() {
                said.push(format!("Approximated: {}", out.approximated.join(", ")));
            }
            let done = Done::made(out.tx, vec![root]);
            Ok(match said.is_empty() {
                true => done,
                false => done.saying(said.join("; ")),
            })
        },
    },
    Verb {
        name: "reparent",
        doc: "Move a layer into another parent, keeping where it is drawn; answers `at`, its place there",
        params: &[
            req("node", NODE, "the layer to move"),
            req("parent", NODE, "its new parent"),
            opt(
                "index",
                INDEX,
                "its place there, 0 the bottom; on top unless given",
            ),
        ],
        run: |ctx, a| {
            let (id, parent) = (a.node("node")?, a.node("parent")?);
            let mut count = node(ctx, parent)?.children().len();
            // Within its own parent the layer's place is counted without it.
            if node(ctx, id)?.parent() == Some(parent) {
                count -= 1;
            }
            let index = a.index("index").unwrap_or(count).min(count);
            Ok(Done::edit(build::reparent_preserving_world(
                ctx.doc, ctx.res, id, parent, index,
            )?))
        },
    },
    Verb {
        name: "reorder",
        doc: "Move a layer to `index` among its siblings, 0 the bottom",
        params: &[
            req("node", NODE, "the layer to move"),
            req("index", INDEX, "its new place"),
        ],
        run: |_, a| {
            Ok(Done::ops(vec![Operation::Reorder {
                id: a.node("node")?,
                index: a.index("index").unwrap_or(0),
            }]))
        },
    },
    Verb {
        name: "arrange",
        doc: "Bring layers to the front or forward, or send them backward or to the back",
        params: &[
            req("nodes", NODES, "the layers to restack"),
            req("move", ParamKind::Choice(MOVES), "where they go"),
        ],
        run: |ctx, a| {
            let mv = match a.choice("move") {
                Some("front") => ZMove::Front,
                Some("forward") => ZMove::Forward,
                Some("backward") => ZMove::Backward,
                _ => ZMove::Back,
            };
            let ids = existing_layers(ctx, a)?;
            let already = match mv {
                ZMove::Front | ZMove::Forward => "already at the front",
                ZMove::Backward | ZMove::Back => "already at the back",
            };
            nothing_if_empty(build::z_order(ctx.doc, &ids, mv), already)
        },
    },
    Verb {
        name: "group",
        doc: "Group layers; answers the new group",
        params: &[req("nodes", NODES, "the layers to group")],
        run: |ctx, a| {
            let (tx, group) = build::group(ctx.doc, ctx.res, ctx.ids, &a.nodes("nodes")?)?;
            Ok(Done::made(tx, vec![group]))
        },
    },
    Verb {
        name: "frame",
        doc: "Wrap layers in a new frame sized to them, with no fill and no clip; answers the frame",
        params: &[req("nodes", NODES, "the layers to frame; one is enough")],
        run: |ctx, a| {
            let nodes = existing_layers(ctx, a)?;
            if !build::can_frame(ctx.doc, &nodes) {
                let parents: std::collections::HashSet<NodeId> = nodes
                    .iter()
                    .filter_map(|n| ctx.doc.get(*n)?.parent())
                    .collect();
                return Err(VerbError::Nothing(match parents.len() {
                    0 | 1 => "a frame cannot go inside a boolean or a mask",
                    _ => "the layers have different parents; frame each parent's layers apart",
                }));
            }
            let (tx, frame) = build::frame(ctx.doc, ctx.res, ctx.ids, &nodes)?;
            Ok(Done::made(tx, vec![frame]))
        },
    },
    Verb {
        name: "ungroup",
        doc: "Lift each group's or boolean's children into its parent, where they are drawn",
        params: &[req("nodes", NODES, "the groups to ungroup")],
        run: |ctx, a| {
            Ok(Done::edit(build::ungroup_all(
                ctx.doc,
                ctx.res,
                &a.nodes("nodes")?,
            )?))
        },
    },
    Verb {
        name: "boolean",
        doc: "Combine shapes into a live boolean; answers the new layer",
        params: &[
            req("nodes", NODES, "the shapes, bottom first"),
            req("op", ParamKind::Choice(BOOL_OPS), "how they combine"),
        ],
        run: |ctx, a| {
            let op = match a.choice("op") {
                Some("subtract") => BoolOp::Subtract,
                Some("intersect") => BoolOp::Intersect,
                Some("exclude") => BoolOp::Exclude,
                _ => BoolOp::Union,
            };
            let (tx, id) = build::boolean(ctx.doc, ctx.res, ctx.ids, &a.nodes("nodes")?, op, None)?;
            Ok(Done::made(tx, vec![id]))
        },
    },
    Verb {
        name: "flatten",
        doc: "Bake layers into one path as drawn; answers the new path",
        params: &[req("nodes", NODES, "the layers to flatten")],
        run: |ctx, a| {
            let (tx, id) = build::flatten(ctx.doc, ctx.res, ctx.ids, &a.nodes("nodes")?)?;
            Ok(Done::made(tx, vec![id]))
        },
    },
    Verb {
        name: "outline_shape",
        doc: "Turn a shape into the path its outline traces, corners baked in; answers the path",
        params: &[req(
            "node",
            NODE,
            "a rectangle, ellipse, polygon, star, line or path",
        )],
        run: |ctx, a| {
            let (tx, id) = build::outline(ctx.doc, ctx.res, ctx.ids, a.layer(ctx.doc, "node")?)?;
            Ok(Done::made(tx, vec![id]))
        },
    },
    Verb {
        name: "outline_stroke",
        doc: "Turn a layer's visible strokes into filled paths; answers the layer to select",
        params: &[req(
            "node",
            NODE,
            "a shape, path or boolean with a visible stroke",
        )],
        run: |ctx, a| {
            let id = a.layer(ctx.doc, "node")?;
            if !build::can_outline_stroke(ctx.doc, id) {
                return Err(VerbError::Nothing("this layer has no stroke to outline"));
            }
            let (tx, made) = build::outline_stroke(ctx.doc, ctx.res, ctx.ids, id)?;
            Ok(Done::made(tx, vec![made]))
        },
    },
    Verb {
        name: "outline_text",
        doc: "Replace a text layer with the path its glyphs trace; the text is gone. Answers the path",
        params: &[req("node", NODE, "the text layer")],
        run: |ctx, a| {
            let (tx, id) =
                build::outline_text(ctx.doc, ctx.res, ctx.ids, a.layer(ctx.doc, "node")?)?;
            Ok(Done::made(tx, vec![id]))
        },
    },
    Verb {
        name: "divide",
        doc: "Cut overlapping shapes, and any lines across them, into one path per face, in a new group; answers the group",
        params: &[req("nodes", NODES, "the shapes and lines to cut")],
        run: |ctx, a| {
            use crate::shape_builder::{self, Refusal};
            let nodes = a.layers(ctx.doc, "nodes")?;
            let arr = shape_builder::arrange(ctx.doc, ctx.res, &nodes).map_err(|r| match r {
                Refusal::Abandoned => VerbError::Nothing("the shapes could not be cut"),
                Refusal::TooFew | Refusal::NoOverlap => VerbError::Nothing(
                    "divide needs overlapping shapes, or a shape and a line across it",
                ),
            })?;
            let (tx, group) = shape_builder::divide(ctx.doc, ctx.res, ctx.ids, &nodes, &arr)?;
            Ok(Done::made(tx, vec![group]))
        },
    },
    Verb {
        name: "blend_shapes",
        doc: "Put shapes between two, stepping from one to the other, in a new group; answers the group",
        params: &[
            req(
                "nodes",
                NODES,
                "two shapes, the first lowest; a third layer that is a path is followed as `spine` is",
            ),
            opt(
                "steps",
                INDEX,
                "how many shapes go between: 1 to 200, 5 unless given",
            ),
            opt(
                "paint",
                BOOL,
                "whether fills, strokes and opacity move from one to the other; true unless given",
            ),
            opt(
                "reverse",
                BOOL,
                "run the second shape's outline the other way round; false unless given",
            ),
            opt(
                "spine",
                NODE,
                "a path the steps follow, the first shape at its start and the second at its end",
            ),
            opt(
                "turn",
                BOOL,
                "turn each step with the spine's direction; false unless given",
            ),
        ],
        run: |ctx, a| {
            let steps = a.index("steps").unwrap_or(5);
            if !(1..=crate::blend::MAX_STEPS).contains(&steps) {
                return Err(a.invalid("steps", "expected 1 to 200"));
            }
            let nodes = a.layers(ctx.doc, "nodes")?;
            let spine = match a.get("spine") {
                Some(_) => Some(a.layer(ctx.doc, "spine")?),
                None => crate::blend::spine_in(ctx.doc, &nodes),
            };
            let blend = crate::blend::Blend {
                steps,
                paint: a.bool("paint").unwrap_or(true),
                reverse: a.bool("reverse").unwrap_or(false),
                spine,
                turn: a.bool("turn").unwrap_or(false),
                ..crate::blend::Blend::DEFAULT
            };
            let (tx, group) = crate::blend::blend(ctx.doc, ctx.res, ctx.ids, &nodes, blend)
                .map_err(|r| VerbError::Nothing(r.says()))?;
            Ok(Done::made(tx, vec![group]))
        },
    },
    Verb {
        name: "apply_brush",
        doc: "Bend a shape along a path (art) or repeat it along it (scatter), as paths in a group in the path's place; answers the group",
        params: &[
            req("path", NODE, "the path the brush runs along"),
            req("brush", NODE, "the shape bent or repeated; it stays"),
            opt(
                "mode",
                ParamKind::Choice(&["art", "scatter"]),
                "art unless given",
            ),
            opt(
                "width",
                ParamKind::Number,
                "art: height across the path, a share of the brush's, 0.01 to 10; 1 unless given",
            ),
            opt(
                "stretch",
                ParamKind::Choice(&["whole", "middle"]),
                "art: middle keeps the first and last fifth unstretched; whole unless given",
            ),
            opt("flip", BOOL, "art: mirror across the path"),
            opt(
                "spacing",
                ParamKind::Number,
                "scatter: copy to copy, a share of the brush's length, 0.1 to 10; 1.2 unless given",
            ),
            opt(
                "size",
                ParamKind::Number,
                "scatter: a share of the brush's size, 0.01 to 10; 0.8 unless given",
            ),
            opt(
                "size_varies",
                ParamKind::Number,
                "scatter: how far size strays either way, 0 to 1; 0.3 unless given",
            ),
            opt(
                "scatter",
                ParamKind::Number,
                "scatter: how far a copy strays sideways, a share of the brush's length, 0 to 5; 0.2 unless given",
            ),
            opt(
                "turn",
                ParamKind::Choice(&["along", "fixed"]),
                "scatter: along unless given",
            ),
            opt("seed", INDEX, "scatter: the random run; 1 unless given"),
            opt("keep_path", BOOL, "keep the path in the group, hidden"),
        ],
        run: |ctx, a| {
            use crate::apply_brush::{self as ab, ApplyBrush, Art, Mode, Scatter, Stretch, Turn};
            let ranged =
                |name: &'static str, default: f64, lo: f64, hi: f64, says: &'static str| {
                    let v = a.number(name).unwrap_or(default);
                    match v.is_finite() && (lo..=hi).contains(&v) {
                        true => Ok(v),
                        false => Err(a.invalid(name, says)),
                    }
                };
            let mode = match a.choice("mode").unwrap_or("art") {
                "scatter" => Mode::Scatter(Scatter {
                    spacing: ranged("spacing", 1.2, 0.1, 10.0, "expected 0.1 to 10")?,
                    size: ranged("size", 0.8, 0.01, 10.0, "expected 0.01 to 10")?,
                    size_varies: ranged("size_varies", 0.3, 0.0, 1.0, "expected 0 to 1")?,
                    scatter: ranged("scatter", 0.2, 0.0, 5.0, "expected 0 to 5")?,
                    turn: match a.choice("turn") {
                        Some("fixed") => Turn::Fixed,
                        _ => Turn::Along,
                    },
                    seed: u32::try_from(a.index("seed").unwrap_or(1))
                        .map_err(|_| a.invalid("seed", "expected 0 to 4294967295"))?,
                }),
                _ => Mode::Art(Art {
                    width: ranged("width", 1.0, 0.01, 10.0, "expected 0.01 to 10")?,
                    stretch: match a.choice("stretch") {
                        Some("middle") => Stretch::Middle,
                        _ => Stretch::Whole,
                    },
                    flip: a.bool("flip").unwrap_or(false),
                }),
            };
            let how = ApplyBrush {
                path: a.layer(ctx.doc, "path")?,
                brush: a.layer(ctx.doc, "brush")?,
                mode,
                keep_path: a.bool("keep_path").unwrap_or(false),
            };
            let (tx, group) = ab::apply(ctx.doc, ctx.res, ctx.ids, &how)
                .map_err(|r| VerbError::Nothing(r.says()))?;
            Ok(Done::made(tx, vec![group]))
        },
    },
    Verb {
        name: "warp",
        doc: "Bend layers through an envelope over their box and bake them into paths: a preset bent by `bend`, or the box's corners moved to `corners`; answers the paths that replaced layers",
        params: &[
            req(
                "nodes",
                NODES,
                "the layers; groups are opened, frames and pictures refused",
            ),
            opt(
                "preset",
                ParamKind::Choice(&[
                    "arc", "flag", "bulge", "wave", "rise", "squeeze", "fisheye", "twist",
                ]),
                "the envelope, set by `bend`",
            ),
            opt(
                "bend",
                ParamKind::Number,
                "how far the preset bends, -1 to 1; 0.5 unless given",
            ),
            opt(
                "vertical",
                BOOL,
                "bend along the box's height rather than its width",
            ),
            opt(
                "corners",
                ParamKind::Model("Quad"),
                "where the box's corners go, [[x, y] × 4] in world space, top-left first and clockwise; in place of `preset`",
            ),
        ],
        run: |ctx, a| {
            use crate::warp::{Envelope, Preset, Warp};
            let nodes = a.layers(ctx.doc, "nodes")?;
            let bounds = crate::warp::subjects(ctx.doc, &nodes)
                .and_then(|_| {
                    crate::warp::bounds(ctx.doc, ctx.res, &nodes)
                        .ok_or(crate::warp::Refusal::Nothing)
                })
                .map_err(|r| VerbError::Nothing(r.says()))?;
            let envelope = match (a.choice("preset"), a.get("corners")) {
                (Some(_), Some(_)) => {
                    return Err(a.invalid("corners", "give `preset` or `corners`, not both"));
                }
                (None, None) => {
                    return Err(a.invalid("preset", "give `preset` and `bend`, or `corners`"));
                }
                (None, Some(_)) => {
                    let quad: [[f64; 2]; 4] = a.model("corners")?;
                    if quad.iter().flatten().any(|n| !n.is_finite()) {
                        return Err(a.invalid("corners", "each corner is two numbers"));
                    }
                    Envelope::Free(quad.map(|[x, y]| kurbo::Point::new(x, y)))
                }
                (Some(word), None) => {
                    let bend = a.number("bend").unwrap_or(0.5);
                    if !(-1.0..=1.0).contains(&bend) {
                        return Err(a.invalid("bend", "expected -1 to 1"));
                    }
                    Envelope::Preset {
                        preset: match word {
                            "arc" => Preset::Arc,
                            "flag" => Preset::Flag,
                            "bulge" => Preset::Bulge,
                            "wave" => Preset::Wave,
                            "rise" => Preset::Rise,
                            "squeeze" => Preset::Squeeze,
                            "fisheye" => Preset::Fisheye,
                            _ => Preset::Twist,
                        },
                        bend,
                        vertical: a.bool("vertical").unwrap_or(false),
                    }
                }
            };
            let warp = Warp { bounds, envelope };
            if warp.folds() {
                return Err(VerbError::Nothing(
                    "The envelope turns over itself: bend it less, or keep the corners in their turn",
                ));
            }
            if warp.is_identity() {
                return Err(VerbError::Nothing("The envelope moves nothing"));
            }
            let (tx, _) = crate::warp::warp(ctx.doc, ctx.res, ctx.ids, &nodes, &warp)
                .map_err(|r| VerbError::Nothing(r.says()))?;
            // The paths standing in for the layers that were not paths.
            let replaced =
                tx.0.iter()
                    .filter_map(|op| match op {
                        Operation::CreateNode { id, .. } => Some(*id),
                        Operation::InsertSubtree { nodes, .. } => nodes.first().map(|n| n.id()),
                        _ => None,
                    })
                    .collect();
            Ok(Done::made(tx, replaced))
        },
    },
    Verb {
        name: "join_paths",
        doc: "Join two paths at their nearest open ends into the first; the second goes",
        params: &[
            req("node", NODE, "the path kept, with its paint and name"),
            req("other", NODE, "the path joined into it"),
        ],
        run: |ctx, a| {
            let (keep, other) = (a.node("node")?, a.node("other")?);
            if keep == other {
                return Err(a.invalid("other", "a path cannot join itself"));
            }
            crate::edit::join_paths(ctx.doc, ctx.res, keep, other)
                .map(Done::edit)
                .map_err(VerbError::Nothing)
        },
    },
    Verb {
        name: "repeat",
        doc: "Repeat layers into a new group, round a point (`count`), in a line or in a grid; answers the group, or in an auto layout the layers and copies",
        params: &[
            req("nodes", NODES, "the layers to repeat, sharing a parent"),
            opt("count", INDEX, "a ring of 2 to 360, originals counted"),
            opt(
                "line",
                ParamKind::Model("RepeatLine"),
                "count as a ring's; gap between boxes, 16 unless given",
            ),
            opt(
                "grid",
                ParamKind::Model("RepeatGrid"),
                "gap [across, down] between boxes",
            ),
            opt(
                "angle",
                ParamKind::Number,
                "a ring's step in degrees, clockwise; a full turn split evenly unless given",
            ),
            opt(
                "turn",
                BOOL,
                "whether a ring's copies turn with their step; true unless given",
            ),
            opt(
                "x",
                ParamKind::Number,
                "the ring's centre across; the layers' centre unless given",
            ),
            opt("y", ParamKind::Number, "the ring's centre down"),
            opt(
                "linked",
                BOOL,
                "make the one layer a component first, its copies instances",
            ),
        ],
        run: |ctx, a| {
            use crate::repeat::Pattern;
            let nodes = build::outermost(ctx.doc, &a.nodes("nodes")?);
            let given = [a.get("count"), a.get("line"), a.get("grid")];
            if given.iter().flatten().count() != 1 {
                return Err(a.invalid("count", "give one of `count`, `line` or `grid`"));
            }
            let pattern = if a.get("line").is_some() {
                let line: crate::repeat::Line = a.model("line")?;
                if !(2..=crate::repeat::MAX_COUNT).contains(&line.count) || !line.gap.is_finite() {
                    return Err(a.invalid("line", "expected a count of 2 to 360 and a finite gap"));
                }
                Pattern::Line(line)
            } else if a.get("grid").is_some() {
                let grid: crate::repeat::Grid = a.model("grid")?;
                let pattern = Pattern::Grid(grid);
                if !pattern.is_valid() || grid.gap.iter().any(|g| !g.is_finite()) {
                    return Err(
                        a.invalid("grid", "expected cols × rows of 2 to 360 and finite gaps")
                    );
                }
                pattern
            } else {
                let count = a.index("count").unwrap_or(0);
                if !(2..=crate::repeat::MAX_COUNT).contains(&count) {
                    return Err(a.invalid("count", "expected 2 to 360"));
                }
                let mut ring = crate::repeat::Ring::full(count, a.bool("turn").unwrap_or(true));
                if let Some(deg) = a.number("angle") {
                    ring.step = deg.to_radians();
                }
                let centre = crate::repeat::centre(ctx.res, &nodes)
                    .ok_or(VerbError::Nothing("the layers have no bounds"))?;
                let pivot = kurbo::Point::new(
                    a.number("x").unwrap_or(centre.x),
                    a.number("y").unwrap_or(centre.y),
                );
                Pattern::Ring { ring, pivot }
            };
            if let Some(why) = crate::repeat::layout_refusal(ctx.doc, &nodes, pattern.way()) {
                return Err(VerbError::Nothing(why));
            }
            let (tx, picked) = if a.bool("linked").unwrap_or(false) {
                use crate::repeat::NotLinked;
                match crate::repeat::why_not_linked(ctx.doc, &nodes) {
                    Some(NotLinked::Several) => {
                        return Err(a.invalid("linked", "linked copies repeat one layer at a time"));
                    }
                    Some(NotLinked::Linked) => {
                        return Err(a.invalid("linked", "copies of a component are linked already"));
                    }
                    Some(NotLinked::Not(why)) => return Err(a.invalid("linked", not_main(why))),
                    None => {}
                }
                crate::repeat::linked(ctx.doc, ctx.res, ctx.ids, nodes[0], pattern)?
            } else {
                crate::repeat::repeat(
                    ctx.doc,
                    ctx.res,
                    ctx.ids,
                    &nodes,
                    pattern,
                    crate::repeat::Place::Group,
                )?
            };
            Ok(Done::made(tx, picked))
        },
    },
    Verb {
        name: "align",
        doc: "Line layers up on one edge or centre of the box round them all",
        params: &[
            req("nodes", NODES, "two or more layers"),
            req("edge", ParamKind::Choice(EDGES), "which edge or centre"),
        ],
        run: |ctx, a| {
            let ids = build::outermost(ctx.doc, &a.nodes("nodes")?);
            if ids.len() < 2 {
                return Err(VerbError::Nothing("align needs two or more layers"));
            }
            let target = ids
                .iter()
                .filter_map(|id| ctx.res.world_bounds(*id))
                .reduce(|a, b| a.union(b))
                .ok_or(VerbError::Nothing("the layers have no bounds"))?;
            let (axis, edge) = match a.choice("edge") {
                Some("left") => (Axis::X, Edge::Min),
                Some("center") => (Axis::X, Edge::Mid),
                Some("right") => (Axis::X, Edge::Max),
                Some("top") => (Axis::Y, Edge::Min),
                Some("middle") => (Axis::Y, Edge::Mid),
                _ => (Axis::Y, Edge::Max),
            };
            Ok(Done::edit(build::align(
                ctx.doc, ctx.res, &ids, target, axis, edge,
            )))
        },
    },
    Verb {
        name: "distribute",
        doc: "Space three or more layers evenly between the outer two",
        params: &[
            req("nodes", NODES, "three or more layers"),
            req("axis", ParamKind::Choice(AXES), "across or down"),
        ],
        run: |ctx, a| {
            let axis = match a.choice("axis") {
                Some("horizontal") => Axis::X,
                _ => Axis::Y,
            };
            let ids = existing_layers(ctx, a)?;
            if ids.len() < 3 {
                return Err(VerbError::Nothing("distribute needs three or more layers"));
            }
            nothing_if_empty(
                build::distribute(ctx.doc, ctx.res, &ids, axis),
                "already evenly spaced",
            )
        },
    },
    Verb {
        name: "tidy_up",
        doc: "Lay layers out as the grid they roughly sit in, on even gaps",
        params: &[req("nodes", NODES, "two or more layers")],
        run: |ctx, a| {
            let nodes = a.layers(ctx.doc, "nodes")?;
            if nodes.len() < 2 {
                return Err(VerbError::Nothing("tidy up needs two or more layers"));
            }
            let tx = build::tidy_up(ctx.doc, ctx.res, &nodes);
            if tx.0.is_empty() {
                return Err(VerbError::Nothing("already tidy"));
            }
            Ok(Done::edit(tx))
        },
    },
    Verb {
        name: "slice",
        doc: "Add a slice fitted to layers' bounds, exporting PNG at 1×, just above the topmost; answers the slice",
        params: &[req("nodes", NODES, "the layers to slice round")],
        run: |ctx, a| {
            let nodes = a.layers(ctx.doc, "nodes")?;
            let all_slices = nodes.iter().all(|i| {
                matches!(
                    ctx.doc.get(*i).map(|n| n.kind()),
                    Some(NodeKind::Slice { .. })
                )
            });
            if all_slices {
                return Err(VerbError::Nothing("a slice on a slice cuts nothing new"));
            }
            let id = ctx.ids.mint();
            let tx = build::slice_selection(ctx.doc, ctx.res, &nodes, id)
                .ok_or(VerbError::Nothing("the layers have no bounds"))?;
            Ok(Done::made(tx, vec![id]))
        },
    },
    // --- components -----------------------------------------------------------
    Verb {
        name: "make_component",
        doc: "Make a frame or group a main component, or wrap layers in a frame made one; answers the main",
        params: &[req(
            "nodes",
            NODES,
            "one frame or group, or the layers to wrap",
        )],
        run: |ctx, a| match build::make_component(ctx.doc, ctx.res, ctx.ids, &a.nodes("nodes")?) {
            Ok((tx, main)) => Ok(Done::made(tx, vec![main])),
            Err(build::MakeError::Nothing) => {
                Err(VerbError::Nothing("no layers to make a component of"))
            }
            Err(build::MakeError::Not(why)) => Err(VerbError::Nothing(not_main(why))),
            Err(build::MakeError::Frame(e)) => Err(e.into()),
        },
    },
    Verb {
        name: "create_instance",
        doc: "Place an instance of a main component, or of a set's default variant, in `parent`; answers it",
        params: &[
            req("main", NODE, "the main component or set"),
            req("parent", NODE, "the frame, group or page to place it in"),
            opt(
                "x",
                ParamKind::Number,
                "its box's left edge in the parent; where the main's is unless given",
            ),
            opt(
                "y",
                ParamKind::Number,
                "its box's top edge in the parent; where the main's is unless given",
            ),
            opt("index", INDEX, "its place, 0 the bottom"),
        ],
        run: |ctx, a| {
            let (main, parent) = (a.node("main")?, a.node("parent")?);
            let m = node(ctx, main)?;
            if !m.component() && m.set().is_none() {
                return Err(a.invalid("main", "not a main component or a set"));
            }
            let count = node(ctx, parent)?.children().len();
            let (mut tx, id) = build::place_instance(ctx.doc, ctx.ids, main, parent, None)
                .ok_or(VerbError::Nothing("the set has no variant to place"))?;
            if let Some(at) = a.index("index") {
                for op in &mut tx.0 {
                    if let Operation::InsertSubtree { nodes, index, .. } = op
                        && nodes.first().is_some_and(|n| n.id == id)
                    {
                        *index = at.min(count);
                    }
                }
            }
            place_corner(ctx.doc, a, &mut tx, id);
            Ok(Done::made(tx, vec![id]))
        },
    },
    Verb {
        name: "detach_instance",
        doc: "Turn the instances layers sit in into ordinary layers that keep their look",
        params: &[req("nodes", NODES, "instances, or layers inside them")],
        run: |ctx, a| {
            let mut roots: Vec<NodeId> = a
                .nodes("nodes")?
                .iter()
                .filter_map(|id| crate::component::instance_root(ctx.doc, *id))
                .collect();
            roots.sort();
            roots.dedup();
            let ops: Vec<Operation> = build::outermost(ctx.doc, &roots)
                .iter()
                .filter_map(|r| crate::component::detach(ctx.doc, *r))
                .flat_map(|tx| tx.0)
                .collect();
            if ops.is_empty() {
                return Err(VerbError::Nothing("none of the layers is in an instance"));
            }
            Ok(Done::ops(ops))
        },
    },
    Verb {
        name: "swap_instance",
        doc: "Show another main in an instance, its overrides carried by name",
        params: &[
            req("node", NODE, "the instance, or a nested copy inside one"),
            req("main", NODE, "the main to show, of the same kind"),
        ],
        run: |ctx, a| {
            let (copy, main) = (a.node("node")?, a.node("main")?);
            // The main it shows already relinks nothing, as the instance card's pick.
            if crate::component::main_of(ctx.doc, copy) == Some(main) {
                return Ok(Done::default());
            }
            // An instance linked straight to a main relinks; a nested copy swaps,
            // carrying its variant values by name, as the instance card does.
            let tx = if crate::swap::can_relink(ctx.doc, copy) {
                crate::swap::can_relink_to(ctx.doc, copy, main)
                    .then(|| crate::variant::switch(ctx.doc, copy, main, ctx.ids))
                    .flatten()
            } else {
                crate::variant::swap_carrying(ctx.doc, copy, main)
            };
            tx.map(Done::edit).ok_or(VerbError::Nothing(
                "that instance cannot show that main: it must be a main of the same kind",
            ))
        },
    },
    Verb {
        name: "combine_as_variants",
        doc: "Combine main components of one kind into a component set; answers the set",
        params: &[req(
            "nodes",
            NODES,
            "the mains, sharing a parent and in no set",
        )],
        run: |ctx, a| {
            let mains = a.nodes("nodes")?;
            if !crate::variant::one_kind(ctx.doc, &mains) {
                return Err(VerbError::Nothing(
                    "a component set holds one kind of layer: these mains mix frames and groups",
                ));
            }
            let (tx, set) = crate::variant::combine(ctx.doc, ctx.res, ctx.ids, &mains)?;
            Ok(Done::made(tx, vec![set]))
        },
    },
    Verb {
        name: "switch_variant",
        doc: "Show another variant in an instance by one property's value",
        params: &[
            req("node", NODE, "the instance of a variant"),
            req("property", TEXT, "the variant property, by name"),
            req("value", TEXT, "its value to show"),
        ],
        run: |ctx, a| {
            let root = a.node("node")?;
            let main = crate::component::main_of(ctx.doc, root)
                .ok_or_else(|| a.invalid("node", "not an instance"))?;
            let set = crate::variant::set_of(ctx.doc, main)
                .and_then(|s| ctx.doc.get(s)?.set())
                .ok_or_else(|| a.invalid("node", "its main is in no component set"))?;
            let name = a.text("property").unwrap_or_default();
            let prop = set
                .props
                .iter()
                .position(|p| p.name == name)
                .ok_or_else(|| {
                    let names: Vec<&str> = set.props.iter().map(|p| p.name.as_str()).collect();
                    a.invalid(
                        "property",
                        format!("the set's properties are {}", names.join(", ")),
                    )
                })?;
            let value = a.text("value").unwrap_or_default();
            let to = crate::variant::switch_target(ctx.doc, main, prop, value)
                .ok_or_else(|| a.invalid("value", "no variant of the set has it"))?;
            if to == main {
                return Ok(Done::default());
            }
            crate::variant::switch(ctx.doc, root, to, ctx.ids)
                .map(Done::edit)
                .ok_or(VerbError::Nothing("that instance cannot switch variants"))
        },
    },
    Verb {
        name: "define_property",
        doc: "Bind a layer inside a main to a component property, a new one unless `property` names one; answers a new one's id",
        params: &[
            req("node", NODE, "the layer inside a main"),
            req(
                "kind",
                ParamKind::Choice(PROP_KINDS),
                "what it drives: the layer's visibility, text or picture, or a nested instance's main",
            ),
            opt(
                "property",
                TEXT,
                "an existing property of that kind to bind to, by name",
            ),
            opt(
                "name",
                TEXT,
                "a new property's name; named for the layer unless given",
            ),
        ],
        run: |ctx, a| {
            use crate::variant::{Bind, PropKind};
            let node_id = a.node("node")?;
            let n = node(ctx, node_id)?;
            let owner = crate::variant::owner_above(ctx.doc, node_id)
                .ok_or_else(|| a.invalid("node", "not a layer inside a main component"))?;
            let shows = n
                .link
                .filter(|l| ctx.doc.get(*l).is_some_and(|m| m.component));
            let kind = match a.choice("kind") {
                Some("text") if matches!(n.kind(), NodeKind::Text { .. }) => PropKind::Text,
                Some("swap") if shows.is_some() => PropKind::Swap,
                Some("image") if crate::variant::image_of(n).is_some() => PropKind::Image,
                Some("boolean") => PropKind::Boolean,
                _ => {
                    return Err(a.invalid(
                        "kind",
                        "a text property needs a text layer, a swap one a nested instance, an image one a layer with an image fill",
                    ));
                }
            };
            let props = ctx
                .doc
                .get(owner)
                .map(|o| o.props().to_vec())
                .unwrap_or_default();
            let (pick, item) = match a.text("property") {
                Some(name) => {
                    let p = props
                        .iter()
                        .find(|p| p.kind == kind && p.name == name)
                        .ok_or_else(|| {
                            a.invalid(
                                "property",
                                "the main has no property of that kind by that name",
                            )
                        })?;
                    (Bind::To(p.id), None)
                }
                None => {
                    let item = ctx.ids.mint_item();
                    (Bind::New(item), Some(item))
                }
            };
            let filter = shows
                .and_then(|m| ctx.doc.get(m))
                .map(|m| crate::swap::suggested_filter(m.name()))
                .unwrap_or_default();
            let mut next = crate::variant::rebind(&props, kind, node_id, pick, n.name(), &filter);
            if let (Some(item), Some(name)) = (item, a.text("name").map(str::trim)) {
                let reserved = ctx
                    .doc
                    .get(owner)
                    .and_then(|o| o.set())
                    .is_some_and(|s| s.props.iter().any(|p| p.name == name));
                if name.is_empty()
                    || reserved
                    || next.iter().any(|p| p.id != item && p.name == name)
                {
                    return Err(a.invalid(
                        "name",
                        "a property needs a name no other property of the main has",
                    ));
                }
                if let Some(p) = next.iter_mut().find(|p| p.id == item) {
                    p.value.name = name.to_owned();
                }
            }
            // The name `set_instance_property` takes it by, which `rebind` chose.
            let named = item
                .and_then(|item| next.iter().find(|p| p.id == item))
                .map(|p| format!("named \"{}\"", p.value.name));
            let ops = vec![Operation::SetProperties {
                id: owner,
                props: next,
            }];
            Ok(match item {
                Some(item) => Done {
                    said: named,
                    ..Done::making(ops, item.0.to_wire())
                },
                None => Done::ops(ops),
            })
        },
    },
    Verb {
        name: "set_instance_property",
        doc: "Set one of an instance's component properties: a visibility, a text, or the main a slot shows",
        params: &[
            req("node", NODE, "the instance"),
            req("property", TEXT, "the property, by name"),
            req(
                "value",
                ParamKind::Model("PropValue"),
                "true or false, a text, or a main's id, as the property's kind takes",
            ),
        ],
        run: |ctx, a| {
            use crate::variant::{PropKind, PropValue};
            let root = crate::component::instance_root(ctx.doc, a.node("node")?)
                .ok_or_else(|| a.invalid("node", "not in an instance"))?;
            let name = a.text("property").unwrap_or_default();
            let props = crate::variant::instance_properties(ctx.doc, root);
            let p = props
                .iter()
                .find(|p| p.name == name && p.kind != PropKind::Nested)
                .ok_or_else(|| {
                    let names: Vec<&crate::variant::Property> = props
                        .iter()
                        .map(|p| &**p)
                        .filter(|p| !p.name.is_empty() && p.kind != PropKind::Nested)
                        .collect();
                    a.invalid(
                        "property",
                        format!("its properties are {}", listed_props(names)),
                    )
                })?;
            let v = a.get("value").cloned().unwrap_or(Value::Null);
            let value = match (p.kind, &v) {
                (PropKind::Boolean, Value::Bool(b)) => PropValue::Boolean(*b),
                (PropKind::Text, Value::String(s)) => PropValue::Text(s.clone()),
                (PropKind::Swap, Value::String(_)) => {
                    let main = wire(&v).map_err(|why| a.invalid("value", why))?;
                    if !ctx.doc.get(main).is_some_and(|m| m.component) {
                        return Err(a.invalid("value", "not a main component"));
                    }
                    let slots = crate::variant::counterparts(ctx.doc, root, p);
                    if slots
                        .iter()
                        .any(|c| !crate::swap::can_swap_to(ctx.doc, *c, main))
                    {
                        return Err(a.invalid(
                            "value",
                            "the slot cannot show that main: it must be a main of the same kind, not one holding the slot",
                        ));
                    }
                    PropValue::Swap(main)
                }
                // A picture by its id in the document.
                (PropKind::Image, Value::String(s)) => {
                    let id = crate::ImageId(s.clone());
                    if ctx.doc.image(&id).is_none() {
                        return Err(a.invalid("value", "no picture of that id in the document"));
                    }
                    PropValue::Image(id)
                }
                _ => return Err(a.invalid("value", "not a value of the property's kind")),
            };
            Ok(Done::ops(crate::variant::set_property_detaching(
                ctx.doc,
                &[root],
                p,
                &value,
            )))
        },
    },
    Verb {
        name: "reset_overrides",
        doc: "Put instances, or layers inside them, back as their main has them",
        params: &[
            req("nodes", NODES, "instances, or layers inside them"),
            opt(
                "scope",
                ParamKind::Choice(RESETS),
                "everything (`all`, unless given), the fields alone, removed children restored, or the order",
            ),
        ],
        run: |ctx, a| {
            let scopes: Vec<NodeId> = build::outermost(ctx.doc, &a.nodes("nodes")?)
                .into_iter()
                .filter(|id| crate::component::instance_root(ctx.doc, *id).is_some())
                .collect();
            if scopes.is_empty() {
                return Err(VerbError::Nothing("none of the layers is in an instance"));
            }
            let kind = match a.choice("scope") {
                Some("fields") => crate::reset::Kind::Fields,
                Some("children") => crate::reset::Kind::Children,
                Some("order") => crate::reset::Kind::Order,
                _ => crate::reset::Kind::All,
            };
            nothing_if_empty(
                Transaction(crate::reset::stated(ctx.doc, kind, &scopes, ctx.ids)),
                "nothing here differs from the main component",
            )
        },
    },
    Verb {
        name: "push_to_main",
        doc: "Write an instance's changes into its main, so every instance of it takes them",
        params: &[
            req("node", NODE, "the instance, or a layer inside it"),
            opt(
                "scope",
                ParamKind::Choice(PUSHES),
                "properties, fields, local layers and order (`all`, unless given), or one of them, or the main's layers this instance removed",
            ),
        ],
        run: |ctx, a| {
            use crate::push::{self, Blocked, Kind};
            let id = a.layer(ctx.doc, "node")?;
            let root = crate::component::instance_root(ctx.doc, id)
                .ok_or(VerbError::Nothing("the layer is in no instance"))?;
            match push::blocked(ctx.doc, root) {
                Some(Blocked::Kit) => {
                    return Err(VerbError::Nothing(
                        "its main comes from a kit — change it in the kit",
                    ));
                }
                Some(Blocked::Locked) => {
                    return Err(VerbError::Nothing("its main is locked"));
                }
                None => {}
            }
            let kind = match a.choice("scope") {
                Some("properties") => Kind::Properties,
                Some("fields") => Kind::Fields,
                Some("layers") => Kind::Layers,
                Some("order") => Kind::Order,
                Some("removals") => Kind::Removals,
                _ => Kind::All,
            };
            let ops = push::push(ctx.doc, kind, root, ctx.ids);
            if ops.is_empty() {
                return Err(VerbError::Nothing("nothing to push"));
            }
            Ok(Done::ops(ops))
        },
    },
    Verb {
        name: "add_variant",
        doc: "Add a variant to a set: a copy of one variant taking the first free combination of values; answers it",
        params: &[req(
            "node",
            NODE,
            "the set, copying its last variant, or the variant to copy",
        )],
        run: |ctx, a| {
            use crate::variant;
            let id = a.layer(ctx.doc, "node")?;
            let (set, from) = match variant::is_set(ctx.doc, id) {
                true => (id, None),
                false => match variant::set_of(ctx.doc, id) {
                    Some(set) => (set, Some(id)),
                    None => return Err(a.invalid("node", "neither a set nor a variant")),
                },
            };
            let (tx, new) = variant::add_variant(ctx.doc, ctx.res, ctx.ids, set, from)
                .ok_or(VerbError::Nothing("this set has no variant to copy"))?;
            Ok(Done::made(tx, vec![new]))
        },
    },
    Verb {
        name: "set_variant_value",
        doc: "Set one of a variant's values, as the variant card's dropdowns do; a combination another variant holds shows as a clash",
        params: &[
            req("node", NODE, "the variant, a main in a set"),
            req("property", TEXT, "the variant property, by name"),
            req("value", TEXT, "one of the property's values"),
        ],
        run: |ctx, a| {
            let id = a.layer(ctx.doc, "node")?;
            let set = crate::variant::set_of(ctx.doc, id)
                .ok_or_else(|| a.invalid("node", "not a variant of a component set"))?;
            let (prop, p) = variant_prop(ctx, a, set)?;
            let value = &p.values[variant_value(a, p)?];
            let mut values = node(ctx, id)?.variant().to_vec();
            if values.get(prop) == Some(value) {
                return Ok(Done::default());
            }
            if let Some(v) = values.get_mut(prop) {
                v.clone_from(value);
            }
            crate::variant::set_values(ctx.doc, id, values)
                .map(Done::edit)
                .ok_or(VerbError::Nothing(
                    "the variant's values do not fit its set",
                ))
        },
    },
    Verb {
        name: "rename_variant_property",
        doc: "Rename a component set's variant property",
        params: &[
            req("node", NODE, "the set, or one of its variants"),
            req("property", TEXT, "the variant property, by name"),
            req("name", TEXT, "its new name"),
        ],
        run: |ctx, a| {
            let set = set_arg(ctx, a)?;
            let (prop, _) = variant_prop(ctx, a, set)?;
            let name = a.text("name").unwrap_or_default();
            crate::variant::rename_property(ctx.doc, set, prop, name)
                .map(Done::edit)
                .ok_or_else(|| a.invalid("name", PROP_NAME_TAKEN))
        },
    },
    Verb {
        name: "rename_variant_value",
        doc: "Rename a value of a variant property, in the set and in every variant holding it",
        params: &[
            req("node", NODE, "the set, or one of its variants"),
            req("property", TEXT, "the variant property, by name"),
            req("value", TEXT, "the value, by name"),
            req("name", TEXT, "its new name"),
        ],
        run: |ctx, a| {
            let set = set_arg(ctx, a)?;
            let (prop, p) = variant_prop(ctx, a, set)?;
            let value = variant_value(a, p)?;
            let name = a.text("name").unwrap_or_default();
            crate::variant::rename_value(ctx.doc, set, prop, value, name)
                .map(Done::edit)
                .ok_or_else(|| a.invalid("name", VALUE_NAME_TAKEN))
        },
    },
    Verb {
        name: "add_variant_property",
        doc: "Add a variant property to a component set, every variant taking its one value",
        params: &[
            req("node", NODE, "the set, or one of its variants"),
            opt(
                "name",
                TEXT,
                "its name; the first free `Property N` unless given",
            ),
            opt("value", TEXT, "its one value; `Default` unless given"),
        ],
        run: |ctx, a| {
            let set = set_arg(ctx, a)?;
            let vs = node(ctx, set)?.set().cloned().unwrap_or_default();
            let name = match a.text("name") {
                Some(n) => n.to_owned(),
                None => crate::variant::next_property_name(&vs),
            };
            let value = a.text("value").unwrap_or("Default");
            if value.trim().is_empty() {
                return Err(a.invalid("value", "a value needs a name"));
            }
            crate::variant::add_property(ctx.doc, set, &name, value)
                .map(Done::edit)
                .ok_or_else(|| a.invalid("name", PROP_NAME_TAKEN))
        },
    },
    Verb {
        name: "add_variant_value",
        doc: "Add a value to a variant property; makes no variant",
        params: &[
            req("node", NODE, "the set, or one of its variants"),
            req("property", TEXT, "the variant property, by name"),
            opt(
                "value",
                TEXT,
                "its name; the first free `Value N` unless given",
            ),
        ],
        run: |ctx, a| {
            let set = set_arg(ctx, a)?;
            let (prop, _) = variant_prop(ctx, a, set)?;
            let value = match a.text("value") {
                Some(v) => v.to_owned(),
                None => {
                    let vs = node(ctx, set)?.set().cloned().unwrap_or_default();
                    crate::variant::next_value_name(&vs, prop)
                }
            };
            crate::variant::add_value(ctx.doc, set, prop, &value)
                .map(|tx| {
                    Done::edit(tx).saying(format!(
                        "no variant holds “{value}” yet: `add_variant` copies one, and \
                         `set_variant_value` gives the copy this value"
                    ))
                })
                .ok_or_else(|| a.invalid("value", VALUE_NAME_TAKEN))
        },
    },
    Verb {
        name: "delete_variant_property",
        doc: "Delete a variant property from a component set and from every variant",
        params: &[
            req("node", NODE, "the set, or one of its variants"),
            req("property", TEXT, "the variant property, by name"),
        ],
        run: |ctx, a| {
            let set = set_arg(ctx, a)?;
            let (prop, _) = variant_prop(ctx, a, set)?;
            crate::variant::delete_property(ctx.doc, set, prop)
                .map(Done::edit)
                .ok_or(VerbError::Nothing("a set needs at least one property"))
        },
    },
    Verb {
        name: "move_variant_value",
        doc: "Move a value of a variant property to another place in its list",
        params: &[
            req("node", NODE, "the set, or one of its variants"),
            req("property", TEXT, "the variant property, by name"),
            req("value", TEXT, "the value, by name"),
            req("index", INDEX, "its new place, counted from 0"),
        ],
        run: |ctx, a| {
            let set = set_arg(ctx, a)?;
            let (prop, p) = variant_prop(ctx, a, set)?;
            let from = variant_value(a, p)?;
            let to = a.index("index").unwrap_or(0);
            if to >= p.values.len() {
                let why = format!(
                    "the property has {}",
                    plural(p.values.len(), "value", "values")
                );
                return Err(a.invalid("index", why));
            }
            if to == from {
                return Ok(Done::default());
            }
            crate::variant::move_value(ctx.doc, set, prop, from, to)
                .map(Done::edit)
                .ok_or(VerbError::Nothing("that value cannot move there"))
        },
    },
    Verb {
        name: "delete_variant_value",
        doc: "Delete a value of a variant property, and the variants holding it; their instances detach",
        params: &[
            req("node", NODE, "the set, or one of its variants"),
            req("property", TEXT, "the variant property, by name"),
            req("value", TEXT, "the value, by name"),
        ],
        run: |ctx, a| {
            let set = set_arg(ctx, a)?;
            let (prop, p) = variant_prop(ctx, a, set)?;
            let value = variant_value(a, p)?;
            let gone = crate::variant::using_value(ctx.doc, set, prop, value).len();
            let tx = crate::variant::delete_value(ctx.doc, set, prop, value)
                .ok_or(VerbError::Nothing("a property needs at least one value"))?;
            let done = Done::edit(tx);
            Ok(match gone {
                0 => done,
                n => done.saying(format!("deleted {}", plural(n, "variant", "variants"))),
            })
        },
    },
    Verb {
        name: "tie_state",
        doc: "Tie a value of a variant property to a browser state, making it the set's interaction state, or untie it",
        params: &[
            req("node", NODE, "the set, or one of its variants"),
            req("property", TEXT, "the variant property, by name"),
            req("value", TEXT, "the value, by name"),
            opt(
                "state",
                ParamKind::Choice(STATES),
                "the state it stands for; absent unties it",
            ),
        ],
        run: |ctx, a| {
            let set = set_arg(ctx, a)?;
            let (prop, p) = variant_prop(ctx, a, set)?;
            let value = variant_value(a, p)?;
            let state = a
                .choice("state")
                .map(|w| serde_json::from_value(Value::from(w)))
                .transpose()
                .map_err(|e| a.invalid("state", e))?;
            if p.state_of(&p.values[value]) == state {
                return Ok(Done::default());
            }
            if state.is_some()
                && let Some(why) = crate::variant::why_not_tie(ctx.doc, set, prop, value)
            {
                return Err(a.invalid("value", why));
            }
            crate::variant::tie_state(ctx.doc, set, prop, value, state)
                .map(Done::edit)
                .ok_or_else(|| a.invalid("state", "another value is tied to that state"))
        },
    },
    Verb {
        name: "show_nested_properties",
        doc: "Show a nested instance's own properties on every instance of the main holding it, or stop",
        params: &[
            req("node", NODE, "the instance nested inside a main"),
            req("shown", BOOL, "whether its properties show"),
        ],
        run: |ctx, a| {
            let id = a.layer(ctx.doc, "node")?;
            let nested = node(ctx, id)?
                .link()
                .and_then(|l| ctx.doc.get(l))
                .is_some_and(|l| l.component());
            if !nested || crate::variant::owner_above(ctx.doc, id).is_none() {
                return Err(a.invalid("node", "not an instance nested inside a main"));
            }
            let on = a.bool("shown").unwrap_or(true);
            Ok(crate::variant::set_shown(ctx.doc, ctx.ids, id, on)
                .map(Done::edit)
                .unwrap_or_default())
        },
    },
    Verb {
        name: "edit_property",
        doc: "Rename a component property, or change a swap property's filter and preferred mains",
        params: &[
            req("node", NODE, "the main or set owning it, or a variant"),
            req("property", TEXT, "the property, by name"),
            opt("name", TEXT, "its new name"),
            opt(
                "filter",
                TEXT,
                "a swap property's filter: the start of the names of the mains its picker offers; empty offers every main",
            ),
            opt(
                "preferred",
                ParamKind::Model("[Node]"),
                "a swap property's preferred mains, offered first, in order",
            ),
        ],
        run: |ctx, a| {
            use crate::variant::{PropKind, Property};
            let (owner, p) = component_prop(ctx, a)?;
            let name = match a.text("name").map(str::trim) {
                Some(n) => {
                    let o = node(ctx, owner)?;
                    let taken = o.props().iter().any(|q| q.id != p.id && q.name == n)
                        || o.set().is_some_and(|s| s.props.iter().any(|v| v.name == n));
                    if n.is_empty() || taken {
                        return Err(a.invalid("name", PROP_NAME_TAKEN));
                    }
                    n.to_owned()
                }
                None => p.name.clone(),
            };
            let swap = |param: &'static str| match p.kind {
                PropKind::Swap => Ok(()),
                _ => Err(a.invalid(param, "only a swap property has one")),
            };
            let filter = match a.text("filter") {
                Some(f) => {
                    swap("filter")?;
                    f.trim().to_owned()
                }
                None => p.filter.clone(),
            };
            let preferred = match a.model_opt::<Vec<String>>("preferred")? {
                Some(list) => {
                    swap("preferred")?;
                    // What the picker offers the slot, as the card's own picker of
                    // preferred mains lists it; each once.
                    let slot = p.bound.first().copied().ok_or_else(|| {
                        a.invalid(
                            "preferred",
                            "the property is bound to no nested instance yet",
                        )
                    })?;
                    let offered = crate::swap::options(ctx.doc, slot, "", "");
                    let mut mains = Vec::new();
                    for s in &list {
                        let m = NodeId::from_wire(s)
                            .filter(|m| offered.contains(m))
                            .ok_or_else(|| {
                                a.invalid(
                                    "preferred",
                                    format!("`{s}` is not a main the slot can show"),
                                )
                            })?;
                        if !mains.contains(&m) {
                            mains.push(m);
                        }
                    }
                    mains
                }
                None => p.preferred.clone(),
            };
            let next = Property {
                name,
                filter,
                preferred,
                ..p.value.clone()
            };
            if next == p.value {
                return Ok(Done::default());
            }
            crate::variant::edit_property(ctx.doc, owner, p.id, |_| Some(next))
                .map(Done::edit)
                .ok_or(VerbError::Nothing("that property is gone"))
        },
    },
    Verb {
        name: "delete_property",
        doc: "Delete a component property; the layers bound to it keep their values",
        params: &[
            req("node", NODE, "the main or set owning it, or a variant"),
            req("property", TEXT, "the property, by name"),
        ],
        run: |ctx, a| {
            let (owner, p) = component_prop(ctx, a)?;
            crate::variant::edit_property(ctx.doc, owner, p.id, |_| None)
                .map(Done::edit)
                .ok_or(VerbError::Nothing("that property is gone"))
        },
    },
    Verb {
        name: "duplicate_as_component",
        doc: "Copy a main, or a layer holding mains, as new mains, not instances; answers it",
        params: &[
            req("node", NODE, "a main or a layer with one"),
            opt(
                "dx",
                ParamKind::Number,
                "how far right to move the copy, 0 unless given",
            ),
            opt(
                "dy",
                ParamKind::Number,
                "how far down to move the copy, 0 unless given",
            ),
        ],
        run: |ctx, a| {
            let id = a.layer(ctx.doc, "node")?;
            if !node(ctx, id)?.component() && !crate::component::holds_mains(ctx.doc, id) {
                return Err(a.invalid("node", "neither a main component nor a layer holding one"));
            }
            let (Some(nodes), Some(parent)) =
                (ctx.doc.capture_subtree(id), node(ctx, id)?.parent())
            else {
                return Err(VerbError::Nothing("this component cannot be copied"));
            };
            let index = node(ctx, parent)?
                .children()
                .iter()
                .position(|c| *c == id)
                .map(|i| i + 1);
            let offset =
                kurbo::Vec2::new(a.number("dx").unwrap_or(0.0), a.number("dy").unwrap_or(0.0));
            let (tx, created) = build::insert_subtrees_as(
                ctx.doc,
                ctx.ids,
                &[build::Placement {
                    nodes,
                    parent,
                    index,
                }],
                offset,
                crate::component::MainCopy::NewMain,
            );
            Ok(Done::made(tx, created.into_iter().take(1).collect()))
        },
    },
    // --- variables and modes ----------------------------------------------------
    Verb {
        name: "add_variable",
        doc: "Add a variable holding one value in every mode; answers its id",
        params: &[
            req("kind", ParamKind::Choice(VAR_KINDS), "what it holds"),
            opt(
                "name",
                TEXT,
                "its name, `/` between groups; the first free `kind N` unless given",
            ),
            opt(
                "value",
                ParamKind::Model("Cell"),
                "what it holds; the kind's default unless given",
            ),
            opt(
                "collection",
                ParamKind::Id("collection"),
                "its collection; the first unless given",
            ),
        ],
        run: |ctx, a| {
            let kind: VarKind =
                serde_json::from_value(Value::from(a.choice("kind").unwrap_or("color")))
                    .map_err(|e| a.invalid("kind", e))?;
            let table = ctx.doc.variables();
            let c = collection_arg(ctx, a)?.unwrap_or(table.first_collection().id);
            let value = a.model_opt::<Cell>("value")?;
            let (ops, id) =
                crate::variable::add_variable_ops(table, ctx.ids, c, kind, a.text("name"), value)
                    .map_err(|why| {
                    // A taken name: the variable already there may be the one wanted.
                    let held = a.text("name").and_then(|n| {
                        table
                            .of_collection(c)
                            .find(|v| v.name == n)
                            .map(|v| v.id.0.to_wire())
                    });
                    match held {
                        Some(id) => a.invalid("name", format!("{why} Its id is `{id}`.")),
                        None => a.invalid("name", why),
                    }
                })?;
            Ok(Done::making(ops, id.0.to_wire()))
        },
    },
    Verb {
        name: "generate_scale",
        doc: "Make `name/50`…`name/950` from a colour variable's default-mode value, even in OKLCH lightness; a name taken by a colour variable is updated, the source never",
        params: &[
            req("variable", ParamKind::Id("variable"), "the colour variable"),
            opt(
                "name",
                TEXT,
                "the group, its name less a numeric step unless given",
            ),
            opt("steps", ParamKind::Index, "3–11; 11 unless given"),
            opt(
                "anchor",
                ParamKind::Index,
                "the step it lands on: its name's, else by lightness",
            ),
            opt(
                "exact",
                ParamKind::Bool,
                "whether that step is the colour itself; true unless given",
            ),
        ],
        run: |ctx, a| {
            let table = ctx.doc.variables();
            let id = own_variable(ctx, a)?;
            let (c, _, base) = crate::variable::scale_source(table, id)
                .ok_or_else(|| a.invalid("variable", "not a colour variable"))?;
            let steps = crate::oklch::SCALE_STEPS;
            let count = a.index("steps").unwrap_or(steps.len());
            if !(crate::oklch::SCALE_MIN_STEPS..=steps.len()).contains(&count) {
                return Err(a.invalid("steps", "3 to 11"));
            }
            let keep = crate::oklch::scale_subset(count);
            let name = table.get(id).map(|v| v.name.as_str()).unwrap_or_default();
            let stem = a
                .text("name")
                .unwrap_or_else(|| crate::variable::scale_stem(name))
                .to_owned();
            let anchor = match a.index("anchor") {
                None => crate::oklch::nearest_kept(
                    &keep,
                    crate::variable::scale_anchor(name, &stem, base),
                ),
                Some(s) => keep
                    .iter()
                    .copied()
                    .find(|i| usize::from(steps[*i]) == s)
                    .ok_or_else(|| {
                        let kept: Vec<String> =
                            keep.iter().map(|i| steps[*i].to_string()).collect();
                        a.invalid("anchor", format!("one of {}", kept.join(", ")))
                    })?,
            };
            // The source on a step of its own scale keeps its colour there.
            if let Some(at) = crate::variable::scale_pinned(name, &stem, &keep) {
                let why = format!(
                    "{name} is this scale's {}, which keeps its colour; leave it out",
                    steps[at]
                );
                if anchor != at {
                    return Err(a.invalid("anchor", why));
                }
                if a.bool("exact") == Some(false) {
                    return Err(a.invalid("exact", why));
                }
            }
            let exact = a.bool("exact").unwrap_or(true);
            let ramp = crate::oklch::scale_kept(base, anchor, exact, &keep);
            let (ops, made, updated) =
                crate::variable::scale_ops(table, ctx.ids, c, &stem, &ramp, None, Some(id))
                    .map_err(|why| a.invalid("name", why))?;
            let span = (steps[keep[0]], steps[keep[keep.len() - 1]]);
            let said = crate::variable::scale_said(&stem, made, &updated, span);
            Ok(Done::ops(ops).saying(said))
        },
    },
    Verb {
        name: "remove_variable",
        doc: "Remove a variable; its users keep the value they drew, and aliases of it resolve",
        params: &[req("variable", ParamKind::Id("variable"), "the variable")],
        run: |ctx, a| {
            let id = own_variable(ctx, a)?;
            Ok(Done::ops(crate::variable::remove_ops(ctx.doc, &[id])))
        },
    },
    Verb {
        name: "rename_variable",
        doc: "Rename a variable",
        params: &[
            req("variable", ParamKind::Id("variable"), "the variable"),
            req("name", TEXT, "its new name, `/` between groups"),
        ],
        run: |ctx, a| {
            let id = own_variable(ctx, a)?;
            crate::variable::rename_variable_ops(
                ctx.doc.variables(),
                id,
                a.text("name").unwrap_or_default(),
            )
            .map(Done::ops)
            .map_err(|why| a.invalid("name", why))
        },
    },
    Verb {
        name: "set_variable_value",
        doc: "Set what a variable holds in one mode: a value, or an alias of another variable",
        params: &[
            req("variable", ParamKind::Id("variable"), "the variable"),
            opt(
                "mode",
                ParamKind::Id("mode"),
                "the mode, of the variable's collection; its only one when it has one",
            ),
            req(
                "value",
                ParamKind::Model("Cell"),
                "the value, `{ \"alias\": <variable id> }`, or a boolean's opposite `{ \"not\": <variable id> }`",
            ),
        ],
        run: |ctx, a| {
            let id = own_variable(ctx, a)?;
            let only = || match ctx.doc.variables().collection_of(id).map(|c| &c.modes[..]) {
                Some([m]) => Ok(m.id),
                _ => Err(a.invalid(
                    "mode",
                    "the variable's collection has more than one mode: name one",
                )),
            };
            let mode: ModeId = match a.id_opt("mode")? {
                Some(mode) => mode,
                None => only()?,
            };
            crate::variable::set_cell_ops(ctx.doc.variables(), id, mode, a.model("value")?)
                .map(Done::ops)
                .map_err(|why| a.invalid("value", why))
        },
    },
    Verb {
        name: "add_mode",
        doc: "Add a mode to a collection, a copy of its first; answers its id",
        params: &[
            opt(
                "name",
                TEXT,
                "its name; the first free `Mode N` unless given",
            ),
            opt(
                "collection",
                ParamKind::Id("collection"),
                "the collection; the first unless given",
            ),
        ],
        run: |ctx, a| {
            let c = collection_arg(ctx, a)?.unwrap_or(ctx.doc.variables().first_collection().id);
            let (ops, id) = crate::variable::add_mode_ops(ctx.doc, ctx.ids, c, a.text("name"))
                .map_err(|why| a.invalid("name", why))?;
            Ok(Done::making(ops, id.0.to_wire()))
        },
    },
    Verb {
        name: "remove_mode",
        doc: "Remove a mode; frames that picked it inherit their collection's again",
        params: &[req("mode", ParamKind::Id("mode"), "the mode")],
        run: |ctx, a| {
            let mode = own_mode(ctx, a)?;
            Ok(Done::ops(crate::variable::remove_mode_ops(ctx.doc, mode)))
        },
    },
    Verb {
        name: "rename_mode",
        doc: "Rename a mode",
        params: &[
            req("mode", ParamKind::Id("mode"), "the mode"),
            req("name", TEXT, "its new name, unique in its collection"),
        ],
        run: |ctx, a| {
            let mode = own_mode(ctx, a)?;
            let name = a.text("name").unwrap_or_default().trim();
            if let Some(why) = crate::variable::mode_name_refusal(ctx.doc.variables(), mode, name) {
                return Err(a.invalid("name", why));
            }
            Ok(Done::ops(crate::variable::rename_mode_ops(
                ctx.doc, mode, name,
            )))
        },
    },
    Verb {
        name: "set_frame_mode",
        doc: "Pick which mode of a collection a frame or page shows, or let it inherit",
        params: &[
            req("node", NODE, "the frame or page"),
            opt("mode", ParamKind::Id("mode"), "the mode; absent inherits"),
            opt(
                "collection",
                ParamKind::Id("collection"),
                "the collection; the mode's, else the first",
            ),
        ],
        run: |ctx, a| {
            let table = ctx.doc.variables();
            let mode = a.id_opt::<ModeId>("mode")?;
            let c = match (collection_arg(ctx, a)?, mode) {
                (Some(c), _) => c,
                (None, Some(m)) => {
                    table
                        .collection_of_mode(m)
                        .ok_or_else(|| a.invalid("mode", "no mode has that id"))?
                        .id
                }
                (None, None) => table.first_collection().id,
            };
            crate::variable::frame_mode_ops(ctx.doc, a.node("node")?, c, mode)
                .map(Done::ops)
                .map_err(|why| a.invalid("node", why))
        },
    },
    Verb {
        name: "move_mode",
        doc: "Move a mode to another place in its collection; the first is the default every unset frame reads",
        params: &[
            req("mode", ParamKind::Id("mode"), "the mode"),
            req("index", INDEX, "its new place, counted from 0"),
        ],
        run: |ctx, a| {
            let mode = own_mode(ctx, a)?;
            let t = ctx.doc.variables();
            let col = t
                .collection_of_mode(mode)
                .ok_or_else(|| a.invalid("mode", "no mode has that id"))?;
            if col.by_width() {
                return Err(a.invalid(
                    "mode",
                    "a collection picked by width orders its modes by their widths",
                ));
            }
            let to = a.index("index").unwrap_or(0);
            if to >= col.modes.len() {
                let why = format!(
                    "the collection has {}",
                    plural(col.modes.len(), "mode", "modes")
                );
                return Err(a.invalid("index", why));
            }
            Ok(Done::ops(crate::variable::move_mode_ops(ctx.doc, mode, to)))
        },
    },
    Verb {
        name: "add_collection",
        doc: "Add a variable collection with one mode, last in the list; answers its id",
        params: &[opt(
            "name",
            TEXT,
            "its name; the first free `Collection N` unless given",
        )],
        run: |ctx, a| {
            let t = ctx.doc.variables();
            let taken: Vec<&str> = t.collections().iter().map(|c| c.name.as_str()).collect();
            let name = match a.text("name") {
                Some(n) => collection_name(ctx, a, n, None)?,
                None => crate::variable::numbered_name(&taken, "Collection", taken.len() + 1),
            };
            let (id, mode) = (ctx.ids.mint_collection(), ctx.ids.mint_mode());
            let ops = crate::variable::new_collection_ops(id, mode, &name, taken.len());
            Ok(Done::making(ops, id.0.to_wire()))
        },
    },
    Verb {
        name: "rename_collection",
        doc: "Rename a variable collection",
        params: &[
            req("collection", ParamKind::Id("collection"), "the collection"),
            req("name", TEXT, "its new name, unique among the collections"),
        ],
        run: |ctx, a| {
            let c = collection_req(ctx, a)?;
            let name = collection_name(ctx, a, a.text("name").unwrap_or_default(), Some(c))?;
            if ctx
                .doc
                .variables()
                .collection(c)
                .is_some_and(|col| col.name == name)
            {
                return Ok(Done::default());
            }
            Ok(Done::ops(vec![Operation::SetCollectionName {
                id: c,
                name,
            }]))
        },
    },
    Verb {
        name: "duplicate_collection",
        doc: "Copy a variable collection after itself, its modes and variables under new ids, aliases within it pointing at the copies; answers the copy's id",
        params: &[
            req("collection", ParamKind::Id("collection"), "the collection"),
            opt(
                "name",
                TEXT,
                "the copy's name; `<name> copy`, numbered if taken, unless given",
            ),
        ],
        run: |ctx, a| {
            let c = collection_req(ctx, a)?;
            let t = ctx.doc.variables();
            let name = match a.text("name") {
                Some(n) => collection_name(ctx, a, n, None)?,
                None => {
                    let taken: Vec<&str> =
                        t.collections().iter().map(|c| c.name.as_str()).collect();
                    let stem = format!("{} copy", t.collection(c).map_or("", |c| c.name.as_str()));
                    match taken.contains(&stem.as_str()) {
                        true => crate::variable::numbered_name(&taken, &stem, 2),
                        false => stem,
                    }
                }
            };
            let ops = crate::variable::duplicate_collection_ops(ctx.doc, c, &name, ctx.ids);
            let made = ops.iter().find_map(|op| match op {
                Operation::AddCollection { collection, .. } => Some(collection.id.0.to_wire()),
                _ => None,
            });
            Ok(match made {
                Some(id) => Done::making(ops, id),
                None => Done::ops(ops),
            })
        },
    },
    Verb {
        name: "delete_collection",
        doc: "Delete a variable collection and its variables; their users keep the values they drew, and frames set to its modes inherit",
        params: &[req(
            "collection",
            ParamKind::Id("collection"),
            "the collection",
        )],
        run: |ctx, a| {
            let c = collection_req(ctx, a)?;
            let ops = crate::variable::remove_collection_ops(ctx.doc, c);
            if ops.is_empty() {
                return Err(VerbError::Nothing("the last collection cannot go"));
            }
            let (_, layers, _) = crate::variable::collection_reach(ctx.doc, c);
            let done = Done::ops(ops);
            Ok(match layers {
                0 => done,
                n => done.saying(format!(
                    "{} {} their values",
                    plural(n, "layer", "layers"),
                    if n == 1 { "keeps" } else { "keep" }
                )),
            })
        },
    },
    Verb {
        name: "set_by_width",
        doc: "Pick a collection's mode by the width of the frame a layer sits in, or stop; one collection at most",
        params: &[
            req("collection", ParamKind::Id("collection"), "the collection"),
            req("on", BOOL, "whether its modes are picked by width"),
        ],
        run: |ctx, a| {
            let c = collection_req(ctx, a)?;
            let ops = crate::variable::by_width_ops(ctx.doc, c, a.bool("on").unwrap_or(true))
                .map_err(|why| a.invalid("collection", why))?;
            let pinned = ops
                .iter()
                .filter(|op| matches!(op, Operation::SetModes { .. }))
                .count();
            let done = Done::ops(ops);
            Ok(match pinned {
                0 => done,
                n => done.saying(format!(
                    "{} {} the mode {} drew",
                    plural(n, "frame", "frames"),
                    if n == 1 { "keeps" } else { "keep" },
                    if n == 1 { "it" } else { "they" }
                )),
            })
        },
    },
    Verb {
        name: "set_breakpoint",
        doc: "Set the minimum width a mode of a collection picked by width starts at",
        params: &[
            req(
                "mode",
                ParamKind::Id("mode"),
                "the mode, not its collection's first",
            ),
            req("width", ParamKind::Number, "its minimum width in px"),
        ],
        run: |ctx, a| {
            let mode = own_mode(ctx, a)?;
            let by_width = ctx
                .doc
                .variables()
                .collection_of_mode(mode)
                .is_some_and(|c| c.by_width());
            if !by_width {
                return Err(a.invalid(
                    "mode",
                    "its collection is not picked by width: `set_by_width` first",
                ));
            }
            crate::variable::breakpoint_ops(ctx.doc, mode, a.number("width").unwrap_or(0.0))
                .map(Done::ops)
                .map_err(|why| a.invalid("width", why))
        },
    },
    Verb {
        name: "set_width_query",
        doc: "Let a main's contents pick a collection's mode by the main's own width, or stop",
        params: &[
            req(
                "node",
                NODE,
                "the main component, a frame whose width is set from outside; or a frame that has a width query and is no instance, to stop",
            ),
            req("on", BOOL, "whether it picks by its width"),
            opt(
                "collection",
                ParamKind::Id("collection"),
                "the collection; the first unless given",
            ),
        ],
        run: |ctx, a| {
            let id = a.layer(ctx.doc, "node")?;
            let n = node(ctx, id)?;
            // As the mode rows offer *By width*: a main, or a frame holding a
            // query of its own that follows no main, so a detached copy can let
            // its main's go.
            if !(n.component() || (n.link().is_none() && !n.queries().is_empty())) {
                return Err(a.invalid(
                    "node",
                    "only a main component picks modes by its own width: make it a component first",
                ));
            }
            let c = collection_arg(ctx, a)?.unwrap_or(ctx.doc.variables().first_collection().id);
            let on = a.bool("on").unwrap_or(true);
            if on && let Some(why) = crate::variable::why_no_query(ctx.doc, id) {
                return Err(a.invalid("node", why));
            }
            let modes = ctx
                .doc
                .variables()
                .collection(c)
                .map_or(0, |c| c.modes.len());
            if on && modes < 2 {
                return Err(a.invalid(
                    "collection",
                    "a width picks between two modes or more, and the collection has fewer",
                ));
            }
            crate::variable::query_ops(ctx.doc, id, crate::variable::ModeOwner::Collection(c), on)
                .map(Done::ops)
                .map_err(|why| a.invalid("node", why))
        },
    },
    Verb {
        name: "set_scope",
        doc: "Narrow the fields a variable may bind, or let it bind every field of its kind; fields it leaves detach to what they drew",
        params: &[
            req("variable", ParamKind::Id("variable"), "the variable"),
            opt(
                "scope",
                ParamKind::Model("Scope"),
                "the fields, by name, of the variable's kind; absent for every field",
            ),
        ],
        run: |ctx, a| {
            use crate::variable::{Field, Scope};
            let id = own_variable(ctx, a)?;
            let t = ctx.doc.variables();
            let kind = t.get(id).map(|v| v.kind);
            let scope = match a.model_opt::<Vec<String>>("scope")? {
                None => None,
                Some(words) => {
                    let mut fields = Vec::new();
                    for w in &words {
                        let f = Field::from_wire(w)
                            .filter(|f| Some(f.kind()) == kind)
                            .ok_or_else(|| {
                                let ok: Vec<&str> = kind
                                    .into_iter()
                                    .flat_map(Field::of_kind)
                                    .map(Field::wire)
                                    .collect();
                                a.invalid("scope", format!("`{w}` is not one of {}", ok.join(", ")))
                            })?;
                        fields.push(f);
                    }
                    if fields.is_empty() {
                        return Err(a.invalid(
                            "scope",
                            "a scope names a field at least; leave it out for every field",
                        ));
                    }
                    Some(Scope::of(fields))
                }
            };
            if let Some(held) = crate::variable::scope_held_by(ctx.doc, id, scope) {
                let name = |v| t.get(v).map_or("", |v| v.name.as_str());
                return Err(a.invalid(
                    "scope",
                    format!(
                        "{} reads {} from another collection, so its words cannot change",
                        name(held),
                        name(id)
                    ),
                ));
            }
            let dropped = crate::variable::users_by_field(ctx.doc, id)
                .into_iter()
                .filter(|(f, _)| crate::variable::scope_drops(scope, *f))
                .flat_map(|(_, ids)| ids)
                .collect::<std::collections::BTreeSet<_>>()
                .len();
            let done = Done::ops(crate::variable::scope_ops(ctx.doc, id, scope));
            Ok(match dropped {
                0 => done,
                n => done.saying(format!(
                    "{} {} detached",
                    plural(n, "layer", "layers"),
                    if n == 1 { "was" } else { "were" }
                )),
            })
        },
    },
    Verb {
        name: "bind_variable",
        doc: "Bind a field of layers to a variable, or detach it to the value it draws",
        params: &[
            req("nodes", NODES, "the layers"),
            req(
                "field",
                ParamKind::Model("BoundField"),
                "the field and its place: `{\"fill\": 0}`, `{\"corner\": \"all\"}`, `{\"spacing\": \"gaps\"}` (or `padding`, `all`)",
            ),
            opt(
                "variable",
                ParamKind::Id("variable"),
                "the variable, of the field's kind; absent detaches",
            ),
        ],
        run: |ctx, a| {
            let field = a.model::<build::BoundField>("field")?;
            if !field.in_range() {
                return Err(a.invalid("field", "no such place in that field"));
            }
            let var = a.id_opt::<VarId>("variable")?;
            if var.is_some_and(|v| ctx.doc.variables().get(v).is_none()) {
                return Err(a.invalid("variable", "no variable has that id"));
            }
            // A page has no eye or opacity control, as `set_visible` and
            // `set_opacity` refuse it; its paint fields reach its shapes.
            let nodes = match field {
                build::BoundField::Visible | build::BoundField::Opacity => {
                    a.layers(ctx.doc, "nodes")?
                }
                _ => a.nodes("nodes")?,
            };
            for id in &nodes {
                node(ctx, *id)?;
            }
            let has_item = |i: usize, stroke: bool| {
                build::paint_targets(ctx.doc, &nodes).iter().any(|id| {
                    ctx.doc.get(*id).is_some_and(|n| match stroke {
                        true => n.paint().strokes.len() > i,
                        false => n.paint().fills.len() > i,
                    })
                })
            };
            match field {
                build::BoundField::Fill(i) if !has_item(i, false) => {
                    return Err(a.invalid("field", "none of the layers has a fill there"));
                }
                build::BoundField::Stroke(i) if !has_item(i, true) => {
                    return Err(a.invalid("field", "none of the layers has a stroke there"));
                }
                _ => {}
            }
            Ok(Done::edit(build::bind_field_ops(
                ctx.doc, &nodes, field, var,
            )))
        },
    },
    // --- styles -----------------------------------------------------------------
    Verb {
        name: "add_style",
        doc: "Add a style from a value or from a layer's look, and apply it to layers; answers its id",
        params: &[
            opt(
                "value",
                ParamKind::Model("StyleValue"),
                "what it holds; or give `from`",
            ),
            opt(
                "from",
                NODE,
                "a layer whose look in `slot` it takes; or give `value`",
            ),
            opt(
                "slot",
                ParamKind::Choice(STYLE_SLOTS),
                "the slot `from` is read in",
            ),
            opt(
                "name",
                TEXT,
                "its name; `<Slot> style`, numbered if taken, unless given",
            ),
            opt("nodes", NODES, "layers to apply it to"),
        ],
        run: |ctx, a| {
            let value = match (a.model_opt::<StyleInput>("value")?, a.get("from")) {
                (Some(input), None) => style_value(ctx, input, None),
                (None, Some(_)) => {
                    let slot = a.choice("slot").map(slot_of).ok_or(VerbError::Missing {
                        verb: "add_style",
                        param: "slot",
                    })?;
                    crate::style::value_drawn(ctx.doc, a.node("from")?, slot).ok_or_else(|| {
                        a.invalid("from", "it has nothing in that slot to make a style of")
                    })?
                }
                _ => {
                    return Err(a.invalid("value", "give a `value` or a `from` layer, one of them"));
                }
            };
            let slot = value.slot();
            let styles = ctx.doc.styles();
            let name = match a.text("name").map(str::trim) {
                Some(n) => {
                    if let Some(why) = crate::style::name_refusal(styles, slot, n, None) {
                        return Err(a.invalid("name", why));
                    }
                    n.to_owned()
                }
                None => crate::style::free_name(styles, slot, &format!("{} style", slot.title())),
            };
            let id = ctx.ids.mint_style();
            let mut ops = vec![crate::style::add_op(id, &name, value)];
            if a.get("nodes").is_some() {
                ops.extend(
                    crate::style::targets(ctx.doc, &style_nodes(ctx, a)?, slot)
                        .into_iter()
                        .map(|n| crate::style::bind_op(n, slot, Some(id))),
                );
            }
            Ok(Done::making(ops, id.0.to_wire()))
        },
    },
    Verb {
        name: "remove_style",
        doc: "Remove a style; its users keep the look they drew",
        params: &[req("style", ParamKind::Id("style"), "the style")],
        run: |ctx, a| {
            let id = own_style(ctx, a)?.id;
            Ok(Done::ops(crate::style::remove_ops(ctx.doc, &[id])))
        },
    },
    Verb {
        name: "rename_style",
        doc: "Rename a style",
        params: &[
            req("style", ParamKind::Id("style"), "the style"),
            req("name", TEXT, "its new name, unique among its slot's"),
        ],
        run: |ctx, a| {
            let id: StyleId = a.id("style")?;
            crate::style::rename_ops(ctx.doc.styles(), id, a.text("name").unwrap_or_default())
                .map(Done::ops)
                .map_err(|why| a.invalid("name", why))
        },
    },
    Verb {
        name: "set_style_value",
        doc: "Change what a style holds; every layer applying it follows",
        params: &[
            req("style", ParamKind::Id("style"), "the style"),
            req(
                "value",
                ParamKind::Model("StyleValue"),
                "its new value, of its own slot",
            ),
        ],
        run: |ctx, a| {
            let st = own_style(ctx, a)?;
            let (id, own) = (st.id, st.value.clone());
            let value = style_value(ctx, a.model("value")?, Some(&own));
            if value.slot() != own.slot() {
                return Err(a.invalid(
                    "value",
                    format!(
                        "a {} style holds {} only",
                        own.slot().title(),
                        own.slot().title()
                    ),
                ));
            }
            Ok(Done::ops(vec![Operation::SetStyleValue { id, value }]))
        },
    },
    Verb {
        name: "apply_style",
        doc: "Apply a style to layers, or detach them from the style in a slot",
        params: &[
            req("nodes", NODES, "the layers"),
            opt(
                "style",
                ParamKind::Id("style"),
                "the style; absent detaches",
            ),
            opt(
                "slot",
                ParamKind::Choice(STYLE_SLOTS),
                "the slot to detach; the style's when it is given",
            ),
        ],
        run: |ctx, a| {
            let nodes = style_nodes(ctx, a)?;
            let (style, slot) = match a.id_opt::<StyleId>("style")? {
                Some(id) => {
                    let st = ctx
                        .doc
                        .styles()
                        .get(id)
                        .ok_or_else(|| a.invalid("style", "no style has that id"))?;
                    (Some(id), st.slot())
                }
                None => {
                    let slot = a.choice("slot").map(slot_of).ok_or(VerbError::Missing {
                        verb: "apply_style",
                        param: "slot",
                    })?;
                    (None, slot)
                }
            };
            let ops = crate::style::targets(ctx.doc, &nodes, slot)
                .into_iter()
                .filter(|n| {
                    style.is_some()
                        || ctx
                            .doc
                            .get(*n)
                            .is_some_and(|x| x.styles().get(slot).is_some())
                })
                .map(|n| crate::style::bind_op(n, slot, style))
                .collect();
            Ok(Done::ops(ops))
        },
    },
    // --- scripts ----------------------------------------------------------------
    Verb {
        name: "add_script",
        doc: "Store a Luau script in the document, last in its list; answers its id. A stored script never runs by itself",
        params: &[
            req("name", TEXT, "its name in the script list"),
            req("source", TEXT, "its Luau source"),
        ],
        run: |ctx, a| {
            let name = script_name(a)?;
            let id = ctx.ids.mint_script();
            let script = crate::script::Script {
                id,
                name,
                source: a.text("source").unwrap_or_default().to_owned(),
            };
            let index = ctx.doc.scripts().len();
            Ok(Done::making(
                vec![Operation::AddScript { script, index }],
                id.0.to_wire(),
            ))
        },
    },
    Verb {
        name: "set_script",
        doc: "Rename a stored script or replace its source",
        params: &[
            req("script", ParamKind::Id("script"), "the script"),
            opt("name", TEXT, "its new name"),
            opt("source", TEXT, "its new Luau source, whole"),
        ],
        run: |ctx, a| {
            let id = own_script(ctx, a)?;
            let mut ops = Vec::new();
            if a.get("name").is_some() {
                ops.push(Operation::SetScriptName {
                    id,
                    name: script_name(a)?,
                });
            }
            if let Some(source) = a.text("source") {
                ops.push(Operation::SetScriptSource {
                    id,
                    source: source.to_owned(),
                });
            }
            if ops.is_empty() {
                return Err(a.invalid("script", "give a `name` or a `source` to set"));
            }
            Ok(Done::ops(ops))
        },
    },
    Verb {
        name: "remove_script",
        doc: "Remove a stored script from the document",
        params: &[req("script", ParamKind::Id("script"), "the script")],
        run: |ctx, a| {
            let id = own_script(ctx, a)?;
            Ok(Done::ops(vec![Operation::RemoveScript { id }]))
        },
    },
    // --- pages ----------------------------------------------------------------
    Verb {
        name: "add_page",
        doc: "Add a page; answers it",
        params: &[
            opt(
                "name",
                TEXT,
                "its name; the first free `Page N` unless given",
            ),
            opt(
                "index",
                INDEX,
                "its place in the page order, 0 the first; last unless given",
            ),
        ],
        run: |ctx, a| {
            let index = a.index("index").unwrap_or(usize::MAX);
            let name = a
                .text("name")
                .map(|n| page_name(a, n).map(str::to_owned))
                .transpose()?;
            let (tx, id) = crate::page::add(ctx.doc, ctx.ids, index, name, None);
            Ok(Done::made(tx, vec![id]))
        },
    },
    Verb {
        name: "duplicate_page",
        doc: "Copy a page below it, with its layers and guides; answers the copy",
        params: &[req("page", NODE, "the page")],
        run: |ctx, a| {
            let page = a.node("page")?;
            let (tx, id) = crate::page::duplicate(ctx.doc, ctx.ids, page)
                .ok_or_else(|| a.invalid("page", "not a page"))?;
            Ok(Done::made(tx, vec![id]))
        },
    },
    Verb {
        name: "delete_page",
        doc: "Delete a page with its layers and guides; instances of a main on it are detached",
        params: &[req("page", NODE, "the page")],
        run: |ctx, a| {
            let page = a.node("page")?;
            if !ctx.doc.is_page(page) {
                return Err(a.invalid("page", "not a page"));
            }
            crate::page::delete(ctx.doc, page)
                .map(Done::edit)
                .ok_or(VerbError::Nothing("a document keeps at least one page"))
        },
    },
    Verb {
        name: "move_page",
        doc: "Move a page to `index` in the page order, 0 the first",
        params: &[
            req("page", NODE, "the page"),
            req("index", INDEX, "its new place"),
        ],
        run: |ctx, a| {
            let page = a.node("page")?;
            if !ctx.doc.is_page(page) {
                return Err(a.invalid("page", "not a page"));
            }
            let last = ctx.doc.pages().len() - 1;
            Ok(Done::ops(vec![Operation::MovePage {
                id: page,
                index: a.index("index").unwrap_or(0).min(last),
            }]))
        },
    },
    Verb {
        name: "set_page_draft",
        doc: "Mark a page as a draft, left out of publishing, export all and present, or not",
        params: &[req("page", NODE, "the page"), req("draft", BOOL, "a draft")],
        run: |ctx, a| {
            let page = a.node("page")?;
            if !ctx.doc.is_page(page) {
                return Err(a.invalid("page", "not a page"));
            }
            Ok(Done::ops(vec![Operation::SetPageDraft {
                id: page,
                draft: a.bool("draft").unwrap_or(true),
            }]))
        },
    },
    Verb {
        name: "set_canvas_colour",
        doc: "Set the colour behind and around a page's frames, or clear it to the viewer's default",
        params: &[
            req("page", NODE, "the page"),
            opt(
                "colour",
                ParamKind::Model("Color"),
                "the colour, opaque; the viewer's default unless given",
            ),
        ],
        run: |ctx, a| {
            let page = a.node("page")?;
            if !ctx.doc.is_page(page) {
                return Err(a.invalid("page", "not a page"));
            }
            // Nothing lies behind the canvas for an alpha to show.
            let ground = a
                .model_opt::<peniko::Color>("colour")?
                .map(|c| c.with_alpha(1.0));
            Ok(Done::ops(vec![Operation::SetPageGround {
                id: page,
                ground,
            }]))
        },
    },
    Verb {
        name: "set_flows",
        doc: "Replace a page's prototype flows: each a name, a start frame, starting modes and starting variable values",
        params: &[
            req("page", NODE, "the page"),
            req(
                "flows",
                ParamKind::Model("[Flow]"),
                "every flow; one without an `id` is new",
            ),
        ],
        run: |ctx, a| {
            let page = a.node("page")?;
            if !ctx.doc.is_page(page) {
                return Err(a.invalid("page", "not a page"));
            }
            let ids = &mut *ctx.ids;
            let flows: Vec<Keyed<crate::prototype::Flow>> = keyed_arg(a, "flows", "[Flow]", |v| {
                ids_in(v, &mut || ids.mint_item());
                Ok(())
            })?;
            if let Some(twice) = crate::item::first_duplicate(&flows) {
                let why = format!("two flows share the id `{}`", twice.0.to_wire());
                return Err(a.invalid("flows", why));
            }
            // A start that is gone is kept, as a target is; one that is there but
            // no frame would never open.
            for (i, f) in flows.iter().enumerate() {
                if ctx
                    .doc
                    .get(f.start)
                    .is_some_and(|n| !matches!(n.kind(), NodeKind::Artboard { .. }))
                {
                    let why = format!(
                        "at `[{i}].start`: `{}` is no frame; a flow starts on a frame",
                        f.start.to_wire()
                    );
                    return Err(a.invalid("flows", why));
                }
            }
            Ok(Done::ops(vec![Operation::SetFlows { id: page, flows }]))
        },
    },
    Verb {
        name: "set_page_play",
        doc: "Set a page's prototype settings: its device, scaling and the transition a new link takes",
        params: &[
            req("page", NODE, "the page"),
            req("play", ParamKind::Model("PagePlay"), "the settings"),
        ],
        run: |ctx, a| {
            let page = a.node("page")?;
            if !ctx.doc.is_page(page) {
                return Err(a.invalid("page", "not a page"));
            }
            let play = a.model::<crate::prototype::PagePlay>("play")?;
            if !play.transition.timing.is_usable() {
                return Err(a.invalid("play", UNUSABLE_TIMING));
            }
            // The door keeps a name no preset has (a file may name one); a
            // verb refuses it, since the screen would play as drawn unsaid.
            if let Some(d) = play.device.as_deref()
                && crate::presets::device_size(d).is_none()
            {
                let names: Vec<&str> = crate::presets::FRAME_PRESETS
                    .iter()
                    .filter(|(g, _)| crate::presets::DEVICE_GROUPS.contains(g))
                    .flat_map(|(_, entries)| entries.iter().map(|(n, ..)| *n))
                    .collect();
                let why = format!("no device is called `{d}`: one of {}", names.join(", "));
                return Err(a.invalid("play", why));
            }
            Ok(Done::ops(vec![Operation::SetPagePlay { id: page, play }]))
        },
    },
    Verb {
        name: "set_play_export",
        doc: "Set the prototype's HTML export settings, as the export card keeps them: the flows that go in, the one it opens on, device frame, player bar, hotspot hints, scaling and a responsive page on a phone",
        params: &[req(
            "settings",
            ParamKind::Model("PlayExport"),
            "every setting; a flow is named by its page and its id as the page's `flows` read",
        )],
        run: |ctx, a| {
            let settings = a.model::<crate::prototype::PlayExport>("settings")?;
            let named = |f: &crate::prototype::FlowRef| {
                ctx.doc
                    .page(f.page)
                    .is_some_and(|p| p.flows.iter().any(|k| k.id == f.flow))
            };
            let every = || {
                let flows: Vec<String> = ctx
                    .doc
                    .pages()
                    .iter()
                    .flat_map(|p| {
                        p.flows.iter().map(|k| {
                            let page = p.id.to_wire();
                            format!(
                                "{} ({{page: {page}, flow: {}}})",
                                k.value.name,
                                k.id.0.to_wire()
                            )
                        })
                    })
                    .collect();
                match flows.is_empty() {
                    true => "the document has no flows; add one with `set_flows`".to_string(),
                    false => format!("its flows are {}", flows.join(", ")),
                }
            };
            if let Some(f) = settings
                .flows
                .iter()
                .chain(&settings.opens)
                .find(|f| !named(f))
            {
                let why = format!(
                    "{{page: {}, flow: {}}} is no flow; {}",
                    f.page.to_wire(),
                    f.flow.0.to_wire(),
                    every()
                );
                return Err(a.invalid("settings", why));
            }
            if let Some(o) = &settings.opens
                && !settings.flows.is_empty()
                && !settings.flows.contains(o)
            {
                return Err(a.invalid("settings", "`opens` is a flow `flows` lists"));
            }
            Ok(Done::ops(vec![Operation::SetPlayExport { settings }]))
        },
    },
    Verb {
        name: "set_guides",
        doc: "Replace the ruler guides of a frame or a page: a listed guide with an `id` keeps it, one without is added, and an unlisted one is removed",
        params: &[
            req(
                "owner",
                NODE,
                "the frame the guides are scoped to, or the page they run across",
            ),
            req(
                "guides",
                ParamKind::Model("[Guide]"),
                "every guide it keeps; an empty list removes them all",
            ),
        ],
        run: |ctx, a| {
            let owner = a.node("owner")?;
            let is_frame =
                node(ctx, owner).is_ok_and(|n| matches!(n.kind(), NodeKind::Artboard { .. }));
            if !ctx.doc.is_page(owner) && !is_frame {
                return Err(a.invalid("owner", "guides belong to a frame or a page"));
            }
            let wanted = a.model::<Vec<GuideIn>>("guides")?;
            nothing_if_empty(
                Transaction(guide_ops(ctx, a, owner, &wanted)?),
                "Those are its guides already",
            )
        },
    },
    // --- placement and geometry -------------------------------------------
    Verb {
        name: "set_transform",
        doc: "Set a layer's placement in its parent",
        params: &[
            req("node", NODE, "the layer"),
            req(
                "transform",
                ParamKind::Model("Affine"),
                "the six coefficients",
            ),
        ],
        run: |ctx, a| {
            let id = a.layer(ctx.doc, "node")?;
            let transform: Affine = a.model("transform")?;
            // Its turn and scale land; a move of a laid-out layer would not.
            let was = node(ctx, id)?.transform().translation();
            if transform.translation() != was
                && let Some(why) = build::laid_out_refusal(ctx.doc, id)
            {
                return Err(a.invalid("transform", why));
            }
            Ok(Done::ops(vec![Operation::SetTransform { id, transform }]))
        },
    },
    Verb {
        name: "set_pivot",
        doc: "Set or clear the point a layer turns and scales about",
        params: &[
            req("node", NODE, "the layer"),
            opt(
                "pivot",
                ParamKind::Model("Pivot"),
                "the point; absent clears it",
            ),
        ],
        run: |ctx, a| {
            Ok(Done::ops(vec![Operation::SetPivot {
                id: a.layer(ctx.doc, "node")?,
                pivot: a.model_opt::<Pivot>("pivot")?,
            }]))
        },
    },
    Verb {
        name: "set_geometry",
        doc: "Change a layer's kind-specific geometry: a size, a path, a corner radius",
        params: &[
            req("node", NODE, "the layer"),
            req("geometry", ParamKind::Model("GeometryPatch"), "the change"),
        ],
        run: |ctx, a| {
            let id = a.node("node")?;
            let mut geometry = a.model::<GeometryPatch>("geometry")?;
            let negative = match &geometry {
                GeometryPatch::TextSizing(TextSizing::AutoHeight(w)) => *w < 0.0,
                GeometryPatch::TextSizing(TextSizing::Fixed(s)) => s.width < 0.0 || s.height < 0.0,
                _ => false,
            };
            if negative {
                return Err(a.invalid("geometry", "a size is not negative"));
            }
            keep_radii(ctx, a, id, &mut geometry)?;
            if bakes_rounding(ctx, id, &geometry) {
                return Err(a.invalid(
                    "geometry",
                    "this is the path as drawn, its rounded corners expanded; write the points the snapshot's `subpaths` lists, or move one with `set_point`",
                ));
            }
            Ok(Done::ops(vec![Operation::SetGeometry { id, geometry }]))
        },
    },
    Verb {
        name: "set_point",
        doc: "Move one point of a path, aim its handles or round its corner, in the layer's own space as the snapshot's `subpaths` lists them",
        params: &[
            req("node", NODE, "the path"),
            req(
                "point",
                INDEX,
                "which point of the subpath, counted from 0 as `subpaths` lists them",
            ),
            opt("subpath", INDEX, "which subpath; 0 unless given"),
            opt(
                "x",
                ParamKind::Number,
                "where the point goes across; its handles move with it",
            ),
            opt(
                "y",
                ParamKind::Number,
                "where the point goes down; its handles move with it",
            ),
            opt(
                "in_x",
                ParamKind::Number,
                "where the handle arriving at the point ends, across",
            ),
            opt(
                "in_y",
                ParamKind::Number,
                "where the handle arriving at the point ends, down",
            ),
            opt(
                "out_x",
                ParamKind::Number,
                "where the handle leaving the point ends, across; one handle given on a smooth point mirrors the other",
            ),
            opt(
                "out_y",
                ParamKind::Number,
                "where the handle leaving the point ends, down",
            ),
            opt(
                "radius",
                ParamKind::Number,
                "how far the corner at the point is rounded; 0 for sharp",
            ),
        ],
        run: set_point,
    },
    Verb {
        name: "set_size",
        doc: "Make a layer this wide and high, whatever its kind; a text gets a fixed box",
        params: &[
            req("node", NODE, "the layer"),
            req("width", ParamKind::Number, "its width"),
            req("height", ParamKind::Number, "its height"),
        ],
        run: |ctx, a| {
            let id = a.node("node")?;
            node(ctx, id)?;
            let want = kurbo::Size::new(
                a.number("width").unwrap_or(0.0),
                a.number("height").unwrap_or(0.0),
            );
            if want.width < 0.0 || want.height < 0.0 {
                return Err(a.invalid("width", "a size is not negative"));
            }
            Ok(Done::edit(crate::edit::resize_to(
                ctx.doc, ctx.res, id, want,
            )))
        },
    },
    Verb {
        name: "pixel_fit",
        doc: "Put layers' edges and points on whole units, so they render crisp; text and turned layers are left",
        params: &[req(
            "nodes",
            NODES,
            "the layers, groups and frames taken whole",
        )],
        run: |ctx, a| {
            let (tx, _) = crate::pixel_fit::fit(ctx.doc, ctx.res, &a.nodes("nodes")?);
            Ok(Done::edit(tx))
        },
    },
    // --- layer state --------------------------------------------------------
    Verb {
        name: "set_name",
        doc: "Rename a layer or a page",
        params: &[
            req("node", NODE, "the layer or page"),
            req("name", TEXT, "its new name"),
        ],
        run: |ctx, a| {
            let id = a.node("node")?;
            let name = a.text("name").unwrap_or_default();
            let name = match ctx.doc.is_page(id) {
                true => page_name(a, name)?,
                false => name,
            };
            Ok(Done::ops(vec![Operation::SetName {
                id,
                name: name.to_owned(),
            }]))
        },
    },
    Verb {
        name: "set_visible",
        doc: "Show or hide layers",
        params: &[
            req("nodes", NODES, "the layers"),
            req("visible", BOOL, "shown"),
        ],
        run: |ctx, a| {
            let visible = a.bool("visible").unwrap_or(true);
            each(ctx.doc, a, |id| Operation::SetVisible { id, visible })
        },
    },
    Verb {
        name: "set_locked",
        doc: "Lock or unlock layers",
        params: &[
            req("nodes", NODES, "the layers"),
            req("locked", BOOL, "locked"),
        ],
        run: |ctx, a| {
            let locked = a.bool("locked").unwrap_or(false);
            each(ctx.doc, a, |id| Operation::SetLocked { id, locked })
        },
    },
    Verb {
        name: "set_opacity",
        doc: "Set layers' opacity, 0 to 1",
        params: &[
            req("nodes", NODES, "the layers"),
            req("opacity", ParamKind::Number, "0 to 1"),
        ],
        run: |ctx, a| {
            let opacity = a.number("opacity").unwrap_or(1.0) as f32;
            if !build::valid_opacity(opacity) {
                return Err(a.invalid("opacity", "expected 0 to 1"));
            }
            each(ctx.doc, a, |id| Operation::SetOpacity { id, opacity })
        },
    },
    Verb {
        name: "set_blend",
        doc: "Set how layers blend with what is under them; Pass through is Normal on a layer that is not a frame or group",
        params: &[
            req(
                "nodes",
                NODES,
                "the layers; a member inside another is left to it",
            ),
            req("blend", ParamKind::Choice(BLENDS), "the mode"),
        ],
        run: |ctx, a| {
            let blend = blend_of(a.choice("blend").unwrap_or("Normal"));
            Ok(Done::edit(build::set_blend_all(
                ctx.doc,
                &a.layers(ctx.doc, "nodes")?,
                blend,
            )))
        },
    },
    Verb {
        name: "set_clip",
        doc: "Clip frames' contents to their boxes, or not",
        params: &[
            req("nodes", NODES, "the frames"),
            req("clip", BOOL, "clipped"),
        ],
        run: |ctx, a| {
            let clip = a.bool("clip").unwrap_or(true);
            each(ctx.doc, a, |id| Operation::SetClip { id, clip })
        },
    },
    Verb {
        name: "set_mask",
        doc: "Use layers as masks for the layers above each in its parent, or stop",
        params: &[
            req("nodes", NODES, "the layers"),
            req("mask", BOOL, "a mask"),
            opt(
                "mode",
                ParamKind::Choice(MASK_MODES),
                "what of the mask decides what survives: its outline (`shape`), its alpha, or its brightness (`luminance`); kept unless given",
            ),
            opt(
                "inverted",
                BOOL,
                "keep what lies outside the mask instead of inside; kept unless given",
            ),
        ],
        run: |ctx, a| {
            let mask = a.bool("mask").unwrap_or(true);
            let inverted = a.bool("inverted");
            if inverted.is_some() && !mask {
                return Err(a.invalid("inverted", "only a mask is inverted"));
            }
            let mode = match a.choice("mode") {
                Some(_) if !mask => return Err(a.invalid("mode", "only a mask has a mode")),
                Some(word) => Some(
                    MASK_MODES
                        .iter()
                        .position(|w| *w == word)
                        .map_or(crate::node::MaskMode::Shape, |i| {
                            crate::node::MaskMode::ALL[i]
                        }),
                ),
                None => None,
            };
            let mut ops = Vec::new();
            for id in a.layers(ctx.doc, "nodes")? {
                // A mask leaves its container's flow, so it is written where it
                // is drawn first, as *Use as mask* does (`build::mask`).
                if mask && !node(ctx, id)?.mask() {
                    ops.extend(build::baked(ctx.doc, ctx.res, id));
                }
                ops.push(Operation::SetMask { id, mask });
                if let Some(mode) = mode
                    && node(ctx, id)?.mask_mode() != mode
                {
                    ops.push(Operation::SetMaskMode { id, mode });
                }
                if let Some(inverted) = inverted
                    && node(ctx, id)?.mask_inverted() != inverted
                {
                    ops.push(Operation::SetMaskInverted { id, inverted });
                }
            }
            Ok(Done::ops(ops))
        },
    },
    Verb {
        name: "set_element",
        doc: "Set the HTML element a code export writes a frame, group or text as, or Automatic; a variant writes every variant of its set",
        params: &[
            req("node", NODE, "the frame, group or text"),
            opt(
                "element",
                ParamKind::Choice(ELEMENTS),
                "its tag; absent for Automatic",
            ),
        ],
        run: |ctx, a| {
            use crate::element::{self, HtmlElement};
            let id = a.layer(ctx.doc, "node")?;
            let n = node(ctx, id)?;
            element_refusal(ctx, a, id)?;
            let element = match a.choice("element") {
                None => None,
                Some(tag) => {
                    let offered = element::offered(n.kind()).unwrap_or(&[]);
                    let e = offered
                        .iter()
                        .copied()
                        .find(|e| e.tag() == tag)
                        .ok_or_else(|| {
                            let tags: Vec<&str> = offered.iter().map(|e| e.tag()).collect();
                            a.invalid("element", format!("it takes {}", tags.join(", ")))
                        })?;
                    if let Some(why) = element::targets(ctx.doc, id)
                        .iter()
                        .find_map(|t| element::refusal(ctx.doc, *t, e))
                    {
                        return Err(a.invalid("element", why));
                    }
                    Some::<HtmlElement>(e)
                }
            };
            Ok(Done::ops(element::set_ops(ctx.doc, id, element)))
        },
    },
    Verb {
        name: "set_link",
        doc: "Set the address a layer written as a link (`a`) goes to; empty clears it",
        params: &[
            req("node", NODE, "the layer, its element `a`"),
            req(
                "href",
                TEXT,
                "an https:// or http:// address, a mailto: or tel: link, a /path or a #id",
            ),
        ],
        run: |ctx, a| {
            let id = a.layer(ctx.doc, "node")?;
            if node(ctx, id)?.element() != Some(crate::element::HtmlElement::Link) {
                return Err(a.invalid(
                    "node",
                    "only a layer written as a link takes an address: set its element to `a` first",
                ));
            }
            element_refusal(ctx, a, id)?;
            let href = a.text("href").unwrap_or_default().trim();
            if !crate::element::is_href(href) {
                return Err(a.invalid("href", "not an address a link can take"));
            }
            if node(ctx, id)?.href() == href {
                return Ok(Done::default());
            }
            Ok(Done::ops(vec![Operation::SetHref {
                id,
                href: href.to_owned(),
            }]))
        },
    },
    Verb {
        name: "set_note",
        doc: "Set a layer's annotation, what the drawing cannot say (\"this list scrolls\"); empty clears it",
        params: &[
            req("node", NODE, "the layer"),
            req("text", TEXT, "the annotation, lines and all"),
        ],
        run: |ctx, a| {
            let id = a.layer(ctx.doc, "node")?;
            let text = a.text("text").unwrap_or_default().trim().to_owned();
            if node(ctx, id)?.note() == text {
                return Ok(Done::default());
            }
            Ok(Done::ops(vec![Operation::SetNote { id, text }]))
        },
    },
    Verb {
        name: "set_description",
        doc: "Set the description of a main component or set, a style or a variable; empty clears it",
        params: &[
            opt("node", NODE, "a main component or set"),
            opt("style", ParamKind::Id("style"), "a style"),
            opt("variable", ParamKind::Id("variable"), "a variable"),
            req("text", TEXT, "the description"),
        ],
        run: |ctx, a| {
            use crate::op::Described;
            let of = match (a.get("node"), a.get("style"), a.get("variable")) {
                (Some(_), None, None) => Described::Node(component_owner(ctx, a)?),
                (None, Some(_), None) => Described::Style(own_style(ctx, a)?.id),
                (None, None, Some(_)) => Described::Variable(own_variable(ctx, a)?),
                _ => {
                    return Err(a.invalid("node", "give one of `node`, `style` and `variable`"));
                }
            };
            let text = a.text("text").unwrap_or_default().trim().to_owned();
            let now = match of {
                Described::Node(id) => node(ctx, id)?.description(),
                Described::Style(id) => ctx
                    .doc
                    .styles()
                    .get(id)
                    .map_or("", |s| s.description.as_str()),
                Described::Variable(id) => ctx
                    .doc
                    .variables()
                    .get(id)
                    .map_or("", |v| v.description.as_str()),
            };
            if now == text {
                return Ok(Done::default());
            }
            Ok(Done::ops(vec![Operation::SetDescription { of, text }]))
        },
    },
    Verb {
        name: "set_doc_link",
        doc: "Set the documentation address of a main component or set; empty clears it",
        params: &[
            req("node", NODE, "the main component or set"),
            req(
                "url",
                TEXT,
                "an http or https address; a bare host takes https://",
            ),
        ],
        run: |ctx, a| {
            let id = component_owner(ctx, a)?;
            let url = crate::node::web_link(a.text("url").unwrap_or_default())
                .map_err(|why| a.invalid("url", why))?;
            if node(ctx, id)?.doc_link() == url {
                return Ok(Done::default());
            }
            Ok(Done::ops(vec![Operation::SetDocLink { id, url }]))
        },
    },
    Verb {
        name: "convert_container",
        doc: "Turn a group into a frame of its own box, or a frame into a group keeping its look; answers what it became",
        params: &[req("node", NODE, "the group or frame")],
        run: |ctx, a| {
            let id = a.layer(ctx.doc, "node")?;
            if !matches!(
                node(ctx, id)?.kind(),
                NodeKind::Group | NodeKind::Artboard { .. }
            ) {
                return Err(a.invalid("node", "neither a group nor a frame"));
            }
            if let Some(why) = build::convert_container_refusal(ctx.doc, id) {
                return Err(a.invalid("node", why));
            }
            let (tx, new) = build::convert_container(ctx.doc, ctx.res, ctx.ids, id)?;
            Ok(Done::made(tx, vec![new]))
        },
    },
    Verb {
        name: "fit_to_content",
        doc: "Resize a frame without a layout to the box of what is inside it, everything inside staying where it is drawn",
        params: &[req("node", NODE, "the frame")],
        run: |ctx, a| {
            let id = a.layer(ctx.doc, "node")?;
            let n = node(ctx, id)?;
            build::fit_frame(ctx.doc, ctx.res, id)
                .map(Done::edit)
                .ok_or_else(|| {
                    let why = match n.kind() {
                        NodeKind::Group => "a group always fits its content",
                        NodeKind::Artboard { .. } if n.display().is_some() => {
                            "a frame with a layout sizes itself: give its width and height fit-content with `set_layout_item`"
                        }
                        NodeKind::Artboard { .. } if n.children().is_empty() => {
                            "nothing inside to fit to"
                        }
                        NodeKind::Artboard { .. } => "already fits its content",
                        _ => "only a frame fits its content",
                    };
                    a.invalid("node", why)
                })
        },
    },
    Verb {
        name: "replace_font",
        doc: "Replace a font family everywhere the document sets it: layers, runs, text styles, charts and string variables",
        params: &[
            req("from", TEXT, "the family replaced, in any case"),
            req(
                "to",
                TEXT,
                "the family set instead, a name `list_fonts` answers",
            ),
        ],
        run: |ctx, a| {
            let (from, to) = (
                a.text("from").unwrap_or_default().trim(),
                a.text("to").unwrap_or_default().trim(),
            );
            if to.is_empty() {
                return Err(a.invalid("to", "a family needs a name"));
            }
            // A replacement within one family would still drop every match's
            // axis and feature settings, which go with the old typeface.
            if from.to_lowercase() == to.to_lowercase() {
                return Err(a.invalid("to", "it is the family replaced"));
            }
            let tx = crate::families::replace_family(ctx.doc, from, to);
            if tx.0.is_empty() {
                return Err(VerbError::Nothing(
                    "nothing of the document's own sets that family: no layer, run, text style, chart or variable, though a kit's may",
                ));
            }
            Ok(Done::edit(tx))
        },
    },
    // --- text ---------------------------------------------------------------
    Verb {
        name: "set_text",
        doc: "Replace a text layer's characters, dropping its runs of character styling",
        params: &[
            req("node", NODE, "the text layer"),
            req("text", TEXT, "the new text"),
        ],
        run: |_, a| {
            Ok(Done::ops(vec![Operation::SetText {
                id: a.node("node")?,
                content: a.text("text").unwrap_or_default().to_owned(),
                spans: Default::default(),
                para_spans: Default::default(),
            }]))
        },
    },
    Verb {
        name: "replace_text",
        doc: "Replace what text layers say wherever `find` matches, each replacement styled as its match was; locked layers are left alone",
        params: &[
            req("find", TEXT, "what to look for"),
            req("replace", TEXT, "what takes its place"),
            opt(
                "nodes",
                NODES,
                "only the text in and under these layers; every page but drafts unless given",
            ),
            opt(
                "match_case",
                BOOL,
                "match capitals exactly; false unless given",
            ),
            opt(
                "whole_words",
                BOOL,
                "match only whole words; false unless given",
            ),
            opt(
                "pattern",
                BOOL,
                "`find` is a regular expression, and `[1]` in `replace` inserts its first group; false unless given",
            ),
            opt(
                "keep_case",
                BOOL,
                "give each replacement the capitals of what it replaces; false unless given",
            ),
            opt(
                "inside_instances",
                BOOL,
                "also override text inside instances; false unless given",
            ),
        ],
        run: |ctx, a| {
            use crate::find::{self, Query, Scope};
            let flag = |name| a.bool(name).unwrap_or(false);
            let query = Query::new(
                a.text("find").unwrap_or_default(),
                a.text("replace").unwrap_or_default(),
                flag("match_case"),
                flag("whole_words"),
                flag("pattern"),
                flag("keep_case"),
            )
            .map_err(|why| a.invalid("find", why))?;
            // No page is in view here, so no draft is searched unless named.
            let scope = match a.get("nodes") {
                Some(_) => Scope::Under(a.nodes("nodes")?),
                None => Scope::Under(ctx.doc.gathered_pages().collect()),
            };
            let hits = find::find(ctx.doc, &scope, &query);
            let done = find::replace(ctx.doc, &hits, &query, flag("inside_instances"));
            if done.matches == 0 {
                return Err(VerbError::Nothing(match hits.is_empty() {
                    true => "nothing matches",
                    false => "every match is locked or inside an instance",
                }));
            }
            let alone = match hits.len() - done.matches {
                0 => String::new(),
                n => format!("; {n} locked or in an instance left alone"),
            };
            Ok(Done::edit(done.tx).saying(format!(
                "Replaced {} in {}{alone}",
                plural(done.matches, "match", "matches"),
                plural(done.layers, "layer", "layers"),
            )))
        },
    },
    Verb {
        name: "set_text_style",
        doc: "Set text layers' character style, and optionally one text's runs",
        params: &[
            req("nodes", NODES, "the text layers"),
            req(
                "style",
                ParamKind::Model("TextStyle"),
                "the fields to change; each text keeps the rest of its own",
            ),
            opt(
                "spans",
                ParamKind::Model("CharSpans"),
                "the runs over it, counted in one text's characters",
            ),
        ],
        run: |ctx, a| {
            let ids = texts(ctx, a)?;
            let mut patch = a.get("style").cloned().unwrap_or_default();
            loose::loosen("", &mut patch);
            let Value::Object(patch) = patch else {
                return Err(a.invalid("style", "expected an object"));
            };
            let spans = a.model_opt::<CharSpans>("spans")?;
            if spans.is_some() && ids.len() > 1 {
                return Err(a.invalid("spans", "runs are counted in one text's characters"));
            }
            let mut ops = Vec::new();
            for id in ids {
                let Some(NodeKind::Text { style, content, .. }) = ctx.doc.get(id).map(|n| n.kind())
                else {
                    continue;
                };
                let mut whole = serde_json::to_value(style.as_ref()).unwrap_or_default();
                if let Value::Object(map) = &mut whole {
                    map.extend(patch.clone());
                }
                let style = settle::<TextStyle>("TextStyle", whole)
                    .and_then(|v| read_model::<TextStyle>(&v))
                    .map_err(|why| a.invalid("style", why))?;
                let spans = match &spans {
                    Some(s) => Some(spans_in_bytes(a, "spans", content, s.clone(), &style)?),
                    None => None,
                };
                ops.push(Operation::SetTextStyle { id, style, spans });
            }
            Ok(Done::ops(ops))
        },
    },
    Verb {
        name: "set_text_spans",
        doc: "Set a text layer's runs of character styling",
        params: &[
            req("node", NODE, "the text layer"),
            req(
                "spans",
                ParamKind::Model("CharSpans"),
                "the runs, counted in its characters",
            ),
        ],
        run: |ctx, a| {
            let (t, content) = text_of(ctx, a)?;
            let spans = spans_in_bytes(a, "spans", content, a.model("spans")?, t.style)?;
            Ok(Done::ops(vec![Operation::SetTextSpans {
                id: a.node("node")?,
                spans,
            }]))
        },
    },
    Verb {
        name: "set_paragraph_style",
        doc: "Set text layers' paragraph style",
        params: &[
            req("nodes", NODES, "the text layers"),
            req("paragraph", ParamKind::Model("ParagraphStyle"), "the style"),
        ],
        run: |ctx, a| {
            let paragraph = a.model::<ParagraphStyle>("paragraph")?;
            Ok(Done::ops(
                texts(ctx, a)?
                    .into_iter()
                    .map(|id| Operation::SetParagraphStyle {
                        id,
                        paragraph: paragraph.clone(),
                        spans: None,
                    })
                    .collect(),
            ))
        },
    },
    Verb {
        name: "set_block_style",
        doc: "Set text layers' block style: the box, overflow and vertical alignment",
        params: &[
            req("nodes", NODES, "the text layers"),
            req("block", ParamKind::Model("BlockStyle"), "the style"),
        ],
        run: |ctx, a| {
            let block = a.model::<BlockStyle>("block")?;
            Ok(Done::ops(
                texts(ctx, a)?
                    .into_iter()
                    .map(|id| Operation::SetBlockStyle { id, block })
                    .collect(),
            ))
        },
    },
    Verb {
        name: "style_text",
        doc: "Set character attributes on a text layer, over all of it or a range of its characters",
        params: &[
            req("node", NODE, "the text layer"),
            req(
                "attrs",
                ParamKind::Model("[CharAttr]"),
                "the attributes to set",
            ),
            opt(
                "start",
                INDEX,
                "the first character styled, counted from 0; 0 unless given",
            ),
            opt(
                "end",
                INDEX,
                "the character after the last styled; the end unless given",
            ),
        ],
        run: |ctx, a| {
            let attrs = a.model::<Vec<CharAttr>>("attrs")?;
            let (t, content) = text_of(ctx, a)?;
            let (range, whole) = char_range(a, content)?;
            Ok(Done::edit(if whole {
                build::char_defaults(&t, range, attrs).0
            } else {
                build::char_runs(&t, range, attrs)
            }))
        },
    },
    Verb {
        name: "style_paragraphs",
        doc: "Set paragraph attributes on a text layer, over all of it or the paragraphs a range of its characters reaches",
        params: &[
            req("node", NODE, "the text layer"),
            req(
                "attrs",
                ParamKind::Model("[ParaAttr]"),
                "the attributes to set",
            ),
            opt(
                "start",
                INDEX,
                "the first character of the range, counted from 0; 0 unless given",
            ),
            opt(
                "end",
                INDEX,
                "the character after the range; the end unless given",
            ),
        ],
        run: |ctx, a| {
            let attrs = a.model::<Vec<ParaAttr>>("attrs")?;
            let (t, content) = text_of(ctx, a)?;
            let (range, whole) = char_range(a, content)?;
            Ok(Done::edit(if whole {
                build::para_defaults(&t, range, attrs).0
            } else {
                build::para_runs(&t, content, range, attrs)
            }))
        },
    },
    // --- paint --------------------------------------------------------------
    Verb {
        name: "set_fills",
        doc: "Replace layers' fills (colours, gradients, images), bottom first, as the Fill card writes a selection: a group's reach its shapes",
        params: &[
            req("nodes", NODES, "the layers"),
            req("fills", ParamKind::Model("[Fill]"), "the fills"),
        ],
        run: |ctx, a| {
            let ids = painted(ctx, a)?;
            let fills = a.model::<Vec<Fill>>("fills")?;
            Ok(Done::edit(match build::shared_fills(ctx.doc, &ids) {
                build::PaintShown::List(anchor) => {
                    let edited = rekeyed(ctx, &anchor, fills);
                    build::retarget_fills_all(ctx.doc, &ids, &anchor, &edited, ctx.ids)
                }
                build::PaintShown::Mixed => build::set_fills_all(ctx.doc, &ids, &fills, ctx.ids),
            }))
        },
    },
    Verb {
        name: "set_strokes",
        doc: "Replace layers' strokes, bottom first, as the Stroke card writes a selection: a group's reach its shapes",
        params: &[
            req("nodes", NODES, "the layers"),
            req("strokes", ParamKind::Model("[Stroke]"), "the strokes"),
        ],
        run: |ctx, a| {
            let ids = painted(ctx, a)?;
            let strokes = a.model::<Vec<Stroke>>("strokes")?;
            Ok(Done::edit(match build::shared_strokes(ctx.doc, &ids) {
                build::PaintShown::List(anchor) => {
                    let edited = rekeyed(ctx, &anchor, strokes);
                    build::retarget_strokes_all(ctx.doc, &ids, &anchor, &edited, ctx.ids)
                }
                build::PaintShown::Mixed => {
                    build::set_strokes_all(ctx.doc, &ids, &strokes, ctx.ids)
                }
            }))
        },
    },
    Verb {
        name: "set_effects",
        doc: "Replace layers' effects, shadows and blurs in order, on the outermost of them",
        params: &[
            req("nodes", NODES, "the layers"),
            req("effects", ParamKind::Model("[Effect]"), "the effects"),
        ],
        run: |ctx, a| {
            let ids = existing_layers(ctx, a)?;
            let effects = a.model::<Vec<Effect>>("effects")?;
            Ok(Done::edit(build::set_effects_all(
                ctx.doc, &ids, &effects, ctx.ids,
            )))
        },
    },
    Verb {
        name: "merge_colours",
        doc: "Make near-identical colours one, as Colour review merges a group: every place painted in one of `colours` takes `into`",
        params: &[
            req(
                "colours",
                ParamKind::Model("[Hex]"),
                "the colours to replace, `#rrggbbaa` as `review_colours` writes them",
            ),
            req(
                "into",
                TEXT,
                "the colour they become, `#rrggbbaa`, one the document uses; a variable's colour binds them to it",
            ),
            opt(
                "nodes",
                NODES,
                "only the layers in and under these; every page unless given",
            ),
        ],
        run: |ctx, a| {
            use crate::color_review::{self, Group};
            let parse = |param: &'static str, s: &str| {
                peniko::color::parse_color(s.trim())
                    .map(|c| {
                        c.to_alpha_color::<peniko::color::Srgb>()
                            .to_rgba8()
                            .to_u8_array()
                    })
                    .map_err(|_| a.invalid(param, format!("`{s}` is no colour")))
            };
            let into = parse("into", a.text("into").unwrap_or_default())?;
            let from = a
                .model::<Vec<String>>("colours")?
                .iter()
                .map(|s| parse("colours", s))
                .collect::<Result<Vec<_>, _>>()?;
            let scope = match a.get("nodes") {
                Some(_) => Some(a.nodes("nodes")?),
                None => None,
            };
            let roots = color_review::roots(ctx.doc, scope.as_deref());
            let census = color_review::census(ctx.doc, &roots);
            let raw = |i: usize| {
                census[i]
                    .is_raw()
                    .then(|| census[i].color.to_rgba8().to_u8_array())
            };
            // A variable that is this colour first, as Colour review's default
            // target: the raw paints are then bound to it.
            let bound: Vec<usize> = (0..census.len())
                .filter(|i| {
                    !census[*i].is_raw() && census[*i].color.to_rgba8().to_u8_array() == into
                })
                .collect();
            let target = match bound.as_slice() {
                [one] => *one,
                [] => (0..census.len())
                    .find(|i| raw(*i) == Some(into))
                    .ok_or_else(|| a.invalid("into", "no paint in scope is this colour"))?,
                _ => {
                    return Err(a.invalid(
                        "into",
                        "several variables are this colour; bind the paints with `bind_variable`",
                    ));
                }
            };
            let mut members = vec![target];
            members.extend(
                (0..census.len())
                    .filter(|i| *i != target && raw(*i).is_some_and(|b| from.contains(&b))),
            );
            if members.len() < 2 {
                return Err(VerbError::Nothing("none of the colours is used in scope"));
            }
            let merged = color_review::merge(ctx.doc, &roots, &census, &Group { members }, 0);
            if merged.uses == 0 && merged.locked > 0 {
                return Err(VerbError::Nothing("every place is on a locked layer"));
            }
            if merged.uses == 0 {
                return Err(VerbError::Nothing(
                    "every place is that colour already where no variable binds: a text's runs or colour, a gradient stop, a shadow",
                ));
            }
            let locked = match merged.locked {
                0 => String::new(),
                n => format!(
                    "; {} on locked layers left alone",
                    plural(n, "place", "places")
                ),
            };
            Ok(Done::edit(merged.tx).saying(format!(
                "Merged {} on {}{locked}",
                plural(merged.uses, "place", "places"),
                plural(merged.layers, "layer", "layers"),
            )))
        },
    },
    Verb {
        name: "set_exports",
        doc: "Replace layers' export settings",
        params: &[
            req("nodes", NODES, "the layers"),
            req("exports", ParamKind::Model("[ExportSpec]"), "the settings"),
        ],
        run: |ctx, a| {
            let exports = a.model::<Vec<ExportSpec>>("exports")?;
            let mut ops = Vec::new();
            // One id for a row past any layer's end, shared across the layers, as
            // the Export card gives a row added over a selection.
            let mut fresh = PastTheEnd::default();
            for id in existing_layers(ctx, a)? {
                let own = node(ctx, id)?.exports().to_vec();
                let exports = rekey_by_position(&own, exports.clone(), &mut fresh, ctx.ids);
                ops.push(Operation::SetExports { id, exports });
            }
            Ok(Done::ops(ops))
        },
    },
    Verb {
        name: "set_reactions",
        doc: "Replace layers' prototype reactions: a trigger and the actions it runs, in order",
        params: &[
            req("nodes", NODES, "the layers"),
            req(
                "reactions",
                ParamKind::Model("[Reaction]"),
                "every reaction; an item without an `id` is new, and a transition left out is the page's. \
                 An expression names a variable `{Count}`, a table `{Products.rows}`, `{Products.first.Price}` \
                 (`first`, `last`, `opened`, a row number) or `{Products.Price.sum}` (`min`, `max`, `avg`), \
                 and in a row action `{row.Price}`; text is quoted",
            ),
        ],
        run: |ctx, a| {
            let mut ops = Vec::new();
            for id in existing_layers(ctx, a)? {
                let base = page_transition(ctx.doc, id);
                let ids = &mut *ctx.ids;
                let mut reactions: Vec<Keyed<crate::prototype::Reaction>> =
                    keyed_arg(a, "reactions", "[Reaction]", |v| {
                        reactions_in(v, &base, &mut || ids.mint_item())
                    })?;
                for (i, r) in reactions.iter_mut().enumerate() {
                    let at = format!("reactions[{i}].actions");
                    exprs_stored(ctx.doc, &mut r.value.actions, &at)
                        .and_then(|()| targets_checked(ctx.doc, &r.value.actions, &at))
                        .map_err(|why| a.invalid("reactions", why))?;
                }
                if let Some(twice) = crate::item::first_duplicate(&reactions) {
                    let why = format!("two reactions share the id `{}`", twice.0.to_wire());
                    return Err(a.invalid("reactions", why));
                }
                let own = node(ctx, id)?.reactions().to_vec();
                let ids = &mut *ctx.ids;
                crate::prototype::moved_actions_reminted(&own, &mut reactions, &mut || {
                    ids.mint_item()
                });
                for (i, r) in reactions.iter().enumerate() {
                    crate::prototype::check(std::slice::from_ref(r))
                        .map_err(|e| a.invalid("reactions", format!("at `reactions[{i}]`: {e}")))?;
                }
                // Each reaction passed alone: what is left is an action id two
                // reactions share.
                crate::prototype::check(&reactions)
                    .map_err(|e| a.invalid("reactions", e.to_string()))?;
                ops.push(Operation::SetReactions { id, reactions });
            }
            Ok(Done::ops(ops))
        },
    },
    Verb {
        name: "copy_reactions",
        doc: "Add a layer's reactions after each layer's own, as Paste interactions does: what was the source's own instance or snap point becomes each layer's",
        params: &[
            req("nodes", NODES, "the layers that take them"),
            req("from", NODE, "the layer whose reactions are copied"),
        ],
        run: |ctx, a| {
            let from = a.layer(ctx.doc, "from")?;
            let copied: Vec<crate::prototype::Reaction> = ctx
                .doc
                .get(from)
                .map(|n| n.reactions().iter().map(|k| k.value.clone()).collect())
                .unwrap_or_default();
            if copied.is_empty() {
                return Err(VerbError::Nothing("the layer has no reactions to copy"));
            }
            let mut ops = Vec::new();
            let origin = crate::prototype::CopiedFrom::of(ctx.doc, from);
            for id in existing_layers(ctx, a)? {
                if id == from {
                    continue;
                }
                let ids = &mut *ctx.ids;
                let reactions =
                    crate::prototype::pasted(ctx.doc, &origin, id, &copied, &mut || {
                        ids.mint_item()
                    })
                    .map_err(|why| a.invalid("nodes", why.in_doc(ctx.doc)))?;
                ops.push(Operation::SetReactions { id, reactions });
            }
            Ok(Done::ops(ops))
        },
    },
    Verb {
        name: "set_play",
        doc: "Replace layers' prototype settings: a frame's scroll and snap, a child's pin, an input, how a layer follows a drag, and a scroll effect",
        params: &[
            req("nodes", NODES, "the layers"),
            req("play", ParamKind::Model("LayerPlay"), "the settings"),
        ],
        run: |ctx, a| {
            let ids = &mut *ctx.ids;
            let play: crate::prototype::LayerPlay = keyed_arg(a, "play", "LayerPlay", |v| {
                play_in(v, &mut || ids.mint_item());
                Ok(())
            })?;
            let ops = existing_layers(ctx, a)?
                .into_iter()
                .map(|id| Operation::SetPlay {
                    id,
                    play: play.clone(),
                })
                .collect();
            Ok(Done::ops(ops))
        },
    },
    Verb {
        name: "add_image",
        doc: "Add a picture to the document for a fill to show; answers its image id",
        params: &[req(
            "bytes",
            TEXT,
            "the PNG, JPEG, WebP or GIF file, in base64",
        )],
        run: |ctx, a| {
            use base64::Engine as _;
            let bytes = base64::engine::general_purpose::STANDARD
                .decode(a.text("bytes").unwrap_or_default())
                .map_err(|e| a.invalid("bytes", e))?;
            let format = crate::foreign::sniff(&bytes)
                .ok_or_else(|| a.invalid("bytes", "not a PNG, JPEG, WebP or GIF file"))?;
            let sink = ctx
                .images
                .as_mut()
                .ok_or(VerbError::Nothing("this caller cannot read pictures"))?;
            let (id, entry) = sink
                .place(bytes, format)
                .ok_or_else(|| a.invalid("bytes", "the picture does not decode"))?;
            // A picture already in the table is the same bytes: one entry, two users.
            let ops = if ctx.doc.has_image(&id) {
                Vec::new()
            } else {
                vec![Operation::AddImage {
                    id: id.clone(),
                    entry,
                }]
            };
            Ok(Done::making(ops, id.0))
        },
    },
    Verb {
        name: "replace_image",
        doc: "Show another picture in a layer's fill, keeping how the old one was framed",
        params: &[
            req("node", NODE, "the layer"),
            req("image", TEXT, "the picture, by the id `add_image` answered"),
            opt(
                "index",
                INDEX,
                "the fill, 0 the bottom; its first picture fill unless given",
            ),
        ],
        run: |ctx, a| {
            let id = a.node("node")?;
            let image = crate::image::ImageId(a.text("image").unwrap_or_default().to_owned());
            if !ctx.doc.has_image(&image) {
                return Err(a.invalid("image", "the document holds no such picture; add it first"));
            }
            let fills = &node(ctx, id)?.paint().fills;
            let index = a.index("index").unwrap_or_else(|| {
                fills
                    .iter()
                    .position(|f| matches!(f.brush.literal(), Some(crate::Brush::Image(_))))
                    .unwrap_or(0)
            });
            build::replace_image_fill(ctx.doc, id, index, &image, None)
                .map(Done::edit)
                .ok_or_else(|| a.invalid("index", "the layer has no fill there"))
        },
    },
    Verb {
        name: "trace_image",
        doc: "Trace a picture layer as shown into filled paths, in a Trace group just above it; answers the group",
        params: &[
            req("node", NODE, "the picture layer"),
            opt(
                "colours",
                INDEX,
                "trace in this many colours, 2 to 16; black and white unless given",
            ),
            opt(
                "threshold",
                ParamKind::Number,
                "in black and white, how dark a pixel turns black, 0 to 1; 0.5 unless given",
            ),
            opt(
                "detail",
                ParamKind::Number,
                "how closely the curves follow the pixels, 0 to 1; 0.6 unless given",
            ),
            opt(
                "corners",
                ParamKind::Number,
                "how readily a turn stays a corner, 0 to 1; 0.5 unless given",
            ),
            opt(
                "speck",
                ParamKind::Number,
                "regions smaller than this many units across merge into their surroundings; 4 unless given",
            ),
            opt(
                "keep",
                BOOL,
                "keep the picture under the trace; true unless given",
            ),
        ],
        run: |ctx, a| {
            use crate::trace::{Mode, Settings};
            let id = a.layer(ctx.doc, "node")?;
            if node(ctx, id)?.paint().image_fill().is_none() {
                return Err(a.invalid("node", "not a picture: only a layer showing one traces"));
            }
            if let Some(why) = crate::trace::refusal(ctx.doc, id) {
                return Err(VerbError::Nothing(why));
            }
            let unit = |name: &'static str, default: f64| match a.number(name) {
                Some(n) if !(0.0..=1.0).contains(&n) => Err(a.invalid(name, "expected 0 to 1")),
                n => Ok(n.unwrap_or(default)),
            };
            let defaults = Settings::default();
            let mode = match a.index("colours") {
                Some(n) if !(2..=crate::trace::MAX_COLOURS).contains(&n) => {
                    return Err(a.invalid("colours", "expected 2 to 16"));
                }
                Some(count) => Mode::Colours { count },
                None => Mode::BlackAndWhite {
                    threshold: unit("threshold", 0.5)?,
                },
            };
            let speck = a.number("speck").unwrap_or(defaults.speck);
            if speck < 0.0 {
                return Err(a.invalid("speck", "expected 0 or more"));
            }
            let shot = ctx
                .images
                .as_mut()
                .and_then(|sink| sink.shoot(ctx.doc, ctx.res, id))
                .ok_or(VerbError::Nothing(
                    "this caller cannot render the picture to trace it",
                ))?;
            // `speck` is in units; the trace counts it in the raster's pixels.
            let settings = Settings {
                mode,
                detail: unit("detail", defaults.detail)?,
                corners: unit("corners", defaults.corners)?,
                speck: speck * shot.scale,
            };
            let traced =
                crate::trace::trace(&crate::trace::label(shot.bitmap(), &settings), &settings);
            let keep = a.bool("keep").unwrap_or(true);
            let (tx, group) = crate::trace::place(
                ctx.doc, ctx.res, ctx.ids, id, &traced, shot.frame, shot.scale, keep,
            )
            .map_err(VerbError::Nothing)?;
            Ok(Done::made(tx, vec![group]).saying(format!(
                "Traced into {}",
                plural(traced.paths(), "path", "paths")
            )))
        },
    },
    Verb {
        name: "fix_contrast",
        doc: "Give layers whose contrast fails the nearest colour that passes, or a colour or variable you name",
        params: &[
            req("nodes", NODES, "text layers and small shapes"),
            opt(
                "level",
                ParamKind::Choice(LEVELS),
                "the standard: `aa` unless given, `aaa` or `apca`",
            ),
            opt(
                "colour",
                ParamKind::Model("Color"),
                "the colour the failing paints take; the nearest passing one unless given",
            ),
            opt(
                "variable",
                ParamKind::Id("variable"),
                "a colour variable the failing paints are bound to instead",
            ),
        ],
        run: |ctx, a| {
            use crate::contrast::{Fix, Level};
            let level = match a.choice("level") {
                Some("aaa") => Level::Aaa,
                Some("apca") => Level::Apca,
                _ => Level::Aa,
            };
            let named = match (
                a.model_opt::<peniko::Color>("colour")?,
                a.id_opt::<VarId>("variable")?,
            ) {
                (Some(_), Some(_)) => {
                    return Err(a.invalid("variable", "give a colour or a variable, not both"));
                }
                (Some(c), None) => Some(Fix::Colour(c)),
                (None, Some(v)) => {
                    let var = ctx
                        .doc
                        .variables()
                        .get(v)
                        .ok_or_else(|| a.invalid("variable", "no variable has that id"))?;
                    if var.kind != VarKind::Color {
                        return Err(a.invalid("variable", "not a colour variable"));
                    }
                    Some(Fix::Var(v))
                }
                (None, None) => None,
            };
            let nodes = a.nodes("nodes")?;
            for id in &nodes {
                node(ctx, *id)?;
            }
            let probe = ctx
                .contrast
                .as_mut()
                .ok_or(VerbError::Nothing("this caller cannot measure contrast"))?;
            let measured = probe.measure(ctx.doc, &nodes, level);
            // What was given and is left unfixed for a reason other than passing.
            let named_as = |id: NodeId, why: &str| {
                let name = ctx.doc.get(id).map_or("", |n| n.name());
                format!("{name} ({why})")
            };
            let mut unfixed: Vec<String> = measured
                .skipped
                .iter()
                .map(|(id, why)| named_as(*id, why))
                .collect();
            unfixed.extend(
                nodes
                    .iter()
                    .filter(|id| {
                        !measured.findings.iter().any(|f| f.id == **id)
                            && !measured.skipped.iter().any(|(s, _)| s == *id)
                    })
                    .map(|id| named_as(*id, "not a shown text layer or small shape")),
            );
            let mut ops = Vec::new();
            for f in measured.findings.iter().filter(|f| !f.passes()) {
                let fix = match named {
                    Some(fix) => fix,
                    None => match f
                        .worst()
                        .and_then(|w| crate::contrast::nearest(f, level, w.ink))
                    {
                        Some(c) => Fix::Colour(c),
                        None => {
                            unfixed.push(named_as(f.id, "no lightness at its hue passes"));
                            continue;
                        }
                    },
                };
                let new_fill = ctx.ids.mint_item();
                if let Some(tx) =
                    crate::contrast::fix_transaction(ctx.doc, f.id, f, level, fix, new_fill)
                {
                    ops.extend(tx.0);
                }
            }
            // Nothing changed is "they pass" only when every layer was measured.
            if ops.is_empty() && !unfixed.is_empty() {
                return Err(a.invalid("nodes", format!("could not fix {}", unfixed.join(", "))));
            }
            Ok(Done::ops(ops))
        },
    },
    Verb {
        name: "clean_up",
        doc: "Take away what draws nothing: hidden paint, covered fills, hidden and empty \
              layers, one-layer groups, guides; what would change a pixel is left",
        params: &[
            opt(
                "nodes",
                NODES,
                "the layers to clean, with everything under them; every page but drafts \
                 unless given",
            ),
            opt(
                "hidden_paint",
                ParamKind::Bool,
                "hidden fills, strokes, effects and grids; on unless given",
            ),
            opt(
                "covered_fills",
                ParamKind::Bool,
                "fills under an opaque solid fill; on unless given",
            ),
            opt(
                "hidden_layers",
                ParamKind::Bool,
                "hidden layers; off unless given",
            ),
            opt(
                "empty_layers",
                ParamKind::Bool,
                "empty layers; on unless given",
            ),
            opt(
                "one_layer_groups",
                ParamKind::Bool,
                "groups holding one layer; on unless given",
            ),
            opt(
                "guides",
                ParamKind::Bool,
                "ruler guides in scope; off unless given",
            ),
            opt(
                "skip_locked",
                ParamKind::Bool,
                "leave locked layers alone; on unless given",
            ),
        ],
        run: |ctx, a| {
            use crate::clean_up::{Rule, Rules, Scope};
            let scope = match a.get("nodes") {
                Some(_) => Scope::Layers(a.nodes("nodes")?),
                None => Scope::Pages(ctx.doc.gathered_pages().collect()),
            };
            let mut rules = Rules::default();
            for rule in Rule::ALL {
                if let Some(on) = a.bool(rule.key()) {
                    rules.set(rule, on);
                }
            }
            if let Some(skip) = a.bool("skip_locked") {
                rules.skip_locked = skip;
            }
            Ok(Done::edit(
                crate::clean_up::plan(ctx.doc, ctx.res, &scope, &rules).tx,
            ))
        },
    },
    Verb {
        name: "fill_content",
        doc: "Fill text layers with realistic names, addresses, companies, text, numbers and \
              dates, and avatar shapes with initials or a pattern",
        params: &[
            req(
                "nodes",
                NODES,
                "the layers to fill, with every text layer under them",
            ),
            opt(
                "locale",
                TEXT,
                "a shipped locale's tag (en-US, en-GB, de, fr, es, it, pt-BR, nl); the \
                 layers' own Language when they agree, else en-US, unless given",
            ),
            opt(
                "kind",
                ParamKind::Choice(&crate::content_fill::KIND_KEYS),
                "one kind for every text layer; each read from its name, then its text, \
                 unless given",
            ),
            opt(
                "records",
                ParamKind::Bool,
                "keep each frame's values together; on unless given",
            ),
            opt(
                "avatars",
                ParamKind::Choice(&["initials", "pattern", "none"]),
                "how shapes named as avatars are drawn; initials unless given",
            ),
            opt(
                "seed",
                INDEX,
                "the draw: one seed always fills the same values; 0 unless given",
            ),
            opt(
                "example_emails",
                ParamKind::Bool,
                "every email at example.com, which reaches no one; off unless given",
            ),
            opt(
                "table",
                TEXT,
                "a table: layers named like its columns take them, a row a record",
            ),
            opt("from_row", INDEX, "the first record's row, from 1"),
            opt("shuffled", ParamKind::Bool, "rows dealt by the seed"),
        ],
        run: |ctx, a| {
            use crate::content_fill::{self as fill, AvatarStyle, Kind, Options, TableContent};
            let nodes = a.nodes("nodes")?;
            let locale = match a.text("locale") {
                Some(tag) => fill::locale_for(tag)
                    .ok_or_else(|| a.invalid("locale", "not one of the shipped locales"))?,
                None => fill::agreed_locale(ctx.doc, &nodes)
                    .or_else(|| fill::locales().first())
                    .ok_or(VerbError::Nothing("no locale is shipped"))?,
            };
            let opts = Options {
                locale: locale.tag.clone(),
                content: a.choice("kind").and_then(Kind::from_key),
                records: a.bool("records").unwrap_or(true),
                avatars: match a.choice("avatars") {
                    Some("pattern") => Some(AvatarStyle::Pattern),
                    Some("none") => None,
                    _ => Some(AvatarStyle::Initials),
                },
                seed: a.index("seed").unwrap_or(0) as u64,
                example_emails: a.bool("example_emails").unwrap_or(false),
                table: match a.text("table") {
                    Some(t) => Some(TableContent::ByColumnName(
                        find_table(ctx.doc, t)
                            .map_err(|why| a.invalid("table", why))?
                            .id,
                    )),
                    None => None,
                },
                from_row: a.index("from_row").unwrap_or(1).saturating_sub(1),
                shuffled: a.bool("shuffled").unwrap_or(false),
                ..Options::default()
            };
            let plan = fill::plan(ctx.doc, ctx.res, &nodes, &opts);
            Ok(Done::edit(plan.transaction(
                ctx.doc,
                ctx.res,
                ctx.images.as_deref_mut(),
            )))
        },
    },
    // --- layout -------------------------------------------------------------
    Verb {
        name: "set_display",
        doc: "Make frames lay out their children as flex or grid, or stop",
        params: &[
            req("nodes", NODES, "the frames"),
            opt(
                "display",
                ParamKind::Model("Display"),
                "the layout; absent turns it off",
            ),
        ],
        run: |ctx, a| {
            let display = a.model_opt::<Display>("display")?;
            each(ctx.doc, a, |id| Operation::SetDisplay {
                id,
                display: display.clone(),
            })
        },
    },
    Verb {
        name: "set_layout_item",
        doc: "Set how layers size and place themselves in their parents' layouts",
        params: &[
            req("nodes", NODES, "the layers"),
            req(
                "item",
                ParamKind::Model("LayoutItem"),
                "their item properties; a width or height `Auto` keeps the stored size, `FitContent` hugs, {\"Percent\": 100} fills",
            ),
        ],
        run: |ctx, a| {
            let item = a.model::<LayoutItem>("item")?;
            each(ctx.doc, a, |id| Operation::SetLayoutItem { id, item })
        },
    },
    Verb {
        name: "set_insets",
        doc: "Pin layers to their frames' sides, as CSS insets and auto margins; in a flex or grid frame, out of the flow (absolute)",
        params: &[
            req("nodes", NODES, "the layers"),
            req("insets", ParamKind::Model("Insets"), "the four sides"),
        ],
        run: |ctx, a| {
            let insets = a.model::<Insets>("insets")?;
            each(ctx.doc, a, |id| Operation::SetInsets { id, insets })
        },
    },
    Verb {
        name: "set_layout_grids",
        doc: "Replace frames' layout grids, the columns and rows drawn over them to line work up against",
        params: &[
            req("nodes", NODES, "the frames"),
            req(
                "grids",
                ParamKind::Model("[LayoutGrid]"),
                "every grid, each from its `axis` with the rest as a new one has unless given; an empty list removes them",
            ),
        ],
        run: |ctx, a| {
            let grids = a.model::<Vec<crate::layout::LayoutGrid>>("grids")?;
            let mut ops = Vec::new();
            let mut fresh = PastTheEnd::default();
            for id in existing_layers(ctx, a)? {
                let n = node(ctx, id)?;
                if !matches!(n.kind(), NodeKind::Artboard { .. }) {
                    return Err(a.invalid("nodes", "a layout grid is a frame's"));
                }
                let own = n.grids().to_vec();
                let grids = rekey_by_position(&own, grids.clone(), &mut fresh, ctx.ids);
                ops.push(Operation::SetLayoutGrids { id, grids });
            }
            Ok(Done::ops(ops))
        },
    },
    // --- charts -------------------------------------------------------------
    Verb {
        name: "insert_chart",
        doc: "Make a chart of `kind` in `parent`, on its own table or the kind's sample; answers its frame",
        params: &[
            req("parent", NODE, "the frame, group or page to make it in"),
            req("kind", ParamKind::Choice(CHART_KINDS), "what it draws"),
            opt(
                "table",
                ParamKind::Model("ChartTable"),
                "its data; the kind's sample table unless given",
            ),
            opt(
                "mapping",
                ParamKind::Model("ChartMapping"),
                "which columns it reads, by id; read off the column types unless given",
            ),
            opt(
                "x",
                ParamKind::Number,
                "its frame's left edge in the parent; 0 unless given",
            ),
            opt(
                "y",
                ParamKind::Number,
                "its frame's top edge in the parent; 0 unless given",
            ),
            opt("width", ParamKind::Number, "its width; 480 unless given"),
            opt("height", ParamKind::Number, "its height; 280 unless given"),
            opt(
                "index",
                INDEX,
                "its place among the parent's children, 0 the bottom",
            ),
            opt("name", TEXT, "its name; Chart unless given"),
        ],
        run: |ctx, a| {
            let parent = a.node("parent")?;
            let count = node(ctx, parent)?.children().len();
            let kind = chart_kind(a.choice("kind").unwrap_or_default());
            let mut chart = crate::chart::Chart::sample(kind);
            if let Some(table) = a.model_opt::<crate::chart::Table>("table")? {
                chart.mapping = crate::chart::Mapping::guess(kind, &table);
                chart.table = table;
            }
            if let Some(mapping) = a.model_opt("mapping")? {
                chart.mapping = mapping;
            }
            chart_valid(a, &chart)?;
            let mut size = [480.0, 280.0];
            for (i, p) in ["width", "height"].into_iter().enumerate() {
                match a.number(p) {
                    Some(v) if v <= 0.0 || v > build::GEOMETRY_LIMIT => {
                        let why = format!(
                            "a side is more than 0 and at most {}",
                            build::GEOMETRY_LIMIT
                        );
                        return Err(a.invalid(p, why));
                    }
                    Some(v) => size[i] = v,
                    None => {}
                }
            }
            let at = kurbo::Vec2::new(a.number("x").unwrap_or(0.0), a.number("y").unwrap_or(0.0));
            let id = ctx.ids.mint();
            let mut ops = vec![Operation::CreateNode {
                id,
                parent,
                index: a.index("index").unwrap_or(count).min(count),
                kind: NodeKind::Artboard {
                    size: kurbo::Size::new(size[0], size[1]),
                    corner_radii: Default::default(),
                    corner_smoothing: 0.0,
                },
                transform: Some(Affine::translate(at)),
                name: Some(a.text("name").unwrap_or("Chart").to_owned()),
            }];
            // On white, as the chart tool makes one, but none inside a painted frame.
            if let Some(ground) = crate::chart::starting_ground(ctx.doc, parent) {
                ops.push(Operation::SetFills {
                    id,
                    fills: crate::item::keyed_by_position([crate::node::Fill {
                        brush: crate::Brush::Solid(ground).into(),
                        visible: true,
                    }]),
                });
            }
            ops.push(Operation::SetChart {
                id,
                chart: Some(Box::new(chart)),
            });
            let tx = Transaction(ops);
            Ok(Done::made(tx, vec![id]))
        },
    },
    Verb {
        name: "set_chart",
        doc: "Change a chart's kind, patch its mapping or options, colour or style its series and marks, or set its highlight or references; its parts follow. A switch to or from a Sankey re-reads the mapping off the table's columns before `mapping` patches it",
        params: &[
            req("node", NODE, "the chart"),
            opt("kind", ParamKind::Choice(CHART_KINDS), "what it draws"),
            opt(
                "mapping",
                ParamKind::Model("ChartMapping"),
                "the roles to change, by column id; a null clears one",
            ),
            opt(
                "options",
                ParamKind::Model("ChartOptions"),
                "the options to change, nested as the chart holds them; a null resets one",
            ),
            opt(
                "colours",
                ParamKind::Model("ChartColours"),
                "by series, slice, group or node name: \"#rrggbb\" or a colour variable",
            ),
            opt(
                "series",
                ParamKind::Model("ChartSeries"),
                "by series name: `dash` Solid|Dashed|Dotted, `point` Circle|Square|Diamond|Triangle (null: the chart's), `visible`",
            ),
            opt(
                "highlight",
                ParamKind::Model("ChartHighlight"),
                "categories or series by name, the rest muted; null clears",
            ),
            opt(
                "references",
                ParamKind::Model("[ChartReference]"),
                "every reference line and band, at most 16",
            ),
        ],
        run: |ctx, a| {
            let (id, chart) = chart_of(ctx, a)?;
            if [
                "kind",
                "mapping",
                "options",
                "colours",
                "series",
                "highlight",
                "references",
            ]
            .iter()
            .all(|p| a.get(p).is_none())
                && !a.nulled("highlight")
            {
                return Err(VerbError::Nothing(
                    "`set_chart` changes `kind`, `mapping`, `options`, `colours`, `series`, `highlight` or `references`",
                ));
            }
            let chart = chart_set(ctx.doc, a, chart)?;
            Ok(Done::ops(vec![Operation::SetChart {
                id,
                chart: Some(Box::new(chart)),
            }]))
        },
    },
    Verb {
        name: "set_chart_data",
        doc: "Point a chart at a table of the document's (`data`), or give it new cells (`table`), keeping the mapping and series colours of each column whose name survives. On a chart reading a table the cells go into that table unless `write` is `chart`; a table other lists or charts read too needs `write`",
        params: &[
            req("node", NODE, "the chart"),
            opt(
                "table",
                ParamKind::Model("ChartTable"),
                "its new data; column ids are its own",
            ),
            opt(
                "data",
                TEXT,
                "a table of the document's, by id or name, for the chart to read",
            ),
            opt(
                "write",
                ParamKind::Choice(&["table", "chart"]),
                "with `table` on a chart reading a table: `table` writes the cells into it for every reader, `chart` gives this chart its own copy",
            ),
        ],
        run: tables::set_chart_data,
    },
    Verb {
        name: "flatten_chart",
        doc: "Drop charts' descriptions, leaving their parts as ordinary layers where they are",
        params: &[req("nodes", NODES, "the charts")],
        run: |ctx, a| {
            let ops = a
                .layers(ctx.doc, "nodes")?
                .into_iter()
                .map(|id| {
                    crate::chart::flatten_ops(ctx.doc, id).ok_or_else(|| {
                        a.invalid("nodes", format!("`{}` is not a chart", id.to_wire()))
                    })
                })
                .collect::<Result<Vec<_>, _>>()?;
            Ok(Done::ops(ops.into_iter().flatten().collect()))
        },
    },
    Verb {
        name: "add_table",
        doc: "Add a table to the document, as *Data tables*' *New table*: named `Table N` unless given, a taken name numbered past; with no `columns`, `Column 1` and one empty row. Answers its id",
        params: &[
            opt("name", TEXT, "its name"),
            opt(
                "columns",
                ParamKind::Model("[ColumnName]"),
                "its column names, in order",
            ),
            opt(
                "rows",
                ParamKind::Model("TableRows"),
                "its rows, one text cell per column",
            ),
        ],
        run: tables::add_table,
    },
    Verb {
        name: "edit_table",
        doc: "Edit a table's cells, rows and columns in one step; every list and chart reading it follows. Rows count from 1 as the table stands before the call; `set` and `add` name columns as they stand after `columns`. Refused on a linked table",
        params: &[
            req("table", TEXT, "the table, by id or name"),
            opt(
                "set",
                ParamKind::Model("[TableCell]"),
                "cells to write: row, column (name or id), text",
            ),
            opt(
                "add",
                ParamKind::Model("[TableRow]"),
                "rows to add, each by column name; a column left out is empty",
            ),
            opt(
                "at",
                INDEX,
                "the row the added rows start at; the end unless given",
            ),
            opt(
                "delete",
                ParamKind::Model("[RowNumber]"),
                "the rows to delete",
            ),
            opt(
                "columns",
                ParamKind::Model("TableColumns"),
                "columns to add, rename (old to new) and delete, by name",
            ),
        ],
        run: tables::edit_table,
    },
    Verb {
        name: "set_table",
        doc: "Replace a table's columns and rows, keeping its id; a column keeps its id, and the fills reading it, where the old table has one of its name. Refused on a linked table",
        params: &[
            req("table", TEXT, "the table, by id or name"),
            req(
                "columns",
                ParamKind::Model("[ColumnName]"),
                "its column names, in order",
            ),
            req(
                "rows",
                ParamKind::Model("TableRows"),
                "its rows, one text cell per column",
            ),
        ],
        run: tables::set_table,
    },
    Verb {
        name: "rename_table",
        doc: "Rename a table; a name another table has is numbered past it",
        params: &[
            req("table", TEXT, "the table, by id or name"),
            req("name", TEXT, "its new name"),
        ],
        run: tables::rename_table,
    },
    Verb {
        name: "delete_table",
        doc: "Delete a table no list or chart reads; a linked table's file is untouched",
        params: &[req("table", TEXT, "the table, by id or name")],
        run: tables::delete_table,
    },
    Verb {
        name: "unlink_table",
        doc: "Drop a table's linked file, keeping its cells, so it can be edited",
        params: &[req("table", TEXT, "the table, by id or name")],
        run: tables::unlink_table,
    },
    Verb {
        name: "make_list",
        doc: "Make an instance a list over a table, as *Make a list from data*: wrapped in a frame that repeats it once per row, each copy's properties filled from its row",
        params: &[
            req("node", NODE, "the instance"),
            req("table", TEXT, "the table, by id or name"),
            opt(
                "fill",
                ParamKind::Model("ListFill"),
                "property name to column name; the columns named as the properties unless given",
            ),
        ],
        run: tables::make_list,
    },
    Verb {
        name: "set_list",
        doc: "Change a list's table, rows, fills, filter or sort, as the List card does; columns are named in the table it reads after the call",
        params: &[
            req("node", NODE, "the list's frame or its template"),
            opt(
                "table",
                TEXT,
                "the table it reads, by id or name; each fill keeps the column of its name",
            ),
            opt(
                "rows",
                ParamKind::Model("ListRows"),
                "\"all\", {\"first\": n}, {\"row\": n} or \"opened\"",
            ),
            opt(
                "fill",
                ParamKind::Model("ListFill"),
                "property name to column name, replacing every fill",
            ),
            opt(
                "filter",
                ParamKind::Model("ListFilter"),
                "tests on columns, all passing unless `any`; a value may read `{variable}`",
            ),
            opt(
                "sort",
                ParamKind::Model("ListSort"),
                "the columns its rows are sorted by, at most two",
            ),
        ],
        run: tables::set_list,
    },
    Verb {
        name: "detach_list",
        doc: "Detach lists: each keeps its rows as plain instances and stops reading its table",
        params: &[req("nodes", NODES, "the lists' frames")],
        run: |ctx, a| {
            let nodes = a.nodes("nodes")?;
            if let Some(not) = nodes
                .iter()
                .find(|id| ctx.doc.get(**id).is_none_or(|n| n.list().is_none()))
            {
                let why = format!("{} is no list's frame", not.to_wire());
                return Err(a.invalid("nodes", why));
            }
            Ok(Done::ops(crate::list::detach(ctx.doc, &nodes).0))
        },
    },
];

/// The chart kinds by the word a verb takes, in [`crate::chart::ChartKind::ALL`]'s order.
const CHART_KINDS: &[&str] = &[
    "column",
    "bar",
    "line",
    "area",
    "pie",
    "scatter",
    "sankey",
    "waterfall",
    "radar",
];

/// The chart kind a [`CHART_KINDS`] word names; Column for none.
fn chart_kind(word: &str) -> crate::chart::ChartKind {
    CHART_KINDS
        .iter()
        .position(|w| *w == word)
        .map_or(crate::chart::ChartKind::Column, |i| {
            crate::chart::ChartKind::ALL[i]
        })
}

/// The chart the `node` argument names and a copy of its description.
fn chart_of(ctx: &Ctx<'_>, a: &Args<'_>) -> Result<(NodeId, crate::chart::Chart), VerbError> {
    let id = a.node("node")?;
    let chart = node(ctx, id)?
        .chart()
        .ok_or_else(|| a.invalid("node", "not a chart"))?;
    Ok((id, chart.clone()))
}

/// `chart` with `set_chart`'s arguments `a` written over it, in the verb's order:
/// the kind, the mapping and options patched, the colours, the highlight and the
/// references, checked against the chart as it then stands.
fn chart_set(
    doc: &Document,
    a: &Args<'_>,
    mut chart: crate::chart::Chart,
) -> Result<crate::chart::Chart, VerbError> {
    if let Some(word) = a.choice("kind") {
        chart = chart.with_kind(chart_kind(word));
    }
    if let Some(patch) = a.get("mapping") {
        chart.mapping = patched(a, "mapping", &chart.mapping, patch)?;
    }
    if let Some(patch) = a.loose("options") {
        let palette = chart.options.palette;
        chart.options = patched(a, "options", &chart.options, &patch)?;
        // A null sets the default back, which adds a final total; an empty
        // name is how a caller turns it off.
        let w = &mut chart.options.waterfall;
        if w.final_total
            .as_deref()
            .is_some_and(|n| n.trim().is_empty())
        {
            w.final_total = None;
        }
        // A palette pick puts every colour back on it, as the card's does, but
        // keeps a *From variables* group the same patch names.
        if chart.options.palette != palette {
            let group = std::mem::take(&mut chart.options.palette_group);
            chart = crate::chart::with_palette(&chart, chart.options.palette);
            if patch.get("palette_group").is_some() {
                chart.options.palette_group = group;
            }
        }
    }
    if let Some(Value::Object(colours)) = a.get("colours") {
        for (mark, colour) in colours {
            chart = chart_colour(doc, a, &chart, mark, colour)?;
        }
    }
    if let Some(Value::Object(series)) = a.get("series") {
        for (mark, style) in series {
            chart = chart_series_style(a, &chart, mark, style)?;
        }
    }
    // Checked against the chart as the call leaves it, kind included.
    if a.nulled("highlight") {
        chart.highlight = Default::default();
    }
    if a.get("highlight").is_some() {
        chart.highlight = a.model("highlight")?;
        chart_names_known(a, &chart)?;
    }
    if a.get("references").is_some() {
        chart.references = a.model("references")?;
    }
    chart_valid(a, &chart)?;
    Ok(chart)
}

/// The parts of a chart the `chart` value holds ([`chart_value`]), in order.
const CHART_VALUE_KEYS: [&str; 8] = [
    "kind",
    "mapping",
    "options",
    "colours",
    "series",
    "highlight",
    "references",
    "style",
];

/// Chart `id`'s settings in the shapes `set_chart` takes, for a script's `chart`
/// property: `kind` as its word, `mapping` by column id, `options` as the patch
/// from a new chart's, `colours` for each series or named mark with its own,
/// `series` for each series dashed, marked or hidden, `highlight` and
/// `references` when set, and `style`, the chart style it is
/// bound to, by name. Colours are `"#rrggbb"`, a bound mark colour its
/// variable's name. `None` on a layer that is not a chart. [`chart_whole`]
/// writes it back.
pub fn chart_value(doc: &Document, id: NodeId) -> Option<Value> {
    let node = doc.get(id)?;
    let chart = node.chart()?;
    let mut out = Map::new();
    let kind = crate::chart::ChartKind::ALL
        .iter()
        .position(|k| *k == chart.kind)
        .map_or("column", |i| CHART_KINDS[i]);
    out.insert("kind".into(), Value::from(kind));
    out.insert(
        "mapping".into(),
        serde_json::to_value(&chart.mapping).unwrap_or_default(),
    );
    let own = chart_short(&serde_json::to_value(&chart.options).unwrap_or_default());
    let base =
        chart_short(&serde_json::to_value(crate::chart::Options::default()).unwrap_or_default());
    if let Some(options) = differs_from(&own, &base) {
        out.insert("options".into(), options);
    }
    let colours = chart_colours_value(doc, chart);
    if !colours.is_empty() {
        out.insert("colours".into(), Value::Object(colours));
    }
    let series = chart_series_value(chart);
    if !series.is_empty() {
        out.insert("series".into(), Value::Object(series));
    }
    if chart.highlight != crate::chart::Highlight::default() {
        let h = serde_json::to_value(&chart.highlight).unwrap_or_default();
        out.insert("highlight".into(), chart_short(&h));
    }
    if !chart.references.is_empty() {
        let r = serde_json::to_value(&chart.references).unwrap_or_default();
        out.insert("references".into(), chart_short(&r));
    }
    if let Some(style) = node.styles().chart.and_then(|s| doc.styles().get(s)) {
        out.insert("style".into(), Value::from(style.name.clone()));
    }
    Some(Value::Object(out))
}

/// The marks `chart` colours of its own, by the name `set_chart`'s `colours`
/// takes: a series by its column's name, or on a chart coloured by name each
/// named mark. A mark it no longer draws keeps its colour unread.
fn chart_colours_value(doc: &Document, chart: &crate::chart::Chart) -> Map<String, Value> {
    let mut out = Map::new();
    let mut put = |name: String, fill: &crate::variable::BrushSource| {
        out.insert(name, chart_colour_value(doc, fill));
    };
    match crate::chart::named_marks(chart) {
        Some(marks) => {
            for style in &chart.nodes {
                if let Some(fill) = &style.fill
                    && marks.iter().any(|(n, _)| *n == style.name)
                {
                    put(style.name.clone(), fill);
                }
            }
        }
        None => {
            for column in &chart.mapping.series {
                let fill = chart.series.iter().find(|s| s.column == *column);
                if let Some(fill) = fill.and_then(|s| s.fill.as_ref())
                    && let Some(name) = series_key(chart, *column)
                {
                    put(name, fill);
                }
            }
        }
    }
    out
}

/// One mark's colour as `set_chart`'s `colours` takes it: a plain colour as
/// `"#rrggbb"`, a colour variable bound whole by its name (by its id when the
/// name is not its alone), anything else in the model's own shape.
fn chart_colour_value(doc: &Document, fill: &crate::variable::BrushSource) -> Value {
    use crate::variable::BrushSource;
    match fill {
        BrushSource::Literal(crate::Brush::Solid(c)) => {
            if let Some(hex) = loose::nearest_hex_of(*c) {
                return Value::from(hex);
            }
        }
        BrushSource::Bound { var, alpha } if *alpha == 1.0 => {
            let id = serde_json::to_value(var).unwrap_or_default();
            return match doc.variables().get(*var) {
                Some(v) if id_named(doc, "variable", &v.name).ok() == Some(id.clone()) => {
                    Value::from(v.name.clone())
                }
                _ => id,
            };
        }
        _ => {}
    }
    serde_json::to_value(fill).unwrap_or_default()
}

/// `v`, a chart value in the model's own shape, with each plain colour under one
/// of [`CHART_COLOUR_KEYS`] written `"#rrggbb"`, at any depth: [`chart_hex`]'s
/// inverse.
fn chart_short(v: &Value) -> Value {
    fn brush(v: &mut Value) {
        if let Ok(crate::variable::BrushSource::Literal(crate::Brush::Solid(c))) =
            serde_json::from_value(v.clone())
            && let Some(hex) = loose::nearest_hex_of(c)
        {
            *v = Value::from(hex);
        }
    }
    fn walk(v: &mut Value) {
        match v {
            Value::Object(m) => {
                for (k, field) in m.iter_mut() {
                    if CHART_COLOUR_KEYS.contains(&k.as_str()) {
                        match &mut *field {
                            Value::Array(items) => items.iter_mut().for_each(brush),
                            other => brush(other),
                        }
                    }
                    walk(field);
                }
            }
            Value::Array(items) => items.iter_mut().for_each(walk),
            _ => {}
        }
    }
    let mut v = v.clone();
    walk(&mut v);
    v
}

/// The parts of `v` that differ from `base`, as a merge patch onto `base`: two
/// objects with the same keys (one struct, or one enum variant) compared key by
/// key, anything else whole. `None` when nothing differs.
fn differs_from(v: &Value, base: &Value) -> Option<Value> {
    match (v, base) {
        (Value::Object(own), Value::Object(b))
            if own.len() == b.len() && own.keys().all(|k| b.contains_key(k)) =>
        {
            let changed: Map<String, Value> = own
                .iter()
                .filter_map(|(k, x)| differs_from(x, &b[k]).map(|x| (k.clone(), x)))
                .collect();
            (!changed.is_empty()).then_some(Value::Object(changed))
        }
        _ => (v != base).then(|| v.clone()),
    }
}

/// The edit setting chart `id` to `value`, a table in [`chart_value`]'s shape,
/// whole: an option, colour, series style, highlight, reference or style it leaves out goes
/// back to a new chart's, while a `kind` or `mapping` left out is kept, so one
/// chart's look copies onto another that draws its own data. Checked as
/// `set_chart` checks its arguments; the refusal names the part at fault.
pub fn chart_whole(doc: &Document, id: NodeId, value: &Value) -> Result<Vec<Operation>, String> {
    use crate::chart::Options;
    let Value::Object(given) = value else {
        return Err("expected a table of the chart's settings".into());
    };
    if let Some(k) = given
        .keys()
        .find(|k| !CHART_VALUE_KEYS.contains(&k.as_str()))
    {
        return Err(format!(
            "a chart has no `{k}`; it holds `{}`",
            CHART_VALUE_KEYS.join("`, `")
        ));
    }
    let node = doc.get(id).ok_or("no such layer")?;
    let current = node.chart().ok_or("not a chart")?;
    let mut args = Map::new();
    args.insert("node".into(), Value::from(id.to_wire()));
    for k in ["kind", "colours", "highlight", "references"] {
        if let Some(v) = given.get(k).filter(|v| !v.is_null()) {
            args.insert(k.into(), v.clone());
        }
    }
    // A whole mapping: each role it leaves out cleared.
    match given.get("mapping") {
        None | Some(Value::Null | Value::Object(_)) => {}
        Some(_) => return Err("`mapping`: expected a table of roles".into()),
    }
    if let Some(Value::Object(roles)) = given.get("mapping") {
        let mut mapping = Map::new();
        for role in ["category", "series", "size", "group", "from", "to", "value"] {
            mapping.insert(role.into(), roles.get(role).cloned().unwrap_or(Value::Null));
        }
        if let Some(k) = roles.keys().find(|k| !mapping.contains_key(*k)) {
            return Err(format!("`mapping`: a chart maps no role `{k}`"));
        }
        args.insert("mapping".into(), Value::Object(mapping));
    }
    // Options patched onto a new chart's on the palette it draws, so a palette
    // the value leaves out is a pick that puts the colours back, as the card's.
    let mut options = match given.get("options") {
        Some(Value::Object(o)) => o.clone(),
        None | Some(Value::Null) => Map::new(),
        Some(_) => return Err("`options`: expected a table".into()),
    };
    for k in ["palette", "palette_group"] {
        options.entry(k).or_insert(Value::Null);
    }
    args.insert("options".into(), Value::Object(options));
    let args = Value::Object(args);
    let verb = find("set_chart").ok_or("no `set_chart`")?;
    let named = |e: VerbError| match e {
        VerbError::Invalid { param, why, .. } => format!("`{param}`: {why}"),
        e => e.to_string(),
    };
    let args = by_names(doc, verb, &args).map_err(named)?;
    let a = Args::check(verb, &args).map_err(named)?;
    refuse_list_copies(doc, verb, &a).map_err(named)?;
    let mut base = current.clone();
    base.options = Options {
        palette: current.options.palette,
        palette_group: current.options.palette_group.clone(),
        ..Options::default()
    };
    base.highlight = Default::default();
    base.references.clear();
    let mut chart = chart_set(doc, &a, base).map_err(named)?;
    // The marks the chart colours that the value leaves out go back on the
    // palette; a stale colour under a name nothing draws is kept.
    let kept = chart_colours_value(doc, &chart);
    let colours = given.get("colours").and_then(Value::as_object);
    for name in kept
        .keys()
        .filter(|n| colours.is_none_or(|c| !c.contains_key(*n)))
    {
        chart = chart_colour(doc, &a, &chart, name, &Value::Null).map_err(named)?;
    }
    // Each series styled whole: what its entry leaves out, and every series the
    // value names none for, back at the default.
    let whole = |style: Option<&Value>| {
        let mut w = serde_json::json!({ "dash": "Solid", "point": null, "visible": true });
        if let (Some(Value::Object(given)), Value::Object(into)) = (style, &mut w) {
            into.extend(given.clone());
        }
        w
    };
    let mut styled = Vec::new();
    match given.get("series") {
        None | Some(Value::Null) => {}
        Some(Value::Object(series)) => {
            for (mark, style) in series {
                if !style.is_object() {
                    return Err(format!(
                        "`series`: `{mark}`: expected {{ dash, point, visible }}"
                    ));
                }
                styled.push(series_named(&a, "series", &chart, mark).map_err(named)?);
                chart = chart_series_style(&a, &chart, mark, &whole(Some(style))).map_err(named)?;
            }
        }
        Some(_) => return Err("`series`: expected a table of series by name".into()),
    }
    for column in chart.mapping.series.clone() {
        if !styled.contains(&column) {
            let mark = column.0.to_string();
            chart = chart_series_style(&a, &chart, &mark, &whole(None)).map_err(named)?;
        }
    }
    let mut ops = Vec::new();
    if chart != *current {
        ops.push(Operation::SetChart {
            id,
            chart: Some(Box::new(chart)),
        });
    }
    let style = match given.get("style") {
        None | Some(Value::Null) => None,
        Some(Value::String(name)) => {
            let found = serde_json::from_value::<StyleId>(Value::from(name.as_str()))
                .ok()
                .filter(|s| doc.styles().get(*s).is_some())
                .or_else(|| {
                    id_named(doc, "style", name)
                        .ok()
                        .and_then(|v| serde_json::from_value(v).ok())
                })
                .ok_or_else(|| format!("`style`: no style has the id or name `{name}`"))?;
            if doc.styles().get(found).map(|s| s.slot()) != Some(StyleSlot::Chart) {
                return Err(format!("`style`: `{name}` is not a chart style"));
            }
            Some(found)
        }
        Some(_) => return Err("`style`: expected a chart style's name".into()),
    };
    if node.styles().chart != style {
        ops.push(crate::style::bind_op(id, StyleSlot::Chart, style));
    }
    Ok(ops)
}

/// `chart` with `mark` drawn in `colour`: a series column by its name or id, or
/// a mark the chart colours by name (a slice, a group, a node), the colour
/// `"#rrggbb"`, a colour variable by id or name, bound, a brush in the model's
/// shape, or null for the palette's. A name the chart draws nothing under is
/// refused listing the ones it does, so a typo is not kept as a style nothing
/// reads.
fn chart_colour(
    doc: &Document,
    a: &Args<'_>,
    chart: &crate::chart::Chart,
    mark: &str,
    colour: &Value,
) -> Result<crate::chart::Chart, VerbError> {
    use crate::variable::BrushSource;
    let bad = |why: String| a.invalid("colours", why);
    let fill = match colour {
        Value::Null => None,
        Value::Object(_) => {
            let fill: BrushSource = serde_json::from_value(colour.clone())
                .map_err(|e| bad(format!("`{mark}`: {e}")))?;
            if let BrushSource::Bound { var, .. } = &fill
                && doc.variables().get(*var).map(|v| v.kind) != Some(VarKind::Color)
            {
                return Err(bad(format!("`{mark}`: it binds no colour variable")));
            }
            Some(fill)
        }
        _ => Some(chart_colour_named(doc, a, mark, colour)?),
    };
    if let Some(marks) = crate::chart::named_marks(chart) {
        if !marks.iter().any(|(n, _)| n == mark) {
            let names: Vec<_> = marks.iter().map(|(n, _)| format!("`{n}`")).collect();
            return Err(bad(format!(
                "this chart colours by name; `{mark}` is none of {}",
                names.join(", ")
            )));
        }
        return Ok(crate::chart::with_node_fill(chart, mark, fill));
    }
    let column = series_named(a, "colours", chart, mark)?;
    Ok(crate::chart::with_series_fill(chart, column, fill))
}

/// The series column `chart` draws under `mark`, its column's name or id,
/// refused as `param`'s fault listing the series when there is none.
fn series_named(
    a: &Args<'_>,
    param: &'static str,
    chart: &crate::chart::Chart,
    mark: &str,
) -> Result<crate::chart::ColumnId, VerbError> {
    chart
        .mapping
        .series
        .iter()
        .copied()
        .find(|c| {
            c.0.to_string() == mark || chart.table.column(*c).is_some_and(|col| col.name == mark)
        })
        .ok_or_else(|| {
            let names: Vec<_> = chart
                .mapping
                .series
                .iter()
                .filter_map(|c| chart.table.column(*c))
                .map(|c| format!("`{}`", c.name))
                .collect();
            a.invalid(
                param,
                format!("`{mark}` is no series; the series are {}", names.join(", ")),
            )
        })
}

/// `chart` with series `mark`'s style patched by `style`: its `dash`, its
/// `point` (null for the chart's own shape) and whether it is `visible`, each
/// word in any case. A series with no style yet takes one only when the patch
/// leaves it other than the default, so writing the defaults changes nothing.
fn chart_series_style(
    a: &Args<'_>,
    chart: &crate::chart::Chart,
    mark: &str,
    style: &Value,
) -> Result<crate::chart::Chart, VerbError> {
    use crate::chart::{Dash, PointShape, SeriesStyle};
    let bad = |why: String| a.invalid("series", format!("`{mark}`: {why}"));
    let column = series_named(a, "series", chart, mark)?;
    let Value::Object(fields) = style else {
        return Err(bad("expected { dash, point, visible }".into()));
    };
    let word = |v: &Value, words: &[&str]| -> Result<usize, VerbError> {
        v.as_str()
            .and_then(|s| words.iter().position(|w| w.eq_ignore_ascii_case(s)))
            .ok_or_else(|| bad(format!("expected one of {}", words.join(", "))))
    };
    let mut next = chart
        .series
        .iter()
        .find(|s| s.column == column)
        .cloned()
        .unwrap_or(SeriesStyle {
            column,
            fill: None,
            visible: true,
            dash: Dash::default(),
            point: None,
        });
    for (k, v) in fields {
        match k.as_str() {
            "dash" => {
                next.dash = [Dash::Solid, Dash::Dashed, Dash::Dotted]
                    [word(v, &["Solid", "Dashed", "Dotted"])?];
            }
            "point" if v.is_null() => next.point = None,
            "point" => {
                let shapes = [
                    PointShape::Circle,
                    PointShape::Square,
                    PointShape::Diamond,
                    PointShape::Triangle,
                ];
                next.point = Some(shapes[word(v, &["Circle", "Square", "Diamond", "Triangle"])?]);
            }
            "visible" => {
                next.visible = v
                    .as_bool()
                    .ok_or_else(|| bad("`visible` is true or false".into()))?;
            }
            other => {
                return Err(bad(format!(
                    "a series has no `{other}`; it has `dash`, `point` and `visible`"
                )));
            }
        }
    }
    let mut out = chart.clone();
    match out.series.iter_mut().find(|s| s.column == column) {
        Some(s) => *s = next,
        None if next.visible && next.dash == Dash::default() && next.point.is_none() => {}
        None => out.series.push(next),
    }
    Ok(out)
}

/// The styles `chart` gives its series other than the default, by the name
/// `set_chart`'s `series` takes: a column's name, its id when two share it.
fn chart_series_value(chart: &crate::chart::Chart) -> Map<String, Value> {
    let mut out = Map::new();
    for column in &chart.mapping.series {
        let Some(s) = chart.series.iter().find(|s| s.column == *column) else {
            continue;
        };
        let mut style = Map::new();
        if s.dash != crate::chart::Dash::default() {
            style.insert(
                "dash".into(),
                serde_json::to_value(s.dash).unwrap_or_default(),
            );
        }
        if let Some(p) = s.point {
            style.insert("point".into(), serde_json::to_value(p).unwrap_or_default());
        }
        if !s.visible {
            style.insert("visible".into(), Value::Bool(false));
        }
        if !style.is_empty()
            && let Some(name) = series_key(chart, *column)
        {
            out.insert(name, Value::Object(style));
        }
    }
    out
}

/// The name `set_chart` takes for series `column`: its column's name, or its id
/// when another column shares the name, which would pick the first.
fn series_key(chart: &crate::chart::Chart, column: crate::chart::ColumnId) -> Option<String> {
    let col = chart.table.column(column)?;
    let shared = chart.table.columns.iter().filter(|c| c.name == col.name);
    Some(match shared.count() {
        1 => col.name.clone(),
        _ => column.0.to_string(),
    })
}

/// One mark's colour written as a string: `"#rrggbb"`, or a colour variable by
/// id or name, bound whole.
fn chart_colour_named(
    doc: &Document,
    a: &Args<'_>,
    mark: &str,
    colour: &Value,
) -> Result<crate::variable::BrushSource, VerbError> {
    use crate::variable::BrushSource;
    let bad = |why: String| a.invalid("colours", why);
    let text = colour.as_str().ok_or_else(|| {
        bad(format!(
            "`{mark}`: a colour is \"#rrggbb\", a variable, or null for the palette's"
        ))
    })?;
    Ok(match loose::hex_colour(text) {
        Some(c) => BrushSource::Literal(crate::Brush::Solid(c)),
        None => {
            let id = serde_json::from_value::<VarId>(Value::from(text))
                .ok()
                .filter(|id| doc.variables().get(*id).is_some())
                .or_else(|| {
                    id_named(doc, "variable", text)
                        .ok()
                        .and_then(|v| serde_json::from_value(v).ok())
                })
                .ok_or_else(|| {
                    bad(format!(
                        "`{mark}`: `{text}` is no \"#rrggbb\" colour and no variable"
                    ))
                })?;
            if doc.variables().get(id).map(|v| v.kind) != Some(VarKind::Color) {
                return Err(bad(format!("`{mark}`: `{text}` is not a colour variable")));
            }
            BrushSource::Bound {
                var: id,
                alpha: 1.0,
            }
        }
    })
}

/// `chart`, refused as `SetChart` would refuse it, the reason said as a fault of
/// the table, the mapping or the options a caller sent.
fn chart_valid(a: &Args<'_>, chart: &crate::chart::Chart) -> Result<(), VerbError> {
    use crate::chart::ChartRule as R;
    chart.validate().map_err(|rule| {
        let param = match rule {
            R::UnknownColumn => "mapping",
            R::BadOption => "options",
            _ => "table",
        };
        let param = if a.verb.params.iter().any(|p| p.name == param) {
            param
        } else {
            "node"
        };
        a.invalid(param, rule)
    })
}

/// The fields of a chart's options, highlight and references that hold a colour
/// as a brush.
const CHART_COLOUR_KEYS: [&str; 10] = [
    "colour",
    "ink",
    "fill",
    "below_zero",
    "stem_colour",
    "increase",
    "decrease",
    "total",
    "muted",
    "palette_group",
];

/// `v`, a chart value, with each `"#rrggbb"` string under one of
/// [`CHART_COLOUR_KEYS`] (or in a list there) written as the literal colour a
/// chart's brush reads, at any depth.
fn chart_hex(v: &Value) -> Value {
    fn brush(v: &mut Value) {
        if let Value::String(s) = v
            && let Some(c) = loose::hex_colour(s)
        {
            let b = crate::variable::BrushSource::Literal(crate::Brush::Solid(c));
            *v = serde_json::to_value(b).unwrap_or_default();
        }
    }
    fn walk(v: &mut Value) {
        match v {
            Value::Object(m) => {
                for (k, field) in m.iter_mut() {
                    if CHART_COLOUR_KEYS.contains(&k.as_str()) {
                        match &mut *field {
                            Value::Array(items) => items.iter_mut().for_each(brush),
                            other => brush(other),
                        }
                    }
                    walk(field);
                }
            }
            Value::Array(items) => items.iter_mut().for_each(walk),
            _ => {}
        }
    }
    let mut v = v.clone();
    walk(&mut v);
    v
}

/// `settled` with the nulls of `patch`, the merge patch it was settled from
/// without them, put back where `patch` writes them.
fn nulls_back(settled: &mut Value, patch: &Value) {
    if let (Value::Object(into), Value::Object(from)) = (settled, patch) {
        for (k, v) in from {
            if v.is_null() {
                into.insert(k.clone(), Value::Null);
            } else if let Some(inner) = into.get_mut(k) {
                nulls_back(inner, v);
            }
        }
    }
}

/// Refuse a highlight naming a category or series `chart` draws nothing under,
/// listing the ones it does.
fn chart_names_known(a: &Args<'_>, chart: &crate::chart::Chart) -> Result<(), VerbError> {
    use crate::chart::Highlighted;
    let t = &chart.table;
    let (asked, known, what): (&Vec<String>, Vec<String>, &str) = match &chart.highlight.target {
        Highlighted::None => return Ok(()),
        Highlighted::Categories(n) => {
            let at = chart.mapping.category.and_then(|c| t.index_of(c));
            let known = t
                .rows
                .iter()
                .filter_map(|r| at.map(|c| r[c].trim().to_string()))
                .filter(|s| !s.is_empty())
                .chain(crate::chart::waterfall_steps(chart))
                .collect();
            (n, known, "categories")
        }
        Highlighted::Series(n) => {
            let known = chart
                .mapping
                .series
                .iter()
                .filter_map(|c| t.column(*c).map(|c| c.name.clone()))
                .chain(
                    crate::chart::named_marks(chart)
                        .into_iter()
                        .flatten()
                        .map(|(n, _)| n),
                )
                .collect();
            (n, known, "series")
        }
    };
    if let Some(bad) = asked.iter().find(|n| !known.contains(n)) {
        // A waterfall's steps repeat its rows' names; each is listed once.
        let mut list: Vec<String> = Vec::new();
        for n in &known {
            let n = format!("`{n}`");
            if !list.contains(&n) {
                list.push(n);
            }
        }
        return Err(a.invalid(
            "highlight",
            format!("`{bad}` is none of the chart's {what}: {}", list.join(", ")),
        ));
    }
    Ok(())
}

/// A chart options patch with the old names still read written under the new
/// ones: `dark` as the theme it names (a patch naming `theme` too keeps that),
/// `outline` as the mark stroke, a pie's `sort` as its order.
fn dark_as_theme(patch: &Value) -> Value {
    let mut wrapped = serde_json::json!({ "options": patch });
    crate::io::migrate::renamed_chart_options(&mut wrapped);
    let mut patch = wrapped["options"].take();
    if let Some(o) = patch.as_object_mut()
        && let Some(dark) = o.remove("dark")
        && !o.contains_key("theme")
    {
        let theme = if dark.as_bool() == Some(true) {
            "Dark"
        } else {
            "Light"
        };
        o.insert("theme".into(), Value::from(theme));
    }
    patch
}

/// `own` with `patch` merged over it as a JSON merge patch (RFC 7396): an object
/// merges key by key, a null sets the key back to its value in `T::default()`
/// (not a nested struct's own default, which may differ), anything else
/// replaces it.
fn patched<T: serde::Serialize + serde::de::DeserializeOwned + Default>(
    a: &Args<'_>,
    param: &'static str,
    own: &T,
    patch: &Value,
) -> Result<T, VerbError> {
    fn merge(into: &mut Value, patch: &Value, default: Option<&Value>) {
        match (into, patch) {
            // One enum variant written over another (a colour bound, `{"var": …}`,
            // over `{"Solid": …}`) replaces it; merged, it would hold both. One
            // side is a variant, a single capitalised key, which a struct's field
            // never is, so a struct that skips its empty fields still merges.
            (Value::Object(into), Value::Object(patch))
                if [&*into, patch].iter().any(|m| {
                    m.len() == 1
                        && m.keys()
                            .all(|k| k.starts_with(|c: char| c.is_ascii_uppercase()))
                }) && patch
                    .iter()
                    .all(|(k, v)| !v.is_null() && !into.contains_key(k)) =>
            {
                *into = patch.clone();
            }
            (Value::Object(into), Value::Object(patch)) => {
                for (k, v) in patch {
                    let default = default.and_then(|d| d.get(k));
                    if v.is_null() {
                        match default {
                            Some(d) => into.insert(k.clone(), d.clone()),
                            None => into.remove(k),
                        };
                    } else {
                        merge(into.entry(k.clone()).or_insert(Value::Null), v, default);
                    }
                }
            }
            (into, patch) => *into = patch.clone(),
        }
    }
    let mut v = serde_json::to_value(own).map_err(|e| a.invalid(param, e))?;
    let default = serde_json::to_value(T::default()).map_err(|e| a.invalid(param, e))?;
    merge(&mut v, patch, Some(&default));
    serde_json::from_value(v).map_err(|e| a.invalid(param, e))
}

/// One ruler guide as `set_guides` and a tree take it and the snapshot reads it:
/// its axis, its position in its owner's space (world space for a page's), its
/// colour when it has its own, and, to keep a guide that is there, its id.
#[derive(serde::Serialize, serde::Deserialize, Debug, Clone, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct GuideIn {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub id: Option<String>,
    pub axis: crate::guide::GuideAxis,
    pub position: f64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub color: Option<peniko::Color>,
}

impl GuideIn {
    /// `g` as a caller reads it.
    pub fn of(g: &crate::guide::Guide) -> Self {
        Self {
            id: Some(g.id.to_wire()),
            axis: g.axis,
            position: g.position,
            color: g.color,
        }
    }
}

/// The ops that leave `owner` with the guides `wanted`: a guide whose `id` it
/// holds moved and recoloured in place, one with none added with a minted id,
/// and the ones not listed removed. An id that names none of the owner's guides,
/// or one of another axis, is refused rather than read as a new guide.
fn guide_ops(
    ctx: &mut Ctx<'_>,
    a: &Args<'_>,
    owner: NodeId,
    wanted: &[GuideIn],
) -> Result<Vec<Operation>, VerbError> {
    use crate::guide::{Guide, GuideId};
    let have: Vec<Guide> = ctx
        .doc
        .guides()
        .iter()
        .filter(|g| g.owner == owner)
        .copied()
        .collect();
    let mut kept = Vec::new();
    let mut adds = Vec::new();
    let mut edits = Vec::new();
    for w in wanted {
        if !w.position.is_finite() {
            return Err(a.invalid("guides", "a position is a number"));
        }
        let Some(wire) = &w.id else {
            let id = GuideId(ctx.ids.mint());
            adds.push(Operation::AddGuide {
                guide: Guide {
                    id,
                    axis: w.axis,
                    position: w.position,
                    color: w.color,
                    owner,
                },
            });
            continue;
        };
        let g = GuideId::from_wire(wire)
            .and_then(|id| have.iter().find(|g| g.id == id))
            .ok_or_else(|| {
                a.invalid(
                    "guides",
                    format!("`{wire}` is none of its guides; leave `id` out to add one"),
                )
            })?;
        if g.axis != w.axis {
            return Err(a.invalid(
                "guides",
                format!("`{wire}` keeps its axis; leave `id` out to add a guide on the other"),
            ));
        }
        kept.push(g.id);
        if g.position != w.position {
            edits.push(Operation::SetGuidePosition {
                id: g.id,
                position: w.position,
            });
        }
        if g.color != w.color {
            edits.push(Operation::SetGuideColor {
                id: g.id,
                color: w.color,
            });
        }
    }
    let mut ops: Vec<Operation> = have
        .iter()
        .filter(|g| !kept.contains(&g.id))
        .map(|g| Operation::RemoveGuide { id: g.id })
        .collect();
    ops.extend(edits);
    ops.extend(adds);
    Ok(ops)
}

/// One layer of `insert_subtree`'s tree: what `create_node` takes, the properties
/// the single-layer verbs set, each in the same form, what it reads (styles,
/// modes, bindings) and its part in a component, and the layers inside it.
/// Ids are minted as it is created, so a caller never writes one; `id` names
/// the layer [`tree`] read, only for another layer of the same tree to refer to.
#[derive(serde::Deserialize, Debug)]
#[serde(deny_unknown_fields)]
pub struct Tree {
    /// Left out only by a layer naming its main in `copy_of`, which takes the
    /// main's ([`expand_copies`]).
    pub kind: Option<NodeKind>,
    /// The layer this one was read from, where another layer of the tree
    /// refers to it (`copy_of`, `swap`, a property's `bound`).
    pub id: Option<String>,
    pub name: Option<String>,
    pub transform: Option<Affine>,
    pub visible: Option<bool>,
    pub locked: Option<bool>,
    pub opacity: Option<f32>,
    /// A [`BLENDS`] word.
    pub blend: Option<String>,
    pub clip: Option<bool>,
    pub mask: Option<bool>,
    /// A [`MASK_MODES`] word.
    pub mask_mode: Option<String>,
    pub mask_inverted: Option<bool>,
    /// The rule a path's fill is read by, stored as set.
    pub fill_rule: Option<crate::node::FillRule>,
    /// A frame's fill left out of its export.
    pub fill_hidden_in_export: Option<bool>,
    /// A copy's corners by the name `set` takes, where it leaves its kind to its
    /// main ([`expand_copies`] moves them into it); refused anywhere else, since
    /// the short forms move a written kind's own.
    pub corner_radius: Option<Value>,
    pub proportions_locked: Option<bool>,
    pub fills: Option<Vec<Fill>>,
    pub strokes: Option<Vec<Stroke>>,
    pub effects: Option<Vec<Effect>>,
    pub exports: Option<Vec<ExportSpec>>,
    pub display: Option<Display>,
    pub item: Option<LayoutItem>,
    pub insets: Option<Insets>,
    pub pivot: Option<Pivot>,
    pub element: Option<crate::element::HtmlElement>,
    /// The address a layer written as a link goes to, as `set`'s `link`.
    pub link: Option<String>,
    pub grids: Option<Vec<crate::layout::LayoutGrid>>,
    /// A frame's own guides; each is added anew, whatever `id` it carries.
    pub guides: Option<Vec<GuideIn>>,
    pub styles: Option<crate::style::StyleRefs>,
    pub modes: Option<Vec<ModeId>>,
    pub queries: Option<Vec<crate::variable::Breakpoints>>,
    /// The layer's own bound fields, as `bind_variable` names them; a fill's,
    /// stroke's, effect's, limit's or flow's binding travels in its item.
    pub bindings: Option<Vec<TreeBinding>>,
    /// A main component.
    pub component: Option<bool>,
    /// The layer this one is a linked copy of, an instance's or a nested copy's.
    pub copy_of: Option<String>,
    /// The main a nested copy's contents are swapped to.
    pub swap: Option<String>,
    pub set: Option<crate::variant::VariantSet>,
    pub variant: Option<Vec<String>>,
    /// A variant layer's lineage key, the layer it pairs with in the set's other
    /// variants: remapped like `copy_of` when the tree holds that layer.
    pub lineage: Option<String>,
    pub props: Option<Vec<crate::variant::Property>>,
    pub description: Option<String>,
    pub doc_link: Option<String>,
    /// The layer's annotation.
    pub note: Option<String>,
    /// The layer's prototype reactions as `set_reactions` takes them, read when
    /// the layer is made: a target the tree holds is remapped to its new layer.
    pub reactions: Option<Value>,
    /// The layer's prototype settings, as `set_play` takes them.
    pub play: Option<Value>,
    /// A list frame's description; its copies are made from the template once
    /// the tree is in, so any it names are dropped.
    pub list: Option<crate::list::List>,
    /// How many children a read cut by `depth` left out here; refused, since
    /// inserting the cut tree would drop them.
    pub omitted: Option<usize>,
    /// Bottom first.
    #[serde(default)]
    pub children: Vec<Tree>,
}

/// One bound field of a tree's layer, as the snapshot's `bindings` reads it.
#[derive(serde::Serialize, serde::Deserialize, Debug, Clone, Copy)]
#[serde(deny_unknown_fields)]
pub struct TreeBinding {
    pub field: build::BoundField,
    pub variable: VarId,
}

/// The ids a tree's layers will take, by the id each was read under: what a
/// reference inside the tree is remapped through.
type Minted = rustc_hash::FxHashMap<NodeId, NodeId>;

/// Mint `tree`'s ids, root first and each layer before its children, as
/// `insert_subtree` answers them, keeping each read `id`'s new one in `minted`.
fn mint_tree(ids: &mut IdSource, tree: &Tree, out: &mut Vec<NodeId>, minted: &mut Minted) {
    let id = ids.mint();
    out.push(id);
    if let Some(read) = tree.id.as_deref().and_then(NodeId::from_wire) {
        minted.insert(read, id);
    }
    for child in &tree.children {
        mint_tree(ids, child, out, minted);
    }
}

/// Move each list fill in `late` naming a property of a main the tree also
/// holds onto that property's new item id: the tree writes properties without
/// theirs, so the new main mints fresh ones, matched to the read main's by
/// position. A fill naming a main outside the tree keeps its id.
fn list_fills_onto_new_props(doc: &Document, minted: &Minted, late: &mut Late) {
    use crate::item::ItemId;
    let mut moved: rustc_hash::FxHashMap<ItemId, ItemId> = Default::default();
    for (read, new) in minted {
        let Some(old) = doc.get(*read) else { continue };
        let made = late.iter().find_map(|op| match op {
            Operation::SetProperties { id, props } if id == new => Some(props),
            _ => None,
        });
        for (o, n) in old.props().iter().zip(made.into_iter().flatten()) {
            moved.insert(o.id, n.id);
        }
    }
    for op in late.iter_mut() {
        if let Operation::SetList { list: Some(l), .. } = op {
            for f in &mut l.fill {
                if let crate::list::Target::Prop(p) = &mut f.target
                    && let Some(n) = moved.get(p)
                {
                    *p = *n;
                }
            }
        }
    }
}

/// A layer a tree refers to by `wire`: the tree's own layer read under it, else
/// the document's.
fn tree_ref(a: &Args<'_>, minted: &Minted, wire: &str) -> Result<NodeId, VerbError> {
    let id = NodeId::from_wire(wire)
        .ok_or_else(|| a.invalid("tree", format!("`{wire}` is not a layer id")))?;
    Ok(minted.get(&id).copied().unwrap_or(id))
}

/// What a tree's layers carry that is checked only once every layer is there
/// (a main, a link, a set and its properties): written after the last layer's
/// own ops, so each names layers the transaction has made.
type Late = Vec<Operation>;

/// The ops creating `tree` at `index` in `parent`, then its children inside it,
/// appended to `done`, each layer taking the next of `ids`, minted beforehand
/// ([`mint_tree`]) so a layer may refer to one after it. A layer is created
/// before its properties are set, so each op meets the layer it names.
#[allow(clippy::too_many_arguments)]
fn tree_ops(
    ctx: &mut Ctx<'_>,
    a: &Args<'_>,
    tree: &Tree,
    parent: NodeId,
    index: usize,
    ids: &mut std::slice::Iter<'_, NodeId>,
    minted: &Minted,
    done: &mut Done,
    late: &mut Late,
) -> Result<(), VerbError> {
    let id = *ids
        .next()
        .ok_or(VerbError::Nothing("a tree layer has no id"))?;
    if tree.corner_radius.is_some() {
        return Err(a.invalid(
            "tree",
            "a tree's corners are part of its `kind`, as `{\"Artboard\": {\"size\": {\"width\": 320, \"height\": 200}, \"corner_radii\": 20}}`",
        ));
    }
    if tree.omitted.is_some() {
        return Err(a.invalid(
            "tree",
            "a layer with `omitted` was read with a `depth` that left its children out; read it with `get_tree` without `depth` to insert it whole",
        ));
    }
    let kind = tree.kind.clone().ok_or_else(|| {
        let why = match &tree.copy_of {
            Some(main) => {
                format!("`copy_of` `{main}` is no layer here, so the copy needs its `kind`")
            }
            None => "a layer gives its `kind`, or names its main in `copy_of`".to_owned(),
        };
        a.invalid("tree", why)
    })?;
    let ops = &mut done.tx.0;
    ops.push(Operation::CreateNode {
        id,
        parent,
        index,
        kind: kind.clone(),
        transform: tree.transform,
        name: tree.name.clone(),
    });
    if let Some(visible) = tree.visible {
        ops.push(Operation::SetVisible { id, visible });
    }
    if let Some(locked) = tree.locked {
        ops.push(Operation::SetLocked { id, locked });
    }
    if let Some(opacity) = tree.opacity {
        if !build::valid_opacity(opacity) {
            return Err(a.invalid("tree", "an opacity is 0 to 1"));
        }
        ops.push(Operation::SetOpacity { id, opacity });
    }
    if let Some(word) = &tree.blend {
        if !BLENDS.contains(&word.as_str()) {
            let why = format!("a blend is one of {}", BLENDS.join(", "));
            return Err(a.invalid("tree", why));
        }
        // As `build::set_blend_all` lands it, for a layer it cannot see yet.
        let blend = match blend_of(word) {
            m if m.allowed_on(&kind) => m,
            _ => BlendMode::Normal,
        };
        ops.push(Operation::SetBlend { id, blend });
    }
    if let Some(clip) = tree.clip {
        ops.push(Operation::SetClip { id, clip });
    }
    if let Some(mask) = tree.mask {
        ops.push(Operation::SetMask { id, mask });
    }
    if let Some(word) = &tree.mask_mode {
        let at = MASK_MODES.iter().position(|w| w == word).ok_or_else(|| {
            a.invalid(
                "tree",
                format!("a mask mode is one of {}", MASK_MODES.join(", ")),
            )
        })?;
        let mode = crate::node::MaskMode::ALL[at];
        ops.push(Operation::SetMaskMode { id, mode });
    }
    if let Some(inverted) = tree.mask_inverted {
        ops.push(Operation::SetMaskInverted { id, inverted });
    }
    if let Some(rule) = tree.fill_rule {
        ops.push(Operation::SetFillRule { id, rule });
    }
    if let Some(hidden) = tree.fill_hidden_in_export {
        ops.push(Operation::SetFillHiddenInExport { id, hidden });
    }
    if let Some(locked) = tree.proportions_locked {
        ops.push(Operation::SetProportionsLocked { id, locked });
    }
    if let Some(fills) = &tree.fills {
        let fills = rekeyed(ctx, &[], fills.clone());
        done.tx.0.push(Operation::SetFills { id, fills });
    }
    if let Some(strokes) = &tree.strokes {
        let strokes = rekeyed(ctx, &[], strokes.clone());
        done.tx.0.push(Operation::SetStrokes { id, strokes });
    }
    if let Some(effects) = &tree.effects {
        let effects = rekeyed(ctx, &[], effects.clone());
        done.tx.0.push(Operation::SetEffects { id, effects });
    }
    if let Some(exports) = &tree.exports {
        let exports = rekeyed(ctx, &[], exports.clone());
        done.tx.0.push(Operation::SetExports { id, exports });
    }
    let ops = &mut done.tx.0;
    if let Some(display) = &tree.display {
        ops.push(Operation::SetDisplay {
            id,
            display: Some(display.clone()),
        });
    }
    if let Some(insets) = tree.insets {
        ops.push(Operation::SetInsets { id, insets });
    }
    if let Some(item) = tree.item {
        ops.push(Operation::SetLayoutItem { id, item });
    }
    if let Some(pivot) = tree.pivot {
        ops.push(Operation::SetPivot {
            id,
            pivot: Some(pivot),
        });
    }
    if let Some(element) = tree.element {
        ops.push(Operation::SetElement {
            id,
            element: Some(element),
        });
    }
    if let Some(href) = &tree.link {
        ops.push(Operation::SetHref {
            id,
            href: href.clone(),
        });
    }
    if let Some(styles) = tree.styles {
        for (slot, style) in styles.iter() {
            let style = Some(style);
            ops.push(match slot {
                StyleSlot::Fills => Operation::SetFillStyle { id, style },
                StyleSlot::Strokes => Operation::SetStrokeStyle { id, style },
                StyleSlot::Effects => Operation::SetEffectsStyle { id, style },
                StyleSlot::Text => Operation::SetTypeStyle { id, style },
                StyleSlot::Layout => Operation::SetLayoutStyle { id, style },
                StyleSlot::Chart => Operation::SetChartStyle { id, style },
            });
        }
    }
    if let Some(modes) = &tree.modes {
        ops.push(Operation::SetModes {
            id,
            modes: modes.clone(),
        });
    }
    if let Some(queries) = &tree.queries {
        ops.push(Operation::SetQueries {
            id,
            queries: queries.clone(),
        });
    }
    if let Some(bindings) = &tree.bindings {
        ops.extend(binding_ops(a, id, bindings)?);
    }
    if let Some(grids) = &tree.grids {
        let grids = rekeyed(ctx, &[], grids.clone());
        done.tx.0.push(Operation::SetLayoutGrids { id, grids });
    }
    for g in tree.guides.iter().flatten() {
        let guide = crate::guide::Guide {
            id: crate::guide::GuideId(ctx.ids.mint()),
            axis: g.axis,
            position: g.position,
            color: g.color,
            owner: id,
        };
        done.tx.0.push(Operation::AddGuide { guide });
    }
    if tree.component == Some(true) {
        late.push(Operation::SetComponent {
            id,
            component: true,
        });
    }
    if let Some(set) = &tree.set {
        late.push(Operation::SetVariantSet {
            id,
            set: Some(set.clone()),
        });
    }
    if let Some(values) = &tree.variant {
        late.push(Operation::SetVariant {
            id,
            values: values.clone(),
        });
    }
    if let Some(key) = &tree.lineage {
        late.push(Operation::SetLineage {
            id,
            lineage: Some(tree_ref(a, minted, key)?),
        });
    }
    if let Some(source) = &tree.copy_of {
        late.push(Operation::SetLink {
            id,
            link: Some(tree_ref(a, minted, source)?),
        });
    }
    if let Some(main) = &tree.swap {
        late.push(Operation::SetSwap {
            id,
            swap: Some(tree_ref(a, minted, main)?),
        });
    }
    if let Some(props) = &tree.props {
        let remap = |ids: &[NodeId]| -> Vec<NodeId> {
            ids.iter()
                .map(|i| minted.get(i).copied().unwrap_or(*i))
                .collect()
        };
        let props = props
            .iter()
            .map(|p| crate::variant::Property {
                bound: remap(&p.bound),
                preferred: remap(&p.preferred),
                ..p.clone()
            })
            .collect();
        let props = rekeyed(ctx, &[], props);
        late.push(Operation::SetProperties { id, props });
    }
    if let Some(text) = &tree.description {
        late.push(Operation::SetDescription {
            of: crate::op::Described::Node(id),
            text: text.clone(),
        });
    }
    if let Some(url) = &tree.doc_link {
        late.push(Operation::SetDocLink {
            id,
            url: url.clone(),
        });
    }
    if let Some(text) = tree.note.as_ref().filter(|t| !t.trim().is_empty()) {
        late.push(Operation::SetNote {
            id,
            text: text.trim().to_owned(),
        });
    }
    if let Some(written) = &tree.play {
        let mut v = written.clone();
        let ids = &mut *ctx.ids;
        let play: crate::prototype::LayerPlay = play_names(ctx.doc, &mut v)
            .and_then(|()| loosen_model("LayerPlay", "", &mut v))
            .map(|()| play_in(&mut v, &mut || ids.mint_item()))
            .and_then(|()| settle::<crate::prototype::LayerPlay>("LayerPlay", v))
            .and_then(|v| serde_json::from_value(v).map_err(|e| e.to_string()))
            .map_err(|why| a.invalid("tree", format!("at `play`: {why}")))?;
        late.push(Operation::SetPlay {
            id,
            play: play.map_layers(&mut |t| minted.get(&t).copied().unwrap_or(t)),
        });
    }
    if let Some(written) = &tree.reactions {
        let reactions = tree_reactions(ctx, a, written, minted)?;
        late.push(Operation::SetReactions { id, reactions });
    }
    if let Some(list) = &tree.list {
        let mut list = list.clone();
        list.copies.clear();
        list.validate(id)
            .map_err(|rule| a.invalid("tree", format!("at `list`: {rule}")))?;
        late.push(Operation::SetList {
            id,
            list: Some(Box::new(list)),
        });
    }
    for (i, child) in tree.children.iter().enumerate() {
        tree_ops(ctx, a, child, id, i, ids, minted, done, late)?;
    }
    Ok(())
}

/// A tree layer's `reactions`, read as `set_reactions` reads them (a transition
/// left out is the default of the page the tree goes into) and checked as the
/// door checks them, each target the tree holds remapped to its new layer.
fn tree_reactions(
    ctx: &mut Ctx<'_>,
    a: &Args<'_>,
    written: &Value,
    minted: &Minted,
) -> Result<Vec<Keyed<crate::prototype::Reaction>>, VerbError> {
    let refused = |why: String| a.invalid("tree", format!("at `reactions`: {why}"));
    // Every layer below the root has a parent the document does not hold yet.
    let base = page_transition(ctx.doc, a.node("parent")?);
    let mut v = written.clone();
    reaction_names(ctx.doc, &mut v).map_err(refused)?;
    loosen_model("[Reaction]", "", &mut v).map_err(refused)?;
    let ids = &mut *ctx.ids;
    reactions_in(&mut v, &base, &mut || ids.mint_item()).map_err(refused)?;
    let v = settle::<Vec<Keyed<crate::prototype::Reaction>>>("[Reaction]", v).map_err(refused)?;
    if let Some(why) = keyed_extra("[Reaction]", &v) {
        return Err(refused(why));
    }
    let mut read: Vec<Keyed<crate::prototype::Reaction>> =
        serde_json::from_value(v).map_err(|e| refused(e.to_string()))?;
    for (i, r) in read.iter_mut().enumerate() {
        let at = format!("reactions[{i}].actions");
        exprs_stored(ctx.doc, &mut r.value.actions, &at)
            .and_then(|()| targets_checked(ctx.doc, &r.value.actions, &at))
            .map_err(|why| a.invalid("tree", why))?;
    }
    if let Some(twice) = crate::item::first_duplicate(&read) {
        return Err(refused(format!(
            "two reactions share the id `{}`",
            twice.0.to_wire()
        )));
    }
    for (i, r) in read.iter().enumerate() {
        crate::prototype::check(std::slice::from_ref(r))
            .map_err(|e| a.invalid("tree", format!("at `reactions[{i}]`: {e}")))?;
    }
    crate::prototype::check(&read).map_err(|e| a.invalid("tree", e.to_string()))?;
    Ok(crate::prototype::map_layers(&read, &mut |t| {
        minted.get(&t).copied().unwrap_or(t)
    }))
}

/// The ops binding a tree layer's own fields: each binding written alone, beside
/// the literal the tree gives, which reads as the placeholder [`tree`] wrote. A
/// binding a fill, stroke, effect or layout item carries in itself is refused
/// here, naming where it goes.
fn binding_ops(
    a: &Args<'_>,
    id: NodeId,
    bindings: &[TreeBinding],
) -> Result<Vec<Operation>, VerbError> {
    use crate::variable::{TEXT_CONTENT, TEXT_FAMILY};
    use build::BoundField as F;
    let mut corners = [None; 4];
    let mut spacing = [None; 6];
    let mut size = [None; 2];
    let mut text = [None; 7];
    let (mut visible, mut opacity) = (None, None);
    for b in bindings {
        if !b.field.in_range() {
            return Err(a.invalid("tree", "a binding names no such place in its field"));
        }
        let var = Some(b.variable);
        match b.field {
            F::Radius => corners = [var; 4],
            F::Corner(k) => corners[k] = var,
            F::Spacing(bits) => (0..6)
                .filter(|k| bits & (1 << k) != 0)
                .for_each(|k| spacing[k] = var),
            F::Size(k) => size[k] = var,
            F::Content => text[TEXT_CONTENT] = var,
            F::Family => text[TEXT_FAMILY] = var,
            F::Type(k) => text[k] = var,
            F::Visible => visible = var,
            F::Opacity => opacity = var,
            F::Flow(_) => {
                return Err(a.invalid("tree", "a flex's flow binding travels in its `display`"));
            }
            F::Fill(_) | F::Stroke(_) | F::StrokeWidth(_) | F::Effect(..) | F::Limit(_) => {
                return Err(a.invalid(
                    "tree",
                    "a fill's, stroke's, effect's or limit's binding travels in that item: a bound fill is `{\"brush\": {\"var\": \"<variable id>\"}}` in `fills`",
                ));
            }
        }
    }
    let mut ops = Vec::new();
    if corners.iter().any(Option::is_some) {
        ops.push(Operation::SetCornerVars { id, vars: corners });
    }
    if spacing.iter().any(Option::is_some) {
        ops.push(Operation::SetSpacingVars { id, vars: spacing });
    }
    if size.iter().any(Option::is_some) {
        ops.push(Operation::SetSizeVars { id, vars: size });
    }
    if text.iter().any(Option::is_some) {
        ops.push(Operation::SetTextVars { id, vars: text });
    }
    if visible.is_some() {
        ops.push(Operation::SetVisibleVar { id, var: visible });
    }
    if opacity.is_some() {
        ops.push(Operation::SetOpacityVar { id, var: opacity });
    }
    Ok(ops)
}

/// A layer's own bound fields, in [`TreeBinding`]'s terms: the ones a paint,
/// stroke, effect, layout item or flex does not carry in itself.
fn own_bindings(n: &crate::node::Node) -> Vec<TreeBinding> {
    use crate::variable::{TEXT_CONTENT, TEXT_FAMILY};
    use build::BoundField as F;
    let mut out = Vec::new();
    let mut push = |field, var: Option<VarId>| {
        if let Some(variable) = var {
            out.push(TreeBinding { field, variable });
        }
    };
    for (k, v) in n.corner_vars().into_iter().enumerate() {
        push(F::Corner(k), v);
    }
    for (k, v) in n.spacing_vars().into_iter().enumerate() {
        push(F::Spacing(1 << k), v);
    }
    push(F::Visible, n.visible_var());
    push(F::Opacity, n.opacity_var());
    for (k, v) in n.size_vars().into_iter().enumerate() {
        push(F::Size(k), v);
    }
    for (k, v) in n.text_vars().into_iter().enumerate() {
        let field = match k {
            TEXT_CONTENT => F::Content,
            TEXT_FAMILY => F::Family,
            k => F::Type(k),
        };
        push(field, v);
    }
    out
}

/// Layer `id` and the layers inside it as a [`Tree`], the shape `insert_subtree`
/// takes and the setters write: its kind with a text's runs, every fill and stroke
/// with the hidden ones, its fill rule, effects and exports, what it reads (styles,
/// modes, width queries, its own bindings), its pivot, element, link, grids and
/// guides, its export-fill and proportion flags, and its part in a component with
/// a variant layer's lineage key, each field only where it differs from a new
/// layer's, and no item ids. A copy also
/// writes each field its source has and it leaves at a new layer's default
/// ([`copy_defaults`]), which `insert_subtree` would otherwise fill back from the
/// source. With `drawn`, a bound field is written as drawn and left unbound, for a
/// `like` sample's look; without, it is the model: the placeholder literal beside
/// its binding, so the tree inserts back as it was. Every layer carries its `id`,
/// which [`tree`] keeps only where the tree refers to it. `None` for no such
/// layer. Every value in its full shape; [`tree`] writes the short forms.
fn full_tree(doc: &Document, id: NodeId, drawn: bool) -> Option<Value> {
    layer_tree(doc, id, drawn, true)
}

/// [`full_tree`], or with `deep` false the layer alone: no children and no
/// [`copy_defaults`].
fn layer_tree(doc: &Document, id: NodeId, drawn: bool, deep: bool) -> Option<Value> {
    fn values<T: serde::Serialize>(list: &[Keyed<T>]) -> Value {
        serde_json::to_value(list.iter().map(|k| &k.value).collect::<Vec<_>>()).unwrap_or_default()
    }
    let n = doc.get(id)?;
    let mut out = Map::new();
    let mut put = |k: &str, v: Value| {
        out.insert(k.to_owned(), v);
    };
    let mut kind = match drawn {
        true => doc
            .resolved_kind(id)
            .map_or_else(|| n.kind().clone(), std::borrow::Cow::into_owned),
        false => n.kind().clone(),
    };
    if drawn && let Some(radii) = kind.corner_radii().and_then(|r| doc.resolve_corners(id, r)) {
        kind.set_corner_radii(radii);
    }
    put("kind", serde_json::to_value(&kind).unwrap_or_default());
    put("id", Value::from(id.to_wire()));
    put("name", Value::from(n.name()));
    if n.transform() != Affine::IDENTITY {
        put(
            "transform",
            serde_json::to_value(n.transform()).unwrap_or_default(),
        );
    }
    let visible = match drawn {
        true => doc.shown(id),
        false => n.visible(),
    };
    if !visible {
        put("visible", Value::Bool(false));
    }
    if n.locked() {
        put("locked", Value::Bool(true));
    }
    let opacity = match drawn {
        true => doc.drawn_opacity(id),
        false => n.opacity(),
    };
    if opacity != 1.0 {
        put("opacity", Value::from(opacity));
    }
    if n.blend() != BlendMode::default_for(n.kind()) {
        put("blend", Value::from(n.blend().label()));
    }
    if n.clip() {
        put("clip", Value::Bool(true));
    }
    if n.mask() {
        put("mask", Value::Bool(true));
        let at = crate::node::MaskMode::ALL
            .iter()
            .position(|m| *m == n.mask_mode())
            .unwrap_or(0);
        put("mask_mode", Value::from(MASK_MODES[at]));
        if n.mask_inverted() {
            put("mask_inverted", Value::Bool(true));
        }
    }
    // The stored rule, as a file keeps it: an exclude reads even-odd whatever it says.
    if n.fill_rule != crate::node::FillRule::default() {
        put(
            "fill_rule",
            serde_json::to_value(n.fill_rule).unwrap_or_default(),
        );
    }
    if n.fill_hidden_in_export() {
        put("fill_hidden_in_export", Value::Bool(true));
    }
    if n.proportions_locked() {
        put("proportions_locked", Value::Bool(true));
    }
    let paint = n.paint();
    if !paint.fills.is_empty() {
        put("fills", values(&paint.fills));
    }
    if !paint.strokes.is_empty() {
        put("strokes", values(&paint.strokes));
    }
    if !n.effects().is_empty() {
        put("effects", values(n.effects()));
    }
    if !n.exports().is_empty() {
        put("exports", values(n.exports()));
    }
    let display = match drawn {
        true => doc.resolved_display(id).map(std::borrow::Cow::into_owned),
        false => n.display().cloned(),
    };
    if let Some(d) = display {
        put("display", serde_json::to_value(&d).unwrap_or_default());
    }
    if *n.item() != LayoutItem::default() {
        put("item", serde_json::to_value(n.item()).unwrap_or_default());
    }
    if *n.insets() != Insets::default() {
        put(
            "insets",
            serde_json::to_value(n.insets()).unwrap_or_default(),
        );
    }
    if let Some(pivot) = n.pivot() {
        put("pivot", serde_json::to_value(pivot).unwrap_or_default());
    }
    if let Some(element) = n.element() {
        put("element", serde_json::to_value(element).unwrap_or_default());
    }
    if !n.href().is_empty() {
        put("link", Value::from(n.href()));
    }
    if !n.grids().is_empty() {
        put("grids", values(n.grids()));
    }
    let guides: Vec<Value> = doc
        .guides()
        .iter()
        .filter(|g| g.owner == id && !doc.is_page(id))
        .map(|g| {
            let g = GuideIn {
                id: None,
                ..GuideIn::of(g)
            };
            serde_json::to_value(g).unwrap_or_default()
        })
        .collect();
    if !guides.is_empty() {
        put("guides", Value::Array(guides));
    }
    if n.styles() != crate::style::StyleRefs::default() {
        put(
            "styles",
            serde_json::to_value(n.styles()).unwrap_or_default(),
        );
    }
    if !n.modes().is_empty() {
        put("modes", serde_json::to_value(n.modes()).unwrap_or_default());
    }
    if !n.queries().is_empty() {
        put(
            "queries",
            serde_json::to_value(n.queries()).unwrap_or_default(),
        );
    }
    let bindings = own_bindings(n);
    if !drawn && !bindings.is_empty() {
        put(
            "bindings",
            serde_json::to_value(bindings).unwrap_or_default(),
        );
    }
    if n.component() {
        put("component", Value::Bool(true));
    }
    if let Some(source) = n.link() {
        put("copy_of", Value::from(source.to_wire()));
    }
    if let Some(main) = n.swap() {
        put("swap", Value::from(main.to_wire()));
    }
    if let Some(set) = n.set() {
        put("set", serde_json::to_value(set).unwrap_or_default());
    }
    if !n.variant().is_empty() {
        put(
            "variant",
            serde_json::to_value(n.variant()).unwrap_or_default(),
        );
    }
    if !drawn && let Some(key) = n.lineage {
        put("lineage", Value::from(key.to_wire()));
    }
    if !n.props().is_empty() {
        put("props", values(n.props()));
    }
    if !n.description().is_empty() {
        put("description", Value::from(n.description()));
    }
    if !n.doc_link().is_empty() {
        put("doc_link", Value::from(n.doc_link()));
    }
    if !n.note().is_empty() {
        put("note", Value::from(n.note()));
    }
    if !deep {
        return Some(Value::Object(out));
    }
    if let Some(src) = n.link().and_then(|s| doc.get(s))
        && let Some(Value::Object(theirs)) = layer_tree(doc, src.id(), drawn, false)
    {
        copy_defaults(n, &mut out, &theirs);
        if n.children().is_empty() && !src.children().is_empty() {
            out.insert("children".into(), Value::Array(Vec::new()));
        }
    }
    let children: Vec<Value> = n
        .children()
        .iter()
        .filter_map(|c| full_tree(doc, *c, drawn))
        .collect();
    if !children.is_empty() {
        out.insert("children".into(), Value::Array(children));
    }
    Some(Value::Object(out))
}

/// A copy's read, `own`, given explicitly each field its source's read `theirs`
/// has and it leaves out, the copy's own being a new layer's default: an override
/// `insert_subtree` would otherwise fill back from the source. Its kind likewise,
/// field by field and a text's style field by field.
fn copy_defaults(n: &crate::node::Node, own: &mut Map<String, Value>, theirs: &Map<String, Value>) {
    let empty = || Value::Array(Vec::new());
    for k in theirs.keys() {
        if own.contains_key(k) {
            continue;
        }
        let unset = match k.as_str() {
            "transform" => serde_json::to_value(Affine::IDENTITY).unwrap_or_default(),
            "visible" => Value::Bool(true),
            "opacity" => Value::from(1.0),
            "blend" => Value::from(BlendMode::default_for(n.kind()).label()),
            "locked"
            | "clip"
            | "mask"
            | "mask_inverted"
            | "fill_hidden_in_export"
            | "proportions_locked" => Value::Bool(false),
            "mask_mode" => {
                let at = crate::node::MaskMode::ALL
                    .iter()
                    .position(|m| *m == n.mask_mode())
                    .unwrap_or(0);
                Value::from(MASK_MODES[at])
            }
            "fill_rule" => serde_json::to_value(n.fill_rule).unwrap_or_default(),
            "fills" | "strokes" | "effects" | "exports" | "grids" | "modes" | "queries"
            | "bindings" => empty(),
            "display" | "pivot" | "element" | "swap" => Value::Null,
            "item" => serde_json::to_value(n.item()).unwrap_or_default(),
            "insets" => serde_json::to_value(n.insets()).unwrap_or_default(),
            "styles" => serde_json::to_value(n.styles()).unwrap_or_default(),
            "link" => Value::from(""),
            // A main's own, or always written.
            _ => continue,
        };
        own.insert(k.clone(), unset);
    }
    if let (Some(mine), Some(source)) = (own.get_mut("kind"), theirs.get("kind")) {
        explicit_kind(n.kind(), mine, source);
    }
}

/// `mine`, `kind` as serde writes it, given explicitly each field serde left out
/// (its default) where `theirs`, another kind of the same variant, has one, and
/// a text's style field by field. A field's default is found as the one candidate
/// that reads back as `kind`; one with none found is left out. The candidates
/// include the defaults of the types whose default no zeroed value spells: a
/// text's Original case, a zero length in px, a path with no mirror.
fn explicit_kind(kind: &NodeKind, mine: &mut Value, theirs: &Value) {
    fn zeroed(v: &Value) -> Value {
        match v {
            Value::Number(_) => Value::from(0.0),
            Value::Bool(_) => Value::Bool(false),
            Value::String(_) => Value::from(""),
            Value::Array(_) => Value::Array(Vec::new()),
            Value::Object(m) => {
                Value::Object(m.iter().map(|(k, v)| (k.clone(), zeroed(v))).collect())
            }
            Value::Null => Value::Null,
        }
    }
    let Some((variant, Value::Object(from))) = theirs.as_object().and_then(|m| m.iter().next())
    else {
        return;
    };
    let reads_back = |trial: &Value| {
        serde_json::from_value::<NodeKind>(trial.clone())
            .ok()
            .as_ref()
            == Some(kind)
    };
    let fill = |mine: &mut Value, path: &[&str], source: &Value| {
        let candidates = [
            Value::Null,
            Value::Bool(false),
            Value::from(0.0),
            Value::Array(Vec::new()),
            Value::Object(Map::new()),
            zeroed(source),
            serde_json::to_value(crate::typography::TextCase::default()).unwrap_or_default(),
            serde_json::to_value(Length::ZERO).unwrap_or_default(),
            serde_json::to_value(crate::symmetry::PathMirror::default()).unwrap_or_default(),
        ];
        for c in candidates {
            let mut trial = mine.clone();
            let Some(Value::Object(at)) = path
                .iter()
                .take(path.len() - 1)
                .try_fold(&mut trial, |v, k| v.get_mut(*k))
            else {
                return;
            };
            at.insert(path[path.len() - 1].to_owned(), c.clone());
            if reads_back(&trial) {
                *mine = trial;
                return;
            }
        }
    };
    for (k, source) in from {
        // A copy's own words take none of its source's runs.
        if matches!(k.as_str(), "spans" | "para_spans") {
            continue;
        }
        let at = mine.get(variant).and_then(|v| v.get(k));
        match (at, source) {
            (None, _) => fill(mine, &[variant, k], source),
            (Some(Value::Object(style)), Value::Object(theirs)) if k == "style" => {
                let missing: Vec<&String> = theirs
                    .keys()
                    .filter(|sk| !style.contains_key(*sk))
                    .collect();
                for sk in missing {
                    fill(mine, &[variant, k, sk], &theirs[sk]);
                }
            }
            _ => {}
        }
    }
}

/// [`full_tree`] of the model in the short forms `insert_subtree` reads: hex
/// colours, paints without their default fields, lengths as `"22px"`, a path as
/// SVG data. A layer keeps its `id` only where another layer of the tree refers
/// to it, and a link that leaves the tree for an instance the read is inside of
/// is dropped: that copy is placed as a plain one, as a layer pasted out of an
/// instance is. A list frame gives its `list` and its template, not its copies.
pub fn tree(doc: &Document, id: NodeId) -> Option<Value> {
    tree_to(doc, id, None)
}

/// [`tree`] cut `depth` levels below `id` when given: a layer at the cut keeps
/// its `id` and gives `omitted`, how many children it has, in their place.
pub fn tree_to(doc: &Document, id: NodeId, depth: Option<usize>) -> Option<Value> {
    let mut v = full_tree(doc, id, false)?;
    // A layer inside an instance the read starts within is linked through that
    // instance's root, which the tree does not hold.
    let outer = crate::component::instance_root(doc, id).filter(|r| *r != id);
    if let Some(outer) = outer {
        unlink_members(doc, outer, &mut v);
    }
    collapse_lists(doc, &mut v);
    let mut cut = std::collections::HashSet::new();
    if let Some(depth) = depth {
        cut_at(&mut v, depth, &mut cut);
    }
    let mut layers = Vec::new();
    layer_ids(&v, &mut layers);
    let mut referred = std::collections::HashSet::new();
    referenced(&v, &mut referred);
    for r in layers
        .iter()
        .filter_map(|l| own_reactions(doc, *l))
        .flatten()
    {
        referred.extend(r.trigger.layer_target().map(|t| t.to_wire()));
        for k in &r.actions {
            referred.extend(k.value.layer_targets().iter().map(|t| t.to_wire()));
        }
    }
    for l in &layers {
        let target = doc.get(*l).and_then(|n| n.play().layer_target());
        referred.extend(target.map(|t| t.to_wire()));
    }
    referred.extend(cut);
    keep_referred_ids(&mut v, &referred);
    loose::tighten("tree", &mut v);
    keep_copy_italics(doc, &mut v);
    put_prototype(doc, &mut v, &mut layers.into_iter());
    Some(v)
}

/// Each list frame of `v` given its `list` and left without its copies: the
/// list makes them again from its template once inserted, and copies written
/// out would land as plain instances beside the ones it makes.
fn collapse_lists(doc: &Document, v: &mut Value) {
    let Value::Object(map) = v else { return };
    let list = map
        .get("id")
        .and_then(Value::as_str)
        .and_then(NodeId::from_wire)
        .and_then(|id| doc.get(id))
        .and_then(|n| n.list());
    if let Some(list) = list {
        let copies: std::collections::HashSet<String> =
            list.copies.iter().map(|c| c.to_wire()).collect();
        if let Some(Value::Array(children)) = map.get_mut("children") {
            children.retain(|c| {
                c.get("id")
                    .and_then(Value::as_str)
                    .is_none_or(|id| !copies.contains(id))
            });
        }
        let mut own = list.clone();
        own.copies.clear();
        map.insert(
            "list".into(),
            serde_json::to_value(&own).unwrap_or_default(),
        );
    }
    for child in map
        .get_mut("children")
        .and_then(Value::as_array_mut)
        .into_iter()
        .flatten()
    {
        collapse_lists(doc, child);
    }
}

/// `v` cut `depth` levels below its root: a layer at the cut with children
/// gives `omitted`, their count, in their place, and its id goes into `cut`
/// so the tree keeps it for the next read to start from.
fn cut_at(v: &mut Value, depth: usize, cut: &mut std::collections::HashSet<String>) {
    let Value::Object(map) = v else { return };
    if depth == 0 {
        let count = map
            .get("children")
            .and_then(Value::as_array)
            .map_or(0, Vec::len);
        if count > 0 {
            map.remove("children");
            map.insert("omitted".into(), Value::from(count));
            if let Some(Value::String(id)) = map.get("id") {
                cut.insert(id.clone());
            }
        }
        return;
    }
    for child in map
        .get_mut("children")
        .and_then(Value::as_array_mut)
        .into_iter()
        .flatten()
    {
        cut_at(child, depth - 1, cut);
    }
}

/// The ids of a tree's layers, pre-order, as [`full_tree`] writes each.
fn layer_ids(v: &Value, out: &mut Vec<NodeId>) {
    if let Some(id) = v
        .get("id")
        .and_then(Value::as_str)
        .and_then(NodeId::from_wire)
    {
        out.push(id);
    }
    for child in v
        .get("children")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
    {
        layer_ids(child, out);
    }
}

/// The reactions a tree writes on `id`: its own when it has any, and a copy's
/// even when empty where its source has some, since `insert_subtree` does not
/// fill a copy's reactions from its main.
fn own_reactions(doc: &Document, id: NodeId) -> Option<&[Keyed<crate::prototype::Reaction>]> {
    let n = doc.get(id)?;
    let source = n.link().and_then(|s| doc.get(s));
    (!n.reactions().is_empty() || source.is_some_and(|s| !s.reactions().is_empty()))
        .then(|| n.reactions())
}

/// Each layer of the tree `v` given its `reactions` and `play` in the short forms
/// `set_reactions` and `set_play` take, taking the layers' ids from `layers`
/// pre-order. Written after the tree's own short forms, which do not reach them.
/// Each transition is written whole: the tree may be inserted on a page whose
/// default transition is not this one's.
fn put_prototype(doc: &Document, v: &mut Value, layers: &mut impl Iterator<Item = NodeId>) {
    let Some(id) = layers.next() else { return };
    let Value::Object(map) = v else { return };
    if let Some(reactions) = own_reactions(doc, id) {
        map.insert("reactions".into(), reactions_json(doc, reactions, None));
    }
    if let Some(n) = doc.get(id) {
        let source = n.link().and_then(|s| doc.get(s));
        if !n.play().is_empty() || source.is_some_and(|s| !s.play().is_empty()) {
            map.insert("play".into(), play_value(n.play()));
        }
    }
    for child in map
        .get_mut("children")
        .and_then(Value::as_array_mut)
        .into_iter()
        .flatten()
    {
        put_prototype(doc, child, layers);
    }
}

/// Put back the `"italic": false` the short forms leave out of a copy's text
/// style where its source is italic, which `insert_subtree` would otherwise
/// fill back from the source.
fn keep_copy_italics(doc: &Document, v: &mut Value) {
    let Value::Object(map) = v else { return };
    let italic_source = map
        .get("copy_of")
        .and_then(Value::as_str)
        .and_then(NodeId::from_wire)
        .and_then(|s| doc.get(s))
        .is_some_and(|s| matches!(s.kind(), NodeKind::Text { style, .. } if style.italic));
    if italic_source
        && let Some(Value::Object(style)) = map
            .get_mut("kind")
            .and_then(|k| k.get_mut("Text"))
            .and_then(|t| t.get_mut("style"))
    {
        style.entry("italic").or_insert(Value::Bool(false));
    }
    for child in map
        .get_mut("children")
        .and_then(Value::as_array_mut)
        .into_iter()
        .flatten()
    {
        keep_copy_italics(doc, child);
    }
}

/// Drop `copy_of` from every layer of `v` whose instance root is `outer`.
fn unlink_members(doc: &Document, outer: NodeId, v: &mut Value) {
    let Value::Object(map) = v else { return };
    let id = map
        .get("id")
        .and_then(Value::as_str)
        .and_then(NodeId::from_wire);
    if id.and_then(|id| crate::component::instance_root(doc, id)) == Some(outer) {
        map.remove("copy_of");
    }
    for child in map
        .get_mut("children")
        .and_then(Value::as_array_mut)
        .into_iter()
        .flatten()
    {
        unlink_members(doc, outer, child);
    }
}

/// Every layer id a layer of `v` refers to: its link, its swap, its lineage
/// key, and its properties' bound layers and preferred mains; and a layer
/// holding properties itself, whose read id `insert_subtree` moves a list's
/// fills by.
fn referenced(v: &Value, out: &mut std::collections::HashSet<String>) {
    let Value::Object(map) = v else { return };
    if map.contains_key("props")
        && let Some(Value::String(id)) = map.get("id")
    {
        out.insert(id.clone());
    }
    for key in ["copy_of", "swap", "lineage"] {
        if let Some(Value::String(s)) = map.get(key) {
            out.insert(s.clone());
        }
    }
    for p in map
        .get("props")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
    {
        for key in ["bound", "preferred"] {
            for s in p.get(key).and_then(Value::as_array).into_iter().flatten() {
                if let Some(s) = s.as_str() {
                    out.insert(s.to_owned());
                }
            }
        }
    }
    for child in map
        .get("children")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
    {
        referenced(child, out);
    }
}

/// Keep each layer's `id` in `v` only where `referred` holds it.
fn keep_referred_ids(v: &mut Value, referred: &std::collections::HashSet<String>) {
    let Value::Object(map) = v else { return };
    if map
        .get("id")
        .and_then(Value::as_str)
        .is_none_or(|id| !referred.contains(id))
    {
        map.remove("id");
    }
    for child in map
        .get_mut("children")
        .and_then(Value::as_array_mut)
        .into_iter()
        .flatten()
    {
        keep_referred_ids(child, referred);
    }
}

/// `v`, a read of the model such as a variable's cells or a style's value, in
/// the short forms the writes take, as [`tree`] writes a layer.
pub fn shorten(v: &mut Value) {
    loose::tighten("", v);
}

/// A frame's layout grids as `set_layout_grids` takes them, in the short forms.
pub fn grids_value(grids: &[Keyed<crate::layout::LayoutGrid>]) -> Value {
    let mut v = serde_json::to_value(grids.iter().map(|k| &k.value).collect::<Vec<_>>())
        .unwrap_or_default();
    loose::tighten("grids", &mut v);
    v
}

/// The ruler guides `owner` holds as `set_guides` takes them, each with its id,
/// colours as hex; empty for none.
pub fn guides_value(doc: &Document, owner: NodeId) -> Vec<Value> {
    doc.guides()
        .iter()
        .filter(|g| g.owner == owner)
        .map(|g| {
            let mut v = serde_json::to_value(GuideIn::of(g)).unwrap_or_default();
            loose::tighten("", &mut v);
            v
        })
        .collect()
}

/// A layer's reactions as `set_reactions` takes them, each with its id: a
/// transition's fields left out where they are `base`'s, the page's default that
/// `set_reactions` fills back in, and colours as hex. A smart animate whose kind
/// is written keeps its side unless it has none, since one named without a side
/// takes none (`transition_in`).
pub fn reactions_value(
    doc: &Document,
    reactions: &[Keyed<crate::prototype::Reaction>],
    base: &crate::prototype::Transition,
) -> Value {
    reactions_json(doc, reactions, Some(base))
}

/// [`reactions_value`], each transition written whole when `base` is `None`.
fn reactions_json(
    doc: &Document,
    reactions: &[Keyed<crate::prototype::Reaction>],
    base: Option<&crate::prototype::Transition>,
) -> Value {
    fn tighten(v: &mut Value, base: &Map<String, Value>) {
        match v {
            Value::Object(map) => {
                if let Some(Value::Object(t)) = map.get_mut("transition") {
                    // A smart animate written by kind keeps a side it has.
                    let sided = t.get("kind").and_then(Value::as_str) == Some("SmartAnimate")
                        && base.get("kind") != t.get("kind")
                        && t.get("direction").and_then(Value::as_str) != Some("None");
                    t.retain(|k, field| (sided && k == "direction") || base.get(k) != Some(field));
                    if t.is_empty() {
                        map.remove("transition");
                    }
                }
                map.values_mut().for_each(|c| tighten(c, base));
            }
            Value::Array(items) => items.iter_mut().for_each(|c| tighten(c, base)),
            _ => {}
        }
    }
    let mut named = reactions.to_vec();
    for r in &mut named {
        exprs_shown(doc, &mut r.value.actions);
    }
    let mut v = serde_json::to_value(named).unwrap_or_default();
    if let Some(Value::Object(base)) = base.map(|b| serde_json::to_value(b).unwrap_or_default()) {
        tighten(&mut v, &base);
    }
    loose::tighten("", &mut v);
    v
}

/// A prototype value with no transitions of its own to leave out (a layer's or a
/// page's play settings, a page's flows) as its setter takes it, colours as hex.
pub fn play_value<T: serde::Serialize>(value: &T) -> Value {
    let mut v = serde_json::to_value(value).unwrap_or_default();
    loose::tighten("", &mut v);
    v
}

/// What `set_page_play` says of a timing the door would refuse.
const UNUSABLE_TIMING: &str = "at `play.transition.timing`: a timing's numbers are finite, a spring's positive, and a cubic bezier's x within 0 to 1";

/// The transition a new link on `id`'s page takes, as serde writes it: what an
/// action written without one is given.
fn page_transition(doc: &Document, id: NodeId) -> Value {
    let t = doc
        .page_of(id)
        .and_then(|p| doc.page(p))
        .map(|p| p.play.transition)
        .unwrap_or_default();
    serde_json::to_value(t).unwrap_or_default()
}

/// The keyed list parameter `name` of the model `model`, read from its short
/// forms once `prepare` has filled in what the model's serde shape requires and
/// a caller may leave out: an item's id, an action's transition.
fn keyed_arg<T: serde::de::DeserializeOwned>(
    a: &Args<'_>,
    name: &'static str,
    model: &str,
    prepare: impl FnOnce(&mut Value) -> Result<(), String>,
) -> Result<T, VerbError> {
    let mut v = a.get(name).cloned().ok_or(VerbError::Missing {
        verb: a.verb.name,
        param: name,
    })?;
    // Some clients send a parameter with no schema type as JSON text.
    if let Value::String(s) = &v
        && let Ok(parsed) = serde_json::from_str::<Value>(s.trim())
    {
        v = parsed;
    }
    loosen_model(model, "", &mut v).map_err(|e| a.invalid(name, e))?;
    prepare(&mut v).map_err(|e| a.invalid(name, e))?;
    let v = settle::<T>(model, v).map_err(|e| a.invalid(name, e))?;
    if let Some(why) = keyed_extra(model, &v) {
        return Err(a.invalid(name, why));
    }
    serde_json::from_value(v).map_err(|e| a.invalid(name, e))
}

/// The first key of an item of the keyed list `v` that its model has no field
/// for, refused listing the fields: `Keyed` flattens its value, which hides such
/// a key from `read_model`'s check, so it would be dropped without a word.
fn keyed_extra(model: &str, v: &Value) -> Option<String> {
    let fields = match model {
        "[Flow]" => struct_fields::<crate::prototype::Flow>()?,
        "[Reaction]" => struct_fields::<crate::prototype::Reaction>()?,
        _ => return None,
    };
    v.as_array()?.iter().enumerate().find_map(|(i, item)| {
        let key = item
            .as_object()?
            .keys()
            .find(|k| *k != "id" && !fields.contains(&k.as_str()))?;
        Some(format!(
            "at `[{i}].{key}`: no such field; the fields there are `id`, `{}`",
            fields.join("`, `")
        ))
    })
}

/// An `id` given to each item of the list `v` written without one.
fn ids_in(v: &mut Value, mint: &mut dyn FnMut() -> crate::item::ItemId) {
    for item in v.as_array_mut().into_iter().flatten() {
        if let Value::Object(map) = item
            && !map.contains_key("id")
        {
            map.insert(
                "id".into(),
                serde_json::to_value(mint()).unwrap_or_default(),
            );
        }
    }
}

/// A layer's play settings `v` in their serde shape: an `id` for each snap point
/// written without one, which a *Drop* trigger names it by.
fn play_in(v: &mut Value, mint: &mut dyn FnMut() -> crate::item::ItemId) {
    if let Some(points) = v.pointer_mut("/follow/points") {
        ids_in(points, mint);
    }
}

/// Each prototype action by its name, and whether it carries a transition: what
/// an action written as a bare word, or its kind in another case, is read as.
/// Kept with [`crate::prototype::Action`]'s variants.
const ACTIONS: [(&str, bool); 15] = [
    ("Navigate", true),
    ("Back", false),
    ("OpenOverlay", true),
    ("CloseOverlay", true),
    ("SwapOverlay", true),
    ("ScrollTo", true),
    ("ChangeState", true),
    ("SetVariable", false),
    ("SetMode", false),
    ("Conditional", false),
    ("OpenUrl", false),
    ("RunScript", false),
    ("AddRow", false),
    ("EditRow", false),
    ("DeleteRow", false),
];

/// The list of reactions `v` in its serde shape: an `id` for each reaction and
/// action written without one, its trigger's slips put right, and each action as
/// [`actions_in`] reads it; or the first action that does not read, by its path.
fn reactions_in(
    v: &mut Value,
    base: &Value,
    mint: &mut dyn FnMut() -> crate::item::ItemId,
) -> Result<(), String> {
    ids_in(v, mint);
    for (i, r) in v.as_array_mut().into_iter().flatten().enumerate() {
        // Settled alone: inside a keyed item serde loses the path a repair needs.
        if let Some(t) = r.get_mut("trigger")
            && let Ok(settled) = settle::<crate::prototype::Trigger>("[Reaction]", t.clone())
        {
            *t = settled;
        }
        if let Some(actions) = r.get_mut("actions") {
            actions_in(actions, &format!("[{i}].actions"), base, mint)?;
        }
    }
    Ok(())
}

/// Each expression in `actions` (at `path`) written with names stored in the id
/// form, as the Prototype tab's fields store it, and refused where a field
/// would refuse it: one that does not read, names what `doc` has none of, or
/// comes to the wrong kind. One in the stored form naming what is gone is left
/// as written: a target may dangle, so a reaction sent back as read is never
/// refused for it.
fn exprs_stored(
    doc: &Document,
    actions: &mut [Keyed<crate::prototype::Action>],
    path: &str,
) -> Result<(), String> {
    use crate::prototype::{Action, Reads, VarChange, WhichRow};
    let store = |e: &mut crate::prototype::Expr,
                 want: Option<VarKind>,
                 reads: Reads,
                 at: &str|
     -> Result<(), String> {
        if dangles(doc, e, reads.row()) {
            return Ok(());
        }
        let text = std::mem::take(&mut e.0);
        *e = crate::prototype::expr_stored(doc, &text, want, reads)
            .map_err(|(why, _)| format!("at `{at}`: `{text}`: {why}"))?;
        Ok(())
    };
    // A blank cell is dropped from *Add row*, which leaves an unlisted column
    // empty, as its field drops one; *Edit row* refuses it, as its field does,
    // since play could not evaluate it.
    let cells = |cells: &mut Vec<crate::prototype::CellWrite>, add: bool, reads, at: &str| {
        if add {
            cells.retain(|c| !c.to.0.trim().is_empty());
        }
        cells.iter_mut().enumerate().try_for_each(|(i, c)| {
            let at = format!("{at}.cells[{i}].to");
            if c.to.0.trim().is_empty() {
                return Err(format!(
                    "at `{at}`: a cell is an expression; write \"\" to empty it, or leave its column out"
                ));
            }
            store(&mut c.to, None, reads, &at)
        })
    };
    let row = |row: &mut WhichRow, table, at: &str| match row {
        WhichRow::FirstWhere { when } => store(
            when,
            Some(VarKind::Boolean),
            Reads::Tables(Some(table)),
            &format!("{at}.row.when"),
        ),
        _ => Ok(()),
    };
    for (j, k) in actions.iter_mut().enumerate() {
        let at = format!("{path}[{j}]");
        match &mut k.value {
            Action::SetVariable {
                var,
                change: VarChange::Set { to },
            } => {
                let want = doc.variables().get(*var).map(|v| v.kind);
                store(to, want, Reads::Tables(None), &format!("{at}.to"))?;
            }
            Action::Conditional {
                branches,
                otherwise,
            } => {
                for (b, branch) in branches.iter_mut().enumerate() {
                    let here = format!("{at}.branches[{b}]");
                    store(
                        &mut branch.when,
                        Some(VarKind::Boolean),
                        Reads::Tables(None),
                        &format!("{here}.when"),
                    )?;
                    exprs_stored(doc, &mut branch.actions, &format!("{here}.actions"))?;
                }
                exprs_stored(doc, otherwise, &format!("{at}.otherwise"))?;
            }
            Action::AddRow { cells: c, .. } => cells(c, true, Reads::Tables(None), &at)?,
            Action::EditRow {
                table,
                row: r,
                cells: c,
            } => {
                row(r, *table, &at)?;
                cells(c, false, Reads::Tables(Some(*table)), &at)?;
            }
            Action::DeleteRow { table, row: r } => row(r, *table, &at)?,
            _ => {}
        }
    }
    Ok(())
}

/// Each action in `actions` (at `path`) whose target is there but would never
/// play refused: a screen action going to a layer that is no frame, *Change to*
/// switching a layer that is no instance or main, or to one that is no main. A
/// target that is gone is let through, since targets dangle.
fn targets_checked(
    doc: &Document,
    actions: &[Keyed<crate::prototype::Action>],
    path: &str,
) -> Result<(), String> {
    use crate::prototype::Action;
    for (j, k) in actions.iter().enumerate() {
        let at = format!("{path}[{j}]");
        let frame = |id: NodeId, field: &str| match doc.get(id) {
            Some(n) if !matches!(n.kind(), NodeKind::Artboard { .. }) => Err(format!(
                "at `{at}.{field}`: `{}` is no frame; a screen action goes to a frame",
                id.to_wire()
            )),
            _ => Ok(()),
        };
        match &k.value {
            Action::Navigate { to, .. } => frame(*to, "Navigate.to")?,
            Action::OpenOverlay { overlay, .. } => frame(*overlay, "OpenOverlay.overlay")?,
            Action::SwapOverlay { overlay, .. } => frame(*overlay, "SwapOverlay.overlay")?,
            Action::ChangeState { instance, to, .. } => {
                if let Some(n) = doc.get(*instance)
                    && n.link().is_none()
                    && !n.component
                {
                    return Err(format!(
                        "at `{at}.ChangeState.instance`: `{}` is no instance; Change to switches an instance to another variant",
                        instance.to_wire()
                    ));
                }
                if doc.get(*to).is_some_and(|n| !n.component) {
                    return Err(format!(
                        "at `{at}.ChangeState.to`: `{}` is no component main; Change to switches to a variant of the instance's set",
                        to.to_wire()
                    ));
                }
            }
            Action::Conditional {
                branches,
                otherwise,
            } => {
                for (b, branch) in branches.iter().enumerate() {
                    targets_checked(doc, &branch.actions, &format!("{at}.branches[{b}].actions"))?;
                }
                targets_checked(doc, otherwise, &format!("{at}.otherwise"))?;
            }
            _ => {}
        }
    }
    Ok(())
}

/// Whether `e`, in the stored form, reads a variable, table or column `doc` no
/// longer has, `row` being the table a `{row.…}` reads.
fn dangles(
    doc: &Document,
    e: &crate::prototype::Expr,
    row: Option<crate::list::DataSetId>,
) -> bool {
    use crate::prototype::expr::{Ref, Unread, table_kind};
    let gone = |r: &Ref| match r {
        Ref::Var(v) => doc.variables().get(*v).is_none(),
        r => matches!(table_kind(doc.data_sets(), row, r), Err(Unread::Missing)),
    };
    e.check().is_ok() && e.refs().iter().any(gone)
}

/// Each expression in `actions` written with names, as the Prototype tab's
/// fields show it, each read against the row [`exprs_stored`] stores it with: the
/// inverse of that store, so what is read writes back as it was. One that
/// [`dangles`] stays whole in the stored form, which the store lets through; a
/// name two variables share stays its id.
fn exprs_shown(doc: &Document, actions: &mut [Keyed<crate::prototype::Action>]) {
    use crate::prototype::{Action, VarChange, WhichRow, expr_shown};
    let show = |e: &mut crate::prototype::Expr, row| {
        if !dangles(doc, e, row) {
            *e = crate::prototype::Expr(expr_shown(doc, e, row));
        }
    };
    let which = |r: &mut WhichRow, table| {
        if let WhichRow::FirstWhere { when } = r {
            show(when, Some(table));
        }
    };
    for k in actions {
        match &mut k.value {
            Action::SetVariable {
                change: VarChange::Set { to },
                ..
            } => show(to, None),
            Action::Conditional {
                branches,
                otherwise,
            } => {
                for b in branches.iter_mut() {
                    show(&mut b.when, None);
                    exprs_shown(doc, &mut b.actions);
                }
                exprs_shown(doc, otherwise);
            }
            Action::AddRow { cells, .. } => cells.iter_mut().for_each(|c| show(&mut c.to, None)),
            Action::EditRow { table, row, cells } => {
                which(row, *table);
                cells.iter_mut().for_each(|c| show(&mut c.to, Some(*table)));
            }
            Action::DeleteRow { table, row } => which(row, *table),
            _ => {}
        }
    }
}

/// The list of actions `v`, at `path` in the list of reactions, in its serde
/// shape: a bare word as its kind (`"Back"`), the kind in any case, an `id`
/// where none is written, a transition written as its kind alone or in part
/// completed from `base`, each action's slips put right as [`settle`] puts them,
/// and the actions a conditional nests likewise.
fn actions_in(
    v: &mut Value,
    path: &str,
    base: &Value,
    mint: &mut dyn FnMut() -> crate::item::ItemId,
) -> Result<(), String> {
    for (j, item) in v.as_array_mut().into_iter().flatten().enumerate() {
        let at = format!("{path}[{j}]");
        if let Value::String(word) = item {
            *item = Value::Object(Map::from_iter([(word.clone(), Value::Null)]));
        }
        let Value::Object(map) = item else { continue };
        let id = map
            .remove("id")
            .unwrap_or_else(|| serde_json::to_value(mint()).unwrap_or_default());
        let written = map.keys().next().cloned();
        if let Some(written) = written
            && let Some((name, moves)) = ACTIONS.iter().find(|(n, _)| plain(n) == plain(&written))
        {
            let mut body = map.remove(&written).unwrap_or(Value::Null);
            if *moves && body.is_null() {
                body = Value::Object(Map::new());
            }
            if let Value::Object(fields) = &mut body {
                if *moves {
                    let given = fields.remove("transition");
                    fields.insert("transition".into(), transition_in(given, base));
                }
                for (b, branch) in fields
                    .get_mut("branches")
                    .and_then(Value::as_array_mut)
                    .into_iter()
                    .flatten()
                    .enumerate()
                {
                    if let Some(nested) = branch.get_mut("actions") {
                        let inner = format!("{at}.Conditional.branches[{b}].actions");
                        actions_in(nested, &inner, base, mint)?;
                    }
                }
                if let Some(nested) = fields.get_mut("otherwise") {
                    let inner = format!("{at}.Conditional.otherwise");
                    actions_in(nested, &inner, base, mint)?;
                }
            }
            map.insert((*name).into(), body);
        }
        // Settled alone, as a trigger is; a conditional's own keyed lists are
        // settled above.
        let settled = settle::<crate::prototype::Action>("[Reaction]", Value::Object(map.clone()))
            .map_err(|e| format!("in `{at}`: {e}"))?;
        if let Value::Object(settled) = settled {
            *map = settled;
        }
        map.insert("id".into(), id);
    }
    Ok(())
}

/// A transition as written, completed from `base`: left out it is `base`, a word
/// is its kind, and an object's missing fields are `base`'s — except that a smart
/// animate named by its kind without a direction takes none, so its new layers
/// fade, as the prototype mode picks it. An object naming no kind keeps the
/// base's side too.
fn transition_in(given: Option<Value>, base: &Value) -> Value {
    let smart_without_side = |t: &mut Map<String, Value>, named: bool| {
        let smart = t
            .get("kind")
            .and_then(Value::as_str)
            .is_some_and(|k| k.eq_ignore_ascii_case("smartanimate"));
        if smart && !named {
            t.insert("direction".into(), Value::String("None".into()));
        }
    };
    match (given, base) {
        (None | Some(Value::Null), _) => base.clone(),
        (Some(Value::String(kind)), Value::Object(b)) => {
            let mut t = b.clone();
            t.insert("kind".into(), Value::String(kind));
            smart_without_side(&mut t, false);
            Value::Object(t)
        }
        (Some(Value::Object(fields)), Value::Object(b)) => {
            let named = fields.contains_key("direction") || !fields.contains_key("kind");
            let mut t = b.clone();
            t.extend(fields);
            smart_without_side(&mut t, named);
            Value::Object(t)
        }
        (Some(other), _) => other,
    }
}

/// Fill in a tree's `like`s: a layer with `"like": "<id>"` takes that layer's
/// fills, strokes, effects, opacity and blend where it gives none, and a text
/// its text style field by field and its paragraph. A text that gives its own
/// fills does not take the style's `color`, which would draw over them.
/// Refused when the sample has nothing to give: a text naming a layer that is
/// not one, or any layer naming one with no paint, which would otherwise draw
/// in the defaults without a word. Recurses into `children`.
fn expand_like(doc: &Document, v: &mut Value) -> Result<(), String> {
    const LOOK: [&str; 5] = ["fills", "strokes", "effects", "opacity", "blend"];
    let Value::Object(map) = v else {
        return Ok(());
    };
    if let Some(like) = map.remove("like") {
        let own_fills = map.contains_key("fills");
        let id = like
            .as_str()
            .and_then(NodeId::from_wire)
            .ok_or_else(|| format!("`like`: {like} is not a layer id"))?;
        let Some(Value::Object(src)) = full_tree(doc, id, true) else {
            return Err(format!("`like`: no layer `{}`", id.to_wire()));
        };
        let src_is_text = src.get("kind").and_then(|k| k.get("Text")).is_some();
        let is_text = map.get("kind").and_then(|k| k.get("Text")).is_some();
        let what = || {
            doc.get(id)
                .map_or("layer", |n| n.kind().default_name())
                .to_lowercase()
        };
        if is_text && !src_is_text {
            return Err(format!(
                "`like`: `{}` is a {}, not a text, so it has no text style to give; name a text",
                id.to_wire(),
                what()
            ));
        }
        if !src_is_text && !LOOK.iter().any(|k| src.contains_key(*k)) {
            return Err(format!(
                "`like`: `{}` is a {} with no fill, stroke or effect to give",
                id.to_wire(),
                what()
            ));
        }
        for k in LOOK {
            if let Some(val) = src.get(k) {
                map.entry(k).or_insert_with(|| val.clone());
            }
        }
        let src_text = src.get("kind").and_then(|k| k.get("Text"));
        let text = map.get_mut("kind").and_then(|k| k.get_mut("Text"));
        if let (Some(Value::Object(from)), Some(Value::Object(to))) = (src_text, text) {
            if let Some(Value::Object(src_style)) = from.get("style") {
                let style = to
                    .entry("style")
                    .or_insert_with(|| Value::Object(Map::new()));
                if let Value::Object(style) = style {
                    for (k, val) in src_style {
                        if own_fills && k == "color" {
                            continue;
                        }
                        style.entry(k.clone()).or_insert_with(|| val.clone());
                    }
                }
            }
            if let Some(p) = from.get("paragraph") {
                to.entry("paragraph").or_insert_with(|| p.clone());
            }
        }
    }
    if let Some(Value::Array(children)) = map.get_mut("children") {
        for child in children {
            expand_like(doc, child)?;
        }
    }
    Ok(())
}

/// The blend mode a [`BLENDS`] label names; Normal for none.
fn blend_of(label: &str) -> BlendMode {
    BlendMode::ALL
        .into_iter()
        .find(|m| m.label() == label)
        .unwrap_or(BlendMode::Normal)
}

/// Fill in a tree's copies: a layer whose `copy_of` names a document layer no
/// layer of the tree was read as takes what it leaves out from that layer, its
/// kind whole, and its children as copies of that layer's when it gives none,
/// so the fields it writes are its overrides. A copy taking a new layer's
/// defaults instead would override its main's look with nothing. Runs after
/// [`expand_like`], whose sample's look is the copy's own. Recurses into
/// `children`.
fn expand_copies(doc: &Document, v: &mut Value) {
    fn read_ids(v: &Value, out: &mut FxHashSet<String>) {
        if let Some(id) = v.get("id").and_then(Value::as_str) {
            out.insert(id.to_owned());
        }
        for child in v
            .get("children")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
        {
            read_ids(child, out);
        }
    }
    /// What a main has that is its own and not a copy's.
    const MAIN_ONLY: [&str; 10] = [
        "id",
        "component",
        "set",
        "variant",
        "lineage",
        "props",
        "description",
        "doc_link",
        "guides",
        "copy_of",
    ];
    /// `main`, a [`full_tree`] read, as a copy of it: each layer links to the
    /// one it was read from.
    fn as_copy(main: &mut Value) {
        let Value::Object(map) = main else { return };
        if let Some(Value::String(id)) = map.remove("id") {
            for k in MAIN_ONLY {
                map.remove(k);
            }
            map.insert("copy_of".into(), Value::String(id));
        }
        for child in map
            .get_mut("children")
            .and_then(Value::as_array_mut)
            .into_iter()
            .flatten()
        {
            as_copy(child);
        }
    }
    /// A copy's `own` kind completed from its main's `theirs` where both are the
    /// same variant: each field it leaves out, a text's style field by field.
    /// A text giving its own content takes none of the main's runs, which index
    /// the main's words.
    fn merge_kind(own: &mut Map<String, Value>, theirs: &Map<String, Value>) {
        let Some((variant, Value::Object(from))) = theirs.iter().next() else {
            return;
        };
        let Some(Value::Object(to)) = own.get_mut(variant) else {
            return;
        };
        let own_words = to.contains_key("content");
        for (k, val) in from {
            if own_words && matches!(k.as_str(), "spans" | "para_spans") {
                continue;
            }
            match (to.get_mut(k), val) {
                (Some(Value::Object(mine)), Value::Object(main)) if k == "style" => {
                    for (sk, sv) in main {
                        mine.entry(sk.clone()).or_insert_with(|| sv.clone());
                    }
                }
                (Some(_), _) => {}
                (None, _) => {
                    to.insert(k.clone(), val.clone());
                }
            }
        }
    }
    /// A copy's `own` kind written in any case, or a frame as `Frame`, named as
    /// its main's `theirs` is, so [`merge_kind`] meets the same variant.
    fn name_variant(own: &mut Map<String, Value>, theirs: &Map<String, Value>) {
        let (Some(variant), Some(written)) = (theirs.keys().next(), own.keys().next().cloned())
        else {
            return;
        };
        let want = plain(variant);
        let mine = plain(&written);
        if written != *variant
            && own.len() == 1
            && (mine == want || alias(&mine) == want)
            && let Some(inner) = own.remove(&written)
        {
            own.insert(variant.clone(), inner);
        }
    }
    /// `above` is the main of the copy this layer sits in, if any.
    fn walk(doc: &Document, read: &FxHashSet<String>, v: &mut Value, above: Option<NodeId>) {
        let Value::Object(map) = v else { return };
        let source = map
            .get("copy_of")
            .and_then(Value::as_str)
            .filter(|id| !read.contains(*id))
            .and_then(NodeId::from_wire)
            .filter(|id| doc.get(*id).is_some());
        let main = source.and_then(|id| full_tree(doc, id, false));
        if let Some(Value::Object(mut main)) = main {
            for k in MAIN_ONLY {
                main.remove(k);
            }
            // Its place is its main's only as a copy of that main's own child;
            // a layer copied in from elsewhere lands where a new layer does.
            let own_child = above.is_some()
                && source.and_then(|s| doc.get(s)).and_then(|n| n.parent()) == above;
            if !own_child {
                main.remove("transform");
            }
            if let (Some(Value::Object(own)), Some(Value::Object(theirs))) =
                (map.get_mut("kind"), main.get("kind"))
            {
                name_variant(own, theirs);
            }
            // `corner_radius`, the name `set` takes, is the copy's own corners,
            // not to lose to the main's merged in below.
            let own_corners = map
                .get("kind")
                .and_then(Value::as_object)
                .and_then(|k| k.values().next())
                .is_some_and(|inner| inner.get("corner_radii").is_some());
            let radius = (!own_corners)
                .then(|| map.remove("corner_radius"))
                .flatten();
            if let (Some(Value::Object(own)), Some(Value::Object(theirs))) =
                (map.get_mut("kind"), main.get("kind"))
            {
                merge_kind(own, theirs);
            }
            if let Some(r) = radius {
                map.entry("kind")
                    .or_insert_with(|| main.get("kind").cloned().unwrap_or_default());
                match map
                    .get_mut("kind")
                    .and_then(Value::as_object_mut)
                    .and_then(|k| k.iter_mut().next())
                {
                    Some((variant, Value::Object(inner)))
                        if matches!(variant.as_str(), "Rect" | "Artboard") =>
                    {
                        inner.insert("corner_radii".into(), r);
                    }
                    // No corners to take it: left for the read to refuse.
                    _ => {
                        map.insert("corner_radius".into(), r);
                    }
                }
            }
            if !map.contains_key("children")
                && let Some(children) = main.get_mut("children")
            {
                for child in children.as_array_mut().into_iter().flatten() {
                    as_copy(child);
                }
            }
            for (k, val) in main {
                map.entry(k).or_insert(val);
            }
        }
        for child in map
            .get_mut("children")
            .and_then(Value::as_array_mut)
            .into_iter()
            .flatten()
        {
            walk(doc, read, child, source);
        }
    }
    let mut read = FxHashSet::default();
    read_ids(v, &mut read);
    walk(doc, &read, v, None);
}

/// Why a script stopped: the step, counted from 1, and what went wrong there.
#[derive(thiserror::Error, Debug)]
#[error("step {step}: {why}")]
pub struct ScriptError {
    pub step: usize,
    pub why: String,
}

/// Run `script` against `doc`, each step committed before the next reads it: a list
/// of `{ "verb": name, "args": { … }, "as": label }`. A string argument `"@label"`
/// stands for the first thing an earlier step labelled `label` made, a node's wire
/// form or another thing's id ([`Done::made`]), and `"@page"` for the first page; a string
/// starting `@@` is written with one `@`. A step holding any other key is refused.
/// Answers what each step made, in those forms; on a refusal `doc` keeps the steps before it.
pub fn run_script(
    doc: &mut Document,
    ids: &mut IdSource,
    mut lent: Lent<'_>,
    script: &Value,
) -> Result<Vec<Vec<String>>, ScriptError> {
    let fail = |step: usize, why: String| ScriptError { step, why };
    let Value::Array(steps) = script else {
        return Err(fail(0, "a script is a list of steps".into()));
    };
    let mut labels: rustc_hash::FxHashMap<String, String> = rustc_hash::FxHashMap::default();
    labels.insert("page".into(), doc.first_page().to_wire());
    let mut out = Vec::new();
    for (i, step) in steps.iter().enumerate() {
        let n = i + 1;
        // A misspelled `args` would run the verb with none, at its widest.
        if let Some(extra) = step.as_object().and_then(|m| {
            m.keys()
                .find(|k| !["verb", "args", "as"].contains(&k.as_str()))
        }) {
            return Err(fail(n, format!("a step takes no `{extra}`")));
        }
        let verb = step
            .get("verb")
            .and_then(Value::as_str)
            .ok_or_else(|| fail(n, "a step names its `verb`".into()))?;
        let mut args = step.get("args").cloned().unwrap_or(Value::Null);
        substitute(&mut args, &labels).map_err(|why| fail(n, why))?;
        let res = Resolved::rebuild(doc);
        let done = call(
            &mut Ctx {
                doc,
                res: &res,
                ids,
                // A cast, not `as_deref_mut`: it shortens the object lifetime to
                // this step's borrow, which `&mut`'s invariance otherwise forbids.
                images: lent.images.as_mut().map(|s| &mut **s as &mut dyn ImageSink),
                contrast: lent
                    .contrast
                    .as_mut()
                    .map(|p| &mut **p as &mut dyn crate::contrast::Probe),
            },
            verb,
            &args,
        )
        .map_err(|e| fail(n, e.to_string()))?;
        // What the components owe the edit, as an import commits: instances follow
        // their mains and links settle.
        let mut tx = done.tx;
        crate::propagate::owed(doc, &mut tx, ids);
        doc.apply(&tx).map_err(|e| fail(n, e.to_string()))?;
        let made: Vec<String> = done
            .created
            .iter()
            .map(|id| id.to_wire())
            .chain(done.made)
            .collect();
        if let Some(label) = step.get("as").and_then(Value::as_str) {
            let first = made
                .first()
                .ok_or_else(|| fail(n, format!("`{verb}` makes nothing to call `{label}`")))?;
            labels.insert(label.to_owned(), first.clone());
        }
        out.push(made);
    }
    Ok(out)
}

/// Every `"@label"` string in `v` replaced by what its label stands for, and a
/// string starting `@@` by itself less one `@`.
pub fn substitute<S: std::hash::BuildHasher>(
    v: &mut Value,
    labels: &std::collections::HashMap<String, String, S>,
) -> Result<(), String> {
    match v {
        // `@@` escapes a literal `@`.
        Value::String(s) if s.starts_with("@@") => {
            s.remove(0);
        }
        Value::String(s) if s.starts_with('@') => {
            let made = labels
                .get(&s[1..])
                .ok_or_else(|| format!("no earlier step is called `{}`", &s[1..]))?;
            *s = made.clone();
        }
        Value::Array(list) => list.iter_mut().try_for_each(|v| substitute(v, labels))?,
        Value::Object(map) => map.values_mut().try_for_each(|v| substitute(v, labels))?,
        _ => {}
    }
    Ok(())
}
