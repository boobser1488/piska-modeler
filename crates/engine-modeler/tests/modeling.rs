//! Моделирование мешей: каждая операция на маленьких мешах — точные числа вершин и
//! граней, положения, инварианты (нет вырожденных и повторных граней, замкнутый меш
//! остаётся замкнутым с обходом наружу — объём положителен, характеристика Эйлера),
//! UV новых граней внутри атласа и без наложений, отмена восстанавливает документ
//! побайтно, близнецы остаются отражёнными; случайные цепочки операций не падают.

use std::collections::BTreeSet;

use engine_modeler::mesh_ops::{self as mo, edge, Delete, EditMesh, Primitive};
use engine_modeler::{mesh_edit, ops, Document, Element, Session};
use glam::{Vec2, Vec3};

fn cube() -> EditMesh {
    mo::box_mesh(Vec3::new(-4.0, 0.0, -4.0), Vec3::new(4.0, 8.0, 4.0))
}

/// Грань с нормалью, ближайшей к `n`.
fn face_toward(m: &EditMesh, n: Vec3) -> usize {
    (0..m.faces.len()).max_by(|a, b| m.normal(*a).dot(n).total_cmp(&m.normal(*b).dot(n))).unwrap()
}

fn assert_closed(m: &EditMesh, what: &str) {
    m.check().unwrap_or_else(|e| panic!("{what}: {e}"));
    assert!(m.is_closed_manifold(), "{what}: closed and consistently wound");
    assert!(m.signed_volume() > 0.0, "{what}: outward winding (volume {})", m.signed_volume());
    assert_eq!(m.euler(), 2, "{what}: a closed sphere-like surface");
}

fn set<T: Ord + Copy>(items: &[T]) -> BTreeSet<T> {
    items.iter().copied().collect()
}

#[test]
fn extrude_top_face_of_a_cube() {
    let mut m = cube();
    let top = face_toward(&m, Vec3::Y);
    let (caps, sides) = mo::extrude_faces(&mut m, &set(&[top]), 2.0);
    assert_eq!((m.points.len(), m.faces.len(), sides.len()), (12, 10, 4));
    assert_closed(&m, "extrude");
    assert!((m.signed_volume() - (512.0 + 128.0)).abs() < 1e-3);
    let cap = *caps.iter().next().unwrap();
    assert!(m.face_points(cap).iter().all(|p| (p.y - 10.0).abs() < 1e-5), "the cap moved up by 2");
    // Выдавить внутрь — тоже без нарушений.
    let mut m = cube();
    mo::extrude_faces(&mut m, &set(&[top]), -3.0);
    assert_closed(&m, "extrude inward");
}

#[test]
fn extrude_edges_of_a_plane_makes_a_wall() {
    let mut m = Primitive::Plane { size: 8.0 }.build();
    let edges: BTreeSet<_> = m.face_edges(0).iter().take(1).map(|(a, b)| edge(*a, *b)).collect();
    let (new, rim) = mo::extrude_edges(&mut m, &edges, 2.0);
    assert_eq!((new.len(), rim.len(), m.faces.len(), m.points.len()), (1, 1, 2, 6));
    m.check().unwrap();
    // Новая грань в той же плоскости и с той же стороной.
    assert!(m.normal(1).dot(m.normal(0)) > 0.99);
}

#[test]
fn inset_top_face() {
    let mut m = cube();
    let top = face_toward(&m, Vec3::Y);
    let (inner, ring) = mo::inset(&mut m, &set(&[top]), 1.0);
    assert_eq!((m.points.len(), m.faces.len(), ring.len()), (12, 10, 4));
    assert_closed(&m, "inset");
    assert!((m.signed_volume() - 512.0).abs() < 1e-3, "inset does not change the volume");
    let face = *inner.iter().next().unwrap();
    for p in m.face_points(face) {
        assert!((p.x.abs() - 3.0).abs() < 1e-4 && (p.z.abs() - 3.0).abs() < 1e-4 && p.y == 8.0, "{p}");
    }
}

#[test]
fn loop_cut_around_the_sides() {
    let vertical = |m: &EditMesh| {
        m.all_edges().into_iter().find(|(a, b)| {
            let (p, q) = (m.points[*a as usize], m.points[*b as usize]);
            p.x == q.x && p.z == q.z
        }).unwrap()
    };
    // Вертикальное ребро: кольцо — четыре боковые грани, разрез горизонтальный посередине.
    let mut m = cube();
    let e = vertical(&m);
    let cut = mo::loop_cut(&mut m, e, 1, 0.5);
    assert_eq!((m.points.len(), m.faces.len(), cut.len()), (12, 10, 4));
    assert_closed(&m, "one cut");
    assert!(m.points[8..].iter().all(|p| (p.y - 4.0).abs() < 1e-4), "the cut runs at mid height");
    let mut m = cube();
    let e = vertical(&m);
    mo::loop_cut(&mut m, e, 3, 0.5);
    assert_eq!((m.points.len(), m.faces.len()), (20, 18));
    assert_closed(&m, "three cuts");
    let mut heights: Vec<i32> = m.points[8..].iter().map(|p| (p.y * 100.0).round() as i32).collect();
    heights.sort();
    heights.dedup();
    assert_eq!(heights, [200, 400, 600]);
}

#[test]
fn subdivide_triangulate_and_back() {
    let mut m = cube();
    let all: BTreeSet<usize> = (0..6).collect();
    mo::subdivide(&mut m, &all);
    assert_eq!((m.points.len(), m.faces.len()), (26, 24));
    assert_closed(&m, "subdivide");
    let mut m = cube();
    mo::triangulate(&mut m, &all);
    assert_eq!(m.faces.len(), 12);
    assert_closed(&m, "triangulate");
    let tris: BTreeSet<usize> = (0..12).collect();
    assert_eq!(mo::tris_to_quads(&mut m, &tris), 6);
    assert_eq!(m.faces.len(), 6);
    assert_closed(&m, "tris to quads");
}

#[test]
fn merge_dissolve_delete_and_fill() {
    // Верхние четыре вершины в одну — пирамида.
    let mut m = cube();
    let top: BTreeSet<u32> = (0..8).filter(|v| m.points[*v as usize].y == 8.0).collect();
    mo::merge(&mut m, &top, None);
    assert_eq!((m.points.len(), m.faces.len()), (5, 5));
    assert_closed(&m, "merge at centre");

    // Поделить верх и растворить его обратно в одну грань.
    let mut m = cube();
    let up = face_toward(&m, Vec3::Y);
    let parts = mo::subdivide(&mut m, &set(&[up]));
    let merged = mo::dissolve_faces(&mut m, &parts);
    assert_eq!(merged.len(), 1);
    assert_eq!(m.faces.len(), 6);
    assert_closed(&m, "dissolve");

    // Удалить верх и заполнить дыру по краю.
    let mut m = cube();
    let up = face_toward(&m, Vec3::Y);
    let sel = mo::select_faces(&m, set(&[up]));
    mo::delete(&mut m, &sel, Delete::OnlyFaces);
    assert_eq!(m.faces.len(), 5);
    assert!(!m.is_closed_manifold());
    let boundary: BTreeSet<u32> = (0..m.points.len() as u32).filter(|v| m.points[*v as usize].y == 8.0).collect();
    let sel = mo::select_verts(&m, boundary);
    let f = mo::fill(&mut m, &sel).unwrap();
    assert!(m.normal(f).y > 0.99, "the new face looks out");
    assert_closed(&m, "fill");

    // Удаление вершины уносит её грани.
    let mut m = cube();
    let sel = mo::select_verts(&m, set(&[0]));
    mo::delete(&mut m, &sel, Delete::Vertices);
    assert_eq!((m.points.len(), m.faces.len()), (7, 3));
}

#[test]
fn flip_and_recalculate_normals() {
    let mut m = cube();
    mo::flip(&mut m, &set(&[0, 3]));
    assert!(!m.is_closed_manifold());
    mo::recalc_normals(&mut m);
    assert_closed(&m, "recalc");
    let all: BTreeSet<usize> = (0..6).collect();
    mo::flip(&mut m, &all);
    assert!(m.signed_volume() < 0.0);
    mo::recalc_normals(&mut m);
    assert_closed(&m, "recalc after flipping everything");
}

#[test]
fn duplicate_split_separate_solidify_bridge_mirror_array() {
    let mut m = cube();
    let new = mo::duplicate(&mut m, &set(&[0, 1]));
    assert_eq!((m.faces.len(), new.len()), (8, 2));
    let mut m = cube();
    let part = mo::separate(&mut m, &set(&[0]));
    assert_eq!((part.faces.len(), part.points.len(), m.faces.len()), (1, 4, 5));
    let mut m = cube();
    mo::split(&mut m, &set(&[0]));
    assert_eq!((m.faces.len(), m.points.len()), (6, 12));

    let mut plane = Primitive::Plane { size: 8.0 }.build();
    mo::solidify(&mut plane, 1.0);
    assert_closed(&plane, "solidify");
    assert!((plane.signed_volume() - 64.0).abs() < 1e-3);

    // Два квадрата друг над другом, смотрят в разные стороны: мост — коробка.
    let mut m = EditMesh::default();
    let b = mo::box_mesh(Vec3::ZERO, Vec3::splat(4.0));
    m.points = b.points.clone();
    m.faces = vec![b.faces[2].clone(), b.faces[3].clone()]; // −Y и +Y
    let edges = m.all_edges();
    let new = mo::bridge(&mut m, &edges).unwrap();
    assert_eq!(new.len(), 4);
    assert_closed(&m, "bridge");

    // Половина коробки без грани на плоскости X = 0 — зеркало делает целую.
    let mut half = mo::box_mesh(Vec3::new(0.0, 0.0, -2.0), Vec3::new(2.0, 4.0, 2.0));
    let west = face_toward(&half, Vec3::NEG_X);
    half.faces.remove(west);
    mo::mirror(&mut half, 0, 0.0);
    assert_eq!((half.points.len(), half.faces.len()), (12, 10));
    assert_closed(&half, "mirror");

    let mut m = cube();
    mo::array(&mut m, 3, Vec3::new(10.0, 0.0, 0.0));
    assert_eq!((m.points.len(), m.faces.len()), (24, 18));
    assert!((m.signed_volume() - 3.0 * 512.0).abs() < 1e-2);

    // Сужение: верх в два раза уже.
    let mut m = cube();
    let all: BTreeSet<u32> = (0..8).collect();
    mo::deform(&mut m, &all, 1, 0.5, 0.0);
    let top: Vec<Vec3> = m.points.iter().copied().filter(|p| p.y == 8.0).collect();
    assert!(top.iter().all(|p| (p.x.abs() - 2.0).abs() < 1e-4));
    assert_closed(&m, "taper");
}

#[test]
fn primitives_lathe_and_sweep_are_clean() {
    for p in Primitive::DEFAULTS {
        let m = p.build();
        m.check().unwrap_or_else(|e| panic!("{}: {e}", p.name()));
        let flat = matches!(p, Primitive::Plane { .. } | Primitive::Grid { .. } | Primitive::Circle { .. });
        if flat {
            assert!(m.normal(0).y > 0.99, "{}: faces up", p.name());
        } else if matches!(p, Primitive::Torus { .. }) {
            m.check().unwrap();
            assert!(m.is_closed_manifold() && m.signed_volume() > 0.0 && m.euler() == 0, "torus");
        } else if !matches!(p, Primitive::Stairs { .. }) {
            assert_closed(&m, p.name());
        } else {
            assert!(m.signed_volume() > 0.0);
        }
        let min_y = m.points.iter().map(|q| q.y).fold(f32::MAX, f32::min);
        assert!(min_y.abs() < 1e-3 || matches!(p, Primitive::Torus { .. }), "{} stands on the ground", p.name());
    }
    // Кувшин: профиль снизу вверх.
    let jar = mo::lathe(&[Vec2::new(0.0, 0.0), Vec2::new(3.0, 0.0), Vec2::new(4.0, 4.0), Vec2::new(2.0, 8.0), Vec2::new(2.5, 9.0), Vec2::new(0.0, 9.0)], 10, true);
    assert_closed(&jar, "jar");
    assert_eq!(jar.faces.len(), 10 * 5);
    let pipe = mo::sweep(&[Vec2::new(-1.0, -1.0), Vec2::new(1.0, -1.0), Vec2::new(1.0, 1.0), Vec2::new(-1.0, 1.0)], &[Vec3::ZERO, Vec3::new(0.0, 0.0, 8.0), Vec3::new(8.0, 0.0, 8.0)], true);
    assert_closed(&pipe, "sweep");
}

#[test]
fn selection_helpers() {
    let mut cyl = Primitive::Cylinder { radius: 4.0, height: 8.0, segments: 8, caps: false }.build();
    let vertical = cyl.all_edges().into_iter().find(|(a, b)| cyl.points[*a as usize].y != cyl.points[*b as usize].y).unwrap();
    mo::loop_cut(&mut cyl, vertical, 1, 0.5);
    let ring_edge = cyl.all_edges().into_iter().find(|(a, b)| (cyl.points[*a as usize].y - 4.0).abs() < 1e-3 && (cyl.points[*b as usize].y - 4.0).abs() < 1e-3).unwrap();
    let lp = mo::edge_loop(&cyl, ring_edge);
    let vf = cyl.vertex_faces();
    assert_eq!(lp.len(), 8, "a horizontal loop goes round the cylinder: {lp:?} from {ring_edge:?}; faces at ends {:?} {:?}; faces {:?}", vf[ring_edge.0 as usize], vf[ring_edge.1 as usize], cyl.faces.iter().map(|f| f.vertices.clone()).collect::<Vec<_>>());
    // Петля граней через вертикальное ребро нижнего ряда — весь ряд вокруг.
    let side = cyl.all_edges().into_iter().find(|(a, b)| cyl.points[*a as usize].y == 0.0 && (cyl.points[*b as usize].y - 4.0).abs() < 1e-3 || cyl.points[*b as usize].y == 0.0 && (cyl.points[*a as usize].y - 4.0).abs() < 1e-3).unwrap();
    assert_eq!(mo::face_loop(&cyl, side).len(), 8);
    let one: BTreeSet<u32> = set(&[0]);
    assert_eq!(mo::linked(&cyl, &one).len(), cyl.points.len());
    assert!(mo::grow(&cyl, &one).len() > 1);
    assert!(mo::shrink(&cyl, &mo::grow(&cyl, &one)).len() <= 1);
    let c = cube();
    assert_eq!(mo::similar_normal(&c, 0, 10.0, false).len(), 1);
}

#[test]
fn document_level_ops_allocate_uvs_undo_and_twins() {
    let mut doc = Document::new("t");
    let m = cube();
    let id = mesh_edit::add_mesh(&mut doc, "box", &m.points, m.faces.clone(), None);
    mesh_edit::generate_uvs(&mut doc, id, None).unwrap();
    let twin = ops::mirror_x(&mut doc, &[id])[0];
    let mut session = Session::new(doc);
    let before = session.doc.clone();
    // Выдавить верх и разложить стенки в свободные места атласа.
    session.edit("Extrude", |doc| {
        let sides = mo::apply(doc, id, |m| {
            let top = face_toward(m, Vec3::Y);
            mo::extrude_faces(m, &set(&[top]), 4.0).1
        })
        .unwrap();
        mesh_edit::allocate_uvs(doc, id, &sides).unwrap();
    });
    let doc = &session.doc;
    let mesh = mo::EditMesh::of(doc.element(id).unwrap()).unwrap();
    assert_closed(&mesh, "document extrude");
    let [w, h] = doc.textures[0].image.size();
    for f in &mesh.faces {
        assert!(f.uvs.iter().all(|uv| uv[0] >= 0.0 && uv[1] >= 0.0 && uv[0] <= w as f32 && uv[1] <= h as f32), "UV inside the atlas");
        let density = mesh_edit::face_density(&mesh.points, f);
        assert!((density - doc.texel_density).abs() < 0.05, "texel density {density}");
    }
    // Места граней в атласе не пересекаются.
    let rects = engine_modeler::uv::occupied(doc, 0, &[twin]);
    for (i, a) in rects.iter().enumerate() {
        for b in &rects[i + 1..] {
            let overlap = a[0] < b[0] + b[2] && b[0] < a[0] + a[2] && a[1] < b[1] + b[3] && b[1] < a[1] + a[3];
            assert!(!overlap || std::ptr::eq(a, b), "{a:?} overlaps {b:?}");
        }
    }
    // Близнец повторил выдавливание зеркально.
    let src: Vec<Vec3> = doc.element(id).unwrap().mesh_points();
    let tw: Vec<Vec3> = doc.element(twin).unwrap().mesh_points();
    let mut a: Vec<[i32; 3]> = src.iter().map(|p| [(-p.x * 100.0).round() as i32, (p.y * 100.0).round() as i32, (p.z * 100.0).round() as i32]).collect();
    let mut b: Vec<[i32; 3]> = tw.iter().map(|p| [(p.x * 100.0).round() as i32, (p.y * 100.0).round() as i32, (p.z * 100.0).round() as i32]).collect();
    a.sort();
    b.sort();
    assert_eq!(a, b, "the twin is the mirror image");
    // Отмена — тот же документ, повтор — тот же результат.
    let after = session.doc.clone();
    session.undo();
    assert_eq!(session.doc, before);
    session.redo();
    assert_eq!(session.doc, after);
}

#[test]
fn cube_to_mesh_and_back() {
    let mut doc = Document::new("t");
    let id = ops::add_element(&mut doc, "c", Element::cuboid(Vec3::new(-4.0, 0.0, -4.0), Vec3::splat(8.0)), None);
    let element = doc.element(id).unwrap().clone();
    let mesh = mo::cuboid_to_mesh(&doc, &element).unwrap();
    let edit = mo::EditMesh::of(&mesh).unwrap();
    assert_eq!((edit.points.len(), edit.faces.len()), (8, 6));
    assert_closed(&edit, "cube to mesh");
    let before = engine_modeler::bake(&doc);
    doc.node_mut(id).unwrap().kind = engine_modeler::NodeKind::Element(mesh);
    let after = engine_modeler::bake(&doc);
    let key = |b: &engine_modeler::BakedModel| {
        let mut v: Vec<[i32; 5]> = b.positions.iter().zip(&b.uvs).map(|(p, uv)| [(p[0] * 1e3) as i32, (p[1] * 1e3) as i32, (p[2] * 1e3) as i32, (uv[0] * 1e4) as i32, (uv[1] * 1e4) as i32]).collect();
        v.sort();
        v.dedup();
        v
    };
    assert_eq!(key(&after), key(&before), "the same corners with the same UVs");
    let back = mesh_edit::convert_to_cubes(&mut doc, id).unwrap();
    assert_eq!(back.len(), 1);
}

/// Случайные цепочки операций на случайных примитивах: без паник, грани корректны.
#[test]
fn random_operation_sequences_keep_invariants() {
    let mut seed = 0x9e37_79b9_7f4a_7c15u64;
    let mut next = move |n: usize| {
        seed ^= seed << 13;
        seed ^= seed >> 7;
        seed ^= seed << 17;
        (seed % n.max(1) as u64) as usize
    };
    for round in 0..120 {
        let mut m = Primitive::DEFAULTS[next(Primitive::DEFAULTS.len())].build();
        for f in &mut m.faces {
            f.uvs = vec![[1.0, 1.0]; f.vertices.len()];
        }
        for step in 0..8 {
            if m.faces.is_empty() {
                break;
            }
            let f = next(m.faces.len());
            let faces = set(&[f]);
            let edges = m.all_edges();
            let e = *edges.iter().nth(next(edges.len())).unwrap();
            let verts = set(&[e.0, e.1]);
            let op = next(14);
            match op {
                0 => drop(mo::extrude_faces(&mut m, &faces, 1.0 + next(3) as f32)),
                1 => drop(mo::inset(&mut m, &faces, 0.5)),
                2 => drop(mo::loop_cut(&mut m, e, 1 + next(3), 0.5)),
                3 => drop(mo::subdivide(&mut m, &faces)),
                4 => mo::triangulate(&mut m, &faces),
                5 => drop(mo::merge(&mut m, &verts, None)),
                6 => {
                    let faces = mo::select_verts(&m, verts.clone()).faces;
                    drop(mo::dissolve_faces(&mut m, &faces))
                }
                7 => {
                    let sel = mo::select_faces(&m, faces);
                    mo::delete(&mut m, &sel, Delete::Faces)
                }
                8 => {
                    let sel = mo::select_verts(&m, mo::grow(&m, &verts));
                    drop(mo::fill(&mut m, &sel))
                }
                9 => mo::recalc_normals(&mut m),
                10 => drop(mo::extrude_edges(&mut m, &set(&[e]), 1.0)),
                11 => {
                    let all: BTreeSet<usize> = (0..m.faces.len()).collect();
                    mo::tris_to_quads(&mut m, &all);
                }
                12 => drop(mo::merge_by_distance(&mut m, 0.5)),
                _ => mo::dissolve_edges(&mut m, &set(&[e])),
            }
            m.check().unwrap_or_else(|err| panic!("round {round} step {step} op {op}: {err}"));
        }
    }
}

#[test]
fn bevel_and_knife() {
    // Фаска верхнего переднего ребра куба: 10 вершин, 7 граней, замкнут, объём меньше.
    let mut m = cube();
    let e = m.all_edges().into_iter().find(|(a, b)| {
        let (p, q) = (m.points[*a as usize], m.points[*b as usize]);
        p.y == 8.0 && q.y == 8.0 && p.z == 4.0 && q.z == 4.0
    }).unwrap();
    let new = mo::bevel(&mut m, &set(&[e]), 1.0).unwrap();
    assert_eq!((m.points.len(), m.faces.len(), new.len()), (10, 7, 1));
    assert_closed(&m, "bevel");
    assert!((m.signed_volume() - (512.0 - 0.5 * 8.0)).abs() < 1e-3, "a 1×1 corner prism is cut off");
    let n = m.normal(new[0]);
    assert!(n.y > 0.6 && n.z > 0.6, "the chamfer faces up and forward: {n}");
    // Нож через верх куба: верх делится на две, соседи получают точки.
    let mut m = cube();
    let up = face_toward(&m, Vec3::Y);
    let es = m.face_edges(up);
    let (e1, e2) = (edge(es[0].0, es[0].1), edge(es[2].0, es[2].1));
    mo::knife(&mut m, up, (e1, 0.5), (e2, 0.5)).unwrap();
    assert_eq!((m.points.len(), m.faces.len()), (10, 7));
    assert_closed(&m, "knife");
}

/// Скорость на меше в 20 000 треугольников: каждая операция — доли секунды.
/// `cargo test --test modeling -- --nocapture performance` печатает миллисекунды.
#[test]
fn performance_on_a_20k_triangle_mesh() {
    let profile: Vec<Vec2> = (0..=100).map(|i| Vec2::new(4.0 + (i as f32 * 0.3).sin(), i as f32 * 0.2)).collect();
    let big = mo::lathe(&profile, 100, true);
    let tris: usize = big.faces.iter().map(|f| f.vertices.len() - 2).sum();
    assert!(tris >= 20_000, "{tris}");
    let mut timings = Vec::new();
    let mut time = |name: &str, f: &mut dyn FnMut()| {
        let start = std::time::Instant::now();
        f();
        let ms = start.elapsed().as_secs_f64() * 1000.0;
        timings.push(format!("{name}: {ms:.1} ms"));
        assert!(ms < 2000.0, "{name} took {ms} ms");
    };
    let mut doc = Document::new("big");
    let id = mesh_edit::add_mesh(&mut doc, "big", &big.points, big.faces.clone(), None);
    time("generate UVs", &mut || drop(mesh_edit::generate_uvs(&mut doc, id, None)));
    time("extrude one face (document)", &mut || drop(mo::apply(&mut doc, id, |m| mo::extrude_faces(m, &set(&[500]), 1.0))));
    time("inset 100 faces", &mut || drop(mo::apply(&mut doc, id, |m| mo::inset(m, &(1000..1100).collect(), 0.05))));
    let e = mo::EditMesh::of(doc.element(id).unwrap()).unwrap().all_edges().into_iter().nth(4000).unwrap();
    time("loop cut", &mut || drop(mo::apply(&mut doc, id, |m| mo::loop_cut(m, e, 2, 0.5))));
    time("recalculate normals", &mut || { mo::apply(&mut doc, id, mo::recalc_normals); });
    time("merge by distance", &mut || { mo::apply(&mut doc, id, |m| mo::merge_by_distance(m, 1e-4)); });
    time("build the view mesh", &mut || drop(engine_modeler::build_mesh(&doc, Default::default())));
    time("bake for the engine", &mut || drop(engine_modeler::bake(&doc)));
    println!("{}", timings.join("\n"));
}
