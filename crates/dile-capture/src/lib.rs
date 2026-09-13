//! Microphone capture for Dile: hold a key, speak, let go, get a buffer.
//!
//! This is the audio half of `docs/PROJECT.md` WP2. The hotkey half lives elsewhere; this
//! crate opens no window, registers no shortcut and depends on no Tauri. It takes a
//! microphone and produces exactly what `dile-engine` accepts — **16 kHz mono `f32`** — plus
//! the two things the rest of the product needs beside the audio: a live level stream for
//! the panel's meter, and a speech summary so that silence is never sent to a decoder.
//!
//! ```no_run
//! use dile_capture::{Capture, CaptureConfig};
//!
//! // Opening arms the stream: the pre-roll ring starts filling, nothing is recorded yet.
//! let capture = Capture::open(CaptureConfig::default())?;
//!
//! // The key went down. Recording starts here, and the pre-roll goes in front of it.
//! capture.start()?;
//! // ... the user speaks ...
//! let recording = capture.stop()?;
//!
//! if recording.speech.has_speech {
//!     // `recording.samples` is what `dile_engine::Engine::transcribe` wants.
//! }
//! # Ok::<(), dile_capture::Error>(())
//! ```
//!
//! # The shape of it, and why
//!
//! ```text
//!   cpal callback ──push──▶ lock-free ring ──▶ worker thread ──▶ Recording
//!   (real time)             (rtrb, SPSC)        downmix
//!                                               resample to 16 kHz (rubato)
//!                                               pre-roll ring / record / cap
//!                                               level meter ──▶ bounded channel
//!                                               VAD ──▶ SpeechSummary
//! ```
//!
//! * **Nothing but a push happens in the audio callback.** No allocation, no mutex, no
//!   resampler. A callback that misses its deadline is a click in the recording, and a
//!   callback that panics takes the process down. When the ring fills because the worker was
//!   descheduled, the samples that do not fit are counted in [`Capture::overruns`] and
//!   dropped — never waited for.
//! * **The pre-roll ring is the first-word protection** WP2's acceptance criterion asks for.
//!   A push-to-talk key reaches the application after the first syllable has begun; the ring
//!   holds the last `pre_roll_ms` of converted audio, and [`Capture::start`] puts it at the
//!   head of the recording.
//! * **The recording is never trimmed.** The VAD produces a [`SpeechSummary`] beside the
//!   audio and nothing else. Its job is to let the caller skip the engine on silence — the
//!   silence half of the hallucination guard, after M0 found every canned hallucination in
//!   the robustness set sitting on the same ten seconds of applause (`docs/PROJECT.md`, log
//!   2026-09-09) — not to decide which samples the user meant.
//! * **The whole pipeline after the ring is driven through [`Source`]**, so the tests feed
//!   it a synthetic tone and CI, which has no audio device, runs all of them. The two
//!   functions that touch real hardware are [`Capture::open`] and [`Capture::devices`], and
//!   the one test that needs a microphone is `#[ignore]`.
//!
//! # Trying it on a real microphone
//!
//! ```text
//! cargo run -p dile-capture --example record -- 3
//! ```
//!
//! Arms the stream for a second, records for three seconds (or whatever number is given),
//! draws a level bar at 20 Hz, writes `out.wav` in the working directory as 16 kHz mono, and
//! prints the speech summary, the overrun count and whether the cap was hit. It is the check
//! that the device opens and the conversion is right; the automated tests cover everything
//! after that.
//!
//! # What is not here
//!
//! The hotkey, the panel, the paste and the engine. `earshot` and Silero are not integrated
//! either: [`Vad`] is the seam M0's winner drops into, and [`RmsVad`] is the relative-RMS
//! baseline `docs/PROJECT.md` WP1 asks for so that the learned detectors have something to
//! beat.

#![forbid(unsafe_code)]

mod convert;
mod device;
mod error;
mod level;
mod pipeline;
mod source;
mod vad;

use std::sync::Arc;
use std::sync::atomic::{AtomicU8, AtomicU64, Ordering};
use std::thread::JoinHandle;
use std::time::Duration;

use crossbeam_channel::{Receiver, Sender, bounded};

pub use crate::device::DeviceInfo;
pub use crate::error::Error;
pub use crate::level::{LevelEvent, MIN_DBFS};
pub use crate::pipeline::Recording;
pub use crate::source::{
    RawSink, Source, SourceSpec, StreamGuard, SyntheticFeeder, SyntheticSource,
};
pub use crate::vad::{Decision, RmsVad, RmsVadConfig, SpeechSummary, Vad};

use crate::pipeline::Pipeline;

/// The rate every buffer leaving this crate is at. The same constant `dile-engine` states.
pub const SAMPLE_RATE: u32 = 16_000;

/// The shortest recording cap the settings accept, in seconds.
pub const MIN_CAP_SECS: u32 = 1;

/// The longest recording cap the settings accept, in seconds.
///
/// `docs/PROJECT.md` §7: "A setting: default 60 s, maximum 300 s."
pub const MAX_CAP_SECS: u32 = 300;

/// The receiving end of the level stream.
///
/// A `crossbeam_channel::Receiver` under an alias, so a caller can name the type without
/// depending on the channel crate itself.
pub type LevelReceiver = Receiver<LevelEvent>;

/// How many level events are held for a reader that has fallen behind.
///
/// Three seconds at the default interval. Past that the oldest are dropped: a meter is a
/// live reading, and an old one is worse than none.
const LEVEL_BACKLOG: usize = 64;

/// How long the worker waits for a command before looking at the ring again.
const WORKER_POLL: Duration = Duration::from_millis(4);

/// How the microphone is opened and how the recording behaves.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CaptureConfig {
    /// Which input to open: a [`DeviceInfo::id`], a [`DeviceInfo::name`], or the host
    /// default when `None`.
    pub device: Option<String>,
    /// How much audio from before the key went down is kept in front of the recording.
    pub pre_roll_ms: u32,
    /// How long a single recording may run, in seconds.
    ///
    /// Clamped to [`MIN_CAP_SECS`]..=[`MAX_CAP_SECS`] rather than rejected — a settings file
    /// that has been hand-edited to 600 should record for five minutes, not refuse to open
    /// the microphone. The clamp is logged.
    pub cap_secs: u32,
    /// How often a [`LevelEvent`] is produced, in milliseconds.
    ///
    /// The panel's meter needs at least 20 Hz (WP2's acceptance criterion), which is what
    /// the default 50 ms gives.
    pub level_interval_ms: u32,
}

impl Default for CaptureConfig {
    fn default() -> Self {
        CaptureConfig {
            device: None,
            pre_roll_ms: 500,
            cap_secs: 60,
            level_interval_ms: 50,
        }
    }
}

impl CaptureConfig {
    /// The config as it will actually be used, with every out-of-range value clamped.
    #[must_use]
    pub fn clamped(&self) -> CaptureConfig {
        let mut clamped = self.clone();

        clamped.cap_secs = self.cap_secs.clamp(MIN_CAP_SECS, MAX_CAP_SECS);
        if clamped.cap_secs != self.cap_secs {
            log::warn!(
                "recording cap of {} s is outside {}..={} s and was clamped to {} s",
                self.cap_secs,
                MIN_CAP_SECS,
                MAX_CAP_SECS,
                clamped.cap_secs
            );
        }

        clamped.level_interval_ms = self.level_interval_ms.clamp(5, 1000);
        if clamped.level_interval_ms != self.level_interval_ms {
            log::warn!(
                "level interval of {} ms is outside 5..=1000 ms and was clamped to {} ms",
                self.level_interval_ms,
                clamped.level_interval_ms
            );
        }

        clamped.pre_roll_ms = self.pre_roll_ms.min(5_000);
        if clamped.pre_roll_ms != self.pre_roll_ms {
            log::warn!(
                "pre-roll of {} ms is longer than the 5000 ms maximum and was clamped",
                self.pre_roll_ms
            );
        }

        clamped
    }
}

/// Where a [`Capture`] is.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(u8)]
pub enum State {
    /// The stream is open and the pre-roll ring is filling. Nothing is being recorded.
    Armed = 0,
    /// Recording, and the elapsed time is running against the cap.
    Recording = 1,
    /// The cap stopped the recording by itself; the buffer is waiting for
    /// [`Capture::stop`].
    ///
    /// The user may still be holding the key. `docs/PROJECT.md` §3 puts the cap on the
    /// panel for exactly this moment, so that "it stopped" is something they are told rather
    /// than something they discover.
    Capped = 2,
}

impl State {
    fn from_u8(value: u8) -> State {
        match value {
            1 => State::Recording,
            2 => State::Capped,
            _ => State::Armed,
        }
    }
}

/// Every command carries the channel its answer goes back on.
///
/// The acknowledgement is what makes "recording begins now" true. Without it `start()`
/// would return while the worker was still draining the ring, and the audio that arrived
/// in that window would land in the pre-roll instead of in the recording — invisible with
/// a long pre-roll, and a lost half second with none.
enum Command {
    Start(Sender<()>),
    Stop(Sender<Recording>),
    Discard(Sender<()>),
    Shutdown,
}

/// An open microphone.
///
/// Opening arms the stream; [`Capture::start`] and [`Capture::stop`] bracket one dictation;
/// the stream stays open in between so that the next press does not pay for a device open.
/// Dropping it closes the device and stops the worker.
pub struct Capture {
    commands: Sender<Command>,
    levels: Receiver<LevelEvent>,
    overruns: Arc<AtomicU64>,
    state: Arc<AtomicU8>,
    config: CaptureConfig,
    spec: SourceSpec,
    worker: Option<JoinHandle<()>>,
    /// Dropped before the worker is joined, so the callback has stopped by then.
    stream: Option<Box<dyn StreamGuard>>,
}

impl Capture {
    /// Open the configured microphone and arm it.
    ///
    /// Nothing is recorded until [`Capture::start`]; what happens immediately is that the
    /// pre-roll ring begins to fill and [`Capture::levels`] begins to produce events, which
    /// is what lets the panel show a live meter before the first dictation.
    pub fn open(config: CaptureConfig) -> Result<Capture, Error> {
        let source = device::DeviceSource::open(config.device.as_deref())?;
        let spec = source.spec();
        log::info!(
            "input device {:?}: {} Hz, {} channel(s), {}",
            source.name(),
            spec.sample_rate,
            spec.channels,
            source.sample_format()
        );
        Capture::open_with(config, Box::new(source))
    }

    /// Open on a [`Source`] of your own, hardware or not.
    ///
    /// The seam the tests use, through [`SyntheticSource`]. It is public rather than
    /// `pub(crate)` because it is also the honest way for `dile-app` to run the capture
    /// stage against a recorded file when a bug report arrives with a WAV attached.
    pub fn open_with(config: CaptureConfig, source: Box<dyn Source>) -> Result<Capture, Error> {
        let config = config.clamped();
        let spec = source.spec();

        let pipeline = Pipeline::new(
            spec,
            config.pre_roll_ms,
            config.cap_secs,
            config.level_interval_ms,
            Box::new(RmsVad::new()),
        )?;

        // Half a second of raw audio. Long enough that a descheduled worker loses nothing,
        // short enough that a worker that has genuinely stopped is visible in the overrun
        // count rather than hidden behind a minute of buffering.
        let ring_capacity = (spec.sample_rate as usize)
            .saturating_mul(usize::from(spec.channels).max(1))
            .div_ceil(2)
            .max(8192);
        let (producer, consumer) = rtrb::RingBuffer::<f32>::new(ring_capacity);

        let overruns = Arc::new(AtomicU64::new(0));
        let state = Arc::new(AtomicU8::new(State::Armed as u8));
        let (command_tx, command_rx) = bounded(16);
        let (level_tx, level_rx) = bounded(LEVEL_BACKLOG);

        let worker_state = Arc::clone(&state);
        let worker_levels = level_rx.clone();
        let worker = std::thread::Builder::new()
            .name("dile-capture".to_string())
            .spawn(move || {
                run(
                    consumer,
                    command_rx,
                    pipeline,
                    level_tx,
                    worker_levels,
                    worker_state,
                );
            })
            .map_err(|error| Error::Buffer(error.to_string()))?;

        let sink = RawSink::new(producer, Arc::clone(&overruns));
        let stream = source.start(sink)?;

        Ok(Capture {
            commands: command_tx,
            levels: level_rx,
            overruns,
            state,
            config,
            spec,
            worker: Some(worker),
            stream: Some(stream),
        })
    }

    /// Every input device on the default host, for the settings window.
    pub fn devices() -> Result<Vec<DeviceInfo>, Error> {
        device::list()
    }

    /// Start recording now, with the pre-roll ring's contents in front.
    ///
    /// Everything the source produced before this call belongs to the pre-roll, and
    /// everything after it to the recording. Returns once the worker has drained the ring
    /// and flipped, so the boundary is this call rather than whenever the worker next
    /// happened to wake — a round trip of well under a millisecond on an idle machine.
    pub fn start(&self) -> Result<(), Error> {
        let (tx, rx) = bounded(1);
        self.send(Command::Start(tx))?;
        rx.recv().map_err(|_| Error::WorkerGone)
    }

    /// Stop recording and take the buffer.
    ///
    /// The stream stays open and armed; the pre-roll ring starts filling again immediately.
    /// Calling this without a [`Capture::start`] returns an empty [`Recording`] rather than
    /// an error — a release with no press is a state the hotkey layer can genuinely produce
    /// (the application started with the key already down) and it is not a fault.
    pub fn stop(&self) -> Result<Recording, Error> {
        let (tx, rx) = bounded(1);
        self.send(Command::Stop(tx))?;
        rx.recv().map_err(|_| Error::WorkerGone)
    }

    /// Throw away the recording in progress and stay armed.
    ///
    /// What a tap shorter than the press-length threshold does: `docs/PROJECT.md` §3 says a
    /// tap "does nothing except open the panel idle, so an accidental tap never dictates".
    /// Returns once the buffer is gone, like [`Capture::start`].
    pub fn discard(&self) -> Result<(), Error> {
        let (tx, rx) = bounded(1);
        self.send(Command::Discard(tx))?;
        rx.recv().map_err(|_| Error::WorkerGone)
    }

    /// The live level stream, one event per `level_interval_ms` of audio.
    ///
    /// Events arrive while armed as well as while recording. The channel is bounded and
    /// drops its oldest event when a reader falls behind, so a panel that is not being
    /// painted can never stall the audio path.
    ///
    /// The returned receiver may be cloned, but the events are not: two readers share one
    /// stream rather than each getting a copy.
    #[must_use]
    pub fn levels(&self) -> LevelReceiver {
        self.levels.clone()
    }

    /// Where the capture is right now.
    ///
    /// [`State::Capped`] is how the caller learns the cap stopped the recording while the
    /// key is still held.
    #[must_use]
    pub fn state(&self) -> State {
        State::from_u8(self.state.load(Ordering::Relaxed))
    }

    /// Raw samples dropped because the ring was full, since [`Capture::open`].
    ///
    /// Non-zero means the machine could not keep up and the recording has a gap in it.
    #[must_use]
    pub fn overruns(&self) -> u64 {
        self.overruns.load(Ordering::Relaxed)
    }

    /// The config in force, after clamping.
    #[must_use]
    pub fn config(&self) -> &CaptureConfig {
        &self.config
    }

    /// The device's native rate and channel count, before conversion.
    #[must_use]
    pub fn source_spec(&self) -> SourceSpec {
        self.spec
    }

    fn send(&self, command: Command) -> Result<(), Error> {
        self.commands.send(command).map_err(|_| Error::WorkerGone)
    }
}

impl Drop for Capture {
    fn drop(&mut self) {
        // The stream first: the callback stops before the worker does, so nothing is left
        // pushing into a ring nobody drains.
        self.stream = None;
        let _ = self.commands.send(Command::Shutdown);
        if let Some(worker) = self.worker.take() {
            let _ = worker.join();
        }
    }
}

/// The worker thread: drain the ring, run the pipeline, answer commands.
fn run(
    mut consumer: rtrb::Consumer<f32>,
    commands: Receiver<Command>,
    mut pipeline: Pipeline,
    levels: Sender<LevelEvent>,
    level_drain: Receiver<LevelEvent>,
    state: Arc<AtomicU8>,
) {
    loop {
        drain(&mut consumer, &mut pipeline, &levels, &level_drain);

        // Commands are handled after the ring is drained, and the ring is drained again
        // before each one takes effect: `start()` must not sweep audio from before the key
        // press into the recording, and `stop()` must not leave audio from before the
        // release out of it.
        let command = match commands.recv_timeout(WORKER_POLL) {
            Ok(command) => command,
            Err(crossbeam_channel::RecvTimeoutError::Timeout) => {
                state.store(pipeline.state() as u8, Ordering::Relaxed);
                continue;
            }
            Err(crossbeam_channel::RecvTimeoutError::Disconnected) => break,
        };

        drain(&mut consumer, &mut pipeline, &levels, &level_drain);

        // **The state is published before the answer, in every arm.** The acknowledgement is
        // what makes "recording begins now" true for the caller, so everything the caller can
        // observe the moment it returns has to be true already. Storing the state after the
        // reply left a window — narrow, and wide enough to lose on a loaded machine — in
        // which `stop()` had handed back the recording and `state()` still said `Recording`.
        // Found by the test at the bottom of this file going red on a CI runner, 2026-09-13,
        // rather than by anybody reading the code.
        match command {
            Command::Start(reply) => {
                pipeline.start();
                state.store(pipeline.state() as u8, Ordering::Relaxed);
                let _ = reply.send(());
            }
            Command::Stop(reply) => {
                let recording = pipeline.stop();
                state.store(pipeline.state() as u8, Ordering::Relaxed);
                let _ = reply.send(recording);
            }
            Command::Discard(reply) => {
                pipeline.discard();
                state.store(pipeline.state() as u8, Ordering::Relaxed);
                let _ = reply.send(());
            }
            Command::Shutdown => break,
        }
    }
}

/// Move everything the ring holds through the pipeline.
fn drain(
    consumer: &mut rtrb::Consumer<f32>,
    pipeline: &mut Pipeline,
    levels: &Sender<LevelEvent>,
    level_drain: &Receiver<LevelEvent>,
) {
    loop {
        let available = consumer.slots();
        if available == 0 {
            return;
        }
        let Ok(chunk) = consumer.read_chunk(available) else {
            return;
        };
        let (first, second) = chunk.as_slices();

        let mut emit = |event: LevelEvent| publish(levels, level_drain, event);
        for slice in [first, second] {
            if slice.is_empty() {
                continue;
            }
            if let Err(error) = pipeline.feed(slice, &mut emit) {
                // Conversion is arithmetic on a buffer; the only way it fails is a bug in
                // this crate. Dropping the block and carrying on keeps the microphone open,
                // and the log line is what turns it into a report.
                log::error!("capture pipeline failed on a block and dropped it: {error}");
            }
        }
        chunk.commit_all();
    }
}

/// Send a level event, dropping the oldest rather than blocking when nobody is reading.
fn publish(levels: &Sender<LevelEvent>, drain: &Receiver<LevelEvent>, event: LevelEvent) {
    if levels.try_send(event).is_ok() {
        return;
    }
    // Full. Make room by discarding the stalest reading and try once more; if that also
    // fails, the reader has gone and the event is not worth another attempt.
    let _ = drain.try_recv();
    let _ = levels.try_send(event);
}

#[cfg(test)]
mod tests {
    use super::{
        Capture, CaptureConfig, MAX_CAP_SECS, MIN_CAP_SECS, SAMPLE_RATE, State, SyntheticSource,
    };

    const NATIVE_RATE: u32 = 48_000;

    fn tone(seconds: f32, amplitude: f32, phase: &mut usize) -> Vec<f32> {
        let frames = (NATIVE_RATE as f32 * seconds) as usize;
        (0..frames)
            .map(|_| {
                let value = amplitude
                    * (std::f32::consts::TAU * 440.0 * *phase as f32 / NATIVE_RATE as f32).sin();
                *phase += 1;
                value
            })
            .collect()
    }

    #[test]
    fn the_default_config_is_the_one_the_spec_names() {
        let config = CaptureConfig::default();
        assert_eq!(config.pre_roll_ms, 500);
        assert_eq!(config.cap_secs, 60);
        assert_eq!(
            config.level_interval_ms, 50,
            "50 ms is 20 Hz, the panel's floor"
        );
        assert_eq!(config.device, None);
        assert_eq!(
            config.clamped(),
            config,
            "the defaults are already in range"
        );
    }

    #[test]
    fn an_out_of_range_cap_is_clamped_rather_than_rejected() {
        let too_long = CaptureConfig {
            cap_secs: 600,
            ..CaptureConfig::default()
        };
        assert_eq!(too_long.clamped().cap_secs, MAX_CAP_SECS);

        let zero = CaptureConfig {
            cap_secs: 0,
            ..CaptureConfig::default()
        };
        assert_eq!(zero.clamped().cap_secs, MIN_CAP_SECS);

        // And the other two knobs, which can otherwise divide by zero or hold a ring the
        // size of the whole recording.
        let silly = CaptureConfig {
            level_interval_ms: 0,
            pre_roll_ms: 60_000,
            ..CaptureConfig::default()
        };
        let clamped = silly.clamped();
        assert_eq!(clamped.level_interval_ms, 5);
        assert_eq!(clamped.pre_roll_ms, 5_000);
    }

    #[test]
    fn a_whole_dictation_runs_through_the_threaded_pipeline() {
        // The same scenario as the pipeline's own test, but through the worker thread, the
        // ring and the command channel — which is the part that has a clock in it.
        let (source, mut feeder) = SyntheticSource::new(NATIVE_RATE, 1);
        let capture = Capture::open_with(CaptureConfig::default(), Box::new(source))
            .expect("a synthetic source always opens");

        let mut phase = 0;
        assert_eq!(capture.source_spec().sample_rate, NATIVE_RATE);

        feeder.feed(&tone(1.5, 0.4, &mut phase));
        capture.start().expect("the worker is alive");
        feeder.feed(&tone(0.5, 0.4, &mut phase));
        let recording = capture.stop().expect("the worker is alive");

        let ms = recording.samples.len() as u64 * 1000 / u64::from(SAMPLE_RATE);
        assert!(
            ms.abs_diff(1000) <= 30,
            "500 ms of pre-roll plus 500 ms recorded, got {ms} ms"
        );
        assert!(recording.speech.has_speech);
        assert!(!recording.cap_hit);
        assert_eq!(
            capture.overruns(),
            0,
            "the feeder waits for room, so nothing is lost"
        );
        assert_eq!(capture.state(), State::Armed);
    }

    #[test]
    fn level_events_arrive_at_twenty_hertz_while_armed() {
        // WP2's acceptance criterion, and the half of it that is easy to miss: the meter has
        // to move before anything is recorded, or a dead microphone looks like a quiet one.
        let (source, mut feeder) = SyntheticSource::new(NATIVE_RATE, 1);
        let capture = Capture::open_with(CaptureConfig::default(), Box::new(source))
            .expect("a synthetic source always opens");
        let levels = capture.levels();

        let mut phase = 0;
        feeder.feed(&tone(1.0, 1.0, &mut phase));
        // A command round-trip is what proves the worker has drained the ring: `stop`
        // answers only after the drain.
        let _ = capture.stop().expect("the worker is alive");

        let events: Vec<_> = levels.try_iter().collect();
        assert!(
            events.len() >= 19,
            "a second of audio at 50 ms per event is 20 events, got {}",
            events.len()
        );
        for event in &events {
            assert!(
                (event.rms_dbfs - (-3.01)).abs() < 0.5,
                "a full-scale sine reads about -3 dBFS, got {}",
                event.rms_dbfs
            );
        }
    }

    #[test]
    fn the_cap_fires_through_the_worker_and_the_stream_stays_armed() {
        let (source, mut feeder) = SyntheticSource::new(NATIVE_RATE, 1);
        let capture = Capture::open_with(
            CaptureConfig {
                pre_roll_ms: 100,
                cap_secs: 1,
                ..CaptureConfig::default()
            },
            Box::new(source),
        )
        .expect("a synthetic source always opens");

        let mut phase = 0;
        feeder.feed(&tone(0.3, 0.4, &mut phase));
        capture.start().expect("the worker is alive");
        feeder.feed(&tone(2.0, 0.4, &mut phase));

        let recording = capture.stop().expect("the worker is alive");
        assert!(recording.cap_hit, "two seconds against a one second cap");
        let ms = recording.samples.len() as u64 * 1000 / u64::from(SAMPLE_RATE);
        assert!(
            ms.abs_diff(1100) <= 30,
            "100 ms of pre-roll plus exactly the cap, got {ms} ms"
        );
        assert_eq!(
            capture.state(),
            State::Armed,
            "and the stream is still open"
        );
    }

    #[test]
    fn stopping_without_starting_is_an_empty_recording_rather_than_an_error() {
        let (source, mut feeder) = SyntheticSource::new(NATIVE_RATE, 1);
        let capture = Capture::open_with(CaptureConfig::default(), Box::new(source))
            .expect("a synthetic source always opens");

        let mut phase = 0;
        feeder.feed(&tone(0.2, 0.4, &mut phase));
        let recording = capture.stop().expect("the worker is alive");

        assert!(recording.is_empty());
        assert!(!recording.cap_hit);
        assert!(!recording.speech.has_speech);
    }

    #[test]
    fn a_sixteen_kilohertz_device_needs_no_resampler_and_loses_no_samples() {
        let (source, mut feeder) = SyntheticSource::new(SAMPLE_RATE, 1);
        let capture = Capture::open_with(
            CaptureConfig {
                pre_roll_ms: 0,
                ..CaptureConfig::default()
            },
            Box::new(source),
        )
        .expect("a synthetic source always opens");

        capture.start().expect("the worker is alive");
        feeder.feed(&vec![0.25f32; SAMPLE_RATE as usize / 2]);
        let recording = capture.stop().expect("the worker is alive");

        assert_eq!(
            recording.samples.len(),
            SAMPLE_RATE as usize / 2,
            "pass-through means every sample, not almost every sample"
        );
        assert!(recording.samples.iter().all(|&sample| sample == 0.25));
    }

    #[test]
    fn a_discarded_tap_leaves_the_stream_armed_for_the_next_press() {
        let (source, mut feeder) = SyntheticSource::new(NATIVE_RATE, 1);
        let capture = Capture::open_with(CaptureConfig::default(), Box::new(source))
            .expect("a synthetic source always opens");

        let mut phase = 0;
        feeder.feed(&tone(0.8, 0.4, &mut phase));
        capture.start().expect("the worker is alive");
        feeder.feed(&tone(0.1, 0.4, &mut phase));
        capture.discard().expect("the worker is alive");
        assert!(capture.stop().expect("the worker is alive").is_empty());

        feeder.feed(&tone(0.6, 0.4, &mut phase));
        capture.start().expect("the worker is alive");
        feeder.feed(&tone(0.2, 0.4, &mut phase));
        let recording = capture.stop().expect("the worker is alive");

        let ms = recording.samples.len() as u64 * 1000 / u64::from(SAMPLE_RATE);
        assert!(
            ms.abs_diff(700) <= 30,
            "500 ms of pre-roll plus 200 ms recorded, got {ms} ms"
        );
    }
}
