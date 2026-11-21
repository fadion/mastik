# Lists from data

A list is a frame that repeats a component instance, its **template**, once per row of one of
the document's **data sets**, each copy's component properties filled from its row's cells. The
copies are ordinary instances, saved and drawn as any layer is, and made again whenever the
template, the list's description or its data set changes. Core: `mastik_core::list`; app:
`list_data.rs` and the List card, `panels/list.rs`.

## Model

### Data sets

- **`DataSet { id, name, table, source }`**, in `Document::data_sets()` in order, looked up by
  `DataSetId` (a node-id wire form, minted by `IdSource::mint_data_set`). The `table` is the
  chart's `Table` (`charts.md`, *The description*) under the chart's limits
  (`list::validate_table`), and the name is at most 256 characters (`list::MAX_NAME_CHARS`).
- **A data set is the document's, not a list's**: several lists may read one, and an edit to it
  regenerates every list reading it. It is never drawn, so `SetDataSet` dirties nothing.
- **A data set lives while a list reads it.** After every commit, a data set no list reads (its
  last list moved onto another, detached or deleted) is removed in the same undo step
  (`list::unread_data_sets`), so the dropdown never offers data nothing could pick again.
- **`source: Option<DataLink>`** is the linked file, the chart's `DataLink` (path, the stat of the
  read, the header choice). The table is always the document's own copy.
- **`SetDataSet { id, set }`** adds, replaces or (with `None`) removes one whole data set, refused
  when the set's id is not `id` or `DataSet::validate` refuses it (`OpError::BadList`).

### The list (`list::List`)

- **`Node::list: Option<Box<List>>`**, on a frame only, a flag on a frame as a chart is
  (`charts.md`, *A chart is a frame with a description*). `SetList` writes it whole (`None`
  detaches), refused off a frame (`ListRule::NotAFrame`) or with a copy named twice or the frame
  named as its own copy (`ListRule::BadCopies`). It keeps nothing of the description it replaces.
- **`data`** — the data set it reads.
- **`rows`** — `Rows::All` (every row, up to `MAX_DRAWN`), `Rows::First(n)`, or `Rows::Opened`:
  the one row opened in play (*Play*, below), row 1 in the document.
- **`fill: Vec<Fill { target, column }>`** — which column fills which property.
  `Target::Prop(ItemId)` names a text or boolean component property of the template's main;
  `Target::Variant(name)` a variant property of its set. Columns are named by `ColumnId`.
- **`copies`** — the copies the last regeneration made, in row order; written only by
  `list::regenerate`.
- **`opened`** — the row an `Opened` list shows: 0 in the document, set on the played copy.
- **The template is the frame's first child that is not a copy** (`list::template`). A copy and
  its place are found by `list::row_of`; the list, place and data row a layer sits in by
  `list::row_at`, walking up.

### What a column fills

- **Columns fill component properties, not any layer's field**: a text property takes the cell,
  trimmed; a boolean takes `list::cell_bool` (true, yes, on, 1, y shown; false, no, off, 0, n or
  empty hidden; anything else leaves the property as the template has it); a variant column
  switches the copy to the variant whose value the cell names. Swap and nested properties are
  not filled.
- **A variant cell naming no value** of its property leaves that property where the template
  has it, as does a combination of values no variant of the set has. The card warns of such
  cells (`list::unmatched_variants`).
- **By name** (`list::fill_by_name`): a column fills the property whose name matches its own,
  ignoring case, spaces, dashes and underscores; text and boolean properties first, then the
  set's variant properties.

## Regeneration and undo

- **`list::regenerate`** brings a list's copies to what its description and data set make of its
  template, in three steps handed to the caller in order, each settled by `propagate::owed` as a
  commit would:
  1. the old copies deleted, and one capture of the template inserted after it per shown row
     past the first, the `SetList` naming them in the same step;
  2. each row (the template is row 1) switched to the variant its cells name;
  3. each row's text and boolean properties set through `variant::set_property`.

  Each copy is the template's subtree copied whole, its overrides included; the cells then
  overwrite what they fill. It does nothing (`false`) when the frame is not a list or its data
  set or template is missing.
- **`list::touched`** says which lists a committed transaction left out of date: its `SetList`s,
  the lists over a data set its `SetDataSet`s wrote, and a list whose template subtree it
  dirtied, a main's edit reaching the template through the propagation included.
- **In the app the rows are made inside the edit's undo step.** `EditorSession::commit_inner`
  reads the edit's `SetList` and `SetDataSet` ops before committing it, then
  `list::regenerate_in_history` commits each regeneration step and joins it onto the edit's
  (`History::join_last(2)`), so one undo takes the edit and its rows back together and redo puts
  them back. The copies are instances that other edits write in history (a main's change
  propagated into them, a variable detach); a regeneration outside history would leave those
  steps naming copies a later regeneration replaced. This is where
  lists part from charts, whose parts nothing else writes and whose settle runs outside history
  (`charts.md`, *Regeneration*).
- **Copies are saved**, so a load, an export or the CLI draws them as saved and regenerates
  nothing. In play a regeneration applies to the played copy directly (*Play*).
- **A list whose data set the document lacks** (one pasted from another document: the clipboard
  carries no data sets) is not regenerated; it keeps the rows it was copied with, and the card
  says so.

### Traps

- **The three steps cannot be one transaction.** Each is planned on the document the previous one
  left: the switches need the copies to exist, and the properties are read off the variant each
  row switched to.
- **Each step joins the edit's step.** Dropping the join leaves the rows as undo steps of their
  own (`tests/lists.rs · a_list_repeats_its_template_per_row_and_undoes_whole`).
- **A copy is edited only by regeneration in practice, not by a rule.** The canvas picks a copy as
  its list and the layers panel does not list it, but no guard refuses another door's edit to a
  copy (an agent's `set`); the next regeneration replaces it.

## Doors

- **Make a list from data…** — on an instance's context menu (after *Detach instance*) and the
  palette (*Needs an instance* without one), dimmed with `list::why_not`'s reason: the layer must
  be an instance's root, not inside a main, set, instance, list or chart, and not in the vault.
  It opens the file picker (CSV, TSV, JSON), reads the file on the chart import's worker
  (`chart_import`'s `Then::List`), and `list::make` wraps the instance in a new frame at its place:
  a flex column, 8 px row gap, aligned to the start, hugging its rows both ways, named *‹component
  or set› list*. A new data set named for the file (`list::free_name` numbering a taken name),
  linked to it, and the list over it with its fill by name land in one step, *Make list*; the
  rows are made at the commit. The list is selected and the status line says *Made a list from
  products.csv · 24 rows*.
- **Detach list** — the List card's foot, `Ctrl+E` over a selection of lists only
  (`flatten_selection`, after charts), a list frame's context menu (promoted from Structure) and
  the palette (*Select a list*). It drops the description and keeps the copies as plain
  instances, one step for every list, *Detach list*.
- **The readers and the worker are the chart's** (`charts.md`, *Import and the linked file*): the
  same formats, limits, encoding guesses and refusals, a read landing through `Then::List`,
  `Then::ListData` or `Then::DataSet`.

## The List card

The list frame's inspector card (`panels/list.rs`), after Item; the frame keeps its Container
card, which arranges the rows. Each edit is one `SetList`, and the list is regenerated in its
step.

- **Data** — a dropdown with the `TABLE` glyph listing the document's data sets (*name · N rows*)
  and, after a separator, *From a file…*; picking a data set moves the list onto it, undo step
  *List data*: each fill keeps its column where the new table has a column of the same name, and
  a property left unfilled takes the column of its own name (`MastikApp::list_on`). *From a
  file…* reads a new data set from a picked file and moves the list onto it in one step. Under
  it, *N rows · N columns*; while linked, the source line (*Linked to products.csv*, *Changed on
  disk* in the accent, *File not found* in the warning ink, the path its tooltip) with *Update*
  while changed, *Relink…* while missing, and *Unlink*. *Showing 200 of N rows* in the warning
  ink when `All` or `First` is cut at `MAX_DRAWN`. A data set the document lacks reads *Missing
  data* with *Its data set is not in this document: pick one*.
- **Rows** (a `ui::fold_rule` section) — `ui::segmented` *All* / *First* / *Opened row*. *First*
  starts at 3 and shows a *Rows* count field (1 to 200); *Opened row* says *Shows the row opened
  in play. Row 1 here.* Undo step *List rows*.
- **Fill · ‹component›** (a fold) — one labelled dropdown per text and boolean property, then per
  variant property: *None* and the data set's columns. Under a variant row, *N values name no ‹X›
  variant* in the warning ink, the cells its tooltip. With no such property, *Its component has
  no text, boolean or variant property*. Undo step *List fill*.
- **Detach list** at the foot (`ui::action_button`), `Ctrl+E` in its tooltip.

### The linked file

- **Update** reads the file again under the set's stored header choice and replaces the table,
  undo step *Update list data*; every list over the set follows in the step. *Unlink* drops the
  link, undo step *Unlink data file*. *Relink…* reads a picked file into a new data set and moves
  this list onto it, as *From a file…* does.
- **Watching** is the chart's (`chart_import`'s watch takes the data sets' linked paths too): the
  toolbar pill *products.csv changed · Update list* (*Update 3 lists* when several read it, shared
  `chart_import::link_pill`) for the first changed set a list reads; a click updates it. A file's
  contents enter the document only on that click.

## On the canvas and in the layers tree

- **A copy picks as its list.** `query::hit_test` answers a copy, or anything inside one, with the
  list frame (`list::copy_owner`); the template is a layer like any other, so editing it edits
  every row.
- **The layers tree lists the template and not the copies.**

## Play

- **A pointer reaction firing inside a list row opens that row** (`Player::open_row`, called by
  `fire` before the reaction's actions): every list over the same data set showing *Opened row*
  gets the row as `opened` and is regenerated on the played copy, so a navigation the reaction
  makes lands on a detail screen already showing it. Nothing is written to the document. There is
  no *Open row* action; the tap is the act
  (`a_tapped_row_is_opened_on_the_detail_screen`).

## Save format

`DocumentDto::data` (the data sets in order, skipped when none) and `NodeDto::list` (skipped when
none; `fill` and `copies` skipped when empty, `opened` at 0), in v21 (`io.md`, the ladder). The
loader refuses a data set id twice or a data set `DataSet::validate` refuses, and drops a list off
a frame or with bad copies. A duplicated list's copies follow their new ids
(`document::remap_subtree`); a copy not copied is dropped from `copies`.

## Limits

A list draws at most 200 rows, the template included (`list::MAX_DRAWN`). A data set holds the
chart table's limits: 10,000 rows, 64 columns, 256 characters a cell. An empty table deletes the copies and leaves the template showing its own values, on the canvas and
in play.
