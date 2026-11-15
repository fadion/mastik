//! Command Mode's command bar (`docs/design/command-mode.md`): `:` opens a line
//! that runs against the selection in the scripting language, a sibling of the
//! palette's card docked at the canvas's foot. A write commits one undo step; a
//! read shows its value as it is typed; the log above the field lists past lines
//! and every other edit as the command that would make it.

pub(crate) mod editor;
pub(crate) mod host;
pub(crate) mod log;

use crate::app::MastikApp;
use crate::palette::{
    self, FIELD_H, FOOTER_H, FooterLine, PAD, ROW_GAP, ROW_GLYPH_X, ROW_H, ROW_LABEL_X, SECTION_H,
};
use crate::theme::{self, color, icon};
use eframe::egui;
use mastik_core::NodeId;

/// The card's width.
const W: f32 = 640.0;
/// Between the card and the canvas edge it docks to.
const DOCK: f32 = 24.0;
/// The most completion rows the list shows.
const LIST_ROWS: usize = 8;
/// The log lines the list shows while the field is empty.
const LOG_ROWS: usize = 6;
/// Where the field's text starts: in the rows' label column.
const TEXT_X: f32 = PAD + ROW_LABEL_X;
/// Between a read's value and the field's right edge.
const VALUE_INSET: f32 = 14.0;
/// Between the line and a read's value.
const VALUE_GAP: f32 = 10.0;

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
    /// The highlighted completion row.
    highlight: usize,
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
    /// Whether the completion list is up ([`Bar::follow_suggest`]).
    suggest: Suggest,
    /// The text as the field left it last frame, to tell typing from a
    /// keystroke-free frame.
    seen: String,
}

/// The completion list's state, as a code editor's suggest widget keeps it: it
/// opens on typing a name or a trigger, follows the word being typed, and shuts
/// when the caret leaves that word.
#[derive(Default, Clone, Copy, PartialEq, Eq, Debug)]
enum Suggest {
    #[default]
    Off,
    /// Up for the word starting at this byte.
    On(usize),
    /// Escape shut it for the word starting at this byte: typing on in that
    /// word leaves it shut, as an editor's does.
    Dismissed(usize),
}

impl Bar {
    /// Move the suggest state on from what changed since last frame. Typing a
    /// name's letter, `.` or `:`, or the space before a verb's argument opens
    /// it; any other character shuts it; deleting keeps it while the word
    /// stays, and a caret that leaves the word shuts it. `invoke` (`Ctrl+Space`)
    /// opens it on the word at the caret whatever came before.
    fn follow_suggest(&mut self, c: Option<&mastik_vm::Completion>, caret: usize, invoke: bool) {
        let start = c.map(|c| c.replace.start);
        if self.text != self.seen {
            self.highlight = 0;
            let grew = self.text.len() > self.seen.len();
            self.suggest = match (c, start) {
                (Some(c), Some(s)) if s <= caret && caret <= self.text.len() => {
                    let word = &self.text[s..caret];
                    let trigger = trigger_at(&self.text, s, c);
                    match self.suggest {
                        Suggest::Dismissed(a) if a == s => Suggest::Dismissed(a),
                        _ if grew => {
                            let typed = self.text[..caret].chars().next_back();
                            let opens = match typed {
                                Some(ch) if ch.is_alphanumeric() || ch == '_' => {
                                    !word.starts_with(|d: char| d.is_ascii_digit())
                                }
                                _ => word.is_empty() && trigger,
                            };
                            match opens {
                                true => Suggest::On(s),
                                false => Suggest::Off,
                            }
                        }
                        Suggest::On(a) if a == s && (caret > s || trigger) => Suggest::On(a),
                        _ => Suggest::Off,
                    }
                }
                _ => Suggest::Off,
            };
        }
        if let Suggest::On(a) | Suggest::Dismissed(a) = self.suggest
            && Some(a) != start
        {
            self.suggest = Suggest::Off;
        }
        if invoke && let Some(s) = start {
            self.suggest = Suggest::On(s);
        }
    }

    /// Escape on an open list: shut it for this word.
    fn dismiss(&mut self) {
        if let Suggest::On(a) = self.suggest {
            self.suggest = Suggest::Dismissed(a);
        }
    }

    /// Take completion row `i` into the field, and shut the list.
    fn accept(&mut self, c: &mastik_vm::Completion, i: usize) {
        let Some(row) = c.rows.get(i) else { return };
        let r = c.replace.clone();
        if r.end > self.text.len() {
            return;
        }
        let insert = insert_of(row);
        self.text.replace_range(r.clone(), insert);
        self.caret_to = Some(r.start + insert.len());
        self.highlight = 0;
        self.suggest = Suggest::Off;
    }
}

/// Whether an empty word at `start` sits where a list opens on its own: just
/// after `.` or `:`, or at a verb's argument, whose choices `c` holds.
fn trigger_at(text: &str, start: usize, c: &mastik_vm::Completion) -> bool {
    let before = text[..start].chars().next_back();
    matches!(before, Some('.' | ':'))
        || (!c.rows.is_empty()
            && c.rows
                .iter()
                .all(|r| r.section == mastik_vm::Section::Choices))
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
}

/// Take the keys the open bar answers. Called before any panel draws.
pub(crate) fn read_keys(ctx: &egui::Context) -> Keys {
    let none = egui::Modifiers::NONE;
    ctx.input_mut(|i| Keys {
        up: i.consume_key(none, egui::Key::ArrowUp),
        down: i.consume_key(none, egui::Key::ArrowDown),
        enter: i.consume_key(none, egui::Key::Enter),
        tab: i.consume_key(none, egui::Key::Tab),
        escape: i.consume_key(none, egui::Key::Escape),
        // Ctrl on every platform, as the editors this follows bind it.
        suggest: i.consume_key(egui::Modifiers::CTRL, egui::Key::Space),
    })
}

/// What the list above the field shows.
enum List {
    None,
    Log,
    Complete(mastik_vm::Completion),
}

impl MastikApp {
    /// `:` — open the bar, closing whatever else floats over the canvas.
    pub(crate) fn open_command_bar(&mut self) {
        if self.command_bar.is_some() {
            return;
        }
        self.palette = None;
        self.context_menu = None;
        self.open_menu = crate::app::TopMenu::None;
        self.command_bar = Some(Bar::default());
    }

    /// The selection a line acts on.
    fn bar_selection(&self) -> Vec<NodeId> {
        self.session.selection.ids().to_vec()
    }

    /// `Card and 2 more`: the first selected layer by name and how many others,
    /// or that nothing is selected.
    fn bar_subject(&self) -> String {
        let ids = self.session.selection.ids();
        let Some(first) = ids.first().and_then(|id| self.session.doc.get(*id)) else {
            return "Nothing selected".into();
        };
        match ids.len() {
            1 => first.name().to_string(),
            n => format!("{} and {} more", first.name(), n - 1),
        }
    }

    /// The leap labels lines may name, as Navigate last dealt them.
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
        let selection = self.bar_selection();
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

    /// `.` in Navigate: run the last line typed in the bar again on the selection.
    pub(crate) fn repeat_bar_line(&mut self, ctx: &egui::Context) {
        let Some(line) = self.session.command_log.last_typed().map(str::to_owned) else {
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
                let selection = self.bar_selection();
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
        let Some(mut bar) = self.command_bar.take() else {
            return;
        };
        if self.modal_is_up() {
            self.command_bar = Some(bar);
            return;
        }
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

        let caret = egui::TextEdit::load_state(ctx, field_id())
            .and_then(|s| s.cursor.char_range())
            .map(|r| char_to_byte(&bar.text, r.primary.index.0))
            .unwrap_or(bar.text.len());
        let completion = (!bar.text.trim().is_empty() && bar.recalled.is_none())
            .then(|| self.bar_completion(&mut bar, caret));
        bar.follow_suggest(completion.as_ref(), caret, keys.suggest);
        let suggesting = matches!(bar.suggest, Suggest::On(_))
            && bar.error.is_none()
            && completion.as_ref().is_some_and(|c| !c.rows.is_empty());

        // Escape closes the innermost thing first: the log's menu, the
        // completion list, then the text, then the bar.
        if keys.escape {
            if bar.log_menu.is_some() {
                bar.log_menu = None;
            } else if suggesting {
                bar.dismiss();
            } else if bar.text.is_empty() {
                return;
            } else {
                bar = Bar::default();
            }
        }
        let list = match completion {
            _ if bar.text.trim().is_empty() || bar.recalled.is_some() => {
                match self.session.command_log.entries().len() {
                    0 => List::None,
                    _ => List::Log,
                }
            }
            Some(c) if suggesting && matches!(bar.suggest, Suggest::On(_)) => List::Complete(c),
            _ => List::None,
        };
        let rows = match &list {
            List::Complete(c) => c.rows.len().min(LIST_ROWS),
            _ => 0,
        };
        if rows > 0 {
            bar.highlight = bar.highlight.min(rows - 1);
        }

        // Keys: a list of completions takes the arrows, Tab and Enter, as an
        // editor's does; otherwise the arrows walk the typed lines, Tab takes an
        // error's fix and Enter runs.
        let mut accepted = false;
        if let List::Complete(c) = &list {
            if keys.down {
                bar.highlight = (bar.highlight + 1) % rows;
            }
            if keys.up {
                bar.highlight = (bar.highlight + rows - 1) % rows;
            }
            // Enter takes the row only when it would change the word: on a word
            // already whole, it runs the line.
            let changes = c
                .rows
                .get(bar.highlight)
                .is_some_and(|row| bar.text.get(c.replace.clone()) != Some(insert_of(row)));
            if keys.tab || (keys.enter && changes) {
                bar.accept(c, bar.highlight);
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
            bar.suggest = Suggest::Off;
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

        // Dock at the foot unless the selection reaches into that band.
        let list_h = list_height(&list, self.session.command_log.entries().len());
        let height = list_h + FIELD_H + 1.0 + FOOTER_H;
        let ppp = ctx.pixels_per_point();
        let band = egui::Rect::from_min_max(
            egui::pos2(canvas.center().x - W / 2.0, canvas.bottom() - DOCK - height),
            egui::pos2(canvas.center().x + W / 2.0, canvas.bottom()),
        );
        let covers = self.selection_bounds().is_some_and(|b| {
            let a = self.to_screen(mastik_core::kurbo::Point::new(b.x0, b.y0), canvas, ppp);
            let z = self.to_screen(mastik_core::kurbo::Point::new(b.x1, b.y1), canvas, ppp);
            egui::Rect::from_two_pos(a, z).intersects(band)
        });
        // Placed from the height worked out above rather than anchored: an anchor
        // reads last frame's size, so a growing list would push the foot down a
        // frame before it settled. The list is drawn into exactly that height.
        let outer = egui::vec2(W, height) + crate::settings::card_frame().total_margin().sum();
        let top = match covers {
            false => canvas.bottom() - DOCK - outer.y,
            true => canvas.top() + DOCK,
        };
        // `Modal::default_area` less its centring anchor, which would win over
        // the place. Not constrained: egui keeps an area on screen by last
        // frame's size, so a list that just shrank would shove the card up a frame.
        let area = egui::Area::new(egui::Id::new("command-bar"))
            .kind(egui::UiKind::Modal)
            .sense(egui::Sense::hover())
            .order(egui::Order::Foreground)
            .interactable(true)
            .constrain(false)
            .fixed_pos(egui::pos2(canvas.center().x - outer.x / 2.0, top));

        let subject = self.bar_subject();
        let parsed = mastik_vm::desugar(bar.text.trim());
        let mut clicked_row: Option<usize> = None;
        let row_layers = bar
            .completed
            .as_ref()
            .map(|c| c.layers.clone())
            .unwrap_or_default();
        let mut log_click: Option<(usize, bool)> = None;
        let mut log_menu_at: Option<(usize, egui::Pos2)> = None;
        let entries: Vec<log::Entry> = self.session.command_log.entries().cloned().collect();
        let modal = egui::Modal::new(egui::Id::new("command-bar"))
            .area(area)
            .backdrop_color(egui::Color32::TRANSPARENT)
            .frame(crate::settings::card_frame())
            .show(ctx, |ui| {
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
                            let mut section = None;
                            for (i, row) in c.rows.iter().take(LIST_ROWS).enumerate() {
                                if section != Some(row.section) {
                                    palette::section_label(ui, section_title(row.section));
                                    section = Some(row.section);
                                }
                                let layer = row_layers
                                    .get(i)
                                    .copied()
                                    .flatten()
                                    .and_then(|id| self.session.doc.get(id));
                                if complete_row(ui, layer, row, i == bar.highlight).clicked() {
                                    clicked_row = Some(i);
                                }
                            }
                        });
                    }
                }
                let error = bar.error.as_ref().map(|(_, e)| e.clone());
                let error = error.as_ref();
                let value = bar.read.as_ref().and_then(|(_, _, v)| v.clone());
                // What the field changes from here on is typing, which next
                // frame's `follow_suggest` reads against this.
                bar.seen = bar.text.clone();
                field(ui, &mut bar, error, value.as_deref());
                crate::ui::card_rule(ui);
                match error {
                    Some(e) => error_line(ui, e),
                    None => {
                        let (left, right) = match (&list, &value) {
                            (List::Complete(_), _) => (
                                "Tab completes · ↑↓ choose · Esc hides".to_owned(),
                                subject.clone(),
                            ),
                            _ if bar.recalled.is_some() => {
                                ("↑ recalled from the log".to_owned(), subject.clone())
                            }
                            (_, Some(v)) => (parsed.luau.clone(), format!("Enter copies {v}")),
                            _ if bar.text.trim().is_empty() => {
                                ("↑ recalls a line · Esc closes".to_owned(), subject.clone())
                            }
                            _ if parsed.each_by_itself => {
                                (format!("{} · each by itself", parsed.luau), subject.clone())
                            }
                            _ => (parsed.luau.clone(), subject.clone()),
                        };
                        footer(ui, &left, &right);
                    }
                }
            });
        // A click writes the text, not typing: `seen` takes it so the list
        // does not open on it.
        if let Some(i) = clicked_row
            && let List::Complete(c) = &list
        {
            bar.accept(c, i);
            bar.seen = bar.text.clone();
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
                bar.seen = bar.text.clone();
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
                    self.command_bar = None;
                    self.open_script_editor_with(source);
                    return;
                }
            }
        }
        if modal.backdrop_response.clicked() && bar.log_menu.is_none() {
            return;
        }
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

/// What taking `row` writes: an almost-match's suggestion, else its label.
fn insert_of(row: &mastik_vm::Row) -> &str {
    match (&row.suggestion, row.dim) {
        (Some(s), true) => s,
        _ => &row.label,
    }
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
    let (sections, rows) = match list {
        List::None => return 0.0,
        List::Log => (1, log_len.min(LOG_ROWS)),
        List::Complete(c) => {
            let rows = c.rows.len().min(LIST_ROWS);
            let mut sections = 0;
            let mut last = None;
            for row in c.rows.iter().take(rows) {
                if last != Some(row.section) {
                    sections += 1;
                    last = Some(row.section);
                }
            }
            (sections, rows)
        }
    };
    let gaps = (sections + rows).saturating_sub(1);
    let rule = 1.0;
    2.0 * PAD + sections as f32 * SECTION_H + rows as f32 * ROW_H + gaps as f32 * ROW_GAP + rule
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

/// The field: the `:` prompt in the accent, the line, a read's value at the right
/// edge, and a squiggle under the token an error names.
fn field(
    ui: &mut egui::Ui,
    bar: &mut Bar,
    error: Option<&mastik_vm::VmError>,
    value: Option<&str>,
) {
    let (rect, _) = ui.allocate_exact_size(egui::vec2(W, FIELD_H), egui::Sense::empty());
    let p = ui.painter().clone();
    p.text(
        egui::pos2(rect.left() + PAD + ROW_GLYPH_X, rect.center().y),
        egui::Align2::CENTER_CENTER,
        ":",
        mono(),
        color::accent_300(),
    );
    let value_w = value.map_or(0.0, |v| {
        let g = p.layout_no_wrap(format!("= {v}"), mono(), theme::text::dim());
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
        egui::pos2(rect.left() + TEXT_X, rect.top()),
        egui::pos2(rect.right() - VALUE_INSET - value_w, rect.bottom()),
    );
    let mut child = ui.new_child(
        egui::UiBuilder::new()
            .max_rect(text_rect)
            .layout(egui::Layout::left_to_right(egui::Align::Center)),
    );
    #[expect(
        clippy::disallowed_methods,
        reason = "the command bar's field: the palette's own scale, in the monospace face"
    )]
    let out = egui::TextEdit::singleline(&mut bar.text)
        .id(field_id())
        .frame(egui::Frame::NONE)
        .desired_width(f32::INFINITY)
        .font(mono())
        .text_color(color::text())
        .hint_text(
            egui::RichText::new("Type a command, or a sum")
                .font(mono())
                .color(theme::text::faint()),
        )
        .show(&mut child);
    // Every frame: the bar owns the keyboard while it is open.
    if !out.response.has_focus() {
        out.response.request_focus();
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

/// The footer: the Luau on the left, the subject or the read's note on the right.
fn footer(ui: &mut egui::Ui, left: &str, right: &str) {
    palette::footer_line(
        ui,
        FooterLine {
            left: (
                left,
                egui::FontId::monospace(crate::ui::CAPTION_PT),
                theme::text::dim(),
            ),
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
        (S::Properties | S::Names, _) => icon::HASH,
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
        app.command_bar.as_ref().expect("the bar is open").suggest
    }

    /// Type `text`, and the frame after it that reads the typing.
    fn type_in(ctx: &egui::Context, app: &mut MastikApp, text: &str) {
        pass(ctx, app, vec![egui::Event::Text(text.into())], NONE);
        pass(ctx, app, Vec::new(), NONE);
    }

    /// A name's letters open the list and an operator shuts it, though the
    /// empty word after it has rows to offer.
    /// flip: `follow_suggest` opening on any typed character → fails at "an
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
    /// flip: `follow_suggest` letting typing reopen a dismissed word → fails at
    /// "stays hidden".
    #[test]
    fn escape_hides_the_list_for_the_word() {
        let (ctx, mut app, _) = typed(".wi");
        pass(&ctx, &mut app, vec![key(egui::Key::Escape, NONE)], NONE);
        let bar = app.command_bar.as_ref().expect("the bar stays open");
        assert_eq!(bar.text, ".wi", "the line stays");
        assert!(matches!(bar.suggest, Suggest::Dismissed(_)), "hidden");
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
