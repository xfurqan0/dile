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

There is **no frontend build step and no npm.** `ui/` is static markup that `frontendDist`
points at directly. WP5 writes the review panel in plain TypeScript with esbuild, the way
nazar-tray does, and `frontendDist` moves to `ui/dist` then — when there is something to
compile.

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

GitHub Actions cannot run on this repository at the moment — it is private and the free
minutes are gone — so the gate has to run here, before a push leaves the machine.
`scripts/ci-local.ps1` mirrors `.github/workflows/ci.yml`: the same steps, in the same order,
with the same environment, and the `audit` job's `cargo deny check` on the end.

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
cargo build -p dile-engine --features gpu-vulkan
```

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

## Troubleshooting

| Symptom | Cause |
|---|---|
| `LNK1181: cannot open input file 'vulkan-1.lib'` | `LIB` does not carry `%VULKAN_SDK%\Lib`. See above. |
| `cmake` not found, from inside a build script | CMake is not on `PATH`. On a fresh install, open a new shell — the installer sets the machine `PATH` and existing shells do not see it. |
| `unsupported timestamp granularity (status 12)` | Word timestamps were asked for. `transcribe-cpp` 0.2.3 gives segment granularity only; `dile-engine` asks for segments. |
| `TRANSCRIBE_ERR_GGUF` | The model file is not GGUF. Legacy `ggml-*.bin` files happen to load, but that is undocumented behaviour and nothing here relies on it. |
| The tray icon does not appear | Windows keeps new icons in the `^` overflow. Drag it onto the taskbar to pin it. |
