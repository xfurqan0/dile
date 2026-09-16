//! Speech to text, and nothing else.
//!
//! A thin wrapper over [`transcribe_cpp`], which `docs/PROJECT.md` §3 makes **the single
//! decision runtime**: one runtime for Whisper, Qwen3-ASR and Nemotron, with Vulkan, CUDA
//! and Metal behind one API. The wrapper exists so that the rest of the product depends on
//! this crate's four types rather than on the runtime's forty, and so that the decoder
//! settings the M0 protocol fixes live in exactly one place.
//!
//! ```no_run
//! use dile_engine::{Compute, Model};
//!
//! # let pcm_16khz_mono: Vec<f32> = Vec::new();
//! let model = Model::load("whisper-large-v3-turbo-F16.gguf", Compute::Cpu)?;
//! let mut engine = model.engine()?;
//! let transcript = engine.transcribe(&pcm_16khz_mono, "tr")?;
//! println!("{}", transcript.text);
//! # Ok::<(), dile_engine::Error>(())
//! ```
//!
//! ## What is fixed here, and why
//!
//! * **The GPU is a compile-time feature and a probed runtime tier, never a guess.**
//!   [`Compute::Vulkan`] needs the `gpu-vulkan` cargo feature; without it, asking for the
//!   GPU is a typed error rather than a silent fall back to the CPU. The product ships with
//!   that feature on and prefers Vulkan, but only after a probe transcription succeeds in
//!   this process; otherwise the caller drops to the CPU tier (`docs/PROJECT.md` §3).
//!   Handy's open bug #1755 — a Vulkan auto-GPU path bug-checking an RTX 5090 — is why the
//!   answer is a probe rather than an "Auto" setting.
//! * **Greedy decoding, temperature 0.** `transcribe-cpp` 0.2.3 exposes **no beam-size
//!   knob** and the whisper family samples greedily; verified in its sources and by running
//!   it (`docs/PROJECT.md`, log 2026-09-09). So the only decoder knobs that exist are
//!   temperature and the prompt. Temperature is pinned at 0, because a dictation that
//!   changes its mind between two runs of the same audio is a bug report nobody can act on;
//!   the **prompt is the one parameter**, because it is what the dictionary is fed through
//!   and it is what moved accuracy the most in M0 — WER 0.379 → 0.261 and 17 → 30 of 31
//!   technical terms.
//! * **Segment timestamps only.** Word granularity returns *unsupported timestamp
//!   granularity* from this runtime. Dictation does not need word times.
//! * **16 kHz mono `f32`.** The buffer is what the capture stage (WP2) produces; this crate
//!   opens no microphone and reads no file.
//!
//! ## Who links this
//!
//! **`dile-engine-host` and nothing else.** Process isolation (WP3) is the reason this crate
//! is separate from the application: the engine runs in its own process so that a driver
//! fault takes the engine down and not the tray. The tray talks to that process over
//! `dile-engine-proto`, and `dile-app` has no dependency on this crate at all.

#![forbid(unsafe_code)]

use std::fmt;
use std::path::Path;
use std::sync::Once;

use transcribe_cpp::{
    Backend, ModelOptions, RunExtension, RunOptions, SessionOptions, TimestampKind,
    WhisperRunOptions,
};

/// The sample rate every buffer this crate accepts must be at.
pub const SAMPLE_RATE: u32 = 16_000;

/// The most threads [`default_threads`] will ever ask for, however large the machine is.
///
/// A ceiling rather than a target, and the measurement below is why there is one at all:
/// asking for more threads than a machine has is not merely wasted, it is **slower than the
/// runtime's own capped default**. On the 14-core, 20-thread i9-13900H the CPU tier was
/// measured at, thirty-two threads took 25.7 s against twenty's 15.0 s and the capped
/// default's 28.3 s — oversubscription gave most of the win straight back. Nothing below can
/// exceed what [`std::thread::available_parallelism`] reports, so this number never bites on
/// a machine like that one; it is here for the other end, where the gain from more threads
/// was already sublinear at twenty (ten to twenty threads bought 1.54x, not 2x) and a
/// hundred and twenty-eight of them would buy contention.
pub const MAX_THREADS: u32 = 32;

/// How many CPU threads the engine should ask for on a machine that has `available` of them.
///
/// The whole of the policy, as a function of one number, so that the decision is testable on
/// a runner whose core count is nothing like a laptop's. [`default_threads`] is this function
/// applied to the machine it is running on.
///
/// `None` means the platform would not say how much parallelism there is, and the answer is
/// then `0` — which is the wire and runtime spelling of *you decide*. A guess would be worse
/// than the runtime's own conservative default in exactly the case where nothing is known.
#[must_use]
pub fn threads_for(available: Option<std::num::NonZeroUsize>) -> u32 {
    let Some(available) = available else {
        return 0;
    };
    // `usize` to `u32` cannot be done by `try_from` alone here without turning a machine with
    // more threads than `u32` can hold into a zero, which would read as "you decide" and
    // quietly undo the whole change. Saturating, then clamped, keeps a huge machine at the
    // ceiling where it belongs.
    let available = u32::try_from(available.get()).unwrap_or(u32::MAX);
    available.min(MAX_THREADS)
}

/// How many CPU threads the engine should ask for on **this** machine.
///
/// **Not the runtime's own default, and that is the point.** `transcribe-cpp` defaults to the
/// number of CPUs the process may run on *capped at eight* —
/// `src/transcribe-batch-util.h`, `default_n_threads(int cap = 8)` — and above eight hardware
/// threads the rest of the machine sits idle. Measured on a 14-core, 20-thread i9-13900H
/// against the committed two-second probe clip, three runs each, on battery under the
/// `powersave` governor:
///
/// | threads | median | real-time factor against the 30 s window |
/// |---|---|---|
/// | `0` (the runtime's own) | 28.3 s | 0.94 |
/// | 8 | 28.3 s | 0.94 |
/// | 10 | 23.1 s | 0.77 |
/// | 14 (the physical cores) | 17.5 s | 0.58 |
/// | 20 (every hardware thread) | **15.0 s** | **0.50** |
/// | 32 | 25.7 s | 0.86 |
///
/// `0` and `8` agreeing to within a tenth of a second is the cap being *observed* rather than
/// read out of a header, and the twenty-thread row is why this asks for logical parallelism
/// rather than physical cores: the eight extra hyperthreads were worth another 14 % on top of
/// the fourteen physical ones, which is small but is the right sign — and reading physical
/// core counts needs a different piece of platform code on every platform for a number that
/// measured *worse*.
///
/// `docs/BUILDING.md` carries the same table for somebody deciding whether to trust it.
#[must_use]
pub fn default_threads() -> u32 {
    threads_for(std::thread::available_parallelism().ok())
}

/// The encoder positions a full Whisper window has, and the spelling of *do not shorten it*.
///
/// Whisper's encoder is fixed at thirty seconds: the frontend pads any shorter buffer to
/// 480000 samples, the mel to 3000 frames, and the encoder attends over all 1500 positions.
/// So a three-second dictation pays for twenty-seven seconds of zeros — and on this
/// workload that one encoder pass is **80–93 % of the wall clock** (measured on an Iris Xe
/// under Vulkan and on twenty CPU threads; `docs/PROJECT.md` §3, WP8).
pub const FULL_AUDIO_CTX: i32 = 1500;

/// The shortest encoder window [`audio_ctx_for`] will ever ask for.
///
/// A measurement, and a conservative reading of it. The window that holds a short take
/// comfortably is far narrower than this — a three-second clip decodes correctly at 256 —
/// but the failures found while measuring were not near the point where the window stops
/// holding the audio, and they were not small. Forced to 512, a five-second recording of
/// the same sentence twice came back with the sentence **once** on one of the three model
/// and device combinations tried, and correct on the other two. A dropped sentence that
/// only appears on one backend is exactly the bug that cannot be reproduced from a report,
/// so the floor sits at the narrowest width that was clean everywhere.
///
/// Below it the failure is not an error. At 256 a three-second clip is correct; at 192 it
/// returned the sentence **four times over and took 40 s**, and at 128 seven times over and
/// 22–50 s. The shortened window puts the decoder into a repetition loop, which trips
/// `compression_ratio_thold`, which starts the temperature fallback ladder — and everything
/// the encoder saved comes back multiplied.
pub const MIN_AUDIO_CTX: i32 = 640;

/// Encoder positions per second of audio the window is scaled by.
///
/// Fifty positions is what one second of audio *occupies* (1500 over 30 s), so this is a
/// **factor of two of headroom** — and the headroom is the entire safety margin, arrived at
/// by finding where a smaller one breaks. A nine-second recording of two Turkish sentences,
/// on the shipped tier's model:
///
/// | window | result |
/// |---|---|
/// | 512 | 35.0 s, ending in a hallucinated subtitle credit |
/// | 603 — what 67 positions per second asks for | 3.5 s, the second sentence repeated |
/// | **640** | **2.2 s, correct** |
/// | 768 | 2.4 s, correct |
///
/// Sixty-seven per second is the figure the first round of research arrived at, and it is
/// **inside the broken band** on this stack. Being generous costs little and the cost is
/// bounded — past about six seconds the saving was already shrinking, and at twenty seconds
/// it was 16 % — while being tight costs a 35-second answer that is also wrong.
const AUDIO_CTX_PER_SECOND: f64 = 100.0;

/// How wide an encoder window a buffer of `samples` should be transcribed in.
///
/// The whole of the policy as a function of one number, so the decision is testable without
/// a model, a GPU or a microphone. [`FULL_AUDIO_CTX`] is the answer for anything long enough
/// that shortening buys nothing — and returning it there is the *load-bearing* half: a fixed
/// 512 on a 29.5-second take ran **eight times slower than the default and hallucinated**.
/// A dial that is wrong in that direction is worse than no dial.
///
/// | take | window | on the shipped tier, against the full window |
/// |---|---|---|
/// | ≤ 6.4 s | 640 — the floor | 3 s: 1.3 s against 3.9 s |
/// | 10 s | 1000 | 2.3 s against 3.8 s |
/// | ≥ 15 s | 1500 — the full window, i.e. unchanged | — |
#[must_use]
pub fn audio_ctx_for(samples: usize) -> i32 {
    // `f64` and not `f32`: five minutes of audio is 4.8 million samples, past the point where
    // an `f32` counts them exactly, and this number decides whether speech gets dropped.
    let seconds = samples as f64 / f64::from(SAMPLE_RATE);
    let wanted = (seconds * AUDIO_CTX_PER_SECOND).ceil();
    // Through `f64` first: a take long enough to overflow `i32` positions must land on the
    // ceiling, not wrap into the broken band.
    if wanted >= f64::from(FULL_AUDIO_CTX) {
        return FULL_AUDIO_CTX;
    }
    (wanted as i32).clamp(MIN_AUDIO_CTX, FULL_AUDIO_CTX)
}

/// The window this process was told to use, overriding [`audio_ctx_for`], or `None`.
///
/// `DILE_AUDIO_CTX` exists so the measurement table in `docs/BUILDING.md` can be reproduced
/// and so a machine that regresses has a one-variable way out: `DILE_AUDIO_CTX=0` restores
/// the full thirty-second window and with it exactly the behaviour of every build before
/// this one. Values below [`MIN_AUDIO_CTX`] are *allowed* here and nowhere else, because
/// measuring the broken band is the reason the floor exists.
fn audio_ctx_override() -> Option<i32> {
    static OVERRIDE: std::sync::OnceLock<Option<i32>> = std::sync::OnceLock::new();
    *OVERRIDE.get_or_init(|| {
        let raw = std::env::var("DILE_AUDIO_CTX").ok()?;
        let asked: i32 = raw.trim().parse().ok()?;
        // 0 is the documented spelling of "off", and off is the full window.
        Some(if asked <= 0 {
            FULL_AUDIO_CTX
        } else {
            asked.min(FULL_AUDIO_CTX)
        })
    })
}

/// Which device the model runs on.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub enum Compute {
    /// Strict CPU. The default, and the determinism choice.
    #[default]
    Cpu,
    /// Require Vulkan. Needs the `gpu-vulkan` feature at build time.
    Vulkan,
}

impl Compute {
    /// The identifier this choice is stored and sent over the IPC boundary as.
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Compute::Cpu => "cpu",
            Compute::Vulkan => "vulkan",
        }
    }

    /// The runtime backend, or an error when this build cannot satisfy the request.
    fn backend(self) -> Result<Backend, Error> {
        match self {
            Compute::Cpu => Ok(Backend::Cpu),
            #[cfg(feature = "gpu-vulkan")]
            Compute::Vulkan => Ok(Backend::Vulkan),
            #[cfg(not(feature = "gpu-vulkan"))]
            Compute::Vulkan => Err(Error::GpuNotCompiled),
        }
    }
}

/// Anything that can go wrong loading a model or transcribing a buffer.
#[derive(Debug)]
#[non_exhaustive]
pub enum Error {
    /// The runtime said no. Its own message is preserved.
    Runtime(transcribe_cpp::Error),
    /// [`Compute::Vulkan`] was asked for from a build without the `gpu-vulkan` feature.
    ///
    /// A distinct error rather than a fallback to the CPU: a user who turned the GPU on and
    /// silently got the slow path would report a performance bug instead of a build one.
    GpuNotCompiled,
    /// The buffer is empty. The runtime accepts it and returns nothing, which reads to a
    /// user as "the app heard nothing" when the truth is that the microphone gave nothing.
    EmptyAudio,
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Error::Runtime(inner) => write!(f, "{inner}"),
            Error::GpuNotCompiled => {
                write!(
                    f,
                    "this build has no GPU support; rebuild with --features gpu-vulkan"
                )
            }
            Error::EmptyAudio => write!(f, "no audio to transcribe"),
        }
    }
}

impl std::error::Error for Error {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Error::Runtime(inner) => Some(inner),
            _ => None,
        }
    }
}

impl From<transcribe_cpp::Error> for Error {
    fn from(inner: transcribe_cpp::Error) -> Self {
        Error::Runtime(inner)
    }
}

/// One line of the transcript, with the times the runtime gave it.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Segment {
    /// Start of the segment, in milliseconds from the start of the buffer.
    pub start_ms: i64,
    /// End of the segment, in milliseconds from the start of the buffer.
    pub end_ms: i64,
    /// The text of the segment, trimmed.
    pub text: String,
}

/// What one run produced.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Transcript {
    /// The whole transcript, trimmed.
    pub text: String,
    /// The segments, in order.
    pub segments: Vec<Segment>,
    /// The language the model detected, when it reports one.
    ///
    /// Empirically `None` from this runtime even with a hint (`docs/PROJECT.md`, log
    /// 2026-09-09). Carried through rather than dropped so WP3 can decide whether the field
    /// is worth having.
    pub language: Option<String>,
    /// The encoder window this transcript was decoded in, in encoder positions.
    ///
    /// Reported rather than assumed, and carried all the way out to `dile transcribe
    /// --json`, because it is the one setting that can make a run both faster and wrong. A
    /// bug report that says "it hallucinated" is actionable with this number in it and a
    /// guess without it. [`FULL_AUDIO_CTX`] means the window was not shortened.
    pub audio_ctx: i32,
}

/// The backends are registered once per process, whatever loads a model first.
static BACKENDS: Once = Once::new();

/// A loaded model. Loading is the expensive part — seconds for a multi-gigabyte GGUF — so
/// this is held for the lifetime of the engine process and [`Engine`]s are made from it.
#[derive(Debug)]
pub struct Model {
    inner: transcribe_cpp::Model,
}

impl Model {
    /// Load a GGUF model onto `compute`.
    ///
    /// GGUF is the documented format of the runtime and the pinned source of v1 weights
    /// (`docs/PROJECT.md`, log 2026-09-09). Legacy `ggml-*.bin` files happen to load as
    /// well; that is undocumented behaviour and nothing here relies on it.
    pub fn load(path: impl AsRef<Path>, compute: Compute) -> Result<Self, Error> {
        BACKENDS.call_once(|| {
            // A backend that fails to register is not fatal by itself: the CPU path is
            // always present, and asking for a backend that did not come up produces a
            // `Backend` error from the load below, which says which one and why.
            let _ = transcribe_cpp::init_backends_default();
        });

        let options = ModelOptions {
            backend: compute.backend()?,
            device: None,
        };
        let inner = transcribe_cpp::Model::load_with(path, &options)?;
        Ok(Model { inner })
    }

    /// The model family the file turned out to hold, such as `whisper`.
    #[must_use]
    pub fn arch(&self) -> String {
        self.inner.arch()
    }

    /// The backend the runtime actually put the model on.
    ///
    /// Worth reading rather than assuming: a Vulkan request that the driver could not
    /// satisfy is the difference between a 0.03 real-time factor and a 1.0 one.
    #[must_use]
    pub fn backend(&self) -> String {
        self.inner.backend()
    }

    /// Open a session on this model at [`default_threads`] for this machine.
    ///
    /// The CPU tier's answer. A caller that has a device to think about — the engine host is
    /// the only one — decides for itself with [`Model::engine_with_threads`].
    pub fn engine(&self) -> Result<Engine, Error> {
        self.engine_with_threads(default_threads())
    }

    /// Open a session on this model, choosing how many CPU threads it may use.
    ///
    /// `0` leaves the decision to the runtime, and the runtime's decision is **not** "as many
    /// as this machine has": `transcribe-cpp` takes the number of CPUs the process may run on
    /// *capped at eight* (`src/transcribe-batch-util.h`, `default_n_threads(int cap = 8)`), so
    /// above eight hardware threads the rest of the machine sits idle. [`default_threads`]
    /// carries the measurement and is what every caller in this workspace passes on the CPU
    /// tier.
    ///
    /// `0` is still the right thing to pass on the Vulkan tier, where almost nothing runs on
    /// the CPU and where the number above was never measured. The engine host is where those
    /// two sentences meet, because it is the only place that knows which device was asked for.
    pub fn engine_with_threads(&self, threads: u32) -> Result<Engine, Error> {
        let options = SessionOptions {
            // Saturating rather than wrapping: a thread count that arrived as nonsense
            // becomes "as many as this platform can express", never a negative that the
            // native side would read as something else entirely.
            n_threads: i32::try_from(threads).unwrap_or(i32::MAX),
            ..SessionOptions::default()
        };
        Ok(Engine {
            session: self.inner.session_with(&options)?,
        })
    }
}

/// A session that turns buffers into text.
#[derive(Debug)]
pub struct Engine {
    session: transcribe_cpp::Session,
}

impl Engine {
    /// Transcribe one 16 kHz mono buffer of `f32` samples in `language`, with no prompt.
    ///
    /// `language` is an ISO code — `"tr"`, `"en"` — and is always given rather than left to
    /// autodetection: Dile knows which language it is set to, and a hint costs nothing.
    pub fn transcribe(&mut self, pcm: &[f32], language: &str) -> Result<Transcript, Error> {
        self.transcribe_with(pcm, language, None)
    }

    /// Transcribe one buffer with an initial prompt in front of the decoder.
    ///
    /// The prompt is how the user's dictionary reaches the model (`docs/PROJECT.md` §3, v1
    /// model), and an empty one is passed as `None` rather than as an empty string: whisper
    /// conditions on whatever it is given, and an empty prompt is not the same thing as no
    /// prompt.
    pub fn transcribe_with(
        &mut self,
        pcm: &[f32],
        language: &str,
        prompt: Option<&str>,
    ) -> Result<Transcript, Error> {
        if pcm.is_empty() {
            return Err(Error::EmptyAudio);
        }

        let prompt = prompt.filter(|text| !text.trim().is_empty());
        let audio_ctx = audio_ctx_override().unwrap_or_else(|| audio_ctx_for(pcm.len()));
        let result = self
            .session
            .run(pcm, &run_options(language, prompt, audio_ctx))?;
        Ok(Transcript {
            audio_ctx,
            text: result.text.trim().to_string(),
            segments: result
                .segments
                .iter()
                .map(|segment| Segment {
                    start_ms: segment.t0_ms,
                    end_ms: segment.t1_ms,
                    text: segment.text.trim().to_string(),
                })
                .collect(),
            language: result.language.clone(),
        })
    }
}

/// The decoder settings, in one place because M0 compares candidates under identical ones.
///
/// Temperature 0 is the parity baseline of `docs/PROJECT.md` WP1 and beam size is absent
/// because this runtime has none, which leaves the prompt as the only thing a caller
/// decides — and the only thing M0 measured a difference from.
fn run_options(language: &str, prompt: Option<&str>, audio_ctx: i32) -> RunOptions {
    RunOptions {
        language: Some(language.to_string()),
        timestamps: TimestampKind::Segment,
        family: Some(RunExtension::Whisper(WhisperRunOptions {
            initial_prompt: prompt.map(str::to_string),
            temperature: Some(0.0),
            // Always sent, never left to the runtime's default, for the same reason
            // temperature is: the window is now part of what "the M0 baseline" means, and a
            // reader should find it named here rather than have to know that the absence of
            // a field means thirty seconds.
            audio_ctx: Some(audio_ctx),
            ..Default::default()
        })),
        ..Default::default()
    }
}

#[cfg(test)]
mod tests {
    use super::{
        Compute, Error, FULL_AUDIO_CTX, MAX_THREADS, MIN_AUDIO_CTX, SAMPLE_RATE, audio_ctx_for,
        default_threads, run_options, threads_for,
    };
    use std::num::NonZeroUsize;
    use transcribe_cpp::{RunExtension, TimestampKind};

    /// `available` as the policy takes it, for a machine of that size.
    fn machine(threads: usize) -> Option<NonZeroUsize> {
        NonZeroUsize::new(threads)
    }

    #[test]
    fn the_default_is_the_whole_machine_up_to_the_ceiling() {
        // The number that matters: every hardware thread, not the runtime's eight. This is
        // the row the measurement in `default_threads` was taken on.
        assert_eq!(threads_for(machine(20)), 20);
        assert_eq!(threads_for(machine(14)), 14);
        assert_eq!(threads_for(machine(8)), 8);
        // A machine smaller than the runtime's own cap is not talked up to it.
        assert_eq!(threads_for(machine(4)), 4);
        assert_eq!(threads_for(machine(1)), 1);
    }

    #[test]
    fn a_machine_bigger_than_the_ceiling_stops_at_it() {
        // Thirty-two threads on a twenty-thread machine measured *slower* than the capped
        // default, so nothing here may ever ask for more than the machine has — and on a
        // machine that has more than this, the gain had already gone sublinear.
        assert_eq!(threads_for(machine(64)), MAX_THREADS);
        assert_eq!(threads_for(machine(128)), MAX_THREADS);
        assert_eq!(threads_for(machine(usize::MAX)), MAX_THREADS);
    }

    #[test]
    fn a_machine_that_will_not_say_gets_the_runtimes_own_answer() {
        // Not a guess. `0` is the wire and runtime spelling of "you decide", and in the one
        // case where nothing is known that is better than a number made up here.
        assert_eq!(threads_for(None), 0);
    }

    #[test]
    fn this_machine_gets_a_number_it_can_actually_run() {
        let threads = default_threads();
        assert!(
            threads <= MAX_THREADS,
            "the ceiling is not a suggestion: {threads}"
        );
        // A runner that reports its parallelism must never be told to leave it idle, which
        // is the whole bug this policy exists to fix.
        if let Ok(available) = std::thread::available_parallelism() {
            assert_eq!(u64::from(threads), available.get().min(32) as u64);
            assert!(threads >= 1);
        }
    }

    #[test]
    fn cpu_is_the_default_and_the_gpu_is_a_build_time_decision() {
        assert_eq!(Compute::default(), Compute::Cpu);
        assert_eq!(Compute::Cpu.as_str(), "cpu");
        assert!(Compute::Cpu.backend().is_ok());

        // The two builds of this crate differ in exactly one observable way, and this is it.
        let vulkan = Compute::Vulkan.backend();
        if cfg!(feature = "gpu-vulkan") {
            assert!(
                vulkan.is_ok(),
                "the gpu-vulkan build must be able to ask for Vulkan"
            );
        } else {
            assert!(matches!(vulkan, Err(Error::GpuNotCompiled)));
        }
    }

    /// Samples for a take of that many seconds, at the one rate this crate takes.
    fn take(seconds: f32) -> usize {
        (seconds * SAMPLE_RATE as f32) as usize
    }

    #[test]
    fn a_short_take_gets_the_floor_and_not_what_the_arithmetic_says() {
        // 100 * 3 = 300, and a three-second clip is in fact correct at 256. The floor is
        // not about whether the window holds the audio: forced to 512 a five-second take
        // lost a whole sentence on one of three model-and-device combinations and was
        // correct on the other two, and at 192 a three-second clip returned the sentence
        // four times over and took 40 s.
        assert_eq!(audio_ctx_for(take(1.0)), MIN_AUDIO_CTX);
        assert_eq!(audio_ctx_for(take(2.0)), MIN_AUDIO_CTX);
        assert_eq!(audio_ctx_for(take(3.0)), MIN_AUDIO_CTX);
        assert_eq!(audio_ctx_for(take(6.4)), MIN_AUDIO_CTX);
        // An empty buffer is not a special case worth a branch, but it must not be zero:
        // zero is the spelling of "full window", and a floor of 512 is the honest answer.
        assert_eq!(audio_ctx_for(0), MIN_AUDIO_CTX);
    }

    #[test]
    fn a_middling_take_scales_with_the_audio() {
        // Twice what the audio occupies. The measured safe point for a nine-second take
        // of two sentences was 640 and 603 was already broken, so the rule has to sit well
        // clear of the arithmetic rather than just above it.
        assert_eq!(audio_ctx_for(take(9.0)), 900);
        assert!(
            audio_ctx_for(take(9.0)) >= 640,
            "the nine-second safe point is 640"
        );
        assert_eq!(audio_ctx_for(take(10.0)), 1000);
        assert_eq!(audio_ctx_for(take(12.0)), 1200);
        assert_eq!(audio_ctx_for(take(14.0)), 1400);
    }

    #[test]
    fn a_long_take_asks_for_the_whole_window() {
        // Past fifteen seconds the rule saturates, and saturating at the full window is
        // the important half: forced to 512 a nine-second take ran 35 s and ended in a
        // hallucinated subtitle credit. The knob must give up before it does that, and by
        // then it was buying little anyway — 16 % at twenty seconds.
        assert_eq!(audio_ctx_for(take(15.0)), FULL_AUDIO_CTX);
        assert_eq!(audio_ctx_for(take(20.0)), FULL_AUDIO_CTX);
        assert_eq!(audio_ctx_for(take(30.0)), FULL_AUDIO_CTX);
        assert_eq!(audio_ctx_for(take(120.0)), FULL_AUDIO_CTX);
        assert_eq!(audio_ctx_for(usize::MAX), FULL_AUDIO_CTX);
    }

    #[test]
    fn the_rule_never_leaves_the_measured_band() {
        // Every length from a tenth of a second to five minutes, because the one failure
        // mode that matters is silent: a value below the floor does not error, it
        // hallucinates.
        for tenths in 0..3000 {
            let ctx = audio_ctx_for(take(tenths as f32 / 10.0));
            assert!(
                (MIN_AUDIO_CTX..=FULL_AUDIO_CTX).contains(&ctx),
                "{tenths} tenths of a second asked for {ctx}"
            );
        }
    }

    #[test]
    fn the_window_is_never_shorter_than_the_audio_in_it() {
        // 50 encoder positions is one second of audio, and the rule asks for twice that.
        // The window must hold the take with room to spare, or the engine is asked to drop
        // speech rather than padding — and the C++ floor that catches that is a backstop,
        // not the policy.
        for tenths in 1..300 {
            let samples = take(tenths as f32 / 10.0);
            // What the audio occupies, unrounded: 1500 positions spread over 30 seconds.
            let occupies = samples as f64 / f64::from(SAMPLE_RATE) * 50.0;
            assert!(
                f64::from(audio_ctx_for(samples))
                    >= (occupies * 2.0).min(f64::from(FULL_AUDIO_CTX)),
                "{samples} samples occupy {occupies:.1} positions and must get twice that"
            );
        }
    }

    #[test]
    fn the_run_options_carry_the_window_that_was_chosen() {
        // The decode settings and the window are decided in the same place, because a
        // reader asking "what did this run actually do" should find one answer.
        let options = run_options("tr", None, 640);
        let Some(RunExtension::Whisper(whisper)) = options.family else {
            panic!("the whisper extension carries the window");
        };
        assert_eq!(whisper.audio_ctx, Some(640));

        let full = run_options("tr", None, FULL_AUDIO_CTX);
        let Some(RunExtension::Whisper(whisper)) = full.family else {
            panic!("the whisper extension carries the window");
        };
        assert_eq!(whisper.audio_ctx, Some(FULL_AUDIO_CTX));
    }

    #[test]
    fn the_decoder_settings_are_the_m0_parity_baseline() {
        let options = run_options("tr", None, FULL_AUDIO_CTX);
        assert_eq!(options.language.as_deref(), Some("tr"));
        assert_eq!(options.timestamps, TimestampKind::Segment);

        let Some(RunExtension::Whisper(whisper)) = options.family else {
            panic!("the whisper decode knobs must be set explicitly, not left to defaults");
        };
        assert_eq!(whisper.temperature, Some(0.0));
        assert_eq!(whisper.initial_prompt, None);

        assert_eq!(SAMPLE_RATE, 16_000);
    }

    #[test]
    fn the_prompt_is_the_one_decoder_knob_a_caller_sets() {
        let prompted = run_options("tr", Some("cron, SSH, Tauri."), MIN_AUDIO_CTX);
        let Some(RunExtension::Whisper(whisper)) = prompted.family else {
            panic!("the whisper extension carries the prompt");
        };
        assert_eq!(whisper.initial_prompt.as_deref(), Some("cron, SSH, Tauri."));
        // Everything else is still the baseline: a prompt must not quietly move a threshold.
        assert_eq!(whisper.temperature, Some(0.0));
        assert_eq!(whisper.condition_on_prev_tokens, None);
    }
}
