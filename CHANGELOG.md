# Changelog

All notable changes to this project are documented here. The format follows
[Keep a Changelog](https://keepachangelog.com/en/1.1.0/), and this project adheres to
[Semantic Versioning](https://semver.org/spec/v2.0.0.html).

0.1.0 is the first release. The work packages that got it there are in `docs/PROJECT.md` §6.

## [Unreleased]

### Changed

- **A three-second dictation no longer pays for thirty seconds.** Whisper's encoder is fixed
  at a thirty-second window and pads anything shorter to fill it, so every take — two
  seconds or twenty — cost the same, and that one encoder pass was 80–93 % of the wall
  clock. Dile now shortens the window to the length of the recording. On the Vulkan tier a
  **three-second clip goes from 3.9 s to 1.3 s** and a ten-second one from 3.8 s to 1.5 s,
  with the same transcript; a thirty-second take is unchanged, because at thirty seconds
  there is nothing to trim.
  - The width is a function of the audio, never a fixed number: too narrow a window puts the
    decoder into a repetition loop that costs far more than the encoder saved, so the rule
    scales with the recording and stops at a measured floor.
  - `dile transcribe --json` now reports `audio_ctx`, the window the run actually used.
    `DILE_AUDIO_CTX=0` restores the full thirty-second window on any build, if a machine
    ever needs it back.
  - The speech runtime (`transcribe.cpp`) is now **carried as a fork in this repository**,
    at `vendor/transcribe-cpp/`, because the parameter this needs is not on its public
    surface and upstream declined to add it. It is upstream 0.2.3 plus one field; the
    provenance, the diff and the re-vendoring procedure are in
    `vendor/transcribe-cpp/VENDOR.md`.

- **Linux gets the GPU tier, probed, like Windows.** The packages carry an engine host built
  with `gpu-vulkan`, and the first start transcribes the committed two-second Turkish clip on
  the GPU before trusting it — the same arrangement Windows has had since 0.1.0. Until now a
  Linux machine went to the CPU tier and stayed there unless somebody set
  `engine.tier_override` by hand, because whisper.cpp has an open, unfixed `DeviceLost` fault
  on Intel integrated graphics under Mesa. Measured on exactly that hardware — Intel Iris Xe
  on Mesa 26.1.8 — the fault did not happen in thirty transcriptions, and the tier
  transcribes a three-second clip in **3.7 s against the CPU tier's 14.6 s**, varying by 20 ms
  between runs where the CPU tier varies by seconds. A machine whose driver does fail the
  probe drops to the CPU tier once and the answer is remembered, so nothing here is a promise
  about hardware nobody has measured.
  - **The packages now require a Vulkan loader** (`libvulkan1` on Debian, `libvulkan.so.1` on
    rpm), because the engine binary links it. A machine without one refuses the package
    rather than installing an engine that cannot start.
  - The engine host grows from about 4 MB to about 42 MB, nearly all of it compiled shaders.
    `scripts/build-installer.sh --cpu-engine` builds the smaller, CPU-only shape.

### Fixed

- **The panel reopens where you dragged it on the Linux X11 backend — and under Wayland it
  opens in the middle of the screen every time, which is the honest limit rather than a bug
  left alone.** Dragging the card worked all along; closing it was what threw the position
  away. `gtk_widget_hide` sends `xdg_toplevel.destroy`, mutter answers a destroyed toplevel by
  destroying its window, and the next card is placed from scratch — in the middle of the
  screen, because `center-new-windows` is on. Minimizing the card instead keeps the window
  alive and its place with it, and on **X11** that is what now happens: measured as a number
  there, a window moved to (417, 733) came back from hide-and-show at the centre of the screen
  and from minimize-and-present at (417, 733). The cost is a minimized Dile in the switcher
  while a card is closed.
  - **Under Wayland the same trick does not work, and this release is where that was found
    out.** Keeping the window alive is only half a round trip; the other half is the compositor
    agreeing to raise it. With another application focused — the only state a trigger read off
    an evdev device can fire in, because the compositor never saw the key — the same
    `xdg_activation_v1.activate` gets a `configure` carrying *activated* on a **mapped** surface
    and **nothing at all** on a **minimized** one. The card stayed down, and the second right
    Ctrl of a run did nothing anyone could see. So a closed card is hidden there, as it was
    before, and the start-up log says plainly that every card opens where the compositor puts it.
  - The alternative of never unmapping the window — invisible rather than closed, which would
    keep the place and still raise — is not available either: GDK3 implements neither
    `set_opacity` nor `set_keep_above` on its Wayland backend.
  - Windows is untouched: a card there is hidden and shown as it always was, and its position is
    still remembered per monitor in `settings.json`.
- **`scripts/build-host.sh` asks for the SPIR-V headers before it spends four minutes finding
  out.** `ggml-vulkan.cpp` includes `spirv/unified1/spirv.hpp` directly and never links the
  CMake target that carries its include directory, so a machine with the package config and
  no header on the default path used to configure cleanly and then fail inside a translation
  unit. The GPU pre-flight now puts that question to the compiler along with its existing
  Vulkan loader and `glslc` checks, and names the package to install.

## [0.1.0] — 2026-09-15

### Added

- **The trigger is the right Ctrl, held on its own.** There is one trigger and it takes two
  shapes: a modifier key held alone, or a chord such as `Ctrl+Alt+Space` — which is now what
  you change the trigger to rather than what you start with. A lone modifier is never
  swallowed, so `Right Ctrl` + `C` still copies and anything else pressed during the hold
  withdraws the recording; a chord is swallowed exactly as before, because a `Ctrl+Alt+Space`
  that reached the focused window would put a space in the editor being dictated into. The
  hand test of the installer is what decided it: a hand can rest on a lone modifier for a
  whole sentence, and a three-key chord tires it inside a minute.
  - **`hotkey.trigger` replaces `hotkey.chord` and `hotkey.second_key`** in `settings.json`,
    and holds either shape as a string. A file written by a pre-release build is migrated
    where it is read — `second_key: true` means the right Ctrl already *was* that person's
    trigger, so it becomes `RightCtrl`, and otherwise the chord carries over — and the old
    keys are gone after the first save. Nobody has a released file yet, which is why this is
    a note rather than a migration.
- **WP7 — there is something to install.** An NSIS installer that puts Dile in
  `%LOCALAPPDATA%\Dile` per user with no administrator rights, a winget package, a release
  workflow that builds it on a GitHub runner from a tag, and a command line.
  - **Three programs in one installer.** `dile-engine-host` and `dile` are declared as Tauri
    sidecars (`bundle.externalBin`), which puts them next to the application — the same place
    the client already looked for the engine, so the lookup did not change when the packaging
    did. The bundle carries five files and **no weights**: the model is still downloaded on
    first run, with consent, from a pinned Hugging Face commit.
  - **`dile transcribe <file.wav> [--json]`, and nothing else.** One command, no daemon and no
    batch mode — the scope race with dikte's CLI is still declined. It is the same product
    rather than a second one: it reads the settings window's own `settings.json`, runs the
    tier this machine's probe decided, loads the same weights, and puts the same dictionary in
    the decoder's prompt. It will not download a model without `--yes`; without it the
    question is printed and nothing is fetched. A machine that has never run Dile has never
    been probed, and it says so rather than choosing a tier of its own.
  - **`dile-client`, the half of Dile that has no window.** The settings document, the tier
    record, the model table, the downloader and the engine client moved out of `dile-app` into
    a crate that links no Tauri, because two programs now need all five and a second copy of
    any of them would be two products that disagree. A move rather than a rewrite: the three
    functions that genuinely need an `AppHandle` stayed behind as one-line shims.
  - **A release workflow that stops at a draft.** On a tag: the whole gate again, the tag
    checked against both manifests *before* ten minutes of Vulkan shaders, a pinned Vulkan SDK
    installed and cached on the runner, the installer built, `SHA256SUMS` written, the build
    provenance attested by GitHub, and a **draft** release created. Publishing it is a
    person's click, and so is the tag. The SignPath signing job is written and switched off,
    with the one line that turns it on in a comment above it.
  - `docs/RELEASE.md`, `docs/CODE_SIGNING.md`, `docs/release-notes-0.1.0.md`,
    `packaging/winget/` under `xfurqan0.dile`, and a README that says what Dile is, what it
    measured, what it costs to run and what it does not do.
- **WP5b — the panel, and the paste that ends a dictation.** The product loop closes here.
  Hold the key, speak, let go: the cleaned text appears in a card at the top of the monitor
  the focused window was on, and 2.5 s later it is in that window with your own clipboard put
  back. Cancel it, edit it, copy it instead, re-run the cleanup at another strictness, or look
  at what the engine actually wrote — all before a single character lands anywhere.
  - **The panel never takes the focus**, which is the whole design. `focusable: false` is
    `WS_EX_NOACTIVATE` on Windows, so the editor behind the card keeps its caret, its
    selection and its focus ring. Enter, Esc and `Ctrl+C` are therefore read from the global
    keyboard hook rather than from the page — and **blocked while the panel is up**, because
    an Enter that also reached the editor would paste a sentence *and* a newline. Clicking
    into the text is the one gesture that activates the panel, and the target window gets the
    focus back before anything is pasted.
  - **The paste knows when it has happened.** Dile does not write the clipboard and wait: it
    offers the text as a *delayed render*, sends `Ctrl+V` — `Ctrl+Shift+V` for the terminal
    family — and is told by Windows the moment the target asks for the text. Only then is the
    previous clipboard restored. A target that never pastes times out after two seconds with
    a line in the log, and the clipboard goes back anyway. **Text is restored byte for byte;
    other clipboard formats are not**, and `docs/PROJECT.md` §3 says so rather than leaving it
    to be found out.
  - **It says where it is going.** The window the dictation was aimed at is captured when the
    recording *starts*, not when the text comes back, and the panel shows its name — "→ VS
    Code", "→ Windows Terminal". A window that has gone by the time the text is ready takes
    the paste with it: the text stays in the panel and says so, rather than landing in
    whatever inherited the focus.
  - **A card that fits what is in it.** 680 × 64 while recording and working, growing to three
    lines of text and then scrolling inside itself; a level meter that moves with the
    microphone, an elapsed clock against the cap, an honest working line that names the model
    and the tier, and a thin countdown line rather than a number. Draggable, and remembered
    per monitor. The countdown stops if you press ✗, if you edit the text, or if you simply
    rest the pointer on it for a third of a second — because that is what reading looks like.
  - **All the unsafe code in the application is now in one module** (`src/win32/`), and the
    crate says `deny(unsafe_code)` rather than `forbid` for exactly that reason. The decisions
    about pasting — which chord, which application name, what to restore — are ordinary unit
    tests that run on a machine with no clipboard.

- **WP5a — settings: a file, a window, and nothing that needs a restart.** Everything
  `docs/PROJECT.md` calls a setting is now something a person can change while Dile is
  running: the chord, hold or toggle, the second key, the recording cap, the microphone, the
  cleanup level, auto-transfer and its delay, the engine tier, the dictionary, the interface
  language and start-with-Windows.
  - **`settings.json`, beside `engine.json`.** Every field defaults, so a file written by an
    older build loads and so does one from a newer build carrying fields this one has never
    heard of — losing a dictionary to a downgrade is not an acceptable failure. A value out of
    range is **clamped with a log line rather than rejected**, because a settings file that
    refuses to load is a tray application that will not start. The write is a temporary file
    and a rename: this file holds the dictionary.
  - **Live apply, worked out by comparison.** The settings window sends the whole document on
    every keystroke and the Rust side works out what actually moved, so a dictionary entry
    never re-opens the microphone and a cleanup level never re-installs a global keyboard
    hook. The hook and the microphone are replaced on the thread that owns them; the engine
    reads the strictness and the dictionary at the moment it uses them, so a term typed while
    the last sentence was being transcribed is in the next one's prompt; the tray menu is
    rebuilt in the new language; the tier override is written into `engine.json` and the
    engine restarts on it without probing again.
  - **The dictionary is finally reachable.** It was implemented and measured in WP4 — the
    prompt is what took word error from 0.379 to 0.261 and technical-term recall from 17/31 to
    30/31 — and until now there was nowhere to type one. It is edited inline in a table:
    canonical, variants, pinned, delete, add a row. Two entries claiming the same term are
    refused where they were typed, under Turkish casing, so `Cron` and `cron` are one claim.
  - **A chord you can write down and press.** `Ctrl+Alt+Space` is a string in the file and a
    row of key caps in the window, and the Change button listens for ten seconds and takes
    what you press. `Ctrl+Space` is refused with the reason, and so is a chord with no
    modifier in it — a bare key as a global hotkey takes that key away from the whole machine.
  - **720 × 520, a left rail, dark grey, and no npm.** One HTML file, one stylesheet, one
    script; `frontendDist` still points at `ui/` as static assets. Every visible word is a
    `data-i18n` key resolved from the same catalogue the tray reads, and the no-hard-coded-text
    test now scans every document and script under `ui/`, not just the panel's markup.
  - **Nine commands are the whole of the window's power.** Its capability grants `core:default`
    and nothing else — no dialog, no shell, no filesystem, and none of the autostart plugin's
    permissions even though the plugin is registered: the switch travels as a boolean in the
    document and Rust writes the registry. The panel's capability is untouched.
  - The interface language follows Windows when it is left on Automatic, and switching it
    re-renders the tray, the menu and the window without a restart.
- **WP3 — the engine, at arm's length.** Hold the hotkey, speak, let go, and cleaned Turkish
  text comes back. The engine runs in a process of its own and the tray links the wire rather
  than the runtime, so killing that process costs the dictation in flight and nothing else.
  - `dile-engine-proto`: the framing and the four messages both sides read, in a crate that
    belongs to neither of them and depends on no speech runtime at all. A `u32` length, a kind
    byte, then either UTF-8 JSON or raw `f32` samples at 16 kHz, with a 64 MiB cap so that a
    desynchronised stream fails in a bounded way instead of asking the allocator for whatever
    four misread bytes happen to say. A truncated frame, an oversize one and an audio payload
    that is not a whole number of samples are each a named error rather than a surprise.
  - `dile-engine-host`: the only binary in this workspace that links `transcribe-cpp`. stdout
    is the wire, stderr is the log its parent forwards, and **nothing panics on a bad frame** —
    a request that does not parse comes back as an error response with an id on it and the
    loop goes on. The end of stdin is a normal exit.
  - **The first-run GPU probe.** The application transcribes two seconds of Turkish whose
    answer it already knows, inside the engine process, and only a device that returns enough
    of it is used. The clip is Windows' own Turkish voice, committed, 64 KB — no recording of
    a person ships in this repository, and a probe has to be the same audio on every machine
    or it is measuring the microphone. Half the sentence is the bar, and diacritics are **not**
    folded: a device that writes `guzel` for `güzel` has lost the point of the product. The
    answer is written down once per machine rather than paid for on every start.
  - **A machine that fails the probe drops to the CPU tier, and says so.** The fallback runs
    `ggml-large-v3-turbo-q5_0.bin` — accuracy over speed, because a machine that ends up here
    lost a probe rather than being in a hurry — and the tray reads *CPU tier (GPU probe
    failed)* instead of quietly running ten times slower.
  - **Crash isolation, with a bound.** The engine process going down is a log line, a tray
    state and a respawn with a growing backoff; three in a row and it is left down and said
    so, rather than restarted for ever against a fault that repeats. Every request carries a
    deadline — two minutes for a cold Vulkan load, thirty seconds plus twice the audio for a
    transcription — and one that passes it kills the process by its own pid, because there is
    no way to un-wedge a thread. That is the whole reason the engine is a process.
  - **The model downloader.** Hugging Face only, pinned by commit sha, sha256 embedded in the
    application, `.part` then rename, `Range` resume, progress at four reports a second, and a
    native consent dialog in both languages before the first byte. *Not now* is a state rather
    than an error: the hotkey still records, the tray says there is no model, and nothing
    crashes. Three edge cases are handled because they happened rather than because they were
    imagined — a server that ignores the range, a 416, and a `.part` that is already the whole
    file because the application was closed in the second before the rename.
  - The tray gained five states and one idea: *back to normal* stopped being a constant,
    because what normal looks like now depends on whether this machine has weights, a GPU and
    an engine that stays up.
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

- **The auto-transfer countdown is 2.5 s, not 1.5 s.** The hand test WP5b was waiting for
  passed on Notepad, VS Code, Windows Terminal and a browser field, and asked for exactly one
  change: 1.5 s is not long enough to read a sentence in before it fires. A default rather than
  a rule — a `settings.json` that already carries a number keeps it, and the 500–5000 ms range
  is unchanged.
- **The bundle is NSIS only.** MSI is dropped: a per-user MSI is awkward, WiX is a second
  toolchain to download, and one installer is what a release needs.
- **The sidecars are a build prerequisite now.** `tauri-build` checks `bundle.externalBin` in
  its build script, so `cargo clippy` and `cargo test` fail without them, not only the bundler.
  `scripts/build-host.ps1 -Cpu -DebugBuild` is the cheap pair, and it is the first line of
  `CONTRIBUTING.md` and of `docs/BUILDING.md`'s normal build.
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

### Fixed

- **The binaries were carrying the build machine's account name, 182 times.** Every `panic!`,
  `unwrap()` and `GGML_ASSERT` compiles the source file it came from in as a string literal,
  which `strip = true` does not remove and which no grep over the working tree can see: it
  exists only in the artefact. A release build produced **159** copies of
  `C:\Users\<account>\.cargo\registry\src\…` in `dile.exe` and **23** in
  `dile-engine-host.exe`, and an installer carrying them was days from being published. Two
  compilers, two fixes — `--remap-path-prefix` for rustc, and `/d1trimfile:` for MSVC, because
  ggml is built from source through CMake and no rustc flag reaches it — both passed by
  `scripts/build-installer.ps1`. `scripts/check-binary-paths.ps1` reads all three binaries
  back, and the build **deletes the bundle** rather than leave a failing installer on disk.
- **`Capture::stop` could return before the state it left behind was visible.** The worker
  thread published its new state *after* answering the command, so a caller could be handed a
  finished recording while `state()` still said *Recording*. Narrow enough that it had never
  been seen locally, wide enough to lose on a CI runner — which is where it turned up. The
  state is now stored before the reply in every arm, because the acknowledgement is what makes
  "recording begins now" true for the caller, and everything the caller can observe the moment
  it returns has to be true already.

### Known gaps

**The README has no GIF.** One dictation — hold, speak, the card, the paste — as a screen
recording, which is a thing a person makes rather than a thing a build produces. The README
carries a comment where it goes and describes the loop in words until then.

**The VAD winner is still unmeasured**, so what ships is the relative-RMS baseline behind the
`Vad` trait. **The latency budget** is one machine and one two-second clip, which is a shape
rather than a number to publish.

**Unsigned.** SignPath Foundation asks that a project already be released, so the first
release ships without a certificate and `docs/CODE_SIGNING.md` is what ships in its place —
with a `SHA256SUMS` file and a GitHub build attestation where a signature would go.
