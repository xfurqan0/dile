//! Native interleaved audio to 16 kHz mono, off the audio thread.
//!
//! The engine takes "16 kHz mono `f32`" and nothing else (`dile-engine`'s `SAMPLE_RATE`), so
//! something has to downmix and resample. That something is deliberately **not** the cpal
//! callback: a callback that runs an FFT is a callback that will eventually miss its
//! deadline, and a missed deadline is a click in the recording. The callback pushes raw
//! samples into a lock-free ring and returns; this runs on the worker thread that drains it.

use rubato::audioadapter_buffers::direct::InterleavedSlice;
use rubato::{FixedSync, Indexing, Resampler};

use crate::error::Error;
use crate::{SAMPLE_RATE, SourceSpec};

/// Frames per `process` call on the resampler.
///
/// Small on purpose. `rubato::Fft` splits a chunk into sub-chunks of about 256 frames and
/// the FFT block size — which is what sets the anti-aliasing quality — comes from the
/// sub-chunk, not from this. So a chunk of 256 costs nothing in quality and makes the
/// converter's granularity four times finer than a chunk of 1024 would: the samples still
/// sitting in `pending` when the key is released are the tail that the recording loses, and
/// at 48 kHz this keeps that tail under 6 ms.
const CHUNK_FRAMES: usize = 256;

/// Interleaved native audio in, 16 kHz mono out.
///
/// **Two callers, one converter.** The capture pipeline drives it from the worker thread
/// that drains the audio ring; `dile transcribe` drives it over a WAV file, so a recording
/// somebody already has goes through exactly the same downmix and the same resampler as one
/// spoken into the microphone. That is the point of it being public: a second resampler in
/// the command line would be a second answer to the same question.
pub struct Converter {
    channels: usize,
    /// A frame that arrived split across two callbacks.
    partial: Vec<f32>,
    /// Mono samples at the native rate, waiting for a full resampler chunk.
    pending: Vec<f32>,
    resampler: Option<Resampling>,
}

struct Resampling {
    inner: rubato::Fft<f32>,
    scratch: Vec<f32>,
    /// Output frames still to be thrown away to compensate the resampler's own delay.
    ///
    /// `rubato` reports a delay of half the internal FFT block, which arrives as near
    /// silence at the head of the stream. Dropping exactly that many output frames once, at
    /// the start of the stream, keeps the 16 kHz timeline aligned with the microphone's —
    /// which is what makes the pre-roll ring hold the number of milliseconds it claims.
    skip: usize,
}

impl Converter {
    /// A converter for a device running at `spec`.
    pub fn new(spec: SourceSpec) -> Result<Self, Error> {
        let channels = usize::from(spec.channels).max(1);
        let resampler = if spec.sample_rate == SAMPLE_RATE {
            // The pass-through case is a real case, not an optimisation: plenty of USB
            // microphones offer 16 kHz natively, and running a resampler at a ratio of
            // exactly 1.0 would put a filter in the path for nothing.
            None
        } else {
            let inner = rubato::Fft::<f32>::new(
                spec.sample_rate as usize,
                SAMPLE_RATE as usize,
                CHUNK_FRAMES,
                1,
                FixedSync::Input,
            )
            .map_err(|source| Error::ResamplerBuild {
                from: spec.sample_rate,
                source,
            })?;
            let skip = inner.output_delay();
            let scratch = vec![0.0; inner.output_frames_max()];
            Some(Resampling {
                inner,
                scratch,
                skip,
            })
        };

        Ok(Converter {
            channels,
            partial: Vec::with_capacity(channels),
            pending: Vec::with_capacity(CHUNK_FRAMES * 4),
            resampler,
        })
    }

    /// Convert `interleaved` and append the 16 kHz mono result to `out`.
    ///
    /// Whatever does not make a whole resampler chunk stays inside and comes out on a later
    /// call, so the stream is continuous across callback boundaries.
    pub fn process(&mut self, interleaved: &[f32], out: &mut Vec<f32>) -> Result<(), Error> {
        self.downmix(interleaved);

        let Some(resampling) = self.resampler.as_mut() else {
            out.append(&mut self.pending);
            return Ok(());
        };

        let mut consumed = 0;
        let mut indexing = Indexing::new();
        loop {
            let needed = resampling.inner.input_frames_next();
            if self.pending.len() - consumed < needed {
                break;
            }

            let input =
                InterleavedSlice::new(&self.pending[consumed..consumed + needed], 1, needed)
                    .map_err(|error| Error::Buffer(error.to_string()))?;
            let frames = resampling.scratch.len();
            let mut output = InterleavedSlice::new_mut(&mut resampling.scratch, 1, frames)
                .map_err(|error| Error::Buffer(error.to_string()))?;

            indexing.input_offset = 0;
            indexing.output_offset = 0;
            let (taken, produced) =
                resampling
                    .inner
                    .process_into_buffer(&input, &mut output, Some(&indexing))?;
            consumed += taken;

            let skipped = resampling.skip.min(produced);
            resampling.skip -= skipped;
            out.extend_from_slice(&resampling.scratch[skipped..produced]);
        }

        self.pending.drain(..consumed);
        Ok(())
    }

    /// Interleaved to mono, averaging the channels.
    ///
    /// Averaging rather than taking channel 0: a stereo headset that puts the microphone on
    /// one side would otherwise record silence on a machine that happened to pick the other.
    fn downmix(&mut self, interleaved: &[f32]) {
        if self.channels == 1 {
            self.pending.extend_from_slice(interleaved);
            return;
        }

        let mut rest = interleaved;
        if !self.partial.is_empty() {
            let want = self.channels - self.partial.len();
            let take = want.min(rest.len());
            self.partial.extend_from_slice(&rest[..take]);
            rest = &rest[take..];
            if self.partial.len() == self.channels {
                let sum: f32 = self.partial.iter().sum();
                self.pending.push(sum / self.channels as f32);
                self.partial.clear();
            }
        }

        let mut frames = rest.chunks_exact(self.channels);
        for frame in &mut frames {
            let sum: f32 = frame.iter().sum();
            self.pending.push(sum / self.channels as f32);
        }
        self.partial.extend_from_slice(frames.remainder());
    }
}

#[cfg(test)]
mod tests {
    use super::Converter;
    use crate::{SAMPLE_RATE, SourceSpec};

    fn sine(frequency: f32, rate: u32, seconds: f32, channels: u16) -> Vec<f32> {
        let frames = (rate as f32 * seconds) as usize;
        let mut out = Vec::with_capacity(frames * usize::from(channels));
        for n in 0..frames {
            let value = (std::f32::consts::TAU * frequency * n as f32 / rate as f32).sin();
            for _ in 0..channels {
                out.push(value);
            }
        }
        out
    }

    /// Rising zero crossings, ignoring a guard band at each end so that the filter's
    /// settling and the last partial chunk cannot skew the count.
    fn rising_crossings(samples: &[f32], guard: usize) -> usize {
        let body = &samples[guard..samples.len() - guard];
        body.windows(2)
            .filter(|pair| pair[0] <= 0.0 && pair[1] > 0.0)
            .count()
    }

    #[test]
    fn a_1khz_tone_at_48khz_is_still_1khz_at_16khz() {
        let mut converter =
            Converter::new(SourceSpec::new(48_000, 1)).expect("48 kHz is a resamplable rate");
        let mut out = Vec::new();
        // Fed in callback-sized pieces, because that is how it is fed in production and
        // because a converter that only works on one big slice is not a streaming converter.
        for block in sine(1000.0, 48_000, 2.0, 1).chunks(480) {
            converter.process(block, &mut out).expect("conversion");
        }

        let expected = 2 * SAMPLE_RATE as usize;
        assert!(
            out.len().abs_diff(expected) < SAMPLE_RATE as usize / 50,
            "2 s at 48 kHz is 2 s at 16 kHz, got {} samples",
            out.len()
        );

        let guard = SAMPLE_RATE as usize / 10;
        let body_seconds = (out.len() - 2 * guard) as f32 / SAMPLE_RATE as f32;
        let crossings = rising_crossings(&out, guard);
        let frequency = crossings as f32 / body_seconds;
        assert!(
            (frequency - 1000.0).abs() < 2.0,
            "1 kHz in must be 1 kHz out, measured {frequency} Hz"
        );
    }

    #[test]
    fn a_16khz_input_passes_through_unchanged() {
        let mut converter =
            Converter::new(SourceSpec::new(SAMPLE_RATE, 1)).expect("16 kHz needs no resampler");
        let input = sine(1000.0, SAMPLE_RATE, 1.0, 1);
        let mut out = Vec::new();
        for block in input.chunks(160) {
            converter.process(block, &mut out).expect("conversion");
        }

        assert_eq!(out.len(), input.len(), "no resampler, no length change");
        assert_eq!(out, input, "no resampler, no filter, byte for byte");
    }

    #[test]
    fn stereo_is_averaged_to_mono_across_callback_boundaries() {
        let mut converter = Converter::new(SourceSpec::new(SAMPLE_RATE, 2)).expect("16 kHz stereo");
        let mut out = Vec::new();
        // A deliberately odd split: the second callback starts in the middle of a frame.
        let input = [1.0f32, 0.0, 0.5, 0.5, 0.25];
        converter
            .process(&input[..3], &mut out)
            .expect("first half");
        converter
            .process(&input[3..], &mut out)
            .expect("second half");

        assert_eq!(
            out,
            vec![0.5, 0.5],
            "left and right averaged, frame by frame"
        );
    }

    #[test]
    fn a_44_1khz_device_resamples_too() {
        let mut converter =
            Converter::new(SourceSpec::new(44_100, 2)).expect("44.1 kHz is a resamplable rate");
        let mut out = Vec::new();
        for block in sine(1000.0, 44_100, 1.0, 2).chunks(882) {
            converter.process(block, &mut out).expect("conversion");
        }

        assert!(
            out.len().abs_diff(SAMPLE_RATE as usize) < SAMPLE_RATE as usize / 50,
            "1 s at 44.1 kHz is 1 s at 16 kHz, got {} samples",
            out.len()
        );
        let guard = SAMPLE_RATE as usize / 10;
        let body_seconds = (out.len() - 2 * guard) as f32 / SAMPLE_RATE as f32;
        let frequency = rising_crossings(&out, guard) as f32 / body_seconds;
        assert!(
            (frequency - 1000.0).abs() < 2.0,
            "1 kHz in must be 1 kHz out, measured {frequency} Hz"
        );
    }
}
