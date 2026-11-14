//! Простые сетки треугольников: Stanford `.ply` (ASCII, двоичный little- и big-endian;
//! цвета вершин и граней, UV и `comment TextureFile`) и `.stl` (ASCII и двоичный).
//!
//! В движке цвета вершин становятся палитрой-текстурой: каждый различный цвет — тексель,
//! UV угла указывает в его центр (низкополигональные модели с цветом по граням выглядят
//! как в источнике; плавные переходы цвета внутри грани не передаются).

use std::collections::HashMap;

use crate::image::{decode_png, RgbaImage};
use crate::math::V3;
use crate::mesh::{finish, Builder, Imported, Options, Sheet, Slot};
use crate::model::Report;
use crate::{limits, safe_relative_path, Error, Format, Resolver};

/// Сетка из файла: треугольники (многоугольники уже разрезаны веером).
#[derive(Clone, Debug, Default, PartialEq)]
pub struct PolyMesh {
    pub positions: Vec<V3>,
    pub triangles: Vec<[u32; 3]>,
    /// Цвет каждой вершины.
    pub vertex_colors: Option<Vec<[u8; 4]>>,
    /// Цвет каждого треугольника (важнее цветов вершин).
    pub face_colors: Option<Vec<[u8; 4]>>,
    /// UV вершин ((0, 0) — левый верхний угол) и текстура.
    pub uvs: Option<Vec<[f32; 2]>>,
    pub texture: Option<RgbaImage>,
    pub report: Report,
}

// ----------------------------------------------------------------------
// PLY
// ----------------------------------------------------------------------

#[derive(Clone, Copy, PartialEq, Debug)]
enum Encoding {
    Ascii,
    Little,
    Big,
}

#[derive(Clone, Copy, Debug)]
enum Scalar {
    I8,
    U8,
    I16,
    U16,
    I32,
    U32,
    F32,
    F64,
}

impl Scalar {
    fn parse(name: &str) -> Option<Scalar> {
        Some(match name {
            "char" | "int8" => Scalar::I8,
            "uchar" | "uint8" => Scalar::U8,
            "short" | "int16" => Scalar::I16,
            "ushort" | "uint16" => Scalar::U16,
            "int" | "int32" => Scalar::I32,
            "uint" | "uint32" => Scalar::U32,
            "float" | "float32" => Scalar::F32,
            "double" | "float64" => Scalar::F64,
            _ => return None,
        })
    }

    fn size(self) -> usize {
        match self {
            Scalar::I8 | Scalar::U8 => 1,
            Scalar::I16 | Scalar::U16 => 2,
            Scalar::I32 | Scalar::U32 | Scalar::F32 => 4,
            Scalar::F64 => 8,
        }
    }

    /// Цвет 0…255: целые — как есть, дробные — доля от 1.
    fn is_float(self) -> bool {
        matches!(self, Scalar::F32 | Scalar::F64)
    }
}

#[derive(Clone, Debug)]
struct Property {
    name: String,
    kind: Scalar,
    /// Тип длины списка (`property list uchar int vertex_indices`).
    list: Option<Scalar>,
}

#[derive(Clone, Debug)]
struct Element {
    name: String,
    count: usize,
    properties: Vec<Property>,
}

struct Header {
    encoding: Encoding,
    elements: Vec<Element>,
    texture_file: Option<String>,
    body: usize,
}

fn parse_header(bytes: &[u8]) -> Result<Header, Error> {
    if !bytes.starts_with(b"ply") {
        return Err(Error("not a PLY file (it does not start with `ply`)".to_string()));
    }
    let limit = bytes.len().min(1 << 16);
    let end = bytes[..limit]
        .windows(10)
        .position(|w| w == b"end_header")
        .ok_or_else(|| Error("the PLY header has no `end_header` in its first 64 KB".to_string()))?;
    let mut body = end + 10;
    // Конец строки после end_header: \n или \r\n.
    if bytes.get(body) == Some(&b'\r') {
        body += 1;
    }
    if bytes.get(body) == Some(&b'\n') {
        body += 1;
    }
    let text = String::from_utf8_lossy(&bytes[..end]);
    let mut encoding = None;
    let mut elements: Vec<Element> = Vec::new();
    let mut texture_file = None;
    for (number, line) in text.lines().enumerate().skip(1) {
        let parts: Vec<&str> = line.split_whitespace().collect();
        let fail = |what: &str| Error(format!("PLY header line {}: {what}", number + 1));
        match parts.as_slice() {
            [] => {}
            ["format", kind, _version] => {
                encoding = Some(match *kind {
                    "ascii" => Encoding::Ascii,
                    "binary_little_endian" => Encoding::Little,
                    "binary_big_endian" => Encoding::Big,
                    other => return Err(fail(&format!("unknown format `{other}`"))),
                })
            }
            ["comment", "TextureFile", file, ..] | ["comment", "texturefile", file, ..] => texture_file = Some(file.to_string()),
            ["comment", ..] | ["obj_info", ..] => {}
            ["element", name, count] => {
                let count: usize = count.parse().map_err(|_| fail(&format!("bad element count `{count}`")))?;
                if elements.len() > 64 {
                    return Err(fail("too many elements"));
                }
                elements.push(Element { name: name.to_string(), count, properties: Vec::new() });
            }
            ["property", "list", count_type, item_type, name] => {
                let element = elements.last_mut().ok_or_else(|| fail("a property before any element"))?;
                let list = Scalar::parse(count_type).ok_or_else(|| fail(&format!("unknown type `{count_type}`")))?;
                let kind = Scalar::parse(item_type).ok_or_else(|| fail(&format!("unknown type `{item_type}`")))?;
                element.properties.push(Property { name: name.to_string(), kind, list: Some(list) });
            }
            ["property", kind, name] => {
                let element = elements.last_mut().ok_or_else(|| fail("a property before any element"))?;
                let kind = Scalar::parse(kind).ok_or_else(|| fail(&format!("unknown type `{kind}`")))?;
                element.properties.push(Property { name: name.to_string(), kind, list: None });
            }
            _ => return Err(fail(&format!("cannot read `{}`", crate::json::short(line)))),
        }
    }
    let encoding = encoding.ok_or_else(|| Error("the PLY header has no `format` line".to_string()))?;
    Ok(Header { encoding, elements, texture_file, body })
}

/// Источник чисел тела файла.
struct Body<'a> {
    bytes: &'a [u8],
    at: usize,
    encoding: Encoding,
}

impl Body<'_> {
    fn token(&mut self) -> Result<&str, Error> {
        while self.at < self.bytes.len() && self.bytes[self.at].is_ascii_whitespace() {
            self.at += 1;
        }
        let start = self.at;
        while self.at < self.bytes.len() && !self.bytes[self.at].is_ascii_whitespace() {
            self.at += 1;
        }
        if start == self.at {
            return Err(Error("the PLY file is truncated (fewer values than the header declares)".to_string()));
        }
        std::str::from_utf8(&self.bytes[start..self.at]).map_err(|_| Error("the PLY data is not ASCII".to_string()))
    }

    fn value(&mut self, kind: Scalar) -> Result<f64, Error> {
        if self.encoding == Encoding::Ascii {
            let token = self.token()?;
            return token.parse::<f64>().map_err(|_| Error(format!("PLY value `{}` is not a number", crate::json::short(token))));
        }
        let n = kind.size();
        let raw = self
            .bytes
            .get(self.at..self.at + n)
            .ok_or_else(|| Error("the PLY file is truncated (fewer values than the header declares)".to_string()))?;
        self.at += n;
        let mut b = [0u8; 8];
        b[..n].copy_from_slice(raw);
        if self.encoding == Encoding::Big {
            b[..n].reverse();
        }
        Ok(match kind {
            Scalar::I8 => b[0] as i8 as f64,
            Scalar::U8 => b[0] as f64,
            Scalar::I16 => i16::from_le_bytes([b[0], b[1]]) as f64,
            Scalar::U16 => u16::from_le_bytes([b[0], b[1]]) as f64,
            Scalar::I32 => i32::from_le_bytes([b[0], b[1], b[2], b[3]]) as f64,
            Scalar::U32 => u32::from_le_bytes([b[0], b[1], b[2], b[3]]) as f64,
            Scalar::F32 => f32::from_le_bytes([b[0], b[1], b[2], b[3]]) as f64,
            Scalar::F64 => f64::from_le_bytes(b),
        })
    }

    fn remaining(&self) -> usize {
        self.bytes.len() - self.at
    }
}

fn color(value: f64, kind: Scalar) -> u8 {
    let v = if kind.is_float() { value * 255.0 } else { value };
    v.round().clamp(0.0, 255.0) as u8
}

/// Прочитать `.ply`.
pub fn read_ply(bytes: &[u8], resolve: Resolver) -> Result<PolyMesh, Error> {
    let header = parse_header(bytes)?;
    let mut body = Body { bytes, at: header.body, encoding: header.encoding };
    let mut mesh = PolyMesh::default();
    let mut colors: Vec<[u8; 4]> = Vec::new();
    let mut uvs: Vec<[f32; 2]> = Vec::new();
    let mut face_colors: Vec<[u8; 4]> = Vec::new();
    let mut corner_uvs: Vec<[f32; 2]> = Vec::new(); // `texcoord` граней: UV по углам треугольников
    for element in &header.elements {
        // Каждое значение занимает хотя бы байт: счётчик больше остатка файла — ложь.
        let least = element.properties.len().max(1);
        if element.count.saturating_mul(least) > body.remaining() {
            return Err(Error(format!(
                "the PLY header declares {} `{}` items, but the file is too short for them",
                element.count, element.name
            )));
        }
        let find = |names: &[&str]| element.properties.iter().position(|p| p.list.is_none() && names.contains(&p.name.as_str()));
        match element.name.as_str() {
            "vertex" => {
                if element.count > limits::MAX_VERTICES {
                    return Err(Error(format!("the PLY file has {} vertices; at most {} are supported", element.count, limits::MAX_VERTICES)));
                }
                let xyz = [find(&["x"]), find(&["y"]), find(&["z"])];
                if xyz.iter().any(Option::is_none) {
                    return Err(Error("PLY vertices have no x, y, z properties".to_string()));
                }
                let rgba = [find(&["red", "r", "diffuse_red"]), find(&["green", "g", "diffuse_green"]), find(&["blue", "b", "diffuse_blue"]), find(&["alpha", "a"])];
                let st = [find(&["s", "u", "texture_u", "texture_s"]), find(&["t", "v", "texture_v", "texture_t"])];
                let has_color = rgba[..3].iter().all(Option::is_some);
                let has_uv = st.iter().all(Option::is_some);
                let mut values = vec![0f64; element.properties.len()];
                for _ in 0..element.count {
                    for (i, p) in element.properties.iter().enumerate() {
                        values[i] = read_property(&mut body, p)?;
                    }
                    let p = [values[xyz[0].unwrap_or(0)], values[xyz[1].unwrap_or(0)], values[xyz[2].unwrap_or(0)]];
                    if p.iter().any(|v| !v.is_finite() || v.abs() > limits::MAX_COORDINATE as f64) {
                        return Err(Error(format!("PLY vertex {} has a position out of range", mesh.positions.len())));
                    }
                    mesh.positions.push(p.map(|v| v as f32));
                    if has_color {
                        let channel = |k: usize| rgba[k].map_or(255, |i| color(values[i], element.properties[i].kind));
                        colors.push([channel(0), channel(1), channel(2), channel(3)]);
                    }
                    if has_uv {
                        let (s, t) = (values[st[0].unwrap_or(0)], values[st[1].unwrap_or(0)]);
                        // У PLY (0, 0) — левый нижний угол (как в OpenGL).
                        uvs.push([s as f32, 1.0 - t as f32]);
                    }
                }
            }
            "face" => {
                let index_list = element
                    .properties
                    .iter()
                    .position(|p| p.list.is_some() && (p.name == "vertex_indices" || p.name == "vertex_index"))
                    .ok_or_else(|| Error("PLY faces have no `vertex_indices` list".to_string()))?;
                let texcoord = element.properties.iter().position(|p| p.list.is_some() && p.name == "texcoord");
                let rgba = [find(&["red"]), find(&["green"]), find(&["blue"]), find(&["alpha"])];
                let has_color = rgba[..3].iter().all(Option::is_some);
                for _ in 0..element.count {
                    let mut corners: Vec<u32> = Vec::new();
                    let mut coords: Vec<f64> = Vec::new();
                    let mut scalars = vec![0f64; element.properties.len()];
                    for (i, p) in element.properties.iter().enumerate() {
                        match p.list {
                            Some(count_type) => {
                                let n = body.value(count_type)?;
                                if !(0.0..=1024.0).contains(&n) {
                                    return Err(Error(format!("a PLY face has {n} corners; at most 1024 are supported")));
                                }
                                for _ in 0..n as usize {
                                    let v = body.value(p.kind)?;
                                    if i == index_list {
                                        if v < 0.0 || v as usize >= mesh.positions.len() {
                                            return Err(Error(format!(
                                                "a PLY face uses vertex {v}; the file has {} vertices",
                                                mesh.positions.len()
                                            )));
                                        }
                                        corners.push(v as u32);
                                    } else if Some(i) == texcoord {
                                        coords.push(v);
                                    }
                                }
                            }
                            None => scalars[i] = body.value(p.kind)?,
                        }
                    }
                    if corners.len() < 3 {
                        continue;
                    }
                    if mesh.triangles.len() + corners.len() - 2 > limits::MAX_TRIANGLES {
                        return Err(Error(format!("the PLY file has more than {} triangles", limits::MAX_TRIANGLES)));
                    }
                    let face_color = has_color.then(|| {
                        let channel = |k: usize| rgba[k].map_or(255, |i| color(scalars[i], element.properties[i].kind));
                        [channel(0), channel(1), channel(2), channel(3)]
                    });
                    for k in 1..corners.len() - 1 {
                        mesh.triangles.push([corners[0], corners[k], corners[k + 1]]);
                        if let Some(c) = face_color {
                            face_colors.push(c);
                        }
                        if coords.len() == corners.len() * 2 {
                            for c in [0, k, k + 1] {
                                corner_uvs.push([coords[2 * c] as f32, 1.0 - coords[2 * c + 1] as f32]);
                            }
                        }
                    }
                }
            }
            _ => {
                mesh.report.ignore(format!("PLY element `{}`", crate::json::short(&element.name)));
                for _ in 0..element.count {
                    for p in &element.properties {
                        read_property(&mut body, p)?;
                    }
                }
            }
        }
    }
    if mesh.triangles.is_empty() {
        return Err(Error("the PLY file has no faces (point clouds are not imported)".to_string()));
    }
    if colors.len() == mesh.positions.len() && !colors.is_empty() {
        mesh.vertex_colors = Some(colors);
    }
    if face_colors.len() == mesh.triangles.len() {
        mesh.face_colors = Some(face_colors);
    }
    if uvs.len() == mesh.positions.len() && !uvs.is_empty() {
        mesh.uvs = Some(uvs);
    } else if corner_uvs.len() == mesh.triangles.len() * 3 {
        // UV по углам граней: каждый треугольник со своими вершинами.
        let mut positions = Vec::with_capacity(corner_uvs.len());
        let mut triangles = Vec::with_capacity(mesh.triangles.len());
        let mut colors = mesh.vertex_colors.as_ref().map(|_| Vec::with_capacity(corner_uvs.len()));
        for t in &mesh.triangles {
            let base = positions.len() as u32;
            for &v in t {
                positions.push(mesh.positions[v as usize]);
                if let (Some(out), Some(source)) = (colors.as_mut(), mesh.vertex_colors.as_ref()) {
                    out.push(source[v as usize]);
                }
            }
            triangles.push([base, base + 1, base + 2]);
        }
        mesh.positions = positions;
        mesh.triangles = triangles;
        mesh.vertex_colors = colors;
        mesh.uvs = Some(corner_uvs);
    }
    if let Some(file) = &header.texture_file {
        match safe_relative_path(file, true).and_then(|p| resolve(&p).map(|b| (p, b))) {
            Some((p, bytes)) => match decode_png(&bytes) {
                Ok(image) => mesh.texture = Some(image),
                Err(e) => mesh.report.warn(format!("texture `{p}` is not a readable PNG ({e})")),
            },
            None => mesh.report.warn(format!("texture `{}` was not found next to the model", crate::json::short(file))),
        }
    }
    if mesh.texture.is_some() && mesh.uvs.is_none() {
        mesh.report.warn("the PLY file names a texture but has no UV coordinates; the texture is not used");
        mesh.texture = None;
    }
    Ok(mesh)
}

fn read_property(body: &mut Body, p: &Property) -> Result<f64, Error> {
    match p.list {
        Some(count_type) => {
            let n = body.value(count_type)?;
            if !(0.0..=1024.0).contains(&n) {
                return Err(Error(format!("a PLY list has {n} items; at most 1024 are supported")));
            }
            for _ in 0..n as usize {
                body.value(p.kind)?;
            }
            Ok(n)
        }
        None => body.value(p.kind),
    }
}

/// Есть ли в `.ply` цвета или текстура (по заголовку).
pub fn ply_has_texture(bytes: &[u8]) -> bool {
    parse_header(bytes).is_ok_and(|h| {
        h.texture_file.is_some()
            || h.elements.iter().any(|e| (e.name == "vertex" || e.name == "face") && e.properties.iter().any(|p| matches!(p.name.as_str(), "red" | "r" | "diffuse_red")))
    })
}

/// Внешняя текстура `.ply` (`comment TextureFile`).
pub fn ply_dependencies(bytes: &[u8]) -> Vec<String> {
    parse_header(bytes).ok().and_then(|h| h.texture_file).and_then(|f| safe_relative_path(&f, true)).into_iter().collect()
}

// ----------------------------------------------------------------------
// STL
// ----------------------------------------------------------------------

/// Прочитать `.stl` (двоичный или ASCII). Нормали файла не используются: сторона грани
/// — по обходу вершин (против часовой стрелки снаружи, как требует формат).
pub fn read_stl(bytes: &[u8]) -> Result<PolyMesh, Error> {
    let binary_count = (bytes.len() >= 84).then(|| u32::from_le_bytes([bytes[80], bytes[81], bytes[82], bytes[83]]) as usize);
    let exact_binary = binary_count.is_some_and(|n| n.checked_mul(50).and_then(|b| b.checked_add(84)) == Some(bytes.len()));
    let looks_ascii = bytes.iter().take(512).skip_while(|b| b.is_ascii_whitespace()).take(5).eq(b"solid".iter())
        && bytes.windows(5).take(4096).any(|w| w == b"facet" || w == b"endso");
    let mut mesh = PolyMesh::default();
    if !exact_binary && looks_ascii {
        let text = String::from_utf8_lossy(bytes);
        let mut corners: Vec<V3> = Vec::new();
        let mut tokens = text.split_whitespace();
        while let Some(token) = tokens.next() {
            match token {
                "vertex" => {
                    let mut p = [0.0f32; 3];
                    for v in &mut p {
                        let t = tokens.next().ok_or_else(|| Error("the STL file is truncated inside a `vertex`".to_string()))?;
                        let n: f64 = t.parse().map_err(|_| Error(format!("STL value `{}` is not a number", crate::json::short(t))))?;
                        if !n.is_finite() || n.abs() > limits::MAX_COORDINATE as f64 {
                            return Err(Error("an STL vertex is out of range".to_string()));
                        }
                        *v = n as f32;
                    }
                    corners.push(p);
                }
                "endloop" => {
                    if corners.len() >= 3 {
                        push_polygon(&mut mesh, &corners)?;
                    }
                    corners.clear();
                }
                _ => {}
            }
        }
    } else {
        let count = binary_count.ok_or_else(|| Error("not an STL file (too short for a binary STL, no `solid` text)".to_string()))?;
        let needed = count.checked_mul(50).and_then(|b| b.checked_add(84)).unwrap_or(usize::MAX);
        if needed > bytes.len() {
            return Err(Error(format!(
                "the binary STL declares {count} triangles ({needed} bytes), but the file has {} bytes",
                bytes.len()
            )));
        }
        if count > limits::MAX_TRIANGLES {
            return Err(Error(format!("the STL file has {count} triangles; at most {} are supported", limits::MAX_TRIANGLES)));
        }
        for t in 0..count {
            let at = 84 + t * 50 + 12;
            let f = |k: usize| {
                let o = at + k * 4;
                f32::from_le_bytes([bytes[o], bytes[o + 1], bytes[o + 2], bytes[o + 3]])
            };
            let corners = [[f(0), f(1), f(2)], [f(3), f(4), f(5)], [f(6), f(7), f(8)]];
            if corners.iter().flatten().any(|v| !v.is_finite() || v.abs() > limits::MAX_COORDINATE) {
                return Err(Error(format!("STL triangle {t} has a vertex out of range")));
            }
            push_polygon(&mut mesh, &corners)?;
        }
    }
    if mesh.triangles.is_empty() {
        return Err(Error("the STL file has no triangles".to_string()));
    }
    Ok(mesh)
}

fn push_polygon(mesh: &mut PolyMesh, corners: &[V3]) -> Result<(), Error> {
    if mesh.triangles.len() + corners.len() > limits::MAX_TRIANGLES {
        return Err(Error(format!("the file has more than {} triangles", limits::MAX_TRIANGLES)));
    }
    let base = mesh.positions.len() as u32;
    mesh.positions.extend_from_slice(corners);
    for k in 1..corners.len() as u32 - 1 {
        mesh.triangles.push([base, base + k, base + k + 1]);
    }
    Ok(())
}

// ----------------------------------------------------------------------
// В меш движка
// ----------------------------------------------------------------------

/// Меш для движка: плоские нормали; цвета — палитрой-текстурой, текстура `.ply` — как есть.
pub fn to_mesh(source: &PolyMesh, format: Format, options: &Options) -> Result<Imported, Error> {
    let mut builder = Builder::default();
    let mut sheets = Vec::new();
    let mut report = source.report.clone();
    let colored = source.face_colors.is_some() || source.vertex_colors.is_some();
    if let (Some(texture), Some(uvs)) = (&source.texture, &source.uvs) {
        sheets.push(Sheet { name: "texture".to_string(), image: texture.clone() });
        if colored {
            report.ignore("vertex colours (the texture is used)");
        }
        for t in &source.triangles {
            let points = t.map(|v| source.positions[v as usize]);
            let uv = t.map(|v| uvs[v as usize]);
            builder.polygon(&points, &uv, Slot::Texture(0))?;
        }
    } else if colored {
        // Палитра различных цветов: тексель на цвет.
        let mut index: HashMap<[u8; 4], u32> = HashMap::new();
        let mut list: Vec<[u8; 4]> = Vec::new();
        let mut color_of = |c: [u8; 4]| -> u32 {
            *index.entry(c).or_insert_with(|| {
                list.push(c);
                list.len() as u32 - 1
            })
        };
        let mut corner_colors: Vec<[u32; 3]> = Vec::with_capacity(source.triangles.len());
        for (i, t) in source.triangles.iter().enumerate() {
            corner_colors.push(match (&source.face_colors, &source.vertex_colors) {
                (Some(faces), _) => [color_of(faces[i]); 3],
                (None, Some(vertices)) => t.map(|v| color_of(vertices[v as usize])),
                _ => [0; 3],
            });
        }
        if list.len() > 1 << 20 {
            return Err(Error(format!("the mesh has {} different colours; at most {} are supported", list.len(), 1 << 20)));
        }
        let side = (list.len() as f64).sqrt().ceil().max(1.0) as u32;
        let rows = (list.len() as u32).div_ceil(side).max(1);
        let mut palette = RgbaImage::new(side, rows, [0; 4]);
        for (i, c) in list.iter().enumerate() {
            palette.set(i as u32 % side, i as u32 / side, *c);
        }
        let center = |i: u32| [((i % side) as f32 + 0.5) / side as f32, ((i / side) as f32 + 0.5) / rows as f32];
        for (t, colors) in source.triangles.iter().zip(&corner_colors) {
            let points = t.map(|v| source.positions[v as usize]);
            builder.polygon(&points, &colors.map(center), Slot::Texture(0))?;
        }
        sheets.push(Sheet { name: "colors".to_string(), image: palette });
    } else {
        for t in &source.triangles {
            let points = t.map(|v| source.positions[v as usize]);
            builder.polygon(&points, &[[0.0, 0.0]; 3], Slot::None)?;
        }
    }
    finish(builder, &sheets, format, [0.0; 3], options, report)
}
