//! На стыках граней нет чужих цветов. Со сглаживанием (MSAA) пиксель на ребре грани
//! раньше брал координаты текстуры за краем грани — там в атласе лежит соседняя грань,
//! и вдоль шва появлялись цветные точки. Программный растеризатор за край не выходит,
//! поэтому он здесь эталон: каждый пиксель вида на видеокарте обязан лежать в диапазоне
//! цветов своей окрестности в эталоне.

use std::sync::Arc;

use engine_modeler::camera::Camera;
use engine_modeler::geometry::{build_mesh, BuildOptions};
use engine_modeler::gpu::GpuViewport;
use engine_modeler::raster::{RenderOptions, ViewRequest};
use engine_modeler::samples;

/// Пиксели, чей цвет не объясняется окрестностью 5×5 эталона (с запасом `slack`).
fn foreign_pixels(image: &image::RgbaImage, reference: &engine_modeler::raster::Frame, slack: i32) -> Vec<(u32, u32)> {
    let (w, h) = (reference.width as i32, reference.height as i32);
    let mut found = Vec::new();
    for y in 0..h {
        for x in 0..w {
            let p = image.get_pixel(x as u32, y as u32);
            let (mut lo, mut hi) = ([255i32; 3], [0i32; 3]);
            for dy in -2..=2 {
                for dx in -2..=2 {
                    let (nx, ny) = ((x + dx).clamp(0, w - 1), (y + dy).clamp(0, h - 1));
                    let q = reference.pixels[(ny * w + nx) as usize];
                    for c in 0..3 {
                        lo[c] = lo[c].min(q[c] as i32);
                        hi[c] = hi[c].max(q[c] as i32);
                    }
                }
            }
            if (0..3).any(|c| (p[c] as i32) < lo[c] - slack || (p[c] as i32) > hi[c] + slack) {
                found.push((x as u32, y as u32));
            }
        }
    }
    found
}

#[test]
fn face_edges_do_not_bleed_neighbouring_texels() {
    let instance = wgpu::Instance::new(wgpu::InstanceDescriptor::default());
    let Ok((_, device, queue)) = piska_modeler::request_device(&instance, None) else {
        eprintln!("no GPU: skipped");
        return;
    };
    let mut egui_renderer = egui_wgpu::Renderer::new(&device, wgpu::TextureFormat::Rgba8Unorm, None, 1, false);
    let mut gpu = GpuViewport::new(&device, &mut egui_renderer);

    // Со старым шейдером (UV без centroid) на образцах набиралось 200–800 таких пикселей
    // на модель; после исправления остаются единицы — разница сглаживания тонких деталей
    // (растеризатор-эталон их края не сглаживает).
    for (index, (name, doc)) in samples::all().into_iter().enumerate() {
        let mesh = Arc::new(build_mesh(&doc, BuildOptions::default()));
        let mut camera = Camera::default();
        let (min, max) = doc.model_bounds().unwrap();
        camera.frame(min, max);
        let mut total = 0;
        for step in 0..6 {
            camera.orbit(0.37, if step % 2 == 0 { 0.11 } else { -0.07 });
            let request = ViewRequest {
                doc: doc.clone(),
                mesh: mesh.clone(),
                // Своя ревизия на модель: по ней вид решает, перезагружать ли вершины.
                geometry_rev: index as u64 + 1,
                camera,
                options: RenderOptions { grid: false, ..RenderOptions::default() },
                size: [640, 420],
            };
            gpu.render(&device, &queue, &mut egui_renderer, &request);
            let (texture, size) = gpu.output().unwrap();
            let image = piska_modeler::read_texture(&device, &queue, texture, size);
            let reference = request.render_software();
            let foreign = foreign_pixels(&image, &reference, 16);
            if let (Ok(dir), false) = (std::env::var("SEAM_DUMP"), foreign.is_empty()) {
                eprintln!("{name}, view {step}: {} pixels, first at {:?}", foreign.len(), foreign[0]);
                image.save(format!("{dir}/{name}_{step}_gpu.png")).unwrap();
            }
            total += foreign.len();
        }
        assert!(total <= 80, "{name}: {total} pixels along face edges have a colour from a neighbouring face");
    }
}
