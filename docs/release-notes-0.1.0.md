# Dile 0.1.0

**Hold a key. Speak. Let go.** Local, Turkish-first dictation for Windows. What you said
appears in a small panel at the top of the screen, then lands in whatever you were typing in —
with your own clipboard put back afterwards.

    winget install xfurqan0.dile

Windows 10 1809 or newer. Installs per user, into `%LOCALAPPDATA%\Dile`, and asks for no
administrator rights.

**The winget package is reviewed a day or two after this release**, so until that command
resolves, take `Dile_0.1.0_x64-setup.exe` from the assets below and run it.

## What it does

- **`Ctrl+Alt+Space`, held.** The chord is swallowed, so no space reaches the editor behind
  it. Hold-to-talk by default, press-to-toggle as a setting, and a second key you can turn on.
- **It never pastes blind.** The cleaned text appears in a card on the monitor the focused
  window is on, with a countdown line; it transfers itself after 2.5 s unless you press ✗,
  edit the text, or simply rest the pointer on it. Edit it and press Enter and what lands is
  your edit. Copy instead, re-run the cleanup at another strictness, or look at what the
  engine actually wrote.
- **Turkish first, and measured.** Stock Whisper large-v3 `q5_0` with your personal dictionary
  as the decoder's prompt: on Dile's own 20-sentence set that took word error from 0.379 to
  0.261 and technical-term recall from 17 of 31 to 30. Turkish `ı/İ` casing, punctuation
  repair and three cleanup strictness levels are rules rather than a second model.
- **The dictionary keeps `ç ğ ı ö ş ü` byte for byte**, in both directions — fed to the
  recognizer, and used to correct the spelling afterwards, keeping the Turkish suffix
  (`keş'i` → `cache'i`).
- **The engine is a process of its own**, on the GPU through Vulkan, chosen by a probe on
  first start. A driver fault costs the dictation in flight and not the application.
- **Nothing leaves the machine.** Audio is processed in memory, never written to disk in a
  release build, never sent anywhere. The only network access is downloading the speech model
  on first run, behind a consent dialog, from Hugging Face at a pinned commit with the sha256
  checked before the file takes its name.
- **`dile transcribe <file.wav> --json`** for a script: the same engine, the same settings,
  the same dictionary.
- English and Turkish interface, following Windows when it is left on Automatic.

## First run

The application starts with no model. It asks — once, in a dialog, in your language — before
downloading **1,081 MB** of weights, and *Not now* leaves it running with the hotkey still
recording and the tray saying there is no model. Then it transcribes two seconds of Turkish
whose answer it already knows to decide whether this machine's GPU can be trusted; that
decision is written down once per machine rather than taken every morning.

## Known limits

- **Windows only.** macOS and Linux come from the same codebase in v2.
- **The clipboard restore is text only.** An image, a file list or a spreadsheet range on the
  clipboard before a dictation does not survive it.
- A machine whose GPU probe fails drops to a CPU tier with a smaller model and says so. That
  tier is slower than real time; it is a fallback, not a mode to choose.
- Recording is capped at 60 s by default, 300 s at most. Dile is dictation, not transcription:
  meeting recordings, file batches and subtitles are deliberately out of scope.
- **Unsigned.** SmartScreen warns on a browser download — *More info → Run anyway*;
  `winget install` does not go through it. A push-to-talk tool installs a keyboard hook and
  opens a microphone, so an endpoint-protection product may look twice. Why, and what to check
  instead: [docs/CODE_SIGNING.md](https://github.com/xfurqan0/dile/blob/main/docs/CODE_SIGNING.md).
- Uninstalling silently — which is what `winget uninstall` does — keeps your settings and the
  downloaded model. Remove them by hand with
  `Remove-Item -Recurse "$env:APPDATA\io.github.xfurqan0.dile", "$env:LOCALAPPDATA\io.github.xfurqan0.dile"`.
- Windows 11 keeps new tray icons in the `^` overflow; drag it onto the taskbar to pin it.

## Verify

Both files below were built by the release workflow on a GitHub runner, from this tag, and
`SHA256SUMS` is the hash list it wrote:

    Get-FileHash .\Dile_0.1.0_x64-setup.exe -Algorithm SHA256
    gh attestation verify .\Dile_0.1.0_x64-setup.exe --repo xfurqan0/dile

The attestation is the stronger of the two: signed by GitHub, it names the workflow run and
the commit that produced that exact file.

MIT licensed. The speech weights are OpenAI's Whisper, MIT, quantized by the `whisper.cpp`
project — the README lists the files and where they come from.
