//! Chart styles: a chart's look as a sixth style slot — its options less what
//! says something about its data (`chart::Options::look`) — written onto every
//! chart bound to it by the commit, as the other slots' values are.

use mastik_core::chart::{Chart, ChartKind, Stacking, Theme};
use mastik_core::kurbo::{RoundedRectRadii, Size};
use mastik_core::style::{StyleId, StyleRule, StyleSlot, StyleValue};
use mastik_core::{
    BrushSource, Cell, Document, IdSource, Mode, NodeId, NodeKind, OpError, Operation, Transaction,
    VarId, VarKind, Variable, io,
};

struct F {
    doc: Document,
    ids: IdSource,
    /// A stacked column chart titled *Revenue in $k*, its first series red.
    a: NodeId,
    /// A line chart titled *Visits*, its value axis at 0 to 50.
    b: NodeId,
    /// A frame with no chart.
    plain: NodeId,
    /// A colour variable, `brand/1`.
    brand: VarId,
}

fn frame(id: NodeId, parent: NodeId, index: usize) -> Operation {
    Operation::CreateNode {
        id,
        parent,
        index,
        kind: NodeKind::Artboard {
            size: Size::new(320.0, 200.0),
            corner_radii: RoundedRectRadii::default(),
            corner_smoothing: 0.0,
        },
        transform: None,
        name: None,
    }
}

fn red() -> BrushSource {
    mastik_core::Brush::Solid(mastik_core::peniko::Color::from_rgb8(255, 0, 0)).into()
}

fn fixture() -> F {
    let mut ids = IdSource::new(0xC5);
    let root = ids.mint();
    let mut doc = Document::new(root);
    let (a, b, plain, mode, brand) = (
        ids.mint(),
        ids.mint(),
        ids.mint(),
        ids.mint_mode(),
        ids.mint_var(),
    );
    let mut ca = Chart::sample(ChartKind::Column);
    ca.options.bars.stacking = Stacking::Stacked;
    ca.options.value_axis.title = "Revenue in $k".into();
    ca = mastik_core::chart::with_series_fill(&ca, ca.mapping.series[0], Some(red()));
    let mut cb = Chart::sample(ChartKind::Line);
    cb.options.value_axis.title = "Visits".into();
    cb.options.value_axis.range = Some((0.0, 50.0));
    doc.apply(&Transaction(vec![
        frame(a, root, 0),
        frame(b, root, 1),
        frame(plain, root, 2),
        Operation::SetChart {
            id: a,
            chart: Some(Box::new(ca)),
        },
        Operation::SetChart {
            id: b,
            chart: Some(Box::new(cb)),
        },
        Operation::AddMode {
            collection: mastik_core::variable::CollectionId::FIRST,
            mode: Mode {
                id: mode,
                name: "Mode 1".into(),
            },
            index: 0,
            cells: vec![],
        },
        Operation::AddVariable {
            var: Variable {
                id: brand,
                name: "brand/1".into(),
                kind: VarKind::Color,
                cells: [(
                    mode,
                    Cell::Color(mastik_core::peniko::Color::from_rgb8(0x12, 0x34, 0x56)),
                )]
                .into(),
                scope: None,
                origin: None,
                forked: None,
                description: String::new(),
            },
        },
    ]))
    .expect("two charts, a plain frame and a colour variable");
    F {
        doc,
        ids,
        a,
        b,
        plain,
        brand,
    }
}

/// Commit as the app does (`tests/styles.rs`' `commit`).
fn commit(f: &mut F, ops: Vec<Operation>) -> Result<Transaction, OpError> {
    let mut tx = Transaction(ops);
    mastik_core::propagate::owed(&f.doc, &mut tx, &mut f.ids);
    f.doc.apply(&tx).map(|o| o.inverse)
}

fn chart(doc: &Document, id: NodeId) -> &Chart {
    doc.get(id).unwrap().chart().unwrap()
}

/// `a`'s look given a 9px bar radius, a fade and the Dark theme, made a style
/// named *Brand/Dashboard*, `a` and `b` bound to it.
fn styled(f: &mut F) -> StyleId {
    let mut next = chart(&f.doc, f.a).clone();
    next.options.bars.radius = 9.0;
    next.options.fade.on = true;
    next.options.theme = Theme::Dark;
    let a = f.a;
    commit(
        f,
        vec![Operation::SetChart {
            id: a,
            chart: Some(Box::new(next)),
        }],
    )
    .unwrap();
    let s = f.ids.mint_style();
    let b = f.b;
    let ops =
        mastik_core::style::create_ops(&f.doc, s, "Brand/Dashboard", a, StyleSlot::Chart, &[a, b])
            .expect("a is a chart");
    commit(f, ops).unwrap();
    s
}

/// **A chart style carries the look and never what the chart says**: `b`, a line
/// chart, bound to `a`'s style takes the radius, the fade and the theme, and keeps
/// its kind, its own axis title and its typed range; `a` keeps its stacking and
/// its red series.
///
/// Flip, run: `Options::take_own` leaving out the axis title → fails at "b keeps
/// its title".
#[test]
fn a_chart_style_carries_the_look_and_not_the_data() {
    let mut f = fixture();
    let s = styled(&mut f);
    for id in [f.a, f.b] {
        assert_eq!(f.doc.get(id).unwrap().styles().chart, Some(s));
    }
    let b = chart(&f.doc, f.b);
    assert_eq!(b.options.bars.radius, 9.0, "b takes the radius");
    assert!(b.options.fade.on, "b takes the fade");
    assert_eq!(b.options.theme, Theme::Dark, "b takes the theme");
    assert_eq!(b.kind, ChartKind::Line, "b keeps its kind");
    assert_eq!(b.options.value_axis.title, "Visits", "b keeps its title");
    assert_eq!(
        b.options.value_axis.range,
        Some((0.0, 50.0)),
        "b keeps its range"
    );
    let a = chart(&f.doc, f.a);
    assert_eq!(
        a.options.bars.stacking,
        Stacking::Stacked,
        "a keeps its stacking"
    );
    assert_eq!(
        mastik_core::chart::series_fill(a, a.mapping.series[0]),
        Some(red()),
        "a keeps its red series"
    );
    let StyleValue::Chart(look) = &f.doc.styles().get(s).unwrap().value else {
        unreachable!("a chart style")
    };
    assert!(look.value_axis.title.is_empty(), "the style holds no title");
    assert_eq!(look.bars.stacking, Stacking::Apart, "nor a stacking");
}

/// **The style's edit reaches every chart, a data edit keeps the binding, and a
/// look edit through a door detaches**: the style's radius set to 2 rewrites both
/// charts; `b`'s table written changes no look and stays bound; `b`'s legend moved
/// by a door detaches `b` and keeps the move, while `a` stays bound.
///
/// Flip, run: `style::writes_slot` answering `SetChart` as no slot's → fails at
/// "a look edit detaches".
#[test]
fn a_style_edit_reaches_its_charts_and_a_look_edit_detaches() {
    let mut f = fixture();
    let s = styled(&mut f);
    let StyleValue::Chart(mut look) = f.doc.styles().get(s).unwrap().value.clone() else {
        unreachable!("a chart style")
    };
    look.bars.radius = 2.0;
    commit(
        &mut f,
        vec![Operation::SetStyleValue {
            id: s,
            value: StyleValue::Chart(look),
        }],
    )
    .unwrap();
    for id in [f.a, f.b] {
        assert_eq!(chart(&f.doc, id).options.bars.radius, 2.0, "{id:?} follows");
    }
    let b = f.b;
    let mut data = chart(&f.doc, b).clone();
    data.table.rows[0][1] = "99".into();
    commit(
        &mut f,
        vec![Operation::SetChart {
            id: b,
            chart: Some(Box::new(data)),
        }],
    )
    .unwrap();
    assert_eq!(
        f.doc.get(b).unwrap().styles().chart,
        Some(s),
        "a data edit keeps it"
    );
    let mut moved = chart(&f.doc, b).clone();
    moved.options.legend.position = mastik_core::chart::LegendPosition::Bottom;
    commit(
        &mut f,
        vec![Operation::SetChart {
            id: b,
            chart: Some(Box::new(moved)),
        }],
    )
    .unwrap();
    assert_eq!(
        f.doc.get(b).unwrap().styles().chart,
        None,
        "a look edit detaches"
    );
    assert_eq!(
        chart(&f.doc, b).options.legend.position,
        mastik_core::chart::LegendPosition::Bottom,
        "and keeps the move"
    );
    assert_eq!(
        f.doc.get(f.a).unwrap().styles().chart,
        Some(s),
        "a stays bound"
    );
}

/// **Flatten unbinds the chart style in the same transaction**: a `SetChart`
/// to `None` on a styled chart commits, the binding gone with the chart — no
/// door has to know the slot exists.
///
/// Flip, run: `style::settle` skipping a chart slot on a layer with no chart
/// (as it skips the others) → the commit is refused, failing at "flatten
/// commits".
#[test]
fn flatten_unbinds_the_chart_style() {
    let mut f = fixture();
    styled(&mut f);
    let a = f.a;
    commit(&mut f, vec![Operation::SetChart { id: a, chart: None }]).expect("flatten commits");
    assert_eq!(f.doc.get(a).unwrap().styles().chart, None);
}

/// **Only a chart binds a chart style, and a style holds only options a chart
/// may**: binding the plain frame is refused, as is a style whose fade runs to
/// 200%.
#[test]
fn a_chart_style_binds_charts_only_and_holds_valid_options() {
    let mut f = fixture();
    let s = styled(&mut f);
    let plain = f.plain;
    let err = commit(
        &mut f,
        vec![mastik_core::style::bind_op(
            plain,
            StyleSlot::Chart,
            Some(s),
        )],
    );
    assert!(
        matches!(err, Err(OpError::Styles(StyleRule::BadBinding(n))) if n == plain),
        "{err:?}"
    );
    let mut bad = chart(&f.doc, f.a).options.look();
    bad.fade.to = 200.0;
    let id = f.ids.mint_style();
    let err = commit(
        &mut f,
        vec![mastik_core::style::add_op(
            id,
            "Bad",
            StyleValue::Chart(Box::new(bad)),
        )],
    );
    assert!(
        matches!(err, Err(OpError::Styles(StyleRule::BadChart(x))) if x == id),
        "{err:?}"
    );
}

/// **A style's *From variables* palette moves the unset series and leaves a
/// picked one**: `a`'s style put on `brand/1`'s group reaches `b`, whose series
/// all draw bound to it; `a`'s red series stays red, its second series bound.
///
/// Flip, run: `colours::place_fill` reading the palette alone → fails at "b's
/// series take the group".
#[test]
fn a_styles_variable_palette_moves_the_unset_series_only() {
    let mut f = fixture();
    let s = styled(&mut f);
    let StyleValue::Chart(mut look) = f.doc.styles().get(s).unwrap().value.clone() else {
        unreachable!("a chart style")
    };
    look.palette_group = vec![BrushSource::Bound {
        var: f.brand,
        alpha: 1.0,
    }];
    commit(
        &mut f,
        vec![Operation::SetStyleValue {
            id: s,
            value: StyleValue::Chart(look),
        }],
    )
    .unwrap();
    let bound = BrushSource::Bound {
        var: f.brand,
        alpha: 1.0,
    };
    let b = chart(&f.doc, f.b);
    for col in &b.mapping.series {
        assert_eq!(
            mastik_core::chart::series_fill(b, *col),
            Some(bound.clone()),
            "b's series take the group"
        );
    }
    let a = chart(&f.doc, f.a);
    assert_eq!(
        mastik_core::chart::series_fill(a, a.mapping.series[0]),
        Some(red())
    );
    assert_eq!(
        mastik_core::chart::series_fill(a, a.mapping.series[1]),
        Some(bound)
    );
}

/// **The style's edit door**: under an edit of the style, a look write on one
/// chart becomes the style's new value, which the commit writes onto the other;
/// a data write under the same door makes no style edit.
#[test]
fn edit_through_turns_a_look_write_into_the_styles() {
    let mut f = fixture();
    let s = styled(&mut f);
    let a = f.a;
    let mut data = chart(&f.doc, a).clone();
    data.table.rows[0][1] = "7".into();
    let tx = mastik_core::style::edit_through(
        &f.doc,
        Transaction(vec![Operation::SetChart {
            id: a,
            chart: Some(Box::new(data)),
        }]),
        s,
    );
    assert!(
        !tx.0
            .iter()
            .any(|op| matches!(op, Operation::SetStyleValue { .. })),
        "a data write is no style edit"
    );
    let mut wide = chart(&f.doc, a).clone();
    wide.options.lines.width = 5.0;
    let tx = mastik_core::style::edit_through(
        &f.doc,
        Transaction(vec![Operation::SetChart {
            id: a,
            chart: Some(Box::new(wide)),
        }]),
        s,
    );
    commit(&mut f, tx.0).unwrap();
    assert_eq!(
        f.doc.get(f.b).unwrap().styles().chart,
        Some(s),
        "b stays bound"
    );
    assert_eq!(
        chart(&f.doc, f.b).options.lines.width,
        5.0,
        "b follows the edit"
    );
}

/// **A chart style survives the save format**, its binding with it.
#[test]
fn a_chart_style_round_trips_through_a_save() {
    let mut f = fixture();
    let s = styled(&mut f);
    let back = io::load(&io::save(&f.doc).unwrap()).unwrap();
    assert_eq!(back.styles().get(s), f.doc.styles().get(s));
    assert_eq!(back.get(f.b).unwrap().styles().chart, Some(s));
}
