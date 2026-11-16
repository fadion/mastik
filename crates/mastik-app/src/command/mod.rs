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
pub(crate) mod script_help;
pub(crate) mod suggest;

use crate::app::MastikApp;
use crate::assistant::{Asked, BarReply};
use crate::mcp::access::Level;
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
    /// The line's subject: a label naming one of these wears the selection's blue.
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

    /// Each layer `text` names, with its hue: the selection's blue for a
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
                true => color::SELECT,
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

/// A live read's value: as the bar shows it, and as `Enter` copies it.
#[derive(Clone, Debug)]
struct ReadValue {
    shown: String,
    copied: String,
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
    /// The live read: the text, revision and subject it was taken for, and its
    /// value.
    read: Option<(String, u64, Vec<NodeId>, Option<ReadValue>)>,
    /// The log rows picked for *Copy as script*: first and last, by their place
    /// in the log (`log::Entry::seq`), which an edit logged meanwhile leaves alone.
    picked: Option<(u64, u64)>,
    /// The log's context menu, where it opened.
    log_menu: Option<egui::Pos2>,
    /// The log menu's row the arrows lit, if they have.
    log_menu_lit: Option<usize>,
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
    /// `/ai`: the request and where it stands.
    ai: Option<Ai>,
}

/// The command bar's `/ai` request, from sending to what its reply came to.
#[derive(Clone, Debug)]
enum Ai {
    /// Waiting for the assistant's program to be found, then sent with the
    /// labels it names.
    Waiting {
        request: String,
        named: Vec<(String, NodeId)>,
    },
    /// Sent; its reply is the assistant thread's message at `msg`.
    Asking { request: String, msg: usize },
    /// The reply ended: a script `Enter` runs, proposals it applies, or a
    /// sentence it closes.
    Replied { request: String, reply: BarReply },
}

impl Ai {
    fn request(&self) -> &str {
        match self {
            Ai::Waiting { request, .. }
            | Ai::Asking { request, .. }
            | Ai::Replied { request, .. } => request,
        }
    }
}

/// The request of a line reading `/ai` and a request, `None` for any other
/// line. A bare `/ai` is one with an empty request.
fn ai_request(line: &str) -> Option<&str> {
    let rest = line.trim().strip_prefix("/ai")?;
    match rest.chars().next() {
        None => Some(""),
        Some(c) if c.is_whitespace() => Some(rest.trim()),
        Some(_) => None,
    }
}

/// An error the bar shows that no run raised: a refusal in words.
fn said(message: String) -> mastik_vm::VmError {
    mastik_vm::VmError {
        kind: mastik_vm::ErrorKind::Error,
        message,
        line: None,
        span: None,
        fix: None,
    }
}

/// How long an `/ai` step's name runs before it is cut.
const AI_LABEL_CHARS: usize = 40;

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
    // Inside a string the backtick is Luau's own, an interpolated string's
    // quote; a quote in a comment opens no string.
    let caret = caret_of(ctx, text);
    let quoted = code_keys::caret_in(text, caret) == code_keys::In::String;
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

/// What the list above the field shows.
enum List {
    None,
    Log,
    Complete(mastik_vm::Completion),
    /// An `/ai` reply as one row under its section label.
    Reply(ReplyRow),
}

/// The one row the list shows for an `/ai` reply.
#[derive(Default)]
struct ReplyRow {
    /// The section label: *Script*, *Proposed* or *Reply*.
    label: &'static str,
    line: String,
    /// Whether the line is code, in the code face.
    mono: bool,
    /// A faint note at the row's right end.
    note: Option<String>,
}

/// The row for `reply`. A script of one line shows itself. A longer one shows
/// the names its code calls and sets (`mastik_vm::names_used`), read from the
/// code, with the model's sentence beside them as a note: the sentence is the
/// model's own account, so it never stands alone for code that runs on `Enter`.
fn reply_row(reply: &BarReply) -> ReplyRow {
    let row = |label, line: String| ReplyRow {
        label,
        line,
        ..Default::default()
    };
    match reply {
        BarReply::Script { script, said } => {
            let n = script.lines().count();
            if n == 1 {
                return ReplyRow {
                    mono: true,
                    ..row("Script", script.trim().to_owned())
                };
            }
            let names = mastik_vm::names_used(script);
            let account = said.clone().unwrap_or_else(|| format!("{n} lines"));
            match names.is_empty() {
                true => row("Script", account),
                false => ReplyRow {
                    mono: true,
                    note: Some(account),
                    ..row("Script", names.join(", "))
                },
            }
        }
        BarReply::Proposals { ids, said } => {
            let line = said.clone().unwrap_or_else(|| match ids.len() {
                1 => "1 proposed edit".to_owned(),
                n => format!("{n} proposed edits"),
            });
            row("Proposed", line)
        }
        BarReply::Said(said) => row(
            "Reply",
            said.clone()
                .unwrap_or_else(|| "Done: the reply is in the assistant panel".to_owned()),
        ),
    }
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
            return match mastik_vm::run_line(&mut h, &env, line) {
                Ok(outcome) => {
                    // Copied as a field takes it: a string without its quotes.
                    if let Some(v) = outcome.copied {
                        ctx.copy_text(v);
                    }
                    // A read that prints (`print(.x)`) has no value: its last
                    // printed line is the result, as a write's is.
                    let result = outcome
                        .value
                        .map(|v| format!("= {v}"))
                        .or_else(|| outcome.printed.last().cloned())
                        .unwrap_or_default();
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

    /// An `/ai` line's footer: where its request stands, what the keys do with
    /// its reply, the right edge's words and the mark that leads it.
    fn bar_ai_footer(&self, bar: &Bar) -> (String, String, Mark) {
        let who = self
            .prefs
            .assistant
            .harness()
            .map_or_else(|| "the assistant".to_owned(), |h| h.speaker().to_owned());
        match &bar.ai {
            Some(Ai::Waiting { .. }) => (
                self.prefs.assistant.harness().map_or_else(
                    || "Looking for the assistant…".to_owned(),
                    |h| format!("Looking for {}…", h.label()),
                ),
                "Esc clears".to_owned(),
                Mark::Working,
            ),
            Some(Ai::Asking { .. }) => (
                format!("Asking {who} · the reply shows in the assistant panel"),
                "Esc clears".to_owned(),
                Mark::Working,
            ),
            Some(Ai::Replied { reply, .. }) => match reply {
                BarReply::Script { script, .. } => {
                    let n = script.lines().count();
                    (
                        match self.assistant_level() {
                            Level::Look => "Enter opens it in the editor · the chat can only look",
                            _ => "Enter runs it as one step · Tab opens it in the editor",
                        }
                        .to_owned(),
                        format!("{n} {}", if n == 1 { "line" } else { "lines" }),
                        Mark::None,
                    )
                }
                BarReply::Proposals { ids, .. } => (
                    match ids.len() {
                        1 => "Enter applies it · Tab opens the assistant panel".to_owned(),
                        _ => "Enter applies them · Tab opens the assistant panel".to_owned(),
                    },
                    "Esc clears".to_owned(),
                    Mark::Waiting,
                ),
                BarReply::Said(_) => (
                    "Tab opens the assistant panel".to_owned(),
                    "Enter clears".to_owned(),
                    Mark::None,
                ),
            },
            None => (
                format!("Enter asks {who}"),
                "` picks".to_owned(),
                Mark::None,
            ),
        }
    }

    /// An `/ai` request on its way: sent once the program is found, and once
    /// its turn ends, what the reply came to, or why it is gone on the error
    /// line.
    fn bar_ai_poll(&mut self, ctx: &egui::Context, bar: &mut Bar) {
        match bar.ai.clone() {
            Some(Ai::Waiting { request, named }) => self.bar_ai_send(bar, request, &named),
            Some(Ai::Asking { request, msg }) => match self.assistant_bar_reply(msg, &request) {
                None => ctx.request_repaint_after(std::time::Duration::from_millis(250)),
                Some(Ok(reply)) => {
                    let result = match &reply {
                        BarReply::Script { .. } => "Script proposed",
                        BarReply::Proposals { ids, .. } if ids.len() == 1 => "Edit proposed",
                        BarReply::Proposals { .. } => "Edits proposed",
                        BarReply::Said(_) => "Answered",
                    };
                    self.session.command_log.ai_replied(result);
                    bar.ai = Some(Ai::Replied { request, reply });
                }
                Some(Err(why)) => {
                    bar.error = Some((bar.text.clone(), said(why)));
                    bar.ai = None;
                }
            },
            _ => {}
        }
    }

    /// Send `request` to the assistant, or wait while its program is looked
    /// for: the lookup starts on the first ask, so a refusal here would leave
    /// the bar stuck on *Looking for…* though the program is about to be found.
    fn bar_ai_send(&mut self, bar: &mut Bar, request: String, named: &[(String, NodeId)]) {
        match self.assistant_ask_bar(&request, named) {
            Asked::Sent(msg) => {
                self.session.command_log.asked(&request);
                bar.ai = Some(Ai::Asking { request, msg });
            }
            Asked::Looking => {
                bar.ai = Some(Ai::Waiting {
                    request,
                    named: named.to_vec(),
                })
            }
            Asked::Refused(why) => {
                bar.error = Some((bar.text.clone(), said(why)));
                bar.ai = None;
            }
        }
    }

    /// `Enter` on an `/ai` line: act on what its reply came to (run the script,
    /// apply the proposals, or close on its sentence), else send the request
    /// with the layers its labels name; nothing while it is on its way.
    fn bar_ai_enter(&mut self, bar: &mut Bar, named: &[(String, NodeId)]) {
        let Some(request) = ai_request(&bar.text).map(str::to_owned) else {
            return;
        };
        match bar.ai.take() {
            Some(Ai::Replied {
                request,
                reply: BarReply::Script { script, said },
            }) => match self.run_ai_script(&request, &script) {
                None => *bar = Bar::default(),
                Some(e) => {
                    bar.error = Some((bar.text.clone(), e));
                    bar.ai = Some(Ai::Replied {
                        request,
                        reply: BarReply::Script { script, said },
                    });
                }
            },
            Some(Ai::Replied {
                reply: BarReply::Proposals { ids, .. },
                ..
            }) => {
                // Each applies as its own step, as from its card; one a later
                // edit made stale says so on the status line.
                for id in ids {
                    self.assistant_apply(id);
                }
                *bar = Bar::default();
            }
            Some(Ai::Replied {
                reply: BarReply::Said(_),
                ..
            }) => *bar = Bar::default(),
            Some(on_its_way) => bar.ai = Some(on_its_way),
            None if request.is_empty() => {
                bar.error = Some((bar.text.clone(), said("Type a request after /ai".into())));
            }
            None => self.bar_ai_send(bar, request, named),
        }
    }

    /// Run an `/ai` reply's script on the selection as one undo step named by
    /// the request, marked as the assistant's: the person runs it, the agent
    /// wrote it. The bar's labels read in it as in a line. `None` when it ran.
    fn run_ai_script(&mut self, request: &str, script: &str) -> Option<mastik_vm::VmError> {
        let selection = self.session.selection.ids().to_vec();
        let labels = self.bar_labels();
        let env = mastik_vm::Env {
            selection: &selection,
            labels: &labels,
            params: Default::default(),
        };
        let label: String = match request.chars().count() > AI_LABEL_CHARS {
            true => format!(
                "{}…",
                request.chars().take(AI_LABEL_CHARS - 1).collect::<String>()
            ),
            false => request.to_owned(),
        };
        let speaker = self
            .prefs
            .assistant
            .harness()
            .map_or_else(|| "the assistant".to_owned(), |h| h.speaker().to_owned());
        let ran = host::run_as_step(&mut self.session, &label, icon::SPARKLE, |h| {
            mastik_vm::run_script(h, &env, script)
        });
        match ran {
            Ok((outcome, counts, serial)) => {
                if serial.is_some()
                    && let Some(note) = self.session.history.current_note_mut()
                {
                    note.origin = mastik_core::Origin::Mcp {
                        client: speaker,
                        tool: "/ai".into(),
                    };
                }
                if let Some(ids) = outcome.selection {
                    self.session.selection.set(ids);
                }
                let result = outcome
                    .printed
                    .last()
                    .cloned()
                    .or_else(|| counts.said())
                    .unwrap_or_else(|| "Nothing changed".into());
                self.session
                    .command_log
                    .ai_ran(request, script.to_owned(), result, serial);
                None
            }
            // The script's place is not the field's: its span would squiggle
            // the request.
            Err(mut e) => {
                e.span = None;
                e.fix = None;
                Some(e)
            }
        }
    }

    /// The live read for the bar's text: taken again when the text, the
    /// document or the line's subject (a selection change) moved, never for a
    /// write.
    fn bar_read(&mut self, bar: &mut Bar) {
        let text = bar.text.trim().to_owned();
        let rev = self.session.revision();
        let selection = self.line_subject(&text);
        if bar
            .read
            .as_ref()
            .is_some_and(|(t, r, s, _)| *t == text && *r == rev && *s == selection)
        {
            return;
        }
        let value = (!text.is_empty()
            && ai_request(&text).is_none()
            && mastik_vm::desugar(&text).kind == mastik_vm::LineKind::Read)
            .then(|| {
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
                mastik_vm::read_line(&mut h, &env, &text)
                    .ok()
                    .and_then(|o| o.value.zip(o.copied))
                    .map(|(shown, copied)| ReadValue { shown, copied })
            })
            .flatten();
        bar.read = Some((text, rev, selection, value));
    }

    /// Draw the open bar and act on this frame's keys.
    pub(crate) fn command_bar_ui(
        &mut self,
        ctx: &egui::Context,
        canvas: egui::Rect,
        mut keys: Keys,
    ) {
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
        // An edited `/ai` line is another request: the one sent goes on in the
        // assistant panel, and its script is not this line's.
        if bar
            .ai
            .as_ref()
            .is_some_and(|a| ai_request(&bar.text) != Some(a.request()))
        {
            bar.ai = None;
        }
        self.bar_ai_poll(ctx, &mut bar);
        let ai_line = ai_request(&bar.text).is_some();
        // Typing `/ai` starts the program's lookup, so `Enter` seldom waits.
        if ai_line && bar.ai.is_none() {
            let _ = self.assistant_blocked();
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
        let completion =
            (!picking && !ai_line && !bar.text.trim().is_empty() && bar.recalled.is_none())
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
        // The menu belongs to the log's rows, which a typed line replaces: typing
        // closes it, so Enter runs the line.
        if bar.log_menu.is_some() && !bar.text.trim().is_empty() && bar.recalled.is_none() {
            bar.log_menu = None;
        }
        // While the log's menu is open, the arrows and Enter are its own, as a
        // context menu's: nothing is recalled or run behind it.
        let mut menu_nav = None;
        if bar.log_menu.is_some() {
            use crate::menu::{Nav, Step};
            menu_nav = match (keys.down, keys.up, keys.enter) {
                (true, _, _) => Some(Nav::Move(Step::Next)),
                (_, true, _) => Some(Nav::Move(Step::Prev)),
                (_, _, true) => Some(Nav::Activate),
                _ => None,
            };
            (keys.up, keys.down, keys.enter) = (false, false, false);
        }
        let replied = match &bar.ai {
            Some(Ai::Replied { reply, .. }) => Some(reply.clone()),
            _ => None,
        };
        let list = match completion {
            // Picking keeps the canvas clear: the field sleeps and lists nothing.
            _ if picking => List::None,
            _ if replied.is_some() => {
                List::Reply(replied.as_ref().map(reply_row).unwrap_or_default())
            }
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
        // Tab takes a reply where it reads whole: a script to the editor, any
        // other to the assistant panel. At *Look* the chat may not edit, so its
        // script never runs from one key: Enter opens it in the editor too.
        let to_editor = keys.enter
            && self.assistant_level() == Level::Look
            && matches!(replied, Some(BarReply::Script { .. }));
        if (keys.tab || to_editor)
            && let Some(reply) = replied
        {
            self.end_bar_session();
            match reply {
                BarReply::Script { script, .. } => self.open_script_editor_with(script),
                _ if !self.assistant.open => self.toggle_assistant(),
                _ => {}
            }
            return;
        }
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
                .filter(|e| matches!(e.source, log::Source::Bar | log::Source::Ai))
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
        if keys.enter && !accepted && ai_line {
            let named: Vec<(String, NodeId)> = pills
                .named(&bar.text)
                .into_iter()
                .map(|(r, id)| (bar.text[r].to_owned(), id))
                .collect();
            self.bar_ai_enter(&mut bar, &named);
            ctx.request_repaint();
        } else if keys.enter && !accepted {
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
        let typed = self.pick_typed();
        let pick_hint = self.pick_hint();
        let mut focus_click: Option<bool> = None;
        let parsed = mastik_vm::desugar(bar.text.trim());
        let ai_footer = ai_line.then(|| self.bar_ai_footer(&bar));
        let mut clicked_row: Option<usize> = None;
        let mut hovered_row: Option<usize> = None;
        let row_layers = bar
            .completed
            .as_ref()
            .map(|c| c.layers.clone())
            .unwrap_or_default();
        let mut pointed_pill: Option<NodeId> = None;
        // Log rows by their place in the log, never by index.
        let mut log_click: Option<(u64, bool)> = None;
        let mut log_menu_at: Option<(u64, egui::Pos2)> = None;
        area.show(ctx, |ui| {
            crate::settings::card_frame().show(ui, |ui| {
                ui.set_width(W);
                ui.spacing_mut().item_spacing = egui::vec2(0.0, 0.0);
                match &list {
                    List::None => {}
                    List::Log => {
                        let log = &self.session.command_log;
                        let first = log.entries().len().saturating_sub(LOG_ROWS);
                        list_frame(ui, list_h, |ui| {
                            palette::section_label(ui, "Log");
                            for e in log.entries().skip(first) {
                                let resp = log_row(ui, e, picks(bar.picked, e));
                                if resp.clicked() {
                                    let shift = ui.input(|i| i.modifiers.shift);
                                    log_click = Some((e.seq(), shift));
                                }
                                if resp.secondary_clicked()
                                    && let Some(p) = resp.interact_pointer_pos()
                                {
                                    log_menu_at = Some((e.seq(), p));
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
                    List::Reply(row) => list_frame(ui, list_h, |ui| reply_rows(ui, row)),
                }
                let error = bar.error.as_ref().map(|(_, e)| e.clone());
                let error = error.as_ref();
                let (value, copied) = bar
                    .read
                    .as_ref()
                    .and_then(|(_, _, _, v)| v.clone())
                    .map(|v| (v.shown, v.copied))
                    .unzip();
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
                        let (left, _) = hint(pick_hint);
                        footer(ui, &left, false, "` types");
                    }
                    Some(e) => error_line(ui, e),
                    None if ai_footer.is_some() => {
                        if let Some((left, right, mark)) = &ai_footer {
                            footer_marked(ui, &hint(left).0, false, right, *mark);
                        }
                    }
                    None => {
                        // The Luau in the code face; a hint about keys in the
                        // caption face, spelled by `input::keys` as the palette's.
                        // The subject is the head row's; the right edge says how
                        // to pick one.
                        let picks = "` picks".to_owned();
                        let (left, right) = match (&list, &copied) {
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
        if let Some((seq, shift)) = log_click {
            bar.picked = match (shift, bar.picked) {
                (true, Some((a, _))) => Some((a, seq)),
                _ => Some((seq, seq)),
            };
            if !shift
                && let Some(e) = self.session.command_log.entries().find(|e| e.seq() == seq)
                && let Some(line) = e.line.clone()
                && !matches!(e.source, log::Source::Script(_))
            {
                bar.text = line;
                bar.caret_to = Some(bar.text.len());
                bar.suggest.seen = bar.text.clone();
            }
        }
        if let Some((seq, at)) = log_menu_at {
            if !bar
                .picked
                .is_some_and(|(a, b)| (a.min(b)..=a.max(b)).contains(&seq))
            {
                bar.picked = Some((seq, seq));
            }
            bar.log_menu = Some(at);
            bar.log_menu_lit = None;
        }
        if let Some(at) = bar.log_menu {
            let picked: Vec<&log::Entry> = self
                .session
                .command_log
                .entries()
                .filter(|e| picks(bar.picked, e))
                .collect();
            match log_menu_ui(ctx, at, &picked, &mut bar.log_menu_lit, menu_nav) {
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
}

/// Whether log entry `e` is among the `picked` rows.
fn picks(picked: Option<(u64, u64)>, e: &log::Entry) -> bool {
    picked.is_some_and(|(a, b)| (a.min(b)..=a.max(b)).contains(&e.seq()))
}

/// The log's right-click menu over the `picked` entries: *Copy N lines as
/// script* and *Open in editor*, its rows taking the arrows and Enter (`nav`)
/// as a context menu's do, `lit` the row they lit.
fn log_menu_ui(
    ctx: &egui::Context,
    at: egui::Pos2,
    picked: &[&log::Entry],
    lit: &mut Option<usize>,
    nav: Option<crate::menu::Nav>,
) -> LogMenu {
    use crate::menu::{Nav, navigate_index, pointer_moved};
    if picked.is_empty() {
        return LogMenu::Closed;
    }
    let script = as_script(picked);
    let copy = match picked.len() {
        1 => "Copy as script".to_owned(),
        n => format!("Copy {n} lines as script"),
    };
    let rows = [
        (icon::COPY, copy.as_str()),
        (icon::FILE_CODE, "Open in editor"),
    ];
    let mut chosen = None;
    match nav {
        Some(Nav::Move(step)) => *lit = navigate_index(rows.len(), *lit, step),
        Some(Nav::Activate) => chosen = *lit,
        Some(Nav::Open | Nav::Back) => {}
        None if pointer_moved(ctx) => *lit = None,
        None => {}
    }
    let size = egui::vec2(crate::menu::MENU_W, crate::ui::menu_height(rows.len(), 0));
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
                for (i, (glyph, label)) in rows.into_iter().enumerate() {
                    let mut row = crate::ui::MenuRow::new(glyph, label);
                    if let Some(hl) = *lit {
                        row = row.highlight(i == hl);
                    }
                    if crate::ui::menu_row(ui, row, crate::ui::MENU_ROW_H).clicked() {
                        chosen = Some(i);
                    }
                }
            });
        });
    match chosen {
        Some(0) => {
            ctx.copy_text(script);
            LogMenu::Copied
        }
        Some(_) => LogMenu::Editor(script),
        None if ctx.input(|i| i.pointer.any_pressed()) && !resp.response.contains_pointer() => {
            LogMenu::Closed
        }
        None => LogMenu::Open,
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
pub(crate) fn as_script(entries: &[&log::Entry]) -> String {
    let mut out = String::new();
    for e in entries {
        match (&e.luau, &e.source) {
            (Some(luau), _) => out.push_str(luau),
            (None, log::Source::Script(name)) => {
                out.push_str(&format!("-- run {}", mastik_vm::quoted(name)));
            }
            (None, log::Source::Edit { label, .. }) => {
                out.push_str(&format!("-- {label}: no command yet"));
            }
            (None, log::Source::Ai) => {
                out.push_str(&format!("-- {}", e.line.as_deref().unwrap_or("/ai")));
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
        List::Reply { .. } => stack_height(1, 1),
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
/// and a squiggle under the token an error names. Asleep (`sleep`, what Pick has
/// half typed) while the bar picks: the line is kept but dimmed and unfocused,
/// and the right edge shows what is half typed. Answers the layer of
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
        // Already spelled for show (`MastikApp::pick_typed`): a filter keeps its case.
        Some(typed) => (!typed.is_empty()).then(|| (format!("{typed}…"), color::leap())),
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
            .map_or(color::SELECT, |(_, c)| *c);
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
                color::SELECT,
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
            ..Default::default()
        },
    );
}

/// The footer: the Luau (`code`) or a hint on the left, a hint or the read's
/// note on the right.
fn footer(ui: &mut egui::Ui, left: &str, code: bool, right: &str) {
    footer_marked(ui, left, code, right, Mark::None);
}

/// What leads an `/ai` footer: nothing, the spinner while the request is on its
/// way, or the proposal's sparkle in amber while it waits to be applied.
#[derive(Clone, Copy, Debug, PartialEq)]
enum Mark {
    None,
    Working,
    Waiting,
}

/// [`footer`] led by `mark`.
fn footer_marked(ui: &mut egui::Ui, left: &str, code: bool, right: &str, mark: Mark) {
    let face = match code {
        true => egui::FontId::monospace(crate::ui::CAPTION_PT),
        false => palette::caption_font(),
    };
    palette::footer_line(
        ui,
        FooterLine {
            glyph: (mark == Mark::Waiting).then(|| (icon::SPARKLE, color::warn())),
            spinner: (mark == Mark::Working).then(theme::text::dim),
            left: (left, face, theme::text::dim()),
            right: Some((right, theme::text::muted())),
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
        log::Source::Ai => icon::SPARKLE,
        log::Source::Script(_) => icon::FILE_CODE,
        log::Source::Edit { glyph, .. } if !glyph.is_empty() => glyph.as_ref(),
        log::Source::Edit { .. } => icon::CURSOR,
    };
    palette::row_glyph(&p, rect, glyph, theme::text::dim());
    let side = match &e.source {
        log::Source::Edit { label, .. } => label.clone(),
        log::Source::Script(_) => format!("Script · {}", e.result),
        log::Source::Bar | log::Source::Ai => e.result.clone(),
    };
    let side_ink = match e.failed {
        true => color::error(),
        false => theme::text::faint(),
    };
    let (text, ink) = match (&e.line, &e.source) {
        (Some(line), log::Source::Bar | log::Source::Ai) => (line.as_str(), color::text()),
        (Some(line), _) => (line.as_str(), theme::text::dim()),
        (None, _) => ("— no command yet —", theme::text::faint()),
    };
    let job = palette::lit_label(text, &[], row_mono(), ink, ink);
    palette::row_texts(&p, rect, rect.right() - 8.0, job, Some((&side, side_ink)));
    resp
}

/// An `/ai` reply's one row under its section label ([`reply_row`]): code in
/// the monospace face, a sentence in the rows' own, its note faint as the
/// log's results are.
fn reply_rows(ui: &mut egui::Ui, row: &ReplyRow) {
    palette::section_label(ui, row.label);
    let (rect, _) = ui.allocate_exact_size(
        egui::vec2(ui.available_width(), ROW_H),
        egui::Sense::hover(),
    );
    // The sparkle every `/ai` row wears, in the log's glyph ink.
    palette::row_glyph(ui.painter(), rect, icon::SPARKLE, theme::text::dim());
    let font = match row.mono {
        true => row_mono(),
        false => palette::row_font(),
    };
    let job = palette::lit_label(&row.line, &[], font, color::text(), color::text());
    let note = row.note.as_deref().map(|n| (n, theme::text::faint()));
    palette::row_texts(ui.painter(), rect, rect.right() - 8.0, job, note);
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
        let _ = pass_out(ctx, app, events, modifiers);
    }

    /// `pass`, answering the frame's output.
    fn pass_out(
        ctx: &egui::Context,
        app: &mut MastikApp,
        events: Vec<egui::Event>,
        modifiers: egui::Modifiers,
    ) -> egui::FullOutput {
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
        ctx.run_ui(input, |ui| eframe::App::ui(app, ui, &mut frame))
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

    /// A selected layer named in the line wears the selection's blue; the others
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
                (sel, color::SELECT),
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
            value.and_then(|r| r.3).map(|v| v.shown).as_deref(),
            Some("20"),
            "the live value"
        );
        let depth = app.session.history.undo_depth();
        pass(&ctx, &mut app, vec![key(egui::Key::Enter, NONE)], NONE);
        assert_eq!(app.session.history.undo_depth(), depth);
        let last = app.session.command_log.entries().last().unwrap();
        assert_eq!(last.result, "= 20");
    }

    /// The live read follows the selection: another layer selected under the
    /// bar shows that layer's value, which Enter then copies.
    /// flip: `bar_read`'s cache keyed without the subject → fails at "the
    /// second layer's".
    #[test]
    fn the_live_read_follows_the_selection() {
        let (ctx, mut app, card) = typed(".width");
        let other = add_rect(&mut app);
        app.session.selection.set_one(other);
        assert!(app.run_bar_line(&ctx, ".width = 50").is_none());
        app.session.selection.set_one(card);
        pass(&ctx, &mut app, Vec::new(), NONE);
        let shown = |app: &MastikApp| {
            let read = app.command_bar.as_ref().and_then(|b| b.read.clone());
            read.and_then(|r| r.3).map(|v| v.shown)
        };
        assert_eq!(shown(&app).as_deref(), Some("10"), "fixture: the first's");
        app.session.selection.set_one(other);
        pass(&ctx, &mut app, Vec::new(), NONE);
        assert_eq!(shown(&app).as_deref(), Some("50"), "the second layer's");
    }

    /// Enter on a read that prints logs what it printed.
    /// flip: the read arm building its result from the value alone → fails at
    /// "what it printed".
    #[test]
    fn enter_on_a_read_that_prints_logs_the_print() {
        let (ctx, mut app, _) = typed("print(.width)");
        pass(&ctx, &mut app, vec![key(egui::Key::Enter, NONE)], NONE);
        let last = app.session.command_log.entries().last().unwrap();
        assert_eq!(last.result, "10", "what it printed");
    }

    /// Enter on a read of a string copies it without Luau's quotes, as the
    /// footer says it will.
    /// flip: `run_bar_line` copying the shown value → fails at "no quotes".
    #[test]
    fn enter_copies_a_strings_text_without_its_quotes() {
        let (ctx, mut app, _) = typed(".name");
        let read = app.command_bar.as_ref().and_then(|b| b.read.clone());
        let value = read
            .and_then(|r| r.3)
            .expect("fixture: the read shows a value");
        assert_eq!(value.shown, "\"Card\"", "fixture: shown as Luau writes it");
        assert_eq!(value.copied, "Card", "the footer's Enter copies Card");
        let out = pass_out(&ctx, &mut app, vec![key(egui::Key::Enter, NONE)], NONE);
        let copied: Vec<&egui::OutputCommand> = out
            .platform_output
            .commands
            .iter()
            .filter(|c| matches!(c, egui::OutputCommand::CopyText(_)))
            .collect();
        assert_eq!(
            copied,
            [&egui::OutputCommand::CopyText("Card".into())],
            "no quotes"
        );
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
        let bar = app.command_bar.as_mut().unwrap();
        // A recalled line keeps the log's rows, and so their menu.
        bar.recalled = Some(".x".into());
        bar.log_menu = Some(egui::pos2(400.0, 400.0));
        pass(&ctx, &mut app, vec![key(egui::Key::Escape, NONE)], NONE);
        let bar = app.command_bar.as_ref().expect("the bar stays open");
        assert_eq!(bar.text, ".x", "the line stays");
        assert!(bar.log_menu.is_none(), "the menu closed");
    }

    /// A line typed while the log's menu is open closes the menu, so Enter
    /// runs it.
    /// flip: `command_bar_ui` keeping the menu over a typed line → fails at
    /// "the line ran".
    #[test]
    fn typing_over_the_log_menu_closes_it() {
        let (ctx, mut app, card) = typed("");
        app.session.command_log.push(log::Entry::typed(
            ".y = 1",
            "sel.y = 1".into(),
            String::new(),
            false,
            None,
        ));
        let newest = app.session.command_log.entries().last().unwrap().seq();
        let bar = app.command_bar.as_mut().unwrap();
        bar.picked = Some((newest, newest));
        bar.log_menu = Some(egui::pos2(400.0, 400.0));
        pass(&ctx, &mut app, Vec::new(), NONE);
        assert!(
            app.command_bar.as_ref().unwrap().log_menu.is_some(),
            "fixture: the menu is open"
        );
        pass(
            &ctx,
            &mut app,
            vec![egui::Event::Text(".x = 5".into())],
            NONE,
        );
        pass(&ctx, &mut app, Vec::new(), NONE);
        pass(&ctx, &mut app, vec![key(egui::Key::Enter, NONE)], NONE);
        assert_eq!(left(&app, card), 5.0, "the line ran");
        assert!(app.command_bar.as_ref().unwrap().log_menu.is_none());
    }

    /// `` ` `` picks unless the caret is in a string, where it is Luau's quote:
    /// a `'` in a comment opens no string.
    /// flip: `read_keys` reading quotes without comments → fails at "after a comment".
    #[test]
    fn a_backtick_picks_past_a_comment_but_not_in_a_string() {
        let picks = |text: &str| {
            let ctx = egui::Context::default();
            let mut pick = None;
            let input = egui::RawInput {
                events: vec![key(egui::Key::Backtick, NONE)],
                ..Default::default()
            };
            let _ = ctx.run_ui(input, |ui| pick = Some(read_keys(ui.ctx(), text).pick));
            pick.unwrap()
        };
        assert!(picks(".x = 1 -- it's"), "after a comment");
        assert!(!picks("print(\"a"), "in a string");
        assert!(picks(".x = 1"));
    }

    /// With the log's menu open, the arrows light its rows and Enter takes the
    /// lit one: nothing is recalled behind it. The picked rows stay on their
    /// entries when a full log drops its oldest.
    /// flip: `command_bar_ui` leaving the arrows to the recall while the menu is
    /// open → fails at "nothing recalled".
    #[test]
    fn the_log_menu_takes_the_arrows_and_its_rows_stay_picked() {
        let (ctx, mut app, _) = typed("");
        for i in 0..200 {
            app.session.command_log.push(log::Entry::typed(
                &format!(".x = {i}"),
                format!("sel.x = {i}"),
                String::new(),
                false,
                None,
            ));
        }
        let newest = app.session.command_log.entries().last().unwrap().seq();
        let bar = app.command_bar.as_mut().unwrap();
        bar.picked = Some((newest, newest));
        bar.log_menu = Some(egui::pos2(400.0, 400.0));
        pass(&ctx, &mut app, Vec::new(), NONE);
        // Up from none lights the last row, and again the first.
        pass(&ctx, &mut app, vec![key(egui::Key::ArrowUp, NONE)], NONE);
        pass(&ctx, &mut app, vec![key(egui::Key::ArrowUp, NONE)], NONE);
        let bar = app.command_bar.as_ref().unwrap();
        assert!(bar.recalled.is_none(), "nothing recalled");
        assert_eq!(bar.log_menu_lit, Some(0), "the first row lit");

        app.session.command_log.push(log::Entry::typed(
            ".y = 1",
            "sel.y = 1".into(),
            String::new(),
            false,
            None,
        ));
        let picked: Vec<String> = app
            .session
            .command_log
            .entries()
            .filter(|e| picks(app.command_bar.as_ref().unwrap().picked, e))
            .filter_map(|e| e.line.clone())
            .collect();
        assert_eq!(picked, [".x = 199"], "still the row picked");

        pass(&ctx, &mut app, vec![key(egui::Key::Enter, NONE)], NONE);
        let bar = app.command_bar.as_ref().expect("the bar stays open");
        assert!(bar.log_menu.is_none(), "Enter took the lit row");
        assert_eq!(bar.text, "", "nothing ran or was recalled");
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

    /// `/ai` and a request is an `/ai` line; `/aim` or a request elsewhere is not.
    #[test]
    fn an_ai_line_is_read_off_its_prefix() {
        assert_eq!(ai_request("/ai move it right"), Some("move it right"));
        assert_eq!(ai_request("  /ai  "), Some(""));
        assert_eq!(ai_request("/ai"), Some(""));
        assert_eq!(ai_request("/aim at it"), None);
        assert_eq!(ai_request(".x += 5 /ai"), None);
    }

    /// `Enter` on an `/ai` line while the assistant's program is still being
    /// looked for waits instead of refusing, and asks again by itself once the
    /// lookup ends.
    ///
    /// flip: `bar_ai_send` treating `Asked::Looking` as a refusal → fails at
    /// "waits for the lookup".
    #[test]
    fn an_ai_request_waits_for_the_program_lookup() {
        let h = mastik_ai::Harness::Claude;
        let (ctx, mut app, _) = typed("/ai move it right");
        app.prefs.assistant.agent = h.key().into();
        app.session.path = Some(std::env::temp_dir().join("ai-wait.mastik"));
        app.hold_lookup(h, None);
        pass(&ctx, &mut app, vec![key(egui::Key::Enter, NONE)], NONE);
        let bar = app.command_bar.as_ref().unwrap();
        assert!(
            matches!(bar.ai, Some(Ai::Waiting { .. })) && bar.error.is_none(),
            "waits for the lookup"
        );
        let missing = mastik_ai::NotFound {
            program: "claude",
            looked_in: Vec::new(),
        };
        app.hold_lookup(h, Some(Err(missing)));
        pass(&ctx, &mut app, Vec::new(), NONE);
        let bar = app.command_bar.as_ref().unwrap();
        assert!(bar.ai.is_none(), "asked again once the lookup ended");
        assert!(
            bar.error
                .as_ref()
                .is_some_and(|(_, e)| !e.message.starts_with("Looking")),
            "the lookup's answer, not the wait, is on the error line"
        );
    }

    /// An `/ai` line waiting on a reply holding `text`, as a turn that ended,
    /// after the question `question` in the thread.
    fn asked_after(question: &str, text: &str) -> (egui::Context, MastikApp, NodeId) {
        use crate::assistant::thread::Part;
        asked_with(question, vec![Part::Text(text.into())])
    }

    /// [`asked_after`] with the reply's parts as given, tool rows included.
    fn asked_with(
        question: &str,
        parts: Vec<crate::assistant::thread::Part>,
    ) -> (egui::Context, MastikApp, NodeId) {
        use crate::assistant::thread::Msg;
        let (ctx, mut app, card) = typed("/ai move it right");
        app.assistant.msgs.push(Msg::Person {
            text: question.into(),
            chips: Vec::new(),
            scope: None,
        });
        let msg = app.assistant.msgs.len();
        app.assistant.msgs.push(Msg::Agent {
            speaker: "Claude".into(),
            harness: "claude".into(),
            parts,
            stats: None,
        });
        app.command_bar.as_mut().unwrap().ai = Some(Ai::Asking {
            request: "move it right".into(),
            msg,
        });
        pass(&ctx, &mut app, Vec::new(), NONE);
        (ctx, app, card)
    }

    /// [`asked_after`] the bar's own question.
    fn asked(text: &str) -> (egui::Context, MastikApp, NodeId) {
        asked_after("/ai move it right", text)
    }

    /// A reply whose question is no longer this request (a new chat, another
    /// document's thread) proposes nothing, however much script it holds.
    ///
    /// flip: `assistant_bar_reply` not checking the question → fails at "not
    /// this request's".
    #[test]
    fn a_reply_to_another_question_proposes_nothing() {
        let (_ctx, app, _) = asked_after("what is this?", "```luau\nsel.x += 40\n```");
        let bar = app.command_bar.as_ref().unwrap();
        assert!(bar.ai.is_none(), "not this request's");
        assert!(bar.error.is_some());
    }

    /// An `/ai` reply's script shows in the bar, and `Enter` runs it on the
    /// selection as one step named by the request and marked as the
    /// assistant's, logged as the `/ai` line.
    ///
    /// flip: `run_ai_script` leaving the step's origin as the person's → fails
    /// at "marked as the assistant's".
    #[test]
    fn an_ai_reply_runs_as_one_step() {
        let (ctx, mut app, card) = asked("Moves it.\n\n```luau\nsel.x += 40\n```");
        assert!(
            matches!(
                &app.command_bar.as_ref().unwrap().ai,
                Some(Ai::Replied { reply: BarReply::Script { script, .. }, .. })
                    if script == "sel.x += 40"
            ),
            "the script is proposed"
        );
        let (x, depth) = (left(&app, card), app.session.history.undo_depth());
        pass(&ctx, &mut app, vec![key(egui::Key::Enter, NONE)], NONE);
        assert_eq!(left(&app, card), x + 40.0);
        assert_eq!(app.session.history.undo_depth(), depth + 1);
        let top = app.session.history.done().next_back().unwrap();
        assert_eq!(top.note.label, "move it right");
        assert!(
            matches!(&top.note.origin, mastik_core::Origin::Mcp { tool, .. } if tool == "/ai"),
            "marked as the assistant's"
        );
        let last = app.session.command_log.entries().last().unwrap();
        assert_eq!(last.line.as_deref(), Some("/ai move it right"));
        assert_eq!(app.command_bar.as_ref().map(|b| b.text.as_str()), Some(""));
    }

    /// A longer `/ai` script's row leads with the names its code calls and
    /// sets, the model's sentence only beside them; at *Look*, `Enter` opens it
    /// in the editor and runs nothing.
    ///
    /// flip: `command_bar_ui` running the script on Enter at *Look* → fails at
    /// "nothing ran at Look".
    #[test]
    fn an_ai_script_shows_its_names_and_waits_at_look() {
        let reply = BarReply::Script {
            script: "for _, c in ipairs(sel) do\n  c.x += 4\nend\ndelete_node()".into(),
            said: Some("Tidies the spacing.".into()),
        };
        let row = reply_row(&reply);
        assert_eq!(row.line, "sel, x, delete_node", "the names lead");
        assert_eq!(row.note.as_deref(), Some("Tidies the spacing."));

        let (ctx, mut app, card) = asked("Moves it.\n\n```luau\nsel.x += 40\nprint(1)\n```");
        app.assistant.level = Some(Level::Look);
        let (x, depth) = (left(&app, card), app.session.history.undo_depth());
        pass(&ctx, &mut app, vec![key(egui::Key::Enter, NONE)], NONE);
        assert_eq!(left(&app, card), x, "nothing ran at Look");
        assert_eq!(app.session.history.undo_depth(), depth);
        assert!(app.command_bar.is_none(), "the bar closed");
        assert!(app.scripts.open, "the editor opened on it");
    }

    /// A reply that edited the canvas itself shows only its sentence, and
    /// `Enter` clears the line without running the code block it also held.
    ///
    /// flip: `bar_reply` taking a script from a reply that wrote → fails at
    /// "the edit's sentence, not a script".
    #[test]
    fn an_ai_reply_that_edited_shows_its_sentence() {
        use crate::assistant::thread::{Part, ToolRow, ToolState};
        let wrote = ToolRow {
            id: "1".into(),
            name: "set".into(),
            arg: String::new(),
            state: ToolState::Done,
            result: "1 line".into(),
            proposal: None,
        };
        let (ctx, mut app, card) = asked_with(
            "/ai move it right",
            vec![
                Part::Tool(wrote),
                Part::Text("```luau\nsel.x += 40\n```\n\nMoved Card 40 px right.".into()),
            ],
        );
        assert!(
            matches!(
                &app.command_bar.as_ref().unwrap().ai,
                Some(Ai::Replied { reply: BarReply::Said(Some(s)), .. })
                    if s == "Moved Card 40 px right."
            ),
            "the edit's sentence, not a script"
        );
        let x = left(&app, card);
        pass(&ctx, &mut app, vec![key(egui::Key::Enter, NONE)], NONE);
        assert_eq!(left(&app, card), x, "nothing ran");
        assert_eq!(app.command_bar.as_ref().map(|b| b.text.as_str()), Some(""));
    }

    /// `Enter` on a reply that proposed an edit applies it as a step and clears
    /// the line.
    /// flip: `bar_ai_enter`'s proposals arm applying nothing → fails at "applied".
    #[test]
    fn enter_applies_an_ai_replys_proposal() {
        use crate::assistant::thread::{Part, ToolRow, ToolState};
        let proposed = ToolRow {
            id: "1".into(),
            name: "propose_edit".into(),
            arg: String::new(),
            state: ToolState::Done,
            result: String::new(),
            proposal: Some(1),
        };
        let (ctx, mut app, card) = asked_with(
            "/ai move it right",
            vec![Part::Tool(proposed), Part::Text("Renames it.".into())],
        );
        assert!(
            matches!(
                &app.command_bar.as_ref().unwrap().ai,
                Some(Ai::Replied { reply: BarReply::Proposals { ids, .. }, .. }) if ids == &[1]
            ),
            "fixture: the edit is proposed"
        );
        app.assistant.held.push(crate::mcp::propose::Proposal {
            id: 1,
            key: None,
            document: std::path::PathBuf::new(),
            title: "Rename the card".into(),
            changes: vec!["Renamed Card to Hero".into()],
            calls: vec![(
                "set_name".into(),
                serde_json::json!({ "node": card.to_wire(), "name": "Hero" }),
            )],
            rev: app.session.revision(),
            writes: vec![card],
            before: vec![app.session.doc.get(card).unwrap().clone()],
            client: "Claude".into(),
        });
        let depth = app.session.history.undo_depth();
        pass(&ctx, &mut app, vec![key(egui::Key::Enter, NONE)], NONE);
        assert_eq!(app.session.doc.get(card).unwrap().name(), "Hero", "applied");
        assert_eq!(app.session.history.undo_depth(), depth + 1, "as one step");
        assert_eq!(app.command_bar.as_ref().map(|b| b.text.as_str()), Some(""));
    }

    /// `Enter` again while an `/ai` request is on its way neither drops it nor
    /// asks twice.
    /// flip: `bar_ai_enter` dropping a request on its way (`Some(_) => {}`) →
    /// fails at "still on its way".
    #[test]
    fn enter_while_a_request_is_on_its_way_keeps_it() {
        let h = mastik_ai::Harness::Claude;
        let (ctx, mut app, _) = typed("/ai move it right");
        app.prefs.assistant.agent = h.key().into();
        app.session.path = Some(std::env::temp_dir().join("ai-twice.mastik"));
        app.hold_lookup(h, None);
        pass(&ctx, &mut app, vec![key(egui::Key::Enter, NONE)], NONE);
        let rows = app.session.command_log.entries().len();
        pass(&ctx, &mut app, vec![key(egui::Key::Enter, NONE)], NONE);
        pass(&ctx, &mut app, Vec::new(), NONE);
        let bar = app.command_bar.as_ref().unwrap();
        assert!(
            matches!(&bar.ai, Some(Ai::Waiting { request, .. }) if request == "move it right"),
            "still on its way"
        );
        assert_eq!(bar.text, "/ai move it right", "the line stays");
        assert_eq!(app.session.command_log.entries().len(), rows, "asked once");
    }

    /// One step on the selection running `script`, named `label`, its origin
    /// set to `origin`, as the log's watcher then sees it.
    fn step_as(
        ctx: &egui::Context,
        app: &mut MastikApp,
        script: &str,
        label: &str,
        origin: mastik_core::Origin,
    ) {
        let selection = app.session.selection.ids().to_vec();
        let labels = Default::default();
        let env = mastik_vm::Env {
            selection: &selection,
            labels: &labels,
            params: Default::default(),
        };
        host::run_as_step(&mut app.session, label, icon::SPARKLE, |h| {
            mastik_vm::run_script(h, &env, script)
        })
        .expect("the step ran");
        app.session.history.current_note_mut().unwrap().origin = origin;
        pass(ctx, app, Vec::new(), NONE);
    }

    /// A sent `/ai` request is one log row wearing its line, and an agent's
    /// steps after it are that row's, named as the applied proposal's step is,
    /// by the agent and not its chat; the person's own edit closes it.
    ///
    /// flip: `Log::observe` not folding the agent's step into the open
    /// request → fails at "the request's own step".
    #[test]
    fn an_ai_request_takes_the_assistants_steps_in_the_log() {
        let (ctx, mut app, _) = typed("/ai make it red");
        app.session.command_log.asked("make it red");
        let rows = app.session.command_log.entries().len();
        let agent = mastik_core::Origin::Mcp {
            client: "Claude Code".into(),
            tool: "propose_edit".into(),
        };
        step_as(&ctx, &mut app, "sel.x += 1", "Fill Card red", agent);
        let log = &app.session.command_log;
        assert_eq!(log.entries().len(), rows, "the request's own step");
        let last = log.entries().last().unwrap();
        assert_eq!(last.source, log::Source::Ai);
        assert_eq!(last.line.as_deref(), Some("/ai make it red"));
        assert_eq!(last.result, "Fill Card red");
        step_as(
            &ctx,
            &mut app,
            "sel.x += 1",
            "Nudge",
            mastik_core::Origin::User,
        );
        assert_eq!(
            app.session.command_log.entries().len(),
            rows + 1,
            "the person's edit is a row of its own"
        );
    }

    /// An agent's step the log has no command for is left out, where the
    /// person's own shows as a hole; one with a command still shows.
    ///
    /// flip: `Log::observe` logging an agent's step with no command → fails at
    /// "left out".
    #[test]
    fn an_agents_step_with_no_command_is_left_out_of_the_log() {
        let (ctx, mut app, _) = typed("x");
        let agent = || mastik_core::Origin::Mcp {
            client: "Claude Code".into(),
            tool: "set".into(),
        };
        let rows = app.session.command_log.entries().len();
        step_as(&ctx, &mut app, "sel.fill = \"#ff0000\"", "Fill", agent());
        assert_eq!(app.session.command_log.entries().len(), rows, "left out");
        step_as(&ctx, &mut app, "sel.x += 1", "Move", agent());
        assert_eq!(app.session.command_log.entries().len(), rows + 1);
        step_as(
            &ctx,
            &mut app,
            "sel.fill = \"#00ff00\"",
            "Fill",
            mastik_core::Origin::User,
        );
        let last = app.session.command_log.entries().last().unwrap();
        assert!(
            last.line.is_none() && matches!(last.source, log::Source::Edit { .. }),
            "the person's step with no command is a hole"
        );
    }

    /// Two agents' steps committed in one frame are both logged, oldest first.
    ///
    /// flip: `Log::observe` reading only the top step → fails at "both steps".
    #[test]
    fn steps_landing_in_one_frame_are_all_logged() {
        let (ctx, mut app, card) = typed("x");
        let other = add_rect(&mut app);
        pass(&ctx, &mut app, Vec::new(), NONE);
        let rows = app.session.command_log.entries().len();
        for (id, script, client) in [
            (card, "sel.x += 3", "Claude Code"),
            (other, "sel.opacity = 0.5", "Codex"),
        ] {
            let labels = Default::default();
            let env = mastik_vm::Env {
                selection: &[id],
                labels: &labels,
                params: Default::default(),
            };
            host::run_as_step(&mut app.session, "Edit", icon::SPARKLE, |h| {
                mastik_vm::run_script(h, &env, script)
            })
            .expect("the step ran");
            app.session.history.current_note_mut().unwrap().origin = mastik_core::Origin::Mcp {
                client: client.into(),
                tool: "run_script".into(),
            };
        }
        pass(&ctx, &mut app, Vec::new(), NONE);
        let lines: Vec<Option<&str>> = app
            .session
            .command_log
            .entries()
            .skip(rows)
            .map(|e| e.line.as_deref())
            .collect();
        assert_eq!(
            lines,
            [Some(".x += 3"), Some(".opacity = 0.5")],
            "both steps"
        );
    }

    /// `↑` in an empty bar recalls an `/ai` request as it recalls a typed line.
    ///
    /// flip: the recall leaving `Source::Ai` out → fails at "recalled".
    #[test]
    fn an_ai_request_is_recalled_with_the_arrows() {
        let (ctx, mut app, _) = typed("x");
        app.session.command_log.asked("make it red");
        app.command_bar.as_mut().unwrap().text.clear();
        pass(&ctx, &mut app, Vec::new(), NONE);
        pass(&ctx, &mut app, vec![key(egui::Key::ArrowUp, NONE)], NONE);
        assert_eq!(
            app.command_bar.as_ref().map(|b| b.text.as_str()),
            Some("/ai make it red"),
            "recalled"
        );
    }
}
