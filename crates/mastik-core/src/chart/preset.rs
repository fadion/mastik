//! Style presets (`charts.md`, *Options*): a set of option values written over a
//! description in one step, not a link. The Chart card's Style cells and an
//! agent's `set_chart` write the same values.

use super::{ChartKind, LegendPosition, NodeLabels, Options, PointStyle, ValuePosition};

/// The presets, in the card's order.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Preset {
    /// The defaults: solid gridlines, a legend on top, no values.
    Clean,
    /// Rounded marks and a light dashed grid.
    Soft,
    /// Heavy marks, no grid, large values; a Sankey's nodes wider and its
    /// flows stronger.
    Bold,
    /// No axes, no grid, no legend, no Sankey labels: a sparkline when the
    /// chart is small.
    Minimal,
    /// Marks stroked in their colour over a light tint of it, and thin lines.
    Outline,
}

/// The stroke the *Outline* preset gives marks and lines, and *Soft*'s
/// separators.
pub const OUTLINE_WIDTH: f64 = 1.5;

impl Preset {
    pub const ALL: [Preset; 5] = [
        Preset::Clean,
        Preset::Soft,
        Preset::Bold,
        Preset::Minimal,
        Preset::Outline,
    ];

    /// The preset's name as the UI says it.
    pub fn label(self) -> &'static str {
        match self {
            Preset::Clean => "Clean",
            Preset::Soft => "Soft",
            Preset::Bold => "Bold",
            Preset::Minimal => "Minimal",
            Preset::Outline => "Outline",
        }
    }

    /// `options` with this preset's values written over the style fields it
    /// owns; data, palette, text family and number formats are left as they are.
    /// A scatter draws no value labels, so on one their position and size are
    /// left as they are too.
    pub fn apply(self, kind: ChartKind, options: &Options) -> Options {
        let clean = Options::default();
        let mut o = options.clone();
        o.bars.radius = clean.bars.radius;
        o.bars.radius_all = clean.bars.radius_all;
        o.bars.group_gap = clean.bars.group_gap;
        o.lines.width = clean.lines.width;
        o.lines.points = clean.lines.points;
        o.slices.pad_angle = clean.slices.pad_angle;
        o.slices.radius = clean.slices.radius;
        o.flows.node_radius = clean.flows.node_radius;
        o.flows.node_width = clean.flows.node_width;
        o.flows.opacity = clean.flows.opacity;
        o.flows.labels = clean.flows.labels;
        for (axis, default) in [
            (&mut o.value_axis, &clean.value_axis),
            (&mut o.category_axis, &clean.category_axis),
        ] {
            axis.show = default.show;
            axis.grid.show = default.grid.show;
        }
        // Only *Soft* dashes, and only the value gridlines: the category
        // gridlines' dash is the user's.
        o.value_axis.grid.dashed = clean.value_axis.grid.dashed;
        o.legend.position = clean.legend.position;
        o.values.position = clean.values.position;
        o.text.value_size = clean.text.value_size;
        o.text.value.weight = clean.text.value.weight;
        o.stroke = clean.stroke.clone();
        o.values.totals = clean.values.totals;
        match self {
            Preset::Clean => {}
            Preset::Soft => {
                o.bars.radius = 6.0;
                o.bars.radius_all = true;
                o.lines.points = PointStyle::Filled;
                o.slices.pad_angle = 2.0;
                o.slices.radius = 4.0;
                o.flows.node_radius = 6.0;
                o.value_axis.grid.dashed = true;
                // Rounded marks kept apart by a line of the background.
                o.stroke.width = OUTLINE_WIDTH;
            }
            Preset::Bold => {
                o.bars.group_gap = 15.0;
                o.lines.width = 4.0;
                o.value_axis.grid.show = false;
                o.category_axis.grid.show = false;
                o.values.position = ValuePosition::Outside;
                o.values.totals = true;
                o.text.value_size = 14.0;
                o.text.value.weight = Some(600);
                o.flows.node_width = 18.0;
                o.flows.opacity = 0.6;
            }
            Preset::Minimal => {
                o.value_axis.show = false;
                o.category_axis.show = false;
                o.value_axis.grid.show = false;
                o.category_axis.grid.show = false;
                o.legend.position = LegendPosition::Off;
                o.flows.labels = NodeLabels::Off;
            }
            Preset::Outline => {
                o.stroke.style = super::StrokeStyle::Outline;
                o.stroke.width = OUTLINE_WIDTH;
                o.lines.width = OUTLINE_WIDTH;
            }
        }
        if kind == ChartKind::Scatter {
            o.values.position = options.values.position;
            o.values.totals = options.values.totals;
            o.text.value_size = options.text.value_size;
            o.text.value.weight = options.text.value.weight;
        }
        o
    }

    /// The preset a `kind` chart's `options` is exactly at, for the cell that
    /// lights.
    pub fn of(kind: ChartKind, options: &Options) -> Option<Preset> {
        Preset::ALL
            .into_iter()
            .find(|p| p.apply(kind, options) == *options)
    }

    /// The preset `options` are nearest to: the one whose application changes
    /// the fewest of their values, the first of a tie — what a chart style's
    /// summary names.
    pub fn nearest(kind: ChartKind, options: &Options) -> Preset {
        let here = leaves(options);
        Preset::ALL
            .into_iter()
            .min_by_key(|p| {
                let there = leaves(&p.apply(kind, options));
                here.iter()
                    .filter(|(k, v)| there.get(*k) != Some(*v))
                    .count()
                    + there.keys().filter(|k| !here.contains_key(*k)).count()
            })
            .expect("there are presets")
    }
}

/// Every value `options` writes, by its path.
fn leaves(options: &Options) -> std::collections::BTreeMap<String, serde_json::Value> {
    fn walk(
        at: String,
        v: serde_json::Value,
        out: &mut std::collections::BTreeMap<String, serde_json::Value>,
    ) {
        match v {
            serde_json::Value::Object(m) => {
                for (k, v) in m {
                    walk(format!("{at}/{k}"), v, out);
                }
            }
            v => {
                out.insert(at, v);
            }
        }
    }
    let mut out = Default::default();
    if let Ok(v) = serde_json::to_value(options) {
        walk(String::new(), v, &mut out);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Each preset reads back as itself after it is applied, from any other, so
    /// one cell lights; the defaults are *Clean*.
    #[test]
    fn a_preset_applied_is_the_one_that_lights() {
        for kind in [ChartKind::Column, ChartKind::Scatter] {
            assert_eq!(Preset::of(kind, &Options::default()), Some(Preset::Clean));
            for from in Preset::ALL {
                for to in Preset::ALL {
                    let o = to.apply(kind, &from.apply(kind, &Options::default()));
                    assert_eq!(
                        Preset::of(kind, &o),
                        Some(to),
                        "{kind:?}: {from:?} → {to:?}"
                    );
                }
            }
        }
        let mut custom = Options::default();
        custom.bars.radius = 9.0;
        assert_eq!(
            Preset::of(ChartKind::Column, &custom),
            None,
            "an edited option lights none"
        );
    }

    /// *Bold* on a scatter, which draws no value labels, leaves them as they
    /// are; on a column chart it puts them outside.
    /// flip: `Preset::apply` ignoring the kind → fails at "a scatter's values".
    #[test]
    fn bold_leaves_a_scatters_value_labels_alone() {
        let o = Options::default();
        let scatter = Preset::Bold.apply(ChartKind::Scatter, &o);
        assert_eq!(scatter.values, o.values, "a scatter's values");
        assert_eq!(scatter.text.value_size, o.text.value_size);
        let column = Preset::Bold.apply(ChartKind::Column, &o);
        assert_eq!(column.values.position, ValuePosition::Outside);
    }

    /// An option no preset sets survives every preset: the category
    /// gridlines' dash.
    /// flip: `Preset::apply` resetting both axes' dash → fails at "kept".
    #[test]
    fn a_preset_keeps_the_category_gridlines_dash() {
        let mut o = Options::default();
        o.category_axis.grid.dashed = true;
        for p in Preset::ALL {
            assert!(
                p.apply(ChartKind::Column, &o).category_axis.grid.dashed,
                "{p:?}: kept"
            );
        }
    }

    /// Every preset changes something a Sankey draws, so no two of its cells
    /// pick the same look.
    #[test]
    fn every_preset_changes_what_a_sankey_draws() {
        let flows = |p: Preset| {
            let o = p.apply(ChartKind::Sankey, &Options::default());
            format!("{:?}{:?}", o.flows, o.stroke)
        };
        for (i, a) in Preset::ALL.iter().enumerate() {
            for b in &Preset::ALL[i + 1..] {
                assert_ne!(flows(*a), flows(*b), "{a:?} and {b:?}");
            }
        }
    }
}
