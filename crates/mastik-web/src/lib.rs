//! The prototype player's browser shell: a document loaded from bytes and drawn
//! into a page's `<canvas>` by the renderer the editor's canvas uses — vello on
//! WebGPU, else `vello_cpu` into a 2D context (`docs/design/prototyping-export.md`).
//!
//! The page's script (written by `mastik_export::prototype`) unpacks the export,
//! registers each font with [`add_font`], then calls [`start`] and draws.
//!
//! **The engine seam.** [`Player`] shows one screen, [`Player::screen`]: the
//! start frame of the flow the export opens on. The play engine (`mastik-play`)
//! plugs in there — it owns the copy and the current screen, and the shell keeps
//! only drawing and the canvas.
#![cfg(target_arch = "wasm32")]

use mastik_core::kurbo::Rect;
use mastik_core::peniko::Color;
use mastik_core::{Document, NodeId, NodeKind, Resolved};
use mastik_render::{ImageStore, RenderOverrides, VelloCpuRenderer, VelloGpuRenderer, Viewport};
use wasm_bindgen::prelude::*;
use web_sys::{CanvasRenderingContext2d, HtmlCanvasElement};

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

/// Load `document` (a saved `.mastik`) and set up drawing into `canvas`: WebGPU
/// when the browser has it and `cpu` is false, else `vello_cpu`.
#[wasm_bindgen]
pub async fn start(
    canvas: HtmlCanvasElement,
    document: Vec<u8>,
    cpu: bool,
) -> Result<Player, JsValue> {
    // Under `panic=unwind` wasm-bindgen wants this future unwind safe. The GPU
    // setup is awaited first, so no `Resolved` is held across an await, and is
    // asserted safe: a panic in it drops everything it made.
    let gpu = match cpu {
        true => None,
        false => std::panic::AssertUnwindSafe(Gpu::new(&canvas)).await,
    };
    let mut doc = mastik_core::io::load(&document).map_err(|e| err(&e))?;
    let res = Resolved::settled(&mut doc);
    let (page, frame) = opening_screen(&doc).ok_or_else(|| err(&"the export has no frame"))?;
    let bounds = res
        .world_bounds(frame)
        .filter(|r| r.width() > 0.0 && r.height() > 0.0)
        .ok_or_else(|| err(&"the opening frame has no size"))?;
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
        doc,
        res,
        images,
        page,
        frame,
        bounds,
        backend,
    })
}

/// The flow the export opens on, as (page, start frame): `PlayExport::opens`,
/// else the first flow of the first page that has one, else the first page's
/// first frame. A start that dangles falls through to the next answer.
fn opening_screen(doc: &Document) -> Option<(NodeId, NodeId)> {
    let live = |page: NodeId, frame: NodeId| {
        (doc.get(frame).is_some() && doc.page_of(frame) == Some(page)).then_some((page, frame))
    };
    let opens = doc.play_export().opens.and_then(|r| {
        let flow = doc.page(r.page)?.flows.iter().find(|f| f.id == r.flow)?;
        live(r.page, flow.value.start)
    });
    let first_flow = || {
        doc.pages()
            .iter()
            .flat_map(|p| p.flows.iter().map(move |f| (p.id, f.value.start)))
            .find_map(|(p, f)| live(p, f))
    };
    let first_frame = || {
        let page = doc.first_page();
        let frame = doc.get(page)?.children().iter().copied().find(|c| {
            doc.get(*c)
                .is_some_and(|n| matches!(n.kind(), NodeKind::Artboard { .. }))
        })?;
        Some((page, frame))
    };
    opens.or_else(first_flow).or_else(first_frame)
}

fn err(e: &dyn std::fmt::Display) -> JsValue {
    JsValue::from_str(&e.to_string())
}

/// A loaded document and the canvas it draws into.
#[wasm_bindgen]
pub struct Player {
    doc: Document,
    res: Resolved,
    images: ImageStore,
    page: NodeId,
    frame: NodeId,
    /// The frame's world box, which the canvas shows.
    bounds: Rect,
    backend: Backend,
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

    /// The screen's size in document units, `[width, height]`, for the page to fit
    /// the canvas to.
    pub fn screen_size(&self) -> Vec<f64> {
        vec![self.bounds.width(), self.bounds.height()]
    }

    /// Draw the screen into the canvas at `width` × `height` device pixels, the
    /// canvas's backing size.
    pub fn draw(&mut self, width: u32, height: u32) -> Result<(), JsValue> {
        let (page, frame) = self.screen();
        let vp = Viewport {
            view: self.bounds,
            pixel_size: (width.max(1), height.max(1)),
        };
        match &mut self.backend {
            Backend::Gpu(gpu) => gpu.draw(&self.doc, &self.res, &vp, &self.images, page),
            Backend::Cpu(ctx, renderer) => {
                let (rgba, w, h) = renderer.render_subtrees_to_rgba(
                    &self.doc,
                    &self.res,
                    &vp,
                    &self.images,
                    &[frame],
                );
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

impl Player {
    /// The page and frame on screen. The play engine answers this in Wave 2.
    fn screen(&self) -> (NodeId, NodeId) {
        (self.page, self.frame)
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
        let (device, queue) = adapter
            .request_device(&wgpu::DeviceDescriptor::default())
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
        res: &Resolved,
        vp: &Viewport,
        images: &ImageStore,
        page: NodeId,
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
        self.renderer
            .render(
                &self.device,
                &self.queue,
                doc,
                res,
                vp,
                &RenderOverrides::default(),
                view,
                Color::WHITE,
                images,
                page,
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
