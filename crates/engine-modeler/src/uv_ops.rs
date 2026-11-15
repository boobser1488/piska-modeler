//! Правка UV мешей: чистые функции над документом, без интерфейса.
//!
//! UV граней меша лежат в **текселях** текстуры (угол текселя — целое число), поэтому всё
//! здесь считается в текселях и умеет «прижиматься» к целым (пиксельный редактор: углы UV
//! садятся на углы текселей).
//!
//! Что есть:
//! * выбор: углы граней ([`Corner`]), острова ([`islands`]) — грани, связанные общим ребром
//!   с совпадающими UV;
//! * преобразования выбранных углов: сдвиг, поворот, масштаб, отражение, прижатие к текселям;
//! * развёртки ([`unwrap`]): по граням, «коробкой», вдоль оси, из вида камеры;
//! * укладка островов ([`pack_islands`]), одна плотность текселей ([`scale_to_density`]),
//!   вынос граней в свой остров ([`split_to_island`]), склейка совпадающих UV ([`weld`]).
//!
//! Операции, которые переносят острова, переносят и их пиксели (нарисованное остаётся на
//! гранях); развёртки заново заливают острова цветом, который был у грани.

use std::collections::{BTreeMap, BTreeSet, HashMap};

use glam::{Vec2, Vec3};

use crate::doc::{Document, MeshFace, MeshShape, NodeId};
use crate::image::{Rgba, CLEAR};
use crate::{ops, uv};

/// Угол грани: (номер грани, номер угла в ней).
pub type Corner = (usize, usize);

const EPS: f32 = 1e-3;

// ----------------------------------------------------------------------
// Аффинное преобразование
// ----------------------------------------------------------------------

/// `p' = M·p + t`, `M = [[m0, m1], [m2, m3]]`.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Xf {
    pub m: [f32; 4],
    pub t: Vec2,
}

impl Xf {
    pub const IDENTITY: Xf = Xf { m: [1.0, 0.0, 0.0, 1.0], t: Vec2::ZERO };

    pub fn translate(t: Vec2) -> Xf {
        Xf { m: [1.0, 0.0, 0.0, 1.0], t }
    }

    pub fn scale(s: f32) -> Xf {
        Xf { m: [s, 0.0, 0.0, s], t: Vec2::ZERO }
    }

    /// Поворот на 90° по часовой стрелке на экране (ось Y вниз): `(x, y) → (−y, x)`.
    pub fn rot90() -> Xf {
        Xf { m: [0.0, -1.0, 1.0, 0.0], t: Vec2::ZERO }
    }

    pub fn apply(&self, p: Vec2) -> Vec2 {
        Vec2::new(self.m[0] * p.x + self.m[1] * p.y + self.t.x, self.m[2] * p.x + self.m[3] * p.y + self.t.y)
    }

    /// Сначала `self`, потом `next`.
    pub fn then(self, next: Xf) -> Xf {
        let (a, b) = (next.m, self.m);
        Xf {
            m: [a[0] * b[0] + a[1] * b[2], a[0] * b[1] + a[1] * b[3], a[2] * b[0] + a[3] * b[2], a[2] * b[1] + a[3] * b[3]],
            t: next.apply(self.t),
        }
    }

    pub fn inverse(&self) -> Option<Xf> {
        let det = self.m[0] * self.m[3] - self.m[1] * self.m[2];
        if det.abs() < 1e-9 {
            return None;
        }
        let m = [self.m[3] / det, -self.m[1] / det, -self.m[2] / det, self.m[0] / det];
        let inv = Xf { m, t: Vec2::ZERO };
        Some(Xf { m, t: -inv.apply(self.t) })
    }
}

// ----------------------------------------------------------------------
// Углы и границы
// ----------------------------------------------------------------------

/// Все углы граней `faces`.
pub fn corners_of_faces(shape: &MeshShape, faces: &BTreeSet<usize>) -> BTreeSet<Corner> {
    let mut out = BTreeSet::new();
    for &f in faces {
        if let Some(face) = shape.faces.get(f) {
            for k in 0..face.uvs.len() {
                out.insert((f, k));
            }
        }
    }
    out
}

/// Грани, которым принадлежат углы.
pub fn faces_of(corners: &BTreeSet<Corner>) -> BTreeSet<usize> {
    corners.iter().map(|c| c.0).collect()
}

pub fn uv_of(shape: &MeshShape, corner: Corner) -> Option<Vec2> {
    shape.faces.get(corner.0)?.uvs.get(corner.1).map(|p| Vec2::from(*p))
}

/// Границы UV углов: (min, max).
pub fn bounds(shape: &MeshShape, corners: &BTreeSet<Corner>) -> Option<(Vec2, Vec2)> {
    let mut lo = Vec2::splat(f32::INFINITY);
    let mut hi = Vec2::splat(f32::NEG_INFINITY);
    let mut any = false;
    for c in corners {
        if let Some(p) = uv_of(shape, *c) {
            lo = lo.min(p);
            hi = hi.max(p);
            any = true;
        }
    }
    any.then_some((lo, hi))
}

pub fn center(shape: &MeshShape, corners: &BTreeSet<Corner>) -> Option<Vec2> {
    bounds(shape, corners).map(|(lo, hi)| (lo + hi) * 0.5)
}

fn faces_bounds(shape: &MeshShape, faces: &[usize]) -> Option<(Vec2, Vec2)> {
    let set: BTreeSet<usize> = faces.iter().copied().collect();
    bounds(shape, &corners_of_faces(shape, &set))
}

// ----------------------------------------------------------------------
// Преобразования выбранных углов
// ----------------------------------------------------------------------

/// Заменить UV каждого выбранного угла.
pub fn map_corners(shape: &mut MeshShape, corners: &BTreeSet<Corner>, f: impl Fn(Vec2) -> Vec2) {
    for &(fi, k) in corners {
        if let Some(uv) = shape.faces.get_mut(fi).and_then(|face| face.uvs.get_mut(k)) {
            let p = f(Vec2::from(*uv));
            *uv = [p.x, p.y];
        }
    }
}

/// Сдвинуть на `delta`; с `snap` левый верхний угол границ садится на угол текселя.
/// Возвращает сделанный сдвиг.
pub fn move_corners(shape: &mut MeshShape, corners: &BTreeSet<Corner>, delta: Vec2, snap: bool) -> Vec2 {
    let Some((lo, _)) = bounds(shape, corners) else { return Vec2::ZERO };
    let delta = if snap { (lo + delta).round() - lo } else { delta };
    map_corners(shape, corners, |p| p + delta);
    delta
}

fn snap_origin(shape: &mut MeshShape, corners: &BTreeSet<Corner>) {
    if let Some((lo, _)) = bounds(shape, corners) {
        let d = lo.round() - lo;
        if d != Vec2::ZERO {
            map_corners(shape, corners, |p| p + d);
        }
    }
}

/// Повернуть на `radians` по часовой стрелке (на экране) вокруг `pivot`.
pub fn rotate_corners(shape: &mut MeshShape, corners: &BTreeSet<Corner>, pivot: Vec2, radians: f32, snap: bool) {
    let (s, c) = radians.sin_cos();
    map_corners(shape, corners, |p| {
        let d = p - pivot;
        pivot + Vec2::new(c * d.x - s * d.y, s * d.x + c * d.y)
    });
    if snap {
        snap_origin(shape, corners);
    }
}

/// Повернуть на `quarters` четвертей оборота (по часовой стрелке) вокруг центра границ;
/// результат ровно на целых (синус и косинус — точные 0 и ±1).
pub fn rotate_quarters(shape: &mut MeshShape, corners: &BTreeSet<Corner>, quarters: i32, snap: bool) {
    let Some(c) = center(shape, corners) else { return };
    let q = quarters.rem_euclid(4);
    map_corners(shape, corners, |p| {
        let d = p - c;
        c + match q {
            1 => Vec2::new(-d.y, d.x),
            2 => Vec2::new(-d.x, -d.y),
            3 => Vec2::new(d.y, -d.x),
            _ => d,
        }
    });
    if snap {
        snap_origin(shape, corners);
    }
}

/// Масштаб вокруг `pivot`; с `snap` границы после него — целое число текселей (левый
/// верхний угол на углу текселя, размер не меньше 1).
pub fn scale_corners(shape: &mut MeshShape, corners: &BTreeSet<Corner>, pivot: Vec2, s: Vec2, snap: bool) {
    map_corners(shape, corners, |p| pivot + (p - pivot) * s);
    if !snap {
        return;
    }
    let Some((lo, hi)) = bounds(shape, corners) else { return };
    let size = hi - lo;
    let new_lo = lo.round();
    let fit = |a: f32| if a > 1e-6 { a.round().max(1.0) / a } else { 1.0 };
    let k = Vec2::new(fit(size.x), fit(size.y));
    // Целые остаются целыми (погрешность умножения не должна оставлять 5.9999995).
    let clean = |v: f32| if (v - v.round()).abs() < 1e-3 { v.round() } else { v };
    map_corners(shape, corners, |p| {
        let q = new_lo + (p - lo) * k;
        Vec2::new(clean(q.x), clean(q.y))
    });
}

/// Отразить по горизонтали (`vertical == false`) или вертикали вокруг центра границ.
pub fn flip_corners(shape: &mut MeshShape, corners: &BTreeSet<Corner>, vertical: bool) {
    let Some((lo, hi)) = bounds(shape, corners) else { return };
    map_corners(shape, corners, |p| if vertical { Vec2::new(p.x, lo.y + hi.y - p.y) } else { Vec2::new(lo.x + hi.x - p.x, p.y) });
}

/// Прижать каждый выбранный угол к углу текселя.
pub fn snap_corners(shape: &mut MeshShape, corners: &BTreeSet<Corner>) {
    map_corners(shape, corners, Vec2::round);
}

// ----------------------------------------------------------------------
// Острова
// ----------------------------------------------------------------------

fn near(a: Vec2, b: Vec2) -> bool {
    (a - b).abs().max_element() < EPS
}

/// Острова UV: грани, у которых общее ребро и совпадают UV его концов (и текстура).
/// `only` — рассматривать только эти грани. Острова упорядочены по наименьшей грани.
pub fn islands(shape: &MeshShape, only: Option<&BTreeSet<usize>>) -> Vec<Vec<usize>> {
    let n = shape.faces.len();
    let mut parent: Vec<usize> = (0..n).collect();
    fn find(parent: &mut [usize], mut x: usize) -> usize {
        while parent[x] != x {
            parent[x] = parent[parent[x]];
            x = parent[x];
        }
        x
    }
    type Side = (usize, Vec2, Vec2);
    let mut edges: HashMap<(u32, u32), Vec<Side>> = HashMap::new();
    for (fi, face) in shape.faces.iter().enumerate() {
        if only.is_some_and(|o| !o.contains(&fi)) {
            continue;
        }
        let count = face.vertices.len();
        for k in 0..count {
            let (a, b) = (face.vertices[k], face.vertices[(k + 1) % count]);
            let (Some(ua), Some(ub)) = (face.uvs.get(k), face.uvs.get((k + 1) % count)) else { continue };
            if a == b {
                continue;
            }
            let (ua, ub) = (Vec2::from(*ua), Vec2::from(*ub));
            if a < b {
                edges.entry((a, b)).or_default().push((fi, ua, ub));
            } else {
                edges.entry((b, a)).or_default().push((fi, ub, ua));
            }
        }
    }
    for list in edges.values() {
        for i in 0..list.len() {
            for j in i + 1..list.len() {
                let (f, g) = (list[i], list[j]);
                if shape.faces[f.0].texture == shape.faces[g.0].texture && near(f.1, g.1) && near(f.2, g.2) {
                    let (ra, rb) = (find(&mut parent, f.0), find(&mut parent, g.0));
                    if ra != rb {
                        parent[ra.max(rb)] = ra.min(rb);
                    }
                }
            }
        }
    }
    let mut groups: BTreeMap<usize, Vec<usize>> = BTreeMap::new();
    for fi in 0..n {
        if only.is_some_and(|o| !o.contains(&fi)) {
            continue;
        }
        let root = find(&mut parent, fi);
        groups.entry(root).or_default().push(fi);
    }
    groups.into_values().collect()
}

/// Остров, в который входит грань.
pub fn island_of(shape: &MeshShape, face: usize) -> Vec<usize> {
    islands(shape, None).into_iter().find(|i| i.contains(&face)).unwrap_or_default()
}

/// Грани, связанные общим ребром (по номерам вершин) — для проекции «коробкой».
fn components(shape: &MeshShape, faces: &[usize]) -> Vec<Vec<usize>> {
    let mut by_edge: HashMap<(u32, u32), Vec<usize>> = HashMap::new();
    for &f in faces {
        let v = &shape.faces[f].vertices;
        for k in 0..v.len() {
            let (a, b) = (v[k], v[(k + 1) % v.len()]);
            by_edge.entry((a.min(b), a.max(b))).or_default().push(f);
        }
    }
    let mut seen: BTreeSet<usize> = BTreeSet::new();
    let mut out = Vec::new();
    for &start in faces {
        if !seen.insert(start) {
            continue;
        }
        let mut stack = vec![start];
        let mut group = vec![start];
        while let Some(f) = stack.pop() {
            let v = &shape.faces[f].vertices;
            for k in 0..v.len() {
                let (a, b) = (v[k], v[(k + 1) % v.len()]);
                for &g in by_edge.get(&(a.min(b), a.max(b))).into_iter().flatten() {
                    if seen.insert(g) {
                        group.push(g);
                        stack.push(g);
                    }
                }
            }
        }
        group.sort_unstable();
        out.push(group);
    }
    out
}

// ----------------------------------------------------------------------
// Плотность
// ----------------------------------------------------------------------

/// Плотность текселей граней: текселей на единицу модели (корень из площади UV к площади
/// в пространстве модели); 0 — у вырожденных.
pub fn density_of(points: &[Vec3], shape: &MeshShape, faces: &[usize]) -> f32 {
    let (mut uv_area, mut area) = (0.0f32, 0.0f32);
    for &f in faces {
        let Some(face) = shape.faces.get(f) else { continue };
        let corners: Vec<Vec3> = face.vertices.iter().map(|v| points.get(*v as usize).copied().unwrap_or(Vec3::ZERO)).collect();
        area += crate::mesh_ops::newell(&corners).length() * 0.5;
        let n = face.uvs.len();
        let mut a = 0.0;
        for i in 0..n {
            let (p, q) = (face.uvs[i], face.uvs[(i + 1) % n]);
            a += p[0] * q[1] - q[0] * p[1];
        }
        uv_area += a.abs() * 0.5;
    }
    if area < 1e-9 {
        0.0
    } else {
        (uv_area / area).sqrt()
    }
}

/// Перевод «пикселей на метр» в тексели на единицу модели (`units_per_metre` единиц = 1 м).
pub fn texels_per_unit(px_per_metre: f32, units_per_metre: f32) -> f32 {
    px_per_metre / units_per_metre.max(1e-6)
}

// ----------------------------------------------------------------------
// Укладка островов в атлас
// ----------------------------------------------------------------------

/// Параметры укладки и развёртки.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Pack {
    /// Зазор между островами, текселей.
    pub margin: u32,
    /// Разрешить поворот островов на 90° (длинной стороной по горизонтали).
    pub rotate: bool,
    /// Размеры островов — целые тексели.
    pub snap: bool,
}

impl Default for Pack {
    fn default() -> Self {
        Self { margin: 1, rotate: false, snap: true }
    }
}

enum Content {
    /// Залить цветом (по цвету на грань).
    Fill(Vec<Rgba>),
    /// Перенести пиксели: `Xf` — из старых UV в местные (от угла острова).
    Move(Xf),
}

struct Island {
    faces: Vec<usize>,
    texture: u32,
    /// Местные UV (от левого верхнего угла острова) для каждого угла каждой грани.
    local: Vec<Vec<[f32; 2]>>,
    size: [u32; 2],
    content: Content,
}

impl Island {
    /// Повернуть на 90° по часовой стрелке: `(x, y) → (h − y, x)`.
    fn rotated(mut self) -> Island {
        let h = self.size[1] as f32;
        for face in &mut self.local {
            for p in face {
                *p = [h - p[1], p[0]];
            }
        }
        self.size = [self.size[1], self.size[0]];
        if let Content::Move(xf) = &mut self.content {
            *xf = xf.then(Xf { m: [0.0, -1.0, 1.0, 0.0], t: Vec2::new(h, 0.0) });
        }
        self
    }
}

/// Остров из граней, перенесённых преобразованием `xf` (пиксели едут следом).
fn island_moved(shape: &MeshShape, faces: Vec<usize>, xf: Xf, round: bool) -> Island {
    let mut lo = Vec2::splat(f32::INFINITY);
    let mut hi = Vec2::splat(f32::NEG_INFINITY);
    for &f in &faces {
        for uv in &shape.faces[f].uvs {
            let p = xf.apply(Vec2::from(*uv));
            lo = lo.min(p);
            hi = hi.max(p);
        }
    }
    if !lo.is_finite() {
        lo = Vec2::ZERO;
        hi = Vec2::ONE;
    }
    let origin = lo.floor();
    let m = xf.then(Xf::translate(-origin));
    let local = faces
        .iter()
        .map(|&f| {
            shape.faces[f]
                .uvs
                .iter()
                .map(|uv| {
                    let p = m.apply(Vec2::from(*uv));
                    let p = if round { p.round() } else { p };
                    [p.x, p.y]
                })
                .collect()
        })
        .collect();
    let size = ((hi - origin) - Vec2::splat(EPS)).ceil().max(Vec2::ONE);
    Island { texture: shape.faces[faces[0]].texture, faces, local, size: [size.x as u32, size.y as u32], content: Content::Move(m) }
}

/// Остров из новых координат углов `coords` (в текселях, любое начало), залитый цветами.
fn island_projected(shape: &MeshShape, faces: Vec<usize>, coords: Vec<Vec<Vec2>>, colors: Vec<Rgba>, snap: bool) -> Island {
    let mut lo = Vec2::splat(f32::INFINITY);
    let mut hi = Vec2::splat(f32::NEG_INFINITY);
    for p in coords.iter().flatten() {
        lo = lo.min(*p);
        hi = hi.max(*p);
    }
    let raw = (hi - lo).max(Vec2::ZERO);
    let (size, k) = if snap {
        let s = raw.round().max(Vec2::ONE);
        (s, Vec2::new(if raw.x > 1e-6 { s.x / raw.x } else { 1.0 }, if raw.y > 1e-6 { s.y / raw.y } else { 1.0 }))
    } else {
        ((raw - Vec2::splat(EPS)).ceil().max(Vec2::ONE), Vec2::ONE)
    };
    let local = coords.iter().map(|face| face.iter().map(|p| {
        let q = (*p - lo) * k;
        [q.x, q.y]
    }).collect()).collect();
    Island { texture: shape.faces[faces[0]].texture, faces, local, size: [size.x as u32, size.y as u32], content: Content::Fill(colors) }
}

/// Цвет грани сейчас: тексель в центре её UV (без UV или в пустоте — основа модели).
fn face_color(doc: &Document, face: &MeshFace) -> Rgba {
    if face.uvs.is_empty() {
        return ops::BASE_COLOR;
    }
    let c = face.uvs.iter().fold(Vec2::ZERO, |s, uv| s + Vec2::from(*uv)) / face.uvs.len() as f32;
    match doc.textures.get(face.texture as usize).and_then(|t| t.image.get(c.x.floor() as i32, c.y.floor() as i32)) {
        Some(c) if c[3] > 0 => c,
        _ => ops::BASE_COLOR,
    }
}

/// Прямоугольник текселей, который занимают UV грани (если она размечена).
fn face_rect(face: &MeshFace) -> Option<uv::Rect> {
    let (mut lo, mut hi) = ([f32::INFINITY; 2], [f32::NEG_INFINITY; 2]);
    for uv in &face.uvs {
        for k in 0..2 {
            lo[k] = lo[k].min(uv[k]);
            hi[k] = hi[k].max(uv[k]);
        }
    }
    if lo[0] <= hi[0] && hi[0] > 0.0 {
        let (x0, y0) = (lo[0].floor().max(0.0) as u32, lo[1].floor().max(0.0) as u32);
        let (x1, y1) = (hi[0].ceil().max(0.0) as u32, hi[1].ceil().max(0.0) as u32);
        Some([x0, y0, (x1 - x0).max(1), (y1 - y0).max(1)])
    } else {
        None
    }
}

fn in_rect(r: &uv::Rect, x: u32, y: u32) -> bool {
    x >= r[0] && y >= r[1] && x < r[0] + r[2] && y < r[1] + r[3]
}

fn inside_poly(poly: &[Vec2], p: Vec2) -> bool {
    let mut inside = false;
    for i in 0..poly.len() {
        let (a, b) = (poly[i], poly[(i + 1) % poly.len()]);
        if (a.y > p.y) != (b.y > p.y) && p.x < a.x + (p.y - a.y) / (b.y - a.y) * (b.x - a.x) {
            inside = !inside;
        }
    }
    inside
}

/// Полки: прямоугольники по убыванию высоты на холст, начиная со строки `start`.
fn shelf(sizes: &[[u32; 2]], canvas: [u32; 2], start: u32) -> Option<Vec<[u32; 2]>> {
    let mut order: Vec<usize> = (0..sizes.len()).collect();
    order.sort_by(|a, b| sizes[*b][1].cmp(&sizes[*a][1]).then(a.cmp(b)));
    let mut out = vec![[0u32; 2]; sizes.len()];
    let (mut x, mut y, mut row) = (0u32, start, 0u32);
    for i in order {
        let [w, h] = sizes[i];
        if w > canvas[0] {
            return None;
        }
        if x + w > canvas[0] {
            x = 0;
            y += row;
            row = 0;
        }
        if y + h > canvas[1] {
            return None;
        }
        out[i] = [x, y];
        x += w;
        row = row.max(h);
    }
    Some(out)
}

/// Положить острова в атлас (холст растёт, если места нет), записать UV и пиксели.
fn place(doc: &mut Document, id: NodeId, islands: Vec<Island>, pack: Pack) -> Result<usize, String> {
    let mut shape = doc.element(id).and_then(|e| e.mesh.clone()).ok_or("the selection is not a mesh")?;
    if islands.is_empty() {
        return Ok(0);
    }
    if doc.textures.is_empty() {
        ops::add_texture(doc, "atlas", crate::doc::DEFAULT_TEXTURE_SIZE, crate::doc::DEFAULT_TEXTURE_SIZE);
    }
    let last = doc.textures.len() as u32 - 1;
    let islands: Vec<Island> = islands
        .into_iter()
        .map(|mut i| {
            i.texture = i.texture.min(last);
            if pack.rotate && i.size[0] < i.size[1] {
                i = i.rotated();
            }
            i
        })
        .collect();
    let total: usize = islands.iter().map(|i| i.faces.len()).sum();
    let textures: BTreeSet<u32> = islands.iter().map(|i| i.texture).collect();
    for t in textures {
        let idx: Vec<usize> = (0..islands.len()).filter(|k| islands[*k].texture == t).collect();
        let moved: BTreeSet<usize> = idx.iter().flat_map(|k| islands[*k].faces.iter().copied()).collect();
        let old = doc.textures[t as usize].image.clone();
        let old_rects: Vec<uv::Rect> = moved.iter().filter_map(|f| face_rect(&shape.faces[*f])).collect();
        // Занятое: другие элементы и грани этого меша, которые не двигаются.
        let mut kept = uv::occupied(doc, t, &[id]);
        for (fi, f) in shape.faces.iter().enumerate() {
            if f.texture == t && !moved.contains(&fi) {
                kept.extend(face_rect(f));
            }
        }
        // Зазор и до уже занятого: чужие места «толще» на зазор.
        let mut taken: Vec<uv::Rect> = kept.iter().map(|r| [r[0], r[1], r[2] + pack.margin, r[3] + pack.margin]).collect();
        let pad = |k: usize| [islands[k].size[0] + pack.margin, islands[k].size[1] + pack.margin];
        let mut origins: BTreeMap<usize, [u32; 2]> = BTreeMap::new();
        if idx.len() <= 64 {
            let mut order = idx.clone();
            order.sort_by(|a, b| islands[*b].size[1].cmp(&islands[*a].size[1]).then(a.cmp(b)));
            for k in order {
                let size = pad(k);
                let origin = loop {
                    let canvas = doc.textures[t as usize].image.size();
                    if let Some(o) = uv::find_free(&taken, canvas, size) {
                        break o;
                    }
                    if !uv::grow_texture(doc, t) {
                        return Err(format!("there is no room on the {0}x{0} texture; add a larger texture or reduce the texel density", uv::MAX_TEXTURE_SIZE));
                    }
                };
                taken.push([origin[0], origin[1], size[0], size[1]]);
                origins.insert(k, origin);
            }
        } else {
            let sizes: Vec<[u32; 2]> = idx.iter().map(|k| pad(*k)).collect();
            let start = taken.iter().map(|r| r[1] + r[3]).max().unwrap_or(0);
            loop {
                let canvas = doc.textures[t as usize].image.size();
                if let Some(places) = shelf(&sizes, canvas, start) {
                    for (k, o) in idx.iter().zip(places) {
                        origins.insert(*k, o);
                    }
                    break;
                }
                if !uv::grow_texture(doc, t) {
                    return Err(format!("{} islands do not fit on a {1}x{1} texture; reduce the texel density", idx.len(), uv::MAX_TEXTURE_SIZE));
                }
            }
        }
        let mut image = doc.textures[t as usize].image.clone();
        // Старые места граней освобождаются (кроме занятых другими).
        for r in &old_rects {
            for y in r[1]..r[1] + r[3] {
                for x in r[0]..r[0] + r[2] {
                    if !kept.iter().any(|k| in_rect(k, x, y)) {
                        image.set(x as i32, y as i32, CLEAR);
                    }
                }
            }
        }
        for &k in &idx {
            let island = &islands[k];
            let [ox, oy] = origins[&k];
            match &island.content {
                Content::Move(xf) => {
                    if let Some(inv) = xf.inverse() {
                        for y in 0..island.size[1] {
                            for x in 0..island.size[0] {
                                let p = inv.apply(Vec2::new(x as f32 + 0.5, y as f32 + 0.5));
                                if let Some(c) = old.get(p.x.floor() as i32, p.y.floor() as i32) {
                                    image.set((ox + x) as i32, (oy + y) as i32, c);
                                }
                            }
                        }
                    }
                }
                Content::Fill(colors) => {
                    let base = colors.first().copied().unwrap_or(ops::BASE_COLOR);
                    for y in 0..island.size[1] {
                        for x in 0..island.size[0] {
                            image.set((ox + x) as i32, (oy + y) as i32, base);
                        }
                    }
                    for (face, color) in island.local.iter().zip(colors) {
                        let poly: Vec<Vec2> = face.iter().map(|p| Vec2::from(*p)).collect();
                        if poly.len() < 3 {
                            continue;
                        }
                        let lo = poly.iter().fold(Vec2::splat(f32::INFINITY), |m, p| m.min(*p)).floor().max(Vec2::ZERO);
                        let hi = poly.iter().fold(Vec2::splat(f32::NEG_INFINITY), |m, p| m.max(*p)).ceil();
                        for y in lo.y as i32..hi.y as i32 {
                            for x in lo.x as i32..hi.x as i32 {
                                if inside_poly(&poly, Vec2::new(x as f32 + 0.5, y as f32 + 0.5)) {
                                    image.set(ox as i32 + x, oy as i32 + y, *color);
                                }
                            }
                        }
                    }
                }
            }
            for (f, local) in island.faces.iter().zip(&island.local) {
                shape.faces[*f].uvs = local.iter().map(|p| [p[0] + ox as f32, p[1] + oy as f32]).collect();
            }
        }
        doc.textures[t as usize].image = image;
    }
    if let Some(mesh) = doc.element_mut(id).and_then(|e| e.mesh.as_mut()) {
        mesh.faces = shape.faces;
    }
    ops::sync_twins(doc, &[id]);
    Ok(total)
}

fn mesh_of(doc: &Document, id: NodeId) -> Result<(MeshShape, Vec<Vec3>), String> {
    let element = doc.element(id).ok_or("the selection is not a mesh")?;
    let shape = element.mesh.clone().ok_or("the selection is not a mesh")?;
    Ok((shape, element.mesh_points()))
}

fn face_set(shape: &MeshShape, faces: &BTreeSet<usize>) -> Vec<usize> {
    if faces.is_empty() {
        (0..shape.faces.len()).collect()
    } else {
        faces.iter().copied().filter(|f| *f < shape.faces.len()).collect()
    }
}

/// Уложить острова граней `faces` (пусто — всех) в атлас: пиксели едут следом.
pub fn pack_islands(doc: &mut Document, id: NodeId, faces: &BTreeSet<usize>, pack: Pack) -> Result<usize, String> {
    let (shape, _) = mesh_of(doc, id)?;
    let wanted = face_set(&shape, faces);
    if wanted.is_empty() {
        return Err("the mesh has no faces".to_string());
    }
    let only: BTreeSet<usize> = wanted.iter().copied().collect();
    let list: Vec<Island> = islands(&shape, Some(&only)).into_iter().map(|g| island_moved(&shape, g, Xf::IDENTITY, false)).collect();
    place(doc, id, list, pack)
}

/// Грани `faces` — в один новый остров (их взаимное расположение сохраняется, связь с
/// остальными по UV рвётся); пиксели едут следом.
pub fn split_to_island(doc: &mut Document, id: NodeId, faces: &BTreeSet<usize>, pack: Pack) -> Result<usize, String> {
    let (shape, _) = mesh_of(doc, id)?;
    let wanted: Vec<usize> = faces.iter().copied().filter(|f| *f < shape.faces.len()).collect();
    if wanted.is_empty() {
        return Err("select faces first".to_string());
    }
    let island = island_moved(&shape, wanted, Xf::IDENTITY, false);
    place(doc, id, vec![island], pack)
}

/// Одна плотность текселей `density` (текселей на единицу модели) для островов граней
/// `faces` (пусто — всех): остров растягивается как целое, пиксели масштабируются
/// «ближайшим соседом» и едут следом.
pub fn scale_to_density(doc: &mut Document, id: NodeId, faces: &BTreeSet<usize>, density: f32, pack: Pack) -> Result<usize, String> {
    let (shape, points) = mesh_of(doc, id)?;
    let wanted = face_set(&shape, faces);
    let only: BTreeSet<usize> = wanted.iter().copied().collect();
    let target = density.clamp(0.01, 256.0);
    let mut list = Vec::new();
    for group in islands(&shape, Some(&only)) {
        let current = density_of(&points, &shape, &group);
        let factor = if current < 1e-6 { 1.0 } else { target / current };
        let Some((lo, _)) = faces_bounds(&shape, &group) else { continue };
        // Масштаб вокруг угла острова, чтобы он не уезжал.
        let xf = Xf::translate(-lo).then(Xf::scale(factor)).then(Xf::translate(lo * factor));
        list.push(island_moved(&shape, group, xf, pack.snap));
    }
    place(doc, id, list, pack)
}

// ----------------------------------------------------------------------
// Развёртки
// ----------------------------------------------------------------------

/// Способ развёртки.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Method {
    /// Каждая грань — свой остров в настоящую величину (в своей плоскости).
    ByFace,
    /// Проекция «кубом»: грань идёт на плоскость главной оси нормали; связные грани одной
    /// стороны образуют остров.
    Box,
    /// Одна плоская проекция всех граней вдоль оси (0 — X, 1 — Y, 2 — Z), как со стороны +ось.
    Axis(usize),
    /// Одна плоская проекция из вида: `right` и `up` — оси картинки в пространстве меша.
    View { right: Vec3, up: Vec3 },
}

#[derive(Clone, Copy)]
struct Basis {
    right: Vec3,
    up: Vec3,
}

impl Basis {
    /// Вид снаружи на плоскость с нормалью `n`: картинка не зеркальная, у боков «верх» — +Y.
    fn for_normal(n: Vec3) -> Basis {
        let n = n.normalize_or_zero();
        if n == Vec3::ZERO {
            return Basis { right: Vec3::X, up: Vec3::Y };
        }
        let reference = if n.y.abs() < 0.9 {
            Vec3::Y
        } else if n.y > 0.0 {
            -Vec3::Z
        } else {
            Vec3::Z
        };
        let up = (reference - n * n.dot(reference)).normalize_or_zero();
        Basis { right: up.cross(n).normalize_or_zero(), up }
    }

    fn project(&self, p: Vec3, density: f32) -> Vec2 {
        Vec2::new(p.dot(self.right), -p.dot(self.up)) * density
    }
}

/// Развернуть грани `faces` (пусто — все) выбранным способом с плотностью `density`
/// (текселей на единицу модели) и уложить острова в свободные места атласа. Грани
/// заливаются цветом, который у них был.
pub fn unwrap(doc: &mut Document, id: NodeId, faces: &BTreeSet<usize>, method: Method, density: f32, pack: Pack) -> Result<usize, String> {
    let (shape, points) = mesh_of(doc, id)?;
    let wanted = face_set(&shape, faces);
    if wanted.is_empty() {
        return Err("the mesh has no faces".to_string());
    }
    let density = density.clamp(0.01, 256.0);
    let normal = |f: usize| {
        let corners: Vec<Vec3> = shape.faces[f].vertices.iter().map(|v| points.get(*v as usize).copied().unwrap_or(Vec3::ZERO)).collect();
        crate::mesh_ops::newell(&corners).normalize_or_zero()
    };
    let mut groups: Vec<(Vec<usize>, Option<Basis>)> = Vec::new();
    match method {
        Method::ByFace => groups.extend(wanted.iter().map(|f| (vec![*f], None))),
        Method::Box => {
            let mut classes: BTreeMap<(usize, bool), Vec<usize>> = BTreeMap::new();
            for &f in &wanted {
                let n = normal(f);
                let a = n.abs();
                let axis = if a.x >= a.y && a.x >= a.z { 0 } else if a.y >= a.z { 1 } else { 2 };
                classes.entry((axis, n[axis] >= 0.0)).or_default().push(f);
            }
            for ((axis, positive), list) in classes {
                let mut n = Vec3::ZERO;
                n[axis] = if positive { 1.0 } else { -1.0 };
                for group in components(&shape, &list) {
                    groups.push((group, Some(Basis::for_normal(n))));
                }
            }
        }
        Method::Axis(axis) => {
            let mut n = Vec3::ZERO;
            n[axis.min(2)] = 1.0;
            groups.push((wanted.clone(), Some(Basis::for_normal(n))));
        }
        Method::View { right, up } => {
            let (right, up) = (right.normalize_or_zero(), up.normalize_or_zero());
            if right == Vec3::ZERO || up == Vec3::ZERO {
                return Err("the view has no direction".to_string());
            }
            groups.push((wanted.clone(), Some(Basis { right, up })));
        }
    }
    let mut list = Vec::with_capacity(groups.len());
    for (group, basis) in groups {
        let coords: Vec<Vec<Vec2>> = group
            .iter()
            .map(|&f| {
                let b = basis.unwrap_or_else(|| Basis::for_normal(normal(f)));
                shape.faces[f].vertices.iter().map(|v| b.project(points.get(*v as usize).copied().unwrap_or(Vec3::ZERO), density)).collect()
            })
            .collect();
        let colors: Vec<Rgba> = group.iter().map(|f| face_color(doc, &shape.faces[*f])).collect();
        list.push(island_projected(&shape, group, coords, colors, pack.snap));
    }
    place(doc, id, list, pack)
}

// ----------------------------------------------------------------------
// Склейка
// ----------------------------------------------------------------------

/// Склеить углы граней `faces` (пусто — всех), которые стоят в одной вершине модели и
/// лежат ближе `tolerance` текселей друг к другу: все получают среднюю точку. Возвращает,
/// сколько углов сдвинулось.
pub fn weld(doc: &mut Document, id: NodeId, faces: &BTreeSet<usize>, tolerance: f32) -> usize {
    let Some(shape) = doc.element_mut(id).and_then(|e| e.mesh.as_mut()) else { return 0 };
    let wanted = face_set(shape, faces);
    let mut by_vertex: BTreeMap<u32, Vec<Corner>> = BTreeMap::new();
    for &f in &wanted {
        for (k, v) in shape.faces[f].vertices.iter().enumerate() {
            if k < shape.faces[f].uvs.len() {
                by_vertex.entry(*v).or_default().push((f, k));
            }
        }
    }
    let mut changed = 0;
    for list in by_vertex.values() {
        let mut left: Vec<Corner> = list.clone();
        while let Some(first) = left.first().copied() {
            let base = Vec2::from(shape.faces[first.0].uvs[first.1]);
            let tex = shape.faces[first.0].texture;
            let (cluster, rest): (Vec<Corner>, Vec<Corner>) = left
                .into_iter()
                .partition(|c| shape.faces[c.0].texture == tex && (Vec2::from(shape.faces[c.0].uvs[c.1]) - base).abs().max_element() <= tolerance);
            left = rest;
            if cluster.len() < 2 {
                continue;
            }
            let mean = cluster.iter().fold(Vec2::ZERO, |s, c| s + Vec2::from(shape.faces[c.0].uvs[c.1])) / cluster.len() as f32;
            for c in cluster {
                let uv = &mut shape.faces[c.0].uvs[c.1];
                if (Vec2::from(*uv) - mean).abs().max_element() > 1e-6 {
                    *uv = [mean.x, mean.y];
                    changed += 1;
                }
            }
        }
    }
    if changed > 0 {
        ops::sync_twins(doc, &[id]);
    }
    changed
}

/// Заменить UV углов в документе (для правки мышью: результат считается от снимка).
pub fn edit_shape(doc: &mut Document, id: NodeId, f: impl FnOnce(&mut MeshShape)) -> bool {
    let Some(shape) = doc.element_mut(id).and_then(|e| e.mesh.as_mut()) else { return false };
    f(shape);
    ops::sync_twins(doc, &[id]);
    true
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::doc::Element;
    use crate::mesh_ops;

    fn cube(doc: &mut Document, size: f32) -> NodeId {
        let m = mesh_ops::box_mesh(Vec3::ZERO, Vec3::splat(size));
        ops::add_element(doc, "m", Element::mesh(&m.points, m.faces), None)
    }

    fn shape(doc: &Document, id: NodeId) -> MeshShape {
        doc.element(id).unwrap().mesh.clone().unwrap()
    }

    fn rect_of(shape: &MeshShape, f: usize) -> [f32; 4] {
        let (lo, hi) = faces_bounds(shape, &[f]).unwrap();
        [lo.x, lo.y, hi.x, hi.y]
    }

    fn no_margin() -> Pack {
        Pack { margin: 0, rotate: false, snap: true }
    }

    #[test]
    fn xf_composes_and_inverts() {
        let a = Xf::rot90().then(Xf::translate(Vec2::new(4.0, 0.0)));
        assert_eq!(a.apply(Vec2::new(1.0, 2.0)), Vec2::new(2.0, 1.0));
        let inv = a.inverse().unwrap();
        assert_eq!(inv.apply(Vec2::new(2.0, 1.0)), Vec2::new(1.0, 2.0));
        let s = Xf::scale(2.0).then(Xf::translate(Vec2::new(1.0, 1.0)));
        assert_eq!(s.inverse().unwrap().apply(s.apply(Vec2::new(3.0, 5.0))), Vec2::new(3.0, 5.0));
    }

    #[test]
    fn unwrap_by_face_gives_exact_rectangles_at_the_texel_density() {
        let mut doc = Document::new("t");
        let id = cube(&mut doc, 4.0);
        assert_eq!(unwrap(&mut doc, id, &BTreeSet::new(), Method::ByFace, 1.0, no_margin()), Ok(6));
        let s = shape(&doc, id);
        for f in 0..6 {
            assert_eq!(rect_of(&s, f), [4.0 * f as f32, 0.0, 4.0 * f as f32 + 4.0, 4.0], "face {f}");
        }
        // Грань +Z (вторая): низ-лево, низ-право, верх-право, верх-лево — картинка не зеркальная.
        assert_eq!(s.faces[1].uvs, vec![[4.0, 4.0], [8.0, 4.0], [8.0, 0.0], [4.0, 0.0]]);
        assert_eq!(islands(&s, None).len(), 6);
        // Плотность 2 — вдвое больше текселей.
        let mut doc = Document::new("t");
        let id = cube(&mut doc, 4.0);
        unwrap(&mut doc, id, &BTreeSet::new(), Method::ByFace, 2.0, no_margin()).unwrap();
        assert_eq!(rect_of(&shape(&doc, id), 0), [0.0, 0.0, 8.0, 8.0]);
    }

    #[test]
    fn unwrap_by_face_uses_the_true_shape_of_a_slanted_face() {
        // Квад 3×4 в наклонной плоскости (диагональ 5 по Y-Z): 3 × 5 текселей.
        let mut doc = Document::new("t");
        let points = [Vec3::new(0.0, 0.0, 0.0), Vec3::new(3.0, 0.0, 0.0), Vec3::new(3.0, 3.0, 4.0), Vec3::new(0.0, 3.0, 4.0)];
        let face = MeshFace { vertices: vec![0, 1, 2, 3], uvs: vec![[0.0, 0.0]; 4], texture: 0 };
        let id = ops::add_element(&mut doc, "q", Element::mesh(&points, vec![face]), None);
        unwrap(&mut doc, id, &BTreeSet::new(), Method::ByFace, 1.0, no_margin()).unwrap();
        assert_eq!(rect_of(&shape(&doc, id), 0), [0.0, 0.0, 3.0, 5.0]);
    }

    #[test]
    fn box_projection_makes_one_island_per_side_and_keeps_adjacent_faces_connected() {
        let mut doc = Document::new("t");
        let id = cube(&mut doc, 4.0);
        // Лента из двух граней на одной стороне (две половины верхней грани).
        let mut tall = mesh_ops::EditMesh::of(doc.element(id).unwrap()).unwrap();
        mesh_ops::subdivide(&mut tall, &BTreeSet::from([3usize]));
        let n = tall.faces.len();
        let mut d2 = Document::new("t");
        let id2 = ops::add_element(&mut d2, "m", Element::mesh(&tall.points, tall.faces), None);
        assert!(n > 6);
        unwrap(&mut d2, id2, &BTreeSet::new(), Method::Box, 1.0, no_margin()).unwrap();
        let s = shape(&d2, id2);
        let groups = islands(&s, None);
        assert_eq!(groups.len(), 6, "the subdivided top stays one island: {groups:?}");
        assert!(groups.iter().any(|g| g.len() == n - 5));
    }

    #[test]
    fn axis_and_view_projection_make_one_island() {
        let mut doc = Document::new("t");
        let id = cube(&mut doc, 4.0);
        unwrap(&mut doc, id, &BTreeSet::new(), Method::Axis(2), 1.0, no_margin()).unwrap();
        let s = shape(&doc, id);
        // Вся проекция — квадрат 4×4: все вершины в нём.
        let (lo, hi) = faces_bounds(&s, &(0..6).collect::<Vec<_>>()).unwrap();
        assert_eq!((lo, hi), (Vec2::ZERO, Vec2::splat(4.0)));
        // Вид сбоку (right = Z, up = Y): тоже один остров 4×4.
        let mut doc = Document::new("t");
        let id = cube(&mut doc, 4.0);
        unwrap(&mut doc, id, &BTreeSet::new(), Method::View { right: Vec3::Z, up: Vec3::Y }, 1.0, no_margin()).unwrap();
        let (lo, hi) = faces_bounds(&shape(&doc, id), &(0..6).collect::<Vec<_>>()).unwrap();
        assert_eq!((lo, hi), (Vec2::ZERO, Vec2::splat(4.0)));
    }

    #[test]
    fn transforms_land_on_texel_corners() {
        let mut doc = Document::new("t");
        let id = cube(&mut doc, 4.0);
        unwrap(&mut doc, id, &BTreeSet::new(), Method::ByFace, 1.0, no_margin()).unwrap();
        let mut s = shape(&doc, id);
        let c = corners_of_faces(&s, &BTreeSet::from([0usize]));
        // Сдвиг на 2.4 с прижатием — на 2.
        let d = move_corners(&mut s, &c, Vec2::new(2.4, 1.6), true);
        assert_eq!(d, Vec2::new(2.0, 2.0));
        assert_eq!(rect_of(&s, 0), [2.0, 2.0, 6.0, 6.0]);
        // Без прижатия — как есть.
        move_corners(&mut s, &c, Vec2::new(0.5, 0.0), false);
        assert_eq!(rect_of(&s, 0), [2.5, 2.0, 6.5, 6.0]);
        // Поворот на 90° вокруг центра: остаётся квадрат, углы на целых.
        rotate_quarters(&mut s, &c, 1, true);
        let r = rect_of(&s, 0);
        assert_eq!(r[2] - r[0], 4.0);
        assert!(r.iter().all(|v| v.fract() == 0.0), "{r:?}");
        // Масштаб ×1.4 с прижатием: 4 → 6 (5.6 округляется), угол — целый.
        scale_corners(&mut s, &c, Vec2::new(r[0], r[1]), Vec2::splat(1.4), true);
        let r = rect_of(&s, 0);
        assert_eq!((r[2] - r[0], r[3] - r[1]), (6.0, 6.0));
        assert!(r.iter().all(|v| v.fract() == 0.0), "{r:?}");
        // Масштаб 0.01 не схлопывает в точку.
        scale_corners(&mut s, &c, Vec2::new(r[0], r[1]), Vec2::splat(0.01), true);
        let r = rect_of(&s, 0);
        assert_eq!((r[2] - r[0], r[3] - r[1]), (1.0, 1.0));
    }

    #[test]
    fn rotate_90_and_flip_have_exact_coordinates() {
        let mut s = MeshShape {
            vertices: vec![[0.0; 3]; 4],
            faces: vec![MeshFace { vertices: vec![0, 1, 2, 3], uvs: vec![[2.0, 2.0], [6.0, 2.0], [6.0, 4.0], [2.0, 4.0]], texture: 0 }],
        };
        let c: BTreeSet<Corner> = (0..4).map(|k| (0, k)).collect();
        rotate_quarters(&mut s, &c, 1, true);
        // Центр (4, 3): размер 2×4, левый верхний угол (3, 1); по часовой: левый верх → правый верх.
        assert_eq!(s.faces[0].uvs, vec![[5.0, 1.0], [5.0, 5.0], [3.0, 5.0], [3.0, 1.0]]);
        rotate_quarters(&mut s, &c, 3, true);
        assert_eq!(s.faces[0].uvs, vec![[2.0, 2.0], [6.0, 2.0], [6.0, 4.0], [2.0, 4.0]]);
        flip_corners(&mut s, &c, false);
        assert_eq!(s.faces[0].uvs, vec![[6.0, 2.0], [2.0, 2.0], [2.0, 4.0], [6.0, 4.0]]);
        flip_corners(&mut s, &c, true);
        assert_eq!(s.faces[0].uvs, vec![[6.0, 4.0], [2.0, 4.0], [2.0, 2.0], [6.0, 2.0]]);
        // Свободный поворот на 90° по часовой вокруг (4, 3) без прижатия.
        rotate_corners(&mut s, &c, Vec2::new(4.0, 3.0), std::f32::consts::FRAC_PI_2, false);
        let p = Vec2::from(s.faces[0].uvs[0]);
        assert!((p - Vec2::new(3.0, 5.0)).length() < 1e-4, "{p:?}");
        snap_corners(&mut s, &c);
        assert_eq!(s.faces[0].uvs[0], [3.0, 5.0]);
    }

    #[test]
    fn islands_follow_shared_edges_with_equal_uvs() {
        let mut doc = Document::new("t");
        let id = cube(&mut doc, 4.0);
        unwrap(&mut doc, id, &BTreeSet::new(), Method::Axis(2), 1.0, no_margin()).unwrap();
        // Все грани разом — связный остров (проекция непрерывна).
        let s = shape(&doc, id);
        assert_eq!(islands(&s, None).len(), 1);
        // Рвём: грань 5 уезжает в свой остров.
        split_to_island(&mut doc, id, &BTreeSet::from([5usize]), Pack { margin: 1, ..no_margin() }).unwrap();
        let s = shape(&doc, id);
        let groups = islands(&s, None);
        assert_eq!(groups.len(), 2);
        assert!(groups.iter().any(|g| g == &vec![5]));
        assert_eq!(island_of(&s, 5), vec![5]);
        // Склейка возвращает то, что стоит рядом: вершины одной точки в пределах допуска.
        assert_eq!(weld(&mut doc, id, &BTreeSet::new(), 0.01), 0, "nothing coincides within a hundredth of a texel after the split");
    }

    #[test]
    fn weld_joins_coincident_uvs_of_one_vertex() {
        let mut doc = Document::new("t");
        let id = cube(&mut doc, 4.0);
        unwrap(&mut doc, id, &BTreeSet::new(), Method::Axis(2), 1.0, no_margin()).unwrap();
        // Сдвигаем угол одной грани на четверть текселя.
        let (f, k) = (1, 0);
        edit_shape(&mut doc, id, |s| s.faces[f].uvs[k][0] += 0.25);
        let before = shape(&doc, id);
        let v = before.faces[f].vertices[k];
        let corners_of = |s: &MeshShape| -> Vec<Vec2> {
            s.faces.iter().flat_map(|face| face.vertices.iter().zip(&face.uvs).filter(|(w, _)| **w == v).map(|(_, uv)| Vec2::from(*uv))).collect()
        };
        let spread = |c: &[Vec2]| c.iter().fold(0.0f32, |m, p| m.max((*p - c[0]).abs().max_element()));
        assert!(spread(&corners_of(&before)) > 0.2, "the nudged corner is apart from the others");
        assert!(weld(&mut doc, id, &BTreeSet::new(), 0.5) >= 1);
        assert!(spread(&corners_of(&shape(&doc, id))) < 1e-5, "all corners of the vertex are one point now");
    }

    #[test]
    fn pack_moves_pixels_with_the_islands_and_respects_the_margin() {
        let mut doc = Document::new("t");
        let id = cube(&mut doc, 4.0);
        unwrap(&mut doc, id, &BTreeSet::new(), Method::ByFace, 1.0, no_margin()).unwrap();
        // Метка в левом верхнем тексель грани 2 и в правом нижнем тексель грани 4.
        let s = shape(&doc, id);
        let r2 = rect_of(&s, 2);
        let r4 = rect_of(&s, 4);
        let (m2, m4) = ((r2[0] as i32, r2[1] as i32), (r4[2] as i32 - 1, r4[3] as i32 - 1));
        doc.textures[0].image.set(m2.0, m2.1, [1, 2, 3, 255]);
        doc.textures[0].image.set(m4.0, m4.1, [9, 8, 7, 255]);
        let pack = Pack { margin: 2, rotate: false, snap: true };
        assert_eq!(pack_islands(&mut doc, id, &BTreeSet::new(), pack), Ok(6));
        let s = shape(&doc, id);
        // Зазор: острова не ближе 2 текселей.
        let rects: Vec<[f32; 4]> = (0..6).map(|f| rect_of(&s, f)).collect();
        for a in 0..6 {
            for b in a + 1..6 {
                let (p, q) = (rects[a], rects[b]);
                let apart = p[2] + 2.0 <= q[0] || q[2] + 2.0 <= p[0] || p[3] + 2.0 <= q[1] || q[3] + 2.0 <= p[1];
                assert!(apart, "{a} and {b} are closer than the margin: {p:?} {q:?}");
            }
        }
        // Метки на тех же местах граней.
        let n2 = rect_of(&s, 2);
        let n4 = rect_of(&s, 4);
        assert_eq!(doc.textures[0].image.get(n2[0] as i32, n2[1] as i32), Some([1, 2, 3, 255]));
        assert_eq!(doc.textures[0].image.get(n4[2] as i32 - 1, n4[3] as i32 - 1), Some([9, 8, 7, 255]));
    }

    #[test]
    fn pack_with_rotation_turns_pixels_and_uvs_together() {
        // Высокий остров 2×6 с меткой в левом верхнем тексель — при повороте ложится плашмя.
        let mut doc = Document::new("t");
        let points = [Vec3::new(0.0, 0.0, 0.0), Vec3::new(2.0, 0.0, 0.0), Vec3::new(2.0, 6.0, 0.0), Vec3::new(0.0, 6.0, 0.0)];
        let face = MeshFace { vertices: vec![0, 1, 2, 3], uvs: vec![[10.0, 10.0], [12.0, 10.0], [12.0, 16.0], [10.0, 16.0]], texture: 0 };
        let id = ops::add_element(&mut doc, "q", Element::mesh(&points, vec![face]), None);
        doc.textures[0].image.set(10, 10, [5, 5, 5, 255]);
        let pack = Pack { margin: 0, rotate: true, snap: true };
        pack_islands(&mut doc, id, &BTreeSet::new(), pack).unwrap();
        let s = shape(&doc, id);
        let r = rect_of(&s, 0);
        assert_eq!((r[2] - r[0], r[3] - r[1]), (6.0, 2.0), "the island lies flat");
        // Угол (10,10) грани — теперь там, куда отправился первый угол UV; на нём метка.
        let corner_texel = |uv: [f32; 2], toward: [f32; 2]| ((uv[0] + (toward[0] - uv[0]) * 0.1).floor() as i32, (uv[1] + (toward[1] - uv[1]) * 0.1).floor() as i32);
        let t = corner_texel(s.faces[0].uvs[0], s.faces[0].uvs[2]);
        assert_eq!(doc.textures[0].image.get(t.0, t.1), Some([5, 5, 5, 255]), "the marker stays at the first corner of the face");
    }

    #[test]
    fn scale_to_density_sets_one_density_for_all_islands() {
        let mut doc = Document::new("t");
        let id = cube(&mut doc, 4.0);
        unwrap(&mut doc, id, &BTreeSet::new(), Method::ByFace, 1.0, no_margin()).unwrap();
        scale_to_density(&mut doc, id, &BTreeSet::new(), 2.0, no_margin()).unwrap();
        let (s, points) = mesh_of(&doc, id).unwrap();
        for f in 0..6 {
            let d = density_of(&points, &s, &[f]);
            assert!((d - 2.0).abs() < 0.01, "face {f}: {d}");
        }
        // Назад к 1 — и острова снова 4×4.
        scale_to_density(&mut doc, id, &BTreeSet::new(), 1.0, no_margin()).unwrap();
        let s = shape(&doc, id);
        for f in 0..6 {
            let r = rect_of(&s, f);
            assert_eq!((r[2] - r[0], r[3] - r[1]), (4.0, 4.0));
        }
        assert_eq!(texels_per_unit(16.0, 16.0), 1.0);
        assert_eq!(texels_per_unit(32.0, 16.0), 2.0);
    }

    #[test]
    fn unwrap_keeps_the_face_colour_and_frees_the_old_place() {
        let mut doc = Document::new("t");
        let id = cube(&mut doc, 4.0);
        unwrap(&mut doc, id, &BTreeSet::new(), Method::ByFace, 1.0, no_margin()).unwrap();
        // Покрасим грань 0 целиком в красный.
        let r = rect_of(&shape(&doc, id), 0);
        for y in r[1] as i32..r[3] as i32 {
            for x in r[0] as i32..r[2] as i32 {
                doc.textures[0].image.set(x, y, [200, 0, 0, 255]);
            }
        }
        // Развёртка только грани 0 по-другому (вид) — цвет остаётся красным на новом месте.
        unwrap(&mut doc, id, &BTreeSet::from([0usize]), Method::ByFace, 1.0, no_margin()).unwrap();
        let s = shape(&doc, id);
        let n = rect_of(&s, 0);
        let (cx, cy) = (((n[0] + n[2]) / 2.0) as i32, ((n[1] + n[3]) / 2.0) as i32);
        assert_eq!(doc.textures[0].image.get(cx, cy), Some([200, 0, 0, 255]));
        // Остальные грани не тронуты.
        assert_eq!(rect_of(&s, 1), [4.0, 0.0, 8.0, 4.0]);
    }

    #[test]
    fn the_atlas_grows_when_the_islands_do_not_fit_and_uvs_keep_their_pixels() {
        let mut doc = Document::new("t");
        let id = cube(&mut doc, 40.0);
        let before = doc.textures[0].image.size();
        unwrap(&mut doc, id, &BTreeSet::new(), Method::ByFace, 1.0, no_margin()).unwrap();
        let after = doc.textures[0].image.size();
        assert!(after[0] * after[1] > before[0] * before[1], "{before:?} -> {after:?}");
        // UV в текселях: рост холста их не меняет, а нормированные UV пересчитываются.
        let s = shape(&doc, id);
        let mesh = crate::build_mesh(&doc, Default::default());
        let uv0 = s.faces[0].uvs[0];
        assert!(mesh.vertices.iter().any(|v| (v.uv.x * after[0] as f32 - uv0[0]).abs() < 1e-3 && (v.uv.y * after[1] as f32 - uv0[1]).abs() < 1e-3));
        // Ручной рост холста: те же тексели.
        ops::resize_texture(&mut doc, 0, after[0] * 2, after[1] * 2);
        assert_eq!(shape(&doc, id), s);
        let mesh = crate::build_mesh(&doc, Default::default());
        assert!(mesh.vertices.iter().any(|v| (v.uv.x * (after[0] * 2) as f32 - uv0[0]).abs() < 1e-3));
    }

    #[test]
    fn operations_on_a_selection_leave_other_faces_alone() {
        let mut doc = Document::new("t");
        let id = cube(&mut doc, 4.0);
        unwrap(&mut doc, id, &BTreeSet::new(), Method::ByFace, 1.0, no_margin()).unwrap();
        let before = shape(&doc, id);
        pack_islands(&mut doc, id, &BTreeSet::from([2usize, 3]), no_margin()).unwrap();
        let after = shape(&doc, id);
        for f in [0, 1, 4, 5] {
            assert_eq!(before.faces[f].uvs, after.faces[f].uvs, "face {f}");
        }
        // Занятые места не пересекаются.
        let rects = uv::occupied(&doc, 0, &[]);
        for a in 0..rects.len() {
            for b in a + 1..rects.len() {
                let (p, q) = (rects[a], rects[b]);
                assert!(!(p[0] < q[0] + q[2] && q[0] < p[0] + p[2] && p[1] < q[1] + q[3] && q[1] < p[1] + p[3]), "{p:?} overlaps {q:?}");
            }
        }
        assert!(unwrap(&mut doc, NodeId(999), &BTreeSet::new(), Method::Box, 1.0, no_margin()).is_err());
    }
}
