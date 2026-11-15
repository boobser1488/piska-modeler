//! Замер интерфейса Modeler без окна и GPU (копия `kit::metrics` движка, правила плотности
//! [`Rules::compact`]): экраны × английский и русский × масштаб 100 / 125 / 150 % × ширина окна.
//! Падает на тех же нарушениях, что и проверки редактора и хаба: высота контрола не 22,
//! соседи строки расходятся, промежуток не из шкалы, рамка за пределами контрола, контрол
//! вне своего контейнера, текст у самого края контрола или под чужим контролом. На наименьшей
//! ширине контролам прогоняются наведение, нажатие и фокус.
//!
//! `UI_METRICS_OUT=<файл>` — записать сводку в Markdown.
#![cfg(feature = "ui")]

use engine_modeler::ui::metrics::{self, Report, Rules};
use engine_modeler::mesh_ops::Primitive;
use engine_modeler::ui::{Command, MeshCmd, Mode, ModelerApp, PromptKind, UvCmd};
use engine_modeler::Shape;

/// Кадр окна `size` точек при масштабе `scale` (как на экране с таким DPI).
fn frame(ctx: &egui::Context, app: &mut ModelerApp, size: egui::Vec2, scale: f32, time: &mut f64, events: Vec<egui::Event>) -> Vec<egui::epaint::ClippedShape> {
    let mut input = egui::RawInput {
        screen_rect: Some(egui::Rect::from_min_size(egui::Pos2::ZERO, size)),
        time: Some(*time),
        predicted_dt: 1.0 / 60.0,
        events,
        ..Default::default()
    };
    input.viewports.entry(egui::ViewportId::ROOT).or_default().native_pixels_per_point = Some(scale);
    *time += 1.0 / 60.0;
    ctx.run(input, |ctx| {
        egui::CentralPanel::default().frame(egui::Frame::none()).show(ctx, |ui| app.show(ui));
    })
    .shapes
}

fn screens() -> Vec<(&'static str, fn(&mut ModelerApp))> {
    vec![
        ("empty", |_| {}),
        ("cuboid-selected", |app| app.run(Command::Add(Shape::Cuboid))),
        ("sphere-selected", |app| app.run(Command::Add(Shape::Sphere { segments: 12, rings: 6 }))),
        ("paint-mode", |app| {
            app.run(Command::Add(Shape::Cuboid));
            app.run(Command::SetMode(Mode::Paint));
        }),
        ("prompt-save-as", |app| app.run(Command::Ask(PromptKind::SaveAs))),
        ("prompt-import-mesh", |app| app.run(Command::Ask(PromptKind::ImportMesh))),
        ("prompt-resize-texture", |app| app.run(Command::Ask(PromptKind::ResizeTexture))),
        ("prompt-export-bbmodel", |app| app.run(Command::Ask(PromptKind::ExportBbmodel))),
        // Меню примитивов (Shift+A над видом) — события подаются ниже по имени экрана.
        ("add-menu", |_| {}),
        // Редактор UV мешей: пустой, с развёрнутым кубом и в режиме рисования.
        ("uv-editor-empty", |app| app.run(Command::ToggleUvEditor)),
        ("uv-editor-mesh", |app| {
            app.run(Command::Mesh(MeshCmd::Add(Primitive::Cube { size: 12.0 })));
            app.run(Command::ToggleUvEditor);
            app.run(Command::Uv(UvCmd::ByFace));
        }),
        ("uv-editor-mesh-selected", |app| {
            app.run(Command::Mesh(MeshCmd::Add(Primitive::Cube { size: 12.0 })));
            app.run(Command::ToggleUvEditor);
            app.run(Command::Uv(UvCmd::Box));
            app.run(Command::EditMeshVertices);
            app.run(Command::Mesh(MeshCmd::SelectAll));
        }),
        // Меню UV и Select редактора UV — щелчок по кнопке меню (подаётся ниже по имени экрана).
        ("uv-menu", |app| {
            app.run(Command::Mesh(MeshCmd::Add(Primitive::Cube { size: 12.0 })));
            app.run(Command::ToggleUvEditor);
            app.run(Command::Uv(UvCmd::ByFace));
        }),
        ("uv-menu-select", |app| {
            app.run(Command::Mesh(MeshCmd::Add(Primitive::Cube { size: 12.0 })));
            app.run(Command::ToggleUvEditor);
        }),
        ("uv-editor-paint", |app| {
            app.run(Command::Mesh(MeshCmd::Add(Primitive::Cube { size: 12.0 })));
            app.run(Command::ToggleUvEditor);
            app.run(Command::SetMode(Mode::Paint));
        }),
    ]
}

#[test]
fn modeler_screens_measure_up() {
    let mut total = Report::default();
    let (mut screens_done, mut states_done) = (0, 0);
    let mut menus = 0;
    let mut uv_screens = 0;
    let mut uv_menus = 0;
    for language in [engine_i18n::Language::English, engine_i18n::Language::Russian] {
        engine_i18n::set(language);
        for scale in [1.0_f32, 1.25, 1.5] {
            for (name, setup) in screens() {
                for width in [1100.0_f32, 1400.0, 1800.0] {
                    let mut app = ModelerApp::new();
                    app.autosave_seconds = 0.0;
                    let ctx = egui::Context::default();
                    // Тема — как в окне программы (piska-modeler ставит её до первого кадра).
                    engine_modeler::ui::kit::apply(&ctx, true);
                    let size = egui::vec2(width, 860.0) / scale;
                    let mut time = 0.0;
                    for _ in 0..3 {
                        frame(&ctx, &mut app, size, scale, &mut time, Vec::new());
                    }
                    setup(&mut app);
                    for _ in 0..3 {
                        frame(&ctx, &mut app, size, scale, &mut time, Vec::new());
                    }
                    if name == "add-menu" {
                        let at = egui::pos2(size.x * 0.45, size.y * 0.5);
                        let shift = egui::Modifiers::SHIFT;
                        let layers_before = ctx.memory(|m| m.layer_ids().count());
                        frame(&ctx, &mut app, size, scale, &mut time, vec![egui::Event::PointerMoved(at)]);
                        frame(&ctx, &mut app, size, scale, &mut time, vec![egui::Event::Key { key: egui::Key::A, physical_key: None, pressed: true, repeat: false, modifiers: shift }]);
                        frame(&ctx, &mut app, size, scale, &mut time, vec![egui::Event::Key { key: egui::Key::A, physical_key: None, pressed: false, repeat: false, modifiers: shift }]);
                        menus += usize::from(ctx.memory(|m| m.layer_ids().count()) > layers_before);
                    }
                    if name.starts_with("uv-menu") {
                        // Кнопки «Select» и «UV» — в первой строке панели над холстом.
                        let at = app.uv_menu_buttons()[usize::from(name == "uv-menu")].center();
                        let layers_before = ctx.memory(|m| m.layer_ids().count());
                        let button = |pressed| egui::Event::PointerButton { pos: at, button: egui::PointerButton::Primary, pressed, modifiers: egui::Modifiers::NONE };
                        frame(&ctx, &mut app, size, scale, &mut time, vec![egui::Event::PointerMoved(at)]);
                        frame(&ctx, &mut app, size, scale, &mut time, vec![button(true)]);
                        frame(&ctx, &mut app, size, scale, &mut time, vec![button(false)]);
                        frame(&ctx, &mut app, size, scale, &mut time, Vec::new());
                        uv_menus += usize::from(ctx.memory(|m| m.layer_ids().count()) > layers_before);
                    }
                    let shapes = frame(&ctx, &mut app, size, scale, &mut time, Vec::new());
                    if name.starts_with("uv-editor") {
                        assert!(app.uv_editor_open(), "{name}: the UV editor is open");
                        assert!(app.uv_canvas().is_some() || name == "uv-editor-empty", "{name}: the UV canvas is drawn");
                        uv_screens += 1;
                    }
                    let screen = format!("{language:?} {scale}x {width}px {name}");
                    let rules = Rules::compact();
                    total.merge(metrics::analyze_shapes(&screen, &metrics::widgets(&ctx), &shapes, rules));
                    screens_done += 1;
                    if width == 1100.0 && scale != 1.25 {
                        let (report, states) = metrics::analyze_states(&screen, &ctx, rules, |events| frame(&ctx, &mut app, size, scale, &mut time, events));
                        total.merge(report);
                        states_done += states;
                    }
                }
            }
        }
    }
    engine_i18n::set(engine_i18n::Language::English);
    let report = format!(
        "# Modeler UI metrics\n\n{screens_done} screens, {states_done} control states.\n\n## Control heights (pt)\n\n| height | count |\n|---|---|\n{}\n\n## Gaps between row neighbours (pt)\n\n| gap | count |\n|---|---|\n{}\n\n## Offenders ({})\n\n{}\n",
        Report::table(&total.heights),
        Report::table(&total.gaps),
        total.offenders.len(),
        total.offenders.iter().map(|o| format!("* {o}")).collect::<Vec<_>>().join("\n")
    );
    if let Some(out) = std::env::var_os("UI_METRICS_OUT") {
        std::fs::write(out, &report).unwrap();
    }
    assert!(screens_done >= 100, "screens measured: {screens_done}");
    assert!(menus > 0, "the Shift+A primitive menu opened in the measured screens");
    assert!(uv_screens >= 24, "the measured screens reach the UV editor: {uv_screens}");
    assert!(uv_menus >= 12, "the Select and UV menus of the UV editor opened in the measured screens: {uv_menus}");
    assert!(states_done > 100, "control states driven: {states_done}");
    assert!(total.offenders.is_empty(), "{} offenders, first ones:\n{}", total.offenders.len(), total.offenders.iter().take(25).cloned().collect::<Vec<_>>().join("\n"));
}

/// Границы каждого нарисованного текста кадра (текст, прямоугольник).
fn text_rects(shapes: &[egui::epaint::ClippedShape]) -> Vec<(String, egui::Rect)> {
    fn walk(shape: &egui::epaint::Shape, out: &mut Vec<(String, egui::Rect)>) {
        match shape {
            egui::epaint::Shape::Text(text) => {
                let rect = egui::Rect::from_min_size(text.pos, text.galley.size());
                out.push((text.galley.text().to_string(), rect));
            }
            egui::epaint::Shape::Vec(list) => list.iter().for_each(|s| walk(s, out)),
            _ => {}
        }
    }
    let mut out = Vec::new();
    for clipped in shapes {
        walk(&clipped.shape, &mut out);
    }
    out
}

/// Подпись строки свойства не налезает на соседний текст (метку оси X / Y / Z, значение) ни на
/// одном языке, масштабе и ширине окна: так выглядела «Опорная точка» на русском при 150 %.
#[test]
fn row_labels_do_not_run_into_the_next_widget_of_the_row() {
    let mut bad = Vec::new();
    for language in [engine_i18n::Language::English, engine_i18n::Language::Russian] {
        engine_i18n::set(language);
        for scale in [1.0_f32, 1.25, 1.5] {
            for width in [1100.0_f32, 1400.0, 1800.0] {
                let mut app = ModelerApp::new();
                app.autosave_seconds = 0.0;
                let ctx = egui::Context::default();
                engine_modeler::ui::kit::apply(&ctx, true);
                let size = egui::vec2(width, 860.0) / scale;
                let mut time = 0.0;
                for _ in 0..3 {
                    frame(&ctx, &mut app, size, scale, &mut time, Vec::new());
                }
                app.run(Command::Add(Shape::Cuboid));
                let mut shapes = Vec::new();
                for _ in 0..4 {
                    shapes = frame(&ctx, &mut app, size, scale, &mut time, Vec::new());
                }
                let texts = text_rects(&shapes);
                for (i, (a, ra)) in texts.iter().enumerate() {
                    for (b, rb) in texts.iter().skip(i + 1) {
                        let overlap = ra.intersect(*rb);
                        let axis = |s: &str| matches!(s, "X" | "Y" | "Z");
                        if overlap.width() > 0.5 && overlap.height() > 0.5 && (axis(a) || axis(b)) {
                            bad.push(format!("{language:?} {scale}x {width}px: `{a}` overlaps `{b}`"));
                        }
                    }
                }
            }
        }
    }
    engine_i18n::set(engine_i18n::Language::English);
    assert!(bad.is_empty(), "{} overlaps, first:
{}", bad.len(), bad.iter().take(15).cloned().collect::<Vec<_>>().join("
"));
}
