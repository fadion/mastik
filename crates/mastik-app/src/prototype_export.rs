//! The prototype's export card (`docs/design/prototyping-export.md`): one card
//! with three outputs switched at its head. *HTML file* picks the flows that go
//! into the file, the one it opens on, the player's chrome and scaling, and
//! shows the file's size split by what makes it, warnings, and what holds the
//! export, all measured before anything is written. *Video* writes one of the
//! player's takes; *Flow map* a PDF of the flows picked. The HTML settings are
//! kept with the document, so a second export asks nothing.

use crate::app::MastikApp;
use crate::job::Job;
use crate::settings::{self, CardHold};
use crate::theme::{self, color, icon};
use crate::ui;
use eframe::egui;
use mastik_core::prototype::{FlowRef, OnPhone, PlayExport};
use mastik_core::{Document, ImageEntry, ImageId, ImageSource, NodeId, Operation, Transaction};
use mastik_export::prototype::{Breakdown, Hold, HoldWhy};
use mastik_export::video::Format;

/// The card's width.
const W: f32 = 380.0;
/// The preview's height in the Video output.
const PREVIEW_H: f32 = 132.0;
/// The air the preview's still keeps inside its well, on every side.
const PREVIEW_INSET: f32 = 4.0;
/// A video's frame rate.
const VIDEO_FPS: u32 = 30;
/// The sizes a video is offered at, as multiples of the screen.
const VIDEO_TIMES: [u32; 3] = [1, 2, 3];

/// The card's three outputs, in the order its head shows them.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Output {
    Html,
    Video,
    FlowMap,
}

impl Output {
    const ALL: [Output; 3] = [Output::Html, Output::Video, Output::FlowMap];

    fn label(self) -> &'static str {
        match self {
            Output::Html => "HTML file",
            Output::Video => "Video",
            Output::FlowMap => "Flow map",
        }
    }
}

/// A picture made smaller for this export only, and the size it is made.
pub(crate) type Downscale = (ImageId, (u32, u32));

/// What the measure worker answers: the file's size split by part, the
/// pictures it carries, and what holds the export.
#[derive(Clone)]
struct Measured {
    breakdown: Breakdown,
    pictures: Vec<(ImageId, u64)>,
    holds: Vec<Hold>,
}

/// What a measure is keyed by: the settings, the document's revision and the
/// pictures downscaled.
type Key = (PlayExport, u64, Vec<Downscale>);

/// The Video output's choices.
#[derive(Clone, Copy, Debug, PartialEq)]
struct VideoPick {
    /// The take, by its place among the player's takes, oldest first.
    take: usize,
    format: Format,
    times: u32,
    clicks: bool,
    device: bool,
}

impl VideoPick {
    fn options(&self) -> mastik_export::video::Options {
        mastik_export::video::Options {
            format: self.format,
            fps: VIDEO_FPS,
            scale: f64::from(self.times),
            clicks: self.clicks,
            device: self.device,
        }
    }
}

/// A heavy picture's warning: drawn far below its size.
struct Heavy {
    id: ImageId,
    layer: String,
    bytes: u64,
    size: (u32, u32),
    /// How wide it is drawn, in units, at most.
    drawn_w: u32,
    /// 2× as drawn, the size *Downscale to 2×* makes it.
    target: (u32, u32),
}

/// The card's state while it is open.
pub(crate) struct ExportCard {
    pub(crate) output: Output,
    /// The HTML settings as the card shows them.
    settings: PlayExport,
    /// The pictures this export downscales.
    downscale: Vec<Downscale>,
    measured: Option<(Key, Result<Measured, String>)>,
    measuring: Option<(Key, Job<Result<Measured, String>>)>,
    /// The flows the flow map takes; empty is every flow.
    map_flows: Vec<FlowRef>,
    video: VideoPick,
    /// The screen size of the take picked, in units, by take.
    screen: Option<(usize, Option<mastik_core::kurbo::Size>)>,
    preview: Option<(VideoPick, Option<egui::TextureHandle>)>,
    previewing: Option<(VideoPick, Job<Option<egui::ColorImage>>)>,
    /// The tallest the body has been, which it keeps: switching to a shorter
    /// output never shrinks the card.
    body_h: f32,
}

/// Every flow of `doc`, with the words its row shows: the page, then the flow.
pub(crate) fn every_flow(doc: &Document) -> Vec<(FlowRef, String)> {
    let mut out = Vec::new();
    for (pi, p) in doc.pages().iter().enumerate() {
        for (fi, f) in p.flows.iter().enumerate() {
            let page = crate::private::page_named(doc.get(p.id).map_or("", |n| n.name()), pi);
            let flow = crate::private::flow(&f.name, fi);
            out.push((
                FlowRef {
                    page: p.id,
                    flow: f.id,
                },
                format!("{page} · {flow}"),
            ));
        }
    }
    out
}

/// `list` (empty is every one of `all`) with `flow` in (`on`) or out. Turning
/// one off writes out the rest, and the last one back on empties it again.
fn toggled(list: &[FlowRef], all: &[FlowRef], flow: FlowRef, on: bool) -> Vec<FlowRef> {
    let mut list = match list.is_empty() {
        true => all.to_vec(),
        false => list.to_vec(),
    };
    list.retain(|f| *f != flow);
    if on {
        list.push(flow);
        list.sort_by_key(|f| all.iter().position(|a| a == f));
    }
    match list.len() == all.len() {
        true => Vec::new(),
        false => list,
    }
}

/// `settings` with `flow` in (`on`) or out of the file. An empty list is every
/// flow, so turning one off writes out the rest, and the last one back on
/// empties the list again.
pub(crate) fn with_flow(
    settings: &PlayExport,
    all: &[FlowRef],
    flow: FlowRef,
    on: bool,
) -> PlayExport {
    let mut out = settings.clone();
    out.flows = toggled(&settings.flows, all, flow, on);
    if out
        .opens
        .is_some_and(|o| !out.flows.is_empty() && !out.flows.contains(&o))
    {
        out.opens = None;
    }
    out
}

/// The folders to look for a browser player in, in order: `dirs` as they are, or,
/// for a document that runs a script, each preceded by its `-scripts` sibling,
/// the player that carries Luau. Only such a file pays for it, and without that
/// player the plain one plays, skipping *Run script* with a notice.
pub(crate) fn player_folders(
    dirs: Vec<std::path::PathBuf>,
    scripts: bool,
) -> Vec<std::path::PathBuf> {
    if !scripts {
        return dirs;
    }
    dirs.into_iter()
        .flat_map(|dir| {
            let mut name = dir.file_name().unwrap_or_default().to_os_string();
            name.push("-scripts");
            [dir.with_file_name(name), dir]
        })
        .collect()
}

/// Replace each picture `list` names in `doc` by one resized to its size, as
/// *Reduce picture…* would: a JPEG at a keeping quality, or a PNG when the
/// picture has see-through pixels. A picture that does not decode is left.
pub(crate) fn downscale(doc: &mut Document, list: &[Downscale]) {
    use crate::pictures::{Out, decode_rgba, encode, has_alpha};
    for (id, (w, h)) in list {
        let Some(entry) = doc.image(id).cloned() else {
            continue;
        };
        let Some(px) = decode_rgba(&entry) else {
            continue;
        };
        let small = image::imageops::resize(&px, *w, *h, image::imageops::FilterType::Lanczos3);
        let out = match has_alpha(&small) {
            true => Out::Png,
            false => Out::Jpeg(DOWNSCALE_JPEG),
        };
        let Some(e) = encode(&small, out) else {
            continue;
        };
        let new = crate::app::content_hash(&e.bytes);
        let mut ops = Vec::new();
        if !doc.has_image(&new) {
            ops.push(Operation::AddImage {
                id: new.clone(),
                entry: ImageEntry {
                    source: ImageSource::Embedded(e.bytes.into()),
                    format: e.format,
                    width: e.width,
                    height: e.height,
                },
            });
        }
        let scale = f64::from(entry.width) / f64::from(e.width.max(1));
        ops.extend(mastik_core::edit::repoint_image(doc, id, &new, scale));
        // A table's rows draw their pictures when a list refills in play.
        for set in doc.data_sets() {
            if set.pictures.iter().any(|(_, p)| p == id) {
                let mut next = set.clone();
                for (_, p) in &mut next.pictures {
                    if p == id {
                        *p = new.clone();
                    }
                }
                ops.push(Operation::SetDataSet {
                    id: set.id,
                    set: Some(Box::new(next)),
                });
            }
        }
        if doc.apply(&Transaction(ops)).is_err() {
            continue;
        }
        // Gone only once nothing names it: a cell may name it by its id.
        if mastik_core::io::unused_images(doc).contains(id) {
            let _ = doc.apply(&Transaction(vec![Operation::RemoveImage {
                id: id.clone(),
            }]));
        }
    }
}

/// The quality a picture downscaled for the export is encoded at.
const DOWNSCALE_JPEG: u8 = 85;

/// The file's parts and holds for `doc` under its own settings.
fn measure(doc: &Document, fonts: &[mastik_core::peniko::FontData]) -> Result<Measured, String> {
    mastik_core::text::register_font_data(fonts);
    let holds = mastik_export::prototype::holds(doc);
    let (wasm, glue) = crate::player_build(doc)?;
    let player = mastik_export::prototype::Player {
        wasm: &wasm,
        glue: &glue,
    };
    let parts = mastik_export::prototype::Parts::of(doc, player).map_err(|e| e.to_string())?;
    Ok(Measured {
        breakdown: parts.breakdown(),
        pictures: parts.pictures.clone(),
        holds,
    })
}

/// *On a phone*'s choices: the setting, its words and its caption.
const ON_PHONE: [(OnPhone, &str, &str); 2] = [
    (
        OnPhone::Follow,
        "Follow its turn",
        "laid out at the phone's size",
    ),
    (
        OnPhone::Keep,
        "Keep the page's",
        "the page's device, fitted",
    ),
];

/// *On a phone* as a dropdown, each row's caption in the accelerator's place,
/// as the scaling dropdown above it shows its own.
fn on_phone_combo(ui: &mut egui::Ui, w: f32, s: &mut PlayExport) {
    let shown = ON_PHONE
        .iter()
        .find(|(o, ..)| *o == s.on_phone)
        .map_or("Follow its turn", |(_, label, _)| label);
    ui::fitted_combo(
        ui,
        "export-on-phone",
        w,
        ui::combo_text(shown),
        shown,
        |ui| {
            ui::menu_rows(ui);
            for (o, label, caption) in ON_PHONE {
                let on = s.on_phone == o;
                let row = ui::MenuRow::new("", label).note(caption).selected(on);
                if ui::menu_row(ui, row, ui::MENU_ROW_H).clicked() && !on {
                    s.on_phone = o;
                }
            }
        },
    )
    .response
    .on_hover_text("What a Responsive page does on a phone, which never draws the device frame");
}

/// The size meter's parts: each one's name, share and ink.
fn meter_parts(b: &Breakdown) -> [(&'static str, u64, egui::Color32); 4] {
    [
        ("Player", b.player, color::accent()),
        ("Fonts", b.fonts, color::saved()),
        ("Images", b.images, color::warn()),
        ("Document", b.document, theme::text::faint()),
    ]
}

/// The size meter: the total on disk, the bar split by part, and its legend.
fn size_meter(ui: &mut egui::Ui, b: &Breakdown) {
    let total = b.total().max(1);
    let parts = meter_parts(b);
    ui::labelled(
        ui,
        &format!("On disk, about {}", settings::human_bytes(b.total())),
        |ui| {
            let shares: Vec<(f32, egui::Color32)> = parts
                .iter()
                .map(|(_, n, ink)| (*n as f32 / total as f32, *ink))
                .collect();
            ui::level_bar_parts(ui, ui.available_width(), &shares);
            let legend: Vec<(String, egui::Color32)> = parts
                .iter()
                .filter(|(_, n, _)| *n > 0)
                .map(|(name, n, ink)| (format!("{name} {}", settings::human_bytes(*n)), *ink))
                .collect();
            ui::level_bar_legend(ui, &legend);
        },
    );
}

/// The first layer that shows `id` in `doc`, by its name.
fn layer_showing(doc: &Document, id: &ImageId) -> Option<NodeId> {
    doc.node_ids().find(|n| {
        doc.get(*n).is_some_and(|node| {
            let paint = node.paint();
            (paint.fills.iter().map(|f| &f.brush))
                .chain(paint.strokes.iter().map(|s| &s.brush))
                .any(|b| matches!(b.literal(), Some(mastik_core::Brush::Image(i)) if i.image.id == *id))
        })
    })
}

/// `name` with the file name's forbidden characters taken out.
fn file_safe(name: &str) -> String {
    name.chars()
        .filter(|c| !matches!(c, '/' | '\\' | ':' | '*' | '?' | '"' | '<' | '>' | '|'))
        .collect::<String>()
        .trim()
        .to_string()
}

/// A take's row words: what played and when, then its length and clicks.
fn take_words(t: &crate::player::TakeRec) -> (String, String) {
    let secs = t.at as i64 + crate::library::clock::local_offset(t.at);
    let day = secs.rem_euclid(86_400);
    let when = format!("{}, {}:{:02}", t.name, day / 3600, (day % 3600) / 60);
    let clicks = match t.clicks() {
        1 => "1 click".to_string(),
        n => format!("{n} clicks"),
    };
    (
        when,
        format!("{} · {clicks}", crate::player::clock(t.take.length)),
    )
}

impl MastikApp {
    /// Open the export card on `output`, the HTML settings the document's own.
    /// The HTML file and the flow map need a flow; a video needs none.
    pub(crate) fn open_export(&mut self, output: Output) {
        if output != Output::Video && every_flow(&self.session.doc).is_empty() {
            let what = match output {
                Output::FlowMap => "a flow map",
                _ => "a prototype",
            };
            self.session
                .info(format!("Add a flow in prototype mode to export {what}"));
            return;
        }
        let takes = self.playing.as_ref().map_or(0, |p| p.takes.len());
        if let Some(card) = self.proto_export.as_mut() {
            card.output = output;
            card.video.take = takes.saturating_sub(1);
            return;
        }
        self.proto_export = Some(ExportCard {
            output,
            settings: self.session.doc.play_export().clone(),
            downscale: Vec::new(),
            measured: None,
            measuring: None,
            map_flows: Vec::new(),
            video: VideoPick {
                take: takes.saturating_sub(1),
                format: Format::Mp4,
                times: 2,
                clicks: true,
                device: false,
            },
            screen: None,
            preview: None,
            previewing: None,
            body_h: 0.0,
        });
    }

    /// A flow chip's menu row on `flow`: play it over the canvas, or open the
    /// export card with the file opening on it or the map holding it alone.
    pub(crate) fn act_on_flow(
        &mut self,
        ctx: &egui::Context,
        item: crate::menu::Item,
        flow: FlowRef,
    ) {
        use crate::menu::Item;
        match item {
            Item::Play => self.play_at(
                ctx,
                mastik_play::Start::Flow(flow),
                crate::player::Place::Over,
                false,
            ),
            Item::ExportPrototype => {
                self.open_export(Output::Html);
                if let Some(card) = self.proto_export.as_mut() {
                    let all: Vec<FlowRef> = every_flow(&self.session.doc)
                        .into_iter()
                        .map(|(r, _)| r)
                        .collect();
                    if !card.settings.flows.is_empty() && !card.settings.flows.contains(&flow) {
                        card.settings = with_flow(&card.settings, &all, flow, true);
                    }
                    card.settings.opens = Some(flow);
                }
            }
            Item::ExportFlowMap => {
                self.open_export(Output::FlowMap);
                if let Some(card) = self.proto_export.as_mut() {
                    card.map_flows = vec![flow];
                }
            }
            _ => {}
        }
    }

    /// The export card, while open.
    pub(crate) fn prototype_export_ui(&mut self, ctx: &egui::Context, canvas: egui::Rect) {
        let Some(mut card) = self.proto_export.take() else {
            return;
        };
        let revision = self.session.revision();
        let all = every_flow(&self.session.doc);
        if card.output == Output::Html && !all.is_empty() {
            self.measure_export(ctx, &mut card, revision);
        }
        let takes: Vec<crate::player::TakeRec> = self
            .playing
            .as_ref()
            .map(|p| p.takes.clone())
            .unwrap_or_default();
        if card.output == Output::Video
            && let Some(t) = takes.get(card.video.take)
        {
            self.preview_take(ctx, &mut card, t);
        }
        let heavy = match (&card.output, &card.measured) {
            (Output::Html, Some((_, Ok(m)))) => self.heavy_pictures(&m.pictures),
            _ => Vec::new(),
        };
        let mut close = false;
        let mut export = false;
        let outside =
            settings::canvas_card(ctx, "prototype-export", canvas, W, CardHold::Modal, |ui| {
                if settings::card_head(ui, W, "Export prototype") {
                    close = true;
                }
                let mut why_not: Option<String> = None;
                let mut note = String::new();
                egui::Frame::NONE
                    .inner_margin(egui::Margin::symmetric(settings::CANVAS_CARD_PAD as i8, 4))
                    .show(ui, |ui| {
                        ui.spacing_mut().item_spacing.y = ui::CARD_ROW_GAP;
                        let inner = W - 2.0 * settings::CANVAS_CARD_PAD;
                        let at = Output::ALL
                            .iter()
                            .position(|o| *o == card.output)
                            .unwrap_or(0);
                        let picked =
                            ui::segmented(ui, inner, ui::SEGMENT_CELL_H, 3, at, |p, i, r, on| {
                                ui::segment_label(p, r, Output::ALL[i].label(), on);
                            });
                        if let Some(i) = picked {
                            card.output = Output::ALL[i];
                        }
                        let top = ui.cursor().top();
                        ui.scope(|ui| {
                            ui.set_min_height(card.body_h);
                            match card.output {
                                Output::Html => self.html_body(
                                    ui,
                                    &mut card,
                                    &all,
                                    &heavy,
                                    revision,
                                    &mut note,
                                    &mut why_not,
                                ),
                                Output::Video => {
                                    video_body(ui, &mut card, &takes, &mut note, &mut why_not)
                                }
                                Output::FlowMap => flow_map_body(ui, &mut card, &all, &mut why_not),
                            }
                        });
                        card.body_h = card.body_h.max(ui.cursor().top() - top - ui::CARD_ROW_GAP);
                    });
                match settings::card_foot(
                    ui,
                    W,
                    &note,
                    &[
                        ("Cancel", ui::FieldButton::Off),
                        ("Export…", ui::FieldButton::Primary),
                    ],
                    why_not.as_deref(),
                ) {
                    Some(0) => close = true,
                    Some(_) => export = true,
                    None => {}
                }
            });
        if outside || ctx.input(|i| i.key_pressed(egui::Key::Escape)) {
            close = true;
        }
        if export {
            match card.output {
                Output::Html => {
                    self.export_prototype_from_card(card);
                    return;
                }
                Output::Video => {
                    if let Some(t) = takes.get(card.video.take) {
                        let (stem, take) = (self.export_stem(), t.clone());
                        let (ext, filter) = match card.video.format {
                            Format::Mp4 => ("mp4", "MP4 video"),
                            Format::Gif => ("gif", "Animated GIF"),
                        };
                        let name = format!("{stem} — {}.{ext}", file_safe(&take.name));
                        if let Some(path) = self.ask_path(filter, ext, &name) {
                            self.export_take_to(take.take, card.video.options(), path);
                            return;
                        }
                    }
                }
                Output::FlowMap => {
                    let name = format!("{} — Flow map.pdf", self.export_stem());
                    if let Some(path) = self.ask_path("PDF", "pdf", &name) {
                        self.export_flow_map_to(card.map_flows.clone(), path);
                        return;
                    }
                }
            }
        }
        if !close {
            self.proto_export = Some(card);
        }
    }

    /// The HTML file's body: flows, opening, player, scaling, then the size
    /// meter, the warnings and what holds the export.
    #[allow(clippy::too_many_arguments)]
    fn html_body(
        &self,
        ui: &mut egui::Ui,
        card: &mut ExportCard,
        all: &[(FlowRef, String)],
        heavy: &[Heavy],
        revision: u64,
        note: &mut String,
        why_not: &mut Option<String>,
    ) {
        if all.is_empty() {
            ui::empty_note(ui, "Add a flow in prototype mode to export a prototype");
            *why_not = Some("The document has no flow".into());
            return;
        }
        let refs: Vec<FlowRef> = all.iter().map(|(r, _)| *r).collect();
        // The page the file opens on, whose own scaling *Page's own* names.
        let opens = card.settings.opens.or(refs.first().copied());
        let page_scaling = opens
            .and_then(|r| self.session.doc.page(r.page))
            .map(|p| p.play.scaling)
            .unwrap_or_default();
        let s = &mut card.settings;
        ui::labelled(ui, "Flows", |ui| {
            if let Some(r) = flow_ticks(ui, all, &s.flows, "The file holds at least one flow") {
                let on = s.flows.is_empty() || s.flows.contains(&r);
                *s = with_flow(s, &refs, r, !on);
            }
        });
        let included: Vec<&(FlowRef, String)> = all
            .iter()
            .filter(|(r, _)| s.flows.is_empty() || s.flows.contains(r))
            .collect();
        ui::labelled(ui, "Opens on", |ui| {
            let shown = s
                .opens
                .and_then(|o| included.iter().copied().find(|(r, _)| *r == o))
                .or(included.first().copied())
                .map_or("None".to_string(), |(_, w)| w.clone());
            let w = ui.available_width();
            ui::fitted_combo(
                ui,
                "prototype-export-opens",
                w,
                ui::combo_text(&shown),
                &shown,
                |ui| {
                    ui::menu_rows(ui);
                    for (r, words) in &included {
                        let on = s.opens == Some(*r);
                        let row = ui::MenuRow::new("", words).selected(on);
                        if ui::menu_row(ui, row, ui::MENU_ROW_H).clicked() {
                            s.opens = Some(*r);
                        }
                    }
                },
            );
        });
        ui::labelled(ui, "Player", |ui| {
            if ui::switch_row(ui, "Device frame", s.device_frame, ui::CONTROL_H).clicked() {
                s.device_frame = !s.device_frame;
            }
            if ui::switch_row(ui, "Player bar", s.player_bar, ui::CONTROL_H).clicked() {
                s.player_bar = !s.player_bar;
            }
            if ui::switch_row(ui, "Hotspots", s.hotspot_hints, ui::CONTROL_H).clicked() {
                s.hotspot_hints = !s.hotspot_hints;
            }
        });
        ui::labelled(ui, "Scaling", |ui| {
            let w = ui.available_width();
            let page = Some(page_scaling);
            if let Some(v) =
                crate::panels::prototype::scaling_combo(ui, "export-scaling", w, page, s.scaling)
            {
                s.scaling = v;
            }
        });
        let responsive =
            s.scaling.unwrap_or(page_scaling) == mastik_core::prototype::Scaling::Responsive;
        ui::labelled(ui, "On a phone", |ui| {
            let w = ui.available_width();
            let r = ui.add_enabled_ui(responsive, |ui| on_phone_combo(ui, w, s));
            if !responsive {
                r.response
                    .on_disabled_hover_text("Only a Responsive page turns with the phone");
            }
        });
        let key = (card.settings.clone(), revision, card.downscale.clone());
        match &card.measured {
            Some((k, Ok(m))) if *k == key => {
                size_meter(ui, &m.breakdown);
                let doc = &self.session.doc;
                for h in &m.holds {
                    let name = crate::private::layer(doc, h.layer);
                    let text = match &h.why {
                        HoldWhy::Missing => format!("{name} acts on a layer that is gone"),
                        HoldWhy::Expression(e) => format!("{name}'s expression doesn't read: {e}"),
                    };
                    ui::note_line(ui, icon::WARNING, color::danger(), &text, None);
                }
                if !m.holds.is_empty() {
                    *why_not = Some("Fix the interactions named above first".into());
                }
            }
            Some((k, Err(why))) if *k == key => {
                *note = why.clone();
                *why_not = Some(why.clone());
            }
            // Held until measured: the measure also finds what holds the export.
            _ => {
                *note = "Measuring…".into();
                *why_not = Some("Measuring the file and checking its interactions".into());
            }
        }
        for h in heavy {
            let text = format!(
                "{}'s picture is {} at {} × {} and drawn at {} wide.",
                h.layer,
                settings::human_bytes(h.bytes),
                h.size.0,
                h.size.1,
                h.drawn_w
            );
            if ui::note_line(
                ui,
                icon::WARNING,
                color::warn(),
                &text,
                Some("Downscale to 2×"),
            ) {
                card.downscale.push((h.id.clone(), h.target));
            }
        }
        let mut kept = None;
        for (i, (_, (w, h))) in card.downscale.iter().enumerate() {
            let text = format!(
                "A picture is downscaled to {w} × {h} in this file; the document keeps the original."
            );
            if ui::note_line(
                ui,
                icon::CORNERS_IN,
                theme::text::dim(),
                &text,
                Some("Keep the original"),
            ) {
                kept = Some(i);
            }
        }
        if let Some(i) = kept {
            card.downscale.remove(i);
        }
    }

    /// The pictures of `carried` drawn far below their size, with what
    /// *Downscale to 2×* makes of each.
    fn heavy_pictures(&self, carried: &[(ImageId, u64)]) -> Vec<Heavy> {
        let doc = &self.session.doc;
        let mut out = Vec::new();
        for (id, bytes) in carried {
            let Some(entry) = doc.image(id) else { continue };
            let weight = self.picture_weight(id);
            let Some(drawn) = weight.drawn.filter(|d| *d < crate::pictures::HEAVY_BELOW) else {
                continue;
            };
            let at = |n: u32, k: f64| ((f64::from(n) * drawn * k).round() as u32).max(1);
            let name = layer_showing(doc, id).map_or_else(
                || "A layer".to_string(),
                |n| crate::private::layer(doc, n).into_owned(),
            );
            out.push(Heavy {
                id: id.clone(),
                layer: name,
                bytes: *bytes,
                size: (entry.width, entry.height),
                drawn_w: at(entry.width, 1.0),
                target: (at(entry.width, 2.0), at(entry.height, 2.0)),
            });
        }
        out
    }

    /// Measure the file for the card's settings on a worker, once per change of
    /// settings, document or downscaled pictures; repaint when the answer lands.
    fn measure_export(&mut self, ctx: &egui::Context, card: &mut ExportCard, revision: u64) {
        if let Some((key, job)) = &card.measuring {
            match job.poll() {
                Ok(Some(answer)) => {
                    card.measured = Some((key.clone(), answer));
                    card.measuring = None;
                }
                Ok(None) => {}
                Err(()) => card.measuring = None,
            }
        }
        let key = (card.settings.clone(), revision, card.downscale.clone());
        let have = card.measured.as_ref().is_some_and(|(k, _)| *k == key);
        let asked = card.measuring.as_ref().is_some_and(|(k, _)| *k == key);
        if have || asked {
            return;
        }
        let mut doc = self.session.doc.clone();
        let _ = doc.apply(&Transaction(vec![Operation::SetPlayExport {
            settings: card.settings.clone(),
        }]));
        let fonts = self.session.resolved.text_fonts();
        let list = card.downscale.clone();
        let job = Job::spawn("mastik-export-measure", ctx, move || {
            downscale(&mut doc, &list);
            measure(&doc, &fonts)
        });
        card.measuring = Some((key, job));
    }

    /// Draw the Video output's preview of `t` on a worker, once per change of
    /// take or options; land it as a texture.
    fn preview_take(
        &mut self,
        ctx: &egui::Context,
        card: &mut ExportCard,
        t: &crate::player::TakeRec,
    ) {
        if card
            .screen
            .as_ref()
            .is_none_or(|(i, _)| *i != card.video.take)
        {
            let size = mastik_export::video::screen_size(
                &self.session.doc,
                &t.take,
                crate::session::random_actor(),
            );
            card.screen = Some((card.video.take, size));
        }
        if let Some((pick, job)) = &card.previewing {
            match job.poll() {
                Ok(Some(img)) => {
                    let tex = img.map(|i| {
                        ctx.load_texture("export-take-preview", i, egui::TextureOptions::LINEAR)
                    });
                    card.preview = Some((*pick, tex));
                    card.previewing = None;
                }
                Ok(None) => {}
                Err(()) => card.previewing = None,
            }
        }
        // The preview is drawn small; its size has no bearing on what it shows.
        let pick = VideoPick {
            times: 1,
            format: Format::Mp4,
            ..card.video
        };
        let have = card.preview.as_ref().is_some_and(|(p, _)| *p == pick);
        let asked = card.previewing.as_ref().is_some_and(|(p, _)| *p == pick);
        if have || asked {
            return;
        }
        let doc = self.session.doc.clone();
        let fonts = self.session.resolved.text_fonts();
        let take = t.take.clone();
        // The first click shows its ring; a take with none shows its start.
        let at = take
            .inputs
            .iter()
            .find(|(_, i)| matches!(i, mastik_play::Input::PointerDown(_)))
            .map_or(0.0, |(at, _)| at + 120.0);
        let job = Job::spawn("mastik-take-preview", ctx, move || {
            mastik_core::text::register_font_data(&fonts);
            let options = pick.options();
            let (rgba, (w, h)) = mastik_export::video::still(
                &doc,
                &take,
                crate::session::random_actor(),
                &options,
                at,
            )
            .ok()?;
            Some(egui::ColorImage::from_rgba_unmultiplied(
                [w as usize, h as usize],
                &rgba,
            ))
        });
        card.previewing = Some((pick, job));
    }

    /// The name the export's files start with: the document's file name.
    fn export_stem(&self) -> String {
        self.session
            .path
            .as_ref()
            .and_then(|p| p.file_stem())
            .map_or_else(|| "Prototype".into(), |s| s.to_string_lossy().into_owned())
    }

    /// Ask where to write a file of `ext`, offering `name`.
    fn ask_path(&self, filter: &str, ext: &str, name: &str) -> Option<std::path::PathBuf> {
        rfd::FileDialog::new()
            .set_directory(self.export_start_dir())
            .add_filter(filter, &[ext])
            .set_file_name(name)
            .save_file()
    }

    /// Ask where to write the file, named `document — flow`, keep the settings
    /// with the document when they changed, and write it on an export worker.
    fn export_prototype_from_card(&mut self, card: ExportCard) {
        let settings = card.settings.clone();
        let flow = every_flow(&self.session.doc)
            .into_iter()
            .filter(|(r, _)| settings.flows.is_empty() || settings.flows.contains(r))
            .find(|(r, _)| settings.opens.is_none_or(|o| o == *r))
            .and_then(|(r, _)| {
                let page = self.session.doc.page(r.page)?;
                let at = page.flows.iter().position(|f| f.id == r.flow)?;
                Some(crate::private::flow(&page.flows[at].name, at).into_owned())
            })
            .unwrap_or_default();
        let name = match flow.is_empty() {
            true => format!("{}.html", self.export_stem()),
            false => format!("{} — {}.html", self.export_stem(), file_safe(&flow)),
        };
        let Some(path) = self.ask_path("HTML file", "html", &name) else {
            self.proto_export = Some(card);
            return;
        };
        if *self.session.doc.play_export() != settings {
            self.session
                .name_step("Export settings", crate::theme::icon::EXPORT);
            self.commit_edit(Transaction(vec![Operation::SetPlayExport {
                settings: settings.clone(),
            }]));
        }
        self.export_prototype_to(settings, card.downscale, path);
    }
}

/// The Video output's body: the take, its format and size, the clicks and the
/// device frame, and a preview.
fn video_body(
    ui: &mut egui::Ui,
    card: &mut ExportCard,
    takes: &[crate::player::TakeRec],
    note: &mut String,
    why_not: &mut Option<String>,
) {
    if takes.is_empty() {
        ui::empty_note(ui, "Record a take with the player's record button");
        *why_not = Some("No take recorded while playing".into());
        return;
    }
    card.video.take = card.video.take.min(takes.len() - 1);
    let v = &mut card.video;
    ui::labelled(ui, "Take", |ui| {
        let (shown, _) = take_words(&takes[v.take]);
        let w = ui.available_width();
        ui::fitted_combo(ui, "export-take", w, ui::combo_text(&shown), &shown, |ui| {
            ui::menu_rows(ui);
            // The newest first.
            for (i, t) in takes.iter().enumerate().rev() {
                let (words, length) = take_words(t);
                let row = ui::MenuRow::new("", &words)
                    .note(&length)
                    .selected(i == v.take);
                if ui::menu_row(ui, row, ui::MENU_ROW_H).clicked() {
                    v.take = i;
                }
            }
        });
    });
    let screen = card
        .screen
        .as_ref()
        .filter(|(i, _)| *i == v.take)
        .and_then(|(_, s)| *s);
    let half = (ui.available_width() - ui::CARD_COL_GAP) / 2.0;
    let column = egui::Layout::top_down(egui::Align::Min);
    ui.horizontal(|ui| {
        ui.spacing_mut().item_spacing.x = ui::CARD_COL_GAP;
        ui.allocate_ui_with_layout(egui::vec2(half, 0.0), column, |ui| {
            ui::labelled(ui, "Format", |ui| {
                let shown = match v.format {
                    Format::Mp4 => "MP4",
                    Format::Gif => "GIF",
                };
                ui::fitted_combo(
                    ui,
                    "export-video-format",
                    half,
                    ui::combo_text(shown),
                    shown,
                    |ui| {
                        ui::menu_rows(ui);
                        for (f, label, caption) in [
                            (Format::Mp4, "MP4", "H.264, small"),
                            (Format::Gif, "GIF", "loops, plays anywhere"),
                        ] {
                            let row = ui::MenuRow::new("", label)
                                .note(caption)
                                .selected(v.format == f);
                            if ui::menu_row(ui, row, ui::MENU_ROW_H).clicked() {
                                v.format = f;
                            }
                        }
                    },
                );
            });
        });
        ui.allocate_ui_with_layout(egui::vec2(half, 0.0), column, |ui| {
            ui::labelled(ui, "Size", |ui| {
                let words = |k: u32| match screen {
                    Some(s) => {
                        let (w, h) = mastik_export::video::framed_size(
                            (
                                (s.width * f64::from(k)).round() as u32,
                                (s.height * f64::from(k)).round() as u32,
                            ),
                            f64::from(k),
                            v.device,
                        );
                        format!("{k}× · {w} × {h}")
                    }
                    None => format!("{k}×"),
                };
                let shown = words(v.times);
                ui::fitted_combo(
                    ui,
                    "export-video-size",
                    half,
                    ui::combo_text(&shown),
                    &shown,
                    |ui| {
                        ui::menu_rows(ui);
                        for k in VIDEO_TIMES {
                            let label = words(k);
                            let row = ui::MenuRow::new("", &label).selected(v.times == k);
                            if ui::menu_row(ui, row, ui::MENU_ROW_H).clicked() {
                                v.times = k;
                            }
                        }
                    },
                );
            });
        });
    });
    if ui::switch_row(ui, "Show clicks", v.clicks, ui::CONTROL_H).clicked() {
        v.clicks = !v.clicks;
    }
    if ui::switch_row(ui, "Device frame", v.device, ui::CONTROL_H).clicked() {
        v.device = !v.device;
    }
    let w = ui.available_width();
    let (rect, _) = ui.allocate_exact_size(egui::vec2(w, PREVIEW_H), egui::Sense::hover());
    ui.painter().rect_filled(rect, ui::BUTTON_R, color::field());
    match card.preview.as_ref().and_then(|(_, t)| t.as_ref()) {
        Some(tex) => {
            let size = tex.size_vec2();
            let room = rect.shrink(PREVIEW_INSET).size();
            let k = (room.y / size.y).min(room.x / size.x);
            let at = egui::Rect::from_center_size(rect.center(), size * k);
            ui.painter().image(
                tex.id(),
                at,
                egui::Rect::from_min_max(egui::pos2(0.0, 0.0), egui::pos2(1.0, 1.0)),
                egui::Color32::WHITE,
            );
        }
        None => {
            let now = ui.input(|i| i.time);
            let ink = theme::text::faint();
            ui::paint_spinner(ui.painter(), rect.center(), ui::CONTROL_GLYPH_PT, ink, now);
            ui.ctx().request_repaint();
        }
    }
    let secs = takes[v.take].take.length / 1000.0;
    *note = format!("{secs:.1} s at {VIDEO_FPS} fps");
}

/// The Flow map output's body: which flows go in.
fn flow_map_body(
    ui: &mut egui::Ui,
    card: &mut ExportCard,
    all: &[(FlowRef, String)],
    why_not: &mut Option<String>,
) {
    if all.is_empty() {
        ui::empty_note(ui, "Add a flow in prototype mode to export a flow map");
        *why_not = Some("The document has no flow".into());
        return;
    }
    let refs: Vec<FlowRef> = all.iter().map(|(r, _)| *r).collect();
    ui::labelled(ui, "Flows", |ui| {
        if let Some(r) = flow_ticks(ui, all, &card.map_flows, "The map holds at least one flow") {
            let on = card.map_flows.is_empty() || card.map_flows.contains(&r);
            card.map_flows = toggled(&card.map_flows, &refs, r, !on);
        }
    });
    settings::caption_unled(
        ui,
        "Each flow's frames with their connectors, then every interaction in words",
    );
}

/// The flows of `all` as ticked rows, `list` (empty is every one) ticked, the
/// one left ticked alone fixed with `last_tip`. Answers the flow clicked.
fn flow_ticks(
    ui: &mut egui::Ui,
    all: &[(FlowRef, String)],
    list: &[FlowRef],
    last_tip: &str,
) -> Option<FlowRef> {
    let count = match list.is_empty() {
        true => all.len(),
        false => list.len(),
    };
    let w = ui.available_width();
    let mut clicked = None;
    // The rows touch, as a ticked list's do.
    ui.spacing_mut().item_spacing.y = 0.0;
    for (r, words) in all {
        let on = list.is_empty() || list.contains(r);
        let (tick, tip) = match on && count == 1 {
            true => (ui::Tick::Fixed, Some(last_tip)),
            false => (ui::Tick::Open(on), None),
        };
        let row = ui::tick_row(
            ui,
            w,
            tick,
            ui::RowMark::Glyph(icon::PLAY),
            words,
            ui::RowTail::None,
            tip,
        );
        if row.clicked() {
            clicked = Some(*r);
        }
    }
    clicked
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A document that runs a script looks for the scripts player before each
    /// plain one; any other looks only for the plain ones.
    /// flip: `player_folders` ignoring `scripts` → fails at "scripts first".
    #[test]
    fn a_scripted_export_looks_for_the_scripts_player_first() {
        let dirs = || {
            vec![
                std::path::PathBuf::from("target/player"),
                std::path::PathBuf::from("share/mastik/player"),
            ]
        };
        assert_eq!(player_folders(dirs(), false), dirs(), "plain only");
        let both: Vec<String> = player_folders(dirs(), true)
            .iter()
            .map(|d| d.to_string_lossy().replace('\\', "/"))
            .collect();
        assert_eq!(
            both,
            [
                "target/player-scripts",
                "target/player",
                "share/mastik/player-scripts",
                "share/mastik/player",
            ],
            "scripts first"
        );
    }

    fn flow(n: u64) -> FlowRef {
        let mut ids = mastik_core::IdSource::new(7);
        let page = ids.mint();
        let item = (0..n).map(|_| ids.mint_item()).last().expect("n ≥ 1");
        FlowRef { page, flow: item }
    }

    /// Every flow is the empty list; one off writes out the rest, and the last
    /// back on empties it again. The flow the file opens on goes with its flow.
    /// flip: `with_flow` keeping the full list when all are on → fails at "all on is empty".
    #[test]
    fn the_flow_list_keeps_every_flow_as_empty() {
        let all = [flow(1), flow(2), flow(3)];
        let s = PlayExport {
            opens: Some(all[1]),
            ..PlayExport::default()
        };
        let off = with_flow(&s, &all, all[1], false);
        assert_eq!(off.flows, vec![all[0], all[2]]);
        assert_eq!(off.opens, None, "the opening flow went out with it");
        let on = with_flow(&off, &all, all[1], true);
        assert!(on.flows.is_empty(), "all on is empty");
    }

    /// The HTML file and the flow map need a flow, so on a document without one
    /// the card does not open; a video needs none and opens on its output.
    /// flip: `open_export` refusing every output without a flow → fails at "a video opens".
    #[test]
    fn the_card_opens_on_the_output_asked_for() {
        let ctx = egui::Context::default();
        let mut app = MastikApp::headless(&ctx);
        app.open_export(Output::FlowMap);
        assert!(app.proto_export.is_none(), "no flow, no map");
        app.open_export(Output::Video);
        assert_eq!(
            app.proto_export.as_ref().map(|c| c.output),
            Some(Output::Video),
            "a video opens"
        );
    }

    /// A picture downscaled for the export is replaced in the copy by one of the
    /// size asked, and the layer showing it, and the table naming it, name the
    /// new one.
    /// flip: `downscale` not repointing the layers → fails at "the original is shown by nothing";
    /// not repointing the tables → fails at "the original goes" (kept, as still named).
    #[test]
    fn a_downscaled_picture_replaces_the_original_in_the_copy() {
        let mut ids = mastik_core::IdSource::new(0x31);
        let page = ids.mint();
        let mut doc = Document::new(page);
        let px = image::RgbaImage::from_pixel(400, 200, image::Rgba([200, 40, 40, 255]));
        let mut bytes = Vec::new();
        px.write_with_encoder(image::codecs::png::PngEncoder::new(&mut bytes))
            .expect("png");
        let old = crate::app::content_hash(&bytes);
        let rect = ids.mint();
        let table = ids.mint_data_set();
        doc.apply(&Transaction(vec![
            Operation::AddImage {
                id: old.clone(),
                entry: ImageEntry {
                    source: ImageSource::Embedded(bytes.into()),
                    format: mastik_core::ImageFormat::Png,
                    width: 400,
                    height: 200,
                },
            },
            Operation::CreateNode {
                id: rect,
                parent: page,
                index: 0,
                kind: mastik_core::NodeKind::Rect {
                    size: mastik_core::kurbo::Size::new(40.0, 20.0),
                    corner_radii: Default::default(),
                    corner_smoothing: 0.0,
                },
                transform: None,
                name: Some("Hero".into()),
            },
            Operation::SetFills {
                id: rect,
                fills: mastik_core::keyed_by_position([mastik_core::Fill {
                    brush: mastik_core::image_brush(old.clone()).into(),
                    visible: true,
                }]),
            },
            Operation::SetDataSet {
                id: table,
                set: Some(Box::new(mastik_core::list::DataSet {
                    id: table,
                    name: "People".into(),
                    table: mastik_core::chart::Table {
                        columns: vec![mastik_core::chart::Column {
                            id: mastik_core::chart::ColumnId(0),
                            name: "Photo".into(),
                        }],
                        rows: vec![vec!["a.png".into()]],
                    },
                    source: None,
                    pictures: vec![("a.png".into(), old.clone())],
                })),
            },
        ]))
        .expect("fixture");
        downscale(&mut doc, &[(old.clone(), (80, 40))]);
        let shown = layer_showing(&doc, &old);
        assert_eq!(shown, None, "the original is shown by nothing");
        assert!(!doc.has_image(&old), "the original goes");
        let set = doc.data_set(table).expect("the table");
        let row = set.picture(&doc, "a.png").expect("the row's picture");
        assert!(doc.has_image(&row), "the table's row draws a picture held");
        let new = doc
            .images()
            .map(|(id, _)| id.clone())
            .find(|id| *id != old)
            .expect("a new picture");
        assert_eq!(doc.image(&new).map(|e| e.width), Some(80));
        assert_eq!(
            layer_showing(&doc, &new),
            Some(rect),
            "the layer shows the new picture"
        );
    }
}
