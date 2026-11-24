//! The document's pictures as files: what each weighs, how small it is drawn,
//! *Remove unused pictures*, which takes the ones nothing draws out of the table,
//! and the two actions that rewrite a picture's bytes, *Apply crop* and *Reduce
//! picture…* (`docs/design/app-image-editing.md`). Each rewrite decodes and
//! encodes on a worker and lands as one undo step.

use crate::app::MastikApp;
use crate::job::Job;
use crate::theme::{self, icon};
use crate::ui::{self, FieldButton, Prefix, Scrub};
use eframe::egui;
use mastik_core::{
    Document, ImageEntry, ImageFormat, ImageId, ImageSource, NodeId, Operation, Transaction,
};
use std::sync::Arc;

/// What the palette and the Tools menu say when nothing can be removed.
pub(crate) const ALL_PICTURES_USED: &str = "Every picture is in use";

/// Under this share of its pixels (across, after the crop) a picture's weight
/// readout turns amber: drawn this small, *Reduce picture…* pays off.
pub(crate) const HEAVY_BELOW: f64 = 0.25;

/// [`MastikApp::unused_pictures`]' answer, kept until the document changes.
#[derive(Clone, Default)]
pub(crate) struct Unused {
    /// The document it was read from: its revision and the page in front, which
    /// tells one tab's document from another's.
    key: Option<(u64, NodeId)>,
    pub(crate) ids: Vec<ImageId>,
    /// What removing them frees, in bytes.
    pub(crate) bytes: u64,
}

/// What a picture's entry holds in the file: its encoded bytes, or nothing for a
/// linked one.
pub(crate) fn stored_bytes(doc: &Document, id: &ImageId) -> u64 {
    match doc.image(id).map(|e| &e.source) {
        Some(ImageSource::Embedded(b)) => b.len() as u64,
        _ => 0,
    }
}

/// What the image card's Asset section reads out about a picture.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct Weight {
    /// The entry, when the table holds one: `None` for a reference to nothing.
    pub(crate) entry: Option<mastik_core::ImageEntry>,
    /// Whether its pixels can be had: embedded, not linked.
    pub(crate) embedded: bool,
    pub(crate) bytes: u64,
    /// How many layers show it.
    pub(crate) layers: usize,
    /// The largest it is drawn anywhere, in document units per source pixel
    /// across: `1.0` is one unit a pixel. `None` when no layer draws it.
    pub(crate) drawn: Option<f64>,
}

impl Weight {
    /// The readout's two lines and whether the second is a warning: format,
    /// pixels and stored size, then how small it is drawn and on how many layers.
    pub(crate) fn lines(&self) -> (String, String, bool) {
        let layers = match self.layers {
            0 | 1 => "on this layer only".to_string(),
            n => format!("on {n} layers"),
        };
        let Some(e) = self.entry.as_ref().filter(|_| self.embedded) else {
            let first = match &self.entry {
                Some(e) => format!("Missing · {} × {} when saved", e.width, e.height),
                None => "Missing".to_string(),
            };
            return (first, ui::upper_first(&layers), false);
        };
        let first = format!(
            "{} · {} × {} · {}",
            e.format.extension().to_uppercase().replace("JPG", "JPEG"),
            e.width,
            e.height,
            crate::settings::human_bytes(self.bytes)
        );
        let (drawn, heavy) = match self.drawn {
            Some(f) if f < 0.95 => {
                let n = (1.0 / f).round().max(2.0);
                (format!("Drawn at 1/{n} of its pixels"), f < HEAVY_BELOW)
            }
            _ => ("Drawn at 1:1".to_string(), false),
        };
        (first, format!("{drawn} · {layers}"), heavy)
    }
}

impl MastikApp {
    /// What `id` weighs and how small it is drawn, across every layer that
    /// shows it: the largest drawn size, so a picture drawn small in one place and
    /// full size in another is not flagged. The zoom plays no part.
    pub(crate) fn picture_weight(&self, id: &ImageId) -> Weight {
        let doc = &self.session.doc;
        let res = &self.session.resolved;
        let entry = doc.image(id).cloned();
        let mut layers = 0;
        let mut drawn: Option<f64> = None;
        for n in doc.node_ids() {
            let Some(node) = doc.get(n) else { continue };
            let paint = node.paint();
            let brushes = (paint.fills.iter().map(|f| &f.brush))
                .chain(paint.strokes.iter().map(|s| &s.brush));
            let mut shows = false;
            for b in brushes {
                let Some(mastik_core::Brush::Image(img)) = b.literal() else {
                    continue;
                };
                if img.image.id != *id {
                    continue;
                }
                shows = true;
                let (Some(e), Some(frame), Some(world)) = (
                    entry.as_ref(),
                    mastik_core::local_box(doc, res, n),
                    res.world_transform(n),
                ) else {
                    continue;
                };
                let m = world * img.image.framing(frame, e.width, e.height).transform;
                let per_px = largest_scale(m);
                if per_px.is_finite() && per_px > 0.0 {
                    drawn = Some(drawn.map_or(per_px, |d| d.max(per_px)));
                }
            }
            layers += usize::from(shows);
        }
        Weight {
            embedded: matches!(
                entry.as_ref().map(|e| &e.source),
                Some(ImageSource::Embedded(_))
            ),
            bytes: stored_bytes(doc, id),
            entry,
            layers,
            drawn,
        }
    }

    /// The pictures nothing draws and what they weigh, read once per document
    /// change: the palette and the Tools menu ask every frame they are open.
    pub(crate) fn unused_pictures(&self) -> (Vec<ImageId>, u64) {
        let key = (self.session.revision(), self.session.page);
        let mut cache = self.unused_cache.borrow_mut();
        if cache.key != Some(key) {
            let doc = &self.session.doc;
            let ids = mastik_core::io::unused_images(doc);
            let bytes = ids.iter().map(|id| stored_bytes(doc, id)).sum();
            *cache = Unused {
                key: Some(key),
                ids,
                bytes,
            };
        }
        (cache.ids.clone(), cache.bytes)
    }

    /// *Remove unused pictures*: one undo step taking every picture nothing draws
    /// out of the table, so undo brings each one back.
    pub(crate) fn remove_unused_images(&mut self) {
        let (ids, bytes) = self.unused_pictures();
        if ids.is_empty() {
            return self.session.info(ALL_PICTURES_USED);
        }
        let n = ids.len();
        let tx = Transaction(
            ids.into_iter()
                .map(|id| Operation::RemoveImage { id })
                .collect(),
        );
        self.session
            .name_step("Remove unused pictures", icon::TRASH_ALL);
        if self.session.commit(tx) {
            self.session.info(format!(
                "Removed {} — {}",
                crate::ui::count_as(n, "unused picture", "unused pictures"),
                crate::settings::human_bytes(bytes)
            ));
        }
    }

    /// The layer's name for a rewrite's status line.
    fn picture_layer_name(&self, id: NodeId) -> String {
        (self.session.doc.get(id)).map_or_else(|| "the picture".into(), |n| n.name().to_string())
    }

    /// The writes that put `entry` in the table as `new` and take `old` out when
    /// `ops` leave nothing naming it, so a rewrite makes the file smaller in the
    /// same step and undo brings every byte back.
    fn rewrite_tx(
        &self,
        old: &ImageId,
        new: &ImageId,
        entry: ImageEntry,
        ops: Vec<Operation>,
    ) -> Transaction {
        let doc = &self.session.doc;
        let mut all = Vec::new();
        if !doc.has_image(new) {
            all.push(Operation::AddImage {
                id: new.clone(),
                entry,
            });
        }
        all.extend(ops);
        let mut after = doc.clone();
        if old != new
            && after.apply(&Transaction(all.clone())).is_ok()
            && mastik_core::io::unused_images(&after).contains(old)
        {
            all.push(Operation::RemoveImage { id: old.clone() });
        }
        Transaction(all)
    }

    /// *Apply crop*: keep only the pixels inside `id`'s crop, cut in the picture's
    /// own pixel space so the mirrors, the turn and the adjustments stay live.
    /// The layer gets a picture of its own, still in Crop and drawn exactly as it
    /// was; others showing the original keep it. Decoded and encoded on
    /// a worker, landed by [`Self::poll_pictures`].
    pub(crate) fn apply_crop(&mut self, id: NodeId, fill: usize) {
        if self.picture_busy() {
            return;
        }
        let Some((img, entry)) = self.fill_picture(id, fill) else {
            return;
        };
        let Some(cut) = mastik_core::edit::crop_source_px(&img, entry.width, entry.height) else {
            return;
        };
        let out = match entry.format {
            ImageFormat::Jpeg => Out::Jpeg(JPEG_KEEP),
            _ => Out::Png,
        };
        let job = Job::spawn("mastik-apply-crop", &self.egui_ctx, move || {
            let px = decode_rgba(&entry)?;
            let (x, y, w, h) = cut;
            let cropped = image::imageops::crop_imm(&px, x, y, w, h).to_image();
            encode(&cropped, out)
        });
        self.session.info("Cropping the picture…");
        self.picture_job = Some(PictureJob {
            job,
            target: Target {
                layer: id,
                fill,
                picture: img.id,
                revision: self.session.revision(),
                kind: Rewrite::ApplyCrop,
            },
        });
    }

    /// Whether a rewrite is still on its worker, said on the status line: one runs
    /// at a time, and a second would drop the first unlanded.
    fn picture_busy(&mut self) -> bool {
        let busy = self.picture_job.is_some();
        if busy {
            self.session
                .info("Still rewriting a picture — try again in a moment");
        }
        busy
    }

    /// The image fill at `fill` on `id`, and its table entry.
    fn fill_picture(&self, id: NodeId, fill: usize) -> Option<(mastik_core::ImageRef, ImageEntry)> {
        let node = self.session.doc.get(id)?;
        let mastik_core::Brush::Image(b) = node.paint().fills.get(fill)?.brush.literal()? else {
            return None;
        };
        let entry = self.session.doc.image(&b.image.id)?.clone();
        Some((b.image.clone(), entry))
    }

    /// Land a finished rewrite, only on the document it was asked of: an edit
    /// in between could have moved the layer or the picture it read.
    pub(crate) fn poll_pictures(&mut self) {
        let Some(pending) = self.picture_job.take() else {
            return;
        };
        let landed = match pending.job.poll() {
            Ok(None) => {
                self.picture_job = Some(pending);
                return;
            }
            Ok(Some(v)) => v,
            Err(()) => None,
        };
        let Some(encoded) = landed else {
            return self
                .session
                .info("The picture could not be read to rewrite");
        };
        if pending.target.revision != self.session.revision() {
            return self
                .session
                .info("The document changed meanwhile — the picture was left as it was");
        }
        self.land_rewrite(&pending.target, encoded);
    }

    /// Commit a rewrite of `p.picture` as `e`, one undo step with its status.
    fn land_rewrite(&mut self, p: &Target, e: Encoded) {
        let doc = &self.session.doc;
        let before = stored_bytes(doc, &p.picture);
        let new = crate::app::content_hash(&e.bytes);
        let (w, h, after) = (e.width, e.height, e.bytes.len() as u64);
        let entry = ImageEntry {
            source: ImageSource::Embedded(e.bytes.into()),
            format: e.format,
            width: w,
            height: h,
        };
        let name = self.picture_layer_name(p.layer);
        let sizes = format!(
            "{} to {}",
            crate::settings::human_bytes(before),
            crate::settings::human_bytes(after)
        );
        let Some(was) = doc.image(&p.picture) else {
            return;
        };
        let (was_w, was_h) = (was.width, was.height);
        let (ops, step, glyph, said) = match p.kind {
            Rewrite::ApplyCrop => {
                let Some(node) = doc.get(p.layer) else { return };
                let mut fills = node.paint().fills.clone();
                let Some(mastik_core::Brush::Image(b)) =
                    fills.get_mut(p.fill).and_then(|f| f.brush.literal_mut())
                else {
                    return;
                };
                // The mode stays: Crop maps the crop onto the box exactly, and the
                // crop left is what was shown of the cut, rounded outward to pixels.
                let Some(crop) = mastik_core::edit::crop_after_cut(&b.image, was_w, was_h) else {
                    return;
                };
                b.image.id = new.clone();
                b.image.crop = crop;
                (
                    vec![Operation::SetFills { id: p.layer, fills }],
                    "Apply crop",
                    icon::CROP,
                    format!("Cropped {name} to {w} × {h} — {sizes}"),
                )
            }
            Rewrite::Reduce => (
                mastik_core::edit::repoint_image(
                    doc,
                    &p.picture,
                    &new,
                    f64::from(was_w) / f64::from(w.max(1)),
                ),
                "Reduce picture",
                icon::CORNERS_IN,
                format!("Reduced {name} — {sizes}"),
            ),
        };
        let tx = self.rewrite_tx(&p.picture, &new, entry, ops);
        self.session.name_step(step, glyph);
        if self.commit_edit(tx) {
            self.session.info(said);
        }
    }

    /// Open *Reduce picture…* on the picture `id` shows, decoding it on a worker.
    pub(crate) fn open_reduce(&mut self, id: NodeId, picture: ImageId) {
        let Some(entry) = self.session.doc.image(&picture).cloned() else {
            return;
        };
        let weight = self.picture_weight(&picture);
        let sizes = reduce_sizes(entry.width, entry.height, weight.drawn);
        let pick = reduce_pick(&sizes);
        let decoding = Job::spawn("mastik-reduce-read", &self.egui_ctx, move || {
            decode_rgba(&entry).map(Arc::new)
        });
        self.reduce = Some(ReduceCard {
            layer: id,
            picture,
            revision: self.session.revision(),
            layers: weight.layers,
            sizes,
            size: pick,
            jpeg: true,
            quality: 85.0,
            decoding: Some(decoding),
            pixels: None,
            unreadable: false,
            measuring: None,
            measured: None,
        });
    }

    /// *Reduce picture…*'s card over the canvas: size, format and quality, the
    /// weight now and measured after, then *Cancel* and *Reduce*. Escape, the ×,
    /// *Cancel* or a change under it closes it.
    pub(crate) fn reduce_ui(&mut self, ctx: &egui::Context, canvas: egui::Rect) {
        let Some(mut card) = self.reduce.take() else {
            return;
        };
        if card.revision != self.session.revision()
            || ctx.input(|i| i.key_pressed(egui::Key::Escape))
        {
            return;
        }
        let enter = ui::card_enter(ctx);
        card.pump(ctx);
        let before = stored_bytes(&self.session.doc, &card.picture);
        let mut act = None;
        let hold = crate::settings::CardHold::Modal;
        let clicked_away = crate::settings::movable_canvas_card(
            ctx,
            "reduce-picture",
            canvas,
            REDUCE_W,
            hold,
            |ui| {
                if crate::settings::card_head(ui, REDUCE_W, "Reduce picture") {
                    act = Some(false);
                }
                ui::card_rule(ui);
                egui::Frame::NONE
                    .inner_margin(egui::Margin::same(crate::settings::CANVAS_CARD_PAD as i8))
                    .show(ui, |ui| reduce_body(ui, &mut card, before));
                ui::card_rule(ui);
                let ready = card.done().is_some();
                let why_not = match (card.unreadable, ready) {
                    (true, _) => Some("This picture can't be read to reduce"),
                    (false, false) => Some("Still measuring the result"),
                    _ => None,
                };
                let state = match why_not {
                    None => FieldButton::Primary,
                    Some(_) => FieldButton::Disabled,
                };
                let note = match card.layers {
                    0 | 1 => "On this layer only".to_string(),
                    n => format!("On {n} layers, all made lighter together"),
                };
                let buttons = [("Cancel", FieldButton::Off), ("Reduce", state)];
                match crate::settings::card_foot(ui, REDUCE_W, &note, &buttons, why_not) {
                    Some(0) => act = Some(false),
                    Some(_) if why_not.is_none() => act = Some(true),
                    _ => {}
                }
            },
        );
        if clicked_away {
            return;
        }
        if enter && card.done().is_some() && act.is_none() {
            act = Some(true);
        }
        match act {
            Some(false) => {}
            Some(true) => {
                if let Some(e) = card.done() {
                    let p = Target {
                        layer: card.layer,
                        fill: 0,
                        picture: card.picture.clone(),
                        revision: card.revision,
                        kind: Rewrite::Reduce,
                    };
                    self.land_rewrite(&p, (*e).clone());
                }
            }
            None => self.reduce = Some(card),
        }
    }
}

/// *Reduce picture…*'s card width.
const REDUCE_W: f32 = 380.0;

/// The quality a JPEG keeps when *Apply crop* re-encodes it: the cut is not the
/// place to lose detail.
const JPEG_KEEP: u8 = 92;

/// How a rewrite encodes.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) enum Out {
    Jpeg(u8),
    Png,
}

/// A rewritten picture, encoded.
#[derive(Clone, Debug)]
pub(crate) struct Encoded {
    pub(crate) bytes: Vec<u8>,
    pub(crate) format: ImageFormat,
    pub(crate) width: u32,
    pub(crate) height: u32,
}

/// Which rewrite a [`PictureJob`] is.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) enum Rewrite {
    ApplyCrop,
    Reduce,
}

/// What a rewrite was asked of: the layer and fill it started from (whose name
/// the status line gives), the picture, and the document's revision then.
pub(crate) struct Target {
    layer: NodeId,
    fill: usize,
    picture: ImageId,
    revision: u64,
    kind: Rewrite,
}

/// A rewrite on its worker, landed only on the document it was asked of.
pub(crate) struct PictureJob {
    job: Job<Option<Encoded>>,
    target: Target,
}

/// The picture's pixels, straight RGBA, when its bytes are here to decode.
pub(crate) fn decode_rgba(entry: &ImageEntry) -> Option<image::RgbaImage> {
    let ImageSource::Embedded(bytes) = &entry.source else {
        return None;
    };
    let px = mastik_render::images::decode(bytes, entry.format).ok()?;
    image::RgbaImage::from_raw(px.width, px.height, px.data.data().to_vec())
}

/// `px` encoded as `out` asks.
pub(crate) fn encode(px: &image::RgbaImage, out: Out) -> Option<Encoded> {
    let mut bytes = Vec::new();
    let format = match out {
        Out::Jpeg(q) => {
            let rgb = image::DynamicImage::ImageRgba8(px.clone()).to_rgb8();
            image::codecs::jpeg::JpegEncoder::new_with_quality(&mut bytes, q)
                .encode_image(&rgb)
                .ok()?;
            ImageFormat::Jpeg
        }
        Out::Png => {
            px.write_with_encoder(image::codecs::png::PngEncoder::new(&mut bytes))
                .ok()?;
            ImageFormat::Png
        }
    };
    Some(Encoded {
        bytes,
        format,
        width: px.width(),
        height: px.height(),
    })
}

/// Whether any pixel is less than opaque, which JPEG cannot keep.
pub(crate) fn has_alpha(px: &image::RgbaImage) -> bool {
    px.pixels().any(|p| p.0[3] < 255)
}

/// One size *Reduce picture…* offers: a multiple of the largest drawn size, or
/// the original's own.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct ReduceSize {
    /// `Some(k)` for k× as drawn, `None` for the original.
    pub(crate) times: Option<u32>,
    pub(crate) width: u32,
    pub(crate) height: u32,
}

impl ReduceSize {
    fn label(&self) -> String {
        match self.times {
            Some(k) => format!("{k}× as drawn · {} × {}", self.width, self.height),
            None => format!("Original · {} × {}", self.width, self.height),
        }
    }
}

/// The size *Reduce picture…* opens on: 2× as drawn, or past it the original,
/// which is the nearest size still sharp at 2×.
fn reduce_pick(sizes: &[ReduceSize]) -> usize {
    sizes
        .iter()
        .position(|s| s.times.is_none_or(|k| k >= 2))
        .unwrap_or(sizes.len().saturating_sub(1))
}

/// The most units one source pixel spans along any direction under `m`: the
/// larger singular value of its linear part, so a picture squeezed on one axis
/// reads at the scale of the axis it is drawn largest on.
fn largest_scale(m: mastik_core::kurbo::Affine) -> f64 {
    let [a, b, c, d, _, _] = m.as_coeffs();
    let sum = a * a + b * b + c * c + d * d;
    let det = a * d - b * c;
    ((sum + (sum * sum - 4.0 * det * det).max(0.0).sqrt()) / 2.0).sqrt()
}

/// The sizes offered for a `width` × `height` picture drawn at most `drawn`
/// units a pixel: 1×, 2× and 3× the drawn size while each is smaller than the
/// original, which comes last and only re-encodes. Never larger: a reduce does
/// not upscale.
pub(crate) fn reduce_sizes(width: u32, height: u32, drawn: Option<f64>) -> Vec<ReduceSize> {
    let mut out = Vec::new();
    if let Some(f) = drawn.filter(|f| f.is_finite() && *f > 0.0) {
        for k in 1..=3u32 {
            let s = f * f64::from(k);
            let (w, h) = (
                (f64::from(width) * s).round() as u32,
                (f64::from(height) * s).round() as u32,
            );
            if w >= 1 && h >= 1 && w < width {
                out.push(ReduceSize {
                    times: Some(k),
                    width: w,
                    height: h,
                });
            }
        }
    }
    out.push(ReduceSize {
        times: None,
        width,
        height,
    });
    out
}

/// What *Reduce picture…* encodes with: a size from the list, the format and
/// the quality.
#[derive(Clone, Copy, Debug, PartialEq)]
struct Asked {
    width: u32,
    height: u32,
    out: Out,
}

/// *Reduce picture…*'s state while it is open.
pub(crate) struct ReduceCard {
    layer: NodeId,
    picture: ImageId,
    revision: u64,
    layers: usize,
    sizes: Vec<ReduceSize>,
    size: usize,
    jpeg: bool,
    quality: f64,
    decoding: Option<Job<Option<Arc<image::RgbaImage>>>>,
    pixels: Option<Arc<image::RgbaImage>>,
    unreadable: bool,
    measuring: Option<(Asked, Job<Option<Encoded>>)>,
    measured: Option<(Asked, Arc<Encoded>)>,
}

impl ReduceCard {
    /// Whether the picture has transparency, once read; JPEG is not offered then.
    fn alpha(&self) -> bool {
        self.pixels.as_deref().is_some_and(has_alpha)
    }

    fn asked(&self) -> Asked {
        let s = &self.sizes[self.size.min(self.sizes.len() - 1)];
        Asked {
            width: s.width,
            height: s.height,
            out: match self.jpeg && !self.alpha() {
                true => Out::Jpeg(self.quality.round().clamp(1.0, 100.0) as u8),
                false => Out::Png,
            },
        }
    }

    /// The measured result, when it answers the settings now asked.
    fn done(&self) -> Option<Arc<Encoded>> {
        self.measured
            .as_ref()
            .filter(|(a, _)| *a == self.asked())
            .map(|(_, e)| e.clone())
    }

    /// Take what the workers answered, and start measuring what is now asked
    /// when nothing is measuring it: one encode at a time.
    fn pump(&mut self, ctx: &egui::Context) {
        if let Some(job) = &self.decoding {
            match job.poll() {
                Ok(None) => {}
                Ok(Some(px)) => {
                    self.unreadable = px.is_none();
                    self.pixels = px;
                    self.decoding = None;
                }
                Err(()) => {
                    self.unreadable = true;
                    self.decoding = None;
                }
            }
        }
        if let Some((asked, job)) = &self.measuring {
            match job.poll() {
                Ok(None) => return,
                Ok(Some(e)) => {
                    self.measured = e.map(|e| (*asked, Arc::new(e)));
                    self.measuring = None;
                }
                Err(()) => self.measuring = None,
            }
        }
        let asked = self.asked();
        let Some(px) = self.pixels.clone() else {
            return;
        };
        if self.measured.as_ref().is_some_and(|(a, _)| *a == asked) {
            return;
        }
        let job = Job::spawn("mastik-reduce-measure", ctx, move || {
            let sized = match (asked.width, asked.height) == (px.width(), px.height()) {
                true => (*px).clone(),
                false => image::imageops::resize(
                    &*px,
                    asked.width,
                    asked.height,
                    image::imageops::FilterType::Lanczos3,
                ),
            };
            encode(&sized, asked.out)
        });
        self.measuring = Some((asked, job));
    }
}

/// The card's body: Size, Format and Quality, then the weight now and after.
fn reduce_body(ui: &mut egui::Ui, card: &mut ReduceCard, before: u64) {
    ui.spacing_mut().item_spacing.y = ui::POPOVER_SECTION_GAP;
    let inner = REDUCE_W - 2.0 * crate::settings::CANVAS_CARD_PAD;
    let combo = |ui: &mut egui::Ui, salt: &str, shown: String, rows: &[String], at: &mut usize| {
        ui.scope(|ui| {
            ui.spacing_mut().interact_size.y = ui::CONTROL_H;
            ui.spacing_mut().button_padding.y = 0.0;
            egui::ComboBox::from_id_salt(salt)
                .icon(ui::combo_chevron)
                .width(inner)
                .selected_text(ui::combo_text(&shown))
                .show_ui(ui, |ui| {
                    ui::menu_rows(ui);
                    for (i, row) in rows.iter().enumerate() {
                        if ui.selectable_label(i == *at, row).clicked() {
                            *at = i;
                        }
                    }
                })
        })
        .inner
        .response
    };
    let rows: Vec<String> = card.sizes.iter().map(ReduceSize::label).collect();
    ui::labelled(ui, "Size", |ui| {
        let shown = rows[card.size].clone();
        combo(ui, "reduce-size", shown, &rows, &mut card.size).on_hover_text(
            "Multiples of the largest size the picture is drawn at; 2× stays sharp on a \
             high-density screen",
        )
    });
    let alpha = card.alpha();
    let formats: Vec<String> = match alpha {
        true => vec!["PNG".into()],
        false => vec!["JPEG".into(), "PNG".into()],
    };
    let mut at = usize::from(!card.jpeg || alpha).min(formats.len() - 1);
    ui::labelled(ui, "Format", |ui| {
        let shown = formats[at].clone();
        combo(ui, "reduce-format", shown, &formats, &mut at).on_hover_text(match alpha {
            true => "This picture has transparency, which only PNG keeps",
            false => "JPEG is far smaller for a photograph; PNG keeps every pixel",
        })
    });
    card.jpeg = !alpha && at == 0;
    let jpeg = card.jpeg;
    ui::labelled(ui, "Quality", |ui| {
        ui.scope(|ui| {
            if !jpeg {
                ui.disable();
            }
            ui::value_field(
                ui,
                egui::vec2(inner, ui::CONTROL_H),
                Prefix::Icon(icon::GAUGE),
                &mut card.quality,
                Scrub::whole(0.25).range(1.0..=100.0),
                |d| d.custom_formatter(ui::number(0)),
            )
        })
        .inner
        .on_hover_text(match jpeg {
            true => "Lower is a smaller file. 85 is hard to tell from the original",
            false => "Quality — only JPEG has one",
        })
    });
    let entry_dims = card.sizes.last().map_or((0, 0), |s| (s.width, s.height));
    let asked = card.asked();
    let after = match card.done() {
        Some(e) => format!(
            "After, about {} · {} × {}",
            crate::settings::human_bytes(e.bytes.len() as u64),
            e.width,
            e.height
        ),
        None if card.unreadable => "After: can't be measured".to_string(),
        None => format!("After: measuring… · {} × {}", asked.width, asked.height),
    };
    ui.vertical(|ui| {
        ui.spacing_mut().item_spacing.y = 2.0;
        for (text, ink) in [
            (
                format!(
                    "Now {} · {} × {}",
                    crate::settings::human_bytes(before),
                    entry_dims.0,
                    entry_dims.1
                ),
                theme::text::muted(),
            ),
            (after, theme::text::muted()),
        ] {
            ui.label(egui::RichText::new(text).size(ui::SMALL_PT).color(ink));
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;
    use mastik_core::kurbo::{Affine, Size};
    use mastik_core::{Fill, IdSource, NodeKind, image_brush};

    /// A 4 × 2 PNG whose left half is red and right half blue.
    fn halves() -> Vec<u8> {
        let px = image::RgbaImage::from_fn(4, 2, |x, _| match x < 2 {
            true => image::Rgba([255, 0, 0, 255]),
            false => image::Rgba([0, 0, 255, 255]),
        });
        encode(&px, Out::Png).expect("encodes").bytes
    }

    /// A headless app with a 40 × 20 rect showing `halves` under `fit` and
    /// `crop`, the picture's id the bytes' own hash.
    fn app_with(
        ctx: &egui::Context,
        fit: mastik_core::ImageFit,
        crop: mastik_core::kurbo::Rect,
    ) -> (MastikApp, NodeId, ImageId) {
        let mut app = MastikApp::headless(ctx);
        let mut ids = IdSource::new(0x91C);
        let root = ids.mint();
        let mut doc = Document::new(root);
        let page = doc.first_page();
        let rect = ids.mint();
        let bytes = halves();
        let picture = crate::app::content_hash(&bytes);
        let mut brush = image_brush(picture.clone());
        if let mastik_core::Brush::Image(b) = &mut brush {
            b.image.fit = fit;
            b.image.crop = crop;
        }
        doc.apply(&Transaction(vec![
            Operation::AddImage {
                id: picture.clone(),
                entry: ImageEntry {
                    source: ImageSource::Embedded(bytes.into()),
                    format: ImageFormat::Png,
                    width: 4,
                    height: 2,
                },
            },
            Operation::CreateNode {
                id: rect,
                parent: page,
                index: 0,
                kind: NodeKind::Rect {
                    size: Size::new(40.0, 20.0),
                    corner_radii: Default::default(),
                    corner_smoothing: 0.0,
                },
                transform: Some(Affine::translate((10.0, 10.0))),
                name: Some("Hero".into()),
            },
            Operation::SetFills {
                id: rect,
                fills: mastik_core::keyed_by_position([Fill {
                    brush: brush.into(),
                    visible: true,
                }]),
            },
        ]))
        .expect("build");
        app.session.adopt_document(doc, None);
        (app, rect, picture)
    }

    fn shown(app: &MastikApp, id: NodeId) -> mastik_core::ImageRef {
        let Some(mastik_core::Brush::Image(b)) =
            app.session.doc.get(id).expect("the layer").paint().fills[0]
                .brush
                .literal()
        else {
            panic!("an image fill");
        };
        b.image.clone()
    }

    /// *Apply crop* keeps only the cropped pixels in a picture of the layer's own,
    /// whole and still in Crop, drawn exactly where they were (the fixture's box is
    /// stretched against the cut), takes the original out once nothing shows it,
    /// and one undo brings it all back.
    /// flip: `rewrite_tx` never removing the old picture → fails at "the original goes";
    /// `land_rewrite` switching the mode to Fill → fails at the mode, and the
    /// position check behind it fails too.
    #[test]
    fn apply_crop_keeps_the_cut_and_undo_brings_the_rest_back() {
        let ctx = egui::Context::default();
        let right = mastik_core::kurbo::Rect::new(0.5, 0.0, 1.0, 1.0);
        let (mut app, rect, old) = app_with(&ctx, mastik_core::ImageFit::Crop, right);
        let frame = {
            let s = &app.session;
            mastik_core::local_box(&s.doc, &s.resolved, rect).expect("a box")
        };
        let before = shown(&app, rect).framing(frame, 4, 2).transform;
        app.apply_crop(rect, 0);
        let p = app.picture_job.take().expect("a worker");
        let e = p.job.wait().flatten().expect("encoded");
        app.land_rewrite(&p.target, e);

        let now = shown(&app, rect);
        assert_ne!(now.id, old, "the layer shows a picture of its own");
        let entry = app
            .session
            .doc
            .image(&now.id)
            .expect("in the table")
            .clone();
        assert_eq!((entry.width, entry.height), (2, 2), "only the cut");
        let px = decode_rgba(&entry).expect("decodes");
        assert_eq!(
            px.get_pixel(0, 0).0,
            [0, 0, 255, 255],
            "the right half, blue"
        );
        assert_eq!(now.crop, mastik_core::whole_crop());
        assert_eq!(now.fit, mastik_core::ImageFit::Crop);
        let after = now.framing(frame, 2, 2).transform;
        use mastik_core::kurbo::Point;
        for (was, is) in [((2.0, 0.0), (0.0, 0.0)), ((4.0, 2.0), (2.0, 2.0))] {
            let (a, b) = (before * Point::from(was), after * Point::from(is));
            assert!(
                a.distance(b) < 1e-9,
                "drawn where it was: {a:?} against {b:?}"
            );
        }
        assert!(!app.session.doc.has_image(&old), "the original goes");
        assert!(
            app.session
                .status()
                .text
                .starts_with("Cropped Hero to 2 × 2")
        );

        app.session.undo();
        assert_eq!(shown(&app, rect).id, old, "one undo step");
        assert!(app.session.doc.has_image(&old), "with every byte back");
    }

    /// A second *Apply crop* while the first is on its worker is refused and says
    /// so, rather than dropping the first unlanded. flip: `apply_crop` without
    /// `picture_busy` → fails at "the first job stays".
    #[test]
    fn a_second_apply_crop_waits_for_the_first() {
        let ctx = egui::Context::default();
        let right = mastik_core::kurbo::Rect::new(0.5, 0.0, 1.0, 1.0);
        let (mut app, rect, _) = app_with(&ctx, mastik_core::ImageFit::Crop, right);
        app.apply_crop(rect, 0);
        let first = app.picture_job.as_ref().expect("a worker").target.revision;
        app.session.info("");
        app.apply_crop(rect, 0);
        assert!(app.session.status().text.starts_with("Still rewriting"));
        let p = app.picture_job.take().expect("the first job stays");
        assert_eq!(p.target.revision, first);
        let _ = p.job.wait();
    }

    /// A rewrite lands only on the document it was asked of, and one whose
    /// picture could not be read says so. flip: `poll_pictures` without the
    /// revision check → fails at "the original stays".
    #[test]
    fn a_rewrite_is_dropped_when_the_document_moved_meanwhile() {
        let ctx = egui::Context::default();
        let right = mastik_core::kurbo::Rect::new(0.5, 0.0, 1.0, 1.0);
        let (mut app, rect, old) = app_with(&ctx, mastik_core::ImageFit::Crop, right);
        app.apply_crop(rect, 0);
        let p = app.picture_job.take().expect("a worker");
        let encoded = p.job.wait().flatten();
        assert!(encoded.is_some(), "the fixture: it encodes");
        app.picture_job = Some(PictureJob {
            job: Job::done(encoded),
            target: p.target,
        });
        assert!(app.session.commit(Transaction(vec![Operation::SetName {
            id: rect,
            name: "Renamed".into(),
        }])));
        app.poll_pictures();
        assert_eq!(shown(&app, rect).id, old, "the original stays");
        assert_eq!(
            app.session.status().text,
            "The document changed meanwhile — the picture was left as it was"
        );

        app.apply_crop(rect, 0);
        let p = app.picture_job.take().expect("a worker");
        let _ = p.job.wait();
        app.picture_job = Some(PictureJob {
            job: Job::done(None),
            target: p.target,
        });
        app.poll_pictures();
        assert_eq!(shown(&app, rect).id, old);
        assert_eq!(
            app.session.status().text,
            "The picture could not be read to rewrite"
        );
    }

    /// `picture_weight` reads the largest drawn size across every layer showing
    /// the picture, and counts a layer once, a stroke-only one too. flip:
    /// `d.max` → `d.min` → fails at "the largest".
    #[test]
    fn a_pictures_weight_is_its_largest_drawn_size_over_every_layer() {
        let ctx = egui::Context::default();
        let (mut app, _, old) =
            app_with(&ctx, mastik_core::ImageFit::Fill, mastik_core::whole_crop());
        let mut ids = IdSource::new(0x91E);
        let (big, stroked) = (ids.mint(), ids.mint());
        let page = app.session.page;
        let rect = |w: f64, h: f64| NodeKind::Rect {
            size: Size::new(w, h),
            corner_radii: Default::default(),
            corner_smoothing: 0.0,
        };
        assert!(app.session.commit(Transaction(vec![
            Operation::CreateNode {
                id: big,
                parent: page,
                index: 1,
                kind: rect(80.0, 40.0),
                transform: None,
                name: None,
            },
            Operation::SetFills {
                id: big,
                fills: mastik_core::keyed_by_position([Fill {
                    brush: image_brush(old.clone()).into(),
                    visible: true,
                }]),
            },
            Operation::CreateNode {
                id: stroked,
                parent: page,
                index: 2,
                kind: rect(8.0, 4.0),
                transform: None,
                name: None,
            },
            Operation::SetStrokes {
                id: stroked,
                strokes: mastik_core::keyed_by_position([mastik_core::Stroke {
                    brush: image_brush(old.clone()).into(),
                    ..Default::default()
                }]),
            },
        ])));
        let w = app.picture_weight(&old);
        // 4 × 2 pixels on boxes 40 wide (10 a pixel), 80 (20) and 8 (2).
        assert_eq!(w.drawn, Some(20.0), "the largest");
        assert_eq!(w.layers, 3, "one each, the stroke-only layer too");
    }

    /// The Reduce card encodes PNG for a picture with transparency whatever the
    /// JPEG switch says, and answers only a measurement of the settings now asked.
    /// flip: `asked` ignoring `alpha` → fails at "PNG keeps the transparency".
    #[test]
    fn the_reduce_card_keeps_alpha_and_answers_only_what_is_asked() {
        let ctx = egui::Context::default();
        let (mut app, rect, old) =
            app_with(&ctx, mastik_core::ImageFit::Fill, mastik_core::whole_crop());
        app.open_reduce(rect, old);
        let mut card = app.reduce.take().expect("the card");
        let px = card.decoding.take().expect("decoding").wait().flatten();
        card.pixels = px;
        assert!(
            card.jpeg && matches!(card.asked().out, Out::Jpeg(_)),
            "opaque: JPEG"
        );
        let mut clear = (*card.pixels.clone().unwrap()).clone();
        clear.put_pixel(0, 0, image::Rgba([0, 0, 0, 0]));
        card.pixels = Some(Arc::new(clear.clone()));
        assert_eq!(card.asked().out, Out::Png, "PNG keeps the transparency");
        let e = Arc::new(encode(&clear, Out::Png).expect("encodes"));
        card.measured = Some((card.asked(), e));
        assert!(card.done().is_some(), "the measurement answers");
        card.size = (card.size + 1) % card.sizes.len();
        if card.sizes.len() > 1 {
            assert!(
                card.done().is_none(),
                "another size is not this measurement"
            );
        }
    }

    /// The drawn scale is the largest along any axis, so a picture squeezed on
    /// one axis is not reduced below the size it is drawn on the other; and the
    /// card opens on 2× or, past it, on the original. flip: `largest_scale` as
    /// √det → 0.5 for 1.
    #[test]
    fn the_drawn_scale_is_the_largest_axis_and_the_pick_is_2x_or_original() {
        use mastik_core::kurbo::Affine;
        assert!((largest_scale(Affine::scale_non_uniform(1.0, 0.25)) - 1.0).abs() < 1e-12);
        assert!((largest_scale(Affine::rotate(0.7) * Affine::scale(0.5)) - 0.5).abs() < 1e-12);
        let sizes = reduce_sizes(3000, 2000, Some(0.6));
        assert_eq!(sizes.last().map(|s| s.times), Some(None), "{sizes:?}");
        assert_eq!(
            reduce_pick(&sizes),
            sizes.len() - 1,
            "past 2×, the original"
        );
        let sizes = reduce_sizes(3000, 2000, Some(0.1));
        assert_eq!(sizes[reduce_pick(&sizes)].times, Some(2));
    }

    /// *Reduce* moves every layer and style showing the picture onto the smaller
    /// one, so the file gets lighter: a fill, a stroke, a style and a Tile layer,
    /// whose tiles keep the size they were drawn. flip: `repoint_image` without its
    /// styles loop → the style still names the original, which stays in the table
    /// (fails at `!has_image(&old)`); without the tile scale → at "each tile".
    #[test]
    fn reduce_lands_on_every_layer_showing_the_picture() {
        let ctx = egui::Context::default();
        let (mut app, rect, old) =
            app_with(&ctx, mastik_core::ImageFit::Fill, mastik_core::whole_crop());
        let mut ids = IdSource::new(0x91D);
        let (stroked, tiled) = (ids.mint(), ids.mint());
        let style = mastik_core::style::StyleId(ids.mint());
        let page = app.session.page;
        let mut tile = image_brush(old.clone());
        if let mastik_core::Brush::Image(b) = &mut tile {
            b.image.fit = mastik_core::ImageFit::Tile;
            b.image.tile_scale = 3.0;
        }
        let square = || NodeKind::Rect {
            size: Size::new(40.0, 20.0),
            corner_radii: Default::default(),
            corner_smoothing: 0.0,
        };
        let fill = |brush: mastik_core::Brush| {
            mastik_core::keyed_by_position([Fill {
                brush: brush.into(),
                visible: true,
            }])
        };
        assert!(app.session.commit(Transaction(vec![
            Operation::CreateNode {
                id: stroked,
                parent: page,
                index: 1,
                kind: square(),
                transform: None,
                name: None,
            },
            Operation::SetStrokes {
                id: stroked,
                strokes: mastik_core::keyed_by_position([mastik_core::Stroke {
                    brush: image_brush(old.clone()).into(),
                    ..Default::default()
                }]),
            },
            Operation::CreateNode {
                id: tiled,
                parent: page,
                index: 2,
                kind: square(),
                transform: None,
                name: None,
            },
            Operation::SetFills {
                id: tiled,
                fills: fill(tile),
            },
            Operation::AddStyle {
                style: mastik_core::style::Style {
                    id: style,
                    name: "Texture".into(),
                    value: mastik_core::style::StyleValue::Fills(fill(image_brush(old.clone()))),
                    origin: None,
                    forked: None,
                    description: String::new(),
                },
            },
        ])));
        let entry = app.session.doc.image(&old).expect("there").clone();
        let small = image::imageops::resize(
            &decode_rgba(&entry).expect("decodes"),
            2,
            1,
            image::imageops::FilterType::Lanczos3,
        );
        let e = encode(&small, Out::Png).expect("encodes");
        let target = Target {
            layer: rect,
            fill: 0,
            picture: old.clone(),
            revision: app.session.revision(),
            kind: Rewrite::Reduce,
        };
        app.land_rewrite(&target, e);
        let now = shown(&app, rect);
        let entry = app.session.doc.image(&now.id).expect("in the table");
        assert_eq!((entry.width, entry.height), (2, 1));
        assert!(!app.session.doc.has_image(&old));
        assert!(app.session.status().text.starts_with("Reduced Hero —"));
        let doc = &app.session.doc;
        let picture = |b: &mastik_core::BrushSource| match b.literal() {
            Some(mastik_core::Brush::Image(i)) => i.image.clone(),
            _ => panic!("a picture"),
        };
        let stroke = picture(&doc.get(stroked).unwrap().paint().strokes[0].brush);
        assert_eq!(stroke.id, now.id, "the stroke");
        let tiles = picture(&doc.get(tiled).unwrap().paint().fills[0].brush);
        assert_eq!(tiles.id, now.id, "the tile layer");
        // 4 px wide → 2: each tile drawn from half the pixels, so twice the scale.
        assert_eq!(tiles.tile_scale, 6.0, "each tile the size it was drawn");
        let mastik_core::style::StyleValue::Fills(f) = &doc.styles().get(style).unwrap().value
        else {
            panic!("a fill style")
        };
        assert_eq!(picture(&f[0].brush).id, now.id, "the style");
    }

    /// The sizes offered are multiples of the drawn size below the original,
    /// then the original; a picture drawn at full size offers only the original.
    #[test]
    fn reduce_offers_multiples_of_the_drawn_size_and_never_upscales() {
        let at = |s: &[ReduceSize]| s.iter().map(|s| (s.times, s.width)).collect::<Vec<_>>();
        assert_eq!(
            at(&reduce_sizes(6000, 4000, Some(1180.0 / 6000.0))),
            vec![
                (Some(1), 1180),
                (Some(2), 2360),
                (Some(3), 3540),
                (None, 6000)
            ]
        );
        assert_eq!(at(&reduce_sizes(800, 600, Some(1.0))), vec![(None, 800)]);
        assert_eq!(at(&reduce_sizes(800, 600, None)), vec![(None, 800)]);
    }

    /// The readout keeps two lines whatever it says, turns amber only when the
    /// picture is drawn under a quarter of its pixels, and names a missing one.
    #[test]
    fn the_weight_readout_says_what_the_picture_costs() {
        let entry = ImageEntry {
            source: ImageSource::Embedded(vec![0u8; 3].into()),
            format: ImageFormat::Jpeg,
            width: 6000,
            height: 4000,
        };
        let w = |drawn, layers| Weight {
            entry: Some(entry.clone()),
            embedded: true,
            bytes: 5_000_000,
            layers,
            drawn,
        };
        let (first, second, heavy) = w(Some(0.125), 3).lines();
        assert_eq!(first, "JPEG · 6000 × 4000 · 4.8 MB");
        assert_eq!(second, "Drawn at 1/8 of its pixels · on 3 layers");
        assert!(heavy);
        let (_, second, heavy) = w(Some(1.0), 1).lines();
        assert_eq!(second, "Drawn at 1:1 · on this layer only");
        assert!(!heavy);
        assert!(!w(Some(0.5), 1).lines().2, "half is not heavy");
        let missing = Weight {
            embedded: false,
            ..w(None, 1)
        };
        assert_eq!(missing.lines().0, "Missing · 6000 × 4000 when saved");
    }

    /// *Remove unused pictures* takes out only what nothing shows, as one step.
    #[test]
    fn remove_unused_takes_only_the_orphans() {
        let ctx = egui::Context::default();
        let (mut app, _, used) =
            app_with(&ctx, mastik_core::ImageFit::Fill, mastik_core::whole_crop());
        let orphan = ImageId("sha256:orphan".into());
        app.session.commit(Transaction(vec![Operation::AddImage {
            id: orphan.clone(),
            entry: ImageEntry {
                source: ImageSource::Embedded(vec![0u8; 2048].into()),
                format: ImageFormat::Png,
                width: 1,
                height: 1,
            },
        }]));
        assert_eq!(app.unused_pictures().0, vec![orphan.clone()], "the fixture");
        app.remove_unused_images();
        assert!(!app.session.doc.has_image(&orphan));
        assert!(app.session.doc.has_image(&used));
        assert_eq!(
            app.session.status().text,
            "Removed 1 unused picture — 2.0 KB"
        );
        assert!(
            app.unused_pictures().0.is_empty(),
            "the cache follows the change"
        );
        app.session.undo();
        assert!(app.session.doc.has_image(&orphan), "one undo step");
    }

    /// Taking back a layer's deletion under a later *Remove unused pictures*
    /// brings the layer back with its picture, and undoing that revert takes
    /// both out again.
    /// flip: `History::revert` committing the step without `carried_pictures` →
    /// fails at "the picture comes back".
    #[test]
    fn a_revert_under_a_purge_brings_the_picture_back() {
        let ctx = egui::Context::default();
        let (mut app, rect, picture) =
            app_with(&ctx, mastik_core::ImageFit::Fill, mastik_core::whole_crop());
        assert!(
            app.session
                .commit(Transaction(vec![Operation::DeleteNode { id: rect }]))
        );
        let deleted = app.session.history.last_serial();
        app.remove_unused_images();
        assert!(
            !app.session.doc.has_image(&picture),
            "the fixture purges it"
        );
        app.session.revert(deleted).expect("the revert is exact");
        assert!(app.session.doc.get(rect).is_some(), "the layer comes back");
        assert!(
            app.session.doc.has_image(&picture),
            "the picture comes back"
        );
        app.session.undo();
        assert!(app.session.doc.get(rect).is_none());
        assert!(!app.session.doc.has_image(&picture));
    }

    /// Taking back the step that added a picture keeps the picture while a later
    /// step's layer still names it, and undoing that revert leaves the table as
    /// it was.
    /// flip: `History::revert` carrying only from the unwound document →
    /// fails at "the copy keeps its picture".
    #[test]
    fn a_revert_of_a_placed_picture_keeps_it_for_a_later_copy() {
        let ctx = egui::Context::default();
        let (mut app, _, _) =
            app_with(&ctx, mastik_core::ImageFit::Fill, mastik_core::whole_crop());
        let page = app.session.doc.first_page();
        let placed = ImageId("sha256:placed".into());
        let mut ids = IdSource::new(0x91D);
        let (layer, copy) = (ids.mint(), ids.mint());
        let make = |id: NodeId| {
            vec![
                Operation::CreateNode {
                    id,
                    parent: page,
                    index: 0,
                    kind: NodeKind::Rect {
                        size: Size::new(20.0, 20.0),
                        corner_radii: Default::default(),
                        corner_smoothing: 0.0,
                    },
                    transform: Some(Affine::IDENTITY),
                    name: None,
                },
                Operation::SetFills {
                    id,
                    fills: mastik_core::keyed_by_position([Fill {
                        brush: image_brush(placed.clone()).into(),
                        visible: true,
                    }]),
                },
            ]
        };
        let mut first = vec![Operation::AddImage {
            id: placed.clone(),
            entry: ImageEntry {
                source: ImageSource::Embedded(vec![0u8; 64].into()),
                format: ImageFormat::Png,
                width: 1,
                height: 1,
            },
        }];
        first.extend(make(layer));
        assert!(app.session.commit(Transaction(first)));
        let placing = app.session.history.last_serial();
        assert!(app.session.commit(Transaction(make(copy))));
        app.session.revert(placing).expect("the revert is exact");
        assert!(app.session.doc.get(layer).is_none(), "the layer goes");
        assert!(app.session.doc.get(copy).is_some(), "the copy stays");
        assert!(
            app.session.doc.has_image(&placed),
            "the copy keeps its picture"
        );
        app.session.undo();
        assert!(app.session.doc.get(layer).is_some());
        assert!(app.session.doc.has_image(&placed));
    }
}
