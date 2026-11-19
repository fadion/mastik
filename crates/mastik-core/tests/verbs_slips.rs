//! The slips an agent makes writing a verb's arguments: the ones put right on the
//! way in, and the refusals that show the form to write for the rest.

use mastik_core::verbs::{self, Ctx, Done, VerbError};
use mastik_core::{Document, IdSource, NodeId, NodeKind, Resolved};
use serde_json::{Value, json};

/// A document with one page and nothing on it, the id source that made it, and the page.
fn blank() -> (Document, IdSource, NodeId) {
    let mut ids = IdSource::new(0x7E5B);
    let page = ids.mint();
    (Document::new(page), ids, page)
}

/// What `name` with `args` answers on `doc`, committing nothing.
fn answer(doc: &Document, ids: &mut IdSource, name: &str, args: &Value) -> Result<Done, VerbError> {
    let res = Resolved::rebuild(doc);
    verbs::call(
        &mut Ctx {
            doc,
            res: &res,
            ids,
            images: None,
            contrast: None,
        },
        name,
        args,
    )
}

/// Run `name` with `args` and commit it, checking its undo and redo land where
/// they should; answers what it created.
fn run(doc: &mut Document, ids: &mut IdSource, name: &str, args: Value) -> Done {
    let done = answer(doc, ids, name, &args).unwrap_or_else(|e| panic!("{name}: {e}"));
    let before = doc.clone();
    let undo = doc
        .apply(&done.tx)
        .unwrap_or_else(|e| panic!("{name} commits: {e}"))
        .inverse;
    let after = doc.clone();
    let redo = doc.apply_restoring(&undo).expect("undo").inverse;
    assert!(*doc == before, "{name}: undo restores the document");
    doc.apply_restoring(&redo).expect("redo");
    assert!(*doc == after, "{name}: redo restores the edit");
    done
}

/// Why `name` with `args` is refused, panicking when it is taken.
fn why(doc: &Document, ids: &mut IdSource, name: &str, args: Value) -> String {
    match answer(doc, ids, name, &args) {
        Err(e) => e.to_string(),
        Ok(_) => panic!("{name} took {args}"),
    }
}

/// A frame of 400 × 400 on the page, for layers to go in.
fn frame_on(doc: &mut Document, ids: &mut IdSource, page: NodeId) -> NodeId {
    run(
        doc,
        ids,
        "create_node",
        json!({ "parent": page.to_wire(), "kind": { "Artboard": { "size": { "width": 400, "height": 400 } } } }),
    )
    .created[0]
}

/// An enum word in another case or in kebab-case, and a field in camelCase, are
/// read as the model spells them.
///
/// flip: `repair` answering false for an unknown variant → fails at "align_items".
#[test]
fn an_enum_word_and_a_field_are_read_in_any_case() {
    let (mut doc, mut ids, page) = blank();
    let f = frame_on(&mut doc, &mut ids, page);
    let display = json!({ "Flex": { "direction": "column", "align_items": "center", "justify_content": "space-between" } });
    run(
        &mut doc,
        &mut ids,
        "set_display",
        json!({ "nodes": [f.to_wire()], "display": display }),
    );
    let flex = serde_json::to_value(doc.get(f).unwrap().display()).unwrap();
    assert_eq!(flex["Flex"]["align_items"], "Center", "align_items: {flex}");
    assert_eq!(flex["Flex"]["justify_content"], "SpaceBetween", "{flex}");
    let t = run(
        &mut doc,
        &mut ids,
        "create_text",
        json!({ "parent": f.to_wire(), "text": "Hi" }),
    )
    .created[0];
    run(
        &mut doc,
        &mut ids,
        "set_text_style",
        json!({ "nodes": [t.to_wire()], "style": { "fontSize": 40 } }),
    );
    let NodeKind::Text { style, .. } = doc.get(t).unwrap().kind() else {
        panic!("not a text");
    };
    assert_eq!(style.font_size, 40.0, "fontSize read as font_size");
}

/// `Frame` is taken for `Artboard`, the kind every tool's words call a frame.
#[test]
fn a_frame_is_an_artboard() {
    let (doc, mut ids, page) = blank();
    let done = answer(
        &doc,
        &mut ids,
        "create_node",
        &json!({ "parent": page.to_wire(), "kind": { "frame": { "size": { "width": 10, "height": 10 } } } }),
    );
    assert!(done.is_ok(), "{done:?}");
}

/// A kind written without its fields, or as an object of the wrong shape, is
/// refused with the form to write.
///
/// flip: `explain` answering `said` alone → fails at "the ellipse's form".
#[test]
fn a_kind_without_its_fields_is_shown_its_form() {
    let (doc, mut ids, page) = blank();
    let bare = why(
        &doc,
        &mut ids,
        "create_node",
        json!({ "parent": page.to_wire(), "kind": "Ellipse" }),
    );
    assert!(
        bare.contains(r#"{"Ellipse":{"size""#),
        "the ellipse's form: {bare}"
    );
    // Quoted again by a client that sends the parameter as JSON text.
    let quoted = why(
        &doc,
        &mut ids,
        "create_node",
        json!({ "parent": page.to_wire(), "kind": "\"Ellipse\"" }),
    );
    assert!(quoted.contains(r#"{"Ellipse":{"size""#), "{quoted}");
    let flat = why(
        &doc,
        &mut ids,
        "create_node",
        json!({ "parent": page.to_wire(), "kind": { "type": "rect", "width": 10 } }),
    );
    assert!(flat.contains("one key naming the variant"), "{flat}");
}

/// A tree child written with `at` is refused with the `transform` that places it.
///
/// flip: `explain` without its tree-placement arm → fails at "the placing form".
#[test]
fn a_tree_child_placed_with_at_is_shown_transform() {
    let (doc, mut ids, page) = blank();
    let rect = json!({ "Rect": { "size": { "width": 1, "height": 1 } } });
    let why = why(
        &doc,
        &mut ids,
        "insert_subtree",
        json!({ "parent": page.to_wire(), "tree": { "kind": rect,
            "children": [{ "at": [4, 4], "kind": rect }] } }),
    );
    assert!(
        why.contains("[1, 0, 0, 1, x, y]"),
        "the placing form: {why}"
    );
}

/// A fill written wrong inside a tree is shown the fill example, with its
/// gradient, not the whole tree's.
///
/// flip: `explain` showing the parameter's own model's example → fails at
/// "the fill example".
#[test]
fn a_tree_fill_written_wrong_is_shown_fills() {
    let (doc, mut ids, page) = blank();
    let rect = json!({ "Rect": { "size": { "width": 1, "height": 1 } } });
    let why = why(
        &doc,
        &mut ids,
        "insert_subtree",
        json!({ "parent": page.to_wire(), "tree": { "kind": rect,
            "fills": [{ "brush": { "LinearGradient": { "stops": [] } } }] } }),
    );
    assert!(
        why.contains("\"Gradient\"") && !why.contains("\"Card\""),
        "the fill example: {why}"
    );
}

/// `Hug` is read as `FitContent`, a bare number as a letter spacing's px and a
/// line height's multiple, and a kind missing a field is shown that kind.
///
/// flip: `alias` without its `hug` arm → fails at "hug"; `loosen_in` without its
/// bare-number arm → fails at "bare numbers"; `explain` without its
/// missing-field kind arm → fails at "the Path form".
#[test]
fn hug_bare_spacing_and_a_kinds_own_form_are_read() {
    let (mut doc, mut ids, page) = blank();
    let f = frame_on(&mut doc, &mut ids, page);
    let hug = answer(
        &doc,
        &mut ids,
        "set_layout_item",
        &json!({ "nodes": [f.to_wire()], "item": { "height": "Hug" } }),
    );
    assert!(hug.is_ok(), "hug: {hug:?}");
    let text = json!({ "parent": page.to_wire(), "kind": { "Text": { "content": "Hi",
        "style": { "font_size": 16, "letter_spacing": -0.4, "line_height": 1.4 } } } });
    let made = answer(&doc, &mut ids, "create_node", &text);
    assert!(made.is_ok(), "bare numbers: {made:?}");
    let refused = why(
        &doc,
        &mut ids,
        "create_node",
        json!({ "parent": page.to_wire(), "kind": { "Path": { "d": "M0 0 L10 0" } } }),
    );
    assert!(
        refused.contains("\"Path\":{\"path\""),
        "the Path form: {refused}"
    );
}

/// A text style's value is completed as a layer's style is, a flex takes
/// `align` and `justify`, a tree's rectangle `corner_radius`, and a path's
/// geometry `{"path": …}` without its variant.
///
/// flip: `loosen_in` without its style-value `text` arm → fails at "the style";
/// its `Flex` arm without `align` → fails at "the tree"; the corner move
/// matching `Artboard` alone → fails at "a frame written `Frame`".
#[test]
fn styles_flex_corners_and_paths_take_the_names_agents_write() {
    let (mut doc, mut ids, page) = blank();
    let style = json!({ "slot": "text", "name": "Display",
        "value": { "text": { "font_family": "Inter", "font_size": 64, "line_height": "68px" } } });
    let made = answer(&doc, &mut ids, "add_style", &style);
    assert!(made.is_ok(), "the style: {made:?}");
    let rect = json!({ "Rect": { "size": { "width": 10, "height": 10 } } });
    let tree = json!({ "parent": page.to_wire(), "tree": {
        "kind": { "Artboard": { "size": { "width": 100, "height": 40 } } },
        "display": { "Flex": { "align": "Center", "justify": "SpaceBetween" } },
        "children": [{ "kind": rect, "corner_radius": 4 }] } });
    let made = answer(&doc, &mut ids, "insert_subtree", &tree);
    assert!(made.is_ok(), "the tree: {made:?}");
    let framed = json!({ "parent": page.to_wire(), "tree": {
        "kind": { "Frame": { "size": { "width": 100, "height": 40 } } }, "corner_radius": 20 } });
    let made = answer(&doc, &mut ids, "insert_subtree", &framed);
    assert!(made.is_ok(), "a frame written `Frame`: {made:?}");
    let path = run(
        &mut doc,
        &mut ids,
        "create_node",
        json!({ "parent": page.to_wire(), "kind": { "Path": { "path": "M0 0 L10 0 L10 10 Z" } } }),
    )
    .created[0];
    let patch = json!({ "node": path.to_wire(), "geometry": { "path": "M0 0 L20 0 L20 20 Z" } });
    let set = answer(&doc, &mut ids, "set_geometry", &patch);
    assert!(set.is_ok(), "the path: {set:?}");
}

/// A width written `Fill` is refused naming the size that fills, since a frame
/// is never stretched.
///
/// flip: `explain` without its fill arm → fails at "the fill's".
#[test]
fn a_fill_width_is_shown_the_size_that_fills() {
    let (doc, mut ids, page) = blank();
    let frame = json!({ "Artboard": { "size": { "width": 10, "height": 10 } } });
    let refused = why(
        &doc,
        &mut ids,
        "insert_subtree",
        json!({ "parent": page.to_wire(), "tree": { "kind": frame, "item": { "width": "Fill" } } }),
    );
    assert!(
        refused.contains("\"Percent\": 100"),
        "the fill's: {refused}"
    );
}

/// A text style written with a `sizing` is refused naming the geometry patch
/// that sets it.
///
/// flip: `explain` without its sizing arm → fails at "the sizing's".
#[test]
fn a_styles_sizing_is_shown_the_geometry() {
    let (mut doc, mut ids, page) = blank();
    let text = run(
        &mut doc,
        &mut ids,
        "create_text",
        json!({ "parent": page.to_wire(), "text": "Hi" }),
    )
    .created[0];
    let refused = why(
        &doc,
        &mut ids,
        "set_text_style",
        json!({ "nodes": [text.to_wire()], "style": { "sizing": "Auto" } }),
    );
    assert!(refused.contains("TextSizing"), "the sizing's: {refused}");
}

/// A layout item written with a `position`, in a tree or alone, is refused
/// with the insets that take a child out of its frame's flow.
///
/// flip: `explain` without its `LayoutItem` position arm → fails at "the item's".
#[test]
fn an_items_position_is_shown_insets() {
    let (mut doc, mut ids, page) = blank();
    let rect = json!({ "Rect": { "size": { "width": 1, "height": 1 } } });
    let in_tree = why(
        &doc,
        &mut ids,
        "insert_subtree",
        json!({ "parent": page.to_wire(), "tree": { "kind": rect,
            "item": { "position": "Absolute" } } }),
    );
    assert!(in_tree.contains("`insets`"), "the tree's: {in_tree}");
    let f = frame_on(&mut doc, &mut ids, page);
    let alone = why(
        &doc,
        &mut ids,
        "set_layout_item",
        json!({ "nodes": [f.to_wire()], "item": { "position": "absolute" } }),
    );
    assert!(alone.contains("`insets`"), "the item's: {alone}");
}

/// Insets are taken as CSS writes them, in a tree and alone: a number of px,
/// `"12px"` and `"50%"`.
///
/// flip: `loosen_in` without its `insets` arm → fails at "a tree's".
#[test]
fn insets_are_taken_as_css_writes_them() {
    let (mut doc, mut ids, page) = blank();
    let f = frame_on(&mut doc, &mut ids, page);
    let rect = json!({ "Rect": { "size": { "width": 10, "height": 10 } } });
    let tree = json!({ "parent": f.to_wire(), "tree": { "kind": rect,
        "insets": { "top": 12, "right": "12px", "left": "50%" } } });
    let taken = answer(&doc, &mut ids, "insert_subtree", &tree);
    assert!(taken.is_ok(), "a tree's: {taken:?}");
    let alone = json!({ "nodes": [f.to_wire()], "insets": { "top": "8px", "bottom": 4 } });
    let taken = answer(&doc, &mut ids, "set_insets", &alone);
    assert!(taken.is_ok(), "set_insets': {taken:?}");
}

/// A parameter a client sent as JSON text is read as the JSON.
#[test]
fn a_model_parameter_sent_as_json_text_is_read() {
    let (doc, mut ids, page) = blank();
    let kind = r#"{"Rect": {"size": {"width": 10, "height": 10}}}"#;
    let done = answer(
        &doc,
        &mut ids,
        "create_node",
        &json!({ "parent": page.to_wire(), "kind": kind }),
    );
    assert!(done.is_ok(), "{done:?}");
}

/// A stroke as a bare colour or with `color`, a fill with `color` and `opacity`,
/// padding as two numbers, a text without its sizing, a star without its ratio
/// and a geometry as path data alone are all taken, and stored as they mean.
///
/// flip: `loosen`'s two-number padding read as [horizontal, vertical] → fails at "padding".
#[test]
fn the_short_forms_agents_write_are_taken() {
    let (mut doc, mut ids, page) = blank();
    let f = frame_on(&mut doc, &mut ids, page);
    let r = run(
        &mut doc,
        &mut ids,
        "create_node",
        json!({ "parent": f.to_wire(), "kind": { "Rect": { "size": { "width": 10, "height": 10 } } } }),
    )
    .created[0];
    let stored = |doc: &Document, id: NodeId| json!({ "kind": serde_json::to_value(doc.get(id).unwrap().kind()).unwrap() });
    let n = json!([r.to_wire()]);
    let dark = mastik_core::peniko::Color::from_rgb8(0x12, 0x35, 0x2a);
    run(
        &mut doc,
        &mut ids,
        "set_strokes",
        json!({ "nodes": n, "strokes": ["#12352a"] }),
    );
    let strokes = &doc.get(r).unwrap().paint().strokes;
    assert_eq!(
        strokes[0].brush,
        mastik_core::Brush::Solid(dark).into(),
        "strokes"
    );
    assert_eq!(strokes[0].width, 1.0);
    run(
        &mut doc,
        &mut ids,
        "set_strokes",
        json!({ "nodes": n, "strokes": [{ "color": "#12352a", "width": 2 }] }),
    );
    assert_eq!(doc.get(r).unwrap().paint().strokes[0].width, 2.0, "a width");
    run(
        &mut doc,
        &mut ids,
        "set_fills",
        json!({ "nodes": n, "fills": [{ "color": "#1a76ff", "opacity": 0.5 }] }),
    );
    let fill = serde_json::to_value(&doc.get(r).unwrap().paint().fills[0]).unwrap();
    let alpha = fill["brush"]["Solid"]["components"][3].as_f64().unwrap();
    assert!((alpha - 0.5).abs() < 0.01, "an opacity folded: {fill}");
    run(
        &mut doc,
        &mut ids,
        "set_display",
        json!({ "nodes": [f.to_wire()], "display": { "Flex": { "padding": [16, 24] } } }),
    );
    let flex = serde_json::to_value(doc.get(f).unwrap().display()).unwrap();
    assert_eq!(
        flex["Flex"]["padding"],
        json!([16.0, 24.0, 16.0, 24.0]),
        "padding"
    );
    run(
        &mut doc,
        &mut ids,
        "set_geometry",
        json!({ "node": r.to_wire(), "geometry": { "CornerRadius": 4 } }),
    );
    assert_eq!(
        stored(&doc, r)["kind"]["Rect"]["corner_radii"]["top_left"],
        json!(4.0),
        "a corner radius"
    );
    let p = run(
        &mut doc,
        &mut ids,
        "create_node",
        json!({ "parent": f.to_wire(), "kind": { "Path": { "path": "M0,0 L10,0 L5,8 Z" } } }),
    )
    .created[0];
    run(
        &mut doc,
        &mut ids,
        "set_geometry",
        json!({ "node": p.to_wire(), "geometry": "M0,0 L20,0 L10,16 Z" }),
    );
    let NodeKind::Path { path, .. } = doc.get(p).unwrap().kind() else {
        panic!("a path");
    };
    assert_eq!(path.to_svg(), "M0,0 L20,0 L10,16 Z", "path data alone");
    run(
        &mut doc,
        &mut ids,
        "set_geometry",
        json!({ "node": p.to_wire(), "geometry": { "Path": { "path": "M0,0 L20,0 Z" } } }),
    );
    let tree = json!({ "kind": "Group", "children": [
        { "kind": { "Text": { "content": "Hi", "style": { "font_size": 14 } } } },
        { "kind": { "Star": { "size": { "width": 10, "height": 10 } } } },
    ] });
    let g = run(
        &mut doc,
        &mut ids,
        "insert_subtree",
        json!({ "parent": f.to_wire(), "tree": tree }),
    )
    .created[0];
    let kids = doc.get(g).unwrap().children().to_vec();
    assert!(matches!(
        doc.get(kids[0]).unwrap().kind(),
        NodeKind::Text {
            sizing: mastik_core::TextSizing::Auto,
            ..
        }
    ));
    assert_eq!(
        stored(&doc, kids[1])["kind"]["Star"]["inner_ratio"],
        json!(0.382)
    );
}

/// A rounded path's outline written short keeps its corners' radii while it has
/// as many points, and is refused naming `corner_radii` when it has not.
/// flip: `keep_radii` returning at once → fails at "the radii stay".
#[test]
fn a_rounded_paths_short_outline_keeps_its_radii() {
    let (mut doc, mut ids, page) = blank();
    let f = frame_on(&mut doc, &mut ids, page);
    let square = "M0,0 L100,0 L100,100 L0,100 Z";
    let p = run(
        &mut doc,
        &mut ids,
        "create_node",
        json!({ "parent": f.to_wire(), "kind": { "Path": { "path": square, "corner_radii": [10, 10, 10, 10] } } }),
    )
    .created[0];
    let radii = |doc: &Document| match doc.get(p).unwrap().kind() {
        NodeKind::Path { corner_radii, .. } => corner_radii.clone(),
        _ => panic!("a path"),
    };
    assert_eq!(radii(&doc), vec![10.0; 4], "the fixture");
    for geometry in [json!(square), json!({ "Path": square })] {
        run(
            &mut doc,
            &mut ids,
            "set_geometry",
            json!({ "node": p.to_wire(), "geometry": geometry }),
        );
        assert_eq!(radii(&doc), vec![10.0; 4], "the radii stay: {geometry}");
    }
    let fewer = why(
        &doc,
        &mut ids,
        "set_geometry",
        json!({ "node": p.to_wire(), "geometry": "M0,0 L100,0 L50,100 Z" }),
    );
    assert!(fewer.contains("corner_radii"), "{fewer}");
    run(
        &mut doc,
        &mut ids,
        "set_geometry",
        json!({ "node": p.to_wire(), "geometry": { "Path": { "path": square, "corner_radii": [] } } }),
    );
    assert!(radii(&doc).is_empty(), "radii given are written");
}

/// An opacity or alpha outside 0–1 is refused naming it, never folded into a
/// colour: a fill's `opacity`, a paint's `alpha`, a flat effect's `opacity`.
/// flip: `unit` taking any number → fails at "a fill's opacity".
#[test]
fn an_opacity_outside_zero_to_one_is_refused() {
    let (mut doc, mut ids, page) = blank();
    let f = frame_on(&mut doc, &mut ids, page);
    let n = json!([f.to_wire()]);
    let fill = why(
        &doc,
        &mut ids,
        "set_fills",
        json!({ "nodes": n, "fills": [{ "color": "#ff0000", "opacity": 50 }] }),
    );
    assert!(
        fill.contains("`opacity` is from 0 to 1") && fill.contains("0.5"),
        "a fill's opacity: {fill}"
    );
    let negative = why(
        &doc,
        &mut ids,
        "set_fills",
        json!({ "nodes": n, "fills": [{ "color": "#ff0000", "opacity": -1 }] }),
    );
    assert!(negative.contains("opacity"), "{negative}");
    let alpha = why(
        &doc,
        &mut ids,
        "set_fills",
        json!({ "nodes": n, "fills": [{ "paint": { "type": "Solid", "color": "#ff0000", "alpha": 50 } }] }),
    );
    assert!(alpha.contains("`alpha` is from 0 to 1"), "{alpha}");
    let shadow = |opacity: Value, color: Value| {
        json!({ "nodes": n, "effects": [{ "kind": "DropShadow", "color": color, "opacity": opacity,
                                          "blur": 4, "offset": [0, 2], "spread": 0 }] })
    };
    let effect = why(
        &doc,
        &mut ids,
        "set_effects",
        shadow(json!(50), json!("#000000")),
    );
    assert!(effect.contains("`opacity` is from 0 to 1"), "{effect}");

    // A colour in the model's own shape takes the fold too, not only hex.
    let model = json!({ "components": [0.0, 0.0, 0.0, 1.0], "cs": "srgb" });
    run(&mut doc, &mut ids, "set_effects", shadow(json!(0.5), model));
    let effects = serde_json::to_value(doc.get(f).unwrap().effects()).unwrap();
    let a = effects[0]["kind"]["color"]["components"][3].as_f64();
    assert_eq!(a, Some(0.5), "the opacity folded: {effects}");
}

/// A text sizing missing a dimension its type has is refused, not read as 0.
/// flip: `loosen_sizing` reading an absent dimension as 0 → fails at "Fixed".
#[test]
fn a_sizing_without_its_dimension_is_refused() {
    let (doc, mut ids, page) = blank();
    let text = |sizing: Value| json!({ "parent": page.to_wire(), "kind": { "Text": { "content": "Hi", "sizing": sizing } } });
    let fixed = why(
        &doc,
        &mut ids,
        "create_node",
        text(json!({ "type": "Fixed", "width": 100 })),
    );
    assert!(
        fixed.contains("`Fixed` sizing needs its `height`"),
        "Fixed: {fixed}"
    );
    let auto = why(
        &doc,
        &mut ids,
        "create_node",
        text(json!({ "type": "AutoHeight" })),
    );
    assert!(auto.contains("`width`"), "{auto}");
    let taken = answer(
        &doc,
        &mut ids,
        "create_node",
        &text(json!({ "type": "AutoHeight", "width": 80 })),
    );
    assert!(taken.is_ok(), "{taken:?}");
}

/// A field a gradient brush or a grid track has no place for is refused naming
/// it, as one on a solid fill is: their flattened and untagged shapes hide it
/// from serde.
/// flip: `attempt` skipping `unseen_field` → fails at "the gradient's".
#[test]
fn a_gradients_and_a_tracks_unknown_field_is_refused() {
    let (mut doc, mut ids, page) = blank();
    let f = frame_on(&mut doc, &mut ids, page);
    let ramp = mastik_core::peniko::Gradient::new_linear((0.0, 0.0), (10.0, 0.0)).with_stops(
        [
            mastik_core::peniko::Color::BLACK,
            mastik_core::peniko::Color::WHITE,
        ]
        .as_slice(),
    );
    let gradient = serde_json::to_value(mastik_core::Brush::Gradient(mastik_core::GradientBrush {
        gradient: ramp,
        transform: Default::default(),
        opacity: 1.0,
    }))
    .unwrap();
    let ok = json!({ "nodes": [f.to_wire()], "fills": [{ "brush": gradient.clone() }] });
    assert!(
        answer(&doc, &mut ids, "set_fills", &ok).is_ok(),
        "the control"
    );
    let mut bad = gradient;
    bad["Gradient"]["opactiy"] = json!(0.3);
    let refused = why(
        &doc,
        &mut ids,
        "set_fills",
        json!({ "nodes": [f.to_wire()], "fills": [{ "brush": bad }] }),
    );
    assert!(
        refused.contains("opactiy") && refused.contains("no such field"),
        "the gradient's: {refused}"
    );

    let grid = |track: Value| json!({ "nodes": [f.to_wire()], "display": { "Grid": { "columns": [track], "rows": ["Auto"] } } });
    let minmax = json!({ "min": { "Px": 100.0 }, "max": { "Fr": 1.0 } });
    run(&mut doc, &mut ids, "set_display", grid(minmax.clone()));
    let mut extra = minmax;
    extra["step"] = json!(2);
    let refused = why(&doc, &mut ids, "set_display", grid(extra));
    assert!(refused.contains("step"), "a track's: {refused}");
}

/// A gradient is taken without its colour-space fields, its points as `[x, y]`
/// and its stops' colours as hex; the fill example shows it so; and a brush
/// written wrong is refused with serde's own reason, not the untagged one.
/// flip: `brush_miss` answering `None` → fails at "the brush's own reason".
#[test]
fn a_gradient_is_written_short_and_a_wrong_brush_says_why() {
    let (mut doc, mut ids, page) = blank();
    let f = frame_on(&mut doc, &mut ids, page);
    let short = json!({ "Gradient": {
        "kind": { "Linear": { "start": [0, 0], "end": [0, 100] } },
        "stops": [{ "offset": 0, "color": "#0b102066" }, { "offset": 1, "color": "#0b1020" }],
    } });
    let ok = json!({ "nodes": [f.to_wire()], "fills": [{ "brush": short }] });
    let taken = answer(&doc, &mut ids, "set_fills", &ok);
    assert!(taken.is_ok(), "the short gradient: {taken:?}");

    let example = mastik_core::verbs::example("[Fill]").unwrap();
    let shown = example
        .as_array()
        .unwrap()
        .iter()
        .find_map(|f| f.get("brush")?.get("Gradient"))
        .expect("the example shows a gradient");
    assert!(shown.get("interpolation_cs").is_none(), "{shown}");
    assert!(shown["stops"][0]["color"].is_string(), "{shown}");
    assert!(
        mastik_core::verbs::check_model("[Fill]", &example).is_ok(),
        "the example reads back"
    );

    let wrong = json!({ "LinearGradient": { "stops": [] } });
    let refused = why(
        &doc,
        &mut ids,
        "set_fills",
        json!({ "nodes": [f.to_wire()], "fills": [{ "brush": wrong }] }),
    );
    assert!(
        !refused.contains("untagged") && refused.contains("Gradient"),
        "the brush's own reason: {refused}"
    );
}

/// A field an effect's kind or a solid brush's colour has no place for is
/// refused naming it: the tagged and untagged shapes hide it from serde.
/// flip: `unseen_field` without its effect arm → fails at "the shadow's".
#[test]
fn an_effects_and_a_colours_unknown_field_is_refused() {
    let (mut doc, mut ids, page) = blank();
    let f = frame_on(&mut doc, &mut ids, page);
    let n = json!([f.to_wire()]);
    let effects = |effect: Value| json!({ "nodes": n, "effects": [effect] });
    let shadow = json!({ "kind": "DropShadow", "color": "#000000", "blur": 4,
                         "offset": [0, 2], "spread": 0 });
    run(&mut doc, &mut ids, "set_effects", effects(shadow.clone()));
    let mut blnd = shadow;
    blnd["blnd"] = json!("Multiply");
    let refused = why(&doc, &mut ids, "set_effects", effects(blnd));
    assert!(refused.contains("blnd"), "the shadow's: {refused}");
    let nested = json!({ "kind": { "type": "DropShadow", "color": "#000000", "blur": 4,
                                   "offset": [0, 2], "spread": 0, "blnd": "Multiply" } });
    let refused = why(&doc, &mut ids, "set_effects", effects(nested));
    assert!(refused.contains("blnd"), "the nested shadow's: {refused}");
    let blur = json!({ "kind": "LayerBlur", "radius": 4, "opacity": 0.5 });
    let refused = why(&doc, &mut ids, "set_effects", effects(blur));
    assert!(refused.contains("opacity"), "the blur's: {refused}");
    let solid = json!({ "Solid": { "components": [1, 0, 0, 1], "cs": "srgb", "alpha": 0.5 } });
    let fills = json!({ "nodes": n, "fills": [{ "brush": solid }] });
    let refused = why(&doc, &mut ids, "set_fills", fills);
    assert!(refused.contains("alpha"), "the colour's: {refused}");
}

/// A noise effect takes its short forms: flat, one `color` that fades to clear
/// with its `opacity` folded in, `size` as one number, `colors` as hex, and
/// anything left out at its default; a field it has no place for is refused.
/// flip: `loosen_noise` without its `color` arm → fails at the one-colour read.
#[test]
fn a_noise_takes_its_short_forms() {
    use mastik_core::peniko::Color;
    let (mut doc, mut ids, page) = blank();
    let f = frame_on(&mut doc, &mut ids, page);
    let n = json!([f.to_wire()]);
    let noise = |doc: &mut Document, ids: &mut IdSource, effect: Value| {
        run(
            doc,
            ids,
            "set_effects",
            json!({ "nodes": n, "effects": [effect] }),
        );
        match &doc.get(f).unwrap().effects()[0].kind {
            mastik_core::EffectKind::Noise(n) => *n,
            other => panic!("not a noise: {other:?}"),
        }
    };
    let one = noise(
        &mut doc,
        &mut ids,
        json!({ "kind": "Noise", "color": "#ff0000", "opacity": 0.5, "size": 3, "density": 0.4 }),
    );
    assert_eq!(one.size, [3.0, 3.0]);
    assert_eq!(one.density, 0.4);
    assert_eq!(
        one.colors[0],
        Color::from_rgba8(255, 0, 0, 255).with_alpha(0.5)
    );
    assert_eq!(
        one.colors[1].components[3], 0.0,
        "the second end is the colour clear"
    );
    let two = noise(
        &mut doc,
        &mut ids,
        json!({ "kind": { "type": "noise", "colors": ["#000000", "#ffffff80"], "multi": false } }),
    );
    assert_eq!(two.colors[1], Color::from_rgba8(255, 255, 255, 128));
    let bare = noise(&mut doc, &mut ids, json!({ "kind": "noise" }));
    assert_eq!(bare, mastik_core::Noise::default());
    // What `get_node` answers reads back as the noise it describes.
    let summary = json!({ "id": "0:4", "kind": "Noise", "size": [1.0, 2.0], "density": 0.5,
                          "seed": 7, "colors": ["#00000066", "#00000000"], "multi": false,
                          "blend": "Soft light" });
    let read = noise(&mut doc, &mut ids, summary);
    assert_eq!((read.size, read.seed), ([1.0, 2.0], 7));
    assert_eq!(read.blend, mastik_core::BlendMode::SoftLight);
    let refused = why(
        &doc,
        &mut ids,
        "set_effects",
        json!({ "nodes": n, "effects": [{ "kind": "Noise", "densty": 0.5 }] }),
    );
    assert!(refused.contains("densty"), "the noise's: {refused}");
    assert!(
        refused.contains("density") && !refused.contains("spread"),
        "the hint names a noise's fields, not a shadow's: {refused}"
    );
}

/// What `get_node` writes reads back: a line height as `"120%"`, an effect's
/// blend by its label.
/// flip: `css_length` refusing a `%` → fails at "a percent line height".
#[test]
fn a_line_height_in_percent_and_a_blend_label_are_taken() {
    let (mut doc, mut ids, page) = blank();
    let f = frame_on(&mut doc, &mut ids, page);
    let t = run(
        &mut doc,
        &mut ids,
        "create_text",
        json!({ "parent": f.to_wire(), "text": "Hi" }),
    )
    .created[0];
    run(
        &mut doc,
        &mut ids,
        "set_text_style",
        json!({ "nodes": [t.to_wire()], "style": { "line_height": "150%" } }),
    );
    let NodeKind::Text { style, .. } = doc.get(t).unwrap().kind() else {
        panic!("a text");
    };
    assert_eq!(
        style.line_height,
        Some(mastik_core::Length::Em(1.5)),
        "a percent line height"
    );
    run(
        &mut doc,
        &mut ids,
        "set_effects",
        json!({ "nodes": [f.to_wire()], "effects": [{ "kind": "DropShadow", "color": "#000000",
                 "blur": 4, "offset": [0, 2], "spread": 0, "blend": "Color burn" }] }),
    );
    let effects = serde_json::to_value(doc.get(f).unwrap().effects()).unwrap();
    assert_eq!(effects[0]["kind"]["blend"], json!("ColorBurn"), "{effects}");
}

/// A fill given as a colour with an opacity draws that colour at that alpha.
#[test]
fn a_fill_opacity_folds_into_its_colour() {
    let (mut doc, mut ids, page) = blank();
    let f = frame_on(&mut doc, &mut ids, page);
    run(
        &mut doc,
        &mut ids,
        "set_fills",
        json!({ "nodes": [f.to_wire()], "fills": [{ "color": "#ff0000", "opacity": 0.5 }] }),
    );
    let fills = serde_json::to_value(&doc.get(f).unwrap().paint().fills).unwrap();
    let alpha = fills[0]["brush"]["Solid"]["components"][3]
        .as_f64()
        .unwrap();
    assert!((alpha - 0.5).abs() < 0.01, "{fills}");
}

/// `create_node` places a layer's box by `x` and `y`; `insert_subtree` places its
/// root by `x` and `y`.
///
/// flip: `place_corner` returning early → fails at "the rect's left".
#[test]
fn create_node_and_insert_subtree_place_by_the_box() {
    let (mut doc, mut ids, page) = blank();
    let f = frame_on(&mut doc, &mut ids, page);
    let r = run(
        &mut doc,
        &mut ids,
        "create_node",
        json!({ "parent": f.to_wire(), "x": 30, "y": 40,
                "kind": { "Rect": { "size": { "width": 10, "height": 10 } } } }),
    )
    .created[0];
    let at = doc.get(r).unwrap().transform().translation();
    assert_eq!((at.x, at.y), (30.0, 40.0), "the rect's left and top");
    let tree = json!({ "kind": { "Artboard": { "size": { "width": 50, "height": 20 } } },
                       "transform": [1, 0, 0, 1, 5, 5] });
    let root = run(
        &mut doc,
        &mut ids,
        "insert_subtree",
        json!({ "parent": f.to_wire(), "tree": tree, "x": 100, "y": 7 }),
    )
    .created[0];
    let at = doc.get(root).unwrap().transform().translation();
    assert_eq!((at.x, at.y), (100.0, 7.0), "the root's left and top");
}

/// A variable is taken by its name where an id is wanted, and a value bare
/// when the kind says what it is; a taken name answers the variable's id.
///
/// flip: `by_names` leaving names as they are → fails at the first unwrap.
#[test]
fn a_variable_is_named_and_its_value_written_bare() {
    let (mut doc, mut ids, page) = blank();
    let f = frame_on(&mut doc, &mut ids, page);
    let made = run(
        &mut doc,
        &mut ids,
        "add_variable",
        json!({ "kind": "color", "name": "brand/orange", "value": "#ff7a1a" }),
    )
    .made
    .unwrap();
    run(
        &mut doc,
        &mut ids,
        "add_variable",
        json!({ "kind": "number", "name": "radius/card", "value": 16 }),
    );
    run(
        &mut doc,
        &mut ids,
        "set_fills",
        json!({ "nodes": [f.to_wire()], "fills": ["#000000"] }),
    );
    run(
        &mut doc,
        &mut ids,
        "bind_variable",
        json!({ "nodes": [f.to_wire()], "field": { "fill": 0 }, "variable": "brand/orange" }),
    );
    run(
        &mut doc,
        &mut ids,
        "set_variable_value",
        json!({ "variable": "radius/card", "value": 24 }),
    );
    let taken = why(
        &doc,
        &mut ids,
        "add_variable",
        json!({ "kind": "color", "name": "brand/orange" }),
    );
    assert!(taken.contains(&made), "the taken name's id: {taken}");
    let missing = why(
        &doc,
        &mut ids,
        "bind_variable",
        json!({ "nodes": [f.to_wire()], "field": { "fill": 0 }, "variable": "brand/blue" }),
    );
    assert!(
        missing.contains("`brand/orange`"),
        "the names there are: {missing}"
    );
}

/// `get_tree` writes even corners as one number and a line's end as `[x, y]`,
/// and `insert_subtree` reads both back.
#[test]
fn get_tree_writes_corners_and_ends_short() {
    let (mut doc, mut ids, page) = blank();
    let f = frame_on(&mut doc, &mut ids, page);
    let tree = json!({ "kind": "Group", "children": [
        { "kind": { "Rect": { "size": { "width": 10, "height": 10 }, "corner_radii": 6 } } },
        { "kind": { "Line": { "end": [40, 0] } } },
    ] });
    let g = run(
        &mut doc,
        &mut ids,
        "insert_subtree",
        json!({ "parent": f.to_wire(), "tree": tree }),
    )
    .created[0];
    let written = verbs::tree(&doc, g).unwrap();
    let text = written.to_string();
    assert!(text.contains(r#""corner_radii":6"#), "{text}");
    assert!(text.contains(r#""end":[40.0,0.0]"#), "{text}");
    let back = answer(
        &doc,
        &mut ids,
        "insert_subtree",
        &json!({ "parent": f.to_wire(), "tree": written }),
    );
    assert!(back.is_ok(), "{back:?}");
}

/// The names agents write for a bound field, a bound fill and a blur's radius
/// are read: every corner as `"all"`, spacing by its places' names, a fill as
/// `{"variable": id}` and a blur's `blur`; and a parameter no tool takes is
/// refused with the ones it does.
///
/// flip: `loosen_in` without its `field` arm → the `corner: "all"` bind is
/// refused.
#[test]
fn bound_fields_fills_and_blurs_take_the_names_agents_write() {
    let (mut doc, mut ids, page) = blank();
    let f = frame_on(&mut doc, &mut ids, page);
    run(
        &mut doc,
        &mut ids,
        "set_display",
        json!({ "nodes": [f.to_wire()], "display": { "Flex": { "gap": 8 } } }),
    );
    let number = run(
        &mut doc,
        &mut ids,
        "add_variable",
        json!({ "kind": "number", "name": "space/sm", "value": 8 }),
    )
    .made
    .unwrap();
    let colour = run(
        &mut doc,
        &mut ids,
        "add_variable",
        json!({ "kind": "color", "name": "ink", "value": "#112233" }),
    )
    .made
    .unwrap();
    let bind = |field: Value| json!({ "nodes": [f.to_wire()], "field": field, "variable": number });
    run(
        &mut doc,
        &mut ids,
        "bind_variable",
        bind(json!({ "corner": "all" })),
    );
    let n = doc.get(f).unwrap();
    assert!(n.corner_vars().iter().all(Option::is_some), "every corner");
    run(
        &mut doc,
        &mut ids,
        "bind_variable",
        bind(json!({ "spacing": "gaps" })),
    );
    let gaps = doc.get(f).unwrap().spacing_vars();
    assert!(
        gaps[0].is_some() && gaps[1].is_some() && gaps[2].is_none(),
        "the gaps"
    );

    run(
        &mut doc,
        &mut ids,
        "set_fills",
        json!({ "nodes": [f.to_wire()], "fills": [{ "variable": colour }] }),
    );
    assert!(
        doc.get(f).unwrap().paint().fills[0].brush.is_bound(),
        "the bound fill"
    );
    let blur = json!({ "nodes": [f.to_wire()],
        "effects": [{ "kind": { "type": "background_blur", "blur": 20 } }] });
    assert!(
        answer(&doc, &mut ids, "set_effects", &blur).is_ok(),
        "the blur"
    );

    let refused = why(
        &doc,
        &mut ids,
        "set_fills",
        json!({ "nodes": [f.to_wire()], "fills": [], "name": "x" }),
    );
    assert!(refused.contains("`fills`"), "what it takes: {refused}");
}

/// A variant's own word wins over an alias another enum's word has: an image
/// fill's lowercase `fit` reads as `ImageFit::Fit`, not as hugging contents.
/// flip: `repair` trying the alias before the written word → fails at the read.
#[test]
fn a_variants_own_word_wins_over_an_alias() {
    let fill = mastik_core::Fill {
        brush: mastik_core::image::image_brush(mastik_core::ImageId("sha256:x".into())).into(),
        visible: true,
    };
    let mut v = serde_json::to_value(vec![fill]).unwrap();
    v[0]["brush"]["Image"]["image"]["fit"] = json!("fit");
    verbs::check_model("[Fill]", &v).unwrap_or_else(|e| panic!("reads as Fit: {e}"));
}
