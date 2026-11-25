//! The prototype player's browser shell: `mastik-play`'s engine over a document
//! loaded from bytes, drawn into a page's `<canvas>` by the renderer the editor's
//! canvas uses — vello on WebGPU, else `vello_cpu` into a 2D context
//! (`docs/design/prototyping-export.md`).
//!
//! The page's script (written by `mastik_export::prototype`) unpacks the export,
//! registers each font with [`add_font`], calls [`start`], then drives the
//! [`Player`]: each animation frame [`Player::frame`] ticks the engine and draws,
//! and the canvas's pointer, wheel and key events go to its input methods in
//! CSS pixels, which the shell maps back into screen space (`shell`).
#![cfg(target_arch = "wasm32")]

mod shell;
#[cfg(feature = "scripts")]
mod wasi;

use mastik_core::kurbo::{Point, Rect, Size};
use mastik_core::peniko::Color;
use mastik_core::prototype::FlowRef;
use mastik_core::{Document, NodeId};
use mastik_play::{Input, Request, Scene, Start};
use mastik_render::{ImageStore, VelloCpuRenderer, VelloGpuRenderer, Viewport};
use shell::{CANVAS_AREA, CANVAS_SIDE, Placement};
use wasm_bindgen::prelude::*;
use web_sys::{CanvasRenderingContext2d, HtmlCanvasElement};

/// The actor the page's player mints ids from. Any value serves: the engine
/// reserves every id the document already holds under it.
const ACTOR: u64 = 0x6d61_7374_696b_7765;

/// Panics reach the browser's console; std's own hook writes to a stderr the page
/// does not have. A panic core catches is reported too, as on the desktop.
#[wasm_bindgen(start)]
pub fn init() {
    std::panic::set_hook(Box::new(|info| {
        web_sys::console::error_1(&JsValue::from_str(&info.to_string()));
    }));
}

/// Register one font file with the shaping engine, under `alias` when the canvas
/// drew it under a name other than its own. Whether anything registered.
#[wasm_bindgen]
pub fn add_font(bytes: Vec<u8>, alias: Option<String>) -> bool {
    match alias {
        Some(family) => mastik_core::text::register_fonts_as(bytes, &family),
        None => !mastik_core::text::register_fonts(bytes).is_empty(),
    }
}

/// Load `document` (a saved `.mastik`), start playing it, and set up drawing into
/// `canvas`: WebGPU when the browser has it and `cpu` is false, else `vello_cpu`.
/// `frame`, a frame's id, starts there when the document holds it (a mirrored
/// page whose player plays no flow); empty starts as the export opens.
#[wasm_bindgen]
pub async fn start(
    canvas: HtmlCanvasElement,
    document: Vec<u8>,
    cpu: bool,
    frame: String,
) -> Result<Player, JsValue> {
    // Under `panic=unwind` wasm-bindgen wants this future unwind safe. The GPU
    // setup is awaited first, so no `Resolved` is held across an await, and is
    // asserted safe: a panic in it drops everything it made.
    let gpu = match cpu {
        true => None,
        false => std::panic::AssertUnwindSafe(Gpu::new(&canvas)).await,
    };
    let doc = mastik_core::io::load(&document).map_err(|e| err(&e))?;
    let flows = shell::flows(&doc);
    let (start, flow) =
        shell::opening(&doc, &flows, &frame).ok_or_else(|| err(&"the export has no frame"))?;
    let play = mastik_play::Player::new(&doc, start, ACTOR).map_err(|e| err(&e))?;
    let mut images = ImageStore::new();
    images.prepare(&doc);
    let backend = match gpu {
        Some(gpu) => Backend::Gpu(Box::new(gpu)),
        None => {
            let ctx = canvas
                .get_context("2d")?
                .ok_or_else(|| err(&"the canvas has no 2D context"))?
                .dyn_into::<CanvasRenderingContext2d>()?;
            Backend::Cpu(ctx, VelloCpuRenderer::new())
        }
    };
    Ok(Player {
        settings: doc.play_export().clone(),
        doc,
        play,
        images,
        backend,
        canvas,
        flows,
        flow,
        placed: None,
        drawn: None,
        turned: None,
        on_phone: false,
    })
}

fn err(e: &dyn std::fmt::Display) -> JsValue {
    JsValue::from_str(&e.to_string())
}

/// A prototype playing in the page, and the canvas it draws into.
#[wasm_bindgen]
pub struct Player {
    /// The document as loaded, which a new flow starts from.
    doc: Document,
    play: mastik_play::Player,
    settings: mastik_core::prototype::PlayExport,
    images: ImageStore,
    backend: Backend,
    canvas: HtmlCanvasElement,
    flows: Vec<(FlowRef, String)>,
    /// The flow playing, an index into `flows`; `None` when the file has none.
    flow: Option<usize>,
    /// Where the last frame put the screen, which maps the pointer back.
    placed: Option<Placement>,
    /// The last scene drawn and its pixel size, to skip a frame that changes
    /// nothing.
    drawn: Option<(Scene, (u32, u32))>,
    /// The device turned from the bar, `Some(true)` on its side; `None`
    /// follows the page's own setting.
    turned: Option<bool>,
    /// The page is open on a phone (`set_on_phone`), which the page tells
    /// from a coarse pointer on a small screen.
    on_phone: bool,
}

enum Backend {
    Gpu(Box<Gpu>),
    Cpu(CanvasRenderingContext2d, VelloCpuRenderer),
}

#[wasm_bindgen]
impl Player {
    /// `"webgpu"` or `"cpu"`: which renderer draws.
    pub fn backend(&self) -> String {
        match self.backend {
            Backend::Gpu(_) => "webgpu",
            Backend::Cpu(..) => "cpu",
        }
        .into()
    }

    /// Whether the export shows the player bar.
    pub fn player_bar(&self) -> bool {
        self.settings.player_bar
    }

    /// Whether the export shows hotspot hints: their flash after a click on
    /// nothing, and the bar's switch that keeps them shown.
    pub fn hotspot_hints(&self) -> bool {
        self.settings.hotspot_hints
    }

    /// The flows' names, in the order [`Player::play_flow`] takes.
    pub fn flows(&self) -> Vec<String> {
        self.flows.iter().map(|(_, name)| name.clone()).collect()
    }

    /// The flow playing, an index into [`Player::flows`]; -1 for none.
    pub fn flow(&self) -> i32 {
        self.flow.map_or(-1, |i| i as i32)
    }

    /// Play the flow at `index` from its start.
    pub fn play_flow(&mut self, index: usize) -> Result<(), JsValue> {
        let (r, _) = self.flows.get(index).ok_or_else(|| err(&"no such flow"))?;
        self.play =
            mastik_play::Player::new(&self.doc, Start::Flow(*r), ACTOR).map_err(|e| err(&e))?;
        self.flow = Some(index);
        self.drawn = None;
        Ok(())
    }

    /// Play `document` (a whole saved `.mastik`) in place of the one playing,
    /// keeping the run's place (`mastik_play::Player::rebase`): a mirrored
    /// page's edit from the app.
    pub fn rebase(&mut self, document: Vec<u8>) -> Result<(), JsValue> {
        let doc = mastik_core::io::load(&document).map_err(|e| err(&e))?;
        self.images.prepare(&doc);
        self.play.rebase(&doc);
        self.settings = doc.play_export().clone();
        self.flows = shell::flows(&doc);
        self.flow = self.flow.filter(|i| *i < self.flows.len());
        self.doc = doc;
        self.drawn = None;
        Ok(())
    }

    /// Whether the page is open on a phone: no device frame, and under
    /// *Responsive* the export's *On a phone* decides the device.
    pub fn set_on_phone(&mut self, on: bool) {
        self.on_phone = on;
        self.drawn = None;
    }

    /// The name of the frame on screen, which a mirrored page reports.
    pub fn screen_name(&self) -> String {
        self.play
            .document()
            .get(self.play.screen())
            .map(|n| n.name().to_string())
            .unwrap_or_default()
    }

    /// Play again from the start.
    pub fn restart(&mut self) {
        self.play.restart();
        self.drawn = None;
    }

    /// Whether the page shown has a device to turn.
    pub fn can_turn(&self) -> bool {
        self.page_play().can_turn()
    }

    /// Whether the device plays on its side: the bar's turn, else the page's.
    pub fn turned(&self) -> bool {
        self.turned.unwrap_or_else(|| self.page_play().landscape)
    }

    /// Turn the device on its side, or back, for this run.
    pub fn turn(&mut self) {
        self.turned = Some(!self.turned());
        self.drawn = None;
    }

    /// Advance play by `ms`, place the screen in a room of `room_w` × `room_h` CSS
    /// pixels and draw it at `dpr` device pixels per CSS pixel, skipping the draw
    /// when nothing changed. Answers `[scale, stage_w, stage_h, x, y, w, h, bezel,
    /// radius, moving, hint_flash, hint_density]` (the `shell::Placement`, all CSS
    /// pixels but `scale`; `moving` 1 while play changes without input;
    /// `hint_flash` the hints' flash strength, 0 for none; `hint_density` the
    /// device pixels per CSS pixel the page's 2D hints canvas is drawn at, `dpr`
    /// lowered to the 2D canvas's limits).
    pub fn frame(
        &mut self,
        ms: f64,
        room_w: f64,
        room_h: f64,
        dpr: f64,
    ) -> Result<Vec<f64>, JsValue> {
        self.play.tick(ms);
        let play = self.page_play();
        let scaling = self.settings.scaling.unwrap_or(play.scaling);
        // A phone is the device: its frame is never drawn there.
        let device = self.settings.device_frame && !self.on_phone && play.device.is_some();
        // Laid out at the window, or the device's size, turned or not; a resize
        // lays it out again.
        let room = Size::new(room_w, room_h);
        let want = play.play_size(scaling, self.turned(), room);
        if self.play.set_play_size(want) {
            self.drawn = None;
        }
        let scene = self.play.scene();
        let p = shell::place(scaling, scene.size, Size::new(room_w, room_h), device);
        self.placed = Some(p);
        let changed = !self.play.take_dirty().is_empty();
        let (side, area) = match &self.backend {
            Backend::Gpu(gpu) => (gpu.device.limits().max_texture_dimension_2d, f64::INFINITY),
            Backend::Cpu(..) => (CANVAS_SIDE, CANVAS_AREA),
        };
        let scale = shell::draw_scale(scene.size, p.scale * dpr.max(0.1), side, area);
        let vp = mastik_render::play::viewport(&scene, scale);
        let same = !changed
            && self
                .drawn
                .as_ref()
                .is_some_and(|(s, px)| *s == scene && *px == vp.pixel_size);
        if !same {
            self.draw(&scene, &vp, scale)?;
        }
        let flash = f64::from(scene.hint_flash.unwrap_or(0.0));
        let hint_density = shell::hint_density(&p, dpr);
        self.drawn = Some((scene, vp.pixel_size));
        Ok(vec![
            p.scale,
            p.stage.width,
            p.stage.height,
            p.at.x,
            p.at.y,
            p.screen.width,
            p.screen.height,
            p.bezel,
            p.radius,
            f64::from(u8::from(self.play.is_moving())),
            flash,
            hint_density,
        ])
    }

    /// The pointer is a finger (`true`) or a mouse from here on: the page tells
    /// from each event's `pointerType`, so a finger fires no hover trigger.
    pub fn set_touch(&mut self, on: bool) {
        self.play.input(Input::Touch(on));
    }

    /// The pointer moved to (`x`, `y`) CSS pixels from the canvas's top-left.
    pub fn pointer_move(&mut self, x: f64, y: f64) -> bool {
        let at = self.at(x, y);
        self.play.input(Input::PointerMove(at))
    }

    pub fn pointer_down(&mut self, x: f64, y: f64) -> bool {
        let at = self.at(x, y);
        self.play.input(Input::PointerDown(at))
    }

    pub fn pointer_up(&mut self, x: f64, y: f64) -> bool {
        let at = self.at(x, y);
        self.play.input(Input::PointerUp(at))
    }

    pub fn pointer_leave(&mut self) -> bool {
        self.play.input(Input::PointerLeave)
    }

    /// A wheel event at (`x`, `y`) by the DOM's deltas in its `mode`, a page being
    /// `page` CSS pixels. Whether a frame scrolled, so the page scrolls otherwise.
    pub fn wheel(&mut self, x: f64, y: f64, dx: f64, dy: f64, mode: u32, page: f64) -> bool {
        let at = self.at(x, y);
        let delta = shell::wheel_delta(dx, dy, mode, self.scale(), page);
        self.play.input(Input::Wheel { at, delta })
    }

    /// A DOM key went down, with Shift or not; chords with Ctrl, Alt or Meta are
    /// the browser's and never come here. A printable key types into the focused
    /// field. Whether the prototype used it.
    pub fn key(&mut self, key: &str, shift: bool) -> bool {
        if self.play.focused().is_some()
            && let Some(text) = shell::typed(key)
        {
            return self.play.input(Input::Text(text.into()));
        }
        match shell::key_name(key) {
            Some(key) => self.play.input(Input::Key { key, shift }),
            None => false,
        }
    }

    /// Whether a text field has play's focus, so the page holds the browser's
    /// focus in its hidden textarea and a phone shows its keyboard.
    pub fn focused(&self) -> bool {
        self.play.focused().is_some()
    }

    /// `text` typed into the focused field, from the page's textarea (a phone's
    /// keyboard, a finished composition). Whether a field took it.
    pub fn type_text(&mut self, text: &str) -> bool {
        self.play.focused().is_some() && self.play.input(Input::Text(text.into()))
    }

    /// The places the person can act on, `[x, y, w, h]` each, in CSS pixels from
    /// the canvas's top-left.
    pub fn hotspots(&self) -> Vec<f64> {
        let s = self.scale();
        self.play
            .hotspots()
            .iter()
            .flat_map(|h| {
                let r = h.rect;
                [r.x0 * s, r.y0 * s, r.width() * s, r.height() * s]
            })
            .collect()
    }

    /// Whether (`x`, `y`) CSS pixels is over a place the person can act on, for the
    /// pointer's cursor.
    pub fn over_hotspot(&self, x: f64, y: f64) -> bool {
        let at = self.at(x, y);
        self.play.hotspots().iter().any(|h| h.rect.contains(at))
    }

    /// The addresses play asked to open since the last call.
    pub fn take_urls(&mut self) -> Vec<String> {
        self.play
            .take_requests()
            .into_iter()
            .map(|r| match r {
                Request::OpenUrl(url) => url,
            })
            .collect()
    }

    /// What play has to say since the last call, a sentence each; the first
    /// character `!` marks one the viewer is shown, the rest go to the console.
    pub fn take_notices(&mut self) -> Vec<String> {
        self.play
            .take_notices()
            .into_iter()
            .map(shell::notice_text)
            .collect()
    }

    // ── Inspect mode ────────────────────────────────────────────────────────

    /// The topmost visible layer under (`x`, `y`) CSS pixels in the frame on top
    /// (the top overlay, else the screen), by its box, as its id; empty over
    /// none. The frame itself counts, under its children.
    pub fn layer_at(&self, x: f64, y: f64) -> String {
        let at = self.at(x, y);
        let Some((scene, _)) = &self.drawn else {
            return String::new();
        };
        let Some(top) = scene.layers.last() else {
            return String::new();
        };
        shell::topmost(self.play.document(), top.frame, at, |id| {
            self.screen_rect(id)
        })
        .map(NodeId::to_wire)
        .unwrap_or_default()
    }

    /// `id`'s box, `[x, y, w, h]` in CSS pixels from the canvas; empty when the
    /// layer is not on a shown frame.
    pub fn layer_box(&self, id: &str) -> Vec<f64> {
        let s = self.scale();
        NodeId::from_wire(id)
            .and_then(|id| self.screen_rect(id))
            .map(|r| vec![r.x0 * s, r.y0 * s, r.width() * s, r.height() * s])
            .unwrap_or_default()
    }

    /// `id`'s name; empty for none.
    pub fn layer_name(&self, id: &str) -> String {
        NodeId::from_wire(id)
            .and_then(|id| self.play.document().get(id))
            .map(|n| n.name().to_string())
            .unwrap_or_default()
    }

    /// `id`'s annotation; empty for none.
    pub fn layer_note(&self, id: &str) -> String {
        NodeId::from_wire(id)
            .and_then(|id| self.play.document().get(id))
            .map(|n| n.note().to_string())
            .unwrap_or_default()
    }

    /// The distances between `a` and `b` as the canvas's `Alt` measure quotes
    /// them (`mastik_core::measure::between`), with `b` empty meaning `a`'s
    /// parent. Each is `"h"` or `"v"` (the line's direction), where it sits,
    /// where it runs from and to, all CSS pixels from the canvas, and its label,
    /// joined by `|`.
    pub fn measures(&self, a: &str, b: &str) -> Vec<String> {
        let Some(a) = NodeId::from_wire(a) else {
            return Vec::new();
        };
        let b = match b {
            "" => self.play.document().get(a).and_then(|n| n.parent()),
            b => NodeId::from_wire(b),
        };
        let (Some(ra), Some(rb)) = (self.screen_rect(a), b.and_then(|b| self.screen_rect(b)))
        else {
            return Vec::new();
        };
        shell::measure_lines(ra, rb, self.scale())
    }
}

impl Player {
    fn scale(&self) -> f64 {
        self.placed.map_or(1.0, |p| p.scale)
    }

    /// The prototype settings of the page shown.
    /// On a phone under *Responsive* with the export's *Follow its turn*, the
    /// page plays with no device, so it lays out at the phone's size and turns
    /// with it; otherwise as the page says.
    fn page_play(&self) -> mastik_core::prototype::PagePlay {
        use mastik_core::prototype::{OnPhone, Scaling};
        let doc = self.play.document();
        let mut play = doc
            .page(self.play.page())
            .map(|p| p.play.clone())
            .unwrap_or_default();
        let scaling = self.settings.scaling.unwrap_or(play.scaling);
        if self.on_phone
            && scaling == Scaling::Responsive
            && self.settings.on_phone == OnPhone::Follow
        {
            play.device = None;
            play.landscape = false;
        }
        play
    }

    /// `id`'s box in screen space where the last scene drew it: its own box, or
    /// its bounds for a layer with none, moved from its frame's world place to
    /// the frame's place on screen and by every scrolling frame above it. `None`
    /// when no shown frame holds it.
    fn screen_rect(&self, id: NodeId) -> Option<Rect> {
        let (scene, _) = self.drawn.as_ref()?;
        let doc = self.play.document();
        let res = self.play.resolved();
        let chain: Vec<NodeId> =
            std::iter::successors(Some(id), |n| doc.get(*n)?.parent()).collect();
        let layer = scene
            .layers
            .iter()
            .rev()
            .find(|l| chain.contains(&l.frame))?;
        let world = |n: NodeId| -> Option<Rect> {
            match res.used_kind(doc, n)?.box_size() {
                Some(size) => Some(
                    res.world_transform(n)?
                        .transform_rect_bbox(Rect::from_origin_size(Point::ZERO, size)),
                ),
                None => res.world_bounds(n),
            }
        };
        let origin = res.world_transform(layer.frame)? * Point::ZERO;
        let scroll = chain
            .iter()
            .skip(1)
            .take_while(|n| **n != layer.frame)
            .chain(std::iter::once(&layer.frame).filter(|f| **f != id))
            .fold(mastik_core::kurbo::Vec2::ZERO, |v, f| {
                v + self.play.scroll_offset(*f)
            });
        Some(world(id)? + (layer.at - origin) - scroll)
    }

    fn at(&self, x: f64, y: f64) -> Point {
        shell::to_screen(Point::new(x, y), self.scale())
    }

    /// Draw `scene` at `scale` device pixels per screen unit, sizing the canvas's
    /// backing store to `vp`.
    fn draw(&mut self, scene: &Scene, vp: &Viewport, scale: f64) -> Result<(), JsValue> {
        let (width, height) = vp.pixel_size;
        if (self.canvas.width(), self.canvas.height()) != (width, height) {
            self.canvas.set_width(width);
            self.canvas.set_height(height);
        }
        let doc = self.play.document();
        let res = self.play.resolved();
        match &mut self.backend {
            Backend::Gpu(gpu) => gpu.draw(doc, res, vp, &self.images, scene),
            Backend::Cpu(ctx, renderer) => {
                let (rgba, w, h) =
                    renderer.render_play_to_rgba(doc, res, &self.images, scene, scale);
                let data = web_sys::ImageData::new_with_u8_clamped_array_and_sh(
                    wasm_bindgen::Clamped(&rgba),
                    w,
                    h,
                )?;
                ctx.put_image_data(&data, 0.0, 0.0)
            }
        }
    }
}

/// vello on the browser's WebGPU, drawing into a storage texture that is then
/// copied onto the canvas: a canvas texture cannot be a storage target.
struct Gpu {
    device: wgpu::Device,
    queue: wgpu::Queue,
    surface: wgpu::Surface<'static>,
    config: wgpu::SurfaceConfiguration,
    renderer: VelloGpuRenderer,
    blitter: wgpu::util::TextureBlitter,
    target: Option<(wgpu::Texture, wgpu::TextureView)>,
}

impl Gpu {
    /// `None` wherever WebGPU is missing or refuses: no `navigator.gpu`, no
    /// adapter, no device. The canvas is claimed for WebGPU only once all three
    /// answered, so a refusal leaves it free for the 2D fallback.
    async fn new(canvas: &HtmlCanvasElement) -> Option<Self> {
        if !wgpu::util::is_browser_webgpu_supported().await {
            return None;
        }
        let instance = wgpu::Instance::new(wgpu::InstanceDescriptor {
            backends: wgpu::Backends::BROWSER_WEBGPU,
            ..wgpu::InstanceDescriptor::new_without_display_handle()
        });
        let adapter = instance
            .request_adapter(&wgpu::RequestAdapterOptions::default())
            .await
            .ok()?;
        // The adapter's own limits, so a tall screen draws sharp past the
        // default 8192 where the hardware allows; `frame` caps the rest.
        let (device, queue) = adapter
            .request_device(&wgpu::DeviceDescriptor {
                required_limits: adapter.limits(),
                ..wgpu::DeviceDescriptor::default()
            })
            .await
            .ok()?;
        let renderer = VelloGpuRenderer::new(&device).ok()?;
        let surface = instance
            .create_surface(wgpu::SurfaceTarget::Canvas(canvas.clone()))
            .ok()?;
        let caps = surface.get_capabilities(&adapter);
        // vello writes sRGB-encoded values; an `-srgb` canvas would encode them twice.
        let format = caps.formats.iter().copied().find(|f| !f.is_srgb())?;
        let config = wgpu::SurfaceConfiguration {
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT,
            format,
            width: canvas.width().max(1),
            height: canvas.height().max(1),
            present_mode: wgpu::PresentMode::Fifo,
            desired_maximum_frame_latency: 2,
            alpha_mode: caps.alpha_modes.first().copied()?,
            view_formats: Vec::new(),
        };
        let blitter = wgpu::util::TextureBlitter::new(&device, format);
        Some(Self {
            device,
            queue,
            surface,
            config,
            renderer,
            blitter,
            target: None,
        })
    }

    fn draw(
        &mut self,
        doc: &Document,
        res: &mastik_core::Resolved,
        vp: &Viewport,
        images: &ImageStore,
        scene: &Scene,
    ) -> Result<(), JsValue> {
        let (width, height) = vp.pixel_size;
        if (self.config.width, self.config.height) != (width, height) || self.target.is_none() {
            self.config.width = width;
            self.config.height = height;
            self.surface.configure(&self.device, &self.config);
            let texture = self.device.create_texture(&wgpu::TextureDescriptor {
                label: Some("mastik-web-target"),
                size: wgpu::Extent3d {
                    width,
                    height,
                    depth_or_array_layers: 1,
                },
                mip_level_count: 1,
                sample_count: 1,
                dimension: wgpu::TextureDimension::D2,
                format: wgpu::TextureFormat::Rgba8Unorm,
                usage: wgpu::TextureUsages::STORAGE_BINDING | wgpu::TextureUsages::TEXTURE_BINDING,
                view_formats: &[],
            });
            let view = texture.create_view(&wgpu::TextureViewDescriptor::default());
            self.target = Some((texture, view));
        }
        let Some((_, view)) = &self.target else {
            return Ok(());
        };
        let patches = mastik_render::play::overrides(doc, res, scene);
        self.renderer
            .render_play(
                &self.device,
                &self.queue,
                doc,
                res,
                vp,
                &patches,
                scene,
                view,
                Color::WHITE,
                images,
            )
            .map_err(|e| err(&e))?;
        let frame = match self.surface.get_current_texture() {
            wgpu::CurrentSurfaceTexture::Success(f)
            | wgpu::CurrentSurfaceTexture::Suboptimal(f) => f,
            _ => return Ok(()),
        };
        let out = frame
            .texture
            .create_view(&wgpu::TextureViewDescriptor::default());
        let mut encoder = self
            .device
            .create_command_encoder(&wgpu::CommandEncoderDescriptor::default());
        self.blitter.copy(&self.device, &mut encoder, view, &out);
        self.queue.submit([encoder.finish()]);
        frame.present();
        Ok(())
    }
}
