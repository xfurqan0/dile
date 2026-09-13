//! Everything that happens to a sample after it leaves the audio callback.
//!
//! Raw interleaved audio in, a [`Recording`] out, by way of the converter, the pre-roll
//! ring, the recording cap, the level meter and the VAD. Deliberately synchronous and
//! deliberately free of threads and channels: the worker thread in [`crate`] is a loop
//! around this type, and every rule WP2 has to keep — the pre-roll is prepended, the cap
//! stops cleanly, the audio is never trimmed — is testable here without a clock.

use std::collections::VecDeque;
use std::time::Duration;

use crate::convert::Converter;
use crate::error::Error;
use crate::level::{LevelEvent, LevelMeter};
use crate::vad::{SpeechSummary, SpeechTracker, Vad};
use crate::{SAMPLE_RATE, SourceSpec, State};

/// One held-and-released dictation, as the engine will be given it.
#[derive(Clone, Debug, PartialEq)]
pub struct Recording {
    /// 16 kHz mono `f32`, pre-roll first, then everything up to the release.
    ///
    /// Nothing is trimmed, ever. The VAD's opinion is in [`Recording::speech`]; the audio
    /// is whole.
    pub samples: Vec<f32>,
    /// How long [`Recording::samples`] is, pre-roll included.
    pub duration: Duration,
    /// Whether recording stopped because it reached the cap rather than on release.
    ///
    /// `docs/PROJECT.md` WP2: "the cap is adjustable and stops cleanly at the limit". The
    /// panel says so instead of pretending the user let go.
    pub cap_hit: bool,
    /// What the VAD made of it.
    pub speech: SpeechSummary,
}

impl Recording {
    /// True when there is nothing to transcribe.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.samples.is_empty()
    }
}

/// The ring that holds the last `pre_roll_ms` of audio while nothing is being recorded.
///
/// First-word protection, and the reason it is a ring rather than a flag: a push-to-talk
/// hotkey reaches the application after the user has already started the first syllable —
/// the key travel, the low-level hook and the event loop are each a few milliseconds — and
/// `docs/PROJECT.md` WP2 asks for "audio buffer with no clipped first syllable".
struct PreRoll {
    buffer: VecDeque<f32>,
    capacity: usize,
}

impl PreRoll {
    fn new(capacity: usize) -> Self {
        PreRoll {
            buffer: VecDeque::with_capacity(capacity),
            capacity,
        }
    }

    fn push(&mut self, samples: &[f32]) {
        if self.capacity == 0 {
            return;
        }
        let keep_from = samples.len().saturating_sub(self.capacity);
        let incoming = &samples[keep_from..];
        let overflow = (self.buffer.len() + incoming.len()).saturating_sub(self.capacity);
        self.buffer.drain(..overflow);
        self.buffer.extend(incoming);
    }

    fn drain_into(&mut self, out: &mut Vec<f32>) {
        out.extend(self.buffer.iter().copied());
    }

    #[cfg(test)]
    fn len(&self) -> usize {
        self.buffer.len()
    }
}

/// A recording in progress, or one waiting to be collected after the cap stopped it.
struct InFlight {
    samples: Vec<f32>,
    /// False once the cap has been reached: the buffer is closed but not yet handed over.
    live: bool,
    cap_hit: bool,
    /// Samples recorded since `start`, which is what the cap counts.
    live_samples: usize,
}

/// The synchronous core of the capture stage.
pub(crate) struct Pipeline {
    converter: Converter,
    pre_roll: PreRoll,
    meter: LevelMeter,
    vad: Box<dyn Vad>,
    tracker: SpeechTracker,
    in_flight: Option<InFlight>,
    cap_samples: usize,
    pre_roll_samples: usize,
    converted: Vec<f32>,
}

impl Pipeline {
    pub(crate) fn new(
        spec: SourceSpec,
        pre_roll_ms: u32,
        cap_secs: u32,
        level_interval_ms: u32,
        vad: Box<dyn Vad>,
    ) -> Result<Self, Error> {
        let pre_roll_samples = (u64::from(SAMPLE_RATE) * u64::from(pre_roll_ms) / 1000) as usize;
        let cap_samples = (u64::from(SAMPLE_RATE) * u64::from(cap_secs)) as usize;
        let tracker = SpeechTracker::new(vad.frame_len());

        Ok(Pipeline {
            converter: Converter::new(spec)?,
            pre_roll: PreRoll::new(pre_roll_samples),
            meter: LevelMeter::new(level_interval_ms),
            vad,
            tracker,
            in_flight: None,
            cap_samples,
            pre_roll_samples,
            converted: Vec::with_capacity(4096),
        })
    }

    /// Where the pipeline is right now.
    pub(crate) fn state(&self) -> State {
        match &self.in_flight {
            None => State::Armed,
            Some(in_flight) if in_flight.live => State::Recording,
            Some(_) => State::Capped,
        }
    }

    /// Feed raw interleaved samples from the source, emitting level events as windows close.
    pub(crate) fn feed(
        &mut self,
        interleaved: &[f32],
        emit: &mut impl FnMut(LevelEvent),
    ) -> Result<(), Error> {
        self.converted.clear();
        self.converter.process(interleaved, &mut self.converted)?;
        if self.converted.is_empty() {
            return Ok(());
        }

        // The meter runs on everything, armed or recording alike: the panel's meter has to
        // move before the key goes down, or the user cannot tell a dead microphone from a
        // quiet one until after the dictation is lost.
        let converted = std::mem::take(&mut self.converted);
        self.meter.push(&converted, emit);

        let mut rest = converted.as_slice();
        while !rest.is_empty() {
            let Some(in_flight) = self.in_flight.as_mut() else {
                self.pre_roll.push(rest);
                break;
            };
            if !in_flight.live {
                self.pre_roll.push(rest);
                break;
            }

            let room = self.cap_samples.saturating_sub(in_flight.live_samples);
            let take = room.min(rest.len());
            if take > 0 {
                in_flight.samples.extend_from_slice(&rest[..take]);
                in_flight.live_samples += take;
                self.tracker.push(&rest[..take], self.vad.as_mut());
                rest = &rest[take..];
            }

            if in_flight.live_samples >= self.cap_samples {
                // Stops itself, keeps the buffer, leaves the stream armed. The user is
                // still holding the key; the panel is what tells them the cap was reached.
                in_flight.live = false;
                in_flight.cap_hit = true;
                log::info!(
                    "recording cap of {} samples reached; recording stopped and the stream stays armed",
                    self.cap_samples
                );
            }
        }

        self.converted = converted;
        self.converted.clear();
        Ok(())
    }

    /// Recording begins now, with the pre-roll ring's contents in front of it.
    pub(crate) fn start(&mut self) {
        let mut samples = Vec::with_capacity(self.pre_roll_samples + self.cap_samples);
        self.pre_roll.drain_into(&mut samples);

        self.tracker.reset(self.vad.as_mut());
        self.tracker.push(&samples, self.vad.as_mut());

        self.in_flight = Some(InFlight {
            samples,
            live: true,
            cap_hit: false,
            live_samples: 0,
        });
    }

    /// Hand over what was recorded, and go back to filling the pre-roll ring.
    pub(crate) fn stop(&mut self) -> Recording {
        let in_flight = self.in_flight.take();
        let speech = self.tracker.summary();
        let (samples, cap_hit) = match in_flight {
            Some(in_flight) => (in_flight.samples, in_flight.cap_hit),
            None => (Vec::new(), false),
        };

        Recording {
            duration: Duration::from_nanos(
                samples.len() as u64 * 1_000_000_000 / u64::from(SAMPLE_RATE),
            ),
            speech: if samples.is_empty() {
                SpeechSummary::default()
            } else {
                speech
            },
            samples,
            cap_hit,
        }
    }

    /// Throw the recording away and stay armed.
    pub(crate) fn discard(&mut self) {
        self.in_flight = None;
    }

    #[cfg(test)]
    pub(crate) fn pre_roll_len(&self) -> usize {
        self.pre_roll.len()
    }
}

#[cfg(test)]
mod tests {
    use super::{Pipeline, PreRoll};
    use crate::vad::RmsVad;
    use crate::{SAMPLE_RATE, SourceSpec, State};

    const NATIVE_RATE: u32 = 48_000;

    fn pipeline(pre_roll_ms: u32, cap_secs: u32) -> Pipeline {
        Pipeline::new(
            SourceSpec::new(NATIVE_RATE, 1),
            pre_roll_ms,
            cap_secs,
            50,
            Box::new(RmsVad::new()),
        )
        .expect("48 kHz mono is a buildable pipeline")
    }

    /// `seconds` of 440 Hz at the native rate, in 10 ms pieces the way a callback delivers.
    fn feed(pipeline: &mut Pipeline, seconds: f32, amplitude: f32, phase: &mut usize) {
        let frames = (NATIVE_RATE as f32 * seconds) as usize;
        let mut block = Vec::with_capacity(480);
        let mut written = 0;
        while written < frames {
            block.clear();
            let this = 480.min(frames - written);
            for _ in 0..this {
                let value = amplitude
                    * (std::f32::consts::TAU * 440.0 * *phase as f32 / NATIVE_RATE as f32).sin();
                block.push(value);
                *phase += 1;
            }
            pipeline
                .feed(&block, &mut |_| {})
                .expect("conversion cannot fail on a synthetic tone");
            written += this;
        }
    }

    fn near(samples: usize, expected_ms: u64, tolerance_ms: u64) -> bool {
        let ms = samples as u64 * 1000 / u64::from(SAMPLE_RATE);
        ms.abs_diff(expected_ms) <= tolerance_ms
    }

    #[test]
    fn the_pre_roll_ring_is_prepended_to_the_recording() {
        // The scenario WP2's acceptance criterion describes: two seconds of audio through
        // the whole pipeline, the key goes down at 1.5 s and comes up at 2.0 s. The half
        // second that was spoken lands in the recording, and so does the 500 ms of pre-roll
        // in front of it — which is the first syllable the hotkey was too slow to catch.
        let mut pipeline = pipeline(500, 60);
        let mut phase = 0;

        feed(&mut pipeline, 1.5, 0.4, &mut phase);
        assert_eq!(pipeline.state(), State::Armed);
        assert!(
            near(pipeline.pre_roll_len(), 500, 5),
            "the ring holds 500 ms once it has seen more than that"
        );

        pipeline.start();
        assert_eq!(pipeline.state(), State::Recording);
        feed(&mut pipeline, 0.5, 0.4, &mut phase);

        let recording = pipeline.stop();
        assert_eq!(pipeline.state(), State::Armed);
        assert!(
            near(recording.samples.len(), 1000, 20),
            "500 ms of pre-roll plus 500 ms recorded, got {} samples",
            recording.samples.len()
        );
        assert!(
            recording.duration.as_millis().abs_diff(1000) <= 20,
            "the duration agrees with the sample count, got {:?}",
            recording.duration
        );
        assert!(!recording.cap_hit);
        assert!(
            recording.speech.has_speech,
            "a 440 Hz tone well above the noise floor is speech to an RMS detector"
        );
    }

    #[test]
    fn without_a_pre_roll_only_what_was_recorded_survives() {
        let mut pipeline = pipeline(0, 60);
        let mut phase = 0;

        feed(&mut pipeline, 1.0, 0.4, &mut phase);
        assert_eq!(pipeline.pre_roll_len(), 0);
        pipeline.start();
        feed(&mut pipeline, 0.5, 0.4, &mut phase);

        let recording = pipeline.stop();
        assert!(
            near(recording.samples.len(), 500, 20),
            "no pre-roll, so exactly the half second that was recorded, got {} samples",
            recording.samples.len()
        );
    }

    #[test]
    fn the_cap_stops_recording_by_itself_and_leaves_the_stream_armed() {
        let mut pipeline = pipeline(200, 1);
        let mut phase = 0;

        feed(&mut pipeline, 0.5, 0.4, &mut phase);
        pipeline.start();
        // Two seconds offered against a one second cap.
        feed(&mut pipeline, 2.0, 0.4, &mut phase);

        assert_eq!(
            pipeline.state(),
            State::Capped,
            "the cap stops the recording without anyone calling stop"
        );

        let recording = pipeline.stop();
        assert!(recording.cap_hit, "and says so");
        assert!(
            near(recording.samples.len(), 1200, 20),
            "200 ms of pre-roll plus exactly the one second cap, got {} samples",
            recording.samples.len()
        );

        // The stream is still armed: the next press records normally.
        assert_eq!(pipeline.state(), State::Armed);
        feed(&mut pipeline, 0.5, 0.4, &mut phase);
        pipeline.start();
        feed(&mut pipeline, 0.3, 0.4, &mut phase);
        let second = pipeline.stop();
        assert!(!second.cap_hit);
        assert!(near(second.samples.len(), 500, 20));
    }

    #[test]
    fn discard_throws_the_buffer_away_and_keeps_the_stream_armed() {
        // A tap shorter than the press-length threshold: the panel opens idle and nothing
        // is dictated (docs/PROJECT.md §3, Hotkey).
        let mut pipeline = pipeline(500, 60);
        let mut phase = 0;

        feed(&mut pipeline, 1.0, 0.4, &mut phase);
        pipeline.start();
        feed(&mut pipeline, 0.1, 0.4, &mut phase);
        pipeline.discard();
        assert_eq!(pipeline.state(), State::Armed);

        let nothing = pipeline.stop();
        assert!(nothing.is_empty());
        assert!(!nothing.speech.has_speech);

        // And the ring kept filling through all of it.
        feed(&mut pipeline, 0.6, 0.4, &mut phase);
        pipeline.start();
        feed(&mut pipeline, 0.2, 0.4, &mut phase);
        let recording = pipeline.stop();
        assert!(near(recording.samples.len(), 700, 20));
    }

    #[test]
    fn silence_is_reported_as_silence_and_the_audio_is_still_whole() {
        let mut pipeline = pipeline(300, 60);
        let mut phase = 0;

        feed(&mut pipeline, 0.5, 0.0, &mut phase);
        pipeline.start();
        feed(&mut pipeline, 1.0, 0.0, &mut phase);

        let recording = pipeline.stop();
        assert!(
            !recording.speech.has_speech,
            "the caller skips the engine on this one"
        );
        assert!(
            near(recording.samples.len(), 1300, 20),
            "and the audio is untouched all the same: the VAD never trims"
        );
    }

    #[test]
    fn the_ring_keeps_the_newest_audio_when_a_single_push_overflows_it() {
        let mut ring = PreRoll::new(4);
        ring.push(&[1.0, 2.0, 3.0, 4.0, 5.0, 6.0]);
        assert_eq!(
            ring.buffer.iter().copied().collect::<Vec<_>>(),
            vec![3.0, 4.0, 5.0, 6.0]
        );

        ring.push(&[7.0, 8.0]);
        assert_eq!(
            ring.buffer.iter().copied().collect::<Vec<_>>(),
            vec![5.0, 6.0, 7.0, 8.0]
        );
    }
}
