//! Моделирование мешей: топологические операции над вершинами и гранями.
//!
//! Работают над [`EditMesh`] — вершины в единицах модели (до поворота элемента) и грани
//! (против часовой стрелки снаружи, UV углов в текселях). [`apply`] берёт меш элемента
//! документа, применяет операцию и записывает результат обратно (коробка элемента — по
//! новым границам; близнец обновляется). Историю правок ведёт [`crate::Session`]: каждая
//! операция в окне — один шаг.
//!
//! Инварианты, которые держит каждая операция (их проверяют тесты): нет граней меньше
//! трёх вершин, нет повторов вершины подряд в грани, нет двух граней с одним набором
//! вершин, у замкнутого меша обход наружу (объём положителен), UV новых граней — внутри
//! атласа (места выделяет [`crate::mesh_edit::allocate_uvs`]) или интерполированы из
//! исходной грани (разрез, вставка, деление — нарисованное остаётся на месте).

use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};

use glam::{Quat, Vec2, Vec3};

use crate::doc::{Document, Element, MeshFace, NodeId};
use crate::ops;

/// Ребро без направления: (меньшая вершина, большая).
pub type Edge = (u32, u32);

pub fn edge(a: u32, b: u32) -> Edge {
    if a < b {
        (a, b)
    } else {
        (b, a)
    }
}

/// Меш для правки.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct EditMesh {
    pub points: Vec<Vec3>,
    pub faces: Vec<MeshFace>,
}

/// Что выделено: вершины, рёбра, грани (режим решает, что показывать и чем оперировать).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Selection {
    pub verts: BTreeSet<u32>,
    pub edges: BTreeSet<Edge>,
    pub faces: BTreeSet<usize>,
}

/// Режим выделения: 1 — вершины, 2 — рёбра, 3 — грани (как в Blender).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum SelectMode {
    #[default]
    Vertex,
    Edge,
    Face,
}

impl EditMesh {
    /// Меш элемента (вершины в единицах модели).
    pub fn of(element: &Element) -> Option<EditMesh> {
        let shape = element.mesh.as_ref()?;
        Some(EditMesh { points: element.mesh_points(), faces: shape.faces.clone() })
    }

    pub fn face_points(&self, f: usize) -> Vec<Vec3> {
        self.faces[f].vertices.iter().map(|v| self.points[*v as usize]).collect()
    }

    /// Нормаль Ньюэлла (единичная; ноль у вырожденной).
    pub fn normal(&self, f: usize) -> Vec3 {
        newell(&self.face_points(f)).normalize_or_zero()
    }

    pub fn centroid(&self, f: usize) -> Vec3 {
        let p = self.face_points(f);
        p.iter().fold(Vec3::ZERO, |s, q| s + *q) / p.len().max(1) as f32
    }

    /// Площадь грани.
    pub fn area(&self, f: usize) -> f32 {
        newell(&self.face_points(f)).length() * 0.5
    }

    /// Рёбра грани по обходу: (из, в).
    pub fn face_edges(&self, f: usize) -> Vec<(u32, u32)> {
        let v = &self.faces[f].vertices;
        (0..v.len()).map(|i| (v[i], v[(i + 1) % v.len()])).collect()
    }

    /// Ребро → грани, в которых оно есть.
    pub fn edge_faces(&self) -> HashMap<Edge, Vec<usize>> {
        let mut map: HashMap<Edge, Vec<usize>> = HashMap::new();
        for f in 0..self.faces.len() {
            for (a, b) in self.face_edges(f) {
                map.entry(edge(a, b)).or_default().push(f);
            }
        }
        map
    }

    /// Все рёбра меша.
    pub fn all_edges(&self) -> BTreeSet<Edge> {
        (0..self.faces.len()).flat_map(|f| self.face_edges(f)).map(|(a, b)| edge(a, b)).collect()
    }

    /// Вершина → грани.
    pub fn vertex_faces(&self) -> Vec<Vec<usize>> {
        let mut out = vec![Vec::new(); self.points.len()];
        for (f, face) in self.faces.iter().enumerate() {
            for v in &face.vertices {
                if let Some(list) = out.get_mut(*v as usize) {
                    if !list.contains(&f) {
                        list.push(f);
                    }
                }
            }
        }
        out
    }

    /// Ориентированный объём (положителен у замкнутого меша с обходом наружу).
    pub fn signed_volume(&self) -> f32 {
        let mut v = 0.0;
        for face in &self.faces {
            let p: Vec<Vec3> = face.vertices.iter().map(|i| self.points[*i as usize]).collect();
            for k in 1..p.len().saturating_sub(1) {
                v += p[0].dot(p[k].cross(p[k + 1]));
            }
        }
        v / 6.0
    }

    /// Каждое ребро — ровно у двух граней, и в них оно идёт навстречу (замкнут и согласован).
    pub fn is_closed_manifold(&self) -> bool {
        let mut directed: HashMap<(u32, u32), usize> = HashMap::new();
        for f in 0..self.faces.len() {
            for e in self.face_edges(f) {
                *directed.entry(e).or_default() += 1;
            }
        }
        directed.iter().all(|((a, b), n)| *n == 1 && directed.get(&(*b, *a)) == Some(&1))
    }

    /// Эйлерова характеристика V − E + F (используемые вершины).
    pub fn euler(&self) -> i64 {
        let used: HashSet<u32> = self.faces.iter().flat_map(|f| f.vertices.iter().copied()).collect();
        used.len() as i64 - self.all_edges().len() as i64 + self.faces.len() as i64
    }

    /// Проверить инварианты; ошибка — что нарушено.
    pub fn check(&self) -> Result<(), String> {
        let mut seen: HashSet<Vec<u32>> = HashSet::new();
        for (f, face) in self.faces.iter().enumerate() {
            if face.vertices.len() < 3 {
                return Err(format!("face {f} has {} vertices", face.vertices.len()));
            }
            if face.uvs.len() != face.vertices.len() {
                return Err(format!("face {f} has {} UVs for {} vertices", face.uvs.len(), face.vertices.len()));
            }
            if face.vertices.iter().any(|v| *v as usize >= self.points.len()) {
                return Err(format!("face {f} uses a missing vertex"));
            }
            let unique: HashSet<u32> = face.vertices.iter().copied().collect();
            if unique.len() != face.vertices.len() {
                return Err(format!("face {f} repeats a vertex"));
            }
            if self.area(f) < 1e-7 {
                return Err(format!("face {f} is degenerate"));
            }
            let mut key = face.vertices.clone();
            key.sort_unstable();
            if !seen.insert(key) {
                return Err(format!("face {f} duplicates another face"));
            }
        }
        Ok(())
    }

    /// Убрать вершины без граней (номера вершин перенумеровываются); вернуть старый → новый.
    pub fn compact(&mut self) -> Vec<Option<u32>> {
        let mut used = vec![false; self.points.len()];
        for f in &self.faces {
            for v in &f.vertices {
                used[*v as usize] = true;
            }
        }
        let mut remap = vec![None; self.points.len()];
        let mut points = Vec::new();
        for (i, p) in self.points.iter().enumerate() {
            if used[i] {
                remap[i] = Some(points.len() as u32);
                points.push(*p);
            }
        }
        for f in &mut self.faces {
            for v in &mut f.vertices {
                *v = remap[*v as usize].unwrap_or(0);
            }
        }
        self.points = points;
        remap
    }

    /// Привести грани в порядок после слияния вершин: убрать повторы подряд, грани меньше
    /// трёх вершин, вырожденные и дубликаты.
    pub fn clean(&mut self) {
        let mut seen: HashSet<Vec<u32>> = HashSet::new();
        let points = self.points.clone();
        self.faces.retain_mut(|f| {
            let mut v: Vec<(u32, [f32; 2])> = Vec::new();
            for (k, x) in f.vertices.iter().enumerate() {
                if v.last().map(|l| l.0) != Some(*x) {
                    v.push((*x, f.uvs.get(k).copied().unwrap_or([0.0, 0.0])));
                }
            }
            while v.len() > 1 && v.first().map(|a| a.0) == v.last().map(|a| a.0) {
                v.pop();
            }
            // Вершина, повторённая не подряд (грань «восьмёркой»), — грань убирается.
            let unique: HashSet<u32> = v.iter().map(|x| x.0).collect();
            if unique.len() != v.len() || v.len() < 3 {
                return false;
            }
            f.vertices = v.iter().map(|x| x.0).collect();
            f.uvs = v.iter().map(|x| x.1).collect();
            let p: Vec<Vec3> = f.vertices.iter().map(|i| points[*i as usize]).collect();
            if newell(&p).length() < 1e-6 {
                return false;
            }
            let mut key = f.vertices.clone();
            key.sort_unstable();
            seen.insert(key)
        });
    }
}

pub fn newell(points: &[Vec3]) -> Vec3 {
    let mut n = Vec3::ZERO;
    for i in 0..points.len() {
        let (a, b) = (points[i], points[(i + 1) % points.len()]);
        n += Vec3::new((a.y - b.y) * (a.z + b.z), (a.z - b.z) * (a.x + b.x), (a.x - b.x) * (a.y + b.y));
    }
    n
}

/// Изменить меш элемента документа: операция над [`EditMesh`], запись обратно (коробка —
/// по границам, точка вращения на месте), обновление близнеца. `None` — не меш.
pub fn apply<R>(doc: &mut Document, id: NodeId, f: impl FnOnce(&mut EditMesh) -> R) -> Option<R> {
    let element = doc.element(id)?;
    let mut mesh = EditMesh::of(element)?;
    let result = f(&mut mesh);
    let element = doc.element_mut(id)?;
    if let Some(shape) = element.mesh.as_mut() {
        shape.faces = mesh.faces;
    }
    element.set_mesh_points(&mesh.points);
    ops::sync_twins(doc, &[id]);
    Some(result)
}

// ----------------------------------------------------------------------
// UV внутри грани
// ----------------------------------------------------------------------

/// UV точки `p` внутри (плоской) грани по UV её углов: барицентрические координаты в
/// треугольнике веера, в который точка попадает (или ближайшем).
pub fn interpolate_uv(corners: &[Vec3], uvs: &[[f32; 2]], p: Vec3) -> [f32; 2] {
    if corners.len() < 3 {
        return uvs.first().copied().unwrap_or([0.0, 0.0]);
    }
    let mut best: Option<(f32, [f32; 2])> = None;
    for k in 1..corners.len() - 1 {
        let (a, b, c) = (corners[0], corners[k], corners[k + 1]);
        let (v0, v1, v2) = (b - a, c - a, p - a);
        let (d00, d01, d11, d20, d21) = (v0.dot(v0), v0.dot(v1), v1.dot(v1), v2.dot(v0), v2.dot(v1));
        let den = d00 * d11 - d01 * d01;
        if den.abs() < 1e-12 {
            continue;
        }
        let v = (d11 * d20 - d01 * d21) / den;
        let w = (d00 * d21 - d01 * d20) / den;
        let u = 1.0 - v - w;
        let outside = (-u).max(-v).max(-w).max(0.0);
        let uv = [
            u * uvs[0][0] + v * uvs[k][0] + w * uvs[k + 1][0],
            u * uvs[0][1] + v * uvs[k][1] + w * uvs[k + 1][1],
        ];
        if best.is_none_or(|(o, _)| outside < o) {
            best = Some((outside, uv));
        }
    }
    best.map_or(uvs[0], |b| b.1)
}

fn lerp2(a: [f32; 2], b: [f32; 2], t: f32) -> [f32; 2] {
    [a[0] + (b[0] - a[0]) * t, a[1] + (b[1] - a[1]) * t]
}

/// Новые грани, у которых есть общее ребро со старыми, получают обход, встречный старым.
fn orient_new_faces(m: &mut EditMesh, new: &[usize]) {
    let old: HashSet<usize> = (0..m.faces.len()).filter(|f| !new.contains(f)).collect();
    let mut directed: HashSet<(u32, u32)> = HashSet::new();
    for f in &old {
        directed.extend(m.face_edges(*f));
    }
    for &f in new {
        let same = m.face_edges(f).iter().filter(|e| directed.contains(e)).count();
        let opposite = m.face_edges(f).iter().filter(|(a, b)| directed.contains(&(*b, *a))).count();
        if same > opposite {
            m.faces[f].vertices.reverse();
            m.faces[f].uvs.reverse();
        }
        directed.extend(m.face_edges(f));
    }
}

// ----------------------------------------------------------------------
// Выделение
// ----------------------------------------------------------------------

/// Грани, все вершины которых выделены.
pub fn faces_of_verts(m: &EditMesh, verts: &BTreeSet<u32>) -> BTreeSet<usize> {
    (0..m.faces.len()).filter(|f| m.faces[*f].vertices.iter().all(|v| verts.contains(v))).collect()
}

/// Рёбра, оба конца которых выделены.
pub fn edges_of_verts(m: &EditMesh, verts: &BTreeSet<u32>) -> BTreeSet<Edge> {
    m.all_edges().into_iter().filter(|(a, b)| verts.contains(a) && verts.contains(b)).collect()
}

/// Выделение из вершин: рёбра и грани между ними.
pub fn select_verts(m: &EditMesh, verts: BTreeSet<u32>) -> Selection {
    Selection { edges: edges_of_verts(m, &verts), faces: faces_of_verts(m, &verts), verts }
}

/// Выделение из граней: их вершины и рёбра.
pub fn select_faces(m: &EditMesh, faces: BTreeSet<usize>) -> Selection {
    let verts: BTreeSet<u32> = faces.iter().filter_map(|f| m.faces.get(*f)).flat_map(|f| f.vertices.iter().copied()).collect();
    let edges = faces.iter().filter(|f| **f < m.faces.len()).flat_map(|f| m.face_edges(*f)).map(|(a, b)| edge(a, b)).collect();
    Selection { verts, edges, faces }
}

/// Выделение из рёбер.
pub fn select_edges(m: &EditMesh, edges: BTreeSet<Edge>) -> Selection {
    let verts: BTreeSet<u32> = edges.iter().flat_map(|(a, b)| [*a, *b]).collect();
    Selection { faces: faces_of_verts(m, &verts), verts, edges }
}

/// Всё связанное с выделенными вершинами (L / Ctrl+L).
pub fn linked(m: &EditMesh, verts: &BTreeSet<u32>) -> BTreeSet<u32> {
    let vf = m.vertex_faces();
    let mut out = verts.clone();
    let mut stack: Vec<u32> = verts.iter().copied().collect();
    while let Some(v) = stack.pop() {
        for f in vf.get(v as usize).into_iter().flatten() {
            for w in &m.faces[*f].vertices {
                if out.insert(*w) {
                    stack.push(*w);
                }
            }
        }
    }
    out
}

/// Расширить выделение на соседей (Ctrl +).
pub fn grow(m: &EditMesh, verts: &BTreeSet<u32>) -> BTreeSet<u32> {
    let mut out = verts.clone();
    for face in &m.faces {
        if face.vertices.iter().any(|v| verts.contains(v)) {
            out.extend(face.vertices.iter().copied());
        }
    }
    out
}

/// Сузить выделение: убрать вершины на его краю (Ctrl −).
pub fn shrink(m: &EditMesh, verts: &BTreeSet<u32>) -> BTreeSet<u32> {
    let mut out = verts.clone();
    for face in &m.faces {
        if face.vertices.iter().any(|v| !verts.contains(v)) {
            for v in &face.vertices {
                out.remove(v);
            }
        }
    }
    out
}

/// Грани с нормалью в пределах `degrees` от нормали грани `f` (выбор по нормали);
/// `coplanar` — ещё и в той же плоскости.
pub fn similar_normal(m: &EditMesh, f: usize, degrees: f32, coplanar: bool) -> BTreeSet<usize> {
    let n = m.normal(f);
    let d = n.dot(m.centroid(f));
    let cos = degrees.to_radians().cos();
    (0..m.faces.len())
        .filter(|g| m.normal(*g).dot(n) >= cos - 1e-6 && (!coplanar || (m.normal(*g).dot(m.centroid(*g)) - d).abs() < 1e-3))
        .collect()
}

/// Петля рёбер через ребро (Alt+щелчок): по вершинам с четырьмя рёбрами — к «противоположному» ребру.
pub fn edge_loop(m: &EditMesh, start: Edge) -> BTreeSet<Edge> {
    let ef = m.edge_faces();
    let vf = m.vertex_faces();
    let edges_at = |v: u32| -> Vec<Edge> {
        let mut out: Vec<Edge> = vf[v as usize].iter().flat_map(|f| m.face_edges(*f)).filter(|(a, b)| *a == v || *b == v).map(|(a, b)| edge(a, b)).collect();
        out.sort();
        out.dedup();
        out
    };
    let mut out = BTreeSet::from([start]);
    for (mut from, mut at) in [(start.0, start.1), (start.1, start.0)] {
        for _ in 0..m.points.len() {
            let current = edge(from, at);
            let around = edges_at(at);
            if around.len() != 4 {
                break;
            }
            let faces = ef.get(&current).cloned().unwrap_or_default();
            let next: Vec<Edge> = around
                .into_iter()
                .filter(|e| *e != current && !ef.get(e).is_some_and(|fs| fs.iter().any(|f| faces.contains(f))))
                .collect();
            let [next] = next[..] else { break };
            if !out.insert(next) {
                break;
            }
            let other = if next.0 == at { next.1 } else { next.0 };
            from = at;
            at = other;
        }
    }
    out
}

/// Кольцо квадов через ребро: (грань, входное ребро по обходу грани, выходное) — основа
/// петлевого разреза и выбора петли граней.
fn ring(m: &EditMesh, start: Edge) -> Vec<(usize, (u32, u32), (u32, u32))> {
    let ef = m.edge_faces();
    let mut out: Vec<(usize, (u32, u32), (u32, u32))> = Vec::new();
    let mut visited: HashSet<usize> = HashSet::new();
    let step = |f: usize, entry: Edge| -> Option<((u32, u32), (u32, u32))> {
        let v = &m.faces[f].vertices;
        if v.len() != 4 {
            return None;
        }
        let i = (0..4).find(|i| edge(v[*i], v[(i + 1) % 4]) == entry)?;
        Some(((v[i], v[(i + 1) % 4]), (v[(i + 2) % 4], v[(i + 3) % 4])))
    };
    for (pass, &f0) in ef.get(&start).into_iter().flatten().enumerate().take(2) {
        let (mut f, mut entry) = (f0, start);
        let mut chain = Vec::new();
        loop {
            if !visited.insert(f) {
                break;
            }
            let Some((enter, exit)) = step(f, entry) else { break };
            chain.push((f, enter, exit));
            let next = edge(exit.0, exit.1);
            let Some(g) = ef.get(&next).and_then(|fs| fs.iter().find(|g| **g != f)).copied() else { break };
            f = g;
            entry = next;
        }
        if pass == 0 {
            out = chain;
        } else {
            // Вторая сторона — в обратном порядке перед первой.
            chain.reverse();
            chain.extend(out);
            out = chain;
        }
    }
    out
}

/// Петля граней (Alt+щелчок в режиме граней).
pub fn face_loop(m: &EditMesh, start: Edge) -> BTreeSet<usize> {
    ring(m, start).into_iter().map(|r| r.0).collect()
}

// ----------------------------------------------------------------------
// Операции
// ----------------------------------------------------------------------

/// Выдавить область граней на `distance` вдоль средней нормали (E). Возвращает
/// (грани-крышки, новые боковые грани). Края области, не делённые с другими выделенными
/// гранями, становятся стенками.
pub fn extrude_faces(m: &mut EditMesh, faces: &BTreeSet<usize>, distance: f32) -> (BTreeSet<usize>, Vec<usize>) {
    let faces: BTreeSet<usize> = faces.iter().copied().filter(|f| *f < m.faces.len()).collect();
    if faces.is_empty() {
        return (faces, Vec::new());
    }
    let normal = faces.iter().fold(Vec3::ZERO, |s, f| s + m.normal(*f) * m.area(*f)).normalize_or_zero();
    // Края области: ребро одной выделенной грани без пары среди выделенных.
    let mut directed: Vec<(u32, u32, usize)> = Vec::new();
    let mut count: HashMap<Edge, usize> = HashMap::new();
    for f in &faces {
        for (a, b) in m.face_edges(*f) {
            directed.push((a, b, *f));
            *count.entry(edge(a, b)).or_default() += 1;
        }
    }
    let mut copy: BTreeMap<u32, u32> = BTreeMap::new();
    for f in &faces {
        for v in m.faces[*f].vertices.clone() {
            copy.entry(v).or_insert_with(|| {
                m.points.push(m.points[v as usize] + normal * distance);
                m.points.len() as u32 - 1
            });
        }
    }
    let mut sides = Vec::new();
    for (a, b, f) in directed {
        if count[&edge(a, b)] != 1 {
            continue;
        }
        let uv = &m.faces[f];
        let ia = uv.vertices.iter().position(|v| *v == a).unwrap_or(0);
        let ib = uv.vertices.iter().position(|v| *v == b).unwrap_or(0);
        let (ua, ub) = (uv.uvs[ia], uv.uvs[ib]);
        let texture = uv.texture;
        m.faces.push(MeshFace { vertices: vec![a, b, copy[&b], copy[&a]], uvs: vec![ua, ub, ub, ua], texture });
        sides.push(m.faces.len() - 1);
    }
    for f in &faces {
        for v in &mut m.faces[*f].vertices {
            *v = copy[v];
        }
    }
    (faces, sides)
}

/// Выдавить края-рёбра (у которых одна грань) наружу в плоскости их грани на `distance`.
/// Возвращает новые грани и новые рёбра (край).
pub fn extrude_edges(m: &mut EditMesh, edges: &BTreeSet<Edge>, distance: f32) -> (Vec<usize>, BTreeSet<Edge>) {
    let ef = m.edge_faces();
    let mut copy: BTreeMap<u32, u32> = BTreeMap::new();
    let mut offset: BTreeMap<u32, Vec3> = BTreeMap::new();
    let mut work = Vec::new();
    for e in edges {
        let Some([f]) = ef.get(e).map(|v| v.as_slice()) else { continue };
        let Some(&(a, b)) = m.face_edges(*f).iter().find(|(a, b)| edge(*a, *b) == *e) else { continue };
        let n = m.normal(*f);
        let dir = (m.points[b as usize] - m.points[a as usize]).cross(n).normalize_or_zero();
        for v in [a, b] {
            *offset.entry(v).or_insert(Vec3::ZERO) += dir;
        }
        work.push((a, b, *f));
    }
    for (&v, dir) in &offset {
        m.points.push(m.points[v as usize] + dir.normalize_or_zero() * distance);
        copy.insert(v, m.points.len() as u32 - 1);
    }
    let mut new = Vec::new();
    let mut rim = BTreeSet::new();
    for (a, b, f) in work {
        let face = &m.faces[f];
        let uv = |v: u32| face.uvs[face.vertices.iter().position(|x| *x == v).unwrap_or(0)];
        let (ua, ub) = (uv(a), uv(b));
        let texture = face.texture;
        m.faces.push(MeshFace { vertices: vec![b, a, copy[&a], copy[&b]], uvs: vec![ub, ua, ua, ub], texture });
        new.push(m.faces.len() - 1);
        rim.insert(edge(copy[&a], copy[&b]));
    }
    (new, rim)
}

/// Вставка (I): у каждой грани — внутренняя копия на расстоянии `thickness` от краёв и
/// кольцо квадов. UV внутри исходной грани интерполируются (нарисованное остаётся).
/// Возвращает внутренние грани и кольца.
pub fn inset(m: &mut EditMesh, faces: &BTreeSet<usize>, thickness: f32) -> (BTreeSet<usize>, Vec<usize>) {
    let mut inner = BTreeSet::new();
    let mut ring = Vec::new();
    if thickness <= 1e-4 {
        return (faces.clone(), ring);
    }
    for &f in faces {
        if f >= m.faces.len() {
            continue;
        }
        let corners = m.face_points(f);
        let uvs = m.faces[f].uvs.clone();
        let n = corners.len();
        let center = corners.iter().fold(Vec3::ZERO, |s, p| s + *p) / n as f32;
        let mut new_ids = Vec::with_capacity(n);
        let mut new_uvs = Vec::with_capacity(n);
        for i in 0..n {
            let (p, prev, next) = (corners[i], corners[(i + n - 1) % n], corners[(i + 1) % n]);
            let (e1, e2) = ((prev - p).normalize_or_zero(), (next - p).normalize_or_zero());
            let bis = (e1 + e2).normalize_or_zero();
            let sin = e1.cross(bis).length().max(0.05);
            let mut q = p + bis * (thickness / sin);
            // Не дальше центра грани.
            if (q - p).length() > (center - p).length() * 0.95 || bis == Vec3::ZERO {
                q = p + (center - p) * 0.95_f32.min(thickness / (center - p).length().max(1e-6));
            }
            m.points.push(q);
            new_ids.push(m.points.len() as u32 - 1);
            new_uvs.push(interpolate_uv(&corners, &uvs, q));
        }
        let outer = m.faces[f].vertices.clone();
        let texture = m.faces[f].texture;
        // Внутренняя грань схлопнулась (слишком толсто для узкой грани) — грань не трогаем.
        let inner_points: Vec<Vec3> = new_ids.iter().map(|v| m.points[*v as usize]).collect();
        if newell(&inner_points).length() < 1e-5 {
            m.points.truncate(m.points.len() - n);
            continue;
        }
        for i in 0..n {
            let j = (i + 1) % n;
            // Сторона на прямой (точка разреза соседа) даёт пустой квад — его нет.
            let quad_points = [m.points[outer[i] as usize], m.points[outer[j] as usize], m.points[new_ids[j] as usize], m.points[new_ids[i] as usize]];
            if newell(&quad_points).length() < 1e-6 {
                continue;
            }
            m.faces.push(MeshFace {
                vertices: vec![outer[i], outer[j], new_ids[j], new_ids[i]],
                uvs: vec![uvs[i], uvs[j], new_uvs[j], new_uvs[i]],
                texture,
            });
            ring.push(m.faces.len() - 1);
        }
        m.faces[f] = MeshFace { vertices: new_ids, uvs: new_uvs, texture };
        inner.insert(f);
    }
    (inner, ring)
}

/// Петлевой разрез (Ctrl+R): `cuts` разрезов по кольцу квадов через ребро `start`, в
/// долях `slide` (0.5 — посередине; для одного разреза). Возвращает новые рёбра разреза.
pub fn loop_cut(m: &mut EditMesh, start: Edge, cuts: usize, slide: f32) -> BTreeSet<Edge> {
    let cuts = cuts.clamp(1, 64);
    let chain = ring(m, start);
    let mut points: HashMap<(Edge, usize), u32> = HashMap::new();
    let fraction = |k: usize| -> f32 {
        if cuts == 1 {
            slide.clamp(0.02, 0.98)
        } else {
            k as f32 / (cuts + 1) as f32
        }
    };
    // Точка на ребре (u → v), k-я от u; хранится по номеру от меньшей вершины ребра.
    let mut point = |m: &mut EditMesh, u: u32, v: u32, k: usize| -> u32 {
        let e = edge(u, v);
        let j = if u == e.0 { k } else { cuts + 1 - k };
        *points.entry((e, j)).or_insert_with(|| {
            let t = fraction(j);
            let (a, b) = (m.points[e.0 as usize], m.points[e.1 as usize]);
            m.points.push(a + (b - a) * t);
            m.points.len() as u32 - 1
        })
    };
    let mut cut_edges = BTreeSet::new();
    for (f, (a, b), (c, d)) in chain {
        let face = m.faces[f].clone();
        let at = |v: u32| face.uvs[face.vertices.iter().position(|x| *x == v).unwrap_or(0)];
        let (ua, ub, uc, ud) = (at(a), at(b), at(c), at(d));
        let p: Vec<u32> = (1..=cuts).map(|k| point(m, a, b, k)).collect();
        let q: Vec<u32> = (1..=cuts).map(|k| point(m, d, c, k)).collect();
        let tk = |k: usize| {
            let (pa, pb) = (m.points[a as usize], m.points[b as usize]);
            (m.points[p[k] as usize] - pa).length() / (pb - pa).length().max(1e-9)
        };
        let mut row_p = vec![(a, ua)];
        let mut row_q = vec![(d, ud)];
        for k in 0..cuts {
            let t = tk(k);
            row_p.push((p[k], lerp2(ua, ub, t)));
            row_q.push((q[k], lerp2(ud, uc, t)));
            cut_edges.insert(edge(p[k], q[k]));
        }
        row_p.push((b, ub));
        row_q.push((c, uc));
        let mut new_faces = Vec::new();
        for k in 0..=cuts {
            new_faces.push(MeshFace {
                vertices: vec![row_p[k].0, row_p[k + 1].0, row_q[k + 1].0, row_q[k].0],
                uvs: vec![row_p[k].1, row_p[k + 1].1, row_q[k + 1].1, row_q[k].1],
                texture: face.texture,
            });
        }
        m.faces[f] = new_faces.remove(0);
        m.faces.extend(new_faces);
    }
    insert_edge_points(m, &points);
    cut_edges
}

/// Грани вне разреза, у которых на рёбрах появились новые точки, получают их в обход
/// (без Т-стыков).
fn insert_edge_points(m: &mut EditMesh, points: &HashMap<(Edge, usize), u32>) {
    let mut by_edge: HashMap<Edge, Vec<(usize, u32)>> = HashMap::new();
    for ((e, j), v) in points {
        by_edge.entry(*e).or_default().push((*j, *v));
    }
    for list in by_edge.values_mut() {
        list.sort();
    }
    for f in 0..m.faces.len() {
        let face = m.faces[f].clone();
        let n = face.vertices.len();
        let mut vertices = Vec::with_capacity(n);
        let mut uvs = Vec::with_capacity(n);
        let mut changed = false;
        for i in 0..n {
            let (u, v) = (face.vertices[i], face.vertices[(i + 1) % n]);
            vertices.push(u);
            uvs.push(face.uvs[i]);
            let Some(list) = by_edge.get(&edge(u, v)) else { continue };
            // Уже разрезанная грань содержит точки не на этом ребре — пропускаем, если они есть.
            let mut along: Vec<(f32, u32)> = list
                .iter()
                .map(|(_, w)| {
                    let (a, b, p) = (m.points[u as usize], m.points[v as usize], m.points[*w as usize]);
                    ((p - a).length() / (b - a).length().max(1e-9), *w)
                })
                .collect();
            along.sort_by(|x, y| x.0.total_cmp(&y.0));
            for (t, w) in along {
                if !face.vertices.contains(&w) {
                    vertices.push(w);
                    uvs.push(lerp2(face.uvs[i], face.uvs[(i + 1) % n], t));
                    changed = true;
                }
            }
        }
        if changed {
            m.faces[f] = MeshFace { vertices, uvs, texture: face.texture };
        }
    }
}

/// Фаска рёбер (Ctrl+B), один сегмент шириной `width`: ребро между двумя гранями
/// заменяется новой гранью, его концы раздваиваются вдоль соседних рёбер. Рёбра не должны
/// касаться друг друга (фаска нескольких рёбер у одной вершины — по очереди). Возвращает
/// новые грани.
pub fn bevel(m: &mut EditMesh, edges: &BTreeSet<Edge>, width: f32) -> Result<Vec<usize>, String> {
    let mut used: HashSet<u32> = HashSet::new();
    for (a, b) in edges {
        if !used.insert(*a) || !used.insert(*b) {
            return Err("bevel edges that do not touch each other (or bevel them one at a time)".to_string());
        }
    }
    let mut new = Vec::new();
    for &e in edges {
        new.push(bevel_one(m, e, width)?);
    }
    m.compact();
    Ok(new)
}

fn bevel_one(m: &mut EditMesh, e: Edge, width: f32) -> Result<usize, String> {
    let ef = m.edge_faces();
    let Some([f1, f2]) = ef.get(&e).map(|f| f.as_slice()) else {
        return Err("only edges between two faces can be bevelled".to_string());
    };
    let (f1, f2) = (*f1, *f2);
    // Точка на соседнем ребре грани `f` у вершины `v` (не на ребре фаски): (точка, UV, сосед, доля).
    let side = |m: &EditMesh, f: usize, v: u32, partner: u32| -> (Vec3, [f32; 2], u32, f32) {
        let face = &m.faces[f];
        let n = face.vertices.len();
        let i = face.vertices.iter().position(|x| *x == v).unwrap_or(0);
        let (prev, next) = (face.vertices[(i + n - 1) % n], face.vertices[(i + 1) % n]);
        let (other, k) = if prev == partner { (next, (i + 1) % n) } else { (prev, (i + n - 1) % n) };
        let dir = m.points[other as usize] - m.points[v as usize];
        let t = (width / dir.length().max(1e-6)).min(0.45);
        (m.points[v as usize] + dir * t, lerp2(face.uvs[i], face.uvs[k], t), other, t)
    };
    let mut created: HashMap<(usize, u32), u32> = HashMap::new(); // (грань, вершина) → новая точка
    let mut along: HashMap<(u32, u32), (u32, f32)> = HashMap::new(); // (вершина, сосед) → (новая точка, доля)
    // Сначала все четыре точки (по исходным граням), потом замены.
    let sides: Vec<(usize, u32, (Vec3, [f32; 2], u32, f32))> = [(f1, e.0, e.1), (f1, e.1, e.0), (f2, e.0, e.1), (f2, e.1, e.0)]
        .into_iter()
        .map(|(f, v, partner)| (f, v, side(m, f, v, partner)))
        .collect();
    for (f, v, (p, uv, other, t)) in sides {
        m.points.push(p);
        let id = m.points.len() as u32 - 1;
        created.insert((f, v), id);
        along.insert((v, other), (id, t));
        let face = &mut m.faces[f];
        if let Some(i) = face.vertices.iter().position(|x| *x == v) {
            face.vertices[i] = id;
            face.uvs[i] = uv;
        }
    }
    // Остальные грани у концов ребра: вершина → две точки на её рёбрах.
    for v in [e.0, e.1] {
        for f in 0..m.faces.len() {
            if f == f1 || f == f2 || !m.faces[f].vertices.contains(&v) {
                continue;
            }
            let face = m.faces[f].clone();
            let n = face.vertices.len();
            let i = face.vertices.iter().position(|x| *x == v).expect("checked");
            let (pi, ni) = ((i + n - 1) % n, (i + 1) % n);
            let (prev, next) = (face.vertices[pi], face.vertices[ni]);
            let (Some(&(x, tx)), Some(&(y, ty))) = (along.get(&(v, prev)), along.get(&(v, next))) else {
                return Err("this vertex has more than three faces around it; bevel needs simple corners".to_string());
            };
            let (ux, uy) = (lerp2(face.uvs[i], face.uvs[pi], tx), lerp2(face.uvs[i], face.uvs[ni], ty));
            let mut vertices = face.vertices.clone();
            let mut uvs = face.uvs.clone();
            vertices.splice(i..=i, [x, y]);
            uvs.splice(i..=i, [ux, uy]);
            m.faces[f] = MeshFace { vertices, uvs, texture: face.texture };
        }
    }
    let texture = m.faces[f1].texture;
    let (a1, b1, a2, b2) = (created[&(f1, e.0)], created[&(f1, e.1)], created[&(f2, e.0)], created[&(f2, e.1)]);
    m.faces.push(MeshFace { vertices: vec![a1, a2, b2, b1], uvs: vec![[0.0, 0.0]; 4], texture });
    let f = m.faces.len() - 1;
    orient_new_faces(m, &[f]);
    Ok(f)
}

/// Разрезать грань ножом (K) между точками на двух её рёбрах: (ребро, доля от его
/// меньшей вершины). Соседние грани получают новые точки. Возвращает новое ребро.
pub fn knife(m: &mut EditMesh, face: usize, first: (Edge, f32), second: (Edge, f32)) -> Result<Edge, String> {
    let Some(f) = m.faces.get(face).cloned() else { return Err("no such face".to_string()) };
    let n = f.vertices.len();
    let at = |e: Edge| (0..n).find(|i| edge(f.vertices[*i], f.vertices[(i + 1) % n]) == e);
    let (Some(i), Some(j)) = (at(first.0), at(second.0)) else {
        return Err("both cut points must lie on edges of the same face".to_string());
    };
    if i == j {
        return Err("the cut points are on the same edge".to_string());
    }
    let mut points: HashMap<(Edge, usize), u32> = HashMap::new();
    let mut ids = [0u32; 2];
    for (k, (e, t)) in [first, second].into_iter().enumerate() {
        let (a, b) = (m.points[e.0 as usize], m.points[e.1 as usize]);
        m.points.push(a + (b - a) * t.clamp(0.02, 0.98));
        ids[k] = m.points.len() as u32 - 1;
        points.insert((e, k + 1), ids[k]);
    }
    insert_edge_points(m, &points);
    // Грань с двумя новыми точками — на две по хорде.
    let g = m.faces[face].clone();
    let (p, q) = (g.vertices.iter().position(|v| *v == ids[0]).expect("inserted"), g.vertices.iter().position(|v| *v == ids[1]).expect("inserted"));
    let (lo, hi) = (p.min(q), p.max(q));
    let part = |from: usize, to: usize| -> MeshFace {
        let mut vertices = Vec::new();
        let mut uvs = Vec::new();
        let mut k = from;
        loop {
            vertices.push(g.vertices[k]);
            uvs.push(g.uvs[k]);
            if k == to {
                break;
            }
            k = (k + 1) % g.vertices.len();
        }
        MeshFace { vertices, uvs, texture: g.texture }
    };
    m.faces[face] = part(lo, hi);
    m.faces.push(part(hi, lo));
    Ok(edge(ids[0], ids[1]))
}

/// Слить вершины в одну точку: в центре (`at = None`) или в `at` (Alt+M «в последнюю»).
pub fn merge(m: &mut EditMesh, verts: &BTreeSet<u32>, at: Option<Vec3>) -> Option<u32> {
    let first = *verts.iter().next()?;
    let center = at.unwrap_or_else(|| verts.iter().fold(Vec3::ZERO, |s, v| s + m.points[*v as usize]) / verts.len() as f32);
    m.points[first as usize] = center;
    for face in &mut m.faces {
        for v in &mut face.vertices {
            if verts.contains(v) {
                *v = first;
            }
        }
    }
    m.clean();
    let remap = m.compact();
    remap.get(first as usize).copied().flatten()
}

/// Слить вершины ближе `distance` (по всему мешу). Возвращает, сколько убрано.
pub fn merge_by_distance(m: &mut EditMesh, distance: f32) -> usize {
    let before = m.points.len();
    let mut target: Vec<u32> = (0..m.points.len() as u32).collect();
    for i in 0..m.points.len() {
        if target[i] != i as u32 {
            continue;
        }
        for j in i + 1..m.points.len() {
            if target[j] == j as u32 && m.points[i].distance(m.points[j]) <= distance {
                target[j] = i as u32;
            }
        }
    }
    for face in &mut m.faces {
        for v in &mut face.vertices {
            *v = target[*v as usize];
        }
    }
    m.clean();
    m.compact();
    before - m.points.len()
}

/// Что удалить.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Delete {
    Vertices,
    Edges,
    Faces,
    /// Только грани (вершины и рёбра других граней остаются).
    OnlyFaces,
}

pub fn delete(m: &mut EditMesh, sel: &Selection, what: Delete) {
    let doomed: HashSet<usize> = match what {
        Delete::Vertices => (0..m.faces.len()).filter(|f| m.faces[*f].vertices.iter().any(|v| sel.verts.contains(v))).collect(),
        Delete::Edges => (0..m.faces.len()).filter(|f| m.face_edges(*f).iter().any(|(a, b)| sel.edges.contains(&edge(*a, *b)))).collect(),
        Delete::Faces | Delete::OnlyFaces => sel.faces.iter().copied().collect(),
    };
    let mut index = 0;
    m.faces.retain(|_| {
        let keep = !doomed.contains(&index);
        index += 1;
        keep
    });
    m.compact();
}

/// Растворить грани: каждая связная область выделенных граней с одним краем становится
/// одной гранью. Возвращает новые грани.
pub fn dissolve_faces(m: &mut EditMesh, faces: &BTreeSet<usize>) -> BTreeSet<usize> {
    // Области по общим рёбрам.
    let ef = m.edge_faces();
    let mut region_of: HashMap<usize, usize> = HashMap::new();
    let mut regions: Vec<Vec<usize>> = Vec::new();
    for &f in faces {
        if region_of.contains_key(&f) || f >= m.faces.len() {
            continue;
        }
        let r = regions.len();
        let mut list = vec![f];
        region_of.insert(f, r);
        let mut i = 0;
        while i < list.len() {
            let g = list[i];
            for (a, b) in m.face_edges(g) {
                for h in ef.get(&edge(a, b)).into_iter().flatten() {
                    if faces.contains(h) && !region_of.contains_key(h) {
                        region_of.insert(*h, r);
                        list.push(*h);
                    }
                }
            }
            i += 1;
        }
        regions.push(list);
    }
    let mut replaced: Vec<(usize, MeshFace)> = Vec::new();
    let mut removed: HashSet<usize> = HashSet::new();
    for region in regions.iter().filter(|r| r.len() > 1) {
        let set: HashSet<usize> = region.iter().copied().collect();
        // Края области: направленные рёбра без встречного внутри области.
        let mut inner: HashSet<(u32, u32)> = HashSet::new();
        for f in region {
            inner.extend(m.face_edges(*f));
        }
        let mut next: HashMap<u32, (u32, [f32; 2])> = HashMap::new();
        let mut uv_at: HashMap<u32, [f32; 2]> = HashMap::new();
        let mut ok = true;
        for f in region {
            let face = &m.faces[*f];
            for (k, (a, b)) in m.face_edges(*f).into_iter().enumerate() {
                if !inner.contains(&(b, a)) {
                    if next.insert(a, (b, face.uvs[k])).is_some() {
                        ok = false;
                    }
                    uv_at.insert(a, face.uvs[k]);
                }
            }
        }
        if !ok || next.is_empty() {
            continue;
        }
        let start = *next.keys().min().expect("non-empty");
        let mut loop_v = vec![start];
        let mut cur = start;
        while let Some((n, _)) = next.get(&cur) {
            if *n == start {
                break;
            }
            if loop_v.len() > next.len() {
                ok = false;
                break;
            }
            loop_v.push(*n);
            cur = *n;
        }
        if !ok || loop_v.len() != next.len() {
            continue; // край из нескольких петель (дырка) — не растворяется
        }
        let texture = m.faces[region[0]].texture;
        let uvs = loop_v.iter().map(|v| uv_at[v]).collect();
        replaced.push((region[0], MeshFace { vertices: loop_v, uvs, texture }));
        removed.extend(set.iter().copied().filter(|f| *f != region[0]));
    }
    for (f, face) in replaced.iter().cloned() {
        m.faces[f] = face;
    }
    let keep: Vec<usize> = replaced.iter().map(|r| r.0).collect();
    let mut index = 0;
    let mut new_index = BTreeSet::new();
    let mut position = 0;
    m.faces.retain(|_| {
        let keep_it = !removed.contains(&index);
        if keep_it {
            if keep.contains(&index) {
                new_index.insert(position);
            }
            position += 1;
        }
        index += 1;
        keep_it
    });
    remove_inline_vertices(m);
    m.clean();
    m.compact();
    new_index
}

/// Растворить рёбра: две грани по сторонам ребра — одна.
pub fn dissolve_edges(m: &mut EditMesh, edges: &BTreeSet<Edge>) {
    let ef = m.edge_faces();
    let faces: BTreeSet<usize> = edges.iter().filter_map(|e| ef.get(e)).filter(|f| f.len() == 2).flatten().copied().collect();
    // Растворяются только пары через выбранные рёбра: области строятся по всем выбранным рёбрам.
    dissolve_faces(m, &faces);
}

/// Растворить вершины: грани вокруг — одна, сама вершина убирается из её обхода.
pub fn dissolve_verts(m: &mut EditMesh, verts: &BTreeSet<u32>) {
    for v in verts.iter().rev() {
        let around: BTreeSet<usize> = (0..m.faces.len()).filter(|f| m.faces[*f].vertices.contains(v)).collect();
        if around.len() > 1 {
            dissolve_faces(m, &around);
        }
        let positions = m.points.len();
        if (*v as usize) < positions {
            for face in &mut m.faces {
                if face.vertices.len() > 3 {
                    if let Some(i) = face.vertices.iter().position(|x| x == v) {
                        face.vertices.remove(i);
                        face.uvs.remove(i);
                    }
                }
            }
        }
    }
    m.clean();
    m.compact();
}

/// Убрать из граней вершины, лежащие на прямой между соседями и не нужные другим граням.
fn remove_inline_vertices(m: &mut EditMesh) {
    let vf = m.vertex_faces();
    for f in 0..m.faces.len() {
        let mut changed = true;
        while changed && m.faces[f].vertices.len() > 3 {
            changed = false;
            let n = m.faces[f].vertices.len();
            for i in 0..n {
                let (p, v, q) = (m.faces[f].vertices[(i + n - 1) % n], m.faces[f].vertices[i], m.faces[f].vertices[(i + 1) % n]);
                if vf[v as usize].len() > 1 {
                    continue;
                }
                let (a, b, c) = (m.points[p as usize], m.points[v as usize], m.points[q as usize]);
                if (b - a).cross(c - b).length() < 1e-5 * (c - a).length().max(1.0) {
                    m.faces[f].vertices.remove(i);
                    m.faces[f].uvs.remove(i);
                    changed = true;
                    break;
                }
            }
        }
    }
}

/// Создать грань (F): из выделенной петли краевых рёбер или из выделенных вершин (по
/// углу вокруг центра). Обход — встречный соседним граням или наружу от центра меша.
pub fn fill(m: &mut EditMesh, sel: &Selection) -> Result<usize, String> {
    let verts: Vec<u32> = sel.verts.iter().copied().collect();
    if verts.len() < 3 {
        return Err("select at least 3 vertices (or a closed loop of edges) to make a face".to_string());
    }
    // Петля по краевым рёбрам, если выделение — замкнутый край.
    let ef = m.edge_faces();
    let boundary: Vec<(u32, u32)> = (0..m.faces.len())
        .flat_map(|f| m.face_edges(f))
        .filter(|(a, b)| ef.get(&edge(*a, *b)).is_some_and(|fs| fs.len() == 1) && sel.verts.contains(a) && sel.verts.contains(b))
        .collect();
    let mut order: Vec<u32> = Vec::new();
    if boundary.len() == verts.len() {
        // Край идёт по граням a → b; новая грань идёт навстречу: b → a.
        let next: HashMap<u32, u32> = boundary.iter().map(|(a, b)| (*b, *a)).collect();
        let mut cur = verts[0];
        for _ in 0..verts.len() {
            order.push(cur);
            match next.get(&cur) {
                Some(n) => cur = *n,
                None => break,
            }
        }
        if cur != verts[0] || order.len() != verts.len() {
            order.clear();
        }
    }
    if order.is_empty() {
        let pts: Vec<Vec3> = verts.iter().map(|v| m.points[*v as usize]).collect();
        let c = pts.iter().fold(Vec3::ZERO, |s, p| s + *p) / pts.len() as f32;
        let mut n = Vec3::ZERO;
        for i in 0..pts.len() {
            for j in i + 1..pts.len() {
                let k = (pts[i] - c).cross(pts[j] - c);
                n += if k.dot(n) < 0.0 { -k } else { k };
            }
        }
        let n = n.normalize_or_zero();
        if n == Vec3::ZERO {
            return Err("the selected vertices lie on one line".to_string());
        }
        let u = (pts[0] - c).normalize_or_zero();
        let w = n.cross(u);
        let mut angles: Vec<(f32, u32)> = verts.iter().zip(&pts).map(|(v, p)| ((*p - c).dot(w).atan2((*p - c).dot(u)), *v)).collect();
        angles.sort_by(|a, b| a.0.total_cmp(&b.0));
        order = angles.into_iter().map(|a| a.1).collect();
        // Наружу от центра меша.
        let center = m.points.iter().fold(Vec3::ZERO, |s, p| s + *p) / m.points.len().max(1) as f32;
        if n.dot(c - center) < 0.0 {
            order.reverse();
        }
    }
    let mut key = order.clone();
    key.sort_unstable();
    if m.faces.iter().any(|f| {
        let mut k = f.vertices.clone();
        k.sort_unstable();
        k == key
    }) {
        return Err("these vertices already have a face".to_string());
    }
    let texture = m.faces.first().map_or(0, |f| f.texture);
    m.faces.push(MeshFace { uvs: vec![[0.0, 0.0]; order.len()], vertices: order, texture });
    let f = m.faces.len() - 1;
    orient_new_faces(m, &[f]);
    if m.area(f) < 1e-7 {
        m.faces.pop();
        return Err("the new face would have no area".to_string());
    }
    Ok(f)
}

/// Развернуть грани.
pub fn flip(m: &mut EditMesh, faces: &BTreeSet<usize>) {
    for f in faces {
        if let Some(face) = m.faces.get_mut(*f) {
            face.vertices.reverse();
            face.uvs.reverse();
        }
    }
}

/// Пересчитать нормали наружу (Shift+N): обход согласуется по соседям в каждой связной
/// части, затем часть разворачивается, если её объём отрицателен (у открытой — если
/// грани в среднем смотрят к её центру).
pub fn recalc_normals(m: &mut EditMesh) {
    let ef = m.edge_faces();
    let mut done = vec![false; m.faces.len()];
    for seed in 0..m.faces.len() {
        if done[seed] {
            continue;
        }
        let mut component = vec![seed];
        done[seed] = true;
        let mut i = 0;
        while i < component.len() {
            let f = component[i];
            for (a, b) in m.face_edges(f) {
                for &g in ef.get(&edge(a, b)).into_iter().flatten() {
                    if done[g] {
                        continue;
                    }
                    // Сосед должен идти по ребру навстречу.
                    if m.face_edges(g).contains(&(a, b)) {
                        m.faces[g].vertices.reverse();
                        m.faces[g].uvs.reverse();
                    }
                    done[g] = true;
                    component.push(g);
                }
            }
            i += 1;
        }
        let part = EditMesh { points: m.points.clone(), faces: component.iter().map(|f| m.faces[*f].clone()).collect() };
        let outward = if part.is_closed_manifold() {
            part.signed_volume() >= 0.0
        } else {
            let used: Vec<Vec3> = part.faces.iter().flat_map(|f| f.vertices.iter().map(|v| m.points[*v as usize])).collect();
            let center = used.iter().fold(Vec3::ZERO, |s, p| s + *p) / used.len().max(1) as f32;
            (0..part.faces.len()).map(|f| part.normal(f).dot(part.centroid(f) - center) * part.area(f)).sum::<f32>() >= 0.0
        };
        if !outward {
            for f in &component {
                m.faces[*f].vertices.reverse();
                m.faces[*f].uvs.reverse();
            }
        }
    }
}

/// Поделить грани: n-угольник — n квадов через середины рёбер и центр; соседние грани
/// получают середины рёбер. UV интерполируются.
pub fn subdivide(m: &mut EditMesh, faces: &BTreeSet<usize>) -> BTreeSet<usize> {
    let mut mids: HashMap<(Edge, usize), u32> = HashMap::new();
    let mut out = BTreeSet::new();
    for &f in faces {
        if f >= m.faces.len() {
            continue;
        }
        let face = m.faces[f].clone();
        let n = face.vertices.len();
        let corners = m.face_points(f);
        let c = corners.iter().fold(Vec3::ZERO, |s, p| s + *p) / n as f32;
        let uc = interpolate_uv(&corners, &face.uvs, c);
        m.points.push(c);
        let center = m.points.len() as u32 - 1;
        let mut mid = Vec::with_capacity(n);
        for i in 0..n {
            let (a, b) = (face.vertices[i], face.vertices[(i + 1) % n]);
            let e = edge(a, b);
            let v = *mids.entry((e, 1)).or_insert_with(|| {
                m.points.push((m.points[a as usize] + m.points[b as usize]) * 0.5);
                m.points.len() as u32 - 1
            });
            mid.push((v, lerp2(face.uvs[i], face.uvs[(i + 1) % n], 0.5)));
        }
        let mut new = Vec::new();
        for i in 0..n {
            let prev = mid[(i + n - 1) % n];
            new.push(MeshFace {
                vertices: vec![face.vertices[i], mid[i].0, center, prev.0],
                uvs: vec![face.uvs[i], mid[i].1, uc, prev.1],
                texture: face.texture,
            });
        }
        m.faces[f] = new.remove(0);
        out.insert(f);
        for g in new {
            m.faces.push(g);
            out.insert(m.faces.len() - 1);
        }
    }
    insert_edge_points(m, &mids);
    out
}

/// Треугольники многоугольника (номера углов): веер из угла, при котором ни один
/// треугольник не вырожден и не вывернут; иначе отрезание «ушей». Точки на рёбрах
/// (после разреза у соседа) не дают пустых треугольников.
pub fn triangulate_polygon(points: &[Vec3]) -> Vec<[usize; 3]> {
    let n = points.len();
    if n < 3 {
        return Vec::new();
    }
    let normal = newell(points);
    let area = |a: usize, b: usize, c: usize| (points[b] - points[a]).cross(points[c] - points[a]).dot(normal);
    let eps = normal.length_squared() * 1e-7;
    for origin in 0..n {
        let fan: Vec<[usize; 3]> = (1..n - 1).map(|i| [origin, (origin + i) % n, (origin + i + 1) % n]).collect();
        if fan.iter().all(|t| area(t[0], t[1], t[2]) > eps) {
            return fan;
        }
    }
    // Уши: выпуклый угол без других точек внутри.
    let mut left: Vec<usize> = (0..n).collect();
    let mut out = Vec::new();
    let mut guard = 0;
    while left.len() > 3 && guard < n * n {
        guard += 1;
        let k = left.len();
        let mut clipped = false;
        for i in 0..k {
            let (a, b, c) = (left[(i + k - 1) % k], left[i], left[(i + 1) % k]);
            if area(a, b, c) <= eps {
                continue;
            }
            let inside = left.iter().any(|&p| {
                if p == a || p == b || p == c {
                    return false;
                }
                area(a, b, p) >= 0.0 && area(b, c, p) >= 0.0 && area(c, a, p) >= 0.0
            });
            if !inside {
                out.push([a, b, c]);
                left.remove(i);
                clipped = true;
                break;
            }
        }
        if !clipped {
            break;
        }
    }
    if left.len() == 3 && area(left[0], left[1], left[2]) > eps {
        out.push([left[0], left[1], left[2]]);
    }
    if out.is_empty() {
        (1..n - 1).map(|i| [0, i, i + 1]).collect()
    } else {
        out
    }
}

/// Разбить грани на треугольники (Ctrl+T).
pub fn triangulate(m: &mut EditMesh, faces: &BTreeSet<usize>) {
    for &f in faces {
        let Some(face) = m.faces.get(f).cloned() else { continue };
        if face.vertices.len() <= 3 {
            continue;
        }
        let points = m.face_points(f);
        let tris: Vec<MeshFace> = triangulate_polygon(&points)
            .into_iter()
            .filter(|t| (points[t[1]] - points[t[0]]).cross(points[t[2]] - points[t[0]]).length() > 1e-6)
            .map(|t| MeshFace { vertices: t.iter().map(|i| face.vertices[*i]).collect(), uvs: t.iter().map(|i| face.uvs[*i]).collect(), texture: face.texture })
            .collect();
        let mut tris = tris.into_iter();
        let Some(first) = tris.next() else { continue };
        m.faces[f] = first;
        m.faces.extend(tris);
    }
}

/// Пары соседних треугольников в одной плоскости — квады (Alt+J).
pub fn tris_to_quads(m: &mut EditMesh, faces: &BTreeSet<usize>) -> usize {
    let ef = m.edge_faces();
    let mut used: HashSet<usize> = HashSet::new();
    let mut merged = Vec::new();
    for &f in faces {
        if used.contains(&f) || m.faces.get(f).is_none_or(|x| x.vertices.len() != 3) {
            continue;
        }
        let mut best: Option<(f32, usize, (u32, u32))> = None;
        for (a, b) in m.face_edges(f) {
            for &g in ef.get(&edge(a, b)).into_iter().flatten() {
                if g == f || used.contains(&g) || !faces.contains(&g) || m.faces[g].vertices.len() != 3 || m.faces[g].texture != m.faces[f].texture {
                    continue;
                }
                let d = m.normal(f).dot(m.normal(g));
                let length = (m.points[a as usize] - m.points[b as usize]).length();
                if d > 0.999 && best.is_none_or(|(l, ..)| length > l) {
                    best = Some((length, g, (a, b)));
                }
            }
        }
        let Some((_, g, (a, b))) = best else { continue };
        // f: a → b → x; g: b → a → y. Квад: a → y → b → x.
        let fx = &m.faces[f];
        let ix = (0..3).find(|i| fx.vertices[*i] != a && fx.vertices[*i] != b).expect("third vertex");
        let gy = &m.faces[g];
        let iy = (0..3).find(|i| gy.vertices[*i] != a && gy.vertices[*i] != b).expect("third vertex");
        let uv_f = |v: u32| fx.uvs[fx.vertices.iter().position(|x| *x == v).unwrap_or(0)];
        let quad = MeshFace {
            vertices: vec![a, gy.vertices[iy], b, fx.vertices[ix]],
            uvs: vec![uv_f(a), gy.uvs[iy], uv_f(b), fx.uvs[ix]],
            texture: fx.texture,
        };
        // Только выпуклый квад.
        let pts: Vec<Vec3> = quad.vertices.iter().map(|v| m.points[*v as usize]).collect();
        let n = newell(&pts);
        let convex = (0..4).all(|i| (pts[(i + 1) % 4] - pts[i]).cross(pts[(i + 2) % 4] - pts[(i + 1) % 4]).dot(n) > 0.0);
        if !convex {
            continue;
        }
        used.insert(f);
        used.insert(g);
        merged.push((f, g, quad));
    }
    let count = merged.len();
    let drop: HashSet<usize> = merged.iter().map(|x| x.1).collect();
    for (f, _, quad) in merged {
        m.faces[f] = quad;
    }
    let mut index = 0;
    m.faces.retain(|_| {
        let keep = !drop.contains(&index);
        index += 1;
        keep
    });
    count
}

/// Копия граней с новыми вершинами (Shift+D); возвращает новые грани.
pub fn duplicate(m: &mut EditMesh, faces: &BTreeSet<usize>) -> BTreeSet<usize> {
    let mut copy: BTreeMap<u32, u32> = BTreeMap::new();
    let mut out = BTreeSet::new();
    for &f in faces {
        let Some(mut face) = m.faces.get(f).cloned() else { continue };
        for v in &mut face.vertices {
            *v = *copy.entry(*v).or_insert_with(|| {
                m.points.push(m.points[*v as usize]);
                m.points.len() as u32 - 1
            });
        }
        m.faces.push(face);
        out.insert(m.faces.len() - 1);
    }
    out
}

/// Оторвать грани (Y / V): у них свои вершины, остальные грани не задеты.
pub fn split(m: &mut EditMesh, faces: &BTreeSet<usize>) -> BTreeSet<usize> {
    let new = duplicate(m, faces);
    let mapping: Vec<(usize, usize)> = faces.iter().copied().zip(new.iter().copied()).collect();
    for (old, new) in mapping {
        m.faces[old] = m.faces[new].clone();
    }
    let drop: HashSet<usize> = new.into_iter().collect();
    let mut index = 0;
    m.faces.retain(|_| {
        let keep = !drop.contains(&index);
        index += 1;
        keep
    });
    m.compact();
    faces.clone()
}

/// Выделить грани в отдельный меш (P): они убираются отсюда и возвращаются.
pub fn separate(m: &mut EditMesh, faces: &BTreeSet<usize>) -> EditMesh {
    let mut out = EditMesh { points: m.points.clone(), faces: faces.iter().filter_map(|f| m.faces.get(*f).cloned()).collect() };
    out.compact();
    let mut index = 0;
    m.faces.retain(|_| {
        let keep = !faces.contains(&index);
        index += 1;
        keep
    });
    m.compact();
    out
}

/// Толщина (solidify): оболочка внутрь на `thickness` вдоль нормалей вершин, края
/// соединяются стенками. Замкнутый меш получает внутреннюю полость.
pub fn solidify(m: &mut EditMesh, thickness: f32) {
    let n = m.points.len();
    let mut normals = vec![Vec3::ZERO; n];
    for f in 0..m.faces.len() {
        let nf = m.normal(f) * m.area(f).max(1e-6);
        for v in &m.faces[f].vertices {
            normals[*v as usize] += nf;
        }
    }
    let base = m.points.len() as u32;
    for i in 0..n {
        m.points.push(m.points[i] - normals[i].normalize_or_zero() * thickness);
    }
    let ef = m.edge_faces();
    let count = m.faces.len();
    for f in 0..count {
        let mut inner = m.faces[f].clone();
        for v in &mut inner.vertices {
            *v += base;
        }
        inner.vertices.reverse();
        inner.uvs.reverse();
        m.faces.push(inner);
    }
    for f in 0..count {
        for (k, (a, b)) in m.face_edges(f).into_iter().enumerate() {
            if ef.get(&edge(a, b)).is_some_and(|fs| fs.len() == 1) {
                let face = &m.faces[f];
                let (ua, ub) = (face.uvs[k], face.uvs[(k + 1) % face.vertices.len()]);
                let texture = face.texture;
                m.faces.push(MeshFace { vertices: vec![b, a, a + base, b + base], uvs: vec![ub, ua, ua, ub], texture });
            }
        }
    }
    m.compact();
}

/// Соединить две краевые петли квадами (bridge). Петли — выделенные краевые рёбра.
pub fn bridge(m: &mut EditMesh, edges: &BTreeSet<Edge>) -> Result<Vec<usize>, String> {
    let ef = m.edge_faces();
    // Направленные краевые рёбра (как в их гранях).
    let directed: Vec<(u32, u32)> = (0..m.faces.len())
        .flat_map(|f| m.face_edges(f))
        .filter(|(a, b)| edges.contains(&edge(*a, *b)) && ef.get(&edge(*a, *b)).is_some_and(|fs| fs.len() == 1))
        .collect();
    let next: HashMap<u32, u32> = directed.iter().copied().collect();
    let mut loops: Vec<Vec<u32>> = Vec::new();
    let mut seen: HashSet<u32> = HashSet::new();
    for &(start, _) in &directed {
        if seen.contains(&start) {
            continue;
        }
        let mut list = vec![start];
        seen.insert(start);
        let mut cur = start;
        while let Some(&n) = next.get(&cur) {
            if n == start || !seen.insert(n) {
                break;
            }
            list.push(n);
            cur = n;
        }
        loops.push(list);
    }
    let [a, b] = &loops[..] else {
        return Err(format!("select two open edge loops to bridge (found {})", loops.len()));
    };
    if a.len() != b.len() {
        return Err(format!("the loops have {} and {} edges; bridge needs the same number", a.len(), b.len()));
    }
    let n = a.len();
    // b идёт навстречу a: подбираем сдвиг с наименьшим расстоянием.
    let rev: Vec<u32> = b.iter().rev().copied().collect();
    let cost = |s: usize| (0..n).map(|i| m.points[a[i] as usize].distance(m.points[rev[(i + s) % n] as usize])).sum::<f32>();
    let shift = (0..n).min_by(|x, y| cost(*x).total_cmp(&cost(*y))).unwrap_or(0);
    let texture = m.faces.first().map_or(0, |f| f.texture);
    let mut new = Vec::new();
    for i in 0..n {
        let j = (i + 1) % n;
        let (a0, a1, b0, b1) = (a[i], a[j], rev[(i + shift) % n], rev[(j + shift) % n]);
        m.faces.push(MeshFace { vertices: vec![a1, a0, b0, b1], uvs: vec![[0.0, 0.0]; 4], texture });
        new.push(m.faces.len() - 1);
    }
    orient_new_faces(m, &new);
    m.clean();
    Ok(new.into_iter().filter(|f| *f < m.faces.len()).collect())
}

/// Отражение (mirror apply) по оси `axis` относительно плоскости `plane`: копия с
/// обратным обходом, вершины на плоскости сливаются.
pub fn mirror(m: &mut EditMesh, axis: usize, plane: f32) {
    let n = m.points.len() as u32;
    let mirrored: Vec<Vec3> = m.points.iter().map(|p| {
        let mut q = *p;
        q[axis] = 2.0 * plane - q[axis];
        q
    }).collect();
    m.points.extend(mirrored);
    let count = m.faces.len();
    for f in 0..count {
        let mut face = m.faces[f].clone();
        for v in &mut face.vertices {
            // Вершины на плоскости остаются общими.
            if (m.points[*v as usize][axis] - plane).abs() > 1e-4 {
                *v += n;
            }
        }
        face.vertices.reverse();
        face.uvs.reverse();
        m.faces.push(face);
    }
    m.clean();
    m.compact();
}

/// Массив: `count` копий со сдвигом `offset`.
pub fn array(m: &mut EditMesh, count: usize, offset: Vec3) {
    let base = m.clone();
    for k in 1..count.clamp(1, 256) {
        let shift = offset * k as f32;
        let first = m.points.len() as u32;
        m.points.extend(base.points.iter().map(|p| *p + shift));
        for face in &base.faces {
            let mut f = face.clone();
            for v in &mut f.vertices {
                *v += first;
            }
            m.faces.push(f);
        }
    }
}

/// Деформация выделенных вершин вдоль оси `axis` (по их границам): сужение (`taper` —
/// масштаб на дальнем конце), скручивание (`twist` — градусы на дальнем конце).
pub fn deform(m: &mut EditMesh, verts: &BTreeSet<u32>, axis: usize, taper: f32, twist: f32) {
    if verts.is_empty() {
        return;
    }
    let (lo, hi) = verts.iter().map(|v| m.points[*v as usize][axis]).fold((f32::MAX, f32::MIN), |(a, b), x| (a.min(x), b.max(x)));
    let center = verts.iter().fold(Vec3::ZERO, |s, v| s + m.points[*v as usize]) / verts.len() as f32;
    let dir = Vec3::AXES[axis];
    for v in verts {
        let p = m.points[*v as usize];
        let t = if hi > lo { (p[axis] - lo) / (hi - lo) } else { 0.0 };
        let mut d = p - center;
        let along = dir * d.dot(dir);
        let mut across = d - along;
        across *= 1.0 + (taper - 1.0) * t;
        across = Quat::from_axis_angle(dir, (twist * t).to_radians()) * across;
        d = along + across;
        m.points[*v as usize] = center + d;
    }
}

// ----------------------------------------------------------------------
// Примитивы, вращение профиля, протяжка
// ----------------------------------------------------------------------

fn quad(a: u32, b: u32, c: u32, d: u32) -> MeshFace {
    MeshFace { vertices: vec![a, b, c, d], uvs: vec![[0.0, 0.0]; 4], texture: 0 }
}

fn poly(v: Vec<u32>) -> MeshFace {
    MeshFace { uvs: vec![[0.0, 0.0]; v.len()], vertices: v, texture: 0 }
}

/// Вращение профиля (lathe) вокруг оси Y: точки профиля — (радиус, высота) снизу вверх;
/// точки с радиусом 0 становятся полюсами (веером), концы с радиусом больше нуля
/// закрываются крышками, если `caps`. `segments` — число граней по кругу.
pub fn lathe(profile: &[Vec2], segments: usize, caps: bool) -> EditMesh {
    let segments = segments.clamp(3, 256);
    let mut m = EditMesh::default();
    let mut rows: Vec<Vec<u32>> = Vec::new();
    for p in profile {
        if p.x.abs() < 1e-6 {
            m.points.push(Vec3::new(0.0, p.y, 0.0));
            rows.push(vec![m.points.len() as u32 - 1; segments]);
        } else {
            let mut row = Vec::with_capacity(segments);
            for s in 0..segments {
                let a = std::f32::consts::TAU * s as f32 / segments as f32;
                m.points.push(Vec3::new(p.x * a.cos(), p.y, -p.x * a.sin()));
                row.push(m.points.len() as u32 - 1);
            }
            rows.push(row);
        }
    }
    for r in 0..rows.len().saturating_sub(1) {
        let (lo, hi) = (&rows[r], &rows[r + 1]);
        for s in 0..segments {
            let t = (s + 1) % segments;
            let mut v = vec![lo[s], lo[t], hi[t], hi[s]];
            v.dedup();
            if v.first() == v.last() && v.len() > 1 {
                v.pop();
            }
            if v.len() >= 3 {
                m.faces.push(poly(v));
            }
        }
    }
    if caps {
        if let (Some(first), Some(last)) = (rows.first(), rows.last()) {
            if first[0] != first[1] {
                m.faces.push(poly(first.iter().rev().copied().collect()));
            }
            if last[0] != last[1] && rows.len() > 1 {
                m.faces.push(poly(last.clone()));
            }
        }
    }
    m.clean();
    recalc_normals(&mut m);
    m
}

/// Протяжка замкнутого профиля (в плоскости XY, вокруг нуля) вдоль ломаной `path`:
/// труба с крышками на концах.
pub fn sweep(profile: &[Vec2], path: &[Vec3], caps: bool) -> EditMesh {
    let mut m = EditMesh::default();
    let n = profile.len();
    if n < 3 || path.len() < 2 {
        return m;
    }
    let mut rows = Vec::new();
    for (i, p) in path.iter().enumerate() {
        let tangent = if i == 0 {
            path[1] - path[0]
        } else if i == path.len() - 1 {
            path[i] - path[i - 1]
        } else {
            (path[i + 1] - path[i]).normalize_or_zero() + (path[i] - path[i - 1]).normalize_or_zero()
        }
        .normalize_or_zero();
        let up = if tangent.dot(Vec3::Y).abs() > 0.99 { Vec3::X } else { Vec3::Y };
        let side = up.cross(tangent).normalize_or_zero();
        let normal = tangent.cross(side);
        let mut row = Vec::new();
        for q in profile {
            m.points.push(*p + side * q.x + normal * q.y);
            row.push(m.points.len() as u32 - 1);
        }
        rows.push(row);
    }
    for r in 0..rows.len() - 1 {
        for k in 0..n {
            let j = (k + 1) % n;
            m.faces.push(quad(rows[r][k], rows[r][j], rows[r + 1][j], rows[r + 1][k]));
        }
    }
    if caps {
        m.faces.push(poly(rows[0].iter().rev().copied().collect()));
        m.faces.push(poly(rows[rows.len() - 1].clone()));
    }
    m.clean();
    recalc_normals(&mut m);
    m
}

/// Примитив.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Primitive {
    Plane { size: f32 },
    Cube { size: f32 },
    Circle { radius: f32, segments: usize },
    Cylinder { radius: f32, height: f32, segments: usize, caps: bool },
    Cone { radius: f32, height: f32, segments: usize },
    UvSphere { radius: f32, segments: usize, rings: usize },
    IcoSphere { radius: f32, subdivisions: usize },
    Torus { major: f32, minor: f32, segments: usize, sides: usize },
    Wedge { size: f32 },
    Stairs { width: f32, steps: usize, step: f32 },
    Grid { size: f32, cells: usize },
}

impl Primitive {
    pub fn name(&self) -> &'static str {
        match self {
            Primitive::Plane { .. } => "Plane",
            Primitive::Cube { .. } => "Cube",
            Primitive::Circle { .. } => "Circle",
            Primitive::Cylinder { .. } => "Cylinder",
            Primitive::Cone { .. } => "Cone",
            Primitive::UvSphere { .. } => "UV Sphere",
            Primitive::IcoSphere { .. } => "Ico Sphere",
            Primitive::Torus { .. } => "Torus",
            Primitive::Wedge { .. } => "Wedge",
            Primitive::Stairs { .. } => "Stairs",
            Primitive::Grid { .. } => "Grid",
        }
    }

    /// Примитивы меню Add Mesh с размерами по умолчанию (в единицах модели).
    pub const DEFAULTS: [Primitive; 11] = [
        Primitive::Plane { size: 16.0 },
        Primitive::Cube { size: 8.0 },
        Primitive::Circle { radius: 4.0, segments: 12 },
        Primitive::Cylinder { radius: 4.0, height: 8.0, segments: 12, caps: true },
        Primitive::Cone { radius: 4.0, height: 8.0, segments: 12 },
        Primitive::UvSphere { radius: 4.0, segments: 12, rings: 6 },
        Primitive::IcoSphere { radius: 4.0, subdivisions: 1 },
        Primitive::Torus { major: 4.0, minor: 1.5, segments: 12, sides: 6 },
        Primitive::Wedge { size: 8.0 },
        Primitive::Stairs { width: 8.0, steps: 4, step: 2.0 },
        Primitive::Grid { size: 16.0, cells: 4 },
    ];

    /// Меш примитива: стоит на земле (наименьший Y = 0, кроме плоских — на Y = 0), по центру.
    pub fn build(&self) -> EditMesh {
        let ring = |r: f32, y: f32, n: usize| -> Vec<Vec2> {
            let _ = y;
            (0..n).map(|s| {
                let a = std::f32::consts::TAU * s as f32 / n as f32;
                Vec2::new(r * a.cos(), r * a.sin())
            }).collect()
        };
        let mut m = match *self {
            Primitive::Plane { size } => grid(size, 1),
            Primitive::Grid { size, cells } => grid(size, cells.clamp(1, 64)),
            Primitive::Cube { size } => {
                let h = size * 0.5;
                box_mesh(Vec3::new(-h, 0.0, -h), Vec3::new(h, size, h))
            }
            Primitive::Circle { radius, segments } => {
                let mut m = EditMesh::default();
                for p in ring(radius, 0.0, segments.clamp(3, 256)) {
                    m.points.push(Vec3::new(p.x, 0.0, -p.y));
                }
                m.faces.push(poly((0..m.points.len() as u32).collect()));
                m
            }
            Primitive::Cylinder { radius, height, segments, caps } => {
                lathe(&[Vec2::new(radius, 0.0), Vec2::new(radius, height)], segments, caps)
            }
            Primitive::Cone { radius, height, segments } => lathe(&[Vec2::new(radius, 0.0), Vec2::new(0.0, height)], segments, true),
            Primitive::UvSphere { radius, segments, rings } => {
                let rings = rings.clamp(2, 128);
                let profile: Vec<Vec2> = (0..=rings)
                    .map(|i| {
                        let a = std::f32::consts::PI * i as f32 / rings as f32;
                        Vec2::new(if i == 0 || i == rings { 0.0 } else { radius * a.sin() }, radius - radius * a.cos())
                    })
                    .collect();
                lathe(&profile, segments, false)
            }
            Primitive::IcoSphere { radius, subdivisions } => ico(radius, subdivisions.min(3)),
            Primitive::Torus { major, minor, segments, sides } => {
                let (sides, segments) = (sides.clamp(3, 64), segments.clamp(3, 128));
                let mut m = EditMesh::default();
                for s in 0..segments {
                    let a = std::f32::consts::TAU * s as f32 / segments as f32;
                    let (out, up) = (Vec3::new(a.cos(), 0.0, -a.sin()), Vec3::Y);
                    for k in 0..sides {
                        let b = std::f32::consts::TAU * k as f32 / sides as f32;
                        m.points.push(out * (major + minor * b.cos()) + up * (minor + minor * b.sin()));
                    }
                }
                let at = |s: usize, k: usize| ((s % segments) * sides + k % sides) as u32;
                for s in 0..segments {
                    for k in 0..sides {
                        m.faces.push(quad(at(s, k), at(s + 1, k), at(s + 1, k + 1), at(s, k + 1)));
                    }
                }
                recalc_normals(&mut m);
                m
            }
            Primitive::Wedge { size } => {
                let h = size * 0.5;
                let mut m = EditMesh::default();
                m.points = vec![
                    Vec3::new(-h, 0.0, -h), Vec3::new(h, 0.0, -h), Vec3::new(h, 0.0, h), Vec3::new(-h, 0.0, h),
                    Vec3::new(-h, size, -h), Vec3::new(h, size, -h),
                ];
                m.faces = vec![
                    poly(vec![0, 1, 2, 3]), poly(vec![0, 4, 5, 1]), poly(vec![3, 2, 5, 4]), poly(vec![0, 3, 4]), poly(vec![1, 5, 2]),
                ];
                recalc_normals(&mut m);
                m
            }
            Primitive::Stairs { width, steps, step } => {
                let mut m = EditMesh::default();
                let steps = steps.clamp(1, 64);
                let h = width * 0.5;
                for k in 0..steps {
                    let part = box_mesh(Vec3::new(-h, 0.0, k as f32 * step - steps as f32 * step * 0.5), Vec3::new(h, (k + 1) as f32 * step, (k + 1) as f32 * step - steps as f32 * step * 0.5));
                    let first = m.points.len() as u32;
                    m.points.extend(part.points);
                    m.faces.extend(part.faces.into_iter().map(|mut f| {
                        for v in &mut f.vertices {
                            *v += first;
                        }
                        f
                    }));
                }
                m
            }
        };
        m.clean();
        m.compact();
        m
    }
}

fn grid(size: f32, cells: usize) -> EditMesh {
    let mut m = EditMesh::default();
    let h = size * 0.5;
    let step = size / cells as f32;
    for j in 0..=cells {
        for i in 0..=cells {
            m.points.push(Vec3::new(-h + i as f32 * step, 0.0, -h + j as f32 * step));
        }
    }
    let at = |i: usize, j: usize| (j * (cells + 1) + i) as u32;
    for j in 0..cells {
        for i in 0..cells {
            // Сверху: против часовой стрелки (−Z вверху картинки).
            m.faces.push(quad(at(i, j), at(i, j + 1), at(i + 1, j + 1), at(i + 1, j)));
        }
    }
    m
}

/// Коробка мешем: 8 вершин, 6 квадов наружу.
pub fn box_mesh(min: Vec3, max: Vec3) -> EditMesh {
    let p = |x: bool, y: bool, z: bool| Vec3::new(if x { max.x } else { min.x }, if y { max.y } else { min.y }, if z { max.z } else { min.z });
    let points = vec![p(false, false, false), p(true, false, false), p(true, true, false), p(false, true, false), p(false, false, true), p(true, false, true), p(true, true, true), p(false, true, true)];
    let faces = vec![
        quad(0, 3, 2, 1), // −Z
        quad(4, 5, 6, 7), // +Z
        quad(0, 1, 5, 4), // −Y
        quad(3, 7, 6, 2), // +Y
        quad(0, 4, 7, 3), // −X
        quad(1, 2, 6, 5), // +X
    ];
    EditMesh { points, faces }
}

fn ico(radius: f32, subdivisions: usize) -> EditMesh {
    let t = (1.0 + 5.0f32.sqrt()) / 2.0;
    let raw = [
        (-1.0, t, 0.0), (1.0, t, 0.0), (-1.0, -t, 0.0), (1.0, -t, 0.0), (0.0, -1.0, t), (0.0, 1.0, t),
        (0.0, -1.0, -t), (0.0, 1.0, -t), (t, 0.0, -1.0), (t, 0.0, 1.0), (-t, 0.0, -1.0), (-t, 0.0, 1.0),
    ];
    let mut m = EditMesh { points: raw.iter().map(|(x, y, z)| Vec3::new(*x, *y, *z).normalize()).collect(), faces: Vec::new() };
    let tris = [
        [0, 11, 5], [0, 5, 1], [0, 1, 7], [0, 7, 10], [0, 10, 11], [1, 5, 9], [5, 11, 4], [11, 10, 2], [10, 7, 6], [7, 1, 8],
        [3, 9, 4], [3, 4, 2], [3, 2, 6], [3, 6, 8], [3, 8, 9], [4, 9, 5], [2, 4, 11], [6, 2, 10], [8, 6, 7], [9, 8, 1],
    ];
    m.faces = tris.iter().map(|t| poly(t.to_vec())).collect();
    for _ in 0..subdivisions {
        let mut mids: HashMap<Edge, u32> = HashMap::new();
        let mut faces = Vec::new();
        for f in &m.faces {
            let v = &f.vertices;
            let mut mid = |a: u32, b: u32, pts: &mut Vec<Vec3>| -> u32 {
                *mids.entry(edge(a, b)).or_insert_with(|| {
                    pts.push(((pts[a as usize] + pts[b as usize]) * 0.5).normalize());
                    pts.len() as u32 - 1
                })
            };
            let (a, b, c) = (v[0], v[1], v[2]);
            let ab = mid(a, b, &mut m.points);
            let bc = mid(b, c, &mut m.points);
            let ca = mid(c, a, &mut m.points);
            faces.extend([poly(vec![a, ab, ca]), poly(vec![b, bc, ab]), poly(vec![c, ca, bc]), poly(vec![ab, bc, ca])]);
        }
        m.faces = faces;
    }
    for p in &mut m.points {
        *p = *p * radius + Vec3::new(0.0, radius, 0.0);
    }
    recalc_normals(&mut m);
    m
}

/// Кубоид → меш с теми же UV сторон (Convert Cube to Mesh). Не кубоид — `None`.
pub fn cuboid_to_mesh(doc: &Document, element: &Element) -> Option<Element> {
    if element.shape != crate::doc::Shape::Cuboid {
        return None;
    }
    let mut built = crate::geometry::BuiltMesh::default();
    // Без поворота: поворот и точка вращения переходят в меш как есть.
    let unrotated = Element { rotation: Vec3::ZERO, ..element.clone() };
    crate::geometry::build_element(doc, NodeId(0), &unrotated, glam::Mat4::IDENTITY, &mut built);
    let mut m = EditMesh { points: built.vertices.iter().map(|v| v.position).collect(), faces: Vec::new() };
    for t in &built.triangles {
        let size = doc.textures.get(t.texture as usize).map_or([1.0, 1.0], |x| [x.image.width() as f32, x.image.height() as f32]);
        m.faces.push(MeshFace {
            vertices: t.indices.to_vec(),
            uvs: t.indices.iter().map(|i| {
                let uv = built.vertices[*i as usize].uv;
                [uv.x * size[0], uv.y * size[1]]
            }).collect(),
            texture: t.texture,
        });
    }
    // Треугольники сторон — снова квады; углы сторон — общие вершины.
    let all: BTreeSet<usize> = (0..m.faces.len()).collect();
    tris_to_quads(&mut m, &all);
    merge_by_distance(&mut m, 1e-4);
    let mut out = Element::mesh(&m.points, m.faces);
    out.pivot = element.pivot;
    out.rotation = element.rotation;
    out.faces[crate::doc::FaceDir::North.index()].tint = element.faces[0].tint;
    Some(out)
}
