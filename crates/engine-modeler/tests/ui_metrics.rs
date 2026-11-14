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
use engine_modeler::ui::{Command, Mode, ModelerApp, PromptKind};
use engine_modeler::Shape;

fn frame(ctx: &egui::Context, app: &mut ModelerApp, size: egui::Vec2, time: &mut f64, events: Vec<egui::Event>) -> Vec<egui::epaint::ClippedShape> {
    let input = egui::RawInput {
        screen_rect: Some(egui::Rect::from_min_size(egui::Pos2::ZERO, size)),
        time: Some(*time),
        predicted_dt: 1.0 / 60.0,
        events,
        ..Default::default()
    };
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
    ]
}

#[test]
#[ignore = "harness unfinished: with set_zoom_factor the headless viewport asks for a 9570 px texture; run with --ignored after fixing the frame size"]
fn modeler_screens_measure_up() {
    let mut total = Report::default();
    let (mut screens_done, mut states_done) = (0, 0);
    for language in [engine_i18n::Language::English, engine_i18n::Language::Russian] {
        engine_i18n::set(language);
        for scale in [1.0_f32, 1.25, 1.5] {
            for (name, setup) in screens() {
                for width in [1100.0_f32, 1400.0, 1800.0] {
                    let mut app = ModelerApp::new();
                    app.autosave_seconds = 0.0;
                    let ctx = egui::Context::default();
                    ctx.set_zoom_factor(scale);
                    let size = egui::vec2(width, 860.0) / scale;
                    let mut time = 0.0;
                    for _ in 0..3 {
                        frame(&ctx, &mut app, size, &mut time, Vec::new());
                    }
                    setup(&mut app);
                    for _ in 0..3 {
                        frame(&ctx, &mut app, size, &mut time, Vec::new());
                    }
                    let shapes = frame(&ctx, &mut app, size, &mut time, Vec::new());
                    let screen = format!("{language:?} {scale}x {width}px {name}");
                    let rules = Rules::compact();
                    total.merge(metrics::analyze_shapes(&screen, &metrics::widgets(&ctx), &shapes, rules));
                    screens_done += 1;
                    if width == 1100.0 && scale != 1.25 {
                        let (report, states) = metrics::analyze_states(&screen, &ctx, rules, |events| frame(&ctx, &mut app, size, &mut time, events));
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
    assert!(states_done > 100, "control states driven: {states_done}");
    assert!(total.offenders.is_empty(), "{} offenders, first ones:\n{}", total.offenders.len(), total.offenders.iter().take(25).cloned().collect::<Vec<_>>().join("\n"));
}
