//! Everything that can go wrong between the microphone and a buffer.

use thiserror::Error;

/// What capture failed at.
///
/// Every variant names a thing the user or the maintainer can act on. `dile-app` turns
/// these into locale strings: this crate is a library and holds no user-facing text
/// (`docs/PROJECT.md` §3, UI languages).
#[derive(Debug, Error)]
#[non_exhaustive]
pub enum Error {
    /// The host has no audio input at all.
    #[error("this machine has no audio input device")]
    NoDevice,

    /// A device was named in the settings and the host does not have it any more.
    ///
    /// Distinct from [`Error::NoDevice`] on purpose: a headset that was unplugged is a
    /// different message from a machine with no microphone, and the two want different
    /// buttons in the settings window.
    #[error("no audio input device matches {0:?}")]
    DeviceNotFound(String),

    /// The device exists but offers nothing this build can record from.
    #[error("the input device has no usable configuration")]
    UnsupportedConfig(#[source] cpal::Error),

    /// The device's native sample format is one this crate does not convert.
    ///
    /// The list is deliberately finite rather than a catch-all cast: a silent
    /// misinterpretation of DSD or 24-bit-in-32 samples is noise at full scale, which is
    /// worse to debug than a refusal.
    #[error("sample format {0} is not one this build can read")]
    UnsupportedSampleFormat(cpal::SampleFormat),

    /// The host refused to build or start the stream.
    #[error("the input stream could not be opened")]
    Stream(#[source] cpal::Error),

    /// The device list could not be read.
    #[error("the audio device list could not be read")]
    Devices(#[source] cpal::Error),

    /// The resampler could not be built for this rate pair.
    #[error("no resampler exists from {from} Hz to 16000 Hz")]
    ResamplerBuild {
        /// The device's native rate, which is the half of the pair that varies.
        from: u32,
        /// What rubato said.
        #[source]
        source: rubato::ResamplerConstructionError,
    },

    /// The resampler failed on a buffer.
    #[error("resampling failed")]
    Resample(#[source] rubato::ResampleError),

    /// An internal buffer was handed to the resampler with the wrong geometry.
    ///
    /// This is a bug in this crate rather than a condition the caller can provoke, and it
    /// exists so that the arithmetic around chunk sizes fails loudly instead of through an
    /// `unwrap` (`docs/PROJECT.md` WP2: no `unwrap` outside tests).
    #[error("audio buffer geometry mismatch: {0}")]
    Buffer(String),

    /// The worker thread is gone, so the command had nobody to reach.
    ///
    /// Reachable in exactly one way that is not a bug: the worker panicked. Returning it
    /// rather than panicking in the caller's thread keeps a microphone fault out of the
    /// tray's event loop.
    #[error("the capture worker thread is no longer running")]
    WorkerGone,
}

impl From<rubato::ResampleError> for Error {
    fn from(source: rubato::ResampleError) -> Self {
        Error::Resample(source)
    }
}
