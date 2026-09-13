//! The word lists the cleanup rules consult, with the evidence for each one beside it.
//!
//! Kept in one file because a list is data and a rule is logic: changing a phrase should not
//! mean reading a rule, and reading a rule should not mean scrolling past forty phrases.

/// The canned phrases the hallucination filter removes when one stands alone as a whole
/// segment.
///
/// **Source.** Rows 1–17 are the Turkish block of
/// `sachaarbonel/whisper-hallucinations` (`phrases.csv`, MIT, ~7,900 phrases across ~100
/// languages), which carries 24 Turkish rows. Rows 18–27 are the **seed extended from field
/// data**: the phrase our own robustness run actually produced
/// (`izlediğiniz için teşekkürler`, `docs/PROJECT.md` §8, 2026-09-09) and the full sentences
/// that the dataset only carries as truncations. The list is a seed, not a census; it grows
/// from what this product's own engine writes, not from imagination.
///
/// **Seven of the 24 dataset rows are deliberately not here**, because each of them is
/// something a person could plausibly dictate as the whole of an utterance, and the product
/// policy is *when in doubt, keep*:
///
/// | Row | Why it is out |
/// |---|---|
/// | `abone ol` | two words, and an ordinary imperative someone might dictate into a message |
/// | `afiyet olsun` | ordinary Turkish |
/// | `görüşürüz` | ordinary Turkish, and a one-word dictation is a real thing |
/// | `olsun` | ordinary Turkish |
/// | `ayı unutmayın` | a mid-word truncation of `…mayı unutmayın`; it also reads as *don't forget the month* |
/// | `mayın teşekkürler` | the same truncation, from the other side; the full sentences below cover it |
/// | `tıklamayı unutmayın` | a truncation of `beğen butonuna tıklamayı unutmayın`, which is here in full |
///
/// **The closest call in the list is `beğenmeyi unutmayın`**, and it is here on purpose.
/// M0's robustness run found it **genuinely spoken** inside a screencast
/// (`docs/PROJECT.md` §8, 2026-09-09, finding b), which is exactly why the filter may only
/// delete a whole segment: the sentence it was spoken in protects it, and nothing else
/// would have. A user who dictates video copy for a living can drop it from
/// [`super::Config::hallucinations`].
pub(crate) const DEFAULT_HALLUCINATIONS: &[&str] = &[
    // --- the MIT dataset's Turkish block, the rows that cannot be ordinary speech ---
    "abone olmayı beğenmeyi ve yorum yapmayı unutmayın",
    "abone olmayı ve videoyu beğenmeyi unutmayın",
    "abone olmayı yorum yapmayı ve beğen butonuna tıklam",
    "altyazı m k",
    "ayı unutmayın teşekkürler",
    "bir sonraki videoda görüşürüz",
    "bu videoyu beğenmeyi ve kanalımıza abone olmayı unut",
    "i zlediğiniz için teşekkür ederim",
    "i zlediğiniz için teşekkür ederim bir sonraki videoda",
    "instagram da hoşçakalın",
    "instagram'da ki videoları da paylaşabilirsiniz",
    "kanalıma abone olmayı ve videoyu beğenmeyi unutmayın",
    "kanalıma abone olmayı yorum yapmayı ve beğen butonuna",
    "ve bu şekilde de gizli cimrimizdeki gibi",
    "www feyyaz tv",
    "yeni videolarda görüşünceye kadar hoşçakalın",
    "çeviri ve altyazı m k",
    // --- extended from field data: what our own runs wrote, and the full sentences the
    //     dataset only carries in truncated form ---
    "izlediğiniz için teşekkürler",
    "izlediğiniz için teşekkür ederim",
    "izlediğiniz için teşekkür ederiz",
    "abone olmayı unutmayın",
    "beğenmeyi unutmayın",
    "beğenmeyi ve abone olmayı unutmayın",
    "beğen butonuna tıklamayı unutmayın",
    "kanalıma abone olmayı unutmayın",
    "kanalıma abone olun",
    "bu videoyu beğendiyseniz",
];

/// The dataset rows that were considered and left out. See [`DEFAULT_HALLUCINATIONS`].
#[cfg(test)]
pub(crate) const DECLINED_HALLUCINATIONS: &[&str] = &[
    "abone ol",
    "afiyet olsun",
    "görüşürüz",
    "olsun",
    "ayı unutmayın",
    "mayın teşekkürler",
    "tıklamayı unutmayın",
];

/// Reduplications that are correct Turkish and survive the strict repeat rule.
///
/// `docs/PROJECT.md` §4: consecutive-word repeats run at 0.5 per minute in the maintainer's
/// edit log and **about 70 % of them are legitimate**. A whitelist that errs towards keeping
/// is therefore the only shape that does not delete correct Turkish, and the list is longer
/// than the obvious four for that reason. `bir bir` is in it: *bir bir saydı* — "counted them
/// one by one" — is ordinary speech, and losing one of the two words changes the sentence.
pub(crate) const REDUPLICATIONS: &[&str] = &[
    "adım adım",
    "ağır ağır",
    "akşam akşam",
    "ayrı ayrı",
    "bir bir",
    "bol bol",
    "çok çok",
    "damla damla",
    "derin derin",
    "dolu dolu",
    "gide gide",
    "güle güle",
    "güzel güzel",
    "hafif hafif",
    "hemen hemen",
    "hızlı hızlı",
    "kısa kısa",
    "kolay kolay",
    "koşa koşa",
    "parça parça",
    "sabah sabah",
    "sık sık",
    "tatlı tatlı",
    "teker teker",
    "tek tek",
    "uzun uzun",
    "yavaş yavaş",
    "yer yer",
    "zaman zaman",
];

/// The question particle *mı / mi / mu / mü* and the suffixes it takes.
///
/// Matched in Turkish lower case rather than folded, because folding would merge `mudur`
/// with `müdür` and `müdür` is not in this list: it is also the noun *director*, so
/// *Ahmet müdür.* is a statement and putting a question mark on it would be a visible wrong
/// edit. Every other form here is a particle and nothing else.
pub(crate) const QUESTION_PARTICLES: &[&str] = &[
    "mı",
    "mi",
    "mu",
    "mü",
    "mıyım",
    "miyim",
    "muyum",
    "müyüm",
    "mısın",
    "misin",
    "musun",
    "müsün",
    "mıyız",
    "miyiz",
    "muyuz",
    "müyüz",
    "mısınız",
    "misiniz",
    "musunuz",
    "müsünüz",
    "mıydı",
    "miydi",
    "muydu",
    "müydü",
    "mıydım",
    "miydim",
    "muydum",
    "müydüm",
    "mıydın",
    "miydin",
    "muydun",
    "müydün",
    "mıydık",
    "miydik",
    "muyduk",
    "müydük",
    "mıydınız",
    "miydiniz",
    "muydunuz",
    "müydünüz",
    "mıydılar",
    "miydiler",
    "muydular",
    "müydüler",
    "mıymış",
    "miymiş",
    "muymuş",
    "müymüş",
    "mıdır",
    "midir",
    "mudur",
];

/// The question words that make a sentence a question when it starts with one.
///
/// Exactly the nine of `docs/PROJECT.md` §5 — *ne zaman* starts with *ne* and needs no row
/// of its own. Siblings such as *niçin* and *nereden* are deliberately absent: this rule
/// rewrites punctuation the user did not ask for, so the list stays the one that was
/// decided rather than the one that could be inferred.
pub(crate) const QUESTION_WORDS: &[&str] = &[
    "ne", "neden", "niye", "nasıl", "nerede", "nereye", "kim", "hangi", "kaç",
];

/// Words that turn a following *şey* into part of a noun phrase rather than a hesitation.
///
/// `docs/PROJECT.md` §5: *"drop standalone şey before a pause (bir şey stays)"*. *bir şey*,
/// *her şey* and *hiçbir şey* are the three the rule names; the rest are here because they
/// form the same kind of phrase and the cost of keeping a *şey* is a word, while the cost of
/// deleting one is a sentence that no longer says what was said.
pub(crate) const SEY_GUARDS: &[&str] = &[
    "bir", "her", "hiçbir", "hiç", "başka", "o", "bu", "şu", "ne", "böyle", "şöyle", "öyle",
    "aynı", "tek", "birkaç",
];

/// The particles the strict level drops at a clause edge.
pub(crate) const CLAUSE_PARTICLES: &[&str] = &["ya", "yani", "hani"];

/// Words that make a leading *ya* the first half of *ya … ya da*, which is not a particle.
pub(crate) const YA_CORRELATIVES: &[&str] = &["da", "de", "ya"];
