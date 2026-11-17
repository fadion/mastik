//! A run — a bar line with Enter, a script, an `/ai` reply's script — on a worker
//! against a copy of the document as it stood (`command-mode.md` §8). The window
//! keeps drawing, `Escape` or the status bar's *Stop* ends it, and what it made
//! lands as one undo step. The document is held meanwhile
//! ([`EditorSession::script_run`]): nothing else commits, so the step lands on the
//! very document the copy was made from.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

use mastik_core::{IdSource, NodeId, StepNote, Transaction};
use rustc_hash::FxHashMap;

use super::editor::Which;
use super::host::{self, Counts, SessionHost};
use crate::app::MastikApp;
use crate::job::Job;
use crate::session::EditorSession;

/// How long the frame that starts a run waits for it: a run that ends sooner
/// lands in that frame, as a quick line should, with nothing shown running.
#[cfg_attr(
    test,
    expect(dead_code, reason = "a test waits as long as `INLINE` says")
)]
const LANDS_INLINE: Duration = Duration::from_millis(50);

#[cfg(test)]
thread_local! {
    /// [`LANDS_INLINE`] in tests: long, so a run lands where it starts and a
    /// test reads its result at once; a test of a run in flight sets it to zero.
    pub(crate) static INLINE: std::cell::Cell<Duration> =
        const { std::cell::Cell::new(Duration::from_secs(120)) };
}

/// How long the starting frame waits.
fn inline_wait() -> Duration {
    #[cfg(test)]
    return INLINE.with(std::cell::Cell::get);
    #[cfg(not(test))]
    LANDS_INLINE
}

/// The status line's refusal while `name` runs: *“Card grid” is running — Esc
/// stops it*.
pub(crate) fn held(name: &str) -> String {
    format!("{} is running — Esc stops it", upper_first(name))
}

/// `s` with its first letter in upper case.
fn upper_first(s: &str) -> String {
    let mut c = s.chars();
    c.next()
        .map_or_else(String::new, |f| f.to_uppercase().chain(c).collect())
}

/// What to run.
pub(crate) enum Code {
    /// A bar line, through the line sugar.
    Line(String),
    /// A script, as written.
    Script(String),
}

/// What a landed run reports to, and how.
pub(crate) enum After {
    /// A bar line that writes: `line` and the Luau it was read as.
    Bar { line: String, luau: String },
    /// A bar line that reads: its value is copied.
    Read { line: String, luau: String },
    /// A script from the editor or the palette.
    Script { which: Which, name: String },
    /// An `/ai` reply's script, the step marked as the agent's.
    Ai {
        request: String,
        script: String,
        speaker: String,
    },
}

/// A run in flight.
pub(crate) struct Running {
    /// How the status line names it: *“Card grid”*, *the line*.
    pub(crate) name: String,
    pub(crate) started: Instant,
    /// The session's revision the copy was made at: a step lands only on it.
    revision: u64,
    stop: Stop,
    job: Job<Landed>,
    pub(crate) after: After,
}

/// A run's stop flag, set when it is dropped: a run no one waits for any more
/// (its tab closed, its document replaced) stops, since its step would land
/// nowhere.
struct Stop(Arc<AtomicBool>);

impl Drop for Stop {
    fn drop(&mut self) {
        self.0.store(true, Ordering::Relaxed);
    }
}

/// The step a run made on its copy, ready to land: as the copy's commit door
/// settled it, the note it carries, and the seq the copy's id source minted up
/// to, which the session's moves past.
pub(crate) struct Settled {
    pub(crate) forward: Transaction,
    pub(crate) note: StepNote,
    pub(crate) next_seq: u64,
}

/// What a worker answers.
pub(crate) struct Landed {
    pub(crate) ran: Result<(mastik_vm::Outcome, Counts, Option<Settled>), mastik_vm::VmError>,
    pub(crate) secs: f32,
}

/// Everything a worker needs, owned.
struct Order {
    doc: mastik_core::Document,
    resolved: mastik_core::Resolved,
    ids: IdSource,
    page: NodeId,
    selection: Vec<NodeId>,
    labels: FxHashMap<String, NodeId>,
    params: serde_json::Map<String, serde_json::Value>,
    code: Code,
    read_only: bool,
    label: String,
    glyph: &'static str,
}

/// Run `order` on a copy of the session it was taken from: one step, settled.
fn work(order: Order, stop: Arc<AtomicBool>) -> Landed {
    let started = Instant::now();
    // A worker's text engine holds only Inter: the document's faces first.
    mastik_core::text::register_font_data(&order.resolved.text_fonts());
    let mut scratch = EditorSession::scratch(order.doc, order.resolved, order.ids, order.page);
    let env = mastik_vm::Env {
        selection: &order.selection,
        labels: &order.labels,
        params: order.params,
    };
    let leash = mastik_vm::Leash {
        budget: mastik_vm::WORKER_BUDGET,
        stop: Some(stop),
    };
    let call = |h: &mut SessionHost<'_>| match &order.code {
        Code::Line(line) => mastik_vm::run_line_leashed(h, &env, line, &leash),
        Code::Script(source) => mastik_vm::run_script_leashed(h, &env, source, &leash),
    };
    let ran = if order.read_only {
        let mut h = SessionHost {
            s: &mut scratch,
            read_only: true,
        };
        call(&mut h).map(|outcome| (outcome, Counts::default(), None))
    } else {
        host::run_as_step(&mut scratch, &order.label, order.glyph, call).map(
            |(outcome, counts, serial)| {
                let settled = serial.and_then(|_| settled_step(&mut scratch));
                (outcome, counts, settled)
            },
        )
    };
    Landed {
        ran,
        secs: started.elapsed().as_secs_f32(),
    }
}

/// The step on top of `scratch`, as forward edit: undone once, its redo is
/// what applying it does to the document the copy was made from.
fn settled_step(scratch: &mut EditorSession) -> Option<Settled> {
    let note = scratch.history.done().next_back()?.note.clone();
    let next_seq = scratch.ids.next_seq();
    scratch.history.undo(&mut scratch.doc).ok()??;
    let forward = scratch.history.next_redo()?.clone();
    Some(Settled {
        forward,
        note,
        next_seq,
    })
}

impl MastikApp {
    /// Run `code` on a worker as one step named `label` with `glyph`, reporting
    /// to `after`. Refused, with the status line saying why, while another run
    /// holds the document. A run that ends within [`LANDS_INLINE`] lands now.
    #[allow(clippy::too_many_arguments)]
    pub(crate) fn begin_run(
        &mut self,
        name: String,
        code: Code,
        read_only: bool,
        selection: Vec<NodeId>,
        labels: FxHashMap<String, NodeId>,
        params: serde_json::Map<String, serde_json::Value>,
        label: &str,
        glyph: &'static str,
        after: After,
    ) {
        if let Some(run) = &self.session.script_run {
            let why = held(&run.name);
            self.session.fail(why);
            return;
        }
        let s = &self.session;
        // The copy mints from where the session would, so the ids it makes are
        // the ones the session would have made.
        let mut ids = IdSource::new(s.ids.actor());
        ids.skip_to(s.ids.next_seq());
        let order = Order {
            doc: s.doc.clone(),
            resolved: s.resolved.clone(),
            ids,
            page: s.page,
            selection,
            labels,
            params,
            code,
            read_only,
            label: label.to_owned(),
            glyph,
        };
        let stop = Arc::new(AtomicBool::new(false));
        let flag = stop.clone();
        let started = Instant::now();
        let revision = self.session.revision();
        let job = Job::spawn("mastik-script", &self.egui_ctx, move || work(order, flag));
        let landed = job.wait_for(inline_wait());
        let run = Running {
            name,
            started,
            revision,
            stop: Stop(stop),
            job,
            after,
        };
        match landed {
            Ok(Some(landed)) => self.land_run(run, landed),
            Ok(None) => {
                self.session.script_run = Some(run);
                self.egui_ctx.request_repaint();
            }
            Err(()) => self.run_died(run),
        }
    }

    /// Each frame: `Escape` stops a run in flight, and one that has ended
    /// lands, unless a gesture is live or a modal is up, which may be showing
    /// the document as it stands.
    pub(crate) fn tend_script_run(&mut self, ctx: &egui::Context) {
        if self.session.script_run.is_none() {
            return;
        }
        if !self.modal_is_up()
            && ctx.input_mut(|i| i.consume_key(egui::Modifiers::NONE, egui::Key::Escape))
        {
            self.stop_script_run();
            return;
        }
        // Wake once a second for the status bar's count, and for the landing.
        ctx.request_repaint_after(Duration::from_millis(250));
        if self.gesture_live() || self.modal_is_up() {
            return;
        }
        let Some(polled) = self.session.script_run.as_ref().map(|r| r.job.poll()) else {
            return;
        };
        match polled {
            Ok(None) => {}
            Ok(Some(landed)) => {
                if let Some(run) = self.session.script_run.take() {
                    self.land_run(run, landed);
                }
            }
            Err(()) => {
                if let Some(run) = self.session.script_run.take() {
                    self.run_died(run);
                }
            }
        }
    }

    /// Stop the run in flight: nothing it made lands. Its worker ends at its
    /// next step, unseen.
    pub(crate) fn stop_script_run(&mut self) {
        let Some(run) = self.session.script_run.take() else {
            return;
        };
        run.stop.0.store(true, Ordering::Relaxed);
        let secs = run.started.elapsed().as_secs_f32();
        let e = mastik_vm::VmError {
            kind: mastik_vm::ErrorKind::Stopped,
            message: "Stopped".into(),
            line: None,
            span: None,
            fix: None,
        };
        self.run_failed(run.after, e, secs);
    }

    /// The status bar's words while a run is in flight: *Running “Card grid”…*,
    /// the seconds once one has passed.
    pub(crate) fn running_said(&self) -> Option<String> {
        let run = self.session.script_run.as_ref()?;
        let secs = run.started.elapsed().as_secs();
        Some(match secs {
            0 => format!("Running {}…", run.name),
            n => format!("Running {}… {n} s", run.name),
        })
    }

    /// A worker that ended without an answer: nothing changed.
    fn run_died(&mut self, run: Running) {
        let e = mastik_vm::VmError {
            kind: mastik_vm::ErrorKind::Error,
            message: "The run ended without an answer".into(),
            line: None,
            span: None,
            fix: None,
        };
        let secs = run.started.elapsed().as_secs_f32();
        self.run_failed(run.after, e, secs);
    }

    /// Land what `run`'s worker answered: its step committed as one, then the
    /// report its door asked for.
    pub(crate) fn land_run(&mut self, run: Running, landed: Landed) {
        let Running {
            revision, after, ..
        } = run;
        let (outcome, counts, settled) = match landed.ran {
            Ok(ran) => ran,
            Err(e) => return self.run_failed(after, e, landed.secs),
        };
        let mut serial = None;
        if let Some(mut step) = settled {
            // Held while it ran, the document moves only when replaced whole.
            if self.session.revision() != revision {
                let e = mastik_vm::VmError {
                    kind: mastik_vm::ErrorKind::Error,
                    message: "The document changed while it ran".into(),
                    line: None,
                    span: None,
                    fix: None,
                };
                return self.run_failed(after, e, landed.secs);
            }
            if let After::Ai { speaker, .. } = &after {
                step.note.origin = mastik_core::Origin::Mcp {
                    client: speaker.clone(),
                    tool: "/ai".into(),
                };
            }
            match self
                .session
                .commit_settled(step.forward, step.note, step.next_seq)
            {
                Ok(s) => serial = s,
                Err(e) => {
                    let e = mastik_vm::VmError {
                        kind: mastik_vm::ErrorKind::Error,
                        message: format!("Its step did not apply: {e}"),
                        line: None,
                        span: None,
                        fix: None,
                    };
                    return self.run_failed(after, e, landed.secs);
                }
            }
        }
        match after {
            After::Bar { line, luau } => {
                self.bar_line_landed(&line, luau, false, outcome, counts, serial);
            }
            After::Read { line, luau } => {
                self.bar_line_landed(&line, luau, true, outcome, counts, serial);
            }
            After::Script { which, name } => {
                self.script_landed(&which, &name, outcome, counts, serial, landed.secs);
            }
            After::Ai {
                request, script, ..
            } => self.ai_script_landed(&request, script, outcome, counts, serial),
        }
    }

    /// Report a run that changed nothing to the door that started it.
    fn run_failed(&mut self, after: After, e: mastik_vm::VmError, secs: f32) {
        match after {
            After::Bar { line, .. } | After::Read { line, .. } => self.bar_line_failed(&line, e),
            After::Script { which, name } => self.script_failed(&which, &name, &e, secs),
            After::Ai { .. } => self.ai_script_failed(e),
        }
    }
}

/// Wait for the run in flight to end, then land it, as a frame would.
#[cfg(test)]
pub(crate) fn settle(app: &mut MastikApp) {
    let Some(run) = app.session.script_run.take() else {
        return;
    };
    let landed = loop {
        match run.job.wait_for(Duration::from_secs(60)) {
            Ok(Some(landed)) => break landed,
            Ok(None) => {}
            Err(()) => panic!("the worker ended without an answer"),
        }
    };
    app.land_run(run, landed);
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::command::tests::{key, pass};
    use crate::palette::tests::add_rect;
    use mastik_core::{Operation, Transaction};

    /// A headless app holding one rectangle, selected.
    fn with_card() -> (egui::Context, MastikApp, NodeId) {
        let ctx = egui::Context::default();
        crate::theme::install(&ctx);
        let mut app = MastikApp::headless(&ctx);
        let card = add_rect(&mut app);
        app.session.selection.set_one(card);
        (ctx, app, card)
    }

    /// Run bar `line` with no inline wait, so it is still in flight after.
    fn start(app: &mut MastikApp, line: &str) {
        let held = INLINE.replace(Duration::ZERO);
        app.run_bar_line(line);
        INLINE.set(held);
    }

    /// A rename of `id`, as any door would commit one.
    fn rename(id: NodeId, name: &str) -> Transaction {
        Transaction(vec![Operation::SetName {
            id,
            name: name.into(),
        }])
    }

    /// A run made on a worker lands as one step on the document it was made
    /// from, and the session mints past the ids its layers took.
    /// flip: `commit_settled` not moving the id source past the copy's → fails
    /// at "a layer made after it commits".
    #[test]
    fn a_run_lands_as_one_step() {
        let (_ctx, mut app, card) = with_card();
        let page = app.session.doc.first_page();
        let count = |app: &MastikApp| app.session.doc.get(page).unwrap().children().len();
        let depth = app.session.history.undo_depth();
        let before = count(&app);
        start(&mut app, "duplicate(); duplicate(); duplicate()");
        assert!(app.session.script_run.is_some(), "fixture: in flight");
        settle(&mut app);
        assert_eq!(count(&app), before + 3, "three copies");
        assert_eq!(app.session.history.undo_depth(), depth + 1, "one step");
        assert!(app.session.script_run.is_none());
        let id = app.session.ids.mint();
        let made = Transaction(vec![Operation::CreateNode {
            id,
            parent: page,
            index: 0,
            kind: app.session.doc.get(card).unwrap().kind().clone(),
            transform: None,
            name: None,
        }]);
        assert!(
            app.session.try_commit(made).is_ok(),
            "a layer made after it commits"
        );
        assert!(app.session.undo() && app.session.undo());
        assert_eq!(count(&app), before, "one undo takes the run back whole");
    }

    /// While a run is in flight the document takes no edit and no undo, the
    /// status bar says it runs, and Escape stops it with nothing changed,
    /// ending the hold.
    /// flip: `tend_script_run` not reading Escape → fails at "Escape stopped it".
    #[test]
    fn escape_stops_a_run_and_nothing_changes() {
        let (ctx, mut app, card) = with_card();
        let depth = app.session.history.undo_depth();
        start(&mut app, ".name = \"Busy\"; while true do end");
        assert!(app.session.script_run.is_some(), "fixture: in flight");
        assert!(!app.session.commit(rename(card, "Mine")), "held");
        assert_eq!(
            app.session.status().text,
            "The line is running — Esc stops it"
        );
        assert!(!app.session.undo(), "undo is held");
        assert_eq!(
            app.running_said().as_deref(),
            Some("Running the line…"),
            "the status bar says so"
        );
        pass(
            &ctx,
            &mut app,
            vec![key(egui::Key::Escape, egui::Modifiers::NONE)],
            egui::Modifiers::NONE,
        );
        assert!(app.session.script_run.is_none(), "Escape stopped it");
        assert_eq!(
            app.session.status().text,
            "The line stopped — nothing changed"
        );
        assert_eq!(app.session.history.undo_depth(), depth, "no step");
        assert_eq!(app.session.doc.get(card).unwrap().name(), "Card");
        assert!(app.session.commit(rename(card, "Mine")), "the hold ends");
    }
}
