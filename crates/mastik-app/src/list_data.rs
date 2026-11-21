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

/// Whether list `id` reads a data set with no rows.
pub(crate) fn list_is_empty(doc: &mastik_core::Document, id: NodeId) -> bool {
    doc.get(id)
        .and_then(|n| n.list())
        .and_then(|l| doc.data_set(l.data))
        .is_some_and(|d| d.table.rows.is_empty())
}

/// What list `id` draws, in the words its canvas tab and layers badge show,
/// with the ink: *24 rows*, *200 of 1,240 rows*, *Opened row*, or *No rows* and
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
                    Then::List { path, .. } | Then::ListData { path, .. } => {
                        path.to_string_lossy().into_owned()
                    }
                    Then::DataSet { path, .. } => path.clone(),
                    _ => String::new(),
                };
                return self
                    .session
                    .fail(format!("Cannot read {}: {why}", file_name(&path)));
            }
        };
        match then {
            Then::List {
                instance,
                path,
                linkable,
            } => {
                let set = self.data_set_from(&path, linkable, &parsed);
                self.commit_make_list(instance, list::Source::New(set), Some(&path));
            }
            Then::ListData {
                list: id,
                path,
                linkable,
            } => {
                let set = self.data_set_from(&path, linkable, &parsed);
                self.move_list_to_new_set(id, set);
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
        }
    }

    /// Commit a list made around `instance` over `source`, undo step *Make
    /// list*; selects it and says what it made.
    fn commit_make_list(&mut self, instance: NodeId, source: list::Source, from: Option<&Path>) {
        let made = list::make(
            &self.session.doc,
            &self.session.resolved,
            &mut self.session.ids,
            instance,
            source,
        );
        let (tx, frame) = match made {
            Ok(m) => m,
            Err(why) => return self.session.fail(why),
        };
        self.session.name_step("Make list", icon::TABLE);
        if self.commit_edit(tx) {
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
                Some(p) => format!("Made a list from {} \u{b7} {rows}", list::name_for_path(p)),
                None => format!("Made a list \u{b7} {rows}"),
            });
        }
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

    /// *Relink…*: pick a file for data set `set`, whose linked file is gone, and
    /// read it into the same set under its header choice, so every list over it
    /// follows.
    pub(crate) fn relink_data_set(&mut self, set: DataSetId) {
        let Some(link) = self
            .session
            .doc
            .data_set(set)
            .and_then(|d| d.source.clone())
        else {
            return;
        };
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
        self.start_chart_read(&name, Source::File(path), format, vec![link.header], then);
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
        let table = parsed.read.table.clone();
        let mut ops = Vec::new();
        let mut lost = 0;
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
            if fill != was.fill {
                ops.push(Operation::SetList {
                    id: *l,
                    list: Some(Box::new(List { fill, ..was })),
                });
            }
        }
        let next = DataSet {
            table,
            source: Some(link_for(Path::new(path), parsed, header)),
            ..now
        };
        ops.insert(
            0,
            Operation::SetDataSet {
                id: set,
                set: Some(Box::new(next)),
            },
        );
        let (label, glyph) = match relink {
            true => ("Relink list data", icon::LINK_SIMPLE),
            false => ("Update list data", icon::IMPORT),
        };
        self.session.name_step(label, glyph);
        if self.commit_edit(Transaction(ops)) {
            let rows = crate::ui::count_as(parsed.read.table.rows.len(), "row", "rows");
            let name = file_name(path);
            let mut said = match relink {
                true => format!(
                    "Relinked {name} for {} \u{b7} {rows}",
                    crate::ui::count_as(over.len(), "list", "lists")
                ),
                false => format!("Updated {name} \u{b7} {rows}"),
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

    /// The first data set read by a list whose linked file changed on disk.
    fn changed_data_set(&self) -> Option<(DataSetId, String, usize)> {
        let doc = &self.session.doc;
        doc.data_sets().iter().find_map(|d| {
            let path = d.source.as_ref()?.path.clone();
            (self.data_link_state(d.id) == Some(LinkState::Changed)).then_some(())?;
            let n = self.lists_over(d.id).len();
            (n > 0).then_some((d.id, path, n))
        })
    }

    /// The pill beside the charts' one: *products.csv changed · Update list*,
    /// the count when several lists read it. A click updates the data set.
    pub(crate) fn list_link_pill(&mut self, ui: &mut egui::Ui) {
        let Some((set, path, n)) = self.changed_data_set() else {
            return;
        };
        let name = file_name(&path);
        let text = match n {
            1 => format!("{name} changed \u{b7} Update list"),
            n => format!("{name} changed \u{b7} Update {n} lists"),
        };
        let tip = format!("{name} changed on disk \u{b7} click to read it into its lists");
        if crate::chart_import::link_pill(ui, &text, &tip) {
            self.update_data_set(set);
        }
    }
}
