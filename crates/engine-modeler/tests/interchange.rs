//! Чужие форматы в Piska Modeler: `.bbmodel` открывается кубами и группами и пишется
//! обратно без потерь; запечённая модель совпадает с импортом движка (тот же крейт
//! форматов, другой путь сборки треугольников); меши (`.obj`, `.stl`, `.ply`) становятся
//! редактируемыми элементами-мешами, `.pmodel` с ними читается и пишется версией 2.

use std::path::{Path, PathBuf};

use engine_model_formats as formats;
use engine_modeler::interchange::{import_mesh_into, open_bytes, to_bbmodel};
use engine_modeler::{bake, format, mesh_edit, ops, Document, Element, FaceDir, NodeKind, Shape};
use glam::Vec3;

const FIXTURES: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/../../vendor/engine-model-formats/tests/fixtures");

fn fixture(name: &str) -> Vec<u8> {
    std::fs::read(Path::new(FIXTURES).join(name)).unwrap()
}

fn resolve(path: &str) -> Option<Vec<u8>> {
    if path.starts_with("..") {
        return None;
    }
    std::fs::read(Path::new(FIXTURES).join(path)).ok()
}

/// (центр треугольника ×100, нормаль ×100, цвет текстуры в центре) — отсортировано.
type Signature = Vec<([i32; 3], [i32; 3], [u8; 4])>;

fn signature(positions: &[[f32; 3]], normals: &[[f32; 3]], uvs: &[[f32; 2]], indices: &[u32], texture: &formats::RgbaImage) -> Signature {
    let mut out: Signature = indices
        .chunks_exact(3)
        .map(|t| {
            let i = [t[0] as usize, t[1] as usize, t[2] as usize];
            let c: [f32; 3] = std::array::from_fn(|k| (positions[i[0]][k] + positions[i[1]][k] + positions[i[2]][k]) / 3.0);
            let uv = [(uvs[i[0]][0] + uvs[i[1]][0] + uvs[i[2]][0]) / 3.0, (uvs[i[0]][1] + uvs[i[1]][1] + uvs[i[2]][1]) / 3.0];
            (c.map(|v| (v * 100.0).round() as i32), normals[i[0]].map(|v| (v * 100.0).round() as i32), texture.sample(uv))
        })
        .collect();
    out.sort();
    out
}

#[test]
fn opened_bbmodel_bakes_like_the_engine_imports_it() {
    for name in ["asym.bbmodel", "legacy_v3.bbmodel", "java_block.bbmodel", "slab.model.json"] {
        let bytes = fixture(name);
        let engine = formats::import(name, &bytes, &resolve, &formats::Options::default()).unwrap();
        let opened = open_bytes(name, &bytes, &resolve).unwrap();
        let baked = bake(&opened.doc);
        let atlas = baked.texture.as_ref().unwrap();
        let atlas = formats::RgbaImage { width: atlas.width(), height: atlas.height(), pixels: atlas.pixels().iter().flatten().copied().collect() };
        let a = signature(&engine.mesh.positions, &engine.mesh.normals, &engine.mesh.uvs, &engine.mesh.indices, engine.texture.as_ref().unwrap());
        let b = signature(&baked.positions, &baked.normals, &baked.uvs, &baked.indices, &atlas);
        assert_eq!(a.len(), b.len(), "{name}: triangle count");
        assert_eq!(a, b, "{name}: the Modeler's cubes match the engine import (positions, normals, texel colours)");
    }
}

#[test]
fn bbmodel_structure_becomes_cubes_groups_and_meshes() {
    let opened = open_bytes("asym.bbmodel", &fixture("asym.bbmodel"), &resolve).unwrap();
    let doc = &opened.doc;
    assert!(opened.summary.contains("editable"), "{}", opened.summary);
    let outer = doc.find_by_name("outer").unwrap();
    let inner = doc.find_by_name("inner").unwrap();
    assert!(doc.node(outer).unwrap().is_group() && doc.node(inner).unwrap().parent == Some(outer));
    let tip = doc.find_by_name("tip").unwrap();
    assert_eq!(doc.node(tip).unwrap().parent, Some(inner));
    assert_eq!(doc.element(tip).unwrap().shape, Shape::Cuboid);
    let sheet = doc.find_by_name("sheet").unwrap();
    assert_eq!(doc.element(sheet).unwrap().shape, Shape::Mesh, "Blockbench meshes are editable meshes");
    assert_eq!(doc.textures.len(), 3);
    assert!(doc.nodes.iter().all(|n| !n.locked), "nothing comes in locked");
    let boxy = doc.element(doc.find_by_name("boxy").unwrap()).unwrap();
    assert!(boxy.box_uv);
    assert_eq!(boxy.uv_origin, [0, 8]);
}

#[test]
fn bbmodel_export_round_trips_the_document() {
    let mut doc = Document::new("round");
    ops::resize_texture(&mut doc, 0, 64, 32);
    doc.textures[0].image.set(3, 4, [10, 20, 30, 255]);
    let t2 = ops::add_texture(&mut doc, "second", 16, 16);
    let g = ops::add_group(&mut doc, "arm", Vec3::new(4.0, 8.0, 0.0), None);
    doc.node_mut(g).unwrap().group_mut().unwrap().rotation = Vec3::new(0.0, 0.0, -30.0);
    let a = ops::add_element(&mut doc, "upper", Element::cuboid(Vec3::new(4.0, 2.0, -1.0), Vec3::new(2.0, 6.0, 2.0)), Some(g));
    let e = doc.element_mut(a).unwrap();
    e.rotation = Vec3::new(10.0, 20.0, 30.0);
    e.inflate = 0.5;
    let up = e.face_mut(FaceDir::Up);
    up.uv.rotation = 1;
    up.uv.flip_u = true;
    e.face_mut(FaceDir::South).texture = t2;
    let b = ops::add_element(&mut doc, "body", Element::cuboid(Vec3::splat(-4.0), Vec3::splat(8.0)), None);
    doc.element_mut(b).unwrap().box_uv = false;
    let (points, faces) = (vec![Vec3::ZERO, Vec3::X * 4.0, Vec3::new(4.0, 0.0, 4.0)], vec![engine_modeler::MeshFace { vertices: vec![0, 2, 1], uvs: vec![[0.0, 0.0], [4.0, 4.0], [4.0, 0.0]], texture: 0 }]);
    mesh_edit::add_mesh(&mut doc, "tri", &points, faces, None);

    let (text, notes) = to_bbmodel(&doc);
    assert!(notes.is_empty(), "{notes:?}");
    let back = open_bytes("round.bbmodel", text.as_bytes(), &|_| None).unwrap().doc;
    assert_eq!(back.nodes.len(), doc.nodes.len());
    for (x, y) in doc.nodes.iter().zip(&back.nodes) {
        assert_eq!((x.id, &x.name, x.parent, x.visible), (y.id, &y.name, y.parent, y.visible));
        match (&x.kind, &y.kind) {
            (NodeKind::Group(p), NodeKind::Group(q)) => assert_eq!(p, q),
            (NodeKind::Element(p), NodeKind::Element(q)) => {
                assert_eq!(p.shape, q.shape, "{}", x.name);
                assert!((p.from - q.from).length() < 1e-4 && (p.size - q.size).length() < 1e-4, "{}", x.name);
                assert!((p.pivot - q.pivot).length() < 1e-4 && (p.rotation - q.rotation).length() < 1e-4, "{}", x.name);
                assert_eq!((p.inflate, p.box_uv), (q.inflate, q.box_uv), "{}", x.name);
                if p.box_uv {
                    assert_eq!(p.uv_origin, q.uv_origin, "{}", x.name);
                }
                for dir in FaceDir::ALL {
                    let (f, g) = (p.face(dir), q.face(dir));
                    assert_eq!(f.enabled, g.enabled, "{} {dir:?}", x.name);
                    if f.enabled && p.shape == Shape::Cuboid {
                        assert_eq!((f.texture, f.uv), (g.texture, g.uv), "{} {dir:?}", x.name);
                    }
                }
                assert_eq!(p.mesh, q.mesh, "{}", x.name);
            }
            _ => panic!("node kinds differ"),
        }
    }
    assert_eq!(back.textures.iter().map(|t| &t.image).collect::<Vec<_>>(), doc.textures.iter().map(|t| &t.image).collect::<Vec<_>>());
    assert_eq!(bake(&back).positions, bake(&doc).positions, "same geometry");
}

fn temp(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("modeler-interchange-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

const CUBE_OBJ: &str = "mtllib cube.mtl\no crate\n\
v 0 0 0\nv 1 0 0\nv 1 1 0\nv 0 1 0\nv 0 0 1\nv 1 0 1\nv 1 1 1\nv 0 1 1\n\
vt 0 0\nvt 1 0\nvt 1 1\nvt 0 1\nusemtl wood\n\
f 1/1 4/4 3/3 2/2\nf 5/1 6/2 7/3 8/4\nf 1/1 2/2 6/3 5/4\nf 4/1 8/4 7/3 3/2\nf 1/1 5/2 8/3 4/4\nf 2/1 3/4 7/3 6/2\n";

#[test]
fn obj_with_texture_becomes_a_paintable_mesh_and_converts_to_cubes() {
    let dir = temp("obj");
    let png = engine_modeler::Image::new(8, 8, [200, 30, 30, 255]).to_png();
    std::fs::write(dir.join("wood.png"), &png).unwrap();
    std::fs::write(dir.join("cube.mtl"), "newmtl wood\nKd 1 1 1\nmap_Kd wood.png\n").unwrap();
    let resolve = |p: &str| std::fs::read(dir.join(p)).ok();
    let opened = open_bytes("crate.obj", CUBE_OBJ.as_bytes(), &resolve).unwrap();
    assert!(opened.summary.contains("crate.obj as an editable mesh"), "{}", opened.summary);
    let doc = opened.doc;
    let (id, element) = doc.elements().next().unwrap();
    assert_eq!(element.shape, Shape::Mesh);
    assert_eq!(doc.node(id).unwrap().name, "crate");
    assert_eq!(element.size, Vec3::splat(16.0), "1 m = 16 model units");
    assert_eq!(doc.textures[0].image.get(0, 0), Some([200, 30, 30, 255]), "the texture replaces the blank atlas");
    let mesh = element.mesh.as_ref().unwrap();
    assert_eq!(mesh.vertices.len(), 8);
    assert!(mesh.has_uvs());
    // Нормали наружу.
    let built = engine_modeler::build_mesh(&doc, Default::default());
    for t in &built.triangles {
        let [a, b, c] = built.triangle_positions(t);
        assert!((b - a).cross(c - a).dot((a + b + c) / 3.0 - Vec3::splat(8.0)) > 0.0);
    }
    // Кубы.
    let mut doc = doc;
    let cubes = mesh_edit::convert_to_cubes(&mut doc, id).unwrap();
    assert_eq!(cubes.len(), 1);
    assert_eq!(doc.element(cubes[0]).unwrap().size, Vec3::splat(16.0));
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn colour_meshes_get_generated_uvs_and_pmodel_v2_round_trips() {
    let dir = temp("ply");
    let mut doc = Document::new("colours");
    let (ids, _) = import_mesh_into(&mut doc, "quad.ply", &fixture("quad_ascii.ply"), &resolve, true).unwrap();
    let element = doc.element(ids[0]).unwrap();
    assert!(element.mesh.as_ref().unwrap().has_uvs(), "colours become a generated texture");
    // Красный квадрат (смотрел в +Z у .ply) — сверху: тексель под верхней гранью красный.
    let built = engine_modeler::build_mesh(&doc, Default::default());
    let top = built.triangles.iter().find(|t| built.vertices[t.indices[0] as usize].normal.y > 0.99).unwrap();
    let uv = top.indices.iter().fold(glam::Vec2::ZERO, |s, i| s + built.vertices[*i as usize].uv) / 3.0;
    let size = doc.textures[0].image.size();
    let texel = doc.textures[0].image.get((uv.x * size[0] as f32) as i32, (uv.y * size[1] as f32) as i32);
    assert_eq!(texel, Some([255, 0, 0, 255]));
    let (stl, _) = import_mesh_into(&mut doc, "wedge.stl", &fixture("wedge_binary.stl"), &|_| None, false).unwrap();
    assert_eq!(doc.element(stl[0]).unwrap().mesh.as_ref().unwrap().vertices.len(), 4, "shared corners are merged");

    let path = dir.join("colours.pmodel");
    format::save(&doc, &path).unwrap();
    let text = std::fs::read_to_string(&path).unwrap();
    assert!(text.contains("version: 2"));
    assert_eq!(format::load(&path).unwrap(), doc);
    // Без мешей — по-прежнему версия 1.
    assert!(format::to_string(&Document::new("plain")).contains("version: 1"));
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn unsupported_and_broken_files_explain_themselves() {
    let err = open_bytes("robot.fbx", b"Kaydara", &|_| None).unwrap_err().to_string();
    assert!(err.contains("FBX is not supported"), "{err}");
    let err = open_bytes("x.obj", b"v 0 0 0\nf 1 2 3\n", &|_| None).unwrap_err().to_string();
    assert!(err.contains("bad vertex"), "{err}");
    let err = open_bytes("x.bbmodel", b"{}", &|_| None).unwrap_err().to_string();
    assert!(err.contains("meta"), "{err}");
}
