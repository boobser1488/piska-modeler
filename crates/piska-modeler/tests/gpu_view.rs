//! Вид на видеокарте рисует то же, что программный растеризатор; кисть выгружает только
//! изменённые тексели; без изменений кадр не рисуется. Один тест — одно устройство.

use std::sync::Arc;

use engine_modeler::camera::{Camera, ViewPreset};
use engine_modeler::geometry::{build_mesh, BuildOptions};
use engine_modeler::gpu::GpuViewport;
use engine_modeler::raster::{RenderOptions, Shading, ViewRequest};
use engine_modeler::{paint, samples};

#[test]
fn gpu_view_matches_the_software_rasterizer() {
    let instance = wgpu::Instance::new(wgpu::InstanceDescriptor::default());
    let Ok((_, device, queue)) = piska_modeler::request_device(&instance, None) else {
        eprintln!("no GPU: skipped");
        return;
    };
    let mut egui_renderer = egui_wgpu::Renderer::new(&device, wgpu::TextureFormat::Rgba8Unorm, None, 1, false);
    let mut gpu = GpuViewport::new(&device, &mut egui_renderer);

    let mut doc = samples::rover();
    let mesh = Arc::new(build_mesh(&doc, BuildOptions::default()));
    let mut camera = Camera::default();
    let (min, max) = doc.model_bounds().unwrap();
    camera.frame(min, max);
    let hull = doc.find_by_name("hull").unwrap();
    let mut request = ViewRequest {
        doc: doc.clone(),
        mesh: mesh.clone(),
        geometry_rev: 1,
        camera,
        options: RenderOptions { highlight: vec![hull], ..RenderOptions::default() },
        size: [480, 320],
    };

    let compare = |gpu: &GpuViewport, request: &ViewRequest, label: &str| {
        let (texture, size) = gpu.output().unwrap();
        let image = piska_modeler::read_texture(&device, &queue, texture, size);
        let reference = request.render_software();
        let (mut different, mut sum) = (0usize, 0u64);
        for (i, p) in image.pixels().enumerate() {
            let q = reference.pixels[i];
            let d = (0..3).map(|c| (p[c] as i32 - q[c] as i32).unsigned_abs()).max().unwrap();
            sum += d as u64;
            if d > 24 {
                different += 1;
            }
        }
        let n = image.pixels().len();
        let mean = sum as f32 / n as f32;
        // Отличаются только края (MSAA против резкого края растеризатора).
        assert!(different * 100 < n * 3, "{label}: {different} of {n} pixels differ");
        assert!(mean < 3.0, "{label}: mean difference {mean}");
    };

    gpu.render(&device, &queue, &mut egui_renderer, &request);
    compare(&gpu, &request, "textured perspective");

    request.options.shading = Shading::Solid;
    request.camera.set_preset(ViewPreset::Front);
    request.camera.orthographic = true;
    gpu.render(&device, &queue, &mut egui_renderer, &request);
    compare(&gpu, &request, "solid orthographic");
    let uploads = gpu.stats.mesh_uploads;

    // Штрих кисти: меш не пересобирается, выгружается только прямоугольник штриха.
    request.options.shading = Shading::Textured;
    gpu.render(&device, &queue, &mut egui_renderer, &request);
    let (mesh_uploads, texels) = (gpu.stats.mesh_uploads, gpu.stats.texels_uploaded);
    assert_eq!(mesh_uploads, uploads + 1, "the shading mode rebuilds the vertices once");
    paint::line(&mut doc.textures[0].image, (2, 2), (6, 2), 1, [255, 0, 255, 255], None);
    request.doc = doc.clone();
    gpu.render(&device, &queue, &mut egui_renderer, &request);
    assert_eq!(gpu.stats.mesh_uploads, mesh_uploads, "a paint stroke does not rebuild the mesh");
    assert_eq!(gpu.stats.texels_uploaded - texels, 5, "only the 5 painted texels are uploaded");
    compare(&gpu, &request, "after a paint stroke");
}
