//! The List card (`docs/design/lists.md`, *The List card*): a list frame's
//! data set, which rows it draws, which column fills which property of its
//! template, and *Detach list*.

use crate::app::MastikApp;
use crate::chart_import::{LinkState, file_name};
use crate::theme::{self, color, icon};
use crate::ui::{self, Prefix, Scrub};
use eframe::egui;
use mastik_core::chart::ColumnId;
use mastik_core::list::{self, List, Rows, Target};
use mastik_core::variant::PropKind;
use mastik_core::{NodeId, Operation, Transaction};

/// A scrub that commits as it goes (the List card's *Rows*): its field, the tab
/// it runs on, the undo depth before its first step, the redo steps held aside,
/// and the serials of the steps it committed, so a cancel or release takes back
/// only its own steps and leaves the history as it found it.
pub(crate) struct LiveScrub {
    pub field: egui::Id,
    pub tab: Option<crate::tabs::TabId>,
    pub depth: usize,
    pub redo: mastik_core::history::RedoStack,
    pub steps: Vec<u64>,
}

impl MastikApp {
    /// Whether the live scrub still stands on top of the active tab's history:
    /// its tab is in front and the step on top is one it committed.
    pub(crate) fn live_scrub_holds(&self) -> bool {
        self.proto.live_scrub.as_ref().is_some_and(|s| {
            s.tab == self.tabs.active
                && self
                    .session
                    .history
                    .current()
                    .is_some_and(|c| s.steps.contains(&c))
        })
    }

    /// End the live scrub, taking back its steps and putting back the redo it
    /// held aside when it still holds; a scrub that no longer holds keeps its
    /// steps. Answers whether it was taken back.
    pub(crate) fn take_back_live_scrub(&mut self) -> bool {
        let holds = self.live_scrub_holds();
        let Some(s) = self.proto.live_scrub.take() else {
            return false;
        };
        if !holds {
            return false;
        }
        while self.session.history.undo_depth() > s.depth
            && self
                .session
                .history
                .current()
                .is_some_and(|c| s.steps.contains(&c))
            && self.session.take_back()
        {}
        if self.session.history.undo_depth() != s.depth {
            return false;
        }
        self.session.history.restore_redo(s.redo);
        true
    }
}

/// The rows modes in the segmented control's order, with their words.
const ROWS: [(&str, &str); 4] = [
    ("All", ALL_TIP),
    ("First", "The first rows only"),
    ("Row", "One row of the table, the same in play"),
    (
        "Opened",
        "The row opened in play by a tap on a list over the same data",
    ),
];

/// What the Filter and Sort sections say under a one-row mode.
const ONE_ROW: &str = "One row shows: filter and sort apply to All and First";

/// *All*'s tooltip. The segmented control's tips are `&'static str`, so the
/// number is written out and held to `list::MAX_DRAWN` when built.
const ALL_TIP: &str = "Every row, up to 200";
const _: () = assert!(list::MAX_DRAWN == 200, "ALL_TIP names MAX_DRAWN");

/// What the List and Chart cards say under a table dropdown whose table is gone.
pub(crate) const MISSING_TABLE: &str = "Its table is not in this document: pick one";

/// A picture as a row names it: the name of the first layer showing it (a
/// placed picture is named after its file), with its size; a picture no layer
/// shows by its size alone.
pub(crate) fn picture_words(doc: &mastik_core::Document, id: &mastik_core::ImageId) -> String {
    let Some(entry) = doc.image(id) else {
        return "Missing picture".into();
    };
    let size = format!("{} × {}", entry.width, entry.height);
    let shown_by = doc.node_ids().find_map(|n| {
        let node = doc.get(n)?;
        let (_, r) = mastik_core::variant::image_of(node)?;
        (r.id == *id && !node.name().is_empty()).then(|| node.name().to_string())
    });
    match shown_by {
        Some(name) => format!("{name} \u{b7} {size}"),
        None => size,
    }
}

/// The properties of `template` a column can fill, in the order the List card
/// and the import box show them: text, boolean and image, then the set's variant
/// properties, each with its name and the Properties card's glyph.
pub(crate) fn list_targets(
    doc: &mastik_core::Document,
    template: NodeId,
) -> Vec<(Target, String, &'static str)> {
    let mut out: Vec<(Target, String, &'static str)> =
        mastik_core::variant::instance_properties(doc, template)
            .into_iter()
            .filter_map(|p| {
                let glyph = match p.kind {
                    PropKind::Text => icon::TEXT_T,
                    PropKind::Boolean => icon::EYE,
                    PropKind::Image => icon::IMAGE,
                    PropKind::Swap | PropKind::Nested => return None,
                };
                Some((Target::Prop(p.id), p.name.clone(), glyph))
            })
            .collect();
    out.extend(
        list::variant_props(doc, template)
            .into_iter()
            .map(|p| (Target::Variant(p.name.clone()), p.name, icon::VARIANT_SET)),
    );
    out
}

impl MastikApp {
    /// The List card over list frame `id`; nothing for any other layer.
    pub(crate) fn inspector_list(&mut self, ui: &mut egui::Ui, id: NodeId) {
        let Some(now) = self.session.doc.get(id).and_then(|n| n.list()).cloned() else {
            return;
        };
        self.panel(ui, "List", None, |app, ui| {
            ui.spacing_mut().item_spacing.y = ui::CARD_ROW_GAP;
            app.list_data_rows(ui, id, &now);
            app.list_rows_rows(ui, id, &now);
            app.list_filter_rows(ui, id, &now);
            app.list_sort_rows(ui, id, &now);
            app.list_fill_rows(ui, id, &now);
            let size = egui::vec2(ui.available_width(), ui::CONTROL_H);
            let tip = crate::input::keys("Keep the rows as instances and drop the list (Ctrl+E)");
            if ui::action_button(
                ui,
                icon::STACK_SIMPLE,
                "Detach list",
                ui::FieldButton::Off,
                size,
            )
            .on_hover_text(tip)
            .clicked()
            {
                app.detach_lists(&[id]);
            }
        });
    }

    /// Write `next` over list `id`'s description as one undo step `label`,
    /// unless it is what the list holds.
    pub(crate) fn list_commit(&mut self, id: NodeId, label: &'static str, next: List) {
        if self.session.doc.get(id).and_then(|n| n.list()) == Some(&next) {
            return;
        }
        self.session.name_step(label, icon::TABLE);
        self.commit_edit(Transaction(vec![Operation::SetList {
            id,
            list: Some(Box::new(next)),
        }]));
    }

    /// *List fill*: `next` written over list `id`, and, while its data set is
    /// linked, the pictures named by the image columns this edit newly points a
    /// property at read beside the file in the same step.
    fn commit_list_fill(&mut self, id: NodeId, next: List) {
        let doc = &self.session.doc;
        let set = doc.data_set(next.data).cloned();
        let before = doc
            .get(id)
            .and_then(|n| n.list())
            .map(|l| crate::list_data::picture_columns(doc, id, &l.fill))
            .unwrap_or_default();
        let mut columns = crate::list_data::picture_columns(doc, id, &next.fill);
        columns.retain(|c| !before.contains(c));
        match set {
            Some(set) => {
                self.commit_list_onto(id, &set, next, &columns, false, "List fill");
            }
            None => self.list_commit(id, "List fill", next),
        }
    }

    /// The data set's dropdown, its summary, its file and its limits.
    fn list_data_rows(&mut self, ui: &mut egui::Ui, id: NodeId, now: &List) {
        use crate::list_data::{TablePick, TableRows, table_combo};
        let doc = &self.session.doc;
        let set = doc.data_set(now.data).cloned();
        let shown = set
            .as_ref()
            .map_or_else(|| "Missing data".to_string(), |s| s.name.clone());
        let w = ui.available_width();
        let pick = table_combo(
            ui,
            ("list-data", id),
            w,
            doc,
            &shown,
            Some(now.data),
            &TableRows::default(),
        );
        match pick {
            Some(TablePick::Set(d)) => self.list_onto_set(id, d),
            Some(TablePick::File) => self.list_data_dialog(id),
            _ => {}
        }
        let Some(set) = set else {
            ui::selection_note(ui, MISSING_TABLE);
            return;
        };
        let rows = set.table.rows.len();
        ui.label(
            egui::RichText::new(format!(
                "{} \u{b7} {}",
                ui::count_as(rows, "row", "rows"),
                ui::count_as(set.table.columns.len(), "column", "columns")
            ))
            .size(ui::SMALL_PT)
            .color(theme::text::muted()),
        );
        let size = egui::vec2(ui.available_width(), ui::CONTROL_H);
        if ui::action_button(ui, icon::TABLE, "Edit data…", ui::FieldButton::Off, size)
            .on_hover_text("Open the list's table over the canvas")
            .clicked()
        {
            self.open_list_data(id);
        }
        self.data_set_link_rows(ui, &set);
        let doc = &self.session.doc;
        let drawn = list::shown(doc, id).len();
        let passing = now.passing(&set.table, &|v| doc.resolve_var(id, v)).len();
        if rows == 0 {
            let said = "No rows: the template is hidden in play";
            ui::note_line(ui, icon::WARNING, color::warn(), said, None);
        } else if drawn == 0 {
            let said = "No row passes the filter: the template is hidden in play";
            ui::note_line(ui, icon::WARNING, color::warn(), said, None);
        }
        // Cut only when the cap stops short of what the mode asked for.
        let asked = match now.rows {
            Rows::All => passing,
            Rows::First(n) => passing.min(n as usize),
            Rows::Opened | Rows::Row(_) => 0,
        };
        if drawn < asked {
            let said = format!("Showing {drawn} of {passing} rows");
            ui::note_line(ui, icon::WARNING, color::warn(), &said, None);
        }
    }

    /// A linked data set's source line (*Linked to products.csv*, *Changed on
    /// disk*, *File not found*) and its *Update*, *Relink…* and *Unlink*, on the
    /// List and Chart cards; nothing while unlinked.
    pub(crate) fn data_set_link_rows(&mut self, ui: &mut egui::Ui, set: &list::DataSet) {
        use crate::chart_import::{LinkAct, link_rows};
        if let Some(link) = &set.source {
            let state = self.data_link_state(set.id).unwrap_or(LinkState::Current);
            let tip = match self.readers_of(set.id) {
                (0..=1, 0) | (0, 1) => format!("Pick the file again for {}", set.name),
                _ => format!(
                    "Pick the file again for the {} reading {}",
                    self.readers_words(set.id),
                    set.name
                ),
            };
            match link_rows(ui, state, &link.path, Some(&tip)) {
                Some(LinkAct::Update) => self.update_data_set(set.id),
                Some(LinkAct::Relink) => self.relink_data_set(set.id),
                Some(LinkAct::Unlink) => self.unlink_data_set(set.id),
                None => {}
            }
        }
    }

    /// *Rows*: every row, the first few, one fixed row, or the row opened in
    /// play.
    fn list_rows_rows(&mut self, ui: &mut egui::Ui, id: NodeId, now: &List) {
        if !self.card_fold(ui, "List rows", "Rows") {
            return;
        }
        let at = match now.rows {
            Rows::All => 0,
            Rows::First(_) => 1,
            Rows::Row(_) => 2,
            Rows::Opened => 3,
        };
        let pick = ui::segmented_tipped(
            ui,
            ui.available_width(),
            ui::SEGMENT_CELL_H,
            ROWS.len(),
            at,
            |_| true,
            |i| ROWS[i].1,
            |p, i, r, on| ui::segment_label(p, r, ROWS[i].0, on),
        );
        if let Some(i) = pick.filter(|i| *i != at) {
            let rows = match i {
                0 => Rows::All,
                1 => Rows::First(3),
                2 => Rows::Row(1),
                _ => Rows::Opened,
            };
            self.list_commit(
                id,
                "List rows",
                List {
                    rows,
                    ..now.clone()
                },
            );
        }
        match now.rows {
            Rows::First(n) => {
                // A list's description has no preview: a scrub commits each whole
                // step, so the copies follow it live, and the steps stand as one
                // undo step once released; a typed value waits for the valve.
                let rid = ui.id().with(("list-first", id));
                let mut v = self
                    .proto
                    .scrub
                    .filter(|(i, _)| *i == rid)
                    .map_or(f64::from(n), |(_, x)| x);
                let resp = ui::value_field(
                    ui,
                    egui::vec2(ui.available_width(), ui::CONTROL_H),
                    Prefix::Label("Rows"),
                    &mut v,
                    Scrub::whole(0.25).range(1.0..=list::MAX_DRAWN as f64),
                    |d| d.custom_formatter(ui::number(0)),
                )
                .on_hover_text("How many rows the list draws");
                let next = v.round().clamp(1.0, list::MAX_DRAWN as f64) as u32;
                let tx = match next != n {
                    true => Transaction(vec![Operation::SetList {
                        id,
                        list: Some(Box::new(List {
                            rows: Rows::First(next),
                            ..now.clone()
                        })),
                    }]),
                    false => Transaction::default(),
                };
                // Escape or a right-click takes the scrub's steps back
                // (`cancel_gesture`). A release takes them back too and commits
                // where the scrub ended as one step, however the run's time
                // window split them.
                let mine = |s: &Option<LiveScrub>| s.as_ref().is_some_and(|s| s.field == rid);
                // A step someone else committed over the run (an agent's call) ends
                // it where it stands: its steps stay, and nothing under the foreign
                // step is taken back.
                if mine(&self.proto.live_scrub) && !self.live_scrub_holds() {
                    self.proto.live_scrub = None;
                }
                if !resp.dragged()
                    && !self.gesture_cancelled
                    && mine(&self.proto.live_scrub)
                    && self.take_back_live_scrub()
                {
                    // Onto the list as the take-back left it: `now` was read from
                    // the scrubbed document, whose template the take-back replaced.
                    let back = self.session.doc.get(id).and_then(|n| n.list()).cloned();
                    if let Some(back) = back {
                        let rows = now.rows;
                        self.list_commit(id, "List rows", List { rows, ..back });
                    }
                }
                let tx = if resp.dragged() && !self.gesture_cancelled && !tx.0.is_empty() {
                    if !mine(&self.proto.live_scrub) {
                        self.proto.live_scrub = Some(LiveScrub {
                            field: rid,
                            tab: self.tabs.active,
                            depth: self.session.history.undo_depth(),
                            redo: self.session.history.take_redo(),
                            steps: Vec::new(),
                        });
                    }
                    self.session.name_step("List rows", icon::TABLE);
                    self.note_edit(Some(&resp), &tx);
                    self.session.commit_run(tx, "list-first-rows");
                    if let (Some(s), Some(step)) = (
                        self.proto.live_scrub.as_mut(),
                        self.session.history.current(),
                    ) && !s.steps.contains(&step)
                    {
                        s.steps.push(step);
                    }
                    Transaction::default()
                } else {
                    tx
                };
                self.proto_valve(rid, v, &resp, "List rows", icon::TABLE, tx);
            }
            Rows::Row(n) => self.list_row_pick(ui, id, now, n),
            Rows::Opened => {
                ui::selection_note(ui, "Shows the row opened in play, row 1 here");
            }
            Rows::All => {}
        }
    }

    /// *Row*'s dropdown: the table's rows by number and first cell, as the
    /// player's *Opened rows* lists them.
    fn list_row_pick(&mut self, ui: &mut egui::Ui, id: NodeId, now: &List, n: u32) {
        use crate::list_data::{ROWS_LISTED, row_words};
        let firsts: Vec<String> = self
            .session
            .doc
            .data_set(now.data)
            .map(|d| {
                d.table
                    .rows
                    .iter()
                    .map(|r| r.first().cloned().unwrap_or_default())
                    .collect()
            })
            .unwrap_or_default();
        // Past the end the list shows the last row, and the face says which.
        let shown_at = (n.max(1) as usize - 1).min(firsts.len().saturating_sub(1));
        let words = |i: usize| row_words(i, firsts.get(i).map(String::as_str));
        let face = match words(shown_at) {
            w if w.starts_with("Row ") => w,
            w => format!("Row {w}"),
        };
        let mut pick = None;
        let w = ui.available_width();
        ui::fitted_combo(
            ui,
            ("list-row", id),
            w,
            ui::combo_text(&face),
            &face,
            |ui| {
                ui::menu_rows(ui);
                for i in 0..firsts.len().min(ROWS_LISTED) {
                    let on = i == shown_at;
                    if ui.selectable_label(on, words(i)).clicked() && !on {
                        pick = Some(i);
                    }
                }
                if firsts.len() > ROWS_LISTED {
                    let all = firsts.len();
                    let said =
                        format!("Showing {ROWS_LISTED} of {all} rows: pick in the data window");
                    ui::menu_note(ui, &said);
                }
            },
        );
        if let Some((_, at, was)) = self.list_row_moved.as_ref().filter(|m| m.1 == n) {
            let was = if was.is_empty() { "an empty row" } else { was };
            let said = format!("Row {at} was {was} before rows were deleted above it");
            ui::note_line(ui, icon::INFO, theme::text::dim(), &said, None);
        }
        if let Some(i) = pick {
            self.list_row_moved = None;
            let rows = Rows::Row(i as u32 + 1);
            self.list_commit(
                id,
                "List rows",
                List {
                    rows,
                    ..now.clone()
                },
            );
        }
    }

    /// The column dropdown of a filter test, a sort or a fill: *None* when `none` is
    /// offered, then the table's columns; a column the table lacks shows
    /// *Missing column*. Answers the pick (`None` for *None*).
    fn list_column_combo(
        ui: &mut egui::Ui,
        salt: impl std::hash::Hash + std::fmt::Debug,
        w: f32,
        columns: &[(ColumnId, String)],
        now: Option<ColumnId>,
        none: bool,
    ) -> Option<Option<ColumnId>> {
        let shown = match now {
            None => "None".to_string(),
            Some(c) => columns
                .iter()
                .find(|(i, _)| *i == c)
                .map_or_else(|| "Missing column".to_string(), |(_, n)| n.clone()),
        };
        let mut pick = None;
        ui::fitted_combo(ui, salt, w, ui::combo_text(&shown), &shown, |ui| {
            ui::menu_rows(ui);
            if none && ui.selectable_label(now.is_none(), "None").clicked() {
                pick = Some(None);
            }
            for (c, n) in columns {
                if ui.selectable_label(now == Some(*c), n).clicked() {
                    pick = Some(Some(*c));
                }
            }
        });
        pick.filter(|p| *p != now)
    }

    /// The table's columns as the column dropdowns list them: id and name.
    fn columns_of(table: &mastik_core::chart::Table) -> Vec<(ColumnId, String)> {
        table
            .columns
            .iter()
            .map(|c| (c.id, c.name.clone()))
            .collect()
    }

    /// *Filter*: tests on columns joined by *All* or *Any*, each a column, a
    /// test word and a value. Dimmed under a one-row mode.
    fn list_filter_rows(&mut self, ui: &mut egui::Ui, id: NodeId, now: &List) {
        let count = now.filter.tests.len();
        if !self.card_fold_noted(ui, "List filter", "Filter", count, None) {
            return;
        }
        let one = now.rows.is_one_row();
        if one {
            ui::selection_note(ui, ONE_ROW);
        }
        let doc = &self.session.doc;
        let Some(set) = doc.data_set(now.data).cloned() else {
            return;
        };
        let columns = Self::columns_of(&set.table);
        let passing = now.passing(&set.table, &|v| doc.resolve_var(id, v)).len();
        let mut next: Option<List> = None;
        ui::disable_unless(ui, !one, |ui| {
            let w = ui.available_width();
            let half = (w - ui::CARD_COL_GAP) / 2.0;
            if count > 1 {
                let at = usize::from(now.filter.any);
                let pick = ui::segmented_tipped(
                    ui,
                    half,
                    ui::SEGMENT_CELL_H,
                    2,
                    at,
                    |_| true,
                    |i| ["Every test must pass", "Any test passing will do"][i],
                    |p, i, r, on| ui::segment_label(p, r, ["All", "Any"][i], on),
                );
                if let Some(i) = pick.filter(|i| *i != at) {
                    let mut l = now.clone();
                    l.filter.any = i == 1;
                    next = Some(l);
                }
            }
            for (i, t) in now.filter.tests.iter().enumerate() {
                // A test is two rows: a second gap sets one test apart from the
                // next, as between two strokes.
                if i > 0 {
                    ui.add_space(ui::CARD_ROW_GAP);
                }
                let numeric = set.table.is_numeric(t.column);
                ui.horizontal(|ui| {
                    ui.spacing_mut().item_spacing.x = ui::CARD_COL_GAP;
                    if let Some(Some(c)) = Self::list_column_combo(
                        ui,
                        ("list-test-col", id, i),
                        half,
                        &columns,
                        Some(t.column),
                        false,
                    ) {
                        let mut l = now.clone();
                        l.filter.tests[i].column = c;
                        next = Some(l);
                    }
                    let word = t.op.words();
                    let mut op = None;
                    ui::fitted_combo(
                        ui,
                        ("list-test-op", id, i),
                        half,
                        ui::combo_text(word),
                        word,
                        |ui| {
                            ui::menu_rows(ui);
                            for o in list::TestOp::ALL {
                                if o.is_number() && !numeric {
                                    continue;
                                }
                                if ui.selectable_label(o == t.op, o.words()).clicked() && o != t.op
                                {
                                    op = Some(o);
                                }
                            }
                        },
                    );
                    if let Some(o) = op {
                        let mut l = now.clone();
                        l.filter.tests[i].op = o;
                        next = Some(l);
                    }
                });
                ui.horizontal(|ui| {
                    ui.spacing_mut().item_spacing.x = ui::CARD_COL_GAP;
                    let fw = w - ui::CONTROL_H - ui::CARD_COL_GAP;
                    if t.op.takes_value() {
                        let key = super::prototype::DraftKey::ListTest(id, i);
                        let want = super::prototype::ExprWant::LiteralOr;
                        let reads = super::prototype::Reads::Vars;
                        if let Some(e) = self.expr_field(ui, key, &t.value, fw, want, reads) {
                            let mut l = now.clone();
                            l.filter.tests[i].value = e;
                            next = Some(l);
                        }
                    } else {
                        ui.add_space(fw);
                    }
                    if ui::field_button(ui, icon::X, ui::CONTROL_H, ui::FieldButton::Off)
                        .on_hover_text("Remove this test")
                        .clicked()
                    {
                        let mut l = now.clone();
                        l.filter.tests.remove(i);
                        next = Some(l);
                        // A test has no id of its own: a refused value's draft,
                        // keyed by place, would move onto the test taking it.
                        if self.proto.draft.as_ref().is_some_and(|d| {
                            matches!(d.key, super::prototype::DraftKey::ListTest(l, _) if l == id)
                        }) {
                            self.proto.draft = None;
                        }
                    }
                });
                if set.table.index_of(t.column).is_none() {
                    let said =
                        "This test reads a column the table no longer has, so it passes every row";
                    ui::note_line(ui, icon::WARNING, color::warn(), said, None);
                }
            }
            if count < list::MAX_TESTS && !columns.is_empty() {
                let size = egui::vec2(w, ui::CONTROL_H);
                if ui::action_button(ui, icon::PLUS, "Add test", ui::FieldButton::Off, size)
                    .on_hover_text("Show only the rows whose cell passes a test")
                    .clicked()
                {
                    let mut l = now.clone();
                    l.filter.tests.push(list::Test {
                        column: columns[0].0,
                        op: list::TestOp::Is,
                        value: Default::default(),
                    });
                    next = Some(l);
                }
            }
            if count > 0 {
                let said = format!(
                    "{passing} of {} pass here",
                    ui::count_as(set.table.rows.len(), "row", "rows")
                );
                ui::selection_note(ui, &said);
            }
        });
        if let Some(l) = next {
            self.list_commit(id, "List filter", l);
        }
    }

    /// *Sort*: a column and a direction, and one *Then by*. Dimmed under a
    /// one-row mode.
    fn list_sort_rows(&mut self, ui: &mut egui::Ui, id: NodeId, now: &List) {
        let doc = &self.session.doc;
        let Some(set) = doc.data_set(now.data) else {
            return;
        };
        let columns = Self::columns_of(&set.table);
        let note = now
            .sort
            .first()
            .and_then(|k| columns.iter().find(|(c, _)| *c == k.column))
            .map(|(_, n)| n.clone());
        if !self.card_fold_noted(ui, "List sort", "Sort", 0, note.as_deref()) {
            return;
        }
        let one = now.rows.is_one_row();
        if one {
            ui::selection_note(ui, ONE_ROW);
        }
        let mut next: Option<List> = None;
        ui::disable_unless(ui, !one, |ui| {
            let w = ui.available_width();
            // The direction takes two square cells; the column the rest.
            let dir_w = 2.0 * ui::CONTROL_H;
            let col_w = w - dir_w - ui::CARD_COL_GAP;
            let shown = (now.sort.len() + 1).min(list::MAX_SORT);
            for i in 0..shown {
                let key = now.sort.get(i);
                let label = if i == 0 { "Sort by" } else { "Then by" };
                ui::labelled(ui, label, |ui| {
                    ui.horizontal(|ui| {
                        ui.spacing_mut().item_spacing.x = ui::CARD_COL_GAP;
                        let salt = ("list-sort", id, i);
                        if let Some(c) = Self::list_column_combo(
                            ui,
                            salt,
                            col_w,
                            &columns,
                            key.map(|k| k.column),
                            true,
                        ) {
                            let mut l = now.clone();
                            match c {
                                Some(column) if i < l.sort.len() => l.sort[i].column = column,
                                Some(column) => l.sort.push(list::SortKey {
                                    column,
                                    descending: false,
                                }),
                                // *None* takes this key and any after it.
                                None => l.sort.truncate(i),
                            }
                            next = Some(l);
                        }
                        // Without a column the direction shows disabled, not as a gap.
                        let at = key.map_or(0, |k| usize::from(k.descending));
                        let pick = ui::disable_unless(ui, key.is_some(), |ui| {
                            ui::segmented_tipped(
                                ui,
                                dir_w,
                                ui::SEGMENT_CELL_H,
                                2,
                                at,
                                |_| true,
                                |d| ["Low to high, A to Z", "High to low, Z to A"][d],
                                |p, d, r, on| {
                                    let g = [icon::SORT_ASCENDING, icon::SORT_DESCENDING][d];
                                    ui::segment_glyph(p, r, g, on);
                                },
                            )
                        })
                        .inner;
                        if let Some(d) = pick.filter(|d| *d != at && key.is_some()) {
                            let mut l = now.clone();
                            l.sort[i].descending = d == 1;
                            next = Some(l);
                        }
                    });
                });
            }
        });
        if let Some(l) = next {
            self.list_commit(id, "List sort", l);
        }
    }

    /// *Fill*: one dropdown of columns per property of the template.
    fn list_fill_rows(&mut self, ui: &mut egui::Ui, id: NodeId, now: &List) {
        let doc = &self.session.doc;
        let Some(tpl) = list::template(doc, id) else {
            return;
        };
        let owner = mastik_core::component::main_of(doc, tpl)
            .map(|m| mastik_core::variant::set_of(doc, m).unwrap_or(m))
            .and_then(|m| doc.get(m))
            .map(|m| m.name().to_string())
            .unwrap_or_default();
        let targets: Vec<(Target, String)> = list_targets(doc, tpl)
            .into_iter()
            .map(|(t, name, _)| (t, name))
            .collect();
        let columns = doc
            .data_set(now.data)
            .map(|d| Self::columns_of(&d.table))
            .unwrap_or_default();
        let unmatched = list::unmatched_variants(doc, id);
        let no_picture = list::unmatched_pictures(doc, id);
        let picture_set = (!no_picture.is_empty())
            .then(|| doc.data_set(now.data).cloned())
            .flatten();
        let data_file = doc
            .data_set(now.data)
            .and_then(|d| d.source.as_ref())
            .map(|l| file_name(&l.path));
        let title = if owner.is_empty() {
            "Fill".to_string()
        } else {
            format!("Fill \u{b7} {owner}")
        };
        if !self.card_fold(ui, "List fill", &title) {
            return;
        }
        if targets.is_empty() {
            ui::selection_note(ui, crate::list_data::NO_FILL_TARGETS);
            return;
        }
        for (target, name) in targets {
            let now_col = now.column_for(&target);
            let mut pick: Option<Option<ColumnId>> = None;
            ui::labelled(ui, &name, |ui| {
                let w = ui.available_width();
                let salt = ("list-fill", id, name.as_str());
                pick = Self::list_column_combo(ui, salt, w, &columns, now_col, true);
            });
            if let Target::Variant(v) = &target
                && let Some((_, cells)) = unmatched.iter().find(|(p, _)| p == v)
            {
                let said = format!(
                    "{} name no {v} variant",
                    ui::count_as(cells.len(), "value", "values")
                );
                let tip = cells
                    .iter()
                    .map(|c| {
                        if c.is_empty() {
                            "an empty cell"
                        } else {
                            c.as_str()
                        }
                    })
                    .collect::<Vec<_>>()
                    .join(", ");
                ui::note_line_tip(ui, icon::WARNING, color::warn(), &said, &tip);
            }
            if matches!(target, Target::Prop(_))
                && let Some((_, cells)) = no_picture.iter().find(|(p, _)| *p == name)
            {
                let (missing, refused) = crate::list_data::split_refused(
                    picture_set.as_ref(),
                    cells.iter().map(String::as_str),
                );
                if !missing.is_empty() {
                    let tip = match &data_file {
                        Some(f) => format!("{}: looked for next to {f}", missing.join(", ")),
                        None => missing.join(", "),
                    };
                    let said = format!(
                        "{} not found",
                        ui::count_as(missing.len(), "picture", "pictures")
                    );
                    ui::note_line_tip(ui, icon::WARNING, color::warn(), &said, &tip);
                }
                if !refused.is_empty() {
                    let said = format!(
                        "{} not read",
                        ui::count_as(refused.len(), "picture", "pictures")
                    );
                    ui::note_line_tip(ui, icon::WARNING, color::warn(), &said, &refused.join("\n"));
                }
            }
            if let Some(col) = pick.filter(|c| *c != now_col) {
                let mut fill: Vec<list::Fill> = now
                    .fill
                    .iter()
                    .filter(|f| f.target != target)
                    .cloned()
                    .collect();
                if let Some(column) = col {
                    fill.push(list::Fill { target, column });
                }
                self.commit_list_fill(
                    id,
                    List {
                        fill,
                        ..now.clone()
                    },
                );
                return;
            }
        }
    }

    /// A folding section of a card (List, Prototype), headed by `ui::fold_rule`,
    /// its fold kept by `key`; answers whether it is open.
    pub(super) fn card_fold(&mut self, ui: &mut egui::Ui, key: &'static str, label: &str) -> bool {
        self.card_fold_noted(ui, key, label, 0, None)
    }

    /// [`Self::card_fold`] whose head carries a count or a short note
    /// (`ui::fold_rule_noted`).
    pub(super) fn card_fold_noted(
        &mut self,
        ui: &mut egui::Ui,
        key: &'static str,
        label: &str,
        count: usize,
        note: Option<&str>,
    ) -> bool {
        let open = !self.collapsed_panels.contains(key);
        if ui::fold_rule_noted(ui, label, open, count, note).clicked() {
            if open {
                self.collapsed_panels.insert(key);
            } else {
                self.collapsed_panels.remove(key);
            }
        }
        open
    }
}

#[cfg(test)]
mod tests {
    use crate::app::MastikApp;
    use eframe::egui;
    use mastik_core::chart::{Column, ColumnId, Table};
    use mastik_core::list::{self, DataSet, Source};
    use mastik_core::verbs::{self, Ctx};
    use mastik_core::{NodeId, NodeKind};
    use serde_json::{Value, json};
    use std::path::{Path, PathBuf};

    /// Run verb `name` on the app's document and commit it as the app does.
    fn verb(app: &mut MastikApp, name: &str, args: Value) -> Vec<NodeId> {
        let res = app.session.resolved.clone();
        let done = verbs::call(
            &mut Ctx {
                doc: &app.session.doc,
                res: &res,
                ids: &mut app.session.ids,
                images: None,
                contrast: None,
            },
            name,
            &args,
        )
        .unwrap_or_else(|e| panic!("{name}: {e}"));
        assert!(app.session.commit(done.tx), "{name} commits");
        done.created
    }

    /// A headless app holding a main `Row` with a text property `Title`, and an
    /// instance of it on the page; answers the app and the instance.
    fn with_row(ctx: &egui::Context) -> (MastikApp, NodeId) {
        crate::theme::install(ctx);
        let mut app = MastikApp::headless(ctx);
        let page = app.session.doc.first_page().to_wire();
        let frame = serde_json::to_value(NodeKind::Artboard {
            size: mastik_core::kurbo::Size::new(200.0, 40.0),
            corner_radii: Default::default(),
            corner_smoothing: 0.0,
        })
        .unwrap();
        let main = verb(
            &mut app,
            "create_node",
            json!({ "parent": page, "kind": frame, "name": "Row", "x": -4000 }),
        )[0];
        let text = verb(
            &mut app,
            "create_text",
            json!({ "parent": main.to_wire(), "text": "Name" }),
        )[0];
        verb(
            &mut app,
            "make_component",
            json!({ "nodes": [main.to_wire()] }),
        );
        verb(
            &mut app,
            "define_property",
            json!({ "node": text.to_wire(), "kind": "text", "name": "Title" }),
        );
        let inst = verb(
            &mut app,
            "create_instance",
            json!({ "main": main.to_wire(), "parent": page, "x": -3500, "y": 0 }),
        )[0];
        (app, inst)
    }

    fn pass(ctx: &egui::Context, app: &mut MastikApp) {
        let mut frame = eframe::Frame::_new_kittest();
        let input = egui::RawInput {
            screen_rect: Some(egui::Rect::from_min_size(
                egui::Pos2::ZERO,
                egui::vec2(1320.0, 820.0),
            )),
            ..Default::default()
        };
        let _ = ctx.run_ui(input, |ui| eframe::App::ui(app, ui, &mut frame));
    }

    /// A list committed through the app's door gets its rows in the same undo
    /// step, shows the List card beside the frame's Container card, and its
    /// copies are hit as the list.
    /// flip: `commit_inner` handing `regenerate_in_history` neither its list ops
    /// nor its dirty set → fails at "three rows"; either alone still finds the
    /// list, through the `SetList` or the reparented template.
    #[test]
    fn a_list_made_in_the_app_fills_its_rows_and_shows_its_card() {
        let ctx = egui::Context::default();
        let (mut app, inst) = with_row(&ctx);
        let set = DataSet {
            id: app.session.ids.mint_data_set(),
            name: "rows.csv".into(),
            table: Table {
                columns: vec![Column {
                    id: ColumnId(0),
                    name: "title".into(),
                }],
                rows: vec![vec!["A".into()], vec!["B".into()], vec!["C".into()]],
            },
            source: None,
            pictures: Vec::new(),
        };
        let (tx, l) = list::make(
            &app.session.doc,
            &app.session.resolved,
            &mut app.session.ids,
            inst,
            Source::New(set),
        )
        .unwrap();
        let depth = app.session.history.undo_depth();
        assert!(app.commit_edit(tx));
        let copies = app
            .session
            .doc
            .get(l)
            .unwrap()
            .list()
            .unwrap()
            .copies
            .clone();
        assert_eq!(copies.len() + 1, 3, "three rows");
        assert_eq!(app.session.history.undo_depth(), depth + 1, "one step");

        app.session.selection.set_one(l);
        for _ in 0..3 {
            pass(&ctx, &mut app);
        }
        let shown = &app.cards_shown;
        assert!(shown.contains(&"List"), "the List card: {shown:?}");
        assert!(shown.contains(&"Container"), "its layout stays: {shown:?}");

        let res = &app.session.resolved;
        let at = res.world_bounds(copies[1]).unwrap().center();
        let hits = mastik_core::hit_test(res, &app.session.doc, app.session.page, at, 0.0);
        assert_eq!(hits.first(), Some(&l), "a copy is hit as its list");

        app.undo();
        assert!(app.session.doc.get(l).is_none(), "one undo takes it all");
    }

    /// A list over a data set of `rows` titles around `inst`, committed as the
    /// app does; answers the list and the data set.
    fn listed(app: &mut MastikApp, inst: NodeId, rows: &[&str]) -> (NodeId, list::DataSetId) {
        let set = DataSet {
            id: app.session.ids.mint_data_set(),
            name: "rows.csv".into(),
            table: Table {
                columns: vec![Column {
                    id: ColumnId(0),
                    name: "title".into(),
                }],
                rows: rows.iter().map(|r| vec![(*r).to_string()]).collect(),
            },
            source: Some(mastik_core::chart::DataLink {
                path: std::env::temp_dir()
                    .join("mastik-nowhere")
                    .join("rows.csv")
                    .to_string_lossy()
                    .into_owned(),
                size: 0,
                modified: 0,
                header: Some(true),
            }),
            pictures: Vec::new(),
        };
        let id = set.id;
        let (tx, l) = list::make(
            &app.session.doc,
            &app.session.resolved,
            &mut app.session.ids,
            inst,
            Source::New(set),
        )
        .unwrap();
        assert!(app.commit_edit(tx));
        (l, id)
    }

    /// A row action on the main a list's rows are instances of fires in a list
    /// row, as one on the template does, so a new one acts on *This row*; a
    /// layer outside every list gets *Opened row*.
    /// flip: `in_list_over` not asking the main's instances → fails at "the
    /// main's layer".
    #[test]
    fn a_row_action_on_a_rows_main_acts_on_this_row() {
        use super::super::prototype::{ActionKind, in_list_over, new_action};
        use mastik_core::prototype::{Action, WhichRow};
        let ctx = egui::Context::default();
        let (mut app, inst) = with_row(&ctx);
        let main = mastik_core::component::main_of(&app.session.doc, inst).unwrap();
        let label = app.session.doc.get(main).unwrap().children()[0];
        let (_, set) = listed(&mut app, inst, &["A", "B"]);
        let doc = &app.session.doc;
        assert!(in_list_over(doc, inst, set), "the template");
        assert!(in_list_over(doc, label, set), "the main's layer");
        let page = doc.first_page();
        assert!(!in_list_over(doc, page, set), "outside every list");
        let row_of = |holder| match new_action(doc, holder, ActionKind::EditRow) {
            Some(Action::EditRow { table, row, .. }) => (table, row),
            other => panic!("{other:?}"),
        };
        assert_eq!(row_of(label), (set, WhichRow::This));
        assert_eq!(row_of(page), (set, WhichRow::Opened));
    }

    /// A filled property stays read-only while the template is among several
    /// selected roots, and turns editable over a table that draws no row.
    /// flip: `filled_column` asking only the first root → fails at "among roots".
    #[test]
    fn a_filled_property_is_read_only_among_roots_and_editable_over_no_rows() {
        let ctx = egui::Context::default();
        let (mut app, inst) = with_row(&ctx);
        let (l, set) = listed(&mut app, inst, &["A", "B"]);
        let doc = &app.session.doc;
        let fill = doc.get(l).unwrap().list().unwrap().fill[0].target.clone();
        assert!(
            matches!(fill, list::Target::Prop(_)),
            "the fixture fills Title"
        );
        let main = mastik_core::component::main_of(doc, inst).unwrap();
        let filled = |app: &MastikApp, roots: &[NodeId]| {
            crate::panels::variants::filled_column(&app.session.doc, roots, &fill)
        };
        assert!(filled(&app, &[inst]).is_some(), "the template alone");
        assert!(filled(&app, &[main, inst]).is_some(), "among roots");
        assert!(filled(&app, &[main]).is_none(), "the main is no template");
        let mut empty = app.session.doc.data_set(set).unwrap().clone();
        empty.table.rows.clear();
        assert!(app.commit_edit(mastik_core::Transaction(vec![
            mastik_core::Operation::SetDataSet {
                id: set,
                set: Some(Box::new(empty)),
            }
        ])));
        assert!(filled(&app, &[inst]).is_none(), "no row drawn");
    }

    /// *Relink…* reads the picked file into the same data set, so every list
    /// over it follows in one step, and a reordered file's columns keep their
    /// ids by name, which a chart on the set reads them by.
    /// flip: `replace_data_set_table` skipping `take_ids_by_name` → fails at
    /// "the old id"; the fills' remap is a second defence the flip leaves standing.
    #[test]
    fn relink_reads_into_the_data_set_for_every_list() {
        let ctx = egui::Context::default();
        let (mut app, inst) = with_row(&ctx);
        let (l, set) = listed(&mut app, inst, &["A", "B"]);
        let page = app.session.doc.first_page();
        let main = mastik_core::component::main_of(&app.session.doc, inst).unwrap();
        let other = verb(
            &mut app,
            "create_instance",
            json!({ "main": main.to_wire(), "parent": page.to_wire(), "x": -3000, "y": 0 }),
        )[0];
        let (tx, l2) = list::make(
            &app.session.doc,
            &app.session.resolved,
            &mut app.session.ids,
            other,
            Source::Existing(set),
        )
        .unwrap();
        assert!(app.commit_edit(tx));

        let dir = std::env::temp_dir().join(format!("mastik-relink-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("moved.csv");
        std::fs::write(&path, "note,title\nx,C\ny,D\nz,E\n").unwrap();
        let depth = app.session.history.undo_depth();
        app.start_chart_read(
            "moved.csv",
            crate::chart_import::Source::File(path.clone()),
            mastik_core::chart::table_io::Format::Csv,
            vec![Some(true)],
            crate::chart_import::Then::DataSet {
                set,
                path: path.to_string_lossy().into_owned(),
                relink: true,
            },
        );
        app.settle_chart_read();
        let _ = std::fs::remove_dir_all(&dir);

        let doc = &app.session.doc;
        let d = doc.data_set(set).expect("the same data set");
        assert_eq!(d.table.rows.len(), 3);
        assert!(d.source.as_ref().unwrap().path.ends_with("moved.csv"));
        assert_eq!(app.session.history.undo_depth(), depth + 1, "one step");
        let title = d
            .table
            .columns
            .iter()
            .find(|c| c.name == "title")
            .unwrap()
            .id;
        assert_eq!(
            title,
            ColumnId(0),
            "the old id, though the file moved it second"
        );
        for list_id in [l, l2] {
            let x = doc.get(list_id).unwrap().list().unwrap();
            assert_eq!(x.copies.len(), 2, "every list over it follows");
            assert_eq!(x.fill[0].column, title, "kept by name");
        }
    }

    /// Read file `csv` for `then`, as the app does, and wait for it to land.
    fn read_into(
        app: &mut MastikApp,
        name: &str,
        csv: &str,
        then: impl FnOnce(PathBuf) -> crate::chart_import::Then,
    ) {
        let dir = std::env::temp_dir().join(format!("mastik-read-{name}-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join(name);
        std::fs::write(&path, csv).unwrap();
        app.start_chart_read(
            name,
            crate::chart_import::Source::File(path.clone()),
            mastik_core::chart::table_io::Format::Csv,
            vec![Some(true)],
            then(path),
        );
        app.settle_chart_read();
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// A relink to a file without a filled column drops that fill and says so;
    /// relinking a set never linked is *Link table*; *Choose file…* on a list
    /// moves it onto a new set in one *List data* step, its fill kept by name.
    /// flip: `replace_data_set_table` not counting `lost` → fails at "lost its
    /// column"; reading `first_link` as false → fails at "Link table";
    /// `move_list_to_new_set` keeping the old set → fails at "the new set".
    #[test]
    fn a_relink_counts_a_lost_fill_and_a_first_link_is_link_table() {
        let ctx = egui::Context::default();
        let (mut app, inst) = with_row(&ctx);
        let (l, set) = listed(&mut app, inst, &["A", "B"]);
        assert!(
            !app.session
                .doc
                .get(l)
                .unwrap()
                .list()
                .unwrap()
                .fill
                .is_empty(),
            "fixture: title fills Title"
        );
        let depth = app.session.history.undo_depth();
        read_into(&mut app, "lost.csv", "note\nx\ny\n", |path| {
            crate::chart_import::Then::DataSet {
                set,
                path: path.to_string_lossy().into_owned(),
                relink: true,
            }
        });
        assert_eq!(app.session.history.undo_depth(), depth + 1, "one step");
        assert!(
            app.session
                .doc
                .get(l)
                .unwrap()
                .list()
                .unwrap()
                .fill
                .is_empty(),
            "the fill goes with its column"
        );
        let said = app.session.status().text.clone();
        assert!(
            said.ends_with("\u{b7} 1 fill lost its column"),
            "lost its column: {said}"
        );

        // A set made here, never linked: its first file links it.
        let main = mastik_core::component::main_of(&app.session.doc, inst).unwrap();
        let page = app.session.doc.first_page().to_wire();
        let other = verb(
            &mut app,
            "create_instance",
            json!({ "main": main.to_wire(), "parent": page, "x": -3000, "y": 0 }),
        )[0];
        let fresh = DataSet {
            id: app.session.ids.mint_data_set(),
            name: "Table 1".into(),
            table: Table {
                columns: vec![Column {
                    id: ColumnId(0),
                    name: "title".into(),
                }],
                rows: vec![vec!["A".into()]],
            },
            source: None,
            pictures: Vec::new(),
        };
        let fresh_id = fresh.id;
        let (tx, l2) = list::make(
            &app.session.doc,
            &app.session.resolved,
            &mut app.session.ids,
            other,
            Source::New(fresh),
        )
        .unwrap();
        assert!(app.commit_edit(tx));
        read_into(&mut app, "first.csv", "title\nC\n", |path| {
            crate::chart_import::Then::DataSet {
                set: fresh_id,
                path: path.to_string_lossy().into_owned(),
                relink: true,
            }
        });
        let label = app
            .session
            .history
            .done()
            .last()
            .unwrap()
            .note
            .label
            .clone();
        assert_eq!(label, "Link table", "Link table");

        let depth = app.session.history.undo_depth();
        read_into(&mut app, "moved.csv", "x,title\n1,D\n2,E\n", |path| {
            crate::chart_import::Then::ListData {
                list: l2,
                path,
                linkable: true,
            }
        });
        assert_eq!(
            app.session.history.undo_depth(),
            depth + 1,
            "one List data step"
        );
        let doc = &app.session.doc;
        let moved = doc.get(l2).unwrap().list().unwrap();
        assert_ne!(moved.data, fresh_id, "the new set");
        let new_set = doc
            .data_set(moved.data)
            .expect("the new set is in the document");
        let title = new_set
            .table
            .columns
            .iter()
            .find(|c| c.name == "title")
            .unwrap()
            .id;
        assert_eq!(
            moved.fill.first().map(|f| f.column),
            Some(title),
            "its fill kept by name"
        );
    }

    /// A refused value typed into a filter test does not move onto the test
    /// taking its place when a test above it is removed.
    /// flip: the remove button keeping the draft → fails at "not moved".
    #[test]
    fn a_refused_test_draft_stays_with_its_test() {
        let ctx = egui::Context::default();
        crate::theme::install(&ctx);
        let (mut app, inst) = with_row(&ctx);
        let (l, _) = listed(&mut app, inst, &["A", "B"]);
        let mut next = app.session.doc.get(l).unwrap().list().unwrap().clone();
        for v in ["\"A\"", "\"B\"", "\"C\""] {
            next.filter.tests.push(list::Test {
                column: ColumnId(0),
                op: list::TestOp::Is,
                value: mastik_core::prototype::Expr(v.into()),
            });
        }
        app.list_commit(l, "List filter", next);
        app.proto.draft = Some(
            crate::ui::Draft::new(
                &ctx,
                super::super::prototype::DraftKey::ListTest(l, 1),
                "zz(".into(),
            )
            .refuse("Not read".into()),
        );
        let frame = |app: &mut MastikApp, events: Vec<egui::Event>| {
            let input = egui::RawInput {
                screen_rect: Some(egui::Rect::from_min_size(
                    egui::Pos2::ZERO,
                    egui::vec2(400.0, 800.0),
                )),
                events,
                ..Default::default()
            };
            ctx.run_ui(input, |ui| {
                ui.set_max_width(280.0);
                let now = app.session.doc.get(l).unwrap().list().unwrap().clone();
                app.list_filter_rows(ui, l, &now);
            })
        };
        let at = |out: &egui::FullOutput, text: &str| -> Vec<egui::Pos2> {
            out.shapes
                .iter()
                .filter_map(|s| match &s.shape {
                    egui::Shape::Text(t) if t.galley.job.text == text => {
                        Some(t.galley.rect.translate(t.pos.to_vec2()).center())
                    }
                    _ => None,
                })
                .collect()
        };
        let mut out = frame(&mut app, vec![]);
        for _ in 0..3 {
            out = frame(&mut app, vec![]);
        }
        assert_eq!(at(&out, "zz(").len(), 1, "fixture: the refused draft shows");
        let mut xs = at(&out, crate::theme::icon::X);
        xs.sort_by(|a, b| a.y.total_cmp(&b.y));
        assert_eq!(xs.len(), 3, "fixture: a remove per test");
        let press = |pressed| egui::Event::PointerButton {
            pos: xs[0],
            button: egui::PointerButton::Primary,
            pressed,
            modifiers: Default::default(),
        };
        frame(
            &mut app,
            vec![egui::Event::PointerMoved(xs[0]), press(true)],
        );
        frame(&mut app, vec![press(false)]);
        let mut out = frame(&mut app, vec![]);
        for _ in 0..3 {
            out = frame(&mut app, vec![]);
        }
        let tests = &app.session.doc.get(l).unwrap().list().unwrap().filter.tests;
        assert_eq!(tests.len(), 2, "fixture: the first test removed");
        assert!(at(&out, "zz(").is_empty(), "not moved");
    }

    /// A number typed into *First*'s *Rows* field lands as one step, though the
    /// typed value reaches the field a frame after the valve's commit frame.
    /// flip: `list_rows_rows` ignoring the held value → fails at "landed" (`First(3)`).
    #[test]
    fn a_typed_first_count_lands() {
        use mastik_core::list::Rows;
        let ctx = egui::Context::default();
        crate::theme::install(&ctx);
        let (mut app, inst) = with_row(&ctx);
        let (l, _) = listed(&mut app, inst, &["A", "B", "C", "D", "E", "F", "G"]);
        let mut next = app.session.doc.get(l).unwrap().list().unwrap().clone();
        next.rows = Rows::First(3);
        app.list_commit(l, "List rows", next);
        let frame = |app: &mut MastikApp, events: Vec<egui::Event>| {
            let input = egui::RawInput {
                screen_rect: Some(egui::Rect::from_min_size(
                    egui::Pos2::ZERO,
                    egui::vec2(400.0, 800.0),
                )),
                events,
                ..Default::default()
            };
            ctx.run_ui(input, |ui| {
                ui.set_max_width(280.0);
                let now = app.session.doc.get(l).unwrap().list().unwrap().clone();
                app.list_rows_rows(ui, l, &now);
            })
        };
        let mut out = frame(&mut app, vec![]);
        for _ in 0..3 {
            out = frame(&mut app, vec![]);
        }
        let field = out
            .shapes
            .iter()
            .find_map(|s| match &s.shape {
                egui::Shape::Text(t) if t.galley.job.text == "3" => {
                    Some(t.galley.rect.translate(t.pos.to_vec2()).center())
                }
                _ => None,
            })
            .expect("fixture: the field shows 3");
        let press = |pos, pressed| egui::Event::PointerButton {
            pos,
            button: egui::PointerButton::Primary,
            pressed,
            modifiers: Default::default(),
        };
        let depth = app.session.history.undo_depth();
        frame(&mut app, vec![egui::Event::PointerMoved(field)]);
        frame(&mut app, vec![press(field, true)]);
        frame(&mut app, vec![press(field, false)]);
        frame(&mut app, vec![]);
        frame(&mut app, vec![egui::Event::Text("6".into())]);
        let away = egui::pos2(5.0, 780.0);
        frame(&mut app, vec![press(away, true)]);
        frame(&mut app, vec![press(away, false)]);
        for _ in 0..3 {
            frame(&mut app, vec![]);
        }
        let rows = app.session.doc.get(l).unwrap().list().unwrap().rows;
        assert_eq!(rows, Rows::First(6), "landed");
        assert_eq!(app.session.history.undo_depth() - depth, 1, "one step");
    }

    /// Scrubbing *First*'s *Rows* field writes the count while the pointer is
    /// still down, and the whole scrub is one undo step.
    /// flip: `list_rows_rows` sending a dragged value through the valve only →
    /// fails at "live, before the release".
    #[test]
    fn a_scrubbed_first_count_lands_while_dragging() {
        use mastik_core::list::Rows;
        let ctx = egui::Context::default();
        crate::theme::install(&ctx);
        let (mut app, inst) = with_row(&ctx);
        let (l, _) = listed(&mut app, inst, &["A", "B", "C", "D", "E", "F", "G"]);
        let mut next = app.session.doc.get(l).unwrap().list().unwrap().clone();
        next.rows = Rows::First(3);
        app.list_commit(l, "List rows", next);
        let frame = |app: &mut MastikApp, events: Vec<egui::Event>| {
            let input = egui::RawInput {
                screen_rect: Some(egui::Rect::from_min_size(
                    egui::Pos2::ZERO,
                    egui::vec2(400.0, 800.0),
                )),
                events,
                ..Default::default()
            };
            ctx.run_ui(input, |ui| {
                ui.set_max_width(280.0);
                let now = app.session.doc.get(l).unwrap().list().unwrap().clone();
                app.list_rows_rows(ui, l, &now);
            })
        };
        let mut out = frame(&mut app, vec![]);
        for _ in 0..3 {
            out = frame(&mut app, vec![]);
        }
        let field = out
            .shapes
            .iter()
            .find_map(|s| match &s.shape {
                egui::Shape::Text(t) if t.galley.job.text == "3" => {
                    Some(t.galley.rect.translate(t.pos.to_vec2()).center())
                }
                _ => None,
            })
            .expect("fixture: the field shows 3");
        let press = |pos, pressed| egui::Event::PointerButton {
            pos,
            button: egui::PointerButton::Primary,
            pressed,
            modifiers: Default::default(),
        };
        let rows = |app: &MastikApp| app.session.doc.get(l).unwrap().list().unwrap().rows;
        let depth = app.session.history.undo_depth();
        frame(&mut app, vec![egui::Event::PointerMoved(field)]);
        frame(&mut app, vec![press(field, true)]);
        // A quarter of a row a point: twelve points right is three rows more.
        let mut x = field.x;
        for _ in 0..6 {
            x += 2.0;
            frame(
                &mut app,
                vec![egui::Event::PointerMoved(egui::pos2(x, field.y))],
            );
        }
        frame(&mut app, vec![]);
        let held = rows(&app);
        assert!(
            matches!(held, Rows::First(n) if n > 3),
            "live, before the release: {held:?}"
        );
        frame(&mut app, vec![press(egui::pos2(x, field.y), false)]);
        for _ in 0..3 {
            frame(&mut app, vec![]);
        }
        assert_eq!(rows(&app), held, "the release keeps it");
        assert_eq!(app.session.history.undo_depth() - depth, 1, "one step");
    }

    /// A cancel mid-scrub (`cancel_gesture`, Escape's and a right-click's) puts
    /// *First*'s count back, leaves no undo step and keeps the redo it found.
    /// flip: `cancel_gesture` leaving `live_scrub` alone → fails at "back to 2".
    #[test]
    fn a_cancelled_first_count_scrub_puts_the_count_back() {
        use mastik_core::list::Rows;
        let ctx = egui::Context::default();
        crate::theme::install(&ctx);
        let (mut app, inst) = with_row(&ctx);
        let (l, _) = listed(&mut app, inst, &["A", "B", "C", "D", "E", "F", "G"]);
        let rows = |app: &MastikApp| app.session.doc.get(l).unwrap().list().unwrap().rows;
        let mut next = app.session.doc.get(l).unwrap().list().unwrap().clone();
        next.rows = Rows::First(2);
        app.list_commit(l, "List rows", next);
        // Read again: the commit made the copies anew.
        let mut next = app.session.doc.get(l).unwrap().list().unwrap().clone();
        next.rows = Rows::First(3);
        app.list_commit(l, "List rows", next);
        // One step undone, so there is a redo for the scrub to keep.
        assert!(app.session.undo());
        assert_eq!(rows(&app), Rows::First(2), "fixture: undone to 2");
        let frame = |app: &mut MastikApp, events: Vec<egui::Event>| {
            let input = egui::RawInput {
                screen_rect: Some(egui::Rect::from_min_size(
                    egui::Pos2::ZERO,
                    egui::vec2(400.0, 800.0),
                )),
                events,
                ..Default::default()
            };
            ctx.run_ui(input, |ui| {
                ui.set_max_width(280.0);
                let now = app.session.doc.get(l).unwrap().list().unwrap().clone();
                app.list_rows_rows(ui, l, &now);
            })
        };
        let mut out = frame(&mut app, vec![]);
        for _ in 0..3 {
            out = frame(&mut app, vec![]);
        }
        let field = out
            .shapes
            .iter()
            .find_map(|s| match &s.shape {
                egui::Shape::Text(t) if t.galley.job.text == "2" => {
                    Some(t.galley.rect.translate(t.pos.to_vec2()).center())
                }
                _ => None,
            })
            .expect("fixture: the field shows 2");
        let press = |pos, pressed| egui::Event::PointerButton {
            pos,
            button: egui::PointerButton::Primary,
            pressed,
            modifiers: Default::default(),
        };
        let depth = app.session.history.undo_depth();
        frame(&mut app, vec![egui::Event::PointerMoved(field)]);
        frame(&mut app, vec![press(field, true)]);
        let mut x = field.x;
        for _ in 0..6 {
            x += 2.0;
            frame(
                &mut app,
                vec![egui::Event::PointerMoved(egui::pos2(x, field.y))],
            );
        }
        assert!(
            matches!(rows(&app), Rows::First(n) if n > 2),
            "fixture: live while scrubbing"
        );
        assert!(app.cancel_gesture(&ctx), "fixture: the scrub is cancelled");
        assert_eq!(rows(&app), Rows::First(2), "back to 2");
        frame(&mut app, vec![]);
        frame(&mut app, vec![press(egui::pos2(x, field.y), false)]);
        app.end_released_gestures();
        for _ in 0..3 {
            frame(&mut app, vec![]);
        }
        assert_eq!(rows(&app), Rows::First(2), "the release adds nothing");
        assert_eq!(app.session.history.undo_depth(), depth, "no step");
        assert!(app.session.redo(), "the redo it found is kept");
        assert_eq!(rows(&app), Rows::First(3), "and redoes to 3");
    }

    /// A live *Rows* scrub takes back only its own steps: an agent's step landed
    /// mid-scrub survives the release and a cancel, a release over a split run
    /// still leaves one step, and no record outlives the release.
    /// flip: `take_back_live_scrub` and `live_scrub_holds` not checking the step
    /// on top → fails at "the agent's rename survives the release"; the release
    /// block left out → fails at "one step".
    #[test]
    fn a_first_count_scrub_takes_back_only_its_own_steps() {
        use mastik_core::list::Rows;
        let ctx = egui::Context::default();
        crate::theme::install(&ctx);
        let (mut app, inst) = with_row(&ctx);
        let (l, _) = listed(&mut app, inst, &["A", "B", "C", "D", "E", "F", "G"]);
        let mut next = app.session.doc.get(l).unwrap().list().unwrap().clone();
        next.rows = Rows::First(2);
        app.list_commit(l, "List rows", next);
        let frame = |app: &mut MastikApp, events: Vec<egui::Event>| {
            let input = egui::RawInput {
                screen_rect: Some(egui::Rect::from_min_size(
                    egui::Pos2::ZERO,
                    egui::vec2(400.0, 800.0),
                )),
                events,
                ..Default::default()
            };
            ctx.run_ui(input, |ui| {
                ui.set_max_width(280.0);
                let now = app.session.doc.get(l).unwrap().list().unwrap().clone();
                app.list_rows_rows(ui, l, &now);
            })
        };
        let press = |pos, pressed| egui::Event::PointerButton {
            pos,
            button: egui::PointerButton::Primary,
            pressed,
            modifiers: Default::default(),
        };
        let rows = |app: &MastikApp| app.session.doc.get(l).unwrap().list().unwrap().rows;
        let page = app.session.doc.first_page();
        let name = |app: &MastikApp| app.session.doc.get(page).unwrap().name().to_owned();
        // Press on the field showing `n`, scrub three frames, run `between`, scrub
        // three more; then release, or cancel. Answers the depth before the press.
        let scrub =
            |app: &mut MastikApp, n: u32, between: &dyn Fn(&mut MastikApp), cancel: bool| {
                let mut out = frame(app, vec![]);
                for _ in 0..3 {
                    out = frame(app, vec![]);
                }
                let text = n.to_string();
                let field = out
                    .shapes
                    .iter()
                    .find_map(|s| match &s.shape {
                        egui::Shape::Text(t) if t.galley.job.text == text => {
                            Some(t.galley.rect.translate(t.pos.to_vec2()).center())
                        }
                        _ => None,
                    })
                    .expect("fixture: the field shows its count");
                let depth = app.session.history.undo_depth();
                frame(app, vec![egui::Event::PointerMoved(field)]);
                frame(app, vec![press(field, true)]);
                let mut x = field.x;
                for _ in 0..6 {
                    x += 2.0;
                    frame(app, vec![egui::Event::PointerMoved(egui::pos2(x, field.y))]);
                }
                let before = rows(app);
                assert!(
                    matches!(before, Rows::First(m) if m > n),
                    "fixture: live before `between`"
                );
                between(app);
                for _ in 0..6 {
                    x += 2.0;
                    frame(app, vec![egui::Event::PointerMoved(egui::pos2(x, field.y))]);
                }
                assert_ne!(rows(app), before, "fixture: live after `between`");
                if cancel {
                    assert!(app.cancel_gesture(&ctx), "fixture: the scrub is cancelled");
                    frame(app, vec![]);
                }
                frame(app, vec![press(egui::pos2(x, field.y), false)]);
                app.end_released_gestures();
                for _ in 0..3 {
                    frame(app, vec![]);
                }
                depth
            };
        let rename = |to: &str| {
            let to = to.to_owned();
            move |app: &mut MastikApp| {
                app.session
                    .name_mcp_step("agent", "set", crate::theme::icon::TABLE);
                assert!(app.session.commit(mastik_core::Transaction(vec![
                    mastik_core::Operation::SetName {
                        id: page,
                        name: to.clone(),
                    }
                ])));
            }
        };

        // A run the time window split: the release still leaves one step.
        let depth = scrub(&mut app, 2, &|app| app.session.end_edit_run(), false);
        assert_eq!(app.session.history.undo_depth() - depth, 1, "one step");
        assert!(app.proto.live_scrub.is_none(), "live_scrub cleared");

        // An agent's step between two scrub frames: kept by the release.
        let n = match rows(&app) {
            Rows::First(n) => n,
            other => panic!("fixture: {other:?}"),
        };
        scrub(&mut app, n, &rename("Agent one"), false);
        assert_eq!(
            name(&app),
            "Agent one",
            "the agent's rename survives the release"
        );
        assert!(app.proto.live_scrub.is_none(), "cleared after the release");

        // And by a cancel, which takes back only the steps above it.
        let n = match rows(&app) {
            Rows::First(n) => n,
            other => panic!("fixture: {other:?}"),
        };
        scrub(&mut app, n, &rename("Agent two"), true);
        assert_eq!(
            name(&app),
            "Agent two",
            "the agent's rename survives a cancel"
        );
    }

    /// The layers tree offers no drop that takes a list's template out of its
    /// list, while a reorder inside the list stays legal.
    /// flip: `drop_is_legal` guarding only the destination → fails at "stays in
    /// its list".
    #[test]
    fn a_lists_template_cannot_be_dragged_out() {
        let ctx = egui::Context::default();
        let (mut app, inst) = with_row(&ctx);
        let (l, _) = listed(&mut app, inst, &["A", "B"]);
        let page = app.session.doc.first_page();
        let template = list::template(&app.session.doc, l).expect("the template");
        assert!(app.drop_is_legal(template, l), "a reorder in the list");
        assert!(!app.drop_is_legal(template, page), "stays in its list");
    }

    /// An *Update* whose read lands after *Unlink* writes nothing: the set stays
    /// unlinked with its table, and the landing adds no step.
    /// flip: `replace_data_set_table` not checking the set's link → fails at
    /// "stays unlinked".
    #[test]
    fn an_update_landing_after_unlink_writes_nothing() {
        let ctx = egui::Context::default();
        let (mut app, inst) = with_row(&ctx);
        let (_, set) = listed(&mut app, inst, &["A", "B"]);
        let dir = std::env::temp_dir().join(format!("mastik-unlinked-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("people.csv");
        std::fs::write(&path, "title\nC\nD\nE\n").unwrap();
        app.start_chart_read(
            "people.csv",
            crate::chart_import::Source::File(path.clone()),
            mastik_core::chart::table_io::Format::Csv,
            vec![Some(true)],
            crate::chart_import::Then::DataSet {
                set,
                path: path.to_string_lossy().into_owned(),
                relink: true,
            },
        );
        app.settle_chart_read();
        let rows = |app: &MastikApp| app.session.doc.data_set(set).unwrap().table.rows.len();
        assert_eq!(rows(&app), 3, "the fixture links the file");

        std::fs::write(&path, "title\nF\nG\nH\nI\n").unwrap();
        app.update_data_set(set);
        app.unlink_data_set(set);
        let depth = app.session.history.undo_depth();
        app.settle_chart_read();
        let _ = std::fs::remove_dir_all(&dir);

        let d = app.session.doc.data_set(set).unwrap();
        assert!(d.source.is_none(), "stays unlinked");
        assert_eq!(rows(&app), 3, "its table kept");
        assert_eq!(app.session.history.undo_depth(), depth, "no step");
    }

    /// `list_count` says how many rows a list draws, and *No rows* over an
    /// empty table.
    #[test]
    fn a_list_counts_its_rows() {
        let ctx = egui::Context::default();
        let (mut app, inst) = with_row(&ctx);
        let (l, set) = listed(&mut app, inst, &["A", "B", "C"]);
        let count = |app: &MastikApp| crate::list_data::list_count(&app.session.doc, l).0;
        assert_eq!(count(&app), "3 rows");
        let mut empty = app.session.doc.data_set(set).unwrap().clone();
        empty.table.rows.clear();
        assert!(app.commit_edit(mastik_core::Transaction(vec![
            mastik_core::Operation::SetDataSet {
                id: set,
                set: Some(Box::new(empty)),
            }
        ])));
        assert_eq!(count(&app), "No rows");
    }

    /// Deleting a row above a *Row* list's row records what the row was for the
    /// List card's note, and a gutter click in its data window picks a new row
    /// and drops the note.
    /// flip: `commit_data_table` not calling `row_moved` → fails at "noted".
    #[test]
    fn a_fixed_row_moved_by_a_delete_is_noted_and_a_gutter_click_repicks() {
        use mastik_core::list::{List, Rows};
        let ctx = egui::Context::default();
        let (mut app, inst) = with_row(&ctx);
        let (l, set) = listed(&mut app, inst, &["A", "B", "C"]);
        let now = app.session.doc.get(l).unwrap().list().unwrap().clone();
        app.list_commit(
            l,
            "List rows",
            List {
                rows: Rows::Row(2),
                ..now
            },
        );
        let mut table = app.session.doc.data_set(set).unwrap().table.clone();
        table.rows.remove(0);
        app.commit_data_table(set, table);
        assert_eq!(
            app.list_row_moved,
            Some((l, 2, "B".to_string())),
            "noted: row 2 was B before row 1 went"
        );
        app.list_row_from_gutter(l, 0);
        let rows = app.session.doc.get(l).unwrap().list().unwrap().rows;
        assert_eq!(rows, Rows::Row(1), "the gutter's row");
        assert_eq!(app.list_row_moved, None, "the note goes with a new pick");
    }

    /// A list inside a frame wears its tab, and so hangs an annotation's mark
    /// after it, only while selected or empty; unselected with rows, it has no
    /// tab and its mark goes outside its corner like any layer's.
    /// flip: `frame_label` answering for every list → fails at "no tab".
    #[test]
    fn a_nested_list_has_a_tab_only_while_selected_or_empty() {
        let ctx = egui::Context::default();
        let (mut app, inst) = with_row(&ctx);
        let page = app.session.doc.first_page().to_wire();
        let frame = serde_json::to_value(NodeKind::Artboard {
            size: mastik_core::kurbo::Size::new(600.0, 600.0),
            corner_radii: Default::default(),
            corner_smoothing: 0.0,
        })
        .unwrap();
        let screen = verb(
            &mut app,
            "create_node",
            json!({ "parent": page, "kind": frame, "name": "Screen", "x": 500, "y": 400 }),
        )[0];
        let main = mastik_core::component::main_of(&app.session.doc, inst).unwrap();
        let inner = verb(
            &mut app,
            "create_instance",
            json!({ "main": main.to_wire(), "parent": screen.to_wire(), "x": 20, "y": 20 }),
        )[0];
        let (l, _) = listed(&mut app, inner, &["A", "B"]);
        assert_eq!(
            app.session.doc.get(l).and_then(|n| n.parent()),
            Some(screen),
            "fixture: the list sits in the frame"
        );
        let rect = egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(1200.0, 800.0));
        let tagged = |app: &mut MastikApp| {
            let mut out = false;
            let _ = ctx.run_ui(
                egui::RawInput {
                    screen_rect: Some(rect),
                    ..Default::default()
                },
                |ui| out = app.frame_label(ui, l, rect, 1.0).is_some(),
            );
            out
        };
        app.session.selection.clear();
        let corner = app.selection_quad(l, rect, 1.0).map(|q| q[0]);
        assert!(
            corner.is_some_and(|c| rect.contains(c)),
            "fixture: the list is on screen at {corner:?}"
        );
        assert!(!tagged(&mut app), "no tab");
        app.session.selection.set_one(l);
        assert!(tagged(&mut app), "a tab while selected");
    }

    /// The moved-row note goes with an undo of the delete, and when the
    /// inspector shows another layer.
    /// flip: `document_rewound` keeping `list_row_moved` → fails at "undo";
    /// `inspector_ui` not dropping it → fails at "another layer".
    #[test]
    fn a_moved_row_note_goes_with_undo_and_another_layer() {
        use mastik_core::list::{List, Rows};
        let ctx = egui::Context::default();
        let (mut app, inst) = with_row(&ctx);
        let (l, set) = listed(&mut app, inst, &["A", "B", "C"]);
        let now = app.session.doc.get(l).unwrap().list().unwrap().clone();
        app.list_commit(
            l,
            "List rows",
            List {
                rows: Rows::Row(2),
                ..now
            },
        );
        app.session.selection.set_one(l);
        let delete = |app: &mut MastikApp| {
            let mut table = app.session.doc.data_set(set).unwrap().table.clone();
            table.rows.remove(0);
            app.commit_data_table(set, table);
        };
        delete(&mut app);
        assert!(app.list_row_moved.is_some(), "noted");
        pass(&ctx, &mut app);
        assert!(app.list_row_moved.is_some(), "kept while its list is shown");
        app.undo();
        assert_eq!(app.list_row_moved, None, "undo");

        delete(&mut app);
        assert!(app.list_row_moved.is_some(), "noted again");
        app.session.selection.set_one(inst);
        pass(&ctx, &mut app);
        assert_eq!(app.list_row_moved, None, "another layer");
    }

    /// A copy of a list standing on the page picks as its list, and a
    /// double-click on it selects the template, wherever it lands.
    /// flip: `double_click_pick` skipping `copy_template_at` → fails at "the
    /// template"; `pick_leaf` skipping the list as an occupied page frame →
    /// fails at "picks as its list".
    #[test]
    fn a_double_click_on_a_copy_selects_the_template() {
        let ctx = egui::Context::default();
        let (mut app, inst) = with_row(&ctx);
        let (l, _) = listed(&mut app, inst, &["A", "B", "C"]);
        assert!(
            mastik_core::query::is_page_frame(&app.session.doc, l),
            "fixture: the list stands on the page"
        );
        let copy = app.session.doc.get(l).unwrap().list().unwrap().copies[1];
        let at = app.session.resolved.world_bounds(copy).unwrap().center();
        let leaf = app.pick_leaf(at);
        assert_eq!(leaf, Some(l), "picks as its list");
        app.double_click_pick(leaf, at);
        assert_eq!(app.session.selection.single(), Some(inst), "the template");
    }

    /// *New table* makes a list over a table seeded from the instance's own
    /// values and an empty row 2, each column filling the property of its name,
    /// and opens the data window typing into row 2.
    /// flip: `make_list_new_table` not calling `type_into_cell` → fails at
    /// "typing into row 2".
    #[test]
    fn new_table_seeds_a_row_from_the_instance() {
        let ctx = egui::Context::default();
        let (mut app, inst) = with_row(&ctx);
        app.make_list_new_table(inst);
        let doc = &app.session.doc;
        let set = &doc.data_sets()[0];
        assert_eq!(set.name, "Table 1");
        assert_eq!(set.table.columns[0].name, "Title");
        assert_eq!(
            set.table.rows,
            [vec!["Name".to_string()], vec![String::new()]],
            "the instance's text, then an empty row"
        );
        let l = mastik_core::list::lists(doc)[0];
        assert_eq!(
            doc.get(l).unwrap().list().unwrap().fill.len(),
            1,
            "filled by name"
        );
        let open = app.chart_data.as_ref().expect("the data window opened");
        assert_eq!(open.data, Some(set.id));
        assert_eq!(open.typing_row(), Some(1), "typing into row 2");
        for _ in 0..3 {
            pass(&ctx, &mut app);
        }
        assert!(app.chart_data.is_some(), "drawn and still open");
    }

    /// The template's property a column fills shows that column in a capsule in
    /// place of its field, so row 1 is not edited where the next regeneration
    /// would undo it.
    /// flip: the instance rows not asking `filled_column` → fails at "the
    /// column's capsule".
    #[test]
    fn a_filled_property_on_the_template_shows_its_column() {
        let ctx = egui::Context::default();
        let (mut app, inst) = with_row(&ctx);
        let _ = listed(&mut app, inst, &["Linen", "Wool"]);
        app.session.selection.set_one(inst);
        let mut frame = eframe::Frame::_new_kittest();
        let mut texts: Vec<String> = Vec::new();
        for _ in 0..4 {
            let input = egui::RawInput {
                screen_rect: Some(egui::Rect::from_min_size(
                    egui::Pos2::ZERO,
                    egui::vec2(1320.0, 820.0),
                )),
                ..Default::default()
            };
            let out = ctx.run_ui(input, |ui| eframe::App::ui(&mut app, ui, &mut frame));
            texts = out
                .shapes
                .iter()
                .filter_map(|c| match &c.shape {
                    egui::Shape::Text(t) => Some(t.galley.text().to_string()),
                    _ => None,
                })
                .collect();
        }
        assert!(
            texts.iter().any(|t| t == "title"),
            "the column's capsule: {texts:?}"
        );
    }

    /// The template's text layer a filled property drives refuses the canvas
    /// text session with the column's words, so no edit lands that the
    /// regeneration would write back; a field no column fills stays editable.
    /// flip: `begin_edit_text` not asking `filled_layer` → fails at "no session".
    #[test]
    fn a_filled_layer_on_the_template_refuses_the_text_session() {
        use mastik_core::variant::PropKind;
        let ctx = egui::Context::default();
        let (mut app, inst) = with_row(&ctx);
        let _ = listed(&mut app, inst, &["Linen", "Wool"]);
        let label = app.session.doc.get(inst).unwrap().children()[0];
        let doc = &app.session.doc;
        assert!(
            crate::panels::variants::filled_layer(doc, label, PropKind::Text)
                .is_some_and(|w| w.contains("Filled from title in rows.csv")),
            "the column's words"
        );
        assert_eq!(
            crate::panels::variants::filled_layer(doc, label, PropKind::Boolean),
            None,
            "no column fills its visibility"
        );
        let depth = app.session.history.undo_depth();
        app.begin_edit_text(Some(label), None);
        assert!(app.text.is_none(), "no session");
        assert!(
            app.session.status().text.contains("Filled from title"),
            "said: {}",
            app.session.status().text
        );
        assert_eq!(app.session.history.undo_depth(), depth, "no step");
    }

    /// The Text card's content field over a template text a column fills is
    /// shown disabled; over a text no column fills it is editable.
    /// flip: `type_content_row`'s `read_only` without `filled` → fails at
    /// "read-only".
    #[test]
    fn the_text_cards_content_is_read_only_on_a_filled_text() {
        let ctx = egui::Context::default();
        let (mut app, inst) = with_row(&ctx);
        let _ = listed(&mut app, inst, &["Linen", "Wool"]);
        let label = app.session.doc.get(inst).unwrap().children()[0];
        let main = mastik_core::component::main_of(&app.session.doc, inst).unwrap();
        let free = app.session.doc.get(main).unwrap().children()[0];
        let enabled = |app: &mut MastikApp, id: NodeId| {
            app.session.selection.set_one(id);
            for _ in 0..4 {
                pass(&ctx, app);
            }
            ctx.read_response(egui::Id::new(("type-content", id)))
                .map(|r| r.enabled())
        };
        assert_eq!(enabled(&mut app, free), Some(true), "control: editable");
        assert_eq!(enabled(&mut app, label), Some(false), "read-only");
    }

    /// A list over `with_row`'s row whose `on_sale` column fills a boolean
    /// property bound to the template's text; answers the template's text.
    fn listed_with_a_shown_column(ctx: &egui::Context) -> (MastikApp, NodeId) {
        let (mut app, inst) = with_row(ctx);
        let main = mastik_core::component::main_of(&app.session.doc, inst).unwrap();
        let text = app.session.doc.get(main).unwrap().children()[0];
        verb(
            &mut app,
            "define_property",
            json!({ "node": text.to_wire(), "kind": "boolean", "name": "on_sale" }),
        );
        let set = DataSet {
            id: app.session.ids.mint_data_set(),
            name: "rows.csv".into(),
            table: Table {
                columns: vec![
                    Column {
                        id: ColumnId(0),
                        name: "title".into(),
                    },
                    Column {
                        id: ColumnId(1),
                        name: "on_sale".into(),
                    },
                ],
                rows: vec![
                    vec!["Linen".into(), "true".into()],
                    vec!["Wool".into(), "false".into()],
                ],
            },
            source: None,
            pictures: Vec::new(),
        };
        let (tx, _) = list::make(
            &app.session.doc,
            &app.session.resolved,
            &mut app.session.ids,
            inst,
            Source::New(set),
        )
        .unwrap();
        assert!(app.commit_edit(tx));
        let label = app.session.doc.get(inst).unwrap().children()[0];
        (app, label)
    }

    /// *Hide* (the chord and both context menus) and the layers eye refuse a
    /// template layer a filled column shows or hides, with the column's words
    /// and no step.
    /// flip: `toggle_hidden` not asking `filled_op` → fails at "Hide: no
    /// step"; `layer_eye` not asking it → fails at "eye: no step".
    #[test]
    fn hide_and_the_eye_refuse_a_layer_a_column_shows() {
        let ctx = egui::Context::default();
        let (mut app, label) = listed_with_a_shown_column(&ctx);
        assert!(
            crate::panels::variants::filled_layer(
                &app.session.doc,
                label,
                mastik_core::variant::PropKind::Boolean
            )
            .is_some(),
            "fixture: a column fills its visibility"
        );
        let depth = app.session.history.undo_depth();
        app.session.selection.set_one(label);
        app.dispatch(&ctx, crate::input::Action::ToggleHidden);
        assert_eq!(app.session.history.undo_depth(), depth, "Hide: no step");
        assert!(
            app.session.status().text.contains("on_sale"),
            "said: {}",
            app.session.status().text
        );
        app.layer_eye(label, true);
        assert_eq!(app.session.history.undo_depth(), depth, "eye: no step");
    }

    /// *Fill with content* over a template's text a filled column drives leaves
    /// it as the row has it and records no step, naming the column. The cells
    /// are emails so the box plans a write (it keeps a text it cannot read).
    /// flip: `commit_fill_content` not dropping `filled_op`'s operations →
    /// fails at "no step".
    #[test]
    fn fill_with_content_leaves_a_filled_text() {
        let ctx = egui::Context::default();
        let (mut app, inst) = with_row(&ctx);
        let _ = listed(&mut app, inst, &["ada@example.com", "alan@example.com"]);
        let label = app.session.doc.get(inst).unwrap().children()[0];
        let depth = app.session.history.undo_depth();
        app.session.selection.set_one(label);
        app.open_fill_content();
        let b = app.fill_content.take().expect("the box");
        app.run_fill_content(b);
        assert_eq!(app.session.history.undo_depth(), depth, "no step");
        let Some(NodeKind::Text { content, .. }) = app.session.doc.get(label).map(|n| n.kind())
        else {
            panic!("a text");
        };
        assert_eq!(content, "ada@example.com", "the row's cell");
        assert!(
            app.session.status().text.contains("Filled from title"),
            "said: {}",
            app.session.status().text
        );
    }

    /// A panel door committing through `commit_edit` (the inspector header's
    /// eye, *Back into flow*, the Fill card's image swap) refuses a template
    /// layer a filled column drives, with the column's words and no step.
    /// flip: `commit_edit` not asking `filled_op` → fails at "refused".
    #[test]
    fn commit_edit_refuses_a_layer_a_column_shows() {
        let ctx = egui::Context::default();
        let (mut app, label) = listed_with_a_shown_column(&ctx);
        let depth = app.session.history.undo_depth();
        app.session.name_step("Hide", crate::theme::icon::EYE_SLASH);
        let took = app.commit_edit(mastik_core::Transaction(vec![
            mastik_core::Operation::SetVisible {
                id: label,
                visible: false,
            },
        ]));
        assert!(!took, "refused");
        assert_eq!(app.session.history.undo_depth(), depth, "no step");
        assert!(
            app.session.status().text.contains("on_sale"),
            "said: {}",
            app.session.status().text
        );
        assert!(!app.session.has_step_name(), "the name is not left waiting");
    }

    /// *Fill with content* over a template whose picture a filled column drives
    /// fills the free text, leaves that layer whole (its fills and credit) and
    /// adds no picture for it, and says how many layers it skipped.
    /// flip: `filled_op` without its `SetFills` arm, or `commit_fill_content`
    /// filtering by op → fails at "credit kept" (the regeneration puts the
    /// fills back either way).
    #[test]
    fn fill_with_content_skips_a_filled_picture_whole() {
        use mastik_core::list::{List, Target};
        let ctx = egui::Context::default();
        crate::theme::install(&ctx);
        let mut app = MastikApp::headless(&ctx);
        let inst = with_photo(&mut app);
        let main = mastik_core::component::main_of(&app.session.doc, inst).unwrap();
        let text = verb(
            &mut app,
            "create_text",
            json!({ "parent": main.to_wire(), "text": "Name" }),
        )[0];
        // Named as the box reads them: an avatar and a full name.
        let main_thumb = app.session.doc.get(main).unwrap().children()[0];
        assert!(app.session.commit(mastik_core::Transaction(vec![
            mastik_core::Operation::SetName {
                id: main_thumb,
                name: "Avatar".into(),
            },
            mastik_core::Operation::SetName {
                id: text,
                name: "Name".into(),
            },
        ])));
        let set = DataSet {
            id: app.session.ids.mint_data_set(),
            name: "People".into(),
            table: Table {
                columns: vec![Column {
                    id: ColumnId(0),
                    name: "pic".into(),
                }],
                rows: vec![vec!["ada.png".into()], vec!["alan.png".into()]],
            },
            source: None,
            pictures: Vec::new(),
        };
        let (tx, l) = list::make(
            &app.session.doc,
            &app.session.resolved,
            &mut app.session.ids,
            inst,
            Source::New(set),
        )
        .unwrap();
        assert!(app.commit_edit(tx));
        let photo = mastik_core::variant::instance_properties(&app.session.doc, inst)
            .iter()
            .find(|p| p.name == "Photo")
            .map(|p| p.id)
            .expect("fixture: the image property");
        let now: List = app.session.doc.get(l).unwrap().list().unwrap().clone();
        app.commit_list_fill(
            l,
            List {
                fill: vec![list::Fill {
                    target: Target::Prop(photo),
                    column: ColumnId(0),
                }],
                ..now
            },
        );
        let kids = app.session.doc.get(inst).unwrap().children().to_vec();
        let (thumb, name) = (kids[0], kids[1]);
        assert!(
            crate::panels::variants::filled_layer(
                &app.session.doc,
                thumb,
                mastik_core::variant::PropKind::Image
            )
            .is_some(),
            "fixture: a column fills the picture"
        );
        let credit: mastik_core::credit::Credit = serde_json::from_value(json!({
            "kind": "photo",
            "text": "Photo by Ana on Unsplash",
            "author": "Ana",
            "provider": "Unsplash",
            "licence": "Unsplash License",
        }))
        .unwrap();
        assert!(app.session.commit(mastik_core::Transaction(vec![
            mastik_core::Operation::SetCredit {
                id: thumb,
                credit: Some(Box::new(credit)),
            }
        ])));
        let fills = |app: &MastikApp| app.session.doc.get(thumb).unwrap().paint().fills.to_vec();
        let before = fills(&app);
        let images = app.session.doc.images().count();
        assert!(
            app.session.doc.get(thumb).unwrap().credit().is_some(),
            "fixture: credited"
        );
        let depth = app.session.history.undo_depth();
        app.session.selection.set_one(inst);
        app.open_fill_content();
        let mut b = app.fill_content.take().expect("the box");
        // Invented values, not the table's: an avatar drawn and a name made up.
        b.opts.avatars = Some(Default::default());
        b.opts.columns.clear();
        app.run_fill_content(b);
        assert!(app.fill_content.is_none(), "fixture: the fill ran");
        assert_eq!(app.session.history.undo_depth(), depth + 1, "one step");
        assert_ne!(
            match app.session.doc.get(name).map(|n| n.kind()) {
                Some(NodeKind::Text { content, .. }) => content.as_str(),
                _ => "",
            },
            "Name",
            "the free text filled"
        );
        assert_eq!(fills(&app), before, "fills unchanged");
        assert!(
            app.session.doc.get(thumb).unwrap().credit().is_some(),
            "credit kept"
        );
        assert_eq!(app.session.doc.images().count(), images, "no picture added");
        assert!(
            app.session.status().text.contains("skipped 1 a list fills"),
            "said: {}",
            app.session.status().text
        );
    }

    /// *Reset all* on a list's template takes back its other overrides and
    /// leaves each filled property at its row's cell, since the list is made
    /// again in the reset's step; so it stays offered there.
    /// flip: `list::touched` not counting a list whose template the commit
    /// dirtied → fails at "refilled".
    #[test]
    fn reset_all_on_a_template_keeps_its_filled_values() {
        let ctx = egui::Context::default();
        let (mut app, inst) = with_row(&ctx);
        let (l, _) = listed(&mut app, inst, &["Linen", "Wool"]);
        verb(
            &mut app,
            "set_opacity",
            json!({ "nodes": [inst.to_wire()], "opacity": 0.5 }),
        );
        assert_eq!(app.session.doc.get(inst).unwrap().opacity(), 0.5, "fixture");
        app.session.selection.set_one(inst);
        let depth = app.session.history.undo_depth();
        app.reset_selection(mastik_core::reset::Kind::All);
        let doc = &app.session.doc;
        assert_eq!(
            doc.get(inst).unwrap().opacity(),
            1.0,
            "the override taken back"
        );
        let text = mastik_core::build::subtree_nodes(doc, &[inst])
            .into_iter()
            .find_map(|id| match doc.get(id)?.kind() {
                NodeKind::Text { content, .. } => Some(content.clone()),
                _ => None,
            });
        assert_eq!(text.as_deref(), Some("Linen"), "refilled");
        assert_eq!(doc.get(l).unwrap().list().unwrap().copies.len(), 1);
        assert_eq!(app.session.history.undo_depth(), depth + 1, "one step");
    }

    /// An edit in the data window writes the data set in one step, every list
    /// over it made again, a fill on a deleted column dropped; the data set is
    /// renamed past a name another has.
    /// flip: `commit_data_table` keeping every fill → fails at "dropped".
    #[test]
    fn the_data_window_edits_the_data_set() {
        let ctx = egui::Context::default();
        let (mut app, inst) = with_row(&ctx);
        let (l, set) = listed(&mut app, inst, &["A", "B"]);
        let mut grown = app.session.doc.data_set(set).unwrap().table.clone();
        grown.rows.push(vec!["C".into()]);
        let depth = app.session.history.undo_depth();
        app.commit_data_table(set, grown.clone());
        assert_eq!(app.session.history.undo_depth(), depth + 1, "one step");
        let copies = |app: &MastikApp| app.session.doc.get(l).unwrap().list().unwrap().copies.len();
        assert_eq!(copies(&app), 2, "three rows");

        let mut cut = grown;
        cut.columns.clear();
        cut.rows.iter_mut().for_each(|r| r.clear());
        app.commit_data_table(set, cut);
        let x = app.session.doc.get(l).unwrap().list().unwrap();
        assert!(x.fill.is_empty(), "dropped");

        app.rename_data_set(set, "Products");
        assert_eq!(app.session.doc.data_set(set).unwrap().name, "Products");
    }

    /// The instance menu's *Make a list from data ▸* lists the document's data
    /// sets, then *From a file…* and *New table*.
    #[test]
    fn the_make_list_flyout_lists_the_data_sets() {
        let ctx = egui::Context::default();
        let (mut app, inst) = with_row(&ctx);
        let page = app.session.doc.first_page();
        let main = mastik_core::component::main_of(&app.session.doc, inst).unwrap();
        let other = verb(
            &mut app,
            "create_instance",
            json!({ "main": main.to_wire(), "parent": page.to_wire(), "x": -3000, "y": 0 }),
        )[0];
        let (_, set) = listed(&mut app, inst, &["A", "B"]);
        app.session.selection.set_one(other);
        let target = crate::menu::Target::Layer {
            id: other,
            door: crate::menu::Door::Canvas,
        };
        app.open_context_menu(&ctx, target, None);
        let rows = app.open_menu_rows();
        let head = rows
            .iter()
            .flatten()
            .find(|r| r.item == crate::menu::Item::MakeList)
            .expect("the row");
        let labels: Vec<Vec<String>> = head
            .sub
            .iter()
            .map(|g| g.iter().map(|r| r.label.to_string()).collect())
            .collect();
        assert_eq!(
            labels,
            [
                vec!["rows.csv \u{b7} 2 rows".to_string()],
                vec!["From a file…".into(), "New table".into()]
            ]
        );
        app.make_list_over(other, set);
        assert_eq!(mastik_core::list::lists(&app.session.doc).len(), 2);
        assert_eq!(
            app.session.doc.data_sets().len(),
            1,
            "one data set, two lists"
        );
    }

    /// A picked file opens *Make a list*'s box with each property on the column
    /// of its name; the fill chosen there is the list's, and the file is linked.
    /// flip: `commit_make_list_filled` ignoring its `fill` → fails at "the box's fill".
    #[test]
    fn the_import_box_fills_the_list_as_chosen() {
        let ctx = egui::Context::default();
        let (mut app, inst) = with_row(&ctx);
        let dir = std::env::temp_dir().join(format!("mastik-list-box-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("products.csv");
        std::fs::write(&path, "title,label\nA,x\nB,y\n").unwrap();
        app.read_list_file(path.clone(), None, |path, linkable| {
            crate::chart_import::Then::List {
                instance: inst,
                path,
                linkable,
            }
        });
        app.settle_chart_read();
        let _ = std::fs::remove_dir_all(&dir);
        assert_eq!(
            app.import_fill(),
            Some(vec![("Title".to_string(), Some("title".to_string()))]),
            "on the column of its name"
        );
        assert!(
            mastik_core::list::lists(&app.session.doc).is_empty(),
            "nothing yet"
        );
        for _ in 0..3 {
            pass(&ctx, &mut app);
        }
        assert!(app.import_fill().is_some(), "the box draws and stays");

        app.press_make_list("Title", Some("label"));
        let doc = &app.session.doc;
        let l = mastik_core::list::lists(doc)[0];
        let x = doc.get(l).unwrap().list().unwrap();
        let set = doc.data_set(x.data).unwrap();
        let label = set
            .table
            .columns
            .iter()
            .find(|c| c.name == "label")
            .unwrap()
            .id;
        assert_eq!(x.fill[0].column, label, "the box's fill");
        assert!(set.source.is_some(), "linked");
        assert_eq!(x.copies.len(), 1);
    }

    /// A data file dropped on an instance asks for *Make a list*'s box, and the
    /// instance is the drop's target; dropped on bare canvas it is not.
    #[test]
    fn a_data_file_dropped_on_an_instance_makes_a_list() {
        let ctx = egui::Context::default();
        let (app, inst) = with_row(&ctx);
        let on = app.session.resolved.world_bounds(inst).unwrap().center();
        assert_eq!(app.list_drop_target(on), Some(inst));
        let off = mastik_core::kurbo::Point::new(-9000.0, -9000.0);
        assert_eq!(app.list_drop_target(off), None);
    }

    /// A picture column's cells are read as paths beside the data file: a
    /// relative path that reads as a picture is added once and paired with its
    /// cell; an absolute path, one climbing out with `..`, and a missing file are
    /// left out.
    /// flip: `read_pictures` dropping its plain-path check → fails at "not climbed out".
    #[test]
    fn picture_cells_are_read_beside_the_data_file() {
        let ctx = egui::Context::default();
        let (app, _) = with_row(&ctx);
        let root = std::env::temp_dir().join(format!("mastik-pictures-{}", std::process::id()));
        let dir = root.join("data");
        std::fs::create_dir_all(dir.join("img")).unwrap();
        let png = crate::app::rgba_to_png(&[200; 16], (2, 2)).unwrap();
        std::fs::write(dir.join("img").join("a.png"), &png).unwrap();
        std::fs::write(root.join("out.png"), &png).unwrap();
        let cells = ["img/a.png", "img/a.png", "../out.png", "nothing.png"];
        let table = Table {
            columns: vec![Column {
                id: ColumnId(0),
                name: "photo".into(),
            }],
            rows: cells.iter().map(|c| vec![(*c).to_string()]).collect(),
        };
        let (ops, pairs) =
            crate::list_data::read_pictures(&app.session.doc, Some(&dir), &table, &[ColumnId(0)]);
        let _ = std::fs::remove_dir_all(&root);
        let cells: Vec<&str> = pairs.iter().map(|(c, _)| c.as_str()).collect();
        assert_eq!(cells, ["img/a.png"], "not climbed out");
        assert_eq!(ops.len(), 1, "added once");
    }

    /// A picture beside the data file that is there but refused (an SVG) is
    /// split from one not found, with the read's reason; once it reads, the
    /// reason goes; one there that no read in this process looked at (after a
    /// reopen) is still not read, not missing.
    /// flip: `read_pictures_in` not calling `refuse` on a refused read → fails
    /// at "the read's reason"; `refused_picture` without `unread_picture` →
    /// fails at "there, not missing".
    #[test]
    fn a_refused_picture_is_not_read_rather_than_not_found() {
        let ctx = egui::Context::default();
        let (app, _) = with_row(&ctx);
        let dir = std::env::temp_dir().join(format!("mastik-refused-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("logo.svg"), "<svg/>").unwrap();
        let table = Table {
            columns: vec![Column {
                id: ColumnId(0),
                name: "photo".into(),
            }],
            rows: vec![vec!["logo.svg".into()], vec!["nobody.png".into()]],
        };
        let set = DataSet {
            id: mastik_core::IdSource::new(0x5E).mint_data_set(),
            name: "people.csv".into(),
            table: table.clone(),
            source: Some(mastik_core::chart::DataLink {
                path: dir.join("people.csv").to_string_lossy().into_owned(),
                size: 0,
                modified: 0,
                header: Some(true),
            }),
            pictures: Vec::new(),
        };
        let (_, pairs) =
            crate::list_data::read_pictures(&app.session.doc, Some(&dir), &table, &[ColumnId(0)]);
        assert!(pairs.is_empty(), "fixture: nothing reads");
        let (missing, refused) =
            crate::list_data::split_refused(Some(&set), ["logo.svg", "nobody.png"]);
        assert_eq!(missing, ["nobody.png"], "not found");
        assert_eq!(refused.len(), 1, "not read");
        assert!(
            refused[0].contains("logo.svg") && !refused[0].contains("Update reads it"),
            "the read's reason: {refused:?}"
        );

        let png = crate::app::rgba_to_png(&[200; 16], (2, 2)).unwrap();
        std::fs::write(dir.join("logo.svg"), &png).unwrap();
        let _ =
            crate::list_data::read_pictures(&app.session.doc, Some(&dir), &table, &[ColumnId(0)]);
        let reads = crate::list_data::last_refusal(&set, "logo.svg");
        // A file no read in this process looked at, as after a reopen.
        std::fs::write(dir.join("unread.png"), &png).unwrap();
        let (missing, refused) = crate::list_data::split_refused(Some(&set), ["unread.png"]);
        let _ = std::fs::remove_dir_all(&dir);
        assert_eq!(reads, None, "it reads now");
        assert!(missing.is_empty(), "there, not missing");
        assert!(
            refused.len() == 1 && refused[0].contains("not read"),
            "{refused:?}"
        );
    }

    /// *Make a list from data* over a linked table already in the document reads
    /// the pictures its image column names beside the file, in the same step.
    /// flip: `commit_make_list_filled` skipping `read_set_pictures` → fails at
    /// "read beside".
    #[test]
    fn a_list_made_over_a_linked_table_reads_its_pictures() {
        use mastik_core::chart::DataLink;
        let ctx = egui::Context::default();
        crate::theme::install(&ctx);
        let mut app = MastikApp::headless(&ctx);
        let inst = with_photo(&mut app);
        let dir = std::env::temp_dir().join(format!("mastik-make-list-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let other = crate::app::rgba_to_png(&[10; 16], (2, 2)).unwrap();
        std::fs::write(dir.join("ada.png"), other).unwrap();
        let set = DataSet {
            id: app.session.ids.mint_data_set(),
            name: "People".into(),
            table: Table {
                columns: vec![Column {
                    id: ColumnId(0),
                    name: "Photo".into(),
                }],
                rows: vec![vec!["ada.png".into()]],
            },
            source: Some(DataLink {
                path: dir.join("people.csv").to_string_lossy().into_owned(),
                size: 0,
                modified: 0,
                header: None,
            }),
            pictures: Vec::new(),
        };
        let data = set.id;
        assert!(app.session.commit(mastik_core::Transaction(vec![
            mastik_core::Operation::SetDataSet {
                id: data,
                set: Some(Box::new(set)),
            }
        ])));
        app.make_list_over(inst, data);
        let _ = std::fs::remove_dir_all(&dir);
        let doc = &app.session.doc;
        let made = doc.get(app.session.selection.single().expect("the list"));
        let made = made.and_then(|n| n.list()).expect("a list");
        assert_eq!(made.data, data, "the list reads the table");
        let set = doc.data_set(data).unwrap();
        assert!(set.picture(doc, "ada.png").is_some(), "read beside");
    }

    /// The table dropdown and *From a file…* move a list onto a linked table
    /// and read the pictures its image column names beside the file, in one
    /// *List data* step each.
    /// flip: `commit_list_onto` skipping `read_set_pictures` → fails at
    /// "dropdown: maps ada.png".
    #[test]
    fn moving_a_list_onto_a_linked_table_reads_its_pictures() {
        use mastik_core::chart::DataLink;
        let ctx = egui::Context::default();
        crate::theme::install(&ctx);
        let mut app = MastikApp::headless(&ctx);
        let inst = with_photo(&mut app);
        let dir = std::env::temp_dir().join(format!("mastik-move-list-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let png = crate::app::rgba_to_png(&[10; 16], (2, 2)).unwrap();
        std::fs::write(dir.join("ada.png"), &png).unwrap();
        let photo_table = |rows: Vec<Vec<String>>| Table {
            columns: vec![Column {
                id: ColumnId(0),
                name: "Photo".into(),
            }],
            rows,
        };
        let linked = |app: &mut MastikApp, name: &str| DataSet {
            id: app.session.ids.mint_data_set(),
            name: name.into(),
            table: photo_table(vec![vec!["ada.png".into()]]),
            source: Some(DataLink {
                path: dir.join("people.csv").to_string_lossy().into_owned(),
                size: 0,
                modified: 0,
                header: None,
            }),
            pictures: Vec::new(),
        };
        let blank = DataSet {
            id: app.session.ids.mint_data_set(),
            name: "Blank".into(),
            table: photo_table(vec![vec![String::new()]]),
            source: None,
            pictures: Vec::new(),
        };
        let first = blank.id;
        let other = linked(&mut app, "People");
        let other_id = other.id;
        assert!(app.session.commit(mastik_core::Transaction(vec![
            mastik_core::Operation::SetDataSet {
                id: first,
                set: Some(Box::new(blank)),
            },
            mastik_core::Operation::SetDataSet {
                id: other_id,
                set: Some(Box::new(other)),
            },
        ])));
        app.make_list_over(inst, first);
        let l = app.session.selection.single().expect("the list");

        let depth = app.session.history.undo_depth();
        app.list_onto_set(l, other_id);
        let doc = &app.session.doc;
        assert_eq!(app.session.history.undo_depth(), depth + 1, "one step");
        assert_eq!(
            doc.get(l).and_then(|n| n.list()).map(|x| x.data),
            Some(other_id)
        );
        let set = doc.data_set(other_id).unwrap();
        assert!(
            set.picture(doc, "ada.png").is_some(),
            "dropdown: maps ada.png"
        );

        let fresh = linked(&mut app, "People 2");
        let fresh_id = fresh.id;
        app.move_list_to_new_set(l, fresh);
        let _ = std::fs::remove_dir_all(&dir);
        let doc = &app.session.doc;
        let set = doc.data_set(fresh_id).expect("added");
        assert!(
            set.picture(doc, "ada.png").is_some(),
            "from a file: maps ada.png"
        );
    }

    /// *List fill* pointing the template's image property at a column of a
    /// linked set reads the pictures it names in the same step, and a later fill
    /// edit that points nothing new at a picture column reads none again.
    /// flip: `commit_list_fill` without its `retain` of new columns → fails at
    /// "nothing read again".
    #[test]
    fn a_list_fill_reads_only_the_picture_columns_it_newly_points_at() {
        use mastik_core::chart::DataLink;
        use mastik_core::list::{List, Rows, Target};
        let ctx = egui::Context::default();
        crate::theme::install(&ctx);
        let mut app = MastikApp::headless(&ctx);
        let inst = with_photo(&mut app);
        let dir = std::env::temp_dir().join(format!("mastik-list-fill-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let ada = crate::app::rgba_to_png(&[10; 16], (2, 2)).unwrap();
        std::fs::write(dir.join("ada.png"), ada).unwrap();
        // Named apart from the property, so the list starts with no fill.
        let set = DataSet {
            id: app.session.ids.mint_data_set(),
            name: "People".into(),
            table: Table {
                columns: vec![Column {
                    id: ColumnId(0),
                    name: "pic".into(),
                }],
                rows: vec![vec!["ada.png".into()]],
            },
            source: Some(DataLink {
                path: dir.join("people.csv").to_string_lossy().into_owned(),
                size: 0,
                modified: 0,
                header: None,
            }),
            pictures: Vec::new(),
        };
        let data = set.id;
        let (tx, l) = list::make(
            &app.session.doc,
            &app.session.resolved,
            &mut app.session.ids,
            inst,
            Source::New(set),
        )
        .unwrap();
        assert!(app.commit_edit(tx));
        let now: List = app.session.doc.get(l).unwrap().list().unwrap().clone();
        assert!(now.fill.is_empty(), "fixture: no fill yet");
        let photo = mastik_core::variant::instance_properties(&app.session.doc, inst)
            .iter()
            .find(|p| p.name == "Photo")
            .map(|p| p.id)
            .expect("fixture: the image property");
        let images = |app: &MastikApp| app.session.doc.images().count();
        let (depth, before) = (app.session.history.undo_depth(), images(&app));
        let filled = List {
            fill: vec![list::Fill {
                target: Target::Prop(photo),
                column: ColumnId(0),
            }],
            ..now
        };
        app.commit_list_fill(l, filled.clone());
        assert_eq!(app.session.history.undo_depth(), depth + 1, "one step");
        let doc = &app.session.doc;
        let set = doc.data_set(data).unwrap();
        assert_eq!(set.pictures.len(), 1, "the pair");
        assert!(set.picture(doc, "ada.png").is_some(), "read beside");
        assert_eq!(images(&app), before + 1, "one picture added");

        // The file changes on disk; a fill edit naming no new picture column
        // must not read it in again.
        let other = crate::app::rgba_to_png(&[90; 16], (2, 2)).unwrap();
        std::fs::write(dir.join("ada.png"), other).unwrap();
        let next = List {
            rows: Rows::First(1),
            ..filled
        };
        app.commit_list_fill(l, next);
        let _ = std::fs::remove_dir_all(&dir);
        assert_eq!(images(&app), before + 1, "nothing read again");
    }

    /// A relink reads the pictures beside the new file again, replacing the
    /// set's map rather than adding to it; *Unlink data file* drops the link in
    /// one step and does nothing to a set with none.
    /// flip: `replace_data_set_table` keeping the old `pictures` → fails at "the
    /// new file's picture". `unlink_data_set`'s no-link return has no flip of
    /// its own: the unchanged commit it skips makes no step either.
    #[test]
    fn a_relink_reads_its_pictures_again_and_unlink_drops_the_link() {
        use mastik_core::chart::DataLink;
        use mastik_core::list::{List, Target};
        let ctx = egui::Context::default();
        crate::theme::install(&ctx);
        let mut app = MastikApp::headless(&ctx);
        let inst = with_photo(&mut app);
        let dir = std::env::temp_dir().join(format!("mastik-relink-pics-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let first = crate::app::rgba_to_png(&[10; 16], (2, 2)).unwrap();
        std::fs::write(dir.join("ada.png"), first).unwrap();
        let set = DataSet {
            id: app.session.ids.mint_data_set(),
            name: "People".into(),
            table: Table {
                columns: vec![Column {
                    id: ColumnId(0),
                    name: "pic".into(),
                }],
                rows: vec![vec!["ada.png".into()]],
            },
            source: Some(DataLink {
                path: dir.join("people.csv").to_string_lossy().into_owned(),
                size: 0,
                modified: 0,
                header: None,
            }),
            pictures: Vec::new(),
        };
        let data = set.id;
        let (tx, l) = list::make(
            &app.session.doc,
            &app.session.resolved,
            &mut app.session.ids,
            inst,
            Source::New(set),
        )
        .unwrap();
        assert!(app.commit_edit(tx));
        let photo = mastik_core::variant::instance_properties(&app.session.doc, inst)
            .iter()
            .find(|p| p.name == "Photo")
            .map(|p| p.id)
            .expect("fixture: the image property");
        let now: List = app.session.doc.get(l).unwrap().list().unwrap().clone();
        app.commit_list_fill(
            l,
            List {
                fill: vec![list::Fill {
                    target: Target::Prop(photo),
                    column: ColumnId(0),
                }],
                ..now
            },
        );
        let read = |app: &MastikApp| {
            let doc = &app.session.doc;
            let set = doc.data_set(data).unwrap();
            (set.pictures.len(), set.picture(doc, "ada.png"))
        };
        let (_, old) = read(&app);
        assert!(old.is_some(), "fixture: the first file's picture");

        // The new file sits in `read_into`'s folder, beside its own picture.
        let name = "relinked.csv";
        let there = std::env::temp_dir().join(format!("mastik-read-{name}-{}", std::process::id()));
        std::fs::create_dir_all(&there).unwrap();
        let second = crate::app::rgba_to_png(&[90; 16], (2, 2)).unwrap();
        std::fs::write(there.join("ada.png"), second).unwrap();
        read_into(&mut app, name, "pic\nada.png\n", |path| {
            crate::chart_import::Then::DataSet {
                set: data,
                path: path.to_string_lossy().into_owned(),
                relink: true,
            }
        });
        let _ = std::fs::remove_dir_all(&dir);
        let (pairs, new) = read(&app);
        assert!(new.is_some() && new != old, "the new file's picture");
        assert_eq!(pairs, 1, "the map replaced, not added to");

        let depth = app.session.history.undo_depth();
        app.unlink_data_set(data);
        assert!(
            app.session.doc.data_set(data).unwrap().source.is_none(),
            "the link dropped"
        );
        assert_eq!(app.session.history.undo_depth(), depth + 1, "one step");
        app.unlink_data_set(data);
        assert_eq!(
            app.session.history.undo_depth(),
            depth + 1,
            "nothing to unlink"
        );
    }

    /// An image property's *Replace…* lands on the one layer it drives and that
    /// layer's image fill, not its first fill; with two instances selected it
    /// has nowhere to land.
    /// flip: `image_replace_target` answering fill 0 → fails at "its image
    /// fill"; without its one-root gate → fails at "two instances".
    #[test]
    fn an_image_propertys_replace_lands_on_its_layers_image_fill() {
        let ctx = egui::Context::default();
        crate::theme::install(&ctx);
        let mut app = MastikApp::headless(&ctx);
        let inst = with_photo(&mut app);
        let main = mastik_core::component::main_of(&app.session.doc, inst).unwrap();
        let thumb = app.session.doc.get(main).unwrap().children()[0];
        // A solid fill under the picture, so the image fill is the second.
        let mut fills = app.session.doc.get(thumb).unwrap().paint().fills.to_vec();
        let solid = mastik_core::Fill {
            brush: mastik_core::BrushSource::Literal(mastik_core::Brush::Solid(
                mastik_core::peniko::Color::BLACK,
            )),
            visible: true,
        };
        fills.insert(
            0,
            mastik_core::Keyed::new(app.session.ids.mint_item(), solid),
        );
        assert!(app.session.commit(mastik_core::Transaction(vec![
            mastik_core::Operation::SetFills { id: thumb, fills }
        ])));
        let page = app.session.doc.first_page().to_wire();
        let other = verb(
            &mut app,
            "create_instance",
            json!({ "main": main.to_wire(), "parent": page, "x": -3000, "y": 0 }),
        )[0];
        let doc = &app.session.doc;
        let photo = mastik_core::variant::instance_properties(doc, inst)
            .into_iter()
            .find(|p| p.name == "Photo")
            .expect("fixture: the image property");
        let copy = mastik_core::variant::counterparts(doc, inst, &photo);
        assert_eq!(copy.len(), 1, "fixture: one layer");
        assert_eq!(
            crate::panels::variants::image_replace_target(doc, &[inst], &photo),
            Some((copy[0], 1)),
            "its image fill"
        );
        assert_eq!(
            crate::panels::variants::image_replace_target(doc, &[inst, other], &photo),
            None,
            "two instances"
        );
    }

    /// A main *Card* whose rect *thumb* shows a picture through an image
    /// property *Photo*, and an instance of it on the page; answers the instance.
    fn with_photo(app: &mut MastikApp) -> NodeId {
        let png = crate::app::rgba_to_png(&[200; 16], (2, 2)).unwrap();
        let (main_pic, _) =
            crate::app::read_picture(png.clone(), Some("png".into()), None).unwrap();
        let page = app.session.doc.first_page().to_wire();
        let size = mastik_core::kurbo::Size::new(20.0, 20.0);
        let frame = serde_json::to_value(NodeKind::Artboard {
            size,
            corner_radii: Default::default(),
            corner_smoothing: 0.0,
        })
        .unwrap();
        let rect = serde_json::to_value(NodeKind::Rect {
            size,
            corner_radii: Default::default(),
            corner_smoothing: 0.0,
        })
        .unwrap();
        let main = verb(
            app,
            "create_node",
            json!({ "parent": page, "kind": frame, "name": "Card", "x": -4000 }),
        )[0];
        let thumb = verb(
            app,
            "create_node",
            json!({ "parent": main.to_wire(), "kind": rect }),
        )[0];
        let fill = mastik_core::Fill {
            brush: mastik_core::BrushSource::Literal(mastik_core::image_brush(main_pic.id.clone())),
            visible: true,
        };
        let item = app.session.ids.mint_item();
        assert!(app.session.commit(mastik_core::Transaction(vec![
            mastik_core::Operation::AddImage {
                id: main_pic.id.clone(),
                entry: main_pic.entry.clone(),
            },
            mastik_core::Operation::SetFills {
                id: thumb,
                fills: vec![mastik_core::Keyed::new(item, fill)],
            },
        ])));
        verb(app, "make_component", json!({ "nodes": [main.to_wire()] }));
        verb(
            app,
            "define_property",
            json!({ "node": thumb.to_wire(), "kind": "image", "name": "Photo" }),
        );
        verb(
            app,
            "create_instance",
            json!({ "main": main.to_wire(), "parent": page, "x": -3500, "y": 0 }),
        )[0]
    }

    /// Pictures beside a data file are read only under a local absolute folder:
    /// a folder the document names as relative (or, on Windows, behind a
    /// non-drive prefix) is never looked at, though the file is there; a cell
    /// written with `\` reads on every platform.
    /// flip: `read_pictures_in` dropping its `local_path` check → fails at "not local".
    #[test]
    fn pictures_are_read_only_beside_a_local_file() {
        let ctx = egui::Context::default();
        let (app, _) = with_row(&ctx);
        let png = crate::app::rgba_to_png(&[200; 16], (2, 2)).unwrap();
        let table = Table {
            columns: vec![Column {
                id: ColumnId(0),
                name: "photo".into(),
            }],
            rows: vec![vec![r"img\a.png".to_string()]],
        };
        let read = |dir: &Path| {
            crate::list_data::read_pictures(&app.session.doc, Some(dir), &table, &[ColumnId(0)])
        };
        let abs = std::env::temp_dir().join(format!("mastik-local-{}", std::process::id()));
        std::fs::create_dir_all(abs.join("img")).unwrap();
        std::fs::write(abs.join("img").join("a.png"), &png).unwrap();
        let rel = PathBuf::from(format!(".mastik-local-{}", std::process::id()));
        std::fs::create_dir_all(rel.join("img")).unwrap();
        std::fs::write(rel.join("img").join("a.png"), &png).unwrap();
        let found = read(&abs).1.len();
        let relative = read(&rel).1.len();
        let verbatim = cfg!(windows).then(|| {
            read(&PathBuf::from(format!(r"\\?\{}", abs.display())))
                .1
                .len()
        });
        let _ = std::fs::remove_dir_all(&abs);
        let _ = std::fs::remove_dir_all(&rel);
        assert_eq!(found, 1, "a local folder is read, `\\` or not");
        assert_eq!(relative, 0, "not local: relative");
        assert_eq!(verbatim.unwrap_or(0), 0, "not local: verbatim prefix");
    }
}
