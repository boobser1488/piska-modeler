//! Экспорт в `.glb` (glTF 2.0, всё в одном файле) и `.obj` + `.mtl` + PNG.
//!
//! glTF: узел на каждую группу (перенос и поворот вокруг её точки — иерархия годится для
//! анимации в других программах), меш группы — её элементы, примитив на каждую пару
//! «текстура, свечение». Координаты в метрах. Текстуры — PNG внутри файла, фильтр
//! «ближайший», альфа — вырезка (MASK). Оттенки сторон — цвета вершин `COLOR_0`.

use std::path::Path;

use glam::{Mat4, Vec3};
use serde_json::{json, Value};

use crate::doc::{euler_quat, Document, NodeId};
use crate::geometry::{build_element, BuiltMesh};
use crate::ModelError;

/// Двоичный буфер glTF с выравниванием по 4 байта.
#[derive(Default)]
struct Buffer {
    bytes: Vec<u8>,
    views: Vec<Value>,
    accessors: Vec<Value>,
}

impl Buffer {
    fn view(&mut self, data: &[u8], target: Option<u32>) -> usize {
        while !self.bytes.len().is_multiple_of(4) {
            self.bytes.push(0);
        }
        let mut view = json!({ "buffer": 0, "byteOffset": self.bytes.len(), "byteLength": data.len() });
        if let Some(target) = target {
            view["target"] = json!(target);
        }
        self.bytes.extend_from_slice(data);
        self.views.push(view);
        self.views.len() - 1
    }

    fn floats(&mut self, values: &[f32], kind: &str, count: usize, bounds: bool) -> usize {
        let data: Vec<u8> = values.iter().flat_map(|v| v.to_le_bytes()).collect();
        let view = self.view(&data, Some(34962));
        let mut accessor = json!({
            "bufferView": view, "componentType": 5126, "count": count, "type": kind,
        });
        if bounds {
            let width = values.len() / count.max(1);
            let mut min = vec![f32::INFINITY; width];
            let mut max = vec![f32::NEG_INFINITY; width];
            for chunk in values.chunks(width) {
                for i in 0..width {
                    min[i] = min[i].min(chunk[i]);
                    max[i] = max[i].max(chunk[i]);
                }
            }
            accessor["min"] = json!(min);
            accessor["max"] = json!(max);
        }
        self.accessors.push(accessor);
        self.accessors.len() - 1
    }

    fn indices(&mut self, values: &[u32]) -> usize {
        let data: Vec<u8> = values.iter().flat_map(|v| v.to_le_bytes()).collect();
        let view = self.view(&data, Some(34963));
        self.accessors.push(json!({
            "bufferView": view, "componentType": 5125, "count": values.len(), "type": "SCALAR",
        }));
        self.accessors.len() - 1
    }
}

/// Меш элементов-детей группы (или корня) — номер в `meshes`.
type MeshFor<'a> = dyn FnMut(Option<NodeId>, &str, &mut Buffer) -> Option<usize> + 'a;

/// Документ → байты `.glb`.
pub fn to_glb(doc: &Document) -> Vec<u8> {
    let scale = 1.0 / doc.units_per_metre.max(1e-6);
    let mut buffer = Buffer::default();

    // Текстуры: картинка, сэмплер «ближайший», текстура.
    let mut images = Vec::new();
    let mut textures = Vec::new();
    for (i, texture) in doc.textures.iter().enumerate() {
        let view = buffer.view(&texture.image.to_png(), None);
        images.push(json!({ "bufferView": view, "mimeType": "image/png", "name": texture.name }));
        textures.push(json!({ "source": i, "sampler": 0 }));
    }

    let mut materials: Vec<Value> = Vec::new();
    let mut material_keys: Vec<(u32, bool)> = Vec::new();
    let mut material = |texture: u32, emissive: bool| -> usize {
        if let Some(i) = material_keys.iter().position(|k| *k == (texture, emissive)) {
            return i;
        }
        let name = doc.textures.get(texture as usize).map_or("material", |t| t.name.as_str());
        let mut m = json!({
            "name": if emissive { format!("{name}_emissive") } else { name.to_string() },
            "pbrMetallicRoughness": {
                "baseColorTexture": { "index": texture },
                "metallicFactor": 0.0,
                "roughnessFactor": 1.0,
            },
            "alphaMode": "MASK",
            "alphaCutoff": 0.5,
        });
        if emissive {
            m["emissiveTexture"] = json!({ "index": texture });
            m["emissiveFactor"] = json!([1.0, 1.0, 1.0]);
        }
        materials.push(m);
        material_keys.push((texture, emissive));
        materials.len() - 1
    };

    let mut nodes: Vec<Value> = Vec::new();
    let mut meshes: Vec<Value> = Vec::new();

    // Меш из элементов-детей `parent` в пространстве этого родителя.
    let mut mesh_for = |parent: Option<NodeId>, name: &str, buffer: &mut Buffer| -> Option<usize> {
        let mut built = BuiltMesh::default();
        for node in doc.children(parent) {
            if let Some(element) = node.element() {
                let matrix = Mat4::from_scale(Vec3::splat(scale)) * element.local_matrix();
                build_element(doc, node.id, element, matrix, &mut built);
            }
        }
        if built.triangles.is_empty() {
            return None;
        }
        let tinted = built.vertices.iter().any(|v| v.color != [255, 255, 255, 255]);
        let mut keys: Vec<(u32, bool)> = built.triangles.iter().map(|t| (t.texture, t.emissive)).collect();
        keys.sort();
        keys.dedup();
        let mut primitives = Vec::new();
        for key in keys {
            // Свои вершины у каждого примитива: индексы — от нуля.
            let mut remap = vec![u32::MAX; built.vertices.len()];
            let mut used = Vec::new();
            let mut indices = Vec::new();
            for t in built.triangles.iter().filter(|t| (t.texture, t.emissive) == key) {
                for i in t.indices {
                    if remap[i as usize] == u32::MAX {
                        remap[i as usize] = used.len() as u32;
                        used.push(i as usize);
                    }
                    indices.push(remap[i as usize]);
                }
            }
            let count = used.len();
            let positions: Vec<f32> = used.iter().flat_map(|i| built.vertices[*i].position.to_array()).collect();
            let normals: Vec<f32> = used.iter().flat_map(|i| built.vertices[*i].normal.to_array()).collect();
            let uvs: Vec<f32> = used.iter().flat_map(|i| built.vertices[*i].uv.to_array()).collect();
            let mut attributes = json!({
                "POSITION": buffer.floats(&positions, "VEC3", count, true),
                "NORMAL": buffer.floats(&normals, "VEC3", count, false),
                "TEXCOORD_0": buffer.floats(&uvs, "VEC2", count, false),
            });
            if tinted {
                let colors: Vec<f32> = used
                    .iter()
                    .flat_map(|i| built.vertices[*i].color.map(|c| srgb_to_linear(c as f32 / 255.0)))
                    .collect();
                attributes["COLOR_0"] = json!(buffer.floats(&colors, "VEC4", count, false));
            }
            primitives.push(json!({
                "attributes": attributes,
                "indices": buffer.indices(&indices),
                "material": material(key.0.min(doc.textures.len().saturating_sub(1) as u32), key.1),
            }));
        }
        meshes.push(json!({ "name": name, "primitives": primitives }));
        Some(meshes.len() - 1)
    };

    // Узлы групп — рекурсивно; узел создаётся до детей, дети дописываются потом.
    fn group_nodes(
        doc: &Document,
        parent: Option<NodeId>,
        scale: f32,
        nodes: &mut Vec<Value>,
        buffer: &mut Buffer,
        mesh_for: &mut MeshFor,
    ) -> Vec<usize> {
        let mut out = Vec::new();
        for node in doc.children(parent) {
            let Some(group) = node.group() else { continue };
            let rotation = euler_quat(group.rotation);
            // G = T(o)·R·T(−o) = T(o − R·o)·R
            let translation = (group.origin - rotation * group.origin) * scale;
            let index = nodes.len();
            nodes.push(json!({
                "name": node.name,
                "translation": translation.to_array(),
                "rotation": rotation.to_array(),
            }));
            if let Some(mesh) = mesh_for(Some(node.id), &node.name, buffer) {
                nodes[index]["mesh"] = json!(mesh);
            }
            let children = group_nodes(doc, Some(node.id), scale, nodes, buffer, mesh_for);
            if !children.is_empty() {
                nodes[index]["children"] = json!(children);
            }
            out.push(index);
        }
        out
    }

    nodes.push(json!({ "name": doc.name }));
    if let Some(mesh) = mesh_for(None, &doc.name, &mut buffer) {
        nodes[0]["mesh"] = json!(mesh);
    }
    let children = group_nodes(doc, None, scale, &mut nodes, &mut buffer, &mut mesh_for);
    if !children.is_empty() {
        nodes[0]["children"] = json!(children);
    }

    let mut gltf = json!({
        "asset": { "version": "2.0", "generator": "Piska Modeler" },
        "scene": 0,
        "scenes": [{ "name": doc.name, "nodes": [0] }],
        "nodes": nodes,
        "buffers": [{ "byteLength": buffer.bytes.len() }],
        "bufferViews": buffer.views,
        "accessors": buffer.accessors,
    });
    if !meshes.is_empty() {
        gltf["meshes"] = json!(meshes);
    }
    if !materials.is_empty() {
        gltf["materials"] = json!(materials);
        gltf["images"] = json!(images);
        gltf["textures"] = json!(textures);
        // 9728 — NEAREST: пиксели остаются резкими.
        gltf["samplers"] = json!([{ "magFilter": 9728, "minFilter": 9728 }]);
    }

    let mut json_bytes = serde_json::to_vec(&gltf).expect("JSON serialization cannot fail");
    while !json_bytes.len().is_multiple_of(4) {
        json_bytes.push(b' ');
    }
    let mut bin = buffer.bytes;
    while !bin.len().is_multiple_of(4) {
        bin.push(0);
    }
    let total = 12 + 8 + json_bytes.len() + 8 + bin.len();
    let mut out = Vec::with_capacity(total);
    out.extend_from_slice(b"glTF");
    out.extend_from_slice(&2u32.to_le_bytes());
    out.extend_from_slice(&(total as u32).to_le_bytes());
    out.extend_from_slice(&(json_bytes.len() as u32).to_le_bytes());
    out.extend_from_slice(b"JSON");
    out.extend_from_slice(&json_bytes);
    out.extend_from_slice(&(bin.len() as u32).to_le_bytes());
    out.extend_from_slice(b"BIN\0");
    out.extend_from_slice(&bin);
    out
}

fn srgb_to_linear(c: f32) -> f32 {
    if c <= 0.04045 {
        c / 12.92
    } else {
        ((c + 0.055) / 1.055).powf(2.4)
    }
}

/// Записать `.glb`.
pub fn write_glb(doc: &Document, path: impl AsRef<Path>) -> Result<(), ModelError> {
    let path = path.as_ref();
    std::fs::write(path, to_glb(doc)).map_err(|source| ModelError::Io {
        path: path.display().to_string(),
        source,
    })
}

/// Результат экспорта в OBJ: текст `.obj`, текст `.mtl` и картинки (имя файла, PNG).
#[derive(Clone, Debug)]
pub struct ObjExport {
    pub obj: String,
    pub mtl: String,
    pub images: Vec<(String, Vec<u8>)>,
}

/// Документ → OBJ + MTL. `stem` — имя файлов без расширения (`crate` → `crate.mtl`,
/// `crate_atlas.png`).
pub fn to_obj(doc: &Document, stem: &str) -> ObjExport {
    use std::fmt::Write;
    let scale = 1.0 / doc.units_per_metre.max(1e-6);
    let material_name = |texture: u32, emissive: bool| {
        let name = doc.textures.get(texture as usize).map_or("material".to_string(), |t| sanitize(&t.name));
        if emissive { format!("{name}_emissive") } else { name }
    };
    let mut obj = format!("# Piska Modeler — {}\nmtllib {stem}.mtl\n", doc.name);
    let mut used_materials: Vec<(u32, bool)> = Vec::new();
    let mut base = 1u32;
    for node in &doc.nodes {
        let Some(element) = node.element() else { continue };
        let mut built = BuiltMesh::default();
        let matrix = Mat4::from_scale(Vec3::splat(scale)) * doc.world_matrix(node.id);
        build_element(doc, node.id, element, matrix, &mut built);
        if built.triangles.is_empty() {
            continue;
        }
        let _ = writeln!(obj, "o {}", sanitize(&node.name));
        for v in &built.vertices {
            let _ = writeln!(obj, "v {} {} {}", v.position.x, v.position.y, v.position.z);
        }
        for v in &built.vertices {
            let _ = writeln!(obj, "vt {} {}", v.uv.x, 1.0 - v.uv.y);
        }
        for v in &built.vertices {
            let _ = writeln!(obj, "vn {} {} {}", v.normal.x, v.normal.y, v.normal.z);
        }
        let mut current = None;
        for t in &built.triangles {
            let key = (t.texture, t.emissive);
            if current != Some(key) {
                let _ = writeln!(obj, "usemtl {}", material_name(key.0, key.1));
                current = Some(key);
                if !used_materials.contains(&key) {
                    used_materials.push(key);
                }
            }
            let [a, b, c] = t.indices.map(|i| i + base);
            let _ = writeln!(obj, "f {a}/{a}/{a} {b}/{b}/{b} {c}/{c}/{c}");
        }
        base += built.vertices.len() as u32;
    }
    let mut mtl = format!("# Piska Modeler — {}\n", doc.name);
    let mut images = Vec::new();
    used_materials.sort();
    for (texture, emissive) in used_materials {
        let Some(t) = doc.textures.get(texture as usize) else { continue };
        let file = format!("{stem}_{}.png", sanitize(&t.name));
        let _ = writeln!(mtl, "\nnewmtl {}\nKd 1 1 1\nKs 0 0 0\nd 1\nillum 1\nmap_Kd {file}", material_name(texture, emissive));
        if emissive {
            let _ = writeln!(mtl, "Ke 1 1 1\nmap_Ke {file}");
        }
        if !images.iter().any(|(f, _): &(String, Vec<u8>)| *f == file) {
            images.push((file, t.image.to_png()));
        }
    }
    ObjExport { obj, mtl, images }
}

/// Записать `<path>.obj`, `.mtl` и картинки рядом.
pub fn write_obj(doc: &Document, path: impl AsRef<Path>) -> Result<(), ModelError> {
    let path = path.as_ref();
    let stem = path.file_stem().and_then(|s| s.to_str()).unwrap_or("model").to_string();
    let dir = path.parent().unwrap_or(Path::new("."));
    let export = to_obj(doc, &stem);
    let write = |file: &Path, bytes: &[u8]| {
        std::fs::write(file, bytes).map_err(|source| ModelError::Io {
            path: file.display().to_string(),
            source,
        })
    };
    write(path, export.obj.as_bytes())?;
    write(&dir.join(format!("{stem}.mtl")), export.mtl.as_bytes())?;
    for (name, png) in &export.images {
        write(&dir.join(name), png)?;
    }
    Ok(())
}

/// Имя без пробелов и знаков, которые ломают OBJ / MTL.
fn sanitize(name: &str) -> String {
    let s: String = name
        .chars()
        .map(|c| if c.is_alphanumeric() || c == '_' || c == '-' || c == '.' { c } else { '_' })
        .collect();
    if s.is_empty() { "unnamed".to_string() } else { s }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::doc::Element;
    use crate::ops;

    fn sample() -> Document {
        let mut doc = Document::new("figure");
        let body = ops::add_group(&mut doc, "body", Vec3::new(0.0, 8.0, 0.0), None);
        ops::add_element(&mut doc, "torso", Element::cuboid(Vec3::new(-4.0, 8.0, -2.0), Vec3::new(8.0, 12.0, 4.0)), Some(body));
        let arm = ops::add_group(&mut doc, "arm", Vec3::new(6.0, 20.0, 0.0), Some(body));
        ops::add_element(&mut doc, "hand", Element::cuboid(Vec3::new(4.0, 8.0, -2.0), Vec3::new(4.0, 12.0, 4.0)), Some(arm));
        doc.node_mut(arm).unwrap().group_mut().unwrap().rotation = Vec3::new(-30.0, 0.0, 10.0);
        ops::add_element(&mut doc, "base", Element::cuboid(Vec3::new(-8.0, 0.0, -8.0), Vec3::new(16.0, 2.0, 16.0)), None);
        doc
    }

    #[test]
    fn glb_is_a_valid_gltf_with_group_nodes() {
        let doc = sample();
        let bytes = to_glb(&doc);
        assert_eq!(&bytes[0..4], b"glTF");
        assert_eq!(u32::from_le_bytes(bytes[8..12].try_into().unwrap()) as usize, bytes.len());
        let gltf = gltf::Gltf::from_slice(&bytes).expect("the gltf crate accepts the file");
        let names: Vec<&str> = gltf.document.nodes().filter_map(|n| n.name()).collect();
        assert_eq!(names, ["figure", "body", "arm"]);
        let arm = gltf.document.nodes().find(|n| n.name() == Some("arm")).unwrap();
        assert!(arm.mesh().is_some());
        let sampler = gltf.document.samplers().next().unwrap();
        assert_eq!(sampler.mag_filter(), Some(gltf::texture::MagFilter::Nearest));
    }

    #[test]
    fn obj_lists_every_element_and_its_material() {
        let doc = sample();
        let export = to_obj(&doc, "figure");
        assert_eq!(export.obj.matches("\no ").count(), 3);
        assert_eq!(export.obj.matches("\nf ").count(), 36);
        assert!(export.obj.contains("mtllib figure.mtl"));
        assert!(export.mtl.contains("map_Kd figure_atlas.png"));
        assert_eq!(export.images.len(), 1);
    }
}
