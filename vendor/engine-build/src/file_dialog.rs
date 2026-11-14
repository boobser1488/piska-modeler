//! Системные окна «Открыть», «Сохранить как» и «Выбрать папку» — одни для редактора, хаба
//! и Piska Modeler.
//!
//! * Windows — `GetOpenFileNameW` / `GetSaveFileNameW` (фильтры, папка, имя по умолчанию,
//!   окно-владелец) и `SHBrowseForFolderW` для папок; через уже подключённый `windows-sys`.
//! * Linux — `zenity`, если есть, иначе `kdialog`; нет ни того, ни другого — встроенное окно
//!   программы ([`Outcome::Unavailable`]).
//! * macOS — `osascript` (`choose file` / `choose file name` / `choose folder`).
//!
//! Окно открывается в отдельном потоке ([`Dialog::spawn`]): цикл событий программы не
//! ждёт и не входит в себя повторно. Системные окна включаются явно ([`enable_native`]) —
//! только из `main` оконных программ: тесты никогда не открывают настоящее окно. Тесты
//! подставляют свой ответ через [`set_provider`].

use std::path::{Path, PathBuf};
use std::sync::mpsc::{self, Receiver, TryRecvError};
use std::sync::{Arc, Mutex};

/// Что спросить.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    Open,
    Save,
    Folder,
}

/// Фильтр файлов: «Piska model» + `["pmodel"]`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Filter {
    pub name: String,
    /// Расширения без точки; пусто — все файлы.
    pub extensions: Vec<String>,
}

impl Filter {
    pub fn new(name: impl Into<String>, extensions: &[&str]) -> Self {
        Self { name: name.into(), extensions: extensions.iter().map(|e| e.trim_start_matches('.').to_string()).collect() }
    }

    /// Шаблоны: `*.obj;*.glb` (пустой фильтр — `*.*`).
    pub fn patterns(&self) -> String {
        if self.extensions.is_empty() {
            "*.*".to_string()
        } else {
            self.extensions.iter().map(|e| format!("*.{e}")).collect::<Vec<_>>().join(";")
        }
    }
}

/// Вопрос пользователю.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Request {
    pub kind: Kind,
    pub title: String,
    pub filters: Vec<Filter>,
    /// С какой папки начать.
    pub folder: Option<PathBuf>,
    /// Имя файла по умолчанию (Save).
    pub file_name: Option<String>,
}

impl Request {
    pub fn new(kind: Kind, title: impl Into<String>) -> Self {
        Self { kind, title: title.into(), filters: Vec::new(), folder: None, file_name: None }
    }

    pub fn filter(mut self, name: impl Into<String>, extensions: &[&str]) -> Self {
        self.filters.push(Filter::new(name, extensions));
        self
    }

    /// Начальная папка: ближайший существующий каталог.
    pub fn folder(mut self, folder: impl AsRef<Path>) -> Self {
        self.folder = folder.as_ref().ancestors().find(|a| a.is_dir()).map(Path::to_path_buf);
        self
    }

    pub fn file_name(mut self, name: impl Into<String>) -> Self {
        let name = name.into();
        self.file_name = (!name.is_empty()).then_some(name);
        self
    }

    /// Расширение по умолчанию (первое у первого фильтра).
    pub fn default_extension(&self) -> Option<&str> {
        self.filters.iter().flat_map(|f| f.extensions.first()).map(String::as_str).next()
    }

    /// Строка фильтров для `OPENFILENAMEW`: «имя (*.a;*.b)\0*.a;*.b\0…\0\0».
    pub fn win32_filter(&self) -> String {
        let mut out = String::new();
        for filter in &self.filters {
            let patterns = filter.patterns();
            out.push_str(&format!("{} ({patterns})\0{patterns}\0", filter.name));
        }
        out.push('\0');
        out
    }

    /// Выбранный путь с расширением по умолчанию, если пользователь его не написал (Save).
    pub fn with_default_extension(&self, path: PathBuf) -> PathBuf {
        if self.kind != Kind::Save || path.extension().is_some() {
            return path;
        }
        match self.default_extension() {
            Some(ext) => path.with_extension(ext),
            None => path,
        }
    }

    /// Аргументы `zenity`.
    pub fn zenity_args(&self) -> Vec<String> {
        let mut args = vec!["--file-selection".to_string(), format!("--title={}", self.title)];
        match self.kind {
            Kind::Open => {}
            Kind::Save => args.extend(["--save".to_string(), "--confirm-overwrite".to_string()]),
            Kind::Folder => args.push("--directory".to_string()),
        }
        let start = match (&self.folder, &self.file_name) {
            (Some(folder), Some(name)) => Some(folder.join(name)),
            (Some(folder), None) => Some(folder.join("")),
            (None, Some(name)) => Some(PathBuf::from(name)),
            (None, None) => None,
        };
        if let Some(start) = start {
            args.push(format!("--filename={}", start.display()));
        }
        if self.kind != Kind::Folder {
            for filter in &self.filters {
                let patterns = filter.patterns().replace(';', " ");
                args.push(format!("--file-filter={} | {patterns}", filter.name));
            }
        }
        args
    }

    /// Аргументы `kdialog`.
    pub fn kdialog_args(&self) -> Vec<String> {
        let start = match (&self.folder, &self.file_name) {
            (Some(folder), Some(name)) => folder.join(name).display().to_string(),
            (Some(folder), None) => folder.display().to_string(),
            (None, Some(name)) => name.clone(),
            (None, None) => ".".to_string(),
        };
        let filter = self
            .filters
            .iter()
            .map(|f| format!("{} ({})", f.name, f.patterns().replace(';', " ")))
            .collect::<Vec<_>>()
            .join("\n");
        let mut args = vec!["--title".to_string(), self.title.clone()];
        match self.kind {
            Kind::Open => args.extend(["--getopenfilename".to_string(), start, filter]),
            Kind::Save => args.extend(["--getsavefilename".to_string(), start, filter]),
            Kind::Folder => args.extend(["--getexistingdirectory".to_string(), start]),
        }
        args
    }
}

/// Ответ.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Outcome {
    Chosen(PathBuf),
    Cancelled,
    /// Системного окна нет (не включено, нет zenity / kdialog, ошибка) — показать своё.
    Unavailable,
}

/// Кто отвечает на вопрос: система или тест.
pub trait Provider: Send + Sync {
    fn ask(&self, request: &Request) -> Outcome;
}

static PROVIDER: Mutex<Option<Arc<dyn Provider>>> = Mutex::new(None);
static OWNER: Mutex<Option<isize>> = Mutex::new(None);

/// Включить системные окна (из `main` оконной программы).
pub fn enable_native() {
    set_provider(Arc::new(Native));
}

/// Поставить своего отвечающего (тесты); `None` — системных окон нет.
pub fn set_provider_opt(provider: Option<Arc<dyn Provider>>) {
    *PROVIDER.lock().unwrap_or_else(|e| e.into_inner()) = provider;
}

pub fn set_provider(provider: Arc<dyn Provider>) {
    set_provider_opt(Some(provider));
}

/// Есть ли кому спросить (иначе программа сразу показывает своё окно).
pub fn available() -> bool {
    PROVIDER.lock().unwrap_or_else(|e| e.into_inner()).is_some()
}

/// Окно-владелец (HWND на Windows): системное окно встаёт над ним и блокирует его.
pub fn set_owner_window(handle: Option<isize>) {
    *OWNER.lock().unwrap_or_else(|e| e.into_inner()) = handle;
}

pub fn owner_window() -> Option<isize> {
    *OWNER.lock().unwrap_or_else(|e| e.into_inner())
}

/// Открытое окно: ответ придёт в фоне.
pub struct Dialog {
    pub request: Request,
    rx: Receiver<Outcome>,
}

impl Dialog {
    /// Спросить в отдельном потоке. Без отвечающего — сразу [`Outcome::Unavailable`].
    pub fn spawn(request: Request) -> Self {
        let (tx, rx) = mpsc::channel();
        let provider = PROVIDER.lock().unwrap_or_else(|e| e.into_inner()).clone();
        match provider {
            None => {
                let _ = tx.send(Outcome::Unavailable);
            }
            Some(provider) => {
                let asked = request.clone();
                let spawned = std::thread::Builder::new().name("file-dialog".into()).spawn(move || {
                    let outcome = match provider.ask(&asked) {
                        Outcome::Chosen(path) => Outcome::Chosen(asked.with_default_extension(path)),
                        other => other,
                    };
                    let _ = tx.send(outcome);
                });
                if spawned.is_err() {
                    // Поток не создался — своё окно.
                    let (tx, again) = mpsc::channel();
                    let _ = tx.send(Outcome::Unavailable);
                    return Self { request, rx: again };
                }
            }
        }
        Self { request, rx }
    }

    /// Ответ, если уже есть.
    pub fn poll(&self) -> Option<Outcome> {
        match self.rx.try_recv() {
            Ok(outcome) => Some(outcome),
            Err(TryRecvError::Empty) => None,
            Err(TryRecvError::Disconnected) => Some(Outcome::Unavailable),
        }
    }
}

/// Системные окна текущей ОС.
pub struct Native;

impl Provider for Native {
    fn ask(&self, request: &Request) -> Outcome {
        native_ask(request)
    }
}

#[cfg(windows)]
fn native_ask(request: &Request) -> Outcome {
    match request.kind {
        Kind::Folder => win::pick_folder(request),
        _ => win::file(request),
    }
}

#[cfg(all(unix, not(target_os = "macos")))]
fn native_ask(request: &Request) -> Outcome {
    for (program, args) in [("zenity", request.zenity_args()), ("kdialog", request.kdialog_args())] {
        match crate::host::command(program).args(&args).output() {
            Ok(output) if output.status.success() => {
                let text = String::from_utf8_lossy(&output.stdout).trim().to_string();
                return if text.is_empty() { Outcome::Cancelled } else { Outcome::Chosen(PathBuf::from(text)) };
            }
            // 1 — «Отмена» у обоих.
            Ok(output) if output.status.code() == Some(1) => return Outcome::Cancelled,
            _ => continue,
        }
    }
    Outcome::Unavailable
}

#[cfg(target_os = "macos")]
fn native_ask(request: &Request) -> Outcome {
    let quote = |s: &str| s.replace('\\', "\\\\").replace('"', "\\\"");
    let mut script = match request.kind {
        Kind::Open => format!("POSIX path of (choose file with prompt \"{}\"", quote(&request.title)),
        Kind::Save => format!("POSIX path of (choose file name with prompt \"{}\"", quote(&request.title)),
        Kind::Folder => format!("POSIX path of (choose folder with prompt \"{}\"", quote(&request.title)),
    };
    if let Some(folder) = &request.folder {
        script.push_str(&format!(" default location (POSIX file \"{}\")", quote(&folder.display().to_string())));
    }
    if let (Kind::Save, Some(name)) = (request.kind, &request.file_name) {
        script.push_str(&format!(" default name \"{}\"", quote(name)));
    }
    script.push(')');
    match crate::host::command("osascript").arg("-e").arg(script).output() {
        Ok(output) if output.status.success() => {
            let text = String::from_utf8_lossy(&output.stdout).trim().to_string();
            if text.is_empty() { Outcome::Cancelled } else { Outcome::Chosen(PathBuf::from(text)) }
        }
        Ok(_) => Outcome::Cancelled,
        Err(_) => Outcome::Unavailable,
    }
}

#[cfg(not(any(windows, unix)))]
fn native_ask(_: &Request) -> Outcome {
    Outcome::Unavailable
}

#[cfg(windows)]
mod win {
    use super::{owner_window, Kind, Outcome, Request};
    use std::os::windows::ffi::{OsStrExt, OsStringExt};
    use std::path::PathBuf;
    use windows_sys::Win32::System::Com::{CoInitializeEx, CoTaskMemFree, COINIT_APARTMENTTHREADED};
    use windows_sys::Win32::UI::Controls::Dialogs::{
        CommDlgExtendedError, GetOpenFileNameW, GetSaveFileNameW, OFN_EXPLORER, OFN_FILEMUSTEXIST, OFN_NOCHANGEDIR,
        OFN_OVERWRITEPROMPT, OFN_PATHMUSTEXIST, OPENFILENAMEW,
    };
    use windows_sys::Win32::UI::Shell::{
        SHBrowseForFolderW, SHGetPathFromIDListW, BFFM_INITIALIZED, BFFM_SETSELECTIONW, BIF_NEWDIALOGSTYLE,
        BIF_RETURNONLYFSDIRS, BROWSEINFOW,
    };
    use windows_sys::Win32::UI::WindowsAndMessaging::SendMessageW;

    fn wide(text: &str) -> Vec<u16> {
        std::ffi::OsStr::new(text).encode_wide().chain(std::iter::once(0)).collect()
    }

    fn from_wide(buffer: &[u16]) -> String {
        let end = buffer.iter().position(|c| *c == 0).unwrap_or(buffer.len());
        std::ffi::OsString::from_wide(&buffer[..end]).to_string_lossy().into_owned()
    }

    fn owner() -> *mut core::ffi::c_void {
        owner_window().unwrap_or(0) as *mut core::ffi::c_void
    }

    pub fn file(request: &Request) -> Outcome {
        const MAX: usize = 32 * 1024;
        let mut buffer = vec![0u16; MAX];
        if let Some(name) = &request.file_name {
            let name = wide(name);
            let n = name.len().min(MAX - 1);
            buffer[..n].copy_from_slice(&name[..n]);
            buffer[MAX - 1] = 0;
        }
        let filter = if request.filters.is_empty() { None } else { Some(request.win32_filter().encode_utf16().collect::<Vec<u16>>()) };
        let folder = request.folder.as_ref().map(|f| wide(&crate::host::native_path(f).display().to_string()));
        let title = wide(&request.title);
        let default_ext = request.default_extension().map(wide);
        // SAFETY: OPENFILENAMEW — простая структура C; все указатели живут до конца вызова.
        let mut ofn: OPENFILENAMEW = unsafe { std::mem::zeroed() };
        ofn.lStructSize = std::mem::size_of::<OPENFILENAMEW>() as u32;
        ofn.hwndOwner = owner();
        ofn.lpstrFilter = filter.as_ref().map_or(std::ptr::null(), |f| f.as_ptr());
        ofn.nFilterIndex = 1;
        ofn.lpstrFile = buffer.as_mut_ptr();
        ofn.nMaxFile = MAX as u32;
        ofn.lpstrInitialDir = folder.as_ref().map_or(std::ptr::null(), |f| f.as_ptr());
        ofn.lpstrTitle = title.as_ptr();
        ofn.lpstrDefExt = default_ext.as_ref().map_or(std::ptr::null(), |e| e.as_ptr());
        ofn.Flags = OFN_EXPLORER | OFN_NOCHANGEDIR | OFN_PATHMUSTEXIST;
        let ok = unsafe {
            if request.kind == Kind::Save {
                ofn.Flags |= OFN_OVERWRITEPROMPT;
                GetSaveFileNameW(&mut ofn)
            } else {
                ofn.Flags |= OFN_FILEMUSTEXIST;
                GetOpenFileNameW(&mut ofn)
            }
        };
        if ok != 0 {
            return Outcome::Chosen(PathBuf::from(from_wide(&buffer)));
        }
        // 0 — «Отмена»; другой код — ошибка окна: тогда своё окно программы.
        if unsafe { CommDlgExtendedError() } == 0 { Outcome::Cancelled } else { Outcome::Unavailable }
    }

    unsafe extern "system" fn browse_callback(hwnd: *mut core::ffi::c_void, message: u32, _: isize, data: isize) -> i32 {
        if message == BFFM_INITIALIZED && data != 0 {
            SendMessageW(hwnd, BFFM_SETSELECTIONW, 1, data);
        }
        0
    }

    pub fn pick_folder(request: &Request) -> Outcome {
        // Окно папок нового вида требует COM в однопоточном режиме (поток — свой).
        unsafe {
            CoInitializeEx(std::ptr::null(), COINIT_APARTMENTTHREADED as u32);
        }
        let title = wide(&request.title);
        let start = request.folder.as_ref().map(|f| wide(&crate::host::native_path(f).display().to_string()));
        let mut display = vec![0u16; 260];
        let info = BROWSEINFOW {
            hwndOwner: owner(),
            pidlRoot: std::ptr::null_mut(),
            pszDisplayName: display.as_mut_ptr(),
            lpszTitle: title.as_ptr(),
            ulFlags: BIF_RETURNONLYFSDIRS | BIF_NEWDIALOGSTYLE,
            lpfn: Some(browse_callback),
            lParam: start.as_ref().map_or(0, |s| s.as_ptr() as isize),
            iImage: 0,
        };
        // SAFETY: структура и строки живут до конца вызова; PIDL освобождается CoTaskMemFree.
        let pidl = unsafe { SHBrowseForFolderW(&info) };
        if pidl.is_null() {
            return Outcome::Cancelled;
        }
        let mut path = vec![0u16; 32 * 1024];
        let ok = unsafe { SHGetPathFromIDListW(pidl, path.as_mut_ptr()) };
        unsafe { CoTaskMemFree(pidl as *const core::ffi::c_void) };
        if ok == 0 {
            return Outcome::Unavailable;
        }
        Outcome::Chosen(PathBuf::from(from_wide(&path)))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn model_request() -> Request {
        Request::new(Kind::Save, "Save Model As")
            .filter("Piska model", &["pmodel"])
            .filter("All files", &[])
            .file_name("Crate")
    }

    #[test]
    fn win32_filters_are_double_null_terminated_pairs() {
        let request = Request::new(Kind::Open, "Import").filter("Meshes", &["obj", ".glb", "gltf"]).filter("All files", &[]);
        assert_eq!(request.win32_filter(), "Meshes (*.obj;*.glb;*.gltf)\0*.obj;*.glb;*.gltf\0All files (*.*)\0*.*\0\0");
        assert_eq!(request.default_extension(), Some("obj"));
    }

    #[test]
    fn save_adds_the_default_extension_only_when_missing() {
        let request = model_request();
        assert_eq!(request.with_default_extension(PathBuf::from("a/Crate")), PathBuf::from("a/Crate.pmodel"));
        assert_eq!(request.with_default_extension(PathBuf::from("a/Crate.bak")), PathBuf::from("a/Crate.bak"));
        let open = Request::new(Kind::Open, "Open").filter("Piska model", &["pmodel"]);
        assert_eq!(open.with_default_extension(PathBuf::from("x")), PathBuf::from("x"));
    }

    #[test]
    fn the_start_folder_is_the_nearest_existing_one() {
        let dir = std::env::temp_dir();
        let request = Request::new(Kind::Open, "Open").folder(dir.join("нет такой папки").join("и этой"));
        assert_eq!(request.folder.as_deref(), Some(dir.as_path()));
    }

    #[test]
    fn linux_helpers_get_title_filters_and_start_name() {
        let request = model_request().folder(std::env::temp_dir());
        let zenity = request.zenity_args();
        assert!(zenity.contains(&"--save".to_string()) && zenity.contains(&"--confirm-overwrite".to_string()));
        assert!(zenity.contains(&"--title=Save Model As".to_string()));
        assert!(zenity.iter().any(|a| a == "--file-filter=Piska model | *.pmodel"));
        assert!(zenity.iter().any(|a| a.starts_with("--filename=") && a.ends_with("Crate")));
        let kdialog = request.kdialog_args();
        assert_eq!(kdialog[2], "--getsavefilename");
        assert!(kdialog[3].ends_with("Crate"));
        assert_eq!(kdialog[4], "Piska model (*.pmodel)\nAll files (*.*)");
        let folder = Request::new(Kind::Folder, "Pick").zenity_args();
        assert!(folder.contains(&"--directory".to_string()));
        assert!(!folder.iter().any(|a| a.starts_with("--file-filter")));
    }

    struct Fake(Outcome);
    impl Provider for Fake {
        fn ask(&self, _: &Request) -> Outcome {
            self.0.clone()
        }
    }

    #[test]
    fn dialogs_answer_through_the_provider_and_never_open_without_one() {
        static LOCK: Mutex<()> = Mutex::new(());
        let _guard = LOCK.lock().unwrap_or_else(|e| e.into_inner());
        set_provider_opt(None);
        assert!(!available());
        let dialog = Dialog::spawn(model_request());
        assert_eq!(dialog.poll(), Some(Outcome::Unavailable), "no provider: the program shows its own window");

        set_provider(Arc::new(Fake(Outcome::Chosen(PathBuf::from("models/Crate")))));
        let dialog = Dialog::spawn(model_request());
        let outcome = loop {
            if let Some(outcome) = dialog.poll() {
                break outcome;
            }
            std::thread::yield_now();
        };
        assert_eq!(outcome, Outcome::Chosen(PathBuf::from("models/Crate.pmodel")), "Save adds .pmodel");
        set_provider(Arc::new(Fake(Outcome::Cancelled)));
        let dialog = Dialog::spawn(model_request());
        let outcome = loop {
            if let Some(outcome) = dialog.poll() {
                break outcome;
            }
            std::thread::yield_now();
        };
        assert_eq!(outcome, Outcome::Cancelled);
        set_provider_opt(None);
    }
}
