//! The prototype player in the app: the play engine's shell
//! (`docs/design/prototyping-player.md`). A [`Playing`] holds a `mastik_play::Player` over a copy of the document
//! it was started from, follows that document's commits, and shows in one of
//! three places: a card floating over the canvas, a window of its own, or the
//! main window filled (present mode). Every place draws the same body: the
//! screen, the hotspot hints and the notices, over a bar of the player's
//! commands.

use crate::canvas::CanvasRenderer;
use crate::tabs::TabId;
use crate::theme::{self, color, icon};
use crate::ui;
use eframe::egui;
use mastik_core::kurbo::{Point, Size, Vec2};
use mastik_core::prototype::{Action, FlowRef, Scaling};
use mastik_core::{Document, NodeId, Value, VarKind};
use mastik_play::{Input, Notice, Player, Request, Scene, Start, Take};

/// Where the player shows.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Place {
    /// A card floating over the canvas, the editor usable around it.
    Over,
    /// A window of its own.
    Detached,
    /// The main window filled, the editor hidden: present mode.
    Fullscreen,
}

/// How long a notice stays in the player's corner, in seconds.
const NOTICE_HOLD: f64 = 4.0;
/// The most room a screen takes over the canvas, as a share of the canvas.
const OVER_SHARE: f32 = 0.7;
/// The narrowest the card over the canvas gets, so its bar fits.
const OVER_MIN_W: f32 = 260.0;
/// The detached window's id.
const WINDOW_ID: &str = "prototype-player-window";
/// The card's id over the canvas.
const CARD_ID: &str = "prototype-player";

/// Play asked for and held while its scripts are shown first.
#[derive(Clone)]
pub(crate) struct Owed {
    start: Start,
    place: Place,
    /// Present mode asked.
    from_present: bool,
    /// Each script's name and source.
    scripts: Vec<(String, String)>,
}

/// A prototype being played.
pub(crate) struct Playing {
    pub(crate) player: Player,
    /// Where it started, for *Restart* after the document was replaced.
    start: Start,
    /// The tab whose document is played; `None` for a document no tab shows.
    tab: Option<TabId>,
    pub(crate) place: Place,
    /// Started by present mode: Escape and leaving fullscreen end play.
    from_present: bool,
    /// Whether the window was fullscreen before play filled it.
    was_fullscreen: bool,
    /// The detached window is fullscreen.
    detached_full: bool,
    /// The player's own renderer and texture; `None` headless.
    canvas: Option<CanvasRenderer>,
    texture: Option<egui::TextureId>,
    /// What the texture holds: the scene and its pixel size.
    drawn: Option<(Scene, (u32, u32))>,
    /// egui's clock at the last tick.
    last_time: Option<f64>,
    /// The hotspot hints are shown.
    pub(crate) hints: bool,
    /// The state card is open.
    pub(crate) card: bool,
    /// The player holds the keyboard (a press landed in it).
    pub(crate) focused: bool,
    /// A press began in the player and has not been released.
    pressing: bool,
    /// The pointer as the player last heard it, in screen space.
    pointer: Option<Point>,
    /// Notices to show, each with when it came.
    notices: Vec<(String, f64)>,
    /// The take the record button last finished.
    pub(crate) take: Option<Take>,
    /// The state card's fields being typed in, by variable.
    drafts: Vec<(mastik_core::VarId, String)>,
}

/// Where play starts on `page` of `doc`: the page's first flow, else the first
/// flow on any page, else `frame` when given, else the page's first top-level
/// frame in reading order. `None` when the document has no frame to play.
pub(crate) fn start_for(doc: &Document, page: NodeId, frame: Option<NodeId>) -> Option<Start> {
    let first_flow = |p: NodeId| {
        doc.page(p)
            .and_then(|info| info.flows.first().map(|f| f.id))
            .map(|flow| Start::Flow(FlowRef { page: p, flow }))
    };
    if let Some(s) = first_flow(page) {
        return Some(s);
    }
    if let Some(s) = doc.pages().iter().find_map(|p| first_flow(p.id)) {
        return Some(s);
    }
    if let Some(f) = frame {
        return Some(Start::Frame(f));
    }
    let frames: Vec<_> = doc
        .get(page)?
        .children()
        .iter()
        .filter(|id| {
            doc.get(**id)
                .is_some_and(|n| matches!(n.kind(), mastik_core::NodeKind::Artboard { .. }))
        })
        .filter_map(|id| Some((*id, doc_bounds(doc, *id)?)))
        .collect();
    crate::present::reading_order(frames)
        .first()
        .map(|f| Start::Frame(*f))
}

fn doc_bounds(doc: &Document, id: NodeId) -> Option<mastik_core::kurbo::Rect> {
    let n = doc.get(id)?;
    let size = n.kind().box_size()?;
    Some(n.transform().transform_rect_bbox(size.to_rect()))
}

/// Whether `doc` has any flow, on any page: present mode plays when it does.
pub(crate) fn has_flows(doc: &Document) -> bool {
    doc.pages().iter().any(|p| !p.flows.is_empty())
}

/// The document scripts *Run script* names anywhere in `doc`, each once.
pub(crate) fn scripts_played(doc: &Document) -> Vec<mastik_core::ScriptId> {
    fn walk(list: &[mastik_core::Keyed<Action>], out: &mut Vec<mastik_core::ScriptId>) {
        for k in list {
            if let Action::RunScript { script } = &k.value
                && !out.contains(script)
            {
                out.push(*script);
            }
            let nested: Vec<_> = k.value.nested().cloned().collect();
            walk(&nested, out);
        }
    }
    let mut out = Vec::new();
    for id in doc.node_ids() {
        let Some(n) = doc.get(id) else { continue };
        for r in n.reactions() {
            walk(&r.value.actions, &mut out);
        }
    }
    out
}

/// A number as the state card writes it: whole without a point.
fn number_text(n: f64) -> String {
    format!("{n}")
}

/// The scale, in points per screen unit, that shows a `screen` in `room` by
/// `scaling`: *Fit* whole, *Fill width* across, *Actual size* one to one — each
/// no larger than the room allows across, so the bar never runs off.
pub(crate) fn fit_scale(screen: Size, room: egui::Vec2, scaling: Scaling) -> f32 {
    let (w, h) = (screen.width.max(1.0) as f32, screen.height.max(1.0) as f32);
    let fit = (room.x / w).min(room.y / h);
    match scaling {
        Scaling::Fit => fit,
        Scaling::FillWidth => room.x / w,
        Scaling::ActualSize => 1.0f32.min(room.x / w),
    }
    .max(0.01)
}

/// `pos` on the screen drawn in `rect`, at `scale` points per unit, in screen space.
pub(crate) fn to_screen(pos: egui::Pos2, rect: egui::Rect, scale: f32) -> Point {
    let v = (pos - rect.min) / scale;
    Point::new(f64::from(v.x), f64::from(v.y))
}

/// What a notice says, in the app's words.
pub(crate) fn notice_text(n: &Notice) -> String {
    use mastik_play::Fallback;
    match n {
        Notice::Missing { .. } => "An action names something not there; it was skipped".into(),
        Notice::Expression { error, .. } => format!("An expression doesn't read: {error}"),
        Notice::Script { name, message } => format!("{name} failed: {message}"),
        Notice::ScriptNeedsInput { name } => {
            format!("{name} asks for input, which play can't give")
        }
        Notice::ScriptsOff => "Scripts don't run in this player".into(),
        Notice::Fallback(Fallback::Screen) => {
            "The frame playing was deleted; back to the start".into()
        }
        Notice::Fallback(Fallback::Overlay) => "The overlay showing was deleted".into(),
        Notice::Fallback(Fallback::Variable) => "A variable playing was deleted".into(),
        Notice::Loop => "Reactions set each other off too deep; stopped there".into(),
    }
}

/// An egui key as the player names it, for a key trigger: the same name the
/// prototype mode records.
fn key_name(key: egui::Key) -> String {
    key.name().to_string()
}

impl Playing {
    fn new(player: Player, start: Start, tab: Option<TabId>, place: Place) -> Self {
        Playing {
            player,
            start,
            tab,
            place,
            from_present: false,
            was_fullscreen: false,
            detached_full: false,
            canvas: None,
            texture: None,
            drawn: None,
            last_time: None,
            hints: false,
            card: false,
            focused: true,
            pressing: false,
            pointer: None,
            notices: Vec::new(),
            take: None,
            drafts: Vec::new(),
        }
    }

    /// Advance the engine to `now` (egui's clock) and gather what it says.
    fn tick(&mut self, ctx: &egui::Context, now: f64) {
        let dt = self
            .last_time
            .map_or(0.0, |t| ((now - t) * 1000.0).max(0.0));
        self.last_time = Some(now);
        if self.player.is_moving() || self.player.is_recording() {
            self.player.tick(dt);
        }
        for n in self.player.take_notices() {
            let text = notice_text(&n);
            if !self.notices.iter().any(|(t, _)| *t == text) {
                self.notices.push((text, now));
            }
        }
        self.notices.retain(|(_, at)| now - at < NOTICE_HOLD);
        for r in self.player.take_requests() {
            match r {
                Request::OpenUrl(url) => ctx.open_url(egui::OpenUrl::new_tab(url)),
            }
        }
        if self.player.is_moving() || self.player.is_recording() || !self.notices.is_empty() {
            ctx.request_repaint();
        }
    }

    /// Feed `ui`'s pointer over `rect` (the screen drawn at `scale`) to the
    /// player. Answers whether the pointer is over it.
    fn drive_pointer(&mut self, ui: &egui::Ui, rect: egui::Rect, scale: f32) -> bool {
        let id = ui.id().with("player-screen");
        let resp = ui.interact(rect, id, egui::Sense::click_and_drag());
        let hovered = resp.hovered() || self.pressing;
        let (pos, pressed, released, wheel) = ui.input(|i| {
            (
                i.pointer.interact_pos(),
                i.pointer.primary_pressed(),
                i.pointer.primary_released(),
                i.smooth_scroll_delta,
            )
        });
        let at = pos.map(|p| to_screen(p, rect, scale));
        if hovered && let Some(at) = at {
            if self.pointer != Some(at) {
                self.player.input(Input::PointerMove(at));
                self.pointer = Some(at);
            }
            if pressed && resp.hovered() {
                self.player.input(Input::PointerDown(at));
                self.pressing = true;
                self.focused = true;
            }
            if wheel != egui::Vec2::ZERO && resp.hovered() {
                let delta = Vec2::new(f64::from(-wheel.x), f64::from(-wheel.y)) / f64::from(scale);
                self.player.input(Input::Wheel { at, delta });
            }
        } else if self.pointer.take().is_some() {
            self.player.input(Input::PointerLeave);
        }
        if released && self.pressing {
            self.pressing = false;
            let at = at.or(self.pointer).unwrap_or_default();
            self.player.input(Input::PointerUp(at));
        }
        hovered
    }

    /// Take this pass's plain and Shift-ed keys and text for the player, leaving
    /// chords with Ctrl or Alt for the editor. An Escape the prototype does not
    /// use hands the keyboard back; answers whether one did.
    fn take_keys(&mut self, ctx: &egui::Context) -> bool {
        let mut gave_back = false;
        let player = &mut self.player;
        ctx.input_mut(|i| {
            i.events.retain(|e| match e {
                egui::Event::Key {
                    key,
                    pressed: true,
                    modifiers,
                    ..
                } if !modifiers.ctrl && !modifiers.alt && !modifiers.command => {
                    let used = player.input(Input::Key {
                        key: key_name(*key),
                        shift: modifiers.shift,
                    });
                    if *key == egui::Key::Escape && !used {
                        gave_back = true;
                    }
                    false
                }
                egui::Event::Key {
                    pressed: false,
                    modifiers,
                    ..
                } if !modifiers.ctrl && !modifiers.alt && !modifiers.command => false,
                egui::Event::Text(t) => {
                    player.input(Input::Text(t.clone()));
                    false
                }
                _ => true,
            });
        });
        if gave_back {
            self.focused = false;
        }
        gave_back
    }

    /// Draw the screen into `rect` at `pixels` device pixels, through the
    /// player's own renderer when anything moved since the last draw.
    fn paint_screen(&mut self, ui: &egui::Ui, rect: egui::Rect, scale: f32, ground: egui::Color32) {
        let scene = self.player.scene();
        let ppp = ui.ctx().pixels_per_point();
        let px = (
            ((rect.width() * ppp).round() as u32).max(1),
            ((rect.height() * ppp).round() as u32).max(1),
        );
        let moved = !self.player.take_dirty().is_empty();
        let stale = moved
            || self.texture.is_none()
            || self
                .drawn
                .as_ref()
                .is_none_or(|(s, p)| *s != scene || *p != px);
        if stale && let Some(canvas) = self.canvas.as_mut() {
            let backdrop = crate::session::page_ground(self.player.document(), scene.page);
            self.texture = canvas.render_play(
                self.player.document(),
                self.player.resolved(),
                &scene,
                px,
                backdrop,
            );
            self.drawn = Some((scene.clone(), px));
        }
        let painter = ui.painter_at(rect);
        painter.rect_filled(rect, 0.0, ground);
        if let Some(tex) = self.texture {
            let uv = egui::Rect::from_min_max(egui::pos2(0.0, 0.0), egui::pos2(1.0, 1.0));
            painter.image(tex, rect, uv, egui::Color32::WHITE);
        }
        // The hints: every place that takes an act, outlined, while switched on or
        // flashing after a click on nothing.
        let strength = match (self.hints, scene.hint_flash) {
            (true, _) => 1.0,
            (false, Some(f)) => f,
            (false, None) => 0.0,
        };
        if strength > 0.0 {
            let a = (strength * 255.0) as u8;
            for h in self.player.hotspots() {
                let r = egui::Rect::from_min_max(
                    rect.min + egui::vec2(h.rect.x0 as f32, h.rect.y0 as f32) * scale,
                    rect.min + egui::vec2(h.rect.x1 as f32, h.rect.y1 as f32) * scale,
                );
                painter.rect_filled(r, 2.0, color::accent_a(a / 5));
                painter.rect_stroke(
                    r,
                    2.0,
                    egui::Stroke::new(1.0, color::accent_a(a)),
                    egui::StrokeKind::Inside,
                );
            }
        }
        // The notices, newest last, in the screen's top-left corner.
        let mut y = rect.top() + 8.0;
        for (text, _) in &self.notices {
            let galley = painter.layout(
                text.clone(),
                egui::FontId::proportional(ui::SMALL_PT),
                theme::text::strong(),
                (rect.width() - 32.0).max(40.0),
            );
            let bg = egui::Rect::from_min_size(
                egui::pos2(rect.left() + 8.0, y),
                galley.size() + egui::vec2(16.0, 10.0),
            );
            painter.rect_filled(bg, 4.0, color::card());
            painter.galley(bg.min + egui::vec2(8.0, 5.0), galley, theme::text::strong());
            y = bg.bottom() + 6.0;
        }
    }
}

/// What a click on the player's bar asked for.
#[derive(Clone, Copy, PartialEq, Eq)]
enum BarAct {
    Restart,
    Hints,
    Card,
    Record,
    Detach,
    Attach,
    Fullscreen,
    Close,
    /// Write the last take as a video.
    ExportTake,
}

/// The player's bar: its commands as icon buttons, `hint`-ed as the rest of
/// the app's are. Returns the one clicked.
fn bar(ui: &mut egui::Ui, p: &Playing) -> Option<BarAct> {
    let mut act = None;
    let size = ui::CONTROL_H;
    let glyph = ui::CONTROL_GLYPH_PT;
    let mut button = |ui: &mut egui::Ui, g: &str, lit: bool, tip: &str, a: BarAct| {
        if ui::icon_button(ui, g, size, glyph, lit, true)
            .on_hover_text(tip)
            .clicked()
        {
            act = Some(a);
        }
    };
    ui.horizontal_centered(|ui| {
        ui.spacing_mut().item_spacing.x = 2.0;
        button(
            ui,
            icon::ARROW_COUNTER_CLOCKWISE,
            false,
            "Restart",
            BarAct::Restart,
        );
        button(
            ui,
            icon::HAND_POINTING,
            p.hints,
            "Show hotspots",
            BarAct::Hints,
        );
        button(
            ui,
            icon::SLIDERS_HORIZONTAL,
            p.card,
            "Variables",
            BarAct::Card,
        );
        let recording = p.player.is_recording();
        let (g, tip) = match recording {
            true => (icon::STOP, "Stop recording"),
            false => (icon::RECORD, "Record a take"),
        };
        button(ui, g, recording, tip, BarAct::Record);
        if p.take.is_some() && !recording {
            button(
                ui,
                icon::EXPORT,
                false,
                "Export the take as a video",
                BarAct::ExportTake,
            );
        }
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            ui.spacing_mut().item_spacing.x = 2.0;
            if p.place != Place::Fullscreen {
                button(ui, icon::PLAY, false, "Fullscreen", BarAct::Fullscreen);
            }
            match p.place {
                Place::Detached => button(
                    ui,
                    icon::ARROW_SQUARE_OUT,
                    true,
                    "Back over the canvas",
                    BarAct::Attach,
                ),
                _ => button(
                    ui,
                    icon::ARROW_SQUARE_OUT,
                    false,
                    "Open in a window",
                    BarAct::Detach,
                ),
            }
            if p.place == Place::Fullscreen {
                button(ui, icon::X, false, "Stop playing", BarAct::Close);
            }
        });
    });
    act
}

/// The state card's rows: each variable with its value, set by typing (a
/// number or text) or a switch (a boolean). A colour is shown, not set.
fn state_rows(ui: &mut egui::Ui, p: &mut Playing) {
    let modes = p.player.modes();
    if !modes.is_empty() {
        ui.label(ui::eyebrow("Modes"));
        for (i, row) in modes.iter().enumerate() {
            mode_row(ui, p, i, row);
        }
        ui.label(ui::eyebrow("Values"));
    }
    let rows = p.player.values();
    if rows.is_empty() {
        ui::empty_note(ui, "No variables");
        return;
    }
    for row in rows {
        let w = ui.available_width();
        ui.allocate_ui_with_layout(
            egui::vec2(w, ui::CONTROL_H),
            egui::Layout::left_to_right(egui::Align::Center),
            |ui| {
                let name =
                    egui::RichText::new(&row.name)
                        .size(ui::SMALL_PT)
                        .color(match row.played {
                            true => theme::text::strong(),
                            false => theme::text::muted(),
                        });
                ui.add_sized(
                    egui::vec2(w * 0.45, ui::CONTROL_H),
                    egui::Label::new(name).truncate(),
                );
                let field = egui::vec2(ui.available_width(), ui::CONTROL_H);
                match (&row.value, row.kind) {
                    (Value::Boolean(b), VarKind::Boolean) => {
                        if ui::switch_row(ui, "", *b, ui::CONTROL_H).clicked() {
                            let _ = p.player.set_value(row.var, Value::Boolean(!b));
                        }
                    }
                    (Value::Number(_) | Value::String(_), _) => {
                        let shown = match &row.value {
                            Value::Number(n) => number_text(*n),
                            Value::String(s) => s.clone(),
                            _ => String::new(),
                        };
                        let at = p.drafts.iter().position(|(v, _)| *v == row.var);
                        let mut text = at.map_or(shown, |i| p.drafts[i].1.clone());
                        let r = ui::text_field(ui, field, &mut text, "");
                        let enter = r.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter));
                        if enter {
                            let value = match row.kind {
                                VarKind::Number => {
                                    text.trim().parse::<f64>().ok().map(Value::Number)
                                }
                                _ => Some(Value::String(text.clone())),
                            };
                            if let Some(v) = value {
                                let _ = p.player.set_value(row.var, v);
                            }
                        }
                        p.drafts.retain(|(v, _)| *v != row.var);
                        if r.has_focus() {
                            p.drafts.push((row.var, text));
                        }
                    }
                    _ => {
                        ui.label(
                            egui::RichText::new(crate::player::value_words(&row.value))
                                .size(ui::SMALL_PT)
                                .color(theme::text::muted()),
                        );
                    }
                }
            },
        );
    }
}

/// One collection's row in the state card: its name and a dropdown of its modes.
/// A pick sets the mode on every page, as *Set mode* does by default; a screen
/// that sets its own shows that one, the dropdown dim.
fn mode_row(ui: &mut egui::Ui, p: &mut Playing, i: usize, row: &mastik_play::ModeRow) {
    let w = ui.available_width();
    let mut pick = None;
    ui.allocate_ui_with_layout(
        egui::vec2(w, ui::CONTROL_H),
        egui::Layout::left_to_right(egui::Align::Center),
        |ui| {
            let name = egui::RichText::new(&row.name)
                .size(ui::SMALL_PT)
                .color(theme::text::muted());
            ui.add_sized(
                egui::vec2(w * 0.45, ui::CONTROL_H),
                egui::Label::new(name).truncate(),
            );
            let shown = row
                .modes
                .iter()
                .find(|(m, _)| *m == row.current)
                .map_or(String::new(), |(_, n)| n.clone());
            let field = ui.available_width();
            let r = ui.add_enabled_ui(!row.screen_sets, |ui| {
                ui::fitted_combo(ui, ("play-mode", i), field, &shown, &shown, |ui| {
                    ui::menu_rows(ui);
                    for (m, n) in &row.modes {
                        let on = *m == row.current;
                        if ui.selectable_label(on, n).clicked() && !on {
                            pick = Some(*m);
                        }
                    }
                })
            });
            r.response
                .on_disabled_hover_text(format!("This screen sets its own {} mode", row.name));
        },
    );
    if let Some(m) = pick {
        p.player.input(mastik_play::Input::SetMode(m));
    }
}

/// A value the state card shows but cannot set, in words.
pub(crate) fn value_words(v: &Value) -> String {
    match v {
        Value::Color(c) => {
            let [r, g, b, a] = c.to_rgba8().to_u8_array();
            match a {
                255 => format!("#{r:02x}{g:02x}{b:02x}"),
                _ => format!("#{r:02x}{g:02x}{b:02x}{a:02x}"),
            }
        }
        Value::Number(n) => number_text(*n),
        Value::String(s) => s.clone(),
        Value::Boolean(b) => b.to_string(),
    }
}

impl crate::app::MastikApp {
    /// Play the prototype from the current page in `place`: its first flow, else
    /// the first flow anywhere, else the selected frame or the page's first.
    /// Scripts the person has not seen run here are shown first.
    pub(crate) fn play(&mut self, ctx: &egui::Context, place: Place, from_present: bool) {
        let doc = &self.session.doc;
        let page = self.session.page;
        let selected = self.session.selection.ids().iter().copied().find(|id| {
            doc.get(*id).is_some_and(|n| {
                n.parent() == Some(page)
                    && matches!(n.kind(), mastik_core::NodeKind::Artboard { .. })
            })
        });
        let Some(start) = start_for(doc, page, selected) else {
            self.session
                .info("Nothing to play: the page has no frames".to_string());
            return;
        };
        self.play_at(ctx, start, place, from_present);
    }

    /// Play from `start` in `place`, the scripts the person has not seen run
    /// here shown first.
    pub(crate) fn play_at(
        &mut self,
        ctx: &egui::Context,
        start: Start,
        place: Place,
        from_present: bool,
    ) {
        let doc = &self.session.doc;
        let unseen: Vec<(String, String)> = scripts_played(doc)
            .into_iter()
            .filter_map(|id| {
                let s = doc.script(id)?;
                let key = crate::prefs::disclosure_key(doc.meta().id.as_deref(), id);
                let seen = key.is_some_and(|k| self.prefs.disclosed(&k, &s.source));
                (!seen).then(|| (s.name.clone(), s.source.clone()))
            })
            .collect();
        if !unseen.is_empty() {
            self.play_disclosure = Some(Owed {
                start,
                place,
                from_present,
                scripts: unseen,
            });
            return;
        }
        self.play_from(ctx, start, place, from_present);
    }

    /// Start playing at `start`, the disclosure owed already shown.
    pub(crate) fn play_from(
        &mut self,
        ctx: &egui::Context,
        start: Start,
        place: Place,
        from_present: bool,
    ) {
        self.stop_playing(ctx);
        let player = match Player::new(&self.session.doc, start, crate::session::random_actor()) {
            Ok(p) => p,
            Err(e) => {
                self.session.fail(format!("Can't play: {e}"));
                return;
            }
        };
        self.session.history.keep_journal(true);
        let mut p = Playing::new(player, start, self.tabs.active, place);
        p.from_present = from_present;
        p.canvas = self.canvas.another();
        if place == Place::Fullscreen {
            p.was_fullscreen = ctx.input(|i| i.viewport().fullscreen.unwrap_or(false));
            ctx.send_viewport_cmd(egui::ViewportCommand::Fullscreen(true));
        }
        self.playing = Some(Box::new(p));
    }

    /// Stop playing, putting the window back as play found it.
    pub(crate) fn stop_playing(&mut self, ctx: &egui::Context) {
        let Some(p) = self.playing.take() else {
            return;
        };
        if p.place == Place::Fullscreen && !p.was_fullscreen {
            ctx.send_viewport_cmd(egui::ViewportCommand::Fullscreen(false));
        }
        if p.place == Place::Detached {
            self.prefs.save();
        }
        self.session.history.keep_journal(false);
    }

    /// Whether the player holds the keyboard this pass.
    pub(crate) fn player_holds_keys(&self) -> bool {
        self.playing
            .as_ref()
            .is_some_and(|p| p.focused && p.place != Place::Detached)
    }

    /// Hand this pass's plain keys to the player, then the editor's chords to
    /// the keymap as ever.
    pub(crate) fn player_keys(&mut self, ctx: &egui::Context) {
        if let Some(p) = self.playing.as_mut() {
            let gave_back = p.take_keys(ctx);
            if gave_back && p.place == Place::Fullscreen {
                match p.from_present {
                    true => self.stop_playing(ctx),
                    false => self.place_player(ctx, Place::Over),
                }
            }
        }
        for action in crate::input::resolve(ctx, self.keymap_mode(), self.prefs.nudge) {
            self.dispatch(ctx, action);
        }
    }

    /// Move the player to `place`.
    fn place_player(&mut self, ctx: &egui::Context, place: Place) {
        let Some(p) = self.playing.as_mut() else {
            return;
        };
        if p.place == place {
            return;
        }
        if p.place == Place::Fullscreen && !p.was_fullscreen {
            ctx.send_viewport_cmd(egui::ViewportCommand::Fullscreen(false));
        }
        if p.place == Place::Detached {
            self.prefs.save();
        }
        if place == Place::Fullscreen {
            p.was_fullscreen = ctx.input(|i| i.viewport().fullscreen.unwrap_or(false));
            ctx.send_viewport_cmd(egui::ViewportCommand::Fullscreen(true));
        }
        p.from_present = false;
        p.detached_full = false;
        p.place = place;
        p.focused = true;
    }

    /// Bring the player up to date with the document it plays: the commits since
    /// last pass, a document read again (restart), a tab closed (stop).
    fn follow_document(&mut self, ctx: &egui::Context) {
        let Some(p) = self.playing.as_mut() else {
            return;
        };
        if let Some(tab) = p.tab
            && !self.tabs.list.iter().any(|t| t.id == tab)
        {
            self.stop_playing(ctx);
            return;
        }
        // Only the document in front commits; a parked one waits, and its player
        // catches up when it comes back.
        if p.tab != self.tabs.active {
            return;
        }
        if !self.session.history.keeps_journal() {
            // The session read another document in: play it again from the start.
            self.session.history.keep_journal(true);
            match Player::new(&self.session.doc, p.start, crate::session::random_actor()) {
                Ok(fresh) => p.player = fresh,
                Err(_) => self.stop_playing(ctx),
            }
            return;
        }
        for tx in self.session.history.take_journal() {
            let _ = p.player.apply_edit(&tx);
        }
    }

    /// The player for this pass: follow the document, tick, and draw it where it
    /// shows. Called once per editor pass, after the canvas.
    pub(crate) fn player_ui(&mut self, ctx: &egui::Context) {
        self.play_disclosure_card(ctx);
        self.follow_document(ctx);
        let now = ctx.input(|i| i.time);
        let Some(mut p) = self.playing.take() else {
            return;
        };
        p.tick(ctx, now);
        let act = match p.place {
            Place::Over => self.player_card(ctx, &mut p),
            Place::Detached => self.player_window(ctx, &mut p),
            Place::Fullscreen => None,
        };
        self.playing = Some(p);
        if let Some(a) = act {
            self.player_act(ctx, a);
        }
    }

    /// Present mode's branch of the frame: the player filling the window.
    pub(crate) fn player_fullscreen_ui(&mut self, ui: &mut egui::Ui) {
        let ctx = ui.ctx().clone();
        self.follow_document(&ctx);
        let now = ctx.input(|i| i.time);
        let Some(mut p) = self.playing.take() else {
            return;
        };
        p.tick(&ctx, now);
        if !self.modal_is_up() {
            let gave_back = p.take_keys(&ctx);
            p.focused = true;
            if gave_back {
                self.playing = Some(p);
                match self.playing.as_ref().is_some_and(|p| p.from_present) {
                    true => self.stop_playing(&ctx),
                    false => self.place_player(&ctx, Place::Over),
                }
                return;
            }
        }
        let act = self.player_body(ui, &mut p, ui.available_rect_before_wrap());
        self.playing = Some(p);
        if let Some(a) = act {
            self.player_act(&ctx, a);
        }
    }

    /// The screen, bar and state card, laid out in `rect`.
    fn player_body(
        &mut self,
        ui: &mut egui::Ui,
        p: &mut Playing,
        rect: egui::Rect,
    ) -> Option<BarAct> {
        let scene = p.player.scene();
        let scaling = self
            .session
            .doc
            .page(scene.page)
            .map(|i| i.play.scaling)
            .unwrap_or_default();
        let bar_h = ui::CONTROL_H + 8.0;
        let card_w = if p.card { 220.0 } else { 0.0 };
        let room = egui::vec2(rect.width() - card_w, rect.height() - bar_h);
        let scale = fit_scale(scene.size, room, scaling);
        let size = egui::vec2(scene.size.width as f32, scene.size.height as f32) * scale;
        let screen_area = egui::Rect::from_min_size(rect.min, room);
        let screen = egui::Rect::from_center_size(screen_area.center(), size)
            .intersect(screen_area)
            .with_min_x((screen_area.center().x - size.x / 2.0).max(screen_area.left()))
            .with_min_y((screen_area.center().y - size.y / 2.0).max(screen_area.top()));
        let screen = egui::Rect::from_min_size(screen.min, size);
        let ground =
            crate::ui::color_to_egui(crate::session::page_ground(p.player.document(), scene.page));
        ui.painter().rect_filled(screen_area, 0.0, ground);
        ui.scope_builder(egui::UiBuilder::new().max_rect(screen_area), |ui| {
            ui.set_clip_rect(screen_area.intersect(ui.clip_rect()));
            p.drive_pointer(ui, screen.intersect(screen_area), scale);
            p.paint_screen(ui, screen, scale, ground);
        });
        if p.card {
            let card = egui::Rect::from_min_size(
                egui::pos2(screen_area.right(), rect.top()),
                egui::vec2(card_w, room.y),
            );
            ui.painter().rect_filled(card, 0.0, color::card());
            ui.scope_builder(
                egui::UiBuilder::new()
                    .max_rect(card.shrink(crate::settings::CANVAS_CARD_PAD / 2.0))
                    .layout(egui::Layout::top_down(egui::Align::Min)),
                |ui| state_rows(ui, p),
            );
        }
        let bar_rect = egui::Rect::from_min_size(
            egui::pos2(rect.left(), rect.bottom() - bar_h),
            egui::vec2(rect.width(), bar_h),
        );
        ui.painter().rect_filled(bar_rect, 0.0, color::card());
        let mut act = None;
        ui.scope_builder(
            egui::UiBuilder::new()
                .max_rect(bar_rect.shrink2(egui::vec2(8.0, 4.0)))
                .layout(egui::Layout::left_to_right(egui::Align::Center)),
            |ui| act = bar(ui, p),
        );
        act
    }

    /// The card over the canvas. Answers the bar's act, or `Close` for its ✕.
    fn player_card(&mut self, ctx: &egui::Context, p: &mut Playing) -> Option<BarAct> {
        let canvas = self.canvas_rect;
        let scene = p.player.scene();
        let room = egui::vec2(canvas.width(), canvas.height() - 120.0) * OVER_SHARE;
        let scale = fit_scale(scene.size, room, Scaling::Fit);
        let card_w = if p.card { 220.0 } else { 0.0 };
        let w = (scene.size.width as f32 * scale + card_w).max(OVER_MIN_W);
        let body_h = scene.size.height as f32 * scale + ui::CONTROL_H + 8.0;
        let id = egui::Id::new(CARD_ID);
        let at =
            egui::pos2(canvas.right() - w - 24.0, canvas.top() + 24.0) + ui::card_drag(ctx, id);
        let title = self.play_title(p);
        let mut act = None;
        let mut pressed_in = false;
        egui::Window::new("Prototype")
            .id(id)
            .title_bar(false)
            .resizable(false)
            .collapsible(false)
            .movable(false)
            .fixed_size(egui::vec2(
                w,
                crate::settings::CANVAS_CARD_HEAD_H + 1.0 + body_h,
            ))
            .current_pos(at)
            .frame(crate::settings::card_frame().inner_margin(egui::Margin::ZERO))
            .show(ctx, |ui| {
                ui.spacing_mut().item_spacing = egui::Vec2::ZERO;
                if crate::settings::card_head(ui, w, &title) {
                    act = Some(BarAct::Close);
                }
                ui::card_rule(ui);
                let (rect, _) = ui.allocate_exact_size(egui::vec2(w, body_h), egui::Sense::hover());
                if let Some(a) = self.player_body(ui, p, rect) {
                    act = Some(a);
                }
                pressed_in = ui.ui_contains_pointer();
            });
        // A press on the canvas gives the keyboard back to the editor.
        if ctx.input(|i| i.pointer.any_pressed()) && !pressed_in {
            p.focused = false;
        }
        act
    }

    /// The detached window: the same body in a viewport of its own, wearing the
    /// app's title bar. Its keys are its own.
    fn player_window(&mut self, ctx: &egui::Context, p: &mut Playing) -> Option<BarAct> {
        let chrome = crate::chrome::Chrome::current();
        let title = format!("{} — Mastik", self.play_title(p));
        let scene = p.player.scene();
        let mut size = egui::vec2(scene.size.width as f32, scene.size.height as f32 + 48.0);
        size = size.clamp(egui::vec2(OVER_MIN_W, 200.0), egui::vec2(1600.0, 1200.0));
        let mut builder = egui::ViewportBuilder::default()
            .with_title(title.clone())
            .with_inner_size(size)
            .with_min_inner_size(egui::vec2(OVER_MIN_W, 200.0));
        if let Some([x, y]) = self.prefs.player_window {
            builder = builder.with_position(egui::pos2(x, y));
        }
        let builder = chrome.viewport(builder);
        let id = egui::ViewportId::from_hash_of(WINDOW_ID);
        let mut act = None;
        let mut moved_to = None;
        ctx.show_viewport_immediate(id, builder, |ctx, _class| {
            if ctx.input(|i| i.viewport().close_requested()) {
                act = Some(BarAct::Close);
            }
            moved_to = ctx
                .input(|i| i.viewport().outer_rect)
                .map(|r| [r.min.x, r.min.y]);
            let focused = ctx.input(|i| i.focused);
            if focused {
                p.focused = true;
                p.take_keys(ctx);
            }
            egui::CentralPanel::default()
                .frame(egui::Frame::NONE.fill(color::card()))
                .show(ctx, |ui| {
                    let full = ui.max_rect();
                    let head = egui::Rect::from_min_size(
                        full.min,
                        egui::vec2(full.width(), crate::app::TOP_BAR_H),
                    );
                    crate::chrome::drag_strip(ui, head, true);
                    ui.scope_builder(
                        egui::UiBuilder::new()
                            .max_rect(head.shrink2(egui::vec2(12.0, 0.0)))
                            .layout(egui::Layout::left_to_right(egui::Align::Center)),
                        |ui| {
                            ui.label(
                                egui::RichText::new(&title)
                                    .size(ui::SMALL_PT)
                                    .color(theme::text::muted()),
                            );
                            ui.with_layout(
                                egui::Layout::right_to_left(egui::Align::Center),
                                |ui| crate::chrome::caption_buttons(ui, chrome, true),
                            );
                        },
                    );
                    let body = full.with_min_y(head.bottom());
                    if let Some(a) = self.player_body(ui, p, body) {
                        act = Some(a);
                    }
                });
            if p.detached_full != ctx.input(|i| i.viewport().fullscreen.unwrap_or(false)) {
                ctx.send_viewport_cmd(egui::ViewportCommand::Fullscreen(p.detached_full));
            }
        });
        // Kept as it moves, written when play leaves the window (`stop_playing`,
        // `place_player`), not on every pass of a drag.
        if let Some(at) = moved_to {
            self.prefs.player_window = Some(at);
        }
        // In the detached window, *Fullscreen* fills its own screen.
        if act == Some(BarAct::Fullscreen) {
            p.detached_full = !p.detached_full;
            return None;
        }
        act
    }

    /// The player's title: the flow's name, else the page's, masked in private
    /// mode as the page and flow names are everywhere.
    fn play_title(&self, p: &Playing) -> String {
        let doc = p.player.document();
        let page = p.player.page();
        let flow = match p.start {
            Start::Flow(r) => doc.page(r.page).and_then(|i| {
                let at = i.flows.iter().position(|f| f.id == r.flow)?;
                Some(crate::private::flow(&i.flows[at].name, at).into_owned())
            }),
            Start::Frame(_) => None,
        };
        flow.unwrap_or_else(|| crate::private::page(doc, page).into_owned())
    }

    fn player_act(&mut self, ctx: &egui::Context, act: BarAct) {
        match act {
            BarAct::Close => self.stop_playing(ctx),
            BarAct::Detach => self.place_player(ctx, Place::Detached),
            BarAct::Attach => self.place_player(ctx, Place::Over),
            BarAct::Fullscreen => self.place_player(ctx, Place::Fullscreen),
            BarAct::ExportTake => {
                if let Some(take) = self.playing.as_ref().and_then(|p| p.take.clone()) {
                    self.export_take(take);
                }
            }
            BarAct::Restart | BarAct::Hints | BarAct::Card | BarAct::Record => {
                let Some(p) = self.playing.as_mut() else {
                    return;
                };
                match act {
                    BarAct::Restart => {
                        p.player.restart();
                        p.drawn = None;
                    }
                    BarAct::Hints => p.hints = !p.hints,
                    BarAct::Card => p.card = !p.card,
                    _ => match p.player.is_recording() {
                        true => {
                            p.take = p.player.stop_recording();
                            if let Some(t) = &p.take {
                                let secs = t.length / 1000.0;
                                self.session.info(format!("Take recorded: {secs:.1} s"));
                            }
                        }
                        false => p.player.record(),
                    },
                }
            }
        }
    }

    /// The card a prototype's scripts are shown in before they first run here,
    /// as Command Mode shows a document's script before its first run.
    fn play_disclosure_card(&mut self, ctx: &egui::Context) {
        let Some(Owed {
            start,
            place,
            from_present,
            scripts,
        }) = self.play_disclosure.clone()
        else {
            return;
        };
        const W: f32 = 472.0;
        let mut answer = None;
        let outside = crate::settings::canvas_card(
            ctx,
            "play-scripts",
            self.canvas_rect,
            W,
            crate::settings::CardHold::Modal,
            |ui| {
                if crate::settings::card_head(ui, W, "This prototype runs scripts") {
                    answer = Some(false);
                }
                egui::Frame::NONE
                    .inner_margin(egui::Margin::symmetric(
                        crate::settings::CANVAS_CARD_PAD as i8,
                        4,
                    ))
                    .show(ui, |ui| {
                        ui.spacing_mut().item_spacing.y = 12.0;
                        ui.label(
                            egui::RichText::new(
                                "Its interactions run these scripts on the played copy. \
                                 Read them before you play.",
                            )
                            .size(ui::CONTROL_PT)
                            .color(theme::text::strong()),
                        );
                        ui::field_frame()
                            .inner_margin(egui::Margin::symmetric(10, 10))
                            .show(ui, |ui| {
                                egui::ScrollArea::both()
                                    .id_salt("play-scripts-code")
                                    .max_height(220.0)
                                    .show(ui, |ui| {
                                        for (name, source) in &scripts {
                                            ui.label(ui::eyebrow(name));
                                            ui.label(
                                                egui::RichText::new(mastik_vm::as_read(source))
                                                    .monospace()
                                                    .size(ui::SMALL_PT)
                                                    .color(theme::text::muted()),
                                            );
                                        }
                                    });
                            });
                    });
                let foot = crate::settings::card_foot(
                    ui,
                    W,
                    "",
                    &[
                        ("Cancel", ui::FieldButton::Off),
                        ("Play", ui::FieldButton::Primary),
                    ],
                    None,
                );
                match foot {
                    Some(0) => answer = Some(false),
                    Some(_) => answer = Some(true),
                    None => {}
                }
            },
        );
        if outside || ctx.input(|i| i.key_pressed(egui::Key::Escape)) {
            answer = answer.or(Some(false));
        }
        let Some(play) = answer else {
            return;
        };
        self.play_disclosure = None;
        if !play {
            return;
        }
        let owner = self.session.doc.meta().id.clone();
        for id in scripts_played(&self.session.doc) {
            if let (Some(s), Some(key)) = (
                self.session.doc.script(id),
                crate::prefs::disclosure_key(owner.as_deref(), id),
            ) {
                self.prefs.mark_disclosed(&key, &s.source);
            }
        }
        self.prefs.save();
        self.play_from(ctx, start, place, from_present);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::app::MastikApp;
    use mastik_core::kurbo::{Affine, RoundedRectRadii};
    use mastik_core::prototype::{Flow, Reaction, Transition, Trigger};
    use mastik_core::{IdSource, Keyed, NodeKind, Operation, Transaction};

    /// One frame of the whole app, with `events` delivered to it.
    fn whole_frame(ctx: &egui::Context, app: &mut MastikApp, events: Vec<egui::Event>) {
        let mut frame = eframe::Frame::_new_kittest();
        let input = egui::RawInput {
            screen_rect: Some(egui::Rect::from_min_size(
                egui::pos2(0.0, 0.0),
                egui::vec2(1320.0, 820.0),
            )),
            events,
            ..Default::default()
        };
        let _ = ctx.run_ui(input, |ui| eframe::App::ui(app, ui, &mut frame));
    }

    /// A page with frames A (holding a rectangle) and B to its right; A's key
    /// `N` navigates to B. With `flow`, the page has a flow starting at B. The
    /// app, then A, B and the rectangle.
    fn fixture(ctx: &egui::Context, flow: bool) -> (MastikApp, [NodeId; 3]) {
        let mut app = MastikApp::headless(ctx);
        let mut ids = IdSource::new(0x9E);
        let page = ids.mint();
        let mut doc = Document::new(page);
        let [a, b, rect] = [(); 3].map(|_| ids.mint());
        let frame = |id, x: f64, name: &str| Operation::CreateNode {
            id,
            parent: page,
            index: 0,
            kind: NodeKind::Artboard {
                size: Size::new(100.0, 80.0),
                corner_radii: RoundedRectRadii::default(),
                corner_smoothing: 0.0,
            },
            transform: Some(Affine::translate((x, 0.0))),
            name: Some(name.into()),
        };
        let navigate = Action::Navigate {
            to: b,
            transition: Transition::default(),
            keep_scroll: false,
        };
        let mut ops = vec![
            frame(a, 0.0, "A"),
            frame(b, 300.0, "B"),
            Operation::CreateNode {
                id: rect,
                parent: a,
                index: 0,
                kind: NodeKind::Rect {
                    size: Size::new(10.0, 10.0),
                    corner_radii: RoundedRectRadii::default(),
                    corner_smoothing: 0.0,
                },
                transform: Some(Affine::translate((5.0, 5.0))),
                name: Some("Dot".into()),
            },
            Operation::SetReactions {
                id: a,
                reactions: vec![Keyed::new(
                    ids.mint_item(),
                    Reaction {
                        trigger: Trigger::Key {
                            key: "N".into(),
                            shift: false,
                        },
                        actions: vec![Keyed::new(ids.mint_item(), navigate)],
                    },
                )],
            },
        ];
        if flow {
            ops.push(Operation::SetFlows {
                id: page,
                flows: vec![Keyed::new(
                    ids.mint_item(),
                    Flow {
                        name: "Checkout".into(),
                        start: b,
                        values: Vec::new(),
                    },
                )],
            });
        }
        doc.apply(&Transaction(ops)).expect("the fixture applies");
        app.session.adopt_document(doc, None);
        (app, [a, b, rect])
    }

    /// Play starts on the page's first flow when there is one, else on the
    /// page's first frame in reading order.
    /// flip: `start_for` skipping its flow lookups → fails at "the flow's start".
    #[test]
    fn play_starts_on_the_first_flow_else_the_first_frame() {
        let ctx = egui::Context::default();
        let (app, [a, ..]) = fixture(&ctx, false);
        let page = app.session.page;
        assert_eq!(
            start_for(&app.session.doc, page, None),
            Some(Start::Frame(a)),
            "no flow: the first frame"
        );
        let (app, _) = fixture(&ctx, true);
        let page = app.session.page;
        assert!(
            matches!(start_for(&app.session.doc, page, None), Some(Start::Flow(r)) if r.page == page),
            "the flow's start"
        );
    }

    /// An edit committed in the editor reaches the played copy on the next pass;
    /// so does its undo.
    /// flip: `follow_document` not applying the journal → fails at "the move reached the copy".
    #[test]
    fn the_player_follows_commits_and_undo() {
        let ctx = egui::Context::default();
        let (mut app, [_, _, rect]) = fixture(&ctx, false);
        app.play(&ctx, Place::Over, false);
        assert!(app.playing.is_some(), "playing");
        let moved = Affine::translate((40.0, 30.0));
        assert!(
            app.session
                .commit(Transaction(vec![Operation::SetTransform {
                    id: rect,
                    transform: moved,
                }]))
        );
        whole_frame(&ctx, &mut app, Vec::new());
        let at = |app: &MastikApp| {
            let p = app.playing.as_ref().expect("still playing");
            p.player.document().get(rect).map(|n| n.transform())
        };
        assert_eq!(at(&app), Some(moved), "the move reached the copy");
        app.undo();
        whole_frame(&ctx, &mut app, Vec::new());
        assert_eq!(
            at(&app),
            Some(Affine::translate((5.0, 5.0))),
            "the undo reached the copy"
        );
    }

    /// While the player holds the keyboard a plain key is the prototype's: `N`
    /// fires the key reaction.
    /// flip: `player_holds_keys` answering false → fails at "the key reaction fired".
    #[test]
    fn a_plain_key_goes_to_the_focused_player() {
        let ctx = egui::Context::default();
        let (mut app, [_, b, _]) = fixture(&ctx, false);
        app.play(&ctx, Place::Over, false);
        whole_frame(&ctx, &mut app, Vec::new());
        let key = |pressed| egui::Event::Key {
            key: egui::Key::N,
            physical_key: None,
            pressed,
            repeat: false,
            modifiers: egui::Modifiers::NONE,
        };
        whole_frame(&ctx, &mut app, vec![key(true), key(false)]);
        let p = app.playing.as_ref().expect("still playing");
        assert_eq!(p.player.screen(), b, "the key reaction fired");
    }

    /// Present mode plays the prototype fullscreen when the document has a flow,
    /// and steps through frames as before when it has none.
    /// flip: `Action::Present` always presenting → fails at "a flow plays".
    #[test]
    fn present_mode_plays_a_document_with_flows() {
        let ctx = egui::Context::default();
        let (mut app, _) = fixture(&ctx, true);
        app.dispatch(&ctx, crate::input::Action::Present);
        assert_eq!(
            app.playing.as_ref().map(|p| p.place),
            Some(Place::Fullscreen),
            "a flow plays"
        );
        assert!(app.presenting.is_none());
        app.stop_playing(&ctx);
        let (mut app, _) = fixture(&ctx, false);
        app.dispatch(&ctx, crate::input::Action::Present);
        assert!(app.playing.is_none(), "no flow: no player");
        assert!(app.presenting.is_some(), "the frames are presented");
    }

    /// The scale fits, fills the width or keeps one to one, never past the room
    /// across; a point maps back into screen space.
    #[test]
    fn the_screen_scales_and_maps_back() {
        let s = Size::new(400.0, 800.0);
        let room = egui::vec2(200.0, 200.0);
        assert_eq!(fit_scale(s, room, Scaling::Fit), 0.25);
        assert_eq!(fit_scale(s, room, Scaling::FillWidth), 0.5);
        assert_eq!(fit_scale(s, room, Scaling::ActualSize), 0.5);
        let rect = egui::Rect::from_min_size(egui::pos2(10.0, 20.0), egui::vec2(100.0, 200.0));
        assert_eq!(
            to_screen(egui::pos2(60.0, 70.0), rect, 0.5),
            Point::new(100.0, 100.0)
        );
    }
}
