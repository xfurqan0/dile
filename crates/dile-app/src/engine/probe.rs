//! The two seconds of Turkish that decide which tier a machine runs on.
//!
//! `docs/PROJECT.md` §3 makes Vulkan the default tier and then refuses to assume it: "the
//! tier is never assumed: on first start the app runs a **probe transcription inside the
//! isolated engine process**, and only a device that returns a correct result is used."
//! Handy's open issue #1755 is a Vulkan auto-GPU path bug-checking an RTX 5090, and the
//! lesson taken from it is that a driver is not something to ask politely — it is something
//! to make transcribe a sentence whose answer is already known, in a process that can be
//! allowed to die.
//!
//! **What "correct" means here is deliberately loose.** The test is not whether the engine
//! wrote the sentence: it is whether the device produced Turkish words that belong to it.
//! Half of [`WORDS`] is the bar, because a working GPU that mishears one word in a
//! synthesised clip is a working GPU, and the failure this is looking for — a driver that
//! returns silence, garbage, or nothing at all before the timeout — is nowhere near it.
//!
//! **Where the clip came from.** `assets/probe.wav` is Windows' own Turkish text-to-speech
//! voice reading [`SENTENCE`], trimmed to its speech and rewritten as 16 kHz mono — the
//! shape `dile-capture` produces. `docs/BUILDING.md` carries the command that regenerates
//! it. It is synthesised rather than recorded on purpose: nobody's voice ships in this
//! repository, and a probe clip has to be the same two seconds on every machine.

use std::io::Cursor;

use dile_core::normalizer;

/// The probe clip: 2.04 s, 16 kHz mono, 16-bit.
pub const CLIP: &[u8] = include_bytes!("../../assets/probe.wav");

/// What the clip says.
pub const SENTENCE: &str = "Bugün hava çok güzel ve deniz sakin.";

/// The words a device has to produce enough of. Lower-case under Turkish casing rules,
/// which is the form [`matched`] compares in.
pub const WORDS: [&str; 7] = ["bugün", "hava", "çok", "güzel", "ve", "deniz", "sakin"];

/// The reasons a machine does not get the Vulkan tier.
///
/// Three of them never involve a GPU at all, which is the point of naming them: "the probe
/// failed" is a bad line to find in a bug report when what actually happened is that the
/// engine binary had no GPU support compiled into it.
#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum ProbeError {
    /// The embedded clip did not decode. Only reachable if the committed file is replaced
    /// with something that is not a WAV.
    #[error("the probe clip did not decode: {0}")]
    Clip(String),
    /// The embedded clip decoded and is not the shape the capture stage produces.
    #[error("the probe clip is {rate} Hz with {channels} channel(s), not 16 kHz mono")]
    ClipFormat {
        /// Its sample rate.
        rate: u32,
        /// Its channel count.
        channels: u16,
    },
    /// The engine host was built without the `gpu-vulkan` feature, so there is no GPU path
    /// to probe. A CPU-only developer build, or a packaging mistake.
    #[error("the engine host was built without GPU support")]
    NoGpuSupport,
    /// `DILE_FORCE_PROBE_FAIL` was set. Debug builds only, and only so that the fallback
    /// path can be exercised on a machine whose GPU works.
    #[error("the probe was forced to fail by DILE_FORCE_PROBE_FAIL")]
    Forced,
    /// The device answered, and what it wrote was not the sentence.
    #[error("the device returned {matched} of {expected} expected words")]
    WrongWords {
        /// How many of [`WORDS`] came back.
        matched: usize,
        /// How many there are.
        expected: usize,
    },
}

/// The clip as 16 kHz mono `f32`, which is what the engine takes.
///
/// # Errors
///
/// [`ProbeError::Clip`] when the embedded file is not the WAV it is supposed to be.
pub fn samples() -> Result<Vec<f32>, ProbeError> {
    let reader = hound::WavReader::new(Cursor::new(CLIP))
        .map_err(|error| ProbeError::Clip(error.to_string()))?;
    let spec = reader.spec();
    if spec.channels != 1 || spec.sample_rate != dile_capture::SAMPLE_RATE {
        return Err(ProbeError::ClipFormat {
            rate: spec.sample_rate,
            channels: spec.channels,
        });
    }

    let mut samples = Vec::with_capacity(reader.len() as usize);
    for sample in reader.into_samples::<i16>() {
        let sample = sample.map_err(|error| ProbeError::Clip(error.to_string()))?;
        // The same scale `session.rs` writes `last.wav` back out at, in reverse.
        samples.push(f32::from(sample) / f32::from(i16::MAX));
    }
    Ok(samples)
}

/// How many of [`WORDS`] a transcript contains.
///
/// Whole words, compared after Turkish lower-casing and after everything that is not a
/// letter or a digit has become a space — so `Bugün,` matches `bugün` and `bugünkü` does
/// not. Diacritics are **not** folded: a device that writes `guzel` for `güzel` has lost the
/// thing this product exists to get right, and it should not pass by accident.
#[must_use]
pub fn matched(text: &str) -> usize {
    let lowered = normalizer::to_lower(text);
    let spoken: Vec<&str> = lowered
        .split(|c: char| !c.is_alphanumeric())
        .filter(|word| !word.is_empty())
        .collect();
    WORDS.iter().filter(|word| spoken.contains(word)).count()
}

/// Whether a transcript is good enough to trust the device that produced it.
#[must_use]
pub fn passed(text: &str) -> bool {
    matched(text) * 2 >= WORDS.len()
}

#[cfg(test)]
mod tests {
    use super::{CLIP, SENTENCE, WORDS, matched, passed, samples};
    use dile_core::normalizer;

    #[test]
    fn the_committed_clip_is_the_two_seconds_of_turkish_it_claims_to_be() {
        let samples = samples().expect("the committed probe clip decodes");
        let seconds = samples.len() as f32 / dile_capture::SAMPLE_RATE as f32;
        assert!(
            (1.5..=3.5).contains(&seconds),
            "the probe clip is {seconds:.2} s; the design asks for about two"
        );
        assert!(
            CLIP.len() <= 100 * 1024,
            "the clip is committed to this repository and must stay small: {} bytes",
            CLIP.len()
        );
        assert!(
            samples.iter().any(|sample| sample.abs() > 0.05),
            "the clip is silent, so nothing could ever pass the probe"
        );
    }

    #[test]
    fn the_expected_words_are_the_sentence_and_are_already_lower_case() {
        let lowered = normalizer::to_lower(SENTENCE);
        for word in WORDS {
            assert_eq!(
                word,
                normalizer::to_lower(word),
                "{word} is not in the form `matched` compares in"
            );
            assert!(
                lowered.contains(word),
                "{word} is not a word of the sentence the clip says"
            );
        }
    }

    #[test]
    fn the_sentence_itself_passes_and_a_device_that_returns_nothing_does_not() {
        assert_eq!(matched(SENTENCE), WORDS.len());
        assert!(passed(SENTENCE));

        // Punctuation, casing and the engine's habit of ending a sentence with a full stop
        // are all handled by the comparison rather than by hoping.
        assert!(passed("BUGÜN HAVA ÇOK GÜZEL, VE DENİZ SAKİN!"));

        // What a broken device actually returns.
        assert!(!passed(""));
        assert!(!passed("   "));
        assert!(!passed("[BLANK_AUDIO]"));
        assert!(!passed("Thank you for watching."));
    }

    #[test]
    fn half_the_sentence_is_enough_and_a_third_of_it_is_not() {
        // Four of seven: a working device that misheard the odd word.
        assert!(passed("bugün hava çok güzel"));
        // Two of seven: not a device anybody should dictate through.
        assert!(!passed("hava deniz"));
    }

    #[test]
    fn a_mangled_diacritic_does_not_pass_for_the_word_it_mangled() {
        // Folding `ü` to `u` here would let a device that cannot write Turkish look fine,
        // and Turkish is the product.
        assert_eq!(matched("bugun hava cok guzel ve deniz sakin"), 4);
        assert_eq!(matched("bugünkü havalar güzeldi"), 0);
    }
}
