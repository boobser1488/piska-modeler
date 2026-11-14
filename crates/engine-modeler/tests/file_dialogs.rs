//! File ▸ Open / Save As / Import / Export спрашивают путь системным окном файлов (здесь —
//! подставным, настоящее окно тесты не открывают): заголовок, фильтры, папка и имя по
//! умолчанию; выбранный путь выполняет команду, «Отмена» — ничего, нет системного окна —
//! своё окно с полем пути.
#![cfg(feature = "ui")]

use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use engine_build::file_dialog::{self, Kind, Outcome, Provider, Request};
use engine_modeler::ui::{Command, ModelerApp, PromptKind, REFERENCE_FORMATS};

/// Тесты этого файла меняют общего отвечающего — по одному.
static SERIAL: Mutex<()> = Mutex::new(());

struct Fake {
    answer: Mutex<Outcome>,
    asked: Mutex<Vec<Request>>,
}

impl Provider for Fake {
    fn ask(&self, request: &Request) -> Outcome {
        self.asked.lock().unwrap().push(request.clone());
        self.answer.lock().unwrap().clone()
    }
}

fn fake(answer: Outcome) -> Arc<Fake> {
    let fake = Arc::new(Fake { answer: Mutex::new(answer), asked: Mutex::new(Vec::new()) });
    file_dialog::set_provider(fake.clone());
    fake
}

fn frame(ctx: &egui::Context, app: &mut ModelerApp, events: Vec<egui::Event>) {
    let input = egui::RawInput {
        screen_rect: Some(egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(1400.0, 860.0))),
        events,
        ..Default::default()
    };
    let _ = ctx.run(input, |ctx| {
        egui::CentralPanel::default().frame(egui::Frame::none()).show(ctx, |ui| app.show(ui));
    });
}

/// Кадры, пока системное окно не ответит.
fn settle(ctx: &egui::Context, app: &mut ModelerApp) {
    for _ in 0..500 {
        frame(ctx, app, vec![]);
        if !app.file_dialog_open() {
            return;
        }
        std::thread::sleep(std::time::Duration::from_millis(2));
    }
    panic!("the file dialog answered");
}

fn temp(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("modeler-dialogs-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

fn app() -> (egui::Context, ModelerApp) {
    let mut app = ModelerApp::new();
    app.autosave_seconds = 0.0;
    let ctx = egui::Context::default();
    frame(&ctx, &mut app, vec![]);
    (ctx, app)
}

#[test]
fn save_as_and_open_use_the_system_dialog() {
    let _serial = SERIAL.lock().unwrap_or_else(|e| e.into_inner());
    let dir = temp("save-open");
    let (ctx, mut app) = app();
    app.working_dir = dir.clone();
    app.run(Command::Add(engine_modeler::Shape::Cuboid));

    // Save As: имя по умолчанию — имя модели, папка — рабочая, фильтр .pmodel; расширение
    // дописывается, если его не написали.
    let save = fake(Outcome::Chosen(dir.join("Сундук с пробелом")));
    app.run(Command::SaveAs);
    assert!(app.file_dialog_open());
    settle(&ctx, &mut app);
    let request = save.asked.lock().unwrap()[0].clone();
    assert_eq!(request.kind, Kind::Save);
    assert_eq!(request.title, "Save Model As");
    assert_eq!(request.file_name.as_deref(), Some("untitled.pmodel"));
    assert_eq!(request.folder.as_deref(), Some(dir.as_path()));
    assert_eq!(request.filters[0].extensions, ["pmodel"]);
    let saved = dir.join("Сундук с пробелом.pmodel");
    assert!(saved.is_file(), "Save As wrote {}", saved.display());
    assert_eq!(app.path(), Some(saved.as_path()));
    assert!(!app.path_prompt_open(), "no path prompt when the system dialog answered");

    // Ctrl+O — окно «Открыть»: все модели (свои и чужие форматы), .pmodel, .bbmodel и
    // «All files», папка — папка документа.
    app.run(Command::New);
    let open = fake(Outcome::Chosen(saved.clone()));
    let ctrl_o = egui::Event::Key { key: egui::Key::O, physical_key: None, pressed: true, repeat: false, modifiers: egui::Modifiers::COMMAND };
    frame(&ctx, &mut app, vec![ctrl_o]);
    assert!(app.file_dialog_open() || !open.asked.lock().unwrap().is_empty(), "Ctrl+O opens the Open dialog");
    settle(&ctx, &mut app);
    let request = open.asked.lock().unwrap()[0].clone();
    assert_eq!((request.kind, request.title.as_str()), (Kind::Open, "Open Model"));
    let filters: Vec<Vec<String>> = request.filters.iter().map(|f| f.extensions.clone()).collect();
    assert_eq!(filters.len(), 4, "{filters:?}");
    for ext in ["pmodel", "bbmodel", "obj", "glb", "ply", "stl", "vox"] {
        assert!(filters[0].iter().any(|e| e == ext), "the Models filter lists .{ext}");
    }
    assert_eq!((&filters[1], &filters[2], &filters[3]), (&vec!["pmodel".to_string()], &vec!["bbmodel".to_string()], &vec![]));
    assert_eq!(app.path(), Some(saved.as_path()), "the chosen model is open");
    file_dialog::set_provider_opt(None);
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn cancel_does_nothing_and_no_system_dialog_falls_back_to_the_path_prompt() {
    let _serial = SERIAL.lock().unwrap_or_else(|e| e.into_inner());
    let (ctx, mut app) = app();
    fake(Outcome::Cancelled);
    app.run(Command::Open);
    settle(&ctx, &mut app);
    assert!(!app.path_prompt_open() && app.tab_count() == 1, "Cancel: nothing happens");

    fake(Outcome::Unavailable);
    app.run(Command::Open);
    settle(&ctx, &mut app);
    assert!(app.path_prompt_open(), "no system dialog (no zenity, an error): the built-in path prompt");

    // Без отвечающего (тесты, снимки) — сразу своё окно, ничего не ждёт.
    file_dialog::set_provider_opt(None);
    let (ctx, mut app) = self::app();
    app.run(Command::SaveAs);
    assert!(!app.file_dialog_open());
    assert!(app.path_prompt_open());
    frame(&ctx, &mut app, vec![]);
}

#[test]
fn import_and_export_dialogs_list_their_formats() {
    let _serial = SERIAL.lock().unwrap_or_else(|e| e.into_inner());
    let (ctx, mut app) = app();
    let fake = fake(Outcome::Cancelled);
    // Те же окна, что из меню File: импорт опорного меша — таблица форматов.
    app.run(Command::Ask(PromptKind::ImportReference));
    settle(&ctx, &mut app);
    let request = fake.asked.lock().unwrap().last().cloned().unwrap();
    assert_eq!(request.kind, Kind::Open);
    let all: Vec<String> = REFERENCE_FORMATS.iter().flat_map(|(_, e)| e.iter().map(|s| s.to_string())).collect();
    assert_eq!(request.filters[0].extensions, all, "the first filter has every reference format");
    for (i, (_, extensions)) in REFERENCE_FORMATS.iter().enumerate() {
        assert_eq!(request.filters[i + 1].extensions, extensions.iter().map(|s| s.to_string()).collect::<Vec<_>>());
    }
    app.run(Command::Ask(PromptKind::ExportGlb));
    settle(&ctx, &mut app);
    let request = fake.asked.lock().unwrap().last().cloned().unwrap();
    assert_eq!(request.kind, Kind::Save);
    assert_eq!(request.file_name.as_deref(), Some("untitled.glb"));
    assert_eq!(request.default_extension(), Some("glb"));
    file_dialog::set_provider_opt(None);
}

#[test]
fn the_path_prompt_has_the_shared_button_row_and_moves_by_its_title() {
    let _serial = SERIAL.lock().unwrap_or_else(|e| e.into_inner());
    file_dialog::set_provider_opt(None);
    let (ctx, mut app) = app();
    app.run(Command::SaveAs);
    for _ in 0..3 {
        frame(&ctx, &mut app, vec![]);
    }
    let layout = ModelerApp::prompt_layout(&ctx).expect("the Save Model As prompt is drawn");
    // «Cancel | Save» справа внизу, одна линия, одна высота, отступ от поля 12–14 px.
    assert!((layout.primary.right() - layout.body.right()).abs() <= 1.0, "{layout:?}: the primary button is flush right");
    assert!(layout.cancel.right() <= layout.primary.left(), "Cancel is left of the primary button");
    assert!((layout.cancel.center().y - layout.primary.center().y).abs() < 0.5, "one baseline");
    assert_eq!(layout.cancel.height(), layout.primary.height());
    assert!(layout.cancel.width() >= 80.0 && layout.primary.width() >= 80.0);
    let gap = layout.primary.top() - layout.content_bottom;
    assert!((12.0..=24.0).contains(&gap), "gap above the buttons is {gap}");

    // Заголовок тянется мышью: окно сдвигается на (+120, +80).
    let window = ctx.memory(|m| m.area_rect(egui::Id::new("modeler.prompt"))).expect("the prompt window is an area");
    let grab = egui::pos2(window.center().x, window.top() + 10.0);
    let to = grab + egui::vec2(120.0, 80.0);
    let button = |pos, pressed| egui::Event::PointerButton { pos, button: egui::PointerButton::Primary, pressed, modifiers: egui::Modifiers::NONE };
    frame(&ctx, &mut app, vec![egui::Event::PointerMoved(grab)]);
    frame(&ctx, &mut app, vec![button(grab, true)]);
    for i in 1..=6 {
        frame(&ctx, &mut app, vec![egui::Event::PointerMoved(grab + (to - grab) * (i as f32 / 6.0))]);
    }
    frame(&ctx, &mut app, vec![button(to, false)]);
    frame(&ctx, &mut app, vec![]);
    let moved = ctx.memory(|m| m.area_rect(egui::Id::new("modeler.prompt"))).unwrap();
    assert!((moved.min - window.min - egui::vec2(120.0, 80.0)).length() < 4.0, "{window:?} → {moved:?}");

    // Esc закрывает окно.
    let esc = egui::Event::Key { key: egui::Key::Escape, physical_key: None, pressed: true, repeat: false, modifiers: egui::Modifiers::NONE };
    frame(&ctx, &mut app, vec![esc]);
    assert!(!app.path_prompt_open(), "Esc cancels");
}
