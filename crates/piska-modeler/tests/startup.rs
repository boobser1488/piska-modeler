//! Запуск Piska Modeler без окна и GPU — тот же конечный автомат, что ведёт окно
//! (`startup::start` / `startup::frame`): первый показанный кадр — заставка в цветах темы с
//! текстом шага; окно не показывают раньше полного кадра с темой (`Boot::themed`, а `Reveal`
//! ждёт два кадра); окно моделей появляется, когда работа кончилась; сбой при запуске — экран
//! ошибки с текстом, а не зависшее окно.

use std::sync::mpsc;
use std::time::Duration;

use engine_modeler::ui::boot::{painted_fills, painted_texts, Boot, BootState};
use engine_modeler::ui::kit as theme;
use engine_modeler::ui::ModelerApp;
use piska_modeler::startup;

fn run(ctx: &egui::Context, boot: &mut Boot<ModelerApp>, time: f64) -> egui::FullOutput {
    let input = egui::RawInput {
        screen_rect: Some(egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(1440.0, 880.0))),
        time: Some(time),
        predicted_dt: 1.0 / 60.0,
        ..egui::RawInput::default()
    };
    ctx.run(input, |ctx| startup::frame(boot, ctx))
}

fn until(ctx: &egui::Context, boot: &mut Boot<ModelerApp>, time: &mut f64, limit: f64, mut done: impl FnMut(&Boot<ModelerApp>) -> bool) -> egui::FullOutput {
    let mut full = run(ctx, boot, *time);
    let end = *time + limit;
    while !done(boot) && *time < end {
        *time += 0.016;
        std::thread::sleep(Duration::from_millis(4));
        full = run(ctx, boot, *time);
    }
    full
}

#[test]
fn the_first_presented_frame_is_the_themed_splash_with_a_status_line() {
    let (release, gate) = mpsc::channel::<()>();
    let mut boot = startup::start_with(move |progress| {
        progress.set_step("Opening the model files…");
        gate.recv().ok();
        Ok(ModelerApp::new())
    });
    let ctx = egui::Context::default();
    run(&ctx, &mut boot, 0.0);
    assert!(!boot.themed(), "the theme-setting frame is never shown");
    let full = run(&ctx, &mut boot, 0.016);
    assert!(boot.themed(), "a full themed frame: the window may be shown now");
    assert_eq!(boot.state(), BootState::Loading);
    let texts = painted_texts(&full);
    assert!(texts.iter().any(|t| t == "Piska Modeler"), "the name is on the splash: {texts:?}");
    assert!(texts.iter().any(|t| t.contains("Opening the model files")), "the real step is on the splash: {texts:?}");
    let (rect, fill) = painted_fills(&full)[0];
    assert!(rect.width() >= 1439.0 && rect.height() >= 879.0, "the first fill covers the window");
    assert_eq!(fill, theme::BG_APP, "the window background is the theme colour, never white or black");
    let mut time = 0.032;
    let full = until(&ctx, &mut boot, &mut time, 1.0, |_| false);
    assert!(painted_texts(&full).iter().any(|t| t.contains("Opening the model files")), "the window keeps drawing while it waits");
    release.send(()).unwrap();
}

#[test]
fn the_modeler_replaces_the_splash_when_the_work_completes() {
    let (release, gate) = mpsc::channel::<()>();
    let mut boot = startup::start_with(move |_| {
        gate.recv().ok();
        Ok(ModelerApp::new())
    });
    let ctx = egui::Context::default();
    run(&ctx, &mut boot, 0.0);
    run(&ctx, &mut boot, 0.016);
    release.send(()).unwrap();
    let mut time = 0.032;
    until(&ctx, &mut boot, &mut time, 20.0, |b| b.state() == BootState::Ready);
    assert_eq!(boot.state(), BootState::Ready, "the program was built in the background");
    let full = run(&ctx, &mut boot, time + 0.016);
    let texts = painted_texts(&full);
    assert!(texts.iter().any(|t| t == "File"), "the Modeler's own menu is drawn under the fading splash: {texts:?}");
    until(&ctx, &mut boot, &mut time, 30.0, |b| !b.splash_visible());
    assert!(!boot.splash_visible());
    assert!(time >= 0.6, "the splash is held for at least 0.6 s (gone at {time:.2} s)");
}

#[test]
fn a_failure_while_loading_shows_an_error_screen_instead_of_hanging() {
    let mut boot = startup::start_with(|_| Err("cannot read `rover.pmodel`: access is denied".to_string()));
    let ctx = egui::Context::default();
    let mut time = 0.0;
    until(&ctx, &mut boot, &mut time, 5.0, |b| b.state() == BootState::Failed);
    assert_eq!(boot.state(), BootState::Failed);
    let full = run(&ctx, &mut boot, time + 0.016);
    let texts = painted_texts(&full);
    assert!(texts.iter().any(|t| t.contains("Piska Modeler could not start")), "{texts:?}");
    assert!(texts.iter().any(|t| t.contains("access is denied")), "the message is on the screen: {texts:?}");
    assert!(texts.iter().any(|t| t == "Close") && texts.iter().any(|t| t == "Copy details"), "{texts:?}");
    assert!(boot.themed());
}

#[test]
fn the_real_start_opens_the_files_from_the_command_line() {
    let options = piska_modeler::parse_args(["--sample".to_string(), "crate".to_string()]).unwrap();
    let mut boot = startup::start(options);
    let ctx = egui::Context::default();
    let mut time = 0.0;
    until(&ctx, &mut boot, &mut time, 20.0, |b| b.state() != BootState::Loading);
    assert_eq!(boot.state(), BootState::Ready, "{:?}", boot.error());
    assert!(boot.value().unwrap().document().find_by_name("body").is_some() || !boot.value().unwrap().document().nodes.is_empty(), "the sample model was opened");
}
