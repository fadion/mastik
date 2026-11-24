# Lists from data

A list is a frame that repeats a component instance, its **template**, once per row of one of
the document's **data sets**, each copy's component properties filled from its row's cells. The
copies are ordinary instances, saved and drawn as any layer is, and made again whenever the
template, the list's description, its data set or a variable its filter reads changes. Core: `mastik_core::list`; app:
`list_data.rs` and the List card, `panels/list.rs`.

## Model

### Data sets

- **`DataSet { id, name, table, source, pictures }`**, in `Document::data_sets()` in order,
  looked up by `DataSetId` (a node-id wire form, minted by `IdSource::mint_data_set`). The `table` is the
  chart's `Table` (`charts.md`, *The description*) under the chart's limits
  (`list::validate_table`), and the name is at most 256 characters (`list::MAX_NAME_CHARS`). No door
  names one `row`, the word an expression reads a row action's row by: `list::free_name` numbers
  past it, and the loader renames a table called `row`. `SetDataSet` itself does not refuse it.
- **A data set is the document's, not a list's**: several lists and charts may read one, and an
  edit to it regenerates every list reading it and brings every chart reading it onto its table
  (`charts.md`, *The description*, `data`). It is never drawn, so `SetDataSet` dirties nothing.
- **A data set stays until deleted**, whatever reads it: one its last reader left (moved onto
  another, detached, flattened or deleted) or one made in *Data tables* stays, and every Data
  dropdown offers it. `Document::apply` refuses a transaction leaving a set gone while a list or
  chart still reads it (`OpError::DataSetRead`, *‹id› is read by a list or chart — point it at
  another table first*); `list::is_read` answers whether anything reads one. An export row's
  *One file per row* reads one without holding it: deleting the table leaves that row writing
  one file (`export.md`, *Plan*). No sweep takes an
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
  named as its own copy (`ListRule::BadCopies`), or past the filter and sort limits
  (`ListRule::BadFilter`). It keeps nothing of the description it replaces.
- **`data`** — the data set it reads.
- **`rows`** — `Rows::All` (every row, up to `MAX_DRAWN`), `Rows::First(n)`, `Rows::Row(n)`: one
  fixed row counted from 1, held by position and the same in play, the last row when `n` is past
  the table's end; or `Rows::Opened`: the one row opened in play (*Play*, below), row 1 in the
  document. `Rows::is_one_row` is true for `Row` and `Opened`.
- **`filter: Filter { any, tests }`** — `Test { column, op, value }`s joined by all (`any`
  false) or any. `sort: Vec<SortKey { column, descending }>` — a column and at most one *Then by*.
  Both are described under *Filter and sort*.
- **`fill: Vec<Fill { target, column }>`** — which column fills which property.
  `Target::Prop(ItemId)` names a text, boolean or image component property of the template's main;
  `Target::Variant(name)` a variant property of its set; renaming or deleting the property
  renames or drops the fills naming it in the same step (`list::variant_prop_ops`). Columns are
  named by `ColumnId`.
- **`copies`** — the copies the last regeneration made, in the order their rows are drawn; written only by
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
  empty hidden; anything else gives the row the main's value); an image property
  takes the picture the cell names (*Picture columns*); a variant column switches the copy to the
  variant whose value the cell names. Swap and nested properties are not filled.
- **A cell naming no value takes the component's own**: a variant cell naming no value of its
  property gives the row the default variant's value of it, and an unread boolean or picture
  cell resets the property to the main's. Never the template's: the first row drawn fills the
  template, so what it shows is the last fill, and the result would depend on history. A combination of
  values no variant of the set has leaves the row's variant as it was captured.
  `list::unmatched_variant_cells` answers each such cell of the drawn rows (property, row,
  column, cell); the List card counts them (`list::unmatched_variants`, built from it) and the
  data window draws them in the warning ink.
- **The first row drawn is the template, filled like every row**, so an edit to a filled property there would
  be lost at the next regeneration without a word. The instance card shows such a property's
  column, read-only (`components.md`, *Variants chrome*).
- **By name** (`list::fill_by_name`): a column fills the property whose name matches its own,
  ignoring case, spaces, dashes and underscores; text, boolean and image properties first, then
  the set's variant properties.

### Picture columns

- **A cell names a picture by a path or by its id.** `DataSet::picture(doc, cell)` answers the
  picture `pictures` maps the trimmed cell to, else the document's picture of that id. A document
  picture has no name of its own, so there is nothing else to name it by; a web address is not
  read. `DataSet::image_columns(doc)` is the columns with a cell naming a picture, which *Fill
  with content* offers a shape (`app-canvas-chrome.md`).
- **A cell naming nothing gives the row the main's picture.** The List card warns under that fill
  row (`list::unmatched_pictures`, *Fill*, below).
- **The app reads the files** (`list_data::read_pictures`): only a plain relative path beside the
  data file (no absolute path, no `..`; `\` reads as `/`), and only when the joined path passes
  `tokens_import::local_path`, since the folder comes from the document's own link and a stat
  under `\\host\share` sends the user's credentials there. Only in the rows a list can draw
  (`MAX_DRAWN`), each file
  up to `tokens_import::MAX_READ`, decoded by `app::read_picture`. A cell already naming a
  document picture's id is not read, and each cell is tried once per read however many rows name
  it. Each new picture's `AddImage` lands in the same step as the map. Reads happen whenever a
  list comes to read a linked table (`list_data::read_set_pictures`): the import box's *Make
  list*, *Make a list from data ▸* a table, the card's table dropdown and *From a file…*; on
  *Update* and *Relink…* (which replace the map); and when the card's *Fill* points an image
  property at a column it did not already fill while the set is linked. Any other fill edit
  reads nothing. *Fill with content* reads every linked table's first `MAX_DRAWN` rows when it
  opens, and any further row its plan takes; a per-row export reads every row, and its preview
  the row shown.
  A browser drop's file has no folder, so nothing is read for it.
- **The reads run on the UI thread** at those moments, not on the chart import's worker.

### Filter and sort

- **Order: the filter, then the sort, then the rows mode**, so *First 3* is the top three after
  the sort. `List::shown_rows(table, read)` answers the drawn table rows in order, `List::passing`
  the rows passing the filter, and `list::shown(doc, id)` a list's drawn rows with its variables
  read in the mode in force at the list's frame (`doc.resolve_var(list frame, var)`). A one-row
  mode (`Row`, `Opened`) shows a row of the table as it stands, so filter and sort do not apply
  to it. The table itself is never reordered.
- **A test** is a column, a `TestOp` and a value: *is*, *is not*, *contains*, *starts with*,
  *less than*, *at most*, *more than*, *at least*, *is empty*, *is not empty*. Text tests ignore
  case. The four number tests are offered on a number column only (`Table::is_numeric`) and fail a
  cell that is no number. An empty value passes every row, so a search not yet typed into shows
  everything.
- **A value naming no `{…}` is literal text**, so `2024-01` and `12` mean what they say; one naming
  any is an expression in the stored id form (`list::test_value`), and one that does not evaluate
  reads as empty, which passes.
- **A test whose column the table lacks passes every row**, and a sort key on one is skipped; the
  card says so under the test. The regeneration that follows the column's removal drops both, as
  it drops fills (`List::on_columns_of`), since a column added later can be minted under the
  freed id and would otherwise be read in its place.
- **Sort** compares by `list::sort_order` over `list::cell_order`: a number column by value, text
  ignoring case, empty cells last whichever way it runs; stable, so equal rows keep table order.
  The data window's *Sort ascending* / *descending* sorts by the same function
  (`chart_data::sorted`).
- **Limits**: `MAX_TESTS` (8) tests and `MAX_SORT` (2) sort keys; `List::validate` refuses more
  (`ListRule::BadFilter`).

## Regeneration and undo

- **`list::regenerate`** brings a list's copies to what its description and data set make of its
  template, in three steps handed to the caller in order, each settled by `propagate::owed` as a
  commit would:
  1. the old copies deleted, and one capture of the template inserted after it per shown row
     past the first, the `SetList` naming them in the same step;
  2. each row (the template is row 1) switched to the variant its cells name;
  3. each row's text, boolean and image properties set through `variant::set_property`.

  Each copy is the template's subtree copied whole, its overrides included; the cells then
  overwrite what they fill. Steps 2 and 3 are `variant_ops` and `property_ops`, which
  `list::fill_row` also runs on an export's throwaway copy, so an instance exported *One file per
  row* fills as a list's row does (`export.md`, *Plan*). It does nothing (`false`) when the frame is not a list or its data
  set or template is missing.
- **`list::touched`** says which lists a committed transaction left out of date: its `SetList`s,
  the lists over a data set its `SetDataSet`s wrote, a list whose template subtree it
  dirtied, a main's edit reaching the template through the propagation included, and, when it
  carries a `SetVariable`, `SetVariableCell`, `RemoveVariable`, `RemoveMode`, `SetModes`,
  `SetQueries` or `SetBreakpoints`, every list not in a one-row mode whose filter reads a variable.
  Also a list filling a variant property whose template's set it dirtied, itself or a variant
  (a variant or value added: a row naming it switches), and a list with such a filter that a
  `Reparent` moved, itself or an ancestor, since the mode in force at its frame may differ.
  A filter checked again when a value it reads changes is a redraw of the list, not a constraint
  (`non-goals.md`, *Constraints or reactivity*).
- **In the app the rows are made inside the edit's undo step.** `EditorSession::commit_inner`
  takes the edit's ops `list::follow_ops` keeps (those `touched` reads, and `SetChart { chart:
  Some }`) before committing it, then `list::regenerate_in_history` commits, each joined onto the edit's step
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
- **`follow_ops` keeps the variable, mode and `Reparent` ops.** `touched` sees only what it is handed, so
  dropping them leaves a filtered list on its old rows after a variable edit
  (`tests/lists.rs · a_filter_follows_the_variable_it_reads`).
- **A copy is edited only by regeneration.** The canvas picks a copy as its list and the layers
  panel does not list it. A verb naming a layer of a copy as its `node`, `nodes` or `parent` is
  refused, pointing at the template (`verbs::refuse_list_copies`), so an agent's or a script's
  edit is not landed and then lost; a verb that only reads its layer (`add_style`,
  `duplicate_as_component`, `push_to_main`) may name one. The snapshot marks each copy's layers
  with `list_copy` (`export.md`, *Snapshot*). `Document::apply` holds no such rule: the
  regeneration itself, and a main's edit propagated into the copies, write them.

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
  moves the list onto it, undo step *List data*: each fill, filter test and sort key keeps its
  column where the new table has a column of the same name and is dropped where it has none, and
  a property left unfilled takes the column of its own name (`verbs::moved_onto`, which
  `set_list`'s `table` moves a list through too). *From a
  file…* reads a new data set from a picked file and moves this list alone onto it in one step.
  Under it, *N rows · N columns* and *Edit data…* (`ui::action_button`, the data window, as on
  the Chart card); while linked, the source line (*Linked to products.csv*, *Changed on disk* in
  the accent, *File not found* in the warning ink, the path its tooltip) with *Update* while
  changed, *Relink…* while missing (tooltip *Pick the file again for ‹set›*, or with several
  readers *Pick the file again for the 2 lists, 1 chart reading ‹set›*), and *Unlink*
  (`data_set_link_rows`, which the Chart card shows for a chart reading a set, painted by
  `chart_import::link_rows`, the Chart card's own source line's painter too). The card's warnings
  are `ui::note_line`s, the `WARNING` glyph in the warning ink and the words muted: *No rows: the
  template is hidden in play* over an empty table; *No row passes the filter: the template is
  hidden in play* when the table has rows but none is drawn; *Showing 200 of N rows*, N the
  passing rows, when `All` is cut at `MAX_DRAWN` (*First* asks for 200 at most, so it is never
  cut). A data set the document lacks
  reads *Missing data* with *Its table is not in this document: pick one*
  (`panels::list::MISSING_TABLE`, the Chart card's too).
- **Rows** (a `ui::fold_rule` section) — `ui::segmented` *All* / *First* / *Row* / *Opened row*
  (*Row*'s tooltip *One row of the table, the same in play*). *First* starts at 3 and shows a
  *Rows* count field (1 to 200) half the column wide, as the cards' number fields. *Row* starts
  at 1 and shows a full-width `ui::fitted_combo` of the table's rows as *N · first cell*, its face
  *Row 3 · Linen shirt*, up to `list_data::ROWS_LISTED` (200) listed and then a menu note
  *Showing 200 of N rows: pick in the data window*; the words are `list_data::row_words`, which
  the player's *Opened rows* dropdown uses too. A click on a row's gutter number in the data
  window opened from a *Row* list sets its row. The row is held by position, so a table edit in
  the data window that deletes rows and leaves another record on it is said under the dropdown
  (`ui::note_line`, the `INFO` glyph in `text::dim`): *Row 3 was Linen shirt before rows were
  deleted above it*, until the card shows another layer or a row is picked. *Opened row* says *Shows the row opened in play, row 1 here*
  (`ui::selection_note`). Undo step *List rows*.
- **Filter** (`MastikApp::card_fold_noted`, its head carrying the test count) — *All* / *Any*
  (`ui::segmented`, half width, shown with two tests or more); each test a column combo and a
  test-word combo side by side, half width each, then the value field, the Prototype card's
  expression field with its `@` list (`expr_field` with `ExprWant::LiteralOr`, hint *A value, or
  @ for a variable*; none for the two emptiness tests), and an `icon::X` `ui::field_button`
  *Remove this test*. A test reading a gone column carries *This test reads a column the table no
  longer has, so it passes every row*. *Add test* (`ui::action_button`, `icon::PLUS`) adds *is*
  on the first column, offered under `MAX_TESTS`; then *6 of 24 rows pass here*
  (`ui::selection_note`). Undo step *List filter*.
- **Sort** (its head noting the first sort column's name, `ui::fold_rule_noted`) — `ui::labelled`
  *Sort by* and *Then by* rows, each a column combo with *None*, which removes that key and any
  after it, and a two-cell `ui::segmented` direction, `icon::SORT_ASCENDING` /
  `SORT_DESCENDING` (*Low to high, A to Z* / *High to low, Z to A*). *Then by* shows once *Sort
  by* is set. Undo step *List sort*.
- **Under a one-row mode** Filter and Sort keep what they hold, say *One row shows: filter and
  sort apply to All and First* (`ui::selection_note`) and are disabled (`ui::disable_unless`).
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
chart's own window, which writes and renames the set, and a chart's own table becomes a set by
*Save as table* (`charts.md`, *The data window*); with nothing behind it
the window is *Data tables* (below).

- **Head**: *List data*, its note the data set's name, then its readers in words (*· 2 lists, 1
  chart*, `readers_words`) when more than one list, or any chart, reads it. A double-click on the
  name, or the toolbar's *Rename table* button after *Import…*, renames the data set in place
  (`settings::card_head_renamable`), undo step *Rename table*, a taken name numbered past
  (`list::free_name`). The field sits on the note's line, so the name does not move as it opens.
- **The grid is the chart's.** Each header shows in the accent the property its column fills on
  the list the window was opened from (tooltip *Fills Title on this list*); the chart-only header
  rows (*Use as category* / *Use as X*, *Show as series*) are not offered. A cell naming no
  variant of the property its column fills on that list is drawn in the warning ink, an empty
  one with a warning-ink edge (`Heads::Fills`, `list::unmatched_variant_cells`), so the cells
  the List card counts are found
  where they are fixed. *Import…* relinks the
  data set (*The linked file*). A linked set's grid is read-only with *Edited in products.csv —
  unlink to edit here*, as a chart's.
- **Each edit is one step, *Edit table***: a `SetDataSet`, a fill, filter test or sort key whose
  column is gone dropped, every list over the set regenerated and every chart over it brought
  onto the table. `list::regenerate` drops them too, whichever door removed the column, so a
  column minted later under the freed id is read by none of them. `edit_table` mints an added
  column past the ids the table had before the call, since its deletes and adds land in one
  step.
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
  ticked; a separator; *New table* (`icon::PLUS`) and *From a file…* (`icon::IMPORT`). After
  *Import…* come two `CONTROL_H` squares on the field ground (`ui::field_button`), destructive
  last. *Rename table* (`icon::PENCIL_SIMPLE`, `chart_data::rename_button`, shared by every
  window over a data set) opens the head's name field and is lit (`FieldButton::On`) while it is
  open; a click then keeps the name, as `Enter` does. *Delete table* (`icon::TRASH`, tooltip
  *Delete table*) is disabled while anything reads the table, with the tooltip *Delete table:
  used by 2 lists and 1 chart. Point them at another table first.* The grid takes these through its `bar` callback (`Slot::Lead`,
  `Slot::Trail`). *Import…* relinks the table (*The linked file*); edits go through
  `commit_data_table`, *Edit table*.
- **Headers are plain** (`Heads::Plain`): no fill, no role mark, the names in full ink, and the
  header menu without *Use as category* and *Show as series*, which belong to a chart, not the
  table.
- **With no table**: the head alone, then `ui::empty_state` with the `TABLE` glyph, *This
  document has no tables*, *Make one, or read one from a CSV, TSV or JSON file*, and *New table*
  and *From a file…* as word buttons (`settings::text_button`, as every empty state's), in a
  body as tall as the toolbar and the grid, so the first table made does
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

## Verbs

The tables and lists for agents, Command Mode and scripts: registry verbs (`verbs/tables.rs`),
MCP's Phase 3 (`mcp.md`), each acting as its door in the app.

- **Naming.** A table is named by id or name (`verbs::find_table`); a name matching none or
  several is refused listing the tables. Rows count from 1. Cells are text in and out, a number
  or true/false sent in kept as its text, and the table models are read as written
  (`verbs::as_written`), since cells and column names are the person's words.
- **`add_table(name?, columns?, rows?)`** is *New table*: *Table N* unnamed, a taken name
  numbered past and said; with no columns, *Column 1* and one empty row. Answers its id.
- **`edit_table(table, set?, add?, at?, delete?, columns?)`** is one *Edit table*: `columns`
  adds, renames and deletes columns by name, `set` writes cells (`{row, column, value}`, the
  column by name or id), `add` inserts rows keyed by column name at `at` or the end, `delete`
  takes row numbers. Rows are numbered as the table stood before the call; `set` and `add` name
  columns as they stand after `columns`. Column deletes go before renames, and the renames are
  named together, so a rename may take a name deleted or renamed away in the same call. Said
  *People: 3 rows*.
- **`set_table(table, columns, rows)`** replaces the cells; a column whose name survives keeps its
  id (`Table::take_ids_by_name`), so the fills reading it hold. Names are trimmed, except one the
  table already has exactly, so a table sent back as read keeps a name with edge spaces.
- **`rename_table`**, **`delete_table`** (refused while read: *Delete table: used by 1 list and 1
  chart. Point them at another table first.*) and **`unlink_table`** are the window's and the
  card's acts.
- **A linked table refuses a cell edit** (*People is linked to people.csv: unlink it first*): the
  next *Update* would read the file over it. Limits and rows are refused in words (*At most 64
  columns*, *People has 2 rows, not 14*, *rows count from 1*).
- **Every table edit is one `SetDataSet`**, so its commit regenerates the lists over the table and
  brings the charts over it along, as the data window's edits do.
- **`make_list(node, table, fill?)`** is *Make a list from data* over a table there is: filled by
  name unless `fill` maps property names to column names. Answers the frame.
- **`set_list(node, table?, rows?, fill?, filter?, sort?)`** is the List card: `node` the list
  frame or its template, `rows` `"all"`, `{first}`, `{row}` or `"opened"`, columns named in the
  table the list reads after the call, and a `{Variable}` in a filter value stored in the id form
  (`Expr::from_names`). `table` moves the list as the Data dropdown does.
- **`set_chart_data`** writes a chart's set or gives the chart its own table (`charts.md`,
  *Elsewhere*).
- **Scripts reach the tables through `tables`** (`mastik-vm` `run/tables.rs`; Luau's own `table`
  library keeps its name): `tables.People` or `tables["Pending orders"]` is a handle with
  `rows` (and `#t`), `name`, `t[3].Name` (a row as a table of its cells), `t:add{…}`,
  `t:set(3, {…})`, `t:remove(3)` and `t:column("Price")`; `tables.new(name, columns)` is
  `add_table`. A run reads a table once and works on its own copy, so a read after a write sees
  it; the writes land at the run's end as one `set_table` per table written, so a loop of writes
  is one edit, the run one undo step, and a run that fails part-way writes no table. A table
  verb the run calls (`edit_table`, `set_chart_data`…) is read afresh after it; one called on a
  table the run has already written through `tables` is refused, since the run's end would write
  over it. A live read writes nothing, and a linked table refuses the first write.
- **Command Mode completes table names** after `tables.`, in the bar and the script editor, and
  for a verb's `table` argument, each with its size (*12 rows · 4 columns*).

## On the canvas and in the layers tree

- **A copy picks as its list.** `query::hit_test` answers a copy, or anything inside one, with the
  list frame (`list::copy_owner`); the template is a layer like any other, so editing it edits
  every row, save a filled property, which the first drawn row's cells set (*What a column fills*). A double-click on a copy selects the template and enters the list frame
  (`copy_template_at`, asked first in `double_click_pick`).
- **The list's tag is a component set's tab** (`canvas_tags`, `frame_label`): card ground,
  rounded top corners, without the set's resting outline, the `TABLE` glyph, the frame's name and
  the rows it draws from `list_data::list_count` (*· 24 rows*, *200 of 1240 rows*, *6 of 24 rows*
  under a filter, *row 3*, *Opened row*, or *No rows*, *0 of 24 rows* when no row passes, and
  *Missing data* in the warning ink). It shows while the list is selected, and
  always while its table is empty, wherever the frame sits on the page.
- **The layers tree lists the template and not the copies.** A list frame's row shows
  `list_count`'s words as its badge, in place of the frame size.

## Play

- **A pointer reaction firing inside a list row opens that row** (`Player::open_row`, called by
  `fire` before the reaction's actions): every list over the same data set showing *Opened row*
  gets the row as `opened` and is regenerated on the played copy, so a navigation the reaction
  makes lands on a detail screen already showing it. Nothing is written to the document. The
  player keeps the row it opened in each data set (`Player::opened`) and opens it again after
  every live edit (`reopen_rows`), since the editor's regeneration and a re-clone both put the
  list back on row 1. There is
  no *Open row* action; the tap is the act
  (`a_tapped_row_is_opened_on_the_detail_screen`).
- **The player's state card starts with *Opened rows*** (`state_rows`): one row per data set an
  *Opened row* list reads (`Player::opened_rows` → `mastik_play::OpenedRow`), its name and a
  dropdown of its rows as *N · first cell* (up to 200 listed), the open one ticked. A pick is
  `Input::OpenRow { data, row }`, which a take records, and opens the row as a tap does
  (`Player::open_data_row`, which `open_row` calls).
- **A filter follows its variables in play.** When a variable's value changes
  (`Player::changed`), `refilter` regenerates on the played copy each list not in a one-row mode
  whose filter reads it (`Player::regenerate_list`, the step `open_data_row` shares), then hides
  or shows templates as below: a search field or a chip narrows the list. A mode set on the copy
  (`put_modes`: *Set mode*, the state card, a flow's start) regenerates every list whose filter
  reads any variable. An *Opened row* list keeps its row.
- **Row actions edit the played copy's tables** (*Add row*, *Edit row*, *Delete row*;
  `play.md`, *Actions*), as a *Run script* can: every list and chart over the table follows on
  the copy (`Player::follow_tables`), and nothing reaches the document, the save or undo. A linked
  table is edited in play too; its refusal of edits is the document's. Expressions read a table's
  row count, cells and column totals (`play.md`, *Expressions*). The state card lists each table
  play wrote, by a row action or a script, with *Show* and *Reset* (`prototyping-player.md`).
- **A list drawing no row draws nothing in play**: an empty table, or no row passing its filter.
  `Player::hide_empty_templates`, run from `prepare`, after `refilter` and after every table
  write (`follow_tables`), hides its template on
  the played copy and shows it again as the document has it once a row is drawn, so a list a
  row action empties or fills follows. The canvas
  keeps the template, the one row to edit. The exported page plays through `mastik-play` and
  does the same. A list whose data set is missing keeps its rows.

## Save format

`DocumentDto::data` (the data sets in order, skipped when none; a set's `pictures` skipped when
empty) and `NodeDto::list` (skipped when none; `fill` and `copies` skipped when empty, `opened` at
0, `filter` and `sort` when empty, a filter's `any` when false, a test's `value` when empty, a
sort key's `descending` when false), in v21 (`io.md`, the ladder). `DataSet.pictures`,
`PropKind::Image`, `Rows::Row`, `filter` and `sort` are additive in v21, with no version bump.
The loader refuses a data set id twice or a data set `DataSet::validate` refuses, and drops a
list off a frame, with bad copies, past the filter or sort limits, or standing where a list may
not (*Where a list may stand*). A duplicated list's copies follow their new ids
(`document::remap_subtree`); a copy not copied is dropped from `copies`.

## Limits

A list draws at most 200 rows, the template included (`list::MAX_DRAWN`). A data set holds the
chart table's limits: 10,000 rows, 64 columns, 256 characters a cell. A filter holds at most 8
tests and a sort 2 columns (*Filter and sort*). An empty table, or a filter no row passes,
deletes the copies and leaves the template showing its own values on the canvas; play hides it
(*Play*).
