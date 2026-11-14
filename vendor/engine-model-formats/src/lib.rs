//! Модели сторонних форматов для движка Piska и Piska Modeler.
//!
//! * `.bbmodel` — проект Blockbench (JSON, форматы 3.x–5.x): кубы, группы, повороты,
//!   UV по сторонам и «коробкой», встроенные текстуры, свободные меши. Чтение
//!   ([`bbmodel::read`]) и запись ([`bbmodel::write`]).
//! * Модель блока / предмета Minecraft (`.json` с `elements`) — [`java::read`].
//! * MagicaVoxel `.vox` — [`vox::read`], жадная сетка или кубоиды.
//! * `.ply` (ASCII и двоичный) и `.stl` (ASCII и двоичный) — [`poly`].
//!
//! Два уровня результата: редактируемая сцена [`Model`] (группы, кубы, меши — её берёт
//! Piska Modeler) и треугольный меш с атласом [`Imported`] (его берёт движок,
//! [`import`]). Соглашения (проверены тестами на несимметричных моделях):
//!
//! * правая тройка, Y вверх, «север» — −Z, как в Blockbench и Minecraft;
//! * поворот элемента и группы — углы Эйлера в градусах, сначала X, потом Y, потом Z
//!   (порядок `ZYX` в three.js: `R = Rz·Ry·Rx`) вокруг `origin`;
//! * UV: (0, 0) — левый верхний угол картинки; сторона куба видна снаружи «стоя»
//!   (у верха и низа «север» сверху и снизу соответственно, как в Minecraft);
//! * обход треугольников — против часовой стрелки снаружи;
//! * единица Blockbench / Minecraft / вокселя — 1/16 метра (16 = 1 блок = 1 м),
//!   у `.ply` / `.stl` — метр; множитель — [`Options::scale`].
//!
//! Устойчивость: испорченные и враждебные файлы дают ошибку с объяснением, а не панику;
//! все счётчики из файла проверяются по [`limits`] до выделения памяти; внешние файлы
//! (текстуры) читаются только через переданную функцию и только по относительным путям
//! без выхода наверх за корень ([`safe_relative_path`]).
#![forbid(unsafe_code)]

pub mod bbmodel;
mod base64;
mod image;
pub mod java;
mod json;
mod math;
mod mesh;
mod model;
pub mod poly;
pub mod vox;

pub use image::{decode_data_uri, decode_png, encode_png, png_data_uri, RgbaImage};
pub use model::{box_uv_layout, box_uv_size};
pub use math::Affine;
pub use mesh::{model_to_mesh, Imported, Options, Part, TriMesh, UpAxis};
pub use model::{BoxUv, Cube, CubeFace, Group, MeshElement, MeshFace, Model, Node, Report, Side, Texture};

use std::fmt;

/// Ошибка чтения: сообщение говорит, что не так и что сделать.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Error(pub String);

impl Error {
    pub fn new(message: impl Into<String>) -> Self {
        Self(message.into())
    }
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl std::error::Error for Error {}

/// Чтение соседнего файла по пути относительно файла модели (уже проверенному
/// [`safe_relative_path`]); `None` — файла нет.
pub type Resolver<'a> = &'a dyn Fn(&str) -> Option<Vec<u8>>;

/// Пределы: всё, что задаёт файл, проверяется по ним до выделения памяти.
pub mod limits {
    /// Наибольший размер файла модели.
    pub const MAX_FILE_BYTES: usize = 256 << 20;
    /// Элементов (кубов и мешей) в модели.
    pub const MAX_ELEMENTS: usize = 100_000;
    /// Групп в модели.
    pub const MAX_GROUPS: usize = 100_000;
    /// Вложенность групп.
    pub const MAX_DEPTH: usize = 64;
    /// Текстур в модели.
    pub const MAX_TEXTURES: usize = 256;
    /// Сторона одной текстуры, пикселей.
    pub const MAX_TEXTURE_SIDE: u32 = 8192;
    /// Пикселей в общем атласе.
    pub const MAX_ATLAS_PIXELS: u64 = 16 << 20;
    /// Вершин в итоговом меше.
    pub const MAX_VERTICES: usize = 8_000_000;
    /// Треугольников в итоговом меше.
    pub const MAX_TRIANGLES: usize = 8_000_000;
    /// Наибольшая координата по модулю (в единицах файла).
    pub const MAX_COORDINATE: f32 = 1.0e6;
    /// Ячеек в плотной сетке вокселей (все модели `.vox` вместе).
    pub const MAX_VOXEL_CELLS: u64 = 32 << 20;
    /// Кубоидов при открытии `.vox` в редакторе моделей.
    pub const MAX_VOXEL_BOXES: usize = 20_000;
}

/// Поддерживаемый сторонний формат.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Format {
    /// Проект Blockbench `.bbmodel`.
    Bbmodel,
    /// Модель блока / предмета Minecraft Java (`.json`).
    JavaModel,
    /// MagicaVoxel `.vox`.
    Vox,
    /// Stanford `.ply`.
    Ply,
    /// `.stl`.
    Stl,
}

/// Расширения файлов, которые движок импортирует как меши этим крейтом (кроме
/// `.json`: модель Minecraft — по двойному расширению `.model.json` или явно).
pub const EXTENSIONS: &[&str] = &["bbmodel", "vox", "ply", "stl"];

/// Двойное расширение модели Minecraft, которую движок импортирует сам.
pub const JAVA_MODEL_SUFFIX: &str = ".model.json";

impl Format {
    /// Формат по имени файла. Обычный `.json` — `None`: это может быть файл данных
    /// (модель Minecraft распознаёт [`Format::detect`] по содержимому).
    pub fn from_file_name(name: &str) -> Option<Format> {
        let lower = name.to_ascii_lowercase();
        if lower.ends_with(JAVA_MODEL_SUFFIX) {
            return Some(Format::JavaModel);
        }
        match lower.rsplit_once('.').map(|(_, e)| e)? {
            "bbmodel" => Some(Format::Bbmodel),
            "vox" => Some(Format::Vox),
            "ply" => Some(Format::Ply),
            "stl" => Some(Format::Stl),
            _ => None,
        }
    }

    /// Формат по имени, а для `.json` — по содержимому (`elements` с `from` / `to`
    /// или `parent` с `textures`).
    pub fn detect(name: &str, bytes: &[u8]) -> Option<Format> {
        Format::from_file_name(name).or_else(|| {
            let is_json = name.to_ascii_lowercase().ends_with(".json");
            (is_json && java::looks_like_java_model(bytes)).then_some(Format::JavaModel)
        })
    }

    pub fn name(self) -> &'static str {
        match self {
            Format::Bbmodel => "Blockbench model",
            Format::JavaModel => "Minecraft block/item model",
            Format::Vox => "MagicaVoxel model",
            Format::Ply => "PLY mesh",
            Format::Stl => "STL mesh",
        }
    }

    /// Ось «вверх» в файлах формата по умолчанию: `.ply` и `.stl` чаще всего пишут
    /// программы с осью Z вверх (Blender, САПР, слайсеры), остальные — Y вверх.
    pub fn default_up(self) -> UpAxis {
        match self {
            Format::Ply | Format::Stl => UpAxis::Z,
            _ => UpAxis::Y,
        }
    }

    /// Метров в единице файла (до [`Options::scale`]).
    pub fn metres_per_unit(self) -> f32 {
        match self {
            Format::Bbmodel | Format::JavaModel | Format::Vox => 1.0 / 16.0,
            Format::Ply | Format::Stl => 1.0,
        }
    }

    /// Нужен ли модели «ближайший» фильтр текстуры (пиксельные источники).
    pub fn pixel_art(self) -> bool {
        true
    }
}

/// Сообщение для формата, который движок и редактор моделей не читают (`.fbx`,
/// `.blend`, `.dae`, `.3ds`…), — вместо молчаливого пропуска файла. `None` — формат
/// не из этого списка.
pub fn unsupported_message(file_name: &str) -> Option<String> {
    let ext = file_name.rsplit_once('.').map(|(_, e)| e.to_ascii_lowercase())?;
    let what = match ext.as_str() {
        "fbx" => "FBX",
        "blend" => "Blender .blend",
        "dae" => "Collada .dae",
        "3ds" => "3ds Max .3ds",
        "max" => "3ds Max .max",
        "ma" | "mb" => "Maya",
        "c4d" => "Cinema 4D",
        "skp" => "SketchUp",
        "x" => "DirectX .x",
        "lwo" | "lws" => "LightWave",
        "usd" | "usda" | "usdc" | "usdz" => "USD",
        _ => return None,
    };
    Some(format!(
        "{what} is not supported; export the model as glTF (.glb), OBJ, Blockbench (.bbmodel), PLY or STL and import that file"
    ))
}

/// Как Piska Modeler открывает файл.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ModelerSupport {
    /// Свой документ (`.pmodel`).
    Native,
    /// Редактируемо: кубы остаются кубами, группы — группами, текстура — рисуемой
    /// (`.bbmodel`, модель Minecraft `.json`, `.vox`).
    Editable,
    /// Опорный меш для обрисовки (`.obj`, `.glb`, `.gltf`, `.ply`, `.stl`).
    Reference,
}

/// Может ли Piska Modeler открыть файл и как (для пунктов «Edit in Modeler» /
/// «Edit a Copy in Modeler» редактора движка). `.json` — только `.model.json`.
pub fn modeler_support(file_name: &str) -> Option<ModelerSupport> {
    let lower = file_name.to_ascii_lowercase();
    if lower.ends_with(JAVA_MODEL_SUFFIX) {
        return Some(ModelerSupport::Editable);
    }
    match lower.rsplit_once('.').map(|(_, e)| e)? {
        "pmodel" => Some(ModelerSupport::Native),
        "bbmodel" | "vox" => Some(ModelerSupport::Editable),
        "obj" | "glb" | "gltf" | "ply" | "stl" => Some(ModelerSupport::Reference),
        _ => None,
    }
}

/// Проверить путь внешнего файла из модели: только относительный, без букв дисков,
/// UNC и схем, с разделителем `/`; `.` убираются, `..` разрешены только внутри пути
/// (`a/../b`), выход выше каталога модели — `None`, если `allow_parent` ложно, иначе
/// ведущие `..` сохраняются (их ограничит корнем ассетов вызывающий, см.
/// `engine_assets::join_normalized`). Пустой результат — `None`.
pub fn safe_relative_path(path: &str, allow_parent: bool) -> Option<String> {
    let path = path.trim();
    if path.is_empty()
        || path.len() > 1024
        || path.contains(':')
        || path.starts_with('/')
        || path.starts_with('\\')
        || path.contains('\0')
    {
        return None;
    }
    let mut parts: Vec<&str> = Vec::new();
    let mut ups = 0usize;
    for part in path.split(['/', '\\']) {
        match part {
            "" | "." => {}
            ".." => {
                if parts.pop().is_none() {
                    if !allow_parent {
                        return None;
                    }
                    ups += 1;
                }
            }
            other => parts.push(other),
        }
    }
    if parts.is_empty() {
        return None;
    }
    let mut out = "../".repeat(ups);
    out.push_str(&parts.join("/"));
    Some(out)
}

/// Прочитать модель любого поддерживаемого формата и собрать меш с атласом для движка.
/// `name` — имя файла (по нему и по содержимому `.json` выбирается формат), `resolve` —
/// чтение соседних файлов (внешние текстуры, родитель модели Minecraft).
pub fn import(name: &str, bytes: &[u8], resolve: Resolver, options: &Options) -> Result<Imported, Error> {
    let format = Format::detect(name, bytes).ok_or_else(|| match unsupported_message(name) {
        Some(message) => Error(message),
        None => Error(format!("`{name}` is not a model format this importer reads (.bbmodel, .model.json, .vox, .ply, .stl)")),
    })?;
    check_size(bytes)?;
    match format {
        Format::Bbmodel => model_to_mesh(&bbmodel::read(name, bytes, resolve)?, options),
        Format::JavaModel => model_to_mesh(&java::read(name, bytes, resolve)?, options),
        Format::Vox => vox::to_mesh(&vox::read(bytes)?, options),
        Format::Ply => poly::to_mesh(&poly::read_ply(bytes, resolve)?, Format::Ply, options),
        Format::Stl => poly::to_mesh(&poly::read_stl(bytes)?, Format::Stl, options),
    }
}

/// Прочитать редактируемую сцену (для Piska Modeler): `.bbmodel`, модель Minecraft,
/// `.vox` (кубоиды). Для `.ply` / `.stl` — ошибка: это опорные меши ([`poly`]).
pub fn read_model(name: &str, bytes: &[u8], resolve: Resolver) -> Result<Model, Error> {
    check_size(bytes)?;
    match Format::detect(name, bytes) {
        Some(Format::Bbmodel) => bbmodel::read(name, bytes, resolve),
        Some(Format::JavaModel) => java::read(name, bytes, resolve),
        Some(Format::Vox) => vox::to_model(&vox::read(bytes)?, model::stem(name)),
        Some(format @ (Format::Ply | Format::Stl)) => {
            Err(Error(format!("a {} has no editable elements; import it as a reference mesh", format.name())))
        }
        None => Err(Error(
            unsupported_message(name).unwrap_or_else(|| format!("`{name}` is not a .bbmodel, Minecraft .json model or .vox file")),
        )),
    }
}

/// Внешние файлы, от которых зависит модель (текстуры по относительным путям) — чтобы
/// предпросмотр и импорт обновлялись, когда они меняются. Дёшево и без ошибок.
pub fn dependencies(name: &str, bytes: &[u8]) -> Vec<String> {
    match Format::detect(name, bytes) {
        Some(Format::Bbmodel) => bbmodel::dependencies(bytes),
        Some(Format::JavaModel) => java::dependencies(bytes),
        Some(Format::Ply) => poly::ply_dependencies(bytes),
        _ => Vec::new(),
    }
}

/// Есть ли у модели своя текстура (атлас), которую движок берёт вместо назначенной:
/// у `.bbmodel`, модели Minecraft и `.vox` — всегда, у `.ply` — если в нём цвета или
/// текстура, у `.stl` — нет.
pub fn has_texture(name: &str, bytes: &[u8]) -> bool {
    match Format::detect(name, bytes) {
        Some(Format::Bbmodel | Format::JavaModel | Format::Vox) => true,
        Some(Format::Ply) => poly::ply_has_texture(bytes),
        _ => false,
    }
}

fn check_size(bytes: &[u8]) -> Result<(), Error> {
    if bytes.len() > limits::MAX_FILE_BYTES {
        return Err(Error(format!(
            "the file is {} MB; models larger than {} MB are not imported",
            bytes.len() >> 20,
            limits::MAX_FILE_BYTES >> 20
        )));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn formats_by_name_and_content() {
        assert_eq!(Format::from_file_name("a/B.BBMODEL"), Some(Format::Bbmodel));
        assert_eq!(Format::from_file_name("stone.model.json"), Some(Format::JavaModel));
        assert_eq!(Format::from_file_name("data.json"), None, "data files are not hijacked");
        let model = br#"{"elements":[{"from":[0,0,0],"to":[16,16,16],"faces":{}}]}"#;
        assert_eq!(Format::detect("stone.json", model), Some(Format::JavaModel));
        assert_eq!(Format::detect("stats.json", br#"{"hp": 3, "elements": [1, 2]}"#), None);
        assert!(unsupported_message("x.FBX").unwrap().contains("FBX is not supported"));
        assert!(unsupported_message("x.obj").is_none());
        assert_eq!(modeler_support("a.bbmodel"), Some(ModelerSupport::Editable));
        assert_eq!(modeler_support("a.stl"), Some(ModelerSupport::Reference));
        assert_eq!(modeler_support("a.json"), None);
        assert_eq!(modeler_support("a.fbx"), None);
    }

    #[test]
    fn relative_paths_never_escape() {
        assert_eq!(safe_relative_path("tex/a.png", false).as_deref(), Some("tex/a.png"));
        assert_eq!(safe_relative_path(".\\tex\\.\\a.png", false).as_deref(), Some("tex/a.png"));
        assert_eq!(safe_relative_path("tex/../a.png", false).as_deref(), Some("a.png"));
        assert_eq!(safe_relative_path("../a.png", false), None);
        assert_eq!(safe_relative_path("../textures/a.png", true).as_deref(), Some("../textures/a.png"));
        for bad in ["/etc/passwd", "C:/Windows/x.png", "c:x.png", "\\\\server\\share\\a.png", "file:///a.png", "", ".", "a/.."] {
            assert_eq!(safe_relative_path(bad, true), None, "{bad}");
        }
    }
}
