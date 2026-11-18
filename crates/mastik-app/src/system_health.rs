//! *System health…* (Tools menu, palette): a card over the canvas reading how
//! well the selection, the page or every page uses the design system — how much
//! paint, type and spacing is bound to variables and styles, and what is left to
//! tidy. The readings are `mastik_export::review::health`'s, the ones MCP's
//! `health` and `mastik check health` print, so the numbers agree.
//!
//! **The card reads; other surfaces fix.** Each row's button opens the place that
//! already does the fix (Colour review, Bind other uses, the browser's *Unused*,
//! the kit update popover) or selects the layers; the card never edits. The
//! canvas stays live under it, and it reads again whenever the scope, the page,
//! the document or the selection changes, as Clean up plans again.

use crate::app::MastikApp;
use crate::batch_rename::{Scope, scope_segments};
use crate::theme::{self, color, icon};
use crate::ui::{self, FieldButton};
use eframe::egui;
use mastik_core::NodeId;
use mastik_export::review::{self, Health, Match};

/// The card's width: room for a tidy row's count, sentence and button.
const W: f32 = 420.0;
const PAD: f32 = crate::settings::CANVAS_CARD_PAD;
/// The air between the controls inside a section.
const GAP: f32 = 4.0;
/// A section's eyebrow row.
const EYEBROW_H: f32 = 28.0;
/// A *To tidy* row: a list row a control tall, so its button sits inside it.
const ROW_H: f32 = ui::CONTROL_H;
/// A tidy row's button, a segment cell tall inside the row.
const BUTTON_H: f32 = ui::SEGMENT_CELL_H;
/// A tidy row's count column, right-aligned.
const COUNT_W: f32 = 26.0;

/// The open card.
pub(crate) struct SystemHealth {
    scope: Scope,
    /// What *Selection* means this frame: the live selection, or [`Self::pin`]'s
    /// roots while a selection a row made stands.
    selection: Vec<NodeId>,
    /// The roots *Selection* read before a row's *Select* replaced the
    /// selection, and that selection: selecting what a row counts is no change
    /// of scope (Clean up's rule).
    pin: Option<(Vec<NodeId>, Vec<NodeId>)>,
    health: Health,
    kits: usize,
    /// What `health` was read for: the scope, the page, the revision and the
    /// selection.
    asked: Option<(Scope, NodeId, u64, Vec<NodeId>)>,
}

/// A *To tidy* row, in the card's order.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Row {
    RawMatch,
    NearDuplicates,
    OffScale,
    Detached,
    UnusedComponents,
    UnusedVariables,
    UnusedStyles,
    Kits,
}

impl Row {
    const ALL: [Row; 8] = [
        Row::RawMatch,
        Row::NearDuplicates,
        Row::OffScale,
        Row::Detached,
        Row::UnusedComponents,
        Row::UnusedVariables,
        Row::UnusedStyles,
        Row::Kits,
    ];

    fn glyph(self) -> &'static str {
        match self {
            Row::RawMatch => icon::LINK_SIMPLE,
            Row::NearDuplicates => icon::PALETTE,
            Row::OffScale => icon::RULER,
            Row::Detached => icon::LINK_BREAK,
            Row::UnusedComponents => icon::HEXAGON,
            Row::UnusedVariables => icon::BRACKETS_CURLY,
            Row::UnusedStyles => icon::STACK,
            Row::Kits => icon::ARROW_CIRCLE_DOWN,
        }
    }

    /// The sentence after the count, for one and for many.
    fn words(self, step: f64) -> (String, String) {
        let both = |one: &str, many: &str| (one.to_string(), many.to_string());
        match self {
            Row::RawMatch => both(
                "layer has a raw value a variable or style holds",
                "layers have raw values a variable or style holds",
            ),
            Row::NearDuplicates => both(
                "group of near-identical colours",
                "groups of near-identical colours",
            ),
            Row::OffScale => (
                format!("layer with a gap or radius off the {step}-unit scale"),
                format!("layers with gaps or radii off the {step}-unit scale"),
            ),
            Row::Detached => both(
                "layer detached from a component",
                "layers detached from a component",
            ),
            Row::UnusedComponents => both(
                "main component with no instances",
                "main components with no instances",
            ),
            Row::UnusedVariables => both("variable nothing uses", "variables nothing uses"),
            Row::UnusedStyles => both("unused style", "unused styles"),
            Row::Kits => both("kit update waiting", "kit updates waiting"),
        }
    }

    /// What the row's hover says.
    fn hint(self) -> &'static str {
        match self {
            Row::RawMatch => {
                "Values that match a variable or style exactly but are written raw. Binding them changes nothing on the canvas."
            }
            Row::NearDuplicates => {
                "Colours close enough to read as one, as Colour review groups them"
            }
            Row::OffScale => "Gaps, padding and corner radii not on a multiple of the scale's step",
            Row::Detached => "Counts layers detached since this was recorded",
            Row::UnusedComponents => "Main components no instance shows, here or on any page",
            Row::UnusedVariables => "This document's variables no layer, style or alias reads",
            Row::UnusedStyles => "This document's styles no layer uses",
            Row::Kits => "Kits this document uses that have a newer version",
        }
    }
}

/// What a frame of the card asked for.
enum Act {
    Close,
    Scope(Scope),
    /// Select these layers; the word names what they have, for the status.
    Select(Vec<NodeId>, &'static str),
    Row(Row),
}

impl SystemHealth {
    /// The roots the scope reads under: the selection, the page, or every page.
    fn roots(&self, doc: &mastik_core::Document, page: NodeId) -> Vec<NodeId> {
        match self.scope {
            Scope::Selection => mastik_core::build::outermost(doc, &self.selection),
            Scope::Page => vec![page],
            Scope::Document => review::every_page(doc, page),
        }
    }

    /// Read again when the scope, the page, the document or the selection moved.
    fn reread(&mut self, app: &MastikApp) {
        let rev = app.session.revision();
        let page = app.session.page;
        let now = (self.scope, page, rev, self.selection.clone());
        if self.asked.as_ref() == Some(&now) {
            return;
        }
        let doc = &app.session.doc;
        let roots = self.roots(doc, page);
        let scope = review::Scope::new(doc, &app.session.resolved, &roots);
        self.health = review::health(&scope, review::Options::default().step);
        self.kits = app.kits_waiting(false).len();
        self.asked = Some(now);
    }

    /// How many a row counts.
    fn count(&self, row: Row) -> usize {
        let h = &self.health;
        match row {
            Row::RawMatch => h.raw_match.count,
            Row::NearDuplicates => h.near_duplicates.count,
            Row::OffScale => h.off_scale.count,
            Row::Detached => h.detached.count,
            Row::UnusedComponents => h.unused_components.count,
            Row::UnusedVariables => h.unused_variables.len(),
            Row::UnusedStyles => h.unused_styles.len(),
            Row::Kits => self.kits,
        }
    }

    /// The word on a row's button. *Bind…* opens Bind other uses for the
    /// commonest variable match; a style match has no such door, so it selects.
    fn button(&self, row: Row) -> &'static str {
        match row {
            Row::RawMatch => match self.health.matches.first() {
                Some((Match::Variable(_), _)) => "Bind…",
                _ => "Select",
            },
            Row::NearDuplicates => "Review",
            Row::OffScale | Row::Detached | Row::UnusedComponents => "Select",
            Row::UnusedVariables | Row::UnusedStyles => "Show",
            Row::Kits => "Open",
        }
    }
}

impl MastikApp {
    /// *System health…*: open the card on the selection when there is one, else
    /// on every page. One card over the canvas's top at a time.
    pub(crate) fn open_system_health(&mut self) {
        self.palette = None;
        self.context_menu = None;
        self.open_menu = crate::app::TopMenu::None;
        if self.system_health.is_some() {
            return;
        }
        self.close_find();
        self.colour_review.open = false;
        self.batch_rename = None;
        self.clean_up = None;
        self.fill_content = None;
        self.chart_data = None;
        let selection = self.selected_layers();
        self.system_health = Some(SystemHealth {
            scope: match selection.is_empty() {
                true => Scope::Document,
                false => Scope::Selection,
            },
            selection,
            pin: None,
            health: Health::default(),
            kits: 0,
            asked: None,
        });
    }

    /// The whole document's coverage for the Tools menu's row, read once per
    /// revision: the coverage pass alone, not the checks.
    pub(crate) fn document_coverage(&self) -> Option<usize> {
        let rev = self.session.revision();
        if let Some((at, p)) = self.coverage_cache.get()
            && at == rev
        {
            return p;
        }
        let doc = &self.session.doc;
        let roots = review::every_page(doc, self.session.page);
        let p = review::overall(&review::Scope::new(doc, &self.session.resolved, &roots));
        self.coverage_cache.set(Some((rev, p)));
        p
    }

    /// Draw the open card and answer this frame's clicks.
    pub(crate) fn system_health_ui(&mut self, ctx: &egui::Context, canvas: egui::Rect) {
        let Some(mut c) = self.system_health.take() else {
            return;
        };
        let live = self.selected_layers();
        c.selection = match &c.pin {
            Some((roots, made)) if *made == live => roots.clone(),
            _ => {
                c.pin = None;
                live
            }
        };
        if c.selection.is_empty() && c.scope == Scope::Selection {
            c.scope = Scope::Document;
        }
        c.reread(self);
        let mut act = None;
        let hold = crate::settings::CardHold::Open;
        crate::settings::movable_canvas_card(ctx, "system-health", canvas, W, hold, |ui| {
            if crate::settings::card_head(ui, W, "System health") {
                act = Some(Act::Close);
            }
            egui::Frame::NONE
                .inner_margin(egui::Margin {
                    left: PAD as i8,
                    right: PAD as i8,
                    top: PAD as i8,
                    bottom: 8,
                })
                .show(ui, |ui| {
                    const SCOPES: [Scope; 3] = [Scope::Selection, Scope::Page, Scope::Document];
                    let lit = SCOPES.iter().position(|s| *s == c.scope).unwrap_or(2);
                    if let Some(i) = scope_segments(ui, lit, c.selection.len(), ui::SEGMENT_CELL_H)
                    {
                        act = Some(Act::Scope(SCOPES[i]));
                    }
                });
            ui::card_rule(ui);
            coverage(ui, &c.health, &mut act);
            ui::card_rule(ui);
            tidy(ui, &c, &mut act);
            ui::card_rule(ui);
            let note = format!(
                "Checked {} on {}",
                ui::count(c.health.checked, "layer"),
                ui::count(c.health.pages, "page")
            );
            if crate::settings::card_foot(ui, W, &note, &[("Close", FieldButton::Off)], None)
                .is_some()
            {
                act = Some(Act::Close);
            }
        });
        match act {
            Some(Act::Close) => return,
            Some(Act::Scope(s)) => c.scope = s,
            Some(Act::Select(ids, what)) => self.select_counted(&mut c, ids, what),
            Some(Act::Row(row)) if self.open_row(&mut c, row) => return,
            Some(Act::Row(_)) => {}
            None => {}
        }
        if self.system_health.is_none() {
            self.system_health = Some(c);
        }
    }

    /// A row's button: the door that fixes what it counts. True when that door
    /// closed the card, as Colour review's does, being a card over the canvas.
    fn open_row(&mut self, c: &mut SystemHealth, row: Row) -> bool {
        let h = &c.health;
        match row {
            Row::RawMatch => match h.matches.first().cloned() {
                Some((Match::Variable(var), _)) => self.open_bind_others(var),
                Some((Match::Style(_), ids)) => self.select_counted(c, ids, "a raw style"),
                None => {}
            },
            Row::NearDuplicates => {
                self.open_colour_review();
                return true;
            }
            Row::OffScale => {
                let ids = h.off_scale.ids.clone();
                self.select_counted(c, ids, "spacing off the scale");
            }
            Row::Detached => {
                let ids = h.detached.ids.clone();
                self.select_counted(c, ids, "no component");
            }
            Row::UnusedComponents => {
                let ids = h.unused_components.ids.clone();
                self.select_counted(c, ids, "no instances");
            }
            Row::UnusedVariables => self.open_unused(false),
            Row::UnusedStyles => self.open_unused(true),
            Row::Kits => self.open_menu = crate::app::TopMenu::KitUpdates,
        }
        false
    }

    /// Select what a row or a meter counts: those on this page, or on the page of
    /// the first when none is here. A locked layer is counted and left out.
    fn select_counted(&mut self, c: &mut SystemHealth, ids: Vec<NodeId>, what: &str) {
        let doc = &self.session.doc;
        let here: Vec<NodeId> = ids
            .iter()
            .copied()
            .filter(|id| doc.page_of(*id) == Some(self.session.page))
            .collect();
        let ids = match here.is_empty() {
            false => here,
            true => {
                let Some(page) = ids.first().and_then(|id| doc.page_of(*id)) else {
                    return;
                };
                let there: Vec<NodeId> = ids
                    .into_iter()
                    .filter(|id| doc.page_of(*id) == Some(page))
                    .collect();
                self.go_to_page(page);
                there
            }
        };
        let counted = ids.len();
        let taken: Vec<NodeId> = ids
            .into_iter()
            .filter(|id| self.may_select_in_bulk(*id))
            .collect();
        if taken.is_empty() {
            self.session.info(format!(
                "{} with {what} locked",
                match counted {
                    1 => "The 1 layer".to_string(),
                    n => format!("All {n} layers"),
                }
            ));
            return;
        }
        if c.scope == Scope::Selection && c.pin.is_none() {
            c.pin = Some((c.selection.clone(), taken.clone()));
        }
        let n = taken.len();
        self.entered_group = None;
        self.session.selection.set(taken);
        if let Some((_, made)) = &mut c.pin {
            *made = self.selected_layers();
        }
        self.session.info(match counted - n {
            0 => format!("Selected {} with {what}", ui::count(n, "layer")),
            l => format!("Selected {n} of {counted} layers with {what} — {l} locked"),
        });
        self.reveal_selection();
    }
}

/// A section's eyebrow, `note` faint on the right.
fn eyebrow(ui: &mut egui::Ui, label: &str, note: &str) {
    let (rect, _) = ui.allocate_exact_size(egui::vec2(W, EYEBROW_H), egui::Sense::hover());
    let p = ui.painter();
    let y = rect.bottom() - 10.0;
    ui::paint_eyebrow(
        p,
        egui::pos2(rect.left() + PAD, y),
        egui::Align2::LEFT_CENTER,
        label,
    );
    p.text(
        egui::pos2(rect.right() - PAD, y),
        egui::Align2::RIGHT_CENTER,
        note,
        egui::FontId::proportional(ui::CAPTION_PT),
        theme::text::dim(),
    );
}

/// *Bound to the system*: the three meters. A click selects the raw uses.
fn coverage(ui: &mut egui::Ui, h: &Health, act: &mut Option<Act>) {
    let total = h.paint.total + h.text.total + h.spacing.total;
    let note = match h.overall() {
        Some(p) => format!("{p}% of {}", ui::count(total, "use")),
        None => "Nothing to count".to_string(),
    };
    eyebrow(ui, "Bound to the system", &note);
    egui::Frame::NONE
        .inner_margin(egui::Margin {
            left: PAD as i8,
            right: PAD as i8,
            top: 4,
            bottom: 12,
        })
        .show(ui, |ui| {
            ui.spacing_mut().item_spacing.y = GAP;
            for (label, c, what) in [
                ("Paint", &h.paint, "raw paint"),
                ("Type", &h.text, "no text style"),
                ("Spacing and radii", &h.spacing, "raw spacing"),
            ] {
                let counts = format!("{} of {}", c.bound, c.total);
                let resp = ui::meter_readout(ui, W - 2.0 * PAD, label, &counts, (c.bound, c.total));
                let resp = match c.raw.len() {
                    0 => resp.on_hover_text("Every use is bound"),
                    n => resp
                        .on_hover_text(format!("Select the {} with {what}", ui::count(n, "layer"))),
                };
                if resp.clicked() && !c.raw.is_empty() {
                    *act = Some(Act::Select(c.raw.clone(), what));
                }
            }
        });
}

/// *To tidy*: every row, always, so the card keeps one height; a row with
/// nothing to report dims, shows its zero in the saved ink and has no button.
fn tidy(ui: &mut egui::Ui, c: &SystemHealth, act: &mut Option<Act>) {
    let open = Row::ALL.iter().filter(|r| c.count(**r) > 0).count();
    eyebrow(ui, "To tidy", &format!("{open} of {}", Row::ALL.len()));
    let step = review::Options::default().step;
    let button_w = Row::ALL
        .iter()
        .map(|r| crate::settings::text_button_w(ui, c.button(*r)))
        .fold(0.0, f32::max);
    egui::Frame::NONE
        .inner_margin(egui::Margin {
            left: (PAD - 6.0) as i8,
            right: (PAD - 6.0) as i8,
            top: 2,
            bottom: 10,
        })
        .show(ui, |ui| {
            ui.spacing_mut().item_spacing.y = 1.0;
            for row in Row::ALL {
                let n = c.count(row);
                let (rect, resp) = ui.allocate_exact_size(
                    egui::vec2(W - 2.0 * (PAD - 6.0), ROW_H),
                    egui::Sense::hover(),
                );
                resp.on_hover_text(row.hint());
                let live = n > 0;
                // The pointer over the row's button still lights the row.
                let over = ui.rect_contains_pointer(rect);
                ui::list_row_ground(ui.painter(), rect, false, live && over);
                let fade = |c: egui::Color32| match live {
                    true => c,
                    false => c.gamma_multiply(0.55),
                };
                let p = ui.painter();
                let y = rect.center().y;
                let mut x = rect.left() + 6.0;
                p.text(
                    egui::pos2(x + 7.0, y),
                    egui::Align2::CENTER_CENTER,
                    row.glyph(),
                    theme::icon_font(14.0),
                    fade(theme::text::dim()),
                );
                x += 14.0 + 9.0;
                let count_ink = match live {
                    true => color::text(),
                    false => fade(color::saved()),
                };
                p.text(
                    egui::pos2(x + COUNT_W, y),
                    egui::Align2::RIGHT_CENTER,
                    n.to_string(),
                    egui::FontId::proportional(ui::CONTROL_PT),
                    count_ink,
                );
                x += COUNT_W + 9.0;
                let (one, many) = row.words(step);
                let sentence = if n == 1 { one } else { many };
                let room = rect.right() - x - button_w - 10.0;
                let mut job = egui::text::LayoutJob::single_section(
                    sentence,
                    egui::TextFormat::simple(
                        egui::FontId::proportional(ui::SMALL_PT),
                        fade(theme::text::muted()),
                    ),
                );
                job.wrap = egui::text::TextWrapping::truncate_at_width(room.max(0.0));
                let galley = ui.fonts_mut(|f| f.layout_job(job));
                ui.painter().galley(
                    egui::pos2(x, y - galley.size().y / 2.0),
                    galley,
                    color::text(),
                );
                if !live {
                    continue;
                }
                let at = egui::Rect::from_center_size(
                    egui::pos2(rect.right() - 4.0 - button_w / 2.0, y),
                    egui::vec2(button_w, BUTTON_H),
                );
                let clicked = ui
                    .scope_builder(egui::UiBuilder::new().max_rect(at), |ui| {
                        ui::label_button(ui, c.button(row), at.size(), FieldButton::Off)
                    })
                    .inner
                    .clicked();
                if clicked {
                    *act = Some(Act::Row(row));
                }
            }
        });
}

#[cfg(test)]
mod tests {
    //! The card on a headless app: what it reads, and what its rows select.
    //!
    //! Plain backticks: a `#[cfg(test)]` module is invisible to `cargo doc`.
    use super::*;
    use mastik_core::{Document, IdSource};
    use serde_json::{Value, json};

    fn rect(page: &str, x: f64, fill: &str) -> Value {
        json!({ "verb": "insert_subtree", "args": { "parent": page, "tree": {
            "kind": { "Rect": { "size": { "width": 40, "height": 40 } } },
            "fills": [fill], "transform": [1, 0, 0, 1, x, 0] } } })
    }

    /// A page with a colour variable, two rects in its colour written raw, a
    /// main with one live instance and one detached; and the detached layer.
    fn fixture(ctx: &egui::Context) -> (MastikApp, NodeId) {
        crate::theme::install(ctx);
        let mut app = MastikApp::headless(ctx);
        let mut ids = IdSource::new(0x5a1);
        let page = ids.mint();
        let mut doc = Document::new(page);
        let p = page.to_wire();
        let mut main = rect(&p, 200.0, "#ff0000");
        main["as"] = json!("main");
        let steps = vec![
            json!({ "verb": "add_variable", "args": { "kind": "color", "name": "Brand",
                "value": { "color": "#3366cc" } } }),
            rect(&p, 0.0, "#3366cc"),
            rect(&p, 100.0, "#3366cc"),
            main,
            json!({ "verb": "make_component", "args": { "nodes": ["@main"] }, "as": "used" }),
            json!({ "verb": "create_instance", "args": { "main": "@used", "parent": p,
                "x": 300, "y": 0 } }),
            json!({ "verb": "create_instance", "args": { "main": "@used", "parent": p,
                "x": 400, "y": 0 }, "as": "gone" }),
            json!({ "verb": "detach_instance", "args": { "nodes": ["@gone"] } }),
        ];
        let made = mastik_core::verbs::run_script(
            &mut doc,
            &mut ids,
            mastik_core::verbs::Lent::default(),
            &Value::Array(steps),
        )
        .expect("the fixture builds");
        let gone = NodeId::from_wire(&made[6][0]).unwrap();
        app.session.adopt_document(doc, None);
        app.canvas_rect =
            egui::Rect::from_min_size(egui::pos2(0.0, 0.0), egui::vec2(1000.0, 700.0));
        (app, gone)
    }

    fn run(app: &mut MastikApp, ctx: &egui::Context) {
        let input = egui::RawInput {
            screen_rect: Some(egui::Rect::from_min_size(
                egui::pos2(0.0, 0.0),
                egui::vec2(1400.0, 900.0),
            )),
            ..Default::default()
        };
        let canvas = app.canvas_rect;
        let _ = ctx.run_ui(input, |_| app.system_health_ui(ctx, canvas));
    }

    /// With nothing selected the card reads every page and reads again after an
    /// edit, the Tools menu's coverage agreeing; its *Select* row selects the
    /// detached layer and keeps the card open.
    ///
    /// flip: `reread` keyed without the revision → fails at "read again".
    #[test]
    fn the_card_reads_the_document_and_follows_an_edit() {
        let ctx = egui::Context::default();
        let (mut app, gone) = fixture(&ctx);
        app.session.selection.set(Vec::new());
        app.open_system_health();
        run(&mut app, &ctx);
        let c = app.system_health.as_ref().expect("the card is open");
        assert_eq!(c.scope, Scope::Document, "nothing selected: every page");
        assert_eq!(c.health.detached.ids, vec![gone]);
        assert_eq!(c.health.paint.bound, 0, "nothing bound yet");
        assert_eq!(app.document_coverage(), c.health.overall());

        let raw = c.health.raw_match.ids.clone();
        assert_eq!(raw.len(), 2, "both #3366cc rects are Brand, written raw");
        let var = app.session.doc.variables().iter().next().unwrap().id;
        let tx = mastik_core::build::bind_field_ops(
            &app.session.doc,
            &raw[..1],
            mastik_core::build::BoundField::Fill(0),
            Some(var),
        );
        assert!(app.session.commit(tx));
        run(&mut app, &ctx);
        let c = app.system_health.as_ref().unwrap();
        assert_eq!(c.health.paint.bound, 1, "read again after the edit");
        assert_eq!(app.document_coverage(), c.health.overall());

        let mut c = app.system_health.take().unwrap();
        assert!(
            !app.open_row(&mut c, Row::Detached),
            "Select keeps the card"
        );
        app.system_health = Some(c);
        assert_eq!(app.session.selection.ids(), &[gone]);
        run(&mut app, &ctx);
        assert!(app.system_health.is_some());
    }

    /// Opened on a selection, the card reads the selection, and a row's
    /// *Select* does not narrow it to what it selected.
    ///
    /// flip: `select_counted` without its pin → fails at "the scope held".
    #[test]
    fn a_select_keeps_the_selection_scope() {
        let ctx = egui::Context::default();
        let (mut app, gone) = fixture(&ctx);
        let page = app.session.page;
        let all: Vec<NodeId> = app.session.doc.get(page).unwrap().children().to_vec();
        app.session.selection.set(all.clone());
        app.open_system_health();
        run(&mut app, &ctx);
        assert_eq!(app.system_health.as_ref().unwrap().scope, Scope::Selection);
        let mut c = app.system_health.take().unwrap();
        app.open_row(&mut c, Row::Detached);
        app.system_health = Some(c);
        run(&mut app, &ctx);
        let c = app.system_health.as_ref().unwrap();
        assert_eq!(app.session.selection.ids(), &[gone]);
        assert_eq!(c.scope, Scope::Selection);
        assert_eq!(c.selection.len(), all.len(), "the scope held");
    }
}
