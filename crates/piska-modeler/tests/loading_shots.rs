//! Снимки заставки запуска Piska Modeler, экрана ошибки и строк прогресса (`kit::boot`) без окна:
//! английский при 100 % и русский при 150 %. Куда писать — `PISKA_SHOTS_DIR` (без неё только
//! проверка, что кадр не пустой). Один тест на файл: видеокарта одна.

use std::sync::mpsc;
use std::time::Duration;

use engine_i18n::{tr_s, Language};
use engine_modeler::ui::boot::{progress_row, BootState};
use engine_modeler::ui::{controls, kit as theme};
use piska_modeler::startup;

const FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Rgba8Unorm;
const SIZE: (u32, u32) = (720, 440);

/// Рисует `frames` кадров `ui(ctx)` при масштабе `scale` и возвращает последний.
fn render(scale: f32, frames: usize, mut ui: impl FnMut(&egui::Context)) -> image::RgbaImage {
    let size = [(SIZE.0 as f32 * scale).round() as u32, (SIZE.1 as f32 * scale).round() as u32];
    let instance = wgpu::Instance::new(wgpu::InstanceDescriptor::default());
    let (_, device, queue) = piska_modeler::request_device(&instance, None).expect("a GPU");
    let mut renderer = egui_wgpu::Renderer::new(&device, FORMAT, None, 1, false);
    let target = device.create_texture(&wgpu::TextureDescriptor {
        label: Some("loading shot"),
        size: wgpu::Extent3d { width: size[0], height: size[1], depth_or_array_layers: 1 },
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format: FORMAT,
        usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::COPY_SRC,
        view_formats: &[],
    });
    let view = target.create_view(&wgpu::TextureViewDescriptor::default());
    let ctx = egui::Context::default();
    ctx.set_zoom_factor(scale);
    for i in 0..frames {
        let raw = egui::RawInput {
            screen_rect: Some(egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(SIZE.0 as f32, SIZE.1 as f32))),
            time: Some(i as f64 * 0.05),
            predicted_dt: 0.05,
            ..egui::RawInput::default()
        };
        let full = ctx.run(raw, |ctx| ui(ctx));
        piska_modeler::paint_ui(&device, &queue, &mut renderer, &ctx, full, &view, size);
    }
    piska_modeler::read_texture(&device, &queue, &target, size)
}

fn save(image: &image::RgbaImage, name: &str) {
    let first = image.get_pixel(0, 0);
    assert!(image.pixels().any(|p| p != first), "{name}: the picture is empty");
    if let Ok(dir) = std::env::var("PISKA_SHOTS_DIR") {
        let dir = std::path::PathBuf::from(dir);
        std::fs::create_dir_all(&dir).unwrap();
        image.save(dir.join(name)).unwrap();
    }
}

fn gallery(ctx: &egui::Context) {
    if !engine_modeler::ui::boot::theme_ready(ctx) {
        return;
    }
    egui::CentralPanel::default().frame(egui::Frame::none().fill(theme::BG_PANEL).inner_margin(egui::Margin::same(theme::SP_8))).show(ctx, |ui| {
        controls::card(ui, |ui| {
            progress_row(ui, "Importing rover_body.glb: reading 12 of 48 meshes", Some(0.25), true);
            ui.add_space(theme::SP_6);
            progress_row(ui, tr_s("Preparing the editor…"), None, false);
            ui.add_space(theme::SP_6);
            progress_row(ui, "Exporting assets/models/characters/hero_idle_forward_and_backwards_walk_cycle.glb", Some(0.62), true);
        });
    });
}

#[test]
fn the_start_up_screens_look_right_in_english_and_russian() {
    for (language, scale, tag) in [(Language::English, 1.0, "en100"), (Language::Russian, 1.5, "ru150")] {
        engine_i18n::set(language);
        let (release, gate) = mpsc::channel::<()>();
        let mut boot = startup::start_with(move |progress| {
            progress.set_step(tr_s("Opening the model files…"));
            gate.recv().ok();
            Err("released".to_string())
        });
        let image = render(scale, 6, |ctx| startup::frame(&mut boot, ctx));
        assert_eq!(boot.state(), BootState::Loading);
        save(&image, &format!("modeler_splash_{tag}.png"));
        release.send(()).unwrap();
        std::thread::sleep(Duration::from_millis(50));

        let mut boot = startup::start_with(|_| Err("cannot read `rover.pmodel`: the file is locked by another program (os error 32)".to_string()));
        std::thread::sleep(Duration::from_millis(100));
        let image = render(scale, 6, |ctx| startup::frame(&mut boot, ctx));
        assert_eq!(boot.state(), BootState::Failed);
        save(&image, &format!("modeler_error_{tag}.png"));

        let image = render(scale, 6, gallery);
        save(&image, &format!("progress_rows_modeler_{tag}.png"));
    }
    engine_i18n::set(Language::English);
}
