//! Проект Blockbench `.bbmodel` (JSON): чтение форматов 3.x–5.x и запись (4.x).
//!
//! Что берётся из файла:
//!
//! * `meta.format_version`, `meta.model_format` (`java_block` — координаты 0…16, модель
//!   сдвигается к центру), `meta.box_uv` (3.x: для всех кубов);
//! * `resolution` и у текстур `uv_width` / `uv_height` — размер пространства UV;
//! * `elements`: кубы (`from`, `to`, `origin`, `rotation`, `inflate`, `rescale`, стороны
//!   с `uv`, `texture` — номер, UUID или `null` = стороны нет, `rotation`; `box_uv` +
//!   `uv_offset` + `mirror_uv`; `visibility`; `export: false` — пропуск) и меши
//!   (`vertices`, `faces` с UV по вершинам);
//! * `outliner`: группы с `origin` / `rotation` и детьми (UUID элементов или вложенные
//!   группы; в 5.x поля групп — в отдельном списке `groups`);
//! * `textures`: встроенные `source` (PNG в `data:`-URI) или внешние `relative_path` /
//!   `path` (читается только файл рядом с моделью — абсолютные пути не открываются);
//!   анимированные текстуры (кадры столбиком) — кадр 0.
//!
//! Пропускаются (и перечисляются в отчёте): `animations`, `display`, локаторы, нулевые
//! объекты и прочие типы элементов.

use std::collections::{HashMap, HashSet};

use serde_json::{json, Map, Value};

use crate::image::{decode_data_uri, decode_png, png_data_uri, RgbaImage};
use crate::json::{self, boolean, number, numbers, short, string};
use crate::math::{cross, dot, normalize, sub, V3};
use crate::model::{box_uv_layout, box_uv_size, stem, BoxUv, Cube, CubeFace, Group, MeshElement, MeshFace, Model, Node, Report, Side, Texture};
use crate::{limits, safe_relative_path, Error, Format, Resolver};

/// Версия формата, которую пишет [`write`].
pub const WRITE_VERSION: &str = "4.5";

/// Ключ элемента, в котором Piska Modeler сохраняет угол своей «коробочной» развёртки
/// (Blockbench его игнорирует; UV сторон записаны явно).
pub const PISKA_BOX_UV: &str = "piska_box_uv";

/// Прочитать `.bbmodel`. `name` — имя файла (для имени модели), `resolve` — чтение
/// внешних текстур рядом с моделью.
pub fn read(name: &str, bytes: &[u8], resolve: Resolver) -> Result<Model, Error> {
    let root = json::parse(bytes, "Blockbench model (.bbmodel)")?;
    let root = root.as_object().ok_or_else(|| Error("not a Blockbench model: the file is not a JSON object".to_string()))?;
    let meta = root
        .get("meta")
        .and_then(Value::as_object)
        .ok_or_else(|| Error("not a Blockbench model: the `meta` section is missing".to_string()))?;
    let mut report = Report::default();
    let version = match meta.get("format_version") {
        Some(Value::String(s)) => s.clone(),
        Some(Value::Number(n)) => n.to_string(),
        _ => return Err(Error("not a Blockbench model: `meta.format_version` is missing".to_string())),
    };
    let major: u32 = version.split('.').next().and_then(|m| m.trim().parse().ok()).unwrap_or(0);
    if major < 3 {
        return Err(Error(format!(
            "Blockbench format version {version} is too old; open the model in a current Blockbench and save it again"
        )));
    }
    if major > 5 {
        report.warn(format!("format version {version} is newer than this importer knows (up to 5.x); unknown parts are skipped"));
    }
    let model_format = string(meta, "model_format").unwrap_or("free").to_string();
    let global_box_uv = boolean(meta, "box_uv", false);

    let resolution = match root.get("resolution").and_then(Value::as_object) {
        Some(r) => [
            positive(r.get("width"), "`resolution.width`")?.unwrap_or(16.0),
            positive(r.get("height"), "`resolution.height`")?.unwrap_or(16.0),
        ],
        None => [16.0, 16.0],
    };

    let (textures, by_uuid) = read_textures(root, resolution, resolve, &mut report)?;

    // Элементы по UUID и по порядку файла.
    let mut elements: HashMap<String, Node> = HashMap::new();
    let mut order: Vec<String> = Vec::new();
    let mut skipped: HashSet<String> = HashSet::new();
    let list = match root.get("elements") {
        Some(Value::Array(list)) => list.as_slice(),
        None | Some(Value::Null) => &[],
        Some(_) => return Err(Error("`elements` must be a list".to_string())),
    };
    if list.len() > limits::MAX_ELEMENTS {
        return Err(Error(format!("the model has {} elements; at most {} are supported", list.len(), limits::MAX_ELEMENTS)));
    }
    for (index, value) in list.iter().enumerate() {
        let object = value.as_object().ok_or_else(|| Error(format!("element #{index} is not an object")))?;
        let uuid = string(object, "uuid").map_or_else(|| format!("#element-{index}"), str::to_string);
        let kind = string(object, "type").unwrap_or("cube");
        if !boolean(object, "export", true) {
            report.ignore("elements with export disabled");
            skipped.insert(uuid);
            continue;
        }
        let node = match kind {
            "cube" => Node::Cube(read_cube(object, index, global_box_uv, &textures, &by_uuid, &mut report)?),
            "mesh" => Node::Mesh(read_mesh(object, index, &textures, &by_uuid, &mut report)?),
            other => {
                report.ignore(format!("`{}` elements", short(other)));
                skipped.insert(uuid);
                continue;
            }
        };
        if elements.insert(uuid.clone(), node).is_some() {
            report.warn(format!("two elements share UUID `{}`; the first one is dropped", short(&uuid)));
        } else {
            order.push(uuid);
        }
    }

    // Дерево групп.
    let groups_by_uuid: HashMap<String, &Map<String, Value>> = match root.get("groups") {
        Some(Value::Array(list)) => list
            .iter()
            .filter_map(Value::as_object)
            .filter_map(|g| string(g, "uuid").map(|u| (u.to_string(), g)))
            .collect(),
        _ => HashMap::new(),
    };
    let mut outline = Outline {
        elements: &mut elements,
        skipped: &skipped,
        groups: &groups_by_uuid,
        seen_groups: HashSet::new(),
        group_count: 0,
        report: &mut report,
    };
    let mut roots = match root.get("outliner") {
        Some(Value::Array(items)) => outline.items(items, 0)?,
        None | Some(Value::Null) => Vec::new(),
        Some(_) => return Err(Error("`outliner` must be a list".to_string())),
    };
    // Элементы вне дерева — в корень, по порядку файла (так делает и Blockbench).
    for uuid in order {
        if let Some(node) = elements.remove(&uuid) {
            roots.push(node);
        }
    }

    if let Some(Value::Array(animations)) = root.get("animations") {
        if !animations.is_empty() {
            report.ignore(format!("{} animation(s)", animations.len()));
        }
    }
    if root.get("display").and_then(Value::as_object).is_some_and(|d| !d.is_empty()) {
        report.ignore("display transforms (`display`)");
    }
    if root.get("animation_controllers").and_then(Value::as_array).is_some_and(|a| !a.is_empty()) {
        report.ignore("animation controllers");
    }

    Ok(Model {
        name: string(root, "name").filter(|n| !n.is_empty()).unwrap_or_else(|| stem(name)).to_string(),
        format: Format::Bbmodel,
        textures,
        roots,
        offset: if model_format == "java_block" { [-8.0, 0.0, -8.0] } else { [0.0; 3] },
        report,
    })
}

fn positive(value: Option<&Value>, what: &str) -> Result<Option<f32>, Error> {
    match value {
        None | Some(Value::Null) => Ok(None),
        Some(v) => {
            let n = number(v, &|| what.to_string())?;
            if !(n > 0.0 && n <= 65536.0) {
                return Err(Error(format!("{what} must be between 1 and 65536 (it is {n})")));
            }
            Ok(Some(n))
        }
    }
}

fn read_textures(
    root: &Map<String, Value>,
    resolution: [f32; 2],
    resolve: Resolver,
    report: &mut Report,
) -> Result<(Vec<Texture>, HashMap<String, usize>), Error> {
    let list = match root.get("textures") {
        Some(Value::Array(list)) => list.as_slice(),
        None | Some(Value::Null) => &[],
        Some(_) => return Err(Error("`textures` must be a list".to_string())),
    };
    if list.len() > limits::MAX_TEXTURES {
        return Err(Error(format!("the model has {} textures; at most {} are supported", list.len(), limits::MAX_TEXTURES)));
    }
    let mut textures = Vec::with_capacity(list.len());
    let mut by_uuid = HashMap::new();
    for (index, value) in list.iter().enumerate() {
        let object = value.as_object().ok_or_else(|| Error(format!("texture #{index} is not an object")))?;
        let name = string(object, "name").filter(|n| !n.is_empty()).map_or_else(|| format!("texture {index}"), str::to_string);
        let what = format!("texture `{}`", short(&name));
        let uv_size = [
            positive(object.get("uv_width"), &format!("{what}: `uv_width`"))?.unwrap_or(resolution[0]),
            positive(object.get("uv_height"), &format!("{what}: `uv_height`"))?.unwrap_or(resolution[1]),
        ];
        let mut path = None;
        let image = match string(object, "source").filter(|s| s.starts_with("data:")) {
            Some(source) => {
                let bytes = decode_data_uri(source).map_err(|e| Error(format!("{what}: the embedded image is damaged: {e}")))?;
                Some(decode_png(&bytes).map_err(|e| Error(format!("{what}: {e}")))?)
            }
            None => {
                // Внешний файл: только относительно модели — `relative_path`, иначе имя файла из `path`.
                let candidates: Vec<String> = [
                    string(object, "relative_path").and_then(|p| safe_relative_path(p, true)),
                    string(object, "path").and_then(file_name).and_then(|p| safe_relative_path(p, false)),
                    Some(name.clone()).filter(|n| n.to_ascii_lowercase().ends_with(".png")).and_then(|p| safe_relative_path(&p, false)),
                ]
                .into_iter()
                .flatten()
                .collect();
                let found = candidates.iter().find_map(|p| resolve(p).map(|bytes| (p.clone(), bytes)));
                match found {
                    Some((p, bytes)) => {
                        path = Some(p.clone());
                        match decode_png(&bytes) {
                            Ok(image) => Some(image),
                            Err(e) => {
                                report.warn(format!("{what}: `{p}` is not a readable PNG ({e}); a checkerboard is used"));
                                None
                            }
                        }
                    }
                    None => {
                        path = candidates.first().cloned();
                        report.warn(format!(
                            "{what}: the image is not embedded and {} was not found next to the model; a checkerboard is used",
                            candidates.first().map_or("no usable relative path".to_string(), |p| format!("`{p}`"))
                        ));
                        None
                    }
                }
            }
        };
        let image = image.map(|image| first_frame(image, uv_size, &what, report));
        if let Some(uuid) = string(object, "uuid") {
            by_uuid.insert(uuid.to_string(), index);
        }
        textures.push(Texture { name, image, uv_size, path });
    }
    Ok((textures, by_uuid))
}

fn file_name(path: &str) -> Option<&str> {
    path.rsplit(['/', '\\']).next().filter(|n| !n.is_empty())
}

/// Анимированная текстура (кадры столбиком): кадр 0. Высота кадра — по пропорции
/// пространства UV.
fn first_frame(image: RgbaImage, uv_size: [f32; 2], what: &str, report: &mut Report) -> RgbaImage {
    let frame = (image.width as f32 * uv_size[1] / uv_size[0]).round() as u32;
    if frame >= 1 && image.height >= frame * 2 && image.height.is_multiple_of(frame) {
        report.warn(format!("{what} is animated ({} frames); frame 0 is used", image.height / frame));
        return image.crop(0, 0, image.width, frame);
    }
    image
}

/// Номер текстуры стороны: `Ok(None)` — без текстуры, `Err(())` — стороны нет (`null`).
fn face_texture(
    value: Option<&Value>,
    textures: &[Texture],
    by_uuid: &HashMap<String, usize>,
    owner: &str,
    report: &mut Report,
) -> Result<Option<usize>, ()> {
    match value {
        Some(Value::Null) => Err(()),
        None | Some(Value::Bool(_)) => Ok(None),
        Some(Value::Number(n)) => match n.as_u64().map(|n| n as usize) {
            Some(i) if i < textures.len() => Ok(Some(i)),
            _ => {
                report.warn(format!("{owner}: texture {n} does not exist; the face is white"));
                Ok(None)
            }
        },
        Some(Value::String(uuid)) => match by_uuid.get(uuid) {
            Some(i) => Ok(Some(*i)),
            None => {
                report.warn(format!("{owner}: texture `{}` does not exist; the face is white", short(uuid)));
                Ok(None)
            }
        },
        Some(_) => Ok(None),
    }
}

fn quarter_turns(value: Option<&Value>, owner: &dyn Fn() -> String, report: &mut Report) -> Result<u8, Error> {
    let degrees = match value {
        None | Some(Value::Null) => return Ok(0),
        Some(v) => number(v, &|| format!("{}: `rotation`", owner()))?,
    };
    let turns = (degrees / 90.0).round();
    if (turns * 90.0 - degrees).abs() > 0.01 {
        report.warn(format!("{}: face rotation {degrees}° is not a multiple of 90°; rounded", owner()));
    }
    Ok(turns.rem_euclid(4.0) as u8)
}

fn read_cube(
    object: &Map<String, Value>,
    index: usize,
    global_box_uv: bool,
    textures: &[Texture],
    by_uuid: &HashMap<String, usize>,
    report: &mut Report,
) -> Result<Cube, Error> {
    let name = string(object, "name").unwrap_or("cube").to_string();
    let owner = || format!("element `{}` (#{index})", short(&name));
    let from = numbers::<3>(object, "from", None, &owner)?;
    let to = numbers::<3>(object, "to", None, &owner)?;
    let origin = numbers::<3>(object, "origin", Some([0.0; 3]), &owner)?;
    let rotation = match object.get("rotation") {
        // Старые файлы (и формат блоков): `{"angle": 22.5, "axis": "y", "origin": [...]}`.
        Some(Value::Object(r)) => {
            let angle = r.get("angle").map(|a| number(a, &|| format!("{}: rotation angle", owner()))).transpose()?.unwrap_or(0.0);
            let mut rotation = [0.0; 3];
            match string(r, "axis") {
                Some("x") => rotation[0] = angle,
                Some("y") => rotation[1] = angle,
                Some("z") => rotation[2] = angle,
                _ => {}
            }
            rotation
        }
        _ => numbers::<3>(object, "rotation", Some([0.0; 3]), &owner)?,
    };
    let origin = match object.get("rotation").and_then(Value::as_object) {
        Some(r) if r.contains_key("origin") => numbers::<3>(r, "origin", Some(origin), &owner)?,
        _ => origin,
    };
    let inflate = object.get("inflate").map(|v| number(v, &|| format!("{}: `inflate`", owner()))).transpose()?.unwrap_or(0.0);
    let box_uv = match object.get("box_uv") {
        Some(Value::Bool(b)) => *b,
        _ => global_box_uv,
    };
    let uv_offset = numbers::<2>(object, "uv_offset", Some([0.0; 2]), &owner)?;
    let mirror = boolean(object, "mirror_uv", false);
    let layout = box_uv.then(|| box_uv_layout(box_uv_size(from, to), uv_offset, mirror));
    let faces_object = object.get("faces").and_then(Value::as_object);
    let mut faces = [None; 6];
    for side in Side::ALL {
        let Some(face) = faces_object.and_then(|f| f.get(side.key())).and_then(Value::as_object) else {
            // У куба с «коробочной» развёрткой стороны есть всегда.
            if let Some(layout) = layout {
                faces[side.index()] = Some(CubeFace { uv: layout[side.index()], rotation: 0, texture: None });
            }
            continue;
        };
        let face_owner = || format!("{} face `{}`", owner(), side.key());
        let Ok(texture) = face_texture(face.get("texture"), textures, by_uuid, &face_owner(), report) else {
            continue; // `texture: null` — прозрачная сторона, её нет
        };
        let uv = match layout {
            Some(layout) => layout[side.index()],
            None => numbers::<4>(face, "uv", Some([0.0; 4]), &face_owner)?,
        };
        let rotation = if layout.is_some() { 0 } else { quarter_turns(face.get("rotation"), &face_owner, report)? };
        faces[side.index()] = Some(CubeFace { uv, rotation, texture });
    }
    let piska = object.get(PISKA_BOX_UV).and_then(Value::as_array).and_then(|a| {
        let x = a.first()?.as_f64()?;
        let y = a.get(1)?.as_f64()?;
        (x.is_finite() && y.is_finite()).then_some([x as f32, y as f32])
    });
    Ok(Cube {
        uuid: string(object, "uuid").unwrap_or("").to_string(),
        from,
        to,
        origin,
        rotation,
        inflate,
        rescale: boolean(object, "rescale", false),
        faces,
        box_uv: if box_uv {
            Some(BoxUv { offset: uv_offset, mirror })
        } else {
            piska.map(|offset| BoxUv { offset, mirror: false })
        },
        visible: boolean(object, "visibility", true),
        locked: boolean(object, "locked", false),
        name,
    })
}

fn read_mesh(
    object: &Map<String, Value>,
    index: usize,
    textures: &[Texture],
    by_uuid: &HashMap<String, usize>,
    report: &mut Report,
) -> Result<MeshElement, Error> {
    let name = string(object, "name").unwrap_or("mesh").to_string();
    let owner = || format!("mesh `{}` (#{index})", short(&name));
    let origin = numbers::<3>(object, "origin", Some([0.0; 3]), &owner)?;
    let rotation = numbers::<3>(object, "rotation", Some([0.0; 3]), &owner)?;
    let vertex_map = object.get("vertices").and_then(Value::as_object).ok_or_else(|| Error(format!("{}: `vertices` is missing", owner())))?;
    if vertex_map.len() > limits::MAX_VERTICES {
        return Err(Error(format!("{}: too many vertices ({})", owner(), vertex_map.len())));
    }
    let mut keys: HashMap<&str, u32> = HashMap::with_capacity(vertex_map.len());
    let mut vertices = Vec::with_capacity(vertex_map.len());
    for (key, value) in vertex_map {
        let list = value.as_array().filter(|a| a.len() >= 3).ok_or_else(|| Error(format!("{}: vertex `{}` must be 3 numbers", owner(), short(key))))?;
        let mut p = [0.0; 3];
        for i in 0..3 {
            p[i] = number(&list[i], &|| format!("{}: vertex `{}`", owner(), short(key)))?;
        }
        keys.insert(key, vertices.len() as u32);
        vertices.push(p);
    }
    let mut faces = Vec::new();
    if let Some(face_map) = object.get("faces").and_then(Value::as_object) {
        for (key, value) in face_map {
            let Some(face) = value.as_object() else { continue };
            let face_owner = format!("{} face `{}`", owner(), short(key));
            let corner_keys: Vec<&str> = face.get("vertices").and_then(Value::as_array).map_or(Vec::new(), |list| list.iter().filter_map(Value::as_str).collect());
            if corner_keys.len() < 3 {
                continue; // ребро или точка — не грань
            }
            if corner_keys.len() > 64 {
                return Err(Error(format!("{face_owner}: a face with {} corners is not supported", corner_keys.len())));
            }
            let mut corners = Vec::with_capacity(corner_keys.len());
            let mut uvs = Vec::with_capacity(corner_keys.len());
            let uv_map = face.get("uv").and_then(Value::as_object);
            for k in &corner_keys {
                let v = *keys.get(k).ok_or_else(|| Error(format!("{face_owner}: vertex `{}` does not exist", short(k))))?;
                corners.push(v);
                let uv = uv_map.and_then(|m| m.get(*k)).and_then(Value::as_array);
                let uv = match uv {
                    Some(list) if list.len() >= 2 => [
                        number(&list[0], &|| format!("{face_owner}: UV"))?,
                        number(&list[1], &|| format!("{face_owner}: UV"))?,
                    ],
                    _ => [0.0, 0.0],
                };
                uvs.push(uv);
            }
            let Ok(texture) = face_texture(face.get("texture"), textures, by_uuid, &face_owner, report) else { continue };
            if corners.len() == 4 {
                order_quad(&mut corners, &mut uvs, &vertices);
            }
            faces.push(MeshFace { vertices: corners, uvs, texture });
        }
    }
    Ok(MeshElement { uuid: string(object, "uuid").unwrap_or("").to_string(), origin, rotation, vertices, faces, visible: boolean(object, "visibility", true), name })
}

/// Четырёхугольник меша Blockbench хранит вершины не обязательно по кругу. Выбрать
/// обход без самопересечения (наибольшая площадь) той же стороны, что и треугольник из
/// первых трёх вершин (против часовой стрелки — лицевая сторона, как в three.js).
fn order_quad(corners: &mut Vec<u32>, uvs: &mut Vec<[f32; 2]>, vertices: &[V3]) {
    let p = |i: usize| vertices[corners[i] as usize];
    let facing = cross(sub(p(1), p(0)), sub(p(2), p(0)));
    let candidates = [[0, 1, 2, 3], [0, 1, 3, 2], [0, 2, 1, 3]];
    let area = |order: &[usize; 4]| {
        let pts: Vec<V3> = order.iter().map(|&i| p(i)).collect();
        let mut n = [0.0f32; 3];
        for i in 0..4 {
            let (a, b) = (pts[i], pts[(i + 1) % 4]);
            n[0] += (a[1] - b[1]) * (a[2] + b[2]);
            n[1] += (a[2] - b[2]) * (a[0] + b[0]);
            n[2] += (a[0] - b[0]) * (a[1] + b[1]);
        }
        n
    };
    let best = candidates
        .iter()
        .max_by(|a, b| {
            let (la, lb) = (dot(area(a), area(a)), dot(area(b), area(b)));
            la.partial_cmp(&lb).unwrap_or(std::cmp::Ordering::Equal)
        })
        .copied()
        .unwrap_or([0, 1, 2, 3]);
    let mut order = best.to_vec();
    if normalize(facing).is_some() && dot(area(&best), facing) < 0.0 {
        order.reverse();
    }
    let (c, u): (Vec<u32>, Vec<[f32; 2]>) = order.iter().map(|&i| (corners[i], uvs[i])).unzip();
    *corners = c;
    *uvs = u;
}

/// Обход дерева `outliner`.
struct Outline<'a, 'b> {
    elements: &'a mut HashMap<String, Node>,
    skipped: &'a HashSet<String>,
    groups: &'a HashMap<String, &'b Map<String, Value>>,
    seen_groups: HashSet<String>,
    group_count: usize,
    report: &'a mut Report,
}

impl Outline<'_, '_> {
    fn items(&mut self, items: &[Value], depth: usize) -> Result<Vec<Node>, Error> {
        if depth > limits::MAX_DEPTH {
            return Err(Error(format!("the outliner nests groups deeper than {} levels", limits::MAX_DEPTH)));
        }
        let mut out = Vec::new();
        for item in items {
            match item {
                Value::String(uuid) => match self.elements.remove(uuid) {
                    Some(node) => out.push(node),
                    None if self.skipped.contains(uuid) => {}
                    None => self.report.warn(format!("the outliner refers to element `{}` that does not exist (or is listed twice)", short(uuid))),
                },
                Value::Object(object) => {
                    if let Some(group) = self.group(object, depth)? {
                        out.push(Node::Group(group));
                    }
                }
                _ => self.report.warn("the outliner contains an entry that is neither a group nor an element UUID"),
            }
        }
        Ok(out)
    }

    fn group(&mut self, object: &Map<String, Value>, depth: usize) -> Result<Option<Group>, Error> {
        let uuid = string(object, "uuid").unwrap_or("").to_string();
        if !uuid.is_empty() && !self.seen_groups.insert(uuid.clone()) {
            self.report.warn(format!("group `{}` is listed twice in the outliner; the copy is skipped", short(&uuid)));
            return Ok(None);
        }
        self.group_count += 1;
        if self.group_count > limits::MAX_GROUPS {
            return Err(Error(format!("the model has more than {} groups", limits::MAX_GROUPS)));
        }
        // 5.x: в дереве только UUID и дети, остальное — в списке `groups`.
        let fields = if object.contains_key("origin") || object.contains_key("name") {
            object
        } else {
            self.groups.get(&uuid).copied().unwrap_or(object)
        };
        let name = string(fields, "name").unwrap_or("group").to_string();
        let owner = || format!("group `{}`", short(&name));
        let origin = numbers::<3>(fields, "origin", Some([0.0; 3]), &owner)?;
        let rotation = numbers::<3>(fields, "rotation", Some([0.0; 3]), &owner)?;
        let children = match object.get("children") {
            Some(Value::Array(children)) => self.items(children, depth + 1)?,
            _ => Vec::new(),
        };
        if !boolean(fields, "export", true) {
            self.report.ignore("groups with export disabled (and their elements)");
            return Ok(None);
        }
        Ok(Some(Group { name, uuid, origin, rotation, visible: boolean(fields, "visibility", true), children }))
    }
}

/// Внешние текстуры модели (пути относительно модели) — для отслеживания изменений.
pub fn dependencies(bytes: &[u8]) -> Vec<String> {
    let Ok(Value::Object(root)) = json::parse(bytes, "") else { return Vec::new() };
    let Some(list) = root.get("textures").and_then(Value::as_array) else { return Vec::new() };
    list.iter()
        .filter_map(Value::as_object)
        .filter(|t| !string(t, "source").is_some_and(|s| s.starts_with("data:")))
        .filter_map(|t| {
            string(t, "relative_path")
                .and_then(|p| safe_relative_path(p, true))
                .or_else(|| string(t, "path").and_then(file_name).and_then(|p| safe_relative_path(p, false)))
        })
        .collect()
}

// ----------------------------------------------------------------------
// Запись
// ----------------------------------------------------------------------

/// Число для JSON: самая короткая запись `f32` (читается обратно точно).
fn num(x: f32) -> Value {
    let x = if x == 0.0 { 0.0 } else { x }; // без «-0»
    format!("{x}").parse::<f64>().ok().and_then(serde_json::Number::from_f64).map_or(Value::from(0), Value::Number)
}

fn nums(values: &[f32]) -> Value {
    Value::Array(values.iter().map(|v| num(*v)).collect())
}

/// UUID вида Blockbench для узла без своего: детерминированный (одинаковые байты при
/// каждом сохранении).
fn make_uuid(kind: u32, counter: u32) -> String {
    format!("{kind:08x}-0000-4000-8000-{counter:012x}")
}

/// Записать модель в `.bbmodel` (формат 4.5, `model_format: free`, текстуры встроены
/// PNG). Кубы пишутся с явными UV сторон (`box_uv: false`); угол «коробочной»
/// развёртки Piska Modeler сохраняется в [`PISKA_BOX_UV`]. Стороны без текстуры —
/// `texture: false`, отсутствующие — `texture: null`.
pub fn write(model: &Model) -> String {
    let mut elements = Vec::new();
    let mut counter = 0u32;
    let outliner = write_nodes(&model.roots, &mut elements, &mut counter);
    let resolution = model.textures.first().map_or([16.0, 16.0], |t| t.uv_size);
    let textures: Vec<Value> = model
        .textures
        .iter()
        .enumerate()
        .map(|(i, t)| {
            let image = t.image.clone().unwrap_or_else(|| crate::mesh::placeholder(t.uv_size));
            json!({
                "name": t.name,
                "id": i.to_string(),
                "uuid": make_uuid(0x7e47, i as u32),
                "width": image.width,
                "height": image.height,
                "uv_width": num(t.uv_size[0]),
                "uv_height": num(t.uv_size[1]),
                "folder": "",
                "namespace": "",
                "particle": i == 0,
                "render_mode": "default",
                "render_sides": "auto",
                "frame_time": 1,
                "visible": true,
                "internal": true,
                "saved": true,
                "mode": "bitmap",
                "source": png_data_uri(&image),
            })
        })
        .collect();
    let file = json!({
        "meta": { "format_version": WRITE_VERSION, "model_format": "free", "box_uv": false },
        "name": model.name,
        "model_identifier": "",
        "visible_box": [1, 1, 0],
        "resolution": { "width": num(resolution[0]), "height": num(resolution[1]) },
        "elements": elements,
        "outliner": outliner,
        "textures": textures,
    });
    let mut text = serde_json::to_string_pretty(&file).expect("JSON values are always serializable");
    text.push('\n');
    text
}

fn write_nodes(nodes: &[Node], elements: &mut Vec<Value>, counter: &mut u32) -> Vec<Value> {
    let mut outline = Vec::new();
    for node in nodes {
        *counter += 1;
        let fallback = |kind: u32, counter: u32| make_uuid(kind, counter);
        match node {
            Node::Group(g) => {
                let uuid = if g.uuid.is_empty() { fallback(0x6709, *counter) } else { g.uuid.clone() };
                let children = write_nodes(&g.children, elements, counter);
                outline.push(json!({
                    "name": g.name,
                    "origin": nums(&g.origin),
                    "rotation": nums(&g.rotation),
                    "color": 0,
                    "uuid": uuid,
                    "export": true,
                    "mirror_uv": false,
                    "isOpen": true,
                    "locked": false,
                    "visibility": g.visible,
                    "autouv": 0,
                    "children": children,
                }));
            }
            Node::Cube(c) => {
                let uuid = if c.uuid.is_empty() { fallback(0xc0be, *counter) } else { c.uuid.clone() };
                let mut faces = Map::new();
                for side in Side::ALL {
                    let face = match c.faces[side.index()] {
                        Some(face) => {
                            let mut f = json!({
                                "uv": nums(&face.uv),
                                "texture": face.texture.map_or(Value::Bool(false), Value::from),
                            });
                            if face.rotation % 4 != 0 {
                                f["rotation"] = Value::from((face.rotation % 4) as u32 * 90);
                            }
                            f
                        }
                        None => json!({ "uv": [0, 0, 0, 0], "texture": null }),
                    };
                    faces.insert(side.key().to_string(), face);
                }
                let mut element = json!({
                    "name": c.name,
                    "box_uv": false,
                    "rescale": c.rescale,
                    "locked": c.locked,
                    "render_order": "default",
                    "allow_mirror_modeling": true,
                    "from": nums(&c.from),
                    "to": nums(&c.to),
                    "autouv": 0,
                    "color": 0,
                    "origin": nums(&c.origin),
                    "uv_offset": nums(&c.box_uv.map_or([0.0, 0.0], |b| b.offset)),
                    "faces": faces,
                    "type": "cube",
                    "uuid": uuid.clone(),
                    "visibility": c.visible,
                    "export": true,
                });
                if c.rotation != [0.0; 3] {
                    element["rotation"] = nums(&c.rotation);
                }
                if c.inflate != 0.0 {
                    element["inflate"] = num(c.inflate);
                }
                if let Some(b) = c.box_uv {
                    element[PISKA_BOX_UV] = nums(&b.offset);
                }
                elements.push(element);
                outline.push(Value::String(uuid));
            }
            Node::Mesh(m) => {
                let uuid = if m.uuid.is_empty() { fallback(0x3e54, *counter) } else { m.uuid.clone() };
                let mut vertices = Map::new();
                for (i, v) in m.vertices.iter().enumerate() {
                    vertices.insert(format!("v{i:04}"), nums(v));
                }
                let mut faces = Map::new();
                for (i, f) in m.faces.iter().enumerate() {
                    let mut uv = Map::new();
                    for (corner, v) in f.vertices.iter().enumerate() {
                        uv.insert(format!("v{v:04}"), nums(&f.uvs.get(corner).copied().unwrap_or([0.0, 0.0])));
                    }
                    faces.insert(
                        format!("f{i}"),
                        json!({
                            "uv": uv,
                            "vertices": f.vertices.iter().map(|v| format!("v{v:04}")).collect::<Vec<_>>(),
                            "texture": f.texture.map_or(Value::Bool(false), Value::from),
                        }),
                    );
                }
                elements.push(json!({
                    "name": m.name,
                    "color": 0,
                    "origin": nums(&m.origin),
                    "rotation": nums(&m.rotation),
                    "export": true,
                    "visibility": m.visible,
                    "locked": false,
                    "render_order": "default",
                    "allow_mirror_modeling": true,
                    "vertices": vertices,
                    "faces": faces,
                    "type": "mesh",
                    "uuid": uuid.clone(),
                }));
                outline.push(Value::String(uuid));
            }
        }
    }
    outline
}

#[cfg(test)]
mod tests {
    use super::*;

    fn none(_: &str) -> Option<Vec<u8>> {
        None
    }

    #[test]
    fn hostile_files_give_precise_errors() {
        let cases: &[(&str, &str)] = &[
            ("[]", "not a JSON object"),
            ("{}", "`meta` section is missing"),
            (r#"{"meta":{"format_version":"2.0"}}"#, "too old"),
            (r#"{"meta":{"format_version":"4.5"},"elements":[{"from":[0,0],"to":[1,1,1]}]}"#, "`from` must be a list of 3 numbers"),
            (r#"{"meta":{"format_version":"4.5"},"elements":[{"from":[0,0,1e40],"to":[1,1,1]}]}"#, "out of range"),
            (r#"{"meta":{"format_version":"4.5"},"resolution":{"width":-3,"height":16}}"#, "between 1 and 65536"),
            (r#"{"meta":{"format_version":"4.5"},"textures":[{"name":"a","source":"data:image/png;base64,@@@"}]}"#, "embedded image is damaged"),
            (r#"{"meta":{"format_version":"4.5"},"elements":"x"}"#, "`elements` must be a list"),
            ("{\"meta\": {", "not a valid Blockbench model"),
        ];
        for (text, expected) in cases {
            let err = read("x.bbmodel", text.as_bytes(), &none).unwrap_err().0;
            assert!(err.contains(expected), "{text}: {err}");
        }
        // Глубокая вложенность групп: ошибка, а не переполнение стека.
        let mut outliner = String::from("\"leaf\"");
        for _ in 0..200 {
            outliner = format!("{{\"name\":\"g\",\"origin\":[0,0,0],\"children\":[{outliner}]}}");
        }
        let text = format!("{{\"meta\":{{\"format_version\":\"4.5\"}},\"outliner\":[{outliner}]}}");
        assert!(read("x.bbmodel", text.as_bytes(), &none).is_err());
    }

    #[test]
    fn absolute_and_escaping_texture_paths_are_never_requested() {
        let text = r#"{"meta":{"format_version":"4.5"},"textures":[
            {"name":"a","path":"C:\\secret\\a.png","relative_path":"../../../../etc/a.png"},
            {"name":"b","relative_path":"C:/x/b.png"},
            {"name":"/etc/passwd"}]}"#;
        let asked = std::cell::RefCell::new(Vec::new());
        let resolve = |p: &str| {
            asked.borrow_mut().push(p.to_string());
            None
        };
        let model = read("m.bbmodel", text.as_bytes(), &resolve).unwrap();
        assert_eq!(model.textures.len(), 3);
        for p in asked.borrow().iter() {
            assert!(!p.contains(':') && !p.starts_with('/'), "{p}");
        }
        assert!(asked.borrow().contains(&"a.png".to_string()), "only the file name next to the model: {:?}", asked.borrow());
        assert!(model.report.warnings.iter().any(|w| w.contains("checkerboard")));
    }
}
