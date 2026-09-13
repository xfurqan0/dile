# Dile

<img src="assets/dile.png" alt="The Dile icon: seven rounded waveform bars on a dark rounded square, the middle and tallest one green" width="128">

**Hold a key. Speak. Let go.** Local, Turkish-first dictation. What you said appears in a small panel for a moment, then lands in whatever you're typing in. Fix a word, copy it, or cancel before it does.

> Status: **pre-alpha, it records.** The workspace, the tray shell and CI are in place, and holding the hotkey now captures: the chord is swallowed, the microphone opens, the first syllable is kept and the tray says what is happening. Nothing transcribes yet, nothing pastes, and there is nothing to install. See [docs/PROJECT.md](docs/PROJECT.md) for the v1 spec and the work packages.

## Why another dictation app

There are excellent local dictation tools (Handy, OpenWhispr). Dile exists for one reason: **Turkish.**

- What Turkish actually comes back wrong is the terms and the punctuation, not the fillers: measured on Dile's own set, no supported engine writes a vocalised "eee" at all. So the answer v1 ships is a dictionary that works in both directions — fed to the recognizer before it decodes, and used to correct the spelling afterwards. The measurement is already done: on that set the dictionary took word error from 0.379 to 0.261 and technical-term recall from 17 to 30 out of 31, with no language model anywhere in the loop.
- Dictionaries in other tools silently drop words with `ç ğ ı ö ş ü`. Dile's keeps them byte for byte — the round trip is tested letter by letter — so terms like `cron`, `token` and `cache` stop coming back as whatever Turkish word happened to fit the slot.
- The model is chosen by measured Turkish accuracy on Dile's own test set, not by a model card — and it is chosen: **v1 ships stock Whisper large-v3** (`q5_0`, GGUF) with that dictionary prompt. No fine-tune, because no Turkish fine-tune has beaten the stock weights on our material yet.
- Turkish `ı/İ` casing, the three cleanup strictness levels, deterministic punctuation repair and the hallucination-phrase filter are implemented and tested today, from a real 230-cut edit log. The cleanup runs no model and opens no socket: it is a fixed pipeline of rules, and every rule says in its own source which sentence of the spec it implements. What is still missing is the engine in front of it and the panel behind it.
- **Vulkan by default** is the decided tier, chosen on first start by a probe that runs inside the isolated engine process; a machine where the probe fails gets the CPU tier and a small model instead. Either way the engine gets a process of its own, so a driver crash never takes the app down.

English works too. Turkish just comes first.

## Principles

These are the rules v1 is built to. The status line above says how much of it runs today.

- **Everything stays on your machine.** Audio is processed in memory, never written to disk and never sent anywhere. The only network access is downloading a model, with your consent.
- **No account, no telemetry.** True from the first commit: nothing here opens a socket, and [CONTRIBUTING.md](CONTRIBUTING.md) refuses a dependency that does.
- **Hold to talk by default** (`Ctrl+Alt+Space`), with press-to-toggle and a second key as settings.
- **Never pastes blind.** The transcript is shown in a panel first and pastes after a short countdown; press Esc to cancel, edit it, or copy it instead. The countdown can be turned off.

## Roadmap

- v1: Windows, hold-to-talk, local engine with GPU support, Turkish cleanup, personal dictionary, review panel with level meter, paste into active window
- v2: "brief mode" that turns what you said into a structured instruction for a coding agent; macOS and Linux builds
- later: live streaming, per-app profiles, translation

## Building

Windows, a Rust toolchain, CMake and MSVC. The Vulkan SDK is needed only for the opt-in GPU
build, and there is one environment variable the SDK installer does not set — both are in
[docs/BUILDING.md](docs/BUILDING.md), along with the prerequisites and the traps.

```powershell
cargo test --workspace
cargo tauri build --debug --no-bundle
```

[CONTRIBUTING.md](CONTRIBUTING.md) has the rules, [SECURITY.md](SECURITY.md) what Dile reads
and never reads.

## Credits

Paste-and-restore-clipboard approach inspired by [Handy](https://github.com/cjpais/Handy) (MIT).

## License

MIT
