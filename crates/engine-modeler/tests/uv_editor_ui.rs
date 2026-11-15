//! Редактор UV мешей мышью и клавишами (egui без окна): панель открывается клавишей U,
//! щелчок выбирает грань (и она же выбрана в 3D-виде), G с числом сдвигает, результат —
//! на углах текселей, Esc отменяет, каждая правка — один шаг истории, Ctrl+Z возвращает
//! документ байт в байт; рамка, остров (режим 3 и L), R / S с числом, развёртка проекцией.
#![cfg(feature = "ui")]

use std::collections::BTreeSet;

use egui::{Key, Modifiers, Pos2};
use engine_modeler::mesh_ops::{self, SelectMode};
use engine_modeler::ui::{Command, MeshCmd, ModelerApp, UvCmd};
use engine_modeler::{ops, uv_ops, Document, Element, NodeId};
use glam::Vec3;

const SCREEN: egui::Vec2 = egui::vec2(1400.0, 860.0);

struct Hands {
    ctx: egui::Context,
    time: f64,
    modifiers: Modifiers,
}

impl Hands {
    fn new(app: &mut ModelerApp) -> Self {
        let mut hands = Self { ctx: egui::Context::default(), time: 0.0, modifiers: Modifiers::NONE };
        app.autosave_seconds = 0.0;
        engine_modeler::ui::kit::apply(&hands.ctx, true);
        for _ in 0..3 {
            hands.frame(app, vec![]);
        }
        hands
    }

    fn frame(&mut self, app: &mut ModelerApp, events: Vec<egui::Event>) {
        let input = egui::RawInput {
            screen_rect: Some(egui::Rect::from_min_size(Pos2::ZERO, SCREEN)),
            time: Some(self.time),
            predicted_dt: 1.0 / 60.0,
            modifiers: self.modifiers,
            events,
            ..Default::default()
        };
        self.time += 1.0 / 60.0;
        let _ = self.ctx.run(input, |ctx| {
            egui::CentralPanel::default().frame(egui::Frame::none()).show(ctx, |ui| app.show(ui));
        });
    }

    fn button(&self, pos: Pos2, pressed: bool) -> egui::Event {
        egui::Event::PointerButton { pos, button: egui::PointerButton::Primary, pressed, modifiers: self.modifiers }
    }

    fn move_to(&mut self, app: &mut ModelerApp, pos: Pos2) {
        self.frame(app, vec![egui::Event::PointerMoved(pos)]);
        self.frame(app, vec![]);
    }

    fn click(&mut self, app: &mut ModelerApp, pos: Pos2) {
        self.frame(app, vec![egui::Event::PointerMoved(pos)]);
        let down = self.button(pos, true);
        self.frame(app, vec![down]);
        let up = self.button(pos, false);
        self.frame(app, vec![up]);
        self.frame(app, vec![]);
    }

    fn drag(&mut self, app: &mut ModelerApp, from: Pos2, to: Pos2) {
        self.frame(app, vec![egui::Event::PointerMoved(from)]);
        let down = self.button(from, true);
        self.frame(app, vec![down]);
        for k in 1..=4 {
            let at = from + (to - from) * (k as f32 / 4.0);
            self.frame(app, vec![egui::Event::PointerMoved(at)]);
        }
        let up = self.button(to, false);
        self.frame(app, vec![up]);
        self.frame(app, vec![]);
    }

    fn key(&mut self, app: &mut ModelerApp, key: Key, modifiers: Modifiers) {
        let old = self.modifiers;
        self.modifiers = modifiers;
        self.frame(app, vec![egui::Event::Key { key, physical_key: None, pressed: true, repeat: false, modifiers }]);
        self.frame(app, vec![egui::Event::Key { key, physical_key: None, pressed: false, repeat: false, modifiers }]);
        self.modifiers = old;
        self.frame(app, vec![]);
    }

    fn click_uv(&mut self, app: &mut ModelerApp, uv: [f32; 2]) {
        let pos = app.uv_to_screen(uv).expect("the UV editor is open");
        self.click(app, pos);
    }

    fn move_uv(&mut self, app: &mut ModelerApp, uv: [f32; 2]) {
        let pos = app.uv_to_screen(uv).expect("the UV editor is open");
        self.move_to(app, pos);
    }

    fn drag_uv(&mut self, app: &mut ModelerApp, from: [f32; 2], to: [f32; 2]) {
        let (a, b) = (app.uv_to_screen(from).unwrap(), app.uv_to_screen(to).unwrap());
        self.drag(app, a, b);
    }

    fn typed(&mut self, app: &mut ModelerApp, text: &str) {
        self.frame(app, vec![egui::Event::Text(text.to_string())]);
    }
}

/// Куб 8×8×8 мешем, развёрнутый «по граням»: шесть островов 8×8 с зазором 1.
fn cube_app() -> (ModelerApp, Hands, NodeId) {
    let mut app = ModelerApp::new();
    let mut doc = Document::new("t");
    let m = mesh_ops::box_mesh(Vec3::ZERO, Vec3::splat(8.0));
    let id = ops::add_element(&mut doc, "cube", Element::mesh(&m.points, m.faces), None);
    app.open_document(doc, None);
    app.session_mut().select(Some(id));
    let mut hands = Hands::new(&mut app);
    // U открывает панель.
    let center = egui::pos2(700.0, 400.0);
    hands.move_to(&mut app, center);
    hands.key(&mut app, Key::U, Modifiers::NONE);
    assert!(app.uv_editor_open(), "U opens the UV editor");
    app.run(Command::Uv(UvCmd::ByFace));
    hands.frame(&mut app, vec![]);
    hands.frame(&mut app, vec![]);
    (app, hands, id)
}

fn face_rect(app: &ModelerApp, id: NodeId, f: usize) -> [f32; 4] {
    let shape = app.document().element(id).unwrap().mesh.clone().unwrap();
    let uvs = &shape.faces[f].uvs;
    let min = |k: usize| uvs.iter().map(|u| u[k]).fold(f32::INFINITY, f32::min);
    let max = |k: usize| uvs.iter().map(|u| u[k]).fold(f32::NEG_INFINITY, f32::max);
    [min(0), min(1), max(0), max(1)]
}

fn at(app: &ModelerApp, uv: [f32; 2]) -> Pos2 {
    app.uv_to_screen(uv).expect("the UV editor is open")
}

#[test]
fn unwrap_by_face_lays_out_exact_rectangles_in_the_panel() {
    let (app, _hands, id) = cube_app();
    for f in 0..6 {
        let x = 9.0 * f as f32;
        assert_eq!(face_rect(&app, id, f), [x, 0.0, x + 8.0, 8.0], "face {f}");
    }
    let canvas = app.uv_canvas().expect("the canvas is drawn");
    assert!(canvas.width() > 250.0 && canvas.height() > 250.0, "the canvas is usable at 1400x860: {canvas:?}");
}

#[test]
fn click_selects_a_face_here_and_in_the_3d_view_and_back() {
    let (mut app, mut hands, id) = cube_app();
    hands.click_uv(&mut app, [13.0, 4.0]);
    let sel = app.uv_selection();
    assert_eq!(sel.len(), 4, "face 1 has four corners: {sel:?}");
    assert!(sel.iter().all(|c| c.0 == 1));
    let (mode, _, _, faces) = app.mesh_selection().expect("selecting in the UV editor starts the face selection in 3D");
    assert_eq!((mode, faces), (SelectMode::Face, vec![1]));
    // Shift-щелчок по другой грани добавляет её.
    hands.modifiers = Modifiers::SHIFT;
    hands.click_uv(&mut app, [31.0, 4.0]);
    hands.modifiers = Modifiers::NONE;
    assert_eq!(app.mesh_selection().unwrap().3, vec![1, 3]);
    // Выбор в 3D-виде → подсветка и выбор здесь.
    app.run(Command::Mesh(MeshCmd::SelectNone));
    hands.frame(&mut app, vec![]);
    assert!(app.uv_selection().is_empty());
    app.run(Command::Mesh(MeshCmd::SelectAll));
    hands.frame(&mut app, vec![]);
    assert_eq!(app.uv_selection().len(), 24);
    let _ = id;
}

#[test]
fn g_with_a_typed_value_moves_on_texel_corners_and_undo_restores() {
    let (mut app, mut hands, id) = cube_app();
    let before = app.document().clone();
    let history = app.session().history_len().0;
    hands.click_uv(&mut app, [13.0, 4.0]);
    hands.key(&mut app, Key::G, Modifiers::NONE);
    assert!(app.uv_edit_in_progress());
    hands.typed(&mut app, "3");
    hands.key(&mut app, Key::Enter, Modifiers::NONE);
    assert!(!app.uv_edit_in_progress());
    assert_eq!(face_rect(&app, id, 1), [12.0, 0.0, 20.0, 8.0]);
    assert_eq!(app.session().history_len().0, history + 1, "one undo step");
    // Мышью: на 2.4 текселя вправо и 1.6 вниз — с прижатием ровно (2, 2).
    let start = at(&app, [16.0, 4.0]);
    hands.move_to(&mut app, start);
    hands.key(&mut app, Key::G, Modifiers::NONE);
    let scale = (at(&app, [17.0, 4.0]) - at(&app, [16.0, 4.0])).x;
    let to = start + egui::vec2(2.4 * scale, 1.6 * scale);
    hands.move_to(&mut app, to);
    hands.click(&mut app, to);
    let r = face_rect(&app, id, 1);
    assert_eq!(r, [14.0, 2.0, 22.0, 10.0], "snapped to whole texels: {r:?}");
    assert_eq!(app.session().history_len().0, history + 2);
    // Ctrl+Z ×2 — как было, байт в байт.
    hands.key(&mut app, Key::Z, Modifiers::COMMAND);
    hands.key(&mut app, Key::Z, Modifiers::COMMAND);
    assert_eq!(app.document(), &before);
}

#[test]
fn escape_cancels_and_typed_values_are_not_snapped() {
    let (mut app, mut hands, id) = cube_app();
    hands.click_uv(&mut app, [13.0, 4.0]);
    let before = app.document().clone();
    let history = app.session().history_len();
    hands.key(&mut app, Key::G, Modifiers::NONE);
    hands.typed(&mut app, "5");
    hands.key(&mut app, Key::Escape, Modifiers::NONE);
    assert!(!app.uv_edit_in_progress());
    assert_eq!(app.document(), &before, "Esc leaves the document untouched");
    assert_eq!(app.session().history_len(), history);
    // Значение с точкой — как введено (без прижатия), X запирает ось.
    hands.key(&mut app, Key::G, Modifiers::NONE);
    hands.key(&mut app, Key::Y, Modifiers::NONE);
    hands.typed(&mut app, "2.5");
    hands.key(&mut app, Key::Enter, Modifiers::NONE);
    assert_eq!(face_rect(&app, id, 1), [9.0, 2.5, 17.0, 10.5]);
}

#[test]
fn rotate_and_scale_with_numbers() {
    let (mut app, mut hands, id) = cube_app();
    hands.click_uv(&mut app, [13.0, 4.0]);
    // Скос: сначала растянем по X вдвое (S X 2), затем R 90 — прямоугольник 16×8 ляжет 8×16.
    hands.key(&mut app, Key::S, Modifiers::NONE);
    hands.key(&mut app, Key::X, Modifiers::NONE);
    hands.typed(&mut app, "2");
    hands.key(&mut app, Key::Enter, Modifiers::NONE);
    let r = face_rect(&app, id, 1);
    assert_eq!((r[2] - r[0], r[3] - r[1]), (16.0, 8.0), "{r:?}");
    hands.key(&mut app, Key::R, Modifiers::NONE);
    hands.typed(&mut app, "90");
    hands.key(&mut app, Key::Enter, Modifiers::NONE);
    let r = face_rect(&app, id, 1);
    assert_eq!((r[2] - r[0], r[3] - r[1]), (8.0, 16.0), "{r:?}");
    assert!(r.iter().all(|v| v.fract() == 0.0), "corners stay on texel corners: {r:?}");
}

#[test]
fn box_select_island_select_and_select_all() {
    let (mut app, mut hands, _) = cube_app();
    // Рамка вокруг граней 2 и 3 (центры в (22, 4) и (31, 4)).
    hands.drag_uv(&mut app, [19.0, 10.0], [36.0, -2.0]);
    let faces: BTreeSet<usize> = app.uv_selection().iter().map(|c| c.0).collect();
    assert_eq!(faces, BTreeSet::from([2, 3]), "box selects the faces whose centres are inside");
    assert_eq!(app.mesh_selection().unwrap().3, vec![2, 3]);
    // A — всё, Alt+A — ничего.
    hands.key(&mut app, Key::A, Modifiers::NONE);
    assert_eq!(app.uv_selection().len(), 24);
    hands.key(&mut app, Key::A, Modifiers::ALT);
    assert!(app.uv_selection().is_empty());
    // Режим «остров» (3) и L: проекция вдоль Z склеивает весь куб в один остров.
    app.run(Command::Uv(UvCmd::Axis(2)));
    hands.frame(&mut app, vec![]);
    hands.key(&mut app, Key::Num3, Modifiers::NONE);
    hands.click_uv(&mut app, [4.0, 4.0]);
    assert!(app.uv_selection().len() >= 16, "an island of the box projection: {}", app.uv_selection().len());
    hands.key(&mut app, Key::A, Modifiers::ALT);
    hands.move_uv(&mut app, [4.0, 4.0]);
    hands.key(&mut app, Key::L, Modifiers::NONE);
    assert!(app.uv_selection().len() >= 16, "L selects the linked island");
}

#[test]
fn projection_gives_the_expected_rectangle() {
    let (mut app, mut hands, id) = cube_app();
    app.run(Command::Uv(UvCmd::Axis(1)));
    hands.frame(&mut app, vec![]);
    let shape = app.document().element(id).unwrap().mesh.clone().unwrap();
    let all: BTreeSet<usize> = (0..6).collect();
    let (lo, hi) = uv_ops::bounds(&shape, &uv_ops::corners_of_faces(&shape, &all)).unwrap();
    assert_eq!((lo.to_array(), hi.to_array()), ([0.0, 0.0], [8.0, 8.0]), "a top-down projection of the 8x8 cube is an 8x8 rectangle");
    // Куб из шести граней — после проекции кубом снова шесть островов 8×8.
    app.run(Command::Uv(UvCmd::Box));
    hands.frame(&mut app, vec![]);
    let shape = app.document().element(id).unwrap().mesh.clone().unwrap();
    assert_eq!(uv_ops::islands(&shape, None).len(), 6);
    // Плотность 32 px/m при 16 единицах на метр — 2 текселя на единицу: острова 16×16.
    app.set_uv_pixels_per_metre(32.0);
    app.run(Command::Uv(UvCmd::ByFace));
    hands.frame(&mut app, vec![]);
    let units = app.document().units_per_metre;
    let want = 8.0 * 32.0 / units;
    let r = face_rect(&app, id, 0);
    assert_eq!((r[2] - r[0], r[3] - r[1]), (want.round(), want.round()), "texel density is pixels per metre / units per metre");
}

#[test]
fn the_panel_is_closed_again_with_u_and_keys_return_to_the_3d_view() {
    let (mut app, mut hands, _) = cube_app();
    hands.move_to(&mut app, egui::pos2(1000.0, 400.0));
    hands.key(&mut app, Key::U, Modifiers::NONE);
    assert!(!app.uv_editor_open());
    assert!(app.uv_canvas().is_none());
}

/// Грань, которая смотрит на камеру больше всех (её и двигаем, по ней и рисуем).
fn facing_face(app: &ModelerApp, id: NodeId) -> usize {
    let forward = app.camera().forward_dir();
    let m = mesh_ops::EditMesh::of(app.document().element(id).unwrap()).unwrap();
    (0..m.faces.len()).max_by(|a, b| (-m.normal(*a).dot(forward)).total_cmp(&(-m.normal(*b).dot(forward)))).unwrap()
}

/// Правка UV → кисть в 3D-виде рисует там, где UV, а не там, где грань лежала раньше →
/// `.pmodel` → загрузка → запекание для движка: UV и пиксель на месте. С `MODELER_PROPS_OUT`
/// кладёт образец для теста движка (`crates/engine/tests/modeled_props.rs`).
#[test]
fn edited_uvs_are_painted_saved_loaded_and_baked_where_the_editor_put_them() {
    let (mut app, mut hands, id) = cube_app();
    app.run(Command::Frame);
    hands.frame(&mut app, vec![]);
    let f = facing_face(&app, id);
    let old = face_rect(&app, id, f);
    let [x0, y0, x1, y1] = old;
    // Выбрать остров в панели и сдвинуть его вниз на 20 текселей: G, Y, 20, Enter.
    hands.click_uv(&mut app, [(x0 + x1) / 2.0, (y0 + y1) / 2.0]);
    hands.key(&mut app, Key::G, Modifiers::NONE);
    hands.key(&mut app, Key::Y, Modifiers::NONE);
    hands.typed(&mut app, "20");
    hands.key(&mut app, Key::Enter, Modifiers::NONE);
    let moved = face_rect(&app, id, f);
    assert_eq!(moved, [x0, y0 + 20.0, x1, y1 + 20.0]);
    assert_eq!(app.document().textures[0].image.get(x0 as i32 + 4, y0 as i32 + 24), Some([0, 0, 0, 0]), "the new place of the island is empty");
    // Кисть в 3D-виде: Tab, карандаш, красный, щелчок по центру этой грани.
    hands.key(&mut app, Key::Tab, Modifiers::NONE);
    assert_eq!(app.mode(), engine_modeler::ui::Mode::Paint);
    hands.key(&mut app, Key::B, Modifiers::NONE);
    app.paint.color = [255, 0, 0, 255];
    app.paint.size = 1;
    let m = mesh_ops::EditMesh::of(app.document().element(id).unwrap()).unwrap();
    let centre = m.centroid(f);
    let at = app.world_to_screen(centre).expect("the face is on screen");
    hands.click(&mut app, at);
    let image = app.document().textures[0].image.clone();
    let red: Vec<(i32, i32)> = (0..image.height() as i32).flat_map(|y| (0..image.width() as i32).map(move |x| (x, y))).filter(|(x, y)| image.get(*x, *y) == Some([255, 0, 0, 255])).collect();
    assert_eq!(red.len(), 1, "one texel painted: {red:?}");
    let (rx, ry) = red[0];
    assert!((x0 as i32..x1 as i32).contains(&rx) && ((y0 + 20.0) as i32..(y1 + 20.0) as i32).contains(&ry), "the stroke landed in the moved island ({rx}, {ry}), not in the old place");
    // Сохранение и загрузка: документ тот же, UV на месте.
    let dir = std::env::temp_dir().join(format!("modeler-uvedit-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("uvedit.pmodel");
    app.run(Command::SaveTo(path.clone()));
    let loaded = engine_modeler::format::load(&path).unwrap();
    assert_eq!(&loaded, app.document(), "edited UVs and the painted texel survive save and load");
    assert_eq!(face_rect_of(&loaded, id, f), moved);
    // Запекание для движка: треугольник, в который попадает красный тексель, — из UV этой грани.
    let baked = engine_modeler::bake_bytes(&std::fs::read(&path).unwrap()).unwrap();
    let atlas = baked.texture.clone().expect("the model has an atlas");
    let [aw, ah] = atlas.size();
    assert_eq!(atlas.get(rx, ry), Some([255, 0, 0, 255]), "the atlas has the marker where it was painted");
    let marker = [(rx as f32 + 0.5) / aw as f32, (ry as f32 + 0.5) / ah as f32];
    let corners: Vec<[f32; 2]> = loaded.element(id).unwrap().mesh.as_ref().unwrap().faces[f].uvs.iter().map(|u| [u[0] / aw as f32, u[1] / ah as f32]).collect();
    let near = |a: [f32; 2], b: [f32; 2]| (a[0] - b[0]).abs() < 1e-5 && (a[1] - b[1]).abs() < 1e-5;
    let inside = |t: [[f32; 2]; 3]| {
        let s = |a: [f32; 2], b: [f32; 2], p: [f32; 2]| (a[0] - p[0]) * (b[1] - p[1]) - (b[0] - p[0]) * (a[1] - p[1]);
        let d = [s(t[0], t[1], marker), s(t[1], t[2], marker), s(t[2], t[0], marker)];
        d.iter().all(|v| *v >= -1e-9) || d.iter().all(|v| *v <= 1e-9)
    };
    let hit: Vec<[[f32; 2]; 3]> = baked
        .indices
        .chunks_exact(3)
        .map(|t| [baked.uvs[t[0] as usize], baked.uvs[t[1] as usize], baked.uvs[t[2] as usize]])
        .filter(|t| inside(*t))
        .collect();
    assert!(!hit.is_empty(), "a baked triangle covers the marker texel");
    for t in &hit {
        assert!(t.iter().all(|u| corners.iter().any(|c| near(*c, *u))), "the triangle under the marker uses the UV corners of the edited face");
    }
    if let Ok(out) = std::env::var("MODELER_PROPS_OUT") {
        let out = std::path::PathBuf::from(out);
        std::fs::copy(&path, out.join("uvedit.pmodel")).unwrap();
        std::fs::write(out.join("uvedit.txt"), format!("{rx} {ry} {aw} {ah}\n")).unwrap();
    }
    let _ = std::fs::remove_dir_all(dir);
}

fn face_rect_of(doc: &Document, id: NodeId, f: usize) -> [f32; 4] {
    let uvs = &doc.element(id).unwrap().mesh.as_ref().unwrap().faces[f].uvs;
    let min = |k: usize| uvs.iter().map(|u| u[k]).fold(f32::INFINITY, f32::min);
    let max = |k: usize| uvs.iter().map(|u| u[k]).fold(f32::NEG_INFINITY, f32::max);
    [min(0), min(1), max(0), max(1)]
}

#[test]
fn the_menus_open_and_paint_mode_leaves_the_uvs_alone() {
    let (mut app, mut hands, _) = cube_app();
    let layers = hands.ctx.memory(|m| m.layer_ids().count());
    let button = app.uv_menu_buttons()[1];
    hands.click(&mut app, button.center());
    assert!(hands.ctx.memory(|m| m.layer_ids().count()) > layers, "the UV menu opens from its button");
    hands.click(&mut app, button.center());
    // В режиме Paint панель только показывает: щелчок и клавиши не правят UV.
    app.run(Command::SetMode(engine_modeler::ui::Mode::Paint));
    hands.frame(&mut app, vec![]);
    let before = app.document().clone();
    hands.click_uv(&mut app, [13.0, 4.0]);
    assert!(app.uv_selection().is_empty(), "no selection in Paint mode");
    hands.key(&mut app, Key::G, Modifiers::NONE);
    assert!(!app.uv_edit_in_progress());
    assert_eq!(app.document(), &before);
}
