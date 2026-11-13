//! Операции над документом. Каждая — обычная функция над `&mut Document`; историю
//! правок ведёт [`crate::Session`].
//!
//! После любой правки элемента его отражённый близнец (если есть) обновляется:
//! см. [`sync_twins`].

use glam::{Mat4, Vec3};

use crate::doc::{Document, Element, FaceDir, Group, Node, NodeId, NodeKind, Shape, Texture};
use crate::image::{Image, Rgba, CLEAR};
use crate::uv;

/// Цвет, которым заливаются стороны нового элемента — чтобы было по чему рисовать.
pub const BASE_COLOR: Rgba = [0xc6, 0xc2, 0xba, 0xff];

/// Как выровнять по оси.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Align {
    Min,
    Center,
    Max,
}

/// Добавить элемент. Развёртка коробкой размещается в свободном месте атласа, стороны
/// заливаются `BASE_COLOR`.
pub fn add_element(doc: &mut Document, name: &str, element: Element, parent: Option<NodeId>) -> NodeId {
    let id = doc.allocate_id();
    let name = doc.unique_name(name);
    let parent = parent.filter(|p| doc.node(*p).is_some_and(Node::is_group));
    let box_uv = element.box_uv;
    doc.nodes.push(Node {
        id,
        name,
        parent,
        visible: true,
        locked: false,
        kind: NodeKind::Element(element),
    });
    if box_uv && uv::place(doc, id) {
        uv::fill_faces(doc, id, BASE_COLOR);
    }
    id
}

/// Кубоид по умолчанию для кнопок панели: стоит на земле в начале координат.
pub fn default_element(shape: Shape) -> Element {
    match shape {
        Shape::Plane => Element::new(shape, Vec3::new(-8.0, 0.0, -8.0), Vec3::new(16.0, 0.0, 16.0)),
        Shape::Sphere { .. } => Element::new(shape, Vec3::new(-4.0, 0.0, -4.0), Vec3::splat(8.0)),
        _ => Element::new(shape, Vec3::new(-4.0, 0.0, -4.0), Vec3::new(8.0, 8.0, 8.0)),
    }
}

/// Добавить группу.
pub fn add_group(doc: &mut Document, name: &str, origin: Vec3, parent: Option<NodeId>) -> NodeId {
    let id = doc.allocate_id();
    let name = doc.unique_name(name);
    let parent = parent.filter(|p| doc.node(*p).is_some_and(Node::is_group));
    doc.nodes.push(Node {
        id,
        name,
        parent,
        visible: true,
        locked: false,
        kind: NodeKind::Group(Group {
            origin,
            rotation: Vec3::ZERO,
        }),
    });
    id
}

/// Удалить узлы вместе с потомками. Близнецы удалённых элементов становятся обычными.
pub fn delete(doc: &mut Document, ids: &[NodeId]) {
    let mut doomed: Vec<NodeId> = Vec::new();
    for id in ids {
        if doc.node(*id).is_some() {
            doomed.push(*id);
            doomed.extend(doc.descendants(*id));
        }
    }
    for node in &mut doc.nodes {
        if let Some(e) = node.element_mut() {
            if e.twin.is_some_and(|t| doomed.contains(&t)) {
                e.twin = None;
            }
        }
    }
    // Освободить место в атласе: развёртки удалённых элементов стираются.
    for id in &doomed {
        if let Some(e) = doc.element(*id).cloned() {
            if e.box_uv {
                let [w, h] = uv::footprint(&e, doc.texel_density);
                if let Some(tex) = doc.textures.get_mut(uv::element_texture(&e) as usize) {
                    for y in e.uv_origin[1]..e.uv_origin[1] + h {
                        for x in e.uv_origin[0]..e.uv_origin[0] + w {
                            tex.image.set(x as i32, y as i32, CLEAR);
                        }
                    }
                }
            }
        }
    }
    doc.nodes.retain(|n| !doomed.contains(&n.id));
}

/// Дублировать узлы (с поддеревьями). Копия встаёт сразу после оригинала; развёртки
/// копий получают своё место в атласе вместе с нарисованным. Возвращает новые корни.
pub fn duplicate(doc: &mut Document, ids: &[NodeId]) -> Vec<NodeId> {
    let roots = top_level(doc, ids);
    let mut created = Vec::new();
    for root in roots {
        let mut subtree = vec![root];
        subtree.extend(doc.descendants(root));
        let mut mapping: Vec<(NodeId, NodeId)> = Vec::new();
        for old in &subtree {
            mapping.push((*old, doc.allocate_id()));
        }
        let map = |id: NodeId| mapping.iter().find(|(o, _)| *o == id).map(|(_, n)| *n);
        let mut copies = Vec::new();
        for old in &subtree {
            let Some(node) = doc.node(*old) else { continue };
            let mut copy = node.clone();
            copy.id = map(*old).expect("mapped above");
            if *old == root {
                copy.name = doc.unique_name(&node.name);
            } else {
                copy.parent = copy.parent.and_then(map);
            }
            if let Some(e) = copy.element_mut() {
                e.twin = None;
            }
            copies.push(copy);
        }
        let at = doc.index_of(root).map_or(doc.nodes.len(), |i| i + 1);
        let new_root = copies[0].id;
        for (offset, copy) in copies.into_iter().enumerate() {
            doc.nodes.insert(at + offset, copy);
        }
        // Развёртки копий — на новое место, с картинкой оригинала.
        for (old, new) in &mapping {
            let Some(original) = doc.element(*old).cloned() else { continue };
            if !original.box_uv {
                continue;
            }
            if uv::place(doc, *new) {
                copy_footprint(doc, &original, *new);
            }
        }
        created.push(new_root);
    }
    created
}

/// Скопировать картинку развёртки элемента `from` в развёртку элемента `to`.
pub fn copy_footprint(doc: &mut Document, from: &Element, to: NodeId) {
    let Some(target) = doc.element(to).cloned() else { return };
    let size = uv::footprint(from, doc.texel_density);
    let source = doc.textures.get(uv::element_texture(from) as usize).map(|t| t.image.clone());
    let Some(source) = source else { return };
    if let Some(tex) = doc.textures.get_mut(uv::element_texture(&target) as usize) {
        tex.image.blit(
            &source,
            from.uv_origin[0] as i32,
            from.uv_origin[1] as i32,
            target.uv_origin[0] as i32,
            target.uv_origin[1] as i32,
            size[0],
            size[1],
        );
    }
}

/// Узлы без выбранных предков (чтобы не обрабатывать потомка дважды).
pub fn top_level(doc: &Document, ids: &[NodeId]) -> Vec<NodeId> {
    ids.iter()
        .copied()
        .filter(|id| doc.node(*id).is_some())
        .filter(|id| !ids.iter().any(|other| other != id && doc.is_ancestor(*other, *id)))
        .collect()
}

/// Все элементы среди узлов и их потомков.
pub fn elements_of(doc: &Document, ids: &[NodeId]) -> Vec<NodeId> {
    let mut out = Vec::new();
    for id in top_level(doc, ids) {
        let mut all = vec![id];
        all.extend(doc.descendants(id));
        for node in all {
            if doc.element(node).is_some() && !out.contains(&node) {
                out.push(node);
            }
        }
    }
    out
}

pub fn rename(doc: &mut Document, id: NodeId, name: &str) {
    let name = name.trim();
    if name.is_empty() {
        return;
    }
    if let Some(node) = doc.node_mut(id) {
        node.name = name.to_string();
    }
}

pub fn set_visible(doc: &mut Document, id: NodeId, visible: bool) {
    if let Some(node) = doc.node_mut(id) {
        node.visible = visible;
    }
}

pub fn set_locked(doc: &mut Document, id: NodeId, locked: bool) {
    if let Some(node) = doc.node_mut(id) {
        node.locked = locked;
    }
}

/// Перенести узел к новому родителю (`None` — в корень) на место `index` среди его детей
/// (`None` — в конец). Узел нельзя вложить в самого себя или своего потомка; родителем
/// может быть только группа. Возвращает, получилось ли.
pub fn reparent(doc: &mut Document, id: NodeId, parent: Option<NodeId>, index: Option<usize>) -> bool {
    if let Some(p) = parent {
        if doc.is_ancestor(id, p) || !doc.node(p).is_some_and(Node::is_group) {
            return false;
        }
    }
    let Some(at) = doc.index_of(id) else { return false };
    let mut node = doc.nodes.remove(at);
    node.parent = parent;
    let siblings: Vec<usize> = doc
        .nodes
        .iter()
        .enumerate()
        .filter(|(_, n)| n.parent == parent)
        .map(|(i, _)| i)
        .collect();
    let position = match index.and_then(|i| siblings.get(i)) {
        Some(before) => *before,
        None => siblings.last().map_or(doc.nodes.len(), |last| last + 1),
    };
    doc.nodes.insert(position.min(doc.nodes.len()), node);
    true
}

/// Сдвинуть узлы на `delta` в мировых координатах (группа — со всем содержимым).
pub fn move_by(doc: &mut Document, ids: &[NodeId], delta: Vec3) {
    let roots = top_level(doc, ids);
    let mut changed = Vec::new();
    for root in roots {
        // Сдвиг в пространстве родителя узла: группы над ним могут быть повёрнуты.
        let local = doc.parent_matrix(root).inverse().transform_vector3(delta);
        let mut all = vec![root];
        all.extend(doc.descendants(root));
        for id in all {
            let Some(node) = doc.node_mut(id) else { continue };
            match &mut node.kind {
                NodeKind::Element(e) => {
                    e.from += local;
                    e.pivot += local;
                    changed.push(id);
                }
                NodeKind::Group(g) => g.origin += local,
            }
        }
    }
    sync_twins(doc, &changed);
}

/// Заменить элемент целиком (числовые поля, гизмо). Развёртка и близнец обновляются.
pub fn set_element(doc: &mut Document, id: NodeId, element: Element) {
    let Some(old) = doc.element(id).cloned() else { return };
    if let Some(e) = doc.element_mut(id) {
        *e = element;
    }
    uv::refit(doc, id, &old);
    sync_twins(doc, &[id]);
}

/// Заменить группу.
pub fn set_group(doc: &mut Document, id: NodeId, group: Group) {
    if let Some(g) = doc.node_mut(id).and_then(Node::group_mut) {
        *g = group;
    }
}

/// Сдвинуть сторону элемента наружу на `amount` (внутрь — отрицательное): противоположная
/// сторона остаётся на месте. Размер не становится меньше нуля.
pub fn resize_face(doc: &mut Document, id: NodeId, face: FaceDir, amount: f32) {
    let Some(mut element) = doc.element(id).cloned() else { return };
    let axis = face.axis();
    let amount = amount.max(-element.size[axis]);
    element.size[axis] += amount;
    if !face.positive() {
        element.from[axis] -= amount;
    }
    set_element(doc, id, element);
}

/// Отразить элементы по оси `axis` (0 — X, 1 — Y, 2 — Z) относительно центра их общих
/// границ: геометрия, поворот, точка вращения и картинки сторон.
pub fn flip(doc: &mut Document, ids: &[NodeId], axis: usize) {
    let elements = elements_of(doc, ids);
    let Some((min, max)) = doc.bounds(&elements) else { return };
    let center = (min[axis] + max[axis]) * 0.5;
    for id in &elements {
        let Some(mut e) = doc.element(*id).cloned() else { continue };
        e.from[axis] = 2.0 * center - (e.from[axis] + e.size[axis]);
        e.pivot[axis] = 2.0 * center - e.pivot[axis];
        for other in 0..3 {
            if other != axis {
                e.rotation[other] = -e.rotation[other];
            }
        }
        mirror_faces(&mut e, axis);
        if let Some(target) = doc.element_mut(*id) {
            *target = e;
        }
    }
    sync_twins(doc, &elements);
}

/// Картинки сторон зеркально по оси: стороны этой оси меняются местами, остальные
/// отражаются в своей плоскости.
fn mirror_faces(e: &mut Element, axis: usize) {
    let (a, b) = match axis {
        0 => (FaceDir::East, FaceDir::West),
        1 => (FaceDir::Up, FaceDir::Down),
        _ => (FaceDir::North, FaceDir::South),
    };
    let round = !matches!(e.shape, Shape::Cuboid | Shape::Plane);
    if !round {
        e.faces.swap(a.index(), b.index());
    }
    for dir in FaceDir::ALL {
        let uv = &mut e.faces[dir.index()].uv;
        // Для верхней и нижней сторон вертикаль картинки — ось Z, горизонталь — X.
        let vertical = match axis {
            1 => !matches!(dir, FaceDir::Up | FaceDir::Down),
            2 => matches!(dir, FaceDir::Up | FaceDir::Down),
            _ => false,
        };
        if vertical {
            uv.mirror_vertical();
        } else {
            uv.mirror_horizontal();
        }
    }
    if round && axis == 1 {
        e.faces.swap(FaceDir::Up.index(), FaceDir::Down.index());
    }
}

/// Сдвинуть узлы так, чтобы центр их границ встал на X = 0 и Z = 0 (по высоте — как было).
pub fn center(doc: &mut Document, ids: &[NodeId]) {
    let Some((min, max)) = doc.bounds(ids) else { return };
    let c = (min + max) * 0.5;
    move_by(doc, ids, Vec3::new(-c.x, 0.0, -c.z));
}

/// Выровнять узлы по оси: по минимуму, центру или максимуму их общих границ.
pub fn align(doc: &mut Document, ids: &[NodeId], axis: usize, mode: Align) {
    let roots = top_level(doc, ids);
    let Some((min, max)) = doc.bounds(&roots) else { return };
    let target = match mode {
        Align::Min => min[axis],
        Align::Center => (min[axis] + max[axis]) * 0.5,
        Align::Max => max[axis],
    };
    for id in roots {
        let Some((lo, hi)) = doc.bounds(&[id]) else { continue };
        let current = match mode {
            Align::Min => lo[axis],
            Align::Center => (lo[axis] + hi[axis]) * 0.5,
            Align::Max => hi[axis],
        };
        let mut delta = Vec3::ZERO;
        delta[axis] = target - current;
        move_by(doc, &[id], delta);
    }
}

/// Расставить узлы по оси с равными промежутками между центрами (крайние на месте).
pub fn distribute(doc: &mut Document, ids: &[NodeId], axis: usize) {
    let roots = top_level(doc, ids);
    let mut centers: Vec<(NodeId, f32)> = roots
        .iter()
        .filter_map(|id| doc.bounds(&[*id]).map(|(lo, hi)| (*id, (lo[axis] + hi[axis]) * 0.5)))
        .collect();
    if centers.len() < 3 {
        return;
    }
    centers.sort_by(|a, b| a.1.total_cmp(&b.1));
    let (first, last) = (centers[0].1, centers[centers.len() - 1].1);
    let step = (last - first) / (centers.len() - 1) as f32;
    for (i, (id, c)) in centers.iter().enumerate() {
        let mut delta = Vec3::ZERO;
        delta[axis] = first + step * i as f32 - c;
        move_by(doc, &[*id], delta);
    }
}

/// Создать отражённых по X близнецов для элементов. Близнец связан с оригиналом:
/// правка любого из них повторяется на другом; картинки сторон общие (отражённые).
pub fn mirror_x(doc: &mut Document, ids: &[NodeId]) -> Vec<NodeId> {
    let mut created = Vec::new();
    for id in elements_of(doc, ids) {
        let Some(node) = doc.node(id).cloned() else { continue };
        let Some(element) = node.element() else { continue };
        if element.twin.is_some() {
            continue;
        }
        let twin_id = doc.allocate_id();
        let mut twin = mirrored(element);
        twin.box_uv = false;
        twin.twin = Some(id);
        let name = doc.unique_name(&mirror_name(&node.name));
        let at = doc.index_of(id).map_or(doc.nodes.len(), |i| i + 1);
        doc.nodes.insert(
            at,
            Node {
                id: twin_id,
                name,
                parent: node.parent,
                visible: node.visible,
                locked: node.locked,
                kind: NodeKind::Element(twin),
            },
        );
        if let Some(e) = doc.element_mut(id) {
            e.twin = Some(twin_id);
        }
        created.push(twin_id);
    }
    created
}

/// Имя близнеца: `arm_left` ↔ `arm_right`, `leg.L` ↔ `leg.R`, иначе `<имя>_mirror`.
pub fn mirror_name(name: &str) -> String {
    for (a, b) in [("left", "right"), ("Left", "Right"), (".L", ".R"), ("_l", "_r"), ("_L", "_R")] {
        if let Some(stem) = name.strip_suffix(a) {
            return format!("{stem}{b}");
        }
        if let Some(stem) = name.strip_suffix(b) {
            return format!("{stem}{a}");
        }
    }
    format!("{name}_mirror")
}

/// Разорвать связь близнецов (оба остаются как есть).
pub fn unlink_twin(doc: &mut Document, id: NodeId) {
    let twin = doc.element(id).and_then(|e| e.twin);
    for node in [Some(id), twin].into_iter().flatten() {
        if let Some(e) = doc.element_mut(node) {
            e.twin = None;
        }
    }
}

/// Отражение элемента по X относительно X = 0 (без связей и без развёртки).
pub fn mirrored(e: &Element) -> Element {
    let mut m = e.clone();
    m.from.x = -(e.from.x + e.size.x);
    m.pivot.x = -e.pivot.x;
    m.rotation.y = -e.rotation.y;
    m.rotation.z = -e.rotation.z;
    if matches!(e.shape, Shape::Cuboid | Shape::Plane) {
        m.faces.swap(FaceDir::East.index(), FaceDir::West.index());
    }
    for face in &mut m.faces {
        face.uv.mirror_horizontal();
    }
    m
}

/// Повторить элементы `ids` на их близнецах: геометрия и стороны отражаются по X;
/// у близнеца остаются его собственные связи и признак развёртки.
pub fn sync_twins(doc: &mut Document, ids: &[NodeId]) {
    for id in ids {
        let Some(source) = doc.element(*id).cloned() else { continue };
        let Some(twin_id) = source.twin else { continue };
        if ids.contains(&twin_id) && twin_id < *id {
            continue; // оба изменились — ведущим считается первый по id
        }
        let Some(target) = doc.element(twin_id).cloned() else { continue };
        let mut copy = mirrored(&source);
        copy.twin = target.twin;
        copy.box_uv = target.box_uv;
        copy.uv_origin = target.uv_origin;
        if target.box_uv {
            // Ведомый со своей развёрткой: геометрия общая, картинки свои.
            copy.faces = target.faces;
        }
        if let Some(e) = doc.element_mut(twin_id) {
            *e = copy;
        }
        if target.box_uv {
            uv::refit(doc, twin_id, &target);
            // Развёртка ведущего-оригинала могла переехать: ведомый берёт её заново.
            if !source.box_uv {
                sync_twins(doc, &[twin_id]);
            }
        }
    }
}

/// Включить автоматическую развёртку для элементов и разложить их заново.
pub fn auto_uv(doc: &mut Document, ids: &[NodeId]) {
    for id in elements_of(doc, ids) {
        let Some(e) = doc.element(id) else { continue };
        if e.twin.is_some() && !e.box_uv && doc.element(e.twin.unwrap()).is_some_and(|t| t.box_uv) {
            continue; // близнец берёт развёртку оригинала
        }
        if let Some(e) = doc.element_mut(id) {
            e.box_uv = true;
        }
        if uv::place(doc, id) {
            uv::fill_faces(doc, id, BASE_COLOR);
        }
        sync_twins(doc, &[id]);
    }
}

/// Задать прямоугольник стороны вручную (элемент перестаёт раскладываться автоматически).
pub fn set_face_uv(doc: &mut Document, id: NodeId, face: FaceDir, rect: crate::doc::UvRect) {
    if let Some(e) = doc.element_mut(id) {
        e.box_uv = false;
        e.face_mut(face).uv = rect;
    }
    sync_twins(doc, &[id]);
}

/// Добавить пустую текстуру; возвращает её номер.
pub fn add_texture(doc: &mut Document, name: &str, width: u32, height: u32) -> u32 {
    doc.textures.push(Texture {
        name: name.to_string(),
        image: Image::new(width, height, CLEAR),
        source: None,
    });
    doc.textures.len() as u32 - 1
}

/// Добавить текстуру из PNG.
pub fn import_texture(doc: &mut Document, name: &str, png: &[u8], source: Option<String>) -> Result<u32, crate::ModelError> {
    let image = Image::from_png(png)?;
    doc.textures.push(Texture {
        name: name.to_string(),
        image,
        source,
    });
    Ok(doc.textures.len() as u32 - 1)
}

/// Заменить картинку текстуры (перечитанный файл).
pub fn replace_texture_image(doc: &mut Document, texture: u32, image: Image) {
    if let Some(t) = doc.textures.get_mut(texture as usize) {
        t.image = image;
    }
}

/// Новый размер холста текстуры (содержимое остаётся слева сверху).
pub fn resize_texture(doc: &mut Document, texture: u32, width: u32, height: u32) {
    let (width, height) = (width.clamp(1, uv::MAX_TEXTURE_SIZE), height.clamp(1, uv::MAX_TEXTURE_SIZE));
    if let Some(t) = doc.textures.get_mut(texture as usize) {
        t.image = t.image.resized_canvas(width, height, CLEAR);
    }
}

/// Повернуть узлы на `degrees` вокруг оси `axis` (добавить к углу).
pub fn rotate_by(doc: &mut Document, ids: &[NodeId], axis: usize, degrees: f32) {
    let mut changed = Vec::new();
    for id in top_level(doc, ids) {
        if let Some(node) = doc.node_mut(id) {
            match &mut node.kind {
                NodeKind::Element(e) => {
                    e.rotation[axis] = wrap_degrees(e.rotation[axis] + degrees);
                    changed.push(id);
                }
                NodeKind::Group(g) => g.rotation[axis] = wrap_degrees(g.rotation[axis] + degrees),
            }
        }
    }
    sync_twins(doc, &changed);
}

/// Угол в диапазоне (−180, 180].
pub fn wrap_degrees(angle: f32) -> f32 {
    let mut a = angle % 360.0;
    if a <= -180.0 {
        a += 360.0;
    } else if a > 180.0 {
        a -= 360.0;
    }
    a
}

/// Мировая матрица → матрица в пространстве родителя узла (для гизмо).
pub fn to_parent_space(doc: &Document, id: NodeId) -> Mat4 {
    doc.parent_matrix(id).inverse()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cube(doc: &mut Document, from: Vec3, size: Vec3) -> NodeId {
        add_element(doc, "cube", Element::cuboid(from, size), None)
    }

    #[test]
    fn resize_from_a_face_keeps_the_opposite_face_fixed() {
        let mut doc = Document::new("t");
        let id = cube(&mut doc, Vec3::new(1.0, 2.0, 3.0), Vec3::new(4.0, 5.0, 6.0));
        for face in FaceDir::ALL {
            let before = doc.element(id).unwrap().clone();
            resize_face(&mut doc, id, face, 3.0);
            let after = doc.element(id).unwrap().clone();
            let axis = face.axis();
            assert_eq!(after.size[axis], before.size[axis] + 3.0);
            if face.positive() {
                assert_eq!(after.from[axis], before.from[axis], "{face:?}: the min side stays");
            } else {
                assert_eq!(after.to()[axis], before.to()[axis], "{face:?}: the max side stays");
            }
        }
        // Внутрь не дальше противоположной стороны.
        let to = doc.element(id).unwrap().to();
        resize_face(&mut doc, id, FaceDir::West, -100.0);
        let e = doc.element(id).unwrap();
        assert_eq!(e.size.x, 0.0);
        assert_eq!(e.from.x, to.x);
    }

    #[test]
    fn mirror_twin_follows_and_stays_mirrored() {
        let mut doc = Document::new("t");
        let arm = add_element(&mut doc, "arm_left", Element::cuboid(Vec3::new(4.0, 10.0, -2.0), Vec3::new(4.0, 12.0, 4.0)), None);
        let twin = mirror_x(&mut doc, &[arm])[0];
        assert_eq!(doc.node(twin).unwrap().name, "arm_right");
        let check = |doc: &Document| {
            let (a, b) = (doc.element(arm).unwrap(), doc.element(twin).unwrap());
            assert_eq!(b.from.x, -(a.from.x + a.size.x));
            assert_eq!((b.from.y, b.from.z, b.size), (a.from.y, a.from.z, a.size));
            assert_eq!(b.pivot, Vec3::new(-a.pivot.x, a.pivot.y, a.pivot.z));
            assert_eq!(b.rotation, Vec3::new(a.rotation.x, -a.rotation.y, -a.rotation.z));
            // Восточная сторона близнеца показывает западную картинку оригинала, отражённую.
            let mut west = a.face(FaceDir::West).uv;
            west.mirror_horizontal();
            assert_eq!(b.face(FaceDir::East).uv, west);
            // Мировые вершины — зеркальны.
            let ca = doc.element_corners(arm).unwrap();
            let cb = doc.element_corners(twin).unwrap();
            for p in ca {
                let m = Vec3::new(-p.x, p.y, p.z);
                assert!(cb.iter().any(|q| (*q - m).length() < 1e-4), "{p} has a mirror image");
            }
        };
        check(&doc);
        move_by(&mut doc, &[arm], Vec3::new(2.0, 1.0, -3.0));
        check(&doc);
        rotate_by(&mut doc, &[arm], 2, 22.5);
        rotate_by(&mut doc, &[arm], 1, -15.0);
        check(&doc);
        resize_face(&mut doc, arm, FaceDir::East, 2.0);
        check(&doc);
        // Правка близнеца ведёт оригинал.
        resize_face(&mut doc, twin, FaceDir::Up, 1.0);
        check(&doc);
        assert_eq!(doc.element(arm).unwrap().size.y, 13.0);
        unlink_twin(&mut doc, twin);
        move_by(&mut doc, &[arm], Vec3::X);
        assert_ne!(doc.element(twin).unwrap().from.x, -(doc.element(arm).unwrap().to().x));
    }

    #[test]
    fn reparent_reorders_and_refuses_cycles() {
        let mut doc = Document::new("t");
        let body = add_group(&mut doc, "body", Vec3::ZERO, None);
        let head = add_group(&mut doc, "head", Vec3::ZERO, Some(body));
        let a = cube(&mut doc, Vec3::ZERO, Vec3::ONE);
        let b = cube(&mut doc, Vec3::ZERO, Vec3::ONE);
        assert!(!reparent(&mut doc, body, Some(head), None), "a group cannot go into its own child");
        assert!(!reparent(&mut doc, a, Some(b), None), "only groups are parents");
        assert!(reparent(&mut doc, b, Some(body), Some(0)));
        let kids: Vec<NodeId> = doc.children(Some(body)).map(|n| n.id).collect();
        assert_eq!(kids, [b, head]);
        assert!(reparent(&mut doc, a, Some(body), Some(1)));
        let kids: Vec<NodeId> = doc.children(Some(body)).map(|n| n.id).collect();
        assert_eq!(kids, [b, a, head]);
        assert!(reparent(&mut doc, b, None, None));
        assert_eq!(doc.children(None).map(|n| n.id).collect::<Vec<_>>(), [body, b]);
    }

    #[test]
    fn duplicate_copies_subtree_and_painted_footprint() {
        let mut doc = Document::new("t");
        let group = add_group(&mut doc, "leg", Vec3::ZERO, None);
        let a = add_element(&mut doc, "shin", Element::cuboid(Vec3::ZERO, Vec3::new(2.0, 6.0, 2.0)), Some(group));
        let e = doc.element(a).unwrap().clone();
        let north = e.face(FaceDir::North).uv;
        doc.textures[0].image.set(north.x as i32, north.y as i32, [255, 0, 0, 255]);
        let copies = duplicate(&mut doc, &[group, a]);
        assert_eq!(copies.len(), 1, "the child is copied with its group, not twice");
        let copy_child = doc.children(Some(copies[0])).next().unwrap().id;
        let c = doc.element(copy_child).unwrap();
        assert_ne!(c.uv_origin, e.uv_origin, "the copy gets its own place on the atlas");
        let r = c.face(FaceDir::North).uv;
        assert_eq!(doc.textures[0].image.get(r.x as i32, r.y as i32), Some([255, 0, 0, 255]));
        assert_eq!(doc.node(copies[0]).unwrap().name, "leg2");
    }

    #[test]
    fn align_distribute_center_flip() {
        let mut doc = Document::new("t");
        let ids: Vec<NodeId> = (0..3)
            .map(|i| cube(&mut doc, Vec3::new(i as f32 * 10.0, i as f32, 0.0), Vec3::new(2.0, 2.0 + i as f32, 2.0)))
            .collect();
        align(&mut doc, &ids, 1, Align::Min);
        for id in &ids {
            assert_eq!(doc.element(*id).unwrap().from.y, 0.0);
        }
        move_by(&mut doc, &[ids[1]], Vec3::new(-7.0, 0.0, 0.0));
        distribute(&mut doc, &ids, 0);
        let xs: Vec<f32> = ids.iter().map(|id| doc.element(*id).unwrap().center().x).collect();
        assert!((xs[1] - (xs[0] + xs[2]) * 0.5).abs() < 1e-4, "{xs:?}");
        center(&mut doc, &ids);
        let (min, max) = doc.bounds(&ids).unwrap();
        assert!(((min + max) * 0.5).x.abs() < 1e-4 && ((min + max) * 0.5).z.abs() < 1e-4);
        let before = doc.element(ids[0]).unwrap().clone();
        flip(&mut doc, &ids, 0);
        flip(&mut doc, &ids, 0);
        assert_eq!(doc.element(ids[0]).unwrap(), &before, "flipping twice is the identity");
    }
}
