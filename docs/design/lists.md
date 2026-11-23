# Lists from data

A list is a frame that repeats a component instance, its **template**, once per row of one of
the document's **data sets**, each copy's component properties filled from its row's cells. The
copies are ordinary instances, saved and drawn as any layer is, and made again whenever the
template, the list's description or its data set changes. Core: `mastik_core::list`; app:
`list_data.rs` and the List card, `panels/list.rs`.

## Model

### Data sets

- **`DataSet { id, name, table, source, pictures }`**, in `Document::data_sets()` in order,
  looked up by `DataSetId` (a node-id wire form, minted by `IdSource::mint_data_set`). The `table` is the
  chart's `Table` (`charts.md`, *The description*) under the chart's limits
  (`list::validate_table`), and the name is at most 256 characters (`list::MAX_NAME_CHARS`).
- **A data set is the document's, not a list's**: several lists and charts may read one, and an
  edit to it regenerates every list reading it and brings every chart reading it onto its table
  (`charts.md`, *The description*, `data`). It is never drawn, so `SetDataSet` dirties nothing.
- **A data set stays until deleted**, whatever reads it: one its last reader left (moved onto
  another, detached, flattened or deleted) or one made in *Data tables* stays, and every Data
  dropdown offers it. `Document::apply` refuses a transaction leaving a set gone while a list or
  chart still reads it (`OpError::DataSetRead`, *‹id› is read by a list or chart — point it at
  another table first*); `list::is_read` answers whether anything reads one. No sweep takes an
  unread table (*Clean up…*, *System health*): a table holds typed data, and losing it to a sweep
  costs more than a stray layer (`tests/lists.rs · a_data_set_stays_until_deleted`).
- **`source: Option<DataLink>`** is the linked file, the chart's `DataLink` (path, the stat of the
  read, the header choice). The table is always the document's own copy.
- **`pictures: Vec<(String, ImageId)>`** maps a cell's text to the picture the app read from it
  (*Picture columns*, below); core reads no files, so the map is filled where the file is read.
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
  `Target::Prop(ItemId)` names a text, boolean or image component property of the template's main;
  `Target::Variant(name)` a variant property of its set. Columns are named by `ColumnId`.
- **`copies`** — the copies the last regeneration made, in row order; written only by
  `list::regenerate`.
- **`opened`** — the row an `Opened` list shows: 0 in the document, set on the played copy.
- **The template is the frame's first child that is not a copy** (`list::template`). A copy and
  its place are found by `list::row_of`; the list, place and data row a layer sits in by
  `list::row_at`, walking up.

### Where a list may stand

A list frame is never a main, a set or an instance, and never sits inside a main, a set, an
instance, a chart, another list or the vault. `list::misplaced(nodes, vault)` lists the frames
that break this, and `Document::apply` refuses a transaction leaving one with
`OpError::ListPlacement(id)` (*A list can't be part of a component, an instance, a chart or
another list — detach it first*), over the whole document as `chart::check` holds a chart's place
(`charts.md`). The doors ask first: *Make a list* by `list::why_not` (*Doors*),
and *Create component* dims on a layer that is or holds a list (`NotMain::List`, *A list can't be
part of a component — detach it first*). The loader drops a misplaced list's description, keeping
its copies as plain instances, rather than refusing the file, since earlier builds saved such files.

**A list frame holds one instance besides its copies.** `list::bad_templates(nodes)` lists the
frames whose other children are not exactly one instance, and `Document::apply` refuses a
transaction leaving one with `OpError::ListTemplate(id)` (*A list repeats one instance — detach
the list to delete, wrap or add to its layers*): deleting the template would leave copies nothing
regenerates, and wrapping it or placing a layer ahead of it would make that layer the template.
The layers panel offers no drop into a list frame (`drop_is_legal`); the loader drops such a
list's description as it drops a misplaced one.

### What a column fills

- **Columns fill component properties, not any layer's field**: a text property takes the cell,
  trimmed; a boolean takes `list::cell_bool` (true, yes, on, 1, y shown; false, no, off, 0, n or
  empty hidden; anything else leaves the property as the template has it); an image property
  takes the picture the cell names (*Picture columns*); a variant column switches the copy to the
  variant whose value the cell names. Swap and nested properties are not filled.
- **A variant cell naming no value** of its property leaves that property where the template
  has it, as does a combination of values no variant of the set has.
  `list::unmatched_variant_cells` answers each such cell of the drawn rows (property, row,
  column, cell); the List card counts them (`list::unmatched_variants`, built from it) and the
  data window draws them in the warning ink.
- **Row 1 is the template, filled like every row**, so an edit to a filled property there would
  be lost at the next regeneration without a word. The instance card shows such a property's
  column, read-only (`components.md`, *Variants chrome*).
- **By name** (`list::fill_by_name`): a column fills the property whose name matches its own,
  ignoring case, spaces, dashes and underscores; text, boolean and image properties first, then
  the set's variant properties.

### Picture columns

- **A cell names a picture by a path or by its id.** `DataSet::picture(doc, cell)` answers the
  picture `pictures` maps the trimmed cell to, else the document's picture of that id. A document
  picture has no name of its own, so there is nothing else to name it by; a web address is not
  read.
- **A cell naming nothing leaves the template's picture.** The List card warns under that fill
  row (`list::unmatched_pictures`, *Fill*, below).
- **The app reads the files** (`list_data::read_pictures`): only a plain relative path beside the
  data file (no absolute path, no `..`), only in the rows a list can draw (`MAX_DRAWN`), each file
  up to `tokens_import::MAX_READ`, decoded by `app::read_picture`. A cell already naming a
  document picture's id is not read, and each cell is tried once per read however many rows name
  it. Each new picture's `AddImage` lands in the same step as the map. Reads happen on the import
  box's *Make list*, on *Update* and *Relink…* (which replace the map), and when the card's *Fill*
  points an image property at a column it did not already fill while the set is linked; any
  other fill edit reads nothing.
  A browser drop's file has no folder, so nothing is read for it.
- **The reads run on the UI thread** at those moments, not on the chart import's worker.

## Regeneration and undo

- **`list::regenerate`** brings a list's copies to what its description and data set make of its
  template, in three steps handed to the caller in order, each settled by `propagate::owed` as a
  commit would:
  1. the old copies deleted, and one capture of the template inserted after it per shown row
     past the first, the `SetList` naming them in the same step;
  2. each row (the template is row 1) switched to the variant its cells name;
  3. each row's text, boolean and image properties set through `variant::set_property`.

  Each copy is the template's subtree copied whole, its overrides included; the cells then
  overwrite what they fill. It does nothing (`false`) when the frame is not a list or its data
  set or template is missing.
- **`list::touched`** says which lists a committed transaction left out of date: its `SetList`s,
  the lists over a data set its `SetDataSet`s wrote, and a list whose template subtree it
  dirtied, a main's edit reaching the template through the propagation included.
- **In the app the rows are made inside the edit's undo step.** `EditorSession::commit_inner`
  reads the edit's `SetList`, `SetDataSet` and `SetChart { chart: Some }` ops before committing
  it, then `list::regenerate_in_history` commits, each joined onto the edit's step
  (`History::join_last(2)`): first the charts brought onto their sets' tables
  (`list::chart_follow_ops`), then each regeneration step. One undo takes the edit, its rows and the charts' copies back together and redo
  puts them back. The edit's run stays open across the joins (`History::current_run`,
  `History::resume_run`), so a scrub or a held nudge on a template is one step, its
  regenerations' inverses stacked inside it. The copies are instances that other edits write in history (a main's change
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

- **Make a list from data ▸** — a flyout on an instance's context menu (after *Detach instance*),
  dimmed with `list::why_not`'s reason: the layer must be an instance's root, not inside a main,
  set, instance, list or chart, and not in the vault. Its rows: one per data set
  (`Item::MakeListFrom`, in `list_data::table_words`, as every table dropdown: *products.csv · 24
  rows*, or *products.csv · not used* when no list or chart reads it), then *From a file…* and
  *New table*. The
  palette offers *Make a list from a file…* and *Make a list from a new table* (*Needs an
  instance* without one), not the per-data-set rows.
- **Every door ends in `list::make`**, which wraps the instance in a new frame at its place: a flex
  column, 8 px row gap, aligned to the start, hugging its rows both ways, named *‹component or set›
  list*. The list lands in one step, *Make list*; the rows are made at the commit. The list is
  selected and the status line says *Made a list from products.csv · 24 rows*.
  - **A data set** makes the list at once over that set, filled by name, with no box.
  - **From a file…** opens the file picker (CSV, TSV, JSON), reads the file on the chart import's
    worker (`Then::List`) and opens the import box (below).
  - **New table** makes a data set of `list::seed_table(doc, template)`: one column per text,
    boolean or image property, then per variant property, each named after it, and one row holding
    the instance's own values (`true`/`false`, a picture's id, the variant value shown), so the
    fill by name fills every property. `make_list_new_table` adds an empty row 2, so the list
    starts with two rows. It is named *Table 1* (`list::free_name`), unlinked; the list is made
    over it and the data window opens typing into row 2's first cell (`type_into_cell`), the
    next thing to write.
- **The import box** is the chart's (`chart_import.rs`; `charts.md`, *Import and the linked
  file*), headed *Make a list*, with a *Fill · ‹component›* section after the preview: one
  labelled `ui::fitted_combo` per property the List card fills (three to a row, scrolling past six),
  each starting on the column of its name (`fill_by_name`), *None* offered. The fill is held by
  column name, so the header switch's re-read keeps it where the name survives; a choice the
  re-read table lacks shows *None* and fills nothing, and returns when the header is switched
  back. *Make list*
  commits a new data set named for the file (`list::free_name` numbering a taken name), linked to
  it, and the list with that fill, one step.
- **Dropping a data file on an instance**: a single CSV, TSV or JSON file dragged over an instance
  `list::why_not` takes rings it and labels *Make a list from products.csv*
  (`draw_drop_indicator`); the drop opens the import box (`drop_chart_data` →
  `list_drop_target`). Onto a chart it is still the chart's import box, or a read into the set a
  chart reads (`charts.md`, *Import and the linked file*); elsewhere a CSV or TSV makes a chart.
- **Detach list** — the List card's foot, `Ctrl+E` over a selection of lists only
  (`flatten_selection`, after charts), a list frame's context menu (promoted from Structure) and
  the palette (*Select a list*). It drops the description and keeps the copies as plain
  instances, one step for every list, *Detach list*.
- **The readers and the worker are the chart's** (`charts.md`, *Import and the linked file*): the
  same formats, limits, encoding guesses and refusals, a read landing through `Then::List`,
  `Then::ListData`, `Then::ChartData` (the Chart card's *From a file…*) or `Then::DataSet {
  relink }`.

## The List card

The list frame's inspector card (`panels/list.rs`), after Item; the frame keeps its Container
card, which arranges the rows. Each edit is one `SetList`, and the list is regenerated in its
step.

- **Data** — `list_data::table_combo`, the one table dropdown (the Chart card's and *Data
  tables*' too): a `ui::fitted_combo` with the `TABLE` glyph, its width kept whatever the tables
  are called, listing every data set in the document (`table_words`: *name · N rows*, or *name ·
  not used* when nothing reads it) and, after a separator, *From a file…*; picking a data set
  moves the list onto it, undo step *List data*: each fill keeps its column where the new table has a column of the same name, and
  a property left unfilled takes the column of its own name (`MastikApp::list_on`). *From a
  file…* reads a new data set from a picked file and moves this list alone onto it in one step.
  Under it, *N rows · N columns* and *Edit data…* (`ui::action_button`, the data window, as on
  the Chart card); while linked, the source line (*Linked to products.csv*, *Changed on disk* in
  the accent, *File not found* in the warning ink, the path its tooltip) with *Update* while
  changed, *Relink…* while missing (tooltip *Pick the file again for ‹set›*, or with several
  readers *Pick the file again for the 2 lists, 1 chart reading ‹set›*), and *Unlink*
  (`data_set_link_rows`, which the Chart card shows for a chart reading a set, painted by
  `chart_import::link_rows`, the Chart card's own source line's painter too). The card's warnings
  are `ui::note_line`s, the `WARNING` glyph in the warning ink and the words muted: *No rows: the
  template is hidden in play* over an empty table; *Showing 200 of N rows* when `All` is cut at
  `MAX_DRAWN` (*First* asks for 200 at most, so it is never cut). A data set the document lacks
  reads *Missing data* with *Its table is not in this document: pick one*
  (`panels::list::MISSING_TABLE`, the Chart card's too).
- **Rows** (a `ui::fold_rule` section) — `ui::segmented` *All* / *First* / *Opened row*. *First*
  starts at 3 and shows a *Rows* count field (1 to 200) half the column wide, as the cards'
  number fields; *Opened row* says *Shows the row opened in play, row 1 here*
  (`ui::selection_note`). Undo step *List rows*.
- **Fill · ‹component›** (a fold) — one labelled `ui::fitted_combo` per text, boolean and image
  property, then per variant property: *None* and the data set's columns. Under a variant row,
  *N values name no ‹X› variant*, the cells its tooltip (an empty one as *an empty cell*); under
  an image row, *3 pictures not found*, the cells and *looked for next to products.csv* its
  tooltip. Both are `ui::note_line_tip`s, warning glyph and muted words as above. With no such
  property, *Its component has no text, boolean, image or variant property*
  (`list_data::NO_FILL_TARGETS`, the import box's too). Undo step *List fill*, which reads the
  image column's pictures in the same step while the set is linked.
- **Detach list** at the foot (`ui::action_button`), `Ctrl+E` in its tooltip.

### The linked file

- **Update** reads the file again under the set's stored header choice and replaces the table,
  undo step *Update table*. **Relink…** (and the data window's *Import…*) reads a picked
  file into the same data set (`Then::DataSet { relink: true }`): table and link replaced, name
  kept, undo step *Relink table*, status *Relinked products.csv for 2 lists, 1 chart · 26
  rows* (its readers in words, `readers_words`; without *for …* when nothing reads it); on a set
  never linked (*New table*) the step is *Link table* and the status *Linked …*. To point one
  list or chart at another file, the Data dropdown's *From a file…* stays.
- **Both regenerate every list over the set in the step**, each list's fills remapped onto the new
  table by column name and a fill whose column name is gone dropped, and bring every chart over
  it onto the new table; the status line adds *· 1 fill lost its column*. Both read the image columns' pictures again (*Picture columns*).
  *Unlink* drops the link, undo step *Unlink data file*.
- **Watching** is the chart's (`chart_import`'s watch takes the data sets' linked paths too): the
  toolbar pill *products.csv changed · Update list* (*Update chart* for one chart, *Update 2
  lists, 1 chart* when several read it; tooltip *… click to read it into its table*; shared
  `chart_import::link_pill`) for the first changed set a list or chart reads; a click updates it. A file's
  contents enter the document only on that click.

### The data window over a data set

The chart's data window (`chart_data.rs`; `charts.md`, *The data window*) opened from a list over
its data set, `ChartData.data` naming it. Opened by *Edit data…* on the List card, on a list
frame's context menu and after *Make a list*'s *New table*. A chart reading a set opens the
chart's own window, which writes the set (`charts.md`, *The data window*); with nothing behind it
the window is *Data tables* (below).

- **Head**: *List data*, its note the data set's name, then its readers in words (*· 2 lists, 1
  chart*, `readers_words`) when more than one list, or any chart, reads it. A double-click on the name renames the data set in place (`settings::card_head_renamable`), undo
  step *Rename table*, a taken name numbered past (`list::free_name`).
- **The grid is the chart's.** Each header shows in the accent the property its column fills on
  the list the window was opened from (tooltip *Fills Title on this list*); the chart-only header
  rows (*Use as category* / *Use as X*, *Show as series*) are not offered. A cell naming no
  variant of the property its column fills on that list is drawn in the warning ink, an empty
  one with a warning-ink edge (`Heads::Fills`, `list::unmatched_variant_cells`), so the cells
  the List card counts are found
  where they are fixed. *Import…* relinks the
  data set (*The linked file*). A linked set's grid is read-only with *Edited in products.csv —
  unlink to edit here*, as a chart's.
- **Each edit is one step, *Edit table***: a `SetDataSet`, a fill whose column is gone
  dropped, every list over the set regenerated and every chart over it brought onto the table.
  `list::regenerate` drops such a fill too, whichever door removed the column, so a column
  minted later under the freed id fills nothing.
- **It follows the selection**: selecting another list alone moves it to that list's data set; it
  closes when the data set is gone.

### Data tables

The same window with nothing behind it (`ChartData.chart` is `None`, `tables_ui`): a manager over
the document's data sets, which it calls tables. Named *Data tables*, not *Content manager*, since
*Fill with content…* is a different tool in the same menu.

- **Doors**: *Data tables…* in the Tools menu after *Scripts…* (`ToolsRow::DataTables`) and in the
  palette (`Item::DataTables`, `icon::TABLE`, aliases *tables*, *data*, *content*), both
  `Action::OpenDataTables` → `MastikApp::open_data_tables`; never dimmed, no chord. A second
  press while it is open leaves it as it is. It opens on the table it last showed this session
  (`MastikApp::tables_last`), else the first; a picked table gone (deleted, undone) moves it to
  the first.
- **Head**: *Data tables*, its note the picked table's name, renamed as above (*Rename table*),
  then *· 2 lists, 1 chart* (`readers_words`) or *· not used*. It does not follow the selection;
  its ✕ and `Escape` close it.
- **Toolbar**: the table dropdown leads it, before *Add row* (`list_data::table_combo`,
  `TABLES_PICK_W` wide, with `TableRows { new }`): each table in `table_words`, the picked one
  ticked; a separator; *New table* (`icon::PLUS`) and *From a file…* (`icon::IMPORT`). *Delete table* ends the toolbar after *Import…*: a `CONTROL_H`
  square `ui::icon_button_sized` with `icon::TRASH`, tooltip *Delete table*, dimmed while
  anything reads the table with the tooltip *Delete table: used by 2 lists and 1 chart. Point
  them at another table first.* The grid takes these through its `bar` callback (`Slot::Lead`,
  `Slot::Trail`). *Import…* relinks the table (*The linked file*); edits go through
  `commit_data_table`, *Edit table*.
- **Headers are plain** (`Heads::Plain`): no fill, no role mark, the names in full ink, and the
  header menu without *Use as category* and *Show as series*, which belong to a chart, not the
  table.
- **With no table**: the head alone, then `ui::empty_state` with the `TABLE` glyph, *This
  document has no tables*, *Make one, or read one from a CSV, TSV or JSON file*, and *New table*
  and *From a file…*, in a body as tall as the toolbar and the grid, so the first table made does
  not resize the window. Deleting the last table lands here.
- **New table** (`list_data::new_table`): *Table N* (`list::free_name` from *Table 1*), one empty
  column *Column 1* and one empty row, undo step *New table*; the window moves onto it, typing
  into its first cell (`type_into_cell`).
- **From a file…** (`table_file_dialog`): the picker, the worker's read (`Then::Table`), then the
  chart's import box in its table mode (`ChartImport.table`, `open_table_import_box`): head
  *Import data*, no Fill section, primary *Import*, *Keep this file linked* as usual. *Import*
  adds a set named for the file (`list::free_name`), linked when ticked, undo step *New table*,
  status *Imported products.csv · 24 rows*, and the window moves onto it
  (`add_table_from_import`).
- **Delete table** (`delete_table`): refused while anything reads the table; undo step *Delete
  table* (`icon::TRASH`), status *Deleted Table 2*; the window moves to the next table, else the
  previous, else the empty state. A linked table's file is never touched.

#### Traps

- **Two refusals guard *Delete table***: `delete_table`'s own `list::is_read` check and the
  commit's `OpError::DataSetRead`. `chart_data.rs · delete_table_only_takes_an_unread_table`
  stays green with either removed alone; the commit's refusal is the one every door meets.

## On the canvas and in the layers tree

- **A copy picks as its list.** `query::hit_test` answers a copy, or anything inside one, with the
  list frame (`list::copy_owner`); the template is a layer like any other, so editing it edits
  every row, save a filled property, which row 1's cells set (*What a column fills*). A double-click on a copy selects the template and enters the list frame
  (`copy_template_at`, asked first in `double_click_pick`).
- **The list's tag is a component set's tab** (`canvas_tags`, `frame_label`): card ground,
  rounded top corners, without the set's resting outline, the `TABLE` glyph, the frame's name and
  the rows it draws from `list_data::list_count` (*· 24 rows*, *200 of 1240 rows*, *Opened row*,
  or *No rows* and *Missing data* in the warning ink). It shows while the list is selected, and
  always while its table is empty, wherever the frame sits on the page.
- **The layers tree lists the template and not the copies.** A list frame's row shows
  `list_count`'s words as its badge, in place of the frame size.

## Play

- **A pointer reaction firing inside a list row opens that row** (`Player::open_row`, called by
  `fire` before the reaction's actions): every list over the same data set showing *Opened row*
  gets the row as `opened` and is regenerated on the played copy, so a navigation the reaction
  makes lands on a detail screen already showing it. Nothing is written to the document. There is
  no *Open row* action; the tap is the act
  (`a_tapped_row_is_opened_on_the_detail_screen`).
- **The player's state card starts with *Opened rows*** (`state_rows`): one row per data set an
  *Opened row* list reads (`Player::opened_rows` → `mastik_play::OpenedRow`), its name and a
  dropdown of its rows as *N · first cell* (up to 200 listed), the open one ticked. A pick is
  `Input::OpenRow { data, row }`, which a take records, and opens the row as a tap does
  (`Player::open_data_row`, which `open_row` calls).
- **A list over an empty table draws no row in play.** `Player::hide_empty_templates`, run from
  `prepare`, hides its template on the played copy and shows it again as the document has it
  once a live edit gives the table rows. The canvas keeps the template, the one row to edit. The
  exported page plays through `mastik-play` and does the same. A list whose data set is missing
  keeps its rows.

## Save format

`DocumentDto::data` (the data sets in order, skipped when none; a set's `pictures` skipped when
empty) and `NodeDto::list` (skipped when none; `fill` and `copies` skipped when empty, `opened` at
0), in v21 (`io.md`, the ladder). `DataSet.pictures` and `PropKind::Image` are additive in v21,
with no version bump. The loader refuses a data set id twice or a data set `DataSet::validate`
refuses, and drops a list off a frame, with bad copies or standing where a list may not
(*Where a list may stand*). A duplicated list's copies follow their new ids
(`document::remap_subtree`); a copy not copied is dropped from `copies`.

## Limits

A list draws at most 200 rows, the template included (`list::MAX_DRAWN`). A data set holds the
chart table's limits: 10,000 rows, 64 columns, 256 characters a cell. An empty table deletes the
copies and leaves the template showing its own values on the canvas; play hides it (*Play*).
