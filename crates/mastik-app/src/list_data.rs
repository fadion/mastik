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

/// Whether list `id` reads a data set with no rows.
pub(crate) fn list_is_empty(doc: &mastik_core::Document, id: NodeId) -> bool {
    doc.get(id)
        .and_then(|n| n.list())
        .and_then(|l| doc.data_set(l.data))
        .is_some_and(|d| d.table.rows.is_empty())
}

/// What list `id` draws, in the words its canvas tab and layers badge show,
/// with the ink: *24 rows*, *200 of 1240 rows*, *Opened row*, or *No rows* and
/// *Missing data* in the warning ink.
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
    if l.rows == list::Rows::Opened {
        return ("Opened row".into(), text::dim());
    }
    let drawn = l.shown_rows(rows).len();
    let said = if drawn < rows && drawn == list::MAX_DRAWN {
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
/// file in `dir`, in the rows a list can draw: read and decoded now, each new
/// picture's `AddImage` and every (cell, picture) pair. A cell already naming a
/// picture's id, one that is no plain relative path (absolute, or climbing out
/// with `..`), or one that does not read as a picture is left out.
pub(crate) fn read_pictures(
    doc: &mastik_core::Document,
    dir: Option<&Path>,
    table: &mastik_core::chart::Table,
    columns: &[mastik_core::chart::ColumnId],
) -> (Vec<Operation>, Vec<(String, mastik_core::ImageId)>) {
    let (mut ops, mut pairs) = (Vec::new(), Vec::<(String, mastik_core::ImageId)>::new());
    let Some(dir) = dir else {
        return (ops, pairs);
    };
    for col in columns {
        let Some(c) = table.index_of(*col) else {
            continue;
        };
        for row in table.rows.iter().take(list::MAX_DRAWN) {
            let cell = row[c].trim();
            if cell.is_empty()
                || pairs.iter().any(|(p, _)| p == cell)
                || doc.image(&mastik_core::ImageId(cell.to_string())).is_some()
            {
                continue;
            }
            let rel = Path::new(cell);
            let plain = rel.components().all(|k| {
                matches!(
                    k,
                    std::path::Component::Normal(_) | std::path::Component::CurDir
                )
            });
            if !plain {
                continue;
            }
            let path = dir.join(rel);
            let fits = std::fs::metadata(&path)
                .is_ok_and(|m| m.is_file() && m.len() <= crate::tokens_import::MAX_READ);
            let Some(bytes) = fits.then(|| std::fs::read(&path).ok()).flatten() else {
                continue;
            };
            let ext = crate::app::file_extension(&path);
            let name = Some(crate::app::image_name(&path));
            let Ok((image, _)) = crate::app::read_picture(bytes, ext, name) else {
                continue;
            };
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
                if list::why_not(&self.session.doc, instance).is_none() {
                    self.open_list_import_box(instance, path, linkable, parsed);
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
        let name = list::name_for_path(path);
        let columns = picture_columns_of(&self.session.doc, instance, &fill);
        let (images, pictures) = read_pictures(&self.session.doc, dir, &table, &columns);
        let set = DataSet {
            id: self.session.ids.mint_data_set(),
            name: list::free_name(&self.session.doc, &name),
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
        let made = list::make(
            &self.session.doc,
            &self.session.resolved,
            &mut self.session.ids,
            instance,
            source,
        );
        let made = made.map(|(mut tx, frame)| {
            tx.0.splice(0..0, images);
            if let Some(fill) = fill {
                for op in &mut tx.0 {
                    if let Operation::SetList { list: Some(l), .. } = op {
                        l.fill = fill.clone();
                    }
                }
            }
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
    fn move_list_to_new_set(&mut self, id: NodeId, set: DataSet) {
        let Some(now) = self.session.doc.get(id).and_then(|n| n.list()).cloned() else {
            return;
        };
        let next = self.list_on(id, &now, &set.table, set.id);
        let name = set.name.clone();
        let tx = Transaction(vec![
            Operation::SetDataSet {
                id: set.id,
                set: Some(Box::new(set)),
            },
            Operation::SetList {
                id,
                list: Some(Box::new(next)),
            },
        ]);
        self.session.name_step("List data", icon::TABLE);
        if self.commit_edit(tx) {
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

    /// `now` moved onto data set `data` of `table`: a fill keeps its column where
    /// the table has one of the same name; a property left unfilled takes the
    /// column matching its own name.
    pub(crate) fn list_on(
        &self,
        id: NodeId,
        now: &List,
        table: &mastik_core::chart::Table,
        data: DataSetId,
    ) -> List {
        let doc = &self.session.doc;
        let old = doc.data_set(now.data).map(|d| &d.table);
        let mut fill: Vec<list::Fill> = now
            .fill
            .iter()
            .filter_map(|f| {
                let name = &old?.column(f.column)?.name;
                let col = table.columns.iter().find(|c| c.name == *name)?;
                Some(list::Fill {
                    target: f.target.clone(),
                    column: col.id,
                })
            })
            .collect();
        if let Some(t) = list::template(doc, id) {
            for f in list::fill_by_name(doc, t, table) {
                if !fill.iter().any(|g| g.target == f.target) {
                    fill.push(f);
                }
            }
        }
        List {
            data,
            fill,
            ..now.clone()
        }
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
    /// `path`, undo step *Update list data* (*Relink list data* when `relink`);
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
        let header = now.source.as_ref().and_then(|s| s.header);
        // A set never linked (*New table*) is linked by the window's *Import…*.
        let first_link = relink && now.source.is_none();
        let table = parsed.read.table.clone();
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
        // The pictures the image columns name, read beside the file again.
        let dir = Path::new(path).parent();
        let (images, pictures) = read_pictures(&self.session.doc, dir, &table, &columns);
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
            (true, true) => ("Link list data", icon::LINK_SIMPLE),
            (true, false) => ("Relink list data", icon::LINK_SIMPLE),
            (false, _) => ("Update list data", icon::IMPORT),
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
