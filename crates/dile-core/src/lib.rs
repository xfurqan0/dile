//! The Turkish layer: what happens to the engine's output before a human sees it.
//!
//! This crate has **no Tauri, no audio and no engine in it** on purpose. It is text in,
//! text out, so it builds and tests on every platform from the first commit and a later
//! macOS or Linux port is a step rather than a rewrite. It has **no dependencies** either,
//! not even a regular-expression crate: the rules of `docs/PROJECT.md` §5 are a handful of
//! scans over a string, and writing them by hand keeps the one crate that has to build
//! everywhere free of anything that might not.
//!
//! Three modules, in the order a transcript passes through them:
//!
//! | Module | What it owns |
//! |---|---|
//! | [`normalizer`] | Turkish casing (`i ↔ İ`, `ı ↔ I`), whitespace and punctuation hygiene, the comparison fold |
//! | [`cleanup`] | The three strictness levels, the rule pipeline and the hallucination filter |
//! | [`dictionary`] | Proper nouns and English terms, non-ASCII safe, also fed to the engine as a prompt |
//!
//! # What a caller does
//!
//! ```
//! use dile_core::cleanup::{Strictness, clean};
//!
//! assert_eq!(clean("eee bu böyle", Strictness::Medium), "Bu böyle");
//! ```
//!
//! and, when the user's own terms and a record of what changed are wanted:
//!
//! ```
//! use dile_core::cleanup::{Config, Strictness, clean_with};
//! use dile_core::dictionary::{Dictionary, Entry};
//!
//! let mut config = Config::default();
//! config.dictionary = Dictionary::from_entries([Entry::new("cron").with_variant("kron")]);
//!
//! let report = clean_with("kron görevini durdur.", Strictness::Medium, &config);
//! assert_eq!(report.text, "cron görevini durdur.");
//! assert_eq!(report.rules_fired(), ["dictionary"]);
//! ```
//!
//! **Nothing here is guessed.** The rules come from the maintainer's own edit log — 230
//! timestamped cuts — and from the M0 engine runs, both summarised in `docs/PROJECT.md` §4
//! and §8. Where the data says a rule has nothing to do, the rule is still here and says so
//! in its own documentation rather than being quietly dropped.

#![forbid(unsafe_code)]

pub mod cleanup;
pub mod dictionary;
pub mod normalizer;

mod text;

/// The version of this crate, for the settings page and bug reports.
#[must_use]
pub fn version() -> &'static str {
    env!("CARGO_PKG_VERSION")
}

#[cfg(test)]
mod tests {
    use super::version;

    #[test]
    fn version_is_the_crate_version() {
        assert_eq!(version(), env!("CARGO_PKG_VERSION"));
    }
}
