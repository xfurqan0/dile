//! Where the hotkey meets the microphone, and where a recording leaves for the engine.
//!
//! `dile-hotkey` turns key events into four verbs, `dile-capture` turns a microphone into a
//! buffer, and `crate::engine` turns a buffer into text. None of the three knows the others
//! exist — that was the point of building them apart — and this module is the seam. It is
//! also, deliberately, the only place in the application that knows what a dictation *is*.
//!
//! ```text
//!   HotkeyListener ──Action──▶ session thread ──▶ Capture ──Recording──▶ EngineClient
//!                                    │                │                       │
//!                                    │                └──LevelEvent──▶ level thread
//!                                    ▼                                      │
//!                              tray::Status ───────────────────────▶ dile://state
//!                                                                    dile://level
//! ```
//!
//! **Two threads, and neither of them is the UI thread.** The session thread blocks on the
//! hotkey's action channel and owns the [`Capture`]; the level thread blocks on the capture's
//! level channel and forwards readings to the panel window. Both talk to the application
//! through [`Ui`], which dispatches to the main thread for them.
//!
//! **The handoff does not wait.** [`handoff`] puts the buffer in the engine's queue and
//! returns, so the next key press is heard even while the last sentence is still being
//! transcribed. What happens to it after that — the tray's working state, the cleanup, the
//! text — belongs to `crate::engine`, which owns the one thread that talks to the engine
//! process.
//!
//! **A setting applies without a restart, and on this thread.** The hotkey listener owns a
//! global keyboard hook and the capture owns a device; both have to be replaced by whoever
//! owns them, so [`SettingsStore`] raises a flag and the loop below acts on it between key
//! presses. That is also why the loop waits with a timeout instead of blocking for ever on
//! the action channel: a change made while nobody is dictating has to arrive anyway.
//!
//! **The panel is opened here and closed almost everywhere else.** The session is the only
//! thing that knows when a dictation begins, so it is the only thing that can capture the
//! window the dictation is aimed at — `docs/PROJECT.md` §3 wants the target taken at the
//! moment the hotkey went down, not at the moment the text comes back, because by then the
//! user may well be looking at something else. Everything after that — the result, the
//! countdown, the paste — belongs to `crate::panel`, and the three keys the panel claims come
//! back through this same action channel as [`Action::PanelTransfer`] and its two siblings.
//!
//! **Silence never reaches an engine.** M0 found every canned hallucination in the robustness
//! set sitting on the same ten seconds of applause (`docs/PROJECT.md`, log 2026-09-09), so a
//! recording whose VAD summary says there was no speech is dropped here and the tray says
//! "nothing heard" — the silence half of the hallucination guard, owned by WP2 as that entry
//! says.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::mpsc::RecvTimeoutError;
use std::thread;
use std::time::{Duration, Instant};

use dile_capture::{Capture, LevelReceiver, Recording};
use dile_hotkey::{Action, HotkeyListener};
use serde::Serialize;
use tauri::AppHandle;
use tauri_plugin_dialog::{DialogExt, MessageDialogButtons, MessageDialogKind};
// `Manager` is only what `AppHandle::path` needs, and that call lives in
// `save_last_recording`, which is a debug build's alone.
#[cfg(debug_assertions)]
use tauri::Manager;

use crate::engine::EngineClient;
use crate::panel::Panel;
use crate::settings::{Settings, SettingsStore};
use crate::tray::Status;
use crate::ui::{EVENT_LEVEL, Ui};

/// How long the session waits for a key press before looking at the settings again.
///
/// Only the latency of a settings change that arrives while nobody is dictating, so there is
/// no reason to spin faster. Every dictation is still driven by an event, not by this.
const SETTINGS_POLL: Duration = Duration::from_millis(200);

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

/// Install the hotkey and start the session.
///
/// # Errors
///
/// Only the hotkey, and only because the application is useless without it: a global hook
/// that will not install means no key press ever reaches Dile, so the honest thing is to fail
/// to start rather than to sit in the tray doing nothing. A microphone that will not open is
/// **not** an error here — it is reported on the tray and retried at the next press, because
/// a device can be plugged in a minute later.
///
/// **One of those failures gets a window instead of an `Err`.** On Linux the trigger needs a
/// permission the distribution does not grant, and a tray application that exits with a line
/// on a stderr nobody is reading has told the user nothing. That case returns `Ok`, puts
/// [`no_keyboard_access`] on screen, and the application ends when the window is dismissed —
/// which is still failing to start, with the person told why first.
pub fn start(
    ui: Ui,
    store: SettingsStore,
    engine: EngineClient,
    panel: Arc<Panel>,
) -> Result<(), Box<dyn std::error::Error>> {
    let settings = store.get();
    let listener = match HotkeyListener::spawn(settings.hotkey_config()) {
        Ok(listener) => listener,
        Err(error) => {
            log::error!("the trigger could not be installed: {error}");
            if matches!(error, dile_hotkey::Error::KeyboardAccess) {
                // Not an error out of here, and the reason is mechanical. Start-up runs
                // inside the event loop's first turn, and that loop is what draws a window —
                // so returning now would take the process down before anything could be put
                // on a screen, and *blocking* here to show something would wait forever for
                // a loop that is waiting for this function. [`no_keyboard_access`] hands the
                // window to a thread of its own and lets start-up finish, and the
                // application ends when the person has read it.
                no_keyboard_access(&ui);
                return Ok(());
            }
            return Err(Box::new(error));
        }
    };
    // The panel arms and disarms its three keys on whichever hook is installed right now,
    // and this is that hook. Re-attached in `reapply` every time the trigger changes.
    panel.attach(listener.remote());
    log::info!(
        "hotkey hook installed: {} in {:?} mode",
        settings.hotkey.trigger,
        settings.hotkey.mode
    );

    // A flag rather than a channel: the listener's action channel is the one this thread
    // blocks on, and a second channel would need a select that `std::sync::mpsc` does not
    // have. The flag is read once per timeout, which is as often as it can matter.
    let pending = Arc::new(AtomicBool::new(false));
    let raised = Arc::clone(&pending);
    store.on_change(move |change| {
        if change.hotkey || change.capture {
            raised.store(true, Ordering::SeqCst);
        }
    });

    thread::Builder::new()
        .name("dile-session".to_owned())
        .spawn(move || run(&ui, &store, &engine, &panel, listener, &pending))?;

    Ok(())
}

/// Say, in a window, that the keyboard cannot be read — and what opens it.
///
/// On Linux this is a tray application with no tray yet and quite possibly no terminal behind
/// it, so an error on stderr at this moment reaches nobody. A native dialog is the only
/// surface that exists this early, which is also why the model-download consent uses one.
///
/// **On a thread of its own, and that is not a detail.** `blocking_show` waits on a channel
/// that the *event loop* answers, so calling it from the loop's own turn — which is where
/// start-up runs — waits forever for itself, with no window ever drawn. Measured that way
/// first: the process sat in `mpsc::Receiver::recv` under `no_keyboard_access` and never came
/// back. Here the loop is left free to draw the thing, and the thread that asked for it takes
/// the application down once it has an answer.
///
/// It is not shown for any other hotkey failure. Everything else the operating system refuses
/// a hook for is a bug or a conflict with another program, and neither has a paragraph a user
/// can act on; this one does, and the paragraph is the point of the window.
fn no_keyboard_access(ui: &Ui) {
    let app = ui.app().clone();
    let strings = ui.strings();
    let spawned = thread::Builder::new()
        .name("dile-keyboard-access".to_owned())
        .spawn(move || {
            app.dialog()
                .message(strings.text("dialog.keyboard.body"))
                .title(strings.text("dialog.keyboard.title"))
                .kind(MessageDialogKind::Error)
                .buttons(MessageDialogButtons::Ok)
                .blocking_show();
            // Dismissing it is the answer to "why did nothing happen when I held the key",
            // and there is nothing else this run can do.
            app.exit(1);
        });
    if let Err(error) = spawned {
        // Nowhere left to say it but the log, and nothing left to wait for.
        log::error!("the keyboard-access window could not be opened: {error}");
        ui.app().exit(1);
    }
}

/// The session thread: one action at a time, in the order the user produced them.
fn run(
    ui: &Ui,
    store: &SettingsStore,
    engine: &EngineClient,
    panel: &Arc<Panel>,
    mut listener: HotkeyListener,
    pending: &AtomicBool,
) {
    let elapsed = Elapsed::new();
    let mut settings = store.get();
    let mut capture = open_capture(ui, &settings, &elapsed);
    // Whether audio is being captured right now, which is what tells Esc apart: while a
    // recording is running it cancels the recording, and after one it cancels the panel.
    let mut recording = false;

    loop {
        if pending.swap(false, Ordering::SeqCst) {
            let next = store.get();
            reapply(
                ui,
                panel,
                &settings,
                &next,
                &elapsed,
                &mut listener,
                &mut capture,
            );
            settings = next;
        }

        // A timeout rather than a blocking receive, so a settings change made while nobody
        // is dictating is applied within a fifth of a second. Disconnection ends the session:
        // it means the listener thread stopped, which happens at shutdown.
        let emitted = match listener.actions().recv_timeout(SETTINGS_POLL) {
            Ok(emitted) => emitted,
            Err(RecvTimeoutError::Timeout) => continue,
            Err(RecvTimeoutError::Disconnected) => break,
        };

        match emitted.action {
            Action::StartRecording => {
                if capture.is_none() {
                    // A microphone that was missing at start-up may be plugged in now.
                    capture = open_capture(ui, &settings, &elapsed);
                }
                let Some(open) = capture.as_ref() else {
                    continue;
                };
                match open.start() {
                    Ok(()) => {
                        elapsed.start();
                        recording = true;
                        // Before the panel is shown, because what it captures is whatever had
                        // the focus when the key went down — and the panel is about to be one
                        // more window on the screen, even if it is one that never takes it.
                        panel.begin();
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
                recording = false;
                let Some(open) = capture.as_ref() else {
                    continue;
                };
                match open.stop() {
                    // A release with no press behind it — the application started with the
                    // key already down, or the cap took the recording and the buffer has
                    // already been handed over — gives an empty recording rather than an
                    // error, and nothing is owed to anybody.
                    Ok(taken) if taken.is_empty() => {
                        log::debug!("a release arrived with nothing recorded behind it");
                        panel.close();
                        ui.rest();
                    }
                    Ok(taken) => finish(ui, engine, open.overruns(), &taken),
                    Err(error) => {
                        log::error!("the recording could not be taken: {error}");
                        capture = None;
                        ui.show(Status::NoMicrophone);
                    }
                }
            }

            Action::DiscardRecording => {
                elapsed.stop();
                recording = false;
                if let Some(open) = capture.as_ref()
                    && let Err(error) = open.discard()
                {
                    log::error!("the recording could not be discarded: {error}");
                    capture = None;
                }
                // Closed rather than left showing a recording that is not happening. When the
                // discard was a tap, `OpenPanelIdle` is the very next action in this channel
                // and puts the hint up in its place.
                panel.close();
                ui.rest();
            }

            Action::OpenPanelIdle => {
                log::info!("a tap: the panel opens with the hint and nothing in it");
                panel.open_idle();
            }

            // The three the panel claims while it is on screen. They arrive here rather than
            // in the webview because the panel never has the keyboard: see `crate::panel`.
            Action::PanelTransfer => transfer(panel),

            Action::PanelCancel => {
                if recording {
                    // Esc during a dictation cancels the dictation, not the window. The
                    // release that follows finds an empty capture and says nothing.
                    log::info!("Esc during a recording: the audio is thrown away");
                    elapsed.stop();
                    recording = false;
                    if let Some(open) = capture.as_ref()
                        && let Err(error) = open.discard()
                    {
                        log::error!("the recording could not be discarded: {error}");
                        capture = None;
                    }
                    ui.rest();
                }
                panel.cancel();
            }

            Action::PanelCopy => panel.copy(None),
        }
    }

    log::info!("the hotkey listener has stopped; the session is over");
}

/// Put a settings change into effect on the two things this thread owns.
///
/// **Only what moved.** Re-installing a global keyboard hook because somebody changed the
/// recording cap would be a hook the whole machine pays for; re-opening the microphone
/// because the chord changed would throw away the pre-roll ring for nothing.
///
/// A hook the operating system refuses leaves the previous one in place, because a settings
/// window that can take the hotkey away from a running application would be worse than one
/// that says no. The chord itself is checked when it is saved, so what reaches here is a
/// chord that parses and an operating system that declined anyway.
fn reapply(
    ui: &Ui,
    panel: &Arc<Panel>,
    before: &Settings,
    after: &Settings,
    elapsed: &Elapsed,
    listener: &mut HotkeyListener,
    capture: &mut Option<Capture>,
) {
    if before.hotkey != after.hotkey {
        match HotkeyListener::spawn(after.hotkey_config()) {
            Ok(fresh) => {
                // The assignment drops the previous listener, which takes its hook down and
                // joins its thread. New first would mean two hooks answering one press.
                *listener = fresh;
                // The panel's remote pointed at the hook that has just gone; without this its
                // three keys would be armed on a thread that has ended.
                panel.attach(listener.remote());
                log::info!(
                    "the hotkey is now {} in {:?} mode",
                    after.hotkey.trigger,
                    after.hotkey.mode
                );
            }
            Err(error) => log::error!(
                "the new hotkey could not be installed, so the previous one is still in force: {error}"
            ),
        }
    }

    if before.capture != after.capture {
        // Closed before it is opened: the same device cannot be opened twice, and the one
        // being replaced is usually the one being asked for again.
        *capture = None;
        *capture = open_capture(ui, after, elapsed);
        log::info!(
            "the microphone is now {:?} with a {} s cap and {} ms of pre-roll",
            after.capture.device,
            after.capture.cap_secs,
            after.capture.pre_roll_ms
        );
    }
}

/// Paste what is in the panel, on a thread of its own.
///
/// `Panel::transfer` waits for the target window to come forward and then for the clipboard
/// receipt — up to a couple of seconds — and this is the thread the next key press arrives
/// on. A transfer that blocked here would make Enter the one action after which Dile stops
/// hearing its own hotkey.
fn transfer(panel: &Arc<Panel>) {
    let panel = Arc::clone(panel);
    let spawned = thread::Builder::new()
        .name("dile-paste".to_owned())
        .spawn(move || panel.transfer(None));
    if let Err(error) = spawned {
        log::error!("the paste thread could not be started: {error}");
    }
}

/// Open the microphone and start forwarding its level stream.
///
/// Returns `None` when there is no usable input device, having said so on the tray. The
/// application keeps running: `docs/PROJECT.md` §3 puts the microphone behind a setting, and
/// a tray application that exits because a headset is unplugged is one nobody leaves running.
fn open_capture(ui: &Ui, settings: &Settings, elapsed: &Elapsed) -> Option<Capture> {
    match Capture::open(settings.capture_config()) {
        Ok(capture) => {
            spawn_level_thread(
                ui.clone(),
                capture.levels(),
                elapsed.clone(),
                settings.cap_ms(),
            );
            ui.rest();
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
fn finish(ui: &Ui, engine: &EngineClient, overruns: u64, recording: &Recording) {
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
        handoff(ui.app(), engine, recording);
    } else {
        log::info!(
            "nothing heard in {:.2} s ({} samples); the recording was dropped",
            recording.duration.as_secs_f32(),
            recording.samples.len()
        );
        ui.show(Status::NothingHeard);
    }
}

/// Hand the buffer to the engine and go back to listening for the next key press.
///
/// **This does not block and does not wait for text.** The engine's supervisor owns the
/// transcription, the tray's working state and the cleaned result; what leaves here is 16 kHz
/// mono `f32` with the pre-roll already in front of it, which is exactly what
/// `dile-engine-proto` puts on the wire. A copy is made because the recording belongs to the
/// capture crate and the engine will still be holding it after this function returns.
fn handoff(app: &AppHandle, engine: &EngineClient, recording: &Recording) {
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

    engine.submit(recording.samples.clone());
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
    use super::{Elapsed, LevelPayload};
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
    fn the_level_payload_is_the_shape_wp5_will_read() {
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
    }
}
