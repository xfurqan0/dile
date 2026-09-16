# Contributing

Thanks for looking. Dile is deliberately narrow, and these are the few rules that keep it
that way.

## Getting set up

`docs/BUILDING.md` has the prerequisites and the traps. The short version:

```powershell
rustup toolchain install stable
cargo install tauri-cli --locked

# First, and before every cargo command: the application declares two sidecar binaries,
# and `tauri-build` refuses to compile without them on disk.
powershell -NoProfile -ExecutionPolicy Bypass -File scripts\build-host.ps1 -Cpu -DebugBuild

cargo test --workspace
cargo tauri build --debug --no-bundle
```

You need CMake and MSVC. You do **not** need the Vulkan SDK unless you are working on the
GPU path — that is what `-Cpu` is for.

The workspace also builds and tests on Linux, and CI runs the same gate there. Same shape,
one script name and a handful of system packages apart:

```bash
scripts/build-host.sh --cpu --debug

cargo test --workspace
cargo tauri build --debug --no-bundle
```

`docs/BUILDING.md`, "Building on Linux", has the package list and — more to the point — what
does and does not work there yet. Windows is still the platform Dile ships on.

## The rules

- **English everywhere.** Code, comments, commit messages, issues, pull requests,
  documentation. The interface is translated; the repository is not.
- **No hard-coded UI text.** Every word a user can see comes from `locales/<lang>.json`,
  in the tray and in the panel alike. `crates/dile-app/tests/i18n.rs` reads the sources and
  fails on a literal that reads like a sentence. Command-line output stays English on
  purpose — WP7's `dile transcribe --json` emits machine-readable text a script parses.
- **EN and TR together, or neither.** A new message key goes into both files in the same
  commit; a test asserts exact parity, including placeholders. `locales/README.md` has the
  rules a locale file must keep.
- **Tests are required.** A change without a test that fails before it and passes after it
  does not go in. Bug fixes need the regression test that pins the bug.
- **When in doubt, keep.** This is the cleanup layer's whole philosophy and it is a rule
  about code as much as about text: a rule that deletes a word a user actually said is worse
  than no rule. The competitor's policy is "when in doubt, delete", and the maintainer's own
  edit log is why Dile's is the opposite — *yani* and *aslında* are kept, not tidied away.
- **No invented Turkish.** Every cleanup rule comes from measured data in the edit-log
  corpus, not from an intuition about how people speak. The first draft of the plan had a
  filler regex covering `ı+` and `hmm+`; 230 real cuts contained neither.
- **Audio and text never leave the machine.** No telemetry, no analytics, no crash reporter,
  no "anonymous usage statistics". The only network call in the product is a model download
  the user consented to. A dependency that opens a socket does not go in.
- **Nothing from dikte.** [dikte](https://github.com/yusufipk/dikte) is GPL-3.0 and Dile is
  MIT. No code, no snippet, no data file. Ideas may be studied and reimplemented
  independently, and `docs/PROJECT.md` records each such case with the idea rather than the
  code.
- **Permissive dependencies only.** `deny.toml` holds the policy and CI runs
  `cargo deny check`. Adding a dependency means adding it to `THIRD-PARTY-NOTICES.md` too.
- **No absolute paths.** Every path is derived at run time. Test fixtures come from
  environment variables, which is why the engine's smoke test reads `DILE_TEST_MODEL` and
  `DILE_TEST_WAV` instead of pointing at somebody's disk.
- **No AI attribution in commits or pull requests.** No `Generated with …` trailer, no
  `Co-Authored-By:` for a model or an assistant, no session links. Write the message in your
  own voice; the history is a record of decisions, not of tooling.

## Commits and pull requests

One change per pull request, with the reasoning in the message rather than in a comment
thread. `docs/PROJECT.md` §8 is the project log; a change that closes or reopens a decision
belongs there too.

Run before pushing:

```powershell
cargo fmt --all
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace
```

## Translations

`locales/README.md` has the rules and the status table. A native speaker fixing a sentence
is a welcome pull request and needs no issue first. More languages are WP6 and are gated on
v1 shipping, not on anybody's willingness — the format is already right for them.

## What is out of scope

Meeting recording with speaker split, file or batch transcription, and SRT or subtitle
export. All three are deliberate exclusions rather than forgotten gaps; `docs/PROJECT.md` §2
is the full list of what Dile is not. A voice assistant that runs commands is also not this
product.
