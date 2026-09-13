//! The one test that proves the engine really transcribes, run by hand.
//!
//! `#[ignore]`d on purpose, and it is the right kind of ignored: it needs a multi-gigabyte
//! GGUF model and a recording, neither of which lives in this repository (`.gitignore`,
//! `docs/PROJECT.md` §4) or on a CI runner. Everything the automated suite can check
//! without them is checked in `src/lib.rs`.
//!
//! Both paths come from the environment, so no machine's directory layout is written down
//! here and the test skips with a message naming the variable when one is missing:
//!
//! ```powershell
//! $env:DILE_TEST_MODEL = "<your model directory>\whisper-large-v3-turbo-F16.gguf"
//! $env:DILE_TEST_WAV   = "<your recording directory>\take-02.wav"
//! cargo test -p dile-engine --test smoke -- --ignored --nocapture
//! ```
//!
//! Add `--features gpu-vulkan` and set `LIB=%VULKAN_SDK%\Lib;%LIB%` first (docs/BUILDING.md)
//! to run the same check on the GPU.

use std::path::PathBuf;

use dile_engine::{Compute, Model, SAMPLE_RATE};

/// Read a 16 kHz mono 16-bit WAV into the `f32` buffer the engine takes.
///
/// The capture stage (WP2) produces this shape directly from `cpal`; here it comes from a
/// file, which is the only reason this crate has `hound` as a dev-dependency.
fn read_wav(path: &PathBuf) -> Vec<f32> {
    let mut reader = hound::WavReader::open(path).expect("open the test wav");
    let spec = reader.spec();
    assert_eq!(spec.channels, 1, "the engine takes mono");
    assert_eq!(spec.sample_rate, SAMPLE_RATE, "the engine takes 16 kHz");
    assert_eq!(spec.bits_per_sample, 16, "the fixture is 16-bit PCM");
    reader
        .samples::<i16>()
        .map(|sample| f32::from(sample.expect("read a sample")) / 32768.0)
        .collect()
}

/// Read one path from the environment, or say which variable is missing and give up.
///
/// A missing variable is not a failure: this test is `#[ignore]`d and is asked for by hand,
/// so the useful answer is the name of the variable to set rather than a panic that reads
/// like a broken engine.
fn from_env(key: &str) -> Option<PathBuf> {
    match std::env::var(key) {
        Ok(value) => Some(PathBuf::from(value)),
        Err(_) => {
            println!("skipped: {key} is not set; see the module comment for what it points at");
            None
        }
    }
}

#[test]
#[ignore = "needs a local GGUF model and a recording; see the module comment"]
fn transcribes_turkish_on_the_cpu() {
    // Both are read before either is checked, so a run with neither set names both.
    let model = from_env("DILE_TEST_MODEL");
    let wav = from_env("DILE_TEST_WAV");
    let (Some(model_path), Some(wav_path)) = (model, wav) else {
        return;
    };

    let pcm = read_wav(&wav_path);
    let seconds = pcm.len() as f64 / f64::from(SAMPLE_RATE);

    let started = std::time::Instant::now();
    let model = Model::load(&model_path, Compute::Cpu).expect("load the model");
    let load = started.elapsed().as_secs_f64();
    let mut engine = model.engine().expect("open a session");

    let started = std::time::Instant::now();
    let transcript = engine.transcribe(&pcm, "tr").expect("transcribe");
    let wall = started.elapsed().as_secs_f64();

    println!("arch: {} backend: {}", model.arch(), model.backend());
    println!(
        "audio {seconds:.1} s | load {load:.2} s | run {wall:.2} s | rtf {:.3}",
        wall / seconds
    );
    println!("segments: {}", transcript.segments.len());
    println!("---TEXT---\n{}\n---END---", transcript.text);

    assert!(
        !transcript.text.is_empty(),
        "a recording of speech must produce text"
    );
    assert!(
        !transcript.segments.is_empty(),
        "segment timestamps must be populated"
    );
    assert!(
        transcript.segments.iter().all(|s| s.end_ms >= s.start_ms),
        "a segment cannot end before it starts"
    );
}
