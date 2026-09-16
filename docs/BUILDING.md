# Building Dile

Windows is the platform v1 ships on, and everything below is written for it unless it says
otherwise. **The workspace also builds, lints and tests on Linux** — that is what "Building on
Linux" at the end of this file is about, and it is a build rather than a release: the trigger,
the microphone and the engine work there, and the two things that need a desktop (placing the
card, pasting the text) do not yet. macOS comes from the same codebase later.

## Prerequisites

| What | Why | Needed for |
|---|---|---|
| **Rust, stable** | `rust-toolchain.toml` pins the channel and the components. No target list: rustup installs the host's, and this workspace is never cross-compiled | everything |
| **Visual Studio Build Tools** with the *Desktop development with C++* workload | the MSVC linker, and the compiler that builds `transcribe.cpp` | everything |
| **CMake** ≥ 3.20 | `transcribe-cpp-sys` builds its native library from source through CMake | everything |
| **WebView2** | the panel's runtime. Part of Windows 10 1803+ and Windows 11; the installer downloads a bootstrapper if it is missing | running the app |
| **Vulkan SDK** ≥ 1.3 | the GPU backend's headers, `vulkan-1.lib` and `glslc`, which compiles about two thousand SPIR-V shaders | the `gpu-vulkan` feature only |

```powershell
rustup toolchain install stable
cargo install tauri-cli --locked
```

**The speech runtime is built from a copy in this repository, not from crates.io.**
`vendor/transcribe-cpp/` is `transcribe.cpp` 0.2.3 plus one field this product needs and
upstream declined to add, and the workspace's `[patch.crates-io]` points both
`transcribe-cpp` and `transcribe-cpp-sys` at it. Nothing about the build changes — the same
CMake compiles the same C++ and it takes the same three to four minutes — but a clone is now
the only thing a build needs, and `vendor/transcribe-cpp/VENDOR.md` is where the provenance,
the diff and the procedure for taking a newer upstream live.

A **CPU build needs only CMake and MSVC**, and that is the default cargo feature set, so
nobody has to install a graphics SDK to work on Dile. It is a *build* default and not the
product's runtime tier: `docs/PROJECT.md` §3 makes Vulkan the default tier behind a first-run
probe, with CPU as the fallback, so a release build enables `gpu-vulkan`.

## The normal build

**The sidecars come first — before every cargo command, not only before the bundler.**
`dile-app` declares two external binaries in `tauri.conf.json` (`bundle.externalBin`), and
`tauri-build` checks for them in its build script. A missing one therefore fails `cargo
clippy` and `cargo test` as well as `cargo tauri build`, with:

```
resource path `binaries\dile-engine-host-x86_64-pc-windows-msvc.exe` doesn't exist
```

which is the right failure and a confusing first five minutes if you have not met it before.
One script puts both binaries there:

```powershell
powershell -NoProfile -ExecutionPolicy Bypass -File scripts\build-host.ps1 -Cpu -DebugBuild

cargo fmt --all
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace
cargo tauri build --debug --no-bundle
```

`-Cpu -DebugBuild` is the cheap pair and what CI runs: no Vulkan SDK, no release profile. The
engine host it produces **cannot run the Vulkan tier** — it answers `hello` without the
feature, and both the application and `dile transcribe` say so rather than falling back
silently. Drop both switches for the one that ships.

Roughly three minutes cold, seconds warm. Nearly all of the cold time is
`transcribe-cpp-sys` compiling `transcribe.cpp`; nothing else in the workspace is large.

The copies land in `crates/dile-app/binaries/`, which is git-ignored: they are build
artefacts, and committing a binary to satisfy a build step is how a repository ends up
shipping a stale one.

There is **no frontend build step and no npm**, and WP5 did not change that. `ui/` is static
markup that `frontendDist` points at directly:

```text
ui/index.html            the review panel's shell (WP5b fills it)
ui/settings/index.html   the settings window
ui/settings/settings.css
ui/settings/settings.js
```

WP0 predicted that WP5 would write the panel in TypeScript with esbuild, the way nazar-tray
does, and move `frontendDist` to `ui/dist`. It did not: the settings window is one document,
one stylesheet and one script with no imports and no framework, and a bundler for that would
be a lockfile, an `npm ci` and two minutes of runner time for output identical to its input.
The decision is open again when there is something that actually needs compiling.

Every visible word in those files is a `data-i18n` attribute resolved at run time from
`locales/<lang>.json` — the same catalogue the Rust side reads, handed over by the
`get_strings` command. `crates/dile-app/tests/i18n.rs` reads every `.html` and `.js` under
`ui/` and fails the build on a literal that reads like a sentence.

## Trying the capture path

WP2's acceptance criteria are about a person holding a key — *no clipped first syllable*, *the
cap stops cleanly*, *the IME check on the decided default* — so none of them is a test the
suite can run. This is how they get checked.

```powershell
cargo tauri dev
```

Hold the **right Ctrl** — the default trigger, and a chord such as `Ctrl+Alt+Space` can be set
in its place — say a sentence that begins with a hard consonant, let go. The console prints
the session:

```text
[INFO  dile_app::session] hotkey hook installed: RightCtrl in Hold mode
[INFO  dile_capture] input device "Mikrofon (PRO X)": 48000 Hz, 1 channel(s), f32
[INFO  dile_app::session] recording: 2.31 s, 36960 samples, speech 1340 ms from 0.62 s to 1.96 s
[INFO  dile_app::session] debug build: wrote %LOCALAPPDATA%\io.github.xfurqan0.dile\last.wav (2.31 s, speech true)
```

What to look at, in order:

| Check | What proves it |
|---|---|
| The trigger is not taken from the machine | With the default lone modifier there is nothing to swallow: `Right Ctrl` + `C` still copies in the editor behind the app, and the hold itself leaves nothing in it. Set a chord instead and the opposite is the check — the editor gains no space, because the hook blocks that one. |
| No clipped first syllable | Play `last.wav`. The recording opens with half a second of the room, then the word — the pre-roll ring. |
| The tray follows the session | The accent bar goes red while the key is down and amber for the blink before the buffer is handed on; the tooltip says the same. |
| Silence is not dictated | Hold the key and say nothing: the log says *nothing heard*, no WAV is written, and the tray says so until the next press. |
| A tap does nothing | Press and release under 250 ms: the log shows the tap, and nothing is recorded. |
| The cap stops cleanly | Lower the recording cap in the settings window (or `capture.cap_secs` in `settings.json`), hold past it, and the release still returns the capped buffer with `the recording cap stopped this one` in the log. |

`last.wav` is **debug builds only** and is overwritten every time. A release build does not
contain the code that writes it.

The two crates can also be exercised on their own, without the application:

```powershell
cargo run -p dile-hotkey --example hotkey_probe -- --seconds 30 --trigger RightCtrl
cargo run -p dile-hotkey --example hotkey_probe -- --seconds 30 --trigger Ctrl+Alt+Space
cargo run -p dile-capture --example record -- 5 2
```

The probe prints every decision with the **keyboard layout of the focused window** next to it,
which is how `docs/PROJECT.md` §3's "re-verified against IME in WP2" is actually answered:
switch to the Turkish layout and watch `layout=041F` appear on the line. `--trigger` takes
either shape and defaults to `RightCtrl`; a chord trigger is blocked while the probe runs,
which is the behaviour being checked, and a lone-modifier one never is, so `Ctrl+C` keeps
working throughout. The capture example
records five seconds with a two-second cap, so the cap fires on purpose, draws the level bar
at 20 Hz and writes `out.wav` next to you.

## Trying the panel

WP5b's acceptance criteria are all about a person holding a key and watching where the text
lands, so none of them is a test the suite can run. This is how they get checked. Everything
below is one `cargo tauri dev` and a Notepad.

```powershell
cargo tauri dev
```

**The loop.** Put the caret in a text field — Notepad will do — hold the **right Ctrl**, say
a sentence, let go. The card appears at the top of the screen with the microphone level moving
in it, then says what it is doing while the engine works, then shows the cleaned text with a
thin green line running out underneath. Do nothing and the sentence is in the field 2.5 s
later.

| Check | What proves it |
|---|---|
| The target keeps the focus | The caret in the text field **keeps blinking** the whole time the card is up, and the field keeps its selection. If it stops, `WS_EX_NOACTIVATE` is not on the window — the log says which at start-up: `panel window: hwnd 0x…, non-activating true`. |
| The card is on the right screen | With two monitors, dictate into a window on the **second** one. The card appears above that window, not on the primary. |
| Enter transfers, Esc cancels | Both are read from the global hook, so they work without clicking the card. Esc during a *recording* throws the audio away; Esc after one closes the card and pastes nothing. |
| Enter does not also reach the editor | Press Enter to transfer into a text field: the sentence arrives and **no newline follows it**. That is the hook blocking the key. |
| Editing then ✓ pastes the edit | Click into the text, change a word, press Enter. What lands is what is on screen, not what the engine said. |
| The clipboard comes back | Copy something first (`dile önceki`), dictate, let it transfer, then paste by hand somewhere: **`dile önceki`**, not the dictation. |
| Copy does *not* restore | Dictate, press **Copy**. The clipboard now holds the dictation and keeps it — that is the one place the two paths differ on purpose. |
| Cancel leaves the clipboard alone | Copy something, dictate, press Esc. The clipboard is untouched: nothing was ever put on it. |
| The terminal gets the right chord | Dictate into **Windows Terminal**. It pastes, which means `Ctrl+Shift+V` went out — `Ctrl+V` would have done nothing there. The log names the chord and the application. |
| A gone window is not pasted into | Dictate into a Notepad, then close it *while the card is up*. The card says the window is gone and keeps the text for you to copy. |
| The countdown is stoppable three ways | ✗, any edit, and **resting the pointer on the text for a third of a second**. The green line stops in all three. |
| The strictness pills re-clean | Click *strict*: the sentence changes, and the countdown starts again from the top so there is time to read the new one. `ham` shows what the engine actually wrote. |
| The card is draggable and remembered | Drag it somewhere, dictate again: it comes back where you left it. The position is per monitor, in `settings.json` under `ui.panel_position`. |

**Where the text goes is worth checking in four different kinds of window**, because they
handle pasting differently: **VS Code** (Electron), **Windows Terminal** (`Ctrl+Shift+V`), a
**browser** text field, and **Notepad** (a packaged application with a XAML editor).

**Without a microphone**, the four states can still be looked at:

```powershell
$env:DILE_OPEN_PANEL = "result"; cargo tauri dev
```

**The paste, end to end, by machine.** One test does the whole thing against a real window and
a real clipboard, and it is `#[ignore]` because it opens a Notepad on whoever's desktop is
running it and takes the machine's clipboard for a second:

```powershell
cargo test -p dile-app --bin dile-app -- --ignored --nocapture notepad
```

```text
target: C:\Program Files\WindowsApps\Microsoft.WindowsNotepad_…\Notepad.exe hwnd 0x80882
outcome: Pasted
notepad said: "Non Client Input Sink Window\ndile test"
clipboard after: Some("dile onceki pano")
```

`outcome: Pasted` is the receipt: it means Windows asked this process to hand the text over,
which is the only proof that a paste actually happened rather than a key that went nowhere.

## Regenerating the tray icons

The tray shows one of three icons — `crates/dile-app/icons/tray-idle.png`,
`tray-recording.png` and `tray-working.png`, all 64×64 — and all three come from
`assets/dile.png` by recolouring the one accent bar of the waveform mark. They are committed,
so no build needs `ffmpeg`; this is here for the day the mark changes.

```powershell
$t = "clip((g(X,Y)-max(r(X,Y),b(X,Y)))/54,0,1)"
ffmpeg -y -i assets/dile.png -vf "scale=64:64:flags=lanczos" crates/dile-app/icons/tray-idle.png
ffmpeg -y -i assets/dile.png -vf "format=rgba,geq=r='r(X,Y)+$t*109':g='g(X,Y)+$t*-142':b='b(X,Y)+$t*-83':a='alpha(X,Y)',scale=64:64:flags=lanczos" crates/dile-app/icons/tray-recording.png
ffmpeg -y -i assets/dile.png -vf "format=rgba,geq=r='r(X,Y)+$t*125':g='g(X,Y)+$t*-49':b='b(X,Y)+$t*-124':a='alpha(X,Y)',scale=64:64:flags=lanczos" crates/dile-app/icons/tray-working.png
```

`$t` is how green a pixel is relative to the accent colour `#78D6A0`: `(green − the larger of
red and blue) / 54`, clipped to `0..1`. The background is neutral and the other bars are
near-white, so both score 0 and are left alone; the accent bar scores 1, and the anti-aliased
pixels along its edge score in between. Each channel then moves by `t ×` the difference
between the accent and the target colour, which **swaps** the colour rather than painting over
it — a half-blended edge pixel stays half-blended with the background behind it, so no fringe
appears. The numbers are the per-channel deltas from `#78D6A0` to `#E5484D` (recording) and to
`#F5A524` (working); idle keeps the accent and needs nothing but the scale.

## Running the CI gate locally

GitHub Actions runs on every push now that the repository is public, and `scripts/ci-local.ps1`
is what answers the same question before a push leaves the machine — a red run twelve minutes
after the fact is a worse way to find a formatting error. It mirrors
`.github/workflows/ci.yml`: the same steps, in the same order, with the same environment, and
the `audit` job's `cargo deny check` on the end.

```powershell
cargo install cargo-deny --locked    # once; tauri-cli is already in the prerequisites above
powershell -NoProfile -ExecutionPolicy Bypass -File scripts\ci-local.ps1
```

It **clones `HEAD` into a temporary directory and runs there**, not in the working tree, and
that is the point of it. An untracked file — or one `.gitignore` hides — can carry the piece
that makes a broken commit look green, and a runner would never see it. A tree with
uncommitted changes to tracked files is refused for the same reason; `-AllowDirty` runs
anyway and says plainly that those edits are not in the run.

The build cache, unlike the source, is deliberately **not** fresh. `CARGO_TARGET_DIR` points
at `%LOCALAPPDATA%\dile\ci-target`, so `transcribe.cpp` is not recompiled on every run — the
same trade the CI job makes with `Swatinem/rust-cache`. The source tree is fresh, the build
cache is warm.

| Switch | What it does |
|---|---|
| `-SkipTauriBuild` | fmt, clippy, tests and `cargo deny` only. The quick loop, and what the hook runs. |
| `-AllowDirty` | Runs with a dirty tree. Only the committed state is tested, and it says so. |
| `-Clean` | Wipes the build cache first, so everything compiles from scratch. |

Missing tools are reported up front with the line that installs them, rather than as an
`unknown command` five minutes in. The temporary clone is deleted when every step passes and
kept when one fails, so a red step can be reproduced by hand in the tree that produced it.

### The pre-push hook

```powershell
powershell -NoProfile -ExecutionPolicy Bypass -File scripts\install-hooks.ps1
```

installs a `pre-push` hook that runs the quick loop on every push. Running the installer
again when the hook is already current does nothing, so it is safe after a pull, and
`-Uninstall` takes it back out.

**`git push --no-verify` skips the hook.** That is the right move when the gate is red for a
reason that has nothing to do with what is being pushed — a new advisory against a crate deep
in the Tauri tree, say — and the wrong one the rest of the time.

## The GPU build, and the one thing that breaks it

```powershell
# What the script does, which is what to run:
powershell -NoProfile -ExecutionPolicy Bypass -File scripts\build-host.ps1

# What it does inside, if you would rather do it by hand:
$env:LIB = "$env:VULKAN_SDK\Lib;$env:LIB"
cargo build -p dile-engine-host --release --features gpu-vulkan
```

`scripts\build-host.ps1` checks `VULKAN_SDK` and `vulkan-1.lib` **before** spending eight
minutes on shaders, sets `LIB` itself, builds both sidecars and copies them into the
target-triple name `bundle.externalBin` expects. `-Cpu` drops the feature and the SDK
requirement; `-DebugBuild` drops the release profile.

**`dile-engine-host` is the binary that matters.** It is the only crate in this workspace
that links the runtime; the application talks to it over a pipe and has no dependency on it
at build time (`docs/PROJECT.md` §3, engine tiers). `cargo build -p dile-engine --features
gpu-vulkan` compiles the same native library without the process around it, which is what
that crate's own smoke test needs and nothing else.

**`LIB=%VULKAN_SDK%\Lib;%LIB%` is required, and the Vulkan SDK installer does not set it.**
Without it the link fails with:

```
LINK : fatal error LNK1181: cannot open input file 'vulkan-1.lib'
```

The SDK sets `VULKAN_SDK` machine-wide and puts `glslc` on `PATH`, so the shader compilation
succeeds and everything looks fine until the very last step — which is what makes this worth
a paragraph rather than a footnote. `LIB` is the only missing piece; nothing else needs
setting. In `cmd`, the same line is `set LIB=%VULKAN_SDK%\Lib;%LIB%`.

A cold Vulkan build is about eight minutes against three for CPU: `glslc` compiles ~1,977
SPIR-V shaders on the way. Verified on Vulkan SDK 1.4.357 with CMake 4.4.3, 2026-09-09.

### Where the application looks for the engine

`dile-app` starts `dile-engine-host` by path, and looks in this order:

| Where | When |
|---|---|
| `DILE_ENGINE_HOST` | debug builds only, and only if it names a file |
| next to the running executable | always — and the only one a release build has |
| `%CARGO_TARGET_DIR%\debug\` and `\release\` | debug builds only |

A `cargo build` puts both binaries in the same `target\<profile>` directory, so the second
row covers development as well as an installation. **WP7 declared it as a Tauri sidecar**,
which puts it next to `dile-app.exe` in the installation directory — the same row again. The
lookup did not change when the packaging did, which is why that row says *always*.

The two debug-only rows exist for the one case the first row cannot cover: a test binary,
which cargo puts in `target\debug\deps\`. That is why the round-trip test below wants
`CARGO_TARGET_DIR` set.

**What did change is that the binary is no longer optional at build time.** Before WP7 the
application would compile without it and say so on the tray; now `bundle.externalBin` makes
`tauri-build` refuse, and "The normal build" above is where that is spelled out. The run-time
behaviour is unchanged: an installation that somehow lost its engine still starts, still
records, and still says which command builds one.

**CI does not build the GPU host.** The SDK is a ~250 MB download and an installer on every
run, for a feature that a runner with no GPU cannot exercise anyway, so `ci.yml` builds a
CPU-only debug host purely to satisfy the sidecar check. The GPU build happens in
`.github/workflows/release.yml`, which installs a pinned SDK on the runner and caches it, and
on the maintainer's machine before a tag.

## Building the installer

```powershell
powershell -NoProfile -ExecutionPolicy Bypass -File scripts\build-installer.ps1
```

Four things whose order matters, which is why it is one script: the remapped environment, the
sidecars, `cargo tauri build`, and then a check that reads the binaries back. It prints the
three programs and the installer with their sizes and the installer's SHA-256, which is what
`docs/RELEASE.md` step 2 asks to be kept.

**The check is the part worth knowing about.** Every `panic!`, `unwrap()` and `GGML_ASSERT`
compiles the source file it came from in as a string literal, and `strip = true` does not
touch those — so a release build can carry the path every crate was compiled from, which on
a laptop is an account name inside an installer anyone can download. Measured on this tree
before the fix: **159** copies in `dile.exe`, **23** in `dile-engine-host.exe`, none of them
visible to any grep over the working tree.

Two different fixes, because there are two compilers:

| Half | Flag | Where |
|---|---|---|
| Rust | `--remap-path-prefix`, three prefixes | `CARGO_ENCODED_RUSTFLAGS`, set by the script |
| C and C++ (ggml, through CMake) | `/d1trimfile:` — undocumented, and MSVC's only answer | `CFLAGS` / `CXXFLAGS`, set by the script |

`scripts\check-binary-paths.ps1` is the proof that both still work, and `build-installer.ps1`
**deletes the bundle** if either fails, so an installer that exists after it ran is one that
passed. Run the check on its own whenever something has touched `target\release`:

```powershell
powershell -NoProfile -ExecutionPolicy Bypass -File scripts\check-binary-paths.ps1
```

```text
target\release\dile-app.exe                          5.83 MB  0 match(es)
target\release\dile-engine-host.exe                 54.89 MB  0 match(es)
target\release\dile.exe                              2.38 MB  0 match(es)
no machine-specific paths in any of them
```

One trap: **cargo does not rebuild the native library when `CFLAGS` changes**, because
`transcribe-cpp-sys` declares no `rerun-if-env-changed` for it. A tree that already built
ggml without the trim keeps what it has, the check goes red naming `ggml`, and the fix is
`cargo clean -p transcribe-cpp-sys --release` followed by another run. The script's failure
message says so.

## The command line

`dile transcribe <file.wav>` is the second sidecar. It reads the same `settings.json`, runs
the tier the same `engine.json` records and loads the same weights, so it is the product
rather than a second one:

```powershell
cargo run -p dile-cli -- transcribe crates\dile-app\assets\probe.wav --json
```

```text
2.04 s of audio, 16000 Hz 1 channel(s) -> 16 kHz mono
loading ggml-large-v3-q5_0.bin on the vulkan tier
{
  "raw": "Bugün hava çok güzel ve deniz sakin.",
  "cleaned": "Bugün hava çok güzel ve deniz sakin.",
  "segments": [ { "start_ms": 0, "end_ms": 2040, "text": "Bugün hava çok güzel ve deniz sakin." } ],
  "took_ms": 307,
  "device": "Vulkan0",
  "model": "ggml-large-v3-q5_0.bin"
}
```

It finds `dile-engine-host` the same way the application does — beside itself — so in a
development tree `scripts\build-host.ps1` has to have run, and a `cargo run` of it works
because cargo puts both binaries in the same directory. A machine with no `engine.json` is
told to start Dile once rather than being probed: the probe costs a cold model load and
belongs to the application.

## Running the engine against a real model

Neither weights nor recordings live in this repository (`.gitignore`, `docs/PROJECT.md` §4),
so the engine's end-to-end test is `#[ignore]`d and takes both paths from the environment.
No directory layout is written into the test; point the two variables at your own copies.

| Variable | What it points at |
|---|---|
| `DILE_TEST_MODEL` | A **GGUF** model file — the documented format of the runtime and the pinned source of v1 weights. |
| `DILE_TEST_WAV` | A **16 kHz, mono, 16-bit** WAV, which is the shape the capture stage produces. |

```powershell
$env:DILE_TEST_MODEL = "<your model directory>\whisper-large-v3-turbo-F16.gguf"
$env:DILE_TEST_WAV   = "<your recording directory>\take-02.wav"
cargo test -p dile-engine --test smoke -- --ignored --nocapture
```

Leave either one unset and the test prints which variable is missing and returns without
failing. It is asked for by hand, so the useful answer is the name of the variable to set
rather than a panic that reads like a broken engine.

## Trying the whole path, without a microphone

WP3's round trip — spawn the engine process, load a model, transcribe, clean the text — is
one `#[ignore]`d test. It uses the committed probe clip, so it needs a model and nothing
else, and it prints the raw transcript next to the cleaned one.

```powershell
$env:CARGO_TARGET_DIR = "$PWD\target"   # so the test binary can find dile-engine-host
$env:DILE_TEST_MODEL  = "$env:LOCALAPPDATA\io.github.xfurqan0.dile\models\ggml-large-v3-q5_0.bin"
$env:DILE_TEST_DEVICE = "vulkan"        # or cpu
cargo test -p dile-app --bin dile-app -- --ignored --nocapture engine_roundtrip
```

```text
host 0.1.0 features ["gpu-vulkan"] pid 28384
loaded on Vulkan0 in 921 ms (wall 921 ms)
transcribed 32640 samples in 311 ms (wall 313 ms) on Vulkan0
expected: Bugün hava çok güzel ve deniz sakin.
raw:      Bugün hava çok güzel ve deniz sakin.
cleaned:  Bugün hava çok güzel ve deniz sakin.
```

### The debug-only environment switches

These variables exist so that the paths a person normally has to click through can be run
from a terminal. All of them are behind `#[cfg(debug_assertions)]`: **a release build does not
contain the code that reads them.**

| Variable | What it does |
|---|---|
| `DILE_ASSUME_CONSENT=1` | answers *yes* to the model-download dialog instead of showing it |
| `DILE_MODEL_DIR` | reads and writes models here instead of in the application's local data directory |
| `DILE_FORCE_PROBE_FAIL=1` | makes the first-run GPU probe fail, so the CPU fallback tier can be exercised on a machine whose GPU works |
| `DILE_OPEN_SETTINGS=1` | opens the settings window at start-up, because nothing can click a tray menu from a script |
| `DILE_OPEN_SETTINGS=<group>` | the same, on a chosen group: `hotkey`, `cleanup`, `engine`, `dictionary` or `general` |
| `DILE_OPEN_PANEL=<state>` | renders the review panel with sample text: `recording`, `working`, `result` or `nothing` |
| `DILE_DICTATE_PROBE=1` | hands the committed probe clip to the engine at start-up, as though somebody had just said it |

`DILE_OPEN_PANEL` exists because there is no way to speak into a microphone from a script, so
"does the result state still lay out at two lines" is otherwise a question nobody can answer
from a terminal. The card waits for the page to load, then stays up: a preview **does not
count down and does not close itself**, and it deliberately does **not** arm the panel's three
keys — a preview that swallowed Esc for the whole machine while somebody was photographing it
would be a worse bug than the one it was helping to find.

`DILE_DICTATE_PROBE` is the other half of the same problem and a larger answer to it. Where
`DILE_OPEN_PANEL` draws a card with text written into the source, this one puts
`assets/probe.wav` **through the queue the microphone submits to** — so the transcription, the
Turkish cleanup, the panel and whatever the panel does with the result afterwards are every
one of them the real path, and the only step that is skipped is a person holding a key down.
It is how the whole loop is checked on a machine nobody is sitting at:

```bash
RUST_LOG=info DILE_DICTATE_PROBE=1 ./target/debug/dile-app
wl-paste     # on Linux, a few seconds later: Bugün hava çok güzel ve deniz sakin.
```

On Linux it is the *only* way to check the hand-over, and for a reason worth knowing: the
trigger there is an exclusive grab on an evdev device, so the process listening for the key is
the one process on the machine that cannot synthesise it.

`DILE_MODEL_DIR` is how a machine that already holds these weights avoids downloading a
second copy. The tier decision lives in `%APPDATA%\io.github.xfurqan0.dile\engine.json`;
delete it to make the next start probe again.

**One switch is not in that table and not debug-only.** `DILE_AUDIO_CTX` is read by
`dile-engine` in **release builds too**, on purpose:

| Value | What the engine does |
|---|---|
| unset | the policy: the encoder window is scaled to the length of the recording |
| `0` | the full thirty-second window — exactly the behaviour of every build before this one |
| `1`…`1500` | that window, in encoder positions, whatever the policy would have said |

It is compiled in because it is the one-variable way out of a regression on a machine nobody
here has: the window is the single setting that can make a run both faster and *wrong*, so
there has to be an answer that does not need a new binary. Values below the policy's floor
are accepted here and nowhere else, because reproducing the broken band is what the table in
"Where the time goes" was measured with. The value a run actually used comes back on the
wire and is printed by `dile transcribe --json`.

## Regenerating the probe clip

`crates/dile-app/assets/probe.wav` is the two seconds of Turkish the first-run GPU probe
transcribes. It is synthesised by **Windows' own Turkish text-to-speech voice** — nobody's
recording ships in this repository, and the probe has to be the same audio on every machine.
It is committed, so no build needs a speech platform; this is here for the day the sentence
changes, and the sentence and the expected words live beside it in
`crates/dile-app/src/engine/probe.rs`.

The Turkish voice (*Microsoft Tolga*) is a OneCore voice, which `System.Speech` does not
enumerate — so the clip comes from `Windows.Media.SpeechSynthesis`, which does. It produces
16 kHz mono 16-bit PCM directly, which is the shape `dile-capture` hands the engine; the
second half of the script trims the silence off both ends and rewrites the header, because
the speech platform writes an 18-byte `fmt` chunk and the file has to stay under 100 KB.

```powershell
Add-Type -AssemblyName System.Runtime.WindowsRuntime
$asTask = ([System.WindowsRuntimeSystemExtensions].GetMethods() | Where-Object {
    $_.Name -eq 'AsTask' -and $_.GetParameters().Count -eq 1 -and
    $_.GetParameters()[0].ParameterType.Name -eq 'IAsyncOperation`1' })[0]
function Invoke-WinRtAwait { param($Operation, $Type)
    $task = $asTask.MakeGenericMethod($Type).Invoke($null, @($Operation))
    $task.Wait(-1) | Out-Null; return $task.Result }

[Windows.Media.SpeechSynthesis.SpeechSynthesizer, Windows.Media, ContentType = WindowsRuntime] | Out-Null
[Windows.Storage.Streams.DataReader, Windows.Storage.Streams, ContentType = WindowsRuntime] | Out-Null

$synth = New-Object Windows.Media.SpeechSynthesis.SpeechSynthesizer
$synth.Voice = [Windows.Media.SpeechSynthesis.SpeechSynthesizer]::AllVoices |
    Where-Object { $_.Language -like 'tr*' } | Select-Object -First 1
$stream = Invoke-WinRtAwait $synth.SynthesizeTextToStreamAsync("Bugün hava çok güzel ve deniz sakin.") `
    ([Windows.Media.SpeechSynthesis.SpeechSynthesisStream])

$reader = New-Object Windows.Storage.Streams.DataReader($stream.GetInputStreamAt(0))
Invoke-WinRtAwait $reader.LoadAsync([uint32]$stream.Size) ([uint32]) | Out-Null
$bytes = New-Object byte[] ([int]$stream.Size)
$reader.ReadBytes($bytes)
[System.IO.File]::WriteAllBytes("probe-raw.wav", $bytes)
```

Then trim the near-silent head and tail to about two seconds and write a canonical 44-byte
header. `Where-Object { $_.Language -like 'tr*' }` returning nothing means no Turkish voice is
installed: **stop there.** An English clip, or a synthetic tone, would make the probe test
something other than what it exists to test.

## Building on Linux

The workspace builds, lints, tests and links on Linux, and CI runs the whole gate there on
every push. What that buys is a second platform for every assertion in the suite, which is
worth having on its own: the first Linux run found a set of path fixtures that had only ever
been true on Windows.

What is **not** there yet is the desktop half. `src/platform/` is a seam with two
implementations behind it — `win32/` and `unported.rs` — and on Linux the second one answers.
Concretely:

| | On Linux today |
|---|---|
| The trigger, hold-to-talk, right Ctrl | works, once one udev rule is installed — see below |
| Microphone, pre-roll, VAD, 16 kHz | works, through ALSA — which is also how cpal reaches PipeWire |
| The engine, `dile transcribe`, model download | works, on **both** tiers; the GPU is probed on first start, as on Windows — measured below |
| Tray icon and menu | works, and on GNOME needs the AppIndicator extension — the application now says so itself when nothing is hosting one |
| The clipboard | works, and it is how a dictation is handed over here: the card says *copied — press Ctrl+V to paste* |
| Placing the card on a screen | **no** — `xdg-shell` has no global coordinate space |
| Naming the target application | **no** — a Wayland client is not told what has the focus |
| Automatic paste | **off by default, and experimental where it is on** — see below |

The application says each of those in the log at start-up rather than failing quietly.
`docs/PROJECT.md` §9 is why the hand-over is shaped that way rather than as a paste.

### System packages

```bash
# Fedora
sudo dnf install alsa-lib-devel cmake gcc-c++ \
  webkit2gtk4.1-devel gtk3-devel libsoup3-devel javascriptcoregtk4.1-devel \
  librsvg2-devel libayatana-appindicator-gtk3-devel libxdo-devel

# Debian and Ubuntu
sudo apt install libasound2-dev cmake build-essential \
  libwebkit2gtk-4.1-dev libgtk-3-dev librsvg2-dev \
  libayatana-appindicator3-dev libxdo-dev
```

`alsa-lib-devel` (`libasound2-dev`) is the one that is easy to miss and impossible to work
around: `alsa-sys` looks for `alsa.pc` through pkg-config and carries no vendored source, so
without it `dile-capture` stops with *"The system library `alsa` required by crate `alsa-sys`
was not found"*. It is needed even on a machine where PipeWire is what actually serves the
stream — cpal reaches PipeWire through ALSA's compatibility layer.

Four more for the GPU build, which is what a release is built with here and what
`--cpu-engine` skips:

```bash
sudo dnf install vulkan-loader-devel vulkan-headers glslc spirv-headers-devel   # Fedora
sudo apt install libvulkan-dev glslc spirv-headers                             # Debian and Ubuntu
```

**The SPIR-V headers are the one that is easy to miss.** CMake does not ask for them on your
behalf: `find_package(SPIRV-Headers)` succeeds off a package config while `ggml-vulkan.cpp`
includes `spirv/unified1/spirv.hpp` directly through `__has_include`, and never links the
target that carries its include directory. So a machine with the config and no header on the
default include path **configures cleanly and fails four minutes later**, in the middle of a
translation unit, as a missing file. `scripts/build-host.sh` puts that question to the
compiler before it spends the minutes.

### The build

Same shape as the Windows one, and the sidecar rule is the same rule: `tauri-build` checks
`bundle.externalBin` in its build script, so both binaries have to be on disk before anything
compiles `dile-app` — `cargo clippy` and `cargo test` included.

```bash
scripts/build-host.sh --cpu --debug

cargo fmt --all
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace
cargo tauri build --debug --no-bundle
```

`scripts/build-host.sh` is `scripts/build-host.ps1` in shell spelling: `--cpu` is `-Cpu`,
`--debug` is `-DebugBuild`, and both PowerShell forms are accepted too. It copies the two
sidecars into `crates/dile-app/binaries/` under the host triple and without a `.exe`, which
is the name `bundle.externalBin` looks for here.

**The GPU check is the mirror of the Windows one.** There is no SDK on Linux and therefore no
`LIB` to set; the three pieces are distribution packages, and the script asks pkg-config and
`PATH` for them before spending the minutes rather than letting CMake say `Could NOT find
Vulkan (missing: Vulkan_LIBRARY Vulkan_INCLUDE_DIR glslc)` at the end of one.

**`--cpu` is what CI runs and what it is for.** It exercises the configuration without a
Vulkan toolchain on the runner, and the binary it produces answers `hello` without the
feature — which the application and `dile transcribe` both read, so a CPU-only host is
recorded as exactly that rather than as a driver that failed. It is **not** what a release is
built with here any more: see "The two tiers on Linux, measured" below for the numbers that
changed that, and `scripts/build-installer.sh --cpu-engine` for a package built the old way.

### The packages

```bash
scripts/build-installer.sh
```

`scripts/build-installer.ps1` in shell spelling, and the same five steps in the same order:
the tray check, the remapped environment, the sidecars, `cargo tauri build`, and then a check
that reads the binaries back. It prints the three programs and both packages with their sizes
and each package's SHA-256, which is what `docs/RELEASE.md` asks to be kept.

`crates/dile-app/tauri.linux.conf.json` is the configuration: `bundle.targets` is `deb` and
`rpm`, the Tauri CLI merges the file over `tauri.conf.json` on this platform, and `nsis` stays
where it is for Windows. `cargo tauri build --debug` produces both under `target/debug/bundle/`
without any of the release machinery, which is the quick way to look at what a package
contains.

What goes in:

| Path | What |
|---|---|
| `/usr/bin/dile-app` | the tray application |
| `/usr/bin/dile-engine-host` | the engine, in its own process |
| `/usr/bin/dile` | the command line |
| `/usr/lib/Dile/` | `LICENSE.txt` and `THIRD-PARTY-NOTICES.md` |
| `/usr/share/applications/Dile.desktop` | the launcher entry, with `StartupWMClass=dile-app` |
| `/usr/share/icons/hicolor/{32x32,128x128,256x256@2}/apps/dile-app.png` | the icon, three sizes |
| `/usr/lib/udev/rules.d/70-dile-input.rules` | the keyboard rule, reloaded by the post-install script |

**The tray check is first because it decides a dependency name.** `tauri-cli` writes the
appindicator dependency from what pkg-config can see *on the building machine*: with
`ayatana-appindicator3-0.1` present the packages name the maintained library, and without it
they quietly name the 2018 `libappindicator3-1` instead. Nothing fails and nothing is printed.
So the script stops rather than warns, and names the package to install. `--legacy-tray`
builds one anyway for trying the packaging, and says at the end that it is not a release.

**The path check is the other part worth knowing about.** Every `panic!`, `unwrap()` and
`GGML_ASSERT` compiles the source file it came from in as a string literal, and `strip = true`
does not touch those — so a release build can carry the path every crate was compiled from,
which here is `/home/<account>`. Measured on this tree, on the **debug** binaries, which is
what an unremapped build looks like: **8,884** matches across the three programs, none of them
visible to any grep over the working tree.

Two different fixes, because there are two compilers:

| Half | Flag | Where |
|---|---|---|
| Rust | `--remap-path-prefix`, three prefixes | `CARGO_ENCODED_RUSTFLAGS`, set by the script |
| C and C++ (ggml, through CMake) | `-ffile-prefix-map=` | `CFLAGS` / `CXXFLAGS`, set by the script |

`scripts/check-binary-paths.sh` is the proof that both still work, and `build-installer.sh`
**deletes the bundle** if either fails. It reads one encoding rather than the Windows script's
three: an ELF binary has no resource section, so every string in it is UTF-8. Run it on its
own whenever something has touched `target/release`:

```bash
scripts/check-binary-paths.sh
scripts/check-binary-paths.sh --profile debug   # the control: a build with no remapping
```

The same trap applies as on Windows: **cargo does not rebuild the native library when
`CFLAGS` changes**, because `transcribe-cpp-sys` declares no `rerun-if-env-changed` for it. A
tree that already built ggml without the map keeps what it has, the check goes red naming
`ggml`, and the fix is `cargo clean -p transcribe-cpp-sys --release` and another run.

**The packages carry the Vulkan engine host**, and did not until 2026-09-17. Both halves of
the old reasoning went at once: Linux now probes the GPU tier like Windows, so a package
without a Vulkan host would be a probe with nothing to probe. It costs about 38 MB of
compiled shaders in the engine binary and a hard `libvulkan.so.1`, which is why
`tauri.linux.conf.json` names the loader in `depends` — a machine without one refuses the
package rather than installing an engine that cannot start. `scripts/build-installer.sh
--cpu-engine` builds the old shape for a machine or a distribution that does not want that
dependency; the probe records a CPU-only host as exactly that, never as a driver that failed.

**No AppImage.** Tauri's AppImage bundler downloads `appimagetool` and a copy of `patchelf` at
bundle time and rewrites the binary's interpreter path; it is a second packaging format with a
second set of failure modes, for an audience that `deb` and `rpm` already cover. It can be
added when there is somebody to add it for.

**No Flatpak, and that one is not a preference.** A Flatpak sandbox does not have `/dev/input`
or `/dev/uinput`, and both are what the trigger is built on — hold-to-talk on a lone right
Ctrl exists at all because Dile reads evdev rather than asking the desktop for a shortcut. A
Flatpak would have to reach the keyboard through the GlobalShortcuts portal instead, where
there is no release event, which turns hold-to-talk into toggle and makes it a different
product with the same name. `--device=all` would open the devices and would also be a
sandbox that grants everything, which is not a thing to ask a person to accept. It is a
separate piece of work with its own design question, not a target to add to this list.

### Where things live

`crates/dile-client/src/paths.rs` had the XDG branch written before any of this ran, and it
is the one that answers here:

| | Path |
|---|---|
| Settings, `engine.json` | `$XDG_CONFIG_HOME/io.github.xfurqan0.dile/`, else `~/.config/io.github.xfurqan0.dile/` |
| Models | `$XDG_DATA_HOME/io.github.xfurqan0.dile/models/`, else `~/.local/share/io.github.xfurqan0.dile/models/` |

`DILE_MODEL_DIR` overrides the second one in a debug build, which is how a machine that
already holds these weights avoids downloading a second copy.

### The two tiers on Linux, measured

**Linux probes the GPU on first start, exactly as Windows does, and the packages carry an
engine that can answer the probe.** That reverses the port's own first decision, which was
that Linux would put nobody on the Vulkan tier without being asked. The reason it was taken —
whisper.cpp's open, unfixed `DeviceLost` fault on Intel integrated graphics — was a report
about somebody else's machine, and the way to settle a report about a driver is to run the
driver. So it was run.

Measured 2026-09-17 on the machine this port exists for: **i9-13900H, 6 P-cores and 8
E-cores (20 threads), Intel Iris Xe RPL-P on Mesa 26.1.8 (ANV, Vulkan 1.4), Fedora 44,
mains power, `powersave` governor with `balance_performance` EPP** — which is the Fedora
default and not a slow setting, `intel_pstate` reporting turbo on and no frequency cap. The
engine host is spoken to over its own wire with the model already loaded, which is the state
the tray keeps it in, so these are what a dictation costs and not what a cold start costs.

| 3.00 s clip, model loaded | median | spread over the runs | transcript |
|---|---|---|---|
| **Vulkan tier — `ggml-large-v3-q5_0`** | **3.68 s** | 3.67 – 3.69 s | correct |
| Vulkan, the CPU tier's smaller `turbo-q5_0` | 2.50 s | 2.48 – 2.51 s | correct |
| **CPU tier — `turbo-q5_0`, 20 threads** | **14.6 s** | 8.4 – 26.0 s | correct |

**Those are full-window figures, and they are no longer what a dictation costs.** Everything
in this table ran the encoder over a thirty-second window whatever the clip was; the section
below shortens that window to the recording and takes the same three-second clip to **1.38 s**
on the tier that ships. The rows stay because they are what decided the tier, and because the
*ratio* between them is what the shortening does not change.

**The number that decided it is the third column.** The Vulkan tier is four times faster than
the CPU tier, and it is *steady*: twelve consecutive transcriptions landed inside 20 ms of
each other, where the CPU tier moved by a factor of three between runs of the same clip on
the same machine in the same ten minutes. That laptop is power-limited rather than thermally
limited — twenty threads of AVX2 GEMM pull the cores down to about 2 GHz against their 5.2 —
so what the CPU tier costs depends on what else the machine is doing, and a dictation tool
runs while somebody is working. The Vulkan tier was measured through the same interference
and moved by 30 %.

**Thirty runs, no `DeviceLost`.** Twelve of them back to back in one process, and the same
clip put through **upstream whisper.cpp's own Vulkan backend** on the same GPU lands within
30 ms of the figure above — so the runtime's Vulkan path is not an outlier, and neither
number rests on the other. The fault the tier was held back over did not reproduce on this
driver, and the transcript was
`Bugün hava çok güzel ve deniz sakin.` — the clip's sentence, exactly — every single time,
with and without a dictionary in the initial prompt. **The prompt is free on this tier**: 61 ms
of the 3.68 s, and the same text out.

Nothing here makes the tier a promise on hardware nobody has measured, and nothing needs to:
**the probe is per machine.** A driver that returns garbage, hangs, or takes the engine
process down drops that machine to the CPU tier and the answer is written to `engine.json`
once, which is the whole arrangement `docs/PROJECT.md` §3 built for Windows.

#### Where the time goes, and why a four-word sentence costs what a paragraph costs

`TRANSCRIBE_PERF_DEBUG=1` on the engine host prints the stages. For the same 3 s clip:

| stage | CPU tier, 20 threads | Vulkan tier |
|---|---|---|
| **encoder** | **13,484 ms — 92.6 %** | **2,977 ms — 80.1 %** |
| cross-attention KV | 229 ms | 186 ms |
| the prompt | 84 ms | 61 ms |
| 13 decode steps | 723 ms | 460 ms |
| total | 14,567 ms | 3,715 ms |

**The encoder is the whole bill, and the encoder does not know how long the clip is.**
Whisper pads every short-form request to a 30-second mel window — `transcribe.cpp`'s
`arch/whisper/model.cpp` pads the PCM to `fe_n_samples` (480,000) and produces exactly
`fe_nb_max_frames` (3,000) mel frames — so before this was fixed, a 2 s clip, a 3 s clip and
a 10 s clip all ran 1,500 encoder positions and all cost the same.

**Dile now shortens that window to the recording**, which is whisper's `audio_ctx` (`-ac`).
`transcribe-cpp` 0.2.3 does not expose it, 0.2.3 is the newest version published, and the
patch that would add it was proposed upstream and **declined** — so the runtime is carried as
a fork in this repository (`vendor/transcribe-cpp/`, and `VENDOR.md` beside it). The width is
chosen by `dile_engine::audio_ctx_for`, a pure function of the sample count:

```
audio_ctx = clamp(ceil(100 × seconds), 640, 1500)
```

Measured on the machine at the top of this section, engine host over its own wire with the
model loaded, three runs per row, the same recordings in both columns:

| take | full window | shortened | window used | |
|---|---|---|---|---|
| 2.0 s | 3.03 s | **1.38 s** | 640 | 2.20x |
| 3.0 s | 3.03 s | **1.38 s** | 640 | 2.19x |
| 5.0 s | 3.45 s | **1.78 s** | 640 | 1.94x |
| 10.0 s | 3.02 s | **2.13 s** | 1000 | 1.42x |
| 20.0 s | 3.46 s | 3.41 s | 1500 | 1.01x |
| 30.0 s | 3.44 s | 3.42 s | 1500 | 1.01x |

*Vulkan tier, `ggml-large-v3-q5_0`. On the smaller `turbo-q5_0`, the same GPU: 2.86 s → 0.95 s
at three seconds, 2.87 s → 1.65 s at ten.*

**The CPU fallback tier gets the same shape of win**, which matters because it is the tier a
machine whose driver fails the probe lives on — `turbo-q5_0`, 20 threads, two runs per row:

| take | full window | shortened | window used | |
|---|---|---|---|---|
| 2.0 s | 10.90 s | **4.55 s** | 640 | 2.40x |
| 3.0 s | 10.91 s | **4.37 s** | 640 | 2.50x |
| 5.0 s | 11.02 s | **4.85 s** | 640 | 2.27x |
| 10.0 s | 10.84 s | **6.79 s** | 1000 | 1.60x |
| 20.0 s | 10.89 s | 10.96 s | 1500 | 0.99x |
| 30.0 s | 21.91 s | 21.81 s | 1500 | 1.00x |

Two rows there are worth reading twice. **Twenty seconds is 0.99x** — the rule has already
asked for the full window, and the 1 % is this machine's own drift, not a cost. And **thirty
seconds is twice the price of twenty on the CPU tier and the same price on Vulkan**: that is
the decoder, not the encoder. The thirty-second fixture holds fourteen sentences to decode
where the twenty-second one holds nine, and decode steps are what the CPU is slow at.

**Both of the port's targets are met**, where before only the second was: a three-second
dictation under a second and a half, a ten-second one under four. A thirty-second take is
unchanged by construction — at thirty seconds there is no padding to drop, and the rule has
already given up and asked for the full window.

**The rule is the design, and a fixed number could not have shipped.** Shortening the window
puts the decoder into a repeat loop, which trips `compression_ratio_thold`, which starts the
temperature fallback ladder — and the time saved in the encoder comes back multiplied. All of
these are measured on this machine, on the tier that ships:

| forced window | 3 s clip | 9 s clip, two Turkish sentences |
|---|---|---|
| 128 | 22–51 s, the sentence seven times | — |
| 192 | 40 s, the sentence four times | — |
| 256 | 0.88 s, correct | — |
| 512 | 1.13 s, correct | **35.0 s**, and a hallucinated subtitle credit |
| 603 — 67 positions per second | — | 3.5 s, the second sentence repeated |
| **640 — the floor** | 1.38 s, correct | **2.2 s, correct** |
| 1500 — the full window | 3.03 s | 4.4 s |

Two numbers in that table are the whole policy. **A hundred positions per second** is twice
what one second of audio occupies, and the doubling is the margin: sixty-seven per second is
what the first round of research arrived at and it is *inside the broken band* here. **Six
hundred and forty** is the floor, and it is wider than a short take needs — three seconds is
correct at 256 — because the failures found were not near the point where the window stops
holding the audio. Forced to 512, a five-second recording of the same sentence twice came
back with the sentence **once** on one of the three model-and-device combinations tried and
correct on the other two. A dropped sentence that shows up on one backend only is exactly the
bug that cannot be reproduced from a report, so the floor sits at the narrowest width that was
clean everywhere.

**What the quality gate says, including the part that is not clean.** Eleven recordings, two
models, both devices, every run compared against the same recording through the full window:

* On the **real recording** — the committed Turkish probe clip and the fixtures built from it
  — the transcript is **byte-identical with the window shortened and not, 6 of 6 clips**, on
  all three model-and-device combinations, and both configurations are deterministic across
  separate processes.
* On a **synthetic** fixture (espeak-ng Turkish, which the model is already guessing at:
  baseline WER 0.13 – 0.29) the shipped tier differs on 2 of 5 clips, and **both differences
  are the shortened window being more correct** — it writes *demlenmiş* where the full window
  writes *memlenmiş*. Mean WER 0.205 → 0.176; on `turbo-q5_0` every row is identical, on both
  devices.
* **On the CPU fallback tier: 11 of 11 identical**, real and synthetic alike.

That is a better result than the honest expectation, and it is not a guarantee: the window is
an input to the encoder, so on audio the model is unsure about it can move a marginal token
either way. `DILE_AUDIO_CTX=0` puts the full window back on any build, without a new binary.

**A related thread, left deliberately untouched.** `run_options` pins `temperature` at 0 and
says nothing about `temperature_inc`, so the runtime's own default of 0.2 stands and the
fallback ladder is armed. That is a recovery layer, not a bug, and the stage counters above
show it **did not fire** on any clip measured here — `chunks=1 encs=1 crosses=1 prompts=1
steps=13`, one clean pass. Turning it off would be a change to what happens on the recordings
that need it, so it wants its own measurement on real dictation first.

**OpenBLAS does not help here, and the measurement above is why.** `transcribe.cpp`'s README
advertises it as a large win on the decode path, and that is true of the architectures whose
decoders call `cblas_sgemv` directly — parakeet, gigaam. Whisper's decoder goes through ggml
like everything else; the only `cblas` call on this path is the mel filterbank matmul in
`src/transcribe-mel.cpp`, and the mel does not appear in the table above because it is below
the noise of a 3.7-second measurement. `TRANSCRIBE_USE_SYSTEM_BLAS` is already `ON` by
default in the vendored CMake, so nothing had to be turned on to find this out — and on
Fedora the probe fails anyway, because `find_package(BLAS)`'s link check does not pass and
`cblas.h` lives under `/usr/include/openblas/` rather than where `check_include_file` looks.
Both facts point the same way: **there is no `--blas` switch on `build-host.sh` because there
is nothing for it to buy.**

#### What a cold start costs, on top

The numbers above are the warm path, which is the one the tray runs: the engine process is
started once and keeps its weights. `dile transcribe` is the cold path and pays three things
the tray does not.

```text
$ dile transcribe clip-3s.wav --json --yes          # Vulkan tier
  wall 6.3 – 6.7 s     took_ms 5,345 – 5,705
```

* **~0.9 s** of process start, model load (`mmap` of a gigabyte, warm page cache) and reading
  the WAV. The protocol itself is not in it: over the wire, a transcription's wall time and
  the engine's own `took_ms` agree to within 10 ms, samples included.
* **~1.7 s** on the *first* transcription in a Vulkan process, while the driver builds its
  pipelines. The second one in the same process is back to 3.7 s, which is why this is a
  start-up cost and not a per-dictation one.

#### How many threads, and why it is not the runtime's own answer

**`0` does not mean "use this machine".** `transcribe-cpp`'s default is the number of CPUs the
process may run on *capped at eight* — `src/transcribe-batch-util.h`,
`default_n_threads(int cap = 8)` — so everything above eight hardware threads sits idle. That
cap is not read out of a header here, it is **observed**: `0` and `8` produce the same figure
to within a tenth of a second on a machine that has twenty threads.

Three runs of each configuration against `probe.wav`, spoken to the engine host over its own
wire so that the only thing changing is the number:

| threads | median | real-time factor against the 30 s window |
|---|---|---|
| `0`, as the runtime would have it | 28.3 s | 0.94 |
| 8 | 28.3 s | 0.94 |
| 10 | 23.1 s | 0.77 |
| 14 — the physical cores | 17.5 s | 0.58 |
| **20 — every hardware thread** | **15.0 s** | **0.50** |
| 32 — more than the machine has | 25.7 s | 0.86 |

So `dile_engine::default_threads` asks for **every hardware thread the machine reports**,
capped at 32, and `dile-engine-host` resolves a `threads: 0` request to it on the CPU tier.
A dictation on this laptop costs **14.2 s where it used to cost 28.3 s**, measured end to end
through the same wire the application uses.

Three rows of that table decided the policy:

* **Logical, not physical.** The eight hyperthreads on top of the fourteen physical cores were
  worth another 14 %. Small, but the right sign — and reading a physical core count takes a
  different piece of platform code on every platform, for a number that measured *worse*.
* **Never more than the machine has.** Thirty-two threads on a twenty-thread machine measured
  slower than the runtime's own cap; oversubscription gives the win straight back. The 32
  ceiling in `MAX_THREADS` exists for the other end of the range, where the gain was already
  sublinear at twenty.
* **The Vulkan tier keeps its `0`.** Almost nothing runs on the CPU there and none of the above
  was measured on it, so the host leaves it alone. That is the only reason the resolution is
  device-aware.

**These numbers were taken on battery, and the plugged-in machine did not reproduce them.**
The table above is a 14-core, 20-thread i9-13900H under the `powersave` governor on battery,
idling at 645 MHz against a 5.4 GHz maximum, and on that machine more threads monotonically
won. On mains power the same laptop is **power-limited rather than starved**: twenty threads
of AVX2 GEMM pull every core down to about 2 GHz, and re-measuring on 2026-09-17 with the
configurations interleaved — one run of each per pass, the order reversed on alternate
passes, so drift lands on all of them — six threads and twenty threads did **not** separate
at all. Their medians were 18.6 s and 16.2 s with single runs ranging from 14.3 s to 38.0 s,
which is noise of the same size as the effect being looked for.

**So the policy stands as measured and is not re-decided on worse evidence.** What the second
measurement changes is the confidence attached to it: on this hardware, on mains power, the
thread count is not the lever. `TRANSCRIBE_PERF_DEBUG` says why — 92.6 % of a CPU-tier
dictation is one encoder pass over a fixed 1,500-position window, and the two things that
move that are the device it runs on and how many positions it runs, in that order.

Weights live where `crates/dile-client/src/paths.rs` said they would before any of this ran —
`~/.local/share/io.github.xfurqan0.dile/models/` — and the downloader reaches Hugging Face
over rustls with no system OpenSSL involved. A machine that already holds a copy points
`DILE_MODEL_DIR` at it in a debug build.

### Keyboard access on Linux

**The trigger does not work until this is done, and it is one command.** It is also the one
thing on this page worth reading before running: it grants something real.

Dile watches for the trigger at the kernel's input devices instead of asking the desktop for a
shortcut. That is not a shortcut of its own — it is the only way hold-to-talk exists on
Wayland at all. A compositor will not hand a lone modifier to an application (mutter rejects a
modifier-only accelerator outright, and the shortcuts specification has no way to say "the
*right* Ctrl"), and the desktop shortcut registries it offers instead run a command with no
release event, which turns hold-to-talk into toggle. Reading evdev needs neither an X11 nor a
Wayland connection, keeps the two sides of Ctrl apart, and behaves the same on a bare console.

Two device nodes, one permission:

| Node | What it is for |
|---|---|
| `/dev/input/event*` | seeing a key go down and come back up |
| `/dev/uinput` | *swallowing* one. Blocking a key means grabbing the keyboard and re-injecting everything that is not the trigger through a clone, and the clone is a uinput device. |

Dile asks for the second even though the shipped trigger — the right Ctrl, alone — blocks
nothing: the review card adds Enter, Esc and `Ctrl+C` to the same set while it is on screen,
and an Enter that transfers the text and *also* lands in the document behind it is exactly the
bug that set prevents. `handy-keys` checks uinput before it scans, so a machine with only half
the permission fails on that half. The same node is what the experimental auto-paste writes
to, so a machine where the trigger works is a machine where that works too.

**A package does this for you**; these two commands are for a source build, and they are what
the package's post-install script runs:

```bash
sudo install -m 0644 packaging/linux/70-dile-input.rules /etc/udev/rules.d/
sudo udevadm control --reload && sudo udevadm trigger
```

Immediate, and no logging out — from a package, a session that was already open may need one,
because the rule hands the devices to whoever is logged in at the seat.

**What it grants, in plain words.** The rule tags those devices `uaccess`, so logind hands
them to whoever is logged in at the seat as an ACL and takes them back at logout. While you
are logged in at this machine, a program running as you can read the keyboard whatever window
has the focus — which is the door Wayland closes on purpose, reopened for the local seat. It
is the trade every push-to-talk tool on Linux makes. Dile's side of it is that the audio and
the text never leave the machine.

**Not `usermod -aG input $USER`.** `handy-keys` suggests it and the internet suggests it, and
it is the worse of the two: group membership grants every process that account ever starts —
an SSH session from somewhere else included — the ability to read every keystroke on the
machine, permanently, whether or not anybody is sitting at it. The `uaccess` rule is narrower
in both directions and takes effect faster.

`packaging/linux/70-dile-input.rules` carries the same explanation, and the number matters:
`uaccess` tags are collected by `73-seat-late.rules`, so anything numbered above that is read
too late to do anything.

**Check it without starting the application:**

```bash
cargo run -p dile-hotkey --example hotkey_probe -- --seconds 30 --trigger RightCtrl
```

Before the rule, it stops on the sentence that names the rule. After it, it prints
`hook installed` and then a line per decision — hold the right Ctrl for a second and let go,
and `StartRecording` and `StopRecording` appear with the time between them. There is no layout
column as there is on Windows: evdev reports scancodes from below the layout, so the trigger
is the same physical key whichever one is selected.

Inside the application the same failure is a dialog rather than a log line, because a tray
application that exits before it has a tray has no other way to say anything.

### The experimental auto-paste

`Settings > Cleanup and transfer > Experimental: press Ctrl+V for me`, which is `paste.auto`
in `settings.json` and is **off** unless somebody turns it on. With it on, a finished
dictation goes on the clipboard exactly as before and then Dile presses `Ctrl+V` itself,
through a `uinput` keyboard it creates called `Dile virtual keyboard`.

**It is not a paste, and the difference matters before you turn it on.** The chord reaches
whatever holds the keyboard at that moment — Dile is not told which window that is, here or
anywhere else under Wayland — so it is *press the chord and see* rather than *put this
sentence in that window*. Two consequences worth knowing:

* **In a terminal, `Ctrl+V` is not paste.** Most terminal emulators paste on `Ctrl+Shift+V`
  and treat `Ctrl+V` as something else or as nothing. On Windows Dile picks the chord per
  application; picking it needs to know what the target is, and that is the one thing it
  cannot know here. The text is on the clipboard either way, so the fix is to paste it
  yourself.
* **The card goes away before the chord.** It has to: the card holds the keyboard while it is
  on screen, so a chord sent first would be typed into the card. So with this on you get the
  text and not the *copied* line.

It needs no permission the trigger did not already need — `/dev/uinput`, from the same udev
rule — and a machine without it says which of the two reasons it was, on the card, in one
line, with the dictation still on the clipboard:

| The card says | What it means |
|---|---|
| *No auto-paste here* | there is no `/dev/uinput`; the `uinput` kernel module is not loaded |
| *Auto-paste needs the udev rule* | the node is there and this user may not write to it |
| *Auto-paste did not work* | anything else, and the log line beside it says what |

The device exists only while Dile is running: the kernel destroys a `uinput` device when the
last handle on it closes, so quitting or crashing takes it with it. `libinput list-devices`
is where to look for it, and it has two keys on it — `Ctrl` and `V`.

### The tray on GNOME

GNOME has no built-in `StatusNotifierItem` host and no plan for one, so a tray icon is
registered on D-Bus successfully, reports no error, and appears nowhere. The extension is the
only answer:

```bash
gnome-extensions enable appindicatorsupport@rgcjonas.gmail.com
```

The application asks the session bus five seconds after start whether anything owns
`org.kde.StatusNotifierWatcher`, and sends one desktop notification when nothing does. One per
run, never fatal, and every step of it is a log line — an application that could not find out
whether it is visible is still an application that records and transcribes.

**The two `Gtk-CRITICAL` lines a debug console shows on GNOME are the same fault wearing a
disguise**, and they are upstream rather than this repository's:

```
Gtk-CRITICAL **: gtk_widget_get_scale_factor: assertion 'GTK_IS_WIDGET (widget)' failed
```

A backtrace puts them in `libappindicator3`: with no watcher on the bus it falls back on a
timer to `GtkStatusIcon`, which is the legacy X11 tray that GTK3 does not implement under
Wayland, and its image update asks a non-widget for a scale factor. They stop when the
extension is enabled, they are harmless when it is not, and they are why the notification
above says in words what these say in glyphs. The successor library,
`libayatana-appindicator3`, is the one the package list installs; the two conflict, and a
machine with the old one installed is what these lines came off.

## Troubleshooting

| Symptom | Cause |
|---|---|
| `LNK1181: cannot open input file 'vulkan-1.lib'` | `LIB` does not carry `%VULKAN_SDK%\Lib`. See above. |
| `cmake` not found, from inside a build script | CMake is not on `PATH`. On a fresh install, open a new shell — the installer sets the machine `PATH` and existing shells do not see it. |
| `unsupported timestamp granularity (status 12)` | Word timestamps were asked for. `transcribe-cpp` 0.2.3 gives segment granularity only; `dile-engine` asks for segments. |
| `TRANSCRIBE_ERR_GGUF` | The model file is not GGUF. Legacy `ggml-*.bin` files happen to load, but that is undocumented behaviour and nothing here relies on it. |
| The tray icon does not appear | Windows keeps new icons in the `^` overflow. Drag it onto the taskbar to pin it. |
