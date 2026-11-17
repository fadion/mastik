//! `set`: several properties of several layers in one call, through each
//! property's own setter, with the spellings agents guess for them.

use super::writes::{batch, commit, refuse_kit_vault};
use super::{Answer, Connection, nodes, revisions::stale};
use crate::session::EditorSession;
use mastik_core::NodeId;
use serde_json::{Map, Value, json};

/// What `set` writes for each property, in the order it writes them: the setter
/// verb, whether it takes the layers at once as `nodes` (else one `node` per
/// call), and the name it takes the value under.
const SET_PROPS: &[(&str, &str, bool, &str)] = &[
    ("name", "set_name", false, "name"),
    ("visible", "set_visible", true, "visible"),
    ("locked", "set_locked", true, "locked"),
    ("opacity", "set_opacity", true, "opacity"),
    ("blend", "set_blend", true, "blend"),
    ("clip", "set_clip", true, "clip"),
    ("geometry", "set_geometry", false, "geometry"),
    ("text", "set_text", false, "text"),
    ("text_style", "set_text_style", true, "style"),
    ("paragraph", "set_paragraph_style", true, "paragraph"),
    ("block", "set_block_style", true, "block"),
    ("fills", "set_fills", true, "fills"),
    ("strokes", "set_strokes", true, "strokes"),
    ("effects", "set_effects", true, "effects"),
    ("exports", "set_exports", true, "exports"),
    ("display", "set_display", true, "display"),
    ("item", "set_layout_item", true, "item"),
    ("insets", "set_insets", true, "insets"),
    ("grids", "set_layout_grids", true, "grids"),
    ("element", "set_element", false, "element"),
    ("link", "set_link", false, "href"),
    ("transform", "set_transform", false, "transform"),
];

/// The names agents write for what `set` takes otherwise: `x`, `y` for `at`,
/// `width`, `height` for `size`, `corner_radius` for `geometry`, `fill` and
/// `stroke` for one paint, and `rotation`, which no setter takes.
const SET_ALIASES: [&str; 8] = [
    "x",
    "y",
    "width",
    "height",
    "corner_radius",
    "rotation",
    "fill",
    "stroke",
];

/// Guesses at a `set` property no spelling slip reaches, with the one each means.
const SET_GUESSES: &[(&str, &str)] = &[
    ("color", "fill"),
    ("colour", "fill"),
    ("background", "fill"),
    ("border", "stroke"),
    ("radius", "corner_radius"),
    ("border_radius", "corner_radius"),
    ("hidden", "visible"),
    ("font", "text_style"),
    ("font_size", "text_style"),
    ("content", "text"),
    ("characters", "text"),
    ("angle", "rotation"),
    ("position", "at"),
    ("shadow", "effects"),
];

/// `set`: each property through its own setter, as a [`batch`] named `set`, so
/// one call writes what a handful of setters would and lands as one undo step.
/// `size`, `rotation` and `at` follow the rest in that order, each measured once
/// the ones before have landed, so a move lands where the new size and turn
/// leave the box ([`placed_at`]). New fills on a text clear its style's colour,
/// which would draw over them; a call that changes nothing answers `unchanged`.
pub(super) fn set(
    conn: &Connection,
    s: &mut EditorSession,
    client: &str,
    args: &Value,
) -> Result<Answer, String> {
    let ids = nodes(args, "nodes")?.unwrap_or_default();
    if ids.is_empty() {
        return Err("`nodes` names no layer".into());
    }
    let Some(Value::Object(props)) = args.get("props") else {
        return Err("`props` is an object of properties".into());
    };
    let known: Vec<&str> = SET_PROPS
        .iter()
        .map(|p| p.0)
        .chain(["size", "at"])
        .chain(SET_ALIASES)
        .collect();
    if let Some(k) = props.keys().find(|k| !known.contains(&k.as_str())) {
        let near = nearest_name(k, &known)
            .map(|n| format!(" Did you mean `{n}`?"))
            .unwrap_or_default();
        return Err(format!(
            "`set` sets no `{k}`; it sets {}.{near}",
            known.join(", ")
        ));
    }
    let canon = canonical(props)?;
    if canon.contains_key("text") {
        text_targets(s, &ids)?;
    }
    let size = sides(
        props,
        "size",
        ["width", "height"],
        "`size` is {width, height}",
    )?;
    let at = sides(
        props,
        "at",
        ["x", "y"],
        "`at` is [x, y], the left and top of the layer's box in its parent",
    )?;
    // A layer its container lays out has no place of its own to set.
    if at.is_some()
        && let Some(why) = ids
            .iter()
            .find_map(|id| mastik_core::build::laid_out_refusal(&s.doc, *id))
    {
        return Err(format!("`at`: {why}"));
    }
    let rotation = match props.get("rotation") {
        None => None,
        Some(v) => Some(
            v.as_f64()
                .ok_or("`rotation` is the layer's angle in degrees, clockwise")?,
        ),
    };
    let wire: Vec<Value> = ids.iter().map(|id| json!(id.to_wire())).collect();
    let mut steps = Vec::new();
    let mut step = |tool: &str, args: Value| steps.push(json!({ "tool": tool, "args": args }));
    for (prop, verb, all, param) in SET_PROPS {
        let Some(v) = canon.get(*prop) else { continue };
        let with = |key: &str, layers: Value| {
            let mut m = Map::new();
            m.insert(key.into(), layers);
            m.insert((*param).into(), v.clone());
            Value::Object(m)
        };
        if *all {
            step(verb, with("nodes", json!(wire)));
        } else {
            for id in &wire {
                step(verb, with("node", id.clone()));
            }
        }
    }
    // A text draws in its style's colour over its fills, so new fills clear it.
    let given_colour = canon
        .get("text_style")
        .is_some_and(|st| st.get("color").is_some());
    // Over every text the fills reach, through groups, as `set_fills` writes.
    if canon.contains_key("fills") && !given_colour {
        let coloured: Vec<Value> = mastik_core::build::coloured_texts(&s.doc, &ids)
            .iter()
            .map(|id| json!(id.to_wire()))
            .collect();
        if !coloured.is_empty() {
            step(
                "set_text_style",
                json!({ "nodes": coloured, "style": { "color": null } }),
            );
        }
    }
    let batched = |steps: Vec<Value>| {
        let mut batched = json!({ "steps": steps });
        if let Some(base) = args.get("base_rev") {
            batched["base_rev"] = base.clone();
        }
        batched
    };
    let base = match args.get("base_rev").filter(|v| !v.is_null()) {
        None => None,
        Some(v) => Some(v.as_u64().ok_or("`base_rev` must be a `rev`")?),
    };
    let depth = s.history.undo_depth();
    let landed = (|| {
        if !steps.is_empty() {
            batch(conn, s, client, &batched(steps))?;
        }
        // Each measured after the ones before landed: a side kept is the side
        // the other properties left, and `at` places the box the size and turn left.
        if let Some([w, h]) = size {
            let steps = ids
                .iter()
                .map(|id| sized(s, *id, w, h))
                .collect::<Result<Vec<_>, _>>()?;
            batch(conn, s, client, &batched(steps))?;
        }
        if let Some(degrees) = rotation {
            for id in &ids {
                let turned =
                    mastik_core::edit::turned_to(&s.doc, &s.resolved, *id, degrees.to_radians());
                let Some(tx) = turned else {
                    continue;
                };
                refuse_kit_vault(s, &json!({ "node": id.to_wire() }))?;
                if let Some(base) = base {
                    stale(conn, s, base, &tx)?;
                }
                commit(s, client, "set", tx)?;
            }
        }
        if let Some([x, y]) = at {
            let steps = ids
                .iter()
                .map(|id| placed_at(s, *id, x, y))
                .collect::<Result<Vec<_>, _>>()?;
            batch(conn, s, client, &batched(steps))?;
        }
        Ok::<(), String>(())
    })();
    if let Err(why) = landed {
        s.take_back_to(depth);
        return Err(why);
    }
    let made = s.history.undo_depth().saturating_sub(depth);
    if made > 1 {
        s.history.join_last(made);
    }
    let mut written: Vec<&str> = props.keys().map(String::as_str).collect();
    written.sort_unstable();
    // Named by what it set, as a setter's step is named by its tool.
    if made > 0
        && let Some(note) = s.history.current_note_mut()
    {
        note.label = format!("set {}", written.join(", "));
        note.origin = mastik_core::Origin::Mcp {
            client: client.to_owned(),
            tool: "set".into(),
        };
    }
    if made == 0 {
        // A write that lands nothing must not read as one that did.
        return Ok(Answer::Json(
            json!({ "set": [], "unchanged": written, "note": "the layers already had these values" }),
        ));
    }
    Ok(Answer::Json(json!({ "set": written })))
}

/// The `set_transform` step that puts the left and top of `id`'s box at `x`, `y`
/// in its parent's space ([`mastik_core::edit::placed_at`]).
fn placed_at(
    s: &EditorSession,
    id: NodeId,
    x: Option<f64>,
    y: Option<f64>,
) -> Result<Value, String> {
    let moved = mastik_core::edit::placed_at(&s.doc, &s.resolved, id, x, y)
        .ok_or_else(|| format!("no layer `{}`", id.to_wire()))?;
    Ok(json!({ "tool": "set_transform",
        "args": { "node": id.to_wire(), "transform": moved.as_coeffs() } }))
}

/// The `set_size` step that makes `id` `w` wide and `h` high, a side not given
/// kept at its box's.
fn sized(s: &EditorSession, id: NodeId, w: Option<f64>, h: Option<f64>) -> Result<Value, String> {
    let now = match (w, h) {
        (Some(_), Some(_)) => mastik_core::kurbo::Size::ZERO,
        _ => mastik_core::query::local_box(&s.doc, &s.resolved, id)
            .map(|b| b.size())
            .ok_or_else(|| {
                format!(
                    "`{}` has no box to keep a side of: give `width` and `height`",
                    id.to_wire()
                )
            })?,
    };
    Ok(json!({ "tool": "set_size", "args": { "node": id.to_wire(),
        "width": w.unwrap_or(now.width), "height": h.unwrap_or(now.height) } }))
}

/// `set`'s pair `key` (`size` or `at`) as two sides, each `None` when not
/// given: from the pair as an object or `[a, b]`, then from its sides' own
/// names (`x`, `width`), which win. `None` when neither is written.
fn sides(
    props: &Map<String, Value>,
    key: &str,
    names: [&str; 2],
    form: &str,
) -> Result<Option<[Option<f64>; 2]>, String> {
    let mut out = [None, None];
    let number = |v: &Value| v.as_f64().ok_or_else(|| form.to_owned());
    match props.get(key) {
        None => {}
        Some(Value::Array(a)) if a.len() == 2 => {
            out = [Some(number(&a[0])?), Some(number(&a[1])?)];
        }
        Some(Value::Object(m)) if m.keys().all(|k| names.contains(&k.as_str())) => {
            for (side, name) in out.iter_mut().zip(names) {
                *side = m.get(name).map(number).transpose()?;
            }
        }
        Some(_) => return Err(form.to_owned()),
    }
    for (side, name) in out.iter_mut().zip(names) {
        if let Some(v) = props.get(name) {
            *side = Some(v.as_f64().ok_or_else(|| format!("`{name}` is a number"))?);
        }
    }
    Ok(match out {
        [None, None] if props.contains_key(key) => {
            return Err(format!("{form}; either side alone keeps the other"));
        }
        [None, None] => None,
        sides => Some(sides),
    })
}

/// `props` with `fill`, `stroke` and `corner_radius` written as what their
/// setters take: `fills` and `strokes` lists, and a `geometry` patch.
fn canonical(props: &Map<String, Value>) -> Result<Map<String, Value>, String> {
    let mut canon = props.clone();
    for (alias, prop) in [
        ("fill", "fills"),
        ("stroke", "strokes"),
        ("corner_radius", "geometry"),
    ] {
        let Some(v) = canon.remove(alias) else {
            continue;
        };
        if canon.contains_key(prop) {
            return Err(format!("`{alias}` and `{prop}` set one thing: give one"));
        }
        let v = match (alias, v) {
            ("corner_radius", Value::Number(n)) => json!({ "CornerRadius": n }),
            ("corner_radius", Value::Array(r)) if r.len() == 4 => json!({ "CornerRadii": {
                "top_left": r[0], "top_right": r[1], "bottom_right": r[2], "bottom_left": r[3] } }),
            ("corner_radius", corners @ Value::Object(_)) => json!({ "CornerRadii": corners }),
            ("corner_radius", _) => {
                return Err(
                    "`corner_radius` is a number, or four as [top_left, top_right, \
                     bottom_right, bottom_left]"
                        .into(),
                );
            }
            (_, list @ Value::Array(_)) => list,
            (_, one) => json!([one]),
        };
        canon.insert(prop.into(), v);
    }
    Ok(canon)
}

/// The name of `known` an unknown `set` property most likely meant: a guess
/// [`SET_GUESSES`] lists, else the nearest within two edits once written in
/// snake case.
fn nearest_name<'a>(asked: &str, known: &[&'a str]) -> Option<&'a str> {
    let mut snake = String::new();
    for c in asked.chars() {
        match c {
            '-' | ' ' => snake.push('_'),
            c if c.is_uppercase() => {
                if !snake.is_empty() {
                    snake.push('_');
                }
                snake.extend(c.to_lowercase());
            }
            c => snake.push(c),
        }
    }
    let meant = SET_GUESSES
        .iter()
        .find(|(guess, _)| *guess == snake)
        .map_or(snake.as_str(), |(_, meant)| meant);
    known
        .iter()
        .map(|k| (edits(meant, k), *k))
        .filter(|(d, _)| *d <= 2)
        .min_by_key(|(d, _)| *d)
        .map(|(_, k)| k)
}

/// The edit distance between `a` and `b`, in characters.
fn edits(a: &str, b: &str) -> usize {
    let b: Vec<char> = b.chars().collect();
    let mut row: Vec<usize> = (0..=b.len()).collect();
    for (i, ca) in a.chars().enumerate() {
        let mut diagonal = row[0];
        row[0] = i + 1;
        for (j, cb) in b.iter().enumerate() {
            let above = row[j + 1];
            row[j + 1] = (diagonal + usize::from(ca != *cb))
                .min(row[j] + 1)
                .min(above + 1);
            diagonal = above;
        }
    }
    row[b.len()]
}

/// Refuse `set`'s `text` on a layer that is no text, saying where its text is
/// set: an instance's through its properties, any layer's through the text
/// layers inside it.
fn text_targets(s: &EditorSession, ids: &[NodeId]) -> Result<(), String> {
    for &id in ids {
        let Some(n) = s.doc.get(id) else {
            continue;
        };
        if matches!(n.kind(), mastik_core::NodeKind::Text { .. }) {
            continue;
        }
        let mut texts = Vec::new();
        let mut below: Vec<NodeId> = n.children().iter().rev().copied().collect();
        while let Some(at) = below.pop() {
            let Some(c) = s.doc.get(at) else { continue };
            if matches!(c.kind(), mastik_core::NodeKind::Text { .. }) {
                texts.push(format!("`{}` {}", at.to_wire(), c.name()));
            }
            below.extend(c.children().iter().rev());
        }
        let listed = match texts.len() {
            0 => None,
            n if n > TEXTS_LISTED => Some(format!(
                "{}, and {} more",
                texts[..TEXTS_LISTED].join(", "),
                n - TEXTS_LISTED
            )),
            _ => Some(texts.join(", ")),
        };
        let id_wire = id.to_wire();
        if mastik_core::component::instance_root(&s.doc, id) == Some(id) {
            let props: Vec<String> = mastik_core::variant::instance_properties(&s.doc, id)
                .iter()
                .filter(|p| p.kind != mastik_core::variant::PropKind::Nested && !p.name.is_empty())
                .map(|p| {
                    let kind = format!("{:?}", p.kind).to_lowercase();
                    format!("`{}` ({kind})", p.name)
                })
                .collect();
            let props = match props.is_empty() {
                true => "it has none".to_owned(),
                false => format!("its properties: {}", props.join(", ")),
            };
            let inside = listed
                .map(|l| format!(" or set a text layer inside it ({l})"))
                .unwrap_or_default();
            return Err(format!(
                "`{id_wire}` is an instance; set its text with `set_instance_property` \
                 ({props}){inside}"
            ));
        }
        return Err(match listed {
            Some(l) => format!("`{id_wire}` is no text layer; set a text layer inside it ({l})"),
            None => format!("`{id_wire}` is no text layer and holds none"),
        });
    }
    Ok(())
}

/// The most text layers a refusal of `set`'s `text` names.
const TEXTS_LISTED: usize = 8;

#[cfg(test)]
mod tests {
    use super::*;
    use crate::mcp::Engine;
    use crate::mcp::tests::{One, call, ok, open, rect};

    /// `set`'s `rotation` on a line is the angle the Rotation field shows, its
    /// start-to-end: a line drawn at 45° set to 0 lies flat.
    /// flip: `turned_to` measuring the transform's basis → fails at "flat".
    #[test]
    fn set_rotation_turns_a_line_to_the_angle_shown() {
        let (mut engine, mut docs) = (Engine::default(), One::new());
        open(&mut engine, &mut docs, 1);
        let page = docs.session.doc.first_page().to_wire();
        let tree = json!({ "kind": { "Line": { "end": { "x": 100.0, "y": 100.0 } } } });
        let out = call(
            &mut engine,
            &mut docs,
            1,
            "insert_subtree",
            json!({ "parent": page, "tree": tree }),
        );
        let line = ok(&out)["created"][0].clone();
        let set = json!({ "nodes": [line], "props": { "rotation": 0 } });
        ok(&call(&mut engine, &mut docs, 1, "set", set));
        let id = NodeId::from_wire(line.as_str().unwrap()).unwrap();
        let world = docs.session.resolved.world_transform(id).unwrap();
        let mastik_core::NodeKind::Line { end } = docs.session.doc.get(id).unwrap().kind() else {
            panic!("not a line");
        };
        let (a, b) = (world * mastik_core::kurbo::Point::ZERO, world * *end);
        assert!((b.y - a.y).abs() < 1e-6 && b.x > a.x, "flat: {a:?} → {b:?}");
    }

    /// `set`'s `at`, and `set_transform` by name or through `call`, on a layer
    /// its container lays out is refused, naming the layout, rather than dropped
    /// and answered as done; new fills through a
    /// group clear the colour of the texts inside it.
    ///
    /// flip: `set` without its flex-item refusal → fails at "refused"; the
    /// `set_transform` verb without its → fails at "set_transform refused";
    /// clearing over `ids` alone → fails at "the nested text's colour".
    #[test]
    fn set_refuses_a_laid_out_place_and_clears_nested_text_colour() {
        let (mut engine, mut docs) = (Engine::default(), One::new());
        open(&mut engine, &mut docs, 1);
        let page = docs.session.doc.first_page().to_wire();
        let tree = json!({ "kind": { "Artboard": { "size": { "width": 200, "height": 100 } } },
            "children": [ { "kind": { "Rect": { "size": { "width": 10, "height": 10 } } } } ] });
        let out = call(
            &mut engine,
            &mut docs,
            1,
            "insert_subtree",
            json!({ "parent": page, "tree": tree }),
        );
        let created = ok(&out)["created"].as_array().unwrap().clone();
        let (frame, item) = (created[0].clone(), created[1].clone());
        let display = mastik_core::verbs::example("Display").unwrap();
        ok(&call(
            &mut engine,
            &mut docs,
            1,
            "set_display",
            json!({ "nodes": [frame], "display": display }),
        ));
        let item_id = NodeId::from_wire(item.as_str().unwrap()).unwrap();
        assert!(
            mastik_core::build::is_flex_item(&docs.session.doc, item_id),
            "the fixture laid the item out"
        );
        let out = call(
            &mut engine,
            &mut docs,
            1,
            "set",
            json!({ "nodes": [item], "props": { "x": 50 } }),
        );
        assert!(out.is_error, "refused");
        // The verb itself refuses the move, so every door does.
        let moved = json!({ "node": item, "transform": [1, 0, 0, 1, 50, 0] });
        let out = call(&mut engine, &mut docs, 1, "set_transform", moved.clone());
        assert!(out.is_error, "set_transform refused");
        let call_args = json!({ "tool": "set_transform", "args": moved });
        assert!(call(&mut engine, &mut docs, 1, "call", call_args).is_error);

        let text = call(
            &mut engine,
            &mut docs,
            1,
            "create_text",
            json!({ "parent": page, "text": "Hi" }),
        );
        let text = ok(&text)["created"][0].clone();
        let style = json!({ "nodes": [text], "style": { "color": "#ff0000" } });
        ok(&call(&mut engine, &mut docs, 1, "set_text_style", style));
        let group = call(
            &mut engine,
            &mut docs,
            1,
            "group",
            json!({ "nodes": [text] }),
        );
        let group = ok(&group)["created"][0].clone();
        let fills = json!({ "nodes": [group], "props": { "fills": ["#00ff00"] } });
        ok(&call(&mut engine, &mut docs, 1, "set", fills));
        let id = NodeId::from_wire(text.as_str().unwrap()).unwrap();
        let mastik_core::NodeKind::Text { style, .. } = docs.session.doc.get(id).unwrap().kind()
        else {
            panic!("not a text");
        };
        assert_eq!(style.color, None, "the nested text's colour");
    }

    /// `set` writes several properties through their setters as one undo step
    /// named by what it set, the size before the place.
    ///
    /// flip: `set` skipping its `at` → fails at "moved".
    #[test]
    fn set_writes_several_properties_as_one_step() {
        let (mut engine, mut docs) = (Engine::default(), One::new());
        open(&mut engine, &mut docs, 1);
        let id = rect(&mut engine, &mut docs, 1, "Box");
        let props = json!({ "name": "Card", "opacity": 0.5, "fills": ["#ff0000"],
            "size": { "width": 30.0, "height": 40.0 }, "at": [10.0, 20.0] });
        ok(&call(
            &mut engine,
            &mut docs,
            1,
            "set",
            json!({ "nodes": [id], "props": props }),
        ));
        let nid = NodeId::from_wire(&id).unwrap();
        let n = docs.session.doc.get(nid).unwrap();
        assert_eq!(n.name(), "Card");
        assert_eq!(n.opacity(), 0.5);
        let r = docs.session.resolved.world_bounds(nid).unwrap();
        assert_eq!((r.x0, r.y0), (10.0, 20.0), "moved");
        assert_eq!((r.width(), r.height()), (30.0, 40.0));
        assert_eq!(docs.session.history.undo_depth(), 2, "one step for `set`");
        let label = docs
            .session
            .history
            .done()
            .next_back()
            .unwrap()
            .note
            .label
            .clone();
        assert_eq!(
            label, "set at, fills, name, opacity, size",
            "named by what it set"
        );
        let bad = call(
            &mut engine,
            &mut docs,
            1,
            "set",
            json!({ "nodes": [id], "props": { "colour": "#fff" } }),
        );
        assert!(bad.is_error, "an unknown property is refused");
    }

    /// `set`'s fill on a text whose style carries a colour clears that colour so
    /// the fill is what draws, and a `set` that changes nothing answers `unchanged`.
    ///
    /// flip: `set` skipping its `set_text_style` step → fails at "the style's colour
    /// is cleared"; `set` answering what it was given when nothing landed → fails
    /// at "nothing set".
    #[test]
    fn set_fill_recolours_a_text_and_a_no_op_says_so() {
        let (mut engine, mut docs) = (Engine::default(), One::new());
        open(&mut engine, &mut docs, 1);
        let page = docs.session.doc.first_page().to_wire();
        let tree = json!({ "fills": ["#12352a"], "kind": { "Text": { "content": "Hi",
            "sizing": "Auto", "style": { "color": "#ffffff" } } } });
        let made = call(
            &mut engine,
            &mut docs,
            1,
            "insert_subtree",
            json!({ "parent": page, "tree": tree }),
        );
        let id = ok(&made)["created"][0].as_str().unwrap().to_owned();
        let nid = NodeId::from_wire(&id).unwrap();
        let colour = |docs: &One| match docs.session.doc.get(nid).unwrap().kind() {
            mastik_core::NodeKind::Text { style, .. } => style.color,
            _ => unreachable!(),
        };
        assert!(colour(&docs).is_some(), "the fixture's style has a colour");
        let set = |engine: &mut Engine, docs: &mut One| {
            call(
                engine,
                docs,
                1,
                "set",
                json!({ "nodes": [id], "props": { "fill": "#12352a" } }),
            )
        };
        let first = set(&mut engine, &mut docs);
        assert_eq!(ok(&first)["set"], json!(["fill"]));
        assert_eq!(colour(&docs), None, "the style's colour is cleared");
        let again = set(&mut engine, &mut docs);
        let answer = ok(&again);
        assert_eq!(answer["set"], json!([]), "nothing set");
        assert_eq!(answer["unchanged"], json!(["fill"]));
    }

    /// `set`'s `at` puts a group's box where it puts a layer's: its left and top in
    /// the parent, wherever its children sat, not the group's transform moved there.
    ///
    /// flip: `placed_at` writing `x`, `y` as the transform's offset → fails at "the
    /// box's corner".
    #[test]
    fn set_places_a_groups_box_by_its_corner() {
        let (mut engine, mut docs) = (Engine::default(), One::new());
        open(&mut engine, &mut docs, 1);
        let a = rect(&mut engine, &mut docs, 1, "A");
        let b = rect(&mut engine, &mut docs, 1, "B");
        for (id, at) in [(&a, [100.0, 50.0]), (&b, [160.0, 80.0])] {
            let props = json!({ "at": at });
            ok(&call(
                &mut engine,
                &mut docs,
                1,
                "set",
                json!({ "nodes": [id], "props": props }),
            ));
        }
        let made = call(
            &mut engine,
            &mut docs,
            1,
            "group",
            json!({ "nodes": [a, b] }),
        );
        let group = ok(&made)["created"][0].clone();
        let gid = NodeId::from_wire(group.as_str().unwrap()).unwrap();
        let aid = NodeId::from_wire(&a).unwrap();
        let before = docs.session.resolved.world_bounds(aid).unwrap();
        let box_of = |docs: &One| {
            let s = &docs.session;
            let local = s.resolved.used_local(&s.doc, gid).unwrap();
            let b = mastik_core::query::local_box(&s.doc, &s.resolved, gid).unwrap();
            mastik_core::geometry::transform_rect(local, b).origin()
        };
        assert_eq!(
            box_of(&docs),
            (100.0, 50.0).into(),
            "the group spans its children"
        );
        ok(&call(
            &mut engine,
            &mut docs,
            1,
            "set",
            json!({ "nodes": [group], "props": { "at": [10.0, 20.0] } }),
        ));
        let after = docs.session.resolved.world_bounds(aid).unwrap();
        assert_eq!(
            (after.x0 - before.x0, after.y0 - before.y0),
            (-90.0, -30.0),
            "the box's corner moved to [10, 20]"
        );
        assert_eq!(box_of(&docs), (10.0, 20.0).into());
    }

    /// `set` takes the names agents guess: one side of `at` or `size` keeps the
    /// other, `corner_radius`, `fill` and `rotation` land as their setters would,
    /// and an unknown name is answered with the one it most likely meant.
    ///
    /// flip: `placed_at` taking an absent side as 0 → fails at "y kept";
    /// `turned_to` turning about the layer's origin → fails at "about its centre".
    #[test]
    fn set_takes_the_names_agents_guess() {
        let (mut engine, mut docs) = (Engine::default(), One::new());
        open(&mut engine, &mut docs, 1);
        let id = rect(&mut engine, &mut docs, 1, "Box");
        let nid = NodeId::from_wire(&id).unwrap();
        let mut set = |docs: &mut One, props: Value| {
            call(
                &mut engine,
                docs,
                1,
                "set",
                json!({ "nodes": [id], "props": props }),
            )
        };
        ok(&set(
            &mut docs,
            json!({ "at": [10.0, 20.0], "size": { "width": 30.0, "height": 40.0 } }),
        ));
        ok(&set(&mut docs, json!({ "x": 50.0, "width": 60.0 })));
        let bounds = |docs: &One| docs.session.resolved.world_bounds(nid).unwrap();
        let r = bounds(&docs);
        assert_eq!((r.x0, r.width()), (50.0, 60.0), "the sides given");
        assert_eq!((r.y0, r.height()), (20.0, 40.0), "y kept, and the height");
        ok(&set(&mut docs, json!({ "size": { "height": 10.0 } })));
        assert_eq!(
            (bounds(&docs).width(), bounds(&docs).height()),
            (60.0, 10.0)
        );

        ok(&set(
            &mut docs,
            json!({ "corner_radius": 4.0, "fill": "#ff0000" }),
        ));
        let n = docs.session.doc.get(nid).unwrap();
        let mastik_core::NodeKind::Rect { corner_radii, .. } = n.kind() else {
            panic!("the fixture is a rect");
        };
        assert_eq!(corner_radii.top_left, 4.0);
        assert_eq!(n.paint().fills.len(), 1, "one paint as a list");

        let centre = bounds(&docs).center();
        ok(&set(&mut docs, json!({ "rotation": 90.0 })));
        let r = bounds(&docs);
        assert!(
            (r.width() - 10.0).abs() < 1e-6 && (r.height() - 60.0).abs() < 1e-6,
            "a quarter turn: {r:?}"
        );
        let moved = r.center() - centre;
        assert!(moved.hypot() < 1e-6, "about its centre: {moved:?}");
        ok(&set(&mut docs, json!({ "rotation": 90.0 })));
        assert!(
            (bounds(&docs).width() - 10.0).abs() < 1e-6,
            "the angle is set, not added"
        );

        for (guess, meant) in [("corner_radiu", "corner_radius"), ("colour", "fill")] {
            let out = set(&mut docs, json!({ guess: 1.0 }));
            let text = format!("{:?}", out.content);
            assert!(out.is_error, "{guess} is refused");
            assert!(text.contains(&format!("Did you mean `{meant}`")), "{text}");
        }
    }

    /// `set`'s `text` on an instance names `set_instance_property` with the
    /// instance's properties and the text layers inside it, before any setter runs.
    /// On the way, `make_component` answers a group it made a main as `main`, not
    /// `created`, and `define_property` answers the property by its name.
    ///
    /// flip: `set` without its `text_targets` check → fails at "names the tool",
    /// as the setter's own refusal; `write` not filtering `created` → fails at
    /// "not answered as created".
    #[test]
    fn set_text_on_an_instance_names_where_its_text_is() {
        let (mut engine, mut docs) = (Engine::default(), One::new());
        open(&mut engine, &mut docs, 1);
        let page = docs.session.doc.first_page().to_wire();
        let tree = json!({ "name": "Button", "kind": { "Group": null },
            "children": [{ "name": "Label", "kind": { "Text": { "content": "Go",
                "style": mastik_core::verbs::example("TextStyle").unwrap(), "sizing": "Auto" } } }] });
        let made = call(
            &mut engine,
            &mut docs,
            1,
            "insert_subtree",
            json!({ "parent": page, "tree": tree }),
        );
        let answer = ok(&made).clone();
        let group = answer["created"][0].clone();
        let label = answer["named"]["Button/Label"].clone();
        let made = call(
            &mut engine,
            &mut docs,
            1,
            "make_component",
            json!({ "nodes": [group] }),
        );
        let main = ok(&made)["main"].clone();
        assert_eq!(main, group, "the group itself is made the main");
        assert_eq!(
            ok(&made)["created"],
            json!([]),
            "and is not answered as created"
        );
        let args = json!({ "node": label, "kind": "text", "name": "Caption" });
        let defined = call(&mut engine, &mut docs, 1, "define_property", args);
        assert_eq!(ok(&defined)["property"], "Caption", "named, not by id");
        assert!(ok(&defined).get("made").is_none());
        let args = json!({ "main": main, "parent": page, "x": 300, "y": 300 });
        let made = call(&mut engine, &mut docs, 1, "create_instance", args);
        let instance = ok(&made)["created"][0].clone();
        let out = call(
            &mut engine,
            &mut docs,
            1,
            "set",
            json!({ "nodes": [instance], "props": { "text": "Stop" } }),
        );
        let text = format!("{:?}", out.content);
        assert!(out.is_error);
        assert!(
            text.contains("set_instance_property"),
            "names the tool: {text}"
        );
        assert!(text.contains("`Caption` (text)"), "{text}");
        assert!(text.contains("Label"), "the text layer inside: {text}");
    }
}
