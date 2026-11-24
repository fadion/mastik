//! *Fill with content…* (`docs/design/app-canvas-chrome.md`, *Fill with content*):
//! the box over the canvas that fills the selection's text layers with realistic
//! values, and its avatar shapes with initials or a pattern.
//!
//! The rule is `mastik_core::content_fill`'s; this module draws Clean up's box with
//! a row each for the locale, the content, records, emails and avatars, a preview of
//! every layer grouped by record whose kind can be changed and value redrawn, and
//! commits the fill as one undo step.

use crate::app::MastikApp;
use crate::batch_rename::row;
use crate::clean_up_box::{SWITCH_GAP, layer_name};
use crate::theme::{self, color, icon};
use crate::ui::{self, FieldButton, MenuRow};
use eframe::egui;
use mastik_core::content_fill::{
    self as fill, AvatarStyle, Group, Kind, Options, Plan, TableContent, Value,
};
use mastik_core::{Document, NodeId, NodeKind};

/// The box's width, batch rename's and Clean up's.
const W: f32 = 520.0;
const GAP: f32 = 10.0;
const PAD: f32 = crate::settings::CANVAS_CARD_PAD;
/// A preview row: taller than a menu row, since each holds a [`KIND_H`] combo
/// and stacked combos with no air between them read as one column of fields.
const ROW_H: f32 = KIND_H + 8.0;
/// The preview scrolls past this many rows.
const ROWS_SHOWN: f32 = 12.0;
/// A preview row's layer path, before its kind.
const NAME_W: f32 = 168.0;
/// A preview row's kind combo, at the toolbar scale.
const KIND_W: f32 = 128.0;
const KIND_H: f32 = ui::TOOLBAR_H;
/// The open kind list: wide enough for a label and its sample, and never wider
/// or taller once open, since an egui popup cannot grow back.
const KIND_LIST_W: f32 = 280.0;
const KIND_LIST_H: f32 = 340.0;
/// The die a hovered row shows at its value's end, and its glyph.
const DIE: f32 = 20.0;
const DIE_PT: f32 = 14.0;
/// An avatar's swatch in a row's value.
const SWATCH: f32 = 16.0;

/// What the preview says, and why *Fill* is dim, while the selection holds
/// nothing to fill.
pub(crate) const NO_TEXT: &str = "Select text layers, or frames that hold them";
/// The commit's reason when every row is left as it is.
const ALL_KEPT: &str = "Every layer is left as is";

/// The open box.
pub(crate) struct FillBox {
    pub(crate) opts: Options,
    /// The locale the box opened in, the last used: what a selection whose text
    /// agrees on no Language fills in until one is picked.
    opened_locale: String,
    /// Whether a locale was picked in the Locale row, which latches it; until
    /// then each new selection is filled in the locale its text agrees on.
    locale_picked: bool,
    /// The style an avatar shape is drawn in until one is picked on its row:
    /// the last used, kept while the Avatars switch is off.
    style: AvatarStyle,
    /// What the fill reads: the live selection, or [`Self::pin`]'s roots while
    /// the selection a row's click made stands.
    selection: Vec<NodeId>,
    /// The roots looked under before a row's click selected its layer, and that
    /// selection: kept until the selection changes again, as Clean up keeps them.
    pin: Option<(Vec<NodeId>, Vec<NodeId>)>,
    /// The fill for the choices shown.
    pub(crate) plan: Plan,
    /// The fill with records and avatars on, for the counts their rows show
    /// while off: `plan` itself while both are on, else its outline, undrawn.
    every: Plan,
    /// What `plan` was made for, since the canvas stays live under the box.
    asked: Option<(Options, NodeId, u64, Vec<NodeId>)>,
    /// Each kind's sample in the locale shown, and that locale.
    samples: (String, Vec<(Kind, String)>),
    /// What the hovered row names, and whether softly (a record's frame).
    hover: (Vec<NodeId>, bool),
    /// A clicked row's layer, brought into view once drawn.
    reveal: Option<NodeId>,
    /// The `AddImage`s of the pictures read beside linked tables when the box
    /// opened ([`Options::pictures`]); the fill adds those it uses.
    read: Vec<mastik_core::Operation>,
}

/// One line of the preview.
enum Line {
    /// A record's frame, named.
    Head(usize),
    Row(usize),
}

/// A new draw for each opening and each *Shuffle*: different values, the same
/// person for the same layer only within one draw.
fn fresh_seed() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_nanos() as u64)
}

impl FillBox {
    /// What the fill adds before its own edit for the pictures read beside a
    /// linked table that it uses: each picture, and its table's (cell, picture)
    /// pairs, so the table maps them as a list over it would.
    fn beside_ops(&self, doc: &Document) -> Vec<mastik_core::Operation> {
        use mastik_core::Operation;
        let used: Vec<&mastik_core::ImageId> = self
            .plan
            .rows
            .iter()
            .filter_map(|r| match &r.value {
                fill::Value::Picture(id) => Some(id),
                _ => None,
            })
            .collect();
        // Read when the box opened: a picture may have entered the document
        // since, by a list over the same table or a redo, and adding it again
        // would refuse the whole fill.
        let mut ops: Vec<Operation> = self
            .read
            .iter()
            .filter(|op| {
                matches!(op, Operation::AddImage { id, .. }
                    if used.contains(&id) && !doc.has_image(id))
            })
            .cloned()
            .collect();
        for set in doc.data_sets() {
            let new: Vec<_> = self
                .opts
                .pictures
                .iter()
                .filter(|(s, cell, id)| {
                    *s == set.id
                        && used.contains(&id)
                        && !set.pictures.iter().any(|(c, _)| c == cell)
                })
                .map(|(_, cell, id)| (cell.clone(), id.clone()))
                .collect();
            if new.is_empty() {
                continue;
            }
            let mut next = set.clone();
            next.pictures.extend(new);
            ops.push(Operation::SetDataSet {
                id: set.id,
                set: Some(Box::new(next)),
            });
        }
        ops
    }

    /// The layers the canvas rings for the hovered row, and whether softly.
    pub(crate) fn ringed(&self) -> (&[NodeId], bool) {
        (&self.hover.0, self.hover.1)
    }

    /// Plan again when a choice, the page, the document or the selection has
    /// changed since the last plan, as Clean up does.
    pub(crate) fn replan(
        &mut self,
        doc: &Document,
        res: &mastik_core::Resolved,
        page: NodeId,
        rev: u64,
    ) {
        let moved = self.asked.as_ref().is_none_or(|a| a.3 != self.selection);
        if moved && !self.locale_picked {
            self.opts.locale = fill::agreed_locale(doc, &self.selection)
                .map_or_else(|| self.opened_locale.clone(), |l| l.tag.clone());
        }
        let now = (self.opts.clone(), page, rev, self.selection.clone());
        if self.asked.as_ref() == Some(&now) {
            return;
        }
        self.plan = fill::plan(doc, res, &self.selection, &self.opts);
        if self.read_taken_rows(doc) {
            self.plan = fill::plan(doc, res, &self.selection, &self.opts);
        }
        self.every = match self.opts.records && self.opts.avatars.is_some() {
            true => self.plan.clone(),
            false => fill::outline(
                doc,
                &self.selection,
                &Options {
                    records: true,
                    avatars: Some(self.opts.avatars.unwrap_or_default()),
                    ..self.opts.clone()
                },
            ),
        };
        if self.samples.0 != self.opts.locale {
            let locale = self.opts.locale.clone();
            let samples = Kind::ALL
                .into_iter()
                .map(|k| (k, fill::sample(k, &locale)))
                .collect();
            self.samples = (locale, samples);
        }
        // Keyed on the options as read, so pictures read for this plan do not
        // plan it again.
        self.asked = Some((self.opts.clone(), now.1, now.2, now.3));
    }

    /// Read the pictures beside a linked table for the rows `plan` takes past
    /// the first [`mastik_core::list::MAX_DRAWN`] the box read on open (*From row* further
    /// on, or *Shuffled*), adding them to [`Options::pictures`]; whether any
    /// were found.
    fn read_taken_rows(&mut self, doc: &Document) -> bool {
        let mut taken: Vec<(mastik_core::list::DataSetId, usize)> = Vec::new();
        for r in &self.plan.rows {
            if let (Some((d, _)), Some(row)) = (r.column, r.table_row)
                && row >= mastik_core::list::MAX_DRAWN
                && !taken.contains(&(d, row))
            {
                taken.push((d, row));
            }
        }
        let mut found = false;
        for set in doc.data_sets() {
            let Some(link) = &set.source else { continue };
            let rows: Vec<Vec<String>> = (taken.iter())
                .filter(|(d, _)| *d == set.id)
                .filter_map(|(_, r)| set.table.rows.get(*r).cloned())
                .collect();
            if rows.is_empty() {
                continue;
            }
            let table = mastik_core::chart::Table {
                columns: set.table.columns.clone(),
                rows,
            };
            let columns: Vec<_> = table.columns.iter().map(|c| c.id).collect();
            let dir = std::path::Path::new(&link.path).parent();
            let (images, pairs) =
                crate::list_data::read_pictures_in(doc, dir, &table, &columns, table.rows.len());
            for op in images {
                let mastik_core::Operation::AddImage { id, .. } = &op else {
                    continue;
                };
                let held = (self.read.iter()).any(
                    |o| matches!(o, mastik_core::Operation::AddImage { id: had, .. } if had == id),
                );
                if !held {
                    self.read.push(op);
                }
            }
            for (cell, id) in pairs {
                let known = set.picture(doc, &cell).is_some()
                    || (self.opts.pictures.iter()).any(|(s, c, _)| *s == set.id && *c == cell);
                if !known {
                    self.opts.pictures.push((set.id, cell, id));
                    found = true;
                }
            }
        }
        found
    }

    fn sample(&self, kind: Kind) -> &str {
        self.samples
            .1
            .iter()
            .find(|(k, _)| *k == kind)
            .map_or("", |(_, s)| s.as_str())
    }

    /// The preview's lines, record by record in the order first met: a frame's
    /// head with its layers under it, loose layers with no head.
    fn lines(&self) -> Vec<Line> {
        let mut out = Vec::new();
        for (r, record) in self.plan.records.iter().enumerate() {
            let rows: Vec<usize> = (0..self.plan.rows.len())
                .filter(|i| self.plan.rows[*i].record == r)
                .collect();
            if rows.is_empty() {
                continue;
            }
            if record.root.is_some() {
                out.push(Line::Head(r));
            }
            out.extend(rows.into_iter().map(Line::Row));
        }
        out
    }

    /// Why *Fill* is dim, or `None` when it would change something.
    fn why_not(&self) -> Option<&'static str> {
        match () {
            _ if self.plan.rows.is_empty() => Some(NO_TEXT),
            _ if self.plan.changes() == 0 => Some(ALL_KEPT),
            _ => None,
        }
    }
}

/// What `plan`'s records are called: the frames' shared name, or *record* while
/// private mode masks names, since the noun is one.
fn record_noun(plan: &Plan, doc: &Document) -> String {
    match crate::private::on() {
        true => "record".to_string(),
        false => plan.record_noun(doc),
    }
}

/// How many records `plan` fills, and the word for them: *3 rows*.
fn records_label(plan: &Plan, doc: &Document) -> Option<String> {
    let n = plan.filled_records().len();
    (n > 0).then(|| ui::count(n, &record_noun(plan, doc)))
}

/// The preview's count: *13 layers in 3 rows · 1 left as is*.
fn count_label(plan: &Plan, doc: &Document) -> String {
    let mut out = ui::count(plan.rows.len(), "layer");
    if let Some(records) = records_label(plan, doc) {
        out.push_str(&format!(" in {records}"));
    }
    if plan.left() > 0 {
        out.push_str(&format!(" · {} left as is", plan.left()));
    }
    out
}

impl MastikApp {
    /// *Fill with content…*: open the box on the selection, in the locale its
    /// text agrees on, else the last one used, with a new draw. It opens on any
    /// selection, none included: the box follows the selection made after, and
    /// says so while there is nothing to fill.
    pub(crate) fn open_fill_content(&mut self) {
        self.palette = None;
        self.context_menu = None;
        self.open_menu = crate::app::TopMenu::None;
        self.picker = None;
        self.cleanup = None;
        // One card over the canvas's top at a time.
        self.close_find();
        self.colour_review.open = false;
        self.system_health = None;
        self.batch_rename = None;
        self.clean_up = None;
        self.chart_data = None;
        let selection = self.selected_layers();
        let p = &self.prefs.fill_content;
        let last =
            fill::locale_for(&p.locale).map_or_else(|| "en-US".to_string(), |l| l.tag.clone());
        let locale = fill::agreed_locale(&self.session.doc, &selection)
            .map_or_else(|| last.clone(), |l| l.tag.clone());
        let style = match p.avatar_style.as_str() {
            "pattern" => AvatarStyle::Pattern,
            _ => AvatarStyle::Initials,
        };
        let (read, pictures) = read_beside(&self.session.doc);
        self.fill_content = Some(FillBox {
            opts: Options {
                locale,
                records: p.records,
                example_emails: p.example_emails,
                avatars: p.avatars.then_some(style),
                seed: fresh_seed(),
                pictures,
                ..Options::default()
            },
            opened_locale: last,
            locale_picked: false,
            style,
            selection,
            pin: None,
            plan: Plan::default(),
            every: Plan::default(),
            asked: None,
            samples: (String::new(), Vec::new()),
            hover: (Vec::new(), false),
            reveal: None,
            read,
        });
    }

    /// *Fill*: end a live text session first, since its buffer predates the fill
    /// and its commit when the session ended would write over the value filled;
    /// plan again on what that leaves, and write it. The box stays open when
    /// nothing is left to fill.
    fn run_fill_content(&mut self, mut b: FillBox) {
        self.finish_text_first();
        let rev = self.session.revision();
        b.replan(
            &self.session.doc,
            &self.session.resolved,
            self.session.page,
            rev,
        );
        match b.why_not() {
            None => self.commit_fill_content(&b),
            Some(_) => self.fill_content = Some(b),
        }
    }

    /// Write the fill as one undo step and remember the choices.
    fn commit_fill_content(&mut self, b: &FillBox) {
        let p = &mut self.prefs.fill_content;
        p.locale = b.opts.locale.clone();
        p.records = b.opts.records;
        p.example_emails = b.opts.example_emails;
        p.avatars = b.opts.avatars.is_some();
        // The style last picked on a row becomes the next box's, once every
        // shape picked agrees on it.
        let mut picked = b.opts.styles.values();
        if let Some(first) = picked.next()
            && picked.all(|s| s == first)
        {
            p.avatar_style = first.key().to_string();
        }
        self.prefs.save();
        let mut pictures = crate::canvas::Decoded::default();
        let mut tx = b.plan.transaction(
            &self.session.doc,
            &self.session.resolved,
            Some(&mut pictures),
        );
        self.adopt_decoded(pictures);
        tx.0.splice(0..0, b.beside_ops(&self.session.doc));
        // Counted from the edit: an avatar the sink could not draw is left out.
        let changed =
            tx.0.iter()
                .filter(|op| {
                    matches!(
                        op,
                        mastik_core::Operation::SetText { .. }
                            | mastik_core::Operation::SetFills { .. }
                    )
                })
                .count();
        self.session
            .name_step("Fill with content", icon::FILL_CONTENT);
        if self.session.commit(tx) {
            let mut said = format!("Filled {}", ui::count(changed, "layer"));
            let table = b
                .plan
                .rows
                .iter()
                .find_map(|r| r.table_row.and(r.column))
                .and_then(|(d, _)| self.session.doc.data_set(d));
            if let Some(set) = table {
                said.push_str(&format!(" from {}", set.name));
            }
            if b.plan.locked > 0 {
                said.push_str(&format!(" · skipped {} locked", b.plan.locked));
            }
            said.push_str(" · Ctrl+Z to undo");
            self.session.info(said);
        }
    }

    /// Draw the open box and answer this frame's clicks. An open card, as Clean
    /// up's: the canvas stays live under it and its plan follows the document and
    /// the selection. Escape reaches it through [`MastikApp::escape`].
    pub(crate) fn fill_content_ui(&mut self, ctx: &egui::Context, canvas: egui::Rect) {
        let Some(mut b) = self.fill_content.take() else {
            return;
        };
        let live = self.selected_layers();
        b.selection = match &b.pin {
            Some((roots, made)) if *made == live => roots.clone(),
            _ => {
                b.pin = None;
                live
            }
        };
        if let Some(id) = b.reveal.take()
            && let Some(bounds) = self.session.preview_world_bounds(id)
        {
            self.reveal(bounds);
        }
        let rev = self.session.revision();
        b.replan(
            &self.session.doc,
            &self.session.resolved,
            self.session.page,
            rev,
        );
        let why_not = b.why_not();
        let mut act = Act::None;
        let mut hover: (Vec<NodeId>, bool) = (Vec::new(), false);
        let mut clicked: Option<NodeId> = None;
        let doc = &self.session.doc;
        let hold = crate::settings::CardHold::Open;
        crate::settings::movable_canvas_card(ctx, "fill-content", canvas, W, hold, |ui| {
            if crate::settings::card_head(ui, W, "Fill with content") {
                act = Act::Cancel;
            }
            egui::Frame::NONE
                .inner_margin(egui::Margin {
                    left: PAD as i8,
                    right: PAD as i8,
                    top: PAD as i8,
                    bottom: 4,
                })
                .show(ui, |ui| {
                    ui.spacing_mut().item_spacing.y = GAP;
                    locale_row(ui, &mut b);
                    content_row(ui, doc, &mut b);
                    table_rows_row(ui, doc, &mut b);
                    // The switches stand closer, as Clean up's do; the gap above
                    // them was laid with the Content row.
                    ui.spacing_mut().item_spacing.y = SWITCH_GAP;
                    records_row(ui, doc, &mut b);
                    emails_row(ui, &mut b);
                    avatars_row(ui, &mut b);
                });
            ui.add_space(8.0);
            ui::card_rule(ui);
            preview(ui, doc, &mut b, &mut hover, &mut clicked);
            ui::card_rule(ui);
            if let Some(a) = foot(ui, b.plan.changes(), why_not) {
                act = a;
            }
        });
        b.hover = hover;
        if let Some(id) = clicked {
            b.pin = Some((b.selection.clone(), vec![id]));
            self.session.selection.set_one(id);
            b.reveal = Some(id);
        }
        match act {
            Act::Cancel => {}
            Act::Run if why_not.is_none() => {
                self.run_fill_content(b);
                ctx.request_repaint();
            }
            _ => self.fill_content = Some(b),
        }
    }
}

enum Act {
    None,
    Run,
    Cancel,
}

/// A full-width dropdown in a label row, as batch rename's *Order* is.
fn combo_in_row(
    ui: &mut egui::Ui,
    id: &str,
    shown: &str,
    hint: &str,
    list: impl FnOnce(&mut egui::Ui),
) {
    ui.spacing_mut().interact_size.y = ui::CONTROL_H;
    ui.spacing_mut().button_padding.y = 0.0;
    egui::ComboBox::from_id_salt(id)
        .icon(ui::combo_chevron)
        .width(ui.available_width())
        .height(KIND_LIST_H)
        .selected_text(shown)
        .show_ui(ui, |ui| {
            ui::menu_rows(ui);
            list(ui);
        })
        .response
        .on_hover_text(hint);
}

fn locale_row(ui: &mut egui::Ui, b: &mut FillBox) {
    let shown = fill::locale_for(&b.opts.locale).map_or("", |l| l.name.as_str());
    row(ui, "Locale", ui::CONTROL_H, |ui| {
        combo_in_row(
            ui,
            "fill-content-locale",
            shown,
            "Names, addresses, phone numbers, dates and prices follow it",
            |ui| {
                for l in fill::locales() {
                    let on = l.tag == b.opts.locale;
                    if ui::menu_row(ui, MenuRow::new("", &l.name).checked(on), ui::MENU_ROW_H)
                        .clicked()
                    {
                        b.opts.locale = l.tag.clone();
                        b.locale_picked = true;
                        ui.close();
                    }
                }
            },
        );
    });
}

/// What a kind list's row picks: a kind, `None` for the head row (*By layer
/// name* or *Leave as is*), and for an avatar the style it is drawn in.
type Pick = (Option<Kind>, Option<AvatarStyle>);

/// The avatar styles a shape's list offers, one row each.
const STYLES: [AvatarStyle; 2] = [AvatarStyle::Initials, AvatarStyle::Pattern];

/// An avatar's choice as its row and its list name it: *Avatar · Pattern*.
fn avatar_label(style: AvatarStyle) -> String {
    format!("{} · {}", Kind::Avatar.label(), style.label())
}

/// What a kind list offers from the document's tables: whether *By column
/// name* leads each table (the Content row only), and the choice now.
struct TableAsk<'a> {
    doc: &'a Document,
    by_name: bool,
    now: Option<TableContent>,
    /// The pictures read beside linked tables ([`Options::pictures`]).
    beside: &'a [Beside],
}

/// A picture read beside a linked table: (table, cell, picture).
type Beside = (mastik_core::list::DataSetId, String, mastik_core::ImageId);

/// Whether `cell` of table `set` names a picture: the set's own, or one read
/// beside its file.
fn names_picture(
    doc: &Document,
    set: &mastik_core::list::DataSet,
    beside: &[Beside],
    cell: &str,
) -> bool {
    set.picture(doc, cell).is_some()
        || beside
            .iter()
            .any(|(s, c, _)| *s == set.id && c == cell.trim())
}

/// The pictures every linked table's cells name beside its file, in the rows a
/// list reads, which its set does not map yet: each new picture's `AddImage`,
/// and the (table, cell, picture) triples [`Options::pictures`] takes.
fn read_beside(doc: &Document) -> (Vec<mastik_core::Operation>, Vec<Beside>) {
    let (mut ops, mut beside) = (Vec::new(), Vec::new());
    for set in doc.data_sets() {
        let Some(link) = &set.source else { continue };
        let dir = std::path::Path::new(&link.path).parent();
        let columns: Vec<_> = set.table.columns.iter().map(|c| c.id).collect();
        let (images, pairs) = crate::list_data::read_pictures(doc, dir, &set.table, &columns);
        for op in images {
            let new = !ops.iter().any(|o| match (o, &op) {
                (
                    mastik_core::Operation::AddImage { id: a, .. },
                    mastik_core::Operation::AddImage { id: b, .. },
                ) => a == b,
                _ => false,
            });
            if new {
                ops.push(op);
            }
        }
        beside.extend(
            pairs
                .into_iter()
                .filter(|(cell, _)| set.picture(doc, cell).is_none())
                .map(|(cell, id)| (set.id, cell, id)),
        );
    }
    (ops, beside)
}

/// A table column as a row's combo names it: *People ▸ Name*.
fn column_words(doc: &Document, (d, c): fill::Cell) -> String {
    let set = doc.data_set(d);
    let table = set.map_or("Missing table", |s| s.name.as_str());
    let column = set
        .and_then(|s| s.table.column(c))
        .map_or("Missing column", |c| c.name.as_str());
    format!("{table} \u{25B8} {column}")
}

/// The document's tables, real data before invented: each under an eyebrow
/// naming it and its rows, *By column name* first when asked, then its
/// columns with the first row's cell as the faint tail. A shape's list shows
/// only the tables and columns that name pictures; a table with no rows shows
/// its eyebrow alone. Answers the pick.
fn table_rows(ui: &mut egui::Ui, ask: &TableAsk<'_>, shape: bool) -> Option<TableContent> {
    let doc = ask.doc;
    let mut picked = None;
    for set in doc.data_sets() {
        let images: Vec<_> = set
            .table
            .columns
            .iter()
            .filter(|c| {
                set.table
                    .cells(c.id)
                    .any(|v| names_picture(doc, set, ask.beside, v))
            })
            .map(|c| c.id)
            .collect();
        let columns: Vec<&mastik_core::chart::Column> = set
            .table
            .columns
            .iter()
            .filter(|c| !shape || images.contains(&c.id))
            .collect();
        if columns.is_empty() {
            continue;
        }
        let rows = set.table.rows.len();
        let tail = match rows {
            0 => "no rows".to_string(),
            n => ui::count(n, "row"),
        };
        ui.add_space(4.0);
        ui.horizontal(|ui| {
            ui.add_space(8.0);
            ui.label(ui::eyebrow(&format!("{} \u{b7} {tail}", set.name)));
        });
        if rows == 0 {
            continue;
        }
        if ask.by_name && !shape {
            let on = ask.now == Some(TableContent::ByColumnName(set.id));
            let row = MenuRow::new("", "By column name").checked(on);
            if ui::menu_row(ui, row, ui::MENU_ROW_H).clicked() {
                picked = Some(TableContent::ByColumnName(set.id));
            }
        }
        for c in columns {
            let cell = (set.id, c.id);
            let on = ask.now == Some(TableContent::Column(cell));
            let sample = match shape {
                true => ui::count(
                    set.table
                        .cells(c.id)
                        .filter(|v| names_picture(doc, set, ask.beside, v))
                        .count(),
                    "picture",
                ),
                false => set.table.rows[0]
                    .get(set.table.index_of(c.id).unwrap_or(0))
                    .cloned()
                    .unwrap_or_default(),
            };
            // An image column does nothing to a text: listed, dimmed.
            let enabled = shape || !images.contains(&c.id);
            let row = MenuRow::new("", &c.name)
                .checked(on)
                .note(&sample)
                .enabled(enabled);
            if ui::menu_row(ui, row, ui::MENU_ROW_H).clicked() && enabled {
                picked = Some(TableContent::Column(cell));
            }
        }
    }
    if picked.is_some() {
        ui.close();
    }
    picked
}

/// The kinds a text row or the Content row can choose, under their groups'
/// eyebrows, each with its sample as a faint tail; a shape's list is its avatar
/// styles instead, `style` the one it is drawn in now. The document's tables
/// lead the groups (`tables`), their pick answered through `table`.
fn kind_list(
    ui: &mut egui::Ui,
    b: &FillBox,
    head: &str,
    current: Option<Kind>,
    shape: Option<AvatarStyle>,
    tables: &TableAsk<'_>,
    table: &mut Option<TableContent>,
) -> Option<Pick> {
    ui.set_min_width(KIND_LIST_W);
    let mut picked = None;
    if ui::menu_row(
        ui,
        MenuRow::new("", head).checked(current.is_none() && tables.now.is_none()),
        ui::MENU_ROW_H,
    )
    .clicked()
    {
        picked = Some((None, None));
    }
    *table = table_rows(ui, tables, shape.is_some());
    if table.is_some() {
        return None;
    }
    if let Some(style) = shape {
        let note = b.sample(Kind::Avatar);
        for s in STYLES {
            let on = current == Some(Kind::Avatar) && style == s;
            let label = avatar_label(s);
            if ui::menu_row(
                ui,
                MenuRow::new("", &label).checked(on).note(note),
                ui::MENU_ROW_H,
            )
            .clicked()
            {
                picked = Some((Some(Kind::Avatar), Some(s)));
            }
        }
    } else {
        let mut group: Option<Group> = None;
        for kind in Kind::ALL.into_iter().filter(|k| k.is_text()) {
            if group != Some(kind.group()) {
                group = Some(kind.group());
                ui.add_space(4.0);
                ui.horizontal(|ui| {
                    ui.add_space(8.0);
                    ui.label(ui::eyebrow(kind.group().label()));
                });
            }
            let on = current == Some(kind);
            let row = MenuRow::new("", kind.label())
                .checked(on)
                .note(b.sample(kind));
            if ui::menu_row(ui, row, ui::MENU_ROW_H).clicked() {
                picked = Some((Some(kind), None));
            }
        }
    }
    if picked.is_some() {
        ui.close();
    }
    picked
}

fn content_row(ui: &mut egui::Ui, doc: &Document, b: &mut FillBox) {
    let shown = match b.opts.table {
        Some(TableContent::ByColumnName(d)) => {
            let name = doc.data_set(d).map_or("Missing table", |s| s.name.as_str());
            format!("{name} \u{b7} by column name")
        }
        Some(TableContent::Column(c)) => column_words(doc, c),
        None => b
            .opts
            .content
            .map_or("By layer name", Kind::label)
            .to_string(),
    };
    let mut picked = None;
    let mut table = None;
    let ask = TableAsk {
        doc,
        by_name: true,
        now: b.opts.table,
        beside: &b.opts.pictures,
    };
    row(ui, "Content", ui::CONTROL_H, |ui| {
        combo_in_row(
            ui,
            "fill-content-kind",
            &shown,
            "What every text layer is filled with",
            |ui| {
                picked = kind_list(
                    ui,
                    b,
                    "By layer name",
                    b.opts.content,
                    None,
                    &ask,
                    &mut table,
                )
            },
        );
    });
    if picked.is_none() && table.is_none() {
        return;
    }
    b.opts.table = table;
    b.opts.content = picked.and_then(|(kind, _)| kind);
    // The box's choice is one for all: text rows chosen one by one give way.
    let shapes: Vec<NodeId> = b
        .plan
        .rows
        .iter()
        .filter(|r| r.shape)
        .map(|r| r.node)
        .collect();
    b.opts.kinds.retain(|id, _| shapes.contains(id));
    b.opts.columns.retain(|id, _| shapes.contains(id));
}

/// *Rows*, while anything reads a table: the row the first record takes, and
/// whether the rows come in order or dealt by the seed; then what the table
/// could not give.
fn table_rows_row(ui: &mut egui::Ui, doc: &Document, b: &mut FillBox) {
    let Some(data) = b.plan.rows.iter().find_map(|r| r.column.map(|c| c.0)) else {
        return;
    };
    row(ui, "Rows", ui::CONTROL_H, |ui| {
        ui.horizontal(|ui| {
            ui.spacing_mut().item_spacing.x = ui::CARD_COL_GAP;
            let half = (ui.available_width() - ui::CARD_COL_GAP) / 2.0;
            let len = doc.data_set(data).map_or(1, |s| s.table.rows.len().max(1));
            let mut v = (b.opts.from_row + 1) as f64;
            ui::value_field(
                ui,
                egui::vec2(half, ui::CONTROL_H),
                ui::Prefix::Label("From row"),
                &mut v,
                ui::Scrub::whole(0.25).range(1.0..=len as f64),
                |d| d.custom_formatter(ui::number(0)),
            )
            .on_hover_text("The row the first record takes");
            b.opts.from_row = (v.round().clamp(1.0, len as f64) as usize) - 1;
            let at = usize::from(b.opts.shuffled);
            let pick = ui::segmented_tipped(
                ui,
                half,
                ui::SEGMENT_CELL_H,
                2,
                at,
                |_| true,
                |i| ["Each record takes the next row", "Rows dealt by the draw"][i],
                |p, i, r, on| ui::segment_label(p, r, ["In order", "Shuffled"][i], on),
            );
            if let Some(i) = pick {
                b.opts.shuffled = i == 1;
            }
        });
    });
    let name = doc.data_set(data).map_or("", |s| s.name.as_str());
    if b.plan.short > 0 {
        let rows = doc.data_set(data).map_or(0, |s| s.table.rows.len());
        // Records off, each layer takes its own row, so layers are what is short.
        let noun = match b.opts.records {
            true => record_noun(&b.plan, doc),
            false => "layer".to_string(),
        };
        let said = format!(
            "{} left as is: {name} has {}",
            ui::count(b.plan.short, &noun),
            ui::count(rows, "row")
        );
        ui::note_line(ui, icon::WARNING, color::warn(), &said, None);
    }
    if b.plan.missing_pictures > 0 {
        let said = format!(
            "{} not found",
            ui::count(b.plan.missing_pictures, "picture")
        );
        ui::note_line(ui, icon::WARNING, color::warn(), &said, None);
    }
}

fn records_row(ui: &mut egui::Ui, doc: &Document, b: &mut FillBox) {
    let noun = record_noun(&b.every, doc);
    let note = records_label(&b.every, doc).unwrap_or_default();
    row(ui, "Records", ui::CONTROL_H, |ui| {
        let label = format!("Keep each {noun}'s values together");
        if ui::switch_row_noted(ui, &label, &note, b.opts.records, ui::CONTROL_H)
            .on_hover_text("A row's email is built from its name, its city from its postcode")
            .clicked()
        {
            b.opts.records = !b.opts.records;
        }
    });
}

fn emails_row(ui: &mut egui::Ui, b: &mut FillBox) {
    row(ui, "Emails", ui::CONTROL_H, |ui| {
        if ui::switch_row_noted(
            ui,
            "Use example.com addresses",
            "",
            b.opts.example_emails,
            ui::CONTROL_H,
        )
        .on_hover_text("A domain reserved for examples, which reaches no one")
        .clicked()
        {
            b.opts.example_emails = !b.opts.example_emails;
        }
    });
}

fn avatars_row(ui: &mut egui::Ui, b: &mut FillBox) {
    let shapes = b
        .every
        .rows
        .iter()
        .filter(|r| r.shape && r.kind == Some(Kind::Avatar))
        .count();
    // Each shape's style is picked on its own row in the preview.
    row(ui, "Avatars", ui::CONTROL_H, |ui| {
        let on = b.opts.avatars.is_some();
        let label = ui::count(shapes, "shape");
        if ui::switch_row_noted(ui, &label, "", on, ui::CONTROL_H)
            .on_hover_text("Shapes named Avatar or Headshot, or just Photo or Profile picture")
            .clicked()
        {
            b.opts.avatars = match on {
                true => None,
                false => Some(b.style),
            };
        }
    });
}

/// The preview: every layer, under its record's frame. Hovering a row rings its
/// layer and a frame's head the frame, softly; clicking a row selects its layer.
fn preview(
    ui: &mut egui::Ui,
    doc: &Document,
    b: &mut FillBox,
    hover: &mut (Vec<NodeId>, bool),
    clicked: &mut Option<NodeId>,
) {
    let (rect, y) = crate::settings::card_section_head(ui, W, "Preview", None);
    let die = egui::Rect::from_center_size(
        egui::pos2(rect.right() - PAD - DIE / 2.0 + 4.0, y),
        egui::vec2(DIE, DIE),
    );
    let shuffle = ui
        .put(die, |ui: &mut egui::Ui| {
            ui::icon_button(ui, icon::DICE, DIE, DIE_PT, false, !b.plan.rows.is_empty())
        })
        .on_hover_text("Shuffle · Draw new values for every layer");
    if shuffle.clicked() {
        b.opts.seed = fresh_seed();
        b.opts.redraws.clear();
    }
    ui.painter().text(
        egui::pos2(die.left() - 6.0, y),
        egui::Align2::RIGHT_CENTER,
        count_label(&b.plan, doc),
        egui::FontId::proportional(ui::CAPTION_PT),
        theme::text::dim(),
    );
    let lines = b.lines();
    let mut chosen: Option<(NodeId, Pick)> = None;
    let mut column: Option<(NodeId, fill::Cell)> = None;
    let mut redraw: Option<NodeId> = None;
    egui::Frame::NONE
        .inner_margin(egui::Margin {
            left: 6,
            right: 6,
            top: 0,
            bottom: 8,
        })
        .show(ui, |ui| {
            if lines.is_empty() {
                let (rect, _) =
                    ui.allocate_exact_size(egui::vec2(W - 12.0, ROW_H), egui::Sense::hover());
                ui.scope_builder(
                    egui::UiBuilder::new()
                        .max_rect(rect.with_min_x(rect.left() + 8.0))
                        .layout(egui::Layout::left_to_right(egui::Align::Center)),
                    |ui| ui::empty_note(ui, NO_TEXT),
                );
                return;
            }
            // Scrolls by wheel and drag with no bar, as Clean up's preview.
            egui::ScrollArea::vertical()
                .id_salt("fill-content-preview")
                .max_height(ROW_H * ROWS_SHOWN)
                .auto_shrink([false, true])
                .scroll_bar_visibility(egui::scroll_area::ScrollBarVisibility::AlwaysHidden)
                .show_rows(ui, ROW_H, lines.len(), |ui, range| {
                    ui.spacing_mut().item_spacing.y = 0.0;
                    for line in &lines[range] {
                        match line {
                            Line::Head(r) => {
                                let root = b.plan.records[*r].root;
                                if head(ui, doc, root).hovered() {
                                    *hover = (root.into_iter().collect(), true);
                                }
                            }
                            Line::Row(i) => {
                                let out = fill_row(ui, doc, b, *i);
                                if out.hovered {
                                    *hover = (vec![b.plan.rows[*i].node], false);
                                }
                                if out.clicked {
                                    *clicked = Some(b.plan.rows[*i].node);
                                }
                                if let Some(kind) = out.kind {
                                    chosen = Some((b.plan.rows[*i].node, kind));
                                }
                                if let Some(c) = out.column {
                                    column = Some((b.plan.rows[*i].node, c));
                                }
                                if out.redraw {
                                    redraw = Some(b.plan.rows[*i].node);
                                }
                            }
                        }
                    }
                });
        });
    if let Some((node, (kind, style))) = chosen {
        b.opts.kinds.insert(node, kind);
        b.opts.columns.remove(&node);
        if let Some(style) = style {
            b.opts.styles.insert(node, style);
        }
    }
    if let Some((node, c)) = column {
        b.opts.columns.insert(node, c);
        b.opts.kinds.remove(&node);
    }
    if let Some(node) = redraw {
        *b.opts.redraws.entry(node).or_default() += 1;
    }
}

/// A record frame's head: its name, dim.
fn head(ui: &mut egui::Ui, doc: &Document, root: Option<NodeId>) -> egui::Response {
    let (rect, resp) = ui.allocate_exact_size(egui::vec2(W - 12.0, ROW_H), egui::Sense::hover());
    let name = root.map_or_else(String::new, |id| {
        crate::private::layer(doc, id).into_owned()
    });
    ui.painter().text(
        egui::pos2(rect.left() + 8.0, rect.center().y),
        egui::Align2::LEFT_CENTER,
        name,
        egui::FontId::proportional(ui::CAPTION_PT),
        theme::text::dim(),
    );
    resp
}

/// What a preview row's pointer did this frame.
#[derive(Default)]
struct RowOut {
    hovered: bool,
    clicked: bool,
    /// A kind, and an avatar's style, picked from its list.
    kind: Option<Pick>,
    /// A table column picked from its list.
    column: Option<fill::Cell>,
    redraw: bool,
}

/// One text cut to `w` with an ellipsis, left-aligned at `x` on `rect`'s centre.
fn clipped(
    ui: &egui::Ui,
    rect: egui::Rect,
    x: f32,
    w: f32,
    text: &str,
    ink: egui::Color32,
) -> egui::Rect {
    let mut job = egui::text::LayoutJob::single_section(
        text.to_string(),
        egui::TextFormat::simple(egui::FontId::proportional(ui::CONTROL_PT), ink),
    );
    job.wrap = egui::text::TextWrapping::truncate_at_width(w.max(0.0));
    let galley = ui.fonts_mut(|f| f.layout_job(job));
    let at = egui::pos2(x, rect.center().y - galley.size().y / 2.0);
    let drawn = egui::Rect::from_min_size(at, galley.size());
    ui.painter().galley(at, galley, ink);
    drawn
}

fn fill_row(ui: &mut egui::Ui, doc: &Document, b: &FillBox, i: usize) -> RowOut {
    let r = &b.plan.rows[i];
    let (rect, resp) = ui.allocate_exact_size(egui::vec2(W - 12.0, ROW_H), egui::Sense::click());
    // The combo and the die sit over the row and take its hover; the ground and
    // the ring follow the pointer anywhere on it.
    let over = ui.rect_contains_pointer(rect);
    ui::list_row_ground(ui.painter(), rect, false, over);
    let inner = rect.shrink2(egui::vec2(8.0, 0.0));
    let y = rect.center().y;
    let (glyph, tint) = doc
        .get(r.node)
        .map(crate::panels::layers::node_glyph)
        .map_or((icon::SQUARE, theme::text::dim()), |(g, t)| (g, t.colour()));
    ui.painter().text(
        egui::pos2(inner.left() + 8.0, y),
        egui::Align2::CENTER_CENTER,
        glyph,
        theme::icon_font(14.0),
        tint,
    );
    let name_x = inner.left() + 24.0;
    clipped(
        ui,
        rect,
        name_x,
        NAME_W - 24.0 - 8.0,
        &layer_name(doc, r.node),
        theme::text::muted(),
    );
    let mut out = RowOut {
        hovered: over,
        clicked: resp.clicked(),
        ..RowOut::default()
    };

    // The kind, a small combo.
    let kind_rect = egui::Rect::from_min_size(
        egui::pos2(inner.left() + NAME_W, y - KIND_H / 2.0),
        egui::vec2(KIND_W, KIND_H),
    );
    // A shape's style: the one picked for it, else the box's.
    let style = r.shape.then(|| {
        b.opts
            .styles
            .get(&r.node)
            .copied()
            .unwrap_or_else(|| b.opts.avatars.unwrap_or(b.style))
    });
    let shown = match (r.column, r.kind, style) {
        (Some(c), _, _) => column_words(doc, c),
        (None, Some(Kind::Avatar), Some(s)) => avatar_label(s),
        (None, kind, _) => kind.map_or("Leave as is", Kind::label).to_string(),
    };
    let ask = TableAsk {
        doc,
        by_name: false,
        now: r.column.map(TableContent::Column),
        beside: &b.opts.pictures,
    };
    // The combo and the die go in children that do not move the list's cursor: a
    // placed child resets it to its own bottom, above the row's, and every row
    // under a hovered one would ride up.
    let mut kind_ui = ui.new_child(egui::UiBuilder::new().max_rect(kind_rect));
    kind_ui.spacing_mut().interact_size.y = KIND_H;
    kind_ui.spacing_mut().button_padding.y = 0.0;
    egui::ComboBox::from_id_salt(("fill-content-row", r.node))
        .icon(ui::combo_chevron)
        .width(KIND_W)
        .height(KIND_LIST_H)
        .selected_text(shown)
        .show_ui(&mut kind_ui, |ui| {
            ui::menu_rows(ui);
            let mut table = None;
            out.kind = kind_list(ui, b, "Leave as is", r.kind, style, &ask, &mut table);
            out.column = table.and_then(|t| match t {
                TableContent::Column(c) => Some(c),
                TableContent::ByColumnName(_) => None,
            });
        });

    // The value, and the die at its end while the row is hovered.
    let value_x = kind_rect.right() + 10.0;
    let die = egui::Rect::from_center_size(
        egui::pos2(inner.right() - DIE / 2.0 + 4.0, y),
        egui::vec2(DIE, DIE),
    );
    // The locale has one country, so a Country row has nothing to redraw.
    let redrawable = over && r.kind.is_some_and(|k| k != Kind::Country);
    let value_w = match redrawable {
        true => die.left() - 4.0 - value_x,
        false => inner.right() - value_x,
    };
    match &r.value {
        Value::Kept => {
            let now = match doc.get(r.node).map(|n| n.kind()) {
                Some(NodeKind::Text { content, .. }) => content.lines().next().unwrap_or(""),
                _ => "",
            };
            clipped(ui, rect, value_x, value_w, now, theme::text::faint());
        }
        Value::Text(text) => {
            let first = text.lines().next().unwrap_or("");
            let ink = match r.fits {
                true => color::text(),
                false => color::warn(),
            };
            // A table's value says which row it came from, faint at the end.
            let tail_w = match r.table_row {
                Some(n) => {
                    let tail = ui.painter().text(
                        egui::pos2(value_x + value_w, y),
                        egui::Align2::RIGHT_CENTER,
                        format!("row {}", n + 1),
                        egui::FontId::proportional(ui::CAPTION_PT),
                        theme::text::faint(),
                    );
                    tail.width() + 6.0
                }
                None => 0.0,
            };
            let drawn = clipped(ui, rect, value_x, value_w - tail_w, first, ink);
            if !r.fits {
                ui.interact(
                    drawn,
                    ui.id().with(("fill-overflow", r.node)),
                    egui::Sense::hover(),
                )
                .on_hover_text("Longer than its box: no draw fitted");
            }
        }
        Value::Avatar(a) => {
            let c = egui::pos2(value_x + SWATCH / 2.0, y);
            let [r8, g8, b8, _] = a.ground().to_rgba8().to_u8_array();
            ui.painter()
                .circle_filled(c, SWATCH / 2.0, egui::Color32::from_rgb(r8, g8, b8));
            if a.style == AvatarStyle::Initials {
                ui.painter().text(
                    c,
                    egui::Align2::CENTER_CENTER,
                    &a.initials,
                    egui::FontId::proportional(ui::EYEBROW_PT),
                    // text-ink: white on the avatar's own ground, as the picture draws it.
                    egui::Color32::WHITE,
                );
            }
            clipped(
                ui,
                rect,
                value_x + SWATCH + 6.0,
                value_w - SWATCH - 6.0,
                &a.name,
                theme::text::dim(),
            );
        }
        Value::Picture(id) => {
            let words = crate::panels::picture_words(doc, id);
            clipped(ui, rect, value_x, value_w, &words, color::text());
        }
    }
    if redrawable {
        let mut child = ui.new_child(egui::UiBuilder::new().max_rect(die));
        let resp = ui::icon_button(&mut child, icon::DICE, DIE, DIE_PT, false, true)
            .on_hover_text("Draw another");
        out.redraw = resp.clicked();
        out.clicked &= !out.redraw;
    }
    out
}

/// The footer: the hint, then *Cancel* and *Fill N layers*.
fn foot(ui: &mut egui::Ui, changes: usize, why_not: Option<&str>) -> Option<Act> {
    let go = match changes {
        0 => "Fill".to_string(),
        n => format!("Fill {}", ui::count(n, "layer")),
    };
    let state = match why_not {
        None => FieldButton::Primary,
        Some(_) => FieldButton::Disabled,
    };
    let buttons = [("Cancel", FieldButton::Off), (go.as_str(), state)];
    match crate::settings::card_foot(ui, W, "Esc closes", &buttons, why_not) {
        Some(0) => Some(Act::Cancel),
        Some(_) => Some(Act::Run),
        None => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use mastik_core::kurbo::{Affine, Size};
    use mastik_core::{Operation, TextSizing, TextStyle, Transaction};

    fn app() -> (egui::Context, MastikApp) {
        let ctx = egui::Context::default();
        let app = MastikApp::headless(&ctx);
        (ctx, app)
    }

    /// One frame of the whole app.
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

    fn add(app: &mut MastikApp, parent: NodeId, kind: NodeKind, name: &str) -> NodeId {
        let s = &mut app.session;
        let id = s.ids.mint();
        let index = s.doc.get(parent).map_or(0, |n| n.children().len());
        assert!(s.commit(Transaction(vec![Operation::CreateNode {
            id,
            parent,
            index,
            kind,
            transform: Some(Affine::IDENTITY),
            name: Some(name.into()),
        }])));
        id
    }

    fn text(content: &str) -> NodeKind {
        NodeKind::Text {
            content: content.into(),
            style: Box::new(TextStyle::default()),
            spans: Default::default(),
            para_spans: Default::default(),
            paragraph: Default::default(),
            block: Default::default(),
            sizing: TextSizing::Auto,
            on_path: None,
            on_path_flip: false,
            on_path_offset: 0.0,
        }
    }

    fn frame() -> NodeKind {
        NodeKind::Artboard {
            size: Size::new(300.0, 60.0),
            corner_radii: Default::default(),
            corner_smoothing: 0.0,
        }
    }

    /// Two rows of an avatar, a name and an email, selected by their list.
    fn team(app: &mut MastikApp) -> (NodeId, Vec<NodeId>) {
        let page = app.session.page;
        let list = add(app, page, frame(), "Team");
        let rows = (1..=2)
            .map(|i| {
                let row = add(app, list, frame(), &format!("Card {i}"));
                let avatar = NodeKind::Ellipse {
                    size: Size::new(24.0, 24.0),
                    arc: Default::default(),
                };
                add(app, row, avatar, "Avatar");
                add(app, row, text("Name"), "Name");
                add(app, row, text("x@y.z"), "Email");
                row
            })
            .collect();
        app.session.selection.set_one(list);
        (list, rows)
    }

    fn content(app: &MastikApp, id: NodeId) -> String {
        match app.session.doc.get(id).map(|n| n.kind()) {
            Some(NodeKind::Text { content, .. }) => content.clone(),
            _ => String::new(),
        }
    }

    /// Opening over a list and filling writes every text and draws every avatar
    /// as one undo step, names the rows after their frames, and keeps the
    /// choices for next time.
    /// flip: `commit_fill_content` committing without the pictures' sink → fails
    /// at "each avatar is a picture".
    #[test]
    fn fill_writes_texts_and_avatars_in_one_step() {
        let (ctx, mut app) = app();
        let (_, rows) = team(&mut app);
        pass(&ctx, &mut app);
        app.dispatch(&ctx, crate::input::Action::FillContent);
        pass(&ctx, &mut app);
        let b = app.fill_content.as_mut().expect("the box opened");
        assert_eq!(b.plan.changes(), 6, "two names, two emails, two avatars");
        assert_eq!(
            records_label(&b.plan, &app.session.doc).as_deref(),
            Some("2 cards")
        );
        b.opts.locale = "de".into();
        let depth = app.session.history.undo_depth();
        let b = app.fill_content.take().unwrap();
        pass(&ctx, &mut app);
        let mut b = b;
        b.replan(
            &app.session.doc,
            &app.session.resolved,
            app.session.page,
            app.session.revision(),
        );
        app.commit_fill_content(&b);
        assert_eq!(app.session.history.undo_depth(), depth + 1, "one undo step");
        let name = |row: NodeId, n: &str| {
            let doc = &app.session.doc;
            *doc.get(row)
                .unwrap()
                .children()
                .iter()
                .find(|c| doc.get(**c).unwrap().name() == n)
                .unwrap()
        };
        for row in &rows {
            assert_ne!(content(&app, name(*row, "Name")), "Name");
            assert!(content(&app, name(*row, "Email")).ends_with(".de"));
            let avatar = app.session.doc.get(name(*row, "Avatar")).unwrap();
            assert!(
                avatar.paint().image_fill().is_some(),
                "each avatar is a picture"
            );
        }
        assert_eq!(app.prefs.fill_content.locale, "de", "the locale is kept");
        assert!(
            app.session.status().text.starts_with("Filled 6 layers"),
            "{}",
            app.session.status().text
        );
        let first = name(rows[0], "Name");
        assert!(app.session.undo());
        assert_eq!(content(&app, first), "Name", "undo puts it back");
    }

    /// *Content* from a table by column name fills each card's Name from the
    /// next row in one step, says the table in the status line, and leaves the
    /// document's table as it was.
    /// flip: `table_values` skipping text rows → fails at "the table's name".
    #[test]
    fn a_table_fills_the_cards_by_column_name() {
        use mastik_core::chart::{Column, ColumnId, Table};
        let (ctx, mut app) = app();
        let (_, rows) = team(&mut app);
        let set = mastik_core::list::DataSet {
            id: app.session.ids.mint_data_set(),
            name: "People".into(),
            table: Table {
                columns: vec![Column {
                    id: ColumnId(0),
                    name: "Name".into(),
                }],
                rows: vec![vec!["Ada Lovelace".into()], vec!["Grace Hopper".into()]],
            },
            source: None,
            pictures: Vec::new(),
        };
        let data = set.id;
        assert!(app.session.commit(Transaction(vec![Operation::SetDataSet {
            id: data,
            set: Some(Box::new(set)),
        }])));
        pass(&ctx, &mut app);
        app.dispatch(&ctx, crate::input::Action::FillContent);
        pass(&ctx, &mut app);
        let mut b = app.fill_content.take().expect("the box opened");
        b.opts.table = Some(TableContent::ByColumnName(data));
        b.replan(
            &app.session.doc,
            &app.session.resolved,
            app.session.page,
            app.session.revision(),
        );
        app.commit_fill_content(&b);
        let name = |row: NodeId| {
            let doc = &app.session.doc;
            *doc.get(row)
                .unwrap()
                .children()
                .iter()
                .find(|c| doc.get(**c).unwrap().name() == "Name")
                .unwrap()
        };
        assert_eq!(
            content(&app, name(rows[0])),
            "Ada Lovelace",
            "the table's name"
        );
        assert_eq!(content(&app, name(rows[1])), "Grace Hopper");
        let said = app.session.status().text.clone();
        assert!(said.contains("from People"), "{said}");
    }

    /// A linked table's picture column fills the avatars from the files beside
    /// it, read when the box opens, and the table then maps the pictures used.
    /// flip: `read_beside` reading nothing → fails at "nobody.png is counted".
    #[test]
    fn a_linked_tables_pictures_beside_its_file_fill_the_avatars() {
        use mastik_core::chart::{Column, ColumnId, DataLink, Table};
        let (ctx, mut app) = app();
        let (_, rows) = team(&mut app);
        let dir = std::env::temp_dir().join(format!("mastik-fill-beside-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let png = crate::app::rgba_to_png(&[255, 0, 0, 255], (1, 1)).unwrap();
        std::fs::write(dir.join("ada.png"), png).unwrap();
        let set = mastik_core::list::DataSet {
            id: app.session.ids.mint_data_set(),
            name: "People".into(),
            table: Table {
                columns: vec![Column {
                    id: ColumnId(0),
                    name: "Photo".into(),
                }],
                rows: vec![vec!["ada.png".into()], vec!["nobody.png".into()]],
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
        assert!(app.session.commit(Transaction(vec![Operation::SetDataSet {
            id: data,
            set: Some(Box::new(set)),
        }])));
        pass(&ctx, &mut app);
        app.dispatch(&ctx, crate::input::Action::FillContent);
        pass(&ctx, &mut app);
        let mut b = app.fill_content.take().expect("the box opened");
        let avatar = |app: &MastikApp, row: NodeId| {
            let doc = &app.session.doc;
            *doc.get(row)
                .unwrap()
                .children()
                .iter()
                .find(|c| doc.get(**c).unwrap().name() == "Avatar")
                .unwrap()
        };
        for row in &rows {
            b.opts
                .columns
                .insert(avatar(&app, *row), (data, ColumnId(0)));
        }
        b.replan(
            &app.session.doc,
            &app.session.resolved,
            app.session.page,
            app.session.revision(),
        );
        assert_eq!(b.plan.missing_pictures, 1, "nobody.png is counted");
        app.commit_fill_content(&b);
        let doc = &app.session.doc;
        let set = doc.data_set(data).unwrap();
        let id = set.picture(doc, "ada.png").expect("the table maps it now");
        let fill = &doc.get(avatar(&app, rows[0])).unwrap().paint().fills[0];
        assert_eq!(
            fill.brush.literal(),
            Some(&mastik_core::image_brush(id)),
            "the picture beside the file"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// A picture read on open that enters the document before *Fill*, by a list
    /// over the same table or a redo, is not added twice: the fill commits.
    /// flip: `beside_ops` keeping an `AddImage` the document has → fails at
    /// "the fill commits".
    #[test]
    fn a_picture_that_arrived_while_the_box_was_open_is_not_added_twice() {
        use mastik_core::chart::{Column, ColumnId, DataLink, Table};
        let (ctx, mut app) = app();
        let (_, rows) = team(&mut app);
        let dir = std::env::temp_dir().join(format!("mastik-fill-arrived-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let png = crate::app::rgba_to_png(&[0, 0, 255, 255], (1, 1)).unwrap();
        std::fs::write(dir.join("ada.png"), png).unwrap();
        let set = mastik_core::list::DataSet {
            id: app.session.ids.mint_data_set(),
            name: "People".into(),
            table: Table {
                columns: vec![Column {
                    id: ColumnId(0),
                    name: "Photo".into(),
                }],
                rows: vec![vec!["ada.png".into()], vec!["ada.png".into()]],
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
        assert!(app.session.commit(Transaction(vec![Operation::SetDataSet {
            id: data,
            set: Some(Box::new(set)),
        }])));
        pass(&ctx, &mut app);
        app.dispatch(&ctx, crate::input::Action::FillContent);
        pass(&ctx, &mut app);
        let _ = std::fs::remove_dir_all(&dir);
        let mut b = app.fill_content.take().expect("the box opened");
        let avatar = |app: &MastikApp, row: NodeId| {
            let doc = &app.session.doc;
            *doc.get(row)
                .unwrap()
                .children()
                .iter()
                .find(|c| doc.get(**c).unwrap().name() == "Avatar")
                .unwrap()
        };
        for row in &rows {
            b.opts
                .columns
                .insert(avatar(&app, *row), (data, ColumnId(0)));
        }
        b.replan(
            &app.session.doc,
            &app.session.resolved,
            app.session.page,
            app.session.revision(),
        );
        let arrived: Vec<Operation> = b
            .read
            .iter()
            .filter(|op| matches!(op, Operation::AddImage { .. }))
            .cloned()
            .collect();
        assert_eq!(arrived.len(), 1, "the box read the picture");
        assert!(app.session.commit(Transaction(arrived)), "it arrives");
        let before = app.session.history.undo_depth();
        app.commit_fill_content(&b);
        assert_eq!(
            app.session.history.undo_depth(),
            before + 1,
            "the fill commits"
        );
        let doc = &app.session.doc;
        assert!(
            doc.data_set(data)
                .unwrap()
                .picture(doc, "ada.png")
                .is_some(),
            "the table maps it"
        );
    }

    /// A row past the 200 the box reads on open, taken by *From row*, still
    /// finds its picture beside the file: the plan reads the rows it takes.
    /// flip: `replan` not calling `read_taken_rows` → fails at "row 201".
    #[test]
    fn a_picture_past_the_first_200_rows_is_read_when_taken() {
        use mastik_core::chart::{Column, ColumnId, DataLink, Table};
        let (ctx, mut app) = app();
        let (_, rows) = team(&mut app);
        let dir = std::env::temp_dir().join(format!("mastik-fill-past-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let png = crate::app::rgba_to_png(&[255, 0, 0, 255], (1, 1)).unwrap();
        std::fs::write(dir.join("ada.png"), png).unwrap();
        let mut cells: Vec<Vec<String>> = (0..200).map(|_| vec![String::new()]).collect();
        cells.push(vec!["ada.png".into()]);
        let set = mastik_core::list::DataSet {
            id: app.session.ids.mint_data_set(),
            name: "People".into(),
            table: Table {
                columns: vec![Column {
                    id: ColumnId(0),
                    name: "Photo".into(),
                }],
                rows: cells,
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
        assert!(app.session.commit(Transaction(vec![Operation::SetDataSet {
            id: data,
            set: Some(Box::new(set)),
        }])));
        pass(&ctx, &mut app);
        app.dispatch(&ctx, crate::input::Action::FillContent);
        pass(&ctx, &mut app);
        let mut b = app.fill_content.take().expect("the box opened");
        assert!(b.opts.pictures.is_empty(), "row 201 is not read on open");
        let doc = &app.session.doc;
        let avatar = *doc
            .get(rows[0])
            .unwrap()
            .children()
            .iter()
            .find(|c| doc.get(**c).unwrap().name() == "Avatar")
            .unwrap();
        b.opts.columns.insert(avatar, (data, ColumnId(0)));
        b.opts.from_row = 200;
        b.replan(
            &app.session.doc,
            &app.session.resolved,
            app.session.page,
            app.session.revision(),
        );
        let _ = std::fs::remove_dir_all(&dir);
        assert_eq!(b.plan.missing_pictures, 0, "row 201");
        app.commit_fill_content(&b);
        let doc = &app.session.doc;
        let id = (doc.data_set(data).unwrap())
            .picture(doc, "ada.png")
            .expect("the table maps it now");
        let fill = &doc.get(avatar).unwrap().paint().fills[0];
        assert_eq!(fill.brush.literal(), Some(&mastik_core::image_brush(id)));
    }

    /// With nothing fillable selected the box still opens, *Fill* dim with its
    /// reason, and a selection made after fills it; a selection with no text
    /// left dims *Fill* again. Escape closes it.
    #[test]
    fn nothing_to_fill_opens_the_box_and_dims_the_commit() {
        let (ctx, mut app) = app();
        let page = app.session.page;
        let empty = add(&mut app, page, frame(), "Empty");
        app.session.selection.set_one(empty);
        pass(&ctx, &mut app);
        app.open_fill_content();
        pass(&ctx, &mut app);
        let b = app
            .fill_content
            .as_ref()
            .expect("the box opens on anything");
        assert_eq!(b.why_not(), Some(NO_TEXT));

        let label = add(&mut app, page, text("Price"), "Price");
        app.session.selection.set_one(label);
        pass(&ctx, &mut app);
        assert!(app.fill_content.as_ref().unwrap().why_not().is_none());
        app.session.selection.set_one(empty);
        pass(&ctx, &mut app);
        let b = app
            .fill_content
            .as_ref()
            .expect("the box follows the selection");
        assert_eq!(b.why_not(), Some(NO_TEXT));
        app.escape(&ctx);
        assert!(app.fill_content.is_none(), "Escape closes it");
    }

    /// A style picked on one avatar's row draws that avatar in it and leaves the
    /// others in the box's; a fill whose picks agree keeps the style for next time.
    /// flip: the plan's avatar ignoring `Options::styles` → fails at "the picked one".
    #[test]
    fn an_avatars_style_is_picked_on_its_row() {
        let (ctx, mut app) = app();
        team(&mut app);
        pass(&ctx, &mut app);
        app.open_fill_content();
        pass(&ctx, &mut app);
        let b = app.fill_content.as_mut().unwrap();
        let avatars: Vec<NodeId> = b
            .plan
            .rows
            .iter()
            .filter(|r| r.shape)
            .map(|r| r.node)
            .collect();
        assert_eq!(avatars.len(), 2, "the fixture: two avatars");
        b.opts.styles.insert(avatars[0], AvatarStyle::Pattern);
        pass(&ctx, &mut app);
        let b = app.fill_content.as_ref().unwrap();
        let style = |id: NodeId| match &b.plan.rows.iter().find(|r| r.node == id).unwrap().value {
            Value::Avatar(a) => Some(a.style),
            _ => None,
        };
        assert_eq!(
            style(avatars[0]),
            Some(AvatarStyle::Pattern),
            "the picked one"
        );
        assert_eq!(style(avatars[1]), Some(AvatarStyle::Initials), "the rest");
        let b = app.fill_content.take().unwrap();
        app.commit_fill_content(&b);
        assert_eq!(app.prefs.fill_content.avatar_style, "pattern");
    }

    /// A kind chosen for a row wins over its name; choosing one kind for all in
    /// *Content* lets the text rows' own choices go and keeps a shape's.
    #[test]
    fn a_rows_kind_wins_until_content_names_one_for_all() {
        let (ctx, mut app) = app();
        let (_, rows) = team(&mut app);
        pass(&ctx, &mut app);
        app.open_fill_content();
        pass(&ctx, &mut app);
        let b = app.fill_content.as_mut().unwrap();
        let name = b
            .plan
            .rows
            .iter()
            .find(|r| r.kind == Some(Kind::FullName))
            .unwrap()
            .node;
        let avatar = b.plan.rows.iter().find(|r| r.shape).unwrap().node;
        b.opts.kinds.insert(name, Some(Kind::City));
        b.opts.kinds.insert(avatar, None);
        pass(&ctx, &mut app);
        let b = app.fill_content.as_ref().unwrap();
        let kind_of = |b: &FillBox, id| b.plan.rows.iter().find(|r| r.node == id).unwrap().kind;
        assert_eq!(kind_of(b, name), Some(Kind::City));
        assert_eq!(kind_of(b, avatar), None, "the avatar left as is");

        let b = app.fill_content.as_mut().unwrap();
        b.opts.content = Some(Kind::Email);
        b.opts
            .kinds
            .retain(|id, _| b.plan.rows.iter().any(|r| r.node == *id && r.shape));
        pass(&ctx, &mut app);
        let b = app.fill_content.as_ref().unwrap();
        assert_eq!(kind_of(b, name), Some(Kind::Email), "one kind for all");
        assert_eq!(kind_of(b, avatar), None, "a shape keeps its own");
        assert_eq!(rows.len(), 2);
    }

    /// An initials avatar's picture is its ground with white initials over its
    /// middle; a pattern's has no white.
    #[test]
    fn an_avatar_draws_white_initials_on_its_ground() {
        let avatar = |style| fill::Avatar {
            name: "Maya Okafor".into(),
            initials: "MO".into(),
            style,
        };
        let pixels = |style| {
            let png =
                crate::canvas::art_png(64, 64, &avatar(style).art(64.0, 64.0)).expect("drawn");
            let data = mastik_render::images::decode(&png, mastik_core::ImageFormat::Png)
                .expect("decodes");
            data.data.data().to_vec()
        };
        let white = |rgba: &[u8]| {
            rgba.chunks(4)
                .filter(|p| p[0] > 240 && p[1] > 240 && p[2] > 240)
                .count()
        };
        let initials = pixels(AvatarStyle::Initials);
        let [r, g, b, _] = avatar(AvatarStyle::Initials)
            .ground()
            .to_rgba8()
            .to_u8_array();
        assert_eq!(&initials[..3], &[r, g, b], "the corner is the ground");
        assert!(white(&initials) > 100, "the initials are inked");
        assert_eq!(white(&pixels(AvatarStyle::Pattern)), 0);
    }

    /// The canvas stays live under the box, and an edit made with it open is in
    /// its next plan.
    /// flip: `FillBox::replan` leaving the revision out of its key → fails at
    /// "the new layer is listed".
    #[test]
    fn the_box_follows_the_document() {
        let (ctx, mut app) = app();
        let (list, _) = team(&mut app);
        pass(&ctx, &mut app);
        app.open_fill_content();
        pass(&ctx, &mut app);
        assert!(!app.modal_is_up(), "the canvas keeps its input");
        let before = app.fill_content.as_ref().unwrap().plan.rows.len();
        add(&mut app, list, text("Total"), "Total");
        pass(&ctx, &mut app);
        let after = app.fill_content.as_ref().unwrap().plan.rows.len();
        assert_eq!(after, before + 1, "the new layer is listed");
    }

    /// Hovering a preview row, which shows its die, moves no row under it.
    /// flip: place the die with `ui.put` in `fill_row` → fails at "rows hold still".
    #[test]
    fn hovering_a_row_moves_no_other_row() {
        let (ctx, mut app) = app();
        team(&mut app);
        pass(&ctx, &mut app);
        app.open_fill_content();
        // Each preview row's layer path and where it was drawn.
        let run = |ctx: &egui::Context, app: &mut MastikApp, at: Option<egui::Pos2>| {
            let mut frame = eframe::Frame::_new_kittest();
            let input = egui::RawInput {
                screen_rect: Some(egui::Rect::from_min_size(
                    egui::Pos2::ZERO,
                    egui::vec2(1320.0, 820.0),
                )),
                events: at.map(egui::Event::PointerMoved).into_iter().collect(),
                ..Default::default()
            };
            let out = ctx.run_ui(input, |ui| eframe::App::ui(app, ui, &mut frame));
            out.shapes
                .iter()
                .filter_map(|s| match &s.shape {
                    egui::epaint::Shape::Text(t) if t.galley.text().starts_with("Card ") => {
                        Some((t.galley.text().to_string(), t.pos))
                    }
                    _ => None,
                })
                .collect::<Vec<_>>()
        };
        for _ in 0..3 {
            run(&ctx, &mut app, None);
        }
        let before = run(&ctx, &mut app, None);
        let (_, email) = before
            .iter()
            .find(|(t, _)| t == "Card 1 / Email")
            .expect("the fixture: Card 1's email is listed")
            .clone();
        let mut after = Vec::new();
        for _ in 0..4 {
            after = run(&ctx, &mut app, Some(email + egui::vec2(5.0, 6.0)));
        }
        assert!(
            app.fill_content.as_ref().unwrap().hover.0.len() == 1,
            "the fixture: the row is hovered"
        );
        assert_eq!(before, after, "rows hold still");
    }

    /// *Fill* ends a live text session first, so the session's buffer, which
    /// predates the fill, does not write over the value filled when it ends.
    /// flip: `run_fill_content` without `finish_text_first` → fails at "the fill
    /// stands".
    #[test]
    fn fill_ends_a_live_text_session_first() {
        let (ctx, mut app) = app();
        let page = app.session.page;
        let name = add(&mut app, page, text("Name"), "Name");
        app.session.selection.set_one(name);
        pass(&ctx, &mut app);
        app.open_fill_content();
        pass(&ctx, &mut app);
        app.begin_edit_text(Some(name), None);
        app.text.as_mut().expect("editing").editor.insert("X");
        let depth = app.session.history.undo_depth();
        let b = app.fill_content.take().expect("the box is open");
        app.run_fill_content(b);
        let filled = content(&app, name);
        assert!(
            !filled.contains('X'),
            "fixture: filled over the typing: {filled}"
        );
        app.finish_text_first();
        assert_eq!(content(&app, name), filled, "the fill stands");
        assert_eq!(
            app.session.history.undo_depth(),
            depth + 2,
            "the typing, then the fill"
        );
    }

    /// Opened on nothing, the box fills a selection made after in the locale its
    /// text agrees on; a locale picked in the row holds over later selections.
    /// flip: `FillBox::replan` not reading `agreed_locale` again → fails at
    /// "German text fills in German".
    #[test]
    fn the_locale_follows_the_selection_until_one_is_picked() {
        let (ctx, mut app) = app();
        app.prefs.fill_content.locale = "en-US".into();
        let page = app.session.page;
        let in_lang = |tag: &str| {
            let NodeKind::Text {
                content,
                spans,
                para_spans,
                paragraph,
                block,
                sizing,
                ..
            } = text("Name")
            else {
                unreachable!()
            };
            NodeKind::Text {
                content,
                style: Box::new(TextStyle {
                    locale: Some(tag.into()),
                    ..Default::default()
                }),
                spans,
                para_spans,
                paragraph,
                block,
                sizing,
                on_path: None,
                on_path_flip: false,
                on_path_offset: 0.0,
            }
        };
        let de = add(&mut app, page, in_lang("de"), "Name");
        let fr = add(&mut app, page, in_lang("fr"), "Name");
        app.session.selection.clear();
        pass(&ctx, &mut app);
        app.open_fill_content();
        pass(&ctx, &mut app);
        let locale = |app: &MastikApp| app.fill_content.as_ref().unwrap().opts.locale.clone();
        assert_eq!(
            locale(&app),
            "en-US",
            "the last used, with nothing selected"
        );
        app.session.selection.set_one(de);
        pass(&ctx, &mut app);
        assert_eq!(locale(&app), "de", "German text fills in German");
        app.session.selection.set_one(fr);
        pass(&ctx, &mut app);
        assert_eq!(locale(&app), "fr");
        let b = app.fill_content.as_mut().unwrap();
        b.opts.locale = "it".into();
        b.locale_picked = true;
        app.session.selection.set_one(de);
        pass(&ctx, &mut app);
        assert_eq!(locale(&app), "it", "a pick holds");
    }

    /// In private mode the records are counted as *records*, not by the frames'
    /// name.
    /// flip: `records_label` asking `Plan::record_noun` itself → fails at "2
    /// records".
    #[test]
    fn private_mode_counts_records_by_no_name() {
        let (ctx, mut app) = app();
        team(&mut app);
        pass(&ctx, &mut app);
        app.open_fill_content();
        pass(&ctx, &mut app);
        let b = app.fill_content.as_ref().unwrap();
        crate::private::set(true);
        let label = records_label(&b.plan, &app.session.doc);
        crate::private::set(false);
        assert_eq!(label.as_deref(), Some("2 records"));
        assert_eq!(
            records_label(&b.plan, &app.session.doc).as_deref(),
            Some("2 cards"),
            "control: named while private mode is off"
        );
    }
}
