//! Правка элементов-мешей ([`Shape::Mesh`]): вершины и грани, развёртка для рисования,
//! превращение в кубоиды.
//!
//! Что есть: сдвиг вершин, сдвиг грани вдоль её нормали, удаление граней, слияние вершин
//! по расстоянию, отражение, «Generate UVs» (плоская проекция каждой грани в свободное
//! место атласа) и «Convert to Cubes» (если меш — точное объединение осевых коробок).
//! Чего нет: выдавливания, разрезов петлёй, добавления вершин.

use glam::{Vec2, Vec3};

use crate::doc::{Document, Element, Face, FaceDir, MeshFace, MeshShape, NodeId, Shape, UvRect};
use crate::geometry::cuboid_face;
use crate::image::Rgba;
use crate::{ops, uv};

/// Ось, вдоль которой вектор длиннее всего (0 — X, 1 — Y, 2 — Z).
fn major_axis(v: Vec3) -> usize {
    let a = v.abs();
    if a.x >= a.y && a.x >= a.z {
        0
    } else if a.y >= a.z {
        1
    } else {
        2
    }
}

/// Отразить форму по оси (в долях коробки) и развернуть обход граней.
pub fn flip_shape(mesh: &mut MeshShape, axis: usize) {
    for v in &mut mesh.vertices {
        v[axis] = 1.0 - v[axis];
    }
    for face in &mut mesh.faces {
        face.vertices.reverse();
        face.uvs.reverse();
    }
}

/// Добавить меш: `points` — вершины в единицах модели, грани против часовой стрелки.
pub fn add_mesh(doc: &mut Document, name: &str, points: &[Vec3], faces: Vec<MeshFace>, parent: Option<NodeId>) -> NodeId {
    ops::add_element(doc, name, Element::mesh(points, faces), parent)
}

/// Нормаль грани в пространстве элемента (Ньюэлл; ноль у вырожденной).
pub fn face_normal(points: &[Vec3], face: &MeshFace) -> Vec3 {
    let mut n = Vec3::ZERO;
    let count = face.vertices.len();
    for i in 0..count {
        let (Some(a), Some(b)) = (points.get(face.vertices[i] as usize), points.get(face.vertices[(i + 1) % count] as usize)) else {
            return Vec3::ZERO;
        };
        n += Vec3::new((a.y - b.y) * (a.z + b.z), (a.z - b.z) * (a.x + b.x), (a.x - b.x) * (a.y + b.y));
    }
    n.normalize_or_zero()
}

fn with_points(doc: &mut Document, id: NodeId, f: impl FnOnce(&mut Vec<Vec3>, &mut MeshShape)) -> bool {
    let Some(element) = doc.element_mut(id) else { return false };
    let Some(mut shape) = element.mesh.take() else { return false };
    element.mesh = Some(shape.clone());
    let mut points = element.mesh_points();
    f(&mut points, &mut shape);
    element.mesh = Some(shape);
    element.set_mesh_points(&points);
    ops::sync_twins(doc, &[id]);
    true
}

/// Сдвинуть вершины на `delta` в мировых координатах (повороты групп и элемента учтены).
pub fn move_vertices(doc: &mut Document, id: NodeId, vertices: &[u32], delta: Vec3) -> bool {
    let local = doc.world_matrix(id).inverse().transform_vector3(delta);
    with_points(doc, id, |points, _| {
        for v in vertices {
            if let Some(p) = points.get_mut(*v as usize) {
                *p += local;
            }
        }
    })
}

/// Сдвинуть грань вдоль её нормали на `distance` единиц модели (наружу — положительное).
pub fn move_face(doc: &mut Document, id: NodeId, face: usize, distance: f32) -> bool {
    with_points(doc, id, |points, shape| {
        let Some(f) = shape.faces.get(face) else { return };
        let normal = face_normal(points, f);
        let mut moved: Vec<u32> = f.vertices.clone();
        moved.sort_unstable();
        moved.dedup();
        for v in moved {
            if let Some(p) = points.get_mut(v as usize) {
                *p += normal * distance;
            }
        }
    })
}

/// Удалить грани (вершины, которые больше никому не нужны, тоже).
pub fn delete_faces(doc: &mut Document, id: NodeId, faces: &[usize]) -> bool {
    with_points(doc, id, |points, shape| {
        let mut index = 0;
        shape.faces.retain(|_| {
            let keep = !faces.contains(&index);
            index += 1;
            keep
        });
        compact(points, shape);
    })
}

/// Убрать вершины без граней, перенумеровать грани.
fn compact(points: &mut Vec<Vec3>, shape: &mut MeshShape) {
    let mut used = vec![false; points.len()];
    for f in &shape.faces {
        for v in &f.vertices {
            if let Some(u) = used.get_mut(*v as usize) {
                *u = true;
            }
        }
    }
    let mut remap = vec![u32::MAX; points.len()];
    let mut kept = Vec::new();
    for (i, p) in points.iter().enumerate() {
        if used[i] {
            remap[i] = kept.len() as u32;
            kept.push(*p);
        }
    }
    for f in &mut shape.faces {
        for v in &mut f.vertices {
            *v = remap.get(*v as usize).copied().unwrap_or(0);
        }
    }
    *points = kept;
}

/// Слить вершины ближе `distance` друг к другу. Грани, ставшие вырожденными, удаляются.
/// Возвращает, сколько вершин убрано.
pub fn merge_by_distance(doc: &mut Document, id: NodeId, distance: f32) -> usize {
    let mut removed = 0;
    with_points(doc, id, |points, shape| {
        let before = points.len();
        let mut target: Vec<u32> = (0..points.len() as u32).collect();
        for i in 0..points.len() {
            if target[i] != i as u32 {
                continue;
            }
            for j in i + 1..points.len() {
                if target[j] == j as u32 && points[i].distance(points[j]) <= distance {
                    target[j] = i as u32;
                }
            }
        }
        for f in &mut shape.faces {
            let mut corners: Vec<(u32, [f32; 2])> = Vec::new();
            for (k, v) in f.vertices.iter().enumerate() {
                let t = target[*v as usize];
                if corners.last().map(|c| c.0) != Some(t) {
                    corners.push((t, f.uvs.get(k).copied().unwrap_or([0.0, 0.0])));
                }
            }
            if corners.len() > 1 && corners.first().map(|c| c.0) == corners.last().map(|c| c.0) {
                corners.pop();
            }
            f.vertices = corners.iter().map(|c| c.0).collect();
            f.uvs = corners.iter().map(|c| c.1).collect();
        }
        shape.faces.retain(|f| f.vertices.len() >= 3);
        compact(points, shape);
        removed = before - points.len();
    });
    removed
}

//// «Generate UVs»: каждая грань проецируется на плоскость своей главной оси (бока —
/// картинкой вверх) и получает свободное место в атласе (холст растёт), место заливается
/// цветом `colors[грань]` или [`ops::BASE_COLOR`] — по мешу можно рисовать.
pub fn generate_uvs(doc: &mut Document, id: NodeId, colors: Option<&[Rgba]>) -> Result<usize, String> {
    let count = doc.element(id).and_then(|e| e.mesh.as_ref()).ok_or("the selection is not a mesh")?.faces.len();
    let all: Vec<usize> = (0..count).collect();
    layout_faces(doc, id, &all, &|index| colors.and_then(|c| c.get(index)).copied().unwrap_or(ops::BASE_COLOR), true)
}

/// Места в атласе для новых граней меша (после выдавливания, заполнения, моста):
/// проекция с плотностью текселей модели, заливка цветом соседней старой грани — по
/// новым граням сразу можно рисовать, нарисованное на старых не трогается.
pub fn allocate_uvs(doc: &mut Document, id: NodeId, faces: &[usize]) -> Result<usize, String> {
    if faces.is_empty() {
        return Ok(0);
    }
    let colors = neighbour_colors(doc, id, faces);
    layout_faces(doc, id, faces, &|index| colors.get(&index).copied().unwrap_or(ops::BASE_COLOR), false)
}

/// Цвет соседней (не новой) грани для каждой новой: тексель в центре её UV.
fn neighbour_colors(doc: &Document, id: NodeId, faces: &[usize]) -> std::collections::HashMap<usize, Rgba> {
    let mut out = std::collections::HashMap::new();
    let Some(mesh) = doc.element(id).and_then(crate::mesh_ops::EditMesh::of) else { return out };
    let ef = mesh.edge_faces();
    for &f in faces {
        if f >= mesh.faces.len() {
            continue;
        }
        let neighbour = mesh
            .face_edges(f)
            .iter()
            .filter_map(|(a, b)| ef.get(&crate::mesh_ops::edge(*a, *b)))
            .flatten()
            .find(|g| !faces.contains(g) && mesh.faces[**g].uvs.iter().any(|uv| *uv != [0.0, 0.0]))
            .copied();
        if let Some(g) = neighbour {
            let n = &mesh.faces[g];
            let c = n.uvs.iter().fold([0.0f32; 2], |s, uv| [s[0] + uv[0], s[1] + uv[1]]);
            let c = [c[0] / n.uvs.len() as f32, c[1] / n.uvs.len() as f32];
            if let Some(color) = doc.textures.get(n.texture as usize).and_then(|t| t.image.get(c[0].floor() as i32, c[1].floor() as i32)) {
                out.insert(f, color);
            }
        }
    }
    out
}

/// Разложить грани `which` меша плоской проекцией в свободные места атласа.
fn layout_faces(doc: &mut Document, id: NodeId, which: &[usize], color: &dyn Fn(usize) -> Rgba, whole: bool) -> Result<usize, String> {
    let element = doc.element(id).cloned().ok_or("the selection is not a mesh")?;
    let shape = element.mesh.clone().ok_or("the selection is not a mesh")?;
    if doc.textures.is_empty() {
        ops::add_texture(doc, "atlas", crate::doc::DEFAULT_TEXTURE_SIZE, crate::doc::DEFAULT_TEXTURE_SIZE);
    }
    let texture = shape.faces.first().map_or(0, |f| f.texture).min(doc.textures.len() as u32 - 1);
    let points = element.mesh_points();
    let density = doc.texel_density.max(1e-3);
    // Целиком — старые места этого меша свободны; частично — заняты остальными гранями.
    let mut taken = if whole { uv::occupied(doc, texture, &[id]) } else { occupied_except(doc, texture, id, which) };
    let mut faces = shape.faces.clone();
    // Проекции граней и их размеры в текселях.
    let mut items: Vec<(usize, Vec<Vec2>, Vec2, [u32; 2])> = Vec::with_capacity(which.len());
    for &index in which {
        let Some(face) = faces.get(index) else { continue };
        let normal = face_normal(&points, face);
        let axis = major_axis(normal);
        // Горизонталь и вертикаль картинки: у боков вертикаль — −Y (вверх по картинке — вверх).
        let (h, v, flip_v) = match axis {
            0 => (2, 1, true),
            1 => (0, 2, false),
            _ => (0, 1, true),
        };
        let projected: Vec<Vec2> = face
            .vertices
            .iter()
            .map(|i| points.get(*i as usize).copied().unwrap_or(Vec3::ZERO))
            .map(|p| Vec2::new(p[h], if flip_v { -p[v] } else { p[v] }))
            .collect();
        let min = projected.iter().fold(Vec2::splat(f32::INFINITY), |m, p| m.min(*p));
        let max = projected.iter().fold(Vec2::splat(f32::NEG_INFINITY), |m, p| m.max(*p));
        let size = [((max.x - min.x) * density - 1e-3).ceil().max(1.0) as u32, ((max.y - min.y) * density - 1e-3).ceil().max(1.0) as u32];
        items.push((index, projected, min, size));
    }
    // Немного граней — в свободные места между занятыми; много — полками ниже всего занятого
    // (поиск места для каждой из тысяч граней был бы слишком долгим).
    let origins: Vec<[u32; 2]> = if items.len() <= 64 {
        let mut out = Vec::with_capacity(items.len());
        for (index, _, _, size) in &items {
            let origin = loop {
                let canvas = doc.textures[texture as usize].image.size();
                if let Some(origin) = uv::find_free(&taken, canvas, *size) {
                    break origin;
                }
                if !uv::grow_texture(doc, texture) {
                    return Err(format!("there is no room on the texture for face {index}; add a larger texture"));
                }
            };
            taken.push([origin[0], origin[1], size[0], size[1]]);
            out.push(origin);
        }
        out
    } else {
        let start = taken.iter().map(|r| r[1] + r[3]).max().unwrap_or(0);
        let sizes: Vec<[u32; 2]> = items.iter().map(|i| i.3).collect();
        loop {
            let canvas = doc.textures[texture as usize].image.size();
            if let Some(origins) = shelf_pack(&sizes, canvas, start) {
                break origins;
            }
            if !uv::grow_texture(doc, texture) {
                return Err(format!("{} faces do not fit on a {}x{} texture; add a larger texture", items.len(), uv::MAX_TEXTURE_SIZE, uv::MAX_TEXTURE_SIZE));
            }
        }
    };
    for ((index, projected, min, size), origin) in items.into_iter().zip(origins) {
        let face = &mut faces[index];
        face.uvs = projected
            .iter()
            .map(|p| [origin[0] as f32 + (p.x - min.x) * density, origin[1] as f32 + (p.y - min.y) * density])
            .collect();
        face.texture = texture;
        let fill = color(index);
        let image = &mut doc.textures[texture as usize].image;
        for y in origin[1]..origin[1] + size[1] {
            for x in origin[0]..origin[0] + size[0] {
                image.set(x as i32, y as i32, fill);
            }
        }
    }
    if let Some(mesh) = doc.element_mut(id).and_then(|e| e.mesh.as_mut()) {
        mesh.faces = faces;
    }
    ops::sync_twins(doc, &[id]);
    Ok(which.len())
}

/// Уложить прямоугольники полками (по высоте, от высоких к низким) на холст `canvas`
/// начиная со строки `start`; `None` — не помещаются.
fn shelf_pack(sizes: &[[u32; 2]], canvas: [u32; 2], start: u32) -> Option<Vec<[u32; 2]>> {
    let mut order: Vec<usize> = (0..sizes.len()).collect();
    order.sort_by(|a, b| sizes[*b][1].cmp(&sizes[*a][1]).then(a.cmp(b)));
    let mut out = vec![[0u32; 2]; sizes.len()];
    let (mut x, mut y, mut shelf) = (0u32, start, 0u32);
    for i in order {
        let [w, h] = sizes[i];
        if w > canvas[0] {
            return None;
        }
        if x + w > canvas[0] {
            x = 0;
            y += shelf;
            shelf = 0;
        }
        if y + h > canvas[1] {
            return None;
        }
        out[i] = [x, y];
        x += w;
        shelf = shelf.max(h);
    }
    Some(out)
}

// Занятые места текстуры, кроме граней `which` меша `id`.
fn occupied_except(doc: &Document, texture: u32, id: NodeId, which: &[usize]) -> Vec<uv::Rect> {
    let mut out = uv::occupied(doc, texture, &[id]);
    if let Some(mesh) = doc.element(id).and_then(|e| e.mesh.as_ref()) {
        for (_, face) in mesh.faces.iter().enumerate().filter(|(i, f)| !which.contains(i) && f.texture == texture) {
            let (mut lo, mut hi) = ([f32::INFINITY; 2], [f32::NEG_INFINITY; 2]);
            for uv in &face.uvs {
                for k in 0..2 {
                    lo[k] = lo[k].min(uv[k]);
                    hi[k] = hi[k].max(uv[k]);
                }
            }
            if hi[0] > 0.0 || hi[1] > 0.0 {
                let (x0, y0) = (lo[0].floor().max(0.0) as u32, lo[1].floor().max(0.0) as u32);
                out.push([x0, y0, (hi[0].ceil().max(0.0) as u32).saturating_sub(x0).max(1), (hi[1].ceil().max(0.0) as u32).saturating_sub(y0).max(1)]);
            }
        }
    }
    out
}

/// Плотность текселей грани: текселей на единицу модели (корень из отношения площадей).
pub fn face_density(points: &[Vec3], face: &MeshFace) -> f32 {
    let corners: Vec<Vec3> = face.vertices.iter().map(|v| points.get(*v as usize).copied().unwrap_or(Vec3::ZERO)).collect();
    let area = crate::mesh_ops::newell(&corners).length() * 0.5;
    let mut uv_area = 0.0;
    for i in 0..face.uvs.len() {
        let (a, b) = (face.uvs[i], face.uvs[(i + 1) % face.uvs.len()]);
        uv_area += a[0] * b[1] - b[0] * a[1];
    }
    if area < 1e-9 {
        0.0
    } else {
        (uv_area.abs() * 0.5 / area).sqrt()
    }
}

/// Прижать UV всех граней меша к углам текселей (пиксели не скашиваются).
pub fn snap_uvs(doc: &mut Document, id: NodeId) -> bool {
    let Some(mesh) = doc.element_mut(id).and_then(|e| e.mesh.as_mut()) else { return false };
    for face in &mut mesh.faces {
        for uv in &mut face.uvs {
            *uv = [uv[0].round(), uv[1].round()];
        }
    }
    ops::sync_twins(doc, &[id]);
    true
}

/// Одна плотность текселей для меша: развёртка заново с плотностью `density` (цвет
/// каждой грани — тексель в её прежнем центре).
pub fn set_density(doc: &mut Document, id: NodeId, density: f32) -> Result<usize, String> {
    let mesh = doc.element(id).and_then(crate::mesh_ops::EditMesh::of).ok_or("the selection is not a mesh")?;
    let colors: Vec<Rgba> = mesh
        .faces
        .iter()
        .map(|f| {
            let c = f.uvs.iter().fold([0.0f32; 2], |s, uv| [s[0] + uv[0], s[1] + uv[1]]);
            let n = f.uvs.len().max(1) as f32;
            doc.textures.get(f.texture as usize).and_then(|t| t.image.get((c[0] / n).floor() as i32, (c[1] / n).floor() as i32)).unwrap_or(ops::BASE_COLOR)
        })
        .collect();
    let old = doc.texel_density;
    doc.texel_density = density.clamp(0.05, 64.0);
    let result = generate_uvs(doc, id, Some(&colors));
    doc.texel_density = old;
    result
}

// ----------------------------------------------------------------------
// Convert to Cubes
// ----------------------------------------------------------------------

/// Точка внутри многоугольника на плоскости (чётность пересечений).
fn inside_2d(polygon: &[Vec2], p: Vec2) -> bool {
    let mut inside = false;
    for i in 0..polygon.len() {
        let (a, b) = (polygon[i], polygon[(i + 1) % polygon.len()]);
        if (a.y > p.y) != (b.y > p.y) && p.x < a.x + (p.y - a.y) / (b.y - a.y) * (b.x - a.x) {
            inside = !inside;
        }
    }
    inside
}

fn on_or_inside(polygon: &[Vec2], p: Vec2) -> bool {
    inside_2d(polygon, p)
        || (0..polygon.len()).any(|i| {
            let (a, b) = (polygon[i], polygon[(i + 1) % polygon.len()]);
            let ab = b - a;
            let t = ((p - a).dot(ab) / ab.length_squared().max(1e-12)).clamp(0.0, 1.0);
            (a + ab * t).distance(p) < 1e-3
        })
}

/// Грань меша, лежащая в осевой плоскости.
struct AxisFace {
    axis: usize,
    positive: bool,
    plane: f32,
    polygon: Vec<Vec2>,
    /// Три опорные точки (в 2D плоскости) и их UV — для переноса UV на сторону кубоида.
    anchors: [(Vec2, [f32; 2]); 3],
    texture: u32,
    area: f32,
}

fn plane_2d(p: Vec3, axis: usize) -> Vec2 {
    Vec2::new(p[(axis + 1) % 3], p[(axis + 2) % 3])
}

impl AxisFace {
    fn uv_at(&self, p: Vec2) -> [f32; 2] {
        let [(p0, u0), (p1, u1), (p2, u2)] = self.anchors;
        let (e1, e2, d) = (p1 - p0, p2 - p0, p - p0);
        let det = e1.x * e2.y - e1.y * e2.x;
        if det.abs() < 1e-12 {
            return u0;
        }
        let s = (d.x * e2.y - d.y * e2.x) / det;
        let t = (e1.x * d.y - e1.y * d.x) / det;
        [u0[0] + s * (u1[0] - u0[0]) + t * (u2[0] - u0[0]), u0[1] + s * (u1[1] - u0[1]) + t * (u2[1] - u0[1])]
    }
}

/// Прямоугольник стороны по UV её углов (левый верхний, правый верхний, правый нижний,
/// левый нижний): подбираются поворот и отражения.
fn rect_from_corners(corners: [[f32; 2]; 4]) -> Option<UvRect> {
    let min = |k: usize| corners.iter().map(|c| c[k]).fold(f32::INFINITY, f32::min);
    let max = |k: usize| corners.iter().map(|c| c[k]).fold(f32::NEG_INFINITY, f32::max);
    let base = UvRect::new(min(0), min(1), max(0) - min(0), max(1) - min(1));
    for rotation in 0..4 {
        for flip_u in [false, true] {
            for flip_v in [false, true] {
                let rect = UvRect { rotation, flip_u, flip_v, ..base };
                let got = rect.corners();
                if (0..4).all(|i| (got[i][0] - corners[i][0]).abs() < 1e-3 && (got[i][1] - corners[i][1]).abs() < 1e-3) {
                    return Some(rect);
                }
            }
        }
    }
    None
}

/// «Convert to Cubes»: если меш — точное объединение осевых коробок (так выглядят модели,
/// выгруженные из редакторов блоков), заменить его кубоидами с теми же UV и текстурой.
/// Иначе — объяснение, почему нельзя.
pub fn convert_to_cubes(doc: &mut Document, id: NodeId) -> Result<Vec<NodeId>, String> {
    let node = doc.node(id).cloned().ok_or("nothing is selected")?;
    let element = node.element().cloned().ok_or("select a mesh element")?;
    let shape = element.mesh.clone().ok_or("only mesh elements can be converted to cubes")?;
    let points = element.mesh_points();
    let mut faces: Vec<AxisFace> = Vec::new();
    for (index, face) in shape.faces.iter().enumerate() {
        let normal = face_normal(&points, face);
        let axis = major_axis(normal);
        if normal == Vec3::ZERO {
            continue;
        }
        if normal[axis].abs() < 0.9999 {
            return Err(format!("face {index} is not aligned to an axis, so the mesh is not made of boxes"));
        }
        let corners: Vec<Vec3> = face.vertices.iter().map(|v| points[*v as usize]).collect();
        let plane = corners[0][axis];
        if corners.iter().any(|c| (c[axis] - plane).abs() > 1e-3) {
            return Err(format!("face {index} is not flat"));
        }
        let polygon: Vec<Vec2> = corners.iter().map(|c| plane_2d(*c, axis)).collect();
        let uvs: Vec<[f32; 2]> = (0..corners.len()).map(|k| face.uvs.get(k).copied().unwrap_or([0.0, 0.0])).collect();
        // Опорная тройка — первая невырожденная.
        let mut anchors = None;
        'find: for a in 0..polygon.len() {
            for b in a + 1..polygon.len() {
                for c in b + 1..polygon.len() {
                    let (e1, e2) = (polygon[b] - polygon[a], polygon[c] - polygon[a]);
                    if (e1.x * e2.y - e1.y * e2.x).abs() > 1e-6 {
                        anchors = Some([(polygon[a], uvs[a]), (polygon[b], uvs[b]), (polygon[c], uvs[c])]);
                        break 'find;
                    }
                }
            }
        }
        let Some(anchors) = anchors else { continue };
        let mut area = 0.0;
        for i in 0..polygon.len() {
            let (a, b) = (polygon[i], polygon[(i + 1) % polygon.len()]);
            area += a.x * b.y - b.x * a.y;
        }
        faces.push(AxisFace { axis, positive: normal[axis] > 0.0, plane, polygon, anchors, texture: face.texture, area: area.abs() * 0.5 });
    }
    if faces.is_empty() {
        return Err("the mesh has no faces".to_string());
    }
    // Сетка из всех различных координат.
    let mut coords: [Vec<f32>; 3] = Default::default();
    for p in &points {
        for k in 0..3 {
            coords[k].push((p[k] * 1e4).round() / 1e4);
        }
    }
    for c in &mut coords {
        c.sort_by(f32::total_cmp);
        c.dedup();
        if c.len() > 65 {
            return Err("the mesh has too many different coordinates to be a few boxes".to_string());
        }
        if c.len() < 2 {
            return Err("the mesh is flat; it has no volume to fill with boxes".to_string());
        }
    }
    let n = [coords[0].len() - 1, coords[1].len() - 1, coords[2].len() - 1];
    let index = |i: usize, j: usize, k: usize| i + n[0] * (j + n[1] * k);
    let mut inside = vec![false; n[0] * n[1] * n[2]];
    for k in 0..n[2] {
        for j in 0..n[1] {
            for i in 0..n[0] {
                let c = Vec3::new((coords[0][i] + coords[0][i + 1]) * 0.5, (coords[1][j] + coords[1][j + 1]) * 0.5, (coords[2][k] + coords[2][k + 1]) * 0.5);
                let crossings = faces.iter().filter(|f| f.axis == 0 && f.plane > c.x && inside_2d(&f.polygon, plane_2d(c, 0))).count();
                inside[index(i, j, k)] = crossings % 2 == 1;
            }
        }
    }
    let filled = |i: i64, j: i64, k: i64| -> bool {
        i >= 0 && j >= 0 && k >= 0 && (i as usize) < n[0] && (j as usize) < n[1] && (k as usize) < n[2] && inside[index(i as usize, j as usize, k as usize)]
    };
    // Поверхность объединения ячеек должна совпасть с площадью граней меша.
    let mut exposed = 0.0f32;
    for k in 0..n[2] {
        for j in 0..n[1] {
            for i in 0..n[0] {
                if !inside[index(i, j, k)] {
                    continue;
                }
                let size = Vec3::new(coords[0][i + 1] - coords[0][i], coords[1][j + 1] - coords[1][j], coords[2][k + 1] - coords[2][k]);
                let (i, j, k) = (i as i64, j as i64, k as i64);
                for (di, dj, dk, area) in [(1, 0, 0, size.y * size.z), (-1, 0, 0, size.y * size.z), (0, 1, 0, size.x * size.z), (0, -1, 0, size.x * size.z), (0, 0, 1, size.x * size.y), (0, 0, -1, size.x * size.y)] {
                    if !filled(i + di, j + dj, k + dk) {
                        exposed += area;
                    }
                }
            }
        }
    }
    let total: f32 = faces.iter().map(|f| f.area).sum();
    if exposed == 0.0 || (exposed - total).abs() > 1e-3 * total.max(1.0) {
        return Err(format!(
            "the mesh is not a closed union of boxes (its faces cover {total:.2} square units, the boxes inside it {exposed:.2}); it stays a mesh"
        ));
    }
    // Коробки: сначала с жадным объединением ячеек, а если сторона коробки попадает на
    // несколько граней меша (их UV не сводятся к одному прямоугольнику) — по ячейке.
    for merge in [true, false] {
        if let Some(cubes) = boxes(&coords, n, &inside, merge).and_then(|b| cube_elements(&b, &faces, &element, &filled, &coords)) {
            let parent = node.parent;
            let position = doc.index_of(id).unwrap_or(doc.nodes.len());
            doc.nodes.retain(|n| n.id != id);
            let mut ids = Vec::new();
            for (k, cube) in cubes.into_iter().enumerate() {
                let new = doc.allocate_id();
                let name = doc.unique_name(&format!("{} {}", node.name, k + 1));
                doc.nodes.insert(
                    (position + k).min(doc.nodes.len()),
                    crate::doc::Node { id: new, name, parent, visible: node.visible, locked: false, kind: crate::doc::NodeKind::Element(cube) },
                );
                ids.push(new);
            }
            return Ok(ids);
        }
    }
    Err("the box faces do not line up with single mesh faces, so the texture cannot be kept; it stays a mesh".to_string())
}

/// Ячейки → коробки (индексы ячеек: начало и конец, не включая).
fn boxes(coords: &[Vec<f32>; 3], n: [usize; 3], inside: &[bool], merge: bool) -> Option<Vec<([usize; 3], [usize; 3])>> {
    let _ = coords;
    let index = |i: usize, j: usize, k: usize| i + n[0] * (j + n[1] * k);
    let mut used = vec![false; inside.len()];
    let mut out = Vec::new();
    for k in 0..n[2] {
        for j in 0..n[1] {
            for i in 0..n[0] {
                if !inside[index(i, j, k)] || used[index(i, j, k)] {
                    continue;
                }
                let free = |a: usize, b: usize, c: usize, used: &Vec<bool>| inside[index(a, b, c)] && !used[index(a, b, c)];
                let (mut i1, mut k1, mut j1) = (i + 1, k + 1, j + 1);
                if merge {
                    while i1 < n[0] && free(i1, j, k, &used) {
                        i1 += 1;
                    }
                    while k1 < n[2] && (i..i1).all(|a| free(a, j, k1, &used)) {
                        k1 += 1;
                    }
                    while j1 < n[1] && (i..i1).all(|a| (k..k1).all(|c| free(a, j1, c, &used))) {
                        j1 += 1;
                    }
                }
                for c in k..k1 {
                    for b in j..j1 {
                        for a in i..i1 {
                            used[index(a, b, c)] = true;
                        }
                    }
                }
                out.push(([i, j, k], [i1, j1, k1]));
                if out.len() > 4096 {
                    return None;
                }
            }
        }
    }
    Some(out)
}

fn cube_elements(
    list: &[([usize; 3], [usize; 3])],
    faces: &[AxisFace],
    source: &Element,
    filled: &dyn Fn(i64, i64, i64) -> bool,
    coords: &[Vec<f32>; 3],
) -> Option<Vec<Element>> {
    let mut out = Vec::new();
    for (lo, hi) in list {
        let a = Vec3::new(coords[0][lo[0]], coords[1][lo[1]], coords[2][lo[2]]);
        let b = Vec3::new(coords[0][hi[0]], coords[1][hi[1]], coords[2][hi[2]]);
        let mut cube = Element::cuboid(a, b - a);
        cube.pivot = source.pivot;
        cube.rotation = source.rotation;
        cube.box_uv = false;
        let style = *source.face(FaceDir::North);
        for dir in FaceDir::ALL {
            let axis = dir.axis();
            // Сторона закрыта, если за ней все ячейки заняты.
            let (u, v) = ((axis + 1) % 3, (axis + 2) % 3);
            let outside = if dir.positive() { hi[axis] as i64 } else { lo[axis] as i64 - 1 };
            let covered = (lo[u]..hi[u]).all(|x| {
                (lo[v]..hi[v]).all(|y| {
                    let mut p = [0i64; 3];
                    p[axis] = outside;
                    p[u] = x as i64;
                    p[v] = y as i64;
                    filled(p[0], p[1], p[2])
                })
            });
            if covered {
                cube.faces[dir.index()] = Face { enabled: false, ..style };
                continue;
            }
            let corners = cuboid_face(a, b, dir);
            let plane = if dir.positive() { b[axis] } else { a[axis] };
            let center = plane_2d(corners.iter().fold(Vec3::ZERO, |s, c| s + *c) * 0.25, axis);
            let face = faces.iter().find(|f| f.axis == axis && f.positive == dir.positive() && (f.plane - plane).abs() < 1e-3 && inside_2d(&f.polygon, center))?;
            if !corners.iter().all(|c| on_or_inside(&face.polygon, plane_2d(*c, axis))) {
                return None;
            }
            let uv = corners.map(|c| face.uv_at(plane_2d(c, axis)));
            let rect = rect_from_corners(uv)?;
            cube.faces[dir.index()] = Face { enabled: true, texture: face.texture, uv: rect, ..style };
        }
        out.push(cube);
    }
    Some(out)
}

/// Элемент — меш без UV (по нему нельзя рисовать, пока не сделать «Generate UVs»).
pub fn needs_uvs(element: &Element) -> bool {
    element.shape == Shape::Mesh && element.mesh.as_ref().is_some_and(|m| !m.has_uvs())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Единичная коробка мешем: 8 вершин, 6 квадратов против часовой стрелки снаружи.
    pub(crate) fn box_mesh(min: Vec3, max: Vec3, uv: bool) -> (Vec<Vec3>, Vec<MeshFace>) {
        let mut points = Vec::new();
        let mut faces = Vec::new();
        for (k, dir) in FaceDir::ALL.iter().enumerate() {
            let c = cuboid_face(min, max, *dir); // по часовой стрелке снаружи
            let base = points.len() as u32;
            points.extend([c[0], c[3], c[2], c[1]]);
            let x = (k * 4) as f32;
            let uvs = if uv { vec![[x, 0.0], [x, 4.0], [x + 4.0, 4.0], [x + 4.0, 0.0]] } else { vec![[0.0, 0.0]; 4] };
            faces.push(MeshFace { vertices: vec![base, base + 1, base + 2, base + 3], uvs, texture: 0 });
        }
        (points, faces)
    }

    #[test]
    fn mesh_follows_move_resize_and_flip() {
        let mut doc = Document::new("t");
        let (points, faces) = box_mesh(Vec3::ZERO, Vec3::new(2.0, 4.0, 6.0), true);
        let id = add_mesh(&mut doc, "m", &points, faces, None);
        assert_eq!(doc.element(id).unwrap().size, Vec3::new(2.0, 4.0, 6.0));
        ops::move_by(&mut doc, &[id], Vec3::X);
        let moved = doc.element(id).unwrap().mesh_points();
        assert!(moved.iter().all(|p| p.x >= 1.0 && p.x <= 3.0));
        ops::resize_face(&mut doc, id, FaceDir::Up, 4.0);
        assert!((doc.element(id).unwrap().mesh_points().iter().map(|p| p.y).fold(0.0, f32::max) - 8.0).abs() < 1e-5);
        // Отражение сохраняет нормали наружу.
        ops::flip(&mut doc, &[id], 0);
        let mesh = crate::build_mesh(&doc, Default::default());
        let center = doc.element(id).unwrap().center();
        for t in &mesh.triangles {
            let [a, b, c] = mesh.triangle_positions(t);
            assert!((b - a).cross(c - a).dot((a + b + c) / 3.0 - center) > 0.0);
        }
    }

    #[test]
    fn vertex_and_face_edits() {
        let mut doc = Document::new("t");
        let (points, faces) = box_mesh(Vec3::ZERO, Vec3::splat(2.0), true);
        let id = add_mesh(&mut doc, "m", &points, faces, None);
        assert_eq!(merge_by_distance(&mut doc, id, 1e-4), 16, "24 corners of 6 quads share 8 points");
        let up = 4; // FaceDir::Up
        assert!(move_face(&mut doc, id, up, 3.0));
        assert_eq!(doc.element(id).unwrap().size.y, 5.0);
        assert!(move_vertices(&mut doc, id, &[0], Vec3::new(0.0, 0.0, -1.0)));
        assert_eq!(doc.element(id).unwrap().from.z, -1.0);
        assert!(delete_faces(&mut doc, id, &[0, 1]));
        assert_eq!(doc.element(id).unwrap().mesh.as_ref().unwrap().faces.len(), 4);
    }

    #[test]
    fn convert_to_cubes_keeps_uvs_or_explains() {
        let mut doc = Document::new("t");
        ops::resize_texture(&mut doc, 0, 32, 8);
        let (points, faces) = box_mesh(Vec3::ZERO, Vec3::new(4.0, 4.0, 4.0), true);
        let id = add_mesh(&mut doc, "crate", &points, faces, None);
        let before = crate::build_mesh(&doc, Default::default());
        let ids = convert_to_cubes(&mut doc, id).unwrap();
        assert_eq!(ids.len(), 1);
        let cube = doc.element(ids[0]).unwrap();
        assert_eq!(cube.shape, Shape::Cuboid);
        let after = crate::build_mesh(&doc, Default::default());
        // Те же треугольники с теми же UV (как множества точек).
        let key = |m: &crate::BuiltMesh| {
            let mut v: Vec<[i32; 5]> = m
                .vertices
                .iter()
                .map(|v| [(v.position.x * 100.0) as i32, (v.position.y * 100.0) as i32, (v.position.z * 100.0) as i32, (v.uv.x * 1000.0) as i32, (v.uv.y * 1000.0) as i32])
                .collect();
            v.sort();
            v.dedup();
            v
        };
        assert_eq!(key(&after), key(&before));
        // Не коробки: угол одной грани вытянут — поверхность не замкнута.
        let mut doc = Document::new("t");
        let (mut points, faces) = box_mesh(Vec3::ZERO, Vec3::splat(4.0), false);
        points[0].x += 1.0;
        let id = add_mesh(&mut doc, "wedge", &points, faces, None);
        let err = convert_to_cubes(&mut doc, id).unwrap_err();
        assert!(err.contains("not a closed union of boxes"), "{err}");
        assert!(doc.element(id).is_some(), "the mesh stays");
    }

    #[test]
    fn generated_uvs_make_a_mesh_paintable() {
        let mut doc = Document::new("t");
        let (points, faces) = box_mesh(Vec3::ZERO, Vec3::splat(4.0), false);
        let id = add_mesh(&mut doc, "m", &points, faces, None);
        assert!(needs_uvs(doc.element(id).unwrap()));
        let colors = [[255, 0, 0, 255]; 6];
        assert_eq!(generate_uvs(&mut doc, id, Some(&colors)).unwrap(), 6);
        assert!(!needs_uvs(doc.element(id).unwrap()));
        let mesh = doc.element(id).unwrap().mesh.clone().unwrap();
        for f in &mesh.faces {
            let [x, y] = f.uvs[0];
            let c = doc.textures[0].image.get(x.min(63.0) as i32, y.min(63.0) as i32);
            assert!(c.is_some());
        }
        // Места граней не пересекаются.
        let rects = uv::occupied(&doc, 0, &[]);
        assert_eq!(rects.len(), 6);
    }
}
