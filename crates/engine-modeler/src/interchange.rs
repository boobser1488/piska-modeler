//! Чужие файлы ↔ документ Piska Modeler.
//!
//! * `.bbmodel`, модель Minecraft `.json`, `.vox` открываются редактируемо: кубы Blockbench
//!   становятся кубоидами (UV сторон, поворот, раздувание, «коробочная» развёртка),
//!   группы — группами, текстуры — текстурами документа, меши Blockbench и воксельные
//!   коробки — элементами;
//! * `.obj`, `.glb`, `.gltf`, `.ply`, `.stl` становятся элементами-мешами
//!   ([`Shape::Mesh`]): выделяются, двигаются, рисуются (UV и текстура из файла; без UV
//!   — «Generate UVs» с цветами материалов / вершин), правятся по вершинам и граням;
//! * документ записывается в `.bbmodel` ([`to_bbmodel`]).
//!
//! Чтение форматов — общий с движком крейт `engine-model-formats` (`vendor/`).

use std::collections::HashMap;
use std::path::Path;

use glam::{Mat4, Vec3};

use engine_model_formats as formats;

use crate::doc::{Document, Element, Face, FaceDir, Group, MeshFace, Node, NodeId, NodeKind, Shape, Texture, UvRect};
use crate::image::{Image, Rgba};
use crate::{mesh_edit, ops, ModelError};

/// Расширения, которые Modeler открывает (File ▸ Open) и импортирует (File ▸ Import Mesh).
pub const OPEN_EXTENSIONS: &[&str] = &["bbmodel", "json", "vox", "obj", "glb", "gltf", "ply", "stl"];

/// Расширения мешей — они становятся элементами-мешами.
pub const MESH_EXTENSIONS: &[&str] = &["obj", "glb", "gltf", "ply", "stl"];

/// Результат открытия: документ и что стоит сказать пользователю.
#[derive(Clone, Debug)]
pub struct Opened {
    pub doc: Document,
    /// Одна строка для строки состояния: «Imported crate.obj as an editable mesh».
    pub summary: String,
    /// Потери и пропуски (анимации, оттенки, не найденные текстуры…).
    pub notes: Vec<String>,
}

fn extension(name: &str) -> String {
    Path::new(name).extension().and_then(|e| e.to_str()).unwrap_or("").to_ascii_lowercase()
}

fn stem(name: &str) -> String {
    let file = Path::new(name).file_name().and_then(|f| f.to_str()).unwrap_or(name);
    let file = file.strip_suffix(".json").map(|f| f.strip_suffix(".model").unwrap_or(f)).unwrap_or(file);
    Path::new(file).file_stem().and_then(|s| s.to_str()).unwrap_or("model").to_string()
}

/// Открывает ли Modeler файл правкой (не `.pmodel`).
pub fn can_open(name: &str) -> bool {
    OPEN_EXTENSIONS.contains(&extension(name).as_str())
}

fn format_error(name: &str, e: impl std::fmt::Display) -> ModelError {
    ModelError::Format(format!("{name}: {e}"))
}

/// Открыть чужой файл новым документом. `resolve` читает соседние файлы (текстуры,
/// `.mtl`, буферы glTF) по относительному пути.
pub fn open_bytes(name: &str, bytes: &[u8], resolve: &dyn Fn(&str) -> Option<Vec<u8>>) -> Result<Opened, ModelError> {
    let ext = extension(name);
    let file = Path::new(name).file_name().and_then(|f| f.to_str()).unwrap_or(name).to_string();
    if let Some(message) = formats::unsupported_message(name) {
        return Err(ModelError::Format(message));
    }
    if MESH_EXTENSIONS.contains(&ext.as_str()) {
        let mut doc = Document::new(&stem(name));
        let (ids, notes) = import_mesh_into(&mut doc, name, bytes, resolve, true)?;
        let summary = format!("Imported {file} as an editable mesh ({} part{}); Save writes a new .pmodel", ids.len(), if ids.len() == 1 { "" } else { "s" });
        return Ok(Opened { doc, summary, notes });
    }
    let model = formats::read_model(name, bytes, resolve).map_err(|e| format_error(&file, e))?;
    let (doc, notes) = model_to_document(&model);
    let cubes = doc.elements().filter(|(_, e)| e.shape == Shape::Cuboid).count();
    let meshes = doc.elements().filter(|(_, e)| e.shape == Shape::Mesh).count();
    let summary = format!("Imported {file}: {cubes} cube(s), {meshes} mesh(es), editable; Save writes a new .pmodel");
    Ok(Opened { doc, summary, notes })
}

/// Открыть чужой файл с диска.
pub fn open_file(path: &Path) -> Result<Opened, ModelError> {
    let bytes = std::fs::read(path).map_err(|source| ModelError::Io { path: path.display().to_string(), source })?;
    let dir = path.parent().map(Path::to_path_buf).unwrap_or_default();
    let resolve = move |p: &str| formats::safe_relative_path(p, true).and_then(|p| std::fs::read(dir.join(p)).ok());
    open_bytes(&path.to_string_lossy(), &bytes, &resolve)
}

// ----------------------------------------------------------------------
// Сцена (кубы, группы) → документ
// ----------------------------------------------------------------------

fn image_from(rgba: &formats::RgbaImage) -> Image {
    let pixels = rgba.pixels.chunks_exact(4).map(|p| [p[0], p[1], p[2], p[3]]).collect();
    Image::from_pixels(rgba.width.max(1), rgba.height.max(1), pixels).unwrap_or_else(|_| Image::new(1, 1, [255, 255, 255, 255]))
}

fn rgba_from(image: &Image) -> formats::RgbaImage {
    formats::RgbaImage { width: image.width(), height: image.height(), pixels: image.pixels().iter().flatten().copied().collect() }
}

/// Сцена из общего крейта → документ (кубоиды, группы, меши, текстуры) и заметки.
pub fn model_to_document(model: &formats::Model) -> (Document, Vec<String>) {
    let mut doc = Document::new(&model.name);
    let mut notes: Vec<String> = model.report.warnings.clone();
    if !model.report.ignored.is_empty() {
        notes.push(format!("not imported: {}", model.report.ignored.join(", ")));
    }
    if !model.textures.is_empty() {
        doc.textures = model
            .textures
            .iter()
            .map(|t| Texture {
                name: t.name.trim_end_matches(".png").to_string(),
                image: match &t.image {
                    Some(image) => image_from(image),
                    None => Image::new(t.uv_size[0].max(1.0) as u32, t.uv_size[1].max(1.0) as u32, [255, 0, 220, 255]),
                },
                source: None,
            })
            .collect();
        let first = &model.textures[0];
        doc.texel_density = doc.textures[0].image.width() as f32 / first.uv_size[0].max(1e-3);
    }
    let scales: Vec<[f32; 2]> = model
        .textures
        .iter()
        .zip(&doc.textures)
        .map(|(t, d)| [d.image.width() as f32 / t.uv_size[0].max(1e-3), d.image.height() as f32 / t.uv_size[1].max(1e-3)])
        .collect();
    let offset = Vec3::from_array(model.offset);
    // Стороны без текстуры — белые (как в движке): своя белая текстура 2×2.
    fn untextured(nodes: &[formats::Node]) -> bool {
        nodes.iter().any(|n| match n {
            formats::Node::Group(g) => untextured(&g.children),
            formats::Node::Cube(c) => c.faces.iter().flatten().any(|f| f.texture.is_none()),
            formats::Node::Mesh(m) => m.faces.iter().any(|f| f.texture.is_none()),
        })
    }
    let white = untextured(&model.roots).then(|| {
        doc.textures.push(Texture { name: "white".to_string(), image: Image::new(2, 2, [255, 255, 255, 255]), source: None });
        doc.textures.len() as u32 - 1
    });
    let mut ctx = Build { doc: &mut doc, scales, offset, notes: &mut notes, white };
    for node in &model.roots {
        ctx.node(node, None);
    }
    doc.next_id = doc.nodes.iter().map(|n| n.id.0).max().unwrap_or(0) + 1;
    (doc, notes)
}

struct Build<'a> {
    doc: &'a mut Document,
    scales: Vec<[f32; 2]>,
    offset: Vec3,
    notes: &'a mut Vec<String>,
    white: Option<u32>,
}

impl Build<'_> {
    fn push(&mut self, name: &str, parent: Option<NodeId>, visible: bool, kind: NodeKind) -> NodeId {
        let id = self.doc.allocate_id();
        let name = self.doc.unique_name(name);
        // Запертые в Blockbench узлы приходят незапертыми: здесь их надо править.
        self.doc.nodes.push(Node { id, name, parent, visible, locked: false, kind });
        id
    }

    fn node(&mut self, node: &formats::Node, parent: Option<NodeId>) {
        match node {
            formats::Node::Group(g) => {
                let group = Group { origin: Vec3::from_array(g.origin) + self.offset, rotation: Vec3::from_array(g.rotation) };
                let id = self.push(&g.name, parent, g.visible, NodeKind::Group(group));
                for child in &g.children {
                    self.node(child, Some(id));
                }
            }
            formats::Node::Cube(c) => {
                let element = self.cube(c);
                self.push(&c.name, parent, c.visible, NodeKind::Element(element));
            }
            formats::Node::Mesh(m) => {
                let origin = Vec3::from_array(m.origin) + self.offset;
                let points: Vec<Vec3> = m.vertices.iter().map(|v| origin + Vec3::from_array(*v)).collect();
                let faces = m
                    .faces
                    .iter()
                    .map(|f| match (f.texture, self.white) {
                        (None, Some(white)) => MeshFace { vertices: f.vertices.clone(), uvs: vec![[1.0, 1.0]; f.vertices.len()], texture: white },
                        _ => {
                            let texture = f.texture.unwrap_or(0);
                            let s = self.scales.get(texture).copied().unwrap_or([1.0, 1.0]);
                            MeshFace { vertices: f.vertices.clone(), uvs: f.uvs.iter().map(|uv| [uv[0] * s[0], uv[1] * s[1]]).collect(), texture: texture as u32 }
                        }
                    })
                    .collect();
                let mut element = Element::mesh(&points, faces);
                element.pivot = origin;
                element.rotation = Vec3::from_array(m.rotation);
                self.push(&m.name, parent, m.visible, NodeKind::Element(element));
            }
        }
    }

    fn cube(&mut self, c: &formats::Cube) -> Element {
        let mut from = Vec3::from_array(c.from).min(Vec3::from_array(c.to)) + self.offset;
        let mut to = Vec3::from_array(c.from).max(Vec3::from_array(c.to)) + self.offset;
        let origin = Vec3::from_array(c.origin) + self.offset;
        if c.rescale {
            // Растяжение Minecraft: поперёк оси поворота — в 1 / cos(угла) от точки вращения.
            let axes: Vec<usize> = (0..3).filter(|i| c.rotation[*i] != 0.0).collect();
            if let [axis] = axes[..] {
                let k = 1.0 / c.rotation[axis].to_radians().cos().abs().max(1e-3);
                for i in (0..3).filter(|i| *i != axis) {
                    from[i] = origin[i] + (from[i] - origin[i]) * k;
                    to[i] = origin[i] + (to[i] - origin[i]) * k;
                }
                self.notes.push(format!("`{}`: Minecraft `rescale` is applied to the box size", c.name));
            }
        }
        let mut element = Element::cuboid(from, to - from);
        element.pivot = origin;
        element.rotation = Vec3::from_array(c.rotation);
        element.inflate = c.inflate;
        element.box_uv = false;
        for dir in FaceDir::ALL {
            let face = &mut element.faces[dir.index()];
            match c.faces[dir.index()] {
                None => face.enabled = false,
                Some(f) if f.texture.is_none() && self.white.is_some() => {
                    *face = Face { enabled: true, texture: self.white.unwrap_or(0), uv: UvRect::new(0.0, 0.0, 2.0, 2.0), ..Face::default() };
                }
                Some(f) => {
                    let texture = f.texture.unwrap_or(0);
                    let s = self.scales.get(texture).copied().unwrap_or([1.0, 1.0]);
                    let [u1, v1, u2, v2] = f.uv;
                    let mut rect = UvRect::new(u1.min(u2) * s[0], v1.min(v2) * s[1], (u2 - u1).abs() * s[0], (v2 - v1).abs() * s[1]);
                    rect.flip_u = u2 < u1;
                    rect.flip_v = v2 < v1;
                    // Верх и низ у Modeler повёрнуты на 180° относительно Blockbench / Minecraft.
                    let turn = if matches!(dir, FaceDir::Up | FaceDir::Down) { 2 } else { 0 };
                    rect.rotation = (f.rotation + turn) % 4;
                    *face = Face { enabled: true, texture: texture as u32, uv: rect, ..Face::default() };
                }
            }
        }
        if let Some(b) = c.box_uv {
            element.box_uv = true;
            element.uv_origin = [b.offset[0].max(0.0) as u32, b.offset[1].max(0.0) as u32];
        }
        element
    }
}

// ----------------------------------------------------------------------
// Документ → .bbmodel
// ----------------------------------------------------------------------

/// Документ → сцена общего крейта (для записи `.bbmodel`) и заметки о потерях.
pub fn document_to_model(doc: &Document) -> (formats::Model, Vec<String>) {
    let mut notes = Vec::new();
    let textures = doc
        .textures
        .iter()
        .map(|t| formats::Texture {
            name: t.name.clone(),
            image: Some(rgba_from(&t.image)),
            uv_size: [t.image.width() as f32, t.image.height() as f32],
            path: None,
        })
        .collect();
    fn children(doc: &Document, parent: Option<NodeId>, notes: &mut Vec<String>) -> Vec<formats::Node> {
        let mut out = Vec::new();
        for node in doc.children(parent) {
            let uuid = format!("00000000-0000-4000-8000-{:012x}", node.id.0);
            match &node.kind {
                NodeKind::Group(g) => out.push(formats::Node::Group(formats::Group {
                    name: node.name.clone(),
                    uuid,
                    origin: g.origin.to_array(),
                    rotation: g.rotation.to_array(),
                    visible: node.visible,
                    children: children(doc, Some(node.id), notes),
                })),
                NodeKind::Element(e) => out.push(element_node(doc, node, e, uuid, notes)),
            }
        }
        out
    }
    let roots = children(doc, None, &mut notes);
    let model = formats::Model {
        name: doc.name.clone(),
        format: formats::Format::Bbmodel,
        textures,
        roots,
        offset: [0.0; 3],
        report: Default::default(),
    };
    notes.sort();
    notes.dedup();
    (model, notes)
}

fn element_node(doc: &Document, node: &Node, e: &Element, uuid: String, notes: &mut Vec<String>) -> formats::Node {
    if e.faces.iter().any(|f| f.enabled && (f.tint != [255; 4] || f.emissive)) {
        notes.push("face tints and emission are not stored in .bbmodel".to_string());
    }
    if e.twin.is_some() {
        notes.push("mirror twins become independent elements".to_string());
    }
    match e.shape {
        Shape::Cuboid | Shape::Plane => {
            let flat = e.shape == Shape::Plane;
            let mut faces = [None; 6];
            for dir in FaceDir::ALL {
                let f = e.face(dir);
                if !f.enabled || (flat && !matches!(dir, FaceDir::Up | FaceDir::Down)) {
                    continue;
                }
                let r = f.uv;
                let (mut u1, mut u2, mut v1, mut v2) = (r.x, r.x + r.w, r.y, r.y + r.h);
                if r.flip_u {
                    std::mem::swap(&mut u1, &mut u2);
                }
                if r.flip_v {
                    std::mem::swap(&mut v1, &mut v2);
                }
                let turn = if matches!(dir, FaceDir::Up | FaceDir::Down) { 2 } else { 0 };
                faces[dir.index()] = Some(formats::CubeFace { uv: [u1, v1, u2, v2], rotation: (r.rotation + turn) % 4, texture: Some(f.texture as usize) });
            }
            let mut to = e.to();
            if flat {
                to.y = e.from.y;
            }
            formats::Node::Cube(formats::Cube {
                name: node.name.clone(),
                uuid,
                from: e.from.to_array(),
                to: to.to_array(),
                origin: e.pivot.to_array(),
                rotation: e.rotation.to_array(),
                inflate: e.inflate,
                rescale: false,
                faces,
                box_uv: e.box_uv.then_some(formats::BoxUv { offset: [e.uv_origin[0] as f32, e.uv_origin[1] as f32], mirror: false }),
                visible: node.visible,
                locked: node.locked,
            })
        }
        _ => {
            // Меш и круглые формы — свободный меш Blockbench (вершины от точки вращения).
            let (points, faces) = match &e.mesh {
                Some(shape) => (e.mesh_points(), shape.faces.clone()),
                None => {
                    notes.push(format!("{} shapes are written as Blockbench meshes", e.shape.name()));
                    let mut built = crate::geometry::BuiltMesh::default();
                    crate::geometry::build_element(doc, node.id, e, Mat4::IDENTITY, &mut built);
                    let points: Vec<Vec3> = built.vertices.iter().map(|v| v.position).collect();
                    let faces = built
                        .triangles
                        .iter()
                        .map(|t| {
                            let size = doc.textures.get(t.texture as usize).map_or([1.0, 1.0], |x| [x.image.width() as f32, x.image.height() as f32]);
                            MeshFace {
                                vertices: t.indices.to_vec(),
                                uvs: t.indices.iter().map(|i| { let uv = built.vertices[*i as usize].uv; [uv.x * size[0], uv.y * size[1]] }).collect(),
                                texture: t.texture,
                            }
                        })
                        .collect();
                    (points, faces)
                }
            };
            let pivot = e.pivot;
            formats::Node::Mesh(formats::MeshElement {
                name: node.name.clone(),
                uuid,
                origin: pivot.to_array(),
                rotation: e.rotation.to_array(),
                vertices: points.iter().map(|p| (*p - pivot).to_array()).collect(),
                faces: faces
                    .into_iter()
                    .map(|f| formats::MeshFace { vertices: f.vertices, uvs: f.uvs, texture: Some(f.texture as usize) })
                    .collect(),
                visible: node.visible,
            })
        }
    }
}

/// Документ → текст `.bbmodel` и заметки о том, что формат не хранит.
pub fn to_bbmodel(doc: &Document) -> (String, Vec<String>) {
    let (model, notes) = document_to_model(doc);
    (formats::bbmodel::write(&model), notes)
}

// ----------------------------------------------------------------------
// Меши: .obj, .glb / .gltf, .ply, .stl
// ----------------------------------------------------------------------

/// Меш из файла: вершины в метрах, части с гранями.
#[derive(Default)]
struct Source {
    positions: Vec<Vec3>,
    parts: Vec<SourcePart>,
    /// Материалы: картинка (если есть) и цвет.
    materials: Vec<(Option<Image>, Rgba)>,
}

#[derive(Default)]
struct SourcePart {
    name: String,
    /// (вершины против часовой стрелки, UV углов 0…1 или `None`, материал)
    faces: Vec<(Vec<u32>, Option<Vec<[f32; 2]>>, Option<usize>, Option<Rgba>)>,
}

/// Добавить меш из файла элементами-мешами (по одному на часть файла). `blank_atlas`:
/// пустой атлас нового документа заменяется текстурой файла. Возвращает новые узлы и заметки.
pub fn import_mesh_into(
    doc: &mut Document,
    name: &str,
    bytes: &[u8],
    resolve: &dyn Fn(&str) -> Option<Vec<u8>>,
    blank_atlas: bool,
) -> Result<(Vec<NodeId>, Vec<String>), ModelError> {
    let file = Path::new(name).file_name().and_then(|f| f.to_str()).unwrap_or(name).to_string();
    let mut notes = Vec::new();
    let source = match extension(name).as_str() {
        "obj" => obj_source(&String::from_utf8_lossy(bytes), resolve, &mut notes)?,
        "glb" | "gltf" => gltf_source(bytes, resolve, &mut notes).map_err(|e| format_error(&file, e))?,
        "ply" => poly_source(formats::poly::read_ply(bytes, resolve).map_err(|e| format_error(&file, e))?, &mut notes),
        "stl" => poly_source(formats::poly::read_stl(bytes).map_err(|e| format_error(&file, e))?, &mut notes),
        other => return Err(ModelError::Format(formats::unsupported_message(name).unwrap_or_else(|| format!("`.{other}` is not a mesh format; use .obj, .glb, .gltf, .ply or .stl")))),
    };
    if source.parts.iter().all(|p| p.faces.is_empty()) {
        return Err(ModelError::Format(format!("`{file}` has no faces")));
    }
    let scale = doc.units_per_metre;
    // Текстуры материалов → текстуры документа.
    let blank = blank_atlas && doc.nodes.is_empty() && doc.textures.len() == 1 && doc.textures[0].image.pixels().iter().all(|p| p[3] == 0);
    let mut texture_of: HashMap<usize, u32> = HashMap::new();
    for (index, (image, _)) in source.materials.iter().enumerate() {
        let Some(image) = image else { continue };
        let texture = Texture { name: format!("{} {}", stem(name), index), image: image.clone(), source: None };
        if blank && texture_of.is_empty() {
            doc.textures[0] = texture;
            texture_of.insert(index, 0);
        } else {
            doc.textures.push(texture);
            texture_of.insert(index, doc.textures.len() as u32 - 1);
        }
    }
    let mut ids = Vec::new();
    for part in &source.parts {
        if part.faces.is_empty() {
            continue;
        }
        let mut remap: HashMap<u32, u32> = HashMap::new();
        let mut points = Vec::new();
        let mut faces = Vec::new();
        let mut colors = Vec::new();
        let mut textured = true;
        for (vertices, uvs, material, color) in &part.faces {
            let corners: Vec<u32> = vertices
                .iter()
                .map(|v| {
                    *remap.entry(*v).or_insert_with(|| {
                        points.push(source.positions[*v as usize] * scale);
                        points.len() as u32 - 1
                    })
                })
                .collect();
            let texture = material.and_then(|m| texture_of.get(&m)).copied();
            let face_uvs = match (uvs, texture) {
                (Some(uvs), Some(t)) => {
                    let [w, h] = doc.textures[t as usize].image.size();
                    uvs.iter().map(|uv| [uv[0] * w as f32, uv[1] * h as f32]).collect()
                }
                _ => {
                    textured = false;
                    vec![[0.0, 0.0]; corners.len()]
                }
            };
            colors.push(color.or_else(|| material.and_then(|m| source.materials.get(m)).map(|m| m.1)).unwrap_or(ops::BASE_COLOR));
            faces.push(MeshFace { vertices: corners, uvs: face_uvs, texture: texture.unwrap_or(0) });
        }
        let label = if part.name.is_empty() { stem(name) } else { part.name.clone() };
        let id = mesh_edit::add_mesh(doc, &label, &points, faces, None);
        // Общие углы соседних треугольников (.stl, .glb) — одна вершина для правки.
        mesh_edit::merge_by_distance(doc, id, 1e-4);
        if !textured {
            if source.materials.iter().any(|m| m.0.is_some()) {
                notes.push(format!("`{label}`: some faces have no UVs or texture; all its faces got generated UVs with their colours"));
            }
            mesh_edit::generate_uvs(doc, id, Some(&colors)).map_err(ModelError::Format)?;
        }
        ids.push(id);
    }
    Ok((ids, notes))
}

fn obj_source(text: &str, resolve: &dyn Fn(&str) -> Option<Vec<u8>>, notes: &mut Vec<String>) -> Result<Source, ModelError> {
    let mut source = Source::default();
    let mut uvs: Vec<[f32; 2]> = Vec::new();
    let mut material_names: Vec<String> = Vec::new();
    let mut current: Option<usize> = None;
    let mut libraries = Vec::new();
    source.parts.push(SourcePart::default());
    for (number, line) in text.lines().enumerate() {
        let mut parts = line.split_whitespace();
        let fail = |what: &str| ModelError::Format(format!("line {}: {what}", number + 1));
        match parts.next() {
            Some("v") => {
                let v: Vec<f32> = parts.take(3).filter_map(|p| p.parse().ok()).collect();
                if v.len() < 3 || v.iter().any(|x| !x.is_finite()) {
                    return Err(fail("a vertex needs 3 numbers"));
                }
                source.positions.push(Vec3::new(v[0], v[1], v[2]));
            }
            Some("vt") => {
                let v: Vec<f32> = parts.take(2).filter_map(|p| p.parse().ok()).collect();
                if v.len() < 2 {
                    return Err(fail("a texture coordinate needs 2 numbers"));
                }
                uvs.push([v[0], 1.0 - v[1]]); // (0, 0) у OBJ — левый нижний угол
            }
            Some("o" | "g") => {
                let name = parts.collect::<Vec<_>>().join(" ");
                if source.parts.last().is_some_and(|p| !p.faces.is_empty()) {
                    source.parts.push(SourcePart { name, faces: Vec::new() });
                } else if let Some(last) = source.parts.last_mut() {
                    last.name = name;
                }
            }
            Some("mtllib") => libraries.extend(parts.map(str::to_string)),
            Some("usemtl") => {
                let name = parts.next().unwrap_or("").to_string();
                current = Some(material_names.iter().position(|m| *m == name).unwrap_or_else(|| {
                    material_names.push(name);
                    material_names.len() - 1
                }));
            }
            Some("f") => {
                let mut vertices = Vec::new();
                let mut corner_uvs = Vec::new();
                for corner in parts {
                    let mut fields = corner.split('/');
                    let resolve_index = |text: Option<&str>, len: usize| -> Option<usize> {
                        let i: i64 = text?.parse().ok()?;
                        let r = if i < 0 { len as i64 + i } else { i - 1 };
                        (r >= 0 && (r as usize) < len).then_some(r as usize)
                    };
                    let v = resolve_index(fields.next(), source.positions.len()).ok_or_else(|| fail(&format!("bad vertex `{corner}`")))?;
                    vertices.push(v as u32);
                    corner_uvs.push(resolve_index(fields.next().filter(|t| !t.is_empty()), uvs.len()).map(|i| uvs[i]));
                }
                if vertices.len() < 3 {
                    return Err(fail("a face needs at least 3 vertices"));
                }
                let face_uvs = corner_uvs.iter().all(Option::is_some).then(|| corner_uvs.iter().flatten().copied().collect());
                if let Some(part) = source.parts.last_mut() {
                    part.faces.push((vertices, face_uvs, current, None));
                }
            }
            _ => {}
        }
    }
    // Материалы .mtl: цвет Kd и картинка map_Kd.
    let mut found: HashMap<String, (Option<Image>, Rgba)> = HashMap::new();
    for library in libraries {
        let Some(bytes) = formats::safe_relative_path(&library, true).and_then(|p| resolve(&p)) else {
            notes.push(format!("material library `{library}` was not found next to the model"));
            continue;
        };
        let mut name = String::new();
        for line in String::from_utf8_lossy(&bytes).lines() {
            let words: Vec<&str> = line.split_whitespace().collect();
            match words.as_slice() {
                ["newmtl", n, ..] => {
                    name = n.to_string();
                    found.entry(name.clone()).or_insert((None, [204, 204, 204, 255]));
                }
                ["Kd", r, g, b, ..] => {
                    let c = |t: &str| (t.parse::<f32>().unwrap_or(0.8).clamp(0.0, 1.0).powf(1.0 / 2.2) * 255.0).round() as u8;
                    if let Some(m) = found.get_mut(&name) {
                        m.1 = [c(r), c(g), c(b), 255];
                    }
                }
                ["map_Kd", .., file] => {
                    let image = formats::safe_relative_path(file, true).and_then(|p| resolve(&p)).and_then(|b| Image::from_png(&b).ok());
                    if image.is_none() {
                        notes.push(format!("texture `{file}` was not found or is not an image"));
                    }
                    if let Some(m) = found.get_mut(&name) {
                        m.0 = image;
                    }
                }
                _ => {}
            }
        }
    }
    source.materials = material_names.iter().map(|n| found.get(n).cloned().unwrap_or((None, [204, 204, 204, 255]))).collect();
    Ok(source)
}

fn gltf_source(bytes: &[u8], resolve: &dyn Fn(&str) -> Option<Vec<u8>>, notes: &mut Vec<String>) -> Result<Source, String> {
    let gltf::Gltf { document, blob } = gltf::Gltf::from_slice(bytes).map_err(|e| format!("cannot read glTF: {e}"))?;
    let mut blob = blob;
    let mut buffers = Vec::new();
    for buffer in document.buffers() {
        let data = match buffer.source() {
            gltf::buffer::Source::Bin => blob.take().ok_or("the .glb has no binary chunk")?,
            gltf::buffer::Source::Uri(uri) if uri.starts_with("data:") => formats::decode_data_uri(uri).map_err(|e| e.0)?,
            gltf::buffer::Source::Uri(uri) => formats::safe_relative_path(uri, true)
                .and_then(|p| resolve(&p))
                .ok_or_else(|| format!("buffer `{uri}` was not found next to the model"))?,
        };
        if data.len() < buffer.length() {
            return Err(format!("buffer #{} is shorter than declared", buffer.index()));
        }
        buffers.push(data);
    }
    let mut source = Source::default();
    // Материалы: картинка базового цвета и множитель.
    for material in document.materials() {
        let pbr = material.pbr_metallic_roughness();
        let f = pbr.base_color_factor();
        let color = [f[0], f[1], f[2], f[3]].map(|c| (c.clamp(0.0, 1.0).powf(1.0 / 2.2) * 255.0).round() as u8);
        let image = pbr.base_color_texture().and_then(|info| {
            let bytes = match info.texture().source().source() {
                gltf::image::Source::View { view, .. } => {
                    let data = buffers.get(view.buffer().index())?;
                    data.get(view.offset()..view.offset() + view.length())?.to_vec()
                }
                gltf::image::Source::Uri { uri, .. } if uri.starts_with("data:") => formats::decode_data_uri(uri).ok()?,
                gltf::image::Source::Uri { uri, .. } => formats::safe_relative_path(uri, true).and_then(|p| resolve(&p))?,
            };
            let image = Image::from_png(&bytes).ok();
            if image.is_none() {
                notes.push("a glTF texture could not be decoded".to_string());
            }
            image
        });
        source.materials.push((image, [color[0], color[1], color[2], 255]));
    }
    fn visit(node: gltf::Node, parent: Mat4, buffers: &[Vec<u8>], source: &mut Source) {
        let matrix = parent * Mat4::from_cols_array_2d(&node.transform().matrix());
        if let Some(mesh) = node.mesh() {
            let mut part = SourcePart { name: node.name().or(mesh.name()).unwrap_or("").to_string(), faces: Vec::new() };
            for primitive in mesh.primitives().filter(|p| p.mode() == gltf::mesh::Mode::Triangles) {
                let reader = primitive.reader(|b| buffers.get(b.index()).map(|d| &d[..]));
                let Some(read) = reader.read_positions() else { continue };
                let base = source.positions.len() as u32;
                source.positions.extend(read.map(|p| matrix.transform_point3(Vec3::from_array(p))));
                let count = source.positions.len() as u32 - base;
                let uvs: Option<Vec<[f32; 2]>> = reader.read_tex_coords(0).map(|t| t.into_f32().collect());
                let indices: Vec<u32> = match reader.read_indices() {
                    Some(i) => i.into_u32().collect(),
                    None => (0..count).collect(),
                };
                let mirrored = matrix.determinant() < 0.0;
                for t in indices.chunks_exact(3) {
                    if t.iter().any(|i| *i >= count) {
                        continue;
                    }
                    let mut corners = [t[0], t[1], t[2]];
                    if mirrored {
                        corners.swap(1, 2);
                    }
                    let face_uvs = uvs.as_ref().filter(|u| u.len() == count as usize).map(|u| corners.iter().map(|i| u[*i as usize]).collect());
                    part.faces.push((corners.iter().map(|i| base + i).collect(), face_uvs, primitive.material().index(), None));
                }
            }
            source.parts.push(part);
        }
        for child in node.children() {
            visit(child, matrix, buffers, source);
        }
    }
    let scenes: Vec<gltf::Scene> = match document.default_scene() {
        Some(scene) => vec![scene],
        None => document.scenes().collect(),
    };
    for scene in scenes {
        for node in scene.nodes() {
            visit(node, Mat4::IDENTITY, &buffers, &mut source);
        }
    }
    Ok(source)
}

fn poly_source(mesh: formats::poly::PolyMesh, notes: &mut Vec<String>) -> Source {
    notes.extend(mesh.report.warnings.iter().cloned());
    // Файлы .ply / .stl — с осью Z вверх (Blender, САПР): (x, y, z) → (x, z, −y).
    let mut source = Source { positions: mesh.positions.iter().map(|p| Vec3::new(p[0], p[2], -p[1])).collect(), ..Default::default() };
    if let Some(texture) = &mesh.texture {
        source.materials.push((Some(image_from(texture)), [255; 4]));
    }
    let mut part = SourcePart::default();
    for (i, t) in mesh.triangles.iter().enumerate() {
        let color = match (&mesh.face_colors, &mesh.vertex_colors) {
            (Some(c), _) => Some(c[i]),
            (None, Some(c)) => {
                // Цвет грани — средний цвет её вершин.
                let sum = t.iter().fold([0u32; 4], |s, v| {
                    let c = c[*v as usize];
                    [s[0] + c[0] as u32, s[1] + c[1] as u32, s[2] + c[2] as u32, s[3] + c[3] as u32]
                });
                Some(sum.map(|x| (x / 3) as u8))
            }
            _ => None,
        };
        let uvs = mesh.uvs.as_ref().filter(|_| mesh.texture.is_some()).map(|u| t.iter().map(|v| u[*v as usize]).collect());
        let material = mesh.texture.as_ref().map(|_| 0);
        part.faces.push((t.to_vec(), uvs, material, color));
    }
    source.parts.push(part);
    source
}

/// Объединение команд для открытия: перед сохранением чужой файл не перезаписывается —
/// у документа нет пути, Save спросит `.pmodel`.
pub fn is_mesh_file(name: &str) -> bool {
    MESH_EXTENSIONS.contains(&extension(name).as_str())
}
