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

`DILE_OPEN_PANEL` exists because there is no way to speak into a microphone from a script, so
"does the result state still lay out at two lines" is otherwise a question nobody can answer
from a terminal. The card waits for the page to load, then stays up: a preview **does not
count down and does not close itself**, and it deliberately does **not** arm the panel's three
keys — a preview that swallowed Esc for the whole machine while somebody was photographing it
would be a worse bug than the one it was helping to find.

`DILE_MODEL_DIR` is how a machine that already holds these weights avoids downloading a
second copy. The tier decision lives in `%APPDATA%\io.github.xfurqan0.dile\engine.json`;
delete it to make the next start probe again.

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
| Microphone, pre-roll, VAD, 16 kHz | works, through ALSA (which is also how cpal reaches PipeWire) |
| The engine, `dile transcribe`, model download | works; CPU tier |
| Tray icon and menu | works, and on GNOME needs the AppIndicator extension |
| Placing the card on a screen | **no** — `xdg-shell` has no global coordinate space |
| Naming the target application | **no** — a Wayland client is not told what has the focus |
| Clipboard and automatic paste | **no** — the text stays in the card to be taken by hand |

The application says each of those in the log at start-up rather than failing quietly.

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

Two more, for the GPU build only, and skipped by `--cpu`:

```bash
sudo dnf install vulkan-loader-devel vulkan-headers glslc    # Fedora
sudo apt install libvulkan-dev glslc                         # Debian and Ubuntu
```

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

**`--cpu` is the sane default here for a second reason.** `whisper.cpp` has an open, unfixed
`DeviceLost` fault on Intel integrated graphics under Mesa, so the CPU tier is the one a Linux
build should trust until the probe has said otherwise on a particular machine. The engine
runs in its own process precisely so that a driver fault takes the engine down and not the
tray, which makes this a slower tier rather than a broken application — `docs/PROJECT.md` §3.

### Bundles

`crates/dile-app/tauri.linux.conf.json` sets `bundle.targets` to `deb` and `rpm`; the Tauri
CLI merges it over `tauri.conf.json` on this platform, and `nsis` stays where it is for
Windows. `cargo tauri build --debug` produces both under `target/debug/bundle/`, each
carrying `dile-app`, `dile-engine-host` and `dile` in `/usr/bin`.

**No AppImage.** Tauri's AppImage bundler downloads `appimagetool` and a copy of `patchelf`
at bundle time and rewrites the binary's interpreter path; it is a second packaging format
with a second set of failure modes, for an audience that `deb` and `rpm` already cover. It
can be added when there is somebody to add it for.

### Where things live

`crates/dile-client/src/paths.rs` had the XDG branch written before any of this ran, and it
is the one that answers here:

| | Path |
|---|---|
| Settings, `engine.json` | `$XDG_CONFIG_HOME/io.github.xfurqan0.dile/`, else `~/.config/io.github.xfurqan0.dile/` |
| Models | `$XDG_DATA_HOME/io.github.xfurqan0.dile/models/`, else `~/.local/share/io.github.xfurqan0.dile/models/` |

`DILE_MODEL_DIR` overrides the second one in a debug build, which is how a machine that
already holds these weights avoids downloading a second copy.

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
the permission fails on that half.

```bash
sudo install -m 0644 packaging/linux/70-dile-input.rules /etc/udev/rules.d/
sudo udevadm control --reload && sudo udevadm trigger
```

Immediate, and no logging out.

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

### The tray on GNOME

GNOME has no built-in `StatusNotifierItem` host and no plan for one, so a tray icon is
registered on D-Bus successfully, reports no error, and appears nowhere. The extension is the
only answer:

```bash
gnome-extensions enable appindicatorsupport@rgcjonas.gmail.com
```

## Troubleshooting

| Symptom | Cause |
|---|---|
| `LNK1181: cannot open input file 'vulkan-1.lib'` | `LIB` does not carry `%VULKAN_SDK%\Lib`. See above. |
| `cmake` not found, from inside a build script | CMake is not on `PATH`. On a fresh install, open a new shell — the installer sets the machine `PATH` and existing shells do not see it. |
| `unsupported timestamp granularity (status 12)` | Word timestamps were asked for. `transcribe-cpp` 0.2.3 gives segment granularity only; `dile-engine` asks for segments. |
| `TRANSCRIBE_ERR_GGUF` | The model file is not GGUF. Legacy `ggml-*.bin` files happen to load, but that is undocumented behaviour and nothing here relies on it. |
| The tray icon does not appear | Windows keeps new icons in the `^` overflow. Drag it onto the taskbar to pin it. |
