# Building Dile

Windows is the only platform v1 builds on. macOS and Linux come from the same codebase in
v2, and `dile-core` already builds everywhere because it has no platform in it.

## Prerequisites

| What | Why | Needed for |
|---|---|---|
| **Rust, stable, `x86_64-pc-windows-msvc`** | `rust-toolchain.toml` pins the channel and the components | everything |
| **Visual Studio Build Tools** with the *Desktop development with C++* workload | the MSVC linker, and the compiler that builds `transcribe.cpp` | everything |
| **CMake** ≥ 3.20 | `transcribe-cpp-sys` builds its native library from source through CMake | everything |
| **WebView2** | the panel's runtime. Part of Windows 10 1803+ and Windows 11; the installer downloads a bootstrapper if it is missing | running the app |
| **Vulkan SDK** ≥ 1.3 | the GPU backend's headers, `vulkan-1.lib` and `glslc`, which compiles about two thousand SPIR-V shaders | the `gpu-vulkan` feature only |

```powershell
rustup toolchain install stable-x86_64-pc-windows-msvc
cargo install tauri-cli --locked
```

A **CPU build needs only CMake and MSVC**, and that is the default cargo feature set, so
nobody has to install a graphics SDK to work on Dile. It is a *build* default and not the
product's runtime tier: `docs/PROJECT.md` §3 makes Vulkan the default tier behind a first-run
probe, with CPU as the fallback, so a release build enables `gpu-vulkan`.

## The normal build

```powershell
cargo fmt --all
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace
cargo tauri build --debug --no-bundle
```

Roughly three minutes cold, seconds warm. Nearly all of the cold time is
`transcribe-cpp-sys` compiling `transcribe.cpp`; nothing else in the workspace is large.

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

Hold **`Ctrl+Alt+Space`**, say a sentence that begins with a hard consonant, let go. The
console prints the session:

```text
[INFO  dile_app::session] hotkey hook installed: Hold mode, second key false
[INFO  dile_capture] input device "Mikrofon (PRO X)": 48000 Hz, 1 channel(s), f32
[INFO  dile_app::session] recording: 2.31 s, 36960 samples, speech 1340 ms from 0.62 s to 1.96 s
[INFO  dile_app::session] debug build: wrote %LOCALAPPDATA%\io.github.xfurqan0.dile\last.wav (2.31 s, speech true)
```

What to look at, in order:

| Check | What proves it |
|---|---|
| The chord never reaches the window | The editor behind the app gains no space. This is why the hook blocks. |
| No clipped first syllable | Play `last.wav`. The recording opens with half a second of the room, then the word — the pre-roll ring. |
| The tray follows the session | The accent bar goes red while the key is down and amber for the blink before the buffer is handed on; the tooltip says the same. |
| Silence is not dictated | Hold the key and say nothing: the log says *nothing heard*, no WAV is written, and the tray says so until the next press. |
| A tap does nothing | Press and release under 250 ms: the log shows the tap, and nothing is recorded. |
| The cap stops cleanly | Lower `cap_secs` in `crates/dile-app/src/config.rs`, hold past it, and the release still returns the capped buffer with `the recording cap stopped this one` in the log. |

`last.wav` is **debug builds only** and is overwritten every time. A release build does not
contain the code that writes it.

The two crates can also be exercised on their own, without the application:

```powershell
cargo run -p dile-hotkey --example hotkey_probe -- --seconds 30 --second-key
cargo run -p dile-capture --example record -- 5 2
```

The probe prints every decision with the **keyboard layout of the focused window** next to it,
which is how `docs/PROJECT.md` §3's "re-verified against IME in WP2" is actually answered:
switch to the Turkish layout and watch `layout=041F` appear on the line. It also blocks the
chord while it runs, and never the second key — `Ctrl+C` keeps working. The capture example
records five seconds with a two-second cap, so the cap fires on purpose, draws the level bar
at 20 Hz and writes `out.wav` next to you.

## Trying the panel

WP5b's acceptance criteria are all about a person holding a key and watching where the text
lands, so none of them is a test the suite can run. This is how they get checked. Everything
below is one `cargo tauri dev` and a Notepad.

```powershell
cargo tauri dev
```

**The loop.** Put the caret in a text field — Notepad will do — hold **`Ctrl+Alt+Space`**, say
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
$env:LIB = "$env:VULKAN_SDK\Lib;$env:LIB"
cargo build -p dile-engine-host --features gpu-vulkan
```

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
row covers development as well as an installation. **WP7 ships it as a Tauri sidecar**, which
puts it next to `dile.exe` in the installation directory — the same row again. The
application does **not** declare it as a build dependency, so `cargo tauri build` works
whether or not the engine host has been built; a missing one is a tray tooltip and a log line
that says which command builds it, not a broken build.

The two debug-only rows exist for the one case the first row cannot cover: a test binary,
which cargo puts in `target\debug\deps\`. That is why the round-trip test below wants
`CARGO_TARGET_DIR` set.

**CI does not build this.** The SDK is a ~250 MB download and an installer on every run, for
a feature that is opt-in at run time and that a runner with no GPU cannot exercise anyway.
The maintainer builds it locally, and WP7 revisits the question when the release workflow has
to produce a GPU build.

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

## Troubleshooting

| Symptom | Cause |
|---|---|
| `LNK1181: cannot open input file 'vulkan-1.lib'` | `LIB` does not carry `%VULKAN_SDK%\Lib`. See above. |
| `cmake` not found, from inside a build script | CMake is not on `PATH`. On a fresh install, open a new shell — the installer sets the machine `PATH` and existing shells do not see it. |
| `unsupported timestamp granularity (status 12)` | Word timestamps were asked for. `transcribe-cpp` 0.2.3 gives segment granularity only; `dile-engine` asks for segments. |
| `TRANSCRIBE_ERR_GGUF` | The model file is not GGUF. Legacy `ggml-*.bin` files happen to load, but that is undocumented behaviour and nothing here relies on it. |
| The tray icon does not appear | Windows keeps new icons in the `^` overflow. Drag it onto the taskbar to pin it. |
