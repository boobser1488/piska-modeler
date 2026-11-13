use super::*;
use std::sync::Mutex;

/// Тесты, которые меняют язык процесса, идут по очереди.
static LANGUAGE: Mutex<()> = Mutex::new(());

const SAMPLE: &str = r#"
# header
msgid ""
msgstr "Language: ru\n"

#: somewhere.rs:1
msgid "Save"
msgstr "Сохранить"

msgctxt "menu"
msgid "File"
msgstr "Файл"

msgid "File"
msgstr "Файл (слово)"

msgid "{n} object"
msgid_plural "{n} objects"
msgstr[0] "{n} объект"
msgstr[1] "{n} объекта"
msgstr[2] "{n} объектов"

msgid "Long "
"line"
msgstr "Длинная "
"строка\n"

msgid "Not yet"
msgstr ""
"#;

fn sample() -> Catalogue {
    Catalogue::parse_strict(SAMPLE, Language::Russian).expect("the sample is well formed")
}

#[test]
fn lookup_context_and_fallback() {
    let c = sample();
    assert_eq!(c.translate(None, "Save"), Some("Сохранить"));
    assert_eq!(c.translate(Some("menu"), "File"), Some("Файл"));
    assert_eq!(c.translate(None, "File"), Some("Файл (слово)"));
    assert_eq!(c.translate(None, "Missing"), None);
    // Пустой msgstr — не переведено.
    assert_eq!(c.translate(None, "Not yet"), None);
    assert_eq!(c.untranslated().count(), 1);
    // Строки продолжаются, экранирование разбирается.
    assert_eq!(c.translate(None, "Long line"), Some("Длинная строка\n"));
    // Заголовок не попадает в записи.
    assert_eq!(c.len(), 6);
    // Английский — ключ как есть.
    assert_eq!(tr_in(Language::English, "Save"), "Save");
    assert_eq!(tr_in(Language::Russian, "No such key anywhere"), "No such key anywhere");
}

#[test]
fn plural_rule_russian() {
    let ru = Language::Russian;
    let forms = |n: u64| ru.plural_index(n);
    for n in [1, 21, 31, 101, 1001] {
        assert_eq!(forms(n), 0, "{n}");
    }
    for n in [2, 3, 4, 22, 23, 24, 102, 1004] {
        assert_eq!(forms(n), 1, "{n}");
    }
    for n in [0, 5, 6, 10, 11, 12, 13, 14, 15, 19, 20, 25, 100, 111, 112, 114, 1000] {
        assert_eq!(forms(n), 2, "{n}");
    }
    assert_eq!(Language::English.plural_index(1), 0);
    assert_eq!(Language::English.plural_index(0), 1);
    assert_eq!(Language::English.plural_index(2), 1);
}

#[test]
fn plural_lookup_and_english_fallback() {
    let c = sample();
    let word = |n: u64| c.translate_plural("{n} object", n, Language::Russian).unwrap();
    let rendered: Vec<String> =
        [1u64, 2, 5, 11, 21, 101, 102, 111].iter().map(|n| format_named(word(*n), &[("n", n)])).collect();
    assert_eq!(
        rendered,
        ["1 объект", "2 объекта", "5 объектов", "11 объектов", "21 объект", "101 объект", "102 объекта", "111 объектов"]
    );
    // Английский без каталога: один / другое.
    assert_eq!(tr_n_in(Language::English, "{n} object", "{n} objects", 1), "{n} object");
    assert_eq!(tr_n_in(Language::English, "{n} object", "{n} objects", 0), "{n} objects");
    // Нет записи — английские формы даже у русского.
    assert_eq!(tr_n_in(Language::Russian, "{n} gizmo", "{n} gizmos", 5), "{n} gizmos");
}

#[test]
fn placeholders_are_substituted() {
    let one = 1;
    assert_eq!(format_named("{a} + {b} = {c}", &[("a", &one), ("b", &2), ("c", &"three")]), "1 + 2 = three");
    assert_eq!(format_named("{{literal}} {a}", &[("a", &one)]), "{literal} 1");
    // Неизвестное имя остаётся видимым, строка не ломается.
    assert_eq!(format_named("{a} {zzz}", &[("a", &one)]), "1 {zzz}");
    assert_eq!(format_named("unclosed {a", &[("a", &one)]), "unclosed {a");
    assert_eq!(format_named("кириллица {a}!", &[("a", &one)]), "кириллица 1!");
    assert_eq!(placeholders("{n} of {total} {n} {{x}}"), ["n", "total"]);
    let copied = trf!("Copied {count} item(s)", count = 7);
    assert!(copied.contains('7'));
}

#[test]
fn loader_reports_problems_and_keeps_the_rest() {
    let text = r#"
msgid "Good"
msgstr "Хорошо"

msgid "Good"
msgstr "Дубль"

msgid "Broken
msgstr "x"

msgid "Placeholder {a}"
msgstr "Подстановка {b}"

msgid "{n} thing"
msgid_plural "{n} things"
msgstr[0] "{n} вещь"
msgstr[1] "{n} вещи"

msgid "No translation line"

msgstr "Сирота"

bogus line
"#;
    let (catalogue, errors) = Catalogue::parse(text, Language::Russian);
    let messages: Vec<String> = errors.iter().map(|e| e.to_string()).collect();
    let has = |needle: &str| messages.iter().any(|m| m.contains(needle));
    assert!(has("duplicate entry `Good`"), "{messages:?}");
    assert!(has("double quotes"), "{messages:?}");
    assert!(has("placeholders"), "{messages:?}");
    assert!(has("plural forms"), "{messages:?}");
    assert!(has("no msgstr"), "{messages:?}");
    assert!(has("unknown keyword"), "{messages:?}");
    // Годная запись пережила все ошибки вокруг.
    assert_eq!(catalogue.translate(None, "Good"), Some("Хорошо"));
    assert!(Catalogue::parse_strict(text, Language::Russian).is_err());
    // Строки ошибок — номера строк файла.
    assert!(errors.iter().all(|e| e.line >= 1));
}

#[test]
fn quote_round_trips() {
    for text in ["plain", "with \"quotes\"", "line\nbreak", "back\\slash", "tab\there", "кириллица «ёлка»"] {
        let literal = format!("msgid {}\nmsgstr {}\n", catalogue::quote(text), catalogue::quote(text));
        let c = Catalogue::parse_strict(&literal, Language::Russian).unwrap();
        assert_eq!(c.translate(None, text), Some(text));
    }
}

#[test]
fn locale_tags_and_environment() {
    assert_eq!(Language::from_locale_tag("ru_RU.UTF-8"), Some(Language::Russian));
    assert_eq!(Language::from_locale_tag("ru-RU"), Some(Language::Russian));
    assert_eq!(Language::from_locale_tag("RU"), Some(Language::Russian));
    assert_eq!(Language::from_locale_tag("en_US.UTF-8"), Some(Language::English));
    assert_eq!(Language::from_locale_tag("rum"), None);
    assert_eq!(Language::from_locale_tag("de_DE"), None);
    assert_eq!(Language::from_code("ru"), Some(Language::Russian));
    assert_eq!(Language::from_code(" Русский "), Some(Language::Russian));
    assert_eq!(Language::from_code("fr"), None);
    // Windows LANGID: 0x0419 — русский (Россия), 0x0409 — английский (США), 0x0422 — украинский.
    assert_eq!(Language::from_windows_langid(0x0419), Language::Russian);
    assert_eq!(Language::from_windows_langid(0x0409), Language::English);
    assert_eq!(Language::from_windows_langid(0x0422), Language::English);

    let env = |pairs: &'static [(&'static str, &'static str)]| {
        move |name: &str| pairs.iter().find(|(k, _)| *k == name).map(|(_, v)| v.to_string())
    };
    assert_eq!(Language::from_env(env(&[("LANG", "ru_RU.UTF-8")])), Language::Russian);
    assert_eq!(Language::from_env(env(&[("LANG", "en_US.UTF-8")])), Language::English);
    // LC_ALL главнее LANG; LANGUAGE — список через двоеточие, берётся первый.
    assert_eq!(Language::from_env(env(&[("LC_ALL", "en_US"), ("LANG", "ru_RU")])), Language::English);
    assert_eq!(Language::from_env(env(&[("LANGUAGE", "ru:en"), ("LANG", "en_US")])), Language::Russian);
    // C / POSIX и пустое значение пропускаются.
    assert_eq!(Language::from_env(env(&[("LC_ALL", "C"), ("LANG", "ru_RU")])), Language::Russian);
    assert_eq!(Language::from_env(env(&[])), Language::English);
    // «Auto» — язык системы; явный выбор не зависит от системы.
    assert_eq!(resolve(Some(Language::Russian)), Language::Russian);
    assert_eq!(resolve(Some(Language::English)), Language::English);
    let _ = resolve(None);
}

#[test]
fn language_switches_at_runtime() {
    let _guard = LANGUAGE.lock().unwrap_or_else(|e| e.into_inner());
    set(Language::English);
    assert_eq!(current(), Language::English);
    assert_eq!(tr("Save"), "Save");
    set(Language::Russian);
    assert_eq!(current(), Language::Russian);
    assert_eq!(tr("Save"), "Сохранить");
    assert_eq!(tr_ctx("menu", "File"), "Файл");
    assert_eq!(trn!("{n} element", "{n} elements", 21), "21 элемент");
    assert_eq!(trn!("{n} element", "{n} elements", 3), "3 элемента");
    assert_eq!(trn!("{n} element", "{n} elements", 0), "0 элементов");
    set(Language::English);
    assert_eq!(tr("Save"), "Save");
    assert_eq!(trn!("{n} element", "{n} elements", 1), "1 element");
    assert_eq!(trn!("{n} element", "{n} elements", 5), "5 elements");
}

/// Вшитый каталог русского разбирается без ошибок, переведён целиком и не теряет
/// подстановки; в нём нет английских остатков и «машинных» кавычек.
#[test]
fn embedded_russian_catalogue_is_well_formed() {
    assert!(russian_errors().is_empty(), "locale/ru.po has errors:\n{}", russian_errors().iter().map(|e| e.to_string()).collect::<Vec<_>>().join("\n"));
    let catalogue = Language::Russian.catalogue().unwrap();
    assert!(!catalogue.is_empty());
    let untranslated: Vec<&str> = catalogue.untranslated().map(|e| e.id.as_str()).collect();
    assert!(untranslated.is_empty(), "untranslated entries: {untranslated:?}");
    for entry in catalogue.entries() {
        for form in &entry.forms {
            assert!(!form.starts_with(' ') && !form.ends_with(' ') || entry.id.starts_with(' ') || entry.id.ends_with(' '), "stray space around `{form}`");
            // Прямые кавычки допустимы только внутри кода в обратных кавычках.
            assert!((!form.contains('"') || entry.id.contains('"')) && !form.contains("''"), "use «» quotes in `{form}`");
            assert!(entry.id.contains("  ") || !form.contains("  "), "double space in `{form}`");
        }
    }
}
