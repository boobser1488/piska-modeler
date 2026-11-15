//! Запуск Piska Modeler: окно с заставкой появляется сразу, а сама программа (документы из
//! аргументов, образцы, восстановление) собирается в фоновом потоке. Кадр рисует [`frame`]:
//! заставка со знаком Modeler и настоящим шагом, потом окно моделей, при сбое — экран ошибки с
//! текстом, а не зависшее окно. Устройство wgpu готовится в отдельном потоке с первой секунды
//! ([`Warmup`]).
//!
//! И окно (`lib.rs`), и тесты ведут запуск одними и теми же [`start`] / [`frame`].

use std::sync::Arc;
use std::thread::JoinHandle;

use egui::{pos2, vec2, Color32, Rect, Shape, Stroke};
use engine_i18n::{brand, tr_s, trf};
use engine_modeler::ui::boot::{Boot, Progress, Task};
use engine_modeler::ui::ModelerApp;
use winit::window::Window;

use crate::Options;

/// Запуск программы с параметрами командной строки; окно моделей собирается в фоне.
pub fn start(options: Options) -> Boot<ModelerApp> {
    start_with(move |progress| {
        progress.set_step(if options.files.is_empty() { tr_s("Preparing the editor…") } else { tr_s("Opening the model files…") });
        Ok(crate::build_app(&options))
    })
}

/// То же с заданной работой (тесты подставляют медленную или неудачную).
pub fn start_with(load: impl FnOnce(&Progress) -> Result<ModelerApp, String> + Send + 'static) -> Boot<ModelerApp> {
    let task = Task::spawn("modeler-start", tr_s("Starting…"), load);
    Boot::start(brand::MODELER_NAME, &trf!("Version {version}", version = env!("CARGO_PKG_VERSION")), mark, task)
}

/// Один кадр окна: заставка, пока программа собирается; потом окно моделей.
pub fn frame(boot: &mut Boot<ModelerApp>, ctx: &egui::Context) {
    boot.frame(ctx, |ctx, app| {
        egui::CentralPanel::default().frame(egui::Frame::none()).show(ctx, |ui| app.show(ui));
        None
    });
}

/// Знак Piska Modeler (как `assets/icon.png`): белая «P» и кубик на оранжевой плитке.
pub fn mark(painter: &egui::Painter, rect: Rect, alpha: f32) {
    let fade = |c: Color32| c.gamma_multiply(alpha);
    let tile = fade(Color32::from_rgb(230, 100, 38));
    let white = fade(Color32::WHITE);
    let at = |x: f32, y: f32| rect.min + vec2(x * rect.width(), y * rect.height());
    let block = |x0: f32, y0: f32, x1: f32, y1: f32| Rect::from_min_max(at(x0, y0), at(x1, y1));
    let r = rect.width();
    painter.rect_filled(rect, 0.21 * r, tile);
    painter.rect_filled(block(0.24, 0.17, 0.40, 0.83), 0.0, white);
    painter.rect_filled(block(0.40, 0.17, 0.46, 0.55), 0.0, white);
    painter.circle_filled(at(0.46, 0.36), 0.19 * r, white);
    painter.rect_filled(block(0.40, 0.29, 0.47, 0.43), 0.0, tile);
    painter.circle_filled(at(0.47, 0.36), 0.07 * r, tile);
    // Кубик: верх и левая грань светлые, правая тёмная.
    let (cx, cy, s) = (0.72, 0.72, 0.12);
    let k = 0.866 * s;
    let p = |x: f32, y: f32| pos2(rect.min.x + x * rect.width(), rect.min.y + y * rect.height());
    let (top, ur, lr, bottom, ll, ul, mid) =
        (p(cx, cy - s), p(cx + k, cy - s * 0.5), p(cx + k, cy + s * 0.5), p(cx, cy + s), p(cx - k, cy + s * 0.5), p(cx - k, cy - s * 0.5), p(cx, cy));
    painter.add(Shape::convex_polygon(vec![top, ur, mid, ul], fade(Color32::from_rgb(253, 228, 196)), Stroke::NONE));
    painter.add(Shape::convex_polygon(vec![ul, mid, bottom, ll], fade(Color32::from_rgb(255, 200, 150)), Stroke::NONE));
    painter.add(Shape::convex_polygon(vec![mid, ur, lr, bottom], fade(Color32::from_rgb(70, 40, 28)), Stroke::NONE));
}

/// Устройство wgpu, которое готовится в фоне с начала запуска, пока создаются цикл событий и
/// окно: поиск адаптера и открытие устройства на Windows занимают заметное время. Устройство
/// запрашивается без поверхности; когда окно создано, поверхность берётся из того же
/// `Instance`, а если адаптер её не умеет, устройство запрашивается заново — под поверхность.
pub struct Warmup {
    handle: Option<JoinHandle<Result<(wgpu::Instance, wgpu::Adapter, wgpu::Device, wgpu::Queue), String>>>,
}

/// Поверхность окна и устройство, которое её умеет.
pub type WindowDevice = (wgpu::Surface<'static>, wgpu::Adapter, wgpu::Device, wgpu::Queue);

impl Warmup {
    pub fn start() -> Self {
        let handle = std::thread::Builder::new()
            .name("gpu-warmup".to_string())
            .spawn(|| {
                let instance = wgpu::Instance::new(wgpu::InstanceDescriptor::default());
                let (adapter, device, queue) = crate::request_device(&instance, None)?;
                Ok((instance, adapter, device, queue))
            })
            .ok();
        Self { handle }
    }

    /// Поверхность для `window` и устройство; ждёт фоновую подготовку, если она ещё идёт.
    pub fn finish(self, window: Arc<Window>) -> Result<WindowDevice, String> {
        if let Some(Ok((instance, adapter, device, queue))) = self.handle.and_then(|h| h.join().ok()).map(|r| r.map_err(|e| log::warn!("early GPU start failed: {e}"))) {
            if let Ok(surface) = instance.create_surface(window.clone()) {
                if !surface.get_capabilities(&adapter).formats.is_empty() {
                    return Ok((surface, adapter, device, queue));
                }
            }
        }
        let instance = wgpu::Instance::new(wgpu::InstanceDescriptor::default());
        let surface = instance.create_surface(window).map_err(|e| e.to_string())?;
        let (adapter, device, queue) = crate::request_device(&instance, Some(&surface))?;
        Ok((surface, adapter, device, queue))
    }
}
