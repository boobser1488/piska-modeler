//! Окно моделей мышью, без GPU: кубоид с панели, ручка размера, штрих кисти по модели,
//! отмена.
#![cfg(feature = "ui")]

use egui::{vec2, Pos2};
use engine_modeler::ui::{Command, Mode, ModelerApp, Tool};
use engine_modeler::{FaceDir, ViewPreset};

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

    fn key(&mut self, app: &mut ModelerApp, key: egui::Key, modifiers: egui::Modifiers) {
        self.frame(
            app,
            vec![egui::Event::Key { key, physical_key: None, pressed: true, repeat: false, modifiers }],
        );
        self.frame(app, vec![]);
    }

    fn center(&self, name: &str) -> Pos2 {
        self.ctx
            .read_response(ModelerApp::button_id(name))
            .unwrap_or_else(|| panic!("button `{name}` is drawn"))
            .rect
            .center()
    }
}

#[test]
fn cuboid_resize_paint_and_undo_with_the_mouse() {
    let mut app = ModelerApp::new();
    let mut hands = Hands::new(&mut app);

    // Кубоид с панели.
    let add = hands.center("add.cuboid");
    hands.click(&mut app, add);
    assert_eq!(app.document().element_count(), 1, "the toolbar button adds a cuboid");
    let id = app.session().primary().expect("the new cuboid is selected");
    let before = app.document().element(id).unwrap().clone();
    assert_eq!(before.size, glam::Vec3::splat(8.0));

    // Ручка верхней стороны вверх: растёт вверх, низ стоит.
    let tool = hands.center("tool.resize");
    hands.click(&mut app, tool);
    assert_eq!(app.tool(), Tool::Resize);
    hands.frame(&mut app, vec![]);
    let handle = app.resize_handle(FaceDir::Up).expect("the up handle is on screen");
    hands.drag(&mut app, handle, handle - vec2(0.0, 90.0), 8);
    let after = app.document().element(id).unwrap().clone();
    assert!(after.size.y > before.size.y + 2.0, "the box grew: {} -> {} (handle {handle:?}, viewport {:?}, history {:?}, last step {:?})", before.size.y, after.size.y, app.viewport_rect(), app.session().history_len(), app.session().undo_label());
    assert_eq!(after.size.y.fract(), 0.0, "snapped to whole pixels");
    assert_eq!(after.from.y, before.from.y, "the bottom face stays where it was");
    assert_eq!((after.size.x, after.size.z), (before.size.x, before.size.z));

    hands.key(&mut app, egui::Key::Z, egui::Modifiers::COMMAND);
    assert_eq!(app.document().element(id).unwrap(), &before, "Ctrl+Z undoes the resize");

    // Рисование: вид спереди, кисть по северной стороне.
    app.run(Command::View(ViewPreset::Front));
    app.run(Command::Frame);
    hands.key(&mut app, egui::Key::Tab, egui::Modifiers::NONE);
    let pencil = hands.center("tool.pencil");
    hands.click(&mut app, pencil);
    assert_eq!(app.mode(), Mode::Paint);
    app.paint.color = [10, 200, 30, 255];
    hands.frame(&mut app, vec![]);
    let texture_before = app.document().textures[0].image.clone();
    let element = app.document().element(id).unwrap().clone();
    // Через всю северную сторону (−Z) по горизонтали, от края до края.
    let left = app.world_to_screen(element.center() + glam::Vec3::new(3.6, 0.5, -4.0)).unwrap();
    let right = app.world_to_screen(element.center() + glam::Vec3::new(-3.6, 0.5, -4.0)).unwrap();
    hands.drag(&mut app, left, right, 12);
    let texture = &app.document().textures[0].image;
    let [x0, y0, x1, y1] = element.face(FaceDir::North).uv.texel_bounds();
    let mut painted = 0;
    for y in 0..texture.height() as i32 {
        for x in 0..texture.width() as i32 {
            if texture.get(x, y) != texture_before.get(x, y) {
                assert!(x >= x0 && x < x1 && y >= y0 && y < y1, "texel ({x}, {y}) outside the north face was changed");
                assert_eq!(texture.get(x, y), Some([10, 200, 30, 255]));
                painted += 1;
            }
        }
    }
    assert_eq!(painted, 8, "a stroke across the 8-texel face paints its whole row");
    assert_eq!(app.session().history_len().0, 2, "add + one stroke (the resize was undone)");

    hands.key(&mut app, egui::Key::Z, egui::Modifiers::COMMAND);
    assert_eq!(app.document().textures[0].image, texture_before, "undo removes the whole stroke");
}

#[test]
fn shortcuts_switch_mode_tools_and_views() {
    let mut app = ModelerApp::new();
    let mut hands = Hands::new(&mut app);
    hands.key(&mut app, egui::Key::E, egui::Modifiers::NONE);
    assert_eq!(app.tool(), Tool::Rotate);
    hands.key(&mut app, egui::Key::Tab, egui::Modifiers::NONE);
    assert_eq!(app.mode(), Mode::Paint);
    hands.key(&mut app, egui::Key::E, egui::Modifiers::NONE);
    assert_eq!(app.tool(), Tool::Eraser, "E is the eraser while painting");
    hands.key(&mut app, egui::Key::Num5, egui::Modifiers::NONE);
    assert!(app.camera().orthographic);
    let rendered = app.render_count();
    hands.frame(&mut app, vec![]);
    hands.frame(&mut app, vec![]);
    assert_eq!(app.render_count(), rendered, "an idle view is not rasterised again");
}

#[test]
fn the_view_renders_only_when_something_changes() {
    let mut app = ModelerApp::new();
    app.open_document(engine_modeler::samples::rover(), None);
    // Кадр рисует «видеокарта» хозяина: считаются запросы кадра.
    app.set_gpu_texture(Some(egui::TextureId::User(1)));
    let mut hands = Hands::new(&mut app);
    let _ = app.take_view_request();
    let rect = app.viewport_rect().unwrap();
    let center = rect.center();
    let start = app.render_count();
    hands.frame(&mut app, vec![]);
    hands.frame(&mut app, vec![]);
    assert_eq!(app.render_count(), start, "no input, no render");
    assert!(app.take_view_request().is_none());
    // Наведение (подсветка — поверх кадра) кадр не перерисовывает.
    hands.frame(&mut app, vec![egui::Event::PointerMoved(center)]);
    hands.frame(&mut app, vec![egui::Event::PointerMoved(center + vec2(5.0, 3.0))]);
    assert_eq!(app.render_count(), start, "hovering does not re-render the view");
    // Правая кнопка: нажатие — без кадра, одно движение — один кадр, отпускание — без кадра.
    let right = |pos, pressed| egui::Event::PointerButton {
        pos,
        button: egui::PointerButton::Secondary,
        pressed,
        modifiers: egui::Modifiers::NONE,
    };
    hands.frame(&mut app, vec![right(center, true)]);
    assert_eq!(app.render_count(), start);
    hands.frame(&mut app, vec![egui::Event::PointerMoved(center + vec2(40.0, 0.0))]);
    assert_eq!(app.render_count(), start + 1, "one drag event, one render");
    assert!(app.take_view_request().is_some());
    hands.frame(&mut app, vec![right(center + vec2(40.0, 0.0), false)]);
    hands.frame(&mut app, vec![]);
    assert_eq!(app.render_count(), start + 1, "releasing does not render again");
}
