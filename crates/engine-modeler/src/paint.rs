//! Рисование по текстуре: карандаш, ластик, заливка, линия, прямоугольник, пипетка,
//! осветление / затемнение. Всё работает с текселями; `clip` — прямоугольник
//! `[x0, y0, x1, y1)`, за который кисть не выходит (сторона модели), `None` — вся картинка.

use crate::image::{Image, Rgba};

/// Прямоугольник отсечения `[x0, y0, x1, y1)`.
pub type Clip = [i32; 4];

fn inside(clip: Option<Clip>, x: i32, y: i32) -> bool {
    clip.is_none_or(|[x0, y0, x1, y1]| x >= x0 && y >= y0 && x < x1 && y < y1)
}

/// Квадратная кисть `size×size` с центром в тексель `(x, y)` (для чётного размера центр
/// смещён влево вверх). Возвращает тексели, которые она покрывает.
pub fn brush_texels(x: i32, y: i32, size: u32) -> impl Iterator<Item = (i32, i32)> {
    let size = size.max(1) as i32;
    let start = -(size - 1) / 2;
    (0..size).flat_map(move |dy| (0..size).map(move |dx| (x + start + dx, y + start + dy)))
}

/// Мазок кистью в одну точку. Возвращает число изменённых текселей.
pub fn dab(image: &mut Image, x: i32, y: i32, size: u32, color: Rgba, clip: Option<Clip>) -> usize {
    brush_texels(x, y, size)
        .filter(|(px, py)| inside(clip, *px, *py) && image.set(*px, *py, color))
        .count()
}

/// Тексели отрезка (алгоритм Брезенхема), концы включительно.
pub fn line_texels(from: (i32, i32), to: (i32, i32)) -> Vec<(i32, i32)> {
    let (mut x, mut y) = from;
    let dx = (to.0 - x).abs();
    let dy = -(to.1 - y).abs();
    let sx = if x < to.0 { 1 } else { -1 };
    let sy = if y < to.1 { 1 } else { -1 };
    let mut err = dx + dy;
    let mut out = Vec::new();
    loop {
        out.push((x, y));
        if (x, y) == to {
            break;
        }
        let e2 = 2 * err;
        if e2 >= dy {
            err += dy;
            x += sx;
        }
        if e2 <= dx {
            err += dx;
            y += sy;
        }
    }
    out
}

/// Линия кистью.
pub fn line(image: &mut Image, from: (i32, i32), to: (i32, i32), size: u32, color: Rgba, clip: Option<Clip>) -> usize {
    line_texels(from, to)
        .into_iter()
        .map(|(x, y)| dab(image, x, y, size, color, clip))
        .sum()
}

/// Прямоугольник по двум углам (включительно): контур или заливка.
pub fn rectangle(image: &mut Image, a: (i32, i32), b: (i32, i32), color: Rgba, filled: bool, clip: Option<Clip>) -> usize {
    let (x0, x1) = (a.0.min(b.0), a.0.max(b.0));
    let (y0, y1) = (a.1.min(b.1), a.1.max(b.1));
    let mut changed = 0;
    for y in y0..=y1 {
        for x in x0..=x1 {
            let edge = x == x0 || x == x1 || y == y0 || y == y1;
            if (filled || edge) && inside(clip, x, y) && image.set(x, y, color) {
                changed += 1;
            }
        }
    }
    changed
}

/// Залить весь прямоугольник отсечения (сторону модели).
pub fn fill_clip(image: &mut Image, clip: Clip, color: Rgba) -> usize {
    rectangle(image, (clip[0], clip[1]), (clip[2] - 1, clip[3] - 1), color, true, Some(clip))
}

/// Заливка связной области того же цвета (4-связность) в пределах `clip`.
pub fn flood_fill(image: &mut Image, x: i32, y: i32, color: Rgba, clip: Option<Clip>) -> usize {
    let Some(target) = image.get(x, y) else { return 0 };
    if target == color || !inside(clip, x, y) {
        return 0;
    }
    let mut stack = vec![(x, y)];
    let mut changed = 0;
    while let Some((px, py)) = stack.pop() {
        if !inside(clip, px, py) || image.get(px, py) != Some(target) {
            continue;
        }
        image.set(px, py, color);
        changed += 1;
        stack.extend([(px + 1, py), (px - 1, py), (px, py + 1), (px, py - 1)]);
    }
    changed
}

/// Осветлить (`amount > 0`) или затемнить цвет, сохранив оттенок и прозрачность.
pub fn shade_color(color: Rgba, amount: i32) -> Rgba {
    let f = |c: u8| (c as i32 + amount).clamp(0, 255) as u8;
    [f(color[0]), f(color[1]), f(color[2]), color[3]]
}

/// Осветлить / затемнить тексели под кистью (прозрачные не трогаются).
pub fn shade(image: &mut Image, x: i32, y: i32, size: u32, amount: i32, clip: Option<Clip>) -> usize {
    let mut changed = 0;
    for (px, py) in brush_texels(x, y, size) {
        if !inside(clip, px, py) {
            continue;
        }
        if let Some(c) = image.get(px, py) {
            if c[3] > 0 && image.set(px, py, shade_color(c, amount)) {
                changed += 1;
            }
        }
    }
    changed
}

/// Цвет текселя (пипетка).
pub fn pick(image: &Image, x: i32, y: i32) -> Option<Rgba> {
    image.get(x, y)
}

/// Смешать два цвета (`t` от 0 до 1) — для градиента палитры.
pub fn mix(a: Rgba, b: Rgba, t: f32) -> Rgba {
    let t = t.clamp(0.0, 1.0);
    std::array::from_fn(|i| (a[i] as f32 + (b[i] as f32 - a[i] as f32) * t).round() as u8)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::image::CLEAR;

    const RED: Rgba = [255, 0, 0, 255];

    fn changed(a: &Image, b: &Image) -> Vec<(i32, i32)> {
        let mut out = Vec::new();
        for y in 0..a.height() as i32 {
            for x in 0..a.width() as i32 {
                if a.get(x, y) != b.get(x, y) {
                    out.push((x, y));
                }
            }
        }
        out
    }

    #[test]
    fn pencil_and_eraser_touch_exactly_the_brush() {
        let blank = Image::new(8, 8, [10, 10, 10, 255]);
        let mut image = blank.clone();
        assert_eq!(dab(&mut image, 3, 3, 1, RED, None), 1);
        assert_eq!(changed(&blank, &image), [(3, 3)]);
        let mut image = blank.clone();
        assert_eq!(dab(&mut image, 3, 3, 3, RED, None), 9);
        let mut expected: Vec<(i32, i32)> = (2..5).flat_map(|y| (2..5).map(move |x| (x, y))).collect();
        expected.sort_by_key(|p| (p.1, p.0));
        assert_eq!(changed(&blank, &image), expected);
        // Кисть 2 — центр смещён влево вверх; отсечение стороной.
        let mut image = blank.clone();
        dab(&mut image, 0, 0, 2, RED, Some([0, 0, 8, 8]));
        assert_eq!(changed(&blank, &image), [(0, 0), (1, 0), (0, 1), (1, 1)]);
        let mut image = blank.clone();
        dab(&mut image, 4, 4, 3, CLEAR, Some([4, 4, 6, 6]));
        assert_eq!(changed(&blank, &image), [(4, 4), (5, 4), (4, 5), (5, 5)]);
        assert_eq!(image.get(4, 4), Some(CLEAR));
    }

    #[test]
    fn line_rectangle_and_fill() {
        let blank = Image::new(10, 10, CLEAR);
        let mut image = blank.clone();
        line(&mut image, (1, 1), (5, 3), 1, RED, None);
        assert_eq!(changed(&blank, &image), [(1, 1), (2, 2), (3, 2), (4, 3), (5, 3)]);
        let mut image = blank.clone();
        assert_eq!(rectangle(&mut image, (2, 2), (5, 4), RED, false, None), 10);
        // Контур замкнут: заливка внутри не вытекает наружу.
        assert_eq!(flood_fill(&mut image, 3, 3, [0, 0, 255, 255], None), 2);
        assert_eq!(image.get(1, 1), Some(CLEAR));
        let mut image = blank.clone();
        assert_eq!(flood_fill(&mut image, 0, 0, RED, Some([0, 0, 3, 2])), 6, "the face bounds the fill");
        assert_eq!(fill_clip(&mut image, [5, 5, 7, 6], RED), 2);
    }

    #[test]
    fn shade_keeps_alpha_and_skips_transparent() {
        let mut image = Image::new(2, 1, [100, 100, 100, 255]);
        image.set(1, 0, CLEAR);
        shade(&mut image, 0, 0, 3, -20, None);
        assert_eq!(image.get(0, 0), Some([80, 80, 80, 255]));
        assert_eq!(image.get(1, 0), Some(CLEAR));
        assert_eq!(shade_color([250, 5, 0, 7], 10), [255, 15, 10, 7]);
        assert_eq!(mix([0, 0, 0, 255], [255, 255, 255, 255], 0.5), [128, 128, 128, 255]);
    }
}
