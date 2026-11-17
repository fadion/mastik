//! The kit chrome (§5.3e, §15 D1011–D1014) — the document menu that makes a
//! document a kit and the *Kit* tag that says it is one (here), the library's kits
//! as the picker reads them ([`index`]), and the floating insert picker
//! ([`picker`]).
//!
//! The model is `mastik_core::kit`; this module is the editor's face of it. A kit
//! is a document whose `meta.kit` mark is present and sharing
//! (`mastik_core::meta::KitMark`): marking writes that block and saves, because
//! the mark is an outward act — *Save* and *Export* are its company — and is not
//! undoable (§15 D1011 (2)).

pub(crate) mod card;
pub(crate) mod index;
pub(crate) mod picker;
pub(crate) mod publish;
pub(crate) mod summary;
pub(crate) mod updates;
pub(crate) mod used;
pub(crate) mod users;

use crate::app::{MastikApp, TOP_GAP, TopMenu};
use crate::menu::Item;
use crate::theme::{self, color, icon};
use eframe::egui;
use mastik_core::meta::KitMark;

/// The document menu's width.
const MENU_W: f32 = 232.0;

/// The document menu's area id, which its *Export ▸* list hangs from.
const DOC_MENU: &str = "document-menu";

/// A row of the document menu's *Export ▸* list, in its order: the files other
/// tools open, then the frames as images, then the tokens.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) enum DocExport {
    Pdf,
    Penpot,
    Sketch,
    Frames,
    FramesAllPages,
    Layers,
    Tokens,
}

impl DocExport {
    pub(crate) const ALL: [DocExport; 7] = [
        DocExport::Pdf,
        DocExport::Penpot,
        DocExport::Sketch,
        DocExport::Frames,
        DocExport::FramesAllPages,
        DocExport::Layers,
        DocExport::Tokens,
    ];

    /// The row's words: the menu's *Export* says the verb, so the row names what.
    pub(crate) fn label(self) -> &'static str {
        match self {
            DocExport::Pdf => "Frames as one PDF…",
            DocExport::Penpot => "Penpot file…",
            DocExport::Sketch => "Sketch file…",
            DocExport::Frames => "All frames…",
            DocExport::FramesAllPages => "Frames, all pages…",
            DocExport::Layers => "Layers set to export",
            DocExport::Tokens => "Tokens…",
        }
    }

    /// Whether a hairline separates this row from the one above.
    fn after_sep(self) -> bool {
        matches!(self, DocExport::Frames | DocExport::Tokens)
    }

    /// Why the row cannot run, shown dimmed with it; `None` when it can.
    /// `page_frames` is whether the page in front has a frame to export, and
    /// `any_exports` whether *Export all* would take a layer. *Frames, all
    /// pages…* is never dimmed, as on the canvas menu: it reads every page and
    /// says so itself when none has a frame.
    pub(crate) fn refusal(self, page_frames: bool, any_exports: bool) -> Option<&'static str> {
        match self {
            DocExport::Pdf | DocExport::Frames => {
                (!page_frames).then_some("No frames on the canvas")
            }
            DocExport::Layers => {
                (!any_exports).then_some("No layer in this document has export settings")
            }
            DocExport::FramesAllPages
            | DocExport::Penpot
            | DocExport::Sketch
            | DocExport::Tokens => None,
        }
    }
}

/// *Make this a kit*'s tooltip, in the menu and the palette alike.
pub(crate) const MAKE_KIT_TIP: &str = "Other documents can use its components once you publish it";

/// *Show in Icons tab*'s tooltip, in the menus and the palette alike.
pub(crate) const ICON_PACK_TIP: &str =
    "Offer its published components as icons in the assets browser, once switched on in Settings";

impl MastikApp {
    /// Whether the document vendors anything from a kit — when *Kits used…* is offered.
    pub(crate) fn uses_kits(&self) -> bool {
        self.session
            .doc
            .vault()
            .and_then(|v| self.session.doc.get(v))
            .is_some_and(|v| !v.children().is_empty())
    }

    /// This document's kit mark, when it is a kit that is sharing — what the top
    /// bar's tag and the menu's kit rows ask. A mark that stopped sharing is kept
    /// for its version and answers `None` here.
    pub(crate) fn shared_kit(&self) -> Option<&KitMark> {
        self.session
            .doc
            .meta()
            .kit
            .as_ref()
            .filter(|k| k.is_shared())
    }

    /// The document's title as the **document menu's head** (§5.3e) — the
    /// caret beside the name.
    ///
    /// ⚠️ **The menu holds only rows whose verbs exist**: *Rename* and *Duplicate*
    /// are left out, with no editor verb behind them yet, and a row that does
    /// nothing is worse than a short menu. *Version history…*, *Save named
    /// version…* and *Export ▸* lead, the document's own verbs above a
    /// separator; on a kit, *Publish kit…* heads the kit
    /// rows, disabled with *No changes* when there is nothing to publish; *Kits
    /// used…* is offered while the vault holds anything.
    pub(crate) fn document_menu(&mut self, ui: &mut egui::Ui, title: String) {
        let ctx = ui.ctx().clone();
        let open = self.open_menu == TopMenu::Document;
        // A head insets its content `HEAD_LEAD`; here the content is the name,
        // which has to stay 12 from the mark, as on the library screen. So the
        // head starts that much to the left and its hover ground reaches back into
        // the gap rather than moving the name.
        ui.add_space(-crate::ui::HEAD_LEAD);
        let head = crate::ui::menu_head(
            ui,
            egui::RichText::new(title)
                .size(crate::ui::HEAD_PT)
                .color(theme::text::strong()),
            TOP_GAP,
            open,
        )
        .on_hover_text("Document");
        if head.clicked() {
            self.toggle_menu(TopMenu::Document);
        }
        // The *Kit* tag is the top bar's, after the save dot (§15 D1025).
        let flyout_at = Self::flyout_id(DOC_MENU).with("row");
        if self.open_menu != TopMenu::Document {
            ctx.data_mut(|d| d.remove::<egui::Rect>(flyout_at));
            return;
        }

        // Left-aligned under the name, hanging from it.
        let anchor = egui::pos2(head.rect.left(), head.rect.bottom() + crate::ui::MENU_GAP);
        let shared = self.shared_kit().is_some();
        let publishable = self.kit_changes().is_some_and(|c| c.any());
        let uses_kits = self.uses_kits();
        // Labels and glyphs from the registry the palette reads too.
        let item = |ui: &mut egui::Ui, item: Item| {
            let spec = item.spec();
            crate::ui::menu_item(ui, spec.glyph, spec.label, crate::ui::MENU_ROW_H)
        };
        self.dropdown(&ctx, DOC_MENU, anchor, MENU_W, &head, |app, ui| {
            const H: f32 = crate::ui::MENU_ROW_H;
            // The document's own verbs, above the kit's.
            if item(ui, Item::VersionHistory).clicked() {
                app.open_menu = TopMenu::None;
                app.versions_open = true;
            }
            if item(ui, Item::SaveNamedVersion).clicked() {
                app.open_menu = TopMenu::None;
                app.save_named_version();
            }
            // *Credits…*, only while something asks to be credited: most documents
            // never carry one, and the card would have nothing to list.
            if app.has_credits() {
                let row =
                    crate::ui::MenuRow::new(Item::Credits.spec().glyph, Item::Credits.spec().label);
                if crate::ui::menu_row(ui, row, H).clicked() {
                    app.open_menu = TopMenu::None;
                    app.credits_open = true;
                }
            }
            // *Export ▸*, the document's own exports, lit while its list is open.
            let mut flyout = ui.ctx().data(|d| d.get_temp::<egui::Rect>(flyout_at));
            let export = crate::ui::submenu_row(
                ui,
                crate::ui::MenuRow::new(icon::EXPORT, "Export"),
                flyout.is_some(),
            );
            if export.hovered() || export.clicked() {
                flyout = Some(export.rect);
            }
            if let Some(row) = flyout {
                app.document_export_flyout(ui.ctx(), row);
            }
            crate::ui::menu_sep(ui);
            if shared {
                // *Publish kit…*, disabled with its reason when nothing would go
                // out.
                let row = crate::ui::MenuRow {
                    accel: if publishable {
                        Item::PublishKit.spec().accel
                    } else {
                        Some("No changes")
                    },
                    enabled: publishable,
                    ..crate::ui::MenuRow::new(icon::UPLOAD_SIMPLE, "Publish kit…")
                };
                if crate::ui::menu_row(ui, row, H).clicked() && publishable {
                    app.open_menu = TopMenu::None;
                    app.open_publish();
                }
            }
            // *Kits used…*, between the publish and the mark — offered
            // when the document vendors from any kit.
            if uses_kits && item(ui, Item::KitsUsed).clicked() {
                app.open_menu = TopMenu::None;
                app.kits_used_open = true;
            }
            if shared {
                let on = app.shared_kit().is_some_and(|k| k.icons);
                let spec = Item::IconPack.spec();
                let row = crate::ui::MenuRow::new(spec.glyph, spec.label).checked(on);
                if crate::ui::menu_row(ui, row, H)
                    .on_hover_text(ICON_PACK_TIP)
                    .clicked()
                {
                    app.open_menu = TopMenu::None;
                    app.set_icon_pack(!on);
                }
                // Asks first, as the card's row does — so an ellipsis, as on
                // this menu's other dialog rows.
                if item(ui, Item::StopSharingKit).clicked() {
                    app.unsharing_kit = Some(None);
                    app.open_menu = TopMenu::None;
                }
            } else if item(ui, Item::MakeKit)
                .on_hover_text(MAKE_KIT_TIP)
                .clicked()
            {
                app.make_kit();
                app.open_menu = TopMenu::None;
            }
            crate::ui::menu_sep(ui);
            if item(ui, Item::ShowInLibrary).clicked() {
                app.open_menu = TopMenu::None;
                app.go_to_dashboard();
            }
            // Any other row of this menu hovered closes the list.
            let elsewhere = ui
                .ctx()
                .pointer_hover_pos()
                .is_some_and(|p| ui.min_rect().contains(p) && !export.rect.contains(p));
            ui.ctx().data_mut(|d| match flyout.filter(|_| !elsewhere) {
                Some(row) => {
                    d.insert_temp(flyout_at, row);
                }
                None => d.remove::<egui::Rect>(flyout_at),
            });
        });
    }

    /// The document menu's *Export ▸* list beside `row`: the document's exports,
    /// each also in the palette. A row that cannot run is dimmed with its reason
    /// ([`DocExport::refusal`]); *Frames, all pages…* shows only when there is
    /// more than one page.
    fn document_export_flyout(&mut self, ctx: &egui::Context, row: egui::Rect) {
        const H: f32 = crate::ui::MENU_ROW_H;
        let page_frames = !self.exported_frames().is_empty();
        let any_exports = self.document_exports_anything();
        let pages = self.session.doc.pages().len() > 1;
        let rows: Vec<DocExport> = DocExport::ALL
            .into_iter()
            .filter(|r| pages || *r != DocExport::FramesAllPages)
            .collect();
        let labels: Vec<(&str, Option<&str>)> = rows.iter().map(|r| (r.label(), None)).collect();
        let w = crate::ui::menu_check_accel_width(ctx, &labels, false, crate::ui::MENU_PAD, 160.0);
        let seps = rows.iter().filter(|r| r.after_sep()).count();
        let mut picked = None;
        crate::ui::flyout(Self::flyout_id(DOC_MENU), row, w)
            .pad(crate::ui::MENU_PAD)
            .order(egui::Order::Foreground)
            .show(ctx, (rows.len(), seps), |ui| {
                for r in &rows {
                    if r.after_sep() {
                        crate::ui::menu_sep(ui);
                    }
                    let why = r.refusal(page_frames, any_exports);
                    let style = crate::ui::MenuRow::new("", r.label()).enabled(why.is_none());
                    let resp = crate::ui::menu_row(ui, style, H);
                    if let Some(why) = why {
                        resp.on_hover_text(why);
                    } else if resp.clicked() {
                        picked = Some(*r);
                    }
                }
            });
        let Some(r) = picked else {
            return;
        };
        self.open_menu = TopMenu::None;
        use crate::input::Action;
        use crate::panels::EditableFormat;
        match r {
            DocExport::Pdf => self.export_frames_pdf(),
            DocExport::Penpot => self.dispatch(ctx, Action::ExportEditable(EditableFormat::Penpot)),
            DocExport::Sketch => self.dispatch(ctx, Action::ExportEditable(EditableFormat::Sketch)),
            DocExport::Frames => self.export_frames(),
            DocExport::FramesAllPages => self.export_frames_all_pages(),
            DocExport::Layers => self.dispatch(ctx, Action::ExportAll),
            DocExport::Tokens => self.dispatch(ctx, Action::ExportTokens),
        }
    }

    /// *Make this a kit* (§5.3e): write the mark and save. One click and safe —
    /// nothing is shared until the first publish.
    ///
    /// **An unfiled document is given its identity here** (§15 D1013, D1010's
    /// premise (a)): a kit is found by `meta.id` through the library scan, so a
    /// mark on a document with none would be a kit nobody could ever find. The id
    /// is minted the way filing mints one (`library::store::file_document`), and
    /// the save that follows files a document that has no path yet. A mark that
    /// stopped sharing is turned back on with its version kept, so a re-made kit
    /// continues from its last publish.
    pub(crate) fn make_kit(&mut self) {
        if self.refused_by_compare() {
            return;
        }
        // The save below is of `session.doc`, which holds no typed text until
        // the session ends.
        self.finish_text_first();
        if mark_kit(&mut self.session).is_err() {
            return;
        }
        // A failed save's own message stands — the status line has one slot
        // (`D1013`'s fix); the mark is in the document and goes out with the
        // next save that works.
        if !self.save_file(false) {
            return;
        }
        let name = self.shown_title();
        self.session.info(made_kit_said(&name));
    }

    /// What a paste carrying kit mains says when the kit refuses it: an update
    /// owed for a main this document holds at an older version (§15 D1036 (7)),
    /// a kit built on this one (D1036 (2)), or a kit held here without a mode
    /// the copy read (§15 D1064).
    pub(crate) fn carry_refused(&self, e: &mastik_core::kit::KitError) -> String {
        use mastik_core::kit::KitError;
        let doc = &self.session.doc;
        match e {
            KitError::NeedsUpdate(held) => match self.kit_line(*held) {
                Some(line) => format!("Accept {}'s update first, then paste again", line.kit_name),
                None => "Accept the kit's update first, then paste again".to_string(),
            },
            KitError::Circular(own) => format!(
                "The pasted component uses this kit's {}, so it cannot be pasted here",
                match doc.get(*own) {
                    Some(_) => crate::private::layer(doc, *own),
                    None => "component".into(),
                }
            ),
            // The kit's modes held here at another version than the copy's
            // (§15 D1064, `tokens::plan`).
            KitError::Modes(kit) => {
                match doc
                    .variables()
                    .kits()
                    .get(kit)
                    .and_then(|k| k.name.as_deref())
                {
                    Some(name) => format!(
                        "Accept {}'s update first, then paste again",
                        crate::private::document(name, 0)
                    ),
                    None => "Accept the kit's update first, then paste again".to_string(),
                }
            }
            // The carry answers only the three above; this is for a future one,
            // worded rather than printed (D1024: no error prints its Debug).
            _ => "That paste could not be placed — the kit it carries is incomplete".to_string(),
        }
    }

    /// *Stop sharing as kit*: the mark stays, no longer sharing, so its
    /// version carries on if the document is made a kit again. Documents that use
    /// it keep their copies; they read it as no longer a kit. Reached through
    /// [`MastikApp::stop_sharing_dialog`], which asks first.
    pub(crate) fn stop_sharing_kit(&mut self) {
        if self.refused_by_compare() {
            return;
        }
        // As `make_kit`: the save holds no typed text until the session ends.
        self.finish_text_first();
        if unmark_kit(&mut self.session).is_err() {
            return;
        }
        // A failed save's own message stands — the status line has one slot
        // (`D1013`'s fix); the mark is in the document and goes out with the
        // next save that works.
        if !self.save_file(false) {
            return;
        }
        let name = self.shown_title();
        self.session.info(stopped_sharing_said(&name));
    }

    /// *Show in Icons tab* on the open kit: write the flag and save, as
    /// *Make this a kit* does. Nothing reaches a consumer's Icons tab until they
    /// switch the kit on in Settings, so it acts at once.
    pub(crate) fn set_icon_pack(&mut self, on: bool) {
        if self.refused_by_compare() {
            return;
        }
        // As `make_kit`: the save holds no typed text until the session ends.
        self.finish_text_first();
        let mut meta = self.session.doc.meta().clone();
        match meta.kit.as_mut() {
            Some(kit) if kit.is_shared() && kit.icons != on => kit.icons = on,
            _ => return,
        }
        self.session.doc.set_meta(meta);
        self.session.mark_unsaved();
        if !self.save_file(false) {
            return;
        }
        let name = self.shown_title();
        self.session.info(icon_pack_said(&name, on));
    }

    /// The card menu's *Show in Icons tab* on a library file: the open
    /// document's through [`Self::set_icon_pack`], a closed one written into
    /// its file and every open tab's copy patched, as [`Self::set_kit_at`] does.
    pub(crate) fn set_icon_pack_at(&mut self, path: &std::path::Path, on: bool) {
        if self.view != crate::app::View::Dashboard && self.session.path.as_deref() == Some(path) {
            self.set_icon_pack(on);
            self.library.refresh();
            return;
        }
        let Some(entry) = self.library.entry_at(path).cloned() else {
            return;
        };
        if let Err(e) = crate::library::store::set_kit_icons(&self.library.root, &entry, on) {
            self.session.fail(format!(
                "Could not change “{}”: {e}",
                self.library.shown_entry(&entry)
            ));
            return;
        }
        self.kit_mark_written(path);
        let name = self.library.shown_entry(&entry);
        self.session.info(icon_pack_said(&name, on));
    }

    /// After a kit mark was written into the file at `path`: the library read
    /// again, and every tab holding the file, parked ones included, given the
    /// mark from disk — a stale one would go back with that tab's next save.
    fn kit_mark_written(&mut self, path: &std::path::Path) {
        self.library.refresh();
        if let Some(on_disk) = self.library.entry_at(path).map(|e| e.meta.clone()) {
            for session in self.open_sessions() {
                if session.path.as_deref() != Some(path) {
                    continue;
                }
                let mut meta = session.doc.meta().clone();
                meta.id = on_disk.id.clone();
                meta.kit = on_disk.kit.clone();
                session.doc.set_meta(meta);
            }
        }
    }

    /// The card menu's *Make a kit* / *Stop sharing as kit* on a library file.
    /// The open document goes through the editor's own verbs, which save it; a
    /// closed one has its mark written into the file
    /// (`library::store::set_kit`). Stopping is confirmed first — this is
    /// what the confirmation runs.
    ///
    /// **On the dashboard every file is closed**, the one the session holds
    /// included: saving that copy here would write it over whatever reached the
    /// disk since (another window's publish). The file is changed on disk, and
    /// every open tab's copy has its mark patched to match, as the dashboard's
    /// rename and move patch them.
    pub(crate) fn set_kit_at(&mut self, path: &std::path::Path, sharing: bool) {
        if self.view != crate::app::View::Dashboard && self.session.path.as_deref() == Some(path) {
            if sharing {
                self.make_kit();
            } else {
                self.stop_sharing_kit();
            }
            self.library.refresh();
            return;
        }
        let Some(entry) = self.library.entry_at(path).cloned() else {
            return;
        };
        if let Err(e) = crate::library::store::set_kit(&self.library.root, &entry, sharing) {
            self.session.fail(format!(
                "Could not change “{}”: {e}",
                self.library.shown_entry(&entry)
            ));
            return;
        }
        self.kit_mark_written(path);
        let name = self.library.shown_entry(&entry);
        self.session.info(if sharing {
            made_kit_said(&name)
        } else {
            stopped_sharing_said(&name)
        });
    }

    /// *Stop sharing as kit*'s confirmation (§5.3e), for the document menu
    /// and the card menu alike: how many documents use the kit and what happens
    /// to them, and that making it a kit again continues from its last version.
    ///
    /// **The count is read by loading every library file on a worker** (§15
    /// D1021, [`users`]), started when the dialog opens and dropped when it
    /// closes, so each opening counts afresh; until it arrives the first line
    /// says it is checking ([`users::CHECKING`]).
    pub(crate) fn stop_sharing_dialog(&mut self, ctx: &egui::Context) {
        let Some(target) = self.unsharing_kit.clone() else {
            self.kit_users = None;
            return;
        };
        // The document menu's, or a card's for the file the editor holds — but
        // on the dashboard every file is read from the library, as
        // `set_kit_at` writes it there (§15 D1036 (6)).
        let here = target.as_deref().is_none_or(|p| {
            self.view != crate::app::View::Dashboard && self.session.path.as_deref() == Some(p)
        });
        let found = if here {
            self.session
                .doc
                .meta()
                .kit
                .as_ref()
                .map(|k| (self.shown_title(), k.version))
        } else {
            target
                .as_deref()
                .and_then(|p| self.library.entry_at(p))
                .and_then(|e| {
                    let name = self.library.shown_entry(e);
                    e.meta.kit.as_ref().map(|k| (name, k.version))
                })
        };
        let Some((name, version)) = found else {
            self.unsharing_kit = None;
            return;
        };
        let kit_id = if here {
            self.session.doc.meta().id.clone()
        } else {
            target
                .as_deref()
                .and_then(|p| self.library.entry_at(p))
                .and_then(|e| e.meta.id.clone())
        };
        if let Some(id) = kit_id
            && self.kit_users.as_ref().is_none_or(|u| u.kit != id)
        {
            // From the editor the library is as last scanned, and nothing
            // rescans a kit's own window, so without this a consumer filed since
            // goes uncounted and the card says no document uses the kit. The
            // dashboard refreshed on the way in.
            if self.view != crate::app::View::Dashboard {
                self.library.refresh();
            }
            // Every library file but the kit's own and its conflict copies,
            // which carry its id and never vendor it.
            let paths = self
                .library
                .entries
                .iter()
                .filter(|e| e.meta.id.as_deref() != Some(id.as_str()))
                .map(|e| e.path.clone())
                .collect();
            self.kit_users = Some(users::KitUsers::start(id, paths));
        }
        if self.kit_users.as_mut().is_some_and(|u| u.pass()) {
            ctx.request_repaint_after(std::time::Duration::from_millis(50));
        }
        let count = self.kit_users.as_ref().and_then(|u| u.count);
        let counting = self
            .kit_users
            .as_ref()
            .is_some_and(users::KitUsers::counting);
        let mut decision: Option<bool> = None;
        let modal = crate::settings::card("stop-sharing-kit", ctx, |ui| {
            ui.set_width(crate::ui::menu_inner_w(380.0, crate::settings::PAD));
            if crate::settings::modal_title(ui, &format!("Stop sharing {name} as a kit?")) {
                decision = Some(false);
            }
            crate::settings::title_gap(ui);
            let again = if version == 0 {
                "You can make it a kit again later.".to_string()
            } else {
                format!(
                    "You can make it a kit again later. Publishing then continues from v{version}."
                )
            };
            for line in [users::sentence(&name, count, counting), again] {
                ui.label(
                    egui::RichText::new(line)
                        .size(crate::ui::CONTROL_PT)
                        .color(theme::text::muted()),
                );
                ui.add_space(4.0);
            }
            if let Some(d) = crate::settings::modal_footer(ui, "Stop sharing", true) {
                decision = Some(matches!(d, crate::settings::Decision::Save));
            }
        });
        // `Escape` and the backdrop are *Cancel*, as on every card (§15 D525).
        if modal.should_close() {
            decision = Some(false);
        }
        match decision {
            Some(true) => {
                self.unsharing_kit = None;
                match target {
                    Some(path) => self.set_kit_at(&path, false),
                    None => self.stop_sharing_kit(),
                }
            }
            Some(false) => self.unsharing_kit = None,
            None => {}
        }
        if self.unsharing_kit.is_none() {
            self.kit_users = None;
        }
    }
}

/// *Make this a kit* on `s`'s document, before its save: the kit mark set to
/// sharing, its version kept, and an id minted for a document with none, as
/// filing mints one. Shared by the menu row and the agent's `make_kit`, and
/// refused with [`ALREADY_A_KIT`], nothing changed, for a kit already sharing.
pub(crate) fn mark_kit(s: &mut crate::session::EditorSession) -> Result<(), &'static str> {
    let mut meta = s.doc.meta().clone();
    if meta.kit.as_ref().is_some_and(KitMark::is_shared) {
        return Err(ALREADY_A_KIT);
    }
    if meta.id.is_none() {
        meta.id = Some(crate::library::ids::mint());
    }
    meta.kit.get_or_insert_with(KitMark::default).sharing = true;
    s.doc.set_meta(meta);
    s.mark_unsaved();
    Ok(())
}

/// *Stop sharing as kit* on `s`'s document, before its save: the mark kept, no
/// longer sharing. Refused with [`NOT_A_KIT`], nothing changed, unless it is
/// sharing.
pub(crate) fn unmark_kit(s: &mut crate::session::EditorSession) -> Result<(), &'static str> {
    let mut meta = s.doc.meta().clone();
    let Some(kit) = meta.kit.as_mut().filter(|k| k.is_shared()) else {
        return Err(NOT_A_KIT);
    };
    kit.sharing = false;
    s.doc.set_meta(meta);
    s.mark_unsaved();
    Ok(())
}

/// Why a kit row does not apply to a document that is not a sharing kit.
pub(crate) const NOT_A_KIT: &str = "This document is not a kit";

/// Why *Make this a kit* does not apply to a kit already sharing.
pub(crate) const ALREADY_A_KIT: &str = "This document is already a kit";

/// The status line after `name` is made a kit.
pub(crate) fn made_kit_said(name: &str) -> String {
    format!("{name} is a kit — nothing is shared until you publish it")
}

/// The status line after the kit `name` stops sharing.
pub(crate) fn stopped_sharing_said(name: &str) -> String {
    format!("{name} is no longer shared — documents using it keep their copies")
}

/// The status line after *Show in Icons tab* on the kit `name`.
fn icon_pack_said(name: &str, on: bool) -> String {
    match on {
        true => format!("{name} shows in the Icons tab of documents that switch it on"),
        false => format!("{name} no longer shows in the Icons tab"),
    }
}

/// The neutral *Kit* tag after the save dot: a package glyph and the word
/// on an 8% chip — so the author always knows this file reaches others. Neutral
/// rather than the component violet, which belongs to selection (§15 D1001,
/// D1011 (1)).
///
/// Publish's box exactly: [`crate::ui::TOOLBAR_H`] tall, 10 in from each end, 7
/// between glyph and word, the same corner and type — the two sit side by side,
/// and two heights would read as two sizes of one thing.
pub(crate) fn kit_tag(ui: &mut egui::Ui) {
    const PAD: f32 = 10.0;
    const GAP: f32 = 7.0;
    let scale = crate::ui::Scale::Toolbar;
    let ink = theme::text::strong();
    let glyph = egui::WidgetText::from(theme::icon_text(icon::PACKAGE, scale.glyph_pt(), ink))
        .into_galley(
            ui,
            Some(egui::TextWrapMode::Extend),
            f32::INFINITY,
            egui::TextStyle::Body,
        );
    let word = ui.ctx().fonts_mut(|f| {
        f.layout_no_wrap(
            "Kit".into(),
            egui::FontId::proportional(scale.text_pt()),
            ink,
        )
    });
    let w = PAD + glyph.size().x + GAP + word.size().x + PAD;
    let (rect, resp) =
        ui.allocate_exact_size(egui::vec2(w, crate::ui::TOOLBAR_H), egui::Sense::hover());
    let p = ui.painter();
    p.rect_filled(
        rect,
        egui::CornerRadius::same(crate::ui::BUTTON_R),
        color::text_a(20),
    );
    let mid = rect.center().y;
    p.galley(
        egui::pos2(rect.left() + PAD, mid - glyph.size().y / 2.0),
        glyph.clone(),
        ink,
    );
    p.galley(
        egui::pos2(
            rect.left() + PAD + glyph.size().x + GAP,
            mid - word.size().y / 2.0,
        ),
        word,
        ink,
    );
    resp.on_hover_text("This document is a kit: other documents can use what it publishes");
}

#[cfg(test)]
mod tests {
    use super::DocExport;

    /// With no frame on the page in front, the current-page frame rows dim and
    /// *Frames, all pages…* stays live, since another page may hold frames.
    ///
    /// flip: `DocExport::refusal` giving `FramesAllPages` the frame rows' reason →
    /// fails at "all pages stays live".
    #[test]
    fn the_export_rows_dim_for_their_own_reasons() {
        assert_eq!(
            DocExport::FramesAllPages.refusal(false, false),
            None,
            "all pages stays live"
        );
        for r in [DocExport::Pdf, DocExport::Frames] {
            assert_eq!(
                r.refusal(false, true),
                Some("No frames on the canvas"),
                "{r:?}"
            );
            assert_eq!(r.refusal(true, false), None, "{r:?}");
        }
        assert!(DocExport::Layers.refusal(true, false).is_some());
        assert_eq!(DocExport::Layers.refusal(false, true), None);
        for r in [DocExport::Penpot, DocExport::Sketch, DocExport::Tokens] {
            assert_eq!(r.refusal(false, false), None, "{r:?}");
        }
    }
}
