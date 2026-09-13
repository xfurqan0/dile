//! Voice activity detection: the seam, and the baseline that sits in it today.
//!
//! # What the VAD is for here, and what it is not for
//!
//! **It never touches the audio.** [`Recording`](crate::Recording) keeps every sample from
//! the pre-roll to the release, and nothing in this module can shorten it. The VAD produces
//! a [`SpeechSummary`] beside the audio, and the caller decides what to do with it:
//!
//! * skip the engine entirely when `has_speech` is false, which is the silence half of the
//!   hallucination guard — `docs/PROJECT.md` log 2026-09-09 found that every canned
//!   hallucination in the robustness set sat in the same ten seconds of applause, "an
//!   insertion over non-speech, so WP2's VAD / silence gate, not the decoder, owns it";
//! * tell the user "nothing heard" instead of pasting whatever a decoder invents from room
//!   tone;
//! * give WP4's hallucination filter the duration and position guards §5 asks for.
//!
//! Trimming inside the recording is deliberately not offered. A VAD that cuts is a VAD that
//! can cut a word, and first-word protection is the point of the whole capture stage.
//!
//! # The seam
//!
//! [`Vad`] is the trait, [`RmsVad`] is the only implementation today. `docs/PROJECT.md` §3
//! puts `earshot` (40 KB minGRU, zero deps) against Silero ONNX in M0 and WP2 ships the
//! winner; neither is integrated yet, and the M0 row that decides between them has not been
//! measured. The trait exists so that arrival is a `Box<dyn Vad>` in one constructor rather
//! than a rewrite of the pipeline, and so the relative-RMS baseline the M0 protocol calls
//! for ("a ~130-line relative-RMS VAD (the approach dikte uses) is a free baseline against
//! `earshot` and Silero") is a real implementation rather than a plan.

use std::time::Duration;

use crate::SAMPLE_RATE;
use crate::level::dbfs;

/// What a VAD makes of one frame.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Decision {
    /// The frame is background: room tone, fan noise, a keyboard.
    Silence,
    /// The frame is speech, or close enough to it to keep the utterance whole.
    Speech,
}

/// A frame-by-frame voice activity detector.
///
/// Implementations are fed contiguous, non-overlapping frames of exactly [`Vad::frame_len`]
/// samples of 16 kHz mono `f32`, in order, from the head of a recording to its end.
pub trait Vad: Send {
    /// How many 16 kHz samples one frame holds.
    ///
    /// The pipeline chops the stream on this boundary, so a detector with an opinion about
    /// its window size states it here rather than hoping the caller guessed it.
    fn frame_len(&self) -> usize;

    /// Classify one frame of exactly [`Vad::frame_len`] samples.
    fn process(&mut self, frame_16k: &[f32]) -> Decision;

    /// Forget everything learned, ready for a new recording.
    fn reset(&mut self);
}

/// How [`RmsVad`] decides, in numbers.
///
/// The defaults are the ones the baseline runs with; they are here as a struct rather than
/// as constants because M0 compares this detector against `earshot` and Silero, and a
/// baseline whose knobs cannot be moved is not a baseline.
#[derive(Clone, Copy, Debug)]
pub struct RmsVadConfig {
    /// Frame length in milliseconds. 20 ms is the usual dictation frame.
    pub frame_ms: u32,
    /// How far above the tracked noise floor a frame must sit to count as speech.
    pub margin_db: f32,
    /// The lowest the tracked noise floor is allowed to go.
    pub floor_min_dbfs: f32,
    /// The highest the tracked noise floor is allowed to go.
    ///
    /// The one knob that stops the classic failure of an adaptive detector: a recording
    /// that opens in the middle of a word teaches the floor that speech is the background,
    /// and the whole utterance then reads as silence. Anything above this is not a room, it
    /// is a voice, and the floor refuses to follow it. It is also what makes it safe for the
    /// floor to keep learning *during* speech, which is what stops the hangover latching.
    pub floor_max_dbfs: f32,
    /// The lowest the speech threshold is allowed to go, however quiet the room is.
    pub threshold_min_dbfs: f32,
    /// Consecutive frames above the threshold before speech is declared.
    ///
    /// Two frames — 40 ms — rejects a mouse click without losing a plosive.
    pub attack_frames: u32,
    /// How long speech keeps being reported after the level drops back.
    ///
    /// Turkish is full of short stops inside a word; without a hangover, `bir tak-tık` is
    /// three speech regions and two silences.
    pub hangover_ms: u32,
    /// Smoothing applied when the frame is louder than the tracked floor.
    ///
    /// Slow on purpose, and it runs during speech as well: a held vowel moves the floor by
    /// a fraction of a decibel, while a room that turned out to be noisier than the first
    /// frame suggested is learned inside a second.
    pub floor_rise: f32,
    /// Smoothing applied when the frame is quieter than the tracked floor.
    ///
    /// Fast on purpose: the pause after a word is where the real floor is visible, and a
    /// detector that takes seconds to come back down spends them deaf.
    pub floor_fall: f32,
}

impl Default for RmsVadConfig {
    fn default() -> Self {
        RmsVadConfig {
            frame_ms: 20,
            margin_db: 9.0,
            floor_min_dbfs: -75.0,
            floor_max_dbfs: -40.0,
            threshold_min_dbfs: -55.0,
            attack_frames: 2,
            hangover_ms: 150,
            floor_rise: 0.05,
            floor_fall: 0.35,
        }
    }
}

/// The relative-RMS baseline: an adaptive noise floor, a relative threshold and a hangover.
///
/// The detector every dictation tool starts with, and the one `docs/PROJECT.md` WP1 names as
/// the free baseline the learned detectors have to beat. It knows nothing about speech; it
/// knows that speech is louder than the room and that the room drifts.
pub struct RmsVad {
    config: RmsVadConfig,
    frame_len: usize,
    hangover_frames: u32,
    floor_dbfs: f32,
    above_run: u32,
    hangover_left: u32,
    in_speech: bool,
    seen_a_frame: bool,
}

impl RmsVad {
    /// The baseline with [`RmsVadConfig::default`].
    #[must_use]
    pub fn new() -> Self {
        Self::with_config(RmsVadConfig::default())
    }

    /// The baseline with knobs of your own.
    #[must_use]
    pub fn with_config(config: RmsVadConfig) -> Self {
        let frame_len = (SAMPLE_RATE as u64 * u64::from(config.frame_ms) / 1000).max(1) as usize;
        let hangover_frames =
            u64::from(config.hangover_ms).div_ceil(u64::from(config.frame_ms).max(1)) as u32;
        let mut vad = RmsVad {
            config,
            frame_len,
            hangover_frames,
            floor_dbfs: 0.0,
            above_run: 0,
            hangover_left: 0,
            in_speech: false,
            seen_a_frame: false,
        };
        vad.reset();
        vad
    }

    /// The level a frame has to beat right now, in dBFS.
    ///
    /// Public because the settings window will eventually want to draw it across the meter,
    /// and because a detector whose threshold cannot be read is a detector nobody can
    /// diagnose.
    #[must_use]
    pub fn threshold_dbfs(&self) -> f32 {
        (self.floor_dbfs + self.config.margin_db).max(self.config.threshold_min_dbfs)
    }

    /// The noise floor as it stands, in dBFS.
    #[must_use]
    pub fn noise_floor_dbfs(&self) -> f32 {
        self.floor_dbfs
    }
}

impl Default for RmsVad {
    fn default() -> Self {
        Self::new()
    }
}

impl Vad for RmsVad {
    fn frame_len(&self) -> usize {
        self.frame_len
    }

    fn process(&mut self, frame_16k: &[f32]) -> Decision {
        let sum_squares: f64 = frame_16k.iter().map(|&s| f64::from(s) * f64::from(s)).sum();
        let level = if frame_16k.is_empty() {
            crate::level::MIN_DBFS
        } else {
            dbfs((sum_squares / frame_16k.len() as f64).sqrt() as f32)
        };

        // The very first frame sets the floor rather than being measured against a guess,
        // so a quiet room does not spend the first second above the threshold.
        if !self.seen_a_frame {
            self.seen_a_frame = true;
            self.floor_dbfs = level.clamp(self.config.floor_min_dbfs, self.config.floor_max_dbfs);
        }

        if level > self.threshold_dbfs() {
            self.above_run = self.above_run.saturating_add(1);
            if self.above_run >= self.config.attack_frames {
                self.in_speech = true;
                self.hangover_left = self.hangover_frames;
            }
        } else {
            self.above_run = 0;
            if self.hangover_left > 0 {
                self.hangover_left -= 1;
            } else {
                self.in_speech = false;
            }
        }

        // The floor learns from every frame, including the ones just called speech.
        //
        // The tempting alternative — only learn while nobody thinks it is speech — latches,
        // and it latched on real hardware the first time this crate met a room: a machine
        // whose noise floor sits at −49 dBFS is above the absolute minimum threshold, so the
        // first transient starts a hangover, the hangover suppresses every floor update, the
        // threshold stays pinned at its minimum, and the next transient re-arms it. Four
        // seconds of an empty room came back as 3.84 seconds of speech.
        //
        // What makes learning-during-speech safe is `floor_max_dbfs` rather than the
        // decision: speech cannot teach the floor that speech is the background, because the
        // floor is not allowed above a level no room ever reaches. The asymmetry does the
        // rest — down fast, up slowly — so a pause of a few frames recovers the real floor
        // while a held vowel barely moves it.
        let alpha = if level > self.floor_dbfs {
            self.config.floor_rise
        } else {
            self.config.floor_fall
        };
        self.floor_dbfs += alpha * (level - self.floor_dbfs);
        self.floor_dbfs = self
            .floor_dbfs
            .clamp(self.config.floor_min_dbfs, self.config.floor_max_dbfs);

        if self.in_speech {
            Decision::Speech
        } else {
            Decision::Silence
        }
    }

    fn reset(&mut self) {
        self.floor_dbfs = self.config.floor_min_dbfs;
        self.above_run = 0;
        self.hangover_left = 0;
        self.in_speech = false;
        self.seen_a_frame = false;
    }
}

/// What the VAD made of one recording.
///
/// Everything here is measured from the head of the recording, which is the first pre-roll
/// sample rather than the moment the key went down. That is the timeline the samples are on,
/// so it is the timeline the caller can index with.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct SpeechSummary {
    /// Whether any speech was found at all.
    ///
    /// The flag the caller checks before starting the engine. False means "say nothing
    /// heard", not "transcribe and hope".
    pub has_speech: bool,
    /// Total milliseconds classified as speech, hangover included.
    pub speech_ms: u32,
    /// Start of the first frame classified as speech.
    pub first_speech_at: Option<Duration>,
    /// End of the last frame classified as speech.
    pub last_speech_at: Option<Duration>,
}

/// Chops the recording into frames, runs a [`Vad`] over them and keeps the summary.
pub(crate) struct SpeechTracker {
    frame: Vec<f32>,
    frame_len: usize,
    frame_ms: u32,
    offset_samples: u64,
    summary: SpeechSummary,
}

impl SpeechTracker {
    pub(crate) fn new(frame_len: usize) -> Self {
        let frame_len = frame_len.max(1);
        SpeechTracker {
            frame: Vec::with_capacity(frame_len),
            frame_len,
            frame_ms: (frame_len as u64 * 1000 / u64::from(SAMPLE_RATE)).max(1) as u32,
            offset_samples: 0,
            summary: SpeechSummary::default(),
        }
    }

    pub(crate) fn reset(&mut self, vad: &mut dyn Vad) {
        self.frame.clear();
        self.offset_samples = 0;
        self.summary = SpeechSummary::default();
        vad.reset();
    }

    /// Feed the next samples of the recording, in order.
    pub(crate) fn push(&mut self, samples: &[f32], vad: &mut dyn Vad) {
        let mut rest = samples;
        while !rest.is_empty() {
            let want = self.frame_len - self.frame.len();
            let take = want.min(rest.len());
            self.frame.extend_from_slice(&rest[..take]);
            rest = &rest[take..];

            if self.frame.len() == self.frame_len {
                let decision = vad.process(&self.frame);
                self.record(decision);
                self.frame.clear();
                self.offset_samples += self.frame_len as u64;
            }
        }
    }

    /// The summary as it stands. The trailing partial frame — under one frame of audio — is
    /// not classified; at 20 ms it cannot change `has_speech` in a way anyone would notice.
    pub(crate) fn summary(&self) -> SpeechSummary {
        self.summary
    }

    fn record(&mut self, decision: Decision) {
        if decision != Decision::Speech {
            return;
        }
        let start = samples_to_duration(self.offset_samples);
        let end = samples_to_duration(self.offset_samples + self.frame_len as u64);
        self.summary.has_speech = true;
        self.summary.speech_ms += self.frame_ms;
        if self.summary.first_speech_at.is_none() {
            self.summary.first_speech_at = Some(start);
        }
        self.summary.last_speech_at = Some(end);
    }
}

fn samples_to_duration(samples: u64) -> Duration {
    Duration::from_nanos(samples * 1_000_000_000 / u64::from(SAMPLE_RATE))
}

#[cfg(test)]
mod tests {
    use super::{Decision, RmsVad, SpeechTracker, Vad};
    use crate::SAMPLE_RATE;

    /// `seconds` of 300 Hz at `amplitude`, the shape of a vowel as far as an RMS detector
    /// is concerned.
    fn tone(seconds: f32, amplitude: f32) -> Vec<f32> {
        let count = (SAMPLE_RATE as f32 * seconds) as usize;
        (0..count)
            .map(|n| {
                amplitude * (std::f32::consts::TAU * 300.0 * n as f32 / SAMPLE_RATE as f32).sin()
            })
            .collect()
    }

    fn silence(seconds: f32) -> Vec<f32> {
        vec![0.0; (SAMPLE_RATE as f32 * seconds) as usize]
    }

    #[test]
    fn a_burst_between_two_silences_is_found_and_timed() {
        let mut vad = RmsVad::new();
        let mut tracker = SpeechTracker::new(vad.frame_len());
        tracker.reset(&mut vad);

        let mut audio = silence(0.5);
        audio.extend(tone(1.0, 0.3));
        audio.extend(silence(0.5));
        tracker.push(&audio, &mut vad);

        let summary = tracker.summary();
        assert!(summary.has_speech);

        let first = summary.first_speech_at.expect("speech was found");
        let last = summary.last_speech_at.expect("speech was found");
        assert!(
            (first.as_millis() as i64 - 500).abs() <= 60,
            "the burst starts at 500 ms, the detector said {first:?}"
        );
        // The hangover deliberately runs past the end of the burst, so the window is
        // one-sided: never early, at most a hangover late.
        assert!(
            last.as_millis() as i64 >= 1500 && last.as_millis() as i64 <= 1500 + 200,
            "the burst ends at 1500 ms, the detector said {last:?}"
        );
        assert!(
            summary.speech_ms >= 950 && summary.speech_ms <= 1250,
            "a one-second burst plus hangover, got {} ms",
            summary.speech_ms
        );
    }

    #[test]
    fn a_quiet_room_is_not_speech() {
        let mut vad = RmsVad::new();
        let mut tracker = SpeechTracker::new(vad.frame_len());
        tracker.reset(&mut vad);

        // Digital silence, then room tone at -60 dBFS, which is quieter than the floor the
        // threshold is allowed to fall to.
        let mut audio = silence(0.5);
        audio.extend(tone(2.0, 0.001));
        tracker.push(&audio, &mut vad);

        let summary = tracker.summary();
        assert!(!summary.has_speech, "room tone is not speech");
        assert_eq!(summary.speech_ms, 0);
        assert_eq!(summary.first_speech_at, None);
    }

    #[test]
    fn a_recording_that_opens_mid_word_still_hears_it() {
        // The failure an adaptive floor invites: the detector's first frame is already
        // speech, so a floor that simply follows the signal learns that speech is the room.
        // `floor_max_dbfs` is the guard, and this is the test that it holds.
        let mut vad = RmsVad::new();
        let mut tracker = SpeechTracker::new(vad.frame_len());
        tracker.reset(&mut vad);

        tracker.push(&tone(1.5, 0.25), &mut vad);

        let summary = tracker.summary();
        assert!(
            summary.has_speech,
            "speech from the first sample is still speech"
        );
        assert!(
            summary.speech_ms >= 1400,
            "almost all of it is speech, got {} ms",
            summary.speech_ms
        );
    }

    #[test]
    fn a_room_louder_than_the_minimum_threshold_does_not_latch() {
        // The failure this detector actually shipped with, found on the maintainer's machine
        // rather than in a test: a room at about -49 dBFS is *above* `threshold_min_dbfs`, so
        // the first frames of it read as speech while the floor is still down at its minimum.
        // With the floor frozen during speech, the hangover then suppressed every update, the
        // threshold never moved, and four seconds of an empty room came back as 3.84 seconds
        // of speech. The floor has to keep learning through the hangover for this to recover.
        let mut vad = RmsVad::new();
        let mut tracker = SpeechTracker::new(vad.frame_len());
        tracker.reset(&mut vad);

        // A sine at 0.005 is -49 dBFS RMS: quiet enough to be a room, loud enough to sit
        // above the -55 dBFS absolute threshold that the floor starts out implying.
        let mut audio = silence(0.1);
        audio.extend(tone(4.0, 0.005));
        tracker.push(&audio, &mut vad);

        let summary = tracker.summary();
        assert!(
            summary.speech_ms < 1_000,
            "the detector has to learn the room within a second, it called {} ms of 4100 speech",
            summary.speech_ms
        );
        let last = summary.last_speech_at.unwrap_or(std::time::Duration::ZERO);
        assert!(
            last.as_millis() < 1_500,
            "and then stay quiet: the last speech frame was at {last:?} of 4100 ms"
        );
        assert!(
            vad.noise_floor_dbfs() > -55.0,
            "the floor has followed the room up to about -49 dBFS, not stayed at its minimum; it is at {}",
            vad.noise_floor_dbfs()
        );
    }

    #[test]
    fn the_hangover_bridges_a_stop_inside_a_word() {
        let mut vad = RmsVad::new();
        let frame = vad.frame_len();

        // Reach speech, then hand it 60 ms of silence: shorter than the 150 ms hangover.
        for _ in 0..10 {
            let _ = vad.process(&tone(frame as f32 / SAMPLE_RATE as f32, 0.3));
        }
        let quiet = vec![0.0f32; frame];
        assert_eq!(vad.process(&quiet), Decision::Speech, "20 ms gap");
        assert_eq!(vad.process(&quiet), Decision::Speech, "40 ms gap");
        assert_eq!(vad.process(&quiet), Decision::Speech, "60 ms gap");

        // And that it does eventually let go.
        for _ in 0..10 {
            let _ = vad.process(&quiet);
        }
        assert_eq!(vad.process(&quiet), Decision::Silence);
    }
}
