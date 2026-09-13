//! Turkish text normalisation: casing, whitespace, punctuation.
//!
//! Runs inside the cleanup pipeline as its whitespace rule and is the one stage that is
//! **not** optional at any strictness level, because getting it wrong changes words rather
//! than tidying them.
//!
//! **Turkish casing is not Unicode default casing.** Turkish has two `i` letters and they
//! do not cross: dotted `i` uppercases to `İ` and dotless `ı` uppercases to `I`. Rust's
//! `to_uppercase` gives `I` for `i`, which turns *ilgi* into *ILGI* and, worse, *ışık* into
//! *IŞIK* correctly but *için* into *IÇIN* wrongly. Every casing decision in this product
//! goes through this module, and `char::to_uppercase` is not called anywhere else.
//!
//! **Diacritics are never stripped** (`docs/PROJECT.md` §5). [`fold_char`] exists only as a
//! comparison key for the dictionary matcher and the hallucination filter, and its result
//! never reaches the output.

use crate::text;

/// Uppercase one character the way Turkish does.
///
/// The only difference from Unicode default casing is `i → İ`; every other letter follows
/// the default rule.
#[must_use]
pub fn upper_char(c: char) -> char {
    match c {
        'i' => 'İ',
        'ı' => 'I',
        other => other.to_uppercase().next().unwrap_or(other),
    }
}

/// Lowercase one character the way Turkish does.
///
/// The only difference from Unicode default casing is `I → ı`; `İ` lowercases to `i` under
/// both rules, and is spelled out here so the pair reads as a pair.
#[must_use]
pub fn lower_char(c: char) -> char {
    match c {
        'I' => 'ı',
        'İ' => 'i',
        other => other.to_lowercase().next().unwrap_or(other),
    }
}

/// Uppercase a string the way Turkish does.
#[must_use]
pub fn to_upper(text: &str) -> String {
    text.chars().map(upper_char).collect()
}

/// Lowercase a string the way Turkish does.
#[must_use]
pub fn to_lower(text: &str) -> String {
    text.chars().map(lower_char).collect()
}

/// Fold one character to a diacritic-free, case-free comparison key.
///
/// **A key, never output.** `ş` and `s` compare equal here so that `Altyazı M.K.` matches a
/// stored `altyazi m k`, and so that an engine which wrote `Kübernetis` can still be matched
/// against a dictionary variant typed as `kubernetis`. Both spellings survive untouched;
/// only the lookup is folded.
///
/// `İ` is handled by name because `char::to_lowercase('İ')` yields two characters — `i` plus
/// a combining dot — and a comparison key wants one character per letter.
#[must_use]
pub fn fold_char(c: char) -> char {
    match c {
        'ı' | 'I' | 'İ' | 'î' | 'Î' | 'i' => 'i',
        'ç' | 'Ç' => 'c',
        'ğ' | 'Ğ' => 'g',
        'ö' | 'Ö' => 'o',
        'ş' | 'Ş' => 's',
        'ü' | 'Ü' => 'u',
        'â' | 'Â' => 'a',
        'û' | 'Û' => 'u',
        'ê' | 'Ê' => 'e',
        'ô' | 'Ô' => 'o',
        other => other.to_lowercase().next().unwrap_or(other),
    }
}

/// Fold a string to a comparison key. See [`fold_char`].
#[must_use]
pub fn fold(text: &str) -> String {
    text.chars().map(fold_char).collect()
}

/// Fold a whole phrase to a comparison key: diacritics folded, punctuation gone, exactly one
/// space between words.
///
/// [`fold`] answers "is this the same word"; this answers "is this the same sentence", which
/// is what the hallucination filter needs — `Altyazı M.K.`, `altyazi m k` and `ALTYAZI M. K.`
/// are one phrase and none of them is what gets written out.
#[must_use]
pub fn fold_phrase(phrase: &str) -> String {
    let mut out = String::with_capacity(phrase.len());
    let mut pending_space = false;
    for c in phrase.chars() {
        if text::is_word_char(c) {
            if pending_space && !out.is_empty() {
                out.push(' ');
            }
            pending_space = false;
            out.push(fold_char(c));
        } else {
            pending_space = true;
        }
    }
    out
}

/// The punctuation marks that may not carry a space in front of them.
const TIGHT_LEFT: [char; 6] = [',', '.', '?', '!', ':', ';'];

/// The punctuation marks that take exactly one space after them inside a sentence.
const LOOSE_RIGHT: [char; 3] = [',', ':', ';'];

/// Normalise whitespace and punctuation in a raw transcript.
///
/// Implements the whitespace half of `docs/PROJECT.md` §5 Light — *"fix leftover whitespace
/// and comma at sentence start"* — and nothing that could change a word:
///
/// * runs of whitespace collapse to one space, and sentences are rejoined with exactly one;
/// * no space before `, . ? ! : ;`;
/// * exactly one space after `, : ;` — except between two digits, because `3,14` is a
///   Turkish decimal and not a list;
/// * a comma sitting directly in front of `. ? ! ;` is dropped, since no Turkish sentence
///   ends `…,.`;
/// * a sentence that begins with a comma loses it.
///
/// Line structure is not preserved: a dictation is one utterance, and what arrives from the
/// engine is segments joined by whitespace rather than a laid-out document.
#[must_use]
pub fn normalize(raw: &str) -> String {
    let mut out = String::with_capacity(raw.len());
    for range in text::sentences(raw) {
        let sentence = normalize_sentence(&raw[range]);
        if sentence.is_empty() {
            continue;
        }
        if !out.is_empty() {
            out.push(' ');
        }
        out.push_str(&sentence);
    }
    out
}

/// The per-sentence half of [`normalize`].
fn normalize_sentence(sentence: &str) -> String {
    // One space for every run of whitespace, and none at the edges.
    let mut squeezed = String::with_capacity(sentence.len());
    let mut space = false;
    for c in sentence.trim().chars() {
        if c.is_whitespace() {
            space = true;
            continue;
        }
        if space && !squeezed.is_empty() {
            squeezed.push(' ');
        }
        space = false;
        squeezed.push(c);
    }

    // No space in front of punctuation that hugs the word to its left.
    let mut tight = String::with_capacity(squeezed.len());
    for c in squeezed.chars() {
        if TIGHT_LEFT.contains(&c) && tight.ends_with(' ') {
            tight.pop();
        }
        tight.push(c);
    }

    // A comma in front of a terminator is never right, and the strict particle rules leave
    // exactly that behind when they drop a *yani* at the end of a clause.
    let chars: Vec<char> = tight.chars().collect();
    let mut without_stray_comma = String::with_capacity(tight.len());
    for (i, c) in chars.iter().enumerate() {
        if *c == ','
            && chars[i + 1..]
                .iter()
                .find(|next| **next != ' ')
                .is_some_and(|next| ['.', '?', '!', ';'].contains(next))
        {
            continue;
        }
        without_stray_comma.push(*c);
    }

    // Exactly one space after a comma, colon or semicolon — but `3,14` keeps its decimal.
    let chars: Vec<char> = without_stray_comma.chars().collect();
    let mut spaced = String::with_capacity(without_stray_comma.len());
    for (i, c) in chars.iter().enumerate() {
        spaced.push(*c);
        if !LOOSE_RIGHT.contains(c) {
            continue;
        }
        let Some(next) = chars.get(i + 1) else {
            continue;
        };
        if next.is_whitespace() {
            continue;
        }
        let previous_is_digit = i > 0 && chars[i - 1].is_ascii_digit();
        if *c == ',' && previous_is_digit && next.is_ascii_digit() {
            continue;
        }
        spaced.push(' ');
    }

    // A sentence that starts with a comma lost a word in front of it; the comma goes too.
    let mut result = spaced.trim().to_string();
    while result.starts_with([',', ';', ':']) {
        result = result[1..].trim_start().to_string();
    }
    result
}

#[cfg(test)]
mod tests {
    use super::{fold, fold_phrase, normalize, to_lower, to_upper};

    #[test]
    fn the_two_turkish_i_letters_never_cross() {
        // The bug this module exists to prevent, in one assertion pair: Rust's own casing
        // gives "ILGI" and "iSTANBUL", both wrong in Turkish.
        assert_eq!(to_upper("ilgi"), "İLGİ");
        assert_eq!(to_lower("ILGI"), "ılgı");
        assert_eq!(to_upper("ışık"), "IŞIK");
        assert_eq!(to_lower("İSTANBUL"), "istanbul");

        // Diacritics survive both directions untouched.
        assert_eq!(to_upper("çğöşü"), "ÇĞÖŞÜ");
        assert_eq!(to_lower("ÇĞÖŞÜ"), "çğöşü");
    }

    #[test]
    fn folding_is_a_comparison_key_and_never_an_output() {
        assert_eq!(fold("Iğdır"), "igdir");
        assert_eq!(fold("İSTANBUL"), "istanbul");
        assert_eq!(fold("ÇĞIİÖŞÜ"), "cgiiosu");
        // Folding returns a new string; the word it was asked about is untouched.
        let word = "şey";
        assert_eq!(fold(word), "sey");
        assert_eq!(word, "şey");
    }

    #[test]
    fn a_phrase_folds_to_words_without_punctuation() {
        assert_eq!(fold_phrase("Altyazı M.K."), "altyazi m k");
        assert_eq!(fold_phrase("ALTYAZI M. K."), "altyazi m k");
        assert_eq!(
            fold_phrase("İzlediğiniz için teşekkürler!"),
            "izlediginiz icin tesekkurler"
        );
        assert_eq!(fold_phrase("  çğıöşü  "), "cgiosu");
        assert_eq!(fold_phrase("   "), "");
    }

    #[test]
    fn whitespace_and_commas_are_tidied_without_touching_words() {
        assert_eq!(normalize("bir  şey ,  oldu"), "bir şey, oldu");
        assert_eq!(normalize("  Merhaba .  Sonra ?  "), "Merhaba. Sonra?");
        assert_eq!(normalize(", bu böyle"), "bu böyle");
        assert_eq!(normalize("bu böyle , ."), "bu böyle.");
        // A Turkish decimal is not a list.
        assert_eq!(normalize("oran 3,14 çıktı"), "oran 3,14 çıktı");
        // Diacritics and letters are never touched.
        assert_eq!(normalize("çğıöşü ÇĞIİÖŞÜ"), "çğıöşü ÇĞIİÖŞÜ");
    }
}
