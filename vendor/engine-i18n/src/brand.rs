//! Имя продукта и всё, что от него производно, — в одном месте.
//!
//! Движок будет переименован; чтобы это не превратилось в поиск по всему дереву, новый код
//! (формат файла проекта, диалоги выбора файлов, заголовки окон, справка командной строки,
//! сообщения) берёт названия и расширение файла проекта **отсюда**. Чтобы переименовать
//! продукт для всего, что использует этот модуль, достаточно поменять константы ниже.
//!
//! Что модуль намеренно не покрывает: имена крейтов и программ (`piska-hub`, `piska-export`),
//! переменные окружения (`PISKA_*`), ключи `[package.metadata.piska]` в `Cargo.toml` игр,
//! подписи `tr_s("Piska Hub")` в старых местах интерфейса и русский каталог — это глобальное
//! переименование отдельным заходом (см. `docs/api/project-file.md`, раздел «Название»).
//! Магическое число файла проекта в этот список не входит: оно не содержит названия и не
//! меняется никогда (старые файлы должны читаться всегда).

/// Название продукта: «Piska».
pub const PRODUCT: &str = "Piska";
/// Название движка целиком: «Piska Engine».
pub const ENGINE_NAME: &str = "Piska Engine";
/// Название редактора.
pub const EDITOR_NAME: &str = "Piska Editor";
/// Название стартового окна с проектами.
pub const HUB_NAME: &str = "Piska Hub";
/// Название редактора моделей.
pub const MODELER_NAME: &str = "Piska Modeler";
/// Короткое имя — так начинаются имя каталога настроек, служебные файлы и расширение файла
/// проекта.
pub const SHORT_NAME: &str = "pks";
/// Расширение файла с исходниками проекта — `.<короткое имя>`, без точки, строчными.
pub const PROJECT_EXTENSION: &str = SHORT_NAME;
/// Описание типа файла проекта (диалоги выбора файла, ассоциация файлов в системе).
pub const PROJECT_FILE_DESCRIPTION: &str = "Piska project source";
/// Имя каталога настроек (в `%APPDATA%`, `~/.config`).
pub const CONFIG_DIR_NAME: &str = "piska";

/// Имя файла проекта: `my_game` → `my_game.pks`.
pub fn project_file_name(project: &str) -> String {
    format!("{project}.{PROJECT_EXTENSION}")
}

/// Файл с расширением проекта (без учёта регистра)?
pub fn is_project_file(path: &std::path::Path) -> bool {
    path.extension()
        .and_then(|e| e.to_str())
        .is_some_and(|e| e.eq_ignore_ascii_case(PROJECT_EXTENSION))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_extension_is_derived_from_the_short_name() {
        assert_eq!(PROJECT_EXTENSION, SHORT_NAME);
        assert!(PROJECT_EXTENSION.chars().all(|c| c.is_ascii_lowercase() || c.is_ascii_digit()));
        assert_eq!(project_file_name("my_game"), format!("my_game.{SHORT_NAME}"));
        assert!(is_project_file(std::path::Path::new(&format!("a/b.{}", SHORT_NAME.to_uppercase()))));
        assert!(!is_project_file(std::path::Path::new("a/b.zip")));
        assert!(ENGINE_NAME.starts_with(PRODUCT) && HUB_NAME.starts_with(PRODUCT));
    }
}
