//! The prototype's HTML export (`mastik_export::prototype`): what the trim keeps,
//! that the trimmed document loads, and the packed page.

use mastik_core::kurbo::{Affine, RoundedRectRadii, Size};
use mastik_core::prototype::{Action, Flow, FlowRef, PlayExport, Reaction, Transition, Trigger};
use mastik_core::{
    Document, Fill, IdSource, ImageEntry, ImageFormat, ImageId, ImageSource, Keyed, NodeId,
    NodeKind, Operation, Transaction, image_brush, io, keyed_by_position,
};
use mastik_export::prototype::{Parts, Player, chosen_flows, prune, reached_pages};

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
                },
            ),
        },
        Operation::SetReactions {
            id: other,
            reactions: click(
                &mut ids,
                Action::OpenOverlay {
                    overlay: modal,
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
                        values: Vec::new(),
                    },
                ),
                Keyed::new(
                    spare_id,
                    Flow {
                        name: "Spare".into(),
                        start: home,
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
        "measures",
    ] {
        assert!(called.contains(&owed), "the page calls {owed}");
    }
    assert!(html.contains("requestAnimationFrame"), "ticks by frame");
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
