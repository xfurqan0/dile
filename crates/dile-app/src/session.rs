//! Where the hotkey meets the microphone: WP2's two halves, wired together.
//!
//! `dile-hotkey` turns key events into four verbs and `dile-capture` turns a microphone into
//! a buffer. Neither knows the other exists — that was the point of building them apart —
//! and this module is the seam. It is also, deliberately, the only place in the application
//! that knows what a dictation *is*.
//!
//! ```text
//!   HotkeyListener ──Action──▶ session thread ──▶ Capture ──Recording──▶ handoff (WP3)
//!                                    │                │
//!                                    │                └──LevelEvent──▶ level thread
//!                                    ▼                                      │
//!                              tray::Status ───────────────────────▶ dile://state
//!                                                                    dile://level
//! ```
//!
//! **Two threads, and neither of them is the UI thread.** The session thread blocks on the
//! hotkey's action channel and owns the [`Capture`]; the level thread blocks on the capture's
//! level channel and forwards readings to the panel window. Both talk to the tray through
//! [`crate::tray::show`], which Tauri dispatches to the main thread for them.
//!
//! **Where a dictation stops, for now.** [`handoff`] is the seam WP3 replaces: today it
//! writes a line about what was captured, and in debug builds it writes the audio next to it
//! so the maintainer can listen to what the pre-roll caught. WP3 sends the same buffer to the
//! engine process instead, and nothing else here changes.
//!
//! **Silence never reaches an engine.** M0 found every canned hallucination in the robustness
//! set sitting on the same ten seconds of applause (`docs/PROJECT.md`, log 2026-09-09), so a
//! recording whose VAD summary says there was no speech is dropped here and the tray says
//! "nothing heard" — the silence half of the hallucination guard, owned by WP2 as that entry
//! says.

use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::thread;
use std::time::Instant;

use dile_capture::{Capture, LevelReceiver, Recording};
use dile_hotkey::{Action, HotkeyListener};
use serde::Serialize;
use tauri::{AppHandle, Emitter, EventTarget, Manager};

use crate::config::AppConfig;
use crate::i18n::Strings;
use crate::tray::{self, Status};

/// The window the review panel lives in. Declared in `tauri.conf.json`, hidden until WP5.
const PANEL_WINDOW: &str = "panel";

/// One meter reading, on its way to the panel's level bar.
const EVENT_LEVEL: &str = "dile://level";
/// One state change, on its way to the panel.
const EVENT_STATE: &str = "dile://state";

/// The payload of [`EVENT_LEVEL`].
///
/// Decibels rather than an amplitude because that is what a meter is drawn in, and the
/// elapsed/cap pair because `docs/PROJECT.md` §3 puts both on the panel: "elapsed time, the
/// 60 s cap shown".
#[derive(Clone, Debug, Serialize)]
struct LevelPayload {
    /// Root-mean-square level of the window, in dBFS, floored at −100.
    rms_dbfs: f32,
    /// Largest absolute sample of the window, in dBFS, floored at −100.
    peak_dbfs: f32,
    /// Milliseconds since this recording started, or 0 when nothing is being recorded.
    elapsed_ms: u64,
    /// The recording cap in force, in milliseconds.
    cap_ms: u64,
}

/// The payload of [`EVENT_STATE`].
#[derive(Clone, Debug, Serialize)]
struct StatePayload {
    /// One of `tray::Status::event_state`.
    state: &'static str,
}

/// How long a recording has been running, shared between the two threads without a lock.
///
/// The level thread needs the elapsed time and the session thread is the only one that knows
/// when a recording started, which is one `AtomicU64` rather than a mutex: 0 means nothing is
/// being recorded, anything else is the moment it began, in milliseconds since `base`, offset
/// by one so that the very first millisecond is not mistaken for idle.
#[derive(Clone, Debug)]
struct Elapsed {
    base: Instant,
    started_ms: Arc<AtomicU64>,
}

impl Elapsed {
    fn new() -> Self {
        Elapsed {
            base: Instant::now(),
            started_ms: Arc::new(AtomicU64::new(0)),
        }
    }

    fn now_ms(&self) -> u64 {
        u64::try_from(self.base.elapsed().as_millis()).unwrap_or(u64::MAX)
    }

    fn start(&self) {
        self.started_ms
            .store(self.now_ms().saturating_add(1), Ordering::Relaxed);
    }

    fn stop(&self) {
        self.started_ms.store(0, Ordering::Relaxed);
    }

    /// Milliseconds since the recording began, or 0 when there is no recording.
    fn since_start(&self) -> u64 {
        match self.started_ms.load(Ordering::Relaxed) {
            0 => 0,
            started => self.now_ms().saturating_sub(started - 1),
        }
    }
}

/// Everything the session threads need to reach the application with.
#[derive(Clone)]
struct Ui {
    app: AppHandle,
    strings: Arc<Strings>,
    /// Whether the panel window exists. Checked once, because the answer cannot change until
    /// WP5 creates windows at run time — and a `get_webview_window` per level event would be
    /// a hash lookup twenty times a second for an answer that is always the same.
    panel: bool,
}

impl Ui {
    fn new(app: AppHandle, strings: Arc<Strings>) -> Self {
        let panel = app.get_webview_window(PANEL_WINDOW).is_some();
        if !panel {
            // Not fatal, and not even unusual until WP5: the window is declared in
            // `tauri.conf.json`, so this is a configuration change nobody meant to make.
            log::warn!("no {PANEL_WINDOW} window; level and state events go nowhere");
        }
        Ui {
            app,
            strings,
            panel,
        }
    }

    /// Show a state on the tray and tell the panel about it.
    fn show(&self, status: Status) {
        tray::show(&self.app, &self.strings, status);
        self.emit(
            EVENT_STATE,
            StatePayload {
                state: status.event_state(),
            },
        );
    }

    /// Send one event to the panel window, or to nowhere if there is no panel.
    fn emit<P: Serialize + Clone>(&self, event: &str, payload: P) {
        if !self.panel {
            return;
        }
        if let Err(error) =
            self.app
                .emit_to(EventTarget::webview_window(PANEL_WINDOW), event, payload)
        {
            // At twenty a second this could fill a log on its own, so it is a debug line:
            // the panel is not listening yet, and a failure here costs one frame of a meter.
            log::debug!("{event} was not delivered: {error}");
        }
    }
}

/// Install the hotkey and start the session.
///
/// # Errors
///
/// Only the hotkey, and only because the application is useless without it: a global hook
/// that will not install means no key press ever reaches Dile, so the honest thing is to fail
/// to start rather than to sit in the tray doing nothing. A microphone that will not open is
/// **not** an error here — it is reported on the tray and retried at the next press, because
/// a device can be plugged in a minute later.
pub fn start(
    app: &AppHandle,
    strings: Arc<Strings>,
    config: AppConfig,
) -> Result<(), Box<dyn std::error::Error>> {
    let listener = HotkeyListener::spawn(config.hotkey)?;
    log::info!(
        "hotkey hook installed: {:?} mode, second key {}",
        config.hotkey.mode,
        config.hotkey.second_key.is_some()
    );

    let ui = Ui::new(app.clone(), strings);
    thread::Builder::new()
        .name("dile-session".to_owned())
        .spawn(move || run(&ui, &config, listener))?;

    Ok(())
}

/// The session thread: one action at a time, in the order the user produced them.
fn run(ui: &Ui, config: &AppConfig, listener: HotkeyListener) {
    let elapsed = Elapsed::new();
    let mut capture = open_capture(ui, config, &elapsed);

    // `recv` ends when the listener thread stops, which happens when the listener is dropped
    // — and it is owned here, so that is at shutdown.
    while let Ok(emitted) = listener.actions().recv() {
        match emitted.action {
            Action::StartRecording => {
                if capture.is_none() {
                    // A microphone that was missing at start-up may be plugged in now.
                    capture = open_capture(ui, config, &elapsed);
                }
                let Some(open) = capture.as_ref() else {
                    continue;
                };
                match open.start() {
                    Ok(()) => {
                        elapsed.start();
                        ui.show(Status::Recording);
                    }
                    Err(error) => {
                        log::error!("the microphone stopped before a recording began: {error}");
                        capture = None;
                        ui.show(Status::NoMicrophone);
                    }
                }
            }

            Action::StopRecording => {
                elapsed.stop();
                let Some(open) = capture.as_ref() else {
                    continue;
                };
                match open.stop() {
                    // A release with no press behind it — the application started with the
                    // key already down, or the cap took the recording and the buffer has
                    // already been handed over — gives an empty recording rather than an
                    // error, and nothing is owed to anybody.
                    Ok(recording) if recording.is_empty() => {
                        log::debug!("a release arrived with nothing recorded behind it");
                        ui.show(Status::Idle);
                    }
                    Ok(recording) => finish(ui, open.overruns(), &recording),
                    Err(error) => {
                        log::error!("the recording could not be taken: {error}");
                        capture = None;
                        ui.show(Status::NoMicrophone);
                    }
                }
            }

            Action::DiscardRecording => {
                elapsed.stop();
                if let Some(open) = capture.as_ref()
                    && let Err(error) = open.discard()
                {
                    log::error!("the recording could not be discarded: {error}");
                    capture = None;
                }
                ui.show(Status::Idle);
            }

            // TODO(WP5): show the panel, empty, so a tap proves the hotkey works without
            // dictating anything (docs/PROJECT.md §3, Hotkey).
            Action::OpenPanelIdle => {
                log::info!("a tap: the panel would open idle here");
            }
        }
    }

    log::info!("the hotkey listener has stopped; the session is over");
}

/// Open the microphone and start forwarding its level stream.
///
/// Returns `None` when there is no usable input device, having said so on the tray. The
/// application keeps running: `docs/PROJECT.md` §3 puts the microphone behind a setting, and
/// a tray application that exits because a headset is unplugged is one nobody leaves running.
fn open_capture(ui: &Ui, config: &AppConfig, elapsed: &Elapsed) -> Option<Capture> {
    match Capture::open(config.capture.clone()) {
        Ok(capture) => {
            spawn_level_thread(
                ui.clone(),
                capture.levels(),
                elapsed.clone(),
                config.cap_ms(),
            );
            ui.show(Status::Idle);
            Some(capture)
        }
        Err(error) => {
            log::error!("no microphone: {error}");
            ui.show(Status::NoMicrophone);
            None
        }
    }
}

/// Forward the capture's level stream to the panel until the capture goes away.
///
/// A thread of its own rather than a branch in the session loop: the two channels come from
/// different crates, and a meter that only moved between key presses would be no meter at
/// all. It ends by itself when the [`Capture`] is dropped and its sender hangs up.
fn spawn_level_thread(ui: Ui, levels: LevelReceiver, elapsed: Elapsed, cap_ms: u64) {
    let spawned = thread::Builder::new()
        .name("dile-level".to_owned())
        .spawn(move || {
            while let Ok(event) = levels.recv() {
                ui.emit(
                    EVENT_LEVEL,
                    LevelPayload {
                        rms_dbfs: event.rms_dbfs,
                        peak_dbfs: event.peak_dbfs,
                        elapsed_ms: elapsed.since_start(),
                        cap_ms,
                    },
                );
            }
        });

    if let Err(error) = spawned {
        // The meter is dead; recording is not. WP5's panel would show a still bar.
        log::error!("the level stream could not be started: {error}");
    }
}

/// A finished recording: report it, drop it if it is silence, hand it on if it is not.
fn finish(ui: &Ui, overruns: u64, recording: &Recording) {
    if overruns > 0 {
        // The ring filled, so the machine could not keep up. The count is cumulative since
        // the microphone was opened, which is why this says "a recording" rather than "this
        // one" — the crate counts drops, not which buffer lost them.
        log::warn!("{overruns} samples dropped since the microphone opened; a recording has a gap");
    }
    if recording.cap_hit {
        log::info!("the recording cap stopped this one; the key was still down");
    }

    if recording.speech.has_speech {
        ui.show(Status::Working);
        handoff(&ui.app, recording);
        ui.show(Status::Idle);
    } else {
        log::info!(
            "nothing heard in {:.2} s ({} samples); the recording was dropped",
            recording.duration.as_secs_f32(),
            recording.samples.len()
        );
        ui.show(Status::NothingHeard);
    }
}

/// Where a dictation leaves WP2.
///
/// **WP3 replaces the body of this function** with a send to the isolated engine process, and
/// the tray's working state — which is a blink today — becomes the transcription. Everything
/// before it is finished: the buffer is 16 kHz mono `f32` with the pre-roll already in front
/// of it, which is exactly what `dile_engine::Engine::transcribe` takes.
fn handoff(app: &AppHandle, recording: &Recording) {
    log::info!(
        "recording: {:.2} s, {} samples, speech {} ms from {:.2} s to {:.2} s",
        recording.duration.as_secs_f32(),
        recording.samples.len(),
        recording.speech.speech_ms,
        recording
            .speech
            .first_speech_at
            .unwrap_or_default()
            .as_secs_f32(),
        recording
            .speech
            .last_speech_at
            .unwrap_or_default()
            .as_secs_f32(),
    );

    #[cfg(debug_assertions)]
    save_last_recording(app, recording);
    #[cfg(not(debug_assertions))]
    let _ = app;
}

/// Write the recording to `last.wav` in the application's local data directory.
///
/// **Debug builds only, and that is a product rule rather than a convenience.** Dile's claim
/// is that audio is processed in memory and never leaves the machine; a release build that
/// left the last dictation on disk would make that claim false, so the whole function is
/// compiled out. What it buys in a debug build is the hand test WP2's acceptance criterion
/// needs — "an audio buffer with no clipped first syllable" is something a person has to
/// listen to.
#[cfg(debug_assertions)]
fn save_last_recording(app: &AppHandle, recording: &Recording) {
    let directory = match app.path().app_local_data_dir() {
        Ok(directory) => directory,
        Err(error) => {
            log::warn!("no local data directory to write the debug recording to: {error}");
            return;
        }
    };
    if let Err(error) = std::fs::create_dir_all(&directory) {
        log::warn!("the debug recording directory could not be created: {error}");
        return;
    }

    let path = directory.join("last.wav");
    let spec = hound::WavSpec {
        channels: 1,
        sample_rate: dile_capture::SAMPLE_RATE,
        bits_per_sample: 16,
        sample_format: hound::SampleFormat::Int,
    };

    let written = hound::WavWriter::create(&path, spec).and_then(|mut writer| {
        for &sample in &recording.samples {
            // The capture crate's buffers are already in −1.0..=1.0; the clamp is for the
            // one sample a resampler can ring past the edge on.
            let clamped = sample.clamp(-1.0, 1.0) * f32::from(i16::MAX);
            writer.write_sample(clamped as i16)?;
        }
        writer.finalize()
    });

    match written {
        Ok(()) => log::info!(
            "debug build: wrote {} ({:.2} s, speech {})",
            path.display(),
            recording.duration.as_secs_f32(),
            recording.speech.has_speech
        ),
        Err(error) => log::warn!("the debug recording could not be written: {error}"),
    }
}

#[cfg(test)]
mod tests {
    use super::{Elapsed, LevelPayload, StatePayload};
    use crate::tray::Status;
    use std::thread::sleep;
    use std::time::Duration;

    #[test]
    fn elapsed_is_zero_until_a_recording_starts_and_zero_again_after_it_stops() {
        let elapsed = Elapsed::new();
        assert_eq!(elapsed.since_start(), 0);

        elapsed.start();
        sleep(Duration::from_millis(12));
        let during = elapsed.since_start();
        assert!(during >= 10, "{during} ms is not the 12 ms that passed");

        elapsed.stop();
        assert_eq!(elapsed.since_start(), 0);
    }

    #[test]
    fn a_recording_that_starts_in_the_first_millisecond_is_not_mistaken_for_idle() {
        // The +1 offset in `Elapsed::start`: without it, a recording that began at t = 0
        // would store 0, which is the value that means "nothing is being recorded".
        let elapsed = Elapsed::new();
        elapsed.start();
        sleep(Duration::from_millis(2));
        assert!(elapsed.since_start() > 0);
    }

    #[test]
    fn the_payloads_are_the_shape_wp5_will_read() {
        let level = serde_json::to_value(LevelPayload {
            rms_dbfs: -42.5,
            peak_dbfs: -30.0,
            elapsed_ms: 1_200,
            cap_ms: 60_000,
        })
        .expect("a level payload serializes");
        assert_eq!(level["rms_dbfs"], -42.5);
        assert_eq!(level["peak_dbfs"], -30.0);
        assert_eq!(level["elapsed_ms"], 1_200);
        assert_eq!(level["cap_ms"], 60_000);

        let state = serde_json::to_value(StatePayload {
            state: Status::Recording.event_state(),
        })
        .expect("a state payload serializes");
        assert_eq!(state["state"], "recording");
    }
}
