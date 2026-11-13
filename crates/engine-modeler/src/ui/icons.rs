//! Векторные иконки окна моделей: рисуются линиями в любом масштабе.

use std::f32::consts::{PI, TAU};

use egui::{pos2, Color32, Painter, Pos2, Rect, Shape, Stroke};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Icon {
    Select,
    Move,
    Resize,
    Rotate,
    Pivot,
    Pencil,
    Eraser,
    Fill,
    Line,
    Rectangle,
    Picker,
    Shade,
    Cuboid,
    Plane,
    Cylinder,
    Cone,
    Sphere,
    Group,
    Mirror,
    Flip,
    Align,
    Center,
    Eye,
    EyeOff,
    Lock,
    Unlock,
    Undo,
    Redo,
    Grid,
    Perspective,
    Orthographic,
    Textured,
    Solid,
    Wireframe,
    Frame,
    Duplicate,
    Trash,
    Plus,
    ChevronRight,
    ChevronDown,
    Link,
    Repack,
    RotateUv,
    FlipU,
    FlipV,
    Swap,
    Close,
    Texture,
    Warning,
    /// Цвета по умолчанию: два квадрата.
    DefaultColors,
}

/// Нарисовать иконку в `rect`.
pub fn paint(painter: &Painter, rect: Rect, icon: Icon, color: Color32) {
    let c = rect.center();
    let s = rect.width().min(rect.height()) * 0.5;
    let at = |x: f32, y: f32| pos2(c.x + x * s, c.y + y * s);
    let w = (s * 0.17).clamp(1.2, 2.0);
    let line = Stroke::new(w, color);
    let thin = Stroke::new((w * 0.7).max(1.0), color);
    let faint = color.gamma_multiply(0.35);
    let seg = |a: (f32, f32), b: (f32, f32)| {
        painter.line_segment([at(a.0, a.1), at(b.0, b.1)], line);
    };
    let poly = |points: &[(f32, f32)], closed: bool| {
        let pts: Vec<Pos2> = points.iter().map(|p| at(p.0, p.1)).collect();
        if closed {
            painter.add(Shape::closed_line(pts, line));
        } else {
            painter.add(Shape::line(pts, line));
        }
    };
    let fill = |points: &[(f32, f32)], color: Color32| {
        let pts: Vec<Pos2> = points.iter().map(|p| at(p.0, p.1)).collect();
        painter.add(Shape::convex_polygon(pts, color, Stroke::NONE));
    };
    let arc = |cx: f32, cy: f32, r: f32, from: f32, to: f32| -> Vec<Pos2> {
        (0..=20)
            .map(|i| {
                let a = from + (to - from) * i as f32 / 20.0;
                at(cx + a.cos() * r, cy + a.sin() * r)
            })
            .collect()
    };
    let arrow_head = |tip: (f32, f32), dir: (f32, f32), size: f32| {
        let len = (dir.0 * dir.0 + dir.1 * dir.1).sqrt().max(1e-6);
        let (dx, dy) = (dir.0 / len, dir.1 / len);
        let (px, py) = (-dy, dx);
        fill(
            &[
                tip,
                (tip.0 - dx * size + px * size * 0.6, tip.1 - dy * size + py * size * 0.6),
                (tip.0 - dx * size - px * size * 0.6, tip.1 - dy * size - py * size * 0.6),
            ],
            color,
        );
    };
    // Кубик в изометрии (общая основа иконок формы).
    let cube = |filled: bool| {
        let top = [(0.0, -0.85), (0.8, -0.42), (0.0, 0.0), (-0.8, -0.42)];
        if filled {
            fill(&top, faint);
        }
        poly(&[(0.0, -0.85), (0.8, -0.42), (0.8, 0.45), (0.0, 0.88), (-0.8, 0.45), (-0.8, -0.42)], true);
        poly(&[(-0.8, -0.42), (0.0, 0.0), (0.8, -0.42)], false);
        seg((0.0, 0.0), (0.0, 0.88));
    };

    match icon {
        Icon::Select => {
            fill(&[(-0.55, -0.85), (0.5, 0.15), (-0.55, 0.3)], color);
            fill(&[(-0.55, 0.3), (-0.05, 0.12), (-0.2, 0.45)], color);
            seg((-0.05, 0.2), (0.25, 0.85));
        }
        Icon::Move => {
            seg((-0.85, 0.0), (0.85, 0.0));
            seg((0.0, -0.85), (0.0, 0.85));
            for dir in [(1.0, 0.0), (-1.0, 0.0), (0.0, 1.0), (0.0, -1.0)] {
                arrow_head((dir.0 * 0.95, dir.1 * 0.95), dir, 0.32);
            }
        }
        Icon::Resize => {
            painter.rect_stroke(Rect::from_min_max(at(-0.85, -0.2), at(0.2, 0.85)), 1.0, thin);
            seg((-0.1, 0.1), (0.75, -0.75));
            arrow_head((0.85, -0.85), (1.0, -1.0), 0.38);
            painter.rect_filled(Rect::from_min_max(at(-0.85, -0.2), at(0.2, 0.85)), 1.0, faint);
        }
        Icon::Rotate => {
            painter.add(Shape::line(arc(0.0, 0.0, 0.72, -PI * 0.15, PI * 1.45), line));
            let end = -PI * 0.15;
            arrow_head((0.72 * end.cos() + 0.08, 0.72 * end.sin() + 0.12), (0.4, 1.0), 0.36);
        }
        Icon::Pivot => {
            painter.circle_stroke(at(0.0, 0.0), s * 0.55, line);
            painter.circle_filled(at(0.0, 0.0), s * 0.18, color);
            for (a, b) in [((0.0, -0.95), (0.0, -0.6)), ((0.0, 0.6), (0.0, 0.95)), ((-0.95, 0.0), (-0.6, 0.0)), ((0.6, 0.0), (0.95, 0.0))] {
                seg(a, b);
            }
        }
        Icon::Pencil => {
            poly(&[(-0.75, 0.75), (-0.62, 0.3), (0.45, -0.77), (0.77, -0.45), (-0.3, 0.62)], true);
            fill(&[(-0.75, 0.75), (-0.62, 0.3), (-0.3, 0.62)], color);
            seg((0.25, -0.57), (0.57, -0.25));
        }
        Icon::Eraser => {
            poly(&[(-0.85, 0.35), (0.05, -0.55), (0.75, 0.15), (0.05, 0.85), (-0.35, 0.85)], true);
            fill(&[(-0.85, 0.35), (-0.4, -0.1), (0.3, 0.6), (0.05, 0.85), (-0.35, 0.85)], faint);
            seg((-0.4, -0.1), (0.3, 0.6));
            seg((-0.35, 0.85), (0.85, 0.85));
        }
        Icon::Fill => {
            poly(&[(-0.75, -0.05), (-0.05, -0.75), (0.6, -0.1), (-0.1, 0.6)], true);
            fill(&[(-0.75, -0.05), (0.6, -0.1), (-0.1, 0.6)], faint);
            seg((-0.35, -0.45), (-0.65, -0.75));
            painter.circle_filled(at(0.75, 0.55), s * 0.17, color);
            fill(&[(0.75, 0.2), (0.6, 0.5), (0.9, 0.5)], color);
        }
        Icon::Line => {
            seg((-0.75, 0.75), (0.75, -0.75));
            painter.circle_filled(at(-0.75, 0.75), s * 0.16, color);
            painter.circle_filled(at(0.75, -0.75), s * 0.16, color);
        }
        Icon::Rectangle => {
            painter.rect_stroke(Rect::from_min_max(at(-0.8, -0.6), at(0.8, 0.6)), 1.5, line);
        }
        Icon::Picker => {
            poly(&[(-0.8, 0.8), (-0.75, 0.45), (0.15, -0.45), (0.45, -0.15), (-0.45, 0.75)], true);
            fill(&[(0.05, -0.55), (0.45, -0.95), (0.95, -0.45), (0.55, -0.05)], color);
        }
        Icon::Shade => {
            painter.circle_stroke(at(0.0, 0.0), s * 0.78, line);
            let half: Vec<Pos2> = arc(0.0, 0.0, 0.78, -PI * 0.5, PI * 0.5);
            let mut pts = half;
            pts.push(at(0.0, -0.78));
            painter.add(Shape::convex_polygon(pts, color, Stroke::NONE));
        }
        Icon::Cuboid => cube(true),
        Icon::Plane => {
            fill(&[(0.0, -0.45), (0.9, 0.0), (0.0, 0.45), (-0.9, 0.0)], faint);
            poly(&[(0.0, -0.45), (0.9, 0.0), (0.0, 0.45), (-0.9, 0.0)], true);
        }
        Icon::Cylinder => {
            painter.add(Shape::ellipse_stroke(at(0.0, -0.55), egui::vec2(s * 0.7, s * 0.28), line));
            painter.add(Shape::line(
                (0..=16).map(|i| {
                    let a = PI * i as f32 / 16.0;
                    at(a.cos() * 0.7, 0.55 + a.sin() * 0.28)
                }).collect(),
                line,
            ));
            seg((-0.7, -0.55), (-0.7, 0.55));
            seg((0.7, -0.55), (0.7, 0.55));
        }
        Icon::Cone => {
            painter.add(Shape::line(
                (0..=16).map(|i| {
                    let a = PI * i as f32 / 16.0;
                    at(a.cos() * 0.75, 0.55 + a.sin() * 0.28)
                }).collect(),
                line,
            ));
            poly(&[(-0.75, 0.55), (0.0, -0.9), (0.75, 0.55)], false);
        }
        Icon::Sphere => {
            painter.circle_stroke(at(0.0, 0.0), s * 0.82, line);
            painter.add(Shape::ellipse_stroke(at(0.0, 0.0), egui::vec2(s * 0.82, s * 0.3), thin));
            painter.add(Shape::ellipse_stroke(at(0.0, 0.0), egui::vec2(s * 0.32, s * 0.82), thin));
        }
        Icon::Group => {
            poly(&[(-0.9, -0.6), (-0.3, -0.6), (-0.15, -0.4), (0.9, -0.4), (0.9, 0.75), (-0.9, 0.75)], true);
            fill(&[(-0.9, -0.2), (0.9, -0.2), (0.9, 0.75), (-0.9, 0.75)], faint);
        }
        Icon::Mirror => {
            painter.add(Shape::dashed_line(&[at(0.0, -0.95), at(0.0, 0.95)], thin, 2.0, 2.0));
            // Квадрат и его отражение по оси: залитый оригинал, контурный близнец.
            painter.rect_filled(Rect::from_min_max(at(-0.85, -0.45), at(-0.25, 0.45)), 1.0, color);
            painter.rect_stroke(Rect::from_min_max(at(0.25, -0.45), at(0.85, 0.45)), 1.0, line);
        }
        Icon::Flip => {
            seg((0.0, -0.9), (0.0, 0.9));
            painter.add(Shape::line(arc(0.0, 0.1, 0.6, PI * 1.15, PI * 1.85), line));
            arrow_head((0.52, -0.22), (0.5, 0.6), 0.3);
            arrow_head((-0.52, -0.22), (-0.5, 0.6), 0.3);
        }
        Icon::Align => {
            seg((-0.85, -0.9), (-0.85, 0.9));
            painter.rect_filled(Rect::from_min_max(at(-0.65, -0.65), at(0.75, -0.15)), 1.0, color);
            painter.rect_filled(Rect::from_min_max(at(-0.65, 0.15), at(0.3, 0.65)), 1.0, color);
        }
        Icon::Center => {
            painter.rect_stroke(Rect::from_min_max(at(-0.55, -0.55), at(0.55, 0.55)), 1.0, line);
            seg((0.0, -0.95), (0.0, -0.7));
            seg((0.0, 0.7), (0.0, 0.95));
            seg((-0.95, 0.0), (-0.7, 0.0));
            seg((0.7, 0.0), (0.95, 0.0));
            painter.circle_filled(at(0.0, 0.0), s * 0.14, color);
        }
        Icon::Eye | Icon::EyeOff => {
            let upper: Vec<Pos2> = (0..=16)
                .map(|i| {
                    let t = i as f32 / 16.0;
                    at(-0.9 + 1.8 * t, -(t * PI).sin() * 0.55)
                })
                .collect();
            let lower: Vec<Pos2> = (0..=16)
                .map(|i| {
                    let t = i as f32 / 16.0;
                    at(-0.9 + 1.8 * t, (t * PI).sin() * 0.55)
                })
                .collect();
            painter.add(Shape::line(upper, line));
            painter.add(Shape::line(lower, line));
            painter.circle_filled(at(0.0, 0.0), s * 0.24, color);
            if icon == Icon::EyeOff {
                seg((-0.8, 0.8), (0.8, -0.8));
            }
        }
        Icon::Lock | Icon::Unlock => {
            painter.rect_filled(Rect::from_min_max(at(-0.65, -0.05), at(0.65, 0.85)), 2.0, color);
            let (from, to) = if icon == Icon::Lock { (PI, TAU) } else { (PI * 1.2, TAU * 1.0) };
            let mut shackle = arc(0.0, -0.35, 0.42, from, to);
            if icon == Icon::Lock {
                shackle.insert(0, at(-0.42, -0.05));
                shackle.push(at(0.42, -0.05));
            } else {
                shackle.push(at(0.42, -0.05));
            }
            painter.add(Shape::line(shackle, line));
        }
        Icon::Undo | Icon::Redo => {
            let flip = if icon == Icon::Undo { 1.0 } else { -1.0 };
            let pts: Vec<Pos2> = (0..=16)
                .map(|i| {
                    let a = -PI * 0.5 + PI * i as f32 / 16.0;
                    pos2(c.x + flip * (0.1 + a.cos() * 0.6) * s, c.y + (0.1 + a.sin() * 0.5) * s)
                })
                .collect();
            painter.add(Shape::line(pts, line));
            painter.line_segment([at(flip * 0.1, 0.6), at(-flip * 0.5, 0.6)], line);
            arrow_head((-flip * 0.75, -0.4), (-flip, 0.0), 0.34);
            painter.line_segment([at(flip * 0.1, -0.4), at(-flip * 0.45, -0.4)], line);
        }
        Icon::Grid => {
            for k in [-0.8, -0.27, 0.27, 0.8] {
                painter.line_segment([at(k, -0.8), at(k, 0.8)], thin);
                painter.line_segment([at(-0.8, k), at(0.8, k)], thin);
            }
        }
        Icon::Perspective => {
            poly(&[(-0.85, 0.7), (-0.35, -0.6), (0.35, -0.6), (0.85, 0.7)], true);
            seg((0.0, -0.6), (0.0, 0.7));
            seg((-0.6, 0.05), (0.6, 0.05));
        }
        Icon::Orthographic => {
            painter.rect_stroke(Rect::from_min_max(at(-0.75, -0.65), at(0.75, 0.65)), 0.0, line);
            seg((0.0, -0.65), (0.0, 0.65));
            seg((-0.75, 0.0), (0.75, 0.0));
        }
        Icon::Textured => {
            painter.rect_stroke(Rect::from_min_max(at(-0.8, -0.8), at(0.8, 0.8)), 1.0, line);
            for (x, y) in [(-0.8, -0.8), (0.0, 0.0), (-0.8, 0.0), (0.0, -0.8)] {
                if (x + y) as i32 % 2 == 0 || x == y {
                    painter.rect_filled(Rect::from_min_max(at(x, y), at(x + 0.8, y + 0.8)), 0.0, color);
                }
            }
        }
        Icon::Solid => {
            painter.circle_filled(at(0.0, 0.0), s * 0.8, color.gamma_multiply(0.5));
            let lit: Vec<Pos2> = arc(0.0, 0.0, 0.8, PI * 0.75, PI * 1.75);
            painter.add(Shape::convex_polygon(lit, color, Stroke::NONE));
        }
        Icon::Wireframe => {
            cube(false);
        }
        Icon::Frame => {
            for (x, y) in [(-1.0, -1.0), (1.0, -1.0), (1.0, 1.0), (-1.0, 1.0)] {
                poly(&[(x * 0.85, y * 0.4), (x * 0.85, y * 0.85), (x * 0.4, y * 0.85)], false);
            }
            painter.circle_filled(at(0.0, 0.0), s * 0.2, color);
        }
        Icon::Duplicate => {
            painter.rect_stroke(Rect::from_min_max(at(-0.85, -0.85), at(0.35, 0.35)), 1.5, line);
            painter.rect_filled(Rect::from_min_max(at(-0.35, -0.35), at(0.85, 0.85)), 1.5, color);
        }
        Icon::Trash => {
            seg((-0.8, -0.6), (0.8, -0.6));
            poly(&[(-0.3, -0.6), (-0.2, -0.85), (0.2, -0.85), (0.3, -0.6)], false);
            poly(&[(-0.6, -0.6), (-0.5, 0.85), (0.5, 0.85), (0.6, -0.6)], false);
            seg((-0.17, -0.3), (-0.12, 0.55));
            seg((0.17, -0.3), (0.12, 0.55));
        }
        Icon::Plus => {
            seg((-0.75, 0.0), (0.75, 0.0));
            seg((0.0, -0.75), (0.0, 0.75));
        }
        Icon::ChevronRight => poly(&[(-0.3, -0.7), (0.4, 0.0), (-0.3, 0.7)], false),
        Icon::ChevronDown => poly(&[(-0.7, -0.3), (0.0, 0.4), (0.7, -0.3)], false),
        Icon::Link => {
            painter.add(Shape::closed_line(arc(-0.35, 0.0, 0.42, 0.0, TAU), thin));
            painter.add(Shape::closed_line(arc(0.35, 0.0, 0.42, 0.0, TAU), thin));
        }
        Icon::Repack => {
            for (x, y, w, h) in [(-0.9, -0.9, 1.0, 0.8), (0.2, -0.9, 0.7, 0.8), (-0.9, 0.0, 0.6, 0.9), (-0.2, 0.0, 1.1, 0.9)] {
                painter.rect_stroke(Rect::from_min_max(at(x + 0.05, y + 0.05), at(x + w - 0.05, y + h - 0.05)), 0.0, thin);
            }
        }
        Icon::RotateUv => {
            painter.rect_stroke(Rect::from_min_max(at(-0.45, -0.45), at(0.45, 0.45)), 0.0, thin);
            painter.add(Shape::line(arc(0.0, 0.0, 0.8, -PI * 0.9, -PI * 0.1), line));
            arrow_head((0.78, -0.12), (0.2, 1.0), 0.3);
        }
        Icon::FlipU => {
            painter.add(Shape::dashed_line(&[at(0.0, -0.9), at(0.0, 0.9)], thin, 2.0, 2.0));
            arrow_head((-0.85, 0.0), (-1.0, 0.0), 0.35);
            arrow_head((0.85, 0.0), (1.0, 0.0), 0.35);
            seg((-0.6, 0.0), (-0.2, 0.0));
            seg((0.2, 0.0), (0.6, 0.0));
        }
        Icon::FlipV => {
            painter.add(Shape::dashed_line(&[at(-0.9, 0.0), at(0.9, 0.0)], thin, 2.0, 2.0));
            arrow_head((0.0, -0.85), (0.0, -1.0), 0.35);
            arrow_head((0.0, 0.85), (0.0, 1.0), 0.35);
            seg((0.0, -0.6), (0.0, -0.2));
            seg((0.0, 0.2), (0.0, 0.6));
        }
        Icon::Swap => {
            painter.add(Shape::line(arc(0.0, 0.0, 0.7, PI * 1.1, PI * 1.9), line));
            arrow_head((0.67, -0.2), (0.3, 1.0), 0.3);
            painter.add(Shape::line(arc(0.0, 0.0, 0.7, PI * 0.1, PI * 0.9), line));
            arrow_head((-0.67, 0.2), (-0.3, -1.0), 0.3);
        }
        Icon::Close => {
            seg((-0.6, -0.6), (0.6, 0.6));
            seg((-0.6, 0.6), (0.6, -0.6));
        }
        Icon::Texture => {
            painter.rect_stroke(Rect::from_min_max(at(-0.85, -0.85), at(0.85, 0.85)), 1.0, line);
            fill(&[(-0.85, 0.85), (-0.2, 0.0), (0.25, 0.45), (0.5, 0.2), (0.85, 0.6), (0.85, 0.85)], faint);
            painter.circle_filled(at(0.35, -0.4), s * 0.17, color);
        }
        Icon::DefaultColors => {
            painter.rect_filled(Rect::from_min_max(at(-0.1, -0.1), at(0.85, 0.85)), 1.0, faint);
            painter.rect_stroke(Rect::from_min_max(at(-0.1, -0.1), at(0.85, 0.85)), 1.0, thin);
            painter.rect_filled(Rect::from_min_max(at(-0.85, -0.85), at(0.15, 0.15)), 1.0, color);
        }
        Icon::Warning => {
            poly(&[(0.0, -0.85), (0.9, 0.75), (-0.9, 0.75)], true);
            seg((0.0, -0.3), (0.0, 0.25));
            painter.circle_filled(at(0.0, 0.5), s * 0.09, color);
        }
    }
}
