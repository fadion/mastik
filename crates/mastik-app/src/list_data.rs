//! Lists from data in the app (`docs/design/lists.md`): *Make a list from data*,
//! a list moved onto another data set, a linked data set read again, *Detach
//! list*, and the pill a changed data file shows. The List card is
//! `panels/list.rs`; the file reads go through the chart import's worker
//! (`chart_import`), which these land on.

use crate::app::MastikApp;
use crate::chart_import::{LinkState, Parsed, Source, Then, file_name};
use crate::theme::icon;
use eframe::egui;
use mastik_core::chart::{DataLink, table_io};
use mastik_core::list::{self, DataSet, DataSetId, List};
use mastik_core::{NodeId, Operation, Transaction};
use std::path::{Path, PathBuf};

/// The file picker's filter for a data file.
const FILTER: (&str, &[&str]) = ("Data", &["csv", "tsv", "json"]);

/// What a list's card and import box say when its template offers nothing a
/// column can fill.
pub(crate) const NO_FILL_TARGETS: &str =
    "Its component has no text, boolean, image or variant property";

/// A table as every table dropdown and flyout lists it: *Products · 24 rows*,
/// or *Products · not used* when no list or chart reads it.
pub(crate) fn table_words(doc: &mastik_core::Document, set: &DataSet) -> String {
    let count = if list::is_read(doc, set.id) {
        crate::ui::count_as(set.table.rows.len(), "row", "rows")
    } else {
        "not used".to_string()
    };
    format!("{} \u{b7} {count}", set.name)
}

/// What a pick in [`table_combo`] asks for.
pub(crate) enum TablePick {
    /// *Own table*: a chart's table of its own.
    Own,
    Set(DataSetId),
    /// *New table*.
    New,
    /// *Save as table*: a chart's own table made one of the document's.
    Save,
    /// *From a file…*.
    File,
}

/// The rows a [`table_combo`] offers beside the document's tables: *Own table*
/// first, ticked when `own` is `Some(true)`, and *New table* and *Save as
/// table* before *From a file…*, which `no_file` leaves out where only a table
/// already there can be named (a row action's).
#[derive(Default)]
pub(crate) struct TableRows {
    pub own: Option<bool>,
    pub new: bool,
    pub save: bool,
    pub no_file: bool,
}

/// *Save as table*'s tooltip, on the Chart card's dropdown row and the data
/// window's button.
pub(crate) const SAVE_AS_TABLE_TIP: &str =
    "Keep this chart's data as a table that other charts and lists can read";

/// The dropdown of the document's tables on the List card, the Chart card and
/// *Data tables*' toolbar: `width` wide whatever the tables are called
/// ([`crate::ui::fitted_combo`]), its face the `TABLE` glyph and `face`, each
/// table as [`table_words`] with `ticked` ticked, then `extra` and *From a
/// file…*. Picking the ticked table answers nothing.
pub(crate) fn table_combo(
    ui: &mut egui::Ui,
    salt: impl std::hash::Hash + std::fmt::Debug,
    width: f32,
    doc: &mastik_core::Document,
    face: &str,
    ticked: Option<DataSetId>,
    extra: &TableRows,
) -> Option<TablePick> {
    use crate::ui;
    let mut pick = None;
    ui::fitted_combo(
        ui,
        salt,
        width,
        ui::glyph_and_text(icon::TABLE, face),
        face,
        |ui| {
            ui::menu_rows(ui);
            if let Some(on) = extra.own {
                if ui
                    .selectable_label(on, "Own table")
                    .on_hover_text("A table of the chart's own, which nothing else reads")
                    .clicked()
                    && !on
                {
                    pick = Some(TablePick::Own);
                }
                ui::menu_sep(ui);
            }
            for d in doc.data_sets() {
                let on = ticked == Some(d.id);
                if ui.selectable_label(on, table_words(doc, d)).clicked() && !on {
                    pick = Some(TablePick::Set(d.id));
                }
            }
            let more = extra.new || extra.save || !extra.no_file;
            if !doc.data_sets().is_empty() && more {
                ui::menu_sep(ui);
            }
            if extra.new
                && ui
                    .selectable_label(false, ui::glyph_and_text(icon::PLUS, "New table"))
                    .clicked()
            {
                pick = Some(TablePick::New);
            }
            if extra.save
                && ui
                    .selectable_label(
                        false,
                        ui::glyph_and_text(icon::FLOPPY_DISK, "Save as table"),
                    )
                    .on_hover_text(SAVE_AS_TABLE_TIP)
                    .clicked()
            {
                pick = Some(TablePick::Save);
            }
            if !extra.no_file
                && ui
                    .selectable_label(false, ui::glyph_and_text(icon::IMPORT, "From a file…"))
                    .clicked()
            {
                pick = Some(TablePick::File);
            }
        },
    );
    pick
}

/// The rows a row dropdown lists, as many as a list draws.
pub(crate) const ROWS_LISTED: usize = list::MAX_DRAWN;

/// Row `n` (from 0) as a row dropdown names it, by its first cell, the one a
/// person reads it by: *3 · Linen shirt*, or *Row 3* when that cell is empty.
pub(crate) fn row_words(n: usize, first: Option<&str>) -> String {
    match first.map(str::trim).filter(|c| !c.is_empty()) {
        Some(cell) => format!("{} \u{b7} {cell}", n + 1),
        None => format!("Row {}", n + 1),
    }
}

/// What list `id` draws, in the words its canvas tab and layers badge show,
/// with the ink: *24 rows*, *200 of 1240 rows*, *6 of 24 rows* under a
/// filter, *Opened row*, *row 3*, or *No rows*, *0 of 24 rows* and *Missing
/// data* in the warning ink.
pub(crate) fn list_count(doc: &mastik_core::Document, id: NodeId) -> (String, egui::Color32) {
    use crate::theme::{color, text};
    let Some(l) = doc.get(id).and_then(|n| n.list()) else {
        return (String::new(), text::dim());
    };
    let Some(set) = doc.data_set(l.data) else {
        return ("Missing data".into(), color::warn());
    };
    let rows = set.table.rows.len();
    if rows == 0 {
        return ("No rows".into(), color::warn());
    }
    let shown = list::shown(doc, id);
    match l.rows {
        list::Rows::Opened => return ("Opened row".into(), text::dim()),
        list::Rows::Row(_) => {
            let n = shown.first().map_or(1, |r| r + 1);
            return (format!("row {n}"), text::dim());
        }
        _ => {}
    }
    let drawn = shown.len();
    if drawn == 0 {
        return (format!("0 of {rows} rows"), color::warn());
    }
    let said = if drawn < rows && (drawn == list::MAX_DRAWN || !l.filter.is_empty()) {
        format!("{drawn} of {rows} rows")
    } else {
        crate::ui::count_as(drawn, "row", "rows")
    };
    (said, text::dim())
}

/// The columns of list `id`'s data set that fill an image property of its
/// template under `fill`.
pub(crate) fn picture_columns(
    doc: &mastik_core::Document,
    id: NodeId,
    fill: &[list::Fill],
) -> Vec<mastik_core::chart::ColumnId> {
    let Some(tpl) = list::template(doc, id) else {
        return Vec::new();
    };
    picture_columns_of(doc, tpl, fill)
}

/// [`picture_columns`] for a template not yet in a list.
fn picture_columns_of(
    doc: &mastik_core::Document,
    template: NodeId,
    fill: &[list::Fill],
) -> Vec<mastik_core::chart::ColumnId> {
    let props = mastik_core::variant::instance_properties(doc, template);
    fill.iter()
        .filter(|f| {
            matches!(&f.target, list::Target::Prop(p)
                if props.iter().any(|q| q.id == *p && q.kind == mastik_core::variant::PropKind::Image))
        })
        .map(|f| f.column)
        .collect()
}

/// The pictures the cells of `table`'s `columns` name as paths beside the data
/// file in `dir`, in its first [`list::MAX_DRAWN`] rows, for a table no list
/// reads yet: read and decoded now, each new picture's `AddImage` and every
/// (cell, picture) pair. A cell already naming a picture's id, one
/// [`beside_path`] refuses, or one that does not read as a picture is left out.
pub(crate) fn read_pictures(
    doc: &mastik_core::Document,
    dir: Option<&Path>,
    table: &mastik_core::chart::Table,
    columns: &[mastik_core::chart::ColumnId],
) -> (Vec<Operation>, Vec<(String, mastik_core::ImageId)>) {
    let rows: Vec<usize> = (0..table.rows.len().min(list::MAX_DRAWN)).collect();
    read_pictures_in(doc, dir, table, columns, &rows)
}

/// The rows of `table` the lists over data set `set` can draw, `also` (a list
/// not over it in the document yet) among them: each list's shown rows, and
/// every row once one is *Opened*, which play may open on any. With no list,
/// the first [`list::MAX_DRAWN`].
pub(crate) fn drawn_rows(
    doc: &mastik_core::Document,
    set: Option<DataSetId>,
    table: &mastik_core::chart::Table,
    also: Option<(NodeId, &List)>,
) -> Vec<usize> {
    let mut lists: Vec<(NodeId, &List)> = also.into_iter().collect();
    if let Some(set) = set {
        for id in list::lists(doc) {
            if let Some(l) = doc.get(id).and_then(|n| n.list())
                && l.data == set
                && also.is_none_or(|(a, _)| a != id)
            {
                lists.push((id, l));
            }
        }
    }
    let len = table.rows.len();
    if lists.is_empty() {
        return (0..len.min(list::MAX_DRAWN)).collect();
    }
    if lists.iter().any(|(_, l)| l.rows == list::Rows::Opened) {
        return (0..len).collect();
    }
    let mut rows: Vec<usize> = lists
        .iter()
        .flat_map(|(id, l)| l.shown_rows(table, &|v| doc.resolve_var(*id, v)))
        .collect();
    rows.sort_unstable();
    rows.dedup();
    rows
}

/// [`read_pictures`] for data set `set`, beside its linked file, in the rows
/// its lists and `also` draw ([`drawn_rows`]): the `AddImage`s of the pictures
/// `columns` name that `set` does not map yet, with `set`'s map grown by them;
/// `None` when it is not linked or nothing is found.
pub(crate) fn read_set_pictures(
    doc: &mastik_core::Document,
    set: &DataSet,
    columns: &[mastik_core::chart::ColumnId],
    also: Option<(NodeId, &List)>,
) -> Option<(Vec<Operation>, DataSet)> {
    let link = set.source.as_ref()?;
    if columns.is_empty() {
        return None;
    }
    let dir = Path::new(&link.path).parent();
    let rows = drawn_rows(doc, Some(set.id), &set.table, also);
    let (images, pairs) = read_pictures_in(doc, dir, &set.table, columns, &rows);
    let mut next = set.clone();
    for (cell, picture) in pairs {
        if set.picture(doc, &cell).is_none() && !next.pictures.iter().any(|(c, _)| *c == cell) {
            next.pictures.push((cell, picture));
        }
    }
    (next.pictures.len() > set.pictures.len()).then_some((images, next))
}

/// The operations that put list `id` (as `next`) over data set `set`, ahead of
/// the list's own: the pictures `columns` name read and added, then the set with
/// its grown map; the set alone when `adding` it and nothing is read; `None`
/// when there is nothing to write ahead.
fn set_ahead(
    doc: &mastik_core::Document,
    set: &DataSet,
    (id, next): (NodeId, &List),
    columns: &[mastik_core::chart::ColumnId],
    adding: bool,
) -> Option<Vec<Operation>> {
    let (mut ops, set) = match read_set_pictures(doc, set, columns, Some((id, next))) {
        Some((ops, set)) => (ops, set),
        None if adding => (Vec::new(), set.clone()),
        None => return None,
    };
    ops.push(Operation::SetDataSet {
        id: set.id,
        set: Some(Box::new(set)),
    });
    Some(ops)
}

impl MastikApp {
    /// Put list `id` as `next` over data set `set` as one step `label`: the
    /// pictures `columns` name beside its linked file read and mapped first, the
    /// set written when `adding` it or when the read grew its map, then the list.
    /// Every door that points a list at a table, or a property at an image
    /// column, writes the set through [`set_ahead`]. Whether the step landed.
    pub(crate) fn commit_list_onto(
        &mut self,
        id: NodeId,
        set: &DataSet,
        next: List,
        columns: &[mastik_core::chart::ColumnId],
        adding: bool,
        label: &'static str,
    ) -> bool {
        let Some(mut ops) = set_ahead(&self.session.doc, set, (id, &next), columns, adding) else {
            self.list_commit(id, label, next);
            return true;
        };
        ops.push(Operation::SetList {
            id,
            list: Some(Box::new(next)),
        });
        self.session.name_step(label, icon::TABLE);
        self.commit_edit(Transaction(ops))
    }

    /// List `id` moved onto data set `d`, already in the document: each
    /// property keeps its column where the table has one of that name, and the
    /// pictures its image properties name are read in the same step.
    pub(crate) fn list_onto_set(&mut self, id: NodeId, d: DataSetId) {
        let doc = &self.session.doc;
        let (Some(set), Some(now)) = (
            doc.data_set(d).cloned(),
            doc.get(id).and_then(|n| n.list()).cloned(),
        ) else {
            return;
        };
        let next = self.list_on(id, &now, &set);
        let columns = picture_columns(&self.session.doc, id, &next.fill);
        self.commit_list_onto(id, &set, next, &columns, false, "List data");
    }
}

/// Where the picture `cell` names beside a data file in `dir` is, when it may be
/// looked at: a plain relative path (no root, no `..`) under a local folder.
/// `dir` comes from the document's own link, so it is untrusted: a stat under
/// `\\host\share` sends the user's credentials to that host. `img\a.png` names
/// the same file on every platform.
pub(crate) fn beside_path(dir: &Path, cell: &str) -> Option<PathBuf> {
    let rel = cell.trim().replace('\\', "/");
    let plain = Path::new(&rel).components().all(|k| {
        matches!(
            k,
            std::path::Component::Normal(_) | std::path::Component::CurDir
        )
    });
    let path = dir.join(&rel);
    (plain && crate::tokens_import::local_path(&path)).then_some(path)
}

/// [`read_pictures`] over table rows `rows`: every row for an export, which
/// draws a file per row, and the rows its lists draw for a data set.
pub(crate) fn read_pictures_in(
    doc: &mastik_core::Document,
    dir: Option<&Path>,
    table: &mastik_core::chart::Table,
    columns: &[mastik_core::chart::ColumnId],
    rows: &[usize],
) -> (Vec<Operation>, Vec<(String, mastik_core::ImageId)>) {
    let (mut ops, mut pairs) = (Vec::new(), Vec::<(String, mastik_core::ImageId)>::new());
    let Some(dir) = dir else {
        return (ops, pairs);
    };
    // Every cell tried once, so a failed read is not repeated for each row naming it.
    let mut tried = rustc_hash::FxHashSet::<&str>::default();
    for col in columns {
        let Some(c) = table.index_of(*col) else {
            continue;
        };
        for row in rows.iter().filter_map(|r| table.rows.get(*r)) {
            let cell = row[c].trim();
            if cell.is_empty()
                || !tried.insert(cell)
                || doc.image(&mastik_core::ImageId(cell.to_string())).is_some()
            {
                continue;
            }
            let Some(path) = beside_path(dir, cell) else {
                continue;
            };
            let Some(size) = std::fs::metadata(&path)
                .ok()
                .filter(|m| m.is_file())
                .map(|m| m.len())
            else {
                forget_refusal(&path);
                continue;
            };
            let shown = crate::app::image_name(&path);
            let max = crate::tokens_import::MAX_READ;
            if size > max {
                let why = format!("“{shown}” is larger than {} MB", max / (1024 * 1024));
                refuse(&path, why);
                continue;
            }
            let Ok(bytes) = std::fs::read(&path) else {
                forget_refusal(&path);
                continue;
            };
            let ext = crate::app::file_extension(&path);
            let image = match crate::app::read_picture(bytes, ext, Some(shown.clone())) {
                Ok((image, _)) => image,
                Err(e) => {
                    refuse(&path, e.message(&shown));
                    continue;
                }
            };
            forget_refusal(&path);
            let added = ops
                .iter()
                .any(|op| matches!(op, Operation::AddImage { id, .. } if *id == image.id));
            if !doc.has_image(&image.id) && !added {
                ops.push(Operation::AddImage {
                    id: image.id.clone(),
                    entry: image.entry.clone(),
                });
            }
            pairs.push((cell.to_string(), image.id));
        }
    }
    (ops, pairs)
}

/// Pictures found beside a data file but refused when read, by path, with
/// what the read said. [`read_pictures_in`] keeps it, so a note on a cell no
/// picture fills says the file was not read, and why, instead of sending the
/// person looking for a file that is there.
static REFUSED: std::sync::LazyLock<std::sync::Mutex<rustc_hash::FxHashMap<PathBuf, String>>> =
    std::sync::LazyLock::new(Default::default);

fn refuse(path: &Path, why: String) {
    if let Ok(mut r) = REFUSED.lock() {
        r.insert(path.to_path_buf(), why);
    }
}

fn forget_refusal(path: &Path) {
    if let Ok(mut r) = REFUSED.lock() {
        r.remove(path);
    }
}

/// Why the picture `cell` names beside data set `set`'s linked file was
/// refused, when the last read found it and could not take it. With no read
/// in this process (the document was reopened), a file that is there is
/// worded as not read rather than not found.
pub(crate) fn refused_picture(set: &DataSet, cell: &str) -> Option<String> {
    last_refusal(set, cell).or_else(|| unread_picture(set, cell))
}

/// What the last read in this process said of the picture `cell` names beside
/// `set`'s linked file, when it refused it.
pub(crate) fn last_refusal(set: &DataSet, cell: &str) -> Option<String> {
    let dir = Path::new(&set.source.as_ref()?.path).parent()?;
    let path = beside_path(dir, cell)?;
    REFUSED.lock().ok()?.get(&path).cloned()
}

/// The note for a picture `cell` names that is beside `set`'s linked file but
/// no read in this process has looked at.
fn unread_picture(set: &DataSet, cell: &str) -> Option<String> {
    let dir = Path::new(&set.source.as_ref()?.path).parent()?;
    let path = beside_path(dir, cell)?;
    let there = std::fs::metadata(&path).is_ok_and(|m| m.is_file());
    there.then(|| {
        let shown = crate::app::image_name(&path);
        format!("“{shown}” is there but was not read: Update reads it")
    })
}

/// `cells` no picture fills, split into those not found and those found but
/// refused, the latter with their reasons.
pub(crate) fn split_refused<'a>(
    set: Option<&DataSet>,
    cells: impl IntoIterator<Item = &'a str>,
) -> (Vec<&'a str>, Vec<String>) {
    let mut missing = Vec::new();
    let mut refused = Vec::new();
    for c in cells {
        match set.and_then(|s| refused_picture(s, c)) {
            Some(why) => refused.push(why),
            None => missing.push(c),
        }
    }
    (missing, refused)
}

/// The link a data set read from `path` keeps, from the read's stat.
fn link_for(path: &Path, parsed: &Parsed, header: Option<bool>) -> DataLink {
    let (modified, size) = parsed.stat.unwrap_or_default();
    DataLink {
        path: path.to_string_lossy().into_owned(),
        size,
        modified,
        header,
    }
}

impl MastikApp {
    /// The instance *Make a list from data* would act on: the one the menu was
    /// opened over, or the one selected layer.
    pub(crate) fn list_instance(&self, hit: Option<NodeId>) -> Option<NodeId> {
        hit.or(self.session.selection.single())
    }

    /// *Make a list from data*: pick a data file for instance `id`, read it on
    /// the worker, and make the list when it lands. Refused on the status line
    /// with the reason when `id` cannot be a list's template.
    pub(crate) fn make_list_dialog(&mut self, id: NodeId) {
        if let Some(why) = list::why_not(&self.session.doc, id) {
            return self.session.info(why);
        }
        let Some(path) = rfd::FileDialog::new()
            .add_filter(FILTER.0, FILTER.1)
            .pick_file()
        else {
            return;
        };
        self.read_list_file(path, None, |path, linkable| Then::List {
            instance: id,
            path,
            linkable,
        });
    }

    /// *Make a list from data ▸ products.csv*: instance `id` made a list over data
    /// set `set` at once, filled by name; the card changes the fill.
    pub(crate) fn make_list_over(&mut self, id: NodeId, set: DataSetId) {
        let Some(name) = self.session.doc.data_set(set).map(|d| d.name.clone()) else {
            return;
        };
        self.commit_make_list(id, list::Source::Existing(set), Some(name));
    }

    /// *Make a list from data ▸ New table*: instance `id` made a list over a new
    /// table holding its own values as row 1 (`list::seed_table`) and an empty
    /// row 2, and the data window opened typing into row 2's first cell, the
    /// next thing to write.
    pub(crate) fn make_list_new_table(&mut self, id: NodeId) {
        if let Some(why) = list::why_not(&self.session.doc, id) {
            return self.session.info(why);
        }
        let mut table = list::seed_table(&self.session.doc, id);
        table.rows.push(vec![String::new(); table.columns.len()]);
        let set = DataSet {
            id: self.session.ids.mint_data_set(),
            name: list::free_name(&self.session.doc, "Table 1"),
            table,
            source: None,
            pictures: Vec::new(),
        };
        let name = set.name.clone();
        if let Some(frame) = self.commit_make_list(id, list::Source::New(set), Some(name)) {
            self.open_list_data(frame);
            self.type_into_cell((1, 0));
        }
    }

    /// *From a file…* on list `id`'s data dropdown: a new data set read from a
    /// picked file, the list moved onto it.
    pub(crate) fn list_data_dialog(&mut self, id: NodeId) {
        let Some(path) = rfd::FileDialog::new()
            .add_filter(FILTER.0, FILTER.1)
            .pick_file()
        else {
            return;
        };
        self.read_list_file(path, None, |path, linkable| Then::ListData {
            list: id,
            path,
            linkable,
        });
    }

    /// Read the data file at `path` (or a browser drop's `bytes`) on the chart
    /// import's worker, landing as `then` makes of it.
    pub(crate) fn read_list_file(
        &mut self,
        path: PathBuf,
        bytes: Option<Vec<u8>>,
        then: impl FnOnce(PathBuf, bool) -> Then,
    ) {
        let name = list::name_for_path(&path);
        let Some(format) = table_io::Format::of_name(&name) else {
            return self
                .session
                .fail(format!("Cannot read {name}: not a CSV, TSV or JSON file"));
        };
        let linkable = bytes.is_none();
        let source = match bytes {
            Some(b) => Source::Bytes(b),
            None => Source::File(path.clone()),
        };
        self.start_chart_read(&name, source, format, vec![None], then(path, linkable));
    }

    /// Land a data file read for a list, by what it was for. A refusal is said on
    /// the status line.
    pub(crate) fn land_list_read(&mut self, then: Then, answer: Result<Vec<Parsed>, String>) {
        let parsed = match answer.map(|mut all| all.pop()) {
            Ok(Some(p)) => p,
            Ok(None) => return,
            Err(why) => {
                let path = match &then {
                    Then::List { path, .. }
                    | Then::ListData { path, .. }
                    | Then::ChartData { path, .. }
                    | Then::Table { path, .. } => path.to_string_lossy().into_owned(),
                    Then::DataSet { path, .. } => path.clone(),
                    _ => String::new(),
                };
                return self
                    .session
                    .fail(format!("Cannot read {}: {why}", file_name(&path)));
            }
        };
        match then {
            // The import box, its Fill section on the columns of their names.
            Then::List {
                instance,
                path,
                linkable,
            } => {
                match list::why_not(&self.session.doc, instance) {
                    None => self.open_list_import_box(instance, path, linkable, parsed),
                    // The instance stopped qualifying while the file was read.
                    Some(why) => self.session.info(why),
                }
            }
            Then::ListData {
                list: id,
                path,
                linkable,
            } => {
                let set = self.data_set_from(&path, linkable, &parsed);
                self.move_list_to_new_set(id, set);
            }
            Then::ChartData {
                chart,
                path,
                linkable,
            } => {
                let set = self.data_set_from(&path, linkable, &parsed);
                self.move_chart_to_set(chart, set.id, Some(set));
            }
            Then::Table { path, linkable } => {
                self.open_table_import_box(path, linkable, parsed);
            }
            Then::DataSet { set, path, relink } => {
                self.replace_data_set_table(set, &path, &parsed, relink);
            }
            _ => {}
        }
    }

    /// A new data set holding `parsed`'s table, named for its file, linked to it
    /// when `linkable`.
    fn data_set_from(&mut self, path: &Path, linkable: bool, parsed: &Parsed) -> DataSet {
        let name = list::free_name(&self.session.doc, &list::name_for_path(path));
        DataSet {
            id: self.session.ids.mint_data_set(),
            name,
            table: parsed.read.table.clone(),
            source: linkable.then(|| link_for(path, parsed, Some(parsed.read.header))),
            pictures: Vec::new(),
        }
    }

    /// The import box's *Make list*: a data set of `table` named after `path`,
    /// linked by `link`, and the list around `instance` over it, filled as
    /// `fill` says (target and column name), one step.
    pub(crate) fn make_list_from_import(
        &mut self,
        instance: NodeId,
        path: &Path,
        table: mastik_core::chart::Table,
        link: Option<DataLink>,
        dir: Option<&Path>,
        fill: &[(list::Target, Option<String>)],
    ) {
        let fill: Vec<list::Fill> = fill
            .iter()
            .filter_map(|(target, col)| {
                let col = col.as_ref()?;
                let column = table.columns.iter().find(|c| c.name == *col)?.id;
                Some(list::Fill {
                    target: target.clone(),
                    column,
                })
            })
            .collect();
        // The status line names the set as made, numbered past a taken name.
        let name = list::free_name(&self.session.doc, &list::name_for_path(path));
        let columns = picture_columns_of(&self.session.doc, instance, &fill);
        let (images, pictures) = read_pictures(&self.session.doc, dir, &table, &columns);
        let set = DataSet {
            id: self.session.ids.mint_data_set(),
            name: name.clone(),
            table,
            source: link,
            pictures,
        };
        self.commit_make_list_filled(
            instance,
            list::Source::New(set),
            Some(name),
            Some(fill),
            images,
        );
    }

    /// Commit a list made around `instance` over `source`, undo step *Make
    /// list*; selects it and says what it made from `from`, a data set's name.
    /// Answers the list frame once committed.
    fn commit_make_list(
        &mut self,
        instance: NodeId,
        source: list::Source,
        from: Option<String>,
    ) -> Option<NodeId> {
        self.commit_make_list_filled(instance, source, from, None, Vec::new())
    }

    /// [`Self::commit_make_list`] with `fill` in place of the fill by name, and
    /// `images`, the pictures its cells name, added in the same step.
    fn commit_make_list_filled(
        &mut self,
        instance: NodeId,
        source: list::Source,
        from: Option<String>,
        fill: Option<Vec<list::Fill>>,
        images: Vec<Operation>,
    ) -> Option<NodeId> {
        let existing = match &source {
            list::Source::Existing(set) => Some(*set),
            list::Source::New(_) => None,
        };
        let made = list::make(
            &self.session.doc,
            &self.session.resolved,
            &mut self.session.ids,
            instance,
            source,
        );
        let doc = &self.session.doc;
        let made = made.map(|(mut tx, frame)| {
            let mut ahead = None;
            for op in &mut tx.0 {
                if let Operation::SetList { id, list: Some(l) } = op {
                    if let Some(fill) = &fill {
                        l.fill = fill.clone();
                    }
                    // A table already in the document has not been read for
                    // this template's pictures; a new one came with its own.
                    if let Some(set) = existing.and_then(|s| doc.data_set(s)) {
                        let columns = picture_columns_of(doc, instance, &l.fill);
                        ahead = set_ahead(doc, set, (*id, l), &columns, false);
                    }
                }
            }
            let mut front = images;
            front.extend(ahead.into_iter().flatten());
            tx.0.splice(0..0, front);
            (tx, frame)
        });
        let (tx, frame) = match made {
            Ok(m) => m,
            Err(why) => {
                self.session.fail(why);
                return None;
            }
        };
        self.session.name_step("Make list", icon::TABLE);
        if !self.commit_edit(tx) {
            return None;
        }
        {
            self.session.selection.set_one(frame);
            let rows = self
                .session
                .doc
                .get(frame)
                .and_then(|n| n.list())
                .and_then(|l| self.session.doc.data_set(l.data))
                .map_or(0, |d| d.table.rows.len());
            let rows = crate::ui::count_as(rows, "row", "rows");
            self.session.info(match from {
                Some(name) => format!("Made a list from {name} \u{b7} {rows}"),
                None => format!("Made a list \u{b7} {rows}"),
            });
        }
        Some(frame)
    }

    /// List `id` moved onto the new data set `set`, added in the same step: each
    /// property keeps its column where the new table has one of that name.
    pub(crate) fn move_list_to_new_set(&mut self, id: NodeId, set: DataSet) {
        let Some(now) = self.session.doc.get(id).and_then(|n| n.list()).cloned() else {
            return;
        };
        let next = self.list_on(id, &now, &set);
        let name = set.name.clone();
        let columns = picture_columns(&self.session.doc, id, &next.fill);
        if self.commit_list_onto(id, &set, next, &columns, true, "List data") {
            self.session.info(format!("The list reads {name}"));
        }
    }

    /// *New table* in *Data tables*: *Table 3* (numbered past the taken names)
    /// with one empty column, *Column 1*, and one empty row, one undo step
    /// *New table*; the window moves onto it, typing into its first cell.
    pub(crate) fn new_table(&mut self) {
        let set = DataSet {
            id: self.session.ids.mint_data_set(),
            name: list::free_name(&self.session.doc, "Table 1"),
            table: mastik_core::chart::Table {
                columns: vec![mastik_core::chart::Column {
                    id: mastik_core::chart::ColumnId(0),
                    name: "Column 1".into(),
                }],
                rows: vec![vec![String::new()]],
            },
            source: None,
            pictures: Vec::new(),
        };
        let id = set.id;
        self.session.name_step("New table", icon::PLUS);
        if self.commit_edit(Transaction(vec![Operation::SetDataSet {
            id,
            set: Some(Box::new(set)),
        }])) {
            self.show_table(id);
            self.type_into_cell((0, 0));
        }
    }

    /// *From a file…* in *Data tables*: a picked file read on the worker into
    /// the import box, which adds it as a new linked table.
    pub(crate) fn table_file_dialog(&mut self) {
        let Some(path) = rfd::FileDialog::new()
            .add_filter(FILTER.0, FILTER.1)
            .pick_file()
        else {
            return;
        };
        self.read_list_file(path, None, |path, linkable| Then::Table { path, linkable });
    }

    /// The import box's *Import* in *Data tables*: `table` added as a new data
    /// set named for `path`, linked by `link`, one undo step *New table*; the
    /// window moves onto it.
    pub(crate) fn add_table_from_import(
        &mut self,
        path: &Path,
        table: mastik_core::chart::Table,
        link: Option<DataLink>,
    ) {
        let rows = crate::ui::count_as(table.rows.len(), "row", "rows");
        let set = DataSet {
            id: self.session.ids.mint_data_set(),
            name: list::free_name(&self.session.doc, &list::name_for_path(path)),
            table,
            source: link,
            pictures: Vec::new(),
        };
        let (id, name) = (set.id, set.name.clone());
        self.session.name_step("New table", icon::IMPORT);
        if self.commit_edit(Transaction(vec![Operation::SetDataSet {
            id,
            set: Some(Box::new(set)),
        }])) {
            self.show_table(id);
            self.session.info(format!("Imported {name} \u{b7} {rows}"));
        }
    }

    /// *Delete table*: data set `set` removed, one undo step *Delete table*,
    /// refused while anything reads it; *Data tables* moves to the next table.
    /// A linked table's file is never touched.
    pub(crate) fn delete_table(&mut self, set: DataSetId) {
        let doc = &self.session.doc;
        let Some(at) = doc.data_sets().iter().position(|d| d.id == set) else {
            return;
        };
        if list::is_read(doc, set) {
            return;
        }
        let name = doc.data_sets()[at].name.clone();
        let next = doc
            .data_sets()
            .get(at + 1)
            .or_else(|| at.checked_sub(1).and_then(|i| doc.data_sets().get(i)))
            .map(|d| d.id);
        self.session.name_step("Delete table", icon::TRASH);
        if self.commit_edit(Transaction(vec![Operation::SetDataSet {
            id: set,
            set: None,
        }])) {
            if let Some(next) = next {
                self.show_table(next);
            }
            self.session.info(format!("Deleted {name}"));
        }
    }

    /// Chart `id` moved onto data set `set`, undo step *Chart data*: the set's
    /// table in place of its own, the mapping kept on the columns of its names
    /// (`Chart::reading`). `new` is a set made for it, added in the same step.
    pub(crate) fn move_chart_to_set(&mut self, id: NodeId, set: DataSetId, new: Option<DataSet>) {
        let Some(chart) = self.session.doc.get(id).and_then(|n| n.chart()).cloned() else {
            return;
        };
        let Some(data) = new
            .clone()
            .or_else(|| self.session.doc.data_set(set).cloned())
        else {
            return;
        };
        let mut ops: Vec<Operation> = new
            .map(|d| Operation::SetDataSet {
                id: set,
                set: Some(Box::new(d)),
            })
            .into_iter()
            .collect();
        ops.push(Operation::SetChart {
            id,
            chart: Some(Box::new(chart.reading(set, &data.table))),
        });
        self.session.name_step("Chart data", icon::TABLE);
        if self.commit_edit(Transaction(ops)) {
            self.session.info(format!("The chart reads {}", data.name));
        }
    }

    /// Chart `id` back on a table of its own, undo step *Chart data*: it keeps
    /// the data set's rows as they are now.
    pub(crate) fn chart_own_table(&mut self, id: NodeId) {
        let Some(chart) = self.session.doc.get(id).and_then(|n| n.chart()).cloned() else {
            return;
        };
        if chart.data.is_none() {
            return;
        }
        self.session.name_step("Chart data", icon::TABLE);
        self.commit_edit(Transaction(vec![Operation::SetChart {
            id,
            chart: Some(Box::new(mastik_core::chart::Chart {
                data: None,
                ..chart
            })),
        }]));
    }

    /// *Save as table* on chart `id`, which reads no data set of the
    /// document's: its table, column ids and linked file kept, added as a data
    /// set named for the chart and read by it, one undo step *Save as table*.
    /// Answers the new set.
    pub(crate) fn save_chart_table(&mut self, id: NodeId) -> Option<DataSetId> {
        if self.chart_set(id).is_some() {
            return None;
        }
        let node = self.session.doc.get(id)?;
        let chart = node.chart()?.clone();
        let wanted = match node.name().trim() {
            "" => "Table 1",
            n => n,
        };
        let wanted: String = wanted.chars().take(list::MAX_NAME_CHARS).collect();
        let set = DataSet {
            id: self.session.ids.mint_data_set(),
            name: list::free_name(&self.session.doc, &wanted),
            table: chart.table.clone(),
            source: chart.source.clone(),
            pictures: Vec::new(),
        };
        let (sid, name) = (set.id, set.name.clone());
        let reads = chart.reading(sid, &set.table);
        self.session.name_step("Save as table", icon::FLOPPY_DISK);
        let landed = self.commit_edit(Transaction(vec![
            Operation::SetDataSet {
                id: sid,
                set: Some(Box::new(set)),
            },
            Operation::SetChart {
                id,
                chart: Some(Box::new(reads)),
            },
        ]));
        landed.then(|| {
            self.session
                .info(format!("Saved the chart's data as {name}"));
            sid
        })
    }

    /// *From a file…* on chart `id`'s data dropdown: a new data set read from a
    /// picked file, the chart moved onto it.
    pub(crate) fn chart_data_dialog(&mut self, id: NodeId) {
        let Some(path) = rfd::FileDialog::new()
            .add_filter(FILTER.0, FILTER.1)
            .pick_file()
        else {
            return;
        };
        self.read_list_file(path, None, |path, linkable| Then::ChartData {
            chart: id,
            path,
            linkable,
        });
    }

    /// `now` moved onto data set `set` as `set_list` moves it
    /// (`verbs::moved_onto`): fills, filter tests and sort keys keep their
    /// columns by name, and a property left unfilled takes its own name's.
    pub(crate) fn list_on(&self, id: NodeId, now: &List, set: &list::DataSet) -> List {
        mastik_core::verbs::moved_onto(&self.session.doc, id, now, set)
    }

    /// *Update*: read data set `set`'s linked file again. A click while that
    /// read is pending reads nothing more.
    pub(crate) fn update_data_set(&mut self, set: DataSetId) {
        let Some(link) = self
            .session
            .doc
            .data_set(set)
            .and_then(|d| d.source.clone())
        else {
            return;
        };
        if self.reading(|t| matches!(t, Then::DataSet { set: s, .. } if *s == set)) {
            return;
        }
        let name = file_name(&link.path);
        let Some(format) = table_io::Format::of_name(&name) else {
            return;
        };
        let then = Then::DataSet {
            set,
            path: link.path.clone(),
            relink: false,
        };
        let source = Source::File(PathBuf::from(&link.path));
        self.start_chart_read(&name, source, format, vec![link.header], then);
    }

    /// *Relink…* (and the data window's *Import…*): pick a file for data set
    /// `set` and read it into the same set, linked to it, under the set's header
    /// choice, so every list over it follows.
    pub(crate) fn relink_data_set(&mut self, set: DataSetId) {
        let Some(data) = self.session.doc.data_set(set) else {
            return;
        };
        let header = data.source.as_ref().and_then(|l| l.header);
        let Some(path) = rfd::FileDialog::new()
            .add_filter(FILTER.0, FILTER.1)
            .pick_file()
        else {
            return;
        };
        let name = list::name_for_path(&path);
        let Some(format) = table_io::Format::of_name(&name) else {
            return self
                .session
                .fail(format!("Cannot read {name}: not a CSV, TSV or JSON file"));
        };
        let then = Then::DataSet {
            set,
            path: path.to_string_lossy().into_owned(),
            relink: true,
        };
        self.start_chart_read(&name, Source::File(path), format, vec![header], then);
    }

    /// The lists reading data set `set`, in id order.
    pub(crate) fn lists_over(&self, set: DataSetId) -> Vec<NodeId> {
        let doc = &self.session.doc;
        list::lists(doc)
            .into_iter()
            .filter(|l| {
                doc.get(*l)
                    .and_then(|n| n.list())
                    .is_some_and(|x| x.data == set)
            })
            .collect()
    }

    /// How many lists and charts read data set `set`.
    pub(crate) fn readers_of(&self, set: DataSetId) -> (usize, usize) {
        (
            self.lists_over(set).len(),
            list::charts_over(&self.session.doc, set).len(),
        )
    }

    /// Data set `set`'s readers in words: *2 lists, 1 chart*, *1 list*; empty
    /// when nothing reads it.
    pub(crate) fn readers_words(&self, set: DataSetId) -> String {
        let (lists, charts) = self.readers_of(set);
        let mut out = Vec::new();
        if lists > 0 {
            out.push(crate::ui::count_as(lists, "list", "lists"));
        }
        if charts > 0 {
            out.push(crate::ui::count_as(charts, "chart", "charts"));
        }
        out.join(", ")
    }

    /// Data set `set`'s table replaced with `parsed`'s and its link restamped to
    /// `path`, undo step *Update table* (*Relink table* when `relink`);
    /// every list reading it follows in the step, each fill kept on the column
    /// of its name and dropped where the new table has none.
    pub(crate) fn replace_data_set_table(
        &mut self,
        set: DataSetId,
        path: &str,
        parsed: &Parsed,
        relink: bool,
    ) {
        let Some(now) = self.session.doc.data_set(set).cloned() else {
            return;
        };
        // An *Update* landing after *Unlink*, or a relink elsewhere, would undo
        // it and overwrite the edits made since.
        if !relink && now.source.as_ref().map(|l| l.path.as_str()) != Some(path) {
            return;
        }
        let header = now.source.as_ref().and_then(|s| s.header);
        // A set never linked (*New table*) is linked by the window's *Import…*.
        let first_link = relink && now.source.is_none();
        // Every reader keeps its columns by id: a chart on its own set does, so
        // the re-read columns take the old ids by name.
        let mut table = parsed.read.table.clone();
        table.take_ids_by_name(&now.table);
        let mut ops = Vec::new();
        let mut lost = 0;
        let mut columns = Vec::new();
        let over = self.lists_over(set);
        for l in &over {
            let Some(was) = self.session.doc.get(*l).and_then(|n| n.list()).cloned() else {
                continue;
            };
            let fill: Vec<list::Fill> = was
                .fill
                .iter()
                .filter_map(|f| {
                    let name = &now.table.column(f.column)?.name;
                    let col = table.columns.iter().find(|c| c.name == *name)?;
                    Some(list::Fill {
                        target: f.target.clone(),
                        column: col.id,
                    })
                })
                .collect();
            lost += was.fill.len() - fill.len();
            columns.extend(picture_columns(&self.session.doc, *l, &fill));
            if fill != was.fill {
                ops.push(Operation::SetList {
                    id: *l,
                    list: Some(Box::new(List { fill, ..was })),
                });
            }
        }
        // The pictures the image columns name, read beside the file again, in
        // the rows the lists draw from the new table.
        let dir = Path::new(path).parent();
        let rows = drawn_rows(&self.session.doc, Some(set), &table, None);
        let (images, pictures) = read_pictures_in(&self.session.doc, dir, &table, &columns, &rows);
        let next = DataSet {
            table,
            source: Some(link_for(Path::new(path), parsed, header)),
            pictures,
            ..now
        };
        ops.insert(
            0,
            Operation::SetDataSet {
                id: set,
                set: Some(Box::new(next)),
            },
        );
        ops.splice(0..0, images);
        let (label, glyph) = match (relink, first_link) {
            (true, true) => ("Link table", icon::LINK_SIMPLE),
            (true, false) => ("Relink table", icon::LINK_SIMPLE),
            (false, _) => ("Update table", icon::IMPORT),
        };
        self.session.name_step(label, glyph);
        if self.commit_edit(Transaction(ops)) {
            let rows = crate::ui::count_as(parsed.read.table.rows.len(), "row", "rows");
            let name = file_name(path);
            let readers = self.readers_words(set);
            let mut said = match (relink, first_link, readers.is_empty()) {
                (true, true, true) => format!("Linked {name} \u{b7} {rows}"),
                (true, true, false) => format!("Linked {name} for {readers} \u{b7} {rows}"),
                (true, false, true) => format!("Relinked {name} \u{b7} {rows}"),
                (true, false, false) => format!("Relinked {name} for {readers} \u{b7} {rows}"),
                (false, ..) => format!("Updated {name} \u{b7} {rows}"),
            };
            if lost > 0 {
                said.push_str(&format!(
                    " \u{b7} {} lost {} column",
                    crate::ui::count_as(lost, "fill", "fills"),
                    if lost == 1 { "its" } else { "their" }
                ));
            }
            self.session.info(said);
        }
    }

    /// *Unlink*: data set `set` keeps its table and drops its file.
    pub(crate) fn unlink_data_set(&mut self, set: DataSetId) {
        let Some(now) = self.session.doc.data_set(set).cloned() else {
            return;
        };
        if now.source.is_none() {
            return;
        }
        self.session
            .name_step("Unlink data file", icon::LINK_SIMPLE);
        self.commit_edit(Transaction(vec![Operation::SetDataSet {
            id: set,
            set: Some(Box::new(DataSet {
                source: None,
                ..now
            })),
        }]));
    }

    /// *Detach list*: each of `ids` keeps its rows as plain instances and drops
    /// its description, one undo step.
    pub(crate) fn detach_lists(&mut self, ids: &[NodeId]) {
        let tx = list::detach(&self.session.doc, ids);
        if tx.0.is_empty() {
            return;
        }
        self.session.name_step("Detach list", icon::STACK_SIMPLE);
        self.commit_edit(tx);
    }

    /// The first data set read by a list or chart whose linked file changed on
    /// disk, with how many lists and charts read it.
    fn changed_data_set(&self) -> Option<(DataSetId, String, (usize, usize))> {
        let doc = &self.session.doc;
        doc.data_sets().iter().find_map(|d| {
            let path = d.source.as_ref()?.path.clone();
            (self.data_link_state(d.id) == Some(LinkState::Changed)).then_some(())?;
            let n = self.readers_of(d.id);
            (n != (0, 0)).then_some((d.id, path, n))
        })
    }

    /// The pill beside the charts' one: *products.csv changed · Update list*,
    /// its readers counted when several read it (*Update 2 lists, 1 chart*). A
    /// click updates the data set.
    pub(crate) fn list_link_pill(&mut self, ui: &mut egui::Ui) {
        let Some((set, path, n)) = self.changed_data_set() else {
            return;
        };
        let name = file_name(&path);
        let text = match n {
            (1, 0) => format!("{name} changed \u{b7} Update list"),
            (0, 1) => format!("{name} changed \u{b7} Update chart"),
            _ => format!("{name} changed \u{b7} Update {}", self.readers_words(set)),
        };
        let tip = format!("{name} changed on disk \u{b7} click to read it into its table");
        if crate::chart_import::link_pill(ui, &text, &tip) {
            self.update_data_set(set);
        }
    }
}
