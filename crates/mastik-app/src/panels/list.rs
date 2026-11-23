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

/// The rows modes in the segmented control's order, with their words.
const ROWS: [(&str, &str); 3] = [
    ("All", "Every row, up to 200"),
    ("First", "The first rows only"),
    (
        "Opened row",
        "The row opened in play by a tap on a list over the same data",
    ),
];

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
    fn list_commit(&mut self, id: NodeId, label: &'static str, next: List) {
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
        let dir = set
            .as_ref()
            .and_then(|s| s.source.as_ref())
            .map(|l| std::path::PathBuf::from(&l.path));
        let read =
            match (&set, &dir) {
                (Some(s), Some(file)) if !columns.is_empty() => Some(
                    crate::list_data::read_pictures(doc, file.parent(), &s.table, &columns),
                ),
                _ => None,
            };
        let Some((images, pairs)) = read.filter(|(_, p)| !p.is_empty()) else {
            return self.list_commit(id, "List fill", next);
        };
        let Some(mut set) = set else { return };
        for (cell, picture) in pairs {
            if !set.pictures.iter().any(|(c, _)| *c == cell) {
                set.pictures.push((cell, picture));
            }
        }
        let mut ops = images;
        ops.push(Operation::SetDataSet {
            id: set.id,
            set: Some(Box::new(set)),
        });
        ops.push(Operation::SetList {
            id,
            list: Some(Box::new(next)),
        });
        self.session.name_step("List fill", icon::TABLE);
        self.commit_edit(Transaction(ops));
    }

    /// The data set's dropdown, its summary, its file and its limits.
    fn list_data_rows(&mut self, ui: &mut egui::Ui, id: NodeId, now: &List) {
        enum Pick {
            Set(list::DataSetId),
            File,
        }
        let doc = &self.session.doc;
        let set = doc.data_set(now.data).cloned();
        let sets: Vec<(list::DataSetId, String, usize)> = doc
            .data_sets()
            .iter()
            .map(|d| (d.id, d.name.clone(), d.table.rows.len()))
            .collect();
        let shown = set
            .as_ref()
            .map_or_else(|| "Missing data".to_string(), |s| s.name.clone());
        let mut pick = None;
        ui.scope(|ui| {
            ui::combo_height(ui);
            egui::ComboBox::from_id_salt(("list-data", id))
                .icon(ui::combo_chevron)
                .width(ui.available_width())
                .selected_text(ui::glyph_and_text(icon::TABLE, &shown))
                .show_ui(ui, |ui| {
                    ui::menu_rows(ui);
                    for (d, name, rows) in &sets {
                        let row = format!("{name} \u{b7} {}", ui::count_as(*rows, "row", "rows"));
                        if ui.selectable_label(*d == now.data, row).clicked() {
                            pick = Some(Pick::Set(*d));
                        }
                    }
                    if !sets.is_empty() {
                        ui::menu_sep(ui);
                    }
                    if ui
                        .selectable_label(false, ui::glyph_and_text(icon::IMPORT, "From a file…"))
                        .clicked()
                    {
                        pick = Some(Pick::File);
                    }
                });
        });
        match pick {
            Some(Pick::Set(d)) if d != now.data => {
                if let Some(table) = self.session.doc.data_set(d).map(|s| s.table.clone()) {
                    let next = self.list_on(id, now, &table, d);
                    self.list_commit(id, "List data", next);
                }
            }
            Some(Pick::File) => self.list_data_dialog(id),
            _ => {}
        }
        let Some(set) = set else {
            ui::selection_note(ui, "Its data set is not in this document: pick one");
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
        let drawn = now.shown_rows(rows).len();
        if rows == 0 {
            ui.label(
                egui::RichText::new("No rows: the template is hidden in play")
                    .size(ui::SMALL_PT)
                    .color(color::warn()),
            );
        }
        // Cut only when the cap stops short of what the mode asked for.
        let asked = match now.rows {
            Rows::All => rows,
            Rows::First(n) => rows.min(n as usize),
            Rows::Opened => 0,
        };
        if drawn < asked {
            ui.label(
                egui::RichText::new(format!("Showing {drawn} of {rows} rows"))
                    .size(ui::SMALL_PT)
                    .color(color::warn()),
            );
        }
    }

    /// A linked data set's source line (*Linked to products.csv*, *Changed on
    /// disk*, *File not found*) and its *Update*, *Relink…* and *Unlink*, on the
    /// List and Chart cards; nothing while unlinked.
    pub(crate) fn data_set_link_rows(&mut self, ui: &mut egui::Ui, set: &list::DataSet) {
        if let Some(link) = &set.source {
            let state = self.data_link_state(set.id).unwrap_or(LinkState::Current);
            let name = file_name(&link.path);
            let (said, ink) = match state {
                LinkState::Current => (format!("Linked to {name}"), theme::text::muted()),
                LinkState::Changed => (
                    format!("{name} \u{b7} Changed on disk"),
                    color::accent_300(),
                ),
                LinkState::Missing => (format!("{name} \u{b7} File not found"), color::warn()),
            };
            ui.horizontal(|ui| {
                ui.spacing_mut().item_spacing.x = 8.0;
                ui.label(theme::icon_text(
                    icon::LINK_SIMPLE,
                    13.0,
                    theme::text::dim(),
                ));
                ui.add(
                    egui::Label::new(egui::RichText::new(said).size(ui::SMALL_PT).color(ink))
                        .truncate(),
                )
                .on_hover_text(&link.path);
            });
            ui.horizontal(|ui| {
                ui.spacing_mut().item_spacing.x = 12.0;
                if state == LinkState::Changed && ui::word_link(ui, "Update", false).clicked() {
                    self.update_data_set(set.id);
                }
                if state == LinkState::Missing {
                    let tip = match self.readers_of(set.id) {
                        (0..=1, 0) | (0, 1) => format!("Pick the file again for {}", set.name),
                        _ => format!(
                            "Pick the file again for the {} reading {}",
                            self.readers_words(set.id),
                            set.name
                        ),
                    };
                    if ui::word_link(ui, "Relink…", false)
                        .on_hover_text(tip)
                        .clicked()
                    {
                        self.relink_data_set(set.id);
                    }
                }
                if ui::word_link(ui, "Unlink", true).clicked() {
                    self.unlink_data_set(set.id);
                }
            });
        }
    }

    /// *Rows*: every row, the first few, or the row opened in play.
    fn list_rows_rows(&mut self, ui: &mut egui::Ui, id: NodeId, now: &List) {
        if !self.card_fold(ui, "List rows", "Rows") {
            return;
        }
        let at = match now.rows {
            Rows::All => 0,
            Rows::First(_) => 1,
            Rows::Opened => 2,
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
                let mut v = f64::from(n);
                let resp = ui::value_field(
                    ui,
                    egui::vec2(96.0, ui::CONTROL_H),
                    Prefix::Label("Rows"),
                    &mut v,
                    Scrub::whole(0.25).range(1.0..=list::MAX_DRAWN as f64),
                    |d| d.custom_formatter(ui::number(0)),
                )
                .on_hover_text("How many rows the list draws");
                let n = v.round().clamp(1.0, list::MAX_DRAWN as f64) as u32;
                let tx = Transaction(vec![Operation::SetList {
                    id,
                    list: Some(Box::new(List {
                        rows: Rows::First(n),
                        ..now.clone()
                    })),
                }]);
                self.named_valve("List rows", icon::TABLE, &resp, tx);
            }
            Rows::Opened => {
                ui.label(
                    egui::RichText::new("Shows the row opened in play. Row 1 here.")
                        .size(ui::SMALL_PT)
                        .color(theme::text::muted()),
                );
            }
            Rows::All => {}
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
        let columns: Vec<(ColumnId, String)> = doc
            .data_set(now.data)
            .map(|d| {
                d.table
                    .columns
                    .iter()
                    .map(|c| (c.id, c.name.clone()))
                    .collect()
            })
            .unwrap_or_default();
        let unmatched = list::unmatched_variants(doc, id);
        let no_picture = list::unmatched_pictures(doc, id);
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
            let shown = now_col
                .and_then(|c| columns.iter().find(|(i, _)| *i == c))
                .map_or("None", |(_, n)| n.as_str())
                .to_string();
            let mut pick: Option<Option<ColumnId>> = None;
            ui::labelled(ui, &name, |ui| {
                ui::combo_height(ui);
                egui::ComboBox::from_id_salt(("list-fill", id, name.as_str()))
                    .icon(ui::combo_chevron)
                    .width(ui.available_width())
                    .selected_text(ui::combo_text(&shown))
                    .show_ui(ui, |ui| {
                        ui::menu_rows(ui);
                        if ui.selectable_label(now_col.is_none(), "None").clicked() {
                            pick = Some(None);
                        }
                        for (c, n) in &columns {
                            if ui.selectable_label(now_col == Some(*c), n).clicked() {
                                pick = Some(Some(*c));
                            }
                        }
                    });
            });
            if let Target::Variant(v) = &target
                && let Some((_, cells)) = unmatched.iter().find(|(p, _)| p == v)
            {
                ui.label(
                    egui::RichText::new(format!(
                        "{} name no {v} variant",
                        ui::count_as(cells.len(), "value", "values")
                    ))
                    .size(ui::SMALL_PT)
                    .color(color::warn()),
                )
                .on_hover_text(
                    cells
                        .iter()
                        .map(|c| {
                            if c.is_empty() {
                                "an empty cell"
                            } else {
                                c.as_str()
                            }
                        })
                        .collect::<Vec<_>>()
                        .join(", "),
                );
            }
            if matches!(target, Target::Prop(_))
                && let Some((_, cells)) = no_picture.iter().find(|(p, _)| *p == name)
            {
                let tip = match &data_file {
                    Some(f) => format!("{}: looked for next to {f}", cells.join(", ")),
                    None => cells.join(", "),
                };
                ui.label(
                    egui::RichText::new(format!(
                        "{} not found",
                        ui::count_as(cells.len(), "picture", "pictures")
                    ))
                    .size(ui::SMALL_PT)
                    .color(color::warn()),
                )
                .on_hover_text(tip);
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
        let open = !self.collapsed_panels.contains(key);
        if ui::fold_rule(ui, label, open, 0).clicked() {
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
        assert!(crate::list_data::list_is_empty(&app.session.doc, l));
    }

    /// A double-click on a copy selects the template, wherever it lands.
    /// flip: `double_click_pick` skipping `copy_template_at` → fails at "the template".
    #[test]
    fn a_double_click_on_a_copy_selects_the_template() {
        let ctx = egui::Context::default();
        let (mut app, inst) = with_row(&ctx);
        let (l, _) = listed(&mut app, inst, &["A", "B", "C"]);
        let copy = app.session.doc.get(l).unwrap().list().unwrap().copies[1];
        let at = app.session.resolved.world_bounds(copy).unwrap().center();
        app.double_click_pick(Some(l), at);
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
}
