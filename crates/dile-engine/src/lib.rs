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
//! * **Greedy decoding, temperature 0, no prompt.** `transcribe-cpp` 0.2.3 exposes **no
//!   beam-size knob** and the whisper family samples greedily; verified in its sources and
//!   by running it (`docs/PROJECT.md`, log 2026-09-09). So the only decoder knobs that
//!   exist are temperature and the prompt, and both are pinned: comparing candidates in M0
//!   means comparing them under identical settings, and a dictation that changes its mind
//!   between two runs of the same audio is a bug report nobody can act on.
//! * **Segment timestamps only.** Word granularity returns *unsupported timestamp
//!   granularity* from this runtime. Dictation does not need word times.
//! * **16 kHz mono `f32`.** The buffer is what the capture stage (WP2) produces; this crate
//!   opens no microphone and reads no file.
//!
//! ## What is not here yet
//!
//! Process isolation (WP3) is the reason this crate is separate from the app: the engine
//! runs in its own process so that a driver crash takes the engine down and not the tray.
//! WP0 links it into the workspace and proves it loads a model and transcribes; nothing in
//! `dile-app` calls it yet.

#![forbid(unsafe_code)]

use std::fmt;
use std::path::Path;
use std::sync::Once;

use transcribe_cpp::{
    Backend, ModelOptions, RunExtension, RunOptions, TimestampKind, WhisperRunOptions,
};

/// The sample rate every buffer this crate accepts must be at.
pub const SAMPLE_RATE: u32 = 16_000;

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

    /// Open a session on this model.
    pub fn engine(&self) -> Result<Engine, Error> {
        Ok(Engine {
            session: self.inner.session()?,
        })
    }
}

/// A session that turns buffers into text.
#[derive(Debug)]
pub struct Engine {
    session: transcribe_cpp::Session,
}

impl Engine {
    /// Transcribe one 16 kHz mono buffer of `f32` samples in `language`.
    ///
    /// `language` is an ISO code — `"tr"`, `"en"` — and is always given rather than left to
    /// autodetection: Dile knows which language it is set to, and a hint costs nothing.
    pub fn transcribe(&mut self, pcm: &[f32], language: &str) -> Result<Transcript, Error> {
        if pcm.is_empty() {
            return Err(Error::EmptyAudio);
        }

        let result = self.session.run(pcm, &run_options(language))?;
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
/// Temperature 0 with no initial prompt is the parity baseline of `docs/PROJECT.md` WP1.
/// Beam size is absent because this runtime has none. The dictionary prompt (WP4) becomes a
/// parameter here, measured with and without.
fn run_options(language: &str) -> RunOptions {
    RunOptions {
        language: Some(language.to_string()),
        timestamps: TimestampKind::Segment,
        family: Some(RunExtension::Whisper(WhisperRunOptions {
            initial_prompt: None,
            temperature: Some(0.0),
            ..Default::default()
        })),
        ..Default::default()
    }
}

#[cfg(test)]
mod tests {
    use super::{Compute, Error, SAMPLE_RATE, run_options};
    use transcribe_cpp::{RunExtension, TimestampKind};

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
        let options = run_options("tr");
        assert_eq!(options.language.as_deref(), Some("tr"));
        assert_eq!(options.timestamps, TimestampKind::Segment);

        let Some(RunExtension::Whisper(whisper)) = options.family else {
            panic!("the whisper decode knobs must be set explicitly, not left to defaults");
        };
        assert_eq!(whisper.temperature, Some(0.0));
        assert_eq!(
            whisper.initial_prompt, None,
            "WP4 adds the dictionary prompt, not WP0"
        );

        assert_eq!(SAMPLE_RATE, 16_000);
    }
}
