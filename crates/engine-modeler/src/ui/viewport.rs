//! 3D-вид: картинка программного растеризатора, камера, выделение, гизмо, рисование по
//! модели, линейка размеров и значок ориентации.

use std::collections::HashSet;

use egui::{pos2, vec2, Color32, PointerButton, Pos2, Rect, Sense, Shape as EShape, Stroke, Ui};
use engine_i18n::{tr_s, trn};
use glam::{Vec2, Vec3};

use super::style::{self, Palette};
use super::{Mode, PaintSettings, Tool, ViewSettings};
use crate::camera::{raycast, Camera, Ray, ViewPreset};
use crate::doc::{Document, FaceDir, NodeId, NodeKind};
use crate::geometry::{build_mesh, BuildOptions, BuiltMesh, Polygon};
use crate::image::CLEAR;
use crate::paint::{self, Clip};
use crate::raster::{self, Frame, RenderOptions};
use crate::session::Session;
use crate::ops;

/// Что окну моделей нужно от хозяина на этот кадр.
pub(super) struct Context<'a> {
    pub session: &'a mut Session,
    pub mode: Mode,
    pub tool: Tool,
    pub paint: &'a mut PaintSettings,
    pub view: &'a ViewSettings,
}

/// Ручка гизмо.
#[derive(Clone, Copy, Debug, PartialEq)]
enum Handle {
    Axis(usize),
    Plane,
    Face(FaceDir),
    Ring(usize),
}

#[derive(Clone, Debug)]
enum Drag {
    Gizmo {
        handle: Handle,
        origin: Vec3,
        axis: Vec3,
        /// Параметр на оси / угол / точка на плоскости в начале.
        start: f32,
        start_point: Vec3,
        ids: Vec<NodeId>,
    },
    Box {
        start: Pos2,
    },
    /// Сдвиг вершин (стрелки осей) или грани (стрелка нормали) меша.
    Mesh {
        handle: Handle,
        origin: Vec3,
        axis: Vec3,
        start: f32,
    },
}

// Правка мешей (режим V): выделение, перетаскивание с клавиатуры, операции, панели.
#[path = "mesh_tools.rs"]
mod mesh_tools;
pub use mesh_tools::{LastKind, LastOp, MeshCmd, MeshEdit};

/// Куда попала кисть.
#[derive(Clone, Copy, Debug, PartialEq)]
struct PaintTarget {
    node: NodeId,
    face: FaceDir,
    texture: u32,
    x: i32,
    y: i32,
    clip: Clip,
}

struct StrokeState {
    tool: Tool,
    last: Vec<PaintTarget>,
    anchor: Vec<PaintTarget>,
    touched: HashSet<(u32, i32, i32)>,
    darken: bool,
}

/// Что рисовалось в прошлый раз: кадр перерисовывается, только если это изменилось.
#[derive(Clone, Debug, PartialEq)]
struct RenderKey {
    camera: Camera,
    size: [u32; 2],
    options: RenderOptions,
}

#[derive(Default)]
pub(super) struct Viewport {
    pub camera: Camera,
    pub rect: Option<Rect>,
    texture: Option<egui::TextureHandle>,
    frame: Frame,
    key: Option<RenderKey>,
    mesh: std::sync::Arc<BuiltMesh>,
    mesh_doc: Option<Document>,
    /// Растёт при каждой смене геометрии (не пикселей текстур).
    geometry_rev: u64,
    /// Кадр рисует хозяин на видеокарте в эту текстуру egui (`None` — растеризатор).
    pub gpu_texture: Option<egui::TextureId>,
    /// Запрос кадра для видеокарты, ещё не забранный хозяином.
    pub pending: Option<raster::ViewRequest>,
    drag: Option<Drag>,
    stroke: Option<StrokeState>,
    /// Режим правки вершин меша.
    pub mesh_edit: Option<MeshEdit>,
    /// Сообщение для строки состояния (почему щелчок ничего не сделал).
    pub notice: Option<String>,
    /// Закончилась операция с числами: (подпись, операция) — окно запомнит её для панели.
    pub pending_last: Option<(&'static str, LastKind)>,
    /// Модель изменилась после последнего кадра.
    frame_stale: bool,
    pub renders: u64,
    /// Время последней отрисовки кадра вида: растеризация и выгрузка в текстуру, мс.
    pub raster_ms: f32,
    pub upload_ms: f32,
}

impl Viewport {
    pub fn triangle_count(&self) -> usize {
        self.mesh.triangles.len()
    }

    pub fn world_to_screen(&self, point: Vec3) -> Option<Pos2> {
        let rect = self.rect?;
        let p = raster::project(&self.camera, Vec2::new(rect.width(), rect.height()), point)?;
        Some(rect.min + vec2(p.x, p.y))
    }

    /// Меш пересобирается, только когда меняется геометрия; штрих кисти меняет лишь
    /// пиксели текстуры — тогда кадр просто перерисовывается.
    fn sync_mesh(&mut self, doc: &Document) {
        let Some(old) = self.mesh_doc.as_ref() else {
            self.mesh = std::sync::Arc::new(build_mesh(doc, BuildOptions::default()));
            self.mesh_doc = Some(doc.clone());
            self.geometry_rev += 1;
            self.frame_stale = true;
            return;
        };
        if old == doc {
            return;
        }
        let sizes = |d: &Document| d.textures.iter().map(|t| t.image.size()).collect::<Vec<_>>();
        let geometry = old.nodes != doc.nodes || old.references != doc.references || sizes(old) != sizes(doc);
        if geometry {
            self.mesh = std::sync::Arc::new(build_mesh(doc, BuildOptions::default()));
            self.geometry_rev += 1;
        }
        self.mesh_doc = Some(doc.clone());
        self.frame_stale = true;
    }

    fn ray(&self, rect: Rect, pos: Pos2) -> Ray {
        let ndc = Vec2::new(
            (pos.x - rect.min.x) / rect.width() * 2.0 - 1.0,
            1.0 - (pos.y - rect.min.y) / rect.height() * 2.0,
        );
        self.camera.ray(ndc, rect.width() / rect.height().max(1.0))
    }

    /// Единиц модели в одной точке экрана рядом с `at`.
    fn units_per_point(&self, at: Vec3, rect: Rect) -> f32 {
        let tan = (self.camera.fov_deg.to_radians() * 0.5).tan();
        let depth = if self.camera.orthographic {
            self.camera.distance
        } else {
            (at - self.camera.eye()).dot(self.camera.forward_dir()).max(0.01)
        };
        2.0 * depth * tan / rect.height().max(1.0)
    }

    /// Нарисовать вид и обработать ввод. Возвращает вид, выбранный значком ориентации.
    pub fn show(&mut self, ui: &mut Ui, cx: &mut Context) -> Option<ViewPreset> {
        let rect = ui.max_rect();
        self.rect = Some(rect);
        let p = Palette::from_ctx(ui.ctx());
        let response = ui.interact(rect, egui::Id::new("modeler.viewport"), Sense::click_and_drag());
        self.sync_mesh(&cx.session.doc);

        let compass_center = pos2(rect.max.x - 52.0, rect.min.y + 52.0);
        let pointer = ui.input(|i| i.pointer.hover_pos()).filter(|pos| rect.contains(*pos));
        let over_compass = pointer.is_some_and(|pos| pos.distance(compass_center) < 46.0);

        self.camera_input(ui, &response, rect);

        let modifiers = ui.input(|i| i.modifiers);
        let mut hover_handle = None;
        if self.mesh_edit.as_ref().is_some_and(|m| cx.session.doc.element(m.id).is_none_or(|e| e.mesh.is_none())) {
            self.mesh_edit = None;
        }
        match cx.mode {
            Mode::Edit if self.mesh_edit.is_some() => {
                hover_handle = self.mesh_edit_input(ui, &response, rect, cx, over_compass, modifiers);
            }
            Mode::Edit => {
                hover_handle = self.edit_input(ui, &response, rect, cx, over_compass, modifiers);
            }
            Mode::Paint => {
                self.paint_input(ui, rect, cx, over_compass, modifiers);
            }
        }
        self.sync_mesh(&cx.session.doc);

        // Кадр вида: перерисовывается, только если изменились камера, модель, выделение,
        // настройки или размер.
        let ppp = ui.ctx().pixels_per_point();
        let full_size = [
            (rect.width() * ppp).round().max(1.0) as u32,
            (rect.height() * ppp).round().max(1.0) as u32,
        ];
        // Растеризатор во время перетаскивания рисует вдвое меньший кадр без сверхвыборки:
        // ввод не ждёт. Видеокарте это не нужно.
        let interacting = ui.input(|i| i.pointer.any_down() || i.smooth_scroll_delta != egui::Vec2::ZERO);
        let reduced = self.gpu_texture.is_none() && interacting;
        let size = if reduced { [(full_size[0] / 2).max(1), (full_size[1] / 2).max(1)] } else { full_size };
        let options = RenderOptions {
            shading: cx.view.shading,
            grid: cx.view.grid,
            grid_step: 1.0,
            major_every: cx.session.doc.units_per_metre.round().max(1.0) as u32,
            background: [style::from_color32(style::mix(p.bg_inset, p.bg_raised, 0.9)), style::from_color32(p.bg_inset)],
            highlight: if cx.mode == Mode::Edit { cx.session.selected_elements() } else { Vec::new() },
            highlight_color: style::from_color32(p.accent),
            supersample: if reduced || self.gpu_texture.is_some() { 1 } else { cx.view.supersample },
        };
        let key = RenderKey {
            camera: self.camera,
            size,
            options: options.clone(),
        };
        let stale = self.key.as_ref() != Some(&key) || self.frame_stale;
        match self.gpu_texture {
            Some(id) => {
                if stale {
                    self.pending = Some(raster::ViewRequest {
                        doc: cx.session.doc.clone(),
                        mesh: self.mesh.clone(),
                        geometry_rev: self.geometry_rev,
                        camera: self.camera,
                        options,
                        size,
                    });
                    self.renders += 1;
                    self.key = Some(key);
                    self.frame_stale = false;
                }
                ui.painter_at(rect).image(id, rect, Rect::from_min_max(pos2(0.0, 0.0), pos2(1.0, 1.0)), Color32::WHITE);
            }
            None => {
                if stale || self.texture.is_none() {
                    if self.frame.width != size[0] || self.frame.height != size[1] {
                        self.frame = Frame::new(size[0], size[1]);
                    }
                    let started = std::time::Instant::now();
                    raster::render(&cx.session.doc, &self.mesh, &self.camera, &options, &mut self.frame);
                    self.raster_ms = started.elapsed().as_secs_f32() * 1000.0;
                    let started = std::time::Instant::now();
                    self.renders += 1;
                    let image = egui::ColorImage {
                        size: [size[0] as usize, size[1] as usize],
                        pixels: self
                            .frame
                            .pixels
                            .iter()
                            .map(|c| Color32::from_rgba_premultiplied(c[0], c[1], c[2], 255))
                            .collect(),
                    };
                    match &mut self.texture {
                        Some(texture) => texture.set(image, egui::TextureOptions::NEAREST),
                        None => {
                            self.texture =
                                Some(ui.ctx().load_texture("modeler.viewport", image, egui::TextureOptions::NEAREST));
                        }
                    }
                    self.upload_ms = started.elapsed().as_secs_f32() * 1000.0;
                    self.key = Some(key);
                    self.frame_stale = false;
                }
                if let Some(texture) = &self.texture {
                    ui.painter_at(rect).image(texture.id(), rect, Rect::from_min_max(pos2(0.0, 0.0), pos2(1.0, 1.0)), Color32::WHITE);
                }
            }
        }
        let painter = ui.painter_at(rect);

        self.overlays(ui, rect, cx, pointer, hover_handle, &p);
        let preset = self.compass(ui, rect, compass_center, &p);

        // Подпись вида.
        let label = format!(
            "{} · {}",
            if self.camera.orthographic { tr_s("Orthographic") } else { tr_s("Perspective") },
            match cx.view.shading {
                raster::Shading::Textured => tr_s("Textured"),
                raster::Shading::Solid => tr_s("Solid"),
                raster::Shading::Wireframe => tr_s("Wireframe"),
            }
        );
        painter.text(rect.min + vec2(12.0, 10.0), egui::Align2::LEFT_TOP, label, style::small(), p.text_faint);
        if cx.session.doc.nodes.is_empty() && cx.session.doc.references.is_empty() {
            let c = rect.center() - vec2(0.0, 40.0);
            painter.text(c, egui::Align2::CENTER_CENTER, "Empty model", egui::FontId::proportional(17.0), p.text_muted);
            painter.text(
                c + vec2(0.0, 26.0),
                egui::Align2::CENTER_CENTER,
                "Add a cuboid from the toolbar, or open a .pmodel from File",
                style::body(),
                p.text_faint,
            );
            painter.text(
                c + vec2(0.0, 46.0),
                egui::Align2::CENTER_CENTER,
                "Right drag orbits · Middle drag pans · Wheel zooms · Tab switches to painting",
                style::small(),
                p.text_faint,
            );
        }
        preset
    }

    fn camera_input(&mut self, ui: &Ui, response: &egui::Response, rect: Rect) {
        let (scroll, alt, shift) = ui.input(|i| (i.smooth_scroll_delta.y, i.modifiers.alt, i.modifiers.shift));
        if response.hovered() && scroll != 0.0 {
            self.camera.zoom((-scroll * 0.0018).exp());
        }
        let delta = response.drag_delta();
        let orbit = (response.dragged_by(PointerButton::Secondary) && !shift)
            || (response.dragged_by(PointerButton::Primary) && alt);
        let pan = response.dragged_by(PointerButton::Middle) || (response.dragged_by(PointerButton::Secondary) && shift);
        if orbit {
            self.camera.orbit(-delta.x * 0.0085, delta.y * 0.0085);
        } else if pan {
            let h = rect.height().max(1.0);
            self.camera.pan(delta.x / h, delta.y / h);
        }
    }

    // ------------------------------------------------------------------
    // Правка формы
    // ------------------------------------------------------------------

    /// Ручки гизмо: (ручка, точки на экране для попадания и рисования, цвет, ось, начало).
    fn handles(&self, rect: Rect, cx: &Context) -> Vec<(Handle, Vec<Pos2>, Color32, Vec3, Vec3)> {
        let doc = &cx.session.doc;
        let session = &*cx.session;
        let mut out = Vec::new();
        if let Some(state) = &self.mesh_edit {
            return self.mesh_handles(rect, doc, state);
        }
        let Some(primary) = session.primary() else { return out };
        if doc.is_locked(primary) {
            return out;
        }
        let axes = [Vec3::X, Vec3::Y, Vec3::Z];
        let colors = AXIS;
        let screen = |p: Vec3| self.world_to_screen(p);
        match cx.tool {
            Tool::Move | Tool::Pivot => {
                let origin = if cx.tool == Tool::Move {
                    match doc.bounds(&session.selection) {
                        Some((min, max)) => (min + max) * 0.5,
                        None => pivot_world(doc, primary),
                    }
                } else {
                    pivot_world(doc, primary)
                };
                let length = 78.0 * self.units_per_point(origin, rect);
                for (i, axis) in axes.iter().enumerate() {
                    if let (Some(a), Some(b)) = (screen(origin), screen(origin + *axis * length)) {
                        out.push((Handle::Axis(i), vec![a, b], colors[i], *axis, origin));
                    }
                }
                if cx.tool == Tool::Move {
                    if let Some(c) = screen(origin) {
                        out.push((Handle::Plane, vec![c], Color32::WHITE, Vec3::ZERO, origin));
                    }
                }
            }
            Tool::Resize => {
                let Some(element) = doc.element(primary) else { return out };
                let matrix = doc.world_matrix(primary);
                for dir in element.shape.faces() {
                    let Some(center) = face_center(doc, primary, *dir) else { continue };
                    let normal = matrix.transform_vector3(dir.normal()).normalize_or_zero();
                    let stem = 22.0 * self.units_per_point(center, rect);
                    if let (Some(a), Some(b)) = (screen(center), screen(center + normal * stem)) {
                        out.push((Handle::Face(*dir), vec![a, b], colors[dir.axis()], normal, center));
                    }
                }
                // Плоскость — только верх и низ; у кубоида — все шесть.
                if !matches!(element.shape, crate::doc::Shape::Cuboid | crate::doc::Shape::Plane) {
                    for dir in [FaceDir::East, FaceDir::West, FaceDir::South] {
                        let Some(center) = face_center(doc, primary, dir) else { continue };
                        let normal = matrix.transform_vector3(dir.normal()).normalize_or_zero();
                        let stem = 22.0 * self.units_per_point(center, rect);
                        if let (Some(a), Some(b)) = (screen(center), screen(center + normal * stem)) {
                            out.push((Handle::Face(dir), vec![a, b], colors[dir.axis()], normal, center));
                        }
                    }
                }
            }
            Tool::Rotate => {
                let origin = pivot_world(doc, primary);
                let radius = 64.0 * self.units_per_point(origin, rect);
                for (i, axis) in axes.iter().enumerate() {
                    let (u, v) = basis(*axis);
                    let points: Vec<Pos2> = (0..=48)
                        .filter_map(|k| {
                            let a = std::f32::consts::TAU * k as f32 / 48.0;
                            screen(origin + (u * a.cos() + v * a.sin()) * radius)
                        })
                        .collect();
                    if points.len() > 40 {
                        out.push((Handle::Ring(i), points, colors[i], *axis, origin));
                    }
                }
            }
            _ => {}
        }
        out
    }

    fn handle_at(&self, handles: &[(Handle, Vec<Pos2>, Color32, Vec3, Vec3)], pos: Pos2) -> Option<usize> {
        let mut best: Option<(usize, f32)> = None;
        for (i, (handle, points, ..)) in handles.iter().enumerate() {
            let d = match handle {
                Handle::Plane => points[0].distance(pos) - 4.0,
                Handle::Face(_) => points[1].distance(pos).min(points[0].distance(pos) + 2.0) - 3.0,
                _ => points
                    .windows(2)
                    .map(|w| segment_distance(pos, w[0], w[1]))
                    .fold(f32::INFINITY, f32::min),
            };
            if d < 8.0 && best.is_none_or(|(_, bd)| d < bd) {
                best = Some((i, d));
            }
        }
        best.map(|(i, _)| i)
    }

    fn edit_input(
        &mut self,
        ui: &Ui,
        response: &egui::Response,
        rect: Rect,
        cx: &mut Context,
        over_compass: bool,
        modifiers: egui::Modifiers,
    ) -> Option<Handle> {
        let handles = self.handles(rect, cx);
        let pointer = ui.input(|i| i.pointer.hover_pos());
        let hover = if self.drag.is_none() { pointer.and_then(|pos| self.handle_at(&handles, pos)) } else { None };

        let press = ui.input(|i| i.pointer.press_origin());
        if response.drag_started_by(PointerButton::Primary) && !modifiers.alt && !over_compass {
            let start = press.unwrap_or_default();
            if let Some(i) = self.handle_at(&handles, start) {
                let (handle, _, _, axis, origin) = handles[i].clone();
                let ray = self.ray(rect, start);
                let start_value = match handle {
                    Handle::Axis(_) | Handle::Face(_) => axis_param(origin, axis, ray).unwrap_or(0.0),
                    Handle::Ring(_) => screen_angle(self.world_to_screen(origin).unwrap_or(start), start),
                    Handle::Plane => 0.0,
                };
                let start_point = if handle == Handle::Plane {
                    ray.plane(origin, -self.camera.forward_dir()).map_or(origin, |t| ray.at(t))
                } else {
                    origin
                };
                let ids = match handle {
                    Handle::Face(_) => cx.session.primary().into_iter().collect(),
                    _ if cx.tool == Tool::Pivot => cx.session.primary().into_iter().collect(),
                    _ => cx
                        .session
                        .selection
                        .iter()
                        .copied()
                        .filter(|id| !cx.session.doc.is_locked(*id))
                        .collect(),
                };
                let label = match cx.tool {
                    Tool::Move => "Move",
                    Tool::Resize => "Resize",
                    Tool::Rotate => "Rotate",
                    _ => "Move Pivot",
                };
                cx.session.begin(label);
                self.drag = Some(Drag::Gizmo {
                    handle,
                    origin,
                    axis,
                    start: start_value,
                    start_point,
                    ids,
                });
            } else {
                self.drag = Some(Drag::Box { start });
            }
        }

        match self.drag.clone() {
            Some(Drag::Gizmo {
                handle,
                origin,
                axis,
                start,
                start_point,
                ids,
            }) => {
                if response.dragged_by(PointerButton::Primary) {
                    if let (Some(pos), Some(base)) = (pointer, cx.session.pending_base().cloned()) {
                        let ray = self.ray(rect, pos);
                        let mut doc = base;
                        let snap = |v: f32| snap_value(v, cx.view.snap_step, cx.view.snapping, modifiers);
                        match handle {
                            Handle::Axis(i) => {
                                if let Some(t) = axis_param(origin, axis, ray) {
                                    let mut delta = Vec3::ZERO;
                                    delta[i] = snap(t - start);
                                    if cx.tool == Tool::Pivot {
                                        move_pivot(&mut doc, &ids, delta);
                                    } else {
                                        ops::move_by(&mut doc, &ids, delta);
                                    }
                                }
                            }
                            Handle::Plane => {
                                if let Some(t) = ray.plane(origin, -self.camera.forward_dir()) {
                                    let raw = ray.at(t) - start_point;
                                    let delta = Vec3::new(snap(raw.x), snap(raw.y), snap(raw.z));
                                    ops::move_by(&mut doc, &ids, delta);
                                }
                            }
                            Handle::Face(face) => {
                                if let Some(t) = axis_param(origin, axis, ray) {
                                    let amount = snap(t - start);
                                    for id in &ids {
                                        ops::resize_face(&mut doc, *id, face, amount);
                                    }
                                }
                            }
                            Handle::Ring(i) => {
                                let center = self.world_to_screen(origin).unwrap_or(pos);
                                let mut delta = (screen_angle(center, pos) - start).to_degrees();
                                delta = (delta + 540.0).rem_euclid(360.0) - 180.0;
                                // К камере обращена ось — поворот на экране против часовой.
                                let toward = axis.dot(self.camera.eye() - origin) > 0.0;
                                if toward {
                                    delta = -delta;
                                }
                                let step = if modifiers.shift { 1.0 } else { cx.view.rotation_step };
                                let snapping = (step > 0.0) != modifiers.command;
                                if snapping && step > 0.0 {
                                    delta = (delta / step).round() * step;
                                }
                                ops::rotate_by(&mut doc, &ids, i, delta);
                            }
                        }
                        cx.session.doc = doc;
                    }
                }
                if !ui.input(|i| i.pointer.primary_down()) {
                    cx.session.commit();
                    self.drag = None;
                }
            }
            Some(Drag::Box { start }) if !ui.input(|i| i.pointer.primary_down()) => {
                if let Some(end) = pointer {
                    let area = Rect::from_two_pos(start, end);
                    if area.width() > 3.0 || area.height() > 3.0 {
                        let doc = &cx.session.doc;
                        let picked: Vec<NodeId> = doc
                            .elements()
                            .filter(|(id, e)| {
                                doc.is_visible(*id)
                                    && !doc.is_locked(*id)
                                    && self
                                        .world_to_screen(doc.world_matrix(*id).transform_point3(e.center()))
                                        .is_some_and(|p| area.contains(p))
                            })
                            .map(|(id, _)| id)
                            .collect();
                        if modifiers.command || modifiers.shift {
                            for id in picked {
                                if !cx.session.is_selected(id) {
                                    cx.session.selection.push(id);
                                }
                            }
                        } else {
                            cx.session.selection = picked;
                        }
                    }
                }
                self.drag = None;
            }
            _ => {}
        }

        // Щелчок: выбор элемента.
        if response.clicked_by(PointerButton::Primary) && !over_compass && self.drag.is_none() {
            if let Some(pos) = response.interact_pointer_pos() {
                if self.handle_at(&handles, pos).is_none() {
                    let doc = &cx.session.doc;
                    let hit = raycast(&self.mesh, self.ray(rect, pos), |id| !doc.is_locked(id));
                    // Щелчок по элементу внутри выделенной группы оставляет группу выбранной
                    // только с Alt; иначе выбирается сам элемент.
                    match (hit, modifiers.command || modifiers.shift) {
                        (Some(hit), true) => cx.session.toggle_selected(hit.node),
                        (Some(hit), false) => {
                            cx.session.select(Some(hit.node));
                            cx.session.face = Some(hit.face);
                        }
                        (None, false) => cx.session.select(None),
                        (None, true) => {}
                    }
                }
            }
        }
        hover.map(|i| handles[i].0)
    }

    // ------------------------------------------------------------------
    // Рисование
    // ------------------------------------------------------------------

    fn target(&self, doc: &Document, ray: Ray) -> Option<PaintTarget> {
        let hit = raycast(&self.mesh, ray, |id| !doc.is_locked(id))?;
        let element = doc.element(hit.node)?;
        let clip = match &element.mesh {
            Some(mesh) => {
                if !mesh.has_uvs() {
                    return None;
                }
                // Грань меша: её прямоугольник UV.
                let part = self.mesh.triangles.get(hit.triangle).map_or(0, |t| t.part as usize);
                let face = mesh.faces.get(part)?;
                let (mut lo, mut hi) = ([f32::INFINITY; 2], [f32::NEG_INFINITY; 2]);
                for uv in &face.uvs {
                    for k in 0..2 {
                        lo[k] = lo[k].min(uv[k]);
                        hi[k] = hi[k].max(uv[k]);
                    }
                }
                [lo[0].floor() as i32, lo[1].floor() as i32, (hi[0].ceil() as i32).max(lo[0].floor() as i32 + 1), (hi[1].ceil() as i32).max(lo[1].floor() as i32 + 1)]
            }
            None => element.face(hit.face).uv.texel_bounds(),
        };
        let (texture, x, y) = hit.texel(doc)?;
        let [x0, y0, x1, y1] = clip;
        Some(PaintTarget {
            node: hit.node,
            face: hit.face,
            texture,
            x: x.clamp(x0, (x1 - 1).max(x0)),
            y: y.clamp(y0, (y1 - 1).max(y0)),
            clip,
        })
    }

    /// Цели кисти: попадание и (при зеркальном рисовании) его отражение по X.
    fn targets(&self, doc: &Document, ray: Ray, mirror: bool) -> Vec<PaintTarget> {
        let Some(hit) = raycast(&self.mesh, ray, |id| !doc.is_locked(id)) else { return Vec::new() };
        let mut out: Vec<PaintTarget> = self.target(doc, ray).into_iter().collect();
        if mirror {
            let p = Vec3::new(-hit.point.x, hit.point.y, hit.point.z);
            let n = Vec3::new(-hit.normal.x, hit.normal.y, hit.normal.z);
            let mirrored = Ray {
                origin: p + n * 0.05,
                dir: -n,
            };
            if let Some(t) = self.target(doc, mirrored) {
                if !out.contains(&t) {
                    out.push(t);
                }
            }
        }
        out
    }

    fn paint_input(&mut self, ui: &Ui, rect: Rect, cx: &mut Context, over_compass: bool, modifiers: egui::Modifiers) {
        let (pressed, down, pos) = ui.input(|i| (i.pointer.primary_pressed(), i.pointer.primary_down(), i.pointer.interact_pos()));
        let Some(pos) = pos else { return };
        if pressed && rect.contains(pos) && !over_compass && !modifiers.alt && self.stroke.is_none() {
            let targets = self.targets(&cx.session.doc, self.ray(rect, pos), cx.paint.mirror);
            if targets.is_empty() {
                let doc = &cx.session.doc;
                let hit = raycast(&self.mesh, self.ray(rect, pos), |_| true);
                self.notice = Some(match hit.and_then(|h| doc.node(h.node).map(|n| (n, h))) {
                    Some((node, _)) if doc.is_locked(node.id) => format!("`{}` is locked; unlock it in the outliner to paint", node.name),
                    Some((node, _)) if node.element().is_some_and(crate::mesh_edit::needs_uvs) => {
                        format!("`{}` has no UVs yet: Texture ▸ Generate UVs (mesh) makes it paintable", node.name)
                    }
                    Some(_) => "this face has no texture area to paint on".to_string(),
                    None => "Paint mode: the brush paints on the model; press Tab to select and move elements".to_string(),
                });
            }
            match cx.tool {
                Tool::Picker => {
                    if let Some(t) = targets.first() {
                        if let Some(c) = cx.session.doc.textures.get(t.texture as usize).and_then(|tex| tex.image.get(t.x, t.y)) {
                            cx.paint.color = c;
                            cx.paint.remember(c);
                        }
                    }
                }
                Tool::Fill => {
                    if !targets.is_empty() {
                        let color = cx.paint.color;
                        let mode = cx.paint.fill;
                        cx.session.edit("Fill", |doc| {
                            for t in &targets {
                                let Some(tex) = doc.textures.get_mut(t.texture as usize) else { continue };
                                match mode {
                                    super::FillMode::Face => paint::fill_clip(&mut tex.image, t.clip, color),
                                    super::FillMode::Connected => paint::flood_fill(&mut tex.image, t.x, t.y, color, Some(t.clip)),
                                };
                            }
                        });
                        cx.paint.remember(color);
                    }
                }
                tool => {
                    if !targets.is_empty() {
                        cx.session.begin(match tool {
                            Tool::Eraser => "Erase",
                            Tool::Line => "Line",
                            Tool::Rectangle => "Rectangle",
                            Tool::Shade => "Shade",
                            _ => "Paint",
                        });
                        self.stroke = Some(StrokeState {
                            tool,
                            last: Vec::new(),
                            anchor: targets.clone(),
                            touched: HashSet::new(),
                            darken: modifiers.shift,
                        });
                    }
                }
            }
        }
        if self.stroke.is_some() {
            if down {
                let targets = self.targets(&cx.session.doc, self.ray(rect, pos), cx.paint.mirror);
                self.apply_stroke(cx, &targets, modifiers);
            } else {
                let stroke = self.stroke.take().expect("checked");
                cx.session.commit();
                if stroke.tool != Tool::Eraser && stroke.tool != Tool::Shade {
                    cx.paint.remember(cx.paint.color);
                }
            }
        }
    }

    fn apply_stroke(&mut self, cx: &mut Context, targets: &[PaintTarget], modifiers: egui::Modifiers) {
        let Some(stroke) = self.stroke.as_mut() else { return };
        let color = if stroke.tool == Tool::Eraser { CLEAR } else { cx.paint.color };
        let size = cx.paint.size;
        let doc = &mut cx.session.doc;
        match stroke.tool {
            Tool::Pencil | Tool::Eraser => {
                for (i, t) in targets.iter().enumerate() {
                    let Some(tex) = doc.textures.get_mut(t.texture as usize) else { continue };
                    match stroke.last.get(i) {
                        Some(l) if l.node == t.node && l.face == t.face && l.texture == t.texture => {
                            paint::line(&mut tex.image, (l.x, l.y), (t.x, t.y), size, color, Some(t.clip));
                        }
                        _ => {
                            paint::dab(&mut tex.image, t.x, t.y, size, color, Some(t.clip));
                        }
                    }
                }
                stroke.last = targets.to_vec();
            }
            Tool::Shade => {
                let amount = if cx.paint.darken != (stroke.darken || modifiers.shift) { -cx.paint.shade } else { cx.paint.shade };
                for t in targets {
                    let Some(tex) = doc.textures.get_mut(t.texture as usize) else { continue };
                    for (x, y) in paint::brush_texels(t.x, t.y, size) {
                        let [x0, y0, x1, y1] = t.clip;
                        if x < x0 || y < y0 || x >= x1 || y >= y1 || !stroke.touched.insert((t.texture, x, y)) {
                            continue;
                        }
                        if let Some(c) = tex.image.get(x, y) {
                            if c[3] > 0 {
                                tex.image.set(x, y, paint::shade_color(c, amount));
                            }
                        }
                    }
                }
            }
            Tool::Line | Tool::Rectangle => {
                // Каждый кадр — заново от картинки начала штриха.
                let Some(base) = cx.session.pending_base().cloned() else { return };
                let doc = &mut cx.session.doc;
                for anchor in &stroke.anchor {
                    if let (Some(b), Some(tex)) = (base.textures.get(anchor.texture as usize), doc.textures.get_mut(anchor.texture as usize)) {
                        tex.image = b.image.clone();
                    }
                }
                for (anchor, t) in stroke.anchor.iter().zip(targets) {
                    if anchor.node != t.node || anchor.face != t.face {
                        continue;
                    }
                    let Some(tex) = doc.textures.get_mut(t.texture as usize) else { continue };
                    if stroke.tool == Tool::Line {
                        paint::line(&mut tex.image, (anchor.x, anchor.y), (t.x, t.y), size, color, Some(t.clip));
                    } else {
                        paint::rectangle(&mut tex.image, (anchor.x, anchor.y), (t.x, t.y), color, modifiers.shift != cx.paint.filled, Some(t.clip));
                    }
                }
            }
            _ => {}
        }
    }

    // ------------------------------------------------------------------
    // Поверх картинки
    // ------------------------------------------------------------------

    fn polygon_screen(&self, poly: &Polygon) -> Option<Vec<Pos2>> {
        poly.points.iter().map(|p| self.world_to_screen(*p)).collect()
    }

    fn outline(&self, painter: &egui::Painter, node: NodeId, stroke: Stroke) {
        for poly in self.mesh.polygons.iter().filter(|p| p.node == node) {
            if let Some(points) = self.polygon_screen(poly) {
                stroke_polygon(painter, &points, stroke);
            }
        }
    }

    #[allow(clippy::too_many_arguments)]
    fn overlays(&self, ui: &Ui, rect: Rect, cx: &Context, pointer: Option<Pos2>, hover_handle: Option<Handle>, p: &Palette) {
        let painter = ui.painter_at(rect);
        let doc = &cx.session.doc;

        if cx.view.shading == raster::Shading::Wireframe {
            let stroke = Stroke::new(1.0_f32, p.text_muted);
            for poly in &self.mesh.polygons {
                if let Some(points) = self.polygon_screen(poly) {
                    stroke_polygon(&painter, &points, stroke);
                }
            }
        }
        // Опорные меши в каркасе — их видно и сквозь модель.
        let selected = cx.session.selected_elements();
        if cx.mode == Mode::Edit {
            // Наведение.
            if self.drag.is_none() && hover_handle.is_none() {
                if let Some(pos) = pointer {
                    if let Some(hit) = raycast(&self.mesh, self.ray(rect, pos), |id| !doc.is_locked(id)) {
                        if !selected.contains(&hit.node) {
                            self.outline(&painter, hit.node, Stroke::new(1.0_f32, p.accent.gamma_multiply(0.55)));
                        }
                    }
                }
            }
            for id in &selected {
                self.outline(&painter, *id, Stroke::new(1.5_f32, p.accent));
                if let Some(twin) = doc.element(*id).and_then(|e| e.twin) {
                    if !selected.contains(&twin) {
                        self.outline(&painter, twin, Stroke::new(1.0_f32, p.accent.gamma_multiply(0.4)));
                    }
                }
            }
            // Точка вращения выбранной группы.
            for id in &cx.session.selection {
                if let Some(NodeKind::Group(_)) = doc.node(*id).map(|n| &n.kind) {
                    if let Some(c) = self.world_to_screen(pivot_world(doc, *id)) {
                        painter.circle_stroke(c, 5.0, Stroke::new(1.5_f32, p.accent));
                        painter.circle_filled(c, 2.0, p.accent);
                    }
                }
            }
            if cx.view.ruler {
                if let Some(id) = cx.session.primary() {
                    self.ruler(&painter, doc, id, p);
                }
            }
            if let Some(state) = &self.mesh_edit {
                self.mesh_overlay(&painter, doc, state, p);
            }
            self.draw_gizmo(&painter, rect, cx, hover_handle, p);
            if let Some(Drag::Box { start }) = &self.drag {
                if let Some(end) = pointer {
                    let area = Rect::from_two_pos(*start, end);
                    painter.rect_filled(area, 0.0, p.accent.gamma_multiply(0.08));
                    painter.rect_stroke(area, 0.0, Stroke::new(1.0_f32, p.accent));
                }
            }
        } else if let Some(pos) = pointer {
            // Кисть: обводка текселей под курсором.
            let targets = self.targets(doc, self.ray(rect, pos), cx.paint.mirror);
            for t in &targets {
                if let Some(poly) = self.mesh.polygons.iter().find(|q| q.node == t.node && q.face == t.face && q.uv_corners.is_some()) {
                    let size = cx.paint.size.max(1) as i32;
                    let start = -(size - 1) / 2;
                    let [x0, y0, x1, y1] = t.clip;
                    let (bx0, by0) = ((t.x + start).max(x0), (t.y + start).max(y0));
                    let (bx1, by1) = ((t.x + start + size).min(x1), (t.y + start + size).min(y1));
                    if let Some(quad) = texel_quad(poly, bx0 as f32, by0 as f32, bx1 as f32, by1 as f32) {
                        let points: Option<Vec<Pos2>> = quad.iter().map(|q| self.world_to_screen(*q)).collect();
                        if let Some(points) = points {
                            stroke_polygon(&painter, &points, Stroke::new(2.5_f32, Color32::from_black_alpha(140)));
                            stroke_polygon(&painter, &points, Stroke::new(1.2_f32, Color32::WHITE));
                        }
                    }
                } else {
                    painter.circle_stroke(pos, 4.0, Stroke::new(1.0_f32, Color32::WHITE));
                }
            }
        }
    }

    fn draw_gizmo(&self, painter: &egui::Painter, rect: Rect, cx: &Context, hover: Option<Handle>, p: &Palette) {
        let handles = self.handles(rect, cx);
        let active = match &self.drag {
            Some(Drag::Gizmo { handle, .. }) => Some(*handle),
            _ => None,
        };
        for (handle, points, color, _, _) in &handles {
            let lit = Some(*handle) == hover || Some(*handle) == active;
            let color = if lit { style::mix(*color, Color32::WHITE, 0.45) } else { *color };
            match handle {
                Handle::Axis(_) => {
                    let (a, b) = (points[0], points[1]);
                    painter.line_segment([a, b], Stroke::new(if lit { 3.0_f32 } else { 2.2 }, color));
                    let dir = (b - a).normalized();
                    let side = vec2(-dir.y, dir.x);
                    if cx.tool == Tool::Pivot {
                        painter.circle_filled(b, 5.0, color);
                    } else {
                        painter.add(EShape::convex_polygon(
                            vec![b + dir * 12.0, b + side * 5.5, b - side * 5.5],
                            color,
                            Stroke::NONE,
                        ));
                    }
                }
                Handle::Plane => {
                    let r = Rect::from_center_size(points[0], vec2(11.0, 11.0));
                    painter.rect_filled(r, 2.0, if lit { p.accent } else { Color32::from_white_alpha(70) });
                    painter.rect_stroke(r, 2.0, Stroke::new(1.0_f32, Color32::WHITE));
                }
                Handle::Face(_) => {
                    let (a, b) = (points[0], points[1]);
                    painter.line_segment([a, b], Stroke::new(1.5_f32, color));
                    let r = Rect::from_center_size(b, vec2(10.0, 10.0));
                    painter.rect_filled(r, 2.0, color);
                    painter.rect_stroke(r, 2.0, Stroke::new(1.0_f32, Color32::from_black_alpha(160)));
                }
                Handle::Ring(_) => {
                    painter.add(EShape::line(points.clone(), Stroke::new(if lit { 3.0_f32 } else { 2.0 }, color)));
                }
            }
        }
        // Подпись величины при перетаскивании.
        if let (Some(Drag::Gizmo { ids, .. }), Some(id)) = (&self.drag, cx.session.primary()) {
            let doc = &cx.session.doc;
            let text = match (cx.tool, doc.element(id)) {
                (Tool::Rotate, Some(e)) => format!("{}°, {}°, {}°", super::trim(e.rotation.x), super::trim(e.rotation.y), super::trim(e.rotation.z)),
                (Tool::Resize, Some(e)) => format!("{} × {} × {}", super::trim(e.size.x), super::trim(e.size.y), super::trim(e.size.z)),
                (_, Some(e)) => format!("{}, {}, {}", super::trim(e.from.x), super::trim(e.from.y), super::trim(e.from.z)),
                _ => trn!("{n} item", "{n} items", ids.len()),
            };
            if let Some(pos) = painter.ctx().input(|i| i.pointer.hover_pos()) {
                pill(painter, pos + vec2(16.0, 18.0), &text, p);
            }
        }
    }

    /// Линейка: размеры выделенного по трём осям вдоль ближних к камере рёбер.
    fn ruler(&self, painter: &egui::Painter, doc: &Document, id: NodeId, p: &Palette) {
        // Элемент — по его собственным (повёрнутым) рёбрам, группа — по общим границам.
        let corners: [Vec3; 8] = match doc.element_corners(id) {
            Some(c) => c,
            None => {
                let Some((min, max)) = doc.bounds(&[id]) else { return };
                std::array::from_fn(|i| {
                    Vec3::new(
                        if i & 1 == 0 { min.x } else { max.x },
                        if i & 2 == 0 { min.y } else { max.y },
                        if i & 4 == 0 { min.z } else { max.z },
                    )
                })
            }
        };
        let (min, max) = (corners[0], corners[7]);
        let eye = self.camera.eye();
        let corner = |x: bool, y: bool, z: bool| corners[x as usize | (y as usize) << 1 | (z as usize) << 2];
        let stroke = Stroke::new(1.0_f32, p.text_muted);
        for axis in 0..3 {
            // Из четырёх рёбер вдоль оси — ближайшее к камере (у вертикали — любое).
            let mut best: Option<(Vec3, Vec3, f32)> = None;
            for k in 0..4 {
                let (a, b) = (k & 1 == 1, k & 2 == 2);
                // Горизонтальные размеры — по нижним рёбрам, высота — по любому.
                let (p0, p1) = match axis {
                    0 if !a => (corner(false, false, b), corner(true, false, b)),
                    1 => (corner(a, false, b), corner(a, true, b)),
                    2 if !b => (corner(a, false, false), corner(a, false, true)),
                    _ => continue,
                };
                let d = ((p0 + p1) * 0.5 - eye).length();
                if best.is_none_or(|(_, _, bd)| d < bd) {
                    best = Some((p0, p1, d));
                }
            }
            let Some((p0, p1, _)) = best else { continue };
            let length = (p1 - p0).length();
            if length < 1e-3 {
                continue;
            }
            let (Some(a), Some(b)) = (self.world_to_screen(p0), self.world_to_screen(p1)) else { continue };
            if a.distance(b) < 24.0 {
                continue;
            }
            // Отодвинуть от модели в сторону экрана.
            let mid = pos2((a.x + b.x) * 0.5, (a.y + b.y) * 0.5);
            let dir = (b - a).normalized();
            let mut side = vec2(-dir.y, dir.x);
            let center = self.world_to_screen((min + max) * 0.5).unwrap_or(mid);
            if side.dot(mid - center) < 0.0 {
                side = -side;
            }
            let offset = side * 14.0;
            let (a, b) = (a + offset, b + offset);
            painter.line_segment([a, b], stroke);
            painter.line_segment([a - side * 4.0, a + side * 4.0], stroke);
            painter.line_segment([b - side * 4.0, b + side * 4.0], stroke);
            let label = format!("{} px", super::trim(length));
            let at = pos2((a.x + b.x) * 0.5, (a.y + b.y) * 0.5) + side * 10.0;
            pill(painter, at, &label, p);
        }
    }

    /// Значок ориентации: щелчок по концу оси ставит вид с этой стороны.
    fn compass(&self, ui: &Ui, rect: Rect, center: Pos2, p: &Palette) -> Option<ViewPreset> {
        let painter = ui.painter_at(rect);
        let radius = 34.0;
        let pointer = ui.input(|i| i.pointer.hover_pos());
        let near = pointer.is_some_and(|pos| pos.distance(center) < radius + 12.0);
        painter.circle_filled(center, radius + 10.0, if near { Color32::from_white_alpha(14) } else { Color32::from_white_alpha(6) });
        let view = self.camera.view();
        let mut ends: Vec<(f32, Pos2, usize, bool)> = Vec::new();
        for axis in 0..3 {
            for positive in [true, false] {
                let mut dir = Vec3::ZERO;
                dir[axis] = if positive { 1.0 } else { -1.0 };
                let v = view.transform_vector3(dir);
                let pos = center + vec2(v.x, -v.y) * radius;
                ends.push((v.z, pos, axis, positive));
            }
        }
        ends.sort_by(|a, b| a.0.total_cmp(&b.0));
        let hovered = pointer.and_then(|pos| {
            ends.iter()
                .rev()
                .find(|(_, end, _, _)| end.distance(pos) < 10.0)
                .map(|(_, end, axis, positive)| (*end, *axis, *positive))
        });
        for (_, pos, axis, positive) in &ends {
            let color = p.axis[*axis];
            let lit = hovered.is_some_and(|(e, ..)| e == *pos);
            if *positive {
                painter.line_segment([center, *pos], Stroke::new(2.0_f32, color));
                painter.circle_filled(*pos, if lit { 10.0 } else { 8.5 }, if lit { style::mix(color, Color32::WHITE, 0.35) } else { color });
                painter.text(*pos, egui::Align2::CENTER_CENTER, ["X", "Y", "Z"][*axis], egui::FontId::proportional(10.5), Color32::from_black_alpha(220));
            } else {
                painter.circle_filled(*pos, 6.5, style::mix(color, p.bg_inset, 0.55));
                painter.circle_stroke(*pos, 6.5, Stroke::new(if lit { 2.0_f32 } else { 1.2 }, color));
            }
        }
        let clicked = ui.input(|i| i.pointer.primary_clicked());
        let (_, axis, positive) = hovered?;
        if !clicked {
            return None;
        }
        Some(match (axis, positive) {
            (0, true) => ViewPreset::Right,
            (0, false) => ViewPreset::Left,
            (1, true) => ViewPreset::Top,
            (1, false) => ViewPreset::Bottom,
            (2, true) => ViewPreset::Back,
            _ => ViewPreset::Front,
        })
    }
}

/// Цвета осей X, Y, Z (как в редакторе сцены).
const AXIS: [Color32; 3] = [
    Color32::from_rgb(0xe8, 0x55, 0x55),
    Color32::from_rgb(0x74, 0xc9, 0x52),
    Color32::from_rgb(0x4f, 0x92, 0xec),
];

fn pill(painter: &egui::Painter, at: Pos2, text: &str, p: &Palette) {
    let galley = painter.layout_no_wrap(text.to_string(), style::small(), p.text);
    let rect = Rect::from_center_size(at, galley.size() + vec2(10.0, 4.0));
    painter.rect_filled(rect, 3.0, Color32::from_black_alpha(170));
    painter.galley(rect.min + vec2(5.0, 2.0), galley, p.text);
}

/// Мировая точка вращения узла (элемент — `pivot`, группа — `origin`).
pub(super) fn pivot_world(doc: &Document, id: NodeId) -> Vec3 {
    let parent = doc.parent_matrix(id);
    match doc.node(id).map(|n| &n.kind) {
        Some(NodeKind::Element(e)) => parent.transform_point3(e.pivot),
        Some(NodeKind::Group(g)) => parent.transform_point3(g.origin),
        None => Vec3::ZERO,
    }
}

/// Центр стороны элемента в мире.
pub(super) fn face_center(doc: &Document, id: NodeId, face: FaceDir) -> Option<Vec3> {
    let e = doc.element(id)?;
    let half = e.size * 0.5 + Vec3::splat(e.inflate);
    let local = e.center() + face.normal() * half[face.axis()];
    Some(doc.world_matrix(id).transform_point3(local))
}

/// Сдвинуть точку вращения узлов на `delta` (мир), не трогая форму.
fn move_pivot(doc: &mut Document, ids: &[NodeId], delta: Vec3) {
    for id in ids {
        let local = doc.parent_matrix(*id).inverse().transform_vector3(delta);
        match doc.node(*id).map(|n| n.kind.clone()) {
            Some(NodeKind::Element(mut e)) => {
                e.pivot += local;
                ops::set_element(doc, *id, e);
            }
            Some(NodeKind::Group(mut g)) => {
                g.origin += local;
                ops::set_group(doc, *id, g);
            }
            None => {}
        }
    }
}

/// Параметр точки на прямой `origin + t·axis`, ближайшей к лучу.
fn axis_param(origin: Vec3, axis: Vec3, ray: Ray) -> Option<f32> {
    let w0 = origin - ray.origin;
    let a = axis.dot(axis);
    let b = axis.dot(ray.dir);
    let c = ray.dir.dot(ray.dir);
    let d = axis.dot(w0);
    let e = ray.dir.dot(w0);
    let denom = a * c - b * b;
    if denom.abs() < 1e-6 {
        return None; // ось смотрит прямо в камеру
    }
    Some((b * e - c * d) / denom)
}

fn screen_angle(center: Pos2, pos: Pos2) -> f32 {
    (pos.y - center.y).atan2(pos.x - center.x)
}

/// Два перпендикуляра к оси.
fn basis(axis: Vec3) -> (Vec3, Vec3) {
    let u = if axis.x.abs() > 0.9 { Vec3::Y } else { Vec3::X };
    let v = axis.cross(u).normalize();
    let u = v.cross(axis).normalize();
    (u, v)
}

fn segment_distance(p: Pos2, a: Pos2, b: Pos2) -> f32 {
    let ab = b - a;
    let t = ((p - a).dot(ab) / ab.length_sq().max(1e-6)).clamp(0.0, 1.0);
    p.distance(a + ab * t)
}

/// Привязка: с включённой привязкой (или Ctrl при выключенной) — к шагу, Shift — к четверти.
pub(super) fn snap_value(v: f32, step: f32, snapping: bool, modifiers: egui::Modifiers) -> f32 {
    let on = snapping != modifiers.command;
    if !on {
        return v;
    }
    let step = if modifiers.shift { step * 0.25 } else { step };
    (v / step).round() * step
}

/// Углы прямоугольника текселей `[x0, x1) × [y0, y1)` на стороне в мире.
fn texel_quad(poly: &Polygon, x0: f32, y0: f32, x1: f32, y1: f32) -> Option<[Vec3; 4]> {
    let uv = poly.uv_corners?;
    let (tl, tr, bl) = (Vec2::from(uv[0]), Vec2::from(uv[1]), Vec2::from(uv[3]));
    let (du, dv) = (tr - tl, bl - tl);
    let det = du.perp_dot(dv);
    if det.abs() < 1e-9 {
        return None;
    }
    let p = &poly.points;
    let map = |x: f32, y: f32| {
        let q = Vec2::new(x, y) - tl;
        let s = q.perp_dot(dv) / det;
        let t = du.perp_dot(q) / det;
        p[0] + (p[1] - p[0]) * s + (p[3] - p[0]) * t
    };
    Some([map(x0, y0), map(x1, y0), map(x1, y1), map(x0, y1)])
}

/// Контур многоугольника отдельными отрезками. `closed_line` соединяет рёбра «усами»
/// (miter): у острых углов и у совпадающих точек (вершина конуса и пирамиды) они
/// выстреливают далеко за фигуру.
pub(crate) fn stroke_polygon(painter: &egui::Painter, points: &[Pos2], stroke: Stroke) {
    for (i, a) in points.iter().enumerate() {
        let b = points[(i + 1) % points.len()];
        if a.distance(b) > 0.25 {
            painter.line_segment([*a, b], stroke);
        }
    }
}
