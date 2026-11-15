//! Command Mode's command bar (`docs/design/command-mode.md`): a sibling of the
//! palette's card docked at the canvas's foot, with two focuses. In Type (`:`)
//! its field takes a line that runs against the selection in the scripting
//! language: a write commits one undo step, a read shows its value as it is
//! typed, and the log above the field lists past lines and every other edit as
//! the command that would make it. In Pick (`` ` ``, `crate::navigate`) the
//! canvas has the keys and leap labels select. The canvas stays live under the
//! card either way: the mouse pans, zooms and selects, and never takes the keys.

pub(crate) mod code_keys;
pub(crate) mod editor;
pub(crate) mod host;
pub(crate) mod log;
pub(crate) mod suggest;

use crate::app::MastikApp;
use crate::palette::{
    self, FIELD_H, FOOTER_H, FOOTER_INSET, FooterLine, PAD, ROW_GAP, ROW_H, SECTION_H,
};
use crate::theme::{self, color, icon};
use eframe::egui;
use mastik_core::NodeId;
use suggest::{Suggest, Suggester};

/// The card's width.
const W: f32 = 640.0;
/// Between the card and the canvas edge it docks to.
const DOCK: f32 = 24.0;
/// The most completion rows the list shows.
const LIST_ROWS: usize = 8;
/// The log lines the list shows while the field is empty.
const LOG_ROWS: usize = 6;
/// Between a read's value and the field's right edge.
const VALUE_INSET: f32 = 14.0;
/// Between the line and a read's value.
const VALUE_GAP: f32 = 10.0;
/// The head row: a control's height in the card's inset.
const HEAD_H: f32 = crate::ui::CONTROL_H + 2.0 * PAD;
/// Between the head row's parts: the focus toggle, the path, the subject.
const HEAD_GAP: f32 = 12.0;
/// Between a pill in the field and the text beside it.
const PILL_GAP: f32 = 2.0;
/// How strongly the field's pills draw while the bar picks and the line sleeps.
const ASLEEP: f32 = 0.5;

/// What the field draws as pills. A leap label named in the line is a pill in
/// the label's own letters, so the line reads as the canvas shows it; one that
/// opens the line is its subject ([`MastikApp::line_subject`]). Before each `.`
/// rule 2 reads as the subject, the subject's pill is drawn too, so a bare `.x`
/// says whose `x` it is.
struct Pills {
    /// The subject's letters (`SK`, `SK +2`) and what it is, for its tooltip.
    subject: Option<(String, String)>,
    /// The line's subject: a label naming one of these wears the subject's yellow.
    subject_ids: Vec<NodeId>,
    /// The labels a line may name.
    labels: rustc_hash::FxHashMap<String, NodeId>,
    /// The names of the labels' layers, for their tooltips.
    names: rustc_hash::FxHashMap<NodeId, String>,
    /// The first byte of the word the completion list is open on: still being
    /// typed, so not yet a pill.
    typing: Option<usize>,
}

impl Pills {
    /// The labels `text` names, by range, but the word being typed.
    fn named(&self, text: &str) -> Vec<(std::ops::Range<usize>, NodeId)> {
        mastik_vm::marks(text)
            .names
            .into_iter()
            .filter(|r| Some(r.start) != self.typing)
            .filter_map(|r| Some((r.clone(), *self.labels.get(&text[r])?)))
            .collect()
    }

    /// Each layer `text` names, with its hue: the subject's yellow for a
    /// selected layer, else the next of `color::REFERENCE` in order of first
    /// mention, so one layer keeps one hue however often it is named.
    fn hues(&self, text: &str) -> Vec<(NodeId, egui::Color32)> {
        let mut out: Vec<(NodeId, egui::Color32)> = Vec::new();
        let mut next = 0;
        for (_, id) in self.named(text) {
            if out.iter().any(|(o, _)| *o == id) {
                continue;
            }
            let hue = match self.subject_ids.contains(&id) {
                true => color::leap(),
                false => {
                    next += 1;
                    color::REFERENCE[(next - 1) % color::REFERENCE.len()]
                }
            };
            out.push((id, hue));
        }
        out
    }

    /// The dots that read as the subject, when there is one.
    fn subject_dots(&self, text: &str) -> Vec<usize> {
        match self.subject {
            Some(_) => mastik_vm::marks(text).subject,
            None => Vec::new(),
        }
    }
}

/// A pill's size for `letters`: a leap label's, as the canvas draws it.
fn pill_size(painter: &egui::Painter, letters: &str) -> egui::Vec2 {
    let font = egui::FontId::proportional(crate::ui::BADGE_PT);
    painter
        .layout_no_wrap(letters.to_owned(), font, egui::Color32::PLACEHOLDER)
        .size()
        + crate::navigate::LABEL_PAD * 2.0
}

/// One character's advance in the field's monospace face.
fn mono_advance(painter: &egui::Painter) -> f32 {
    painter
        .layout_no_wrap("m".to_owned(), mono(), egui::Color32::PLACEHOLDER)
        .size()
        .x
}

/// The field's galley: the line in `ink`; each label's letters hidden and laid
/// out wide enough to span its pill exactly, so the caret at either end of a
/// label stands at the pill's edge rather than inside it; room before each
/// subject dot for the subject's pill. Every run keeps the plain line's height.
fn pill_galley(
    ui: &egui::Ui,
    text: &str,
    pills: &Pills,
    ink: egui::Color32,
) -> std::sync::Arc<egui::Galley> {
    let p = ui.painter();
    let advance = mono_advance(p);
    let row_h = p
        .layout_no_wrap("m".to_owned(), mono(), egui::Color32::PLACEHOLDER)
        .size()
        .y;
    // (range, room before, room after, the hidden letters' scale)
    let mut pieces: Vec<(std::ops::Range<usize>, f32, f32, Option<f32>)> = pills
        .named(text)
        .into_iter()
        .map(|(r, _)| {
            let label = &text[r.clone()];
            let span = label.chars().count() as f32 * advance;
            let scale = (pill_size(p, &label.to_uppercase()).x / span).max(1.0);
            (r, PILL_GAP, PILL_GAP, Some(scale))
        })
        .collect();
    if let Some((letters, _)) = &pills.subject {
        let room = PILL_GAP + pill_size(p, letters).x + 1.0;
        pieces.extend(
            pills
                .subject_dots(text)
                .into_iter()
                .map(|d| (d..d + 1, room, 0.0, None)),
        );
    }
    pieces.sort_by_key(|(r, ..)| r.start);
    let format = |hidden: Option<f32>| {
        let (size, color) = match hidden {
            Some(scale) => (crate::ui::HEAD_PT * scale, egui::Color32::TRANSPARENT),
            None => (crate::ui::HEAD_PT, ink),
        };
        egui::TextFormat {
            font_id: egui::FontId::monospace(size),
            color,
            line_height: Some(row_h),
            valign: egui::Align::Center,
            ..Default::default()
        }
    };
    let mut job = egui::text::LayoutJob::default();
    let (mut at, mut pending) = (0, 0.0);
    for (r, before, after, hidden) in pieces {
        if r.start > at {
            job.append(&text[at..r.start], pending, format(None));
            pending = 0.0;
        }
        job.append(&text[r.clone()], pending + before, format(hidden));
        pending = after;
        at = r.end;
    }
    if at < text.len() {
        job.append(&text[at..], pending, format(None));
    }
    job.wrap.max_width = f32::INFINITY;
    ui.fonts_mut(|f| f.layout_job(job))
}

/// The open bar.
#[derive(Default)]
pub(crate) struct Bar {
    pub(crate) text: String,
    /// How far back `↑` has recalled, 0 for the newest typed line; `None` while
    /// the text is the person's own.
    recall: Option<usize>,
    /// The text a recall put in the field, so the footer says so until it changes.
    recalled: Option<String>,
    /// What the person had typed when `↑` first recalled, given back when `↓`
    /// walks past the newest line.
    draft: Option<String>,
    /// The last error, with the text it was for: an edit clears it.
    error: Option<(String, mastik_vm::VmError)>,
    /// The live read: the text and revision it was taken for, and its value.
    read: Option<(String, u64, Option<String>)>,
    /// The log rows picked for *Copy as script*: first and last, by entry index.
    picked: Option<(usize, usize)>,
    /// The log's context menu, where it opened.
    log_menu: Option<egui::Pos2>,
    /// Put the caret at the end of the field next frame, after a recall or a
    /// completion wrote the text.
    caret_to: Option<usize>,
    /// The completion list as last worked out ([`MastikApp::bar_completion`]).
    completed: Option<Completed>,
    /// Whether the completion list is up, its lit row, and the text as the
    /// field left it last frame.
    suggest: Suggester,
    /// The layer the lit completion row names, which the canvas outlines.
    lit: Option<NodeId>,
    /// A word to write at the caret next frame ([`Bar::write_at_caret`]).
    insert: Option<String>,
    /// The layers the line names other than its subject, with the hue each
    /// wears in the field, for the canvas to outline in it.
    references: Vec<(NodeId, egui::Color32)>,
    /// The pick in progress, writing its current layer into the line.
    live: Option<Live>,
}

/// A pick writing into the line as it goes ([`MastikApp::pick_into_line`]).
struct Live {
    /// Where it writes: the caret when the pick began.
    at: usize,
    /// The label it wrote, once the pick has moved.
    wrote: Option<std::ops::Range<usize>>,
    /// The selection when the pick began: it writes once this changes.
    from: Vec<NodeId>,
}

impl Bar {
    /// The layer the lit completion row names (a label's, or a layer's by
    /// name), for the canvas to outline as it outlines a hovered Layers row.
    pub(crate) fn lit_layer(&self) -> Option<NodeId> {
        self.lit
    }

    /// The layers the line names other than the selection, each with its pill's
    /// hue, for the canvas to outline in that hue.
    pub(crate) fn references(&self) -> &[(NodeId, egui::Color32)] {
        &self.references
    }

    /// The letters typed of a leap label, lowercase, while the list offers
    /// labels for the word at the caret: the canvas shows the labels then,
    /// narrowed by these letters as Pick's first letter narrows them.
    pub(crate) fn labels_typed(&self) -> Option<String> {
        if !self.suggest.on() || self.error.is_some() {
            return None;
        }
        let c = self.completed.as_ref()?;
        let offers = c
            .completion
            .rows
            .iter()
            .any(|r| r.section == mastik_vm::Section::Labels);
        if c.key.0 != self.text || !offers {
            return None;
        }
        let word = c.key.0.get(c.completion.replace.start..c.key.1)?;
        let word = word.to_lowercase();
        let home = word.len() <= 2 && word.chars().all(crate::navigate::is_home_letter);
        home.then_some(word)
    }

    /// Write `word` into the line at the caret as the bar next draws, a space
    /// before it when it would run into the word there: how an `Alt`-click on the
    /// canvas names a layer by its label.
    pub(crate) fn write_at_caret(&mut self, word: String) {
        self.insert = Some(word);
    }

    /// Take completion row `i` into the field, and shut the list.
    fn accept(&mut self, c: &mastik_vm::Completion, i: usize) {
        if let Some(at) = self.suggest.accept(&mut self.text, c, i) {
            self.caret_to = Some(at);
        }
    }
}

/// A completion list and what it was worked out for — the text, the caret, the
/// document's revision and the page — with each shown row's layer.
struct Completed {
    key: (String, usize, u64, NodeId),
    completion: mastik_vm::Completion,
    layers: Vec<Option<NodeId>>,
}

/// This frame's bar keys, taken out of `Events` before the field can read them.
#[derive(Default, Clone, Copy)]
pub(crate) struct Keys {
    up: bool,
    down: bool,
    enter: bool,
    tab: bool,
    escape: bool,
    /// `Ctrl+Space`: open the completion list on the word at the caret.
    suggest: bool,
    /// `` ` `` outside a string: give the keys to the canvas (Pick).
    pick: bool,
}

/// Take the keys the bar's Type focus answers, `text` being its line. Called
/// before any panel draws.
pub(crate) fn read_keys(ctx: &egui::Context, text: &str) -> Keys {
    let none = egui::Modifiers::NONE;
    // Inside a string the backtick is Luau's own, an interpolated string's quote.
    let caret = caret_of(ctx, text);
    let quoted = in_string(&text[..caret]);
    ctx.input_mut(|i| {
        let pick = !quoted && i.consume_key(none, egui::Key::Backtick);
        if pick {
            i.events
                .retain(|e| !matches!(e, egui::Event::Text(t) if t == "`"));
        }
        Keys {
            up: i.consume_key(none, egui::Key::ArrowUp),
            down: i.consume_key(none, egui::Key::ArrowDown),
            enter: i.consume_key(none, egui::Key::Enter),
            tab: i.consume_key(none, egui::Key::Tab),
            escape: i.consume_key(none, egui::Key::Escape),
            // Ctrl on every platform, as the editors this follows bind it.
            suggest: i.consume_key(egui::Modifiers::CTRL, egui::Key::Space),
            pick,
        }
    })
}

/// The caret's byte offset in the field's `text`, its end when the field has
/// none yet.
fn caret_of(ctx: &egui::Context, text: &str) -> usize {
    egui::TextEdit::load_state(ctx, field_id())
        .and_then(|s| s.cursor.char_range())
        .map(|r| char_to_byte(text, r.primary.index.0))
        .unwrap_or(text.len())
}

/// Whether `before` ends inside a Luau string: an open `"`, `'` or `` ` ``, a
/// backslash escaping the next character.
fn in_string(before: &str) -> bool {
    let mut open: Option<char> = None;
    let mut escaped = false;
    for c in before.chars() {
        match open {
            Some(_) if escaped => escaped = false,
            Some(_) if c == '\\' => escaped = true,
            Some(q) if c == q => open = None,
            Some(_) => {}
            None if matches!(c, '"' | '\'' | '`') => open = Some(c),
            None => {}
        }
    }
    open.is_some()
}

/// What the list above the field shows.
enum List {
    None,
    Log,
    Complete(mastik_vm::Completion),
}

impl MastikApp {
    /// `:` — open the bar on its Type focus, closing whatever else floats over
    /// the canvas. Labels are dealt as it draws, so a line can name what it sees.
    pub(crate) fn open_command_bar(&mut self) {
        if self.command_bar.is_some() {
            return;
        }
        self.palette = None;
        self.context_menu = None;
        self.open_menu = crate::app::TopMenu::None;
        self.command_bar = Some(Bar::default());
        self.begin_bar_session();
    }

    /// What `line` acts on, its subject: the layer of the label that opens it,
    /// when one does (the first pick writes it there), else the selection. So a
    /// bare `.x` means the line's first pill however the selection moves later.
    fn line_subject(&self, line: &str) -> Vec<NodeId> {
        let lead = line.len() - line.trim_start().len();
        let first = mastik_vm::marks(line)
            .names
            .into_iter()
            .next()
            .filter(|r| r.start == lead)
            .and_then(|r| self.leap_labels().get(&line[r]).copied())
            .filter(|id| self.session.doc.contains(*id));
        match first {
            Some(id) => vec![id],
            None => self.session.selection.ids().to_vec(),
        }
    }

    /// `Card and 2 more`: the first of `ids` by name and how many others, or
    /// that nothing is selected.
    fn bar_subject(&self, ids: &[NodeId]) -> String {
        let Some(first) = ids.first().and_then(|id| self.session.doc.get(*id)) else {
            return "Nothing selected".into();
        };
        match ids.len() {
            1 => first.name().to_string(),
            n => format!("{} and {} more", first.name(), n - 1),
        }
    }

    /// The subject's pill for `ids`: the first one's label, given one when it has
    /// none, and how many more there are (`SK +2`), with what it is for its
    /// tooltip. `None` with nothing to act on.
    fn subject_pill(&mut self, ids: &[NodeId]) -> Option<(String, String)> {
        let first = *ids.first()?;
        let label = self.label_for(first)?.to_uppercase();
        let letters = match ids.len() - 1 {
            0 => label,
            n => format!("{label} +{n}"),
        };
        Some((letters, self.bar_subject(ids)))
    }

    /// While the bar picks, write the current layer into the line as the pick
    /// goes: once the selection has moved off what it was when the pick began,
    /// its label stands at the caret the pick began from, replaced as the pick
    /// moves on. `` ` `` ends the pick with it there.
    fn pick_into_line(&mut self, bar: &mut Bar, picking: bool, caret: usize) {
        if !picking {
            if let Some(live) = bar.live.take()
                && let Some(r) = live.wrote
            {
                bar.caret_to = Some(r.end);
            }
            return;
        }
        let selection = self.session.selection.ids().to_vec();
        let live = bar.live.get_or_insert_with(|| Live {
            at: caret.min(bar.text.len()),
            wrote: None,
            from: selection.clone(),
        });
        if live.wrote.is_none() && selection == live.from {
            return;
        }
        let Some(current) = self.nav_current() else {
            return;
        };
        let Some(label) = self.label_for(current) else {
            return;
        };
        let range = match live.wrote.clone() {
            Some(r) if bar.text.get(r.clone()) == Some(label.as_str()) => return,
            Some(r) if r.end <= bar.text.len() => r,
            _ => {
                // A space first when the label would run into the word before.
                let runs_on = bar.text[..live.at]
                    .chars()
                    .next_back()
                    .is_some_and(|c| c.is_alphanumeric() || matches!(c, '_' | ')' | ']'));
                if runs_on {
                    bar.text.insert(live.at, ' ');
                    live.at += 1;
                }
                live.at..live.at
            }
        };
        bar.text.replace_range(range.clone(), &label);
        live.wrote = Some(range.start..range.start + label.len());
        // Written, not typed: the list does not open on it.
        bar.suggest.seen = bar.text.clone();
    }

    /// The leap labels lines may name, as the bar last dealt them.
    fn bar_labels(&self) -> rustc_hash::FxHashMap<String, NodeId> {
        self.leap_labels()
            .iter()
            .map(|(k, v)| (k.clone(), *v))
            .collect()
    }

    /// The layer each shown completion row names, for its kind glyph: a label's
    /// layer, or the first on the page by that name. One walk of the page, and
    /// only when a row names a layer.
    fn completion_layers(&self, rows: &[mastik_vm::Row]) -> Vec<Option<NodeId>> {
        use mastik_vm::Section;
        let doc = &self.session.doc;
        let labels = self.leap_labels();
        let wanted: rustc_hash::FxHashSet<&str> = rows
            .iter()
            .filter(|r| r.section == Section::Layers)
            .map(|r| r.label.as_str())
            .collect();
        let mut by_name: rustc_hash::FxHashMap<&str, NodeId> = Default::default();
        if !wanted.is_empty() {
            for id in mastik_core::build::subtree_nodes(doc, &[self.session.page]) {
                if let Some(name) = doc.get(id).map(|n| n.name())
                    && wanted.contains(name)
                {
                    by_name.entry(name).or_insert(id);
                }
            }
        }
        rows.iter()
            .map(|row| match row.section {
                Section::Labels => labels.get(row.label.as_str()).copied(),
                Section::Layers => by_name.get(row.label.as_str()).copied(),
                _ => None,
            })
            .collect()
    }

    /// What to complete at `caret` in `bar`'s text, worked out again only when
    /// the text, the caret, the document or the page has changed since.
    fn bar_completion(&self, bar: &mut Bar, caret: usize) -> mastik_vm::Completion {
        let key = (
            bar.text.clone(),
            caret,
            self.session.revision(),
            self.session.page,
        );
        if let Some(c) = &bar.completed
            && c.key == key
        {
            return c.completion.clone();
        }
        let labels = self.bar_labels();
        let completion = mastik_vm::complete(
            &self.session.doc,
            self.session.page,
            &labels,
            &bar.text,
            caret,
        );
        let shown = &completion.rows[..completion.rows.len().min(LIST_ROWS)];
        bar.completed = Some(Completed {
            key,
            layers: self.completion_layers(shown),
            completion: completion.clone(),
        });
        completion
    }

    /// Run `line`: a read copies its value, a write commits one undo step named
    /// after its verb or *Edit*. Answers the error that leaves the line in the
    /// field, or `None` when it ran.
    pub(crate) fn run_bar_line(
        &mut self,
        ctx: &egui::Context,
        line: &str,
    ) -> Option<mastik_vm::VmError> {
        let parsed = mastik_vm::desugar(line);
        let selection = self.line_subject(line);
        let labels = self.bar_labels();
        let env = mastik_vm::Env {
            selection: &selection,
            labels: &labels,
            params: Default::default(),
        };
        if parsed.kind == mastik_vm::LineKind::Read {
            let mut h = host::SessionHost {
                s: &mut self.session,
                read_only: true,
            };
            // Enter runs a read under the run budget, through the read-only host:
            // the live read's budget is for every keystroke, not for Enter.
            return match mastik_vm::run_line(&mut h, &env, line).map(|o| o.value) {
                Ok(value) => {
                    if let Some(v) = &value {
                        ctx.copy_text(v.clone());
                    }
                    let result = value.map(|v| format!("= {v}")).unwrap_or_default();
                    self.session.command_log.push(log::Entry::typed(
                        line,
                        parsed.luau,
                        result,
                        false,
                        None,
                    ));
                    None
                }
                Err(e) => Some(e),
            };
        }
        let label = verb_named(line).unwrap_or("Edit");
        let ran = host::run_as_step(&mut self.session, label, icon::TERMINAL_WINDOW, |h| {
            mastik_vm::run_line(h, &env, line)
        });
        match ran {
            Ok((outcome, counts, serial)) => {
                let result = outcome
                    .printed
                    .last()
                    .cloned()
                    .or_else(|| counts.said())
                    .unwrap_or_else(|| "Nothing changed".into());
                if let Some(ids) = outcome.selection {
                    self.session.selection.set(ids);
                }
                self.session.command_log.push(log::Entry::typed(
                    line,
                    parsed.luau,
                    result,
                    false,
                    serial,
                ));
                None
            }
            Err(e) => Some(e),
        }
    }

    /// `.` in Pick: run the last line in the log again on the selection, a line
    /// typed or one an edit made elsewhere was written as (a drag's `.x += 24`).
    pub(crate) fn repeat_bar_line(&mut self, ctx: &egui::Context) {
        let Some(line) = self.session.command_log.last_line().map(str::to_owned) else {
            self.session.info("No line to repeat yet");
            return;
        };
        if let Some(e) = self.run_bar_line(ctx, &line) {
            self.session.fail(e.message);
        }
    }

    /// The live read for the bar's text: taken again when the text or the
    /// document moved, never for a write.
    fn bar_read(&mut self, bar: &mut Bar) {
        let text = bar.text.trim().to_owned();
        let rev = self.session.revision();
        if bar
            .read
            .as_ref()
            .is_some_and(|(t, r, _)| *t == text && *r == rev)
        {
            return;
        }
        let value = (!text.is_empty()
            && mastik_vm::desugar(&text).kind == mastik_vm::LineKind::Read)
            .then(|| {
                let selection = self.line_subject(&text);
                let labels = self.bar_labels();
                let env = mastik_vm::Env {
                    selection: &selection,
                    labels: &labels,
                    params: Default::default(),
                };
                let mut h = host::SessionHost {
                    s: &mut self.session,
                    read_only: true,
                };
                // While a read is incomplete or raises, the edge stays empty.
                mastik_vm::eval_read(&mut h, &env, &text).ok().flatten()
            })
            .flatten();
        bar.read = Some((text, rev, value));
    }

    /// Draw the open bar and act on this frame's keys.
    pub(crate) fn command_bar_ui(&mut self, ctx: &egui::Context, canvas: egui::Rect, keys: Keys) {
        // Before the bar is taken out: the deal reads the line, for the labels it holds.
        self.deal_tick(ctx);
        let Some(mut bar) = self.command_bar.take() else {
            // Closed elsewhere (a tab switch): the session ends with it.
            self.end_bar_session();
            return;
        };
        if self.modal_is_up() {
            self.command_bar = Some(bar);
            return;
        }
        if keys.pick {
            self.set_picking(true);
            bar.suggest.dismiss();
        }
        let picking = self.pick_focus();
        // An edit of the text drops the error it was for and the recall.
        if bar.error.as_ref().is_some_and(|(t, _)| *t != bar.text) {
            bar.error = None;
        }
        // An edited recall is the person's own line now; the draft goes with it.
        if bar.recalled.as_ref().is_some_and(|t| *t != bar.text) {
            bar.recalled = None;
            bar.recall = None;
            bar.draft = None;
        }
        self.bar_read(&mut bar);

        let mut caret = caret_of(ctx, &bar.text);
        if let Some(word) = bar.insert.take() {
            let runs_on = bar.text[..caret]
                .chars()
                .next_back()
                .is_some_and(|c| c.is_alphanumeric() || matches!(c, '_' | ')' | ']' | '"' | '\''));
            let word = match runs_on {
                true => format!(" {word}"),
                false => word,
            };
            bar.text.insert_str(caret, &word);
            caret += word.len();
            bar.caret_to = Some(caret);
            // Written, not typed: the list does not open on it.
            bar.suggest.seen = bar.text.clone();
        }
        self.pick_into_line(&mut bar, picking, caret);
        // The subject's pill first: it may give the subject a label.
        let subject_ids = self.line_subject(&bar.text);
        let subject = self.subject_pill(&subject_ids);
        let mut pills = Pills {
            subject,
            subject_ids,
            labels: self.bar_labels(),
            names: Default::default(),
            typing: match bar.suggest.state {
                Suggest::On(s) => Some(s),
                _ => None,
            },
        };
        pills.names = pills
            .labels
            .values()
            .map(|id| {
                (
                    *id,
                    crate::private::layer(&self.session.doc, *id).into_owned(),
                )
            })
            .collect();
        // `Backspace` just after a pill takes the whole label, as one thing.
        let backspace =
            !picking && ctx.input(|i| i.key_pressed(egui::Key::Backspace) && i.modifiers.is_none());
        if backspace
            && let Some((r, _)) = pills
                .named(&bar.text)
                .into_iter()
                .find(|(r, _)| r.end == caret)
        {
            ctx.input_mut(|i| i.consume_key(egui::Modifiers::NONE, egui::Key::Backspace));
            bar.text.replace_range(r.clone(), "");
            caret = r.start;
            bar.caret_to = Some(caret);
        }
        let completion = (!picking && !bar.text.trim().is_empty() && bar.recalled.is_none())
            .then(|| self.bar_completion(&mut bar, caret));
        bar.suggest
            .follow(&bar.text, completion.as_ref(), caret, keys.suggest);
        let suggesting = bar.suggest.on()
            && bar.error.is_none()
            && completion.as_ref().is_some_and(|c| !c.rows.is_empty());

        // Escape closes the innermost thing first: the log's menu, the
        // completion list, then the text, then the bar.
        if keys.escape {
            if bar.log_menu.is_some() {
                bar.log_menu = None;
            } else if suggesting {
                bar.suggest.dismiss();
            } else if bar.text.is_empty() {
                self.end_bar_session();
                return;
            } else {
                bar = Bar::default();
            }
        }
        let list = match completion {
            // Picking keeps the canvas clear: the field sleeps and lists nothing.
            _ if picking => List::None,
            _ if bar.text.trim().is_empty() || bar.recalled.is_some() => {
                match self.session.command_log.entries().len() {
                    0 => List::None,
                    _ => List::Log,
                }
            }
            Some(c) if suggesting => List::Complete(c),
            _ => List::None,
        };
        let rows = match &list {
            List::Complete(c) => c.rows.len().min(LIST_ROWS),
            _ => 0,
        };
        if rows > 0 {
            bar.suggest.highlight = bar.suggest.highlight.min(rows - 1);
        }
        let lit_of = |bar: &Bar| match &list {
            List::Complete(_) => bar
                .completed
                .as_ref()
                .and_then(|c| c.layers.get(bar.suggest.highlight).copied().flatten()),
            _ => None,
        };

        // Keys: a list of completions takes the arrows, Tab and Enter, as an
        // editor's does; otherwise the arrows walk the typed lines, Tab takes an
        // error's fix and Enter runs.
        let mut accepted = false;
        if let List::Complete(c) = &list {
            if keys.down {
                bar.suggest.step(rows, false);
            }
            if keys.up {
                bar.suggest.step(rows, true);
            }
            // Enter takes the row only when it would change the word: on a word
            // already whole, it runs the line.
            let changes = bar.suggest.changes(&bar.text, c);
            if keys.tab || (keys.enter && changes) {
                bar.accept(c, bar.suggest.highlight);
                accepted = true;
            }
        } else {
            let typed: Vec<String> = self
                .session
                .command_log
                .entries()
                .rev()
                .filter(|e| e.source == log::Source::Bar)
                .filter_map(|e| e.line.clone())
                .collect();
            if keys.up && !typed.is_empty() {
                if bar.recall.is_none() {
                    bar.draft = Some(bar.text.clone());
                }
                let at = bar.recall.map_or(0, |n| (n + 1).min(typed.len() - 1));
                bar.recall = Some(at);
                bar.text = typed[at].clone();
                bar.recalled = Some(bar.text.clone());
                bar.caret_to = Some(bar.text.len());
            }
            if keys.down
                && let Some(n) = bar.recall
            {
                if n == 0 {
                    bar.recall = None;
                    bar.recalled = None;
                    bar.text = bar.draft.take().unwrap_or_default();
                } else {
                    bar.recall = Some(n - 1);
                    bar.text = typed[n - 1].clone();
                    bar.recalled = Some(bar.text.clone());
                }
                bar.caret_to = Some(bar.text.len());
            }
            if keys.tab
                && let Some((said, e)) = &bar.error
                && *said == bar.text
                && let Some((r, fix)) = e.fix.clone()
                && r.end <= bar.text.len()
                && bar.text.is_char_boundary(r.start)
                && bar.text.is_char_boundary(r.end)
            {
                bar.text.replace_range(r.clone(), &fix);
                bar.caret_to = Some(r.start + fix.len());
                bar.error = None;
            }
        }
        if keys.enter && !accepted {
            bar.suggest.state = Suggest::Off;
            let line = bar.text.trim().to_owned();
            // The error's spans count from the trimmed line; the field holds the
            // spaces before it.
            let lead = bar.text.len() - bar.text.trim_start().len();
            if !line.is_empty() {
                match self.run_bar_line(ctx, &line) {
                    None => {
                        bar = Bar::default();
                    }
                    Some(mut e) => {
                        let shift = |r: &mut std::ops::Range<usize>| {
                            *r = r.start + lead..r.end + lead;
                        };
                        if let Some(span) = e.span.as_mut() {
                            shift(span);
                        }
                        if let Some((r, _)) = e.fix.as_mut() {
                            shift(r);
                        }
                        bar.error = Some((bar.text.clone(), e));
                    }
                }
                ctx.request_repaint();
            }
        }

        // Docked at the foot always: a card that jumps away from the selection
        // pulls the eye with it, and the canvas under it pans to reveal.
        let list_h = list_height(&list, self.session.command_log.entries().len());
        let height = list_h + HEAD_H + 1.0 + FIELD_H + 1.0 + FOOTER_H;
        // Placed from the height worked out above rather than anchored: an anchor
        // reads last frame's size, so a growing list would push the foot down a
        // frame before it settled. The list is drawn into exactly that height.
        let outer = egui::vec2(W, height) + crate::settings::card_frame().total_margin().sum();
        let top = canvas.bottom() - DOCK - outer.y;
        // A plain area, not a modal: the canvas under it stays live, and only the
        // card takes the pointer. Not constrained: egui keeps an area on screen by
        // last frame's size, so a list that just shrank would shove the card up a
        // frame.
        let area = egui::Area::new(egui::Id::new("command-bar"))
            .sense(egui::Sense::hover())
            .order(egui::Order::Foreground)
            .interactable(true)
            .constrain(false)
            .fixed_pos(egui::pos2(
                self.card_lane(ctx).center().x - outer.x / 2.0,
                top,
            ));

        let subject = self.bar_subject(&pills.subject_ids);
        let subject_glyph = pills
            .subject_ids
            .first()
            .and_then(|id| self.session.doc.get(*id))
            .map(|n| crate::panels::layers::node_icon(n).0);
        let path = self.pick_path();
        let typed = self.pick_typed().to_owned();
        let mut focus_click: Option<bool> = None;
        let parsed = mastik_vm::desugar(bar.text.trim());
        let mut clicked_row: Option<usize> = None;
        let mut hovered_row: Option<usize> = None;
        let row_layers = bar
            .completed
            .as_ref()
            .map(|c| c.layers.clone())
            .unwrap_or_default();
        let mut pointed_pill: Option<NodeId> = None;
        let mut log_click: Option<(usize, bool)> = None;
        let mut log_menu_at: Option<(usize, egui::Pos2)> = None;
        let entries: Vec<log::Entry> = self.session.command_log.entries().cloned().collect();
        area.show(ctx, |ui| {
            crate::settings::card_frame().show(ui, |ui| {
                ui.set_width(W);
                ui.spacing_mut().item_spacing = egui::vec2(0.0, 0.0);
                match &list {
                    List::None => {}
                    List::Log => {
                        let first = entries.len().saturating_sub(LOG_ROWS);
                        list_frame(ui, list_h, |ui| {
                            palette::section_label(ui, "Log");
                            for (i, e) in entries.iter().enumerate().skip(first) {
                                let picked = bar
                                    .picked
                                    .is_some_and(|(a, b)| (a.min(b)..=a.max(b)).contains(&i));
                                let resp = log_row(ui, e, picked);
                                if resp.clicked() {
                                    let shift = ui.input(|i| i.modifiers.shift);
                                    log_click = Some((i, shift));
                                }
                                if resp.secondary_clicked()
                                    && let Some(p) = resp.interact_pointer_pos()
                                {
                                    log_menu_at = Some((i, p));
                                }
                            }
                        });
                    }
                    List::Complete(c) => {
                        list_frame(ui, list_h, |ui| {
                            let layer_of = |i: usize| {
                                row_layers
                                    .get(i)
                                    .copied()
                                    .flatten()
                                    .and_then(|id| self.session.doc.get(id))
                            };
                            (clicked_row, hovered_row) =
                                completion_rows(ui, &c.rows, bar.suggest.highlight, layer_of);
                        });
                    }
                }
                let error = bar.error.as_ref().map(|(_, e)| e.clone());
                let error = error.as_ref();
                let value = bar.read.as_ref().and_then(|(_, _, v)| v.clone());
                if let Some(pick) = head(ui, picking, &path, subject_glyph, &subject) {
                    focus_click = Some(pick);
                }
                crate::ui::card_rule(ui);
                // What the field changes from here on is typing, which next
                // frame's `Suggester::follow` reads against this.
                bar.suggest.seen = bar.text.clone();
                let sleep = picking.then_some(typed.as_str());
                pointed_pill = field(ui, &mut bar, error, value.as_deref(), sleep, &pills);
                crate::ui::card_rule(ui);
                let hint = |s: &str| (crate::input::keys(s).into_owned(), false);
                match error {
                    _ if picking => {
                        let (left, _) =
                            hint("Labels pick · Shift adds · Enter in · Backspace out · * in each repeat");
                        footer(ui, &left, false, "` types");
                    }
                    Some(e) => error_line(ui, e),
                    None => {
                        // The Luau in the code face; a hint about keys in the
                        // caption face, spelled by `input::keys` as the palette's.
                        // The subject is the head row's; the right edge says how
                        // to pick one.
                        let picks = "` picks".to_owned();
                        let (left, right) = match (&list, &value) {
                            (List::Complete(_), _) => {
                                (hint("Tab completes · ↑↓ choose · Esc hides"), picks)
                            }
                            _ if bar.recalled.is_some() => (hint("↑ recalled from the log"), picks),
                            (_, Some(v)) => {
                                ((parsed.luau.clone(), true), format!("Enter copies {v}"))
                            }
                            _ if bar.text.trim().is_empty() => {
                                (hint("↑ recalls a line · Esc closes"), picks)
                            }
                            _ if parsed.each_by_itself => {
                                ((format!("{} · each by itself", parsed.luau), true), picks)
                            }
                            _ => ((parsed.luau.clone(), true), picks),
                        };
                        footer(ui, &left.0, left.1, &right);
                    }
                }
            });
        });
        match focus_click {
            Some(true) => self.set_picking(true),
            Some(false) => self.set_picking(false),
            None => {}
        }
        // A click writes the text, not typing: `seen` takes it so the list
        // does not open on it.
        if let Some(i) = hovered_row {
            bar.suggest.highlight = i;
        }
        if let Some(i) = clicked_row
            && let List::Complete(c) = &list
        {
            bar.accept(c, i);
            bar.suggest.seen = bar.text.clone();
        }
        if let Some((i, shift)) = log_click {
            bar.picked = match (shift, bar.picked) {
                (true, Some((a, _))) => Some((a, i)),
                _ => Some((i, i)),
            };
            if !shift
                && let Some(line) = entries.get(i).and_then(|e| e.line.clone())
                && !matches!(entries[i].source, log::Source::Script(_))
            {
                bar.text = line;
                bar.caret_to = Some(bar.text.len());
                bar.suggest.seen = bar.text.clone();
            }
        }
        if let Some((i, at)) = log_menu_at {
            if !bar
                .picked
                .is_some_and(|(a, b)| (a.min(b)..=a.max(b)).contains(&i))
            {
                bar.picked = Some((i, i));
            }
            bar.log_menu = Some(at);
        }
        if let Some(at) = bar.log_menu {
            match self.log_menu_ui(ctx, at, &entries, bar.picked) {
                LogMenu::Open => {}
                LogMenu::Closed => bar.log_menu = None,
                LogMenu::Copied => {
                    bar.log_menu = None;
                }
                LogMenu::Editor(source) => {
                    bar.log_menu = None;
                    self.end_bar_session();
                    self.open_script_editor_with(source);
                    return;
                }
            }
        }
        bar.lit = match bar.suggest.state {
            Suggest::On(_) => lit_of(&bar),
            _ => None,
        }
        .or(pointed_pill);
        bar.references = pills
            .hues(&bar.text)
            .into_iter()
            .filter(|(id, _)| !pills.subject_ids.contains(id))
            .collect();
        self.command_bar = Some(bar);
    }

    /// The log's right-click menu: *Copy N lines as script* and *Open in editor*.
    fn log_menu_ui(
        &mut self,
        ctx: &egui::Context,
        at: egui::Pos2,
        entries: &[log::Entry],
        picked: Option<(usize, usize)>,
    ) -> LogMenu {
        let Some((a, b)) = picked else {
            return LogMenu::Closed;
        };
        let script = as_script(&entries[a.min(b)..=a.max(b).min(entries.len() - 1)]);
        let n = a.abs_diff(b) + 1;
        let copy = match n {
            1 => "Copy as script".to_owned(),
            n => format!("Copy {n} lines as script"),
        };
        let mut out = LogMenu::Open;
        let size = egui::vec2(crate::menu::MENU_W, crate::ui::menu_height(2, 0));
        let resp = egui::Area::new(egui::Id::new("command-log-menu"))
            .order(egui::Order::Foreground)
            .fixed_pos(crate::ui::menu_place(ctx, at, size))
            .show(ctx, |ui| {
                crate::ui::menu_frame(crate::ui::MENU_PAD).show(ui, |ui| {
                    ui.set_width(crate::ui::menu_inner_w(
                        crate::menu::MENU_W,
                        crate::ui::MENU_PAD,
                    ));
                    crate::ui::popup_rows(ui);
                    let h = crate::ui::MENU_ROW_H;
                    if crate::ui::menu_item(ui, icon::COPY, &copy, h).clicked() {
                        ctx.copy_text(script.clone());
                        out = LogMenu::Copied;
                    }
                    if crate::ui::menu_item(ui, icon::FILE_CODE, "Open in editor", h).clicked() {
                        out = LogMenu::Editor(script.clone());
                    }
                });
            });
        if matches!(out, LogMenu::Open)
            && (ctx.input(|i| i.pointer.any_pressed()) && !resp.response.contains_pointer())
        {
            return LogMenu::Closed;
        }
        out
    }
}

/// What the log's menu did this frame.
enum LogMenu {
    Open,
    Closed,
    Copied,
    Editor(String),
}

/// The Luau a run of log entries writes as a script: each line's footer form; an
/// edit with no command yet as a comment naming its step.
pub(crate) fn as_script(entries: &[log::Entry]) -> String {
    let mut out = String::new();
    for e in entries {
        match (&e.luau, &e.source) {
            (Some(luau), _) => out.push_str(luau),
            (None, log::Source::Script(name)) => {
                out.push_str(&format!("-- run {}", log::quoted(name)));
            }
            (None, log::Source::Edit { label, .. }) => {
                out.push_str(&format!("-- {label}: no command yet"));
            }
            (None, log::Source::Bar) => {}
        }
        out.push('\n');
    }
    out
}

/// A completion section's eyebrow.
fn section_title(s: mastik_vm::Section) -> &'static str {
    use mastik_vm::Section as S;
    match s {
        S::Names => "Names",
        S::Verbs => "Verbs",
        S::Properties => "Properties",
        S::Labels => "Labels",
        S::Layers => "Layers",
        S::Choices => "Choices",
        S::Script => "This script",
        S::Luau => "Luau",
    }
}

/// The verb a bar line calls, when its first word names one: its undo step is
/// named after it.
fn verb_named(line: &str) -> Option<&'static str> {
    let word: String = line
        .trim_start()
        .chars()
        .take_while(|c| c.is_ascii_alphanumeric() || *c == '_')
        .collect();
    mastik_core::verbs::find(&word).map(|v| v.name)
}

/// The field's id, so the caret can be read and placed.
fn field_id() -> egui::Id {
    egui::Id::new("command-bar-field")
}

/// The byte offset of char `i` in `s`.
fn char_to_byte(s: &str, i: usize) -> usize {
    s.char_indices().nth(i).map_or(s.len(), |(b, _)| b)
}

/// The list's height above the field for `list`, its rule under it included,
/// which the dock reads before the card draws.
fn list_height(list: &List, log_len: usize) -> f32 {
    let inner = match list {
        List::None => return 0.0,
        List::Log => stack_height(1, log_len.min(LOG_ROWS)),
        List::Complete(c) => completion_height(&c.rows),
    };
    let rule = 1.0;
    2.0 * PAD + inner + rule
}

/// `sections` section labels and `rows` rows stacked with a gap between each.
fn stack_height(sections: usize, rows: usize) -> f32 {
    let gaps = (sections + rows).saturating_sub(1);
    sections as f32 * SECTION_H + rows as f32 * ROW_H + gaps as f32 * ROW_GAP
}

/// What [`completion_rows`] draws `rows` into: the first [`LIST_ROWS`] and a
/// label over each run of one section, insets left out.
fn completion_height(rows: &[mastik_vm::Row]) -> f32 {
    let shown = &rows[..rows.len().min(LIST_ROWS)];
    let sections = 1 + shown
        .windows(2)
        .filter(|w| w[0].section != w[1].section)
        .count();
    match shown.is_empty() {
        true => 0.0,
        false => stack_height(sections, shown.len()),
    }
}

/// The first [`LIST_ROWS`] completion rows, a section label over each run, row
/// `lit` lit; `layer_of(i)` is row `i`'s layer, for its glyph. Answers the row
/// clicked, and the row the pointer moved onto (the highlight follows a moving
/// pointer, as the palette's does; a resting one leaves the keys theirs). The
/// command bar's list and the script editor's draw it.
fn completion_rows<'a>(
    ui: &mut egui::Ui,
    rows: &[mastik_vm::Row],
    lit: usize,
    layer_of: impl Fn(usize) -> Option<&'a mastik_core::Node>,
) -> (Option<usize>, Option<usize>) {
    let moved = ui.input(|i| i.pointer.delta() != egui::Vec2::ZERO);
    let (mut clicked, mut hovered) = (None, None);
    let mut section = None;
    for (i, row) in rows.iter().take(LIST_ROWS).enumerate() {
        if section != Some(row.section) {
            palette::section_label(ui, section_title(row.section));
            section = Some(row.section);
        }
        let resp = complete_row(ui, layer_of(i), row, i == lit);
        if resp.clicked() {
            clicked = Some(i);
        }
        if resp.hovered() && moved {
            hovered = Some(i);
        }
    }
    (clicked, hovered)
}

/// The list in its inset, drawn into exactly `h` (from [`list_height`]) with
/// the rule under it, so a miscount can clip a row but never move the card.
fn list_frame(ui: &mut egui::Ui, h: f32, add: impl FnOnce(&mut egui::Ui)) {
    let (rect, _) = ui.allocate_exact_size(egui::vec2(W, h - 1.0), egui::Sense::hover());
    let mut child = ui.new_child(
        egui::UiBuilder::new()
            .max_rect(rect.shrink(PAD))
            .layout(egui::Layout::top_down(egui::Align::Min)),
    );
    child.set_clip_rect(rect.intersect(ui.clip_rect()));
    child.spacing_mut().item_spacing.y = ROW_GAP;
    add(&mut child);
    crate::ui::card_rule(ui);
}

/// The monospace face at the palette field's size.
fn mono() -> egui::FontId {
    egui::FontId::monospace(crate::ui::HEAD_PT)
}

/// The head row: the focus as one icon toggle (`icon::LEAP_LABEL`, on while the
/// bar picks), the path to where the selection sits, and the subject a line
/// acts on with its kind's glyph. Answers a click on the toggle: `true` to pick.
fn head(
    ui: &mut egui::Ui,
    picking: bool,
    path: &[String],
    glyph: Option<&str>,
    subject: &str,
) -> Option<bool> {
    let (rect, _) = ui.allocate_exact_size(egui::vec2(W, HEAD_H), egui::Sense::hover());
    let inner = rect.shrink2(egui::vec2(FOOTER_INSET, 0.0));
    let mut child = ui.new_child(
        egui::UiBuilder::new()
            .max_rect(inner)
            .layout(egui::Layout::left_to_right(egui::Align::Center)),
    );
    // A card head's action icon, on while the bar picks; a click toggles it.
    let tip = match picking {
        true => "Back to typing (`)",
        false => "Pick layers by label (`)",
    };
    let clicked = crate::ui::icon_button(
        &mut child,
        icon::LEAP_LABEL,
        crate::ui::HEAD_ACTION_W,
        crate::ui::HEAD_ACTION_GLYPH_PT,
        picking,
        true,
    )
    .on_hover_text(tip)
    .clicked()
    .then_some(!picking);
    let p = ui.painter().clone();
    let y = rect.center().y;
    let font = egui::FontId::proportional(crate::ui::SMALL_PT);
    // The subject first, from the right, so the path knows its room.
    let (ink, glyph_ink) = match glyph {
        Some(_) => (color::text(), theme::text::dim()),
        None => (theme::text::faint(), theme::text::faint()),
    };
    let g = p.layout_no_wrap(subject.to_owned(), font.clone(), ink);
    let mut right = inner.right() - g.size().x;
    p.galley(egui::pos2(right, y - g.size().y / 2.0), g, ink);
    if let Some(glyph) = glyph {
        right -= 6.0;
        let r = p.text(
            egui::pos2(right, y),
            egui::Align2::RIGHT_CENTER,
            glyph,
            theme::icon_font(crate::ui::CONTROL_GLYPH_PT),
            glyph_ink,
        );
        right = r.left();
    }
    let room = egui::Rect::from_min_max(
        egui::pos2(
            inner.left() + crate::ui::HEAD_ACTION_W + HEAD_GAP,
            rect.top(),
        ),
        egui::pos2(right - HEAD_GAP, rect.bottom()),
    );
    let clipped = p.with_clip_rect(room.intersect(p.clip_rect()));
    let mut x = room.left();
    for (i, name) in path.iter().enumerate() {
        if i > 0 {
            let r = clipped.text(
                egui::pos2(x, y),
                egui::Align2::LEFT_CENTER,
                icon::CARET_RIGHT,
                theme::icon_font(crate::ui::SMALL_PT),
                theme::text::faint(),
            );
            x = r.right() + 4.0;
        }
        let last = i + 1 == path.len();
        let ink = match last {
            true => theme::text::strong(),
            false => theme::text::faint(),
        };
        let r = clipped.text(
            egui::pos2(x, y),
            egui::Align2::LEFT_CENTER,
            name,
            font.clone(),
            ink,
        );
        x = r.right() + 4.0;
    }
    clicked
}

/// The field: the subject's pill, or with nothing selected the `:` prompt in the
/// accent; the line with its pills (`Pills`); a read's value at the right edge,
/// and a squiggle under the token an error names. Asleep (`sleep`, the letters
/// typed of a label) while the bar picks: the line is kept but dimmed and
/// unfocused, and the right edge shows the letters typed. Answers the layer of
/// the pill under the pointer.
fn field(
    ui: &mut egui::Ui,
    bar: &mut Bar,
    error: Option<&mastik_vm::VmError>,
    value: Option<&str>,
    sleep: Option<&str>,
    pills: &Pills,
) -> Option<NodeId> {
    let (rect, _) = ui.allocate_exact_size(egui::vec2(W, FIELD_H), egui::Sense::empty());
    let mut p = ui.painter().clone();
    if sleep.is_some() {
        p.multiply_opacity(ASLEEP);
    }
    let pill = |p: &egui::Painter, at: egui::Pos2, align: egui::Align2, letters: &str, hue| {
        let pad = crate::navigate::LABEL_PAD;
        crate::ui::pill_shapes(p, at, align, letters, hue, pad, 0)
    };
    // The whole width is the line's, from the head row's inset: no prompt column.
    let text_left = rect.left() + FOOTER_INSET;
    let mut tips: Vec<(egui::Rect, String, Option<NodeId>)> = Vec::new();
    let edge = match sleep {
        Some(typed) => {
            (!typed.is_empty()).then(|| (format!("{}…", typed.to_uppercase()), color::leap()))
        }
        None => value.map(|v| (format!("= {v}"), theme::text::dim())),
    };
    let value_w = edge.map_or(0.0, |(v, ink)| {
        let g = p.layout_no_wrap(v, mono(), ink);
        let w = g.size().x;
        p.galley(
            egui::pos2(
                rect.right() - VALUE_INSET - w,
                rect.center().y - g.size().y / 2.0,
            ),
            g,
            theme::text::dim(),
        );
        w + VALUE_GAP
    });
    let text_rect = egui::Rect::from_min_max(
        egui::pos2(text_left, rect.top()),
        egui::pos2(rect.right() - VALUE_INSET - value_w, rect.bottom()),
    );
    let mut child = ui.new_child(
        egui::UiBuilder::new()
            .max_rect(text_rect)
            .layout(egui::Layout::left_to_right(egui::Align::Center)),
    );
    let ink = match sleep {
        Some(_) => theme::text::faint(),
        None => color::text(),
    };
    let mut layouter = |ui: &egui::Ui, buf: &dyn egui::TextBuffer, _wrap: f32| {
        pill_galley(ui, buf.as_str(), pills, ink)
    };
    // The pills go under the caret, so their place is taken before the field draws.
    let under = p.add(egui::Shape::Noop);
    #[expect(
        clippy::disallowed_methods,
        reason = "the command bar's field: the palette's own scale, in the monospace face"
    )]
    let out = egui::TextEdit::singleline(&mut bar.text)
        .id(field_id())
        .frame(egui::Frame::NONE)
        .desired_width(f32::INFINITY)
        .font(mono())
        .interactive(sleep.is_none())
        .text_color(ink)
        .layouter(&mut layouter)
        .hint_text(
            egui::RichText::new(match sleep {
                Some(_) => "Picking: type a label",
                None => "Pick a layer and type a command",
            })
            .font(mono())
            .color(theme::text::faint()),
        )
        .show(&mut child);
    // Every frame: the field owns the keyboard while the bar types, and gives it
    // up while the bar picks, so the canvas's letters reach the labels.
    match sleep {
        None if !out.response.has_focus() => out.response.request_focus(),
        // By id: a field that is not interactive may not say it holds the focus.
        Some(_) => ui.memory_mut(|m| m.surrender_focus(field_id())),
        None => {}
    }
    if let Some(at) = bar.caret_to.take() {
        let mut state = out.state.clone();
        let chars = bar.text[..at.min(bar.text.len())].chars().count();
        state
            .cursor
            .set_char_range(Some(egui::text::CCursorRange::one(
                egui::text::CCursor::new(chars),
            )));
        state.store(ui.ctx(), field_id());
    }
    if let Some(span) = error.and_then(|e| e.span.clone())
        && span.end <= bar.text.len()
        && span.start < span.end
        && bar.text.is_char_boundary(span.start)
        && bar.text.is_char_boundary(span.end)
    {
        let x = |b: usize| {
            let c = bar.text[..b].chars().count();
            out.galley_pos.x
                + out
                    .galley
                    .pos_from_cursor(egui::text::CCursor::new(c))
                    .min
                    .x
        };
        let y = out.galley_pos.y + out.galley.size().y + 1.0;
        crate::canvas::paint_squiggle(&p, egui::pos2(x(span.start), y), egui::pos2(x(span.end), y));
    }
    // The pills, from the galley the field just drew, which the layouter made
    // room in. The text may have changed this frame, so it is read again.
    let text = &bar.text;
    let x_of = |b: usize| {
        let c = text[..b].chars().count();
        out.galley_pos.x
            + out
                .galley
                .pos_from_cursor(egui::text::CCursor::new(c))
                .min
                .x
    };
    let y = out.galley_pos.y + out.galley.size().y / 2.0;
    let clipped = p.with_clip_rect(text_rect.intersect(p.clip_rect()));
    let mut shapes = Vec::new();
    let hues = pills.hues(text);
    for (r, id) in pills.named(text) {
        // The layouter laid the hidden letters out as wide as the pill.
        let at = egui::pos2(x_of(r.start), y);
        let letters = text[r].to_uppercase();
        let hue = hues
            .iter()
            .find(|(h, _)| *h == id)
            .map_or(color::leap(), |(_, c)| *c);
        let (rect, s) = pill(&clipped, at, egui::Align2::LEFT_CENTER, &letters, hue);
        shapes.extend(s);
        let name = pills.names.get(&id).cloned().unwrap_or_default();
        tips.push((rect.intersect(text_rect), name, Some(id)));
    }
    if let Some((letters, what)) = &pills.subject {
        for d in pills.subject_dots(text) {
            // Clear of the caret that stands before the dot.
            let at = egui::pos2(x_of(d) - 1.0, y);
            let (rect, s) = pill(
                &clipped,
                at,
                egui::Align2::RIGHT_CENTER,
                letters,
                color::leap(),
            );
            shapes.extend(s);
            tips.push((rect.intersect(text_rect), what.clone(), None));
        }
    }
    clipped.set(under, egui::Shape::Vec(shapes));
    let mut hovered = None;
    for (i, (rect, tip, id)) in tips.into_iter().enumerate() {
        if !rect.is_positive() {
            continue;
        }
        let resp = ui
            .interact(rect, field_id().with(("pill", i)), egui::Sense::hover())
            .on_hover_text(tip);
        if resp.hovered() {
            hovered = id;
        }
    }
    hovered
}

/// A caption in the footer's place: the error, and *Tab fixes it* when it can.
fn error_line(ui: &mut egui::Ui, e: &mastik_vm::VmError) {
    palette::footer_line(
        ui,
        FooterLine {
            glyph: Some((icon::WARNING, color::error())),
            left: (&e.message, palette::caption_font(), color::error()),
            right: e
                .fix
                .is_some()
                .then_some(("Tab fixes it", theme::text::faint())),
        },
    );
}

/// The footer: the Luau (`code`) or a hint on the left, a hint or the read's
/// note on the right.
fn footer(ui: &mut egui::Ui, left: &str, code: bool, right: &str) {
    let face = match code {
        true => egui::FontId::monospace(crate::ui::CAPTION_PT),
        false => palette::caption_font(),
    };
    palette::footer_line(
        ui,
        FooterLine {
            left: (left, face, theme::text::dim()),
            right: Some((right, theme::text::muted())),
            ..Default::default()
        },
    );
}

/// A row's line of code: the monospace face at the rows' size.
fn row_mono() -> egui::FontId {
    egui::FontId::monospace(crate::ui::CONTROL_PT)
}

/// One log row: where it came from as a glyph, the line in the monospace face
/// (dim for an edit made elsewhere), and its result or where it came from.
fn log_row(ui: &mut egui::Ui, e: &log::Entry, picked: bool) -> egui::Response {
    let (rect, resp) = ui.allocate_exact_size(
        egui::vec2(ui.available_width(), ROW_H),
        egui::Sense::click(),
    );
    let p = ui.painter().clone();
    palette::row_ground(&p, rect, picked, true);
    let glyph = match &e.source {
        log::Source::Bar => icon::TERMINAL_WINDOW,
        log::Source::Script(_) => icon::FILE_CODE,
        log::Source::Edit { glyph, .. } if !glyph.is_empty() => glyph.as_ref(),
        log::Source::Edit { .. } => icon::CURSOR,
    };
    palette::row_glyph(&p, rect, glyph, theme::text::dim());
    let side = match &e.source {
        log::Source::Edit { label, .. } => label.clone(),
        log::Source::Script(_) => format!("Script · {}", e.result),
        log::Source::Bar => e.result.clone(),
    };
    let side_ink = match e.failed {
        true => color::error(),
        false => theme::text::faint(),
    };
    let (text, ink) = match (&e.line, &e.source) {
        (Some(line), log::Source::Bar) => (line.as_str(), color::text()),
        (Some(line), _) => (line.as_str(), theme::text::dim()),
        (None, _) => ("— no command yet —", theme::text::faint()),
    };
    let job = palette::lit_label(text, &[], row_mono(), ink, ink);
    palette::row_texts(&p, rect, rect.right() - 8.0, job, Some((&side, side_ink)));
    resp
}

/// One completion row: its glyph, the insert with the matched letters lit, and
/// its side note — a verb's arguments, a property's type, a layer's page, or an
/// almost-match's suggestion, dim.
fn complete_row(
    ui: &mut egui::Ui,
    layer: Option<&mastik_core::Node>,
    row: &mastik_vm::Row,
    lit: bool,
) -> egui::Response {
    use mastik_vm::Section as S;
    let (rect, resp) = ui.allocate_exact_size(
        egui::vec2(ui.available_width(), ROW_H),
        egui::Sense::click(),
    );
    let p = ui.painter().clone();
    palette::row_ground(&p, rect, lit, true);
    let glyph = match (row.section, layer) {
        (_, Some(node)) => crate::panels::layers::node_icon(node).0,
        (S::Verbs, _) => icon::TERMINAL_WINDOW,
        (S::Properties | S::Names | S::Script | S::Luau, _) => icon::HASH,
        (S::Choices | S::Labels | S::Layers, None) => icon::CARET_RIGHT,
    };
    let glyph_ink = match lit {
        true => color::accent_200(),
        false => theme::text::dim(),
    };
    palette::row_glyph(&p, rect, glyph, glyph_ink);
    let base = match (row.dim, lit) {
        (true, _) => theme::text::faint(),
        (false, true) => color::text(),
        (false, false) => theme::text::strong(),
    };
    let job = palette::lit_label(
        &row.label,
        &row.matched,
        row_mono(),
        base,
        color::accent_300(),
    );
    palette::row_texts(
        &p,
        rect,
        rect.right() - 8.0,
        job,
        Some((&row.detail, theme::text::faint())),
    );
    resp
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use crate::palette::tests::add_rect;

    /// One frame of the whole app, with these events and modifiers held.
    pub(crate) fn pass(
        ctx: &egui::Context,
        app: &mut MastikApp,
        events: Vec<egui::Event>,
        modifiers: egui::Modifiers,
    ) {
        let mut frame = eframe::Frame::_new_kittest();
        let input = egui::RawInput {
            screen_rect: Some(egui::Rect::from_min_size(
                egui::Pos2::ZERO,
                egui::vec2(1320.0, 820.0),
            )),
            modifiers,
            events,
            ..Default::default()
        };
        let _ = ctx.run_ui(input, |ui| eframe::App::ui(app, ui, &mut frame));
    }

    pub(crate) fn key(key: egui::Key, modifiers: egui::Modifiers) -> egui::Event {
        egui::Event::Key {
            key,
            physical_key: None,
            pressed: true,
            repeat: false,
            modifiers,
        }
    }

    const NONE: egui::Modifiers = egui::Modifiers::NONE;

    /// An app with one selected rectangle, the bar opened by `:` and `line` typed.
    fn typed(line: &str) -> (egui::Context, MastikApp, NodeId) {
        let ctx = egui::Context::default();
        crate::theme::install(&ctx);
        let mut app = MastikApp::headless(&ctx);
        let card = add_rect(&mut app);
        app.session.selection.set_one(card);
        pass(&ctx, &mut app, Vec::new(), NONE);
        pass(
            &ctx,
            &mut app,
            vec![key(egui::Key::Colon, egui::Modifiers::SHIFT)],
            egui::Modifiers::SHIFT,
        );
        assert!(app.command_bar.is_some(), "`:` opens the bar");
        pass(&ctx, &mut app, Vec::new(), NONE);
        pass(&ctx, &mut app, vec![egui::Event::Text(line.into())], NONE);
        pass(&ctx, &mut app, Vec::new(), NONE);
        assert_eq!(
            app.command_bar.as_ref().map(|b| b.text.as_str()),
            Some(line),
            "fixture: the line was typed"
        );
        (ctx, app, card)
    }

    fn left(app: &MastikApp, id: NodeId) -> f64 {
        app.session.resolved.world_bounds(id).expect("a box").x0
    }

    /// A label's hidden letters are laid out exactly as wide as its pill, so the
    /// caret at either end of it stands at the pill's edge, not inside it.
    ///
    /// flip: `pill_galley` laying the letters out at the plain size → fails at
    /// "as wide as the pill".
    #[test]
    fn a_labels_letters_span_its_pill() {
        let ctx = egui::Context::default();
        crate::theme::install(&ctx);
        let id = mastik_core::IdSource::new(1).mint();
        let pills = Pills {
            subject: None,
            subject_ids: Vec::new(),
            labels: [("fj".to_owned(), id)].into_iter().collect(),
            names: Default::default(),
            typing: None,
        };
        let mut got = (0.0, 0.0);
        let _ = ctx.run_ui(egui::RawInput::default(), |ui| {
            let g = pill_galley(ui, "fj", &pills, egui::Color32::WHITE);
            let x = |c: usize| g.pos_from_cursor(egui::text::CCursor::new(c)).min.x;
            got = (x(2) - x(0), pill_size(ui.painter(), "FJ").x);
        });
        assert!((got.0 - got.1).abs() < 0.5, "as wide as the pill: {got:?}");
    }

    /// A label named in the line is a pill unless it is the word being typed or
    /// is reached through a dot; every subject dot, the one opening the line
    /// too, wears the subject's pill.
    ///
    /// flip: `Pills::named` keeping the word being typed → fails at "still typed".
    #[test]
    fn pills_are_the_labels_named_and_the_subject_dots() {
        let id = mastik_core::IdSource::new(1).mint();
        let mut pills = Pills {
            subject: Some(("SK".into(), "Card".into())),
            subject_ids: Vec::new(),
            labels: [("fj".to_owned(), id), ("d".to_owned(), id)]
                .into_iter()
                .collect(),
            names: Default::default(),
            typing: None,
        };
        let line = ".x = fj.x - .d + d";
        let named: Vec<&str> = pills
            .named(line)
            .into_iter()
            .map(|(r, _)| &line[r])
            .collect();
        assert_eq!(named, ["fj", "d"], "not the `d` after a dot");
        assert_eq!(pills.subject_dots(line), [0, 12], "the opening dot too");
        pills.typing = Some(17);
        assert_eq!(pills.named(line).len(), 1, "still typed");
    }

    /// A selected layer named in the line wears the subject's yellow; the others
    /// take the reference hues in order of first mention, one hue per layer
    /// however often it is named.
    ///
    /// flip: `Pills::hues` giving each mention its own hue → fails at "one hue
    /// per layer".
    #[test]
    fn named_layers_take_hues_by_first_mention() {
        let mut ids = mastik_core::IdSource::new(1);
        let [sel, a, b] = [ids.mint(), ids.mint(), ids.mint()];
        let labels = [("sk", sel), ("fj", a), ("dk", b)]
            .into_iter()
            .map(|(l, id)| (l.to_owned(), id))
            .collect();
        let pills = Pills {
            subject: Some(("SK".into(), "Card".into())),
            subject_ids: vec![sel],
            labels,
            names: Default::default(),
            typing: None,
        };
        let hues = pills.hues(".x = dk.x + sk.x - fj.x + dk.y");
        assert_eq!(
            hues,
            [
                (b, color::REFERENCE[0]),
                (sel, color::leap()),
                (a, color::REFERENCE[1]),
            ],
            "one hue per layer"
        );
    }

    /// `:`, a line and Enter: the selection moves, as one undo step named *Edit*
    /// with the bar's glyph, the bar stays open and empty, and the log lists the line.
    /// flip: `run_as_step` leaving the note unnamed → fails at "the bar's glyph".
    #[test]
    fn a_line_edits_the_selection_as_one_named_step() {
        let (ctx, mut app, card) = typed(".x += 15");
        let (x, depth) = (left(&app, card), app.session.history.undo_depth());
        pass(&ctx, &mut app, vec![key(egui::Key::Enter, NONE)], NONE);
        assert_eq!(left(&app, card), x + 15.0);
        assert_eq!(app.session.history.undo_depth(), depth + 1);
        let top = app.session.history.done().next_back().unwrap();
        assert_eq!(top.note.label, "Edit");
        assert_eq!(top.note.glyph, icon::TERMINAL_WINDOW, "the bar's glyph");
        assert_eq!(app.command_bar.as_ref().map(|b| b.text.as_str()), Some(""));
        let last = app.session.command_log.entries().last().unwrap();
        assert_eq!(last.line.as_deref(), Some(".x += 15"));
        assert_eq!(last.source, log::Source::Bar);
        app.session.undo();
        assert_eq!(left(&app, card), x, "one undo takes it back");
    }

    /// A line that raises after a write changes nothing: every step it committed
    /// is taken back, and the line stays in the field with its error.
    /// flip: `run_as_step` skipping the take-back loop → fails at "nothing changed".
    #[test]
    fn a_line_that_fails_changes_nothing() {
        let (ctx, mut app, card) = typed(r#".x = 300; error("no")"#);
        let (x, depth) = (left(&app, card), app.session.history.undo_depth());
        pass(&ctx, &mut app, vec![key(egui::Key::Enter, NONE)], NONE);
        assert_eq!(left(&app, card), x, "nothing changed");
        assert_eq!(app.session.history.undo_depth(), depth);
        let bar = app.command_bar.as_ref().unwrap();
        assert_eq!(bar.text, r#".x = 300; error("no")"#);
        assert!(bar.error.is_some());
    }

    /// A failed line leaves the redo stack as it found it.
    /// flip: `run_as_step` not restoring the held redo on an error → fails at
    /// "redo kept".
    #[test]
    fn a_line_that_fails_keeps_redo() {
        let (ctx, mut app, _) = typed(r#".x = 300; error("no")"#);
        add_rect(&mut app);
        app.session.undo();
        assert_eq!(
            app.session.history.redo_depth(),
            1,
            "fixture: one step to redo"
        );
        pass(&ctx, &mut app, vec![key(egui::Key::Enter, NONE)], NONE);
        assert!(
            app.command_bar.as_ref().unwrap().error.is_some(),
            "the line failed"
        );
        assert_eq!(app.session.history.redo_depth(), 1, "redo kept");
    }

    /// A read shows its value as it is typed and Enter makes no undo step.
    /// flip: `bar_read` taking no value for a read → fails at "the live value".
    #[test]
    fn a_read_shows_its_value_and_writes_nothing() {
        let (ctx, mut app, _) = typed(".width * 2");
        let value = app.command_bar.as_ref().and_then(|b| b.read.clone());
        assert_eq!(
            value.and_then(|r| r.2).as_deref(),
            Some("20"),
            "the live value"
        );
        let depth = app.session.history.undo_depth();
        pass(&ctx, &mut app, vec![key(egui::Key::Enter, NONE)], NONE);
        assert_eq!(app.session.history.undo_depth(), depth);
        let last = app.session.command_log.entries().last().unwrap();
        assert_eq!(last.result, "= 20");
    }

    /// A misspelt property is an error with a fix, and Tab writes the fix.
    /// flip: the Tab arm ignoring `fix` → fails at "Tab fixes it".
    #[test]
    fn tab_takes_an_errors_fix() {
        let (ctx, mut app, _) = typed(".widht = 20");
        pass(&ctx, &mut app, vec![key(egui::Key::Enter, NONE)], NONE);
        assert!(
            app.command_bar
                .as_ref()
                .unwrap()
                .error
                .as_ref()
                .is_some_and(|(_, e)| e.fix.is_some()),
            "fixture: the error offers a fix"
        );
        pass(&ctx, &mut app, Vec::new(), NONE);
        pass(&ctx, &mut app, vec![key(egui::Key::Tab, NONE)], NONE);
        assert_eq!(
            app.command_bar.as_ref().unwrap().text,
            ".width = 20",
            "Tab fixes it"
        );
    }

    /// `↑` then `↓` gives back what was being typed before the recall.
    /// flip: `↓` past the newest line clearing the text instead of taking the
    /// draft → fails at "the draft is back".
    #[test]
    fn recall_keeps_the_draft() {
        let (ctx, mut app, _) = typed(".x += 1");
        pass(&ctx, &mut app, vec![key(egui::Key::Enter, NONE)], NONE);
        pass(&ctx, &mut app, vec![egui::Event::Text("zzq".into())], NONE);
        pass(&ctx, &mut app, Vec::new(), NONE);
        let text = |app: &MastikApp| app.command_bar.as_ref().unwrap().text.clone();
        assert_eq!(text(&app), "zzq", "fixture: a draft typed");
        pass(&ctx, &mut app, vec![key(egui::Key::ArrowUp, NONE)], NONE);
        assert_eq!(text(&app), ".x += 1", "fixture: the line recalled");
        pass(&ctx, &mut app, vec![key(egui::Key::ArrowDown, NONE)], NONE);
        assert_eq!(text(&app), "zzq", "the draft is back");
    }

    /// The completion list is kept between frames but worked out again when the
    /// document changes under the same text.
    /// flip: `bar_completion`'s key leaving out the revision → fails at "the
    /// renamed layer is offered".
    #[test]
    fn completions_follow_the_document() {
        let (ctx, mut app, card) = typed("Car");
        let offered = |app: &MastikApp| -> Vec<String> {
            let bar = app.command_bar.as_ref().unwrap();
            bar.completed
                .as_ref()
                .map(|c| c.completion.rows.iter().map(|r| r.label.clone()).collect())
                .unwrap_or_default()
        };
        assert!(
            offered(&app).contains(&"Card".to_string()),
            "fixture: Card offered: {:?}",
            offered(&app)
        );
        assert!(app.session.commit(mastik_core::Transaction(vec![
            mastik_core::Operation::SetName {
                id: card,
                name: "Carrot".into(),
            }
        ])));
        pass(&ctx, &mut app, Vec::new(), NONE);
        assert!(
            offered(&app).contains(&"Carrot".to_string()),
            "the renamed layer is offered: {:?}",
            offered(&app)
        );
    }

    /// With the log's menu open, Escape closes the menu and leaves the line.
    /// flip: Escape skipping the menu check → fails at "the line stays".
    #[test]
    fn escape_closes_the_log_menu_first() {
        let (ctx, mut app, _) = typed(".x");
        app.command_bar.as_mut().unwrap().log_menu = Some(egui::pos2(400.0, 400.0));
        pass(&ctx, &mut app, vec![key(egui::Key::Escape, NONE)], NONE);
        let bar = app.command_bar.as_ref().expect("the bar stays open");
        assert_eq!(bar.text, ".x", "the line stays");
        assert!(bar.log_menu.is_none(), "the menu closed");
    }

    /// An error's fix lands on the right word when the line starts with spaces,
    /// which the run trims.
    /// flip: Enter storing the error without shifting its spans → fails at
    /// "past the spaces".
    #[test]
    fn a_fix_lands_past_leading_spaces() {
        let (ctx, mut app, _) = typed("  .widht = 20");
        pass(&ctx, &mut app, vec![key(egui::Key::Enter, NONE)], NONE);
        pass(&ctx, &mut app, Vec::new(), NONE);
        pass(&ctx, &mut app, vec![key(egui::Key::Tab, NONE)], NONE);
        assert_eq!(
            app.command_bar.as_ref().unwrap().text,
            "  .width = 20",
            "past the spaces"
        );
    }

    /// An edit made elsewhere is logged as the command that would make it, and a
    /// run folding more into its step renders the step again.
    /// flip: `observe` skipping the re-render of a grown run → fails at "the run's
    /// one line, again".
    #[test]
    fn an_edit_elsewhere_is_logged_as_its_command() {
        let ctx = egui::Context::default();
        crate::theme::install(&ctx);
        let mut app = MastikApp::headless(&ctx);
        let card = add_rect(&mut app);
        let s = &mut app.session;
        let tx = mastik_core::build::move_by_world(
            &s.doc,
            &s.resolved,
            &[card],
            mastik_core::kurbo::Vec2::new(5.0, 0.0),
        )
        .unwrap();
        assert!(s.commit_run(tx, "nudge"));
        log::Log::watch(s);
        assert_eq!(
            s.command_log.entries().last().unwrap().line.as_deref(),
            Some(".x += 5")
        );
        let tx = mastik_core::build::move_by_world(
            &s.doc,
            &s.resolved,
            &[card],
            mastik_core::kurbo::Vec2::new(5.0, -2.0),
        )
        .unwrap();
        assert!(s.commit_run(tx, "nudge"));
        log::Log::watch(s);
        let lines: Vec<_> = s
            .command_log
            .entries()
            .filter_map(|e| e.line.clone())
            .collect();
        assert_eq!(
            lines,
            vec![".x += 10; .y -= 2".to_owned()],
            "the run's one line, again"
        );
    }

    /// The card grows back as its list does: empty, then with a log above the
    /// field, its height follows, and it keeps its foot where it docks.
    #[test]
    fn the_card_grows_with_its_list() {
        let (ctx, mut app, _) = typed("1 + 1");
        let rect = |ctx: &egui::Context| {
            ctx.memory(|m| m.area_rect(egui::Id::new("command-bar")))
                .expect("the card is drawn")
        };
        let short = rect(&ctx);
        pass(&ctx, &mut app, vec![key(egui::Key::Enter, NONE)], NONE);
        for _ in 0..3 {
            pass(&ctx, &mut app, Vec::new(), NONE);
        }
        let tall = rect(&ctx);
        assert!(
            tall.height() > short.height() + ROW_H,
            "the log made it taller: {short:?} → {tall:?}"
        );
        let foot = app.canvas_rect.bottom() - DOCK;
        assert!(
            (tall.bottom() - foot).abs() < 1.0,
            "docked {DOCK} above the canvas's foot: {tall:?}, canvas {:?}",
            app.canvas_rect
        );
    }

    /// The card keeps its foot where it docks on every frame: with a list of two
    /// sections, and on the frame its list shrinks to nothing.
    /// flip: the area left constrained → fails at "the frame it shrank". A
    /// miscount in `list_height` does not bite: `list_frame` draws into that
    /// height, so a miscount clips a row rather than moving the card.
    #[test]
    fn the_card_keeps_its_foot_as_its_list_changes() {
        let (ctx, mut app, _) = typed("s");
        let foot = |app: &MastikApp| app.canvas_rect.bottom() - DOCK;
        let rect = |ctx: &egui::Context| {
            ctx.memory(|m| m.area_rect(egui::Id::new("command-bar")))
                .expect("the card is drawn")
        };
        let sections: std::collections::BTreeSet<_> = app
            .command_bar
            .as_ref()
            .and_then(|b| b.completed.as_ref())
            .map(|c| {
                c.completion
                    .rows
                    .iter()
                    .take(LIST_ROWS)
                    .map(|r| r.section)
                    .collect()
            })
            .unwrap_or_default();
        assert!(sections.len() >= 2, "fixture: two sections: {sections:?}");
        let tall = rect(&ctx);
        assert!(
            (tall.bottom() - foot(&app)).abs() < 0.5,
            "two sections: {tall:?}, foot {}",
            foot(&app)
        );
        // The list is worked out before the field takes the keys, so it shrinks
        // on the frame after the typing.
        pass(&ctx, &mut app, vec![egui::Event::Text("qqzx".into())], NONE);
        pass(&ctx, &mut app, Vec::new(), NONE);
        let short = rect(&ctx);
        assert!(
            short.height() < tall.height() - ROW_H,
            "fixture: the list shrank"
        );
        assert!(
            (short.bottom() - foot(&app)).abs() < 0.5,
            "the frame it shrank: {short:?}, foot {}",
            foot(&app)
        );
    }

    /// A completion row keeps its whole label and cuts its note.
    /// flip: `row_texts` laying the note out uncut → fails at "the note is cut".
    #[test]
    fn a_long_note_is_cut_before_the_label() {
        let ctx = egui::Context::default();
        crate::theme::install(&ctx);
        let row = mastik_vm::Row {
            section: mastik_vm::Section::Verbs,
            label: "set_blend_mode".into(),
            detail: "Set how layers blend with what is under them; Pass through is \
                     Normal on a layer that is not a frame or group"
                .into(),
            matched: Vec::new(),
            dim: false,
            suggestion: None,
        };
        let out = ctx.run_ui(Default::default(), |ui| {
            ui.set_width(360.0);
            complete_row(ui, None, &row, false);
        });
        let texts: Vec<_> = out
            .shapes
            .iter()
            .filter_map(|s| match &s.shape {
                egui::Shape::Text(t) => Some(t.galley.clone()),
                _ => None,
            })
            .collect();
        let label = texts
            .iter()
            .find(|g| g.text().starts_with("set_blend_mode"))
            .expect("the label is drawn");
        assert!(!label.elided, "the label is whole");
        let note = texts
            .iter()
            .find(|g| g.text().starts_with("Set how"))
            .expect("the note is drawn");
        assert!(note.elided, "the note is cut");
    }

    fn suggest(app: &MastikApp) -> Suggest {
        app.command_bar
            .as_ref()
            .expect("the bar is open")
            .suggest
            .state
    }

    /// Type `text`, and the frame after it that reads the typing.
    fn type_in(ctx: &egui::Context, app: &mut MastikApp, text: &str) {
        pass(ctx, app, vec![egui::Event::Text(text.into())], NONE);
        pass(ctx, app, Vec::new(), NONE);
    }

    /// A name's letters open the list and an operator shuts it, though the
    /// empty word after it has rows to offer.
    /// flip: `Suggester::follow` opening on any typed character → fails at "an
    /// operator shuts it".
    #[test]
    fn typing_a_name_opens_the_list_and_an_operator_shuts_it() {
        let (ctx, mut app, _) = typed(".wid");
        assert!(matches!(suggest(&app), Suggest::On(_)), "a name opens it");
        type_in(&ctx, &mut app, " = ");
        assert_eq!(suggest(&app), Suggest::Off, "an operator shuts it");
        type_in(&ctx, &mut app, "pa");
        assert!(
            matches!(suggest(&app), Suggest::On(_)),
            "a new name opens it"
        );
    }

    /// Escape hides the list and keeps the line; typing on in that word leaves
    /// it hidden, and `Ctrl+Space` brings it back.
    /// flip: `Suggester::follow` letting typing reopen a dismissed word → fails
    /// at "stays hidden".
    #[test]
    fn escape_hides_the_list_for_the_word() {
        let (ctx, mut app, _) = typed(".wi");
        pass(&ctx, &mut app, vec![key(egui::Key::Escape, NONE)], NONE);
        let bar = app.command_bar.as_ref().expect("the bar stays open");
        assert_eq!(bar.text, ".wi", "the line stays");
        assert!(matches!(bar.suggest.state, Suggest::Dismissed(_)), "hidden");
        type_in(&ctx, &mut app, "d");
        assert!(
            matches!(suggest(&app), Suggest::Dismissed(_)),
            "stays hidden"
        );
        pass(
            &ctx,
            &mut app,
            vec![key(egui::Key::Space, egui::Modifiers::CTRL)],
            egui::Modifiers::CTRL,
        );
        assert!(matches!(suggest(&app), Suggest::On(_)), "Ctrl+Space");
    }

    /// Enter takes a row that would change the word, and runs a line whose word
    /// is already whole.
    /// flip: Enter always running → fails at "Enter took the row".
    #[test]
    fn enter_takes_a_row_that_changes_the_word() {
        let (ctx, mut app, card) = typed(".widt");
        pass(&ctx, &mut app, vec![key(egui::Key::Enter, NONE)], NONE);
        assert_eq!(
            app.command_bar.as_ref().unwrap().text,
            ".width",
            "Enter took the row"
        );
        type_in(&ctx, &mut app, " = 40");
        pass(&ctx, &mut app, vec![key(egui::Key::Enter, NONE)], NONE);
        let w = app
            .session
            .resolved
            .world_bounds(card)
            .expect("a box")
            .width();
        assert_eq!(w, 40.0, "the line ran");
    }

    /// Opening the bar deals labels to what is in view; typing a label shows
    /// the labels on the canvas, the label leads the list, and its row names
    /// its layer for the canvas to outline.
    /// flip: `open_command_bar` not dealing → fails at "the bar dealt";
    /// `by_best_section` keeping the fixed order → fails at "the label leads";
    /// `labels_typed` ignoring the suggest state → fails at "an operator hides them".
    #[test]
    fn the_bar_shows_the_labels_it_offers() {
        let (ctx, mut app, card) = typed("");
        let label = app
            .leap_labels()
            .iter()
            .find(|(_, id)| **id == card)
            .map(|(k, _)| k.clone())
            .expect("the bar dealt");
        type_in(&ctx, &mut app, &label);
        let bar = app.command_bar.as_ref().unwrap();
        assert_eq!(bar.labels_typed().as_deref(), Some(label.as_str()));
        let top = &bar.completed.as_ref().unwrap().completion.rows[0];
        assert_eq!(
            (top.section, top.label.as_str()),
            (mastik_vm::Section::Labels, label.as_str()),
            "the label leads"
        );
        assert_eq!(bar.lit_layer(), Some(card), "the lit row's layer");
        type_in(&ctx, &mut app, " + ");
        let bar = app.command_bar.as_ref().unwrap();
        assert_eq!(bar.labels_typed(), None, "an operator hides them");
        assert_eq!(bar.lit_layer(), None);
    }

    /// Counts read as the status line says them.
    #[test]
    fn counts_say_only_what_is_not_zero() {
        let c = |made, changed, deleted| {
            host::Counts {
                made,
                changed,
                deleted,
                scripts: 0,
            }
            .said()
        };
        assert_eq!(c(0, 6, 0).as_deref(), Some("6 layers changed"));
        assert_eq!(c(12, 2, 0).as_deref(), Some("12 layers made, 2 changed"));
        assert_eq!(c(0, 0, 1).as_deref(), Some("1 layer deleted"));
        assert_eq!(c(0, 0, 0), None);
        let scripts = host::Counts {
            scripts: 1,
            ..Default::default()
        };
        assert_eq!(scripts.said().as_deref(), Some("1 script changed"));
    }
}
