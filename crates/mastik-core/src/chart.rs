//! Charts (`docs/design/charts.md`): a frame carrying a [`Chart`] description,
//! whose children are ordinary shapes and text generated from it.
//!
//! This module holds the description and the rules that keep its parts derived:
//! [`guard`] refuses an operation that writes a part outside the chart's own
//! regeneration, and [`check`] refuses a chart where one may not sit. A
//! regeneration is a transaction carrying [`crate::Operation::SetChartParts`] for
//! its chart; nothing else writes under a chart frame.

mod colours;
mod generate;
mod preset;
pub mod scale;
mod settle;
pub mod table_io;

pub use colours::{
    bound_variables, by_category, named_marks, place_fill, sankey_nodes, sankey_totals,
    series_fill, series_fill_set, unset_starting, with_node_fill, with_palette, with_series_fill,
    with_variables,
};
pub use generate::{
    Generated, Part, Refusal, category_order, generate, generate_on, palette, waterfall_steps,
};
pub use preset::{OUTLINE_WIDTH, Preset};
pub(crate) use settle::part_actor;
pub use settle::{settle, settle_all, settle_round, stale};
pub use table_io::grouped;

use crate::id::NodeId;
use crate::node::{Node, NodeKind};
use crate::op::Operation;
use crate::variable::BrushSource;
use rustc_hash::{FxHashMap, FxHashSet};
use serde::{Deserialize, Serialize};

/// The most rows a table holds.
pub const MAX_ROWS: usize = 10_000;
/// The most columns a table holds.
pub const MAX_COLUMNS: usize = 64;
/// The most characters in one cell.
pub const MAX_CELL_CHARS: usize = 256;
/// The most bars, points or slices a chart draws; past it, it draws nothing new.
pub const MAX_MARKS: usize = 2_000;
/// The most slices a pie draws.
pub const MAX_SLICES: usize = 100;
/// The most nodes a Sankey draws.
pub const MAX_SANKEY_NODES: usize = 200;
/// The most flows a Sankey draws.
pub const MAX_FLOWS: usize = 1_000;
/// The longest a chart's text option (an axis title, a family, a prefix) may be.
pub const MAX_TEXT_CHARS: usize = 256;

/// A chart's description: what it draws and how. `Node::chart` on a frame.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Chart {
    pub kind: ChartKind,
    pub table: Table,
    #[serde(default)]
    pub mapping: Mapping,
    /// One per series column, keyed by its column, so a reorder or a rename keeps
    /// its colour.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub series: Vec<SeriesStyle>,
    /// The colours of the marks a chart names rather than numbers by series (a
    /// Sankey's nodes, a pie's slices, a scatter's groups: `named_marks`),
    /// keyed by name, so a re-import keeps them.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub nodes: Vec<NodeStyle>,
    /// Every kind's options at once, so switching kind and back keeps them.
    #[serde(default)]
    pub options: Options,
    /// The file the table was read from, while linked. Never carried by a copy.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source: Option<DataLink>,
    /// The document's data set the chart reads (`lists.md`), in place of a
    /// table of its own: `table` is then that set's, copied in by the commit
    /// that changes either (`list::regenerate_in_history`), and kept as drawn
    /// when the set is missing.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub data: Option<crate::list::DataSetId>,
    /// The generated nodes by role, written only by a regeneration. Ids in the
    /// file's own spelling (`NodeId::to_wire`).
    #[serde(default, skip_serializing_if = "Vec::is_empty", with = "wire_parts")]
    pub parts: Vec<(PartKey, NodeId)>,
    /// The size the parts were generated at, written with them; `None` once the
    /// description changes, so the next settle regenerates ([`stale`]).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub generated: Option<[f64; 2]>,
    /// The ground the parts were generated on ([`ground_of`]'s colour), written
    /// with them; settle regenerates when the ground under the chart differs,
    /// and the card reads the derived chrome off it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub grounded: Option<peniko::Color>,
    /// The highest sequence number a regeneration has given a part, raised by
    /// every `SetChartParts` and never lowered, so a part id a regeneration
    /// deleted is never minted again for another part.
    #[serde(default, skip_serializing_if = "is_zero_u64")]
    pub minted: u64,
    /// The marks drawn as they are, the rest muted or faded: by name, so a
    /// re-import or a reorder keeps it. Belongs to this chart, not its data.
    #[serde(default, skip_serializing_if = "is_default")]
    pub highlight: Highlight,
    /// Lines and bands on an axis, at a value or one the data gives.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub references: Vec<Reference>,
}

/// The most references a chart holds.
pub const MAX_REFERENCES: usize = 16;
/// The most colours a *From variables* palette holds.
pub const MAX_PALETTE_GROUP: usize = MAX_COLUMNS;

/// What a highlight names, by name.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub enum Highlighted {
    #[default]
    None,
    /// Categories by their cell's text; a pie's slices.
    Categories(Vec<String>),
    /// Series by their column's name; a grouped scatter's groups.
    Series(Vec<String>),
}

/// How the marks a highlight leaves out draw.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum Others {
    /// In one colour, derived from the ground unless set.
    #[default]
    Muted,
    /// In their own colour at [`FADED`] of its alpha.
    Faded,
}

/// The share of its alpha a faded mark keeps.
pub const FADED: f32 = 0.3;

/// The red a new reference and a switched-on *Colour below zero* start in.
pub const ALERT_RED: peniko::Color = peniko::Color::from_rgb8(0xC0, 0x39, 0x2B);

/// The light grey a switched-on plot background starts in.
pub const PLOT_GREY: peniko::Color = peniko::Color::from_rgb8(0xF2, 0xF3, 0xF5);

/// A chart's highlight (`Chart::highlight`).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Highlight {
    pub target: Highlighted,
    pub others: Others,
    /// The muted colour; `None` is the ink at 25% over the ground.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub muted: Option<BrushSource>,
    /// Value labels on the highlighted marks only, while something is
    /// highlighted.
    pub values_only: bool,
}

impl Default for Highlight {
    fn default() -> Self {
        Self {
            target: Highlighted::None,
            others: Others::Muted,
            muted: None,
            values_only: true,
        }
    }
}

/// A reference's shape.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum RefKind {
    #[default]
    Line,
    /// From `at` to `to`.
    Band,
}

/// The axis a reference stands on.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum RefAxis {
    #[default]
    Value,
    Category,
}

/// A statistic over the series shown, or one series.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum Stat {
    #[default]
    Average,
    Median,
    Min,
    Max,
}

/// Where a reference, or one end of a band, stands.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub enum At {
    /// A typed value; on a scatter's X axis, a number.
    Value(f64),
    /// A category, by name, at its band's centre.
    Category(String),
    /// A statistic of the values drawn: over every series shown, or one.
    Stat {
        stat: Stat,
        of: Option<ColumnId>,
    },
    AxisStart,
    AxisEnd,
}

impl Default for At {
    fn default() -> Self {
        At::Value(0.0)
    }
}

/// Where a reference's label sits along it.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum LabelAt {
    Start,
    Middle,
    #[default]
    End,
}

/// A line or band on an axis (`Chart::references`).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Reference {
    /// Its own number, which its parts are keyed by; unique in the chart.
    pub id: u32,
    pub kind: RefKind,
    pub axis: RefAxis,
    pub at: At,
    /// A band's far end.
    pub to: At,
    /// `{value}` writes the value it stands at; empty draws no label.
    pub label: String,
    pub colour: BrushSource,
    pub width: f64,
    pub dash: Dash,
    pub label_at: LabelAt,
    /// A typed value outside Auto's range widens it to hold the line.
    pub in_range: bool,
    pub visible: bool,
}

impl Default for Reference {
    fn default() -> Self {
        Self {
            id: 0,
            kind: RefKind::Line,
            axis: RefAxis::Value,
            at: At::Value(0.0),
            to: At::AxisEnd,
            label: "Reference".into(),
            colour: BrushSource::Literal(crate::Brush::Solid(ALERT_RED)),
            width: 1.5,
            dash: Dash::Dashed,
            label_at: LabelAt::End,
            in_range: true,
            visible: true,
        }
    }
}

/// `serde(with)` for [`Chart::parts`]: each id as the save format spells a node
/// id, so a part reads like every other reference in the file.
mod wire_parts {
    use super::{NodeId, PartKey};
    use serde::{Deserialize, Deserializer, Serialize, Serializer};

    pub fn serialize<S: Serializer>(parts: &[(PartKey, NodeId)], s: S) -> Result<S::Ok, S::Error> {
        let wire: Vec<(PartKey, String)> = parts.iter().map(|(k, id)| (*k, id.to_wire())).collect();
        wire.serialize(s)
    }

    pub fn deserialize<'de, D: Deserializer<'de>>(
        d: D,
    ) -> Result<Vec<(PartKey, NodeId)>, D::Error> {
        let wire = Vec::<(PartKey, String)>::deserialize(d)?;
        wire.into_iter()
            .map(|(k, id)| {
                NodeId::from_wire(&id)
                    .map(|id| (k, id))
                    .ok_or_else(|| serde::de::Error::custom(format!("malformed node id {id:?}")))
            })
            .collect()
    }
}

/// The kinds a chart can be.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum ChartKind {
    #[default]
    Column,
    Bar,
    Line,
    Area,
    Pie,
    Scatter,
    Sankey,
    /// A running total walked through its gains and losses (`Options::waterfall`).
    Waterfall,
    /// A line chart's table drawn round a centre, a spoke per category
    /// (`Options::radar`).
    Radar,
}

impl ChartKind {
    /// In the rail's and the Type dropdown's order: the bar family, the line
    /// family, the round one, then the specialist kinds.
    pub const ALL: [ChartKind; 9] = [
        ChartKind::Column,
        ChartKind::Bar,
        ChartKind::Line,
        ChartKind::Area,
        ChartKind::Pie,
        ChartKind::Scatter,
        ChartKind::Sankey,
        ChartKind::Waterfall,
        ChartKind::Radar,
    ];

    /// The kind's name as the UI says it.
    pub fn label(self) -> &'static str {
        match self {
            ChartKind::Column => "Column",
            ChartKind::Bar => "Bar",
            ChartKind::Line => "Line",
            ChartKind::Area => "Area",
            ChartKind::Pie => "Pie",
            ChartKind::Scatter => "Scatter",
            ChartKind::Sankey => "Sankey",
            ChartKind::Waterfall => "Waterfall",
            ChartKind::Radar => "Radar",
        }
    }

    /// Whether the kind draws in a rectangular plot, which a plot background
    /// and references need: every kind with axes but a radar.
    pub fn rect_plot(self) -> bool {
        self.has_axes() && self != ChartKind::Radar
    }

    /// Whether the kind has a value and a category axis.
    pub fn has_axes(self) -> bool {
        !matches!(self, ChartKind::Pie | ChartKind::Sankey)
    }
}

/// The data: named columns and rows of text cells. A column is a number column
/// while every non-empty cell in it parses as one ([`parse_number`]); the type
/// is read, never set.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Table {
    pub columns: Vec<Column>,
    /// Each row holds one cell per column, in column order.
    #[serde(default)]
    pub rows: Vec<Vec<String>>,
}

/// A column: its id, which the mapping and the series styles name it by, and
/// its header.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Column {
    pub id: ColumnId,
    pub name: String,
}

/// A column's identity within its table, kept across renames and reorders.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(transparent)]
pub struct ColumnId(pub u32);

impl Table {
    /// The column `id`'s place, `None` when the table has none.
    pub fn index_of(&self, id: ColumnId) -> Option<usize> {
        self.columns.iter().position(|c| c.id == id)
    }

    /// The column `id`, `None` when the table has none.
    pub fn column(&self, id: ColumnId) -> Option<&Column> {
        self.columns.iter().find(|c| c.id == id)
    }

    /// A name no column but `except` has: `wanted`, else numbered past it
    /// (*Price 2*), since a list's fill and a re-read's remap find a column
    /// by its name, and two of one name would always find the first.
    pub fn free_column_name(&self, wanted: &str, except: Option<ColumnId>) -> String {
        let taken: FxHashSet<&str> = self
            .columns
            .iter()
            .filter(|c| Some(c.id) != except)
            .map(|c| c.name.as_str())
            .collect();
        free_among(&taken, wanted)
    }

    /// Each column named apart from those before it, a repeat numbered past
    /// (*Price*, *Price 2*): a table written by name always finds the first of
    /// two equal names.
    pub fn name_columns_apart(&mut self) {
        let mut taken: FxHashSet<String> = FxHashSet::default();
        for c in &mut self.columns {
            let free: FxHashSet<&str> = taken.iter().map(String::as_str).collect();
            c.name = free_among(&free, &c.name);
            taken.insert(c.name.clone());
        }
    }

    /// The cells of column `id`, top to bottom; empty when the table has none.
    pub fn cells(&self, id: ColumnId) -> impl Iterator<Item = &str> + '_ {
        let at = self.index_of(id);
        self.rows
            .iter()
            .filter_map(move |r| at.and_then(|i| r.get(i)).map(String::as_str))
    }

    /// Whether column `id` is a number column: at least one cell, and every
    /// non-empty cell a number.
    pub fn is_numeric(&self, id: ColumnId) -> bool {
        let mut any = false;
        for c in self.cells(id) {
            if c.trim().is_empty() {
                continue;
            }
            if parse_number(c).is_none() {
                return false;
            }
            any = true;
        }
        any
    }

    /// An id no column of this table has: past the highest, or the lowest free
    /// one when the highest is `u32::MAX`, as an agent may send.
    pub fn next_column_id(&self) -> ColumnId {
        let taken: FxHashSet<ColumnId> = self.columns.iter().map(|c| c.id).collect();
        let past = self
            .columns
            .iter()
            .map(|c| c.id.0)
            .max()
            .map_or(0, |m| m.wrapping_add(1));
        free_column_id(&taken, past)
    }

    /// This freshly read table's columns renumbered against `old`: a column
    /// whose name `old` also has takes that column's id, the rest ids `old`
    /// never used. Answers the ids kept. A re-read file numbers its columns by
    /// position, so a reordered file would otherwise move every reader onto the
    /// wrong column.
    pub fn take_ids_by_name(&mut self, old: &Table) -> FxHashSet<ColumnId> {
        let next = old.next_column_id().0.max(self.next_column_id().0);
        let mut taken: FxHashSet<ColumnId> = old.columns.iter().map(|c| c.id).collect();
        let mut kept = FxHashSet::default();
        for c in &mut self.columns {
            let same = old
                .columns
                .iter()
                .find(|o| o.name == c.name && !kept.contains(&o.id));
            c.id = match same {
                Some(o) => o.id,
                None => free_column_id(&taken, next),
            };
            taken.insert(c.id);
            kept.insert(c.id);
        }
        kept
    }
}

/// `wanted` when `taken` lacks it or it is empty, else numbered past it; at
/// most [`MAX_CELL_CHARS`] either way.
pub(crate) fn free_among(taken: &FxHashSet<&str>, wanted: &str) -> String {
    if wanted.is_empty() {
        return String::new();
    }
    crate::naming::free_or_numbered(taken, wanted, MAX_CELL_CHARS)
}

/// The first id from `from` up, wrapping, that `taken` does not hold. A table
/// holds at most [`MAX_COLUMNS`], so the search is short.
fn free_column_id(taken: &FxHashSet<ColumnId>, from: u32) -> ColumnId {
    let mut id = from;
    while taken.contains(&ColumnId(id)) {
        id = id.wrapping_add(1);
    }
    ColumnId(id)
}

/// A cell read as a number: an optional sign, digits with an optional `,`
/// thousands separator in groups of three, an optional `.` fraction, and space
/// around. A decimal comma does not parse, and neither does anything that is not
/// finite. `None` for an empty cell.
pub fn parse_number(cell: &str) -> Option<f64> {
    let s = cell.trim();
    let body = s.strip_prefix(['-', '+']).unwrap_or(s);
    if body.is_empty() {
        return None;
    }
    let (int, frac) = match body.split_once('.') {
        Some((i, f)) => (i, Some(f)),
        None => (body, None),
    };
    if frac.is_some_and(|f| f.is_empty() || !f.bytes().all(|b| b.is_ascii_digit())) {
        return None;
    }
    if int.is_empty() && frac.is_none() {
        return None;
    }
    if !int.is_empty() {
        let groups: Vec<&str> = int.split(',').collect();
        let digits = |g: &str| !g.is_empty() && g.bytes().all(|b| b.is_ascii_digit());
        let ok = match groups.as_slice() {
            [one] => digits(one),
            [first, rest @ ..] => {
                digits(first) && first.len() <= 3 && rest.iter().all(|g| digits(g) && g.len() == 3)
            }
            [] => false,
        };
        if !ok {
            return None;
        }
    }
    let plain: String = s.chars().filter(|c| *c != ',').collect();
    plain.parse::<f64>().ok().filter(|v| v.is_finite())
}

/// Which columns the chart reads, by role.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Mapping {
    /// The category column; a scatter's X.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub category: Option<ColumnId>,
    /// The series columns, in the order drawn; a scatter's Y columns.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub series: Vec<ColumnId>,
    /// A bubble chart's size column.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub size: Option<ColumnId>,
    /// A scatter's group column, which splits its points into series.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub group: Option<ColumnId>,
    /// A Sankey's From, To and Value columns.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub from: Option<ColumnId>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub to: Option<ColumnId>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub value: Option<ColumnId>,
}

impl Mapping {
    /// Every column the mapping names.
    pub fn columns(&self) -> impl Iterator<Item = ColumnId> + '_ {
        self.category
            .into_iter()
            .chain(self.series.iter().copied())
            .chain(self.size)
            .chain(self.group)
            .chain(self.from)
            .chain(self.to)
            .chain(self.value)
    }
}

/// How one series is drawn.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct SeriesStyle {
    pub column: ColumnId,
    /// `None` is the palette's colour at the series' place, for the theme drawn.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub fill: Option<BrushSource>,
    #[serde(default = "yes")]
    pub visible: bool,
    #[serde(default, skip_serializing_if = "is_default")]
    pub dash: Dash,
    /// The series' own point shape; `None` takes a scatter's `Points::shape`, and
    /// a line's circle.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub point: Option<PointShape>,
}

/// A named mark's colour (`named_marks`), by name.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct NodeStyle {
    pub name: String,
    /// `None` is the palette's colour at the mark's place, for the theme drawn.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub fill: Option<BrushSource>,
}

/// A line's dash.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum Dash {
    #[default]
    Solid,
    Dashed,
    Dotted,
}

/// A point's shape: a scatter's marks and a line's points.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum PointShape {
    #[default]
    Circle,
    Square,
    Diamond,
    Triangle,
}

/// The linked file a chart's table was read from (`charts.md`, *Import and the
/// linked file*): its path and the stamp it had when last read.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct DataLink {
    pub path: String,
    #[serde(default)]
    pub size: u64,
    /// Modified time in whole seconds since the Unix epoch.
    #[serde(default)]
    pub modified: u64,
    /// Whether a CSV's or TSV's first row is the header, as the import chose;
    /// `None` guesses it on each read.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub header: Option<bool>,
}

/// A generated part's role in its chart, which regeneration keeps its id by.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
pub struct PartKey {
    pub role: PartRole,
    /// The series (its column id) or the axis the part belongs to; 0 when none.
    #[serde(default, skip_serializing_if = "is_zero")]
    pub series: u32,
    /// The part's place among its role's: a row, a tick, a node; 0 when one.
    #[serde(default, skip_serializing_if = "is_zero")]
    pub index: u32,
}

/// What a generated part is.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
pub enum PartRole {
    Legend,
    LegendItem,
    LegendSwatch,
    LegendLabel,
    Body,
    ValueAxis,
    CategoryAxis,
    Plot,
    /// The plot's own background (`Options::plot`).
    Ground,
    /// A progress ring's track.
    Track,
    Grid,
    Gridline,
    AxisLine,
    Tick,
    TickLabel,
    CategoryLabel,
    AxisTitle,
    Series,
    Bar,
    Line,
    Area,
    Point,
    Slice,
    PieFrame,
    Values,
    ValueLabel,
    /// A stack's net total, past its end.
    Total,
    /// A lollipop's stem and head, in its bar's frame.
    Stem,
    Head,
    /// A dumbbell's line joining its band's heads, or a waterfall's line from
    /// one step's end to the next one's start.
    Connector,
    /// The frame holding a waterfall's connectors.
    Connectors,
    /// A line's or area's name at its last value.
    EndLabel,
    /// The frame holding a chart's references.
    References,
    /// A reference band, under the marks.
    Band,
    /// A reference line, over them.
    Reference,
    ReferenceLabel,
    LeaderLine,
    Flows,
    Flow,
    Nodes,
    SankeyNode,
    NodeLabel,
}

/// Every option of every kind (`charts.md`, *Options*).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Options {
    pub palette: Palette,
    /// *From variables*: the colours an unset series or named mark takes in
    /// place of the palette's, in order, round again past the last — each bound
    /// to a variable of the group picked, so a mode switch recolours the chart.
    /// Empty is the palette. At most [`MAX_PALETTE_GROUP`].
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub palette_group: Vec<BrushSource>,
    /// Which ground the unset colours are derived for (`Chart::drawn_dark`).
    pub theme: Theme,
    /// The plot's own background, under the grid.
    #[serde(skip_serializing_if = "is_default")]
    pub plot: Plot,
    pub bars: Bars,
    pub lines: Lines,
    pub slices: Slices,
    pub points: Points,
    pub flows: Flows,
    pub value_axis: Axis,
    pub category_axis: Axis,
    pub values: Values,
    pub legend: Legend,
    pub text: ChartText,
    /// The stroke on bars, slices, points and Sankey nodes. Lines are strokes
    /// already, and areas keep their opacity.
    #[serde(skip_serializing_if = "is_default")]
    pub stroke: MarkStroke,
    /// A one-series column or bar chart's bars coloured by category, as a
    /// pie's slices are (`named_marks`).
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    pub by_category: bool,
    /// One colour for every value under the baseline: bars, and the runs of a
    /// line or area below zero.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub below_zero: Option<BrushSource>,
    #[serde(skip_serializing_if = "is_default")]
    pub waterfall: Waterfall,
    #[serde(skip_serializing_if = "is_default")]
    pub radar: Radar,
    /// Areas and bars from their colour at the value end to a share of it at
    /// the baseline.
    #[serde(skip_serializing_if = "is_default")]
    pub fade: Fade,
    /// Space between the frame's edge and the chart, and between the legend and
    /// the body.
    pub padding: f64,
    pub gap: f64,
}

/// The share of a mark's colour an outlined mark is filled with.
pub const OUTLINE_TINT: f32 = 0.14;

impl Default for Options {
    fn default() -> Self {
        Self {
            palette: Palette::default(),
            palette_group: Vec::new(),
            theme: Theme::default(),
            plot: Plot::default(),
            bars: Bars::default(),
            lines: Lines::default(),
            slices: Slices::default(),
            points: Points::default(),
            flows: Flows::default(),
            // The gridlines carry the value axis, so its own line starts off.
            value_axis: Axis {
                line: false,
                ..Axis::default()
            },
            category_axis: Axis {
                grid: Gridlines {
                    show: false,
                    ..Gridlines::default()
                },
                ..Axis::default()
            },
            values: Values::default(),
            legend: Legend::default(),
            text: ChartText::default(),
            stroke: MarkStroke::default(),
            by_category: false,
            below_zero: None,
            waterfall: Waterfall::default(),
            radar: Radar::default(),
            fade: Fade::default(),
            padding: 16.0,
            gap: 12.0,
        }
    }
}

/// Which ground a chart's unset colours are derived for: *Auto* reads the
/// ground it is drawn on (`Chart::drawn_dark`).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum Theme {
    #[default]
    Auto,
    Light,
    Dark,
}

/// The plot's own background: a rect under the grid, absent while `fill` is
/// `None`.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Plot {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub fill: Option<BrushSource>,
    pub radius: f64,
}

/// How a mark's stroke draws: on a solid mark, or over a tint of it.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum StrokeStyle {
    /// The mark filled in its colour, stroked inside in the stroke's colour
    /// (unset: the ground), which separates neighbouring marks.
    #[default]
    Solid,
    /// The mark stroked inside in its colour (or the stroke's) over an
    /// [`OUTLINE_TINT`] of it.
    Outline,
}

/// The stroke on a chart's marks; width 0 draws none.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct MarkStroke {
    pub style: StrokeStyle,
    pub width: f64,
    /// `None` is the ground under *Solid*, each mark's own colour under
    /// *Outline*.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub colour: Option<BrushSource>,
}

/// The order a column, bar or pie chart draws its categories in. The table
/// keeps its own.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum Order {
    #[default]
    Table,
    /// Largest first: at the top of a bar chart, the left of a column chart,
    /// clockwise from the start of a pie.
    Descending,
    Ascending,
}

/// A legend swatch's shape.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum Swatch {
    #[default]
    Square,
    Circle,
    /// The series' line, dash and all.
    Line,
}

/// What a label at a line's end says.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum EndLabel {
    #[default]
    Name,
    NameValue,
}

/// Which ink a label at a line's end is written in.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum EndInk {
    /// Its series' colour.
    #[default]
    Series,
    /// The legend's ink, as the legend box's labels.
    Text,
}

/// One kind of label's own weight and ink; `None` takes the chart's.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct RoleText {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub weight: Option<u16>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub ink: Option<BrushSource>,
}

/// Which ink a label inside a mark is written in.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum InsideInk {
    /// The text ink or the ground, whichever contrasts more with the mark.
    #[default]
    Auto,
    /// Always the text ink.
    Text,
}

/// The colours new series take, in order.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum Palette {
    #[default]
    Mastik,
    Cool,
    Warm,
    Mono,
}

/// Whether series stack: a column or bar chart's Grouped, Stacked and 100%, an
/// area chart's Overlapping, Stacked and 100%.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum Stacking {
    #[default]
    Apart,
    Stacked,
    Percent,
}

/// Column and bar options.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Bars {
    pub stacking: Stacking,
    /// The gap between groups, as a percentage of a band.
    pub group_gap: f64,
    /// The gap between a group's bars, as a percentage of a bar.
    pub bar_gap: f64,
    pub radius: f64,
    /// Whether the radius rounds every corner, not only the end away from the baseline.
    pub radius_all: bool,
    /// The order the categories draw in, by the first series shown or a
    /// stack's total.
    #[serde(skip_serializing_if = "is_default")]
    pub order: Order,
    /// Bars, or lollipops: a stem in a bar's slot and a head at its value.
    #[serde(skip_serializing_if = "is_default")]
    pub shape: BarShape,
    /// A lollipop's stem width and head size.
    pub stem: f64,
    pub head: f64,
    pub head_shape: PointShape,
    /// Filled, or hollow on the ground.
    pub head_style: PointStyle,
    /// Every series' head on its band's centre, joined: a dumbbell.
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    pub connect: bool,
    /// The stems' colour; `None` is each series' own.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub stem_colour: Option<BrushSource>,
}

/// How a column or bar chart draws a value.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum BarShape {
    #[default]
    Bars,
    Lollipops,
}

/// Waterfall options (`ChartKind::Waterfall`).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Waterfall {
    /// The Bar variant: steps run across rather than up.
    pub horizontal: bool,
    /// The steps' colours; `None` is blue, orange and slate for the theme drawn.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub increase: Option<BrushSource>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub decrease: Option<BrushSource>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub total: Option<BrushSource>,
    /// The categories drawn as the running total, by name.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub totals: Vec<String>,
    /// A total the chart adds after the last row, named so. Written even when
    /// `None`, as the default is on: a missing key reads back as `Total`.
    pub final_total: Option<String>,
    /// Lines from each step's end to the next one's start.
    pub connectors: bool,
    pub dashed: bool,
}

impl Default for Waterfall {
    fn default() -> Self {
        Self {
            horizontal: false,
            increase: None,
            decrease: None,
            total: None,
            totals: Vec::new(),
            final_total: Some("Total".into()),
            connectors: true,
            dashed: true,
        }
    }
}

impl Waterfall {
    /// A step's starting colour: blue for an increase, orange for a decrease,
    /// slate for a total (green and red merge under deuteranopia), each its
    /// dark twin on a dark ground.
    pub fn starting(step: Step, dark: bool) -> peniko::Color {
        let [r, g, b] = match (step, dark) {
            (Step::Increase, false) => [0x3B, 0x6F, 0xE0],
            (Step::Increase, true) => [0x6B, 0x95, 0xF0],
            (Step::Decrease, false) => [0xE0, 0x6C, 0x1E],
            (Step::Decrease, true) => [0xF0, 0x8C, 0x3C],
            (Step::Total, false) => [0x5D, 0x6D, 0x7E],
            (Step::Total, true) => [0x8E, 0x9D, 0xAD],
        };
        peniko::Color::from_rgb8(r, g, b)
    }
}

/// *Fade*: a mark's colour from full at its value end to `to` percent of it at
/// the baseline, as a generated linear gradient.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Fade {
    pub on: bool,
    /// 0 to 100.
    pub to: f64,
}

/// A radar's rings' shape.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum RadarGrid {
    #[default]
    Polygon,
    Circle,
}

/// Radar options (`ChartKind::Radar`); its lines take `Options::lines`.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Radar {
    /// Each shape painted at the area opacity, not only outlined.
    pub filled: bool,
    pub grid: RadarGrid,
    /// The category gridlines: a line from the centre out to each category.
    pub spokes: bool,
    /// Degrees clockwise from twelve o'clock where the first spoke points.
    pub start_angle: f64,
}

impl Default for Radar {
    fn default() -> Self {
        Self {
            filled: false,
            grid: RadarGrid::Polygon,
            spokes: true,
            start_angle: 0.0,
        }
    }
}

/// The fewest and most spokes a radar draws.
pub const RADAR_SPOKES: std::ops::RangeInclusive<usize> = 3..=36;

/// A waterfall step's kind, which its colour follows.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Step {
    Increase,
    Decrease,
    Total,
}

impl Default for Bars {
    fn default() -> Self {
        Self {
            stacking: Stacking::Apart,
            group_gap: 30.0,
            bar_gap: 10.0,
            radius: 2.0,
            radius_all: false,
            order: Order::Table,
            shape: BarShape::Bars,
            stem: 2.0,
            head: 12.0,
            head_shape: PointShape::Circle,
            head_style: PointStyle::Filled,
            connect: false,
            stem_colour: None,
        }
    }
}

/// A line's curve between its points.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum Curve {
    #[default]
    Straight,
    /// A monotone cubic (Fritsch–Carlson), which never overshoots a peak.
    Smooth,
    Step,
}

/// How a line's points are drawn.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum PointStyle {
    #[default]
    None,
    Hollow,
    Filled,
}

/// Line and area options.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Lines {
    pub width: f64,
    pub curve: Curve,
    pub points: PointStyle,
    pub point_size: f64,
    /// An area's fill opacity, 0 to 1.
    pub area_opacity: f64,
    pub connect_gaps: bool,
    /// An area chart's Overlapping, Stacked and 100%.
    pub area_stacking: Stacking,
}

impl Default for Lines {
    fn default() -> Self {
        Self {
            width: 2.0,
            curve: Curve::Straight,
            points: PointStyle::None,
            point_size: 6.0,
            area_opacity: 0.3,
            connect_gaps: false,
            area_stacking: Stacking::Apart,
        }
    }
}

/// Pie and donut options.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Slices {
    /// The Donut variant; its hole is [`Self::hole`].
    pub donut: bool,
    /// The hole, as a percentage of the radius.
    pub hole: f64,
    /// Degrees between slices.
    pub pad_angle: f64,
    pub radius: f64,
    /// Degrees clockwise from twelve o'clock.
    pub start_angle: f64,
    /// The order the slices run in, clockwise from the start angle.
    #[serde(skip_serializing_if = "is_default")]
    pub order: Order,
    /// Slices under this percentage fold into *Other*; `None` keeps every slice.
    pub group_below: Option<f64>,
}

impl Default for Slices {
    fn default() -> Self {
        Self {
            donut: false,
            hole: 60.0,
            pad_angle: 0.0,
            radius: 0.0,
            start_angle: 0.0,
            order: Order::Table,
            group_below: None,
        }
    }
}

/// Scatter and bubble options.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Points {
    pub size: f64,
    pub shape: PointShape,
    /// 0 to 1.
    pub opacity: f64,
    /// The largest bubble's size.
    pub max_size: f64,
}

impl Default for Points {
    fn default() -> Self {
        Self {
            size: 8.0,
            shape: PointShape::Circle,
            opacity: 0.8,
            max_size: 40.0,
        }
    }
}

/// Where a Sankey moves nodes after depth places them.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum FlowAlign {
    #[default]
    Justify,
    Left,
    Right,
    Centre,
}

/// How a Sankey's flow is coloured.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum FlowColour {
    #[default]
    Source,
    Target,
    Gradient,
    Neutral,
}

/// What a Sankey node's label says.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum NodeLabels {
    #[default]
    Name,
    NameAndValue,
    Off,
}

/// Sankey options.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Flows {
    pub node_width: f64,
    pub node_gap: f64,
    pub node_radius: f64,
    pub align: FlowAlign,
    /// Whether the relaxation passes run; off keeps table order.
    pub relax: bool,
    pub colour: FlowColour,
    /// 0 to 1.
    pub opacity: f64,
    pub labels: NodeLabels,
}

impl Default for Flows {
    fn default() -> Self {
        Self {
            node_width: 12.0,
            node_gap: 10.0,
            node_radius: 2.0,
            align: FlowAlign::Justify,
            relax: true,
            colour: FlowColour::Source,
            opacity: 0.4,
            labels: NodeLabels::Name,
        }
    }
}

/// A tick label's angle, on either axis and either side of the plot.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum LabelAngle {
    /// Category labels under columns turn 45° and then thin to every *n*th
    /// when they would touch; every other axis keeps its labels flat.
    #[default]
    Auto,
    Flat,
    Slanted,
    Upright,
}

/// Which side of the plot an axis sits on: left or bottom at `Start`.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum AxisSide {
    #[default]
    Start,
    End,
}

/// One axis's options.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Axis {
    pub show: bool,
    pub line: bool,
    pub ticks: bool,
    pub labels: bool,
    pub angle: LabelAngle,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub title: String,
    /// A typed min and max; `None` is Auto.
    pub range: Option<(f64, f64)>,
    /// Off by default; columns, bars and areas hold zero whatever it says, since
    /// their length is the value.
    pub start_at_zero: bool,
    /// `None` aims for five ticks on 1-2-5 steps.
    pub tick_count: Option<u32>,
    pub side: AxisSide,
    pub format: NumberFormat,
    pub grid: Gridlines,
    /// The line's and ticks' colour; `None` is derived (`Chart::axis_colour`).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub colour: Option<BrushSource>,
    /// The line's and ticks' width.
    pub line_width: f64,
}

impl Default for Axis {
    fn default() -> Self {
        Self {
            show: true,
            line: true,
            ticks: false,
            labels: true,
            angle: LabelAngle::Auto,
            title: String::new(),
            range: None,
            start_at_zero: false,
            tick_count: None,
            side: AxisSide::Start,
            format: NumberFormat::default(),
            grid: Gridlines::default(),
            colour: None,
            line_width: 1.0,
        }
    }
}

/// An axis's gridlines.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Gridlines {
    pub show: bool,
    pub dashed: bool,
    /// `None` is derived (`Chart::grid_colour`).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub colour: Option<BrushSource>,
    pub width: f64,
}

impl Default for Gridlines {
    fn default() -> Self {
        Self {
            show: true,
            dashed: false,
            colour: None,
            width: 1.0,
        }
    }
}

/// How a number is written on an axis or a value label.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct NumberFormat {
    /// `None` writes as many as the value needs, up to the tick step's.
    pub decimals: Option<u8>,
    pub thousands: bool,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub prefix: String,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub suffix: String,
    /// 1.2k, 3.4M.
    pub compact: bool,
    /// ×100 with `%`.
    pub percent: bool,
}

/// Where value labels sit.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum ValuePosition {
    #[default]
    Off,
    Inside,
    Outside,
    End,
}

/// What a pie's labels say.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum PieLabel {
    #[default]
    Value,
    Percent,
    Category,
}

/// Value label options.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Values {
    pub position: ValuePosition,
    pub pie: PieLabel,
    /// Whether a pie's labels sit outside with a leader line.
    pub pie_outside: bool,
    pub format: NumberFormat,
    /// The ink of a label sitting inside its mark.
    pub inside_ink: InsideInk,
    /// A net total past each stack of a stacked column or bar chart.
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    pub totals: bool,
}

/// Where the legend sits.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum LegendPosition {
    Off,
    #[default]
    Top,
    Bottom,
    Left,
    Right,
}

/// A legend's or a row's alignment.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum Align {
    #[default]
    Start,
    Centre,
    End,
}

/// Legend options.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Legend {
    pub position: LegendPosition,
    pub align: Align,
    /// The swatch's shape in a legend box.
    #[serde(skip_serializing_if = "is_default")]
    pub swatch: Swatch,
    /// Each line or area named at its last value, beside the plot: a switch of
    /// its own, since every preset writes the position.
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    pub line_ends: bool,
    #[serde(skip_serializing_if = "is_default")]
    pub end_label: EndLabel,
    /// The ink a line-end label is written in.
    #[serde(skip_serializing_if = "is_default")]
    pub end_ink: EndInk,
}

/// The chart's type: one family and weight, a size per kind of label, and the
/// ink; each kind of label may take its own weight and ink.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct ChartText {
    /// Empty is the document's default text family.
    #[serde(skip_serializing_if = "String::is_empty")]
    pub family: String,
    pub weight: u16,
    pub tick_size: f64,
    pub value_size: f64,
    pub legend_size: f64,
    pub title_size: f64,
    /// `None` is derived (`Chart::ink`).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub ink: Option<BrushSource>,
    /// Tick and category labels.
    #[serde(skip_serializing_if = "is_default")]
    pub tick: RoleText,
    /// Value labels, stack totals and a pie's labels.
    #[serde(skip_serializing_if = "is_default")]
    pub value: RoleText,
    /// The legend, line-end labels and a Sankey's node labels.
    #[serde(skip_serializing_if = "is_default")]
    pub legend: RoleText,
    /// Axis titles.
    #[serde(skip_serializing_if = "is_default")]
    pub title: RoleText,
}

impl Default for ChartText {
    fn default() -> Self {
        Self {
            family: String::new(),
            weight: 400,
            tick_size: 11.0,
            value_size: 11.0,
            legend_size: 12.0,
            title_size: 12.0,
            ink: None,
            tick: RoleText::default(),
            value: RoleText::default(),
            legend: RoleText::default(),
            title: RoleText::default(),
        }
    }
}

/// A chart's chrome colours for one ground.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Chrome {
    /// The labels' ink.
    pub ink: peniko::Color,
    /// The axes' lines and ticks.
    pub axis: peniko::Color,
    /// The gridlines.
    pub grid: peniko::Color,
    /// The ground it is drawn for.
    pub ground: peniko::Color,
}

/// The fixed chrome of *Light* or (`dark`) *Dark*: the ink as legible, the axes
/// and gridlines as quiet, against that theme's standard ground.
pub fn chrome(dark: bool) -> Chrome {
    if dark {
        Chrome {
            ink: grey(0.72),
            axis: grey(0.38),
            grid: grey(0.22),
            ground: peniko::Color::from_rgb8(0x18, 0x18, 0x1B),
        }
    } else {
        Chrome {
            ink: grey(0.35),
            axis: grey(0.75),
            grid: grey(0.9),
            ground: peniko::Color::WHITE,
        }
    }
}

fn grey(l: f32) -> peniko::Color {
    peniko::Color::new([l, l, l, 1.0])
}

/// The share of the ink over the ground an *Auto* chart's gridlines and axes
/// are drawn in.
const AUTO_GRID: f32 = 0.12;
const AUTO_AXIS: f32 = 0.30;
/// How much more contrast the dark theme's ink must win by before *Auto* picks
/// it, so a mid grey reads the same each time rather than flipping per nudge.
const AUTO_TIE: f64 = 0.25;

/// `top` at `k` over `ground`, opaque.
pub(crate) fn mix(ground: peniko::Color, top: peniko::Color, k: f32) -> peniko::Color {
    let [gr, gg, gb, _] = ground.components;
    let [tr, tg, tb, _] = top.components;
    peniko::Color::new([
        gr + (tr - gr) * k,
        gg + (tg - gg) * k,
        gb + (tb - gb) * k,
        1.0,
    ])
}

/// WCAG relative luminance.
fn luminance(c: peniko::Color) -> f64 {
    let lin = |v: f32| {
        let v = f64::from(v.clamp(0.0, 1.0));
        if v <= 0.04045 {
            v / 12.92
        } else {
            ((v + 0.055) / 1.055).powf(2.4)
        }
    };
    let [r, g, b, _] = c.components;
    0.2126 * lin(r) + 0.7152 * lin(g) + 0.0722 * lin(b)
}

/// The WCAG contrast ratio between two colours, 1 to 21.
pub(crate) fn contrast(a: peniko::Color, b: peniko::Color) -> f64 {
    let (la, lb) = (luminance(a), luminance(b));
    (la.max(lb) + 0.05) / (la.min(lb) + 0.05)
}

/// Whether *Auto* draws dark on `ground`: the dark theme's ink contrasts more
/// with it than the light theme's, by more than [`AUTO_TIE`].
pub fn dark_on(ground: peniko::Color) -> bool {
    contrast(chrome(true).ink, ground) > contrast(chrome(false).ink, ground) + AUTO_TIE
}

/// Where a chart's ground was read ([`ground_of`]).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum GroundFrom {
    /// The chart frame's own fill.
    Own,
    /// The nearest painted ancestor frame.
    Ancestor(NodeId),
    /// The page.
    Page,
}

/// The ground a chart is drawn on: what a hollow point fills with and what
/// *Auto* reads.
#[derive(Clone, Debug, PartialEq)]
pub struct Ground {
    /// The chart frame's own fill as it is, binding included; anything else's
    /// as its colour in the chart's modes, so the chart frame stays the only
    /// user of the variables its parts draw.
    pub brush: BrushSource,
    /// The colour it draws, opaque.
    pub colour: peniko::Color,
    pub from: GroundFrom,
    /// The colour each variable the chart binds draws at the chart, so the
    /// generator can judge contrast without the document.
    pub vars: Vec<(crate::VarId, peniko::Color)>,
}

impl Ground {
    /// A ground of `colour` alone, for a chart drawn outside a document.
    pub fn plain(colour: peniko::Color) -> Ground {
        Ground {
            brush: BrushSource::Literal(crate::Brush::Solid(colour)),
            colour,
            from: GroundFrom::Page,
            vars: Vec::new(),
        }
    }
}

/// `top` composited over `under`, straight alpha.
fn over(top: peniko::Color, under: peniko::Color) -> peniko::Color {
    let [tr, tg, tb, ta] = top.components;
    let [ur, ug, ub, ua] = under.components;
    let a = ta + ua * (1.0 - ta);
    if a <= 0.0 {
        return peniko::Color::TRANSPARENT;
    }
    let ch = |t: f32, u: f32| (t * ta + u * ua * (1.0 - ta)) / a;
    peniko::Color::new([ch(tr, ur), ch(tg, ug), ch(tb, ub), a])
}

/// The colour a node's visible fills make, each over the ones under it (a
/// gradient read as its first stop, an image not at all); `None` when nothing
/// is painted.
fn painted(doc: &crate::Document, n: &Node) -> Option<peniko::Color> {
    painted_by(doc, n.id, &n.paint.fills)
}

/// [`painted`] over `fills`, resolved at node `id`.
fn painted_by(
    doc: &crate::Document,
    id: NodeId,
    fills: &[crate::item::Keyed<crate::node::Fill>],
) -> Option<peniko::Color> {
    let mut colour: Option<peniko::Color> = None;
    for f in fills.iter().filter(|f| f.visible) {
        let c = match &*doc.resolve_brush(id, &f.brush) {
            crate::Brush::Solid(c) => *c,
            crate::Brush::Gradient(g) => match g.gradient.stops.first() {
                Some(s) => s.color.to_alpha_color::<peniko::color::Srgb>(),
                None => continue,
            },
            _ => continue,
        };
        colour = Some(colour.map_or(c, |under| over(c, under)));
    }
    colour.filter(|c| c.components[3] > 0.0)
}

/// The ground colour chart `id` would read ([`ground_of`]) with `fills` as its
/// own: a picker's preview of the chart's background, so *Auto* turns with
/// the drag rather than on release.
pub fn ground_with(
    doc: &crate::Document,
    id: NodeId,
    fills: &[crate::item::Keyed<crate::node::Fill>],
) -> peniko::Color {
    let under = match doc.get(id).and_then(|n| n.parent) {
        Some(p) => ground_of(doc, p).colour,
        None => ground_of(doc, id).colour,
    };
    painted_by(doc, id, fills)
        .map_or(under, |c| over(c, under))
        .with_alpha(1.0)
}

/// The fill a chart made under `parent` starts with: white on an unpainted
/// canvas, none inside a painted frame, so a chart drawn on a card reads the
/// card.
pub fn starting_ground(doc: &crate::Document, parent: NodeId) -> Option<peniko::Color> {
    let mut at = Some(parent);
    while let Some(n) = at.and_then(|a| doc.get(a)) {
        if matches!(n.kind, NodeKind::Artboard { .. }) && painted(doc, n).is_some() {
            return None;
        }
        at = n.parent;
    }
    Some(peniko::Color::WHITE)
}

/// The ground chart `id` is drawn on: its own fills, else each ancestor frame's
/// in turn (groups skipped), else the page's ground, a translucent layer laid
/// over what is under it.
pub fn ground_of(doc: &crate::Document, id: NodeId) -> Ground {
    let page = doc
        .page_of(id)
        .and_then(|p| doc.page(p))
        .and_then(|p| p.ground)
        .unwrap_or(crate::DEFAULT_CANVAS_BACKGROUND)
        .with_alpha(1.0);
    let mut above: Option<peniko::Color> = None;
    let mut from = GroundFrom::Page;
    let mut at = Some(id);
    while let Some(n) = at.and_then(|a| doc.get(a)) {
        if matches!(n.kind, NodeKind::Artboard { .. })
            && let Some(c) = painted(doc, n)
        {
            if from == GroundFrom::Page {
                from = if n.id == id {
                    GroundFrom::Own
                } else {
                    GroundFrom::Ancestor(n.id)
                };
            }
            above = Some(above.map_or(c, |a| over(a, c)));
            if above.is_some_and(|a| a.components[3] >= 1.0) {
                break;
            }
        }
        at = n.parent;
    }
    let colour = above.map_or(page, |a| over(a, page)).with_alpha(1.0);
    // The own fill passes as it is, binding included, while it alone makes the
    // ground: one visible fill, opaque.
    let own: Vec<&BrushSource> = doc
        .get(id)
        .map(|n| {
            n.paint
                .fills
                .iter()
                .filter(|f| f.visible)
                .map(|f| &f.brush)
                .collect()
        })
        .unwrap_or_default();
    let opaque = |b: &BrushSource| matches!(&*doc.resolve_brush(id, b), crate::Brush::Solid(c) if c.components[3] >= 1.0);
    let brush = match own.as_slice() {
        [b] if from == GroundFrom::Own && opaque(b) => (*b).clone(),
        _ => BrushSource::Literal(crate::Brush::Solid(colour)),
    };
    let mut vars: Vec<(crate::VarId, peniko::Color)> = Vec::new();
    let chart = doc.get(id).and_then(|n| n.chart());
    for b in chart.into_iter().flat_map(Chart::brushes).chain([&brush]) {
        if let BrushSource::Bound { var, .. } = b
            && !vars.iter().any(|(v, _)| v == var)
            && let Some(crate::variable::Value::Color(c)) = doc.resolve_var(id, *var)
        {
            vars.push((*var, c));
        }
    }
    Ground {
        brush,
        colour,
        from,
        vars,
    }
}

fn solid(c: peniko::Color) -> BrushSource {
    BrushSource::Literal(crate::Brush::Solid(c))
}

fn yes() -> bool {
    true
}

fn is_zero(v: &u32) -> bool {
    *v == 0
}

fn is_zero_u64(v: &u64) -> bool {
    *v == 0
}

fn is_default<T: Default + PartialEq>(v: &T) -> bool {
    *v == T::default()
}

/// Why a description is refused.
#[derive(Clone, Copy, Debug, PartialEq, Eq, thiserror::Error)]
pub enum ChartRule {
    #[error("a chart table holds at most {MAX_ROWS} rows and {MAX_COLUMNS} columns")]
    TooLarge,
    #[error("a chart cell holds at most {MAX_CELL_CHARS} characters")]
    CellTooLong,
    #[error("a chart table's rows must each hold one cell per column")]
    RaggedRow,
    #[error("a chart table's column ids must be unique")]
    DuplicateColumn,
    #[error("a chart's mapping or series names a column its table does not have")]
    UnknownColumn,
    #[error("a chart's series are mapped and styled once each")]
    DuplicateSeries,
    #[error("a chart option is out of range")]
    BadOption,
    #[error("a chart's parts name a node twice or a role twice")]
    DuplicatePart,
    #[error("a chart holds at most {MAX_REFERENCES} references, each with its own id")]
    References,
    #[error("lollipops don't stack: make the chart grouped first")]
    LollipopsStack,
}

impl Chart {
    /// The highlight's names and the references in shape: names within the
    /// cell limit, at most [`MAX_REFERENCES`] references with distinct ids,
    /// every number finite and every width in range.
    fn validate_annotations(&self) -> Result<(), ChartRule> {
        let name = |s: &String| s.chars().count() <= MAX_CELL_CHARS;
        let names = match &self.highlight.target {
            Highlighted::None => true,
            Highlighted::Categories(n) | Highlighted::Series(n) => {
                n.len() <= MAX_ROWS && n.iter().all(name)
            }
        };
        if !names {
            return Err(ChartRule::BadOption);
        }
        if self.references.len() > MAX_REFERENCES {
            return Err(ChartRule::References);
        }
        let mut ids = FxHashSet::default();
        for r in &self.references {
            if !ids.insert(r.id) {
                return Err(ChartRule::References);
            }
            let at_ok = |a: &At| match a {
                At::Value(v) => v.is_finite(),
                At::Category(c) => name(c),
                _ => true,
            };
            let ok = at_ok(&r.at)
                && at_ok(&r.to)
                && r.label.chars().count() <= MAX_TEXT_CHARS
                && r.width.is_finite()
                && (0.0..=64.0).contains(&r.width);
            if !ok {
                return Err(ChartRule::BadOption);
            }
        }
        Ok(())
    }
}

impl Chart {
    /// Whether this description may be stored: the table's limits and shape, the
    /// mapping naming columns the table has, every number finite and in range.
    /// The same rule for an operation and the loader.
    pub fn validate(&self) -> Result<(), ChartRule> {
        let t = &self.table;
        if t.rows.len() > MAX_ROWS || t.columns.len() > MAX_COLUMNS {
            return Err(ChartRule::TooLarge);
        }
        let mut ids = FxHashSet::default();
        for c in &t.columns {
            if !ids.insert(c.id) {
                return Err(ChartRule::DuplicateColumn);
            }
            if c.name.chars().count() > MAX_CELL_CHARS {
                return Err(ChartRule::CellTooLong);
            }
        }
        for r in &t.rows {
            if r.len() != t.columns.len() {
                return Err(ChartRule::RaggedRow);
            }
            if r.iter().any(|c| c.chars().count() > MAX_CELL_CHARS) {
                return Err(ChartRule::CellTooLong);
            }
        }
        if self.mapping.columns().any(|c| !ids.contains(&c)) {
            return Err(ChartRule::UnknownColumn);
        }
        // A series is keyed by its column, so one mapped twice draws two parts
        // under every key.
        let mut mapped = FxHashSet::default();
        if !self.mapping.series.iter().all(|c| mapped.insert(*c)) {
            return Err(ChartRule::DuplicateSeries);
        }
        let mut styled = FxHashSet::default();
        for s in &self.series {
            if !ids.contains(&s.column) {
                return Err(ChartRule::UnknownColumn);
            }
            if !styled.insert(s.column) {
                return Err(ChartRule::DuplicateSeries);
            }
        }
        let mut names = FxHashSet::default();
        for n in &self.nodes {
            if !names.insert(n.name.as_str()) || n.name.chars().count() > MAX_CELL_CHARS {
                return Err(ChartRule::DuplicateSeries);
            }
        }
        self.options.validate()?;
        self.validate_annotations()?;
        let mut keys = FxHashSet::default();
        let mut nodes = FxHashSet::default();
        for (k, id) in &self.parts {
            if !keys.insert(*k) || !nodes.insert(*id) {
                return Err(ChartRule::DuplicatePart);
            }
        }
        if self
            .generated
            .is_some_and(|s| !s.iter().all(|v| v.is_finite() && *v >= 0.0))
        {
            return Err(ChartRule::BadOption);
        }
        Ok(())
    }

    /// Every colour the description holds; an unset one holds none.
    pub fn brushes(&self) -> impl Iterator<Item = &BrushSource> + '_ {
        self.series
            .iter()
            .filter_map(|s| s.fill.as_ref())
            .chain(self.nodes.iter().filter_map(|n| n.fill.as_ref()))
            .chain(self.options.brushes())
            .chain(&self.highlight.muted)
            .chain(self.references.iter().map(|r| &r.colour))
    }

    /// [`Self::brushes`], to rewrite in place: a binding detached or rebound.
    pub fn brushes_mut(&mut self) -> impl Iterator<Item = &mut BrushSource> + '_ {
        self.series
            .iter_mut()
            .filter_map(|s| s.fill.as_mut())
            .chain(self.nodes.iter_mut().filter_map(|n| n.fill.as_mut()))
            .chain(self.options.brushes_mut())
            .chain(&mut self.highlight.muted)
            .chain(self.references.iter_mut().map(|r| &mut r.colour))
    }

    /// The colour a highlight's other marks draw in under *Muted*: set, else
    /// the ink at 25% over the ground.
    pub fn muted(&self) -> BrushSource {
        self.highlight.muted.clone().unwrap_or_else(|| {
            let c = self.chrome();
            solid(mix(c.ground, c.ink, 0.25))
        })
    }

    /// Whether the chart is drawn for a dark ground: its theme, or under
    /// *Auto* what the ground it was last drawn on reads as ([`dark_on`]);
    /// light before it has been drawn.
    pub fn drawn_dark(&self) -> bool {
        match self.options.theme {
            Theme::Light => false,
            Theme::Dark => true,
            Theme::Auto => self.grounded.is_some_and(dark_on),
        }
    }

    /// The chrome an unset colour derives: *Light*'s and *Dark*'s fixed
    /// greys; under *Auto* the picked theme's ink, with the gridlines and axes
    /// that ink at [`AUTO_GRID`] and [`AUTO_AXIS`] over the ground.
    pub fn chrome(&self) -> Chrome {
        let dark = self.drawn_dark();
        let fixed = chrome(dark);
        match (self.options.theme, self.grounded) {
            (Theme::Auto, Some(ground)) => Chrome {
                ink: fixed.ink,
                axis: mix(ground, fixed.ink, AUTO_AXIS),
                grid: mix(ground, fixed.ink, AUTO_GRID),
                ground,
            },
            _ => fixed,
        }
    }

    /// The labels' ink as drawn: set, else derived.
    pub fn ink(&self) -> BrushSource {
        self.options
            .text
            .ink
            .clone()
            .unwrap_or_else(|| solid(self.chrome().ink))
    }

    /// An axis's line colour as drawn, the value axis's when `value`.
    pub fn axis_colour(&self, value: bool) -> BrushSource {
        let a = self.axis(value);
        a.colour
            .clone()
            .unwrap_or_else(|| solid(self.chrome().axis))
    }

    /// An axis's gridline colour as drawn, the value axis's when `value`.
    pub fn grid_colour(&self, value: bool) -> BrushSource {
        let a = self.axis(value);
        a.grid
            .colour
            .clone()
            .unwrap_or_else(|| solid(self.chrome().grid))
    }

    fn axis(&self, value: bool) -> &Axis {
        if value {
            &self.options.value_axis
        } else {
            &self.options.category_axis
        }
    }

    /// This chart as drawn on `ground`: its stamp set to it, so every derived
    /// colour reads that ground.
    pub fn on(&self, ground: peniko::Color) -> Chart {
        Chart {
            grounded: Some(ground),
            ..self.clone()
        }
    }

    /// Whether `other` describes the same chart, whatever parts either holds.
    pub fn same_description(&self, other: &Chart) -> bool {
        let Chart {
            kind,
            table,
            mapping,
            series,
            nodes,
            options,
            source,
            data,
            parts: _,
            generated: _,
            grounded: _,
            minted: _,
            highlight,
            references,
        } = self;
        *kind == other.kind
            && *table == other.table
            && *mapping == other.mapping
            && *series == other.series
            && *nodes == other.nodes
            && *options == other.options
            && *source == other.source
            && *data == other.data
            && *highlight == other.highlight
            && *references == other.references
    }

    /// This description as a copy carries it: the linked file dropped, since
    /// pasted content must not make the receiving document read a file the copy
    /// chose.
    pub fn copied(&self) -> Chart {
        Chart {
            source: None,
            ..self.clone()
        }
    }

    /// A new chart of `kind` on its sample table, so a chart made by the tool, the
    /// palette or an agent draws something until its data is typed or pasted:
    /// *Category* and three series over *Jan*–*Jun*; a scatter's numeric X and two
    /// Y series; a Sankey's eight flows over three columns.
    pub fn sample(kind: ChartKind) -> Chart {
        let cols = |names: &[&str]| -> Vec<Column> {
            (0..)
                .zip(names)
                .map(|(id, name)| Column {
                    id: ColumnId(id),
                    name: (*name).to_string(),
                })
                .collect()
        };
        let rows = |rows: &[&[&str]]| -> Vec<Vec<String>> {
            rows.iter()
                .map(|r| r.iter().map(|c| (*c).to_string()).collect())
                .collect()
        };
        let (columns, rows, mapping) = match kind {
            ChartKind::Waterfall => (
                cols(&["Step", "Change"]),
                rows(&[
                    &["Revenue", "420"],
                    &["COGS", "-180"],
                    &["Gross", ""],
                    &["Opex", "-120"],
                    &["Other", "20"],
                    &["Tax", "-30"],
                ]),
                Mapping {
                    category: Some(ColumnId(0)),
                    series: vec![ColumnId(1)],
                    ..Mapping::default()
                },
            ),
            ChartKind::Sankey => (
                cols(&["From", "To", "Value"]),
                rows(&[
                    &["Search", "Signup", "1200"],
                    &["Social", "Signup", "600"],
                    &["Direct", "Signup", "400"],
                    &["Search", "Bounce", "800"],
                    &["Social", "Bounce", "300"],
                    &["Direct", "Bounce", "100"],
                    &["Signup", "Trial", "1500"],
                    &["Signup", "Leave", "700"],
                ]),
                Mapping {
                    from: Some(ColumnId(0)),
                    to: Some(ColumnId(1)),
                    value: Some(ColumnId(2)),
                    ..Mapping::default()
                },
            ),
            ChartKind::Scatter => (
                cols(&["Hours", "Team A", "Team B"]),
                rows(&[
                    &["2", "41", "35"],
                    &["4", "52", "44"],
                    &["5", "58", "47"],
                    &["7", "63", "60"],
                    &["8", "71", "62"],
                    &["10", "78", "70"],
                ]),
                Mapping {
                    category: Some(ColumnId(0)),
                    series: vec![ColumnId(1), ColumnId(2)],
                    ..Mapping::default()
                },
            ),
            _ => (
                cols(&["Category", "Revenue", "Costs", "Profit"]),
                rows(&[
                    &["Jan", "42", "30", "12"],
                    &["Feb", "48", "32", "16"],
                    &["Mar", "45", "34", "11"],
                    &["Apr", "56", "36", "20"],
                    &["May", "61", "38", "23"],
                    &["Jun", "68", "41", "27"],
                ]),
                Mapping {
                    category: Some(ColumnId(0)),
                    series: vec![ColumnId(1), ColumnId(2), ColumnId(3)],
                    ..Mapping::default()
                },
            ),
        };
        Chart {
            kind,
            table: Table { columns, rows },
            mapping,
            series: Vec::new(),
            nodes: Vec::new(),
            options: Options {
                // A line chart's legend shows each series' line.
                legend: Legend {
                    swatch: if kind == ChartKind::Line {
                        Swatch::Line
                    } else {
                        Swatch::Square
                    },
                    ..Legend::default()
                },
                // The sample's Gross row is a running total, Net its end.
                waterfall: if kind == ChartKind::Waterfall {
                    Waterfall {
                        totals: vec!["Gross".into()],
                        final_total: Some("Net".into()),
                        ..Waterfall::default()
                    }
                } else {
                    Waterfall::default()
                },
                ..Options::default()
            },
            source: None,
            data: None,
            parts: Vec::new(),
            generated: None,
            grounded: None,
            minted: 0,
            highlight: Highlight::default(),
            references: Vec::new(),
        }
    }

    /// The kind's variant as the Chart card names it (*Stacked*, *Smooth*,
    /// *Donut*, *Bubble*); `None` for a Sankey, which has none.
    pub fn variant(&self) -> Option<&'static str> {
        let stack = |s: Stacking, apart| match s {
            Stacking::Apart => apart,
            Stacking::Stacked => "Stacked",
            Stacking::Percent => "100%",
        };
        let o = &self.options;
        Some(match self.kind {
            ChartKind::Column | ChartKind::Bar => stack(o.bars.stacking, "Grouped"),
            ChartKind::Line => match o.lines.curve {
                Curve::Straight => "Straight",
                Curve::Smooth => "Smooth",
                Curve::Step => "Step",
            },
            ChartKind::Area => stack(o.lines.area_stacking, "Overlapping"),
            ChartKind::Pie if o.slices.donut => "Donut",
            ChartKind::Pie => "Pie",
            ChartKind::Scatter if self.mapping.size.is_some() => "Bubble",
            ChartKind::Scatter => "Scatter",
            ChartKind::Sankey => return None,
            ChartKind::Waterfall if o.waterfall.horizontal => "Bar",
            ChartKind::Waterfall => "Column",
            ChartKind::Radar if o.radar.filled => "Filled",
            ChartKind::Radar => "Lines",
        })
    }

    /// This chart as `kind`: a switch to or from a Sankey re-reads the mapping
    /// off the table's columns ([`Mapping::guess`]), since a Sankey maps *From*,
    /// *To* and *Value* where the other kinds map a category and series.
    pub fn with_kind(&self, kind: ChartKind) -> Chart {
        let mut next = self.clone();
        if kind != self.kind && (kind == ChartKind::Sankey || self.kind == ChartKind::Sankey) {
            next.mapping = Mapping::guess(kind, &self.table);
        }
        next.kind = kind;
        next
    }

    /// This chart on `table` in place of its own (`charts.md`, *Import*): a
    /// column whose name the old table also has takes that column's id, so the
    /// mapping and the series styles naming it are kept; the rest are dropped.
    /// When nothing of the mapping survives, the mapping is guessed
    /// ([`Mapping::guess`]). The link, the parts and the stamp are left as they
    /// were, for the caller to say.
    pub fn with_table(&self, mut table: Table) -> Chart {
        let kept = table.take_ids_by_name(&self.table);
        let keep = |c: &Option<ColumnId>| c.filter(|id| kept.contains(id));
        let m = &self.mapping;
        let mut mapping = Mapping {
            category: keep(&m.category),
            series: m
                .series
                .iter()
                .copied()
                .filter(|c| kept.contains(c))
                .collect(),
            size: keep(&m.size),
            group: keep(&m.group),
            from: keep(&m.from),
            to: keep(&m.to),
            value: keep(&m.value),
        };
        if mapping.columns().next().is_none() {
            mapping = Mapping::guess(self.kind, &table);
        }
        Chart {
            mapping,
            series: self
                .series
                .iter()
                .filter(|s| kept.contains(&s.column))
                .cloned()
                .collect(),
            table,
            ..self.clone()
        }
    }

    /// This chart reading data set `data`, whose table is `table`: the table
    /// taken as it is, ids included, and the mapping and series styles moved
    /// onto its columns. On the set it already reads a column is kept by id, so
    /// a rename keeps it; moved onto a table, a column is kept where `table` has
    /// it under the same id and name, else onto the column of its name, else
    /// dropped. When no column that draws (a series, or a Sankey's value)
    /// survives the move or the edit, the mapping is guessed, so a chart moved
    /// onto an unrelated table draws it; on the set it reads, a mapping that
    /// drew nothing already stays as it is, unless the table offered nothing to
    /// draw before the edit. The link is dropped, since the data set holds its
    /// own.
    pub fn reading(&self, data: crate::list::DataSetId, table: &Table) -> Chart {
        let old = &self.table;
        let same = self.data == Some(data);
        let onto = |id: ColumnId| -> Option<ColumnId> {
            if same && table.column(id).is_some() {
                return Some(id);
            }
            let name = &old.column(id)?.name;
            if table.column(id).is_some_and(|c| c.name == *name) {
                return Some(id);
            }
            table.columns.iter().find(|c| c.name == *name).map(|c| c.id)
        };
        let m = &self.mapping;
        let mut series = Vec::new();
        for c in m.series.iter().filter_map(|c| onto(*c)) {
            if !series.contains(&c) {
                series.push(c);
            }
        }
        let mut mapping = Mapping {
            category: m.category.and_then(onto),
            series,
            size: m.size.and_then(onto),
            group: m.group.and_then(onto),
            from: m.from.and_then(onto),
            to: m.to.and_then(onto),
            value: m.value.and_then(onto),
        };
        // On the set it reads, a mapping drawing nothing before the edit is the
        // person's own choice, not a loss to make up for, unless the table then
        // offered nothing to draw: then the guess had nothing to find.
        let drew = !m.series.is_empty() || m.value.is_some();
        let offered = || {
            let g = Mapping::guess(self.kind, old);
            !g.series.is_empty() || g.value.is_some()
        };
        if mapping.series.is_empty() && mapping.value.is_none() && (drew || !same || !offered()) {
            mapping = Mapping::guess(self.kind, table);
        }
        let mut styles: Vec<SeriesStyle> = Vec::new();
        for s in &self.series {
            if let Some(column) = onto(s.column)
                && !styles.iter().any(|t| t.column == column)
            {
                styles.push(SeriesStyle {
                    column,
                    ..s.clone()
                });
            }
        }
        Chart {
            table: table.clone(),
            mapping,
            series: styles,
            source: None,
            data: Some(data),
            ..self.clone()
        }
    }
}

impl Mapping {
    /// A mapping for `kind` read off `table`'s column types, for a table that
    /// arrives without one: a Sankey's first two text columns and first number
    /// column; a scatter's first number column as X and the other number columns
    /// as Y; otherwise the first text column (else the first column) as the
    /// category and every other number column as a series.
    pub fn guess(kind: ChartKind, table: &Table) -> Mapping {
        let numeric: Vec<(ColumnId, bool)> = table
            .columns
            .iter()
            .map(|c| (c.id, table.is_numeric(c.id)))
            .collect();
        let text = || numeric.iter().filter(|c| !c.1).map(|c| c.0);
        let numbers = || numeric.iter().filter(|c| c.1).map(|c| c.0);
        match kind {
            ChartKind::Sankey => {
                let mut t = text();
                Mapping {
                    from: t.next(),
                    to: t.next(),
                    value: numbers().next(),
                    ..Mapping::default()
                }
            }
            ChartKind::Scatter => {
                let x = numbers().next();
                Mapping {
                    category: x,
                    series: numbers().filter(|c| Some(*c) != x).collect(),
                    ..Mapping::default()
                }
            }
            _ => {
                let category = text().next().or_else(|| numeric.first().map(|c| c.0));
                Mapping {
                    category,
                    series: numbers().filter(|c| Some(*c) != category).collect(),
                    ..Mapping::default()
                }
            }
        }
    }
}

impl Options {
    /// `onto` with every option that differs between `before` and `after` set
    /// as `after` has it, the rest left as `onto` has them: one chart's edit
    /// carried to the others of a multi-selection, each keeping its own.
    pub fn carried(before: &Options, after: &Options, onto: &Options) -> Options {
        use serde_json::Value;
        // A patch's mark for a key `after` skips, which sets it back to its default;
        // a JSON null is a value of its own (`Waterfall::final_total` off).
        fn unset() -> Value {
            Value::Object(serde_json::Map::from_iter([(
                "\u{0}unset".to_string(),
                Value::Bool(true),
            )]))
        }
        fn diff(before: &Value, after: &Value) -> Option<Value> {
            match (before, after) {
                (Value::Object(b), Value::Object(a)) => {
                    let mut patch = serde_json::Map::new();
                    for (k, av) in a {
                        let d = match b.get(k) {
                            Some(bv) => diff(bv, av),
                            None => Some(av.clone()),
                        };
                        if let Some(d) = d {
                            patch.insert(k.clone(), d);
                        }
                    }
                    // A key `after` skips is at its default.
                    for k in b.keys().filter(|k| !a.contains_key(*k)) {
                        patch.insert(k.clone(), unset());
                    }
                    (!patch.is_empty()).then_some(Value::Object(patch))
                }
                (b, a) => (b != a).then(|| a.clone()),
            }
        }
        fn merge(into: &mut Value, patch: &Value) {
            match (into, patch) {
                (Value::Object(into), Value::Object(patch)) => {
                    for (k, v) in patch {
                        if *v == unset() {
                            into.remove(k);
                        } else {
                            merge(into.entry(k.clone()).or_insert(Value::Null), v);
                        }
                    }
                }
                (into, patch) => *into = patch.clone(),
            }
        }
        let (Ok(b), Ok(a), Ok(mut o)) = (
            serde_json::to_value(before),
            serde_json::to_value(after),
            serde_json::to_value(onto),
        ) else {
            return onto.clone();
        };
        if let Some(patch) = diff(&b, &a) {
            merge(&mut o, &patch);
        }
        serde_json::from_value(o).unwrap_or_else(|_| onto.clone())
    }

    /// Every colour the options hold; an unset one holds none.
    pub fn brushes(&self) -> impl Iterator<Item = &BrushSource> + '_ {
        [
            &self.value_axis.colour,
            &self.value_axis.grid.colour,
            &self.category_axis.colour,
            &self.category_axis.grid.colour,
            &self.text.ink,
            &self.plot.fill,
            &self.stroke.colour,
            &self.text.tick.ink,
            &self.text.value.ink,
            &self.text.legend.ink,
            &self.text.title.ink,
            &self.below_zero,
            &self.bars.stem_colour,
            &self.waterfall.increase,
            &self.waterfall.decrease,
            &self.waterfall.total,
        ]
        .into_iter()
        .flatten()
        .chain(&self.palette_group)
    }

    /// [`Self::brushes`], to rewrite in place.
    pub fn brushes_mut(&mut self) -> impl Iterator<Item = &mut BrushSource> + '_ {
        [
            &mut self.value_axis.colour,
            &mut self.value_axis.grid.colour,
            &mut self.category_axis.colour,
            &mut self.category_axis.grid.colour,
            &mut self.text.ink,
            &mut self.plot.fill,
            &mut self.stroke.colour,
            &mut self.text.tick.ink,
            &mut self.text.value.ink,
            &mut self.text.legend.ink,
            &mut self.text.title.ink,
            &mut self.below_zero,
            &mut self.bars.stem_colour,
            &mut self.waterfall.increase,
            &mut self.waterfall.decrease,
            &mut self.waterfall.total,
        ]
        .into_iter()
        .flatten()
        .chain(&mut self.palette_group)
    }

    /// These options as a chart style holds them: how the chart looks, with
    /// every option that says something about one chart's data at its default
    /// ([`Self::take_own`]).
    pub fn look(&self) -> Options {
        let mut o = self.clone();
        o.take_own(&Options::default());
        o
    }

    /// `look`'s look over these options' own fields: what a chart's options
    /// become when a chart style is written onto it.
    pub fn with_look(&self, look: &Options) -> Options {
        let mut o = look.clone();
        o.take_own(self);
        o
    }

    /// `from`'s options that belong to one chart rather than its look, set
    /// here: the stacking, order and bar shape, which change what the chart
    /// says as its kind does; the axis titles and typed ranges, which name this
    /// data; a pie's small-slice grouping; and a waterfall's direction and the
    /// categories it totals. The one list both [`Self::look`] and
    /// [`Self::with_look`] read.
    fn take_own(&mut self, from: &Options) {
        self.bars.stacking = from.bars.stacking;
        self.bars.order = from.bars.order;
        self.bars.shape = from.bars.shape;
        self.lines.area_stacking = from.lines.area_stacking;
        self.slices.order = from.slices.order;
        self.slices.group_below = from.slices.group_below;
        for (to, from) in [
            (&mut self.value_axis, &from.value_axis),
            (&mut self.category_axis, &from.category_axis),
        ] {
            to.title.clone_from(&from.title);
            to.range = from.range;
        }
        self.waterfall.horizontal = from.waterfall.horizontal;
        self.waterfall.totals.clone_from(&from.waterfall.totals);
        self.waterfall
            .final_total
            .clone_from(&from.waterfall.final_total);
    }

    /// Whether these options may be stored, on a chart or as a chart style's
    /// value: every number finite and in range, every text and list within its
    /// limit, and lollipops never stacked.
    pub(crate) fn validate(&self) -> Result<(), ChartRule> {
        let b = &self.bars;
        if b.shape == BarShape::Lollipops && b.stacking != Stacking::Apart {
            return Err(ChartRule::LollipopsStack);
        }
        let name = |s: &String| s.chars().count() <= MAX_CELL_CHARS;
        let w = &self.waterfall;
        let sizes = [b.stem, b.head]
            .iter()
            .all(|v| v.is_finite() && (0.0..=4096.0).contains(v));
        let wnames = w.totals.len() <= MAX_ROWS
            && w.totals.iter().all(name)
            && w.final_total.as_ref().is_none_or(name);
        let angle = self.radar.start_angle;
        let fade = self.fade.to;
        let radar = angle.is_finite()
            && (-360.0..=360.0).contains(&angle)
            && fade.is_finite()
            && (0.0..=100.0).contains(&fade);
        if !sizes || !wnames || !radar || self.palette_group.len() > MAX_PALETTE_GROUP {
            return Err(ChartRule::BadOption);
        }
        let len = |v: f64, max: f64| v.is_finite() && (0.0..=max).contains(&v);
        let unit = |v: f64| len(v, 1.0);
        let pct = |v: f64| len(v, 100.0);
        let px = |v: f64| len(v, crate::build::GEOMETRY_LIMIT);
        let size = |v: f64| len(v, 4096.0);
        let text = |s: &str| s.chars().count() <= MAX_TEXT_CHARS;
        let format = |f: &NumberFormat| {
            f.decimals.is_none_or(|d| d <= 10) && text(&f.prefix) && text(&f.suffix)
        };
        let axis = |a: &Axis| {
            text(&a.title)
                && a.range
                    .is_none_or(|(lo, hi)| (hi - lo).is_finite() && lo < hi)
                && a.tick_count.is_none_or(|n| (1..=100).contains(&n))
                && format(&a.format)
                && a.line_width > 0.0
                && size(a.line_width)
                && a.grid.width > 0.0
                && size(a.grid.width)
        };
        let role = |r: &RoleText| r.weight.is_none_or(|w| (1..=1000).contains(&w));
        let b = &self.bars;
        let l = &self.lines;
        let s = &self.slices;
        let p = &self.points;
        let f = &self.flows;
        let t = &self.text;
        let ok = pct(b.group_gap)
            && pct(b.bar_gap)
            && px(b.radius)
            && px(self.plot.radius)
            && size(self.stroke.width)
            && size(l.width)
            && size(l.point_size)
            && unit(l.area_opacity)
            && pct(s.hole)
            && len(s.pad_angle, 45.0)
            && px(s.radius)
            && s.start_angle.is_finite()
            && (-360.0..=360.0).contains(&s.start_angle)
            && s.group_below.is_none_or(pct)
            && size(p.size)
            && unit(p.opacity)
            && size(p.max_size)
            && size(f.node_width)
            && size(f.node_gap)
            && px(f.node_radius)
            && unit(f.opacity)
            && axis(&self.value_axis)
            && axis(&self.category_axis)
            && format(&self.values.format)
            && text(&t.family)
            && (1..=1000).contains(&t.weight)
            && [&t.tick, &t.value, &t.legend, &t.title]
                .into_iter()
                .all(role)
            && [t.tick_size, t.value_size, t.legend_size, t.title_size]
                .iter()
                .all(|v| *v > 0.0 && size(*v))
            && px(self.padding)
            && px(self.gap);
        if ok {
            Ok(())
        } else {
            Err(ChartRule::BadOption)
        }
    }
}

/// Why a chart may not sit where it is (`charts.md`, *Where a chart may sit*).
#[derive(Clone, Copy, Debug, PartialEq, Eq, thiserror::Error)]
pub enum PlacementRule {
    #[error("is a chart on a layer that is not a frame")]
    NotAFrame,
    /// Inside a main component, an instance, a kit main, a boolean or a mask.
    #[error("is a chart that can't be part of a component — flatten it first")]
    InComponent,
    #[error("is a chart that sets modes, width queries, guides or layout grids")]
    FrameSettings,
    /// A guide scoped to a frame the chart's regeneration writes and deletes.
    #[error("is part of a chart, which holds no guides")]
    GuideOnPart,
}

/// The chart rules over the whole document, after every transaction and on load:
/// a chart is a frame, outside any main, instance, kit main, boolean or mask, and
/// it sets no modes or width queries, holds no guides or layout grids and is not
/// a component set; and no guide is scoped to one of its parts, which its
/// regeneration deletes. Whole-document for `component::check`'s reason: moving a
/// chart's ancestor into a main dirties the ancestor, not the chart.
pub fn check(
    nodes: &FxHashMap<NodeId, Node>,
    vault: Option<NodeId>,
    guide_owners: impl IntoIterator<Item = NodeId>,
) -> Result<(), (NodeId, PlacementRule)> {
    let charts: Vec<&Node> = nodes.values().filter(|n| n.chart.is_some()).collect();
    if charts.is_empty() {
        return Ok(());
    }
    let owners: FxHashSet<NodeId> = guide_owners.into_iter().collect();
    for &o in &owners {
        let mut at = nodes.get(&o).and_then(|n| n.parent);
        while let Some(p) = at.and_then(|p| nodes.get(&p)) {
            if p.chart.is_some() {
                return Err((o, PlacementRule::GuideOnPart));
            }
            at = p.parent;
        }
    }
    for n in charts {
        if !matches!(n.kind, NodeKind::Artboard { .. }) {
            return Err((n.id, PlacementRule::NotAFrame));
        }
        if !n.grids.is_empty()
            || !n.modes.is_empty()
            || !n.queries.is_empty()
            || n.set.is_some()
            || owners.contains(&n.id)
        {
            return Err((n.id, PlacementRule::FrameSettings));
        }
        let mut at = Some(n);
        let mut first = true;
        while let Some(m) = at {
            let barred = m.component
                || m.link.is_some()
                || m.mask
                || Some(m.id) == vault
                || (!first && matches!(m.kind, NodeKind::Boolean { .. }));
            if barred {
                return Err((n.id, PlacementRule::InComponent));
            }
            first = false;
            at = m.parent.and_then(|p| nodes.get(&p));
        }
    }
    Ok(())
}

/// The chart frame `id` is a part of: its nearest strict ancestor carrying a
/// chart. `None` for a node outside every chart and for a chart frame itself.
pub fn owner(doc: &crate::Document, id: NodeId) -> Option<NodeId> {
    let mut at = doc.get(id)?.parent;
    while let Some(p) = at {
        let n = doc.get(p)?;
        if n.chart.is_some() {
            return Some(p);
        }
        at = n.parent;
    }
    None
}

/// The operation that rewrites chart `id`'s colour bindings by `map` (a
/// binding's new paint at the chart frame, `None` to keep it): its description
/// alone, which leaves the chart stale so settle remakes the parts. A part id in
/// an undo step would outlive the regeneration that deletes it, so the parts
/// are never written here; until settle they may draw a variable the same step
/// deleted, which `variable::check` allows while the chart is stale. A
/// variable's delete and detach go through here. Empty when nothing changes.
pub fn rebind_ops(
    doc: &crate::Document,
    id: NodeId,
    map: &dyn Fn(NodeId, &BrushSource) -> Option<BrushSource>,
) -> Vec<Operation> {
    let Some(chart) = doc.get(id).and_then(|n| n.chart()) else {
        return Vec::new();
    };
    let mut next = chart.clone();
    let mut wrote = false;
    for b in next.brushes_mut().filter(|b| b.is_bound()) {
        if let Some(to) = map(id, b) {
            *b = to;
            wrote = true;
        }
    }
    if !wrote {
        return Vec::new();
    }
    vec![Operation::SetChart {
        id,
        chart: Some(Box::new(next)),
    }]
}

/// The charts `ops` regenerate: those it writes parts for.
pub fn regenerated(ops: &[Operation]) -> FxHashSet<NodeId> {
    ops.iter()
        .filter_map(|op| match op {
            Operation::SetChartParts { id, .. } => Some(*id),
            _ => None,
        })
        .collect()
}

/// Refuse `op`, against `doc` before it applies, when it writes a chart's parts
/// outside that chart's regeneration (`regenerated`): any edit of a node under a
/// chart frame, a child added to or moved into one, and the frame's own layout
/// (`display`), which the regeneration writes. A whole chart created, inserted,
/// moved or deleted is not a part edit.
pub fn guard(
    doc: &crate::Document,
    op: &Operation,
    regenerated: &FxHashSet<NodeId>,
) -> Result<(), crate::op::OpError> {
    let free = |chart: NodeId| regenerated.contains(&chart);
    let is_chart = |id: NodeId| doc.get(id).is_some_and(|n| n.chart.is_some());
    let refuse = |id: NodeId| Err(crate::op::OpError::ChartPart(id));
    // A node under a chart, or (for `into`) a chart frame or a node under one.
    let part = |id: NodeId| owner(doc, id).filter(|c| !free(*c));
    let into = |parent: NodeId| {
        if is_chart(parent) && !free(parent) {
            Some(parent)
        } else {
            part(parent)
        }
    };
    let hit = match op {
        Operation::CreateNode { parent, .. } | Operation::InsertSubtree { parent, .. } => {
            into(*parent).map(|_| *parent)
        }
        Operation::Reparent { id, new_parent, .. } => part(*id)
            .map(|_| *id)
            .or_else(|| into(*new_parent).map(|_| *id)),
        Operation::SetDisplay { id, .. } if is_chart(*id) && !free(*id) => Some(*id),
        other => {
            let ids = match other {
                Operation::DeleteNode { id } | Operation::Reorder { id, .. } => vec![*id],
                _ => other.overwrites().into_iter().collect(),
            };
            ids.into_iter().find(|id| part(*id).is_some())
        }
    };
    match hit {
        Some(id) => refuse(id),
        None => Ok(()),
    }
}

/// The description's drop alone, [`flatten_ops`]' first operation: on its own
/// the commit refuses it when a part draws a binding its variable's scope
/// leaves out. `None` for a node that is not a chart.
pub fn flatten(doc: &crate::Document, id: NodeId) -> Option<Operation> {
    doc.get(id)?.chart.as_ref()?;
    Some(Operation::SetChart { id, chart: None })
}

/// *Flatten chart*: the frame keeps everything it is, its parts stay where they
/// are, and the description goes; a part's binding its variable's scope refuses
/// (a line's stroke drawing a colour scoped to fills) detaches to the colour it
/// drew. The drop comes first, so the detaches no longer write a part. A
/// chart style the chart is bound to is unbound beside it, asked for as the
/// flatten's own. `None` for a node that is not a chart.
pub fn flatten_ops(doc: &crate::Document, id: NodeId) -> Option<Vec<Operation>> {
    let drop = flatten(doc, id)?;
    let node = doc.get(id)?;
    let parts: Vec<NodeId> = node.chart()?.parts.iter().map(|(_, p)| *p).collect();
    let mut ops = vec![drop];
    if node.styles().chart.is_some() {
        ops.push(Operation::SetChartStyle { id, style: None });
    }
    ops.extend(crate::variable::out_of_scope_ops(doc, &parts));
    Some(ops)
}

#[cfg(test)]
mod tests;
