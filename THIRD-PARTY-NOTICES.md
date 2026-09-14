# Third-party notices

Dile is MIT-licensed (see `LICENSE`) and ships with third-party code. This file lists what
travels inside the installer and what Dile credits without shipping. It is maintained by
hand while the dependency list is short; when it stops being short it becomes a generated
file, the way nazar-tray's is.

Every dependency must be under a permissive licence, or under the one weak-copyleft licence
`deny.toml` allows — MPL-2.0, which binds the files it covers rather than the program that
links them. `deny.toml` holds that policy and `cargo deny check` enforces it. A strong
copyleft dependency in an MIT product is a rewrite, not a paperwork problem.

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

## Mozilla Public License 2.0

Five crates arrive through the Tauri tree under
[MPL-2.0](https://www.mozilla.org/MPL/2.0/), the only non-permissive entry in `deny.toml`'s
allow list. `cargo tree -i <crate> -p dile-app` is where each of these paths comes from:

| Crate | Version | How it arrives |
|---|---|---|
| [`cssparser`](https://crates.io/crates/cssparser) | 0.36.0 | `tauri-utils` → `dom_query`, on the build side only: `tauri-macros` is a proc-macro crate and rewrites the frontend's HTML at compile time. |
| [`cssparser-macros`](https://crates.io/crates/cssparser-macros) | 0.6.1 | Same path, through `cssparser`. |
| [`dtoa-short`](https://crates.io/crates/dtoa-short) | 0.3.5 | Same path, through `cssparser`. |
| [`selectors`](https://crates.io/crates/selectors) | 0.36.1 | Same path, through `dom_query`. |
| [`option-ext`](https://crates.io/crates/option-ext) | 0.2.0 | `tauri` → `dirs` → `dirs-sys`. The one of the five that is linked into `dile-app.exe`. |

MPL-2.0 is file-level copyleft: the obligation travels with the covered files, not with the
program that links them, so it changes nothing about Dile's own MIT licence. **Dile modifies
none of their source**, which leaves source availability as the whole of the obligation. The
source is the crate as published: each one at the version above from
[crates.io](https://crates.io), and upstream at
[servo/rust-cssparser](https://github.com/servo/rust-cssparser) (`cssparser`,
`cssparser-macros`), [upsuper/dtoa-short](https://github.com/upsuper/dtoa-short),
[servo/stylo](https://github.com/servo/stylo) (`selectors`) and
[soc/option-ext](https://github.com/soc/option-ext).

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

There are two of them, one per engine tier, and the first-run probe decides which one this
machine fetches:

| Tier | File | Size | Licence |
|---|---|---|---|
| Vulkan (default) | `ggml-large-v3-q5_0.bin` | 1,081 MB | MIT |
| CPU (fallback) | `ggml-large-v3-turbo-q5_0.bin` | 574 MB | MIT |

Both are quantizations of **OpenAI's Whisper**, which is
[MIT licensed](https://github.com/openai/whisper/blob/main/LICENSE). They come from
[`ggerganov/whisper.cpp`](https://huggingface.co/ggerganov/whisper.cpp) on Hugging Face at
commit `5359861c739e955e79d9a303bcbc70fb988958b1`, and that repository is MIT as well. The
README carries the same two rows with a direct download link on each, for a machine that
cannot reach them from inside the application.
