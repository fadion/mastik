//! A live brush: a path layer linked to a brush layer and the *Apply brush*
//! card's options (`Node::brush`). The brush draws along the path in place of
//! the path's strokes, its fills still drawn, rebuilt from the bake's geometry
//! ([`apply_brush::ink`]) whenever the path, the brush or the options change;
//! nothing of the brush is copied into the layer.
//!
//! The link is a reference, so every door that could leave it naming nothing
//! bakes instead: deleting the brush ([`baked_for_deletes`]) and pasting the
//! layer where its brush is not.

use crate::apply_brush::{self, ApplyBrush, Ink, Mode, Refusal};
use crate::kurbo::{Affine, BezPath};
use crate::{Document, IdSource, NodeId, Operation, Resolved};

/// A layer's link to the brush it draws with.
#[derive(Clone, Copy, Debug, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct BrushLink {
    /// The brush layer.
    pub of: NodeId,
    pub mode: Mode,
    /// Whether the brush draws; hidden, the path's own strokes draw again.
    pub visible: bool,
}

impl BrushLink {
    /// A shown link to `of` with `mode`.
    pub fn new(of: NodeId, mode: Mode) -> BrushLink {
        BrushLink {
            of,
            mode,
            visible: true,
        }
    }
}

/// Whether a layer of `kind` has an outline a brush can run along.
pub fn runs_along(kind: &crate::NodeKind) -> bool {
    apply_brush::outlined(kind)
}

/// Why `brush` cannot be linked to `path`, if it cannot: the bake's tests but
/// the two a bake owes its group (a mask, an auto layout), a brush inside an
/// instance, and a link that would loop back to the path.
pub fn refusal(doc: &Document, res: &Resolved, path: NodeId, brush: NodeId) -> Option<Refusal> {
    if let Some(r) = apply_brush::pair_refusal(doc, res, path, brush, false) {
        return Some(r);
    }
    if in_instance(doc, brush) {
        return Some(Refusal::BrushInInstance);
    }
    loops(doc, path, brush).then_some(Refusal::Loop)
}

/// Why the selection cannot take a live brush, if it cannot: the pair
/// (`apply_brush::pair`), then [`refusal`] for it.
pub fn selection_refusal(doc: &Document, res: &Resolved, selection: &[NodeId]) -> Option<Refusal> {
    match apply_brush::pair(doc, selection) {
        Ok((path, brush)) => refusal(doc, res, path, brush),
        Err(r) => Some(r),
    }
}

/// The selected layers drawn with a live brush, in selection order.
pub fn brushed_in(doc: &Document, selection: &[NodeId]) -> Vec<NodeId> {
    selection
        .iter()
        .copied()
        .filter(|id| doc.get(*id).is_some_and(|n| n.brush().is_some()))
        .collect()
}

/// A source's brush link as it reads on its copy `copy`: a brush the copy's
/// instance holds a copy of is named by that copy, so each instance draws with
/// its own; a brush outside it stays as named.
pub(crate) fn link_at_copy(doc: &Document, copy: NodeId, link: BrushLink) -> BrushLink {
    let of = crate::component::instance_root(doc, copy)
        .and_then(|root| crate::propagate::counterpart(doc, root, link.of))
        .unwrap_or(link.of);
    BrushLink { of, ..link }
}

/// A copy's brush link in its source's terms, the inverse of [`link_at_copy`]:
/// a brush in the copy's own instance is named by what it is a copy of. Links
/// compare in these terms, so an instance drawing with its own copy of the
/// main's brush overrides nothing.
pub(crate) fn link_at_source(doc: &Document, copy: NodeId, link: BrushLink) -> BrushLink {
    let root = |id| crate::component::instance_root(doc, id);
    let of = doc
        .get(link.of)
        .and_then(|b| b.link())
        .filter(|_| root(link.of).is_some() && root(link.of) == root(copy))
        .unwrap_or(link.of);
    BrushLink { of, ..link }
}

/// `op` with a `SetBrush`'s link read through `at` for the copy `copy`
/// ([`link_at_copy`] or [`link_at_source`]); any other operation as it is.
pub(crate) fn brush_op_at(
    doc: &Document,
    copy: NodeId,
    op: Operation,
    at: fn(&Document, NodeId, BrushLink) -> BrushLink,
) -> Operation {
    match op {
        Operation::SetBrush {
            id,
            brush: Some(link),
        } => Operation::SetBrush {
            id,
            brush: Some(at(doc, copy, link)),
        },
        op => op,
    }
}

/// Whether `id` or one of its ancestors is part of an instance.
fn in_instance(doc: &Document, id: NodeId) -> bool {
    let mut at = doc.get(id);
    while let Some(n) = at {
        if n.link().is_some() {
            return true;
        }
        at = n.parent().and_then(|p| doc.get(p));
    }
    false
}

/// Whether `brush`, or a layer inside it, is drawn through a chain of links by a
/// brush that is `path`: linking the two would close the chain.
fn loops(doc: &Document, path: NodeId, brush: NodeId) -> bool {
    let mut seen = rustc_hash::FxHashSet::default();
    let mut stack = vec![brush];
    while let Some(id) = stack.pop() {
        if id == path {
            return true;
        }
        if !seen.insert(id) {
            continue;
        }
        let Some(n) = doc.get(id) else { continue };
        stack.extend(n.children().iter().copied());
        if let Some(link) = n.brush() {
            stack.push(link.of);
        }
    }
    false
}

/// Whether `id`'s strokes give way to its brush: its link is shown and names a
/// layer the document holds. A brush that draws no ink (flat, or no longer a
/// shape) still takes the strokes' place, so the layer shows it is broken.
pub fn hides_strokes(doc: &Document, id: NodeId) -> bool {
    doc.get(id)
        .and_then(|n| n.brush())
        .is_some_and(|l| l.visible && doc.contains(l.of) && !doc.in_vault(l.of))
}

/// What `id`'s live brush draws, from `world` and `local_path` (the answers of
/// whichever view is drawing: the committed one, or a preview's), or `None` when
/// it draws none.
pub fn ink_with(
    doc: &Document,
    id: NodeId,
    link: BrushLink,
    world: &dyn Fn(NodeId) -> Option<Affine>,
    local_path: &dyn Fn(NodeId) -> Option<BezPath>,
) -> Option<Ink> {
    if !link.visible || link.of == id || !doc.contains(link.of) || doc.in_vault(link.of) {
        return None;
    }
    let brush = doc.get(link.of)?;
    if matches!(brush.kind(), crate::NodeKind::Text { .. }) || brush.paint().image_fill().is_some()
    {
        return None;
    }
    apply_brush::ink(
        world(id)?,
        &local_path(id)?,
        world(link.of)?,
        &local_path(link.of)?,
        link.mode,
    )
    .ok()
}

/// What `id`'s live brush draws as committed, or `None`.
pub fn ink(doc: &Document, res: &Resolved, id: NodeId) -> Option<Ink> {
    let link = doc.get(id)?.brush()?;
    ink_with(doc, id, link, &|n| res.world_transform(n), &|n| {
        res.local_path(doc, n)
    })
}

/// Why `id`'s live brush draws nothing though it takes the strokes' place, for
/// the brush row's amber tooltip; `None` while it draws.
pub fn broken(doc: &Document, res: &Resolved, id: NodeId) -> Option<Refusal> {
    let link = doc.get(id)?.brush()?;
    if !hides_strokes(doc, id) {
        return None;
    }
    if let Some(r) = apply_brush::brush_refusal(doc, res, link.of) {
        return Some(r);
    }
    apply_brush::ink(
        res.world_transform(id)?,
        &res.local_path(doc, id)?,
        res.world_transform(link.of)?,
        &res.local_path(doc, link.of)?,
        link.mode,
    )
    .err()
}

/// `id`'s live brush baked as *Apply brush* bakes it: the `Brushed · ‹brush›`
/// group in its place, the path kept at the group's bottom while a fill of its
/// own shows (its strokes and link dropped, as they drew nothing), deleted
/// otherwise. Answers the edit and the group; a brush that draws nothing
/// answers only the link dropped, the strokes drawing again.
pub fn bake(
    doc: &Document,
    res: &Resolved,
    ids: &mut IdSource,
    id: NodeId,
) -> Option<(Vec<Operation>, Option<NodeId>)> {
    let node = doc.get(id)?;
    let link = node.brush()?;
    let unlink = Operation::SetBrush { id, brush: None };
    if !hides_strokes(doc, id) || ink(doc, res, id).is_none() {
        return Some((vec![unlink], None));
    }
    let how = ApplyBrush {
        path: id,
        brush: link.of,
        mode: link.mode,
        keep_path: true,
    };
    let Ok((tx, group)) = apply_brush::build(doc, res, ids, &how) else {
        return Some((vec![unlink], None));
    };
    let filled = node.paint().fills.iter().any(|f| f.visible);
    let mut ops: Vec<Operation> = Vec::with_capacity(tx.0.len() + 2);
    for op in tx.0 {
        match op {
            // The kept path shows its fill, under its own name.
            Operation::SetVisible { .. } | Operation::SetName { .. } if filled => {}
            Operation::Reparent { .. }
            | Operation::SetVisible { .. }
            | Operation::SetName { .. }
                if !filled => {}
            op => ops.push(op),
        }
    }
    if filled {
        ops.push(unlink);
        ops.push(Operation::SetStrokes {
            id,
            strokes: Vec::new(),
        });
    } else {
        ops.push(Operation::DeleteNode { id });
    }
    Some((ops, Some(group)))
}

/// The bakes `tx` owes: every layer whose brush it deletes, unless the layer
/// goes too, baked before the deletes run, so it keeps drawing what it drew and
/// the document never holds a link to nothing. To be put in front of `tx`.
pub fn baked_for_deletes(
    doc: &Document,
    res: &Resolved,
    tx: &crate::Transaction,
    ids: &mut IdSource,
) -> Vec<Operation> {
    let deleted: Vec<NodeId> =
        tx.0.iter()
            .filter_map(|op| match op {
                Operation::DeleteNode { id } => Some(*id),
                _ => None,
            })
            .collect();
    orphaned(doc, &deleted)
        .into_iter()
        .filter_map(|u| bake(doc, res, ids, u))
        .flat_map(|(ops, _)| ops)
        .collect()
}

/// Beside each of `roots`, a capture of its subtree with every live brush
/// whose brush lies outside it baked, built on a copy of `doc`; `None` for a
/// subtree with none. What the clipboard carries so a paste where the brush is
/// not gets the drawing rather than a link to nothing.
pub fn baked_copies(
    doc: &Document,
    res: &Resolved,
    roots: &[NodeId],
    ids: &mut IdSource,
) -> Vec<Option<Vec<crate::Node>>> {
    roots
        .iter()
        .map(|root| {
            let inside: rustc_hash::FxHashSet<NodeId> = crate::build::subtree_nodes(doc, &[*root])
                .into_iter()
                .collect();
            let outside: Vec<NodeId> = inside
                .iter()
                .copied()
                .filter(|id| {
                    doc.get(*id)
                        .and_then(|n| n.brush())
                        .is_some_and(|l| !inside.contains(&l.of))
                })
                .collect();
            if outside.is_empty() {
                return None;
            }
            let mut ops = Vec::new();
            let mut top = *root;
            for user in crate::build::in_document_order(doc, &outside) {
                let (baked, group) = bake(doc, res, ids, user)?;
                if user == *root
                    && let Some(g) = group
                {
                    top = g;
                }
                ops.extend(baked);
            }
            let mut scratch = doc.clone();
            scratch.apply(&crate::Transaction(ops)).ok()?;
            scratch.capture_subtree(top)
        })
        .collect()
}

/// The actor a [`baked_view`] mints its groups' ids as: a scratch copy's own,
/// never written to the document.
const VIEW_ACTOR: u64 = 0xB4_5EED;

/// `doc` with every live brush under `origins` baked, as a writer that cannot
/// draw one live takes it (SVG and PDF, Penpot, Sketch): a copy of `doc`, its
/// resolve brought up to date from `res`, and `origins` with each brushed one
/// named by the group that took its place. `None` when no brush is under them.
pub fn baked_view(
    doc: &Document,
    res: &Resolved,
    origins: &[NodeId],
) -> Option<(Document, Resolved, Vec<NodeId>)> {
    let users: Vec<NodeId> = res
        .brushed()
        .map(|(user, _)| user)
        .filter(|u| {
            origins
                .iter()
                .any(|o| *o == *u || crate::query::is_within(doc, *u, *o))
        })
        .collect();
    if users.is_empty() {
        return None;
    }
    let mut ids = IdSource::new(VIEW_ACTOR);
    let mut ops = Vec::new();
    let mut took: rustc_hash::FxHashMap<NodeId, NodeId> = Default::default();
    for user in crate::build::in_document_order(doc, &users) {
        if let Some((baked, group)) = bake(doc, res, &mut ids, user) {
            ops.extend(baked);
            if let Some(g) = group {
                took.insert(user, g);
            }
        }
    }
    let mut scratch = doc.clone();
    let out = scratch.apply(&crate::Transaction(ops)).ok()?;
    let mut resolved = res.clone();
    resolved.update(&scratch, &out.dirty);
    let origins = origins
        .iter()
        .map(|o| took.get(o).copied().unwrap_or(*o))
        .collect();
    Some((scratch, resolved, origins))
}

/// `subtrees` as a paste into `doc` takes them: each holding a live brush
/// whose brush neither came along nor is in `doc` swapped for its twin in
/// `baked`. Answers whether any was.
pub fn fitted(
    doc: &Document,
    subtrees: &[Vec<crate::Node>],
    baked: &[Option<Vec<crate::Node>>],
) -> (Vec<Vec<crate::Node>>, bool) {
    let mut swapped = false;
    let out = subtrees
        .iter()
        .enumerate()
        .map(|(i, nodes)| {
            let dangles = nodes.iter().any(|n| {
                n.brush()
                    .is_some_and(|l| !nodes.iter().any(|m| m.id() == l.of) && !doc.contains(l.of))
            });
            match baked.get(i).and_then(Option::as_ref) {
                Some(twin) if dangles => {
                    swapped = true;
                    twin.clone()
                }
                _ => nodes.clone(),
            }
        })
        .collect();
    (out, swapped)
}

/// Every layer whose brush is among `deleted` or inside one of them, the layer
/// itself not, in document order: what deleting them bakes.
pub fn orphaned(doc: &Document, deleted: &[NodeId]) -> Vec<NodeId> {
    if deleted.is_empty() || !doc.nodes_iter().any(|n| n.brush().is_some()) {
        return Vec::new();
    }
    let gone: rustc_hash::FxHashSet<NodeId> = crate::build::subtree_nodes(doc, deleted)
        .into_iter()
        .collect();
    let users: Vec<NodeId> = doc
        .nodes_iter()
        .filter(|n| {
            !gone.contains(&n.id())
                && !doc.in_vault(n.id())
                && n.brush().is_some_and(|l| gone.contains(&l.of))
        })
        .map(|n| n.id())
        .collect();
    crate::build::in_document_order(doc, &users)
}
