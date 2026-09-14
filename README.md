# Dile

<img src="assets/dile.png" alt="The Dile icon: seven rounded waveform bars on a dark rounded square, the middle and tallest one green" width="128">

**Hold a key. Speak. Let go.** Local, Turkish-first dictation for Windows. What you said
appears in a small panel at the top of the screen, then lands in whatever you're typing in —
with your own clipboard put back afterwards. Fix a word, copy it, or cancel before it does.

```powershell
winget install xfurqan0.dile
```

Windows 10 1809 or newer. Installs per user, no administrator rights. The winget package is
reviewed a day or two after each release, so until it resolves the installer is on the
[Releases](https://github.com/xfurqan0/dile/releases) page.

<!-- A GIF of one dictation — hold, speak, the card, the paste — goes here. It needs a screen
     recording, which is a thing a person makes rather than a thing a build produces;
     docs/RELEASE.md step 10 keeps it on the list. Until then the loop is described in words
     under "The loop" below. -->

## Why another dictation app

There are excellent local dictation tools. Dile exists for one reason: **Turkish.**

- **What Turkish actually comes back wrong is the terms and the punctuation, not the fillers.**
  Measured on Dile's own set, no supported engine writes a vocalised "eee" at all. So the
  answer v1 ships is a dictionary that works in both directions — fed to the recognizer before
  it decodes, and used to correct the spelling afterwards. On the 20-sentence command set that
  took word error from **0.379 to 0.261** and technical-term recall from **17 of 31 to 30 of
  31**, with no language model anywhere in the loop.
- **Dictionaries in other tools silently drop words with `ç ğ ı ö ş ü`.** Dile's keeps them
  byte for byte — the round trip is tested letter by letter — and keeps the Turkish suffix
  when it corrects: `keş'i` becomes `cache'i`.
- **The model is chosen by measured Turkish accuracy on Dile's own test set, not by a model
  card** — and it is chosen: v1 ships **stock Whisper large-v3 `q5_0`** with that dictionary
  prompt. No fine-tune, because no Turkish fine-tune has beaten the stock weights on our
  material yet.
- **Turkish `ı/İ` casing, three cleanup strictness levels, deterministic punctuation repair
  and a hallucination-phrase filter**, from a real 230-cut edit log. The cleanup runs no model
  and opens no socket: it is a fixed pipeline of rules, and every rule says in its own source
  which sentence of the spec it implements.

English works too. Turkish just comes first.

## The loop

Put the caret wherever you are typing. Hold the **right Ctrl**, say a sentence, let go.

1. **Recording.** A card appears at the top of the monitor the focused window is on, with a
   level meter that moves with your microphone and the elapsed time against the 60 s cap. The
   key itself is never swallowed — `Right Ctrl` + `C` still copies — and pressing anything
   else while you hold it withdraws the recording. A chord, if you set one instead, *is*
   swallowed, so no space reaches the editor behind it. Esc throws the recording away.
2. **Working.** An honest line — the model, the tier, how much audio — rather than a blank
   spinner.
3. **Ready.** The cleaned text, with a thin countdown line under it. Do nothing and it is in
   your window **2.5 s** later, with your own clipboard put back. Or:
   - **edit it in place** and press Enter — what lands is your edit, not what the engine said;
   - **Copy** (`Ctrl+C`) puts it on the clipboard without pasting, and deliberately does not
     restore the clipboard afterwards, because you asked for it;
   - **Esc** cancels, and nothing was ever put on your clipboard;
   - the **light / medium / strict** pills re-run the cleanup instantly, and **raw** shows what
     the engine actually wrote;
   - the countdown stops if you press ✗, if you edit, or if you simply rest the pointer on the
     text for a third of a second — because that is what reading looks like.

**The panel never takes the focus.** The editor behind the card keeps its caret, its selection
and its focus ring, which is why Enter, Esc and `Ctrl+C` are read from a global keyboard hook
while the panel is up. The card says where the text is going — "→ VS Code", "→ Windows
Terminal" — and the window it is aimed at is captured when the recording *starts*, so a window
that closes in the meantime takes the paste with it rather than letting the text land
somewhere else.

Drag the card and it stays there, per monitor.

## First run

Dile starts with nothing but a tray icon, and asks before it does anything expensive.

- **The model.** A dialog, in your language, before a single byte: the file, its size, and
  where it comes from. *Not now* leaves the application running — the hotkey still records and
  the tray says there is no model. The download resumes if it is interrupted, and the file is
  checked against a sha256 compiled into the application before it takes its real name.
- **The GPU probe.** Dile transcribes two seconds of Turkish whose answer it already knows, in
  the engine process, and only a device that returns enough of it is used. Half the sentence
  is the bar, and diacritics are not folded: a GPU that writes `guzel` for `güzel` has lost the
  point of the product. The answer is written down **once per machine**, not paid for on every
  start.
- **If the probe fails**, the machine gets the CPU tier with a smaller model, and the tray says
  *CPU tier (GPU probe failed)* rather than quietly running ten times slower.

The clip the probe uses is Windows' own Turkish text-to-speech voice, committed to this
repository. No recording of a person ships here.

## What it costs to run

Measured on the maintainer's machine — RTX 5060, Vulkan SDK 1.4.357 — on Dile's own test
material. One machine, so read these as the shape rather than as a benchmark:

| | Vulkan tier (default) | CPU tier (fallback) |
|---|---|---|
| Model | `ggml-large-v3-q5_0.bin`, 1,081 MB | `ggml-large-v3-turbo-q5_0.bin`, 574 MB |
| Word error, with the dictionary prompt | 0.261 | 0.348 |
| Technical terms found, of 31 | 30 | 28 |
| Transcription, p95 on the 20-sentence set | 2.38 s | — |
| Real-time factor | 0.288 | ~2.3 — slower than real time |
| Model load, warm | ~0.9 s | — |

The countdown starts after the text is there, so what you actually wait for is the
transcription time alone.

## Settings

From the tray menu, and **everything applies while Dile is running** — no restart, ever.

- **Hotkey.** The trigger, and hold or toggle. The trigger is one key held on its own — the
  right Ctrl by default — or a chord such as `Ctrl+Alt+Space`; press Change and it listens for
  ten seconds and takes whichever of the two you press. A lone key is hold-to-talk in either
  mode, because a recording latched to a key your hand rests on has no way out of it.
  `Ctrl+Space` is refused with the reason (an IME swallows it before any hook sees it), and so
  is a chord with no modifier — a bare key as a global hotkey takes that key away from the
  whole machine.
- **Recording.** Microphone, the cap in seconds, and how much audio from before the key went
  down is kept so the first syllable is never clipped.
- **Cleanup and transfer.** Strictness, auto-transfer on or off, and how long the countdown
  runs (500–5000 ms).
- **Engine.** The tier this machine decided on, what the probe saw, a button to run it again,
  and an override.
- **Dictionary.** A table: canonical spelling, variants, pinned, delete, add a row. Two entries
  claiming the same term are refused where they were typed, under Turkish casing, so `Cron` and
  `cron` are one claim.
- **General.** Interface language (English, Türkçe, or follow Windows) and start with Windows.

Everything lives in `%APPDATA%\io.github.xfurqan0.dile\settings.json`, which you can read and
edit. A value out of range is clamped with a line in the log rather than rejected, and a file
from an older or newer build still loads — losing a dictionary to a downgrade is not an
acceptable failure.

## The dictionary is the accuracy feature

It is not a correction list. The entries become the **decoder's initial prompt**, capped at 180
characters, pinned first and then most recently added — which is what took word error from
0.379 to 0.261. The same entries are then used to repair whatever still came back wrong. Type a
term in the settings window and it is in the next dictation's prompt, even if the last one is
still being transcribed.

**The shipped dictionary is empty**, because every entry is a claim about one person's
vocabulary.

## The command line

```powershell
& "$env:LOCALAPPDATA\Dile\dile.exe" transcribe take.wav
& "$env:LOCALAPPDATA\Dile\dile.exe" transcribe take.wav --json
```

One command. It reads the same `settings.json`, runs the tier this machine already decided on
and loads the same weights, so it is the product rather than a second one: your dictionary is
in its prompt and your strictness decides its cleanup.

```json
{
  "raw": "Bugün hava çok güzel ve deniz sakin.",
  "cleaned": "Bugün hava çok güzel ve deniz sakin.",
  "segments": [ { "start_ms": 0, "end_ms": 2040, "text": "Bugün hava çok güzel ve deniz sakin." } ],
  "took_ms": 307,
  "device": "Vulkan0",
  "model": "ggml-large-v3-q5_0.bin"
}
```

WAV only, any sample rate and channel count — it is converted by the same converter the
microphone path uses. It will not download a model without `--yes`; without it, it prints the
question and stops. A machine that has never run Dile has never been probed, and it says so
rather than choosing a tier of its own. **It is not added to `PATH` in v1**: use the full path
above, or add the directory yourself.

## Privacy

- **Audio is processed in memory, never written to disk and never sent anywhere.** A debug
  build writes the last recording next to the application's data so that "no clipped first
  syllable" is something a person can listen to; a release build does not contain that code.
- **No account, no telemetry.** True from the first commit. Nothing here opens a socket except
  the model download, and [CONTRIBUTING.md](CONTRIBUTING.md) refuses a dependency that does.
- **The only network access is Hugging Face**, once, for the weights, with your consent, at a
  pinned commit.
- **The clipboard restore is text only.** `CF_UNICODETEXT` comes back byte for byte; an image,
  a file list, rich text or a spreadsheet range on the clipboard before a dictation does **not**
  survive it. That is a v1 limit, written here rather than discovered.
- [SECURITY.md](SECURITY.md) is the longer version: what Dile reads, what it never reads, and
  how to report something.

## Unsigned, and what to check instead

Dile 0.1.0 is not code-signed. A browser download gets a SmartScreen warning (*More info → Run
anyway*); `winget install` does not go through it. Every release carries a `SHA256SUMS` file
and a GitHub build attestation, which is the stronger of the two — it is signed by GitHub and
names the workflow run and the commit that produced that exact file:

```powershell
gh attestation verify .\Dile_0.1.0_x64-setup.exe --repo xfurqan0/dile
```

A push-to-talk tool installs a low-level keyboard hook and opens a microphone, which is what
makes a signature worth more here than for most small applications — an endpoint-protection
product has nothing to attribute those two things to. The plan, the alternatives, and why the
application to SignPath Foundation comes *after* the first release, are in
[docs/CODE_SIGNING.md](docs/CODE_SIGNING.md).

## The speech models

Dile ships no weights. It downloads one of two files on first run, from
[`ggerganov/whisper.cpp`](https://huggingface.co/ggerganov/whisper.cpp) on Hugging Face, pinned
to commit `5359861c739e955e79d9a303bcbc70fb988958b1` and verified against a sha256 compiled
into the application:

| Tier | File | Size | Source |
|---|---|---|---|
| Vulkan (default) | `ggml-large-v3-q5_0.bin` | 1,081 MB | [download](https://huggingface.co/ggerganov/whisper.cpp/resolve/5359861c739e955e79d9a303bcbc70fb988958b1/ggml-large-v3-q5_0.bin) |
| CPU (fallback) | `ggml-large-v3-turbo-q5_0.bin` | 574 MB | [download](https://huggingface.co/ggerganov/whisper.cpp/resolve/5359861c739e955e79d9a303bcbc70fb988958b1/ggml-large-v3-turbo-q5_0.bin) |

Both are quantizations of **OpenAI's Whisper**, which is
[MIT licensed](https://github.com/openai/whisper/blob/main/LICENSE); the repository they come
from is MIT as well. They are kept in `%LOCALAPPDATA%\io.github.xfurqan0.dile\models\`, and
those two links are also how to fetch them by hand on a machine that cannot reach them from
inside the application.

## Known limits

- **Windows only.** macOS and Linux come from the same codebase in v2.
- Dictation, not transcription. Recording is capped at 60 s by default and 300 s at most;
  meeting recordings, file batches and subtitle files are deliberately out of scope.
- The CPU fallback tier is slower than real time. It is a fallback, not a mode to choose.
- The clipboard restore is text only (above).
- Unsigned (above).
- Uninstalling silently — which is what `winget uninstall` does — keeps your settings and the
  downloaded model. Remove them with
  `Remove-Item -Recurse "$env:APPDATA\io.github.xfurqan0.dile", "$env:LOCALAPPDATA\io.github.xfurqan0.dile"`.
- Windows 11 keeps new tray icons in the `^` overflow; drag it onto the taskbar to pin it.

## Roadmap

- **v1** — Windows, hold-to-talk, local engine with GPU support, Turkish cleanup, personal
  dictionary, review panel, paste into the active window, minimal CLI. This release.
- **v2** — a Turkish fine-tune of our own; "brief mode", which turns what you said into a
  structured instruction for a coding agent; macOS and Linux builds.
- **later** — live streaming, per-app profiles, translation.

## Building

Windows, a Rust toolchain, CMake and MSVC. The Vulkan SDK is needed only for the GPU build,
and there is one environment variable its installer does not set.

```powershell
# The two sidecar binaries first: `tauri-build` refuses to compile without them.
powershell -NoProfile -ExecutionPolicy Bypass -File scripts\build-host.ps1 -Cpu -DebugBuild

cargo test --workspace
cargo tauri build --debug --no-bundle
```

[docs/BUILDING.md](docs/BUILDING.md) has the prerequisites, the traps and how the installer is
built; [docs/RELEASE.md](docs/RELEASE.md) is the release checklist;
[CONTRIBUTING.md](CONTRIBUTING.md) has the rules. [docs/PROJECT.md](docs/PROJECT.md) is the v1
spec and the reasoning behind every decision above.

## Credits

Speech recognition by [whisper.cpp](https://github.com/ggerganov/whisper.cpp) and
[OpenAI Whisper](https://github.com/openai/whisper) (both MIT), through
[`transcribe-cpp`](https://crates.io/crates/transcribe-cpp) (MIT).
Paste-and-restore-clipboard approach inspired by [Handy](https://github.com/cjpais/Handy) (MIT).

## License

MIT
