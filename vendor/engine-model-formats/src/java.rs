//! Модель блока / предмета Minecraft Java (`.json`): `elements` с `from` / `to`,
//! `rotation {origin, axis, angle, rescale}`, стороны `{uv, texture: "#имя", rotation}`,
//! словарь `textures` (переменные `#имя` разрешаются по цепочке), `parent` — ищется рядом
//! с моделью (`<parent>.json`); не найденный родитель попадает в отчёт.
//!
//! UV у Minecraft всегда в пространстве 16×16, какого бы размера ни была картинка;
//! сторона без `uv` получает UV по положению элемента (как в игре). Текстура `ns:block/x`
//! ищется как `block/x.png` рядом с моделью, в `../textures/` и `textures/`. Модель
//! сдвигается на (−8, 0, −8): блок стоит в начале координат по центру.

use std::collections::HashMap;

use serde_json::{Map, Value};

use crate::image::decode_png;
use crate::json::{self, boolean, number, numbers, short, string};
use crate::model::{stem, Cube, CubeFace, Model, Node, Report, Side, Texture};
use crate::{limits, safe_relative_path, Error, Format, Resolver};

/// Похож ли JSON на модель Minecraft: объект с `elements` из объектов с `from` и `to`,
/// или с `parent` и `textures`. Дёшево отсекает файлы данных.
pub fn looks_like_java_model(bytes: &[u8]) -> bool {
    let Ok(Value::Object(root)) = json::parse(bytes, "") else { return false };
    if let Some(Value::Array(elements)) = root.get("elements") {
        return !elements.is_empty()
            && elements.iter().all(|e| e.as_object().is_some_and(|e| e.contains_key("from") && e.contains_key("to")));
    }
    root.get("parent").is_some_and(Value::is_string) && root.get("textures").is_some_and(Value::is_object)
}

/// Прочитать модель Minecraft. `resolve` — чтение текстур и родителей рядом с моделью.
pub fn read(name: &str, bytes: &[u8], resolve: Resolver) -> Result<Model, Error> {
    let root = json::parse(bytes, "Minecraft model (.json)")?;
    let mut root = root.as_object().cloned().ok_or_else(|| Error("not a Minecraft model: the file is not a JSON object".to_string()))?;
    let mut report = Report::default();

    // Цепочка родителей: текстуры наследуются (свои важнее), элементы — от первого, у кого есть.
    let mut textures_map: Map<String, Value> = root.get("textures").and_then(Value::as_object).cloned().unwrap_or_default();
    let mut parent = string(&root, "parent").map(str::to_string);
    let mut depth = 0;
    while let Some(reference) = parent.take() {
        depth += 1;
        if depth > 8 {
            report.warn("the `parent` chain is longer than 8 models; the rest is ignored");
            break;
        }
        let path = strip_namespace(&reference);
        let found = [format!("{path}.json"), format!("{}.json", path.rsplit('/').next().unwrap_or(path))]
            .iter()
            .filter_map(|p| safe_relative_path(p, false))
            .find_map(|p| resolve(&p));
        let Some(bytes) = found else {
            if !root.contains_key("elements") {
                return Err(Error(format!(
                    "the model has no `elements` of its own and its parent `{}` is not next to it; \
                     put `{path}.json` beside the model or copy the parent's elements into it",
                    short(&reference)
                )));
            }
            if !reference.starts_with("builtin/") && !reference.ends_with("block/block") {
                report.ignore(format!("parent `{}` (not found; its textures and display are not inherited)", short(&reference)));
            }
            break;
        };
        let parent_root = json::parse(&bytes, "parent model")?;
        let parent_root = parent_root.as_object().ok_or_else(|| Error(format!("parent `{}` is not a JSON object", short(&reference))))?;
        if let Some(parent_textures) = parent_root.get("textures").and_then(Value::as_object) {
            for (k, v) in parent_textures {
                textures_map.entry(k.clone()).or_insert_with(|| v.clone());
            }
        }
        if !root.contains_key("elements") {
            if let Some(elements) = parent_root.get("elements") {
                root.insert("elements".to_string(), elements.clone());
            }
        }
        parent = string(parent_root, "parent").map(str::to_string);
    }

    let elements = match root.get("elements") {
        Some(Value::Array(list)) => list.clone(),
        Some(_) => return Err(Error("`elements` must be a list".to_string())),
        None => {
            return Err(Error(
                "the model has no `elements` (generated item models and built-in models have no geometry to import)".to_string(),
            ))
        }
    };
    if elements.len() > limits::MAX_ELEMENTS {
        return Err(Error(format!("the model has {} elements; at most {} are supported", elements.len(), limits::MAX_ELEMENTS)));
    }

    // Текстуры: переменная → путь → номер картинки.
    let mut textures: Vec<Texture> = Vec::new();
    let mut by_path: HashMap<String, usize> = HashMap::new();
    let mut by_variable: HashMap<String, Option<usize>> = HashMap::new();
    let mut texture_for = |variable: &str, report: &mut Report| -> Option<usize> {
        if let Some(found) = by_variable.get(variable) {
            return *found;
        }
        let result = resolve_variable(variable, &textures_map).and_then(|path| {
            if let Some(i) = by_path.get(&path) {
                return Some(*i);
            }
            if textures.len() >= limits::MAX_TEXTURES {
                return None;
            }
            let texture = load_texture(&path, resolve, report);
            textures.push(texture);
            by_path.insert(path, textures.len() - 1);
            Some(textures.len() - 1)
        });
        if result.is_none() {
            report.warn(format!("texture variable `{}` is not defined; its faces are white", short(variable)));
        }
        by_variable.insert(variable.to_string(), result);
        result
    };

    let mut roots = Vec::with_capacity(elements.len());
    for (index, value) in elements.iter().enumerate() {
        let object = value.as_object().ok_or_else(|| Error(format!("element #{index} is not an object")))?;
        let owner = || format!("element #{index}");
        let from = numbers::<3>(object, "from", None, &owner)?;
        let to = numbers::<3>(object, "to", None, &owner)?;
        let (mut origin, mut rotation, mut rescale) = ([8.0; 3], [0.0; 3], false);
        if let Some(r) = object.get("rotation").and_then(Value::as_object) {
            origin = numbers::<3>(r, "origin", Some([8.0; 3]), &owner)?;
            let angle = r.get("angle").map(|a| number(a, &|| format!("{}: rotation angle", owner()))).transpose()?.unwrap_or(0.0);
            match string(r, "axis") {
                Some("x") => rotation[0] = angle,
                Some("y") => rotation[1] = angle,
                Some("z") => rotation[2] = angle,
                other => return Err(Error(format!("{}: rotation axis `{}` must be x, y or z", owner(), short(other.unwrap_or(""))))),
            }
            rescale = boolean(r, "rescale", false);
        }
        let mut faces = [None; 6];
        if let Some(face_map) = object.get("faces").and_then(Value::as_object) {
            for side in Side::ALL {
                let Some(face) = face_map.get(side.key()).and_then(Value::as_object) else { continue };
                let face_owner = || format!("{} face `{}`", owner(), side.key());
                let uv = numbers::<4>(face, "uv", Some(default_uv(side, from, to)), &face_owner)?;
                let texture = match string(face, "texture") {
                    Some(variable) => texture_for(variable, &mut report),
                    None => None,
                };
                let degrees = face.get("rotation").map(|r| number(r, &|| format!("{}: `rotation`", face_owner()))).transpose()?.unwrap_or(0.0);
                if degrees.rem_euclid(90.0) != 0.0 {
                    report.warn(format!("{}: face rotation {degrees}° is not a multiple of 90°; rounded", face_owner()));
                }
                let rotation = (degrees / 90.0).round().rem_euclid(4.0) as u8;
                faces[side.index()] = Some(CubeFace { uv, rotation, texture });
            }
        }
        if object.contains_key("shade") || object.contains_key("light_emission") {
            report.ignore("element shading flags (`shade`, `light_emission`)");
        }
        roots.push(Node::Cube(Cube {
            name: string(object, "name").map_or_else(|| format!("element {index}"), str::to_string),
            uuid: String::new(),
            from,
            to,
            origin,
            rotation,
            inflate: 0.0,
            rescale,
            faces,
            box_uv: None,
            visible: true,
            locked: false,
        }));
    }
    for (key, what) in [("display", "display transforms (`display`)"), ("overrides", "item overrides"), ("gui_light", "`gui_light`"), ("ambientocclusion", "`ambientocclusion`")] {
        if root.contains_key(key) {
            report.ignore(what);
        }
    }
    Ok(Model { name: stem(name).to_string(), format: Format::JavaModel, textures, roots, offset: [-8.0, 0.0, -8.0], report })
}

fn strip_namespace(reference: &str) -> &str {
    reference.split_once(':').map_or(reference, |(_, path)| path)
}

/// `#имя` → путь картинки по словарю (цепочка ссылок не длиннее 16).
fn resolve_variable(variable: &str, map: &Map<String, Value>) -> Option<String> {
    let mut current = variable.to_string();
    for _ in 0..16 {
        let key = current.strip_prefix('#')?;
        let value = map.get(key)?.as_str()?;
        if !value.starts_with('#') {
            return Some(strip_namespace(value).to_string());
        }
        current = value.to_string();
    }
    None
}

/// Найти и прочитать картинку текстуры; не нашлась — шахматка и запись в отчёт.
fn load_texture(path: &str, resolve: Resolver, report: &mut Report) -> Texture {
    let last = path.rsplit('/').next().unwrap_or(path);
    let candidates: Vec<String> = [
        format!("{path}.png"),
        format!("../textures/{path}.png"),
        format!("../../textures/{path}.png"),
        format!("textures/{path}.png"),
        format!("{last}.png"),
    ]
    .iter()
    .filter_map(|p| safe_relative_path(p, true))
    .collect();
    let mut texture = Texture { name: path.to_string(), image: None, uv_size: [16.0, 16.0], path: None };
    match candidates.iter().find_map(|p| resolve(p).map(|b| (p, b))) {
        Some((p, bytes)) => {
            texture.path = Some(p.clone());
            match decode_png(&bytes) {
                Ok(image) => {
                    // Анимированная текстура (кадры-квадраты столбиком): кадр 0.
                    if image.height > image.width && image.height % image.width == 0 {
                        report.warn(format!("texture `{}` is animated ({} frames); frame 0 is used", short(path), image.height / image.width));
                        texture.image = Some(image.crop(0, 0, image.width, image.width));
                    } else {
                        texture.image = Some(image);
                    }
                }
                Err(e) => report.warn(format!("texture `{}`: `{p}` is not a readable PNG ({e}); a checkerboard is used", short(path))),
            }
        }
        None => report.warn(format!("texture `{}` was not found (looked for `{}`); a checkerboard is used", short(path), candidates.first().map_or("", String::as_str))),
    }
    texture
}

/// UV стороны без `uv` — по положению элемента, как в Minecraft.
pub fn default_uv(side: Side, from: [f32; 3], to: [f32; 3]) -> [f32; 4] {
    match side {
        Side::North => [16.0 - to[0], 16.0 - to[1], 16.0 - from[0], 16.0 - from[1]],
        Side::South => [from[0], 16.0 - to[1], to[0], 16.0 - from[1]],
        Side::East => [16.0 - to[2], 16.0 - to[1], 16.0 - from[2], 16.0 - from[1]],
        Side::West => [from[2], 16.0 - to[1], to[2], 16.0 - from[1]],
        Side::Up => [from[0], from[2], to[0], to[2]],
        Side::Down => [from[0], 16.0 - to[2], to[0], 16.0 - from[2]],
    }
}

/// Пути текстур модели (как их назовёт [`read`]), для отслеживания изменений.
pub fn dependencies(bytes: &[u8]) -> Vec<String> {
    let Ok(Value::Object(root)) = json::parse(bytes, "") else { return Vec::new() };
    let Some(map) = root.get("textures").and_then(Value::as_object) else { return Vec::new() };
    let mut out: Vec<String> = map
        .values()
        .filter_map(Value::as_str)
        .filter(|v| !v.starts_with('#'))
        .filter_map(|v| safe_relative_path(&format!("{}.png", strip_namespace(v)), true))
        .collect();
    out.sort();
    out.dedup();
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_uvs_follow_the_element_position() {
        let (from, to) = ([2.0, 3.0, 4.0], [10.0, 12.0, 14.0]);
        assert_eq!(default_uv(Side::Up, from, to), [2.0, 4.0, 10.0, 14.0]);
        assert_eq!(default_uv(Side::North, from, to), [6.0, 4.0, 14.0, 13.0]);
        assert_eq!(resolve_variable("#side", serde_json::json!({"all": "minecraft:block/stone", "side": "#all"}).as_object().unwrap()).as_deref(), Some("block/stone"));
        let looping = serde_json::json!({"a": "#b", "b": "#a"});
        assert_eq!(resolve_variable("#a", looping.as_object().unwrap()), None, "reference loops end");
    }
}
