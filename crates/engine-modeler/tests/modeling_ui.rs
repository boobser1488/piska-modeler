//! Моделирование только мышью и клавишами (egui без окна): кувшин вращением профиля
//! (Shift+A ▸ Lathe, петля Alt+щелчком, S — шире, I — вставка, E с вводом числа — горло
//! вглубь, кисть) и тележка из примитивов (куб, вставка и выдавливание кузова, петлевой
//! разрез Ctrl+R, колесо — цилиндр, повёрнутый R X 90 и сдвинутый G X 6). Оба сохраняются
//! в `.pmodel` и `.glb`; `.glb` читается обратно с тем же числом треугольников.
//!
//! `MODELER_PROPS_OUT=<каталог>` — положить туда `jar.pmodel` и `cart.glb` (образцы для
//! теста движка `crates/engine-assets/tests/modeled_props.rs`).
#![cfg(feature = "ui")]

use std::collections::BTreeSet;
use std::path::PathBuf;

use egui::{vec2, Key, Modifiers, Pos2};
use engine_modeler::mesh_ops::{EditMesh, SelectMode};
use engine_modeler::ui::{Command, Mode, ModelerApp};
use engine_modeler::{NodeId, Shape, ViewPreset};
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

    fn key(&mut self, app: &mut ModelerApp, key: Key, modifiers: Modifiers) {
        let old = self.modifiers;
        self.modifiers = modifiers;
        self.frame(app, vec![egui::Event::Key { key, physical_key: None, pressed: true, repeat: false, modifiers }]);
        self.frame(app, vec![egui::Event::Key { key, physical_key: None, pressed: false, repeat: false, modifiers }]);
        self.modifiers = old;
        self.frame(app, vec![]);
    }

    fn typed(&mut self, app: &mut ModelerApp, text: &str) {
        self.frame(app, vec![egui::Event::Text(text.to_string())]);
    }

    fn center(&self, name: &str) -> Pos2 {
        self.ctx.read_response(ModelerApp::button_id(name)).unwrap_or_else(|| panic!("button `{name}` is drawn")).rect.center()
    }
}

fn mesh(app: &ModelerApp, id: NodeId) -> EditMesh {
    EditMesh::of(app.document().element(id).unwrap()).unwrap()
}

fn world(app: &ModelerApp, id: NodeId) -> Vec<Vec3> {
    let w = app.document().world_matrix(id);
    mesh(app, id).points.iter().map(|p| w.transform_point3(*p)).collect()
}

fn assert_closed(m: &EditMesh, what: &str) {
    m.check().unwrap_or_else(|e| panic!("{what}: {e}"));
    assert!(m.is_closed_manifold() && m.signed_volume() > 0.0, "{what}: closed with outward faces");
}

/// Shift+A у центра вида и пункт меню.
fn add_mesh(app: &mut ModelerApp, hands: &mut Hands, entry: &str) -> NodeId {
    let center = app.viewport_rect().unwrap().center();
    hands.move_to(app, center);
    hands.key(app, Key::A, Modifiers::SHIFT);
    let at = hands.center(&format!("add_mesh.{entry}"));
    hands.click(app, at);
    let id = app.session().primary().expect("the new mesh is selected");
    assert_eq!(app.document().element(id).unwrap().shape, Shape::Mesh, "{entry}");
    id
}

/// Видимое ребро меша, оба конца которого удовлетворяют `pick`, ближайшее к камере.
fn visible_edge(app: &ModelerApp, id: NodeId, pick: impl Fn(Vec3) -> bool) -> Pos2 {
    let m = mesh(app, id);
    let points = world(app, id);
    let eye = app.camera().eye();
    let (a, b) = m
        .all_edges()
        .into_iter()
        .filter(|(a, b)| pick(points[*a as usize]) && pick(points[*b as usize]))
        .min_by(|x, y| {
            let dx = ((points[x.0 as usize] + points[x.1 as usize]) * 0.5).distance(eye);
            let dy = ((points[y.0 as usize] + points[y.1 as usize]) * 0.5).distance(eye);
            dx.total_cmp(&dy)
        })
        .expect("such an edge");
    app.world_to_screen((points[a as usize] + points[b as usize]) * 0.5).unwrap()
}

fn temp(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("modeler-props-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

/// Сохранить `.pmodel` и `.glb`, прочитать `.glb` обратно, отдать образцы движку.
fn save_and_export(app: &mut ModelerApp, name: &str) {
    let dir = temp(name);
    let pmodel = dir.join(format!("{name}.pmodel"));
    let glb = dir.join(format!("{name}.glb"));
    app.run(Command::SaveTo(pmodel.clone()));
    app.run(Command::ExportGlb(glb.clone()));
    let doc = engine_modeler::format::load(&pmodel).unwrap();
    assert_eq!(&doc, app.document(), "{name}: .pmodel round trip");
    let baked = engine_modeler::bake(&doc);
    let opened = engine_modeler::interchange::open_file(&glb).unwrap();
    let back = engine_modeler::bake(&opened.doc);
    assert_eq!(back.triangle_count(), baked.triangle_count(), "{name}: the .glb has the same triangles");
    if let Ok(out) = std::env::var("MODELER_PROPS_OUT") {
        std::fs::copy(&pmodel, PathBuf::from(&out).join(format!("{name}.pmodel"))).unwrap();
        std::fs::copy(&glb, PathBuf::from(&out).join(format!("{name}.glb"))).unwrap();
        std::fs::write(PathBuf::from(&out).join(format!("{name}.txt")), format!("{}\n", baked.triangle_count())).unwrap();
    }
    let _ = std::fs::remove_dir_all(dir);
}

fn paint_somewhere(app: &mut ModelerApp, hands: &mut Hands, id: NodeId) {
    hands.key(app, Key::Tab, Modifiers::NONE);
    assert_eq!(app.mode(), Mode::Paint);
    hands.key(app, Key::B, Modifiers::NONE);
    app.paint.color = [200, 40, 30, 255];
    let points = world(app, id);
    let c = points.iter().fold(Vec3::ZERO, |s, p| s + *p) / points.len() as f32;
    let at = app.world_to_screen(c).unwrap();
    let before: Vec<_> = app.document().textures.iter().map(|t| t.image.clone()).collect();
    hands.frame(app, vec![egui::Event::PointerMoved(at - vec2(6.0, 0.0))]);
    let down = hands.button(at - vec2(6.0, 0.0), true);
    hands.frame(app, vec![down]);
    for k in 0..6 {
        hands.frame(app, vec![egui::Event::PointerMoved(at + vec2(-6.0 + 2.0 * k as f32, 0.0))]);
    }
    let up = hands.button(at + vec2(6.0, 0.0), false);
    hands.frame(app, vec![up]);
    let changed = app.document().textures.iter().zip(&before).any(|(t, b)| t.image != *b);
    assert!(changed, "a stroke paints the mesh (status: {:?})", app.status_message());
    hands.key(app, Key::Tab, Modifiers::NONE);
}

#[test]
fn a_jar_by_lathe_scale_inset_and_extrude() {
    let mut app = ModelerApp::new();
    let mut hands = Hands::new(&mut app);
    let id = add_mesh(&mut app, &mut hands, "Lathe");
    assert_closed(&mesh(&app, id), "lathe");
    app.run(Command::View(ViewPreset::Front));
    app.run(Command::Frame);
    hands.frame(&mut app, vec![]);

    // V — вершины, 2 — рёбра, Alt+щелчок по ребру «живота» (Y = 4) — вся петля.
    hands.key(&mut app, Key::V, Modifiers::NONE);
    assert!(app.mesh_selection().is_some(), "V enters mesh mode: {:?}", app.status_message());
    hands.key(&mut app, Key::Num2, Modifiers::NONE);
    let belly = visible_edge(&app, id, |p| (p.y - 4.0).abs() < 1e-3);
    hands.modifiers = Modifiers::ALT;
    hands.click(&mut app, belly);
    hands.modifiers = Modifiers::NONE;
    let (mode, verts, edges, _) = app.mesh_selection().unwrap();
    assert_eq!(mode, SelectMode::Edge);
    assert_eq!((verts.len(), edges.len()), (12, 12), "Alt+click selects the whole ring");

    // S: курсор от центра петли в полтора раза дальше — петля в полтора раза шире.
    let pivot = app.world_to_screen(Vec3::new(0.0, 4.0, 0.0)).unwrap();
    hands.move_to(&mut app, pivot + vec2(80.0, 0.0));
    hands.key(&mut app, Key::S, Modifiers::NONE);
    hands.move_to(&mut app, pivot + vec2(120.0, 0.0));
    hands.click(&mut app, pivot + vec2(120.0, 0.0));
    let ring: Vec<f32> = world(&app, id).iter().filter(|p| (p.y - 4.0).abs() < 1e-3).map(|p| (p.x * p.x + p.z * p.z).sqrt()).collect();
    assert!(ring.iter().all(|r| (r - 6.0).abs() < 0.2), "the belly is 1.5 times wider: {ring:?}");
    assert_closed(&mesh(&app, id), "scaled");

    // Вид сверху, 3 — грани, щелчок по крышке, I — вставка, E и «-6» Enter — горло вглубь.
    app.run(Command::View(ViewPreset::Top));
    app.run(Command::Frame);
    hands.frame(&mut app, vec![]);
    hands.key(&mut app, Key::Num3, Modifiers::NONE);
    let top = app.world_to_screen(Vec3::new(0.0, 9.0, 0.0)).unwrap();
    hands.click(&mut app, top);
    let (mode, verts, _, faces) = app.mesh_selection().unwrap();
    assert_eq!(faces.len(), 1, "the top cap is selected: mode {mode:?}, {} verts, status {:?}, click at {top:?}, viewport {:?}", verts.len(), app.status_message(), app.viewport_rect());
    let before = mesh(&app, id).faces.len();
    // Вставка мышью: дальше от точки начала — толще; щелчок применяет.
    hands.move_to(&mut app, top + vec2(4.0, 0.0));
    hands.key(&mut app, Key::I, Modifiers::NONE);
    hands.move_to(&mut app, top + vec2(60.0, 0.0));
    hands.click(&mut app, top + vec2(60.0, 0.0));
    assert_eq!(mesh(&app, id).faces.len(), before + 12, "inset adds a ring of 12 quads (status {:?})", app.status_message());
    let inset = app.last_operation().expect("the inset is the last operation").clone();
    let (_, _, _, faces) = app.mesh_selection().unwrap();
    let selected_corners: Vec<usize> = faces.iter().map(|f| mesh(&app, id).faces[*f].vertices.len()).collect();
    assert_eq!(selected_corners, [12], "after the inset the inner cap stays selected");
    hands.key(&mut app, Key::E, Modifiers::NONE);
    hands.typed(&mut app, "-6");
    hands.key(&mut app, Key::Enter, Modifiers::NONE);
    let m = mesh(&app, id);
    assert_eq!(m.faces.len(), before + 24, "extrude adds 12 walls");
    assert_closed(&m, "jar");
    let bottom_of_neck = world(&app, id).iter().filter(|p| p.y > 2.0 && p.y < 3.5).count();
    assert_eq!(bottom_of_neck, 12, "the neck goes 6 units down");
    assert_ne!(app.last_operation(), Some(&inset), "the extrude replaced it in the panel");

    // Esc — назад к элементам; кисть по боку.
    hands.key(&mut app, Key::Escape, Modifiers::NONE);
    assert!(app.mesh_selection().is_none());
    app.run(Command::View(ViewPreset::Front));
    hands.frame(&mut app, vec![]);
    paint_somewhere(&mut app, &mut hands, id);
    save_and_export(&mut app, "jar");
}

#[test]
fn a_cart_from_primitives_extrude_and_loop_cuts() {
    let mut app = ModelerApp::new();
    let mut hands = Hands::new(&mut app);
    let body = add_mesh(&mut app, &mut hands, "Cube");
    // Панель последней операции: куб 8 → 12.
    let last = app.last_operation().expect("the add is the last operation").kind.clone();
    if let engine_modeler::ui::LastKind::Primitive { parent, .. } = last {
        app.redo_last(engine_modeler::ui::LastKind::Primitive { prim: engine_modeler::mesh_ops::Primitive::Cube { size: 12.0 }, parent });
    }
    let body = app.session().primary().unwrap_or(body);
    let (lo, hi) = app.document().bounds(&[body]).unwrap();
    assert!((hi - lo - Vec3::splat(12.0)).length() < 1e-4, "the panel resized the cube: {lo} {hi}");

    app.run(Command::View(ViewPreset::Top));
    app.run(Command::Frame);
    hands.frame(&mut app, vec![]);
    hands.key(&mut app, Key::V, Modifiers::NONE);
    hands.key(&mut app, Key::Num3, Modifiers::NONE);
    let top = app.world_to_screen(Vec3::new(0.0, 12.0, 0.0)).unwrap();
    hands.click(&mut app, top);
    hands.move_to(&mut app, top + vec2(4.0, 0.0));
    hands.key(&mut app, Key::I, Modifiers::NONE);
    hands.typed(&mut app, "1");
    hands.key(&mut app, Key::Enter, Modifiers::NONE);
    hands.key(&mut app, Key::E, Modifiers::NONE);
    hands.typed(&mut app, "-8");
    hands.key(&mut app, Key::Enter, Modifiers::NONE);
    let m = mesh(&app, body);
    assert_eq!((m.points.len(), m.faces.len()), (16, 14), "a hollow bed");
    assert_closed(&m, "bed");

    // Сбоку: петлевой разрез поперёк по вертикальному ребру (курсор над ним, Ctrl+R).
    app.run(Command::View(ViewPreset::Front));
    app.run(Command::Frame);
    hands.frame(&mut app, vec![]);
    hands.key(&mut app, Key::Num2, Modifiers::NONE);
    let edge = visible_edge(&app, body, |p| p.z > 5.9 && p.x > 5.9);
    hands.move_to(&mut app, edge);
    let before = mesh(&app, body).faces.len();
    hands.key(&mut app, Key::R, Modifiers::COMMAND);
    let after = mesh(&app, body).faces.len();
    assert!(after > before, "Ctrl+R cut a loop ({before} → {after}, status {:?})", app.status_message());
    assert_closed(&mesh(&app, body), "loop cut");
    hands.key(&mut app, Key::Escape, Modifiers::NONE);

    // Колесо: цилиндр, все вершины (A), R X 90, G Z typed 7.
    let wheel = add_mesh(&mut app, &mut hands, "Cylinder");
    hands.key(&mut app, Key::V, Modifiers::NONE);
    hands.key(&mut app, Key::A, Modifiers::NONE);
    hands.key(&mut app, Key::R, Modifiers::NONE);
    hands.key(&mut app, Key::X, Modifiers::NONE);
    hands.typed(&mut app, "90");
    hands.key(&mut app, Key::Enter, Modifiers::NONE);
    hands.key(&mut app, Key::G, Modifiers::NONE);
    hands.key(&mut app, Key::Z, Modifiers::NONE);
    hands.typed(&mut app, "7");
    hands.key(&mut app, Key::Enter, Modifiers::NONE);
    let points = world(&app, wheel);
    let (lo, hi) = points.iter().fold((Vec3::splat(f32::MAX), Vec3::splat(f32::MIN)), |(a, b), p| (a.min(*p), b.max(*p)));
    assert!((hi.z - lo.z - 8.0).abs() < 0.05 && (hi.y - lo.y - 8.0).abs() < 0.05, "the wheel stands on its edge: {lo} {hi}");
    assert!(lo.z > 2.9, "and moved 7 along Z: {lo}");
    assert_closed(&mesh(&app, wheel), "wheel");
    hands.key(&mut app, Key::Escape, Modifiers::NONE);

    let ids: BTreeSet<NodeId> = app.document().elements().map(|(id, _)| id).collect();
    assert_eq!(ids.len(), 2);
    app.session_mut().select(Some(body));
    hands.frame(&mut app, vec![]);
    paint_somewhere(&mut app, &mut hands, body);
    save_and_export(&mut app, "cart");
}
