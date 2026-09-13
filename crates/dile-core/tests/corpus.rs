//! The WP4 acceptance corpus: before → after, one table per strictness level.
//!
//! `docs/PROJECT.md` §6 WP4 closes on three things — rule tests, a dictionary that
//! round-trips `ç ğ ı ö ş ü`, and *yani* surviving medium. This file is the first and the
//! third; the second is here too, next to the dictionary unit tests that cover it letter by
//! letter.
//!
//! **The fixtures are written by hand and the vocabulary is neutral on purpose.** The real
//! ASR output that taught us the error shapes — an English term inside a Turkish sentence
//! coming back as the nearest Turkish-sounding word — lives in the benchmark workspace
//! outside this repository, together with the maintainer's own recordings. Copying his
//! sentences in would publish them; copying the product names in would ship somebody's
//! vocabulary as a default. So the shapes are reproduced with terms any Turkish developer
//! would recognise, and nothing here is a transcript of anyone.
//!
//! A row is `(input, expected)`. Rows where the two are equal are not filler: at every
//! level, *what the rules refuse to touch* is as much of the specification as what they
//! change.

use dile_core::cleanup::{Config, Strictness, clean, clean_with};
use dile_core::dictionary::{Dictionary, Entry};

/// Run one table and report every disagreement at once, not just the first.
fn check(level: Strictness, table: &[(&str, &str)]) {
    let mut failures = Vec::new();
    for (input, expected) in table {
        let actual = clean(input, level);
        if actual != *expected {
            failures.push(format!(
                "  {level:?}  {input:?}\n      expected {expected:?}\n      got      {actual:?}"
            ));
        }
    }
    assert!(
        failures.is_empty(),
        "{} of {} rows disagree:\n{}",
        failures.len(),
        table.len(),
        failures.join("\n")
    );
}

/// Light: the filler, the hallucination filter and whitespace hygiene. Nothing else.
const LIGHT: &[(&str, &str)] = &[
    // --- the filler, in every shape it arrives in ---
    ("Eee, bu böyle.", "bu böyle."),
    ("eee sonra ne oldu", "sonra ne oldu"),
    ("Eeee, tamam.", "tamam."),
    ("EEE tamam", "tamam"),
    ("Ee, peki.", "peki."),
    ("eee", ""),
    ("Merhaba eee dünya", "Merhaba dünya"),
    ("Eee, eee, tamam.", "tamam."),
    // --- and the far more common case: there is no filler to remove ---
    ("Bugün hava güzel.", "Bugün hava güzel."),
    (
        "Ne demek istediğini anlamadım.",
        "Ne demek istediğini anlamadım.",
    ),
    ("veee sonra gittik", "veee sonra gittik"),
    ("Bu ne demek", "Bu ne demek"),
    ("Ege bölgesine gittik.", "Ege bölgesine gittik."),
    // --- whole-segment hallucinations ---
    ("Altyazı M.K.", ""),
    ("altyazi m.k", ""),
    ("Altyazı M.K", ""),
    ("Altyazı M.K. Altyazı M.K.", ""),
    ("Çeviri ve Altyazı M.K.", ""),
    ("Merhaba. Altyazı M.K.", "Merhaba."),
    ("İzlediğiniz için teşekkürler.", ""),
    ("izlediğiniz için teşekkür ederim", ""),
    ("Abone olmayı unutmayın.", ""),
    ("Beğenmeyi unutmayın.", ""),
    ("Kanalıma abone olun.", ""),
    ("Bir sonraki videoda görüşürüz.", ""),
    ("www feyyaz tv", ""),
    // --- and what the filter may not touch ---
    (
        "Bu bölümde beğenmeyi unutmayın dedim.",
        "Bu bölümde beğenmeyi unutmayın dedim.",
    ),
    ("Görüşürüz.", "Görüşürüz."),
    ("Afiyet olsun.", "Afiyet olsun."),
    ("Abone ol.", "Abone ol."),
    // --- whitespace and comma hygiene ---
    ("bir  şey  oldu", "bir şey oldu"),
    ("bir şey , oldu", "bir şey, oldu"),
    ("bir şey,oldu", "bir şey, oldu"),
    (", bu böyle", "bu böyle"),
    ("bu böyle , .", "bu böyle."),
    ("  kenarlarda boşluk var  ", "kenarlarda boşluk var"),
    (
        "Bir şey oldu .  Sonra bitti .",
        "Bir şey oldu. Sonra bitti.",
    ),
    ("oran 3,14 oldu", "oran 3,14 oldu"),
    ("çğıöşü korunur", "çğıöşü korunur"),
    ("ÇĞIİÖŞÜ korunur", "ÇĞIİÖŞÜ korunur"),
    // --- nothing from a higher level leaks down ---
    ("bu doğru mu", "bu doğru mu"),
    ("kaç tane var", "kaç tane var"),
    ("cache'i temizledim", "cache'i temizledim"),
    ("bu ne yani", "bu ne yani"),
    ("bir bir saydı", "bir bir saydı"),
    ("tamam tamam olur", "tamam tamam olur"),
    ("bugün geldik. sonra gittik.", "bugün geldik. sonra gittik."),
];

/// Medium, the default: light plus casing, punctuation and the dictionary.
const MEDIUM: &[(&str, &str)] = &[
    // --- sentence-initial capital, with the two Turkish i letters ---
    ("bu böyle.", "Bu böyle."),
    ("işte tam da bu.", "İşte tam da bu."),
    ("izmir güzel.", "İzmir güzel."),
    ("ışık yandı.", "Işık yandı."),
    (
        "bugün hava güzel. yarın yağmur var.",
        "Bugün hava güzel. Yarın yağmur var.",
    ),
    ("2026 yılında oldu.", "2026 yılında oldu."),
    ("İyi günler.", "İyi günler."),
    ("buraya bak", "Buraya bak"),
    // --- the question particle, as the last word ---
    ("Bu doğru mu.", "Bu doğru mu?"),
    ("Bunu görebilir misin.", "Bunu görebilir misin?"),
    ("Sen de gelecek miydin.", "Sen de gelecek miydin?"),
    ("Bu böyle midir.", "Bu böyle midir?"),
    ("bir bakabilir misin.", "Bir bakabilir misin?"),
    ("gelecek misiniz.", "Gelecek misiniz?"),
    ("çalışıyor mu", "Çalışıyor mu?"),
    ("Bunu duydunuz mu.", "Bunu duydunuz mu?"),
    // --- and where it is not the last word, the rule stays out ---
    ("bunu şimdi mi yapayım.", "Bunu şimdi mi yapayım."),
    // --- müdür is a noun as often as it is a particle, so it is not in the list ---
    ("Ahmet müdür.", "Ahmet müdür."),
    // --- the question word, at the start ---
    ("Ne oldu.", "Ne oldu?"),
    ("Neden gecikti.", "Neden gecikti?"),
    ("Nasıl çalışıyor.", "Nasıl çalışıyor?"),
    ("Kaç tane var.", "Kaç tane var?"),
    ("Hangi dosya değişti.", "Hangi dosya değişti?"),
    ("Kim yazdı.", "Kim yazdı?"),
    ("Nerede duruyor.", "Nerede duruyor?"),
    ("Nereye gitti.", "Nereye gitti?"),
    ("Niye durdu.", "Niye durdu?"),
    ("ne zaman geldin.", "Ne zaman geldin?"),
    // --- the speaker's own punctuation outranks the rule ---
    ("Bu doğru mu?", "Bu doğru mu?"),
    ("Ne güzel!", "Ne güzel!"),
    ("Bugün geldi.", "Bugün geldi."),
    ("Bu bir test.", "Bu bir test."),
    // --- comma pile-ups ---
    ("bir şey , , oldu.", "Bir şey, oldu."),
    ("evet, , tamam.", "Evet, tamam."),
    ("bu  böyle ,  oldu.", "Bu böyle, oldu."),
    // --- light still runs underneath ---
    ("eee bu böyle.", "Bu böyle."),
    ("Altyazı M.K.", ""),
    (
        "bugün geldik. eee sonra gittik.",
        "Bugün geldik. Sonra gittik.",
    ),
    // --- and nothing from strict leaks down: the particles all survive ---
    ("yani bu böyle.", "Yani bu böyle."),
    ("işte böyle , yani.", "İşte böyle, yani."),
    ("ya sonra ne oldu.", "Ya sonra ne oldu."),
    ("hani bir şey vardı.", "Hani bir şey vardı."),
    ("Bu şey biraz karışık.", "Bu şey biraz karışık."),
    ("aslında böyle olmalı.", "Aslında böyle olmalı."),
    ("evet, tamam.", "Evet, tamam."),
    ("bir bir saydı.", "Bir bir saydı."),
    ("tamam tamam olur.", "Tamam tamam olur."),
    ("çok çok iyi oldu.", "Çok çok iyi oldu."),
];

/// Strict: medium plus repeat collapsing and the clause-edge particles.
const STRICT: &[(&str, &str)] = &[
    // --- repeats that are not reduplication ---
    ("bunu bunu yapalım.", "Bunu yapalım."),
    ("sistem sistem çöktü.", "Sistem çöktü."),
    ("tamam tamam olur.", "Tamam olur."),
    ("kod kod incelendi.", "Kod incelendi."),
    ("bu bu olmaz.", "Bu olmaz."),
    ("test test test bitti.", "Test bitti."),
    // --- reduplication, which is correct Turkish and stays ---
    ("güle güle dedi.", "Güle güle dedi."),
    ("tek tek saydı.", "Tek tek saydı."),
    ("yavaş yavaş ilerledi.", "Yavaş yavaş ilerledi."),
    ("bir bir saydı.", "Bir bir saydı."),
    ("çok çok güzel oldu.", "Çok çok güzel oldu."),
    ("çok çok çok iyi.", "Çok çok çok iyi."),
    ("sabah sabah kalktı.", "Sabah sabah kalktı."),
    ("adım adım ilerledik.", "Adım adım ilerledik."),
    ("zaman zaman oluyor.", "Zaman zaman oluyor."),
    ("hemen hemen bitti.", "Hemen hemen bitti."),
    ("sık sık geliyor.", "Sık sık geliyor."),
    ("ayrı ayrı baktık.", "Ayrı ayrı baktık."),
    ("bol bol zaman var.", "Bol bol zaman var."),
    ("teker teker geçti.", "Teker teker geçti."),
    ("uzun uzun anlattı.", "Uzun uzun anlattı."),
    ("yer yer bulutlu.", "Yer yer bulutlu."),
    // --- a pause between the two words means the speaker meant both ---
    (
        "bir daha, bir daha söyleyeyim.",
        "Bir daha, bir daha söyleyeyim.",
    ),
    (
        "bir daha bir daha söyleyeyim.",
        "Bir daha bir daha söyleyeyim.",
    ),
    // --- ya / yani / hani at a clause edge ---
    ("yani bu böyle.", "Bu böyle."),
    ("Yani, bu böyle.", "Bu böyle."),
    ("bu böyle, yani.", "Bu böyle."),
    ("hani, bu böyle.", "Bu böyle."),
    ("hani şöyle bir şey vardı.", "Şöyle bir şey vardı."),
    ("ya sonra ne oldu.", "Sonra ne oldu."),
    ("işte böyle, ya.", "İşte böyle."),
    ("eee yani bu böyle.", "Bu böyle."),
    // --- and never mid-clause, which is where it carries meaning ---
    ("ben yani böyle düşünüyorum.", "Ben yani böyle düşünüyorum."),
    ("bunu yani şimdi mi yapalım.", "Bunu yani şimdi mi yapalım."),
    ("ya da böyle olur.", "Ya da böyle olur."),
    // --- standalone şey before a pause ---
    ("ben, şey, düşünüyordum.", "Ben, düşünüyordum."),
    ("bugün biraz şey.", "Bugün biraz."),
    // --- but never the şey that belongs to a noun phrase ---
    ("bir şey oldu.", "Bir şey oldu."),
    ("her şey yolunda.", "Her şey yolunda."),
    ("hiçbir şey olmadı.", "Hiçbir şey olmadı."),
    ("o şey nerede.", "O şey nerede."),
    // --- particles no level deletes ---
    ("aslında böyle olmalı.", "Aslında böyle olmalı."),
    ("evet, tamam.", "Evet, tamam."),
    // --- everything below strict still runs ---
    ("Altyazı M.K.", ""),
    ("bu doğru mu.", "Bu doğru mu?"),
    ("eee bu böyle.", "Bu böyle."),
    ("bir  şey , oldu.", "Bir şey, oldu."),
];

/// What the dictionary does, with a neutral term list assembled for this test alone.
///
/// The shipped dictionary is empty (`docs/PROJECT.md` §5 and `dile_core::dictionary`), so a
/// dictionary corpus has to bring its own terms. These are ordinary technical vocabulary and
/// the mis-hearings are the shape M0 recorded: an English term inside a Turkish sentence
/// coming back as the nearest Turkish-sounding word.
const DICTIONARY: &[(&str, &str)] = &[
    ("kron görevini durdur.", "cron görevini durdur."),
    ("KRON durdu.", "cron durdu."),
    ("keş'i temizle.", "cache'i temizle."),
    ("cache'i temizle.", "cache'i temizle."),
    ("judo ile derle.", "CUDA ile derle."),
    ("Judo hazır.", "CUDA hazır."),
    ("takım sayısı arttı.", "token sayısı arttı."),
    ("kubernetis kümesi hazır.", "Kubernetes kümesi hazır."),
    ("vebhuk geldi.", "webhook geldi."),
    ("tavri projesi.", "Tauri projesi."),
    ("kron ve keş birlikte.", "cron ve cache birlikte."),
    ("cekirdek yüklendi.", "çekirdek yüklendi."),
    // Whole words only: a term inside a longer word is another word.
    ("kronometre bozuldu.", "Kronometre bozuldu."),
    ("takımlar geldi.", "Takımlar geldi."),
];

fn neutral_dictionary() -> Dictionary {
    Dictionary::from_entries([
        Entry::new("cron").with_variant("kron"),
        Entry::new("cache").with_variant("keş").with_variant("Cage"),
        Entry::new("CUDA").with_variant("judo"),
        Entry::new("token").with_variant("takım"),
        Entry::new("Kubernetes").with_variant("kubernetis"),
        Entry::new("webhook").with_variant("vebhuk"),
        Entry::new("Tauri").with_variant("tavri"),
        Entry::new("çekirdek").with_variant("cekirdek"),
    ])
}

#[test]
fn light_level_corpus() {
    check(Strictness::Light, LIGHT);
}

#[test]
fn medium_level_corpus() {
    check(Strictness::Medium, MEDIUM);
}

#[test]
fn strict_level_corpus() {
    check(Strictness::Strict, STRICT);
}

#[test]
fn dictionary_corpus() {
    let config = Config {
        dictionary: neutral_dictionary(),
        ..Config::default()
    };
    let mut failures = Vec::new();
    for (input, expected) in DICTIONARY {
        let actual = clean_with(input, Strictness::Medium, &config).text;
        if actual != *expected {
            failures.push(format!(
                "  {input:?}\n      expected {expected:?}\n      got      {actual:?}"
            ));
        }
    }
    assert!(
        failures.is_empty(),
        "{} of {} rows disagree:\n{}",
        failures.len(),
        DICTIONARY.len(),
        failures.join("\n")
    );
}

/// The acceptance criterion of `docs/PROJECT.md` §6 WP4, on its own so that it cannot be
/// lost in a table.
///
/// *yani* runs at 1.7–2.1 per minute in the maintainer's edit log and he **keeps** it (§4).
/// Medium is the default level, so medium is the level that may not delete it.
#[test]
fn yani_survives_medium() {
    for raw in [
        "yani bu böyle.",
        "ben yani böyle düşünüyorum.",
        "işte böyle, yani.",
        "Yani, tamam.",
    ] {
        let cleaned = clean(raw, Strictness::Medium);
        assert!(
            cleaned.to_lowercase().contains("yani"),
            "medium deleted yani: {raw:?} -> {cleaned:?}"
        );
    }
    // And strict is where it goes, at a clause edge only.
    assert_eq!(clean("yani bu böyle.", Strictness::Strict), "Bu böyle.");
    assert!(
        clean("ben yani böyle düşünüyorum.", Strictness::Strict).contains("yani"),
        "strict deleted a mid-clause yani"
    );
}

/// Cleanup is deterministic and idempotent: running it twice changes nothing the second
/// time.
///
/// This is what "no model, no network, no variance" means in a test. The panel re-runs
/// cleanup every time the strictness chip is touched, and a rule set that drifted on the
/// second pass would make that control unusable.
#[test]
fn cleaning_twice_is_the_same_as_cleaning_once() {
    let tables = [
        (Strictness::Light, LIGHT),
        (Strictness::Medium, MEDIUM),
        (Strictness::Strict, STRICT),
    ];
    for (level, table) in tables {
        for (input, _) in table {
            let once = clean(input, level);
            let twice = clean(&once, level);
            assert_eq!(once, twice, "{level:?} is not idempotent on {input:?}");
        }
    }
}

/// No level invents a letter, and no level strips a diacritic.
///
/// Every rule either deletes a whole word or changes punctuation and case; none of them may
/// transliterate. `docs/PROJECT.md` §5: *"diacritics are never stripped"*.
#[test]
fn no_level_ever_strips_a_diacritic() {
    let raw = "Çağrı geldi, ışık yandı, öğle şöyle güzeldi, ürün hazır.";
    for level in [Strictness::Light, Strictness::Medium, Strictness::Strict] {
        let cleaned = clean(raw, level);
        for letter in ['Ç', 'ğ', 'ı', 'ş', 'ö', 'ü'] {
            assert!(
                cleaned.contains(letter),
                "{level:?} lost {letter}: {cleaned:?}"
            );
        }
    }
}

/// The report is the audit trail WP5's "show raw" and the maintainer's edit log read.
#[test]
fn the_report_keeps_the_raw_text_and_names_every_rule_that_fired() {
    let report = clean_with(
        "eee yani bu  bu böyle , oldu.",
        Strictness::Strict,
        &Config::default(),
    );
    assert_eq!(report.raw, "eee yani bu  bu böyle , oldu.");
    assert_eq!(report.text, "Bu böyle, oldu.");
    assert_eq!(
        report.rules_fired(),
        ["filler", "repeats", "particles", "whitespace", "casing"]
    );
    // Every entry is a real step: the text the rule saw, and the text it handed on.
    for pair in report.changes.windows(2) {
        assert_eq!(pair[0].after, pair[1].before);
    }
    assert_eq!(report.changes.last().unwrap().after, report.text);
}

/// The corpus is WP4's acceptance artefact, so how much of it there is belongs in it.
#[test]
fn every_level_has_a_corpus_worth_the_name() {
    assert!(LIGHT.len() >= 40, "light corpus has {} rows", LIGHT.len());
    assert!(
        MEDIUM.len() >= 40,
        "medium corpus has {} rows",
        MEDIUM.len()
    );
    assert!(
        STRICT.len() >= 40,
        "strict corpus has {} rows",
        STRICT.len()
    );
    assert!(
        DICTIONARY.len() >= 12,
        "dictionary corpus has {} rows",
        DICTIONARY.len()
    );
    eprintln!(
        "corpus rows: light {}, medium {}, strict {}, dictionary {}",
        LIGHT.len(),
        MEDIUM.len(),
        STRICT.len(),
        DICTIONARY.len()
    );
}
