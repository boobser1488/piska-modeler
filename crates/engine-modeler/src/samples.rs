//! Образцы моделей, собранные тем же API, что и кнопки редактора: ящик, фонарь, ель,
//! ремонтный робот с группами-суставами. Генератор `cargo run -p engine-modeler --example
//! gen_samples` пишет их в `assets/models/*.pmodel`.

use glam::Vec3;

use crate::doc::{Document, Element, FaceDir, NodeId, Shape};
use crate::image::Rgba;
use crate::ops;

/// Все образцы: (имя файла без расширения, документ).
pub fn all() -> Vec<(&'static str, Document)> {
    vec![
        ("crate", wooden_crate()),
        ("lantern", lantern()),
        ("pine", pine_tree()),
        ("rover", rover()),
    ]
}

/// Детерминированный «шум» 0…1 по координатам — чтобы образцы были одинаковыми при каждом
/// запуске генератора.
fn hash(x: i32, y: i32, seed: u32) -> f32 {
    let mut h = (x as u32).wrapping_mul(0x8da6_b343) ^ (y as u32).wrapping_mul(0xd816_3841) ^ seed.wrapping_mul(0xcb1a_b31f);
    h ^= h >> 13;
    h = h.wrapping_mul(0x5bd1_e995);
    h ^= h >> 15;
    (h & 0xffff) as f32 / 65535.0
}

fn scale(color: Rgba, k: f32) -> Rgba {
    let f = |c: u8| (c as f32 * k).round().clamp(0.0, 255.0) as u8;
    [f(color[0]), f(color[1]), f(color[2]), color[3]]
}

/// Раскрасить сторону элемента: `paint(x, y, w, h)` — цвет текселя в координатах стороны
/// (0, 0 — левый верхний угол, как сторону видно снаружи).
fn paint_face(doc: &mut Document, id: NodeId, face: FaceDir, paint: impl Fn(i32, i32, i32, i32) -> Rgba) {
    let Some(element) = doc.element(id) else { return };
    let f = *element.face(face);
    if !f.enabled {
        return;
    }
    let [x0, y0, x1, y1] = f.uv.texel_bounds();
    let (w, h) = (x1 - x0, y1 - y0);
    if w <= 0 || h <= 0 {
        return;
    }
    // Обход по пространству стороны: (s, t) → тексель с учётом поворота и отражений.
    let image = &mut doc.textures[f.texture as usize].image;
    let (fw, fh) = if f.uv.rotation % 2 == 0 { (w, h) } else { (h, w) };
    for t in 0..fh {
        for s in 0..fw {
            let uv = crate::geometry::face_uv(&f.uv, (s as f32 + 0.5) / fw as f32, (t as f32 + 0.5) / fh as f32);
            image.set(uv[0].floor() as i32, uv[1].floor() as i32, paint(s, t, fw, fh));
        }
    }
}

fn paint_all(doc: &mut Document, id: NodeId, paint: impl Fn(FaceDir, i32, i32, i32, i32) -> Rgba) {
    for face in FaceDir::ALL {
        paint_face(doc, id, face, |x, y, w, h| paint(face, x, y, w, h));
    }
}

fn add(doc: &mut Document, name: &str, from: [f32; 3], size: [f32; 3], parent: Option<NodeId>) -> NodeId {
    ops::add_element(doc, name, Element::cuboid(Vec3::from_array(from), Vec3::from_array(size)), parent)
}

/// Деревянный ящик 1×1×1 м: доски, рамка, диагональная перекладина.
pub fn wooden_crate() -> Document {
    let mut doc = Document::new("crate");
    let body = add(&mut doc, "body", [-8.0, 0.0, -8.0], [16.0, 16.0, 16.0], None);
    let wood = [0xb0, 0x7a, 0x45, 255];
    let frame = [0x6e, 0x45, 0x24, 255];
    paint_all(&mut doc, body, |face, x, y, w, h| {
        let border = x == 0 || y == 0 || x == w - 1 || y == h - 1;
        let edge = x <= 1 || y <= 1 || x >= w - 2 || y >= h - 2;
        let diagonal = face != FaceDir::Up && face != FaceDir::Down && ((x - y).abs() <= 1);
        let n = hash(x, y, face as u32);
        if border {
            scale(frame, 0.75 + 0.1 * n)
        } else if edge || diagonal {
            scale(frame, 0.95 + 0.15 * n)
        } else {
            // Доски по 4 текселя со щелью между ними.
            let plank = y % 4 == 3;
            let grain = hash(x / 3, y, 7 + face as u32);
            if plank {
                scale(wood, 0.62)
            } else {
                scale(wood, 0.9 + 0.18 * grain + 0.05 * n)
            }
        }
    });
    doc
}

/// Фонарь: основание, стойки, светящееся стекло, крыша-конус, ручка.
pub fn lantern() -> Document {
    let mut doc = Document::new("lantern");
    let iron = [0x3b, 0x3f, 0x46, 255];
    let base = add(&mut doc, "base", [-5.0, 0.0, -5.0], [10.0, 2.0, 10.0], None);
    let glass = add(&mut doc, "glass", [-4.0, 2.0, -4.0], [8.0, 9.0, 8.0], None);
    let cap = add(&mut doc, "cap", [-5.0, 11.0, -5.0], [10.0, 1.0, 10.0], None);
    let roof = ops::add_element(
        &mut doc,
        "roof",
        Element::new(Shape::Cone { segments: 8 }, Vec3::new(-4.5, 12.0, -4.5), Vec3::new(9.0, 4.0, 9.0)),
        None,
    );
    let handle = ops::add_group(&mut doc, "handle", Vec3::new(0.0, 16.0, 0.0), None);
    let ring_left = add(&mut doc, "ring_left", [1.0, 16.0, -0.5], [1.0, 3.0, 1.0], Some(handle));
    // Правое ушко — отражённый близнец левого, картинка общая.
    ops::mirror_x(&mut doc, &[ring_left]);
    let ring_top = add(&mut doc, "ring_top", [-2.0, 19.0, -0.5], [4.0, 1.0, 1.0], Some(handle));
    let mut posts = Vec::new();
    for (i, (x, z)) in [(-5.0, -5.0), (4.0, -5.0), (-5.0, 4.0), (4.0, 4.0)].into_iter().enumerate() {
        posts.push(add(&mut doc, &format!("post{}", i + 1), [x, 2.0, z], [1.0, 9.0, 1.0], None));
    }
    for id in [base, cap, ring_left, ring_top].iter().chain(&posts) {
        paint_all(&mut doc, *id, |face, x, y, _, _| scale(iron, 0.85 + 0.3 * hash(x, y, face as u32 + 31)));
    }
    paint_all(&mut doc, roof, |face, x, y, _, _| {
        let rivet = (x + y) % 5 == 0;
        scale(iron, if rivet { 1.35 } else { 0.8 + 0.2 * hash(x, y, face as u32 + 5) })
    });
    let flame = [0xff, 0xc8, 0x5a, 255];
    paint_all(&mut doc, glass, |face, x, y, w, h| {
        if face == FaceDir::Up || face == FaceDir::Down {
            return scale(iron, 0.7);
        }
        let dx = (x as f32 + 0.5 - w as f32 / 2.0) / (w as f32 / 2.0);
        let dy = (y as f32 + 0.5 - h as f32 * 0.55) / (h as f32 / 2.0);
        let glow = (1.0 - (dx * dx + dy * dy).sqrt()).clamp(0.0, 1.0);
        let frame = x == 0 || x == w - 1;
        if frame {
            scale(iron, 0.9)
        } else {
            let k = 0.55 + 0.6 * glow;
            [flame[0], (flame[1] as f32 * k).min(255.0) as u8, (flame[2] as f32 * k * 0.8) as u8, 255]
        }
    });
    if let Some(e) = doc.element_mut(glass) {
        for face in [FaceDir::North, FaceDir::East, FaceDir::South, FaceDir::West] {
            e.face_mut(face).emissive = true;
        }
    }
    doc
}

/// Ель: ствол-цилиндр и три яруса конусов.
pub fn pine_tree() -> Document {
    let mut doc = Document::new("pine");
    let trunk = ops::add_element(
        &mut doc,
        "trunk",
        Element::new(Shape::Cylinder { segments: 6 }, Vec3::new(-2.0, 0.0, -2.0), Vec3::new(4.0, 12.0, 4.0)),
        None,
    );
    let bark = [0x6b, 0x47, 0x2b, 255];
    paint_all(&mut doc, trunk, |face, x, y, _, _| {
        let stripe = hash(x, y / 3, face as u32 + 11);
        scale(bark, 0.75 + 0.4 * stripe)
    });
    let crown = ops::add_group(&mut doc, "crown", Vec3::new(0.0, 8.0, 0.0), None);
    let needles = [0x2f, 0x7a, 0x45, 255];
    for (i, (y, size, height)) in [(6.0, 18.0, 10.0), (12.0, 14.0, 9.0), (17.0, 9.0, 9.0)].into_iter().enumerate() {
        let id = ops::add_element(
            &mut doc,
            &format!("tier{}", i + 1),
            Element::new(
                Shape::Cone { segments: 7 },
                Vec3::new(-size / 2.0, y, -size / 2.0),
                Vec3::new(size, height, size),
            ),
            Some(crown),
        );
        let seed = 50 + i as u32;
        paint_all(&mut doc, id, |face, x, y, _, h| {
            let shade = 0.7 + 0.35 * (1.0 - y as f32 / h.max(1) as f32) + 0.15 * hash(x, y, seed + face as u32);
            let snow = i == 2 && y < 2 && hash(x, y, seed) > 0.4;
            if snow { [0xe8, 0xee, 0xf2, 255] } else { scale(needles, shade) }
        });
    }
    doc
}

/// Ремонтный робот «Ровер»: гусеницы, широкий корпус с панелью и заклёпками, купол с
/// полосой-визором и антенной, руки-клещи. Группы — суставы (гусеницы, голова, плечи);
/// правые гусеница и рука — отражённые близнецы левых.
pub fn rover() -> Document {
    let mut doc = Document::new("rover");
    let orange = [0xe8, 0x7a, 0x2a, 255];
    let graphite = [0x3a, 0x3d, 0x44, 255];
    let steel = [0x8c, 0x92, 0x9a, 255];
    let rubber = [0x22, 0x23, 0x26, 255];
    let cyan = [0x5c, 0xe1, 0xe6, 255];

    let body = ops::add_group(&mut doc, "body", Vec3::new(0.0, 6.0, 0.0), None);
    // Гусеницы.
    let tread_l = ops::add_group(&mut doc, "tread_left", Vec3::new(5.0, 3.0, 0.0), Some(body));
    let tread_r = ops::add_group(&mut doc, "tread_right", Vec3::new(-5.0, 3.0, 0.0), Some(body));
    let tread = add(&mut doc, "track_left", [4.0, 0.0, -6.0], [4.0, 5.0, 12.0], Some(tread_l));
    // Корпус и шея.
    let hull = add(&mut doc, "hull", [-6.0, 5.0, -5.0], [12.0, 9.0, 10.0], Some(body));
    let neck = ops::add_element(
        &mut doc,
        "neck",
        Element::new(Shape::Cylinder { segments: 8 }, Vec3::new(-1.5, 14.0, -1.5), Vec3::new(3.0, 2.0, 3.0)),
        Some(body),
    );
    // Голова: купол, визор, антенна.
    let head = ops::add_group(&mut doc, "head", Vec3::new(0.0, 16.0, 0.0), Some(body));
    let dome = add(&mut doc, "dome", [-4.5, 16.0, -4.0], [9.0, 5.0, 8.0], Some(head));
    let visor = add(&mut doc, "visor", [-3.5, 17.5, -4.5], [7.0, 2.0, 1.0], Some(head));
    let mast = ops::add_element(
        &mut doc,
        "antenna",
        Element::new(Shape::Cylinder { segments: 6 }, Vec3::new(1.5, 21.0, 0.5), Vec3::new(1.0, 4.0, 1.0)),
        Some(head),
    );
    let beacon = ops::add_element(
        &mut doc,
        "beacon",
        Element::new(Shape::Sphere { segments: 6, rings: 4 }, Vec3::new(1.0, 25.0, 0.0), Vec3::splat(2.0)),
        Some(head),
    );
    // Руки: плечо и клещи.
    let arm_l = ops::add_group(&mut doc, "arm_left", Vec3::new(7.0, 12.5, 0.0), Some(body));
    let arm_r = ops::add_group(&mut doc, "arm_right", Vec3::new(-7.0, 12.5, 0.0), Some(body));
    let arm = add(&mut doc, "boom_left", [6.0, 7.0, -1.5], [2.5, 7.0, 3.0], Some(arm_l));
    let clamp = add(&mut doc, "clamp_left", [5.5, 4.0, -2.5], [3.5, 3.0, 5.0], Some(arm_l));
    for (source, group) in [(tread, tread_r), (arm, arm_r), (clamp, arm_r)] {
        let twin = ops::mirror_x(&mut doc, &[source])[0];
        ops::reparent(&mut doc, twin, Some(group), None);
    }
    // Поза: одна рука поднята, другая опущена, голова чуть повёрнута.
    for (group, axis, angle) in [(arm_l, 0, -35.0), (arm_r, 0, 10.0), (head, 1, 15.0)] {
        if let Some(g) = doc.node_mut(group).and_then(|n| n.group_mut()) {
            g.rotation[axis] = angle;
        }
    }

    paint_all(&mut doc, tread, |face, x, y, w, h| {
        let n = hash(x, y, 200 + face as u32);
        // Траки поперёк хода и катки на боках.
        let link = match face {
            FaceDir::East | FaceDir::West => (x % 3 == 0 && (y == 0 || y == h - 1)) || (y == h / 2 && x % 4 == 1),
            _ => y % 2 == 0,
        };
        let edge = x == 0 || x == w - 1;
        if link {
            scale(steel, 0.75 + 0.2 * n)
        } else if edge {
            scale(rubber, 1.3)
        } else {
            scale(rubber, 0.9 + 0.3 * n)
        }
    });
    paint_all(&mut doc, hull, |face, x, y, w, h| {
        let n = hash(x, y, 220 + face as u32);
        let border = x == 0 || y == 0 || x == w - 1 || y == h - 1;
        let rivet = border && (x + y) % 3 == 0 && face != FaceDir::Down;
        if rivet {
            return scale(steel, 1.15);
        }
        if face == FaceDir::North {
            // Панель на груди: рамка, решётка вентиляции, индикаторы.
            let panel = (2..w - 2).contains(&x) && (2..h - 2).contains(&y);
            if panel {
                let inner = (3..w - 3).contains(&x) && (3..h - 3).contains(&y);
                if !inner {
                    return scale(graphite, 0.8);
                }
                if y == 3 && (x == 4 || x == 6) {
                    return if x == 4 { [0x7c, 0xe0, 0x5a, 255] } else { [0xf2, 0xc9, 0x4c, 255] };
                }
                return if y % 2 == 1 { scale(graphite, 1.2) } else { scale(graphite, 0.95 + 0.1 * n) };
            }
        }
        if border {
            scale(orange, 0.72)
        } else {
            // Потёртости краски.
            if n > 0.93 { scale(steel, 0.9) } else { scale(orange, 0.92 + 0.12 * n) }
        }
    });
    paint_all(&mut doc, neck, |face, x, y, _, _| scale(graphite, 0.9 + 0.2 * hash(x, y, 240 + face as u32)));
    paint_all(&mut doc, dome, |face, x, y, w, h| {
        let n = hash(x, y, 260 + face as u32);
        let seam = y == h - 1 || (face == FaceDir::Up && (x == w / 2));
        if seam {
            scale(orange, 0.8)
        } else {
            scale(graphite, 0.95 + 0.15 * n)
        }
    });
    paint_all(&mut doc, visor, |_, x, _, w, _| {
        // Бегущая полоса: ярче к середине.
        let d = (x as f32 + 0.5 - w as f32 / 2.0).abs() / (w as f32 / 2.0);
        scale(cyan, 1.1 - 0.45 * d)
    });
    if let Some(e) = doc.element_mut(visor) {
        e.face_mut(FaceDir::North).emissive = true;
    }
    paint_all(&mut doc, mast, |face, x, y, _, _| scale(steel, 0.8 + 0.3 * hash(x, y, 280 + face as u32)));
    paint_all(&mut doc, beacon, |face, x, y, _, _| scale([0xff, 0x4d, 0x3a, 255], 0.85 + 0.25 * hash(x, y, 290 + face as u32)));
    if let Some(e) = doc.element_mut(beacon) {
        e.face_mut(FaceDir::North).emissive = true;
    }
    paint_all(&mut doc, arm, |face, x, y, _, _| {
        let n = hash(x, y, 300 + face as u32);
        if y % 3 == 0 { scale(steel, 0.85) } else { scale(graphite, 0.95 + 0.15 * n) }
    });
    paint_all(&mut doc, clamp, |face, x, y, w, h| {
        let n = hash(x, y, 320 + face as u32);
        // Зазор между челюстями внизу.
        let gap = face != FaceDir::Up && y >= h - 1 && (1..w - 1).contains(&x);
        if gap {
            scale(rubber, 1.0)
        } else {
            scale(orange, 0.85 + 0.15 * n)
        }
    });
    doc
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn samples_are_valid_and_survive_a_round_trip() {
        for (name, doc) in all() {
            assert!(doc.element_count() >= 1, "{name}");
            let text = crate::format::to_string(&doc);
            assert_eq!(crate::format::from_str(&text).unwrap(), doc, "{name}");
            let baked = crate::bake::bake(&doc);
            assert!(baked.triangle_count() >= 12, "{name}");
            // Ни одна сторона не осталась прозрачной: модели видны в движке.
            let mesh = crate::geometry::build_mesh(&doc, crate::geometry::BuildOptions::default());
            for t in &mesh.triangles {
                let uv = t.indices.iter().map(|i| mesh.vertices[*i as usize].uv).sum::<glam::Vec2>() / 3.0;
                let image = &doc.textures[t.texture as usize].image;
                let texel = image.get((uv.x * image.width() as f32) as i32, (uv.y * image.height() as f32) as i32);
                assert_eq!(texel.map(|c| c[3]), Some(255), "{name}: {:?} of {:?} is painted", t.face, t.node);
            }
        }
        let rover = rover();
        let (min, max) = rover.model_bounds().unwrap();
        assert!(max.y > 24.0 && min.y < 1.0, "the rover is about 1.6 m tall: {min} {max}");
        let groups = rover.nodes.iter().filter(|n| n.is_group()).count();
        assert_eq!(groups, 6);
    }
}
