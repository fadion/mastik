//! `chart::generate`: a description laid out into a size, as the parts that
//! draw it (`docs/design/charts.md`, *Laid out, not placed*).
//!
//! Every part carries both its place at this size (its stored transform and
//! geometry) and the layout that keeps it there as the frame resizes (insets in
//! percent of the plot, flex and grid on the containers), so the parts draw
//! right before any layout runs and reflow with it after.

use super::scale::{self, Ticks};
use super::{
    Axis, AxisSide, Chart, ChartKind, ColumnId, LabelAngle, LegendPosition, PartKey, PartRole,
    Stacking, ValuePosition, parse_number,
};
use crate::container::{
    AlignContent, AlignItems, AutoMargins, Dimension, Display, Flex, FlexDirection, FlexWrap, Grid,
    GridLines, GridPlacement, Insets, JustifyContent, LayoutItem, LengthPct, Track, TrackBreadth,
    TrackSize,
};
use crate::node::{Fill, NodeKind, Stroke, TextSizing};
use crate::typography::TextStyle;
use crate::variable::BrushSource;
use kurbo::{Affine, Point, Rect, RoundedRectRadii, Shape, Size};

/// One generated node: its role, where it hangs, and everything it is.
#[derive(Clone, Debug, PartialEq)]
pub struct Part {
    pub key: PartKey,
    /// The part it is a child of; `None` hangs it on the chart frame.
    pub parent: Option<PartKey>,
    pub name: String,
    pub kind: NodeKind,
    pub transform: Affine,
    pub fills: Vec<Fill>,
    pub strokes: Vec<Stroke>,
    pub opacity: f32,
    pub insets: Insets,
    pub display: Option<Display>,
    pub item: LayoutItem,
    /// A container that clips what it holds (a run below zero); every other
    /// container lets a label overflow it.
    pub clip: bool,
}

/// What [`generate`] makes of a chart at a size.
#[derive(Clone, Debug, PartialEq)]
pub struct Generated {
    /// The chart frame's own layout.
    pub display: Display,
    /// Parents before their children, siblings bottom to top.
    pub parts: Vec<Part>,
    /// Values the kind cannot draw, each with its reason, for the card's line.
    pub left_out: Vec<String>,
    /// The value axis's and a numeric category axis's drawn extent, min to
    /// max — what *Auto range* turned off starts from, so the axis holds.
    pub extents: [Option<(f64, f64)>; 2],
}

/// Why a chart draws nothing new.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Refusal {
    /// More marks than [`super::MAX_MARKS`]: the count and what they are.
    TooManyMarks { count: usize, what: &'static str },
    /// A Sankey whose flows lead back to a node already passed, named as the
    /// loop runs: `Signup → Search → Signup`.
    LoopsBack(String),
}

/// The gap between an axis's labels and the plot.
const AXIS_GAP: f64 = 6.0;
/// The legend's gap between items, and between a swatch and its label.
const LEGEND_GAP: f64 = 12.0;
const SWATCH_GAP: f64 = 6.0;
/// A legend swatch's side.
const SWATCH: f64 = 10.0;
/// A *Line* legend swatch's length and stroke.
const LINE_SWATCH: f64 = 14.0;
const LINE_SWATCH_W: f64 = 2.0;
/// The space between a value label and its bar.
const VALUE_GAP: f64 = 4.0;
/// A tick mark's length, out from the plot into its axis's track.
const TICK_LEN: f64 = 4.0;
/// A dashed gridline's dash and gap.
const GRID_DASH: [f64; 2] = [3.0, 3.0];

/// The palettes' colours, in the order series take them, for a light ground or
/// (`dark`) a dark one. A dark colour is its light twin at the same place,
/// lifted to hold its contrast against [`super::chrome`]'s dark ground, so a
/// series keeps its hue when the chart changes ground.
pub fn palette(p: super::Palette, dark: bool) -> &'static [[u8; 3]] {
    if dark {
        return dark_palette(p);
    }
    match p {
        // Every two at least 7 apart (CIE76) under the three dichromacies
        // (`mastik-render`'s vision tests).
        super::Palette::Mastik => &[
            [0x3B, 0x6F, 0xE0],
            [0xE0, 0x6C, 0x1E],
            [0x2E, 0xA8, 0x8A],
            [0xD9, 0x4E, 0x7A],
            [0x7A, 0x52, 0xC8],
            [0xC9, 0xA2, 0x27],
            [0x4A, 0xA8, 0xD8],
            [0x7A, 0x7A, 0x7A],
        ],
        super::Palette::Cool => &[
            [0x1F, 0x4E, 0x99],
            [0x2E, 0x86, 0xC1],
            [0x48, 0xC9, 0xB0],
            [0x5D, 0x6D, 0x7E],
            [0x76, 0xD7, 0xEA],
            [0x1A, 0x93, 0x6F],
        ],
        super::Palette::Warm => &[
            [0xC0, 0x39, 0x2B],
            [0xE6, 0x7E, 0x22],
            [0xF1, 0xC4, 0x0F],
            [0xA0, 0x40, 0x00],
            [0xE8, 0x8E, 0x8E],
            [0x8E, 0x44, 0xAD],
        ],
        super::Palette::Mono => &[
            [0x1E, 0x3A, 0x5F],
            [0x2F, 0x5C, 0x93],
            [0x4A, 0x7F, 0xC1],
            [0x7B, 0xA4, 0xD9],
            [0xAE, 0xC8, 0xEA],
        ],
    }
}

/// [`palette`]'s dark-ground colours.
fn dark_palette(p: super::Palette) -> &'static [[u8; 3]] {
    match p {
        // Held apart as the light set is.
        super::Palette::Mastik => &[
            [0x6B, 0x95, 0xF0],
            [0xF0, 0x8C, 0x3C],
            [0x3C, 0xC4, 0xA1],
            [0xE2, 0x5D, 0x88],
            [0x94, 0x68, 0xE2],
            [0xE8, 0xCC, 0x4E],
            [0x6E, 0xC2, 0xEC],
            [0x8C, 0x8C, 0x8C],
        ],
        super::Palette::Cool => &[
            [0x5A, 0x8A, 0xD6],
            [0x4F, 0xA6, 0xDE],
            [0x5F, 0xDC, 0xC3],
            [0x8E, 0x9D, 0xAD],
            [0x9A, 0xE4, 0xF2],
            [0x2D, 0xBB, 0x8F],
        ],
        super::Palette::Warm => &[
            [0xE5, 0x58, 0x4A],
            [0xF0, 0x96, 0x4A],
            [0xF5, 0xD3, 0x3F],
            [0xD0, 0x6A, 0x26],
            [0xF2, 0xAA, 0xAA],
            [0xB2, 0x6F, 0xD0],
        ],
        // Brightest first: on a dark ground the first series leads, as the
        // darkest does on a light one.
        super::Palette::Mono => &[
            [0xC7, 0xDB, 0xF5],
            [0x9C, 0xBE, 0xEB],
            [0x6E, 0x9D, 0xDC],
            [0x4A, 0x7F, 0xC1],
            [0x2F, 0x5C, 0x93],
        ],
    }
}

fn rgb(c: [u8; 3]) -> BrushSource {
    BrushSource::Literal(crate::Brush::Solid(peniko::Color::from_rgb8(
        c[0], c[1], c[2],
    )))
}

/// One series as drawn: its column, name, colour and values by row (`None` for
/// an empty cell or one left out).
struct SeriesData {
    column: ColumnId,
    name: String,
    fill: BrushSource,
    values: Vec<Option<f64>>,
}

/// What the table says, read for the kind: the categories (rows whose category
/// is not empty) and the visible series.
struct Data {
    categories: Vec<String>,
    series: Vec<SeriesData>,
    left_out: Vec<String>,
    /// Each category's own colour, by drawn place, when the bars are coloured
    /// by category (`Options::by_category`); empty otherwise.
    by_category: Vec<BrushSource>,
}

fn read(chart: &Chart) -> Data {
    let t = &chart.table;
    let mut left_out = Vec::new();
    let cat = chart.mapping.category.and_then(|c| t.index_of(c));
    let mut rows = Vec::new();
    for (i, r) in t.rows.iter().enumerate() {
        match cat {
            Some(c) if r[c].trim().is_empty() => {
                left_out.push(format!("Row {}: no category", i + 1));
            }
            Some(c) => rows.push((i, r[c].trim().to_string())),
            None => rows.push((i, format!("{}", i + 1))),
        }
    }
    let colours = super::colours::drawn_palette(chart);
    let mut series = Vec::new();
    for (n, &col) in chart.mapping.series.iter().enumerate() {
        let Some(at) = t.index_of(col) else { continue };
        let style = chart.series.iter().find(|s| s.column == col);
        if style.is_some_and(|s| !s.visible) {
            continue;
        }
        let name = t.columns[at].name.clone();
        let mut values = Vec::with_capacity(rows.len());
        for (i, _) in &rows {
            let cell = t.rows[*i][at].trim();
            if cell.is_empty() {
                values.push(None);
            } else if let Some(v) = parse_number(cell) {
                values.push(Some(v));
            } else {
                left_out.push(format!("{name}, row {}: “{cell}” is not a number", i + 1));
                values.push(None);
            }
        }
        series.push(SeriesData {
            column: col,
            name,
            fill: style
                .and_then(|s| s.fill.clone())
                .unwrap_or_else(|| rgb(colours[n % colours.len()])),
            values,
        });
    }
    Data {
        categories: rows.into_iter().map(|(_, c)| c).collect(),
        series,
        left_out,
        by_category: Vec::new(),
    }
}

/// The order a column or bar chart draws its categories in, as their places
/// among the categories read (rows with no category left out); `None` in table
/// order or for any other kind. Keyed by the first series shown, or by each
/// stack's total when stacked; ties keep table order and a category with no
/// value goes last. A mark's index counts drawn places, so a reader mapping one
/// back to its row goes through this.
pub fn category_order(chart: &Chart) -> Option<Vec<usize>> {
    drawn_order(chart, &read(chart))
}

fn drawn_order(chart: &Chart, data: &Data) -> Option<Vec<usize>> {
    let b = &chart.options.bars;
    if !matches!(chart.kind, ChartKind::Column | ChartKind::Bar) || b.order == super::Order::Table {
        return None;
    }
    let key = |c: usize| -> Option<f64> {
        if b.stacking == Stacking::Apart {
            data.series.first().and_then(|s| s.values[c])
        } else {
            let vs: Vec<f64> = data.series.iter().filter_map(|s| s.values[c]).collect();
            (!vs.is_empty()).then(|| vs.iter().sum())
        }
    };
    let mut order: Vec<usize> = (0..data.categories.len()).collect();
    let desc = b.order == super::Order::Descending;
    order.sort_by(|a, b| match (key(*a), key(*b)) {
        (Some(x), Some(y)) if desc => y.total_cmp(&x),
        (Some(x), Some(y)) => x.total_cmp(&y),
        (Some(_), None) => std::cmp::Ordering::Less,
        (None, Some(_)) => std::cmp::Ordering::Greater,
        (None, None) => std::cmp::Ordering::Equal,
    });
    Some(order)
}

/// [`generate_on`] the ground the chart was last drawn on, as a plain colour:
/// for a picture or a preview drawn apart from the document.
pub fn generate(chart: &Chart, size: Size, family: &str) -> Result<Generated, Refusal> {
    let ground = super::Ground::plain(chart.grounded.unwrap_or(peniko::Color::WHITE));
    generate_on(chart, size, family, &ground)
}

/// Lays `chart` out into `size` as parts, on `ground` (`chart::ground_of`):
/// every unset colour derived for it. `family` is the document's default text
/// family, used when the chart names none.
pub fn generate_on(
    chart: &Chart,
    size: Size,
    family: &str,
    ground: &super::Ground,
) -> Result<Generated, Refusal> {
    let chart = &resolved(&chart.on(ground.colour));
    let mut data = read(chart);
    if let Some(order) = drawn_order(chart, &data) {
        data.categories = order.iter().map(|i| data.categories[*i].clone()).collect();
        for s in &mut data.series {
            s.values = order.iter().map(|i| s.values[*i]).collect();
        }
    }
    if super::by_category(chart)
        && let Some(marks) = super::named_marks(chart)
    {
        let fallback = data.series.first().map(|s| s.fill.clone());
        data.by_category = data
            .categories
            .iter()
            .filter_map(|c| {
                marks
                    .iter()
                    .find(|(n, _)| n == c)
                    .map(|(_, f)| f.clone())
                    .or_else(|| fallback.clone())
            })
            .collect();
    }
    // A pie's slices are its legend's entries; every other kind's are its series.
    let slices = (chart.kind == ChartKind::Pie).then(|| pie_slices(chart, &mut data));
    let clouds = (chart.kind == ChartKind::Scatter).then(|| scatter_clouds(chart, &mut data));
    // Refused before the legend measures an entry for each.
    if let Some(s) = slices.as_ref().filter(|s| s.len() > super::MAX_SLICES) {
        return Err(Refusal::TooManyMarks {
            count: s.len(),
            what: "slices",
        });
    }
    let points: usize = clouds.iter().flatten().map(|c| c.points.len()).sum();
    if points > super::MAX_MARKS {
        return Err(Refusal::TooManyMarks {
            count: points,
            what: "points",
        });
    }
    let mut entries: Vec<Entry> = match (&slices, &clouds) {
        // A waterfall's colours belong to the step, not the series.
        _ if chart.kind == ChartKind::Waterfall => [
            (super::Step::Increase, "Increase"),
            (super::Step::Decrease, "Decrease"),
            (super::Step::Total, "Total"),
        ]
        .into_iter()
        .enumerate()
        .map(|(i, (step, name))| Entry {
            series: i as u32,
            name: name.into(),
            fill: step_fill(chart, step),
            dash: super::Dash::Solid,
        })
        .collect(),
        // Bars coloured by category list their categories, as a pie does.
        _ if !data.by_category.is_empty() => data
            .categories
            .iter()
            .zip(&data.by_category)
            .enumerate()
            .map(|(i, (name, fill))| Entry {
                series: i as u32,
                name: name.clone(),
                fill: fill.clone(),
                dash: super::Dash::Solid,
            })
            .collect(),
        (Some(s), _) => s
            .iter()
            .map(|s| Entry {
                series: s.index,
                name: s.name.clone(),
                fill: s.fill.clone(),
                dash: super::Dash::Solid,
            })
            .collect(),
        (_, Some(c)) => c
            .iter()
            .map(|c| Entry {
                series: c.id,
                name: c.name.clone(),
                fill: c.fill.clone(),
                dash: super::Dash::Solid,
            })
            .collect(),
        _ => data
            .series
            .iter()
            .map(|s| Entry {
                series: s.column.0,
                name: s.name.clone(),
                fill: s.fill.clone(),
                dash: chart
                    .series
                    .iter()
                    .find(|st| st.column == s.column)
                    .map_or(super::Dash::Solid, |st| st.dash),
            })
            .collect(),
    };
    // A swatch for the below-zero colour, while some value is below.
    let percent = match chart.kind {
        ChartKind::Column | ChartKind::Bar => chart.options.bars.stacking == Stacking::Percent,
        ChartKind::Area => chart.options.lines.area_stacking == Stacking::Percent,
        _ => false,
    };
    if let Some(below) = &chart.options.below_zero
        && !percent
        && matches!(
            chart.kind,
            ChartKind::Column | ChartKind::Bar | ChartKind::Line | ChartKind::Area
        )
        && data
            .series
            .iter()
            .any(|s| s.values.iter().flatten().any(|v| *v < 0.0))
    {
        entries.push(Entry {
            series: u32::MAX,
            name: "Below zero".into(),
            fill: below.clone(),
            dash: super::Dash::Solid,
        });
    }
    let data = data;
    let o = &chart.options;
    let text = Text::of(o, family);
    let mut parts = Vec::new();
    let mut out = Out {
        parts: &mut parts,
        text: &text,
        extents: [None; 2],
        ground: ground.brush.clone(),
        ground_colour: ground.colour,
        vars: &ground.vars,
        left_out: Vec::new(),
    };

    let legend_at = match o.legend.position {
        LegendPosition::Off => None,
        p if entries.is_empty() || matches!(chart.kind, ChartKind::Sankey) => {
            let _ = p;
            None
        }
        p => Some(p),
    };
    let row = matches!(
        legend_at,
        Some(LegendPosition::Left | LegendPosition::Right)
    );
    let pad = o.padding;
    let inner = Size::new(
        (size.width - 2.0 * pad).max(0.0),
        (size.height - 2.0 * pad).max(0.0),
    );

    // The legend first: its size decides the body's.
    // A *Line* swatch draws a line only where the series are lines.
    let swatch = match o.legend.swatch {
        super::Swatch::Line if !matches!(chart.kind, ChartKind::Line | ChartKind::Area) => {
            super::Swatch::Square
        }
        s => s,
    };
    let legend = legend_at.map(|at| legend_items(&entries, swatch, &text, at, inner));
    let legend_box = legend.as_ref().map_or(Size::ZERO, |l| l.size);
    let body = if row {
        Size::new(
            (inner.width - legend_box.width - o.gap).max(0.0),
            inner.height,
        )
    } else if legend.is_some() {
        Size::new(
            inner.width,
            (inner.height - legend_box.height - o.gap).max(0.0),
        )
    } else {
        inner
    };
    let legend_first = matches!(legend_at, Some(LegendPosition::Top | LegendPosition::Left));
    let body_at = Point::new(
        pad + if row && legend_first {
            legend_box.width + o.gap
        } else {
            0.0
        },
        pad + if !row && legend_first {
            legend_box.height + o.gap
        } else {
            0.0
        },
    );
    let legend_origin = Point::new(
        pad + if row && !legend_first {
            body.width + o.gap
        } else {
            0.0
        },
        pad + if !row && !legend_first {
            body.height + o.gap
        } else {
            0.0
        },
    );

    let emit_legend = |out: &mut Out| {
        if let Some(l) = &legend {
            out.legend(l, legend_origin, row, o.legend.align);
        }
    };
    if legend_first {
        emit_legend(&mut out);
    }
    let mut sankey_left = Vec::new();
    match chart.kind {
        ChartKind::Column | ChartKind::Bar => {
            bars(chart, &data, body, body_at, &mut out)?;
        }
        ChartKind::Waterfall => {
            waterfall(chart, &data, body, body_at, &mut out)?;
        }
        ChartKind::Radar => {
            radar(chart, &data, body, body_at, row, &mut out)?;
        }
        ChartKind::Line | ChartKind::Area => {
            lines(chart, &data, body, body_at, &mut out)?;
        }
        ChartKind::Sankey => {
            let mut left = Vec::new();
            sankey(chart, &mut left, body, body_at, &mut out)?;
            sankey_left = left;
        }
        ChartKind::Scatter => {
            scatter(
                chart,
                &data,
                clouds.as_deref().unwrap_or(&[]),
                body,
                body_at,
                &mut out,
            )?;
        }
        ChartKind::Pie => {
            pie(
                chart,
                slices.as_deref().unwrap_or(&[]),
                body,
                body_at,
                row,
                &mut out,
            )?;
        }
    }
    if !legend_first {
        emit_legend(&mut out);
    }

    let display = Display::Flex(Flex {
        direction: if row {
            FlexDirection::Row
        } else {
            FlexDirection::Column
        },
        wrap: FlexWrap::NoWrap,
        justify_content: JustifyContent::Start,
        align_items: AlignItems::Stretch,
        align_content: AlignContent::default(),
        row_gap: o.gap,
        column_gap: o.gap,
        padding: [pad; 4],
        ..Flex::default()
    });
    let extents = out.extents;
    let notes = std::mem::take(&mut out.left_out);
    let names = Names {
        categories: &data.categories,
        series: data
            .series
            .iter()
            .map(|s| (s.column.0, s.name.as_str()))
            .chain(clouds.iter().flatten().map(|c| (c.id, c.name.as_str())))
            .collect(),
        slices: slices
            .iter()
            .flatten()
            .map(|s| (s.index, s.name.as_str()))
            .collect(),
        by_category: !data.by_category.is_empty(),
    };
    apply_highlight(chart, &names, &mut parts);
    let mut left_out = data.left_out;
    left_out.extend(sankey_left);
    left_out.extend(notes);
    Ok(Generated {
        display,
        parts,
        left_out,
        extents,
    })
}

/// What a highlight reads a part's key by: the drawn categories, the series
/// and scatter groups by key, a pie's slices by key.
struct Names<'a> {
    categories: &'a [String],
    series: Vec<(u32, &'a str)>,
    slices: Vec<(u32, &'a str)>,
    /// Legend entries are categories (colour by category).
    by_category: bool,
}

/// The parts of a highlighted chart's other marks muted or faded, its value
/// labels on them dropped under *Highlighted only*, their category labels in the
/// muted colour, and a muted line thinner and drawn under the highlighted. A
/// highlight naming nothing drawn leaves every mark as it is.
fn apply_highlight(chart: &Chart, names: &Names, parts: &mut Vec<Part>) {
    let h = &chart.highlight;
    let (cats, series) = match &h.target {
        super::Highlighted::None => return,
        super::Highlighted::Categories(n) => (Some(n), None),
        super::Highlighted::Series(n) => (None, Some(n)),
    };
    let series_of = |k: u32| names.series.iter().find(|(c, _)| *c == k).map(|(_, n)| *n);
    // Whether the mark keyed `k` is lit: `None` for a part a highlight leaves
    // alone.
    let lit = |k: &PartKey| -> Option<bool> {
        let by_cat = |i: u32| {
            let name = match chart.kind {
                ChartKind::Pie => names.slices.iter().find(|(s, _)| *s == i).map(|(_, n)| *n),
                _ => names.categories.get(i as usize).map(String::as_str),
            };
            cats.map(|c| name.is_some_and(|n| c.iter().any(|x| x == n)))
        };
        let by_series =
            |s: u32| series.map(|names| series_of(s).is_some_and(|n| names.iter().any(|x| x == n)));
        match k.role {
            PartRole::Bar => by_cat(k.index).or_else(|| by_series(k.series)),
            PartRole::Slice => by_cat(k.index),
            PartRole::ValueLabel if chart.kind == ChartKind::Pie => by_cat(k.index),
            PartRole::ValueLabel | PartRole::Point => match chart.kind {
                ChartKind::Scatter => by_series(k.series),
                _ => by_cat(k.index).or_else(|| by_series(k.series)),
            },
            PartRole::Line | PartRole::Area => by_series(k.series),
            PartRole::CategoryLabel => by_cat(k.index),
            PartRole::LegendSwatch if chart.kind == ChartKind::Pie || names.by_category => {
                by_cat(k.series)
            }
            PartRole::LegendSwatch => by_series(k.series),
            _ => None,
        }
    };
    // Nothing drawn is lit: the highlight names nothing here, so it does nothing.
    let any = parts.iter().any(|p| {
        matches!(
            p.key.role,
            PartRole::Bar | PartRole::Slice | PartRole::Line | PartRole::Point
        ) && lit(&p.key) == Some(true)
    });
    if !any {
        return;
    }
    let muted = chart.muted();
    let dim = |b: &BrushSource| match h.others {
        super::Others::Muted => muted.clone(),
        super::Others::Faded => tinted(b, super::FADED),
    };
    let mut drop = Vec::new();
    for (i, p) in parts.iter_mut().enumerate() {
        if lit(&p.key) != Some(false) {
            continue;
        }
        match p.key.role {
            PartRole::ValueLabel if h.values_only => drop.push(i),
            PartRole::ValueLabel => {}
            PartRole::CategoryLabel => {
                for f in &mut p.fills {
                    f.brush = dim(&f.brush);
                }
            }
            _ => {
                for f in &mut p.fills {
                    f.brush = dim(&f.brush);
                }
                for s in &mut p.strokes {
                    s.brush = dim(&s.brush);
                    if p.key.role == PartRole::Line {
                        s.width *= 0.75;
                    }
                }
            }
        }
    }
    for i in drop.into_iter().rev() {
        parts.remove(i);
    }
    // A muted line's series goes under the lit ones, keeping their order.
    let unlit_series = parts.iter().any(|p| {
        p.key.role == PartRole::Series
            && p.key.index == 0
            && !by_series_name(series, series_of(p.key.series))
    });
    if matches!(chart.kind, ChartKind::Line | ChartKind::Area) && unlit_series {
        let lit_series = |p: &Part| {
            p.key.role == PartRole::Series
                && p.key.index == 0
                && by_series_name(series, series_of(p.key.series))
        };
        // Each lit series' frame with everything under it, which follows it.
        let mut lits: Vec<Part> = Vec::new();
        let mut i = 0;
        while i < parts.len() {
            if !lit_series(&parts[i]) {
                i += 1;
                continue;
            }
            let mut under: Vec<PartKey> = vec![parts[i].key];
            lits.push(parts.remove(i));
            while i < parts.len() && parts[i].parent.is_some_and(|k| under.contains(&k)) {
                under.push(parts[i].key);
                lits.push(parts.remove(i));
            }
        }
        // After the last unlit series' subtree, so they draw over it.
        let last = parts
            .iter()
            .rposition(|p| p.key.role == PartRole::Series && p.key.index == 0);
        if let Some(mut at) = last {
            let mut under = vec![parts[at].key];
            at += 1;
            while at < parts.len() && parts[at].parent.is_some_and(|k| under.contains(&k)) {
                under.push(parts[at].key);
                at += 1;
            }
            for (k, p) in lits.into_iter().enumerate() {
                parts.insert(at + k, p);
            }
        } else {
            parts.extend(lits);
        }
    }
}

/// Whether the series named `name` is among `names`.
fn by_series_name(names: Option<&Vec<String>>, name: Option<&str>) -> bool {
    names.is_some_and(|ns| name.is_some_and(|n| ns.iter().any(|x| x == n)))
}

/// `chart` with every unset chrome colour set to what it derives
/// (`Chart::ink` and its neighbours), so the passes below read them as set.
fn resolved(chart: &Chart) -> Chart {
    let mut c = chart.clone();
    c.options.text.ink = Some(chart.ink());
    for value in [true, false] {
        let (axis, grid) = (chart.axis_colour(value), chart.grid_colour(value));
        let a = if value {
            &mut c.options.value_axis
        } else {
            &mut c.options.category_axis
        };
        a.colour = Some(axis);
        a.grid.colour = Some(grid);
    }
    c
}

/// A chrome colour [`resolved`] has set.
fn set(b: &Option<BrushSource>) -> BrushSource {
    b.clone()
        .unwrap_or(BrushSource::Literal(crate::Brush::Solid(
            peniko::Color::TRANSPARENT,
        )))
}

fn key(role: PartRole, series: u32, index: u32) -> PartKey {
    PartKey {
        role,
        series,
        index,
    }
}

/// The body fills what the legend leaves, along the frame's axis and across it.
fn body_item(row: bool) -> LayoutItem {
    let mut item = LayoutItem {
        grow: 1.0,
        basis: Dimension::Px(0.0),
        ..LayoutItem::default()
    };
    if row {
        item.min_width = Dimension::Px(0.0);
        item.height = Dimension::Percent(100.0);
    } else {
        item.min_height = Dimension::Px(0.0);
        item.width = Dimension::Percent(100.0);
    }
    item
}

/// The chart's type, resolved.
struct Text {
    family: String,
    /// Each role's size, weight and ink, in [`Role`]'s order.
    roles: [(f64, u16, BrushSource); 4],
}

/// The kind of a label, which sets its size, weight and ink (`ChartText`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Role {
    Tick,
    Value,
    Legend,
    Title,
}

impl Text {
    /// The type `o` sets each role in, `family` taking an unnamed one, every
    /// unset weight and ink the chart's.
    fn of(o: &super::Options, family: &str) -> Text {
        let t = &o.text;
        let role = |size: f64, r: &super::RoleText| {
            (
                size,
                r.weight.unwrap_or(t.weight),
                r.ink.clone().unwrap_or_else(|| set(&t.ink)),
            )
        };
        Text {
            family: if t.family.is_empty() {
                family.to_string()
            } else {
                t.family.clone()
            },
            roles: [
                role(t.tick_size, &t.tick),
                role(t.value_size, &t.value),
                role(t.legend_size, &t.legend),
                role(t.title_size, &t.title),
            ],
        }
    }

    fn size(&self, r: Role) -> f64 {
        self.roles[r as usize].0
    }

    fn weight(&self, r: Role) -> u16 {
        self.roles[r as usize].1
    }

    fn ink(&self, r: Role) -> BrushSource {
        self.roles[r as usize].2.clone()
    }

    fn kind(&self, content: &str, r: Role) -> NodeKind {
        self.kind_weighted(content, r, self.weight(r))
    }

    /// [`Self::kind`] at `weight` rather than the role's own.
    fn kind_weighted(&self, content: &str, r: Role, weight: u16) -> NodeKind {
        NodeKind::Text {
            content: content.to_string(),
            style: Box::new(TextStyle {
                font_family: self.family.clone(),
                font_size: self.size(r),
                weight,
                ..TextStyle::default()
            }),
            spans: Default::default(),
            para_spans: Default::default(),
            paragraph: Default::default(),
            block: Default::default(),
            sizing: TextSizing::Auto,
            on_path: None,
            on_path_flip: false,
            on_path_offset: 0.0,
        }
    }

    /// The box `content` sets in as role `r`, from the same layout a text node
    /// uses.
    fn measure(&self, content: &str, r: Role) -> Rect {
        crate::geometry::local_bounds(&self.kind(content, r), None).unwrap_or(Rect::new(
            0.0,
            0.0,
            0.0,
            self.size(r) * 1.2,
        ))
    }
}

/// The parts being written, and the type they set labels in.
struct Out<'a> {
    parts: &'a mut Vec<Part>,
    text: &'a Text,
    /// [`Generated::extents`], written where the axes are ticked.
    extents: [Option<(f64, f64)>; 2],
    /// The ground the chart is drawn on (`Ground::brush`), and its colour.
    ground: BrushSource,
    ground_colour: peniko::Color,
    /// The colours of the variables the chart binds (`Ground::vars`).
    vars: &'a [(crate::VarId, peniko::Color)],
    /// What a pass left out and says on the card, beside the data's.
    left_out: Vec<String>,
}

fn fill(brush: BrushSource) -> Vec<Fill> {
    vec![Fill {
        brush,
        visible: true,
    }]
}

/// A mark's paint in `brush` with `Options::stroke` laid inside it, so a mark
/// keeps the size its value gives it: *Solid* fills it and strokes it in the
/// stroke's colour (unset, `ground`), *Outline* strokes it in its colour (or the
/// stroke's) over a tint of it. Width 0 fills it plain.
fn mark_paint(
    o: &super::Options,
    brush: &BrushSource,
    ground: &BrushSource,
) -> (Vec<Fill>, Vec<Stroke>) {
    let s = &o.stroke;
    if s.width <= 0.0 {
        return (fill(brush.clone()), Vec::new());
    }
    let (body, ink) = match s.style {
        super::StrokeStyle::Solid => (brush.clone(), s.colour.clone().unwrap_or(ground.clone())),
        super::StrokeStyle::Outline => (
            tinted(brush, super::OUTLINE_TINT),
            s.colour.clone().unwrap_or(brush.clone()),
        ),
    };
    let mut stroke = line_stroke(ink, s.width, super::Dash::Solid);
    stroke.align = crate::node::StrokeAlign::Inside;
    (fill(body), vec![stroke])
}

/// `brush` at `k` of its alpha; a bound colour keeps its binding.
fn tinted(brush: &BrushSource, k: f32) -> BrushSource {
    match brush {
        BrushSource::Bound { var, alpha } => BrushSource::Bound {
            var: *var,
            alpha: alpha * k,
        },
        BrushSource::Literal(crate::Brush::Solid(c)) => {
            BrushSource::Literal(crate::Brush::Solid(c.multiply_alpha(k)))
        }
        BrushSource::Literal(crate::Brush::Gradient(g)) => {
            let mut g = g.clone();
            for s in g.gradient.stops.iter_mut() {
                s.color = s.color.multiply_alpha(k);
            }
            BrushSource::Literal(crate::Brush::Gradient(g))
        }
        other => other.clone(),
    }
}

fn pct(v: f64) -> Option<LengthPct> {
    Some(LengthPct::Percent(v))
}

fn px(v: f64) -> Option<LengthPct> {
    Some(LengthPct::Px(v))
}

impl Out<'_> {
    #[allow(clippy::too_many_arguments)]
    fn push(
        &mut self,
        key: PartKey,
        parent: Option<PartKey>,
        name: String,
        kind: NodeKind,
        at: Point,
        fills: Vec<Fill>,
        insets: Insets,
        item: LayoutItem,
    ) {
        self.parts.push(Part {
            key,
            parent,
            name,
            kind,
            transform: Affine::translate(at.to_vec2()),
            fills,
            strokes: Vec::new(),
            opacity: 1.0,
            insets,
            display: None,
            item,
            clip: false,
        });
    }

    /// An unpainted container frame at `bx` in its parent.
    fn frame(
        &mut self,
        key: PartKey,
        parent: Option<PartKey>,
        name: &str,
        bx: Rect,
        display: Option<Display>,
        item: LayoutItem,
    ) {
        self.frame_pinned(key, parent, name, bx, display, item, Insets::default());
    }

    #[allow(clippy::too_many_arguments)]
    fn frame_pinned(
        &mut self,
        key: PartKey,
        parent: Option<PartKey>,
        name: &str,
        bx: Rect,
        display: Option<Display>,
        item: LayoutItem,
        insets: Insets,
    ) {
        self.push(
            key,
            parent,
            name.to_string(),
            NodeKind::Artboard {
                size: bx.size(),
                corner_radii: RoundedRectRadii::default(),
                corner_smoothing: 0.0,
            },
            bx.origin(),
            Vec::new(),
            insets,
            item,
        );
        self.parts.last_mut().expect("pushed").display = display;
    }

    /// A text label in role `role`'s type, its box's corner at `at`.
    #[allow(clippy::too_many_arguments)]
    fn label(
        &mut self,
        key: PartKey,
        parent: PartKey,
        name: String,
        content: &str,
        role: Role,
        at: Point,
        insets: Insets,
        turn: f64,
    ) {
        let kind = self.text.kind(content, role);
        let ink = self.text.ink(role);
        self.push(
            key,
            Some(parent),
            name,
            kind,
            at,
            fill(ink),
            insets,
            LayoutItem::default(),
        );
        if turn != 0.0 {
            let b = self.text.measure(content, role);
            let p = self.parts.last_mut().expect("pushed");
            // Turned about its box's centre, the way layout turns a placed box.
            p.transform = crate::container::placed_at(at, b, Affine::rotate(turn));
        }
    }

    /// The colour `brush` draws, a bound one read from [`super::Ground::vars`];
    /// `None` when it cannot be told.
    fn colour_of(&self, brush: &BrushSource) -> Option<peniko::Color> {
        match brush {
            BrushSource::Literal(crate::Brush::Solid(c)) => Some(*c),
            BrushSource::Literal(crate::Brush::Gradient(g)) => g
                .gradient
                .stops
                .first()
                .map(|s| s.color.to_alpha_color::<peniko::color::Srgb>()),
            BrushSource::Bound { var, alpha } => self
                .vars
                .iter()
                .find(|(v, _)| v == var)
                .map(|(_, c)| c.multiply_alpha(*alpha)),
            _ => None,
        }
    }

    /// The label just pushed, sitting inside a mark drawn in `mark`, rewritten
    /// in the ground when that contrasts more with the mark than the text ink
    /// does (*Inside ink: Auto*); a tie keeps the ink.
    fn ink_inside(&mut self, o: &super::Options, mark: &BrushSource) {
        if o.values.inside_ink != super::InsideInk::Auto {
            return;
        }
        let ink = self.text.ink(Role::Value);
        let (Some(m), Some(i), Some(g)) = (
            self.colour_of(mark),
            self.colour_of(&ink),
            self.colour_of(&self.ground),
        ) else {
            return;
        };
        let m = super::mix(self.ground_colour, m.with_alpha(1.0), m.components[3]);
        if super::contrast(g, m) > super::contrast(i, m) {
            let ground = self.ground.clone();
            if let Some(p) = self.parts.last_mut() {
                p.fills = fill(ground);
            }
        }
    }

    /// The part [`Self::rect`] just pushed, repainted as a mark ([`mark_paint`]).
    fn as_mark(&mut self, o: &super::Options) {
        let ground = self.ground.clone();
        let p = self.parts.last_mut().expect("pushed");
        if let Some(brush) = p.fills.first().map(|f| f.brush.clone()) {
            (p.fills, p.strokes) = mark_paint(o, &brush, &ground);
        }
    }

    fn rect(&mut self, key: PartKey, parent: PartKey, name: String, bx: Rect, f: Spec) {
        self.push(
            key,
            Some(parent),
            name,
            NodeKind::Rect {
                size: bx.size(),
                corner_radii: f.radii,
                corner_smoothing: 0.0,
            },
            bx.origin(),
            fill(f.fill),
            f.insets,
            LayoutItem::default(),
        );
    }

    /// A gridline across the plot at `share` along the value axis (see
    /// [`rule_at`]): a 1 px rect, or a dashed line when the grid is dashed, since
    /// a dash is a stroke's. The line is flat on one axis, so pinned on both
    /// edges it keeps its place by its near inset.
    #[allow(clippy::too_many_arguments)]
    fn gridline(
        &mut self,
        key: PartKey,
        parent: PartKey,
        name: String,
        horizontal: bool,
        share: f64,
        plot: Size,
        grid: &super::Gridlines,
    ) {
        if !grid.dashed {
            let (bx, insets) = rule_at(horizontal, share, plot, grid.width);
            let radii = RoundedRectRadii::default();
            let fill = set(&grid.colour);
            self.rect(
                key,
                parent,
                name,
                bx,
                Spec {
                    fill,
                    radii,
                    insets,
                },
            );
            return;
        }
        let mut p = kurbo::BezPath::new();
        if horizontal {
            let x = share * plot.width;
            p.move_to((x, 0.0));
            p.line_to((x, plot.height));
        } else {
            let y = (1.0 - share) * plot.height;
            p.move_to((0.0, y));
            p.line_to((plot.width, y));
        }
        let mut stroke = line_stroke(set(&grid.colour), grid.width, super::Dash::Solid);
        // The dash scales with the line, so a thick dashed gridline stays dashed.
        stroke.dashes = GRID_DASH.iter().map(|d| d * grid.width).collect();
        stroke.cap = kurbo::Cap::Butt;
        self.path(key, parent, name, p, plot, Vec::new(), vec![stroke], 1.0);
    }

    /// A path drawn in its parent's space, which is `plot` big: pinned on every
    /// edge by its own box's place as percentages, so it stretches with the plot
    /// (`container::inset_stretch`) and lands exactly where regeneration would.
    #[allow(clippy::too_many_arguments)]
    fn path(
        &mut self,
        key: PartKey,
        parent: PartKey,
        name: String,
        path: kurbo::BezPath,
        plot: Size,
        fills: Vec<Fill>,
        strokes: Vec<Stroke>,
        opacity: f32,
    ) {
        self.path_rounded(
            key,
            parent,
            name,
            (path, Vec::new()),
            plot,
            fills,
            strokes,
            opacity,
        );
    }

    /// [`Self::path`] with a corner radius per anchor, pinned by the box of the
    /// rounded outline it draws, since that is the box layout stretches.
    #[allow(clippy::too_many_arguments)]
    fn path_rounded(
        &mut self,
        key: PartKey,
        parent: PartKey,
        name: String,
        (path, corner_radii): (kurbo::BezPath, Vec<f64>),
        plot: Size,
        fills: Vec<Fill>,
        strokes: Vec<Stroke>,
        opacity: f32,
    ) {
        if path.elements().is_empty() {
            return;
        }
        let b = crate::geometry::round_corners(&path, &corner_radii).bounding_box();
        let (w, h) = (plot.width.max(1e-9), plot.height.max(1e-9));
        let insets = Insets {
            left: pct(b.x0 / w * 100.0),
            right: pct((1.0 - b.x1 / w) * 100.0),
            top: pct(b.y0 / h * 100.0),
            bottom: pct((1.0 - b.y1 / h) * 100.0),
            margin_auto: AutoMargins::default(),
        };
        self.parts.push(Part {
            key,
            parent: Some(parent),
            name,
            kind: NodeKind::Path {
                path,
                corner_radii,
                corner_smoothing: 0.0,
                mirror: crate::symmetry::PathMirror::NONE,
            },
            transform: Affine::IDENTITY,
            fills,
            strokes,
            opacity,
            insets,
            display: None,
            item: LayoutItem::default(),
            clip: false,
        });
    }

    fn legend(&mut self, l: &LegendLayout, at: Point, row: bool, align: super::Align) {
        let k = key(PartRole::Legend, 0, 0);
        let justify = match align {
            super::Align::Start => JustifyContent::Start,
            super::Align::Centre => JustifyContent::Center,
            super::Align::End => JustifyContent::End,
        };
        let display = Display::Flex(Flex {
            direction: if row {
                FlexDirection::Column
            } else {
                FlexDirection::Row
            },
            wrap: if row {
                FlexWrap::NoWrap
            } else {
                FlexWrap::Wrap
            },
            justify_content: justify,
            align_items: AlignItems::Center,
            row_gap: SWATCH_GAP,
            column_gap: LEGEND_GAP,
            ..Flex::default()
        });
        let item = if row {
            LayoutItem {
                width: Dimension::FitContent,
                height: Dimension::Percent(100.0),
                shrink: 0.0,
                ..LayoutItem::default()
            }
        } else {
            LayoutItem {
                width: Dimension::Percent(100.0),
                height: Dimension::FitContent,
                shrink: 0.0,
                ..LayoutItem::default()
            }
        };
        self.frame(
            k,
            None,
            "Legend",
            Rect::from_origin_size(at, l.size),
            Some(display),
            item,
        );
        for it in &l.items {
            let ik = key(PartRole::LegendItem, it.series, 0);
            self.frame(
                ik,
                Some(k),
                &format!("Legend · {}", it.name),
                it.bx,
                Some(Display::Flex(Flex {
                    direction: FlexDirection::Row,
                    align_items: AlignItems::Center,
                    column_gap: SWATCH_GAP,
                    ..Flex::default()
                })),
                LayoutItem {
                    width: Dimension::FitContent,
                    height: Dimension::FitContent,
                    shrink: 0.0,
                    ..LayoutItem::default()
                },
            );
            let sk = key(PartRole::LegendSwatch, it.series, 0);
            let sname = format!("Swatch · {}", it.name);
            let sw = swatch_width(l.swatch);
            match l.swatch {
                super::Swatch::Line => {
                    let mid = it.bx.height() / 2.0;
                    let mut p = kurbo::BezPath::new();
                    p.move_to((0.0, LINE_SWATCH_W / 2.0));
                    p.line_to((sw, LINE_SWATCH_W / 2.0));
                    let mut stroke = line_stroke(it.fill.clone(), LINE_SWATCH_W, it.dash);
                    stroke.cap = kurbo::Cap::Butt;
                    self.path(
                        sk,
                        ik,
                        sname,
                        p,
                        Size::new(sw, LINE_SWATCH_W),
                        Vec::new(),
                        vec![stroke],
                        1.0,
                    );
                    let part = self.parts.last_mut().expect("pushed");
                    part.transform = Affine::translate((0.0, mid - LINE_SWATCH_W / 2.0));
                    // A flex item beside its label, as the rect swatch is.
                    part.insets = Insets::default();
                    part.item = LayoutItem::default();
                }
                shape => {
                    let sy = (it.bx.height() - SWATCH) / 2.0;
                    let r = if shape == super::Swatch::Circle {
                        SWATCH / 2.0
                    } else {
                        2.0
                    };
                    self.rect(
                        sk,
                        ik,
                        sname,
                        Rect::new(0.0, sy, SWATCH, sy + SWATCH),
                        Spec {
                            fill: it.fill.clone(),
                            radii: RoundedRectRadii::from_single_radius(r),
                            insets: Insets::default(),
                        },
                    );
                }
            }
            let ly = (it.bx.height() - it.label.height()) / 2.0 - it.label.y0;
            self.label(
                key(PartRole::LegendLabel, it.series, 0),
                ik,
                format!("Label · {}", it.name),
                &it.name,
                Role::Legend,
                Point::new(sw + SWATCH_GAP - it.label.x0, ly),
                Insets::default(),
                0.0,
            );
        }
    }
}

/// A filled rect's paint, corners and insets.
struct Spec {
    fill: BrushSource,
    radii: RoundedRectRadii,
    insets: Insets,
}

struct LegendItem {
    series: u32,
    name: String,
    fill: BrushSource,
    dash: super::Dash,
    label: Rect,
    bx: Rect,
}

struct LegendLayout {
    size: Size,
    items: Vec<LegendItem>,
    swatch: super::Swatch,
}

/// The legend's items placed in rows that wrap at the width (top or bottom), or
/// in one column (left or right).
fn legend_items(
    entries: &[Entry],
    swatch: super::Swatch,
    text: &Text,
    at: LegendPosition,
    inner: Size,
) -> LegendLayout {
    let column = matches!(at, LegendPosition::Left | LegendPosition::Right);
    let mut items = Vec::new();
    let (mut x, mut y, mut line_h, mut widest) = (0.0f64, 0.0f64, 0.0f64, 0.0f64);
    let swatch_w = swatch_width(swatch);
    for s in entries {
        let label = text.measure(&s.name, Role::Legend);
        let w = swatch_w + SWATCH_GAP + label.width();
        let h = label.height().max(SWATCH);
        if column {
            if !items.is_empty() {
                y += SWATCH_GAP;
            }
            items.push((s, label, Rect::new(0.0, y, w, y + h)));
            y += h;
            widest = widest.max(w);
        } else {
            if x > 0.0 && x + w > inner.width {
                x = 0.0;
                y += line_h + SWATCH_GAP;
                line_h = 0.0;
            }
            items.push((s, label, Rect::new(x, y, x + w, y + h)));
            x += w + LEGEND_GAP;
            line_h = line_h.max(h);
            widest = widest.max(x - LEGEND_GAP);
        }
    }
    let size_box = if column {
        Size::new(widest, y)
    } else {
        Size::new(inner.width, y + line_h)
    };
    LegendLayout {
        size: size_box,
        items: items
            .into_iter()
            .map(|(s, label, bx)| LegendItem {
                series: s.series,
                name: s.name.clone(),
                fill: s.fill.clone(),
                dash: s.dash,
                label,
                bx,
            })
            .collect(),
        swatch,
    }
}

/// A legend swatch's width: a *Line* is longer than it is tall.
fn swatch_width(swatch: super::Swatch) -> f64 {
    match swatch {
        super::Swatch::Line => LINE_SWATCH,
        _ => SWATCH,
    }
}

/// The category count up to which every label is measured. Past it a long
/// axis writes only every so many, and measuring all of them cost most of a
/// generation (10,000 labels took about 90 ms).
const MEASURE_ALL_CATEGORIES: usize = 256;

/// Each category label's box. Past [`MEASURE_ALL_CATEGORIES`] each label's
/// width is estimated as the sum of its characters' advances, the widest by
/// that estimate are measured, and every other label stands in at the widest
/// and tallest of those and of the estimates: the axis's reach and thinning
/// read the largest box, and the labels written are measured again once thinned.
fn category_boxes(names: &[String], measure: impl Fn(&str) -> Rect) -> Vec<Rect> {
    if names.len() <= MEASURE_ALL_CATEGORIES {
        return names.iter().map(|c| measure(c)).collect();
    }
    // A lone space sets no width, so whitespace is measured between letters.
    let between = measure("xx").width();
    let mut advance: rustc_hash::FxHashMap<char, f64> = Default::default();
    let mut tallest: f64 = 0.0;
    let mut estimate = vec![0.0; names.len()];
    for (e, name) in estimate.iter_mut().zip(names) {
        for c in name.chars() {
            *e += *advance.entry(c).or_insert_with(|| {
                let b = if c.is_whitespace() {
                    let b = measure(&format!("x{c}x"));
                    b.with_size((b.width() - between, b.height()))
                } else {
                    measure(c.encode_utf8(&mut [0; 4]))
                };
                tallest = tallest.max(b.height());
                b.width().max(0.0)
            });
        }
    }
    let mut widest_first: Vec<usize> = (0..names.len()).collect();
    widest_first.sort_by(|a, b| estimate[*b].total_cmp(&estimate[*a]));
    let (exact, rest) = widest_first.split_at(64);
    let measured: Vec<(usize, Rect)> = exact.iter().map(|i| (*i, measure(&names[*i]))).collect();
    let widest = measured
        .iter()
        .map(|(_, b)| b.width())
        .chain(rest.iter().map(|i| estimate[*i]))
        .fold(0.0, f64::max);
    let tallest = measured
        .iter()
        .map(|(_, b)| b.height())
        .fold(tallest, f64::max);
    let mut out = vec![Rect::new(0.0, 0.0, widest, tallest); names.len()];
    for (i, b) in measured {
        out[i] = b;
    }
    out
}

/// A value read along a value axis: 0 at its min, 1 at its max.
fn ticks_for(axis: &Axis, lo: f64, hi: f64, length: f64, label_h: f64) -> Ticks {
    let fit = (length / (label_h * 2.2)).floor().max(1.0) as u32;
    let count = axis.tick_count.unwrap_or(5.min(fit));
    let (lo, hi) = if axis.start_at_zero {
        (lo.min(0.0), hi.max(0.0))
    } else {
        (lo, hi)
    };
    scale::ticks(lo, hi, count, axis.range)
}

/// A radar: a square centred in the body as a pie's is, a spoke per category
/// clockwise from the start angle, rings at the value axis's ticks, each series
/// a closed shape through its values (painted at the area opacity when
/// filled), and the category labels outside the outer ring, anchored by angle.
/// Fewer than 3 or more than 36 categories draw nothing and say why.
fn radar(
    chart: &Chart,
    data: &Data,
    body: Size,
    body_at: Point,
    row: bool,
    out: &mut Out,
) -> Result<(), Refusal> {
    use std::f64::consts::{FRAC_PI_2, TAU};
    let o = &chart.options;
    let ro = &o.radar;
    let l = &o.lines;
    let body_key = key(PartRole::Body, 0, 0);
    out.frame(
        body_key,
        None,
        "Body",
        Rect::from_origin_size(body_at, body),
        None,
        body_item(row),
    );
    let n = data.categories.len();
    if n < *super::RADAR_SPOKES.start() {
        out.left_out
            .push("A radar needs at least 3 categories".into());
        return Ok(());
    }
    if n > *super::RADAR_SPOKES.end() {
        out.left_out
            .push(format!("Too many spokes: {n}, at most 36"));
        return Ok(());
    }
    let marks: usize = data.series.len() * n;
    if marks > super::MAX_MARKS {
        return Err(Refusal::TooManyMarks {
            count: marks,
            what: "points",
        });
    }
    let side = body.width.min(body.height).max(0.0);
    let frame = Rect::from_center_size(
        Rect::from_origin_size(Point::ZERO, body).center(),
        (side, side),
    );
    let radar_key = key(PartRole::PieFrame, 0, 0);
    // Centred at its own size, so it stays regular as the body resizes.
    out.frame_pinned(
        radar_key,
        Some(body_key),
        "Radar",
        frame,
        None,
        LayoutItem::default(),
        centred(50.0, 50.0),
    );
    let sq = Size::new(side, side);
    let centre = Point::new(side / 2.0, side / 2.0);
    // The square shrinks until the longest label fits around the outer ring.
    let ca = &o.category_axis;
    let labels_on = ca.show && ca.labels;
    let reach = if labels_on {
        data.categories
            .iter()
            .map(|c| {
                let b = out.text.measure(c, Role::Tick);
                b.width().max(b.height())
            })
            .fold(0.0, f64::max)
            + AXIS_GAP
    } else {
        0.0
    };
    let r = (side / 2.0 - reach).max(side / 4.0);
    let angle = |c: usize| ro.start_angle.to_radians() - FRAC_PI_2 + c as f64 * TAU / n as f64;
    let at = |c: usize, k: f64| {
        let a = angle(c);
        Point::new(centre.x + k * a.cos(), centre.y + k * a.sin())
    };
    // The value axis from the centre out, holding zero unless a range is typed.
    let va = &o.value_axis;
    let (lo, hi) = data
        .series
        .iter()
        .flat_map(|s| s.values.iter().flatten().copied())
        .fold((f64::INFINITY, f64::NEG_INFINITY), |(lo, hi), v| {
            (lo.min(v), hi.max(v))
        });
    let (lo, hi) = if lo.is_finite() { (lo, hi) } else { (0.0, 1.0) };
    let mut axis = va.clone();
    axis.start_at_zero = true;
    let label_h = out.text.measure("0", Role::Tick).height();
    let t = ticks_for(&axis, lo, hi, r, label_h);
    out.extents[0] = Some((t.min, t.max));
    let radius = |v: f64| t.t(v).clamp(0.0, 1.0) * r;

    // The grid: rings at the ticks, spokes out to each category.
    let grid_key = key(PartRole::Grid, 0, 0);
    out.frame_pinned(
        grid_key,
        Some(radar_key),
        "Grid",
        Rect::from_origin_size(Point::ZERO, sq),
        None,
        LayoutItem::default(),
        fill_insets(),
    );
    let ring_stroke = |g: &super::Gridlines| {
        let mut s = line_stroke(set(&g.colour), g.width, super::Dash::Solid);
        if g.dashed {
            s.dashes = GRID_DASH.iter().map(|d| d * g.width).collect();
            s.cap = kurbo::Cap::Butt;
        }
        s
    };
    if va.show && va.grid.show {
        for (i, v) in t.values.iter().enumerate() {
            let k = radius(*v);
            if k <= 0.0 {
                continue;
            }
            let p = match ro.grid {
                super::RadarGrid::Circle => kurbo::Circle::new(centre, k).to_path(0.1),
                super::RadarGrid::Polygon => {
                    let mut p = kurbo::BezPath::new();
                    p.move_to(at(0, k));
                    for c in 1..n {
                        p.line_to(at(c, k));
                    }
                    p.close_path();
                    p
                }
            };
            out.path(
                key(PartRole::Gridline, 0, i as u32),
                grid_key,
                format!("Ring {}", scale::format(*v, &va.format, t.decimals())),
                p,
                sq,
                Vec::new(),
                vec![ring_stroke(&va.grid)],
                1.0,
            );
        }
    }
    // The spokes are the category gridlines, by the radar's own switch.
    if ro.spokes {
        for (c, name) in data.categories.iter().enumerate() {
            let mut p = kurbo::BezPath::new();
            p.move_to(centre);
            p.line_to(at(c, r));
            out.path(
                key(PartRole::Gridline, 1, c as u32),
                grid_key,
                format!("Spoke {name}"),
                p,
                sq,
                Vec::new(),
                vec![ring_stroke(&ca.grid)],
                1.0,
            );
        }
    }

    // Each series a closed shape through its values, joined across a gap.
    for s in &data.series {
        let series_key = key(PartRole::Series, s.column.0, 0);
        out.frame_pinned(
            series_key,
            Some(radar_key),
            &format!("Series {}", s.name),
            Rect::from_origin_size(Point::ZERO, sq),
            None,
            LayoutItem::default(),
            fill_insets(),
        );
        let pts: Vec<(usize, Point)> = (0..n)
            .filter_map(|c| {
                let v = s.values[c]?;
                if v < t.min {
                    out.left_out
                        .push(format!("{}, row {}: below the axis's start", s.name, c + 1));
                }
                Some((c, at(c, radius(v))))
            })
            .collect();
        if pts.len() < 2 {
            continue;
        }
        let mut p = kurbo::BezPath::new();
        p.move_to(pts[0].1);
        for (_, q) in &pts[1..] {
            p.line_to(*q);
        }
        p.close_path();
        if ro.filled {
            out.path(
                key(PartRole::Area, s.column.0, 0),
                series_key,
                format!("Area · {}", s.name),
                p.clone(),
                sq,
                fill(s.fill.clone()),
                Vec::new(),
                l.area_opacity as f32,
            );
        }
        let dash = chart
            .series
            .iter()
            .find(|st| st.column == s.column)
            .map_or(super::Dash::Solid, |st| st.dash);
        out.path(
            key(PartRole::Line, s.column.0, 0),
            series_key,
            format!("Line · {}", s.name),
            p,
            sq,
            Vec::new(),
            vec![line_stroke(s.fill.clone(), l.width, dash)],
            1.0,
        );
        if l.points != super::PointStyle::None {
            for (c, q) in &pts {
                let bx = Rect::from_center_size(*q, (l.point_size, l.point_size));
                let (fills, strokes) = if l.points == super::PointStyle::Hollow {
                    (
                        fill(out.ground.clone()),
                        vec![line_stroke(
                            s.fill.clone(),
                            l.width.min(l.point_size / 3.0),
                            super::Dash::Solid,
                        )],
                    )
                } else {
                    mark_paint(o, &s.fill, &out.ground)
                };
                let shape = chart
                    .series
                    .iter()
                    .find(|st| st.column == s.column)
                    .and_then(|st| st.point)
                    .unwrap_or_default();
                out.parts.push(Part {
                    key: key(PartRole::Point, s.column.0, *c as u32),
                    parent: Some(series_key),
                    name: format!("{} · {}", s.name, data.categories[*c]),
                    kind: point_kind(shape, bx.size()),
                    transform: Affine::translate(bx.origin().to_vec2()),
                    fills,
                    strokes,
                    opacity: 1.0,
                    insets: centred(q.x / side.max(1e-9) * 100.0, q.y / side.max(1e-9) * 100.0),
                    display: None,
                    item: LayoutItem::default(),
                    clip: false,
                });
            }
        }
    }

    // Category labels outside the outer ring, anchored by angle: start on
    // the right, end on the left, centred at the top and bottom.
    if labels_on {
        let ck = key(PartRole::CategoryAxis, 0, 0);
        out.frame_pinned(
            ck,
            Some(radar_key),
            "Category labels",
            Rect::from_origin_size(Point::ZERO, sq),
            None,
            LayoutItem::default(),
            fill_insets(),
        );
        for (c, name) in data.categories.iter().enumerate() {
            let b = out.text.measure(name, Role::Tick);
            let a = angle(c);
            let p = at(c, r + AXIS_GAP);
            let x = if a.cos() > 0.3 {
                p.x
            } else if a.cos() < -0.3 {
                p.x - b.width()
            } else {
                p.x - b.width() / 2.0
            };
            let y = if a.sin() > 0.3 {
                p.y
            } else if a.sin() < -0.3 {
                p.y - b.height()
            } else {
                p.y - b.height() / 2.0
            };
            let lb = Rect::from_origin_size(Point::new(x, y), b.size());
            out.label(
                key(PartRole::CategoryLabel, 0, c as u32),
                ck,
                format!("Category {name}"),
                name,
                Role::Tick,
                Point::new(x - b.x0, y - b.y0),
                centred(
                    lb.center().x / side.max(1e-9) * 100.0,
                    lb.center().y / side.max(1e-9) * 100.0,
                ),
                0.0,
            );
        }
    }
    // Ring labels up the first spoke.
    if va.show && va.labels {
        let vk = key(PartRole::ValueAxis, 0, 0);
        out.frame_pinned(
            vk,
            Some(radar_key),
            "Value axis",
            Rect::from_origin_size(Point::ZERO, sq),
            None,
            LayoutItem::default(),
            fill_insets(),
        );
        for (i, v) in t.values.iter().enumerate() {
            let s = scale::format(*v, &va.format, t.decimals());
            let b = out.text.measure(&s, Role::Tick);
            let p = at(0, radius(*v));
            let (x, y) = (p.x + VALUE_GAP, p.y - b.height() / 2.0);
            let lb = Rect::from_origin_size(Point::new(x, y), b.size());
            out.label(
                key(PartRole::TickLabel, 0, i as u32),
                vk,
                format!("Tick {s}"),
                &s,
                Role::Tick,
                Point::new(x - b.x0, y - b.y0),
                centred(
                    lb.center().x / side.max(1e-9) * 100.0,
                    lb.center().y / side.max(1e-9) * 100.0,
                ),
                0.0,
            );
        }
    }
    Ok(())
}

/// A waterfall step's colour: set, else its starting colour for the theme drawn.
fn step_fill(chart: &Chart, step: super::Step) -> BrushSource {
    let w = &chart.options.waterfall;
    let own = match step {
        super::Step::Increase => &w.increase,
        super::Step::Decrease => &w.decrease,
        super::Step::Total => &w.total,
    };
    own.clone().unwrap_or_else(|| {
        BrushSource::Literal(crate::Brush::Solid(super::Waterfall::starting(
            step,
            chart.drawn_dark(),
        )))
    })
}

/// One waterfall bar: its name, its kind, where it runs from and to, and the
/// change it writes (a total its running sum).
struct WalkStep {
    name: String,
    step: super::Step,
    from: f64,
    to: f64,
}

/// A waterfall's walk over the first series shown: each step from the running
/// total to the total plus its value, each named total from zero to the
/// running total (its own cell ignored, and said when it disagrees), and the
/// final total the chart adds after the last row.
fn walk(chart: &Chart, data: &Data, left_out: &mut Vec<String>) -> Vec<WalkStep> {
    let w = &chart.options.waterfall;
    let Some(s) = data.series.first() else {
        return Vec::new();
    };
    let mut running = 0.0;
    let mut steps = Vec::new();
    for (c, name) in data.categories.iter().enumerate() {
        if w.totals.iter().any(|t| t == name) {
            if let Some(v) = s.values[c]
                && (v - running).abs() > 1e-9 * v.abs().max(1.0)
            {
                let f = &chart.options.values.format;
                let d = |x: f64| scale::format(x, f, scale::decimals_of(x).min(3));
                left_out.push(format!(
                    "{name}: the table says {}, the steps sum to {}",
                    d(v),
                    d(running)
                ));
            }
            steps.push(WalkStep {
                name: name.clone(),
                step: super::Step::Total,
                from: 0.0,
                to: running,
            });
            continue;
        }
        let Some(v) = s.values[c] else { continue };
        steps.push(WalkStep {
            name: name.clone(),
            step: if v >= 0.0 {
                super::Step::Increase
            } else {
                super::Step::Decrease
            },
            from: running,
            to: running + v,
        });
        running += v;
    }
    if let Some(name) = w.final_total.as_ref().filter(|n| !n.trim().is_empty()) {
        steps.push(WalkStep {
            name: name.clone(),
            step: super::Step::Total,
            from: 0.0,
            to: running,
        });
    }
    steps
}

/// A waterfall: the walk's bars on a band axis, each in its step's colour,
/// connectors from each bar's end to the next one's start, and value labels
/// signed for a step and plain for a total.
fn waterfall(
    chart: &Chart,
    data: &Data,
    body: Size,
    body_at: Point,
    out: &mut Out,
) -> Result<(), Refusal> {
    let o = &chart.options;
    let w = &o.waterfall;
    let horizontal = w.horizontal;
    let steps = walk(chart, data, &mut out.left_out);
    if steps.len() > super::MAX_MARKS {
        return Err(Refusal::TooManyMarks {
            count: steps.len(),
            what: "steps",
        });
    }
    if data.series.len() > 1 {
        let name = &data.series[0].name;
        out.left_out.push(format!("Draws {name} only"));
    }
    let (lo, hi) = steps
        .iter()
        .flat_map(|s| [s.from, s.to])
        .fold((0.0f64, 0.0f64), |(lo, hi), v| (lo.min(v), hi.max(v)));
    // The axes read the walk's names as categories and its ends as values.
    let walked = Data {
        categories: steps.iter().map(|s| s.name.clone()).collect(),
        series: data
            .series
            .first()
            .map(|s| SeriesData {
                column: s.column,
                name: s.name.clone(),
                fill: s.fill.clone(),
                values: steps.iter().map(|st| Some(st.to - st.from)).collect(),
            })
            .into_iter()
            .collect(),
        left_out: Vec::new(),
        by_category: Vec::new(),
    };
    let spec = AxisSpec {
        horizontal,
        lo,
        hi,
        zero: true,
        percent: false,
        x: None,
        ends: Vec::new(),
    };
    let ax = axes(chart, &walked, body, body_at, out, spec);
    ax.reference_bands(chart, &walked, out);
    let (plot, plot_key, across_len) = (ax.plot, ax.plot_key, ax.across_len.max(1e-9));
    let col = data.series.first().map_or(0, |s| s.column.0);
    let series_key = key(PartRole::Series, col, 0);
    out.frame_pinned(
        series_key,
        Some(plot_key),
        "Steps",
        Rect::from_origin_size(Point::ZERO, plot),
        None,
        LayoutItem::default(),
        fill_insets(),
    );
    let band = ax.band;
    let group = band * (1.0 - (o.bars.group_gap / 100.0).clamp(0.0, 0.95));
    // A step's box, as shares of the plot (across, along) and in plot units.
    let place = |c: usize, a: f64, b: f64| -> (Rect, Insets) {
        let a0 = c as f64 * band + (band - group) / 2.0;
        let a1 = a0 + group;
        let (s0, s1) = (ax.t.t(a.min(b)), ax.t.t(a.max(b)));
        let (x0, x1, y0, y1) = if horizontal {
            (s0, s1, a0 / across_len, a1 / across_len)
        } else {
            (a0 / across_len, a1 / across_len, 1.0 - s1, 1.0 - s0)
        };
        (
            Rect::new(
                x0 * plot.width,
                y0 * plot.height,
                x1 * plot.width,
                y1 * plot.height,
            ),
            Insets {
                left: pct(x0 * 100.0),
                right: pct((1.0 - x1) * 100.0),
                top: pct(y0 * 100.0),
                bottom: pct((1.0 - y1) * 100.0),
                margin_auto: AutoMargins::default(),
            },
        )
    };
    let mut boxes = Vec::new();
    for (c, s) in steps.iter().enumerate() {
        let (bx, insets) = place(c, s.from, s.to);
        let r = o
            .bars
            .radius
            .min(bx.width() / 2.0)
            .min(bx.height() / 2.0)
            .max(0.0);
        out.rect(
            key(PartRole::Bar, col, c as u32),
            series_key,
            format!("Step {}", s.name),
            bx,
            Spec {
                fill: step_fill(chart, s.step),
                radii: RoundedRectRadii::from_single_radius(r),
                insets,
            },
        );
        out.as_mark(o);
        // From the step's far end to its near one, as a bar fades to its baseline.
        let up = s.to >= s.from;
        let (w, h) = (bx.width(), bx.height());
        let (full, faint) = match (horizontal, up) {
            (true, true) => (Point::new(w, 0.0), Point::ZERO),
            (true, false) => (Point::ZERO, Point::new(w, 0.0)),
            (false, true) => (Point::ZERO, Point::new(0.0, h)),
            (false, false) => (Point::new(0.0, h), Point::ZERO),
        };
        fade_last(out, o, full, faint, &s.name);
        boxes.push(bx);
    }
    // Connectors: from each bar's end across to the next bar, at that end.
    if w.connectors && steps.len() > 1 {
        let k = key(PartRole::Connectors, 0, 0);
        out.frame_pinned(
            k,
            Some(plot_key),
            "Connectors",
            Rect::from_origin_size(Point::ZERO, plot),
            None,
            LayoutItem::default(),
            fill_insets(),
        );
        let ink = set(&o.value_axis.colour);
        for i in 0..steps.len() - 1 {
            let end = steps[i].to;
            let (a, b) = (boxes[i], boxes[i + 1]);
            let mut p = kurbo::BezPath::new();
            if horizontal {
                let x = ax.t.t(end) * plot.width;
                p.move_to((x, a.y1));
                p.line_to((x, b.y0));
            } else {
                let y = (1.0 - ax.t.t(end)) * plot.height;
                p.move_to((a.x1, y));
                p.line_to((b.x0, y));
            }
            let mut stroke = line_stroke(ink.clone(), 1.0, super::Dash::Solid);
            if w.dashed {
                stroke.dashes = GRID_DASH.to_vec();
            }
            stroke.cap = kurbo::Cap::Butt;
            out.path(
                key(PartRole::Connector, 0, i as u32),
                k,
                format!("Connector {}", i + 1),
                p,
                plot,
                Vec::new(),
                vec![stroke],
                1.0,
            );
        }
    }
    // Values: a step signed, a total plain.
    if o.values.position != ValuePosition::Off {
        let values_key = key(PartRole::Values, 0, 0);
        out.frame_pinned(
            values_key,
            Some(plot_key),
            "Values",
            Rect::from_origin_size(Point::ZERO, plot),
            None,
            LayoutItem::default(),
            fill_insets(),
        );
        for (c, s) in steps.iter().enumerate() {
            let total = s.step == super::Step::Total;
            let v = if total { s.to } else { s.to - s.from };
            let mut text = scale::format(v, &o.values.format, scale::decimals_of(v).min(3));
            if !total && v > 0.0 {
                text.insert(0, '+');
            }
            let b = out.text.measure(&text, Role::Value);
            let up = s.to >= s.from;
            let Some((at, insets)) =
                value_place(o.values.position, horizontal, up, boxes[c], b, plot)
            else {
                continue;
            };
            out.label(
                key(PartRole::ValueLabel, col, c as u32),
                values_key,
                format!("Value {text}"),
                &text,
                Role::Value,
                at,
                insets,
                0.0,
            );
            if matches!(
                o.values.position,
                ValuePosition::Inside | ValuePosition::End
            ) {
                out.ink_inside(o, &step_fill(chart, s.step));
            }
        }
    }
    ax.reference_lines(chart, &walked, out);
    ax.category_labels(&walked, out);
    Ok(())
}

/// Column and bar charts: the body grid with its two axes, the plot's grid,
/// bars and value labels.
fn bars(
    chart: &Chart,
    data: &Data,
    body: Size,
    body_at: Point,
    out: &mut Out,
) -> Result<(), Refusal> {
    let o = &chart.options;
    let horizontal = chart.kind == ChartKind::Bar;
    let stacking = o.bars.stacking;
    let n_cat = data.categories.len();
    let n_ser = data.series.len();
    let marks = n_cat * n_ser;
    if marks > super::MAX_MARKS {
        return Err(Refusal::TooManyMarks {
            count: marks,
            what: "bars",
        });
    }
    // Each category is a band of its own, drawn with no series too.
    if n_cat > super::MAX_MARKS {
        return Err(Refusal::TooManyMarks {
            count: n_cat,
            what: "categories",
        });
    }

    // Each category's extent: stacked positives up and negatives down, apart
    // the values themselves.
    let mut lo = 0.0f64;
    let mut hi = 0.0f64;
    let totals: Vec<f64> = (0..n_cat)
        .map(|c| {
            data.series
                .iter()
                .filter_map(|s| s.values[c])
                .map(f64::abs)
                .sum::<f64>()
        })
        .collect();
    let shown = |c: usize, v: f64| -> f64 {
        match stacking {
            Stacking::Percent if totals[c] > 0.0 => v / totals[c],
            _ => v,
        }
    };
    for c in 0..n_cat {
        let (mut up, mut down) = (0.0, 0.0);
        for s in &data.series {
            let Some(v) = s.values[c] else { continue };
            let v = shown(c, v);
            match stacking {
                Stacking::Apart => {
                    lo = lo.min(v);
                    hi = hi.max(v);
                }
                _ => {
                    if v >= 0.0 {
                        up += v;
                    } else {
                        down += v;
                    }
                }
            }
        }
        lo = lo.min(down);
        hi = hi.max(up);
    }
    if stacking == Stacking::Percent {
        hi = if hi > 0.0 { 1.0 } else { 0.0 };
        lo = if lo < 0.0 { -1.0 } else { 0.0 };
    }
    let spec = AxisSpec {
        horizontal,
        lo,
        hi,
        // A bar's length is its value, so its axis always holds zero.
        zero: true,
        percent: stacking == Stacking::Percent,
        x: None,
        ends: Vec::new(),
    };
    let ax = axes(chart, data, body, body_at, out, spec);
    ax.reference_bands(chart, data, out);
    bar_marks(chart, data, &ax, &totals, out);
    ax.reference_lines(chart, data, out);
    ax.category_labels(data, out);
    Ok(())
}

/// What a value axis spans and how it reads: across the plot for a bar chart,
/// up it otherwise; whether it must hold zero; whether it writes shares.
struct AxisSpec {
    horizontal: bool,
    lo: f64,
    hi: f64,
    zero: bool,
    percent: bool,
    /// A scatter's X domain: the category axis read as numbers.
    x: Option<(f64, f64)>,
    /// Labels at the lines' ends (`Legend::line_ends`): each series' column,
    /// its words, the value its line ends at and its colour.
    ends: Vec<(u32, String, f64, BrushSource)>,
}

/// The body grid built: the plot and what the marks need of it, and what the
/// category labels are drawn with.
struct Axes {
    horizontal: bool,
    t: Ticks,
    plot: Size,
    plot_key: PartKey,
    /// The plot's length across the categories, and one category's band of it.
    across_len: f64,
    band: f64,
    ca_show: bool,
    every: usize,
    cat_boxes: Vec<Rect>,
    turn: f64,
    side_box: Rect,
    under_box: Rect,
    left_at_end: bool,
    bottom_at_top: bool,
    /// The category labels' distance from the plot, past any tick marks.
    ca_gap: f64,
    /// The category axis's tick mark colour, when it draws them.
    ca_ticks: Option<(BrushSource, f64)>,
    /// A scatter's X ticks and their text; `None` reads categories.
    x: Option<Ticks>,
    x_text: Vec<String>,
    /// The value axis's number format, as its ticks write.
    value_fmt: super::NumberFormat,
}

/// A statistic of the values drawn: of series `of`, else of every series shown;
/// a stacked chart's of each stack's total. `None` with no value.
fn stat_of(chart: &Chart, data: &Data, stat: super::Stat, of: Option<ColumnId>) -> Option<f64> {
    let stacked = match chart.kind {
        ChartKind::Column | ChartKind::Bar => chart.options.bars.stacking == Stacking::Stacked,
        ChartKind::Area => chart.options.lines.area_stacking == Stacking::Stacked,
        _ => false,
    };
    let mut vs: Vec<f64> = match of {
        Some(col) => data
            .series
            .iter()
            .filter(|s| s.column == col)
            .flat_map(|s| s.values.iter().flatten().copied())
            .collect(),
        None if stacked => (0..data.categories.len())
            .filter_map(|c| {
                let v: Vec<f64> = data.series.iter().filter_map(|s| s.values[c]).collect();
                (!v.is_empty()).then(|| v.iter().sum())
            })
            .collect(),
        None => data
            .series
            .iter()
            .flat_map(|s| s.values.iter().flatten().copied())
            .collect(),
    };
    if vs.is_empty() {
        return None;
    }
    vs.sort_by(f64::total_cmp);
    Some(match stat {
        super::Stat::Average => vs.iter().sum::<f64>() / vs.len() as f64,
        super::Stat::Median => {
            let n = vs.len();
            if n % 2 == 1 {
                vs[n / 2]
            } else {
                (vs[n / 2 - 1] + vs[n / 2]) / 2.0
            }
        }
        super::Stat::Min => vs[0],
        super::Stat::Max => vs[vs.len() - 1],
    })
}

impl Axes {
    /// Where `a` stands along reference axis `axis`, as a share of the plot
    /// from the axis's start, and the value or name it writes; `None` when the
    /// data gives none.
    fn ref_share(
        &self,
        chart: &Chart,
        data: &Data,
        axis: super::RefAxis,
        a: &super::At,
    ) -> Option<(f64, String)> {
        use super::At;
        match (axis, a) {
            (_, At::AxisStart) => Some((0.0, String::new())),
            (_, At::AxisEnd) => Some((1.0, String::new())),
            (super::RefAxis::Value, At::Value(v)) => Some((self.t.t(*v), self.value_text(*v))),
            (super::RefAxis::Value, At::Stat { stat, of }) => {
                let v = stat_of(chart, data, *stat, *of)?;
                Some((self.t.t(v), self.value_text(v)))
            }
            (super::RefAxis::Category, At::Value(v)) => {
                let xt = self.x.as_ref()?;
                Some((
                    xt.t(*v),
                    scale::format(*v, &chart.options.category_axis.format, xt.decimals()),
                ))
            }
            (super::RefAxis::Category, At::Category(name)) => {
                let c = data.categories.iter().position(|n| n == name)?;
                let along = (c as f64 + 0.5) * self.band / self.across_len.max(1e-9);
                // Down a bar chart's category axis from the top.
                Some((
                    if self.horizontal { 1.0 - along } else { along },
                    name.clone(),
                ))
            }
            _ => None,
        }
    }

    fn value_text(&self, v: f64) -> String {
        let d = if self.value_fmt.percent {
            scale::decimals_of(self.t.step * 100.0)
        } else {
            self.t.decimals()
        };
        scale::format(v, &self.value_fmt, d)
    }

    /// Whether a reference on `axis` runs up the plot (a vertical line).
    fn ref_vertical(&self, axis: super::RefAxis) -> bool {
        (axis == super::RefAxis::Value) == self.horizontal
    }

    /// The chart's reference bands, under the marks, in a *References* frame.
    fn reference_bands(&self, chart: &Chart, data: &Data, out: &mut Out) {
        let bands: Vec<&super::Reference> = chart
            .references
            .iter()
            .filter(|r| r.visible && r.kind == super::RefKind::Band)
            .collect();
        if bands.is_empty() {
            return;
        }
        let plot = self.plot;
        let frame = key(PartRole::References, 0, 0);
        out.frame_pinned(
            frame,
            Some(self.plot_key),
            "References",
            Rect::from_origin_size(Point::ZERO, plot),
            None,
            LayoutItem::default(),
            fill_insets(),
        );
        for r in bands {
            let (Some((a, ta)), Some((b, tb))) = (
                self.ref_share(chart, data, r.axis, &r.at),
                self.ref_share(chart, data, r.axis, &r.to),
            ) else {
                continue;
            };
            let (s0, s1) = (a.min(b).clamp(0.0, 1.0), a.max(b).clamp(0.0, 1.0));
            if s1 <= s0 {
                continue;
            }
            let (x0, x1, y0, y1) = if self.ref_vertical(r.axis) {
                (s0, s1, 0.0, 1.0)
            } else {
                (0.0, 1.0, 1.0 - s1, 1.0 - s0)
            };
            let bx = Rect::new(
                x0 * plot.width,
                y0 * plot.height,
                x1 * plot.width,
                y1 * plot.height,
            );
            out.rect(
                key(PartRole::Band, 0, r.id),
                frame,
                format!("Band · {}", r.label),
                bx,
                Spec {
                    fill: tinted(&r.colour, BAND_TINT),
                    radii: RoundedRectRadii::default(),
                    insets: Insets {
                        left: pct(x0 * 100.0),
                        right: pct((1.0 - x1) * 100.0),
                        top: pct(y0 * 100.0),
                        bottom: pct((1.0 - y1) * 100.0),
                        margin_auto: AutoMargins::default(),
                    },
                },
            );
            // A band's `{value}` writes both ends, an axis end left out.
            let ends: Vec<&str> = [&ta, &tb]
                .into_iter()
                .map(String::as_str)
                .filter(|t| !t.is_empty())
                .collect();
            let words = r.label.replace("{value}", &ends.join("–"));
            self.reference_label(r, &words, bx, true, out, frame);
        }
    }

    /// The chart's reference lines over the marks, in a *Reference lines*
    /// frame, each with its label; a line outside the axis's range is left out
    /// and said.
    fn reference_lines(&self, chart: &Chart, data: &Data, out: &mut Out) {
        let lines: Vec<&super::Reference> = chart
            .references
            .iter()
            .filter(|r| r.visible && r.kind == super::RefKind::Line)
            .collect();
        if lines.is_empty() {
            return;
        }
        let plot = self.plot;
        let frame = key(PartRole::References, 1, 0);
        out.frame_pinned(
            frame,
            Some(self.plot_key),
            "Reference lines",
            Rect::from_origin_size(Point::ZERO, plot),
            None,
            LayoutItem::default(),
            fill_insets(),
        );
        for r in lines {
            let Some((s, text)) = self.ref_share(chart, data, r.axis, &r.at) else {
                continue;
            };
            let name = if r.label.trim().is_empty() {
                "Reference".to_string()
            } else {
                r.label.replace("{value}", "").trim().to_string()
            };
            if !(-1e-9..=1.0 + 1e-9).contains(&s) {
                out.left_out
                    .push(format!("{name} is outside the axis's range"));
                continue;
            }
            let vertical = self.ref_vertical(r.axis);
            let k = key(PartRole::Reference, 0, r.id);
            let (bx, insets) = rule_at(vertical, s, plot, r.width.max(0.5));
            if r.dash == super::Dash::Solid {
                out.rect(
                    k,
                    frame,
                    format!("Reference · {name}"),
                    bx,
                    Spec {
                        fill: r.colour.clone(),
                        radii: RoundedRectRadii::default(),
                        insets,
                    },
                );
            } else {
                let mut p = kurbo::BezPath::new();
                if vertical {
                    p.move_to((s * plot.width, 0.0));
                    p.line_to((s * plot.width, plot.height));
                } else {
                    let y = (1.0 - s) * plot.height;
                    p.move_to((0.0, y));
                    p.line_to((plot.width, y));
                }
                let mut stroke = line_stroke(r.colour.clone(), r.width, r.dash);
                stroke.cap = kurbo::Cap::Butt;
                out.path(
                    k,
                    frame,
                    format!("Reference · {name}"),
                    p,
                    plot,
                    Vec::new(),
                    vec![stroke],
                    1.0,
                );
            }
            let words = r.label.replace("{value}", &text);
            self.reference_label(r, &words, bx, false, out, frame);
        }
    }

    /// A reference's label in its colour and the value labels' type: along a
    /// line on the side away from the plot's edge, at its start, middle or end;
    /// inside a band at its top-left. Empty writes none.
    fn reference_label(
        &self,
        r: &super::Reference,
        words: &str,
        bx: Rect,
        band: bool,
        out: &mut Out,
        parent: PartKey,
    ) {
        let words = words.trim();
        if words.is_empty() {
            return;
        }
        let plot = self.plot;
        let b = out.text.measure(words, Role::Value);
        let (w, h) = (b.width(), b.height());
        let pad = VALUE_GAP;
        let vertical = bx.width() < bx.height() && !band;
        let (x, y) = if band {
            (bx.x0 + pad, bx.y0 + pad)
        } else if vertical {
            let y = match r.label_at {
                super::LabelAt::End => pad,
                super::LabelAt::Middle => (plot.height - h) / 2.0,
                super::LabelAt::Start => plot.height - h - pad,
            };
            // Right of the line, unless that runs past the plot.
            let x = if bx.x1 + pad + w <= plot.width {
                bx.x1 + pad
            } else {
                bx.x0 - pad - w
            };
            (x, y)
        } else {
            let x = match r.label_at {
                super::LabelAt::End => plot.width - w - pad,
                super::LabelAt::Middle => (plot.width - w) / 2.0,
                super::LabelAt::Start => pad,
            };
            // Above the line, unless that runs past the plot's top.
            let y = if bx.y0 - pad - h >= 0.0 {
                bx.y0 - pad - h
            } else {
                bx.y1 + pad
            };
            (x, y)
        };
        let lb = Rect::from_origin_size(Point::new(x, y), Size::new(w, h));
        out.label(
            key(PartRole::ReferenceLabel, 0, r.id),
            parent,
            format!("Label · {words}"),
            words,
            Role::Value,
            Point::new(x - b.x0, y - b.y0),
            centred(
                lb.center().x / plot.width.max(1e-9) * 100.0,
                lb.center().y / plot.height.max(1e-9) * 100.0,
            ),
            0.0,
        );
        if let Some(p) = out.parts.last_mut() {
            p.fills = fill(r.colour.clone());
        }
    }
}

/// The share of its colour a reference band is filled with.
const BAND_TINT: f32 = 0.07;

/// A categorical chart's body: its grid with the value axis's track and labels,
/// the category axis's track, and the plot with its gridlines and baseline.
fn axes(
    chart: &Chart,
    data: &Data,
    body: Size,
    body_at: Point,
    out: &mut Out,
    spec: AxisSpec,
) -> Axes {
    let o = &chart.options;
    let AxisSpec {
        horizontal,
        lo,
        hi,
        zero,
        percent,
        x,
        ends,
    } = spec;
    let n_cat = data.categories.len();
    // The line-end labels' track past the plot: the widest label and a gap.
    let end_w = ends
        .iter()
        .map(|(_, s, ..)| out.text.measure(s, Role::Legend).width() + AXIS_GAP)
        .fold(0.0, f64::max);
    // The value axis's labels decide its thickness, and the category labels
    // the other axis's.
    let va = &o.value_axis;
    let ca = &o.category_axis;
    let label_h = out.text.measure("0", Role::Tick).height();
    let mut value_fmt = va.format.clone();
    if percent {
        value_fmt.percent = true;
    }
    // A first guess of the plot's length along the value axis, for the count.
    let along = if horizontal { body.width } else { body.height } * 0.8;
    let mut axis_va = va.clone();
    axis_va.start_at_zero = zero || va.start_at_zero;
    // An Auto range takes in each typed value-axis reference that asks to be.
    let (mut lo, mut hi) = (lo, hi);
    if va.range.is_none() {
        for r in chart
            .references
            .iter()
            .filter(|r| r.visible && r.in_range && r.axis == super::RefAxis::Value)
        {
            for a in [&r.at, &r.to] {
                if let super::At::Value(v) = a
                    && (r.kind == super::RefKind::Band || std::ptr::eq(a, &r.at))
                {
                    lo = lo.min(*v);
                    hi = hi.max(*v);
                }
            }
        }
    }
    let t = ticks_for(&axis_va, lo, hi, along, label_h);
    out.extents[0] = Some((t.min, t.max));
    let tick_text: Vec<String> = t
        .values
        .iter()
        .map(|v| {
            // A percent axis counts places in the shares it writes, not the step.
            let d = if value_fmt.percent {
                scale::decimals_of(t.step * 100.0)
            } else {
                t.decimals()
            };
            scale::format(*v, &value_fmt, d)
        })
        .collect();
    let tick_boxes: Vec<Rect> = tick_text
        .iter()
        .map(|s| out.text.measure(s, Role::Tick))
        .collect();
    let va_show = va.show && va.labels;
    let ca_show = ca.show && ca.labels;
    // Tick marks stand between the plot and the labels, pushing them out.
    let va_tick = if va.show && va.ticks { TICK_LEN } else { 0.0 };
    let ca_tick = if ca.show && ca.ticks { TICK_LEN } else { 0.0 };
    let (va_gap, ca_gap) = (AXIS_GAP + va_tick, AXIS_GAP + ca_tick);
    let va_turn = set_turn(va.angle);
    let va_thick = if !va_show {
        va_tick
    } else if horizontal && va_turn == 0.0 {
        label_h + va_gap
    } else {
        tick_boxes
            .iter()
            .map(|b| {
                let (across, down) = turned_reach(*b, va_turn);
                if horizontal { down } else { across }
            })
            .fold(0.0, f64::max)
            + va_gap
    };
    // A title's text and box, in its own track past its axis's labels.
    let title_of = |a: &Axis| {
        let t = a.title.trim();
        (a.show && !t.is_empty()).then(|| (t.to_string(), out.text.measure(t, Role::Title)))
    };
    let (va_title, ca_title) = (title_of(va), title_of(ca));
    let title_thick =
        |t: &Option<(String, Rect)>| t.as_ref().map_or(0.0, |(_, b)| b.height() + AXIS_GAP);

    // A scatter's X axis is numbers on ticks, spaced for labels four high.
    let x_ticks = x.map(|(lo, hi)| {
        let fit = ((body.width - va_thick) / (label_h * 4.0)).floor().max(1.0) as u32;
        let count = ca.tick_count.unwrap_or(5.min(fit));
        let (lo, hi) = if ca.start_at_zero {
            (lo.min(0.0), hi.max(0.0))
        } else {
            (lo, hi)
        };
        scale::ticks(lo, hi, count, ca.range)
    });
    out.extents[1] = x_ticks.as_ref().map(|t| (t.min, t.max));
    let x_text: Vec<String> = x_ticks.as_ref().map_or_else(Vec::new, |xt| {
        xt.values
            .iter()
            .map(|v| scale::format(*v, &ca.format, xt.decimals()))
            .collect()
    });
    // Category labels: flat, or turned and thinned where they would touch.
    let mut cat_boxes: Vec<Rect> = match &x_ticks {
        Some(_) => x_text
            .iter()
            .map(|s| out.text.measure(s, Role::Tick))
            .collect(),
        None => category_boxes(&data.categories, |c| out.text.measure(c, Role::Tick)),
    };
    let band_len_guess = if horizontal {
        body.height
    } else {
        body.width - va_thick
    } / n_cat.max(1) as f64;
    let (turn, every) = if x_ticks.is_some() {
        (set_turn(ca.angle), 1)
    } else if horizontal {
        // Beside the plot a turned label takes more of its band, so it thins
        // by how far it reaches down.
        let turn = set_turn(ca.angle);
        let need = if turn == 0.0 {
            label_h * 1.2
        } else {
            cat_boxes
                .iter()
                .map(|b| turned_reach(*b, turn).1)
                .fold(0.0, f64::max)
                + 4.0
        };
        (turn, thin_every(need, band_len_guess))
    } else {
        category_turn(ca.angle, &cat_boxes, band_len_guess, label_h)
    };
    // The labels written are measured exactly, wherever `category_boxes`
    // stood in its widest guess.
    if x_ticks.is_none() && data.categories.len() > MEASURE_ALL_CATEGORIES {
        for (c, name) in data.categories.iter().enumerate().step_by(every.max(1)) {
            cat_boxes[c] = out.text.measure(name, Role::Tick);
        }
    }
    let ca_thick = if !ca_show {
        ca_tick
    } else {
        cat_boxes
            .iter()
            .map(|b| {
                let (across, down) = turned_reach(*b, turn);
                if horizontal { across } else { down }
            })
            .fold(0.0, f64::max)
            + ca_gap
    };

    // The body grid: the value axis beside the plot, the category axis under it
    // (a bar chart has them the other way round), each title in a track past
    // its axis's. An axis frame spans its title's track too.
    let (side_title, under_title) = if horizontal {
        (&ca_title, &va_title)
    } else {
        (&va_title, &ca_title)
    };
    let (side_axis_w, under_axis_h) = if horizontal {
        (ca_thick, va_thick)
    } else {
        (va_thick, ca_thick)
    };
    let (side_title_w, under_title_h) = (title_thick(side_title), title_thick(under_title));
    let (left_w, bottom_h) = (side_axis_w + side_title_w, under_axis_h + under_title_h);
    let left_at_end = if horizontal {
        ca.side == AxisSide::End
    } else {
        va.side == AxisSide::End
    };
    let bottom_at_top = if horizontal {
        va.side == AxisSide::End
    } else {
        ca.side == AxisSide::End
    };
    let plot = Size::new(
        (body.width - left_w - end_w).max(0.0),
        (body.height - bottom_h).max(0.0),
    );
    let px_track = |v: f64| Track::Size(TrackSize::Breadth(TrackBreadth::Px(v)));
    // `minmax(0, 1fr)`: a bare `1fr` keeps the plot's stored size as its least
    // (`auto`'s minimum), so a chart made smaller would not shrink its plot.
    let fr = Track::Size(TrackSize::MinMax {
        min: TrackBreadth::Px(0.0),
        max: TrackBreadth::Fr(1.0),
    });
    // An axis's tracks out from the plot: its labels', then its title's.
    let outward = |axis: f64, title: &Option<(String, Rect)>, title_w: f64| {
        let mut t = vec![px_track(axis)];
        if title.is_some() {
            t.push(px_track(title_w));
        }
        t
    };
    let side_tracks = outward(side_axis_w, side_title, side_title_w);
    let under_tracks = outward(under_axis_h, under_title, under_title_h);
    let (side_span, under_span) = (side_tracks.len() as u16, under_tracks.len() as u16);
    let lay = |at_end: bool, mut tracks: Vec<Track>| {
        if at_end {
            tracks.insert(0, fr.clone());
        } else {
            tracks.reverse();
            tracks.push(fr.clone());
        }
        tracks
    };
    let mut columns = lay(left_at_end, side_tracks);
    let rows = lay(!bottom_at_top, under_tracks);
    let (n_cols, n_rows) = (columns.len() as i16, rows.len() as i16);
    // Past everything else on the right, outside a right-hand axis.
    if end_w > 0.0 {
        columns.push(px_track(end_w));
    }
    let body_key = key(PartRole::Body, 0, 0);
    out.frame(
        body_key,
        None,
        "Body",
        Rect::from_origin_size(body_at, body),
        Some(Display::Grid(Grid {
            columns,
            rows,
            ..Grid::default()
        })),
        body_item(
            matches!(
                o.legend.position,
                LegendPosition::Left | LegendPosition::Right
            ) && !data.series.is_empty(),
        ),
    );
    let (plot_col, side_col) = if left_at_end { (1, 2) } else { (n_cols, 1) };
    let (plot_row, under_row) = if bottom_at_top { (n_rows, 1) } else { (1, 2) };
    let lines = |start: i16, span: u16| GridLines {
        start: GridPlacement::Line(start),
        end: if span > 1 {
            GridPlacement::Span(span)
        } else {
            GridPlacement::Auto
        },
    };
    let cell = |(col, col_span): (i16, u16), (row, row_span): (i16, u16)| LayoutItem {
        width: Dimension::Percent(100.0),
        height: Dimension::Percent(100.0),
        grid_column: lines(col, col_span),
        grid_row: lines(row, row_span),
        ..LayoutItem::default()
    };
    let plot_x = if left_at_end { 0.0 } else { left_w };
    let plot_y = if bottom_at_top { bottom_h } else { 0.0 };
    let plot_box = Rect::from_origin_size(Point::new(plot_x, plot_y), plot);
    let side_box = Rect::from_origin_size(
        Point::new(if left_at_end { plot.width } else { 0.0 }, plot_y),
        Size::new(left_w, plot.height),
    );
    let under_box = Rect::from_origin_size(
        Point::new(plot_x, if bottom_at_top { 0.0 } else { plot.height }),
        Size::new(plot.width, bottom_h),
    );

    // Keys and names of the two axis frames, by which one is the value axis.
    let (side_role, under_role) = if horizontal {
        (PartRole::CategoryAxis, PartRole::ValueAxis)
    } else {
        (PartRole::ValueAxis, PartRole::CategoryAxis)
    };
    let side_key = key(side_role, 0, 0);
    let under_key = key(under_role, 0, 0);
    let plot_key = key(PartRole::Plot, 0, 0);
    let axis_name = |r: PartRole| {
        if r == PartRole::ValueAxis {
            "Value axis"
        } else {
            "Category axis"
        }
    };
    out.frame(
        side_key,
        Some(body_key),
        axis_name(side_role),
        side_box,
        None,
        cell((side_col, side_span), (plot_row, 1)),
    );
    out.frame(
        under_key,
        Some(body_key),
        axis_name(under_role),
        under_box,
        None,
        cell((plot_col, 1), (under_row, under_span)),
    );
    out.frame(
        plot_key,
        Some(body_key),
        "Plot",
        plot_box,
        None,
        cell((plot_col, 1), (plot_row, 1)),
    );

    // Along the value axis, as a share of the plot from its start: up from the
    // bottom for columns, right from the left for bars.
    let across_len = if horizontal { plot.height } else { plot.width };
    let value_share = |v: f64| t.t(v);

    // Each line named at its last value, beside the plot: centred there, then
    // pushed apart down and back up inside the plot's height.
    if end_w > 0.0 {
        let ends_key = key(PartRole::EndLabel, 0, 0);
        let ends_box = Rect::from_origin_size(
            Point::new(left_w + plot.width, plot_y),
            Size::new(end_w, plot.height),
        );
        out.frame(
            ends_key,
            Some(body_key),
            "Line ends",
            ends_box,
            None,
            cell((n_cols + 1, 1), (plot_row, 1)),
        );
        let mut placed: Vec<(usize, Rect, f64)> = ends
            .iter()
            .enumerate()
            .map(|(i, (_, s, v, _))| {
                let b = out.text.measure(s, Role::Legend);
                (i, b, (1.0 - value_share(*v)) * plot.height)
            })
            .collect();
        placed.sort_by(|a, b| a.2.total_cmp(&b.2));
        let h = |b: &Rect| b.height();
        // Down: each at least a label below the one above.
        for i in 1..placed.len() {
            let least = placed[i - 1].2 + (h(&placed[i - 1].1) + h(&placed[i].1)) / 2.0;
            placed[i].2 = placed[i].2.max(least);
        }
        // Up: the last inside the plot, each at most a label above the one below.
        if let Some(last) = placed.last_mut() {
            last.2 = last.2.min(plot.height - h(&last.1) / 2.0);
        }
        for i in (0..placed.len().saturating_sub(1)).rev() {
            let most = placed[i + 1].2 - (h(&placed[i].1) + h(&placed[i + 1].1)) / 2.0;
            placed[i].2 = placed[i].2.min(most).max(h(&placed[i].1) / 2.0);
        }
        for (i, b, y) in placed {
            let (col, text, _, fill_) = &ends[i];
            let mut insets = centred_y(y / plot.height.max(1e-9) * 100.0);
            insets.left = px(AXIS_GAP);
            out.label(
                key(PartRole::EndLabel, *col, 1),
                ends_key,
                format!("End · {text}"),
                text,
                Role::Legend,
                Point::new(AXIS_GAP - b.x0, y - b.height() / 2.0 - b.y0),
                insets,
                0.0,
            );
            if let Some(p) = out.parts.last_mut() {
                p.fills = fill(fill_.clone());
            }
        }
    }

    // Value tick labels.
    let value_axis_key = key(PartRole::ValueAxis, 0, 0);
    if va_show {
        let (frame_w, frame_h) = if horizontal {
            (under_box.width(), under_box.height())
        } else {
            (side_box.width(), side_box.height())
        };
        for (i, (s, b)) in tick_text.iter().zip(&tick_boxes).enumerate() {
            let share = value_share(t.values[i]);
            let name = format!("Tick {s}");
            let k = key(PartRole::TickLabel, 0, i as u32);
            let frame = Size::new(frame_w, frame_h);
            let (at, insets) = if horizontal {
                let centre = share * frame_w;
                turned_label(*b, va_turn, true, centre, va_gap, frame, !bottom_at_top)
            } else {
                let centre = (1.0 - share) * frame_h;
                turned_label(*b, va_turn, false, centre, va_gap, frame, left_at_end)
            };
            out.label(k, value_axis_key, name, s, Role::Tick, at, insets, va_turn);
        }
    }
    // Value tick marks, out from the plot's edge at each tick.
    if va_tick > 0.0 {
        let (frame, from_start) = if horizontal {
            (under_box.size(), !bottom_at_top)
        } else {
            (side_box.size(), left_at_end)
        };
        for (i, v) in t.values.iter().enumerate() {
            let share = value_share(*v);
            let along = if horizontal { share } else { 1.0 - share };
            let (bx, insets) = tick_mark(!horizontal, along, from_start, frame, va.line_width);
            out.rect(
                key(PartRole::Tick, 0, i as u32),
                value_axis_key,
                format!("Tick mark {}", tick_text[i]),
                bx,
                Spec {
                    fill: set(&va.colour),
                    radii: RoundedRectRadii::default(),
                    insets,
                },
            );
        }
    }
    // Each title in its own track, centred on the plot along its axis.
    let titles = [
        (side_title, side_key, true, left_at_end),
        (under_title, under_key, false, !bottom_at_top),
    ];
    for (title, axis_key, turned, edge) in titles {
        let Some((text, b)) = title else { continue };
        let series = u32::from(axis_key.role == PartRole::CategoryAxis);
        let frame = if turned { side_box } else { under_box };
        let length = if turned {
            frame.height()
        } else {
            frame.width()
        };
        let (shown, b) = within(text, *b, length, out.text, Role::Title);
        let (at, insets, turn) = title_place(b, frame.size(), turned, edge);
        out.label(
            key(PartRole::AxisTitle, series, 0),
            axis_key,
            format!("Title {text}"),
            &shown,
            Role::Title,
            at,
            insets,
            turn,
        );
    }

    // The plot's own background under everything in it.
    if let Some(ground) = &o.plot.fill {
        let r = o.plot.radius;
        out.rect(
            key(PartRole::Ground, 0, 0),
            plot_key,
            "Ground".into(),
            Rect::from_origin_size(Point::ZERO, plot),
            Spec {
                fill: ground.clone(),
                radii: RoundedRectRadii::from_single_radius(r),
                insets: fill_insets(),
            },
        );
    }
    // The plot's grid and its lines.
    let grid_key = key(PartRole::Grid, 0, 0);
    out.frame_pinned(
        grid_key,
        Some(plot_key),
        "Grid",
        Rect::from_origin_size(Point::ZERO, plot),
        None,
        LayoutItem::default(),
        fill_insets(),
    );
    if va.show && va.grid.show {
        for (i, v) in t.values.iter().enumerate() {
            let share = value_share(*v);
            let name = format!("Gridline {}", tick_text[i]);
            let k = key(PartRole::Gridline, 0, i as u32);
            out.gridline(k, grid_key, name, horizontal, share, plot, &va.grid);
        }
    }
    if let Some(xt) = &x_ticks
        && ca.show
        && ca.grid.show
    {
        for (i, v) in xt.values.iter().enumerate() {
            out.gridline(
                key(PartRole::Gridline, 1, i as u32),
                grid_key,
                format!("Gridline {}", x_text[i]),
                true,
                xt.t(*v),
                plot,
                &ca.grid,
            );
        }
    }
    if ca.show && ca.line {
        // The baseline at zero; at the plot's edge on its axis's side when zero
        // is out of range.
        let zero = value_share(0.0);
        let at_zero = (-1e-9..=1.0 + 1e-9).contains(&zero);
        let edge = if horizontal {
            left_at_end
        } else {
            bottom_at_top
        };
        let share = if at_zero {
            zero
        } else {
            f64::from(u8::from(edge))
        };
        let (bx, insets) = rule_at(horizontal, share, plot, ca.line_width);
        out.rect(
            key(PartRole::AxisLine, 1, 0),
            grid_key,
            if at_zero { "Baseline" } else { "Axis line" }.into(),
            bx,
            Spec {
                fill: set(&ca.colour),
                radii: RoundedRectRadii::default(),
                insets,
            },
        );
    }
    if va.show && va.line {
        // Along the plot's edge on the value axis's side.
        let edge = if horizontal {
            bottom_at_top
        } else {
            left_at_end
        };
        let (bx, insets) = rule_at(!horizontal, f64::from(u8::from(edge)), plot, va.line_width);
        out.rect(
            key(PartRole::AxisLine, 0, 0),
            grid_key,
            "Value axis line".into(),
            bx,
            Spec {
                fill: set(&va.colour),
                radii: RoundedRectRadii::default(),
                insets,
            },
        );
    }

    Axes {
        horizontal,
        t,
        plot,
        plot_key,
        across_len,
        band: across_len / n_cat.max(1) as f64,
        ca_show,
        every,
        cat_boxes,
        turn,
        side_box,
        under_box,
        left_at_end,
        bottom_at_top,
        ca_gap,
        ca_ticks: (ca_tick > 0.0).then(|| (set(&ca.colour), ca.line_width)),
        x: x_ticks,
        x_text,
        value_fmt,
    }
}

/// The colour a bar of series `s` at category `c` draws in: below the baseline
/// the below-zero colour when set, else its category's (colour by category),
/// else its series'.
fn bar_fill(o: &super::Options, data: &Data, s: &SeriesData, c: usize, raw: f64) -> BrushSource {
    match &o.below_zero {
        Some(b) if raw < 0.0 && o.bars.stacking != Stacking::Percent => b.clone(),
        _ => data
            .by_category
            .get(c)
            .cloned()
            .unwrap_or_else(|| s.fill.clone()),
    }
}

/// One lollipop in its bar's slot `bx` (pinned by `insets`): a frame keyed and
/// named as the bar would be, holding a stem centred across the slot from the
/// baseline to the value and a head on the value's end. `end_up`: the value
/// end is the slot's top (a column) or right (a bar). Answers the box a value
/// label sits past, the head included.
fn lollipop(
    out: &mut Out,
    o: &super::Options,
    (k, parent): (PartKey, PartKey),
    name: String,
    (bx, insets): (Rect, Insets),
    (horizontal, end_up): (bool, bool),
    fill_: BrushSource,
) -> Rect {
    let b = &o.bars;
    out.frame_pinned(
        k,
        Some(parent),
        &name,
        bx,
        None,
        LayoutItem::default(),
        insets,
    );
    let (w, h) = (bx.width(), bx.height());
    let stem = b.stem.max(0.0);
    let head = b.head.max(0.0);
    let stem_fill = b.stem_colour.clone().unwrap_or_else(|| fill_.clone());
    // The stem: the slot's length along the value axis, `stem` across.
    let (sb, si) = if horizontal {
        let mut i = centred_y(50.0);
        i.left = px(0.0);
        i.right = px(0.0);
        (
            Rect::new(0.0, h / 2.0 - stem / 2.0, w, h / 2.0 + stem / 2.0),
            i,
        )
    } else {
        let mut i = centred_x(50.0);
        i.top = px(0.0);
        i.bottom = px(0.0);
        (
            Rect::new(w / 2.0 - stem / 2.0, 0.0, w / 2.0 + stem / 2.0, h),
            i,
        )
    };
    out.rect(
        key(PartRole::Stem, k.series, k.index),
        k,
        "Stem".into(),
        sb,
        Spec {
            fill: stem_fill,
            radii: RoundedRectRadii::default(),
            insets: si,
        },
    );
    // The head, centred on the value's end of the slot.
    let (cx, cy) = match (horizontal, end_up) {
        (true, true) => (w, h / 2.0),
        (true, false) => (0.0, h / 2.0),
        (false, true) => (w / 2.0, 0.0),
        (false, false) => (w / 2.0, h),
    };
    let hb = Rect::from_center_size((cx, cy), (head, head));
    let (fills, strokes) = if b.head_style == super::PointStyle::Hollow {
        (
            fill(out.ground.clone()),
            vec![line_stroke(
                fill_.clone(),
                (head / 6.0).clamp(1.0, 3.0),
                super::Dash::Solid,
            )],
        )
    } else {
        mark_paint(o, &fill_, &out.ground)
    };
    out.parts.push(Part {
        key: key(PartRole::Head, k.series, k.index),
        parent: Some(k),
        name: "Head".into(),
        kind: point_kind(b.head_shape, hb.size()),
        transform: Affine::translate(hb.origin().to_vec2()),
        fills,
        strokes,
        opacity: 1.0,
        insets: centred(cx / w.max(1e-9) * 100.0, cy / h.max(1e-9) * 100.0),
        display: None,
        item: LayoutItem::default(),
        clip: false,
    });
    // In the plot's own units: the slot grown by half a head at its value end.
    let r = head / 2.0;
    match (horizontal, end_up) {
        (true, true) => Rect::new(bx.x0, bx.y0, bx.x1 + r, bx.y1),
        (true, false) => Rect::new(bx.x0 - r, bx.y0, bx.x1, bx.y1),
        (false, true) => Rect::new(bx.x0, bx.y0 - r, bx.x1, bx.y1),
        (false, false) => Rect::new(bx.x0, bx.y0, bx.x1, bx.y1 + r),
    }
}

/// Column and bar marks: one series frame of bars each, and the value labels.
fn bar_marks(chart: &Chart, data: &Data, ax: &Axes, totals: &[f64], out: &mut Out) {
    let o = &chart.options;
    // Kept above zero: a plot the axes' labels leave no room for draws its
    // bars at its edge rather than at NaN.
    let (horizontal, plot, plot_key, across_len) =
        (ax.horizontal, ax.plot, ax.plot_key, ax.across_len.max(1e-9));
    let stacking = o.bars.stacking;
    // Totals over a stack; a 100% chart's would all read 100%.
    let totals_on = o.values.totals && stacking == Stacking::Stacked;
    let (n_cat, n_ser) = (data.categories.len(), data.series.len());
    let value_share = |v: f64| ax.t.t(v);
    let shown = |c: usize, v: f64| -> f64 {
        match stacking {
            Stacking::Percent if totals[c] > 0.0 => v / totals[c],
            _ => v,
        }
    };
    let band = ax.band;
    let group_gap = (o.bars.group_gap / 100.0).clamp(0.0, 0.95);
    let group = band * (1.0 - group_gap);
    let lollipops = o.bars.shape == super::BarShape::Lollipops;
    // Connected heads share their band's one centre.
    let connected = lollipops && o.bars.connect && n_ser > 1;
    let slots = match stacking {
        Stacking::Apart if !connected => n_ser.max(1),
        _ => 1,
    };
    // A dumbbell's line from each band's least head to its greatest, under them.
    if connected {
        let k = key(PartRole::Connectors, 0, 0);
        out.frame_pinned(
            k,
            Some(plot_key),
            "Connectors",
            Rect::from_origin_size(Point::ZERO, plot),
            None,
            LayoutItem::default(),
            fill_insets(),
        );
        let ink = o.bars.stem_colour.clone().unwrap_or_else(|| chart.muted());
        for c in 0..n_cat {
            let vs: Vec<f64> = data.series.iter().filter_map(|s| s.values[c]).collect();
            let (Some(lo), Some(hi)) = (
                vs.iter().copied().reduce(f64::min),
                vs.iter().copied().reduce(f64::max),
            ) else {
                continue;
            };
            let mid = (c as f64 + 0.5) * band / across_len;
            let (s0, s1) = (value_share(lo), value_share(hi));
            let w = o.bars.stem.max(0.5);
            let (bx, insets) = if horizontal {
                let y = mid * plot.height;
                let r = Rect::new(s0 * plot.width, y - w / 2.0, s1 * plot.width, y + w / 2.0);
                let mut i = centred_y(mid * 100.0);
                i.left = pct(s0 * 100.0);
                i.right = pct((1.0 - s1) * 100.0);
                (r, i)
            } else {
                let x = mid * plot.width;
                let r = Rect::new(
                    x - w / 2.0,
                    (1.0 - s1) * plot.height,
                    x + w / 2.0,
                    (1.0 - s0) * plot.height,
                );
                let mut i = centred_x(mid * 100.0);
                i.top = pct((1.0 - s1) * 100.0);
                i.bottom = pct(s0 * 100.0);
                (r, i)
            };
            out.rect(
                key(PartRole::Connector, 0, c as u32),
                k,
                format!("Connector · {}", data.categories[c]),
                bx,
                Spec {
                    fill: ink.clone(),
                    radii: RoundedRectRadii::default(),
                    insets,
                },
            );
        }
    }
    let bar_gap = (o.bars.bar_gap / 100.0).clamp(0.0, 0.95);
    // `slots` bars and the gaps between them fill the group.
    let bar = group / (slots as f64 + bar_gap * (slots as f64 - 1.0).max(0.0));
    let mut values_parts = Vec::new();
    let mut ups = vec![0.0f64; n_cat];
    let mut downs = vec![0.0f64; n_cat];
    for (si, s) in data.series.iter().enumerate() {
        let series_key = key(PartRole::Series, s.column.0, 0);
        out.frame_pinned(
            series_key,
            Some(plot_key),
            &format!("Series {}", s.name),
            Rect::from_origin_size(Point::ZERO, plot),
            None,
            LayoutItem::default(),
            fill_insets(),
        );
        for (c, cat) in data.categories.iter().enumerate() {
            let Some(raw) = s.values[c] else { continue };
            let v = shown(c, raw);
            let (from, to) = match stacking {
                Stacking::Apart => (0.0, v),
                _ if v >= 0.0 => {
                    let from = ups[c];
                    ups[c] += v;
                    (from, ups[c])
                }
                _ => {
                    let from = downs[c];
                    downs[c] += v;
                    (from, downs[c])
                }
            };
            let slot = if stacking == Stacking::Apart { si } else { 0 };
            let a0 = c as f64 * band + (band - group) / 2.0 + slot as f64 * bar * (1.0 + bar_gap);
            let a1 = a0 + bar;
            let (s0, s1) = (value_share(from.min(to)), value_share(from.max(to)));
            // Across as shares of the plot, along as shares of the value axis.
            let (x0, x1, y0, y1) = if horizontal {
                (s0, s1, a0 / across_len, a1 / across_len)
            } else {
                (a0 / across_len, a1 / across_len, 1.0 - s1, 1.0 - s0)
            };
            let bx = Rect::new(
                x0 * plot.width,
                y0 * plot.height,
                x1 * plot.width,
                y1 * plot.height,
            );
            let insets = Insets {
                left: pct(x0 * 100.0),
                right: pct((1.0 - x1) * 100.0),
                top: pct(y0 * 100.0),
                bottom: pct((1.0 - y1) * 100.0),
                margin_auto: AutoMargins::default(),
            };
            let r = o
                .bars
                .radius
                .min(bx.width() / 2.0)
                .min(bx.height() / 2.0)
                .max(0.0);
            let end_up = to >= from && v >= 0.0;
            let radii = if o.bars.radius_all {
                RoundedRectRadii::from_single_radius(r)
            } else if horizontal {
                if end_up {
                    RoundedRectRadii::new(0.0, r, r, 0.0)
                } else {
                    RoundedRectRadii::new(r, 0.0, 0.0, r)
                }
            } else if end_up {
                RoundedRectRadii::new(r, r, 0.0, 0.0)
            } else {
                RoundedRectRadii::new(0.0, 0.0, r, r)
            };
            if lollipops {
                let fill_ = bar_fill(o, data, s, c, raw);
                let labelled = lollipop(
                    out,
                    o,
                    (key(PartRole::Bar, s.column.0, c as u32), series_key),
                    format!("{} · {}", s.name, cat),
                    (bx, insets),
                    (horizontal, end_up),
                    fill_,
                );
                if o.values.position != ValuePosition::Off {
                    values_parts.push((s.column.0, c, raw, labelled, end_up));
                }
                continue;
            }
            out.rect(
                key(PartRole::Bar, s.column.0, c as u32),
                series_key,
                format!("{} · {}", s.name, cat),
                bx,
                Spec {
                    fill: bar_fill(o, data, s, c, raw),
                    radii,
                    insets,
                },
            );
            out.as_mark(o);
            // From the value end to the baseline, in the bar's own box.
            let (w, h) = (bx.width(), bx.height());
            let (full, faint) = match (horizontal, end_up) {
                (true, true) => (Point::new(w, 0.0), Point::ZERO),
                (true, false) => (Point::ZERO, Point::new(w, 0.0)),
                (false, true) => (Point::ZERO, Point::new(0.0, h)),
                (false, false) => (Point::new(0.0, h), Point::ZERO),
            };
            fade_last(out, o, full, faint, &s.name);
            if o.values.position != ValuePosition::Off {
                values_parts.push((s.column.0, c, raw, bx, end_up));
            }
        }
    }

    // Value labels, where they fit.
    if !values_parts.is_empty() {
        let values_key = key(PartRole::Values, 0, 0);
        out.frame_pinned(
            values_key,
            Some(plot_key),
            "Values",
            Rect::from_origin_size(Point::ZERO, plot),
            None,
            LayoutItem::default(),
            fill_insets(),
        );
        let vf = &o.values.format;
        // In a stack only the outermost segment each way has an outside to sit
        // in; the ones it covers take their label inside, where it fits. A
        // zero covers nothing and so is never the end.
        let outermost = |i: usize| {
            let (_, c, raw, _, up) = values_parts[i];
            stacking == Stacking::Apart
                || (raw != 0.0
                    && !values_parts[i + 1..]
                        .iter()
                        .any(|(_, c2, raw2, _, up2)| *c2 == c && *up2 == up && *raw2 != 0.0))
        };
        let flags: Vec<bool> = (0..values_parts.len()).map(outermost).collect();
        for ((series, c, raw, bx, up), outer) in values_parts.into_iter().zip(flags) {
            let shown_v = if stacking == Stacking::Percent && totals[c] > 0.0 {
                raw / totals[c]
            } else {
                raw
            };
            let mut f = vf.clone();
            if stacking == Stacking::Percent {
                f.percent = true;
            }
            // As many places as the value has, up to three; a share in whole percent.
            let decimals = if f.percent {
                0
            } else {
                scale::decimals_of(raw).min(3)
            };
            let s = scale::format(shown_v, &f, decimals);
            let b = out.text.measure(&s, Role::Value);
            // A stack's total takes the place past its end.
            let position = match o.values.position {
                // A lollipop has no inside: its label sits past the head.
                _ if lollipops => ValuePosition::Outside,
                ValuePosition::Outside if !outer || totals_on => ValuePosition::Inside,
                p => p,
            };
            let Some((at, insets)) = value_place(position, horizontal, up, bx, b, plot) else {
                continue;
            };
            out.label(
                key(PartRole::ValueLabel, series, c as u32),
                values_key,
                format!("Value {s}"),
                &s,
                Role::Value,
                at,
                insets,
                0.0,
            );
            if matches!(position, ValuePosition::Inside | ValuePosition::End)
                && let Some(s) = data.series.iter().find(|s| s.column.0 == series)
            {
                out.ink_inside(o, &s.fill);
            }
        }
    }

    // Each stack's net total past its end, in the value labels' type a weight
    // heavier.
    if totals_on {
        let totals_key = key(PartRole::Values, 1, 0);
        out.frame_pinned(
            totals_key,
            Some(plot_key),
            "Totals",
            Rect::from_origin_size(Point::ZERO, plot),
            None,
            LayoutItem::default(),
            fill_insets(),
        );
        let weight = (out.text.weight(Role::Value) + 200).min(900);
        for (c, cat) in data.categories.iter().enumerate() {
            if !data.series.iter().any(|s| s.values[c].is_some()) {
                continue;
            }
            let net = ups[c] + downs[c];
            let up = net >= 0.0;
            let end = if up { ups[c] } else { downs[c] };
            let a0 = c as f64 * band + (band - group) / 2.0;
            let a1 = a0 + bar;
            let (s0, s1) = (value_share(end.min(0.0)), value_share(end.max(0.0)));
            let bx = if horizontal {
                Rect::new(
                    s0 * plot.width,
                    a0 / across_len * plot.height,
                    s1 * plot.width,
                    a1 / across_len * plot.height,
                )
            } else {
                Rect::new(
                    a0 / across_len * plot.width,
                    (1.0 - s1) * plot.height,
                    a1 / across_len * plot.width,
                    (1.0 - s0) * plot.height,
                )
            };
            let s = scale::format(net, &o.values.format, scale::decimals_of(net).min(3));
            let kind = out.text.kind_weighted(&s, Role::Value, weight);
            let b = crate::geometry::local_bounds(&kind, None).unwrap_or_default();
            let Some((at, insets)) =
                value_place(ValuePosition::Outside, horizontal, up, bx, b, plot)
            else {
                continue;
            };
            out.label(
                key(PartRole::Total, 0, c as u32),
                totals_key,
                format!("Total · {cat}"),
                &s,
                Role::Value,
                at,
                insets,
                0.0,
            );
            if let Some(p) = out.parts.last_mut() {
                p.kind = kind;
            }
        }
    }
}

impl Axes {
    /// The category labels, every `every`th, centred on their band.
    fn category_labels(&self, data: &Data, out: &mut Out) {
        let (horizontal, left_at_end, bottom_at_top) =
            (self.horizontal, self.left_at_end, self.bottom_at_top);
        let (band, across_len, every, turn) =
            (self.band, self.across_len.max(1e-9), self.every, self.turn);
        let (side_box, under_box, cat_boxes) = (self.side_box, self.under_box, &self.cat_boxes);
        let cat_axis_key = key(PartRole::CategoryAxis, 0, 0);
        let frame = if horizontal { side_box } else { under_box };
        // Tick marks: at a scatter's X ticks, else at the centre of every
        // category a label is written for, so a long axis draws no more ticks
        // than labels.
        if let Some((colour, width)) = &self.ca_ticks {
            let at: Vec<(usize, f64, String)> = match &self.x {
                Some(xt) => xt
                    .values
                    .iter()
                    .zip(&self.x_text)
                    .enumerate()
                    .map(|(i, (v, s))| (i, xt.t(*v), s.clone()))
                    .collect(),
                None => data
                    .categories
                    .iter()
                    .enumerate()
                    .filter(|(c, _)| c % every == 0)
                    .map(|(c, name)| (c, (c as f64 + 0.5) * band / across_len, name.clone()))
                    .collect(),
            };
            let from_start = if horizontal {
                left_at_end
            } else {
                !bottom_at_top
            };
            for (i, along, name) in at {
                let (bx, insets) = tick_mark(horizontal, along, from_start, frame.size(), *width);
                out.rect(
                    key(PartRole::Tick, 1, i as u32),
                    cat_axis_key,
                    format!("Tick mark {name}"),
                    bx,
                    Spec {
                        fill: colour.clone(),
                        radii: RoundedRectRadii::default(),
                        insets,
                    },
                );
            }
        }
        if !self.ca_show {
            return;
        }
        let gap = self.ca_gap;
        // A scatter's X axis writes number ticks at their place; any other,
        // every `every`th category at its band's centre.
        let items: Vec<(f64, &String, Rect, PartKey, String)> = match &self.x {
            Some(xt) => self
                .x_text
                .iter()
                .enumerate()
                .map(|(i, s)| {
                    (
                        xt.t(xt.values[i]) * across_len,
                        s,
                        cat_boxes[i],
                        key(PartRole::TickLabel, 1, i as u32),
                        format!("Tick {s}"),
                    )
                })
                .collect(),
            None => data
                .categories
                .iter()
                .enumerate()
                .filter(|(c, _)| c % every == 0)
                .map(|(c, name)| {
                    (
                        (c as f64 + 0.5) * band,
                        name,
                        cat_boxes[c],
                        key(PartRole::CategoryLabel, 0, c as u32),
                        format!("Category {name}"),
                    )
                })
                .collect(),
        };
        for (centre, name, b, k, label_name) in items {
            let (at, insets) = if horizontal {
                turned_label(b, turn, false, centre, gap, frame.size(), left_at_end)
            } else {
                turned_label(b, turn, true, centre, gap, frame.size(), !bottom_at_top)
            };
            out.label(
                k,
                cat_axis_key,
                label_name,
                name,
                Role::Tick,
                at,
                insets,
                turn,
            );
        }
    }
}

/// One legend entry: a series, or a pie's slice.
struct Entry {
    /// The series' column id, or the slice's place.
    series: u32,
    name: String,
    fill: BrushSource,
    /// The series' line dash, for a *Line* swatch.
    dash: super::Dash,
}

/// The relaxation passes a Sankey's order takes: a fixed count, not a
/// convergence test, so the same table always draws the same chart.
const RELAX_PASSES: usize = 6;
/// The gap between a Sankey node and its label.
const NODE_LABEL_GAP: f64 = 6.0;

/// A Sankey node: its name, value, column and place.
struct Node {
    name: String,
    value: f64,
    column: usize,
    y: f64,
    h: f64,
    fill: BrushSource,
}

/// A Sankey flow: from and to node indices, its value, and its ribbon's ends
/// as offsets down each node.
struct Flow {
    from: usize,
    to: usize,
    value: f64,
    sy: f64,
    ty: f64,
}

/// A Sankey: flows between named nodes, the nodes in columns by depth, the
/// order in each column relaxed toward what the nodes connect to, the flows as
/// ribbons as thick as their value. Rows that cannot be drawn are pushed onto
/// `left` with their reasons.
fn sankey(
    chart: &Chart,
    left: &mut Vec<String>,
    body: Size,
    body_at: Point,
    out: &mut Out,
) -> Result<(), Refusal> {
    let o = &chart.options;
    let f = &o.flows;
    let t = &chart.table;
    let m = &chart.mapping;
    let at = |c: Option<ColumnId>| c.and_then(|c| t.index_of(c));
    let (Some(fc), Some(tc), Some(vc)) = (at(m.from), at(m.to), at(m.value)) else {
        body_frame(out, body, body_at);
        return Ok(());
    };

    // Nodes in first-seen order; same-named flows added up. Looked up by
    // hash, so a table of many names costs its rows, not rows × names, before
    // the limits refuse it.
    let mut nodes: Vec<Node> = Vec::new();
    let mut flows: Vec<Flow> = Vec::new();
    let mut named: rustc_hash::FxHashMap<String, usize> = Default::default();
    let mut joined: rustc_hash::FxHashMap<(usize, usize), usize> = Default::default();
    let colours = super::colours::drawn_palette(chart);
    let mut index = |name: &str, nodes: &mut Vec<Node>| -> usize {
        if let Some(i) = named.get(name) {
            return *i;
        }
        let i = nodes.len();
        named.insert(name.to_string(), i);
        let fill = named_fill(chart, name).unwrap_or_else(|| rgb(colours[i % colours.len()]));
        nodes.push(Node {
            name: name.to_string(),
            value: 0.0,
            column: 0,
            y: 0.0,
            h: 0.0,
            fill,
        });
        i
    };
    for (r, row) in t.rows.iter().enumerate() {
        let (from, to) = (row[fc].trim(), row[tc].trim());
        if from.is_empty() || to.is_empty() {
            left.push(format!("Row {}: no From or To", r + 1));
            continue;
        }
        if from == to {
            left.push(format!("Row {}: {from} flows into itself", r + 1));
            continue;
        }
        let Some(v) = parse_number(&row[vc]).filter(|v| *v > 0.0) else {
            left.push(format!("Row {}: a flow's value must be above zero", r + 1));
            continue;
        };
        let (a, b) = (index(from, &mut nodes), index(to, &mut nodes));
        match joined.get(&(a, b)) {
            Some(&k) => flows[k].value += v,
            None => {
                joined.insert((a, b), flows.len());
                flows.push(Flow {
                    from: a,
                    to: b,
                    value: v,
                    sy: 0.0,
                    ty: 0.0,
                });
            }
        }
    }
    if nodes.len() > super::MAX_SANKEY_NODES {
        return Err(Refusal::TooManyMarks {
            count: nodes.len(),
            what: "nodes",
        });
    }
    if flows.len() > super::MAX_FLOWS {
        return Err(Refusal::TooManyMarks {
            count: flows.len(),
            what: "flows",
        });
    }
    if let Some(cycle) = find_loop(nodes.len(), &flows) {
        let mut names: Vec<&str> = cycle.iter().map(|i| nodes[*i].name.as_str()).collect();
        names.push(names[0]);
        return Err(Refusal::LoopsBack(names.join(" → ")));
    }

    // Columns by depth, then aligned.
    let n = nodes.len();
    let ins = |i: usize| flows.iter().filter(move |fl| fl.to == i);
    let outs = |i: usize| flows.iter().filter(move |fl| fl.from == i);
    let mut depth = vec![0usize; n];
    // Longest path from a source: repeat until stable, at most n rounds (no loop).
    for _ in 0..n {
        let mut moved = false;
        for fl in &flows {
            if depth[fl.to] < depth[fl.from] + 1 {
                depth[fl.to] = depth[fl.from] + 1;
                moved = true;
            }
        }
        if !moved {
            break;
        }
    }
    let last = depth.iter().copied().max().unwrap_or(0);
    let mut height = vec![0usize; n];
    for _ in 0..n {
        let mut moved = false;
        for fl in &flows {
            if height[fl.from] < height[fl.to] + 1 {
                height[fl.from] = height[fl.to] + 1;
                moved = true;
            }
        }
        if !moved {
            break;
        }
    }
    for i in 0..n {
        let sink = outs(i).next().is_none();
        let source = ins(i).next().is_none();
        nodes[i].column = match f.align {
            super::FlowAlign::Left => depth[i],
            super::FlowAlign::Right => last - height[i],
            super::FlowAlign::Justify if sink => last,
            super::FlowAlign::Justify => depth[i],
            super::FlowAlign::Centre if source => outs(i)
                .map(|fl| depth[fl.to])
                .min()
                .map_or(0, |d| d.saturating_sub(1)),
            super::FlowAlign::Centre => depth[i],
        };
        nodes[i].value = ins(i)
            .map(|fl| fl.value)
            .sum::<f64>()
            .max(outs(i).map(|fl| fl.value).sum::<f64>());
    }
    let columns = last + 1;

    // Room for the labels outside the first and last columns.
    let label_text = |nd: &Node| match f.labels {
        super::NodeLabels::Name => nd.name.clone(),
        super::NodeLabels::NameAndValue => format!(
            "{} {}",
            nd.name,
            scale::format(
                nd.value,
                &o.values.format,
                scale::decimals_of(nd.value).min(2)
            )
        ),
        super::NodeLabels::Off => String::new(),
    };
    let widest = |col: usize| {
        nodes
            .iter()
            .filter(|nd| nd.column == col)
            .map(|nd| out.text.measure(&label_text(nd), Role::Legend).width())
            .fold(0.0, f64::max)
    };
    let (room_l, room_r) = if f.labels == super::NodeLabels::Off {
        (0.0, 0.0)
    } else {
        (
            widest(0) + NODE_LABEL_GAP,
            if columns > 1 {
                widest(last) + NODE_LABEL_GAP
            } else {
                0.0
            },
        )
    };
    let plot = body;
    let nw = f.node_width.min(plot.width / 4.0).max(1.0);
    let span = (plot.width - room_l - room_r - nw).max(0.0);
    let x_of = |col: usize| {
        room_l
            + if columns > 1 {
                span * col as f64 / (columns - 1) as f64
            } else {
                span / 2.0
            }
    };

    // Heights: one scale for every column, so a value is as tall everywhere.
    // The gaps of the most crowded column take at most half the height, so
    // its nodes keep the rest.
    let crowd = (0..columns)
        .map(|c| nodes.iter().filter(|nd| nd.column == c).count())
        .max()
        .unwrap_or(0);
    let gap = if crowd > 1 {
        f.node_gap.min(plot.height / 2.0 / (crowd - 1) as f64)
    } else {
        f.node_gap
    };
    let ky = (0..columns)
        .filter_map(|c| {
            let members: Vec<&Node> = nodes.iter().filter(|nd| nd.column == c).collect();
            let total: f64 = members.iter().map(|nd| nd.value).sum();
            (total > 0.0).then(|| {
                (plot.height - gap * (members.len() as f64 - 1.0).max(0.0)).max(0.0) / total
            })
        })
        .fold(f64::INFINITY, f64::min);
    let ky = if ky.is_finite() { ky } else { 0.0 };
    for nd in nodes.iter_mut() {
        nd.h = nd.value * ky;
    }
    // Table order first, stacked from the top of each column.
    let mut order: Vec<Vec<usize>> = vec![Vec::new(); columns];
    for (i, nd) in nodes.iter().enumerate() {
        order[nd.column].push(i);
    }
    let settle = |nodes: &mut Vec<Node>, col: &[usize]| {
        // Overlaps pushed down, then back up from the bottom.
        let mut ids = col.to_vec();
        ids.sort_by(|a, b| nodes[*a].y.total_cmp(&nodes[*b].y));
        let mut y = 0.0;
        for &i in &ids {
            nodes[i].y = nodes[i].y.max(y);
            y = nodes[i].y + nodes[i].h + gap;
        }
        let mut bottom = plot.height;
        for &i in ids.iter().rev() {
            if nodes[i].y + nodes[i].h > bottom {
                nodes[i].y = (bottom - nodes[i].h).max(0.0);
            }
            bottom = nodes[i].y - gap;
        }
    };
    for col in &order {
        let mut y = 0.0;
        for &i in col {
            nodes[i].y = y;
            y += nodes[i].h + gap;
        }
    }
    if f.relax {
        let centre = |nd: &Node| nd.y + nd.h / 2.0;
        for pass in 0..RELAX_PASSES {
            let cols: Vec<usize> = if pass % 2 == 0 {
                (1..columns).collect()
            } else {
                (0..columns.saturating_sub(1)).rev().collect()
            };
            for c in cols {
                for &i in &order[c] {
                    let (mut sum, mut weight) = (0.0, 0.0);
                    for fl in &flows {
                        let other = if fl.to == i && pass % 2 == 0 {
                            Some(fl.from)
                        } else if fl.from == i && pass % 2 == 1 {
                            Some(fl.to)
                        } else {
                            None
                        };
                        if let Some(o) = other {
                            sum += centre(&nodes[o]) * fl.value;
                            weight += fl.value;
                        }
                    }
                    if weight > 0.0 {
                        nodes[i].y = sum / weight - nodes[i].h / 2.0;
                    }
                }
                settle(&mut nodes, &order[c]);
            }
        }
    }

    // Ribbons stack on each node in the order of the nodes at their other ends,
    // so they cross as little as they can.
    let mut by_source: Vec<usize> = (0..flows.len()).collect();
    by_source.sort_by(|a, b| {
        let (fa, fb) = (&flows[*a], &flows[*b]);
        fa.from
            .cmp(&fb.from)
            .then(nodes[fa.to].y.total_cmp(&nodes[fb.to].y))
    });
    let mut down = vec![0.0f64; n];
    for &k in &by_source {
        let s = flows[k].from;
        flows[k].sy = down[s];
        down[s] += flows[k].value * ky;
    }
    let mut by_target: Vec<usize> = (0..flows.len()).collect();
    by_target.sort_by(|a, b| {
        let (fa, fb) = (&flows[*a], &flows[*b]);
        fa.to
            .cmp(&fb.to)
            .then(nodes[fa.from].y.total_cmp(&nodes[fb.from].y))
    });
    let mut down = vec![0.0f64; n];
    for &k in &by_target {
        let d = flows[k].to;
        flows[k].ty = down[d];
        down[d] += flows[k].value * ky;
    }

    let body_key = body_frame(out, body, body_at);
    let flows_key = key(PartRole::Flows, 0, 0);
    out.frame_pinned(
        flows_key,
        Some(body_key),
        "Flows",
        Rect::from_origin_size(Point::ZERO, plot),
        None,
        LayoutItem::default(),
        fill_insets(),
    );
    let neutral = set(&o.value_axis.colour);
    for (k, fl) in flows.iter().enumerate() {
        let (a, b) = (&nodes[fl.from], &nodes[fl.to]);
        let th = fl.value * ky;
        let x0 = x_of(a.column) + nw;
        let x1 = x_of(b.column);
        let (y0, y1) = (a.y + fl.sy, b.y + fl.ty);
        let mid = (x0 + x1) / 2.0;
        let mut p = kurbo::BezPath::new();
        p.move_to((x0, y0));
        p.curve_to((mid, y0), (mid, y1), (x1, y1));
        p.line_to((x1, y1 + th));
        p.curve_to((mid, y1 + th), (mid, y0 + th), (x0, y0 + th));
        p.close_path();
        let brush = match f.colour {
            super::FlowColour::Source => a.fill.clone(),
            super::FlowColour::Gradient => flow_gradient(&a.fill, &b.fill, x0, x1),
            super::FlowColour::Target => b.fill.clone(),
            super::FlowColour::Neutral => neutral.clone(),
        };
        out.path(
            key(PartRole::Flow, 0, k as u32),
            flows_key,
            format!("Flow · {} → {}", a.name, b.name),
            p,
            plot,
            fill(brush),
            Vec::new(),
            f.opacity as f32,
        );
    }
    let nodes_key = key(PartRole::Nodes, 0, 0);
    out.frame_pinned(
        nodes_key,
        Some(body_key),
        "Nodes",
        Rect::from_origin_size(Point::ZERO, plot),
        None,
        LayoutItem::default(),
        fill_insets(),
    );
    let (w, h) = (plot.width.max(1e-9), plot.height.max(1e-9));
    for (i, nd) in nodes.iter().enumerate() {
        let x = x_of(nd.column);
        let bx = Rect::new(x, nd.y, x + nw, nd.y + nd.h);
        // Its place and height follow the plot; its width stays.
        out.rect(
            key(PartRole::SankeyNode, 0, i as u32),
            nodes_key,
            format!("Node · {}", nd.name),
            bx,
            Spec {
                fill: nd.fill.clone(),
                radii: RoundedRectRadii::from_single_radius(
                    f.node_radius.min(nw / 2.0).min(nd.h / 2.0).max(0.0),
                ),
                insets: Insets {
                    left: pct(x / w * 100.0),
                    top: pct(nd.y / h * 100.0),
                    bottom: pct((1.0 - (nd.y + nd.h) / h) * 100.0),
                    ..Insets::default()
                },
            },
        );
        out.as_mark(o);
        let text = label_text(nd);
        if text.is_empty() {
            continue;
        }
        let b = out.text.measure(&text, Role::Legend);
        // Outside on the first and last columns; elsewhere away from the middle.
        let on_left = if nd.column == 0 && columns > 1 {
            true
        } else if nd.column == last {
            false
        } else {
            x + nw / 2.0 < plot.width / 2.0
        };
        let cy = nd.y + nd.h / 2.0;
        let mut insets = centred_y(cy / h * 100.0);
        let lx = if on_left {
            let right = x - NODE_LABEL_GAP;
            insets.right = pct((1.0 - right / w) * 100.0);
            right - b.width()
        } else {
            let left = x + nw + NODE_LABEL_GAP;
            insets.left = pct(left / w * 100.0);
            left
        };
        out.label(
            key(PartRole::NodeLabel, 0, i as u32),
            nodes_key,
            format!("Label · {}", nd.name),
            &text,
            Role::Legend,
            Point::new(lx - b.x0, cy - b.height() / 2.0 - b.y0),
            insets,
            0.0,
        );
    }
    Ok(())
}

/// `brush` fading from full at `full` to `k` of its alpha at `faint`, in its
/// part's own coordinates; `None` for a colour a gradient stop cannot hold
/// (a bound one).
fn faded(brush: &BrushSource, full: Point, faint: Point, k: f64) -> Option<BrushSource> {
    let Some(crate::Brush::Solid(c)) = brush.literal() else {
        return None;
    };
    let stop = |offset: f32, color: peniko::Color| peniko::ColorStop {
        offset,
        color: peniko::color::DynamicColor::from_alpha_color(color),
    };
    let gradient = peniko::Gradient::new_linear(full, faint)
        .with_stops([stop(0.0, *c), stop(1.0, c.multiply_alpha(k as f32))]);
    Some(BrushSource::Literal(crate::Brush::Gradient(
        crate::GradientBrush::from(gradient),
    )))
}

/// The part just pushed faded (*Fade*) from `full` to `faint`, or left flat
/// and said once for series `name` when its colour is bound.
fn fade_last(out: &mut Out, o: &super::Options, full: Point, faint: Point, name: &str) {
    if !o.fade.on {
        return;
    }
    let k = (o.fade.to / 100.0).clamp(0.0, 1.0);
    let Some(p) = out.parts.last_mut() else {
        return;
    };
    let Some(f) = p.fills.first_mut() else { return };
    match faded(&f.brush, full, faint, k) {
        Some(b) => f.brush = b,
        None => {
            let note = format!("Fades need an unbound colour: {name} draws flat");
            if !out.left_out.contains(&note) {
                out.left_out.push(note);
            }
        }
    }
}

/// A *Gradient* flow's fill: a linear gradient across the ribbon from `x0` (the
/// source node's edge) in the source's colour to `x1` in the target's, in the
/// ribbon's own space. A colour that is no literal solid (one bound to a
/// variable) has no stop to give, so the flow takes the source's paint whole.
fn flow_gradient(from: &BrushSource, to: &BrushSource, x0: f64, x1: f64) -> BrushSource {
    let solid = |s: &BrushSource| match s.literal() {
        Some(crate::Brush::Solid(c)) => Some(*c),
        _ => None,
    };
    let (Some(a), Some(b)) = (solid(from), solid(to)) else {
        return from.clone();
    };
    let stop = |offset: f32, color: peniko::Color| peniko::ColorStop {
        offset,
        color: peniko::color::DynamicColor::from_alpha_color(color),
    };
    let gradient =
        peniko::Gradient::new_linear((x0, 0.0), (x1, 0.0)).with_stops([stop(0.0, a), stop(1.0, b)]);
    BrushSource::Literal(crate::Brush::Gradient(crate::GradientBrush::from(gradient)))
}

/// The Body frame with nothing laid out in it, for the kinds that draw straight
/// into it.
fn body_frame(out: &mut Out, body: Size, body_at: Point) -> PartKey {
    let k = key(PartRole::Body, 0, 0);
    out.frame(
        k,
        None,
        "Body",
        Rect::from_origin_size(body_at, body),
        None,
        body_item(false),
    );
    k
}

/// The nodes of a loop among `flows`, in the order it runs, if there is one.
fn find_loop(n: usize, flows: &[Flow]) -> Option<Vec<usize>> {
    // 0 unvisited, 1 on the current path, 2 done.
    let mut state = vec![0u8; n];
    let mut path = Vec::new();
    fn visit(
        i: usize,
        flows: &[Flow],
        state: &mut [u8],
        path: &mut Vec<usize>,
    ) -> Option<Vec<usize>> {
        state[i] = 1;
        path.push(i);
        for fl in flows.iter().filter(|fl| fl.from == i) {
            match state[fl.to] {
                1 => {
                    let at = path.iter().position(|p| *p == fl.to).expect("on the path");
                    return Some(path[at..].to_vec());
                }
                0 => {
                    if let Some(c) = visit(fl.to, flows, state, path) {
                        return Some(c);
                    }
                }
                _ => {}
            }
        }
        path.pop();
        state[i] = 2;
        None
    }
    (0..n).find_map(|i| {
        if state[i] == 0 {
            visit(i, flows, &mut state, &mut path)
        } else {
            None
        }
    })
}

/// One series of a scatter as drawn: a Y column, or one group's rows of the
/// first Y column, with its points as (x, y, size, row).
struct Cloud {
    /// The Y column's id, or past every column id for a group.
    id: u32,
    name: String,
    fill: BrushSource,
    /// The series' own shape, else `Points::shape`; a group takes its series'.
    shape: super::PointShape,
    points: Vec<ScatterPoint>,
}

/// A scatter's point: x, y, its bubble size, and the row it came from.
type ScatterPoint = (f64, f64, Option<f64>, usize);

/// The id a scatter's `n`th group draws under, clear of every column id.
fn group_id(n: usize) -> u32 {
    super::MAX_COLUMNS as u32 + n as u32
}

/// The name a scatter's group of rows with no group value goes by.
pub(super) const NO_GROUP: &str = "(none)";

/// The colour a named mark (a pie's slice, a scatter's group) is styled in,
/// from the description's named styles; `None` draws the palette's.
fn named_fill(chart: &Chart, name: &str) -> Option<BrushSource> {
    chart
        .nodes
        .iter()
        .find(|s| s.name == name)
        .and_then(|s| s.fill.clone())
}

/// A scatter's series: X read off the category column as numbers (a row whose
/// X is not a number left out and said), Y off each series column, or off the
/// first split by the group column; a bubble's size off the size column, a row
/// with none left out.
fn scatter_clouds(chart: &Chart, data: &mut Data) -> Vec<Cloud> {
    let t = &chart.table;
    let m = &chart.mapping;
    let colours = super::colours::drawn_palette(chart);
    let xs: Vec<Option<f64>> = data.categories.iter().map(|c| parse_number(c)).collect();
    let rows: Vec<usize> = {
        // The table's rows that survived `read` (an empty X), in order.
        let cat = m.category.and_then(|c| t.index_of(c));
        (0..t.rows.len())
            .filter(|r| cat.is_none_or(|c| !t.rows[*r][c].trim().is_empty()))
            .collect()
    };
    let mut left = Vec::new();
    for (i, x) in xs.iter().enumerate() {
        if x.is_none() {
            left.push(format!(
                "Row {}: “{}” is not a number",
                rows[i] + 1,
                data.categories[i]
            ));
        }
    }
    let column = |id: Option<ColumnId>| -> Vec<Option<String>> {
        let at = id.and_then(|c| t.index_of(c));
        rows.iter()
            .map(|r| at.map(|a| t.rows[*r][a].trim().to_string()))
            .collect()
    };
    let sizes: Vec<Option<f64>> = column(m.size)
        .into_iter()
        .map(|c| c.and_then(|c| parse_number(&c)))
        .collect();
    let bubble = m.size.is_some();
    // Why a bubble's row is left out: no size, or one below zero, which has no
    // area to draw.
    let no_area = |i: usize| -> Option<String> {
        match sizes[i] {
            None => Some(format!("Row {}: no size", rows[i] + 1)),
            Some(s) if s < 0.0 => Some(format!("Row {}: a negative size", rows[i] + 1)),
            Some(_) => None,
        }
    };
    let shape_of = |col: ColumnId| {
        chart
            .series
            .iter()
            .find(|s| s.column == col)
            .and_then(|s| s.point)
            .unwrap_or(chart.options.points.shape)
    };
    let mut out = Vec::new();
    let groups = column(m.group);
    match (m.group.is_some(), data.series.first()) {
        (true, Some(s)) => {
            let mut names: Vec<String> = Vec::new();
            for (i, g) in groups.iter().enumerate() {
                let g = g.clone().unwrap_or_default();
                let n = match names.iter().position(|n| *n == g) {
                    Some(n) => n,
                    None => {
                        names.push(g.clone());
                        let name = if g.is_empty() { NO_GROUP.into() } else { g };
                        out.push(Cloud {
                            id: group_id(names.len() - 1),
                            fill: named_fill(chart, &name)
                                .unwrap_or_else(|| rgb(colours[(names.len() - 1) % colours.len()])),
                            name,
                            shape: shape_of(s.column),
                            points: Vec::new(),
                        });
                        names.len() - 1
                    }
                };
                if let (Some(x), Some(y)) = (xs[i], s.values[i]) {
                    if let Some(why) = no_area(i).filter(|_| bubble) {
                        left.push(why);
                        continue;
                    }
                    out[n].points.push((x, y, sizes[i], rows[i]));
                }
            }
        }
        _ => {
            // A row with no size is said once, however many series it holds.
            let mut sizeless = vec![false; rows.len()];
            for s in &data.series {
                let mut points = Vec::new();
                for (i, y) in s.values.iter().enumerate() {
                    if let (Some(x), Some(y)) = (xs[i], y) {
                        if let Some(why) = no_area(i).filter(|_| bubble) {
                            if !std::mem::replace(&mut sizeless[i], true) {
                                left.push(why);
                            }
                            continue;
                        }
                        points.push((x, *y, sizes[i], rows[i]));
                    }
                }
                out.push(Cloud {
                    id: s.column.0,
                    name: s.name.clone(),
                    fill: s.fill.clone(),
                    shape: shape_of(s.column),
                    points,
                });
            }
        }
    }
    data.left_out.extend(left);
    out
}

/// A scatter or bubble chart: numbers on both axes, each series' points at
/// their place, kept at their size as the plot resizes.
fn scatter(
    chart: &Chart,
    data: &Data,
    clouds: &[Cloud],
    body: Size,
    body_at: Point,
    out: &mut Out,
) -> Result<(), Refusal> {
    let o = &chart.options;
    let p = &o.points;
    let all = || clouds.iter().flat_map(|c| c.points.iter());
    let span = |f: fn(&ScatterPoint) -> f64| {
        let (lo, hi) = all()
            .map(f)
            .fold((f64::INFINITY, f64::NEG_INFINITY), |(a, b), v| {
                (a.min(v), b.max(v))
            });
        if lo.is_finite() { (lo, hi) } else { (0.0, 1.0) }
    };
    let (x_lo, x_hi) = span(|p| p.0);
    let (y_lo, y_hi) = span(|p| p.1);
    let spec = AxisSpec {
        horizontal: false,
        lo: y_lo,
        hi: y_hi,
        zero: false,
        percent: false,
        x: Some((x_lo, x_hi)),
        ends: Vec::new(),
    };
    let ax = axes(chart, data, body, body_at, out, spec);
    ax.reference_bands(chart, data, out);
    let plot = ax.plot;
    let xt = ax.x.as_ref().expect("a scatter's X axis is numeric");
    let largest = all()
        .filter_map(|p| p.2)
        .fold(0.0f64, |a, v| a.max(v.abs()));
    for c in clouds {
        let series_key = key(PartRole::Series, c.id, 0);
        out.frame_pinned(
            series_key,
            Some(ax.plot_key),
            &format!("Series {}", c.name),
            Rect::from_origin_size(Point::ZERO, plot),
            None,
            LayoutItem::default(),
            fill_insets(),
        );
        for (x, y, size, row) in &c.points {
            let at = Point::new(xt.t(*x) * plot.width, (1.0 - ax.t.t(*y)) * plot.height);
            // A bubble's area follows its size, so its side follows the root.
            let d = match size {
                Some(s) if largest > 0.0 => (p.max_size * (s.abs() / largest).sqrt()).max(1.0),
                _ => p.size,
            };
            let bx = Rect::from_center_size(at, (d, d));
            let kind = point_kind(c.shape, bx.size());
            let (fills, strokes) = mark_paint(o, &c.fill, &out.ground);
            out.parts.push(Part {
                key: key(PartRole::Point, c.id, *row as u32),
                parent: Some(series_key),
                name: format!("{} · row {}", c.name, row + 1),
                kind,
                transform: Affine::translate(bx.origin().to_vec2()),
                fills,
                strokes,
                opacity: p.opacity as f32,
                insets: centred(
                    at.x / plot.width.max(1e-9) * 100.0,
                    at.y / plot.height.max(1e-9) * 100.0,
                ),
                display: None,
                item: LayoutItem::default(),
                clip: false,
            });
        }
    }
    ax.reference_lines(chart, data, out);
    ax.category_labels(data, out);
    Ok(())
}

/// A point's shape at `size`: a circle, a square, or a diamond or triangle
/// path, which its centring insets keep at its size.
fn point_kind(shape: super::PointShape, size: Size) -> NodeKind {
    let (w, h) = (size.width, size.height);
    let path = |pts: &[(f64, f64)]| {
        let mut p = kurbo::BezPath::new();
        p.move_to(pts[0]);
        for q in &pts[1..] {
            p.line_to(*q);
        }
        p.close_path();
        NodeKind::Path {
            path: p,
            corner_radii: Vec::new(),
            corner_smoothing: 0.0,
            mirror: crate::symmetry::PathMirror::NONE,
        }
    };
    match shape {
        super::PointShape::Circle => NodeKind::Ellipse {
            size,
            arc: Default::default(),
        },
        super::PointShape::Square => NodeKind::Rect {
            size,
            corner_radii: RoundedRectRadii::default(),
            corner_smoothing: 0.0,
        },
        super::PointShape::Diamond => {
            path(&[(w / 2.0, 0.0), (w, h / 2.0), (w / 2.0, h), (0.0, h / 2.0)])
        }
        super::PointShape::Triangle => path(&[(w / 2.0, 0.0), (w, h), (0.0, h)]),
    }
}

/// One slice of a pie as drawn: its place, category, value and colour.
struct Slice {
    index: u32,
    name: String,
    value: f64,
    fill: BrushSource,
}

/// The name a pie's folded small slices go by.
const OTHER: &str = "Other";

/// A pie's slices: the first series' values by category, a negative left out
/// and said, a zero left out, sorted if asked, and those under *Group small
/// slices*' share folded into *Other* while at least one slice stays out of
/// it. Colours run through the palette by category.
fn pie_slices(chart: &Chart, data: &mut Data) -> Vec<Slice> {
    let o = &chart.options.slices;
    let colours = super::colours::drawn_palette(chart);
    let Some(s) = data.series.first() else {
        return Vec::new();
    };
    let mut out = Vec::new();
    let mut left = Vec::new();
    for (c, v) in s.values.iter().enumerate() {
        let Some(v) = v else { continue };
        if *v < 0.0 {
            left.push(format!(
                "{}, {}: a pie cannot draw a negative value",
                s.name, data.categories[c]
            ));
            continue;
        }
        if *v == 0.0 {
            continue;
        }
        let name = data.categories[c].clone();
        out.push(Slice {
            index: c as u32,
            fill: named_fill(chart, &name).unwrap_or_else(|| rgb(colours[c % colours.len()])),
            name,
            value: *v,
        });
    }
    data.left_out.extend(left);
    // Stable, so ties keep table order.
    match o.order {
        super::Order::Table => {}
        super::Order::Descending => out.sort_by(|a, b| b.value.total_cmp(&a.value)),
        super::Order::Ascending => out.sort_by(|a, b| a.value.total_cmp(&b.value)),
    }
    if let Some(below) = o.group_below {
        let total: f64 = out.iter().map(|s| s.value).sum();
        let small = |s: &Slice| total > 0.0 && s.value / total * 100.0 < below;
        let folded: f64 = out.iter().filter(|s| small(s)).map(|s| s.value).sum();
        let n_small = out.iter().filter(|s| small(s)).count();
        // Folding every slice would leave one, which draws as a progress ring.
        if n_small > 1 && n_small < out.len() {
            out.retain(|s| !small(s));
            out.push(Slice {
                index: super::MAX_ROWS as u32,
                name: OTHER.into(),
                value: folded,
                fill: rgb(*colours.last().expect("a palette has colours")),
            });
        }
    }
    out
}

/// A pie or a donut: its slices in a square frame centred in the body, each a
/// wedge path pinned by its own box, and their labels inside or outside with a
/// leader line.
fn pie(
    chart: &Chart,
    slices: &[Slice],
    body: Size,
    body_at: Point,
    row: bool,
    out: &mut Out,
) -> Result<(), Refusal> {
    let o = &chart.options;
    let so = &o.slices;
    let body_key = key(PartRole::Body, 0, 0);
    out.frame(
        body_key,
        None,
        "Body",
        Rect::from_origin_size(body_at, body),
        None,
        body_item(row),
    );
    let side = body.width.min(body.height).max(0.0);
    let frame = Rect::from_center_size(
        Rect::from_origin_size(Point::ZERO, body).center(),
        (side, side),
    );
    let pie_key = key(PartRole::PieFrame, 0, 0);
    // Centred at its own size: the pie keeps round as the body resizes.
    out.frame_pinned(
        pie_key,
        Some(body_key),
        "Pie",
        frame,
        None,
        LayoutItem::default(),
        centred(50.0, 50.0),
    );
    let outside = o.values.position != ValuePosition::Off && o.values.pie_outside;
    let centre = Point::new(side / 2.0, side / 2.0);
    // Room for labels outside: the widest, and a leader line's length.
    let labels: Vec<String> = slices.iter().map(|s| pie_label(chart, s, slices)).collect();
    let widest = labels
        .iter()
        .map(|l| out.text.measure(l, Role::Value).width())
        .fold(0.0, f64::max);
    let r = if outside {
        (side / 2.0 - widest - LEADER).max(side / 4.0)
    } else {
        side / 2.0
    };
    let hole = if so.donut {
        r * (so.hole / 100.0).clamp(0.0, 0.95)
    } else {
        0.0
    };
    // One slice of at most 100 is a progress ring: its share of 100, over a
    // track of what is left.
    let ring = slices.len() == 1 && slices[0].value <= 100.0;
    let total: f64 = if ring {
        100.0
    } else {
        slices.iter().map(|s| s.value).sum()
    };
    let series_key = key(PartRole::Series, 0, 0);
    out.frame_pinned(
        series_key,
        Some(pie_key),
        "Slices",
        Rect::from_origin_size(Point::ZERO, Size::new(side, side)),
        None,
        LayoutItem::default(),
        fill_insets(),
    );
    let pad = so.pad_angle.to_radians();
    let mut a = so.start_angle.to_radians() - std::f64::consts::FRAC_PI_2;
    if ring {
        out.path(
            key(PartRole::Track, 0, 0),
            series_key,
            "Track".into(),
            wedge(centre, r, hole, a, a + std::f64::consts::TAU),
            Size::new(side, side),
            fill(set(&o.value_axis.grid.colour)),
            Vec::new(),
            1.0,
        );
    }
    let mut mids = Vec::new();
    for (s, text) in slices.iter().zip(&labels) {
        let sweep = if total > 0.0 {
            s.value / total * std::f64::consts::TAU
        } else {
            0.0
        };
        let (a0, a1) = if slices.len() > 1 {
            (a + pad / 2.0, a + sweep - pad / 2.0)
        } else {
            (a, a + sweep)
        };
        let mid = a + sweep / 2.0;
        a += sweep;
        // A slice too thin to draw past its padding takes no label either; a
        // lone slice (a ring at nothing) keeps its own.
        if a1 <= a0 {
            if slices.len() == 1 {
                mids.push((mid, s, text));
            }
            continue;
        }
        mids.push((mid, s, text));
        let (path, corners) = wedge_corners(centre, r, hole, a0, a1);
        let radii = if so.radius > 0.0 {
            let mut radii = vec![0.0; corners.iter().max().map_or(0, |m| m + 1)];
            for i in corners {
                radii[i] = so.radius;
            }
            radii
        } else {
            Vec::new()
        };
        let (fills, strokes) = mark_paint(o, &s.fill, &out.ground);
        out.path_rounded(
            key(PartRole::Slice, 0, s.index),
            series_key,
            format!("Slice · {}", s.name),
            (path, radii),
            Size::new(side, side),
            fills,
            strokes,
            1.0,
        );
    }

    if o.values.position != ValuePosition::Off {
        let values_key = key(PartRole::Values, 0, 0);
        out.frame_pinned(
            values_key,
            Some(pie_key),
            "Values",
            Rect::from_origin_size(Point::ZERO, Size::new(side, side)),
            None,
            LayoutItem::default(),
            fill_insets(),
        );
        for (mid, s, text) in mids {
            let b = out.text.measure(text, Role::Value);
            let dir = kurbo::Vec2::new(mid.cos(), mid.sin());
            let at = if outside {
                let from = centre + dir * r;
                let to = centre + dir * (r + LEADER * 0.6);
                let mut lead = kurbo::BezPath::new();
                lead.move_to(from);
                lead.line_to(to);
                out.path(
                    key(PartRole::LeaderLine, 0, s.index),
                    values_key,
                    format!("Leader · {}", s.name),
                    lead,
                    Size::new(side, side),
                    Vec::new(),
                    vec![line_stroke(set(&o.text.ink), 1.0, super::Dash::Solid)],
                    1.0,
                );
                // Beside the leader's end, on the side it points to.
                let end = centre + dir * (r + LEADER * 0.75);
                let x = if dir.x >= 0.0 {
                    end.x
                } else {
                    end.x - b.width()
                };
                Point::new(x, end.y - b.height() / 2.0)
            } else {
                // At the middle of the ring, or two thirds out on a pie.
                let at_r = if hole > 0.0 {
                    (r + hole) / 2.0
                } else {
                    r * 0.62
                };
                let p = centre + dir * at_r;
                Point::new(p.x - b.width() / 2.0, p.y - b.height() / 2.0)
            };
            let bx = Rect::from_origin_size(at, b.size());
            out.label(
                key(PartRole::ValueLabel, 0, s.index),
                values_key,
                format!("Value {text}"),
                text,
                Role::Value,
                Point::new(at.x - b.x0, at.y - b.y0),
                centred(
                    bx.center().x / side.max(1e-9) * 100.0,
                    bx.center().y / side.max(1e-9) * 100.0,
                ),
                0.0,
            );
            if !outside {
                out.ink_inside(o, &s.fill);
            }
        }
    }
    Ok(())
}

/// The room a pie's outside labels leave for their leader lines.
const LEADER: f64 = 16.0;

/// What a slice's label says: its value, its share or its category.
fn pie_label(chart: &Chart, s: &Slice, slices: &[Slice]) -> String {
    let f = &chart.options.values.format;
    match chart.options.values.pie {
        super::PieLabel::Category => s.name.clone(),
        super::PieLabel::Percent => {
            // A progress ring's share is of 100, as it is drawn.
            let total: f64 = match slices {
                [one] if one.value <= 100.0 => 100.0,
                _ => slices.iter().map(|s| s.value).sum(),
            };
            let mut f = f.clone();
            f.percent = true;
            scale::format(s.value / total.max(1e-12), &f, 0)
        }
        super::PieLabel::Value => scale::format(s.value, f, scale::decimals_of(s.value).min(3)),
    }
}

/// A ring's wedge from `a0` to `a1` (radians, clockwise from three o'clock on
/// screen) between radii `inner` and `outer` about `c`; a pie's when `inner` is 0.
fn wedge(c: Point, outer: f64, inner: f64, a0: f64, a1: f64) -> kurbo::BezPath {
    wedge_corners(c, outer, inner, a0, a1).0
}

/// [`wedge`], and the anchors where its edges meet at an angle (in
/// `geometry::anchor_runs`' order, which counts one anchor per element after the
/// move): the ones a slice's corner radius rounds. A whole ring has none.
fn wedge_corners(
    c: Point,
    outer: f64,
    inner: f64,
    a0: f64,
    a1: f64,
) -> (kurbo::BezPath, Vec<usize>) {
    let mut p = kurbo::BezPath::new();
    let on = |r: f64, a: f64| c + kurbo::Vec2::new(a.cos(), a.sin()) * r;
    let arc = |p: &mut kurbo::BezPath, r: f64, from: f64, to: f64| {
        let a = kurbo::Arc::new(c, (r, r), from, to - from, 0.0);
        a.to_cubic_beziers(0.01, |p1, p2, p3| p.curve_to(p1, p2, p3));
    };
    let whole = a1 - a0 >= std::f64::consts::TAU - 1e-9;
    let last = |p: &kurbo::BezPath| p.elements().len() - 1;
    let mut corners = vec![0];
    p.move_to(on(outer, a0));
    arc(&mut p, outer, a0, a1);
    corners.push(last(&p));
    if inner > 0.0 {
        p.line_to(on(inner, a1));
        corners.push(last(&p));
        arc(&mut p, inner, a1, a0);
        corners.push(last(&p));
    } else if !whole {
        p.line_to(c);
        corners.push(last(&p));
    }
    p.close_path();
    if whole {
        corners.clear();
    }
    (p, corners)
}

/// Line and area charts: per series its area (an area chart), its line and its
/// points, each a path or a shape pinned by percentage insets of the plot.
fn lines(
    chart: &Chart,
    data: &Data,
    body: Size,
    body_at: Point,
    out: &mut Out,
) -> Result<(), Refusal> {
    let o = &chart.options;
    let l = &o.lines;
    let area = chart.kind == ChartKind::Area;
    let stacking = if area {
        l.area_stacking
    } else {
        Stacking::Apart
    };
    let n_cat = data.categories.len();
    let points_on = l.points != super::PointStyle::None;
    // Points and value labels are one per value each, so either counts.
    let values_on = o.values.position != ValuePosition::Off;
    if points_on || values_on {
        let marks = n_cat * data.series.len();
        if marks > super::MAX_MARKS {
            return Err(Refusal::TooManyMarks {
                count: marks,
                what: if points_on { "points" } else { "value labels" },
            });
        }
    }

    // Each series' drawn values: its own, or stacked on the ones below it (an
    // empty cell stacks as zero), or as shares of the category's total.
    let totals: Vec<f64> = (0..n_cat)
        .map(|c| {
            data.series
                .iter()
                .filter_map(|s| s.values[c])
                .map(f64::abs)
                .sum::<f64>()
        })
        .collect();
    // Positives stack up from zero and negatives down, apart, as bars do.
    let mut ups = vec![0.0f64; n_cat];
    let mut downs = vec![0.0f64; n_cat];
    let mut layers: Vec<(Vec<Option<f64>>, Vec<f64>)> = Vec::new();
    for s in &data.series {
        let mut top = Vec::with_capacity(n_cat);
        let mut base = vec![0.0; n_cat];
        for c in 0..n_cat {
            let v = s.values[c];
            let v = match stacking {
                Stacking::Percent if totals[c] > 0.0 => v.map(|v| v / totals[c]),
                _ => v,
            };
            match (stacking, v) {
                (Stacking::Apart, _) | (_, None) => {
                    // A gap stands on what is under it, so the next run joins there.
                    base[c] = if stacking == Stacking::Apart {
                        0.0
                    } else {
                        ups[c]
                    };
                    top.push(v);
                }
                (_, Some(v)) if v >= 0.0 => {
                    base[c] = ups[c];
                    ups[c] += v;
                    top.push(Some(ups[c]));
                }
                (_, Some(v)) => {
                    base[c] = downs[c];
                    downs[c] += v;
                    top.push(Some(downs[c]));
                }
            }
        }
        layers.push((top, base));
    }
    let stand = l.connect_gaps && stacking != Stacking::Apart;
    if !points_on {
        // A lone value's dot is a mark too.
        let dots: usize = layers
            .iter()
            .map(|(top, base)| {
                runs_of(top, base, stand, l.connect_gaps)
                    .iter()
                    .filter(|r| r.len() == 1)
                    .count()
            })
            .sum();
        if dots > super::MAX_MARKS {
            return Err(Refusal::TooManyMarks {
                count: dots,
                what: "points",
            });
        }
    }
    let (mut lo, mut hi) = (f64::INFINITY, f64::NEG_INFINITY);
    for (top, base) in &layers {
        for (t, b) in top.iter().zip(base) {
            if let Some(t) = t {
                lo = lo.min(*t);
                hi = hi.max(*t);
                if area {
                    lo = lo.min(*b);
                    hi = hi.max(*b);
                }
            }
        }
    }
    if !lo.is_finite() {
        (lo, hi) = (0.0, 1.0);
    }
    // Shares end at 100% each way, as a 100% stacked bar's do.
    if stacking == Stacking::Percent {
        hi = if hi > 0.0 { 1.0 } else { 0.0 };
        lo = if lo < 0.0 { -1.0 } else { 0.0 };
    }
    // Each line's name at its last drawn value, its own last value written
    // after it under *Name, value*.
    let ends = if o.legend.line_ends {
        data.series
            .iter()
            .zip(&layers)
            .filter_map(|(s, (top, _))| {
                let c = top.iter().rposition(Option::is_some)?;
                let text = match o.legend.end_label {
                    super::EndLabel::Name => s.name.clone(),
                    super::EndLabel::NameValue => {
                        let v = s.values[c].unwrap_or(0.0);
                        let f = &o.values.format;
                        format!(
                            "{}, {}",
                            s.name,
                            scale::format(v, f, scale::decimals_of(v).min(3))
                        )
                    }
                };
                Some((s.column.0, text, top[c]?, s.fill.clone()))
            })
            .collect()
    } else {
        Vec::new()
    };
    let spec = AxisSpec {
        horizontal: false,
        lo,
        hi,
        // An area fills down to its baseline, so its axis holds zero.
        zero: area,
        percent: stacking == Stacking::Percent,
        x: None,
        ends,
    };
    let ax = axes(chart, data, body, body_at, out, spec);
    ax.reference_bands(chart, data, out);
    let plot = ax.plot;
    let at =
        |c: usize, v: f64| Point::new((c as f64 + 0.5) * ax.band, (1.0 - ax.t.t(v)) * plot.height);

    for (si, s) in data.series.iter().enumerate() {
        let (top, base) = &layers[si];
        let style = chart.series.iter().find(|st| st.column == s.column);
        let series_key = key(PartRole::Series, s.column.0, 0);
        out.frame_pinned(
            series_key,
            Some(ax.plot_key),
            &format!("Series {}", s.name),
            Rect::from_origin_size(Point::ZERO, plot),
            None,
            LayoutItem::default(),
            fill_insets(),
        );
        let runs = runs_of(top, base, stand, l.connect_gaps);
        let mut area_path = None;

        if area {
            let mut path = kurbo::BezPath::new();
            for run in &runs {
                let pts: Vec<Point> = run.iter().map(|(c, v)| at(*c, *v)).collect();
                curve(&mut path, &pts, l.curve);
                // Back along what it stands on, right to left.
                let floor: Vec<Point> = run.iter().rev().map(|(c, _)| at(*c, base[*c])).collect();
                if let Some(first) = floor.first() {
                    path.line_to(*first);
                }
                let mut back = kurbo::BezPath::new();
                curve(
                    &mut back,
                    &floor,
                    if stacking == Stacking::Apart {
                        super::Curve::Straight
                    } else {
                        l.curve
                    },
                );
                for el in back.elements().iter().skip(1) {
                    path.push(*el);
                }
                path.close_path();
            }
            area_path = Some(path.clone());
            // From the area's far edge to its baseline, in the plot's space.
            let reach = path.bounding_box();
            let base = ((1.0 - ax.t.t(0.0)) * plot.height).clamp(0.0, plot.height);
            let far = if (reach.y0 - base).abs() >= (reach.y1 - base).abs() {
                reach.y0
            } else {
                reach.y1
            };
            out.path(
                key(PartRole::Area, s.column.0, 0),
                series_key,
                format!("Area · {}", s.name),
                path,
                plot,
                fill(s.fill.clone()),
                Vec::new(),
                l.area_opacity as f32,
            );
            fade_last(out, o, Point::new(0.0, far), Point::new(0.0, base), &s.name);
        }
        let mut path = kurbo::BezPath::new();
        for run in &runs {
            let pts: Vec<Point> = run.iter().map(|(c, v)| at(*c, *v)).collect();
            curve(&mut path, &pts, l.curve);
        }
        let dash = style.map_or(super::Dash::Solid, |st| st.dash);
        let line_path = path.clone();
        out.path(
            key(PartRole::Line, s.column.0, 0),
            series_key,
            format!("Line · {}", s.name),
            path,
            plot,
            Vec::new(),
            vec![line_stroke(s.fill.clone(), l.width, dash)],
            1.0,
        );
        // Below the baseline the line and area draw again in the below-zero
        // colour, inside a frame that clips them there, so the colours meet on
        // the baseline whatever the curve.
        if let Some(below) = &o.below_zero
            && stacking != Stacking::Percent
            && top.iter().flatten().any(|v| *v < 0.0)
        {
            let zy = ((1.0 - ax.t.t(0.0)) * plot.height).clamp(0.0, plot.height);
            let size = Size::new(plot.width, (plot.height - zy).max(0.0));
            let bz = key(PartRole::Series, s.column.0, 1);
            out.frame_pinned(
                bz,
                Some(series_key),
                "Below zero",
                Rect::from_origin_size(Point::new(0.0, zy), size),
                None,
                LayoutItem::default(),
                Insets {
                    left: pct(0.0),
                    right: pct(0.0),
                    top: pct(zy / plot.height.max(1e-9) * 100.0),
                    bottom: pct(0.0),
                    margin_auto: AutoMargins::default(),
                },
            );
            if let Some(p) = out.parts.last_mut() {
                p.clip = true;
            }
            let into = Affine::translate((0.0, -zy));
            if let Some(a) = area_path {
                out.path(
                    key(PartRole::Area, s.column.0, 1),
                    bz,
                    format!("Area · {} · below zero", s.name),
                    into * a,
                    size,
                    fill(below.clone()),
                    Vec::new(),
                    l.area_opacity as f32,
                );
            }
            out.path(
                key(PartRole::Line, s.column.0, 1),
                bz,
                format!("Line · {} · below zero", s.name),
                into * line_path,
                size,
                Vec::new(),
                vec![line_stroke(below.clone(), l.width, dash)],
                1.0,
            );
        }
        // A value with no neighbour draws no line, so it is a dot as wide as
        // the line, unless points mark every value already.
        if !points_on {
            for run in runs.iter().filter(|r| r.len() == 1) {
                let (c, v) = run[0];
                let p = at(c, v);
                let bx = Rect::from_center_size(p, (l.width, l.width));
                out.parts.push(Part {
                    key: key(PartRole::Point, s.column.0, c as u32),
                    parent: Some(series_key),
                    name: format!("{} · {}", s.name, data.categories[c]),
                    kind: point_kind(super::PointShape::Circle, bx.size()),
                    transform: Affine::translate(bx.origin().to_vec2()),
                    fills: fill(s.fill.clone()),
                    strokes: Vec::new(),
                    opacity: 1.0,
                    insets: centred(
                        p.x / plot.width.max(1e-9) * 100.0,
                        p.y / plot.height.max(1e-9) * 100.0,
                    ),
                    display: None,
                    item: LayoutItem::default(),
                    clip: false,
                });
            }
        }
        if points_on {
            let size = l.point_size;
            let hollow = l.points == super::PointStyle::Hollow;
            let shape = style.and_then(|st| st.point).unwrap_or_default();
            for run in &runs {
                for (c, v) in run.iter().filter(|(c, _)| top[*c].is_some()) {
                    let p = at(*c, *v);
                    let bx = Rect::from_center_size(p, (size, size));
                    let (fills, strokes) = if hollow {
                        (
                            fill(o.plot.fill.clone().unwrap_or_else(|| out.ground.clone())),
                            vec![line_stroke(
                                s.fill.clone(),
                                (l.width).min(size / 3.0),
                                super::Dash::Solid,
                            )],
                        )
                    } else {
                        mark_paint(o, &s.fill, &out.ground)
                    };
                    out.parts.push(Part {
                        key: key(PartRole::Point, s.column.0, *c as u32),
                        parent: Some(series_key),
                        name: format!("{} · {}", s.name, data.categories[*c]),
                        kind: point_kind(shape, bx.size()),
                        transform: Affine::translate(bx.origin().to_vec2()),
                        fills,
                        strokes,
                        opacity: 1.0,
                        insets: centred(
                            p.x / plot.width.max(1e-9) * 100.0,
                            p.y / plot.height.max(1e-9) * 100.0,
                        ),
                        display: None,
                        item: LayoutItem::default(),
                        clip: false,
                    });
                }
            }
        }
    }

    // Value labels sit above their point.
    if values_on {
        let values_key = key(PartRole::Values, 0, 0);
        out.frame_pinned(
            values_key,
            Some(ax.plot_key),
            "Values",
            Rect::from_origin_size(Point::ZERO, plot),
            None,
            LayoutItem::default(),
            fill_insets(),
        );
        for (si, s) in data.series.iter().enumerate() {
            for (c, v) in layers[si].0.iter().enumerate() {
                let (Some(v), Some(raw)) = (v, s.values[c]) else {
                    continue;
                };
                let mut f = o.values.format.clone();
                if stacking == Stacking::Percent {
                    f.percent = true;
                }
                let decimals = if f.percent {
                    0
                } else {
                    scale::decimals_of(raw).min(3)
                };
                let shown_v = if stacking == Stacking::Percent {
                    raw / totals[c].max(1e-12)
                } else {
                    raw
                };
                let text = scale::format(shown_v, &f, decimals);
                let b = out.text.measure(&text, Role::Value);
                let p = at(c, *v);
                let lift = VALUE_GAP + l.point_size.max(l.width) / 2.0;
                let top_y = p.y - lift - b.height();
                let mut insets = centred_x(p.x / plot.width.max(1e-9) * 100.0);
                insets.top = pct(top_y / plot.height.max(1e-9) * 100.0);
                out.label(
                    key(PartRole::ValueLabel, s.column.0, c as u32),
                    values_key,
                    format!("Value {text}"),
                    &text,
                    Role::Value,
                    Point::new(p.x - b.width() / 2.0 - b.x0, top_y - b.y0),
                    insets,
                    0.0,
                );
            }
        }
    }
    ax.reference_lines(chart, data, out);
    ax.category_labels(data, out);
    Ok(())
}

/// The runs a series' line draws, each `(category, value)`: broken at an empty
/// cell unless `connect`. A stacked area's gap with `stand` is visited at what
/// it stands on (`base`), zero thick, so the line and the band above it join on
/// the stack rather than across it.
fn runs_of(
    top: &[Option<f64>],
    base: &[f64],
    stand: bool,
    connect: bool,
) -> Vec<Vec<(usize, f64)>> {
    let mut runs: Vec<Vec<(usize, f64)>> = vec![Vec::new()];
    for (c, v) in top.iter().enumerate() {
        let run = runs.last_mut().expect("one run");
        match v {
            Some(v) => run.push((c, *v)),
            None if stand => run.push((c, base[c])),
            None if !connect && !run.is_empty() => runs.push(Vec::new()),
            None => {}
        }
    }
    // A run starts and ends at a value: only a gap between two is stood on.
    for run in &mut runs {
        while run.last().is_some_and(|(c, _)| top[*c].is_none()) {
            run.pop();
        }
        let lead = run.iter().take_while(|(c, _)| top[*c].is_none()).count();
        run.drain(..lead);
    }
    runs.retain(|r| !r.is_empty());
    runs
}

/// `pts` appended to `path` as `curve` draws them, starting with a move.
///
/// *Smooth* is a monotone cubic (Fritsch–Carlson): each point's tangent is
/// limited so the curve never overshoots a value the data does not have.
/// *Step* turns halfway between points.
fn curve(path: &mut kurbo::BezPath, pts: &[Point], curve: super::Curve) {
    let Some(first) = pts.first() else { return };
    path.move_to(*first);
    match curve {
        super::Curve::Straight => {
            for p in &pts[1..] {
                path.line_to(*p);
            }
        }
        super::Curve::Step => {
            for w in pts.windows(2) {
                let mid = (w[0].x + w[1].x) / 2.0;
                path.line_to((mid, w[0].y));
                path.line_to((mid, w[1].y));
                path.line_to(w[1]);
            }
        }
        super::Curve::Smooth => {
            let n = pts.len();
            // Points with no room between them (a plot of no width) have no
            // slope to take, so they draw straight. Either direction is a
            // curve: an area's floor runs back right to left.
            let rising = pts.windows(2).all(|w| w[1].x > w[0].x);
            let falling = pts.windows(2).all(|w| w[1].x < w[0].x);
            if n < 3 || !(rising || falling) {
                for p in &pts[1..] {
                    path.line_to(*p);
                }
                return;
            }
            let slope = |i: usize| (pts[i + 1].y - pts[i].y) / (pts[i + 1].x - pts[i].x);
            let d: Vec<f64> = (0..n - 1).map(slope).collect();
            let mut m = vec![0.0; n];
            m[0] = d[0];
            m[n - 1] = d[n - 2];
            for i in 1..n - 1 {
                m[i] = if d[i - 1] * d[i] <= 0.0 {
                    0.0
                } else {
                    (d[i - 1] + d[i]) / 2.0
                };
            }
            for i in 0..n - 1 {
                if d[i] == 0.0 {
                    m[i] = 0.0;
                    m[i + 1] = 0.0;
                    continue;
                }
                let (a, b) = (m[i] / d[i], m[i + 1] / d[i]);
                let s = a * a + b * b;
                if s > 9.0 {
                    let t = 3.0 / s.sqrt();
                    m[i] = t * a * d[i];
                    m[i + 1] = t * b * d[i];
                }
            }
            for i in 0..n - 1 {
                let (p0, p1) = (pts[i], pts[i + 1]);
                let h = (p1.x - p0.x) / 3.0;
                path.curve_to(
                    (p0.x + h, p0.y + m[i] * h),
                    (p1.x - h, p1.y - m[i + 1] * h),
                    (p1.x, p1.y),
                );
            }
        }
    }
}

/// A series line's stroke: round joins and caps, dashed as the series says.
fn line_stroke(brush: BrushSource, width: f64, dash: super::Dash) -> Stroke {
    let (dashes, cap) = match dash {
        super::Dash::Solid => (Vec::new(), kurbo::Cap::Round),
        super::Dash::Dashed => (vec![width * 3.0, width * 2.0], kurbo::Cap::Butt),
        super::Dash::Dotted => (vec![0.0, width * 2.0], kurbo::Cap::Round),
    };
    Stroke {
        brush,
        width,
        width_var: None,
        join: kurbo::Join::Round,
        cap,
        dashes,
        dash_offset: 0.0,
        sides: crate::node::StrokeSides::All,
        miter_limit: 4.0,
        dash_fit: false,
        align: crate::node::StrokeAlign::Center,
        ..Default::default()
    }
}

/// Insets centring a box on (`x`, `y`) percent of its parent.
fn centred(x: f64, y: f64) -> Insets {
    let mut i = centred_x(x);
    let v = centred_y(y);
    i.top = v.top;
    i.bottom = v.bottom;
    i.margin_auto.top = true;
    i.margin_auto.bottom = true;
    i
}

/// Insets of 0 on every side: a container covering its parent.
fn fill_insets() -> Insets {
    Insets {
        left: px(0.0),
        right: px(0.0),
        top: px(0.0),
        bottom: px(0.0),
        margin_auto: AutoMargins::default(),
    }
}

/// Insets centring a box horizontally on `x` percent of its parent.
fn centred_x(x: f64) -> Insets {
    Insets {
        left: pct(x),
        right: pct(100.0 - x),
        margin_auto: AutoMargins {
            left: true,
            right: true,
            ..AutoMargins::default()
        },
        ..Insets::default()
    }
}

/// Insets centring a box vertically on `y` percent of its parent.
fn centred_y(y: f64) -> Insets {
    Insets {
        top: pct(y),
        bottom: pct(100.0 - y),
        margin_auto: AutoMargins {
            top: true,
            bottom: true,
            ..AutoMargins::default()
        },
        ..Insets::default()
    }
}

/// A 1 px rule across the plot at `share` along the value axis, and its insets.
fn rule_at(horizontal: bool, share: f64, plot: Size, width: f64) -> (Rect, Insets) {
    let h = width / 2.0;
    if horizontal {
        let x = share * plot.width;
        let mut insets = centred_x(share * 100.0);
        insets.top = px(0.0);
        insets.bottom = px(0.0);
        (Rect::new(x - h, 0.0, x + h, plot.height), insets)
    } else {
        let y = (1.0 - share) * plot.height;
        let mut insets = centred_y((1.0 - share) * 100.0);
        insets.left = px(0.0);
        insets.right = px(0.0);
        (Rect::new(0.0, y - h, plot.width, y + h), insets)
    }
}

/// A tick mark in an axis frame `frame` big: a 1 px stub [`TICK_LEN`] long at
/// `along` (a share of the frame along its axis, from its left or top), across
/// the axis from the frame's start edge (left or top) or its end. Centred like a
/// tick label, so it keeps its size while its place follows the plot.
fn tick_mark(
    stub_horizontal: bool,
    along: f64,
    from_start: bool,
    frame: Size,
    width: f64,
) -> (Rect, Insets) {
    let h = width / 2.0;
    if stub_horizontal {
        let y = along * frame.height;
        let x = if from_start {
            0.0
        } else {
            frame.width - TICK_LEN
        };
        let mut insets = centred_y(along * 100.0);
        if from_start {
            insets.left = px(0.0);
        } else {
            insets.right = px(0.0);
        }
        (Rect::new(x, y - h, x + TICK_LEN, y + h), insets)
    } else {
        let x = along * frame.width;
        let y = if from_start {
            0.0
        } else {
            frame.height - TICK_LEN
        };
        let mut insets = centred_x(along * 100.0);
        if from_start {
            insets.top = px(0.0);
        } else {
            insets.bottom = px(0.0);
        }
        (Rect::new(x - h, y, x + h, y + TICK_LEN), insets)
    }
}

/// A tick label's corner and insets in its axis frame `frame` big: its box `b`,
/// turned `turn` about its centre, centred at `centre` along the axis (along x
/// when `along_x`, else along y) and standing `gap` off the frame's edge across
/// it, the start edge (top or left) when `at_start`, else the far one. The
/// insets place the unturned box, which layout turns about its centre, so the
/// centre sits as far from the edge as the turned box reaches. A turned label
/// that would reach past either end of the axis is pinned inside that end
/// instead, so the last tick's label is not drawn over the legend.
fn turned_label(
    b: Rect,
    turn: f64,
    along_x: bool,
    centre: f64,
    gap: f64,
    frame: Size,
    at_start: bool,
) -> (Point, Insets) {
    let (reach_x, reach_y) = turned_reach(b, turn);
    let (w, h) = (b.width(), b.height());
    let (along_len, along_reach, across_len, across_reach, along_size, across_size) = if along_x {
        (frame.width, reach_x, frame.height, reach_y, w, h)
    } else {
        (frame.height, reach_y, frame.width, reach_x, h, w)
    };
    // Across: as far out as the turned box reaches.
    let edge = gap + (across_reach - across_size) / 2.0;
    let across_at = if at_start {
        edge
    } else {
        across_len - gap - (across_reach + across_size) / 2.0
    };
    // Along: centred on its tick, or pinned inside an end it would pass.
    let half = along_reach / 2.0;
    // `Some(true)` pins it to the axis's start, `Some(false)` to its end.
    let pin = if turn == 0.0 {
        None
    } else if centre - half < 0.0 {
        Some(true)
    } else if centre + half > along_len {
        Some(false)
    } else {
        None
    };
    let inset = half - along_size / 2.0;
    let (along_at, mut insets) = match pin {
        Some(true) => {
            let mut i = Insets::default();
            if along_x {
                i.left = px(inset);
            } else {
                i.top = px(inset);
            }
            (inset, i)
        }
        Some(false) => {
            let mut i = Insets::default();
            if along_x {
                i.right = px(inset);
            } else {
                i.bottom = px(inset);
            }
            (along_len - half - along_size / 2.0, i)
        }
        None => {
            let share = centre / along_len.max(1e-9) * 100.0;
            (
                centre - along_size / 2.0,
                if along_x {
                    centred_x(share)
                } else {
                    centred_y(share)
                },
            )
        }
    };
    match (along_x, at_start) {
        (true, true) => insets.top = px(edge),
        (true, false) => insets.bottom = px(edge),
        (false, true) => insets.left = px(edge),
        (false, false) => insets.right = px(edge),
    }
    let corner = if along_x {
        Point::new(along_at, across_at)
    } else {
        Point::new(across_at, along_at)
    };
    (corner - b.origin().to_vec2(), insets)
}

/// `text` (its box `b` at `size`) cut to `length` with an ellipsis when it is
/// longer, and the box it then takes: an axis title runs no further than its
/// axis.
fn within(text: &str, b: Rect, length: f64, measure: &Text, size: Role) -> (String, Rect) {
    if b.width() <= length {
        return (text.to_string(), b);
    }
    let chars: Vec<char> = text.chars().collect();
    let cut = |k: usize| -> String {
        let mut s: String = chars[..k].iter().collect::<String>().trim_end().to_string();
        s.push('…');
        s
    };
    // The most characters that fit with the ellipsis, by halving.
    let (mut lo, mut hi) = (0, chars.len());
    while lo < hi {
        let mid = (lo + hi).div_ceil(2);
        if measure.measure(&cut(mid), size).width() <= length {
            lo = mid;
        } else {
            hi = mid - 1;
        }
    }
    let s = cut(lo);
    let b = measure.measure(&s, size);
    (s, b)
}

/// An axis title's corner, insets and turn in its axis frame `frame` big, at the
/// frame's start edge (left or top) or, `at_end`, its end, centred along it.
/// A side axis's title reads upwards, turned a quarter; its insets place the
/// unturned box, which layout turns about its centre.
fn title_place(b: Rect, frame: Size, turned: bool, at_end: bool) -> (Point, Insets, f64) {
    if turned {
        let cx = if at_end {
            frame.width - b.height() / 2.0
        } else {
            b.height() / 2.0
        };
        let cy = frame.height / 2.0;
        let mut insets = centred_y(50.0);
        let reach = b.height() / 2.0 - b.width() / 2.0;
        if at_end {
            insets.right = px(reach);
        } else {
            insets.left = px(reach);
        }
        let at = Point::new(cx - b.width() / 2.0 - b.x0, cy - b.height() / 2.0 - b.y0);
        (at, insets, -std::f64::consts::FRAC_PI_2)
    } else {
        let top = if at_end {
            frame.height - b.height()
        } else {
            0.0
        };
        let mut insets = centred_x(50.0);
        if at_end {
            insets.bottom = px(0.0);
        } else {
            insets.top = px(0.0);
        }
        let at = Point::new(frame.width / 2.0 - b.width() / 2.0 - b.x0, top - b.y0);
        (at, insets, 0.0)
    }
}

/// Every `n`th label of a band `band` long that keeps labels `need` long apart.
fn thin_every(need: f64, band: f64) -> usize {
    if band <= 0.0 {
        return 1;
    }
    ((need / band).ceil() as usize).max(1)
}

/// How far category labels turn and how many are kept, for columns: flat where
/// they fit; *Auto* turns them 45° and then thins them where they would touch.
fn category_turn(angle: LabelAngle, boxes: &[Rect], band: f64, label_h: f64) -> (f64, usize) {
    let widest = boxes.iter().map(Rect::width).fold(0.0, f64::max);
    let room = band - 4.0;
    let slant = set_turn(LabelAngle::Slanted);
    match angle {
        LabelAngle::Flat => (0.0, thin_every(widest + 4.0, band)),
        LabelAngle::Slanted => (slant, thin_every(label_h * 1.5, band)),
        LabelAngle::Upright => (set_turn(angle), thin_every(label_h * 1.2, band)),
        LabelAngle::Auto if widest <= room => (0.0, 1),
        LabelAngle::Auto => (slant, thin_every(label_h * 1.5, band)),
    }
}

/// A tick label's turn at its set angle, for the axes [`category_turn`] does
/// not decide: *Auto* keeps them flat.
fn set_turn(angle: LabelAngle) -> f64 {
    match angle {
        LabelAngle::Auto | LabelAngle::Flat => 0.0,
        LabelAngle::Slanted => -std::f64::consts::FRAC_PI_4,
        LabelAngle::Upright => -std::f64::consts::FRAC_PI_2,
    }
}

/// How far a label's box `b`, turned `turn` about its centre, reaches across
/// and down. Flat, it is exactly the box's width and height.
fn turned_reach(b: Rect, turn: f64) -> (f64, f64) {
    let (s, c) = turn.sin_cos();
    (
        b.width() * c.abs() + b.height() * s.abs(),
        b.width() * s.abs() + b.height() * c.abs(),
    )
}

/// Where a value label sits on its bar `bar` (in the plot `plot`), its box `b`:
/// its corner and its insets, or `None` when it does not fit inside.
fn value_place(
    at: ValuePosition,
    horizontal: bool,
    up: bool,
    bar: Rect,
    b: Rect,
    plot: Size,
) -> Option<(Point, Insets)> {
    let fits = b.width() + 2.0 <= bar.width() && b.height() + 2.0 <= bar.height();
    let share_x = |x: f64| x / plot.width.max(1e-9) * 100.0;
    let share_y = |y: f64| y / plot.height.max(1e-9) * 100.0;
    let c = bar.center();
    if horizontal {
        let y = c.y - b.height() / 2.0 - b.y0;
        let mut insets = centred_y(share_y(c.y));
        let x = match at {
            ValuePosition::Off => return None,
            ValuePosition::Inside if fits => c.x - b.width() / 2.0,
            ValuePosition::End if fits && up => bar.x1 - VALUE_GAP - b.width(),
            ValuePosition::End if fits => bar.x0 + VALUE_GAP,
            ValuePosition::Outside if up => bar.x1 + VALUE_GAP,
            ValuePosition::Outside => bar.x0 - VALUE_GAP - b.width(),
            _ => return None,
        };
        insets.left = pct(share_x(x));
        Some((Point::new(x - b.x0, y), insets))
    } else {
        let x = c.x - b.width() / 2.0 - b.x0;
        let mut insets = centred_x(share_x(c.x));
        let y = match at {
            ValuePosition::Off => return None,
            ValuePosition::Inside if fits => c.y - b.height() / 2.0,
            ValuePosition::End if fits && up => bar.y0 + VALUE_GAP,
            ValuePosition::End if fits => bar.y1 - VALUE_GAP - b.height(),
            ValuePosition::Outside if up => bar.y0 - VALUE_GAP - b.height(),
            ValuePosition::Outside => bar.y1 + VALUE_GAP,
            _ => return None,
        };
        insets.top = pct(share_y(y));
        Some((Point::new(x, y - b.y0), insets))
    }
}

#[cfg(test)]
mod category_box_tests {
    use super::*;

    /// A width where `W` is three times `i`, and leading and trailing spaces
    /// set nothing, as in a laid line.
    fn weighted(s: &str) -> Rect {
        let w: f64 = s
            .trim()
            .chars()
            .map(|c| if c == 'W' { 3.0 } else { 1.0 })
            .sum();
        Rect::new(0.0, 0.0, w, 10.0)
    }

    /// Past the cap only 64 labels are measured whole, and no box is under its
    /// label's real size, though the labels longest by characters are the
    /// narrowest and the widest are wide only with their spaces.
    /// flip: `category_boxes` measuring a space alone, or picking the labels
    /// longest by characters with the stand-in from those alone → fails at
    /// "never under a label's size". Either the ranking or the stand-in alone
    /// keeps the boxes covered.
    #[test]
    fn many_labels_measure_only_the_widest() {
        let names: Vec<String> = (0..1000)
            .map(|i| match i % 3 {
                0 => "i".repeat(12),
                1 => "W".repeat(7),
                _ => "W W W W W W".to_string(),
            })
            .collect();
        let real: Vec<f64> = names[..3].iter().map(|n| weighted(n).width()).collect();
        assert_eq!(real, [12.0, 21.0, 23.0], "the fixture's widths");
        let whole = std::cell::Cell::new(0);
        let boxes = category_boxes(&names, |s| {
            if names.iter().any(|n| n == s) {
                whole.set(whole.get() + 1);
            }
            weighted(s)
        });
        assert_eq!(whole.get(), 64, "measured whole");
        for (b, n) in boxes.iter().zip(&names) {
            assert!(
                b.width() >= weighted(n).width(),
                "never under a label's size: {n}"
            );
        }
    }
}
