//! Снимки окна с инструментами моделирования: английский интерфейс в масштабе 100 % и
//! русский в 150 %, и «карусель» кувшина и тележки, собранных клавишами. Только по
//! запросу: `MODELER_SHOTS=<каталог>` (окна нет, кадр рисует видеокарта без окна; без
//! переменной тест ничего не делает).

use egui::{vec2, Key, Modifiers, Pos2};
use engine_modeler::camera::ViewPreset;
use engine_modeler::mesh_ops::Primitive;
use engine_modeler::ui::{Command, MeshCmd, ModelerApp, UvCmd};
use glam::Vec3;
use piska_modeler::Headless;

struct Shots {
    headless: Headless,
    dir: std::path::PathBuf,
    tag: String,
    pointer: Pos2,
    size: [u32; 2],
}

impl Shots {
    fn frames(&mut self, app: &mut ModelerApp, events: Vec<egui::Event>) {
        self.headless.frame(app, events);
        for _ in 0..2 {
            self.headless.frame(app, vec![egui::Event::PointerMoved(self.pointer)]);
        }
    }

    fn key(&mut self, app: &mut ModelerApp, key: Key, modifiers: Modifiers) {
        let event = |pressed| egui::Event::Key { key, physical_key: None, pressed, repeat: false, modifiers };
        self.frames(app, vec![event(true), event(false)]);
    }

    fn point(&mut self, app: &mut ModelerApp, at: Pos2) {
        self.pointer = at;
        self.frames(app, vec![egui::Event::PointerMoved(at)]);
    }

    fn click(&mut self, app: &mut ModelerApp, at: Pos2) {
        self.point(app, at);
        let button = |pressed| egui::Event::PointerButton { pos: at, button: egui::PointerButton::Primary, pressed, modifiers: Modifiers::NONE };
        self.headless.frame(app, vec![button(true)]);
        self.frames(app, vec![button(false)]);
    }

    fn typed(&mut self, app: &mut ModelerApp, text: &str) {
        self.frames(app, vec![egui::Event::Text(text.to_string())]);
    }

    fn shot(&mut self, app: &mut ModelerApp, name: &str) {
        self.frames(app, vec![]);
        let path = self.dir.join(format!("{name}_{}.png", self.tag));
        self.headless.image().save(&path).unwrap();
    }

    fn screen(&self, app: &ModelerApp, p: Vec3) -> Pos2 {
        app.world_to_screen(p).unwrap_or(self.pointer)
    }
}

/// Четыре вида модели по кругу, уменьшенные в одну картинку.
fn turntable(shots: &mut Shots, app: &mut ModelerApp, name: &str) {
    let [w, h] = shots.size;
    let mut sheet = image::RgbaImage::new(w * 2, h * 2);
    // Без выделения: ни подсветки рёбер, ни стрелок поверх модели.
    app.run(Command::SelectNone);
    for k in 0..4u32 {
        app.run(Command::View(ViewPreset::Front));
        app.camera_mut().orbit(std::f32::consts::FRAC_PI_2 * k as f32 + 0.5, -0.35);
        app.run(Command::Frame);
        shots.frames(app, vec![]);
        let image = shots.headless.image();
        image::imageops::replace(&mut sheet, &image, ((k % 2) * w) as i64, ((k / 2) * h) as i64);
    }
    let small = image::imageops::resize(&sheet, w, h, image::imageops::FilterType::Triangle);
    small.save(shots.dir.join(format!("turntable_{name}_{}.png", shots.tag))).unwrap();
}

fn new_app(shots: &mut Shots) -> ModelerApp {
    let mut app = ModelerApp::new();
    app.autosave_seconds = 0.0;
    shots.headless.attach(&mut app);
    shots.frames(&mut app, vec![]);
    app
}

#[test]
fn modelling_screenshots() {
    let Ok(dir) = std::env::var("MODELER_SHOTS") else {
        eprintln!("MODELER_SHOTS is not set: skipped");
        return;
    };
    let dir = std::path::PathBuf::from(dir);
    std::fs::create_dir_all(&dir).unwrap();
    for (tag, language, size, scale) in [
        ("en100", engine_i18n::Language::English, [1280u32, 800u32], 1.0f32),
        ("ru150", engine_i18n::Language::Russian, [1920, 1200], 1.5),
    ] {
        engine_i18n::set(language);
        let Ok(headless) = Headless::new(size, scale, true) else {
            eprintln!("no GPU: skipped");
            return;
        };
        let mut shots = Shots { headless, dir: dir.clone(), tag: tag.to_string(), pointer: Pos2::new(400.0, 300.0), size };
        let n = Modifiers::NONE;

        // Кувшин: меню Shift+A, вращение профиля, панель последней операции.
        let mut app = new_app(&mut shots);
        let center = app.viewport_rect().unwrap().center();
        shots.point(&mut app, center);
        shots.key(&mut app, Key::A, Modifiers::SHIFT);
        shots.shot(&mut app, "01_add_mesh_menu");
        shots.key(&mut app, Key::Escape, n);
        app.run(Command::Mesh(MeshCmd::AddLathe));
        app.run(Command::View(ViewPreset::Front));
        app.run(Command::Frame);
        shots.shot(&mut app, "02_lathe_last_operation_panel");
        shots.key(&mut app, Key::V, n);
        shots.key(&mut app, Key::Num2, n);
        shots.key(&mut app, Key::A, n);
        shots.shot(&mut app, "03_edge_mode_select_all");
        let pivot = shots.screen(&app, Vec3::new(0.0, 4.5, 0.0));
        shots.point(&mut app, pivot + vec2(80.0, 0.0));
        shots.key(&mut app, Key::S, n);
        shots.key(&mut app, Key::X, n);
        shots.point(&mut app, pivot + vec2(110.0, 0.0));
        shots.shot(&mut app, "04_scale_along_x");
        shots.key(&mut app, Key::Escape, n);
        app.run(Command::View(ViewPreset::Top));
        app.run(Command::Frame);
        shots.key(&mut app, Key::Num3, n);
        let top = shots.screen(&app, Vec3::new(0.0, 9.0, 0.0));
        shots.click(&mut app, top);
        shots.shot(&mut app, "05_face_select");
        shots.key(&mut app, Key::I, n);
        shots.point(&mut app, top + vec2(40.0, 0.0));
        shots.shot(&mut app, "06_inset_drag");
        shots.key(&mut app, Key::Enter, n);
        shots.key(&mut app, Key::E, n);
        shots.typed(&mut app, "-6");
        shots.shot(&mut app, "07_extrude_typed");
        shots.key(&mut app, Key::Enter, n);
        app.run(Command::View(ViewPreset::Front));
        app.camera_mut().orbit(0.5, -0.45);
        shots.shot(&mut app, "08_jar_extruded");
        shots.key(&mut app, Key::Escape, n);
        turntable(&mut shots, &mut app, "jar");

        // Тележка: куб, вставка и выдавливание, петлевой разрез, нож, колесо.
        let mut app = new_app(&mut shots);
        app.run(Command::Mesh(MeshCmd::Add(Primitive::Cube { size: 12.0 })));
        app.run(Command::View(ViewPreset::Top));
        app.run(Command::Frame);
        shots.key(&mut app, Key::V, n);
        shots.key(&mut app, Key::Num3, n);
        let top = shots.screen(&app, Vec3::new(0.0, 12.0, 0.0));
        shots.click(&mut app, top);
        shots.key(&mut app, Key::I, n);
        shots.typed(&mut app, "1");
        shots.key(&mut app, Key::Enter, n);
        shots.key(&mut app, Key::E, n);
        shots.typed(&mut app, "-8");
        shots.key(&mut app, Key::Enter, n);
        app.run(Command::View(ViewPreset::Front));
        app.run(Command::Frame);
        shots.key(&mut app, Key::Num2, n);
        let edge = shots.screen(&app, Vec3::new(2.0, 12.0, 6.0));
        shots.point(&mut app, edge);
        shots.shot(&mut app, "09_loop_cut_hover");
        shots.key(&mut app, Key::R, Modifiers::COMMAND);
        shots.shot(&mut app, "10_loop_cut_panel");
        shots.key(&mut app, Key::K, n);
        let a = shots.screen(&app, Vec3::new(-6.0, 3.0, 6.0));
        shots.click(&mut app, a);
        let b = shots.screen(&app, Vec3::new(6.0, 9.0, 6.0));
        shots.point(&mut app, b);
        shots.shot(&mut app, "11_knife_first_point");
        shots.key(&mut app, Key::Escape, n);
        shots.key(&mut app, Key::Escape, n);
        app.run(Command::Mesh(MeshCmd::Add(Primitive::Cylinder { radius: 3.0, height: 2.0, segments: 12, caps: true })));
        shots.key(&mut app, Key::V, n);
        shots.key(&mut app, Key::A, n);
        shots.key(&mut app, Key::R, n);
        shots.key(&mut app, Key::X, n);
        shots.typed(&mut app, "90");
        shots.key(&mut app, Key::Enter, n);
        shots.key(&mut app, Key::G, n);
        shots.key(&mut app, Key::Z, n);
        shots.typed(&mut app, "7");
        shots.shot(&mut app, "12_move_typed_z");
        shots.key(&mut app, Key::Enter, n);
        shots.key(&mut app, Key::Escape, n);
        turntable(&mut shots, &mut app, "cart");
        app.run(Command::ShowShortcuts);
        shots.shot(&mut app, "13_shortcuts");
    }
    engine_i18n::set(engine_i18n::Language::English);
}

/// Прямоугольник UV грани меша: (x0, y0, x1, y1).
fn uv_rect(app: &ModelerApp, id: engine_modeler::NodeId, f: usize) -> [f32; 4] {
    let uvs = &app.document().element(id).unwrap().mesh.as_ref().unwrap().faces[f].uvs;
    let min = |k: usize| uvs.iter().map(|u| u[k]).fold(f32::INFINITY, f32::min);
    let max = |k: usize| uvs.iter().map(|u| u[k]).fold(f32::NEG_INFINITY, f32::max);
    [min(0), min(1), max(0), max(1)]
}

fn uv_point(app: &ModelerApp, id: engine_modeler::NodeId, f: usize) -> Pos2 {
    let [x0, y0, x1, y1] = uv_rect(app, id, f);
    app.uv_to_screen([(x0 + x1) / 2.0, (y0 + y1) / 2.0]).expect("the UV editor is open")
}

/// Снимки редактора UV в работе и исправленных дефектов. Только по запросу `MODELER_SHOTS`.
#[test]
fn uv_editor_and_defect_screenshots() {
    let Ok(dir) = std::env::var("MODELER_SHOTS") else {
        eprintln!("MODELER_SHOTS is not set: skipped");
        return;
    };
    let dir = std::path::PathBuf::from(dir);
    std::fs::create_dir_all(&dir).unwrap();
    for (tag, language, size, scale) in [
        ("en100", engine_i18n::Language::English, [1280u32, 800u32], 1.0f32),
        ("ru150", engine_i18n::Language::Russian, [1920, 1200], 1.5),
    ] {
        engine_i18n::set(language);
        let Ok(headless) = Headless::new(size, scale, true) else {
            eprintln!("no GPU: skipped");
            return;
        };
        let mut shots = Shots { headless, dir: dir.clone(), tag: tag.to_string(), pointer: Pos2::new(400.0, 300.0), size };
        let n = Modifiers::NONE;

        // Куб-меш, развёрнутый по граням; каждая грань своего цвета, чтобы острова были видны.
        let mut app = new_app(&mut shots);
        app.run(Command::Mesh(MeshCmd::Add(Primitive::Cube { size: 12.0 })));
        let id = app.session().primary().unwrap();
        app.run(Command::Frame);
        app.camera_mut().orbit(0.55, -0.45);
        let center = app.viewport_rect().unwrap().center();
        shots.point(&mut app, center);
        shots.key(&mut app, Key::U, n);
        app.run(Command::Uv(UvCmd::ByFace));
        let colors: [[u8; 4]; 6] = [[198, 96, 84, 255], [96, 160, 110, 255], [88, 130, 196, 255], [214, 184, 90, 255], [160, 110, 190, 255], [90, 180, 190, 255]];
        let rects: Vec<[f32; 4]> = (0..6).map(|f| uv_rect(&app, id, f)).collect();
        app.session_mut().edit("Colour the faces", |doc| {
            for (r, c) in rects.iter().zip(colors) {
                for y in r[1] as i32..r[3] as i32 {
                    for x in r[0] as i32..r[2] as i32 {
                        // Узор в два тона, чтобы было видно, как лежат тексели.
                        let k = if (x + y) % 4 == 0 { 0.8 } else { 1.0 };
                        doc.textures[0].image.set(x, y, [(c[0] as f32 * k) as u8, (c[1] as f32 * k) as u8, (c[2] as f32 * k) as u8, 255]);
                    }
                }
            }
        });
        shots.shot(&mut app, "uv01_editor_overview");

        // Выбор: рамка вокруг двух граней — подсвечены и здесь, и в 3D.
        let (a, b) = (uv_rect(&app, id, 4), uv_rect(&app, id, 5));
        let from = app.uv_to_screen([a[0] - 1.0, a[1] - 1.0]).unwrap();
        let to = app.uv_to_screen([b[2] + 1.0, b[3] + 1.0]).unwrap();
        shots.point(&mut app, from);
        let button = |pos, pressed| egui::Event::PointerButton { pos, button: egui::PointerButton::Primary, pressed, modifiers: n };
        shots.headless.frame(&mut app, vec![button(from, true)]);
        shots.headless.frame(&mut app, vec![egui::Event::PointerMoved(from + (to - from) * 0.5)]);
        shots.headless.frame(&mut app, vec![egui::Event::PointerMoved(to)]);
        shots.frames(&mut app, vec![button(to, false)]);
        shots.shot(&mut app, "uv02_box_select_two_faces_both_views");

        // G с числом: сдвиг на 2 текселя вправо (показание в углу холста).
        let at = uv_point(&app, id, 4);
        shots.point(&mut app, at);
        shots.key(&mut app, Key::G, n);
        shots.typed(&mut app, "2");
        shots.shot(&mut app, "uv03_move_typed_value");
        shots.key(&mut app, Key::Enter, n);

        // Меню UV, подменю «Развернуть».
        let button_rect = app.uv_menu_buttons()[1];
        shots.click(&mut app, button_rect.center());
        let unwrap_item = Pos2::new(button_rect.min.x + 40.0, button_rect.max.y + 6.0 + 22.0 * 1.5);
        shots.point(&mut app, unwrap_item);
        shots.shot(&mut app, "uv04_uv_menu_unwrap");
        shots.click(&mut app, button_rect.center());
        // Укладка и развёртка — по всему мешу: выбор снят (Alt+A над холстом).
        let canvas_centre = app.uv_canvas().unwrap().center();
        shots.point(&mut app, canvas_centre);
        shots.key(&mut app, Key::A, Modifiers::ALT);

        // Укладка с зазором 2, затем развёртка с 32 пикселями на метр.
        app.uv_pack_mut().margin = 2;
        app.run(Command::Uv(UvCmd::Pack));
        shots.shot(&mut app, "uv05_packed_margin_2");
        app.set_uv_pixels_per_metre(32.0);
        app.run(Command::Uv(UvCmd::ByFace));
        shots.shot(&mut app, "uv06_unwrap_32_px_per_metre");

        // Крупный масштаб: сетка текселей.
        let at = uv_point(&app, id, 0);
        shots.point(&mut app, at);
        for _ in 0..3 {
            shots.frames(&mut app, vec![egui::Event::MouseWheel { unit: egui::MouseWheelUnit::Point, delta: vec2(0.0, 220.0), modifiers: n }]);
        }
        shots.shot(&mut app, "uv07_zoomed_texel_grid");

        // Кисть в 3D-виде по перенесённому UV.
        shots.key(&mut app, Key::Tab, n);
        shots.key(&mut app, Key::B, n);
        app.paint.color = [255, 255, 255, 255];
        let target = app.world_to_screen(Vec3::new(6.0, 12.0, 6.0)).unwrap_or(shots.pointer);
        shots.click(&mut app, target);
        shots.shot(&mut app, "uv08_paint_after_uv_edit");

        // Дефекты. Тележка: куб, вставка и выдавливание вглубь.
        let mut app = new_app(&mut shots);
        app.run(Command::Mesh(MeshCmd::Add(Primitive::Cube { size: 12.0 })));
        app.run(Command::View(ViewPreset::Top));
        app.run(Command::Frame);
        shots.key(&mut app, Key::V, n);
        shots.key(&mut app, Key::Num3, n);
        let top = shots.screen(&app, Vec3::new(0.0, 12.0, 0.0));
        shots.click(&mut app, top);
        shots.key(&mut app, Key::I, n);
        shots.typed(&mut app, "1");
        shots.key(&mut app, Key::Enter, n);
        shots.key(&mut app, Key::E, n);
        shots.typed(&mut app, "-8");
        shots.key(&mut app, Key::Enter, n);
        app.run(Command::View(ViewPreset::Front));
        app.camera_mut().orbit(0.5, -0.5);
        app.run(Command::Frame);
        shots.key(&mut app, Key::Num2, n);
        shots.key(&mut app, Key::A, Modifiers::ALT);
        // Предпросмотр петлевого разреза: Ctrl+R вдали от рёбер включает режим, «+» — два разреза,
        // курсор над ребром — две новые петли поперёк кольца.
        let corner = app.viewport_rect().unwrap().left_bottom() + vec2(60.0, -60.0);
        shots.point(&mut app, corner);
        app.run(Command::Mesh(MeshCmd::LoopCut));
        shots.key(&mut app, Key::Plus, n);
        let edge = shots.screen(&app, Vec3::new(0.0, 12.0, 6.0));
        shots.point(&mut app, edge);
        shots.shot(&mut app, "uv09_loop_cut_preview_two_cuts");
        shots.key(&mut app, Key::Escape, n);
        // Нож: подсказка одна (внизу вида), строка состояния не повторяет её.
        shots.key(&mut app, Key::K, n);
        shots.shot(&mut app, "uv10_knife_hint_once");
        shots.key(&mut app, Key::Escape, n);
        // Выход из режима меша: обводка элемента под курсором не показывает рёбра за стенками.
        shots.key(&mut app, Key::Escape, n);
        app.run(Command::SelectNone);
        let over = shots.screen(&app, Vec3::new(0.0, 6.0, 6.0));
        shots.point(&mut app, over);
        shots.shot(&mut app, "uv11_hover_outline_no_see_through");
    }
    engine_i18n::set(engine_i18n::Language::English);
}
