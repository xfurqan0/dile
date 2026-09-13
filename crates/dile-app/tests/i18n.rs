//! The locale files, and the rule that keeps them honest.
//!
//! `docs/PROJECT.md` WP6's acceptance criterion is *no hard-coded UI strings*, and this file
//! is that criterion, enforced from WP0 rather than from the package that finishes it. Four
//! things are checked:
//!
//! * **Exact parity.** Every language has exactly English's keys — a missing one fails, and
//!   so does an invented one.
//! * **Nothing empty, nothing nested.** Every value is a non-blank string. A nested object is
//!   worse than a missing key: `src/i18n.rs` reads these files as `BTreeMap<String, String>`,
//!   so one of them makes the whole file fail to parse and that language silently stops being
//!   offered. That is why the status table lives in `locales/README.md`.
//! * **The same holes.** A translation's placeholders are English's placeholders, or the
//!   panel renders `{hotkey}` at somebody.
//! * **No hard-coded text.** The last two tests read every `.rs` file under
//!   `crates/dile-app/src` — the whole tree rather than just its top level, because WP3 put
//!   the engine in a subdirectory and a rule with a directory-shaped hole in it is not a
//!   rule — and `ui/index.html`, and fail on prose that never passed through a catalogue.

use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::path::{Path, PathBuf};

/// The repository root, from this crate's manifest directory.
fn repo() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .canonicalize()
        .expect("resolve the repository root")
}

fn locale_dir() -> PathBuf {
    repo().join("locales")
}

fn load(locale: &str) -> BTreeMap<String, String> {
    let path = locale_dir().join(format!("{locale}.json"));
    let text = fs::read_to_string(&path).unwrap_or_else(|_| panic!("read {}", path.display()));
    serde_json::from_str(&text).unwrap_or_else(|error| panic!("{}: {error}", path.display()))
}

/// The languages on disk, which is the list everything else is checked against.
fn locales() -> Vec<String> {
    let mut found: Vec<String> = fs::read_dir(locale_dir())
        .expect("read locales/")
        .filter_map(|entry| {
            let name = entry.ok()?.file_name().to_string_lossy().into_owned();
            name.strip_suffix(".json").map(str::to_string)
        })
        .collect();
    found.sort();
    found
}

fn placeholders(template: &str) -> BTreeSet<String> {
    let mut found = BTreeSet::new();
    let mut rest = template;
    while let Some(open) = rest.find('{') {
        let after = &rest[open + 1..];
        match after.find('}') {
            Some(close) => {
                found.insert(after[..close].to_string());
                rest = &after[close + 1..];
            }
            None => break,
        }
    }
    found
}

#[test]
fn v1_ships_english_and_turkish_and_the_loader_knows_both() {
    // EN + TR only in v1 (docs/PROJECT.md §3, UI languages). A file added here without a
    // match arm in src/i18n.rs would never be read; a match arm without a file would not
    // compile. This asserts the first half.
    assert_eq!(locales(), vec!["en".to_string(), "tr".to_string()]);

    let loader =
        fs::read_to_string(repo().join("crates/dile-app/src/i18n.rs")).expect("read i18n.rs");
    for locale in locales() {
        assert!(
            loader.contains(&format!("\"{locale}\" => Some(")),
            "locales/{locale}.json exists but src/i18n.rs cannot load it"
        );
    }
}

#[test]
fn every_locale_file_is_a_flat_object_of_non_empty_strings() {
    for locale in locales() {
        for (key, value) in load(&locale) {
            assert!(!value.trim().is_empty(), "{locale}.json: {key} is blank");
            assert_eq!(
                value,
                value.trim(),
                "{locale}.json: {key} has surrounding space"
            );
            assert!(
                !key.starts_with('_'),
                "{locale}.json carries {key}; metadata belongs in locales/README.md"
            );
        }
    }
}

#[test]
fn every_language_covers_exactly_the_english_key_set() {
    let english: BTreeSet<String> = load("en").into_keys().collect();
    assert!(
        english.len() >= 15,
        "the WP0 key set is fifteen keys, not {}",
        english.len()
    );

    for locale in locales() {
        let keys: BTreeSet<String> = load(&locale).into_keys().collect();
        let missing: Vec<&String> = english.difference(&keys).collect();
        let extra: Vec<&String> = keys.difference(&english).collect();
        assert!(missing.is_empty(), "{locale}.json is missing {missing:?}");
        assert!(extra.is_empty(), "{locale}.json invents {extra:?}");
    }
}

#[test]
fn every_translation_keeps_the_placeholders_of_the_english_original() {
    let english = load("en");
    for locale in locales() {
        for (key, template) in load(&locale) {
            assert_eq!(
                placeholders(&template),
                placeholders(&english[&key]),
                "{locale}.json: {key} has different placeholders from English"
            );
        }
    }
}

#[test]
fn the_product_name_is_never_translated() {
    // A translated brand is a wrong brand. "Dile" is a Turkish word already; it is still
    // the product's name in the English file and not a word to be rendered.
    for locale in locales() {
        assert_eq!(
            load(&locale)["app.name"],
            "Dile",
            "{locale}.json translated the name"
        );
    }
}

#[test]
fn the_keys_that_carry_a_count_are_frozen_so_a_new_one_is_a_decision() {
    // Both of these render a number next to a unit *abbreviation*, which is the same word
    // after 1 as after 5 in English and Turkish alike — that is why this product needs no
    // plural forms. A counted string that spells its unit out would need a rule in
    // src/i18n.rs, so adding one to this list has to be deliberate.
    let counted: BTreeSet<&str> = ["elapsed", "cap", "count", "seconds", "minutes"].into();
    let mut found: Vec<String> = load("en")
        .into_iter()
        .filter(|(_, template)| {
            placeholders(template)
                .iter()
                .any(|p| counted.contains(p.as_str()))
        })
        .map(|(key, _)| key)
        .collect();
    found.sort();

    assert_eq!(
        found,
        ["panel.processing.elapsed", "panel.recording.elapsed"]
    );
}

// ------------------------------------------------------------------- no hard-coded text
//
// The two tests below read the sources, throw away everything a user could not see, and
// compare what is left with an allow-list.
//
// Thrown away by rule rather than listed:
//
//   * **Comments and doc comments.** They are prose by design.
//   * **The unit-test module.** Everything from `#[cfg(test)]` to the end of the file: an
//     assertion message is written for whoever reads the failure, and that reader is a
//     developer.
//   * **Diagnostics.** The literal argument of `eprintln!`, `println!`, `panic!`,
//     `unreachable!`, `todo!`, `write!`, `writeln!`, `.expect(…)`, the `log` macros and
//     `thiserror`'s `#[error("…")]`. These reach a terminal, a log file or a crash report,
//     never the tray — and Dile's command-line surface stays English on purpose, the way
//     `dile transcribe --json` will in WP7. The `log` macros joined the list in WP2, when the
//     session began reporting what it had captured, and `#[error]` in WP3, when the engine
//     gained error types: an error message is a diagnostic by exactly the same rule as a
//     `println!`, and the tray tooltips of both packages went into `locales/` like every
//     other visible word.

/// Comments out, string literals intact.
fn strip_comments(source: &str) -> String {
    let bytes: Vec<char> = source.chars().collect();
    let mut out = String::with_capacity(source.len());
    let mut index = 0;
    while index < bytes.len() {
        let rest: String = bytes[index..].iter().take(2).collect();
        match bytes[index] {
            '"' => {
                out.push('"');
                index += 1;
                while index < bytes.len() {
                    if bytes[index] == '\\' {
                        out.push(bytes[index]);
                        if index + 1 < bytes.len() {
                            out.push(bytes[index + 1]);
                        }
                        index += 2;
                        continue;
                    }
                    out.push(bytes[index]);
                    index += 1;
                    if bytes[index - 1] == '"' {
                        break;
                    }
                }
            }
            _ if rest == "//" => {
                while index < bytes.len() && bytes[index] != '\n' {
                    index += 1;
                }
            }
            _ if rest == "/*" => {
                index += 2;
                while index + 1 < bytes.len() && !(bytes[index] == '*' && bytes[index + 1] == '/') {
                    index += 1;
                }
                index += 2;
            }
            other => {
                out.push(other);
                index += 1;
            }
        }
    }
    out
}

/// Every string literal in `source`, with what precedes it, in order.
fn literals(source: &str) -> Vec<(String, String)> {
    let chars: Vec<char> = source.chars().collect();
    let mut found = Vec::new();
    let mut index = 0;
    let mut since = String::new();
    while index < chars.len() {
        if chars[index] == '"' {
            let mut value = String::new();
            index += 1;
            while index < chars.len() && chars[index] != '"' {
                if chars[index] == '\\' {
                    index += 1;
                }
                if index < chars.len() {
                    value.push(chars[index]);
                }
                index += 1;
            }
            index += 1;
            found.push((value, since.clone()));
            since.clear();
        } else {
            since.push(chars[index]);
            index += 1;
        }
    }
    found
}

/// True when the literal is the message of a diagnostic macro or `.expect`.
///
/// The `log` macros are matched on their bare name as well as through `log::`, because both
/// spellings are in use across the workspace and an import would otherwise decide whether a
/// line is a diagnostic.
fn is_diagnostic(before: &str) -> bool {
    let head = before.trim_end();
    [
        "eprintln!(",
        "println!(",
        "panic!(",
        "unreachable!(",
        "todo!(",
        "write!(",
        "writeln!(",
        "expect(",
        "assert!(",
        "error!(",
        "warn!(",
        "info!(",
        "debug!(",
        "trace!(",
        // `thiserror`'s message attribute. An error's text goes to a log or a bug report,
        // and the sentence a user reads about the same failure is a tray tooltip that comes
        // out of `locales/` like every other visible word.
        "#[error(",
    ]
    .iter()
    .any(|macro_head| head.ends_with(macro_head))
}

/// True when the literal reads like a sentence rather than an identifier or a path.
///
/// Two or more space-separated runs of letters. `"tray.menu.quit"`, `"Ctrl+Alt+Space"` and
/// `"../../../locales/en.json"` are not prose; `"Recording stopped"` is.
fn is_prose(value: &str) -> bool {
    value
        .split_whitespace()
        .filter(|word| word.chars().filter(|c| c.is_alphabetic()).count() >= 2)
        .count()
        >= 2
}

#[test]
fn the_rust_side_hard_codes_no_text_a_user_could_read() {
    // Every entry here is a literal that survives the rules above and is still not UI text.
    // A new one needs a reason written beside it, which is the point of the list.
    let allowed: BTreeSet<&str> = [
        // The tray tooltip's placeholder value, not a sentence: WP2 owns the hotkey and its
        // display form; the tooltip around it comes from the catalogue.
        "Ctrl+Alt+Space",
        // WP3's probe clip says this, and `engine/probe.rs` holds it so that a failed probe
        // can log both sides of the comparison. It is the content of an audio file, not a
        // string anybody is shown: translating it would mean the expected words no longer
        // matched the committed WAV, which is the one thing that must never drift.
        "Bugün hava çok güzel ve deniz sakin.",
    ]
    .into();

    let src = repo().join("crates/dile-app/src");
    let sources = rust_files(&src);
    let mut checked = 0;
    for path in &sources {
        let source = fs::read_to_string(path).expect("read a source file");
        // The unit tests live at the end of the file, after `#[cfg(test)]`.
        let code = source.split("#[cfg(test)]").next().unwrap_or_default();
        checked += 1;

        for (value, before) in literals(&strip_comments(code)) {
            if is_diagnostic(&before) || !is_prose(&value) || allowed.contains(value.as_str()) {
                continue;
            }
            panic!(
                "{}: hard-coded UI text {value:?} — every visible word comes from locales/",
                path.display()
            );
        }
    }
    assert!(checked >= 8, "the scan found only {checked} source files");
    assert!(
        sources
            .iter()
            .any(|path| path.components().any(|part| part.as_os_str() == "engine")),
        "the scan never reached src/engine/, so a subdirectory could hide a sentence"
    );
}

/// Every `.rs` file under `directory`, including the ones in subdirectories.
///
/// Sorted, so a failure names the same file on every machine.
fn rust_files(directory: &Path) -> Vec<PathBuf> {
    let mut found = Vec::new();
    let mut pending = vec![directory.to_path_buf()];
    while let Some(current) = pending.pop() {
        for entry in fs::read_dir(&current).expect("read a source directory") {
            let path = entry.expect("a directory entry").path();
            if path.is_dir() {
                pending.push(path);
            } else if path.extension().is_some_and(|extension| extension == "rs") {
                found.push(path);
            }
        }
    }
    found.sort();
    found
}

#[test]
fn the_markup_hard_codes_no_text_a_user_could_read() {
    // WP5 fills ui/ with the review panel. The rule is written now so the first sentence
    // anybody types into the markup fails the build rather than shipping untranslatable.
    let html = fs::read_to_string(repo().join("ui/index.html")).expect("read ui/index.html");

    let mut text = String::new();
    let mut inside_tag = false;
    let mut rest = html.as_str();
    // Comments, <style> and <head> are not text a user reads.
    while let Some(start) = rest.find("<!--") {
        let after = &rest[start..];
        let end = after.find("-->").map_or(after.len(), |at| start + at + 3);
        rest = &rest[end.min(rest.len())..];
    }
    let body = html.split("<body").nth(1).unwrap_or(rest);
    for character in body.chars() {
        match character {
            '<' => inside_tag = true,
            '>' => inside_tag = false,
            other if !inside_tag => text.push(other),
            _ => {}
        }
    }

    assert!(
        text.trim().is_empty(),
        "ui/index.html carries text: {:?} — every visible word comes from locales/",
        text.trim()
    );
}
