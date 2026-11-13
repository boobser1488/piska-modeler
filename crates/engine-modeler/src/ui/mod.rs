//! Интерфейс Piska Modeler на egui.
//!
//! [`ModelerApp::show`] рисует всё окно моделей внутри любого `Ui`: так одно и то же
//! окно работает отдельной программой (`tools/piska-modeler`) и вкладкой Modeler
//! редактора. Цвета — из текущего стиля egui (общая тема Piska ставится хозяином).
//!
//! Всё, что делают кнопки и клавиши, — значение [`Command`] для [`ModelerApp::run`]:
//! так ими пользуются и меню, и тесты.

mod icons;
pub mod kit;
mod outliner;
mod panels;
mod style;
mod uv_editor;
mod viewport;

use std::path::{Path, PathBuf};
use std::time::SystemTime;

use egui::{pos2, vec2, Color32, Key, KeyboardShortcut, Modifiers, Rect, Sense, Stroke, Ui};
use engine_i18n::{n_, tr, tr_ctx, trf, trn};
use glam::Vec3;

use crate::camera::ViewPreset;
use crate::doc::{Document, FaceDir, NodeId, Shape};
use crate::image::Rgba;
use crate::ops::{self, Align};
use crate::raster::Shading;
use crate::session::{clipboard_has_content, Session};
use crate::{export, format, import, uv, ModelError};

pub use icons::Icon;
use style::Palette;

/// Режим окна: правка формы или рисование по текстуре (Tab).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Mode {
    Edit,
    Paint,
}

/// Инструмент левой колонки.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Tool {
    Select,
    Move,
    Resize,
    Rotate,
    Pivot,
    Pencil,
    Eraser,
    Fill,
    Line,
    Rectangle,
    Picker,
    Shade,
}

impl Tool {
    pub const EDIT: [Tool; 5] = [Tool::Select, Tool::Move, Tool::Resize, Tool::Rotate, Tool::Pivot];
    pub const PAINT: [Tool; 7] = [
        Tool::Pencil,
        Tool::Eraser,
        Tool::Fill,
        Tool::Line,
        Tool::Rectangle,
        Tool::Picker,
        Tool::Shade,
    ];

    pub fn name(self) -> &'static str {
        match self {
            Tool::Select => n_("Select"),
            Tool::Move => n_("Move"),
            Tool::Resize => n_("Resize"),
            Tool::Rotate => n_("Rotate"),
            Tool::Pivot => n_("Pivot"),
            Tool::Pencil => n_("Pencil"),
            Tool::Eraser => n_("Eraser"),
            Tool::Fill => n_("Fill"),
            Tool::Line => n_("Line"),
            Tool::Rectangle => n_("Rectangle"),
            Tool::Picker => n_("Color Picker"),
            Tool::Shade => n_("Shade"),
        }
    }

    /// Клавиша инструмента (в своём режиме).
    pub fn key(self) -> Key {
        match self {
            Tool::Select => Key::Q,
            Tool::Move => Key::W,
            Tool::Rotate => Key::E,
            Tool::Resize => Key::R,
            Tool::Pivot => Key::P,
            Tool::Pencil => Key::B,
            Tool::Eraser => Key::E,
            Tool::Fill => Key::G,
            Tool::Line => Key::L,
            Tool::Rectangle => Key::U,
            Tool::Picker => Key::I,
            Tool::Shade => Key::D,
        }
    }

    pub fn icon(self) -> Icon {
        match self {
            Tool::Select => Icon::Select,
            Tool::Move => Icon::Move,
            Tool::Resize => Icon::Resize,
            Tool::Rotate => Icon::Rotate,
            Tool::Pivot => Icon::Pivot,
            Tool::Pencil => Icon::Pencil,
            Tool::Eraser => Icon::Eraser,
            Tool::Fill => Icon::Fill,
            Tool::Line => Icon::Line,
            Tool::Rectangle => Icon::Rectangle,
            Tool::Picker => Icon::Picker,
            Tool::Shade => Icon::Shade,
        }
    }

    /// Подсказка в строке состояния.
    pub fn hint(self) -> &'static str {
        match self {
            Tool::Select => n_("Click to select, drag on empty space for a box, Ctrl adds"),
            Tool::Move => n_("Drag an arrow to move along it, the centre square in the view plane"),
            Tool::Resize => n_("Drag a face handle: the box grows from that side, the opposite side stays"),
            Tool::Rotate => n_("Drag a ring to rotate around the pivot"),
            Tool::Pivot => n_("Drag the arrows to move the pivot without moving the shape"),
            Tool::Pencil => n_("Paint texels on the model or in the UV view"),
            Tool::Eraser => n_("Make texels transparent"),
            Tool::Fill => n_("Fill the face (or the connected colour, see Fill mode)"),
            Tool::Line => n_("Drag a straight line on one face"),
            Tool::Rectangle => n_("Drag a rectangle outline on one face (Shift: filled)"),
            Tool::Picker => n_("Click to take a colour"),
            Tool::Shade => n_("Drag to lighten, with Shift to darken"),
        }
    }

    /// Что делает мышь с этим инструментом (строка состояния).
    pub fn mouse_hint(self) -> &'static str {
        match self {
            Tool::Select => n_("LMB select · Ctrl/Shift+LMB add · drag on empty space: box · RMB orbit · MMB pan · wheel zoom"),
            Tool::Move => n_("Drag an arrow: move along it · centre square: in the view plane · Ctrl: toggle snap · Shift: fine"),
            Tool::Resize => n_("Drag a face handle: grow from that side, the opposite side stays · Ctrl: toggle snap · Shift: fine"),
            Tool::Rotate => n_("Drag a ring: rotate around the pivot · Ctrl: toggle angle snap · Shift: 1° steps"),
            Tool::Pivot => n_("Drag an arrow: move the pivot, the shape stays"),
            Tool::Pencil => n_("LMB paint on the model or in the UV view · [ ] size · X swap colours · Alt+LMB orbit"),
            Tool::Eraser => n_("LMB erase texels (they become transparent) · [ ] size"),
            Tool::Fill => n_("LMB fill the face or the connected colour (see Mode) · X swap colours"),
            Tool::Line => n_("Drag: straight line on one face · [ ] size"),
            Tool::Rectangle => n_("Drag: rectangle on one face · Shift: the other of outline / filled"),
            Tool::Picker => n_("LMB take the colour under the cursor"),
            Tool::Shade => n_("Drag: lighten or darken (see Mode) · Shift: the other one · each texel once per stroke"),
        }
    }

    pub fn is_paint(self) -> bool {
        Tool::PAINT.contains(&self)
    }
}

/// Заливка: всей стороны или связной области одного цвета.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FillMode {
    Face,
    Connected,
}

/// Кисть и палитра.
#[derive(Clone, Debug)]
pub struct PaintSettings {
    pub color: Rgba,
    pub secondary: Rgba,
    pub size: u32,
    /// Шаг осветления / затемнения.
    pub shade: i32,
    pub fill: FillMode,
    /// Прямоугольник — залитый (Shift на время штриха — наоборот).
    pub filled: bool,
    /// Shade затемняет (Shift на время штриха — наоборот).
    pub darken: bool,
    /// Рисовать зеркально по X.
    pub mirror: bool,
    pub recent: Vec<Rgba>,
    pub palette: Vec<Rgba>,
}

impl Default for PaintSettings {
    fn default() -> Self {
        Self {
            color: [0xd9, 0x5f, 0x43, 255],
            secondary: [0x2b, 0x2d, 0x33, 255],
            size: 1,
            shade: 18,
            fill: FillMode::Face,
            filled: false,
            darken: false,
            mirror: false,
            recent: Vec::new(),
            palette: DEFAULT_PALETTE.to_vec(),
        }
    }
}

impl PaintSettings {
    /// Запомнить цвет в недавних.
    pub fn remember(&mut self, color: Rgba) {
        self.recent.retain(|c| *c != color);
        self.recent.insert(0, color);
        self.recent.truncate(12);
    }
}

/// Палитра по умолчанию: нейтральные, земля, зелень, вода, тёплые и акценты.
pub const DEFAULT_PALETTE: [Rgba; 32] = [
    [0x14, 0x14, 0x17, 255],
    [0x2b, 0x2d, 0x33, 255],
    [0x4a, 0x4d, 0x55, 255],
    [0x74, 0x77, 0x80, 255],
    [0xa3, 0xa6, 0xad, 255],
    [0xcf, 0xd1, 0xd5, 255],
    [0xf2, 0xf2, 0xf0, 255],
    [0xff, 0xff, 0xff, 255],
    [0x3d, 0x28, 0x1c, 255],
    [0x6b, 0x47, 0x2b, 255],
    [0x9a, 0x6a, 0x3d, 255],
    [0xc8, 0x95, 0x5c, 255],
    [0xe6, 0xc0, 0x8c, 255],
    [0xe0, 0xac, 0x84, 255],
    [0xa8, 0x6b, 0x4f, 255],
    [0x6e, 0x3a, 0x2f, 255],
    [0x1f, 0x3d, 0x2a, 255],
    [0x2f, 0x6b, 0x3f, 255],
    [0x4f, 0x9a, 0x4a, 255],
    [0x95, 0xc7, 0x5a, 255],
    [0x1e, 0x33, 0x5c, 255],
    [0x2e, 0x6f, 0xb5, 255],
    [0x5c, 0xa8, 0xe0, 255],
    [0xa9, 0xdc, 0xf0, 255],
    [0x8a, 0x1f, 0x2b, 255],
    [0xd9, 0x5f, 0x43, 255],
    [0xf0, 0x9a, 0x3e, 255],
    [0xf6, 0xd0, 0x4d, 255],
    [0x5b, 0x2a, 0x6e, 255],
    [0xa0, 0x5b, 0xc4, 255],
    [0xe3, 0x7b, 0xa8, 255],
    [0x3a, 0xb0, 0xa0, 255],
];

/// Настройки вида.
#[derive(Clone, Debug)]
pub struct ViewSettings {
    pub shading: Shading,
    pub grid: bool,
    /// Привязка перемещения и размера, единиц модели.
    pub snap_step: f32,
    pub snapping: bool,
    /// Шаг поворота в градусах (0 — свободно).
    pub rotation_step: f32,
    pub ruler: bool,
    /// Сверхвыборка растеризатора (1 — быстро, 2 — гладкие края).
    pub supersample: u32,
}

impl Default for ViewSettings {
    fn default() -> Self {
        Self {
            shading: Shading::Textured,
            grid: true,
            snap_step: 1.0,
            snapping: true,
            rotation_step: 22.5,
            ruler: true,
            supersample: 1,
        }
    }
}

/// Команда окна моделей: меню, кнопки, клавиши и тесты пользуются одним списком.
#[derive(Clone, Debug, PartialEq)]
pub enum Command {
    New,
    Save,
    /// Сохранить в этот файл.
    SaveTo(PathBuf),
    /// Открыть файл.
    OpenFile(PathBuf),
    CloseTab,
    ExportGlb(PathBuf),
    ExportObj(PathBuf),
    ExportTexture(PathBuf),
    ImportTexture(PathBuf),
    ImportReference(PathBuf),
    Undo,
    Redo,
    Copy,
    Paste,
    Duplicate,
    Delete,
    SelectAll,
    SelectNone,
    Add(Shape),
    AddGroup,
    /// Положить выделенное в новую группу.
    GroupSelection,
    MirrorX,
    UnlinkTwin,
    Flip(usize),
    Align(usize, Align),
    Distribute(usize),
    Center,
    ToggleVisibility,
    AutoUv,
    Repack,
    AddTexture,
    ResizeTexture(u32, u32),
    /// Отменить сразу несколько шагов (список истории).
    UndoSteps(usize),
    /// Окно со списком горячих клавиш.
    ShowShortcuts,
    Frame,
    View(ViewPreset),
    ToggleOrthographic,
    Shading(Shading),
    ToggleGrid,
    ToggleMode,
    SetMode(Mode),
    Tool(Tool),
    RestoreRecovery,
    DiscardRecovery,
}

/// Открытый документ.
struct DocTab {
    session: Session,
    path: Option<PathBuf>,
    last_autosave: f64,
    /// Найден файл восстановления новее документа.
    recovery: Option<PathBuf>,
    texture_times: Vec<Option<SystemTime>>,
    last_texture_check: f64,
}

impl DocTab {
    fn new(doc: Document, path: Option<PathBuf>) -> Self {
        let recovery_file = format::recovery_path(path.as_deref(), &doc.name);
        let recovery = recovery_file.is_file().then_some(recovery_file).filter(|r| {
            let modified = |p: &Path| std::fs::metadata(p).and_then(|m| m.modified()).ok();
            match (&path, modified(r)) {
                (Some(file), Some(rec)) => modified(file).is_none_or(|f| rec > f),
                (None, Some(_)) => true,
                _ => false,
            }
        });
        let mut tab = Self {
            session: Session::new(doc),
            path,
            last_autosave: 0.0,
            recovery,
            texture_times: Vec::new(),
            last_texture_check: 0.0,
        };
        tab.texture_times = tab.texture_sources().iter().map(|p| p.as_deref().and_then(mtime)).collect();
        tab
    }

    fn title(&self) -> String {
        let name = self
            .path
            .as_ref()
            .and_then(|p| p.file_name())
            .map(|n| n.to_string_lossy().to_string())
            .unwrap_or_else(|| format!("{}.pmodel", self.session.doc.name));
        name
    }

    /// Файлы, из которых импортированы текстуры (путь относительно документа).
    fn texture_sources(&self) -> Vec<Option<PathBuf>> {
        let dir = self.path.as_ref().and_then(|p| p.parent()).map(Path::to_path_buf);
        self.session
            .doc
            .textures
            .iter()
            .map(|t| {
                t.source.as_ref().map(|s| {
                    let p = PathBuf::from(s);
                    match (&dir, p.is_absolute()) {
                        (Some(d), false) => d.join(p),
                        _ => p,
                    }
                })
            })
            .collect()
    }
}

fn mtime(path: &Path) -> Option<SystemTime> {
    std::fs::metadata(path).and_then(|m| m.modified()).ok()
}

/// Окно ввода пути (открыть, сохранить как, экспорт, импорт).
#[derive(Clone, Debug, PartialEq)]
enum PromptKind {
    Open,
    SaveAs,
    ExportGlb,
    ExportObj,
    ExportTexture,
    ImportTexture,
    ImportReference,
    ResizeTexture,
}

struct Prompt {
    kind: PromptKind,
    text: String,
    error: Option<String>,
    focus: bool,
}

/// Окно моделей целиком.
pub struct ModelerApp {
    tabs: Vec<DocTab>,
    active: usize,
    mode: Mode,
    edit_tool: Tool,
    paint_tool: Tool,
    pub paint: PaintSettings,
    pub view: ViewSettings,
    viewport: viewport::Viewport,
    uv: uv_editor::UvEditor,
    outliner: outliner::OutlinerState,
    open_sections: [bool; 5],
    status: Option<(String, f64, bool)>,
    prompt: Option<Prompt>,
    saved: Vec<PathBuf>,
    /// Каталог для путей без каталога (Open…, экспорт нового документа).
    pub working_dir: PathBuf,
    /// Окно получает клавиатуру (в редакторе — когда с ним работали последним).
    keyboard_focus: bool,
    /// Правка числовых полей идёт (история пишется одним шагом).
    field_edit: bool,
    /// Tab переключил режим: egui заодно переводит фокус на поле ввода — снять его.
    drop_focus: bool,
    /// Окно Help ▸ Keyboard Shortcuts открыто.
    show_shortcuts: bool,
    time: f64,
    /// Через сколько секунд писать файл восстановления (0 — не писать).
    pub autosave_seconds: f64,
}

impl Default for ModelerApp {
    fn default() -> Self {
        Self::new()
    }
}

const SECTION_OUTLINER: usize = 0;
const SECTION_ELEMENT: usize = 1;
const SECTION_UV: usize = 2;
const SECTION_PALETTE: usize = 3;
const SECTION_TEXTURES: usize = 4;

impl ModelerApp {
    /// Окно с одним новым документом.
    pub fn new() -> Self {
        Self {
            tabs: vec![DocTab::new(Document::new("untitled"), None)],
            active: 0,
            mode: Mode::Edit,
            edit_tool: Tool::Move,
            paint_tool: Tool::Pencil,
            paint: PaintSettings::default(),
            view: ViewSettings::default(),
            viewport: viewport::Viewport::default(),
            uv: uv_editor::UvEditor::default(),
            outliner: outliner::OutlinerState::default(),
            open_sections: [true, true, true, true, false],
            status: None,
            prompt: None,
            saved: Vec::new(),
            working_dir: std::env::current_dir().unwrap_or_default(),
            keyboard_focus: true,
            field_edit: false,
            drop_focus: false,
            show_shortcuts: false,
            time: 0.0,
            autosave_seconds: 20.0,
        }
    }

    /// Открыть документ (вкладкой). Пустая безымянная вкладка заменяется.
    pub fn open_document(&mut self, doc: Document, path: Option<PathBuf>) {
        let replace = self.tabs.len() == 1
            && self.tabs[0].path.is_none()
            && self.tabs[0].session.doc.nodes.is_empty()
            && !self.tabs[0].session.can_undo();
        let tab = DocTab::new(doc, path);
        if replace {
            self.tabs[0] = tab;
            self.active = 0;
        } else {
            self.tabs.push(tab);
            self.active = self.tabs.len() - 1;
        }
        self.frame_all();
    }

    /// Открыть файл `.pmodel` (уже открытый — просто показать).
    pub fn open(&mut self, path: impl AsRef<Path>) -> Result<(), ModelError> {
        let path = path.as_ref().to_path_buf();
        if let Some(i) = self.tabs.iter().position(|t| t.path.as_deref() == Some(path.as_path())) {
            self.active = i;
            return Ok(());
        }
        let doc = format::load(&path)?;
        self.open_document(doc, Some(path));
        Ok(())
    }

    pub fn session(&self) -> &Session {
        &self.tabs[self.active].session
    }

    pub fn session_mut(&mut self) -> &mut Session {
        &mut self.tabs[self.active].session
    }

    pub fn document(&self) -> &Document {
        &self.session().doc
    }

    /// Путь файла активного документа.
    pub fn path(&self) -> Option<&Path> {
        self.tabs[self.active].path.as_deref()
    }

    pub fn tab_count(&self) -> usize {
        self.tabs.len()
    }

    pub fn set_active_tab(&mut self, index: usize) {
        if index < self.tabs.len() {
            self.active = index;
        }
    }

    pub fn mode(&self) -> Mode {
        self.mode
    }

    pub fn tool(&self) -> Tool {
        match self.mode {
            Mode::Edit => self.edit_tool,
            Mode::Paint => self.paint_tool,
        }
    }

    /// Файлы, сохранённые с прошлого вызова (редактор перечитывает их как ассеты).
    pub fn take_saved_paths(&mut self) -> Vec<PathBuf> {
        std::mem::take(&mut self.saved)
    }

    /// Есть ли несохранённые изменения в каком-нибудь документе.
    pub fn has_unsaved_changes(&self) -> bool {
        self.tabs.iter().any(|t| t.session.is_dirty())
    }

    /// Последнее сообщение строки состояния.
    pub fn status_message(&self) -> Option<&str> {
        self.status.as_ref().map(|s| s.0.as_str())
    }

    pub fn camera(&self) -> &crate::camera::Camera {
        &self.viewport.camera
    }

    pub fn camera_mut(&mut self) -> &mut crate::camera::Camera {
        &mut self.viewport.camera
    }

    /// Прямоугольник 3D-вида в прошлом кадре (точки egui).
    pub fn viewport_rect(&self) -> Option<Rect> {
        self.viewport.rect
    }

    /// Точка модели → точка окна (по камере прошлого кадра).
    pub fn world_to_screen(&self, point: Vec3) -> Option<egui::Pos2> {
        self.viewport.world_to_screen(point)
    }

    /// Где на экране ручка стороны `face` у выделенного элемента (инструмент Resize).
    pub fn resize_handle(&self, face: FaceDir) -> Option<egui::Pos2> {
        let id = self.session().primary()?;
        let center = viewport::face_center(&self.session().doc, id, face)?;
        self.world_to_screen(center)
    }

    /// Прямоугольник UV-вида в прошлом кадре.
    pub fn uv_view_rect(&self) -> Option<Rect> {
        self.uv.canvas
    }

    /// Стабильный идентификатор кнопки панели (для тестов): `"add.cuboid"`, `"tool.move"`…
    pub fn button_id(name: &str) -> egui::Id {
        egui::Id::new(format!("modeler.{name}"))
    }

    /// Идентификатор строки Outliner.
    pub fn outliner_row_id(id: NodeId) -> egui::Id {
        outliner::row_id(id)
    }

    /// Сколько раз растеризатор перерисовал кадр (для тестов: кадр не рисуется зря).
    pub fn render_count(&self) -> u64 {
        self.viewport.renders
    }

    /// Рисовать вид на видеокарте: хозяин передаёт текстуру egui, в которую он рисует
    /// кадр (см. `engine_modeler::gpu::GpuViewport`), и забирает запросы кадра
    /// [`take_view_request`](Self::take_view_request). `None` — программный растеризатор.
    pub fn set_gpu_texture(&mut self, texture: Option<egui::TextureId>) {
        self.viewport.gpu_texture = texture;
        self.viewport.pending = None;
    }

    /// Запрос кадра вида, если с прошлого раза что-то изменилось (камера, модель,
    /// выделение, размер). Без изменений — `None`: рисовать нечего.
    pub fn take_view_request(&mut self) -> Option<crate::raster::ViewRequest> {
        self.viewport.pending.take()
    }

    /// Замеры последней отрисовки вида: (растеризация, выгрузка в текстуру), мс.
    pub fn viewport_timings(&self) -> (f32, f32) {
        (self.viewport.raster_ms, self.viewport.upload_ms)
    }

    fn message(&mut self, text: impl Into<String>) {
        self.status = Some((text.into(), self.time, false));
    }

    fn error(&mut self, text: impl Into<String>) {
        let text = text.into();
        log::warn!("modeler: {text}");
        self.status = Some((text, self.time, true));
    }

    fn frame_all(&mut self) {
        let doc = &self.tabs[self.active].session.doc;
        if let Some((min, max)) = doc.model_bounds() {
            self.viewport.camera.frame(min, max);
        } else {
            self.viewport.camera = crate::camera::Camera::default();
        }
    }

    // ------------------------------------------------------------------
    // Команды
    // ------------------------------------------------------------------

    /// Выполнить команду.
    pub fn run(&mut self, command: Command) {
        let selection = self.session().selection.clone();
        match command {
            Command::New => {
                let name = (1..)
                    .map(|i| if i == 1 { "untitled".to_string() } else { format!("untitled{i}") })
                    .find(|n| !self.tabs.iter().any(|t| t.session.doc.name == *n))
                    .expect("a free name exists");
                self.tabs.push(DocTab::new(Document::new(&name), None));
                self.active = self.tabs.len() - 1;
                self.viewport.camera = crate::camera::Camera::default();
            }
            Command::Save => match self.tabs[self.active].path.clone() {
                Some(path) => self.run(Command::SaveTo(path)),
                None => self.ask(PromptKind::SaveAs),
            },
            Command::SaveTo(path) => {
                let path = self.resolve(&path, "pmodel");
                let result = format::save(&self.session().doc, &path);
                match result {
                    Ok(()) => {
                        let tab = &mut self.tabs[self.active];
                        let stem = path.file_stem().map(|s| s.to_string_lossy().to_string());
                        if let Some(stem) = stem.filter(|s| *s != tab.session.doc.name) {
                            // Имя модели — имя файла (без записи в историю правок).
                            tab.session.doc.name = stem;
                            let _ = format::save(&tab.session.doc, &path);
                        }
                        let old_recovery = format::recovery_path(tab.path.as_deref(), &tab.session.doc.name);
                        let _ = std::fs::remove_file(old_recovery);
                        let _ = std::fs::remove_file(format::recovery_path(None, &tab.session.doc.name));
                        tab.path = Some(path.clone());
                        tab.session.mark_saved();
                        tab.recovery = None;
                        self.saved.push(path.clone());
                        self.message(trf!("Saved {path}", path = path.display()));
                    }
                    Err(e) => self.error(e.to_string()),
                }
            }
            Command::OpenFile(path) => {
                let path = self.resolve(&path, "pmodel");
                if let Err(e) = self.open(&path) {
                    self.error(e.to_string());
                } else {
                    self.message(trf!("Opened {path}", path = path.display()));
                }
            }
            Command::CloseTab => {
                if self.tabs.len() > 1 {
                    self.tabs.remove(self.active);
                    self.active = self.active.min(self.tabs.len() - 1);
                } else {
                    self.tabs[0] = DocTab::new(Document::new("untitled"), None);
                }
            }
            Command::ExportGlb(path) => {
                let path = self.resolve(&path, "glb");
                match export::write_glb(&self.session().doc, &path) {
                    Ok(()) => self.message(trf!("Exported {path}", path = path.display())),
                    Err(e) => self.error(e.to_string()),
                }
            }
            Command::ExportObj(path) => {
                let path = self.resolve(&path, "obj");
                match export::write_obj(&self.session().doc, &path) {
                    Ok(()) => self.message(trf!("Exported {path} with .mtl and textures", path = path.display())),
                    Err(e) => self.error(e.to_string()),
                }
            }
            Command::ExportTexture(path) => {
                let path = self.resolve(&path, "png");
                let index = self.uv.texture as usize;
                let png = self.session().doc.textures.get(index).map(|t| t.image.to_png());
                match png.map(|bytes| std::fs::write(&path, bytes)) {
                    Some(Ok(())) => self.message(trf!("Exported {path}", path = path.display())),
                    Some(Err(e)) => self.error(trf!("cannot write `{path}`: {error}", path = path.display(), error = e)),
                    None => self.error(tr("there is no texture to export")),
                }
            }
            Command::ImportTexture(path) => {
                let path = self.resolve(&path, "png");
                match std::fs::read(&path) {
                    Ok(bytes) => {
                        let name = path.file_stem().map_or("texture".to_string(), |s| s.to_string_lossy().to_string());
                        let source = self.relative_to_document(&path);
                        let result = self.session_mut().edit("Import Texture", |doc| ops::import_texture(doc, &name, &bytes, Some(source)));
                        match result {
                            Ok(index) => {
                                self.uv.texture = index;
                                let tab = &mut self.tabs[self.active];
                                tab.texture_times = tab.texture_sources().iter().map(|p| p.as_deref().and_then(mtime)).collect();
                                self.message(trf!("Imported texture `{name}`; assign it to faces in the UV panel", name = name));
                            }
                            Err(e) => self.error(e.to_string()),
                        }
                    }
                    Err(e) => self.error(trf!("cannot read `{path}`: {error}", path = path.display(), error = e)),
                }
            }
            Command::ImportReference(path) => {
                let path = self.resolve(&path, "obj");
                let reference = import::reference_from_file(&self.session().doc, &path);
                match reference {
                    Ok(reference) => {
                        let name = reference.name.clone();
                        self.session_mut().edit("Import Reference", |doc| doc.references.push(reference));
                        self.message(trf!("Reference `{name}` added (shown grey, not editable)", name = name));
                    }
                    Err(e) => self.error(e.to_string()),
                }
            }
            Command::Undo => {
                let label = self.session().undo_label().map(str::to_string);
                if self.session_mut().undo() {
                    self.message(trf!("Undone {label}", label = tr(&label.unwrap_or_default())));
                }
            }
            Command::Redo => {
                let label = self.session().redo_label().map(str::to_string);
                if self.session_mut().redo() {
                    self.message(trf!("Redone {label}", label = tr(&label.unwrap_or_default())));
                }
            }
            Command::Copy => {
                if self.session().copy() {
                    self.message(trn!("Copied {n} item", "Copied {n} items", selection.len()));
                }
            }
            Command::Paste => {
                if self.session_mut().paste() {
                    self.message(tr("Pasted"));
                }
            }
            Command::Duplicate => {
                if !selection.is_empty() {
                    let created = self.session_mut().edit("Duplicate", |doc| ops::duplicate(doc, &selection));
                    self.session_mut().selection = created;
                }
            }
            Command::Delete => {
                if !selection.is_empty() {
                    self.session_mut().edit("Delete", |doc| ops::delete(doc, &selection));
                    self.session_mut().select(None);
                }
            }
            Command::SelectAll => {
                let all: Vec<NodeId> = self.session().doc.children(None).map(|n| n.id).collect();
                self.session_mut().selection = all;
            }
            Command::SelectNone => self.session_mut().select(None),
            Command::Add(shape) => {
                let parent = self.insertion_parent();
                let element = ops::default_element(shape);
                let label = format!("Add {}", shape.name());
                let id = self
                    .session_mut()
                    .edit(&label, |doc| ops::add_element(doc, &shape.name().to_lowercase(), element, parent));
                self.session_mut().select(Some(id));
                if self.mode == Mode::Edit && self.edit_tool == Tool::Select {
                    self.edit_tool = Tool::Move;
                }
            }
            Command::AddGroup => {
                let parent = self.insertion_parent();
                let id = self.session_mut().edit("Add Group", |doc| ops::add_group(doc, "group", Vec3::ZERO, parent));
                self.session_mut().select(Some(id));
            }
            Command::GroupSelection => {
                if selection.is_empty() {
                    return;
                }
                let id = self.session_mut().edit("Group", |doc| {
                    let roots = ops::top_level(doc, &selection);
                    let parent = doc.node(roots[0]).and_then(|n| n.parent);
                    let origin = doc.bounds(&roots).map_or(Vec3::ZERO, |(min, max)| Vec3::new((min.x + max.x) * 0.5, min.y, (min.z + max.z) * 0.5));
                    let group = ops::add_group(doc, "group", origin, parent);
                    // Группа встаёт на место первого из выделенных.
                    if let (Some(at), Some(from)) = (doc.index_of(roots[0]), doc.index_of(group)) {
                        let node = doc.nodes.remove(from);
                        doc.nodes.insert(at, node);
                    }
                    for id in roots {
                        ops::reparent(doc, id, Some(group), None);
                    }
                    group
                });
                self.session_mut().select(Some(id));
            }
            Command::MirrorX => {
                let created = self.session_mut().edit("Mirror X", |doc| ops::mirror_x(doc, &selection));
                if created.is_empty() {
                    self.message(tr("Mirror X needs selected elements without a twin"));
                } else {
                    self.message(trn!("Created {n} mirrored twin: edits on either side are repeated", "Created {n} mirrored twins: edits on either side are repeated", created.len()));
                }
            }
            Command::UnlinkTwin => {
                self.session_mut().edit("Unlink Twin", |doc| {
                    for id in &selection {
                        ops::unlink_twin(doc, *id);
                    }
                });
            }
            Command::Flip(axis) => self.session_mut().edit("Flip", |doc| ops::flip(doc, &selection, axis)),
            Command::Align(axis, mode) => self.session_mut().edit("Align", |doc| ops::align(doc, &selection, axis, mode)),
            Command::Distribute(axis) => self.session_mut().edit("Distribute", |doc| ops::distribute(doc, &selection, axis)),
            Command::Center => {
                let ids = if selection.is_empty() {
                    self.session().doc.children(None).map(|n| n.id).collect()
                } else {
                    selection
                };
                self.session_mut().edit("Center", |doc| ops::center(doc, &ids));
            }
            Command::ToggleVisibility => {
                self.session_mut().edit("Visibility", |doc| {
                    for id in &selection {
                        let visible = doc.node(*id).is_some_and(|n| n.visible);
                        ops::set_visible(doc, *id, !visible);
                    }
                });
            }
            Command::AutoUv => {
                let ids = if selection.is_empty() {
                    self.session().doc.elements().map(|(id, _)| id).collect()
                } else {
                    selection
                };
                self.session_mut().edit("Auto UV", |doc| ops::auto_uv(doc, &ids));
            }
            Command::Repack => {
                let texture = self.uv.texture;
                let ok = self.session_mut().edit("Repack UV", |doc| uv::repack(doc, texture));
                if !ok {
                    self.error(tr("the layouts do not fit even on a 2048 texture"));
                }
            }
            Command::AddTexture => {
                let index = self.session_mut().edit("Add Texture", |doc| {
                    let name = format!("texture{}", doc.textures.len() + 1);
                    ops::add_texture(doc, &name, 64, 64)
                });
                self.uv.texture = index;
            }
            Command::ResizeTexture(w, h) => {
                let texture = self.uv.texture;
                self.session_mut().edit("Resize Texture", |doc| ops::resize_texture(doc, texture, w, h));
            }
            Command::Frame => {
                let doc = &self.session().doc;
                let bounds = if selection.is_empty() { doc.model_bounds() } else { doc.bounds(&selection) };
                if let Some((min, max)) = bounds {
                    self.viewport.camera.frame(min, max);
                }
            }
            Command::View(preset) => self.viewport.camera.set_preset(preset),
            Command::ToggleOrthographic => self.viewport.camera.orthographic = !self.viewport.camera.orthographic,
            Command::Shading(shading) => self.view.shading = shading,
            Command::ToggleGrid => self.view.grid = !self.view.grid,
            Command::ToggleMode => {
                self.set_mode(if self.mode == Mode::Edit { Mode::Paint } else { Mode::Edit });
            }
            Command::SetMode(mode) => self.set_mode(mode),
            Command::Tool(tool) => {
                if tool.is_paint() {
                    self.paint_tool = tool;
                    self.set_mode(Mode::Paint);
                } else {
                    self.edit_tool = tool;
                    self.set_mode(Mode::Edit);
                }
            }
            Command::RestoreRecovery => {
                let tab = &mut self.tabs[self.active];
                if let Some(file) = tab.recovery.take() {
                    match format::load(&file) {
                        Ok(doc) => {
                            let session = &mut tab.session;
                            session.edit("Restore Recovered Changes", |d| *d = doc);
                            self.message(tr("Recovered the unsaved changes; save to keep them"));
                        }
                        Err(e) => self.error(e.to_string()),
                    }
                }
            }
            Command::UndoSteps(n) => {
                for _ in 0..n {
                    if !self.session_mut().undo() {
                        break;
                    }
                }
            }
            Command::ShowShortcuts => self.show_shortcuts = true,
            Command::DiscardRecovery => {
                let tab = &mut self.tabs[self.active];
                if let Some(file) = tab.recovery.take() {
                    let _ = std::fs::remove_file(file);
                }
            }
        }
    }

    fn set_mode(&mut self, mode: Mode) {
        if self.mode != mode {
            self.session_mut().commit();
            self.mode = mode;
        }
    }

    /// Куда добавлять новое: в выделенную группу или к родителю выделенного элемента.
    fn insertion_parent(&self) -> Option<NodeId> {
        let doc = &self.session().doc;
        let node = doc.node(self.session().primary()?)?;
        if node.is_group() {
            Some(node.id)
        } else {
            node.parent
        }
    }

    /// Путь из окна ввода: относительный — от каталога документа (или рабочего); без
    /// расширения — с расширением по умолчанию.
    fn resolve(&self, path: &Path, extension: &str) -> PathBuf {
        let mut path = path.to_path_buf();
        if path.extension().is_none() {
            path.set_extension(extension);
        }
        if path.is_absolute() {
            return path;
        }
        let base = self
            .path()
            .and_then(|p| p.parent())
            .map(Path::to_path_buf)
            .unwrap_or_else(|| self.working_dir.clone());
        base.join(path)
    }

    fn relative_to_document(&self, path: &Path) -> String {
        if let Some(dir) = self.path().and_then(|p| p.parent()) {
            if let Ok(rel) = path.strip_prefix(dir) {
                return rel.to_string_lossy().replace('\\', "/");
            }
        }
        path.to_string_lossy().to_string()
    }

    fn ask(&mut self, kind: PromptKind) {
        let doc = &self.session().doc;
        let stem = self
            .path()
            .and_then(|p| p.file_stem())
            .map_or(doc.name.clone(), |s| s.to_string_lossy().to_string());
        let text = match kind {
            PromptKind::Open => String::new(),
            PromptKind::SaveAs => format!("{stem}.pmodel"),
            PromptKind::ExportGlb => format!("{stem}.glb"),
            PromptKind::ExportObj => format!("{stem}.obj"),
            PromptKind::ExportTexture => format!("{stem}_texture.png"),
            PromptKind::ImportTexture => String::new(),
            PromptKind::ImportReference => String::new(),
            PromptKind::ResizeTexture => {
                let t = doc.textures.get(self.uv.texture as usize);
                t.map_or("64x64".to_string(), |t| format!("{}x{}", t.image.width(), t.image.height()))
            }
        };
        self.prompt = Some(Prompt {
            kind,
            text,
            error: None,
            focus: true,
        });
    }

    // ------------------------------------------------------------------
    // Кадр
    // ------------------------------------------------------------------

    /// Нарисовать окно моделей в `ui` (всё доступное место).
    pub fn show(&mut self, ui: &mut Ui) {
        let ctx = ui.ctx().clone();
        self.time = ctx.input(|i| i.time);
        let p = Palette::from_ctx(&ctx);
        let full = ui.max_rect();
        ui.painter().rect_filled(full, 0.0, p.bg_app);

        // Клавиатура — окну, если с ним работали последним.
        if std::mem::take(&mut self.drop_focus) {
            ctx.memory_mut(|m| m.stop_text_input());
        }
        let pressed_inside = ctx.input(|i| {
            i.pointer.any_pressed() && i.pointer.interact_pos().is_some_and(|pos| full.contains(pos))
        });
        let pressed_outside = ctx.input(|i| {
            i.pointer.any_pressed() && i.pointer.interact_pos().is_some_and(|pos| !full.contains(pos))
        });
        if pressed_inside {
            self.keyboard_focus = true;
        } else if pressed_outside && self.prompt.is_none() {
            self.keyboard_focus = false;
        }
        if self.keyboard_focus && self.prompt.is_none() && !ctx.wants_keyboard_input() {
            self.shortcuts(&ctx);
        }

        let panel_frame = egui::Frame::none().fill(p.bg_panel);
        let bar = |fill: Color32, height: f32| {
            egui::Frame::none()
                .fill(fill)
                .inner_margin(egui::Margin::symmetric(kit::SP_M, ((height - style::CONTROL_H) * 0.5).max(0.0)))
        };
        egui::TopBottomPanel::top("modeler.menu")
            .exact_height(kit::MENU_BAR + 2.0)
            .show_separator_line(false)
            .frame(bar(p.bg_app, kit::MENU_BAR + 2.0))
            .show_inside(ui, |ui| self.menu_bar(ui));
        egui::TopBottomPanel::top("modeler.tabs")
            .exact_height(kit::TAB_BAR + 4.0)
            .show_separator_line(false)
            .frame(bar(p.bg_app, kit::TAB_BAR + 4.0))
            .show_inside(ui, |ui| self.tab_strip(ui));
        egui::TopBottomPanel::top("modeler.toolbar")
            .exact_height(kit::MAIN_TOOLBAR)
            .frame(bar(p.bg_panel, kit::MAIN_TOOLBAR))
            .show_inside(ui, |ui| self.toolbar(ui));
        egui::TopBottomPanel::bottom("modeler.status")
            .exact_height(kit::STATUS_BAR)
            .frame(bar(p.bg_app, kit::STATUS_BAR))
            .show_inside(ui, |ui| self.status_bar(ui));
        egui::SidePanel::left("modeler.tools")
            .exact_width(40.0)
            .resizable(false)
            .frame(panel_frame.inner_margin(egui::Margin::symmetric(6.0, 8.0)))
            .show_inside(ui, |ui| self.tool_column(ui));
        egui::SidePanel::right("modeler.panels")
            .default_width(304.0)
            .width_range(240.0..=520.0)
            .frame(panel_frame)
            .show_inside(ui, |ui| self.right_panels(ui));
        egui::CentralPanel::default()
            .frame(egui::Frame::none().fill(p.bg_inset))
            .show_inside(ui, |ui| {
                let tab = &mut self.tabs[self.active];
                let tool = match self.mode {
                    Mode::Edit => self.edit_tool,
                    Mode::Paint => self.paint_tool,
                };
                let mut ctx = viewport::Context {
                    session: &mut tab.session,
                    mode: self.mode,
                    tool,
                    paint: &mut self.paint,
                    view: &self.view,
                };
                let picked = self.viewport.show(ui, &mut ctx);
                if let Some(preset) = picked {
                    self.viewport.camera.set_preset(preset);
                }
                self.recovery_banner(ui);
            });
        // Правка числовых полей: шаг истории закрывается, когда мышь отпущена.
        if self.field_edit && !ctx.input(|i| i.pointer.any_down()) && !ctx.wants_keyboard_input() {
            self.session_mut().commit();
            self.field_edit = false;
        }
        self.prompt_window(&ctx);
        self.shortcuts_window(&ctx);
        self.background_work();
    }

    fn shortcuts(&mut self, ctx: &egui::Context) {
        let ctrl = |key| KeyboardShortcut::new(Modifiers::COMMAND, key);
        let ctrl_shift = |key| KeyboardShortcut::new(Modifiers::COMMAND | Modifiers::SHIFT, key);
        let mut commands = Vec::new();
        ctx.input_mut(|i| {
            let mut take = |shortcut: KeyboardShortcut, command: Command| {
                if i.consume_shortcut(&shortcut) {
                    commands.push(command);
                }
            };
            take(ctrl_shift(Key::Z), Command::Redo);
            take(ctrl(Key::Z), Command::Undo);
            take(ctrl(Key::Y), Command::Redo);
            take(ctrl(Key::S), Command::Save);
            take(ctrl(Key::D), Command::Duplicate);
            take(ctrl(Key::C), Command::Copy);
            take(ctrl(Key::V), Command::Paste);
            take(ctrl(Key::A), Command::SelectAll);
            take(ctrl(Key::G), Command::GroupSelection);
            take(ctrl(Key::N), Command::New);
            take(ctrl(Key::Num1), Command::View(ViewPreset::Back));
            take(ctrl(Key::Num3), Command::View(ViewPreset::Left));
            take(ctrl(Key::Num7), Command::View(ViewPreset::Bottom));
            let plain = |i: &mut egui::InputState, key: Key| i.consume_key(Modifiers::NONE, key);
            if plain(i, Key::Tab) {
                commands.push(Command::ToggleMode);
                self.drop_focus = true;
            }
            if plain(i, Key::Delete) || plain(i, Key::Backspace) {
                commands.push(Command::Delete);
            }
            if plain(i, Key::F1) {
                commands.push(Command::ShowShortcuts);
            }
            if plain(i, Key::Escape) {
                commands.push(Command::SelectNone);
            }
            for (key, command) in [
                (Key::F, Command::Frame),
                (Key::Num1, Command::View(ViewPreset::Front)),
                (Key::Num3, Command::View(ViewPreset::Right)),
                (Key::Num7, Command::View(ViewPreset::Top)),
                (Key::Num5, Command::ToggleOrthographic),
                (Key::H, Command::ToggleVisibility),
                (Key::M, Command::MirrorX),
            ] {
                if plain(i, key) {
                    commands.push(command);
                }
            }
            let tools: &[Tool] = if self.mode == Mode::Edit { &Tool::EDIT } else { &Tool::PAINT };
            for tool in tools {
                if plain(i, tool.key()) {
                    commands.push(Command::Tool(*tool));
                }
            }
            if self.mode == Mode::Paint {
                if plain(i, Key::X) {
                    std::mem::swap(&mut self.paint.color, &mut self.paint.secondary);
                }
                if plain(i, Key::OpenBracket) {
                    self.paint.size = self.paint.size.saturating_sub(1).max(1);
                }
                if plain(i, Key::CloseBracket) {
                    self.paint.size = (self.paint.size + 1).min(16);
                }
            }
        });
        for command in commands {
            self.run(command);
        }
    }

    fn menu_bar(&mut self, ui: &mut Ui) {
        let p = Palette::from_ctx(ui.ctx());
        let mut command = None;
        ui.horizontal(|ui| {
            ui.spacing_mut().item_spacing.x = 2.0;
            // Знак программы: две грани кубоида.
            let (mark, _) = ui.allocate_exact_size(vec2(18.0, 18.0), Sense::hover());
            icons::paint(ui.painter(), mark.shrink(1.0), Icon::Cuboid, p.accent);
            ui.add_space(4.0);
            // Меню — обычными кнопками в строке (без `menu::bar`, который занимает всю ширину:
            // справа от меню стоят вкладки документов).
            ui.scope(|ui| {
                ui.visuals_mut().button_frame = false;
                ui.spacing_mut().item_spacing.x = kit::SP_L;
                let item = |ui: &mut Ui, text: &str, shortcut: &str, enabled: bool| {
                    ui.add_enabled(enabled, egui::Button::new(tr(text)).shortcut_text(shortcut)).clicked()
                };
                let has_selection = !self.session().selection.is_empty();
                ui.menu_button(tr_ctx("menu", "File"), |ui| {
                    ui.set_min_width(220.0);
                    if item(ui, "New", "Ctrl+N", true) {
                        command = Some(Command::New);
                    }
                    if item(ui, "Open…", "", true) {
                        self.ask(PromptKind::Open);
                    }
                    if item(ui, "Save", "Ctrl+S", true) {
                        command = Some(Command::Save);
                    }
                    if item(ui, "Save As…", "", true) {
                        self.ask(PromptKind::SaveAs);
                    }
                    ui.separator();
                    if item(ui, "Import Texture (PNG)…", "", true) {
                        self.ask(PromptKind::ImportTexture);
                    }
                    if item(ui, "Import Reference (.obj, .glb)…", "", true) {
                        self.ask(PromptKind::ImportReference);
                    }
                    ui.separator();
                    if item(ui, "Export glTF Binary (.glb)…", "", true) {
                        self.ask(PromptKind::ExportGlb);
                    }
                    if item(ui, "Export Wavefront (.obj + .mtl)…", "", true) {
                        self.ask(PromptKind::ExportObj);
                    }
                    if item(ui, "Export Texture (PNG)…", "", true) {
                        self.ask(PromptKind::ExportTexture);
                    }
                    ui.separator();
                    if item(ui, "Close Document", "", true) {
                        command = Some(Command::CloseTab);
                    }
                });
                ui.menu_button(tr_ctx("menu", "Edit"), |ui| {
                    ui.set_min_width(220.0);
                    let undo = self.session().undo_label().map(|l| trf!("Undo {label}", label = tr(l))).unwrap_or_else(|| tr("Undo").to_string());
                    let redo = self.session().redo_label().map(|l| trf!("Redo {label}", label = tr(l))).unwrap_or_else(|| tr("Redo").to_string());
                    if item(ui, &undo, "Ctrl+Z", self.session().can_undo()) {
                        command = Some(Command::Undo);
                    }
                    if item(ui, &redo, "Ctrl+Y", self.session().can_redo()) {
                        command = Some(Command::Redo);
                    }
                    ui.separator();
                    if item(ui, "Copy", "Ctrl+C", has_selection) {
                        command = Some(Command::Copy);
                    }
                    if item(ui, "Paste", "Ctrl+V", clipboard_has_content()) {
                        command = Some(Command::Paste);
                    }
                    if item(ui, "Duplicate", "Ctrl+D", has_selection) {
                        command = Some(Command::Duplicate);
                    }
                    if item(ui, "Delete", "Del", has_selection) {
                        command = Some(Command::Delete);
                    }
                    ui.separator();
                    if item(ui, "Select All", "Ctrl+A", true) {
                        command = Some(Command::SelectAll);
                    }
                    if item(ui, "Select None", "Esc", has_selection) {
                        command = Some(Command::SelectNone);
                    }
                });
                ui.menu_button(tr_ctx("menu", "Model"), |ui| {
                    ui.set_min_width(230.0);
                    for (shape, label) in ADD_SHAPES {
                        if item(ui, add_label(label), "", true) {
                            command = Some(Command::Add(shape));
                        }
                    }
                    if item(ui, "Add Group", "", true) {
                        command = Some(Command::AddGroup);
                    }
                    if item(ui, "Group Selection", "Ctrl+G", has_selection) {
                        command = Some(Command::GroupSelection);
                    }
                    ui.separator();
                    if item(ui, "Mirror X (linked twin)", "M", has_selection) {
                        command = Some(Command::MirrorX);
                    }
                    if item(ui, "Unlink Twin", "", has_selection) {
                        command = Some(Command::UnlinkTwin);
                    }
                    ui.menu_button(tr("Flip"), |ui| {
                        for (axis, name) in ["X", "Y", "Z"].iter().enumerate() {
                            if item(ui, &trf!("Flip {name}", name = name), "", has_selection) {
                                command = Some(Command::Flip(axis));
                            }
                        }
                    });
                    ui.menu_button(tr("Align"), |ui| {
                        for (axis, name) in ["X", "Y", "Z"].iter().enumerate() {
                            for (mode, label) in [(Align::Min, n_("Min")), (Align::Center, n_("Center")), (Align::Max, n_("Max"))] {
                                if item(ui, &trf!("{axis} {mode}", axis = name, mode = tr(label)), "", has_selection) {
                                    command = Some(Command::Align(axis, mode));
                                }
                            }
                        }
                    });
                    ui.menu_button(tr("Distribute"), |ui| {
                        for (axis, name) in ["X", "Y", "Z"].iter().enumerate() {
                            if item(ui, &trf!("Along {name}", name = name), "", has_selection) {
                                command = Some(Command::Distribute(axis));
                            }
                        }
                    });
                    if item(ui, "Center on Origin", "", true) {
                        command = Some(Command::Center);
                    }
                    if item(ui, "Hide / Show", "H", has_selection) {
                        command = Some(Command::ToggleVisibility);
                    }
                });
                ui.menu_button(tr_ctx("menu", "Texture"), |ui| {
                    ui.set_min_width(220.0);
                    if item(ui, "Auto UV (box layout)", "", true) {
                        command = Some(Command::AutoUv);
                    }
                    if item(ui, "Repack UV", "", true) {
                        command = Some(Command::Repack);
                    }
                    ui.separator();
                    if item(ui, "Add Texture", "", true) {
                        command = Some(Command::AddTexture);
                    }
                    if item(ui, "Resize Texture…", "", true) {
                        self.ask(PromptKind::ResizeTexture);
                    }
                });
                ui.menu_button(tr_ctx("menu", "View"), |ui| {
                    ui.set_min_width(220.0);
                    for (preset, label, key) in [
                        (ViewPreset::Front, n_("Front"), "1"),
                        (ViewPreset::Back, n_("Back"), "Ctrl+1"),
                        (ViewPreset::Right, n_("Right"), "3"),
                        (ViewPreset::Left, n_("Left"), "Ctrl+3"),
                        (ViewPreset::Top, n_("Top"), "7"),
                        (ViewPreset::Bottom, n_("Bottom"), "Ctrl+7"),
                    ] {
                        if item(ui, label, key, true) {
                            command = Some(Command::View(preset));
                        }
                    }
                    ui.separator();
                    let ortho = self.viewport.camera.orthographic;
                    if item(ui, if ortho { "Perspective" } else { "Orthographic" }, "5", true) {
                        command = Some(Command::ToggleOrthographic);
                    }
                    if item(ui, "Frame Selection", "F", true) {
                        command = Some(Command::Frame);
                    }
                    ui.separator();
                    ui.checkbox(&mut self.view.grid, tr("Grid"));
                    ui.checkbox(&mut self.view.ruler, tr("Size Ruler"));
                    let mut smooth = self.view.supersample > 1;
                    if ui.checkbox(&mut smooth, tr("Smooth Edges (2× supersampling)")).changed() {
                        self.view.supersample = if smooth { 2 } else { 1 };
                    }
                });
                ui.menu_button(tr_ctx("menu", "Help"), |ui| {
                    ui.set_min_width(220.0);
                    if item(ui, "Keyboard Shortcuts", "F1", true) {
                        command = Some(Command::ShowShortcuts);
                    }
                });
            });

        });
        if let Some(command) = command {
            self.run(command);
        }
    }

    /// Полоса под меню: переключатель режима (всегда на одном месте) и вкладки документов.
    fn tab_strip(&mut self, ui: &mut Ui) {
        let p = Palette::from_ctx(ui.ctx());
        let mut command = None;
        ui.horizontal_centered(|ui| {
            ui.spacing_mut().item_spacing.x = kit::SP_S;
            // i18n-ctx: mode "Edit" "Paint"
            let modes = [
                (Mode::Edit, Some(Icon::Cuboid), tr_ctx("mode", "Edit"), tr("Edit the shape: add, move, resize, rotate (Tab switches)")),
                (Mode::Paint, Some(Icon::Pencil), tr_ctx("mode", "Paint"), tr("Paint the texture on the model and in the UV view (Tab switches)")),
            ];
            let options: Vec<(Mode, Option<Icon>, &str, &str)> =
                modes.iter().map(|(m, i, l, t)| (*m, *i, l.as_ref(), t.as_ref())).collect();
            if let Some(mode) = style::segmented(ui, "modeler.mode", self.mode, &options) {
                command = Some(Command::SetMode(mode));
            }
            style::hairline(ui);
            // Вкладки документов: имя, точка «не сохранено», крестик; «+» — новый документ.
            let mut switch = None;
            let mut close = None;
            for (i, tab) in self.tabs.iter().enumerate() {
                let active = i == self.active;
                let title = tab.title();
                let font = egui::FontId::proportional(kit::FONT_BODY);
                let text_w = ui.painter().layout_no_wrap(title.clone(), font.clone(), p.text).size().x;
                let width = kit::SP_M + 8.0 + kit::SP_S + text_w + kit::SP_S + 16.0 + kit::SP_S;
                let (rect, _) = ui.allocate_exact_size(vec2(width, kit::TAB_BAR), Sense::hover());
                let response = ui.interact(rect, egui::Id::new(("modeler.tab", i)), Sense::click());
                let fill = if active { p.bg_panel } else if response.hovered() { p.bg_raised } else { Color32::TRANSPARENT };
                ui.painter().rect_filled(rect, egui::Rounding { nw: kit::R_L, ne: kit::R_L, sw: 0.0, se: 0.0 }, fill);
                if active {
                    ui.painter().line_segment([rect.left_top(), rect.right_top()], Stroke::new(2.0_f32, p.accent));
                }
                let dot = pos2(rect.min.x + kit::SP_M + 4.0, rect.center().y);
                if tab.session.is_dirty() {
                    ui.painter().circle_filled(dot, 3.5, p.text);
                } else {
                    ui.painter().circle_stroke(dot, 3.0, Stroke::new(1.0_f32, p.text_faint));
                }
                ui.painter().text(
                    pos2(dot.x + 4.0 + kit::SP_S, rect.center().y),
                    egui::Align2::LEFT_CENTER,
                    &title,
                    font,
                    if active { p.text } else { p.text_muted },
                );
                let close_rect = Rect::from_center_size(pos2(rect.max.x - kit::SP_S - 8.0, rect.center().y), vec2(16.0, 16.0));
                let close_response = ui.interact(close_rect, egui::Id::new(("modeler.tab.close", i)), Sense::click());
                if close_response.hovered() {
                    ui.painter().rect_filled(close_rect, kit::R_S, p.hover);
                }
                if active || response.hovered() || close_response.hovered() {
                    icons::paint(ui.painter(), close_rect.shrink(4.0), Icon::Close, if close_response.hovered() { p.text } else { p.text_muted });
                }
                if close_response.on_hover_text(tr("Close document")).clicked() {
                    close = Some(i);
                } else if response.clicked() {
                    switch = Some(i);
                } else if response.middle_clicked() {
                    close = Some(i);
                }
                let tip = match &tab.path {
                    Some(path) => path.display().to_string(),
                    None => tr("Not saved yet").to_string(),
                };
                let status = if tab.session.is_dirty() { tr("unsaved changes") } else { tr("saved") };
                response.on_hover_text(format!("{tip}\n{status}"));
            }
            if style::icon_button(ui, "modeler.tab.new", Icon::Plus, false, true, &style::tip(&tr("New Document"), "Ctrl+N", "")).clicked() {
                command = Some(Command::New);
            }
            if let Some(i) = switch {
                self.active = i;
            }
            if let Some(i) = close {
                self.active = i;
                command = Some(Command::CloseTab);
            }
        });
        if let Some(command) = command {
            self.run(command);
        }
    }

    /// Полоса настроек инструмента: значок и имя активного инструмента, затем только его
    /// настройки группами с подписями; справа — вид и история.
    fn toolbar(&mut self, ui: &mut Ui) {
        let mut command = None;
        let tool = self.tool();
        ui.horizontal_centered(|ui| {
            ui.spacing_mut().item_spacing.x = kit::SP_S;
            style::tool_badge(ui, tool.icon(), &tr(tool.name()));
            style::hairline(ui);
            match tool {
                Tool::Select => {
                    style::caption(ui, &tr("Click an element, drag on empty space for a box selection"));
                }
                Tool::Move | Tool::Resize | Tool::Pivot => self.snap_group(ui),
                Tool::Rotate => self.angle_group(ui),
                Tool::Pencil | Tool::Line => {
                    self.colour_group(ui);
                    style::hairline(ui);
                    self.size_group(ui);
                    style::hairline(ui);
                    self.mirror_group(ui);
                }
                Tool::Rectangle => {
                    self.colour_group(ui);
                    style::hairline(ui);
                    self.size_group(ui);
                    style::hairline(ui);
                    style::caption(ui, &tr("Style"));
                    let options = [
                        (false, Some(Icon::Rectangle), tr("Outline"), tr("Only the border (Shift while dragging: filled)")),
                        (true, None, tr("Filled"), tr("The whole rectangle (Shift while dragging: outline)")),
                    ];
                    let options: Vec<(bool, Option<Icon>, &str, &str)> =
                        options.iter().map(|(v, i, l, t)| (*v, *i, l.as_ref(), t.as_ref())).collect();
                    if let Some(v) = style::segmented(ui, "modeler.rect_style", self.paint.filled, &options) {
                        self.paint.filled = v;
                    }
                    style::hairline(ui);
                    self.mirror_group(ui);
                }
                Tool::Eraser => {
                    self.size_group(ui);
                    style::hairline(ui);
                    self.mirror_group(ui);
                }
                Tool::Fill => {
                    self.colour_group(ui);
                    style::hairline(ui);
                    style::caption(ui, &tr("Mode"));
                    let options = [
                        (FillMode::Face, None, tr("Whole face"), tr("Fill every texel of the face under the cursor")),
                        (FillMode::Connected, None, tr("Same colour"), tr("Fill only the connected area of the colour under the cursor, inside that face")),
                    ];
                    let options: Vec<(FillMode, Option<Icon>, &str, &str)> =
                        options.iter().map(|(v, i, l, t)| (*v, *i, l.as_ref(), t.as_ref())).collect();
                    if let Some(v) = style::segmented(ui, "modeler.fill_mode", self.paint.fill, &options) {
                        self.paint.fill = v;
                    }
                    style::hairline(ui);
                    self.mirror_group(ui);
                }
                Tool::Picker => {
                    self.colour_group(ui);
                    style::hairline(ui);
                    style::caption(ui, &tr("Click a texel to make its colour the brush colour"));
                }
                Tool::Shade => {
                    self.size_group(ui);
                    style::hairline(ui);
                    style::caption(ui, &tr("Mode"));
                    let options = [
                        (false, None, tr("Lighten"), tr("Each stroke makes texels lighter (Shift: darker)")),
                        (true, None, tr("Darken"), tr("Each stroke makes texels darker (Shift: lighter)")),
                    ];
                    let options: Vec<(bool, Option<Icon>, &str, &str)> =
                        options.iter().map(|(v, i, l, t)| (*v, *i, l.as_ref(), t.as_ref())).collect();
                    if let Some(v) = style::segmented(ui, "modeler.shade_mode", self.paint.darken, &options) {
                        self.paint.darken = v;
                    }
                    style::hairline(ui);
                    style::caption(ui, &tr("Strength"));
                    let mut percent = (self.paint.shade as f32 / 255.0 * 100.0).round() as u32;
                    if ui
                        .add_sized(
                            [96.0, style::CONTROL_H],
                            egui::Slider::new(&mut percent, 1..=40).suffix(" %").show_value(true),
                        )
                        .on_hover_text(tr("How much one stroke changes each texel, in % of full brightness"))
                        .changed()
                    {
                        self.paint.shade = ((percent as f32 / 100.0) * 255.0).round().max(1.0) as i32;
                    }
                    style::hairline(ui);
                    self.mirror_group(ui);
                }
            }

            // Справа: вид, затем история.
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                ui.spacing_mut().item_spacing.x = kit::SP_S;
                // История: Redo, Undo и список шагов.
                let labels: Vec<String> = self.session().undo_labels().iter().take(20).map(|l| l.to_string()).collect();
                ui.add_enabled_ui(!labels.is_empty(), |ui| {
                    ui.menu_button(tr("History"), |ui| {
                        ui.set_min_width(200.0);
                        style::caption(ui, &tr("Undo back to…"));
                        for (i, label) in labels.iter().enumerate() {
                            if ui.button(tr(label)).clicked() {
                                command = Some(Command::UndoSteps(i + 1));
                                ui.close_menu();
                            }
                        }
                    })
                    .response
                    .on_hover_text(tr("History: undo several steps at once"));
                });
                if style::icon_button(ui, "modeler.redo", Icon::Redo, false, self.session().can_redo(), &style::tip(&tr("Redo"), "Ctrl+Y", "")).clicked() {
                    command = Some(Command::Redo);
                }
                if style::icon_button(ui, "modeler.undo", Icon::Undo, false, self.session().can_undo(), &style::tip(&tr("Undo"), "Ctrl+Z", "")).clicked() {
                    command = Some(Command::Undo);
                }
                style::hairline(ui);
                if style::icon_button(ui, "modeler.view.frame", Icon::Frame, false, true, &style::tip(&tr("Frame"), "F", &tr("Fit the selection or the model in view"))).clicked() {
                    command = Some(Command::Frame);
                }
                let ortho = self.viewport.camera.orthographic;
                if style::icon_button(
                    ui,
                    "modeler.view.projection",
                    if ortho { Icon::Orthographic } else { Icon::Perspective },
                    false,
                    true,
                    &style::tip(&tr(if ortho { "Orthographic" } else { "Perspective" }), "5", &tr("Switch the projection")),
                )
                .clicked()
                {
                    command = Some(Command::ToggleOrthographic);
                }
                if style::icon_button(ui, "modeler.view.grid", Icon::Grid, self.view.grid, true, &style::tip(&tr("Grid"), "", &tr("Show the ground grid"))).clicked() {
                    command = Some(Command::ToggleGrid);
                }
                let shading = [
                    (Shading::Textured, Some(Icon::Textured), "", tr("Textured: the model with its texture")),
                    (Shading::Solid, Some(Icon::Solid), "", tr("Solid: shape and light only")),
                    (Shading::Wireframe, Some(Icon::Wireframe), "", tr("Wireframe: edges only")),
                ];
                // Справа налево: порядок вариантов обратный, чтобы читались слева направо.
                let options: Vec<(Shading, Option<Icon>, &str, &str)> =
                    shading.iter().map(|(v, i, l, t)| (*v, *i, *l, t.as_ref())).collect();
                if let Some(v) = style::segmented(ui, "modeler.shading", self.view.shading, &options) {
                    command = Some(Command::Shading(v));
                }
                style::caption(ui, &tr("View"));
            });
        });
        if let Some(command) = command {
            self.run(command);
        }
    }

    /// Привязка перемещения и размера.
    fn snap_group(&mut self, ui: &mut Ui) {
        style::caption(ui, &tr("Snap"));
        egui::ComboBox::from_id_salt("modeler.snap")
            .width(72.0)
            .selected_text(if self.view.snapping { format!("{} px", trim(self.view.snap_step)) } else { tr_ctx("snap", "Off").to_string() })
            .show_ui(ui, |ui| {
                ui.selectable_value(&mut self.view.snapping, false, tr_ctx("snap", "Off"));
                for step in [0.25, 0.5, 1.0, 2.0, 4.0, 8.0, 16.0] {
                    if ui.selectable_label(self.view.snapping && self.view.snap_step == step, format!("{} px", trim(step))).clicked() {
                        self.view.snapping = true;
                        self.view.snap_step = step;
                    }
                }
            })
            .response
            .on_hover_text(tr("Grid step for moving and resizing (16 px = 1 m). Ctrl while dragging: toggle, Shift: quarter steps"));
    }

    /// Привязка поворота.
    fn angle_group(&mut self, ui: &mut Ui) {
        style::caption(ui, &tr("Angle"));
        egui::ComboBox::from_id_salt("modeler.rotation_snap")
            .width(72.0)
            .selected_text(if self.view.rotation_step > 0.0 { format!("{}°", trim(self.view.rotation_step)) } else { tr("Free").to_string() })
            .show_ui(ui, |ui| {
                for step in [22.5, 15.0, 5.0, 0.0] {
                    let label = if step > 0.0 { format!("{}°", trim(step)) } else { tr("Free").to_string() };
                    ui.selectable_value(&mut self.view.rotation_step, step, label);
                }
            })
            .response
            .on_hover_text(tr("Rotation steps. Ctrl while dragging: toggle, Shift: 1° steps"));
    }

    /// Цвет кисти и второй цвет, обмен (X) и сброс.
    fn colour_group(&mut self, ui: &mut Ui) {
        style::caption(ui, &tr("Colour"));
        let primary = style::swatch(ui, egui::Id::new("modeler.toolbar.color"), style::to_color32(self.paint.color), style::CONTROL_H, false)
            .on_hover_text(trf!("Brush colour {hex} — click to edit in the Palette", hex = crate::image::to_hex(self.paint.color)));
        if primary.clicked() {
            self.open_sections[SECTION_PALETTE] = true;
        }
        let secondary = style::swatch(ui, egui::Id::new("modeler.toolbar.secondary"), style::to_color32(self.paint.secondary), style::CONTROL_H - 6.0, false)
            .on_hover_text(trf!("Second colour {hex} — click to make it the brush colour", hex = crate::image::to_hex(self.paint.secondary)));
        if secondary.clicked() {
            std::mem::swap(&mut self.paint.color, &mut self.paint.secondary);
        }
        if style::icon_button(ui, "modeler.colour.swap", Icon::Swap, false, true, &style::tip(&tr("Swap colours"), "X", "")).clicked() {
            std::mem::swap(&mut self.paint.color, &mut self.paint.secondary);
        }
        if style::icon_button(ui, "modeler.colour.reset", Icon::DefaultColors, false, true, &style::tip(&tr("Reset colours"), "", &tr("Back to the default brush and second colour"))).clicked() {
            let defaults = PaintSettings::default();
            self.paint.color = defaults.color;
            self.paint.secondary = defaults.secondary;
        }
    }

    /// Размер кисти.
    fn size_group(&mut self, ui: &mut Ui) {
        style::caption(ui, &tr("Size"));
        style::stepper(ui, "modeler.brush_size", &mut self.paint.size, 1..=16, " px", &tr("Brush size in texels ( [ and ] )"));
    }

    /// Зеркальное рисование.
    fn mirror_group(&mut self, ui: &mut Ui) {
        let on = self.paint.mirror;
        if style::toggle(ui, "modeler.paint_mirror", Icon::Mirror, &tr("Mirror X"), on, &tr("Paint on both sides of X = 0 at once")).clicked() {
            self.paint.mirror = !on;
        }
    }

    /// Окно Help ▸ Keyboard Shortcuts.
    fn shortcuts_window(&mut self, ctx: &egui::Context) {
        if !self.show_shortcuts {
            return;
        }
        let mut open = true;
        egui::Window::new(tr("Keyboard Shortcuts"))
            .id(egui::Id::new("modeler.shortcuts"))
            .open(&mut open)
            .collapsible(false)
            .resizable(false)
            .anchor(egui::Align2::CENTER_CENTER, vec2(0.0, 0.0))
            .show(ctx, |ui| {
                let rows: &[(&str, &str)] = &[
                    ("Tab", n_("Edit / Paint mode")),
                    ("Q  W  E  R  P", n_("Select, Move, Rotate, Resize, Pivot (Edit)")),
                    ("B  E  G  L  U  I  D", n_("Pencil, Eraser, Fill, Line, Rectangle, Picker, Shade (Paint)")),
                    ("[  ]   X", n_("Brush size, swap colours")),
                    ("Ctrl+Z  Ctrl+Y", n_("Undo, redo")),
                    ("Ctrl+C  Ctrl+V  Ctrl+D", n_("Copy, paste, duplicate")),
                    ("Del  Ctrl+G  M  H", n_("Delete, group, mirror twin, hide")),
                    ("F  1  3  7  5", n_("Frame, front / right / top view, projection")),
                    ("Ctrl+1  Ctrl+3  Ctrl+7", n_("Back / left / bottom view")),
                    ("Ctrl+N  Ctrl+S", n_("New, save")),
                    ("RMB  MMB  Wheel", n_("Orbit, pan, zoom")),
                ];
                egui::Grid::new("modeler.shortcuts.grid").num_columns(2).spacing([kit::SP_XL, kit::SP_S]).show(ui, |ui| {
                    for (keys, what) in rows {
                        ui.label(egui::RichText::new(*keys).monospace());
                        ui.label(tr(what));
                        ui.end_row();
                    }
                });
            });
        self.show_shortcuts = open;
    }

    fn tool_column(&mut self, ui: &mut Ui) {
        let p = Palette::from_ctx(ui.ctx());
        let mut command = None;
        ui.vertical_centered(|ui| {
            ui.spacing_mut().item_spacing.y = 3.0;
            let (tools, current): (&[Tool], Tool) = match self.mode {
                Mode::Edit => (&Tool::EDIT, self.edit_tool),
                Mode::Paint => (&Tool::PAINT, self.paint_tool),
            };
            for tool in tools {
                let id = format!("modeler.tool.{}", tool.name().to_lowercase().replace(' ', "_"));
                let tip = style::tip(tool.name(), &format!("{:?}", tool.key()), tool.hint());
                if style::icon_button(ui, &id, tool.icon(), *tool == current, true, &tip).clicked() {
                    command = Some(Command::Tool(*tool));
                }
            }
            if self.mode == Mode::Edit {
                ui.add_space(kit::SP_S);
                let (line, _) = ui.allocate_exact_size(vec2(24.0, 1.0), Sense::hover());
                ui.painter().rect_filled(line, 0.0, p.line_strong);
                ui.add_space(kit::SP_S);
                for (shape, label) in ADD_SHAPES {
                    let id = format!("modeler.add.{}", label.to_lowercase());
                    let tip = style::tip(add_label(label), "", &tr("Adds it at the origin, inside the selected group"));
                    if style::icon_button(ui, &id, shape_icon(&shape), false, true, &tip).clicked() {
                        command = Some(Command::Add(shape));
                    }
                }
                if style::icon_button(ui, "modeler.add.group", Icon::Group, false, true, &style::tip(&tr("Add Group"), "Ctrl+G", &tr("A named node with its own pivot"))).clicked() {
                    command = Some(Command::AddGroup);
                }
            }
            if self.mode == Mode::Paint {
                ui.add_space(8.0);
                let (rect, _) = ui.allocate_exact_size(vec2(28.0, 34.0), Sense::hover());
                let back = Rect::from_min_size(rect.min + vec2(9.0, 12.0), vec2(18.0, 18.0));
                let front = Rect::from_min_size(rect.min + vec2(1.0, 2.0), vec2(18.0, 18.0));
                ui.painter().rect_filled(back, 2.0, style::to_color32(self.paint.secondary));
                ui.painter().rect_stroke(back, 2.0, Stroke::new(1.0_f32, p.line_strong));
                ui.painter().rect_filled(front, 2.0, style::to_color32(self.paint.color));
                ui.painter().rect_stroke(front, 2.0, Stroke::new(1.0_f32, Color32::WHITE));
                let swap = ui.interact(rect, egui::Id::new("modeler.tools.swap"), Sense::click());
                if swap.on_hover_text(tr("Brush and second colour (X swaps)")).clicked() {
                    std::mem::swap(&mut self.paint.color, &mut self.paint.secondary);
                }
            }
        });
        if let Some(command) = command {
            self.run(command);
        }
    }

    fn right_panels(&mut self, ui: &mut Ui) {
        egui::ScrollArea::vertical()
            .id_salt("modeler.panels.scroll")
            .auto_shrink([false, false])
            .show(ui, |ui| {
                ui.spacing_mut().item_spacing.y = 0.0;
                // Outliner
                let mut open = self.open_sections[SECTION_OUTLINER];
                let mut command = None;
                style::section(ui, "Outliner", &mut open, |ui| {
                    if style::icon_button(ui, "modeler.outliner.add_group", Icon::Group, false, true, "Add Group").clicked() {
                        command = Some(Command::AddGroup);
                    }
                    if style::icon_button(ui, "modeler.outliner.add_cuboid", Icon::Cuboid, false, true, "Add Cuboid").clicked() {
                        command = Some(Command::Add(Shape::Cuboid));
                    }
                });
                self.open_sections[SECTION_OUTLINER] = open;
                if open {
                    let session = &mut self.tabs[self.active].session;
                    if let Some(c) = self.outliner.show(ui, session) {
                        command = Some(c);
                    }
                }
                if let Some(c) = command.take() {
                    self.run(c);
                }

                // Palette — только при рисовании.
                let mut open = self.open_sections[SECTION_PALETTE];
                if self.mode == Mode::Paint {
                    style::section(ui, "Palette", &mut open, |_| {});
                }
                self.open_sections[SECTION_PALETTE] = open;
                if open && self.mode == Mode::Paint {
                    panels::palette_panel(ui, &mut self.paint);
                }

                if self.mode == Mode::Edit {
                    self.element_section(ui);
                }

                // UV
                let mut open = self.open_sections[SECTION_UV];
                style::section(ui, "UV", &mut open, |_| {});
                self.open_sections[SECTION_UV] = open;
                if open {
                    let tab = &mut self.tabs[self.active];
                    let tool = match self.mode {
                        Mode::Edit => self.edit_tool,
                        Mode::Paint => self.paint_tool,
                    };
                    if let Some(c) = self.uv.show(ui, &mut tab.session, self.mode, tool, &mut self.paint) {
                        self.run(c);
                    }
                }

                if self.mode == Mode::Paint {
                    self.element_section(ui);
                }

                // Textures
                let mut open = self.open_sections[SECTION_TEXTURES];
                style::section(ui, "Textures", &mut open, |_| {});
                self.open_sections[SECTION_TEXTURES] = open;
                if open {
                    let tab = &mut self.tabs[self.active];
                    if let Some(c) = panels::textures_panel(ui, &mut tab.session, &mut self.uv.texture) {
                        match c {
                            panels::TextureAction::Command(c) => self.run(c),
                            panels::TextureAction::Ask(kind) => self.ask(kind),
                        }
                    }
                }
            });
    }

    /// Раздел Element (в режиме Paint — ниже UV).
    fn element_section(&mut self, ui: &mut Ui) {
        let mut open = self.open_sections[SECTION_ELEMENT];
        style::section(ui, "Element", &mut open, |_| {});
        self.open_sections[SECTION_ELEMENT] = open;
        if open {
            let session = &mut self.tabs[self.active].session;
            let edit = panels::element_panel(ui, session, &self.view);
            if edit.started_or_changed() {
                self.field_edit = true;
            }
            if let Some(c) = edit.command {
                self.run(c);
            }
        }
    }

    fn status_bar(&mut self, ui: &mut Ui) {
        let p = Palette::from_ctx(ui.ctx());
        ui.horizontal(|ui| {
            ui.spacing_mut().item_spacing.x = 14.0;
            let small = |text: String, color: Color32| egui::RichText::new(text).size(11.5).color(color);
            // Выделение.
            let session = self.session();
            let doc = &session.doc;
            let summary = match session.selection.as_slice() {
                [] => trf!(
                    "{elements} · {groups}",
                    elements = trn!("{n} element", "{n} elements", doc.element_count()),
                    groups = trn!("{n} group", "{n} groups", doc.nodes.len() - doc.element_count())
                ),
                [id] => match doc.node(*id) {
                    Some(node) => match node.element() {
                        Some(e) => format!(
                            "{} · {} · {}×{}×{} px",
                            node.name,
                            style::shape_name(&e.shape),
                            trim(e.size.x),
                            trim(e.size.y),
                            trim(e.size.z)
                        ),
                        None => trf!("{name} · group · {count} children", name = node.name, count = doc.children(Some(*id)).count()),
                    },
                    None => String::new(),
                },
                many => trf!("{count} selected", count = many.len()),
            };
            ui.label(small(summary, p.text));
            // Сообщение или подсказка инструмента.
            let fresh = self.status.as_ref().filter(|s| self.time - s.1 < 6.0);
            match fresh {
                Some((text, _, true)) => {
                    ui.label(small(text.clone(), p.error));
                }
                Some((text, _, false)) => {
                    ui.label(small(text.clone(), p.text_muted));
                }
                None => {
                    ui.label(small(tr(self.tool().mouse_hint()).to_string(), p.text_faint));
                }
            }
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                let doc = &self.session().doc;
                let triangles = self.viewport.triangle_count();
                ui.label(small(trf!("{triangles} tris", triangles = triangles), p.text_faint));
                ui.label(small(trf!("{px} px = 1 m", px = trim(doc.units_per_metre)), p.text_faint));
                let snap = if self.view.snapping { trf!("snap {step} px", step = trim(self.view.snap_step)) } else { tr("snap off").to_string() };
                ui.label(small(snap, p.text_faint));
                let mode = match self.mode {
                    Mode::Edit => tr_ctx("mode", "EDIT"),
                    Mode::Paint => tr_ctx("mode", "PAINT"),
                };
                ui.label(egui::RichText::new(mode.as_ref()).size(11.0).strong().color(p.accent));
            });
        });
    }

    fn recovery_banner(&mut self, ui: &mut Ui) {
        if self.tabs[self.active].recovery.is_none() {
            return;
        }
        let p = Palette::from_ctx(ui.ctx());
        let rect = ui.max_rect();
        let banner = Rect::from_min_size(pos2(rect.center().x - 250.0, rect.min.y + 10.0), vec2(500.0, 36.0));
        let mut command = None;
        ui.allocate_new_ui(egui::UiBuilder::new().max_rect(banner), |ui| {
            egui::Frame::none()
                .fill(p.bg_raised)
                .stroke(Stroke::new(1.0_f32, p.warn))
                .rounding(5.0)
                .inner_margin(egui::Margin::symmetric(10.0, 5.0))
                .show(ui, |ui| {
                    ui.horizontal(|ui| {
                        let (r, _) = ui.allocate_exact_size(vec2(16.0, 16.0), Sense::hover());
                        icons::paint(ui.painter(), r, Icon::Warning, p.warn);
                        ui.label(egui::RichText::new(tr("Unsaved changes from an earlier session were found")).size(12.5));
                        if ui.button(tr("Restore")).clicked() {
                            command = Some(Command::RestoreRecovery);
                        }
                        if ui.button(tr("Discard")).clicked() {
                            command = Some(Command::DiscardRecovery);
                        }
                    });
                });
        });
        if let Some(c) = command {
            self.run(c);
        }
    }

    fn prompt_window(&mut self, ctx: &egui::Context) {
        let Some(prompt) = self.prompt.as_mut() else { return };
        let (title, hint, button) = match prompt.kind {
            PromptKind::Open => (n_("Open Model"), "path/to/model.pmodel", n_("Open")),
            PromptKind::SaveAs => (n_("Save Model As"), "path/to/model.pmodel", n_("Save")),
            PromptKind::ExportGlb => (n_("Export glTF Binary"), "model.glb", n_("Export")),
            PromptKind::ExportObj => (n_("Export Wavefront OBJ"), "model.obj", n_("Export")),
            PromptKind::ExportTexture => (n_("Export Texture"), "texture.png", n_("Export")),
            PromptKind::ImportTexture => (n_("Import Texture"), "path/to/texture.png", n_("Import")),
            PromptKind::ImportReference => (n_("Import Reference Mesh"), "path/to/mesh.obj or .glb", n_("Import")),
            PromptKind::ResizeTexture => (n_("Resize Texture"), "64x64", n_("Resize")),
        };
        let mut submit = false;
        let mut cancel = false;
        let base = self
            .tabs
            .get(self.active)
            .and_then(|t| t.path.as_ref())
            .and_then(|p| p.parent())
            .map(Path::to_path_buf)
            .unwrap_or_else(|| self.working_dir.clone());
        // Файлы рядом — для Open и импорта.
        let suggestions: Vec<String> = match prompt.kind {
            PromptKind::Open => list_files(&base, &["pmodel"]),
            PromptKind::ImportTexture => list_files(&base, &["png"]),
            PromptKind::ImportReference => list_files(&base, &["obj", "glb", "gltf"]),
            _ => Vec::new(),
        };
        egui::Window::new(tr(title))
            .id(egui::Id::new("modeler.prompt"))
            .collapsible(false)
            .resizable(false)
            .anchor(egui::Align2::CENTER_CENTER, vec2(0.0, -40.0))
            .fixed_size(vec2(420.0, 0.0))
            .show(ctx, |ui| {
                if prompt.kind != PromptKind::ResizeTexture {
                    ui.label(egui::RichText::new(trf!("Relative paths start at {path}", path = base.display())).size(11.5).weak());
                }
                let response = ui.add(egui::TextEdit::singleline(&mut prompt.text).hint_text(hint).desired_width(f32::INFINITY));
                if prompt.focus {
                    response.request_focus();
                    prompt.focus = false;
                }
                if response.lost_focus() && ui.input(|i| i.key_pressed(Key::Enter)) {
                    submit = true;
                }
                if !suggestions.is_empty() {
                    egui::ScrollArea::vertical().max_height(160.0).show(ui, |ui| {
                        for name in &suggestions {
                            if ui.selectable_label(prompt.text == *name, name).clicked() {
                                prompt.text = name.clone();
                            }
                        }
                    });
                }
                if let Some(error) = &prompt.error {
                    ui.colored_label(ui.visuals().error_fg_color, error);
                }
                ui.horizontal(|ui| {
                    if ui.button(tr(button)).clicked() {
                        submit = true;
                    }
                    if ui.button(tr("Cancel")).clicked() || ui.input(|i| i.key_pressed(Key::Escape)) {
                        cancel = true;
                    }
                });
            });
        if cancel {
            self.prompt = None;
            return;
        }
        if !submit {
            return;
        }
        let text = prompt.text.trim().to_string();
        if text.is_empty() {
            prompt.error = Some(tr("Type a path").to_string());
            return;
        }
        let path = PathBuf::from(&text);
        let command = match prompt.kind {
            PromptKind::Open => Command::OpenFile(path),
            PromptKind::SaveAs => Command::SaveTo(path),
            PromptKind::ExportGlb => Command::ExportGlb(path),
            PromptKind::ExportObj => Command::ExportObj(path),
            PromptKind::ExportTexture => Command::ExportTexture(path),
            PromptKind::ImportTexture => Command::ImportTexture(path),
            PromptKind::ImportReference => Command::ImportReference(path),
            PromptKind::ResizeTexture => {
                let parsed = text
                    .split(['x', 'X', '×', ' '])
                    .filter(|s| !s.is_empty())
                    .map(|s| s.parse::<u32>())
                    .collect::<Result<Vec<_>, _>>();
                match parsed.as_deref() {
                    Ok([w, h]) if *w > 0 && *h > 0 && *w <= uv::MAX_TEXTURE_SIZE && *h <= uv::MAX_TEXTURE_SIZE => {
                        Command::ResizeTexture(*w, *h)
                    }
                    _ => {
                        prompt.error = Some(trf!("Type the size as WIDTHxHEIGHT, up to {max}x{max}", max = uv::MAX_TEXTURE_SIZE));
                        return;
                    }
                }
            }
        };
        self.prompt = None;
        self.run(command);
    }

    /// Файл восстановления и перечитывание текстур, изменённых на диске.
    fn background_work(&mut self) {
        let now = self.time;
        let autosave = self.autosave_seconds;
        let mut reloaded = Vec::new();
        for tab in &mut self.tabs {
            if autosave > 0.0 && tab.session.is_dirty() && now - tab.last_autosave > autosave && !tab.session.in_progress() {
                tab.last_autosave = now;
                let file = format::recovery_path(tab.path.as_deref(), &tab.session.doc.name);
                if let Err(e) = format::save(&tab.session.doc, &file) {
                    log::warn!("cannot write the recovery file: {e}");
                }
            }
            if now - tab.last_texture_check > 1.0 {
                tab.last_texture_check = now;
                let sources = tab.texture_sources();
                tab.texture_times.resize(sources.len(), None);
                for (i, source) in sources.iter().enumerate() {
                    let Some(path) = source else { continue };
                    let time = mtime(path);
                    if time.is_some() && time != tab.texture_times[i] {
                        let first = tab.texture_times[i].is_none();
                        tab.texture_times[i] = time;
                        if first {
                            continue;
                        }
                        match std::fs::read(path).map_err(|e| e.to_string()).and_then(|b| {
                            crate::image::Image::from_png(&b).map_err(|e| e.to_string())
                        }) {
                            Ok(image) => {
                                tab.session.edit("Reload Texture", |doc| ops::replace_texture_image(doc, i as u32, image));
                                reloaded.push(path.display().to_string());
                            }
                            Err(e) => log::warn!("cannot reload `{}`: {e}", path.display()),
                        }
                    }
                }
            }
        }
        for path in reloaded {
            self.message(trf!("Reloaded {path}", path = path));
        }
    }
}

/// Подпись «Add …» для формы: целиком, чтобы переводчик мог склонять название.
fn add_label(shape_name: &str) -> &'static str {
    match shape_name {
        "Cuboid" => n_("Add Cuboid"),
        "Plane" => n_("Add Plane"),
        "Cylinder" => n_("Add Cylinder"),
        "Cone" => n_("Add Cone"),
        _ => n_("Add Sphere"),
    }
}

/// Формы для кнопок «добавить».
const ADD_SHAPES: [(Shape, &str); 5] = [
    (Shape::Cuboid, n_("Cuboid")),
    (Shape::Plane, n_("Plane")),
    (Shape::Cylinder { segments: 8 }, n_("Cylinder")),
    (Shape::Cone { segments: 8 }, n_("Cone")),
    (Shape::Sphere { segments: 8, rings: 6 }, n_("Sphere")),
];

fn shape_icon(shape: &Shape) -> Icon {
    match shape {
        Shape::Cuboid => Icon::Cuboid,
        Shape::Plane => Icon::Plane,
        Shape::Cylinder { .. } => Icon::Cylinder,
        Shape::Cone { .. } => Icon::Cone,
        Shape::Sphere { .. } => Icon::Sphere,
    }
}

/// Число без лишних нулей: `8`, `0.5`, `22.5`.
pub(crate) fn trim(v: f32) -> String {
    let s = format!("{:.3}", v);
    let s = s.trim_end_matches('0').trim_end_matches('.');
    if s == "-0" { "0".to_string() } else { s.to_string() }
}

fn list_files(dir: &Path, extensions: &[&str]) -> Vec<String> {
    let mut out: Vec<String> = std::fs::read_dir(dir)
        .into_iter()
        .flatten()
        .filter_map(|e| e.ok())
        .map(|e| e.path())
        .filter(|p| {
            p.extension()
                .and_then(|e| e.to_str())
                .is_some_and(|e| extensions.iter().any(|x| x.eq_ignore_ascii_case(e)))
        })
        .filter_map(|p| p.file_name().map(|n| n.to_string_lossy().to_string()))
        .collect();
    out.sort();
    out.truncate(200);
    out
}
