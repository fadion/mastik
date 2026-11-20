//! A played copy's variable values (`Variables::set_played`): they beat every
//! mode, through an alias and on a kit's variable, and never reach the file.

use mastik_core::kurbo::{RoundedRectRadii, Size};
use mastik_core::peniko::Color;
use mastik_core::variable::{CollectionId, KitMode, KitModes, VarOrigin};
use mastik_core::{
    BrushSource, Cell, Document, Fill, IdSource, Mode, ModeId, NodeId, NodeKind, Operation,
    Resolved, Transaction, Value, VarId, VarKind, Variable, io, keyed_by_position,
};
use std::collections::BTreeMap;

const WHITE: Color = Color::from_rgba8(255, 255, 255, 255);
const NAVY: Color = Color::from_rgba8(10, 20, 80, 255);
const RED: Color = Color::from_rgba8(200, 30, 30, 255);
const KIT: &str = "5eed0000000000000000000000000022";

struct F {
    doc: Document,
    /// Set to the dark mode; `light_frame` reads the default, light.
    dark_frame: NodeId,
    light_frame: NodeId,
    /// A rectangle in the dark frame whose fill is bound to `alias`.
    rect: NodeId,
    /// White in Light, navy in Dark.
    brand: VarId,
    /// Aliases `brand` in both modes.
    alias: VarId,
    /// A kit's number, 3 and 9.
    kit_radius: VarId,
}

fn var(id: VarId, name: &str, kind: VarKind, cells: Vec<(ModeId, Cell)>) -> Variable {
    Variable {
        id,
        name: name.into(),
        kind,
        cells: cells.into_iter().collect(),
        scope: None,
        origin: None,
        forked: None,
        description: String::new(),
    }
}

fn frame(id: NodeId, parent: NodeId, index: usize) -> Operation {
    Operation::CreateNode {
        id,
        parent,
        index,
        kind: NodeKind::Artboard {
            size: Size::new(100.0, 100.0),
            corner_radii: RoundedRectRadii::default(),
            corner_smoothing: 0.0,
        },
        transform: None,
        name: None,
    }
}

fn fixture() -> F {
    let mut ids = IdSource::new(0xC4);
    let root = ids.mint();
    let mut doc = Document::new(root);
    let (dark_frame, light_frame, rect) = (ids.mint(), ids.mint(), ids.mint());
    let (light, dark) = (ids.mint_mode(), ids.mint_mode());
    let (brand, alias, kit_radius) = (ids.mint_var(), ids.mint_var(), ids.mint_var());
    let (k_light, k_night) = (ids.mint_mode(), ids.mint_mode());
    let kit_mode = |id, name: &str| KitMode {
        id,
        name: name.into(),
        from: id,
        collection: String::new(),
    };
    let mut kit_var = var(
        kit_radius,
        "kit/radius",
        VarKind::Number,
        vec![(k_light, Cell::Number(3.0)), (k_night, Cell::Number(9.0))],
    );
    kit_var.origin = Some(VarOrigin {
        kit: KIT.into(),
        var: kit_radius,
        version: 1,
    });
    doc.apply(&Transaction(vec![
        frame(dark_frame, root, 0),
        frame(light_frame, root, 1),
        Operation::CreateNode {
            id: rect,
            parent: dark_frame,
            index: 0,
            kind: NodeKind::Rect {
                size: Size::new(40.0, 20.0),
                corner_radii: RoundedRectRadii::default(),
                corner_smoothing: 0.0,
            },
            transform: None,
            name: None,
        },
        Operation::AddMode {
            collection: CollectionId::FIRST,
            mode: Mode {
                id: light,
                name: "Light".into(),
            },
            index: 0,
            cells: vec![],
        },
        Operation::AddMode {
            collection: CollectionId::FIRST,
            mode: Mode {
                id: dark,
                name: "Dark".into(),
            },
            index: 1,
            cells: vec![],
        },
        Operation::AddVariable {
            var: var(
                brand,
                "brand",
                VarKind::Color,
                vec![(light, Cell::Color(WHITE)), (dark, Cell::Color(NAVY))],
            ),
        },
        Operation::AddVariable {
            var: var(
                alias,
                "surface",
                VarKind::Color,
                vec![(light, Cell::Alias(brand)), (dark, Cell::Alias(brand))],
            ),
        },
        Operation::SetKitModes {
            kit: KIT.into(),
            modes: Some(KitModes {
                name: None,
                modes: vec![kit_mode(k_light, "Light"), kit_mode(k_night, "Night")],
                map: Default::default(),
            }),
        },
        Operation::AddVariable { var: kit_var },
        Operation::SetModes {
            id: dark_frame,
            modes: vec![dark],
        },
        Operation::SetFills {
            id: rect,
            fills: keyed_by_position([Fill {
                brush: BrushSource::Bound {
                    var: alias,
                    alpha: 1.0,
                },
                visible: true,
            }]),
        },
    ]))
    .expect("two frames in two modes, an alias and a kit variable");
    F {
        doc,
        dark_frame,
        light_frame,
        rect,
        brand,
        alias,
        kit_radius,
    }
}

fn played(pairs: impl IntoIterator<Item = (VarId, Value)>) -> BTreeMap<VarId, Value> {
    pairs.into_iter().collect()
}

/// A play value is what the variable reads in every mode, and an alias to it
/// reads it too.
///
/// flip: `Variables::resolve_at` without its play-value read → fails at "dark
/// frame reads it".
#[test]
fn a_play_value_beats_every_mode_and_reaches_through_an_alias() {
    let mut f = fixture();
    assert_eq!(
        f.doc.resolve_var(f.dark_frame, f.brand),
        Some(Value::Color(NAVY)),
        "the mode's value before play"
    );
    f.doc
        .set_play_values(played([(f.brand, Value::Color(RED))]));
    assert_eq!(
        f.doc.resolve_var(f.dark_frame, f.brand),
        Some(Value::Color(RED)),
        "dark frame reads it"
    );
    assert_eq!(
        f.doc.resolve_var(f.light_frame, f.brand),
        Some(Value::Color(RED)),
        "light frame reads it"
    );
    assert_eq!(
        f.doc.resolve_var(f.light_frame, f.alias),
        Some(Value::Color(RED)),
        "through the alias"
    );
}

/// A play value on the alias itself beats the variable it aliases.
///
/// flip: `resolve_at` reading the play value after following the alias (only on
/// the walk's last step) → fails at "the alias's own value".
#[test]
fn a_play_value_on_an_alias_wins_over_its_target() {
    let mut f = fixture();
    f.doc.set_play_values(played([
        (f.alias, Value::Color(RED)),
        (f.brand, Value::Color(WHITE)),
    ]));
    assert_eq!(
        f.doc.resolve_var(f.dark_frame, f.alias),
        Some(Value::Color(RED)),
        "the alias's own value"
    );
}

/// A kit's variable takes a play value as the document's own do.
///
/// flip: `resolve_at` checking play values only for variables with no kit →
/// fails at "kit variable".
#[test]
fn a_kit_variable_takes_a_play_value() {
    let mut f = fixture();
    assert_eq!(
        f.doc.resolve_var(f.dark_frame, f.kit_radius),
        Some(Value::Number(3.0)),
        "the kit's default before play"
    );
    f.doc
        .set_play_values(played([(f.kit_radius, Value::Number(20.0))]));
    assert_eq!(
        f.doc.resolve_var(f.dark_frame, f.kit_radius),
        Some(Value::Number(20.0)),
        "kit variable"
    );
}

/// A value of the wrong kind, or for a variable that is not there, is left out.
///
/// flip: `Variables::set_played` keeping every pair → fails at "wrong kind".
#[test]
fn a_play_value_of_the_wrong_kind_is_left_out() {
    let mut f = fixture();
    let ghost = VarId(NodeId { actor: 7, seq: 7 });
    f.doc.set_play_values(played([
        (f.brand, Value::Number(1.0)),
        (ghost, Value::Number(1.0)),
        (f.kit_radius, Value::Number(f64::NAN)),
    ]));
    assert!(f.doc.variables().played().is_empty(), "wrong kind");
}

/// The bound fill draws the play value once `Resolved` updates the layers the
/// call answers, and the file never carries it.
///
/// flip: `Document::set_play_values` answering an empty set → fails at "the
/// rect is dirtied".
#[test]
fn play_values_dirty_the_bound_layers_and_never_save() {
    let mut f = fixture();
    let mut res = Resolved::rebuild(&f.doc);
    let dirty = f
        .doc
        .set_play_values(played([(f.brand, Value::Color(RED))]));
    assert!(dirty.0.contains(&f.rect), "the rect is dirtied");
    res.update(&f.doc, &dirty);
    let fill = &f.doc.get(f.rect).unwrap().paint().fills[0];
    assert_eq!(
        f.doc.resolve_brush(f.rect, &fill.brush).into_owned(),
        mastik_core::Brush::Solid(RED),
        "drawn red"
    );
    let loaded = io::load(&io::save(&f.doc).expect("saves")).expect("loads");
    assert!(loaded.variables().played().is_empty(), "not saved");
    assert_eq!(
        loaded.resolve_var(f.dark_frame, f.brand),
        Some(Value::Color(NAVY)),
        "the file reads its modes"
    );
}
