//! Lists from data (`docs/design/lists.md`): a frame carrying a [`List`]
//! description repeats its first child, a component instance, once per row of
//! one of the document's [`DataSet`]s, each copy's component properties filled
//! from the row's cells.
//!
//! The copies are ordinary instances, saved and drawn as any layer is. They are
//! made again by [`regenerate`] whenever the template, the description or the
//! data set changes: in the app inside the edit's own undo step, so undo and
//! redo never meet a copy a later regeneration replaced; in play, on the
//! player's copy. A load draws the copies as saved.

use crate::chart::{ColumnId, DataLink, MAX_CELL_CHARS, MAX_COLUMNS, MAX_ROWS, Table};
use crate::document::Document;
use crate::id::{IdSource, NodeId};
use crate::item::ItemId;
use crate::op::{DirtySet, Operation, Transaction};
use crate::variant::{PropKind, PropValue};
use rustc_hash::FxHashSet;
use serde::{Deserialize, Deserializer, Serialize, Serializer};

/// The most rows a list draws, the template's included: each is an instance.
pub const MAX_DRAWN: usize = 200;
/// The longest a data set's name may be.
pub const MAX_NAME_CHARS: usize = 256;

/// A data set's identity in its document.
#[derive(Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct DataSetId(pub NodeId);

impl std::fmt::Debug for DataSetId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "data set {:?}", self.0)
    }
}

/// Written as the node id's wire form, `"<actor-hex>:<seq>"`.
impl Serialize for DataSetId {
    fn serialize<S: Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        s.serialize_str(&self.0.to_wire())
    }
}

impl<'de> Deserialize<'de> for DataSetId {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        let s = String::deserialize(d)?;
        NodeId::from_wire(&s)
            .map(DataSetId)
            .ok_or_else(|| serde::de::Error::custom(format!("bad id {s:?}")))
    }
}

impl IdSource {
    /// A fresh [`DataSetId`].
    pub fn mint_data_set(&mut self) -> DataSetId {
        DataSetId(self.mint())
    }
}

/// One table the document's lists read: its name, its rows, and the file it was
/// read from while linked.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct DataSet {
    pub id: DataSetId,
    pub name: String,
    pub table: Table,
    /// The file the table was read from, while linked. Never carried by a copy.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source: Option<DataLink>,
    /// The pictures the table's cells name: a cell's text (a path beside the
    /// data file) and the picture read from it into the document, filled where
    /// the file is read, since core reads no files. An image property's column
    /// fills from it, or from a cell naming a picture's id.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub pictures: Vec<(String, crate::ImageId)>,
}

impl DataSet {
    /// The picture `cell` names: one read for that cell's text, else the
    /// document's picture of that id.
    pub fn picture(&self, doc: &Document, cell: &str) -> Option<crate::ImageId> {
        let cell = cell.trim();
        if let Some((_, id)) = self.pictures.iter().find(|(c, _)| c == cell) {
            return Some(id.clone());
        }
        let id = crate::ImageId(cell.to_string());
        doc.image(&id).is_some().then_some(id)
    }
}

/// Why a data set or a list may not be stored.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ListRule {
    /// Past the table's limits, a ragged row, a cell or name too long.
    BadTable,
    /// A column id twice.
    DuplicateColumn,
    /// A list on anything but a frame.
    NotAFrame,
    /// A copy named twice, or the list's own frame named as its copy.
    BadCopies,
}

impl std::fmt::Display for ListRule {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            ListRule::BadTable => "a data set's table is past its limits or ragged",
            ListRule::DuplicateColumn => "a data set names a column twice",
            ListRule::NotAFrame => "a list sits only on a frame",
            ListRule::BadCopies => "a list names a copy twice",
        })
    }
}

impl DataSet {
    /// Whether this data set may be stored: the chart table's limits and shape.
    pub fn validate(&self) -> Result<(), ListRule> {
        if self.name.chars().count() > MAX_NAME_CHARS {
            return Err(ListRule::BadTable);
        }
        validate_table(&self.table)
    }
}

/// The chart's rule for a table, which a data set shares.
pub fn validate_table(t: &Table) -> Result<(), ListRule> {
    if t.rows.len() > MAX_ROWS || t.columns.len() > MAX_COLUMNS {
        return Err(ListRule::BadTable);
    }
    let mut ids = FxHashSet::default();
    for c in &t.columns {
        if !ids.insert(c.id) {
            return Err(ListRule::DuplicateColumn);
        }
        if c.name.chars().count() > MAX_CELL_CHARS {
            return Err(ListRule::BadTable);
        }
    }
    for r in &t.rows {
        if r.len() != t.columns.len() || r.iter().any(|c| c.chars().count() > MAX_CELL_CHARS) {
            return Err(ListRule::BadTable);
        }
    }
    Ok(())
}

/// A list's description, `Node::list` on a frame. Its first child that is not a
/// copy is the template.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct List {
    /// The data set it reads. Over one the document lacks (a list pasted from
    /// another document) the list keeps the rows it has until a data set is
    /// picked.
    pub data: DataSetId,
    #[serde(default)]
    pub rows: Rows,
    /// Which column fills which of the template's properties.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub fill: Vec<Fill>,
    /// The copies made by the last regeneration, in row order after the
    /// template; written only by [`regenerate`].
    #[serde(default, skip_serializing_if = "Vec::is_empty", with = "wire_ids")]
    pub copies: Vec<NodeId>,
    /// The row an [`Rows::Opened`] list shows: 0 in the document, the row
    /// opened while playing.
    #[serde(default, skip_serializing_if = "is_zero")]
    pub opened: usize,
}

fn is_zero(v: &usize) -> bool {
    *v == 0
}

mod wire_ids {
    use crate::id::NodeId;
    use serde::{Deserialize, Deserializer, Serialize, Serializer};

    pub fn serialize<S: Serializer>(ids: &[NodeId], s: S) -> Result<S::Ok, S::Error> {
        let wire: Vec<String> = ids.iter().map(|id| id.to_wire()).collect();
        wire.serialize(s)
    }

    pub fn deserialize<'de, D: Deserializer<'de>>(d: D) -> Result<Vec<NodeId>, D::Error> {
        Vec::<String>::deserialize(d)?
            .into_iter()
            .map(|id| {
                NodeId::from_wire(&id)
                    .ok_or_else(|| serde::de::Error::custom(format!("malformed node id {id:?}")))
            })
            .collect()
    }
}

/// Which rows a list draws.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Rows {
    /// Every row, up to [`MAX_DRAWN`].
    #[default]
    All,
    /// The first `n`.
    First(u32),
    /// The one row opened in play by a tap on a row of a list over the same data
    /// set: a detail screen. Row 1 on the canvas.
    Opened,
}

/// One column filling one property of the template.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Fill {
    pub target: Target,
    pub column: ColumnId,
}

/// What a column fills.
#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Target {
    /// A component property of the template's main, by its item id: a text
    /// takes the cell, a boolean a cell reading true, yes or 1.
    Prop(ItemId),
    /// A variant property of the template's set, by name: the copy switches to
    /// the variant whose value the cell names.
    Variant(String),
}

impl List {
    /// A list over `data` filling nothing, every row drawn.
    pub fn new(data: DataSetId) -> List {
        List {
            data,
            rows: Rows::All,
            fill: Vec::new(),
            copies: Vec::new(),
            opened: 0,
        }
    }

    /// Whether this description may be stored on `id`.
    pub fn validate(&self, id: NodeId) -> Result<(), ListRule> {
        let mut seen = FxHashSet::default();
        if !self.copies.iter().all(|c| *c != id && seen.insert(*c)) {
            return Err(ListRule::BadCopies);
        }
        Ok(())
    }

    /// The column filling `target`, if any.
    pub fn column_for(&self, target: &Target) -> Option<ColumnId> {
        self.fill
            .iter()
            .find(|f| f.target == *target)
            .map(|f| f.column)
    }

    /// The table rows this list draws, in order, given a table of `len` rows.
    pub fn shown_rows(&self, len: usize) -> Vec<usize> {
        match self.rows {
            Rows::All => (0..len.min(MAX_DRAWN)).collect(),
            Rows::First(n) => (0..len.min(n as usize).min(MAX_DRAWN)).collect(),
            Rows::Opened if len > 0 => vec![self.opened.min(len - 1)],
            Rows::Opened => Vec::new(),
        }
    }

    /// Whether `other` describes the same list, whatever copies either holds.
    pub fn same_description(&self, other: &List) -> bool {
        self.data == other.data
            && self.rows == other.rows
            && self.fill == other.fill
            && self.opened == other.opened
    }
}

/// The template of list `id`: its first child that is not a copy.
pub fn template(doc: &Document, id: NodeId) -> Option<NodeId> {
    let n = doc.get(id)?;
    let list = n.list()?;
    n.children()
        .iter()
        .copied()
        .find(|c| !list.copies.contains(c))
}

/// The list `id` is the template or a copy of, and its place: 0 for the
/// template, then the copies in order.
pub fn row_of(doc: &Document, id: NodeId) -> Option<(NodeId, usize)> {
    let parent = doc.get(id)?.parent?;
    let list = doc.get(parent)?.list()?;
    if template(doc, parent) == Some(id) {
        return Some((parent, 0));
    }
    let i = list.copies.iter().position(|c| *c == id)?;
    Some((parent, i + 1))
}

/// The list row the layer `id` sits in — the list, its place among the drawn
/// rows and the data set's row it shows — walking up from `id`.
pub fn row_at(doc: &Document, id: NodeId) -> Option<(NodeId, usize, usize)> {
    let mut at = Some(id);
    while let Some(n) = at.and_then(|a| doc.get(a)) {
        if let Some((list, place)) = row_of(doc, n.id) {
            let l = doc.get(list)?.list()?;
            let len = doc.data_set(l.data).map_or(0, |d| d.table.rows.len());
            let row = *l.shown_rows(len).get(place)?;
            return Some((list, place, row));
        }
        at = n.parent;
    }
    None
}

/// The list whose copy holds `id` — `id` itself or a layer inside it — so a
/// copy is picked as its list, never edited apart from its template.
pub fn copy_owner(doc: &Document, id: NodeId) -> Option<NodeId> {
    let mut at = Some(id);
    while let Some(n) = at.and_then(|a| doc.get(a)) {
        if let Some(p) = n.parent
            && doc
                .get(p)
                .and_then(|p| p.list())
                .is_some_and(|l| l.copies.contains(&n.id))
        {
            return Some(p);
        }
        at = n.parent;
    }
    None
}

/// Every list frame in the document, in id order.
pub fn lists(doc: &Document) -> Vec<NodeId> {
    let mut out: Vec<NodeId> = doc
        .node_map()
        .values()
        .filter(|n| n.list().is_some())
        .map(|n| n.id)
        .collect();
    out.sort();
    out
}

/// The lists an applied transaction `tx` left out of date: its description or
/// its data set written, or a layer of its template changed (`dirty`, the
/// transaction's outcome) — a main's edit reaches the template through the
/// propagation, so it counts too.
pub fn touched(doc: &Document, tx: &Transaction, dirty: &DirtySet) -> Vec<NodeId> {
    let all = lists(doc);
    if all.is_empty() {
        return Vec::new();
    }
    let mut out: FxHashSet<NodeId> = FxHashSet::default();
    let mut sets: FxHashSet<DataSetId> = FxHashSet::default();
    for op in &tx.0 {
        match op {
            Operation::SetList { id, .. } => {
                out.insert(*id);
            }
            Operation::SetDataSet { id, .. } => {
                sets.insert(*id);
            }
            _ => {}
        }
    }
    if !sets.is_empty() {
        for l in &all {
            if doc
                .get(*l)
                .and_then(|n| n.list())
                .is_some_and(|x| sets.contains(&x.data))
            {
                out.insert(*l);
            }
        }
    }
    for id in &dirty.0 {
        let mut at = *id;
        while let Some(n) = doc.get(at) {
            let Some(p) = n.parent else { break };
            if doc.get(p).is_some_and(|p| p.list().is_some()) {
                if template(doc, p) == Some(at) {
                    out.insert(p);
                }
                break;
            }
            at = p;
        }
    }
    let mut out: Vec<NodeId> = out
        .into_iter()
        .filter(|l| doc.get(*l).is_some_and(|n| n.list().is_some()))
        .collect();
    out.sort();
    out
}

/// A cell read as a boolean: true, yes, on or 1 is shown; false, no, off, 0 or
/// empty is hidden; anything else leaves the property as the template has it.
pub fn cell_bool(cell: &str) -> Option<bool> {
    match cell.trim().to_ascii_lowercase().as_str() {
        "true" | "yes" | "on" | "1" | "y" => Some(true),
        "false" | "no" | "off" | "0" | "n" | "" => Some(false),
        _ => None,
    }
}

/// The variant properties the template `root`'s set offers, by name, with each
/// one's values.
pub fn variant_props(doc: &Document, root: NodeId) -> Vec<crate::variant::VariantProp> {
    crate::component::main_of(doc, root)
        .and_then(|m| crate::variant::set_of(doc, m))
        .and_then(|s| doc.get(s))
        .and_then(|s| s.set().cloned())
        .map(|s| s.props)
        .unwrap_or_default()
}

/// The rows of `list` whose cell in a variant fill names no value of its
/// property: (property name, the cells that name none), for the card's warning.
pub fn unmatched_variants(doc: &Document, list_id: NodeId) -> Vec<(String, Vec<String>)> {
    let Some(list) = doc.get(list_id).and_then(|n| n.list()) else {
        return Vec::new();
    };
    let Some(set) = doc.data_set(list.data) else {
        return Vec::new();
    };
    let Some(t) = template(doc, list_id) else {
        return Vec::new();
    };
    let props = variant_props(doc, t);
    let rows = list.shown_rows(set.table.rows.len());
    let mut out = Vec::new();
    for f in &list.fill {
        let Target::Variant(name) = &f.target else {
            continue;
        };
        let Some(p) = props.iter().find(|p| p.name == *name) else {
            continue;
        };
        let Some(col) = set.table.index_of(f.column) else {
            continue;
        };
        let mut missing: Vec<String> = Vec::new();
        for r in &rows {
            let cell = set.table.rows[*r][col].trim();
            if !p.values.iter().any(|v| v == cell) && !missing.iter().any(|m| m == cell) {
                missing.push(cell.to_string());
            }
        }
        if !missing.is_empty() {
            out.push((name.clone(), missing));
        }
    }
    out
}

/// The cells of `list`'s drawn rows that fill an image property but name no
/// picture: (property name, the cells), for the card's warning.
pub fn unmatched_pictures(doc: &Document, list_id: NodeId) -> Vec<(String, Vec<String>)> {
    let Some(list) = doc.get(list_id).and_then(|n| n.list()) else {
        return Vec::new();
    };
    let (Some(set), Some(t)) = (doc.data_set(list.data), template(doc, list_id)) else {
        return Vec::new();
    };
    let props = crate::variant::instance_properties(doc, t);
    let rows = list.shown_rows(set.table.rows.len());
    let mut out = Vec::new();
    for f in &list.fill {
        let Target::Prop(item) = &f.target else {
            continue;
        };
        let Some(p) = props
            .iter()
            .find(|p| p.id == *item && p.kind == PropKind::Image)
        else {
            continue;
        };
        let Some(col) = set.table.index_of(f.column) else {
            continue;
        };
        let mut missing: Vec<String> = Vec::new();
        for r in &rows {
            let cell = set.table.rows[*r][col].trim();
            if !cell.is_empty()
                && set.picture(doc, cell).is_none()
                && !missing.iter().any(|m| m == cell)
            {
                missing.push(cell.to_string());
            }
        }
        if !missing.is_empty() {
            out.push((p.name.clone(), missing));
        }
    }
    out
}

/// Each property of the template a column fills by default: one whose name
/// matches the column's, ignoring case, spaces, dashes and underscores. Text and
/// boolean component properties, then the set's variant properties.
pub fn fill_by_name(doc: &Document, template: NodeId, table: &Table) -> Vec<Fill> {
    let key = |s: &str| -> String {
        s.chars()
            .filter(|c| !matches!(c, ' ' | '-' | '_'))
            .flat_map(char::to_lowercase)
            .collect()
    };
    let column = |name: &str| {
        let k = key(name);
        table
            .columns
            .iter()
            .find(|c| key(&c.name) == k)
            .map(|c| c.id)
    };
    let mut out = Vec::new();
    for p in crate::variant::instance_properties(doc, template) {
        if matches!(p.kind, PropKind::Text | PropKind::Boolean | PropKind::Image)
            && let Some(c) = column(&p.name)
        {
            out.push(Fill {
                target: Target::Prop(p.id),
                column: c,
            });
        }
    }
    for p in variant_props(doc, template) {
        if let Some(c) = column(&p.name) {
            out.push(Fill {
                target: Target::Variant(p.name.clone()),
                column: c,
            });
        }
    }
    out
}

/// *New table*'s table for `template`: one column per text, boolean and image
/// property, then per variant property, each named after it, and one row
/// holding the instance's own values (`true`/`false` for a boolean, a
/// picture's id, the value a variant shows), so [`fill_by_name`] fills every
/// property from it.
pub fn seed_table(doc: &Document, template: NodeId) -> Table {
    let mut table = Table::default();
    let mut row = Vec::new();
    let mut push = |table: &mut Table, name: &str, value: String| {
        let id = table.next_column_id();
        table.columns.push(crate::chart::Column {
            id,
            name: name.chars().take(MAX_CELL_CHARS).collect(),
        });
        row.push(value.chars().take(MAX_CELL_CHARS).collect::<String>());
    };
    for p in crate::variant::instance_properties(doc, template) {
        if table.columns.len() >= MAX_COLUMNS {
            break;
        }
        let value = match crate::variant::property_state(doc, template, &p).map(|(v, _)| v) {
            Some(PropValue::Text(t)) if p.kind == PropKind::Text => t,
            Some(PropValue::Boolean(b)) if p.kind == PropKind::Boolean => b.to_string(),
            // A picture by its id, which a cell may name.
            Some(PropValue::Image(id)) if p.kind == PropKind::Image => id.0,
            None if matches!(p.kind, PropKind::Text | PropKind::Boolean | PropKind::Image) => {
                String::new()
            }
            _ => continue,
        };
        push(&mut table, &p.name, value);
    }
    let shown = crate::component::main_of(doc, template)
        .and_then(|m| doc.get(m))
        .map(|m| m.variant().to_vec())
        .unwrap_or_default();
    for (i, p) in variant_props(doc, template).into_iter().enumerate() {
        if table.columns.len() >= MAX_COLUMNS {
            break;
        }
        push(
            &mut table,
            &p.name,
            shown.get(i).cloned().unwrap_or_default(),
        );
    }
    table.rows.push(row);
    table
}

/// Where a new list's data comes from.
pub enum Source {
    /// A data set the document holds.
    Existing(DataSetId),
    /// A new data set, added in the same step.
    New(DataSet),
}

/// Why `id` cannot become a list's template, in the words the UI shows; `None`
/// when it can.
pub fn why_not(doc: &Document, id: NodeId) -> Option<&'static str> {
    let n = doc.get(id)?;
    if n.link().is_none() || crate::component::instance_root(doc, id) != Some(id) {
        return Some("Select an instance: a list fills a component's properties");
    }
    let mut at = n.parent;
    while let Some(p) = at.and_then(|p| doc.get(p)) {
        if p.component
            || p.set().is_some()
            || p.link().is_some()
            || p.list().is_some()
            || p.chart().is_some()
        {
            return Some("A list can't go inside a component, an instance or another list");
        }
        at = p.parent;
    }
    if doc.in_vault(id) {
        return Some("Select an instance on a page");
    }
    None
}

/// The list frames standing where a list may not: a list frame that is a main,
/// a set or an instance, or sits inside one, a chart, another list or the vault.
/// [`why_not`]'s rule over the whole document, so a list frame moved or wrapped
/// into a main is caught as *Make a list* catches an instance there.
pub fn misplaced(
    nodes: &rustc_hash::FxHashMap<NodeId, crate::node::Node>,
    vault: Option<NodeId>,
) -> Vec<NodeId> {
    let mut out: Vec<NodeId> = nodes
        .values()
        .filter(|n| n.list.is_some())
        .filter(|n| {
            if n.component || n.set.is_some() || n.link.is_some() {
                return true;
            }
            let mut at = n.parent.and_then(|p| nodes.get(&p));
            while let Some(p) = at {
                if p.component
                    || p.set.is_some()
                    || p.link.is_some()
                    || p.list.is_some()
                    || p.chart.is_some()
                    || Some(p.id) == vault
                {
                    return true;
                }
                at = p.parent.and_then(|q| nodes.get(&q));
            }
            false
        })
        .map(|n| n.id)
        .collect();
    out.sort();
    out
}

/// *Make a list from data*: instance `id` wrapped in a new frame flowing top to
/// bottom with an 8 px gap and hugging its rows, named after its component, the
/// frame given a list over `source` whose columns fill the properties of the
/// same name. The rows are made when the transaction is committed
/// ([`touched`], [`regenerate`]). Answers the transaction and the new frame.
pub fn make(
    doc: &Document,
    res: &crate::resolve::Resolved,
    ids: &mut IdSource,
    id: NodeId,
    source: Source,
) -> Result<(Transaction, NodeId), String> {
    use crate::container::{AlignItems, Dimension, Display, Flex, FlexDirection};
    if let Some(why) = why_not(doc, id) {
        return Err(why.to_string());
    }
    let (mut tx, frame) = crate::build::frame(doc, res, ids, &[id]).map_err(|e| e.to_string())?;
    let name = crate::component::main_of(doc, id)
        .map(|m| crate::variant::set_of(doc, m).unwrap_or(m))
        .and_then(|m| doc.get(m))
        .map_or_else(|| "List".to_string(), |m| format!("{} list", m.name()));
    tx.0.push(Operation::SetName { id: frame, name });
    tx.0.push(Operation::SetDisplay {
        id: frame,
        display: Some(Display::Flex(Flex {
            direction: FlexDirection::Column,
            align_items: AlignItems::Start,
            row_gap: 8.0,
            ..Flex::default()
        })),
    });
    tx.0.push(Operation::SetLayoutItem {
        id: frame,
        item: crate::container::LayoutItem {
            width: Dimension::FitContent,
            height: Dimension::FitContent,
            ..Default::default()
        },
    });
    let (data, table) = match source {
        Source::Existing(d) => {
            let set = doc.data_set(d).ok_or("That data set is gone")?;
            (d, set.table.clone())
        }
        Source::New(set) => {
            let d = set.id;
            let table = set.table.clone();
            tx.0.push(Operation::SetDataSet {
                id: d,
                set: Some(Box::new(set)),
            });
            (d, table)
        }
    };
    let mut list = List::new(data);
    list.fill = fill_by_name(doc, id, &table);
    tx.0.push(Operation::SetList {
        id: frame,
        list: Some(Box::new(list)),
    });
    Ok((tx, frame))
}

/// *Detach list*: the description dropped from each of `ids`, its copies kept
/// as plain instances.
pub fn detach(doc: &Document, ids: &[NodeId]) -> Transaction {
    Transaction(
        ids.iter()
            .filter(|id| doc.get(**id).is_some_and(|n| n.list().is_some()))
            .map(|id| Operation::SetList {
                id: *id,
                list: None,
            })
            .collect(),
    )
}

/// A data set's name for a file at `path`: its file name.
pub fn name_for_path(path: &std::path::Path) -> String {
    path.file_name()
        .map(|f| f.to_string_lossy().into_owned())
        .unwrap_or_else(|| "Data".to_string())
}

/// A name no data set in `doc` has: `wanted`, else numbered past it.
pub fn free_name(doc: &Document, wanted: &str) -> String {
    let taken: FxHashSet<&str> = doc.data_sets().iter().map(|d| d.name.as_str()).collect();
    if !taken.contains(wanted) {
        return wanted.to_owned();
    }
    match crate::naming::numbered(wanted) {
        Some((stem, n)) => crate::naming::free_name(&taken, stem, n.saturating_add(1)),
        None => crate::naming::free_name(&taken, wanted, 2),
    }
}

/// Bring list `id`'s copies to what its description and data set make of its
/// template, handing each step to `apply` in order: the old copies taken out
/// and the new ones made, then each row's variant switched, then each row's
/// properties filled. Each step is settled by `propagate::owed` before it is
/// handed over, as a commit would. `false` when `id` is not a list, or its data
/// set or template is missing, and nothing was handed over.
pub fn regenerate<E>(
    doc: &mut Document,
    id: NodeId,
    ids: &mut IdSource,
    mut apply: impl FnMut(&mut Document, Transaction) -> Result<(), E>,
) -> Result<bool, E> {
    let mut step = |doc: &mut Document, ids: &mut IdSource, ops: Vec<Operation>| {
        if ops.is_empty() {
            return Ok(());
        }
        let mut tx = Transaction(ops);
        crate::propagate::owed(doc, &mut tx, ids);
        apply(doc, tx)
    };
    let Some(list) = doc.get(id).and_then(|n| n.list()).cloned() else {
        return Ok(false);
    };
    let Some(set) = doc.data_set(list.data).cloned() else {
        return Ok(false);
    };
    let Some(tpl) = template(doc, id) else {
        return Ok(false);
    };
    let rows = list.shown_rows(set.table.rows.len());

    // The old copies out, then the new ones in after the template, the list
    // naming them in the same step.
    let gone: Vec<Operation> = list
        .copies
        .iter()
        .filter(|c| doc.get(**c).is_some_and(|n| n.parent == Some(id)))
        .map(|c| Operation::DeleteNode { id: *c })
        .collect();
    step(doc, ids, gone)?;
    let Some(captured) = doc.capture_subtree(tpl) else {
        return Ok(false);
    };
    let at = doc
        .get(id)
        .and_then(|n| n.children().iter().position(|c| *c == tpl))
        .map_or(1, |i| i + 1);
    let placements: Vec<crate::build::Placement> = (1..rows.len())
        .map(|_| crate::build::Placement {
            nodes: captured.clone(),
            parent: id,
            index: Some(at),
        })
        .collect();
    let (tx, made) = crate::build::insert_subtrees(doc, ids, &placements, kurbo::Vec2::ZERO);
    let mut ops = tx.0;
    ops.push(Operation::SetList {
        id,
        list: Some(Box::new(List {
            copies: made.clone(),
            ..list.clone()
        })),
    });
    step(doc, ids, ops)?;

    let roots: Vec<(NodeId, usize)> = std::iter::once(tpl)
        .chain(made.iter().copied())
        .zip(rows.iter().copied())
        .collect();
    let cell = |row: usize, col: ColumnId| -> Option<&str> {
        let i = set.table.index_of(col)?;
        set.table.rows.get(row)?.get(i).map(|c| c.trim())
    };

    // Each row switched to the variant its cells name, from the variant it shows.
    let mut switches = Vec::new();
    for (root, row) in &roots {
        let Some(main) = crate::component::main_of(doc, *root) else {
            continue;
        };
        let Some(values) = doc.get(main).map(|m| m.variant().to_vec()) else {
            continue;
        };
        let props = variant_props(doc, *root);
        let mut want = values.clone();
        for f in &list.fill {
            let Target::Variant(name) = &f.target else {
                continue;
            };
            let (Some(i), Some(c)) = (
                props.iter().position(|p| p.name == *name),
                cell(*row, f.column),
            ) else {
                continue;
            };
            if props[i].values.iter().any(|v| v == c)
                && let Some(w) = want.get_mut(i)
            {
                *w = c.to_string();
            }
        }
        if want == values {
            continue;
        }
        let Some(to) = crate::variant::set_of(doc, main)
            .and_then(|s| crate::variant::find_variant(doc, s, &want))
        else {
            continue;
        };
        if let Some(tx) = crate::variant::switch(doc, *root, to, ids) {
            switches.extend(tx.0);
        }
    }
    step(doc, ids, switches)?;

    // Then each row's text and boolean properties.
    let mut props = Vec::new();
    for (root, row) in &roots {
        let offered = crate::variant::instance_properties(doc, *root);
        for f in &list.fill {
            let Target::Prop(item) = &f.target else {
                continue;
            };
            let (Some(p), Some(c)) = (offered.iter().find(|p| p.id == *item), cell(*row, f.column))
            else {
                continue;
            };
            let value = match p.kind {
                PropKind::Text => PropValue::Text(c.to_string()),
                PropKind::Boolean => match cell_bool(c) {
                    Some(b) => PropValue::Boolean(b),
                    None => continue,
                },
                // A cell naming no picture leaves the row the template's.
                PropKind::Image => match set.picture(doc, c) {
                    Some(id) => PropValue::Image(id),
                    None => continue,
                },
                PropKind::Swap | PropKind::Nested => continue,
            };
            props.extend(crate::variant::set_property(doc, &[*root], p, &value));
        }
    }
    step(doc, ids, props)?;
    Ok(true)
}

/// The charts reading data set `set`, in id order.
pub fn charts_over(doc: &Document, set: DataSetId) -> Vec<NodeId> {
    let mut out: Vec<NodeId> = doc
        .node_map()
        .values()
        .filter(|n| n.chart().is_some_and(|c| c.data == Some(set)))
        .map(|n| n.id)
        .collect();
    out.sort();
    out
}

/// The `SetChart`s that bring every chart an applied transaction `tx` left
/// behind its data set back onto that set's table ([`crate::chart::Chart::reading`]):
/// the charts it moved onto a set and those over a set it wrote. A chart whose
/// set is missing keeps its table.
pub fn chart_follow_ops(doc: &Document, tx: &Transaction) -> Vec<Operation> {
    let mut charts: Vec<NodeId> = Vec::new();
    for op in &tx.0 {
        match op {
            Operation::SetChart { id, chart: Some(_) } => charts.push(*id),
            Operation::SetDataSet { id, .. } => charts.extend(charts_over(doc, *id)),
            _ => {}
        }
    }
    charts.sort();
    charts.dedup();
    charts
        .into_iter()
        .filter_map(|id| {
            let chart = doc.get(id)?.chart()?;
            let data = chart.data?;
            let set = doc.data_set(data)?;
            (chart.table != set.table).then(|| Operation::SetChart {
                id,
                chart: Some(Box::new(chart.reading(data, &set.table))),
            })
        })
        .collect()
}

/// After `history` committed a step from `tx` that changed `dirty`: every chart
/// over a data set brought onto its table ([`chart_follow_ops`]), then every
/// list it left out of date ([`touched`]) regenerated, each step committed and
/// joined onto that step, so one undo takes the edit and its rows back
/// together. What the regenerations changed is added to `dirty`.
pub fn regenerate_in_history(
    history: &mut crate::history::History,
    doc: &mut Document,
    ids: &mut IdSource,
    tx: &Transaction,
    dirty: &mut DirtySet,
) -> Result<(), crate::op::OpError> {
    let follow = chart_follow_ops(doc, tx);
    if !follow.is_empty() {
        let d = history.commit(doc, Transaction(follow))?;
        history.join_last(2);
        dirty.0.extend(d.0);
    }
    for l in touched(doc, tx, dirty) {
        regenerate(doc, l, ids, |doc, step| {
            let d = history.commit(doc, step)?;
            history.join_last(2);
            dirty.0.extend(d.0);
            Ok(())
        })?;
    }
    Ok(())
}

/// Whether any list or chart reads data set `set`.
pub fn is_read(doc: &Document, set: DataSetId) -> bool {
    doc.node_map().values().any(|n| {
        n.list().is_some_and(|x| x.data == set) || n.chart().is_some_and(|c| c.data == Some(set))
    })
}

/// The actor a list's copies are minted under by
/// [`regenerate_outside_history`], past every id the document holds: one per
/// list, so the same document always regenerates to the same ids.
pub fn copy_actor(list: NodeId) -> u64 {
    let mut z = (list.actor ^ list.seq.rotate_left(32)) ^ 0x115D_0000_0000_0000;
    z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
    z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
    z ^= z >> 31;
    z.max(1)
}

/// Every list in `lists` regenerated straight onto `doc`, outside history: for
/// a document built without an editing session, such as a test's. Answers what
/// changed.
pub fn regenerate_outside_history(doc: &mut Document, lists: &[NodeId]) -> DirtySet {
    let mut all = DirtySet::default();
    for l in lists {
        let mut ids = IdSource::new(copy_actor(*l));
        crate::document::reserve_existing_ids(doc, &mut ids);
        let done = regenerate(doc, *l, &mut ids, |doc, tx| {
            doc.apply(&tx).map(|o| all.0.extend(o.dirty.0))
        });
        // A regeneration the rules refuse leaves the list as it was: a bug in
        // the generator, never the user's to see.
        if let Err(e) = done {
            debug_assert!(false, "a list's regeneration was refused: {e}");
        }
    }
    all
}

#[cfg(test)]
mod tests {
    use super::*;

    /// `shown_rows` caps every mode at the table and at `MAX_DRAWN`.
    #[test]
    fn shown_rows_follow_the_mode() {
        let mut l = List::new(DataSetId(NodeId { actor: 1, seq: 1 }));
        assert_eq!(l.shown_rows(3), vec![0, 1, 2]);
        assert_eq!(l.shown_rows(500).len(), MAX_DRAWN);
        l.rows = Rows::First(2);
        assert_eq!(l.shown_rows(3), vec![0, 1]);
        l.rows = Rows::Opened;
        l.opened = 7;
        assert_eq!(l.shown_rows(3), vec![2]);
        assert!(l.shown_rows(0).is_empty());
    }

    /// A boolean cell reads the usual spellings and leaves anything else alone.
    #[test]
    fn cells_read_as_booleans() {
        assert_eq!(cell_bool(" Yes"), Some(true));
        assert_eq!(cell_bool("0"), Some(false));
        assert_eq!(cell_bool(""), Some(false));
        assert_eq!(cell_bool("maybe"), None);
    }
}
