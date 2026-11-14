//! Сборка треугольного меша с общим атласом из сцены [`Model`] (и из вокселей / `.ply`).
//!
//! Все текстуры модели складываются в один атлас столбиком (по номеру текстуры), стороны
//! без текстуры берут белый блок 2×2 внизу атласа. Почему один атлас: рендерер движка
//! рисует меш-ассет одной встроенной текстурой (как `.pmodel`); части по текстурам
//! ([`Options::merge_by_texture`] = `false`) остаются отдельными диапазонами индексов,
//! чтобы им можно было назначить свои материалы.

use std::collections::BTreeMap;
use std::ops::Range;

use crate::image::RgbaImage;
use crate::math::{normalize, Affine, V3};
use crate::model::{Model, Node, Report, Side};
use crate::{limits, Error, Format};

/// Ось «вверх» в файле.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum UpAxis {
    Y,
    /// Z вверх (Blender, САПР): (x, y, z) → (x, z, −y).
    Z,
}

/// Настройки импорта (в движке — из `.meta`, см. `docs/api/model-formats.md`).
#[derive(Clone, Debug, PartialEq)]
pub struct Options {
    /// Множитель размера поверх единиц формата ([`Format::metres_per_unit`]).
    pub scale: f32,
    /// Ось «вверх» в файле; `None` — по умолчанию формата ([`Format::default_up`]).
    pub up: Option<UpAxis>,
    /// Отразить по X (обход треугольников сохраняется правильным).
    pub flip_x: bool,
    /// Применять повороты групп (костей). `false` — группы только объединяют.
    pub bake_groups: bool,
    /// Один диапазон индексов на весь меш. `false` — часть на каждую текстуру.
    pub merge_by_texture: bool,
    /// Добавить обратные стороны (для плоских элементов: трава, листья, стекло).
    pub double_sided: bool,
}

impl Default for Options {
    fn default() -> Self {
        Self { scale: 1.0, up: None, flip_x: false, bake_groups: true, merge_by_texture: true, double_sided: false }
    }
}

/// Часть меша: диапазон в [`TriMesh::indices`] и имя (текстура).
#[derive(Clone, Debug, PartialEq)]
pub struct Part {
    pub indices: Range<u32>,
    pub name: String,
}

/// Треугольный меш в метрах: Y вверх, обход против часовой стрелки снаружи, плоские
/// нормали, UV в атласе ((0, 0) — левый верхний угол).
#[derive(Clone, Debug, Default, PartialEq)]
pub struct TriMesh {
    pub positions: Vec<[f32; 3]>,
    pub normals: Vec<[f32; 3]>,
    pub uvs: Vec<[f32; 2]>,
    pub indices: Vec<u32>,
    /// Пусто — одна часть на весь меш.
    pub parts: Vec<Part>,
}

impl TriMesh {
    pub fn triangle_count(&self) -> usize {
        self.indices.len() / 3
    }

    /// Границы (минимум, максимум).
    pub fn bounds(&self) -> (V3, V3) {
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
}

/// Результат импорта для движка: меш, его атлас и отчёт.
#[derive(Clone, Debug, PartialEq)]
pub struct Imported {
    pub format: Format,
    pub mesh: TriMesh,
    /// Общий атлас (у `.stl` и `.ply` без цветов — `None`).
    pub texture: Option<RgbaImage>,
    pub report: Report,
}

/// Чем покрашен многоугольник.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub(crate) enum Slot {
    /// Текстура модели с этим номером.
    Texture(usize),
    /// Без текстуры — белый.
    White,
    /// Без атласа вообще (`.stl`): UV нули.
    None,
}

/// Многоугольники в единицах файла до перевода в метры.
#[derive(Default)]
pub(crate) struct Builder {
    points: Vec<V3>,
    uvs: Vec<[f32; 2]>,
    polygons: Vec<(u32, u32, Slot)>,
}

impl Builder {
    /// Добавить выпуклый многоугольник: углы против часовой стрелки снаружи, UV в долях
    /// своей текстуры. Вырожденный (нулевая площадь) пропускается.
    pub fn polygon(&mut self, points: &[V3], uvs: &[[f32; 2]], slot: Slot) -> Result<(), Error> {
        if points.len() < 3 {
            return Ok(());
        }
        if points.iter().flatten().any(|v| !v.is_finite() || v.abs() > limits::MAX_COORDINATE * 4.0) {
            return Err(Error("a vertex position is not a finite number in range".to_string()));
        }
        if normalize(newell(points)).is_none() {
            return Ok(());
        }
        if self.points.len() + points.len() > limits::MAX_VERTICES || self.triangles() + points.len() - 2 > limits::MAX_TRIANGLES {
            return Err(Error(format!(
                "the model has more than {} triangles; it is too large to import",
                limits::MAX_TRIANGLES
            )));
        }
        let start = self.points.len() as u32;
        self.points.extend_from_slice(points);
        for i in 0..points.len() {
            let uv = uvs.get(i).copied().unwrap_or([0.0, 0.0]);
            self.uvs.push(if uv[0].is_finite() && uv[1].is_finite() { uv } else { [0.0, 0.0] });
        }
        self.polygons.push((start, points.len() as u32, slot));
        Ok(())
    }

    fn triangles(&self) -> usize {
        self.polygons.iter().map(|p| p.1 as usize - 2).sum()
    }

    pub fn is_empty(&self) -> bool {
        self.polygons.is_empty()
    }
}

/// Нормаль Ньюэлла (длина — удвоенная площадь), для обхода против часовой стрелки.
fn newell(points: &[V3]) -> V3 {
    let mut n = [0.0f32; 3];
    for i in 0..points.len() {
        let (a, b) = (points[i], points[(i + 1) % points.len()]);
        n[0] += (a[1] - b[1]) * (a[2] + b[2]);
        n[1] += (a[2] - b[2]) * (a[0] + b[0]);
        n[2] += (a[0] - b[0]) * (a[1] + b[1]);
    }
    n
}

/// Картинка для слота атласа.
pub(crate) struct Sheet {
    pub name: String,
    pub image: RgbaImage,
}

/// Заметная замена ненайденной текстуре: пурпурно-чёрная шахматка размером `size`.
pub(crate) fn placeholder(size: [f32; 2]) -> RgbaImage {
    let w = (size[0].round() as u32).clamp(1, 256);
    let h = (size[1].round() as u32).clamp(1, 256);
    let mut image = RgbaImage::new(w, h, [0, 0, 0, 255]);
    for y in 0..h {
        for x in 0..w {
            if (x * 2 / w.max(2) + y * 2 / h.max(2)).is_multiple_of(2) {
                image.set(x, y, [255, 0, 220, 255]);
            }
        }
    }
    image
}

/// Перевести многоугольники в метры, собрать атлас из `sheets` (по номеру текстуры) и
/// треугольный меш.
pub(crate) fn finish(
    builder: Builder,
    sheets: &[Sheet],
    format: Format,
    offset: V3,
    options: &Options,
    report: Report,
) -> Result<Imported, Error> {
    if builder.is_empty() {
        return Err(Error("the model has no visible faces".to_string()));
    }
    let scale = options.scale;
    if !scale.is_finite() || scale <= 0.0 || scale > 1.0e4 {
        return Err(Error(format!("import setting `scale` must be a positive number (it is {scale})")));
    }
    let unit = format.metres_per_unit() * scale;
    let up = options.up.unwrap_or(format.default_up());
    let transform = |p: V3| -> V3 {
        let p = [(p[0] + offset[0]) * unit, (p[1] + offset[1]) * unit, (p[2] + offset[2]) * unit];
        let p = match up {
            UpAxis::Y => p,
            UpAxis::Z => [p[0], p[2], -p[1]],
        };
        if options.flip_x {
            [-p[0], p[1], p[2]]
        } else {
            p
        }
    };

    // Атлас: использованные текстуры столбиком, потом белый блок.
    let mut used: Vec<Slot> = builder.polygons.iter().map(|p| p.2).collect();
    used.sort();
    used.dedup();
    let mut placed: BTreeMap<Slot, ([u32; 2], [u32; 2])> = BTreeMap::new(); // слот → (угол, размер)
    let (mut width, mut height) = (0u32, 0u32);
    for slot in &used {
        let size = match slot {
            Slot::Texture(i) => sheets.get(*i).map_or([2, 2], |s| [s.image.width, s.image.height]),
            Slot::White => [2, 2],
            Slot::None => continue,
        };
        placed.insert(*slot, ([0, height], size));
        width = width.max(size[0]);
        height += size[1];
    }
    let texture = if placed.is_empty() {
        None
    } else {
        if width as u64 * height as u64 > limits::MAX_ATLAS_PIXELS {
            return Err(Error(format!(
                "the textures of the model need a {width}x{height} atlas; at most {} pixels are allowed",
                limits::MAX_ATLAS_PIXELS
            )));
        }
        let mut atlas = RgbaImage::new(width, height, [0, 0, 0, 0]);
        for (slot, (at, size)) in &placed {
            match slot {
                Slot::Texture(i) => match sheets.get(*i) {
                    Some(sheet) => atlas.blit(&sheet.image, at[0], at[1]),
                    None => atlas.blit(&RgbaImage::new(size[0], size[1], [255, 255, 255, 255]), at[0], at[1]),
                },
                _ => atlas.blit(&RgbaImage::new(size[0], size[1], [255, 255, 255, 255]), at[0], at[1]),
            }
        }
        Some(atlas)
    };
    let atlas_size = [width.max(1) as f32, height.max(1) as f32];
    let remap = |slot: Slot, uv: [f32; 2]| -> [f32; 2] {
        match (slot, placed.get(&slot)) {
            (Slot::White, Some((at, _))) => [1.0 / atlas_size[0], (at[1] as f32 + 1.0) / atlas_size[1]],
            (Slot::Texture(_), Some((at, size))) => [
                (at[0] as f32 + uv[0] * size[0] as f32) / atlas_size[0],
                (at[1] as f32 + uv[1] * size[1] as f32) / atlas_size[1],
            ],
            _ => [0.0, 0.0],
        }
    };

    // Порядок многоугольников: по текстуре, если части раздельные.
    let mut order: Vec<usize> = (0..builder.polygons.len()).collect();
    if !options.merge_by_texture {
        order.sort_by_key(|&i| builder.polygons[i].2);
    }
    let mut mesh = TriMesh::default();
    let mut current: Option<(Slot, u32)> = None;
    let part_name = |slot: Slot| match slot {
        Slot::Texture(i) => sheets.get(i).map_or_else(|| format!("texture {i}"), |s| s.name.clone()),
        Slot::White | Slot::None => "untextured".to_string(),
    };
    for &index in &order {
        let (start, count, slot) = builder.polygons[index];
        if !options.merge_by_texture && current.map(|c| c.0) != Some(slot) {
            if let Some((previous, begin)) = current {
                mesh.parts.push(Part { indices: begin..mesh.indices.len() as u32, name: part_name(previous) });
            }
            current = Some((slot, mesh.indices.len() as u32));
        }
        let range = start as usize..(start + count) as usize;
        let mut points: Vec<V3> = builder.points[range.clone()].iter().map(|p| transform(*p)).collect();
        let mut uvs: Vec<[f32; 2]> = builder.uvs[range].iter().map(|uv| remap(slot, *uv)).collect();
        if options.flip_x {
            points.reverse();
            uvs.reverse();
        }
        let sides: &[bool] = if options.double_sided { &[false, true] } else { &[false] };
        for &back in sides {
            let (points, uvs) = if back {
                (points.iter().rev().copied().collect::<Vec<_>>(), uvs.iter().rev().copied().collect::<Vec<_>>())
            } else {
                (points.clone(), uvs.clone())
            };
            let Some(normal) = normalize(newell(&points)) else { continue };
            let base = mesh.positions.len() as u32;
            for (p, uv) in points.iter().zip(&uvs) {
                mesh.positions.push(*p);
                mesh.normals.push(normal);
                mesh.uvs.push(*uv);
            }
            for i in 1..points.len() as u32 - 1 {
                mesh.indices.extend([base, base + i, base + i + 1]);
            }
        }
    }
    if let Some((slot, begin)) = current {
        mesh.parts.push(Part { indices: begin..mesh.indices.len() as u32, name: part_name(slot) });
    }
    if mesh.parts.len() == 1 {
        mesh.parts.clear();
    }
    Ok(Imported { format, mesh, texture, report })
}

/// Собрать меш сцены: кубы, свободные меши, повороты элементов и групп.
pub fn model_to_mesh(model: &Model, options: &Options) -> Result<Imported, Error> {
    let mut report = model.report.clone();
    let mut builder = Builder::default();
    walk(&model.roots, Affine::IDENTITY, model, options, &mut builder, &mut report, 0)?;
    let mut sheets = Vec::with_capacity(model.textures.len());
    for texture in &model.textures {
        let image = match &texture.image {
            Some(image) => image.clone(),
            None => placeholder(texture.uv_size),
        };
        sheets.push(Sheet { name: texture.name.clone(), image });
    }
    finish(builder, &sheets, model.format, model.offset, options, report)
}

fn walk(
    nodes: &[Node],
    parent: Affine,
    model: &Model,
    options: &Options,
    out: &mut Builder,
    report: &mut Report,
    depth: usize,
) -> Result<(), Error> {
    if depth > limits::MAX_DEPTH {
        return Err(Error(format!("groups are nested deeper than {} levels", limits::MAX_DEPTH)));
    }
    let uv_size = |texture: Option<usize>| -> [f32; 2] {
        texture.and_then(|i| model.textures.get(i)).map_or([1.0, 1.0], |t| t.uv_size)
    };
    let slot = |texture: Option<usize>| match texture {
        Some(i) if i < model.textures.len() => Slot::Texture(i),
        _ => Slot::White,
    };
    for node in nodes {
        match node {
            Node::Group(group) => {
                let matrix = if options.bake_groups {
                    parent.then_after(&Affine::pivot(group.origin, group.rotation))
                } else {
                    parent
                };
                walk(&group.children, matrix, model, options, out, report, depth + 1)?;
            }
            Node::Cube(cube) => {
                let mut local = Affine::pivot(cube.origin, cube.rotation);
                if cube.rescale {
                    let axes: Vec<usize> = (0..3).filter(|i| cube.rotation[*i] != 0.0).collect();
                    if let [axis] = axes[..] {
                        let k = 1.0 / cube.rotation[axis].to_radians().cos().abs().max(1e-3);
                        let mut s = [k; 3];
                        s[axis] = 1.0;
                        let o = cube.origin;
                        local = Affine::translation(o)
                            .then_after(&Affine::euler_degrees(cube.rotation))
                            .then_after(&Affine::scale(s))
                            .then_after(&Affine::translation([-o[0], -o[1], -o[2]]));
                    } else if axes.len() > 1 {
                        report.warn(format!("element `{}`: `rescale` works with a rotation around one axis only; ignored", cube.name));
                    }
                }
                let matrix = parent.then_after(&local);
                let lo: V3 = std::array::from_fn(|i| cube.from[i].min(cube.to[i]) - cube.inflate);
                let hi: V3 = std::array::from_fn(|i| cube.from[i].max(cube.to[i]) + cube.inflate);
                for side in Side::ALL {
                    let Some(face) = cube.faces[side.index()] else { continue };
                    let corners = side.corners(lo, hi).map(|p| matrix.apply(p));
                    let size = uv_size(face.texture);
                    let uvs = face.corner_uvs().map(|uv| [uv[0] / size[0], uv[1] / size[1]]);
                    // Углы по часовой стрелке снаружи → против: TL, BL, BR, TR.
                    out.polygon(
                        &[corners[0], corners[3], corners[2], corners[1]],
                        &[uvs[0], uvs[3], uvs[2], uvs[1]],
                        slot(face.texture),
                    )?;
                }
            }
            Node::Mesh(mesh) => {
                let matrix = parent
                    .then_after(&Affine::translation(mesh.origin))
                    .then_after(&Affine::euler_degrees(mesh.rotation));
                for face in &mesh.faces {
                    let mut points = Vec::with_capacity(face.vertices.len());
                    for &v in &face.vertices {
                        let p = mesh.vertices.get(v as usize).ok_or_else(|| {
                            Error(format!("mesh `{}`: a face uses vertex {v}, the mesh has {}", mesh.name, mesh.vertices.len()))
                        })?;
                        points.push(matrix.apply(*p));
                    }
                    let size = uv_size(face.texture);
                    let uvs: Vec<[f32; 2]> = face.uvs.iter().map(|uv| [uv[0] / size[0], uv[1] / size[1]]).collect();
                    out.polygon(&points, &uvs, slot(face.texture))?;
                }
            }
        }
    }
    Ok(())
}
