//! The Chart card (`charts.md`, *The Chart card*): a chart frame's description,
//! edited section by section. Every edit is one `SetChart` of the whole
//! description; a scrubbed field previews through `EditorSession::set_preview`,
//! which draws the parts the edit would generate.

use super::paint::{ChartColour, PaintSlot};
use crate::app::MastikApp;
use crate::theme::{self, color, icon};
use crate::ui::{self, Prefix, Scrub};
use eframe::egui;
use mastik_core::chart::{
    self, Align, AxisSide, Chart, ChartKind, Curve, FlowAlign, FlowColour, LabelAngle,
    LegendPosition, NodeLabels, Palette, PieLabel, PointShape, PointStyle, Preset, Stacking,
    ValuePosition,
};
use mastik_core::kurbo;
use mastik_core::style::StyleSlot;
use mastik_core::{Brush, NodeId, Operation, Transaction};
use std::hash::{Hash, Hasher};

/// The sections a card opens folded: the options a new chart rarely needs first.
pub(crate) const FOLDED: [&str; 5] = [
    "Chart marks",
    "Chart axes",
    "Chart values",
    "Chart legend",
    "Chart text",
];

/// A Style cell's height: the picture and its name under it.
/// Past two control heights, so it reads (and audits) as a tile, not a control.
const STYLE_CELL_H: f32 = 61.0;
/// The picture inside a Style cell at its largest; a narrower cell draws it smaller.
const STYLE_PICTURE: egui::Vec2 = super::chart_picture::SIZE;
/// The least room between a Style cell's edge and its picture, so the lit
/// cell's ring never touches the picture's frame.
const STYLE_INSET: f32 = 5.0;
/// The room above a Style cell's picture: the same as each side's.
const STYLE_TOP: f32 = STYLE_INSET;
/// The gap between Style cells, under `ui::CARD_COL_GAP`: the cells' own
/// insets already space the pictures apart, and the width goes to the pictures.
const STYLE_GAP: f32 = 2.0;
/// The number the *Format* dropdown writes as its sample.
const FORMAT_SAMPLE: f64 = 12_345.6;
/// The *Format* dropdown's least popup width: two text fields to a row.
const FORMAT_POPUP_W: f32 = 220.0;
/// The series style menu's width, past its longest row.
const SERIES_MENU_W: f32 = 140.0;

/// The category and the series of chart `id`'s mark at world point `world`,
/// as a highlight names them: a bar's or lollipop's both (a one-series chart's
/// category only), a line's or legend entry's series (a radar's too), a slice's
/// category, a waterfall step's name. A scatter names its series or group.
pub(crate) fn mark_names(
    app: &MastikApp,
    id: NodeId,
    world: kurbo::Point,
) -> (Option<String>, Option<String>) {
    use chart::PartRole as R;
    let doc = &app.session.doc;
    let Some(chart) = doc.get(id).and_then(|n| n.chart()) else {
        return (None, None);
    };
    let Some(key) =
        mastik_core::chart_part_at(&app.session.resolved, doc, app.session.page, id, world)
    else {
        return (None, None);
    };
    let t = &chart.table;
    let series_name = |col: u32| t.column(chart::ColumnId(col)).map(|c| c.name.clone());
    // The categories as drawn, in the order the chart draws them.
    let cat_col = chart.mapping.category.and_then(|c| t.index_of(c));
    let read: Vec<String> = t
        .rows
        .iter()
        .filter_map(|r| cat_col.map(|c| r[c].trim().to_string()))
        .filter(|s| !s.is_empty())
        .collect();
    let drawn = |i: u32| -> Option<String> {
        let i = i as usize;
        let at = chart::category_order(chart).map_or(i, |o| o.get(i).copied().unwrap_or(i));
        read.get(at).cloned()
    };
    let many = chart.mapping.series.len() > 1;
    match (chart.kind, key.role) {
        (ChartKind::Pie, R::Slice | R::ValueLabel) => (read.get(key.index as usize).cloned(), None),
        (ChartKind::Scatter, R::Point | R::LegendSwatch | R::LegendLabel | R::LegendItem) => {
            let name = series_name(key.series).or_else(|| {
                chart::named_marks(chart).and_then(|m| {
                    m.into_iter()
                        .nth(key.series.checked_sub(chart::MAX_COLUMNS as u32)? as usize)
                        .map(|(n, _)| n)
                })
            });
            (None, name)
        }
        // A waterfall is one series: a step names its category, counted over its steps.
        (ChartKind::Waterfall, R::Bar | R::ValueLabel | R::CategoryLabel) => (
            chart::waterfall_steps(chart)
                .into_iter()
                .nth(key.index as usize),
            None,
        ),
        (
            ChartKind::Column | ChartKind::Bar,
            R::Bar | R::ValueLabel | R::Total | R::Head | R::Stem,
        ) => (
            drawn(key.index),
            if many && key.role != R::Total {
                series_name(key.series)
            } else {
                None
            },
        ),
        (_, R::CategoryLabel) => (drawn(key.index), None),
        (
            ChartKind::Line | ChartKind::Area | ChartKind::Radar,
            R::Line
            | R::Area
            | R::Point
            | R::EndLabel
            | R::LegendSwatch
            | R::LegendLabel
            | R::LegendItem,
        ) => (None, series_name(key.series)),
        (_, R::LegendSwatch | R::LegendLabel | R::LegendItem) if many => {
            (None, series_name(key.series))
        }
        _ => (None, None),
    }
}

impl MastikApp {
    /// Chart `id`'s highlight set to `target`, as *Highlight ‹name›* or *Clear
    /// highlight*.
    pub(crate) fn highlight_chart(&mut self, id: NodeId, target: chart::Highlighted) {
        let Some(chart) = self.session.doc.get(id).and_then(|n| n.chart()) else {
            return;
        };
        let mut next = chart.clone();
        let label = if target == chart::Highlighted::None {
            "Clear highlight"
        } else {
            "Highlight"
        };
        next.highlight.target = target;
        self.chart_commit(id, label, next);
    }
}

/// A reference popover's width.
const REFERENCE_POPOVER_W: f32 = 240.0;

/// The inside of every colour row on the Chart card, drawn into a
/// `ui::field_row_pressable`: the swatch chip on `ground`, then `name` in `ink`,
/// cut to fit, and whatever `right` adds right-aligned. A muted `detail` text
/// is the usual right side ([`detail_text`]).
fn colour_row_face(
    ui: &mut egui::Ui,
    ground: egui::Color32,
    swatch: ui::Swatch,
    name: &str,
    ink: egui::Color32,
    right: impl FnOnce(&mut egui::Ui),
) {
    ui.spacing_mut().item_spacing.x = ui::COLOUR_CHIP_GAP;
    let (chip, _) =
        ui.allocate_exact_size(egui::Vec2::splat(ui::COLOUR_CHIP), egui::Sense::hover());
    ui::paint_swatch(ui.painter(), chip, ground, swatch);
    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
        right(ui);
        ui.with_layout(egui::Layout::left_to_right(egui::Align::Center), |ui| {
            ui.add(
                egui::Label::new(egui::RichText::new(name).size(ui::CONTROL_PT).color(ink))
                    .truncate(),
            );
        });
    });
}

/// A colour row's muted right-hand text.
fn detail_text(ui: &mut egui::Ui, text: &str) -> egui::Response {
    ui.label(
        egui::RichText::new(text)
            .size(ui::CONTROL_PT)
            .color(theme::text::muted()),
    )
}

/// Where a reference stands, in a row's words.
fn at_words(a: &chart::At) -> String {
    use chart::{At, Stat};
    match a {
        At::Value(v) => chart::scale::format(
            *v,
            &chart::NumberFormat::default(),
            chart::scale::decimals_of(*v).min(3),
        ),
        At::Category(c) => c.clone(),
        At::Stat { stat, .. } => match stat {
            Stat::Average => "Average",
            Stat::Median => "Median",
            Stat::Min => "Minimum",
            Stat::Max => "Maximum",
        }
        .to_string(),
        At::AxisStart => "Start".into(),
        At::AxisEnd => "End".into(),
    }
}

/// `now` with one name picked in the Highlight dropdown: a category (`Ok`) or
/// a series (`Err`) turned on or off, a pick of the other kind starting over;
/// `None` clears it.
fn toggled(now: &chart::Highlighted, pick: Option<Result<String, String>>) -> chart::Highlighted {
    use chart::Highlighted as H;
    let flip = |list: &[String], name: String| {
        let mut l = list.to_vec();
        match l.iter().position(|n| *n == name) {
            Some(i) => {
                l.remove(i);
            }
            None => l.push(name),
        }
        l
    };
    let next = match (now, pick) {
        (_, None) => H::None,
        (H::Categories(l), Some(Ok(n))) => H::Categories(flip(l, n)),
        (_, Some(Ok(n))) => H::Categories(vec![n]),
        (H::Series(l), Some(Err(n))) => H::Series(flip(l, n)),
        (_, Some(Err(n))) => H::Series(vec![n]),
    };
    match next {
        H::Categories(l) | H::Series(l) if l.is_empty() => H::None,
        other => other,
    }
}

/// Each label role in `ChartColour::Role`'s order: its row's name, and its undo
/// steps for its colour and its weight.
pub(super) const ROLE_STEPS: [(&str, &str, &str); 4] = [
    ("Tick labels", "Tick labels colour", "Tick labels weight"),
    ("Values", "Values colour", "Values weight"),
    ("Legend", "Legend colour", "Legend weight"),
    ("Axis titles", "Axis titles colour", "Axis titles weight"),
];

/// The orders a column, bar or pie chart draws its categories in.
const ORDERS: &[(chart::Order, &str)] = &[
    (chart::Order::Table, "Table order"),
    (chart::Order::Descending, "Largest first"),
    (chart::Order::Ascending, "Smallest first"),
];
/// What the card measured of its chart, kept while the description stays the same:
/// the Style cells' pictures and the generator's left-out values.
pub(crate) struct ChartCardCache {
    id: NodeId,
    key: u64,
    /// The Style pictures, drawn on first show at [`Self::thumb_px`].
    thumbs: Vec<(Preset, egui::TextureHandle)>,
    thumb_px: [usize; 2],
    picture: super::chart_picture::Picture,
    /// The chart's first colours, its bindings resolved.
    colours: [mastik_core::peniko::Color; super::chart_picture::COLOURS],
    left_out: Vec<String>,
    refusal: Option<String>,
    /// Whether the chart draws a progress ring's track, which the Marks section
    /// then offers a colour for.
    track: bool,
}

/// Where `now` stands in `options`; past the end, lighting none, when it is
/// not there.
fn lit<T: PartialEq>(options: &[(T, &str)], now: T) -> usize {
    options
        .iter()
        .position(|(v, _)| *v == now)
        .unwrap_or(usize::MAX)
}

/// One variant of a kind: its segment's name, and the option it writes,
/// which also tells whether a chart is at it.
type Variant = (&'static str, fn(&mut chart::Options));

/// A kind's variants as the Type section's segments name them, one table so a
/// name never drifts from what it writes; empty for a kind with none to pick.
fn variant_table(kind: ChartKind) -> &'static [Variant] {
    match kind {
        ChartKind::Column | ChartKind::Bar => &[
            ("Grouped", |o| o.bars.stacking = Stacking::Apart),
            ("Stacked", |o| o.bars.stacking = Stacking::Stacked),
            ("100%", |o| o.bars.stacking = Stacking::Percent),
        ],
        ChartKind::Line => &[
            ("Straight", |o| o.lines.curve = Curve::Straight),
            ("Smooth", |o| o.lines.curve = Curve::Smooth),
            ("Step", |o| o.lines.curve = Curve::Step),
        ],
        ChartKind::Area => &[
            ("Overlapping", |o| o.lines.area_stacking = Stacking::Apart),
            ("Stacked", |o| o.lines.area_stacking = Stacking::Stacked),
            ("100%", |o| o.lines.area_stacking = Stacking::Percent),
        ],
        ChartKind::Pie => &[
            ("Pie", |o| o.slices.donut = false),
            ("Donut", |o| o.slices.donut = true),
        ],
        // The variant is the orientation, as Column and Bar are one model.
        ChartKind::Waterfall => &[
            ("Column", |o| o.waterfall.horizontal = false),
            ("Bar", |o| o.waterfall.horizontal = true),
        ],
        ChartKind::Radar => &[
            ("Lines", |o| o.radar.filled = false),
            ("Filled", |o| o.radar.filled = true),
        ],
        ChartKind::Scatter | ChartKind::Sankey => &[],
    }
}

/// The kind's variant names with the one `chart` is at — none lit
/// (`usize::MAX`) when no variant leaves its options as they are; `None` for
/// a kind with none to pick.
fn variants(chart: &Chart) -> Option<(Vec<&'static str>, usize)> {
    let table = variant_table(chart.kind);
    if table.is_empty() {
        return None;
    }
    let at = table
        .iter()
        .position(|(_, set)| {
            let mut o = chart.options.clone();
            set(&mut o);
            o == chart.options
        })
        .unwrap_or(usize::MAX);
    Some((table.iter().map(|(name, _)| *name).collect(), at))
}

/// `chart` at variant `i` of its kind ([`variants`]).
fn with_variant(chart: &Chart, i: usize) -> Chart {
    let mut next = chart.clone();
    if let Some((_, set)) = variant_table(chart.kind).get(i) {
        set(&mut next.options);
    }
    next
}

/// What a chart coloured by name calls its named marks: the Series section's
/// title and the undo step a mark's colour makes. `None` for a chart coloured
/// by series.
pub(crate) fn named_mark_noun(chart: &Chart) -> Option<(&'static str, &'static str)> {
    match chart.kind {
        ChartKind::Sankey => Some(("Nodes", "Node colour")),
        ChartKind::Pie => Some(("Slices", "Slice colour")),
        ChartKind::Scatter if chart.mapping.group.is_some() => Some(("Groups", "Group colour")),
        ChartKind::Column | ChartKind::Bar if chart::by_category(chart) => {
            Some(("Categories", "Category colour"))
        }
        _ => None,
    }
}

/// The paint a chart slot draws in, bound or not: its own, else what it
/// derives (the palette's, the theme's); `None` for an unset plot ground.
pub(crate) fn fill_of(chart: &Chart, colour: ChartColour) -> Option<mastik_core::BrushSource> {
    match colour {
        ChartColour::Series(col) => chart::series_fill(chart, col),
        ChartColour::Node(i) => Some(chart::named_marks(chart)?.into_iter().nth(i)?.1),
        ChartColour::Ink => Some(chart.ink()),
        ChartColour::Axis(value) => Some(chart.axis_colour(value)),
        ChartColour::Grid(value) => Some(chart.grid_colour(value)),
        ChartColour::Neutral => Some(chart.axis_colour(true)),
        ChartColour::Track => Some(chart.grid_colour(true)),
        ChartColour::Plot => chart.options.plot.fill.clone(),
        // Unset under *Solid* it is the ground; under *Outline* each mark's own.
        ChartColour::Stroke => match (&chart.options.stroke.colour, chart.options.stroke.style) {
            (Some(c), _) => Some(c.clone()),
            (None, chart::StrokeStyle::Solid) => Some(Brush::Solid(chart.chrome().ground).into()),
            (None, chart::StrokeStyle::Outline) => None,
        },
        ChartColour::Role(r) => Some(role_of(chart, r).ink.clone().unwrap_or_else(|| chart.ink())),
        ChartColour::Muted => Some(chart.muted()),
        ChartColour::BelowZero => chart.options.below_zero.clone(),
        ChartColour::Reference(id) => chart
            .references
            .iter()
            .find(|r| r.id == id)
            .map(|r| r.colour.clone()),
        // Unset, each series' own: no one colour.
        ChartColour::Stem => chart.options.bars.stem_colour.clone(),
        ChartColour::Step(i) => {
            let w = &chart.options.waterfall;
            let (own, step) = step_slot(w, i);
            Some(own.clone().unwrap_or_else(|| {
                Brush::Solid(chart::Waterfall::starting(step, chart.drawn_dark())).into()
            }))
        }
    }
}

/// A waterfall step colour's slot and kind, by [`ChartColour::Step`]'s number.
fn step_slot(w: &chart::Waterfall, i: u8) -> (&Option<mastik_core::BrushSource>, chart::Step) {
    match i {
        0 => (&w.increase, chart::Step::Increase),
        1 => (&w.decrease, chart::Step::Decrease),
        _ => (&w.total, chart::Step::Total),
    }
}

/// One kind of label's own type (`ChartText`'s roles), by [`ChartColour::Role`]'s
/// number.
fn role_of(chart: &Chart, r: u8) -> &chart::RoleText {
    let t = &chart.options.text;
    match r {
        0 => &t.tick,
        1 => &t.value,
        2 => &t.legend,
        _ => &t.title,
    }
}

fn role_of_mut(chart: &mut Chart, r: u8) -> &mut chart::RoleText {
    let t = &mut chart.options.text;
    match r {
        0 => &mut t.tick,
        1 => &mut t.value,
        2 => &mut t.legend,
        _ => &mut t.title,
    }
}

/// Whether a chart slot holds a colour of its own rather than one it derives.
pub(crate) fn is_set(chart: &Chart, colour: ChartColour) -> bool {
    let o = &chart.options;
    match colour {
        ChartColour::Series(col) => chart::series_fill_set(chart, col),
        ChartColour::Node(i) => chart::named_marks(chart)
            .and_then(|m| m.into_iter().nth(i))
            .is_some_and(|(name, _)| {
                chart
                    .nodes
                    .iter()
                    .any(|s| s.name == name && s.fill.is_some())
            }),
        ChartColour::Ink => o.text.ink.is_some(),
        ChartColour::Axis(true) | ChartColour::Neutral => o.value_axis.colour.is_some(),
        ChartColour::Axis(false) => o.category_axis.colour.is_some(),
        ChartColour::Grid(true) | ChartColour::Track => o.value_axis.grid.colour.is_some(),
        ChartColour::Grid(false) => o.category_axis.grid.colour.is_some(),
        ChartColour::Plot => o.plot.fill.is_some(),
        ChartColour::Stroke => o.stroke.colour.is_some(),
        ChartColour::Role(r) => role_of(chart, r).ink.is_some(),
        ChartColour::Muted => chart.highlight.muted.is_some(),
        ChartColour::BelowZero | ChartColour::Reference(_) => true,
        ChartColour::Stem => o.bars.stem_colour.is_some(),
        ChartColour::Step(i) => step_slot(&o.waterfall, i).0.is_some(),
    }
}

/// The colour a chart slot draws in on chart `id`, solid, a binding read in
/// the mode in force there.
pub(crate) fn colour_of(
    doc: &mastik_core::Document,
    id: NodeId,
    chart: &Chart,
    colour: ChartColour,
) -> Option<mastik_core::peniko::Color> {
    let fill = fill_of(chart, colour)?;
    match doc.resolve_brush(id, &fill).into_owned() {
        Brush::Solid(c) => Some(c),
        _ => None,
    }
}

/// Series `col`'s colour on chart `id`, solid: the data window's header swatch.
pub(crate) fn series_colour(
    doc: &mastik_core::Document,
    id: NodeId,
    chart: &Chart,
    col: chart::ColumnId,
) -> Option<mastik_core::peniko::Color> {
    colour_of(doc, id, chart, ChartColour::Series(col))
}

/// `chart` with the slot's colour set to `c`.
pub(crate) fn with_colour(
    chart: &Chart,
    colour: ChartColour,
    c: mastik_core::peniko::Color,
) -> Chart {
    with_fill(
        chart,
        colour,
        mastik_core::BrushSource::Literal(Brush::Solid(c)),
    )
}

/// `chart` with the slot's paint set to `fill`, bound or not.
pub(crate) fn with_fill(
    chart: &Chart,
    colour: ChartColour,
    fill: mastik_core::BrushSource,
) -> Chart {
    with_paint(chart, colour, Some(fill))
}

/// `chart` with the slot's paint set to `fill`; `None` puts it back to what it
/// derives (*From the theme*), or takes the plot ground away.
pub(crate) fn with_paint(
    chart: &Chart,
    colour: ChartColour,
    fill: Option<mastik_core::BrushSource>,
) -> Chart {
    match colour {
        ChartColour::Series(col) => chart::with_series_fill(chart, col, fill),
        ChartColour::Node(i) => {
            match chart::named_marks(chart).and_then(|m| m.into_iter().nth(i)) {
                Some((name, _)) => chart::with_node_fill(chart, &name, fill),
                None => chart.clone(),
            }
        }
        ChartColour::Role(r) => {
            let mut next = chart.clone();
            role_of_mut(&mut next, r).ink = fill;
            next
        }
        ChartColour::Muted => {
            let mut next = chart.clone();
            next.highlight.muted = fill;
            next
        }
        ChartColour::BelowZero => {
            let mut next = chart.clone();
            next.options.below_zero = fill;
            next
        }
        ChartColour::Reference(id) => {
            let mut next = chart.clone();
            if let (Some(r), Some(f)) = (next.references.iter_mut().find(|r| r.id == id), fill) {
                r.colour = f;
            }
            next
        }
        ChartColour::Stem => {
            let mut next = chart.clone();
            next.options.bars.stem_colour = fill;
            next
        }
        ChartColour::Step(i) => {
            let mut next = chart.clone();
            let w = &mut next.options.waterfall;
            *match i {
                0 => &mut w.increase,
                1 => &mut w.decrease,
                _ => &mut w.total,
            } = fill;
            next
        }
        ChartColour::Ink
        | ChartColour::Axis(_)
        | ChartColour::Grid(_)
        | ChartColour::Neutral
        | ChartColour::Track
        | ChartColour::Plot
        | ChartColour::Stroke => {
            let mut next = chart.clone();
            let o = &mut next.options;
            *match colour {
                ChartColour::Axis(true) | ChartColour::Neutral => &mut o.value_axis.colour,
                ChartColour::Axis(false) => &mut o.category_axis.colour,
                ChartColour::Grid(true) | ChartColour::Track => &mut o.value_axis.grid.colour,
                ChartColour::Grid(false) => &mut o.category_axis.grid.colour,
                ChartColour::Plot => &mut o.plot.fill,
                ChartColour::Stroke => &mut o.stroke.colour,
                _ => &mut o.text.ink,
            } = fill;
            next
        }
    }
}

/// What the cache is keyed by: everything the generator reads but the parts,
/// and the display scale and theme the Style pictures are rasterised in.
fn description_key(id: NodeId, chart: &Chart, size: kurbo::Size, ppp: f32) -> u64 {
    let mut h = std::hash::DefaultHasher::new();
    id.hash(&mut h);
    ppp.to_bits().hash(&mut h);
    format!("{:?}", theme::color::theme()).hash(&mut h);
    format!("{:?}", chart.kind).hash(&mut h);
    format!("{:?}", chart.mapping).hash(&mut h);
    format!("{:?}", chart.series).hash(&mut h);
    format!("{:?}", chart.nodes).hash(&mut h);
    format!("{:?}", chart.options).hash(&mut h);
    format!("{:?}", chart.grounded).hash(&mut h);
    chart.table.rows.hash(&mut h);
    for c in &chart.table.columns {
        c.name.hash(&mut h);
    }
    size.width.to_bits().hash(&mut h);
    size.height.to_bits().hash(&mut h);
    h.finish()
}

/// The device pixels a Style cell `cell_w` wide draws its picture in at `ppp`:
/// [`STYLE_PICTURE`], or smaller in its proportions to keep [`STYLE_INSET`]
/// each side. Whole pixels both ways, so the texture is never stretched.
fn style_picture_px(cell_w: f32, ppp: f32) -> [usize; 2] {
    let w = (STYLE_PICTURE.x * ppp)
        .min((cell_w - 2.0 * STYLE_INSET) * ppp)
        .floor()
        .max(1.0);
    let h = (w * STYLE_PICTURE.y / STYLE_PICTURE.x).round().max(1.0);
    [w as usize, h as usize]
}

/// A Style cell's picture: `preset`'s drawing of `picture` in `colours` and the
/// theme's ink, rasterised at `px` with its edges on the pixel grid.
fn style_picture(
    ctx: &egui::Context,
    picture: super::chart_picture::Picture,
    colours: &[mastik_core::peniko::Color; super::chart_picture::COLOURS],
    preset: Preset,
    px: [usize; 2],
) -> Option<egui::TextureHandle> {
    use super::chart_picture as pictures;
    let k = px[0] as f64 / STYLE_PICTURE.x as f64;
    let art = pictures::art(picture, preset, colours, &pictures::Ink::of_theme(), k);
    let rgba = mastik_render::rasterize_paths(px[0] as u32, px[1] as u32, &art)?;
    let image = egui::ColorImage::from_rgba_unmultiplied(px, &rgba);
    Some(ctx.load_texture("chart-style", image, egui::TextureOptions::LINEAR))
}

impl MastikApp {
    /// Chart `id`'s slot `colour` as bound: its variable and `%`; `None` when
    /// it holds a colour of its own.
    pub(crate) fn chart_binding(
        &self,
        id: NodeId,
        colour: ChartColour,
    ) -> Option<(mastik_core::VarId, f32)> {
        match fill_of(self.session.chart_drawn(id)?, colour)? {
            mastik_core::BrushSource::Bound { var, alpha } => Some((var, alpha)),
            mastik_core::BrushSource::Literal(_) => None,
        }
    }

    /// The charts a write to chart `id`'s slot `colour` lands on: every selected
    /// chart when `id` is one of a selection of charts and the colour is the
    /// chart's own, as the card's other edits write each; only `id` for a
    /// series or node colour, which belongs to that chart's table.
    pub(crate) fn chart_slot_targets(&self, id: NodeId, colour: ChartColour) -> Vec<NodeId> {
        // A series', a mark's or a reference's colour belongs to that chart.
        if matches!(
            colour,
            ChartColour::Series(_) | ChartColour::Node(_) | ChartColour::Reference(_)
        ) {
            return vec![id];
        }
        match self.selected_charts() {
            Some(ids) if ids.contains(&id) => ids,
            _ => vec![id],
        }
    }

    /// The `SetChart`s binding slot `colour` of chart `id` (and of the charts
    /// selected with it, [`Self::chart_slot_targets`]) to `var` at `alpha` (each
    /// one's `%` kept when `None`), or for `var` `None` detaching each to the
    /// colour it draws.
    pub(crate) fn chart_bind_tx(
        &self,
        id: NodeId,
        colour: ChartColour,
        var: Option<mastik_core::VarId>,
        alpha: Option<f32>,
    ) -> Transaction {
        let doc = &self.session.doc;
        let mut ops = Vec::new();
        for at in self.chart_slot_targets(id, colour) {
            let Some(chart) = doc.get(at).and_then(|n| n.chart()) else {
                continue;
            };
            let Some(now) = fill_of(chart, colour) else {
                continue;
            };
            let fill = match var {
                Some(var) => mastik_core::BrushSource::Bound {
                    var,
                    alpha: alpha.unwrap_or(match &now {
                        mastik_core::BrushSource::Bound { alpha, .. } => *alpha,
                        mastik_core::BrushSource::Literal(Brush::Solid(c)) => c.components[3],
                        mastik_core::BrushSource::Literal(_) => 1.0,
                    }),
                },
                None if now.is_bound() => doc.resolve_brush(at, &now).into_owned().into(),
                None => continue,
            };
            ops.push(Operation::SetChart {
                id: at,
                chart: Some(Box::new(with_fill(chart, colour, fill))),
            });
        }
        Transaction(ops)
    }

    /// The `SetChart`s setting the `%` of slot `colour` to `alpha` on chart `id`
    /// and the charts selected with it, each keeping its own variable; a slot
    /// holding a colour of its own is left alone, as a layer's paint is.
    pub(crate) fn chart_alpha_tx(
        &self,
        id: NodeId,
        colour: ChartColour,
        alpha: f32,
    ) -> Transaction {
        let doc = &self.session.doc;
        let ops = self
            .chart_slot_targets(id, colour)
            .into_iter()
            .filter_map(|at| {
                let chart = doc.get(at).and_then(|n| n.chart())?;
                let mastik_core::BrushSource::Bound { var, .. } = fill_of(chart, colour)? else {
                    return None;
                };
                let fill = mastik_core::BrushSource::Bound { var, alpha };
                Some(Operation::SetChart {
                    id: at,
                    chart: Some(Box::new(with_fill(chart, colour, fill))),
                })
            })
            .collect();
        Transaction(ops)
    }

    /// The Chart card, for one chart frame.
    pub(super) fn inspector_chart(&mut self, ui: &mut egui::Ui, id: NodeId) {
        self.inspector_charts(ui, &[id]);
    }

    /// The charts of a multi-selection, when every selected layer is a chart and
    /// all are one kind; `None` otherwise.
    pub(super) fn selected_charts(&self) -> Option<Vec<NodeId>> {
        let ids = self.session.selection.ids();
        let doc = &self.session.doc;
        let kind = doc.get(*ids.first()?)?.chart()?.kind;
        ids.iter()
            .all(|id| {
                doc.get(*id)
                    .and_then(|n| n.chart())
                    .is_some_and(|c| c.kind == kind)
            })
            .then(|| ids.to_vec())
    }

    /// The Chart card over `ids`, the first the one it shows: every edit writes
    /// each ([`Self::chart_tx`]). Several charts leave out the Data and Series
    /// sections, which belong to one chart's table.
    pub(super) fn inspector_charts(&mut self, ui: &mut egui::Ui, ids: &[NodeId]) {
        let Some(&id) = ids.first() else {
            return;
        };
        let Some(chart) = self.session.chart_drawn(id).cloned() else {
            return;
        };
        self.chart_others = ids[1..].to_vec();
        self.refresh_chart_cache(ui.ctx(), id, &chart);
        let several = ids.len() > 1;
        // A chart style's slot: the look's sections fold into its capsule, and
        // what the style never holds (the type, the data, the series' own
        // colours, the references and *This chart*'s rows) stays live round it.
        let styled = super::styles::shown(&self.session.doc, ids, StyleSlot::Chart).any();
        self.style_card_parts(
            ui,
            "Chart",
            StyleSlot::Chart,
            ids,
            None,
            false,
            |app, ui| {
                ui.spacing_mut().item_spacing.y = ui::CARD_ROW_GAP;
                app.chart_type_rows(ui, id, &chart);
            },
            |app, ui| {
                if styled {
                    app.chart_look_sections(ui, id, &chart, true);
                } else {
                    app.chart_style_section(ui, id, &chart, false);
                    app.chart_data_sections(ui, id, &chart, several, false);
                    app.chart_look_sections(ui, id, &chart, false);
                    app.chart_references_section(ui, id, &chart, several);
                    app.chart_text_section(ui, id, &chart);
                }
            },
            |app, ui| {
                if styled {
                    app.chart_data_sections(ui, id, &chart, several, true);
                    app.chart_references_section(ui, id, &chart, several);
                }
                app.flatten_button(ui, ids);
            },
        );
        self.chart_others.clear();
    }

    /// Whether the card's charts take their look from a chart style it is not
    /// editing: the look's rows outside the capsule (the palette) are shown
    /// disabled then.
    fn chart_look_locked(&self, id: NodeId) -> bool {
        let ids: Vec<NodeId> = std::iter::once(id)
            .chain(self.chart_others.iter().copied())
            .collect();
        match super::styles::shown(&self.session.doc, &ids, StyleSlot::Chart) {
            super::styles::StyleShown::Plain => false,
            super::styles::StyleShown::One(s) => self.session.style_edit != Some(s),
            _ => true,
        }
    }

    /// The *Style* section: the presets, the theme, the plot and the spacing,
    /// and the chart's background unless `styled`, where it moves to *This
    /// chart* (a background is the frame's fill, never the chart style's).
    fn chart_style_section(&mut self, ui: &mut egui::Ui, id: NodeId, chart: &Chart, styled: bool) {
        self.chart_section(ui, "Chart style", "Style", |app, ui| {
            app.chart_style_rows(ui, id, chart, !styled);
        });
    }

    /// The sections a chart style holds whole, after *Style*: Marks, Axes,
    /// Values and Legend — and, styled, *Style* and *Text* with them, all of
    /// them under the capsule.
    fn chart_look_sections(&mut self, ui: &mut egui::Ui, id: NodeId, chart: &Chart, styled: bool) {
        if styled {
            self.chart_style_section(ui, id, chart, true);
        }
        self.chart_section(ui, "Chart marks", "Marks", |app, ui| {
            app.chart_mark_rows(ui, id, chart);
        });
        if chart.kind.has_axes() {
            self.chart_section(ui, "Chart axes", "Axes", |app, ui| {
                app.chart_axis_rows(ui, id, chart, !styled);
            });
        }
        if chart.kind != ChartKind::Sankey {
            self.chart_section(ui, "Chart values", "Values", |app, ui| {
                // A scatter draws no value labels: its rows stay, dimmed, under
                // the reason.
                let none = match chart.kind {
                    ChartKind::Scatter => Some("A scatter draws no value labels"),
                    ChartKind::Radar => Some("A radar draws no value labels"),
                    _ => None,
                };
                if let Some(why) = none {
                    ui::selection_note(ui, why);
                }
                ui::disable_unless(ui, none.is_none(), |ui| app.chart_value_rows(ui, id, chart));
            });
            self.chart_section(ui, "Chart legend", "Legend", |app, ui| {
                app.chart_legend_rows(ui, id, chart);
            });
        }
        if styled {
            self.chart_text_section(ui, id, chart);
        }
    }

    fn chart_text_section(&mut self, ui: &mut egui::Ui, id: NodeId, chart: &Chart) {
        self.chart_section(ui, "Chart text", "Text", |app, ui| {
            app.chart_text_rows(ui, id, chart);
        });
    }

    /// *Data*, and *Series* (or a waterfall's *Steps*) for one chart. `styled`
    /// adds *This chart* to Data: the rows a chart style never holds that
    /// otherwise sit among the look's (`Self::chart_own_rows`).
    fn chart_data_sections(
        &mut self,
        ui: &mut egui::Ui,
        id: NodeId,
        chart: &Chart,
        several: bool,
        styled: bool,
    ) {
        self.chart_section(ui, "Chart data", "Data", |app, ui| {
            if several {
                ui::selection_note(ui, "Edit data one chart at a time");
            } else {
                app.chart_data_rows(ui, id, chart);
            }
            if styled {
                app.chart_own_rows(ui, id, chart);
            }
        });
        if !several {
            // A chart coloured by name lists its named marks here.
            let series = named_mark_noun(chart).map_or("Series", |(plural, _)| plural);
            if chart.kind == ChartKind::Waterfall {
                // A waterfall's colours belong to the step, not the series.
                self.chart_section(ui, "Chart steps", "Steps", |app, ui| {
                    app.chart_step_rows(ui, id, chart);
                });
            } else {
                self.chart_section(ui, "Chart series", series, |app, ui| {
                    app.chart_series_rows(ui, id, chart);
                });
            }
        }
    }

    /// *This chart*: a styled chart's background, bar shape and order, slice
    /// order and grouping, axis titles and typed ranges, live under Data since
    /// the style holds none of them.
    fn chart_own_rows(&mut self, ui: &mut egui::Ui, id: NodeId, chart: &Chart) {
        ui.label(ui::eyebrow("This chart"));
        self.chart_background_row(ui, id);
        match chart.kind {
            ChartKind::Column | ChartKind::Bar => {
                self.chart_bar_shape_row(ui, id, chart);
                self.chart_bar_order_rows(ui, id, chart);
            }
            ChartKind::Pie => self.chart_slice_own_rows(ui, id, chart),
            _ => {}
        }
        if !chart.kind.has_axes() {
            return;
        }
        let (first, second) = axis_titles(chart.kind);
        for (value, title) in [(true, first), (false, second)] {
            let label = if value { "Value axis" } else { "Category axis" };
            ui.label(ui::eyebrow(title));
            if value || chart.kind == ChartKind::Scatter {
                self.chart_axis_range(ui, id, chart, value);
            }
            self.chart_axis_title(ui, id, chart, value, label);
        }
    }

    /// *Bars* or *Lollipops*, lollipops disabled on a stacked chart.
    fn chart_bar_shape_row(&mut self, ui: &mut egui::Ui, id: NodeId, chart: &Chart) {
        let b = &chart.options.bars;
        let lollipops = b.shape == chart::BarShape::Lollipops;
        // One reason on both controls: lollipops don't stack.
        let stacked = b.stacking != chart::Stacking::Apart;
        let pick = ui::segmented_tipped(
            ui,
            ui.available_width(),
            ui::SEGMENT_CELL_H,
            2,
            usize::from(lollipops),
            |i| i == 0 || !stacked,
            |i| {
                if i == 1 && stacked {
                    "Lollipops don't stack"
                } else {
                    "Bar shape"
                }
            },
            |p, i, r, on| match i {
                0 => ui::segment_label(p, r, "Bars", on),
                _ if stacked => ui::segment_label_disabled(p, r, "Lollipops"),
                _ => ui::segment_label(p, r, "Lollipops", on),
            },
        );
        if let Some(i) = pick {
            self.chart_each(id, chart, "Bar shape", |c| {
                c.options.bars.shape = if i == 1 {
                    chart::BarShape::Lollipops
                } else {
                    chart::BarShape::Bars
                };
            });
        }
    }

    /// *Bar order*, and what a sorted chart sorts by.
    fn chart_bar_order_rows(&mut self, ui: &mut egui::Ui, id: NodeId, chart: &Chart) {
        self.chart_menu_of(
            ui,
            id,
            chart,
            "chart-bar-order",
            "Bar order",
            ORDERS,
            chart.options.bars.order,
            |c, v| c.options.bars.order = v,
        );
        if chart.options.bars.order == chart::Order::Table {
            return;
        }
        let by = if chart.options.bars.stacking == chart::Stacking::Apart {
            let first = chart
                .mapping
                .series
                .iter()
                .find(|c| !chart.series.iter().any(|s| s.column == **c && !s.visible));
            let name = first
                .and_then(|c| chart.table.column(*c))
                .map_or("the first series", |c| c.name.as_str());
            format!("By {name}, the first series shown")
        } else {
            "By each stack's total".to_string()
        };
        ui::selection_note(ui, &by);
    }

    /// *Slice order* and *Group small slices* with its threshold.
    fn chart_slice_own_rows(&mut self, ui: &mut egui::Ui, id: NodeId, chart: &Chart) {
        self.chart_menu_of(
            ui,
            id,
            chart,
            "chart-slice-order",
            "Slice order",
            ORDERS,
            chart.options.slices.order,
            |c, v| c.options.slices.order = v,
        );
        let group = chart.options.slices.group_below.is_some();
        self.chart_switch(ui, id, chart, "Group small slices", group, |c, on| {
            c.options.slices.group_below = on.then_some(3.0);
        });
        if group {
            self.chart_number(
                ui,
                id,
                chart,
                ui.available_width(),
                "Below",
                "Group small slices",
                Scrub::fine(0.1, 1).range(0.0..=100.0),
                1,
                |c| c.options.slices.group_below.unwrap_or(3.0),
                |c, v| c.options.slices.group_below = Some(v),
            );
        }
    }

    /// The Chart card over charts of different kinds, which it cannot edit
    /// together: why, and *Flatten chart*, which applies to any.
    pub(super) fn inspector_charts_mixed(&mut self, ui: &mut egui::Ui, ids: &[NodeId]) {
        self.panel(ui, "Chart", None, |app, ui| {
            ui.spacing_mut().item_spacing.y = ui::CARD_ROW_GAP;
            ui::selection_note(ui, "Select charts of one kind to edit them together");
            app.flatten_button(ui, ids);
        });
    }

    /// *Flatten chart* over `ids`, the card's last row.
    fn flatten_button(&mut self, ui: &mut egui::Ui, ids: &[NodeId]) {
        let size = egui::vec2(ui.available_width(), ui::CONTROL_H);
        let tip = crate::input::keys("Keep the shapes and drop the chart's options (Ctrl+E)");
        if ui::action_button(
            ui,
            icon::STACK_SIMPLE,
            "Flatten chart",
            ui::FieldButton::Off,
            size,
        )
        .on_hover_text(tip)
        .clicked()
        {
            self.flatten_charts(ids);
        }
    }

    /// The extent chart `id` draws its value axis (`value`) or numeric X axis
    /// at now, as the generator ticked it at its drawn size; [`auto_extent`]'s
    /// estimate where it draws none.
    fn drawn_extent(&self, id: NodeId, chart: &Chart, value: bool) -> (f64, f64) {
        let size = match self.session.resolved.used_kind(&self.session.doc, id) {
            Some(mastik_core::NodeKind::Artboard { size, .. }) => Some(*size),
            _ => None,
        };
        let family = mastik_core::typography::TextStyle::default().font_family;
        size.and_then(|s| chart::generate(chart, s, &family).ok())
            .and_then(|g| g.extents[usize::from(!value)])
            .unwrap_or_else(|| auto_extent(chart, value))
    }

    /// Rebuild [`ChartCardCache`] when the description or the drawn size moved.
    fn refresh_chart_cache(&mut self, ctx: &egui::Context, id: NodeId, chart: &Chart) {
        let size = match self.session.resolved.used_kind(&self.session.doc, id) {
            Some(mastik_core::NodeKind::Artboard { size, .. }) => *size,
            _ => return,
        };
        // Drawn in the colours its bindings read now, so a mode switch redraws it.
        let mut drawn = chart.clone();
        for b in drawn.brushes_mut().filter(|b| b.is_bound()) {
            *b = self.session.doc.resolve_brush(id, b).into_owned().into();
        }
        let chart = &drawn;
        let key = description_key(id, chart, size, ctx.pixels_per_point());
        if self
            .chart_card
            .as_ref()
            .is_some_and(|c| c.id == id && c.key == key)
        {
            return;
        }
        let family = mastik_core::typography::TextStyle::default().font_family;
        let (left_out, refusal, track) = match chart::generate(chart, size, &family) {
            Ok(g) => {
                let track =
                    chart.kind == ChartKind::Pie && g.parts.iter().any(|p| p.name == "Track");
                (g.left_out, None, track)
            }
            Err(chart::Refusal::TooManyMarks { count, what }) => (
                Vec::new(),
                Some(format!(
                    "Too many marks: {} {what}, at most {}",
                    chart::grouped(count),
                    chart::grouped(chart::MAX_MARKS)
                )),
                false,
            ),
            Err(chart::Refusal::LoopsBack(path)) => {
                (Vec::new(), Some(format!("Flows loop back: {path}")), false)
            }
        };
        self.chart_card = Some(ChartCardCache {
            id,
            key,
            thumbs: Vec::new(),
            thumb_px: [0, 0],
            picture: super::chart_picture::Picture::of(chart),
            colours: super::chart_picture::colours(chart),
            left_out,
            refusal,
            track,
        });
    }

    /// One of the card's sections: a fold head ruled off from the rows above and,
    /// open, its rows. `key` is the fold's memory in `collapsed_panels`.
    fn chart_section(
        &mut self,
        ui: &mut egui::Ui,
        key: &'static str,
        label: &str,
        body: impl FnOnce(&mut Self, &mut egui::Ui),
    ) {
        let open = !self.collapsed_panels.contains(key);
        if ui::fold_rule(ui, label, open, 0).clicked() {
            if open {
                self.collapsed_panels.insert(key);
            } else {
                self.collapsed_panels.remove(key);
            }
        }
        if open {
            body(self, ui);
        }
    }

    /// Write `next` over chart `id`'s description as one undo step named `label`,
    /// unless it is what the chart already holds.
    fn chart_commit(&mut self, id: NodeId, label: &'static str, next: Chart) {
        if self.session.doc.get(id).and_then(|n| n.chart()) == Some(&next) {
            return;
        }
        self.session.name_step(label, icon::CHART);
        let tx = self.chart_tx(id, next);
        self.commit_edit(tx);
    }

    /// `next` written over chart `id`, and the change carried to every other
    /// chart the card writes ([`MastikApp::chart_others`]): the kind, and each
    /// option that changed, each chart keeping its own data and the rest.
    fn chart_tx(&self, id: NodeId, next: Chart) -> Transaction {
        let doc = &self.session.doc;
        let mut ops = Vec::new();
        if let Some(before) = doc.get(id).and_then(|n| n.chart()) {
            for other in &self.chart_others {
                let Some(own) = doc.get(*other).and_then(|n| n.chart()) else {
                    continue;
                };
                let mut carried = own.clone();
                if next.kind != before.kind {
                    carried = own.with_kind(next.kind);
                }
                carried.options =
                    chart::Options::carried(&before.options, &next.options, &own.options);
                if carried != *own {
                    ops.push(Operation::SetChart {
                        id: *other,
                        chart: Some(Box::new(carried)),
                    });
                }
            }
        }
        ops.insert(
            0,
            Operation::SetChart {
                id,
                chart: Some(Box::new(next)),
            },
        );
        Transaction(ops)
    }

    /// `set` applied to chart `id` (`chart` as the card shows it) and to each
    /// other chart the card writes ([`MastikApp::chart_others`]), each on its own
    /// description: a value read off the data (*Auto range*'s extent), a preset
    /// and one end of a range land as each chart's own, and a value the first
    /// chart already holds still reaches the others. The first chart's write
    /// leads, whether or not it changed; the others' only where they change.
    fn chart_each_tx(&self, id: NodeId, chart: &Chart, set: impl Fn(&mut Chart)) -> Transaction {
        let doc = &self.session.doc;
        let mut next = chart.clone();
        set(&mut next);
        let mut ops = vec![Operation::SetChart {
            id,
            chart: Some(Box::new(next)),
        }];
        for other in &self.chart_others {
            let Some(own) = doc.get(*other).and_then(|n| n.chart()) else {
                continue;
            };
            let mut next = own.clone();
            set(&mut next);
            if next != *own {
                ops.push(Operation::SetChart {
                    id: *other,
                    chart: Some(Box::new(next)),
                });
            }
        }
        Transaction(ops)
    }

    /// [`Self::chart_each_tx`] committed as one undo step named `label`, unless
    /// it changes no chart.
    fn chart_each(
        &mut self,
        id: NodeId,
        chart: &Chart,
        label: &'static str,
        set: impl Fn(&mut Chart),
    ) {
        let tx = self.chart_each_tx(id, chart, set);
        let first_same = match tx.0.first() {
            Some(Operation::SetChart { chart: Some(c), .. }) => {
                self.session.doc.get(id).and_then(|n| n.chart()) == Some(&**c)
            }
            _ => true,
        };
        if first_same && tx.0.len() == 1 {
            return;
        }
        self.session.name_step(label, icon::CHART);
        self.commit_edit(tx);
    }

    /// A number field over one option: scrubbed, it previews; released or
    /// typed, it commits as `label`, each chart the card writes taking the
    /// number ([`Self::chart_each_tx`]).
    #[allow(clippy::too_many_arguments)]
    fn chart_number(
        &mut self,
        ui: &mut egui::Ui,
        id: NodeId,
        chart: &Chart,
        width: f32,
        prefix: &'static str,
        label: &'static str,
        scrub: Scrub,
        decimals: usize,
        get: impl Fn(&Chart) -> f64,
        set: impl Fn(&mut Chart, f64),
    ) -> egui::Response {
        let mut v = get(chart);
        let resp = ui::value_field(
            ui,
            egui::vec2(width, ui::CONTROL_H),
            Prefix::Label(prefix),
            &mut v,
            scrub,
            |d| d.custom_formatter(ui::number(decimals)),
        )
        .on_hover_text(label);
        let tx = self.chart_each_tx(id, chart, |c| set(c, v));
        self.named_valve(label, icon::CHART, &resp, tx);
        resp
    }

    /// [`Self::chart_number`] shown disabled with `why` unless `live`: an option
    /// this chart does not draw.
    #[allow(clippy::too_many_arguments)]
    fn chart_number_if(
        &mut self,
        ui: &mut egui::Ui,
        live: Result<(), &'static str>,
        id: NodeId,
        chart: &Chart,
        width: f32,
        prefix: &'static str,
        label: &'static str,
        scrub: Scrub,
        decimals: usize,
        get: impl Fn(&Chart) -> f64,
        set: impl Fn(&mut Chart, f64),
    ) {
        let resp = ui::disable_unless(ui, live.is_ok(), |ui| {
            self.chart_number(
                ui, id, chart, width, prefix, label, scrub, decimals, get, set,
            )
        })
        .inner;
        if let Err(why) = live {
            resp.on_disabled_hover_text(why);
        }
    }

    /// Two [`Self::chart_number`]s on one line.
    fn chart_pair(&mut self, ui: &mut egui::Ui, add: impl FnOnce(&mut Self, &mut egui::Ui, f32)) {
        let half = (ui.available_width() - ui::CARD_COL_GAP) / 2.0;
        ui.horizontal(|ui| {
            ui.spacing_mut().item_spacing.x = ui::CARD_COL_GAP;
            add(self, ui, half);
        });
    }

    /// A choice of a few, as segments: picking one commits `set` as `label`.
    #[allow(clippy::too_many_arguments)]
    fn chart_segments(
        &mut self,
        ui: &mut egui::Ui,
        id: NodeId,
        chart: &Chart,
        label: &'static str,
        names: &[&str],
        at: usize,
        set: impl Fn(&mut Chart, usize),
    ) {
        let pick = ui::segmented_tipped(
            ui,
            ui.available_width(),
            ui::SEGMENT_CELL_H,
            names.len(),
            at,
            |_| true,
            |_| label,
            |p, i, r, on| ui::segment_label(p, r, names[i], on),
        );
        if let Some(i) = pick {
            self.chart_pick(id, chart, label, i, set);
        }
    }

    /// [`Self::chart_segments`] over `options`, each value with its name, the
    /// one `now` lit; a value missing from the table lights none, so any pick
    /// writes.
    #[allow(clippy::too_many_arguments)]
    fn chart_segments_of<T: Copy + PartialEq>(
        &mut self,
        ui: &mut egui::Ui,
        id: NodeId,
        chart: &Chart,
        label: &'static str,
        options: &[(T, &str)],
        now: T,
        set: impl Fn(&mut Chart, T),
    ) {
        let names: Vec<&str> = options.iter().map(|(_, n)| *n).collect();
        let at = lit(options, now);
        self.chart_segments(ui, id, chart, label, &names, at, |c, i| {
            set(c, options[i].0);
        });
    }

    /// [`Self::chart_menu`] over `options`, as [`Self::chart_segments_of`].
    #[allow(clippy::too_many_arguments)]
    fn chart_menu_of<T: Copy + PartialEq>(
        &mut self,
        ui: &mut egui::Ui,
        id: NodeId,
        chart: &Chart,
        salt: &'static str,
        label: &'static str,
        options: &[(T, &str)],
        now: T,
        set: impl Fn(&mut Chart, T),
    ) {
        let names: Vec<&str> = options.iter().map(|(_, n)| *n).collect();
        let at = lit(options, now);
        self.chart_menu(ui, id, chart, salt, label, &names, at, |c, i| {
            set(c, options[i].0);
        });
    }

    /// A switch over one option, committing `set` as `label` on each chart.
    fn chart_switch(
        &mut self,
        ui: &mut egui::Ui,
        id: NodeId,
        chart: &Chart,
        label: &'static str,
        on: bool,
        set: impl Fn(&mut Chart, bool),
    ) {
        if ui::switch_row(ui, label, on, crate::settings::SWITCH_H).clicked() {
            self.chart_each(id, chart, label, |c| set(c, !on));
        }
    }

    /// [`Self::chart_switch`] shown disabled with `why` unless `live`.
    #[allow(clippy::too_many_arguments)]
    fn chart_switch_if(
        &mut self,
        ui: &mut egui::Ui,
        live: Result<(), &'static str>,
        id: NodeId,
        chart: &Chart,
        label: &'static str,
        on: bool,
        set: impl Fn(&mut Chart, bool),
    ) {
        let resp = ui::disable_unless(ui, live.is_ok(), |ui| {
            ui::switch_row(ui, label, on, crate::settings::SWITCH_H)
        })
        .inner;
        match live {
            Err(why) => {
                resp.on_disabled_hover_text(why);
            }
            Ok(()) if resp.clicked() => self.chart_each(id, chart, label, |c| set(c, !on)),
            Ok(()) => {}
        }
    }

    /// A dropdown over one option: its rows `names`, `at` lit.
    #[allow(clippy::too_many_arguments)]
    fn chart_menu(
        &mut self,
        ui: &mut egui::Ui,
        id: NodeId,
        chart: &Chart,
        salt: &'static str,
        label: &'static str,
        names: &[&str],
        at: usize,
        set: impl Fn(&mut Chart, usize),
    ) {
        let mut pick = None;
        ui::labelled(ui, label, |ui| {
            ui::combo_height(ui);
            egui::ComboBox::from_id_salt(salt)
                .icon(ui::combo_chevron)
                .width(ui.available_width())
                .selected_text(ui::combo_text(names.get(at).copied().unwrap_or("")))
                .show_ui(ui, |ui| {
                    ui::menu_rows(ui);
                    for (i, name) in names.iter().enumerate() {
                        if ui.selectable_label(i == at, *name).clicked() {
                            pick = Some(i);
                        }
                    }
                });
        });
        if let Some(i) = pick {
            self.chart_pick(id, chart, label, i, set);
        }
    }

    /// Commit row `i` of a segmented control or dropdown on each chart. The lit
    /// row changes nothing on the first chart, so over several charts it
    /// writes the others, which the card does not show and which may differ.
    fn chart_pick(
        &mut self,
        id: NodeId,
        chart: &Chart,
        label: &'static str,
        i: usize,
        set: impl Fn(&mut Chart, usize),
    ) {
        self.chart_each(id, chart, label, |c| set(c, i));
    }

    fn chart_type_rows(&mut self, ui: &mut egui::Ui, id: NodeId, chart: &Chart) {
        let mut pick = None;
        ui.scope(|ui| {
            ui::combo_height(ui);
            egui::ComboBox::from_id_salt("chart-kind")
                .icon(ui::combo_chevron)
                .width(ui.available_width())
                .selected_text(ui::glyph_and_text(
                    crate::rail::chart_glyph(chart.kind),
                    chart.kind.label(),
                ))
                .show_ui(ui, |ui| {
                    ui::menu_rows(ui);
                    for kind in ChartKind::ALL {
                        let row = ui::glyph_and_text(crate::rail::chart_glyph(kind), kind.label());
                        if ui.selectable_label(kind == chart.kind, row).clicked() {
                            pick = Some(kind);
                        }
                    }
                });
        });
        if let Some(kind) = pick.filter(|k| *k != chart.kind) {
            self.chart_commit(id, "Chart type", chart.with_kind(kind));
        }
        let lollipops = matches!(chart.kind, ChartKind::Column | ChartKind::Bar)
            && chart.options.bars.shape == chart::BarShape::Lollipops;
        if let Some((names, at)) = variants(chart) {
            // A variant that is a look (a line's curve, a donut, a filled radar)
            // belongs to a chart style: held while one styles the chart, as the
            // palette is. A stacking or a direction is the chart's own.
            let locked = self.chart_look_locked(id);
            let styled =
                |i: usize| locked && with_variant(chart, i).options.look() != chart.options.look();
            let off = |i: usize| (lollipops && i > 0) || styled(i);
            // Lollipops don't stack: only Grouped is offered while they show.
            let pick = ui::segmented_tipped(
                ui,
                ui.available_width(),
                ui::SEGMENT_CELL_H,
                names.len(),
                at,
                |i| !off(i),
                |i| {
                    if styled(i) {
                        "From the style · Detach to change"
                    } else if lollipops && i > 0 {
                        "Lollipops don't stack"
                    } else {
                        "Chart type"
                    }
                },
                |p, i, r, on| {
                    if off(i) && !on {
                        ui::segment_label_disabled(p, r, names[i]);
                    } else {
                        ui::segment_label(p, r, names[i], on);
                    }
                },
            );
            if let Some(i) = pick {
                self.chart_pick(id, chart, "Chart type", i, |c, i| *c = with_variant(c, i));
            }
        }
    }

    /// The *Style* section's rows; `background` leaves out the background row
    /// when false, as on a styled chart, which shows it under *This chart*.
    fn chart_style_rows(&mut self, ui: &mut egui::Ui, id: NodeId, chart: &Chart, background: bool) {
        let lit = Preset::of(chart.kind, &chart.options);
        let n = Preset::ALL.len() as f32;
        let cell = (ui.available_width() - STYLE_GAP * (n - 1.0)) / n;
        let ctx = ui.ctx().clone();
        let px = style_picture_px(cell, ctx.pixels_per_point());
        if let Some(c) = self.chart_card.as_mut().filter(|c| c.id == id)
            && c.thumb_px != px
        {
            c.thumbs = Preset::ALL
                .into_iter()
                .filter_map(|p| style_picture(&ctx, c.picture, &c.colours, p, px).map(|t| (p, t)))
                .collect();
            c.thumb_px = px;
        }
        let thumbs: Vec<(Preset, egui::TextureId)> = self
            .chart_card
            .as_ref()
            .filter(|c| c.id == id)
            .map(|c| c.thumbs.iter().map(|(p, t)| (*p, t.id())).collect())
            .unwrap_or_default();
        let mut pick = None;
        ui.horizontal(|ui| {
            ui.spacing_mut().item_spacing.x = STYLE_GAP;
            for preset in Preset::ALL {
                let picture = thumbs.iter().find(|(p, _)| *p == preset).map(|(_, t)| *t);
                if style_cell(ui, cell, preset.label(), picture, lit == Some(preset)).clicked() {
                    pick = Some(preset);
                }
            }
        });
        // Each chart takes the preset over its own options, so one already on
        // part of it still lands on all of it.
        if let Some(p) = pick {
            self.chart_each(id, chart, "Chart style", |c| {
                c.options = p.apply(c.kind, &c.options);
            });
        }
        if background {
            self.chart_background_row(ui, id);
        }
        // The theme the unset colours are derived for; a colour of the
        // person's own stays whatever it is.
        self.chart_segments_of(
            ui,
            id,
            chart,
            "Chart theme",
            &[
                (chart::Theme::Auto, "Auto"),
                (chart::Theme::Light, "Light"),
                (chart::Theme::Dark, "Dark"),
            ],
            chart.options.theme,
            |c, t| c.options.theme = t,
        );
        if chart.options.theme == chart::Theme::Auto {
            let doc = &self.session.doc;
            let ground = chart::ground_of(doc, id);
            let picked = if chart::dark_on(ground.colour) {
                "Dark"
            } else {
                "Light"
            };
            let from = match ground.from {
                chart::GroundFrom::Own => "the background".to_string(),
                chart::GroundFrom::Ancestor(a) => doc
                    .get(a)
                    .map_or_else(|| "the frame".to_string(), |n| n.name().to_string()),
                chart::GroundFrom::Page => "the page".to_string(),
            };
            ui::selection_note(ui, &format!("{picked}, read from {from}"));
        }
        self.chart_plot_rows(ui, id, chart);
        // Padding round the chart and the gap between its legend and body.
        self.chart_pair(ui, |app, ui, w| {
            app.chart_number(
                ui,
                id,
                chart,
                w,
                "Padding",
                "Chart padding",
                Scrub::whole(0.25).range(0.0..=400.0),
                0,
                |c| c.options.padding,
                |c, v| c.options.padding = v.max(0.0),
            );
            app.chart_number(
                ui,
                id,
                chart,
                w,
                "Gap",
                "Chart gap",
                Scrub::whole(0.25).range(0.0..=400.0),
                0,
                |c| c.options.gap,
                |c, v| c.options.gap = v.max(0.0),
            );
        });
    }

    /// *Background*: the chart frame's own fill, edited where it is one plain
    /// colour (a single visible solid or bound fill, no fill style); otherwise
    /// disabled, naming why. With no visible fill it reads *None*, and a click
    /// adds a white one to edit, keeping any hidden fills.
    fn chart_background_row(&mut self, ui: &mut egui::Ui, id: NodeId) {
        let doc = &self.session.doc;
        let Some(node) = doc.get(id) else { return };
        let fills: Vec<(usize, &mastik_core::BrushSource)> = node
            .paint()
            .fills
            .iter()
            .enumerate()
            .filter(|(_, f)| f.visible)
            .map(|(i, f)| (i, &f.brush))
            .collect();
        let style = node
            .styles()
            .fills
            .and_then(|s| doc.styles().get(s))
            .map(|s| s.name.clone());
        let why: Option<String> = match (&style, fills.as_slice()) {
            (Some(name), _) => Some(format!("The background is the style {name}")),
            (None, [_, _, ..]) => Some("The background has several fills: edit it in Fill".into()),
            (None, [(_, b)]) => match &*doc.resolve_brush(id, b) {
                Brush::Solid(_) => None,
                Brush::Gradient(_) => Some("The background is a gradient: edit it in Fill".into()),
                _ => Some("The background is an image: edit it in Fill".into()),
            },
            (None, []) => None,
        };
        let shown = fills
            .first()
            .map(|(_, b)| doc.resolve_brush(id, b).into_owned());
        let (swatch, hex) = match &shown {
            Some(Brush::Solid(c)) => (ui::Swatch::Solid(ui::color_to_egui(*c)), ui::hex_of(*c)),
            Some(_) => (ui::Swatch::Mixed, String::new()),
            None => (
                ui::Swatch::Solid(egui::Color32::TRANSPARENT),
                "None".to_string(),
            ),
        };
        let slot = fills.first().map(|(i, _)| PaintSlot::Fill(*i));
        let open = self
            .picker
            .as_ref()
            .is_some_and(|p| Some(p.slot) == slot && self.session.selection.ids() == [id]);
        let size = egui::vec2(ui.available_width(), ui::CONTROL_H);
        let live = why.is_none();
        let ink = if live {
            color::text()
        } else {
            theme::text::off()
        };
        let (_, resp) = ui::field_row_pressable(ui, size, open, live, |ui, ground| {
            colour_row_face(ui, ground, swatch, "Background", ink, |ui| {
                detail_text(ui, &hex);
            });
        });
        if let Some(why) = why {
            resp.on_hover_text(why);
            return;
        }
        if !resp.clicked() {
            return;
        }
        let at = resp.rect.left_top();
        match slot {
            Some(slot) => self.open_picker(ui, id, slot, at),
            None => {
                let fill = mastik_core::Fill {
                    brush: Brush::Solid(mastik_core::peniko::Color::WHITE).into(),
                    visible: true,
                };
                // Added over any hidden fills, which stay.
                let mut fills = self
                    .session
                    .doc
                    .get(id)
                    .map(|n| n.paint().fills.clone())
                    .unwrap_or_default();
                fills.push(mastik_core::item::Keyed::new(
                    self.session.ids.mint_item(),
                    fill,
                ));
                let at_index = fills.len() - 1;
                self.session.name_step("Chart background", icon::CHART);
                self.commit_edit(Transaction(vec![Operation::SetFills { id, fills }]));
                self.open_picker(ui, id, PaintSlot::Fill(at_index), at);
            }
        }
    }

    /// *Plot background*: a colour row that starts at *None* and a click gives
    /// a light grey to edit, a way back to none, and the radius beside it.
    fn chart_plot_rows(&mut self, ui: &mut egui::Ui, id: NodeId, chart: &Chart) {
        let rect_plot = chart.kind.rect_plot();
        let slot = ChartColour::Plot;
        let set = chart.options.plot.fill.is_some();
        let swatch = colour_of(&self.session.doc, id, chart, slot)
            .map_or(ui::Swatch::Solid(egui::Color32::TRANSPARENT), |c| {
                ui::Swatch::Solid(ui::color_to_egui(c))
            });
        let full = ui.available_width();
        let width = if set {
            full - ui::CONTROL_H - ui::CARD_COL_GAP
        } else {
            full
        };
        let why = "Only a chart with a rectangular plot has a plot background";
        let mut clear = false;
        let disabled = ui::disable_unless(ui, rect_plot, |ui| {
            ui.horizontal(|ui| {
                ui.spacing_mut().item_spacing.x = ui::CARD_COL_GAP;
                if set {
                    self.chart_colour_row(
                        ui,
                        id,
                        slot,
                        width,
                        swatch,
                        "Plot background",
                        true,
                        None,
                    );
                    clear = ui::field_button(ui, icon::X, ui::CONTROL_H, ui::FieldButton::Off)
                        .on_hover_text("No plot background")
                        .clicked();
                    None
                } else {
                    let (_, resp) = ui::field_row_pressable(
                        ui,
                        egui::vec2(width, ui::CONTROL_H),
                        false,
                        true,
                        |ui, ground| {
                            colour_row_face(
                                ui,
                                ground,
                                swatch,
                                "Plot background",
                                color::text(),
                                |ui| {
                                    detail_text(ui, "None");
                                },
                            );
                        },
                    );
                    Some(resp)
                }
            })
            .inner
        });
        if !rect_plot {
            disabled.response.on_disabled_hover_text(why);
        } else if let Some(resp) = disabled.inner
            && resp.clicked()
        {
            let grey = Brush::Solid(chart::PLOT_GREY);
            self.chart_each(id, chart, "Plot background", |c| {
                c.options.plot.fill = Some(grey.clone().into());
            });
            self.open_picker(ui, id, PaintSlot::Chart(slot), resp.rect.left_top());
        }
        if clear {
            self.chart_each(id, chart, "Plot background", |c| c.options.plot.fill = None);
        }
        let radius_live = if !rect_plot {
            Err(why)
        } else if !set {
            Err("No plot background to round")
        } else {
            Ok(())
        };
        self.chart_number_if(
            ui,
            radius_live,
            id,
            chart,
            ui.available_width(),
            "Radius",
            "Plot radius",
            Scrub::whole(0.25).range(0.0..=100.0),
            0,
            |c| c.options.plot.radius,
            |c, v| c.options.plot.radius = v.max(0.0),
        );
    }

    /// The table the chart reads: its own, or one of the document's data sets
    /// (`lists.md`), with *From a file…* reading a new one. The List card's
    /// Data dropdown, with *Own table* first.
    fn chart_table_dropdown(&mut self, ui: &mut egui::Ui, id: NodeId, chart: &Chart) {
        use crate::list_data::{TablePick, TableRows, table_combo};
        let doc = &self.session.doc;
        let shown = match chart.data {
            None => "Own table".to_string(),
            Some(d) => doc
                .data_set(d)
                .map_or_else(|| "Missing data".to_string(), |s| s.name.clone()),
        };
        let w = ui.available_width();
        let rows = TableRows {
            own: Some(chart.data.is_none()),
            new: false,
            save: self.chart_set(id).is_none(),
            no_file: false,
        };
        match table_combo(ui, ("chart-table", id), w, doc, &shown, chart.data, &rows) {
            Some(TablePick::Own) => self.chart_own_table(id),
            Some(TablePick::Set(d)) => self.move_chart_to_set(id, d, None),
            Some(TablePick::Save) => {
                self.save_chart_table(id);
            }
            Some(TablePick::File) => self.chart_data_dialog(id),
            Some(TablePick::New) | None => {}
        }
        if chart.data.is_some() && self.chart_set(id).is_none() {
            ui::selection_note(ui, crate::panels::list::MISSING_TABLE);
        }
    }

    fn chart_data_rows(&mut self, ui: &mut egui::Ui, id: NodeId, chart: &Chart) {
        self.chart_table_dropdown(ui, id, chart);
        let rows = chart.table.rows.len();
        let summary = match chart.kind {
            ChartKind::Sankey => ui::count_as(rows, "flow", "flows"),
            _ => format!(
                "{} · {}",
                ui::count_as(chart.mapping.series.len(), "series", "series"),
                ui::count_as(rows, "row", "rows")
            ),
        };
        ui.label(
            egui::RichText::new(summary)
                .size(ui::SMALL_PT)
                .color(theme::text::muted()),
        );
        let size = egui::vec2(ui.available_width(), ui::CONTROL_H);
        if ui::action_button(ui, icon::TABLE, "Edit data…", ui::FieldButton::Off, size)
            .on_hover_text("Open the chart's table over the canvas")
            .clicked()
        {
            self.open_chart_data(id, (0, 0));
        }
        match self
            .chart_set(id)
            .and_then(|s| self.session.doc.data_set(s).cloned())
        {
            Some(set) => self.data_set_link_rows(ui, &set),
            None => self.chart_source_line(ui, id),
        }
        if let Some(cache) = self.chart_card.as_ref().filter(|c| c.id == id) {
            if let Some(why) = &cache.refusal {
                ui.label(
                    egui::RichText::new(why)
                        .size(ui::SMALL_PT)
                        .color(color::warn()),
                );
            } else if !cache.left_out.is_empty() {
                let n = cache.left_out.len();
                ui.label(
                    egui::RichText::new(format!("{} left out", ui::count_as(n, "value", "values")))
                        .size(ui::SMALL_PT)
                        .color(color::warn()),
                )
                .on_hover_text(cache.left_out.join("\n"));
            }
        }
        let names: Vec<&str> = chart
            .table
            .columns
            .iter()
            .map(|c| c.name.as_str())
            .collect();
        let ids: Vec<chart::ColumnId> = chart.table.columns.iter().map(|c| c.id).collect();
        let at = |c: Option<chart::ColumnId>| c.and_then(|c| ids.iter().position(|i| *i == c));
        let m = &chart.mapping;
        match chart.kind {
            ChartKind::Sankey => {
                for (salt, label, now, which) in [
                    ("chart-from", "From", m.from, 0),
                    ("chart-to", "To", m.to, 1),
                    ("chart-value", "Value", m.value, 2),
                ] {
                    let ids = ids.clone();
                    self.chart_menu(
                        ui,
                        id,
                        chart,
                        salt,
                        label,
                        &names,
                        at(now).unwrap_or(usize::MAX),
                        move |c, i| {
                            let col = Some(ids[i]);
                            match which {
                                0 => c.mapping.from = col,
                                1 => c.mapping.to = col,
                                _ => c.mapping.value = col,
                            }
                        },
                    );
                }
            }
            kind => {
                let label = if kind == ChartKind::Scatter {
                    "X"
                } else {
                    "Category"
                };
                let ids2 = ids.clone();
                self.chart_menu(
                    ui,
                    id,
                    chart,
                    "chart-category",
                    label,
                    &names,
                    at(m.category).unwrap_or(usize::MAX),
                    move |c, i| {
                        c.mapping.category = Some(ids2[i]);
                        c.mapping.series.retain(|s| *s != ids2[i]);
                    },
                );
            }
        }
    }

    fn chart_series_rows(&mut self, ui: &mut egui::Ui, id: NodeId, chart: &Chart) {
        let palettes = [
            (Palette::Mastik, "Mastik"),
            (Palette::Cool, "Cool"),
            (Palette::Warm, "Warm"),
            (Palette::Mono, "Mono"),
        ];
        let groups = colour_groups(&self.session.doc);
        // The stored group, else a file's series bound one by one.
        let group: Option<Vec<mastik_core::VarId>> = chart
            .options
            .palette_group
            .iter()
            .map(|b| b.var())
            .collect();
        let bound = group
            .filter(|g| !g.is_empty())
            .or_else(|| chart::bound_variables(chart));
        // A group lights while the series run through it in order; a palette
        // while nothing is bound.
        let lit_group = bound.as_ref().and_then(|b| {
            groups.iter().position(|(_, vars)| {
                b.iter()
                    .enumerate()
                    .all(|(i, v)| vars[i % vars.len()] == *v)
            })
        });
        let shown = match (lit_group, &bound) {
            (Some(g), _) => format!("From variables · {}", groups[g].0),
            (None, Some(_)) => "From variables".to_string(),
            (None, None) => palettes
                .iter()
                .find(|(p, _)| *p == chart.options.palette)
                .map_or("Mastik", |(_, n)| *n)
                .to_string(),
        };
        // A chart style holds the palette: shown disabled, saying where it is
        // from, while the series' own swatches below stay live.
        let locked = self.chart_look_locked(id);
        let shown = match locked {
            true => "From the style · Detach to change".to_string(),
            false => shown,
        };
        enum Pick {
            Palette(Palette),
            Group(usize),
        }
        let mut pick = None;
        ui::disable_unless(ui, !locked, |ui| {
            ui::labelled(ui, "Palette", |ui| {
                ui::combo_height(ui);
                egui::ComboBox::from_id_salt(("chart-palette", id))
                    .icon(ui::combo_chevron)
                    .width(ui.available_width())
                    .selected_text(ui::combo_text(&shown))
                    .show_ui(ui, |ui| {
                        ui::menu_rows(ui);
                        for (p, name) in palettes {
                            let on = bound.is_none() && p == chart.options.palette;
                            if ui.selectable_label(on, name).clicked() {
                                pick = Some(Pick::Palette(p));
                            }
                        }
                        ui::menu_sep(ui);
                        ui::menu_eyebrow(ui, "From variables");
                        if groups.is_empty() {
                            ui::menu_note(ui, "No colour variables yet");
                        }
                        for (g, (name, _)) in groups.iter().enumerate() {
                            if ui.selectable_label(lit_group == Some(g), name).clicked() {
                                pick = Some(Pick::Group(g));
                            }
                        }
                    });
            });
        });
        match pick {
            Some(Pick::Palette(p)) => {
                self.chart_commit(id, "Palette", chart::with_palette(chart, p));
            }
            Some(Pick::Group(g)) if lit_group != Some(g) => {
                let next = chart::with_variables(chart, &groups[g].1);
                self.chart_commit(id, "Palette", next);
            }
            _ => {}
        }
        let totals = chart::sankey_totals(chart);
        // A Sankey's nodes, a pie's slices and a grouped scatter's groups are
        // coloured by name (`chart::named_marks`), so those are the rows.
        let rows: Vec<(ChartColour, String, Option<bool>)> = match chart::named_marks(chart) {
            Some(marks) => marks
                .into_iter()
                .enumerate()
                .map(|(i, (name, _))| (ChartColour::Node(i), name, None))
                .collect(),
            None => chart
                .table
                .columns
                .iter()
                .filter(|c| Some(c.id) != chart.mapping.category)
                .map(|c| {
                    let mapped = chart.mapping.series.contains(&c.id);
                    let visible = chart
                        .series
                        .iter()
                        .find(|s| s.column == c.id)
                        .is_none_or(|s| s.visible);
                    (
                        ChartColour::Series(c.id),
                        c.name.clone(),
                        Some(mapped && visible),
                    )
                })
                .collect(),
        };
        let w = ui.available_width();
        let styled = matches!(
            chart.kind,
            ChartKind::Line | ChartKind::Area | ChartKind::Scatter | ChartKind::Radar
        );
        for (slot, name, shown) in rows {
            let colour = colour_of(&self.session.doc, id, chart, slot).map(ui::color_to_egui);
            let total = match slot {
                ChartColour::Node(i) => totals.get(i).map(|v| {
                    chart::scale::format(
                        *v,
                        &chart.options.values.format,
                        chart::scale::decimals_of(*v).min(3),
                    )
                }),
                _ => None,
            };
            let style_button = styled && matches!(slot, ChartColour::Series(_));
            ui.horizontal(|ui| {
                ui.spacing_mut().item_spacing.x = ui::CARD_COL_GAP;
                let buttons = usize::from(shown.is_some()) + usize::from(style_button);
                let field = w - buttons as f32 * (ui::CONTROL_H + ui::CARD_COL_GAP);
                // A hidden series keeps its colour, dimmed as its name is.
                let swatch = match (colour, shown) {
                    (None, _) => ui::Swatch::Mixed,
                    (Some(c), Some(false)) => ui::Swatch::Solid(c.gamma_multiply(0.4)),
                    (Some(c), _) => ui::Swatch::Solid(c),
                };
                self.chart_colour_row(
                    ui,
                    id,
                    slot,
                    field,
                    swatch,
                    &name,
                    shown != Some(false),
                    total
                        .as_deref()
                        .map(|t| (t, "The node's total: the larger of what flows in and out")),
                );
                if let (true, ChartColour::Series(col)) = (style_button, slot) {
                    self.chart_series_style(ui, id, chart, col);
                }
                if let (Some(on), ChartColour::Series(col)) = (shown, slot) {
                    let (glyph, tip) = if on {
                        (icon::EYE, "Hide this series")
                    } else {
                        (icon::EYE_SLASH, "Show this column as a series")
                    };
                    if ui::field_button(ui, glyph, ui::CONTROL_H, ui::FieldButton::Off)
                        .on_hover_text(tip)
                        .clicked()
                    {
                        let mut next = chart.clone();
                        show_series(&mut next, col, !on);
                        self.chart_commit(id, "Show series", next);
                    }
                }
            });
        }
        if matches!(chart.kind, ChartKind::Column | ChartKind::Bar) {
            let shown = chart
                .mapping
                .series
                .iter()
                .filter(|c| !chart.series.iter().any(|s| s.column == **c && !s.visible))
                .count();
            let live = if shown == 1 {
                Ok(())
            } else {
                Err("Colour by category needs one series")
            };
            let on = chart.options.by_category;
            self.chart_switch_if(ui, live, id, chart, "Colour by category", on, |c, on| {
                c.options.by_category = on;
            });
        }
        self.chart_highlight_rows(ui, id, chart);
    }

    /// *Highlight*: the categories or series drawn as they are, picked by name
    /// from a dropdown (each pick turns one on or off); the others *Muted* or
    /// *Faded*, the muted colour, and *Highlighted only* for the value labels.
    fn chart_highlight_rows(&mut self, ui: &mut egui::Ui, id: NodeId, chart: &Chart) {
        use chart::Highlighted;
        let t = &chart.table;
        let cat_col = chart.mapping.category.and_then(|c| t.index_of(c));
        let mut categories: Vec<String> = Vec::new();
        for r in &t.rows {
            if let Some(c) = cat_col {
                let name = r[c].trim();
                if !name.is_empty() && !categories.iter().any(|n| n == name) {
                    categories.push(name.to_string());
                }
            }
        }
        let mut series: Vec<String> = match chart.kind {
            ChartKind::Scatter if chart.mapping.group.is_some() => chart::named_marks(chart)
                .into_iter()
                .flatten()
                .map(|(n, _)| n)
                .collect(),
            _ => chart
                .mapping
                .series
                .iter()
                .filter_map(|c| t.column(*c).map(|c| c.name.clone()))
                .collect(),
        };
        // A one-series chart lists categories only; a line or scatter series only;
        // a pie its slices.
        match chart.kind {
            ChartKind::Line | ChartKind::Area | ChartKind::Scatter | ChartKind::Radar => {
                categories.clear();
            }
            ChartKind::Pie => series.clear(),
            _ if series.len() == 1 => series.clear(),
            _ => {}
        }
        if chart.kind == ChartKind::Sankey {
            return;
        }
        let h = &chart.highlight;
        let (lit_cats, lit_series) = match &h.target {
            Highlighted::None => (Vec::new(), Vec::new()),
            Highlighted::Categories(n) => (n.clone(), Vec::new()),
            Highlighted::Series(n) => (Vec::new(), n.clone()),
        };
        // A waterfall's final total is a step it can light.
        for step in chart::waterfall_steps(chart) {
            if !categories.contains(&step) {
                categories.push(step);
            }
        }
        // A lit name the data no longer has stays listed, so it can be turned off.
        for (names, lit) in [(&mut categories, &lit_cats), (&mut series, &lit_series)] {
            for n in lit {
                if !names.contains(n) {
                    names.push(n.clone());
                }
            }
        }
        let shown = match (&h.target, lit_cats.len() + lit_series.len()) {
            (Highlighted::None, _) | (_, 0) => "None".to_string(),
            (_, n) if n <= 2 => lit_cats
                .iter()
                .chain(&lit_series)
                .cloned()
                .collect::<Vec<_>>()
                .join(", "),
            (Highlighted::Categories(_), n) => format!("{n} categories"),
            (_, n) => format!("{n} series"),
        };
        // `Ok` a category, `Err` a series; `None` clears.
        let mut pick: Option<Option<Result<String, String>>> = None;
        ui::labelled(ui, "Highlight", |ui| {
            ui::combo_height(ui);
            egui::ComboBox::from_id_salt(("chart-highlight", id))
                .icon(ui::combo_chevron)
                .width(ui.available_width())
                .selected_text(ui::combo_text(&shown))
                .show_ui(ui, |ui| {
                    ui::menu_rows(ui);
                    if ui
                        .selectable_label(h.target == Highlighted::None, "None")
                        .clicked()
                    {
                        pick = Some(None);
                    }
                    if !categories.is_empty() {
                        ui.label(ui::eyebrow("Categories"));
                        for c in &categories {
                            if ui.selectable_label(lit_cats.contains(c), c).clicked() {
                                pick = Some(Some(Ok(c.clone())));
                            }
                        }
                    }
                    if !series.is_empty() {
                        ui.label(ui::eyebrow("Series"));
                        for s in &series {
                            if ui.selectable_label(lit_series.contains(s), s).clicked() {
                                pick = Some(Some(Err(s.clone())));
                            }
                        }
                    }
                });
        });
        if let Some(p) = pick {
            let next = toggled(&h.target, p);
            let label = if next == Highlighted::None {
                "Clear highlight"
            } else {
                "Highlight"
            };
            self.chart_each(id, chart, label, |c| c.highlight.target = next.clone());
        }
        if h.target == Highlighted::None {
            return;
        }
        self.chart_segments_of(
            ui,
            id,
            chart,
            "Highlight others",
            &[
                (chart::Others::Muted, "Muted"),
                (chart::Others::Faded, "Faded"),
            ],
            h.others,
            |c, v| c.highlight.others = v,
        );
        if h.others == chart::Others::Muted {
            self.chart_slot_row(ui, id, chart, ChartColour::Muted, "Muted colour");
        }
        // The highlight's own, so it stays live where a chart style holds Values.
        let only = h.values_only;
        self.chart_switch(ui, id, chart, "Highlighted only", only, |c, on| {
            c.highlight.values_only = on;
        });
    }

    /// *Flat* or *Fade*, and *Fade to*: areas and bars from their colour at the
    /// value end to a share of it at the baseline. A bound series draws flat,
    /// said on the card's left-out line.
    fn chart_fade_rows(&mut self, ui: &mut egui::Ui, id: NodeId, chart: &Chart) {
        let on = chart.options.fade.on;
        self.chart_segments_of(
            ui,
            id,
            chart,
            "Fade",
            &[(false, "Flat"), (true, "Fade")],
            on,
            |c, v| c.options.fade.on = v,
        );
        if on {
            let w = ui.available_width();
            self.chart_number(
                ui,
                id,
                chart,
                w,
                "Fade to",
                "Fade to",
                Scrub::whole(0.5).range(0.0..=100.0),
                0,
                |c| c.options.fade.to,
                |c, v| c.options.fade.to = v.clamp(0.0, 100.0),
            );
        }
    }

    /// A lollipop chart's Bars group: the stem and head sizes, the head's shape
    /// and fill, *Connect series* for a dumbbell, and the stems' colour.
    fn chart_lollipop_rows(&mut self, ui: &mut egui::Ui, id: NodeId, chart: &Chart) {
        self.chart_pair(ui, |app, ui, w| {
            app.chart_number(
                ui,
                id,
                chart,
                w,
                "Stem",
                "Stem width",
                Scrub::fine(0.1, 1).range(0.0..=40.0),
                1,
                |c| c.options.bars.stem,
                |c, v| c.options.bars.stem = v.max(0.0),
            );
            app.chart_number(
                ui,
                id,
                chart,
                w,
                "Head",
                "Head size",
                Scrub::whole(0.25).range(0.0..=100.0),
                0,
                |c| c.options.bars.head,
                |c, v| c.options.bars.head = v.max(0.0),
            );
        });
        self.chart_segments_of(
            ui,
            id,
            chart,
            "Head shape",
            &[
                (PointShape::Circle, "Circle"),
                (PointShape::Square, "Square"),
                (PointShape::Diamond, "Diamond"),
                (PointShape::Triangle, "Triangle"),
            ],
            chart.options.bars.head_shape,
            |c, v| c.options.bars.head_shape = v,
        );
        self.chart_segments_of(
            ui,
            id,
            chart,
            "Head fill",
            &[
                (chart::PointStyle::Filled, "Filled"),
                (chart::PointStyle::Hollow, "Hollow"),
            ],
            chart.options.bars.head_style,
            |c, v| c.options.bars.head_style = v,
        );
        let many = chart
            .mapping
            .series
            .iter()
            .filter(|c| !chart.series.iter().any(|s| s.column == **c && !s.visible))
            .count()
            > 1;
        let on = chart.options.bars.connect;
        self.chart_switch_if(
            ui,
            if many {
                Ok(())
            } else {
                Err("Connecting needs two series")
            },
            id,
            chart,
            "Connect series",
            on,
            |c, on| c.options.bars.connect = on,
        );
        let swatch = colour_of(&self.session.doc, id, chart, ChartColour::Stem)
            .map_or(ui::Swatch::Mixed, |c| {
                ui::Swatch::Solid(ui::color_to_egui(c))
            });
        let set = chart.options.bars.stem_colour.is_some();
        let full = ui.available_width();
        let reset = ui
            .horizontal(|ui| {
                ui.spacing_mut().item_spacing.x = ui::CARD_COL_GAP;
                let w = if set {
                    full - ui::CONTROL_H - ui::CARD_COL_GAP
                } else {
                    full
                };
                let chip = (!set)
                    .then_some(("Series", "Each series' own colour; pick a colour to set it"));
                self.chart_colour_row(
                    ui,
                    id,
                    ChartColour::Stem,
                    w,
                    swatch,
                    "Stem colour",
                    true,
                    chip,
                );
                set && ui::field_button(
                    ui,
                    icon::ARROW_COUNTER_CLOCKWISE,
                    ui::CONTROL_H,
                    ui::FieldButton::Off,
                )
                .on_hover_text("Use each series' colour")
                .clicked()
            })
            .inner;
        if reset {
            self.chart_each(id, chart, "Stem colour", |c| {
                c.options.bars.stem_colour = None
            });
        }
    }

    /// A waterfall's Steps: the three step colours, the totals picked by
    /// category, the final total's name and the connectors.
    fn chart_step_rows(&mut self, ui: &mut egui::Ui, id: NodeId, chart: &Chart) {
        for (slot, name) in [
            (ChartColour::Step(0), "Increase"),
            (ChartColour::Step(1), "Decrease"),
            (ChartColour::Step(2), "Total"),
        ] {
            self.chart_slot_row(ui, id, chart, slot, name);
        }
        let w = &chart.options.waterfall;
        let t = &chart.table;
        let cats: Vec<String> = chart
            .mapping
            .category
            .and_then(|c| t.index_of(c))
            .map(|c| {
                t.rows
                    .iter()
                    .map(|r| r[c].trim().to_string())
                    .filter(|s| !s.is_empty())
                    .collect()
            })
            .unwrap_or_default();
        let shown = match w.totals.len() {
            0 => "None".to_string(),
            1 | 2 => w.totals.join(", "),
            n => format!("{n} totals"),
        };
        let mut pick: Option<String> = None;
        ui::labelled(ui, "Totals at", |ui| {
            ui::combo_height(ui);
            egui::ComboBox::from_id_salt(("chart-waterfall-totals", id))
                .icon(ui::combo_chevron)
                .width(ui.available_width())
                .selected_text(ui::combo_text(&shown))
                .show_ui(ui, |ui| {
                    ui::menu_rows(ui);
                    for c in &cats {
                        if ui.selectable_label(w.totals.contains(c), c).clicked() {
                            pick = Some(c.clone());
                        }
                    }
                });
        });
        if let Some(name) = pick {
            self.chart_each(id, chart, "Waterfall totals", |c| {
                let t = &mut c.options.waterfall.totals;
                match t.iter().position(|n| *n == name) {
                    Some(i) => {
                        t.remove(i);
                    }
                    None => t.push(name.clone()),
                }
            });
        }
        let on = w.final_total.is_some();
        self.chart_switch(ui, id, chart, "Final total", on, |c, on| {
            c.options.waterfall.final_total = on.then(|| "Total".to_string());
        });
        if on {
            let width = ui.available_width();
            self.chart_text(
                ui,
                id,
                chart,
                "chart-final-total",
                width,
                "Name",
                "The final total's name",
                "Final total",
                |c| {
                    Some(
                        c.options
                            .waterfall
                            .final_total
                            .get_or_insert_with(|| "Total".into()),
                    )
                },
            );
        }
        let conn = w.connectors;
        self.chart_switch(ui, id, chart, "Connectors", conn, |c, on| {
            c.options.waterfall.connectors = on;
        });
        if conn {
            let dashed = w.dashed;
            self.chart_switch(ui, id, chart, "Dashed connectors", dashed, |c, on| {
                c.options.waterfall.dashed = on;
            });
        }
        self.chart_highlight_rows(ui, id, chart);
    }

    /// *References*: lines and bands on an axis, a fold headed by a `+` that
    /// adds a line at the value axis's middle, each row opening its popover.
    fn chart_references_section(
        &mut self,
        ui: &mut egui::Ui,
        id: NodeId,
        chart: &Chart,
        several: bool,
    ) {
        const KEY: &str = "Chart references";
        let open = !self.collapsed_panels.contains(KEY);
        let add = if chart.kind == ChartKind::Radar {
            Err("A radar takes no references")
        } else if !chart.kind.has_axes() {
            Err("Only a chart with axes takes references")
        } else if several {
            Err("Edit references one chart at a time")
        } else if chart.references.len() >= chart::MAX_REFERENCES {
            const _: () = assert!(chart::MAX_REFERENCES == 16, "the reason names the limit");
            Err("At most 16 references")
        } else {
            Ok(())
        };
        let (head, added) = ui::fold_rule_add(ui, "References", open, chart.references.len(), add);
        if head.clicked() {
            if open {
                self.collapsed_panels.insert(KEY);
            } else {
                self.collapsed_panels.remove(KEY);
            }
        }
        if added {
            let next_id = chart.references.iter().map(|r| r.id + 1).max().unwrap_or(1);
            let (lo, hi) = self.drawn_extent(id, chart, true);
            let mut next = chart.clone();
            next.references.push(chart::Reference {
                id: next_id,
                at: chart::At::Value(((lo + hi) / 2.0 * 100.0).round() / 100.0),
                ..chart::Reference::default()
            });
            self.chart_commit(id, "Add reference", next);
            self.collapsed_panels.remove(KEY);
        }
        if !open {
            return;
        }
        match add {
            Err(why) if !chart.kind.rect_plot() || several => {
                ui::selection_note(ui, why);
                return;
            }
            _ => {}
        }
        for r in chart.references.clone() {
            self.chart_reference_row(ui, id, chart, &r);
        }
    }

    /// One reference's row: its swatch, label and where it stands, its eye and
    /// its ×; the row opens its popover.
    fn chart_reference_row(
        &mut self,
        ui: &mut egui::Ui,
        id: NodeId,
        chart: &Chart,
        r: &chart::Reference,
    ) {
        let swatch = colour_of(&self.session.doc, id, chart, ChartColour::Reference(r.id))
            .map_or(ui::Swatch::Mixed, |c| {
                ui::Swatch::Solid(ui::color_to_egui(c))
            });
        let at = at_words(&r.at);
        let at = match r.kind {
            chart::RefKind::Line => at,
            chart::RefKind::Band => format!("{at}–{}", at_words(&r.to)),
        };
        let name = if r.label.trim().is_empty() {
            "Reference".to_string()
        } else {
            r.label.replace("{value}", "").trim().to_string()
        };
        let buttons = 2.0 * (ui::CONTROL_H + ui::CARD_COL_GAP);
        let w = ui.available_width() - buttons;
        let rid = r.id;
        let (mut delete, mut eye) = (false, false);
        let row = ui
            .horizontal(|ui| {
                ui.spacing_mut().item_spacing.x = ui::CARD_COL_GAP;
                let (_, resp) = ui::field_row_pressable(
                    ui,
                    egui::vec2(w, ui::CONTROL_H),
                    false,
                    r.visible,
                    |ui, ground| {
                        colour_row_face(ui, ground, swatch, &name, color::text(), |ui| {
                            detail_text(ui, &at);
                        });
                    },
                );
                let glyph = if r.visible {
                    icon::EYE
                } else {
                    icon::EYE_SLASH
                };
                eye = ui::field_button(ui, glyph, ui::CONTROL_H, ui::FieldButton::Off)
                    .on_hover_text(if r.visible {
                        "Hide this reference"
                    } else {
                        "Show this reference"
                    })
                    .clicked();
                delete = ui::field_button(ui, icon::X, ui::CONTROL_H, ui::FieldButton::Off)
                    .on_hover_text("Remove this reference")
                    .clicked();
                resp
            })
            .inner;
        // Double-clicked on the canvas: this row's popover opens.
        if self.open_reference == Some((id, rid)) {
            self.open_reference = None;
            egui::Popup::open_id(ui.ctx(), egui::Popup::default_response_id(&row));
        }
        egui::Popup::menu(&row)
            .close_behavior(egui::PopupCloseBehavior::CloseOnClickOutside)
            .gap(ui::MENU_GAP)
            .show(|ui| {
                ui::popup_rows(ui);
                ui.set_min_width(REFERENCE_POPOVER_W);
                ui.set_max_width(REFERENCE_POPOVER_W);
                self.chart_reference_popover(ui, id, chart, rid);
            });
        if eye {
            self.chart_each(id, chart, "Show reference", |c| {
                if let Some(r) = c.references.iter_mut().find(|r| r.id == rid) {
                    r.visible = !r.visible;
                }
            });
        }
        if delete {
            self.chart_each(id, chart, "Remove reference", |c| {
                c.references.retain(|r| r.id != rid);
            });
        }
    }

    /// A reference's popover: its kind, axis, where it stands (and a band's
    /// far end), its label, colour, width, dash, label place and *Take into
    /// the range*. Every field one step named for it.
    fn chart_reference_popover(&mut self, ui: &mut egui::Ui, id: NodeId, chart: &Chart, rid: u32) {
        let Some(r) = chart.references.iter().find(|r| r.id == rid).cloned() else {
            return;
        };
        let with = move |c: &mut Chart, f: &dyn Fn(&mut chart::Reference)| {
            if let Some(r) = c.references.iter_mut().find(|r| r.id == rid) {
                f(r);
            }
        };
        self.chart_segments_of(
            ui,
            id,
            chart,
            "Reference kind",
            &[
                (chart::RefKind::Line, "Line"),
                (chart::RefKind::Band, "Band"),
            ],
            r.kind,
            move |c, v| with(c, &|r| r.kind = v),
        );
        let axes: &[(chart::RefAxis, &str)] = &[
            (chart::RefAxis::Value, "Value"),
            (chart::RefAxis::Category, "Category"),
        ];
        self.chart_segments_of(
            ui,
            id,
            chart,
            "Reference axis",
            axes,
            r.axis,
            move |c, v| {
                with(c, &|r| {
                    if r.axis != v {
                        r.axis = v;
                        r.at = chart::At::AxisStart;
                        r.to = chart::At::AxisEnd;
                    }
                })
            },
        );
        self.chart_reference_at(ui, id, chart, &r, false);
        // A line keeps the band's far end, disabled: the popover holds the same
        // rows whatever is picked, as a popup never grows back.
        let band = r.kind == chart::RefKind::Band;
        let far = ui::disable_unless(ui, band, |ui| {
            ui.scope(|ui| self.chart_reference_at(ui, id, chart, &r, true))
                .response
                .rect
        })
        .inner;
        if !band {
            ui.interact(
                far,
                ui.id().with(("chart-ref-far", rid)),
                egui::Sense::hover(),
            )
            .on_hover_text("A line stands at one place; a band has a far end");
        }
        let w = ui.available_width();
        self.chart_text(
            ui,
            id,
            chart,
            ("chart-ref-label", rid),
            w,
            "Label",
            "Reference label: {value} writes where it stands",
            "Reference label",
            move |c| {
                c.references
                    .iter_mut()
                    .find(|r| r.id == rid)
                    .map(|r| &mut r.label)
            },
        );
        let swatch = colour_of(&self.session.doc, id, chart, ChartColour::Reference(rid))
            .map_or(ui::Swatch::Mixed, |c| {
                ui::Swatch::Solid(ui::color_to_egui(c))
            });
        let half = (ui.available_width() - ui::CARD_COL_GAP) / 2.0;
        ui.horizontal(|ui| {
            ui.spacing_mut().item_spacing.x = ui::CARD_COL_GAP;
            self.chart_colour_row(
                ui,
                id,
                ChartColour::Reference(rid),
                half,
                swatch,
                "Colour",
                true,
                None,
            );
            self.chart_number(
                ui,
                id,
                chart,
                half,
                "Width",
                "Reference width",
                Scrub::fine(0.05, 1).range(0.5..=16.0),
                1,
                move |c| {
                    c.references
                        .iter()
                        .find(|r| r.id == rid)
                        .map_or(1.5, |r| r.width)
                },
                move |c, v| with(c, &|r| r.width = v.clamp(0.5, 16.0)),
            );
        });
        self.chart_segments_of(
            ui,
            id,
            chart,
            "Reference dash",
            &[
                (chart::Dash::Solid, "Solid"),
                (chart::Dash::Dashed, "Dashed"),
                (chart::Dash::Dotted, "Dotted"),
            ],
            r.dash,
            move |c, v| with(c, &|r| r.dash = v),
        );
        self.chart_segments_of(
            ui,
            id,
            chart,
            "Reference label place",
            &[
                (chart::LabelAt::Start, "Start"),
                (chart::LabelAt::Middle, "Middle"),
                (chart::LabelAt::End, "End"),
            ],
            r.label_at,
            move |c, v| with(c, &|r| r.label_at = v),
        );
        let typed = matches!(r.at, chart::At::Value(_)) && r.axis == chart::RefAxis::Value;
        self.chart_switch_if(
            ui,
            if typed {
                Ok(())
            } else {
                Err("A computed or category reference is inside the data")
            },
            id,
            chart,
            "Take into the range",
            r.in_range,
            move |c, on| with(c, &|r| r.in_range = on),
        );
    }

    /// Where a reference stands (`to`: a band's far end): on the value axis a
    /// typed value or a statistic of every series shown or one; on the category
    /// axis a category, or a number on a scatter's X; either end of the axis.
    fn chart_reference_at(
        &mut self,
        ui: &mut egui::Ui,
        id: NodeId,
        chart: &Chart,
        r: &chart::Reference,
        to: bool,
    ) {
        use chart::{At, Stat};
        let rid = r.id;
        let now = if to { r.to.clone() } else { r.at.clone() };
        let label: &'static str = if to {
            "To"
        } else if r.kind == chart::RefKind::Band {
            "From"
        } else {
            "At"
        };
        let mut options: Vec<(At, String)> = vec![];
        let value_axis = r.axis == chart::RefAxis::Value;
        let scatter = chart.kind == ChartKind::Scatter;
        if value_axis || scatter {
            let v = match now {
                At::Value(v) => v,
                _ => 0.0,
            };
            options.push((At::Value(v), "Value".into()));
        }
        if value_axis {
            for (s, n) in [
                (Stat::Average, "Average"),
                (Stat::Median, "Median"),
                (Stat::Min, "Minimum"),
                (Stat::Max, "Maximum"),
            ] {
                let of = match &now {
                    At::Stat { of, .. } => *of,
                    _ => None,
                };
                options.push((At::Stat { stat: s, of }, n.into()));
            }
        } else if !scatter {
            let t = &chart.table;
            if let Some(c) = chart.mapping.category.and_then(|c| t.index_of(c)) {
                for row in &t.rows {
                    let n = row[c].trim();
                    if !n.is_empty() && !options.iter().any(|(_, o)| o == n) {
                        options.push((At::Category(n.into()), n.into()));
                    }
                }
            }
        }
        if r.kind == chart::RefKind::Band {
            options.push((At::AxisStart, "Start".into()));
            options.push((At::AxisEnd, "End".into()));
        }
        let lit = options
            .iter()
            .position(|(a, _)| match (a, &now) {
                (At::Value(_), At::Value(_)) => true,
                (At::Stat { stat: x, .. }, At::Stat { stat: y, .. }) => x == y,
                (a, b) => a == b,
            })
            .unwrap_or(0);
        let names: Vec<&str> = options.iter().map(|(_, n)| n.as_str()).collect();
        let picks: Vec<At> = options.iter().map(|(a, _)| a.clone()).collect();
        let salt: &'static str = if to { "chart-ref-to" } else { "chart-ref-at" };
        self.chart_menu(ui, id, chart, salt, label, &names, lit, move |c, i| {
            if let Some(r) = c.references.iter_mut().find(|r| r.id == rid) {
                let a = picks[i].clone();
                if to {
                    r.to = a;
                } else {
                    r.at = a;
                }
            }
        });
        match now {
            At::Value(v) => {
                let w = ui.available_width();
                self.chart_number(
                    ui,
                    id,
                    chart,
                    w,
                    "Value",
                    "Reference value",
                    Scrub::fine(0.1, 2),
                    2,
                    move |_| v,
                    move |c, v| {
                        if let Some(r) = c.references.iter_mut().find(|r| r.id == rid) {
                            let a = At::Value(v);
                            if to {
                                r.to = a;
                            } else {
                                r.at = a;
                            }
                        }
                    },
                );
            }
            At::Stat { stat, of } if !to => {
                let mut series: Vec<(Option<chart::ColumnId>, String)> =
                    vec![(None, "Every series shown".into())];
                for c in &chart.mapping.series {
                    if let Some(col) = chart.table.column(*c) {
                        series.push((Some(*c), col.name.clone()));
                    }
                }
                let names: Vec<&str> = series.iter().map(|(_, n)| n.as_str()).collect();
                let lit = series.iter().position(|(c, _)| *c == of).unwrap_or(0);
                let cols: Vec<Option<chart::ColumnId>> = series.iter().map(|(c, _)| *c).collect();
                self.chart_menu(
                    ui,
                    id,
                    chart,
                    "chart-ref-of",
                    "Of",
                    &names,
                    lit,
                    move |c, i| {
                        if let Some(r) = c.references.iter_mut().find(|r| r.id == rid) {
                            r.at = At::Stat { stat, of: cols[i] };
                        }
                    },
                );
            }
            // Every pick keeps one row under it, so the popover holds its size.
            ref other => {
                let why = match other {
                    At::Stat { .. } => "A far end's statistic is over every series shown",
                    At::Category(_) => "A category reference stands at its category",
                    _ => "It stands at the axis's end",
                };
                let w = ui.available_width();
                self.chart_number_if(
                    ui,
                    Err(why),
                    id,
                    chart,
                    w,
                    "Value",
                    "Reference value",
                    Scrub::fine(0.1, 2),
                    2,
                    |_| 0.0,
                    |_, _| {},
                );
            }
        }
    }

    /// One of the chart's own colours (not a series') as a full-width
    /// [`Self::chart_colour_row`] named `name`.
    fn chart_slot_row(
        &mut self,
        ui: &mut egui::Ui,
        id: NodeId,
        chart: &Chart,
        slot: ChartColour,
        name: &str,
    ) {
        let swatch = colour_of(&self.session.doc, id, chart, slot).map_or(ui::Swatch::Mixed, |c| {
            ui::Swatch::Solid(ui::color_to_egui(c))
        });
        // An unset colour says it follows the theme, as an inheriting value
        // does; a set one offers the way back.
        let set = is_set(chart, slot);
        let derived = (!set).then_some((
            "From the theme",
            "Derived from the theme and the background; pick a colour to set it",
        ));
        let full = ui.available_width();
        let reset = ui
            .horizontal(|ui| {
                ui.spacing_mut().item_spacing.x = ui::CARD_COL_GAP;
                let width = if set {
                    full - ui::CONTROL_H - ui::CARD_COL_GAP
                } else {
                    full
                };
                self.chart_colour_row(ui, id, slot, width, swatch, name, true, derived);
                set && ui::field_button(
                    ui,
                    icon::ARROW_COUNTER_CLOCKWISE,
                    ui::CONTROL_H,
                    ui::FieldButton::Off,
                )
                .on_hover_text("Use the theme's colour")
                .clicked()
            })
            .inner;
        if reset {
            self.chart_each(id, chart, "Use the theme's colour", |c| {
                *c = with_paint(c, slot, None);
            });
        }
    }

    /// One colour of the chart as a field `width` wide, the whole of which opens
    /// the picker on the slot while `live` (a hidden series' row is not, and says
    /// so): its swatch, its name (dim when not), a right-aligned `detail` with
    /// its tip, and, bound, its variable's name in a capsule.
    #[allow(clippy::too_many_arguments)]
    fn chart_colour_row(
        &mut self,
        ui: &mut egui::Ui,
        id: NodeId,
        slot: ChartColour,
        width: f32,
        swatch: ui::Swatch,
        name: &str,
        live: bool,
        detail: Option<(&str, &str)>,
    ) {
        let open = self
            .picker
            .as_ref()
            .is_some_and(|p| p.slot == PaintSlot::Chart(slot));
        let size = egui::vec2(width, ui::CONTROL_H);
        let ink = if live {
            color::text()
        } else {
            theme::text::off()
        };
        let binding = self.chart_binding(id, slot).map(|(var, _)| var);
        let doc = &self.session.doc;
        let (_, resp) = ui::field_row_pressable(ui, size, open, live, |ui, ground| {
            colour_row_face(ui, ground, swatch, name, ink, |ui| {
                if let Some((text, tip)) = detail {
                    detail_text(ui, text).on_hover_text(tip);
                }
                if let Some(var) = binding {
                    let var_name = doc.variables().get(var).map_or("", |v| v.name.as_str());
                    let max_w = (ui.available_width() / 2.0).max(0.0);
                    let w = super::binding::capsule_width(ui, var_name, max_w);
                    let (rect, resp) = ui.allocate_exact_size(
                        egui::vec2(w, ui.available_height()),
                        egui::Sense::hover(),
                    );
                    super::binding::capsule(ui, rect.left_center(), var_name, max_w);
                    resp.on_hover_text(super::binding::chip_tip(doc, id, var));
                }
            });
        });
        if !live {
            resp.on_hover_text("Show the series to change its colour");
        } else if resp.clicked() {
            self.open_picker(ui, id, PaintSlot::Chart(slot), resp.rect.left_top());
        }
    }

    /// A series row's style button: a menu of its line's dash (a line or area)
    /// and its points' shape, each pick one step.
    fn chart_series_style(
        &mut self,
        ui: &mut egui::Ui,
        id: NodeId,
        chart: &Chart,
        col: chart::ColumnId,
    ) {
        let style = chart.series.iter().find(|s| s.column == col);
        let dash = style.map_or(chart::Dash::Solid, |s| s.dash);
        let point = style.and_then(|s| s.point);
        let lined = chart.kind != ChartKind::Scatter;
        let tip = if lined {
            "Line and point style"
        } else {
            "Point shape"
        };
        let button = ui::field_button(ui, icon::DOTS_THREE, ui::CONTROL_H, ui::FieldButton::Off)
            .on_hover_text(tip);
        // The dash, or the point shape (`Err`) picked.
        let mut pick: Option<Result<chart::Dash, Option<PointShape>>> = None;
        egui::Popup::menu(&button)
            .align(egui::RectAlign::BOTTOM_END)
            .gap(ui::MENU_GAP)
            .show(|ui| {
                ui::popup_rows(ui);
                ui.set_min_width(SERIES_MENU_W);
                if lined {
                    ui.label(ui::eyebrow("Line"));
                    for (d, name) in [
                        (chart::Dash::Solid, "Solid"),
                        (chart::Dash::Dashed, "Dashed"),
                        (chart::Dash::Dotted, "Dotted"),
                    ] {
                        if ui.selectable_label(d == dash, name).clicked() && d != dash {
                            pick = Some(Ok(d));
                        }
                    }
                    ui.label(ui::eyebrow("Points"));
                }
                // A line's unshaped points are circles, so it lists Circle once;
                // a scatter's take the chart's shape.
                let shapes: &[(Option<PointShape>, &str)] = if lined {
                    &[
                        (None, "Circle"),
                        (Some(PointShape::Square), "Square"),
                        (Some(PointShape::Diamond), "Diamond"),
                        (Some(PointShape::Triangle), "Triangle"),
                    ]
                } else {
                    &[
                        (None, "Chart's shape"),
                        (Some(PointShape::Circle), "Circle"),
                        (Some(PointShape::Square), "Square"),
                        (Some(PointShape::Diamond), "Diamond"),
                        (Some(PointShape::Triangle), "Triangle"),
                    ]
                };
                let point = match point {
                    Some(PointShape::Circle) if lined => None,
                    p => p,
                };
                for (p, name) in shapes {
                    let on = *p == point;
                    if ui.selectable_label(on, *name).clicked() && !on {
                        pick = Some(Err(*p));
                    }
                }
            });
        if let Some(pick) = pick {
            let mut next = chart.clone();
            if !next.series.iter().any(|s| s.column == col) {
                next = chart::with_series_fill(&next, col, None);
            }
            let s = next.series.iter_mut().find(|s| s.column == col);
            let label = match (pick, s) {
                (Ok(d), Some(s)) => {
                    s.dash = d;
                    "Series dash"
                }
                (Err(p), Some(s)) => {
                    s.point = p;
                    "Series point shape"
                }
                (_, None) => return,
            };
            self.chart_commit(id, label, next);
        }
    }

    fn chart_mark_rows(&mut self, ui: &mut egui::Ui, id: NodeId, chart: &Chart) {
        match chart.kind {
            ChartKind::Radar => {
                self.chart_segments_of(
                    ui,
                    id,
                    chart,
                    "Radar grid",
                    &[
                        (chart::RadarGrid::Polygon, "Polygon"),
                        (chart::RadarGrid::Circle, "Circle"),
                    ],
                    chart.options.radar.grid,
                    |c, v| c.options.radar.grid = v,
                );
                let spokes = chart.options.radar.spokes;
                self.chart_switch(ui, id, chart, "Spokes", spokes, |c, on| {
                    c.options.radar.spokes = on;
                });
                self.chart_pair(ui, |app, ui, w| {
                    app.chart_number(
                        ui,
                        id,
                        chart,
                        w,
                        "Start",
                        "Radar start angle",
                        Scrub::whole(1.0).range(-360.0..=360.0),
                        0,
                        |c| c.options.radar.start_angle,
                        |c, v| c.options.radar.start_angle = v.clamp(-360.0, 360.0),
                    );
                    app.chart_number(
                        ui,
                        id,
                        chart,
                        w,
                        "Width",
                        "Line width",
                        Scrub::fine(0.1, 1).range(0.0..=40.0),
                        1,
                        |c| c.options.lines.width,
                        |c, v| c.options.lines.width = v.max(0.0),
                    );
                });
                self.chart_segments_of(
                    ui,
                    id,
                    chart,
                    "Points",
                    &[
                        (chart::PointStyle::None, "None"),
                        (chart::PointStyle::Hollow, "Hollow"),
                        (chart::PointStyle::Filled, "Filled"),
                    ],
                    chart.options.lines.points,
                    |c, v| c.options.lines.points = v,
                );
                let filled = chart.options.radar.filled;
                self.chart_number_if(
                    ui,
                    if filled {
                        Ok(())
                    } else {
                        Err("Only a filled radar paints its area")
                    },
                    id,
                    chart,
                    ui.available_width(),
                    "Area",
                    "Area opacity",
                    Scrub::whole(0.5).range(0.0..=100.0),
                    0,
                    |c| c.options.lines.area_opacity * 100.0,
                    |c, v| c.options.lines.area_opacity = (v / 100.0).clamp(0.0, 1.0),
                );
            }
            ChartKind::Column | ChartKind::Bar | ChartKind::Waterfall => {
                let b = &chart.options.bars;
                let waterfall = chart.kind == ChartKind::Waterfall;
                let lollipops = !waterfall && b.shape == chart::BarShape::Lollipops;
                if !waterfall {
                    self.chart_bar_shape_row(ui, id, chart);
                }
                // The slots a bar or a lollipop takes, either way.
                let bar_gap = if waterfall {
                    Err("A waterfall draws one bar a step")
                } else if lollipops && b.connect {
                    Err("Connected heads share one place")
                } else {
                    Ok(())
                };
                self.chart_pair(ui, |app, ui, w| {
                    app.chart_number(
                        ui,
                        id,
                        chart,
                        w,
                        "Group gap",
                        "Group gap",
                        Scrub::whole(0.5).range(0.0..=100.0),
                        0,
                        |c| c.options.bars.group_gap,
                        |c, v| c.options.bars.group_gap = v,
                    );
                    app.chart_number_if(
                        ui,
                        bar_gap,
                        id,
                        chart,
                        w,
                        "Bar gap",
                        "Bar gap",
                        Scrub::whole(0.5).range(0.0..=100.0),
                        0,
                        |c| c.options.bars.bar_gap,
                        |c, v| c.options.bars.bar_gap = v,
                    );
                });
                if lollipops {
                    self.chart_lollipop_rows(ui, id, chart);
                } else {
                    self.chart_number(
                        ui,
                        id,
                        chart,
                        ui.available_width(),
                        "Radius",
                        "Bar radius",
                        Scrub::whole(0.25).range(0.0..=100.0),
                        0,
                        |c| c.options.bars.radius,
                        |c, v| c.options.bars.radius = v,
                    );
                    self.chart_fade_rows(ui, id, chart);
                    // A floating step rounds every corner already.
                    if !waterfall {
                        let all = chart.options.bars.radius_all;
                        self.chart_switch(ui, id, chart, "Round every corner", all, |c, on| {
                            c.options.bars.radius_all = on;
                        });
                    }
                }
                // A waterfall runs in table order: its walk is the order.
                if !waterfall {
                    self.chart_bar_order_rows(ui, id, chart);
                }
            }
            ChartKind::Line | ChartKind::Area => {
                if chart.kind == ChartKind::Area {
                    self.chart_fade_rows(ui, id, chart);
                }
                // A line picks its curve as its Type variant; an area here, every
                // curve the generator draws for it.
                if chart.kind == ChartKind::Area {
                    self.chart_segments_of(
                        ui,
                        id,
                        chart,
                        "Curve",
                        &[
                            (Curve::Straight, "Straight"),
                            (Curve::Smooth, "Smooth"),
                            (Curve::Step, "Step"),
                        ],
                        chart.options.lines.curve,
                        |c, v| c.options.lines.curve = v,
                    );
                }
                self.chart_pair(ui, |app, ui, w| {
                    app.chart_number(
                        ui,
                        id,
                        chart,
                        w,
                        "Width",
                        "Line width",
                        Scrub::fine(0.1, 1).range(0.0..=40.0),
                        1,
                        |c| c.options.lines.width,
                        |c, v| c.options.lines.width = v,
                    );
                    app.chart_number(
                        ui,
                        id,
                        chart,
                        w,
                        "Points",
                        "Point size",
                        Scrub::whole(0.25).range(0.0..=100.0),
                        0,
                        |c| c.options.lines.point_size,
                        |c, v| c.options.lines.point_size = v,
                    );
                });
                self.chart_segments_of(
                    ui,
                    id,
                    chart,
                    "Points",
                    &[
                        (PointStyle::None, "None"),
                        (PointStyle::Hollow, "Hollow"),
                        (PointStyle::Filled, "Filled"),
                    ],
                    chart.options.lines.points,
                    |c, v| c.options.lines.points = v,
                );
                if chart.kind == ChartKind::Area {
                    self.chart_number(
                        ui,
                        id,
                        chart,
                        ui.available_width(),
                        "Opacity",
                        "Area opacity",
                        Scrub::whole(0.5).range(0.0..=100.0),
                        0,
                        |c| c.options.lines.area_opacity * 100.0,
                        |c, v| c.options.lines.area_opacity = v / 100.0,
                    );
                }
                let gaps = chart.options.lines.connect_gaps;
                self.chart_switch(ui, id, chart, "Connect gaps", gaps, |c, on| {
                    c.options.lines.connect_gaps = on;
                });
            }
            ChartKind::Pie => {
                let donut = match chart.options.slices.donut {
                    true => Ok(()),
                    false => Err("Only a donut has a hole"),
                };
                self.chart_pair(ui, |app, ui, w| {
                    app.chart_number_if(
                        ui,
                        donut,
                        id,
                        chart,
                        w,
                        "Hole",
                        "Donut hole",
                        Scrub::whole(0.5).range(0.0..=95.0),
                        0,
                        |c| c.options.slices.hole,
                        |c, v| c.options.slices.hole = v,
                    );
                    app.chart_number(
                        ui,
                        id,
                        chart,
                        w,
                        "Pad",
                        "Slice gap",
                        Scrub::fine(0.1, 1).range(0.0..=45.0),
                        1,
                        |c| c.options.slices.pad_angle,
                        |c, v| c.options.slices.pad_angle = v,
                    );
                });
                self.chart_pair(ui, |app, ui, w| {
                    app.chart_number(
                        ui,
                        id,
                        chart,
                        w,
                        "Radius",
                        "Slice radius",
                        Scrub::whole(0.25).range(0.0..=100.0),
                        0,
                        |c| c.options.slices.radius,
                        |c, v| c.options.slices.radius = v,
                    );
                    app.chart_number(
                        ui,
                        id,
                        chart,
                        w,
                        "Start",
                        "Start angle",
                        Scrub::whole(1.0).range(-360.0..=360.0),
                        0,
                        |c| c.options.slices.start_angle,
                        |c, v| c.options.slices.start_angle = v,
                    );
                });
                self.chart_slice_own_rows(ui, id, chart);
                if self
                    .chart_card
                    .as_ref()
                    .is_some_and(|c| c.id == id && c.track)
                {
                    self.chart_slot_row(ui, id, chart, ChartColour::Track, "Track colour");
                }
            }
            ChartKind::Scatter => {
                let bubbles = match chart.mapping.size {
                    Some(_) => Ok(()),
                    None => Err("Only bubbles use it: their points take a size column"),
                };
                self.chart_pair(ui, |app, ui, w| {
                    app.chart_number(
                        ui,
                        id,
                        chart,
                        w,
                        "Size",
                        "Point size",
                        Scrub::whole(0.25).range(0.0..=200.0),
                        0,
                        |c| c.options.points.size,
                        |c, v| c.options.points.size = v,
                    );
                    app.chart_number_if(
                        ui,
                        bubbles,
                        id,
                        chart,
                        w,
                        "Largest",
                        "Largest bubble",
                        Scrub::whole(0.5).range(0.0..=400.0),
                        0,
                        |c| c.options.points.max_size,
                        |c, v| c.options.points.max_size = v,
                    );
                });
                self.chart_number(
                    ui,
                    id,
                    chart,
                    ui.available_width(),
                    "Opacity",
                    "Point opacity",
                    Scrub::whole(0.5).range(0.0..=100.0),
                    0,
                    |c| c.options.points.opacity * 100.0,
                    |c, v| c.options.points.opacity = v / 100.0,
                );
                self.chart_segments_of(
                    ui,
                    id,
                    chart,
                    "Point shape",
                    &[
                        (PointShape::Circle, "Circle"),
                        (PointShape::Square, "Square"),
                        (PointShape::Diamond, "Diamond"),
                        (PointShape::Triangle, "Triangle"),
                    ],
                    chart.options.points.shape,
                    |c, v| c.options.points.shape = v,
                );
            }
            ChartKind::Sankey => {
                self.chart_pair(ui, |app, ui, w| {
                    app.chart_number(
                        ui,
                        id,
                        chart,
                        w,
                        "Width",
                        "Node width",
                        Scrub::whole(0.25).range(1.0..=200.0),
                        0,
                        |c| c.options.flows.node_width,
                        |c, v| c.options.flows.node_width = v,
                    );
                    app.chart_number(
                        ui,
                        id,
                        chart,
                        w,
                        "Gap",
                        "Node gap",
                        Scrub::whole(0.25).range(0.0..=200.0),
                        0,
                        |c| c.options.flows.node_gap,
                        |c, v| c.options.flows.node_gap = v,
                    );
                });
                self.chart_pair(ui, |app, ui, w| {
                    app.chart_number(
                        ui,
                        id,
                        chart,
                        w,
                        "Radius",
                        "Node radius",
                        Scrub::whole(0.25).range(0.0..=100.0),
                        0,
                        |c| c.options.flows.node_radius,
                        |c, v| c.options.flows.node_radius = v,
                    );
                    app.chart_number(
                        ui,
                        id,
                        chart,
                        w,
                        "Opacity",
                        "Flow opacity",
                        Scrub::whole(0.5).range(0.0..=100.0),
                        0,
                        |c| c.options.flows.opacity * 100.0,
                        |c, v| c.options.flows.opacity = v / 100.0,
                    );
                });
                self.chart_menu_of(
                    ui,
                    id,
                    chart,
                    "chart-flow-align",
                    "Alignment",
                    &[
                        (FlowAlign::Justify, "Justify"),
                        (FlowAlign::Left, "Left"),
                        (FlowAlign::Right, "Right"),
                        (FlowAlign::Centre, "Centre"),
                    ],
                    chart.options.flows.align,
                    |c, v| c.options.flows.align = v,
                );
                self.chart_segments_of(
                    ui,
                    id,
                    chart,
                    "Node order",
                    &[(true, "Automatic"), (false, "Table order")],
                    chart.options.flows.relax,
                    |c, v| c.options.flows.relax = v,
                );
                self.chart_menu_of(
                    ui,
                    id,
                    chart,
                    "chart-flow-colour",
                    "Flow colour",
                    &[
                        (FlowColour::Source, "Source"),
                        (FlowColour::Target, "Target"),
                        (FlowColour::Gradient, "Gradient"),
                        (FlowColour::Neutral, "Neutral"),
                    ],
                    chart.options.flows.colour,
                    |c, v| c.options.flows.colour = v,
                );
                if chart.options.flows.colour == FlowColour::Neutral {
                    self.chart_slot_row(ui, id, chart, ChartColour::Neutral, "Neutral colour");
                }
                self.chart_segments_of(
                    ui,
                    id,
                    chart,
                    "Node labels",
                    &[
                        (NodeLabels::Name, "Name"),
                        (NodeLabels::NameAndValue, "Name and value"),
                        (NodeLabels::Off, "Off"),
                    ],
                    chart.options.flows.labels,
                    |c, v| c.options.flows.labels = v,
                );
            }
        }
        // One colour for values under the baseline; a 100% chart's are shares.
        if matches!(
            chart.kind,
            ChartKind::Column | ChartKind::Bar | ChartKind::Line | ChartKind::Area
        ) {
            let percent = match chart.kind {
                ChartKind::Area => chart.options.lines.area_stacking == chart::Stacking::Percent,
                ChartKind::Line => false,
                _ => chart.options.bars.stacking == chart::Stacking::Percent,
            };
            let live = if percent {
                Err("A 100% chart draws shares, none below zero")
            } else {
                Ok(())
            };
            let on = chart.options.below_zero.is_some();
            self.chart_switch_if(ui, live, id, chart, "Colour below zero", on, |c, on| {
                c.options.below_zero = on.then(|| Brush::Solid(chart::ALERT_RED).into());
            });
            // Its own colour only: unset is the switch off, so no reset.
            if on && live.is_ok() {
                let swatch = colour_of(&self.session.doc, id, chart, ChartColour::BelowZero)
                    .map_or(ui::Swatch::Mixed, |c| {
                        ui::Swatch::Solid(ui::color_to_egui(c))
                    });
                let w = ui.available_width();
                self.chart_colour_row(
                    ui,
                    id,
                    ChartColour::BelowZero,
                    w,
                    swatch,
                    "Below zero",
                    true,
                    None,
                );
            }
        }
        // The marks' stroke: on a solid mark (a separator, in the background
        // unless set) or over a tint of it (an outline, in the mark's colour).
        let stroke = &chart.options.stroke;
        self.chart_segments_of(
            ui,
            id,
            chart,
            "Mark stroke",
            &[
                (chart::StrokeStyle::Solid, "Solid"),
                (chart::StrokeStyle::Outline, "Outline"),
            ],
            stroke.style,
            |c, v| c.options.stroke.style = v,
        );
        let half = (ui.available_width() - ui::CARD_COL_GAP) / 2.0;
        ui.horizontal(|ui| {
            ui.spacing_mut().item_spacing.x = ui::CARD_COL_GAP;
            self.chart_number(
                ui,
                id,
                chart,
                half,
                "Width",
                "Mark stroke width",
                Scrub::fine(0.1, 1).range(0.0..=20.0),
                1,
                |c| c.options.stroke.width,
                |c, v| c.options.stroke.width = v.max(0.0),
            );
            let swatch = colour_of(&self.session.doc, id, chart, ChartColour::Stroke)
                .map_or(ui::Swatch::Mixed, |c| {
                    ui::Swatch::Solid(ui::color_to_egui(c))
                });
            // Unset, the stroke is the background's or each mark's own.
            let chip = match (stroke.colour.is_some(), stroke.style) {
                (true, _) => None,
                (false, chart::StrokeStyle::Solid) => Some((
                    "Background",
                    "The background's colour; pick a colour to set it",
                )),
                (false, chart::StrokeStyle::Outline) => {
                    Some(("Series", "Each mark's own colour; pick a colour to set it"))
                }
            };
            let set = stroke.colour.is_some();
            let w = if set {
                half - ui::CONTROL_H - ui::CARD_COL_GAP
            } else {
                half
            };
            self.chart_colour_row(ui, id, ChartColour::Stroke, w, swatch, "Stroke", true, chip);
            set && ui::field_button(
                ui,
                icon::ARROW_COUNTER_CLOCKWISE,
                ui::CONTROL_H,
                ui::FieldButton::Off,
            )
            .on_hover_text("Use the default stroke colour")
            .clicked()
        })
        .inner
        .then(|| {
            self.chart_each(id, chart, "Mark stroke colour", |c| {
                c.options.stroke.colour = None;
            });
        });
    }

    /// The *Axes* section's rows; `own` leaves out each axis's typed range and
    /// title when false, as on a styled chart, which shows them under *This
    /// chart*.
    fn chart_axis_rows(&mut self, ui: &mut egui::Ui, id: NodeId, chart: &Chart, own: bool) {
        let (first, second) = axis_titles(chart.kind);
        for (value, title) in [(true, first), (false, second)] {
            ui.label(ui::eyebrow(title));
            let pick = move |c: &Chart| {
                if value {
                    c.options.value_axis.clone()
                } else {
                    c.options.category_axis.clone()
                }
            };
            let put = move |c: &mut Chart, a: chart::Axis| {
                if value {
                    c.options.value_axis = a
                } else {
                    c.options.category_axis = a
                }
            };
            let axis = pick(chart);
            let label = if value { "Value axis" } else { "Category axis" };
            self.chart_switch(
                ui,
                id,
                chart,
                if value {
                    "Show value axis"
                } else {
                    "Show category axis"
                },
                axis.show,
                move |c, on| {
                    let mut a = pick(c);
                    a.show = on;
                    put(c, a);
                },
            );
            if !axis.show {
                continue;
            }
            self.chart_switch(
                ui,
                id,
                chart,
                if value {
                    "Value axis line"
                } else {
                    "Category axis line"
                },
                axis.line,
                move |c, on| {
                    let mut a = pick(c);
                    a.line = on;
                    put(c, a);
                },
            );
            self.chart_switch(
                ui,
                id,
                chart,
                if value {
                    "Value tick marks"
                } else {
                    "Category tick marks"
                },
                axis.ticks,
                move |c, on| {
                    let mut a = pick(c);
                    a.ticks = on;
                    put(c, a);
                },
            );
            if axis.line || axis.ticks {
                let name = if value {
                    "Value axis colour"
                } else {
                    "Category axis colour"
                };
                self.chart_slot_row(ui, id, chart, ChartColour::Axis(value), name);
                // Its own half-width field, since the colour row is one control.
                let half = (ui.available_width() - ui::CARD_COL_GAP) / 2.0;
                self.chart_number(
                    ui,
                    id,
                    chart,
                    half,
                    "Width",
                    "Axis line width",
                    Scrub::fine(0.05, 1).range(0.5..=8.0),
                    1,
                    move |c| pick(c).line_width,
                    move |c, v| {
                        let mut a = pick(c);
                        a.line_width = (v * 2.0).round().clamp(1.0, 16.0) / 2.0;
                        put(c, a);
                    },
                );
            }
            // A band axis (columns, bars, lines, areas) draws no gridlines of its
            // own: only a scatter's X axis is a scale with ticks to rule.
            // A radar's category gridlines are its spokes.
            let grid = match value || matches!(chart.kind, ChartKind::Scatter | ChartKind::Radar) {
                true => Ok(()),
                false => {
                    Err("Only a scatter's X axis and a radar's spokes draw category gridlines")
                }
            };
            // A radar's spokes are switched in Marks; here they are only styled.
            let spokes = !value && chart.kind == ChartKind::Radar;
            if !spokes {
                self.chart_switch_if(
                    ui,
                    grid,
                    id,
                    chart,
                    if value {
                        "Value gridlines"
                    } else {
                        "Category gridlines"
                    },
                    axis.grid.show,
                    move |c, on| {
                        let mut a = pick(c);
                        a.grid.show = on;
                        put(c, a);
                    },
                );
            }
            if (axis.grid.show || spokes) && grid.is_ok() {
                self.chart_switch(
                    ui,
                    id,
                    chart,
                    if value {
                        "Dashed value gridlines"
                    } else {
                        "Dashed category gridlines"
                    },
                    axis.grid.dashed,
                    move |c, on| {
                        let mut a = pick(c);
                        a.grid.dashed = on;
                        put(c, a);
                    },
                );
                let name = if value {
                    "Value gridline colour"
                } else {
                    "Category gridline colour"
                };
                self.chart_slot_row(ui, id, chart, ChartColour::Grid(value), name);
                let half = (ui.available_width() - ui::CARD_COL_GAP) / 2.0;
                self.chart_number(
                    ui,
                    id,
                    chart,
                    half,
                    "Width",
                    "Gridline width",
                    Scrub::fine(0.05, 1).range(0.5..=8.0),
                    1,
                    move |c| pick(c).grid.width,
                    move |c, v| {
                        let mut a = pick(c);
                        a.grid.width = (v * 2.0).round().clamp(1.0, 16.0) / 2.0;
                        put(c, a);
                    },
                );
            }
            self.chart_segments_of(
                ui,
                id,
                chart,
                if value {
                    "Value label angle"
                } else {
                    "Category label angle"
                },
                &[
                    (LabelAngle::Auto, "Auto"),
                    (LabelAngle::Flat, "0°"),
                    (LabelAngle::Slanted, "45°"),
                    (LabelAngle::Upright, "90°"),
                ],
                axis.angle,
                move |c, v| {
                    let mut a = pick(c);
                    a.angle = v;
                    put(c, a);
                },
            );
            let sides: &[(AxisSide, &str)] = match (chart.kind == ChartKind::Bar) == value {
                true => &[(AxisSide::Start, "Bottom"), (AxisSide::End, "Top")],
                false => &[(AxisSide::Start, "Left"), (AxisSide::End, "Right")],
            };
            self.chart_segments_of(
                ui,
                id,
                chart,
                if value {
                    "Value axis side"
                } else {
                    "Category axis side"
                },
                sides,
                axis.side,
                move |c, v| {
                    let mut a = pick(c);
                    a.side = v;
                    put(c, a);
                },
            );
            if value || chart.kind == ChartKind::Scatter {
                let zero_held = !matches!(chart.kind, ChartKind::Line | ChartKind::Scatter);
                if !zero_held {
                    self.chart_switch(
                        ui,
                        id,
                        chart,
                        if value {
                            "Start at zero"
                        } else {
                            "Start X at zero"
                        },
                        axis.start_at_zero,
                        move |c, on| {
                            let mut a = pick(c);
                            a.start_at_zero = on;
                            put(c, a);
                        },
                    );
                }
                if own {
                    self.chart_axis_range(ui, id, chart, value);
                }
                if value {
                    self.chart_format(ui, id, chart, "chart-value-format", |c| {
                        &mut c.options.value_axis.format
                    });
                } else {
                    self.chart_format(ui, id, chart, "chart-x-format", |c| {
                        &mut c.options.category_axis.format
                    });
                }
                let auto = axis.tick_count.is_none();
                self.chart_switch(
                    ui,
                    id,
                    chart,
                    if value { "Auto ticks" } else { "Auto X ticks" },
                    auto,
                    move |c, on| {
                        let mut a = pick(c);
                        a.tick_count = (!on).then_some(5);
                        put(c, a);
                    },
                );
                if !auto {
                    self.chart_number(
                        ui,
                        id,
                        chart,
                        ui.available_width(),
                        "Ticks",
                        if value { "Tick count" } else { "X tick count" },
                        Scrub::whole(0.1).range(1.0..=100.0),
                        0,
                        move |c| f64::from(pick(c).tick_count.unwrap_or(5)),
                        move |c, v| {
                            let mut a = pick(c);
                            a.tick_count = Some(v.round().clamp(1.0, 100.0) as u32);
                            put(c, a);
                        },
                    );
                }
            }
            if own {
                self.chart_axis_title(ui, id, chart, value, label);
            }
        }
    }

    /// An axis's *Auto range* switch and, off, its Min and Max: the value axis's
    /// (`value`), or a scatter's X axis's.
    fn chart_axis_range(&mut self, ui: &mut egui::Ui, id: NodeId, chart: &Chart, value: bool) {
        let pick = move |c: &Chart| {
            if value {
                c.options.value_axis.clone()
            } else {
                c.options.category_axis.clone()
            }
        };
        let put = move |c: &mut Chart, a: chart::Axis| {
            if value {
                c.options.value_axis = a
            } else {
                c.options.category_axis = a
            }
        };
        let axis = pick(chart);
        {
            {
                let auto_range = axis.range.is_none();
                let label = if value { "Auto range" } else { "Auto X range" };
                if ui::switch_row(ui, label, auto_range, crate::settings::SWITCH_H).clicked() {
                    // Turned off, each chart starts at the extent it draws now.
                    let doc = &self.session.doc;
                    let mut ops = Vec::new();
                    let others = self
                        .chart_others
                        .iter()
                        .filter_map(|n| Some((*n, doc.get(*n).and_then(|x| x.chart())?.clone())));
                    for (n, own) in std::iter::once((id, chart.clone())).chain(others) {
                        let mut next = own.clone();
                        let mut a = pick(&next);
                        a.range = auto_range.then(|| self.drawn_extent(n, &own, value));
                        put(&mut next, a);
                        if doc.get(n).and_then(|x| x.chart()) != Some(&next) {
                            ops.push(Operation::SetChart {
                                id: n,
                                chart: Some(Box::new(next)),
                            });
                        }
                    }
                    if !ops.is_empty() {
                        self.session.name_step(label, icon::CHART);
                        self.commit_edit(Transaction(ops));
                    }
                }
                if let Some((lo, hi)) = axis.range {
                    let step = ((hi - lo).abs() / 100.0).max(0.01);
                    let places = chart::scale::decimals_of(step).min(3);
                    let decimals = usize::from(places);
                    let scrub = Scrub::fine(step, u32::from(places));
                    self.chart_pair(ui, |app, ui, w| {
                        app.chart_number(
                            ui,
                            id,
                            chart,
                            w,
                            "Min",
                            if value { "Axis minimum" } else { "X minimum" },
                            scrub.clone(),
                            decimals,
                            move |c| pick(c).range.map_or(lo, |r| r.0),
                            move |c, v| {
                                let mut a = pick(c);
                                if let Some(r) = a.range.as_mut().filter(|r| v < r.1) {
                                    r.0 = v;
                                }
                                put(c, a);
                            },
                        );
                        app.chart_number(
                            ui,
                            id,
                            chart,
                            w,
                            "Max",
                            if value { "Axis maximum" } else { "X maximum" },
                            scrub,
                            decimals,
                            move |c| pick(c).range.map_or(hi, |r| r.1),
                            move |c, v| {
                                let mut a = pick(c);
                                if let Some(r) = a.range.as_mut().filter(|r| v > r.0) {
                                    r.1 = v;
                                }
                                put(c, a);
                            },
                        );
                    });
                }
            }
        }
    }

    /// An axis's title, typed; committed when the field lets go.
    fn chart_axis_title(
        &mut self,
        ui: &mut egui::Ui,
        id: NodeId,
        chart: &Chart,
        value: bool,
        label: &'static str,
    ) {
        let width = ui.available_width();
        self.chart_text(
            ui,
            id,
            chart,
            ("chart-axis-title", value),
            width,
            "Title",
            &format!("{label} title"),
            "Axis title",
            move |c| {
                Some(if value {
                    &mut c.options.value_axis.title
                } else {
                    &mut c.options.category_axis.title
                })
            },
        );
    }

    /// A text option typed into a field `width` wide, committed as `label` when
    /// the field lets go; `slot` finds the text in a description.
    #[allow(clippy::too_many_arguments)]
    fn chart_text(
        &mut self,
        ui: &mut egui::Ui,
        id: NodeId,
        chart: &Chart,
        salt: impl Hash + std::fmt::Debug,
        width: f32,
        hint: &str,
        tip: &str,
        label: &'static str,
        slot: impl Fn(&mut Chart) -> Option<&mut String>,
    ) {
        let mut probe = chart.clone();
        // A chart without the slot (a reference gone) has nothing to edit.
        let Some(stored) = slot(&mut probe).cloned() else {
            return;
        };
        let key = egui::Id::new(("chart-text", id, salt));
        let mut text = ui
            .data(|d| d.get_temp::<String>(key))
            .unwrap_or_else(|| stored.clone());
        let resp = ui::text_field(ui, egui::vec2(width, ui::CONTROL_H), &mut text, hint)
            .on_hover_text(tip);
        if resp.has_focus() {
            ui.data_mut(|d| d.insert_temp(key, text.clone()));
        } else {
            ui.data_mut(|d| d.remove::<String>(key));
        }
        if ui::defocus_commits(&resp) && text != stored {
            let text: String = text.chars().take(chart::MAX_TEXT_CHARS).collect();
            self.chart_each(id, chart, label, |c| {
                if let Some(s) = slot(c) {
                    *s = text.clone();
                }
            });
        }
    }

    /// The *Format* dropdown over one number format: closed, a sample written
    /// through it; open, its prefix and suffix, decimals, and the three
    /// switches, the same rows whatever is set so the popup keeps its size.
    fn chart_format(
        &mut self,
        ui: &mut egui::Ui,
        id: NodeId,
        chart: &Chart,
        salt: &'static str,
        slot: fn(&mut Chart) -> &mut chart::NumberFormat,
    ) {
        let mut probe = chart.clone();
        let f = slot(&mut probe).clone();
        let sample = chart::scale::format(FORMAT_SAMPLE, &f, 1);
        ui::labelled(ui, "Format", |ui| {
            ui::combo_height(ui);
            egui::ComboBox::from_id_salt((salt, id))
                .icon(ui::combo_chevron)
                .width(ui.available_width())
                // The popup holds text fields, so a click inside must not dismiss it.
                .close_behavior(egui::PopupCloseBehavior::CloseOnClickOutside)
                .selected_text(ui::combo_text(&sample))
                .show_ui(ui, |ui| {
                    // As wide as the dropdown, so the rows reach its right edge.
                    ui.set_width(ui.available_width().max(FORMAT_POPUP_W));
                    ui.spacing_mut().item_spacing.y = ui::CARD_ROW_GAP;
                    let half = (ui.available_width() - ui::CARD_COL_GAP) / 2.0;
                    ui.horizontal(|ui| {
                        ui.spacing_mut().item_spacing.x = ui::CARD_COL_GAP;
                        self.chart_text(
                            ui,
                            id,
                            chart,
                            (salt, "prefix"),
                            half,
                            "Prefix",
                            "Written before the number, as $",
                            "Number prefix",
                            move |c| Some(&mut slot(c).prefix),
                        );
                        self.chart_text(
                            ui,
                            id,
                            chart,
                            (salt, "suffix"),
                            half,
                            "Suffix",
                            "Written after the number, as kg",
                            "Number suffix",
                            move |c| Some(&mut slot(c).suffix),
                        );
                    });
                    ui.label(ui::eyebrow("Decimals"));
                    self.chart_segments_of(
                        ui,
                        id,
                        chart,
                        "Decimals",
                        &[
                            (None, "Auto"),
                            (Some(0), "0"),
                            (Some(1), "1"),
                            (Some(2), "2"),
                            (Some(3), "3"),
                        ],
                        f.decimals,
                        move |c, v| slot(c).decimals = v,
                    );
                    for (name, on, set) in [
                        (
                            "Thousands separator",
                            f.thousands,
                            (|f, on| f.thousands = on) as fn(&mut chart::NumberFormat, bool),
                        ),
                        ("Compact", f.compact, |f, on| f.compact = on),
                        ("Percent", f.percent, |f, on| f.percent = on),
                    ] {
                        self.chart_switch(ui, id, chart, name, on, move |c, on| {
                            set(slot(c), on);
                        });
                    }
                });
        });
    }

    fn chart_value_rows(&mut self, ui: &mut egui::Ui, id: NodeId, chart: &Chart) {
        if chart.kind == ChartKind::Pie {
            let off = chart.options.values.position == ValuePosition::Off;
            self.chart_switch(ui, id, chart, "Slice labels", !off, |c, on| {
                c.options.values.position = if on {
                    ValuePosition::Inside
                } else {
                    ValuePosition::Off
                };
            });
            if !off {
                self.chart_segments_of(
                    ui,
                    id,
                    chart,
                    "Slice label",
                    &[
                        (PieLabel::Value, "Value"),
                        (PieLabel::Percent, "Percent"),
                        (PieLabel::Category, "Category"),
                    ],
                    chart.options.values.pie,
                    |c, v| c.options.values.pie = v,
                );
                let outside = chart.options.values.pie_outside;
                self.chart_switch(
                    ui,
                    id,
                    chart,
                    "Outside with a leader line",
                    outside,
                    |c, on| {
                        c.options.values.pie_outside = on;
                    },
                );
            }
        } else {
            self.chart_segments_of(
                ui,
                id,
                chart,
                "Value labels",
                &[
                    (ValuePosition::Off, "Off"),
                    (ValuePosition::Inside, "Inside"),
                    (ValuePosition::Outside, "Outside"),
                    (ValuePosition::End, "End"),
                ],
                chart.options.values.position,
                |c, v| c.options.values.position = v,
            );
        }
        if matches!(chart.kind, ChartKind::Column | ChartKind::Bar) {
            let live = match chart.options.bars.stacking {
                chart::Stacking::Stacked => Ok(()),
                chart::Stacking::Percent => Err("Every stack of a 100% chart totals 100%"),
                chart::Stacking::Apart => Err("Only stacked bars have a total"),
            };
            let on = chart.options.values.totals;
            self.chart_switch_if(ui, live, id, chart, "Stack totals", on, |c, on| {
                c.options.values.totals = on;
            });
        }
        // Offered while some label can sit inside its mark.
        let v = &chart.options.values;
        let inside = match chart.kind {
            ChartKind::Pie => v.position != ValuePosition::Off && !v.pie_outside,
            ChartKind::Column | ChartKind::Bar => {
                v.position == ValuePosition::Inside
                    || v.position == ValuePosition::End
                    || (v.position == ValuePosition::Outside
                        && chart.options.bars.stacking != chart::Stacking::Apart)
            }
            _ => false,
        };
        if inside {
            ui::labelled(ui, "Inside ink", |ui| {
                self.chart_segments_of(
                    ui,
                    id,
                    chart,
                    "Inside ink",
                    &[
                        (chart::InsideInk::Auto, "Auto"),
                        (chart::InsideInk::Text, "Text"),
                    ],
                    chart.options.values.inside_ink,
                    |c, v| c.options.values.inside_ink = v,
                );
            });
        }
        self.chart_format(ui, id, chart, "chart-values-format", |c| {
            &mut c.options.values.format
        });
    }

    fn chart_legend_rows(&mut self, ui: &mut egui::Ui, id: NodeId, chart: &Chart) {
        self.chart_segments_of(
            ui,
            id,
            chart,
            "Legend position",
            &[
                (LegendPosition::Off, "Off"),
                (LegendPosition::Top, "Top"),
                (LegendPosition::Bottom, "Bottom"),
                (LegendPosition::Left, "Left"),
                (LegendPosition::Right, "Right"),
            ],
            chart.options.legend.position,
            |c, v| c.options.legend.position = v,
        );
        if chart.options.legend.position != LegendPosition::Off {
            self.chart_segments_of(
                ui,
                id,
                chart,
                "Legend alignment",
                &[
                    (Align::Start, "Start"),
                    (Align::Centre, "Centre"),
                    (Align::End, "End"),
                ],
                chart.options.legend.align,
                |c, v| c.options.legend.align = v,
            );
        }
        let lined = matches!(chart.kind, ChartKind::Line | ChartKind::Area);
        let ends = chart.options.legend.line_ends;
        self.chart_switch_if(
            ui,
            if lined {
                Ok(())
            } else {
                Err("Only lines and areas end where a label can sit")
            },
            id,
            chart,
            "Label line ends",
            ends,
            |c, on| c.options.legend.line_ends = on,
        );
        if lined && ends {
            self.chart_segments_of(
                ui,
                id,
                chart,
                "Line end label",
                &[
                    (chart::EndLabel::Name, "Name"),
                    (chart::EndLabel::NameValue, "Name, value"),
                ],
                chart.options.legend.end_label,
                |c, v| c.options.legend.end_label = v,
            );
            self.chart_segments_of(
                ui,
                id,
                chart,
                "Line end ink",
                &[
                    (chart::EndInk::Series, "Series"),
                    (chart::EndInk::Text, "Text"),
                ],
                chart.options.legend.end_ink,
                |c, v| c.options.legend.end_ink = v,
            );
        }
        if chart.options.legend.position != LegendPosition::Off {
            ui.label(ui::eyebrow("With a legend box"));
            let shapes: &[(chart::Swatch, &str)] = if lined {
                &[
                    (chart::Swatch::Square, "Square"),
                    (chart::Swatch::Circle, "Circle"),
                    (chart::Swatch::Line, "Line"),
                ]
            } else {
                &[
                    (chart::Swatch::Square, "Square"),
                    (chart::Swatch::Circle, "Circle"),
                ]
            };
            self.chart_segments_of(
                ui,
                id,
                chart,
                "Legend swatch",
                shapes,
                chart.options.legend.swatch,
                |c, v| c.options.legend.swatch = v,
            );
        }
    }

    fn chart_text_rows(&mut self, ui: &mut egui::Ui, id: NodeId, chart: &Chart) {
        // Empty is the document's default family, which the chart is drawn in.
        let shown = match chart.options.text.family.as_str() {
            "" => mastik_core::typography::TextStyle::default().font_family,
            f => f.to_string(),
        };
        let missing = self.fonts.family_status(&shown) == crate::fonts::FamilyStatus::Missing;
        let head = if missing {
            ui::glyph_and_text_tinted(icon::TEXT_AA, &shown, color::warn())
        } else {
            ui::glyph_and_text(icon::TEXT_AA, &shown)
        };
        let (picked, combo) = self.family_dropdown(
            ui,
            ("chart-family", id),
            egui::vec2(ui.available_width(), ui::CONTROL_H),
            head,
            &shown,
        );
        if missing {
            combo.on_hover_text(
                "This font is not installed and cannot be downloaded — \
                 the text is drawn in the fallback face",
            );
        }
        // The family shown picked again still brings the other charts to it.
        // A chart drawn in it already, by name or as the default, keeps its own.
        if let Some(family) = picked {
            self.fonts.ensure_loaded(&family);
            let default = mastik_core::typography::TextStyle::default().font_family;
            self.chart_each(id, chart, "Chart font", |c| {
                let drawn = match c.options.text.family.as_str() {
                    "" => default.as_str(),
                    f => f,
                };
                if drawn != family {
                    c.options.text.family = family.clone();
                }
            });
        }
        self.chart_slot_row(ui, id, chart, ChartColour::Ink, "Text colour");
        self.chart_pair(ui, |app, ui, w| {
            app.chart_number(
                ui,
                id,
                chart,
                w,
                "Ticks",
                "Tick label size",
                Scrub::fine(0.1, 1).range(1.0..=200.0),
                1,
                |c| c.options.text.tick_size,
                |c, v| c.options.text.tick_size = v,
            );
            app.chart_number(
                ui,
                id,
                chart,
                w,
                "Values",
                "Value label size",
                Scrub::fine(0.1, 1).range(1.0..=200.0),
                1,
                |c| c.options.text.value_size,
                |c, v| c.options.text.value_size = v,
            );
        });
        self.chart_pair(ui, |app, ui, w| {
            app.chart_number(
                ui,
                id,
                chart,
                w,
                "Legend",
                "Legend size",
                Scrub::fine(0.1, 1).range(1.0..=200.0),
                1,
                |c| c.options.text.legend_size,
                |c, v| c.options.text.legend_size = v,
            );
            app.chart_number(
                ui,
                id,
                chart,
                w,
                "Titles",
                "Axis title size",
                Scrub::fine(0.1, 1).range(1.0..=200.0),
                1,
                |c| c.options.text.title_size,
                |c, v| c.options.text.title_size = v,
            );
        });
        // The weight is picked from the family's upright faces, as the Type
        // card's variant dropdown picks a text's; a family the app cannot read
        // faces from keeps the number.
        let mut faces: Vec<(u16, String)> = Vec::new();
        for v in mastik_core::text::family_variants(&shown) {
            if !v.italic && !faces.iter().any(|(w, _)| *w == v.weight) {
                faces.push((v.weight, v.name));
            }
        }
        if faces.is_empty() {
            self.chart_number(
                ui,
                id,
                chart,
                ui.available_width(),
                "Weight",
                "Chart text weight",
                Scrub::whole(2.0).range(1.0..=1000.0),
                0,
                |c| f64::from(c.options.text.weight),
                |c, v| c.options.text.weight = v.round().clamp(1.0, 1000.0) as u16,
            );
        } else {
            let now = chart.options.text.weight;
            let mut options: Vec<(u16, &str)> =
                faces.iter().map(|(w, n)| (*w, n.as_str())).collect();
            // A weight no face has reads *Custom*, as the Type card's does.
            if !faces.iter().any(|(w, _)| *w == now) {
                options.push((now, "Custom"));
            }
            self.chart_menu_of(
                ui,
                id,
                chart,
                "chart-weight",
                "Weight",
                &options,
                now,
                |c, w| c.options.text.weight = w,
            );
        }
        // Each kind of label's own ink and weight, the chart's while unset.
        ui.label(ui::eyebrow("By role"));
        for (r, (name, ..)) in ROLE_STEPS.into_iter().enumerate() {
            let r = r as u8;
            let slot = ChartColour::Role(r);
            let own = role_of(chart, r);
            let half = (ui.available_width() - ui::CARD_COL_GAP) / 2.0;
            let swatch =
                colour_of(&self.session.doc, id, chart, slot).map_or(ui::Swatch::Mixed, |c| {
                    // Inheriting, the chart's ink shows faint.
                    let c = ui::color_to_egui(c);
                    ui::Swatch::Solid(if own.ink.is_some() {
                        c
                    } else {
                        c.gamma_multiply(0.55)
                    })
                });
            let mut pick: Option<Option<u16>> = None;
            let mut reset = false;
            ui.horizontal(|ui| {
                ui.spacing_mut().item_spacing.x = ui::CARD_COL_GAP;
                let w = if own.ink.is_some() {
                    half - ui::CONTROL_H - ui::CARD_COL_GAP
                } else {
                    half
                };
                self.chart_colour_row(ui, id, slot, w, swatch, name, true, None);
                if own.ink.is_some() {
                    reset = ui::field_button(
                        ui,
                        icon::ARROW_COUNTER_CLOCKWISE,
                        ui::CONTROL_H,
                        ui::FieldButton::Off,
                    )
                    .on_hover_text("Use the chart's colour")
                    .clicked();
                }
                let shown = own.weight.map_or_else(
                    || "Chart's".to_string(),
                    |w| {
                        faces
                            .iter()
                            .find(|(fw, _)| *fw == w)
                            .map_or_else(|| w.to_string(), |(_, n)| n.clone())
                    },
                );
                ui.scope(|ui| {
                    ui::combo_height(ui);
                    egui::ComboBox::from_id_salt(("chart-role-weight", id, r))
                        .icon(ui::combo_chevron)
                        .width(ui.available_width())
                        .selected_text(ui::combo_text(&shown))
                        .show_ui(ui, |ui| {
                            ui::menu_rows(ui);
                            if ui
                                .selectable_label(own.weight.is_none(), "Chart's")
                                .clicked()
                            {
                                pick = Some(None);
                            }
                            for (w, n) in &faces {
                                if ui.selectable_label(own.weight == Some(*w), n).clicked() {
                                    pick = Some(Some(*w));
                                }
                            }
                        })
                        .response
                        .on_hover_text(format!("{name} weight"));
                });
            });
            if reset {
                self.chart_each(id, chart, ROLE_STEPS[usize::from(r)].1, |c| {
                    role_of_mut(c, r).ink = None;
                });
            }
            if let Some(w) = pick {
                self.chart_each(id, chart, ROLE_STEPS[usize::from(r)].2, |c| {
                    role_of_mut(c, r).weight = w;
                });
            }
        }
    }

    /// *Flatten chart*: each chart keeps its parts and drops its description, one
    /// undo step (`chart::flatten_ops`).
    pub(crate) fn flatten_charts(&mut self, ids: &[NodeId]) {
        let each: Vec<Vec<Operation>> = ids
            .iter()
            .filter_map(|id| chart::flatten_ops(&self.session.doc, *id))
            .collect();
        let n = each.len();
        if n == 0 {
            return;
        }
        let ops: Vec<Operation> = each.into_iter().flatten().collect();
        self.session.name_step("Flatten chart", icon::STACK_SIMPLE);
        if self.commit_edit(Transaction(ops)) {
            self.session.info(match n {
                1 => "Flattened the chart; its shapes are layers now".to_string(),
                n => format!("Flattened {n} charts; their shapes are layers now"),
            });
        }
    }
}

/// The colour variables a chart can take *From variables*, by group (the name
/// before the last `/`, else the collection's name), each group's variables in
/// the table's order: those that may bind a fill.
fn colour_groups(doc: &mastik_core::Document) -> Vec<(String, Vec<mastik_core::VarId>)> {
    use mastik_core::variable::{Field, VarKind, split_name};
    let t = doc.variables();
    let mut out: Vec<(String, Vec<mastik_core::VarId>)> = Vec::new();
    for v in t
        .iter()
        .filter(|v| v.kind == VarKind::Color && v.binds(Field::Fill))
    {
        let (group, _) = split_name(&v.name);
        let name = match group {
            "" => t
                .collection_of(v.id)
                .map_or_else(|| "Colours".to_string(), |c| c.name.clone()),
            g => g.to_string(),
        };
        match out.iter_mut().find(|(n, _)| *n == name) {
            Some((_, vars)) => vars.push(v.id),
            None => out.push((name, vec![v.id])),
        }
    }
    out
}

/// The range an axis starts at when *Auto range* is turned off: its data's
/// extent on nice steps, with zero where the kind or *Start at zero* holds it,
/// so the chart looks as it did until a bound is typed. `value` is the value
/// axis; the other is a scatter's X.
fn auto_extent(chart: &Chart, value: bool) -> (f64, f64) {
    let t = &chart.table;
    let numbers = |col: chart::ColumnId| -> Vec<Option<f64>> {
        t.index_of(col).map_or_else(Vec::new, |i| {
            t.rows.iter().map(|r| chart::parse_number(&r[i])).collect()
        })
    };
    let shown: Vec<chart::ColumnId> = chart
        .mapping
        .series
        .iter()
        .copied()
        .filter(|c| {
            chart
                .series
                .iter()
                .find(|s| s.column == *c)
                .is_none_or(|s| s.visible)
        })
        .collect();
    let o = &chart.options;
    let stacking = match chart.kind {
        ChartKind::Column | ChartKind::Bar => o.bars.stacking,
        ChartKind::Area => o.lines.area_stacking,
        _ => Stacking::Apart,
    };
    let mut seen: Vec<f64> = Vec::new();
    if !value {
        seen.extend(
            chart
                .mapping
                .category
                .map_or_else(Vec::new, numbers)
                .into_iter()
                .flatten(),
        );
    } else if stacking == Stacking::Percent {
        seen.extend([0.0, 1.0]);
    } else if stacking == Stacking::Stacked {
        let mut ups = vec![0.0; t.rows.len()];
        let mut downs = vec![0.0; t.rows.len()];
        for col in &shown {
            for (r, v) in numbers(*col).into_iter().enumerate() {
                match v {
                    Some(v) if v >= 0.0 => ups[r] += v,
                    Some(v) => downs[r] += v,
                    None => {}
                }
            }
        }
        seen.extend(ups.into_iter().chain(downs));
    } else {
        for col in &shown {
            seen.extend(numbers(*col).into_iter().flatten());
        }
    }
    let axis = if value {
        &o.value_axis
    } else {
        &o.category_axis
    };
    let zero = axis.start_at_zero
        || (value && !matches!(chart.kind, ChartKind::Line | ChartKind::Scatter));
    if zero {
        seen.push(0.0);
    }
    let lo = seen.iter().copied().fold(f64::INFINITY, f64::min);
    let hi = seen.iter().copied().fold(f64::NEG_INFINITY, f64::max);
    let ticks = chart::scale::ticks(lo, hi, axis.tick_count.unwrap_or(5), None);
    (ticks.min, ticks.max)
}

/// The value axis's and the category axis's eyebrows, as `kind` draws them.
fn axis_titles(kind: ChartKind) -> (&'static str, &'static str) {
    match kind {
        ChartKind::Bar => ("Value axis (bottom)", "Category axis (left)"),
        ChartKind::Scatter => ("Y axis", "X axis"),
        _ => ("Value axis", "Category axis"),
    }
}

/// Series `col` shown or hidden: a shown column not yet mapped joins the
/// series; a hidden one keeps its place and its colour.
fn show_series(chart: &mut Chart, col: chart::ColumnId, on: bool) {
    if on && !chart.mapping.series.contains(&col) {
        chart.mapping.series.push(col);
    }
    if let Some(s) = chart.series.iter_mut().find(|s| s.column == col) {
        s.visible = on;
    } else if !on {
        *chart = chart::with_series_fill(chart, col, None);
        if let Some(s) = chart.series.iter_mut().find(|s| s.column == col) {
            s.visible = false;
        }
    }
}

/// A Style cell: the preset's picture over its name on a [`ui::tile_ground`],
/// the lit one in the accent.
fn style_cell(
    ui: &mut egui::Ui,
    width: f32,
    name: &str,
    picture: Option<egui::TextureId>,
    on: bool,
) -> egui::Response {
    let (rect, resp) =
        ui.allocate_exact_size(egui::vec2(width, STYLE_CELL_H), egui::Sense::click());
    let p = ui.painter();
    ui::tile_ground(p, rect, on, resp.hovered());
    // On whole device pixels, at exactly the texture's size, so no texel is
    // sampled between two pixels.
    let ppp = ui.ctx().pixels_per_point();
    let px = style_picture_px(width, ppp);
    let size = egui::vec2(px[0] as f32, px[1] as f32) / ppp;
    let snap = |v: f32| (v * ppp).round() / ppp;
    let min = egui::pos2(
        snap(rect.center().x - size.x / 2.0),
        snap(rect.top() + STYLE_TOP),
    );
    let frame = egui::Rect::from_min_size(min, size);
    if let Some(t) = picture {
        p.image(t, frame, crate::thumbs::FULL_UV, egui::Color32::WHITE);
    }
    let ink = if on {
        color::accent_200()
    } else {
        theme::text::muted()
    };
    p.text(
        egui::pos2(rect.center().x, rect.bottom() - 8.0),
        egui::Align2::CENTER_CENTER,
        name,
        egui::FontId::proportional(ui::SMALL_PT),
        ink,
    );
    resp
}

#[cfg(test)]
mod tests {
    use super::*;
    use mastik_core::kurbo::Affine;

    /// A chart on bare canvas, selected, in a headless app.
    fn charted(ctx: &egui::Context, kind: ChartKind) -> (MastikApp, NodeId) {
        crate::theme::install(ctx);
        let mut app = MastikApp::headless(ctx);
        let id = app.session.ids.mint();
        let page = app.session.doc.first_page();
        let index = app.session.doc.get(page).map_or(0, |n| n.children().len());
        let tx = mastik_core::edit::create_chart(
            id,
            page,
            index,
            Affine::IDENTITY,
            kurbo::Point::new(-4000.0, 0.0),
            kurbo::Point::new(-3520.0, 280.0),
            Some(mastik_core::peniko::Color::WHITE),
            Chart::sample(kind),
        );
        assert!(app.session.commit(tx));
        app.session.selection.set_one(id);
        (app, id)
    }

    fn pass(ctx: &egui::Context, app: &mut MastikApp) {
        let mut frame = eframe::Frame::_new_kittest();
        let input = egui::RawInput {
            screen_rect: Some(egui::Rect::from_min_size(
                egui::Pos2::ZERO,
                egui::vec2(1320.0, 820.0),
            )),
            ..Default::default()
        };
        let _ = ctx.run_ui(input, |ui| eframe::App::ui(app, ui, &mut frame));
    }

    /// A selected chart shows the Chart card in place of the frame's Container,
    /// Layout grid and Frame templates cards, with a picture in every Style cell
    /// and *Clean* the lit one on a new chart.
    /// flip: `inspector_single` drawing the Container card for a chart → fails at "no Container card".
    #[test]
    fn a_chart_shows_the_chart_card_and_not_the_frames() {
        let ctx = egui::Context::default();
        let (mut app, id) = charted(&ctx, ChartKind::Column);
        for _ in 0..3 {
            pass(&ctx, &mut app);
        }
        let shown = &app.cards_shown;
        assert!(shown.contains(&"Chart"), "the Chart card: {shown:?}");
        for frame_card in ["Container", "Layout grid", "Frame templates"] {
            assert!(
                !shown.contains(&frame_card),
                "no {frame_card} card: {shown:?}"
            );
        }
        let cache = app
            .chart_card
            .as_ref()
            .expect("the card measured its chart");
        assert_eq!(cache.id, id);
        assert_eq!(
            cache.thumbs.len(),
            Preset::ALL.len(),
            "a picture per Style cell"
        );
        assert!(cache.left_out.is_empty() && cache.refusal.is_none());
        let chart = app.session.doc.get(id).and_then(|n| n.chart()).unwrap();
        assert_eq!(Preset::of(chart.kind, &chart.options), Some(Preset::Clean));
    }

    /// A Style picture keeps its inset each side in a narrow cell, in whole
    /// pixels and its proportions at any scale, and never grows past its own
    /// size in a wide one.
    #[test]
    fn a_style_picture_keeps_its_inset() {
        for ppp in [1.0, 1.5, 2.0] {
            let [w, h] = style_picture_px(46.5, ppp);
            assert!(w as f32 / ppp + 2.0 * STYLE_INSET <= 46.5, "{w} at {ppp}");
            let want = (w as f32 * STYLE_PICTURE.y / STYLE_PICTURE.x).round();
            assert_eq!(h as f32, want, "whole pixels in proportion at {ppp}");
        }
        assert_eq!(style_picture_px(80.0, 1.0), [44, 32]);
    }

    /// A series colour written through the picker's slot reads back from the
    /// slot, and the commit leaves the other series on the palette.
    #[test]
    fn a_series_colour_goes_through_the_pickers_slot() {
        let ctx = egui::Context::default();
        let (mut app, id) = charted(&ctx, ChartKind::Line);
        let chart = app
            .session
            .doc
            .get(id)
            .and_then(|n| n.chart())
            .cloned()
            .unwrap();
        let (first, second) = (chart.mapping.series[0], chart.mapping.series[1]);
        let red = mastik_core::peniko::Color::from_rgb8(255, 0, 0);
        let slot = PaintSlot::Chart(ChartColour::Series(second));
        let tx = app.slot_transaction(id, slot, Brush::Solid(red));
        assert!(app.commit_edit(tx));
        assert_eq!(app.slot_brush(id, slot), Some(Brush::Solid(red)));
        let after = app.session.doc.get(id).and_then(|n| n.chart()).unwrap();
        assert_eq!(
            chart::series_fill(after, first),
            chart::series_fill(&chart, first),
            "the other series kept its colour"
        );
    }

    /// `Ctrl+E` on a selected chart flattens it: the frame keeps its parts as
    /// layers and loses its description, in one undo step.
    #[test]
    fn ctrl_e_flattens_a_chart() {
        let ctx = egui::Context::default();
        let (mut app, id) = charted(&ctx, ChartKind::Pie);
        let parts = app.session.doc.get(id).map_or(0, |n| n.children().len());
        app.flatten_selection();
        let node = app.session.doc.get(id).expect("the frame stays");
        assert!(node.chart().is_none(), "the description is gone");
        assert_eq!(node.children().len(), parts, "the parts stay");
        app.session.undo();
        assert!(app.session.doc.get(id).and_then(|n| n.chart()).is_some());
    }

    /// Two charts of one kind selected show one Chart card, and an edit through
    /// it writes both in one step, each keeping its own other options.
    /// flip: `chart_tx` skipping `chart_others` → fails at "the second chart took the radius".
    #[test]
    fn a_multi_selection_of_charts_edits_each() {
        let ctx = egui::Context::default();
        let (mut app, a) = charted(&ctx, ChartKind::Column);
        let b = app.session.ids.mint();
        let page = app.session.doc.first_page();
        let index = app.session.doc.get(page).map_or(0, |n| n.children().len());
        let mut own = Chart::sample(ChartKind::Column);
        own.options.bars.group_gap = 55.0;
        let tx = mastik_core::edit::create_chart(
            b,
            page,
            index,
            Affine::IDENTITY,
            kurbo::Point::new(-4000.0, 400.0),
            kurbo::Point::new(-3520.0, 680.0),
            Some(mastik_core::peniko::Color::WHITE),
            own,
        );
        assert!(app.session.commit(tx));
        app.session.selection.set_one(a);
        app.session.selection.add(b);
        for _ in 0..3 {
            pass(&ctx, &mut app);
        }
        assert!(app.cards_shown.contains(&"Chart"), "{:?}", app.cards_shown);
        assert!(!app.cards_shown.contains(&"Container"));
        assert_eq!(app.selected_charts(), Some(vec![a, b]));

        let lead = app
            .session
            .doc
            .get(a)
            .and_then(|n| n.chart())
            .cloned()
            .unwrap();
        let mut next = lead.clone();
        next.options.bars.radius = 9.0;
        app.chart_others = vec![b];
        app.chart_commit(a, "Bar radius", next);
        app.chart_others.clear();
        let chart = |app: &MastikApp, id| {
            app.session
                .doc
                .get(id)
                .and_then(|n| n.chart())
                .cloned()
                .unwrap()
        };
        assert_eq!(chart(&app, a).options.bars.radius, 9.0);
        assert_eq!(
            chart(&app, b).options.bars.radius,
            9.0,
            "the second chart took the radius"
        );
        assert_eq!(
            chart(&app, b).options.bars.group_gap,
            55.0,
            "and kept its own gap"
        );
        app.session.undo();
        assert_eq!(
            chart(&app, b).options.bars.radius,
            lead.options.bars.radius,
            "one step"
        );
    }

    /// *Auto range* turned off starts at the range the data already draws: zero
    /// to a nice step past the largest value, or past the largest stack.
    #[test]
    fn auto_range_off_starts_at_the_datas_extent() {
        let mut chart = Chart::sample(ChartKind::Column);
        let most = chart
            .table
            .rows
            .iter()
            .flat_map(|r| r.iter().filter_map(|c| chart::parse_number(c)))
            .fold(0.0, f64::max);
        let (lo, hi) = auto_extent(&chart, true);
        assert_eq!(lo, 0.0);
        assert!(hi >= most && hi < most * 2.0, "{hi} for {most}");
        chart.options.bars.stacking = Stacking::Stacked;
        let (_, stacked) = auto_extent(&chart, true);
        assert!(stacked > hi, "a stack reaches past one series: {stacked}");
    }

    /// *Auto range* off starts at the extent the chart draws at its size, the
    /// generator's: a chart too short for five value labels ticks fewer and
    /// wider, which a five-tick estimate would not match.
    /// flip: `drawn_extent` answering `auto_extent` → fails at "as drawn".
    #[test]
    fn auto_range_off_reads_the_drawn_extent() {
        let ctx = egui::Context::default();
        let (mut app, a) = charted(&ctx, ChartKind::Column);
        assert!(app.session.commit(Transaction(vec![Operation::SetGeometry {
            id: a,
            geometry: mastik_core::GeometryPatch::Size(kurbo::Size::new(480.0, 120.0)),
        }])));
        let chart = chart_at(&app, a);
        let family = mastik_core::typography::TextStyle::default().font_family;
        let drawn = chart::generate(&chart, kurbo::Size::new(480.0, 120.0), &family)
            .unwrap()
            .extents[0]
            .expect("a value axis");
        assert_ne!(drawn, auto_extent(&chart, true), "the fixture: fewer ticks");
        assert_eq!(app.drawn_extent(a, &chart, true), drawn, "as drawn");
    }

    /// The Type section's variants round-trip: each segment written reads back
    /// as itself.
    #[test]
    fn every_variant_reads_back_as_itself() {
        for kind in ChartKind::ALL {
            let chart = Chart::sample(kind);
            let Some((names, _)) = variants(&chart) else {
                continue;
            };
            for i in 0..names.len() {
                let next = with_variant(&chart, i);
                assert_eq!(variants(&next).map(|(_, at)| at), Some(i), "{kind:?} {i}");
            }
        }
    }

    /// One frame of the app, its shapes returned; tall enough that the whole
    /// card is on screen, since a label out of view is not painted.
    fn shapes(ctx: &egui::Context, app: &mut MastikApp) -> Vec<egui::epaint::ClippedShape> {
        let mut frame = eframe::Frame::_new_kittest();
        let input = egui::RawInput {
            screen_rect: Some(egui::Rect::from_min_size(
                egui::Pos2::ZERO,
                egui::vec2(1320.0, 4000.0),
            )),
            ..Default::default()
        };
        ctx.run_ui(input, |ui| eframe::App::ui(app, ui, &mut frame))
            .shapes
    }

    /// Where a text shape reading `s` is centred among `shapes`.
    fn text_at(shapes: &[egui::epaint::ClippedShape], s: &str) -> Option<egui::Pos2> {
        fn find(shape: &egui::Shape, s: &str) -> Option<egui::Pos2> {
            match shape {
                egui::Shape::Text(t) if t.galley.text() == s => {
                    Some(t.pos + t.galley.rect.center().to_vec2())
                }
                egui::Shape::Vec(v) => v.iter().find_map(|x| find(x, s)),
                _ => None,
            }
        }
        shapes.iter().find_map(|c| find(&c.shape, s))
    }

    /// A click at `at` in the app's frame of `shapes`: move, press, release.
    fn click(ctx: &egui::Context, app: &mut MastikApp, at: egui::Pos2) {
        let button = |pressed| egui::Event::PointerButton {
            pos: at,
            button: egui::PointerButton::Primary,
            pressed,
            modifiers: egui::Modifiers::NONE,
        };
        for events in [
            vec![egui::Event::PointerMoved(at)],
            vec![button(true)],
            vec![button(false)],
            vec![],
        ] {
            let mut frame = eframe::Frame::_new_kittest();
            let input = egui::RawInput {
                screen_rect: Some(egui::Rect::from_min_size(
                    egui::Pos2::ZERO,
                    egui::vec2(1320.0, 4000.0),
                )),
                events,
                ..Default::default()
            };
            let _ = ctx.run_ui(input, |ui| eframe::App::ui(app, ui, &mut frame));
        }
    }

    /// Whether a text shape reading `s` is among `shapes`.
    fn drawn(shapes: &[egui::epaint::ClippedShape], s: &str) -> bool {
        fn find(shape: &egui::Shape, s: &str) -> bool {
            match shape {
                egui::Shape::Text(t) => t.galley.text() == s,
                egui::Shape::Vec(v) => v.iter().any(|x| find(x, s)),
                _ => false,
            }
        }
        shapes.iter().any(|c| find(&c.shape, s))
    }

    /// A colour variable `name` holding `c` in a new mode of the first
    /// collection; returns it and the mode.
    fn colour_var(
        app: &mut MastikApp,
        name: &str,
        c: mastik_core::peniko::Color,
    ) -> (mastik_core::VarId, mastik_core::variable::ModeId) {
        use mastik_core::variable::{Cell, CollectionId, Mode, VarKind, Variable};
        let (mode, var) = (app.session.ids.mint_mode(), app.session.ids.mint_var());
        assert!(app.commit_edit(Transaction(vec![
            Operation::AddMode {
                collection: CollectionId::FIRST,
                mode: Mode {
                    id: mode,
                    name: "Light".into(),
                },
                index: 0,
                cells: vec![],
            },
            Operation::AddVariable {
                var: Variable {
                    id: var,
                    name: name.into(),
                    kind: VarKind::Color,
                    cells: [(mode, Cell::Color(c))].into(),
                    scope: None,
                    origin: None,
                    forked: None,
                    description: String::new(),
                },
            },
        ])));
        (var, mode)
    }

    /// Each axis and gridline slot writes its own colour and no other; the
    /// Sankey's neutral and the ring's track are the value axis's and the
    /// value gridlines'.
    /// flip: `with_fill` writing `Axis(false)` to the value axis → fails at "the category axis".
    #[test]
    fn axis_and_gridline_slots_write_their_own_colour() {
        let ctx = egui::Context::default();
        let (mut app, id) = charted(&ctx, ChartKind::Column);
        let red = mastik_core::peniko::Color::from_rgb8(255, 0, 0);
        let lit =
            |c: mastik_core::peniko::Color| -> mastik_core::BrushSource { Brush::Solid(c).into() };
        let before = app
            .session
            .doc
            .get(id)
            .and_then(|n| n.chart())
            .cloned()
            .unwrap();
        let tx = app.slot_transaction(
            id,
            PaintSlot::Chart(ChartColour::Axis(false)),
            Brush::Solid(red),
        );
        assert!(app.commit_edit(tx));
        let o = &app
            .session
            .doc
            .get(id)
            .and_then(|n| n.chart())
            .unwrap()
            .options;
        assert_eq!(o.category_axis.colour, Some(lit(red)), "the category axis");
        assert_eq!(o.value_axis.colour, before.options.value_axis.colour);
        assert_eq!(
            o.category_axis.grid.colour,
            before.options.category_axis.grid.colour
        );
        let blue = mastik_core::peniko::Color::from_rgb8(0, 0, 255);
        let tx = app.slot_transaction(
            id,
            PaintSlot::Chart(ChartColour::Grid(true)),
            Brush::Solid(blue),
        );
        assert!(app.commit_edit(tx));
        let chart = app.session.doc.get(id).and_then(|n| n.chart()).unwrap();
        assert_eq!(
            chart.options.value_axis.grid.colour,
            Some(lit(blue)),
            "the value gridlines"
        );
        assert_eq!(chart.options.category_axis.colour, Some(lit(red)), "kept");
        assert_eq!(fill_of(chart, ChartColour::Track), Some(lit(blue)));
        assert_eq!(
            fill_of(chart, ChartColour::Neutral),
            Some(chart.axis_colour(true))
        );
    }

    /// The Axes section offers an axis's colour while it draws a line or ticks
    /// and its gridlines' colour while they show; a Sankey on Neutral flows
    /// offers its neutral colour, and a progress ring its track's.
    /// flip: `chart_axis_rows` dropping the gridline colour row → fails at "Value gridline colour".
    #[test]
    fn the_card_offers_axis_gridline_neutral_and_track_colours() {
        let ctx = egui::Context::default();
        let (mut app, id) = charted(&ctx, ChartKind::Column);
        app.collapsed_panels.remove("Chart axes");
        app.collapsed_panels.remove("Chart marks");
        let mut out = Vec::new();
        for _ in 0..3 {
            out = shapes(&ctx, &mut app);
        }
        for row in ["Value gridline colour", "Category axis colour"] {
            assert!(drawn(&out, row), "{row}");
        }
        // A new chart's value axis has no line or ticks, its category axis no gridlines.
        for row in [
            "Value axis colour",
            "Category gridline colour",
            "Track colour",
            "Neutral colour",
        ] {
            assert!(!drawn(&out, row), "no {row}");
        }
        let mut chart = app
            .session
            .doc
            .get(id)
            .and_then(|n| n.chart())
            .cloned()
            .unwrap();
        chart.options.value_axis.ticks = true;
        app.chart_commit(id, "Value tick marks", chart);
        for _ in 0..3 {
            out = shapes(&ctx, &mut app);
        }
        assert!(drawn(&out, "Value axis colour"), "the ticks' colour");

        let (mut app, id) = charted(&ctx, ChartKind::Sankey);
        app.collapsed_panels.remove("Chart marks");
        let mut chart = app
            .session
            .doc
            .get(id)
            .and_then(|n| n.chart())
            .cloned()
            .unwrap();
        chart.options.flows.colour = FlowColour::Neutral;
        app.chart_commit(id, "Flow colour", chart);
        for _ in 0..3 {
            out = shapes(&ctx, &mut app);
        }
        assert!(drawn(&out, "Neutral colour"), "Neutral colour");

        let (mut app, id) = charted(&ctx, ChartKind::Pie);
        app.collapsed_panels.remove("Chart marks");
        for _ in 0..3 {
            out = shapes(&ctx, &mut app);
        }
        assert!(
            !drawn(&out, "Track colour"),
            "a pie of several slices has no track"
        );
        let mut chart = app
            .session
            .doc
            .get(id)
            .and_then(|n| n.chart())
            .cloned()
            .unwrap();
        chart.table.rows.truncate(1);
        chart.table.rows[0][1] = "64".into();
        app.chart_commit(id, "Edit chart data", chart);
        for _ in 0..3 {
            out = shapes(&ctx, &mut app);
        }
        assert!(drawn(&out, "Track colour"), "a ring's track");
    }

    /// A bound series row names its variable in a capsule beside the series name.
    /// flip: `chart_colour_row` skipping the capsule → fails at "the variable's name".
    #[test]
    fn a_bound_series_row_names_its_variable() {
        let ctx = egui::Context::default();
        let (mut app, id) = charted(&ctx, ChartKind::Column);
        let (var, _) = colour_var(&mut app, "brand/500", mastik_core::peniko::Color::WHITE);
        let col = app
            .session
            .doc
            .get(id)
            .and_then(|n| n.chart())
            .unwrap()
            .mapping
            .series[0];
        let tx = app.chart_bind_tx(id, ChartColour::Series(col), Some(var), None);
        assert!(app.commit_edit(tx));
        assert_eq!(
            app.chart_binding(id, ChartColour::Series(col)).map(|b| b.0),
            Some(var)
        );
        let mut out = Vec::new();
        for _ in 0..3 {
            out = shapes(&ctx, &mut app);
        }
        assert!(drawn(&out, "brand/500"), "the variable's name");
    }

    /// The Style cells redraw when a variable a series is bound to changes value.
    /// flip: `refresh_chart_cache` keying on the description as written, not as
    /// resolved → fails at "redrawn".
    #[test]
    fn style_pictures_follow_a_bound_variables_value() {
        let ctx = egui::Context::default();
        let (mut app, id) = charted(&ctx, ChartKind::Column);
        let (var, mode) = colour_var(&mut app, "brand/500", mastik_core::peniko::Color::WHITE);
        let col = app
            .session
            .doc
            .get(id)
            .and_then(|n| n.chart())
            .unwrap()
            .mapping
            .series[0];
        let tx = app.chart_bind_tx(id, ChartColour::Series(col), Some(var), None);
        assert!(app.commit_edit(tx));
        for _ in 0..3 {
            shapes(&ctx, &mut app);
        }
        let key = |app: &MastikApp| app.chart_card.as_ref().map(|c| c.key);
        let was = key(&app);
        assert!(was.is_some(), "the card measured its chart");
        let red = mastik_core::peniko::Color::from_rgb8(255, 0, 0);
        assert!(
            app.commit_edit(Transaction(vec![Operation::SetVariableCell {
                id: var,
                mode,
                cell: mastik_core::variable::Cell::Color(red),
            }]))
        );
        for _ in 0..3 {
            shapes(&ctx, &mut app);
        }
        assert_ne!(key(&app), was, "redrawn");
    }

    /// Two column charts, both selected, the first `a`.
    fn two_charts(ctx: &egui::Context) -> (MastikApp, NodeId, NodeId) {
        let (mut app, a) = charted(ctx, ChartKind::Column);
        let b = app.session.ids.mint();
        let page = app.session.doc.first_page();
        let index = app.session.doc.get(page).map_or(0, |n| n.children().len());
        let tx = mastik_core::edit::create_chart(
            b,
            page,
            index,
            Affine::IDENTITY,
            kurbo::Point::new(-4000.0, 400.0),
            kurbo::Point::new(-3520.0, 680.0),
            Some(mastik_core::peniko::Color::WHITE),
            Chart::sample(ChartKind::Column),
        );
        assert!(app.session.commit(tx));
        app.session.selection.set_one(a);
        app.session.selection.add(b);
        (app, a, b)
    }

    fn chart_at(app: &MastikApp, id: NodeId) -> Chart {
        app.session
            .doc
            .get(id)
            .and_then(|n| n.chart())
            .cloned()
            .unwrap()
    }

    /// A chart-wide colour picked over two selected charts writes both, a
    /// series colour only the chart the card shows.
    /// flip: `chart_slot_targets` answering only `id` → fails at "the second chart took the gridlines".
    #[test]
    fn a_chart_colour_over_two_charts_writes_both() {
        let ctx = egui::Context::default();
        let (mut app, a, b) = two_charts(&ctx);
        let red = mastik_core::peniko::Color::from_rgb8(255, 0, 0);
        let slot = PaintSlot::Chart(ChartColour::Grid(true));
        let tx = app.slot_transaction(a, slot, Brush::Solid(red));
        assert!(app.commit_edit(tx));
        let lit: mastik_core::BrushSource = Brush::Solid(red).into();
        assert_eq!(
            chart_at(&app, a).options.value_axis.grid.colour,
            Some(lit.clone())
        );
        assert_eq!(
            chart_at(&app, b).options.value_axis.grid.colour,
            Some(lit.clone()),
            "the second chart took the gridlines"
        );
        let col = chart_at(&app, a).mapping.series[0];
        let before = chart::series_fill(&chart_at(&app, b), col);
        let tx = app.slot_transaction(
            a,
            PaintSlot::Chart(ChartColour::Series(col)),
            Brush::Solid(red),
        );
        assert!(app.commit_edit(tx));
        assert_eq!(chart::series_fill(&chart_at(&app, a), col), Some(lit));
        assert_eq!(
            chart::series_fill(&chart_at(&app, b), col),
            before,
            "a series colour stays the first chart's"
        );
    }

    /// Picking the lit segment over two charts that disagree writes the other
    /// chart, in one step.
    /// flip: `chart_each` returning when the first chart is unchanged → fails at
    /// "the second chart stacks".
    #[test]
    fn picking_the_lit_segment_brings_the_others_to_it() {
        let ctx = egui::Context::default();
        let (mut app, a, b) = two_charts(&ctx);
        let mut stacked = chart_at(&app, a);
        stacked.options.bars.stacking = Stacking::Stacked;
        app.chart_commit(a, "Chart type", stacked.clone());
        assert_eq!(chart_at(&app, b).options.bars.stacking, Stacking::Apart);
        app.chart_others = vec![b];
        app.chart_pick(a, &stacked, "Chart type", 1, |c, i| {
            *c = with_variant(c, i);
        });
        app.chart_others.clear();
        assert_eq!(
            chart_at(&app, b).options.bars.stacking,
            Stacking::Stacked,
            "the second chart stacks"
        );
        assert_eq!(chart_at(&app, a).options.bars.stacking, Stacking::Stacked);
        app.session.undo();
        assert_eq!(
            chart_at(&app, b).options.bars.stacking,
            Stacking::Apart,
            "one step"
        );
        assert_eq!(chart_at(&app, a).options.bars.stacking, Stacking::Stacked);
    }

    /// Over two charts each takes an edit on its own description: *Auto range*
    /// off at its own extent, a *Min* keeping its own *Max*, and a preset whole.
    /// flip: `chart_each_tx` writing the others the first chart's options diff
    /// (`Options::carried`) → fails at "B's own extent".
    #[test]
    fn several_charts_each_take_an_edit_on_their_own() {
        let ctx = egui::Context::default();
        let (mut app, a, b) = two_charts(&ctx);
        let mut big = chart_at(&app, b);
        for row in &mut big.table.rows {
            for cell in row.iter_mut().skip(1) {
                *cell = format!("{}00", cell);
            }
        }
        big.options.stroke.width = chart::OUTLINE_WIDTH;
        app.chart_commit(b, "Edit chart data", big);
        app.chart_others = vec![b];
        let first = chart_at(&app, a);
        app.chart_each(a, &first, "Auto range", |c| {
            c.options.value_axis.range = Some(auto_extent(c, true));
        });
        let own = auto_extent(&chart_at(&app, b), true);
        assert_ne!(own, auto_extent(&chart_at(&app, a), true), "fixture");
        assert_eq!(
            chart_at(&app, b).options.value_axis.range,
            Some(own),
            "B's own extent"
        );
        let first = chart_at(&app, a);
        app.chart_each(a, &first, "Axis minimum", |c| {
            if let Some(r) = c.options.value_axis.range.as_mut().filter(|r| 1.0 < r.1) {
                r.0 = 1.0;
            }
        });
        assert_eq!(
            chart_at(&app, b).options.value_axis.range,
            Some((1.0, own.1)),
            "B keeps its maximum"
        );
        let lit = |c: Chart| Preset::of(c.kind, &c.options);
        assert_ne!(
            lit(chart_at(&app, a)),
            lit(chart_at(&app, b)),
            "fixture: the two at different styles"
        );
        let first = chart_at(&app, a);
        app.chart_each(a, &first, "Chart style", |c| {
            c.options = Preset::Bold.apply(c.kind, &c.options);
        });
        app.chart_others.clear();
        for id in [a, b] {
            assert_eq!(lit(chart_at(&app, id)), Some(Preset::Bold));
        }
    }

    /// A bound colour's `%` changed over two charts keeps each chart's own
    /// variable and leaves a chart holding a colour of its own alone.
    /// flip: `rebind_alpha_tx` writing through `chart_bind_tx` with the first
    /// chart's variable → fails at "B keeps its own colour".
    #[test]
    fn a_bound_colours_percent_keeps_each_charts_own() {
        let ctx = egui::Context::default();
        let (mut app, a, b) = two_charts(&ctx);
        let (var, _) = colour_var(&mut app, "grid", mastik_core::peniko::Color::WHITE);
        let slot = ChartColour::Grid(true);
        let tx = app.chart_bind_tx(a, slot, Some(var), Some(1.0));
        assert!(app.commit_edit(tx));
        let red = mastik_core::peniko::Color::from_rgb8(255, 0, 0);
        let literal = with_colour(&chart_at(&app, b), slot, red);
        app.chart_commit(b, "Chart gridline colour", literal);
        let tx = app.rebind_alpha_tx(a, PaintSlot::Chart(slot), 0.5);
        assert!(app.commit_edit(tx));
        assert_eq!(app.chart_binding(a, slot), Some((var, 0.5)));
        assert_eq!(
            fill_of(&chart_at(&app, b), slot),
            Some(Brush::Solid(red).into()),
            "B keeps its own colour"
        );
    }

    /// Controls a chart does not draw stay on the card, dimmed under their
    /// reason: a scatter's value labels; a band axis's gridlines show no rows
    /// under their switch; an area offers every curve it draws.
    /// flip: `inspector_charts` leaving out the scatter's note → fails at its text.
    #[test]
    fn options_a_chart_does_not_draw_say_so() {
        let ctx = egui::Context::default();
        let (mut app, _) = charted(&ctx, ChartKind::Scatter);
        app.collapsed_panels.remove("Chart values");
        let mut out = Vec::new();
        for _ in 0..3 {
            out = shapes(&ctx, &mut app);
        }
        assert!(drawn(&out, "A scatter draws no value labels"));
        assert!(drawn(&out, "Outside"), "the rows stay");

        let (mut app, id) = charted(&ctx, ChartKind::Area);
        app.collapsed_panels.remove("Chart axes");
        app.collapsed_panels.remove("Chart marks");
        let mut chart = chart_at(&app, id);
        chart.options.category_axis.grid.show = true;
        app.chart_commit(id, "Category gridlines", chart);
        for _ in 0..3 {
            out = shapes(&ctx, &mut app);
        }
        assert!(drawn(&out, "Category gridlines"), "the switch stays");
        assert!(
            !drawn(&out, "Dashed category gridlines"),
            "no rows under it"
        );
        assert!(drawn(&out, "Step"), "an area's step curve");
    }

    /// A right-click on a bar names its category and series for *Highlight
    /// ‹name›*, and the row highlights that category as one step.
    /// flip: `mark_names` reading the bar's category from table order under an
    /// order → fails at "the bar under the pointer".
    #[test]
    fn a_mark_under_the_pointer_is_highlighted_by_name() {
        let ctx = egui::Context::default();
        let (mut app, id) = charted(&ctx, ChartKind::Column);
        let mut chart = chart_at(&app, id);
        chart.options.bars.order = chart::Order::Descending;
        app.chart_commit(id, "Bar order", chart);
        pass(&ctx, &mut app);
        let chart = chart_at(&app, id);
        let first = chart.mapping.series[0];
        // The first bar drawn is June's, the largest.
        let bar = chart
            .parts
            .iter()
            .find(|(k, _)| k.role == chart::PartRole::Bar && k.series == first.0 && k.index == 0)
            .map(|(_, n)| *n)
            .expect("a bar");
        let at = app
            .session
            .resolved
            .world_bounds(bar)
            .expect("drawn")
            .center();
        let (cat, series) = mark_names(&app, id, at);
        assert_eq!(cat.as_deref(), Some("Jun"), "the bar under the pointer");
        assert_eq!(series.as_deref(), Some("Revenue"));
        app.perform_menu_item(
            &ctx,
            crate::menu::Item::HighlightCategory,
            crate::menu::Target::Layer {
                id,
                door: crate::menu::Door::Canvas,
            },
            Some(at),
        );
        assert_eq!(
            chart_at(&app, id).highlight.target,
            chart::Highlighted::Categories(vec!["Jun".into()])
        );
        let step = app.session.history.done().last().unwrap();
        assert_eq!(step.note.label, "Highlight");
    }

    /// A double-click on a reference line opens its popover on the card, not
    /// the data window.
    /// flip: `chart_reference_row` ignoring `open_reference` → fails at "the
    /// popover is open".
    #[test]
    fn a_double_clicked_reference_opens_its_popover() {
        let ctx = egui::Context::default();
        let (mut app, id) = charted(&ctx, ChartKind::Column);
        let mut chart = chart_at(&app, id);
        chart.references.push(chart::Reference {
            id: 1,
            at: chart::At::Value(50.0),
            dash: chart::Dash::Solid,
            ..chart::Reference::default()
        });
        app.chart_commit(id, "Add reference", chart);
        pass(&ctx, &mut app);
        let line = chart_at(&app, id)
            .parts
            .iter()
            .find(|(k, _)| k.role == chart::PartRole::Reference)
            .map(|(_, n)| *n)
            .expect("the line is drawn");
        let at = app
            .session
            .resolved
            .world_bounds(line)
            .expect("drawn")
            .center();
        app.double_click_pick(Some(id), at);
        assert_eq!(app.open_reference, Some((id, 1)), "the reference is named");
        let mut out = Vec::new();
        for _ in 0..3 {
            out = shapes(&ctx, &mut app);
        }
        assert_eq!(app.open_reference, None, "taken by the card");
        assert!(drawn(&out, "Take into the range"), "the popover is open");
    }

    /// A lollipop's head, a waterfall's step and a radar's line each name what
    /// the card's Highlight dropdown offers for that kind.
    /// flip: `mark_names` without its waterfall arm → fails at "a waterfall step".
    #[test]
    fn every_kind_names_its_marks_for_a_highlight() {
        let ctx = egui::Context::default();
        let world_of = |app: &MastikApp, id: NodeId, role: chart::PartRole| {
            let chart = chart_at(app, id);
            let n = chart
                .parts
                .iter()
                .find(|(k, _)| k.role == role && k.index == 0)
                .map(|(_, n)| *n)
                .unwrap_or_else(|| panic!("{role:?} drawn"));
            app.session
                .resolved
                .world_bounds(n)
                .expect("drawn")
                .center()
        };
        let first_category = |app: &MastikApp, id: NodeId| {
            let c = chart_at(app, id);
            let col = c.table.index_of(c.mapping.category.unwrap()).unwrap();
            c.table.rows[0][col].trim().to_string()
        };

        let (mut app, id) = charted(&ctx, ChartKind::Column);
        let mut chart = chart_at(&app, id);
        chart.options.bars.shape = chart::BarShape::Lollipops;
        app.chart_commit(id, "Bar shape", chart);
        pass(&ctx, &mut app);
        let at = world_of(&app, id, chart::PartRole::Head);
        let (cat, _) = mark_names(&app, id, at);
        assert_eq!(cat, Some(first_category(&app, id)), "a lollipop's head");

        let (mut app, id) = charted(&ctx, ChartKind::Waterfall);
        pass(&ctx, &mut app);
        let at = world_of(&app, id, chart::PartRole::Bar);
        let (cat, series) = mark_names(&app, id, at);
        assert_eq!(cat, Some(first_category(&app, id)), "a waterfall step");
        assert_eq!(series, None);

        let (mut app, id) = charted(&ctx, ChartKind::Radar);
        pass(&ctx, &mut app);
        let at = world_of(&app, id, chart::PartRole::Line);
        let (_, series) = mark_names(&app, id, at);
        assert!(series.is_some(), "a radar's line names its series");
    }

    /// The card offers the chart's own spacing, order, totals, mark stroke and
    /// role rows, and a column chart keeps *Label line ends* shown, disabled.
    /// flip: `chart_style_rows` leaving out the padding pair → fails at
    /// "padding".
    #[test]
    fn the_card_offers_the_rows_the_model_has() {
        let ctx = egui::Context::default();
        let (mut app, _) = charted(&ctx, ChartKind::Column);
        for key in [
            "Chart style",
            "Chart marks",
            "Chart values",
            "Chart legend",
            "Chart text",
        ] {
            app.collapsed_panels.remove(key);
        }
        let mut out = Vec::new();
        for _ in 0..3 {
            out = shapes(&ctx, &mut app);
        }
        assert!(drawn(&out, "Padding"), "padding");
        for row in [
            "Gap",
            "Table order",
            "Stack totals",
            "Solid",
            "Label line ends",
            "BY ROLE",
            "Axis titles",
            "Background",
            "Plot background",
        ] {
            assert!(drawn(&out, row), "{row}");
        }
    }

    /// The Style section's *Dark* turns the chart dark as one *Chart theme*
    /// step: its ink, unset, follows to the dark colour.
    /// flip: the row's setter leaving the theme as it is → fails at "dark".
    #[test]
    fn the_theme_row_turns_the_chart_dark() {
        let ctx = egui::Context::default();
        let (mut app, id) = charted(&ctx, ChartKind::Column);
        app.collapsed_panels.remove("Chart style");
        let mut out = Vec::new();
        for _ in 0..3 {
            out = shapes(&ctx, &mut app);
        }
        assert!(drawn(&out, "Auto"), "the row's other segment");
        let at = text_at(&out, "Dark").expect("the Dark segment");
        click(&ctx, &mut app, at);
        let chart = chart_at(&app, id);
        assert_eq!(chart.options.theme, chart::Theme::Dark, "dark");
        let night = chart::chrome(true);
        assert_eq!(chart.ink(), Brush::Solid(night.ink).into());
        let step = app.session.history.done().last().unwrap();
        assert_eq!(step.note.label, "Chart theme");
    }

    /// A pie lists its slices, by category, where a series chart lists its
    /// series, and a slice's colour row writes the colour the slice draws in,
    /// as a step named for a slice.
    /// flip: `chart_series_rows` listing a pie's columns → fails at "the slice's row".
    #[test]
    fn a_pie_lists_and_colours_its_slices() {
        let ctx = egui::Context::default();
        let (mut app, id) = charted(&ctx, ChartKind::Pie);
        app.collapsed_panels.remove("Chart series");
        let marks = chart::named_marks(&chart_at(&app, id)).expect("named");
        let first = marks[0].0.clone();
        let mut out = Vec::new();
        for _ in 0..3 {
            out = shapes(&ctx, &mut app);
        }
        assert!(drawn(&out, "Slices"), "the section's name");
        assert!(drawn(&out, &first), "the slice's row");
        let red = mastik_core::peniko::Color::from_rgb8(255, 0, 0);
        let tx = app.slot_transaction(
            id,
            PaintSlot::Chart(ChartColour::Node(0)),
            Brush::Solid(red),
        );
        app.name_slot_step(id, PaintSlot::Chart(ChartColour::Node(0)));
        assert!(app.commit_edit(tx));
        let step = app.session.history.done().last().unwrap();
        assert_eq!(step.note.label, "Slice colour");
        let marks = chart::named_marks(&chart_at(&app, id)).unwrap();
        assert_eq!(
            marks[0].1,
            Brush::Solid(red).into(),
            "the slice draws in it"
        );
    }

    /// **A styled chart's look folds into the capsule and the rest stays live**:
    /// bound to a chart style, the card draws the capsule's summary, no *Marks*
    /// section, *This chart* under Data and the palette held *From the style*;
    /// detached, Marks is back and *This chart* gone.
    /// flip: `inspector_charts` reading every chart as unstyled, so the whole
    /// card goes under the capsule → fails at "Data stays"; `chart_own_rows`
    /// without the bar rows → fails at "the bar shape is the chart's own".
    #[test]
    fn a_styled_chart_folds_its_look_into_the_capsule() {
        let ctx = egui::Context::default();
        let (mut app, id) = charted(&ctx, ChartKind::Column);
        app.collapsed_panels.remove("Chart series");
        let s = app.session.ids.mint_style();
        let ops = mastik_core::style::create_ops(
            &app.session.doc,
            s,
            "Dashboard",
            id,
            StyleSlot::Chart,
            &[id],
        )
        .expect("a chart has a look");
        assert!(app.commit_edit(Transaction(ops)));
        assert_eq!(
            app.session.doc.get(id).unwrap().styles().chart,
            Some(s),
            "the fixture is styled"
        );
        let mut out = Vec::new();
        for _ in 0..3 {
            out = shapes(&ctx, &mut app);
        }
        assert!(drawn(&out, "Dashboard"), "the capsule's name");
        assert!(drawn(&out, "Clean"), "its summary, the nearest preset");
        assert!(!drawn(&out, "Marks"), "no Marks under the capsule");
        assert!(drawn(&out, "Data"), "Data stays");
        assert!(drawn(&out, "THIS CHART"), "the chart's own rows");
        assert!(drawn(&out, "Lollipops"), "the bar shape is the chart's own");
        assert!(drawn(&out, "BAR ORDER"), "and so is its order");
        assert!(
            drawn(&out, "From the style · Detach to change"),
            "the palette is the style's"
        );
        let detach = mastik_core::style::bind_op(id, StyleSlot::Chart, None);
        assert!(app.commit_edit(Transaction(vec![detach])));
        for _ in 0..3 {
            out = shapes(&ctx, &mut app);
        }
        assert!(drawn(&out, "Marks"), "Marks is back");
        assert!(!drawn(&out, "THIS CHART"), "and This chart gone");
    }
}
