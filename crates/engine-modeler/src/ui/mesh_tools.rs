//! Моделирование мешей в окне: режим вершин / рёбер / граней (V; 1 / 2 / 3), выделение
//! (щелчок, Shift — добавить, Ctrl — убрать, рамка, Alt+щелчок — петля, A, Alt+A, Ctrl+I,
//! L, Ctrl+L, Ctrl+= / Ctrl+−), перенос / поворот / масштаб с клавиатуры (G / R / S, оси
//! X / Y / Z, ввод числа, Ctrl — привязка), операции (E выдавить, I вставка, Ctrl+B фаска,
//! Ctrl+R петлевой разрез, F грань, M слить, X удалить, Shift+D копия, P отделить, Ctrl+T
//! треугольники, Alt+J квады, Shift+N нормали наружу, Alt+N развернуть), меню Mesh,
//! добавление примитивов (Shift+A), панель последней операции с её числами и подсказка
//! внизу вида — что ждёт текущий инструмент.
//!
//! Топология — в [`crate::mesh_ops`]; здесь — только ввод, отрисовка и шаги истории.

use std::collections::BTreeSet;

use egui::{pos2, vec2, Color32, Key, Modifiers, PointerButton, Pos2, Rect, Shape as EShape, Stroke, Ui};
use engine_i18n::{n_, tr, trf};
use glam::{Quat, Vec2, Vec3};

use super::{axis_param, raycast, snap_value, Context, Drag, Handle, Palette, Viewport, AXIS};
use crate::doc::{Document, NodeId, Shape};
use crate::mesh_ops::{self as mo, Delete, Edge, EditMesh, Primitive, SelectMode};
use crate::ui::{controls, kit, Mode, ModelerApp};

/// Состояние режима правки меша.
#[derive(Clone, Debug, PartialEq)]
pub struct MeshEdit {
    pub id: NodeId,
    /// Выбранные вершины (у граней и рёбер — их вершины).
    pub vertices: Vec<u32>,
    /// Активная грань (последняя выбранная щелчком).
    pub face: Option<usize>,
    pub mode: SelectMode,
    pub edges: Vec<Edge>,
    pub faces: Vec<usize>,
    /// Ребро под курсором (для Ctrl+R и Alt+щелчка).
    pub hover_edge: Option<Edge>,
    /// Идёт перенос / поворот / масштаб / выдавливание с клавиатуры.
    pub modal: Option<Modal>,
    /// Перетаскивание завершено нажатием мыши: её отпускание — не щелчок выбора.
    pub swallow_release: bool,
    /// Нож (K) включён; первая точка разреза: (ребро, доля от меньшей вершины).
    pub knife: Option<Option<(Edge, f32)>>,
    /// Режим петлевого разреза (Ctrl+R без ребра под курсором): (число разрезов, доля).
    pub loop_cut: Option<(usize, f32)>,
}

impl MeshEdit {
    pub fn new(id: NodeId) -> Self {
        Self { id, vertices: Vec::new(), face: None, mode: SelectMode::Vertex, edges: Vec::new(), faces: Vec::new(), hover_edge: None, modal: None, swallow_release: false, knife: None, loop_cut: None }
    }

    /// Выделение по вершинам (рёбра и грани между ними).
    fn set_verts(&mut self, m: &EditMesh, verts: BTreeSet<u32>) {
        let sel = mo::select_verts(m, verts);
        self.apply(sel);
    }

    fn apply(&mut self, sel: mo::Selection) {
        self.vertices = sel.verts.into_iter().collect();
        self.edges = sel.edges.into_iter().collect();
        self.faces = sel.faces.into_iter().collect();
        if self.face.is_some_and(|f| !self.faces.contains(&f)) {
            self.face = None;
        }
    }

    fn verts(&self) -> BTreeSet<u32> {
        self.vertices.iter().copied().collect()
    }

    fn face_set(&self) -> BTreeSet<usize> {
        self.faces.iter().copied().collect()
    }

    fn edge_set(&self) -> BTreeSet<Edge> {
        self.edges.iter().copied().collect()
    }

    fn clear(&mut self) {
        self.vertices.clear();
        self.edges.clear();
        self.faces.clear();
        self.face = None;
    }
}

/// Что делает перетаскивание мышью после клавиши.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ModalKind {
    Grab,
    Rotate,
    Scale,
    /// Выдавленные грани (или рёбра) едут вдоль нормали.
    Extrude,
    Inset,
    Bevel,
}

impl ModalKind {
    fn label(self) -> &'static str {
        match self {
            ModalKind::Grab => n_("Move"),
            ModalKind::Rotate => n_("Rotate"),
            ModalKind::Scale => n_("Scale"),
            ModalKind::Extrude => n_("Extrude"),
            ModalKind::Inset => n_("Inset"),
            ModalKind::Bevel => n_("Bevel"),
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct Modal {
    pub kind: ModalKind,
    pub axis: Option<usize>,
    pub typed: String,
    /// Точка экрана в начале (берётся в первом кадре вида).
    pub start: Option<Pos2>,
    /// Центр выбранного в мире.
    pub pivot: Vec3,
    /// Документ, от которого считается каждый кадр (у выдавливания — уже выдавленный).
    pub base: Document,
    pub verts: Vec<u32>,
    /// Нормаль выдавливания в мире.
    pub normal: Vec3,
    pub faces: BTreeSet<usize>,
    pub edges: BTreeSet<Edge>,
    /// Новые грани (выдавливание) — им выделяется место в атласе при подтверждении.
    pub new_faces: Vec<usize>,
    pub value: f32,
    pub confirm: bool,
    pub cancel: bool,
}

/// Последняя операция — её числа можно поправить в панели, операция пересчитывается.
#[derive(Clone, Debug, PartialEq)]
pub struct LastOp {
    pub label: &'static str,
    pub kind: LastKind,
    /// Длина истории сразу после операции: другая правка — панель исчезает.
    pub history: usize,
}

#[derive(Clone, Debug, PartialEq)]
pub enum LastKind {
    Extrude { id: NodeId, faces: BTreeSet<usize>, distance: f32 },
    Inset { id: NodeId, faces: BTreeSet<usize>, thickness: f32 },
    Bevel { id: NodeId, edges: BTreeSet<Edge>, width: f32 },
    LoopCut { id: NodeId, edge: Edge, cuts: usize, slide: f32 },
    Primitive { prim: Primitive, parent: Option<NodeId> },
    Solidify { id: NodeId, thickness: f32 },
    Array { id: NodeId, count: usize, offset: [f32; 3] },
    Deform { id: NodeId, verts: BTreeSet<u32>, axis: usize, taper: f32, twist: f32 },
    Lathe { profile: Vec<Vec2>, segments: usize, at: Vec3, parent: Option<NodeId> },
    Tube { path: Vec<Vec3>, size: f32, parent: Option<NodeId> },
    Subdivide { id: NodeId, faces: BTreeSet<usize>, times: usize },
}

/// Операции над мешем (меню Mesh и клавиши).
#[derive(Clone, Debug, PartialEq)]
pub enum MeshCmd {
    SelectMode(SelectMode),
    SelectAll,
    SelectNone,
    Invert,
    Linked,
    Grow,
    Shrink,
    SimilarNormal,
    Extrude,
    Inset,
    Bevel,
    LoopCut,
    Subdivide,
    Fill,
    MergeCenter,
    Dissolve,
    Delete,
    Duplicate,
    Separate,
    Split,
    /// Нож: два щелчка по рёбрам одной грани режут её.
    Knife,
    /// Мост между двумя выделенными краевыми петлями.
    Bridge,
    FlipNormals,
    RecalcNormals,
    Triangulate,
    TrisToQuads,
    Solidify,
    MirrorX,
    Array,
    Taper,
    LatheSelection,
    TubeSelection,
    Join,
    CubeToMesh,
    SnapUvs,
    TexelDensity,
    Grab,
    Rotate,
    Scale,
    Add(Primitive),
    AddLathe,
}

// Имена примитивов и шагов истории приходят из кода (`Primitive::name`, подписи `edit`):
// i18n-keys: "Plane" "Cube" "Circle" "Cylinder" "Cone" "UV Sphere" "Ico Sphere" "Torus" "Wedge" "Stairs" "Grid"
// i18n-keys: "Move" "Rotate" "Scale" "Fill" "Delete" "Duplicate" "Merge" "Split" "Separate" "Dissolve" "Knife" "Bridge"
/// Профиль кувшина для Add Mesh ▸ Lathe: (радиус, высота) снизу вверх.
const VASE: [(f32, f32); 5] = [(0.0, 0.0), (3.0, 0.0), (4.0, 4.0), (2.0, 8.0), (2.5, 9.0)];

// ----------------------------------------------------------------------
// Вид: ввод, перетаскивание, отрисовка
// ----------------------------------------------------------------------

impl Viewport {
    /// Вершины меша в мире.
    pub(super) fn mesh_world_points(doc: &Document, id: NodeId) -> Vec<Vec3> {
        let Some(element) = doc.element(id) else { return Vec::new() };
        let matrix = doc.world_matrix(id);
        element.mesh_points().iter().map(|p| matrix.transform_point3(*p)).collect()
    }

    /// Стрелки правки меша: оси — у центра выбранных вершин, нормаль — у выбранной грани.
    pub(super) fn mesh_handles(&self, rect: Rect, doc: &Document, state: &MeshEdit) -> Vec<(Handle, Vec<Pos2>, Color32, Vec3, Vec3)> {
        let mut out = Vec::new();
        if doc.is_locked(state.id) || state.vertices.is_empty() || state.modal.is_some() {
            return out;
        }
        let points = Self::mesh_world_points(doc, state.id);
        let chosen: Vec<Vec3> = state.vertices.iter().filter_map(|v| points.get(*v as usize).copied()).collect();
        if chosen.is_empty() {
            return out;
        }
        let origin = chosen.iter().fold(Vec3::ZERO, |s, p| s + *p) / chosen.len() as f32;
        let length = 78.0 * self.units_per_point(origin, rect);
        for (i, axis) in [Vec3::X, Vec3::Y, Vec3::Z].iter().enumerate() {
            if let (Some(a), Some(b)) = (self.world_to_screen(origin), self.world_to_screen(origin + *axis * length)) {
                out.push((Handle::Axis(i), vec![a, b], AXIS[i], *axis, origin));
            }
        }
        if let (Some(face), Some(element)) = (state.face, doc.element(state.id)) {
            if let Some(f) = element.mesh.as_ref().and_then(|m| m.faces.get(face)) {
                let local = crate::mesh_edit::face_normal(&element.mesh_points(), f);
                let normal = doc.world_matrix(state.id).transform_vector3(local).normalize_or_zero();
                let stem = 40.0 * self.units_per_point(origin, rect);
                if let (Some(a), Some(b)) = (self.world_to_screen(origin), self.world_to_screen(origin + normal * stem)) {
                    out.push((Handle::Face(crate::doc::FaceDir::North), vec![a, b], Color32::from_rgb(0xe8, 0xc4, 0x4c), normal, origin));
                }
            }
        }
        out
    }

    /// Ребро меша ближе 7 точек к `pos` на экране.
    fn edge_at(&self, doc: &Document, id: NodeId, pos: Pos2) -> Option<Edge> {
        let element = doc.element(id)?;
        let m = EditMesh::of(element)?;
        let points = Self::mesh_world_points(doc, id);
        let screen: Vec<Option<Pos2>> = points.iter().map(|p| self.world_to_screen(*p)).collect();
        m.all_edges()
            .into_iter()
            .filter_map(|(a, b)| {
                let (pa, pb) = (screen[a as usize]?, screen[b as usize]?);
                Some(((a, b), super::segment_distance(pos, pa, pb)))
            })
            .filter(|(_, d)| *d <= 7.0)
            .min_by(|x, y| x.1.total_cmp(&y.1))
            .map(|x| x.0)
    }

    /// Ввод в режиме меша: щелчок выбирает по режиму (вершина / ребро / грань), Shift —
    /// добавить, Ctrl — убрать, Alt — петля; рамка; стрелки двигают выбранное.
    pub(super) fn mesh_edit_input(
        &mut self,
        ui: &Ui,
        response: &egui::Response,
        rect: Rect,
        cx: &mut Context,
        over_compass: bool,
        modifiers: egui::Modifiers,
    ) -> Option<Handle> {
        let state = self.mesh_edit.clone()?;
        let pointer = ui.input(|i| i.pointer.hover_pos()).filter(|p| rect.contains(*p));
        if let Some(m) = self.mesh_edit.as_mut() {
            m.hover_edge = None;
        }
        let hover_edge = pointer.and_then(|pos| self.edge_at(&cx.session.doc, state.id, pos));
        if let Some(m) = self.mesh_edit.as_mut() {
            m.hover_edge = hover_edge;
        }
        // Предпросмотр петлевого разреза: в режиме Ctrl+R и пока над ребром зажат Ctrl.
        self.loop_preview.clear();
        if state.modal.is_none() && state.knife.is_none() {
            let numbers = state.loop_cut.or_else(|| (modifiers.command && hover_edge.is_some()).then_some((1, 0.5)));
            if let (Some((cuts, slide)), Some(edge)) = (numbers, hover_edge) {
                if let Some(m) = cx.session.doc.element(state.id).and_then(EditMesh::of) {
                    let world = Self::mesh_world_points(&cx.session.doc, state.id);
                    let at = |(u, v, t): (u32, u32, f32)| world[u as usize].lerp(world[v as usize], t);
                    self.loop_preview = mo::loop_cut_preview(&m, edge, cuts, slide).into_iter().map(|[p, q]| (at(p), at(q))).collect();
                }
            }
        }
        if state.modal.is_some() {
            self.mesh_modal(ui, rect, cx, modifiers);
            return None;
        }
        if state.swallow_release {
            // Отпускание кнопки, которой подтвердили перенос, — не щелчок.
            if !ui.input(|i| i.pointer.any_down()) {
                if let Some(m) = self.mesh_edit.as_mut() {
                    m.swallow_release = false;
                }
            }
            return None;
        }
        if let Some((cuts, slide)) = state.loop_cut {
            if response.clicked_by(PointerButton::Primary) {
                if let Some(edge) = hover_edge {
                    let kind = LastKind::LoopCut { id: state.id, edge, cuts, slide };
                    let mut result = Ok(None);
                    cx.session.edit("Loop Cut", |doc| result = kind.apply(doc));
                    if let Some(m) = self.mesh_edit.as_mut() {
                        m.loop_cut = None;
                        m.clear();
                    }
                    match result {
                        Ok(_) => self.pending_last = Some(("Loop Cut", kind)),
                        Err(e) => self.notice = Some(e),
                    }
                } else {
                    self.notice = Some(tr("Loop cut: point at an edge first").to_string());
                }
            }
            return None;
        }
        if let Some(first) = state.knife {
            if response.clicked_by(PointerButton::Primary) {
                if let Some(pos) = response.interact_pointer_pos() {
                    self.knife_click(&mut *cx, state.id, first, pos);
                }
            }
            return None;
        }
        let handles = self.mesh_handles(rect, &cx.session.doc, &state);
        let hover = if self.drag.is_none() { pointer.and_then(|pos| self.handle_at(&handles, pos)) } else { None };
        let press = ui.input(|i| i.pointer.press_origin());
        if response.drag_started_by(PointerButton::Primary) && !modifiers.alt && !over_compass {
            let start = press.unwrap_or_default();
            if let Some(i) = self.handle_at(&handles, start) {
                let (handle, _, _, axis, origin) = handles[i].clone();
                let value = axis_param(origin, axis, self.ray(rect, start)).unwrap_or(0.0);
                cx.session.begin(if matches!(handle, Handle::Face(_)) { "Move Face" } else { "Move Vertices" });
                self.drag = Some(Drag::Mesh { handle, origin, axis, start: value });
            } else {
                self.drag = Some(Drag::Box { start });
            }
        }
        match self.drag.clone() {
            Some(Drag::Mesh { handle, origin, axis, start }) => {
                if response.dragged_by(PointerButton::Primary) {
                    if let (Some(pos), Some(base)) = (pointer, cx.session.pending_base().cloned()) {
                        if let Some(t) = axis_param(origin, axis, self.ray(rect, pos)) {
                            let amount = snap_value(t - start, cx.view.snap_step, cx.view.snapping, modifiers);
                            let mut doc = base;
                            match (handle, state.face) {
                                (Handle::Face(_), Some(face)) => {
                                    crate::mesh_edit::move_face(&mut doc, state.id, face, amount);
                                }
                                _ => {
                                    crate::mesh_edit::move_vertices(&mut doc, state.id, &state.vertices, axis * amount);
                                }
                            }
                            cx.session.doc = doc;
                        }
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
                        self.box_select(&cx.session.doc, &state, area, modifiers);
                    }
                }
                self.drag = None;
            }
            _ => {}
        }
        if response.clicked_by(PointerButton::Primary) && !over_compass && self.drag.is_none() {
            // Щелчок без перетаскивания выбирает и поверх стрелок (их центр часто
            // закрывает то, что под ним); стрелки работают перетаскиванием.
            if let Some(pos) = response.interact_pointer_pos() {
                self.click_select(rect, &cx.session.doc, &state, pos, modifiers);
            }
        }
        hover.map(|i| handles[i].0)
    }

    /// Точка на ребре под курсором: ребро и доля вдоль него от меньшей вершины.
    fn edge_point(&self, doc: &Document, id: NodeId, pos: Pos2) -> Option<(Edge, f32)> {
        let e = self.edge_at(doc, id, pos)?;
        let points = Self::mesh_world_points(doc, id);
        let (a, b) = (self.world_to_screen(points[e.0 as usize])?, self.world_to_screen(points[e.1 as usize])?);
        let ab = b - a;
        let t = ((pos - a).dot(ab) / ab.length_sq().max(1e-6)).clamp(0.05, 0.95);
        Some((e, t))
    }

    /// Щелчок ножом: первая точка запоминается, вторая (на другом ребре той же грани) режет.
    fn knife_click(&mut self, cx: &mut Context, id: NodeId, first: Option<(Edge, f32)>, pos: Pos2) {
        let Some(point) = self.edge_point(&cx.session.doc, id, pos) else {
            self.notice = Some(tr("Knife: click a point on an edge").to_string());
            return;
        };
        let Some(first) = first else {
            if let Some(m) = self.mesh_edit.as_mut() {
                m.knife = Some(Some(point));
            }
            return;
        };
        let Some(mesh) = cx.session.doc.element(id).and_then(EditMesh::of) else { return };
        let ef = mesh.edge_faces();
        let face = ef.get(&first.0).and_then(|a| ef.get(&point.0).and_then(|b| a.iter().find(|f| b.contains(f)))).copied();
        let Some(face) = face.filter(|_| first.0 != point.0) else {
            self.notice = Some(tr("Knife: the second point must be on another edge of the same face").to_string());
            return;
        };
        let mut result = Err(String::new());
        cx.session.edit("Knife", |doc| {
            result = mo::apply(doc, id, |m| mo::knife(m, face, first, point)).unwrap_or(Err(String::new()));
        });
        if let Some(m) = self.mesh_edit.as_mut() {
            m.knife = None;
            m.clear();
            if let (Ok(cut), Some(mesh)) = (&result, cx.session.doc.element(id).and_then(EditMesh::of)) {
                m.apply(mo::select_edges(&mesh, BTreeSet::from([*cut])));
            }
        }
        if let Err(e) = result {
            if !e.is_empty() {
                self.notice = Some(e);
            }
        }
    }

    fn box_select(&mut self, doc: &Document, state: &MeshEdit, area: Rect, modifiers: Modifiers) {
        let Some(m) = doc.element(state.id).and_then(EditMesh::of) else { return };
        let points = Self::mesh_world_points(doc, state.id);
        let inside: BTreeSet<u32> = (0..points.len() as u32).filter(|v| self.world_to_screen(points[*v as usize]).is_some_and(|p| area.contains(p))).collect();
        let Some(edit) = self.mesh_edit.as_mut() else { return };
        let mut verts = if modifiers.shift || modifiers.command { edit.verts() } else { BTreeSet::new() };
        if modifiers.command && !modifiers.shift {
            verts = edit.verts();
            for v in &inside {
                verts.remove(v);
            }
        } else {
            verts.extend(inside);
        }
        edit.set_verts(&m, verts);
    }

    fn click_select(&mut self, rect: Rect, doc: &Document, state: &MeshEdit, pos: Pos2, modifiers: Modifiers) {
        let Some(m) = doc.element(state.id).and_then(EditMesh::of) else { return };
        let points = Self::mesh_world_points(doc, state.id);
        let hit = raycast(&self.mesh, self.ray(rect, pos), |id| id == state.id);
        let hit_face = hit.and_then(|h| self.mesh.triangles.get(h.triangle)).map(|t| t.part as usize);
        let edge = self.edge_at(doc, state.id, pos);
        let (add, remove) = (modifiers.shift, modifiers.command && !modifiers.shift);
        let nearest_vertex = (0..points.len())
            .filter_map(|v| self.world_to_screen(points[v]).map(|p| (v as u32, p.distance(pos))))
            .filter(|(_, d)| *d <= 8.0)
            .min_by(|a, b| a.1.total_cmp(&b.1))
            .map(|(v, _)| v);
        let Some(edit) = self.mesh_edit.as_mut() else { return };
        // Alt+щелчок — петля рёбер (вершины, рёбра) или граней.
        if modifiers.alt {
            if let Some(e) = edge {
                let sel = match edit.mode {
                    SelectMode::Face => mo::select_faces(&m, mo::face_loop(&m, e)),
                    _ => mo::select_edges(&m, mo::edge_loop(&m, e)),
                };
                if add {
                    let mut verts = edit.verts();
                    verts.extend(sel.verts);
                    edit.set_verts(&m, verts);
                } else {
                    edit.apply(sel);
                }
            }
            return;
        }
        let mut verts = if add || remove { edit.verts() } else { BTreeSet::new() };
        let picked: Option<BTreeSet<u32>> = match edit.mode {
            SelectMode::Vertex => nearest_vertex.map(|v| BTreeSet::from([v])),
            SelectMode::Edge => edge.map(|(a, b)| BTreeSet::from([a, b])),
            SelectMode::Face => hit_face.and_then(|f| m.faces.get(f)).map(|f| f.vertices.iter().copied().collect()),
        };
        match picked {
            Some(p) => {
                // Щелчок по уже выбранному с Shift — снять (как в Blender).
                let already = p.iter().all(|v| verts.contains(v));
                if remove || (add && already && edit.mode == SelectMode::Vertex) {
                    for v in &p {
                        verts.remove(v);
                    }
                } else {
                    verts.extend(p);
                }
                edit.set_verts(&m, verts);
                if edit.mode == SelectMode::Face && !remove {
                    edit.face = hit_face;
                    // В режиме граней выбраны именно грани (а не все между вершинами).
                    if !add {
                        edit.faces = hit_face.into_iter().collect();
                    }
                }
            }
            None if !add && !remove => edit.clear(),
            None => {}
        }
    }

    /// Кадр перетаскивания с клавиатуры (G / R / S / E / I / Ctrl+B).
    fn mesh_modal(&mut self, ui: &Ui, rect: Rect, cx: &mut Context, modifiers: Modifiers) {
        let Some(edit) = self.mesh_edit.clone() else { return };
        let Some(mut modal) = edit.modal.clone() else { return };
        let pointer = ui.input(|i| i.pointer.hover_pos()).unwrap_or(rect.center());
        let start = *modal.start.get_or_insert(pointer);
        let (clicked, cancelled) = ui.input(|i| (i.pointer.primary_pressed(), i.pointer.secondary_pressed()));
        modal.confirm |= clicked;
        modal.cancel |= cancelled;
        let by_mouse = clicked || cancelled;
        let typed: Option<f32> = modal.typed.trim().parse().ok();
        let pivot_screen = self.world_to_screen(modal.pivot).unwrap_or(rect.center());
        let upp = self.units_per_point(modal.pivot, rect);
        let snapping = cx.view.snapping;
        let axis_vec = modal.axis.map(|a| Vec3::AXES[a]);
        let id = edit.id;
        let mut doc = modal.base.clone();
        match modal.kind {
            ModalKind::Grab | ModalKind::Extrude => {
                let direction = match (modal.kind, axis_vec) {
                    (_, Some(a)) => Some(a),
                    (ModalKind::Extrude, None) if modal.normal != Vec3::ZERO => Some(modal.normal),
                    _ => None,
                };
                let delta = match direction {
                    Some(dir) => {
                        let t0 = axis_param(modal.pivot, dir, self.ray(rect, start)).unwrap_or(0.0);
                        let t1 = axis_param(modal.pivot, dir, self.ray(rect, pointer)).unwrap_or(t0);
                        let amount = typed.unwrap_or_else(|| snap_value(t1 - t0, cx.view.snap_step, snapping, modifiers));
                        modal.value = amount;
                        dir * amount
                    }
                    None => {
                        let n = -self.camera.forward_dir();
                        let (r0, r1) = (self.ray(rect, start), self.ray(rect, pointer));
                        let p0 = r0.plane(modal.pivot, n).map_or(modal.pivot, |t| r0.at(t));
                        let p1 = r1.plane(modal.pivot, n).map_or(p0, |t| r1.at(t));
                        let raw = match typed {
                            Some(v) => Vec3::X * v,
                            None => p1 - p0,
                        };
                        let d = Vec3::new(
                            snap_value(raw.x, cx.view.snap_step, snapping && typed.is_none(), modifiers),
                            snap_value(raw.y, cx.view.snap_step, snapping && typed.is_none(), modifiers),
                            snap_value(raw.z, cx.view.snap_step, snapping && typed.is_none(), modifiers),
                        );
                        modal.value = d.length();
                        d
                    }
                };
                crate::mesh_edit::move_vertices(&mut doc, id, &modal.verts, delta);
            }
            ModalKind::Rotate => {
                let a0 = (start - pivot_screen).angle();
                let a1 = (pointer - pivot_screen).angle();
                let mut degrees = typed.unwrap_or(-(a1 - a0).to_degrees());
                if typed.is_none() && (snapping != modifiers.command) && cx.view.rotation_step > 0.0 {
                    degrees = (degrees / cx.view.rotation_step).round() * cx.view.rotation_step;
                }
                modal.value = degrees;
                let axis = axis_vec.unwrap_or(-self.camera.forward_dir());
                let q = Quat::from_axis_angle(axis.normalize_or_zero(), degrees.to_radians());
                let pivot = modal.pivot;
                transform_world(&mut doc, id, &modal.verts, |p| pivot + q * (p - pivot));
            }
            ModalKind::Scale => {
                let d0 = (start - pivot_screen).length().max(1.0);
                let d1 = (pointer - pivot_screen).length();
                let mut factor = typed.unwrap_or(d1 / d0);
                if typed.is_none() && modifiers.command {
                    factor = (factor * 10.0).round() / 10.0;
                }
                modal.value = factor;
                let pivot = modal.pivot;
                let scale = match axis_vec {
                    Some(a) => Vec3::ONE + a * (factor - 1.0),
                    None => Vec3::splat(factor),
                };
                transform_world(&mut doc, id, &modal.verts, |p| pivot + (p - pivot) * scale);
            }
            ModalKind::Inset | ModalKind::Bevel => {
                let amount = typed.unwrap_or_else(|| snap_value((pointer - start).length() * upp, cx.view.snap_step * 0.25, snapping, modifiers)).max(0.0);
                modal.value = amount;
                let faces = modal.faces.clone();
                let edges = modal.edges.clone();
                let ok = if modal.kind == ModalKind::Inset {
                    mo::apply(&mut doc, id, |m| mo::inset(m, &faces, amount)).is_some()
                } else {
                    mo::apply(&mut doc, id, |m| mo::bevel(m, &edges, amount).is_ok()).unwrap_or(false)
                };
                if !ok {
                    modal.cancel = true;
                }
            }
        }
        cx.session.doc = doc;
        if modal.cancel {
            cx.session.cancel();
            if let Some(e) = self.mesh_edit.as_mut() {
                e.modal = None;
                e.swallow_release = by_mouse;
            }
            self.notice = Some(trf!("{op} cancelled", op = tr(modal.kind.label())));
            return;
        }
        if modal.confirm {
            // Новые грани — место в атласе с цветом соседей.
            if !modal.new_faces.is_empty() {
                let _ = crate::mesh_edit::allocate_uvs(&mut cx.session.doc, id, &modal.new_faces);
            }
            if matches!(modal.kind, ModalKind::Inset | ModalKind::Bevel) {
                let _ = mo::apply(&mut cx.session.doc, id, |m| {
                    let n = m.faces.len();
                    n
                });
            }
            cx.session.commit();
            self.pending_last = Some(match modal.kind {
                ModalKind::Extrude if !modal.faces.is_empty() => {
                    Some(LastKind::Extrude { id, faces: modal.faces.clone(), distance: modal.value })
                }
                ModalKind::Inset => Some(LastKind::Inset { id, faces: modal.faces.clone(), thickness: modal.value }),
                ModalKind::Bevel => Some(LastKind::Bevel { id, edges: modal.edges.clone(), width: modal.value }),
                _ => None,
            })
            .flatten()
            .map(|kind| (modal.kind.label(), kind));
            if let Some(e) = self.mesh_edit.as_mut() {
                e.modal = None;
                e.swallow_release = by_mouse;
                // Выделение — по новому мешу (номера граней после операции).
                if let Some(m) = cx.session.doc.element(id).and_then(EditMesh::of) {
                    let verts: BTreeSet<u32> = e.vertices.iter().copied().filter(|v| (*v as usize) < m.points.len()).collect();
                    let faces = e.faces.clone();
                    e.set_verts(&m, verts);
                    if e.mode == SelectMode::Face && !faces.is_empty() {
                        e.faces = faces.into_iter().filter(|f| *f < m.faces.len()).collect();
                    }
                }
            }
            return;
        }
        if let Some(e) = self.mesh_edit.as_mut() {
            e.modal = Some(modal);
        }
    }

    /// Рёбра, точки, выбранные грани, размеры выбранного и подсказка внизу вида.
    pub(super) fn mesh_overlay(&self, painter: &egui::Painter, doc: &Document, state: &MeshEdit, p: &Palette) {
        let Some(m) = doc.element(state.id).and_then(EditMesh::of) else { return };
        let points = Self::mesh_world_points(doc, state.id);
        let screen: Vec<Option<Pos2>> = points.iter().map(|q| self.world_to_screen(*q)).collect();
        // Выбранные грани.
        for &f in &state.faces {
            let Some(face) = m.faces.get(f) else { continue };
            let poly: Option<Vec<Pos2>> = face.vertices.iter().map(|v| screen[*v as usize]).collect();
            if let Some(poly) = poly {
                let tris = mo::triangulate_polygon(&face.vertices.iter().map(|v| points[*v as usize]).collect::<Vec<_>>());
                for t in tris {
                    painter.add(EShape::convex_polygon(vec![poly[t[0]], poly[t[1]], poly[t[2]]], p.accent.gamma_multiply(0.22), Stroke::NONE));
                }
            }
        }
        // Рёбра: все тонко, выбранные — акцентом, под курсором — светлее.
        let selected: BTreeSet<Edge> = state.edges.iter().copied().collect();
        for (a, b) in m.all_edges() {
            let (Some(pa), Some(pb)) = (screen[a as usize], screen[b as usize]) else { continue };
            let stroke = if selected.contains(&(a, b)) {
                Stroke::new(2.0_f32, p.accent)
            } else if state.hover_edge == Some((a, b)) && state.mode != SelectMode::Vertex {
                Stroke::new(2.5_f32, Color32::from_rgb(0xe8, 0xc4, 0x4c))
            } else {
                Stroke::new(1.0_f32, Color32::from_black_alpha(150))
            };
            painter.line_segment([pa, pb], stroke);
        }
        if state.mode == SelectMode::Vertex {
            for (v, at) in screen.iter().enumerate() {
                let Some(at) = *at else { continue };
                if state.vertices.contains(&(v as u32)) {
                    painter.circle_filled(at, 4.0, p.accent);
                    painter.circle_stroke(at, 4.0, Stroke::new(1.0_f32, Color32::WHITE));
                } else {
                    painter.circle_filled(at, 2.5, Color32::from_white_alpha(200));
                    painter.circle_stroke(at, 2.5, Stroke::new(1.0_f32, Color32::from_black_alpha(160)));
                }
            }
        } else if state.mode == SelectMode::Face {
            for f in 0..m.faces.len() {
                let c = m.faces[f].vertices.iter().fold(Vec3::ZERO, |s, v| s + points[*v as usize]) / m.faces[f].vertices.len() as f32;
                if let Some(at) = self.world_to_screen(c) {
                    let fill = if state.faces.contains(&f) { p.accent } else { Color32::from_white_alpha(120) };
                    painter.rect_filled(Rect::from_center_size(at, vec2(4.0, 4.0)), 0.0, fill);
                }
            }
        }
        // Размеры выбранного.
        if state.vertices.len() >= 2 && state.modal.is_none() {
            let chosen: Vec<Vec3> = state.vertices.iter().filter_map(|v| points.get(*v as usize).copied()).collect();
            let (lo, hi) = chosen.iter().fold((Vec3::splat(f32::MAX), Vec3::splat(f32::MIN)), |(a, b), q| (a.min(*q), b.max(*q)));
            let text = if state.vertices.len() == 2 {
                trf!("length {d}", d = super::super::trim(chosen[0].distance(chosen[1])))
            } else {
                format!("{} × {} × {}", super::super::trim(hi.x - lo.x), super::super::trim(hi.y - lo.y), super::super::trim(hi.z - lo.z))
            };
            if let Some(at) = self.world_to_screen((lo + hi) * 0.5) {
                super::pill(painter, at + vec2(0.0, -26.0), &text, p);
            }
        }
        // Подсказка: что ждёт инструмент.
        let rect = painter.clip_rect();
        let tris = m.faces.iter().map(|f| f.vertices.len().saturating_sub(2)).sum::<usize>();
        if let Some(first) = state.knife {
            if let Some((e, t)) = first {
                let at = points[e.0 as usize].lerp(points[e.1 as usize], t);
                if let Some(at) = self.world_to_screen(at) {
                    painter.circle_filled(at, 4.0, Color32::from_rgb(0xe8, 0xc4, 0x4c));
                    if let Some(pointer) = painter.ctx().input(|i| i.pointer.hover_pos()) {
                        painter.line_segment([at, pointer], Stroke::new(1.5_f32, Color32::from_rgb(0xe8, 0xc4, 0x4c)));
                    }
                }
            }
        }
        // Предпросмотр петлевого разреза: новые петли поперёк кольца.
        let preview_color = Color32::from_rgb(0xe8, 0xc4, 0x4c);
        for (a, b) in &self.loop_preview {
            if let (Some(pa), Some(pb)) = (self.world_to_screen(*a), self.world_to_screen(*b)) {
                painter.line_segment([pa, pb], Stroke::new(2.0_f32, preview_color));
                painter.circle_filled(pa, 2.5, preview_color);
                painter.circle_filled(pb, 2.5, preview_color);
            }
        }
        let hint = match &state.modal {
            _ if state.knife.is_some() => tr("Knife: click a point on an edge, then a point on another edge of the same face · Esc cancels").to_string(),
            _ if state.loop_cut.is_some() => {
                let (cuts, _) = state.loop_cut.unwrap_or((1, 0.5));
                trf!("Loop cut ({cuts}): point at an edge and click · + − change the number of cuts · [ ] slide · Esc cancels", cuts = cuts)
            }
            Some(modal) => {
                let axis = modal.axis.map_or(String::new(), |a| format!(" · {}", ["X", "Y", "Z"][a]));
                let value = if modal.typed.is_empty() { super::super::trim(modal.value) } else { modal.typed.clone() };
                trf!(
                    "{op}{axis}: {value} · X Y Z axis · type a number · Ctrl snap · click or Enter to apply · right click or Esc to cancel",
                    op = tr(modal.kind.label()),
                    axis = axis,
                    value = value
                )
            }
            None => {
                let mode = match state.mode {
                    SelectMode::Vertex => tr("Vertex"),
                    SelectMode::Edge => tr("Edge"),
                    SelectMode::Face => tr("Face"),
                };
                trf!(
                    "{mode} select (1 2 3) · {tris} tris · G R S move rotate scale · E extrude · I inset · Ctrl+B bevel · Ctrl+R loop cut · F fill · M merge · X delete · Alt+click loop · V exit",
                    mode = mode,
                    tris = tris
                )
            }
        };
        let galley = painter.layout(hint, super::style::small(), p.text, (rect.width() - 24.0).max(100.0));
        let at = pos2(rect.min.x + 12.0, rect.max.y - 12.0 - galley.size().y);
        painter.rect_filled(Rect::from_min_size(at - vec2(6.0, 3.0), galley.size() + vec2(12.0, 6.0)), 3.0, Color32::from_black_alpha(150));
        painter.galley(at, galley, p.text);
    }
}

/// Преобразовать вершины меша функцией мировых координат.
fn transform_world(doc: &mut Document, id: NodeId, verts: &[u32], f: impl Fn(Vec3) -> Vec3) {
    let world = doc.world_matrix(id);
    let inverse = world.inverse();
    mo::apply(doc, id, |m| {
        for v in verts {
            if let Some(p) = m.points.get_mut(*v as usize) {
                *p = inverse.transform_point3(f(world.transform_point3(*p)));
            }
        }
    });
}

// ----------------------------------------------------------------------
// Окно: клавиши, меню, операции, панель последней операции
// ----------------------------------------------------------------------

impl LastKind {
    /// Выполнить над документом (с местами в атласе для новых граней). Возвращает новый узел.
    fn apply(&self, doc: &mut Document) -> Result<Option<NodeId>, String> {
        match self.clone() {
            LastKind::Extrude { id, faces, distance } => {
                let sides = mo::apply(doc, id, |m| mo::extrude_faces(m, &faces, distance).1).ok_or_else(not_mesh)?;
                crate::mesh_edit::allocate_uvs(doc, id, &sides)?;
                Ok(None)
            }
            LastKind::Inset { id, faces, thickness } => {
                mo::apply(doc, id, |m| mo::inset(m, &faces, thickness)).ok_or_else(not_mesh)?;
                Ok(None)
            }
            LastKind::Bevel { id, edges, width } => {
                let new = mo::apply(doc, id, |m| mo::bevel(m, &edges, width)).ok_or_else(not_mesh)??;
                crate::mesh_edit::allocate_uvs(doc, id, &new)?;
                Ok(None)
            }
            LastKind::LoopCut { id, edge, cuts, slide } => {
                mo::apply(doc, id, |m| mo::loop_cut(m, edge, cuts, slide)).ok_or_else(not_mesh)?;
                Ok(None)
            }
            LastKind::Primitive { prim, parent } => Ok(Some(add_mesh(doc, prim.name(), prim.build(), parent)?)),
            LastKind::Solidify { id, thickness } => {
                let before = doc.element(id).and_then(|e| e.mesh.as_ref()).map_or(0, |m| m.faces.len());
                mo::apply(doc, id, |m| mo::solidify(m, thickness)).ok_or_else(not_mesh)?;
                let after = doc.element(id).and_then(|e| e.mesh.as_ref()).map_or(0, |m| m.faces.len());
                crate::mesh_edit::allocate_uvs(doc, id, &(before * 2..after).collect::<Vec<_>>())?;
                Ok(None)
            }
            LastKind::Array { id, count, offset } => {
                mo::apply(doc, id, |m| mo::array(m, count, Vec3::from_array(offset))).ok_or_else(not_mesh)?;
                Ok(None)
            }
            LastKind::Deform { id, verts, axis, taper, twist } => {
                mo::apply(doc, id, |m| mo::deform(m, &verts, axis, taper, twist)).ok_or_else(not_mesh)?;
                Ok(None)
            }
            LastKind::Lathe { profile, segments, at, parent } => {
                let mut m = mo::lathe(&profile, segments, true);
                for p in &mut m.points {
                    *p += at;
                }
                Ok(Some(add_mesh(doc, "Lathe", m, parent)?))
            }
            LastKind::Tube { path, size, parent } => {
                let h = size * 0.5;
                let profile = [Vec2::new(-h, -h), Vec2::new(h, -h), Vec2::new(h, h), Vec2::new(-h, h)];
                Ok(Some(add_mesh(doc, "Tube", mo::sweep(&profile, &path, true), parent)?))
            }
            LastKind::Subdivide { id, faces, times } => {
                let mut faces = faces;
                for _ in 0..times.clamp(1, 4) {
                    faces = mo::apply(doc, id, |m| mo::subdivide(m, &faces)).ok_or_else(not_mesh)?;
                }
                Ok(None)
            }
        }
    }
}

fn not_mesh() -> String {
    "the element is not a mesh".to_string()
}

/// Новый элемент-меш с развёрткой по граням (по нему сразу можно рисовать).
fn add_mesh(doc: &mut Document, name: &str, m: EditMesh, parent: Option<NodeId>) -> Result<NodeId, String> {
    let id = crate::mesh_edit::add_mesh(doc, name, &m.points, m.faces, parent);
    crate::mesh_edit::generate_uvs(doc, id, None)?;
    Ok(id)
}

impl ModelerApp {
    /// Состояние режима меша (тестам и окну).
    pub fn mesh_selection(&self) -> Option<(SelectMode, Vec<u32>, Vec<Edge>, Vec<usize>)> {
        self.viewport.mesh_edit.as_ref().map(|m| (m.mode, m.vertices.clone(), m.edges.clone(), m.faces.clone()))
    }

    /// Предпросмотр петлевого разреза под курсором (отрезки в мировых координатах; пусто —
    /// режим не включён или курсор не над ребром).
    pub fn loop_cut_preview(&self) -> Vec<(Vec3, Vec3)> {
        if self.viewport.mesh_edit.is_none() {
            return Vec::new();
        }
        self.viewport.loop_preview.clone()
    }

    /// Показана ли панель последней операции: только в режиме Edit и только пока не начат
    /// другой инструмент (нож, петлевой разрез, G R S E I…).
    pub fn last_operation_panel_visible(&self) -> bool {
        self.mode == Mode::Edit
            && self.last_operation().is_some()
            && !self.viewport.mesh_edit.as_ref().is_some_and(|e| e.knife.is_some() || e.modal.is_some() || e.loop_cut.is_some())
    }

    /// Включён ли режим петлевого разреза (Ctrl+R без ребра): (число разрезов, доля).
    pub fn loop_cut_mode(&self) -> Option<(usize, f32)> {
        self.viewport.mesh_edit.as_ref().and_then(|e| e.loop_cut)
    }

    /// Последняя операция с числами (панель), если она ещё последняя в истории.
    pub fn last_operation(&self) -> Option<&LastOp> {
        self.last_op.as_ref().filter(|l| l.history == self.session().history_len().0)
    }

    /// Поменять число последней операции: шаг отменяется и делается заново с новым числом.
    pub fn redo_last(&mut self, kind: LastKind) {
        let Some(last) = self.last_operation().cloned() else { return };
        self.session_mut().undo();
        let mut result = Ok(None);
        self.session_mut().edit(last.label, |doc| result = kind.apply(doc));
        match result {
            Ok(new) => {
                if let Some(id) = new {
                    self.session_mut().select(Some(id));
                }
                let history = self.session().history_len().0;
                self.last_op = Some(LastOp { label: last.label, kind, history });
            }
            Err(e) => self.error(e),
        }
    }

    fn record_last(&mut self, label: &'static str, kind: LastKind) {
        let history = self.session().history_len().0;
        self.last_op = Some(LastOp { label, kind, history });
    }

    /// Меш режима правки и его состояние.
    fn mesh_state(&self) -> Option<(MeshEdit, EditMesh)> {
        let edit = self.viewport.mesh_edit.clone()?;
        let m = self.document().element(edit.id).and_then(EditMesh::of)?;
        Some((edit, m))
    }

    /// Клавиши режима меша (до общих сочетаний окна). `true` — клавиша съедена.
    pub(in crate::ui) fn mesh_shortcuts(&mut self, ctx: &egui::Context) {
        // Открытое меню Shift+A закрывает Esc (до того, как Esc снимет выделение).
        if self.add_menu_at.is_some() && ctx.input_mut(|i| i.consume_key(Modifiers::NONE, Key::Escape)) {
            self.add_menu_at = None;
            return;
        }
        let Some(edit) = self.viewport.mesh_edit.clone() else {
            // Вне режима меша: Shift+A — меню примитивов, Ctrl+J — объединить меши.
            let mut add = false;
            let mut join = false;
            ctx.input_mut(|i| {
                add = i.consume_key(Modifiers::SHIFT, Key::A);
                join = i.consume_key(Modifiers::COMMAND, Key::J);
            });
            if add {
                self.add_menu_at = ctx.input(|i| i.pointer.hover_pos());
            }
            if join {
                self.run_mesh(MeshCmd::Join);
            }
            return;
        };
        if self.mode != Mode::Edit {
            return;
        }
        let mut commands: Vec<MeshCmd> = Vec::new();
        if let Some((cuts, slide)) = edit.loop_cut {
            let mut next = (cuts, slide);
            let mut cancel = false;
            ctx.input_mut(|i| {
                cancel = i.consume_key(Modifiers::NONE, Key::Escape);
                if i.consume_key(Modifiers::NONE, Key::Plus) || i.consume_key(Modifiers::NONE, Key::Equals) {
                    next.0 = (next.0 + 1).min(16);
                }
                if i.consume_key(Modifiers::NONE, Key::Minus) {
                    next.0 = next.0.saturating_sub(1).max(1);
                }
                if i.consume_key(Modifiers::NONE, Key::OpenBracket) {
                    next.1 = (next.1 - 0.1).max(0.1);
                }
                if i.consume_key(Modifiers::NONE, Key::CloseBracket) {
                    next.1 = (next.1 + 0.1).min(0.9);
                }
            });
            if let Some(e) = self.viewport.mesh_edit.as_mut() {
                e.loop_cut = if cancel { None } else { Some(next) };
            }
            return;
        }
        if edit.knife.is_some() {
            let mut cancel = false;
            ctx.input_mut(|i| cancel = i.consume_key(Modifiers::NONE, Key::Escape) || i.consume_key(Modifiers::NONE, Key::K));
            if cancel {
                if let Some(e) = self.viewport.mesh_edit.as_mut() {
                    e.knife = None;
                }
            }
            return;
        }
        if let Some(mut modal) = edit.modal.clone() {
            // Во время перетаскивания: оси, число, подтверждение, отмена.
            ctx.input_mut(|i| {
                for (k, key) in [Key::X, Key::Y, Key::Z].into_iter().enumerate() {
                    if i.consume_key(Modifiers::NONE, key) {
                        modal.axis = if modal.axis == Some(k) { None } else { Some(k) };
                    }
                }
                for event in &i.events {
                    if let egui::Event::Text(text) = event {
                        for c in text.chars() {
                            if c.is_ascii_digit() || c == '.' || (c == '-' && modal.typed.is_empty()) {
                                modal.typed.push(c);
                            }
                        }
                    }
                }
                if i.consume_key(Modifiers::NONE, Key::Backspace) {
                    modal.typed.pop();
                }
                if i.consume_key(Modifiers::NONE, Key::Enter) {
                    modal.confirm = true;
                }
                if i.consume_key(Modifiers::NONE, Key::Escape) {
                    modal.cancel = true;
                }
            });
            if let Some(e) = self.viewport.mesh_edit.as_mut() {
                e.modal = Some(modal);
            }
            return;
        }
        ctx.input_mut(|i| {
            let mut take = |m: Modifiers, key: Key, cmd: MeshCmd| {
                if i.consume_key(m, key) {
                    commands.push(cmd);
                }
            };
            let none = Modifiers::NONE;
            take(none, Key::Num1, MeshCmd::SelectMode(SelectMode::Vertex));
            take(none, Key::Num2, MeshCmd::SelectMode(SelectMode::Edge));
            take(none, Key::Num3, MeshCmd::SelectMode(SelectMode::Face));
            take(Modifiers::ALT, Key::A, MeshCmd::SelectNone);
            take(none, Key::A, MeshCmd::SelectAll);
            take(Modifiers::COMMAND, Key::I, MeshCmd::Invert);
            take(Modifiers::COMMAND, Key::L, MeshCmd::Linked);
            take(none, Key::L, MeshCmd::Linked);
            take(Modifiers::COMMAND, Key::Plus, MeshCmd::Grow);
            take(Modifiers::COMMAND, Key::Equals, MeshCmd::Grow);
            take(Modifiers::COMMAND, Key::Minus, MeshCmd::Shrink);
            take(none, Key::G, MeshCmd::Grab);
            take(none, Key::R, MeshCmd::Rotate);
            take(none, Key::S, MeshCmd::Scale);
            take(none, Key::E, MeshCmd::Extrude);
            take(none, Key::I, MeshCmd::Inset);
            take(Modifiers::COMMAND, Key::B, MeshCmd::Bevel);
            take(Modifiers::COMMAND, Key::R, MeshCmd::LoopCut);
            take(none, Key::F, MeshCmd::Fill);
            take(none, Key::M, MeshCmd::MergeCenter);
            take(none, Key::X, MeshCmd::Delete);
            take(none, Key::Delete, MeshCmd::Delete);
            take(Modifiers::SHIFT, Key::D, MeshCmd::Duplicate);
            take(none, Key::P, MeshCmd::Separate);
            take(none, Key::K, MeshCmd::Knife);
            take(none, Key::Y, MeshCmd::Split);
            take(Modifiers::COMMAND, Key::T, MeshCmd::Triangulate);
            take(Modifiers::ALT, Key::J, MeshCmd::TrisToQuads);
            take(Modifiers::SHIFT, Key::N, MeshCmd::RecalcNormals);
            take(Modifiers::ALT, Key::N, MeshCmd::FlipNormals);
        });
        let mut add = false;
        ctx.input_mut(|i| add = i.consume_key(Modifiers::SHIFT, Key::A));
        if add {
            self.add_menu_at = ctx.input(|i| i.pointer.hover_pos());
        }
        for cmd in commands {
            self.run_mesh(cmd);
        }
    }

    /// Начать перетаскивание с клавиатуры над выбранными вершинами.
    fn start_modal(&mut self, kind: ModalKind, base: Document, verts: Vec<u32>, normal: Vec3, faces: BTreeSet<usize>, edges: BTreeSet<Edge>, new_faces: Vec<usize>) {
        // Начат другой инструмент: панель прошлой операции исчезает.
        self.last_op = None;
        let Some(edit) = self.viewport.mesh_edit.as_mut() else { return };
        let points = Viewport::mesh_world_points(&base, edit.id);
        let chosen: Vec<Vec3> = verts.iter().filter_map(|v| points.get(*v as usize).copied()).collect();
        let pivot = if chosen.is_empty() { Vec3::ZERO } else { chosen.iter().fold(Vec3::ZERO, |s, p| s + *p) / chosen.len() as f32 };
        edit.modal = Some(Modal {
            kind,
            axis: None,
            typed: String::new(),
            start: None,
            pivot,
            base,
            verts,
            normal,
            faces,
            edges,
            new_faces,
            value: 0.0,
            confirm: false,
            cancel: false,
        });
    }

    /// Выполнить операцию над мешем режима правки (или над выделенными элементами).
    pub fn run_mesh(&mut self, cmd: MeshCmd) {
        // Без режима меша: добавить примитив, объединить, кубоид в меш.
        match &cmd {
            MeshCmd::Add(prim) => {
                let prim = *prim;
                let parent = self.insertion_parent();
                let mut result = Ok(None);
                self.session_mut().edit("Add Mesh", |doc| result = LastKind::Primitive { prim, parent }.apply(doc));
                match result {
                    Ok(Some(id)) => {
                        self.session_mut().select(Some(id));
                        self.record_last("Add Mesh", LastKind::Primitive { prim, parent });
                        self.message(trf!("Added {name}: V edits its vertices, faces and edges", name = tr(prim.name())));
                    }
                    Ok(None) => {}
                    Err(e) => self.error(e),
                }
                return;
            }
            MeshCmd::AddLathe => {
                let parent = self.insertion_parent();
                let kind = LastKind::Lathe { profile: VASE.iter().map(|(r, y)| Vec2::new(*r, *y)).collect(), segments: 12, at: Vec3::ZERO, parent };
                self.run_new_element("Lathe", kind);
                return;
            }
            MeshCmd::Join => return self.join_selected(),
            MeshCmd::CubeToMesh => return self.cube_to_mesh(),
            _ => {}
        }
        let Some((edit, m)) = self.mesh_state() else {
            match self.selected_mesh() {
                Some(id) => {
                    self.viewport.mesh_edit = Some(MeshEdit::new(id));
                    self.run_mesh(cmd);
                }
                None => self.error(tr("select a mesh element first (Model ▸ Add Mesh, or Convert Cube to Mesh)").to_string()),
            }
            return;
        };
        let id = edit.id;
        let verts = edit.verts();
        let faces = match edit.mode {
            SelectMode::Face => edit.face_set(),
            _ => mo::faces_of_verts(&m, &verts),
        };
        let edges = match edit.mode {
            SelectMode::Edge => edit.edge_set(),
            _ => mo::edges_of_verts(&m, &verts),
        };
        let set_sel = |app: &mut ModelerApp, sel: mo::Selection| {
            if let Some(e) = app.viewport.mesh_edit.as_mut() {
                e.apply(sel);
            }
        };
        match cmd {
            MeshCmd::SelectMode(mode) => {
                if let Some(e) = self.viewport.mesh_edit.as_mut() {
                    e.mode = mode;
                    if mode == SelectMode::Face {
                        e.faces = mo::faces_of_verts(&m, &verts).into_iter().collect();
                    }
                }
            }
            MeshCmd::SelectAll => set_sel(self, mo::select_verts(&m, (0..m.points.len() as u32).collect())),
            MeshCmd::SelectNone => set_sel(self, mo::Selection::default()),
            MeshCmd::Invert => set_sel(self, mo::select_verts(&m, (0..m.points.len() as u32).filter(|v| !verts.contains(v)).collect())),
            MeshCmd::Linked => {
                let mut from = verts.clone();
                if from.is_empty() {
                    if let Some(e) = edit.hover_edge {
                        from.insert(e.0);
                    }
                }
                set_sel(self, mo::select_verts(&m, mo::linked(&m, &from)));
            }
            MeshCmd::Grow => set_sel(self, mo::select_verts(&m, mo::grow(&m, &verts))),
            MeshCmd::Shrink => set_sel(self, mo::select_verts(&m, mo::shrink(&m, &verts))),
            MeshCmd::SimilarNormal => {
                if let Some(f) = edit.face.or(faces.iter().next().copied()) {
                    set_sel(self, mo::select_faces(&m, mo::similar_normal(&m, f, 5.0, false)));
                }
            }
            MeshCmd::Grab | MeshCmd::Rotate | MeshCmd::Scale => {
                if verts.is_empty() {
                    return self.message(tr("select vertices, edges or faces first (click, box or A)").to_string());
                }
                let kind = match cmd {
                    MeshCmd::Grab => ModalKind::Grab,
                    MeshCmd::Rotate => ModalKind::Rotate,
                    _ => ModalKind::Scale,
                };
                self.session_mut().begin(kind.label());
                let base = self.document().clone();
                self.start_modal(kind, base, verts.into_iter().collect(), Vec3::ZERO, faces, edges, Vec::new());
            }
            MeshCmd::Extrude => {
                if verts.is_empty() {
                    return self.message(tr("select faces (3) or border edges (2) to extrude").to_string());
                }
                self.session_mut().begin("Extrude");
                let mut doc = self.document().clone();
                let world = doc.world_matrix(id);
                let (new_faces, moved, normal) = if !faces.is_empty() {
                    let normal = faces.iter().fold(Vec3::ZERO, |s, f| s + m.normal(*f) * m.area(*f)).normalize_or_zero();
                    let (caps, sides) = mo::apply(&mut doc, id, |m| mo::extrude_faces(m, &faces, 0.0)).unwrap_or_default();
                    let mesh = doc.element(id).and_then(EditMesh::of).unwrap_or_default();
                    let moved: BTreeSet<u32> = caps.iter().flat_map(|f| mesh.faces[*f].vertices.iter().copied()).collect();
                    if let Some(e) = self.viewport.mesh_edit.as_mut() {
                        e.set_verts(&mesh, moved.clone());
                        if e.mode == SelectMode::Face {
                            e.faces = caps.iter().copied().collect();
                        }
                    }
                    (sides, moved, world.transform_vector3(normal).normalize_or_zero())
                } else {
                    let (new, rim) = mo::apply(&mut doc, id, |m| mo::extrude_edges(m, &edges, 0.0)).unwrap_or_default();
                    if new.is_empty() {
                        self.session_mut().cancel();
                        return self.message(tr("only border edges (with one face) can be extruded; select faces to extrude a region").to_string());
                    }
                    let mesh = doc.element(id).and_then(EditMesh::of).unwrap_or_default();
                    let moved: BTreeSet<u32> = rim.iter().flat_map(|(a, b)| [*a, *b]).collect();
                    if let Some(e) = self.viewport.mesh_edit.as_mut() {
                        e.set_verts(&mesh, moved.clone());
                    }
                    (new, moved, Vec3::ZERO)
                };
                self.session_mut().doc = doc.clone();
                self.start_modal(ModalKind::Extrude, doc, moved.into_iter().collect(), normal, faces, edges, new_faces);
            }
            MeshCmd::Inset | MeshCmd::Bevel => {
                let inset = cmd == MeshCmd::Inset;
                if (inset && faces.is_empty()) || (!inset && edges.is_empty()) {
                    return self.message(if inset { tr("select faces to inset").to_string() } else { tr("select edges to bevel").to_string() });
                }
                let kind = if inset { ModalKind::Inset } else { ModalKind::Bevel };
                self.session_mut().begin(kind.label());
                let base = self.document().clone();
                self.start_modal(kind, base, verts.into_iter().collect(), Vec3::ZERO, faces, edges, Vec::new());
            }
            MeshCmd::LoopCut => {
                let Some(edge) = edit.hover_edge.or_else(|| edges.iter().next().copied()) else {
                    // Ни ребра под курсором, ни выбранного: режим разреза — наводим, смотрим
                    // предпросмотр, щёлкаем.
                    if let Some(e) = self.viewport.mesh_edit.as_mut() {
                        e.loop_cut = Some((1, 0.5));
                    }
                    self.last_op = None;
                    self.status = None;
                    return;
                };
                self.run_last("Loop Cut", LastKind::LoopCut { id, edge, cuts: 1, slide: 0.5 });
            }
            MeshCmd::Subdivide => {
                if faces.is_empty() {
                    return self.message(tr("select faces to subdivide").to_string());
                }
                self.run_last("Subdivide", LastKind::Subdivide { id, faces, times: 1 });
            }
            MeshCmd::Fill => {
                let sel = match edit.mode {
                    SelectMode::Edge => mo::select_edges(&m, edges),
                    _ => mo::select_verts(&m, verts),
                };
                let mut result = Ok(0);
                self.session_mut().edit("Fill", |doc| {
                    result = mo::apply(doc, id, |m| mo::fill(m, &sel)).unwrap_or(Err(not_mesh()));
                    if let Ok(f) = result {
                        let _ = crate::mesh_edit::allocate_uvs(doc, id, &[f]);
                    }
                });
                if let Err(e) = result {
                    self.error(e);
                }
            }
            MeshCmd::MergeCenter => {
                if verts.len() < 2 {
                    return self.message(tr("select two or more vertices to merge").to_string());
                }
                self.session_mut().edit("Merge", |doc| {
                    mo::apply(doc, id, |m| mo::merge(m, &verts, None));
                });
                set_sel(self, mo::Selection::default());
            }
            MeshCmd::Dissolve => {
                self.session_mut().edit("Dissolve", |doc| {
                    mo::apply(doc, id, |m| match edit.mode {
                        SelectMode::Vertex => mo::dissolve_verts(m, &verts),
                        SelectMode::Edge => mo::dissolve_edges(m, &edges),
                        SelectMode::Face => {
                            mo::dissolve_faces(m, &faces);
                        }
                    });
                });
                set_sel(self, mo::Selection::default());
            }
            MeshCmd::Delete => {
                if verts.is_empty() {
                    return self.message(tr("nothing is selected").to_string());
                }
                let what = match edit.mode {
                    SelectMode::Vertex => Delete::Vertices,
                    SelectMode::Edge => Delete::Edges,
                    SelectMode::Face => Delete::Faces,
                };
                let sel = mo::Selection { verts, edges, faces };
                self.session_mut().edit("Delete", |doc| {
                    mo::apply(doc, id, |m| mo::delete(m, &sel, what));
                });
                set_sel(self, mo::Selection::default());
            }
            MeshCmd::Duplicate => {
                if faces.is_empty() {
                    return self.message(tr("select faces to duplicate").to_string());
                }
                self.session_mut().begin("Duplicate");
                let mut doc = self.document().clone();
                let new = mo::apply(&mut doc, id, |m| mo::duplicate(m, &faces)).unwrap_or_default();
                let mesh = doc.element(id).and_then(EditMesh::of).unwrap_or_default();
                let moved: BTreeSet<u32> = new.iter().flat_map(|f| mesh.faces[*f].vertices.iter().copied()).collect();
                if let Some(e) = self.viewport.mesh_edit.as_mut() {
                    e.set_verts(&mesh, moved.clone());
                }
                self.session_mut().doc = doc.clone();
                self.start_modal(ModalKind::Grab, doc, moved.into_iter().collect(), Vec3::ZERO, new, BTreeSet::new(), Vec::new());
            }
            MeshCmd::Separate => {
                if faces.is_empty() {
                    return self.message(tr("select faces to separate into a new element").to_string());
                }
                let mut new_id = None;
                self.session_mut().edit("Separate", |doc| {
                    let Some(part) = mo::apply(doc, id, |m| mo::separate(m, &faces)) else { return };
                    let Some(source) = doc.element(id).cloned() else { return };
                    let parent = doc.node(id).and_then(|n| n.parent);
                    let mut element = crate::doc::Element::mesh(&part.points, part.faces);
                    element.pivot = source.pivot;
                    element.rotation = source.rotation;
                    element.faces[0] = source.faces[0];
                    new_id = Some(crate::ops::add_element(doc, "part", element, parent));
                });
                if let Some(n) = new_id {
                    self.viewport.mesh_edit = None;
                    self.session_mut().select(Some(n));
                    self.message(tr("separated into a new element").to_string());
                }
            }
            MeshCmd::Knife => {
                if let Some(e) = self.viewport.mesh_edit.as_mut() {
                    e.knife = Some(None);
                }
                // Подсказку ножа показывает подпись вида; в строке состояния её не повторяем,
                // а панель прошлой операции с этого момента не относится к делу.
                self.last_op = None;
                self.status = None;
            }
            MeshCmd::Bridge => {
                let mut result = Ok(Vec::new());
                self.session_mut().edit("Bridge", |doc| {
                    result = mo::apply(doc, id, |m| mo::bridge(m, &edges)).unwrap_or(Err(not_mesh()));
                    if let Ok(new) = &result {
                        let _ = crate::mesh_edit::allocate_uvs(doc, id, new);
                    }
                });
                match result {
                    Ok(new) => {
                        set_sel(self, mo::Selection::default());
                        self.message(trf!("Bridged with {n} faces", n = new.len()));
                    }
                    Err(e) => self.error(e),
                }
            }
            MeshCmd::Split => {
                self.session_mut().edit("Split", |doc| {
                    mo::apply(doc, id, |m| mo::split(m, &faces));
                });
            }
            MeshCmd::FlipNormals => {
                self.session_mut().edit("Flip Normals", |doc| {
                    mo::apply(doc, id, |m| mo::flip(m, &faces));
                });
            }
            MeshCmd::RecalcNormals => {
                self.session_mut().edit("Recalculate Normals", |doc| {
                    mo::apply(doc, id, mo::recalc_normals);
                });
            }
            MeshCmd::Triangulate => {
                let target = if faces.is_empty() { (0..m.faces.len()).collect() } else { faces };
                self.session_mut().edit("Triangulate", |doc| {
                    mo::apply(doc, id, |m| mo::triangulate(m, &target));
                });
                set_sel(self, mo::Selection::default());
            }
            MeshCmd::TrisToQuads => {
                let target = if faces.is_empty() { (0..m.faces.len()).collect() } else { faces };
                self.session_mut().edit("Tris to Quads", |doc| {
                    mo::apply(doc, id, |m| mo::tris_to_quads(m, &target));
                });
                set_sel(self, mo::Selection::default());
            }
            MeshCmd::Solidify => self.run_last("Solidify", LastKind::Solidify { id, thickness: 1.0 }),
            MeshCmd::MirrorX => {
                let pivot = self.document().element(id).map_or(0.0, |e| e.pivot.x);
                self.session_mut().edit("Mirror", |doc| {
                    mo::apply(doc, id, |m| mo::mirror(m, 0, pivot));
                });
                set_sel(self, mo::Selection::default());
            }
            MeshCmd::Array => {
                let width = m.points.iter().map(|p| p.x).fold(f32::MIN, f32::max) - m.points.iter().map(|p| p.x).fold(f32::MAX, f32::min);
                self.run_last("Array", LastKind::Array { id, count: 3, offset: [width.max(1.0) + 1.0, 0.0, 0.0] });
            }
            MeshCmd::Taper => {
                let target = if verts.is_empty() { (0..m.points.len() as u32).collect() } else { verts };
                self.run_last("Taper", LastKind::Deform { id, verts: target, axis: 1, taper: 0.5, twist: 0.0 });
            }
            MeshCmd::LatheSelection => {
                let Some(element) = self.document().element(id).cloned() else { return };
                let mut profile: Vec<Vec2> = verts
                    .iter()
                    .map(|v| {
                        let p = m.points[*v as usize] - element.pivot;
                        Vec2::new((p.x * p.x + p.z * p.z).sqrt(), p.y)
                    })
                    .collect();
                profile.sort_by(|a, b| a.y.total_cmp(&b.y));
                if profile.len() < 2 {
                    return self.message(tr("select two or more vertices of a profile (radius from the element's pivot, height up)").to_string());
                }
                let parent = self.document().node(id).and_then(|n| n.parent);
                self.run_new_element("Lathe", LastKind::Lathe { profile, segments: 12, at: element.pivot, parent });
            }
            MeshCmd::TubeSelection => {
                // Путь — выбранные рёбра цепочкой.
                let mut chain: Vec<u32> = Vec::new();
                let list: Vec<Edge> = edges.iter().copied().collect();
                if let Some(first) = list.first() {
                    chain = vec![first.0, first.1];
                    let mut rest: Vec<Edge> = list[1..].to_vec();
                    while let Some(i) = rest.iter().position(|(a, b)| *a == *chain.last().unwrap_or(&0) || *b == *chain.last().unwrap_or(&0)) {
                        let (a, b) = rest.remove(i);
                        chain.push(if a == *chain.last().unwrap_or(&0) { b } else { a });
                    }
                }
                if chain.len() < 2 {
                    return self.message(tr("select a chain of edges to sweep a tube along").to_string());
                }
                let world = self.document().world_matrix(id);
                let path = chain.iter().map(|v| world.transform_point3(m.points[*v as usize])).collect();
                self.run_new_element("Tube", LastKind::Tube { path, size: 1.0, parent: None });
            }
            MeshCmd::SnapUvs => {
                self.session_mut().edit("Snap UVs", |doc| {
                    crate::mesh_edit::snap_uvs(doc, id);
                });
                self.message(tr("UVs snapped to texel corners").to_string());
            }
            MeshCmd::TexelDensity => {
                let density = self.document().texel_density;
                let mut result = Ok(0);
                self.session_mut().edit("Texel Density", |doc| result = crate::mesh_edit::set_density(doc, id, density));
                match result {
                    Ok(_) => self.message(trf!("UVs laid out again at {d} texels per unit", d = super::super::trim(density))),
                    Err(e) => self.error(e),
                }
            }
            MeshCmd::Join | MeshCmd::CubeToMesh | MeshCmd::Add(_) | MeshCmd::AddLathe => {}
        }
    }

    fn run_last(&mut self, label: &'static str, kind: LastKind) {
        let mut result = Ok(None);
        self.session_mut().edit(label, |doc| result = kind.apply(doc));
        match result {
            Ok(_) => {
                self.record_last(label, kind);
                if let Some(e) = self.viewport.mesh_edit.as_mut() {
                    e.clear();
                }
            }
            Err(e) => self.error(e),
        }
    }

    fn run_new_element(&mut self, label: &'static str, kind: LastKind) {
        let mut result = Ok(None);
        self.session_mut().edit(label, |doc| result = kind.apply(doc));
        match result {
            Ok(Some(id)) => {
                self.viewport.mesh_edit = None;
                self.session_mut().select(Some(id));
                self.record_last(label, kind);
            }
            Ok(None) => {}
            Err(e) => self.error(e),
        }
    }

    /// Ctrl+J: выбранные элементы-меши — в первый (вершины переводятся в его пространство).
    fn join_selected(&mut self) {
        let ids: Vec<NodeId> = self.session().selection.iter().copied().filter(|id| self.document().element(*id).is_some_and(|e| e.shape == Shape::Mesh)).collect();
        if ids.len() < 2 {
            return self.message(tr("select two or more mesh elements to join (Shift+click in the outliner)").to_string());
        }
        let first = ids[0];
        self.session_mut().edit("Join", |doc| {
            let target = doc.world_matrix(first).inverse();
            let mut parts = Vec::new();
            for id in &ids[1..] {
                let world = doc.world_matrix(*id);
                if let Some(m) = doc.element(*id).and_then(EditMesh::of) {
                    parts.push(EditMesh { points: m.points.iter().map(|p| target.transform_point3(world.transform_point3(*p))).collect(), faces: m.faces });
                }
            }
            mo::apply(doc, first, |m| {
                for part in parts {
                    let base = m.points.len() as u32;
                    m.points.extend(part.points);
                    m.faces.extend(part.faces.into_iter().map(|mut f| {
                        for v in &mut f.vertices {
                            *v += base;
                        }
                        f
                    }));
                }
            });
            crate::ops::delete(doc, &ids[1..]);
        });
        self.session_mut().select(Some(first));
        self.message(trf!("Joined {n} meshes", n = ids.len()));
    }

    /// Выбранные кубоиды — мешами (с теми же UV сторон).
    fn cube_to_mesh(&mut self) {
        let ids: Vec<NodeId> = self.session().selection.iter().copied().filter(|id| self.document().element(*id).is_some_and(|e| e.shape == Shape::Cuboid)).collect();
        if ids.is_empty() {
            return self.message(tr("select cuboids to convert to meshes").to_string());
        }
        self.session_mut().edit("Convert Cube to Mesh", |doc| {
            for id in &ids {
                let Some(element) = doc.element(*id).cloned() else { continue };
                if let Some(mesh) = mo::cuboid_to_mesh(doc, &element) {
                    if let Some(node) = doc.node_mut(*id) {
                        node.kind = crate::doc::NodeKind::Element(mesh);
                    }
                }
            }
        });
        self.message(trf!("{n} cuboid(s) are meshes now: V edits their vertices", n = ids.len()));
    }

    /// Меню Mesh.
    pub(in crate::ui) fn mesh_menu(&mut self, ui: &mut Ui) -> Option<MeshCmd> {
        let mut out = None;
        let mesh = self.viewport.mesh_edit.is_some() || self.selected_mesh().is_some();
        let item = |ui: &mut Ui, text: &str, shortcut: &str, enabled: bool| controls::menu_item(ui, &tr(text), shortcut, enabled).clicked();
        ui.set_min_width(240.0);
        ui.menu_button(tr("Add Mesh"), |ui| {
            for prim in Primitive::DEFAULTS {
                if item(ui, prim.name(), "", true) {
                    out = Some(MeshCmd::Add(prim));
                }
            }
            if item(ui, n_("Lathe (vase profile)"), "", true) {
                out = Some(MeshCmd::AddLathe);
            }
        });
        if item(ui, n_("Convert Cube to Mesh"), "", true) {
            out = Some(MeshCmd::CubeToMesh);
        }
        if item(ui, n_("Join Meshes"), "Ctrl+J", true) {
            out = Some(MeshCmd::Join);
        }
        ui.separator();
        let ops: &[(&str, &str, MeshCmd)] = &[
            (n_("Extrude"), "E", MeshCmd::Extrude),
            (n_("Inset Faces"), "I", MeshCmd::Inset),
            (n_("Bevel Edges"), "Ctrl+B", MeshCmd::Bevel),
            (n_("Loop Cut"), "Ctrl+R", MeshCmd::LoopCut),
            (n_("Subdivide"), "", MeshCmd::Subdivide),
            (n_("Fill (Make Face)"), "F", MeshCmd::Fill),
            (n_("Merge at Centre"), "M", MeshCmd::MergeCenter),
            (n_("Dissolve"), "", MeshCmd::Dissolve),
            (n_("Delete"), "X", MeshCmd::Delete),
            (n_("Duplicate"), "Shift+D", MeshCmd::Duplicate),
            (n_("Separate"), "P", MeshCmd::Separate),
            (n_("Split"), "Y", MeshCmd::Split),
            (n_("Knife"), "K", MeshCmd::Knife),
            (n_("Bridge Edge Loops"), "", MeshCmd::Bridge),
        ];
        for (label, key, cmd) in ops {
            if item(ui, label, key, mesh) {
                out = Some(cmd.clone());
            }
        }
        ui.separator();
        let more: &[(&str, &str, MeshCmd)] = &[
            (n_("Flip Normals"), "Alt+N", MeshCmd::FlipNormals),
            (n_("Recalculate Normals Outside"), "Shift+N", MeshCmd::RecalcNormals),
            (n_("Triangulate"), "Ctrl+T", MeshCmd::Triangulate),
            (n_("Tris to Quads"), "Alt+J", MeshCmd::TrisToQuads),
            (n_("Solidify"), "", MeshCmd::Solidify),
            (n_("Mirror X (apply)"), "", MeshCmd::MirrorX),
            (n_("Array"), "", MeshCmd::Array),
            (n_("Taper"), "", MeshCmd::Taper),
            (n_("Lathe Selected Profile"), "", MeshCmd::LatheSelection),
            (n_("Tube Along Selected Edges"), "", MeshCmd::TubeSelection),
        ];
        for (label, key, cmd) in more {
            if item(ui, label, key, mesh) {
                out = Some(cmd.clone());
            }
        }
        ui.separator();
        let select: &[(&str, &str, MeshCmd)] = &[
            (n_("Select All"), "A", MeshCmd::SelectAll),
            (n_("Select Linked"), "L", MeshCmd::Linked),
            (n_("Select More"), "Ctrl+=", MeshCmd::Grow),
            (n_("Select Less"), "Ctrl+−", MeshCmd::Shrink),
            (n_("Invert Selection"), "Ctrl+I", MeshCmd::Invert),
            (n_("Select Similar Normal"), "", MeshCmd::SimilarNormal),
        ];
        for (label, key, cmd) in select {
            if item(ui, label, key, mesh) {
                out = Some(cmd.clone());
            }
        }
        ui.separator();
        if item(ui, n_("Snap UVs to Texels"), "", mesh) {
            out = Some(MeshCmd::SnapUvs);
        }
        if item(ui, n_("Set Texel Density"), "", mesh) {
            out = Some(MeshCmd::TexelDensity);
        }
        if out.is_some() {
            ui.close_menu();
        }
        out
    }

    /// Панель последней операции (в углу вида) и меню Shift+A.
    pub(in crate::ui) fn mesh_panels(&mut self, ctx: &egui::Context) {
        let Some(rect) = self.viewport.rect else { return };
        if let Some(at) = self.add_menu_at {
            let mut chosen = None;
            let mut close = false;
            let response = egui::Area::new(egui::Id::new("modeler.add_mesh")).fixed_pos(at).order(egui::Order::Foreground).show(ctx, |ui| {
                egui::Frame::popup(ui.style()).show(ui, |ui| {
                    ui.label(tr("Add Mesh"));
                    let entry = |ui: &mut Ui, key: &str, label: &str| {
                        let id = ModelerApp::button_id(&format!("add_mesh.{key}"));
                        controls::button_with_id(ui, Some(id), controls::Kind::Normal, None, &tr(label), 140.0).clicked()
                    };
                    for prim in Primitive::DEFAULTS {
                        if entry(ui, prim.name(), prim.name()) {
                            chosen = Some(MeshCmd::Add(prim));
                        }
                    }
                    if entry(ui, "Lathe", n_("Lathe (vase profile)")) {
                        chosen = Some(MeshCmd::AddLathe);
                    }
                });
            });
            let outside = ctx.input(|i| i.pointer.any_pressed() && i.pointer.interact_pos().is_some_and(|p| !response.response.rect.contains(p)));
            if ctx.input(|i| i.key_pressed(Key::Escape)) || (outside && chosen.is_none()) {
                close = true;
            }
            if let Some(cmd) = chosen {
                self.add_menu_at = None;
                self.run_mesh(cmd);
            } else if close {
                self.add_menu_at = None;
            }
        }
        if !self.last_operation_panel_visible() {
            return;
        }
        let Some(last) = self.last_operation().cloned() else { return };
        /// Ширина колонки подписей панели последней операции.
        const LAST_OP_LABEL_W: f32 = 76.0;
        let mut changed: Option<LastKind> = None;
        egui::Area::new(egui::Id::new("modeler.last_op")).fixed_pos(rect.left_top() + vec2(kit::SP_M, 34.0)).order(egui::Order::Foreground).show(ctx, |ui| {
            egui::Frame::popup(ui.style()).show(ui, |ui| {
                ui.set_max_width(220.0);
                ui.label(tr(last.label));
                let mut kind = last.kind.clone();
                let mut edit = |ui: &mut Ui, label: &str, value: &mut f32, lo: f32, hi: f32| {
                    controls::row(ui, |ui| {
                        // Подписи — одной колонкой: поля разных строк начинаются с одного x.
                        let h = ui.available_height();
                        ui.allocate_ui_with_layout(vec2(LAST_OP_LABEL_W, h), egui::Layout::left_to_right(egui::Align::Center), |ui| {
                            ui.set_min_width(LAST_OP_LABEL_W);
                            ui.label(tr(label))
                        });
                        controls::number_field_range(ui, value, lo..=hi, 0.1, "", 70.0).changed()
                    })
                };
                let mut count = |ui: &mut Ui, label: &str, value: &mut usize, lo: usize, hi: usize| {
                    let mut v = *value as f32;
                    let c = controls::row(ui, |ui| {
                        // Подписи — одной колонкой: поля разных строк начинаются с одного x.
                        let h = ui.available_height();
                        ui.allocate_ui_with_layout(vec2(LAST_OP_LABEL_W, h), egui::Layout::left_to_right(egui::Align::Center), |ui| {
                            ui.set_min_width(LAST_OP_LABEL_W);
                            ui.label(tr(label))
                        });
                        controls::number_field_range(ui, &mut v, lo as f32..=hi as f32, 1.0, "", 70.0).changed()
                    });
                    *value = v.round() as usize;
                    c
                };
                let dirty = match &mut kind {
                    LastKind::Extrude { distance, .. } => edit(ui, n_("Distance"), distance, -256.0, 256.0),
                    LastKind::Inset { thickness, .. } => edit(ui, n_("Thickness"), thickness, 0.0, 256.0),
                    LastKind::Bevel { width, .. } => edit(ui, n_("Width"), width, 0.0, 256.0),
                    LastKind::LoopCut { cuts, slide, .. } => {
                        let a = count(ui, n_("Cuts"), cuts, 1, 32);
                        let b = *cuts == 1 && edit(ui, n_("Slide"), slide, 0.02, 0.98);
                        a || b
                    }
                    LastKind::Primitive { prim, .. } => primitive_fields(ui, prim, &mut edit, &mut count),
                    LastKind::Solidify { thickness, .. } => edit(ui, n_("Thickness"), thickness, -64.0, 64.0),
                    LastKind::Array { count: n, offset, .. } => {
                        let a = count(ui, n_("Count"), n, 1, 64);
                        let b = edit(ui, "X", &mut offset[0], -512.0, 512.0);
                        let c = edit(ui, "Y", &mut offset[1], -512.0, 512.0);
                        let d = edit(ui, "Z", &mut offset[2], -512.0, 512.0);
                        a || b || c || d
                    }
                    LastKind::Deform { taper, twist, .. } => {
                        let a = edit(ui, n_("Taper"), taper, 0.0, 4.0);
                        let b = edit(ui, n_("Twist"), twist, -720.0, 720.0);
                        a || b
                    }
                    LastKind::Lathe { segments, .. } => count(ui, n_("Segments"), segments, 3, 64),
                    LastKind::Tube { size, .. } => edit(ui, n_("Size"), size, 0.1, 64.0),
                    LastKind::Subdivide { times, .. } => count(ui, n_("Times"), times, 1, 4),
                };
                if dirty {
                    changed = Some(kind);
                }
            });
        });
        if let Some(kind) = changed {
            self.redo_last(kind);
        }
    }
}

/// Поля примитива в панели последней операции.
fn primitive_fields(
    ui: &mut Ui,
    prim: &mut Primitive,
    edit: &mut dyn FnMut(&mut Ui, &str, &mut f32, f32, f32) -> bool,
    count: &mut dyn FnMut(&mut Ui, &str, &mut usize, usize, usize) -> bool,
) -> bool {
    match prim {
        Primitive::Plane { size } | Primitive::Cube { size } | Primitive::Wedge { size } => edit(ui, n_("Size"), size, 0.5, 256.0),
        Primitive::Grid { size, cells } => edit(ui, n_("Size"), size, 0.5, 256.0) | count(ui, n_("Cells"), cells, 1, 32),
        Primitive::Circle { radius, segments } => edit(ui, n_("Radius"), radius, 0.25, 128.0) | count(ui, n_("Segments"), segments, 3, 64),
        Primitive::Cylinder { radius, height, segments, .. } => {
            edit(ui, n_("Radius"), radius, 0.25, 128.0) | edit(ui, n_("Height"), height, 0.25, 256.0) | count(ui, n_("Segments"), segments, 3, 64)
        }
        Primitive::Cone { radius, height, segments } => {
            edit(ui, n_("Radius"), radius, 0.25, 128.0) | edit(ui, n_("Height"), height, 0.25, 256.0) | count(ui, n_("Segments"), segments, 3, 64)
        }
        Primitive::UvSphere { radius, segments, rings } => {
            edit(ui, n_("Radius"), radius, 0.25, 128.0) | count(ui, n_("Segments"), segments, 3, 64) | count(ui, n_("Rings"), rings, 2, 32)
        }
        Primitive::IcoSphere { radius, subdivisions } => edit(ui, n_("Radius"), radius, 0.25, 128.0) | count(ui, n_("Subdivisions"), subdivisions, 0, 3),
        Primitive::Torus { major, minor, segments, sides } => {
            edit(ui, n_("Radius"), major, 0.5, 128.0) | edit(ui, n_("Thickness"), minor, 0.1, 64.0) | count(ui, n_("Segments"), segments, 3, 64) | count(ui, n_("Sides"), sides, 3, 32)
        }
        Primitive::Stairs { width, steps, step } => {
            edit(ui, n_("Width"), width, 0.5, 256.0) | count(ui, n_("Steps"), steps, 1, 32) | edit(ui, n_("Step"), step, 0.25, 32.0)
        }
    }
}
