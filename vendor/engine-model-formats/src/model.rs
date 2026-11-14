//! Редактируемая сцена модели: группы, кубы, свободные меши, текстуры — общая часть
//! `.bbmodel`, моделей Minecraft и кубоидов `.vox`.

use crate::image::RgbaImage;
use crate::math::V3;
use crate::Format;

/// Сторона куба. Порядок — индекс в [`Cube::faces`] (как `FaceDir` Piska Modeler).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Side {
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

impl Side {
    pub const ALL: [Side; 6] = [Side::North, Side::East, Side::South, Side::West, Side::Up, Side::Down];

    pub fn index(self) -> usize {
        self as usize
    }

    /// Имя стороны в JSON Blockbench и Minecraft.
    pub fn key(self) -> &'static str {
        match self {
            Side::North => "north",
            Side::East => "east",
            Side::South => "south",
            Side::West => "west",
            Side::Up => "up",
            Side::Down => "down",
        }
    }

    pub fn normal(self) -> V3 {
        match self {
            Side::North => [0.0, 0.0, -1.0],
            Side::East => [1.0, 0.0, 0.0],
            Side::South => [0.0, 0.0, 1.0],
            Side::West => [-1.0, 0.0, 0.0],
            Side::Up => [0.0, 1.0, 0.0],
            Side::Down => [0.0, -1.0, 0.0],
        }
    }

    /// Углы стороны коробки `a…b` в порядке «левый верхний, правый верхний, правый
    /// нижний, левый нижний», как сторону видно снаружи (по часовой стрелке) — соглашение
    /// Minecraft / Blockbench: бока «стоя»; верх — сверху, «север» (−Z) у верхнего края
    /// картинки; низ — снизу, «юг» (+Z) у верхнего края.
    pub fn corners(self, a: V3, b: V3) -> [V3; 4] {
        match self {
            Side::North => [[b[0], b[1], a[2]], [a[0], b[1], a[2]], [a[0], a[1], a[2]], [b[0], a[1], a[2]]],
            Side::South => [[a[0], b[1], b[2]], [b[0], b[1], b[2]], [b[0], a[1], b[2]], [a[0], a[1], b[2]]],
            Side::East => [[b[0], b[1], b[2]], [b[0], b[1], a[2]], [b[0], a[1], a[2]], [b[0], a[1], b[2]]],
            Side::West => [[a[0], b[1], a[2]], [a[0], b[1], b[2]], [a[0], a[1], b[2]], [a[0], a[1], a[2]]],
            Side::Up => [[a[0], b[1], a[2]], [b[0], b[1], a[2]], [b[0], b[1], b[2]], [a[0], b[1], b[2]]],
            Side::Down => [[a[0], a[1], b[2]], [b[0], a[1], b[2]], [b[0], a[1], a[2]], [a[0], a[1], a[2]]],
        }
    }
}

/// Что пропущено и на что стоит обратить внимание при импорте.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Report {
    /// Потери и замены: «texture `a.png` not found», «animated texture: frame 0 is used».
    pub warnings: Vec<String>,
    /// Части файла, которые импорт сознательно не берёт: анимации, `display`, локаторы.
    pub ignored: Vec<String>,
}

impl Report {
    const MAX: usize = 64;

    pub fn warn(&mut self, message: impl Into<String>) {
        Self::push(&mut self.warnings, message.into());
    }

    pub fn ignore(&mut self, message: impl Into<String>) {
        Self::push(&mut self.ignored, message.into());
    }

    fn push(list: &mut Vec<String>, message: String) {
        if list.len() < Self::MAX && !list.contains(&message) {
            list.push(message);
        } else if list.len() == Self::MAX {
            list.push("… (more messages are not shown)".to_string());
        }
    }

    pub fn is_empty(&self) -> bool {
        self.warnings.is_empty() && self.ignored.is_empty()
    }

    pub fn extend(&mut self, other: Report) {
        for w in other.warnings {
            self.warn(w);
        }
        for i in other.ignored {
            self.ignore(i);
        }
    }
}

/// Текстура модели.
#[derive(Clone, Debug, PartialEq)]
pub struct Texture {
    pub name: String,
    /// Картинка; `None` — файл не найден (импорт подставит заметную «шахматку»).
    pub image: Option<RgbaImage>,
    /// Размер пространства UV текстуры: UV граней задаются в этих единицах
    /// (`resolution` / `uv_width` Blockbench, 16×16 у Minecraft).
    pub uv_size: [f32; 2],
    /// Путь внешнего файла относительно модели (если картинка не встроена).
    pub path: Option<String>,
}

/// Прямоугольник стороны «коробочной» развёртки: угол развёртки на текстуре.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct BoxUv {
    pub offset: [f32; 2],
    pub mirror: bool,
}

/// Сторона куба: прямоугольник UV `[u1, v1, u2, v2]` в единицах [`Texture::uv_size`]
/// (u2 < u1 — отражение по горизонтали), поворот картинки по часовой стрелке
/// четвертями оборота, номер текстуры (`None` — сторона без текстуры, белая).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct CubeFace {
    pub uv: [f32; 4],
    pub rotation: u8,
    pub texture: Option<usize>,
}

impl CubeFace {
    /// UV углов стороны (в единицах текстуры) в порядке [`Side::corners`]: сначала
    /// отражения (из порядка чисел в `uv`), затем поворот по часовой стрелке — как в
    /// Minecraft (`BlockFaceUV`) и Blockbench.
    pub fn corner_uvs(&self) -> [[f32; 2]; 4] {
        let [u1, v1, u2, v2] = self.uv;
        let list = [[u1, v1], [u2, v1], [u2, v2], [u1, v2]];
        let k = (self.rotation % 4) as usize;
        std::array::from_fn(|i| list[(i + 4 - k) % 4])
    }
}

/// Куб (кубоид Blockbench / элемент Minecraft).
#[derive(Clone, Debug, PartialEq)]
pub struct Cube {
    pub name: String,
    pub uuid: String,
    pub from: V3,
    pub to: V3,
    /// Точка вращения.
    pub origin: V3,
    /// Углы Эйлера в градусах (X, потом Y, потом Z).
    pub rotation: V3,
    /// Раздувание: коробка больше на столько с каждой стороны, UV те же.
    pub inflate: f32,
    /// Minecraft `rescale`: грани растягиваются на 1 / cos(угла) поперёк оси поворота.
    pub rescale: bool,
    /// Стороны в порядке [`Side::ALL`]; `None` — стороны нет.
    pub faces: [Option<CubeFace>; 6],
    /// UV разложены «коробкой» от этого угла (стороны уже содержат итоговые UV).
    pub box_uv: Option<BoxUv>,
    /// Видимость в редакторе (на экспорт не влияет).
    pub visible: bool,
    pub locked: bool,
}

/// Группа (кость): точка вращения, поворот, дети.
#[derive(Clone, Debug, PartialEq)]
pub struct Group {
    pub name: String,
    pub uuid: String,
    pub origin: V3,
    pub rotation: V3,
    pub visible: bool,
    pub children: Vec<Node>,
}

/// Свободный меш Blockbench: вершины в пространстве меша (до `origin` и `rotation`).
#[derive(Clone, Debug, PartialEq)]
pub struct MeshElement {
    pub name: String,
    pub uuid: String,
    pub origin: V3,
    pub rotation: V3,
    pub vertices: Vec<V3>,
    pub faces: Vec<MeshFace>,
    pub visible: bool,
}

/// Многоугольник меша: номера вершин (против часовой стрелки снаружи) и UV углов в
/// единицах текстуры.
#[derive(Clone, Debug, PartialEq)]
pub struct MeshFace {
    pub vertices: Vec<u32>,
    pub uvs: Vec<[f32; 2]>,
    pub texture: Option<usize>,
}

// Кубов в модели много, групп мало: `Box` у кубов стоил бы аллокации на каждый.
#[allow(clippy::large_enum_variant)]
#[derive(Clone, Debug, PartialEq)]
pub enum Node {
    Group(Group),
    Cube(Cube),
    Mesh(MeshElement),
}

impl Node {
    pub fn name(&self) -> &str {
        match self {
            Node::Group(g) => &g.name,
            Node::Cube(c) => &c.name,
            Node::Mesh(m) => &m.name,
        }
    }
}

/// Модель целиком.
#[derive(Clone, Debug, PartialEq)]
pub struct Model {
    pub name: String,
    pub format: Format,
    pub textures: Vec<Texture>,
    /// Узлы верхнего уровня по порядку.
    pub roots: Vec<Node>,
    /// Сдвиг координат файла, чтобы модель стояла в начале координат по центру:
    /// у моделей блоков (0…16) — (−8, 0, −8), у остальных — ноль. Его применяют и
    /// движок, и редактор моделей.
    pub offset: V3,
    pub report: Report,
}

impl Model {
    /// Все кубы по порядку дерева.
    pub fn cubes(&self) -> Vec<&Cube> {
        fn walk<'a>(nodes: &'a [Node], out: &mut Vec<&'a Cube>) {
            for node in nodes {
                match node {
                    Node::Group(g) => walk(&g.children, out),
                    Node::Cube(c) => out.push(c),
                    Node::Mesh(_) => {}
                }
            }
        }
        let mut out = Vec::new();
        walk(&self.roots, &mut out);
        out
    }

    /// Найти узел по имени (в глубину).
    pub fn find(&self, name: &str) -> Option<&Node> {
        fn walk<'a>(nodes: &'a [Node], name: &str) -> Option<&'a Node> {
            for node in nodes {
                if node.name() == name {
                    return Some(node);
                }
                if let Node::Group(g) = node {
                    if let Some(found) = walk(&g.children, name) {
                        return Some(found);
                    }
                }
            }
            None
        }
        walk(&self.roots, name)
    }
}

/// Размер куба для «коробочной» развёртки: целые доли, как у Blockbench (`floor`).
pub fn box_uv_size(from: V3, to: V3) -> V3 {
    std::array::from_fn(|i| ((to[i] - from[i]).abs() + 1e-4).floor())
}

/// Прямоугольники сторон «коробочной» развёртки Blockbench / Minecraft для куба
/// размером `size` (ширина X, высота Y, глубина Z) от угла `offset`:
///
/// ```text
///        d      w      w
///      ┌────┬──────┬──────┐
///    d │    │  up  │ down │      up — повёрнут на 180°, down — отражён по горизонтали
///      ├────┼──────┼────┬─┴────┐
///    h │east│north │west│south │
///      └────┴──────┴────┴──────┘
/// ```
///
/// `mirror` (`mirror_uv`) отражает каждый прямоугольник по горизонтали и меняет местами
/// east и west. Результат — `[u1, v1, u2, v2]` в порядке [`Side::ALL`].
pub fn box_uv_layout(size: V3, offset: [f32; 2], mirror: bool) -> [[f32; 4]; 6] {
    let [w, h, d] = size;
    // (сторона, угол от offset, размер со знаком) — знак размера задаёт отражение.
    let mut list: [(Side, [f32; 2], [f32; 2]); 6] = [
        (Side::East, [0.0, d], [d, h]),
        (Side::West, [d + w, d], [d, h]),
        (Side::Up, [d + w, d], [-w, -d]),
        (Side::Down, [d + 2.0 * w, 0.0], [-w, d]),
        (Side::South, [2.0 * d + w, d], [w, h]),
        (Side::North, [d, d], [w, h]),
    ];
    if mirror {
        for entry in &mut list {
            entry.1[0] += entry.2[0];
            entry.2[0] = -entry.2[0];
        }
        let (east, west) = ((list[0].1, list[0].2), (list[1].1, list[1].2));
        (list[0].1, list[0].2) = west;
        (list[1].1, list[1].2) = east;
    }
    let mut out = [[0.0; 4]; 6];
    for (side, from, size) in list {
        let u1 = offset[0] + from[0];
        let v1 = offset[1] + from[1];
        out[side.index()] = [u1, v1, u1 + size[0], v1 + size[1]];
    }
    out
}

/// Имя модели по имени файла (без каталога и расширений).
pub fn stem(name: &str) -> &str {
    let file = name.rsplit(['/', '\\']).next().unwrap_or(name);
    let file = file.strip_suffix(".json").map(|f| f.strip_suffix(".model").unwrap_or(f)).unwrap_or(file);
    match file.rsplit_once('.') {
        Some((stem, _)) if !stem.is_empty() => stem,
        _ => file,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn corners_are_clockwise_seen_from_outside() {
        let (a, b) = ([0.0, 0.0, 0.0], [1.0, 2.0, 3.0]);
        for side in Side::ALL {
            let c = side.corners(a, b);
            // Обход TL → BL → BR (против часовой стрелки снаружи) даёт внешнюю нормаль.
            let n = crate::math::cross(crate::math::sub(c[3], c[0]), crate::math::sub(c[2], c[0]));
            let n = crate::math::normalize(n).unwrap();
            assert!(crate::math::dot(n, side.normal()) > 0.99, "{side:?} {n:?}");
        }
    }

    #[test]
    fn box_layout_matches_the_minecraft_template() {
        // Голова игрока: 8×8×8 от (0, 0).
        let uv = box_uv_layout([8.0, 8.0, 8.0], [0.0, 0.0], false);
        assert_eq!(uv[Side::Up.index()], [16.0, 8.0, 8.0, 0.0]);
        assert_eq!(uv[Side::Down.index()], [24.0, 0.0, 16.0, 8.0]);
        assert_eq!(uv[Side::East.index()], [0.0, 8.0, 8.0, 16.0]);
        assert_eq!(uv[Side::North.index()], [8.0, 8.0, 16.0, 16.0]);
        assert_eq!(uv[Side::West.index()], [16.0, 8.0, 24.0, 16.0]);
        assert_eq!(uv[Side::South.index()], [24.0, 8.0, 32.0, 16.0]);
        let mirrored = box_uv_layout([8.0, 8.0, 8.0], [0.0, 0.0], true);
        assert_eq!(mirrored[Side::East.index()], [24.0, 8.0, 16.0, 16.0], "east takes west's place, flipped");
        assert_eq!(mirrored[Side::North.index()], [16.0, 8.0, 8.0, 16.0]);
        assert_eq!(stem("models/Stone.model.json"), "Stone");
        assert_eq!(stem("a\\b.bbmodel"), "b");
    }
}
