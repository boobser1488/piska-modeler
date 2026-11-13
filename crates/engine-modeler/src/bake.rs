//! «Запекание» модели для движка: один меш в метрах и одна текстура.
//!
//! Рендерер движка знает один материал на `MeshRenderer`, поэтому все текстуры модели
//! складываются в один атлас (друг под другом), а стороны с оттенком получают свою
//! подкрашенную копию картинки внизу атласа. Иерархия групп сводится к одному мешу:
//! повороты групп уже применены к вершинам.

use glam::Vec2;

use crate::doc::Document;
use crate::geometry::{build_mesh, BuildOptions};
use crate::image::{multiply, Image, CLEAR};

/// Модель, готовая для рендерера: атрибуты по вершинам, индексы троек, текстура.
#[derive(Clone, Debug, PartialEq)]
pub struct BakedModel {
    /// Метры.
    pub positions: Vec<[f32; 3]>,
    pub normals: Vec<[f32; 3]>,
    /// (0, 0) — левый верхний угол текстуры.
    pub uvs: Vec<[f32; 2]>,
    pub indices: Vec<u32>,
    /// Общий атлас (нет текстур или сторон — `None`).
    pub texture: Option<Image>,
    /// Есть ли светящиеся стороны (рендерер движка светит всем мешем — см. документацию).
    pub has_emissive: bool,
}

impl BakedModel {
    pub fn triangle_count(&self) -> usize {
        self.indices.len() / 3
    }

    /// Границы в метрах.
    pub fn bounds(&self) -> ([f32; 3], [f32; 3]) {
        let mut min = [f32::INFINITY; 3];
        let mut max = [f32::NEG_INFINITY; 3];
        for p in &self.positions {
            for i in 0..3 {
                min[i] = min[i].min(p[i]);
                max[i] = max[i].max(p[i]);
            }
        }
        if self.positions.is_empty() {
            ([0.0; 3], [0.0; 3])
        } else {
            (min, max)
        }
    }

    /// Текстура в PNG.
    pub fn texture_png(&self) -> Option<Vec<u8>> {
        self.texture.as_ref().map(Image::to_png)
    }
}

/// Запечь документ.
pub fn bake(doc: &Document) -> BakedModel {
    let mesh = build_mesh(doc, BuildOptions::export(doc));
    let mut baked = BakedModel {
        positions: mesh.vertices.iter().map(|v| v.position.to_array()).collect(),
        normals: mesh.vertices.iter().map(|v| v.normal.to_array()).collect(),
        uvs: vec![[0.0, 0.0]; mesh.vertices.len()],
        indices: mesh.triangles.iter().flat_map(|t| t.indices).collect(),
        texture: None,
        has_emissive: mesh.triangles.iter().any(|t| t.emissive),
    };
    if mesh.triangles.is_empty() || doc.textures.is_empty() {
        return baked;
    }

    // Текстуры — столбиком.
    let width = doc.textures.iter().map(|t| t.image.width()).max().unwrap_or(1);
    let mut offsets = Vec::new();
    let mut height = 0;
    for t in &doc.textures {
        offsets.push(height);
        height += t.image.height();
    }

    // Подкрашенные стороны — копии их прямоугольников ниже, полками.
    struct Tinted {
        node: crate::doc::NodeId,
        face: crate::doc::FaceDir,
        bounds: [i32; 4],
        at: [u32; 2],
    }
    let mut tinted: Vec<Tinted> = Vec::new();
    let (mut shelf_x, mut shelf_y, mut shelf_h) = (0u32, height, 0u32);
    for triangle in &mesh.triangles {
        if tinted.iter().any(|t| t.node == triangle.node && t.face == triangle.face) {
            continue;
        }
        let Some(face) = doc.element(triangle.node).map(|e| *e.face(triangle.face)) else { continue };
        if face.tint == [255, 255, 255, 255] {
            continue;
        }
        let bounds = face.uv.texel_bounds();
        let (w, h) = ((bounds[2] - bounds[0]).max(1) as u32, (bounds[3] - bounds[1]).max(1) as u32);
        if shelf_x + w > width {
            shelf_x = 0;
            shelf_y += shelf_h;
            shelf_h = 0;
        }
        tinted.push(Tinted {
            node: triangle.node,
            face: triangle.face,
            bounds,
            at: [shelf_x, shelf_y],
        });
        shelf_x += w;
        shelf_h = shelf_h.max(h);
    }
    let total_height = (shelf_y + shelf_h).max(height);

    let mut atlas = Image::new(width, total_height, CLEAR);
    for (t, offset) in doc.textures.iter().zip(&offsets) {
        atlas.blit(&t.image, 0, 0, 0, *offset as i32, t.image.width(), t.image.height());
    }
    for t in &tinted {
        let Some(face) = doc.element(t.node).map(|e| *e.face(t.face)) else { continue };
        let source = doc.textures.get(face.texture as usize).unwrap_or(&doc.textures[0]);
        let [x0, y0, x1, y1] = t.bounds;
        for y in y0..y1 {
            for x in x0..x1 {
                let color = source.image.get(x, y).unwrap_or(CLEAR);
                atlas.set(
                    t.at[0] as i32 + x - x0,
                    t.at[1] as i32 + y - y0,
                    multiply(color, face.tint),
                );
            }
        }
    }

    let atlas_size = Vec2::new(width as f32, total_height as f32);
    let mut done = vec![false; mesh.vertices.len()];
    for triangle in &mesh.triangles {
        let texture_index = (triangle.texture as usize).min(doc.textures.len() - 1);
        let size = doc.textures[texture_index].image.size();
        let size = Vec2::new(size[0] as f32, size[1] as f32);
        let tint = tinted.iter().find(|t| t.node == triangle.node && t.face == triangle.face);
        for index in triangle.indices {
            let i = index as usize;
            if done[i] {
                continue;
            }
            done[i] = true;
            let texel = mesh.vertices[i].uv * size;
            let placed = match tint {
                Some(t) => Vec2::new(
                    texel.x - t.bounds[0] as f32 + t.at[0] as f32,
                    texel.y - t.bounds[1] as f32 + t.at[1] as f32,
                ),
                None => Vec2::new(texel.x, texel.y + offsets[texture_index] as f32),
            };
            baked.uvs[i] = (placed / atlas_size).to_array();
        }
    }
    baked.texture = Some(atlas);
    baked
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::doc::{Element, FaceDir};
    use crate::ops;
    use glam::Vec3;

    #[test]
    fn bake_is_in_metres_with_one_atlas() {
        let mut doc = Document::new("t");
        let a = ops::add_element(&mut doc, "a", Element::cuboid(Vec3::new(-8.0, 0.0, -8.0), Vec3::splat(16.0)), None);
        let up = doc.element(a).unwrap().face(FaceDir::Up).uv;
        doc.textures[0].image.set(up.x as i32, up.y as i32, [200, 100, 50, 255]);
        doc.element_mut(a).unwrap().face_mut(FaceDir::Up).tint = [255, 0, 0, 255];
        let t2 = ops::add_texture(&mut doc, "second", 8, 8);
        doc.textures[1].image.set(0, 0, [1, 2, 3, 255]);
        let b = ops::add_element(&mut doc, "b", Element::cuboid(Vec3::ZERO, Vec3::ONE), None);
        for face in &mut doc.element_mut(b).unwrap().faces {
            face.texture = t2;
            face.uv = crate::doc::UvRect::new(0.0, 0.0, 1.0, 1.0);
        }
        doc.element_mut(b).unwrap().box_uv = false;

        let baked = bake(&doc);
        assert_eq!(baked.triangle_count(), 24);
        let (min, max) = baked.bounds();
        assert_eq!((min, max), ([-0.5, 0.0, -0.5], [0.5, 1.0, 0.5]), "16 units = 1 metre");
        let atlas = baked.texture.as_ref().unwrap();
        assert_eq!(atlas.width(), 64);
        assert!(atlas.height() >= 64 + 8 + 16, "two textures and a tinted copy of the 16x16 top");
        // Вершины стороны b — в углу второй текстуры: (0, 0) → цвет (1, 2, 3).
        let b_vertex = baked.positions.iter().position(|p| *p == [1.0 / 16.0, 1.0 / 16.0, 1.0 / 16.0]).unwrap();
        let uv = baked.uvs[b_vertex];
        let x = (uv[0] * atlas.width() as f32).min(atlas.width() as f32 - 0.5) as i32 - 1;
        let y = (uv[1] * atlas.height() as f32) as i32 - 1;
        assert_eq!(atlas.get(x.max(0), y.max(64)), Some([1, 2, 3, 255]));
        // Подкрашенный верх: красный канал оттенка оставил только красный.
        let top: Vec<[f32; 2]> = (0..baked.positions.len())
            .filter(|i| baked.normals[*i] == [0.0, 1.0, 0.0] && baked.positions[*i][1] == 1.0)
            .map(|i| baked.uvs[i])
            .collect();
        assert_eq!(top.len(), 4);
        let center = top.iter().fold([0.0, 0.0], |a, uv| [a[0] + uv[0] / 4.0, a[1] + uv[1] / 4.0]);
        let color = atlas
            .get((center[0] * atlas.width() as f32) as i32, (center[1] * atlas.height() as f32) as i32)
            .unwrap();
        assert!(color[0] > 0 && color[1] == 0 && color[2] == 0, "{color:?}");
        assert!(center[1] * atlas.height() as f32 > 72.0, "the tinted copy lies below both textures");
    }
}
