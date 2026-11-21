//! Video from a take: replayed frame by frame, encoded as GIF and MP4.

use mastik_core::kurbo::{Affine, Point, RoundedRectRadii, Size};
use mastik_core::peniko::Color;
use mastik_core::prototype::{Action, Reaction, Transition, Trigger};
use mastik_core::{
    Brush, Document, Fill, IdSource, Keyed, NodeId, NodeKind, Operation, Transaction,
    keyed_by_position,
};
use mastik_export::video::{Format, Options, VideoError, frame_count, video};
use mastik_play::{Input, Start, Take};

/// The replay's own actor.
const ACTOR: u64 = 0x91A7;

fn frame(doc: &mut Document, ids: &mut IdSource, page: NodeId, x: f64, rgb: [u8; 3]) -> NodeId {
    let id = ids.mint();
    doc.apply(&Transaction(vec![
        Operation::CreateNode {
            id,
            parent: page,
            index: doc.get(page).map_or(0, |p| p.children().len()),
            kind: NodeKind::Artboard {
                size: Size::new(40.0, 30.0),
                corner_radii: RoundedRectRadii::default(),
                corner_smoothing: 0.0,
            },
            transform: Some(Affine::translate((x, 0.0))),
            name: Some("Screen".into()),
        },
        Operation::SetFills {
            id,
            fills: keyed_by_position([Fill {
                brush: Brush::Solid(Color::from_rgba8(rgb[0], rgb[1], rgb[2], 255)).into(),
                visible: true,
            }]),
        },
    ]))
    .expect("a frame");
    id
}

/// A red frame whose click goes to a blue one at once, and a take that clicks
/// at 100 ms and ends at 200 ms.
fn red_to_blue() -> (Document, Take) {
    let mut ids = IdSource::new(0x7E57);
    let page = ids.mint();
    let mut doc = Document::new(page);
    let a = frame(&mut doc, &mut ids, page, 0.0, [255, 0, 0]);
    let b = frame(&mut doc, &mut ids, page, 100.0, [0, 0, 255]);
    doc.apply(&Transaction(vec![Operation::SetReactions {
        id: a,
        reactions: vec![Keyed::new(
            ids.mint_item(),
            Reaction {
                trigger: Trigger::Click,
                actions: vec![Keyed::new(
                    ids.mint_item(),
                    Action::Navigate {
                        to: b,
                        transition: Transition::INSTANT,
                        keep_scroll: false,
                    },
                )],
            },
        )],
    }]))
    .expect("a link");
    let at = Point::new(10.0, 10.0);
    let take = Take {
        start: Start::Frame(a),
        inputs: vec![
            (100.0, Input::PointerMove(at)),
            (100.0, Input::PointerDown(at)),
            (100.0, Input::PointerUp(at)),
        ],
        length: 200.0,
    };
    (doc, take)
}

fn options(format: Format) -> Options {
    Options {
        format,
        fps: 10,
        scale: 1.0,
        clicks: false,
        device: false,
    }
}

fn is_red(p: &[u8]) -> bool {
    p[0] > 200 && p[1] < 60 && p[2] < 60
}

fn is_blue(p: &[u8]) -> bool {
    p[0] < 60 && p[1] < 60 && p[2] > 200
}

/// A GIF has a frame at every tenth of a second from the start to the take's
/// end, red before the click and blue from it, each a tenth long.
///
/// flip: `video` drawing every frame at the start time → fails at "blue at the
/// end".
#[test]
fn a_take_becomes_a_gif() {
    let (doc, take) = red_to_blue();
    assert_eq!(frame_count(&take, 10), 3);
    let mut heard = Vec::new();
    let gif = video(
        &doc,
        &take,
        ACTOR,
        &options(Format::Gif),
        &mut |done, of| {
            heard.push((done, of));
            true
        },
    )
    .expect("a gif");
    assert_eq!(&gif[..6], b"GIF89a");
    assert_eq!(heard, vec![(1, 3), (2, 3), (3, 3)], "progress per frame");
    let mut opts = gif::DecodeOptions::new();
    opts.set_color_output(gif::ColorOutput::RGBA);
    let mut decoder = opts.read_info(&gif[..]).expect("it reads back");
    let mut frames = Vec::new();
    while let Some(f) = decoder.read_next_frame().expect("a frame") {
        assert_eq!((f.width, f.height), (40, 30));
        frames.push((f.buffer.to_vec(), f.delay));
    }
    assert_eq!(frames.len(), 3);
    assert!(is_red(&frames[0].0[..4]), "red at the start");
    assert!(is_blue(&frames[2].0[..4]), "blue at the end");
    assert!(frames.iter().all(|(_, delay)| *delay == 10));
}

/// An MP4 holds one H.264 sample a frame at the frame rate, sized evenly, and
/// decodes back to red, then blue.
///
/// flip: `Sink::push` keeping no parameter sets → fails at "an mp4", which
/// `Sink::finish` refuses rather than let the muxer panic.
#[test]
fn a_take_becomes_an_mp4() {
    let (doc, take) = red_to_blue();
    let mp4 = video(&doc, &take, ACTOR, &options(Format::Mp4), &mut |_, _| true).expect("an mp4");
    assert_eq!(&mp4[4..8], b"ftyp");
    let size = mp4.len() as u64;
    let mut reader =
        mp4::Mp4Reader::read_header(std::io::Cursor::new(mp4), size).expect("it reads back");
    let track = reader.tracks().get(&1).expect("a video track");
    assert_eq!((track.width(), track.height()), (40, 30));
    let (sps, pps) = (
        track.sequence_parameter_set().unwrap().to_vec(),
        track.picture_parameter_set().unwrap().to_vec(),
    );
    assert_eq!(reader.sample_count(1).unwrap(), 3);
    let mut decoder = openh264::decoder::Decoder::new().expect("a decoder");
    let annex_b = |nal: &[u8]| [&[0, 0, 0, 1][..], nal].concat();
    let mut packet = [annex_b(&sps), annex_b(&pps)].concat();
    let mut colours = Vec::new();
    for i in 1..=3 {
        let sample = reader.read_sample(1, i).unwrap().expect("a sample");
        let mut rest = &sample.bytes[..];
        while rest.len() > 4 {
            let n = u32::from_be_bytes([rest[0], rest[1], rest[2], rest[3]]) as usize;
            packet.extend(annex_b(&rest[4..4 + n]));
            rest = &rest[4 + n..];
        }
        let yuv = decoder
            .decode(&packet)
            .expect("decodes")
            .expect("a picture each sample");
        let mut rgb = vec![0u8; 40 * 30 * 3];
        yuv.write_rgb8(&mut rgb);
        colours.push(rgb[..3].to_vec());
        packet.clear();
    }
    assert!(is_red(&colours[0]), "red at the start: {colours:?}");
    assert!(is_blue(&colours[2]), "blue at the end: {colours:?}");
}

/// Answering `false` from the progress stops the video.
#[test]
fn a_video_can_be_stopped() {
    let (doc, take) = red_to_blue();
    let out = video(&doc, &take, ACTOR, &options(Format::Gif), &mut |done, _| {
        done < 2
    });
    assert!(matches!(out, Err(VideoError::Cancelled)));
}
