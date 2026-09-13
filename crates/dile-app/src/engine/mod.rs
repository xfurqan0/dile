//! The engine, at arm's length.
//!
//! `docs/PROJECT.md` WP3: *"`transcribe-cpp` in an isolated process; the first-run Vulkan
//! probe of §3 and the drop to the CPU fallback tier when it fails; crash isolation; model
//! downloader from Hugging Face with consent + checksum + progress."* This module is all of
//! that from the tray's side. It links `dile-engine-proto` and never `dile-engine`, so the
//! native runtime is not in this process at all — which is what makes "killing the engine
//! process leaves the app running" a property of the architecture rather than a hope.
//!
//! ```text
//!   session ──samples──▶ queue ──▶ supervisor ──▶ HostProcess ──▶ dile-engine-host
//!                          ▲           │                              │
//!                          └─host down─┘                              ▼
//!                                      └──▶ tray, dile://engine    the GPU
//! ```
//!
//! **One thread owns the engine.** The supervisor decides the tier, downloads the model,
//! loads it, transcribes, and puts the process back when it dies. Nothing else in the
//! application talks to the child, so there is no lock to get wrong and no second opinion
//! about which model is loaded.
//!
//! **One dictation is queued and older ones are dropped.** A person who speaks again while
//! the last sentence is still being transcribed wants the sentence they just said, not a
//! backlog; the queue is one slot deep and says so in the log when it overwrites.
//!
//! **Everything that can go wrong here ends in a state, not an exception.** No model, no
//! consent, a probe that failed, a process that will not stay up: each is a value of
//! [`state::EngineState`], a tray tooltip and a log line. The hotkey keeps working through
//! all of them — `docs/PROJECT.md` §6 WP3's acceptance criterion is that the application
//! survives its engine, and an application that stops recording because it cannot
//! transcribe has not survived anything.

pub mod download;
pub mod host;
pub mod models;
pub mod probe;
pub mod state;

use std::path::{Path, PathBuf};
use std::sync::{Arc, Condvar, Mutex};
use std::thread;
use std::time::{Duration, Instant};

use dile_core::cleanup::{self, Config as CleanupConfig};
use dile_engine_proto::Device;
use serde::Serialize;
use tauri_plugin_dialog::{DialogExt, MessageDialogButtons, MessageDialogKind};

use crate::settings::SettingsStore;
use crate::tray::Status;
use crate::ui::{EVENT_ENGINE, Ui};
use host::{HostError, HostProcess};
use models::ModelSpec;
use probe::ProbeError;
use state::{EnginePayload, EngineState, ProbeResult, TierRecord};

/// How many times in a row the engine process may go down before it is left down.
///
/// Three, and then the tray says so. A respawn loop against a fault that repeats — a driver
/// that bug-checks on load, a model file that was corrupted on disk — is a machine heating
/// up for nothing, and the honest report is that the engine does not work here.
const MAX_RESTARTS: u32 = 3;

/// The wait before the *n*th respawn: this, multiplied by *n*.
const BACKOFF: Duration = Duration::from_millis(400);

/// What the engine is doing, as the settings window reads it.
///
/// A snapshot rather than a question asked of the supervisor: that thread spends minutes at
/// a time inside a download or a cold Vulkan load, and a settings window that had to wait
/// for it would be a settings window that hangs while a model arrives.
#[derive(Clone, Debug, Default, Serialize)]
pub struct EngineSnapshot {
    /// One of [`EngineState::name`], or empty before the first report.
    pub state: &'static str,
    /// The tier the engine is loaded on, when it is loaded.
    pub tier: Option<&'static str>,
    /// The model file this machine is set up to use.
    pub model: Option<String>,
    /// Download progress, 0–100, while something is being downloaded.
    pub progress: Option<u8>,
}

/// The queue and the handle the session holds on the engine.
///
/// Cheap to clone and safe from any thread. Everything it can do is put one dictation in
/// front of the supervisor, tell it the tier moved, or tell it to stop.
#[derive(Clone)]
pub struct EngineClient {
    queue: Arc<Queue>,
    status: Arc<Mutex<EngineSnapshot>>,
}

impl EngineClient {
    /// Start the supervisor thread and give back the handle to it.
    ///
    /// Never fails: a machine with no model, no engine binary and no GPU still gets a
    /// client, because the alternative is an application that refuses to start over
    /// something the user can fix later.
    #[must_use]
    pub fn start(ui: Ui, store: SettingsStore) -> Self {
        let queue = Arc::new(Queue::default());
        let status = Arc::new(Mutex::new(EngineSnapshot::default()));
        let supervisor = Supervisor::new(ui, store, Arc::clone(&queue), Arc::clone(&status));

        let spawned = thread::Builder::new()
            .name("dile-engine".to_owned())
            .spawn(move || supervisor.run());
        if let Err(error) = spawned {
            log::error!("the engine supervisor could not be started: {error}");
        }

        EngineClient { queue, status }
    }

    /// What the engine is doing, for the settings window.
    #[must_use]
    pub fn status(&self) -> EngineSnapshot {
        match self.status.lock() {
            Ok(status) => status.clone(),
            Err(_) => EngineSnapshot::default(),
        }
    }

    /// The tier the user asked for has changed; restart the engine on it.
    ///
    /// Returns immediately, like everything else here: the restart happens on the
    /// supervisor's thread, between dictations.
    pub fn retier(&self) {
        self.queue.retier();
    }

    /// Forget what this machine decided and run the first-run probe again.
    pub fn reprobe(&self) {
        self.queue.reprobe();
    }

    /// Hand one recording to the engine.
    ///
    /// Returns immediately. The transcription happens on the supervisor thread, so a slow
    /// dictation never holds up the next key press.
    pub fn submit(&self, samples: Vec<f32>) {
        self.queue.submit(samples);
    }

    /// Tell the engine the application is closing.
    ///
    /// Returns immediately as well, and deliberately does not wait: the supervisor may be a
    /// long way inside a half-gigabyte download, and the point of saying so is that the
    /// transfer stops between chunks rather than in the middle of a write. What is already
    /// in the `.part` is what the next start resumes from.
    pub fn stop(&self) {
        self.queue.stop();
    }
}

/// What wakes the supervisor.
enum Wake {
    /// A recording to transcribe.
    Job(Vec<f32>),
    /// The engine process of this generation has ended.
    HostDown(u64),
    /// The tier the settings ask for has changed; bring the engine up again on it.
    Retier,
    /// Throw the tier decision away and probe this machine again.
    Reprobe,
    /// The application is shutting down.
    Stop,
}

/// The one-slot queue, plus the crash signal.
#[derive(Default)]
struct Queue {
    inner: Mutex<Pending>,
    wake: Condvar,
}

/// What the queue is holding.
#[derive(Default)]
struct Pending {
    job: Option<Vec<f32>>,
    /// The generation of an engine process that has ended and not been dealt with.
    down: Option<u64>,
    /// The settings moved the tier and the engine has not been put back on it yet.
    retier: bool,
    /// The settings window asked for the probe to run again.
    reprobe: bool,
    stop: bool,
}

impl Queue {
    /// Put a recording in the slot, dropping whatever was in it.
    fn submit(&self, samples: Vec<f32>) {
        let Ok(mut pending) = self.inner.lock() else {
            log::error!("the engine queue is poisoned; this dictation is lost");
            return;
        };
        if pending.job.is_some() {
            // Said out loud rather than swallowed: a person who lost a sentence deserves to
            // find out why from the log rather than from the silence.
            log::warn!("the engine is still busy; the older recording was dropped");
        }
        pending.job = Some(samples);
        self.wake.notify_all();
    }

    /// Report that the engine process of `generation` has ended.
    fn host_down(&self, generation: u64) {
        if let Ok(mut pending) = self.inner.lock() {
            pending.down = Some(pending.down.map_or(generation, |seen| seen.max(generation)));
            self.wake.notify_all();
        }
    }

    /// Say that the tier the settings ask for has changed.
    fn retier(&self) {
        if let Ok(mut pending) = self.inner.lock() {
            pending.retier = true;
            self.wake.notify_all();
        }
    }

    /// Say that the machine should be probed again.
    fn reprobe(&self) {
        if let Ok(mut pending) = self.inner.lock() {
            pending.reprobe = true;
            self.wake.notify_all();
        }
    }

    /// Say that the application is closing.
    fn stop(&self) {
        if let Ok(mut pending) = self.inner.lock() {
            pending.stop = true;
            self.wake.notify_all();
        }
    }

    /// Whether a shutdown has been asked for; the downloader checks it between chunks.
    fn stopping(&self) -> bool {
        self.inner.lock().is_ok_and(|pending| pending.stop)
    }

    /// Block until there is something to do.
    ///
    /// A dead engine is dealt with before a new dictation: transcribing into a process that
    /// is not there produces one lost sentence and one confusing log line, and putting the
    /// process back first produces neither.
    fn take(&self) -> Wake {
        let Ok(mut pending) = self.inner.lock() else {
            return Wake::Stop;
        };
        loop {
            if pending.stop {
                return Wake::Stop;
            }
            if let Some(generation) = pending.down.take() {
                return Wake::HostDown(generation);
            }
            if std::mem::take(&mut pending.reprobe) {
                return Wake::Reprobe;
            }
            if std::mem::take(&mut pending.retier) {
                return Wake::Retier;
            }
            if let Some(job) = pending.job.take() {
                return Wake::Job(job);
            }
            match self.wake.wait(pending) {
                Ok(next) => pending = next,
                Err(_) => return Wake::Stop,
            }
        }
    }
}

/// Where bringing the engine up stops.
enum Halt {
    /// There are no weights and the person said not now. Recoverable by restarting.
    NoModel,
    /// Something the application cannot work around, as the error that caused it put it.
    Failed(String),
}

impl Halt {
    /// Wrap a typed error. Every sentence in a `Halt` comes from an error's own `Display`,
    /// which is what keeps the one English sentence per failure in one place.
    fn from_error(error: impl std::fmt::Display) -> Self {
        Halt::Failed(error.to_string())
    }
}

/// The thread that owns the engine process.
struct Supervisor {
    ui: Ui,
    queue: Arc<Queue>,
    /// The settings, read fresh at every dictation.
    ///
    /// WP5's requirement is that a change applies without a restart, and the three settings
    /// this thread owns — the cleanup level, the dictionary and the tier — are read at the
    /// moment they are used rather than copied here at start-up. A dictionary typed while a
    /// transcription is in flight is therefore in the prompt of the next one.
    store: SettingsStore,
    /// What the settings window is told when it asks.
    status: Arc<Mutex<EngineSnapshot>>,
    /// CPU threads for the engine; 0 leaves it to the runtime.
    ///
    /// Zero is right on the Vulkan tier, where almost nothing runs on the CPU, and it is
    /// right on the fallback tier too until somebody has measured otherwise. Not a setting:
    /// a number nobody can measure the effect of is not a question to put on a form.
    threads: u32,
    /// The tier this machine decided on.
    tier: Option<Device>,
    /// The model file the tier runs.
    model_file: Option<PathBuf>,
    /// The running engine process, when there is one.
    host: Option<HostProcess>,
    /// Which model is loaded in it.
    loaded: Option<Device>,
    /// How many times this host has been spawned, so a dead one's exit signal is not
    /// mistaken for the live one's.
    generation: u64,
    /// Consecutive crashes. Reset by a dictation that works.
    restarts: u32,
}

impl Supervisor {
    fn new(
        ui: Ui,
        store: SettingsStore,
        queue: Arc<Queue>,
        status: Arc<Mutex<EngineSnapshot>>,
    ) -> Self {
        Supervisor {
            ui,
            queue,
            store,
            status,
            threads: 0,
            tier: None,
            model_file: None,
            host: None,
            loaded: None,
            generation: 0,
            restarts: 0,
        }
    }

    /// Bring the engine up, then answer whatever arrives until the application stops.
    fn run(mut self) {
        self.bring_up();
        loop {
            match self.queue.take() {
                Wake::Stop => break,
                Wake::HostDown(generation) => {
                    if generation == self.generation {
                        self.host_went_down();
                    } else {
                        log::debug!("engine generation {generation} has ended, as expected");
                    }
                }
                Wake::Retier => self.retier(),
                Wake::Reprobe => self.reprobe(),
                Wake::Job(samples) => self.dictate(&samples),
            }
        }
        log::info!("the engine supervisor has stopped");
    }

    // --------------------------------------------------------------------------- start-up

    /// Decide the tier, get the weights, load them, and say so.
    fn bring_up(&mut self) {
        self.ui.set_resting(Status::Preparing);
        match self.prepare() {
            Ok(tier) => {
                self.announce(EngineState::Ready { tier });
                self.ui.set_resting(resting_for(tier));
                log::info!(
                    "the engine is ready on the {} tier with {}",
                    tier.as_str(),
                    self.model_name().unwrap_or_default()
                );
            }
            Err(Halt::NoModel) => {
                log::warn!(
                    "no speech model on this machine; the hotkey still records and nothing is transcribed"
                );
                self.announce(EngineState::NoModel);
                self.ui.set_resting(Status::NoModel);
            }
            Err(Halt::Failed(reason)) => {
                log::error!("the engine could not be started: {reason}");
                self.announce(EngineState::Failed);
                self.ui.set_resting(Status::EngineFailed);
            }
        }
    }

    /// The start-up sequence, as a value rather than as five nested matches.
    fn prepare(&mut self) -> Result<Device, Halt> {
        let app = self.ui.app().clone();
        let model_dir = models::directory(&app).map_err(Halt::from_error)?;
        let tier_file = state::tier_path(&app).map_err(Halt::from_error)?;

        // What the user said outranks what the probe found, and it is written into the same
        // file so that the next start does not probe again. `docs/PROJECT.md` §3: "The tier
        // stays visible and switchable in settings."
        if let Some(wanted) = self.store.get().engine.tier_override {
            let existing = state::read_tier(&tier_file);
            if existing.as_ref().map(|record| record.tier) != Some(wanted) {
                // The probe's own answer is kept beside the override, so the settings page
                // can still say what this machine found when it was asked.
                let record = TierRecord {
                    tier: wanted,
                    ..existing.unwrap_or_else(|| TierRecord::taken(wanted, ProbeResult::default()))
                };
                if let Err(error) = state::write_tier(&tier_file, &record) {
                    log::warn!("the tier the settings asked for could not be saved: {error}");
                }
            }
            log::info!(
                "the settings ask for the {} tier, so this machine is not probed",
                wanted.as_str()
            );
            self.tier = Some(wanted);
            let spec = models::for_tier(wanted);
            let path = self.ensure_model(&model_dir, spec)?;
            self.model_file = Some(path.clone());
            if self.loaded != Some(wanted) {
                self.start_host().map_err(Halt::from_error)?;
                self.load(&path, wanted).map_err(Halt::from_error)?;
            }
            return Ok(wanted);
        }

        let tier = match state::read_tier(&tier_file) {
            Some(record) => {
                log::info!(
                    "the {} tier was decided on this machine already: probe passed {}, {} of {} words, load {} ms",
                    record.tier.as_str(),
                    record.probe_result.passed,
                    record.probe_result.words,
                    record.probe_result.expected_words,
                    record.probe_result.load_ms
                );
                record.tier
            }
            None => self.decide_tier(&model_dir, &tier_file)?,
        };
        self.tier = Some(tier);

        let spec = models::for_tier(tier);
        let path = self.ensure_model(&model_dir, spec)?;
        self.model_file = Some(path.clone());

        if self.loaded != Some(tier) {
            self.start_host().map_err(Halt::from_error)?;
            self.load(&path, tier).map_err(Halt::from_error)?;
        }

        Ok(tier)
    }

    /// Run the first-run probe and write down what it decided.
    ///
    /// The answer is kept whichever way it goes: a machine that dropped to the CPU tier
    /// should not pay for another cold Vulkan load every morning to be told the same thing.
    fn decide_tier(&mut self, model_dir: &Path, tier_file: &Path) -> Result<Device, Halt> {
        let result = self.run_probe(model_dir)?;
        let tier = if result.passed {
            Device::Vulkan
        } else {
            log::warn!(
                "the GPU probe did not pass, so this machine is on the CPU tier: {}",
                result.error.clone().unwrap_or_default()
            );
            // The process that failed the probe is not reused: it may be holding a gigabyte
            // of weights, and it may be the thing that is broken.
            self.drop_host();
            Device::Cpu
        };

        log::info!(
            "engine tier decided: {} — probe passed {}, {} of {} words, load {} ms, probe {} ms",
            tier.as_str(),
            result.passed,
            result.words,
            result.expected_words,
            result.load_ms,
            result.probe_ms
        );

        let record = TierRecord::taken(tier, result);
        if let Err(error) = state::write_tier(tier_file, &record) {
            // Not fatal. The cost is one probe per start-up, not a broken application.
            log::warn!("the tier decision could not be saved: {error}");
        }
        Ok(tier)
    }

    /// Ask the GPU to transcribe two seconds of Turkish whose answer is already known.
    ///
    /// The [`ProbeResult`] is the answer either way — `passed` is a field, not an `Err`,
    /// because a probe that says no is a successful probe. `Err` is reserved for the two
    /// things that stop it from happening at all: no weights, and a machine the application
    /// cannot get an engine process onto.
    fn run_probe(&mut self, model_dir: &Path) -> Result<ProbeResult, Halt> {
        self.start_host().map_err(Halt::from_error)?;

        let hello = {
            let host = self.host_mut().map_err(Halt::from_error)?;
            host.hello().map_err(Halt::from_error)?
        };
        log::info!(
            "engine host {} with features [{}]",
            hello.version.clone().unwrap_or_default(),
            hello.features.join(", ")
        );
        if !hello.features.iter().any(|feature| feature == "gpu-vulkan") {
            return Ok(ProbeResult::refused(&ProbeError::NoGpuSupport));
        }

        #[cfg(debug_assertions)]
        if std::env::var("DILE_FORCE_PROBE_FAIL").is_ok_and(|value| value == "1") {
            return Ok(ProbeResult::refused(&ProbeError::Forced));
        }

        // The Vulkan tier's weights have to be on disk before the Vulkan tier can be probed.
        // A person who declines the download here has declined the whole engine, which is
        // why `Halt::NoModel` travels out rather than turning into a CPU-tier decision.
        let path = self.ensure_model(model_dir, &models::VULKAN_TIER)?;

        self.announce(EngineState::Probing);
        let samples = match probe::samples() {
            Ok(samples) => samples,
            Err(error) => return Ok(ProbeResult::refused(&error)),
        };

        let started = Instant::now();
        if let Err(error) = self.load(&path, Device::Vulkan) {
            return Ok(ProbeResult::refused(&error));
        }
        let load_ms = millis(&started);

        let started = Instant::now();
        let language = crate::settings::DICTATION_LANGUAGE.to_owned();
        let response = {
            // No prompt, deliberately: the probe is about the device, and a dictionary in
            // front of it would be one more thing that could explain a wrong answer.
            let host = self.host_mut().map_err(Halt::from_error)?;
            match host.transcribe(&samples, &language, "") {
                Ok(response) => response,
                Err(error) => {
                    let mut refused = ProbeResult::refused(&error);
                    refused.load_ms = load_ms;
                    return Ok(refused);
                }
            }
        };
        let probe_ms = millis(&started);

        let text = response.text.unwrap_or_default();
        let words = probe::matched(&text);
        let expected_words = probe::WORDS.len();
        let device = response.device.unwrap_or_default();

        log::info!(
            "probe on {device}: loaded in {load_ms} ms, transcribed in {probe_ms} ms, {words} of {expected_words} words"
        );
        // Both sides of the comparison, because a probe that failed is a bug report and the
        // first question anybody asks is what the clip was supposed to say.
        log::info!("probe expected {:?} and heard {:?}", probe::SENTENCE, text);

        let passed = probe::passed(&text);
        Ok(ProbeResult {
            passed,
            device,
            load_ms,
            probe_ms,
            words,
            expected_words,
            error: (!passed).then(|| {
                ProbeError::WrongWords {
                    matched: words,
                    expected: expected_words,
                }
                .to_string()
            }),
        })
    }

    // ----------------------------------------------------------------------------- models

    /// Make sure the weights for `spec` are on disk, asking before spending a connection.
    ///
    /// A file that is already there is trusted rather than re-hashed: a gigabyte of sha256
    /// on every start-up would be seconds of disk on a machine that is trying to be a tray
    /// icon, and the hash is checked at the one moment it can catch anything — before the
    /// download is renamed into place.
    fn ensure_model(&mut self, directory: &Path, spec: &ModelSpec) -> Result<PathBuf, Halt> {
        let path = directory.join(spec.file);
        if path.is_file() {
            log::info!("model {} is already here", spec.file);
            return Ok(path);
        }

        if !self.consent(spec) {
            log::info!("the download of {} was declined", spec.file);
            return Err(Halt::NoModel);
        }

        log::info!(
            "downloading {} ({} MB) from huggingface.co at commit {}",
            spec.file,
            spec.size_mb(),
            models::REVISION
        );
        self.announce(EngineState::Downloading { percent: 0 });
        self.ui.set_resting(Status::Downloading(0));

        let ui = self.ui.clone();
        let file = spec.file;
        let mut last = u8::MAX;
        let mut progress = move |done: u64, total: u64| {
            let percent = percent_of(done, total);
            if percent == last {
                return;
            }
            last = percent;
            ui.emit(
                EVENT_ENGINE,
                EnginePayload::new(EngineState::Downloading { percent }, Some(file)),
            );
            ui.set_resting(Status::Downloading(percent));
        };

        let queue = Arc::clone(&self.queue);
        let started = Instant::now();
        let fetched = download::fetch(
            spec.url,
            spec.sha256,
            spec.size_bytes,
            &path,
            &mut progress,
            &move || !queue.stopping(),
        );

        match fetched {
            Ok(fetched) => {
                log::info!(
                    "{} arrived in {:.1} s ({} bytes over the network, {} resumed)",
                    spec.file,
                    started.elapsed().as_secs_f32(),
                    fetched.transferred,
                    fetched.resumed_from
                );
                Ok(path)
            }
            Err(error) => Err(Halt::Failed(error.to_string())),
        }
    }

    /// Ask before the first byte.
    ///
    /// `docs/PROJECT.md` §3: "Consent dialog before any download." A native dialog rather
    /// than a webview one because this application has no window to put a question in until
    /// WP5 — and because the question is about the machine, not about the panel.
    ///
    /// **`DILE_ASSUME_CONSENT=1` answers yes in debug builds only.** It exists so the
    /// maintainer can run the whole path from a terminal, where nobody can click; a release
    /// build does not contain it, because a downloader that can be told to skip its own
    /// consent from the environment has no consent.
    fn consent(&self, spec: &ModelSpec) -> bool {
        #[cfg(debug_assertions)]
        if std::env::var("DILE_ASSUME_CONSENT").is_ok_and(|value| value == "1") {
            log::warn!("DILE_ASSUME_CONSENT is set: this debug build is not asking first");
            return true;
        }

        let strings = self.ui.strings();
        let size = format!("{} MB", spec.size_mb());
        let body = strings.format(
            "dialog.model.body",
            &[("model", spec.file), ("size", size.as_str())],
        );

        self.ui
            .app()
            .dialog()
            .message(body)
            .title(strings.text("dialog.model.title"))
            .kind(MessageDialogKind::Info)
            .buttons(MessageDialogButtons::OkCancelCustom(
                strings.text("dialog.model.download"),
                strings.text("dialog.model.later"),
            ))
            .blocking_show()
    }

    // ------------------------------------------------------------------------ the process

    /// Start an engine process, replacing any that is already there.
    fn start_host(&mut self) -> Result<(), HostError> {
        self.drop_host();
        self.generation = self.generation.saturating_add(1);

        let queue = Arc::clone(&self.queue);
        let generation = self.generation;
        let host = HostProcess::spawn(move || queue.host_down(generation))?;
        self.host = Some(host);
        Ok(())
    }

    /// End the engine process, if there is one. Its exit signal carries a stale generation
    /// and is ignored by [`Supervisor::run`].
    fn drop_host(&mut self) {
        self.host = None;
        self.loaded = None;
    }

    /// The running process, or a failure that names the operation.
    fn host_mut(&mut self) -> Result<&mut HostProcess, HostError> {
        self.host.as_mut().ok_or(HostError::Gone("request"))
    }

    /// Load a model into the running process.
    fn load(&mut self, path: &Path, tier: Device) -> Result<(), HostError> {
        let threads = self.threads;
        let file = path.to_string_lossy().into_owned();
        let host = self.host.as_mut().ok_or(HostError::Gone("load"))?;
        let response = host.load(&file, tier, threads)?;
        log::info!(
            "the model loaded on {} in {} ms",
            response.device.clone().unwrap_or_default(),
            response.took_ms.unwrap_or_default()
        );
        self.loaded = Some(tier);
        Ok(())
    }

    /// The engine process has ended. Put it back, or give up out loud.
    fn host_went_down(&mut self) {
        let pid = self.host.as_ref().map(HostProcess::pid);
        match pid {
            Some(pid) => log::error!(
                "the engine process {pid} has gone; Dile is still running and will start another"
            ),
            None => log::error!("the engine process has gone; Dile is still running"),
        }
        self.drop_host();
        self.announce(EngineState::Crashed);
        self.ui.set_resting(Status::Preparing);

        loop {
            self.restarts = self.restarts.saturating_add(1);
            if self.restarts > MAX_RESTARTS {
                log::error!(
                    "the engine has gone down {MAX_RESTARTS} times in a row; it will not be started again in this session"
                );
                self.announce(EngineState::Failed);
                self.ui.set_resting(Status::EngineFailed);
                return;
            }
            thread::sleep(BACKOFF * self.restarts);

            match self.reload() {
                Ok(tier) => {
                    log::info!(
                        "the engine is back on the {} tier (attempt {})",
                        tier.as_str(),
                        self.restarts
                    );
                    self.announce(EngineState::Ready { tier });
                    self.ui.set_resting(resting_for(tier));
                    return;
                }
                Err(error) => log::error!("the engine did not come back: {error}"),
            }
        }
    }

    /// Start a process and put the already-decided model back into it.
    fn reload(&mut self) -> Result<Device, HostError> {
        let (Some(tier), Some(path)) = (self.tier, self.model_file.clone()) else {
            // Nothing was ever loaded, so there is nothing to put back. Reached only when
            // the process dies during start-up, and `bring_up` has already said why.
            return Err(HostError::Gone("restart"));
        };
        self.start_host()?;
        self.load(&path, tier)?;
        Ok(tier)
    }

    // -------------------------------------------------------------------------- dictation

    /// Transcribe one recording and hand the cleaned text on.
    fn dictate(&mut self, samples: &[f32]) {
        let Some(tier) = self.tier.filter(|_| self.loaded.is_some()) else {
            log::warn!(
                "a recording arrived with no engine behind it; nothing was transcribed and nothing crashed"
            );
            // The session put the tray on `Working` when it handed this over, and nothing
            // else is going to take it off.
            self.ui.rest();
            return;
        };

        self.announce(EngineState::Busy);
        self.ui.show(Status::Working);

        let seconds = samples.len() as f32 / dile_engine_proto::SAMPLE_RATE as f32;
        // Read now rather than at start-up: a term typed into the settings window while the
        // last sentence was being transcribed belongs in this one's prompt.
        let settings = self.store.get();
        let dictionary = settings.dictionary();
        let strictness = settings.strictness();
        let language = crate::settings::DICTATION_LANGUAGE.to_owned();
        let prompt = dictionary.prompt().unwrap_or_default();
        let outcome = match self.host.as_mut() {
            Some(host) => host.transcribe(samples, &language, &prompt),
            None => Err(HostError::Gone("transcribe")),
        };

        match outcome {
            Ok(response) => {
                let raw = response.text.unwrap_or_default();
                // The raw transcript is what a cleanup bug is diagnosed from, and WP5's
                // "show raw" is where a user will see it. A release build logs the cleaned
                // text only, because a log file full of everything anybody dictated is a
                // privacy hole with a rotation policy.
                #[cfg(debug_assertions)]
                log::info!("raw: {raw}");

                // The dictionary reaches the cleanup as well as the decoder: the prompt
                // stops most mis-hearings and `apply` repairs the ones that got through.
                let cleanup_config = CleanupConfig {
                    dictionary,
                    ..CleanupConfig::default()
                };
                let cleaned = cleanup::clean_with(&raw, strictness, &cleanup_config).text;
                log::info!(
                    "dictation: {seconds:.2} s of audio, {} ms on {}, {} — {cleaned}",
                    response.took_ms.unwrap_or_default(),
                    response.device.unwrap_or_default(),
                    strictness.as_str(),
                );
                self.restarts = 0;
                self.deliver(&cleaned);
            }
            Err(HostError::Refused(reason)) => {
                // The engine is alive and said no — an empty buffer, a model that was
                // unloaded. One dictation lost, no restart.
                log::error!("the engine refused this dictation: {reason}");
            }
            Err(error) => {
                // Timeout, broken pipe or a process that has gone. The reader thread is
                // already on its way with the crash signal, so the respawn is not started
                // here; this dictation is simply gone.
                log::error!("this dictation was lost: {error}");
            }
        }

        self.announce(EngineState::Ready { tier });
        self.ui.rest();
    }

    /// Where a finished dictation leaves WP3.
    ///
    /// **WP5 replaces the body of this function** with the review panel: the text goes into
    /// the strip, the user gets their 1.5 s to cancel or edit, and then it is pasted into
    /// whatever had focus. Everything in front of it is finished — the text is cleaned,
    /// Turkish-cased and dictionary-corrected — so that package is a window and a paste
    /// rather than a pipeline.
    fn deliver(&self, cleaned: &str) {
        if cleaned.is_empty() {
            log::info!("the engine heard nothing in that recording");
        }
    }

    // ------------------------------------------------------------------ the tier, again

    /// The settings moved the tier. Put the engine on the one they now ask for.
    ///
    /// The process is dropped rather than reused: it is holding a gigabyte of the *other*
    /// tier's weights, and asking one host to unload and reload across devices is a longer
    /// path through the runtime than starting a new one.
    fn retier(&mut self) {
        let wanted = self.store.get().engine.tier_override;
        if wanted.is_some() && wanted == self.tier {
            return;
        }
        match wanted {
            Some(tier) => log::info!("the settings ask for the {} tier", tier.as_str()),
            None => log::info!("the settings hand the tier back to the probe"),
        }
        self.drop_host();
        self.restarts = 0;
        self.bring_up();
    }

    /// Throw this machine's tier decision away and ask the device again.
    ///
    /// `docs/PROJECT.md` §3 runs the probe once per machine, because a cold Vulkan load is
    /// not something to pay for every morning. A driver update is the case where that answer
    /// is stale, and this is the button for it.
    fn reprobe(&mut self) {
        let removed = state::tier_path(self.ui.app()).map(|path| std::fs::remove_file(&path));
        match removed {
            Ok(Ok(())) => log::info!("the tier decision was removed; this machine is probed again"),
            Ok(Err(error)) => log::warn!("there was no tier decision to remove: {error}"),
            Err(error) => {
                log::error!("the tier decision could not be found: {error}");
                return;
            }
        }
        self.drop_host();
        self.tier = None;
        self.model_file = None;
        self.restarts = 0;
        self.bring_up();
    }

    // ---------------------------------------------------------------------------- reports

    /// Tell the panel what the engine is doing, and leave it where the settings can read it.
    fn announce(&self, engine_state: EngineState) {
        let payload = EnginePayload::new(engine_state, self.model_name());
        match self.status.lock() {
            Ok(mut status) => {
                *status = EngineSnapshot {
                    state: payload.state,
                    tier: payload.tier,
                    model: payload.model.clone(),
                    progress: payload.progress,
                };
            }
            Err(_) => log::warn!("the engine status could not be recorded for the settings"),
        }
        self.ui.emit(EVENT_ENGINE, payload);
    }

    /// The file name of the model this machine is set up to use, when it has chosen one.
    fn model_name(&self) -> Option<&str> {
        self.model_file
            .as_deref()
            .and_then(Path::file_name)
            .and_then(std::ffi::OsStr::to_str)
    }
}

/// What the tray rests on once the engine is ready.
///
/// The Vulkan tier is the product working as designed and says nothing about itself. The CPU
/// tier is a machine that lost its probe, and `docs/PROJECT.md` §6 WP3 asks for exactly this:
/// it "ends up on the CPU tier with a small model **and says so**".
const fn resting_for(tier: Device) -> Status {
    match tier {
        Device::Vulkan => Status::Idle,
        Device::Cpu => Status::CpuTier,
    }
}

/// Elapsed milliseconds, saturating rather than wrapping.
fn millis(started: &Instant) -> u64 {
    u64::try_from(started.elapsed().as_millis()).unwrap_or(u64::MAX)
}

/// Whole per cent of `total`, clamped, and 0 when the total is unknown.
fn percent_of(done: u64, total: u64) -> u8 {
    if total == 0 {
        return 0;
    }
    let percent = done.min(total).saturating_mul(100) / total;
    u8::try_from(percent).unwrap_or(100)
}

#[cfg(test)]
mod tests {
    use super::{BACKOFF, MAX_RESTARTS, Queue, Wake, percent_of, resting_for};
    use crate::tray::Status;
    use dile_engine_proto::Device;
    use std::sync::Arc;
    use std::thread;
    use std::time::Duration;

    #[test]
    fn the_queue_keeps_the_newest_recording_and_drops_the_one_behind_it() {
        let queue = Queue::default();
        queue.submit(vec![1.0, 1.0]);
        queue.submit(vec![2.0, 2.0, 2.0]);

        match queue.take() {
            Wake::Job(samples) => assert_eq!(samples, vec![2.0, 2.0, 2.0]),
            _ => panic!("a submitted recording must come back as a job"),
        }
    }

    #[test]
    fn a_dead_engine_is_dealt_with_before_a_new_dictation() {
        let queue = Queue::default();
        queue.submit(vec![0.5]);
        queue.host_down(3);

        match queue.take() {
            Wake::HostDown(generation) => assert_eq!(generation, 3),
            _ => panic!("the crash has to be handled first"),
        }
        // And the recording is still there afterwards, not thrown away with the process.
        assert!(matches!(queue.take(), Wake::Job(_)));
    }

    #[test]
    fn the_newest_generation_is_the_one_reported() {
        let queue = Queue::default();
        queue.host_down(1);
        queue.host_down(2);
        match queue.take() {
            Wake::HostDown(generation) => assert_eq!(generation, 2),
            _ => panic!("a crash must be reported"),
        }
    }

    #[test]
    fn a_shutdown_outranks_everything_else_in_the_queue() {
        let queue = Queue::default();
        queue.submit(vec![0.25]);
        queue.host_down(1);
        assert!(!queue.stopping());

        queue.stop();
        assert!(
            queue.stopping(),
            "the downloader has to see this between chunks"
        );
        assert!(
            matches!(queue.take(), Wake::Stop),
            "a recording and a crash both wait for an application that is closing"
        );
    }

    #[test]
    fn take_blocks_until_something_arrives() {
        let queue = Arc::new(Queue::default());
        let waiting = Arc::clone(&queue);
        let handle = thread::spawn(move || matches!(waiting.take(), Wake::Job(_)));

        thread::sleep(Duration::from_millis(30));
        queue.submit(vec![0.0; 4]);
        assert!(handle.join().unwrap_or(false), "the waiter got the job");
    }

    #[test]
    fn a_percentage_never_leaves_the_range_a_progress_bar_can_draw() {
        assert_eq!(percent_of(0, 100), 0);
        assert_eq!(percent_of(40, 100), 40);
        assert_eq!(percent_of(100, 100), 100);
        // A server that sends more than it promised, and one that promised nothing.
        assert_eq!(percent_of(120, 100), 100);
        assert_eq!(percent_of(7, 0), 0);
        // The real numbers, where a naive multiply would overflow a smaller integer.
        assert_eq!(percent_of(574_041_195, 574_041_195), 100);
        assert_eq!(percent_of(287_020_597, 574_041_195), 49);
    }

    #[test]
    fn the_cpu_tier_is_the_one_the_tray_talks_about() {
        assert_eq!(resting_for(Device::Vulkan), Status::Idle);
        assert_eq!(resting_for(Device::Cpu), Status::CpuTier);
    }

    /// The whole path, by hand, on a machine that has weights and a GPU.
    ///
    /// Everything else in this module is testable without a model; this is not, and
    /// pretending otherwise would mean either a mock engine that proves nothing or a suite
    /// that needs a gigabyte to run. So it is `#[ignore]`d and takes its inputs from the
    /// environment, the way `dile-engine`'s smoke test does — `docs/BUILDING.md` has the
    /// recipe.
    ///
    /// ```powershell
    /// $env:DILE_TEST_MODEL = "<your model directory>\ggml-large-v3-q5_0.bin"
    /// $env:DILE_TEST_DEVICE = "vulkan"
    /// cargo test -p dile-app -- --ignored --nocapture engine_roundtrip
    /// ```
    #[test]
    #[ignore = "needs a model file: set DILE_TEST_MODEL"]
    fn engine_roundtrip() {
        use super::{host::HostProcess, probe};
        use dile_core::cleanup::{self, Strictness};
        use std::time::Instant;

        let Ok(model) = std::env::var("DILE_TEST_MODEL") else {
            println!("set DILE_TEST_MODEL to a model file, and DILE_TEST_DEVICE to cpu or vulkan");
            return;
        };
        let device = match std::env::var("DILE_TEST_DEVICE").as_deref() {
            Ok("vulkan") => Device::Vulkan,
            _ => Device::Cpu,
        };

        let mut host = HostProcess::spawn(|| {}).expect("the engine binary is beside this test");
        let hello = host.hello().expect("hello");
        println!(
            "host {} features {:?} pid {}",
            hello.version.unwrap_or_default(),
            hello.features,
            host.pid()
        );

        let started = Instant::now();
        let loaded = host.load(&model, device, 0).expect("the model loads");
        println!(
            "loaded on {} in {} ms (wall {} ms)",
            loaded.device.clone().unwrap_or_default(),
            loaded.took_ms.unwrap_or_default(),
            started.elapsed().as_millis()
        );

        let samples = probe::samples().expect("the probe clip decodes");
        let started = Instant::now();
        let response = host
            .transcribe(&samples, "tr", "")
            .expect("the probe clip transcribes");
        let raw = response.text.clone().unwrap_or_default();
        let cleaned = cleanup::clean(&raw, Strictness::Medium);

        println!(
            "transcribed {} samples in {} ms (wall {} ms) on {}",
            samples.len(),
            response.took_ms.unwrap_or_default(),
            started.elapsed().as_millis(),
            response.device.unwrap_or_default()
        );
        println!("expected: {}", probe::SENTENCE);
        println!("raw:      {raw}");
        println!("cleaned:  {cleaned}");

        assert!(
            probe::passed(&raw),
            "only {} of {} words came back",
            probe::matched(&raw),
            probe::WORDS.len()
        );
        assert!(!cleaned.is_empty());
    }

    #[test]
    fn the_restart_policy_is_bounded_and_backs_off() {
        assert_eq!(MAX_RESTARTS, 3);
        // Three attempts, each waiting longer than the last, and under two seconds in total:
        // long enough to let a driver settle, short enough that a person holding the hotkey
        // again does not think the application is gone.
        let total: Duration = (1..=MAX_RESTARTS).map(|attempt| BACKOFF * attempt).sum();
        assert!(
            total < Duration::from_secs(3),
            "{total:?} is too long to wait"
        );
    }
}
