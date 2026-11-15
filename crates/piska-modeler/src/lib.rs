//! Piska Modeler — отдельное окно редактора моделей: окно (winit + wgpu + egui), снимки
//! без окна и замер кадра.
//!
//! Вид модели рисует видеокарта ([`engine_modeler::gpu::GpuViewport`]); программный
//! растеризатор остаётся запасным путём (`--cpu`). Кадр рисуется только по событию: ввод,
//! анимация egui или изменившийся вид; в покое окно просыпается раз в секунду (файл
//! восстановления, перечитывание текстур) и почти не тратит процессор.

use std::sync::Arc;
use std::time::{Duration, Instant};

use engine_modeler::gpu::GpuViewport;
use engine_modeler::ui::boot::Boot;
use engine_modeler::ui::{Command, Mode, ModelerApp, Tool};
use engine_modeler::{FaceDir, ViewPreset};
use winit::application::ApplicationHandler;
use winit::dpi::LogicalSize;
use winit::event::WindowEvent;
use winit::event_loop::{ActiveEventLoop, ControlFlow, EventLoop};
use winit::window::{Window, WindowId};

pub mod startup;

/// Параметры командной строки.
#[derive(Clone, Debug, Default)]
pub struct Options {
    pub files: Vec<String>,
    pub screenshot: Option<String>,
    pub size: (u32, u32),
    pub frames: u32,
    pub sample: Option<String>,
    pub paint: bool,
    pub tool: Option<String>,
    pub view: Option<String>,
    pub ortho: bool,
    pub select: Option<String>,
    pub face: Option<String>,
    pub pointer: Option<(f32, f32)>,
    pub scale: f32,
    pub bench: bool,
    /// Программный растеризатор вместо видеокарты.
    pub cpu: bool,
    /// `--reference меш.obj|.glb|.bbmodel|…` — добавить меш редактируемыми элементами в открытую
    /// модель и сохранить её
    /// (редактор движка: «Edit a Copy in Modeler» для чужого меша).
    pub reference: Option<String>,
}

pub const USAGE: &str = "usage: piska-modeler [model.pmodel…] [--screenshot out.png --size 1440x860 --frames 6 \
--sample crate|lantern|pine|rover --paint --tool NAME --view front|top|… --ortho --select NODE \
--face north|… --pointer X,Y --scale 1.0 --cpu] [--reference mesh.obj] | --bench [--size WxH --frames N --cpu]";

pub fn parse_args(args: impl IntoIterator<Item = String>) -> Result<Options, String> {
    let mut o = Options { size: (1440, 860), frames: 6, scale: 1.0, ..Options::default() };
    let mut args = args.into_iter();
    while let Some(arg) = args.next() {
        let mut value = |name: &str| args.next().ok_or_else(|| format!("`{name}` needs a value"));
        match arg.as_str() {
            "--screenshot" => o.screenshot = Some(value("--screenshot")?),
            "--size" => {
                let v = value("--size")?;
                let (w, h) = v.split_once('x').ok_or("--size is WIDTHxHEIGHT")?;
                o.size = (w.parse().map_err(|_| "bad width")?, h.parse().map_err(|_| "bad height")?);
            }
            "--frames" => o.frames = value("--frames")?.parse().map_err(|_| "--frames is a number")?,
            "--sample" => o.sample = Some(value("--sample")?),
            "--paint" => o.paint = true,
            "--bench" => o.bench = true,
            "--cpu" => o.cpu = true,
            "--reference" => o.reference = Some(value("--reference")?),
            "--tool" => o.tool = Some(value("--tool")?),
            "--view" => o.view = Some(value("--view")?),
            "--ortho" => o.ortho = true,
            "--select" => o.select = Some(value("--select")?),
            "--face" => o.face = Some(value("--face")?),
            "--scale" => o.scale = value("--scale")?.parse().map_err(|_| "--scale is a number")?,
            "--pointer" => {
                let v = value("--pointer")?;
                let (x, y) = v.split_once(',').ok_or("--pointer is X,Y")?;
                o.pointer = Some((x.parse().map_err(|_| "bad X")?, y.parse().map_err(|_| "bad Y")?));
            }
            "--help" | "-h" => return Err(USAGE.to_string()),
            other if other.starts_with("--") => return Err(format!("unknown option `{other}` (see --help)")),
            file => o.files.push(file.to_string()),
        }
    }
    Ok(o)
}

/// Окно моделей с документами из аргументов и настройками для снимков.
pub fn build_app(o: &Options) -> ModelerApp {
    let mut app = ModelerApp::new();
    for file in &o.files {
        if let Err(e) = app.open(file) {
            eprintln!("piska-modeler: {e}");
        }
    }
    if let Some(name) = &o.sample {
        match engine_modeler::samples::all().into_iter().find(|(n, _)| n == name) {
            Some((_, doc)) => app.open_document(doc, None),
            None => eprintln!("piska-modeler: no sample `{name}` (crate, lantern, pine, rover)"),
        }
    }
    if let Some(mesh) = &o.reference {
        // Копия чужого меша для правки: меш становится редактируемыми элементами новой
        // модели (выделение, перенос, рисование, вершины; `.bbmodel` — кубами), модель сразу
        // сохранена — редактор движка видит её в окне Project.
        app.run(Command::ImportMesh(std::path::PathBuf::from(mesh)));
        if app.path().is_some() {
            app.run(Command::Save);
        }
    }
    if let Some(name) = &o.select {
        let id = app.document().find_by_name(name);
        if id.is_none() {
            eprintln!("piska-modeler: no node `{name}` in the model");
        }
        app.session_mut().select(id);
    }
    if let Some(face) = &o.face {
        let face = FaceDir::ALL.into_iter().find(|f| f.name().eq_ignore_ascii_case(face));
        app.session_mut().face = face;
    }
    if let Some(view) = &o.view {
        let preset = match view.as_str() {
            "front" => Some(ViewPreset::Front),
            "back" => Some(ViewPreset::Back),
            "left" => Some(ViewPreset::Left),
            "right" => Some(ViewPreset::Right),
            "top" => Some(ViewPreset::Top),
            "bottom" => Some(ViewPreset::Bottom),
            _ => None,
        };
        if let Some(preset) = preset {
            app.run(Command::View(preset));
            app.run(Command::Frame);
        }
    }
    if o.ortho {
        app.run(Command::ToggleOrthographic);
    }
    if o.paint {
        app.run(Command::SetMode(Mode::Paint));
    }
    if let Some(name) = &o.tool {
        let all = Tool::EDIT.iter().chain(Tool::PAINT.iter());
        match all.into_iter().find(|t| t.name().eq_ignore_ascii_case(name)) {
            Some(tool) => app.run(Command::Tool(*tool)),
            None => eprintln!("piska-modeler: unknown tool `{name}`"),
        }
    }
    app
}

fn frame_ui(app: &mut ModelerApp, ctx: &egui::Context, input: egui::RawInput) -> egui::FullOutput {
    ctx.run(input, |ctx| {
        egui::CentralPanel::default().frame(egui::Frame::none()).show(ctx, |ui| app.show(ui));
    })
}

/// Устройство wgpu (для окна — совместимое с его поверхностью).
pub fn request_device(
    instance: &wgpu::Instance,
    surface: Option<&wgpu::Surface<'_>>,
) -> Result<(wgpu::Adapter, wgpu::Device, wgpu::Queue), String> {
    let adapter = pollster::block_on(instance.request_adapter(&wgpu::RequestAdapterOptions {
        power_preference: wgpu::PowerPreference::HighPerformance,
        force_fallback_adapter: false,
        compatible_surface: surface,
    }))
    .ok_or("no graphics adapter (GPU) was found")?;
    let (device, queue) = pollster::block_on(adapter.request_device(
        &wgpu::DeviceDescriptor {
            label: Some("piska-modeler"),
            required_features: wgpu::Features::empty(),
            required_limits: wgpu::Limits::downlevel_defaults().using_resolution(adapter.limits()),
            memory_hints: wgpu::MemoryHints::Performance,
        },
        None,
    ))
    .map_err(|e| format!("cannot open the GPU: {e}"))?;
    Ok((adapter, device, queue))
}

/// Нарисовать интерфейс egui в текстуру.
pub fn paint_ui(
    device: &wgpu::Device,
    queue: &wgpu::Queue,
    renderer: &mut egui_wgpu::Renderer,
    ctx: &egui::Context,
    full: egui::FullOutput,
    target: &wgpu::TextureView,
    size: [u32; 2],
) {
    let primitives = ctx.tessellate(full.shapes, full.pixels_per_point);
    let screen = egui_wgpu::ScreenDescriptor { size_in_pixels: size, pixels_per_point: full.pixels_per_point };
    for (id, delta) in &full.textures_delta.set {
        renderer.update_texture(device, queue, *id, delta);
    }
    let mut encoder = device.create_command_encoder(&wgpu::CommandEncoderDescriptor { label: Some("modeler ui") });
    let extra = renderer.update_buffers(device, queue, &mut encoder, &primitives, &screen);
    {
        let mut pass = encoder
            .begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("modeler ui"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: target,
                    resolve_target: None,
                    ops: wgpu::Operations { load: wgpu::LoadOp::Clear(wgpu::Color::BLACK), store: wgpu::StoreOp::Store },
                })],
                depth_stencil_attachment: None,
                timestamp_writes: None,
                occlusion_query_set: None,
            })
            .forget_lifetime();
        renderer.render(&mut pass, &primitives, &screen);
    }
    queue.submit(extra.into_iter().chain(std::iter::once(encoder.finish())));
    for id in &full.textures_delta.free {
        renderer.free_texture(id);
    }
}

/// Прочитать текстуру RGBA8 в картинку.
pub fn read_texture(device: &wgpu::Device, queue: &wgpu::Queue, texture: &wgpu::Texture, size: [u32; 2]) -> image::RgbaImage {
    let [w, h] = size;
    let padded = (4 * w).div_ceil(256) * 256;
    let buffer = device.create_buffer(&wgpu::BufferDescriptor {
        label: Some("modeler readback"),
        size: padded as u64 * h as u64,
        usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
        mapped_at_creation: false,
    });
    let mut encoder = device.create_command_encoder(&wgpu::CommandEncoderDescriptor { label: Some("readback") });
    encoder.copy_texture_to_buffer(
        wgpu::ImageCopyTexture { texture, mip_level: 0, origin: wgpu::Origin3d::ZERO, aspect: wgpu::TextureAspect::All },
        wgpu::ImageCopyBuffer {
            buffer: &buffer,
            layout: wgpu::ImageDataLayout { offset: 0, bytes_per_row: Some(padded), rows_per_image: Some(h) },
        },
        wgpu::Extent3d { width: w, height: h, depth_or_array_layers: 1 },
    );
    queue.submit(Some(encoder.finish()));
    let slice = buffer.slice(..);
    slice.map_async(wgpu::MapMode::Read, |_| {});
    let _ = device.poll(wgpu::Maintain::Wait);
    let data = slice.get_mapped_range();
    let mut pixels = Vec::with_capacity((w * h * 4) as usize);
    for row in 0..h {
        let start = (row * padded) as usize;
        pixels.extend_from_slice(&data[start..start + (w * 4) as usize]);
    }
    drop(data);
    buffer.unmap();
    image::RgbaImage::from_raw(w, h, pixels).expect("size matches")
}

/// Окно моделей без окна ОС: устройство, цель и рендереры для снимков и замеров.
pub struct Headless {
    pub device: wgpu::Device,
    pub queue: wgpu::Queue,
    renderer: egui_wgpu::Renderer,
    gpu_view: Option<GpuViewport>,
    target: wgpu::Texture,
    view: wgpu::TextureView,
    size: [u32; 2],
    pub ctx: egui::Context,
    scale: f32,
    frame: u32,
}

/// Время частей кадра, мс.
#[derive(Clone, Copy, Debug, Default)]
pub struct FrameTimes {
    /// Построение интерфейса (без отрисовки вида).
    pub ui: f32,
    /// Кадр вида: растеризация или запись команд видеокарте.
    pub view: f32,
    /// Выгрузка кадра растеризатора в текстуру.
    pub upload: f32,
    /// Тесселяция и отрисовка egui, ожидание видеокарты.
    pub present: f32,
    pub total: f32,
    pub view_rendered: bool,
}

impl Headless {
    pub fn new(size: [u32; 2], scale: f32, gpu_view: bool) -> Result<Self, String> {
        const FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Rgba8Unorm;
        let instance = wgpu::Instance::new(wgpu::InstanceDescriptor::default());
        let (_, device, queue) = request_device(&instance, None)?;
        let mut renderer = egui_wgpu::Renderer::new(&device, FORMAT, None, 1, false);
        let target = device.create_texture(&wgpu::TextureDescriptor {
            label: Some("modeler capture"),
            size: wgpu::Extent3d { width: size[0], height: size[1], depth_or_array_layers: 1 },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: FORMAT,
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::COPY_SRC,
            view_formats: &[],
        });
        let view = target.create_view(&wgpu::TextureViewDescriptor::default());
        let gpu_view = gpu_view.then(|| GpuViewport::new(&device, &mut renderer));
        let ctx = egui::Context::default();
        engine_modeler::ui::kit::apply(&ctx, true);
        // Масштаб — свойством экрана в каждом кадре (`native_pixels_per_point`), а не
        // `set_zoom_factor`: смена масштаба до первого кадра заставляла egui считать первый кадр
        // по прежнему размеру экрана (10 000 точек) — вид просил текстуру в тысячи пикселей.
        Ok(Self { device, queue, renderer, gpu_view, target, view, size, ctx, scale, frame: 0 })
    }

    /// Подключить окно моделей к виду на видеокарте (если он есть).
    pub fn attach(&self, app: &mut ModelerApp) {
        app.set_gpu_texture(self.gpu_view.as_ref().map(GpuViewport::texture_id));
    }

    pub fn gpu_stats(&self) -> Option<engine_modeler::gpu::GpuStats> {
        self.gpu_view.as_ref().map(|g| g.stats)
    }

    /// Один кадр: интерфейс, вид (если просили), egui; ждёт видеокарту.
    pub fn frame(&mut self, app: &mut ModelerApp, events: Vec<egui::Event>) -> FrameTimes {
        let started = Instant::now();
        let input = egui::RawInput {
            screen_rect: Some(egui::Rect::from_min_size(
                egui::Pos2::ZERO,
                egui::vec2(self.size[0] as f32, self.size[1] as f32) / self.scale,
            )),
            time: Some(self.frame as f64 / 60.0),
            predicted_dt: 1.0 / 60.0,
            events,
            ..Default::default()
        };
        let mut input = input;
        input.viewports.entry(egui::ViewportId::ROOT).or_default().native_pixels_per_point = Some(self.scale);
        self.frame += 1;
        let renders = app.render_count();
        let full = frame_ui(app, &self.ctx, input);
        let mut times = FrameTimes { ui: started.elapsed().as_secs_f32() * 1000.0, ..FrameTimes::default() };
        times.view_rendered = app.render_count() > renders;
        if self.gpu_view.is_none() && times.view_rendered {
            let (raster, upload) = app.viewport_timings();
            times.ui -= raster + upload;
            times.view = raster;
            times.upload = upload;
        }
        if let (Some(gpu), Some(request)) = (self.gpu_view.as_mut(), app.take_view_request()) {
            let view_started = Instant::now();
            gpu.render(&self.device, &self.queue, &mut self.renderer, &request);
            times.view = view_started.elapsed().as_secs_f32() * 1000.0;
        }
        let present_started = Instant::now();
        paint_ui(&self.device, &self.queue, &mut self.renderer, &self.ctx, full, &self.view, self.size);
        let _ = self.device.poll(wgpu::Maintain::Wait);
        times.present = present_started.elapsed().as_secs_f32() * 1000.0;
        times.total = started.elapsed().as_secs_f32() * 1000.0;
        times
    }

    pub fn image(&self) -> image::RgbaImage {
        read_texture(&self.device, &self.queue, &self.target, self.size)
    }

    /// Последний кадр вида на видеокарте (для сравнения с растеризатором).
    pub fn view_image(&self) -> Option<image::RgbaImage> {
        let (texture, size) = self.gpu_view.as_ref()?.output()?;
        Some(read_texture(&self.device, &self.queue, texture, size))
    }
}

/// Снимок без окна.
pub fn screenshot(o: &Options, path: &str) -> Result<(), String> {
    let mut app = build_app(o);
    app.autosave_seconds = 0.0;
    if o.cpu {
        app.view.supersample = 2;
    }
    let mut headless = Headless::new([o.size.0, o.size.1], o.scale, !o.cpu)?;
    headless.attach(&mut app);
    for _ in 0..o.frames.max(3) {
        let events = o.pointer.map(|(x, y)| vec![egui::Event::PointerMoved(egui::pos2(x, y))]).unwrap_or_default();
        headless.frame(&mut app, events);
    }
    headless.image().save(path).map_err(|e| format!("cannot write `{path}`: {e}"))?;
    println!("saved {path} ({}x{})", o.size.0, o.size.1);
    Ok(())
}

/// Замер: орбита камеры каждый кадр (вид перерисовывается), затем покой. Печатает
/// средние миллисекунды частей кадра.
pub fn bench(o: &Options) -> Result<(), String> {
    let mut app = build_app(o);
    app.autosave_seconds = 0.0;
    let mut headless = Headless::new([o.size.0, o.size.1], o.scale, !o.cpu)?;
    headless.attach(&mut app);
    for _ in 0..5 {
        headless.frame(&mut app, Vec::new());
    }
    let frames = o.frames.max(30);
    let average = |samples: &[FrameTimes]| {
        let n = samples.len().max(1) as f32;
        let mut a = FrameTimes::default();
        for s in samples {
            a.ui += s.ui / n;
            a.view += s.view / n;
            a.upload += s.upload / n;
            a.present += s.present / n;
            a.total += s.total / n;
        }
        a
    };
    let orbit: Vec<FrameTimes> = (0..frames)
        .map(|_| {
            app.camera_mut().orbit(0.02, 0.0);
            headless.frame(&mut app, Vec::new())
        })
        .collect();
    let idle: Vec<FrameTimes> = (0..frames).map(|_| headless.frame(&mut app, Vec::new())).collect();
    let idle_renders = idle.iter().filter(|t| t.view_rendered).count();
    let path = if o.cpu { "cpu" } else { "gpu" };
    for (name, samples) in [("orbit", &orbit), ("idle", &idle)] {
        let a = average(samples);
        println!(
            "{name:5} {path} {}x{}: ui {:.2} ms, view {:.2} ms, upload {:.2} ms, egui+gpu wait {:.2} ms, total {:.2} ms ({:.0} fps)",
            o.size.0,
            o.size.1,
            a.ui,
            a.view,
            a.upload,
            a.present,
            a.total,
            1000.0 / a.total.max(0.001)
        );
    }
    println!("idle frames that re-rendered the view: {idle_renders} of {frames}");
    if let Some(stats) = headless.gpu_stats() {
        println!("gpu: {} frames, {} mesh uploads, {} texels uploaded", stats.frames, stats.mesh_uploads, stats.texels_uploaded);
    }
    Ok(())
}

struct Gfx {
    window: Arc<Window>,
    surface: wgpu::Surface<'static>,
    config: wgpu::SurfaceConfiguration,
    device: wgpu::Device,
    queue: wgpu::Queue,
    renderer: egui_wgpu::Renderer,
    gpu_view: GpuViewport,
    egui: egui_winit::State,
}

/// Показ окна: оно создаётся невидимым (белого прямоугольника системы до темы не видно) и
/// показывается, когда в нём уже нарисованы кадры в цветах темы. Невидимое окно на Windows не
/// получает `RedrawRequested` — пока оно спрятано, кадры рисуются из `about_to_wait`. Если
/// кадр так и не удаётся вывести (поверхность потеряна, драйвер), окно всё равно показывается
/// после [`Reveal::MAX_ATTEMPTS`] попыток: спрятанным навсегда оно не остаётся.
#[derive(Clone, Copy, Debug, Default)]
pub struct Reveal {
    presented: u32,
    attempts: u32,
    shown: bool,
}

impl Reveal {
    /// Сколько выведенных кадров до показа: в первом тема ставится, во втором — уже действует.
    pub const FRAMES: u32 = 2;
    /// Сколько попыток нарисовать кадр самое большее, прежде чем показать окно всё равно.
    pub const MAX_ATTEMPTS: u32 = 30;

    /// Окно ещё спрятано: кадры рисует цикл событий сам (из `about_to_wait`).
    pub fn draw_from_idle(&self) -> bool {
        !self.shown
    }

    pub fn shown(&self) -> bool {
        self.shown
    }

    /// Кадр выведен (`presented`) или не удался. `true` — показать окно сейчас.
    pub fn frame_done(&mut self, presented: bool) -> bool {
        if self.shown {
            return false;
        }
        self.attempts += 1;
        if presented {
            self.presented += 1;
        }
        if self.presented >= Self::FRAMES || self.attempts >= Self::MAX_ATTEMPTS {
            self.shown = true;
            return true;
        }
        false
    }
}

struct Runner {
    /// Запуск программы: заставка, потом окно моделей (собирается в фоне, `startup.rs`).
    boot: Boot<ModelerApp>,
    /// Текстура вида на видеокарте: окно моделей получает её, когда собрано.
    gpu_texture: Option<egui::TextureId>,
    attached: bool,
    /// Устройство wgpu готовится в фоне с первой секунды.
    warmup: Option<startup::Warmup>,
    reveal: Reveal,
    gfx: Option<Gfx>,
    error: Option<String>,
    last_frame: Instant,
    next_frame: Instant,
    /// Кадр не чаще частоты монитора.
    min_interval: Duration,
    frames: u64,
    max_frames: Option<u64>,
    title: String,
}

/// Значок программы (рисует `tools/gen_icon.py --design M_modeler`).
pub const ICON_PNG: &[u8] = include_bytes!("../assets/icon.png");

/// Значок окна: заголовок, панель задач, Alt-Tab.
fn window_icon() -> Option<winit::window::Icon> {
    let image = image::load_from_memory(ICON_PNG).ok()?.to_rgba8();
    let (w, h) = image.dimensions();
    winit::window::Icon::from_rgba(image.into_raw(), w, h).ok()
}

/// Через сколько просыпаться в покое (файл восстановления, перечитывание текстур).
const IDLE_WAKE: Duration = Duration::from_secs(1);

impl Runner {
    fn create(&mut self, event_loop: &ActiveEventLoop) -> Result<Gfx, String> {
        let mut attributes = Window::default_attributes()
            .with_title("Piska Modeler")
            .with_inner_size(LogicalSize::new(1440.0, 880.0))
            // Невидимым до первых кадров в цветах темы (см. `Reveal`).
            .with_visible(false);
        if let Some(icon) = window_icon() {
            attributes = attributes.with_window_icon(Some(icon));
        }
        // Панель задач и Alt-Tab берут «большой» значок окна — без него там пусто.
        #[cfg(windows)]
        {
            use winit::platform::windows::WindowAttributesExtWindows;
            attributes = attributes.with_taskbar_icon(window_icon());
        }
        let window = Arc::new(event_loop.create_window(attributes).map_err(|e| e.to_string())?);
        // Системные окна файлов встают над окном Modeler и блокируют его, пока открыты.
        engine_build::file_dialog::set_owner_window(native_handle(&window));
        // Устройство готово или почти готово: его готовил отдельный поток.
        let (surface, adapter, device, queue) = self.warmup.take().unwrap_or_else(startup::Warmup::start).finish(window.clone())?;
        let caps = surface.get_capabilities(&adapter);
        // egui рисует в гамма-пространстве: поверхность не-sRGB.
        let format = caps
            .formats
            .iter()
            .copied()
            .find(|f| !f.is_srgb())
            .or_else(|| caps.formats.first().copied())
            .ok_or("the window surface reports no formats")?;
        // Без ожидания вертикальной синхронизации (задержка ввода меньше); частоту держит
        // сам цикл — не чаще монитора.
        let present_mode = [wgpu::PresentMode::Mailbox, wgpu::PresentMode::Immediate]
            .into_iter()
            .find(|m| caps.present_modes.contains(m))
            .unwrap_or(wgpu::PresentMode::AutoVsync);
        let size = window.inner_size();
        let config = wgpu::SurfaceConfiguration {
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT,
            format,
            width: size.width.max(1),
            height: size.height.max(1),
            present_mode,
            alpha_mode: caps.alpha_modes[0],
            view_formats: vec![],
            desired_maximum_frame_latency: 1,
        };
        surface.configure(&device, &config);
        let ctx = egui::Context::default();
        engine_modeler::ui::kit::apply(&ctx, true);
        let max_side = device.limits().max_texture_dimension_2d as usize;
        let egui = egui_winit::State::new(ctx, egui::ViewportId::ROOT, &window, Some(window.scale_factor() as f32), None, Some(max_side));
        let mut renderer = egui_wgpu::Renderer::new(&device, format, None, 1, false);
        let gpu_view = GpuViewport::new(&device, &mut renderer);
        log::info!("piska-modeler: {} ({:?}), present mode {present_mode:?}", adapter.get_info().name, adapter.get_info().backend);
        Ok(Gfx { window, surface, config, device, queue, renderer, gpu_view, egui })
    }

    fn frame(&mut self, event_loop: &ActiveEventLoop) {
        let Some(gfx) = self.gfx.as_mut() else { return };
        let now = Instant::now();
        self.last_frame = now;
        match gfx.surface.get_current_texture() {
            Ok(frame) => {
                let view = frame.texture.create_view(&wgpu::TextureViewDescriptor::default());
                let input = gfx.egui.take_egui_input(&gfx.window);
                let ctx = gfx.egui.egui_ctx().clone();
                // Программа собрана — окно моделей получает вид на видеокарте до первого кадра.
                self.boot.poll();
                if let (Some(app), false) = (self.boot.value_mut(), self.attached) {
                    app.set_gpu_texture(self.gpu_texture);
                    self.attached = true;
                }
                let boot = &mut self.boot;
                let full = ctx.run(input, |ctx| startup::frame(boot, ctx));
                if let Some(request) = self.boot.value_mut().and_then(|app| app.take_view_request()) {
                    gfx.gpu_view.render(&gfx.device, &gfx.queue, &mut gfx.renderer, &request);
                }
                let platform = full.platform_output.clone();
                let delay = full
                    .viewport_output
                    .get(&egui::ViewportId::ROOT)
                    .map_or(Duration::MAX, |v| v.repaint_delay);
                paint_ui(&gfx.device, &gfx.queue, &mut gfx.renderer, &ctx, full, &view, [gfx.config.width, gfx.config.height]);
                gfx.egui.handle_platform_output(&gfx.window, platform);
                gfx.window.pre_present_notify();
                frame.present();
                if self.reveal.frame_done(true) {
                    gfx.window.set_visible(true);
                }
                // Следующий кадр — когда попросит egui (анимация, подсказка), но не чаще
                // монитора; в покое — раз в IDLE_WAKE. Ввод будит раньше (window_event).
                self.next_frame = now + delay.max(self.min_interval).min(IDLE_WAKE);
            }
            Err(wgpu::SurfaceError::Lost | wgpu::SurfaceError::Outdated) => {
                gfx.surface.configure(&gfx.device, &gfx.config);
                self.next_frame = now;
                if self.reveal.frame_done(false) {
                    gfx.window.set_visible(true);
                }
            }
            Err(e) => {
                if self.reveal.frame_done(false) {
                    gfx.window.set_visible(true);
                }
                log::warn!("skipping a frame: {e}");
                self.next_frame = now + self.min_interval;
            }
        }
        let title = match self.boot.value().and_then(|app| app.path().map(|p| (p.to_path_buf(), app.has_unsaved_changes()))) {
            Some((p, unsaved)) => format!("{}{} — Piska Modeler", p.display(), if unsaved { " ●" } else { "" }),
            None => "Piska Modeler".to_string(),
        };
        if title != self.title {
            gfx.window.set_title(&title);
            self.title = title;
        }
        self.frames += 1;
        if self.max_frames.is_some_and(|m| self.frames >= m) || self.boot.quit_requested() {
            event_loop.exit();
        }
    }
}

impl ApplicationHandler for Runner {
    fn resumed(&mut self, event_loop: &ActiveEventLoop) {
        if self.gfx.is_some() {
            return;
        }
        match self.create(event_loop) {
            Ok(gfx) => {
                if let Some(mhz) = gfx.window.current_monitor().and_then(|m| m.refresh_rate_millihertz()) {
                    self.min_interval = Duration::from_secs_f64(1000.0 / f64::from(mhz.max(30_000)));
                }
                self.gpu_texture = Some(gfx.gpu_view.texture_id());
                gfx.window.request_redraw();
                self.gfx = Some(gfx);
            }
            Err(e) => {
                self.error = Some(e);
                event_loop.exit();
            }
        }
    }

    fn window_event(&mut self, event_loop: &ActiveEventLoop, _id: WindowId, event: WindowEvent) {
        let Some(gfx) = self.gfx.as_mut() else { return };
        let response = gfx.egui.on_window_event(&gfx.window, &event);
        if response.repaint {
            // Ввод — кадр сразу, но не чаще монитора.
            self.next_frame = self.next_frame.min(self.last_frame + self.min_interval);
        }
        match event {
            WindowEvent::CloseRequested => event_loop.exit(),
            WindowEvent::Resized(size) => {
                gfx.config.width = size.width.max(1);
                gfx.config.height = size.height.max(1);
                gfx.surface.configure(&gfx.device, &gfx.config);
                self.next_frame = Instant::now();
            }
            WindowEvent::RedrawRequested => self.frame(event_loop),
            _ => {}
        }
    }

    fn about_to_wait(&mut self, event_loop: &ActiveEventLoop) {
        // Невидимое окно не получает RedrawRequested: первые кадры рисуются отсюда.
        if self.gfx.is_some() && self.reveal.draw_from_idle() {
            self.frame(event_loop);
            event_loop.set_control_flow(ControlFlow::Poll);
            return;
        }
        let Some(gfx) = &self.gfx else { return };
        if Instant::now() >= self.next_frame {
            gfx.window.request_redraw();
            // Следующий кадр назначит сам кадр; до него — ждать событий, не крутиться.
            self.next_frame = Instant::now() + IDLE_WAKE;
            event_loop.set_control_flow(ControlFlow::Wait);
        } else {
            event_loop.set_control_flow(ControlFlow::WaitUntil(self.next_frame));
        }
    }
}

/// HWND окна на Windows (владелец системных окон «Открыть» / «Сохранить»).
fn native_handle(window: &Window) -> Option<isize> {
    use winit::raw_window_handle::{HasWindowHandle, RawWindowHandle};
    match window.window_handle().ok()?.as_raw() {
        RawWindowHandle::Win32(handle) => Some(handle.hwnd.get()),
        _ => None,
    }
}

/// Открыть окно и работать, пока его не закроют. `ENGINE_FRAMES=N` — закрыться после N кадров.
pub fn run_window(o: &Options) -> Result<ModelerApp, String> {
    // Видеокарта готовится с первой секунды, пока создаются цикл событий и окно.
    let warmup = startup::Warmup::start();
    let event_loop = EventLoop::new().map_err(|e| format!("cannot open a window: {e}"))?;
    event_loop.set_control_flow(ControlFlow::Wait);
    let mut runner = Runner {
        boot: startup::start(o.clone()),
        gpu_texture: None,
        attached: false,
        warmup: Some(warmup),
        reveal: Reveal::default(),
        gfx: None,
        error: None,
        last_frame: Instant::now(),
        next_frame: Instant::now(),
        min_interval: Duration::from_secs_f64(1.0 / 144.0),
        frames: 0,
        max_frames: std::env::var("ENGINE_FRAMES").ok().and_then(|v| v.parse().ok()),
        title: String::new(),
    };
    event_loop.run_app(&mut runner).map_err(|e| e.to_string())?;
    if let Some(e) = runner.error {
        return Err(e);
    }
    if let Some(message) = runner.boot.error() {
        return Err(message.to_string());
    }
    // Окно закрыли, пока программа собиралась: сохранять нечего.
    Ok(runner.boot.take_value().unwrap_or_else(ModelerApp::new))
}

#[cfg(test)]
mod reveal_tests {
    use super::Reveal;

    /// Цикл событий спрятанного окна: `RedrawRequested` не приходит никогда, кадры — только из
    /// `about_to_wait`. Окно обязано показаться при любом исходе кадров.
    fn shows_within(outcomes: impl Fn(u32) -> bool, limit: u32) -> Option<u32> {
        let mut reveal = Reveal::default();
        for step in 0..limit {
            if !reveal.draw_from_idle() {
                return Some(step);
            }
            reveal.frame_done(outcomes(step));
        }
        reveal.shown().then_some(limit)
    }

    #[test]
    fn the_window_shows_after_two_themed_frames() {
        assert_eq!(shows_within(|_| true, 100), Some(Reveal::FRAMES));
    }

    #[test]
    fn the_window_never_stays_hidden_even_if_no_frame_presents() {
        assert_eq!(shows_within(|_| false, 1000), Some(Reveal::MAX_ATTEMPTS));
        // Кадры то удаются, то нет.
        let shown = shows_within(|step| step % 3 == 0, 1000).expect("the window is shown");
        assert!(shown <= Reveal::MAX_ATTEMPTS);
    }

    #[test]
    fn once_shown_it_is_not_shown_again() {
        let mut reveal = Reveal::default();
        assert!(!reveal.frame_done(true));
        assert!(reveal.frame_done(true));
        assert!(!reveal.frame_done(true), "set_visible(true) is asked once");
        assert!(!reveal.draw_from_idle(), "after showing, frames come from RedrawRequested again");
    }
}
