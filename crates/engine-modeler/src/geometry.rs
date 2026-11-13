//! Сборка треугольного меша из документа: для окна редактора, выбора лучом, рисования
//! по модели и экспорта.
//!
//! Нормали плоские (низкополигональный вид), обход треугольников — против часовой
//! стрелки, если смотреть снаружи. UV нормированы на размер текстуры стороны, (0, 0) —
//! левый верхний угол.

use glam::{Mat4, Vec2, Vec3};

use crate::doc::{Document, Element, Face, FaceDir, NodeId, Shape, UvRect};
use crate::image::Rgba;

/// Вершина собранного меша.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Vertex {
    pub position: Vec3,
    pub normal: Vec3,
    /// UV в долях текстуры.
    pub uv: Vec2,
    /// Оттенок стороны.
    pub color: Rgba,
}

/// Треугольник и откуда он: узел, сторона, текстура.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Triangle {
    pub indices: [u32; 3],
    pub node: NodeId,
    pub face: FaceDir,
    pub texture: u32,
    pub emissive: bool,
}

/// Многоугольник стороны — для каркаса, подсветки и обратного отображения тексель → точка.
#[derive(Clone, Debug, PartialEq)]
pub struct Polygon {
    pub node: NodeId,
    pub face: FaceDir,
    pub points: Vec<Vec3>,
    /// Для четырёхугольной стороны кубоида и плоскости — UV углов (в текселях) в порядке
    /// «левый верхний, правый верхний, правый нижний, левый нижний», как `points`.
    pub uv_corners: Option<[[f32; 2]; 4]>,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct BuiltMesh {
    pub vertices: Vec<Vertex>,
    pub triangles: Vec<Triangle>,
    pub polygons: Vec<Polygon>,
}

impl BuiltMesh {
    pub fn triangle_positions(&self, triangle: &Triangle) -> [Vec3; 3] {
        triangle.indices.map(|i| self.vertices[i as usize].position)
    }

    pub fn bounds(&self) -> Option<(Vec3, Vec3)> {
        let mut iter = self.vertices.iter().map(|v| v.position);
        let first = iter.next()?;
        Some(iter.fold((first, first), |(min, max), p| (min.min(p), max.max(p))))
    }
}

/// Что собирать.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct BuildOptions {
    /// Собирать и скрытые в окне узлы (экспорт — да, окно — нет).
    pub include_hidden: bool,
    /// Множитель координат (экспорт в метры — `1 / units_per_metre`).
    pub scale: f32,
}

impl Default for BuildOptions {
    fn default() -> Self {
        Self {
            include_hidden: false,
            scale: 1.0,
        }
    }
}

impl BuildOptions {
    /// Для экспорта и движка: всё, в метрах.
    pub fn export(doc: &Document) -> Self {
        Self {
            include_hidden: true,
            scale: 1.0 / doc.units_per_metre.max(1e-6),
        }
    }
}

/// Собрать меш всех элементов документа.
pub fn build_mesh(doc: &Document, options: BuildOptions) -> BuiltMesh {
    let mut mesh = BuiltMesh::default();
    for node in &doc.nodes {
        let Some(element) = node.element() else { continue };
        if !options.include_hidden && !doc.is_visible(node.id) {
            continue;
        }
        let matrix = Mat4::from_scale(Vec3::splat(options.scale)) * doc.world_matrix(node.id);
        build_element(doc, node.id, element, matrix, &mut mesh);
    }
    mesh
}

/// Собрать один элемент с матрицей `matrix`.
pub fn build_element(doc: &Document, id: NodeId, element: &Element, matrix: Mat4, mesh: &mut BuiltMesh) {
    let mut emit = |face: FaceDir, points: &[Vec3], uvs: &[[f32; 2]], quad: bool| {
        let f = element.face(face);
        if !f.enabled || points.len() < 3 {
            return;
        }
        let world: Vec<Vec3> = points.iter().map(|p| matrix.transform_point3(*p)).collect();
        // Нормаль Ньюэлла: устойчива и для слегка неплоских многоугольников.
        let mut normal = Vec3::ZERO;
        for i in 0..world.len() {
            let (a, b) = (world[i], world[(i + 1) % world.len()]);
            normal += Vec3::new(
                (a.y - b.y) * (a.z + b.z),
                (a.z - b.z) * (a.x + b.x),
                (a.x - b.x) * (a.y + b.y),
            );
        }
        if normal.length_squared() < 1e-12 {
            return; // вырожденная сторона (нулевая толщина)
        }
        // Углы идут по часовой стрелке снаружи, а формула даёт нормаль для обхода против.
        let normal = -normal.normalize();
        let size = texture_size(doc, f);
        let base = mesh.vertices.len() as u32;
        for (p, uv) in world.iter().zip(uvs) {
            mesh.vertices.push(Vertex {
                position: *p,
                normal,
                uv: Vec2::new(uv[0] / size[0], uv[1] / size[1]),
                color: f.tint,
            });
        }
        // Углы перечислены по часовой стрелке (как их видно снаружи): веер в обратном
        // порядке даёт обход против часовой.
        for i in 1..world.len() as u32 - 1 {
            mesh.triangles.push(Triangle {
                indices: [base, base + i + 1, base + i],
                node: id,
                face,
                texture: f.texture,
                emissive: f.emissive,
            });
        }
        mesh.polygons.push(Polygon {
            node: id,
            face,
            points: world,
            uv_corners: quad.then(|| [uvs[0], uvs[1], uvs[2], uvs[3]]),
        });
    };

    let a = element.from - Vec3::splat(element.inflate);
    let b = element.to() + Vec3::splat(element.inflate);
    match element.shape {
        Shape::Cuboid => {
            for dir in FaceDir::ALL {
                let corners = cuboid_face(a, b, dir);
                emit(dir, &corners, &element.face(dir).uv.corners(), true);
            }
        }
        Shape::Plane => {
            let y = element.from.y;
            for dir in [FaceDir::Up, FaceDir::Down] {
                let corners = cuboid_face(Vec3::new(a.x, y, a.z), Vec3::new(b.x, y, b.z), dir);
                emit(dir, &corners, &element.face(dir).uv.corners(), true);
            }
        }
        Shape::Cylinder { segments } | Shape::Cone { segments } => {
            let cone = matches!(element.shape, Shape::Cone { .. });
            let n = segments.clamp(3, 64);
            let center = (a + b) * 0.5;
            let radius = Vec2::new((b.x - a.x) * 0.5, (b.z - a.z) * 0.5);
            let ring = |i: u32, y: f32| {
                let theta = std::f32::consts::TAU * i as f32 / n as f32;
                Vec3::new(center.x + radius.x * theta.sin(), y, center.z - radius.y * theta.cos())
            };
            let side = element.face(FaceDir::North).uv;
            for i in 0..n {
                // Слева направо, как сторону видно снаружи, угол убывает.
                let k = (n - 1 - i) as f32;
                let (s0, s1) = (k / n as f32, (k + 1.0) / n as f32);
                if cone {
                    let apex = Vec3::new(center.x, b.y, center.z);
                    let points = [apex, ring(i, a.y), ring(i + 1, a.y)];
                    let uvs = [
                        face_uv(&side, (s0 + s1) * 0.5, 0.0),
                        face_uv(&side, s1, 1.0),
                        face_uv(&side, s0, 1.0),
                    ];
                    // Апекс, правый нижний, левый нижний — по часовой стрелке снаружи.
                    emit(FaceDir::North, &[points[0], points[1], points[2]], &[uvs[0], uvs[1], uvs[2]], false);
                } else {
                    let points = [ring(i + 1, b.y), ring(i, b.y), ring(i, a.y), ring(i + 1, a.y)];
                    let uvs = [
                        face_uv(&side, s0, 0.0),
                        face_uv(&side, s1, 0.0),
                        face_uv(&side, s1, 1.0),
                        face_uv(&side, s0, 1.0),
                    ];
                    emit(FaceDir::North, &points, &uvs, false);
                }
            }
            let cap = |dir: FaceDir, y: f32| -> (Vec<Vec3>, Vec<[f32; 2]>) {
                let rect = element.face(dir).uv;
                // Снаружи сверху угол растёт по часовой стрелке, снизу — против.
                let order: Vec<u32> = if dir == FaceDir::Up { (0..n).collect() } else { (0..n).rev().collect() };
                let points: Vec<Vec3> = order.iter().map(|i| ring(*i, y)).collect();
                let uvs = points.iter().map(|p| cap_uv(&rect, dir, *p, a, b)).collect();
                (points, uvs)
            };
            if !cone {
                let (points, uvs) = cap(FaceDir::Up, b.y);
                emit(FaceDir::Up, &points, &uvs, false);
            }
            let (points, uvs) = cap(FaceDir::Down, a.y);
            emit(FaceDir::Down, &points, &uvs, false);
        }
        Shape::Sphere { segments, rings } => {
            let n = segments.clamp(3, 64);
            let m = rings.clamp(2, 32);
            let center = (a + b) * 0.5;
            let radius = (b - a) * 0.5;
            let point = |i: u32, j: u32| {
                let theta = std::f32::consts::TAU * i as f32 / n as f32;
                let phi = std::f32::consts::PI * j as f32 / m as f32;
                Vec3::new(
                    center.x + radius.x * phi.sin() * theta.sin(),
                    center.y + radius.y * phi.cos(),
                    center.z - radius.z * phi.sin() * theta.cos(),
                )
            };
            let rect = element.face(FaceDir::North).uv;
            for j in 0..m {
                let (t0, t1) = (j as f32 / m as f32, (j + 1) as f32 / m as f32);
                for i in 0..n {
                    let k = (n - 1 - i) as f32;
                    let (s0, s1) = (k / n as f32, (k + 1.0) / n as f32);
                    let quad = [point(i + 1, j), point(i, j), point(i, j + 1), point(i + 1, j + 1)];
                    let uvs = [
                        face_uv(&rect, s0, t0),
                        face_uv(&rect, s1, t0),
                        face_uv(&rect, s1, t1),
                        face_uv(&rect, s0, t1),
                    ];
                    if j == 0 {
                        // У полюса верхние углы совпадают: треугольник.
                        emit(FaceDir::North, &[quad[0], quad[2], quad[3]], &[uvs[0], uvs[2], uvs[3]], false);
                    } else if j == m - 1 {
                        emit(FaceDir::North, &[quad[0], quad[1], quad[2]], &[uvs[0], uvs[1], uvs[2]], false);
                    } else {
                        emit(FaceDir::North, &quad, &uvs, false);
                    }
                }
            }
        }
    }
}

fn texture_size(doc: &Document, face: &Face) -> [f32; 2] {
    let texture = doc
        .textures
        .get(face.texture as usize)
        .or_else(|| doc.textures.first());
    texture.map_or([1.0, 1.0], |t| [t.image.width() as f32, t.image.height() as f32])
}

/// Углы стороны кубоида `a…b` в порядке «левый верхний, правый верхний, правый нижний,
/// левый нижний», если смотреть на сторону снаружи.
///
/// Вверх у боковых сторон — +Y; у верхней — к югу (+Z, её нижний край совпадает с верхним
/// краем северной стороны), у нижней — к северу (−Z).
pub fn cuboid_face(a: Vec3, b: Vec3, dir: FaceDir) -> [Vec3; 4] {
    let p = Vec3::new;
    match dir {
        FaceDir::North => [p(b.x, b.y, a.z), p(a.x, b.y, a.z), p(a.x, a.y, a.z), p(b.x, a.y, a.z)],
        FaceDir::South => [p(a.x, b.y, b.z), p(b.x, b.y, b.z), p(b.x, a.y, b.z), p(a.x, a.y, b.z)],
        FaceDir::East => [p(b.x, b.y, b.z), p(b.x, b.y, a.z), p(b.x, a.y, a.z), p(b.x, a.y, b.z)],
        FaceDir::West => [p(a.x, b.y, a.z), p(a.x, b.y, b.z), p(a.x, a.y, b.z), p(a.x, a.y, a.z)],
        FaceDir::Up => [p(b.x, b.y, b.z), p(a.x, b.y, b.z), p(a.x, b.y, a.z), p(b.x, b.y, a.z)],
        FaceDir::Down => [p(b.x, a.y, a.z), p(a.x, a.y, a.z), p(a.x, a.y, b.z), p(b.x, a.y, b.z)],
    }
}

/// Точка прямоугольника стороны: `s` — слева направо, `t` — сверху вниз (0…1), с учётом
/// поворота и отражений.
pub fn face_uv(rect: &UvRect, s: f32, t: f32) -> [f32; 2] {
    let [tl, tr, br, bl] = rect.corners();
    let lerp = |p: [f32; 2], q: [f32; 2], k: f32| [p[0] + (q[0] - p[0]) * k, p[1] + (q[1] - p[1]) * k];
    let top = lerp(tl, tr, s);
    let bottom = lerp(bl, br, s);
    lerp(top, bottom, t)
}

/// UV точки крышки цилиндра: проекция на прямоугольник стороны Up / Down.
fn cap_uv(rect: &UvRect, dir: FaceDir, p: Vec3, a: Vec3, b: Vec3) -> [f32; 2] {
    let w = (b.x - a.x).max(1e-6);
    let d = (b.z - a.z).max(1e-6);
    let s = (b.x - p.x) / w;
    let t = if dir == FaceDir::Up { (b.z - p.z) / d } else { (p.z - a.z) / d };
    face_uv(rect, s, t)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::doc::{Node, NodeKind};

    fn doc_with(element: Element) -> Document {
        let mut doc = Document::new("t");
        doc.nodes.push(Node {
            id: NodeId(1),
            name: "e".into(),
            parent: None,
            visible: true,
            locked: false,
            kind: NodeKind::Element(element),
        });
        doc.next_id = 2;
        doc
    }

    #[test]
    fn cuboid_has_six_outward_faces_with_ccw_winding() {
        let mut element = Element::cuboid(Vec3::new(-2.0, 0.0, -3.0), Vec3::new(4.0, 6.0, 8.0));
        crate::uv::layout_box(&mut element, [0, 0], 1.0);
        let doc = doc_with(element);
        let mesh = build_mesh(&doc, BuildOptions::default());
        assert_eq!(mesh.triangles.len(), 12);
        assert_eq!(mesh.vertices.len(), 24);
        assert_eq!(mesh.polygons.len(), 6);
        let center = Vec3::new(0.0, 3.0, 1.0);
        for triangle in &mesh.triangles {
            let [a, b, c] = mesh.triangle_positions(triangle);
            let geometric = (b - a).cross(c - a).normalize();
            let stored = mesh.vertices[triangle.indices[0] as usize].normal;
            assert!((geometric - stored).length() < 1e-5, "winding matches the normal");
            assert!((geometric - triangle.face.normal()).length() < 1e-5, "{:?} faces outward", triangle.face);
            assert!(geometric.dot(a - center) > 0.0);
        }
        // UV каждой стороны лежит в её прямоугольнике развёртки.
        for triangle in &mesh.triangles {
            let rect = doc.element(NodeId(1)).unwrap().face(triangle.face).uv;
            for index in triangle.indices {
                let uv = mesh.vertices[index as usize].uv * 64.0;
                assert!(uv.x >= rect.x - 1e-4 && uv.x <= rect.x + rect.w + 1e-4, "{uv} in {rect:?}");
                assert!(uv.y >= rect.y - 1e-4 && uv.y <= rect.y + rect.h + 1e-4, "{uv} in {rect:?}");
            }
        }
    }

    #[test]
    fn disabled_and_flat_faces_are_skipped() {
        let mut element = Element::cuboid(Vec3::ZERO, Vec3::new(4.0, 0.0, 4.0));
        element.face_mut(FaceDir::Down).enabled = false;
        let mesh = build_mesh(&doc_with(element), BuildOptions::default());
        assert_eq!(mesh.polygons.len(), 1, "only the top of a flat cuboid remains");
        assert_eq!(mesh.polygons[0].face, FaceDir::Up);
    }

    #[test]
    fn round_shapes_are_closed_and_face_outward() {
        for shape in [
            Shape::Cylinder { segments: 8 },
            Shape::Cone { segments: 6 },
            Shape::Sphere { segments: 8, rings: 6 },
        ] {
            let element = Element::new(shape, Vec3::splat(-4.0), Vec3::splat(8.0));
            let mesh = build_mesh(&doc_with(element), BuildOptions::default());
            assert!(!mesh.triangles.is_empty());
            for triangle in &mesh.triangles {
                let [a, b, c] = mesh.triangle_positions(triangle);
                let normal = (b - a).cross(c - a);
                let centroid = (a + b + c) / 3.0;
                assert!(normal.dot(centroid) > 0.0, "{shape:?}: {:?} points outward", triangle.face);
            }
            let (min, max) = mesh.bounds().unwrap();
            assert!(min.cmpge(Vec3::splat(-4.0 - 1e-4)).all() && max.cmple(Vec3::splat(4.0 + 1e-4)).all());
        }
    }
}
