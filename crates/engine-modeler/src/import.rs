//! Импорт чужой модели (`.obj`, `.glb`, `.gltf`) как опорного меша: он показывается
//! серым в окне, по нему можно обрисовать модель кубоидами, но редактировать его нельзя.
//! Размеры в файле считаются метрами (как в движке) и переводятся в единицы модели.

use std::path::Path;

use glam::{Mat4, Vec3};

use crate::doc::{Document, ReferenceMesh};
use crate::ModelError;

/// Прочитать опорный меш из байтов файла `name` (по расширению — формат).
pub fn reference_from_bytes(doc: &Document, name: &str, bytes: &[u8]) -> Result<ReferenceMesh, ModelError> {
    let extension = Path::new(name)
        .extension()
        .and_then(|e| e.to_str())
        .unwrap_or("")
        .to_ascii_lowercase();
    let (positions, indices) = match extension.as_str() {
        "obj" => obj_triangles(&String::from_utf8_lossy(bytes))?,
        "glb" | "gltf" => gltf_triangles(bytes)?,
        other => {
            return Err(ModelError::Format(format!(
                "`.{other}` cannot be imported as a reference; use .obj, .glb or .gltf"
            )))
        }
    };
    if indices.is_empty() {
        return Err(ModelError::Format(format!("`{name}` has no triangles")));
    }
    let scale = doc.units_per_metre;
    let stem = Path::new(name).file_stem().and_then(|s| s.to_str()).unwrap_or("reference");
    Ok(ReferenceMesh {
        name: stem.to_string(),
        positions: positions.iter().map(|p| (*p * scale).to_array()).collect(),
        indices,
        visible: true,
    })
}

/// Прочитать опорный меш из файла.
pub fn reference_from_file(doc: &Document, path: impl AsRef<Path>) -> Result<ReferenceMesh, ModelError> {
    let path = path.as_ref();
    let bytes = std::fs::read(path).map_err(|source| ModelError::Io {
        path: path.display().to_string(),
        source,
    })?;
    let name = path.file_name().and_then(|n| n.to_str()).unwrap_or("reference");
    reference_from_bytes(doc, name, &bytes)
}

fn obj_triangles(text: &str) -> Result<(Vec<Vec3>, Vec<u32>), ModelError> {
    let mut positions = Vec::new();
    let mut indices = Vec::new();
    for (number, line) in text.lines().enumerate() {
        let mut parts = line.split_whitespace();
        match parts.next() {
            Some("v") => {
                let v: Vec<f32> = parts.take(3).filter_map(|p| p.parse().ok()).collect();
                if v.len() < 3 {
                    return Err(ModelError::Format(format!("line {}: a vertex needs 3 numbers", number + 1)));
                }
                positions.push(Vec3::new(v[0], v[1], v[2]));
            }
            Some("f") => {
                let corners: Result<Vec<u32>, ModelError> = parts
                    .map(|corner| {
                        let index: i64 = corner
                            .split('/')
                            .next()
                            .and_then(|i| i.parse().ok())
                            .ok_or_else(|| ModelError::Format(format!("line {}: bad face `{corner}`", number + 1)))?;
                        let resolved = if index < 0 { positions.len() as i64 + index } else { index - 1 };
                        if resolved < 0 || resolved as usize >= positions.len() {
                            return Err(ModelError::Format(format!("line {}: vertex {index} does not exist", number + 1)));
                        }
                        Ok(resolved as u32)
                    })
                    .collect();
                let corners = corners?;
                for i in 1..corners.len().saturating_sub(1) {
                    indices.extend([corners[0], corners[i], corners[i + 1]]);
                }
            }
            _ => {}
        }
    }
    Ok((positions, indices))
}

fn gltf_triangles(bytes: &[u8]) -> Result<(Vec<Vec3>, Vec<u32>), ModelError> {
    let fail = |e: gltf::Error| ModelError::Format(format!("cannot read glTF: {e}"));
    let gltf::Gltf { document, blob } = gltf::Gltf::from_slice(bytes).map_err(fail)?;
    let buffers = gltf::import_buffers(&document, None, blob).map_err(fail)?;
    let mut positions = Vec::new();
    let mut indices = Vec::new();
    fn visit(
        node: gltf::Node,
        parent: Mat4,
        buffers: &[gltf::buffer::Data],
        positions: &mut Vec<Vec3>,
        indices: &mut Vec<u32>,
    ) {
        let matrix = parent * Mat4::from_cols_array_2d(&node.transform().matrix());
        if let Some(mesh) = node.mesh() {
            for primitive in mesh.primitives() {
                if primitive.mode() != gltf::mesh::Mode::Triangles {
                    continue;
                }
                let reader = primitive.reader(|b| buffers.get(b.index()).map(|d| &d.0[..]));
                let Some(read) = reader.read_positions() else { continue };
                let base = positions.len() as u32;
                let local: Vec<Vec3> = read.map(|p| matrix.transform_point3(Vec3::from_array(p))).collect();
                let count = local.len() as u32;
                positions.extend(local);
                match reader.read_indices() {
                    Some(read) => indices.extend(read.into_u32().filter(|i| *i < count).map(|i| i + base)),
                    None => indices.extend((0..count).map(|i| i + base)),
                }
            }
        }
        for child in node.children() {
            visit(child, matrix, buffers, positions, indices);
        }
    }
    let scenes: Vec<gltf::Scene> = match document.default_scene() {
        Some(scene) => vec![scene],
        None => document.scenes().collect(),
    };
    for scene in scenes {
        for node in scene.nodes() {
            visit(node, Mat4::IDENTITY, &buffers, &mut positions, &mut indices);
        }
    }
    let usable = indices.len() / 3 * 3;
    indices.truncate(usable);
    Ok((positions, indices))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::doc::Element;
    use crate::ops;

    #[test]
    fn exported_glb_comes_back_as_a_reference() {
        let mut doc = Document::new("t");
        ops::add_element(&mut doc, "a", Element::cuboid(Vec3::ZERO, Vec3::new(16.0, 8.0, 4.0)), None);
        let glb = crate::export::to_glb(&doc);
        let reference = reference_from_bytes(&doc, "a.glb", &glb).unwrap();
        assert_eq!(reference.indices.len(), 36);
        let max = reference.positions.iter().fold(Vec3::splat(f32::MIN), |m, p| m.max(Vec3::from_array(*p)));
        assert!((max - Vec3::new(16.0, 8.0, 4.0)).length() < 1e-4, "metres become model units: {max}");
        let obj = reference_from_bytes(&doc, "q.obj", b"v 0 0 0\nv 1 0 0\nv 1 1 0\nv 0 1 0\nf 1 2 3 4\n").unwrap();
        assert_eq!(obj.indices, [0, 1, 2, 0, 2, 3]);
        assert!(reference_from_bytes(&doc, "x.fbx", b"").is_err());
    }
}
