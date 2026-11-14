//! «Открыл модель — не выделяется и не правится»: настоящий ввод мышью и клавиатурой
//! (egui без окна) после открытия каждого образца `.pmodel` — с командной строки (`open`)
//! и через File ▸ Open (`OpenFile`) — и после открытия чужого меша (`.obj`): щелчок
//! выделяет элемент, гизмо двигает его, штрих кисти меняет текстуру; у меша — правка
//! вершин клавишей V и «Convert to Cubes».
#![cfg(feature = "ui")]

use std::path::{Path, PathBuf};

use egui::{vec2, Pos2};
use engine_modeler::ui::{Command, Mode, ModelerApp, Tool};
use engine_modeler::{Document, Shape};
use glam::Vec3;

const SCREEN: egui::Vec2 = egui::vec2(1400.0, 860.0);

struct Hands {
    ctx: egui::Context,
    time: f64,
}

impl Hands {
    fn new(app: &mut ModelerApp) -> Self {
        let mut hands = Self { ctx: egui::Context::default(), time: 0.0 };
        app.autosave_seconds = 0.0;
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
            events,
            ..Default::default()
        };
        self.time += 1.0 / 60.0;
        let _ = self.ctx.run(input, |ctx| {
            egui::CentralPanel::default().frame(egui::Frame::none()).show(ctx, |ui| app.show(ui));
        });
    }

    fn button(pos: Pos2, pressed: bool, modifiers: egui::Modifiers) -> egui::Event {
        egui::Event::PointerButton { pos, button: egui::PointerButton::Primary, pressed, modifiers }
    }

    fn click(&mut self, app: &mut ModelerApp, pos: Pos2) {
        self.frame(app, vec![egui::Event::PointerMoved(pos)]);
        self.frame(app, vec![Self::button(pos, true, egui::Modifiers::NONE)]);
        self.frame(app, vec![Self::button(pos, false, egui::Modifiers::NONE)]);
        self.frame(app, vec![]);
    }

    fn drag(&mut self, app: &mut ModelerApp, from: Pos2, to: Pos2, steps: usize) {
        self.frame(app, vec![egui::Event::PointerMoved(from)]);
        self.frame(app, vec![Self::button(from, true, egui::Modifiers::NONE)]);
        for i in 1..=steps {
            let t = i as f32 / steps as f32;
            self.frame(app, vec![egui::Event::PointerMoved(from + (to - from) * t)]);
        }
        self.frame(app, vec![Self::button(to, false, egui::Modifiers::NONE)]);
        self.frame(app, vec![]);
    }

    fn key(&mut self, app: &mut ModelerApp, key: egui::Key) {
        self.frame(app, vec![egui::Event::Key { key, physical_key: None, pressed: true, repeat: false, modifiers: egui::Modifiers::NONE }]);
        self.frame(app, vec![]);
    }
}

/// Найти точку экрана, щелчок по которой выделяет элемент: сетка по экранным границам
/// модели. Возвращает точку и выделенный элемент.
fn click_on_model(app: &mut ModelerApp, hands: &mut Hands) -> (Pos2, engine_modeler::NodeId) {
    let doc = app.document().clone();
    let (min, max) = doc.model_bounds().expect("the model has elements");
    let corners: Vec<Pos2> = (0..8)
        .filter_map(|i| {
            let p = Vec3::new(if i & 1 == 0 { min.x } else { max.x }, if i & 2 == 0 { min.y } else { max.y }, if i & 4 == 0 { min.z } else { max.z });
            app.world_to_screen(p)
        })
        .collect();
    let lo = corners.iter().fold(Pos2::new(f32::MAX, f32::MAX), |m, p| Pos2::new(m.x.min(p.x), m.y.min(p.y)));
    let hi = corners.iter().fold(Pos2::new(f32::MIN, f32::MIN), |m, p| Pos2::new(m.x.max(p.x), m.y.max(p.y)));
    // От центра наружу: первая точка, где щелчок выделил элемент.
    let mut points: Vec<Pos2> = (1..10).flat_map(|i| (1..10).map(move |j| (i, j))).map(|(i, j)| Pos2::new(lo.x + (hi.x - lo.x) * i as f32 / 10.0, lo.y + (hi.y - lo.y) * j as f32 / 10.0)).collect();
    let center = Pos2::new((lo.x + hi.x) * 0.5, (lo.y + hi.y) * 0.5);
    points.sort_by(|a, b| a.distance(center).total_cmp(&b.distance(center)));
    for pos in points {
        app.run(Command::SelectNone);
        hands.click(app, pos);
        if let Some(id) = app.session().primary() {
            if app.document().element(id).is_some() {
                return (pos, id);
            }
        }
    }
    panic!("no click on the model selected anything (status: {:?})", app.status_message());
}

/// Выделение, перенос гизмо (квадрат в центре — перенос в плоскости экрана), штрих кисти.
fn select_move_paint(app: &mut ModelerApp, hands: &mut Hands, what: &str) {
    assert_eq!(app.mode(), Mode::Edit, "{what}: opens in Edit mode");
    app.run(Command::Frame);
    hands.frame(app, vec![]);
    let (pos, id) = click_on_model(app, hands);
    assert!(!app.document().is_locked(id), "{what}: the picked element is not locked");

    // Перенос: инструмент Move (W), тащим центральный квадрат гизмо.
    hands.key(app, egui::Key::W);
    assert_eq!(app.tool(), Tool::Move);
    let before = app.document().element(id).unwrap().clone();
    let (min, max) = app.document().bounds(&[id]).unwrap();
    let center = app.world_to_screen((min + max) * 0.5).expect("the gizmo is on screen");
    hands.drag(app, center, center + vec2(60.0, -25.0), 8);
    let after = app.document().element(id).unwrap().clone();
    assert!((after.from - before.from).length() > 0.5, "{what}: the gizmo moved the element ({:?} → {:?})", before.from, after.from);
    app.run(Command::Undo);
    assert_eq!(app.document().element(id).unwrap(), &before, "{what}: undo puts it back");

    // Рисование: Tab, карандаш, штрих там, где щёлкнули.
    hands.key(app, egui::Key::Tab);
    assert_eq!(app.mode(), Mode::Paint);
    hands.key(app, egui::Key::B);
    app.paint.color = [1, 254, 3, 255];
    let textures_before: Vec<_> = app.document().textures.iter().map(|t| t.image.clone()).collect();
    hands.drag(app, pos - vec2(3.0, 0.0), pos + vec2(3.0, 0.0), 4);
    let changed = app.document().textures.iter().zip(&textures_before).any(|(t, b)| t.image != *b);
    assert!(changed, "{what}: a paint stroke changed the texture (status: {:?})", app.status_message());
    hands.key(app, egui::Key::Tab);
    assert_eq!(app.mode(), Mode::Edit);
}

fn sample_files() -> Vec<PathBuf> {
    let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../../assets/models");
    let mut files: Vec<PathBuf> = std::fs::read_dir(&dir)
        .map(|d| d.filter_map(|e| e.ok().map(|e| e.path())).filter(|p| p.extension().is_some_and(|e| e == "pmodel")).collect())
        .unwrap_or_default();
    files.sort();
    files
}

#[test]
fn every_sample_selects_moves_and_paints_after_open() {
    let mut files = sample_files();
    if files.is_empty() {
        // Modeler без движка рядом: образцы из кода.
        let dir = std::env::temp_dir().join(format!("modeler-samples-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        for (name, doc) in engine_modeler::samples::all() {
            let path = dir.join(format!("{name}.pmodel"));
            engine_modeler::format::save(&doc, &path).unwrap();
            files.push(path);
        }
    }
    for file in &files {
        // С командной строки.
        let mut app = ModelerApp::new();
        app.open(file).unwrap();
        let mut hands = Hands::new(&mut app);
        select_move_paint(&mut app, &mut hands, &format!("{} (command line)", file.display()));
        // Через File ▸ Open.
        let mut app = ModelerApp::new();
        let mut hands = Hands::new(&mut app);
        app.run(Command::OpenFile(file.clone()));
        hands.frame(&mut app, vec![]);
        assert!(app.path().is_some(), "{}: opened", file.display());
        select_move_paint(&mut app, &mut hands, &format!("{} (File ▸ Open)", file.display()));
    }
}

#[test]
fn a_click_in_paint_mode_on_empty_space_explains_the_modes() {
    let mut app = ModelerApp::new();
    app.open_document(engine_modeler::samples::rover(), None);
    let mut hands = Hands::new(&mut app);
    hands.key(&mut app, egui::Key::Tab);
    let rect = app.viewport_rect().unwrap();
    hands.click(&mut app, rect.min + vec2(30.0, rect.height() - 30.0));
    let status = app.status_message().unwrap_or_default().to_string();
    assert!(status.contains("Tab"), "{status}");
}

const CUBE_OBJ: &str = "mtllib cube.mtl\no crate\n\
v 0 0 0\nv 1 0 0\nv 1 1 0\nv 0 1 0\nv 0 0 1\nv 1 0 1\nv 1 1 1\nv 0 1 1\n\
vt 0 0\nvt 1 0\nvt 1 1\nvt 0 1\nusemtl wood\n\
f 1/1 4/4 3/3 2/2\nf 5/1 6/2 7/3 8/4\nf 1/1 2/2 6/3 5/4\nf 4/1 8/4 7/3 3/2\nf 1/1 5/2 8/3 4/4\nf 2/1 3/4 7/3 6/2\n";

fn obj_dir(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("modeler-open-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join("crate.obj"), CUBE_OBJ).unwrap();
    std::fs::write(dir.join("cube.mtl"), "newmtl wood\nKd 1 1 1\nmap_Kd wood.png\n").unwrap();
    std::fs::write(dir.join("wood.png"), engine_modeler::Image::new(8, 8, [200, 30, 30, 255]).to_png()).unwrap();
    dir
}

#[test]
fn an_opened_obj_is_an_editable_mesh() {
    let dir = obj_dir("mesh");
    let mut app = ModelerApp::new();
    let mut hands = Hands::new(&mut app);
    app.run(Command::OpenFile(dir.join("crate.obj")));
    hands.frame(&mut app, vec![]);
    assert!(app.status_message().unwrap_or_default().contains("crate.obj as an editable mesh"), "{:?}", app.status_message());
    assert!(app.path().is_none(), "the source .obj is never overwritten: Save asks for a .pmodel");
    assert_eq!(app.document().elements().next().unwrap().1.shape, Shape::Mesh);
    select_move_paint(&mut app, &mut hands, "crate.obj");

    // Вершины: V, щелчок по углу, стрелка X — угол сдвигается вдоль X.
    let (pos, id) = click_on_model(&mut app, &mut hands);
    let _ = pos;
    hands.key(&mut app, egui::Key::V);
    assert!(app.mesh_edit_state().is_some(), "V enters vertex mode (status: {:?})", app.status_message());
    let points = app.document().element(id).unwrap().mesh_points();
    let world = app.document().world_matrix(id);
    // Видимая вершина: ближайшая к камере.
    let eye = app.camera().eye();
    let (index, corner) = points
        .iter()
        .enumerate()
        .map(|(i, p)| (i, world.transform_point3(*p)))
        .min_by(|a, b| a.1.distance(eye).total_cmp(&b.1.distance(eye)))
        .unwrap();
    let at = app.world_to_screen(corner).unwrap();
    hands.click(&mut app, at);
    let (_, vertices, _) = app.mesh_edit_state().unwrap();
    assert_eq!(vertices, vec![index as u32], "the click picked the corner");
    let tip = app.world_to_screen(corner + Vec3::X * 2.0).unwrap();
    let step = (tip - at).normalized();
    hands.drag(&mut app, at + step * 20.0, at + step * 80.0, 8);
    let moved = app.document().element(id).unwrap().mesh_points();
    let moved_corner = world.transform_point3(moved[index]);
    assert!(moved_corner.x > corner.x + 0.5, "the X arrow moved the vertex: {corner} → {moved_corner}");
    assert!((moved_corner.y - corner.y).abs() < 1e-3 && (moved_corner.z - corner.z).abs() < 1e-3);
    app.run(Command::Undo);
    hands.key(&mut app, egui::Key::Escape);
    assert!(app.mesh_edit_state().is_none());

    // Convert to Cubes: коробка становится кубоидом с той же текстурой.
    app.session_mut().select(Some(id));
    app.run(Command::ConvertToCubes);
    let cubes: Vec<_> = app.document().elements().filter(|(_, e)| e.shape == Shape::Cuboid).collect();
    assert_eq!(cubes.len(), 1, "{:?}", app.status_message());
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn edit_a_copy_imports_the_mesh_editably_into_the_target_model() {
    let dir = obj_dir("copy");
    // Так делает редактор движка: пустая .pmodel рядом, затем `--reference crate.obj`.
    let target = dir.join("crate_copy.pmodel");
    engine_modeler::format::save(&Document::new("crate_copy"), &target).unwrap();
    let mut app = ModelerApp::new();
    app.open(&target).unwrap();
    app.run(Command::ImportMesh(dir.join("crate.obj")));
    app.run(Command::Save);
    let saved = engine_modeler::format::load(&target).unwrap();
    assert!(saved.references.is_empty(), "no grey reference any more");
    let (_, element) = saved.elements().next().expect("an editable element");
    assert_eq!(element.shape, Shape::Mesh);
    // Движок читает раздел `baked`: 6 квадратов = 12 треугольников.
    let text = std::fs::read_to_string(&target).unwrap();
    let baked = engine_modeler::format::read_baked(&text).unwrap();
    assert_eq!(baked.indices.len(), 36);
    let _ = std::fs::remove_dir_all(dir);
}
