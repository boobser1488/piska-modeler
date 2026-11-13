//! Программный растеризатор окна модели: z-буфер, текстуры «ближайшим соседом», плоское
//! освещение, сетка на земле с проверкой глубины.
//!
//! Почему не GPU: модели из десятков кубоидов — сотни треугольников, процессор рисует их
//! за миллисекунды, а взамен окно одинаково работает в отдельной программе, внутри
//! редактора и в тестах без видеокарты; пиксели текстуры остаются резкими без настроек.
//! Кадр перерисовывается только когда меняются модель, камера или размер окна.

use glam::{Mat4, Vec2, Vec3, Vec4};

use crate::camera::Camera;
use crate::doc::{Document, NodeId};
use crate::geometry::BuiltMesh;
use crate::image::Rgba;

/// Как рисовать модель.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Shading {
    /// Текстуры и свет.
    Textured,
    /// Только свет и оттенки сторон — форма без картинок.
    Solid,
    /// Только каркас (рисует интерфейс поверх пустого фона и сетки).
    Wireframe,
}

/// Настройки кадра.
#[derive(Clone, Debug, PartialEq)]
pub struct RenderOptions {
    pub shading: Shading,
    pub grid: bool,
    /// Шаг мелкой сетки в единицах модели.
    pub grid_step: f32,
    /// Через сколько мелких клеток — крупная линия (метр при 16 единицах).
    pub major_every: u32,
    /// Фон: верх и низ градиента.
    pub background: [Rgba; 2],
    /// Подсветить эти элементы (выделение).
    pub highlight: Vec<NodeId>,
    pub highlight_color: Rgba,
    /// Сверхвыборка: 2 — сглаженные края ценой вчетверо большей работы.
    pub supersample: u32,
}

impl Default for RenderOptions {
    fn default() -> Self {
        Self {
            shading: Shading::Textured,
            grid: true,
            grid_step: 1.0,
            major_every: 16,
            background: [[0x26, 0x28, 0x2d, 255], [0x15, 0x16, 0x19, 255]],
            highlight: Vec::new(),
            highlight_color: [0x4f, 0x92, 0xec, 255],
            supersample: 1,
        }
    }
}

/// Запрос кадра вида: всё, что нужно любому способу отрисовки (этому растеризатору или
/// видеокарте, см. `gpu`). Документ и меш делят данные с окном (`Arc`), копия дешёвая.
#[derive(Clone, Debug)]
pub struct ViewRequest {
    pub doc: Document,
    pub mesh: std::sync::Arc<BuiltMesh>,
    /// Растёт, когда меняется геометрия (а не только пиксели текстур).
    pub geometry_rev: u64,
    pub camera: Camera,
    pub options: RenderOptions,
    /// Размер кадра в пикселях.
    pub size: [u32; 2],
}

impl ViewRequest {
    /// Нарисовать запрос этим растеризатором.
    pub fn render_software(&self) -> Frame {
        let mut frame = Frame::new(self.size[0], self.size[1]);
        render(&self.doc, &self.mesh, &self.camera, &self.options, &mut frame);
        frame
    }
}

/// Картинка кадра и глубина.
#[derive(Clone, Debug, Default)]
pub struct Frame {
    pub width: u32,
    pub height: u32,
    /// RGBA, непрозрачные.
    pub pixels: Vec<Rgba>,
    depth: Vec<f32>,
}

impl Frame {
    pub fn new(width: u32, height: u32) -> Self {
        let (width, height) = (width.max(1), height.max(1));
        Self {
            width,
            height,
            pixels: vec![[0, 0, 0, 255]; (width * height) as usize],
            depth: vec![f32::INFINITY; (width * height) as usize],
        }
    }

    pub fn get(&self, x: u32, y: u32) -> Rgba {
        self.pixels[(y * self.width + x) as usize]
    }
}

/// Вершина после проекции (до деления на w).
#[derive(Clone, Copy)]
struct ClipVertex {
    clip: Vec4,
    uv: Vec2,
}

/// Яркость стороны с нормалью `n`: мягкий «полуламберт» от верхнего переднего света
/// и немного заполняющего снизу — верх светлее боков, низ темнее.
pub fn face_light(n: Vec3) -> f32 {
    let key = Vec3::new(-0.35, 0.8, -0.5).normalize();
    let fill = Vec3::new(0.55, -0.15, 0.6).normalize();
    let k = 0.5 + 0.5 * n.dot(key);
    let f = n.dot(fill).max(0.0);
    (0.46 + 0.54 * k + 0.07 * f).min(1.0)
}

/// Нарисовать документ в кадр.
pub fn render(doc: &Document, mesh: &BuiltMesh, camera: &Camera, options: &RenderOptions, frame: &mut Frame) {
    let ss = options.supersample.clamp(1, 3);
    if ss > 1 {
        let mut big = Frame::new(frame.width * ss, frame.height * ss);
        render_into(doc, mesh, camera, options, &mut big);
        downsample(&big, frame, ss);
    } else {
        render_into(doc, mesh, camera, options, frame);
    }
}

fn downsample(big: &Frame, frame: &mut Frame, ss: u32) {
    let n = ss * ss;
    for y in 0..frame.height {
        for x in 0..frame.width {
            let mut acc = [0u32; 3];
            for sy in 0..ss {
                for sx in 0..ss {
                    let p = big.get(x * ss + sx, y * ss + sy);
                    for c in 0..3 {
                        acc[c] += p[c] as u32;
                    }
                }
            }
            frame.pixels[(y * frame.width + x) as usize] = [
                ((acc[0] + n / 2) / n) as u8,
                ((acc[1] + n / 2) / n) as u8,
                ((acc[2] + n / 2) / n) as u8,
                255,
            ];
        }
    }
}

/// Полоса строк кадра `y0..y1`: каждый поток рисует свою.
struct Band<'a> {
    width: u32,
    height: u32,
    y0: u32,
    y1: u32,
    pixels: &'a mut [Rgba],
    depth: &'a mut [f32],
}

impl Band<'_> {
    fn index(&self, x: u32, y: u32) -> usize {
        ((y - self.y0) * self.width + x) as usize
    }
}

/// Кадр делится на полосы строк по числу ядер (не больше 8): треугольники и сетка
/// рисуются в каждой полосе отдельно и параллельно.
fn render_into(doc: &Document, mesh: &BuiltMesh, camera: &Camera, options: &RenderOptions, frame: &mut Frame) {
    let (w, h) = (frame.width, frame.height);
    let threads = std::thread::available_parallelism().map_or(1, |n| n.get()).clamp(1, 8) as u32;
    // Маленький кадр (тесты, миниатюры) не стоит потоков.
    let threads = if w * h < 64 * 64 { 1 } else { threads.min(h) };
    let rows = h.div_ceil(threads);
    let pixel_chunks = frame.pixels.chunks_mut((rows * w) as usize);
    let depth_chunks = frame.depth.chunks_mut((rows * w) as usize);
    std::thread::scope(|scope| {
        for (i, (pixels, depth)) in pixel_chunks.zip(depth_chunks).enumerate() {
            let y0 = i as u32 * rows;
            let mut band = Band { width: w, height: h, y0, y1: (y0 + rows).min(h), pixels, depth };
            if i as u32 + 1 == threads || threads == 1 {
                render_band(doc, mesh, camera, options, &mut band);
            } else {
                scope.spawn(move || render_band(doc, mesh, camera, options, &mut band));
            }
        }
    });
}

fn render_band(doc: &Document, mesh: &BuiltMesh, camera: &Camera, options: &RenderOptions, frame: &mut Band) {
    let (w, h) = (frame.width, frame.height);
    // Фон — вертикальный градиент.
    let [top, bottom] = options.background;
    for y in frame.y0..frame.y1 {
        let t = y as f32 / (h.max(2) - 1) as f32;
        let color: Rgba = std::array::from_fn(|c| {
            if c == 3 {
                255
            } else {
                (top[c] as f32 + (bottom[c] as f32 - top[c] as f32) * t).round() as u8
            }
        });
        let row = frame.index(0, y);
        frame.pixels[row..row + w as usize].fill(color);
    }
    frame.depth.fill(f32::INFINITY);

    let aspect = w as f32 / h as f32;
    let view_proj = camera.view_proj(aspect);

    if options.shading != Shading::Wireframe {
        for triangle in &mesh.triangles {
            let verts = triangle.indices.map(|i| mesh.vertices[i as usize]);
            let normal = verts[0].normal;
            let light = if triangle.emissive { 1.0 } else { face_light(normal) };
            let texture = doc
                .textures
                .get(triangle.texture as usize)
                .or_else(|| doc.textures.first())
                .map(|t| &t.image);
            let tint = verts[0].color;
            let highlighted = options.highlight.contains(&triangle.node);
            let clip = verts.map(|v| ClipVertex {
                clip: view_proj * v.position.extend(1.0),
                uv: v.uv,
            });
            let shading = options.shading;
            let accent = options.highlight_color;
            draw_triangle(frame, clip, |uv| {
                let base = match (shading, texture) {
                    (Shading::Textured, Some(image)) => {
                        let x = ((uv.x * image.width() as f32).floor() as i32).clamp(0, image.width() as i32 - 1);
                        let y = ((uv.y * image.height() as f32).floor() as i32).clamp(0, image.height() as i32 - 1);
                        let c = image.get(x, y).unwrap_or([0, 0, 0, 0]);
                        if c[3] < 128 {
                            return None; // вырезка по прозрачности
                        }
                        c
                    }
                    _ => [0xb4, 0xb2, 0xac, 255],
                };
                let mut rgb = [0f32; 3];
                for c in 0..3 {
                    rgb[c] = base[c] as f32 * tint[c] as f32 / 255.0 * light;
                }
                if highlighted {
                    for c in 0..3 {
                        rgb[c] = rgb[c] * 0.82 + accent[c] as f32 * 0.18;
                    }
                }
                Some([rgb[0].min(255.0) as u8, rgb[1].min(255.0) as u8, rgb[2].min(255.0) as u8, 255])
            });
        }
    }

    // Опорные меши — светло-серые, с той же глубиной.
    for reference in doc.references.iter().filter(|r| r.visible) {
        for tri in reference.indices.chunks_exact(3) {
            let p = [0, 1, 2].map(|k| Vec3::from_array(reference.positions[tri[k] as usize]));
            let n = (p[1] - p[0]).cross(p[2] - p[0]).normalize_or_zero();
            let light = face_light(n);
            let shade = (150.0 * light) as u8;
            let color = [shade, shade.saturating_add(4), shade.saturating_add(12), 255];
            let clip = p.map(|q| ClipVertex {
                clip: view_proj * q.extend(1.0),
                uv: Vec2::ZERO,
            });
            // Опорный меш виден с обеих сторон.
            draw_triangle(frame, clip, |_| Some(color));
            draw_triangle(frame, [clip[0], clip[2], clip[1]], |_| Some(color));
        }
    }

    if options.grid {
        draw_grid(frame, camera, &view_proj, options);
    }
}

/// Растеризация треугольника с отсечением по ближней плоскости и проверкой глубины.
/// `shade(uv)` даёт цвет пикселя или `None` (прозрачно).
fn draw_triangle(frame: &mut Band, verts: [ClipVertex; 3], mut shade: impl FnMut(Vec2) -> Option<Rgba>) {
    // Отсечение по ближней плоскости (z ≥ 0 в пространстве отсечения wgpu / glam).
    let mut poly: Vec<ClipVertex> = Vec::with_capacity(4);
    for i in 0..3 {
        let (a, b) = (verts[i], verts[(i + 1) % 3]);
        let (da, db) = (a.clip.z, b.clip.z);
        if da >= 0.0 {
            poly.push(a);
        }
        if (da >= 0.0) != (db >= 0.0) {
            let t = da / (da - db);
            poly.push(ClipVertex {
                clip: a.clip + (b.clip - a.clip) * t,
                uv: a.uv + (b.uv - a.uv) * t,
            });
        }
    }
    if poly.len() < 3 {
        return;
    }
    for i in 1..poly.len() - 1 {
        raster(frame, [poly[0], poly[i], poly[i + 1]], &mut shade);
    }
}

fn raster(frame: &mut Band, v: [ClipVertex; 3], shade: &mut impl FnMut(Vec2) -> Option<Rgba>) {
    let (w, h) = (frame.width as f32, frame.height as f32);
    let mut screen = [Vec2::ZERO; 3];
    let mut inv_w = [0f32; 3];
    let mut depth = [0f32; 3];
    for i in 0..3 {
        let c = v[i].clip;
        if c.w <= 1e-6 {
            return;
        }
        let iw = 1.0 / c.w;
        inv_w[i] = iw;
        screen[i] = Vec2::new((c.x * iw * 0.5 + 0.5) * w, (0.5 - c.y * iw * 0.5) * h);
        depth[i] = c.z * iw;
    }
    // Площадь со знаком: в экранных координатах (Y вниз) лицевой треугольник обходится
    // по часовой стрелке, и площадь отрицательна.
    let area = (screen[1] - screen[0]).perp_dot(screen[2] - screen[0]);
    if area >= -1e-9 {
        return; // изнанка или вырожденный
    }
    let min = screen[0].min(screen[1]).min(screen[2]).max(Vec2::ZERO);
    let max = screen[0].max(screen[1]).max(screen[2]).min(Vec2::new(w - 1.0, h - 1.0));
    if min.x > max.x || min.y > max.y {
        return;
    }
    let (x0, x1) = (min.x.floor() as i32, max.x.ceil() as i32);
    let (y0, y1) = (min.y.floor() as i32, max.y.ceil() as i32);
    let inv_area = 1.0 / area;
    let uvw = [v[0].uv * inv_w[0], v[1].uv * inv_w[1], v[2].uv * inv_w[2]];
    for y in y0.max(frame.y0 as i32)..=y1.min(frame.y1 as i32 - 1) {
        let py = y as f32 + 0.5;
        for x in x0.max(0)..=x1.min(frame.width as i32 - 1) {
            let p = Vec2::new(x as f32 + 0.5, py);
            let b0 = (screen[2] - screen[1]).perp_dot(p - screen[1]) * inv_area;
            let b1 = (screen[0] - screen[2]).perp_dot(p - screen[2]) * inv_area;
            let b2 = 1.0 - b0 - b1;
            // Правило верхнего левого края упрощено небольшим допуском.
            if b0 < -1e-5 || b1 < -1e-5 || b2 < -1e-5 {
                continue;
            }
            let z = b0 * depth[0] + b1 * depth[1] + b2 * depth[2];
            let index = frame.index(x as u32, y as u32);
            if z >= frame.depth[index] || !(0.0..=1.0).contains(&z) {
                continue;
            }
            let q = b0 * inv_w[0] + b1 * inv_w[1] + b2 * inv_w[2];
            let uv = (uvw[0] * b0 + uvw[1] * b1 + uvw[2] * b2) / q;
            if let Some(color) = shade(uv) {
                frame.pixels[index] = color;
                frame.depth[index] = z;
            }
        }
    }
}

/// Сетка на плоскости Y = 0: мелкие линии, крупные (метр), оси X и Z, светлый квадрат
/// одного блока в начале координат. Закрывается моделью по глубине.
fn draw_grid(frame: &mut Band, camera: &Camera, view_proj: &Mat4, options: &RenderOptions) {
    let (w, h) = (frame.width, frame.height);
    let inverse = view_proj.inverse();
    // Обратная проекция линейна в однородных координатах: шаги по пикселю постоянны.
    let ndc = |x: f32, y: f32, z: f32| Vec4::new(x / w as f32 * 2.0 - 1.0, 1.0 - y / h as f32 * 2.0, z, 1.0);
    let near0 = inverse * ndc(0.0, 0.0, 0.0);
    let far0 = inverse * ndc(0.0, 0.0, 1.0);
    let near_dx = inverse * ndc(1.0, 0.0, 0.0) - near0;
    let near_dy = inverse * ndc(0.0, 1.0, 0.0) - near0;
    let far_dx = inverse * ndc(1.0, 0.0, 1.0) - far0;
    let far_dy = inverse * ndc(0.0, 1.0, 1.0) - far0;
    let ground = |x: f32, y: f32| -> Option<Vec3> {
        let n = near0 + near_dx * x + near_dy * y;
        let f = far0 + far_dx * x + far_dy * y;
        let (a, b) = (n.truncate() / n.w, f.truncate() / f.w);
        let dir = b - a;
        if dir.y.abs() < 1e-9 {
            return None;
        }
        let t = -a.y / dir.y;
        (0.0..=1.0).contains(&t).then(|| a + dir * t)
    };
    let step = options.grid_step.max(1e-3);
    let major = step * options.major_every.max(1) as f32;
    let from_below = camera.eye().y < 0.0;
    let fade_radius = camera.distance * 3.0 + major * 4.0;
    let center = camera.target;
    for y in frame.y0..frame.y1 {
        for x in 0..w {
            let (fx, fy) = (x as f32 + 0.5, y as f32 + 0.5);
            let Some(p) = ground(fx, fy) else { continue };
            let index = frame.index(x, y);
            let clip = *view_proj * p.extend(1.0);
            let z = clip.z / clip.w;
            if z >= frame.depth[index] {
                continue;
            }
            let (Some(px), Some(py)) = (ground(fx + 1.0, fy), ground(fx, fy + 1.0)) else { continue };
            let fw = Vec2::new((px.x - p.x).abs() + (py.x - p.x).abs(), (px.z - p.z).abs() + (py.z - p.z).abs())
                .max(Vec2::splat(1e-6));
            let line = |s: f32| {
                let d = Vec2::new(
                    ((p.x / s + 0.5).rem_euclid(1.0) - 0.5).abs() * s,
                    ((p.z / s + 0.5).rem_euclid(1.0) - 0.5).abs() * s,
                );
                let cx = (1.0 - d.x / fw.x).clamp(0.0, 1.0);
                let cz = (1.0 - d.y / fw.y).clamp(0.0, 1.0);
                (cx, cz)
            };
            // Мелкие линии гаснут, когда становятся чаще ~4 пикселей.
            let density = (fw.x.max(fw.y) / step).min(1.0);
            let minor_fade = (1.0 - (density - 0.12) / 0.18).clamp(0.0, 1.0);
            let (mx, mz) = line(step);
            let (bx, bz) = line(major);
            let minor = mx.max(mz) * 0.07 * minor_fade;
            let major_c = bx.max(bz) * 0.16;
            let dist = Vec2::new(p.x - center.x, p.z - center.z).length();
            let fade = (1.0 - dist / fade_radius).clamp(0.0, 1.0);
            let fade = fade * fade * if from_below { 0.4 } else { 1.0 };
            let mut color = frame.pixels[index].map(|c| c as f32);
            // Квадрат одного блока у начала координат — опора для масштаба.
            if p.x.abs() <= major * 0.5 && p.z.abs() <= major * 0.5 {
                for c in color.iter_mut().take(3) {
                    *c += (255.0 - *c) * 0.035 * fade;
                }
            }
            let white = minor.max(major_c) * fade;
            for c in color.iter_mut().take(3) {
                *c += (255.0 - *c) * white;
            }
            // Оси: X — красная (вдоль Z = 0), Z — синяя (вдоль X = 0).
            let axis_x = (1.0 - (p.z.abs() / fw.y)).clamp(0.0, 1.0) * 0.75 * fade;
            let axis_z = (1.0 - (p.x.abs() / fw.x)).clamp(0.0, 1.0) * 0.75 * fade;
            for (amount, tint) in [(axis_x, [0xe8, 0x55, 0x55]), (axis_z, [0x4f, 0x92, 0xec])] {
                if amount > 0.0 {
                    for c in 0..3 {
                        color[c] += (tint[c] as f32 - color[c]) * amount;
                    }
                }
            }
            frame.pixels[index] = [color[0] as u8, color[1] as u8, color[2] as u8, 255];
        }
    }
}

/// Спроецировать точку модели в пиксели кадра `size` (`None` — за камерой).
pub fn project(camera: &Camera, size: Vec2, point: Vec3) -> Option<Vec2> {
    let clip = camera.view_proj(size.x / size.y.max(1.0)) * point.extend(1.0);
    if clip.w <= 1e-6 || clip.z < 0.0 {
        return None;
    }
    let ndc = clip.truncate() / clip.w;
    Some(Vec2::new((ndc.x * 0.5 + 0.5) * size.x, (0.5 - ndc.y * 0.5) * size.y))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::camera::ViewPreset;
    use crate::doc::{Element, FaceDir};
    use crate::geometry::{build_mesh, BuildOptions};
    use crate::ops;

    #[test]
    fn front_view_shows_the_painted_north_face() {
        let mut doc = Document::new("t");
        let id = ops::add_element(&mut doc, "c", Element::cuboid(Vec3::splat(-8.0), Vec3::splat(16.0)), None);
        let north = doc.element(id).unwrap().face(FaceDir::North).uv;
        let [x0, y0, x1, y1] = north.texel_bounds();
        for y in y0..y1 {
            for x in x0..x1 {
                doc.textures[0].image.set(x, y, [250, 20, 20, 255]);
            }
        }
        // Левый верхний тексель (как видно снаружи) — зелёный.
        doc.textures[0].image.set(x0, y0, [20, 250, 20, 255]);
        let mesh = build_mesh(&doc, BuildOptions::default());
        let mut camera = Camera {
            target: Vec3::ZERO,
            distance: 40.0,
            orthographic: true,
            ..Camera::default()
        };
        camera.set_preset(ViewPreset::Front);
        let mut frame = Frame::new(160, 160);
        render(&doc, &mesh, &camera, &RenderOptions::default(), &mut frame);
        let center = frame.get(80, 80);
        assert!(center[0] > 150 && center[1] < 60, "the middle is the red north face: {center:?}");
        // Ортографическая высота кадра = 2·40·tan(22.5°) ≈ 33.1 единицы; куб в 16 — около 77 px.
        let texel = 160.0 / 33.137;
        let corner = frame.get((80.0 - 7.5 * texel) as u32, (80.0 - 7.5 * texel) as u32);
        assert!(corner[1] > 150 && corner[0] < 60, "top-left texel is green: {corner:?}");
        let outside = frame.get(5, 5);
        assert!(outside[0] < 80 && outside[1] < 80, "background around: {outside:?}");
    }

    #[test]
    fn depth_keeps_the_nearer_box_in_front() {
        let mut doc = Document::new("t");
        let near = ops::add_element(&mut doc, "near", Element::cuboid(Vec3::new(-2.0, -2.0, -12.0), Vec3::splat(4.0)), None);
        let far = ops::add_element(&mut doc, "far", Element::cuboid(Vec3::new(-6.0, -6.0, 4.0), Vec3::splat(12.0)), None);
        for (id, color) in [(near, [255, 0, 0, 255]), (far, [0, 0, 255, 255])] {
            for dir in FaceDir::ALL {
                let [x0, y0, x1, y1] = doc.element(id).unwrap().face(dir).uv.texel_bounds();
                for y in y0..y1 {
                    for x in x0..x1 {
                        doc.textures[0].image.set(x, y, color);
                    }
                }
            }
        }
        let mesh = build_mesh(&doc, BuildOptions::default());
        let mut camera = Camera {
            target: Vec3::ZERO,
            distance: 50.0,
            ..Camera::default()
        };
        camera.set_preset(ViewPreset::Front);
        let mut frame = Frame::new(100, 100);
        render(&doc, &mesh, &camera, &RenderOptions { grid: false, ..RenderOptions::default() }, &mut frame);
        let c = frame.get(50, 50);
        assert!(c[0] > c[2], "the near red box covers the far blue one: {c:?}");
        let ring = frame.get(50, 40);
        assert!(ring[2] > ring[0], "the far box shows around it: {ring:?}");
    }
}
