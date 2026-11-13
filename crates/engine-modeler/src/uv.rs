//! Автоматическая развёртка «коробкой» и упаковка развёрток в атлас.
//!
//! Кубоид размером `w×h×d` (в текселях) раскладывается как бумажная коробка:
//!
//! ```text
//!        ┌───┬───┐
//!        │ U │ D │          U — Up, D — Down (высота d)
//!    ┌───┼───┼───┼───┐
//!    │ E │ N │ W │ S │      E, N, W, S — бока (высота h), идут вокруг коробки
//!    └───┴───┴───┴───┘
//!     d   w   d   w
//! ```
//!
//! Соседние стороны в полосе стыкуются краями, как на самой модели, поэтому узор,
//! нарисованный через шов, продолжается. Занятое место — прямоугольник `(2d + 2w) × (d + h)`.

use crate::doc::{Document, Element, FaceDir, NodeId, Shape, UvRect};
use crate::image::{Image, Rgba, CLEAR};

/// Наибольшая сторона атласа, до которой он растёт сам.
pub const MAX_TEXTURE_SIZE: u32 = 2048;

/// Размеры коробки в текселях (округление вверх; нулевая толщина остаётся нулём).
fn dims(element: &Element, density: f32) -> [u32; 3] {
    let t = |v: f32| (v.abs() * density - 1e-4).ceil().max(0.0) as u32;
    [t(element.size.x), t(element.size.y), t(element.size.z)]
}

/// Длина обхода эллипса с осями `w` и `d` (приближение Рамануджана) — ширина боковой
/// полосы цилиндра и шара.
fn perimeter(w: u32, d: u32) -> u32 {
    let (a, b) = (w as f32 * 0.5, d as f32 * 0.5);
    let h = ((a - b) / (a + b).max(1e-6)).powi(2);
    let p = std::f32::consts::PI * (a + b) * (1.0 + 3.0 * h / (10.0 + (4.0 - 3.0 * h).sqrt()));
    (p - 1e-3).ceil().max(1.0) as u32
}

/// Сколько места занимает развёртка элемента: `[ширина, высота]` в текселях.
pub fn footprint(element: &Element, density: f32) -> [u32; 2] {
    let [w, h, d] = dims(element, density);
    let size = match element.shape {
        Shape::Cuboid => [2 * d + 2 * w, d + h],
        Shape::Plane => [2 * w, d],
        Shape::Cylinder { .. } | Shape::Cone { .. } => [(2 * w).max(perimeter(w, d)), d + h],
        Shape::Sphere { .. } => [perimeter(w, d), (std::f32::consts::FRAC_PI_2 * h as f32).ceil() as u32],
    };
    [size[0].max(1), size[1].max(1)]
}

/// Разложить стороны элемента от угла `origin` (поворот и отражения сбрасываются).
pub fn layout_box(element: &mut Element, origin: [u32; 2], density: f32) {
    let [w, h, d] = dims(element, density).map(|v| v as f32);
    let (ox, oy) = (origin[0] as f32, origin[1] as f32);
    element.uv_origin = origin;
    let rects: Vec<(FaceDir, UvRect)> = match element.shape {
        Shape::Cuboid => vec![
            (FaceDir::Up, UvRect::new(ox + d, oy, w, d)),
            (FaceDir::Down, UvRect::new(ox + d + w, oy, w, d)),
            (FaceDir::East, UvRect::new(ox, oy + d, d, h)),
            (FaceDir::North, UvRect::new(ox + d, oy + d, w, h)),
            (FaceDir::West, UvRect::new(ox + d + w, oy + d, d, h)),
            (FaceDir::South, UvRect::new(ox + 2.0 * d + w, oy + d, w, h)),
        ],
        Shape::Plane => vec![
            (FaceDir::Up, UvRect::new(ox, oy, w, d)),
            (FaceDir::Down, UvRect::new(ox + w, oy, w, d)),
        ],
        Shape::Cylinder { .. } | Shape::Cone { .. } => {
            let p = perimeter(w as u32, d as u32) as f32;
            vec![
                (FaceDir::Up, UvRect::new(ox, oy, w, d)),
                (FaceDir::Down, UvRect::new(ox + w, oy, w, d)),
                (FaceDir::North, UvRect::new(ox, oy + d, p, h)),
            ]
        }
        Shape::Sphere { .. } => {
            let [fw, fh] = footprint(element, density).map(|v| v as f32);
            vec![(FaceDir::North, UvRect::new(ox, oy, fw, fh))]
        }
    };
    for (dir, rect) in rects {
        element.face_mut(dir).uv = rect;
    }
}

/// Прямоугольник `[x, y, w, h]`.
pub type Rect = [u32; 4];

fn overlaps(a: Rect, b: Rect) -> bool {
    a[0] < b[0] + b[2] && b[0] < a[0] + a[2] && a[1] < b[1] + b[3] && b[1] < a[1] + a[3]
}

/// Текстура элемента с развёрткой коробкой — текстура его первой включённой стороны.
pub fn element_texture(element: &Element) -> u32 {
    element
        .faces
        .iter()
        .find(|f| f.enabled)
        .map_or(0, |f| f.texture)
}

/// Места, занятые развёртками коробкой на текстуре `texture`, кроме узлов `except`.
pub fn occupied(doc: &Document, texture: u32, except: &[NodeId]) -> Vec<Rect> {
    doc.elements()
        .filter(|(id, e)| e.box_uv && element_texture(e) == texture && !except.contains(id))
        .map(|(_, e)| {
            let [w, h] = footprint(e, doc.texel_density);
            [e.uv_origin[0], e.uv_origin[1], w, h]
        })
        .collect()
}

/// Первое свободное место `size` на холсте `canvas` (построчно сверху вниз).
pub fn find_free(occupied: &[Rect], canvas: [u32; 2], size: [u32; 2]) -> Option<[u32; 2]> {
    if size[0] > canvas[0] || size[1] > canvas[1] {
        return None;
    }
    for y in 0..=canvas[1] - size[1] {
        let mut x = 0;
        while x + size[0] <= canvas[0] {
            let candidate = [x, y, size[0], size[1]];
            match occupied.iter().find(|r| overlaps(**r, candidate)) {
                Some(r) => x = r[0] + r[2],
                None => return Some([x, y]),
            }
        }
    }
    None
}

/// Увеличить холст текстуры вдвое по меньшей стороне. `false` — уже предельный размер.
pub fn grow_texture(doc: &mut Document, texture: u32) -> bool {
    let Some(tex) = doc.textures.get_mut(texture as usize) else {
        return false;
    };
    let [w, h] = tex.image.size();
    let (nw, nh) = if w <= h { (w * 2, h) } else { (w, h * 2) };
    if nw > MAX_TEXTURE_SIZE || nh > MAX_TEXTURE_SIZE {
        return false;
    }
    // UV хранятся в текселях: при росте холста вправо и вниз они не меняются.
    tex.image = tex.image.resized_canvas(nw, nh, CLEAR);
    true
}

/// Найти место для развёртки элемента `id` и разложить её; холст растёт, если места нет.
/// Возвращает `false`, если места нет даже на холсте предельного размера.
pub fn place(doc: &mut Document, id: NodeId) -> bool {
    let Some(element) = doc.element(id) else { return false };
    let size = footprint(element, doc.texel_density);
    let texture = element_texture(element);
    loop {
        let canvas = match doc.textures.get(texture as usize) {
            Some(t) => t.image.size(),
            None => return false,
        };
        let taken = occupied(doc, texture, &[id]);
        if let Some(origin) = find_free(&taken, canvas, size) {
            let density = doc.texel_density;
            if let Some(element) = doc.element_mut(id) {
                layout_box(element, origin, density);
            }
            return true;
        }
        if !grow_texture(doc, texture) {
            return false;
        }
    }
}

/// Залить стороны элемента цветом (новый элемент получает видимую основу для рисования).
pub fn fill_faces(doc: &mut Document, id: NodeId, color: Rgba) {
    let Some(element) = doc.element(id).cloned() else { return };
    for dir in element.shape.faces() {
        let face = element.face(*dir);
        let Some(texture) = doc.textures.get_mut(face.texture as usize) else { continue };
        let [x0, y0, x1, y1] = face.uv.texel_bounds();
        for y in y0..y1 {
            for x in x0..x1 {
                texture.image.set(x, y, color);
            }
        }
    }
}

/// Развёртка элемента после смены размера или формы: место остаётся прежним, если новая
/// развёртка туда помещается, иначе ищется новое. Картинки сторон переезжают вместе с ними
/// (обрезаются или дополняются цветом середины старой стороны).
pub fn refit(doc: &mut Document, id: NodeId, old: &Element) {
    let Some(element) = doc.element(id).cloned() else { return };
    if !element.box_uv {
        return;
    }
    let density = doc.texel_density;
    let texture = element_texture(&element);
    let new_size = footprint(&element, density);
    let old_size = footprint(old, density);
    let same_layout = new_size == old_size && element.shape == old.shape && element.uv_origin == old.uv_origin;
    if same_layout && dims(&element, density) == dims(old, density) {
        return;
    }
    let Some(canvas) = doc.textures.get(texture as usize).map(|t| t.image.size()) else { return };
    let taken = occupied(doc, texture, &[id]);
    let here = [old.uv_origin[0], old.uv_origin[1], new_size[0], new_size[1]];
    let fits_here = here[0] + here[2] <= canvas[0]
        && here[1] + here[3] <= canvas[1]
        && !taken.iter().any(|r| overlaps(*r, here));
    if fits_here {
        if let Some(e) = doc.element_mut(id) {
            layout_box(e, old.uv_origin, density);
        }
    } else if !place(doc, id) {
        log::warn!("no room on the texture for the UV layout of `{}`", doc.node(id).map_or("?", |n| n.name.as_str()));
        return;
    }
    let Some(new) = doc.element(id).cloned() else { return };
    let Some(tex) = doc.textures.get_mut(texture as usize) else { return };
    let before = tex.image.clone();
    // Старое место освобождается: на нём остались бы чужие для атласа пиксели.
    let [ox, oy] = old.uv_origin;
    for y in oy..oy + old_size[1] {
        for x in ox..ox + old_size[0] {
            tex.image.set(x as i32, y as i32, CLEAR);
        }
    }
    for dir in new.shape.faces() {
        let (from, to) = (old.face(*dir).uv, new.face(*dir).uv);
        let [fx0, fy0, fx1, fy1] = from.texel_bounds();
        let [tx0, ty0, tx1, ty1] = to.texel_bounds();
        if fx1 <= fx0 || fy1 <= fy0 {
            continue;
        }
        let middle = before.get((fx0 + fx1) / 2, (fy0 + fy1) / 2).unwrap_or(CLEAR);
        for y in ty0..ty1 {
            for x in tx0..tx1 {
                let (sx, sy) = (fx0 + x - tx0, fy0 + y - ty0);
                let color = if sx < fx1 && sy < fy1 { before.get(sx, sy).unwrap_or(middle) } else { middle };
                tex.image.set(x, y, color);
            }
        }
    }
}

/// Переупаковать все развёртки коробкой на текстуре: плотно, от больших к маленьким;
/// нарисованное переезжает вместе с развёртками. Холст растёт, если нужно.
pub fn repack(doc: &mut Document, texture: u32) -> bool {
    let density = doc.texel_density;
    let mut items: Vec<(NodeId, [u32; 2], [u32; 2])> = doc
        .elements()
        .filter(|(_, e)| e.box_uv && element_texture(e) == texture)
        .map(|(id, e)| (id, footprint(e, density), e.uv_origin))
        .collect();
    items.sort_by(|a, b| (b.1[1], b.1[0], a.0).cmp(&(a.1[1], a.1[0], b.0)));
    let Some(old_image) = doc.textures.get(texture as usize).map(|t| t.image.clone()) else {
        return false;
    };
    let mut canvas = old_image.size();
    let origins = loop {
        let mut placed: Vec<Rect> = Vec::new();
        let mut origins = Vec::new();
        let mut ok = true;
        for (_, size, _) in &items {
            match find_free(&placed, canvas, *size) {
                Some(o) => {
                    placed.push([o[0], o[1], size[0], size[1]]);
                    origins.push(o);
                }
                None => {
                    ok = false;
                    break;
                }
            }
        }
        if ok {
            break origins;
        }
        let next = if canvas[0] <= canvas[1] { [canvas[0] * 2, canvas[1]] } else { [canvas[0], canvas[1] * 2] };
        if next[0] > MAX_TEXTURE_SIZE || next[1] > MAX_TEXTURE_SIZE {
            return false;
        }
        canvas = next;
    };
    let mut image = Image::new(canvas[0], canvas[1], CLEAR);
    // Пиксели вне развёрток (картинки сторон, разложенных вручную) остаются на месте.
    image.blit(&old_image, 0, 0, 0, 0, old_image.width(), old_image.height());
    for (_, size, from) in &items {
        for y in from[1]..from[1] + size[1] {
            for x in from[0]..from[0] + size[0] {
                image.set(x as i32, y as i32, CLEAR);
            }
        }
    }
    for ((id, size, from), to) in items.iter().zip(&origins) {
        image.blit(&old_image, from[0] as i32, from[1] as i32, to[0] as i32, to[1] as i32, size[0], size[1]);
        if let Some(e) = doc.element_mut(*id) {
            layout_box(e, *to, density);
        }
    }
    doc.textures[texture as usize].image = image;
    // Близнецы повторяют развёртку своих оригиналов.
    let ids: Vec<NodeId> = items.iter().map(|i| i.0).collect();
    crate::ops::sync_twins(doc, &ids);
    true
}

#[cfg(test)]
mod tests {
    use super::*;
    use glam::Vec3;

    #[test]
    fn box_layout_rects_do_not_overlap_and_fill_the_footprint() {
        let mut element = Element::cuboid(Vec3::ZERO, Vec3::new(4.0, 6.0, 2.0));
        layout_box(&mut element, [3, 5], 1.0);
        let fp = footprint(&element, 1.0);
        assert_eq!(fp, [12, 8]);
        let rects: Vec<Rect> = FaceDir::ALL
            .iter()
            .map(|d| {
                let r = element.face(*d).uv;
                [r.x as u32, r.y as u32, r.w as u32, r.h as u32]
            })
            .collect();
        let mut area = 0;
        for (i, a) in rects.iter().enumerate() {
            assert!(a[0] >= 3 && a[1] >= 5 && a[0] + a[2] <= 15 && a[1] + a[3] <= 13, "{a:?} in footprint");
            for b in &rects[i + 1..] {
                assert!(!overlaps(*a, *b), "{a:?} vs {b:?}");
            }
            area += a[2] * a[3];
        }
        // Две строки: верх (2·w·d) и полоса боков (2·(w + d)·h).
        assert_eq!(area, 2 * 4 * 2 + 2 * (4 + 2) * 6);
    }

    #[test]
    fn free_space_search_skips_taken_rects() {
        let taken = [[0, 0, 10, 4], [12, 0, 4, 8]];
        assert_eq!(find_free(&taken, [16, 16], [2, 4]), Some([10, 0]));
        assert_eq!(find_free(&taken, [16, 16], [4, 4]), Some([0, 4]));
        assert_eq!(find_free(&taken, [16, 16], [20, 1]), None);
    }
}
