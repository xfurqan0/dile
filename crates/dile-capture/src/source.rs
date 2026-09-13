//! The seam between the audio hardware and everything this crate tests.
//!
//! cpal is touched in exactly two places in this crate: [`Capture::open`](crate::Capture::open)
//! and [`Capture::devices`](crate::Capture::devices). Everything after the ring buffer —
//! downmix, resample, pre-roll, cap, level meter, VAD — is driven through [`Source`], so the
//! tests drive the whole pipeline with [`SyntheticSource`] and never ask the machine for a
//! microphone. That is not a testing nicety: CI runs on `windows-latest`, which has no audio
//! device at all, and a capture crate whose tests need one is a capture crate with no tests.

use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

use crate::error::Error;

/// The shape of the audio a [`Source`] delivers, before any conversion.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct SourceSpec {
    /// Frames per second, as the device runs — not what the engine wants.
    pub sample_rate: u32,
    /// Interleaved channel count.
    pub channels: u16,
}

impl SourceSpec {
    /// A spec with the two numbers that matter.
    #[must_use]
    pub fn new(sample_rate: u32, channels: u16) -> Self {
        SourceSpec {
            sample_rate,
            channels,
        }
    }
}

/// Where a [`Source`] puts its samples: the producer end of a lock-free ring.
///
/// # The contract this type exists to keep
///
/// [`RawSink::push`] is called from the real-time audio callback. It allocates nothing,
/// locks nothing and blocks on nothing. When the ring is full — the worker thread was
/// descheduled, the machine is thrashing — the samples that do not fit are **dropped and
/// counted**, never waited for and never panicked on. A capture stage that panics in the
/// audio callback takes the process with it; one that blocks there produces a click in
/// every other application on the machine.
pub struct RawSink {
    producer: rtrb::Producer<f32>,
    overruns: Arc<AtomicU64>,
}

impl RawSink {
    pub(crate) fn new(producer: rtrb::Producer<f32>, overruns: Arc<AtomicU64>) -> Self {
        RawSink { producer, overruns }
    }

    /// Hand the pipeline interleaved samples. Real-time safe; never blocks.
    pub fn push(&mut self, interleaved: &[f32]) {
        let room = self.producer.slots();
        let take = interleaved.len().min(room);
        if take < interleaved.len() {
            self.overruns
                .fetch_add((interleaved.len() - take) as u64, Ordering::Relaxed);
        }
        if take == 0 {
            return;
        }

        let Ok(mut chunk) = self.producer.write_chunk(take) else {
            // `slots()` said there was room, so this cannot happen with a single producer.
            // Counting it rather than asserting keeps the callback free of panics.
            self.overruns
                .fetch_add(interleaved.len() as u64, Ordering::Relaxed);
            return;
        };
        let (first, second) = chunk.as_mut_slices();
        let split = first.len();
        first.copy_from_slice(&interleaved[..split]);
        second.copy_from_slice(&interleaved[split..split + second.len()]);
        chunk.commit_all();
    }

    /// Free slots in the ring, in samples.
    #[must_use]
    pub fn room(&self) -> usize {
        self.producer.slots()
    }
}

/// Something that delivers live audio into a [`RawSink`].
pub trait Source {
    /// The rate and channel count the samples will arrive at.
    fn spec(&self) -> SourceSpec;

    /// Start delivering. The returned guard owns the stream; dropping it stops delivery.
    fn start(self: Box<Self>, sink: RawSink) -> Result<Box<dyn StreamGuard>, Error>;
}

/// Keeps a started [`Source`] alive.
///
/// A marker trait with no methods, because stopping is what `Drop` is for and a `stop()`
/// that can be forgotten is a microphone left open.
pub trait StreamGuard: Send {}

/// A source the test drives by hand, with no hardware anywhere.
///
/// Pair it with [`SyntheticFeeder`], which is the half the test keeps:
///
/// ```
/// use dile_capture::{Capture, CaptureConfig, SyntheticSource};
///
/// let (source, mut feeder) = SyntheticSource::new(48_000, 1);
/// let capture = Capture::open_with(CaptureConfig::default(), Box::new(source))?;
///
/// feeder.feed(&vec![0.0; 48_000]);   // a second of nothing, into the pre-roll
/// capture.start()?;
/// feeder.feed(&vec![0.0; 24_000]);   // half a second, recorded
/// let recording = capture.stop()?;
/// assert!(!recording.speech.has_speech);
/// # Ok::<(), dile_capture::Error>(())
/// ```
pub struct SyntheticSource {
    spec: SourceSpec,
    shared: Arc<SyntheticShared>,
}

struct SyntheticShared {
    sink: std::sync::Mutex<Option<RawSink>>,
}

/// The handle a test pushes samples through.
///
/// Unlike [`RawSink::push`], [`SyntheticFeeder::feed`] **waits** for room rather than
/// dropping: a test that silently lost half its tone would pass or fail for the wrong
/// reason. Nothing real-time uses this type.
pub struct SyntheticFeeder {
    shared: Arc<SyntheticShared>,
}

impl SyntheticSource {
    /// A source at `sample_rate` and `channels`, plus the feeder that drives it.
    #[must_use]
    pub fn new(sample_rate: u32, channels: u16) -> (Self, SyntheticFeeder) {
        let shared = Arc::new(SyntheticShared {
            sink: std::sync::Mutex::new(None),
        });
        let source = SyntheticSource {
            spec: SourceSpec::new(sample_rate, channels),
            shared: Arc::clone(&shared),
        };
        (source, SyntheticFeeder { shared })
    }
}

impl Source for SyntheticSource {
    fn spec(&self) -> SourceSpec {
        self.spec
    }

    fn start(self: Box<Self>, sink: RawSink) -> Result<Box<dyn StreamGuard>, Error> {
        match self.shared.sink.lock() {
            Ok(mut slot) => *slot = Some(sink),
            Err(_) => return Err(Error::WorkerGone),
        }
        Ok(Box::new(SyntheticGuard {
            shared: self.shared,
        }))
    }
}

struct SyntheticGuard {
    shared: Arc<SyntheticShared>,
}

impl StreamGuard for SyntheticGuard {}

impl Drop for SyntheticGuard {
    fn drop(&mut self) {
        if let Ok(mut slot) = self.shared.sink.lock() {
            *slot = None;
        }
    }
}

impl SyntheticFeeder {
    /// Push interleaved samples, waiting for ring space instead of dropping.
    ///
    /// Returns once every sample is in the ring. Does nothing if the capture that owns the
    /// sink has been dropped, which is how a test that outlives its `Capture` ends rather
    /// than hangs.
    pub fn feed(&mut self, interleaved: &[f32]) {
        let mut rest = interleaved;
        while !rest.is_empty() {
            let Ok(mut slot) = self.shared.sink.lock() else {
                return;
            };
            let Some(sink) = slot.as_mut() else {
                return;
            };
            let room = sink.room();
            if room == 0 {
                drop(slot);
                std::thread::yield_now();
                continue;
            }
            let take = room.min(rest.len());
            sink.push(&rest[..take]);
            rest = &rest[take..];
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{RawSink, SourceSpec};
    use std::sync::Arc;
    use std::sync::atomic::{AtomicU64, Ordering};

    #[test]
    fn a_full_ring_drops_and_counts_instead_of_blocking() {
        let (producer, consumer) = rtrb::RingBuffer::<f32>::new(8);
        let overruns = Arc::new(AtomicU64::new(0));
        let mut sink = RawSink::new(producer, Arc::clone(&overruns));

        sink.push(&[1.0; 6]);
        assert_eq!(overruns.load(Ordering::Relaxed), 0);

        // Four more into two free slots: two land, two are counted and gone.
        sink.push(&[2.0; 4]);
        assert_eq!(overruns.load(Ordering::Relaxed), 2);
        assert_eq!(sink.room(), 0);
        drop(consumer);
    }

    #[test]
    fn a_spec_is_the_two_numbers_the_converter_needs() {
        let spec = SourceSpec::new(44_100, 2);
        assert_eq!(spec.sample_rate, 44_100);
        assert_eq!(spec.channels, 2);
    }
}
