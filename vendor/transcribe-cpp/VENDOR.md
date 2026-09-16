# transcribe.cpp, vendored

This directory is [`transcribe.cpp`](https://github.com/handy-computer/transcribe.cpp)
**0.2.3**, unmodified except for the patch described below. It is MIT-licensed; the
licence text ships with the tree (`LICENSE`, `ggml/LICENSE`) and the attribution is in
`../../THIRD-PARTY-NOTICES.md`.

| | |
|---|---|
| Upstream | https://github.com/handy-computer/transcribe.cpp |
| Version | 0.2.3 (`transcribe-cpp` and `transcribe-cpp-sys` on crates.io) |
| Upstream commit | `63a44d9239d610b3908e8a66b384924cd4a77217` (`.cargo_vcs_info.json` of both tarballs) |
| Source | the two published crates, not a git clone — see "Layout" |
| Licence | MIT |

## Why it is here

Whisper's encoder is fixed at thirty seconds. The frontend pads any shorter buffer to
480000 samples, the mel to 3000 frames, and the encoder attends over all 1500 positions.
A three-second dictation therefore pays for twenty-seven seconds of zeros — and that one
encoder pass is **80–93 % of the wall clock** on this workload:

| stage, 3 s clip | CPU tier (20 threads) | Vulkan tier |
|---|---|---|
| **encoder** | **13 484 ms — 92.6 %** | **2 977 ms — 80.1 %** |
| cross-attention KV | 229 ms | 186 ms |
| prompt | 84 ms | 61 ms |
| 13 decode steps | 723 ms | 460 ms |
| **total** | **14 567 ms** | **3 715 ms** |

whisper.cpp has had a flag for this since 2022 (`-ac` / `whisper_full_params.audio_ctx`).
`transcribe.cpp`'s engine can already do it — `build_encoder_graph` accepts a shorter mel
window and `arch/whisper/encoder.cpp` already takes a prefix view of the positional
embedding when `T_enc` is under the maximum — but the run path pins the window at
`fe_nb_max_frames` and the parameter never reaches the public ABI.

Exactly this patch was offered upstream and **declined**
([#149](https://github.com/handy-computer/transcribe.cpp/issues/149)): *"I generally don't
like adding ENV flags as they feel like a hack… I am okay with some latency penalty at the
cost of correctness."* That is a defensible call for a general-purpose library and the
wrong one for a push-to-talk dictation box, where the latency penalty is the product. So
the field is ours to carry.

## The patch

Five files, one field, no behaviour change when the field is left at its default.

| File | Change |
|---|---|
| `include/transcribe/whisper.h` | `int32_t audio_ctx` appended to `struct transcribe_whisper_run_ext`, with the measured warning about how it fails. |
| `src/arch/whisper/public.cpp` | `transcribe_whisper_run_ext_init` sets it to `0` — the full window, i.e. upstream behaviour. |
| `src/arch/whisper/model.cpp` | `whisper_run`'s chunk loop sizes the encoder window from it, floored at the real unpadded audio in that window and rounded up to an even frame count. `seek_num_frames` is untouched, so timestamps and the seek advance still describe the audio rather than the encoder. |
| `bindings/rust/sys/src/transcribe_sys.rs` | The committed bindgen output: the new field, size 80 → 88, the new offset assertion, and a re-pinned `PUBLIC_HEADER_HASH`. |
| `bindings/rust/transcribe-cpp/src/family.rs` | `WhisperRunOptions::audio_ctx: Option<i32>`, forwarded by `to_sys`. |

`git log --follow vendor/transcribe-cpp` is the real answer; `git show <the engine commit>
-- vendor/transcribe-cpp` is the diff.

### What is deliberately *not* patched

`whisper_run_batch` (`src/arch/whisper/model.cpp`) still runs the full window. Dile never
calls the batch API, and a knob that is only correct on one of two code paths is worse than
one that is honestly absent from the other.

### The two floors in the C++, and why they are there and not only in Rust

Dile's own policy (`crates/dile-engine/src/lib.rs`, `audio_ctx_for`) never asks for a
window shorter than the audio. The C++ enforces it anyway, because the failure is silent:
a window shorter than the speech does not error, it returns a confident wrong transcript.
The second floor is arithmetic — the conv2 stem has stride 2, so `T_enc = n_mel_frames / 2`
and an odd frame count would lose a position.

### `PUBLIC_HEADER_HASH` / `include/transcribe.abihash`

Upstream's is `7df72bf9e667b8c2` and describes upstream's FFI surface. This tree's surface
is not that one, so the pin is recomputed rather than left to claim otherwise:

```sh
cat include/transcribe.h include/transcribe/*.h | sha256sum | cut -c1-16
```

(Upstream generates it with its own tool, which is not in the published tarball, so this is
a different recipe for the same job — a digest that changes when the surface does. Nothing
in Dile compares it across a boundary: the library is linked **statically** from this same
tree, so there is no prebuilt `libtranscribe` for it to disagree with.)

## Layout

`transcribe-cpp-sys`'s manifest lives at the repository root and carries the whole C++ tree;
the safe wrapper is a workspace member under `bindings/rust/transcribe-cpp`. The two
published tarballs split along that line, so this directory is reassembled from both:

```
vendor/transcribe-cpp/                      <- transcribe-cpp-sys-0.2.3 tarball
└── bindings/rust/transcribe-cpp/           <- transcribe-cpp-0.2.3 tarball
```

Two things were changed to make that a self-contained tree rather than a published one:

* `Cargo.toml` gained a `[workspace]` of its own (members: the wrapper) so it does not get
  pulled into Dile's, whose edition, resolver and lints are not this tree's.
* `xtask`, an upstream workspace member, is not in either tarball and is not needed to
  build; the member list names only the wrapper.

`.cargo-ok` and `.cargo_vcs_info.json` — registry bookkeeping — were removed. Everything
else is byte-for-byte what crates.io serves.

## Re-vendoring a newer upstream

1. `cargo add transcribe-cpp@<new>` in a scratch crate, or `cargo fetch`, so both tarballs
   are unpacked under `~/.cargo/registry/src/*/`.
2. Copy them into place as above; delete `.cargo-ok` and `.cargo_vcs_info.json`.
3. Re-apply the five-file patch (`git show` the engine commit that introduced it).
4. Re-pin the abihash with the command above.
5. Re-run the gate: `cargo test --workspace`, then the measurement table in
   `docs/BUILDING.md` — the safe floor is a property of the model and the decoder, and a new
   upstream can move it.

## Why a path patch and not a git one

`[patch.crates-io]` could point at a fork on GitHub instead, and that would keep this
directory out of the repository. It would also mean every build — including a CI runner
with no credentials, and anyone who clones this repository — needs a second host to be
reachable and a second repository to still exist. A vendored tree builds from one `git
clone`, identically on Windows and on Linux, offline. The cost is the tree's size and a
manual re-vendor; the benefit is that the build has one fewer thing that can disappear.
