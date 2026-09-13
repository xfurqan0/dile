//! A WAV file to the buffer the engine takes.
//!
//! The engine takes 16 kHz mono `f32` and nothing else, and a file on somebody's disk is
//! whatever their recorder wrote. So this is the same two steps the microphone path takes —
//! downmix, then resample — through the **same** [`dile_capture::Converter`], rather than a
//! second resampler with its own opinion about filter length.
//!
//! **Four sample formats, one conversion.** `hound` reads 8, 16, 24 and 32-bit integer PCM
//! and 32-bit float; each is scaled to `-1.0..=1.0` by its own full scale, because a 24-bit
//! file divided by `i32::MAX` is a file that comes back 256 times too quiet and transcribes
//! as silence.
//!
//! **Only WAV.** No MP3, no M4A, no ffmpeg: a decoder for the rest is a dependency tree and
//! a licence question for a command that exists so a recording can be checked against the
//! product's own pipeline. `docs/PROJECT.md` §2 puts file and meeting transcription out of
//! scope, and this command is not a way back in.

use std::path::Path;

use dile_capture::{Converter, SAMPLE_RATE, SourceSpec};
use hound::{SampleFormat, WavReader};

/// What a file can fail to be.
#[derive(Debug, thiserror::Error)]
pub enum AudioError {
    /// The file would not open, or is not a WAV.
    #[error("{0} is not a readable WAV file: {1}")]
    Read(String, hound::Error),
    /// A sample would not decode.
    #[error("the audio in {0} stops part way through: {1}")]
    Decode(String, hound::Error),
    /// The header says something this reader will not guess at.
    #[error("{0} says {1}, which this reader does not convert")]
    Unsupported(String, String),
    /// The file holds no audio at all.
    #[error("{0} holds no audio")]
    Empty(String),
    /// The resampler refused the file's rate.
    #[error("{0} could not be resampled to 16 kHz: {1}")]
    Resample(String, dile_capture::Error),
}

/// One decoded file: 16 kHz mono, and how long it was.
#[derive(Debug)]
pub struct Decoded {
    /// The samples, at [`dile_capture::SAMPLE_RATE`].
    pub samples: Vec<f32>,
    /// The rate the file was recorded at, for the line that says what was done to it.
    pub source_rate: u32,
    /// The channel count the file was recorded with.
    pub source_channels: u16,
}

impl Decoded {
    /// The duration in seconds, after conversion.
    #[must_use]
    pub fn seconds(&self) -> f32 {
        self.samples.len() as f32 / SAMPLE_RATE as f32
    }
}

/// Read `path` and hand back 16 kHz mono samples.
///
/// # Errors
///
/// [`AudioError`], one variant per way a file can fail to be audio this command can use.
pub fn read_wav(path: &Path) -> Result<Decoded, AudioError> {
    let name = path.display().to_string();
    let reader = WavReader::open(path).map_err(|error| AudioError::Read(name.clone(), error))?;
    let spec = reader.spec();

    if spec.channels == 0 {
        return Err(AudioError::Unsupported(name, "zero channels".to_string()));
    }
    if spec.sample_rate == 0 {
        return Err(AudioError::Unsupported(
            name,
            "a sample rate of 0".to_string(),
        ));
    }

    let interleaved = decode(reader, &name)?;
    if interleaved.is_empty() {
        return Err(AudioError::Empty(name));
    }

    let mut converter = Converter::new(SourceSpec::new(spec.sample_rate, spec.channels))
        .map_err(|error| AudioError::Resample(name.clone(), error))?;

    let mut samples = Vec::with_capacity(interleaved.len() / usize::from(spec.channels));
    converter
        .process(&interleaved, &mut samples)
        .map_err(|error| AudioError::Resample(name.clone(), error))?;

    // The converter holds back whatever does not fill a resampler chunk, which is right for
    // a microphone that is still running and wrong for a file that has ended. One chunk of
    // silence at the native rate pushes the tail out — under six milliseconds at 48 kHz, and
    // silence is what follows the end of a recording anyway.
    if spec.sample_rate != SAMPLE_RATE {
        let flush = vec![0.0_f32; 512 * usize::from(spec.channels)];
        converter
            .process(&flush, &mut samples)
            .map_err(|error| AudioError::Resample(name.clone(), error))?;
    }

    if samples.is_empty() {
        return Err(AudioError::Empty(name));
    }

    Ok(Decoded {
        samples,
        source_rate: spec.sample_rate,
        source_channels: spec.channels,
    })
}

/// Every sample in the file, interleaved, scaled to `-1.0..=1.0`.
fn decode(
    reader: WavReader<std::io::BufReader<std::fs::File>>,
    name: &str,
) -> Result<Vec<f32>, AudioError> {
    let spec = reader.spec();
    let mut reader = reader;
    match (spec.sample_format, spec.bits_per_sample) {
        (SampleFormat::Float, 32) => reader
            .samples::<f32>()
            .collect::<Result<Vec<_>, _>>()
            .map_err(|error| AudioError::Decode(name.to_string(), error)),
        // Full scale is `2^(bits-1)`, taken from the declared bit depth rather than from the
        // container width: `hound` hands 24-bit audio back in an `i32`, and dividing that by
        // `i32::MAX` would make the file 256 times too quiet.
        (SampleFormat::Int, bits @ 1..=32) => {
            let scale = f32::from(i16::MAX).max(1.0) * 2.0_f32.powi(i32::from(bits) - 16);
            reader
                .samples::<i32>()
                .map(|sample| {
                    sample
                        .map(|value| (value as f32 / scale).clamp(-1.0, 1.0))
                        .map_err(|error| AudioError::Decode(name.to_string(), error))
                })
                .collect()
        }
        (format, bits) => Err(AudioError::Unsupported(
            name.to_string(),
            format!("{bits}-bit {format:?} samples"),
        )),
    }
}

#[cfg(test)]
mod tests {
    use super::read_wav;
    use dile_capture::SAMPLE_RATE;
    use std::path::PathBuf;

    /// The probe clip, which is the one WAV this repository contains.
    fn probe_clip() -> PathBuf {
        PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("../dile-app/assets/probe.wav")
            .canonicalize()
            .expect("the probe clip is committed")
    }

    #[test]
    fn the_probe_clip_decodes_to_the_rate_the_engine_takes() {
        let decoded = read_wav(&probe_clip()).expect("the probe clip reads");
        assert_eq!(
            decoded.source_rate, SAMPLE_RATE,
            "the clip is already 16 kHz"
        );
        assert_eq!(decoded.source_channels, 1);
        // Two seconds of Turkish, give or take the trim.
        assert!(
            decoded.seconds() > 1.0 && decoded.seconds() < 4.0,
            "the clip came out {} s long",
            decoded.seconds()
        );
        assert!(
            decoded.samples.iter().any(|sample| sample.abs() > 0.01),
            "the clip decoded to silence, which means the scaling is wrong"
        );
    }

    #[test]
    fn a_file_that_is_not_a_wav_is_named_rather_than_panicked_over() {
        let manifest = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("Cargo.toml");
        let error = read_wav(&manifest).expect_err("a manifest is not audio");
        let text = error.to_string();
        assert!(
            text.contains("Cargo.toml"),
            "the error must name the file: {text}"
        );
    }
}
