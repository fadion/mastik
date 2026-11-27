use super::*;
use crate::tools::Tool;
use mastik_core::edit::PenAnchor;
use mastik_core::{Document, IdSource, NodeId, NodeKind, Operation, Transaction};

/// A cubic quarter circle's handle length, over its radius.
const K: f64 = 0.552_284_749_8;

fn anchor(at: (f64, f64), back: (f64, f64), out: (f64, f64)) -> PenAnchor {
    PenAnchor {
        at: Point::new(at.0, at.1),
        back: Vec2::new(back.0, back.1),
        out: Vec2::new(out.0, out.1),
        radius: 0.0,
    }
}

/// Two quarter arcs, radius `r1` then `r2`, meeting tangentially at a smooth
/// point; the second turns the same way as the first, or the other way. The
/// middle anchor's out handle is turned by `skew` degrees.
fn two_arcs(r1: f64, r2: f64, same_way: bool, skew: f64) -> Vec<PenSubpath> {
    let (x2, back2) = match same_way {
        true => (r1 - r2, (K * r2, 0.0)),
        false => (r1 + r2, (-K * r2, 0.0)),
    };
    let (sin, cos) = skew.to_radians().sin_cos();
    let out = Vec2::new(-sin * K * r2, cos * K * r2);
    vec![PenSubpath {
        anchors: vec![
            anchor((0.0, 0.0), (0.0, 0.0), (K * r1, 0.0)),
            anchor((r1, r1), (0.0, -K * r1), (out.x, out.y)),
            anchor((x2, r1 + r2), back2, (0.0, 0.0)),
        ],
        closed: false,
    }]
}

/// A circle of four quarter arcs, radius `r`.
fn circle(r: f64) -> Vec<PenSubpath> {
    let h = K * r;
    vec![PenSubpath {
        anchors: vec![
            anchor((r, 0.0), (0.0, -h), (0.0, h)),
            anchor((0.0, r), (h, 0.0), (-h, 0.0)),
            anchor((-r, 0.0), (0.0, h), (0.0, -h)),
            anchor((0.0, -r), (-h, 0.0), (h, 0.0)),
        ],
        closed: true,
    }]
}

/// A circle's comb flags no join, its teeth stand on one side, about 1/r long,
/// and the closing segment is combed too. flip: `comb` counting an open
/// subpath's segments for a closed one → fails at "four segments".
#[test]
fn a_circle_is_smooth_everywhere() {
    let c = comb(&circle(50.0), 8);
    assert_eq!(c.segments.len(), 4, "four segments");
    assert!(c.joins.is_empty(), "no rough join: {:?}", c.joins);
    for seg in &c.segments {
        for t in seg.teeth.iter().flatten() {
            assert!(!t.kink);
            let k = t.k.hypot();
            assert!((k * 50.0 - 1.0).abs() < 0.05, "|κ|·r = {}", k * 50.0);
            // Away from the centre, which is the origin.
            assert!(t.k.dot(t.at.to_vec2()) > 0.0, "points outward");
        }
    }
    assert!((c.peak * 50.0 - 1.0).abs() < 0.05);
}

/// A step in radius grades by how far the curvature jumps: 1.1× passes, 1.3×
/// is amber, 3× red; a turn the other way flips. flip: `judge` dividing by the
/// smaller side → fails at the amber step's value.
#[test]
fn joins_grade_by_the_curvature_step() {
    assert!(comb(&two_arcs(40.0, 44.0, true, 0.0), 8).joins.is_empty());

    let amber = comb(&two_arcs(40.0, 52.0, true, 0.0), 8);
    let [j] = amber.joins[..] else {
        panic!("one join: {:?}", amber.joins)
    };
    assert_eq!(j.at, (0, 1));
    let Fault::Jump(step) = j.fault else {
        panic!("{:?}", j.fault)
    };
    assert!((step - (40.0 / 52.0 - 1.0)).abs() < 1e-6, "{step}");
    assert!(!j.fault.danger(), "amber");
    assert_eq!(j.fault.label(), "Curvature -23%");

    let red = comb(&two_arcs(40.0, 120.0, true, 0.0), 8);
    assert!(red.joins[0].fault.danger(), "red");

    let flip = comb(&two_arcs(40.0, 40.0, false, 0.0), 8);
    assert_eq!(flip.joins[0].fault, Fault::Flips);
}

/// A tangent 2° off line is *Not quite smooth*; 30° off is a pair broken on
/// purpose and is not judged; a corner never is. flip: `judge` without its
/// broken-pair bound → fails at "broken on purpose".
#[test]
fn a_tangent_off_line_is_not_quite_smooth() {
    let off = comb(&two_arcs(40.0, 40.0, true, 2.0), 8);
    assert_eq!(off.joins[0].fault, Fault::NotSmooth);
    assert!(
        comb(&two_arcs(40.0, 40.0, true, 30.0), 8).joins.is_empty(),
        "broken on purpose"
    );
    let mut corner = two_arcs(40.0, 120.0, true, 0.0);
    corner[0].anchors[1].out = Vec2::ZERO;
    corner[0].anchors[1].back = Vec2::ZERO;
    assert!(comb(&corner, 8).joins.is_empty(), "a corner");
}

/// A line running into an arc at a smooth point jumps from no curvature to
/// some: red, growing along the path.
#[test]
fn a_line_into_an_arc_jumps() {
    let path = vec![PenSubpath {
        anchors: vec![
            anchor((-40.0, 0.0), (0.0, 0.0), (0.0, 0.0)),
            anchor((0.0, 0.0), (0.0, 0.0), (K * 40.0, 0.0)),
            anchor((40.0, 40.0), (0.0, -K * 40.0), (0.0, 0.0)),
        ],
        closed: false,
    }];
    let c = comb(&path, 8);
    assert_eq!(c.joins.len(), 1);
    assert_eq!(c.joins[0].fault, Fault::Jump(1.0));
    assert_eq!(c.joins[0].fault.label(), "Curvature +100%");
    assert!(
        c.segments[0]
            .teeth
            .iter()
            .flatten()
            .all(|t| t.k == Vec2::ZERO)
    );
}

/// A segment folding back on itself has kinks beside the fold, kept out of
/// Fitted's peak; its exact cusp has no tooth. flip: `comb` letting kinks set
/// `peak` → fails at "the peak ignores the kinks".
#[test]
fn a_fold_is_a_kink_and_does_not_set_the_fit() {
    let path = vec![PenSubpath {
        anchors: vec![
            anchor((0.0, 0.0), (0.0, 0.0), (100.0, 100.0)),
            anchor((100.0, 0.0), (-100.0, 100.0), (0.0, 0.0)),
        ],
        closed: false,
    }];
    let c = comb(&path, 24);
    let teeth = &c.segments[0].teeth;
    assert!(teeth[12].is_none(), "the cusp itself");
    assert!(teeth[11].is_some_and(|t| t.kink) && teeth[13].is_some_and(|t| t.kink));
    assert!(
        teeth[0].is_some_and(|t| !t.kink),
        "the ends are never kinks"
    );
    let shortest_kink = teeth
        .iter()
        .flatten()
        .filter(|t| t.kink)
        .map(|t| t.k.hypot())
        .fold(f64::INFINITY, f64::min);
    assert!(c.peak < shortest_kink, "the peak ignores the kinks");
    assert!(
        c.fitted(1.0)
            .is_some_and(|s| (s * c.peak - FIT_SHARE * c.size).abs() < 1e-9)
    );
}

/// Fitted's longest tooth is a share of the path, so it zooms with the path,
/// until it reaches its screen cap. flip: `Comb::fitted` sizing to `FIT_PT` on
/// screen at every zoom → fails at "a tenth of the path when zoomed out".
#[test]
fn fitted_teeth_zoom_with_the_path_up_to_a_cap() {
    let c = comb(&circle(50.0), 8);
    // The longest tooth's length on screen at `ppw` screen pt per world unit.
    let longest = |ppw: f64| c.fitted(ppw).expect("curved") * c.peak * ppw;
    for ppw in [0.05, 0.1, 0.5] {
        let path_on_screen = c.size * ppw;
        assert!(
            (longest(ppw) - FIT_SHARE * path_on_screen).abs() < 1e-9,
            "a tenth of the path when zoomed out, at {ppw}"
        );
    }
    assert!(
        (longest(20.0) - FIT_PT).abs() < 1e-9,
        "capped when zoomed in"
    );
}

/// While the pen draws, the card shows and the run in hand is combed, its
/// unclosed end included. flip: `comb_subpaths` leaving out the pen's run → fails
/// at "the run is combed".
#[test]
fn the_pen_run_is_combed_while_drawing() {
    let ctx = egui::Context::default();
    let mut app = MastikApp::headless(&ctx);
    app.tool = Tool::Pen;
    app.prefs.combs.on = true;
    assert!(app.comb_card_shown(), "the card shows under the pen");
    assert!(app.edited_comb().is_none(), "no run, nothing to comb");
    let root = app.session.doc.first_page();
    let arcs = two_arcs(40.0, 120.0, true, 0.0);
    app.pen = Some(crate::preview::PenState {
        parent: root,
        parent_world: mastik_core::kurbo::Affine::IDENTITY,
        anchors: arcs[0].anchors.clone(),
        shaping: false,
        editing: None,
    });
    let c = app.edited_comb().expect("the run is combed");
    assert_eq!(c.segments.len(), 2, "both segments of the run");
    assert_eq!(c.joins.len(), 1, "its rough join is flagged");
}

/// An app editing `subpaths` under the node tool.
fn app_editing(ctx: &egui::Context, subpaths: &[PenSubpath]) -> (MastikApp, NodeId) {
    let mut app = MastikApp::headless(ctx);
    let mut ids = IdSource::new(7);
    let root = ids.mint();
    let mut doc = Document::new(root);
    let id = ids.mint();
    doc.apply(&Transaction(vec![Operation::CreateNode {
        id,
        parent: root,
        index: 0,
        kind: NodeKind::Path {
            path: mastik_core::edit::pen_path(subpaths),
            corner_radii: mastik_core::edit::pen_radii(subpaths),
            corner_smoothing: 0.0,
            mirror: mastik_core::symmetry::PathMirror::NONE,
        },
        transform: None,
        name: None,
    }]))
    .expect("build the fixture");
    app.session.adopt_document(doc, None);
    app.session.selection.set_one(id);
    app.tool = Tool::Node;
    assert_eq!(app.edited_path(), Some(id), "fixture: the path is edited");
    (app, id)
}

/// Three arcs with two rough joins.
fn two_rough_joins() -> Vec<PenSubpath> {
    let mut path = two_arcs(40.0, 120.0, true, 0.0);
    let last = path[0].anchors[2];
    // Carry on from the end, heading -x, into a 3× tighter arc the same way.
    path[0].anchors[2].out = Vec2::new(-K * 40.0, 0.0);
    path[0].anchors.push(anchor(
        (last.at.x - 40.0, last.at.y - 40.0),
        (0.0, K * 40.0),
        (0.0, 0.0),
    ));
    path
}

/// *Next* picks each flagged join in path order and wraps round. flip:
/// `next_rough_join` starting from the first join always → fails at "the second".
#[test]
fn next_steps_through_the_rough_joins() {
    let ctx = egui::Context::default();
    let (mut app, id) = app_editing(&ctx, &two_rough_joins());
    app.prefs.combs.on = true;
    let joins: Vec<PointRef> = app
        .edited_comb()
        .expect("a comb")
        .joins
        .iter()
        .map(|j| j.at)
        .collect();
    assert_eq!(joins, vec![(0, 1), (0, 2)], "fixture: two rough joins");
    let picked = |app: &MastikApp| app.points.on(id).collect::<Vec<_>>();
    app.next_rough_join();
    assert_eq!(picked(&app), vec![(0, 1)], "the first");
    app.next_rough_join();
    assert_eq!(picked(&app), vec![(0, 2)], "the second");
    app.next_rough_join();
    assert_eq!(picked(&app), vec![(0, 1)], "round again");
}

/// `Alt+C`'s switch saves the preference and shows the card; Escape hides the
/// card and leaves combs on; the switch turns them off. flip: the Escape rung
/// turning combs off → fails at "still on".
#[test]
fn the_switch_shows_the_card_and_escape_only_hides_it() {
    let ctx = egui::Context::default();
    let (mut app, _) = app_editing(&ctx, &two_rough_joins());
    app.combs.card = false;
    app.toggle_combs();
    assert!(app.prefs.combs.on && app.view_switch(ViewSwitch::Combs));
    assert!(app.comb_card_shown(), "the card shows");
    app.escape(&ctx);
    assert!(!app.comb_card_shown(), "hidden");
    assert!(app.combs_on(), "still on");
    app.toggle_combs();
    assert!(!app.combs_on());
}

/// The comb draws while points are edited and combs are on, and nothing
/// otherwise; the rough joins draw their rings in their inks. flip:
/// `draw_combs` without its `combs_on` gate → fails at "nothing while off".
#[test]
fn the_comb_draws_only_while_on() {
    let ctx = egui::Context::default();
    let (mut app, _) = app_editing(&ctx, &two_rough_joins());
    let rect = egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(800.0, 600.0));
    app.canvas_px = (800, 600);
    app.session.camera.center = Point::new(-40.0, 80.0);
    app.session.camera.zoom = 2.0;
    let shapes = |app: &MastikApp| {
        ctx.run_ui(Default::default(), |ui| {
            app.draw_combs(ui.painter(), rect, 1.0)
        })
        .shapes
        .into_iter()
        .map(|c| c.shape)
        .collect::<Vec<_>>()
    };
    assert!(shapes(&app).is_empty(), "nothing while off");
    app.prefs.combs.on = true;
    let drawn = shapes(&app);
    let teeth = drawn
        .iter()
        .filter(|s| matches!(s, egui::Shape::LineSegment { stroke, .. } if stroke.color == color::SELECT_DIM))
        .count();
    assert!(teeth > 20, "teeth: {teeth}");
    let rings: Vec<_> = drawn
        .iter()
        .filter_map(|s| match s {
            egui::Shape::Circle(c) => Some(c.stroke.color),
            _ => None,
        })
        .collect();
    assert_eq!(
        rings,
        vec![color::danger(), color::danger()],
        "two red rings"
    );
}

/// A path of straight lines at awkward angles has no curvature, so Fitted has
/// nothing to scale and nothing draws. flip: `comb` reading a line's curvature
/// as computed → fails at "no curvature".
#[test]
fn a_path_of_lines_draws_no_comb() {
    let corners = [
        (12.37, 81.9),
        (97.13, 3.71),
        (143.9, 120.33),
        (61.03, 177.7),
        (20.21, 140.09),
    ];
    let path = vec![PenSubpath {
        anchors: corners
            .iter()
            .map(|&at| anchor(at, (0.0, 0.0), (0.0, 0.0)))
            .collect(),
        closed: true,
    }];
    let c = comb(&path, 64);
    assert_eq!(c.peak, 0.0, "no curvature");
    assert_eq!(c.fitted(1.0), None);
    assert!(c.joins.is_empty());

    let ctx = egui::Context::default();
    let (mut app, _) = app_editing(&ctx, &path);
    app.prefs.combs.on = true;
    app.canvas_px = (800, 600);
    app.session.camera.center = Point::new(80.0, 90.0);
    let rect = egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(800.0, 600.0));
    let drawn = ctx
        .run_ui(Default::default(), |ui| {
            app.draw_combs(ui.painter(), rect, 1.0)
        })
        .shapes;
    assert!(drawn.is_empty(), "nothing drawn");
}
