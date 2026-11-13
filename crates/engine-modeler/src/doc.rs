//! Документ модели: текстуры и дерево узлов — групп и элементов.
//!
//! Единица длины — «пиксель модели»: по умолчанию 16 единиц = 1 метр (так куб 16×16×16
//! с текстурой 16×16 — один блок). Ось Y вверх, правая тройка, «север» — −Z.
//!
//! Координаты элементов абсолютные (в пространстве модели до поворотов групп): кубоид
//! задан углом `from` и размером `size`. Поворот элемента — вокруг его `pivot`, поворот
//! группы — вокруг её `origin`; повороты групп применяются снаружи внутрь, как в любом
//! дереве объектов: `мир = G_родитель · G_группа · E_элемент · вершина`.

use glam::{EulerRot, Mat4, Quat, Vec3};
use serde::{Deserialize, Serialize};

use crate::image::{Image, Rgba};

/// Идентификатор узла, постоянный в пределах документа.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
pub struct NodeId(pub u32);

/// Сторона кубоида. Порядок — индекс в [`Element::faces`].
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum FaceDir {
    /// −Z
    North,
    /// +X
    East,
    /// +Z
    South,
    /// −X
    West,
    /// +Y
    Up,
    /// −Y
    Down,
}

impl FaceDir {
    pub const ALL: [FaceDir; 6] = [
        FaceDir::North,
        FaceDir::East,
        FaceDir::South,
        FaceDir::West,
        FaceDir::Up,
        FaceDir::Down,
    ];

    pub fn index(self) -> usize {
        self as usize
    }

    /// Нормаль стороны в пространстве элемента.
    pub fn normal(self) -> Vec3 {
        match self {
            FaceDir::North => Vec3::NEG_Z,
            FaceDir::East => Vec3::X,
            FaceDir::South => Vec3::Z,
            FaceDir::West => Vec3::NEG_X,
            FaceDir::Up => Vec3::Y,
            FaceDir::Down => Vec3::NEG_Y,
        }
    }

    /// Ось стороны: 0 — X, 1 — Y, 2 — Z.
    pub fn axis(self) -> usize {
        match self {
            FaceDir::East | FaceDir::West => 0,
            FaceDir::Up | FaceDir::Down => 1,
            FaceDir::North | FaceDir::South => 2,
        }
    }

    /// Смотрит ли сторона в положительную сторону своей оси.
    pub fn positive(self) -> bool {
        matches!(self, FaceDir::East | FaceDir::Up | FaceDir::South)
    }

    pub fn opposite(self) -> FaceDir {
        match self {
            FaceDir::North => FaceDir::South,
            FaceDir::South => FaceDir::North,
            FaceDir::East => FaceDir::West,
            FaceDir::West => FaceDir::East,
            FaceDir::Up => FaceDir::Down,
            FaceDir::Down => FaceDir::Up,
        }
    }

    pub fn name(self) -> &'static str {
        match self {
            FaceDir::North => "North",
            FaceDir::East => "East",
            FaceDir::South => "South",
            FaceDir::West => "West",
            FaceDir::Up => "Up",
            FaceDir::Down => "Down",
        }
    }
}

/// Прямоугольник стороны на текстуре, в текселях. Поворот и отражения — как картинка
/// ложится на сторону.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct UvRect {
    pub x: f32,
    pub y: f32,
    pub w: f32,
    pub h: f32,
    /// Поворот на четверти оборота по часовой стрелке (0…3).
    #[serde(default)]
    pub rotation: u8,
    #[serde(default)]
    pub flip_u: bool,
    #[serde(default)]
    pub flip_v: bool,
}

impl UvRect {
    pub fn new(x: f32, y: f32, w: f32, h: f32) -> Self {
        Self {
            x,
            y,
            w,
            h,
            rotation: 0,
            flip_u: false,
            flip_v: false,
        }
    }

    /// UV-координаты (в текселях) для углов стороны в порядке
    /// «левый верхний, правый верхний, правый нижний, левый нижний» — как сторону видно снаружи.
    pub fn corners(&self) -> [[f32; 2]; 4] {
        let (x0, y0, x1, y1) = (self.x, self.y, self.x + self.w, self.y + self.h);
        let mut list = [[x0, y0], [x1, y0], [x1, y1], [x0, y1]];
        if self.flip_u {
            list.swap(0, 1);
            list.swap(2, 3);
        }
        if self.flip_v {
            list.swap(0, 3);
            list.swap(1, 2);
        }
        let k = (self.rotation % 4) as usize;
        std::array::from_fn(|i| list[(i + 4 - k) % 4])
    }

    /// Отразить картинку на стороне по горизонтали (в пространстве стороны).
    pub fn mirror_horizontal(&mut self) {
        if self.rotation.is_multiple_of(2) {
            self.flip_u = !self.flip_u;
        } else {
            self.flip_v = !self.flip_v;
        }
    }

    /// Отразить картинку на стороне по вертикали.
    pub fn mirror_vertical(&mut self) {
        if self.rotation.is_multiple_of(2) {
            self.flip_v = !self.flip_v;
        } else {
            self.flip_u = !self.flip_u;
        }
    }

    /// Целочисленные границы `[x0, y0, x1, y1)` (для кисти и заливки стороны).
    pub fn texel_bounds(&self) -> [i32; 4] {
        let (x0, x1) = (self.x.min(self.x + self.w), self.x.max(self.x + self.w));
        let (y0, y1) = (self.y.min(self.y + self.h), self.y.max(self.y + self.h));
        [x0.floor() as i32, y0.floor() as i32, x1.ceil() as i32, y1.ceil() as i32]
    }
}

/// Сторона элемента: где её картинка на текстуре и как она выглядит.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct Face {
    /// Выключенная сторона не рисуется и не экспортируется.
    pub enabled: bool,
    /// Номер текстуры документа.
    pub texture: u32,
    pub uv: UvRect,
    /// Оттенок, на который умножается текстура (белый — без изменений).
    #[serde(default = "white")]
    pub tint: Rgba,
    /// Светится: не затеняется в окне, в экспорт идёт как emissive.
    #[serde(default)]
    pub emissive: bool,
}

fn white() -> Rgba {
    [255, 255, 255, 255]
}

impl Default for Face {
    fn default() -> Self {
        Self {
            enabled: true,
            texture: 0,
            uv: UvRect::new(0.0, 0.0, 0.0, 0.0),
            tint: white(),
            emissive: false,
        }
    }
}

/// Форма элемента. Всё вписано в коробку `from … from + size`.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub enum Shape {
    Cuboid,
    /// Горизонтальная плоскость на высоте `from.y` (две стороны: Up и Down).
    Plane,
    /// Цилиндр вдоль Y; стороны — `segments` граней, крышки — Up и Down.
    Cylinder { segments: u32 },
    /// Конус вдоль Y (вершина сверху).
    Cone { segments: u32 },
    /// Низкополигональный шар: `segments` по долготе, `rings` по широте.
    Sphere { segments: u32, rings: u32 },
}

impl Shape {
    pub fn name(&self) -> &'static str {
        match self {
            Shape::Cuboid => "Cuboid",
            Shape::Plane => "Plane",
            Shape::Cylinder { .. } => "Cylinder",
            Shape::Cone { .. } => "Cone",
            Shape::Sphere { .. } => "Sphere",
        }
    }

    /// Какие стороны есть у формы.
    pub fn faces(&self) -> &'static [FaceDir] {
        match self {
            Shape::Cuboid => &FaceDir::ALL,
            Shape::Plane => &[FaceDir::Up, FaceDir::Down],
            Shape::Cylinder { .. } | Shape::Cone { .. } => &[FaceDir::North, FaceDir::Up, FaceDir::Down],
            Shape::Sphere { .. } => &[FaceDir::North],
        }
    }

    /// Число сегментов (для форм, где оно есть).
    pub fn segments(&self) -> Option<u32> {
        match *self {
            Shape::Cylinder { segments } | Shape::Cone { segments } | Shape::Sphere { segments, .. } => Some(segments),
            _ => None,
        }
    }
}

/// Элемент — примитив модели.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Element {
    pub shape: Shape,
    /// Минимальный угол коробки.
    #[serde(with = "v3")]
    pub from: Vec3,
    #[serde(with = "v3")]
    pub size: Vec3,
    /// Точка, вокруг которой поворачивается элемент.
    #[serde(with = "v3")]
    pub pivot: Vec3,
    /// Углы Эйлера в градусах; применяются X, затем Y, затем Z.
    #[serde(with = "v3")]
    pub rotation: Vec3,
    /// Раздувание: коробка больше на столько с каждой стороны, текстура та же.
    #[serde(default)]
    pub inflate: f32,
    /// Стороны в порядке [`FaceDir::ALL`].
    pub faces: [Face; 6],
    /// UV раскладывается автоматически «развёрткой коробки» от угла `uv_origin`.
    #[serde(default)]
    pub box_uv: bool,
    #[serde(default)]
    pub uv_origin: [u32; 2],
    /// Отражённый по X близнец: правка одного повторяется на другом.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub twin: Option<NodeId>,
}

impl Element {
    /// Кубоид с углом `from` и размером `size`; поворот вокруг центра.
    pub fn cuboid(from: Vec3, size: Vec3) -> Self {
        Self::new(Shape::Cuboid, from, size)
    }

    pub fn new(shape: Shape, from: Vec3, size: Vec3) -> Self {
        let mut faces = [Face::default(); 6];
        for dir in FaceDir::ALL {
            faces[dir.index()].enabled = shape.faces().contains(&dir);
        }
        Self {
            shape,
            from,
            size,
            pivot: from + size * 0.5,
            rotation: Vec3::ZERO,
            inflate: 0.0,
            faces,
            box_uv: true,
            uv_origin: [0, 0],
            twin: None,
        }
    }

    pub fn face(&self, dir: FaceDir) -> &Face {
        &self.faces[dir.index()]
    }

    pub fn face_mut(&mut self, dir: FaceDir) -> &mut Face {
        &mut self.faces[dir.index()]
    }

    pub fn to(&self) -> Vec3 {
        self.from + self.size
    }

    pub fn center(&self) -> Vec3 {
        self.from + self.size * 0.5
    }

    /// Поворот элемента кватернионом.
    pub fn rotation_quat(&self) -> Quat {
        euler_quat(self.rotation)
    }

    /// Матрица элемента (поворот вокруг `pivot`) — без групп.
    pub fn local_matrix(&self) -> Mat4 {
        pivot_matrix(self.pivot, self.rotation)
    }

    /// Задать цвет всем сторонам.
    pub fn set_tint(&mut self, tint: Rgba) {
        for face in &mut self.faces {
            face.tint = tint;
        }
    }
}

/// Группа: именованный узел с точкой вращения — основа будущей анимации.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Group {
    #[serde(with = "v3")]
    pub origin: Vec3,
    #[serde(with = "v3")]
    pub rotation: Vec3,
}

impl Default for Group {
    fn default() -> Self {
        Self {
            origin: Vec3::ZERO,
            rotation: Vec3::ZERO,
        }
    }
}

// Элементов в модели десятки: выигрыш от `Box` не стоит неудобства в каждой операции.
#[allow(clippy::large_enum_variant)]
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub enum NodeKind {
    Group(Group),
    Element(Element),
}

/// Узел дерева (Outliner).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Node {
    pub id: NodeId,
    pub name: String,
    /// `None` — в корне. Порядок братьев — порядок в [`Document::nodes`].
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub parent: Option<NodeId>,
    /// Видимость в окне редактора (на экспорт не влияет).
    #[serde(default = "yes")]
    pub visible: bool,
    /// Запертый узел не выделяется щелчком в окне и не двигается гизмо.
    #[serde(default)]
    pub locked: bool,
    pub kind: NodeKind,
}

fn yes() -> bool {
    true
}

impl Node {
    pub fn element(&self) -> Option<&Element> {
        match &self.kind {
            NodeKind::Element(e) => Some(e),
            NodeKind::Group(_) => None,
        }
    }

    pub fn element_mut(&mut self) -> Option<&mut Element> {
        match &mut self.kind {
            NodeKind::Element(e) => Some(e),
            NodeKind::Group(_) => None,
        }
    }

    pub fn group(&self) -> Option<&Group> {
        match &self.kind {
            NodeKind::Group(g) => Some(g),
            NodeKind::Element(_) => None,
        }
    }

    pub fn group_mut(&mut self) -> Option<&mut Group> {
        match &mut self.kind {
            NodeKind::Group(g) => Some(g),
            NodeKind::Element(_) => None,
        }
    }

    pub fn is_group(&self) -> bool {
        matches!(self.kind, NodeKind::Group(_))
    }
}

/// Текстура документа.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Texture {
    pub name: String,
    /// В файле — PNG в base64.
    pub image: Image,
    /// Файл PNG, из которого текстура импортирована (перечитывается, когда он меняется).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source: Option<String>,
}

/// Чужая модель (`.obj`, `.glb`) для обрисовки: только показывается, не редактируется.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ReferenceMesh {
    pub name: String,
    /// Вершины в единицах модели.
    pub positions: Vec<[f32; 3]>,
    pub indices: Vec<u32>,
    #[serde(default = "yes")]
    pub visible: bool,
}

/// Модель целиком.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Document {
    pub name: String,
    /// Сколько единиц модели в метре (по умолчанию 16).
    pub units_per_metre: f32,
    /// Текселей на единицу модели при автоматической развёртке (по умолчанию 1).
    pub texel_density: f32,
    pub textures: Vec<Texture>,
    pub nodes: Vec<Node>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub references: Vec<ReferenceMesh>,
    /// Следующий свободный идентификатор узла.
    pub next_id: u32,
}

/// Размер атласа нового документа.
pub const DEFAULT_TEXTURE_SIZE: u32 = 64;
/// Единиц модели в метре по умолчанию.
pub const DEFAULT_UNITS_PER_METRE: f32 = 16.0;

impl Default for Document {
    fn default() -> Self {
        Self::new("model")
    }
}

impl Document {
    /// Пустая модель с одной прозрачной текстурой 64×64.
    pub fn new(name: &str) -> Self {
        Self {
            name: name.to_string(),
            units_per_metre: DEFAULT_UNITS_PER_METRE,
            texel_density: 1.0,
            textures: vec![Texture {
                name: "atlas".to_string(),
                image: Image::new(DEFAULT_TEXTURE_SIZE, DEFAULT_TEXTURE_SIZE, crate::image::CLEAR),
                source: None,
            }],
            nodes: Vec::new(),
            references: Vec::new(),
            next_id: 1,
        }
    }

    pub fn node(&self, id: NodeId) -> Option<&Node> {
        self.nodes.iter().find(|n| n.id == id)
    }

    pub fn node_mut(&mut self, id: NodeId) -> Option<&mut Node> {
        self.nodes.iter_mut().find(|n| n.id == id)
    }

    pub fn index_of(&self, id: NodeId) -> Option<usize> {
        self.nodes.iter().position(|n| n.id == id)
    }

    pub fn element(&self, id: NodeId) -> Option<&Element> {
        self.node(id).and_then(Node::element)
    }

    pub fn element_mut(&mut self, id: NodeId) -> Option<&mut Element> {
        self.node_mut(id).and_then(Node::element_mut)
    }

    pub fn find_by_name(&self, name: &str) -> Option<NodeId> {
        self.nodes.iter().find(|n| n.name == name).map(|n| n.id)
    }

    /// Дети узла (или корня при `None`) по порядку.
    pub fn children(&self, parent: Option<NodeId>) -> impl Iterator<Item = &Node> {
        self.nodes.iter().filter(move |n| n.parent == parent)
    }

    /// Все потомки узла (без него самого).
    pub fn descendants(&self, id: NodeId) -> Vec<NodeId> {
        let mut out = Vec::new();
        let mut stack = vec![id];
        while let Some(current) = stack.pop() {
            for child in self.children(Some(current)) {
                out.push(child.id);
                stack.push(child.id);
            }
        }
        out
    }

    /// Является ли `ancestor` предком `id` (или им самим).
    pub fn is_ancestor(&self, ancestor: NodeId, id: NodeId) -> bool {
        let mut current = Some(id);
        while let Some(node) = current {
            if node == ancestor {
                return true;
            }
            current = self.node(node).and_then(|n| n.parent);
        }
        false
    }

    /// Узел и все его предки видимы.
    pub fn is_visible(&self, id: NodeId) -> bool {
        let mut current = Some(id);
        while let Some(node) = current.and_then(|id| self.node(id)) {
            if !node.visible {
                return false;
            }
            current = node.parent;
        }
        true
    }

    /// Узел или кто-то из предков заперт.
    pub fn is_locked(&self, id: NodeId) -> bool {
        let mut current = Some(id);
        while let Some(node) = current.and_then(|id| self.node(id)) {
            if node.locked {
                return true;
            }
            current = node.parent;
        }
        false
    }

    /// Матрица групп над узлом (без собственной матрицы элемента).
    pub fn parent_matrix(&self, id: NodeId) -> Mat4 {
        let mut chain = Vec::new();
        let mut current = self.node(id).and_then(|n| n.parent);
        while let Some(parent) = current.and_then(|p| self.node(p)) {
            if let Some(group) = parent.group() {
                chain.push(pivot_matrix(group.origin, group.rotation));
            }
            current = parent.parent;
        }
        chain.iter().rev().fold(Mat4::IDENTITY, |acc, m| acc * *m)
    }

    /// Полная матрица узла: группы и (для элемента) его собственный поворот.
    pub fn world_matrix(&self, id: NodeId) -> Mat4 {
        let parent = self.parent_matrix(id);
        match self.node(id).map(|n| &n.kind) {
            Some(NodeKind::Element(e)) => parent * e.local_matrix(),
            Some(NodeKind::Group(g)) => parent * pivot_matrix(g.origin, g.rotation),
            None => Mat4::IDENTITY,
        }
    }

    /// Все элементы по порядку дерева: (id, элемент).
    pub fn elements(&self) -> impl Iterator<Item = (NodeId, &Element)> {
        self.nodes.iter().filter_map(|n| n.element().map(|e| (n.id, e)))
    }

    /// Новый идентификатор.
    pub fn allocate_id(&mut self) -> NodeId {
        let id = NodeId(self.next_id);
        self.next_id += 1;
        id
    }

    /// Имя, которого ещё нет среди узлов: `cube`, `cube2`, `cube3`…
    pub fn unique_name(&self, base: &str) -> String {
        if !base.is_empty() && !self.nodes.iter().any(|n| n.name == base) {
            return base.to_string();
        }
        let base = base.trim_end_matches(|c: char| c.is_ascii_digit());
        let base = if base.is_empty() { "node" } else { base };
        if !self.nodes.iter().any(|n| n.name == base) {
            return base.to_string();
        }
        (2..)
            .map(|i| format!("{base}{i}"))
            .find(|name| !self.nodes.iter().any(|n| n.name == *name))
            .expect("an unused name always exists")
    }

    /// Мировые углы коробки элемента (8 точек, с раздуванием).
    pub fn element_corners(&self, id: NodeId) -> Option<[Vec3; 8]> {
        let element = self.element(id)?;
        let matrix = self.world_matrix(id);
        let a = element.from - Vec3::splat(element.inflate);
        let b = element.to() + Vec3::splat(element.inflate);
        Some(std::array::from_fn(|i| {
            let p = Vec3::new(
                if i & 1 == 0 { a.x } else { b.x },
                if i & 2 == 0 { a.y } else { b.y },
                if i & 4 == 0 { a.z } else { b.z },
            );
            matrix.transform_point3(p)
        }))
    }

    /// Мировые границы набора узлов (группа — все её элементы).
    pub fn bounds(&self, ids: &[NodeId]) -> Option<(Vec3, Vec3)> {
        let mut min = Vec3::splat(f32::INFINITY);
        let mut max = Vec3::splat(f32::NEG_INFINITY);
        let mut any = false;
        for id in ids {
            let mut all = vec![*id];
            all.extend(self.descendants(*id));
            for node in all {
                if let Some(corners) = self.element_corners(node) {
                    for c in corners {
                        min = min.min(c);
                        max = max.max(c);
                        any = true;
                    }
                }
            }
        }
        any.then_some((min, max))
    }

    /// Границы всей модели.
    pub fn model_bounds(&self) -> Option<(Vec3, Vec3)> {
        let roots: Vec<NodeId> = self.children(None).map(|n| n.id).collect();
        self.bounds(&roots)
    }

    pub fn element_count(&self) -> usize {
        self.elements().count()
    }
}

/// Кватернион из углов Эйлера в градусах (сначала X, потом Y, потом Z).
pub fn euler_quat(degrees: Vec3) -> Quat {
    Quat::from_euler(
        EulerRot::ZYX,
        degrees.z.to_radians(),
        degrees.y.to_radians(),
        degrees.x.to_radians(),
    )
}

/// Поворот `degrees` вокруг точки `pivot`.
pub fn pivot_matrix(pivot: Vec3, degrees: Vec3) -> Mat4 {
    if degrees == Vec3::ZERO {
        return Mat4::IDENTITY;
    }
    Mat4::from_translation(pivot) * Mat4::from_quat(euler_quat(degrees)) * Mat4::from_translation(-pivot)
}

/// `Vec3` в файле — кортеж из трёх чисел.
pub(crate) mod v3 {
    use glam::Vec3;
    use serde::{Deserialize, Deserializer, Serialize, Serializer};

    pub fn serialize<S: Serializer>(v: &Vec3, s: S) -> Result<S::Ok, S::Error> {
        (v.x, v.y, v.z).serialize(s)
    }

    pub fn deserialize<'de, D: Deserializer<'de>>(d: D) -> Result<Vec3, D::Error> {
        let (x, y, z) = <(f32, f32, f32)>::deserialize(d)?;
        Ok(Vec3::new(x, y, z))
    }
}

/// Картинка в файле — PNG в base64.
impl Serialize for Image {
    fn serialize<S: serde::Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        crate::image::base64_encode(&self.to_png()).serialize(s)
    }
}

impl<'de> Deserialize<'de> for Image {
    fn deserialize<D: serde::Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        let text = String::deserialize(d)?;
        let bytes = crate::image::base64_decode(&text).map_err(serde::de::Error::custom)?;
        Image::from_png(&bytes).map_err(serde::de::Error::custom)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn uv_corners_rotate_and_flip() {
        let rect = UvRect::new(0.0, 0.0, 2.0, 1.0);
        assert_eq!(rect.corners(), [[0.0, 0.0], [2.0, 0.0], [2.0, 1.0], [0.0, 1.0]]);
        let mut flipped = rect;
        flipped.flip_u = true;
        assert_eq!(flipped.corners()[0], [2.0, 0.0]);
        let mut turned = rect;
        turned.rotation = 1;
        // Поворот по часовой: в левом верхнем углу стороны — бывший левый нижний угол картинки.
        assert_eq!(turned.corners()[0], [0.0, 1.0]);
        // Отражение по горизонтали в пространстве стороны для любого поворота.
        for rotation in 0..4 {
            let mut r = rect;
            r.rotation = rotation;
            let before = r.corners();
            r.mirror_horizontal();
            let after = r.corners();
            assert_eq!([after[0], after[1], after[2], after[3]], [before[1], before[0], before[3], before[2]]);
        }
    }

    #[test]
    fn group_transforms_compose() {
        let mut doc = Document::new("t");
        let outer = NodeId(1);
        let inner = NodeId(2);
        let cube = NodeId(3);
        doc.nodes.push(Node {
            id: outer,
            name: "outer".into(),
            parent: None,
            visible: true,
            locked: false,
            kind: NodeKind::Group(Group {
                origin: Vec3::ZERO,
                rotation: Vec3::new(0.0, 90.0, 0.0),
            }),
        });
        doc.nodes.push(Node {
            id: inner,
            name: "inner".into(),
            parent: Some(outer),
            visible: true,
            locked: false,
            kind: NodeKind::Group(Group {
                origin: Vec3::new(4.0, 0.0, 0.0),
                rotation: Vec3::new(0.0, 0.0, 90.0),
            }),
        });
        doc.nodes.push(Node {
            id: cube,
            name: "cube".into(),
            parent: Some(inner),
            visible: true,
            locked: false,
            kind: NodeKind::Element(Element::cuboid(Vec3::ZERO, Vec3::ONE)),
        });
        doc.next_id = 4;
        // Точка (8, 0, 0): внутренняя группа поворачивает её на 90° по Z вокруг (4, 0, 0) →
        // (4, 4, 0); внешняя — на 90° по Y вокруг нуля → (0, 4, −4).
        let p = doc.world_matrix(cube).transform_point3(Vec3::new(8.0, 0.0, 0.0));
        assert!((p - Vec3::new(0.0, 4.0, -4.0)).length() < 1e-4, "{p}");
        let manual = pivot_matrix(Vec3::ZERO, Vec3::new(0.0, 90.0, 0.0))
            * pivot_matrix(Vec3::new(4.0, 0.0, 0.0), Vec3::new(0.0, 0.0, 90.0));
        assert!(doc.world_matrix(cube).abs_diff_eq(manual, 1e-5));
        assert!(doc.is_ancestor(outer, cube) && !doc.is_ancestor(cube, outer));
        assert_eq!(doc.descendants(outer).len(), 2);
    }
}
