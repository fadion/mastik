//! A chart's colours as the generator assigns them: a series' or a named
//! mark's (a Sankey node, a pie's slice, a scatter's group) own style, else
//! the palette's colour at its place. The Chart card's swatches and the picker
//! read and write through these.

use super::{Chart, ColumnId, NodeStyle, SeriesStyle, palette, parse_number};
use crate::variable::BrushSource;

fn literal(c: [u8; 3]) -> BrushSource {
    BrushSource::Literal(crate::Brush::Solid(peniko::Color::from_rgb8(
        c[0], c[1], c[2],
    )))
}

/// The colour an unset series or named mark takes at place `i`: the *From
/// variables* group's (`Options::palette_group`), else the palette's for the
/// theme `chart` is drawn in, round again past the last either way.
pub fn place_fill(chart: &Chart, i: usize) -> BrushSource {
    let group = &chart.options.palette_group;
    if !group.is_empty() {
        return group[i % group.len()].clone();
    }
    let colours = palette(chart.options.palette, chart.drawn_dark());
    literal(colours[i % colours.len()])
}

/// The fill series `column` draws in: its style's, else [`place_fill`] at its
/// place among the mapped series.
pub fn series_fill(chart: &Chart, column: ColumnId) -> Option<BrushSource> {
    if let Some(f) = chart
        .series
        .iter()
        .find(|s| s.column == column)
        .and_then(|s| s.fill.clone())
    {
        return Some(f);
    }
    let n = chart.mapping.series.iter().position(|c| *c == column)?;
    Some(place_fill(chart, n))
}

/// Whether series `column`'s colour is set rather than the palette's.
pub fn series_fill_set(chart: &Chart, column: ColumnId) -> bool {
    chart
        .series
        .iter()
        .any(|s| s.column == column && s.fill.is_some())
}

/// The fill a named mark's style gives, `None` when it takes the palette's.
fn node_style(chart: &Chart, name: &str) -> Option<BrushSource> {
    chart
        .nodes
        .iter()
        .find(|s| s.name == name)
        .and_then(|s| s.fill.clone())
}

/// A Sankey's node names in the order the generator meets them (rows it draws
/// only), each with the fill it draws in.
pub fn sankey_nodes(chart: &Chart) -> Vec<(String, BrushSource)> {
    let t = &chart.table;
    let m = &chart.mapping;
    let at = |c: Option<ColumnId>| c.and_then(|c| t.index_of(c));
    let (Some(fc), Some(tc), Some(vc)) = (at(m.from), at(m.to), at(m.value)) else {
        return Vec::new();
    };
    let mut out: Vec<(String, BrushSource)> = Vec::new();
    for row in &t.rows {
        let (from, to) = (row[fc].trim(), row[tc].trim());
        if from.is_empty() || to.is_empty() || from == to {
            continue;
        }
        if parse_number(&row[vc]).is_none_or(|v| v <= 0.0) {
            continue;
        }
        for name in [from, to] {
            if out.iter().any(|(n, _)| n == name) {
                continue;
            }
            let i = out.len();
            let fill = node_style(chart, name).unwrap_or_else(|| place_fill(chart, i));
            out.push((name.to_string(), fill));
        }
    }
    out
}

/// The marks a chart colours by name rather than by series, in the order the
/// generator meets them, each with the fill it draws in: a Sankey's nodes, a
/// pie's slices (by category) and a grouped scatter's groups. Each takes its
/// style from `Chart::nodes` by name, else the palette at its place. `None`
/// for a chart coloured by series.
pub fn named_marks(chart: &Chart) -> Option<Vec<(String, BrushSource)>> {
    let marks = mark_places(chart)?;
    let fill =
        |name: &str, i: usize| node_style(chart, name).unwrap_or_else(|| place_fill(chart, i));
    Some(
        marks
            .into_iter()
            .map(|(n, i)| {
                let f = fill(&n, i);
                (n, f)
            })
            .collect(),
    )
}

/// Whether a column or bar chart colours its bars by category: asked to, with
/// one series shown.
pub fn by_category(chart: &Chart) -> bool {
    let shown = chart
        .mapping
        .series
        .iter()
        .filter(|c| !chart.series.iter().any(|s| s.column == **c && !s.visible))
        .count();
    chart.options.by_category
        && shown == 1
        && matches!(chart.kind, super::ChartKind::Column | super::ChartKind::Bar)
}

/// Each of [`named_marks`]' marks with its place in the palette: a pie slice's
/// at its category row, counting the rows it does not draw, every other mark's
/// at its order among the marks.
fn mark_places(chart: &Chart) -> Option<Vec<(String, usize)>> {
    let t = &chart.table;
    let m = &chart.mapping;
    let at = |c: Option<ColumnId>| c.and_then(|c| t.index_of(c));
    // The rows the generator reads: those with a category.
    let cat = at(m.category);
    let rows = || {
        t.rows
            .iter()
            .filter(move |r| cat.is_none_or(|c| !r[c].trim().is_empty()))
    };
    let mut out: Vec<(String, usize)> = Vec::new();
    match chart.kind {
        super::ChartKind::Sankey => {
            let nodes = sankey_nodes(chart).into_iter().enumerate();
            return Some(nodes.map(|(i, (n, _))| (n, i)).collect());
        }
        super::ChartKind::Pie => {
            // The first visible series, as the generator reads it.
            let first = m
                .series
                .iter()
                .find(|c| !chart.series.iter().any(|s| s.column == **c && !s.visible));
            let Some(vc) = at(first.copied()) else {
                return Some(out);
            };
            for (i, r) in rows().enumerate() {
                let name = cat.map_or_else(|| (i + 1).to_string(), |c| r[c].trim().to_string());
                let drawn = parse_number(&r[vc]).is_some_and(|v| v > 0.0);
                if drawn && !out.iter().any(|(n, _)| *n == name) {
                    out.push((name, i));
                }
            }
        }
        super::ChartKind::Column | super::ChartKind::Bar if by_category(chart) => {
            for (i, r) in rows().enumerate() {
                let name = cat.map_or_else(|| (i + 1).to_string(), |c| r[c].trim().to_string());
                if !out.iter().any(|(n, _)| *n == name) {
                    out.push((name, i));
                }
            }
        }
        super::ChartKind::Scatter if m.group.is_some() => {
            let Some(gc) = at(m.group) else {
                return Some(out);
            };
            for r in rows() {
                let g = r[gc].trim();
                let name = if g.is_empty() {
                    super::generate::NO_GROUP
                } else {
                    g
                };
                if !out.iter().any(|(n, _)| n == name) {
                    let i = out.len();
                    out.push((name.to_string(), i));
                }
            }
        }
        _ => return None,
    }
    Some(out)
}

/// Each of [`sankey_nodes`]' nodes' value, in its order: the larger of what flows
/// in and what flows out, as the generator sizes the node.
pub fn sankey_totals(chart: &Chart) -> Vec<f64> {
    let names: Vec<String> = sankey_nodes(chart).into_iter().map(|(n, _)| n).collect();
    let t = &chart.table;
    let m = &chart.mapping;
    let at = |c: Option<ColumnId>| c.and_then(|c| t.index_of(c));
    let (Some(fc), Some(tc), Some(vc)) = (at(m.from), at(m.to), at(m.value)) else {
        return Vec::new();
    };
    let (mut ins, mut outs) = (vec![0.0; names.len()], vec![0.0; names.len()]);
    let index = |name: &str| names.iter().position(|n| n == name);
    for row in &t.rows {
        let (from, to) = (row[fc].trim(), row[tc].trim());
        let Some(v) = parse_number(&row[vc]).filter(|v| *v > 0.0) else {
            continue;
        };
        if from == to {
            continue;
        }
        if let (Some(a), Some(b)) = (index(from), index(to)) {
            outs[a] += v;
            ins[b] += v;
        }
    }
    ins.iter().zip(&outs).map(|(i, o)| i.max(*o)).collect()
}

/// `chart` with series `column` drawn in `fill` (`None` puts it back on the
/// palette), its style made if it had none.
pub fn with_series_fill(chart: &Chart, column: ColumnId, fill: Option<BrushSource>) -> Chart {
    let mut next = chart.clone();
    match next.series.iter_mut().find(|s| s.column == column) {
        Some(s) => s.fill = fill,
        None => next.series.push(SeriesStyle {
            column,
            fill,
            visible: true,
            dash: Default::default(),
            point: Default::default(),
        }),
    }
    next
}

/// `chart` with the named mark `name` (a Sankey node, a pie's slice, a
/// scatter's group: [`named_marks`]) drawn in `fill`; `None` puts it back on
/// the palette.
pub fn with_node_fill(chart: &Chart, name: &str, fill: Option<BrushSource>) -> Chart {
    let mut next = chart.clone();
    match fill {
        Some(fill) => match next.nodes.iter_mut().find(|s| s.name == name) {
            Some(s) => s.fill = Some(fill),
            None => next.nodes.push(NodeStyle {
                name: name.to_string(),
                fill: Some(fill),
            }),
        },
        None => next.nodes.retain(|s| s.name != name),
    }
    next
}

/// `chart` on palette `p`, every series and node back on it, so picking a
/// palette recolours the whole chart.
pub fn with_palette(chart: &Chart, p: super::Palette) -> Chart {
    let mut next = chart.clone();
    next.options.palette = p;
    next.options.palette_group.clear();
    unset_marks(&mut next);
    next
}

/// Every series and named mark of `chart` back on its palette.
fn unset_marks(next: &mut Chart) {
    for s in &mut next.series {
        s.fill = None;
    }
    next.nodes.clear();
}

/// `chart` as a file from before derived colours reads: its theme *Dark* when
/// `dark`, else *Light*, and every colour still at that theme's starting value
/// unset (a series' or a named mark's palette colour at its place, the ink,
/// axes and gridlines at [`super::chrome`]'s), so it draws as it did and then
/// follows its theme.
pub fn unset_starting(chart: &Chart, dark: bool) -> Chart {
    let mut next = chart.clone();
    next.options.theme = if dark {
        super::Theme::Dark
    } else {
        super::Theme::Light
    };
    let colours = palette(chart.options.palette, dark);
    let at = |i: usize| Some(literal(colours[i % colours.len()]));
    for s in &mut next.series {
        if let Some(n) = chart.mapping.series.iter().position(|c| *c == s.column)
            && s.fill == at(n)
        {
            s.fill = None;
        }
    }
    for (name, i) in mark_places(&next).into_iter().flatten() {
        next.nodes.retain(|s| !(s.name == name && s.fill == at(i)));
    }
    let c = super::chrome(dark);
    let solid = |c| Some(BrushSource::Literal(crate::Brush::Solid(c)));
    let o = &mut next.options;
    for (fill, was) in [
        (&mut o.text.ink, c.ink),
        (&mut o.value_axis.colour, c.axis),
        (&mut o.category_axis.colour, c.axis),
        (&mut o.value_axis.grid.colour, c.grid),
        (&mut o.category_axis.grid.colour, c.grid),
    ] {
        if *fill == solid(was) {
            *fill = None;
        }
    }
    next
}

/// `chart` on *From variables*: its palette `vars` in order
/// (`Options::palette_group`, at most [`super::MAX_PALETTE_GROUP`]) and every
/// series and named mark back on it, as a palette pick puts them, so a mode
/// switch recolours the chart and a series a later edit adds takes the group's
/// next colour.
pub fn with_variables(chart: &Chart, vars: &[crate::VarId]) -> Chart {
    if vars.is_empty() {
        return chart.clone();
    }
    let mut next = chart.clone();
    next.options.palette_group = vars
        .iter()
        .take(super::MAX_PALETTE_GROUP)
        .map(|var| BrushSource::Bound {
            var: *var,
            alpha: 1.0,
        })
        .collect();
    unset_marks(&mut next);
    next
}

/// The variables `chart`'s series (its [`named_marks`], where it colours by
/// name) are bound to in order, when every one is bound; what *From variables*
/// lights against.
pub fn bound_variables(chart: &Chart) -> Option<Vec<crate::VarId>> {
    let fills: Vec<BrushSource> = match named_marks(chart) {
        Some(marks) => marks.into_iter().map(|(_, f)| f).collect(),
        None => chart
            .mapping
            .series
            .iter()
            .filter_map(|c| series_fill(chart, *c))
            .collect(),
    };
    let vars: Option<Vec<crate::VarId>> = fills.iter().map(|f| f.var()).collect();
    vars.filter(|v| !v.is_empty())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::chart::ChartKind;

    /// A series with no style reads the palette at its place; a written one reads
    /// back; a palette pick puts it back on the palette.
    #[test]
    fn a_series_colour_reads_its_style_else_the_palette() {
        let chart = Chart::sample(ChartKind::Column);
        let second = chart.mapping.series[1];
        let colours = palette(chart.options.palette, false);
        assert_eq!(series_fill(&chart, second), Some(literal(colours[1])));
        let red = literal([255, 0, 0]);
        let painted = with_series_fill(&chart, second, Some(red.clone()));
        assert_eq!(series_fill(&painted, second), Some(red));
        let warm = with_palette(&painted, super::super::Palette::Warm);
        assert_eq!(
            series_fill(&warm, second),
            Some(literal(palette(super::super::Palette::Warm, false)[1]))
        );
    }

    /// An unset series follows the theme drawn, *Auto* reading the ground; a
    /// picked colour stays whatever the ground.
    /// flip: `series_fill` reading the light palette always → fails at "a
    /// palette series follows".
    #[test]
    fn an_unset_series_follows_the_ground_and_a_picked_one_stays() {
        let chart = Chart::sample(ChartKind::Column);
        let [first, second, ..] = chart.mapping.series[..] else {
            panic!("the sample maps three series");
        };
        let red = literal([255, 0, 0]);
        let chart = with_series_fill(&chart, second, Some(red.clone()));
        let navy = peniko::Color::from_rgb8(0x15, 0x22, 0x3B);
        let night = chart.on(navy);
        assert!(night.drawn_dark(), "Auto reads navy as dark");
        let dark = palette(chart.options.palette, true);
        assert_eq!(
            series_fill(&night, first),
            Some(literal(dark[0])),
            "a palette series follows"
        );
        assert_eq!(series_fill(&night, second), Some(red), "a picked one stays");
        let cream = peniko::Color::from_rgb8(0xF4, 0xEE, 0xE2);
        assert!(!chart.on(cream).drawn_dark(), "Auto reads cream as light");
    }

    /// A file from before derived colours keeps its look: its theme is what
    /// `dark` said, a colour at its theme's start is unset and a picked one
    /// stays. flip: `unset_starting` numbering a pie's marks by their order
    /// among the drawn → fails at "the slice after an undrawn row unsets".
    #[test]
    fn an_old_chart_unsets_what_is_at_its_themes_start() {
        use crate::chart::Theme;
        let mut chart = Chart::sample(ChartKind::Pie);
        chart.table.rows = [["A", "10"], ["B", "0"], ["C", "5"]]
            .iter()
            .map(|r| [r[0], r[1], "1", "1"].map(String::from).to_vec())
            .collect();
        let night = palette(chart.options.palette, true);
        let red = literal([255, 0, 0]);
        let mut old = with_node_fill(&chart, "C", Some(literal(night[2])));
        old = with_node_fill(&old, "A", Some(red.clone()));
        let ink = BrushSource::Literal(crate::Brush::Solid(crate::chart::chrome(true).ink));
        old.options.text.ink = Some(ink);
        let read = unset_starting(&old, true);
        assert_eq!(read.options.theme, Theme::Dark);
        assert_eq!(read.options.text.ink, None, "the ink unsets");
        assert!(
            !read.nodes.iter().any(|s| s.name == "C"),
            "the slice after an undrawn row unsets"
        );
        assert_eq!(named_marks(&read).unwrap()[0].1, red, "a picked one stays");
    }

    /// A Sankey's nodes come in the order the rows first name them.
    #[test]
    fn sankey_nodes_are_in_first_seen_order() {
        let chart = Chart::sample(ChartKind::Sankey);
        let names: Vec<String> = sankey_nodes(&chart).into_iter().map(|(n, _)| n).collect();
        assert_eq!(
            names,
            [
                "Search", "Signup", "Social", "Direct", "Bounce", "Trial", "Leave"
            ]
        );
    }

    /// *From variables* binds the series in order, round again past the last,
    /// and reads back as those variables; a palette pick unbinds them.
    #[test]
    fn from_variables_binds_the_series_in_order() {
        let chart = Chart::sample(ChartKind::Column);
        let mut ids = crate::IdSource::new(1);
        let vars = [ids.mint_var(), ids.mint_var()];
        let bound = with_variables(&chart, &vars);
        assert_eq!(
            bound_variables(&bound),
            Some(vec![vars[0], vars[1], vars[0]])
        );
        let back = with_palette(&bound, super::super::Palette::Mastik);
        assert_eq!(bound_variables(&back), None);
    }

    /// A node's total is the larger of what flows in and what flows out.
    #[test]
    fn a_sankey_nodes_total_is_its_larger_side() {
        let mut chart = Chart::sample(ChartKind::Sankey);
        chart.table.rows = [["A", "B", "10"], ["B", "C", "4"], ["A", "C", "1,000"]]
            .iter()
            .map(|r| r.iter().map(|c| c.to_string()).collect())
            .collect();
        assert_eq!(sankey_totals(&chart), vec![1010.0, 10.0, 1004.0]);
    }
}
