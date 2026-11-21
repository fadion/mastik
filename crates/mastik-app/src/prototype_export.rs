//! The prototype's export card (`docs/design/prototyping-export.md`): which flows
//! go into the HTML file, the one it opens on, the player's chrome and scaling,
//! and the file's size measured before anything is written. The settings are
//! kept with the document, so a second export asks nothing.

use crate::app::MastikApp;
use crate::settings::{self, CardHold};
use crate::ui;
use eframe::egui;
use mastik_core::prototype::{FlowRef, OnPhone, PlayExport};
use mastik_core::{Document, Operation, Transaction};
use std::sync::mpsc::{Receiver, channel};

/// The card's width.
const W: f32 = 380.0;

/// The file's size in bytes, or why it cannot be written.
type Size = Result<u64, String>;

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

/// The card's state while it is open.
pub(crate) struct ExportCard {
    /// The settings as the card shows them.
    settings: PlayExport,
    /// The file's size for `settings` (and the revision it was measured at).
    measured: Option<(PlayExport, u64, Size)>,
    /// A measure under way, for the settings and revision it was asked for.
    measuring: Option<(PlayExport, u64, Receiver<Size>)>,
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

/// `settings` with `flow` in (`on`) or out of the file. An empty list is every
/// flow, so turning one off writes out the rest, and the last one back on
/// empties the list again.
pub(crate) fn with_flow(
    settings: &PlayExport,
    all: &[FlowRef],
    flow: FlowRef,
    on: bool,
) -> PlayExport {
    let mut list = match settings.flows.is_empty() {
        true => all.to_vec(),
        false => settings.flows.clone(),
    };
    list.retain(|f| *f != flow);
    if on {
        list.push(flow);
        list.sort_by_key(|f| all.iter().position(|a| a == f));
    }
    let mut out = settings.clone();
    out.flows = match list.len() == all.len() {
        true => Vec::new(),
        false => list,
    };
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

/// The file's size for `doc` under `settings`, packed as the export packs it.
fn measure(doc: &Document, fonts: &[mastik_core::peniko::FontData]) -> Result<u64, String> {
    mastik_core::text::register_font_data(fonts);
    let (wasm, glue) = crate::player_build(doc)?;
    let player = mastik_export::prototype::Player {
        wasm: &wasm,
        glue: &glue,
    };
    mastik_export::prototype::Parts::of(doc, player)
        .map(|p| p.estimate())
        .map_err(|e| e.to_string())
}

impl MastikApp {
    /// Open the export card, on the document's own settings.
    pub(crate) fn open_prototype_export(&mut self) {
        if every_flow(&self.session.doc).is_empty() {
            self.session
                .info("Add a flow in prototype mode to export a prototype".to_string());
            return;
        }
        self.proto_export = Some(ExportCard {
            settings: self.session.doc.play_export().clone(),
            measured: None,
            measuring: None,
        });
    }

    /// The export card, while open.
    pub(crate) fn prototype_export_ui(&mut self, ctx: &egui::Context, canvas: egui::Rect) {
        let Some(mut card) = self.proto_export.take() else {
            return;
        };
        let revision = self.session.revision();
        self.measure_export(ctx, &mut card, revision);
        let all = every_flow(&self.session.doc);
        let refs: Vec<FlowRef> = all.iter().map(|(r, _)| *r).collect();
        // The page the file opens on, whose own scaling *Page's own* names.
        let opens = card.settings.opens.or(refs.first().copied());
        let page_scaling = opens
            .and_then(|r| self.session.doc.page(r.page))
            .map(|p| p.play.scaling)
            .unwrap_or_default();
        let mut close = false;
        let mut export = false;
        let outside =
            settings::canvas_card(ctx, "prototype-export", canvas, W, CardHold::Modal, |ui| {
                if settings::card_head(ui, W, "Export prototype") {
                    close = true;
                }
                egui::Frame::NONE
                    .inner_margin(egui::Margin::symmetric(settings::CANVAS_CARD_PAD as i8, 4))
                    .show(ui, |ui| {
                        ui.spacing_mut().item_spacing.y = ui::CARD_ROW_GAP;
                        let s = &mut card.settings;
                        ui::labelled(ui, "Flows", |ui| {
                            let count = match s.flows.is_empty() {
                                true => refs.len(),
                                false => s.flows.len(),
                            };
                            for (r, words) in &all {
                                let on = s.flows.is_empty() || s.flows.contains(r);
                                // The file holds at least one flow.
                                let last = on && count == 1;
                                let row = ui
                                    .add_enabled_ui(!last, |ui| {
                                        ui::switch_row(ui, words, on, ui::CONTROL_H)
                                    })
                                    .inner
                                    .on_disabled_hover_text("The file holds at least one flow");
                                if row.clicked() {
                                    *s = with_flow(s, &refs, *r, !on);
                                }
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
                                .map_or("—".to_string(), |(_, w)| w.clone());
                            ui.scope(|ui| {
                                ui::combo_height(ui);
                                egui::ComboBox::from_id_salt("prototype-export-opens")
                                    .icon(ui::combo_chevron)
                                    .width(ui.available_width())
                                    .selected_text(shown)
                                    .show_ui(ui, |ui| {
                                        ui::menu_rows(ui);
                                        for (r, w) in &included {
                                            let on = s.opens == Some(*r);
                                            if ui.selectable_label(on, w).clicked() {
                                                s.opens = Some(*r);
                                            }
                                        }
                                    });
                            });
                        });
                        ui::labelled(ui, "Player", |ui| {
                            if ui::switch_row(ui, "Device frame", s.device_frame, ui::CONTROL_H)
                                .clicked()
                            {
                                s.device_frame = !s.device_frame;
                            }
                            if ui::switch_row(ui, "Player bar", s.player_bar, ui::CONTROL_H)
                                .clicked()
                            {
                                s.player_bar = !s.player_bar;
                            }
                            if ui::switch_row(ui, "Hotspot hints", s.hotspot_hints, ui::CONTROL_H)
                                .clicked()
                            {
                                s.hotspot_hints = !s.hotspot_hints;
                            }
                        });
                        ui::labelled(ui, "Scaling", |ui| {
                            let w = ui.available_width();
                            let salt = "export-scaling";
                            let page = Some(page_scaling);
                            if let Some(v) = crate::panels::prototype::scaling_combo(
                                ui, salt, w, page, s.scaling,
                            ) {
                                s.scaling = v;
                            }
                        });
                        let responsive = s.scaling.unwrap_or(page_scaling)
                            == mastik_core::prototype::Scaling::Responsive;
                        ui::labelled(ui, "On a phone", |ui| {
                            let w = ui.available_width();
                            let r = ui.add_enabled_ui(responsive, |ui| on_phone_combo(ui, w, s));
                            if !responsive {
                                r.response.on_disabled_hover_text(
                                    "Only a Responsive page turns with the phone",
                                );
                            }
                        });
                    });
                let (note, ready) = match &card.measured {
                    Some((m, rev, Ok(n))) if *m == card.settings && *rev == revision => {
                        (format!("About {}", settings::human_bytes(*n)), true)
                    }
                    Some((m, rev, Err(why))) if *m == card.settings && *rev == revision => {
                        (why.clone(), false)
                    }
                    _ => ("Measuring…".to_string(), true),
                };
                let why_not = (!ready).then_some(note.as_str());
                match settings::card_foot(
                    ui,
                    W,
                    &note,
                    &[
                        ("Cancel", ui::FieldButton::Off),
                        ("Export…", ui::FieldButton::Primary),
                    ],
                    why_not,
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
            self.export_prototype_from_card(card.settings);
            return;
        }
        if !close {
            self.proto_export = Some(card);
        }
    }

    /// Measure the file for the card's settings on a worker, once per change of
    /// settings or document; repaint when the answer lands.
    fn measure_export(&mut self, ctx: &egui::Context, card: &mut ExportCard, revision: u64) {
        if let Some((s, rev, rx)) = &card.measuring
            && let Ok(answer) = rx.try_recv()
        {
            card.measured = Some((s.clone(), *rev, answer));
            card.measuring = None;
        }
        let fresh = |m: &PlayExport, rev: u64| *m == card.settings && rev == revision;
        let have = card.measured.as_ref().is_some_and(|(m, r, _)| fresh(m, *r));
        let asked = card
            .measuring
            .as_ref()
            .is_some_and(|(m, r, _)| fresh(m, *r));
        if have || asked {
            return;
        }
        let mut doc = self.session.doc.clone();
        let _ = doc.apply(&Transaction(vec![Operation::SetPlayExport {
            settings: card.settings.clone(),
        }]));
        let fonts = self.session.resolved.text_fonts();
        let (tx, rx) = channel();
        let ctx = ctx.clone();
        std::thread::spawn(move || {
            let _ = tx.send(measure(&doc, &fonts));
            ctx.request_repaint();
        });
        card.measuring = Some((card.settings.clone(), revision, rx));
    }

    /// Ask where to write the file, keep the settings with the document when they
    /// changed, and write it on an export worker.
    fn export_prototype_from_card(&mut self, settings: PlayExport) {
        let stem = self
            .session
            .path
            .as_ref()
            .and_then(|p| p.file_stem())
            .map_or_else(|| "Prototype".into(), |s| s.to_string_lossy().into_owned());
        let Some(path) = rfd::FileDialog::new()
            .set_directory(self.export_start_dir())
            .add_filter("HTML file", &["html"])
            .set_file_name(format!("{stem}.html"))
            .save_file()
        else {
            self.proto_export = Some(ExportCard {
                settings,
                measured: None,
                measuring: None,
            });
            return;
        };
        if *self.session.doc.play_export() != settings {
            self.session
                .name_step("Export settings", crate::theme::icon::EXPORT);
            self.commit_edit(Transaction(vec![Operation::SetPlayExport {
                settings: settings.clone(),
            }]));
        }
        self.export_prototype_to(settings, path);
    }
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
}
