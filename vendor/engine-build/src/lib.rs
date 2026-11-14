//! `engine-build` — помощник для `build.rs`: значок приложения в `.exe` на Windows.
//!
//! Одна строка в `build.rs` игры — и её программа получает значок в проводнике, на панели
//! задач и в Alt-Tab:
//!
//! ```toml
//! # Cargo.toml игры
//! [build-dependencies]
//! engine-build = { path = "../../crates/engine-build" }
//! ```
//!
//! ```text
//! // build.rs игры
//! fn main() {
//!     engine_build::windows_icon();
//! }
//! ```
//!
//! Какой значок берётся (по порядку):
//!
//! 1. переменная окружения `PISKA_ICON` — путь к PNG; её выставляет `piska-export windows`,
//!    если в `[package.metadata.piska]` задан `icon = "…"`;
//! 2. `assets/icon.png` рядом с `Cargo.toml` игры (PNG ≥ 256×256 лучше всего);
//! 3. значок движка (`assets/branding/icon.ico`).
//!
//! Из PNG собирается многоразмерный `.ico` (16…256 px). Ресурс записывается в COFF-объект
//! без `rc.exe` и Windows SDK ([`coff`]) и передаётся линкеру как `rustc-link-arg-bins` /
//! `-examples`. На всех целях, кроме Windows (Linux, Android, веб), функция ничего не делает,
//! так что кросс-сборки из Windows не ломаются.
//!
//! Проверка результата: [`read_exe_icon`] достаёт значок из готового exe.

pub mod coff;
pub mod file_dialog;
pub mod host;
pub mod ico;
pub mod pe;

use std::path::{Path, PathBuf};

pub use coff::{icon_resource_object, Arch};
pub use ico::{ico_from_png, parse_ico, write_ico, IconImage, ICO_SIZES};
pub use pe::{read_exe_icon, read_exe_subsystem, SUBSYSTEM_CONSOLE, SUBSYSTEM_GUI};

/// Значок движка: многоразмерный `.ico`, собранный `tools/gen_icon.py`.
pub const ENGINE_ICO: &[u8] = include_bytes!("../../../crates/piska-modeler/assets/icon.ico");
/// Тот же значок, PNG 256 px.
pub const ENGINE_ICON_PNG: &[u8] = include_bytes!("../../../crates/piska-modeler/assets/icon.png");

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("bad icon file: {0}")]
    BadIco(String),
    #[error("the icon is not a readable PNG ({0}); save it as a PNG image, ideally 256x256 or larger")]
    BadPng(String),
    #[error("not an executable with a resource table: {0}")]
    BadPe(String),
    #[error("cannot read the icon `{path}`: {reason}. Check the path in `PISKA_ICON` or put the icon at assets/icon.png")]
    Read { path: String, reason: String },
}

/// Значок для `build.rs` игры: см. описание крейта. Ничего не делает не на Windows.
pub fn windows_icon() {
    if let Err(e) = try_windows_icon(None) {
        // Сборка без значка лучше, чем сломанная сборка, но ошибку надо видеть.
        println!("cargo:warning=engine-build: no application icon embedded: {e}");
    }
}

/// Программа с окном: значок ([`windows_icon`]) и подсистема Windows «GUI» для программ
/// (`bin`) и примеров пакета — в отладочной сборке тоже, так что рядом с окном не
/// открывается консоль. Тесты (`cargo test`) остаются консольными. Запущенная из
/// терминала программа подключается к его консоли сама ([`host::gui_startup`]).
///
/// `PISKA_CONSOLE=1` при сборке — оставить консоль (разработчику, которому она нужна).
pub fn windows_app() {
    windows_icon();
    windows_subsystem();
}

/// Только подсистема «GUI» (без значка) — см. [`windows_app`].
pub fn windows_subsystem() {
    println!("cargo:rerun-if-env-changed=PISKA_CONSOLE");
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() != Ok("windows")
        || std::env::var("PISKA_CONSOLE").is_ok_and(|v| !v.is_empty() && v != "0")
    {
        return;
    }
    let args: &[&str] = if std::env::var("CARGO_CFG_TARGET_ENV").as_deref() == Ok("msvc") {
        &["/SUBSYSTEM:WINDOWS", "/ENTRY:mainCRTStartup"]
    } else {
        &["-Wl,--subsystem,windows"]
    };
    let manifest = PathBuf::from(std::env::var_os("CARGO_MANIFEST_DIR").unwrap_or_default());
    let text = std::fs::read_to_string(manifest.join("Cargo.toml")).unwrap_or_default();
    let has_bins = manifest.join("src").join("main.rs").is_file() || manifest.join("src").join("bin").is_dir() || text.contains("[[bin]]");
    let has_examples = manifest.join("examples").is_dir() || text.contains("[[example]]");
    for arg in args {
        if has_bins {
            println!("cargo:rustc-link-arg-bins={arg}");
        }
        if has_examples {
            println!("cargo:rustc-link-arg-examples={arg}");
        }
    }
}

/// То же, но с явным файлом: `.png` (масштабируется) или готовый `.ico`; путь — от каталога
/// с `Cargo.toml` игры. Ошибка чтения файла — ошибка сборки (файл указан явно).
pub fn windows_icon_file(path: impl AsRef<Path>) {
    if let Err(e) = try_windows_icon(Some(path.as_ref())) {
        panic!("engine-build: {e}");
    }
}

fn try_windows_icon(explicit: Option<&Path>) -> Result<(), Error> {
    println!("cargo:rerun-if-env-changed=PISKA_ICON");
    println!("cargo:rerun-if-changed=build.rs");
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() != Ok("windows") {
        return Ok(());
    }
    let Some(arch) = std::env::var("CARGO_CFG_TARGET_ARCH").ok().and_then(|a| Arch::from_cargo(&a)) else {
        return Ok(()); // неизвестная архитектура Windows — без значка
    };
    let manifest = PathBuf::from(std::env::var_os("CARGO_MANIFEST_DIR").unwrap_or_default());
    let source = match explicit {
        Some(path) => Some(manifest.join(path)),
        None => match std::env::var_os("PISKA_ICON").filter(|v| !v.is_empty()) {
            Some(path) => Some(PathBuf::from(path)),
            None => Some(manifest.join("assets").join("icon.png")).filter(|p| p.is_file()),
        },
    };
    let ico = match &source {
        None => ENGINE_ICO.to_vec(),
        Some(path) => {
            println!("cargo:rerun-if-changed={}", path.display());
            let bytes = std::fs::read(path).map_err(|e| Error::Read {
                path: path.display().to_string(),
                reason: e.to_string(),
            })?;
            if bytes.starts_with(b"\x89PNG") {
                ico_from_png(&bytes)?
            } else {
                parse_ico(&bytes)?; // проверка: это действительно .ico
                bytes
            }
        }
    };
    let object = icon_resource_object(&parse_ico(&ico)?, arch)?;
    let out = PathBuf::from(std::env::var_os("OUT_DIR").unwrap_or_default()).join("piska_icon.obj");
    std::fs::write(&out, object).map_err(|e| Error::Read { path: out.display().to_string(), reason: e.to_string() })?;
    // Cargo считает ошибкой `rustc-link-arg-bins` у пакета без bin-целей (и так же для
    // примеров), поэтому смотрим, какие цели есть.
    let manifest_text = std::fs::read_to_string(manifest.join("Cargo.toml")).unwrap_or_default();
    let has_bins = manifest.join("src").join("main.rs").is_file()
        || manifest.join("src").join("bin").is_dir()
        || manifest_text.contains("[[bin]]");
    let has_examples = manifest.join("examples").is_dir() || manifest_text.contains("[[example]]");
    if has_bins {
        println!("cargo:rustc-link-arg-bins={}", out.display());
    }
    if has_examples {
        println!("cargo:rustc-link-arg-examples={}", out.display());
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn engine_ico_has_every_size() {
        let images = parse_ico(ENGINE_ICO).unwrap();
        let sizes: Vec<u32> = images.iter().map(|i| i.width).collect();
        assert_eq!(sizes, ICO_SIZES.to_vec());
        for image in &images {
            assert_eq!(image.width, image.height);
            assert!(image.is_png());
            let decoded = image::load_from_memory(&image.data).unwrap();
            assert_eq!(decoded.width(), image.width);
        }
    }

    #[test]
    fn png_becomes_a_multi_size_ico() {
        let ico = ico_from_png(ENGINE_ICON_PNG).unwrap();
        let sizes: Vec<u32> = parse_ico(&ico).unwrap().iter().map(|i| i.width).collect();
        assert_eq!(sizes, ICO_SIZES.to_vec());
    }

    #[test]
    fn bad_input_is_an_error_not_a_panic() {
        assert!(parse_ico(b"nope").is_err());
        assert!(ico_from_png(b"not a png").is_err());
        assert!(read_exe_icon(b"MZ").is_err());
        assert!(icon_resource_object(&[], Arch::X64).is_err());
    }

    #[test]
    fn resource_object_is_deterministic_and_well_formed() {
        let images = parse_ico(ENGINE_ICO).unwrap();
        let a = icon_resource_object(&images, Arch::X64).unwrap();
        let b = icon_resource_object(&images, Arch::X64).unwrap();
        assert_eq!(a, b);
        assert_eq!(u16::from_le_bytes([a[0], a[1]]), 0x8664, "AMD64 machine");
        // Указатель на таблицу символов + 4 символа по 18 байт + таблица строк = конец файла.
        let symbols = u32::from_le_bytes(a[8..12].try_into().unwrap()) as usize;
        assert_eq!(a.len(), symbols + 4 * 18 + 4);
    }
}
