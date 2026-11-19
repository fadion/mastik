//! `export_png` with `since`: the document as it stood at an earlier revision
//! drawn beside it now, over the part that changed, so an agent sees what its
//! last steps did. The earlier document is rebuilt from the history and held to
//! the fingerprint remembered at that look, so a picture of a state the history
//! cannot give back is refused rather than drawn wrong.

use super::revisions::{self, Layer};
use super::{Answer, Connection, PNG_CEILING, PNG_MAX, index, is_page, node, png_viewport, region};
use crate::session::EditorSession;
use mastik_core::kurbo::Rect;
use mastik_core::{Document, NodeId, Resolved};
use mastik_mcp::wire::Content;
use serde_json::{Value, json};
use std::collections::HashMap;

/// How far past what changed the pictures reach, in world units: enough to see
/// it among its neighbours.
const MARGIN: f64 = 16.0;

/// The session's document as it stood when `top` was the step on top of the
/// history (`History::state_at`).
fn document_at(s: &EditorSession, top: Option<u64>) -> Option<Document> {
    s.history.state_at(&s.doc, top)
}

/// `export_png` drawing the document at revision `since` and now, the same
/// rectangle of the same page at the same scale: `region` when given, else the
/// layer `id`'s box then and now, else the boxes of what changed on the
/// person's page (or the first page anything changed on), [`MARGIN`] around.
pub(super) fn export_png_since(
    conn: &Connection,
    s: &EditorSession,
    args: &Value,
    since: u64,
) -> Result<Answer, String> {
    if since == s.revision() {
        return Ok(Answer::Json(json!({ "since": since, "unchanged": true })));
    }
    let (top, print) = revisions::look_at(conn, since).ok_or_else(|| {
        format!(
            "`since` {since} is no revision you were shown: pass a `rev` from an earlier result"
        )
    })?;
    let old = document_at(s, top)
        .ok_or("The history no longer reaches that revision: export without `since`")?;
    let old_res = Resolved::rebuild(&old);
    let then = revisions::layers_of_doc(&old, &old_res);
    if revisions::fingerprint(&then) != print {
        return Err(
            "The document at that revision cannot be rebuilt from the history (an edit merged \
             into a step since, or the step was dropped): export without `since`"
                .into(),
        );
    }
    let now = revisions::layers_of(s);
    let d = revisions::delta(&then, &now);
    let max = index(args, "max").unwrap_or(PNG_MAX);
    if max > PNG_CEILING {
        return Err(format!("`max` is at most {PNG_CEILING}"));
    }
    let max = max.max(16) as f64;
    let id = node(args, "id")?;
    let changed_on = |page: NodeId| -> Option<Rect> {
        boxes(&d.added, &now, |i| s.resolved.world_bounds(i), &s.doc, page)
            .chain(boxes(
                &d.changed,
                &now,
                |i| s.resolved.world_bounds(i),
                &s.doc,
                page,
            ))
            .chain(boxes(
                &d.changed,
                &then,
                |i| old_res.world_bounds(i),
                &old,
                page,
            ))
            .chain(boxes(
                &d.removed,
                &then,
                |i| old_res.world_bounds(i),
                &old,
                page,
            ))
            .reduce(|a, b| a.union(b))
    };
    let (page, view) = match (region(args)?, id) {
        (Some(r), id) => (super::region_page(s, id)?, Some(r)),
        (None, Some(id)) if !is_page(s, id) => {
            // A layer removed since is on the page it was on then.
            let page = s
                .doc
                .page_of(id)
                .or_else(|| old.page_of(id))
                .ok_or_else(|| format!("no layer `{}`", id.to_wire()))?;
            let then = mastik_export::extent(&old_res, &[id]);
            let now = mastik_export::extent(&s.resolved, &[id]);
            (page, then.into_iter().chain(now).reduce(|a, b| a.union(b)))
        }
        (None, id) => {
            let page = match id {
                Some(p) => p,
                None => {
                    let pages = d
                        .added
                        .iter()
                        .chain(&d.changed)
                        .chain(&d.removed)
                        .filter_map(|w| NodeId::from_wire(w))
                        .filter_map(|i| s.doc.page_of(i).or_else(|| old.page_of(i)));
                    let mut pages: Vec<NodeId> = pages.collect();
                    match pages.contains(&s.page) || pages.is_empty() {
                        true => s.page,
                        false => pages.swap_remove(0),
                    }
                }
            };
            (page, changed_on(page).map(|r| r.inflate(MARGIN, MARGIN)))
        }
    };
    let counts = json!({
        "added": d.added.len(),
        "changed": d.changed.len(),
        "removed": d.removed.len(),
    });
    let Some(view) = view.filter(|v| v.width() > 0.0 && v.height() > 0.0) else {
        return Ok(Answer::Json(json!({
            "since": since,
            "page": page.to_wire(),
            "unchanged": true,
            "layers": counts,
        })));
    };
    let scale = (max / view.width().max(view.height())).min(2.0);
    let vp = png_viewport("What changed", view, scale, max)?;
    let image = |bytes| Content::Image {
        bytes,
        mime: "image/png",
    };
    let mut files = Vec::new();
    let before = old.get(page).is_some();
    if before {
        files.push(Content::Text("before".into()));
        files.push(image(mastik_export::png::png(&old, &old_res, &vp, page)));
    }
    let after = s.doc.get(page).is_some();
    if after {
        files.push(Content::Text("after".into()));
        files.push(image(mastik_export::png::png(
            &s.doc,
            &s.resolved,
            &vp,
            page,
        )));
    }
    let r = |v: f64| (v * 10.0).round() / 10.0;
    let shown: Vec<&str> = [(before, "before"), (after, "after")]
        .into_iter()
        .filter_map(|(drawn, name)| drawn.then_some(name))
        .collect();
    let mut report = json!({
        "since": since,
        "page": page.to_wire(),
        "box": [r(view.x0), r(view.y0), r(view.width()), r(view.height())],
        "layers": counts,
        "images": shown,
    });
    if !before {
        report["before"] = json!("the page did not exist then");
    }
    if !after {
        report["after"] = json!("the page is gone now");
    }
    Ok(Answer::Files(report, files))
}

/// The world boxes of the layers `ids` names that lie on `page` of `doc`, the
/// page itself left out: a page reads as changed whenever a layer is added to
/// it.
fn boxes<'a>(
    ids: &'a [String],
    layers: &'a HashMap<String, Layer>,
    bounds: impl Fn(NodeId) -> Option<Rect> + 'a,
    doc: &'a Document,
    page: NodeId,
) -> impl Iterator<Item = Rect> + 'a {
    ids.iter()
        .filter(move |w| layers.contains_key(*w))
        .filter_map(|w| NodeId::from_wire(w))
        .filter(move |i| *i != page && doc.page_of(*i) == Some(page))
        .filter_map(bounds)
}

#[cfg(test)]
mod tests {
    use crate::mcp::Engine;
    use crate::mcp::tests::{One, call, ok, open, rect};
    use mastik_mcp::wire::{Content, Output};
    use serde_json::json;

    fn images(out: &Output) -> usize {
        out.content
            .iter()
            .filter(|c| matches!(c, Content::Image { .. }))
            .count()
    }

    /// `export_png` with `since` draws the page then and now over the layer the
    /// agent added, and after the person's undo still rebuilds the revision
    /// from the steps undone.
    ///
    /// flip: `document_at` answering the document as it is now → refused by the
    /// fingerprint check, failing at the first `ok`.
    #[test]
    fn since_draws_then_and_now_over_what_changed() {
        let (mut engine, mut docs) = (Engine::default(), One::new());
        open(&mut engine, &mut docs, 1);
        let read = call(&mut engine, &mut docs, 1, "get_selection", json!({}));
        let base = ok(&read)["rev"].clone();
        let card = rect(&mut engine, &mut docs, 1, "Card");
        let paint = json!({ "nodes": [card], "props": { "fill": "#ff0000" } });
        ok(&call(&mut engine, &mut docs, 1, "set", paint));
        let out = call(
            &mut engine,
            &mut docs,
            1,
            "export_png",
            json!({ "since": base }),
        );
        let report = ok(&out).clone();
        assert_eq!(report["layers"]["added"], 1, "{report}");
        assert_eq!(images(&out), 2, "before and after");
        let pictures: Vec<&Vec<u8>> = out
            .content
            .iter()
            .filter_map(|c| match c {
                Content::Image { bytes, .. } => Some(bytes),
                _ => None,
            })
            .collect();
        assert_ne!(pictures[0], pictures[1], "the two images differ");
        let id = mastik_core::NodeId::from_wire(&card).unwrap();
        let b = docs.session.resolved.world_bounds(id).unwrap();
        let shown = &report["box"];
        assert!(
            shown[0].as_f64().unwrap() <= b.x0 && shown[2].as_f64().unwrap() >= b.width(),
            "the box holds the card: {report}"
        );

        // The person undoes the card's fill; the revision with it is rebuilt by
        // redoing.
        let with_card = ok(&out)["rev"].clone();
        assert!(docs.session.undo());
        let out = call(
            &mut engine,
            &mut docs,
            1,
            "export_png",
            json!({ "since": with_card }),
        );
        assert_eq!(ok(&out)["layers"]["changed"], 1, "{:?}", out.structured);
        assert_eq!(images(&out), 2);

        let unknown = call(
            &mut engine,
            &mut docs,
            1,
            "export_png",
            json!({ "since": 7 }),
        );
        assert!(unknown.is_error, "a revision never shown");
    }

    /// `since` refuses a `region` beside a layer's `id` as the plain export
    /// does, and draws a layer removed since by its box then, on its page.
    ///
    /// flip: `export_png_since` reading a region's page without `region_page` →
    /// fails at "a region with a layer"; finding the layer's page in the
    /// document now alone → fails at the removed layer's `ok`.
    #[test]
    fn since_refuses_a_region_with_a_layer_and_draws_one_removed() {
        let (mut engine, mut docs) = (Engine::default(), One::new());
        open(&mut engine, &mut docs, 1);
        let card = rect(&mut engine, &mut docs, 1, "Card");
        let read = call(&mut engine, &mut docs, 1, "get_selection", json!({}));
        let base = ok(&read)["rev"].clone();
        rect(&mut engine, &mut docs, 1, "Other");
        let args = json!({ "since": base, "id": card, "region": [0, 0, 400, 300] });
        let out = call(&mut engine, &mut docs, 1, "export_png", args);
        assert!(out.is_error, "a region with a layer");
        assert!(
            format!("{:?}", out.content).contains("is a layer"),
            "{:?}",
            out.content
        );

        let gone = json!({ "nodes": [card] });
        ok(&call(&mut engine, &mut docs, 1, "delete_node", gone));
        let out = call(
            &mut engine,
            &mut docs,
            1,
            "export_png",
            json!({ "since": base, "id": card }),
        );
        let report = ok(&out).clone();
        assert_eq!(report["layers"]["removed"], 1, "{report}");
        assert_eq!(images(&out), 2, "the card then, its place now");
    }
}
