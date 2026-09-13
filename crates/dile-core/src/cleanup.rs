//! Deterministic Turkish cleanup: no model, no network, no variance.
//!
//! The differentiation of the whole product (`docs/PROJECT.md` §1). The competitor that is
//! closest to Dile does this with an LLM prompt over a remote provider and a policy of
//! "when in doubt, delete"; this crate does it with rules derived from a real edit log and
//! a policy of **when in doubt, keep**.
//!
//! # The shape
//!
//! A **pipeline of small rules**, each a pure `fn(&str, &Config) -> String` living in
//! [`rules`], each with a doc comment quoting the sentence of `docs/PROJECT.md` §5 it
//! implements. [`Strictness`] decides which of them run; the order they run in is fixed and
//! is a decision of its own, written out below. Nothing in a rule knows about reporting:
//! [`clean_with`] compares each rule's input with its output and records the difference, so
//! a rule that changes nothing costs one string comparison and produces no entry.
//!
//! | # | Rule | From | What it does |
//! |---|---|---|---|
//! | 1 | `hallucination` | Light | drops a canned phrase that is a whole segment |
//! | 2 | `filler` | Light | drops `eee` and its lengthened forms |
//! | 3 | `repeats` | Strict | collapses a consecutive repeat outside the whitelist |
//! | 4 | `particles` | Strict | drops a standalone *şey*, and *ya / yani / hani* at clause edges |
//! | 5 | `punctuation` | Medium | comma pile-ups, and `?` on a question that lost it |
//! | 6 | `dictionary` | Medium | the user's spelling for their own terms |
//! | 7 | `whitespace` | Light | whitespace and comma hygiene |
//! | 8 | `casing` | Medium | sentence-initial capital, with Turkish `i → İ` |
//!
//! **Why that order.** Deletions come before punctuation, because dropping a *yani* at the
//! end of a clause is what leaves a comma sitting in front of a full stop for the hygiene
//! rule to clear. Punctuation comes before the dictionary so that a question mark is decided
//! on the words the speaker said. Hygiene comes second to last, after every rule that can
//! leave a seam behind. **Casing is last**, because the first word of a sentence is only
//! known once nothing else is going to remove it — a strict run that drops a leading *Yani*
//! must capitalise the word that takes its place, not the one that was there first.
//!
//! # The levels
//!
//! * [`Strictness::Light`] — the filler regex is **exactly** `\b[Ee]{2,}\b` and nothing
//!   else, plus the known hallucination phrases when they are a whole segment, plus
//!   leftover whitespace and commas at sentence start. "eee" is the only filler form in
//!   230 real cuts; `ı+` would hit real Turkish words.
//! * [`Strictness::Medium`] — the default: + sentence-initial capitalisation, punctuation
//!   normalisation, and dictionary spelling of proper nouns and English terms.
//! * [`Strictness::Strict`] — + collapsing consecutive repeats outside a reduplication
//!   whitelist, dropping a standalone *şey* before a pause, and dropping *ya / yani / hani*
//!   at clause edges.
//!
//! **Repeat collapsing never runs in medium.** About 70 % of consecutive repeats in the
//! edit log are legitimate — reduplication idioms and onomatopoeia — so collapsing them by
//! default would delete correct Turkish.
//!
//! **Particles the maintainer keeps** and no level below strict may remove: *yani*,
//! *aslında*, *evet*, *ya*, *şey*. Their rates are in `docs/PROJECT.md` §4, and *aslında*
//! and *evet* are not in any list here at all — not even strict deletes them.

mod lists;
mod rules;

use std::sync::OnceLock;

use crate::dictionary::Dictionary;

/// How much of the raw transcript a level is allowed to change.
///
/// Ordered from the least destructive to the most, and that order is meaningful: every
/// level does everything the level below it does.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Strictness {
    /// Fillers and whole-segment hallucinations only.
    Light,
    /// The default: light, plus casing, punctuation and dictionary spelling.
    #[default]
    Medium,
    /// Medium, plus repeat collapsing and clause-edge particles.
    Strict,
}

impl Strictness {
    /// The identifier this level is stored and sent over the IPC boundary as.
    ///
    /// Deliberately not `Display`: it is a wire value, not a label. The label a user reads
    /// is a locale key, resolved in the app.
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Strictness::Light => "light",
            Strictness::Medium => "medium",
            Strictness::Strict => "strict",
        }
    }
}

/// What the rules are allowed to know about this user.
///
/// Everything here is empty or off by default except the hallucination list, which is the
/// one thing that is about the *engine* rather than about the person: whisper writes those
/// phrases on silence whoever is at the microphone.
#[derive(Clone, Debug)]
pub struct Config {
    /// Extra whole-word fillers to drop, on top of `eee`.
    ///
    /// **Empty by default and meant to stay that way.** The decision of 2026-09-07 is that
    /// the default filler regex is exactly `\b[Ee]{2,}\b` and any other pattern is opt-in;
    /// this field is that opt-in. Matching is case-insensitive under Turkish casing.
    pub extra_fillers: Vec<String>,
    /// The canned phrases removed when one is a whole segment. See [`lists`].
    pub hallucinations: Vec<String>,
    /// The user's own terms. Empty by default — see [`crate::dictionary`].
    pub dictionary: Dictionary,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            extra_fillers: Vec::new(),
            hallucinations: lists::DEFAULT_HALLUCINATIONS
                .iter()
                .map(|phrase| (*phrase).to_string())
                .collect(),
            dictionary: Dictionary::new(),
        }
    }
}

impl Config {
    /// A configuration with no rules that depend on data: no fillers, no phrases, no terms.
    ///
    /// Useful for testing one rule without the others' data, and for a user who wants the
    /// hygiene and punctuation rules and nothing that deletes.
    #[must_use]
    pub fn bare() -> Self {
        Self {
            extra_fillers: Vec::new(),
            hallucinations: Vec::new(),
            dictionary: Dictionary::new(),
        }
    }
}

/// One edit, as the rule that made it saw it.
///
/// `before` and `after` are the whole text on either side of the rule rather than the words
/// that moved. That is deliberate: the panel's "show raw" and the maintainer's edit log both
/// want to replay the transcript step by step, and a diff of two strings can always be
/// computed later while the two strings cannot be recovered from a diff.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Change {
    /// Which rule made the edit.
    pub rule: &'static str,
    /// The text the rule was given.
    pub before: String,
    /// The text the rule returned.
    pub after: String,
}

/// The result of a cleanup run: the text, and every decision that produced it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CleanReport {
    /// What the engine wrote, untouched. This is what the panel's "show raw" shows.
    pub raw: String,
    /// What the user sees.
    pub text: String,
    /// One entry per rule that changed something, in the order the rules ran.
    pub changes: Vec<Change>,
}

impl CleanReport {
    /// Did any rule change anything?
    #[must_use]
    pub fn is_unchanged(&self) -> bool {
        self.changes.is_empty()
    }

    /// The names of the rules that changed something, in order.
    #[must_use]
    pub fn rules_fired(&self) -> Vec<&'static str> {
        self.changes.iter().map(|change| change.rule).collect()
    }
}

/// One rule and the lowest level it runs at.
struct Rule {
    name: &'static str,
    from: Strictness,
    run: fn(&str, &Config) -> String,
}

/// The pipeline, in the order it runs. See the module documentation for why this order.
const PIPELINE: &[Rule] = &[
    Rule {
        name: "hallucination",
        from: Strictness::Light,
        run: rules::hallucination,
    },
    Rule {
        name: "filler",
        from: Strictness::Light,
        run: rules::filler,
    },
    Rule {
        name: "repeats",
        from: Strictness::Strict,
        run: rules::repeats,
    },
    Rule {
        name: "particles",
        from: Strictness::Strict,
        run: rules::particles,
    },
    Rule {
        name: "punctuation",
        from: Strictness::Medium,
        run: rules::punctuation,
    },
    Rule {
        name: "dictionary",
        from: Strictness::Medium,
        run: rules::dictionary,
    },
    Rule {
        name: "whitespace",
        from: Strictness::Light,
        run: rules::whitespace,
    },
    Rule {
        name: "casing",
        from: Strictness::Medium,
        run: rules::casing,
    },
];

/// The rules that run at `level`, in order.
///
/// The settings window shows this list so that a strictness chip is a promise a user can
/// read rather than a word they have to trust.
#[must_use]
pub fn rule_names(level: Strictness) -> Vec<&'static str> {
    PIPELINE
        .iter()
        .filter(|rule| rule.from <= level)
        .map(|rule| rule.name)
        .collect()
}

/// The configuration [`clean`] uses: built once, because it carries a phrase list.
fn shipped_config() -> &'static Config {
    static CONFIG: OnceLock<Config> = OnceLock::new();
    CONFIG.get_or_init(Config::default)
}

/// Apply the cleanup rules of `level` to a raw transcript, with the shipped configuration.
///
/// The shipped configuration is the hallucination list and nothing else: no extra fillers,
/// no dictionary. Use [`clean_with`] to pass the user's own.
#[must_use]
pub fn clean(raw: &str, level: Strictness) -> String {
    clean_with(raw, level, shipped_config()).text
}

/// Apply the cleanup rules of `level`, and report what each of them did.
#[must_use]
pub fn clean_with(raw: &str, level: Strictness, config: &Config) -> CleanReport {
    let mut text = raw.to_string();
    let mut changes = Vec::new();

    for rule in PIPELINE {
        if rule.from > level {
            continue;
        }
        let after = (rule.run)(&text, config);
        if after != text {
            changes.push(Change {
                rule: rule.name,
                before: core::mem::replace(&mut text, after.clone()),
                after,
            });
        }
    }

    CleanReport {
        raw: raw.to_string(),
        text,
        changes,
    }
}

#[cfg(test)]
mod tests {
    use super::{Config, Strictness, clean, clean_with, lists, rule_names};
    use crate::dictionary::{Dictionary, Entry};
    use crate::normalizer;

    #[test]
    fn the_default_level_is_medium_and_the_levels_are_ordered() {
        assert_eq!(Strictness::default(), Strictness::Medium);
        assert!(Strictness::Light < Strictness::Medium);
        assert!(Strictness::Medium < Strictness::Strict);
        assert_eq!(Strictness::Strict.as_str(), "strict");
    }

    #[test]
    fn every_level_runs_everything_the_level_below_it_runs() {
        let light = rule_names(Strictness::Light);
        let medium = rule_names(Strictness::Medium);
        let strict = rule_names(Strictness::Strict);
        assert!(light.iter().all(|rule| medium.contains(rule)));
        assert!(medium.iter().all(|rule| strict.contains(rule)));
        assert_eq!(light.len(), 3);
        assert_eq!(medium.len(), 6);
        assert_eq!(strict.len(), 8);
    }

    #[test]
    fn the_report_names_the_rule_that_moved_the_text() {
        let report = clean_with("eee bu böyle", Strictness::Medium, &Config::default());
        assert_eq!(report.raw, "eee bu böyle");
        assert_eq!(report.text, "Bu böyle");
        assert_eq!(report.rules_fired(), ["filler", "whitespace", "casing"]);
        let filler = &report.changes[0];
        assert_eq!(filler.before, "eee bu böyle");
        assert_eq!(filler.after, " bu böyle");
    }

    #[test]
    fn clean_text_leaves_no_report_entries() {
        let report = clean_with("Bugün hava güzel.", Strictness::Strict, &Config::default());
        assert!(report.is_unchanged());
        assert_eq!(report.text, "Bugün hava güzel.");
    }

    #[test]
    fn the_hallucination_list_is_the_dataset_block_plus_field_data() {
        let config = Config::default();
        assert_eq!(config.hallucinations.len(), 27);
        // Nothing a person could plausibly dictate on its own is in the list.
        for declined in lists::DECLINED_HALLUCINATIONS {
            let folded = normalizer::fold_phrase(declined);
            assert!(
                !config
                    .hallucinations
                    .iter()
                    .any(|phrase| normalizer::fold_phrase(phrase) == folded),
                "{declined} should not be removable"
            );
        }
    }

    #[test]
    fn a_dictionary_term_survives_the_sentence_capital() {
        let config = Config {
            dictionary: Dictionary::from_entries([
                Entry::new("cron").with_variant("kron"),
                Entry::new("CUDA"),
            ]),
            ..Config::default()
        };
        assert_eq!(
            clean_with("kron çalışıyor.", Strictness::Medium, &config).text,
            "cron çalışıyor."
        );
        assert_eq!(
            clean_with("CUDA çalışıyor.", Strictness::Medium, &config).text,
            "CUDA çalışıyor."
        );
    }

    #[test]
    fn two_thousand_characters_are_cleaned_well_inside_the_budget() {
        // `clean` on 2,000 characters has to disappear next to a 2.4 s transcription, so the
        // target is under a millisecond in release. The bound asserted here is deliberately
        // far above that — a timing test that fails on a loaded machine teaches nothing — and
        // a debug build is allowed an order of magnitude more, because the gate runs `cargo
        // test` without optimisations.
        let unit = "Bugün eee sistemde bir şey değişti, yani sonuç aynı kaldı. ";
        let mut sample = String::new();
        while sample.chars().count() < 2_000 {
            sample.push_str(unit);
        }

        let rounds = 20;
        let started = std::time::Instant::now();
        for _ in 0..rounds {
            let cleaned = clean(&sample, Strictness::Strict);
            assert!(!cleaned.is_empty());
        }
        let each = started.elapsed() / rounds;

        // Captured unless the run asks for it, and the first thing anyone wants to see when
        // this test is the one that failed.
        eprintln!(
            "clean: {each:?} per call over {} characters",
            sample.chars().count()
        );

        let budget = if cfg!(debug_assertions) {
            std::time::Duration::from_millis(40)
        } else {
            std::time::Duration::from_millis(2)
        };
        assert!(
            each < budget,
            "cleanup took {each:?} per call, budget {budget:?}"
        );
    }
}
