//! The level meter that drives the panel's recording state.
//!
//! `docs/PROJECT.md` §3 (Panel) asks for "a level meter that moves with the microphone RMS"
//! and WP2's acceptance criterion is "level events arrive at ≥ 20 Hz". Both numbers are
//! produced here, from the converted 16 kHz mono stream rather than from the device's native
//! buffer, so the meter shows the same signal the engine will be given.

use std::time::Duration;

use crate::SAMPLE_RATE;

/// The floor the meter reports instead of negative infinity.
///
/// Digital silence has no decibel value. A meter that returns `-inf` makes every consumer
/// handle it — a bar widget, a `format!`, a JSON payload — so the floor is picked once,
/// here, and −100 dBFS is far below anything a microphone produces.
pub const MIN_DBFS: f32 = -100.0;

/// One meter reading, covering `level_interval_ms` of audio.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct LevelEvent {
    /// Root-mean-square level of the window, in dBFS, floored at [`MIN_DBFS`].
    ///
    /// A full-scale sine reads −3.01 dBFS here, not 0: RMS is the average power, and a sine
    /// spends most of its time away from its peaks. A meter that showed 0 for a sine would
    /// be a peak meter wearing an RMS label.
    pub rms_dbfs: f32,
    /// The largest absolute sample in the window, in dBFS, floored at [`MIN_DBFS`].
    pub peak_dbfs: f32,
    /// Time from [`Capture::open`](crate::Capture::open) to the end of this window.
    ///
    /// Measured in samples of the 16 kHz stream rather than by a clock, so the series is
    /// exactly evenly spaced and a dropped event is visible as a gap instead of as jitter.
    pub t: Duration,
}

/// Turns a stream of samples into one [`LevelEvent`] per window.
pub(crate) struct LevelMeter {
    window: usize,
    filled: usize,
    sum_squares: f64,
    peak: f32,
    elapsed: u64,
}

impl LevelMeter {
    /// A meter that emits one event per `interval_ms` of 16 kHz audio.
    pub(crate) fn new(interval_ms: u32) -> Self {
        let window = (SAMPLE_RATE as u64 * u64::from(interval_ms) / 1000).max(1) as usize;
        LevelMeter {
            window,
            filled: 0,
            sum_squares: 0.0,
            peak: 0.0,
            elapsed: 0,
        }
    }

    /// How many samples one event covers.
    #[cfg(test)]
    pub(crate) fn window(&self) -> usize {
        self.window
    }

    /// Feed converted samples, calling `emit` once per completed window.
    ///
    /// `emit` is a closure rather than a channel send so that the meter itself never blocks
    /// and never knows what happens to the event; the drop-oldest policy lives at the call
    /// site, where the channel is.
    pub(crate) fn push(&mut self, samples: &[f32], mut emit: impl FnMut(LevelEvent)) {
        for &sample in samples {
            let magnitude = sample.abs();
            self.sum_squares += f64::from(sample) * f64::from(sample);
            if magnitude > self.peak {
                self.peak = magnitude;
            }
            self.filled += 1;
            self.elapsed += 1;

            if self.filled == self.window {
                let mean_square = self.sum_squares / self.window as f64;
                emit(LevelEvent {
                    rms_dbfs: dbfs(mean_square.sqrt() as f32),
                    peak_dbfs: dbfs(self.peak),
                    t: Duration::from_nanos(self.elapsed * 1_000_000_000 / u64::from(SAMPLE_RATE)),
                });
                self.filled = 0;
                self.sum_squares = 0.0;
                self.peak = 0.0;
            }
        }
    }
}

/// Amplitude in the `0.0..=1.0` range to dBFS, floored at [`MIN_DBFS`].
pub(crate) fn dbfs(amplitude: f32) -> f32 {
    if amplitude <= 0.0 {
        MIN_DBFS
    } else {
        (20.0 * amplitude.log10()).max(MIN_DBFS)
    }
}

#[cfg(test)]
mod tests {
    use super::{LevelEvent, LevelMeter, MIN_DBFS, dbfs};
    use crate::SAMPLE_RATE;

    fn collect(meter: &mut LevelMeter, samples: &[f32]) -> Vec<LevelEvent> {
        let mut events = Vec::new();
        meter.push(samples, |event| events.push(event));
        events
    }

    #[test]
    fn a_full_scale_sine_reads_minus_three_rms_and_zero_peak() {
        let mut meter = LevelMeter::new(50);
        // 1 kHz at 16 kHz is 16 samples per cycle, and the window holds whole cycles, so
        // the expected values are exact rather than approached.
        let samples: Vec<f32> = (0..meter.window() * 4)
            .map(|n| (std::f32::consts::TAU * 1000.0 * n as f32 / SAMPLE_RATE as f32).sin())
            .collect();

        let events = collect(&mut meter, &samples);
        assert_eq!(events.len(), 4, "a 50 ms meter must fire 20 times a second");

        for event in &events {
            assert!(
                (event.rms_dbfs - (-3.01)).abs() < 0.02,
                "RMS of a full-scale sine is -3.01 dBFS, got {}",
                event.rms_dbfs
            );
            assert!(
                event.peak_dbfs.abs() < 0.01,
                "peak of a full-scale sine is 0 dBFS, got {}",
                event.peak_dbfs
            );
        }

        assert_eq!(events[0].t.as_millis(), 50);
        assert_eq!(events[3].t.as_millis(), 200);
    }

    #[test]
    fn digital_silence_reads_the_floor_and_never_negative_infinity() {
        let mut meter = LevelMeter::new(50);
        let silence = vec![0.0; meter.window() * 2];
        let events = collect(&mut meter, &silence);

        assert_eq!(events.len(), 2);
        for event in &events {
            assert_eq!(event.rms_dbfs, MIN_DBFS);
            assert_eq!(event.peak_dbfs, MIN_DBFS);
            assert!(
                event.rms_dbfs.is_finite(),
                "the floor exists so this is finite"
            );
        }

        assert_eq!(dbfs(0.0), MIN_DBFS);
        assert_eq!(dbfs(-0.0), MIN_DBFS);
        assert_eq!(dbfs(1e-30), MIN_DBFS, "quieter than the floor is the floor");
    }
}
