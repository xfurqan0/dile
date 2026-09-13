//! Small text primitives the cleanup rules share: words and sentences.
//!
//! Private on purpose — these are implementation details of the rules rather than API.
//! Only one thing here is a *decision* rather than a utility: [`sentences`]. Where a
//! sentence ends decides what the hallucination filter is allowed to delete and what the
//! casing rule is allowed to capitalise, so the abbreviation guard is written out in full
//! instead of being buried in a pattern.

use core::ops::Range;

use crate::normalizer;

/// Where one word sits in the text, with its Turkish suffix told apart from its base.
///
/// `cache'i` is one word whose base is `cache` and whose suffix is `'i`. The dictionary
/// replaces the base and leaves the suffix exactly as the speaker's grammar produced it,
/// which is the whole reason this split exists.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Word {
    /// The whole token: base, apostrophe and suffix.
    pub(crate) range: Range<usize>,
    /// The base alone — everything before the apostrophe, or the whole token if there is
    /// no apostrophe.
    pub(crate) base: Range<usize>,
}

/// The characters a word is made of.
///
/// `char::is_alphanumeric` is Unicode-aware, so `ı İ ç ğ ö ş ü` are letters without a list
/// of their own. This is the "Turkish-aware word boundary" the filler rule needs: `eee` is
/// a whole token in `Eee, bu` and is not one inside `veee`.
pub(crate) fn is_word_char(c: char) -> bool {
    c.is_alphanumeric()
}

/// The two apostrophes Turkish text arrives with: ASCII `'` and the typographic `’`.
pub(crate) fn is_apostrophe(c: char) -> bool {
    c == '\'' || c == '\u{2019}'
}

/// The character at `pos`, with the number of bytes it occupies.
fn char_at(text: &str, pos: usize) -> Option<(char, usize)> {
    text[pos..].chars().next().map(|c| (c, c.len_utf8()))
}

/// Every word in `text`, in order.
pub(crate) fn words(text: &str) -> Vec<Word> {
    let mut out = Vec::new();
    let mut pos = 0usize;

    while let Some((c, len)) = char_at(text, pos) {
        if !is_word_char(c) {
            pos += len;
            continue;
        }

        let start = pos;
        pos += len;
        while let Some((c2, len2)) = char_at(text, pos) {
            if is_word_char(c2) {
                pos += len2;
            } else {
                break;
            }
        }
        let base = start..pos;

        // An apostrophe only joins the word when a letter follows it. A closing quote —
        // `dedi'` — is punctuation and must not be swallowed into the token.
        let mut end = pos;
        if let Some((c2, len2)) = char_at(text, pos)
            && is_apostrophe(c2)
            && let Some((c3, _)) = char_at(text, pos + len2)
            && is_word_char(c3)
        {
            let mut scan = pos + len2;
            while let Some((c4, len4)) = char_at(text, scan) {
                if is_word_char(c4) {
                    scan += len4;
                } else {
                    break;
                }
            }
            end = scan;
            pos = scan;
        }

        out.push(Word {
            range: start..end,
            base,
        });
    }

    out
}

/// Split `text` into sentences.
///
/// Every byte of `text` belongs to exactly one range and the ranges are in order, so
/// joining the slices reproduces the input. A range carries its terminator and the
/// whitespace that follows it.
///
/// `!` and `?` always end a sentence. A `.` ends one **unless**:
///
/// 1. a letter or digit follows it immediately, with no space — `whisper.cpp`, `M.K` and
///    `3.14` are one word each, not three sentences; or
/// 2. the word in front of it is a single letter or all digits **and** the next word starts
///    with a lower-case letter — `1. madde` and `M. sokak` continue, while `Altyazı M.K.
///    Merhaba` ends after the `K.` because `Merhaba` is capitalised.
///
/// Both guards err towards **fewer** boundaries, which is the safe direction: the
/// hallucination filter deletes whole sentences, so a missed boundary keeps text and a
/// wrong one would delete it.
pub(crate) fn sentences(text: &str) -> Vec<Range<usize>> {
    let mut out = Vec::new();
    let mut start = 0usize;
    let mut pos = 0usize;

    while let Some((c, len)) = char_at(text, pos) {
        let terminator =
            c == '!' || c == '?' || c == '\u{2026}' || (c == '.' && is_full_stop(text, pos));
        if !terminator {
            pos += len;
            continue;
        }

        // Swallow a run of terminators and closing punctuation, then the whitespace.
        let mut end = pos + len;
        while let Some((c2, len2)) = char_at(text, end) {
            if c2 == '!'
                || c2 == '?'
                || c2 == '.'
                || c2 == '\u{2026}'
                || c2 == '"'
                || c2 == ')'
                || c2 == '\u{201d}'
            {
                end += len2;
            } else {
                break;
            }
        }
        while let Some((c2, len2)) = char_at(text, end) {
            if c2.is_whitespace() {
                end += len2;
            } else {
                break;
            }
        }

        out.push(start..end);
        start = end;
        pos = end;
    }

    if start < text.len() || out.is_empty() {
        out.push(start..text.len());
    }
    out
}

/// Is the `.` at `pos` a sentence end rather than part of a word?
fn is_full_stop(text: &str, pos: usize) -> bool {
    // Guard 1: something is glued to the right of it.
    if let Some((next, _)) = char_at(text, pos + 1)
        && is_word_char(next)
    {
        return false;
    }

    // Guard 2: an ordinal or an initial, followed by a word that does not start a sentence.
    let before = &text[..pos];
    let token: String = before
        .chars()
        .rev()
        .take_while(|c| is_word_char(*c))
        .collect();
    let short = token.chars().count() == 1;
    let ordinal = !token.is_empty() && token.chars().all(|c| c.is_ascii_digit());
    if short || ordinal {
        let rest = &text[pos + 1..];
        if let Some(next) = rest.chars().find(|c| !c.is_whitespace())
            && next.is_alphabetic()
            && normalizer::upper_char(next) != next
        {
            return false;
        }
    }

    true
}

#[cfg(test)]
mod tests {
    use super::{sentences, words};

    fn slices(text: &str) -> Vec<&str> {
        sentences(text).into_iter().map(|r| &text[r]).collect()
    }

    #[test]
    fn a_word_keeps_its_apostrophe_suffix_but_not_a_closing_quote() {
        let text = "cache'i token'ları dedi' bak";
        let found = words(text);
        let tokens: Vec<&str> = found.iter().map(|w| &text[w.range.clone()]).collect();
        let bases: Vec<&str> = found.iter().map(|w| &text[w.base.clone()]).collect();
        assert_eq!(tokens, ["cache'i", "token'ları", "dedi", "bak"]);
        assert_eq!(bases, ["cache", "token", "dedi", "bak"]);
    }

    #[test]
    fn sentences_cover_the_whole_text_and_respect_abbreviations() {
        assert_eq!(slices("Bir. İki! Üç?"), ["Bir. ", "İki! ", "Üç?"]);
        // Glued to the right: one word, one sentence.
        assert_eq!(slices("whisper.cpp derlendi."), ["whisper.cpp derlendi."]);
        // An ordinal followed by a lower-case word is not a boundary.
        assert_eq!(slices("1. madde okundu."), ["1. madde okundu."]);
        // An initial followed by a capitalised word is one.
        assert_eq!(
            slices("Altyazı M.K. Merhaba."),
            ["Altyazı M.K. ", "Merhaba."]
        );
        // Joining the slices always reproduces the input.
        let text = "Bir  şey , oldu.  Sonra?   Bitti";
        assert_eq!(slices(text).concat(), text);
    }
}
