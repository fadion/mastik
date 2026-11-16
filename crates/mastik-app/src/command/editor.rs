//! The script editor (Tools ▸ *Scripts…*): one floating window listing the
//! scripts from four places — this document, the library folder, the kits this
//! document uses, and the session's unsaved ones — with a code pane, an output
//! pane, and *Run*. A run is one undo step named after the script; a script
//! with a header asks for its inputs first, and one that came with a document or
//! a kit shows its source before its first run on this machine.

use super::code_keys;
use super::host;
use super::log;
use crate::app::MastikApp;
use crate::settings::{self, CardHold};
use crate::theme::{self, color, icon};
use crate::ui::{self, FieldButton};
use eframe::egui;
use mastik_core::script::{Script, ScriptId};
use mastik_core::{Operation, Transaction};
use serde_json::Value;
use std::time::Instant;

/// The window's size when it first opens; it keeps what the person makes it.
const START: egui::Vec2 = egui::vec2(1000.0, 620.0);
/// The source list's width.
const LIST_W: f32 = 240.0;
/// The script head's height, over the code.
const HEAD_H: f32 = 44.0;
/// The script head's inset from the pane's edges, the output pane's too.
const HEAD_INSET: f32 = 14.0;
/// The find field's width.
const FIND_W: f32 = 260.0;
/// The output pane's head.
const OUTPUT_HEAD_H: f32 = 30.0;
/// The output pane's height until its edge is dragged. It drags down to its
/// head alone, whose eyebrow then has the same air under it as over it.
const OUTPUT_H: f32 = 132.0;
/// The least the code pane keeps as the output pane is dragged up.
const CODE_MIN_H: f32 = 80.0;
/// How long typing in a library script rests before it is written to its file.
const WRITE_AFTER: std::time::Duration = std::time::Duration::from_millis(600);
/// The run form's width.
const FORM_W: f32 = 300.0;
/// The disclosure's width, and the most of the source it shows before scrolling.
const DISCLOSE_W: f32 = 472.0;
const DISCLOSE_CODE_H: f32 = 220.0;

/// Where a script lives.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum Which {
    /// Saved in this document.
    Document(ScriptId),
    /// A `.luau` file in the library folder, by name.
    Library(String),
    /// Carried by a kit this document uses: the kit's id and name, and the script.
    Kit {
        kit: String,
        kit_name: String,
        id: ScriptId,
    },
    /// Typed this session and not saved, by a serial that stays its own as
    /// others are deleted, so the values and the bad line kept for it do too.
    Unsaved(u64),
}

/// A script typed this session and saved nowhere.
#[derive(Clone, Debug)]
struct Unsaved {
    serial: u64,
    name: String,
    source: String,
}

/// One run's output, under a rule with its time.
#[derive(Clone, Debug)]
struct RunOut {
    at: String,
    secs: f32,
    lines: Vec<(String, bool)>,
}

/// The run form: a script's inputs as fields, before it runs.
struct Form {
    which: Which,
    name: String,
    source: String,
    header: mastik_vm::Header,
    values: serde_json::Map<String, Value>,
}

/// The box a document's or kit's script shows before its first run here.
struct Disclosure {
    which: Which,
    name: String,
    source: String,
}

/// The editor's state, kept for the session whether the window is open or not.
#[derive(Default)]
pub(crate) struct Scripts {
    pub(crate) open: bool,
    current: Option<Which>,
    /// The text in the code pane: a library, kit or unsaved script's, or a
    /// document script's as the document holds it.
    buffer: String,
    unsaved: Vec<Unsaved>,
    /// The serial the next unsaved script takes.
    next_unsaved: u64,
    search: String,
    /// The library folder's scripts, read when the window opens and after a write.
    library: Vec<library::LibraryScript>,
    /// When the library script being typed in was last changed, until written.
    pending: Option<Instant>,
    output: Vec<RunOut>,
    /// The line the last run failed at, for the script it ran.
    bad: Option<(Which, usize)>,
    /// Put the caret on this line (0-based) next frame.
    jump: Option<usize>,
    /// Give the code pane the keyboard next frame, the caret where it was.
    focus_code: bool,
    /// Select this byte range of the buffer next frame: find's match.
    found: Option<std::ops::Range<usize>>,
    /// The open script's menu, while open.
    menu: Option<Menu>,
    /// The menu's key this frame (`↑`/`↓`, `Enter`), read by the app's key
    /// routing while the menu holds the keyboard.
    pub(crate) menu_nav: Option<crate::menu::Nav>,
    /// The window holds the keyboard: the last press landed on it, or it was
    /// just opened. The canvas takes no keys meanwhile.
    keys: bool,
    /// A rename in progress, over the open script's list row: the new name as
    /// typed.
    renaming: Option<String>,
    /// Find, while open: the text sought.
    find: Option<String>,
    /// Find's matches in the buffer this frame, ignoring case.
    find_hits: Vec<std::ops::Range<usize>>,
    /// The current match, by its place in `find_hits`.
    find_current: usize,
    /// Scroll the current match into view next frame.
    find_reveal: bool,
    /// The hash of the text and the sought text `find_hits` were found for.
    find_key: u64,
    /// Give find's field the keyboard once it can take it.
    find_focus: bool,
    form: Option<Form>,
    disclosure: Option<Disclosure>,
    /// A library script whose deletion is being confirmed, by name.
    deleting: Option<String>,
    /// Counts each new text put into the code pane, salting its id
    /// ([`Scripts::code_id`]).
    shown: u64,
    /// The values each script's form last ran with, for the session, by
    /// [`values_key`].
    last_values: rustc_hash::FxHashMap<String, serde_json::Map<String, Value>>,
    /// The output pane's height, its head included, once its top edge has been
    /// dragged; [`OUTPUT_H`] until then.
    output_h: Option<f32>,
    /// The output pane's drag edge, lit after the pointer rests on it.
    output_edge: crate::app::EdgeDwell,
    /// The code pane's completion list, which opens and shuts as the bar's.
    suggest: super::suggest::Suggester,
    /// The completion as last worked out, under what it was worked out for:
    /// the text's hash, the caret, the document's revision and the page.
    completed: Option<(
        (u64, usize, u64, mastik_core::NodeId),
        mastik_vm::Completion,
    )>,
    /// The language help over the code pane: hints, hover cards, problems.
    pub(crate) help: super::script_help::Help,
    /// The code had the keyboard at the end of last frame.
    code_focused: bool,
    /// A double-click's word, held while the button stays down: dragging then
    /// selects whole words from it, in bytes.
    word_drag: Option<std::ops::Range<usize>>,
    /// The last press on the code: when (`ctx` time) and where, to tell a
    /// double-click's second press as it lands.
    last_press: Option<(f64, egui::Pos2)>,
    /// The line a closing word just stepped back, until the next letter typed.
    stepped: Option<code_keys::Stepped>,
    /// The open library script's text as last read from or written to its
    /// file: a file that no longer reads so was changed outside Mastik.
    on_disk: Option<String>,
    /// When the open library script's file is looked at again, as the open
    /// document's is.
    disk_watch: crate::library::disk::Watch,
}

/// The completion list open over the code this frame: its rows (the word they
/// replace starts where the list hangs) and the layer each shown row names, for
/// its glyph.
struct Offer {
    completion: mastik_vm::Completion,
    layers: Vec<Option<mastik_core::NodeId>>,
}

/// The script menu: the ⋯ button's, or a list row's right-click's.
struct Menu {
    at: MenuAt,
    /// The row the keys have lit, by its place among the menu's rows.
    highlight: Option<usize>,
}

/// Where the script menu hangs.
#[derive(Clone, Copy)]
enum MenuAt {
    /// Under the head's ⋯ button, whose rect it is.
    Button(egui::Rect),
    /// At a list row's right-click.
    Press(egui::Pos2),
}

/// What the window asked for this frame.
#[derive(Clone, PartialEq, Debug)]
enum Act {
    Close,
    Pick(Which),
    /// A list row's right-click: open that script, and its menu at the press.
    RowMenu(Which, egui::Pos2),
    New,
    Run,
    OpenFolder,
    SaveToLibrary,
    CopyToDocument,
    Duplicate,
    StartRename,
    Rename(String),
    Delete,
    ShowInFolder,
}

impl Scripts {
    /// Whether a modal card is up over the canvas: the run form, the disclosure
    /// or a library delete's confirmation.
    pub(crate) fn modal_up(&self) -> bool {
        self.form.is_some() || self.disclosure.is_some() || self.deleting.is_some()
    }

    /// Whether the window holds the keyboard, so the canvas takes no keys:
    /// while its menu is open, and after a press on it until a press elsewhere.
    pub(crate) fn holds_keys(&self) -> bool {
        self.open && (self.keys || self.menu.is_some())
    }

    /// Whether the menu is open, holding the keyboard as a context menu does.
    pub(crate) fn menu_open(&self) -> bool {
        self.menu.is_some()
    }

    /// `Escape` while the window holds the keyboard: it closes the menu, and
    /// otherwise goes no further than the window's own controls.
    pub(crate) fn escape_menu(&mut self) {
        self.menu = None;
    }

    /// The code pane's widget id: a fresh one for each script shown, so the
    /// text edit's own undo never brings one script's text into another.
    fn code_id(&self) -> egui::Id {
        egui::Id::new(("script-code-edit", self.shown))
    }
}

impl MastikApp {
    /// The keys while the Scripts window holds the keyboard
    /// ([`Scripts::holds_keys`]): its menu's, as a context menu's, and past any
    /// control that is no text entry the undo, redo and save chords, as past a
    /// focused button; nothing reaches the canvas.
    pub(crate) fn scripts_keys(&mut self, ctx: &egui::Context) {
        if self.scripts.menu_open() {
            if ctx.input(|i| i.key_pressed(egui::Key::Escape)) {
                self.scripts.escape_menu();
            } else {
                self.scripts.menu_nav = crate::menu::read_nav(ctx);
            }
            return;
        }
        let typing = ctx
            .memory(|m| m.focused())
            .is_some_and(|id| egui::TextEdit::load_state(ctx, id).is_some());
        if typing {
            return;
        }
        for action in crate::input::passing_keys(ctx, self.prefs.nudge) {
            self.dispatch(ctx, action);
        }
    }

    /// Tools ▸ *Scripts…*: open the editor, holding the keyboard.
    pub(crate) fn open_scripts(&mut self) {
        self.scripts.open = true;
        self.scripts.keys = true;
        // Never from a headless app, whose prefs are a throwaway: it would write
        // into this machine's real scripts folder.
        if !self.prefs.ephemeral
            && let Err(e) = library::write_definitions()
        {
            ::log::warn!(target: "mastik", "the scripts folder's definitions: {e}");
        }
        self.scripts.library = library::list();
        self.follow_library_disk();
        if self.scripts.current.is_none() {
            let first = self
                .session
                .doc
                .scripts()
                .first()
                .map(|s| Which::Document(s.id))
                .or_else(|| {
                    self.scripts
                        .library
                        .first()
                        .map(|s| Which::Library(s.name.clone()))
                });
            if let Some(w) = first {
                self.pick_script(w);
            }
        }
    }

    /// Start a run of the open script, as its *Run* does.
    #[cfg(test)]
    pub(crate) fn run_open_script(&mut self) {
        if let Some(w) = self.scripts.current.clone() {
            self.start_script_run(w);
        }
    }

    /// The editor open on a new unsaved script holding `source`, its caret at
    /// the end and the completion list up on the word there, as typing that
    /// word would leave it: the control audit's fixture.
    #[cfg(test)]
    pub(crate) fn open_script_completion(&mut self, ctx: &egui::Context, source: &str) {
        self.open_script_editor_with(source.into());
        let id = self.scripts.code_id();
        let mut state = egui::TextEdit::load_state(ctx, id).unwrap_or_default();
        state
            .cursor
            .set_char_range(Some(egui::text::CCursorRange::one(
                egui::text::CCursor::new(source.chars().count()),
            )));
        state.store(ctx, id);
        ctx.memory_mut(|m| m.request_focus(id));
        let word = source
            .chars()
            .rev()
            .take_while(|c| c.is_alphanumeric() || *c == '_')
            .map(char::len_utf8)
            .sum::<usize>();
        self.scripts.suggest.state = super::suggest::Suggest::On(source.len() - word);
        self.scripts.suggest.seen = source.to_owned();
    }

    /// The log's *Open in editor*: a new unsaved script holding `source`.
    pub(crate) fn open_script_editor_with(&mut self, source: String) {
        self.open_scripts();
        self.add_unsaved(source);
    }

    /// Start a new unsaved script holding `source`, and show it.
    fn add_unsaved(&mut self, source: String) {
        let serial = self.scripts.next_unsaved;
        self.scripts.next_unsaved += 1;
        let name = self.unsaved_name();
        self.scripts.unsaved.push(Unsaved {
            serial,
            name,
            source,
        });
        self.pick_script(Which::Unsaved(serial));
    }

    /// Every saved script, in the palette's order: this document's, the
    /// library's as last read, then each kit's that its shelf has read.
    fn saved_scripts(&self) -> Vec<(Which, String)> {
        let mut out: Vec<(Which, String)> = self
            .session
            .doc
            .scripts()
            .iter()
            .map(|s| (Which::Document(s.id), s.name.clone()))
            .collect();
        out.extend(
            self.scripts
                .library
                .iter()
                .map(|s| (Which::Library(s.name.clone()), s.name.clone())),
        );
        for entry in self.script_kits() {
            for s in self.entry_scripts(&entry) {
                out.push((
                    Which::Kit {
                        kit: entry.id.clone(),
                        kit_name: entry.name.clone(),
                        id: s.id,
                    },
                    s.name,
                ));
            }
        }
        out
    }

    /// The saved scripts' names, for the palette's *Run* rows, each with why its
    /// row is held: the reason *Run* gives while a syntax error holds the script.
    pub(crate) fn saved_script_runs(&self) -> Vec<(String, Option<String>)> {
        self.saved_scripts()
            .into_iter()
            .map(|(which, name)| {
                let held = self
                    .script_source(&which)
                    .and_then(|src| self.scripts.help.held_line(&src))
                    .map(super::script_help::held_reason_at);
                (name, held)
            })
            .collect()
    }

    /// What the palette's *Recent* keeps each *Run* row under, in the rows'
    /// order: the script it runs, never its place in the list, which another
    /// document or a script added since would shift onto a script never run.
    pub(crate) fn saved_script_keys(&self) -> Vec<String> {
        let doc = self.session.doc.meta().id.clone().unwrap_or_default();
        self.saved_scripts()
            .into_iter()
            .map(|(which, _)| match which {
                Which::Document(id) => format!("RunScript document {doc} {:?}", id.0),
                Which::Library(name) => format!("RunScript library {name}"),
                Which::Kit { kit, id, .. } => format!("RunScript kit {kit} {:?}", id.0),
                Which::Unsaved(serial) => format!("RunScript unsaved {serial}"),
            })
            .collect()
    }

    /// Read the library folder again, as the palette opens.
    pub(crate) fn refresh_script_library(&mut self) {
        self.scripts.library = library::list();
        self.follow_library_disk();
    }

    /// Show the open library script's file as it now reads, when it was changed
    /// outside Mastik and nothing typed here waits to be written; with typing
    /// waiting, [`Self::flush_library_write`] keeps both.
    fn follow_library_disk(&mut self) {
        let Some(Which::Library(name)) = self.scripts.current.clone() else {
            return;
        };
        if self.scripts.pending.is_some() {
            return;
        }
        let Ok(now) = library::read(&name) else {
            return;
        };
        if self.scripts.on_disk.as_ref() == Some(&now) {
            return;
        }
        self.scripts.on_disk = Some(now.clone());
        if let Some(listed) = self.scripts.library.iter_mut().find(|s| s.name == name) {
            listed.source = now.clone();
        }
        if now != self.scripts.buffer {
            let ctx = self.egui_ctx.clone();
            self.show_changed_text(&ctx, now);
            self.session
                .info(format!("Reloaded “{name}”: it changed on disk"));
        }
    }

    /// Put `text`, changed under the pane by someone else, in the code pane: a
    /// new id, so the text edit's undo never crosses into it, and the keyboard
    /// and the caret (clamped to the new text) kept when the code had them.
    fn show_changed_text(&mut self, ctx: &egui::Context, text: String) {
        let old = self.scripts.code_id();
        let focused = ctx.memory(|m| m.has_focus(old));
        let sel = selection(ctx, old, &self.scripts.buffer);
        self.scripts.buffer = text;
        self.scripts.shown += 1;
        self.scripts.word_drag = None;
        self.scripts.stepped = None;
        if focused {
            self.scripts.focus_code = true;
            if let Some(sel) = sel {
                put_selection(ctx, self.scripts.code_id(), &self.scripts.buffer, sel);
            }
        }
    }

    /// The palette's *Run* row `i`: start that script's run.
    pub(crate) fn run_listed_script(&mut self, i: usize) {
        if let Some((which, _)) = self.saved_scripts().into_iter().nth(i) {
            self.start_script_run(which);
        }
    }

    /// `Untitled N`, the next free among every listed script.
    fn unsaved_name(&self) -> String {
        let mut taken: Vec<Script> = self.session.doc.scripts().to_vec();
        for (i, u) in self.scripts.unsaved.iter().enumerate() {
            taken.push(Script {
                id: ScriptId(mastik_core::NodeId {
                    actor: 0,
                    seq: i as u64,
                }),
                name: u.name.clone(),
                source: String::new(),
            });
        }
        mastik_core::script::free_name(&taken, &format!("{} 1", mastik_core::script::UNTITLED))
    }

    /// Show `which` in the code pane, writing any library edit still waiting. A
    /// library script is read from its file, as it is now.
    fn pick_script(&mut self, which: Which) {
        self.flush_library_write();
        let fresh = match &which {
            Which::Library(name) => library::read(name).ok(),
            _ => None,
        };
        if let (Which::Library(name), Some(text)) = (&which, &fresh)
            && let Some(listed) = self.scripts.library.iter_mut().find(|s| s.name == *name)
        {
            listed.source = text.clone();
        }
        self.scripts.on_disk = fresh;
        self.scripts.buffer = self.script_source(&which).unwrap_or_default();
        self.scripts.shown += 1;
        self.scripts.word_drag = None;
        self.scripts.current = Some(which);
        self.scripts.menu = None;
        self.scripts.renaming = None;
    }

    /// `which`'s name and source as they stand.
    fn script_named(&self, which: &Which) -> Option<(String, String)> {
        match which {
            Which::Document(id) => self
                .session
                .doc
                .script(*id)
                .map(|s| (s.name.clone(), s.source.clone())),
            Which::Library(name) => self
                .scripts
                .library
                .iter()
                .find(|s| s.name == *name)
                .map(|s| (s.name.clone(), s.source.clone())),
            Which::Kit { kit, id, .. } => self
                .kit_scripts(kit)
                .into_iter()
                .find(|s| s.id == *id)
                .map(|s| (s.name, s.source)),
            Which::Unsaved(serial) => self
                .scripts
                .unsaved
                .iter()
                .find(|u| u.serial == *serial)
                .map(|u| (u.name.clone(), u.source.clone())),
        }
    }

    fn script_source(&self, which: &Which) -> Option<String> {
        self.script_named(which).map(|(_, s)| s)
    }

    /// The scripts kit `kit` carries, from its published state, which the kit's
    /// shelf reads when first asked.
    fn kit_scripts(&self, kit: &str) -> Vec<Script> {
        self.script_kits()
            .iter()
            .find(|k| k.id == kit)
            .map(|entry| self.entry_scripts(entry))
            .unwrap_or_default()
    }

    /// The library's entries for the kits this document uses, in vault order.
    /// Read every frame by the editor and the palette, so it walks only the
    /// vault's tops: `kits_used` also counts each kit's instances.
    fn script_kits(&self) -> Vec<crate::kits::index::KitEntry> {
        let doc = &self.session.doc;
        let Some(vault) = doc.vault().and_then(|v| doc.get(v)) else {
            return Vec::new();
        };
        let mut used: Vec<&str> = Vec::new();
        for top in vault.children() {
            if let Some(o) = doc.get(*top).and_then(|n| n.origin())
                && !used.contains(&o.kit.as_str())
            {
                used.push(&o.kit);
            }
        }
        if used.is_empty() {
            return Vec::new();
        }
        let listed = crate::kits::index::kits_in(&self.library, doc.meta().id.as_deref());
        used.iter()
            .filter_map(|kit| listed.iter().find(|k| k.id == *kit).cloned())
            .collect()
    }

    /// `entry`'s scripts as its shelf holds them; none until it is read.
    fn entry_scripts(&self, entry: &crate::kits::index::KitEntry) -> Vec<Script> {
        self.kit_shelf
            .current(entry)
            .map(|p| p.document().scripts().to_vec())
            .unwrap_or_default()
    }

    /// The kits this document uses, each with its scripts; a kit not read yet is
    /// asked for, and lists its scripts once its shelf has them.
    fn kits_with_scripts(&mut self) -> Vec<(String, String, Vec<Script>)> {
        let mut out = Vec::new();
        for entry in self.script_kits() {
            if self.kit_shelf.current(&entry).is_none() {
                self.kit_shelf.ask(&entry);
                continue;
            }
            let scripts = self.entry_scripts(&entry);
            if !scripts.is_empty() {
                out.push((entry.id, entry.name, scripts));
            }
        }
        out
    }

    /// Write the library script being typed in, if it is waiting. When its file
    /// was changed outside Mastik meanwhile, nothing is written over it: the
    /// typing is saved as a new library script beside it (an unsaved one if that
    /// fails), shown in its place. Answers `false` then, so a run or an act does
    /// not go on with the file's text.
    pub(crate) fn flush_library_write(&mut self) -> bool {
        let Some(Which::Library(name)) = self.scripts.current.clone() else {
            self.scripts.pending = None;
            return true;
        };
        if self.scripts.pending.take().is_none() {
            return true;
        }
        let now = library::read(&name).ok();
        if let (Some(now), Some(was)) = (&now, &self.scripts.on_disk)
            && now != was
        {
            let typed = std::mem::take(&mut self.scripts.buffer);
            let copy = library::unique_name(&name);
            let kept = match library::write(&copy, &typed) {
                Ok(()) => {
                    self.scripts.library = library::list();
                    self.pick_script(Which::Library(copy.clone()));
                    copy
                }
                Err(_) => {
                    let unsaved = self.unsaved_name();
                    self.add_unsaved(typed);
                    unsaved
                }
            };
            self.session.fail(format!(
                "“{name}” changed on disk, so your edit was saved as “{kept}” instead"
            ));
            return false;
        }
        match library::write(&name, &self.scripts.buffer) {
            Ok(()) => {
                self.scripts.on_disk = Some(self.scripts.buffer.clone());
                self.scripts.library = library::list();
            }
            Err(e) => self.session.fail(format!("Couldn't save {name}: {e}")),
        }
        true
    }

    /// Put the code pane's text where the script lives: a document script as an
    /// *Edit script* step that folds while typing continues, a library script
    /// after a rest, an unsaved one at once. A kit's script is read-only.
    fn store_buffer(&mut self) {
        let Some(which) = self.scripts.current.clone() else {
            return;
        };
        match which {
            Which::Document(id) => {
                let tx = Transaction(vec![Operation::SetScriptSource {
                    id,
                    source: self.scripts.buffer.clone(),
                }]);
                self.session.name_step("Edit script", icon::FILE_CODE);
                self.session.commit_run(tx, "edit-script");
                self.authored(&which);
            }
            Which::Library(_) => self.scripts.pending = Some(Instant::now()),
            Which::Unsaved(serial) => {
                if let Some(u) = self.scripts.unsaved.iter_mut().find(|u| u.serial == serial) {
                    u.source = self.scripts.buffer.clone();
                }
            }
            Which::Kit { .. } => {}
        }
    }

    /// The code pane's completion for this frame, before the pane draws: the
    /// list follows what was typed last frame, and while it is up its keys are
    /// taken before the text edit reads them — `↑`/`↓` move, `Tab` takes the
    /// lit row and `Enter` too unless the word already reads as it (then Enter
    /// breaks the line), `Escape` shuts it for the word; `Ctrl+Space` opens it.
    /// Only while the code has the focus. Answers the list to draw.
    fn script_suggest(&mut self, ctx: &egui::Context, code_id: egui::Id) -> Option<Offer> {
        let focused = matches!(
            self.scripts.current,
            Some(Which::Document(_) | Which::Library(_) | Which::Unsaved(_))
        ) && ctx.memory(|m| m.has_focus(code_id));
        let caret = focused
            .then(|| caret_byte(ctx, code_id, &self.scripts.buffer))
            .flatten();
        let completion = caret.map(|caret| {
            let key = (
                egui::util::hash(&self.scripts.buffer),
                caret,
                self.session.revision(),
                self.session.page,
            );
            match &self.scripts.completed {
                Some((k, c)) if *k == key => c.clone(),
                _ => {
                    let c = mastik_vm::complete_script(
                        &self.session.doc,
                        self.session.page,
                        &self.scripts.buffer,
                        caret,
                    );
                    self.scripts.completed = Some((key, c.clone()));
                    c
                }
            }
        });
        let invoke =
            focused && ctx.input_mut(|i| i.consume_key(egui::Modifiers::CTRL, egui::Key::Space));
        let s = &mut self.scripts;
        s.suggest
            .follow(&s.buffer, completion.as_ref(), caret.unwrap_or(0), invoke);
        let rows = completion
            .as_ref()
            .filter(|_| s.suggest.on())
            .map_or(0, |c| c.rows.len().min(super::LIST_ROWS));
        // What the pane changes from here on is typing, which next frame's
        // `follow` reads against this.
        s.suggest.seen = s.buffer.clone();
        let completion = completion.filter(|_| rows > 0)?;
        s.suggest.highlight = s.suggest.highlight.min(rows - 1);
        let none = egui::Modifiers::NONE;
        let changes = s.suggest.changes(&s.buffer, &completion);
        let (up, down, take, escape) = ctx.input_mut(|i| {
            (
                i.consume_key(none, egui::Key::ArrowUp),
                i.consume_key(none, egui::Key::ArrowDown),
                i.consume_key(none, egui::Key::Tab)
                    || (changes && i.consume_key(none, egui::Key::Enter)),
                i.consume_key(none, egui::Key::Escape),
            )
        });
        if up || down {
            s.suggest.step(rows, up);
        }
        if escape {
            s.suggest.dismiss();
        }
        if take {
            let lit = s.suggest.highlight;
            self.take_script_row(ctx, code_id, &completion, lit);
            return None;
        }
        if !s.suggest.on() {
            return None;
        }
        let shown = &completion.rows[..rows];
        let layers = self.completion_layers(shown);
        Some(Offer { completion, layers })
    }

    /// Take row `i` of `completion` into the code: the caret after what it
    /// wrote, the list shut, the code focused, and the text stored as typing is.
    fn take_script_row(
        &mut self,
        ctx: &egui::Context,
        code_id: egui::Id,
        completion: &mastik_vm::Completion,
        i: usize,
    ) {
        let s = &mut self.scripts;
        let Some(at) = s.suggest.accept(&mut s.buffer, completion, i) else {
            return;
        };
        // Written, not typed: the list does not open on it.
        s.suggest.seen = s.buffer.clone();
        put_selection(ctx, code_id, &s.buffer, code_keys::Sel::caret(at));
        ctx.memory_mut(|m| m.request_focus(code_id));
        self.store_buffer();
    }

    /// The code pane's editing keys, taken out of this frame's events before
    /// the text edit reads them (`code_keys`): `Enter` indents, `Tab` and
    /// `Shift+Tab` work on lines, `Ctrl+/` comments, `Alt+↑`/`↓` move lines and
    /// `Shift+Alt+↑`/`↓` copy them, brackets and quotes pair, and `Backspace`
    /// takes an empty pair. Handling stops at the first event left to the text
    /// edit, so the frame's keys keep their order. Answers whether a character
    /// was left to the text edit to type, for [`code_keys::dedent_closer`].
    fn script_keys(&mut self, ctx: &egui::Context, code_id: egui::Id) -> bool {
        let editable = matches!(
            self.scripts.current,
            Some(Which::Document(_) | Which::Library(_) | Which::Unsaved(_))
        );
        if !editable || !ctx.memory(|m| m.has_focus(code_id)) {
            return false;
        }
        let Some(mut sel) = selection(ctx, code_id, &self.scripts.buffer) else {
            return false;
        };
        let mut text = self.scripts.buffer.clone();
        let (mut changed, mut typed) = (false, false);
        let mut clipboard: Option<String> = None;
        ctx.input_mut(|i| {
            let mut kept = Vec::new();
            let mut ours = true;
            for e in std::mem::take(&mut i.events) {
                let next = match &e {
                    _ if !ours => None,
                    // `Ctrl+X` with nothing selected cuts the caret's line.
                    egui::Event::Cut if sel.anchor == sel.caret => {
                        let (s, line) = code_keys::cut_line(&mut text, sel);
                        clipboard = Some(line);
                        Some(s)
                    }
                    egui::Event::Key {
                        key,
                        pressed: true,
                        modifiers: m,
                        ..
                    } => edit_key(&mut text, sel, *key, *m),
                    egui::Event::Text(t) => {
                        let mut chars = t.chars();
                        match (chars.next(), chars.next()) {
                            (Some(c), None) => code_keys::type_char(&mut text, sel, c),
                            _ => None,
                        }
                    }
                    _ => None,
                };
                if let Some(s) = next {
                    sel = s;
                    changed = true;
                    continue;
                }
                if matches!(e, egui::Event::Text(_)) {
                    typed = true;
                }
                ours &= !edits_text(&e);
                kept.push(e);
            }
            i.events = kept;
        });
        // Out of `input_mut`: two context accessors nested deadlock.
        if let Some(line) = clipboard {
            ctx.copy_text(line);
        }
        if changed {
            let s = &mut self.scripts;
            s.buffer = text;
            // Written, not typed: the completion list does not open on it.
            s.suggest.seen = s.buffer.clone();
            put_selection(ctx, code_id, &s.buffer, sel);
            // Typed for the help, though: an auto-paired `(` or `{` opens the
            // call's hint, and the check waits for the rest.
            s.help.edited(ctx);
            self.store_buffer();
        }
        typed
    }

    /// Start a run of `which`: the disclosure first for a document's or kit's
    /// script not shown here in this text, then the form for a script with
    /// inputs, then the run.
    pub(crate) fn start_script_run(&mut self, which: Which) {
        if !self.flush_library_write() {
            return;
        }
        // A library script runs as its file reads now, which the pane then shows.
        if matches!(which, Which::Library(_)) {
            self.scripts.library = library::list();
            self.follow_library_disk();
        }
        let Some((name, source)) = self.script_named(&which) else {
            return;
        };
        // A syntax error holds every run, before its disclosure or form; from the
        // editor the caret goes to it.
        if let Some(p) = mastik_vm::syntax_error(&source) {
            self.session
                .fail(format!("{name} has a syntax error on line {}", p.line));
            if self.scripts.open && self.scripts.current.as_ref() == Some(&which) {
                self.scripts.found = Some(p.span.start..p.span.start);
            }
            return;
        }
        // A document or kit script with no key (a file whose id was taken out) is
        // shown every time: nothing can mark it, so it never runs unseen.
        let shown_before = match &which {
            Which::Document(_) | Which::Kit { .. } => self
                .disclosure_key(&which)
                .is_some_and(|key| self.prefs.disclosed(&key, &source)),
            Which::Library(_) | Which::Unsaved(_) => true,
        };
        if !shown_before {
            self.scripts.disclosure = Some(Disclosure {
                which,
                name,
                source,
            });
            return;
        }
        self.script_form_or_run(which, name, source);
    }

    /// The key the first-run disclosure is remembered under on this machine, for
    /// a script that came with a document or a kit; `None` for the person's own,
    /// and for a document with no identity, whose scripts are shown every run.
    fn disclosure_key(&self, which: &Which) -> Option<String> {
        match which {
            Which::Document(id) => {
                crate::prefs::disclosure_key(self.session.doc.meta().id.as_deref(), *id)
            }
            Which::Kit { kit, id, .. } => crate::prefs::disclosure_key(Some(kit), *id),
            Which::Library(_) | Which::Unsaved(_) => None,
        }
    }

    /// Open the form for a script with inputs, else run it.
    fn script_form_or_run(&mut self, which: Which, name: String, source: String) {
        let header = match mastik_vm::header(&source) {
            Ok(h) => h,
            Err(e) => return self.script_failed(&which, &name, &e, 0.0),
        };
        if header.inputs.is_empty() {
            return self.run_script_now(which, name, source, Default::default());
        }
        let mut values = self
            .scripts
            .last_values
            .get(&values_key(&which))
            .cloned()
            .unwrap_or_default();
        for input in &header.inputs {
            values
                .entry(input.name.clone())
                .or_insert_with(|| input.default.clone());
        }
        self.scripts.form = Some(Form {
            which,
            name,
            source,
            header,
            values,
        });
    }

    /// Run a script now, as one undo step named after it, and report.
    fn run_script_now(
        &mut self,
        which: Which,
        name: String,
        source: String,
        params: serde_json::Map<String, Value>,
    ) {
        let selection = self.session.selection.ids().to_vec();
        let labels: rustc_hash::FxHashMap<String, mastik_core::NodeId> = self
            .leap_labels()
            .iter()
            .map(|(k, v)| (k.clone(), *v))
            .collect();
        let env = mastik_vm::Env {
            selection: &selection,
            labels: &labels,
            params,
        };
        let started = Instant::now();
        let ran = host::run_as_step(&mut self.session, &name, icon::FILE_CODE, |h| {
            mastik_vm::run_script(h, &env, &source)
        });
        let secs = started.elapsed().as_secs_f32();
        match ran {
            Ok((outcome, counts, serial)) => {
                let said = counts.said().unwrap_or_else(|| "nothing changed".into());
                let mut lines: Vec<(String, bool)> =
                    outcome.printed.iter().map(|l| (l.clone(), false)).collect();
                lines.push((format!("{name} · {said}"), false));
                self.push_output(lines, secs);
                if let Some(ids) = outcome.selection {
                    self.session.selection.set(ids);
                }
                self.session.info(format!("{name} · {said}"));
                self.session
                    .command_log
                    .push(log::Entry::script(&name, said, false, serial));
                if matches!(self.scripts.bad, Some((ref w, _)) if *w == which) {
                    self.scripts.bad = None;
                }
            }
            Err(e) => self.script_failed(&which, &name, &e, secs),
        }
    }

    /// Report a failed run: the output pane, the status line and the log, and
    /// the editor opened at the line it failed on.
    fn script_failed(&mut self, which: &Which, name: &str, e: &mastik_vm::VmError, secs: f32) {
        let said = match (e.kind, e.line) {
            (mastik_vm::ErrorKind::Budget, _) => {
                format!(
                    "{name} stopped: {} — nothing changed",
                    lower_first(&e.message)
                )
            }
            (_, Some(line)) => format!("{name} failed at line {line} — nothing changed"),
            (_, None) => format!("{name} failed — nothing changed"),
        };
        let first = match e.line {
            Some(line) => format!("Line {line}: {}", e.message),
            None => e.message.clone(),
        };
        self.push_output(vec![(first, true), ("Nothing changed.".into(), true)], secs);
        self.session.fail(said);
        self.session
            .command_log
            .push(log::Entry::script(name, e.message.clone(), true, None));
        if let Some(line) = e.line {
            let at = line.saturating_sub(1) as usize;
            self.scripts.bad = Some((which.clone(), at));
            self.scripts.jump = Some(at);
        }
        self.scripts.open = true;
        if self.scripts.current.as_ref() != Some(which) {
            self.pick_script(which.clone());
        }
    }

    /// Add a run's lines to the output pane, under a rule with the time: one row
    /// per line, a printed line that holds newlines split, a very long one cut.
    fn push_output(&mut self, lines: Vec<(String, bool)>, secs: f32) {
        let lines = lines
            .into_iter()
            .flat_map(|(text, bad)| {
                text.split('\n')
                    .map(|row| (output_row(row), bad))
                    .collect::<Vec<_>>()
            })
            .collect();
        let now = crate::library::clock::now();
        let local = now as i64 + crate::library::clock::local_offset(now);
        let day = local.rem_euclid(86_400);
        let at = format!("{:02}:{:02}", day / 3600, (day % 3600) / 60);
        self.scripts.output.push(RunOut { at, secs, lines });
        // The pane keeps the last few runs; older ones scroll away for good.
        let keep = 20;
        let len = self.scripts.output.len();
        if len > keep {
            self.scripts.output.drain(..len - keep);
        }
    }

    /// Draw the editor, the form and the disclosure, and act on this frame.
    pub(crate) fn scripts_ui(&mut self, ctx: &egui::Context, canvas: egui::Rect) {
        self.script_form_ui(ctx, canvas);
        self.disclosure_ui(ctx, canvas);
        self.script_delete_confirmation(ctx);
        if !self.scripts.open {
            self.scripts.keys = false;
            self.scripts.menu = None;
            return;
        }
        if self
            .scripts
            .pending
            .is_some_and(|at| at.elapsed() >= WRITE_AFTER)
        {
            self.flush_library_write();
        } else if self.scripts.pending.is_some() {
            ctx.request_repaint_after(WRITE_AFTER);
        }
        // An open library script's file is looked at as the open document's is.
        if matches!(self.scripts.current, Some(Which::Library(_))) {
            let (now, focused) = ctx.input(|i| (i.time, i.viewport().focused.unwrap_or(true)));
            match self.scripts.disk_watch.due(now, focused) {
                Ok(()) => self.follow_library_disk(),
                Err(wait) => ctx.request_repaint_after(std::time::Duration::from_secs_f64(wait)),
            }
        }
        // A document script changed under the pane (an undo, an agent's edit):
        // show it as it is, the keyboard staying with the code.
        if let Some(Which::Document(id)) = &self.scripts.current {
            match self.session.doc.script(*id) {
                Some(s) if s.source != self.scripts.buffer => {
                    let text = s.source.clone();
                    self.show_changed_text(ctx, text);
                }
                Some(_) => {}
                None => {
                    self.scripts.current = None;
                    self.scripts.buffer.clear();
                }
            }
        }
        let kits = self.kits_with_scripts();
        let mut act: Option<Act> = None;
        let mut edited = false;
        let current = self.scripts.current.clone();
        let header = current.as_ref().and_then(|w| self.script_named(w));
        let where_ = match &current {
            Some(Which::Document(_)) => "in this document",
            Some(Which::Library(_)) => "in the library",
            Some(Which::Kit { .. }) => "in a kit · read only",
            Some(Which::Unsaved(_)) => "unsaved",
            None => "",
        };
        let edited_note = self.scripts.pending.is_some();
        let doc_scripts: Vec<Script> = self.session.doc.scripts().to_vec();
        let code_id = self.scripts.code_id();
        // egui lets go of a focused field on `Escape` before anything reads the
        // key; when the list or the hint takes it, the code keeps the keyboard.
        let escape = ctx.input(|i| i.key_pressed(egui::Key::Escape));
        if escape
            && self.scripts.code_focused
            && (self.scripts.suggest.on() || self.scripts.help.hint_up())
        {
            ctx.memory_mut(|m| m.request_focus(code_id));
        }
        // Before the list's `Ctrl+Space`, which a held Shift would also match.
        if ctx.memory(|m| m.has_focus(code_id))
            && ctx.input_mut(|i| {
                i.consume_key(
                    egui::Modifiers::CTRL | egui::Modifiers::SHIFT,
                    egui::Key::Space,
                )
            })
        {
            self.scripts.help.ask_hint();
        }
        let offer = self.script_suggest(ctx, self.scripts.code_id());
        // After the list, which takes its own keys first.
        let typed = self.script_keys(ctx, self.scripts.code_id());
        let page = self.session.page;
        self.scripts.help.lit_layer = None;
        let revision = self.session.revision();
        let problems: Vec<mastik_vm::Problem> = match &current {
            Some(_) => self
                .scripts
                .help
                .problems(ctx, &self.session.doc, revision, page, &self.scripts.buffer)
                .to_vec(),
            None => Vec::new(),
        };
        let held = self
            .scripts
            .help
            .syntax(&self.scripts.buffer)
            .map(super::script_help::held_reason);
        let caret = caret_byte(ctx, self.scripts.code_id(), &self.scripts.buffer);
        let mut help_act = None;
        let doc = &self.session.doc;
        let res = &self.session.resolved;
        let (mut row_clicked, mut row_hovered) = (None, None);
        let s = &mut self.scripts;
        let id = egui::Id::new("script-editor");
        // Placed as every dialog over the canvas is (`ui::canvas_dialog_area`), by
        // its top-left: the window is resizable, so its width is last frame's.
        let w = ctx
            .memory(|m| m.area_rect(id))
            .map_or(START.x, |r| r.width());
        let x = ui::card_lane(ctx).unwrap_or(canvas).center().x - w / 2.0;
        let y = ctx.content_rect().top() + ui::dialog_top(ctx, id);
        let at = egui::pos2(x, y) + ui::card_drag(ctx, id);
        let window = egui::Window::new("Scripts")
            .id(id)
            .title_bar(false)
            .resizable(true)
            .collapsible(false)
            .movable(false)
            .default_size(START)
            .min_size(egui::vec2(640.0, 360.0))
            .current_pos(at)
            .frame(crate::settings::card_frame().inner_margin(egui::Margin::ZERO));
        window.show(ctx, |ui| {
            ui.spacing_mut().item_spacing = egui::Vec2::ZERO;
            if settings::card_head(ui, ui.available_width(), "Scripts") {
                act = Some(Act::Close);
            }
            ui::card_rule(ui);
            // The body is laid out in fixed rects and allocated once. Content
            // that measured itself would grow the window: a resizable window
            // keeps the size its content reached, so a column a point past its
            // room grew it a little on every repaint.
            let body = ui.available_rect_before_wrap();
            ui.allocate_rect(body, egui::Sense::hover());
            let list = egui::Rect::from_min_size(body.min, egui::vec2(LIST_W, body.height()));
            let rule = egui::Rect::from_min_size(list.right_top(), egui::vec2(1.0, body.height()));
            let pane = body.with_min_x(rule.right());
            let pane_ui = |ui: &mut egui::Ui, rect: egui::Rect| {
                let mut child = ui.new_child(
                    egui::UiBuilder::new()
                        .max_rect(rect)
                        .layout(egui::Layout::top_down(egui::Align::Min)),
                );
                child.set_clip_rect(rect.intersect(ui.clip_rect()));
                child
            };
            source_list(&mut pane_ui(ui, list), s, &doc_scripts, &kits, &mut act);
            ui::card_vrule(ui, rule);
            let ui = &mut pane_ui(ui, pane);
            let Some(which) = current.clone() else {
                empty_pane(ui, &mut act);
                return;
            };
            let name = header.clone().map(|h| h.0).unwrap_or_default();
            script_head(
                ui,
                s,
                &name,
                where_,
                edited_note,
                &problems,
                held.as_deref(),
                &mut act,
            );
            ui::card_rule(ui);
            // The code and the output share what the head leaves, less the rule
            // between them; the output keeps at least its head.
            let room = ui.available_height() - 1.0;
            let output_h = s
                .output_h
                .unwrap_or(OUTPUT_H)
                .clamp(OUTPUT_HEAD_H, (room - CODE_MIN_H).max(OUTPUT_HEAD_H));
            let code_h = room - output_h;
            let bad = s.bad.as_ref().filter(|(w, _)| *w == which).map(|(_, l)| *l);
            let editable = !matches!(which, Which::Kit { .. });
            let code_id = s.code_id();
            // The pane's air around the text is the text's too: a press there
            // puts the caret at the end, or beside a line number at its line's
            // start. Sensed before the text edit, which takes the presses on it.
            let code_view = egui::Rect::from_min_size(
                ui.cursor().min,
                egui::vec2(ui.available_width(), code_h),
            );
            let blank = editable.then(|| {
                ui.interact(code_view, code_id.with("blank"), egui::Sense::click())
                    .on_hover_cursor(egui::CursorIcon::Text)
            });
            let mut laid: Option<egui::text_edit::TextEditOutput> = None;
            egui::ScrollArea::both()
                .id_salt("script-code")
                .max_height(code_h)
                .min_scrolled_height(code_h)
                .auto_shrink([false, false])
                .show(ui, |ui| {
                    ui.add_space(10.0);
                    let marks = super::script_help::Help::marks(&problems, caret, bad);
                    let out = ui::code_editor(ui, code_id, &mut s.buffer, &inks, &marks, editable);
                    edited |= out.response.changed();
                    // A block's closing word steps back as its last letter is
                    // typed, and steps in again when it grows into a longer name.
                    if typed
                        && out.response.changed()
                        && let Some(caret) = caret_byte(ui.ctx(), code_id, &s.buffer)
                    {
                        let again = s
                            .stepped
                            .take()
                            .and_then(|st| code_keys::step_in_again(&mut s.buffer, caret, &st));
                        let sel = again.or_else(|| {
                            let (sel, st) = code_keys::dedent_closer(&mut s.buffer, caret)?;
                            s.stepped = Some(st);
                            Some(sel)
                        });
                        if let Some(sel) = sel {
                            put_selection(ui.ctx(), code_id, &s.buffer, sel);
                            ui.ctx().request_repaint();
                        }
                    }
                    if std::mem::take(&mut s.focus_code) {
                        out.response.request_focus();
                    }
                    if let Some(line) = s.jump.take() {
                        let at = char_of_line(&s.buffer, line);
                        let mut state = out.state.clone();
                        state
                            .cursor
                            .set_char_range(Some(egui::text::CCursorRange::one(
                                egui::text::CCursor::new(at),
                            )));
                        state.store(ui.ctx(), out.response.id);
                        out.response.request_focus();
                    }
                    if let Some(r) = s.found.take()
                        && r.end <= s.buffer.len()
                        && s.buffer.is_char_boundary(r.start)
                        && s.buffer.is_char_boundary(r.end)
                    {
                        let at = |b: usize| egui::text::CCursor::new(s.buffer[..b].chars().count());
                        let mut state = out.state.clone();
                        state
                            .cursor
                            .set_char_range(Some(egui::text::CCursorRange::two(
                                at(r.start),
                                at(r.end),
                            )));
                        state.store(ui.ctx(), out.response.id);
                        out.response.request_focus();
                    }
                    if ui.input(|i| i.modifiers.command && i.key_pressed(egui::Key::Enter))
                        && out.response.has_focus()
                    {
                        act = Some(Act::Run);
                    }
                    drag_words(ui, s, &out);
                    paint_find_marks(ui, s, &out);
                    let list = offer
                        .as_ref()
                        .and_then(|_| ui.ctx().memory(|m| m.area_rect(id.with("suggest"))));
                    help_act = s.help.pane(
                        ui, id, &out, &s.buffer, code_view, list, doc, res, page, &problems,
                    );
                    laid = Some(out);
                });
            if let (Some(blank), Some(out)) = (blank, &laid)
                && blank.clicked()
            {
                let at = blank.interact_pointer_pos();
                let beside = at.filter(|p| {
                    p.x < out.response.rect.left() && out.response.rect.y_range().contains(p.y)
                });
                let to = match beside {
                    Some(p) => out
                        .galley
                        .cursor_from_pos(egui::vec2(0.0, p.y - out.galley_pos.y)),
                    None => out.galley.end(),
                };
                let mut state = out.state.clone();
                state
                    .cursor
                    .set_char_range(Some(egui::text::CCursorRange::one(to)));
                state.store(ui.ctx(), out.response.id);
                out.response.request_focus();
            }
            if let (Some(offer), Some(out)) = (&offer, &laid) {
                (row_clicked, row_hovered) = suggest_list(
                    ui.ctx(),
                    id,
                    offer,
                    out,
                    &s.buffer,
                    code_view,
                    s.suggest.highlight,
                    doc,
                );
            }
            let rule = ui::card_rule(ui);
            output_pane(ui, &s.output, output_h);
            let band = egui::Rect::from_center_size(
                rule.center(),
                egui::vec2(rule.width(), ui::DRAG_EDGE_GRAB),
            );
            let edge = ui::drag_edge(ui, id.with("output-edge"), band, false, &mut s.output_edge);
            if edge.dragged() {
                let max = (room - CODE_MIN_H).max(OUTPUT_HEAD_H);
                s.output_h = Some((output_h - edge.drag_delta().y).clamp(OUTPUT_HEAD_H, max));
            }
        });
        if let Some(which) = &current
            && let Some(chosen) = script_menu(ctx, &mut self.scripts, which)
        {
            act = Some(chosen);
        }
        // A press on the window or one of its popups gives it the keyboard; a
        // press anywhere else gives the keyboard back to the canvas.
        let pressed = ctx.input(|i| {
            i.pointer
                .any_pressed()
                .then(|| i.pointer.press_origin())
                .flatten()
        });
        if let Some(at) = pressed {
            let ours = [
                id,
                egui::Id::new(MENU_ID),
                egui::Id::new(FIND_ID),
                id.with("suggest"),
                id.with("problem"),
            ];
            self.scripts.keys = ctx.layer_id_at(at).is_some_and(|l| ours.contains(&l.id));
        }
        if ctx.input(|i| i.modifiers.command && i.key_pressed(egui::Key::Enter))
            && self.scripts.current.is_some()
            && act.is_none()
            && !ctx.egui_wants_keyboard_input()
        {
            act = Some(Act::Run);
        }
        match help_act {
            Some(super::script_help::HelpAct::Select(r)) => self.scripts.found = Some(r),
            Some(super::script_help::HelpAct::Replace(r, with))
                if r.end <= self.scripts.buffer.len()
                    && self.scripts.buffer.is_char_boundary(r.start)
                    && self.scripts.buffer.is_char_boundary(r.end) =>
            {
                let end = r.start + with.len();
                self.scripts.buffer.replace_range(r, &with);
                self.scripts.found = Some(end..end);
                edited = true;
            }
            _ => {}
        }
        if edited {
            self.scripts.help.edited(ctx);
            self.store_buffer();
        }
        if let Some(i) = row_hovered {
            self.scripts.suggest.highlight = i;
        }
        if let (Some(i), Some(offer)) = (row_clicked, &offer) {
            self.take_script_row(ctx, self.scripts.code_id(), &offer.completion, i);
        }
        self.scripts_act(act);
        let code_id = self.scripts.code_id();
        self.scripts.code_focused = ctx.memory(|m| m.has_focus(code_id));
    }

    /// Act on what the window asked for.
    fn scripts_act(&mut self, act: Option<Act>) {
        let Some(act) = act else { return };
        // Every act reads the script as last typed. A write refused over an
        // outside edit has just moved the typing elsewhere, which the status line
        // says; the act waits for the person to look.
        if !self.flush_library_write() {
            return;
        }
        let current = self.scripts.current.clone();
        match act {
            Act::Close => {
                self.flush_library_write();
                self.scripts.open = false;
            }
            Act::Pick(w) => self.pick_script(w),
            Act::RowMenu(w, at) => {
                if current.as_ref() != Some(&w) {
                    self.pick_script(w);
                }
                self.scripts.menu = Some(Menu {
                    at: MenuAt::Press(at),
                    highlight: None,
                });
            }
            Act::New => self.add_unsaved(String::new()),
            Act::Run => {
                if let Some(w) = current {
                    self.start_script_run(w);
                }
            }
            Act::OpenFolder => library::show_folder(),
            Act::SaveToLibrary => {
                let Some(w) = current else { return };
                let Some((name, source)) = self.script_named(&w) else {
                    return;
                };
                let name = library::unique_name(&name);
                match library::write(&name, &source) {
                    Ok(()) => {
                        self.scripts.library = library::list();
                        self.session.info(format!("Saved {name} to the library"));
                        let saved = Which::Library(name);
                        // An unsaved script is saved by this, so it leaves the
                        // list, its form's values going with it; a document's or
                        // a kit's stays where it is, copied.
                        if let Which::Unsaved(serial) = w {
                            self.scripts.unsaved.retain(|u| u.serial != serial);
                            if let Some(values) = self.scripts.last_values.remove(&values_key(&w)) {
                                self.scripts.last_values.insert(values_key(&saved), values);
                            }
                        }
                        self.pick_script(saved);
                        self.scripts.focus_code = true;
                    }
                    Err(e) => self.session.fail(format!("Couldn't save {name}: {e}")),
                }
            }
            Act::CopyToDocument => {
                let Some(w) = current else { return };
                let Some((name, source)) = self.script_named(&w) else {
                    return;
                };
                if let Some(id) = self.add_document_script(&name, source) {
                    self.pick_script(Which::Document(id));
                }
            }
            Act::Duplicate => {
                let Some(w) = current else { return };
                let Some((name, source)) = self.script_named(&w) else {
                    return;
                };
                match w {
                    Which::Library(_) => match library::duplicate(&name) {
                        Ok(copy) => {
                            self.scripts.library = library::list();
                            self.pick_script(Which::Library(copy));
                        }
                        Err(e) => self.session.fail(format!("Couldn't duplicate {name}: {e}")),
                    },
                    Which::Unsaved(_) => self.add_unsaved(source),
                    Which::Document(_) | Which::Kit { .. } => {
                        if let Some(id) = self.add_document_script(&name, source) {
                            self.pick_script(Which::Document(id));
                        }
                    }
                }
            }
            Act::StartRename => {
                if let Some((name, _)) = current.as_ref().and_then(|w| self.script_named(w)) {
                    self.scripts.renaming = Some(name);
                }
            }
            Act::Rename(new) => {
                self.scripts.renaming = None;
                let new = new.trim().to_owned();
                let Some(w) = current else { return };
                // An empty or unchanged name keeps the old one, as a layer's does.
                if new.is_empty() || self.script_named(&w).is_some_and(|(old, _)| old == new) {
                    return;
                }
                match w {
                    Which::Document(id) => {
                        self.session.name_step("Rename script", icon::FILE_CODE);
                        self.session
                            .commit(Transaction(vec![Operation::SetScriptName {
                                id,
                                name: new,
                            }]));
                    }
                    Which::Library(old) => {
                        self.flush_library_write();
                        match library::rename(&old, &new) {
                            Ok(()) => {
                                self.scripts.library = library::list();
                                self.scripts.current = Some(Which::Library(new));
                            }
                            Err(e) => self.session.fail(format!("Couldn't rename {old}: {e}")),
                        }
                    }
                    Which::Unsaved(serial) => {
                        if let Some(u) =
                            self.scripts.unsaved.iter_mut().find(|u| u.serial == serial)
                        {
                            u.name = new;
                        }
                    }
                    Which::Kit { .. } => {}
                }
            }
            Act::Delete => {
                let Some(w) = current else { return };
                match w {
                    Which::Document(id) => {
                        self.session.name_step("Delete script", icon::FILE_CODE);
                        self.session
                            .commit(Transaction(vec![Operation::RemoveScript { id }]));
                    }
                    // A file goes with no undo, so it asks first.
                    Which::Library(name) => {
                        self.flush_library_write();
                        self.scripts.deleting = Some(name);
                        return;
                    }
                    Which::Unsaved(serial) => {
                        self.scripts.unsaved.retain(|u| u.serial != serial);
                        self.scripts
                            .last_values
                            .remove(&values_key(&Which::Unsaved(serial)));
                    }
                    Which::Kit { .. } => return,
                }
                self.scripts.current = None;
                self.scripts.buffer.clear();
                self.scripts.shown += 1;
            }
            Act::ShowInFolder => library::show_folder(),
        }
    }

    /// *Delete script?* for a library script: its file goes for good, so *Cancel*
    /// and *Delete*. `Escape`, the backdrop and the ✕ cancel.
    fn script_delete_confirmation(&mut self, ctx: &egui::Context) {
        let Some(name) = self.scripts.deleting.clone() else {
            return;
        };
        let body =
            format!("“{name}” is deleted from the library folder for good. This cannot be undone.");
        let (mut delete, mut cancel) = (false, false);
        let modal = settings::card("confirm-script-delete", ctx, |ui| {
            ui.set_width(ui::menu_inner_w(320.0, settings::PAD));
            if settings::modal_title(ui, "Delete script?") {
                cancel = true;
            }
            settings::title_gap(ui);
            ui.add(
                egui::Label::new(
                    egui::RichText::new(body)
                        .size(ui::CONTROL_PT)
                        .color(theme::text::muted()),
                )
                .wrap(),
            );
            match settings::danger_footer(ui, "Delete") {
                Some(settings::Decision::Save) => delete = true,
                Some(settings::Decision::Cancel) => cancel = true,
                None => {}
            }
        });
        if delete {
            self.scripts.deleting = None;
            if let Err(e) = library::delete(&name) {
                self.session.fail(format!("Couldn't delete {name}: {e}"));
            }
            self.scripts.library = library::list();
            if self.scripts.current == Some(Which::Library(name)) {
                self.scripts.current = None;
                self.scripts.buffer.clear();
                self.scripts.shown += 1;
            }
        } else if cancel || modal.should_close() {
            self.scripts.deleting = None;
        }
    }

    /// Put a copy of a script into this document, as one undo step; answers its id.
    fn add_document_script(&mut self, name: &str, source: String) -> Option<ScriptId> {
        let id = self.session.ids.mint_script();
        let name = mastik_core::script::free_name(self.session.doc.scripts(), name);
        let index = self.session.doc.scripts().len();
        self.session
            .name_step("Copy script into document", icon::FILE_CODE);
        let kept = source.clone();
        let added = self
            .session
            .commit(Transaction(vec![Operation::AddScript {
                script: Script { id, name, source },
                index,
            }]))
            .then_some(id)?;
        // The person put this text here, so it needs no disclosure.
        if let Some(key) = self.disclosure_key(&Which::Document(added)) {
            self.prefs.mark_disclosed(&key, &kept);
        }
        Some(added)
    }

    /// Record a document script's source as the person's own once they write in
    /// it, so its run shows no disclosure. Saved with the next prefs save.
    fn authored(&mut self, which: &Which) {
        if let Some(key) = self.disclosure_key(which) {
            self.prefs.mark_disclosed(&key, &self.scripts.buffer);
        }
    }

    /// The run form, while open: a field per input, *Cancel* and *Run*.
    fn script_form_ui(&mut self, ctx: &egui::Context, canvas: egui::Rect) {
        let Some(mut form) = self.scripts.form.take() else {
            return;
        };
        let enter =
            ctx.input(|i| i.key_pressed(egui::Key::Enter)) && !ctx.egui_wants_keyboard_input();
        let escape = ctx.input(|i| i.key_pressed(egui::Key::Escape));
        let mut decided: Option<bool> = None;
        let outside =
            settings::canvas_card(ctx, "script-form", canvas, FORM_W, CardHold::Modal, |ui| {
                if settings::card_head(ui, FORM_W, &form.name) {
                    decided = Some(false);
                }
                egui::Frame::NONE
                    .inner_margin(egui::Margin::symmetric(settings::CANVAS_CARD_PAD as i8, 4))
                    .show(ui, |ui| {
                        ui.spacing_mut().item_spacing.y = 8.0;
                        let w = FORM_W - 2.0 * settings::CANVAS_CARD_PAD;
                        if !form.header.description.is_empty() {
                            settings::caption_unled(ui, &form.header.description);
                        }
                        for input in &form.header.inputs {
                            let label = input.label.clone().unwrap_or_else(|| input.name.clone());
                            let value =
                                form.values.entry(input.name.clone()).or_insert(Value::Null);
                            // A switch carries its label, as Settings' do.
                            if input.ty == mastik_vm::InputType::Boolean {
                                let on = value.as_bool().unwrap_or(false);
                                if ui::switch_row(ui, &label, on, settings::SWITCH_H).clicked() {
                                    *value = Value::Bool(!on);
                                }
                                continue;
                            }
                            ui::labelled(ui, &label, |ui| input_field(ui, w, input, value));
                        }
                    });
                let foot = settings::card_foot(
                    ui,
                    FORM_W,
                    "",
                    &[("Cancel", FieldButton::Off), ("Run", FieldButton::Primary)],
                    None,
                );
                match foot {
                    Some(0) => decided = Some(false),
                    Some(_) => decided = Some(true),
                    None => {}
                }
            });
        if escape || outside {
            decided = Some(false);
        }
        if enter {
            decided = Some(true);
        }
        match decided {
            None => self.scripts.form = Some(form),
            Some(false) => {}
            // A colour that is not one keeps the form up, saying so.
            Some(true) if let Err(why) = form_colours(&form.header, &mut form.values) => {
                self.session.fail(why);
                self.scripts.form = Some(form);
            }
            Some(true) => {
                self.scripts
                    .last_values
                    .insert(values_key(&form.which), form.values.clone());
                self.run_script_now(form.which, form.name, form.source, form.values);
            }
        }
    }

    /// The first-run disclosure: who wrote the script, what a script can and
    /// cannot do, its source, and *Run*.
    fn disclosure_ui(&mut self, ctx: &egui::Context, canvas: egui::Rect) {
        let Some(d) = self.scripts.disclosure.take() else {
            return;
        };
        let title = match &d.which {
            Which::Kit { kit_name, .. } => {
                format!("“{}” came with the kit “{kit_name}”", d.name)
            }
            _ => format!("“{}” came with this document", d.name),
        };
        let mut decided: Option<u8> = None;
        // Laid out as Luau reads it: no line break or character hides code.
        let mut shown = mastik_vm::as_read(&d.source);
        let lines = shown.lines().count().max(1);
        let names = mastik_vm::names_used(&d.source);
        let outside = settings::canvas_card(
            ctx,
            "script-disclosure",
            canvas,
            DISCLOSE_W,
            CardHold::Modal,
            |ui| {
                if settings::card_head(ui, DISCLOSE_W, &title) {
                    decided = Some(0);
                }
                egui::Frame::NONE
                    .inner_margin(egui::Margin::symmetric(settings::CANVAS_CARD_PAD as i8, 4))
                    .show(ui, |ui| {
                        ui.spacing_mut().item_spacing.y = 12.0;
                        ui.label(
                            egui::RichText::new(DISCLOSURE)
                                .size(ui::CONTROL_PT)
                                .color(theme::text::strong()),
                        );
                        // The recessed field ground, the code inset by its own gutter.
                        ui::field_frame()
                            .inner_margin(egui::Margin::symmetric(0, 10))
                            .show(ui, |ui| {
                                // Both ways: a line past the box's edge must not
                                // hide what the script does.
                                egui::ScrollArea::both()
                                    .id_salt("disclosed-code")
                                    .max_height(DISCLOSE_CODE_H)
                                    .show(ui, |ui| {
                                        ui::code_editor(
                                            ui,
                                            egui::Id::new("disclosed-code-text"),
                                            &mut shown,
                                            &inks,
                                            &[],
                                            false,
                                        );
                                    });
                            });
                        let listed = names.join(", ");
                        let lines = match lines {
                            1 => "1 line".to_owned(),
                            n => format!("{n} lines"),
                        };
                        settings::caption_unled(
                            ui,
                            &match listed.is_empty() {
                                true => lines,
                                false => format!("{lines} · names: {listed}"),
                            },
                        );
                    });
                let foot = settings::card_foot_with(
                    ui,
                    DISCLOSE_W,
                    &[("Cancel", FieldButton::Off), ("Run", FieldButton::Primary)],
                    None,
                    |ui, row| {
                        let mut child = ui.new_child(
                            egui::UiBuilder::new()
                                .max_rect(row)
                                .layout(egui::Layout::left_to_right(egui::Align::Center)),
                        );
                        if settings::text_button(&mut child, "Open in editor", FieldButton::Off)
                            .clicked()
                        {
                            decided = Some(2);
                        }
                    },
                );
                match foot {
                    Some(0) => decided = Some(0),
                    Some(_) => decided = Some(1),
                    None => {}
                }
            },
        );
        if outside || ctx.input(|i| i.key_pressed(egui::Key::Escape)) {
            decided = Some(0);
        }
        match decided {
            None => self.scripts.disclosure = Some(d),
            Some(0) => {}
            Some(2) => {
                self.open_scripts();
                self.pick_script(d.which);
            }
            Some(_) => {
                if let Some(key) = self.disclosure_key(&d.which) {
                    self.prefs.mark_disclosed(&key, &d.source);
                    self.prefs.save();
                }
                self.script_form_or_run(d.which, d.name, d.source);
            }
        }
    }
}

/// The library folder as the editor uses it: this machine's scripts folder, and
/// no scripts when there is no folder to keep them in.
mod library {
    pub(crate) use crate::scripts::library::LibraryScript;
    use crate::scripts::library::{Library, ScriptError};

    #[cfg(test)]
    thread_local! {
        /// Under test, the folder this thread's editor keeps its library in;
        /// none until a test sets it, and never this machine's real folder.
        pub(crate) static TEST_DIR: std::cell::RefCell<Option<std::path::PathBuf>> =
            const { std::cell::RefCell::new(None) };
    }

    fn open() -> Result<Library, ScriptError> {
        #[cfg(test)]
        let lib = TEST_DIR.with(|d| d.borrow().clone()).map(Library::new);
        #[cfg(not(test))]
        let lib = Library::open();
        lib.ok_or_else(|| ScriptError::Missing("the scripts folder".into()))
    }

    pub(crate) fn list() -> Vec<LibraryScript> {
        open().map(|l| l.list()).unwrap_or_default()
    }

    /// The script `name`'s file as it reads now.
    pub(crate) fn read(name: &str) -> Result<String, ScriptError> {
        open()?.read(name)
    }

    /// Bring the folder's API definitions up to this version's.
    pub(crate) fn write_definitions() -> Result<(), ScriptError> {
        open()?.write_definitions(&mastik_vm::definitions())
    }

    pub(crate) fn write(name: &str, source: &str) -> Result<(), ScriptError> {
        open()?.write(name, source).map(|_| ())
    }

    pub(crate) fn rename(from: &str, to: &str) -> Result<(), ScriptError> {
        open()?.rename(from, to).map(|_| ())
    }

    pub(crate) fn duplicate(name: &str) -> Result<String, ScriptError> {
        open()?.duplicate(name)
    }

    pub(crate) fn delete(name: &str) -> Result<(), ScriptError> {
        open()?.delete(name)
    }

    /// `wanted`, numbered past the library's scripts that have it.
    pub(crate) fn unique_name(wanted: &str) -> String {
        open().map_or_else(|_| wanted.to_owned(), |l| l.unique_name(wanted))
    }

    /// Open the folder in the system's file manager.
    pub(crate) fn show_folder() {
        if let Ok(l) = open() {
            l.reveal();
        }
    }
}

/// What the disclosure says before the source.
const DISCLOSURE: &str = "Whoever made this file wrote it, not you. Read it before running it. \
    A script can change this document — its layers, styles, variables and components — and \
    nothing else: it can't open files, reach the network, or keep running after it finishes, \
    and its run is one step you can undo.";

/// What a script's last form values are kept under: the script itself, not its
/// name, which a library and a document script may share.
fn values_key(which: &Which) -> String {
    format!("{which:?}")
}

/// `s` with its first letter lowered, to run on after a name.
fn lower_first(s: &str) -> String {
    let mut c = s.chars();
    match c.next() {
        Some(f) => f.to_lowercase().chain(c).collect(),
        None => String::new(),
    }
}

/// The char index where line `line` (0-based) of `s` starts.
fn char_of_line(s: &str, line: usize) -> usize {
    let mut chars = 0;
    for (i, l) in s.split('\n').enumerate() {
        if i == line {
            return chars;
        }
        chars += l.chars().count() + 1;
    }
    s.chars().count()
}

/// The caret's byte offset in `text`, the text edit `id` holds; none before
/// the edit has placed one.
fn caret_byte(ctx: &egui::Context, id: egui::Id, text: &str) -> Option<usize> {
    egui::TextEdit::load_state(ctx, id)
        .and_then(|s| s.cursor.char_range())
        .map(|r| super::char_to_byte(text, r.primary.index.0))
}

/// The text edit `id`'s selection in `text`, in bytes; none before the edit
/// has placed a caret.
fn selection(ctx: &egui::Context, id: egui::Id, text: &str) -> Option<code_keys::Sel> {
    let r = egui::TextEdit::load_state(ctx, id)?.cursor.char_range()?;
    Some(code_keys::Sel {
        anchor: super::char_to_byte(text, r.secondary.index.0),
        caret: super::char_to_byte(text, r.primary.index.0),
    })
}

/// Give the text edit `id` the selection `sel` in `text`, for its next draw. An
/// offset past the end or inside a character goes back to the nearest boundary
/// rather than panicking.
fn put_selection(ctx: &egui::Context, id: egui::Id, text: &str, sel: code_keys::Sel) {
    let at =
        |b: usize| egui::text::CCursor::new(text[..text.floor_char_boundary(b)].chars().count());
    let mut state = egui::TextEdit::load_state(ctx, id).unwrap_or_default();
    state
        .cursor
        .set_char_range(Some(egui::text::CCursorRange::two(
            at(sel.anchor),
            at(sel.caret),
        )));
    state.store(ctx, id);
}

/// The code pane's edit for `key` pressed with `m`, applied to `text`; `None`
/// for a key the text edit keeps. `Alt+↑`/`↓` at the text's first or last
/// line is still taken, so it never moves the caret instead.
fn edit_key(
    text: &mut String,
    sel: code_keys::Sel,
    key: egui::Key,
    m: egui::Modifiers,
) -> Option<code_keys::Sel> {
    use egui::Key;
    let alt = m.alt && !m.command && !m.ctrl;
    match key {
        Key::Enter if m.is_none() => Some(code_keys::newline(text, sel)),
        Key::Tab if m.is_none() => Some(code_keys::indent(text, sel)),
        Key::Tab if m.matches_exact(egui::Modifiers::SHIFT) => Some(code_keys::outdent(text, sel)),
        Key::Slash if m.matches_exact(egui::Modifiers::COMMAND) => {
            Some(code_keys::toggle_comment(text, sel))
        }
        Key::ArrowUp | Key::ArrowDown if alt && m.shift => {
            Some(code_keys::duplicate_lines(text, sel, key == Key::ArrowUp))
        }
        Key::ArrowUp | Key::ArrowDown if alt => {
            Some(code_keys::move_lines(text, sel, key == Key::ArrowUp).unwrap_or(sel))
        }
        Key::Backspace if m.is_none() => code_keys::backspace_pair(text, sel),
        _ => None,
    }
}

/// Whether the text edit changes its text or its caret on `e`: once one such
/// event is left to it, the events after it are left to it too.
fn edits_text(e: &egui::Event) -> bool {
    use egui::Key;
    match e {
        egui::Event::Text(_) | egui::Event::Paste(_) | egui::Event::Cut => true,
        egui::Event::Key {
            key,
            pressed: true,
            modifiers,
            ..
        } => {
            modifiers.command
                || matches!(
                    key,
                    Key::Enter
                        | Key::Tab
                        | Key::Backspace
                        | Key::Delete
                        | Key::ArrowLeft
                        | Key::ArrowRight
                        | Key::ArrowUp
                        | Key::ArrowDown
                        | Key::Home
                        | Key::End
                        | Key::PageUp
                        | Key::PageDown
                )
        }
        _ => false,
    }
}

/// The highlighting: the language's token kinds in the theme's inks.
pub(crate) fn inks(source: &str) -> ui::CodeInks {
    mastik_vm::tokens(source)
        .into_iter()
        .filter_map(|(r, t)| {
            use mastik_vm::Tok;
            let ink = match t {
                Tok::Keyword => theme::code::keyword(),
                Tok::Constant | Tok::Number => theme::code::literal(),
                Tok::String => theme::code::string(),
                Tok::Comment => theme::text::faint(),
                Tok::Api => color::accent_300(),
                Tok::Builtin => theme::code::builtin(),
                Tok::Function => theme::code::function(),
                Tok::Plain => return None,
            };
            Some((r, ink))
        })
        .collect()
}

/// The list of scripts: a search field, a section per source, *New script* and
/// the folder button at the foot.
fn source_list(
    ui: &mut egui::Ui,
    s: &mut Scripts,
    doc: &[Script],
    kits: &[(String, String, Vec<Script>)],
    act: &mut Option<Act>,
) {
    egui::Frame::NONE
        .inner_margin(egui::Margin::same(8))
        .show(ui, |ui| {
            ui.spacing_mut().item_spacing.y = 1.0;
            let w = ui.available_width();
            ui::search_box(
                ui,
                egui::vec2(w, ui::CONTROL_H),
                &mut s.search,
                "Search scripts",
                egui::Id::new("script-search"),
            );
            ui.add_space(6.0);
            let needle = s.search.trim().to_lowercase();
            let hit = |name: &str| needle.is_empty() || name.to_lowercase().contains(&needle);
            let list_h = ui.available_height() - ui::CONTROL_H - 8.0;
            egui::ScrollArea::vertical()
                .id_salt("script-list")
                .max_height(list_h.max(40.0))
                .auto_shrink([false, false])
                .show(ui, |ui| {
                    let rows: Vec<(&str, Vec<ListRow>)> = vec![
                        (
                            "This document",
                            doc.iter()
                                .map(|sc| {
                                    (
                                        Which::Document(sc.id),
                                        sc.name.clone(),
                                        inputs_note(&sc.source),
                                        icon::FILE,
                                    )
                                })
                                .collect(),
                        ),
                        (
                            "Library",
                            s.library
                                .iter()
                                .map(|l| {
                                    (
                                        Which::Library(l.name.clone()),
                                        l.name.clone(),
                                        inputs_note(&l.source),
                                        icon::FOLDER_SIMPLE,
                                    )
                                })
                                .collect(),
                        ),
                    ];
                    let kit_rows = kits.iter().map(|(kit, name, scripts)| {
                        (
                            format!("Kit · {name}"),
                            scripts
                                .iter()
                                .map(|sc| {
                                    (
                                        Which::Kit {
                                            kit: kit.clone(),
                                            kit_name: name.clone(),
                                            id: sc.id,
                                        },
                                        sc.name.clone(),
                                        inputs_note(&sc.source),
                                        icon::PACKAGE,
                                    )
                                })
                                .collect::<Vec<_>>(),
                        )
                    });
                    let unsaved: Vec<_> = s
                        .unsaved
                        .iter()
                        .map(|u| {
                            (
                                Which::Unsaved(u.serial),
                                u.name.clone(),
                                String::new(),
                                icon::FILE_CODE,
                            )
                        })
                        .collect();
                    let sections = rows
                        .into_iter()
                        .map(|(t, r)| (t.to_owned(), r))
                        .chain(kit_rows)
                        .chain(std::iter::once(("Unsaved".to_owned(), unsaved)));
                    for (title, items) in sections {
                        let items: Vec<_> = items.into_iter().filter(|r| hit(&r.1)).collect();
                        if items.is_empty() {
                            continue;
                        }
                        crate::palette::section_label(ui, &title);
                        for (which, name, note, glyph) in items {
                            let lit = s.current.as_ref() == Some(&which);
                            let renaming = lit && s.renaming.is_some();
                            // The field covers the name and the note while it is up.
                            let (shown, note) = match renaming {
                                true => ("", ""),
                                false => (name.as_str(), note.as_str()),
                            };
                            let resp = list_row(ui, glyph, shown, note, lit);
                            if renaming {
                                row_rename(ui, s, resp.rect, act);
                                continue;
                            }
                            let kit = matches!(which, Which::Kit { .. });
                            // A double-click or the rename keys rename, as a
                            // layers row does; a right-click opens the
                            // script's menu at the press.
                            // The first click of the double picked the row.
                            if lit
                                && !kit
                                && (ui::double_clicked(ui, &resp) || ui::rename_key(ui, &resp))
                            {
                                s.renaming = Some(name.clone());
                            } else if resp.secondary_clicked() {
                                let at = resp.interact_pointer_pos().unwrap_or(resp.rect.center());
                                *act = Some(Act::RowMenu(which, at));
                            } else if resp.clicked() {
                                *act = Some(Act::Pick(which));
                            }
                        }
                    }
                });
            ui.add_space(8.0);
            ui.horizontal(|ui| {
                ui.spacing_mut().item_spacing.x = 6.0;
                let new_w = ui.available_width() - ui::CONTROL_H - 6.0;
                if ui::action_button(
                    ui,
                    icon::PLUS,
                    "New script",
                    FieldButton::Off,
                    egui::vec2(new_w, ui::CONTROL_H),
                )
                .clicked()
                {
                    *act = Some(Act::New);
                }
                if ui::field_button(ui, icon::FOLDER_OPEN, ui::CONTROL_H, FieldButton::Off)
                    .on_hover_text("Open the scripts folder")
                    .clicked()
                {
                    *act = Some(Act::OpenFolder);
                }
            });
        });
}

/// One row of the source list: the script, its name, its note and its source's glyph.
type ListRow = (Which, String, String, &'static str);

/// The rename field over the open script's list row `row`, where its name was
/// drawn, as a page's: the name selected on its first frame, `Enter` and a
/// press elsewhere keeping the typed name, `Escape` the old one.
fn row_rename(ui: &mut egui::Ui, s: &mut Scripts, row: egui::Rect, act: &mut Option<Act>) {
    let Some(text) = s.renaming.as_mut() else {
        return;
    };
    let id = egui::Id::new("script-rename");
    let field = egui::Rect::from_min_max(
        egui::pos2(
            row.left() + crate::palette::ROW_LABEL_X - ui::INLINE_EDIT_PAD_X,
            row.top() + 3.0,
        ),
        egui::pos2(row.right() - 6.0, row.bottom() - 3.0),
    );
    // In a child, so the field's placing leaves the rows below where they are.
    let mut child = ui.new_child(egui::UiBuilder::new().id(id.with("ui")).max_rect(field));
    let first = ui::widget_rect(ui.ctx(), id).is_none();
    let resp = ui::label_edit(&mut child, field, text, id);
    if first {
        resp.request_focus();
        let all = egui::text::CCursorRange::two(
            egui::text::CCursor::new(0),
            egui::text::CCursor::new(text.chars().count()),
        );
        let mut state = egui::TextEdit::load_state(ui.ctx(), id).unwrap_or_default();
        state.cursor.set_char_range(Some(all));
        state.store(ui.ctx(), id);
    }
    let pressed_elsewhere = resp.has_focus()
        && ui.input(|i| i.pointer.any_pressed())
        && ui
            .ctx()
            .pointer_interact_pos()
            .is_none_or(|p| !field.contains(p));
    if pressed_elsewhere {
        ui.ctx().memory_mut(|m| m.surrender_focus(id));
        *act = Some(Act::Rename(text.clone()));
    } else if resp.lost_focus() {
        // Read before anything else: once focus is gone, only the key says
        // which of the two it was.
        match ui.input(|i| i.key_pressed(egui::Key::Escape)) {
            true => s.renaming = None,
            false => *act = Some(Act::Rename(text.clone())),
        }
    }
}

/// `4 inputs` for a script whose header declares some; empty otherwise.
fn inputs_note(source: &str) -> String {
    match mastik_vm::header(source).map(|h| h.inputs.len()) {
        Ok(0) | Err(_) => String::new(),
        Ok(1) => "1 input".into(),
        Ok(n) => format!("{n} inputs"),
    }
}

/// One script row: a list row's height and ground, which answer the pointer,
/// in the palette's row layout under the palette's section labels: its
/// source's glyph, its name, its note. The note keeps its room and a long name
/// is cut, shown whole in the row's tooltip.
fn list_row(ui: &mut egui::Ui, glyph: &str, name: &str, note: &str, lit: bool) -> egui::Response {
    let (rect, resp) = ui.allocate_exact_size(
        egui::vec2(ui.available_width(), ui::MENU_ROW_H),
        egui::Sense::click(),
    );
    let p = ui.painter();
    ui::list_row_ground(p, rect, lit, resp.hovered());
    crate::palette::row_glyph(p, rect, glyph, theme::text::dim());
    let font = egui::FontId::proportional(ui::CONTROL_PT);
    let label = crate::palette::lit_label(name, &[], font, color::text(), color::text());
    let cut = crate::palette::row_texts_note_kept(
        p,
        rect,
        rect.right() - 8.0,
        label,
        Some((note, theme::text::faint())),
    );
    match cut {
        true => resp.on_hover_text(name),
        false => resp,
    }
}

/// Above the empty pane's state.
const EMPTY_TOP: f32 = 64.0;

/// The pane's state with no script open.
fn empty_pane(ui: &mut egui::Ui, act: &mut Option<Act>) {
    ui.add_space(EMPTY_TOP);
    ui::empty_state(
        ui,
        Some(icon::FILE_CODE),
        "No script open",
        "Pick one from the list, or start a new script",
        |ui| {
            if settings::text_button(ui, "New script", FieldButton::Off).clicked() {
                *act = Some(Act::New);
            }
        },
    );
}

/// The script's head: its name, where it lives, its problem counts (a click
/// walks to the next), the ⋯ menu's and find's toggles, and *Run*, disabled
/// with `held`'s reason while a syntax error holds it; and find's field under
/// it while open.
#[expect(
    clippy::too_many_arguments,
    reason = "the head shows the script's name, place, edit state, problems and hold; \
              bundling them would only move the list"
)]
fn script_head(
    ui: &mut egui::Ui,
    s: &mut Scripts,
    name: &str,
    where_: &str,
    edited: bool,
    problems: &[mastik_vm::Problem],
    held: Option<&str>,
    act: &mut Option<Act>,
) {
    let (rect, _) = ui.allocate_exact_size(
        egui::vec2(ui.available_width(), HEAD_H),
        egui::Sense::hover(),
    );
    let mut child = ui.new_child(
        egui::UiBuilder::new()
            .max_rect(rect.shrink2(egui::vec2(HEAD_INSET, 0.0)))
            .layout(egui::Layout::left_to_right(egui::Align::Center)),
    );
    let ui = &mut child;
    ui.spacing_mut().item_spacing.x = 8.0;
    ui.label(
        egui::RichText::new(name)
            .size(ui::HEAD_PT)
            .color(color::text()),
    );
    let note = match edited {
        true => format!("{where_} · edited"),
        false => where_.to_owned(),
    };
    ui.label(
        egui::RichText::new(note)
            .size(ui::CAPTION_PT)
            .color(theme::text::faint()),
    );
    if let Some(counts) = super::script_help::counts(ui, problems)
        && counts.clicked()
    {
        let caret = caret_byte(ui.ctx(), s.code_id(), &s.buffer).unwrap_or(0);
        s.found = super::script_help::next_problem(problems, caret);
    }
    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
        ui.spacing_mut().item_spacing.x = 6.0;
        let run = match held {
            Some(why) => {
                settings::text_button(ui, "Run", FieldButton::Disabled).on_hover_text(format!(
                    "{why} · {}",
                    crate::input::keys("Ctrl+Enter puts the caret on it")
                ))
            }
            None => settings::text_button(ui, "Run", FieldButton::Primary)
                .on_hover_text(crate::input::keys("Run the script · Ctrl+Enter")),
        };
        if run.clicked() && held.is_none() {
            *act = Some(Act::Run);
        }
        // The ⋯ and the loupe are toggles, on while their menu or field is up,
        // as a card head's ⋯ and the inspector's disclosure buttons are. A
        // row's right-click menu is not the ⋯'s, so it leaves the ⋯ off.
        let from_button = s
            .menu
            .as_ref()
            .is_some_and(|m| matches!(m.at, MenuAt::Button(_)));
        let menu = ui::field_button(
            ui,
            icon::DOTS_THREE,
            ui::CONTROL_H,
            FieldButton::on_if(from_button),
        )
        .on_hover_text("More");
        if menu.clicked() {
            s.menu = match from_button {
                true => None,
                false => Some(Menu {
                    at: MenuAt::Button(menu.rect),
                    highlight: None,
                }),
            };
        }
        if ui::field_button(
            ui,
            icon::MAGNIFYING_GLASS,
            ui::CONTROL_H,
            FieldButton::on_if(s.find.is_some()),
        )
        .on_hover_text(crate::input::keys("Find · Ctrl+F"))
        .clicked()
        {
            match s.find {
                Some(_) => close_find(s),
                None => {
                    s.find = Some(String::new());
                    s.find_focus = true;
                }
            }
        }
    });
    let find_field = egui::Id::new(FIND_FIELD);
    // Only from inside the editor: on the canvas, Ctrl+F is the app's own Find.
    let in_editor = ui
        .ctx()
        .memory(|m| m.has_focus(s.code_id()) || m.has_focus(find_field));
    if in_editor
        && ui
            .ctx()
            .input(|i| i.modifiers.command && i.key_pressed(egui::Key::F))
    {
        s.find = Some(s.find.take().unwrap_or_default());
        s.find_focus = true;
    }
    if s.find.is_some() {
        find_row(ui, s, rect);
    }
}

/// Find's row under the head's right end, kept on screen as a menu is: the
/// canvas Find's field with the count at its end, its previous and next arrows
/// and its close. Every match is marked in the code (`paint_find_marks`) and
/// the field keeps the keyboard: `Enter` steps on, `Shift+Enter` back, and
/// closing (`Escape` or ✕) selects the current match in the code.
fn find_row(ui: &mut egui::Ui, s: &mut Scripts, head: egui::Rect) {
    const BTN: f32 = 26.0;
    const GAP: f32 = 6.0;
    let ctx = ui.ctx().clone();
    let field_id = egui::Id::new(FIND_FIELD);
    // Asked before the field is drawn: `Enter` steps while it has the keyboard,
    // and the field keeps it.
    let in_field = ctx.memory(|m| m.has_focus(field_id));
    let (enter, shift, escape) = ctx.input(|i| {
        (
            i.key_pressed(egui::Key::Enter),
            i.modifiers.shift,
            i.key_pressed(egui::Key::Escape),
        )
    });
    let mut step: Option<isize> = (enter && in_field).then_some(if shift { -1 } else { 1 });
    let mut close = false;
    let size = egui::vec2(
        FIND_W + 3.0 * BTN + 2.0 * GAP + 2.0 + 2.0 * (ui::MENU_PAD + ui::MENU_BORDER),
        ui::CONTROL_H + 2.0 * (ui::MENU_PAD + ui::MENU_BORDER),
    );
    let at = head.right_bottom() + egui::vec2(-HEAD_INSET - size.x, ui::MENU_GAP);
    let before = s.find.clone().unwrap_or_default();
    let n = s.find_hits.len();
    let count = match (before.is_empty(), n) {
        (true, _) => String::new(),
        (false, 0) => "0".to_owned(),
        (false, n) => format!("{} of {n}", s.find_current.min(n - 1) + 1),
    };
    egui::Area::new(egui::Id::new(FIND_ID))
        .order(egui::Order::Foreground)
        .fixed_pos(ui::menu_place(&ctx, at, size))
        .show(&ctx, |ui| {
            ui::menu_frame(ui::MENU_PAD).show(ui, |ui| {
                ui.horizontal(|ui| {
                    ui.spacing_mut().item_spacing.x = 0.0;
                    let Some(find) = s.find.as_mut() else { return };
                    let (resp, _) = crate::find::field(
                        ui,
                        FIND_FIELD,
                        find,
                        "Find in script",
                        icon::MAGNIFYING_GLASS,
                        FIND_W,
                        &count,
                        n == 0 && !find.is_empty(),
                        0.0,
                    );
                    // Asked until it holds: a new area's first frame only sizes
                    // itself, and a field there cannot take the keyboard.
                    if s.find_focus {
                        match resp.has_focus() {
                            true => s.find_focus = false,
                            false => resp.request_focus(),
                        }
                    }
                    // egui lets go of the field on `Escape` before it is drawn.
                    if escape && resp.lost_focus() {
                        close = true;
                    }
                    let live = n > 0;
                    ui.add_space(GAP);
                    if ui::icon_button(ui, icon::CARET_LEFT, BTN, 14.0, false, live)
                        .on_hover_text(crate::input::keys("Previous match (Shift+Enter)"))
                        .clicked()
                        && live
                    {
                        step = Some(-1);
                    }
                    ui.add_space(2.0);
                    if ui::icon_button(ui, icon::CARET_RIGHT, BTN, 14.0, false, live)
                        .on_hover_text("Next match (Enter)")
                        .clicked()
                        && live
                    {
                        step = Some(1);
                    }
                    ui.add_space(GAP);
                    if ui::icon_button(ui, icon::X, BTN, 13.0, false, true)
                        .on_hover_text("Close (Esc)")
                        .clicked()
                    {
                        close = true;
                    }
                });
            });
        });
    let now = s.find.clone().unwrap_or_default();
    let key = egui::util::hash((&s.buffer, &now));
    if key != s.find_key {
        s.find_key = key;
        s.find_hits = find_matches(&s.buffer, &now);
        if now != before {
            // A new text starts at the first match at or after the caret.
            let caret = caret_byte(&ctx, s.code_id(), &s.buffer).unwrap_or(0);
            s.find_current = s
                .find_hits
                .iter()
                .position(|r| r.start >= caret)
                .unwrap_or(0);
            s.find_reveal = !s.find_hits.is_empty();
        }
        s.find_current = s.find_current.min(s.find_hits.len().saturating_sub(1));
    }
    if let Some(by) = step
        && !s.find_hits.is_empty()
    {
        let n = s.find_hits.len() as isize;
        s.find_current = (s.find_current as isize + by).rem_euclid(n) as usize;
        s.find_reveal = true;
    }
    if close {
        if let Some(r) = s.find_hits.get(s.find_current).cloned() {
            s.found = Some(r);
        } else {
            s.focus_code = true;
        }
        close_find(s);
    }
}

/// The longest egui waits between the two presses of a double-click.
const DOUBLE_CLICK_S: f64 = 0.3;

/// The name at byte `b` of `text` (the character after `b`, else the one
/// before), or that one character when it is not part of a name.
fn word_at(text: &str, b: usize) -> std::ops::Range<usize> {
    let name = |c: char| c.is_alphanumeric() || c == '_';
    let after = text[b..].chars().next();
    let before = text[..b].chars().next_back();
    let at = match (after, before) {
        (Some(c), _) if name(c) => b,
        (_, Some(c)) if name(c) => b - c.len_utf8(),
        (Some(c), _) => return b..b + c.len_utf8(),
        _ => return b..b,
    };
    let start = text[..at]
        .char_indices()
        .rev()
        .take_while(|(_, c)| name(*c))
        .last()
        .map_or(at, |(i, _)| i);
    let end = text[at..]
        .char_indices()
        .find(|(_, c)| !name(*c))
        .map_or(text.len(), |(i, _)| at + i);
    start..end
}

/// A double-click held and dragged selects whole words: from the second
/// press, while the button stays down, the selection runs from the clicked
/// word to the word under the pointer, either way.
fn drag_words(ui: &egui::Ui, s: &mut Scripts, out: &egui::text_edit::TextEditOutput) {
    let (pressed, down, pos, now) = ui.input(|i| {
        (
            i.pointer.primary_pressed(),
            i.pointer.primary_down(),
            i.pointer.interact_pos(),
            i.time,
        )
    });
    let byte_at = |p: egui::Pos2| {
        let c = out.galley.cursor_from_pos(p - out.galley_pos).index.0;
        super::char_to_byte(&s.buffer, c)
    };
    if pressed
        && let Some(p) = pos
        && out.response.rect.contains(p)
    {
        let second = s
            .last_press
            .is_some_and(|(t, q)| now - t <= DOUBLE_CLICK_S && q.distance(p) < 6.0);
        s.last_press = Some((now, p));
        s.word_drag = second.then(|| word_at(&s.buffer, byte_at(p)));
    }
    if !down {
        s.word_drag = None;
        return;
    }
    let (Some(first), Some(p)) = (s.word_drag.clone(), pos) else {
        return;
    };
    // An undo or an outside write while the button is held leaves the word's
    // offsets from another text.
    if !s.buffer.is_char_boundary(first.start) || !s.buffer.is_char_boundary(first.end) {
        s.word_drag = None;
        return;
    }
    let here = word_at(&s.buffer, byte_at(p));
    let (anchor, caret) = match here.start < first.start {
        true => (first.end, here.start),
        false => (first.start, here.end.max(first.end)),
    };
    let chars = |b: usize| egui::text::CCursor::new(s.buffer[..b].chars().count());
    let mut state = out.state.clone();
    state
        .cursor
        .set_char_range(Some(egui::text::CCursorRange::two(
            chars(anchor),
            chars(caret),
        )));
    state.store(ui.ctx(), out.response.id);
}

/// Every place `needle` stands in `text`, ignoring ASCII case, not overlapping.
fn find_matches(text: &str, needle: &str) -> Vec<std::ops::Range<usize>> {
    if needle.is_empty() {
        return Vec::new();
    }
    let (hay, n) = (text.as_bytes(), needle.as_bytes());
    let mut out = Vec::new();
    let mut i = 0;
    while i + n.len() <= hay.len() {
        if hay[i..i + n.len()].eq_ignore_ascii_case(n)
            && text.is_char_boundary(i)
            && text.is_char_boundary(i + n.len())
        {
            out.push(i..i + n.len());
            i += n.len();
        } else {
            i += 1;
        }
    }
    out
}

/// Find's matches over the laid-out code, as the canvas Find marks them: each
/// in the accent wash, the current one ringed; the current one scrolled into
/// view when find moved to it. Only the matches on the rows in view are laid
/// out, walked in order with one running character count, so a long script's
/// thousands of matches cost the screen's few each frame.
fn paint_find_marks(ui: &mut egui::Ui, s: &mut Scripts, out: &egui::text_edit::TextEditOutput) {
    if s.find.is_none() {
        return;
    }
    let text = s.buffer.as_str();
    let fits = |r: &std::ops::Range<usize>| {
        r.end <= text.len() && text.is_char_boundary(r.start) && text.is_char_boundary(r.end)
    };
    if std::mem::take(&mut s.find_reveal)
        && let Some(r) = s.find_hits.get(s.find_current).filter(|r| fits(r))
    {
        let rect = super::script_help::span_rect(out, text, r);
        ui.scroll_to_rect(rect, Some(egui::Align::Center));
    }
    // The characters the rows in view hold, a line's slack each way.
    let clip = ui.clip_rect();
    let char_at = |x: f32, y: f32| {
        out.galley
            .cursor_from_pos(egui::vec2(x, y - out.galley_pos.y))
            .index
            .0
    };
    let first = char_at(0.0, clip.top() - ui::CODE_LINE_H);
    let last = char_at(f32::INFINITY, clip.bottom() + ui::CODE_LINE_H);
    let (from, to) = (
        super::char_to_byte(text, first),
        super::char_to_byte(text, last),
    );
    let lo = s.find_hits.partition_point(|r| r.end < from);
    let hi = s.find_hits.partition_point(|r| r.start <= to);
    let at = |c: usize| {
        out.galley
            .pos_from_cursor(egui::text::CCursor::new(c))
            .translate(out.galley_pos.to_vec2())
    };
    let (mut chars, mut byte) = (first, from);
    for (i, r) in s.find_hits.iter().enumerate().take(hi).skip(lo) {
        if r.start < byte || !fits(r) {
            continue;
        }
        chars += text[byte..r.start].chars().count();
        byte = r.start;
        let (a, b) = (at(chars), at(chars + text[r.clone()].chars().count()));
        // A match that wraps onto the next row is marked on its first.
        let rect = match (a.center().y - b.center().y).abs() < 1.0 {
            true => a.union(b),
            false => a,
        };
        let current = i == s.find_current;
        let stroke = match current {
            true => egui::Stroke::new(1.5, color::accent()),
            false => egui::Stroke::NONE,
        };
        ui.painter().rect(
            rect,
            2.0,
            color::accent().gamma_multiply_u8(77),
            stroke,
            egui::StrokeKind::Outside,
        );
    }
}

/// Find's area and its field.
const FIND_ID: &str = "script-find";
const FIND_FIELD: &str = "script-find-field";
/// The script menu's area.
const MENU_ID: &str = "script-menu";

/// Close find, its next search starting from the top again.
fn close_find(s: &mut Scripts) {
    s.find = None;
    s.find_hits.clear();
    s.find_current = 0;
    s.find_reveal = false;
    s.find_key = 0;
    s.find_focus = false;
}

/// One row of the script menu: what it does, its glyph and its label.
type MenuItem = (Act, &'static str, &'static str);

/// The script menu's rows for `which`, as groups a rule apart: the copies,
/// then the script's own file and name. A row that can never apply to where
/// the script lives is left out, as a context menu leaves it
/// (`docs/context-menus.md`): a kit's script is read-only and has no file.
fn menu_rows(which: &Which) -> Vec<Vec<MenuItem>> {
    let (library, document, kit) = (
        matches!(which, Which::Library(_)),
        matches!(which, Which::Document(_)),
        matches!(which, Which::Kit { .. }),
    );
    let copies = [
        (!library).then_some((Act::SaveToLibrary, icon::FOLDER_SIMPLE, "Save to library")),
        (!document).then_some((Act::CopyToDocument, icon::FILE, "Copy into this document")),
        Some((Act::Duplicate, icon::COPY_SIMPLE, "Duplicate")),
    ];
    let own = [
        (!kit).then_some((Act::StartRename, icon::CURSOR_TEXT, "Rename")),
        library.then_some((Act::ShowInFolder, icon::FOLDER_OPEN, "Show in folder")),
        (!kit).then_some((Act::Delete, icon::TRASH, "Delete")),
    ];
    [copies, own]
        .into_iter()
        .map(|g| g.into_iter().flatten().collect::<Vec<_>>())
        .filter(|g| !g.is_empty())
        .collect()
}

/// The script menu, while open, for the open script `which`: under the ⋯ or
/// at a row's right-click, drawn with the context menus' rows and keys
/// (`docs/context-menus.md` §8). A choice, `Enter` on the lit row, or a press
/// outside it and its ⋯ closes it. Answers the choice.
fn script_menu(ctx: &egui::Context, s: &mut Scripts, which: &Which) -> Option<Act> {
    use crate::menu::{Nav, navigate_index, pointer_moved};
    let menu = s.menu.as_mut()?;
    let groups = menu_rows(which);
    let rows: Vec<&MenuItem> = groups.iter().flatten().collect();
    let mut chosen = None;
    // The keys, as the tab menu's: an arrow moves the highlight, `Enter` runs
    // it, a pointer move hands it back to the pointer.
    match s.menu_nav.take() {
        Some(Nav::Move(step)) => {
            menu.highlight = navigate_index(rows.len(), menu.highlight, step);
        }
        Some(Nav::Activate) => {
            chosen = menu
                .highlight
                .and_then(|i| rows.get(i))
                .map(|r| r.0.clone())
        }
        Some(Nav::Open | Nav::Back) => {}
        None if pointer_moved(ctx) => menu.highlight = None,
        None => {}
    }
    let size = egui::vec2(
        crate::menu::MENU_W,
        ui::menu_height(rows.len(), groups.len() - 1),
    );
    let (pos, button) = match menu.at {
        MenuAt::Button(r) => (ui::dropdown_place(ctx, r, size, ui::MENU_GAP), Some(r)),
        MenuAt::Press(at) => (ui::menu_place(ctx, at, size), None),
    };
    let highlight = menu.highlight;
    let area = egui::Area::new(egui::Id::new(MENU_ID))
        .order(egui::Order::Foreground)
        .fixed_pos(pos)
        .show(ctx, |ui| {
            ui::menu_frame(ui::MENU_PAD).show(ui, |ui| {
                ui.set_width(ui::menu_inner_w(crate::menu::MENU_W, ui::MENU_PAD));
                ui.spacing_mut().item_spacing.y = 1.0;
                let mut i = 0;
                for (g, group) in groups.iter().enumerate() {
                    if g > 0 {
                        ui::menu_sep(ui);
                    }
                    for (a, glyph, label) in group {
                        let mut row = ui::MenuRow::new(glyph, label).danger(*a == Act::Delete);
                        if let Some(hl) = highlight {
                            row = row.highlight(i == hl);
                        }
                        if ui::menu_row(ui, row, ui::MENU_ROW_H).clicked() {
                            chosen = Some(a.clone());
                        }
                        i += 1;
                    }
                }
            });
        });
    let outside = ctx.input(|i| i.pointer.any_pressed())
        && !area.response.contains_pointer()
        && !button.is_some_and(|r| ctx.pointer_interact_pos().is_some_and(|p| r.contains(p)));
    if chosen.is_some() || outside {
        s.menu = None;
    }
    chosen
}

/// The output pane, `h` tall with its head: each run under a rule with its
/// time, errors in the red. At its head's height it shows the head alone.
fn output_pane(ui: &mut egui::Ui, runs: &[RunOut], h: f32) {
    let w = ui.available_width();
    let (head, _) = ui.allocate_exact_size(egui::vec2(w, OUTPUT_HEAD_H), egui::Sense::hover());
    let p = ui.painter();
    ui::paint_eyebrow(
        p,
        egui::pos2(head.left() + HEAD_INSET, head.center().y),
        egui::Align2::LEFT_CENTER,
        "Output",
    );
    if let Some(last) = runs.last() {
        p.text(
            egui::pos2(head.right() - HEAD_INSET, head.center().y),
            egui::Align2::RIGHT_CENTER,
            format!("Last run {} · {:.2} s", last.at, last.secs),
            egui::FontId::proportional(ui::CAPTION_PT),
            theme::text::faint(),
        );
    }
    let lines_h = h - OUTPUT_HEAD_H;
    if lines_h < 1.0 {
        return;
    }
    // The code's face and size, as the pane above it.
    let font = egui::FontId::monospace(ui::CODE_PT);
    let row_h = ui.fonts_mut(|f| f.row_height(&font));
    // Each run is its rule and its lines, one row each; only the rows in view
    // are laid out, so a run that printed thousands of lines costs no more.
    let total: usize = runs.iter().map(|r| 1 + r.lines.len()).sum();
    egui::ScrollArea::vertical()
        .id_salt("script-output")
        .max_height(lines_h)
        .min_scrolled_height(lines_h)
        .auto_shrink([false, false])
        .stick_to_bottom(true)
        .show_rows(ui, row_h, total, |ui, range| {
            ui.spacing_mut().item_spacing.y = 0.0;
            let mut first = 0;
            for run in runs {
                let rows = 1 + run.lines.len();
                for row in range.start.max(first)..range.end.min(first + rows) {
                    let (text, ink) = match row - first {
                        0 => (format!("— run at {} —", run.at), theme::text::faint()),
                        n => {
                            let (l, bad) = &run.lines[n - 1];
                            (l.clone(), if *bad { color::error() } else { color::text() })
                        }
                    };
                    ui.horizontal(|ui| {
                        ui.set_height(row_h);
                        ui.add_space(HEAD_INSET);
                        ui.label(egui::RichText::new(text).font(font.clone()).color(ink));
                    });
                }
                first += rows;
            }
            if range.end == total {
                ui.add_space(6.0);
            }
        });
}

/// The most of one printed line the output pane keeps, in characters.
const OUTPUT_ROW_CHARS: usize = 2_000;

/// `line` as an output row: cut to [`OUTPUT_ROW_CHARS`] with an ellipsis.
fn output_row(line: &str) -> String {
    match line.char_indices().nth(OUTPUT_ROW_CHARS) {
        Some((at, _)) => format!("{}…", &line[..at]),
        None => line.to_owned(),
    }
}

/// The code pane's completion list's width.
const SUGGEST_W: f32 = 380.0;

/// The code pane's completion list: the bar's rows on a dropdown's ground,
/// hung [`ui::MENU_GAP`] under the word being completed, or over it when below
/// has no room; not drawn while that word is scrolled out of `code_view`.
/// Answers the row clicked and the row the pointer moved onto.
#[expect(
    clippy::too_many_arguments,
    reason = "the list reads the pane's laid-out text, the window's id and the document; \
              bundling them would only move the list"
)]
fn suggest_list(
    ctx: &egui::Context,
    window: egui::Id,
    offer: &Offer,
    out: &egui::text_edit::TextEditOutput,
    text: &str,
    code_view: egui::Rect,
    lit: usize,
    doc: &mastik_core::Document,
) -> (Option<usize>, Option<usize>) {
    let c = &offer.completion;
    let start = text.get(..c.replace.start).map_or(0, |t| t.chars().count());
    let word = out
        .galley
        .pos_from_cursor(egui::text::CCursor::new(start))
        .translate(out.galley_pos.to_vec2());
    if !code_view.intersects(word) {
        return (None, None);
    }
    let frame = ui::menu_frame(crate::palette::PAD);
    let inner = egui::vec2(SUGGEST_W, super::completion_height(&c.rows));
    let at = ui::dropdown_place(ctx, word, inner + frame.total_margin().sum(), ui::MENU_GAP);
    egui::Area::new(window.with("suggest"))
        .order(egui::Order::Foreground)
        .constrain(false)
        .fixed_pos(at)
        .show(ctx, |ui| {
            frame
                .show(ui, |ui| {
                    ui.set_width(inner.x);
                    ui.spacing_mut().item_spacing = egui::vec2(0.0, crate::palette::ROW_GAP);
                    let layer_of = |i: usize| {
                        offer
                            .layers
                            .get(i)
                            .copied()
                            .flatten()
                            .and_then(|id| doc.get(id))
                    };
                    super::completion_rows(ui, &c.rows, lit, layer_of)
                })
                .inner
        })
        .inner
}

/// One input's field, by its type: a value field for a number, a swatch and
/// its hex for a colour, a segmented control for up to three choices and a
/// dropdown beyond, a text field for a string. A boolean's switch row is the
/// form's own, since it carries the label.
fn input_field(ui: &mut egui::Ui, w: f32, input: &mastik_vm::Input, value: &mut Value) {
    use mastik_vm::InputType as T;
    let size = egui::vec2(w, ui::CONTROL_H);
    match &input.ty {
        T::Number | T::Integer => {
            let mut v = value.as_f64().unwrap_or(0.0);
            let whole = matches!(input.ty, T::Integer);
            let scrub = match input.range {
                Some((lo, hi)) => ui::Scrub::whole(1.0).range(lo..=hi),
                None => ui::Scrub::whole(1.0),
            };
            let decimals = if whole { 0 } else { 2 };
            ui::value_field(ui, size, ui::Prefix::Text("#"), &mut v, scrub, |d| {
                d.custom_formatter(ui::number(decimals))
            });
            if let Some((lo, hi)) = input.range {
                v = v.clamp(lo, hi);
            }
            *value = match whole {
                true => Value::from(v.round() as i64),
                false => Value::from(v),
            };
        }
        // The form draws a boolean's switch row itself, label and all.
        T::Boolean => {}
        T::String => {
            let mut s = value.as_str().unwrap_or_default().to_owned();
            if ui::text_field(ui, size, &mut s, "").changed() {
                *value = Value::String(s);
            }
        }
        // Any colour the app's other colour fields read; the run takes it as
        // `#rrggbb[aa]` (`form_colours`).
        T::Color => {
            let mut s = value
                .as_str()
                .unwrap_or("#000000")
                .trim_start_matches('#')
                .to_owned();
            let ink = crate::colour_text::parse(&s).map_or(egui::Color32::TRANSPARENT, |t| {
                ui::color_to_egui(t.color(1.0))
            });
            if ui::colour_text_field(ui, size, &mut s, ink).changed() {
                *value = Value::String(s.trim().to_owned());
            }
        }
        T::Choice(words) => {
            let at = value
                .as_str()
                .and_then(|v| words.iter().position(|w| w == v))
                .unwrap_or(0);
            if words.len() <= 3 {
                if let Some(i) =
                    ui::segmented(ui, w, ui::SEGMENT_CELL_H, words.len(), at, |p, i, r, on| {
                        ui::segment_label(p, r, &title_case(&words[i]), on);
                    })
                {
                    *value = Value::String(words[i].clone());
                }
            } else {
                let mut picked = at;
                egui::ComboBox::from_id_salt(("script-input", &input.name))
                    .width(w)
                    .icon(ui::combo_chevron)
                    .selected_text(title_case(&words[at]))
                    .show_ui(ui, |ui| {
                        ui::menu_rows(ui);
                        for (i, word) in words.iter().enumerate() {
                            ui.selectable_value(&mut picked, i, title_case(word));
                        }
                    });
                *value = Value::String(words[picked].clone());
            }
        }
    }
}

/// `rows` as a choice reads in the form: `Rows`.
fn title_case(s: &str) -> String {
    let mut c = s.chars();
    match c.next() {
        Some(f) => f.to_uppercase().chain(c).collect(),
        None => String::new(),
    }
}

/// The form's colour values as a script reads them, `#rrggbb` (`#rrggbbaa`
/// when not opaque), whatever form each was typed in; the refusal for the
/// first that is not a colour.
fn form_colours(
    header: &mastik_vm::Header,
    values: &mut serde_json::Map<String, Value>,
) -> Result<(), String> {
    for input in &header.inputs {
        if input.ty != mastik_vm::InputType::Color {
            continue;
        }
        let Some(Value::String(text)) = values.get_mut(&input.name) else {
            continue;
        };
        let Some(t) = crate::colour_text::parse(text) else {
            return Err(crate::colour_text::refusal(text)
                .unwrap_or_else(|| format!("{} needs a colour", input.name)));
        };
        let byte = |v: f32| (v.clamp(0.0, 1.0) * 255.0).round() as u8;
        let [r, g, b] = t.rgb.map(byte);
        *text = match t.alpha.map(byte) {
            Some(a) if a < 255 => format!("#{r:02x}{g:02x}{b:02x}{a:02x}"),
            _ => format!("#{r:02x}{g:02x}{b:02x}"),
        };
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::palette::tests::add_rect;

    /// A headless app with one selected rectangle and a document script holding
    /// `source`; the document has `id` as its identity.
    fn with_script(source: &str, id: Option<&str>) -> (MastikApp, mastik_core::NodeId, ScriptId) {
        let ctx = egui::Context::default();
        crate::theme::install(&ctx);
        let mut app = MastikApp::headless(&ctx);
        let card = add_rect(&mut app);
        app.session.selection.set_one(card);
        let meta = mastik_core::DocumentMeta {
            id: id.map(str::to_owned),
            ..app.session.doc.meta().clone()
        };
        app.session.doc.set_meta(meta);
        // Committed directly, as a file would bring it: *Copy into this
        // document* would mark it as the person's own.
        let script = app.session.ids.mint_script();
        assert!(
            app.session.commit(Transaction(vec![Operation::AddScript {
                script: Script {
                    id: script,
                    name: "Nudge".into(),
                    source: source.to_owned(),
                },
                index: 0,
            }])),
            "fixture: the script was added"
        );
        (app, card, script)
    }

    /// [`with_script`] in a document with an identity, its script already shown
    /// on this machine, so a run goes straight to the form or the run.
    fn shown_script(source: &str) -> (MastikApp, mastik_core::NodeId, ScriptId) {
        let (mut app, card, script) = with_script(source, Some("doc-a"));
        let key = app.disclosure_key(&Which::Document(script)).unwrap();
        app.prefs.mark_disclosed(&key, source);
        (app, card, script)
    }

    fn left(app: &MastikApp, id: mastik_core::NodeId) -> f64 {
        app.session.resolved.world_bounds(id).expect("a box").x0
    }

    /// A run is one undo step named after the script with the script glyph, its
    /// report in the log and the status line.
    /// flip: `run_script_now` naming the step after nothing (`""`) → fails at
    /// "named after the script".
    #[test]
    fn a_run_is_one_step_named_after_the_script() {
        let (mut app, card, script) =
            shown_script("for _, n in ipairs(sel) do n.x += 1; n.x += 1 end");
        let (x, depth) = (left(&app, card), app.session.history.undo_depth());
        app.start_script_run(Which::Document(script));
        assert_eq!(left(&app, card), x + 2.0);
        assert_eq!(app.session.history.undo_depth(), depth + 1, "one step");
        let top = app.session.history.done().next_back().unwrap();
        assert_eq!(top.note.label, "Nudge", "named after the script");
        assert_eq!(top.note.glyph, icon::FILE_CODE);
        let last = app.session.command_log.entries().last().unwrap();
        assert_eq!(last.source, log::Source::Script("Nudge".into()));
        assert_eq!(app.session.status().text, "Nudge · 1 layer changed");
    }

    /// A script that came with a document with an identity shows its source
    /// before it runs, and not again once *Run* there has recorded it; a change
    /// to its text brings the box back.
    /// flip: `disclosure_key` answering `None` for a document script → fails at
    /// "the box comes first".
    #[test]
    fn a_documents_script_is_disclosed_before_its_first_run() {
        let (mut app, card, script) = with_script("sel.x += 5", Some("doc-a"));
        let x = left(&app, card);
        app.start_script_run(Which::Document(script));
        assert!(app.scripts.disclosure.is_some(), "the box comes first");
        assert_eq!(left(&app, card), x, "nothing ran yet");
        let d = app.scripts.disclosure.take().unwrap();
        let key = app.disclosure_key(&d.which).unwrap();
        app.prefs.mark_disclosed(&key, &d.source);
        app.start_script_run(Which::Document(script));
        assert!(app.scripts.disclosure.is_none(), "shown once is enough");
        assert_eq!(left(&app, card), x + 5.0);
        app.session
            .commit(Transaction(vec![Operation::SetScriptSource {
                id: script,
                source: "sel.x += 6".into(),
            }]));
        app.start_script_run(Which::Document(script));
        assert!(
            app.scripts.disclosure.is_some(),
            "a changed text is shown again"
        );
    }

    /// The disclosure is modal: a key meant for the canvas does not reach it.
    /// flip: `modal_is_up` leaving out `Scripts::modal_up` → fails at "the
    /// layer stays".
    #[test]
    fn the_disclosure_holds_the_keymap() {
        use crate::command::tests::{key, pass};
        let (mut app, card, script) = with_script("sel.x += 5", Some("doc-a"));
        app.start_script_run(Which::Document(script));
        assert!(app.scripts.disclosure.is_some(), "fixture: the box is up");
        let ctx = egui::Context::default();
        crate::theme::install(&ctx);
        let none = egui::Modifiers::NONE;
        pass(&ctx, &mut app, Vec::new(), none);
        pass(&ctx, &mut app, vec![key(egui::Key::Delete, none)], none);
        assert!(app.session.doc.get(card).is_some(), "the layer stays");
        assert!(app.scripts.disclosure.is_some(), "the box is still up");
    }

    /// Text the person typed into a document's script is theirs: its run needs
    /// no disclosure.
    /// flip: `store_buffer` not calling `authored` → fails at "typed here".
    #[test]
    fn a_script_typed_here_is_not_disclosed() {
        let (mut app, _, script) = with_script("sel.x += 5", Some("doc-a"));
        app.pick_script(Which::Document(script));
        app.scripts.buffer = "sel.x += 7".into();
        app.store_buffer();
        app.start_script_run(Which::Document(script));
        assert!(app.scripts.disclosure.is_none(), "typed here");
    }

    /// A document with no identity has no key to remember a disclosure under, so
    /// its script is shown before every run rather than never.
    /// flip: `start_script_run` treating a missing key as shown → fails at
    /// "shown, with no key".
    #[test]
    fn a_script_with_no_key_is_shown_every_run() {
        let (mut app, card, script) = with_script("sel.x += 5", None);
        assert_eq!(
            app.disclosure_key(&Which::Document(script)),
            None,
            "fixture"
        );
        let x = left(&app, card);
        app.start_script_run(Which::Document(script));
        assert!(app.scripts.disclosure.is_some(), "shown, with no key");
        assert_eq!(left(&app, card), x);
    }

    /// A script with a header opens its form first, and the form's values reach
    /// the run as `param`.
    /// flip: `script_form_or_run` running a script that has inputs → fails at
    /// "the form comes first".
    #[test]
    fn a_header_asks_for_its_inputs_first() {
        let source = "---@input by number = 3 -- By\nsel.x += param.by";
        let (mut app, card, script) = shown_script(source);
        let x = left(&app, card);
        app.start_script_run(Which::Document(script));
        let mut form = app.scripts.form.take().expect("the form comes first");
        assert_eq!(left(&app, card), x);
        form.values.insert("by".into(), Value::from(7.0));
        app.run_script_now(form.which, form.name, form.source, form.values);
        assert_eq!(left(&app, card), x + 7.0);
    }

    /// A colour input reads whatever the app's colour fields read and reaches
    /// the script as `#rrggbb[aa]`; text that is no colour is refused.
    /// flip: `form_colours` writing the alpha off → fails at "with its alpha".
    #[test]
    fn a_colour_input_reads_as_every_colour_field_does() {
        let header = mastik_vm::header("---@input a color\n---@input b color\n---@input c color\n")
            .expect("fixture: the header parses");
        let mut values = serde_json::Map::new();
        values.insert("a".into(), Value::from("ff000080"));
        values.insert("b".into(), Value::from("red"));
        values.insert("c".into(), Value::from("f00"));
        assert_eq!(form_colours(&header, &mut values), Ok(()));
        assert_eq!(values["a"], "#ff000080", "with its alpha");
        assert_eq!(values["b"], "#ff0000", "a named colour");
        assert_eq!(values["c"], "#ff0000", "short hex");
        values.insert("c".into(), Value::from("#red"));
        assert_eq!(
            form_colours(&header, &mut values),
            Err("Not a colour: #red".into())
        );
    }

    /// The values an unsaved script's form last ran with stay its own when an
    /// unsaved script listed before it is deleted.
    /// flip: `script_named` finding an unsaved script by its place in the list →
    /// fails at "B's form is up".
    #[test]
    fn an_unsaved_scripts_values_outlive_a_deletion_before_it() {
        let (mut app, _, _) = with_script("", None);
        let source = "---@input by number = 3 -- By\nsel.x += param.by";
        app.add_unsaved(source.into());
        let a = app.scripts.current.clone().unwrap();
        app.add_unsaved(source.into());
        let b = app.scripts.current.clone().unwrap();
        let mut kept = serde_json::Map::new();
        kept.insert("by".into(), Value::from(9.0));
        app.scripts.last_values.insert(values_key(&b), kept);
        app.pick_script(a);
        app.scripts_act(Some(Act::Delete));
        app.start_script_run(b);
        let form = app.scripts.form.take().expect("B's form is up");
        assert_eq!(form.values.get("by"), Some(&Value::from(9.0)), "still B's");
    }

    /// The open editor keeps its size while the pointer moves over it, with a
    /// script open and with none.
    /// flip: the body allocated a point past its room → fails at "the same
    /// height".
    #[test]
    fn the_editor_keeps_its_size_as_the_pointer_moves() {
        use crate::command::tests::pass;
        let ctx = egui::Context::default();
        crate::theme::install(&ctx);
        let (mut app, _, script) = with_script("sel.x += 1\nsel.y += 1", None);
        let none = egui::Modifiers::NONE;
        let height = |ctx: &egui::Context| {
            ctx.memory(|m| m.area_rect(egui::Id::new("script-editor")))
                .expect("the editor is drawn")
                .height()
        };
        for picked in [None, Some(Which::Document(script))] {
            app.open_scripts();
            if let Some(w) = picked {
                app.pick_script(w);
            }
            for _ in 0..3 {
                pass(&ctx, &mut app, Vec::new(), none);
            }
            let first = height(&ctx);
            for i in 0..30 {
                let at = egui::pos2(400.0 + i as f32 * 7.0, 300.0 + i as f32 * 3.0);
                pass(&ctx, &mut app, vec![egui::Event::PointerMoved(at)], none);
            }
            assert_eq!(height(&ctx), first, "the same height");
        }
    }

    /// The editor open on a document script holding `source`, pumped until laid out.
    fn open_on(source: &str) -> (egui::Context, MastikApp) {
        use crate::command::tests::pass;
        let ctx = egui::Context::default();
        crate::theme::install(&ctx);
        let (mut app, _, script) = with_script(source, None);
        app.open_scripts();
        app.pick_script(Which::Document(script));
        for _ in 0..3 {
            pass(&ctx, &mut app, Vec::new(), egui::Modifiers::NONE);
        }
        (ctx, app)
    }

    /// A primary press and release at `at`, the pointer moved there first.
    fn click_at(ctx: &egui::Context, app: &mut MastikApp, at: egui::Pos2) {
        use crate::command::tests::pass;
        let none = egui::Modifiers::NONE;
        let button = |pressed| egui::Event::PointerButton {
            pos: at,
            button: egui::PointerButton::Primary,
            pressed,
            modifiers: none,
        };
        pass(ctx, app, vec![egui::Event::PointerMoved(at)], none);
        pass(ctx, app, vec![button(true)], none);
        pass(ctx, app, vec![button(false)], none);
        pass(ctx, app, Vec::new(), none);
    }

    /// The code pane's caret, in chars.
    fn caret(ctx: &egui::Context, app: &MastikApp) -> Option<usize> {
        egui::TextEdit::load_state(ctx, app.scripts.code_id())
            .and_then(|s| s.cursor.char_range())
            .map(|r| r.primary.index.0)
    }

    /// A press in the code pane's air under the text puts the caret at the
    /// text's end, and one beside a line number at that line's start.
    /// flip: `scripts_ui` not acting on `blank` → fails at "the end".
    #[test]
    fn a_press_beside_the_code_places_the_caret() {
        let source = "sel.x += 1\nsel.y += 2";
        let (ctx, mut app) = open_on(source);
        let code = ctx
            .read_response(app.scripts.code_id())
            .expect("fixture: the code is drawn")
            .rect;
        click_at(
            &ctx,
            &mut app,
            egui::pos2(code.center().x, code.bottom() + 40.0),
        );
        assert_eq!(caret(&ctx, &app), Some(source.chars().count()), "the end");
        let second = egui::pos2(code.left() - 8.0, code.top() + ui::CODE_LINE_H * 1.5);
        click_at(&ctx, &mut app, second);
        assert_eq!(caret(&ctx, &app), Some(11), "the second line's start");
    }

    /// The output pane's top edge drags: down to the head alone, and up only
    /// as far as leaves the code its least.
    /// flip: the drag's clamp losing its floor (`OUTPUT_HEAD_H` → 0) → fails at
    /// "the head alone".
    #[test]
    fn the_output_drags_down_to_its_head() {
        use crate::command::tests::pass;
        let (ctx, mut app) = open_on("print(1)");
        let none = egui::Modifiers::NONE;
        let edge = || {
            ctx.read_response(egui::Id::new("script-editor").with("output-edge"))
                .expect("fixture: the edge is drawn")
                .rect
                .center()
        };
        let drag = |app: &mut MastikApp, from: egui::Pos2, dy: f32| {
            let button = |pos, pressed| egui::Event::PointerButton {
                pos,
                button: egui::PointerButton::Primary,
                pressed,
                modifiers: none,
            };
            pass(&ctx, app, vec![egui::Event::PointerMoved(from)], none);
            pass(&ctx, app, vec![button(from, true)], none);
            let mut at = from;
            for _ in 0..10 {
                at.y += dy / 10.0;
                pass(&ctx, app, vec![egui::Event::PointerMoved(at)], none);
            }
            pass(&ctx, app, vec![button(at, false)], none);
            pass(&ctx, app, Vec::new(), none);
        };
        let start = edge();
        drag(&mut app, start, 600.0);
        assert_eq!(app.scripts.output_h, Some(OUTPUT_HEAD_H), "the head alone");
        drag(&mut app, edge(), -900.0);
        let code = ctx
            .read_response(app.scripts.code_id())
            .expect("the code is drawn")
            .rect;
        let lowest = edge().y;
        assert!(lowest < start.y, "it grew: {lowest} vs {start:?}");
        assert!(
            lowest - code.top() >= CODE_MIN_H - 12.0,
            "the code keeps its least: {}",
            lowest - code.top()
        );
    }

    /// The editor on `source` with the caret at its end, the code focused by a
    /// press under the text.
    fn typing_at_end(source: &str) -> (egui::Context, MastikApp) {
        let (ctx, mut app) = open_on(source);
        let code = ctx
            .read_response(app.scripts.code_id())
            .expect("fixture: the code is drawn")
            .rect;
        click_at(
            &ctx,
            &mut app,
            egui::pos2(code.center().x, code.bottom() + 40.0),
        );
        assert_eq!(
            caret(&ctx, &app),
            Some(source.chars().count()),
            "fixture: the caret is at the end"
        );
        (ctx, app)
    }

    /// `text` typed into the focused code, and the frame after that reads it.
    fn type_code(ctx: &egui::Context, app: &mut MastikApp, text: &str) {
        use crate::command::tests::pass;
        pass(
            ctx,
            app,
            vec![egui::Event::Text(text.into())],
            egui::Modifiers::NONE,
        );
        pass(ctx, app, Vec::new(), egui::Modifiers::NONE);
    }

    fn press(ctx: &egui::Context, app: &mut MastikApp, key: egui::Key) {
        use crate::command::tests::{key as key_event, pass};
        let none = egui::Modifiers::NONE;
        pass(ctx, app, vec![key_event(key, none)], none);
        pass(ctx, app, Vec::new(), none);
    }

    /// Typing a name in the code opens the list under it with the script's own
    /// names, and `Tab` takes the lit row with the caret after it.
    /// flip: `script_suggest` not taking `Tab` → fails at "Tab took the row".
    #[test]
    fn typing_in_the_code_opens_completion_and_tab_takes_it() {
        let source = "local gap_x = 1\n";
        let (ctx, mut app) = typing_at_end(source);
        type_code(&ctx, &mut app, "gap");
        assert!(app.scripts.suggest.on(), "a name opens the list");
        let list = ctx
            .memory(|m| m.area_rect(egui::Id::new("script-editor").with("suggest")))
            .expect("the list is drawn");
        let code = ctx.read_response(app.scripts.code_id()).unwrap().rect;
        assert!(
            list.top() > code.top() + ui::CODE_LINE_H,
            "under the second line: {list:?}"
        );
        press(&ctx, &mut app, egui::Key::Tab);
        assert_eq!(
            app.scripts.buffer, "local gap_x = 1\ngap_x",
            "Tab took the row"
        );
        assert_eq!(caret(&ctx, &app), Some(21), "the caret after it");
        assert!(!app.scripts.suggest.on(), "and the list shut");
    }

    /// `Enter` on a word that already reads as the lit row breaks the line, and
    /// `Escape` shuts the list for the word, leaving the text.
    /// flip: `script_suggest` taking `Enter` when the row would change nothing →
    /// fails at "Enter broke the line".
    #[test]
    fn enter_on_a_whole_word_breaks_the_line_and_escape_shuts_the_list() {
        let (ctx, mut app) = typing_at_end("local gap = 1\n");
        type_code(&ctx, &mut app, "gap");
        assert!(app.scripts.suggest.on(), "fixture: the list is up");
        press(&ctx, &mut app, egui::Key::Enter);
        assert_eq!(
            app.scripts.buffer, "local gap = 1\ngap\n",
            "Enter broke the line"
        );
        type_code(&ctx, &mut app, "pri");
        assert!(app.scripts.suggest.on(), "fixture: the list is up again");
        press(&ctx, &mut app, egui::Key::Escape);
        assert!(!app.scripts.suggest.on(), "Escape shut it");
        assert_eq!(
            app.scripts.buffer, "local gap = 1\ngap\npri",
            "the text stays"
        );
        assert!(app.scripts.open, "the editor stays open");
    }

    /// `Escape` that shuts the list opened by `Ctrl+Space` leaves the code the
    /// keyboard, as it does the hint.
    /// flip: `scripts_ui` not handing the focus back → fails at "the code keeps
    /// the keyboard".
    #[test]
    fn escape_on_the_list_keeps_the_codes_keyboard() {
        use crate::command::tests::{key, pass};
        let (ctx, mut app) = typing_at_end("local gap = 1\n");
        let ctrl = egui::Modifiers::CTRL;
        pass(&ctx, &mut app, vec![key(egui::Key::Space, ctrl)], ctrl);
        pass(&ctx, &mut app, Vec::new(), egui::Modifiers::NONE);
        assert!(app.scripts.suggest.on(), "fixture: Ctrl+Space opened it");
        press(&ctx, &mut app, egui::Key::Escape);
        assert!(!app.scripts.suggest.on(), "Escape shut it");
        let code = app.scripts.code_id();
        assert!(
            ctx.memory(|m| m.has_focus(code)),
            "the code keeps the keyboard"
        );
    }

    /// `Ctrl+X` with nothing selected cuts the caret's line.
    /// flip: `script_keys` leaving `Event::Cut` to the text edit → fails at
    /// "the line is gone".
    #[test]
    fn cut_with_no_selection_takes_the_line() {
        use crate::command::tests::pass;
        let (ctx, mut app) = typing_at_end("local a = 1\nprint(a)");
        pass(
            &ctx,
            &mut app,
            vec![egui::Event::Cut],
            egui::Modifiers::CTRL,
        );
        pass(&ctx, &mut app, Vec::new(), egui::Modifiers::NONE);
        assert_eq!(app.scripts.buffer, "local a = 1", "the line is gone");
    }

    /// A double-click held and dragged selects whole words.
    /// flip: `drag_words` never taking the second press → fails at "whole words".
    #[test]
    fn a_double_click_dragged_selects_words() {
        use crate::command::tests::pass;
        let (ctx, mut app) = open_on("alpha beta gamma");
        let code = ctx
            .read_response(app.scripts.code_id())
            .expect("fixture: the code is drawn")
            .rect;
        let w = ctx.fonts_mut(|f| {
            f.layout_no_wrap(
                "m".into(),
                egui::FontId::monospace(ui::CODE_PT),
                color::text(),
            )
            .size()
            .x
        });
        // The edit's 10-point inset, then the middle of a character.
        let at = |c: f32| egui::pos2(code.left() + 10.0 + w * (c + 0.5), code.top() + 10.0);
        let none = egui::Modifiers::NONE;
        let button = |p, pressed| egui::Event::PointerButton {
            pos: p,
            button: egui::PointerButton::Primary,
            pressed,
            modifiers: none,
        };
        let (a, b) = (at(7.0), at(13.0));
        pass(&ctx, &mut app, vec![egui::Event::PointerMoved(a)], none);
        pass(&ctx, &mut app, vec![button(a, true)], none);
        pass(&ctx, &mut app, vec![button(a, false)], none);
        pass(&ctx, &mut app, vec![button(a, true)], none);
        pass(&ctx, &mut app, vec![egui::Event::PointerMoved(b)], none);
        pass(&ctx, &mut app, vec![egui::Event::PointerMoved(b)], none);
        let range = egui::TextEdit::load_state(&ctx, app.scripts.code_id())
            .and_then(|s| s.cursor.char_range())
            .map(|r| {
                let (x, y) = (r.primary.index.0, r.secondary.index.0);
                x.min(y)..x.max(y)
            });
        assert_eq!(range, Some(6..16), "whole words: beta to gamma");
        pass(&ctx, &mut app, vec![button(b, false)], none);
    }

    /// A word drag whose text changed under the held button (an undo, an
    /// outside write) lets go of the word instead of slicing inside a character.
    /// flip: `drag_words` testing only the word's end against the length →
    /// panics at the move.
    #[test]
    fn a_word_drag_over_a_changed_text_lets_go() {
        use crate::command::tests::pass;
        let ctx = egui::Context::default();
        crate::theme::install(&ctx);
        let mut app = MastikApp::headless(&ctx);
        app.open_script_editor_with("aé cd".into());
        let none = egui::Modifiers::NONE;
        for _ in 0..3 {
            pass(&ctx, &mut app, Vec::new(), none);
        }
        let code = ctx
            .read_response(app.scripts.code_id())
            .expect("fixture: the code is drawn")
            .rect;
        let at = egui::pos2(code.left() + 14.0, code.top() + 10.0);
        pass(&ctx, &mut app, vec![egui::Event::PointerMoved(at)], none);
        let press = egui::Event::PointerButton {
            pos: at,
            button: egui::PointerButton::Primary,
            pressed: true,
            modifiers: none,
        };
        pass(&ctx, &mut app, vec![press], none);
        app.scripts.buffer = "aéééé".into();
        app.scripts.word_drag = Some(4..6);
        let moved = at + egui::vec2(20.0, 0.0);
        pass(&ctx, &mut app, vec![egui::Event::PointerMoved(moved)], none);
        assert_eq!(app.scripts.word_drag, None, "the word is let go");
    }

    /// The code pane takes its editing keys before the text edit: `Enter`
    /// after `then` indents, `end` typed a letter a frame steps back, `(` types
    /// its close, and `Ctrl+/` comments the line.
    /// flip: `scripts_ui` not calling `script_keys` → fails at "Enter indents".
    #[test]
    fn the_code_pane_takes_its_editing_keys() {
        use crate::command::tests::{key, pass};
        let (ctx, mut app) = typing_at_end("if a then");
        press(&ctx, &mut app, egui::Key::Enter);
        assert_eq!(app.scripts.buffer, "if a then\n  ", "Enter indents");
        type_code(&ctx, &mut app, "x = f");
        type_code(&ctx, &mut app, "(");
        assert_eq!(
            app.scripts.buffer, "if a then\n  x = f()",
            "( types its close"
        );
        assert_eq!(caret(&ctx, &app), Some(18), "between the two");
        type_code(&ctx, &mut app, ")");
        press(&ctx, &mut app, egui::Key::Enter);
        for letter in ["e", "n", "d"] {
            type_code(&ctx, &mut app, letter);
        }
        assert_eq!(
            app.scripts.buffer, "if a then\n  x = f()\nend",
            "end steps back to its block"
        );
        let ctrl = egui::Modifiers::COMMAND;
        pass(&ctx, &mut app, vec![key(egui::Key::Slash, ctrl)], ctrl);
        pass(&ctx, &mut app, Vec::new(), egui::Modifiers::NONE);
        assert_eq!(
            app.scripts.buffer, "if a then\n  x = f()\n-- end",
            "Ctrl+/ comments the line"
        );
    }

    /// `Ctrl+/` with the caret at a comment line's start, under a line ending in
    /// a multibyte character, uncomments it and leaves the caret there.
    /// flip: `toggle_comment` shifting the caret by the whole mark → fails at
    /// "the line's start".
    #[test]
    fn uncommenting_under_a_multibyte_line_keeps_the_caret() {
        use crate::command::tests::{key, pass};
        let (ctx, mut app) = typing_at_end("x→\n-- y");
        let id = app.scripts.code_id();
        put_selection(&ctx, id, &app.scripts.buffer, code_keys::Sel::caret(5));
        let ctrl = egui::Modifiers::COMMAND;
        pass(&ctx, &mut app, vec![key(egui::Key::Slash, ctrl)], ctrl);
        pass(&ctx, &mut app, Vec::new(), egui::Modifiers::NONE);
        assert_eq!(app.scripts.buffer, "x→\ny", "uncommented");
        assert_eq!(caret(&ctx, &app), Some(3), "the line's start");
    }

    /// A selection put past the text's end or inside a character goes back to
    /// the nearest boundary.
    /// flip: `put_selection` slicing at the raw offset → panics at "inside →".
    #[test]
    fn a_selection_inside_a_character_is_put_at_its_start() {
        let ctx = egui::Context::default();
        let id = egui::Id::new("put-selection-probe");
        let chars = |sel| {
            put_selection(&ctx, id, "x→y", sel);
            egui::TextEdit::load_state(&ctx, id)
                .and_then(|s| s.cursor.char_range())
                .map(|r| r.primary.index.0)
        };
        assert_eq!(chars(code_keys::Sel::caret(2)), Some(1), "inside →");
        assert_eq!(chars(code_keys::Sel::caret(99)), Some(3), "past the end");
    }

    /// A run that fails changes nothing, says where, and puts the editor on the line.
    #[test]
    fn a_failed_run_changes_nothing_and_opens_the_line() {
        let (mut app, card, script) = shown_script("sel.x += 4\n\nsel.radius = 3");
        let (x, depth) = (left(&app, card), app.session.history.undo_depth());
        app.start_script_run(Which::Document(script));
        assert_eq!(left(&app, card), x);
        assert_eq!(app.session.history.undo_depth(), depth);
        assert!(app.scripts.open);
        assert_eq!(app.scripts.jump, Some(2), "the third line");
        assert_eq!(
            app.session.status().text,
            "Nudge failed at line 3 — nothing changed"
        );
    }

    /// One frame of the app with `events`, answering what it painted.
    fn painted(
        ctx: &egui::Context,
        app: &mut MastikApp,
        events: Vec<egui::Event>,
    ) -> Vec<egui::epaint::ClippedShape> {
        let mut frame = eframe::Frame::_new_kittest();
        let input = egui::RawInput {
            screen_rect: Some(egui::Rect::from_min_size(
                egui::Pos2::ZERO,
                egui::vec2(1320.0, 820.0),
            )),
            events,
            ..Default::default()
        };
        ctx.run_ui(input, |ui| eframe::App::ui(app, ui, &mut frame))
            .shapes
    }

    /// Every shape in `shapes`, nested ones too.
    fn flat(shapes: &[egui::epaint::ClippedShape]) -> Vec<egui::Shape> {
        fn walk(s: &egui::Shape, out: &mut Vec<egui::Shape>) {
            match s {
                egui::Shape::Vec(v) => v.iter().for_each(|s| walk(s, out)),
                s => out.push(s.clone()),
            }
        }
        let mut out = Vec::new();
        shapes.iter().for_each(|c| walk(&c.shape, &mut out));
        out
    }

    /// Where the editor's head paints `glyph`, its right-most inside the window.
    fn head_glyph(ctx: &egui::Context, app: &mut MastikApp, glyph: &str) -> egui::Pos2 {
        let window = ctx
            .memory(|m| m.area_rect(egui::Id::new("script-editor")))
            .expect("fixture: the editor is drawn");
        flat(&painted(ctx, app, Vec::new()))
            .iter()
            .filter_map(|s| match s {
                egui::Shape::Text(t) if t.galley.text() == glyph => {
                    Some(egui::Rect::from_min_size(t.pos, t.galley.size()).center())
                }
                _ => None,
            })
            .filter(|p| window.contains(*p))
            .max_by(|a, b| a.x.total_cmp(&b.x))
            .unwrap_or_else(|| panic!("fixture: {glyph:?} is in the head"))
    }

    /// Whether a button's ground under `at` wears the accent wash of
    /// `FieldButton::On`, at rest or hovered.
    fn lit_at(ctx: &egui::Context, app: &mut MastikApp, at: egui::Pos2) -> bool {
        // The card's translucency premultiplies the fill, so it is compared
        // with its alpha taken out.
        let opaque = |c: egui::Color32| {
            let a = c.a().max(1) as f32 / 255.0;
            [c.r(), c.g(), c.b()].map(|v| v as f32 / a)
        };
        let near = |a: egui::Color32, b: egui::Color32| {
            let (a, b) = (opaque(a), opaque(b));
            (0..3).all(|i| (a[i] - b[i]).abs() < 3.0)
        };
        let on = [color::accent_900(), color::accent_800()];
        flat(&painted(ctx, app, Vec::new())).iter().any(|s| {
            matches!(s, egui::Shape::Rect(r) if r.rect.contains(at)
                && r.rect.width() <= ui::CONTROL_H + 1.0
                && r.fill.a() > 0
                && on.iter().any(|c| near(*c, r.fill)))
        })
    }

    /// The head's loupe opens find with the keyboard in its field and wears the
    /// accent while find is open; a second click closes it.
    /// flip: `script_head`'s loupe giving `FieldButton::Off` → fails at "the
    /// loupe wears the accent"; its click only ever opening → fails at "a
    /// second click closes it".
    #[test]
    fn the_loupe_toggles_find_and_wears_the_accent_while_open() {
        let (ctx, mut app) = open_on("print(1)");
        let loupe = head_glyph(&ctx, &mut app, icon::MAGNIFYING_GLASS);
        assert!(!lit_at(&ctx, &mut app, loupe), "fixture: off at rest");
        click_at(&ctx, &mut app, loupe);
        assert!(app.scripts.find.is_some(), "a click opens find");
        assert!(
            ctx.memory(|m| m.has_focus(egui::Id::new(FIND_FIELD))),
            "with the keyboard in its field"
        );
        assert!(lit_at(&ctx, &mut app, loupe), "the loupe wears the accent");
        click_at(&ctx, &mut app, loupe);
        assert!(app.scripts.find.is_none(), "a second click closes it");
        assert!(!lit_at(&ctx, &mut app, loupe), "and the accent goes");
    }

    /// A long name is cut so the row's note keeps its place, and the rename
    /// field hides the note with the name.
    /// flip: `list_row` drawing with `palette::row_texts` (the note cut first)
    /// → fails at "the note stays".
    #[test]
    fn a_long_name_keeps_the_rows_note_and_rename_hides_it() {
        let long = "Renaming it to something that does not fit the list";
        let (ctx, mut app) = open_on("---@input gap number = 4\nprint(param.gap)");
        let id = app.session.doc.scripts()[0].id;
        app.scripts_act(Some(Act::Rename(long.into())));
        assert_eq!(
            app.session.doc.script(id).unwrap().name,
            long,
            "fixture: renamed"
        );
        let texts = |ctx: &egui::Context, app: &mut MastikApp| -> Vec<(String, bool)> {
            flat(&painted(ctx, app, Vec::new()))
                .iter()
                .filter_map(|s| match s {
                    egui::Shape::Text(t) => Some((t.galley.text().to_owned(), t.galley.elided)),
                    _ => None,
                })
                .collect()
        };
        let shown = texts(&ctx, &mut app);
        assert!(shown.iter().any(|(t, _)| t == "1 input"), "the note stays");
        assert!(
            shown
                .iter()
                .any(|(t, cut)| *cut && long.starts_with(t.trim_end_matches('…'))),
            "the name is cut: {shown:?}"
        );
        app.scripts_act(Some(Act::StartRename));
        let shown = texts(&ctx, &mut app);
        assert!(
            !shown.iter().any(|(t, _)| t == "1 input"),
            "the field hides the note"
        );
    }

    /// Find counts every match, ignoring case; `Enter` and `Shift+Enter` step
    /// with the keyboard kept in the field; `Escape` closes it and selects the
    /// current match in the code.
    /// flip: `find_row` not reading `Enter` while the field has the keyboard →
    /// fails at "Enter steps on".
    #[test]
    fn find_steps_through_its_matches_and_keeps_the_keyboard() {
        use crate::command::tests::{key, pass};
        let (ctx, mut app) = open_on("sel.x = 1\nSel.y = sel.x");
        let loupe = head_glyph(&ctx, &mut app, icon::MAGNIFYING_GLASS);
        click_at(&ctx, &mut app, loupe);
        let none = egui::Modifiers::NONE;
        pass(&ctx, &mut app, vec![egui::Event::Text("sel".into())], none);
        pass(&ctx, &mut app, Vec::new(), none);
        assert_eq!(
            app.scripts.find_hits,
            [0..3, 10..13, 18..21],
            "three, any case"
        );
        assert_eq!(app.scripts.find_current, 0, "fixture: the first");
        press(&ctx, &mut app, egui::Key::Enter);
        assert_eq!(app.scripts.find_current, 1, "Enter steps on");
        assert!(
            ctx.memory(|m| m.has_focus(egui::Id::new(FIND_FIELD))),
            "the field keeps the keyboard"
        );
        let shift = egui::Modifiers::SHIFT;
        pass(&ctx, &mut app, vec![key(egui::Key::Enter, shift)], shift);
        pass(&ctx, &mut app, Vec::new(), none);
        assert_eq!(app.scripts.find_current, 0, "Shift+Enter steps back");
        pass(&ctx, &mut app, vec![key(egui::Key::Enter, shift)], shift);
        assert_eq!(app.scripts.find_current, 2, "and wraps");
        pass(&ctx, &mut app, Vec::new(), none);
        assert!(
            ctx.memory(|m| m.has_focus(egui::Id::new(FIND_FIELD))),
            "the field still has the keyboard"
        );
        press(&ctx, &mut app, egui::Key::Escape);
        assert!(app.scripts.find.is_none(), "Escape closes find");
        pass(&ctx, &mut app, Vec::new(), none);
        let sel = egui::TextEdit::load_state(&ctx, app.scripts.code_id())
            .and_then(|s| s.cursor.char_range())
            .map(|r| {
                let (a, b) = (r.primary.index.0, r.secondary.index.0);
                a.min(b)..a.max(b)
            });
        assert_eq!(sel, Some(18..21), "the current match selected in the code");
    }

    /// Find over a long script marks only the matches on the rows in view, each
    /// where its text is, and still scrolls to a current match out of view.
    /// flip: `paint_find_marks` walking every match, not the range in view →
    /// fails at "only the rows in view".
    #[test]
    fn find_marks_only_the_matches_in_view() {
        use crate::command::tests::pass;
        let source = "sel.x = 1\n".repeat(1500);
        let (ctx, mut app) = open_on(&source);
        let loupe = head_glyph(&ctx, &mut app, icon::MAGNIFYING_GLASS);
        click_at(&ctx, &mut app, loupe);
        let none = egui::Modifiers::NONE;
        pass(&ctx, &mut app, vec![egui::Event::Text("sel".into())], none);
        pass(&ctx, &mut app, Vec::new(), none);
        assert_eq!(app.scripts.find_hits.len(), 1500, "fixture: every line");
        // The wash, give or take the window's fade a step.
        let wash = color::accent().gamma_multiply_u8(77);
        let is_wash = |c: egui::Color32| {
            c.to_array()
                .iter()
                .zip(wash.to_array())
                .all(|(a, b)| a.abs_diff(b) <= 2)
        };
        let marks = |shapes: &[egui::epaint::ClippedShape]| -> Vec<egui::Rect> {
            flat(shapes)
                .iter()
                .filter_map(|s| match s {
                    egui::Shape::Rect(r) if is_wash(r.fill) => Some(r.rect),
                    _ => None,
                })
                .collect()
        };
        let shown = marks(&painted(&ctx, &mut app, Vec::new()));
        assert!(
            !shown.is_empty() && shown.len() < 100,
            "only the rows in view: {}",
            shown.len()
        );
        let code = ctx.read_response(app.scripts.code_id()).unwrap().rect;
        let first = shown.iter().map(|r| r.top()).fold(f32::INFINITY, f32::min);
        assert!(
            (first - (code.top() + (ui::CODE_LINE_H - ui::CODE_ROW_H) / 2.0)).abs() < 2.0,
            "the first on the first line: {first} vs {code:?}"
        );
        app.scripts.find_current = 1200;
        app.scripts.find_reveal = true;
        // The scroll animates.
        for _ in 0..60 {
            pass(&ctx, &mut app, Vec::new(), none);
        }
        let shown = marks(&painted(&ctx, &mut app, Vec::new()));
        assert!(shown.len() < 100, "still only the rows in view");
        let ringed = flat(&painted(&ctx, &mut app, Vec::new()))
            .iter()
            .any(|s| matches!(s, egui::Shape::Rect(r) if is_wash(r.fill) && r.stroke.width > 0.0));
        assert!(ringed, "the current one, scrolled to, is ringed in view");
    }

    /// The ⋯ menu wears the accent while open, and `Escape` closes it without
    /// reaching the canvas, whose selection stays.
    /// flip: the key routing without its `holds_keys` branch → fails at
    /// "Escape closed the menu".
    #[test]
    fn escape_closes_the_menu_and_not_the_selection() {
        let (ctx, mut app) = open_on("print(1)");
        let selected = app.session.selection.ids().to_vec();
        assert_eq!(selected.len(), 1, "fixture: a layer is selected");
        let dots = head_glyph(&ctx, &mut app, icon::DOTS_THREE);
        click_at(&ctx, &mut app, dots);
        assert!(app.scripts.menu_open(), "fixture: the menu is open");
        assert!(lit_at(&ctx, &mut app, dots), "the ⋯ wears the accent");
        press(&ctx, &mut app, egui::Key::Escape);
        assert!(!app.scripts.menu_open(), "Escape closed the menu");
        assert_eq!(
            app.session.selection.ids(),
            &selected[..],
            "the selection stays"
        );
    }

    /// After a press on the window the canvas takes no keys: `Delete` leaves
    /// the selected layer, and `Ctrl+Z` still reaches the document. A press
    /// outside the window gives the keys back.
    /// flip: the key routing without its `holds_keys` branch → fails at "the
    /// layer stays".
    #[test]
    fn the_window_keeps_the_keys_from_the_canvas_after_a_press_on_it() {
        use crate::command::tests::key;
        let (ctx, mut app) = open_on("print(1)");
        let card = app.session.selection.ids()[0];
        let window = ctx
            .memory(|m| m.area_rect(egui::Id::new("script-editor")))
            .unwrap();
        // The output pane's air: a press there takes no focus.
        click_at(
            &ctx,
            &mut app,
            window.left_bottom() + egui::vec2(400.0, -20.0),
        );
        assert!(
            ctx.memory(|m| m.focused()).is_none(),
            "fixture: nothing has the focus"
        );
        assert!(app.scripts.holds_keys(), "the window holds the keys");
        press(&ctx, &mut app, egui::Key::Delete);
        assert!(app.session.doc.get(card).is_some(), "the layer stays");
        let depth = app.session.history.undo_depth();
        app.session.selection.clear();
        let ctrl = egui::Modifiers::COMMAND;
        crate::command::tests::pass(&ctx, &mut app, vec![key(egui::Key::Z, ctrl)], ctrl);
        assert!(
            app.session.history.undo_depth() < depth,
            "Ctrl+Z still undoes"
        );
        let outside = egui::pos2(window.left() + 20.0, window.bottom() + 20.0);
        assert!(
            ctx.layer_id_at(outside)
                .is_none_or(|l| l.id != egui::Id::new("script-editor")),
            "fixture: the press lands outside"
        );
        click_at(&ctx, &mut app, outside);
        assert!(!app.scripts.holds_keys(), "a press outside gives them back");
    }

    /// Points the editor's library at a fresh scratch folder for this thread.
    fn scratch_library(tag: &str) -> std::path::PathBuf {
        let dir = std::env::temp_dir().join(format!("mastik-editor-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        library::TEST_DIR.with(|d| *d.borrow_mut() = Some(dir.clone()));
        dir
    }

    /// *Save to library* on an unsaved script moves it: the library holds it,
    /// open and with the keyboard in its code, and the unsaved row is gone. A
    /// document's script stays in the document, copied.
    /// flip: `scripts_act` not dropping the unsaved script → fails at "the
    /// unsaved row is gone".
    #[test]
    fn saving_an_unsaved_script_to_the_library_moves_it() {
        let dir = scratch_library("save");
        let (ctx, mut app) = open_on("print(1)");
        app.add_unsaved("print(2)".into());
        assert_eq!(app.scripts.unsaved.len(), 1, "fixture: one unsaved");
        app.scripts_act(Some(Act::SaveToLibrary));
        let saved = Which::Library("Untitled 1".into());
        assert_eq!(app.scripts.current, Some(saved.clone()), "it is open");
        assert!(app.scripts.unsaved.is_empty(), "the unsaved row is gone");
        assert_eq!(app.script_source(&saved).as_deref(), Some("print(2)"));
        crate::command::tests::pass(&ctx, &mut app, Vec::new(), egui::Modifiers::NONE);
        assert!(
            ctx.memory(|m| m.has_focus(app.scripts.code_id())),
            "with the keyboard in its code"
        );
        let doc = Which::Document(app.session.doc.scripts()[0].id);
        app.pick_script(doc.clone());
        app.scripts_act(Some(Act::SaveToLibrary));
        assert_eq!(
            app.session.doc.scripts().len(),
            1,
            "the document's script stays"
        );
        assert_eq!(app.scripts.library.len(), 2, "and the library has its copy");
        let _ = std::fs::remove_dir_all(dir);
    }

    /// A headless app whose editor is open on the library script `Grid`, its
    /// file holding `source`, in a scratch library folder.
    fn open_on_library(tag: &str, source: &str) -> (std::path::PathBuf, MastikApp) {
        let dir = scratch_library(tag);
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("Grid.luau"), source).unwrap();
        let ctx = egui::Context::default();
        crate::theme::install(&ctx);
        let mut app = MastikApp::headless(&ctx);
        app.open_scripts();
        assert_eq!(
            app.scripts.current,
            Some(Which::Library("Grid".into())),
            "fixture: Grid is open"
        );
        (dir, app)
    }

    /// An open library script whose file changed outside, with nothing typed
    /// waiting, shows the file's text once the folder is read again.
    /// flip: `refresh_script_library` not calling `follow_library_disk` → fails
    /// at "the pane shows the file".
    #[test]
    fn an_outside_edit_of_an_open_library_script_is_shown() {
        let (dir, mut app) = open_on_library("follow", "a");
        std::fs::write(dir.join("Grid.luau"), "b").unwrap();
        app.refresh_script_library();
        assert_eq!(app.scripts.buffer, "b", "the pane shows the file");
        assert_eq!(
            app.session.status().text,
            "Reloaded “Grid”: it changed on disk"
        );
        let _ = std::fs::remove_dir_all(dir);
    }

    /// Typing waiting over a library script whose file changed outside is
    /// never written over that file: it is saved beside it and shown there.
    /// flip: `flush_library_write` without its changed-on-disk check → fails at
    /// "the write is refused".
    #[test]
    fn typing_never_overwrites_an_outside_edit() {
        let (dir, mut app) = open_on_library("clash", "a");
        app.scripts.buffer = "a typed".into();
        app.store_buffer();
        assert!(app.scripts.pending.is_some(), "fixture: a write waits");
        std::fs::write(dir.join("Grid.luau"), "b").unwrap();
        assert!(!app.flush_library_write(), "the write is refused");
        let read = |f: &str| std::fs::read_to_string(dir.join(f)).unwrap();
        assert_eq!(read("Grid.luau"), "b", "Grid keeps the outside edit");
        assert_eq!(read("Grid 2.luau"), "a typed", "the typing is kept");
        assert_eq!(app.scripts.current, Some(Which::Library("Grid 2".into())));
        assert_eq!(app.scripts.buffer, "a typed");
        let _ = std::fs::remove_dir_all(dir);
    }

    /// *Copy into this document* of a library script copies what was typed,
    /// before its write delay has passed.
    /// flip: `scripts_act` not flushing first → fails at "the last typing".
    #[test]
    fn copy_into_the_document_takes_the_last_typing() {
        let (dir, mut app) = open_on_library("copy", "a");
        app.scripts.buffer = "a typed".into();
        app.store_buffer();
        app.scripts_act(Some(Act::CopyToDocument));
        let copied = app.session.doc.scripts().last().map(|s| s.source.clone());
        assert_eq!(copied.as_deref(), Some("a typed"), "the last typing");
        let _ = std::fs::remove_dir_all(dir);
    }

    /// A document script changed under the pane while its code has the
    /// keyboard keeps the keyboard in the code, on the new text's id.
    /// flip: `show_changed_text` not setting `focus_code` → fails at "the code
    /// keeps the keyboard".
    #[test]
    fn an_outside_change_to_the_open_script_keeps_the_keyboard() {
        use crate::command::tests::pass;
        let (ctx, mut app) = typing_at_end("sel.x += 1");
        let Some(Which::Document(id)) = app.scripts.current.clone() else {
            panic!("fixture: a document script is open");
        };
        let old = app.scripts.code_id();
        assert!(
            app.session
                .commit(Transaction(vec![Operation::SetScriptSource {
                    id,
                    source: "sel.x += 2".into(),
                }]))
        );
        pass(&ctx, &mut app, Vec::new(), egui::Modifiers::NONE);
        pass(&ctx, &mut app, Vec::new(), egui::Modifiers::NONE);
        assert_eq!(
            app.scripts.buffer, "sel.x += 2",
            "fixture: the pane follows"
        );
        assert_ne!(app.scripts.code_id(), old, "fixture: on a new id");
        let code = app.scripts.code_id();
        assert!(
            ctx.memory(|m| m.has_focus(code)),
            "the code keeps the keyboard"
        );
        assert_eq!(caret(&ctx, &app), Some(10), "the caret where it was");
    }

    /// A right-click on a list row opens that script and its menu at the
    /// press, with the ⋯ menu's rows; a row there acts on that script.
    /// flip: `source_list` not answering `secondary_clicked` → fails at "the
    /// menu opens".
    #[test]
    fn a_right_click_on_a_row_opens_its_menu() {
        use crate::command::tests::pass;
        let (ctx, mut app) = open_on("print(1)");
        app.add_unsaved(String::new());
        let unsaved = app.scripts.current.clone().unwrap();
        let doc = Which::Document(app.session.doc.scripts()[0].id);
        app.pick_script(unsaved);
        pass(&ctx, &mut app, Vec::new(), egui::Modifiers::NONE);
        let row = flat(&painted(&ctx, &mut app, Vec::new()))
            .iter()
            .find_map(|s| match s {
                egui::Shape::Text(t) if t.galley.text() == "Nudge" => {
                    Some(egui::Rect::from_min_size(t.pos, t.galley.size()).center())
                }
                _ => None,
            })
            .expect("fixture: the document's row is drawn");
        let none = egui::Modifiers::NONE;
        let button = |pressed| egui::Event::PointerButton {
            pos: row,
            button: egui::PointerButton::Secondary,
            pressed,
            modifiers: none,
        };
        pass(&ctx, &mut app, vec![egui::Event::PointerMoved(row)], none);
        pass(&ctx, &mut app, vec![button(true)], none);
        pass(&ctx, &mut app, vec![button(false)], none);
        pass(&ctx, &mut app, Vec::new(), none);
        assert!(app.scripts.menu_open(), "the menu opens");
        assert_eq!(app.scripts.current, Some(doc.clone()), "on that script");
        let dots = head_glyph(&ctx, &mut app, icon::DOTS_THREE);
        assert!(!lit_at(&ctx, &mut app, dots), "the ⋯ stays off");
        let menu = ctx
            .memory(|m| m.area_rect(egui::Id::new(MENU_ID)))
            .expect("the menu is drawn");
        assert!(
            menu.left() - row.x < 2.0 && menu.top() - row.y < 2.0,
            "at the press: {menu:?} for {row:?}"
        );
        let labels: Vec<String> = flat(&painted(&ctx, &mut app, Vec::new()))
            .iter()
            .filter_map(|s| match s {
                egui::Shape::Text(t) if menu.contains(t.pos) => Some(t.galley.text().to_owned()),
                _ => None,
            })
            .collect();
        for (_, _, label) in menu_rows(&doc).into_iter().flatten() {
            assert!(labels.iter().any(|l| l == label), "{label:?} in {labels:?}");
        }
    }

    /// The script menu leaves out what can never apply: a kit's script has no
    /// rename, file or delete, a library script no *Save to library*.
    #[test]
    fn the_menu_rows_follow_where_the_script_lives() {
        let labels = |w: &Which| -> Vec<&str> {
            menu_rows(w)
                .into_iter()
                .flatten()
                .map(|(_, _, l)| l)
                .collect()
        };
        let kit = Which::Kit {
            kit: "k".into(),
            kit_name: "Kit".into(),
            id: ScriptId(mastik_core::NodeId { actor: 1, seq: 1 }),
        };
        assert_eq!(
            labels(&kit),
            ["Save to library", "Copy into this document", "Duplicate"]
        );
        assert_eq!(
            labels(&Which::Library("a".into())),
            [
                "Copy into this document",
                "Duplicate",
                "Rename",
                "Show in folder",
                "Delete"
            ]
        );
    }

    /// Rename puts a field over the open script's list row, where its name
    /// was, holding the name; `Enter` keeps what was typed, and `Escape` the
    /// old name.
    /// flip: `row_rename` committing on `Escape` → fails at "Escape keeps the
    /// old name".
    #[test]
    fn rename_types_over_the_list_row() {
        use crate::command::tests::pass;
        let none = egui::Modifiers::NONE;
        let (ctx, mut app) = open_on("print(1)");
        let id = app.session.doc.scripts()[0].id;
        app.scripts_act(Some(Act::StartRename));
        pass(&ctx, &mut app, Vec::new(), none);
        pass(&ctx, &mut app, Vec::new(), none);
        let field = egui::Id::new("script-rename");
        assert!(
            ctx.memory(|m| m.has_focus(field)),
            "the field has the keyboard"
        );
        let rect = ui::widget_rect(&ctx, field)
            .expect("the field is drawn")
            .rect;
        let window = ctx
            .memory(|m| m.area_rect(egui::Id::new("script-editor")))
            .unwrap();
        assert!(
            rect.right() < window.left() + LIST_W,
            "on the list's row: {rect:?}"
        );
        // Typed over the selected name.
        pass(&ctx, &mut app, vec![egui::Event::Text("Grid".into())], none);
        press(&ctx, &mut app, egui::Key::Enter);
        assert_eq!(app.session.doc.script(id).unwrap().name, "Grid");
        assert!(app.scripts.renaming.is_none(), "the field is gone");
        app.scripts_act(Some(Act::StartRename));
        pass(&ctx, &mut app, Vec::new(), none);
        pass(&ctx, &mut app, Vec::new(), none);
        pass(&ctx, &mut app, vec![egui::Event::Text("Rows".into())], none);
        press(&ctx, &mut app, egui::Key::Escape);
        assert_eq!(
            app.session.doc.script(id).unwrap().name,
            "Grid",
            "Escape keeps the old name"
        );
        assert!(app.scripts.renaming.is_none(), "the field is gone");
    }

    /// A syntax error holds a run from anywhere before its disclosure, says so
    /// on the status line, and holds the palette's *Run* row with *Run*'s reason.
    /// flip: `start_script_run` not checking the syntax → fails at "no disclosure".
    #[test]
    fn a_syntax_error_holds_every_run() {
        let (mut app, card, script) =
            with_script("for _, n in ipairs(sel) do\n  n.x += 1\n", Some("doc-a"));
        let x = left(&app, card);
        app.start_script_run(Which::Document(script));
        assert!(app.scripts.disclosure.is_none(), "no disclosure");
        assert_eq!(left(&app, card), x, "nothing ran");
        assert_eq!(
            app.session.status().text,
            "Nudge has a syntax error on line 2"
        );
        let runs = app.saved_script_runs();
        assert_eq!(
            runs,
            [(
                "Nudge".to_owned(),
                Some("Fix the syntax error on line 2 to run".to_owned())
            )]
        );
    }

    /// The check marks a problem once typing rests, not while it goes on, and
    /// never a name's problem under the caret.
    /// flip: `Help::problems` checking without waiting for a rest → fails at
    /// "nothing while typing".
    #[test]
    fn problems_wait_for_a_rest() {
        use crate::command::tests::pass;
        let (ctx, mut app) = typing_at_end("sel.widht = 3\n");
        let none = egui::Modifiers::NONE;
        let marked = |app: &mut MastikApp, ctx: &egui::Context| {
            let (doc, page) = (app.session.doc.clone(), app.session.page);
            let (buffer, rev) = (app.scripts.buffer.clone(), app.session.revision());
            app.scripts
                .help
                .problems(ctx, &doc, rev, page, &buffer)
                .len()
        };
        assert_eq!(
            marked(&mut app, &ctx),
            1,
            "fixture: opened, checked at once"
        );
        type_code(&ctx, &mut app, "print(x)");
        assert_eq!(marked(&mut app, &ctx), 0, "nothing while typing");
        for _ in 0..60 {
            pass(&ctx, &mut app, Vec::new(), none);
        }
        assert_eq!(
            marked(&mut app, &ctx),
            2,
            "after the rest: widht and x in {:?}",
            app.scripts.buffer
        );
        // Just after the `x`, before the `)`.
        let caret = Some(app.scripts.buffer.len() - 1);
        let problems = {
            let (doc, page) = (app.session.doc.clone(), app.session.page);
            let (buffer, rev) = (app.scripts.buffer.clone(), app.session.revision());
            app.scripts
                .help
                .problems(&ctx, &doc, rev, page, &buffer)
                .to_vec()
        };
        let marks = super::super::script_help::Help::marks(&problems, caret, None);
        assert_eq!(marks.len(), 1, "the name under the caret is not marked");
    }

    /// A layer made under an open script is known to the check at once, with
    /// the text unchanged.
    /// flip: `Help::problems` keyed on the text alone → fails at "the new layer".
    #[test]
    fn the_check_follows_the_layers() {
        let (ctx, mut app) = open_on("print(Banner)");
        let marked = |app: &mut MastikApp| {
            let (doc, page) = (app.session.doc.clone(), app.session.page);
            let (buffer, rev) = (app.scripts.buffer.clone(), app.session.revision());
            app.scripts
                .help
                .problems(&ctx, &doc, rev, page, &buffer)
                .iter()
                .map(|p| p.message.clone())
                .collect::<Vec<_>>()
        };
        let before = marked(&mut app);
        assert!(
            before.iter().any(|m| m.starts_with("No name “Banner”")),
            "fixture: unknown at first: {before:?}"
        );
        let id = add_rect(&mut app);
        let named = Transaction(vec![Operation::SetName {
            id,
            name: "Banner".into(),
        }]);
        assert!(app.session.commit(named), "fixture: renamed");
        assert_eq!(marked(&mut app), Vec::<String>::new(), "the new layer");
    }

    /// Typing a verb's `(` hangs its hint over the line; `Escape` hides it. A
    /// caret moved into a call without typing shows none.
    /// flip: `Help::pane` showing the hint for any call the caret is in →
    /// fails at "moved in, not typed".
    #[test]
    fn typing_a_verbs_open_bracket_shows_its_hint() {
        let (ctx, mut app) = typing_at_end("align(\"left\")\n");
        let hint = egui::LayerId::new(
            egui::Order::Foreground,
            egui::Id::new("script-editor").with("hint"),
        );
        let up = |ctx: &egui::Context| ctx.memory(|m| m.areas().is_visible(&hint));
        press(&ctx, &mut app, egui::Key::ArrowUp);
        press(&ctx, &mut app, egui::Key::ArrowRight);
        press(&ctx, &mut app, egui::Key::ArrowRight);
        press(&ctx, &mut app, egui::Key::ArrowRight);
        press(&ctx, &mut app, egui::Key::ArrowRight);
        press(&ctx, &mut app, egui::Key::ArrowRight);
        press(&ctx, &mut app, egui::Key::ArrowRight);
        press(&ctx, &mut app, egui::Key::ArrowRight);
        assert_eq!(caret(&ctx, &app), Some(7), "fixture: in the call");
        assert!(!up(&ctx), "moved in, not typed");
        let chord = egui::Modifiers::CTRL | egui::Modifiers::SHIFT;
        crate::command::tests::pass(
            &ctx,
            &mut app,
            vec![crate::command::tests::key(egui::Key::Space, chord)],
            chord,
        );
        crate::command::tests::pass(&ctx, &mut app, Vec::new(), egui::Modifiers::NONE);
        assert!(up(&ctx), "Ctrl+Shift+Space asks for it");
        assert!(!app.scripts.suggest.on(), "and opens no list");
        let (ctx, mut app) = typing_at_end("");
        type_code(&ctx, &mut app, "align(");
        assert!(up(&ctx), "the hint is up");
        press(&ctx, &mut app, egui::Key::Escape);
        press(&ctx, &mut app, egui::Key::Escape);
        assert!(!up(&ctx), "Escape hid it");
    }

    /// A lone `(` typed after a pause, which the editing keys pair with its
    /// close, opens the call's hint as a typed one does.
    /// flip: `script_keys` not telling the help it edited → fails at "the
    /// paired bracket opens it".
    #[test]
    fn an_auto_paired_bracket_opens_the_hint() {
        use crate::command::tests::pass;
        let (ctx, mut app) = typing_at_end("");
        let hint = egui::LayerId::new(
            egui::Order::Foreground,
            egui::Id::new("script-editor").with("hint"),
        );
        type_code(&ctx, &mut app, "align");
        press(&ctx, &mut app, egui::Key::Escape);
        for _ in 0..5 {
            pass(&ctx, &mut app, Vec::new(), egui::Modifiers::NONE);
        }
        type_code(&ctx, &mut app, "(");
        assert_eq!(
            app.scripts.buffer, "align()",
            "fixture: the close was paired"
        );
        assert!(
            ctx.memory(|m| m.areas().is_visible(&hint)),
            "the paired bracket opens it"
        );
    }

    /// `F12` selects the declaration of the name at the caret.
    /// flip: `Help::pane` ignoring F12 → fails at "the declaration".
    #[test]
    fn f12_goes_to_the_declaration() {
        let source = "local gap = 1\nprint(gap";
        let (ctx, mut app) = typing_at_end(source);
        press(&ctx, &mut app, egui::Key::F12);
        let range = egui::TextEdit::load_state(&ctx, app.scripts.code_id())
            .and_then(|s| s.cursor.char_range())
            .map(|r| {
                let (a, b) = (r.primary.index.0, r.secondary.index.0);
                a.min(b)..a.max(b)
            });
        assert_eq!(range, Some(6..9), "the declaration");
    }

    /// The output pane lays out only the rows in view, however much the kept
    /// runs printed, and still ends on the newest line; a printed line is split
    /// at its newlines and a very long one cut.
    /// flip: `output_pane` walking each run's every row, not the range in view
    /// → fails at "only the rows in view".
    #[test]
    fn the_output_lays_out_only_the_rows_in_view() {
        let (ctx, mut app) = open_on("print(1)");
        let mut frame = eframe::Frame::_new_kittest();
        let mut rows = |app: &mut MastikApp| {
            let input = egui::RawInput {
                screen_rect: Some(egui::Rect::from_min_size(
                    egui::Pos2::ZERO,
                    egui::vec2(1320.0, 820.0),
                )),
                ..Default::default()
            };
            let out = ctx.run_ui(input, |ui| eframe::App::ui(app, ui, &mut frame));
            let mut texts = Vec::new();
            for s in out.shapes {
                if let egui::Shape::Text(t) = s.shape
                    && t.galley.job.text.starts_with("row ")
                {
                    texts.push(t.galley.job.text.clone());
                }
            }
            texts
        };
        // The text cache keeps what the last frame laid out, drawn or clipped.
        let laid_out = || ctx.fonts(|f| f.num_galleys_in_cache());
        rows(&mut app);
        let before = laid_out();
        for _ in 0..20 {
            let lines = (0..10_000).map(|i| (format!("row {i}"), false)).collect();
            app.push_output(lines, 0.1);
        }
        rows(&mut app);
        rows(&mut app);
        let texts = rows(&mut app);
        let grew = laid_out().saturating_sub(before);
        assert!(grew < 200, "only the rows in view: {grew} laid out");
        assert!(texts.iter().any(|t| t == "row 9999"), "the newest line");

        app.push_output(vec![("a\nb".into(), false), ("x".repeat(3_000), true)], 0.1);
        let run = app.scripts.output.last().unwrap();
        assert_eq!(run.lines.len(), 3, "split at the newline");
        assert_eq!(run.lines[2].0.chars().count(), OUTPUT_ROW_CHARS + 1);
    }
}
