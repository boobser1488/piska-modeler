//! Дефекты инструментов моделирования, найденные по снимкам, и их проверка клавишами и
//! мышью (egui без окна): сообщение в строке состояния не живёт дольше своего действия;
//! панель последней операции — только для последней операции; подсказка ножа не дублируется;
//! петлевой разрез показывает предпросмотр новых петель; обводка элемента не просвечивает
//! сквозь модель; кадр вида не больше текстуры egui.
#![cfg(feature = "ui")]

use egui::{Key, Modifiers, Pos2};
use engine_modeler::mesh_ops::{self, EditMesh, Primitive};
use engine_modeler::ui::{Command, MeshCmd, Mode, ModelerApp};
use engine_modeler::{NodeId, ViewPreset};
use glam::Vec3;

struct Hands {
    ctx: egui::Context,
    time: f64,
    modifiers: Modifiers,
    screen: egui::Vec2,
    max_texture_side: Option<usize>,
}

impl Hands {
    fn new(app: &mut ModelerApp) -> Self {
        Self::with(app, egui::vec2(1400.0, 860.0), None)
    }

    fn with(app: &mut ModelerApp, screen: egui::Vec2, max_texture_side: Option<usize>) -> Self {
        let mut hands = Self { ctx: egui::Context::default(), time: 0.0, modifiers: Modifiers::NONE, screen, max_texture_side };
        app.autosave_seconds = 0.0;
        engine_modeler::ui::kit::apply(&hands.ctx, true);
        for _ in 0..3 {
            hands.frame(app, vec![]);
        }
        hands
    }

    fn frame(&mut self, app: &mut ModelerApp, events: Vec<egui::Event>) {
        let input = egui::RawInput {
            screen_rect: Some(egui::Rect::from_min_size(Pos2::ZERO, self.screen)),
            time: Some(self.time),
            predicted_dt: 1.0 / 60.0,
            modifiers: self.modifiers,
            max_texture_side: self.max_texture_side,
            events,
            ..Default::default()
        };
        self.time += 1.0 / 60.0;
        let _ = self.ctx.run(input, |ctx| {
            egui::CentralPanel::default().frame(egui::Frame::none()).show(ctx, |ui| app.show(ui));
        });
    }

    fn move_to(&mut self, app: &mut ModelerApp, pos: Pos2) {
        self.frame(app, vec![egui::Event::PointerMoved(pos)]);
        self.frame(app, vec![]);
    }

    fn click(&mut self, app: &mut ModelerApp, pos: Pos2) {
        let button = |pressed, modifiers| egui::Event::PointerButton { pos, button: egui::PointerButton::Primary, pressed, modifiers };
        self.frame(app, vec![egui::Event::PointerMoved(pos)]);
        let m = self.modifiers;
        self.frame(app, vec![button(true, m)]);
        self.frame(app, vec![button(false, m)]);
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

}

fn mesh(app: &ModelerApp, id: NodeId) -> EditMesh {
    EditMesh::of(app.document().element(id).unwrap()).unwrap()
}

fn world(app: &ModelerApp, id: NodeId) -> Vec<Vec3> {
    let w = app.document().world_matrix(id);
    mesh(app, id).points.iter().map(|p| w.transform_point3(*p)).collect()
}

/// Куб 12 мешем в режиме правки рёбер, вид спереди сверху.
fn cube_in_edge_mode() -> (ModelerApp, Hands, NodeId) {
    let mut app = ModelerApp::new();
    let mut hands = Hands::new(&mut app);
    app.run(Command::Mesh(MeshCmd::Add(Primitive::Cube { size: 12.0 })));
    let id = app.session().primary().expect("the cube is selected");
    app.run(Command::Frame);
    hands.frame(&mut app, vec![]);
    let center = app.viewport_rect().unwrap().center();
    hands.move_to(&mut app, center);
    hands.key(&mut app, Key::V, Modifiers::NONE);
    hands.key(&mut app, Key::Num2, Modifiers::NONE);
    assert!(app.mesh_selection().is_some(), "V starts the mesh mode");
    (app, hands, id)
}

/// Середина видимого ребра куба, ближайшего к камере, оба конца которого удовлетворяют `pick`.
fn near_edge(app: &ModelerApp, id: NodeId, pick: impl Fn(Vec3) -> bool) -> (Pos2, (u32, u32)) {
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
    (app.world_to_screen((points[a as usize] + points[b as usize]) * 0.5).unwrap(), (a, b))
}

fn top_front_edge(app: &ModelerApp, id: NodeId) -> (Pos2, (u32, u32)) {
    let top = world(app, id).iter().map(|p| p.y).fold(f32::MIN, f32::max);
    let front = world(app, id).iter().map(|p| p.z).fold(f32::MIN, f32::max);
    near_edge(app, id, |p| (p.y - top).abs() < 1e-3 && (p.z - front).abs() < 1e-3)
}

#[test]
fn a_status_message_ends_with_the_action_it_belongs_to() {
    let mut app = ModelerApp::new();
    let mut hands = Hands::new(&mut app);
    app.run(Command::Mesh(MeshCmd::Add(Primitive::Cube { size: 12.0 })));
    hands.frame(&mut app, vec![]);
    let center = app.viewport_rect().unwrap().center();
    hands.move_to(&mut app, center);
    hands.key(&mut app, Key::V, Modifiers::NONE);
    let text = app.status_message().expect("entering the mesh mode says what to do").to_string();
    assert!(text.starts_with("Vertex mode"), "{text}");
    // Следующее действие (режим граней) — сообщение исчезает.
    hands.key(&mut app, Key::Num3, Modifiers::NONE);
    assert_eq!(app.status_message(), None, "the old hint is gone after the next action");
    // «Scale cancelled» тоже не висит после следующего действия.
    hands.key(&mut app, Key::A, Modifiers::NONE);
    hands.move_to(&mut app, center);
    hands.key(&mut app, Key::S, Modifiers::NONE);
    hands.key(&mut app, Key::Escape, Modifiers::NONE);
    let cancelled = app.status_message().map(str::to_string);
    assert!(cancelled.as_deref().is_some_and(|t| t.contains("cancelled")), "{cancelled:?}");
    hands.key(&mut app, Key::Num2, Modifiers::NONE);
    assert_eq!(app.status_message(), None, "`Scale cancelled` is gone after the next action");
    // Новое сообщение заменяет старое.
    app.run(Command::Undo);
    hands.frame(&mut app, vec![]);
    app.run(Command::Redo);
    hands.frame(&mut app, vec![]);
    assert!(app.status_message().is_none_or(|t| !t.contains("cancelled")));
}

#[test]
fn the_last_operation_panel_belongs_to_the_last_operation_only() {
    let (mut app, mut hands, id) = cube_in_edge_mode();
    let (at, _) = top_front_edge(&app, id);
    hands.move_to(&mut app, at);
    hands.key(&mut app, Key::R, Modifiers::COMMAND);
    assert_eq!(app.last_operation().map(|l| l.label), Some("Loop Cut"));
    assert!(app.last_operation_panel_visible(), "the panel shows right after the operation");
    // Нож — другой инструмент: панель прошлой операции исчезает и не возвращается после Esc.
    hands.key(&mut app, Key::K, Modifiers::NONE);
    assert!(!app.last_operation_panel_visible(), "the knife is active: no panel of the loop cut");
    assert!(app.last_operation().is_none(), "starting a tool forgets the previous operation");
    hands.key(&mut app, Key::Escape, Modifiers::NONE);
    assert!(!app.last_operation_panel_visible());
    // Другой режим: панель только в режиме Edit.
    let (mut app, mut hands, id) = cube_in_edge_mode();
    let (at, _) = top_front_edge(&app, id);
    hands.move_to(&mut app, at);
    hands.key(&mut app, Key::R, Modifiers::COMMAND);
    assert!(app.last_operation_panel_visible());
    app.run(Command::SetMode(Mode::Paint));
    hands.frame(&mut app, vec![]);
    assert!(!app.last_operation_panel_visible(), "not in Paint mode");
}

#[test]
fn the_knife_hint_is_shown_once() {
    let (mut app, mut hands, _) = cube_in_edge_mode();
    hands.key(&mut app, Key::K, Modifiers::NONE);
    assert!(app.mesh_edit_state().is_some());
    assert_eq!(app.status_message(), None, "the hint lives in the view overlay; the status line does not repeat it");
}

#[test]
fn loop_cut_preview_follows_the_cuts_and_matches_the_real_cut() {
    let (mut app, mut hands, id) = cube_in_edge_mode();
    let (at, _) = top_front_edge(&app, id);
    // Ничего не нажато: предпросмотра нет.
    hands.move_to(&mut app, at);
    assert!(app.loop_cut_preview().is_empty());
    // Ctrl над ребром — предпросмотр одной петли поперёк кольца (4 грани куба кольцом).
    hands.modifiers = Modifiers::COMMAND;
    hands.move_to(&mut app, at);
    let one = app.loop_cut_preview();
    assert_eq!(one.len(), 4, "a ring of four quads, one cut across each");
    hands.modifiers = Modifiers::NONE;
    // Настоящий разрез строит ровно те отрезки.
    let before = app.document().clone();
    hands.move_to(&mut app, at);
    hands.key(&mut app, Key::R, Modifiers::COMMAND);
    let m = mesh(&app, id);
    let points = world(&app, id);
    let new_edges: Vec<(Vec3, Vec3)> = m
        .all_edges()
        .into_iter()
        .filter(|(a, b)| a.max(b) >= &8)
        .map(|(a, b)| (points[a as usize], points[b as usize]))
        .collect();
    for (p, q) in &one {
        let found = new_edges.iter().any(|(a, b)| (a.distance(*p) < 1e-3 && b.distance(*q) < 1e-3) || (a.distance(*q) < 1e-3 && b.distance(*p) < 1e-3));
        assert!(found, "the preview segment {p:?}-{q:?} is an edge of the real cut");
    }
    assert!(app.session_mut().undo());
    assert_eq!(app.document(), &before);
    // Режим петлевого разреза: Ctrl+R без ребра под курсором, + добавляет разрезы.
    let away = egui::pos2(app.viewport_rect().unwrap().min.x + 30.0, app.viewport_rect().unwrap().max.y - 30.0);
    hands.move_to(&mut app, away);
    app.run(Command::Mesh(MeshCmd::LoopCut));
    hands.frame(&mut app, vec![]);
    assert_eq!(app.loop_cut_mode(), Some((1, 0.5)));
    hands.key(&mut app, Key::Plus, Modifiers::NONE);
    assert_eq!(app.loop_cut_mode().map(|m| m.0), Some(2));
    hands.move_to(&mut app, at);
    assert_eq!(app.loop_cut_preview().len(), 8, "two cuts across each of the four faces");
    let faces = mesh(&app, id).faces.len();
    hands.click(&mut app, at);
    assert_eq!(app.loop_cut_mode(), None);
    assert_eq!(mesh(&app, id).faces.len(), faces + 8, "two loops of four quads each");
    assert_eq!(app.last_operation().map(|l| l.label), Some("Loop Cut"), "the panel can still adjust cuts and slide");
}

#[test]
fn the_outline_of_a_hovered_element_does_not_show_through_the_surface() {
    let mut app = ModelerApp::new();
    let mut hands = Hands::new(&mut app);
    app.run(Command::Mesh(MeshCmd::Add(Primitive::Cube { size: 12.0 })));
    let id = app.session().primary().unwrap();
    // Чаша: верхняя грань вставкой и выдавливанием внутрь.
    app.session_mut().edit("Bed", |doc| {
        mesh_ops::apply(doc, id, |m| {
            let top = (0..m.faces.len()).find(|f| m.normal(*f).y > 0.9).unwrap();
            let faces = std::collections::BTreeSet::from([top]);
            let (inner, _) = mesh_ops::inset(m, &faces, 2.0);
            mesh_ops::extrude_faces(m, &inner, -8.0);
        })
        .unwrap();
    });
    app.run(Command::View(ViewPreset::Front));
    app.camera_mut().orbit(0.6, -0.5);
    app.run(Command::Frame);
    hands.frame(&mut app, vec![]);
    let outline = app.visible_outline(id);
    assert!(!outline.is_empty());
    // Независимая проверка: ни у одного куска нет поверхности заметно ближе к камере.
    let built = engine_modeler::build_mesh(app.document(), Default::default());
    let rect = app.viewport_rect().unwrap();
    let eye = app.camera().eye();
    let mut hidden = 0;
    for (a, b) in &outline {
        let mid = (*a + *b) * 0.5;
        let screen = app.world_to_screen(mid).unwrap();
        let ndc = glam::Vec2::new((screen.x - rect.min.x) / rect.width() * 2.0 - 1.0, 1.0 - (screen.y - rect.min.y) / rect.height() * 2.0);
        let ray = app.camera().ray(ndc, rect.width() / rect.height());
        if let Some(hit) = engine_modeler::camera::raycast(&built, ray, |_| true) {
            if hit.point.distance(eye) < mid.distance(eye) - 0.5 {
                hidden += 1;
            }
        }
    }
    assert_eq!(hidden, 0, "no outline piece is behind a surface");
    // И с другой стороны: всё, что скрыто, действительно выброшено — отрезков меньше, чем рёбер.
    let all: usize = {
        let m = mesh(&app, id);
        m.all_edges().len() * 4
    };
    assert!(outline.len() < all, "hidden edges are dropped: {} of {all} pieces", outline.len());
}

#[test]
fn the_software_view_never_asks_for_a_texture_bigger_than_egui_takes() {
    // Окно в тысячи точек (как первый кадр окна без размера) и предел текстуры 1024.
    let mut app = ModelerApp::new();
    app.run(Command::Mesh(MeshCmd::Add(Primitive::Cube { size: 12.0 })));
    let _hands = Hands::with(&mut app, egui::vec2(4000.0, 3000.0), Some(1024));
    let [w, h] = app.viewport_frame_size();
    assert!(w <= 1024 && h <= 1024 && w > 0 && h > 0, "{w}x{h}");
    // Отношение сторон сохраняется.
    let rect = app.viewport_rect().unwrap();
    let want = rect.width() / rect.height();
    assert!(((w as f32 / h as f32) - want).abs() < 0.05, "aspect {} vs {want}", w as f32 / h as f32);
}
