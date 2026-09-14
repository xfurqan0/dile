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
//! **The language is chosen in one place.** [`Strings::system`] reads the operating system's
//! UI language and [`Strings::for_setting`] puts the user's choice in front of it, which is
//! the whole of the language decision: the tray, the dialogs and the settings window all come
//! out of the same catalogue, so they cannot end up in two different languages. The catalogue
//! is replaceable at run time — `docs/PROJECT.md` §6 WP6 asks for a language switch without a
//! restart, and the switch is in WP5's settings window.

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

/// The language Windows is set to, as a primary subtag, or `"en"` when it will not say.
///
/// The one place the operating system is asked. `docs/PROJECT.md` §3 ships EN and TR, so a
/// machine set to anything else lands on English through [`Strings::for_locale`] rather than
/// being special-cased here.
#[must_use]
pub fn system_locale() -> String {
    sys_locale::get_locale()
        .map(|locale| primary_subtag(&locale))
        .filter(|locale| !locale.is_empty())
        .unwrap_or_else(|| "en".to_owned())
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

/// The prefix of the eight keys that name a physical modifier key.
///
/// The second half is the trigger's own spelling — `hotkey.key.RightCtrl` — so the catalogue
/// and `dile_hotkey::ModifierOnly`'s `Display` cannot drift apart without the fallback below
/// showing it.
const KEY_PREFIX: &str = "hotkey.key.";

/// The trigger as a person reads it, in the language the interface is in.
///
/// A chord comes back exactly as it is written: `Ctrl+Alt+Space` is already the notation every
/// application on the machine uses, and translating `Ctrl` would make a settings file and a
/// tooltip disagree about the same key. A **lone modifier** does not have that luxury —
/// `RightCtrl` is a spelling for a settings file, not a phrase — so it is looked up as
/// `hotkey.key.<spelling>` and comes back as "Right Ctrl" or "Sağ Ctrl".
///
/// A string that is not a trigger at all is returned unchanged, and so is one whose key is
/// missing from the catalogue. Both are the same decision as [`Strings::template`]'s: a label
/// that shows something wrong is better than a tooltip with a hole in it.
#[must_use]
pub fn trigger_label(strings: &Strings, trigger: &str) -> String {
    let Ok(parsed) = trigger.parse::<dile_hotkey::Trigger>() else {
        return trigger.to_owned();
    };
    let written = parsed.to_string();
    if parsed.is_lone_key() {
        let key = format!("{KEY_PREFIX}{written}");
        let text = strings.text(&key);
        // `text` answers with the key itself when nobody wrote the string, which would put
        // `hotkey.key.RightCtrl` in a tooltip.
        if text != key {
            return text;
        }
    }
    written
}

/// The strings of one language, with English behind them.
#[derive(Debug)]
pub struct Strings {
    locale: String,
    messages: BTreeMap<String, String>,
    fallback: BTreeMap<String, String>,
}

impl Strings {
    /// The catalogue for a language tag such as `tr`, `tr-TR` or `en-GB`.
    #[must_use]
    pub fn for_locale(locale: &str) -> Self {
        let primary = primary_subtag(locale);
        // A language this build does not carry *is* English, rather than being English with
        // a foreign name on it: the webview stamps this on the document's `lang`, and a page
        // that says it is German while reading English is a page a screen reader mispronounces.
        let (locale, messages) = match source(&primary) {
            // English is the fallback; loading it twice would only double the memory.
            Some(_) if primary == "en" => (primary, BTreeMap::new()),
            Some(text) => (primary, parse(text)),
            None => ("en".to_owned(), BTreeMap::new()),
        };
        Strings {
            locale,
            messages,
            fallback: parse(EN),
        }
    }

    /// The language the operating system is set to.
    #[must_use]
    pub fn system() -> Self {
        Self::for_locale(&system_locale())
    }

    /// The language the user asked for, falling back to the operating system's.
    ///
    /// The `Option` is the "Automatic" row of the setting: a language tag when the user
    /// picked one, `None` when they left it to the machine.
    #[must_use]
    pub fn for_setting(tag: Option<&str>) -> Self {
        match tag {
            Some(tag) => Self::for_locale(tag),
            None => Self::system(),
        }
    }

    /// The language this catalogue is in, as a primary subtag.
    #[must_use]
    pub fn locale(&self) -> &str {
        &self.locale
    }

    /// Every string of this language, English behind it, as the webview reads them.
    ///
    /// The settings window resolves its own `data-i18n` attributes, so it needs the whole
    /// catalogue rather than one key at a time — a window with sixty labels would otherwise
    /// be sixty round trips over the IPC boundary before it could paint.
    #[must_use]
    pub fn all(&self) -> BTreeMap<String, String> {
        let mut merged = self.fallback.clone();
        for (key, value) in &self.messages {
            merged.insert(key.clone(), value.clone());
        }
        merged
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
    use super::{Strings, interpolate, primary_subtag, trigger_label};
    use std::collections::BTreeMap;

    #[test]
    fn a_tag_narrows_to_its_language_and_an_unknown_one_falls_back_to_english() {
        assert_eq!(primary_subtag("tr-TR"), "tr");
        assert_eq!(primary_subtag("EN_gb"), "en");

        let turkish = Strings::for_locale("tr-TR");
        assert_eq!(turkish.text("tray.menu.quit"), "Çık");

        let german = Strings::for_locale("de-DE");
        assert_eq!(german.text("tray.menu.quit"), "Quit");
        assert_eq!(
            german.locale(),
            "en",
            "a language this build does not carry is English, not German-with-English-in-it"
        );
        assert_eq!(turkish.locale(), "tr");

        // A key nobody wrote renders as itself, not as an empty menu entry.
        assert_eq!(turkish.text("tray.menu.nothing"), "tray.menu.nothing");
    }

    #[test]
    fn the_whole_catalogue_is_english_with_the_chosen_language_on_top() {
        let turkish = Strings::for_locale("tr");
        let all = turkish.all();

        assert_eq!(all.get("tray.menu.quit").map(String::as_str), Some("Çık"));
        // Every English key is present even if a translation ever went missing, because the
        // settings window resolves its labels out of this map and a gap would render as a key.
        let english = Strings::for_locale("en");
        for key in english.all().keys() {
            assert!(
                all.contains_key(key),
                "{key} is missing from the merged catalogue"
            );
        }
    }

    #[test]
    fn the_system_language_is_one_this_build_carries() {
        // Whatever this machine is set to, the answer is a catalogue Dile ships.
        let system = Strings::system();
        assert!(
            matches!(system.locale(), "en" | "tr"),
            "{}",
            system.locale()
        );
    }

    #[test]
    fn placeholders_are_filled_and_the_unfilled_ones_stay_visible() {
        assert_eq!(
            interpolate("hold {hotkey}", &[("hotkey", "Ctrl+Alt+Space")]),
            "hold Ctrl+Alt+Space"
        );
        assert_eq!(interpolate("{a} and {b}", &[("a", "one")]), "one and {b}");

        // Not `system()`: this machine's own UI language is whatever the maintainer set it
        // to, and a test that changes answer with Windows' regional settings is not a test.
        let english = Strings::for_locale("en");
        assert_eq!(
            english.format("tray.tooltip.idle", &[("hotkey", "Ctrl+Alt+Space")]),
            "Dile — hold Ctrl+Alt+Space to dictate"
        );
    }

    #[test]
    fn a_lone_modifier_is_named_in_words_and_a_chord_is_passed_through() {
        let english = Strings::for_locale("en");
        let turkish = Strings::for_locale("tr");

        // The shipped trigger, in both languages.
        assert_eq!(trigger_label(&english, "RightCtrl"), "Right Ctrl");
        assert_eq!(trigger_label(&turkish, "RightCtrl"), "Sağ Ctrl");
        assert_eq!(trigger_label(&english, "LeftMeta"), "Left Win");

        // A spelling a person typed into the file by hand still resolves, because the label
        // is taken from the parsed trigger rather than from the string.
        assert_eq!(trigger_label(&english, "right-ctrl"), "Right Ctrl");

        // A chord is notation, not prose: it is the same in every language.
        assert_eq!(trigger_label(&english, "Ctrl+Alt+Space"), "Ctrl+Alt+Space");
        assert_eq!(trigger_label(&turkish, "Ctrl+Alt+Space"), "Ctrl+Alt+Space");
        assert_eq!(
            trigger_label(&english, " alt + ctrl + space "),
            "Ctrl+Alt+Space",
            "a chord is written back in one order, whatever order it was typed in"
        );

        // A string that is not a trigger at all is handed back as it came.
        assert_eq!(trigger_label(&english, "Ctrl"), "Ctrl");
        assert_eq!(trigger_label(&english, ""), "");

        // And a catalogue with nothing in it — which is what a damaged locale file becomes —
        // falls back to the trigger's own spelling rather than to the key it looked up.
        let nothing = Strings {
            locale: "en".to_owned(),
            messages: BTreeMap::new(),
            fallback: BTreeMap::new(),
        };
        assert_eq!(trigger_label(&nothing, "RightCtrl"), "RightCtrl");
        assert_eq!(trigger_label(&nothing, "Ctrl+Alt+Space"), "Ctrl+Alt+Space");
    }
}
