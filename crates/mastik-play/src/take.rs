//! Takes: a run's inputs recorded with their times, replayed on a fresh copy.

use crate::input::Input;
use crate::player::{Player, Start, StartError};
use mastik_core::Document;

/// A recorded run: where it started and each input with the time it arrived, in
/// milliseconds from the start. Replayed on a fresh copy of the same document it
/// reaches the same state at the same times, whatever the frame rate.
#[derive(Clone, Debug, PartialEq)]
pub struct Take {
    pub start: Start,
    pub inputs: Vec<(f64, Input)>,
    /// How long the run lasted.
    pub length: f64,
}

/// A take being recorded.
#[derive(Clone, Debug, Default, PartialEq)]
pub(crate) struct Recording {
    pub(crate) inputs: Vec<(f64, Input)>,
}

impl Player {
    /// Start recording a take. Play restarts first, so the take begins where a
    /// replay does. The restart keeps the play size and the hand, so the take
    /// opens with both, for its replay to start laid out and handed alike. Nothing
    /// is recorded when play cannot restart, since the take could not replay.
    pub fn record(&mut self) {
        if !self.restarted() {
            return;
        }
        let now = self.now;
        self.recording = Some(Recording {
            inputs: vec![
                (now, Input::PlaySize(self.play_size)),
                (now, Input::Touch(self.touch)),
            ],
        });
    }

    /// Whether a take is being recorded.
    pub fn is_recording(&self) -> bool {
        self.recording.is_some()
    }

    /// Stop recording and answer the take; `None` when none was being recorded.
    pub fn stop_recording(&mut self) -> Option<Take> {
        let r = self.recording.take()?;
        Some(Take {
            start: self.start,
            inputs: r.inputs,
            length: self.now,
        })
    }
}

/// A take played back on a fresh copy, stepped by the caller: frame by frame
/// for a video, or straight to the end.
pub struct Replay {
    player: Player,
    take: Take,
    next: usize,
}

impl Replay {
    /// Begin replaying `take` on a copy of `doc`, minting ids from `actor`. The
    /// play size and hand the take opens with are given at once, so the first
    /// frame is laid out as the run's was.
    pub fn new(doc: &Document, take: Take, actor: u64) -> Result<Replay, StartError> {
        let mut player = Player::new(doc, take.start, actor)?;
        let mut next = 0;
        while let Some((at, input)) = take.inputs.get(next)
            && *at <= 0.0
            && matches!(input, Input::PlaySize(_) | Input::Touch(_))
        {
            player.input(input.clone());
            next += 1;
        }
        Ok(Replay { player, take, next })
    }

    /// Move the replay on by `ms`: each input due in that time is given at its
    /// own time.
    pub fn advance(&mut self, ms: f64) {
        let until = self.player.now() + ms.max(0.0);
        while let Some((at, input)) = self.take.inputs.get(self.next).cloned() {
            if at > until {
                break;
            }
            let wait = at - self.player.now();
            self.player.tick(wait);
            self.player.input(input);
            self.next += 1;
        }
        let wait = until - self.player.now();
        self.player.tick(wait);
    }

    /// Replay what is left of the take, to its length.
    pub fn finish(&mut self) {
        let left = self.take.length - self.player.now();
        self.advance(left);
    }

    /// Whether every input has been given and the take's length reached.
    pub fn is_done(&self) -> bool {
        self.next >= self.take.inputs.len() && self.player.now() >= self.take.length
    }

    /// The player the take plays on, to draw.
    pub fn player(&self) -> &Player {
        &self.player
    }
}
