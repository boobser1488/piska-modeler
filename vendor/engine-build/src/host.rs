//! Программы с окном без окна консоли (Windows) — то, что нужно им во время работы.
//!
//! * [`command`] / [`no_console`] — запуск дочерних процессов (cargo, cmake, git, игры)
//!   без вспыхивающего окна консоли (`CREATE_NO_WINDOW`); вывод по-прежнему можно
//!   перехватывать (`Stdio::piped`).
//! * [`gui_startup`] — первое, что делает `main` программы с окном (подсистема Windows, см.
//!   [`crate::windows_app`]): запущена из терминала — подключиться к его консоли, чтобы
//!   `--help`, журнал и паника печатались туда; запущена из проводника — журнал (stdout,
//!   stderr) в файл `<каталог настроек>/logs/<программа>.log` (прошлый — `.old.log`), а
//!   паника показывает окно с сообщением и путём к журналу, а не исчезает молча.
//!
//! На других системах `command` — обычный `Command::new`, `gui_startup` ничего не делает.

use std::ffi::OsStr;
use std::path::{Path, PathBuf};
use std::process::Command;

/// Флаг `CreateProcess`: новый процесс без окна консоли.
pub const CREATE_NO_WINDOW: u32 = 0x0800_0000;

/// Флаги создания процесса, которые ставит [`no_console`] (0 не на Windows).
pub fn creation_flags() -> u32 {
    if cfg!(windows) {
        CREATE_NO_WINDOW
    } else {
        0
    }
}

/// Запускать `command` без окна консоли (Windows). Возвращает его же для цепочки.
pub fn no_console(command: &mut Command) -> &mut Command {
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        command.creation_flags(CREATE_NO_WINDOW);
    }
    command
}

/// `Command::new(program)` без окна консоли — вместо `Command::new` в программах с окном.
pub fn command(program: impl AsRef<OsStr>) -> Command {
    let mut command = Command::new(program);
    no_console(&mut command);
    command
}

/// Куда ушёл вывод программы после [`gui_startup`].
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Output {
    /// Своя консоль (программа собрана консольной, `PISKA_CONSOLE=1`) или вывод
    /// перенаправлен запустившим (тест, `> file`) — ничего не трогали.
    Inherited,
    /// Подключились к консоли терминала, из которого запустили.
    ParentConsole,
    /// Консоли нет: stdout и stderr пишутся в этот файл.
    LogFile(PathBuf),
    /// Не Windows — ничего не делали.
    Unchanged,
}

/// Каталог журналов: `%APPDATA%/<config_dir_name>/logs` (Linux, macOS — `~/.config/…`).
pub fn log_dir(config_dir_name: &str) -> Option<PathBuf> {
    let base = std::env::var_os("APPDATA")
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(|home| PathBuf::from(home).join(".config")))?;
    Some(base.join(config_dir_name).join("logs"))
}

/// Файл журнала программы `program` в каталоге `dir`; прежний файл больше 1 МБ становится
/// `<program>.old.log` (один предыдущий хранится).
pub fn rotate_log(dir: &Path, program: &str) -> std::io::Result<PathBuf> {
    std::fs::create_dir_all(dir)?;
    let file = dir.join(format!("{program}.log"));
    if std::fs::metadata(&file).is_ok_and(|m| m.len() > 1024 * 1024) {
        let _ = std::fs::rename(&file, dir.join(format!("{program}.old.log")));
    }
    Ok(file)
}

/// Начало `main` программы с окном: см. описание модуля. `config_dir_name` — каталог
/// настроек бренда (`engine_i18n::brand::CONFIG_DIR_NAME`).
pub fn gui_startup(program: &str, config_dir_name: &str) -> Output {
    #[cfg(windows)]
    {
        let output = win::attach_or_log(program, config_dir_name);
        win::install_panic_box(program, &output);
        output
    }
    #[cfg(not(windows))]
    {
        let _ = (program, config_dir_name);
        Output::Unchanged
    }
}

#[cfg(windows)]
mod win {
    use super::Output;
    use std::os::windows::io::IntoRawHandle;
    use windows_sys::Win32::Foundation::{HANDLE, INVALID_HANDLE_VALUE};
    use windows_sys::Win32::System::Console::{
        AttachConsole, GetConsoleWindow, GetStdHandle, SetStdHandle, ATTACH_PARENT_PROCESS, STD_ERROR_HANDLE,
        STD_OUTPUT_HANDLE,
    };

    fn valid(handle: HANDLE) -> bool {
        !handle.is_null() && handle != INVALID_HANDLE_VALUE
    }

    pub(super) fn attach_or_log(program: &str, config_dir_name: &str) -> Output {
        // SAFETY: вызовы Win32 без указателей на данные Rust; дескрипторы проверяются.
        unsafe {
            if !GetConsoleWindow().is_null() || valid(GetStdHandle(STD_ERROR_HANDLE)) {
                return Output::Inherited;
            }
            if AttachConsole(ATTACH_PARENT_PROCESS) != 0 {
                // Свежеподключённая консоль: вывод — в неё (stdout / stderr ещё пусты).
                for (name, which) in [("CONOUT$", STD_OUTPUT_HANDLE), ("CONOUT$", STD_ERROR_HANDLE)] {
                    if let Ok(file) = std::fs::OpenOptions::new().write(true).open(name) {
                        SetStdHandle(which, file.into_raw_handle() as HANDLE);
                    }
                }
                return Output::ParentConsole;
            }
        }
        let Some(file) = super::log_dir(config_dir_name).and_then(|dir| super::rotate_log(&dir, program).ok()) else {
            return Output::Unchanged;
        };
        let Ok(log) = std::fs::OpenOptions::new().create(true).append(true).open(&file) else {
            return Output::Unchanged;
        };
        let Ok(second) = log.try_clone() else {
            return Output::Unchanged;
        };
        // SAFETY: дескрипторы файла отдаются процессу навсегда (Rust их больше не закрывает).
        unsafe {
            SetStdHandle(STD_OUTPUT_HANDLE, log.into_raw_handle() as HANDLE);
            SetStdHandle(STD_ERROR_HANDLE, second.into_raw_handle() as HANDLE);
        }
        Output::LogFile(file)
    }

    /// Паника без консоли: окно с сообщением и путём к журналу (сообщение уходит и в журнал).
    pub(super) fn install_panic_box(program: &str, output: &Output) {
        let Output::LogFile(file) = output.clone() else {
            return;
        };
        let program = program.to_string();
        let previous = std::panic::take_hook();
        std::panic::set_hook(Box::new(move |info| {
            previous(info);
            let text = format!(
                "{program} stopped because of an internal error:\n\n{info}\n\nThe log is in {}",
                file.display()
            );
            let wide = |s: &str| s.encode_utf16().chain(std::iter::once(0)).collect::<Vec<u16>>();
            let (text, title) = (wide(&text), wide(&program));
            // SAFETY: строки UTF-16 с нулём на конце живут до конца вызова.
            unsafe {
                windows_sys::Win32::UI::WindowsAndMessaging::MessageBoxW(
                    std::ptr::null_mut(),
                    text.as_ptr(),
                    title.as_ptr(),
                    windows_sys::Win32::UI::WindowsAndMessaging::MB_ICONERROR,
                );
            }
        }));
    }
}


// ---------------------------------------------------------------------------
// Показать в файловом менеджере
// ---------------------------------------------------------------------------

/// Команда «показать в файловом менеджере», собранная [`reveal_command`].
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RevealCommand {
    pub program: String,
    /// Аргументы; на Windows — одна строка как есть (`raw_arg`), без кавычек Rust.
    pub args: Vec<String>,
    /// Аргументы передаются как есть (Windows: `/select,"C:\…"` одним куском).
    pub raw: bool,
}

/// Путь для системы: абсолютный, без `\\?\`, с `\` на Windows, без `\` в конце.
pub fn native_path(path: &Path) -> PathBuf {
    let absolute = if path.is_absolute() {
        path.to_path_buf()
    } else {
        std::env::current_dir().map(|dir| dir.join(path)).unwrap_or_else(|_| path.to_path_buf())
    };
    let absolute = absolute.canonicalize().unwrap_or(absolute);
    let mut text = absolute.display().to_string();
    if cfg!(windows) {
        if let Some(rest) = text.strip_prefix(r"\\?\UNC\") {
            text = format!(r"\\{rest}");
        } else if let Some(rest) = text.strip_prefix(r"\\?\") {
            text = rest.to_string();
        }
        text = text.replace('/', "\\");
        while text.len() > 3 && text.ends_with('\\') {
            text.pop();
        }
    } else {
        while text.len() > 1 && text.ends_with('/') {
            text.pop();
        }
    }
    PathBuf::from(text)
}

/// Команда, которая покажет `path` в файловом менеджере: существующий файл или каталог —
/// выделенным в его каталоге (Windows `explorer /select,"…"`, macOS `open -R`), иначе
/// открыть ближайший существующий каталог-предок; Linux — `xdg-open` каталога.
pub fn reveal_command(path: &Path) -> RevealCommand {
    let native = native_path(path);
    let existing = native.exists();
    let folder = if existing {
        native.clone()
    } else {
        native.ancestors().find(|a| a.is_dir()).map(Path::to_path_buf).unwrap_or_else(|| native.clone())
    };
    if cfg!(windows) {
        // Кавычки — вокруг пути внутри одного аргумента: иначе Rust возьмёт в кавычки весь
        // `/select,…` с пробелами, и проводник откроет «Документы» или рабочий стол.
        let arg = if existing { format!("/select,\"{}\"", native.display()) } else { format!("\"{}\"", folder.display()) };
        RevealCommand { program: "explorer".to_string(), args: vec![arg], raw: true }
    } else if cfg!(target_os = "macos") {
        if existing {
            RevealCommand { program: "open".to_string(), args: vec!["-R".to_string(), native.display().to_string()], raw: false }
        } else {
            RevealCommand { program: "open".to_string(), args: vec![folder.display().to_string()], raw: false }
        }
    } else {
        let dir = if native.is_dir() { native.clone() } else { native.parent().map(Path::to_path_buf).unwrap_or(folder) };
        let dir = if dir.is_dir() { dir } else { dir.ancestors().find(|a| a.is_dir()).map(Path::to_path_buf).unwrap_or(dir) };
        RevealCommand { program: "xdg-open".to_string(), args: vec![dir.display().to_string()], raw: false }
    }
}

/// Показать `path` в файловом менеджере (см. [`reveal_command`]). Код выхода проводника не
/// проверяется: он 1 и при успехе.
pub fn reveal_in_file_manager(path: &Path) -> std::io::Result<()> {
    let plan = reveal_command(path);
    let mut command = command(&plan.program);
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        if plan.raw {
            for arg in &plan.args {
                command.raw_arg(arg);
            }
        } else {
            command.args(&plan.args);
        }
    }
    #[cfg(not(windows))]
    command.args(&plan.args);
    command.spawn().map(|_| ())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_spawn_helper_hides_the_console_and_still_captures_output() {
        if cfg!(windows) {
            assert_eq!(creation_flags(), CREATE_NO_WINDOW);
            let out = command("cmd").args(["/C", "echo hello"]).output().expect("cmd runs");
            assert!(String::from_utf8_lossy(&out.stdout).contains("hello"), "output is still captured");
        } else {
            assert_eq!(creation_flags(), 0);
        }
    }


    #[test]
    fn reveal_selects_existing_files_with_spaces_cyrillic_and_odd_spellings() {
        let dir = std::env::temp_dir().join(format!("piska reveal тест {}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let file = dir.join("модель с пробелом.pmodel");
        std::fs::write(&file, b"x").unwrap();
        let spelled = PathBuf::from(file.display().to_string().replace('\\', "/"));
        let plan = reveal_command(&spelled);
        if cfg!(windows) {
            assert_eq!(plan.program, "explorer");
            assert!(plan.raw);
            let expected = format!("/select,\"{}\"", native_path(&file).display());
            assert_eq!(plan.args, [expected.clone()]);
            assert!(!expected.contains('/') || expected.starts_with("/select"), "no forward slashes in the path");
            assert!(expected.contains("модель с пробелом.pmodel"));
            // `\\?\` и слэш в конце убираются.
            let verbatim = PathBuf::from(format!(r"\\?\{}\", native_path(&dir).display()));
            assert_eq!(native_path(&verbatim), native_path(&dir));
            assert!(!native_path(&verbatim).display().to_string().starts_with(r"\\?"));
        }
        // Файла нет — открыть ближайший существующий каталог.
        let missing = dir.join("нет такого").join("файла.obj");
        let plan = reveal_command(&missing);
        let joined = plan.args.join(" ");
        assert!(joined.contains(&native_path(&dir).display().to_string()), "{joined}");
        assert!(!joined.contains("/select"));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn logs_rotate_into_the_brand_config_folder() {
        let dir = std::env::temp_dir().join(format!("piska-log-test-{}", std::process::id()));
        let file = rotate_log(&dir, "tool").unwrap();
        assert_eq!(file, dir.join("tool.log"));
        std::fs::write(&file, vec![b'x'; 1024 * 1024 + 1]).unwrap();
        let again = rotate_log(&dir, "tool").unwrap();
        assert_eq!(again, file);
        assert!(dir.join("tool.old.log").is_file(), "a big log is kept as .old.log");
        assert!(!file.exists(), "and a fresh one starts");
        let _ = std::fs::remove_dir_all(&dir);
        assert!(log_dir("piska").is_some_and(|d| d.ends_with(Path::new("piska").join("logs"))));
    }
}
