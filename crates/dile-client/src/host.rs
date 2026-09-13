//! The child process, and the three ways it can let the application down.
//!
//! [`HostProcess`] owns one `dile-engine-host`: its pipes, the thread that reads its
//! answers, the thread that forwards its log, and the promise that dropping it leaves no
//! process behind. Everything above it — the tier, the model, the queue — is
//! the supervisor's; everything below it is the runtime's.
//!
//! **Every request has a deadline.** The runtime can block for as long as it likes and the
//! application cannot afford to: a Vulkan driver that is compiling two thousand shaders
//! takes a minute the first time, and a Vulkan driver that has wedged takes for ever. Both
//! look identical from here, so the difference is written down as a number — [`LOAD_TIMEOUT`]
//! and [`transcribe_timeout`] — and a request that passes its deadline kills the process by
//! its own pid and reports it as a crash. That is the whole reason the engine is a process:
//! there is no way to un-wedge a thread.
//!
//! **The reply channel closing is how a crash is noticed.** The reader thread ends when
//! stdout does, which is when the process ends, whether it was killed from Task Manager, by
//! the driver, or by [`HostProcess::drop`]. `on_exit` runs then, and the supervisor wakes up.

use std::io::{BufRead, BufReader, Write};
use std::path::PathBuf;
use std::process::{Child, ChildStdin, Command, Stdio};
use std::sync::mpsc::{Receiver, RecvTimeoutError, Sender, channel};
use std::thread;
use std::time::Duration;

use dile_engine_proto::{ProtoError, Request, Response, read_frame, write_audio, write_json};

/// The name of the engine binary, without the platform's executable suffix.
const HOST_STEM: &str = "dile-engine-host";

/// How long `hello` may take. It is a struct literal on the other side; a machine that
/// cannot answer it in ten seconds has a problem the rest of this module cannot fix.
pub const HELLO_TIMEOUT: Duration = Duration::from_secs(10);

/// How long a model load may take.
///
/// Two minutes, and the number is the **cold Vulkan** case rather than the normal one: the
/// first load on a machine compiles the driver's shader cache, which `docs/BUILDING.md`
/// measures at about two thousand SPIR-V shaders. Every load after it is seconds. A timeout
/// tight enough for the warm case would fail every machine exactly once — on first run,
/// which is the run that decides the tier.
pub const LOAD_TIMEOUT: Duration = Duration::from_secs(120);

/// How long a transcription of `audio` may take.
///
/// Thirty seconds plus twice the audio. The constant is the fixed cost — mel spectrogram,
/// the first decode step — and the multiplier is the real-time factor: M0 measured 0.288 for
/// this model on Vulkan and 2.3 for the fastest whisper on a six-core CPU, so twice
/// real time is generous on the tier this product defaults to and about right on the one it
/// falls back to.
#[must_use]
pub fn transcribe_timeout(samples: usize) -> Duration {
    let seconds = samples as f64 / f64::from(dile_engine_proto::SAMPLE_RATE);
    Duration::from_secs(30) + Duration::from_secs_f64(seconds * 2.0)
}

/// Anything that can go wrong talking to the engine process.
#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum HostError {
    /// The engine binary is not next to the application.
    ///
    /// A packaging failure rather than a run-time one. An installed Dile carries
    /// `dile-engine-host` as a Tauri sidecar, which the bundler puts next to the application
    /// executable; a development tree gets it from `cargo build`, into the same
    /// `target/<profile>` directory the application is in. Neither is a thing that happens
    /// by accident, so this error names the command rather than the cause.
    #[error(
        "the engine binary is not beside the application; an installation carries it as a sidecar, and a development build needs: cargo build -p dile-engine-host"
    )]
    NotFound,
    /// The process would not start.
    #[error("the engine process would not start: {0}")]
    Spawn(#[from] std::io::Error),
    /// The pipe to or from the process failed.
    #[error("the engine connection failed: {0}")]
    Pipe(String),
    /// The operating system started the process without giving it a pipe.
    #[error("the engine process was started without a usable stdout")]
    NoPipe,
    /// The process did not answer in time and has been killed.
    #[error("the engine did not answer the {operation} in {seconds} s and was stopped")]
    Timeout {
        /// Which request went unanswered.
        operation: &'static str,
        /// The deadline it passed, in seconds.
        seconds: u64,
    },
    /// The process ended while a request was in flight.
    #[error("the engine process ended during the {0}")]
    Gone(&'static str),
    /// The engine answered, and the answer was no.
    #[error("{0}")]
    Refused(String),
}

/// One running `dile-engine-host`.
pub struct HostProcess {
    child: Child,
    stdin: Option<ChildStdin>,
    replies: Receiver<Result<Response, ProtoError>>,
    /// The operating-system process id, for the log line a crash produces.
    pid: u32,
    /// The id of the next request, so a stale answer is recognised rather than believed.
    next_id: u64,
}

impl HostProcess {
    /// Start the engine process.
    ///
    /// `on_exit` runs on the reader thread when the child's stdout closes, which is the
    /// moment the process has ended however it ended.
    ///
    /// # Errors
    ///
    /// [`HostError::NotFound`] when the binary is not where it should be, and
    /// [`HostError::Spawn`] when the operating system refused to start it.
    pub fn spawn(on_exit: impl FnOnce() + Send + 'static) -> Result<Self, HostError> {
        let binary = locate().ok_or(HostError::NotFound)?;
        let mut child = Command::new(&binary)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()?;

        let pid = child.id();
        let stdin = child.stdin.take();
        let stdout = child.stdout.take().ok_or(HostError::NoPipe)?;
        let stderr = child.stderr.take();

        let (sender, replies) = channel();
        spawn_reader(stdout, sender, on_exit);
        if let Some(stderr) = stderr {
            spawn_log_forwarder(stderr, pid);
        }

        log::info!("engine process {pid} started from {}", binary.display());
        Ok(HostProcess {
            child,
            stdin,
            replies,
            pid,
            next_id: 1,
        })
    }

    /// The operating-system process id.
    #[must_use]
    pub const fn pid(&self) -> u32 {
        self.pid
    }

    /// Ask what this host is: its version and the cargo features it was built with.
    ///
    /// # Errors
    ///
    /// As [`HostProcess::request`].
    pub fn hello(&mut self) -> Result<Response, HostError> {
        let id = self.take_id();
        self.request(&Request::Hello { id }, None, HELLO_TIMEOUT)
    }

    /// Load a model onto a device.
    ///
    /// # Errors
    ///
    /// As [`HostProcess::request`]. A device this build of the host cannot offer comes back
    /// as [`HostError::Refused`] rather than as a silent fall back to the CPU.
    pub fn load(
        &mut self,
        model: &str,
        device: dile_engine_proto::Device,
        threads: u32,
    ) -> Result<Response, HostError> {
        let id = self.take_id();
        self.request(
            &Request::Load {
                id,
                model: model.to_string(),
                device,
                threads,
            },
            None,
            LOAD_TIMEOUT,
        )
    }

    /// Transcribe one buffer, with the dictionary prompt or an empty one.
    ///
    /// # Errors
    ///
    /// As [`HostProcess::request`].
    pub fn transcribe(
        &mut self,
        samples: &[f32],
        language: &str,
        prompt: &str,
    ) -> Result<Response, HostError> {
        let id = self.take_id();
        self.request(
            &Request::Transcribe {
                id,
                language: language.to_string(),
                prompt: prompt.to_string(),
            },
            Some(samples),
            transcribe_timeout(samples.len()),
        )
    }

    /// Send one request and wait for the answer that matches it.
    ///
    /// # Errors
    ///
    /// [`HostError::Pipe`] when the request could not be written, [`HostError::Timeout`]
    /// when the deadline passed — the process is killed first — [`HostError::Gone`] when the
    /// process ended, and [`HostError::Refused`] when the engine said no.
    fn request(
        &mut self,
        request: &Request,
        audio: Option<&[f32]>,
        timeout: Duration,
    ) -> Result<Response, HostError> {
        let operation = request.op();
        let Some(stdin) = self.stdin.as_mut() else {
            return Err(HostError::Gone(operation));
        };

        write_json(stdin, request).map_err(|error| pipe_error(operation, &error))?;
        if let Some(samples) = audio {
            write_audio(stdin, samples).map_err(|error| pipe_error(operation, &error))?;
        }
        stdin
            .flush()
            .map_err(|error| HostError::Pipe(error.to_string()))?;

        let wanted = request.id();
        loop {
            match self.replies.recv_timeout(timeout) {
                Ok(Ok(response)) if response.id == wanted => {
                    return if response.ok {
                        Ok(response)
                    } else {
                        Err(HostError::Refused(response.error_text()))
                    };
                }
                // An answer to a request that has already timed out. Dropped rather than
                // returned: giving the caller the previous dictation's text would be worse
                // than telling it nothing.
                Ok(Ok(stale)) => {
                    log::debug!("the engine answered request {} late", stale.id);
                }
                Ok(Err(error)) => return Err(HostError::Pipe(error.to_string())),
                Err(RecvTimeoutError::Timeout) => {
                    log::error!(
                        "engine process {} did not answer the {operation}; stopping it",
                        self.pid
                    );
                    self.kill();
                    return Err(HostError::Timeout {
                        operation,
                        seconds: timeout.as_secs(),
                    });
                }
                Err(RecvTimeoutError::Disconnected) => return Err(HostError::Gone(operation)),
            }
        }
    }

    /// The id of the next request.
    fn take_id(&mut self) -> u64 {
        let id = self.next_id;
        self.next_id = self.next_id.saturating_add(1);
        id
    }

    /// End the process now, by its own pid and nothing else's.
    pub fn kill(&mut self) {
        // Dropping stdin first gives a healthy child the chance to leave with 0; a wedged
        // one does not notice, which is what the kill is for.
        self.stdin = None;
        if let Err(error) = self.child.kill() {
            log::debug!("engine process {} was already gone: {error}", self.pid);
        }
        let _ = self.child.wait();
    }
}

impl Drop for HostProcess {
    fn drop(&mut self) {
        // `quit` is written rather than the process being killed outright, so that the
        // runtime gets to free a gigabyte of weights the way it means to. The kill behind
        // it is for the case where it does not answer.
        if let Some(stdin) = self.stdin.as_mut() {
            let id = self.next_id;
            let _ = write_json(stdin, &Request::Quit { id });
            let _ = stdin.flush();
        }
        self.kill();
        log::info!("engine process {} has ended", self.pid);
    }
}

/// Turn a codec failure into the error the caller acts on.
fn pipe_error(operation: &'static str, error: &ProtoError) -> HostError {
    match error {
        // A broken pipe during a write is the process having gone away between the last
        // answer and this request, which is a crash rather than a protocol problem.
        ProtoError::Io(io) if io.kind() == std::io::ErrorKind::BrokenPipe => {
            HostError::Gone(operation)
        }
        other => HostError::Pipe(other.to_string()),
    }
}

/// Read answers until the child's stdout closes, then say so.
fn spawn_reader(
    stdout: std::process::ChildStdout,
    sender: Sender<Result<Response, ProtoError>>,
    on_exit: impl FnOnce() + Send + 'static,
) {
    let spawned = thread::Builder::new()
        .name("dile-engine-reader".to_owned())
        .spawn(move || {
            let mut reader = BufReader::new(stdout);
            loop {
                match read_frame(&mut reader) {
                    Ok(Some(frame)) => {
                        let parsed = frame.json::<Response>();
                        if sender.send(parsed).is_err() {
                            // Nobody is listening any more: the supervisor has moved on.
                            break;
                        }
                    }
                    Ok(None) => break,
                    Err(error) => {
                        let _ = sender.send(Err(error));
                        break;
                    }
                }
            }
            // The channel is dropped here, which turns every waiting `recv` into
            // `Disconnected` — the crash signal — before `on_exit` wakes the supervisor.
            drop(sender);
            on_exit();
        });

    if let Err(error) = spawned {
        log::error!("the engine reader thread could not be started: {error}");
    }
}

/// Forward the child's stderr into this application's log, line by line.
fn spawn_log_forwarder(stderr: std::process::ChildStderr, pid: u32) {
    let spawned = thread::Builder::new()
        .name("dile-engine-stderr".to_owned())
        .spawn(move || {
            for line in BufReader::new(stderr).lines() {
                match line {
                    Ok(line) if line.trim().is_empty() => {}
                    Ok(line) => log::info!("[engine {pid}] {line}"),
                    Err(_) => break,
                }
            }
        });

    if let Err(error) = spawned {
        log::warn!("the engine log could not be forwarded: {error}");
    }
}

/// Find the engine binary.
///
/// | Where | When |
/// |---|---|
/// | `DILE_ENGINE_HOST` | debug builds only, and only if it names a file |
/// | **beside the running executable** | always — and the only one a release build has |
/// | `%CARGO_TARGET_DIR%\debug\` and `\release\` | debug builds only |
///
/// **Beside the application first, and that one row is both cases.** `dile-engine-host` is
/// declared as a Tauri sidecar (`bundle.externalBin`), so the bundler drops it into the
/// installation directory next to the application executable with the target triple stripped
/// off its name; a `cargo build` puts both binaries in the same `target/<profile>` directory.
/// The same lookup finds it either way, which is why the sidecar declaration changed the
/// packaging and not this function.
///
/// The two rows around it are debug-build conveniences and **a release build does not
/// contain them** — a released product that took the path of a process it is about to start
/// from the environment would be one an attacker could aim. `CARGO_TARGET_DIR` exists for
/// the one case the middle row cannot cover: a test binary, which cargo puts in
/// `target/debug/deps/`.
fn locate() -> Option<PathBuf> {
    let file_name = format!("{HOST_STEM}{}", std::env::consts::EXE_SUFFIX);

    #[cfg(debug_assertions)]
    if let Ok(explicit) = std::env::var("DILE_ENGINE_HOST") {
        let path = PathBuf::from(explicit);
        if path.is_file() {
            return Some(path);
        }
        log::warn!("DILE_ENGINE_HOST does not point at a file; looking in the usual places");
    }

    if let Ok(current) = std::env::current_exe()
        && let Some(directory) = current.parent()
    {
        let beside = directory.join(&file_name);
        if beside.is_file() {
            return Some(beside);
        }
    }

    #[cfg(debug_assertions)]
    if let Ok(target) = std::env::var("CARGO_TARGET_DIR") {
        for profile in ["debug", "release"] {
            let candidate = PathBuf::from(&target).join(profile).join(&file_name);
            if candidate.is_file() {
                return Some(candidate);
            }
        }
    }

    None
}

#[cfg(test)]
mod tests {
    use super::{HOST_STEM, HostError, LOAD_TIMEOUT, locate, transcribe_timeout};
    use std::time::Duration;

    #[test]
    fn the_transcribe_deadline_grows_with_the_audio_and_starts_generous() {
        // An empty buffer still gets the fixed cost.
        assert_eq!(transcribe_timeout(0), Duration::from_secs(30));

        // Ten seconds of audio: thirty plus twenty.
        let ten_seconds = 10 * dile_engine_proto::SAMPLE_RATE as usize;
        assert_eq!(transcribe_timeout(ten_seconds), Duration::from_secs(50));

        // The longest recording the product allows is 300 s (docs/PROJECT.md §7), and its
        // deadline has to be longer than the CPU tier's real-time factor of about 2.3.
        let cap = 300 * dile_engine_proto::SAMPLE_RATE as usize;
        assert!(transcribe_timeout(cap) >= Duration::from_secs(630));
    }

    #[test]
    fn the_load_deadline_covers_a_cold_vulkan_shader_compile() {
        // docs/BUILDING.md measures ~1,977 SPIR-V shaders on a cold Vulkan build, and the
        // driver does the same kind of work on a machine's first load. A minute is not
        // enough; two is what the first run gets.
        assert!(LOAD_TIMEOUT >= Duration::from_secs(120));
    }

    #[test]
    fn a_missing_engine_binary_is_a_named_failure_and_not_a_panic() {
        // `locate` looks beside the test binary, which in a cargo run is the same directory
        // the engine host is built into — so this asserts the shape of the answer rather
        // than which branch produced it.
        match locate() {
            Some(path) => {
                assert!(path.is_file());
                assert!(
                    path.file_name()
                        .and_then(|name| name.to_str())
                        .is_some_and(|name| name.starts_with(HOST_STEM))
                );
            }
            None => {
                let error = HostError::NotFound;
                assert!(
                    error
                        .to_string()
                        .contains("cargo build -p dile-engine-host"),
                    "the error has to say how to fix it: {error}"
                );
            }
        }
    }
}
