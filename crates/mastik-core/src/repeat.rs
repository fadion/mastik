//! *Repeat*: copies of a selection turned step by step about one point, stepped
//! along a line or tiled in a grid, grouped with the original as one drawing.
//!
//! The copies are plain copies of the originals as they stand, so a main's copies
//! land as its instances (`component::settle_copy`), which is all *Linked* asks.

use crate::build;
use crate::kurbo::{Affine, Point, Rect, Vec2};
use crate::{Document, IdSource, NodeId, OpError, Operation, Resolved, Transaction};
use serde::Deserialize;

/// The most copies a repeat makes, the original counted: past this a ring is a
/// texture, and the preview would stall.
pub const MAX_COUNT: usize = 360;

/// The gap a line or a grid starts at, between one box and the next.
pub const DEFAULT_GAP: f64 = 16.0;

/// Copies stepped along a line, each `gap` past the box before it.
#[derive(Clone, Copy, Debug, PartialEq, Deserialize)]
pub struct Line {
    /// How many there are, the original counted.
    pub count: usize,
    /// Between one box and the next, not between origins; negative overlaps.
    #[serde(default = "default_gap")]
    pub gap: f64,
    /// Down the page rather than across it.
    #[serde(default)]
    pub down: bool,
}

/// Copies tiled in rows, the original top-left, `gap` across then down between
/// the boxes.
#[derive(Clone, Copy, Debug, PartialEq, Deserialize)]
pub struct Grid {
    pub cols: usize,
    pub rows: usize,
    #[serde(default = "default_gaps")]
    pub gap: [f64; 2],
}

fn default_gap() -> f64 {
    DEFAULT_GAP
}

fn default_gaps() -> [f64; 2] {
    [DEFAULT_GAP; 2]
}

/// How the copies are laid out.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Pattern {
    /// Turned about `pivot`, in world space.
    Ring {
        ring: Ring,
        pivot: Point,
    },
    Line(Line),
    Grid(Grid),
}

impl Pattern {
    /// How many there are, the original counted.
    pub fn count(&self) -> usize {
        match self {
            Pattern::Ring { ring, .. } => ring.count,
            Pattern::Line(line) => line.count,
            Pattern::Grid(grid) => grid.cols.saturating_mul(grid.rows),
        }
    }

    /// Whether the count is one a repeat makes: 2 to [`MAX_COUNT`], and a grid
    /// at least one column and one row.
    pub fn is_valid(&self) -> bool {
        let grid_ok = match self {
            Pattern::Grid(g) => g.cols >= 1 && g.rows >= 1,
            _ => true,
        };
        grid_ok && (2..=MAX_COUNT).contains(&self.count())
    }

    /// The world-space affine that carries an original to copy `k`, for a
    /// selection whose world box is `bounds`.
    pub fn carry(&self, k: usize, bounds: Rect) -> Affine {
        let (w, h) = (bounds.width(), bounds.height());
        match *self {
            Pattern::Ring { ring, pivot } => ring.carry(k, pivot, bounds.center()),
            Pattern::Line(line) => {
                let step = k as f64;
                Affine::translate(match line.down {
                    false => Vec2::new(step * (w + line.gap), 0.0),
                    true => Vec2::new(0.0, step * (h + line.gap)),
                })
            }
            Pattern::Grid(grid) => {
                let cols = grid.cols.max(1);
                let (col, row) = ((k % cols) as f64, (k / cols) as f64);
                Affine::translate((col * (w + grid.gap[0]), row * (h + grid.gap[1])))
            }
        }
    }

    /// The name the group takes: *Tick × 12*, *Tile 4 × 3* for a grid.
    pub fn group_name(&self, first: &str) -> String {
        match self {
            Pattern::Grid(g) => format!("{first} {} × {}", g.cols, g.rows),
            _ => group_name(first, self.count()),
        }
    }
}

/// How the copies are laid round the point.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Ring {
    /// How many there are, the original counted: 12 is a full clock face.
    pub count: usize,
    /// The turn from one to the next, in radians, clockwise on screen.
    pub step: f64,
    /// Whether each copy is turned with its step, as rays are; off, the copies
    /// keep the original's orientation and only travel round.
    pub turn: bool,
}

impl Ring {
    /// A full turn split evenly between `count`.
    pub fn full(count: usize, turn: bool) -> Self {
        Self {
            count,
            step: std::f64::consts::TAU / count.max(1) as f64,
            turn,
        }
    }

    /// The world-space affine that carries an original to copy `k` (1 is the next
    /// one round), about `pivot`. `centre` is what travels when the copies are not
    /// turned: the middle of the selection's box.
    pub fn carry(&self, k: usize, pivot: Point, centre: Point) -> Affine {
        let turn = Affine::translate(pivot.to_vec2())
            * Affine::rotate(self.step * k as f64)
            * Affine::translate(-pivot.to_vec2());
        match self.turn {
            true => turn,
            false => Affine::translate((turn * centre) - centre),
        }
    }
}

/// Where the copies go.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Place {
    /// A new group that takes the originals, named for the first and the count:
    /// the commit.
    Group,
    /// Beside each original, under it, in its own parent — and at `fade` of its
    /// opacity: the preview, which must not reparent what is on the canvas.
    Beside { fade: u8 },
}

/// The middle of the box `members` occupy, in world space.
pub fn centre(res: &Resolved, members: &[NodeId]) -> Option<Point> {
    members
        .iter()
        .filter_map(|id| res.world_bounds(*id))
        .reduce(|a, b| a.union(b))
        .map(|b: Rect| b.center())
}

/// The three ways a repeat lays its copies out.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Way {
    #[default]
    Around,
    Line,
    Grid,
}

impl Pattern {
    /// Which way the pattern lays its copies out.
    pub fn way(&self) -> Way {
        match self {
            Pattern::Ring { .. } => Way::Around,
            Pattern::Line(_) => Way::Line,
            Pattern::Grid(_) => Way::Grid,
        }
    }
}

/// Why a ring is refused for a layer in an auto layout.
pub const IN_LAYOUT: &str = "Repeat can't work inside an auto layout";

/// Why a grid is refused for items of a flex frame.
pub const GRID_IN_LINE: &str = "Auto layout places these in one line";

/// The frame that lays `members` out as its own items, when they all are one's;
/// `None` when they stand free, or reach a layout through a group.
pub fn flow_parent(doc: &Document, members: &[NodeId]) -> Option<NodeId> {
    let view = crate::resolve::DocView(doc);
    let parent = doc.get(*members.first()?)?.parent()?;
    members
        .iter()
        .all(|id| {
            doc.get(*id).and_then(|n| n.parent()) == Some(parent)
                && crate::container::parent_lays_out(&view, *id)
        })
        .then_some(parent)
}

/// Why `way` cannot repeat `members` where they stand, or `None`. A ring is
/// refused anywhere an auto layout would move it ([`in_layout`]); a line in a
/// layout's own items lets the layout space the copies, and so does a grid in a
/// grid frame, but not in a flex one, which lays them in one line.
pub fn layout_refusal(doc: &Document, members: &[NodeId], way: Way) -> Option<&'static str> {
    if !in_layout(doc, members) {
        return None;
    }
    let display = flow_parent(doc, members)
        .and_then(|p| doc.get(p))
        .and_then(|p| p.display());
    match (way, display) {
        (Way::Around, _) | (_, None) => Some(IN_LAYOUT),
        (Way::Line, Some(_)) | (Way::Grid, Some(crate::container::Display::Grid(_))) => None,
        (Way::Grid, Some(crate::container::Display::Flex(_))) => Some(GRID_IN_LINE),
    }
}

/// The gap a line or grid of `members` starts at: their auto layout parent's,
/// across and down, where it has one, else [`DEFAULT_GAP`].
pub fn starting_gap(doc: &Document, members: &[NodeId]) -> [f64; 2] {
    use crate::container::Display;
    let parent = members
        .first()
        .and_then(|id| doc.get(*id))
        .and_then(|n| n.parent())
        .and_then(|p| doc.get(p));
    let gaps = match parent.and_then(|p| p.display()) {
        Some(Display::Flex(f)) => [f.column_gap, f.row_gap],
        Some(Display::Grid(g)) => [g.column_gap, g.row_gap],
        None => [0.0; 2],
    };
    if gaps.iter().all(|g| *g == 0.0) {
        [DEFAULT_GAP; 2]
    } else {
        gaps
    }
}

/// Whether any of `members` sits in a parent that lays its children out, or in
/// plain groups inside one. There the ring's group, or the copies beside the
/// original, would be laid out as items or grow the group that is one: the
/// original and its siblings would move and the ring would not stand round its
/// point. Refused, as the shape builder and the knife refuse such a layer.
pub fn in_layout(doc: &Document, members: &[NodeId]) -> bool {
    let view = crate::resolve::DocView(doc);
    members
        .iter()
        .any(|id| crate::container::lays_out_above(&view, *id))
}

/// The name the group takes: *Tick × 12*.
pub fn group_name(first: &str, count: usize) -> String {
    format!("{first} × {count}")
}

/// Repeat `selection` as `pattern` says.
///
/// Answers the edit and what to select after it: under [`Place::Group`] the group
/// made, or in an auto layout the originals and their copies. The members must
/// share a parent to be grouped, one that lays nothing out ([`in_layout`]); the
/// originals stay first in the group, the copies below them in order. Where
/// [`layout_refusal`] lets a line or grid into an auto layout, the copies go in
/// as its items right after the last original, and the layout places them.
pub fn repeat(
    doc: &Document,
    res: &Resolved,
    ids: &mut IdSource,
    selection: &[NodeId],
    pattern: Pattern,
    into: Place,
) -> Result<(Transaction, Vec<NodeId>), OpError> {
    let members = build::in_document_order(doc, selection);
    if members.is_empty() || !pattern.is_valid() {
        return Err(OpError::MalformedSubtree);
    }
    if layout_refusal(doc, &members, pattern.way()).is_some() {
        return Err(OpError::InvalidParent);
    }
    if in_layout(doc, &members) {
        return flowed(doc, ids, &members, pattern.count(), into);
    }
    let bounds = members
        .iter()
        .filter_map(|id| res.world_bounds(*id))
        .reduce(|a, b| a.union(b))
        .ok_or(OpError::EmptyGeometry)?;
    let (mut ops, group) = match into {
        Place::Group => {
            let (tx, group) = build::group(doc, res, ids, &members)?;
            let top = *members.last().expect("checked non-empty");
            let first = doc
                .get(top)
                .map(|n| n.name().to_string())
                .unwrap_or_default();
            let mut ops = tx.0;
            ops.push(Operation::SetName {
                id: group,
                name: pattern.group_name(&first),
            });
            (ops, Some(group))
        }
        Place::Beside { .. } => (Vec::new(), None),
    };
    // Each step's block goes in under the one before it, so the layers read the
    // original, then the copies in order.
    for k in 1..pattern.count() {
        let carry = pattern.carry(k, bounds);
        for (i, &m) in members.iter().enumerate() {
            let Some((mut nodes, root)) = copy_of(doc, ids, m) else {
                continue;
            };
            let world = res.world_transform(m).ok_or(OpError::NoSuchNode(m))?;
            let transform = build::local_for_world(doc, res, m, carry * world);
            let (parent, index) = match (into, group) {
                (Place::Group, Some(g)) => (g, i),
                _ => {
                    let node = doc.get(m).ok_or(OpError::NoSuchNode(m))?;
                    let parent = node.parent().ok_or(OpError::CannotModifyRoot)?;
                    let at = doc
                        .get(parent)
                        .and_then(|p| p.children().iter().position(|c| *c == m))
                        .unwrap_or(0);
                    (parent, at)
                }
            };
            if let (Place::Beside { fade }, Some(n)) =
                (into, nodes.iter_mut().find(|n| n.id == root))
            {
                n.opacity = doc.drawn_opacity(m) * f32::from(fade) / 255.0;
                n.opacity_var = None;
            }
            ops.push(Operation::InsertSubtree {
                nodes,
                parent,
                index,
            });
            ops.push(Operation::SetTransform {
                id: root,
                transform,
            });
        }
    }
    Ok((Transaction(ops), group.into_iter().collect()))
}

/// A fresh copy of `m`'s subtree, a main's copy settled as its instance, and the
/// copy's root.
fn copy_of(doc: &Document, ids: &mut IdSource, m: NodeId) -> Option<(Vec<crate::Node>, NodeId)> {
    let template = doc.capture_subtree(m)?;
    let (mut nodes, root) = crate::document::remap_subtree(&template, ids)?;
    crate::component::settle_copy(
        doc,
        &template,
        &mut nodes,
        crate::component::MainCopy::Instance,
    );
    Some((nodes, root))
}

/// `count` of `members`, the originals counted, as items of the auto layout that
/// holds them: each step's block right after the one before, the first right
/// after the last original, so the layout reads the originals then the copies in
/// order. A copy's own grid lines and area are cleared, so a grid flows it into
/// the next free cell rather than on top of its original.
fn flowed(
    doc: &Document,
    ids: &mut IdSource,
    members: &[NodeId],
    count: usize,
    into: Place,
) -> Result<(Transaction, Vec<NodeId>), OpError> {
    let parent = flow_parent(doc, members).ok_or(OpError::InvalidParent)?;
    let siblings = doc
        .get(parent)
        .ok_or(OpError::NoSuchNode(parent))?
        .children();
    let last = members
        .iter()
        .filter_map(|m| siblings.iter().position(|c| c == m))
        .max()
        .ok_or(OpError::MalformedSubtree)?;
    let (mut ops, mut picked) = (Vec::new(), members.to_vec());
    let mut index = last + 1;
    for _ in 1..count {
        for &m in members {
            let Some((mut nodes, root)) = copy_of(doc, ids, m) else {
                continue;
            };
            if let Some(n) = nodes.iter_mut().find(|n| n.id == root) {
                n.item.grid_column = Default::default();
                n.item.grid_row = Default::default();
                n.item.grid_area = None;
                if let Place::Beside { fade } = into {
                    n.opacity = doc.drawn_opacity(m) * f32::from(fade) / 255.0;
                    n.opacity_var = None;
                }
            }
            ops.push(Operation::InsertSubtree {
                nodes,
                parent,
                index,
            });
            picked.push(root);
            index += 1;
        }
    }
    Ok((Transaction(ops), picked))
}

/// Why a *Linked* ring cannot make its original a component first.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum NotLinked {
    /// Linked copies repeat one layer at a time.
    Several,
    /// It is a main or an instance: its copies are linked already.
    Linked,
    /// *Create component*'s rule (`component::why_not_make`).
    Not(crate::component::NotMain),
}

/// Why a linked ring of the outermost layers `roots` would refuse, or `None`.
pub fn why_not_linked(doc: &Document, roots: &[NodeId]) -> Option<NotLinked> {
    let [one] = roots else {
        return Some(NotLinked::Several);
    };
    let node = doc.get(*one)?;
    if node.component() || crate::component::instance_root(doc, *one) == Some(*one) {
        return Some(NotLinked::Linked);
    }
    crate::component::why_not_make(doc, roots).map(NotLinked::Not)
}

/// A *Linked* repeat of `id`: the layer made a component where it stands (a frame
/// or a group), or wrapped in a group made the component, then repeated, so its
/// copies land as instances. A group rather than *Create component*'s frame: the
/// wrapper only carries the link, and a frame would add a clip and a fill nobody
/// asked for. The wrapper takes the layer's name, so the main, its instances and
/// the group read as the layer. Worked on a scratch copy so both halves are one edit.
pub fn linked(
    doc: &Document,
    res: &Resolved,
    ids: &mut IdSource,
    id: NodeId,
    pattern: Pattern,
) -> Result<(Transaction, Vec<NodeId>), OpError> {
    if !pattern.is_valid() {
        return Err(OpError::MalformedSubtree);
    }
    if layout_refusal(doc, &[id], pattern.way()).is_some() {
        return Err(OpError::InvalidParent);
    }
    let (mut tx, main) = match crate::component::makes_in_place(doc, id) {
        true => (Transaction(Vec::new()), id),
        false => {
            let (mut tx, group) = build::group(doc, res, ids, &[id])?;
            if let Some(node) = doc.get(id) {
                tx.0.push(Operation::SetName {
                    id: group,
                    name: node.name().to_string(),
                });
            }
            (tx, group)
        }
    };
    tx.0.push(Operation::SetComponent {
        id: main,
        component: true,
    });
    let mut after = doc.clone();
    let outcome = after.apply(&tx)?;
    let mut laid = res.clone();
    laid.update(&after, &outcome.dirty);
    let (more, picked) = repeat(&after, &laid, ids, &[main], pattern, Place::Group)?;
    tx.0.extend(more.0);
    Ok((tx, picked))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::NodeKind;
    use crate::container::{Display, Flex};
    use crate::kurbo::Size;

    /// A layer in a row frame is refused, as a ring, a preview or a linked ring,
    /// so neither it nor its sibling moves; out of the row it repeats.
    /// flip: `repeat` without its `in_layout` refusal → fails at "the ring is refused".
    #[test]
    fn a_layer_in_an_auto_layout_is_refused() {
        let mut ids = IdSource::new(0x2E9);
        let root = ids.mint();
        let (frame, a, b) = (ids.mint(), ids.mint(), ids.mint());
        let mut doc = Document::new(root);
        let rect = |id, index| Operation::CreateNode {
            id,
            parent: frame,
            index,
            kind: NodeKind::Rect {
                size: Size::new(40.0, 30.0),
                corner_radii: Default::default(),
                corner_smoothing: 0.0,
            },
            transform: None,
            name: None,
        };
        doc.apply(&Transaction(vec![
            Operation::CreateNode {
                id: frame,
                parent: root,
                index: 0,
                kind: NodeKind::Artboard {
                    size: Size::new(300.0, 300.0),
                    corner_radii: Default::default(),
                    corner_smoothing: 0.0,
                },
                transform: None,
                name: None,
            },
            rect(a, 0),
            rect(b, 1),
            Operation::SetDisplay {
                id: frame,
                display: Some(Display::Flex(Flex::default())),
            },
        ]))
        .expect("the fixture");
        let res = Resolved::rebuild(&doc);
        assert!(in_layout(&doc, &[a]));
        let ring = Pattern::Ring {
            ring: Ring::full(4, true),
            pivot: Point::new(40.0, 120.0),
        };
        for into in [Place::Group, Place::Beside { fade: 115 }] {
            let made = repeat(&doc, &res, &mut ids, &[a], ring, into);
            assert!(
                matches!(made, Err(OpError::InvalidParent)),
                "the ring is refused"
            );
        }
        assert!(linked(&doc, &res, &mut ids, a, ring).is_err());

        doc.apply(&Transaction(vec![Operation::SetDisplay {
            id: frame,
            display: None,
        }]))
        .unwrap();
        let res = Resolved::rebuild(&doc);
        assert!(repeat(&doc, &res, &mut ids, &[a], ring, Place::Group).is_ok());
    }

    /// A layer in a plain group in a row is refused too: the ring would grow the
    /// group, the row's item, and the row would move it and its siblings.
    /// flip: `in_layout` asking only the direct parent → fails at "in a layout".
    #[test]
    fn a_layer_in_a_group_in_an_auto_layout_is_refused() {
        let mut ids = IdSource::new(0x2EA);
        let root = ids.mint();
        let (frame, g, r, b) = (ids.mint(), ids.mint(), ids.mint(), ids.mint());
        let mut doc = Document::new(root);
        let node = |id, parent, index, kind| Operation::CreateNode {
            id,
            parent,
            index,
            kind,
            transform: None,
            name: None,
        };
        let rect = NodeKind::Rect {
            size: Size::new(40.0, 30.0),
            corner_radii: Default::default(),
            corner_smoothing: 0.0,
        };
        let board = NodeKind::Artboard {
            size: Size::new(300.0, 300.0),
            corner_radii: Default::default(),
            corner_smoothing: 0.0,
        };
        doc.apply(&Transaction(vec![
            node(frame, root, 0, board),
            node(g, frame, 0, NodeKind::Group),
            node(r, g, 0, rect.clone()),
            node(b, frame, 1, rect),
            Operation::SetDisplay {
                id: frame,
                display: Some(Display::Flex(Flex::default())),
            },
        ]))
        .expect("the fixture");
        let res = Resolved::rebuild(&doc);
        assert!(in_layout(&doc, &[r]), "in a layout");
        for pattern in [
            Pattern::Ring {
                ring: Ring::full(4, true),
                pivot: Point::new(20.0, 120.0),
            },
            Pattern::Line(Line {
                count: 3,
                gap: 8.0,
                down: false,
            }),
        ] {
            let made = repeat(&doc, &res, &mut ids, &[r], pattern, Place::Group);
            assert!(matches!(made, Err(OpError::InvalidParent)), "{pattern:?}");
        }
    }

    /// A free-standing layer and its world box, for line and grid tests: a
    /// 40 × 30 rectangle named *Card* at (10, 20) on a plain frame, or on a frame
    /// with `display`, beside a second item.
    fn card_doc(display: Option<Display>) -> (Document, IdSource, NodeId, NodeId) {
        let mut ids = IdSource::new(0x2EB);
        let root = ids.mint();
        let (frame, a, b) = (ids.mint(), ids.mint(), ids.mint());
        let mut doc = Document::new(root);
        let rect = |id, index, name: &str| Operation::CreateNode {
            id,
            parent: frame,
            index,
            kind: NodeKind::Rect {
                size: Size::new(40.0, 30.0),
                corner_radii: Default::default(),
                corner_smoothing: 0.0,
            },
            transform: Some(Affine::translate((10.0, 20.0))),
            name: Some(name.into()),
        };
        doc.apply(&Transaction(vec![
            Operation::CreateNode {
                id: frame,
                parent: root,
                index: 0,
                kind: NodeKind::Artboard {
                    size: Size::new(400.0, 300.0),
                    corner_radii: Default::default(),
                    corner_smoothing: 0.0,
                },
                transform: None,
                name: None,
            },
            rect(a, 0, "Card"),
            rect(b, 1, "Other"),
            Operation::SetDisplay { id: frame, display },
        ]))
        .expect("the fixture");
        (doc, ids, frame, a)
    }

    fn boxes(doc: &Document, ids: &[NodeId]) -> Vec<Rect> {
        let res = Resolved::rebuild(doc);
        ids.iter()
            .map(|id| res.world_bounds(*id).unwrap())
            .collect()
    }

    /// A line of four steps each box 12 past the one before, across or down,
    /// grouped as *Card × 4* with the original first; a negative gap overlaps.
    ///
    /// flip: `Pattern::carry` stepping by the gap alone (origins, not boxes) →
    /// fails at "box to box".
    #[test]
    fn a_line_steps_box_to_box_by_the_gap() {
        for (down, gap) in [(false, 12.0), (true, 12.0), (false, -10.0)] {
            let (doc, mut ids, frame, a) = card_doc(None);
            let res = Resolved::rebuild(&doc);
            let line = Pattern::Line(Line {
                count: 4,
                gap,
                down,
            });
            let (tx, picked) =
                repeat(&doc, &res, &mut ids, &[a], line, Place::Group).expect("repeated");
            let mut after = doc.clone();
            after.apply(&tx).unwrap();
            let [group] = picked[..] else {
                panic!("one group: {picked:?}")
            };
            let g = after.get(group).unwrap();
            assert_eq!(g.name(), "Card × 4");
            assert_eq!(g.parent(), Some(frame));
            let kids = g.children().to_vec();
            assert_eq!(kids.last(), Some(&a), "the original first in the layers");
            // Layers read top down: the original, then the copies in order.
            let order: Vec<NodeId> = kids.iter().rev().copied().collect();
            let b = boxes(&after, &order);
            for pair in b.windows(2) {
                let step = match down {
                    false => (pair[1].x0 - pair[0].x1, pair[1].y0 - pair[0].y0),
                    true => (pair[1].y0 - pair[0].y1, pair[1].x0 - pair[0].x0),
                };
                assert!(
                    (step.0 - gap).abs() < 1e-9 && step.1.abs() < 1e-9,
                    "box to box: {step:?}"
                );
            }
        }
    }

    /// A 3 × 2 grid tiles six, the original top-left, columns 8 apart and rows
    /// 20, named *Card 3 × 2*.
    #[test]
    fn a_grid_tiles_rows_from_the_top_left() {
        let (doc, mut ids, _, a) = card_doc(None);
        let res = Resolved::rebuild(&doc);
        let grid = Pattern::Grid(Grid {
            cols: 3,
            rows: 2,
            gap: [8.0, 20.0],
        });
        let (tx, picked) = repeat(&doc, &res, &mut ids, &[a], grid, Place::Group).unwrap();
        let mut after = doc.clone();
        after.apply(&tx).unwrap();
        let g = after.get(picked[0]).unwrap();
        assert_eq!(g.name(), "Card 3 × 2");
        let order: Vec<NodeId> = g.children().iter().rev().copied().collect();
        assert_eq!(order.len(), 6);
        assert_eq!(order[0], a);
        let b = boxes(&after, &order);
        let at = |i: usize| (b[i].x0, b[i].y0);
        assert_eq!(at(0), (10.0, 20.0));
        assert_eq!(at(2), (10.0 + 2.0 * 48.0, 20.0));
        assert_eq!(at(3), (10.0, 20.0 + 50.0));
        assert_eq!(at(5), (10.0 + 2.0 * 48.0, 70.0));
        assert!(
            !Pattern::Grid(Grid {
                cols: 1,
                rows: 1,
                gap: [0.0; 2]
            })
            .is_valid()
        );
    }

    /// In a row frame a line's copies go in right after the original as the
    /// row's items, no group, and are selected with it; a grid is refused there
    /// and a ring through a group too. In a grid frame a grid's copies flow into
    /// the next cells, their own lines cleared.
    ///
    /// flip: `flowed` inserting at the end of the parent → fails at "right after
    /// the original"; `flowed` keeping the copy's grid lines → fails at "cleared".
    #[test]
    fn in_an_auto_layout_the_copies_are_the_layouts_items() {
        let (doc, mut ids, frame, a) = card_doc(Some(Display::Flex(Flex::default())));
        let res = Resolved::rebuild(&doc);
        let other = doc.get(frame).unwrap().children()[1];
        let line = Pattern::Line(Line {
            count: 3,
            gap: 0.0,
            down: false,
        });
        assert_eq!(layout_refusal(&doc, &[a], Way::Line), None);
        assert_eq!(layout_refusal(&doc, &[a], Way::Grid), Some(GRID_IN_LINE));
        assert_eq!(layout_refusal(&doc, &[a], Way::Around), Some(IN_LAYOUT));
        let (tx, picked) = repeat(&doc, &res, &mut ids, &[a], line, Place::Group).unwrap();
        let mut after = doc.clone();
        after.apply(&tx).unwrap();
        let kids = after.get(frame).unwrap().children().to_vec();
        assert_eq!(kids.len(), 4);
        assert_eq!(kids[0], a);
        assert_eq!(kids[3], other, "right after the original");
        assert_eq!(picked, kids[..3].to_vec());

        let placed = crate::container::Grid {
            column_gap: 6.0,
            ..Default::default()
        };
        let (mut doc, mut ids, frame, a) = card_doc(Some(Display::Grid(placed)));
        let mut item = *doc.get(a).unwrap().item();
        item.grid_column.start = crate::container::GridPlacement::Line(1);
        doc.apply(&Transaction(vec![Operation::SetLayoutItem { id: a, item }]))
            .unwrap();
        assert_eq!(starting_gap(&doc, &[a]), [6.0, 0.0]);
        let res = Resolved::rebuild(&doc);
        let grid = Pattern::Grid(Grid {
            cols: 2,
            rows: 2,
            gap: [0.0; 2],
        });
        let (tx, _) = repeat(&doc, &res, &mut ids, &[a], grid, Place::Group).unwrap();
        let mut after = doc.clone();
        after.apply(&tx).unwrap();
        let kids = after.get(frame).unwrap().children().to_vec();
        assert_eq!(kids.len(), 5);
        for copy in &kids[1..4] {
            assert_eq!(
                after.get(*copy).unwrap().item().grid_column,
                Default::default(),
                "cleared"
            );
        }
    }
}
