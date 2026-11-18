//! The reads an agent takes before adding to a page and after (§8.4, Phase 1):
//! `design_context`, what the layers under a frame or page already use;
//! `check_layout`, the placement mistakes a render would otherwise show; and
//! `review`, the design checks past those. They are answered by
//! `mastik_export::context` and `mastik_export::review`; this reads their
//! arguments.

use super::{Answer, node, nodes};
use crate::session::EditorSession;
use serde_json::Value;

/// The tools this module answers.
pub(super) const TOOLS: [&str; 4] = ["design_context", "check_layout", "review", "health"];

/// Answer `tool` on `s`.
pub(super) fn run(s: &EditorSession, tool: &str, args: &Value) -> Result<Answer, String> {
    let (doc, res) = (&s.doc, &s.resolved);
    // `ids`' outermost layers, else a page: what the checks cover.
    let roots = || -> Result<Vec<mastik_core::NodeId>, String> {
        Ok(match nodes(args, "ids")? {
            Some(ids) => {
                if let Some(gone) = ids.iter().find(|i| doc.get(**i).is_none()) {
                    return Err(format!("no layer `{}`", gone.to_wire()));
                }
                mastik_core::build::outermost(doc, &ids)
            }
            None => vec![super::page_of(s, node(args, "page")?)?],
        })
    };
    match tool {
        "design_context" => {
            let asked = node(args, "id")?.unwrap_or(s.page);
            mastik_export::context::design_context(doc, res, asked).map(Answer::Json)
        }
        "check_layout" => Ok(Answer::Json(mastik_export::context::check_layout(
            doc,
            res,
            &roots()?,
        ))),
        "review" => {
            use mastik_export::contrast::Level;
            use mastik_export::review::{Options, Scope, review, to_json};
            let level = match args.get("level").and_then(Value::as_str) {
                None | Some("aa") => Level::Aa,
                Some("aaa") => Level::Aaa,
                Some("apca") => Level::Apca,
                Some(other) => return Err(format!("`level` is aa, aaa or apca, not `{other}`")),
            };
            let step = match args.get("step").and_then(Value::as_f64) {
                None => Options::default().step,
                Some(s) if s > 0.0 => s,
                Some(_) => return Err("`step` is a length above zero".into()),
            };
            let scope = Scope::new(doc, res, &roots()?);
            let found = review(&scope, Options { step, level });
            Ok(Answer::Json(to_json(&scope, &found)))
        }
        "health" => {
            use mastik_export::review::{Options, Scope, health};
            let step = match args.get("step").and_then(Value::as_f64) {
                None => Options::default().step,
                Some(s) if s > 0.0 => s,
                Some(_) => return Err("`step` is a length above zero".into()),
            };
            let every = args.get("pages").and_then(Value::as_bool) == Some(true);
            let roots = match every && args.get("ids").is_none() {
                true => mastik_export::review::every_page(doc, s.page),
                false => roots()?,
            };
            let scope = Scope::new(doc, res, &roots);
            Ok(Answer::Json(health(&scope, step).to_json(doc)))
        }
        tool => Err(format!("no tool `{tool}`")),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use mastik_core::kurbo::{Affine, RoundedRectRadii, Size};
    use mastik_core::{BlockStyle, NodeId, NodeKind, TextSizing, TextStyle};
    use serde_json::json;

    fn frame(w: f64, h: f64) -> Value {
        json!(NodeKind::Artboard {
            size: Size::new(w, h),
            corner_radii: RoundedRectRadii::default(),
            corner_smoothing: 0.0,
        })
    }

    fn at(x: f64, y: f64) -> Value {
        json!(Affine::translate((x, y)))
    }

    /// A session whose page holds only `tree`.
    fn with(tree: Value) -> EditorSession {
        let mut s = EditorSession::new();
        let starter: Vec<String> = s
            .doc
            .get(s.page)
            .unwrap()
            .children()
            .iter()
            .map(|c| c.to_wire())
            .collect();
        let page = s.page.to_wire();
        let call = |s: &mut EditorSession, name: &str, args: Value| {
            let done = {
                let mut ctx = mastik_core::verbs::Ctx {
                    doc: &s.doc,
                    res: &s.resolved,
                    ids: &mut s.ids,
                    images: None,
                    contrast: None,
                };
                mastik_core::verbs::call(&mut ctx, name, &args).unwrap()
            };
            s.try_commit(done.tx).unwrap();
        };
        if !starter.is_empty() {
            call(&mut s, "delete_node", json!({ "nodes": starter }));
        }
        call(
            &mut s,
            "insert_subtree",
            json!({ "parent": page, "tree": tree }),
        );
        s
    }

    /// `get_outline`'s origin is where the layer's own (0, 0) lies, so a
    /// child's world box less it is the child's position in the layer.
    ///
    /// flip: `reads::origin` reading the layer's local transform for its world
    /// one → fails at the hero's origin, `[0, 200]`.
    #[test]
    fn the_outline_gives_the_origin_a_child_is_placed_from() {
        let headline = NodeKind::Text {
            content: "A headline".into(),
            style: Box::new(TextStyle {
                font_size: 56.0,
                weight: 700,
                ..TextStyle::default()
            }),
            spans: Default::default(),
            para_spans: Default::default(),
            paragraph: Default::default(),
            block: BlockStyle::default(),
            sizing: TextSizing::Auto,
            on_path: None,
            on_path_flip: false,
            on_path_offset: 0.0,
        };
        let s = with(json!({
            "kind": frame(1440.0, 1100.0), "name": "Site", "transform": at(100.0, 50.0),
            "children": [
                { "kind": frame(1440.0, 80.0), "name": "Navigation" },
                {
                    "kind": frame(1440.0, 400.0), "name": "Hero", "transform": at(0.0, 200.0),
                    "children": [{ "kind": headline, "transform": at(120.0, 40.0) }],
                },
            ],
        }));
        let site: NodeId = s.doc.get(s.page).unwrap().children()[0];
        let hero = s.doc.get(site).unwrap().children()[1];
        let out = super::super::reads::outline(&s, hero, 1, 50);
        assert_eq!(out["origin"], json!([100, 250]), "{out}");
        let headline = &out["rows"][1];
        assert_eq!(
            (headline[3].clone(), headline[4].clone()),
            (json!(220), json!(290))
        );
        let page = super::super::reads::outline(&s, s.page, 1, 50);
        assert_eq!(page["origin"], json!([0, 0]));
    }

    /// The handler hands the page, or the outermost of the layers named, to
    /// `check_layout`, and refuses a layer that is gone.
    #[test]
    fn check_layout_reads_its_arguments() {
        let s = with(json!({ "kind": frame(100.0, 100.0), "name": "Card" }));
        let card = s.doc.get(s.page).unwrap().children()[0];
        let Answer::Json(page) = run(&s, "check_layout", &json!({})).unwrap() else {
            panic!("json")
        };
        assert_eq!(page["checked"], json!(1));
        let Answer::Json(one) =
            run(&s, "check_layout", &json!({ "ids": [card.to_wire()] })).unwrap()
        else {
            panic!("json")
        };
        assert_eq!(one["checked"], json!(1));
        let gone = run(&s, "check_layout", &json!({ "ids": ["5a5a:999"] }));
        assert!(gone.is_err());
        let Answer::Json(ctx) = run(&s, "design_context", &json!({})).unwrap() else {
            panic!("json")
        };
        assert_eq!(ctx["root"]["name"], json!("Card"));
    }

    /// `review` hands the page to `mastik_export::review` with the step and
    /// level it was given, and refuses a step of zero.
    ///
    /// flip: the `review` arm ignoring `step` → fails at "a gap of 15 is on a
    /// 5-unit scale".
    #[test]
    fn review_reads_its_arguments() {
        let s = with(json!({ "kind": frame(300.0, 100.0), "name": "Row",
            "display": { "Flex": { "direction": "Row", "gap": 15 } } }));
        let found = |args: Value| {
            let Answer::Json(v) = run(&s, "review", &args).unwrap() else {
                panic!("json")
            };
            v["findings"]
                .as_array()
                .unwrap()
                .iter()
                .any(|f| f["code"] == "off_scale")
        };
        assert!(found(json!({})), "15 is off the 4-unit scale");
        assert!(
            !found(json!({ "step": 5 })),
            "a gap of 15 is on a 5-unit scale"
        );
        assert!(run(&s, "review", &json!({ "step": 0 })).is_err());
    }
}
