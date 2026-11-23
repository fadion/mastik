//! Row actions: *Add row*, *Edit row* and *Delete row* on the played copy's
//! tables, the expressions that read tables, and *When a table changes*.
//!
//! A table edited in play is the copy's: it goes through [`Player::commit`],
//! which brings every list and chart over it along, and never reaches the
//! document, the save or undo. *Restart* puts every table back.

use crate::player::{Notice, Player};
use mastik_core::chart::{MAX_CELL_CHARS, MAX_ROWS, Table};
use mastik_core::list::{self, DataSetId};
use mastik_core::prototype::expr::{self, Ref, TableEnv, Unread};
use mastik_core::prototype::{CellWrite, Expr, ExprError, RowEnd, Trigger, WhichRow};
use mastik_core::variable::Value;
use mastik_core::{ItemId, NodeId, Operation, Transaction, subtree_nodes};

/// One table in the state card's *Tables*: a table play wrote this run.
#[derive(Clone, Debug, PartialEq)]
pub struct TableRow {
    pub data: DataSetId,
    pub name: String,
    /// How many rows it has in play.
    pub rows: usize,
    /// How many the document's table has.
    pub in_file: usize,
}

impl Player {
    /// What `e` comes to on the copy, read from a reaction on `holder`, with
    /// `row` the row a row action is on.
    pub(crate) fn eval(
        &self,
        holder: NodeId,
        e: &Expr,
        row: Option<(DataSetId, &[String])>,
    ) -> Result<Value, ExprError> {
        let opened = |d: DataSetId| self.opened.get(&d).copied().unwrap_or(0);
        let env = TableEnv {
            tables: self.doc.data_sets(),
            opened: &opened,
            row,
        };
        e.eval_with(&|r| match r {
            Ref::Var(v) => self.doc.resolve_var(holder, *v).ok_or(Unread::Missing),
            _ => env.read(r),
        })
    }

    fn missing(&mut self, holder: NodeId, action: ItemId) {
        self.notice_once(
            holder,
            action,
            Notice::Missing {
                layer: holder,
                action,
            },
        );
    }

    fn expression(&mut self, holder: NodeId, action: ItemId, error: ExprError) {
        self.notice_once(
            holder,
            action,
            Notice::Expression {
                layer: holder,
                action,
                error,
            },
        );
    }

    /// The cells `cells` write into `row` of `table`, each worked out from
    /// `row` as it was; `{row.…}` reads it when `reads` names its table's id.
    /// `None`, with a notice, when one does not evaluate; a column the table
    /// lacks is passed over.
    fn written(
        &mut self,
        holder: NodeId,
        action: ItemId,
        table: &Table,
        row: &[String],
        reads: Option<DataSetId>,
        cells: &[CellWrite],
    ) -> Option<Vec<String>> {
        let mut out = row.to_vec();
        for c in cells {
            let Some(i) = table.index_of(c.column) else {
                continue;
            };
            let at = reads.map(|set| (set, row));
            match self.eval(holder, &c.to, at) {
                Ok(v) => {
                    out[i] = expr::format_value(&v)
                        .chars()
                        .take(MAX_CELL_CHARS)
                        .collect()
                }
                Err(error) => {
                    self.expression(holder, action, error);
                    return None;
                }
            }
        }
        Some(out)
    }

    /// The row of `set` that `which` names, from a reaction on `holder`.
    /// `None` when there is none: with a notice when the action cannot apply
    /// (*This row* outside a list over the table, a test that does not
    /// evaluate), silently when the table is empty or no row passes the test.
    fn pick(
        &mut self,
        holder: NodeId,
        action: ItemId,
        set: DataSetId,
        which: &WhichRow,
    ) -> Option<usize> {
        let len = self.doc.data_set(set)?.table.rows.len();
        match which {
            WhichRow::This => {
                let at = list::row_at(&self.doc, holder).filter(|(l, ..)| {
                    self.doc
                        .get(*l)
                        .and_then(|n| n.list())
                        .is_some_and(|x| x.data == set)
                });
                match at {
                    Some((_, _, row)) if row < len => Some(row),
                    Some(_) => None,
                    None => {
                        self.missing(holder, action);
                        None
                    }
                }
            }
            WhichRow::Opened => {
                let row = self.opened.get(&set).copied().unwrap_or(0);
                (len > 0).then(|| row.min(len - 1))
            }
            WhichRow::FirstWhere { when } => {
                let rows = self.doc.data_set(set)?.table.rows.clone();
                for (i, row) in rows.iter().enumerate() {
                    match self.eval(holder, when, Some((set, row))) {
                        Ok(Value::Boolean(true)) => return Some(i),
                        Ok(Value::Boolean(false)) => {}
                        Ok(_) => {
                            let error = ExprError::Type {
                                at: 0..when.0.len(),
                                msg: "a test comes to true or false".into(),
                            };
                            self.expression(holder, action, error);
                            return None;
                        }
                        Err(error) => {
                            self.expression(holder, action, error);
                            return None;
                        }
                    }
                }
                None
            }
        }
    }

    /// *Add row*: a row at `at` of `set`, its listed cells worked out and the
    /// rest empty. Skipped with a notice on a full table.
    pub(crate) fn add_row(
        &mut self,
        holder: NodeId,
        action: ItemId,
        set: DataSetId,
        at: RowEnd,
        cells: &[CellWrite],
    ) {
        let Some(d) = self.doc.data_set(set).cloned() else {
            return self.missing(holder, action);
        };
        if d.table.rows.len() >= MAX_ROWS {
            return self.notice_once(
                holder,
                action,
                Notice::TableFull {
                    layer: holder,
                    action,
                    table: d.name.clone(),
                },
            );
        }
        let empty = vec![String::new(); d.table.columns.len()];
        let Some(row) = self.written(holder, action, &d.table, &empty, None, cells) else {
            return;
        };
        let mut table = d.table;
        match at {
            RowEnd::End => table.rows.push(row),
            RowEnd::Start => table.rows.insert(0, row),
        }
        self.write_table(set, table);
    }

    /// *Edit row*: the listed cells of the row `which` names, each worked out
    /// from the row as it was before the edit.
    pub(crate) fn edit_row(
        &mut self,
        holder: NodeId,
        action: ItemId,
        set: DataSetId,
        which: &WhichRow,
        cells: &[CellWrite],
    ) {
        let Some(d) = self.doc.data_set(set).cloned() else {
            return self.missing(holder, action);
        };
        let Some(i) = self.pick(holder, action, set, which) else {
            return;
        };
        let was = d.table.rows[i].clone();
        let Some(row) = self.written(holder, action, &d.table, &was, Some(set), cells) else {
            return;
        };
        let mut table = d.table;
        table.rows[i] = row;
        self.write_table(set, table);
    }

    /// *Delete row*: the row `which` names.
    pub(crate) fn delete_row(
        &mut self,
        holder: NodeId,
        action: ItemId,
        set: DataSetId,
        which: &WhichRow,
    ) {
        let Some(d) = self.doc.data_set(set).cloned() else {
            return self.missing(holder, action);
        };
        let Some(i) = self.pick(holder, action, set, which) else {
            return;
        };
        let mut table = d.table;
        table.rows.remove(i);
        self.write_table(set, table);
    }

    /// Give the copy's table `set` the rows of `table`: its lists and charts
    /// follow (`Player::commit`, which also notes the table as played) and
    /// *When a table changes* fires.
    fn write_table(&mut self, set: DataSetId, table: Table) {
        let Some(d) = self.doc.data_set(set).cloned() else {
            return;
        };
        if d.table == table {
            return;
        }
        let tx = Transaction(vec![Operation::SetDataSet {
            id: set,
            set: Some(Box::new(list::DataSet { table, ..d })),
        }]);
        if self.commit(tx).is_ok() {
            self.table_changed(set);
        }
    }

    /// Every *When a table changes* reaction on `set` in the shown frames runs.
    fn table_changed(&mut self, set: DataSetId) {
        let mut fire = Vec::new();
        for frame in self.shown() {
            for id in subtree_nodes(&self.doc, &[frame]) {
                let Some(n) = self.doc.get(id) else { continue };
                for r in n.reactions() {
                    if r.value.trigger == (Trigger::TableChanges { table: set }) {
                        fire.push((id, r.id));
                    }
                }
            }
        }
        for (id, reaction) in fire {
            self.run_reaction(id, reaction, None);
        }
    }

    /// Table `set` of the copy put back as the document has it.
    pub(crate) fn reset_table(&mut self, set: DataSetId) -> bool {
        let Some(table) = self.base.data_set(set).map(|d| d.table.clone()) else {
            return false;
        };
        let changed = self.doc.data_set(set).is_some_and(|d| d.table != table);
        self.write_table(set, table);
        self.tables.remove(&set);
        changed
    }

    /// The state card's *Tables*: each table play wrote this run, by a row
    /// action or a script, in the document's order.
    pub fn tables(&self) -> Vec<TableRow> {
        self.doc
            .data_sets()
            .iter()
            .filter(|d| self.tables.contains_key(&d.id))
            .map(|d| TableRow {
                data: d.id,
                name: d.name.clone(),
                rows: d.table.rows.len(),
                in_file: self.base.data_set(d.id).map_or(0, |b| b.table.rows.len()),
            })
            .collect()
    }

    /// The tables play changed, as the copy holds them, taken before a
    /// re-clone puts the document's back.
    pub(crate) fn played_tables(&self) -> Vec<(DataSetId, Table)> {
        self.tables
            .keys()
            .filter_map(|id| Some((*id, self.doc.data_set(*id)?.table.clone())))
            .collect()
    }

    /// After a live edit: each table play changed whose document table is
    /// still the one play started from gets its played rows back (`played`,
    /// taken before a re-clone); one the edit changed is the document's again.
    pub(crate) fn carry_tables(&mut self, played: Vec<(DataSetId, Table)>) {
        for (id, rows) in played {
            let same = self
                .base
                .data_set(id)
                .is_some_and(|b| self.tables.get(&id) == Some(&b.table));
            if !same {
                self.tables.remove(&id);
                continue;
            }
            if self.doc.data_set(id).is_some_and(|d| d.table != rows)
                && let Some(d) = self.doc.data_set(id).cloned()
            {
                let tx = Transaction(vec![Operation::SetDataSet {
                    id,
                    set: Some(Box::new(list::DataSet { table: rows, ..d })),
                }]);
                // A refusal leaves the document's rows, as a dropped table.
                if self.commit(tx).is_err() {
                    self.tables.remove(&id);
                }
            }
        }
    }
}
