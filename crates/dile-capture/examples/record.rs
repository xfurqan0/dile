//! Record from a real microphone and write `out.wav`.
//!
//! The hardware check the automated tests cannot do: CI has no audio device, so everything
//! in the test suite runs on a synthetic source. This is what proves the other end — that
//! the device opens, that the native format converts, and that what comes out sounds like
//! what went in.
//!
//! ```text
//! cargo run -p dile-capture --example record -- [seconds] [cap_secs]
//! ```
//!
//! `seconds` defaults to 3 and `cap_secs` to 60. Pass a cap shorter than the recording to
//! watch it stop itself: `--example record -- 5 2` records two seconds and reports the cap.
//!
//! The stream is armed for a second before recording starts, so the pre-roll ring is full
//! and the first second of `out.wav` is audio from *before* the recording began. That is the
//! first-word protection, and hearing it is the point.

use std::time::{Duration, Instant};

use dile_capture::{Capture, CaptureConfig, LevelReceiver, SAMPLE_RATE};

/// How wide the level bar is drawn, in characters.
const BAR_WIDTH: usize = 40;

/// The quietest level the bar shows. Below this it is empty.
const BAR_FLOOR_DBFS: f32 = -60.0;

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let mut args = std::env::args().skip(1);
    let seconds: u64 = match args.next() {
        Some(value) => value.parse()?,
        None => 3,
    };
    let cap_secs: u32 = match args.next() {
        Some(value) => value.parse()?,
        None => 60,
    };

    println!("Input devices:");
    for device in Capture::devices()? {
        let marker = if device.is_default { "*" } else { " " };
        println!("  {marker} {}", device.name);
    }

    let config = CaptureConfig {
        device: None,
        pre_roll_ms: 1_000,
        cap_secs,
        level_interval_ms: 50,
    };
    let capture = Capture::open(config)?;
    let spec = capture.source_spec();
    println!(
        "\nOpened at {} Hz, {} channel(s); converting to {} Hz mono.",
        spec.sample_rate, spec.channels, SAMPLE_RATE
    );
    println!("Cap {} s, pre-roll {} ms.\n", cap_secs, 1_000);

    let levels = capture.levels();

    println!("Arming for 1 s — the pre-roll ring is filling. Speak whenever you like.");
    meter(&levels, Duration::from_secs(1), "arm");

    capture.start()?;
    println!("\nRecording {seconds} s.");
    meter(&levels, Duration::from_secs(seconds), "rec");

    let recording = capture.stop()?;
    println!("\n");

    let path = "out.wav";
    write_wav(path, &recording.samples)?;

    println!("Recording");
    println!("  samples     {}", recording.samples.len());
    println!("  duration    {:.3} s", recording.duration.as_secs_f32());
    println!(
        "  cap hit     {}",
        if recording.cap_hit { "yes" } else { "no" }
    );
    println!("  overruns    {}", capture.overruns());
    println!("  wrote       {path} (16 kHz mono, 16-bit PCM)");

    let speech = recording.speech;
    println!("Speech summary");
    println!(
        "  has speech  {}",
        if speech.has_speech { "yes" } else { "no" }
    );
    println!("  speech      {} ms", speech.speech_ms);
    println!("  first at    {}", optional_ms(speech.first_speech_at));
    println!("  last at     {}", optional_ms(speech.last_speech_at));
    if !speech.has_speech {
        println!("\n  Nothing heard. This is the recording the engine would be skipped for.");
    }

    Ok(())
}

/// Draw one bar per level event until `duration` has passed.
fn meter(levels: &LevelReceiver, duration: Duration, label: &str) {
    let deadline = Instant::now() + duration;
    while Instant::now() < deadline {
        let remaining = deadline.saturating_duration_since(Instant::now());
        let Ok(event) = levels.recv_timeout(remaining.min(Duration::from_millis(200))) else {
            continue;
        };
        let filled = bar_length(event.rms_dbfs);
        let peak = bar_length(event.peak_dbfs);
        let mut bar = String::with_capacity(BAR_WIDTH);
        for position in 0..BAR_WIDTH {
            if position < filled {
                bar.push('#');
            } else if position + 1 == peak {
                bar.push('|');
            } else {
                bar.push('.');
            }
        }
        print!(
            "\r  {label} {:>6.1} s  [{bar}]  {:>6.1} dBFS rms  {:>6.1} peak",
            event.t.as_secs_f32(),
            event.rms_dbfs,
            event.peak_dbfs
        );
        let _ = std::io::Write::flush(&mut std::io::stdout());
    }
}

fn bar_length(dbfs: f32) -> usize {
    if dbfs <= BAR_FLOOR_DBFS {
        return 0;
    }
    let fraction = (dbfs - BAR_FLOOR_DBFS) / -BAR_FLOOR_DBFS;
    ((fraction * BAR_WIDTH as f32).round() as usize).min(BAR_WIDTH)
}

fn optional_ms(value: Option<Duration>) -> String {
    match value {
        Some(duration) => format!("{} ms", duration.as_millis()),
        None => "-".to_string(),
    }
}

/// 16 kHz mono, 16-bit PCM: the format every player on the machine opens.
fn write_wav(path: &str, samples: &[f32]) -> Result<(), Box<dyn std::error::Error>> {
    let spec = hound::WavSpec {
        channels: 1,
        sample_rate: SAMPLE_RATE,
        bits_per_sample: 16,
        sample_format: hound::SampleFormat::Int,
    };
    let mut writer = hound::WavWriter::create(path, spec)?;
    for &sample in samples {
        let clamped = sample.clamp(-1.0, 1.0);
        writer.write_sample((clamped * i16::MAX as f32) as i16)?;
    }
    writer.finalize()?;
    Ok(())
}
