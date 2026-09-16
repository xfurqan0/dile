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
        let result = self.session.run(pcm, &run_options(language, prompt))?;
        Ok(Transcript {
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
fn run_options(language: &str, prompt: Option<&str>) -> RunOptions {
    RunOptions {
        language: Some(language.to_string()),
        timestamps: TimestampKind::Segment,
        family: Some(RunExtension::Whisper(WhisperRunOptions {
            initial_prompt: prompt.map(str::to_string),
            temperature: Some(0.0),
            ..Default::default()
        })),
        ..Default::default()
    }
}

#[cfg(test)]
mod tests {
    use super::{
        Compute, Error, MAX_THREADS, SAMPLE_RATE, default_threads, run_options, threads_for,
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

    #[test]
    fn the_decoder_settings_are_the_m0_parity_baseline() {
        let options = run_options("tr", None);
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
        let prompted = run_options("tr", Some("cron, SSH, Tauri."));
        let Some(RunExtension::Whisper(whisper)) = prompted.family else {
            panic!("the whisper extension carries the prompt");
        };
        assert_eq!(whisper.initial_prompt.as_deref(), Some("cron, SSH, Tauri."));
        // Everything else is still the baseline: a prompt must not quietly move a threshold.
        assert_eq!(whisper.temperature, Some(0.0));
        assert_eq!(whisper.condition_on_prev_tokens, None);
    }
}
