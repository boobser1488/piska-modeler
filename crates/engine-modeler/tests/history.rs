//! Длинная случайная последовательность операций: Undo до начала и Redo до конца
//! возвращают документ байт в байт на каждом шаге.

use engine_modeler::doc::{FaceDir, Shape};
use engine_modeler::ops::{self, Align};
use engine_modeler::{format, paint, Document, Element, NodeId, Session};
use glam::Vec3;

/// Простой детерминированный генератор (LCG) — тест воспроизводим.
struct Rng(u64);

impl Rng {
    fn next(&mut self) -> u32 {
        self.0 = self.0.wrapping_mul(6364136223846793005).wrapping_add(1442695040888963407);
        (self.0 >> 33) as u32
    }

    fn below(&mut self, n: usize) -> usize {
        self.next() as usize % n.max(1)
    }

    fn float(&mut self, lo: f32, hi: f32) -> f32 {
        lo + (self.next() % 10_000) as f32 / 10_000.0 * (hi - lo)
    }

    fn pick(&mut self, ids: &[NodeId]) -> Option<NodeId> {
        (!ids.is_empty()).then(|| ids[self.below(ids.len())])
    }
}

fn random_step(session: &mut Session, rng: &mut Rng) {
    let all: Vec<NodeId> = session.doc.nodes.iter().map(|n| n.id).collect();
    let elements: Vec<NodeId> = session.doc.elements().map(|(id, _)| id).collect();
    let groups: Vec<NodeId> = session.doc.nodes.iter().filter(|n| n.is_group()).map(|n| n.id).collect();
    match rng.below(16) {
        0 | 1 => {
            let shape = match rng.below(5) {
                0 => Shape::Plane,
                1 => Shape::Cylinder { segments: 3 + rng.below(8) as u32 },
                2 => Shape::Cone { segments: 3 + rng.below(8) as u32 },
                3 => Shape::Sphere { segments: 4 + rng.below(6) as u32, rings: 2 + rng.below(4) as u32 },
                _ => Shape::Cuboid,
            };
            let from = Vec3::new(rng.float(-16.0, 16.0), rng.float(0.0, 8.0), rng.float(-16.0, 16.0)).round();
            let size = Vec3::new(rng.float(1.0, 10.0), rng.float(1.0, 10.0), rng.float(1.0, 10.0)).round();
            let parent = if rng.below(2) == 0 { rng.pick(&groups) } else { None };
            session.edit("Add", |doc| ops::add_element(doc, "part", Element::new(shape, from, size), parent));
        }
        2 => {
            let parent = rng.pick(&groups);
            let origin = Vec3::new(rng.float(-8.0, 8.0), rng.float(0.0, 16.0), 0.0);
            session.edit("Group", |doc| ops::add_group(doc, "group", origin, parent));
        }
        3 => {
            if let Some(id) = rng.pick(&all) {
                session.edit("Delete", |doc| ops::delete(doc, &[id]));
            }
        }
        4 => {
            if let Some(id) = rng.pick(&all) {
                session.edit("Duplicate", |doc| ops::duplicate(doc, &[id]));
            }
        }
        5 => {
            if let Some(id) = rng.pick(&all) {
                let delta = Vec3::new(rng.float(-4.0, 4.0), rng.float(-4.0, 4.0), rng.float(-4.0, 4.0));
                // Перетаскивание: много кадров — один шаг истории.
                session.begin("Move");
                for _ in 0..3 {
                    ops::move_by(&mut session.doc, &[id], delta / 3.0);
                }
                session.commit();
            }
        }
        6 => {
            if let Some(id) = rng.pick(&elements) {
                let face = FaceDir::ALL[rng.below(6)];
                let amount = rng.float(-3.0, 5.0).round();
                session.edit("Resize", |doc| ops::resize_face(doc, id, face, amount));
            }
        }
        7 => {
            if let Some(id) = rng.pick(&all) {
                let axis = rng.below(3);
                let angle = [22.5, 15.0, -5.0, 45.0][rng.below(4)];
                session.edit("Rotate", |doc| ops::rotate_by(doc, &[id], axis, angle));
            }
        }
        8 => {
            if let Some(id) = rng.pick(&elements) {
                session.edit("Mirror", |doc| ops::mirror_x(doc, &[id]));
            }
        }
        9 => {
            let picked: Vec<NodeId> = (0..3).filter_map(|_| rng.pick(&all)).collect();
            let axis = rng.below(3);
            match rng.below(4) {
                0 => session.edit("Flip", |doc| ops::flip(doc, &picked, axis)),
                1 => session.edit("Align", |doc| ops::align(doc, &picked, axis, Align::Center)),
                2 => session.edit("Distribute", |doc| ops::distribute(doc, &picked, axis)),
                _ => session.edit("Center", |doc| ops::center(doc, &picked)),
            }
        }
        10 => {
            if let (Some(id), parent) = (rng.pick(&all), rng.pick(&groups)) {
                let index = rng.below(4);
                session.edit("Reparent", |doc| ops::reparent(doc, id, parent, Some(index)));
            }
        }
        11 | 12 => {
            // Штрих кисти: несколько мазков — один шаг.
            session.begin("Paint");
            let color = [rng.below(256) as u8, rng.below(256) as u8, rng.below(256) as u8, 255];
            let (w, h) = (session.doc.textures[0].image.width() as i32, session.doc.textures[0].image.height() as i32);
            let (mut x, mut y) = (rng.below(w as usize) as i32, rng.below(h as usize) as i32);
            for _ in 0..6 {
                let (nx, ny) = (x + rng.below(5) as i32 - 2, y + rng.below(5) as i32 - 2);
                paint::line(&mut session.doc.textures[0].image, (x, y), (nx, ny), 1 + rng.below(3) as u32, color, None);
                (x, y) = (nx, ny);
            }
            session.commit();
        }
        13 => {
            if let Some(id) = rng.pick(&all) {
                let visible = rng.below(2) == 0;
                session.edit("Visibility", |doc| ops::set_visible(doc, id, visible));
            }
        }
        14 => {
            session.edit("Repack", |doc| engine_modeler::uv::repack(doc, 0));
        }
        _ => {
            if let Some(id) = rng.pick(&all) {
                session.edit("Rename", |doc| ops::rename(doc, id, &format!("n{}", rng.below(100))));
            }
        }
    }
}

#[test]
fn undo_redo_restores_documents_bit_for_bit() {
    let mut rng = Rng(0x5eed_1234);
    let mut session = Session::new(Document::new("random"));
    // Состояния после каждого записанного шага (текст файла — сравнение байт в байт).
    let mut states = vec![format::to_string(&session.doc)];
    for _ in 0..220 {
        let before = session.history_len().0;
        random_step(&mut session, &mut rng);
        if session.history_len().0 > before {
            states.push(format::to_string(&session.doc));
        } else {
            assert_eq!(format::to_string(&session.doc), *states.last().unwrap(), "an unrecorded step changed nothing");
        }
    }
    let steps = states.len() - 1;
    assert!(steps > 150, "most random steps change the document ({steps})");
    assert!(session.doc.element_count() > 3);

    for expected in states.iter().rev().skip(1) {
        assert!(session.undo());
        assert_eq!(format::to_string(&session.doc), *expected);
    }
    assert!(!session.undo(), "the history starts at the empty document");
    for expected in states.iter().skip(1) {
        assert!(session.redo());
        assert_eq!(format::to_string(&session.doc), *expected);
    }
    assert!(!session.redo());
}

#[test]
fn twins_stay_mirrored_through_random_edits() {
    let mut rng = Rng(42);
    let mut session = Session::new(Document::new("mirror"));
    let source = session.edit("Add", |doc| {
        ops::add_element(doc, "arm_left", Element::cuboid(Vec3::new(3.0, 4.0, -1.0), Vec3::new(2.0, 8.0, 2.0)), None)
    });
    let twin = session.edit("Mirror", |doc| ops::mirror_x(doc, &[source])[0]);
    for _ in 0..100 {
        let id = if rng.below(2) == 0 { source } else { twin };
        match rng.below(3) {
            0 => session.edit("Move", |doc| ops::move_by(doc, &[id], Vec3::new(rng.float(-2.0, 2.0), 1.0, 0.0))),
            1 => {
                let face = FaceDir::ALL[rng.below(6)];
                session.edit("Resize", |doc| ops::resize_face(doc, id, face, 1.0));
            }
            _ => session.edit("Rotate", |doc| ops::rotate_by(doc, &[id], rng.below(3), 15.0)),
        }
        let a = session.doc.element_corners(source).unwrap();
        let b = session.doc.element_corners(twin).unwrap();
        for p in a {
            let m = Vec3::new(-p.x, p.y, p.z);
            assert!(b.iter().any(|q| (*q - m).length() < 1e-3), "{p} is mirrored");
        }
    }
}
