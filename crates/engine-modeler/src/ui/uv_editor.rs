//! UV-вид: текстура с прямоугольниками сторон выделенного. В режиме Edit прямоугольник
//! стороны двигается и растягивается мышью (с привязкой к текселям), поворачивается на
//! 90° и отражается; в режиме Paint по текстуре рисуют теми же инструментами.

use std::collections::HashSet;

use egui::{pos2, vec2, Color32, Pos2, Rect, Sense, Stroke, Ui};

use super::icons::Icon;
use super::style::{self, Palette};
use super::{Command, FillMode, Mode, PaintSettings, Tool};
use crate::doc::{FaceDir, NodeId, UvRect};
use crate::image::{Image, CLEAR};
use crate::ops;
use crate::paint;
use crate::session::Session;

#[derive(Clone, Copy, Debug, PartialEq)]
enum UvDrag {
    Move { node: NodeId, face: FaceDir, start: Pos2, rect: UvRect },
    Scale { node: NodeId, face: FaceDir, start: Pos2, rect: UvRect },
    Paint { last: (i32, i32), anchor: (i32, i32) },
}

pub(super) struct UvEditor {
    /// Показанная текстура.
    pub texture: u32,
    /// Пикселей экрана на тексель (0 — вписать в ширину).
    zoom: f32,
    pan: egui::Vec2,
    handle: Option<(egui::TextureHandle, Image)>,
    drag: Option<UvDrag>,
    touched: HashSet<(i32, i32)>,
    pub canvas: Option<Rect>,
}

impl Default for UvEditor {
    fn default() -> Self {
        Self {
            texture: 0,
            zoom: 0.0,
            pan: egui::Vec2::ZERO,
            handle: None,
            drag: None,
            touched: HashSet::new(),
            canvas: None,
        }
    }
}

const FACE_TAGS: [&str; 6] = ["N", "E", "S", "W", "U", "D"];

impl UvEditor {
    pub fn show(&mut self, ui: &mut Ui, session: &mut Session, mode: Mode, tool: Tool, paint_settings: &mut PaintSettings) -> Option<Command> {
        let p = Palette::from_ctx(ui.ctx());
        let mut command = None;
        // Текстура выделенной стороны — сама становится показанной.
        if let (Some(id), Some(face)) = (session.primary(), session.face) {
            if let Some(f) = session.doc.element(id).map(|e| *e.face(face)) {
                if self.drag.is_none() {
                    self.texture = f.texture;
                }
            }
        }
        if self.texture as usize >= session.doc.textures.len() {
            self.texture = 0;
        }
        let texture = session.doc.textures.get(self.texture as usize)?;
        let image = texture.image.clone();
        let texture_name = texture.name.clone();
        let [tw, th] = image.size();

        // Картинка для egui (перезаливается, только когда пиксели изменились).
        let stale = self.handle.as_ref().is_none_or(|(_, cached)| !cached.shares_pixels(&image) || cached.size() != image.size());
        if stale {
            let pixels = image.pixels().iter().map(|c| Color32::from_rgba_unmultiplied(c[0], c[1], c[2], c[3])).collect();
            let color = egui::ColorImage { size: [tw as usize, th as usize], pixels };
            match &mut self.handle {
                Some((handle, cached)) => {
                    handle.set(color, egui::TextureOptions::NEAREST);
                    *cached = image.clone();
                }
                None => {
                    let handle = ui.ctx().load_texture("modeler.uv", color, egui::TextureOptions::NEAREST);
                    self.handle = Some((handle, image.clone()));
                }
            }
        }

        egui::Frame::none().inner_margin(egui::Margin::symmetric(10.0, 8.0)).show(ui, |ui| {
            ui.spacing_mut().item_spacing = vec2(4.0, 6.0);
            ui.horizontal(|ui| {
                ui.label(egui::RichText::new(format!("{texture_name}  {tw}×{th}")).size(11.5).color(p.text_muted));
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    ui.spacing_mut().item_spacing.x = 1.0;
                    if style::icon_button(ui, "modeler.uv.repack", Icon::Repack, false, true, &style::tip("Repack", "", "Pack every box layout tightly; painted texels move along"))
                        .clicked()
                    {
                        command = Some(Command::Repack);
                    }
                    if style::icon_button(ui, "modeler.uv.auto", Icon::Cuboid, false, true, &style::tip("Auto UV", "", "Box layout for the selection (or everything)"))
                        .clicked()
                    {
                        command = Some(Command::AutoUv);
                    }
                });
            });

            // Холст: ширина панели, высота по пропорции (не выше 1.25 ширины).
            let width = ui.available_width();
            let height = (width * th as f32 / tw as f32).clamp(80.0, width * 1.25);
            let (outer, _) = ui.allocate_exact_size(vec2(width, height), Sense::hover());
            self.canvas = Some(outer);
            let response = ui.interact(outer, egui::Id::new("modeler.uv.canvas"), Sense::click_and_drag());
            let fit = (outer.width() / tw as f32).min(outer.height() / th as f32);
            let zoom = if self.zoom <= 0.0 { fit } else { self.zoom };
            // Колесо — масштаб вокруг курсора; средняя кнопка — сдвиг; двойной щелчок — вписать.
            if response.hovered() {
                let scroll = ui.input(|i| i.smooth_scroll_delta.y);
                if scroll != 0.0 {
                    let new_zoom = (zoom * (scroll * 0.002).exp()).clamp(fit * 0.5, 64.0);
                    if let Some(pos) = response.hover_pos() {
                        let origin = outer.center() + self.pan;
                        let rel = (pos - origin) / zoom;
                        self.pan += rel * zoom - rel * new_zoom;
                    }
                    self.zoom = new_zoom;
                }
            }
            if response.double_clicked_by(egui::PointerButton::Middle) || (response.double_clicked() && mode == Mode::Edit && self.hit_face(session, outer, zoom, response.interact_pointer_pos()).is_none()) {
                self.zoom = 0.0;
                self.pan = egui::Vec2::ZERO;
            }
            if response.dragged_by(egui::PointerButton::Middle) || response.dragged_by(egui::PointerButton::Secondary) {
                self.pan += response.drag_delta();
            }
            let zoom = if self.zoom <= 0.0 { fit } else { self.zoom };
            let size = vec2(tw as f32, th as f32) * zoom;
            let image_rect = Rect::from_center_size(outer.center() + self.pan, size);
            let to_screen = |x: f32, y: f32| image_rect.min + vec2(x, y) * zoom;
            let to_texel = |pos: Pos2| {
                let v = (pos - image_rect.min) / zoom;
                (v.x.floor() as i32, v.y.floor() as i32)
            };

            let painter = ui.painter_at(outer);
            painter.rect_filled(outer, 0.0, p.bg_inset);
            style::checker(&painter, image_rect.intersect(outer), (zoom * 2.0).clamp(4.0, 12.0));
            if let Some((handle, _)) = &self.handle {
                painter.image(handle.id(), image_rect, Rect::from_min_max(pos2(0.0, 0.0), pos2(1.0, 1.0)), Color32::WHITE);
            }
            painter.rect_stroke(image_rect, 0.0, Stroke::new(1.0_f32, p.line_strong));
            // Сетка пикселей при крупном масштабе.
            if zoom >= 6.0 {
                let grid = Stroke::new(1.0_f32, Color32::from_black_alpha(if zoom >= 12.0 { 60 } else { 35 }));
                for x in 1..tw {
                    let sx = image_rect.min.x + x as f32 * zoom;
                    if sx > outer.min.x && sx < outer.max.x {
                        painter.line_segment([pos2(sx, image_rect.min.y.max(outer.min.y)), pos2(sx, image_rect.max.y.min(outer.max.y))], grid);
                    }
                }
                for y in 1..th {
                    let sy = image_rect.min.y + y as f32 * zoom;
                    if sy > outer.min.y && sy < outer.max.y {
                        painter.line_segment([pos2(image_rect.min.x.max(outer.min.x), sy), pos2(image_rect.max.x.min(outer.max.x), sy)], grid);
                    }
                }
            }

            // Прямоугольники сторон выделенных элементов.
            let selected = session.selected_elements();
            for id in &selected {
                let Some(e) = session.doc.element(*id) else { continue };
                for dir in e.shape.faces() {
                    let face = e.face(*dir);
                    if !face.enabled || face.texture != self.texture {
                        continue;
                    }
                    let [x0, y0, x1, y1] = face.uv.texel_bounds();
                    let r = Rect::from_min_max(to_screen(x0 as f32, y0 as f32), to_screen(x1 as f32, y1 as f32));
                    let active = Some(*id) == session.primary() && session.face == Some(*dir);
                    let color = if active { p.accent } else { Color32::from_white_alpha(170) };
                    painter.rect_stroke(r, 0.0, Stroke::new(if active { 2.0_f32 } else { 1.0 }, color));
                    if mode == Mode::Edit && r.width() > 12.0 && r.height() > 10.0 {
                        painter.text(r.min + vec2(3.0, 2.0), egui::Align2::LEFT_TOP, FACE_TAGS[dir.index()], egui::FontId::proportional(10.0), color);
                    }
                    if active && mode == Mode::Edit {
                        painter.rect_filled(Rect::from_center_size(r.max, vec2(8.0, 8.0)), 1.0, p.accent);
                    }
                }
            }

            // Ввод.
            let pointer = response.interact_pointer_pos().or(response.hover_pos());
            match mode {
                Mode::Edit => self.edit_input(ui, &response, session, image_rect, zoom),
                Mode::Paint => self.paint_input(ui, &response, session, tool, paint_settings, &to_texel),
            }
            if mode == Mode::Paint {
                if let Some(pos) = pointer.filter(|p| outer.contains(*p)) {
                    let (x, y) = to_texel(pos);
                    let n = paint_settings.size.max(1) as i32;
                    let s = -(n - 1) / 2;
                    let r = Rect::from_min_max(to_screen((x + s) as f32, (y + s) as f32), to_screen((x + s + n) as f32, (y + s + n) as f32));
                    painter.rect_stroke(r, 0.0, Stroke::new(2.0_f32, Color32::from_black_alpha(140)));
                    painter.rect_stroke(r, 0.0, Stroke::new(1.0_f32, Color32::WHITE));
                }
            }

            // Кнопки выбранной стороны.
            if mode == Mode::Edit {
                if let (Some(id), Some(face)) = (session.primary(), session.face) {
                    if let Some(rect) = session.doc.element(id).map(|e| e.face(face).uv) {
                        ui.horizontal(|ui| {
                            ui.spacing_mut().item_spacing.x = 2.0;
                            ui.label(egui::RichText::new(engine_i18n::trf!("{face} face", face = style::face_name(face))).size(11.5).color(p.text_muted));
                            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                                let mut changed = None;
                                if style::icon_button(ui, "modeler.uv.flip_v", Icon::FlipV, rect.flip_v, true, "Mirror vertically").clicked() {
                                    let mut r = rect;
                                    r.flip_v = !r.flip_v;
                                    changed = Some(r);
                                }
                                if style::icon_button(ui, "modeler.uv.flip_u", Icon::FlipU, rect.flip_u, true, "Mirror horizontally").clicked() {
                                    let mut r = rect;
                                    r.flip_u = !r.flip_u;
                                    changed = Some(r);
                                }
                                if style::icon_button(ui, "modeler.uv.rotate", Icon::RotateUv, false, true, "Rotate the picture on the face by 90°").clicked() {
                                    let mut r = rect;
                                    r.rotation = (r.rotation + 1) % 4;
                                    changed = Some(r);
                                }
                                if let Some(r) = changed {
                                    session.edit("Edit UV", |doc| ops::set_face_uv(doc, id, face, r));
                                }
                            });
                        });
                        ui.horizontal(|ui| {
                            let mut r = rect;
                            let mut edited = false;
                            for (label, value) in [("X", &mut r.x), ("Y", &mut r.y), ("W", &mut r.w), ("H", &mut r.h)] {
                                let prefix = format!("{label} ");
                                edited |= super::controls::number_field_ex(ui, value, f32::MIN..=f32::MAX, 0.25, &prefix, "", 2, 64.0).changed();
                            }
                            if edited {
                                session.edit("Edit UV", |doc| ops::set_face_uv(doc, id, face, r));
                            }
                        });
                    }
                } else if !selected.is_empty() {
                    ui.label(egui::RichText::new(engine_i18n::tr("Click a face rectangle to edit it")).size(11.0).color(p.text_faint));
                }
            }
        });
        command
    }

    fn hit_face(&self, session: &Session, outer: Rect, zoom: f32, pos: Option<Pos2>) -> Option<(NodeId, FaceDir, UvRect)> {
        let pos = pos?;
        let doc = &session.doc;
        let texture = doc.textures.get(self.texture as usize)?;
        let size = vec2(texture.image.width() as f32, texture.image.height() as f32) * zoom;
        let image_rect = Rect::from_center_size(outer.center() + self.pan, size);
        let texel = (pos - image_rect.min) / zoom;
        // Сначала главный выделенный, потом остальные.
        let mut order = session.selected_elements();
        if let Some(primary) = session.primary() {
            order.retain(|id| *id != primary);
            order.insert(0, primary);
        }
        for id in order {
            let Some(e) = doc.element(id) else { continue };
            for dir in e.shape.faces() {
                let face = e.face(*dir);
                if !face.enabled || face.texture != self.texture {
                    continue;
                }
                let [x0, y0, x1, y1] = face.uv.texel_bounds();
                if texel.x >= x0 as f32 && texel.x < x1 as f32 && texel.y >= y0 as f32 && texel.y < y1 as f32 {
                    return Some((id, *dir, face.uv));
                }
            }
        }
        None
    }

    fn edit_input(&mut self, ui: &Ui, response: &egui::Response, session: &mut Session, image_rect: Rect, zoom: f32) {
        let outer = self.canvas.unwrap_or(image_rect);
        if response.drag_started_by(egui::PointerButton::Primary) {
            let start = ui.input(|i| i.pointer.press_origin()).unwrap_or_default();
            // Угол выбранной стороны — растяжение.
            let corner = session.primary().zip(session.face).and_then(|(id, face)| {
                let rect = session.doc.element(id)?.face(face).uv;
                let [_, _, x1, y1] = rect.texel_bounds();
                let corner = image_rect.min + vec2(x1 as f32, y1 as f32) * zoom;
                (corner.distance(start) < 8.0).then_some((id, face, rect))
            });
            if let Some((node, face, rect)) = corner {
                session.begin("Scale UV");
                self.drag = Some(UvDrag::Scale { node, face, start, rect });
            } else if let Some((node, face, rect)) = self.hit_face(session, outer, zoom, Some(start)) {
                if session.primary() != Some(node) {
                    session.select(Some(node));
                }
                session.face = Some(face);
                session.begin("Move UV");
                self.drag = Some(UvDrag::Move { node, face, start, rect });
            }
        }
        if response.clicked() {
            if let Some((node, face, _)) = self.hit_face(session, outer, zoom, response.interact_pointer_pos()) {
                if session.primary() != Some(node) {
                    session.select(Some(node));
                }
                session.face = Some(face);
            }
        }
        if let Some(drag) = self.drag {
            let pos = ui.input(|i| i.pointer.interact_pos());
            if let (Some(pos), Some(base)) = (pos, session.pending_base().cloned()) {
                match drag {
                    UvDrag::Move { node, face, start, rect } => {
                        let d = (pos - start) / zoom;
                        let mut r = rect;
                        r.x = (rect.x + d.x).round();
                        r.y = (rect.y + d.y).round();
                        let mut doc = base;
                        ops::set_face_uv(&mut doc, node, face, r);
                        session.doc = doc;
                    }
                    UvDrag::Scale { node, face, start, rect } => {
                        let d = (pos - start) / zoom;
                        let mut r = rect;
                        r.w = (rect.w + d.x).round().max(1.0);
                        r.h = (rect.h + d.y).round().max(1.0);
                        let mut doc = base;
                        ops::set_face_uv(&mut doc, node, face, r);
                        session.doc = doc;
                    }
                    _ => {}
                }
            }
            if !ui.input(|i| i.pointer.primary_down()) {
                session.commit();
                self.drag = None;
            }
        }
    }

    #[allow(clippy::too_many_arguments)]
    fn paint_input(
        &mut self,
        ui: &Ui,
        response: &egui::Response,
        session: &mut Session,
        tool: Tool,
        settings: &mut PaintSettings,
        to_texel: &dyn Fn(Pos2) -> (i32, i32),
    ) {
        let (pressed, down, pos, shift) = ui.input(|i| (i.pointer.primary_pressed(), i.pointer.primary_down(), i.pointer.interact_pos(), i.modifiers.shift));
        let Some(pos) = pos else { return };
        let texture = self.texture as usize;
        let inside = self.canvas.is_some_and(|c| c.contains(pos)) && response.hovered();
        let (x, y) = to_texel(pos);
        // Сторона под текселем (заливка «сторона» и отсечение).
        let face_clip = |session: &Session| -> Option<paint::Clip> {
            session.doc.elements().find_map(|(_, e)| {
                e.shape.faces().iter().map(|d| e.face(*d)).find_map(|f| {
                    let b = f.uv.texel_bounds();
                    (f.enabled && f.texture as usize == texture && x >= b[0] && y >= b[1] && x < b[2] && y < b[3]).then_some(b)
                })
            })
        };
        if pressed && inside && self.drag.is_none() {
            match tool {
                Tool::Picker => {
                    if let Some(c) = session.doc.textures.get(texture).and_then(|t| t.image.get(x, y)) {
                        settings.color = c;
                        settings.remember(c);
                    }
                }
                Tool::Fill => {
                    let color = settings.color;
                    let clip = face_clip(session);
                    let mode = settings.fill;
                    session.edit("Fill", |doc| {
                        if let Some(t) = doc.textures.get_mut(texture) {
                            match (mode, clip) {
                                (FillMode::Face, Some(clip)) => paint::fill_clip(&mut t.image, clip, color),
                                _ => paint::flood_fill(&mut t.image, x, y, color, clip),
                            };
                        }
                    });
                    settings.remember(color);
                }
                _ => {
                    session.begin("Paint Texture");
                    self.touched.clear();
                    self.drag = Some(UvDrag::Paint { last: (x, y), anchor: (x, y) });
                    self.paint_at(session, tool, settings, (x, y), (x, y), (x, y), shift);
                }
            }
        }
        if let Some(UvDrag::Paint { last, anchor }) = self.drag {
            if down {
                if (x, y) != last {
                    self.paint_at(session, tool, settings, last, (x, y), anchor, shift);
                    self.drag = Some(UvDrag::Paint { last: (x, y), anchor });
                }
            } else {
                session.commit();
                if matches!(tool, Tool::Pencil | Tool::Line | Tool::Rectangle) {
                    settings.remember(settings.color);
                }
                self.drag = None;
            }
        }
    }

    #[allow(clippy::too_many_arguments)]
    fn paint_at(&mut self, session: &mut Session, tool: Tool, settings: &PaintSettings, from: (i32, i32), to: (i32, i32), anchor: (i32, i32), shift: bool) {
        let texture = self.texture as usize;
        let base = session.pending_base().and_then(|d| d.textures.get(texture)).map(|t| t.image.clone());
        let Some(t) = session.doc.textures.get_mut(texture) else { return };
        let size = settings.size;
        match tool {
            Tool::Pencil => {
                paint::line(&mut t.image, from, to, size, settings.color, None);
            }
            Tool::Eraser => {
                paint::line(&mut t.image, from, to, size, CLEAR, None);
            }
            Tool::Shade => {
                let amount = if shift != settings.darken { -settings.shade } else { settings.shade };
                for (lx, ly) in paint::line_texels(from, to) {
                    for (px, py) in paint::brush_texels(lx, ly, size) {
                        if self.touched.insert((px, py)) {
                            if let Some(c) = t.image.get(px, py).filter(|c| c[3] > 0) {
                                t.image.set(px, py, paint::shade_color(c, amount));
                            }
                        }
                    }
                }
            }
            Tool::Line | Tool::Rectangle => {
                if let Some(base) = base {
                    t.image = base;
                }
                if tool == Tool::Line {
                    paint::line(&mut t.image, anchor, to, size, settings.color, None);
                } else {
                    paint::rectangle(&mut t.image, anchor, to, settings.color, shift != settings.filled, None);
                }
            }
            _ => {}
        }
    }
}
