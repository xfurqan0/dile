# Locales

Every word a user of Dile can see comes from a file in this directory. Nothing is typed
into code or markup — `crates/dile-app/tests/i18n.rs` reads the sources and fails on a
literal that reads like a sentence.

## The format

A flat JSON object: **every value is a string**, and there are no nested objects, no
arrays and no numbers. That is not a style rule. The Rust side parses these files as
`BTreeMap<String, String>`, and one nested value anywhere makes the whole file fail to
parse — that language then falls back to English *without a word of complaint*. So the
status table lives in this README rather than in a `_meta` key inside the files.

Placeholders are `{name}` and are substituted verbatim. A translation carries exactly the
placeholders the English original does; the test fails on a missing or invented one.

## What ships

| Language | File | Status |
|---|---|---|
| English | `en.json` | Authored. The fallback for every missing string. |
| Turkish | `tr.json` | Authored by the maintainer. |

**EN and TR only in v1**, decided 2026-09-07 (`docs/PROJECT.md` §3, UI languages). Dile's
audience is Turkish and English speakers, and four machine translations would be
maintenance debt on a product whose whole point is careful Turkish. The format and the
policy are identical to nazar-tray's, so adding a language later is a translation job and
not a refactor — which is what WP6 is.

## Rules a locale file must keep

- Exactly English's key set. A missing key fails the build, and so does an invented one.
- No blank values, no leading or trailing whitespace.
- The same `{placeholders}` as the English original.
- **Product names are never translated.** `Dile` is `Dile` in every file.
- A key is added to every file in the same commit.

## Contributing a correction

A native speaker fixing a sentence is a welcome pull request and needs no issue first.
Change the file, run `cargo test -p dile-app`, and say in the message what read wrong.
