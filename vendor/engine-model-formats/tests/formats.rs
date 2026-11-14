//! Форматы моделей на рукописных образцах (`tests/fixtures`, их пишет
//! `gen_fixtures.py` из отчёта волны; `.vox` собирается здесь по байтам).
//!
//! Проверки построены так, чтобы ловить неверные оси, обход, порядок поворотов и
//! отражения UV: модели несимметричны, ожидаемые углы и цвета записаны числами.
//! Текстура `block/faces.png` 16×16: у каждой стороны свой квадрат 4×4 своего цвета,
//! его левый верхний тексель белый, правый верхний — чёрный.

use std::path::Path;

use engine_model_formats::{bbmodel, import, poly, read_model, vox, Format, Imported, Model, Node, Options, Side, UpAxis};

const FIXTURES: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/tests/fixtures");

fn bytes(name: &str) -> Vec<u8> {
    std::fs::read(Path::new(FIXTURES).join(name)).unwrap_or_else(|e| panic!("{name}: {e}"))
}

/// Соседние файлы — только внутри каталога образцов.
fn resolve(path: &str) -> Option<Vec<u8>> {
    assert!(!path.contains(':') && !path.starts_with('/'), "unsafe path requested: {path}");
    if path.starts_with("..") {
        return None;
    }
    std::fs::read(Path::new(FIXTURES).join(path)).ok()
}

fn load(name: &str, options: &Options) -> Imported {
    import(name, &bytes(name), &resolve, options).unwrap_or_else(|e| panic!("{name}: {e}"))
}

/// Только узлы с этими именами (для изолированной проверки элемента).
fn only(model: &Model, names: &[&str]) -> Model {
    let mut m = model.clone();
    m.roots.retain(|n| names.contains(&n.name()));
    m
}

fn asym() -> Model {
    bbmodel::read("asym.bbmodel", &bytes("asym.bbmodel"), &resolve).unwrap()
}

fn mesh_of(model: &Model, options: &Options) -> Imported {
    engine_model_formats::model_to_mesh(model, options).unwrap()
}

const RED: [u8; 4] = [220, 40, 40, 255];
const GREEN: [u8; 4] = [40, 200, 40, 255];
const BLUE: [u8; 4] = [40, 60, 220, 255];
const YELLOW: [u8; 4] = [230, 210, 40, 255];
const CYAN: [u8; 4] = [40, 210, 210, 255];
const MAGENTA: [u8; 4] = [210, 40, 210, 255];
const WHITE: [u8; 4] = [255, 255, 255, 255];
const BLACK: [u8; 4] = [0, 0, 0, 255];

fn near(a: [f32; 3], b: [f32; 3]) -> bool {
    (0..3).all(|i| (a[i] - b[i]).abs() < 1e-4)
}

/// Треугольники меша: (позиции, UV, нормаль).
/// (позиции углов, UV углов, нормаль).
type Triangle = ([[f32; 3]; 3], [[f32; 2]; 3], [f32; 3]);

fn triangles(imported: &Imported) -> Vec<Triangle> {
    let m = &imported.mesh;
    m.indices
        .chunks_exact(3)
        .map(|t| {
            let i = [t[0] as usize, t[1] as usize, t[2] as usize];
            (i.map(|k| m.positions[k]), i.map(|k| m.uvs[k]), m.normals[i[0]])
        })
        .collect()
}

/// Цвет текстуры у угла `corner` стороны с нормалью `normal` (UV угла, сдвинутый к
/// центру треугольника на 2 %, чтобы взять тексель внутри стороны).
fn color_at(imported: &Imported, normal: [f32; 3], corner: [f32; 3]) -> [u8; 4] {
    let texture = imported.texture.as_ref().expect("atlas");
    for (p, uv, n) in triangles(imported) {
        if !near(n, normal) {
            continue;
        }
        if let Some(k) = (0..3).find(|k| near(p[*k], corner)) {
            let c = [(uv[0][0] + uv[1][0] + uv[2][0]) / 3.0, (uv[0][1] + uv[1][1] + uv[2][1]) / 3.0];
            let at = [uv[k][0] + (c[0] - uv[k][0]) * 0.02, uv[k][1] + (c[1] - uv[k][1]) * 0.02];
            return texture.sample(at);
        }
    }
    panic!("no face with normal {normal:?} has a corner at {corner:?}");
}

/// Цвет в центре треугольника стороны с нормалью `normal`, у которой есть угол `corner`.
fn center_color(imported: &Imported, normal: [f32; 3], corner: [f32; 3]) -> [u8; 4] {
    let texture = imported.texture.as_ref().expect("atlas");
    for (p, uv, n) in triangles(imported) {
        if near(n, normal) && p.iter().any(|q| near(*q, corner)) {
            return texture.sample([(uv[0][0] + uv[1][0] + uv[2][0]) / 3.0, (uv[0][1] + uv[1][1] + uv[2][1]) / 3.0]);
        }
    }
    panic!("no face with normal {normal:?} has a corner at {corner:?}");
}

/// Нормали совпадают с обходом (против часовой стрелки снаружи) у каждого треугольника.
fn assert_winding(imported: &Imported) {
    for (p, _, n) in triangles(imported) {
        let e1 = [p[1][0] - p[0][0], p[1][1] - p[0][1], p[1][2] - p[0][2]];
        let e2 = [p[2][0] - p[0][0], p[2][1] - p[0][1], p[2][2] - p[0][2]];
        let c = [e1[1] * e2[2] - e1[2] * e2[1], e1[2] * e2[0] - e1[0] * e2[2], e1[0] * e2[1] - e1[1] * e2[0]];
        let len = (c[0] * c[0] + c[1] * c[1] + c[2] * c[2]).sqrt();
        assert!(len > 0.0);
        let dot = (c[0] * n[0] + c[1] * n[1] + c[2] * n[2]) / len;
        assert!(dot > 0.999, "triangle {p:?} winds against its normal {n:?}");
    }
}

fn s(v: f32) -> f32 {
    v / 16.0
}

fn bounds(imported: &Imported) -> ([f32; 3], [f32; 3]) {
    imported.mesh.bounds()
}

fn assert_bounds(imported: &Imported, min: [f32; 3], max: [f32; 3]) {
    let (lo, hi) = bounds(imported);
    assert!(near(lo, min.map(s)) && near(hi, max.map(s)), "bounds {lo:?}..{hi:?}, expected {:?}..{:?}", min.map(s), max.map(s));
}

#[test]
fn bbmodel_faces_are_upright_as_seen_from_outside() {
    let body = mesh_of(&only(&asym(), &["body"]), &Options::default());
    assert_winding(&body);
    assert_bounds(&body, [-8.0, 0.0, -4.0], [8.0, 4.0, 4.0]);
    // (нормаль, левый верхний угол, правый верхний угол, цвет стороны) — в метрах.
    let (x, y, z) = (0.5, 0.25, 0.25);
    let cases = [
        ("north", [0.0, 0.0, -1.0], [x, y, -z], [-x, y, -z], RED),
        ("south", [0.0, 0.0, 1.0], [-x, y, z], [x, y, z], BLUE),
        ("east", [1.0, 0.0, 0.0], [x, y, z], [x, y, -z], GREEN),
        ("west", [-1.0, 0.0, 0.0], [-x, y, -z], [-x, y, z], YELLOW),
        // Верх: «север» у верхнего края картинки; низ: «юг» у верхнего края.
        ("up", [0.0, 1.0, 0.0], [-x, y, -z], [x, y, -z], CYAN),
        ("down", [0.0, -1.0, 0.0], [-x, 0.0, z], [x, 0.0, z], MAGENTA),
    ];
    for (name, normal, top_left, top_right, color) in cases {
        assert_eq!(color_at(&body, normal, top_left), WHITE, "{name}: top-left corner");
        assert_eq!(color_at(&body, normal, top_right), BLACK, "{name}: top-right corner");
        assert_eq!(center_color(&body, normal, top_left), color, "{name}: face colour");
    }
}

#[test]
fn face_rotation_flips_texture_choice_and_missing_faces() {
    let model = asym();
    let turned = mesh_of(&only(&model, &["turned"]), &Options::default());
    assert_winding(&turned);
    let (a, b) = ([-0.125, 0.5, -0.125], [0.125, 0.75, 0.125]);
    // North, поворот 90° по часовой: левый верхний тексель картинки — в правом верхнем
    // углу стороны, правый верхний — в правом нижнем.
    let north = [0.0, 0.0, -1.0];
    assert_eq!(color_at(&turned, north, [a[0], b[1], a[2]]), WHITE);
    assert_eq!(color_at(&turned, north, [a[0], a[1], a[2]]), BLACK);
    assert_eq!(color_at(&turned, north, [b[0], b[1], a[2]]), RED);
    // Up с u2 < u1 (отражение): белый — справа сверху, чёрный — слева сверху.
    let up = [0.0, 1.0, 0.0];
    assert_eq!(color_at(&turned, up, [b[0], b[1], a[2]]), WHITE);
    assert_eq!(color_at(&turned, up, [a[0], b[1], a[2]]), BLACK);
    // South — вторая текстура по UUID с собственным пространством UV 8×8.
    let south = [0.0, 0.0, 1.0];
    assert_eq!(color_at(&turned, south, [a[0], b[1], b[2]]), WHITE);
    assert_eq!(center_color(&turned, south, [a[0], b[1], b[2]]), [0, 128, 128, 255]);
    // West — анимированная текстура: кадр 0 (оранжевый), не кадр 1 (синий).
    assert_eq!(center_color(&turned, [-1.0, 0.0, 0.0], [a[0], b[1], a[2]]), [255, 128, 0, 255]);
    // East — `texture: null` (стороны нет), Down — в файле нет.
    let normals: Vec<[f32; 3]> = triangles(&turned).iter().map(|t| t.2).collect();
    assert!(!normals.iter().any(|n| near(*n, [1.0, 0.0, 0.0])), "east face must be absent");
    assert!(!normals.iter().any(|n| near(*n, [0.0, -1.0, 0.0])), "down face must be absent");
    assert_eq!(turned.mesh.triangle_count(), 8);
    assert!(model.report.warnings.iter().any(|w| w.contains("animated (2 frames)")), "{:?}", model.report);
}

#[test]
fn element_rotation_is_x_then_y_then_z_around_the_origin() {
    // Брусок 4×1×1 вдоль +X, поворот [90, 0, 90]: сначала X, потом Z — он смотрит в +Y.
    // В обратном порядке (Z, потом X) он смотрел бы в +Z: границы x[−1,0], y[−1,0], z[0,4].
    let arm = mesh_of(&only(&asym(), &["arm"]), &Options::default());
    assert_winding(&arm);
    assert_bounds(&arm, [0.0, 0.0, 0.0], [1.0, 4.0, 1.0]);
}

#[test]
fn nested_group_rotations_apply_inner_first() {
    // inner: 90° по Z вокруг (4, 0, 0); outer: 90° по Y вокруг нуля. Угол (8, 0, 0) → (0, 4, −4).
    let tip = mesh_of(&only(&asym(), &["outer"]), &Options::default());
    assert_winding(&tip);
    assert_bounds(&tip, [0.0, 4.0, -4.0], [1.0, 5.0, -3.0]);
    // Без поворотов групп («bake groups» выключено) брусок остаётся на месте.
    let flat = mesh_of(&only(&asym(), &["outer"]), &Options { bake_groups: false, ..Options::default() });
    assert_bounds(&flat, [8.0, 0.0, 0.0], [9.0, 1.0, 1.0]);
}

#[test]
fn inflate_grows_the_box_but_not_the_uv() {
    let puffy = mesh_of(&only(&asym(), &["puffy"]), &Options::default());
    assert_bounds(&puffy, [19.0, -1.0, -1.0], [23.0, 3.0, 3.0]);
    assert_eq!(color_at(&puffy, [0.0, 0.0, -1.0], [s(23.0), s(3.0), s(-1.0)]), WHITE);
}

#[test]
fn box_uv_follows_the_blockbench_template() {
    let model = asym();
    let Some(Node::Cube(boxy)) = model.find("boxy") else { panic!("boxy") };
    assert_eq!(boxy.box_uv.map(|b| b.offset), Some([0.0, 8.0]));
    assert_eq!(boxy.faces[Side::Up.index()].unwrap().uv, [8.0, 12.0, 4.0, 8.0]);
    let mesh = mesh_of(&only(&model, &["boxy"]), &Options::default());
    let (a, b) = ([s(30.0), 0.0, 0.0], [s(34.0), s(4.0), s(4.0)]);
    let cases = [
        ([0.0, 1.0, 0.0], [a[0], b[1], a[2]], [250, 140, 20, 255]),
        ([0.0, -1.0, 0.0], [a[0], a[1], b[2]], [120, 40, 160, 255]),
        ([1.0, 0.0, 0.0], [b[0], b[1], b[2]], [120, 80, 40, 255]),
        ([0.0, 0.0, -1.0], [b[0], b[1], a[2]], [250, 150, 180, 255]),
        ([-1.0, 0.0, 0.0], [a[0], b[1], a[2]], [128, 128, 128, 255]),
        ([0.0, 0.0, 1.0], [a[0], b[1], b[2]], [20, 100, 40, 255]),
    ];
    for (normal, corner, color) in cases {
        assert_eq!(center_color(&mesh, normal, corner), color, "{normal:?}");
    }
    // Верх развёртки повёрнут на 180°: северо-западный угол верха берёт правый нижний
    // тексель своего прямоугольника (белая метка в (7, 11)).
    assert_eq!(color_at(&mesh, [0.0, 1.0, 0.0], [a[0], b[1], a[2]]), WHITE);
}

#[test]
fn mesh_elements_keep_their_facing_and_uv() {
    let sheet = mesh_of(&only(&asym(), &["sheet"]), &Options::default());
    assert_winding(&sheet);
    assert_eq!(sheet.mesh.triangle_count(), 2, "the quad listed out of order is still one quad");
    assert!(sheet.mesh.normals.iter().all(|n| near(*n, [0.0, 1.0, 0.0])));
    assert_bounds(&sheet, [0.0, 20.0, 0.0], [4.0, 20.0, 4.0]);
    assert_eq!(color_at(&sheet, [0.0, 1.0, 0.0], [0.0, s(20.0), 0.0]), WHITE);
}

#[test]
fn whole_bbmodel_report_parts_and_settings() {
    let imported = load("asym.bbmodel", &Options::default());
    assert_winding(&imported);
    let report = &imported.report;
    for expected in ["1 animation(s)", "display transforms (`display`)", "`locator` elements", "elements with export disabled"] {
        assert!(report.ignored.iter().any(|i| i == expected), "{expected}: {report:?}");
    }
    assert!(imported.mesh.parts.is_empty(), "merged by default");
    let parts = load("asym.bbmodel", &Options { merge_by_texture: false, ..Options::default() });
    let names: Vec<&str> = parts.mesh.parts.iter().map(|p| p.name.as_str()).collect();
    assert_eq!(names, ["faces.png", "second.png", "anim.png"]);
    assert_eq!(parts.mesh.parts.last().unwrap().indices.end as usize, parts.mesh.indices.len());
    let double = load("asym.bbmodel", &Options { double_sided: true, ..Options::default() });
    assert_eq!(double.mesh.triangle_count(), 2 * imported.mesh.triangle_count());
    // Отражение по X: белая метка севера переезжает на −X, обход остаётся правильным.
    let flipped = mesh_of(&only(&asym(), &["body"]), &Options { flip_x: true, ..Options::default() });
    assert_winding(&flipped);
    assert_eq!(color_at(&flipped, [0.0, 0.0, -1.0], [-0.5, 0.25, -0.25]), WHITE);
    // Масштаб и ось Z вверх.
    let z_up = mesh_of(&only(&asym(), &["body"]), &Options { up: Some(UpAxis::Z), scale: 2.0, ..Options::default() });
    assert_winding(&z_up);
    let (lo, hi) = bounds(&z_up);
    assert!(near(lo, [-1.0, -0.5, -0.5]) && near(hi, [1.0, 0.5, 0.0]), "{lo:?} {hi:?}");
    // Детерминизм: те же байты — тот же результат.
    assert_eq!(load("asym.bbmodel", &Options::default()), imported);
}

#[test]
fn legacy_3x_files_use_the_global_box_uv_and_mirror() {
    let model = read_model("legacy_v3.bbmodel", &bytes("legacy_v3.bbmodel"), &resolve).unwrap();
    let Some(Node::Group(bone)) = model.find("bone") else { panic!("bone group") };
    assert_eq!(bone.children.len(), 2);
    let Some(Node::Cube(ear)) = model.find("ear") else { panic!("ear") };
    assert_eq!(ear.rotation, [0.0, 45.0, 0.0]);
    assert_eq!(ear.faces[Side::East.index()].unwrap().uv, [6.0, 10.0, 4.0, 12.0], "mirror_uv swaps east and west");
    let head = mesh_of(&only(&model, &["bone"]), &Options::default());
    assert_winding(&head);
    assert_eq!(center_color(&head, [0.0, 0.0, -1.0], [s(34.0), s(4.0), 0.0]), [250, 150, 180, 255]);
}

#[test]
fn java_block_projects_are_centred_and_read_external_textures() {
    let imported = load("java_block.bbmodel", &Options::default());
    assert_bounds(&imported, [-8.0, 0.0, -8.0], [8.0, 16.0, 8.0]);
    assert_eq!(center_color(&imported, [0.0, 0.0, -1.0], [0.5, 1.0, -0.5]), RED);
    assert!(imported.report.warnings.is_empty(), "{:?}", imported.report);
    // Без файла текстуры — шахматка и запись в отчёте, но модель грузится.
    let missing = import("java_block.bbmodel", &bytes("java_block.bbmodel"), &|_| None, &Options::default()).unwrap();
    assert!(missing.report.warnings.iter().any(|w| w.contains("block/faces.png") && w.contains("checkerboard")), "{:?}", missing.report);
}

#[test]
fn minecraft_json_model_with_rescaled_cross() {
    let imported = load("slab.model.json", &Options::default());
    assert_winding(&imported);
    let (lo, hi) = bounds(&imported);
    assert!(near(lo, [-0.5, 0.0, -0.5]) && near(hi, [0.5, 1.0, 0.5]), "{lo:?} {hi:?}");
    // Верх без `uv` — UV по положению: вся текстура, белый тексель (0, 0) в северо-западном углу.
    assert_eq!(color_at(&imported, [0.0, 1.0, 0.0], [-0.5, 0.5, -0.5]), WHITE);
    // Низ без `uv`: белый тексель у юго-западного угла.
    assert_eq!(color_at(&imported, [0.0, -1.0, 0.0], [-0.5, 0.0, 0.5]), WHITE);
    // North c поворотом 180°: белая метка — в правом нижнем углу стороны, чёрная — в левом нижнем.
    assert_eq!(color_at(&imported, [0.0, 0.0, -1.0], [-0.5, 0.0, -0.5]), WHITE);
    assert_eq!(color_at(&imported, [0.0, 0.0, -1.0], [0.5, 0.0, -0.5]), BLACK);
    // Крест: 45° вокруг Y с rescale — концы на ±7.2/16 по X и Z; +X уходит в −Z (правая тройка).
    let cross: Vec<[f32; 3]> = imported.mesh.positions.iter().copied().filter(|p| p[1] > 0.5 + 1e-4).collect();
    let max_x = cross.iter().fold(f32::MIN, |m, p| m.max(p[0]));
    assert!((max_x - 7.2 / 16.0).abs() < 1e-3, "rescale stretches the cross: {max_x}");
    let end = cross.iter().find(|p| (p[0] - max_x).abs() < 1e-4).unwrap();
    assert!((end[2] + 7.2 / 16.0).abs() < 1e-3, "{end:?}");
    let report = &imported.report;
    assert!(report.warnings.iter().any(|w| w.contains("`#cross` is not defined")), "{report:?}");
    assert!(report.ignored.iter().any(|w| w.contains("display")), "{report:?}");
    // Модель без своих элементов и без найденного родителя — понятная ошибка.
    let err = import("x.model.json", br#"{"parent":"block/cube_all","textures":{"all":"block/x"}}"#, &|_| None, &Options::default()).unwrap_err();
    assert!(err.0.contains("parent `block/cube_all`"), "{err}");
    // Родитель рядом: элементы берутся из него, текстуры наследуются.
    let parent = br##"{"elements":[{"from":[0,0,0],"to":[16,16,16],"faces":{"up":{"texture":"#all"}}}]}"##;
    let child = br#"{"parent":"minecraft:block/cube_all","textures":{"all":"block/faces"}}"#;
    let resolve_parent = |p: &str| if p == "block/cube_all.json" { Some(parent.to_vec()) } else { resolve(p) };
    let cube = import("child.model.json", child, &resolve_parent, &Options::default()).unwrap();
    assert_eq!(cube.mesh.triangle_count(), 2);
    assert_eq!(color_at(&cube, [0.0, 1.0, 0.0], [-0.5, 1.0, -0.5]), WHITE);
}

// ----------------------------------------------------------------------
// .vox по байтам
// ----------------------------------------------------------------------

fn chunk(id: &[u8; 4], content: &[u8], children: &[u8]) -> Vec<u8> {
    let mut out = id.to_vec();
    out.extend((content.len() as i32).to_le_bytes());
    out.extend((children.len() as i32).to_le_bytes());
    out.extend(content);
    out.extend(children);
    out
}

fn vox_file(chunks: &[Vec<u8>]) -> Vec<u8> {
    let mut out = b"VOX ".to_vec();
    out.extend(150i32.to_le_bytes());
    out.extend(chunk(b"MAIN", &[], &chunks.concat()));
    out
}

fn ints(values: &[i32]) -> Vec<u8> {
    values.iter().flat_map(|v| v.to_le_bytes()).collect()
}

fn dict(pairs: &[(&str, &str)]) -> Vec<u8> {
    let mut out = ints(&[pairs.len() as i32]);
    for (k, v) in pairs {
        out.extend(ints(&[k.len() as i32]));
        out.extend(k.as_bytes());
        out.extend(ints(&[v.len() as i32]));
        out.extend(v.as_bytes());
    }
    out
}

fn model_chunks(size: [i32; 3], voxels: &[[u8; 4]]) -> Vec<Vec<u8>> {
    let mut xyzi = ints(&[voxels.len() as i32]);
    for v in voxels {
        xyzi.extend(v);
    }
    vec![chunk(b"SIZE", &ints(&size), &[]), chunk(b"XYZI", &xyzi, &[])]
}

fn transform(id: i32, child: i32, frame: &[(&str, &str)]) -> Vec<u8> {
    let mut c = ints(&[id]);
    c.extend(dict(&[]));
    c.extend(ints(&[child, -1, 0, 1]));
    c.extend(dict(frame));
    chunk(b"nTRN", &c, &[])
}

fn group(id: i32, children: &[i32]) -> Vec<u8> {
    let mut c = ints(&[id]);
    c.extend(dict(&[]));
    c.extend(ints(&[children.len() as i32]));
    c.extend(ints(children));
    chunk(b"nGRP", &c, &[])
}

fn shape(id: i32, model: i32) -> Vec<u8> {
    let mut c = ints(&[id]);
    c.extend(dict(&[]));
    c.extend(ints(&[1, model]));
    c.extend(dict(&[]));
    chunk(b"nSHP", &c, &[])
}

/// Верхняя сторона с нормалью `normal`, наиболее выдвинутая вдоль неё: цвет её центра.
fn extreme_face_color(imported: &Imported, normal: [f32; 3]) -> [u8; 4] {
    let texture = imported.texture.as_ref().unwrap();
    let along = |p: [f32; 3]| p[0] * normal[0] + p[1] * normal[1] + p[2] * normal[2];
    let best = triangles(imported)
        .into_iter()
        .filter(|t| near(t.2, normal))
        .max_by(|a, b| along(a.0[0]).partial_cmp(&along(b.0[0])).unwrap())
        .expect("a face with this normal");
    let uv = best.1;
    texture.sample([(uv[0][0] + uv[1][0] + uv[2][0]) / 3.0, (uv[0][1] + uv[1][1] + uv[2][1]) / 3.0])
}

#[test]
fn vox_single_model_turns_z_up_into_y_up_with_the_default_palette() {
    // Ячейки MagicaVoxel (Z вверх): A (0,0,0) белая, B (2,0,0) синяя справа, C (0,1,0)
    // зелёная «вглубь» (+Y MagicaVoxel), D (0,0,1) красная сверху.
    let file = vox_file(&model_chunks([3, 2, 2], &[[0, 0, 0, 1], [2, 0, 0, 216], [0, 1, 0, 226], [0, 0, 1, 236]]));
    let scene = vox::read(&file).unwrap();
    assert!(scene.report.warnings.iter().any(|w| w.contains("default palette")));
    let imported = vox::to_mesh(&scene, &Options::default()).unwrap();
    assert_winding(&imported);
    assert_eq!(imported.mesh.triangle_count(), 40, "20 exposed unit faces of 4 colours");
    let (lo, hi) = bounds(&imported);
    assert!(near(lo, [-1.0, 0.0, -1.0].map(s)) && near(hi, [2.0, 2.0, 1.0].map(s)), "on the ground, centred: {lo:?} {hi:?}");
    assert_eq!(extreme_face_color(&imported, [0.0, 1.0, 0.0]), [238, 0, 0, 255], "MagicaVoxel +Z is up");
    assert_eq!(extreme_face_color(&imported, [1.0, 0.0, 0.0]), [0, 0, 238, 255], "+X stays +X");
    assert_eq!(extreme_face_color(&imported, [0.0, 0.0, -1.0]), [0, 238, 0, 255], "MagicaVoxel +Y goes to −Z");
    // Для редактора — кубоиды; стороны, закрытые соседом, выключены.
    let model = vox::to_model(&scene, "l").unwrap();
    assert_eq!(model.roots.len(), 4);
    let enabled: usize = model.cubes().iter().map(|c| c.faces.iter().flatten().count()).sum();
    assert_eq!(enabled, 20);
}

#[test]
fn vox_scene_graph_places_and_rotates_models() {
    let mut palette = Vec::new();
    for i in 1..=256u32 {
        palette.extend([(i % 256) as u8, (2 * i % 256) as u8, (3 * i % 256) as u8, 255]);
    }
    let mut chunks = model_chunks([1, 1, 1], &[[0, 0, 0, 10]]);
    chunks.extend(model_chunks([2, 1, 1], &[[0, 0, 0, 20], [1, 0, 0, 30]]));
    chunks.push(transform(0, 1, &[]));
    chunks.push(group(1, &[2, 4]));
    chunks.push(transform(2, 3, &[("_t", "0 0 0")]));
    chunks.push(shape(3, 0));
    // 90° вокруг Z MagicaVoxel: брусок вдоль X ложится вдоль Y (у движка — вдоль Z).
    chunks.push(transform(4, 5, &[("_t", "5 0 3"), ("_r", "17")]));
    chunks.push(shape(5, 1));
    chunks.push(chunk(b"RGBA", &palette, &[]));
    let scene = vox::read(&vox_file(&chunks)).unwrap();
    let mut cells = scene.voxels.clone();
    cells.sort();
    assert_eq!(cells, vec![([-2, 0, -1], 10), ([2, 3, -1], 30), ([2, 3, 0], 20)]);
    let imported = vox::to_mesh(&scene, &Options::default()).unwrap();
    assert_eq!(extreme_face_color(&imported, [0.0, 0.0, 1.0]), [20, 40, 60, 255], "RGBA entry i−1 is colour i");
    assert_eq!(extreme_face_color(&imported, [-1.0, 0.0, 0.0]), [10, 20, 30, 255]);
}

#[test]
fn vox_errors_are_precise() {
    assert!(vox::read(b"RIFF....").unwrap_err().0.contains("VOX"));
    let good = vox_file(&model_chunks([2, 2, 2], &[[0, 0, 0, 1]]));
    for cut in [8, 12, 20, good.len() - 1] {
        assert!(vox::read(&good[..cut]).is_err(), "cut at {cut}");
    }
    // Счётчик вокселей больше блока.
    let mut lie = good.clone();
    let at = lie.len() - 8;
    lie[at..at + 4].copy_from_slice(&1_000_000i32.to_le_bytes());
    assert!(vox::read(&lie).unwrap_err().0.contains("does not fit"));
    // Петля в графе сцены.
    let mut chunks = model_chunks([1, 1, 1], &[[0, 0, 0, 1]]);
    chunks.push(transform(0, 1, &[]));
    chunks.push(group(1, &[0]));
    assert!(vox::read(&vox_file(&chunks)).unwrap_err().0.contains("loops"));
    // Огромный размер модели.
    assert!(vox::read(&vox_file(&model_chunks([100_000, 1, 1], &[]))).unwrap_err().0.contains("out of range"));
}

// ----------------------------------------------------------------------
// PLY и STL
// ----------------------------------------------------------------------

#[test]
fn ply_encodings_agree_and_z_up_becomes_y_up() {
    let ascii = load("quad_ascii.ply", &Options::default());
    assert_eq!(load("quad_le.ply", &Options::default()), ascii);
    assert_eq!(load("quad_be.ply", &Options::default()), ascii);
    assert_winding(&ascii);
    assert_eq!(ascii.mesh.triangle_count(), 3);
    // Красный квадрат смотрел в +Z на высоте 0.5 — теперь в +Y на той же высоте.
    assert_eq!(center_color(&ascii, [0.0, 1.0, 0.0], [0.0, 0.5, 0.0]), [255, 0, 0, 255]);
    // Зелёный треугольник смотрел в −Y — теперь в +Z.
    assert_eq!(center_color(&ascii, [0.0, 0.0, 1.0], [0.0, 0.0, 0.0]), [0, 255, 0, 255]);
    let y_up = load("quad_ascii.ply", &Options { up: Some(UpAxis::Y), ..Options::default() });
    assert_eq!(center_color(&y_up, [0.0, 0.0, 1.0], [0.0, 0.0, 0.5]), [255, 0, 0, 255]);
    assert!(engine_model_formats::has_texture("quad_ascii.ply", &bytes("quad_ascii.ply")));
}

#[test]
fn stl_encodings_agree() {
    let ascii = load("wedge_ascii.stl", &Options::default());
    let binary = load("wedge_binary.stl", &Options::default());
    assert_eq!(ascii.mesh, binary.mesh);
    assert!(ascii.texture.is_none());
    assert_winding(&ascii);
    assert!(near(ascii.mesh.normals[0], [0.0, 1.0, 0.0]));
    assert!(near(ascii.mesh.normals[3], [0.0, 0.0, -1.0]));
    assert!((bounds(&ascii).1[1] - 2.0).abs() < 1e-6, "Z-up file: height 2 along Y");
    assert!(!engine_model_formats::has_texture("wedge_ascii.stl", &bytes("wedge_ascii.stl")));
}

#[test]
fn ply_and_stl_errors_are_precise() {
    let none = |_: &str| None;
    let err = |text: &[u8]| poly::read_ply(text, &none).unwrap_err().0;
    assert!(err(b"plx").contains("not a PLY"));
    assert!(err(b"ply\nformat ascii 1.0\nelement vertex 3\nproperty float x\nend_header\n").contains("too short"));
    assert!(err(b"ply\nformat ascii 1.0\nelement vertex 1\nproperty float x\nproperty float y\nproperty float z\nelement face 1\nproperty list uchar int vertex_indices\nend_header\n0 0 0\n3 0 1 2\n").contains("vertex 1"));
    assert!(err(b"ply\nformat ascii 1.0\nelement vertex 1\nproperty float x\nproperty float y\nproperty float z\nend_header\nnan 0 0\n").contains("out of range"));
    let mut stl = bytes("wedge_binary.stl");
    stl[80..84].copy_from_slice(&1_000_000u32.to_le_bytes());
    assert!(poly::read_stl(&stl).unwrap_err().0.contains("declares 1000000 triangles"));
    let mut nan = bytes("wedge_binary.stl");
    nan[96..100].copy_from_slice(&f32::NAN.to_le_bytes());
    assert!(poly::read_stl(&nan).unwrap_err().0.contains("out of range"));
    for unsupported in ["robot.fbx", "scene.blend", "x.dae", "old.3ds"] {
        let e = import(unsupported, b"anything", &none, &Options::default()).unwrap_err();
        assert!(e.0.contains("is not supported") && e.0.contains("glTF"), "{e}");
    }
    assert!(import("x.json", br#"{"hp": 1}"#, &none, &Options::default()).is_err());
    assert!(import("x.bbmodel", br#"{"meta":{"format_version":"4.5"}}"#, &none, &Options::default()).unwrap_err().0.contains("no visible faces"));
    assert!(import("a.bbmodel", &bytes("asym.bbmodel"), &none, &Options { scale: -1.0, ..Options::default() }).unwrap_err().0.contains("scale"));
}

/// «Почти фаззинг»: обрезки и испорченные байты всех образцов — ошибка или модель, но
/// не паника и не зависание.
#[test]
fn mutated_fixtures_never_panic() {
    let mut files: Vec<(String, Vec<u8>)> = ["asym.bbmodel", "legacy_v3.bbmodel", "slab.model.json", "quad_ascii.ply", "quad_le.ply", "quad_be.ply", "wedge_ascii.stl", "wedge_binary.stl"]
        .iter()
        .map(|n| (n.to_string(), bytes(n)))
        .collect();
    let mut chunks = model_chunks([2, 2, 2], &[[0, 0, 0, 1], [1, 1, 1, 2]]);
    chunks.push(transform(0, 1, &[("_t", "1 2 3"), ("_r", "4")]));
    chunks.push(shape(1, 0));
    files.push(("scene.vox".to_string(), vox_file(&chunks)));
    let mut seed = 0x2545_f491_4f6c_dd1du64;
    let mut next = || {
        seed ^= seed << 13;
        seed ^= seed >> 7;
        seed ^= seed << 17;
        seed
    };
    let none = |_: &str| None;
    for (name, original) in &files {
        for step in 1..48 {
            let cut = original.len() * step / 48;
            let _ = import(name, &original[..cut], &none, &Options::default());
        }
        for _ in 0..150 {
            let mut data = original.clone();
            for _ in 0..1 + next() % 4 {
                let at = (next() as usize) % data.len();
                data[at] = match next() % 4 {
                    0 => 0,
                    1 => 0xff,
                    2 => b'9',
                    _ => next() as u8,
                };
            }
            let _ = import(name, &data, &none, &Options::default());
            let _ = read_model(name, &data, &none);
        }
    }
}

#[test]
fn bbmodel_write_read_round_trip() {
    let mut original = asym();
    original.report = Default::default();
    let text = bbmodel::write(&original);
    assert_eq!(bbmodel::write(&original), text, "deterministic");
    let mut back = bbmodel::read("asym.bbmodel", text.as_bytes(), &|_| None).unwrap();
    assert!(back.report.is_empty(), "{:?}", back.report);
    back.report = Default::default();
    assert_eq!(back, original);
    assert_eq!(Format::detect("x.bbmodel", text.as_bytes()), Some(Format::Bbmodel));
}
