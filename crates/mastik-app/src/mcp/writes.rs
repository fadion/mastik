//! The write path: a registry verb committed as one undo step, and `batch`,
//! which runs several as one that lands whole or not at all.

use super::set::set;
use super::{Answer, Connection, node, nodes, revisions::stale};
use crate::session::EditorSession;
use mastik_core::NodeId;
use serde_json::{Map, Value, json};
use std::collections::HashMap;

/// A registry verb, committed through the session's door as one undo step named
/// by the tool, with the style browser's *Edit* set aside for it.
pub(super) fn write(
    conn: &Connection,
    s: &mut EditorSession,
    client: &str,
    tool: &str,
    args: &Value,
) -> Result<Answer, String> {
    let verb = mastik_core::verbs::find(tool).ok_or_else(|| format!("no tool `{tool}`"))?;
    refuse_kit_vault(s, args)?;
    let mut args = args.clone();
    let base = args
        .as_object_mut()
        .and_then(|m| m.remove("base_rev"))
        .filter(|v| !v.is_null());
    let done = {
        let mut pictures = crate::panels::import_files::Pictures;
        let mut measurer = mastik_export::contrast::Measurer::default();
        let mut ctx = mastik_core::verbs::Ctx {
            doc: &s.doc,
            res: &s.resolved,
            ids: &mut s.ids,
            images: Some(&mut pictures),
            contrast: Some(&mut measurer),
        };
        mastik_core::verbs::call(&mut ctx, verb.name, &args).map_err(|e| e.to_string())?
    };
    if let Some(base) = base {
        let base = base.as_u64().ok_or("`base_rev` must be a `rev`")?;
        stale(conn, s, base, &done.tx)?;
    }
    // A layer that was there before is not one the call created: `make_component`
    // answers a frame it turned into a main as `main` alone.
    let created: Vec<Value> = done
        .created
        .iter()
        .filter(|id| s.doc.get(**id).is_none())
        .map(|id| json!(id.to_wire()))
        .collect();
    commit(s, client, verb.name, done.tx)?;
    // A picture's own size, which decides what aspect `fit` and `crop` keep.
    let pixels = match verb.name {
        "add_image" => done
            .made
            .as_ref()
            .and_then(|id| s.doc.image(&mastik_core::ImageId(id.clone())))
            .map(|e| [e.width, e.height]),
        _ => None,
    };
    let mut answer = json!({ "created": created });
    if let Some(made) = done.made {
        answer[made_key(verb.name)] = match verb.name {
            "define_property" => json!(property_name(s, &args, &made).unwrap_or(made)),
            _ => json!(made),
        };
    }
    if verb.name == "make_component"
        && let Some(main) = done.created.first()
    {
        answer["main"] = json!(main.to_wire());
    }
    if let Some(said) = done.said {
        answer["said"] = json!(said);
    }
    if verb.name == "insert_subtree"
        && let Some(tree) = args.get("tree")
    {
        let mut named = Map::new();
        named_paths(tree, "", &mut done.created.iter(), &mut named);
        // Each id once: `created` keeps the root, which a batch label names,
        // and the layers no name reaches.
        let ids: std::collections::HashSet<&Value> = named.values().collect();
        let created: Vec<Value> = done
            .created
            .iter()
            .enumerate()
            .map(|(i, id)| (i, json!(id.to_wire())))
            .filter(|(i, id)| *i == 0 || !ids.contains(id))
            .map(|(_, id)| id)
            .collect();
        answer["created"] = json!(created);
        answer["named"] = Value::Object(named);
    }
    if let Some(px) = pixels {
        answer["pixel_size"] = json!(px);
    }
    // A move keeps where the layer is drawn, so its place in the new parent is
    // new to the caller, and the next write that places it needs it.
    // Measured in the parent's own space, as `set`'s `at` places it, so a
    // rotated or scaled parent answers the place `at` takes back.
    if verb.name == "reparent"
        && let Ok(Some(id)) = node(&args, "node")
        && let Some(corner) = mastik_core::edit::box_corner(&s.doc, &s.resolved, id)
    {
        let r = |v: f64| (v * 10.0).round() / 10.0;
        answer = json!({ "at": [r(corner.x), r(corner.y)] });
    }
    Ok(Answer::Json(answer))
}

/// The key a write answers what else it made under, named for what it is.
fn made_key(verb: &str) -> &'static str {
    match verb {
        "add_image" => "image",
        "add_variable" => "variable",
        "add_mode" => "mode",
        "add_collection" | "duplicate_collection" => "collection",
        "add_style" => "style",
        "define_property" => "property",
        _ => "made",
    }
}

/// The name of the property `define_property` just made, whose id is `item`:
/// `set_instance_property` takes a property by its name.
fn property_name(s: &EditorSession, args: &Value, item: &str) -> Option<String> {
    let owner = mastik_core::variant::owner_above(&s.doc, node(args, "node").ok()??)?;
    let props = s.doc.get(owner)?.props();
    props
        .iter()
        .find(|p| p.id.0.to_wire() == item)
        .map(|p| p.name.clone())
}

/// Each layer of `tree` the caller named, by the `/`-joined names of it and its
/// named ancestors, with the id `created` minted for it: `insert_subtree` mints
/// in the tree's order, each layer before its children.
fn named_paths<'a>(
    tree: &Value,
    above: &str,
    created: &mut impl Iterator<Item = &'a NodeId>,
    named: &mut Map<String, Value>,
) {
    let Some(id) = created.next() else {
        return;
    };
    let path = match tree.get("name").and_then(Value::as_str) {
        Some(name) => {
            let path = match above {
                "" => name.to_owned(),
                _ => format!("{above}/{name}"),
            };
            named
                .entry(path.clone())
                .or_insert_with(|| json!(id.to_wire()));
            path
        }
        None => above.to_owned(),
    };
    for child in tree
        .get("children")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
    {
        named_paths(child, &path, created, named);
    }
}

/// `batch`: each step's verb or `set` committed in turn, each reading what the last left,
/// then joined into one undo step named `batch`. A refused step takes back the
/// steps before it, so the batch lands whole or not at all. A step names what an
/// earlier one made as `"@label"`, the label that step gave in `as`, as
/// `mastik script` does, and a layer an `insert_subtree` step named by
/// `"@label/Path"`, its path of names below the root; `base_rev` holds for every
/// step.
pub(super) fn batch(
    conn: &Connection,
    s: &mut EditorSession,
    client: &str,
    args: &Value,
) -> Result<Answer, String> {
    let Some(Value::Array(steps)) = args.get("steps") else {
        return Err("`steps` is a list of `{ tool, args, as }`".into());
    };
    if steps.is_empty() {
        return Err("`steps` is empty".into());
    }
    let base = args.get("base_rev").filter(|v| !v.is_null()).cloned();
    let mut labels = HashMap::new();
    let mut answers = Vec::new();
    let mut made = 0;
    let depth = s.history.undo_depth();
    for (i, step) in steps.iter().enumerate() {
        let tool = step.get("tool").and_then(Value::as_str).unwrap_or("?");
        let ran = batch_step(conn, s, client, step, &labels, base.as_ref());
        let (answer, pushed) = match ran {
            Ok(done) => done,
            Err(why) => {
                s.take_back_to(depth);
                return Err(format!(
                    "step {} (`{tool}`): {why}. Nothing was changed.",
                    i + 1
                ));
            }
        };
        made += pushed;
        if let Some(label) = step.get("as").and_then(Value::as_str) {
            record_label(&mut labels, label, tool, &step["args"], &answer);
        }
        answers.push(answer);
    }
    if made > 1 {
        s.history.join_last(made);
    }
    if made > 0
        && let Some(note) = s.history.current_note_mut()
    {
        note.label = "batch".into();
        note.origin = mastik_core::Origin::Mcp {
            client: client.to_owned(),
            tool: "batch".into(),
        };
    }
    Ok(Answer::Json(json!({ "steps": answers })))
}

/// Remember under `label` what a step running `tool` with `args` made, from
/// its `answer`: the first layer it created (or the main, or what its verb names
/// as made), and an `insert_subtree`'s layers by `label/Path`, the path below
/// the root. For [`batch`] and a proposal's calls alike.
pub(super) fn record_label(
    labels: &mut HashMap<String, String>,
    label: &str,
    tool: &str,
    args: &Value,
    answer: &Value,
) {
    let first = answer["created"]
        .get(0)
        .or_else(|| answer.get("main"))
        .or_else(|| answer.get(made_key(tool)))
        .and_then(Value::as_str)
        .map(str::to_owned);
    if let Some(first) = first {
        labels.insert(label.to_owned(), first);
    }
    let root = args["tree"]["name"].as_str();
    for (path, id) in answer
        .get("named")
        .and_then(Value::as_object)
        .into_iter()
        .flatten()
    {
        let below = match root {
            Some(root) => path.strip_prefix(root).and_then(|p| p.strip_prefix('/')),
            None => Some(path.as_str()),
        };
        if let (Some(below), Some(id)) = (below, id.as_str()) {
            labels.insert(format!("{label}/{below}"), id.to_owned());
        }
    }
}

/// One step of [`batch`]: its answer and how many undo steps it committed.
fn batch_step(
    conn: &Connection,
    s: &mut EditorSession,
    client: &str,
    step: &Value,
    labels: &HashMap<String, String>,
    base: Option<&Value>,
) -> Result<(Value, usize), String> {
    if let Some(extra) = step.as_object().and_then(|m| {
        m.keys()
            .find(|k| !["tool", "args", "as"].contains(&k.as_str()))
    }) {
        return Err(format!("a step takes no `{extra}`"));
    }
    let tool = step
        .get("tool")
        .and_then(Value::as_str)
        .ok_or("a step names its `tool`")?;
    if tool != "set" && mastik_core::verbs::find(tool).is_none() {
        return Err(format!(
            "a batch runs `set` and the write tools that are verbs, and `{tool}` is neither"
        ));
    }
    let mut args = step.get("args").cloned().unwrap_or_else(|| json!({}));
    mastik_core::verbs::substitute(&mut args, labels)?;
    if let (Some(base), Some(map)) = (base, args.as_object_mut()) {
        map.insert("base_rev".into(), base.clone());
    }
    let depth = s.history.undo_depth();
    let ran = match tool {
        "set" => set(conn, s, client, &args)?,
        _ => write(conn, s, client, tool, &args)?,
    };
    let Answer::Json(answer) = ran else {
        return Err("a write answers JSON".into());
    };
    Ok((answer, s.history.undo_depth().saturating_sub(depth)))
}

/// Refuse a verb that would edit a kit's vendored component, which a kit
/// instance's `component.main` names: the person can never select one, and an
/// edit there would change every instance until the kit's next update undid it.
/// `main` and `from` only read their layer, so they may name one.
pub(super) fn refuse_kit_vault(s: &EditorSession, args: &Value) -> Result<(), String> {
    let mut ids = Vec::new();
    for key in ["node", "other", "parent"] {
        ids.extend(node(args, key).ok().flatten());
    }
    ids.extend(nodes(args, "nodes").ok().flatten().unwrap_or_default());
    match ids.into_iter().find(|id| s.doc.in_vault(*id)) {
        Some(id) => Err(format!(
            "`{}` is a kit's component, which changes only when the kit does: edit it in the \
             kit's own document and publish, or `make_local` the instance first",
            id.to_wire()
        )),
        None => Ok(()),
    }
}

/// Commit an agent's `tx` as one undo step named by `tool`, with the style
/// browser's *Edit* set aside: an agent's edit means the same whatever is open.
pub(super) fn commit(
    s: &mut EditorSession,
    client: &str,
    tool: &str,
    tx: mastik_core::Transaction,
) -> Result<(), String> {
    s.name_mcp_step(client, tool, "");
    commit_named(s, tx)
}

/// Commit `tx` under the step name already set on `s`, through the door every
/// tool's commit takes.
pub(super) fn commit_named(
    s: &mut EditorSession,
    tx: mastik_core::Transaction,
) -> Result<(), String> {
    let held = s.style_edit.take();
    let applied = s.try_commit(tx);
    s.style_edit = held;
    applied.map_err(|e| match e {
        crate::session::CommitError::Op(e) => format!("Refused: {e}"),
        crate::session::CommitError::ReadOnly => READ_ONLY.to_owned(),
        crate::session::CommitError::Held => {
            "The document is held while a script the person started runs; try again once it ends."
                .to_owned()
        }
    })
}

/// Why nothing an agent asks changes a document the person is comparing.
pub(super) const READ_ONLY: &str = "The document is read-only while the person compares versions.";

#[cfg(test)]
mod tests {
    use super::*;
    use crate::mcp::Engine;
    use crate::mcp::tests::{One, call, ok, open, rect};

    /// `reparent` answers where the moved layer now sits in its new parent,
    /// its box's left and top there, since the move kept it where it was drawn.
    ///
    /// flip: the `reparent` arm of `write` reading the box against the old
    /// parent → fails at `[50, 70]`, answering `[150, 170]`.
    #[test]
    fn reparent_answers_the_place_in_the_new_parent() {
        let (mut engine, mut docs) = (Engine::default(), One::new());
        open(&mut engine, &mut docs, 1);
        let page = docs.session.doc.first_page().to_wire();
        let mut insert = |kind: Value, x: f64, y: f64| {
            let args = json!({ "parent": page, "tree": { "kind": kind }, "x": x, "y": y });
            let out = call(&mut engine, &mut docs, 1, "insert_subtree", args);
            ok(&out)["created"][0].as_str().unwrap().to_owned()
        };
        let frame = insert(
            json!({ "Artboard": { "size": { "width": 200, "height": 200 } } }),
            100.0,
            100.0,
        );
        let dot = insert(
            json!({ "Rect": { "size": { "width": 10, "height": 10 } } }),
            150.0,
            170.0,
        );
        let turned = insert(
            json!({ "Artboard": { "size": { "width": 200, "height": 200 } } }),
            400.0,
            100.0,
        );
        let out = call(
            &mut engine,
            &mut docs,
            1,
            "reparent",
            json!({ "node": dot, "parent": frame }),
        );
        assert_eq!(ok(&out)["at"], json!([50.0, 70.0]), "{:?}", out.structured);

        // Under a turned parent the answer is in the parent's own space: set
        // back as `at`, it moves nothing.
        // flip: the `reparent` arm measuring world box less parent origin →
        // fails at "moves nothing".
        let turn = json!({ "nodes": [turned], "props": { "rotation": 90 } });
        ok(&call(&mut engine, &mut docs, 1, "set", turn));
        let out = call(
            &mut engine,
            &mut docs,
            1,
            "reparent",
            json!({ "node": dot, "parent": turned }),
        );
        let at = ok(&out)["at"].clone();
        let id = NodeId::from_wire(&dot).unwrap();
        let before = docs.session.doc.get(id).unwrap().transform();
        let place = json!({ "nodes": [dot], "props": { "at": at } });
        ok(&call(&mut engine, &mut docs, 1, "set", place));
        let after = docs.session.doc.get(id).unwrap().transform();
        let moved = (after.translation() - before.translation()).hypot();
        assert!(moved < 0.06, "moves nothing: {moved}");
    }

    /// `bytes` in base64, as `add_image` takes a file.
    fn base64(bytes: &[u8]) -> String {
        const ABC: &[u8] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
        let mut out = String::new();
        for chunk in bytes.chunks(3) {
            let b = [
                chunk[0],
                *chunk.get(1).unwrap_or(&0),
                *chunk.get(2).unwrap_or(&0),
            ];
            let n = (u32::from(b[0]) << 16) | (u32::from(b[1]) << 8) | u32::from(b[2]);
            for i in 0..4 {
                match i <= chunk.len() {
                    true => out.push(ABC[(n >> (18 - 6 * i)) as usize & 63] as char),
                    false => out.push('='),
                }
            }
        }
        out
    }

    /// `trace_image` renders the picture through the host and places a *Trace*
    /// group just above it, the picture kept.
    ///
    /// flip: `Pictures` without its `shoot` → fails at the call's `ok`.
    #[test]
    fn trace_image_traces_a_picture_through_the_host() {
        let mut picture = image::RgbaImage::from_pixel(24, 24, image::Rgba([255, 255, 255, 255]));
        for x in 6..18 {
            for y in 6..18 {
                picture.put_pixel(x, y, image::Rgba([0, 0, 0, 255]));
            }
        }
        let mut png = Vec::new();
        picture
            .write_to(&mut std::io::Cursor::new(&mut png), image::ImageFormat::Png)
            .unwrap();
        let (mut engine, mut docs) = (Engine::default(), One::new());
        open(&mut engine, &mut docs, 1);
        let out = call(
            &mut engine,
            &mut docs,
            1,
            "add_image",
            json!({ "bytes": base64(&png) }),
        );
        let image = ok(&out)["image"].as_str().unwrap().to_owned();
        let page = docs.session.doc.first_page().to_wire();
        let example = mastik_core::verbs::example("[Fill]").unwrap()[1].to_string();
        let fill: Value =
            serde_json::from_str(&example.replace("<add_image's id>", &image)).unwrap();
        let tree = json!({ "kind": { "Rect": { "size": { "width": 96, "height": 96 } } },
            "name": "Logo", "fills": [fill] });
        let out = call(
            &mut engine,
            &mut docs,
            1,
            "insert_subtree",
            json!({ "parent": page, "tree": tree }),
        );
        let logo = ok(&out)["created"][0].as_str().unwrap().to_owned();
        let out = call(
            &mut engine,
            &mut docs,
            1,
            "trace_image",
            json!({ "node": logo }),
        );
        let group = ok(&out)["created"][0].as_str().unwrap().to_owned();
        let s = &docs.session;
        let g = s.doc.get(NodeId::from_wire(&group).unwrap()).unwrap();
        assert_eq!(g.name(), mastik_core::trace::GROUP_NAME);
        assert!(!g.children().is_empty(), "paths traced");
        assert!(
            s.doc.get(NodeId::from_wire(&logo).unwrap()).is_some(),
            "kept"
        );
    }

    /// `add_image` answers the picture's own size beside its id.
    ///
    /// flip: `write` leaving out `pixel_size` → fails at the size.
    #[test]
    fn add_image_answers_the_pictures_pixel_size() {
        let mut png = Vec::new();
        image::RgbaImage::new(3, 2)
            .write_to(&mut std::io::Cursor::new(&mut png), image::ImageFormat::Png)
            .unwrap();
        let (mut engine, mut docs) = (Engine::default(), One::new());
        open(&mut engine, &mut docs, 1);
        let args = json!({ "bytes": base64(&png) });
        let out = call(&mut engine, &mut docs, 1, "add_image", args);
        assert!(ok(&out)["image"].is_string());
        assert_eq!(ok(&out)["pixel_size"], json!([3, 2]), "the size");
    }

    /// A batch step names a layer an earlier `insert_subtree` step made by
    /// `@label/Path`, its path of names below the root.
    ///
    /// flip: `batch` not labelling `named` → fails at the batch's `ok`, as no
    /// earlier step is called `card/Title`.
    #[test]
    fn a_batch_names_a_subtrees_layer_by_its_path() {
        let (mut engine, mut docs) = (Engine::default(), One::new());
        open(&mut engine, &mut docs, 1);
        let page = docs.session.doc.first_page().to_wire();
        let kind = mastik_core::verbs::example("NodeKind").unwrap();
        let tree = json!({ "name": "Card", "kind": { "Group": null }, "children": [
            { "name": "Title", "kind": kind },
        ]});
        let insert = json!({ "tool": "insert_subtree", "as": "card",
            "args": { "parent": page, "tree": tree } });
        let rename = json!({ "tool": "set_name",
            "args": { "node": "@card/Title", "name": "Heading" } });
        let out = call(
            &mut engine,
            &mut docs,
            1,
            "batch",
            json!({ "steps": [insert, rename] }),
        );
        let title = ok(&out)["steps"][0]["named"]["Card/Title"]
            .as_str()
            .unwrap()
            .to_owned();
        let n = docs.session.doc.get(NodeId::from_wire(&title).unwrap());
        assert_eq!(n.unwrap().name(), "Heading");
    }

    /// `insert_subtree` answers each named layer by its path of names, an unnamed
    /// layer between them taking its id but no entry, and `created` holds the root
    /// and the unnamed layers, so each id is said once.
    ///
    /// flip: `named_paths` taking an id only for a named layer → fails at the
    /// title's name.
    #[test]
    fn insert_subtree_answers_each_named_layer_by_path() {
        let (mut engine, mut docs) = (Engine::default(), One::new());
        open(&mut engine, &mut docs, 1);
        let page = docs.session.doc.first_page().to_wire();
        let kind = mastik_core::verbs::example("NodeKind").unwrap();
        let tree = json!({ "name": "Hero", "kind": { "Group": null }, "children": [
            { "kind": kind },
            { "name": "Title", "kind": kind },
        ]});
        let out = call(
            &mut engine,
            &mut docs,
            1,
            "insert_subtree",
            json!({ "parent": page, "tree": tree }),
        );
        let answer = ok(&out).clone();
        let created = &answer["created"];
        assert_eq!(answer["named"]["Hero"], created[0], "{answer}");
        assert_eq!(answer["named"].as_object().map(Map::len), Some(2));
        let title = answer["named"]["Hero/Title"].as_str().unwrap();
        let title = NodeId::from_wire(title).unwrap();
        assert_eq!(docs.session.doc.get(title).unwrap().name(), "Title");
        assert_eq!(
            created.as_array().map(Vec::len),
            Some(2),
            "the root and the unnamed layer: {answer}"
        );
        assert!(
            !created
                .as_array()
                .unwrap()
                .contains(&json!(title.to_wire())),
            "each id once"
        );
    }

    /// A batch lands as one undo step named `batch`, a later step naming what an
    /// earlier one made by its label; a refused step leaves the document, the undo
    /// stack and the redo stack as they were.
    ///
    /// flip: `batch` not taking back the steps before a refusal → fails at
    /// "nothing of a refused batch stays".
    #[test]
    fn a_batch_is_one_step_and_lands_whole_or_not_at_all() {
        let (mut engine, mut docs) = (Engine::default(), One::new());
        open(&mut engine, &mut docs, 1);
        let page = docs.session.doc.first_page().to_wire();
        let kind = mastik_core::verbs::example("NodeKind").unwrap();
        let make = json!({ "tool": "create_node", "as": "card",
            "args": { "parent": page, "kind": kind, "name": "Card" } });
        let name = json!({ "tool": "set_name", "args": { "node": "@card", "name": "Hero" } });
        let out = call(
            &mut engine,
            &mut docs,
            1,
            "batch",
            json!({ "steps": [make, name] }),
        );
        let card = ok(&out)["steps"][0]["created"][0]
            .as_str()
            .unwrap()
            .to_owned();
        let id = NodeId::from_wire(&card).unwrap();
        let s = &docs.session;
        assert_eq!(s.doc.get(id).unwrap().name(), "Hero");
        assert_eq!(s.history.undo_depth(), 1, "one step");
        assert_eq!(s.history.done().last().unwrap().note.label, "batch");

        // The person undoes it, so there is a step to redo.
        assert!(docs.session.undo());
        let layers = docs.session.doc.len();
        let bad = json!({ "tool": "set_name", "args": { "node": "@nobody", "name": "X" } });
        let out = call(
            &mut engine,
            &mut docs,
            1,
            "batch",
            json!({ "steps": [make, bad] }),
        );
        assert!(out.is_error);
        let s = &docs.session;
        assert_eq!(s.doc.len(), layers, "nothing of a refused batch stays");
        assert_eq!(s.history.undo_depth(), 0);
        // flip: `answer` not giving back the redo it held → fails here.
        assert_eq!(s.history.redo_depth(), 1, "the person's redo is kept");
        assert!(docs.session.redo(), "and still redoes");
        assert_eq!(docs.session.doc.get(id).unwrap().name(), "Hero");
    }

    /// A batch step can be `set`, naming an earlier step's layer by its label, and
    /// the batch is still one step; a `set` refused there takes back the steps
    /// before it.
    ///
    /// flip: `batch_step` refusing a tool that is no verb, as it did `set` → fails
    /// at the first batch's `ok`.
    #[test]
    fn a_batch_runs_set_as_a_step() {
        let (mut engine, mut docs) = (Engine::default(), One::new());
        open(&mut engine, &mut docs, 1);
        let page = docs.session.doc.first_page().to_wire();
        let kind = mastik_core::verbs::example("NodeKind").unwrap();
        let make = json!({ "tool": "create_node", "as": "card",
            "args": { "parent": page, "kind": kind, "name": "Card" } });
        let set = json!({ "tool": "set",
            "args": { "nodes": ["@card"], "props": { "name": "Hero", "opacity": 0.5 } } });
        let out = call(
            &mut engine,
            &mut docs,
            1,
            "batch",
            json!({ "steps": [make, set] }),
        );
        let card = ok(&out)["steps"][0]["created"][0]
            .as_str()
            .unwrap()
            .to_owned();
        let s = &docs.session;
        let n = s.doc.get(NodeId::from_wire(&card).unwrap()).unwrap();
        assert_eq!((n.name(), n.opacity()), ("Hero", 0.5));
        assert_eq!(s.history.undo_depth(), 1, "one step");
        assert_eq!(s.history.done().last().unwrap().note.label, "batch");

        let layers = docs.session.doc.len();
        let bad = json!({ "tool": "set",
            "args": { "nodes": ["@card"], "props": { "name": "X", "colour": "#fff" } } });
        let out = call(
            &mut engine,
            &mut docs,
            1,
            "batch",
            json!({ "steps": [make, bad] }),
        );
        assert!(out.is_error);
        let s = &docs.session;
        assert_eq!(s.doc.len(), layers, "nothing of a refused batch stays");
        assert_eq!(s.history.undo_depth(), 1);
    }

    /// A refused `batch` or `set` that took back the steps it made leaves `rev`
    /// where it was, for the caller and for another client, with no `changed`.
    ///
    /// flip: `handle` without its `rewind` → fails at "the batch's rev".
    #[test]
    fn a_refused_write_leaves_rev_where_it_was() {
        let (mut engine, mut docs) = (Engine::default(), One::new());
        open(&mut engine, &mut docs, 1);
        open(&mut engine, &mut docs, 2);
        let id = rect(&mut engine, &mut docs, 1, "Card");
        let look = |e: &mut Engine, d: &mut One, conn| {
            let out = call(e, d, conn, "get_selection", json!({}));
            ok(&out).clone()
        };
        let seen = look(&mut engine, &mut docs, 2);
        let rev = look(&mut engine, &mut docs, 1)["rev"].clone();
        let renamed = json!({ "tool": "set_name", "args": { "node": id, "name": "Hero" } });
        let bad = json!({ "tool": "set_name", "args": { "node": "@nobody", "name": "X" } });
        let args = json!({ "steps": [renamed, bad] });
        let out = call(&mut engine, &mut docs, 1, "batch", args);
        assert!(out.is_error, "the fixture's batch is refused");
        assert_eq!(docs.session.history.undo_depth(), 1, "after a step it made");
        assert_eq!(json!(docs.session.revision()), rev, "the batch's rev");
        let props = json!({ "name": "Hero", "opacity": "loud" });
        let out = call(
            &mut engine,
            &mut docs,
            1,
            "set",
            json!({ "nodes": [id], "props": props }),
        );
        assert!(out.is_error, "the fixture's set is refused");
        assert_eq!(json!(docs.session.revision()), rev, "the set's rev");
        let mine = look(&mut engine, &mut docs, 1);
        assert_eq!((&mine["rev"], mine.get("changed")), (&rev, None));
        let theirs = look(&mut engine, &mut docs, 2);
        assert_eq!(theirs["rev"], seen["rev"]);
        assert!(theirs.get("changed").is_none(), "{theirs}");
    }

    /// An agent's write leaves the style browser's *Edit* as it found it.
    #[test]
    fn a_write_leaves_the_style_edit_open() {
        let (mut engine, mut docs) = (Engine::default(), One::new());
        open(&mut engine, &mut docs, 1);
        let style: mastik_core::style::StyleId = serde_json::from_value(json!("1:1")).unwrap();
        docs.session.style_edit = Some(style);
        rect(&mut engine, &mut docs, 1, "Chip");
        assert_eq!(docs.session.style_edit, Some(style));
    }
}
