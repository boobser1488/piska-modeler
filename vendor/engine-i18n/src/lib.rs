//! `engine-i18n` — перевод интерфейса инструментов Piska (редактор, хаб, редактор моделей).
//!
//! Слой в стиле gettext: ключ — **английский текст**, поэтому в коде интерфейса остаётся
//! читаемый английский, а непереведённая строка показывается как есть.
//!
//! ```
//! use engine_i18n::{tr, trf, trn, tr_ctx, Language};
//!
//! engine_i18n::set(Language::Russian);
//! assert_eq!(tr("Add Component"), "Добавить компонент");
//! assert_eq!(tr_ctx("menu", "File"), "Файл");
//! assert_eq!(trf!("Reloaded {path}", path = "a.png"), "Перезагружено: a.png");
//! assert_eq!(trn!("{n} element", "{n} elements", 2), "2 элемента");
//! engine_i18n::set(Language::English);
//! assert_eq!(tr("Add Component"), "Add Component");
//! ```
//!
//! * [`tr`] / [`tr_ctx`] — строка; [`trf!`] — с именованными подстановками `{имя}`;
//!   [`tr_n`] / [`trn!`] — с формой множественного числа (у русского три: 1, 2–4, 5–20).
//! * [`n_`] — пометка строки, которая переводится позже (статические таблицы):
//!   `const NAMES: [&str; 2] = [n_("Move"), n_("Rotate")];` и `tr(NAMES[i])` при показе.
//! * [`set`] / [`current`] — язык процесса. Интерфейс на egui рисуется заново каждый
//!   кадр, поэтому после [`set`] новый язык виден сразу, без перезапуска.
//! * Каталог русского — `locale/ru.po`, встроен в программу (формат описан в
//!   [`catalogue`]). Инструмент `tools/extract_ui_strings.py` собирает строки из кода
//!   и проверяет каталог (`--check`).
//!
//! Язык по умолчанию при запуске библиотеки — английский: выбирает язык хозяин
//! (настройка Preferences → Language, [`Language::system`] для значения «Auto»).
#![deny(unsafe_code)]


use std::borrow::Cow;
use std::fmt::Display;
use std::hash::{BuildHasherDefault, Hasher};
use std::sync::atomic::{AtomicU8, Ordering};
use std::sync::OnceLock;

pub mod brand;
pub use catalogue::{placeholders, Catalogue, CatalogueError, Entry};

/// Быстрый хешер для коротких ключей (FNV-1a): интерфейс ищет сотни строк за кадр.
#[derive(Default)]
pub(crate) struct FnvHasher(u64);

impl Hasher for FnvHasher {
    fn finish(&self) -> u64 {
        self.0
    }

    fn write(&mut self, bytes: &[u8]) {
        let mut hash = if self.0 == 0 { 0xcbf2_9ce4_8422_2325 } else { self.0 };
        for byte in bytes {
            hash ^= u64::from(*byte);
            hash = hash.wrapping_mul(0x100_0000_01b3);
        }
        self.0 = hash;
    }
}

pub(crate) type Fnv = BuildHasherDefault<FnvHasher>;

/// Язык интерфейса.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Language {
    English,
    Russian,
}

impl Language {
    /// Все языки в порядке меню.
    pub const ALL: [Language; 2] = [Language::English, Language::Russian];

    /// Код языка: `en`, `ru` (так он записан в настройках).
    pub fn code(self) -> &'static str {
        match self {
            Language::English => "en",
            Language::Russian => "ru",
        }
    }

    /// Название языка на нём самом — так оно стоит в меню выбора.
    pub fn native_name(self) -> &'static str {
        match self {
            Language::English => "English",
            Language::Russian => "Русский",
        }
    }

    /// Язык по коду из настроек (`en`, `ru`; регистр и `_RU` / `-RU` не важны).
    pub fn from_code(code: &str) -> Option<Language> {
        let code = code.trim();
        Language::ALL.into_iter().find(|l| {
            code.eq_ignore_ascii_case(l.code()) || code.eq_ignore_ascii_case(l.native_name())
        })
    }

    /// Язык по тегу локали: `ru`, `ru_RU.UTF-8`, `ru-RU`, `en_US`. Незнакомое — `None`.
    pub fn from_locale_tag(tag: &str) -> Option<Language> {
        let tag = tag.trim();
        let primary: String = tag.chars().take_while(|c| c.is_ascii_alphabetic()).collect::<String>().to_ascii_lowercase();
        let rest = &tag[primary.len()..];
        if !(rest.is_empty() || rest.starts_with(['_', '-', '.', '@'])) {
            return None;
        }
        match primary.as_str() {
            "ru" => Some(Language::Russian),
            "en" => Some(Language::English),
            _ => None,
        }
    }

    /// Язык по `LANGID` Windows (`GetUserDefaultUILanguage`): младшие 10 бит — язык.
    pub fn from_windows_langid(langid: u16) -> Language {
        if langid & 0x3ff == 0x19 {
            Language::Russian
        } else {
            Language::English
        }
    }

    /// Язык системы: на Windows — язык интерфейса пользователя, иначе — переменные
    /// `LC_ALL`, `LC_MESSAGES`, `LANGUAGE`, `LANG`. Не русский — английский.
    pub fn system() -> Language {
        #[cfg(windows)]
        {
            Language::from_windows_langid(sys::user_ui_langid())
        }
        #[cfg(not(windows))]
        {
            Language::from_env(|name| std::env::var(name).ok())
        }
    }

    /// Язык по переменным окружения (первая заданная из `LC_ALL`, `LC_MESSAGES`,
    /// `LANGUAGE`, `LANG`; у `LANGUAGE` — первый из списка через `:`).
    pub fn from_env(get: impl Fn(&str) -> Option<String>) -> Language {
        for name in ["LC_ALL", "LC_MESSAGES", "LANGUAGE", "LANG"] {
            let Some(value) = get(name).filter(|v| !v.trim().is_empty()) else {
                continue;
            };
            let first = value.split(':').next().unwrap_or("");
            if matches!(first, "C" | "POSIX") {
                continue;
            }
            return Language::from_locale_tag(first).unwrap_or(Language::English);
        }
        Language::English
    }

    /// Сколько форм множественного числа у языка.
    pub fn plural_forms(self) -> usize {
        match self {
            Language::English => 2,
            Language::Russian => 3,
        }
    }

    /// Номер формы для числа `n`: английский — 0 при 1, иначе 1; русский — 0 (1, 21,
    /// 101…), 1 (2–4, 22–24…), 2 (0, 5–20, 25–30…).
    pub fn plural_index(self, n: u64) -> usize {
        match self {
            Language::English => usize::from(n != 1),
            Language::Russian => {
                let (mod10, mod100) = (n % 10, n % 100);
                if mod10 == 1 && mod100 != 11 {
                    0
                } else if (2..=4).contains(&mod10) && !(12..=14).contains(&mod100) {
                    1
                } else {
                    2
                }
            }
        }
    }

    /// Каталог языка (английскому каталог не нужен).
    pub fn catalogue(self) -> Option<&'static Catalogue> {
        match self {
            Language::English => None,
            Language::Russian => Some(russian()),
        }
    }
}

/// Настройка языка: `None` — как в системе (Auto).
pub fn resolve(choice: Option<Language>) -> Language {
    choice.unwrap_or_else(Language::system)
}

/// Текст каталога русского, вшитый в программу.
pub const RU_PO: &str = include_str!("../locale/ru.po");

static RUSSIAN: OnceLock<(Catalogue, Vec<CatalogueError>)> = OnceLock::new();

fn russian_parsed() -> &'static (Catalogue, Vec<CatalogueError>) {
    RUSSIAN.get_or_init(|| Catalogue::parse(RU_PO, Language::Russian))
}

fn russian() -> &'static Catalogue {
    &russian_parsed().0
}

/// Ошибки вшитого каталога русского (пусто, если файл в порядке; их проверяет тест).
pub fn russian_errors() -> &'static [CatalogueError] {
    &russian_parsed().1
}

// ---------------------------------------------------------------------------
// Язык процесса
// ---------------------------------------------------------------------------

static CURRENT: AtomicU8 = AtomicU8::new(0);

/// Текущий язык интерфейса.
pub fn current() -> Language {
    match CURRENT.load(Ordering::Relaxed) {
        1 => Language::Russian,
        _ => Language::English,
    }
}

/// Сменить язык интерфейса. Действует сразу: следующий кадр рисует новый текст.
pub fn set(language: Language) {
    CURRENT.store(
        match language {
            Language::English => 0,
            Language::Russian => 1,
        },
        Ordering::Relaxed,
    );
}

// ---------------------------------------------------------------------------
// Перевод
// ---------------------------------------------------------------------------

/// Перевести строку на текущий язык. Нет перевода — сам английский текст.
pub fn tr(key: &str) -> Cow<'_, str> {
    tr_in(current(), key)
}

/// То же для явно названного языка (тесты, предпросмотр, не трогает язык процесса).
pub fn tr_in(language: Language, key: &str) -> Cow<'_, str> {
    match language.catalogue().and_then(|c| c.translate(None, key)) {
        Some(text) => Cow::Borrowed(text),
        None => Cow::Borrowed(key),
    }
}

/// То же для `&'static str`: результат тоже `&'static str` (каталог живёт всю программу), так
/// что `tr_s("Save")` годится везде, где был литерал, — аргумент `&str`, ветка `if` / `match`,
/// поле структуры. Для таблиц в `const` ключ помечают `n_`, а `tr_s` вызывают при показе.
pub fn tr_s(key: &'static str) -> &'static str {
    match current().catalogue().and_then(|c| c.translate(None, key)) {
        Some(text) => text,
        None => key,
    }
}

/// [`tr_s`] с контекстом.
pub fn tr_ctx_s(context: &str, key: &'static str) -> &'static str {
    match current().catalogue().and_then(|c| c.translate(Some(context), key)) {
        Some(text) => text,
        None => key,
    }
}

/// Перевести с контекстом: `tr_ctx("menu", "File")` — «Файл», а `tr("File")` может
/// значить другое. Нет записи с контекстом — английский `key`.
pub fn tr_ctx<'a>(context: &str, key: &'a str) -> Cow<'a, str> {
    tr_ctx_in(current(), context, key)
}

pub fn tr_ctx_in<'a>(language: Language, context: &str, key: &'a str) -> Cow<'a, str> {
    match language.catalogue().and_then(|c| c.translate(Some(context), key)) {
        Some(text) => Cow::Borrowed(text),
        None => Cow::Borrowed(key),
    }
}

/// Форма множественного числа. `one` — ключ (английское единственное), `other` —
/// английское множественное. В тексте обычно стоит `{n}`; подставляет [`trn!`].
pub fn tr_n<'a>(one: &'a str, other: &'a str, n: u64) -> Cow<'a, str> {
    tr_n_in(current(), one, other, n)
}

pub fn tr_n_in<'a>(language: Language, one: &'a str, other: &'a str, n: u64) -> Cow<'a, str> {
    if let Some(text) = language.catalogue().and_then(|c| c.translate_plural(one, n, language)) {
        return Cow::Borrowed(text);
    }
    Cow::Borrowed(if n == 1 { one } else { other })
}

/// Пометка «этот текст переводится позже» — для таблиц и констант, где вызвать [`tr`]
/// нельзя. Ничего не делает; её видит инструмент `extract_ui_strings.py`.
pub const fn n_(text: &'static str) -> &'static str {
    text
}

/// Подставить `{имя}` в шаблон. `{{` и `}}` — фигурные скобки. Неизвестное имя
/// остаётся как есть (видно, что подстановка потеряна).
pub fn format_named(template: &str, args: &[(&str, &dyn Display)]) -> String {
    let mut out = String::with_capacity(template.len() + 8);
    let mut rest = template;
    let mut next_positional = 0usize;
    while let Some(start) = rest.find(['{', '}']) {
        out.push_str(&rest[..start]);
        let tail = &rest[start..];
        if let Some(after) = tail.strip_prefix("{{") {
            out.push('{');
            rest = after;
        } else if let Some(after) = tail.strip_prefix("}}") {
            out.push('}');
            rest = after;
        } else if tail.starts_with('{') {
            match tail.find('}') {
                Some(end) => {
                    // `{имя}`, `{}` (следующий по порядку), `{0}`; всё после `:` (формат) игнорируется.
                    let name = tail[1..end].split(':').next().unwrap_or("");
                    let positional;
                    let name = if name.is_empty() {
                        positional = next_positional.to_string();
                        next_positional += 1;
                        positional.as_str()
                    } else {
                        name
                    };
                    match args.iter().find(|(n, _)| *n == name) {
                        Some((_, value)) => out.push_str(&value.to_string()),
                        None => out.push_str(&tail[..=end]),
                    }
                    rest = &tail[end + 1..];
                }
                None => {
                    out.push_str(tail);
                    rest = "";
                }
            }
        } else {
            out.push('}');
            rest = &tail[1..];
        }
    }
    out.push_str(rest);
    out
}

/// Перевести шаблон с подстановками по порядку: `{}` или `{0}`, `{1}` (то же, что
/// `trf!("…{}…", a, b)`). Нужен для шаблонов, пришедших из `format!("…{}…", a)`.
pub fn tr_fmt_pos(key: &str, args: &[&dyn Display]) -> String {
    const NAMES: [&str; 12] = ["0", "1", "2", "3", "4", "5", "6", "7", "8", "9", "10", "11"];
    let named: Vec<(&str, &dyn Display)> = args.iter().enumerate().take(NAMES.len()).map(|(i, v)| (NAMES[i], *v)).collect();
    format_named(&tr(key), &named)
}

/// Перевести шаблон и подставить значения (то же, что [`trf!`] без макроса).
pub fn tr_fmt(key: &str, args: &[(&str, &dyn Display)]) -> String {
    format_named(&tr(key), args)
}

/// Строка с подстановками: `trf!("Saved {path}", path = p.display())`.
#[macro_export]
macro_rules! trf {
    ($key:expr $(, $name:ident = $value:expr)* $(,)?) => {
        $crate::tr_fmt($key, &[$((stringify!($name), &$value as &dyn ::std::fmt::Display)),*])
    };
    // Подстановки по порядку, как в `format!("…{}…", a, b)`.
    ($key:expr, $($value:expr),+ $(,)?) => {
        $crate::tr_fmt_pos($key, &[$(&($value) as &dyn ::std::fmt::Display),+])
    };
}

/// Множественное число с числом: `trn!("{n} element", "{n} elements", count)`; `{n}`
/// — число, остальные подстановки — как в [`trf!`].
#[macro_export]
macro_rules! trn {
    ($one:expr, $other:expr, $count:expr $(, $name:ident = $value:expr)* $(,)?) => {{
        let count = $count as u64;
        $crate::format_named(
            &$crate::tr_n($one, $other, count),
            &[("n", &count as &dyn ::std::fmt::Display) $(, (stringify!($name), &$value as &dyn ::std::fmt::Display))*],
        )
    }};
}

// ---------------------------------------------------------------------------
// Язык системы Windows
// ---------------------------------------------------------------------------

#[cfg(windows)]
mod sys {
    // Единственное место с `unsafe` во всём слое: один вызов kernel32, который есть
    // в каждом процессе Windows и не требует буферов.
    #[allow(unsafe_code)]
    pub fn user_ui_langid() -> u16 {
        #[link(name = "kernel32")]
        extern "system" {
            fn GetUserDefaultUILanguage() -> u16;
        }
        // SAFETY: функция без аргументов, возвращает число.
        unsafe { GetUserDefaultUILanguage() }
    }
}

/// Каталог переводов: разбор текстового формата `.po` (подмножество gettext) и поиск.
///
/// Формат — обычный текст, его правят руками или в любом редакторе `.po` (Poedit и т. п.):
///
/// ```text
/// # Комментарий переводчика.
/// #: crates/engine-modeler/src/ui/mod.rs:123   <- откуда ключ (пишет инструмент)
/// msgid "Add Component"
/// msgstr "Добавить компонент"
///
/// # Контекст отличает одинаковые слова: «File» меню и «File» как слово в тексте.
/// msgctxt "menu"
/// msgid "File"
/// msgstr "Файл"
///
/// # Множественное число: msgid — единственное, msgid_plural — для остальных чисел
/// # английского. Форм столько, сколько у языка: у русского три (1 / 2–4 / 0, 5–20).
/// msgid "{n} object"
/// msgid_plural "{n} objects"
/// msgstr[0] "{n} объект"
/// msgstr[1] "{n} объекта"
/// msgstr[2] "{n} объектов"
/// ```
///
/// Строки — в двойных кавычках, продолжаются на следующих строках теми же кавычками;
/// экранирование `\n`, `\t`, `\"`, `\\`. Подстановки — `{имя}` (в переводе те же имена,
/// что в ключе). Пустой `msgstr` — «не переведено»: показывается английский текст.
///
/// Загрузчик не останавливается на первой ошибке: он собирает все (повтор ключа, строка
/// без кавычки, `msgstr` без `msgid`, чужие подстановки в переводе, не то число форм),
/// остальные записи остаются рабочими.
pub mod catalogue {
    use std::collections::HashMap;
    use std::fmt;

    use super::Language;

    /// Ошибка каталога с номером строки (с 1) — для сообщения «файл:строка».
    #[derive(Clone, Debug, PartialEq, Eq)]
    pub struct CatalogueError {
        pub line: usize,
        pub message: String,
    }

    impl fmt::Display for CatalogueError {
        fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
            write!(f, "line {}: {}", self.line, self.message)
        }
    }

    /// Одна запись каталога.
    #[derive(Clone, Debug, PartialEq, Eq)]
    pub struct Entry {
        pub context: Option<String>,
        pub id: String,
        pub id_plural: Option<String>,
        /// Переводы: одна строка либо по форме множественного числа. Пустая строка —
        /// «не переведено».
        pub forms: Vec<String>,
        /// Строка `msgid` в файле (для сообщений).
        pub line: usize,
    }

    impl Entry {
        /// Есть ли настоящий перевод (все формы непусты).
        pub fn is_translated(&self) -> bool {
            !self.forms.is_empty() && self.forms.iter().all(|f| !f.is_empty())
        }
    }

    type Map<V> = HashMap<Box<str>, V, super::Fnv>;

    /// Разобранный каталог одного языка.
    #[derive(Default, Debug)]
    pub struct Catalogue {
        entries: Vec<Entry>,
        plain: Map<usize>,
        with_context: Map<Map<usize>>,
    }

    impl Catalogue {
        /// Разобрать текст. Возвращает каталог из всех годных записей и список ошибок.
        pub fn parse(text: &str, language: Language) -> (Catalogue, Vec<CatalogueError>) {
            let mut errors = Vec::new();
            let raw = parse_raw(text, &mut errors);
            let mut catalogue = Catalogue::default();
            let forms_needed = language.plural_forms();
            for entry in raw {
                let line = entry.line;
                if entry.id.is_empty() {
                    // Заголовок каталога (`msgid ""`) — игнорируется.
                    continue;
                }
                if entry.id_plural.is_some() && entry.forms.len() != forms_needed && entry.forms.iter().any(|f| !f.is_empty()) {
                    errors.push(CatalogueError {
                        line,
                        message: format!(
                            "`{}` has {} plural forms, {} needs {}",
                            entry.id,
                            entry.forms.len(),
                            language.code(),
                            forms_needed
                        ),
                    });
                    continue;
                }
                if entry.id_plural.is_none() && entry.forms.len() != 1 {
                    errors.push(CatalogueError { line, message: format!("`{}` has no single msgstr", entry.id) });
                    continue;
                }
                if let Some(message) = placeholder_mismatch(&entry) {
                    errors.push(CatalogueError { line, message });
                    continue;
                }
                if catalogue.find(entry.context.as_deref(), &entry.id).is_some() {
                    errors.push(CatalogueError { line, message: format!("duplicate entry `{}`", entry.id) });
                    continue;
                }
                catalogue.insert(entry);
            }
            (catalogue, errors)
        }

        /// Как [`Catalogue::parse`], но любая ошибка — отказ.
        pub fn parse_strict(text: &str, language: Language) -> Result<Catalogue, Vec<CatalogueError>> {
            let (catalogue, errors) = Self::parse(text, language);
            if errors.is_empty() {
                Ok(catalogue)
            } else {
                Err(errors)
            }
        }

        fn insert(&mut self, entry: Entry) {
            let index = self.entries.len();
            match &entry.context {
                None => {
                    self.plain.insert(entry.id.clone().into_boxed_str(), index);
                }
                Some(context) => {
                    self.with_context
                        .entry(context.clone().into_boxed_str())
                        .or_default()
                        .insert(entry.id.clone().into_boxed_str(), index);
                }
            }
            self.entries.push(entry);
        }

        /// Запись по контексту и ключу.
        pub fn find(&self, context: Option<&str>, id: &str) -> Option<&Entry> {
            let index = match context {
                None => self.plain.get(id)?,
                Some(context) => self.with_context.get(context)?.get(id)?,
            };
            self.entries.get(*index)
        }

        /// Перевод без форм множественного числа; `None` — нет записи или не переведено.
        pub fn translate(&self, context: Option<&str>, id: &str) -> Option<&str> {
            let entry = self.find(context, id)?;
            let first = entry.forms.first()?;
            (!first.is_empty()).then_some(first.as_str())
        }

        /// Перевод с формой множественного числа по правилу языка.
        pub fn translate_plural(&self, id: &str, n: u64, language: Language) -> Option<&str> {
            let entry = self.find(None, id)?;
            entry.id_plural.as_ref()?;
            let form = entry.forms.get(language.plural_index(n))?;
            (!form.is_empty()).then_some(form.as_str())
        }

        /// Все записи в порядке файла.
        pub fn entries(&self) -> &[Entry] {
            &self.entries
        }

        pub fn len(&self) -> usize {
            self.entries.len()
        }

        pub fn is_empty(&self) -> bool {
            self.entries.is_empty()
        }

        /// Записи без перевода.
        pub fn untranslated(&self) -> impl Iterator<Item = &Entry> {
            self.entries.iter().filter(|e| !e.is_translated())
        }
    }

    /// Имена подстановок `{имя}` в тексте (без `{{` и `}}`), по порядку, без повторов.
    pub fn placeholders(text: &str) -> Vec<String> {
        let mut names: Vec<String> = Vec::new();
        let bytes = text.as_bytes();
        let mut i = 0;
        while i < bytes.len() {
            match bytes[i] {
                b'{' if bytes.get(i + 1) == Some(&b'{') => i += 2,
                b'{' => {
                    if let Some(end) = text[i + 1..].find('}') {
                        let name = &text[i + 1..i + 1 + end];
                        if !name.is_empty()
                            && name.chars().all(|c| c.is_alphanumeric() || c == '_')
                            && !names.iter().any(|n| n == name)
                        {
                            names.push(name.to_string());
                        }
                        i += end + 2;
                    } else {
                        i += 1;
                    }
                }
                _ => i += 1,
            }
        }
        names
    }

    fn placeholder_mismatch(entry: &Entry) -> Option<String> {
        let mut expected = placeholders(&entry.id);
        if let Some(plural) = &entry.id_plural {
            for name in placeholders(plural) {
                if !expected.contains(&name) {
                    expected.push(name);
                }
            }
        }
        expected.sort();
        for form in entry.forms.iter().filter(|f| !f.is_empty()) {
            let mut found = placeholders(form);
            found.sort();
            if found != expected {
                return Some(format!(
                    "translation of `{}` uses placeholders {:?}, the source text uses {:?}",
                    entry.id, found, expected
                ));
            }
        }
        None
    }

    #[derive(PartialEq, Clone, Copy)]
    enum Field {
        Context,
        Id,
        IdPlural,
        Str(usize),
    }

    fn parse_raw(text: &str, errors: &mut Vec<CatalogueError>) -> Vec<Entry> {
        let mut entries = Vec::new();
        let mut context: Option<String> = None;
        let mut id: Option<String> = None;
        let mut id_plural: Option<String> = None;
        let mut forms: Vec<String> = Vec::new();
        let mut id_line = 0;
        let mut field: Option<Field> = None;

        fn finish(
            entries: &mut Vec<Entry>,
            context: &mut Option<String>,
            id: &mut Option<String>,
            id_plural: &mut Option<String>,
            forms: &mut Vec<String>,
            id_line: usize,
            errors: &mut Vec<CatalogueError>,
        ) {
            if let Some(found) = id.take() {
                if forms.is_empty() {
                    errors.push(CatalogueError { line: id_line, message: format!("`{found}` has no msgstr") });
                } else {
                    entries.push(Entry {
                        context: context.take(),
                        id: found,
                        id_plural: id_plural.take(),
                        forms: std::mem::take(forms),
                        line: id_line,
                    });
                }
            } else if context.is_some() || !forms.is_empty() || id_plural.is_some() {
                errors.push(CatalogueError { line: id_line.max(1), message: "msgstr or msgctxt without msgid".to_string() });
            }
            *context = None;
            *id_plural = None;
            forms.clear();
        }

        for (index, raw_line) in text.lines().enumerate() {
            let line_no = index + 1;
            let line = raw_line.trim();
            if line.is_empty() {
                finish(&mut entries, &mut context, &mut id, &mut id_plural, &mut forms, id_line, errors);
                field = None;
                continue;
            }
            if line.starts_with('#') {
                continue;
            }
            let (keyword, rest) = match line.split_once(char::is_whitespace) {
                Some((k, r)) => (k, r.trim()),
                None => (line, ""),
            };
            let new_field = match keyword {
                "msgctxt" => Some(Field::Context),
                "msgid" => Some(Field::Id),
                "msgid_plural" => Some(Field::IdPlural),
                "msgstr" => Some(Field::Str(0)),
                k if k.starts_with("msgstr[") && k.ends_with(']') => match k["msgstr[".len()..k.len() - 1].parse::<usize>() {
                    Ok(n) => Some(Field::Str(n)),
                    Err(_) => {
                        errors.push(CatalogueError { line: line_no, message: format!("bad plural index in `{k}`") });
                        field = None;
                        continue;
                    }
                },
                _ => None,
            };
            let (target, literal) = match new_field {
                Some(f) => {
                    // Запись начинается с msgctxt или msgid; всё, что после msgstr, — новая запись.
                    if matches!(f, Field::Context | Field::Id) && matches!(field, Some(Field::Str(_))) {
                        finish(&mut entries, &mut context, &mut id, &mut id_plural, &mut forms, id_line, errors);
                    }
                    if f == Field::Id {
                        id_line = line_no;
                    }
                    field = Some(f);
                    (f, rest)
                }
                None if line.starts_with('"') => match field {
                    Some(f) => (f, line),
                    None => {
                        errors.push(CatalogueError { line: line_no, message: "a string outside of an entry".to_string() });
                        continue;
                    }
                },
                None => {
                    errors.push(CatalogueError { line: line_no, message: format!("unknown keyword `{keyword}`") });
                    continue;
                }
            };
            let value = match unquote(literal) {
                Ok(v) => v,
                Err(message) => {
                    errors.push(CatalogueError { line: line_no, message });
                    continue;
                }
            };
            let continuation = new_field.is_none();
            match target {
                Field::Context => append(&mut context, value, continuation),
                Field::Id => append(&mut id, value, continuation),
                Field::IdPlural => append(&mut id_plural, value, continuation),
                Field::Str(n) => {
                    if forms.len() <= n {
                        forms.resize(n + 1, String::new());
                    }
                    forms[n].push_str(&value);
                }
            }
        }
        finish(&mut entries, &mut context, &mut id, &mut id_plural, &mut forms, id_line, errors);
        entries
    }

    fn append(slot: &mut Option<String>, value: String, continuation: bool) {
        match slot {
            Some(existing) if continuation => existing.push_str(&value),
            _ => *slot = Some(value),
        }
    }

    fn unquote(literal: &str) -> Result<String, String> {
        let literal = literal.trim();
        let Some(inner) = literal.strip_prefix('"').and_then(|s| s.strip_suffix('"')) else {
            return Err(format!("expected a string in double quotes, found `{literal}`"));
        };
        let mut out = String::with_capacity(inner.len());
        let mut chars = inner.chars();
        while let Some(c) = chars.next() {
            if c != '\\' {
                if c == '"' {
                    return Err("an unescaped quote inside a string (write \\\")".to_string());
                }
                out.push(c);
                continue;
            }
            match chars.next() {
                Some('n') => out.push('\n'),
                Some('t') => out.push('\t'),
                Some('"') => out.push('"'),
                Some('\\') => out.push('\\'),
                Some(other) => return Err(format!("unknown escape `\\{other}`")),
                None => return Err("a string ends with a lone backslash".to_string()),
            }
        }
        Ok(out)
    }

    /// Записать строку в формате каталога (для инструментов, которые дополняют файл).
    pub fn quote(text: &str) -> String {
        let mut out = String::with_capacity(text.len() + 2);
        out.push('"');
        for c in text.chars() {
            match c {
                '\n' => out.push_str("\\n"),
                '\t' => out.push_str("\\t"),
                '"' => out.push_str("\\\""),
                '\\' => out.push_str("\\\\"),
                other => out.push(other),
            }
        }
        out.push('"');
        out
    }
}

#[cfg(test)]
mod tests;
