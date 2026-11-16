//! The script editor's language help, drawn over its code pane: the parameter
//! hint over the call's line, a hover card under the pointer, the problems a
//! check finds once typing rests (marked in the gutter and squiggled, each with
//! its card), and go to declaration (`Ctrl`+click, `F12`). What each says is
//! `mastik_vm`'s (`signature_at`, `hover_at`, `check`, `declaration_at`); this
//! only draws it and says what the pane should do.

use std::ops::Range;

use super::char_to_byte as byte_of;
use crate::theme::{self, color, icon};
use crate::ui;
use eframe::egui;
use mastik_core::{Document, NodeId, Resolved};
use mastik_vm::{Hover, Problem, Severity, Signature};

/// How long typing must rest before the check marks problems: the editor's own
/// library-write delay, so the two settle together.
pub(crate) const CHECK_AFTER: f64 = 0.6;

/// How long a declaration's line stays washed after a jump to it, fading.
const FLASH_FOR: f64 = 0.6;

/// The narrowest and widest a hint or hover card's text runs.
const CARD_MIN_W: f32 = 240.0;
const CARD_MAX_W: f32 = 520.0;

/// Between a card's lines.
const LINE_GAP: f32 = 4.0;

/// The wash over the name a hover card explains, and over a flashed line.
const WASH_ALPHA: u8 = 38;

/// What the pane should do for the help this frame.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum HelpAct {
    /// Select this range of the text (a declaration, a problem's token).
    Select(Range<usize>),
    /// Write `text` over `range`, as one edit of the text.
    Replace(Range<usize>, String),
}

/// The help's state between frames.
#[derive(Default)]
pub(crate) struct Help {
    /// The check's problems, and what they were found in: the text's hash, and
    /// the document's revision and page, whose layer names the check reads.
    checked: Option<((u64, u64, NodeId), Vec<Problem>)>,
    /// Run's hold: a compile of the text, by its hash; never waits for a rest.
    compiled: Option<(u64, Option<Problem>)>,
    /// When the text last changed (`ctx` time); `None` before any typing, so a
    /// script opened is checked at once.
    edited_at: Option<f64>,
    /// The call whose hint is up, by the byte it opens at: opened by typing its
    /// `(`, a `,` or a `{`, shut when the caret leaves the call or by `Escape`.
    hint_call: Option<usize>,
    /// The text changed last frame: this frame reads what was typed.
    typed: bool,
    /// `Ctrl+Shift+Space` asked for the hint of the call the caret is in.
    asked: bool,
    /// What the pointer rests on: a problem (`true`) or a name, by its first byte.
    resting: ui::Resting<(bool, usize)>,
    /// A problem's card left up while the pointer crosses into it: its index
    /// and the card's rect last frame.
    problem_card: Option<(usize, egui::Rect)>,
    /// A line to wash after a jump, by a byte on it, and when the jump was.
    flash: Option<(usize, f64)>,
    /// A byte to scroll into view next frame, once the jump's caret is placed.
    reveal: Option<usize>,
    /// The layer a hover card names, outlined on the canvas while it is up.
    pub(crate) lit_layer: Option<NodeId>,
    /// The saved scripts' syntax errors' lines, by their text's hash, for the
    /// palette's *Run* rows, which are listed every frame it is open.
    holds: std::cell::RefCell<rustc_hash::FxHashMap<u64, Option<u32>>>,
}

/// `text`'s hash, for the caches.
fn hash(text: &str) -> u64 {
    egui::util::hash(text)
}

/// The char index of byte `b` of `text`.
fn chars_to(text: &str, b: usize) -> usize {
    text.get(..b).map_or(0, |t| t.chars().count())
}

/// The screen rect `range` of `text` takes in the laid-out pane (one row: a
/// name, a token, a line's text).
pub(crate) fn span_rect(
    out: &egui::text_edit::TextEditOutput,
    text: &str,
    range: &Range<usize>,
) -> egui::Rect {
    let at = |b: usize| {
        out.galley
            .pos_from_cursor(egui::text::CCursor::new(chars_to(text, b)))
            .translate(out.galley_pos.to_vec2())
    };
    let (a, b) = (at(range.start), at(range.end));
    if (a.center().y - b.center().y).abs() < 1.0 {
        a.union(b)
    } else {
        a
    }
}

/// The screen rect of the row holding byte `b`, the pane's width.
fn line_rect(
    out: &egui::text_edit::TextEditOutput,
    text: &str,
    b: usize,
    view: egui::Rect,
) -> egui::Rect {
    let r = span_rect(out, text, &(b..b));
    egui::Rect::from_x_y_ranges(view.x_range(), r.y_range())
}

/// The byte whose character is under `pos` in the laid-out pane, if one is.
fn byte_under(out: &egui::text_edit::TextEditOutput, text: &str, pos: egui::Pos2) -> Option<usize> {
    let local = pos - out.galley_pos;
    if !out
        .galley
        .rect
        .expand2(egui::vec2(0.0, 0.5))
        .contains(local.to_pos2())
    {
        return None;
    }
    let c = out.galley.cursor_from_pos(local).index.0;
    let b = byte_of(text, c);
    // The cursor is the boundary nearest the pointer: the character hit is the
    // one after it or the one before.
    let before = text[..b].chars().next_back().map(|ch| b - ch.len_utf8());
    [Some(b), before].into_iter().flatten().find(|&s| {
        let e = s + text[s..].chars().next().map_or(0, char::len_utf8);
        e > s
            && span_rect(out, text, &(s..e))
                .expand2(egui::vec2(0.0, 1.0))
                .contains(pos)
    })
}

/// The line (from 0) of byte `b`.
fn line_of(text: &str, b: usize) -> usize {
    text[..b.min(text.len())].matches('\n').count()
}

impl Help {
    /// Note that the text changed this frame: problems wait for the next rest.
    pub(crate) fn edited(&mut self, ctx: &egui::Context) {
        self.edited_at = Some(ctx.input(|i| i.time));
        self.typed = true;
    }

    /// Whether the parameter hint is up.
    pub(crate) fn hint_up(&self) -> bool {
        self.hint_call.is_some()
    }

    /// `Ctrl+Shift+Space`: show the hint of the call the caret is in, typed or not.
    pub(crate) fn ask_hint(&mut self) {
        self.asked = true;
    }

    /// The syntax error in `text` that holds *Run*, compiled again only when the
    /// text changed, never waiting for a rest.
    pub(crate) fn syntax(&mut self, text: &str) -> Option<&Problem> {
        let h = hash(text);
        if self.compiled.as_ref().is_none_or(|(k, _)| *k != h) {
            self.compiled = Some((h, mastik_vm::syntax_error(text)));
        }
        self.compiled.as_ref().and_then(|(_, p)| p.as_ref())
    }

    /// The line of the syntax error that holds a run of `source`, compiled once
    /// per text.
    pub(crate) fn held_line(&self, source: &str) -> Option<u32> {
        *self
            .holds
            .borrow_mut()
            .entry(hash(source))
            .or_insert_with(|| mastik_vm::syntax_error(source).map(|p| p.line))
    }

    /// The problems marked now: the check of `text` against `doc` at `revision`
    /// on `page`, once typing has rested [`CHECK_AFTER`], none while the text
    /// differs from what was checked. An edit to the document or a page switch
    /// with the text unchanged checks again at once: the layer names changed.
    pub(crate) fn problems(
        &mut self,
        ctx: &egui::Context,
        doc: &Document,
        revision: u64,
        page: NodeId,
        text: &str,
    ) -> &[Problem] {
        let key = (hash(text), revision, page);
        let checked = self.checked.as_ref().map(|(k, _)| *k);
        if checked != Some(key) {
            let same_text = checked.is_some_and(|k| k.0 == key.0);
            let now = ctx.input(|i| i.time);
            let left = self.edited_at.map_or(0.0, |at| CHECK_AFTER - (now - at));
            if left > 0.0 && !same_text {
                ctx.request_repaint_after(std::time::Duration::from_secs_f64(left));
                return &[];
            }
            self.checked = Some((key, mastik_vm::check(doc, page, text)));
        }
        self.checked.as_ref().map_or(&[], |(_, p)| p.as_slice())
    }

    /// The problems in `problems` that are marked, by index: a name's problem
    /// under the caret is not (the word may be mid-typing); a syntax error is.
    fn shown(problems: &[Problem], caret: Option<usize>) -> Vec<usize> {
        (0..problems.len())
            .filter(|&i| {
                let p = &problems[i];
                p.syntax || !caret.is_some_and(|c| p.span.start <= c && c <= p.span.end)
            })
            .collect()
    }

    /// The marks `ui::code_editor` draws for `problems`, with a failed run's
    /// line `bad` (from 0) in the error ink.
    pub(crate) fn marks(
        problems: &[Problem],
        caret: Option<usize>,
        bad: Option<usize>,
    ) -> Vec<ui::CodeMark> {
        let mut out: Vec<ui::CodeMark> = Self::shown(problems, caret)
            .into_iter()
            .map(|i| {
                let p = &problems[i];
                ui::CodeMark {
                    line: p.line.saturating_sub(1) as usize,
                    span: Some(p.span.clone()),
                    ink: ink(p.severity),
                }
            })
            .collect();
        if let Some(line) = bad {
            out.push(ui::CodeMark {
                line,
                span: None,
                ink: color::error(),
            });
        }
        out
    }

    /// Draw the help over the pane and answer what it asks for. Called inside the
    /// code's scroll area, after `ui::code_editor` laid `text` out as `out`;
    /// `view` is the code pane's rect, `list` the completion list's when it is up.
    #[expect(
        clippy::too_many_arguments,
        reason = "the help reads the laid-out pane, the document and the page; bundling \
                  them would only move the list"
    )]
    pub(crate) fn pane(
        &mut self,
        ui: &mut egui::Ui,
        window: egui::Id,
        out: &egui::text_edit::TextEditOutput,
        text: &str,
        view: egui::Rect,
        list: Option<egui::Rect>,
        doc: &Document,
        res: &Resolved,
        page: NodeId,
        problems: &[Problem],
    ) -> Option<HelpAct> {
        let ctx = ui.ctx().clone();
        let now = ctx.input(|i| i.time);
        let focused = out.response.has_focus();
        let caret = out
            .state
            .cursor
            .char_range()
            .map(|r| byte_of(text, r.primary.index.0));
        let mut act = None;
        self.lit_layer = None;

        if let Some(b) = self.reveal.take() {
            let line = line_rect(out, text, b.min(text.len()), view);
            if !view.contains_rect(line) {
                let third = view.height() / 3.0;
                ui.scroll_to_rect(line.with_min_y(line.top() - third), Some(egui::Align::Min));
            }
        }
        if let Some((b, at)) = self.flash {
            let t = ((now - at) / FLASH_FOR) as f32;
            if t >= 1.0 || b > text.len() {
                self.flash = None;
            } else {
                let alpha = (f32::from(WASH_ALPHA) * (1.0 - t)) as u8;
                ui.painter().rect_filled(
                    line_rect(out, text, b, view),
                    0.0,
                    color::accent_a(alpha),
                );
                ctx.request_repaint();
            }
        }

        // F12, from the caret, for the keyboard.
        if focused
            && let Some(c) = caret
            && ctx.input_mut(|i| i.consume_key(egui::Modifiers::NONE, egui::Key::F12))
            && let Some((_, decl)) = mastik_vm::declaration_at(text, c)
        {
            act = Some(self.jump(&ctx, decl));
        }

        let pointer = ctx.pointer_hover_pos().filter(|p| view.contains(*p));
        let under = pointer.and_then(|p| byte_under(out, text, p));
        let shown = Self::shown(problems, caret);
        // A problem under the pointer: its token, or its line's number in the gutter.
        let mut on_problem = under.and_then(|b| {
            shown.iter().copied().find(|&i| {
                problems[i].span.start <= b
                    && b < problems[i].span.end.max(problems[i].span.start + 1)
            })
        });
        if on_problem.is_none()
            && let Some(p) = pointer
            && p.x < out.galley_pos.x
        {
            let c = out
                .galley
                .cursor_from_pos(egui::vec2(0.0, p.y - out.galley_pos.y))
                .index
                .0;
            let line = line_of(text, byte_of(text, c)) as u32 + 1;
            on_problem = shown.iter().copied().find(|&i| problems[i].line == line);
        }
        // The card already up stays while the pointer is on it.
        let over_card = self.problem_card.filter(|(i, r)| {
            *i < problems.len()
                && pointer
                    .or(ctx.pointer_hover_pos())
                    .is_some_and(|p| r.contains(p))
        });
        let ctrl = ctx.input(|i| i.modifiers.command);

        // Ctrl over a use of one of the script's own names: a dotted underline
        // and the arrow in place of the text's I-beam, so it reads as a thing
        // to click (the chrome never shows the hand). A declaration itself
        // leads nowhere, so it stays plain.
        if ctrl
            && let Some(b) = under
            && let Some((name, decl)) = mastik_vm::declaration_at(text, b)
            && name != decl
        {
            let r = span_rect(out, text, &name);
            let y = r.bottom() - 1.0;
            ui.painter().add(egui::Shape::dashed_line(
                &[egui::pos2(r.left(), y), egui::pos2(r.right(), y)],
                egui::Stroke::new(1.0, color::accent_300()),
                2.0,
                2.0,
            ));
            ctx.set_cursor_icon(egui::CursorIcon::Default);
            if ctx.input(|i| i.pointer.primary_clicked()) {
                act = Some(self.jump(&ctx, decl));
            }
        }

        let key = match (over_card, on_problem, under) {
            (Some((i, _)), _, _) | (None, Some(i), _) => Some((true, i)),
            (None, None, Some(b)) => Some((false, word_start(text, b))),
            _ => None,
        };
        let rested = self.resting.rested(&ctx, key) || over_card.is_some();
        let mut problem_up = false;
        if rested
            && let Some((true, i)) = key
            && let Some(p) = problems.get(i)
        {
            let anchor = span_rect(out, text, &p.span);
            let (card, chosen) = problem_card(&ctx, window.with("problem"), p, anchor);
            self.problem_card = Some((i, card));
            problem_up = true;
            if chosen && let Some((range, with)) = p.fix.clone() {
                act = Some(HelpAct::Replace(range, with));
                self.problem_card = None;
            }
        } else {
            self.problem_card = None;
        }

        // A name's card, unless the list is up (typing beats pointing) or Ctrl
        // is held (the name is a link then).
        if !problem_up
            && list.is_none()
            && !ctrl
            && rested
            && let Some((false, _)) = key
            && let Some(b) = under
            && let Some(h) = mastik_vm::hover_at(doc, page, text, b)
        {
            let word = span_rect(out, text, &h.range);
            ui.painter()
                .rect_filled(word, 2.0, color::accent_a(WASH_ALPHA));
            let layer = h.layer.and_then(|id| doc.get(id).map(|n| (id, n)));
            self.lit_layer = layer.map(|(id, _)| id);
            let layer_doc = layer.map(|(id, n)| {
                let page_name = doc.get(page).map_or("", |p| p.name());
                let size = res
                    .world_bounds(id)
                    .map(|b| format!(", {} × {}", dimension(b.width()), dimension(b.height())))
                    .unwrap_or_default();
                (
                    crate::panels::layers::node_glyph(n).0,
                    format!(
                        "{} on {page_name}{size} · outlined on the canvas",
                        n.kind().default_name()
                    ),
                )
            });
            hover_card(&ctx, window.with("hover"), &h, layer_doc, word);
        }

        // The hint, over the call's line, while the code has the focus. As in
        // other code editors it opens on typing, never on a click into a call:
        // typing the call's `(`, a `,` or a `{` opens it, and it stays while
        // the caret is in that call.
        let sig = caret.and_then(|c| mastik_vm::signature_at(text, c));
        let typed = std::mem::take(&mut self.typed)
            && caret.is_some_and(|c| text[..c].trim_end().ends_with(['(', ',', '{']));
        if (typed || std::mem::take(&mut self.asked))
            && let Some(s) = &sig
        {
            self.hint_call = Some(s.open);
        }
        if sig.as_ref().map(|s| s.open) != self.hint_call || !focused {
            self.hint_call = None;
        }
        if !problem_up
            && let Some(c) = caret
            && let Some(sig) = sig.filter(|s| Some(s.open) == self.hint_call)
        {
            if ctx.input(|i| i.key_pressed(egui::Key::Escape)) && list.is_none() {
                self.hint_call = None;
            } else {
                let open = span_rect(out, text, &(sig.open..sig.open + 1));
                let caret_line = span_rect(out, text, &(c..c));
                if view.intersects(open) || view.intersects(caret_line) {
                    hint_card(
                        &ctx,
                        window.with("hint"),
                        &sig,
                        open,
                        caret_line,
                        view,
                        list,
                    );
                }
            }
        }
        act
    }

    /// Put the caret on `decl` (selected), wash its line and bring it into view.
    fn jump(&mut self, ctx: &egui::Context, decl: Range<usize>) -> HelpAct {
        self.flash = Some((decl.start, ctx.input(|i| i.time)));
        self.reveal = Some(decl.start);
        HelpAct::Select(decl)
    }
}

/// A layer's width or height: whole when it is, else two decimals.
fn dimension(n: f64) -> String {
    let r = n.round();
    if (n - r).abs() < 0.005 {
        format!("{r}")
    } else {
        format!("{n:.2}")
    }
}

/// The start of the name holding byte `b`, or `b` itself.
fn word_start(text: &str, b: usize) -> usize {
    let bytes = text.as_bytes();
    let mut s = b;
    while s > 0 && (bytes[s - 1].is_ascii_alphanumeric() || bytes[s - 1] == b'_') {
        s -= 1;
    }
    s
}

/// A problem's ink: certain in the error ink, likely in amber.
pub(crate) fn ink(severity: Severity) -> egui::Color32 {
    match severity {
        Severity::Error => color::error(),
        Severity::Warning => color::warn(),
    }
}

/// One line of a card: `text` in `pt` and `ink`, wrapped at `w`.
fn line_job(text: &str, pt: f32, ink: egui::Color32, w: f32, mono: bool) -> egui::text::LayoutJob {
    let font = if mono {
        egui::FontId::monospace(pt)
    } else {
        egui::FontId::proportional(pt)
    };
    let mut job = egui::text::LayoutJob::simple(text.to_owned(), font, ink, w);
    job.halign = egui::Align::LEFT;
    job
}

/// The hint's first line: the signature, the lit parameter underlined in the accent.
fn signature_job(sig: &Signature) -> egui::text::LayoutJob {
    let font = egui::FontId::monospace(ui::CODE_PT);
    let fmt = |ink: egui::Color32| egui::TextFormat::simple(font.clone(), ink);
    let mut job = egui::text::LayoutJob::default();
    job.append(&sig.name, 0.0, fmt(color::text()));
    job.append(
        if sig.named { " { " } else { "(" },
        0.0,
        fmt(theme::text::muted()),
    );
    for (i, p) in sig.params.iter().enumerate() {
        if i > 0 {
            job.append(", ", 0.0, fmt(theme::text::muted()));
        }
        let lit = sig.active == Some(i);
        let mut name = fmt(if lit {
            color::accent_200()
        } else {
            color::text()
        });
        if lit {
            name.underline = egui::Stroke::new(1.0, color::accent_300());
        }
        job.append(&p.name, 0.0, name);
        if let Some(ty) = &p.ty {
            job.append(&format!(": {ty}"), 0.0, fmt(theme::text::faint()));
        }
    }
    job.append(
        if sig.named { " }" } else { ")" },
        0.0,
        fmt(theme::text::muted()),
    );
    job.wrap.max_width = f32::INFINITY;
    job
}

/// Lay out a card's lines at a shared width: the first unwrapped (and setting the
/// width, within [`CARD_MIN_W`]–[`CARD_MAX_W`]), the rest wrapped to it.
fn lay_out(
    ctx: &egui::Context,
    first: egui::text::LayoutJob,
    rest: Vec<(String, f32, egui::Color32)>,
) -> (f32, Vec<std::sync::Arc<egui::Galley>>) {
    let head = ctx.fonts_mut(|f| f.layout_job(first));
    let w = head.size().x.clamp(CARD_MIN_W, CARD_MAX_W);
    let mut out = vec![head];
    for (text, pt, ink) in rest {
        out.push(ctx.fonts_mut(|f| f.layout_job(line_job(&text, pt, ink, w, false))));
    }
    (w, out)
}

/// The size a card of `galleys` at text width `w` takes, its frame included.
fn card_size(w: f32, galleys: &[std::sync::Arc<egui::Galley>]) -> egui::Vec2 {
    let h: f32 = galleys.iter().map(|g| g.size().y).sum::<f32>()
        + LINE_GAP * galleys.len().saturating_sub(1) as f32;
    egui::vec2(w, h) + egui::Vec2::splat(2.0 * (ui::POPOVER_PAD + ui::MENU_BORDER))
}

/// Draw `galleys` stacked on a dropdown's ground at `at`, taking no pointer.
fn show_card(
    ctx: &egui::Context,
    id: egui::Id,
    at: egui::Pos2,
    w: f32,
    galleys: Vec<std::sync::Arc<egui::Galley>>,
) {
    egui::Area::new(id)
        .order(egui::Order::Foreground)
        .interactable(false)
        .constrain(false)
        .fixed_pos(at)
        .show(ctx, |ui| {
            ui::menu_frame(ui::POPOVER_PAD).show(ui, |ui| {
                ui.set_width(w);
                ui.spacing_mut().item_spacing = egui::vec2(0.0, LINE_GAP);
                for g in galleys {
                    let (rect, _) = ui.allocate_exact_size(g.size(), egui::Sense::hover());
                    ui.painter().galley(rect.min, g, color::text());
                }
            });
        });
}

/// The parameter hint: hung [`ui::MENU_GAP`] over the line the call opens on
/// (`open`), left edges together; with no room above, under the completion
/// list (`list`) or else under the caret's line.
fn hint_card(
    ctx: &egui::Context,
    id: egui::Id,
    sig: &Signature,
    open: egui::Rect,
    caret_line: egui::Rect,
    view: egui::Rect,
    list: Option<egui::Rect>,
) {
    let mut rest = Vec::new();
    if let Some(d) = &sig.detail {
        rest.push((d.clone(), ui::SMALL_PT, color::text()));
    }
    if let Some(d) = &sig.doc {
        rest.push((d.clone(), ui::SMALL_PT, theme::text::dim()));
    }
    if let Some(n) = &sig.note {
        rest.push((n.clone(), ui::CAPTION_PT, theme::text::faint()));
    }
    let (w, galleys) = lay_out(ctx, signature_job(sig), rest);
    let size = card_size(w, &galleys);
    let inset = ui::POPOVER_PAD + ui::MENU_BORDER;
    let x = open.left() - inset;
    let above = open.top() - ui::MENU_GAP - size.y;
    let y = if above >= view.top() {
        above
    } else {
        list.unwrap_or(caret_line).bottom() + ui::MENU_GAP
    };
    let at = ui::menu_place(ctx, egui::pos2(x, y), size);
    show_card(ctx, id, at, w, galleys);
}

/// A name's hover card, under the name (`word`): the name and its type, its
/// doc, and what kind of name it is in the faint ink. A layer's card wears the
/// layer's kind glyph and says where it is and how big (`layer`).
fn hover_card(
    ctx: &egui::Context,
    id: egui::Id,
    h: &Hover,
    layer: Option<(&'static str, String)>,
    word: egui::Rect,
) {
    let font = egui::FontId::monospace(ui::CODE_PT);
    let mut first = egui::text::LayoutJob::default();
    if let Some((glyph, _)) = &layer {
        first.append(
            &format!("{glyph} "),
            0.0,
            egui::TextFormat::simple(theme::icon_font(ui::CODE_PT), theme::text::muted()),
        );
    }
    first.append(
        &h.title,
        0.0,
        egui::TextFormat::simple(font.clone(), color::text()),
    );
    if let Some(ty) = &h.ty {
        first.append(
            &format!(": {ty}"),
            0.0,
            egui::TextFormat::simple(font, theme::text::faint()),
        );
    }
    first.wrap.max_width = f32::INFINITY;
    let mut rest = Vec::new();
    if let Some(d) = h
        .doc
        .clone()
        .or_else(|| layer.as_ref().map(|(_, d)| d.clone()))
    {
        rest.push((d, ui::SMALL_PT, color::text()));
    }
    rest.push((h.kind.clone(), ui::CAPTION_PT, theme::text::faint()));
    let (w, galleys) = lay_out(ctx, first, rest);
    let size = card_size(w, &galleys);
    let at = ui::dropdown_place(ctx, word, size, ui::MENU_GAP);
    show_card(ctx, id, at, w, galleys);
}

/// A problem's card under its token (`anchor`): the message in its ink after
/// the warning glyph, then the run's fix as a button and the note. It takes the
/// pointer, so the fix can be clicked; answers its rect and whether it was.
fn problem_card(
    ctx: &egui::Context,
    id: egui::Id,
    p: &Problem,
    anchor: egui::Rect,
) -> (egui::Rect, bool) {
    let ink = ink(p.severity);
    let mut first = egui::text::LayoutJob::default();
    first.append(
        icon::WARNING,
        0.0,
        egui::TextFormat::simple(theme::icon_font(ui::SMALL_PT + 1.0), ink),
    );
    first.append(
        &format!(" {}", p.message),
        0.0,
        egui::TextFormat::simple(egui::FontId::proportional(ui::SMALL_PT), color::text()),
    );
    first.wrap.max_width = CARD_MAX_W;
    let head = ctx.fonts_mut(|f| f.layout_job(first));
    let fix = p.fix.as_ref().map(|(_, w)| format!("Change to {w}"));
    let button_w = |t: &str| {
        ctx.fonts_mut(|f| {
            f.layout_no_wrap(
                t.to_owned(),
                egui::FontId::proportional(ui::CONTROL_PT),
                color::text(),
            )
        })
        .size()
        .x + 2.0 * ui::FIELD_PAD_X
    };
    let note = p.note.clone();
    let has_row = fix.is_some() || note.is_some();
    let w = head.size().x.clamp(CARD_MIN_W, CARD_MAX_W);
    let row_h = if has_row {
        ui::CONTROL_H + LINE_GAP
    } else {
        0.0
    };
    let note_g = note.as_ref().map(|n| {
        let left = w - fix.as_deref().map_or(0.0, |f| button_w(f) + 8.0);
        ctx.fonts_mut(|f| {
            f.layout_job(line_job(
                n,
                ui::CAPTION_PT,
                theme::text::faint(),
                left.max(80.0),
                false,
            ))
        })
    });
    let size = egui::vec2(w, head.size().y + row_h)
        + egui::Vec2::splat(2.0 * (ui::POPOVER_PAD + ui::MENU_BORDER));
    let at = ui::dropdown_place(ctx, anchor, size, ui::MENU_GAP);
    let mut chosen = false;
    let shown = egui::Area::new(id)
        .order(egui::Order::Foreground)
        .constrain(false)
        .fixed_pos(at)
        .show(ctx, |ui| {
            ui::menu_frame(ui::POPOVER_PAD).show(ui, |ui| {
                ui.set_width(w);
                ui.spacing_mut().item_spacing = egui::vec2(8.0, LINE_GAP);
                let (rect, _) = ui.allocate_exact_size(head.size(), egui::Sense::hover());
                ui.painter().galley(rect.min, head, color::text());
                if has_row {
                    ui.horizontal(|ui| {
                        if let Some(f) = &fix
                            && ui::label_button(
                                ui,
                                f,
                                egui::vec2(button_w(f), ui::CONTROL_H),
                                ui::FieldButton::Off,
                            )
                            .clicked()
                        {
                            chosen = true;
                        }
                        if let Some(g) = note_g {
                            let (rect, _) = ui.allocate_exact_size(
                                egui::vec2(g.size().x, ui::CONTROL_H),
                                egui::Sense::hover(),
                            );
                            ui.painter().galley(
                                egui::pos2(rect.left(), rect.center().y - g.size().y / 2.0),
                                g,
                                theme::text::faint(),
                            );
                        }
                    });
                }
            });
        });
    (shown.response.rect, chosen)
}

/// The head's problem counts: errors then warnings, each only when not zero,
/// the warning glyph in its ink and the count in the caption size, on an icon
/// button's hover ground. Answers its response, clicked to walk to the next
/// problem, or `None` when there are none.
pub(crate) fn counts(ui: &mut egui::Ui, problems: &[Problem]) -> Option<egui::Response> {
    let errors = problems
        .iter()
        .filter(|p| p.severity == Severity::Error)
        .count();
    let warnings = problems.len() - errors;
    if problems.is_empty() {
        return None;
    }
    let mut job = egui::text::LayoutJob::default();
    let glyph = theme::icon_font(ui::CAPTION_PT + 2.0);
    let num = egui::FontId::proportional(ui::CAPTION_PT);
    for (n, sev) in [(errors, Severity::Error), (warnings, Severity::Warning)] {
        if n == 0 {
            continue;
        }
        if !job.text.is_empty() {
            job.append(
                "  ",
                0.0,
                egui::TextFormat::simple(num.clone(), theme::text::faint()),
            );
        }
        job.append(
            icon::WARNING,
            0.0,
            egui::TextFormat::simple(glyph.clone(), ink(sev)),
        );
        job.append(
            &format!(" {n}"),
            0.0,
            egui::TextFormat::simple(num.clone(), theme::text::muted()),
        );
    }
    let g = ui.fonts_mut(|f| f.layout_job(job));
    const PAD: f32 = 6.0;
    let (rect, resp) = ui.allocate_exact_size(
        egui::vec2(g.size().x + 2.0 * PAD, ui::CONTROL_H),
        egui::Sense::click(),
    );
    if resp.hovered() {
        ui.painter()
            .rect_filled(rect, egui::CornerRadius::same(5), theme::color::hover());
    }
    ui.painter().galley(
        egui::pos2(rect.left() + PAD, rect.center().y - g.size().y / 2.0),
        g,
        theme::text::muted(),
    );
    let plural = |n: usize, one: &str| {
        if n == 1 {
            format!("1 {one}")
        } else {
            format!("{n} {one}s")
        }
    };
    let said = match (errors, warnings) {
        (0, w) => plural(w, "warning"),
        (e, 0) => plural(e, "error"),
        (e, w) => format!("{}, {}", plural(e, "error"), plural(w, "warning")),
    };
    Some(resp.on_hover_text(format!("{said} · click goes to the next")))
}

/// The next problem's span after byte `caret`, wrapping to the first.
pub(crate) fn next_problem(problems: &[Problem], caret: usize) -> Option<Range<usize>> {
    problems
        .iter()
        .find(|p| p.span.start > caret)
        .or_else(|| problems.first())
        .map(|p| p.span.clone())
}

/// *Run*'s reason when a syntax error holds it.
pub(crate) fn held_reason(p: &Problem) -> String {
    held_reason_at(p.line)
}

/// *Run*'s reason when a syntax error on `line` holds it.
pub(crate) fn held_reason_at(line: u32) -> String {
    format!("Fix the syntax error on line {line} to run")
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The head's counts draw the warning glyph in the icon font, where the
    /// text font has no such character and shows a box.
    /// flip: `counts` laying the glyph out in `FontId::proportional` → fails at
    /// "in the icon font".
    #[test]
    fn the_counts_glyph_is_drawn_in_the_icon_font() {
        let ctx = egui::Context::default();
        crate::theme::install(&ctx);
        let _ = ctx.run_ui(Default::default(), |_| {});
        let page = mastik_core::IdSource::new(1).mint();
        let doc = mastik_core::Document::new(page);
        let problems = mastik_vm::check(&doc, page, "print(nme)");
        assert_eq!(problems.len(), 1, "fixture: one warning");
        let out = ctx.run_ui(Default::default(), |ui| {
            counts(ui, &problems);
        });
        let family = theme::icon_font(1.0).family;
        let glyph_family = out.shapes.iter().find_map(|c| match &c.shape {
            egui::Shape::Text(t) => t
                .galley
                .job
                .sections
                .iter()
                .find(|s| {
                    let r = s.byte_range.start.0..s.byte_range.end.0;
                    t.galley.job.text.get(r) == Some(icon::WARNING)
                })
                .map(|s| s.format.font_id.family.clone()),
            _ => None,
        });
        assert_eq!(glyph_family, Some(family), "in the icon font");
    }
}
