//! The personal dictionary: proper nouns and English terms, with `ç ğ ı ö ş ü` intact.
//!
//! Two jobs, and the second is the one other tools miss (`docs/PROJECT.md` §1):
//!
//! 1. **Spelling after the fact** — [`Dictionary::apply`]. Replace what the engine wrote
//!    with what the user meant. The failure this repairs is always the same shape: an
//!    English term or a product name spoken inside a Turkish sentence comes back as the
//!    nearest Turkish-sounding word. The adjudication run of 2026-09-09 found the class in
//!    the maintainer's own voice, and every engine row made the same kind of mistake.
//! 2. **Biasing the engine before the fact** — [`Dictionary::prompt`]. The same entries are
//!    fed to the engine as an initial prompt. This is not a nicety: `transcribe-cpp` 0.2.3
//!    has **no beam search** and decodes greedily, and M0 measured the prompt lifting term
//!    recall from 17/31 to 30/31 and word error from 0.379 to 0.261 on the 20-sentence set
//!    (`docs/PROJECT.md` §8, 2026-09-09). The prompt is the product's accuracy feature.
//!
//! **The default dictionary is empty**, and deliberately so. Every entry is a claim about
//! one person's vocabulary; shipping a stranger's terms would put words into transcripts
//! that were never spoken, which is the one failure a dictation tool may not have. The
//! tests below use neutral technical vocabulary, not a shipped list.
//!
//! **Non-ASCII is the point.** The leading competitor's fuzzy matcher requires ASCII, so
//! every Turkish word with a diacritic is silently dropped from its dictionary. Matching
//! here is case-insensitive under **both** Turkish and Unicode casing — `keş`, `Keş` and
//! `KEŞ` are one variant — and it is deliberately *not* diacritic-folded, because folding
//! would make the variant `keş` swallow the ordinary Turkish word *kes*. A stored entry
//! keeps its own spelling exactly, byte for byte, on the way in and on the way out.

use std::collections::HashMap;

use crate::normalizer;
use crate::text;

/// How many characters of terms a prompt may carry.
///
/// Whisper's prompt window is 224 tokens, and Turkish tokenises badly — a term list is
/// mostly sub-word pieces, so the safe assumption is well under one token per character.
/// 180 characters keeps the list inside the window with room for the carrier sentence the
/// engine adds, and the cap is a documented constant rather than a guess repeated in two
/// places.
pub const PROMPT_CHAR_BUDGET: usize = 180;

/// One dictionary entry: the spelling the user wants, and what the engine writes instead.
///
/// The direction is deliberate. `canonical` is the truth and appears in the transcript
/// exactly as it is typed here; `variants` are the mis-hearings, and they exist only to be
/// recognised. An entry with no variants is still useful — it goes into the prompt, which
/// is what stops the mis-hearing from happening in the first place.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Entry {
    /// The spelling the user wants, exactly as it must appear in the transcript.
    pub canonical: String,
    /// The forms the engine produces instead, matched case-insensitively.
    pub variants: Vec<String>,
    /// Keep this term at the front of the prompt when the budget cannot hold everything.
    pub pinned: bool,
}

impl Entry {
    /// An entry for `canonical` with no variants yet.
    #[must_use]
    pub fn new(canonical: impl Into<String>) -> Self {
        Self {
            canonical: canonical.into(),
            variants: Vec::new(),
            pinned: false,
        }
    }

    /// Add one mis-hearing to recognise.
    #[must_use]
    pub fn with_variant(mut self, variant: impl Into<String>) -> Self {
        self.variants.push(variant.into());
        self
    }

    /// Keep this term at the front of the prompt. See [`Dictionary::prompt`].
    #[must_use]
    pub fn pin(mut self) -> Self {
        self.pinned = true;
        self
    }
}

/// The user's dictionary.
#[derive(Clone, Debug, Default)]
pub struct Dictionary {
    entries: Vec<Entry>,
}

impl Dictionary {
    /// An empty dictionary — the one this product ships with.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Build a dictionary from entries, in order.
    #[must_use]
    pub fn from_entries(entries: impl IntoIterator<Item = Entry>) -> Self {
        Self {
            entries: entries.into_iter().collect(),
        }
    }

    /// Every entry, in insertion order.
    #[must_use]
    pub fn entries(&self) -> &[Entry] {
        &self.entries
    }

    /// How many entries there are.
    #[must_use]
    pub fn len(&self) -> usize {
        self.entries.len()
    }

    /// Is there nothing to correct or to bias with?
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    /// Add an entry, keeping its spelling exactly as given.
    pub fn insert(&mut self, entry: Entry) {
        self.entries.push(entry);
    }

    /// Is `word` a term this dictionary owns, in either its canonical or a variant form?
    ///
    /// The casing rule asks before it capitalises a sentence's first word, so `cron
    /// çalışıyor.` keeps its lower-case `cron` and `CUDA çalışıyor.` keeps all four
    /// capitals. A dictionary term is a spelling the user chose; a rule that "fixes" it has
    /// undone the feature.
    #[must_use]
    pub fn is_term(&self, word: &str) -> bool {
        let (turkish, unicode) = keys(word);
        self.entries.iter().any(|entry| {
            matches_key(&entry.canonical, &turkish, &unicode)
                || entry
                    .variants
                    .iter()
                    .any(|variant| matches_key(variant, &turkish, &unicode))
        })
    }

    /// Replace every variant in `text` with its canonical spelling.
    ///
    /// Whole words only, case-insensitive on the variant, and the canonical spelling is
    /// copied in byte for byte. **A Turkish suffix survives:** the match is against the part
    /// in front of the apostrophe, so `keş'i` becomes `cache'i` and `cache'i` is left alone.
    /// The first entry that claims a form wins, so a dictionary is deterministic no matter
    /// how it was assembled.
    #[must_use]
    pub fn apply(&self, text: &str) -> String {
        if self.entries.is_empty() {
            return text.to_string();
        }

        let index = self.index();
        let mut out = String::with_capacity(text.len());
        let mut cursor = 0usize;

        for word in text::words(text) {
            let base = &text[word.base.clone()];
            let (turkish, unicode) = keys(base);
            let Some(canonical) = index.get(&turkish).or_else(|| index.get(&unicode)) else {
                continue;
            };
            out.push_str(&text[cursor..word.base.start]);
            out.push_str(canonical);
            cursor = word.base.end;
        }

        out.push_str(&text[cursor..]);
        out
    }

    /// The terms as an engine initial prompt, or `None` when there is nothing to bias with.
    ///
    /// Deterministic and capped. Order: pinned terms first in insertion order, then the rest
    /// **most recently added first** — a term typed today is the one the user is about to
    /// say. Terms are joined with `", "` and the list ends with a full stop, because a
    /// comma-separated list is what whisper conditions on best and because M0 measured the
    /// prompt's comma habit lifting punctuation F1 from 0.63–0.72 to 0.75–0.82
    /// (`docs/PROJECT.md` §8, 2026-09-09).
    ///
    /// The cap is [`PROMPT_CHAR_BUDGET`] characters of terms; a term that would cross it is
    /// left out and so is every term after it, so the same dictionary always produces the
    /// same prompt. An empty dictionary produces `None` rather than an empty string, which
    /// whisper would happily condition on.
    #[must_use]
    pub fn prompt(&self) -> Option<String> {
        let pinned = self.entries.iter().filter(|entry| entry.pinned);
        let rest = self.entries.iter().rev().filter(|entry| !entry.pinned);

        let mut terms: Vec<&str> = Vec::new();
        let mut used = 0usize;
        for entry in pinned.chain(rest) {
            let term = entry.canonical.trim();
            if term.is_empty() {
                continue;
            }
            let separator = if terms.is_empty() { 0 } else { 2 };
            if used + separator + term.chars().count() > PROMPT_CHAR_BUDGET {
                break;
            }
            used += separator + term.chars().count();
            terms.push(term);
        }

        if terms.is_empty() {
            return None;
        }
        Some(format!("{}.", terms.join(", ")))
    }

    /// Every recognised form mapped to the canonical spelling it stands for.
    fn index(&self) -> HashMap<String, &str> {
        let mut map: HashMap<String, &str> = HashMap::new();
        for entry in &self.entries {
            let canonical = entry.canonical.as_str();
            for form in std::iter::once(canonical).chain(entry.variants.iter().map(String::as_str))
            {
                let (turkish, unicode) = keys(form);
                map.entry(turkish).or_insert(canonical);
                map.entry(unicode).or_insert(canonical);
            }
        }
        map
    }
}

/// The two case-insensitive keys a word is looked up by.
///
/// Two, not one, because the two lowercasings disagree on exactly one letter and both
/// disagreements are real: Turkish maps `I` to `ı`, so an English term typed `CLI` would
/// never find a stored `cli`, while Unicode maps `I` to `i`, so a Turkish term typed `IŞIK`
/// would never find a stored `ışık`. Indexing under both costs one extra string per form
/// and removes the whole class.
fn keys(word: &str) -> (String, String) {
    (normalizer::to_lower(word), word.to_lowercase())
}

/// Does `form` match either key?
fn matches_key(form: &str, turkish: &str, unicode: &str) -> bool {
    let (form_turkish, form_unicode) = keys(form);
    form_turkish == turkish || form_unicode == unicode
}

#[cfg(test)]
mod tests {
    use super::{Dictionary, Entry, PROMPT_CHAR_BUDGET};

    fn neutral() -> Dictionary {
        Dictionary::from_entries([
            Entry::new("cache").with_variant("keş").with_variant("Cage"),
            Entry::new("cron").with_variant("kron"),
            Entry::new("CUDA").with_variant("judo"),
            Entry::new("token").with_variant("takım"),
            Entry::new("Kubernetes").with_variant("kubernetis"),
            Entry::new("webhook").with_variant("web hok"),
            Entry::new("Tauri").with_variant("tavri"),
        ])
    }

    #[test]
    fn the_shipped_dictionary_is_empty() {
        // Every entry is a claim about one person's vocabulary. A default list would put
        // words into other people's transcripts that they never said.
        let shipped = Dictionary::new();
        assert!(shipped.is_empty());
        assert_eq!(shipped.len(), 0);
        assert_eq!(shipped.prompt(), None);
        assert_eq!(shipped.apply("bir şey oldu"), "bir şey oldu");
    }

    #[test]
    fn an_entry_keeps_its_turkish_letters_byte_for_byte() {
        // The regression this whole module exists for: a dictionary that folds diacritics
        // on the way in has already lost the word.
        let dictionary = Dictionary::from_entries([
            Entry::new("Iğdır").with_variant("ığdır"),
            Entry::new("çağrı").with_variant("cagri"),
            Entry::new("gözlemci").with_variant("gozlemci"),
            Entry::new("şüphe").with_variant("suphe"),
        ]);

        assert_eq!(dictionary.entries()[0].canonical, "Iğdır");
        assert!(dictionary.entries()[0].canonical.contains('ğ'));
        assert_eq!(dictionary.apply("ığdır yolu"), "Iğdır yolu");
        assert_eq!(dictionary.apply("cagri geldi"), "çağrı geldi");
        assert_eq!(dictionary.apply("gozlemci bekliyor"), "gözlemci bekliyor");
        assert_eq!(dictionary.apply("suphe yok"), "şüphe yok");
    }

    #[test]
    fn every_turkish_letter_round_trips_in_both_directions() {
        // The acceptance criterion of WP4, letter by letter: ç ğ ı ö ş ü in the canonical
        // form and in the variant, replaced and then left alone on a second pass.
        let dictionary = Dictionary::from_entries([
            Entry::new("çekirdek").with_variant("cekirdek"),
            Entry::new("ğ-harfi").with_variant("g-harfi"),
            Entry::new("ışık").with_variant("isik"),
            Entry::new("öbek").with_variant("obek"),
            Entry::new("şema").with_variant("sema"),
            Entry::new("ürün").with_variant("urun"),
        ]);

        let heard = "cekirdek isik obek sema urun";
        let wanted = "çekirdek ışık öbek şema ürün";
        assert_eq!(dictionary.apply(heard), wanted);
        // Idempotent: the canonical form is recognised as its own form.
        assert_eq!(dictionary.apply(wanted), wanted);
        for letter in ['ç', 'ğ', 'ı', 'ö', 'ş', 'ü'] {
            let prompt = dictionary.prompt().unwrap();
            assert!(prompt.contains(letter), "{letter} lost from the prompt");
        }
    }

    #[test]
    fn a_turkish_suffix_survives_the_replacement() {
        let dictionary = neutral();
        assert_eq!(dictionary.apply("keş'i temizle"), "cache'i temizle");
        assert_eq!(dictionary.apply("cache'i temizle"), "cache'i temizle");
        assert_eq!(dictionary.apply("takım'ları say"), "token'ları say");
        assert_eq!(dictionary.apply("judo'yu kur"), "CUDA'yu kur");
    }

    #[test]
    fn matching_is_case_insensitive_on_the_variant_and_whole_word_only() {
        let dictionary = neutral();
        assert_eq!(dictionary.apply("Cage KRON Judo"), "cache cron CUDA");
        // Whole words only: a variant inside a longer word is not a match.
        assert_eq!(dictionary.apply("kronometre kaldı"), "kronometre kaldı");
        assert_eq!(dictionary.apply("takımlar geldi"), "takımlar geldi");
    }

    #[test]
    fn is_term_answers_for_both_forms() {
        let dictionary = neutral();
        assert!(dictionary.is_term("cron"));
        assert!(dictionary.is_term("CRON"));
        assert!(dictionary.is_term("kron"));
        assert!(dictionary.is_term("CUDA"));
        assert!(!dictionary.is_term("kronometre"));
        assert!(!Dictionary::new().is_term("cron"));
    }

    #[test]
    fn the_prompt_is_deterministic_pinned_first_and_capped() {
        let dictionary = neutral();
        let prompt = dictionary.prompt().expect("seven terms make a prompt");
        assert_eq!(prompt, dictionary.prompt().unwrap());
        // Most recently added first.
        assert!(prompt.starts_with("Tauri, webhook, Kubernetes"));
        assert!(prompt.ends_with('.'));
        assert!(prompt.chars().count() <= PROMPT_CHAR_BUDGET + 1);

        let mut pinned = neutral();
        pinned.insert(Entry::new("SSH").pin());
        assert!(pinned.prompt().unwrap().starts_with("SSH, Tauri"));
    }

    #[test]
    fn the_prompt_stops_at_the_budget_rather_than_truncating_a_term() {
        let long: Vec<Entry> = (0..40)
            .map(|i| Entry::new(format!("terim{i:02}-uzun-bir-kelime")))
            .collect();
        let prompt = Dictionary::from_entries(long).prompt().unwrap();
        assert!(prompt.chars().count() <= PROMPT_CHAR_BUDGET + 1);
        // Nothing is cut mid-word: every listed term still ends the way it was stored.
        for term in prompt.trim_end_matches('.').split(", ") {
            assert!(term.ends_with("-uzun-bir-kelime"), "truncated term: {term}");
        }
    }
}
