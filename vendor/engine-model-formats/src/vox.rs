//! MagicaVoxel `.vox`: блоки в стиле RIFF (`SIZE` / `XYZI` / `RGBA`, граф сцены `nTRN` /
//! `nGRP` / `nSHP`, слои `LAYR`), палитра по умолчанию, если `RGBA` нет.
//!
//! Оси: у MagicaVoxel Z вверх; ячейка (x, y, z) становится (x, z, −y − 1) с Y вверх (поворот
//! на −90° вокруг X, правая тройка сохраняется). Модель ставится на «землю» (наименьший Y = 0)
//! и по центру по X и Z. Единица — одна ячейка (1/16 м по умолчанию).
//!
//! Для движка ячейки сводятся жадной сеткой ([`to_mesh`]: соседние грани одного цвета в
//! одной плоскости — один прямоугольник, внутренние грани не строятся), цвет — тексель
//! палитры 16×16. Для редактора моделей — кубоиды ([`to_model`]: жадное объединение
//! ячеек одного цвета в коробки, закрытые соседями стороны выключены).

use std::collections::HashMap;

use crate::image::RgbaImage;
use crate::mesh::{finish, Builder, Imported, Options, Sheet, Slot};
use crate::model::{Cube, CubeFace, Model, Node, Report, Side, Texture};
use crate::{limits, Error, Format};

/// Воксели всех видимых моделей файла в общих координатах (Y вверх).
#[derive(Clone, Debug, PartialEq)]
pub struct VoxScene {
    /// Ячейка (минимальный угол) и номер цвета 1…255.
    pub voxels: Vec<([i32; 3], u8)>,
    /// Цвет по номеру (0 не используется).
    pub palette: [[u8; 4]; 256],
    pub report: Report,
}

/// Палитра MagicaVoxel по умолчанию (когда в файле нет `RGBA`): куб цветов 6×6×6 с шагом
/// 0x33 от белого к чёрному (без чёрного), затем по десять оттенков синего, зелёного,
/// красного и серого.
pub fn default_palette() -> [[u8; 4]; 256] {
    let mut palette = [[0u8; 4]; 256];
    let steps = [0xff, 0xcc, 0x99, 0x66, 0x33, 0x00];
    let mut i = 1;
    for r in steps {
        for g in steps {
            for b in steps {
                if (r, g, b) != (0, 0, 0) {
                    palette[i] = [r, g, b, 255];
                    i += 1;
                }
            }
        }
    }
    let ramp = [0xee, 0xdd, 0xbb, 0xaa, 0x88, 0x77, 0x55, 0x44, 0x22, 0x11];
    for channel in [[0, 0, 1], [0, 1, 0], [1, 0, 0], [1, 1, 1]] {
        for v in ramp {
            palette[i] = [v * channel[0], v * channel[1], v * channel[2], 255];
            i += 1;
        }
    }
    palette
}

struct Reader<'a> {
    data: &'a [u8],
    at: usize,
    what: &'static str,
}

impl<'a> Reader<'a> {
    fn new(data: &'a [u8], what: &'static str) -> Self {
        Self { data, at: 0, what }
    }

    fn bytes(&mut self, n: usize) -> Result<&'a [u8], Error> {
        let end = self.at.checked_add(n).filter(|e| *e <= self.data.len());
        let end = end.ok_or_else(|| Error(format!("the .vox file is truncated (in a `{}` chunk)", self.what)))?;
        let out = &self.data[self.at..end];
        self.at = end;
        Ok(out)
    }

    fn i32(&mut self) -> Result<i32, Error> {
        let b = self.bytes(4)?;
        Ok(i32::from_le_bytes([b[0], b[1], b[2], b[3]]))
    }

    fn count(&mut self, item: usize, what: &str) -> Result<usize, Error> {
        let n = self.i32()?;
        if n < 0 || (n as usize).saturating_mul(item) > self.data.len() - self.at {
            return Err(Error(format!("the .vox file is damaged: {what} count {n} does not fit the `{}` chunk", self.what)));
        }
        Ok(n as usize)
    }

    fn string(&mut self) -> Result<String, Error> {
        let n = self.count(1, "string length")?;
        Ok(String::from_utf8_lossy(self.bytes(n)?).into_owned())
    }

    fn dict(&mut self) -> Result<HashMap<String, String>, Error> {
        let n = self.count(8, "dictionary")?;
        let mut out = HashMap::new();
        for _ in 0..n {
            let key = self.string()?;
            let value = self.string()?;
            out.insert(key, value);
        }
        Ok(out)
    }
}

/// Узел графа сцены.
enum SceneNode {
    Transform { child: i32, rotation: [[i32; 3]; 3], translation: [i32; 3], hidden: bool, layer: i32 },
    Group { children: Vec<i32> },
    Shape { models: Vec<usize> },
}

struct VoxModel {
    size: [i32; 3],
    voxels: Vec<[u8; 4]>,
}

/// Прочитать `.vox`.
pub fn read(bytes: &[u8]) -> Result<VoxScene, Error> {
    let mut r = Reader::new(bytes, "header");
    if r.bytes(4).ok() != Some(b"VOX ") {
        return Err(Error("not a MagicaVoxel file (it does not start with `VOX `)".to_string()));
    }
    let _version = r.i32()?;
    let mut report = Report::default();
    let id = r.bytes(4)?;
    if id != b"MAIN" {
        return Err(Error("the .vox file is damaged: the `MAIN` chunk is missing".to_string()));
    }
    let content = r.count(1, "MAIN content")?;
    let children = r.count(1, "MAIN children")?;
    r.bytes(content)?;
    let main = r.bytes(children)?;

    let mut models: Vec<VoxModel> = Vec::new();
    let mut pending_size: Option<[i32; 3]> = None;
    let mut palette: Option<[[u8; 4]; 256]> = None;
    let mut nodes: HashMap<i32, SceneNode> = HashMap::new();
    let mut hidden_layers: Vec<i32> = Vec::new();
    let mut total_voxels = 0usize;
    let mut chunks = Reader::new(main, "MAIN");
    while chunks.at < main.len() {
        let id: [u8; 4] = chunks.bytes(4)?.try_into().expect("4 bytes");
        let name: &'static str = match &id {
            b"SIZE" => "SIZE",
            b"XYZI" => "XYZI",
            b"RGBA" => "RGBA",
            b"nTRN" => "nTRN",
            b"nGRP" => "nGRP",
            b"nSHP" => "nSHP",
            b"LAYR" => "LAYR",
            _ => "unknown",
        };
        let content_len = chunks.count(1, "chunk content")?;
        let children_len = chunks.count(1, "chunk children")?;
        let content = chunks.bytes(content_len)?;
        chunks.bytes(children_len)?;
        let mut c = Reader::new(content, name);
        match &id {
            b"SIZE" => {
                let size = [c.i32()?, c.i32()?, c.i32()?];
                if size.iter().any(|s| !(1..=2048).contains(s)) {
                    return Err(Error(format!("model size {size:?} is out of range (1..2048 per axis)")));
                }
                pending_size = Some(size);
            }
            b"XYZI" => {
                let size = pending_size.take().ok_or_else(|| Error("the .vox file is damaged: `XYZI` without `SIZE`".to_string()))?;
                let count = c.count(4, "voxel")?;
                total_voxels += count;
                if total_voxels > limits::MAX_VOXEL_CELLS as usize {
                    return Err(Error(format!("the file has more than {} voxels", limits::MAX_VOXEL_CELLS)));
                }
                let raw = c.bytes(count * 4)?;
                let mut voxels = Vec::with_capacity(count);
                let mut outside = 0;
                for v in raw.chunks_exact(4) {
                    if (v[0] as i32) < size[0] && (v[1] as i32) < size[1] && (v[2] as i32) < size[2] && v[3] != 0 {
                        voxels.push([v[0], v[1], v[2], v[3]]);
                    } else {
                        outside += 1;
                    }
                }
                if outside > 0 {
                    report.warn(format!("{outside} voxel(s) outside their model or with colour 0 were skipped"));
                }
                if models.len() >= 65_536 {
                    return Err(Error("the file has more than 65536 models".to_string()));
                }
                models.push(VoxModel { size, voxels });
            }
            b"RGBA" => {
                let raw = c.bytes(256 * 4)?;
                let mut p = [[0u8; 4]; 256];
                for i in 0..255 {
                    p[i + 1] = [raw[i * 4], raw[i * 4 + 1], raw[i * 4 + 2], raw[i * 4 + 3]];
                }
                palette = Some(p);
            }
            b"nTRN" => {
                let id = c.i32()?;
                let attributes = c.dict()?;
                let child = c.i32()?;
                let _reserved = c.i32()?;
                let layer = c.i32()?;
                let frames = c.count(4, "frame")?;
                let mut rotation = [[1, 0, 0], [0, 1, 0], [0, 0, 1]];
                let mut translation = [0; 3];
                for frame in 0..frames {
                    let attrs = c.dict()?;
                    if frame > 0 {
                        continue; // анимация: кадр 0
                    }
                    if let Some(t) = attrs.get("_t") {
                        let parts: Vec<i32> = t.split_whitespace().filter_map(|p| p.parse().ok()).collect();
                        if parts.len() == 3 && parts.iter().all(|p| p.abs() < 1 << 20) {
                            translation = [parts[0], parts[1], parts[2]];
                        }
                    }
                    if let Some(bits) = attrs.get("_r").and_then(|r| r.trim().parse::<u8>().ok()) {
                        rotation = decode_rotation(bits).ok_or_else(|| Error(format!("the .vox file is damaged: rotation code {bits} is not a rotation")))?;
                    }
                }
                if frames > 1 {
                    report.ignore("voxel animation frames (frame 0 is used)");
                }
                let hidden = attributes.get("_hidden").is_some_and(|h| h == "1");
                nodes.insert(id, SceneNode::Transform { child, rotation, translation, hidden, layer });
            }
            b"nGRP" => {
                let id = c.i32()?;
                c.dict()?;
                let n = c.count(4, "group child")?;
                let children = (0..n).map(|_| c.i32()).collect::<Result<Vec<_>, _>>()?;
                nodes.insert(id, SceneNode::Group { children });
            }
            b"nSHP" => {
                let id = c.i32()?;
                c.dict()?;
                let n = c.count(8, "shape model")?;
                let mut list = Vec::with_capacity(n);
                for _ in 0..n {
                    let model = c.i32()?;
                    c.dict()?;
                    list.push(model.max(0) as usize);
                }
                nodes.insert(id, SceneNode::Shape { models: list });
            }
            b"LAYR" => {
                let id = c.i32()?;
                let attributes = c.dict()?;
                if attributes.get("_hidden").is_some_and(|h| h == "1") {
                    hidden_layers.push(id);
                }
            }
            b"MATL" | b"MATT" => report.ignore("voxel materials (`MATL`: glass, metal, emission)"),
            _ => {}
        }
    }
    if models.is_empty() {
        return Err(Error("the .vox file has no models (`SIZE` / `XYZI` chunks)".to_string()));
    }
    let palette = palette.unwrap_or_else(|| {
        report.warn("the file has no palette (`RGBA`); the MagicaVoxel default palette is used");
        default_palette()
    });

    // Мировые ячейки (координаты MagicaVoxel, Z вверх).
    let mut world: Vec<([i32; 3], u8)> = Vec::new();
    let identity = Transform { rotation: [[1, 0, 0], [0, 1, 0], [0, 0, 1]], translation: [0; 3] };
    if nodes.contains_key(&0) {
        let mut ctx = Walk { nodes: &nodes, models: &models, hidden_layers: &hidden_layers, out: &mut world, report: &mut report, stack: Vec::new() };
        ctx.visit(0, identity)?;
    } else {
        for model in &models {
            place(model, &identity, &mut world)?;
        }
    }
    if world.is_empty() {
        return Err(Error("the .vox file has no visible voxels".to_string()));
    }
    // Y вверх, на землю, по центру.
    let mut cells: Vec<([i32; 3], u8)> = world.into_iter().map(|(p, c)| ([p[0], p[2], -p[1] - 1], c)).collect();
    let mut min = [i32::MAX; 3];
    let mut max = [i32::MIN; 3];
    for (p, _) in &cells {
        for i in 0..3 {
            min[i] = min[i].min(p[i]);
            max[i] = max[i].max(p[i]);
        }
    }
    let shift = [-(min[0] + max[0] + 1).div_euclid(2), -min[1], -(min[2] + max[2] + 1).div_euclid(2)];
    for (p, _) in &mut cells {
        for i in 0..3 {
            p[i] += shift[i];
        }
    }
    Ok(VoxScene { voxels: cells, palette, report })
}

/// Код поворота `_r`: в битах 0–1 — столбец единицы в строке 0, в битах 2–3 — в строке 1,
/// биты 4, 5, 6 — знаки строк 0, 1, 2 (1 — минус).
fn decode_rotation(bits: u8) -> Option<[[i32; 3]; 3]> {
    let first = (bits & 3) as usize;
    let second = ((bits >> 2) & 3) as usize;
    if first > 2 || second > 2 || first == second {
        return None;
    }
    let third = 3 - first - second;
    let mut m = [[0; 3]; 3];
    for (row, column) in [first, second, third].into_iter().enumerate() {
        m[row][column] = if bits >> (4 + row) & 1 == 1 { -1 } else { 1 };
    }
    Some(m)
}

#[derive(Clone, Copy)]
struct Transform {
    rotation: [[i32; 3]; 3],
    translation: [i32; 3],
}

impl Transform {
    fn then(&self, inner: &Transform) -> Transform {
        let mut rotation = [[0; 3]; 3];
        for (i, row) in rotation.iter_mut().enumerate() {
            for (j, cell) in row.iter_mut().enumerate() {
                *cell = (0..3).map(|k| self.rotation[i][k] * inner.rotation[k][j]).sum();
            }
        }
        let t = self.apply(inner.translation);
        Transform { rotation, translation: [t[0] + self.translation[0], t[1] + self.translation[1], t[2] + self.translation[2]] }
    }

    fn apply(&self, v: [i32; 3]) -> [i32; 3] {
        std::array::from_fn(|i| (0..3).map(|k| self.rotation[i][k] * v[k]).sum())
    }
}

/// Ячейки модели в мире: центр ячейки v + ½ − ⌊size/2⌋ поворачивается и сдвигается
/// (в удвоенных координатах, чтобы остаться в целых).
fn place(model: &VoxModel, transform: &Transform, out: &mut Vec<([i32; 3], u8)>) -> Result<(), Error> {
    if out.len() + model.voxels.len() > limits::MAX_VOXEL_CELLS as usize {
        return Err(Error(format!("the scene has more than {} voxels", limits::MAX_VOXEL_CELLS)));
    }
    let half = model.size.map(|s| s / 2);
    for v in &model.voxels {
        let twice: [i32; 3] = std::array::from_fn(|i| 2 * (v[i] as i32 - half[i]) + 1);
        let r = transform.apply(twice);
        let cell: [i32; 3] = std::array::from_fn(|i| (2 * transform.translation[i] + r[i] - 1).div_euclid(2));
        out.push((cell, v[3]));
    }
    Ok(())
}

struct Walk<'a> {
    nodes: &'a HashMap<i32, SceneNode>,
    models: &'a [VoxModel],
    hidden_layers: &'a [i32],
    out: &'a mut Vec<([i32; 3], u8)>,
    report: &'a mut Report,
    stack: Vec<i32>,
}

impl Walk<'_> {
    fn visit(&mut self, id: i32, parent: Transform) -> Result<(), Error> {
        if self.stack.contains(&id) || self.stack.len() > limits::MAX_DEPTH {
            return Err(Error("the .vox scene graph loops or is nested too deeply".to_string()));
        }
        let Some(node) = self.nodes.get(&id) else {
            self.report.warn(format!("scene node {id} does not exist"));
            return Ok(());
        };
        self.stack.push(id);
        match node {
            SceneNode::Transform { child, rotation, translation, hidden, layer } => {
                if *hidden || self.hidden_layers.contains(layer) {
                    self.report.ignore("hidden objects and layers");
                } else {
                    let local = Transform { rotation: *rotation, translation: *translation };
                    self.visit(*child, parent.then(&local))?;
                }
            }
            SceneNode::Group { children } => {
                for child in children.clone() {
                    self.visit(child, parent)?;
                }
            }
            SceneNode::Shape { models } => {
                for &m in models {
                    match self.models.get(m) {
                        Some(model) => place(model, &parent, self.out)?,
                        None => self.report.warn(format!("a shape refers to model {m} that does not exist")),
                    }
                }
            }
        }
        self.stack.pop();
        Ok(())
    }
}

/// Плотная сетка ячеек сцены: размеры и номер цвета в каждой (0 — пусто).
struct Grid {
    size: [usize; 3],
    /// Ячейка сцены, соответствующая (0, 0, 0) сетки.
    origin: [i32; 3],
    cells: Vec<u8>,
}

impl Grid {
    fn new(scene: &VoxScene) -> Result<Grid, Error> {
        let mut min = [i32::MAX; 3];
        let mut max = [i32::MIN; 3];
        for (p, _) in &scene.voxels {
            for i in 0..3 {
                min[i] = min[i].min(p[i]);
                max[i] = max[i].max(p[i]);
            }
        }
        if scene.voxels.is_empty() {
            return Err(Error("the voxel scene is empty".to_string()));
        }
        let size: [usize; 3] = std::array::from_fn(|i| (max[i] as i64 - min[i] as i64 + 1) as usize);
        let total = size[0] as u64 * size[1] as u64 * size[2] as u64;
        if total > limits::MAX_VOXEL_CELLS {
            return Err(Error(format!(
                "the voxel scene spans {}x{}x{} cells; at most {} cells are supported",
                size[0], size[1], size[2], limits::MAX_VOXEL_CELLS
            )));
        }
        let mut grid = Grid { size, origin: min, cells: vec![0; total as usize] };
        for (p, c) in &scene.voxels {
            let i = grid.index(std::array::from_fn(|k| (p[k] - min[k]) as usize));
            grid.cells[i] = *c;
        }
        Ok(grid)
    }

    fn index(&self, p: [usize; 3]) -> usize {
        p[0] + self.size[0] * (p[1] + self.size[1] * p[2])
    }

    fn get(&self, p: [i64; 3]) -> u8 {
        if (0..3).any(|i| p[i] < 0 || p[i] >= self.size[i] as i64) {
            return 0;
        }
        self.cells[self.index([p[0] as usize, p[1] as usize, p[2] as usize])]
    }
}

/// Палитра картинкой 16×16: цвет `i` — тексель (i % 16, i / 16).
pub fn palette_image(palette: &[[u8; 4]; 256]) -> RgbaImage {
    let mut image = RgbaImage::new(16, 16, [0; 4]);
    for (i, color) in palette.iter().enumerate() {
        image.set(i as u32 % 16, i as u32 / 16, *color);
    }
    image
}

fn texel_center(color: u8) -> [f32; 2] {
    [((color % 16) as f32 + 0.5) / 16.0, ((color / 16) as f32 + 0.5) / 16.0]
}

/// Жадная сетка: меш с палитрой для движка.
pub fn to_mesh(scene: &VoxScene, options: &Options) -> Result<Imported, Error> {
    let grid = Grid::new(scene)?;
    let mut builder = Builder::default();
    let size = grid.size;
    for d in 0..3 {
        let (u, v) = ((d + 1) % 3, (d + 2) % 3);
        let mut mask = vec![0u8; size[u] * size[v]];
        for dir in [1i64, -1] {
            for s in 0..size[d] {
                for j in 0..size[v] {
                    for i in 0..size[u] {
                        let mut p = [0i64; 3];
                        p[d] = s as i64;
                        p[u] = i as i64;
                        p[v] = j as i64;
                        let here = grid.get(p);
                        p[d] += dir;
                        mask[i + j * size[u]] = if here != 0 && grid.get(p) == 0 { here } else { 0 };
                    }
                }
                for j in 0..size[v] {
                    let mut i = 0;
                    while i < size[u] {
                        let color = mask[i + j * size[u]];
                        if color == 0 {
                            i += 1;
                            continue;
                        }
                        let mut w = 1;
                        while i + w < size[u] && mask[i + w + j * size[u]] == color {
                            w += 1;
                        }
                        let mut h = 1;
                        'grow: while j + h < size[v] {
                            for k in 0..w {
                                if mask[i + k + (j + h) * size[u]] != color {
                                    break 'grow;
                                }
                            }
                            h += 1;
                        }
                        for jj in 0..h {
                            for k in 0..w {
                                mask[i + k + (j + jj) * size[u]] = 0;
                            }
                        }
                        let mut base = [0.0f32; 3];
                        base[d] = (s as i64 + if dir > 0 { 1 } else { 0 } + grid.origin[d] as i64) as f32;
                        base[u] = (i as i64 + grid.origin[u] as i64) as f32;
                        base[v] = (j as i64 + grid.origin[v] as i64) as f32;
                        let mut du = [0.0f32; 3];
                        du[u] = w as f32;
                        let mut dv = [0.0f32; 3];
                        dv[v] = h as f32;
                        let p0 = base;
                        let p1 = std::array::from_fn(|k| base[k] + du[k]);
                        let p2 = std::array::from_fn(|k| base[k] + du[k] + dv[k]);
                        let p3 = std::array::from_fn(|k| base[k] + dv[k]);
                        // e_u × e_v = e_d: обход p0 p1 p2 p3 смотрит в +d.
                        let points = if dir > 0 { [p0, p1, p2, p3] } else { [p0, p3, p2, p1] };
                        builder.polygon(&points, &[texel_center(color); 4], Slot::Texture(0))?;
                        i += w;
                    }
                }
            }
        }
    }
    let sheets = [Sheet { name: "palette".to_string(), image: palette_image(&scene.palette) }];
    finish(builder, &sheets, Format::Vox, [0.0; 3], options, scene.report.clone())
}

/// Кубоиды для редактора моделей: жадное объединение ячеек одного цвета в коробки.
pub fn to_model(scene: &VoxScene, name: &str) -> Result<Model, Error> {
    let grid = Grid::new(scene)?;
    let size = grid.size;
    let mut used = vec![false; grid.cells.len()];
    let mut roots = Vec::new();
    for z in 0..size[2] {
        for y in 0..size[1] {
            for x in 0..size[0] {
                let start = grid.index([x, y, z]);
                let color = grid.cells[start];
                if color == 0 || used[start] {
                    continue;
                }
                let free = |p: [usize; 3], used: &Vec<bool>| {
                    let i = grid.index(p);
                    grid.cells[i] == color && !used[i]
                };
                let mut x1 = x + 1;
                while x1 < size[0] && free([x1, y, z], &used) {
                    x1 += 1;
                }
                let mut z1 = z + 1;
                while z1 < size[2] && (x..x1).all(|xx| free([xx, y, z1], &used)) {
                    z1 += 1;
                }
                let mut y1 = y + 1;
                while y1 < size[1] && (x..x1).all(|xx| (z..z1).all(|zz| free([xx, y1, zz], &used))) {
                    y1 += 1;
                }
                for zz in z..z1 {
                    for yy in y..y1 {
                        for xx in x..x1 {
                            used[grid.index([xx, yy, zz])] = true;
                        }
                    }
                }
                if roots.len() >= limits::MAX_VOXEL_BOXES {
                    return Err(Error(format!(
                        "the voxel model needs more than {} boxes to edit; import it into the engine directly instead",
                        limits::MAX_VOXEL_BOXES
                    )));
                }
                let (lo, hi) = ([x as i64, y as i64, z as i64], [x1 as i64, y1 as i64, z1 as i64]);
                // Сторона закрыта, если за ней все ячейки заняты.
                let covered = |side: Side| -> bool {
                    let (axis, outside) = match side {
                        Side::East => (0, hi[0]),
                        Side::West => (0, lo[0] - 1),
                        Side::Up => (1, hi[1]),
                        Side::Down => (1, lo[1] - 1),
                        Side::South => (2, hi[2]),
                        Side::North => (2, lo[2] - 1),
                    };
                    let (a, b) = ((axis + 1) % 3, (axis + 2) % 3);
                    (lo[a]..hi[a]).all(|i| {
                        (lo[b]..hi[b]).all(|j| {
                            let mut p = [0i64; 3];
                            p[axis] = outside;
                            p[a] = i;
                            p[b] = j;
                            grid.get(p) != 0
                        })
                    })
                };
                let texel = [(color % 16) as f32, (color / 16) as f32];
                let mut faces = [None; 6];
                for side in Side::ALL {
                    if !covered(side) {
                        faces[side.index()] = Some(CubeFace { uv: [texel[0], texel[1], texel[0] + 1.0, texel[1] + 1.0], rotation: 0, texture: Some(0) });
                    }
                }
                let o = grid.origin;
                let from = [(x as i32 + o[0]) as f32, (y as i32 + o[1]) as f32, (z as i32 + o[2]) as f32];
                let to = [(x1 as i32 + o[0]) as f32, (y1 as i32 + o[1]) as f32, (z1 as i32 + o[2]) as f32];
                roots.push(Node::Cube(Cube {
                    name: format!("voxels {}", roots.len() + 1),
                    uuid: String::new(),
                    from,
                    to,
                    origin: std::array::from_fn(|i| (from[i] + to[i]) * 0.5),
                    rotation: [0.0; 3],
                    inflate: 0.0,
                    rescale: false,
                    faces,
                    box_uv: None,
                    visible: true,
                    locked: false,
                }));
            }
        }
    }
    Ok(Model {
        name: name.to_string(),
        format: Format::Vox,
        textures: vec![Texture { name: "palette".to_string(), image: Some(palette_image(&scene.palette)), uv_size: [16.0, 16.0], path: None }],
        roots,
        offset: [0.0; 3],
        report: scene.report.clone(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_palette_has_the_known_layout() {
        let p = default_palette();
        assert_eq!(p[0], [0, 0, 0, 0]);
        assert_eq!(p[1], [255, 255, 255, 255]);
        assert_eq!(p[2], [255, 255, 204, 255], "blue changes fastest");
        assert_eq!(p[215], [0, 0, 51, 255], "the last cube colour before black");
        assert_eq!(p[216], [0, 0, 238, 255], "blue ramp");
        assert_eq!(p[255], [17, 17, 17, 255], "grey ramp ends the palette");
    }

    #[test]
    fn rotation_codes_decode_to_signed_permutations() {
        // 4 = 0b0000100: строка 0 → столбец 0, строка 1 → столбец 1, знаки +: единичная.
        assert_eq!(decode_rotation(4), Some([[1, 0, 0], [0, 1, 0], [0, 0, 1]]));
        // Строка 0 → столбец 1, строка 1 → столбец 0, знак строки 0 минус.
        assert_eq!(decode_rotation(0b0010001), Some([[0, -1, 0], [1, 0, 0], [0, 0, 1]]));
        assert_eq!(decode_rotation(0b0000000), None, "two rows in one column");
        assert_eq!(decode_rotation(3), None);
    }
}
