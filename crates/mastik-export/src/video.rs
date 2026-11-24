//! Video from a prototype's take: the take replayed on a fresh copy of the
//! document, frame by frame at a fixed rate, each frame drawn by the CPU
//! backend and encoded as an animated GIF or an H.264 MP4. Nothing is captured
//! from a screen, so a video is the same wherever it is made.
//!
//! MP4 goes through OpenH264, built from source, and the `mp4` crate's muxer;
//! GIF through the `gif` crate, each frame quantised to its own palette and a
//! run of identical frames written once.

use mastik_core::Document;
use mastik_play::{Replay, StartError, Take};
use mastik_render::{ImageStore, VelloCpuRenderer};

/// The container and codec a video is written in.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Format {
    /// An animated GIF that loops, frame times rounded to its hundredths.
    Gif,
    /// H.264 in an MP4, every frame a sample of the same length.
    Mp4,
}

/// How a video is made.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Options {
    pub format: Format,
    /// Frames a second, 1 to [`MAX_FPS`].
    pub fps: u32,
    /// Pixels per screen unit, more than 0; lowered for the first screen so
    /// its frame keeps to [`MAX_SIDE`] and, as an MP4, to H.264's largest
    /// frame ([`frame_size`]), and kept for the whole video.
    pub scale: f64,
    /// A soft ring drawn where each click landed, widening and fading over
    /// [`CLICK_MS`].
    pub clicks: bool,
    /// The screen inside a device's bezel, its corners rounded, on white.
    pub device: bool,
}

/// How long a click's ring shows, in milliseconds.
pub const CLICK_MS: f64 = 500.0;
/// The device frame's bezel round the screen, in screen units: the exported
/// page's.
pub const BEZEL: f64 = 14.0;
/// The bezel's ink.
const BEZEL_INK: [u8; 3] = [12, 12, 13];
/// A click ring's ink: the accent.
const RING_INK: [u8; 3] = [109, 140, 217];

/// A device screen's corner radius in screen units: a phone's (shorter side
/// under 600) rounder than a tablet's, as the exported page draws them.
pub fn screen_radius(size: (f64, f64)) -> f64 {
    match size.0.min(size.1) < 600.0 {
        true => 40.0,
        false => 18.0,
    }
}

/// A frame's size in pixels for a screen of `screen` pixels at `scale`, with or
/// without the device frame.
pub fn framed_size(screen: (u32, u32), scale: f64, device: bool) -> (u32, u32) {
    let b = bezel_px(scale, device);
    (screen.0 + 2 * b, screen.1 + 2 * b)
}

fn bezel_px(scale: f64, device: bool) -> u32 {
    match device {
        true => (BEZEL * scale).round() as u32,
        false => 0,
    }
}

/// How much of the pixel centred at (`x`, `y`) lies inside a rectangle from the
/// origin to `size` with corners of `radius`: 0 to 1, a pixel's width of
/// anti-aliasing at the edge.
fn rounded_cover(x: f64, y: f64, size: (f64, f64), radius: f64) -> f64 {
    let r = radius.min(size.0 / 2.0).min(size.1 / 2.0).max(0.0);
    let qx = (x - size.0 / 2.0).abs() - (size.0 / 2.0 - r);
    let qy = (y - size.1 / 2.0).abs() - (size.1 / 2.0 - r);
    let outside = qx.max(0.0).hypot(qy.max(0.0));
    let d = outside + qx.max(qy).min(0.0) - r;
    (0.5 - d).clamp(0.0, 1.0)
}

/// `a` over `b` by `t`, per channel.
fn mix(a: [u8; 3], b: &mut [u8], t: f64) {
    for c in 0..3 {
        let v = f64::from(a[c]) * t + f64::from(b[c]) * (1.0 - t);
        b[c] = v.round().clamp(0.0, 255.0) as u8;
    }
}

/// Draw on the opaque `frame` (`size` pixels) a ring for each click of `take`
/// still showing at `now`, at `scale` pixels a unit.
fn draw_clicks(frame: &mut [u8], size: (u32, u32), take: &Take, now: f64, scale: f64) {
    for (at, input) in &take.inputs {
        let mastik_play::Input::PointerDown(p) = input else {
            continue;
        };
        let age = now - at;
        if !(0.0..CLICK_MS).contains(&age) {
            continue;
        }
        let k = age / CLICK_MS;
        let (cx, cy) = (p.x * scale, p.y * scale);
        let radius = (10.0 + 14.0 * k) * scale;
        let half = 1.5 * scale;
        let alpha = 0.7 * (1.0 - k);
        let reach = radius + half + 1.0;
        let x0 = (cx - reach).floor().max(0.0) as u32;
        let y0 = (cy - reach).floor().max(0.0) as u32;
        let x1 = ((cx + reach).ceil().max(0.0) as u32).min(size.0);
        let y1 = ((cy + reach).ceil().max(0.0) as u32).min(size.1);
        for y in y0..y1 {
            for x in x0..x1 {
                let d = (f64::from(x) + 0.5 - cx).hypot(f64::from(y) + 0.5 - cy);
                let cover = (half + 0.5 - (d - radius).abs()).clamp(0.0, 1.0);
                if cover > 0.0 {
                    let i = ((y * size.0 + x) * 4) as usize;
                    mix(RING_INK, &mut frame[i..i + 3], cover * alpha);
                }
            }
        }
    }
}

/// The opaque `screen` (`size` pixels) inside a device frame at `scale`: the
/// bezel round it, both rounded, on white.
fn in_device(screen: &[u8], size: (u32, u32), scale: f64) -> Vec<u8> {
    let b = bezel_px(scale, true);
    let (w, h) = (size.0 + 2 * b, size.1 + 2 * b);
    let r = screen_radius((f64::from(size.0) / scale, f64::from(size.1) / scale)) * scale;
    let outer = (f64::from(w), f64::from(h));
    let inner = (f64::from(size.0), f64::from(size.1));
    let mut out = vec![255u8; (w * h * 4) as usize];
    for y in 0..h {
        for x in 0..w {
            let i = ((y * w + x) * 4) as usize;
            let (px, py) = (f64::from(x) + 0.5, f64::from(y) + 0.5);
            let body = rounded_cover(px, py, outer, r + f64::from(b));
            if body <= 0.0 {
                continue;
            }
            mix(BEZEL_INK, &mut out[i..i + 3], body);
            let (sx, sy) = (x.wrapping_sub(b), y.wrapping_sub(b));
            if sx < size.0 && sy < size.1 {
                let glass = rounded_cover(px - f64::from(b), py - f64::from(b), inner, r);
                if glass > 0.0 {
                    let s = ((sy * size.0 + sx) * 4) as usize;
                    let ink = [screen[s], screen[s + 1], screen[s + 2]];
                    mix(ink, &mut out[i..i + 3], glass);
                }
            }
        }
    }
    out
}

/// The most frames a second a video takes.
pub const MAX_FPS: u32 = 60;
/// The longest side of a video frame, bezel included, in pixels: H.264's
/// highest levels and a GIF viewer both stop well short of a canvas export's
/// size.
pub const MAX_SIDE: u32 = 4096;
/// The most 16-pixel macroblocks an MP4 frame holds: H.264's frame size limit
/// at levels 5.1 and 5.2 (4096 × 2304), the highest OpenH264 writes. A larger
/// frame is still labelled 5.1, and a decoder that keeps to it refuses it.
const MAX_MACROBLOCKS: u32 = 36_864;
/// The most macroblocks a second H.264's level 5.2 decodes.
const MAX_MACROBLOCK_RATE: u32 = 2_073_600;

/// Why a video was not made.
#[derive(Debug, thiserror::Error)]
pub enum VideoError {
    #[error("the take does not start: {0}")]
    Start(#[from] StartError),
    #[error("the frame rate is 1 to {MAX_FPS} and the scale more than 0")]
    Options,
    #[error("the screen has no size to draw")]
    Empty,
    #[error("stopped")]
    Cancelled,
    #[error("the encoder failed: {0}")]
    Encode(String),
}

/// How many frames a video of `take` at `fps` has: one at every frame time from
/// the start to the take's end, both included.
pub fn frame_count(take: &Take, fps: u32) -> usize {
    (take.length.max(0.0) * f64::from(fps.max(1)) / 1000.0).floor() as usize + 1
}

/// `take` replayed on a copy of `doc` as a video, its frames drawn at
/// `options.scale` over white. `actor` is the replay's own, as a player's is.
/// `progress` hears each frame drawn, as (done, of), and stops the video by
/// answering `false`: a long take is a long job.
pub fn video(
    doc: &Document,
    take: &Take,
    actor: u64,
    options: &Options,
    progress: &mut dyn FnMut(usize, usize) -> bool,
) -> Result<Vec<u8>, VideoError> {
    if !(1..=MAX_FPS).contains(&options.fps) || !(options.scale.is_finite() && options.scale > 0.0)
    {
        return Err(VideoError::Options);
    }
    let mut replay = Replay::new(doc, take.clone(), actor)?;
    let mut images = ImageStore::new();
    images.prepare(doc);
    let mut renderer = VelloCpuRenderer::new();
    let total = frame_count(take, options.fps);
    let step = 1000.0 / f64::from(options.fps);
    let mut sink = match options.format {
        Format::Gif => Sink::Gif(None),
        Format::Mp4 => Sink::Mp4(None),
    };
    let mut first: Option<Fixed> = None;
    for i in 0..total {
        if i > 0 {
            // Each frame at its own time from the start, so rounding never
            // drifts.
            let at = i as f64 * step;
            replay.advance(at - replay.player().now());
        }
        let (frame, out, fixed) = draw(&replay, &mut renderer, &images, take, options, first)?;
        first.get_or_insert(fixed);
        sink.push(frame, out, i, options.fps)?;
        if !progress(i + 1, total) {
            return Err(VideoError::Cancelled);
        }
    }
    sink.finish(options.fps)
}

/// One frame of the video of `take` under `options`, `at` milliseconds in, and
/// its size: what the export dialog previews.
pub fn still(
    doc: &Document,
    take: &Take,
    actor: u64,
    options: &Options,
    at: f64,
) -> Result<(Vec<u8>, (u32, u32)), VideoError> {
    if !(options.scale.is_finite() && options.scale > 0.0) {
        return Err(VideoError::Options);
    }
    let mut replay = Replay::new(doc, take.clone(), actor)?;
    replay.advance(at.clamp(0.0, take.length.max(0.0)));
    let mut images = ImageStore::new();
    images.prepare(doc);
    let mut renderer = VelloCpuRenderer::new();
    let (frame, out, _) = draw(&replay, &mut renderer, &images, take, options, None)?;
    Ok((frame, out))
}

/// The size of the screen `take` starts on, in screen units: what a video of it
/// is drawn at, before its scale.
pub fn screen_size(doc: &Document, take: &Take, actor: u64) -> Option<mastik_core::kurbo::Size> {
    let replay = Replay::new(doc, take.clone(), actor).ok()?;
    Some(replay.player().scene().size)
}

/// The screen's size in pixels within a frame, and the pixels a unit it is
/// drawn at: the first frame's, which every frame after it keeps.
#[derive(Clone, Copy)]
struct Fixed {
    screen: (u32, u32),
    scale: f64,
}

/// A drawn frame: opaque RGBA, its size, and its [`Fixed`].
type Drawn = (Vec<u8>, (u32, u32), Fixed);

/// The frame `replay` shows now, drawn and dressed as `options` asks. Given
/// `fixed`, the screen is drawn at its scale into a screen of its size, as
/// every frame after the first is: a screen of another size goes in its
/// top-left corner, cut off or padded white, so the frame's size, the bezel
/// and the content's scale never change mid-video.
fn draw(
    replay: &Replay,
    renderer: &mut VelloCpuRenderer,
    images: &ImageStore,
    take: &Take,
    options: &Options,
    fixed: Option<Fixed>,
) -> Result<Drawn, VideoError> {
    let p = replay.player();
    let scene = p.scene();
    let scale = fixed.map_or_else(|| scale_for(options, scene.size), |f| f.scale);
    // Never past what the backend draws: a far larger later screen is cut to
    // the first one's size anyway.
    let side = scene.size.width.max(scene.size.height);
    let drawn_at = scale.min(f64::from(crate::png::MAX_RASTER_SIDE) / side);
    let (rgba, w, h) =
        renderer.render_play_to_rgba(p.document(), p.resolved(), images, &scene, drawn_at);
    if w == 0 || h == 0 {
        return Err(VideoError::Empty);
    }
    let screen = fixed.map_or((w, h), |f| f.screen);
    let fixed = Fixed { screen, scale };
    let mut frame = fit(&rgba, (w, h), screen);
    if options.clicks {
        draw_clicks(&mut frame, screen, take, p.now(), drawn_at);
    }
    Ok(match options.device {
        true => (
            in_device(&frame, screen, scale),
            framed_size(screen, scale, true),
            fixed,
        ),
        false => (frame, screen, fixed),
    })
}

/// `options.scale` lowered so the frame of a screen of `size`, with its bezel
/// when `options.device`, is at most [`MAX_SIDE`] pixels a side and, as an
/// MP4, at most [`MAX_MACROBLOCKS`] and [`MAX_MACROBLOCK_RATE`] at its fps.
fn scale_for(options: &Options, size: mastik_core::kurbo::Size) -> f64 {
    let bezel = match options.device {
        true => 2.0 * BEZEL,
        false => 0.0,
    };
    let (w, h) = (size.width + bezel, size.height + bezel);
    let mut scale = options.scale.min(f64::from(MAX_SIDE) / w.max(h));
    let most = match options.format {
        Format::Mp4 => MAX_MACROBLOCKS.min(MAX_MACROBLOCK_RATE / options.fps.max(1)),
        Format::Gif => u32::MAX,
    };
    scale = scale.min(16.0 * (f64::from(most) / (w * h)).sqrt());
    // The pixel sizes round, so the bound above can still be a pixel over.
    let fits = |s: f64| {
        let frame = framed_px(size, s, options.device);
        let blocks = u64::from(frame.0.div_ceil(16)) * u64::from(frame.1.div_ceil(16));
        frame.0.max(frame.1) <= MAX_SIDE && blocks <= u64::from(most)
    };
    for _ in 0..64 {
        if fits(scale) {
            break;
        }
        scale *= 0.998;
    }
    scale
}

/// The frame of a screen of `size` units at `scale`, in pixels, with its bezel
/// when `device`: the screen's pixels rounded as the renderer rounds them.
fn framed_px(size: mastik_core::kurbo::Size, scale: f64, device: bool) -> (u32, u32) {
    let px = |v: f64| ((v * scale).round() as u32).max(1);
    framed_size((px(size.width), px(size.height)), scale, device)
}

/// The size in pixels of a video's frames under `options` when its take starts
/// on a screen of `size` units: the scale lowered as the caps ask, the bezel,
/// and an MP4's cut to even sizes. What the export card shows.
pub fn frame_size(options: &Options, size: mastik_core::kurbo::Size) -> (u32, u32) {
    let (w, h) = framed_px(size, scale_for(options, size), options.device);
    match options.format {
        Format::Mp4 => (w & !1, h & !1),
        Format::Gif => (w, h),
    }
}

/// `rgba`, straight alpha at `from`, over white and placed at the top-left of a
/// frame of `to`: opaque RGBA.
fn fit(rgba: &[u8], from: (u32, u32), to: (u32, u32)) -> Vec<u8> {
    let mut out = vec![255u8; (to.0 * to.1 * 4) as usize];
    for y in 0..from.1.min(to.1) {
        for x in 0..from.0.min(to.0) {
            let s = ((y * from.0 + x) * 4) as usize;
            let d = ((y * to.0 + x) * 4) as usize;
            let a = u32::from(rgba[s + 3]);
            for c in 0..3 {
                let v = u32::from(rgba[s + c]) * a + 255 * (255 - a);
                out[d + c] = ((v + 127) / 255) as u8;
            }
            out[d + 3] = 255;
        }
    }
    out
}

/// Where the frames go.
enum Sink {
    Gif(Option<Gif>),
    Mp4(Option<Box<Mp4>>),
}

/// A GIF being written: the encoder, and the frame not written yet with its
/// size and its delay so far. A frame the same as the one before it adds its
/// delay to that one instead of being written again, so a screen nothing
/// moves on costs one frame, not one per frame time.
struct Gif {
    encoder: gif::Encoder<Vec<u8>>,
    held: Option<(Vec<u8>, (u16, u16), u16)>,
}

impl Gif {
    /// Write the held frame, if any.
    fn flush(&mut self) -> Result<(), gif::EncodingError> {
        if let Some((mut frame, (w, h), delay)) = self.held.take() {
            let mut f = gif::Frame::from_rgba_speed(w, h, &mut frame, 10);
            f.delay = delay;
            self.encoder.write_frame(&f)?;
        }
        Ok(())
    }
}

/// An MP4 being written: the encoder, the parameter sets from its first frame,
/// and each frame's NAL units, length-prefixed.
struct Mp4 {
    encoder: openh264::encoder::Encoder,
    size: (u32, u32),
    sps: Vec<u8>,
    pps: Vec<u8>,
    samples: Vec<(Vec<u8>, bool)>,
}

impl Sink {
    fn push(
        &mut self,
        frame: Vec<u8>,
        size: (u32, u32),
        i: usize,
        fps: u32,
    ) -> Result<(), VideoError> {
        let encode = |e: &dyn std::fmt::Display| VideoError::Encode(e.to_string());
        match self {
            Sink::Gif(enc) => {
                let (w, h) = (
                    u16::try_from(size.0).map_err(|e| encode(&e))?,
                    u16::try_from(size.1).map_err(|e| encode(&e))?,
                );
                if enc.is_none() {
                    let mut e = gif::Encoder::new(Vec::new(), w, h, &[]).map_err(|e| encode(&e))?;
                    e.set_repeat(gif::Repeat::Infinite)
                        .map_err(|e| encode(&e))?;
                    *enc = Some(Gif {
                        encoder: e,
                        held: None,
                    });
                }
                let Some(g) = enc.as_mut() else {
                    return Ok(());
                };
                let delay = gif_delay(i, fps);
                if let Some((held, held_size, so_far)) = &mut g.held
                    && *held_size == (w, h)
                    && *held == frame
                    && let Some(longer) = so_far.checked_add(delay)
                {
                    *so_far = longer;
                    return Ok(());
                }
                g.flush().map_err(|e| encode(&e))?;
                g.held = Some((frame, (w, h), delay));
                Ok(())
            }
            Sink::Mp4(mp4) => {
                if mp4.is_none() {
                    let config = openh264::encoder::EncoderConfig::new()
                        .max_frame_rate(openh264::encoder::FrameRate::from_hz(fps as f32))
                        .usage_type(openh264::encoder::UsageType::ScreenContentRealTime)
                        .skip_frames(false)
                        .bitrate(openh264::encoder::BitRate::from_bps(bitrate(size, fps)));
                    let encoder = openh264::encoder::Encoder::with_api_config(
                        openh264::OpenH264API::from_source(),
                        config,
                    )
                    .map_err(|e| encode(&e))?;
                    *mp4 = Some(Box::new(Mp4 {
                        encoder,
                        // H.264 codes 4:2:0 in even sizes.
                        size: (size.0 & !1, size.1 & !1),
                        sps: Vec::new(),
                        pps: Vec::new(),
                        samples: Vec::new(),
                    }));
                }
                let Some(m) = mp4.as_mut() else {
                    return Ok(());
                };
                let even = fit(&frame, size, m.size);
                let rgba = openh264::formats::RgbaSliceU8::new(
                    &even,
                    (m.size.0 as usize, m.size.1 as usize),
                );
                let yuv = openh264::formats::YUVBuffer::from_rgb_source(rgba);
                let stream = m.encoder.encode(&yuv).map_err(|e| encode(&e))?.to_vec();
                let (mut sample, mut sync) = (Vec::new(), false);
                for nal in openh264::nal_units(&stream) {
                    let body = strip_start_code(nal);
                    match body.first().map(|b| b & 0x1f) {
                        Some(7) => m.sps = body.to_vec(),
                        Some(8) => m.pps = body.to_vec(),
                        Some(kind) => {
                            sync |= kind == 5;
                            sample.extend_from_slice(&(body.len() as u32).to_be_bytes());
                            sample.extend_from_slice(body);
                        }
                        None => {}
                    }
                }
                m.samples.push((sample, sync));
                Ok(())
            }
        }
    }

    fn finish(self, fps: u32) -> Result<Vec<u8>, VideoError> {
        let encode = |e: &dyn std::fmt::Display| VideoError::Encode(e.to_string());
        match self {
            Sink::Gif(enc) => {
                let mut g = enc.ok_or(VideoError::Empty)?;
                g.flush().map_err(|e| encode(&e))?;
                g.encoder.into_inner().map_err(|e| encode(&e))
            }
            Sink::Mp4(m) => {
                let m = m.ok_or(VideoError::Empty)?;
                // The muxer indexes into both and panics on an empty one.
                if m.sps.len() < 4 || m.pps.is_empty() {
                    return Err(VideoError::Encode("no parameter sets".into()));
                }
                let config = mp4::Mp4Config {
                    major_brand: str::parse("isom").map_err(|e| encode(&e))?,
                    minor_version: 512,
                    compatible_brands: ["isom", "iso2", "avc1", "mp41"]
                        .iter()
                        .map(|b| str::parse(b))
                        .collect::<Result<_, _>>()
                        .map_err(|e| encode(&e))?,
                    timescale: fps,
                };
                let mut w = mp4::Mp4Writer::write_start(std::io::Cursor::new(Vec::new()), &config)
                    .map_err(|e| encode(&e))?;
                w.add_track(&mp4::TrackConfig {
                    track_type: mp4::TrackType::Video,
                    timescale: fps,
                    language: "und".into(),
                    media_conf: mp4::MediaConfig::AvcConfig(mp4::AvcConfig {
                        width: m.size.0 as u16,
                        height: m.size.1 as u16,
                        seq_param_set: m.sps,
                        pic_param_set: m.pps,
                    }),
                })
                .map_err(|e| encode(&e))?;
                for (i, (bytes, is_sync)) in m.samples.into_iter().enumerate() {
                    w.write_sample(
                        1,
                        &mp4::Mp4Sample {
                            start_time: i as u64,
                            duration: 1,
                            rendering_offset: 0,
                            is_sync,
                            bytes: bytes::Bytes::from(bytes),
                        },
                    )
                    .map_err(|e| encode(&e))?;
                }
                w.write_end().map_err(|e| encode(&e))?;
                Ok(w.into_writer().into_inner())
            }
        }
    }
}

/// Frame `i`'s delay in a GIF's hundredths of a second at `fps`: the rounding
/// carried, so the frames add up to the take's length. The times are counted
/// wide: only one frame's delay has to fit a `u16`.
fn gif_delay(i: usize, fps: u32) -> u16 {
    let at = |n: usize| (n as f64 * 100.0 / f64::from(fps.max(1))).round() as u64;
    u16::try_from(at(i + 1) - at(i)).unwrap_or(u16::MAX)
}

/// A bit rate for screens of `size` at `fps`: about a tenth of a bit a pixel a
/// frame, enough for flat interface colour.
fn bitrate(size: (u32, u32), fps: u32) -> u32 {
    let pixels = u64::from(size.0) * u64::from(size.1) * u64::from(fps);
    (pixels / 10).clamp(500_000, 40_000_000) as u32
}

/// A NAL unit without the Annex B start code `openh264::nal_units` leaves on it.
fn strip_start_code(nal: &[u8]) -> &[u8] {
    let zeros = nal.iter().take_while(|b| **b == 0).count();
    match nal.get(zeros) {
        Some(1) if zeros >= 2 => &nal[zeros + 1..],
        _ => nal,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A GIF's delays carry their rounding: at 30 fps three frames make ten
    /// hundredths.
    #[test]
    fn gif_delays_add_up() {
        let sum: u32 = (0..3).map(|i| u32::from(gif_delay(i, 30))).sum();
        assert_eq!(sum, 10);
    }

    /// A frame past 655 s still has its delay: the times are not counted in a
    /// `u16`.
    ///
    /// flip: `gif_delay` measuring time in a `u16` → fails at "past 655 s".
    #[test]
    fn a_late_frame_keeps_its_delay() {
        assert_eq!(gif_delay(20_000, 30), 3, "past 655 s");
    }

    /// An MP4 of a desktop, a tablet or a square screen at 3×, with or without
    /// the device frame, keeps to H.264's largest frame and its rate at 60 fps;
    /// a GIF keeps only to the side.
    ///
    /// flip: `scale_for` capping only the screen's side → fails at "the
    /// macroblocks".
    #[test]
    fn an_mp4_frame_keeps_to_the_level() {
        use mastik_core::kurbo::Size;
        for (w, h) in [(1440.0, 900.0), (1024.0, 1366.0), (1080.0, 1080.0)] {
            for device in [false, true] {
                for fps in [30, 60] {
                    let options = Options {
                        format: Format::Mp4,
                        fps,
                        scale: 3.0,
                        clicks: false,
                        device,
                    };
                    let s = scale_for(&options, Size::new(w, h));
                    let b = bezel_px(s, device);
                    let px = |v: f64| (v * s).round() as u32;
                    let frame = (px(w) + 2 * b, px(h) + 2 * b);
                    let blocks = frame.0.div_ceil(16) * frame.1.div_ceil(16);
                    assert!(
                        blocks <= MAX_MACROBLOCKS && blocks * fps <= MAX_MACROBLOCK_RATE,
                        "the macroblocks of {frame:?} at {fps}"
                    );
                    assert!(frame.0.max(frame.1) <= MAX_SIDE, "the side, bezel and all");
                    assert!(
                        blocks > MAX_MACROBLOCKS.min(MAX_MACROBLOCK_RATE / fps) * 9 / 10,
                        "no smaller than it needs to be: {frame:?}"
                    );
                }
            }
            let gif = Options {
                format: Format::Gif,
                fps: 30,
                scale: 3.0,
                clicks: false,
                device: false,
            };
            let side = w.max(h) * scale_for(&gif, Size::new(w, h));
            let want = (w.max(h) * 3.0).min(4096.0);
            assert!((side - want).abs() < 0.5, "a GIF keeps to the side: {side}");
        }
    }

    /// The device frame puts the screen inside a dark bezel: white outside the
    /// rounded body, bezel at its edge, the screen's own pixels in its middle,
    /// and the screen's corner cut by its radius.
    /// flip: `in_device` skipping the glass mask → fails at "the corner is bezel".
    #[test]
    fn a_device_frame_rounds_the_screen_inside_a_bezel() {
        let (w, h) = (200u32, 400u32);
        let red = [255u8, 0, 0, 255].repeat((w * h) as usize);
        let out = in_device(&red, (w, h), 1.0);
        let (ow, oh) = framed_size((w, h), 1.0, true);
        assert_eq!((ow, oh), (228, 428));
        let px = |x: u32, y: u32| {
            let i = ((y * ow + x) * 4) as usize;
            [out[i], out[i + 1], out[i + 2]]
        };
        assert_eq!(px(0, 0), [255, 255, 255], "outside the body is white");
        assert_eq!(px(1, oh / 2), BEZEL_INK, "the bezel");
        assert_eq!(px(ow / 2, oh / 2), [255, 0, 0], "the screen");
        // 47 units from the corners' common centre (54, 54): inside the body's
        // 54, outside the screen's 40.
        assert_eq!(px(21, 21), BEZEL_INK, "the corner is bezel");
    }

    /// A click's ring shows round where it landed while it is young and is gone
    /// after [`CLICK_MS`].
    #[test]
    fn a_click_ring_shows_then_goes() {
        let size = (100u32, 100u32);
        let take = Take {
            start: mastik_play::Start::Frame(mastik_core::IdSource::new(1).mint()),
            inputs: vec![(
                0.0,
                mastik_play::Input::PointerDown(mastik_core::kurbo::Point::new(50.0, 50.0)),
            )],
            length: 1000.0,
        };
        let white = || vec![255u8; (size.0 * size.1 * 4) as usize];
        let mut young = white();
        draw_clicks(&mut young, size, &take, 0.0, 1.0);
        let i = ((50 * size.0 + 60) * 4) as usize;
        assert!(young[i] < 255, "the ring at its radius");
        assert_eq!(young[((50 * size.0 + 50) * 4) as usize], 255, "hollow");
        let mut old = white();
        draw_clicks(&mut old, size, &take, CLICK_MS, 1.0);
        assert_eq!(old, white(), "gone");
    }
}
