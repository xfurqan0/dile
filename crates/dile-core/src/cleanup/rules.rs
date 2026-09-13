//! The rules themselves: one function per rule, each pure and each testable alone.
//!
//! Every function has the same shape — `fn(&str, &Config) -> String` — so the pipeline in
//! [`super`] can hold them in a table and the level decides which ones run. A rule never
//! reports what it did: the pipeline compares its input with its output and records the
//! difference, which keeps the rules free of bookkeeping and makes a rule that changes
//! nothing literally free.
//!
//! Each doc comment opens with the sentence of `docs/PROJECT.md` §5 that the rule
//! implements. If the two ever disagree, the document is right and the code is a bug.

use core::ops::Range;

use super::{Config, lists};
use crate::normalizer;
use crate::text;

// ---------------------------------------------------------------------------------------
// Light
// ---------------------------------------------------------------------------------------

/// §5 Light — *"Remove the 24 known hallucination phrases when they appear as a whole
/// segment."*
///
/// **Whole segment means whole segment.** A phrase is removed only when it is the entire
/// text or an entire sentence standing on its own, never when it sits inside one. This is
/// not caution for its own sake: M0's robustness run found `beğenmeyi unutmayın` genuinely
/// spoken in the middle of a screencast sentence (`docs/PROJECT.md` §8, 2026-09-09), and a
/// filter that matched on the phrase alone would have eaten it. The comparison folds case
/// and diacritics — `Altyazı M.K.` and `altyazi m k` are one phrase — and the folded form
/// is never written out, so a sentence that survives keeps every diacritic it arrived with.
///
/// A segment that is nothing but the same phrase repeated — the `Altyazı M.K. Altyazı M.K.`
/// shape whisper produces on silence — is also a whole-segment match.
pub(super) fn hallucination(text: &str, config: &Config) -> String {
    if config.hallucinations.is_empty() {
        return text.to_string();
    }
    let phrases: Vec<String> = config
        .hallucinations
        .iter()
        .map(|phrase| normalizer::fold_phrase(phrase))
        .filter(|phrase| !phrase.is_empty())
        .collect();

    let mut out = String::with_capacity(text.len());
    for range in text::sentences(text) {
        let sentence = &text[range];
        let folded = normalizer::fold_phrase(sentence);
        if folded.is_empty() {
            out.push_str(sentence);
            continue;
        }
        if phrases.iter().any(|phrase| is_repeats_of(&folded, phrase)) {
            continue;
        }
        out.push_str(sentence);
    }
    out
}

/// Is `folded` exactly `phrase`, or `phrase` repeated back to back?
fn is_repeats_of(folded: &str, phrase: &str) -> bool {
    if phrase.is_empty() || folded.len() < phrase.len() {
        return false;
    }
    let mut rest = folded;
    loop {
        let Some(stripped) = rest.strip_prefix(phrase) else {
            return false;
        };
        if stripped.is_empty() {
            return true;
        }
        let Some(next) = stripped.strip_prefix(' ') else {
            return false;
        };
        rest = next;
    }
}

/// §5 Light — *"remove 'eee' and its lengthened forms — the default filler regex is exactly
/// `\b[Ee]{2,}\b` and nothing else."*
///
/// Two or more `e`s and nothing but `e`s, as a whole word. The word boundary is
/// Turkish-aware because [`crate::text::is_word_char`] is Unicode-aware, so `ı` and `İ` are
/// letters and `veee` is one word rather than a word with a filler inside it. A comma that
/// trailed the filler goes with it: `Eee, bu` becomes `bu`, not `, bu`.
///
/// **This rule is expected to do nothing.** M0 measured 20 sentences across 8 engine rows
/// and found zero vocalised fillers — every whisper row had already removed the deliberate
/// `Eee,` in sentence 15 (`docs/PROJECT.md` §8, 2026-09-09). It stays as a guard for engines
/// that do write fillers, which is why one of its tests is that ordinary text comes back
/// untouched. `extra_fillers` is the opt-in the decision of 2026-09-07 asked for, and it
/// ships empty; `ı+` is deliberately not in it, because it would hit real Turkish words.
pub(super) fn filler(text: &str, config: &Config) -> String {
    let extras: Vec<String> = config
        .extra_fillers
        .iter()
        .map(|filler| normalizer::to_lower(filler.trim()))
        .filter(|filler| !filler.is_empty())
        .collect();

    let mut cuts: Vec<Range<usize>> = Vec::new();
    for word in text::words(text) {
        let token = &text[word.range.clone()];
        let hesitation = token.chars().count() >= 2 && token.chars().all(|c| c == 'e' || c == 'E');
        if !hesitation && !extras.contains(&normalizer::to_lower(token)) {
            continue;
        }
        cuts.push(word.range.start..with_trailing_comma(text, word.range.end));
    }
    remove(text, &cuts)
}

/// §5 Light — *"fix leftover whitespace and comma at sentence start."*
///
/// The whole of it lives in [`crate::normalizer::normalize`], because casing and whitespace
/// are the two things that must behave identically no matter which level is running and
/// having one of them in two places is how they drift apart.
pub(super) fn whitespace(text: &str, _config: &Config) -> String {
    normalizer::normalize(text)
}

// ---------------------------------------------------------------------------------------
// Medium
// ---------------------------------------------------------------------------------------

/// §5 Medium — *"punctuation normalisation (comma pile-ups, '?' after question particles
/// mı/mi/mu/mü and question words)."*
///
/// Comma pile-ups first: `, ,` becomes `,`, however many there are. Then each sentence is
/// asked whether it is a question, and the answer is yes when its last word is a form of the
/// question particle, or when its first word is one of the nine question words of §5. A
/// sentence that already ends in `?` or `!` is never touched — the speaker's own punctuation
/// outranks a rule.
///
/// **A known false positive, written down rather than hidden:** `Ne yazık ki geç kaldık.`
/// starts with *ne* and is not a question. The rule is the one §5 specifies, the panel shows
/// the result before it is pasted, and the alternative — a list of exclamative openers — is
/// a second guess layered on a first. If this shows up in real use it becomes a finding, not
/// a silent patch.
pub(super) fn punctuation(text: &str, _config: &Config) -> String {
    let collapsed = collapse_comma_runs(text);
    let mut out = String::with_capacity(collapsed.len());
    for range in text::sentences(&collapsed) {
        out.push_str(&question_mark(&collapsed[range]));
    }
    out
}

/// `, ,` and `,,` become `,`.
fn collapse_comma_runs(text: &str) -> String {
    let chars: Vec<char> = text.chars().collect();
    let mut out = String::with_capacity(text.len());
    let mut index = 0usize;
    while index < chars.len() {
        if chars[index] != ',' {
            out.push(chars[index]);
            index += 1;
            continue;
        }
        out.push(',');
        // Swallow every further comma, and the spaces between them.
        let mut scan = index + 1;
        let mut last_comma = index;
        while scan < chars.len() {
            match chars[scan] {
                ' ' | '\t' => scan += 1,
                ',' => {
                    last_comma = scan;
                    scan += 1;
                }
                _ => break,
            }
        }
        index = last_comma + 1;
    }
    out
}

/// Put a `?` on one sentence if it asks something and does not say so.
fn question_mark(sentence: &str) -> String {
    let body = sentence.trim_end();
    let tail = &sentence[body.len()..];
    if body.ends_with('?') || body.ends_with('!') || body.ends_with('\u{2026}') {
        return sentence.to_string();
    }

    let words = text::words(body);
    let Some(first) = words.first() else {
        return sentence.to_string();
    };
    let last = words.last().unwrap_or(first);

    let opener = normalizer::to_lower(&body[first.base.clone()]);
    let closer = normalizer::to_lower(&body[last.base.clone()]);
    let asks = lists::QUESTION_PARTICLES.contains(&closer.as_str())
        || lists::QUESTION_WORDS.contains(&opener.as_str());
    if !asks {
        return sentence.to_string();
    }

    // The particle has to be the last thing in the sentence; anything after it but the
    // final full stop means the sentence went on.
    if body[last.range.end..].chars().any(|c| c != '.' && c != ' ') {
        return sentence.to_string();
    }

    let mut result = body.trim_end_matches(['.', ' ']).to_string();
    if result.is_empty() {
        return sentence.to_string();
    }
    result.push('?');
    result.push_str(tail);
    result
}

/// §5 Medium — *"dictionary spelling of proper nouns and English terms (non-ASCII safe)."*
///
/// All of it is [`crate::dictionary::Dictionary::apply`]; this is the seam that puts it in
/// the pipeline. The shipped dictionary is empty, so on a machine where the user has not
/// typed a term the rule costs one comparison and changes nothing.
pub(super) fn dictionary(text: &str, config: &Config) -> String {
    config.dictionary.apply(text)
}

/// §5 Medium — *"sentence-initial capitalisation"*, and §5 — *"the engine's Turkish output
/// is normalised to proper ı/İ casing rules."*
///
/// Only the first letter of a sentence is touched, and only upwards: nothing here
/// lower-cases, so a word that arrived in capitals keeps them. A first word the dictionary
/// owns is skipped entirely — `cron çalışıyor.` keeps its lower-case `cron`, which a rule
/// that capitalised blindly would have turned into a different word. The casing itself comes
/// from [`crate::normalizer::upper_char`] and from nowhere else.
pub(super) fn casing(text: &str, config: &Config) -> String {
    let mut out = String::with_capacity(text.len());
    let mut cursor = 0usize;

    for range in text::sentences(text) {
        let sentence = &text[range.clone()];
        let Some(word) = text::words(sentence).into_iter().next() else {
            continue;
        };
        let base = &sentence[word.base.clone()];
        if config.dictionary.is_term(base) {
            continue;
        }
        let Some(first) = base.chars().next() else {
            continue;
        };
        let upper = normalizer::upper_char(first);
        if upper == first {
            continue;
        }
        let at = range.start + word.base.start;
        out.push_str(&text[cursor..at]);
        out.push(upper);
        cursor = at + first.len_utf8();
    }

    out.push_str(&text[cursor..]);
    out
}

// ---------------------------------------------------------------------------------------
// Strict
// ---------------------------------------------------------------------------------------

/// §5 Strict — *"collapse consecutive repeats except a reduplication whitelist"*, and §7 —
/// *"repeat collapsing is a strict-only rule and never runs in medium."*
///
/// Two identical words separated by nothing but spaces become one, unless the pair is in
/// [`lists::REDUPLICATIONS`]. The whitelist errs towards keeping because the edit log says
/// it must: about **70 % of consecutive repeats in real data are legitimate**
/// (`docs/PROJECT.md` §4). A comma between the two words is not a repeat — `bir daha, bir
/// daha` is a rhetorical repetition and the speaker put the pause there on purpose.
pub(super) fn repeats(text: &str, _config: &Config) -> String {
    let words = text::words(text);
    let allowed: Vec<String> = lists::REDUPLICATIONS
        .iter()
        .map(|pair| normalizer::fold_phrase(pair))
        .collect();

    let mut cuts: Vec<Range<usize>> = Vec::new();
    for pair in words.windows(2) {
        let (first, second) = (&pair[0], &pair[1]);
        let gap = &text[first.range.end..second.range.start];
        if gap.is_empty() || !gap.chars().all(|c| c == ' ' || c == '\t') {
            continue;
        }
        let left = normalizer::fold_phrase(&text[first.range.clone()]);
        let right = normalizer::fold_phrase(&text[second.range.clone()]);
        if left.is_empty() || left != right {
            continue;
        }
        if allowed.contains(&format!("{left} {right}")) {
            continue;
        }
        cuts.push(first.range.end..second.range.end);
    }
    remove(text, &cuts)
}

/// §5 Strict — *"drop standalone 'şey' before a pause ('bir şey' stays)"* and *"drop
/// ya / yani / hani at clause edges."*
///
/// A clause edge is the start of a sentence or the position directly in front of `,` `.`
/// `?` `!`. Anything else is mid-clause and is left alone, because *yani* in the middle of a
/// sentence is carrying meaning — the edit log has the maintainer keeping it 1.7–2.1 times a
/// minute (`docs/PROJECT.md` §4), which is why no level below strict touches it at all.
///
/// Two guards. A *şey* preceded by *bir*, *her*, *hiçbir* or any other word of
/// [`lists::SEY_GUARDS`] is part of a noun phrase and stays. A leading *ya* followed by
/// *da*, *de* or another *ya* is the first half of *ya … ya da* and stays.
pub(super) fn particles(text: &str, _config: &Config) -> String {
    let words = text::words(text);
    let starts: Vec<usize> = text::sentences(text)
        .into_iter()
        .filter_map(|range| {
            text::words(&text[range.clone()])
                .first()
                .map(|word| range.start + word.range.start)
        })
        .collect();

    let mut cuts: Vec<Range<usize>> = Vec::new();
    for (index, word) in words.iter().enumerate() {
        let token = normalizer::to_lower(&text[word.range.clone()]);
        let at_start = starts.contains(&word.range.start);
        let before_pause = text[word.range.end..]
            .chars()
            .find(|c| !c.is_whitespace())
            .is_none_or(|c| [',', '.', '?', '!'].contains(&c));

        if token == "şey" {
            if !before_pause {
                continue;
            }
            let guarded = index > 0 && {
                let previous = normalizer::to_lower(&text[words[index - 1].range.clone()]);
                lists::SEY_GUARDS.contains(&previous.as_str())
            };
            if guarded {
                continue;
            }
            cuts.push(word.range.start..with_trailing_comma(text, word.range.end));
            continue;
        }

        if !lists::CLAUSE_PARTICLES.contains(&token.as_str()) {
            continue;
        }
        if !at_start && !before_pause {
            continue;
        }
        if token == "ya"
            && let Some(next) = words.get(index + 1)
        {
            let following = normalizer::to_lower(&text[next.range.clone()]);
            if lists::YA_CORRELATIVES.contains(&following.as_str()) {
                continue;
            }
        }
        cuts.push(word.range.start..with_trailing_comma(text, word.range.end));
    }
    remove(text, &cuts)
}

// ---------------------------------------------------------------------------------------
// Shared plumbing
// ---------------------------------------------------------------------------------------

/// Where a cut should end when the word it removes was followed by a comma.
///
/// Dropping a word and leaving its comma behind turns `Eee, bu böyle.` into `, bu böyle.`
/// and moves the problem to the next rule. Only one comma is taken, and only when nothing
/// but spaces stands between.
fn with_trailing_comma(text: &str, end: usize) -> usize {
    let rest = &text[end..];
    let trimmed = rest.trim_start_matches(' ');
    if trimmed.starts_with(',') {
        text.len() - trimmed.len() + 1
    } else {
        end
    }
}

/// Copy `text` without the byte ranges in `cuts`.
///
/// The ranges arrive in order and may touch but never overlap, which is what lets every rule
/// collect its decisions first and edit once — a rule that edited as it went would invalidate
/// its own offsets.
fn remove(text: &str, cuts: &[Range<usize>]) -> String {
    if cuts.is_empty() {
        return text.to_string();
    }
    let mut out = String::with_capacity(text.len());
    let mut cursor = 0usize;
    for cut in cuts {
        if cut.start < cursor {
            continue;
        }
        out.push_str(&text[cursor..cut.start]);
        cursor = cut.end;
    }
    out.push_str(&text[cursor..]);
    out
}
