//! Камера окна модели и луч из точки экрана в модель: выбор элемента, сторона под
//! курсором, тексель для кисти.

use glam::{Mat4, Vec2, Vec3, Vec4Swizzles};

use crate::doc::{Document, FaceDir, NodeId};
use crate::geometry::BuiltMesh;

/// Готовые виды (клавиши 1 / 3 / 7, с Ctrl — обратные).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ViewPreset {
    /// Спереди: лицо модели — северная сторона (−Z), как «вперёд» у объектов движка.
    Front,
    Back,
    Right,
    Left,
    Top,
    Bottom,
}

/// Орбитальная камера.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Camera {
    pub target: Vec3,
    /// Поворот вокруг вертикали, радианы; 0 — камера на +Z.
    pub yaw: f32,
    /// Наклон: 0 — сбоку, π/2 — сверху.
    pub pitch: f32,
    pub distance: f32,
    pub orthographic: bool,
    pub fov_deg: f32,
}

impl Default for Camera {
    fn default() -> Self {
        Self {
            target: Vec3::new(0.0, 6.0, 0.0),
            yaw: std::f32::consts::PI + 0.65,
            pitch: 0.42,
            distance: 64.0,
            orthographic: false,
            fov_deg: 45.0,
        }
    }
}

/// Луч в пространстве модели.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Ray {
    pub origin: Vec3,
    pub dir: Vec3,
}

impl Ray {
    pub fn at(&self, t: f32) -> Vec3 {
        self.origin + self.dir * t
    }

    /// Пересечение с плоскостью `point · normal = d` (точка на плоскости и нормаль).
    pub fn plane(&self, point: Vec3, normal: Vec3) -> Option<f32> {
        let denom = self.dir.dot(normal);
        if denom.abs() < 1e-8 {
            return None;
        }
        let t = (point - self.origin).dot(normal) / denom;
        (t >= 0.0).then_some(t)
    }
}

const MAX_PITCH: f32 = 1.5607; // чуть меньше π/2: вид сверху без вырожденного «вверх»

impl Camera {
    pub fn forward_dir(&self) -> Vec3 {
        -Vec3::new(
            self.yaw.sin() * self.pitch.cos(),
            self.pitch.sin(),
            self.yaw.cos() * self.pitch.cos(),
        )
    }

    pub fn eye(&self) -> Vec3 {
        self.target - self.forward_dir() * self.distance
    }

    pub fn view(&self) -> Mat4 {
        Mat4::look_at_rh(self.eye(), self.target, Vec3::Y)
    }

    /// Ближняя и дальняя плоскости — от расстояния до цели.
    fn clip_planes(&self) -> (f32, f32) {
        (self.distance * 0.01, self.distance * 40.0 + 1000.0)
    }

    pub fn projection(&self, aspect: f32) -> Mat4 {
        let (near, far) = self.clip_planes();
        if self.orthographic {
            let half_h = self.distance * (self.fov_deg.to_radians() * 0.5).tan();
            let half_w = half_h * aspect;
            // В ортографии камера отодвигается назад, чтобы ничего не отсекалось спереди.
            Mat4::orthographic_rh(-half_w, half_w, -half_h, half_h, -far, far)
        } else {
            Mat4::perspective_rh(self.fov_deg.to_radians(), aspect.max(1e-3), near, far)
        }
    }

    pub fn view_proj(&self, aspect: f32) -> Mat4 {
        self.projection(aspect) * self.view()
    }

    /// Луч через точку `ndc` (−1…1, Y вверх).
    pub fn ray(&self, ndc: Vec2, aspect: f32) -> Ray {
        let inverse = self.view_proj(aspect).inverse();
        let unproject = |z: f32| {
            let p = inverse * glam::Vec4::new(ndc.x, ndc.y, z, 1.0);
            p.xyz() / p.w
        };
        let (a, b) = (unproject(0.0), unproject(1.0));
        let dir = (b - a).normalize_or(self.forward_dir());
        let origin = if self.orthographic { a } else { self.eye() };
        Ray { origin, dir }
    }

    /// Повернуть орбиту (радианы).
    pub fn orbit(&mut self, d_yaw: f32, d_pitch: f32) {
        self.yaw = (self.yaw + d_yaw).rem_euclid(std::f32::consts::TAU);
        self.pitch = (self.pitch + d_pitch).clamp(-MAX_PITCH, MAX_PITCH);
    }

    /// Сдвинуть цель в плоскости экрана: `dx`, `dy` — в долях высоты окна (Y вниз).
    pub fn pan(&mut self, dx: f32, dy: f32) {
        let forward = self.forward_dir();
        let right = forward.cross(Vec3::Y).normalize_or(Vec3::X);
        let up = right.cross(forward);
        let span = 2.0 * self.distance * (self.fov_deg.to_radians() * 0.5).tan();
        self.target += (-right * dx + up * dy) * span;
    }

    /// Приблизить (`factor < 1`) или отдалить.
    pub fn zoom(&mut self, factor: f32) {
        self.distance = (self.distance * factor).clamp(1.0, 4000.0);
    }

    pub fn set_preset(&mut self, preset: ViewPreset) {
        use std::f32::consts::{FRAC_PI_2, PI};
        let (yaw, pitch) = match preset {
            ViewPreset::Front => (PI, 0.0),
            ViewPreset::Back => (0.0, 0.0),
            ViewPreset::Right => (FRAC_PI_2, 0.0),
            ViewPreset::Left => (-FRAC_PI_2, 0.0),
            ViewPreset::Top => (PI, MAX_PITCH),
            ViewPreset::Bottom => (PI, -MAX_PITCH),
        };
        self.yaw = yaw.rem_euclid(std::f32::consts::TAU);
        self.pitch = pitch;
    }

    /// Вписать границы в кадр.
    pub fn frame(&mut self, min: Vec3, max: Vec3) {
        let radius = ((max - min).length() * 0.5).max(2.0);
        self.target = (min + max) * 0.5;
        self.distance = radius / (self.fov_deg.to_radians() * 0.5).sin() * 1.05;
    }
}

/// Попадание луча в модель.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Hit {
    pub node: NodeId,
    pub face: FaceDir,
    pub texture: u32,
    pub t: f32,
    pub point: Vec3,
    pub normal: Vec3,
    /// UV точки (в долях текстуры).
    pub uv: Vec2,
    pub triangle: usize,
}

/// Ближайший треугольник, повёрнутый к лучу лицом. `filter` отбрасывает узлы
/// (запертые, скрытые).
pub fn raycast(mesh: &BuiltMesh, ray: Ray, filter: impl Fn(NodeId) -> bool) -> Option<Hit> {
    let mut best: Option<Hit> = None;
    for (index, triangle) in mesh.triangles.iter().enumerate() {
        if !filter(triangle.node) {
            continue;
        }
        let [a, b, c] = mesh.triangle_positions(triangle);
        let (e1, e2) = (b - a, c - a);
        let normal = e1.cross(e2);
        if normal.dot(ray.dir) >= 0.0 {
            continue; // изнанка
        }
        let p = ray.dir.cross(e2);
        let det = e1.dot(p);
        if det.abs() < 1e-12 {
            continue;
        }
        let inv = 1.0 / det;
        let s = ray.origin - a;
        let u = s.dot(p) * inv;
        if !(-1e-6..=1.0 + 1e-6).contains(&u) {
            continue;
        }
        let q = s.cross(e1);
        let v = ray.dir.dot(q) * inv;
        if v < -1e-6 || u + v > 1.0 + 1e-6 {
            continue;
        }
        let t = e2.dot(q) * inv;
        if t <= 1e-5 || best.is_some_and(|h| h.t <= t) {
            continue;
        }
        let [ia, ib, ic] = triangle.indices.map(|i| mesh.vertices[i as usize].uv);
        best = Some(Hit {
            node: triangle.node,
            face: triangle.face,
            texture: triangle.texture,
            t,
            point: ray.at(t),
            normal: normal.normalize(),
            uv: ia * (1.0 - u - v) + ib * u + ic * v,
            triangle: index,
        });
    }
    best
}

impl Hit {
    /// Тексель под точкой попадания: (текстура, x, y). Прижат к прямоугольнику стороны,
    /// чтобы попадание в самый край не уходило на соседнюю развёртку.
    pub fn texel(&self, doc: &Document) -> Option<(u32, i32, i32)> {
        let texture = doc.textures.get(self.texture as usize)?;
        let [w, h] = texture.image.size();
        let mut x = (self.uv.x * w as f32).floor() as i32;
        let mut y = (self.uv.y * h as f32).floor() as i32;
        if let Some(face) = doc.element(self.node).map(|e| e.face(self.face)) {
            let [x0, y0, x1, y1] = face.uv.texel_bounds();
            if x1 > x0 && y1 > y0 {
                x = x.clamp(x0, x1 - 1);
                y = y.clamp(y0, y1 - 1);
            }
        }
        Some((self.texture, x.clamp(0, w as i32 - 1), y.clamp(0, h as i32 - 1)))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::doc::Element;
    use crate::geometry::{build_mesh, BuildOptions};
    use crate::ops;

    #[test]
    fn ray_from_the_screen_center_hits_the_face_toward_the_camera() {
        let mut doc = Document::new("t");
        let id = ops::add_element(&mut doc, "c", Element::cuboid(Vec3::splat(-4.0), Vec3::splat(8.0)), None);
        let mesh = build_mesh(&doc, BuildOptions::default());
        let mut camera = Camera {
            target: Vec3::ZERO,
            ..Camera::default()
        };
        for (preset, face) in [
            (ViewPreset::Front, FaceDir::North),
            (ViewPreset::Back, FaceDir::South),
            (ViewPreset::Right, FaceDir::East),
            (ViewPreset::Left, FaceDir::West),
            (ViewPreset::Top, FaceDir::Up),
            (ViewPreset::Bottom, FaceDir::Down),
        ] {
            for ortho in [false, true] {
                camera.set_preset(preset);
                camera.orthographic = ortho;
                let hit = raycast(&mesh, camera.ray(Vec2::ZERO, 1.5), |_| true).expect("hits the cube");
                assert_eq!((hit.node, hit.face), (id, face), "{preset:?} ortho={ortho}");
                assert!((hit.point - face.normal() * 4.0).length() < 0.1, "{preset:?}: {}", hit.point);
            }
        }
    }

    #[test]
    fn hit_maps_to_the_texel_of_the_face() {
        let mut doc = Document::new("t");
        let id = ops::add_element(&mut doc, "c", Element::cuboid(Vec3::new(0.0, 0.0, 0.0), Vec3::new(4.0, 4.0, 4.0)), None);
        let mesh = build_mesh(&doc, BuildOptions::default());
        let north = doc.element(id).unwrap().face(FaceDir::North).uv;
        // Северная сторона смотрит на −Z; слева направо (снаружи) X убывает.
        // Луч в точку (3.5, 2.5) стороны: снаружи это первый тексель слева, второй сверху.
        let ray = Ray {
            origin: Vec3::new(3.5, 2.5, -10.0),
            dir: Vec3::Z,
        };
        let hit = raycast(&mesh, ray, |_| true).unwrap();
        assert_eq!(hit.face, FaceDir::North);
        let (texture, x, y) = hit.texel(&doc).unwrap();
        assert_eq!(texture, 0);
        assert_eq!((x, y), (north.x as i32, north.y as i32 + 1));
        let ray = Ray {
            origin: Vec3::new(0.2, 3.9, -10.0),
            dir: Vec3::Z,
        };
        let (_, x, y) = raycast(&mesh, ray, |_| true).unwrap().texel(&doc).unwrap();
        assert_eq!((x, y), (north.x as i32 + 3, north.y as i32), "right edge, top row");
        // Изнанка не ловится: луч изнутри наружу проходит насквозь.
        let inside = Ray {
            origin: Vec3::splat(2.0),
            dir: Vec3::NEG_Z,
        };
        assert!(raycast(&mesh, inside, |_| true).is_none());
        assert!(raycast(&mesh, ray, |n| n != id).is_none(), "the filter skips nodes");
    }
}
