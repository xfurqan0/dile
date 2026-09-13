//! `dile transcribe <file>` — one recording in, the text Dile would have pasted out.
//!
//! **One command, deliberately.** `docs/PROJECT.md` §6 WP7: *"a minimal CLI: `dile
//! transcribe <file>` with `--json`, and nothing more — no scope race with dikte's CLI."*
//! There is no `dile record`, no `dile serve`, no daemon and no batch mode. What this is for
//! is the one thing a tray icon cannot do: be called by something else, and answer in a
//! shape a script can read.
//!
//! **It is the same product, not a second one.** The settings file, the tier this machine
//! decided on, the model that tier runs, the dictionary that becomes the decoder's prompt and
//! the strictness the cleanup uses all come out of [`dile_client`] — the same code the tray
//! reads them with. Type a term into the settings window and the next `dile transcribe` is
//! already using it; there is no second configuration and no second copy of the weights.
//!
//! **It does not decide the tier.** The GPU probe belongs to the application: it costs a cold
//! model load and it is written down once per machine. A command line that probed on its own
//! would either pay that on every invocation or disagree with the application about the same
//! machine, so instead this one reads [`dile_client::tier`] and says what to do when there is
//! nothing there yet.
//!
//! **Nothing leaves the machine except a model download, and only when asked.** The audio is
//! read, converted and handed to a local process. A missing model prints the consent question
//! and stops; `--yes` is the answer, and there is no way to agree by accident.
//!
//! **English only, and that is a decision rather than an omission.** Every word the graphical
//! side shows comes from `locales/<lang>.json`, and a test fails the build on a hard-coded
//! sentence anywhere under `crates/dile-app/src` or `ui/`. This crate is deliberately outside
//! that rule: its output is read by a terminal, a log and a script as often as by a person,
//! and a `--json` document whose sibling diagnostics change language with a settings file is
//! harder to support, not friendlier. The product's *content* — the transcript, the
//! dictionary, the cleanup — is Turkish first either way, because that comes from the engine
//! and `dile-core`, not from here.
//!
//! ```text
//!   dile transcribe take.wav --json
//!     │
//!     ├── settings.json ─── strictness, dictionary, tier override
//!     ├── engine.json ───── the tier this machine decided on
//!     ├── models/ ───────── the weights that tier runs
//!     └── dile-engine-host ─ a process of its own, killed by pid if it wedges
//! ```

#![deny(unsafe_code)]

mod audio;

use std::path::{Path, PathBuf};
use std::process::ExitCode;

use dile_client::download::{self, DownloadError};
use dile_client::host::{HostError, HostProcess};
use dile_client::models::{self, ModelSpec};
use dile_client::paths;
use dile_client::settings::{DICTATION_LANGUAGE, FILE as SETTINGS_FILE, Settings};
use dile_client::tier;
use dile_core::cleanup::{self, Config as CleanupConfig};
use dile_engine_proto::{Device, Response};

/// What the command prints when it is asked, and when it is misused.
const USAGE: &str = "\
dile — hold a key, speak, let go. This is the part without the key.

USAGE
    dile transcribe <file.wav> [--json] [--yes]

ARGUMENTS
    <file.wav>   A WAV file. Any sample rate, any channel count; it is converted
                 to the 16 kHz mono the engine takes, by the same converter the
                 microphone path uses.

OPTIONS
    --json       Print one JSON object instead of the text: raw, cleaned,
                 segments, took_ms, device, model.
    --yes        Agree to download the model this machine's tier needs, if it is
                 not on disk yet. Without it, the question is printed and
                 nothing is fetched.
    -h, --help   Print this.
    -V, --version
                 Print the version.

WHAT IT READS
    The settings window's own settings.json — strictness, dictionary, tier
    override — and the tier this machine decided on when Dile first ran. The
    dictionary becomes the decoder's prompt exactly as it does for a dictation.

EXIT CODES
    0  the text was printed
    1  something failed and said so on stderr
    2  the arguments do not name a thing to do";

fn main() -> ExitCode {
    let arguments: Vec<String> = std::env::args().skip(1).collect();
    match run(&arguments) {
        Ok(()) => ExitCode::SUCCESS,
        Err(failure) => {
            eprintln!("dile: {}", failure.message);
            if let Some(hint) = failure.hint {
                eprintln!("      {hint}");
            }
            ExitCode::from(failure.code)
        }
    }
}

/// A failure with the exit code it deserves, and sometimes the line that fixes it.
#[derive(Debug)]
struct Failure {
    /// What went wrong, as one sentence with no full stop.
    message: String,
    /// What to do about it, when there is something to do.
    hint: Option<String>,
    /// 1 for a failure, 2 for a misuse.
    code: u8,
}

impl Failure {
    /// Something went wrong.
    fn failed(message: impl Into<String>) -> Self {
        Failure {
            message: message.into(),
            hint: None,
            code: 1,
        }
    }

    /// Something went wrong, and there is a line that fixes it.
    fn fix(message: impl Into<String>, hint: impl Into<String>) -> Self {
        Failure {
            message: message.into(),
            hint: Some(hint.into()),
            code: 1,
        }
    }

    /// The arguments do not name a thing to do.
    fn misused(message: impl Into<String>) -> Self {
        Failure {
            message: message.into(),
            hint: Some("dile --help".to_string()),
            code: 2,
        }
    }
}

/// What the arguments asked for.
#[derive(Debug)]
struct Invocation {
    /// The file to transcribe.
    file: PathBuf,
    /// Print JSON instead of the text.
    json: bool,
    /// Agree to a model download.
    consented: bool,
}

/// Parse the arguments, or say why they do not name a thing to do.
fn parse(arguments: &[String]) -> Result<Option<Invocation>, Failure> {
    let mut rest = arguments.iter();
    let Some(first) = rest.next() else {
        println!("{USAGE}");
        return Ok(None);
    };

    match first.as_str() {
        "-h" | "--help" | "help" => {
            println!("{USAGE}");
            return Ok(None);
        }
        "-V" | "--version" | "version" => {
            println!("dile {}", env!("CARGO_PKG_VERSION"));
            return Ok(None);
        }
        "transcribe" => {}
        other => {
            return Err(Failure::misused(format!(
                "there is no `{other}` command; the only one is `transcribe`"
            )));
        }
    }

    let mut file: Option<PathBuf> = None;
    let mut json = false;
    let mut consented = false;
    for argument in rest {
        match argument.as_str() {
            "--json" => json = true,
            "--yes" | "-y" => consented = true,
            flag if flag.starts_with('-') => {
                return Err(Failure::misused(format!("`{flag}` is not an option here")));
            }
            path if file.is_none() => file = Some(PathBuf::from(path)),
            extra => {
                return Err(Failure::misused(format!(
                    "one file at a time; `{extra}` is a second one"
                )));
            }
        }
    }

    let Some(file) = file else {
        return Err(Failure::misused("`transcribe` needs a file to transcribe"));
    };

    Ok(Some(Invocation {
        file,
        json,
        consented,
    }))
}

/// The whole command.
fn run(arguments: &[String]) -> Result<(), Failure> {
    let Some(invocation) = parse(arguments)? else {
        return Ok(());
    };

    if !invocation.file.is_file() {
        return Err(Failure::failed(format!(
            "there is no file at {}",
            invocation.file.display()
        )));
    }

    let config_dir = paths::config_dir().map_err(|error| Failure::failed(error.to_string()))?;
    let data_dir = paths::local_data_dir().map_err(|error| Failure::failed(error.to_string()))?;

    let settings = Settings::load(&config_dir.join(SETTINGS_FILE));
    let tier = resolve_tier(&settings, &config_dir)?;
    let spec = models::for_tier(tier);
    let model = models::directory_in(&data_dir).join(spec.file);

    if !model.is_file() {
        ensure_model(spec, &model, invocation.consented)?;
    }

    let decoded =
        audio::read_wav(&invocation.file).map_err(|error| Failure::failed(error.to_string()))?;
    eprintln!(
        "{:.2} s of audio, {} Hz {} channel(s) -> 16 kHz mono",
        decoded.seconds(),
        decoded.source_rate,
        decoded.source_channels
    );

    let response = transcribe(&model, tier, &decoded.samples, &settings)?;

    let raw = response.text.clone().unwrap_or_default();
    let dictionary = settings.dictionary();
    let cleaned = cleanup::clean_with(
        &raw,
        settings.strictness(),
        &CleanupConfig {
            dictionary,
            ..CleanupConfig::default()
        },
    )
    .text;

    if invocation.json {
        let document = serde_json::json!({
            "raw": raw,
            "cleaned": cleaned,
            "segments": response.segments,
            "took_ms": response.took_ms,
            "device": response.device,
            "model": spec.file,
        });
        let text = serde_json::to_string_pretty(&document)
            .map_err(|error| Failure::failed(format!("the result would not serialize: {error}")))?;
        println!("{text}");
    } else {
        println!("{cleaned}");
    }

    Ok(())
}

/// Which tier this machine runs on: what the user said, then what the probe found.
///
/// The same order the application uses, and the same file. **There is no third branch**: a
/// machine that has never run Dile has never been probed, and this command says so rather
/// than guessing a tier and downloading a gigabyte against the guess.
fn resolve_tier(settings: &Settings, config_dir: &Path) -> Result<Device, Failure> {
    if let Some(wanted) = settings.engine.tier_override {
        return Ok(wanted);
    }
    match tier::read(&config_dir.join(tier::FILE)) {
        Some(record) => Ok(record.tier),
        None => Err(Failure::fix(
            "this machine has not chosen an engine tier yet",
            "start Dile once — it probes the GPU on first run — or set Settings > Engine > Tier",
        )),
    }
}

/// Put the weights on disk, or print the question and stop.
///
/// The tray asks in a dialog; there is nowhere to put a dialog here, so the question is the
/// same question in one paragraph and `--yes` is the answer. **Nothing is fetched without
/// it**, which is the same promise `docs/PROJECT.md` §3 makes about the first run.
fn ensure_model(spec: &ModelSpec, model: &Path, consented: bool) -> Result<(), Failure> {
    if !consented {
        return Err(Failure::fix(
            format!(
                "the {} tier needs {} ({} MB), which is not on this machine",
                spec.file,
                spec.file,
                spec.size_mb()
            ),
            format!(
                "it is downloaded from huggingface.co and nothing else is sent; run it again with --yes to agree, or start Dile and let it ask: {}",
                spec.url
            ),
        ));
    }

    eprintln!(
        "downloading {} ({} MB) from huggingface.co",
        spec.file,
        spec.size_mb()
    );
    let mut last = 200_u8;
    let mut progress = |done: u64, total: u64| {
        let percent = u8::try_from(
            done.saturating_mul(100)
                .checked_div(total)
                .unwrap_or_default(),
        )
        .unwrap_or(100);
        if percent != last {
            last = percent;
            eprint!("\r  {percent} %");
        }
    };
    let result = download::fetch(
        spec.url,
        spec.sha256,
        spec.size_bytes,
        model,
        &mut progress,
        &|| true,
    );
    eprintln!();

    match result {
        Ok(_) => Ok(()),
        Err(DownloadError::Checksum { found, expected }) => Err(Failure::fix(
            format!("the download hashed to {found} rather than {expected}"),
            "the partial file was deleted; run it again",
        )),
        Err(error) => Err(Failure::failed(error.to_string())),
    }
}

/// Start the engine process, load the model, transcribe once, and let the process go.
///
/// The host is dropped at the end of this function, which sends `quit` and then kills by pid
/// if it does not answer. A command that returned while a gigabyte of weights was still held
/// by a child process would be a command that leaks one per invocation.
fn transcribe(
    model: &Path,
    tier: Device,
    samples: &[f32],
    settings: &Settings,
) -> Result<Response, Failure> {
    let mut host = HostProcess::spawn(|| {}).map_err(host_failure)?;

    let hello = host.hello().map_err(host_failure)?;
    if tier == Device::Vulkan && !hello.features.iter().any(|feature| feature == "gpu-vulkan") {
        return Err(Failure::fix(
            "this machine runs the Vulkan tier and the engine beside this command was built without it",
            "rebuild the sidecar with scripts\\build-host.ps1, or install Dile rather than running from a development tree",
        ));
    }

    eprintln!(
        "loading {} on the {} tier",
        model
            .file_name()
            .and_then(|name| name.to_str())
            .unwrap_or("the model"),
        tier.as_str()
    );
    let model_path = model.to_string_lossy().into_owned();
    host.load(&model_path, tier, 0).map_err(host_failure)?;

    let prompt = settings.dictionary().prompt().unwrap_or_default();
    host.transcribe(samples, DICTATION_LANGUAGE, &prompt)
        .map_err(host_failure)
}

/// An engine failure, with the one hint that is worth giving for it.
fn host_failure(error: HostError) -> Failure {
    match error {
        HostError::NotFound => Failure::fix(
            "the engine is not beside this command",
            "an installed Dile carries it; in a development tree, run scripts\\build-host.ps1",
        ),
        other => Failure::failed(other.to_string()),
    }
}

#[cfg(test)]
mod tests {
    use super::{Failure, parse};

    fn arguments(words: &[&str]) -> Vec<String> {
        words.iter().map(|word| (*word).to_string()).collect()
    }

    #[test]
    fn transcribe_takes_a_file_and_two_flags_and_nothing_else() {
        let parsed = parse(&arguments(&["transcribe", "take.wav", "--json", "--yes"]))
            .expect("this parses")
            .expect("and it is an invocation");
        assert_eq!(parsed.file.to_string_lossy(), "take.wav");
        assert!(parsed.json);
        assert!(parsed.consented);

        let plain = parse(&arguments(&["transcribe", "take.wav"]))
            .expect("this parses")
            .expect("and it is an invocation");
        assert!(!plain.json, "--json must be asked for");
        assert!(!plain.consented, "consent must be asked for");
    }

    #[test]
    fn the_flags_may_come_before_the_file() {
        let parsed = parse(&arguments(&["transcribe", "--json", "take.wav"]))
            .expect("this parses")
            .expect("and it is an invocation");
        assert_eq!(parsed.file.to_string_lossy(), "take.wav");
        assert!(parsed.json);
    }

    #[test]
    fn help_and_version_are_answers_rather_than_invocations() {
        for word in ["--help", "-h", "help", "--version", "-V", "version"] {
            let answered = parse(&arguments(&[word])).expect("this parses");
            assert!(answered.is_none(), "`{word}` should print and stop");
        }
        assert!(parse(&[]).expect("no arguments prints the usage").is_none());
    }

    #[test]
    fn every_misuse_exits_with_two_and_points_at_the_help() {
        let misuses = [
            arguments(&["transcribe"]),
            arguments(&["dictate", "take.wav"]),
            arguments(&["transcribe", "take.wav", "--loud"]),
            arguments(&["transcribe", "one.wav", "two.wav"]),
        ];
        for misuse in misuses {
            let failure: Failure = parse(&misuse).expect_err("this is a misuse");
            assert_eq!(failure.code, 2, "{misuse:?} should be a misuse");
            assert_eq!(failure.hint.as_deref(), Some("dile --help"));
        }
    }

    #[test]
    fn the_usage_names_the_one_command_and_the_two_options() {
        assert!(super::USAGE.contains("dile transcribe <file.wav> [--json] [--yes]"));
        assert!(
            !super::USAGE.contains("dile record"),
            "the scope race with dikte's CLI is declined, and the help must not reopen it"
        );
    }
}
