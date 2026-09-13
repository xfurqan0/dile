//! The strings the application shows, taken from `locales/<lang>.json`.
//!
//! About forty lines, and that is the whole i18n runtime: a catalogue lookup, `{placeholder}`
//! substitution, and English behind everything. ICU is not needed for a tray menu and a
//! 680-pixel strip, and the policy in `docs/PROJECT.md` §3 is flat key/value files.
//!
//! **Flat strings only.** [`parse`] asks serde for a `BTreeMap<String, String>`; a file with
//! one nested object in it does not parse, becomes an empty catalogue, and that language
//! silently falls back to English. It is the right failure — a damaged translation must not
//! stop the tray from starting — but it is an invisible one, which is why the status table
//! lives in `locales/README.md` and not in a `_meta` key, and why `tests/i18n.rs` fails on a
//! non-string value.
//!
//! **No plural forms.** Nothing in the v1 key set counts a noun: the two strings that carry
//! a number are a duration and a unit abbreviation, and an abbreviation does not inflect
//! after a numeral in either language. The first counted *word* anybody writes needs a rule
//! here, and `tests/i18n.rs` freezes the set of keys that carry a count so that adding one is
//! a decision somebody makes on purpose.
//!
//! **The language is not chosen yet.** WP5 adds the settings override and reads the Windows
//! UI language; until then [`Strings::system`] answers English. Deciding it in one place now
//! is what stops WP5 from having to unpick a tray that guessed one language and a panel that
//! guessed another.

use std::collections::BTreeMap;

/// English, the fallback for every missing string.
const EN: &str = include_str!("../../../locales/en.json");
/// Turkish, written by hand alongside English.
const TR: &str = include_str!("../../../locales/tr.json");

/// The source text of a catalogue this build carries, or `None`.
///
/// This match is the list of languages v1 ships — EN and TR, decided 2026-09-07. A file
/// added to `locales/` without an arm here would never be read, which is what
/// `tests/i18n.rs` checks; an arm here without a file would not compile.
#[must_use]
pub fn source(locale: &str) -> Option<&'static str> {
    match locale {
        "en" => Some(EN),
        "tr" => Some(TR),
        _ => None,
    }
}

/// Parse one catalogue. A file that does not parse is an empty catalogue, never a panic.
#[must_use]
pub fn parse(text: &str) -> BTreeMap<String, String> {
    serde_json::from_str(text).unwrap_or_default()
}

/// The primary subtag of a language tag: `tr-TR` and `TR_tr` both become `tr`.
#[must_use]
pub fn primary_subtag(locale: &str) -> String {
    locale
        .split(['-', '_'])
        .next()
        .unwrap_or_default()
        .to_ascii_lowercase()
}

/// Replace `{name}` with the matching parameter.
///
/// A placeholder with no parameter is left as written rather than blanked, so a missing
/// value shows up as `{hotkey}` in the panel instead of disappearing quietly.
#[must_use]
pub fn interpolate(template: &str, params: &[(&str, &str)]) -> String {
    let mut out = template.to_string();
    for (name, value) in params {
        out = out.replace(&format!("{{{name}}}"), value);
    }
    out
}

/// The strings of one language, with English behind them.
#[derive(Debug)]
pub struct Strings {
    messages: BTreeMap<String, String>,
    fallback: BTreeMap<String, String>,
}

impl Strings {
    /// The catalogue for a language tag such as `tr`, `tr-TR` or `en-GB`.
    #[must_use]
    pub fn for_locale(locale: &str) -> Self {
        let primary = primary_subtag(locale);
        let messages = match primary.as_str() {
            // English is the fallback; loading it twice would only double the memory.
            "en" => BTreeMap::new(),
            other => source(other).map(parse).unwrap_or_default(),
        };
        Strings {
            messages,
            fallback: parse(EN),
        }
    }

    /// The language the application is in.
    ///
    /// **English, always, in WP0.** WP5 replaces the body with the settings override and
    /// then the operating system's UI language; every caller already goes through here, so
    /// that change is one function rather than a search for hard-coded `"en"`.
    #[must_use]
    pub fn system() -> Self {
        Self::for_locale("en")
    }

    /// One string, with no placeholders in it.
    #[must_use]
    pub fn text(&self, key: &str) -> String {
        self.template(key).to_string()
    }

    /// One string, with its placeholders filled.
    #[must_use]
    pub fn format(&self, key: &str, params: &[(&str, &str)]) -> String {
        interpolate(self.template(key), params)
    }

    /// The chosen language, then English, then the key itself.
    ///
    /// Returning the key makes an untranslated string obvious instead of rendering as a gap.
    fn template<'a>(&'a self, key: &'a str) -> &'a str {
        self.messages
            .get(key)
            .or_else(|| self.fallback.get(key))
            .map_or(key, String::as_str)
    }
}

#[cfg(test)]
mod tests {
    use super::{Strings, interpolate, primary_subtag};

    #[test]
    fn a_tag_narrows_to_its_language_and_an_unknown_one_falls_back_to_english() {
        assert_eq!(primary_subtag("tr-TR"), "tr");
        assert_eq!(primary_subtag("EN_gb"), "en");

        let turkish = Strings::for_locale("tr-TR");
        assert_eq!(turkish.text("tray.menu.quit"), "Çık");

        let german = Strings::for_locale("de-DE");
        assert_eq!(german.text("tray.menu.quit"), "Quit");

        // A key nobody wrote renders as itself, not as an empty menu entry.
        assert_eq!(turkish.text("tray.menu.nothing"), "tray.menu.nothing");
    }

    #[test]
    fn placeholders_are_filled_and_the_unfilled_ones_stay_visible() {
        assert_eq!(
            interpolate("hold {hotkey}", &[("hotkey", "Ctrl+Alt+Space")]),
            "hold Ctrl+Alt+Space"
        );
        assert_eq!(interpolate("{a} and {b}", &[("a", "one")]), "one and {b}");

        let english = Strings::system();
        assert_eq!(
            english.format("tray.tooltip.idle", &[("hotkey", "Ctrl+Alt+Space")]),
            "Dile — hold Ctrl+Alt+Space to dictate"
        );
    }
}
