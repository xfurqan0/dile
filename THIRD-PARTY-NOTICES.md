# Third-party notices

Dile is MIT-licensed (see `LICENSE`) and ships with third-party code. This file lists what
travels inside the installer and what Dile credits without shipping. It is maintained by
hand while the dependency list is short; when it stops being short it becomes a generated
file, the way nazar-tray's is.

Every dependency must be under a permissive licence. `deny.toml` holds that policy and
`cargo deny check` enforces it. A copyleft dependency in an MIT product is a rewrite, not a
paperwork problem.

## Ships inside the application

| Component | Licence | Why it is here |
|---|---|---|
| [Tauri](https://github.com/tauri-apps/tauri) 2 and its plugins | Apache-2.0 OR MIT | The application shell: window, tray, IPC, bundler. |
| [`transcribe-cpp`](https://crates.io/crates/transcribe-cpp) 0.2.3 and `transcribe-cpp-sys` | MIT | The speech-to-text runtime, and the single decision runtime of the M0 protocol. |
| [`transcribe.cpp`](https://github.com/handy-computer/transcribe.cpp), vendored by `transcribe-cpp-sys`, which vendors [`ggml`](https://github.com/ggml-org/ggml) | MIT | The native inference library and its tensor backend. Built from source at compile time. |
| [`serde`](https://serde.rs) and `serde_json` | Apache-2.0 OR MIT | Reading the locale files and the settings. |
| [`hound`](https://github.com/ruuda/hound) | Apache-2.0 | WAV reading, in the engine's smoke test only. Not linked into the shipped binary. |

The full transitive list, with each crate's licence, is what `cargo deny check licenses`
walks. Run it before adding a dependency, not after.

## Credited, not shipped

- **[Handy](https://github.com/cjpais/Handy)** (MIT). Dile's paste path follows Handy's
  approach in `paste_tx/windows.rs`: put the text on the clipboard through a delayed render,
  wait for the target application to take it, then restore the previous clipboard contents.
  The approach is credited; the code is written independently.
- **[dikte](https://github.com/yusufipk/dikte)** (GPL-3.0). Studied as the closest Turkish
  competitor and credited for two ideas — a whole-segment hallucination filter with a
  duration gate, and using the dictionary as an engine prompt. **No code, no snippet and no
  data file is taken from it**, and none may be: Dile is MIT and dikte is GPL-3.0. Where an
  idea is reimplemented, `docs/PROJECT.md` records the idea and where it came from.

## Models

Dile ships **no model weights**. Weights are downloaded from Hugging Face after an explicit
consent dialog, pinned by commit sha with a sha256 the application carries
(`docs/PROJECT.md` §3, Distribution).

Each model's licence and source URL is listed here and in the README when WP3 wires the
downloader and WP1 has chosen the model. The candidates and their licences are in
`docs/PROJECT.md` §3; OpenAI's Whisper weights are MIT.
