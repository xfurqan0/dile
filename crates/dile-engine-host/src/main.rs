//! The process the speech engine runs in, and the only thing in this workspace that links
//! the runtime.
//!
//! `docs/PROJECT.md` §3 makes the GPU a **probed tier rather than a guess**, and the reason
//! it can be probed at all is this binary: Handy's open bug #1755 is a Vulkan auto-GPU path
//! bug-checking an RTX 5090, and a driver fault is not something a `Result` can contain. So
//! the engine is put behind a process boundary, where a fault costs the dictation that was
//! in flight and nothing else — the tray keeps its hotkey, its microphone and its icon.
//!
//! ```text
//!   dile-app ──stdin──▶ dile-engine-host ──▶ dile-engine ──▶ transcribe-cpp ──▶ Vulkan
//!            ◀─stdout──                 ──stderr──▶ the parent's log
//! ```
//!
//! **stdout is the wire and stderr is the log.** Nothing here prints to stdout that is not
//! a frame; a stray `println!` would desynchronise the stream, which is why every line this
//! file writes for a human goes to stderr and the parent forwards it.
//!
//! **Nothing here panics on a bad frame.** A request that does not parse, an unknown frame
//! kind, an audio payload that is not a whole number of samples: each is an error response
//! with the id it can be matched to, and the loop goes on. The two failures that cannot be
//! answered that way are a broken pipe and a desynchronised stream, and both end the
//! process — the parent notices the stdout channel close and respawns.
//!
//! **The end of stdin is a normal exit.** The application drops the child's stdin at
//! shutdown; [`read_frame`] answers `None`, and this process leaves with 0.

#![forbid(unsafe_code)]

use std::io::{self, Read, Write};
use std::process::ExitCode;
use std::time::Instant;

use dile_engine::{Compute, Engine, Model};
use dile_engine_proto::{
    Device, KIND_AUDIO, ProtoError, Request, Response, Segment, read_frame, write_json,
};

/// The exit code for a stream this process can no longer read or write.
///
/// Distinct from 0 — which means the parent asked it to stop — so that a respawn loop in
/// the application can tell a clean shutdown from a broken pipe without parsing stderr.
const EXIT_PROTOCOL: u8 = 2;

fn main() -> ExitCode {
    let stdin = io::stdin();
    let stdout = io::stdout();
    let mut reader = stdin.lock();
    let mut writer = stdout.lock();

    note(&format!(
        "started: version {}, features [{}]",
        env!("CARGO_PKG_VERSION"),
        build_features().join(", ")
    ));

    let mut host = Host::default();

    loop {
        let frame = match read_frame(&mut reader) {
            Ok(Some(frame)) => frame,
            // stdin closed: the parent is done with this engine, and that is not a failure.
            Ok(None) => {
                note("stdin closed; leaving");
                return ExitCode::SUCCESS;
            }
            Err(error) => {
                // The stream cannot be resynchronised from here — the next four bytes are
                // not a length field any more — so the honest move is to say so and stop.
                note(&format!("the frame stream is unusable: {error}"));
                let _ = reply(&mut writer, &Response::failed(0, error.to_string()));
                return ExitCode::from(EXIT_PROTOCOL);
            }
        };

        let request = match frame.json::<Request>() {
            Ok(request) => request,
            Err(error) => {
                // Recoverable: the framing is intact, only the payload was not a message.
                // The id is 0 because there is no request to take one from.
                note(&format!("a frame was not a request: {error}"));
                if reply(&mut writer, &Response::failed(0, error.to_string())).is_err() {
                    return ExitCode::from(EXIT_PROTOCOL);
                }
                continue;
            }
        };

        if let Request::Quit { id } = request {
            note("quit");
            let _ = reply(&mut writer, &Response::ok(id));
            return ExitCode::SUCCESS;
        }

        let response = host.handle(&request, &mut reader);
        if reply(&mut writer, &response).is_err() {
            note("the parent stopped reading; leaving");
            return ExitCode::from(EXIT_PROTOCOL);
        }
    }
}

/// The loaded model and the session on it, for as long as the parent keeps this process.
///
/// Loading is the expensive part — seconds for a quantized multi-gigabyte model, and the
/// first Vulkan load on a machine also compiles the driver's shader cache — so the parent
/// loads once and transcribes many times, and this struct is what makes that possible.
#[derive(Default)]
struct Host {
    /// Kept alive beside the session: the session holds its own handle on the native model,
    /// and dropping this early would only make a reload look like a new model.
    model: Option<Model>,
    engine: Option<Engine>,
    /// What the runtime said the backend actually is, which is not always what was asked.
    device: Option<String>,
}

impl Host {
    /// Answer one request. `reader` is needed because `transcribe` has an audio frame behind
    /// it, and reading it here keeps the stream in step even when the request fails.
    fn handle(&mut self, request: &Request, reader: &mut impl Read) -> Response {
        match request {
            Request::Hello { id } => {
                let mut response = Response::ok(*id);
                response.version = Some(env!("CARGO_PKG_VERSION").to_string());
                response.features = build_features();
                response
            }

            Request::Load {
                id,
                model,
                device,
                threads,
            } => self.load(*id, model, *device, *threads),

            Request::Transcribe {
                id,
                language,
                prompt,
            } => {
                // The audio frame is read whatever happens next: a request that fails must
                // not leave its own payload in the pipe for the next request to read.
                let samples = match next_audio(reader) {
                    Ok(samples) => samples,
                    Err(error) => return Response::failed(*id, error.to_string()),
                };
                self.transcribe(*id, language, prompt, &samples)
            }

            // Handled before `handle` is reached, so that the answer goes out before the
            // process leaves rather than after.
            Request::Quit { id } => Response::ok(*id),
        }
    }

    /// Load a model onto a device, replacing whatever was loaded before.
    fn load(&mut self, id: u64, path: &str, device: Device, threads: u32) -> Response {
        let compute = match device {
            Device::Cpu => Compute::Cpu,
            Device::Vulkan => Compute::Vulkan,
        };

        // **Where "you decide" is decided**, and here rather than in the application because
        // this is the only place that knows which device was asked for. On the CPU tier the
        // runtime's own answer leaves most of a modern machine idle
        // (`dile_engine::default_threads` carries the measurement); on the Vulkan tier almost
        // nothing runs on the CPU, the number was never measured there, and changing it
        // unmeasured is how the main tier on a Windows machine gets slower for a Linux reason.
        let threads = match (threads, device) {
            (0, Device::Cpu) => dile_engine::default_threads(),
            (asked, _) => asked,
        };

        // Dropped before the load rather than after: two multi-gigabyte models in memory at
        // once is how a machine with 16 GB of RAM fails a tier switch.
        self.engine = None;
        self.model = None;
        self.device = None;

        let started = Instant::now();
        let model = match Model::load(path, compute) {
            Ok(model) => model,
            Err(error) => {
                note(&format!("load failed on {}: {error}", device.as_str()));
                return Response::failed(id, error.to_string());
            }
        };
        let engine = match model.engine_with_threads(threads) {
            Ok(engine) => engine,
            Err(error) => {
                note(&format!("session failed on {}: {error}", device.as_str()));
                return Response::failed(id, error.to_string());
            }
        };

        let took = started.elapsed();
        let backend = model.backend();
        // The thread count is in the line because it is the one load parameter a person
        // looking at a slow dictation can do something about, and because a request that said
        // `0` and a log line that says twenty is how the resolution above is seen to have
        // happened at all.
        let asked_for = if threads == 0 {
            String::from("the runtime's own number of cpu threads")
        } else {
            format!("{threads} cpu threads")
        };
        note(&format!(
            "loaded {} on {backend} ({} arch) with {asked_for} in {} ms",
            path,
            model.arch(),
            took.as_millis()
        ));

        self.device = Some(backend.clone());
        self.model = Some(model);
        self.engine = Some(engine);

        let mut response = Response::ok(id);
        response.device = Some(backend);
        response.took_ms = Some(millis(&started));
        response
    }

    /// Transcribe one buffer with the model that is loaded.
    fn transcribe(&mut self, id: u64, language: &str, prompt: &str, samples: &[f32]) -> Response {
        let Some(engine) = self.engine.as_mut() else {
            return Response::failed(id, "no model is loaded");
        };

        let started = Instant::now();
        let prompt = (!prompt.trim().is_empty()).then_some(prompt);
        let transcript = match engine.transcribe_with(samples, language, prompt) {
            Ok(transcript) => transcript,
            Err(error) => {
                note(&format!("transcribe failed: {error}"));
                return Response::failed(id, error.to_string());
            }
        };

        let took_ms = millis(&started);
        note(&format!(
            "transcribed {} samples in {took_ms} ms, {} segments",
            samples.len(),
            transcript.segments.len()
        ));

        let mut response = Response::ok(id);
        response.text = Some(transcript.text);
        response.segments = transcript
            .segments
            .into_iter()
            .map(|segment| Segment {
                start_ms: segment.start_ms,
                end_ms: segment.end_ms,
                text: segment.text,
            })
            .collect();
        response.took_ms = Some(took_ms);
        response.device = self.device.clone();
        response
    }
}

/// Read the audio frame that follows a `transcribe` request.
fn next_audio(reader: &mut impl Read) -> Result<Vec<f32>, ProtoError> {
    match read_frame(reader)? {
        Some(frame) if frame.kind == KIND_AUDIO => frame.samples(),
        Some(frame) => Err(ProtoError::UnknownKind(frame.kind)),
        None => Err(ProtoError::Truncated {
            declared: 1,
            delivered: 0,
        }),
    }
}

/// Write one response frame.
fn reply(writer: &mut impl Write, response: &Response) -> Result<(), ProtoError> {
    write_json(writer, response)
}

/// Elapsed milliseconds, saturating rather than wrapping.
fn millis(started: &Instant) -> u64 {
    u64::try_from(started.elapsed().as_millis()).unwrap_or(u64::MAX)
}

/// The cargo features this binary was built with, which is what `hello` reports.
///
/// The parent uses it to tell a Vulkan-capable host from a CPU-only one **before** asking
/// for a device it cannot offer, so that a mis-packaged build is one clear line in the log
/// instead of a failed load that reads like a broken driver.
fn build_features() -> Vec<String> {
    let mut features = Vec::new();
    if cfg!(feature = "gpu-vulkan") {
        features.push("gpu-vulkan".to_string());
    }
    features
}

/// One line for a human, on stderr, where it cannot be mistaken for a frame.
fn note(line: &str) {
    let _ = writeln!(io::stderr(), "dile-engine-host: {line}");
}

#[cfg(test)]
mod tests {
    use super::{Host, build_features, next_audio};
    use dile_engine_proto::{Device, KIND_JSON, Request, encode_samples, write_audio, write_frame};
    use std::io::Cursor;

    #[test]
    fn a_transcribe_with_no_model_behind_it_answers_rather_than_panicking() {
        let mut host = Host::default();
        let mut audio: Vec<u8> = Vec::new();
        write_audio(&mut audio, &[0.0f32; 16]).expect("write the audio frame");

        let response = host.handle(
            &Request::Transcribe {
                id: 3,
                language: "tr".to_string(),
                prompt: String::new(),
            },
            &mut Cursor::new(audio),
        );
        assert_eq!(response.id, 3);
        assert!(!response.ok);
        assert!(!response.error_text().is_empty());
    }

    #[test]
    fn hello_reports_the_build_rather_than_what_the_caller_hoped_for() {
        let mut host = Host::default();
        let response = host.handle(&Request::Hello { id: 1 }, &mut Cursor::new(Vec::new()));
        assert!(response.ok);
        assert_eq!(response.version.as_deref(), Some(env!("CARGO_PKG_VERSION")));
        assert_eq!(response.features, build_features());
        assert_eq!(
            response.features.contains(&"gpu-vulkan".to_string()),
            cfg!(feature = "gpu-vulkan"),
            "the feature list must describe this binary, not the other build of it"
        );
    }

    #[test]
    fn a_json_frame_where_audio_was_promised_is_an_error_and_not_a_wait() {
        let mut wire: Vec<u8> = Vec::new();
        write_frame(&mut wire, KIND_JSON, b"{}").expect("write");
        assert!(next_audio(&mut Cursor::new(wire)).is_err());

        // So is a stream that simply stops where the audio should have been.
        assert!(next_audio(&mut Cursor::new(Vec::new())).is_err());

        // A ragged payload — not a whole number of samples — is refused by the codec.
        let mut ragged: Vec<u8> = Vec::new();
        let mut bytes = encode_samples(&[1.0f32]);
        bytes.pop();
        write_frame(&mut ragged, dile_engine_proto::KIND_AUDIO, &bytes).expect("write");
        assert!(next_audio(&mut Cursor::new(ragged)).is_err());
    }

    #[test]
    fn a_load_of_a_file_that_is_not_a_model_fails_with_the_runtimes_own_words() {
        let mut host = Host::default();
        let response = host.load(4, "a-path-no-machine-has.gguf", Device::Cpu, 0);
        assert_eq!(response.id, 4);
        assert!(!response.ok);
        assert!(response.device.is_none(), "a failed load claims no device");
    }
}
