//! Редактор UV для мешей: панель слева от вида (View ▸ UV Editor, клавиша U).
//!
//! Показывает текстуру модели (тексель в тексель, колесо — масштаб, средняя кнопка — сдвиг,
//! сетка текселей при крупном масштабе) и поверх неё UV всех граней меша. Грани, выбранные
//! в 3D-виде, подсвечены здесь, а выбранное здесь подсвечивается в 3D (выбор общий).
//!
//! Выбор: щелчок (углы / грани / острова — по режиму 1 / 2 / 3), Shift — добавить или
//! снять, рамка, A — всё, Alt+A — ничего, Ctrl+I — обратить, L — остров под курсором.
//! Правка: G — сдвиг, R — поворот (Ctrl — по 90°), S — масштаб; X / Y — ось, цифры — число,
//! Enter или щелчок — принять, Esc или правая кнопка — отменить; перетаскивание выбранного
//! тоже сдвигает. Прижатие к текселям включено по умолчанию. Каждая правка — один шаг истории.
//! Развёртки и укладка — меню UV (чистые функции живут в [`crate::uv_ops`]).

use std::collections::BTreeSet;

use egui::{pos2, vec2, Color32, Key, Modifiers, PointerButton, Pos2, Rect, Sense, Shape as EShape, Stroke, Ui};
use engine_i18n::{n_, tr, trf};
use glam::{Vec2 as V2, Vec3};

use super::style::{self, Palette};
use super::viewport::MeshEdit;
use super::{controls, kit, Command, Mode, ModelerApp};
use crate::doc::{MeshShape, NodeId};
use crate::image::Image;
use crate::mesh_ops::EditMesh;
use crate::session::Session;
use crate::uv_ops::{self, Corner, Method, Pack};

/// Что выбирает щелчок.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Pick {
    Vertex,
    Face,
    Island,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Op {
    Move,
    Rotate,
    Scale,
}

impl Op {
    fn label(self) -> &'static str {
        match self {
            Op::Move => n_("Move UVs"),
            Op::Rotate => n_("Rotate UVs"),
            Op::Scale => n_("Scale UVs"),
        }
    }
}

/// Правка с клавиатуры или мышью в процессе (документ пересчитывается от снимка).
#[derive(Clone, Debug)]
struct Modal {
    op: Op,
    id: NodeId,
    corners: BTreeSet<Corner>,
    axis: Option<usize>,
    typed: String,
    start: Pos2,
    pivot: V2,
    /// Начата перетаскиванием: заканчивается отпусканием кнопки.
    drag: bool,
    confirm: bool,
    cancel: bool,
    readout: String,
}

#[derive(Clone, Copy, Debug, PartialEq)]
enum Action {
    Pick(Pick),
    SelectAll,
    SelectNone,
    Invert,
    Linked,
    Start(Op),
}

/// Команды меню UV (и тесты).
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum UvCmd {
    /// Каждая грань — свой остров в настоящую величину.
    ByFace,
    /// Проекция кубом.
    Box,
    /// Плоская проекция вдоль оси (0 — X, 1 — Y, 2 — Z).
    Axis(usize),
    /// Плоская проекция из вида камеры.
    View,
    /// Уложить острова с зазором.
    Pack,
    /// Одна плотность текселей (пикселей на метр из панели).
    Density,
    /// Выбранные грани — в свой остров.
    Split,
    /// Склеить совпадающие вершины UV.
    Weld,
    /// Прижать углы к текселям.
    Snap,
    /// Повернуть выбранное на четверть оборота (+1 по часовой, −1 против).
    Rotate90(i32),
    /// Отразить: `false` — по горизонтали, `true` — по вертикали.
    Flip(bool),
}

pub(super) struct UvView {
    pub open: bool,
    zoom: f32,
    pan: egui::Vec2,
    handle: Option<(egui::TextureHandle, Image)>,
    sel: BTreeSet<Corner>,
    /// Грани, которые выбор в 3D-виде показывал в прошлый раз (чужая правка выбора сбрасывает наш).
    synced: BTreeSet<usize>,
    pick: Pick,
    snap: bool,
    pack: Pack,
    /// Пикселей на метр для развёрток (0 — как у модели).
    px_per_metre: f32,
    modal: Option<Modal>,
    box_from: Option<Pos2>,
    actions: Vec<Action>,
    canvas: Option<Rect>,
    image_rect: Rect,
    scale: f32,
    hovered: bool,
    /// Нажатие мыши принято модальной правкой: его отпускание — не щелчок.
    swallow: bool,
    /// Кнопки меню Select и UV в прошлом кадре (тесты и снимки нажимают по ним).
    menus: [Rect; 2],
}

impl UvView {
    /// Панель закрыта: ни холста, ни правки.
    pub fn close(&mut self) {
        self.hovered = false;
        self.canvas = None;
        self.modal = None;
        self.box_from = None;
    }
}

impl Default for UvView {
    fn default() -> Self {
        Self {
            open: false,
            zoom: 0.0,
            pan: egui::Vec2::ZERO,
            handle: None,
            sel: BTreeSet::new(),
            synced: BTreeSet::new(),
            pick: Pick::Face,
            snap: true,
            pack: Pack::default(),
            px_per_metre: 0.0,
            modal: None,
            box_from: None,
            actions: Vec::new(),
            canvas: None,
            image_rect: Rect::NOTHING,
            scale: 1.0,
            hovered: false,
            swallow: false,
            menus: [Rect::NOTHING; 2],
        }
    }
}

// ----------------------------------------------------------------------
// Геометрия в UV
// ----------------------------------------------------------------------

fn face_poly(shape: &MeshShape, f: usize) -> Vec<V2> {
    shape.faces[f].uvs.iter().map(|p| V2::from(*p)).collect()
}

fn in_poly(poly: &[V2], p: V2) -> bool {
    let mut inside = false;
    for i in 0..poly.len() {
        let (a, b) = (poly[i], poly[(i + 1) % poly.len()]);
        if (a.y > p.y) != (b.y > p.y) && p.x < a.x + (p.y - a.y) / (b.y - a.y) * (b.x - a.x) {
            inside = !inside;
        }
    }
    inside
}

fn poly_area(poly: &[V2]) -> f32 {
    let mut a = 0.0;
    for i in 0..poly.len() {
        let (p, q) = (poly[i], poly[(i + 1) % poly.len()]);
        a += p.x * q.y - q.x * p.y;
    }
    a.abs() * 0.5
}

/// Грань под точкой (меньшая — сверху, если наложены).
fn pick_face(shape: &MeshShape, texture: u32, p: V2) -> Option<usize> {
    let mut best: Option<(usize, f32)> = None;
    for (f, face) in shape.faces.iter().enumerate() {
        if face.texture != texture || face.uvs.len() < 3 {
            continue;
        }
        let poly = face_poly(shape, f);
        if in_poly(&poly, p) {
            let area = poly_area(&poly);
            if best.is_none_or(|(_, a)| area < a) {
                best = Some((f, area));
            }
        }
    }
    best.map(|b| b.0)
}

/// Ближайший угол в пределах `radius` текселей и все углы, стоящие в той же точке.
fn pick_corners(shape: &MeshShape, texture: u32, p: V2, radius: f32) -> BTreeSet<Corner> {
    let mut best: Option<(Corner, V2, f32)> = None;
    for (f, face) in shape.faces.iter().enumerate() {
        if face.texture != texture {
            continue;
        }
        for (k, uv) in face.uvs.iter().enumerate() {
            let d = V2::from(*uv).distance(p);
            if d <= radius && best.is_none_or(|b| d < b.2) {
                best = Some(((f, k), V2::from(*uv), d));
            }
        }
    }
    let mut out = BTreeSet::new();
    if let Some((_, at, _)) = best {
        for (f, face) in shape.faces.iter().enumerate() {
            if face.texture != texture {
                continue;
            }
            for (k, uv) in face.uvs.iter().enumerate() {
                if V2::from(*uv).distance(at) < 1e-3 {
                    out.insert((f, k));
                }
            }
        }
    }
    out
}

fn select_at(shape: &MeshShape, texture: u32, pick: Pick, p: V2, radius: f32) -> BTreeSet<Corner> {
    match pick {
        Pick::Vertex => pick_corners(shape, texture, p, radius),
        Pick::Face => pick_face(shape, texture, p).map(|f| uv_ops::corners_of_faces(shape, &BTreeSet::from([f]))).unwrap_or_default(),
        Pick::Island => pick_face(shape, texture, p)
            .map(|f| uv_ops::corners_of_faces(shape, &uv_ops::island_of(shape, f).into_iter().collect()))
            .unwrap_or_default(),
    }
}

fn select_in_box(shape: &MeshShape, texture: u32, pick: Pick, lo: V2, hi: V2) -> BTreeSet<Corner> {
    let inside = |p: V2| p.x >= lo.x && p.x <= hi.x && p.y >= lo.y && p.y <= hi.y;
    let mut out = BTreeSet::new();
    match pick {
        Pick::Vertex => {
            for (f, face) in shape.faces.iter().enumerate() {
                if face.texture == texture {
                    for (k, uv) in face.uvs.iter().enumerate() {
                        if inside(V2::from(*uv)) {
                            out.insert((f, k));
                        }
                    }
                }
            }
        }
        Pick::Face | Pick::Island => {
            let mut faces = BTreeSet::new();
            for (f, face) in shape.faces.iter().enumerate() {
                if face.texture == texture && !face.uvs.is_empty() {
                    let c = face.uvs.iter().fold(V2::ZERO, |s, uv| s + V2::from(*uv)) / face.uvs.len() as f32;
                    if inside(c) {
                        faces.insert(f);
                    }
                }
            }
            if pick == Pick::Island {
                let all = uv_ops::islands(shape, None);
                let mut grown = BTreeSet::new();
                for island in all.iter().filter(|i| i.iter().any(|f| faces.contains(f))) {
                    grown.extend(island.iter().copied());
                }
                faces = grown;
            }
            out = uv_ops::corners_of_faces(shape, &faces);
        }
    }
    out
}

impl MeshEdit {
    /// Выбрать грани целиком (выбор в UV-редакторе → 3D-вид).
    pub(in crate::ui) fn set_faces_from_uv(&mut self, m: &EditMesh, faces: BTreeSet<usize>) {
        let sel = crate::mesh_ops::select_faces(m, faces);
        self.vertices = sel.verts.into_iter().collect();
        self.edges = sel.edges.into_iter().collect();
        self.faces = sel.faces.into_iter().collect();
        self.mode = crate::mesh_ops::SelectMode::Face;
        if self.face.is_some_and(|f| !self.faces.contains(&f)) {
            self.face = None;
        }
    }
}

// ----------------------------------------------------------------------
// Окно приложения
// ----------------------------------------------------------------------

impl ModelerApp {
    /// Открыта ли панель UV-редактора.
    pub fn uv_editor_open(&self) -> bool {
        self.uv_view.open
    }

    /// Выбранные углы UV: (грань, угол).
    pub fn uv_selection(&self) -> Vec<(usize, usize)> {
        self.uv_view.sel.iter().copied().collect()
    }

    /// Холст редактора UV в прошлом кадре.
    pub fn uv_canvas(&self) -> Option<Rect> {
        self.uv_view.canvas
    }

    /// Точка UV (в текселях) на экране в прошлом кадре.
    pub fn uv_to_screen(&self, uv: [f32; 2]) -> Option<Pos2> {
        self.uv_view.canvas?;
        Some(self.uv_view.image_rect.min + vec2(uv[0], uv[1]) * self.uv_view.scale)
    }

    /// Кнопки меню «Select» и «UV» панели в прошлом кадре.
    pub fn uv_menu_buttons(&self) -> [Rect; 2] {
        self.uv_view.menus
    }

    pub fn uv_snap(&self) -> bool {
        self.uv_view.snap
    }

    pub fn set_uv_snap(&mut self, on: bool) {
        self.uv_view.snap = on;
    }

    /// Настройки укладки (зазор, повороты, прижатие размеров).
    pub fn uv_pack_mut(&mut self) -> &mut Pack {
        &mut self.uv_view.pack
    }

    /// Пикселей на метр для развёрток.
    pub fn set_uv_pixels_per_metre(&mut self, px: f32) {
        self.uv_view.px_per_metre = px;
    }

    /// Идёт ли правка UV (G / R / S).
    pub fn uv_edit_in_progress(&self) -> bool {
        self.uv_view.modal.is_some()
    }

    fn uv_target(&self) -> Option<NodeId> {
        self.selected_mesh().or_else(|| self.viewport.mesh_edit.as_ref().map(|m| m.id).filter(|id| self.document().element(*id).is_some_and(|e| e.mesh.is_some())))
    }

    fn uv_highlight(&self, id: NodeId) -> BTreeSet<usize> {
        self.viewport.mesh_edit.as_ref().filter(|m| m.id == id).map(|m| m.faces.iter().copied().collect()).unwrap_or_default()
    }

    /// Углы, над которыми работают правки: выбранные здесь, иначе — грани, выбранные в 3D.
    fn uv_working_corners(&self, id: NodeId) -> BTreeSet<Corner> {
        let Some(shape) = self.document().element(id).and_then(|e| e.mesh.as_ref()) else { return BTreeSet::new() };
        let live: BTreeSet<Corner> = self.uv_view.sel.iter().copied().filter(|c| uv_ops::uv_of(shape, *c).is_some()).collect();
        if !live.is_empty() {
            return live;
        }
        uv_ops::corners_of_faces(shape, &self.uv_highlight(id))
    }

    /// Грани, над которыми работают операции (пусто — над всеми).
    fn uv_working_faces(&self, id: NodeId) -> BTreeSet<usize> {
        uv_ops::faces_of(&self.uv_working_corners(id))
    }

    /// Клавиши редактора UV, пока курсор над ним или идёт правка. `true` — клавиши режима
    /// меша 3D-вида в этом кадре не нужны.
    pub(in crate::ui) fn uv_shortcuts(&mut self, ctx: &egui::Context) -> bool {
        if !self.uv_view.open || self.mode != Mode::Edit {
            return false;
        }
        if let Some(m) = self.uv_view.modal.as_mut() {
            ctx.input_mut(|i| {
                for (k, key) in [Key::X, Key::Y].into_iter().enumerate() {
                    if i.consume_key(Modifiers::NONE, key) {
                        m.axis = if m.axis == Some(k) { None } else { Some(k) };
                    }
                }
                for event in &i.events {
                    if let egui::Event::Text(text) = event {
                        for c in text.chars() {
                            if c.is_ascii_digit() || c == '.' || (c == '-' && m.typed.is_empty()) {
                                m.typed.push(c);
                            }
                        }
                    }
                }
                if i.consume_key(Modifiers::NONE, Key::Backspace) {
                    m.typed.pop();
                }
                if i.consume_key(Modifiers::NONE, Key::Enter) {
                    m.confirm = true;
                }
                if i.consume_key(Modifiers::NONE, Key::Escape) {
                    m.cancel = true;
                }
            });
            return true;
        }
        if !self.uv_view.hovered {
            return false;
        }
        let mut actions = Vec::new();
        let has_sel = !self.uv_view.sel.is_empty();
        ctx.input_mut(|i| {
            let none = Modifiers::NONE;
            for (key, pick) in [(Key::Num1, Pick::Vertex), (Key::Num2, Pick::Face), (Key::Num3, Pick::Island)] {
                if i.consume_key(none, key) {
                    actions.push(Action::Pick(pick));
                }
            }
            if i.consume_key(Modifiers::ALT, Key::A) {
                actions.push(Action::SelectNone);
            } else if i.consume_key(none, Key::A) {
                actions.push(Action::SelectAll);
            }
            if i.consume_key(Modifiers::COMMAND, Key::I) {
                actions.push(Action::Invert);
            }
            if i.consume_key(none, Key::L) {
                actions.push(Action::Linked);
            }
            for (key, op) in [(Key::G, Op::Move), (Key::R, Op::Rotate), (Key::S, Op::Scale)] {
                if i.consume_key(none, key) {
                    actions.push(Action::Start(op));
                }
            }
            if has_sel && i.consume_key(none, Key::Escape) {
                actions.push(Action::SelectNone);
            }
        });
        self.uv_view.actions.extend(actions);
        true
    }

    /// Выбор в 3D-виде по граням (после выбора в UV-редакторе).
    fn uv_push_to_3d(&mut self, id: NodeId) {
        let faces = uv_ops::faces_of(&self.uv_view.sel);
        let Some(mesh) = self.document().element(id).and_then(EditMesh::of) else { return };
        if self.viewport.mesh_edit.as_ref().map(|m| m.id) != Some(id) {
            self.mode = Mode::Edit;
            self.viewport.mesh_edit = Some(MeshEdit::new(id));
        }
        if let Some(edit) = self.viewport.mesh_edit.as_mut() {
            edit.set_faces_from_uv(&mesh, faces.clone());
        }
        self.uv_view.synced = faces;
    }

    /// Команда меню UV.
    pub(in crate::ui) fn run_uv(&mut self, cmd: UvCmd) {
        let Some(id) = self.uv_target() else {
            self.error(tr("select a mesh element first").to_string());
            return;
        };
        let faces = self.uv_working_faces(id);
        let corners = self.uv_working_corners(id);
        let pack = self.uv_view.pack;
        let snap = self.uv_view.snap;
        let units = self.document().units_per_metre;
        let ppm = if self.uv_view.px_per_metre > 0.0 { self.uv_view.px_per_metre } else { self.document().texel_density * units };
        let density = uv_ops::texels_per_unit(ppm, units);
        let result: Result<String, String> = match cmd {
            UvCmd::ByFace | UvCmd::Box | UvCmd::Axis(_) | UvCmd::View => {
                let method = match cmd {
                    UvCmd::ByFace => Method::ByFace,
                    UvCmd::Box => Method::Box,
                    UvCmd::Axis(a) => Method::Axis(a),
                    _ => {
                        let forward = self.viewport.camera.forward_dir();
                        let right = forward.cross(Vec3::Y).normalize_or_zero();
                        let up = right.cross(forward);
                        let inv = self.document().world_matrix(id).inverse();
                        Method::View { right: inv.transform_vector3(right), up: inv.transform_vector3(up) }
                    }
                };
                let label = match cmd {
                    UvCmd::ByFace => n_("Unwrap by Face"),
                    UvCmd::Box => n_("Box Projection"),
                    UvCmd::Axis(0) => n_("Project along X"),
                    UvCmd::Axis(1) => n_("Project along Y"),
                    UvCmd::Axis(_) => n_("Project along Z"),
                    _ => n_("Project from View"),
                };
                let mut out = Err(String::new());
                self.session_mut().edit(label, |doc| out = uv_ops::unwrap(doc, id, &faces, method, density, pack));
                out.map(|n| trf!("{label}: {n} face(s) laid out at {px} px per metre", label = tr(label), n = n, px = trim_number(ppm)))
            }
            UvCmd::Pack => {
                let mut out = Err(String::new());
                self.session_mut().edit("Pack Islands", |doc| out = uv_ops::pack_islands(doc, id, &faces, pack));
                out.map(|n| trf!("Packed the islands of {n} face(s)", n = n))
            }
            UvCmd::Density => {
                let mut out = Err(String::new());
                self.session_mut().edit("Texel Density", |doc| out = uv_ops::scale_to_density(doc, id, &faces, density, pack));
                out.map(|n| trf!("{n} face(s) at {px} px per metre", n = n, px = trim_number(ppm)))
            }
            UvCmd::Split => {
                let mut out = Err(String::new());
                self.session_mut().edit("Split to Island", |doc| out = uv_ops::split_to_island(doc, id, &faces, pack));
                out.map(|n| trf!("{n} face(s) moved into their own island", n = n))
            }
            UvCmd::Weld => {
                let mut n = 0;
                self.session_mut().edit("Weld UV Vertices", |doc| n = uv_ops::weld(doc, id, &faces, 0.5));
                Ok(trf!("Welded {n} UV vertex(es)", n = n))
            }
            UvCmd::Snap => {
                if corners.is_empty() {
                    Err(tr("select faces or UV points first").to_string())
                } else {
                    self.session_mut().edit("Snap UVs to Texels", |doc| {
                        uv_ops::edit_shape(doc, id, |s| uv_ops::snap_corners(s, &corners));
                    });
                    Ok(tr("UV points snapped to texel corners").to_string())
                }
            }
            UvCmd::Rotate90(q) => {
                if corners.is_empty() {
                    Err(tr("select faces or UV points first").to_string())
                } else {
                    self.session_mut().edit("Rotate UVs", |doc| {
                        uv_ops::edit_shape(doc, id, |s| uv_ops::rotate_quarters(s, &corners, q, snap));
                    });
                    Ok(String::new())
                }
            }
            UvCmd::Flip(vertical) => {
                if corners.is_empty() {
                    Err(tr("select faces or UV points first").to_string())
                } else {
                    self.session_mut().edit("Flip UVs", |doc| {
                        uv_ops::edit_shape(doc, id, |s| uv_ops::flip_corners(s, &corners, vertical));
                    });
                    Ok(String::new())
                }
            }
        };
        match result {
            Ok(text) if text.is_empty() => {}
            Ok(text) => self.message(text),
            Err(e) => self.error(e),
        }
    }

    /// Содержимое панели UV-редактора.
    pub(in crate::ui) fn uv_panel(&mut self, ui: &mut Ui) {
        let p = Palette::from_ctx(ui.ctx());
        let mut cmd: Option<UvCmd> = None;
        let mut pushed = false;
        let target = self.uv_target();
        let mut hint: Option<String> = None;
        egui::Frame::none().inner_margin(egui::Margin::same(kit::PANEL_PAD)).show(ui, |ui| {
            ui.spacing_mut().item_spacing = vec2(kit::ITEM_GAP, kit::ITEM_GAP);
            // Строка 1: меню и подгонка.
            let mut fit = false;
            let editing = self.mode == Mode::Edit;
            controls::row(ui, |ui| {
                let select = controls::menu_button(ui, tr("Select"), 0.0, |ui| {
                    let mut act = None;
                    for (label, key, a) in [
                        (n_("All"), "A", Action::SelectAll),
                        (n_("None"), "Alt+A", Action::SelectNone),
                        (n_("Invert"), "Ctrl+I", Action::Invert),
                        (n_("Linked"), "L", Action::Linked),
                        (n_("Vertices"), "1", Action::Pick(Pick::Vertex)),
                        (n_("Faces"), "2", Action::Pick(Pick::Face)),
                        (n_("Islands"), "3", Action::Pick(Pick::Island)),
                    ] {
                        if controls::menu_item(ui, &tr(label), key, target.is_some() && editing).clicked() {
                            act = Some(a);
                        }
                    }
                    if let Some(a) = act {
                        self.uv_view.actions.push(a);
                    }
                });
                self.uv_view.menus[0] = select.response.rect;
                let uv_menu = controls::menu_button(ui, tr("UV"), 0.0, |ui| {
                    let on = target.is_some() && editing;
                    ui.set_min_width(kit::MENU_WIDTH);
                    ui.menu_button(tr("Transform"), |ui| {
                        for (label, key, op) in [(n_("Move"), "G", Op::Move), (n_("Rotate"), "R", Op::Rotate), (n_("Scale"), "S", Op::Scale)] {
                            if controls::menu_item(ui, &tr(label), key, on).clicked() {
                                self.uv_view.actions.push(Action::Start(op));
                            }
                        }
                    });
                    ui.menu_button(tr("Unwrap"), |ui| {
                        for (label, c) in [
                            (n_("Unwrap by Face"), UvCmd::ByFace),
                            (n_("Box Projection"), UvCmd::Box),
                            (n_("Project from View"), UvCmd::View),
                            (n_("Project along X"), UvCmd::Axis(0)),
                            (n_("Project along Y"), UvCmd::Axis(1)),
                            (n_("Project along Z"), UvCmd::Axis(2)),
                        ] {
                            if controls::menu_item(ui, &tr(label), "", on).clicked() {
                                cmd = Some(c);
                            }
                        }
                    });
                    for (label, c) in [
                        (n_("Pack Islands"), UvCmd::Pack),
                        (n_("Scale to Texel Density"), UvCmd::Density),
                        (n_("Split to Island"), UvCmd::Split),
                        (n_("Weld UV Vertices"), UvCmd::Weld),
                        (n_("Snap to Texels"), UvCmd::Snap),
                    ] {
                        if controls::menu_item(ui, &tr(label), "", on).clicked() {
                            cmd = Some(c);
                        }
                    }
                    ui.separator();
                    for (label, c) in [
                        (n_("Rotate 90° Clockwise"), UvCmd::Rotate90(1)),
                        (n_("Rotate 90° Counterclockwise"), UvCmd::Rotate90(-1)),
                        (n_("Flip Horizontally"), UvCmd::Flip(false)),
                        (n_("Flip Vertically"), UvCmd::Flip(true)),
                    ] {
                        if controls::menu_item(ui, &tr(label), "", on).clicked() {
                            cmd = Some(c);
                        }
                    }
                });
                self.uv_view.menus[1] = uv_menu.response.rect;
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    if controls::button(ui, &tr("Fit")).on_hover_text(tr("Fit the texture in the view (double-click the empty view)")).clicked() {
                        fit = true;
                    }
                });
            });
            if fit {
                self.uv_view.zoom = 0.0;
                self.uv_view.pan = egui::Vec2::ZERO;
            }
            // Строка 2: что выбирает щелчок и прижатие.
            controls::row(ui, |ui| {
                let options = [
                    (Pick::Vertex, None, tr("Vertex"), tr("Click selects UV points (1)")),
                    (Pick::Face, None, tr("Face"), tr("Click selects a face (2)")),
                    (Pick::Island, None, tr("Island"), tr("Click selects a whole island (3)")),
                ];
                let options: Vec<(Pick, Option<super::Icon>, &str, &str)> = options.iter().map(|(a, b, c, d)| (*a, *b, c.as_ref(), d.as_ref())).collect();
                if let Some(pick) = style::segmented(ui, "modeler.uv.pick", self.uv_view.pick, &options) {
                    self.uv_view.pick = pick;
                }
                let snap = controls::toggle_button(ui, self.uv_view.snap, &tr("Snap")).on_hover_text(tr("Snap UV points to texel corners (pixel art)"));
                if snap.clicked() {
                    self.uv_view.snap = !self.uv_view.snap;
                }
            });
            let Some(id) = target else {
                hint = Some(tr("Select a mesh element to edit its UVs").to_string());
                return;
            };
            self.uv_canvas_and_footer(ui, id, &p, &mut pushed);
        });
        if let Some(text) = hint {
            self.uv_view.canvas = None;
            let rect = ui.max_rect();
            ui.painter().text(rect.center(), egui::Align2::CENTER_CENTER, text, egui::FontId::proportional(kit::FONT_BODY), p.text_faint);
        }
        if let Some(id) = target.filter(|_| pushed) {
            self.uv_push_to_3d(id);
        }
        if let Some(cmd) = cmd {
            self.run(Command::Uv(cmd));
        }
    }

    /// Холст, обработка ввода и нижняя полоса настроек.
    fn uv_canvas_and_footer(&mut self, ui: &mut Ui, id: NodeId, p: &Palette, pushed: &mut bool) {
        let (shape, units, ppm_default, tex_count) = {
            let doc = &self.tabs[self.active].session.doc;
            let Some(shape) = doc.element(id).and_then(|e| e.mesh.clone()) else { return };
            (shape, doc.units_per_metre, doc.texel_density * doc.units_per_metre, doc.textures.len())
        };
        let highlight = self.uv_highlight(id);
        // 3D-вид поменял выбор не мы — наш выбор углов следует за ним.
        if highlight != self.uv_view.synced && self.uv_view.modal.is_none() {
            self.uv_view.sel = uv_ops::corners_of_faces(&shape, &highlight);
            self.uv_view.synced = highlight.clone();
        }
        self.uv_view.sel.retain(|c| uv_ops::uv_of(&shape, *c).is_some());
        let texture = highlight.iter().next().and_then(|f| shape.faces.get(*f)).or(shape.faces.first()).map_or(0, |f| f.texture);
        let texture = if (texture as usize) < tex_count { texture } else { 0 };
        let Some(image) = self.tabs[self.active].session.doc.textures.get(texture as usize).map(|t| t.image.clone()) else {
            style::caption(ui, &tr("The model has no texture yet"));
            return;
        };
        let (tw, th) = (image.width().max(1), image.height().max(1));
        // Картинка для egui: перезаливается, только когда пиксели изменились.
        let stale = self.uv_view.handle.as_ref().is_none_or(|(_, cached)| !cached.shares_pixels(&image) || cached.size() != image.size());
        if stale {
            let pixels = image.pixels().iter().map(|c| Color32::from_rgba_unmultiplied(c[0], c[1], c[2], c[3])).collect();
            let color = egui::ColorImage { size: [tw as usize, th as usize], pixels };
            match &mut self.uv_view.handle {
                Some((handle, cached)) => {
                    handle.set(color, egui::TextureOptions::NEAREST);
                    *cached = image.clone();
                }
                None => {
                    let handle = ui.ctx().load_texture("modeler.uv_view", color, egui::TextureOptions::NEAREST);
                    self.uv_view.handle = Some((handle, image.clone()));
                }
            }
        }

        // Высота холста: всё, что осталось, без двух нижних строк.
        let footer = kit::CONTROL_H * 2.0 + kit::ITEM_GAP * 2.0;
        let height = (ui.available_height() - footer).max(120.0);
        let (outer, _) = ui.allocate_exact_size(vec2(ui.available_width(), height), Sense::hover());
        let response = ui.interact(outer, egui::Id::new("modeler.uv_view.canvas"), Sense::click_and_drag());
        self.uv_view.canvas = Some(outer);
        self.uv_view.hovered = response.hovered();
        let fit = ((outer.width() - 2.0 * kit::SP_8) / tw as f32).min((outer.height() - 2.0 * kit::SP_8) / th as f32).max(0.05);
        let mut zoom = if self.uv_view.zoom <= 0.0 { fit } else { self.uv_view.zoom };
        if response.hovered() {
            let scroll = ui.input(|i| i.smooth_scroll_delta.y);
            if scroll != 0.0 {
                let new_zoom = (zoom * (scroll * 0.002).exp()).clamp(fit * 0.25, 96.0);
                if let Some(pos) = response.hover_pos() {
                    let origin = outer.center() + self.uv_view.pan;
                    let rel = (pos - origin) / zoom;
                    self.uv_view.pan += rel * zoom - rel * new_zoom;
                }
                self.uv_view.zoom = new_zoom;
                zoom = new_zoom;
            }
        }
        if response.dragged_by(PointerButton::Middle) {
            self.uv_view.pan += response.drag_delta();
        }
        if response.double_clicked_by(PointerButton::Middle) {
            self.uv_view.zoom = 0.0;
            self.uv_view.pan = egui::Vec2::ZERO;
            zoom = fit;
        }
        let ppp = ui.ctx().pixels_per_point();
        let size = vec2(tw as f32, th as f32) * zoom;
        let centre = outer.center() + self.uv_view.pan;
        let snap_px = |v: f32| (v * ppp).round() / ppp;
        let image_rect = Rect::from_min_size(pos2(snap_px(centre.x - size.x * 0.5), snap_px(centre.y - size.y * 0.5)), size);
        self.uv_view.image_rect = image_rect;
        self.uv_view.scale = zoom;
        let to_screen = |u: V2| image_rect.min + vec2(u.x, u.y) * zoom;
        let to_uv = |pos: Pos2| {
            let v = (pos - image_rect.min) / zoom;
            V2::new(v.x, v.y)
        };
        let pointer = ui.input(|i| i.pointer.hover_pos());
        let (shift, ctrl) = ui.input(|i| (i.modifiers.shift, i.modifiers.command));

        // --- Отложенные действия (клавиши и меню) ---
        let mut selection_changed = false;
        let actions: Vec<Action> = std::mem::take(&mut self.uv_view.actions);
        for action in actions {
            match action {
                Action::Pick(pick) => self.uv_view.pick = pick,
                Action::SelectAll => {
                    self.uv_view.sel = uv_ops::corners_of_faces(&shape, &(0..shape.faces.len()).filter(|f| shape.faces[*f].texture == texture).collect());
                    selection_changed = true;
                }
                Action::SelectNone => {
                    self.uv_view.sel.clear();
                    selection_changed = true;
                }
                Action::Invert => {
                    let all = uv_ops::corners_of_faces(&shape, &(0..shape.faces.len()).filter(|f| shape.faces[*f].texture == texture).collect());
                    self.uv_view.sel = all.difference(&self.uv_view.sel).copied().collect();
                    selection_changed = true;
                }
                Action::Linked => {
                    let faces = uv_ops::faces_of(&self.uv_view.sel);
                    let under = pointer.filter(|p| outer.contains(*p)).and_then(|p| pick_face(&shape, texture, to_uv(p)));
                    let seeds: BTreeSet<usize> = if faces.is_empty() { under.into_iter().collect() } else { faces };
                    let mut grown = BTreeSet::new();
                    for island in uv_ops::islands(&shape, None).iter().filter(|i| i.iter().any(|f| seeds.contains(f))) {
                        grown.extend(island.iter().copied());
                    }
                    self.uv_view.sel = uv_ops::corners_of_faces(&shape, &grown);
                    selection_changed = true;
                }
                Action::Start(op) => {
                    let corners = self.uv_working_corners_of(&shape, &highlight);
                    if corners.is_empty() {
                        self.error(tr("select faces or UV points first").to_string());
                    } else if self.uv_view.modal.is_none() {
                        let pivot = uv_ops::center(&shape, &corners).unwrap_or(V2::ZERO);
                        let start = pointer.unwrap_or(outer.center());
                        self.tabs[self.active].session.begin(op.label());
                        self.uv_view.modal = Some(Modal { op, id, corners, axis: None, typed: String::new(), start, pivot, drag: false, confirm: false, cancel: false, readout: String::new() });
                        self.uv_view.swallow = false;
                    }
                }
            }
        }

        // --- Правка в процессе ---
        if let Some(mut m) = self.uv_view.modal.take() {
            let (pressed, secondary, down) = ui.input(|i| (i.pointer.primary_pressed(), i.pointer.secondary_pressed(), i.pointer.primary_down()));
            if !m.drag && pressed {
                m.confirm = true;
                self.uv_view.swallow = true;
            }
            if secondary {
                m.cancel = true;
            }
            let session = &mut self.tabs[self.active].session;
            if m.cancel {
                session.cancel();
            } else {
                let pos = pointer.unwrap_or(m.start);
                let pivot_screen = to_screen(m.pivot);
                apply_modal(&mut m, session, pos, pivot_screen, zoom, self.uv_view.snap, ctrl);
                if m.confirm || (m.drag && !down) {
                    session.commit();
                } else {
                    self.uv_view.modal = Some(m);
                }
            }
        }

        // --- Мышь: выбор, рамка, перетаскивание ---
        // В режиме Paint панель только показывает: рисуют в панели UV справа или по модели.
        let editing = self.mode == Mode::Edit;
        let modal_active = self.uv_view.modal.is_some();
        let swallowed = self.uv_view.swallow || !editing;
        if swallowed && !ui.input(|i| i.pointer.primary_down()) {
            self.uv_view.swallow = false;
        }
        let radius = 9.0 / zoom;
        if !modal_active && !swallowed {
            if response.drag_started_by(PointerButton::Primary) {
                if let Some(start) = ui.input(|i| i.pointer.press_origin()) {
                    let hit = select_at(&shape, texture, self.uv_view.pick, to_uv(start), radius);
                    let on_selected = !hit.is_empty() && hit.iter().all(|c| self.uv_view.sel.contains(c));
                    if on_selected {
                        // Перетаскивание выбранного — сдвиг.
                        let corners = self.uv_view.sel.clone();
                        let pivot = uv_ops::center(&shape, &corners).unwrap_or(V2::ZERO);
                        self.tabs[self.active].session.begin(Op::Move.label());
                        self.uv_view.modal = Some(Modal { op: Op::Move, id, corners, axis: None, typed: String::new(), start, pivot, drag: true, confirm: false, cancel: false, readout: String::new() });
                    } else {
                        self.uv_view.box_from = Some(start);
                    }
                }
            }
            if let Some(from) = self.uv_view.box_from {
                if let Some(now) = pointer {
                    let area = Rect::from_two_pos(from, now);
                    ui.painter().rect_filled(area, 0.0, p.accent.gamma_multiply(0.15));
                    ui.painter().rect_stroke(area, 0.0, Stroke::new(1.0_f32, p.accent));
                }
                if response.drag_stopped() {
                    if let Some(now) = ui.input(|i| i.pointer.interact_pos()).or(pointer) {
                        let (a, b) = (to_uv(from), to_uv(now));
                        let found = select_in_box(&shape, texture, self.uv_view.pick, a.min(b), a.max(b));
                        if shift {
                            self.uv_view.sel.extend(found);
                        } else {
                            self.uv_view.sel = found;
                        }
                        selection_changed = true;
                    }
                    self.uv_view.box_from = None;
                }
            }
            if response.clicked_by(PointerButton::Primary) {
                if let Some(pos) = response.interact_pointer_pos() {
                    let hit = select_at(&shape, texture, self.uv_view.pick, to_uv(pos), radius);
                    if shift {
                        if !hit.is_empty() && hit.iter().all(|c| self.uv_view.sel.contains(c)) {
                            for c in &hit {
                                self.uv_view.sel.remove(c);
                            }
                        } else {
                            self.uv_view.sel.extend(hit);
                        }
                    } else {
                        self.uv_view.sel = hit;
                    }
                    selection_changed = true;
                }
            }
            if response.double_clicked_by(PointerButton::Primary) && select_at(&shape, texture, Pick::Face, to_uv(response.interact_pointer_pos().unwrap_or(outer.center())), radius).is_empty() {
                self.uv_view.zoom = 0.0;
                self.uv_view.pan = egui::Vec2::ZERO;
            }
        }
        if selection_changed {
            *pushed = true;
        }

        // --- Рисование ---
        let shape_now = self.tabs[self.active].session.doc.element(id).and_then(|e| e.mesh.clone()).unwrap_or(shape);
        let painter = ui.painter_at(outer);
        painter.rect_filled(outer, 0.0, p.bg_inset);
        style::checker(&painter, image_rect.intersect(outer), (zoom * 2.0).clamp(4.0, 12.0));
        if let Some((handle, _)) = &self.uv_view.handle {
            painter.image(handle.id(), image_rect, Rect::from_min_max(pos2(0.0, 0.0), pos2(1.0, 1.0)), Color32::WHITE);
        }
        painter.rect_stroke(image_rect, 0.0, Stroke::new(1.0_f32, p.line_strong));
        // Сетка текселей: каждый тексель при крупном масштабе, каждые 8 — при среднем.
        if zoom >= 2.5 {
            let (every, alpha) = if zoom >= 6.0 { (1, 38) } else { (8, 30) };
            let line = |n: u32, horizontal: bool| {
                let major = n % 8 == 0;
                let c = Color32::from_black_alpha(if major { alpha + 30 } else { alpha });
                let (a, b) = if horizontal {
                    let y = image_rect.min.y + n as f32 * zoom;
                    (pos2(image_rect.min.x.max(outer.min.x), y), pos2(image_rect.max.x.min(outer.max.x), y))
                } else {
                    let x = image_rect.min.x + n as f32 * zoom;
                    (pos2(x, image_rect.min.y.max(outer.min.y)), pos2(x, image_rect.max.y.min(outer.max.y)))
                };
                let inside = if horizontal { a.y > outer.min.y && a.y < outer.max.y } else { a.x > outer.min.x && a.x < outer.max.x };
                if inside {
                    painter.line_segment([a, b], Stroke::new(1.0_f32, c));
                }
            };
            for n in (1..tw).filter(|n| n % every == 0) {
                line(n, false);
            }
            for n in (1..th).filter(|n| n % every == 0) {
                line(n, true);
            }
        }
        // Грани: выбранные в 3D — с заливкой акцентом.
        let sel_faces = uv_ops::faces_of(&self.uv_view.sel);
        let vertex_mode = self.uv_view.pick == Pick::Vertex;
        for (f, face) in shape_now.faces.iter().enumerate() {
            if face.texture != texture || face.uvs.len() < 2 {
                continue;
            }
            let pts: Vec<Pos2> = face.uvs.iter().map(|uv| to_screen(V2::from(*uv))).collect();
            if !outer.intersects(Rect::from_points(&pts)) {
                continue;
            }
            let lit = highlight.contains(&f) || sel_faces.contains(&f);
            if lit && pts.len() >= 3 {
                painter.add(EShape::convex_polygon(pts.clone(), p.accent.gamma_multiply(0.38), Stroke::NONE));
            }
            let stroke = if lit { Stroke::new(2.0_f32, Color32::from_rgb(0xff, 0xb0, 0x30)) } else { Stroke::new(1.0_f32, Color32::from_white_alpha(85)) };
            painter.add(EShape::closed_line(pts.clone(), stroke));
            if vertex_mode || lit {
                for (k, pt) in pts.iter().enumerate() {
                    if self.uv_view.sel.contains(&(f, k)) {
                        painter.circle(*pt, 4.0, Color32::from_rgb(0xff, 0xb0, 0x30), Stroke::new(1.0_f32, Color32::BLACK));
                    } else if vertex_mode {
                        painter.circle_filled(*pt, 2.5, Color32::from_white_alpha(210));
                    }
                }
            }
        }
        if let Some(m) = &self.uv_view.modal {
            let c = to_screen(m.pivot);
            painter.circle_stroke(c, 5.0, Stroke::new(1.5_f32, Color32::from_rgb(0xff, 0xb0, 0x30)));
            let text = format!("{}  {}", tr(m.op.label()), m.readout);
            let font = egui::FontId::proportional(kit::FONT_CAPTION);
            let galley = painter.layout_no_wrap(text, font, p.text);
            let at = outer.min + vec2(kit::SP_4, kit::SP_4);
            painter.rect_filled(Rect::from_min_size(at, galley.size() + vec2(2.0 * kit::SP_3, 2.0 * kit::SP_2)), kit::R_S, p.bg_panel);
            painter.galley(at + vec2(kit::SP_3, kit::SP_2), galley, p.text);
        } else if let Some(pos) = pointer.filter(|q| outer.contains(*q)) {
            let uv = to_uv(pos);
            let text = format!("U {:.1}  V {:.1}", uv.x, uv.y);
            let galley = painter.layout_no_wrap(text, egui::FontId::proportional(kit::FONT_CAPTION), p.text_muted);
            let at = outer.right_bottom() - galley.size() - vec2(kit::SP_4 + kit::SP_3, kit::SP_4 + kit::SP_2);
            painter.rect_filled(Rect::from_min_size(at - vec2(kit::SP_3, kit::SP_2), galley.size() + vec2(2.0 * kit::SP_3, 2.0 * kit::SP_2)), kit::R_S, p.bg_panel);
            painter.galley(at, galley, p.text_muted);
        }

        // --- Нижние строки: сведения и настройки ---
        let (points, faces_now) = {
            let doc = &self.tabs[self.active].session.doc;
            (doc.element(id).map(|e| e.mesh_points()).unwrap_or_default(), if sel_faces.is_empty() { highlight.iter().copied().collect::<Vec<_>>() } else { sel_faces.iter().copied().collect() })
        };
        let density = if faces_now.is_empty() { uv_ops::density_of(&points, &shape_now, &(0..shape_now.faces.len()).collect::<Vec<_>>()) } else { uv_ops::density_of(&points, &shape_now, &faces_now) };
        let info = trf!(
            "{w}×{h} px · {faces} selected · {px} px/m",
            w = tw,
            h = th,
            faces = faces_now.len(),
            px = trim_number(density * units)
        );
        let info = if editing { info } else { tr("Switch to Edit (Tab) to edit UVs").to_string() };
        controls::row(ui, |ui| style::caption(ui, &info));
        controls::row(ui, |ui| {
            let mut margin = self.uv_view.pack.margin as f32;
            if controls::number_field_ex(ui, &mut margin, 0.0..=16.0, 0.1, &tr("Margin "), "", 0, 84.0).on_hover_text(tr("Gap between islands when packing, in texels")).changed() {
                self.uv_view.pack.margin = margin.round().max(0.0) as u32;
            }
            let mut rotate = self.uv_view.pack.rotate;
            if controls::check_with_id(ui, egui::Id::new("modeler.uv.rotate_islands"), rotate, &tr("Rotate"), true, false).on_hover_text(tr("Packing may turn islands by 90°")).clicked() {
                rotate = !rotate;
                self.uv_view.pack.rotate = rotate;
            }
            let mut ppm = if self.uv_view.px_per_metre > 0.0 { self.uv_view.px_per_metre } else { ppm_default };
            if controls::number_field_ex(ui, &mut ppm, 1.0..=512.0, 0.5, "", &tr(" px/m"), 0, 84.0).on_hover_text(tr("Texel density for unwrapping and Scale to Texel Density, pixels per metre (the engine uses 16)")).changed() {
                self.uv_view.px_per_metre = ppm;
            }
        });
    }

    /// Углы для правки по уже снятой копии меша и подсветке (без обращения к документу).
    fn uv_working_corners_of(&self, shape: &MeshShape, highlight: &BTreeSet<usize>) -> BTreeSet<Corner> {
        let live: BTreeSet<Corner> = self.uv_view.sel.iter().copied().filter(|c| uv_ops::uv_of(shape, *c).is_some()).collect();
        if live.is_empty() {
            uv_ops::corners_of_faces(shape, highlight)
        } else {
            live
        }
    }
}

fn trim_number(v: f32) -> String {
    if (v - v.round()).abs() < 0.05 {
        format!("{}", v.round() as i64)
    } else {
        format!("{v:.1}")
    }
}

/// Пересчитать документ от снимка по текущему состоянию правки.
fn apply_modal(m: &mut Modal, session: &mut Session, pos: Pos2, pivot_screen: Pos2, zoom: f32, snap: bool, ctrl: bool) {
    let Some(base) = session.pending_base().cloned() else { return };
    let typed = m.typed.parse::<f32>().ok();
    let mut doc = base;
    let (corners, id, pivot) = (m.corners.clone(), m.id, m.pivot);
    match m.op {
        Op::Move => {
            let mut d = V2::new((pos.x - m.start.x) / zoom, (pos.y - m.start.y) / zoom);
            if let Some(t) = typed {
                d = if m.axis == Some(1) { V2::new(0.0, t) } else { V2::new(t, 0.0) };
            } else {
                match m.axis {
                    Some(0) => d.y = 0.0,
                    Some(1) => d.x = 0.0,
                    _ => {}
                }
            }
            let mut done = V2::ZERO;
            uv_ops::edit_shape(&mut doc, id, |s| done = uv_ops::move_corners(s, &corners, d, snap && typed.is_none()));
            m.readout = format!("X {:.2}  Y {:.2}", done.x, done.y);
        }
        Op::Rotate => {
            let angle = match typed {
                Some(deg) => deg.to_radians(),
                None => {
                    let a0 = (m.start - pivot_screen).angle();
                    let a1 = (pos - pivot_screen).angle();
                    let a = a1 - a0;
                    if ctrl {
                        (a / std::f32::consts::FRAC_PI_2).round() * std::f32::consts::FRAC_PI_2
                    } else {
                        a
                    }
                }
            };
            let quarters = angle / std::f32::consts::FRAC_PI_2;
            uv_ops::edit_shape(&mut doc, id, |s| {
                if (quarters - quarters.round()).abs() < 1e-4 {
                    uv_ops::rotate_quarters(s, &corners, quarters.round() as i32, snap);
                } else {
                    uv_ops::rotate_corners(s, &corners, pivot, angle, snap);
                }
            });
            m.readout = format!("{:.1}°", angle.to_degrees());
        }
        Op::Scale => {
            let factor = typed.unwrap_or_else(|| {
                let (d0, d1) = ((m.start - pivot_screen).length().max(4.0), (pos - pivot_screen).length());
                d1 / d0
            });
            let factor = factor.clamp(0.01, 100.0);
            let s = match m.axis {
                Some(0) => V2::new(factor, 1.0),
                Some(1) => V2::new(1.0, factor),
                _ => V2::splat(factor),
            };
            uv_ops::edit_shape(&mut doc, id, |sh| uv_ops::scale_corners(sh, &corners, pivot, s, snap));
            m.readout = format!("×{factor:.2}");
        }
    }
    session.doc = doc;
}
