//! The prototype's HTML export (`mastik_export::prototype`): what the trim keeps,
//! that the trimmed document loads, and the packed page.

use mastik_core::kurbo::{Affine, RoundedRectRadii, Size};
use mastik_core::prototype::{Action, Flow, FlowRef, PlayExport, Reaction, Transition, Trigger};
use mastik_core::{
    Document, Fill, IdSource, ImageEntry, ImageFormat, ImageId, ImageSource, Keyed, NodeId,
    NodeKind, Operation, Transaction, image_brush, io, keyed_by_position,
};
use mastik_export::prototype::{
    HoldWhy, Opening, Parts, Player, chosen_flows, holds, prune, prune_mirror, reached_pages,
};

fn frame(id: NodeId, parent: NodeId, name: &str) -> Operation {
    Operation::CreateNode {
        id,
        parent,
        index: 0,
        kind: NodeKind::Artboard {
            size: Size::new(100.0, 80.0),
            corner_radii: RoundedRectRadii::default(),
            corner_smoothing: 0.0,
        },
        transform: Some(Affine::IDENTITY),
        name: Some(name.into()),
    }
}

fn page(id: NodeId, index: usize, name: &str, flows: Vec<Keyed<Flow>>) -> Operation {
    Operation::AddPage {
        id,
        index,
        name: name.into(),
        ground: None,
        draft: false,
        modes: Vec::new(),
        flows,
        play: Default::default(),
    }
}

fn click(ids: &mut IdSource, action: Action) -> Vec<Keyed<Reaction>> {
    vec![Keyed::new(
        ids.mint_item(),
        Reaction {
            trigger: Trigger::Click,
            actions: vec![Keyed::new(ids.mint_item(), action)],
        },
    )]
}

/// The pages and layers of [`fixture`].
struct Fixture {
    doc: Document,
    /// Holds the flows *Main* and *Spare*, both starting at `home`.
    flows_page: NodeId,
    main: FlowRef,
    spare: FlowRef,
    /// The flow *Other*, on a page of its own, starting at `other`.
    other: FlowRef,
    /// Reached from `home` by a navigate.
    settings_page: NodeId,
    /// Holds the main of an instance on `home`.
    components_page: NodeId,
    /// Reached from `other` only, by an overlay.
    overlay_page: NodeId,
    /// Reached by nothing; holds the only layer wearing `picture`.
    unreached_page: NodeId,
    picture: ImageId,
}

/// Six pages: *Main*'s page, a page `home` navigates to, a page of components
/// `home` places an instance from, *Other*'s page, a page only *Other* reaches,
/// and a page nothing reaches, whose layer wears the document's one picture.
fn fixture() -> Fixture {
    let mut ids = IdSource::new(0x5E);
    let flows_page = ids.mint();
    let mut doc = Document::new(flows_page);
    let [
        settings_page,
        components_page,
        other_page,
        overlay_page,
        unreached_page,
    ] = [ids.mint(), ids.mint(), ids.mint(), ids.mint(), ids.mint()];
    let [home, other, button, settings, card, modal, orphan] = [
        ids.mint(),
        ids.mint(),
        ids.mint(),
        ids.mint(),
        ids.mint(),
        ids.mint(),
        ids.mint(),
    ];
    let picture = ImageId("hash-of-a-photo".into());
    doc.apply(&Transaction(vec![
        page(settings_page, 1, "Settings", Vec::new()),
        page(components_page, 2, "Components", Vec::new()),
        page(other_page, 3, "Other", Vec::new()),
        page(overlay_page, 4, "Overlays", Vec::new()),
        page(unreached_page, 5, "Unreached", Vec::new()),
        frame(home, flows_page, "Home"),
        frame(other, other_page, "Other"),
        frame(button, home, "Button"),
        frame(settings, settings_page, "Settings"),
        frame(card, components_page, "Card"),
        Operation::SetComponent {
            id: card,
            component: true,
        },
        frame(modal, overlay_page, "Modal"),
        frame(orphan, unreached_page, "Orphan"),
        Operation::AddImage {
            id: picture.clone(),
            entry: ImageEntry {
                source: ImageSource::Embedded(vec![1, 2, 3, 4].into()),
                format: ImageFormat::Png,
                width: 2,
                height: 2,
            },
        },
        Operation::SetFills {
            id: orphan,
            fills: keyed_by_position([Fill {
                brush: image_brush(picture.clone()).into(),
                visible: true,
            }]),
        },
    ]))
    .expect("seed");
    let (tx, _) = mastik_core::build::place_instance(&doc, &mut ids, card, home, None)
        .expect("an instance of Card");
    doc.apply(&tx).expect("placed");
    let [main_id, spare_id, other_id] = [ids.mint_item(), ids.mint_item(), ids.mint_item()];
    doc.apply(&Transaction(vec![
        Operation::SetReactions {
            id: button,
            reactions: click(
                &mut ids,
                Action::Navigate {
                    to: settings,
                    transition: Transition::default(),
                    keep_scroll: false,
                },
            ),
        },
        Operation::SetReactions {
            id: other,
            reactions: click(
                &mut ids,
                Action::OpenOverlay {
                    overlay: modal,
                    settings: Default::default(),
                    transition: Transition::default(),
                },
            ),
        },
        Operation::SetFlows {
            id: flows_page,
            flows: vec![
                Keyed::new(
                    main_id,
                    Flow {
                        name: "Main".into(),
                        start: home,
                        modes: Vec::new(),
                        values: Vec::new(),
                    },
                ),
                Keyed::new(
                    spare_id,
                    Flow {
                        name: "Spare".into(),
                        start: home,
                        modes: Vec::new(),
                        values: Vec::new(),
                    },
                ),
            ],
        },
        Operation::SetFlows {
            id: other_page,
            flows: vec![Keyed::new(
                other_id,
                Flow {
                    name: "Other".into(),
                    start: other,
                    modes: Vec::new(),
                    values: Vec::new(),
                },
            )],
        },
    ]))
    .expect("reactions and flows");
    Fixture {
        doc,
        flows_page,
        main: FlowRef {
            page: flows_page,
            flow: main_id,
        },
        spare: FlowRef {
            page: flows_page,
            flow: spare_id,
        },
        other: FlowRef {
            page: other_page,
            flow: other_id,
        },
        settings_page,
        components_page,
        overlay_page,
        unreached_page,
        picture,
    }
}

fn choose(doc: &mut Document, flows: Vec<FlowRef>) {
    doc.apply(&Transaction(vec![Operation::SetPlayExport {
        settings: PlayExport {
            flows,
            ..Default::default()
        },
    }]))
    .expect("export settings");
}

/// One flow reaches its own page, the page it navigates to and the page holding
/// an instance's main, and not a page only another flow's overlay reaches.
///
/// flip: `reached_pages` not following `Node::link` → fails at "the main's page";
/// not following reactions → fails at "navigated to".
#[test]
fn a_flow_reaches_the_pages_it_navigates_to_and_its_mains() {
    let f = fixture();
    let pages = reached_pages(&f.doc, &[f.main]);
    assert!(pages.contains(&f.flows_page), "the flow's own page");
    assert!(pages.contains(&f.settings_page), "navigated to");
    assert!(pages.contains(&f.components_page), "the main's page");
    assert!(!pages.contains(&f.overlay_page), "another flow's overlay");
    assert!(!pages.contains(&f.unreached_page), "nothing reaches it");
}

/// The trimmed document keeps only the reached pages and the chosen flow, names
/// it as the flow the file opens on, drops the picture only an unreached page
/// showed, and loads back as it was saved.
///
/// flip: `prune` keeping every page (`Keep::all`) → fails at "the unreached page
/// goes"; not writing `SetFlows` → fails at "only the chosen flow".
#[test]
fn the_pruned_document_keeps_the_reached_pages_and_loads() {
    let mut f = fixture();
    choose(&mut f.doc, vec![f.main]);
    assert_eq!(
        chosen_flows(&f.doc),
        vec![f.main],
        "fixture: one flow chosen"
    );
    assert!(f.doc.has_image(&f.picture), "fixture: the picture is there");
    let pruned = prune(&f.doc).expect("pruned");
    let pages: Vec<NodeId> = pruned.page_ids().collect();
    assert_eq!(
        pages,
        vec![f.flows_page, f.settings_page, f.components_page],
        "the unreached page goes, the others stay in order"
    );
    let flows = &pruned.page(f.flows_page).expect("kept").flows;
    assert_eq!(flows.len(), 1, "only the chosen flow");
    assert_eq!(flows[0].id, f.main.flow);
    assert_eq!(pruned.play_export().opens, Some(f.main), "opens on it");
    assert!(
        !pruned.has_image(&f.picture),
        "the picture goes with its page"
    );
    let loaded = io::load(&io::save(&pruned).expect("saves")).expect("the loader takes it");
    assert_eq!(loaded.page_ids().collect::<Vec<_>>(), pages, "round-trips");
}

/// The file is handed to other people: a table nothing kept reads goes, a
/// read one stays with the pictures its rows name though no layer shows them,
/// and no linked file's path or meta but the name stays.
/// flip: `trim` skipping `for_sharing` → fails at "the unread table goes";
/// `images_shown` not counting data sets → fails at "its picture stays".
#[test]
fn the_pruned_document_keeps_only_what_the_page_reads() {
    use mastik_core::chart::{Column, ColumnId, DataLink, Table};
    use mastik_core::list::DataSet;
    let mut f = fixture();
    choose(&mut f.doc, vec![f.main]);
    let mut ids = IdSource::new(0x7A);
    let (read, unread, tap) = (ids.mint_data_set(), ids.mint_data_set(), ids.mint());
    let flows = &f.doc.page(f.flows_page).expect("the page").flows;
    let home = flows
        .iter()
        .find(|k| k.id == f.main.flow)
        .expect("Main")
        .start;
    let photo = ImageId("row-photo".into());
    let link = |path: &str| DataLink {
        path: path.into(),
        size: 0,
        modified: 0,
        header: None,
    };
    let set = |id, name: &str, cell: &str, path: &str| DataSet {
        id,
        name: name.into(),
        table: Table {
            columns: vec![Column {
                id: ColumnId(0),
                name: "Photo".into(),
            }],
            rows: vec![vec![cell.into()]],
        },
        source: Some(link(path)),
        pictures: Vec::new(),
    };
    let mut kept = set(read, "Read", "ada.png", r"C:\Users\someone\read.csv");
    kept.pictures.push(("ada.png".into(), photo.clone()));
    f.doc
        .apply(&Transaction(vec![
            Operation::AddImage {
                id: photo.clone(),
                entry: ImageEntry {
                    source: ImageSource::Embedded(vec![0u8; 8].into()),
                    format: ImageFormat::Png,
                    width: 1,
                    height: 1,
                },
            },
            Operation::SetDataSet {
                id: read,
                set: Some(Box::new(kept)),
            },
            Operation::SetDataSet {
                id: unread,
                set: Some(Box::new(set(
                    unread,
                    "Unread",
                    "secret-cell",
                    r"C:\Users\someone\unread.csv",
                ))),
            },
            frame(tap, home, "Tap"),
            Operation::SetReactions {
                id: tap,
                reactions: click(
                    &mut ids,
                    Action::AddRow {
                        table: read,
                        at: Default::default(),
                        cells: Vec::new(),
                    },
                ),
            },
        ]))
        .expect("fixture applies");
    let mut meta = f.doc.meta().clone();
    meta.name = Some("Onboarding".into());
    meta.project = Some("acme-project".into());
    f.doc.set_meta(meta);
    let pruned = prune(&f.doc).expect("pruned");
    assert!(pruned.data_set(unread).is_none(), "the unread table goes");
    let set = pruned.data_set(read).expect("the read table stays");
    assert_eq!(set.source, None, "no path");
    assert!(pruned.has_image(&photo), "its picture stays");
    assert_eq!(pruned.meta().name.as_deref(), Some("Onboarding"));
    let saved = String::from_utf8(io::save(&pruned).expect("saves")).unwrap();
    for gone in ["secret-cell", "someone", "acme-project"] {
        assert!(!saved.contains(gone), "{gone} is not in the file");
    }
}

/// With no flow named, every flow goes in, and with it the page only the second
/// flow's overlay reaches.
///
/// flip: `chosen_flows` taking only the first flow when none is named → fails at
/// the first `assert_eq`.
#[test]
fn naming_no_flow_takes_every_flow() {
    let f = fixture();
    assert_eq!(chosen_flows(&f.doc), vec![f.main, f.spare, f.other]);
    let pruned = prune(&f.doc).expect("pruned");
    assert!(pruned.page(f.overlay_page).is_some(), "the overlay's page");
    assert!(
        pruned.page(f.unreached_page).is_none(),
        "still not this one"
    );
    assert_eq!(pruned.play_export().opens, Some(f.main), "the first flow");
}

/// A document without flows has nothing to export.
#[test]
fn a_document_without_flows_is_refused() {
    let doc = Document::new(IdSource::new(1).mint());
    assert_eq!(
        prune(&doc).map(|_| ()),
        Err(mastik_export::prototype::PackError::NoFlows)
    );
}

const WASM: &[u8] = b"\0asm-not-a-real-module";
const GLUE: &str = "let wasm_bindgen = function () { return \"</script>\"; };";

/// The page carries the payload once as base64, the glue made safe inside its
/// `<script>`, and the opening flow as its title; the payload gunzips to the
/// container with the wasm and the document in it.
///
/// flip: `script_safe` returning the glue unchanged → fails at "the glue cannot
/// close its script".
#[test]
fn the_page_holds_the_packed_parts() {
    let mut f = fixture();
    choose(&mut f.doc, vec![f.main]);
    let parts = Parts::of(
        &f.doc,
        Player {
            wasm: WASM,
            glue: GLUE,
        },
    )
    .expect("parts");
    let html = parts.html();
    assert!(html.contains("<title>Main</title>"), "titled by its flow");
    assert_eq!(
        html.matches("</script>").count(),
        3,
        "the glue cannot close its script"
    );
    let start = html
        .find("type=\"application/octet-stream\">")
        .expect("the payload")
        + "type=\"application/octet-stream\">".len();
    let end = start + html[start..].find("</script>").expect("closed");
    use base64::Engine;
    let gz = base64::engine::general_purpose::STANDARD
        .decode(&html[start..end])
        .expect("base64");
    let mut container = Vec::new();
    std::io::Read::read_to_end(&mut flate2::read::GzDecoder::new(&gz[..]), &mut container)
        .expect("gzip");
    assert_eq!(&container[..8], b"MSTKPLAY");
    assert_eq!(container[12], 1, "the wasm first");
    let wasm_len = u32::from_le_bytes(container[17..21].try_into().unwrap()) as usize;
    assert_eq!(&container[21..21 + wasm_len], WASM);
    let doc_at = 21 + wasm_len;
    assert_eq!(container[doc_at], 2, "then the document");
    let doc_len =
        u32::from_le_bytes(container[doc_at + 5..doc_at + 9].try_into().unwrap()) as usize;
    let doc = io::load(&container[doc_at + 9..doc_at + 9 + doc_len]).expect("the document loads");
    assert_eq!(doc.page_ids().count(), 3, "the pruned one");
}

/// The page's script calls only methods `mastik-web`'s player exports, and
/// every method a shell owes play is called: frame, the pointer, wheel and keys,
/// the hints, opened addresses and notices. The page is never built here, so a
/// renamed or dropped method would otherwise surface only in a browser.
///
/// flip: renaming `Player::over_hotspot` in `mastik-web` → fails at "the player
/// exports".
#[test]
fn the_page_calls_only_what_the_player_exports() {
    let mut f = fixture();
    choose(&mut f.doc, vec![f.main]);
    let html = Parts::of(
        &f.doc,
        Player {
            wasm: WASM,
            glue: GLUE,
        },
    )
    .expect("parts")
    .html();
    let lib = include_str!("../../mastik-web/src/lib.rs");
    let exported: Vec<&str> = lib
        .split("pub fn ")
        .skip(1)
        .filter_map(|s| s.split('(').next())
        .collect();
    let mut called: Vec<&str> = html
        .split("player.")
        .skip(1)
        .filter_map(|s| {
            let name = s.split('(').next()?;
            name.chars()
                .all(|c| c.is_ascii_lowercase() || c == '_')
                .then_some(name)
        })
        .collect();
    called.sort_unstable();
    called.dedup();
    for name in &called {
        assert!(exported.contains(name), "the player exports {name}");
    }
    for owed in [
        "frame",
        "pointer_down",
        "pointer_move",
        "pointer_up",
        "pointer_leave",
        "wheel",
        "key",
        "hotspots",
        "take_urls",
        "take_notices",
        "restart",
        "layer_at",
        "layer_box",
        "layer_note",
        "measures",
        "rebase",
        "screen_name",
    ] {
        assert!(called.contains(&owed), "the page calls {owed}");
    }
    assert!(html.contains("requestAnimationFrame"), "ticks by frame");
}

/// The arguments inside the bracket `s` opens with, split at the commas outside
/// any inner bracket (and, for JavaScript, string); `angle` counts `<>` as
/// brackets, for a Rust parameter list.
fn arguments(s: &str, angle: bool) -> Vec<&str> {
    let (mut depth, mut from, mut quote, mut escaped) = (0usize, 1, None, false);
    let mut out = Vec::new();
    for (i, c) in s.char_indices() {
        if let Some(q) = quote {
            match c {
                _ if escaped => escaped = false,
                '\\' => escaped = true,
                _ if c == q => quote = None,
                _ => {}
            }
            continue;
        }
        match c {
            '"' | '\'' | '`' if !angle => quote = Some(c),
            '(' | '[' | '{' => depth += 1,
            '<' if angle => depth += 1,
            '>' if angle => depth -= 1,
            ')' | ']' | '}' => {
                depth -= 1;
                if depth == 0 {
                    out.push(&s[from..i]);
                    break;
                }
            }
            ',' if depth == 1 => {
                out.push(&s[from..i]);
                from = i + 1;
            }
            _ => {}
        }
    }
    out.into_iter()
        .map(str::trim)
        .filter(|a| !a.is_empty())
        .collect()
}

/// The page passes each player function as many arguments as it takes, and
/// reads `frame`'s answer by the slots `frame` fills, in their order.
///
/// flip: a slot inserted before `moving` in `Player::frame`'s answer → fails at
/// "frame's slots"; a fifth parameter on `start` → fails at "as many arguments".
#[test]
fn the_page_calls_the_player_with_its_arity_and_reads_frames_slots() {
    let mut f = fixture();
    choose(&mut f.doc, vec![f.main]);
    let html = Parts::of(
        &f.doc,
        Player {
            wasm: WASM,
            glue: GLUE,
        },
    )
    .expect("parts")
    .html();
    let lib = include_str!("../../mastik-web/src/lib.rs");
    let takes = |name: &str| {
        let at = [format!("pub fn {name}("), format!("pub async fn {name}(")]
            .iter()
            .find_map(|head| lib.find(head.as_str()).map(|at| at + head.len() - 1))
            .unwrap_or_else(|| panic!("the player exports {name}"));
        arguments(&lib[at..], true)
            .into_iter()
            .filter(|p| !p.ends_with("self"))
            .count()
    };
    let mut checked = Vec::new();
    for object in ["player.", "wasm_bindgen."] {
        for call in html.split(object).skip(1) {
            let name: String = call
                .chars()
                .take_while(|c| c.is_ascii_lowercase() || *c == '_')
                .collect();
            if name.is_empty() || !call[name.len()..].starts_with('(') {
                continue;
            }
            let given = arguments(&call[name.len()..], false);
            // A spread's count is the array's, which a text cannot read.
            if given.iter().any(|a| a.starts_with("...")) {
                continue;
            }
            assert_eq!(
                given.len(),
                takes(&name),
                "the page calls {object}{name} with as many arguments as it takes"
            );
            checked.push(name);
        }
    }
    for owed in ["start", "add_font", "frame", "layer_at"] {
        assert!(checked.iter().any(|c| c == owed), "checked {owed}");
    }

    let body = &lib[lib.find("pub fn frame(").expect("frame")..];
    let body = &body[body.find("Ok(vec![").expect("frame's answer") + "Ok(vec!".len()..];
    assert_eq!(
        arguments(body, false),
        [
            "p.scale",
            "p.stage.width",
            "p.stage.height",
            "p.at.x",
            "p.at.y",
            "p.screen.width",
            "p.screen.height",
            "p.bezel",
            "p.radius",
            "f64::from(u8::from(self.play.is_moving()))",
            "flash",
        ],
        "frame's slots"
    );
    for read in [
        "const [, sw, sh, x, y, w, h, b, r] = p;",
        "p.slice(1, 9)",
        "if (p[9])",
        "drawHints(p[10])",
    ] {
        assert!(html.contains(read), "the page reads frame's slots: {read}");
    }
}

/// Inspect mode's data goes in: every layer's CSS as *Copy as CSS* writes it, and
/// the files a layer's export settings write, rendered, under the layer's id.
///
/// flip: `inspect_files` skipping every layer → fails at "the button's PNG".
#[test]
fn inspect_carries_each_layers_css_and_export_files() {
    let mut f = fixture();
    choose(&mut f.doc, vec![f.main]);
    let button = f
        .doc
        .node_ids()
        .find(|id| f.doc.get(*id).is_some_and(|n| n.name() == "Button"))
        .expect("the button");
    let spec = mastik_core::ExportSpec::new(
        mastik_core::ExportFormat::Png,
        mastik_core::ExportScale::Times(1.0),
    );
    f.doc
        .apply(&Transaction(vec![Operation::SetExports {
            id: button,
            exports: keyed_by_position([spec]),
        }]))
        .expect("export settings");
    let parts = Parts::of(
        &f.doc,
        Player {
            wasm: WASM,
            glue: GLUE,
        },
    )
    .expect("parts");
    let mut pruned = prune(&f.doc).expect("pruned");
    let res = mastik_core::Resolved::settled(&mut pruned);
    let css = parts
        .css
        .iter()
        .find(|(id, _)| *id == button)
        .map(|(_, c)| c.as_str());
    assert_eq!(
        css,
        Some(mastik_export::code::css_of(&pruned, &res, &[button]).as_str()),
        "the button's CSS, as Copy as CSS writes it"
    );
    assert!(
        !parts.css.iter().any(|(id, _)| pruned.is_page(*id)),
        "no page is a layer to inspect"
    );
    let file = parts
        .files
        .iter()
        .find(|f| f.layer == button)
        .expect("the button's PNG");
    assert_eq!(file.path, "Button.png");
    assert_eq!(&file.bytes[..8], b"\x89PNG\r\n\x1a\n");
}

/// A *One file per row* setting offers inspect mode the layer's one file as it
/// stands, a file per setting whatever the table's length, *One PDF* included.
///
/// flip: `inspect_files` planning without `rows_unfilled` → fails at "one file
/// a setting".
#[test]
fn inspect_offers_a_per_row_setting_as_one_file() {
    use mastik_core::chart::{Column, Table};
    use mastik_core::{ExportFormat, ExportScale, ExportSpec, PerRow, RowsAs};
    let mut f = fixture();
    choose(&mut f.doc, vec![f.main]);
    let button = f
        .doc
        .node_ids()
        .find(|id| f.doc.get(*id).is_some_and(|n| n.name() == "Button"))
        .expect("the button");
    let table = IdSource::new(0x7AB).mint_data_set();
    let mut t = Table::default();
    let id = t.next_column_id();
    t.columns.push(Column {
        id,
        name: "Name".into(),
    });
    t.rows = (0..5).map(|r| vec![format!("Row {r}")]).collect();
    let png = ExportSpec {
        per_row: Some(PerRow::new(table)),
        ..ExportSpec::new(ExportFormat::Png, ExportScale::Times(1.0))
    };
    let pdf = ExportSpec {
        per_row: Some(PerRow {
            rows_as: RowsAs::Pages,
            ..PerRow::new(table)
        }),
        ..ExportSpec::new(ExportFormat::Pdf, ExportScale::Times(1.0))
    };
    f.doc
        .apply(&Transaction(vec![
            Operation::SetDataSet {
                id: table,
                set: Some(Box::new(mastik_core::list::DataSet {
                    id: table,
                    name: "People".into(),
                    table: t,
                    source: None,
                    pictures: Vec::new(),
                })),
            },
            Operation::SetExports {
                id: button,
                exports: keyed_by_position([png, pdf]),
            },
        ]))
        .expect("export settings");
    let parts = Parts::of(
        &f.doc,
        Player {
            wasm: WASM,
            glue: GLUE,
        },
    )
    .expect("parts");
    let paths: Vec<&str> = parts
        .files
        .iter()
        .filter(|f| f.layer == button)
        .map(|f| f.path.as_str())
        .collect();
    assert_eq!(paths, ["Button.png", "Button.pdf"], "one file a setting");
    let pdf = parts.files.iter().find(|f| f.path == "Button.pdf");
    let text = String::from_utf8_lossy(&pdf.expect("the PDF").bytes);
    let pages = text.matches("/Type/Page").count() - text.matches("/Type/Pages").count();
    assert_eq!(pages, 1, "the PDF is the layer's one page");
}

/// A mirror of one flow keeps that flow alone and the pages it reaches, and
/// plays the page as the phone shows it: no bar, no device frame, and no device
/// unless the export keeps the page's, a setting the file saves.
///
/// flip: `prune_mirror` keeping the page's device → fails at "the phone is the
/// device"; dropping it under *Keep the page's* → fails at "the page's device
/// kept".
#[test]
fn a_mirror_keeps_its_flow_and_drops_the_device() {
    let mut f = fixture();
    let play = mastik_core::prototype::PagePlay {
        device: Some("iPhone 16".into()),
        landscape: true,
        ..Default::default()
    };
    f.doc
        .apply(&Transaction(vec![Operation::SetPagePlay {
            id: f.other.page,
            play,
        }]))
        .expect("a device");
    let out = prune_mirror(&f.doc, Opening::Flow(f.other)).expect("the mirror");
    let pages: Vec<_> = out.page_ids().collect();
    assert_eq!(
        pages,
        vec![f.other.page, f.overlay_page],
        "the flow's pages alone"
    );
    let export = out.play_export();
    assert_eq!(export.flows, vec![f.other], "that flow alone");
    assert_eq!(export.opens, Some(f.other));
    assert!(!export.player_bar && !export.device_frame, "no chrome");
    let play = &out.page(f.other.page).expect("kept").play;
    assert_eq!(
        (play.device.as_deref(), play.landscape),
        (None, false),
        "the phone is the device"
    );
    let keep = PlayExport {
        on_phone: mastik_core::prototype::OnPhone::Keep,
        ..f.doc.play_export().clone()
    };
    f.doc
        .apply(&Transaction(vec![Operation::SetPlayExport {
            settings: keep,
        }]))
        .expect("keep the page's");
    let saved = io::load(&io::save(&f.doc).expect("saves")).expect("loads");
    assert_eq!(
        saved.play_export().on_phone,
        mastik_core::prototype::OnPhone::Keep,
        "the setting is saved"
    );
    let out = prune_mirror(&f.doc, Opening::Flow(f.other)).expect("the mirror");
    let play = &out.page(f.other.page).expect("kept").play;
    assert_eq!(
        (play.device.as_deref(), play.landscape),
        (Some("iPhone 16"), true),
        "the page's device kept"
    );
}

/// A mirror of a frame on a page with no flow keeps no flow and the pages the
/// frame's page reaches; a frame that is gone is refused.
///
/// flip: `prune_mirror` seeding the walk with no page for a frame → fails at
/// "the mirror" (the trim refuses to keep no page).
#[test]
fn a_mirror_of_a_frame_keeps_its_page_and_no_flow() {
    let f = fixture();
    let settings = f.doc.get(f.settings_page).expect("page").children()[0];
    let out = prune_mirror(&f.doc, Opening::Frame(settings)).expect("the mirror");
    assert_eq!(
        out.page_ids().collect::<Vec<_>>(),
        vec![f.settings_page],
        "the frame's page"
    );
    assert!(
        out.pages().iter().all(|p| p.flows.is_empty()),
        "no flow plays"
    );
    let gone = IdSource::new(0x77).mint();
    assert_eq!(
        prune_mirror(&f.doc, Opening::Frame(gone)).err(),
        Some(mastik_export::prototype::PackError::NoFrame)
    );
}

/// The mirrored page carries its start and version for the boot script; the
/// exported file never carries mirror mode.
#[test]
fn only_the_mirrored_page_is_in_mirror_mode() {
    let f = fixture();
    let settings = f.doc.get(f.settings_page).expect("page").children()[0];
    let player = Player {
        wasm: WASM,
        glue: GLUE,
    };
    let parts = Parts::mirror(&f.doc, player, Opening::Frame(settings)).expect("parts");
    let page = parts.mirror_html(Some(settings), 3);
    let line = format!(
        "window.MASTIK_MIRROR = {{ frame: \"{}\", version: 3 }};",
        settings.to_wire()
    );
    assert!(page.contains(&line), "the start and version");
    assert!(parts.css.is_empty(), "no inspect mode");
    let mut g = fixture();
    choose(&mut g.doc, vec![g.main]);
    let file = Parts::of(&g.doc, player).expect("parts").html();
    assert!(
        !file.contains("window.MASTIK_MIRROR ="),
        "a file is never mirrored"
    );
}

/// A layer's annotation reaches the page in the packed document, where inspect
/// mode reads it (`Player::layer_note`).
///
/// flip: `prune` clearing every layer's note → fails at "the button's note".
#[test]
fn the_packed_document_carries_a_layers_annotation() {
    let mut f = fixture();
    choose(&mut f.doc, vec![f.main]);
    let button = f
        .doc
        .node_ids()
        .find(|id| f.doc.get(*id).is_some_and(|n| n.name() == "Button"))
        .expect("the button");
    f.doc
        .apply(&Transaction(vec![Operation::SetNote {
            id: button,
            text: "Primary action.\nDisabled until the form is valid.".into(),
        }]))
        .expect("the note");
    let parts = Parts::of(
        &f.doc,
        Player {
            wasm: WASM,
            glue: GLUE,
        },
    )
    .expect("parts");
    let packed = mastik_core::io::load(&parts.document).expect("the packed document");
    assert_eq!(
        packed.get(button).map(|n| n.note()),
        Some("Primary action.\nDisabled until the form is valid."),
        "the button's note"
    );
}

/// The size estimate is never under the file written and not far over it: the
/// parts at gzip's fastest level against its best.
///
/// flip: `estimate` counting the stream's bytes without base64's 4/3 → fails at
/// "never under".
#[test]
fn the_estimate_is_close_and_never_under() {
    let mut f = fixture();
    choose(&mut f.doc, vec![f.main]);
    // Incompressible bytes stand for a real wasm, so the estimate has weight to get
    // wrong.
    let mut seed = 0x2545_f491_u32;
    let wasm: Vec<u8> = (0..400_000)
        .map(|_| {
            seed ^= seed << 13;
            seed ^= seed >> 17;
            seed ^= seed << 5;
            seed as u8
        })
        .collect();
    let parts = Parts::of(
        &f.doc,
        Player {
            wasm: &wasm,
            glue: GLUE,
        },
    )
    .expect("parts");
    let written = parts.html().len() as u64;
    let estimate = parts.estimate();
    assert!(estimate >= written, "never under: {estimate} < {written}");
    assert!(
        estimate <= written + written / 10,
        "within a tenth: {estimate} vs {written}"
    );
    assert!(
        written > 400_000 * 4 / 3,
        "fixture: base64 of the wasm at least"
    );
}

fn text(id: NodeId, parent: NodeId, content: &str, family: &str) -> Operation {
    Operation::CreateNode {
        id,
        parent,
        index: 0,
        kind: NodeKind::Text {
            content: content.into(),
            style: Box::new(mastik_core::TextStyle {
                font_family: family.into(),
                font_size: 16.0,
                ..Default::default()
            }),
            spans: Default::default(),
            para_spans: Default::default(),
            paragraph: Default::default(),
            block: Default::default(),
            sizing: mastik_core::TextSizing::Auto,
            on_path: None,
            on_path_flip: false,
            on_path_offset: 0.0,
        },
        transform: Some(Affine::IDENTITY),
        name: None,
    }
}

/// A face the canvas registered under another family's name goes in whole,
/// carrying that name, and the bundled Inter is left out.
///
/// flip: `fonts_used` without its `is_bundled` skip → fails at "Inter stays
/// out"; with `alias: None` → fails at "under its alias".
#[test]
fn the_fonts_go_in_whole_with_their_alias() {
    let emoji = include_bytes!("../../mastik-app/assets/fonts/NotoEmoji.ttf");
    assert!(
        mastik_core::text::register_fonts_as(emoji.to_vec(), "Fancy Emoji"),
        "fixture: registered under an alias"
    );
    let mut ids = IdSource::new(0x6F);
    let page = ids.mint();
    let mut doc = Document::new(page);
    let [board, smile, plain] = [ids.mint(), ids.mint(), ids.mint()];
    doc.apply(&Transaction(vec![
        frame(board, page, "Board"),
        text(smile, board, "😀", "Fancy Emoji"),
        text(plain, board, "Plain", "Inter"),
    ]))
    .expect("seed");
    let res = mastik_core::Resolved::settled(&mut doc);
    let fonts = mastik_export::prototype::fonts_used(&doc, &res);
    assert_eq!(fonts.len(), 1, "Inter stays out");
    assert_eq!(
        fonts[0].alias.as_deref(),
        Some("Fancy Emoji"),
        "under its alias"
    );
    assert_eq!(fonts[0].bytes, emoji, "the whole file");
}

/// The breakdown's four shares add up to the estimate, and a large player
/// outweighs a small document.
///
/// flip: `breakdown` giving the player the document's weight → fails at "the player leads".
#[test]
fn the_breakdown_adds_up_to_the_estimate() {
    let mut f = fixture();
    choose(&mut f.doc, vec![f.main]);
    let mut seed = 0x1234_5678_u32;
    let wasm: Vec<u8> = (0..200_000)
        .map(|_| {
            seed ^= seed << 13;
            seed ^= seed >> 17;
            seed ^= seed << 5;
            seed as u8
        })
        .collect();
    let parts = Parts::of(
        &f.doc,
        Player {
            wasm: &wasm,
            glue: GLUE,
        },
    )
    .expect("parts");
    let b = parts.breakdown();
    assert_eq!(b.total(), parts.estimate(), "adds up");
    assert!(b.player > b.document * 4, "the player leads: {b:?}");
    assert!(
        parts.pictures.is_empty(),
        "the picture's page is not carried"
    );
}

/// A layer the file carries whose action names a layer that is gone, or whose
/// condition does not read, holds the export, named once per kind.
///
/// flip: `holds` not checking targets → fails at "the missing target".
#[test]
fn a_missing_target_or_a_broken_expression_holds_the_export() {
    let mut f = fixture();
    choose(&mut f.doc, vec![f.main]);
    assert!(holds(&f.doc).is_empty(), "the fixture plays as authored");
    let home = f.doc.page(f.flows_page).expect("page").flows[0].value.start;
    let mut ids = IdSource::new(0x77);
    let gone = ids.mint();
    let mut reactions = click(
        &mut ids,
        Action::Navigate {
            to: gone,
            transition: Transition::default(),
            keep_scroll: false,
        },
    );
    reactions.extend(click(
        &mut ids,
        Action::Conditional {
            branches: vec![mastik_core::prototype::Branch {
                when: mastik_core::prototype::Expr("1 +".into()),
                actions: Vec::new(),
            }],
            otherwise: Vec::new(),
        },
    ));
    f.doc
        .apply(&Transaction(vec![Operation::SetReactions {
            id: home,
            reactions,
        }]))
        .expect("reactions");
    let held = holds(&f.doc);
    assert!(
        held.iter()
            .any(|h| h.layer == home && h.why == HoldWhy::Missing),
        "the missing target: {held:?}"
    );
    assert!(
        held.iter()
            .any(|h| h.layer == home && matches!(h.why, HoldWhy::Expression(_))),
        "the broken expression: {held:?}"
    );
    assert_eq!(held.len(), 2, "each once");
}

/// A row action on a table that is gone holds the export as a missing frame
/// does, and a cell that does not read holds it as a condition does.
///
/// flip: `holds` asking only `layer_targets` → fails at "the missing table".
#[test]
fn a_row_action_on_a_missing_table_or_a_broken_cell_holds_the_export() {
    use mastik_core::prototype::{CellWrite, Expr, RowEnd};
    let mut f = fixture();
    choose(&mut f.doc, vec![f.main]);
    let home = f.doc.page(f.flows_page).expect("page").flows[0].value.start;
    let mut ids = IdSource::new(0x78);
    let gone = ids.mint_data_set();
    let reactions = click(
        &mut ids,
        Action::AddRow {
            table: gone,
            at: RowEnd::End,
            cells: vec![CellWrite {
                column: mastik_core::chart::ColumnId(0),
                to: Expr("1 +".into()),
            }],
        },
    );
    f.doc
        .apply(&Transaction(vec![Operation::SetReactions {
            id: home,
            reactions,
        }]))
        .expect("reactions");
    let held = holds(&f.doc);
    assert!(
        held.iter()
            .any(|h| h.layer == home && h.why == HoldWhy::Missing),
        "the missing table: {held:?}"
    );
    assert!(
        held.iter()
            .any(|h| h.layer == home && matches!(h.why, HoldWhy::Expression(_))),
        "the broken cell: {held:?}"
    );
}
