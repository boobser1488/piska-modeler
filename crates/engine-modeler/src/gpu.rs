//! Вид модели на видеокарте (фича `gpu`): тот же кадр, что у программного растеризатора
//! ([`crate::raster`]), но за доли миллисекунды и с MSAA 4×.
//!
//! Хозяин окна (программа `piska-modeler`, редактор) владеет устройством wgpu и
//! рендерером egui. Порядок кадра:
//!
//! ```text
//! let mut gpu = GpuViewport::new(&device, &mut egui_renderer);
//! app.set_gpu_texture(Some(gpu.texture_id()));            // один раз
//! // каждый кадр:
//! let output = ctx.run(input, |ctx| …app.show(ui)…);       // интерфейс
//! if let Some(request) = app.take_view_request() {          // только если вид изменился
//!     gpu.render(&device, &queue, &mut egui_renderer, &request);
//! }
//! // затем отрисовка egui как обычно
//! ```
//!
//! Буферы меша пересобираются, только когда меняется геометрия (или выделение — оно
//! подкрашивает вершины); текстуры после штриха кисти выгружаются частично — только
//! прямоугольник изменившихся текселей.

use crate::image::Image;
use crate::raster::{face_light, Shading, ViewRequest};

/// Формат кадра: «байтовые» значения цвета без перевода в линейный цвет.
const TARGET_FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Rgba8Unorm;
/// То же, как его читает egui (он ждёт sRGB-текстуру и сам кодирует обратно).
const VIEW_FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Rgba8UnormSrgb;
const DEPTH_FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Depth32Float;
/// Сглаживание краёв.
pub const SAMPLES: u32 = 4;
/// Чисел f32 в вершине: позиция 3, UV 2, оттенок 4, [свет, подсветка, без текстуры, 0] 4.
const VERTEX_FLOATS: usize = 13;
/// Чисел f32 в униформах: две матрицы и шесть векторов.
const GLOBAL_FLOATS: usize = 16 * 2 + 4 * 6;

fn bytes(values: &[f32]) -> Vec<u8> {
    values.iter().flat_map(|v| v.to_le_bytes()).collect()
}

/// Текстура документа на видеокарте и её копия на процессоре (что уже выгружено).
struct GpuTexture {
    texture: wgpu::Texture,
    bind_group: wgpu::BindGroup,
    uploaded: Image,
}

/// Кадр нужного размера: MSAA-цвет, глубина и итоговая текстура для egui.
struct Target {
    size: [u32; 2],
    msaa: wgpu::TextureView,
    depth: wgpu::TextureView,
    resolve: wgpu::Texture,
    resolve_view: wgpu::TextureView,
}

/// Счётчики работы (для замеров и тестов).
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct GpuStats {
    pub frames: u64,
    /// Сколько раз пересобирались буферы вершин.
    pub mesh_uploads: u64,
    /// Сколько текселей выгружено за всё время (частичные выгрузки — меньше атласа).
    pub texels_uploaded: u64,
    /// Время записи команд последнего кадра на процессоре, мс.
    pub encode_ms: f32,
}

pub struct GpuViewport {
    mesh_pipeline: wgpu::RenderPipeline,
    background_pipeline: wgpu::RenderPipeline,
    grid_pipeline: wgpu::RenderPipeline,
    globals: wgpu::Buffer,
    globals_group: wgpu::BindGroup,
    texture_layout: wgpu::BindGroupLayout,
    textures: Vec<GpuTexture>,
    white: GpuTexture,
    vertices: Option<wgpu::Buffer>,
    indices: Option<wgpu::Buffer>,
    /// (текстура, первый индекс, число индексов); `None` — белая (опорные меши).
    draws: Vec<(Option<usize>, u32, u32)>,
    mesh_key: Option<(u64, Vec<crate::doc::NodeId>, Shading)>,
    white_image: Image,
    target: Option<Target>,
    texture_id: egui::TextureId,
    pub stats: GpuStats,
}

impl GpuViewport {
    /// Создать конвейеры и зарегистрировать кадр в egui (пока 1×1).
    pub fn new(device: &wgpu::Device, egui_renderer: &mut egui_wgpu::Renderer) -> Self {
        let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("modeler view"),
            source: wgpu::ShaderSource::Wgsl(include_str!("gpu.wgsl").into()),
        });
        let globals_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("modeler globals"),
            entries: &[wgpu::BindGroupLayoutEntry {
                binding: 0,
                visibility: wgpu::ShaderStages::VERTEX_FRAGMENT,
                ty: wgpu::BindingType::Buffer {
                    ty: wgpu::BufferBindingType::Uniform,
                    has_dynamic_offset: false,
                    min_binding_size: None,
                },
                count: None,
            }],
        });
        let texture_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("modeler atlas"),
            entries: &[wgpu::BindGroupLayoutEntry {
                binding: 0,
                visibility: wgpu::ShaderStages::FRAGMENT,
                ty: wgpu::BindingType::Texture {
                    sample_type: wgpu::TextureSampleType::Float { filterable: false },
                    view_dimension: wgpu::TextureViewDimension::D2,
                    multisampled: false,
                },
                count: None,
            }],
        });
        let globals = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("modeler globals"),
            size: (GLOBAL_FLOATS * 4) as u64,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let globals_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("modeler globals"),
            layout: &globals_layout,
            entries: &[wgpu::BindGroupEntry { binding: 0, resource: globals.as_entire_binding() }],
        });
        let layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("modeler view"),
            bind_group_layouts: &[&globals_layout, &texture_layout],
            push_constant_ranges: &[],
        });
        let multisample = wgpu::MultisampleState { count: SAMPLES, mask: !0, alpha_to_coverage_enabled: false };
        let color = |blend| {
            [Some(wgpu::ColorTargetState { format: TARGET_FORMAT, blend, write_mask: wgpu::ColorWrites::ALL })]
        };
        let opaque = color(None);
        let blended = color(Some(wgpu::BlendState::PREMULTIPLIED_ALPHA_BLENDING));
        let pipeline = |label: &str,
                        vs: &str,
                        fs: &str,
                        buffers: &[wgpu::VertexBufferLayout],
                        targets: &[Option<wgpu::ColorTargetState>],
                        cull: Option<wgpu::Face>,
                        depth_write: bool,
                        compare: wgpu::CompareFunction| {
            device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
                label: Some(label),
                layout: Some(&layout),
                vertex: wgpu::VertexState {
                    module: &shader,
                    entry_point: Some(vs),
                    buffers,
                    compilation_options: wgpu::PipelineCompilationOptions::default(),
                },
                fragment: Some(wgpu::FragmentState {
                    module: &shader,
                    entry_point: Some(fs),
                    targets,
                    compilation_options: wgpu::PipelineCompilationOptions::default(),
                }),
                primitive: wgpu::PrimitiveState {
                    topology: wgpu::PrimitiveTopology::TriangleList,
                    front_face: wgpu::FrontFace::Ccw,
                    cull_mode: cull,
                    ..wgpu::PrimitiveState::default()
                },
                depth_stencil: Some(wgpu::DepthStencilState {
                    format: DEPTH_FORMAT,
                    depth_write_enabled: depth_write,
                    depth_compare: compare,
                    stencil: wgpu::StencilState::default(),
                    bias: wgpu::DepthBiasState::default(),
                }),
                multisample,
                multiview: None,
                cache: None,
            })
        };
        let vertex_layout = wgpu::VertexBufferLayout {
            array_stride: (VERTEX_FLOATS * 4) as u64,
            step_mode: wgpu::VertexStepMode::Vertex,
            attributes: &wgpu::vertex_attr_array![0 => Float32x3, 1 => Float32x2, 2 => Float32x4, 3 => Float32x4],
        };
        let mesh_pipeline = pipeline(
            "modeler mesh",
            "vs_mesh",
            "fs_mesh",
            &[vertex_layout],
            &opaque,
            Some(wgpu::Face::Back),
            true,
            wgpu::CompareFunction::Less,
        );
        let background_pipeline = pipeline(
            "modeler background",
            "vs_screen",
            "fs_background",
            &[],
            &opaque,
            None,
            false,
            wgpu::CompareFunction::Always,
        );
        let grid_pipeline = pipeline(
            "modeler grid",
            "vs_screen",
            "fs_grid",
            &[],
            &blended,
            None,
            false,
            wgpu::CompareFunction::Less,
        );
        let white = Self::make_texture(device, &texture_layout, &Image::new(1, 1, [255, 255, 255, 255]));
        let mut viewport = Self {
            mesh_pipeline,
            background_pipeline,
            grid_pipeline,
            globals,
            globals_group,
            texture_layout,
            textures: Vec::new(),
            white,
            vertices: None,
            indices: None,
            draws: Vec::new(),
            mesh_key: None,
            white_image: Image::new(1, 1, [255, 255, 255, 255]),
            target: None,
            texture_id: egui::TextureId::default(),
            stats: GpuStats::default(),
        };
        let target = viewport.make_target(device, [1, 1]);
        viewport.texture_id =
            egui_renderer.register_native_texture(device, &target.view_srgb(), wgpu::FilterMode::Nearest);
        viewport.target = Some(target);
        viewport
    }

    /// Текстура кадра в egui (не меняется при смене размера).
    pub fn texture_id(&self) -> egui::TextureId {
        self.texture_id
    }

    /// Итоговая текстура последнего кадра (для снимков и тестов).
    pub fn output(&self) -> Option<(&wgpu::Texture, [u32; 2])> {
        self.target.as_ref().map(|t| (&t.resolve, t.size))
    }

    fn make_texture(device: &wgpu::Device, layout: &wgpu::BindGroupLayout, image: &Image) -> GpuTexture {
        let texture = device.create_texture(&wgpu::TextureDescriptor {
            label: Some("modeler texture"),
            size: wgpu::Extent3d { width: image.width(), height: image.height(), depth_or_array_layers: 1 },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: wgpu::TextureFormat::Rgba8Unorm,
            usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
            view_formats: &[],
        });
        let view = texture.create_view(&wgpu::TextureViewDescriptor::default());
        let bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("modeler texture"),
            layout,
            entries: &[wgpu::BindGroupEntry { binding: 0, resource: wgpu::BindingResource::TextureView(&view) }],
        });
        // Пустая копия: первая выгрузка будет полной.
        GpuTexture { texture, bind_group, uploaded: Image::new(1, 1, [0, 0, 0, 0]) }
    }

    fn make_target(&self, device: &wgpu::Device, size: [u32; 2]) -> Target {
        let extent = wgpu::Extent3d { width: size[0], height: size[1], depth_or_array_layers: 1 };
        let make = |label, format, samples, usage, view_formats: &[wgpu::TextureFormat]| {
            device.create_texture(&wgpu::TextureDescriptor {
                label: Some(label),
                size: extent,
                mip_level_count: 1,
                sample_count: samples,
                dimension: wgpu::TextureDimension::D2,
                format,
                usage,
                view_formats,
            })
        };
        let msaa = make("modeler msaa", TARGET_FORMAT, SAMPLES, wgpu::TextureUsages::RENDER_ATTACHMENT, &[]);
        let depth = make("modeler depth", DEPTH_FORMAT, SAMPLES, wgpu::TextureUsages::RENDER_ATTACHMENT, &[]);
        let resolve = make(
            "modeler view",
            TARGET_FORMAT,
            1,
            wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_SRC,
            &[VIEW_FORMAT],
        );
        Target {
            size,
            msaa: msaa.create_view(&wgpu::TextureViewDescriptor::default()),
            depth: depth.create_view(&wgpu::TextureViewDescriptor::default()),
            resolve_view: resolve.create_view(&wgpu::TextureViewDescriptor::default()),
            resolve,
        }
    }

    /// Выгрузить изменившиеся тексели (прямоугольник изменений, а не всю картинку).
    fn sync_texture(queue: &wgpu::Queue, gpu: &mut GpuTexture, image: &Image, stats: &mut GpuStats) {
        if gpu.uploaded.shares_pixels(image) {
            return;
        }
        let [w, h] = image.size();
        let full = gpu.uploaded.size() != image.size();
        let (mut x0, mut y0, mut x1, mut y1) = (0, 0, w, h);
        if !full {
            // Границы изменившихся текселей.
            let (old, new) = (gpu.uploaded.pixels(), image.pixels());
            let (mut minx, mut miny, mut maxx, mut maxy) = (w, h, 0, 0);
            for y in 0..h {
                let row = (y * w) as usize;
                if old[row..row + w as usize] == new[row..row + w as usize] {
                    continue;
                }
                miny = miny.min(y);
                maxy = y + 1;
                for x in 0..w {
                    if old[row + x as usize] != new[row + x as usize] {
                        minx = minx.min(x);
                        maxx = maxx.max(x + 1);
                    }
                }
            }
            if maxx <= minx || maxy <= miny {
                gpu.uploaded = image.clone();
                return;
            }
            (x0, y0, x1, y1) = (minx, miny, maxx, maxy);
        }
        let (rw, rh) = (x1 - x0, y1 - y0);
        let mut data = Vec::with_capacity((rw * rh * 4) as usize);
        for y in y0..y1 {
            let row = (y * w + x0) as usize;
            for p in &image.pixels()[row..row + rw as usize] {
                data.extend_from_slice(p);
            }
        }
        queue.write_texture(
            wgpu::ImageCopyTexture {
                texture: &gpu.texture,
                mip_level: 0,
                origin: wgpu::Origin3d { x: x0, y: y0, z: 0 },
                aspect: wgpu::TextureAspect::All,
            },
            &data,
            wgpu::ImageDataLayout { offset: 0, bytes_per_row: Some(rw * 4), rows_per_image: Some(rh) },
            wgpu::Extent3d { width: rw, height: rh, depth_or_array_layers: 1 },
        );
        stats.texels_uploaded += (rw * rh) as u64;
        gpu.uploaded = image.clone();
    }

    /// Собрать вершины и индексы: треугольники по текстурам, затем опорные меши.
    fn rebuild_mesh(&mut self, device: &wgpu::Device, request: &ViewRequest) {
        let doc = &request.doc;
        let mesh = &request.mesh;
        let mut vertices: Vec<f32> = Vec::with_capacity(mesh.vertices.len() * VERTEX_FLOATS);
        for v in &mesh.vertices {
            vertices.extend_from_slice(&[v.position.x, v.position.y, v.position.z, v.uv.x, v.uv.y]);
            vertices.extend(v.color.map(|c| c as f32 / 255.0));
            vertices.extend_from_slice(&[0.0, 0.0, 0.0, 0.0]); // свет и подсветка — ниже, по треугольникам
        }
        let texture_count = doc.textures.len().max(1);
        let mut by_texture: Vec<Vec<u32>> = vec![Vec::new(); texture_count];
        if request.options.shading != Shading::Wireframe {
            for t in &mesh.triangles {
                let light = if t.emissive { 1.0 } else { face_light(mesh.vertices[t.indices[0] as usize].normal) };
                let highlight = if request.options.highlight.contains(&t.node) { 1.0 } else { 0.0 };
                for i in t.indices {
                    let base = i as usize * VERTEX_FLOATS;
                    vertices[base + 9] = light;
                    vertices[base + 10] = highlight;
                }
                let slot = (t.texture as usize).min(texture_count - 1);
                by_texture[slot].extend(t.indices);
            }
        }
        let mut indices: Vec<u32> = Vec::new();
        self.draws.clear();
        for (slot, list) in by_texture.into_iter().enumerate() {
            if !list.is_empty() {
                self.draws.push((Some(slot), indices.len() as u32, list.len() as u32));
                indices.extend(list);
            }
        }
        // Опорные меши: светло-серые, видны с обеих сторон (оба обхода).
        let start = indices.len() as u32;
        for reference in doc.references.iter().filter(|r| r.visible) {
            for tri in reference.indices.chunks_exact(3) {
                let p = [0, 1, 2].map(|k| glam::Vec3::from_array(reference.positions[tri[k] as usize]));
                let n = (p[1] - p[0]).cross(p[2] - p[0]).normalize_or_zero();
                let shade = (150.0 * face_light(n)) as u8;
                let color = [shade, shade.saturating_add(4), shade.saturating_add(12)].map(|c| c as f32 / 255.0);
                for order in [[0, 1, 2], [0, 2, 1]] {
                    for k in order {
                        let q = p[k];
                        indices.push((vertices.len() / VERTEX_FLOATS) as u32);
                        vertices.extend_from_slice(&[q.x, q.y, q.z, 0.0, 0.0, color[0], color[1], color[2], 1.0, 1.0, 0.0, 1.0, 0.0]);
                    }
                }
            }
        }
        if indices.len() as u32 > start {
            self.draws.push((None, start, indices.len() as u32 - start));
        }
        let make = |data: Vec<u8>, usage| {
            if data.is_empty() {
                return None;
            }
            let buffer = device.create_buffer(&wgpu::BufferDescriptor {
                label: Some("modeler mesh"),
                size: data.len() as u64,
                usage,
                mapped_at_creation: true,
            });
            buffer.slice(..).get_mapped_range_mut().copy_from_slice(&data);
            buffer.unmap();
            Some(buffer)
        };
        self.vertices = make(bytes(&vertices), wgpu::BufferUsages::VERTEX);
        self.indices = make(indices.iter().flat_map(|i| i.to_le_bytes()).collect(), wgpu::BufferUsages::INDEX);
        self.stats.mesh_uploads += 1;
    }

    /// Нарисовать кадр по запросу окна моделей.
    pub fn render(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        egui_renderer: &mut egui_wgpu::Renderer,
        request: &ViewRequest,
    ) {
        let started = std::time::Instant::now();
        let size = [request.size[0].max(1), request.size[1].max(1)];
        if self.target.as_ref().is_none_or(|t| t.size != size) {
            let target = self.make_target(device, size);
            egui_renderer.update_egui_texture_from_wgpu_texture(
                device,
                &target.view_srgb(),
                wgpu::FilterMode::Nearest,
                self.texture_id,
            );
            self.target = Some(target);
        }
        // Текстуры документа.
        let doc = &request.doc;
        while self.textures.len() < doc.textures.len() {
            let image = &doc.textures[self.textures.len()].image;
            self.textures.push(Self::make_texture(device, &self.texture_layout, image));
        }
        self.textures.truncate(doc.textures.len());
        for (i, texture) in doc.textures.iter().enumerate() {
            let size = self.textures[i].texture.size();
            if [size.width, size.height] != texture.image.size() {
                self.textures[i] = Self::make_texture(device, &self.texture_layout, &texture.image);
            }
            Self::sync_texture(queue, &mut self.textures[i], &texture.image, &mut self.stats);
        }
        let white = self.white_image.clone();
        Self::sync_texture(queue, &mut self.white, &white, &mut self.stats);
        // Меш — только если изменилась геометрия, выделение или режим показа.
        let key = (request.geometry_rev, request.options.highlight.clone(), request.options.shading);
        if self.mesh_key.as_ref() != Some(&key) {
            self.rebuild_mesh(device, request);
            self.mesh_key = Some(key);
        }

        // Униформы.
        let aspect = size[0] as f32 / size[1] as f32;
        let view_proj = request.camera.view_proj(aspect);
        let inverse = view_proj.inverse();
        let step = request.options.grid_step.max(1e-3);
        let major = step * request.options.major_every.max(1) as f32;
        let fade_radius = request.camera.distance * 3.0 + major * 4.0;
        let rgb = |c: [u8; 4]| [c[0] as f32 / 255.0, c[1] as f32 / 255.0, c[2] as f32 / 255.0, 1.0];
        let mut globals: Vec<f32> = Vec::with_capacity(GLOBAL_FLOATS);
        globals.extend(view_proj.to_cols_array());
        globals.extend(inverse.to_cols_array());
        globals.extend([size[0] as f32, size[1] as f32, step, major]);
        let target = request.camera.target;
        globals.extend([target.x, target.y, target.z, fade_radius]);
        globals.extend(rgb(request.options.background[0]));
        globals.extend(rgb(request.options.background[1]));
        globals.extend(rgb(request.options.highlight_color));
        let solid = if request.options.shading == Shading::Solid { 1.0 } else { 0.0 };
        let below = if request.camera.eye().y < 0.0 { 1.0 } else { 0.0 };
        globals.extend([solid, below, 0.0, 0.0]);
        queue.write_buffer(&self.globals, 0, &bytes(&globals));

        let target = self.target.as_ref().expect("created above");
        let mut encoder = device.create_command_encoder(&wgpu::CommandEncoderDescriptor { label: Some("modeler view") });
        {
            let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("modeler view"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: &target.msaa,
                    resolve_target: Some(&target.resolve_view),
                    ops: wgpu::Operations { load: wgpu::LoadOp::Clear(wgpu::Color::BLACK), store: wgpu::StoreOp::Discard },
                })],
                depth_stencil_attachment: Some(wgpu::RenderPassDepthStencilAttachment {
                    view: &target.depth,
                    depth_ops: Some(wgpu::Operations { load: wgpu::LoadOp::Clear(1.0), store: wgpu::StoreOp::Discard }),
                    stencil_ops: None,
                }),
                timestamp_writes: None,
                occlusion_query_set: None,
            });
            pass.set_bind_group(0, &self.globals_group, &[]);
            pass.set_bind_group(1, &self.white.bind_group, &[]);
            pass.set_pipeline(&self.background_pipeline);
            pass.draw(0..3, 0..1);
            if let (Some(vertices), Some(indices)) = (&self.vertices, &self.indices) {
                pass.set_pipeline(&self.mesh_pipeline);
                pass.set_vertex_buffer(0, vertices.slice(..));
                pass.set_index_buffer(indices.slice(..), wgpu::IndexFormat::Uint32);
                for (slot, first, count) in &self.draws {
                    let group = match slot {
                        Some(i) => &self.textures[*i].bind_group,
                        None => &self.white.bind_group,
                    };
                    pass.set_bind_group(1, group, &[]);
                    pass.draw_indexed(*first..first + count, 0, 0..1);
                }
            }
            if request.options.grid {
                pass.set_bind_group(1, &self.white.bind_group, &[]);
                pass.set_pipeline(&self.grid_pipeline);
                pass.draw(0..3, 0..1);
            }
        }
        queue.submit(Some(encoder.finish()));
        self.stats.frames += 1;
        self.stats.encode_ms = started.elapsed().as_secs_f32() * 1000.0;
    }
}

impl Target {
    /// Представление итоговой текстуры, которое ждёт egui.
    fn view_srgb(&self) -> wgpu::TextureView {
        self.resolve.create_view(&wgpu::TextureViewDescriptor { format: Some(VIEW_FORMAT), ..Default::default() })
    }
}
