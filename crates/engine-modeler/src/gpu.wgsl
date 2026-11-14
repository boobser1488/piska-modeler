// Вид Piska Modeler на видеокарте. Повторяет программный растеризатор (raster.rs):
// вся арифметика — в «байтовом» пространстве цвета (0…1 = 0…255 без перевода в линейный
// цвет), поэтому цель — Rgba8Unorm, а egui читает её через sRGB-представление.

struct Globals {
    view_proj: mat4x4<f32>,
    inverse: mat4x4<f32>,
    // x, y — размер кадра в пикселях; z — шаг сетки; w — крупный шаг.
    size: vec4<f32>,
    // xyz — цель камеры; w — радиус угасания сетки.
    focus: vec4<f32>,
    background_top: vec4<f32>,
    background_bottom: vec4<f32>,
    highlight: vec4<f32>,
    // x — 1: сплошная заливка без текстур; y — 1: камера снизу.
    flags: vec4<f32>,
};

@group(0) @binding(0) var<uniform> globals: Globals;
@group(1) @binding(0) var atlas: texture_2d<f32>;

struct MeshIn {
    @location(0) position: vec3<f32>,
    @location(1) uv: vec2<f32>,
    @location(2) tint: vec4<f32>,
    // x — свет стороны, y — 1: подсветка выделения, z — 1: без текстуры (опорный меш).
    @location(3) shade: vec4<f32>,
};

struct MeshOut {
    @builtin(position) position: vec4<f32>,
    // centroid: со сглаживанием (MSAA) пиксель на ребре грани берёт UV внутри грани, а не
    // продолжает их за её край, где в атласе лежит уже чужая грань (цветные точки на швах).
    @location(0) @interpolate(perspective, centroid) uv: vec2<f32>,
    @location(1) tint: vec4<f32>,
    @location(2) shade: vec4<f32>,
};

@vertex
fn vs_mesh(v: MeshIn) -> MeshOut {
    var out: MeshOut;
    out.position = globals.view_proj * vec4<f32>(v.position, 1.0);
    out.uv = v.uv;
    out.tint = v.tint;
    out.shade = v.shade;
    return out;
}

@fragment
fn fs_mesh(v: MeshOut) -> @location(0) vec4<f32> {
    var base = vec4<f32>(180.0, 178.0, 172.0, 255.0) / 255.0;
    if (v.shade.z > 0.5) {
        base = vec4<f32>(1.0);
    } else if (globals.flags.x < 0.5) {
        // Тексель «ближайшего соседа» — как в растеризаторе (floor и прижим к краю).
        let size = vec2<f32>(textureDimensions(atlas));
        let texel = clamp(floor(v.uv * size), vec2<f32>(0.0), size - 1.0);
        base = textureLoad(atlas, vec2<i32>(texel), 0);
        if (base.a < 0.5) {
            discard;
        }
    }
    var rgb = base.rgb * v.tint.rgb * v.shade.x;
    if (v.shade.y > 0.5) {
        rgb = rgb * 0.82 + globals.highlight.rgb * 0.18;
    }
    return vec4<f32>(min(rgb, vec3<f32>(1.0)), 1.0);
}

struct ScreenOut {
    @builtin(position) position: vec4<f32>,
};

// Один треугольник на весь кадр.
@vertex
fn vs_screen(@builtin(vertex_index) index: u32) -> ScreenOut {
    let x = f32((index << 1u) & 2u);
    let y = f32(index & 2u);
    var out: ScreenOut;
    out.position = vec4<f32>(x * 2.0 - 1.0, 1.0 - y * 2.0, 0.0, 1.0);
    return out;
}

@fragment
fn fs_background(v: ScreenOut) -> @location(0) vec4<f32> {
    let t = (v.position.y - 0.5) / max(globals.size.y - 1.0, 1.0);
    return vec4<f32>(mix(globals.background_top.rgb, globals.background_bottom.rgb, t), 1.0);
}

struct GridOut {
    @location(0) color: vec4<f32>,
    @builtin(frag_depth) depth: f32,
};

// Точка на плоскости Y = 0 под пикселем (в координатах пикселя).
fn ground(pixel: vec2<f32>) -> vec4<f32> {
    let ndc = vec2<f32>(pixel.x / globals.size.x * 2.0 - 1.0, 1.0 - pixel.y / globals.size.y * 2.0);
    let n = globals.inverse * vec4<f32>(ndc, 0.0, 1.0);
    let f = globals.inverse * vec4<f32>(ndc, 1.0, 1.0);
    let a = n.xyz / n.w;
    let b = f.xyz / f.w;
    let dir = b - a;
    if (abs(dir.y) < 1e-9) {
        return vec4<f32>(0.0, 0.0, 0.0, -1.0);
    }
    let t = -a.y / dir.y;
    if (t < 0.0 || t > 1.0) {
        return vec4<f32>(0.0, 0.0, 0.0, -1.0);
    }
    return vec4<f32>(a + dir * t, 1.0);
}

fn line(p: vec3<f32>, s: f32, fw: vec2<f32>) -> vec2<f32> {
    let d = vec2<f32>(
        abs(fract(p.x / s + 0.5) - 0.5) * s,
        abs(fract(p.z / s + 0.5) - 0.5) * s,
    );
    return clamp(vec2<f32>(1.0) - d / fw, vec2<f32>(0.0), vec2<f32>(1.0));
}

@fragment
fn fs_grid(v: ScreenOut) -> GridOut {
    let pixel = v.position.xy;
    let g = ground(pixel);
    if (g.w < 0.0) {
        discard;
    }
    // Производные по соседним пикселям — как у растеризатора (без квадов 2×2 и их
    // различий между производителями).
    let gx = ground(pixel + vec2<f32>(1.0, 0.0));
    let gy = ground(pixel + vec2<f32>(0.0, 1.0));
    if (gx.w < 0.0 || gy.w < 0.0) {
        discard;
    }
    let p = g.xyz;
    let fw = max(
        vec2<f32>(abs(gx.x - p.x) + abs(gy.x - p.x), abs(gx.z - p.z) + abs(gy.z - p.z)),
        vec2<f32>(1e-6),
    );
    let clip = globals.view_proj * vec4<f32>(p, 1.0);
    let step = globals.size.z;
    let major = globals.size.w;
    let density = min(max(fw.x, fw.y) / step, 1.0);
    let minor_fade = clamp(1.0 - (density - 0.12) / 0.18, 0.0, 1.0);
    let m = line(p, step, fw);
    let b = line(p, major, fw);
    let minor = max(m.x, m.y) * 0.07 * minor_fade;
    let major_c = max(b.x, b.y) * 0.16;
    let dist = length(vec2<f32>(p.x - globals.focus.x, p.z - globals.focus.z));
    var fade = clamp(1.0 - dist / globals.focus.w, 0.0, 1.0);
    fade = fade * fade * select(1.0, 0.4, globals.flags.y > 0.5);
    // Последовательные смешивания растеризатора сведены к одному предумноженному цвету.
    var block = 0.0;
    if (abs(p.x) <= major * 0.5 && abs(p.z) <= major * 0.5) {
        block = 0.035 * fade;
    }
    let white = max(minor, major_c) * fade;
    let a = 1.0 - (1.0 - block) * (1.0 - white);
    let ax = clamp(1.0 - abs(p.z) / fw.y, 0.0, 1.0) * 0.75 * fade;
    let az = clamp(1.0 - abs(p.x) / fw.x, 0.0, 1.0) * 0.75 * fade;
    let red = vec3<f32>(232.0, 85.0, 85.0) / 255.0;
    let blue = vec3<f32>(79.0, 146.0, 236.0) / 255.0;
    let color = vec3<f32>(1.0) * a * (1.0 - ax) * (1.0 - az) + red * ax * (1.0 - az) + blue * az;
    let alpha = 1.0 - (1.0 - a) * (1.0 - ax) * (1.0 - az);
    var out: GridOut;
    out.color = vec4<f32>(color, alpha);
    out.depth = clip.z / clip.w;
    return out;
}
