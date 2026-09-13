# Changelog

All notable changes to this project are documented here. The format follows
[Keep a Changelog](https://keepachangelog.com/en/1.1.0/), and this project adheres to
[Semantic Versioning](https://semver.org/spec/v2.0.0.html).

Nothing has been released. The work packages that get to a first release are in
`docs/PROJECT.md` §6.

## [Unreleased]

### Added

- **WP4 — the Turkish layer.** `dile-core` stops returning its input. `clean(raw, level)` is a
  pipeline of eight rules, each a pure function whose documentation opens with the sentence of
  `docs/PROJECT.md` §5 it implements, with `Strictness` deciding which of them run and a
  `CleanReport` recording every edit so the panel's "show raw" can replay the transcript step by
  step. No model, no network, no dependencies — not even a regular-expression crate.
  - **Light:** the filler `eee` and its lengthened forms as a whole word, with the Turkish-aware
    boundary and the comma that trailed it; the canned hallucination phrases; whitespace and
    comma hygiene. The filler rule is a **guard, not a feature**: our own measurements found no
    supported engine writing a vocalised filler at all, and one of its tests is that ordinary
    text comes back untouched.
  - **Medium, the default:** comma pile-ups, a question mark on a sentence that ends in a form of
    *mı/mi/mu/mü* or starts with one of nine question words, the dictionary, and a
    sentence-initial capital that uses Turkish casing and asks the dictionary first, so `cron
    çalışıyor.` keeps its lower-case `cron`.
  - **Strict:** consecutive repeats collapsed outside a whitelist of 29 reduplications, a
    standalone *şey* before a pause dropped while *bir şey* stays, and *ya / yani / hani* dropped
    at a clause edge and never mid-clause. The whitelist errs towards keeping because about 70 %
    of real repeats are legitimate Turkish.
  - **The hallucination filter removes a phrase only when it is a whole segment** — the entire
    text, or a whole sentence standing alone. Never inside a sentence: our own test material has
    `beğenmeyi unutmayın` genuinely spoken in the middle of one. The list is a seed of 27 phrases
    from a public MIT dataset plus our own runs; the rows that were considered and declined are
    kept in the source with the reason beside each, because "abone ol" and "görüşürüz" are things
    people dictate.
  - **The dictionary works in both directions.** `apply` replaces what the engine wrote with what
    the user meant, whole words only, keeping the Turkish suffix — `keş'i` becomes `cache'i`.
    `prompt` turns the same entries into the engine's initial prompt, deterministic and capped so
    it stays inside whisper's window. Matching is case-insensitive under both Turkish and Unicode
    casing and is deliberately not diacritic-folded, and `ç ğ ı ö ş ü` survive byte for byte in
    both the stored spelling and the output. **The shipped dictionary is empty**: every entry is a
    claim about one person's vocabulary.
  - 156 before→after corpus rows across the three levels and the dictionary, plus the properties a
    table cannot state — cleaning twice is the same as cleaning once, and no level ever strips a
    diacritic.
- **WP2 — capture.** Hold `Ctrl+Alt+Space`, speak, let go, and an audio buffer comes back.
  Nothing transcribes it yet; that is WP3.
  - `dile-hotkey`: the push-to-talk trigger as a state machine that reads no clock — every
    event carries the instant it happened at, so tap, hold, toggle, the withdrawn second key
    and the stuck-key ceiling are unit tests rather than a hand test with a stopwatch. The
    Windows adapter sits over `handy-keys` and holds no rules of its own. **Recording starts
    at the press**, so the first syllable is in the buffer before anyone knows whether the
    press was real; a press shorter than 250 ms throws its own recording away.
  - **The primary chord is swallowed, and the second key never is.** The hook is installed
    blocking with `Ctrl+Alt+Space` in its set, because a chord that still reaches the focused
    window puts a space in the editor being dictated into. A bare right Ctrl stays out of that
    set: swallowing it would take `Ctrl+C` away from every application on the machine.
  - `dile-capture`: `cpal` to a lock-free ring to a worker that downmixes, resamples to
    16 kHz mono, keeps a pre-roll ring in front of the recording, stops cleanly at the cap,
    emits a level reading every 50 ms and runs a VAD. Nothing but a push happens in the audio
    callback. The recording is never trimmed — the VAD's opinion travels beside the audio and
    never through it — and the VAD itself is the relative-RMS baseline behind a trait, because
    the M0 row that picks between `earshot` and Silero has not been measured.
  - `dile-app`: the two halves wired together. The tray carries the state in its icon and its
    tooltip — idle, recording, working, nothing heard, no microphone — and level readings and
    state changes leave as the Tauri events `dile://level` and `dile://state`, for the panel
    WP5 will build. A recording with no speech in it is dropped rather than sent anywhere. A
    microphone that will not open is reported and retried at the next press; a keyboard hook
    that will not install is fatal, because every feature begins with a key press.
  - Debug builds write the last recording that held speech next to the application's data, so
    that "no clipped first syllable" is something a person can listen to. **A release build
    does not contain that code**: audio still never reaches disk in anything installable.
- **WP0 — repository skeleton.** A Cargo workspace of three crates, a tray application that
  starts and quits, EN and TR locale files, and CI that gates all of it on Windows.
  - `dile-core`: the Turkish layer's shape — `cleanup` (three strictness levels),
    `dictionary` (non-ASCII safe, and the source of the engine's prompt) and `normalizer`
    (Turkish `i ↔ İ` and `ı ↔ I` casing, which is the one rule already implemented because
    getting it wrong changes words rather than tidying them). No Tauri, no audio, no engine:
    it builds anywhere.
  - `dile-engine`: `transcribe-cpp` 0.2.3 with `default-features = false`, behind a
    `Model` / `Engine` pair that loads a GGUF and transcribes a 16 kHz mono buffer. The
    default cargo feature set is CPU-only and Vulkan sits behind the `gpu-vulkan` feature —
    a build-time switch, not the product's runtime tier, which is the Vulkan-first probe
    described under Changed. Asking for the GPU from a build without the feature is a typed
    error rather than a silent fall back to the slow path. The
    decoder settings are the M0 parity baseline — language hint, temperature 0, no prompt —
    in one place, because there is no beam size in this runtime to fix instead.
  - `dile-app`: a Tauri v2 tray application. Tray icon, a menu with Settings and Quit, and
    **no window shown at start** — the review panel's window is declared hidden and appears
    when the user speaks, from WP5. Bundle identifier `io.github.xfurqan0.dile`, publisher
    Furkan Yıldız, NSIS and MSI targets configured.
  - `locales/en.json` and `locales/tr.json`: the first fifteen keys — tray menu, settings
    title, panel states and panel actions — with the loader the Rust side reads them
    through, and the test that fails on a hard-coded string in the sources or the markup.
  - `docs/BUILDING.md`, `CONTRIBUTING.md`, `SECURITY.md`, `THIRD-PARTY-NOTICES.md`,
    `deny.toml`.
  - CI on `windows-latest`: `cargo fmt --check`, `cargo clippy -D warnings`,
    `cargo test --workspace` on CPU features, and a debug Tauri build with no bundling. A
    second job runs `cargo deny check`. The `gpu-vulkan` feature is not built in CI and
    `docs/BUILDING.md` says why.
- **`scripts/ci-local.ps1`.** The same gate, run on a maintainer's machine against a fresh
  clone of `HEAD` rather than the working tree, so an untracked or ignored file cannot make
  a broken commit look green. `-SkipTauriBuild` is the quick loop and what the pre-push hook
  from `scripts/install-hooks.ps1` runs. It exists because the CI workflow is unverified by a
  real run while the repository is private.

### Changed

- **The default engine tier is Vulkan with a CPU fallback, not CPU with a GPU opt-in.** The
  M0 measurements are what changed it: no whisper model is usable on a six-core CPU (even the
  turbo model runs 2.3× slower than real time), and the small models that *are* fast enough
  cannot take an initial prompt at all, which costs the dictionary — the accuracy feature —
  and lands at a word error around 0.49. So Vulkan is the default, chosen by a probe
  transcription that runs in the isolated engine process on first start; a machine where the
  probe fails gets the CPU tier with a small model. `README.md`, `SECURITY.md` and
  `docs/PROJECT.md` §2–§3 previously said "CPU by default" and now say this.
- **The v1 model is stock Whisper large-v3 `q5_0` with the dictionary prompt**, not a
  Turkish fine-tune. No fine-tune has beaten the stock weights on our own test set, and the
  one Turkish candidate left does not ship in a format the decision runtime can load.
- **M0 is closed, and WP0 with it.** The term-heavy slice of our own test material now has an
  adjudicated reference and has been scored: it confirms the v1 model rather than reversing it,
  and WP0's acceptance criterion — CI green, which a private repository could never produce —
  was met by the first run on the public repository. `docs/PROJECT.md` §4, §6 and §8.
- **The README's "raw Whisper output in Turkish is full of fillers, stutters" claim is
  gone.** Our own M0 data contradicts it: across 20 sentences × 8 engine rows there was not
  one vocalised filler, and every whisper row also repaired a deliberate stutter. The pitch
  it was replaced with is the one the data supports — the dictionary → prompt path and
  Turkish casing and punctuation.

### Known gaps

Everything around the text. Nothing transcribes, nothing pastes, there is no panel and no
settings, and the Turkish layer has no way to reach a user yet: it cleans a string handed to it
in a test. The dictionary has nowhere to be stored, so it is empty in every build. WP2's own
acceptance is **not** closed either: its criteria are about a person holding a key, and that
hand test has not been run yet.
