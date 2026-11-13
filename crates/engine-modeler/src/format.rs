//! Формат `.pmodel`: один текстовый файл RON, текстуры внутри (PNG в base64).
//!
//! ```text
//! (
//!     format: "piska-model",
//!     version: 1,
//!     model: (
//!         name: "crate",
//!         units_per_metre: 16.0,
//!         texel_density: 1.0,
//!         textures: [(name: "atlas", image: "iVBORw0KGgo…")],
//!         nodes: [
//!             (id: 1, name: "body", kind: Element((shape: Cuboid, from: (-8.0, 0.0, -8.0), …))),
//!         ],
//!         next_id: 2,
//!     ),
//! )
//! ```
//!
//! Почему текстуры внутри: файл самодостаточен — его можно перенести, положить в пак
//! ассетов (веб, Android) и перезагрузить в редакторе одним событием «файл изменился».
//!
//! В конце файла — раздел `baked`: та же модель, «запечённая» для движка ([`crate::bake`]):
//! вершины в метрах, индексы и общий атлас. Он пишется при каждом сохранении и только
//! читается движком (`engine-assets`), поэтому движку не нужен код редактора моделей.
//! При чтении документа раздел пропускается.

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::doc::Document;
use crate::ModelError;

/// Метка формата в начале файла.
pub const FORMAT: &str = "piska-model";
/// Версия, которую пишет эта программа.
pub const VERSION: u32 = 1;
/// Расширение файла.
pub const EXTENSION: &str = "pmodel";

#[derive(Serialize, Deserialize)]
struct ModelFile {
    format: String,
    version: u32,
    model: Document,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    baked: Option<BakedSection>,
}

/// Раздел `baked`: плоские массивы (x, y, z, x, y, z…), атлас — PNG в base64.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct BakedSection {
    pub positions: Vec<f32>,
    pub normals: Vec<f32>,
    pub uvs: Vec<f32>,
    pub indices: Vec<u32>,
    #[serde(default)]
    pub texture: Option<String>,
}

impl BakedSection {
    pub fn from_baked(baked: &crate::BakedModel) -> Self {
        Self {
            positions: baked.positions.iter().flatten().copied().collect(),
            normals: baked.normals.iter().flatten().copied().collect(),
            uvs: baked.uvs.iter().flatten().copied().collect(),
            indices: baked.indices.clone(),
            texture: baked.texture_png().map(|png| crate::image::base64_encode(&png)),
        }
    }
}

/// Раздел `baked` файла (как его прочтёт движок).
pub fn read_baked(text: &str) -> Result<BakedSection, ModelError> {
    #[derive(Deserialize)]
    struct Probe {
        #[serde(default)]
        baked: Option<BakedSection>,
    }
    let probe: Probe = ron::from_str(text).map_err(|e| ModelError::Format(format!("damaged .pmodel: {e}")))?;
    probe
        .baked
        .ok_or_else(|| ModelError::Format("the file has no `baked` section; open and save it in Piska Modeler".to_string()))
}

/// Документ → текст `.pmodel`.
pub fn to_string(doc: &Document) -> String {
    let file = ModelFile {
        format: FORMAT.to_string(),
        version: VERSION,
        model: doc.clone(),
        baked: Some(BakedSection::from_baked(&crate::bake(doc))),
    };
    let config = ron::ser::PrettyConfig::new()
        .depth_limit(6)
        .struct_names(false)
        .compact_arrays(true)
        // Одинаковые байты на всех ОС (иначе на Windows переводы строк были бы CRLF).
        .new_line("\n".to_string());
    ron::ser::to_string_pretty(&file, config).expect("a model is always serializable")
}

/// Текст `.pmodel` → документ.
pub fn from_str(text: &str) -> Result<Document, ModelError> {
    // Сначала заголовок: понятная ошибка для чужого файла и файла новой версии.
    if let Ok(header) = ron::from_str::<HeaderProbe>(text) {
        if header.format != FORMAT {
            return Err(ModelError::Format(format!(
                "this is not a Piska model (format `{}`, expected `{FORMAT}`)",
                header.format
            )));
        }
        if header.version > VERSION {
            return Err(ModelError::Format(format!(
                "the model was saved by a newer Piska Modeler (format version {}, this one reads up to {VERSION}); update the engine",
                header.version
            )));
        }
    }
    let file: ModelFile = ron::from_str(text).map_err(|e| ModelError::Format(format!("damaged .pmodel: {e}")))?;
    let mut doc = file.model;
    repair(&mut doc);
    Ok(doc)
}

/// Заголовок, читаемый без остальных полей.
#[derive(Deserialize)]
struct HeaderProbe {
    format: String,
    version: u32,
    #[serde(rename = "model")]
    _model: ron::Value,
}

/// Поправить то, что могло испортиться при ручной правке файла: ссылки на несуществующих
/// родителей и близнецов, повторы идентификаторов, номера текстур, счётчик идентификаторов.
fn repair(doc: &mut Document) {
    if doc.textures.is_empty() {
        doc.textures = Document::new("").textures;
    }
    let mut seen = std::collections::HashSet::new();
    doc.nodes.retain(|n| seen.insert(n.id));
    let ids: std::collections::HashSet<_> = doc.nodes.iter().map(|n| n.id).collect();
    let groups: std::collections::HashSet<_> = doc.nodes.iter().filter(|n| n.is_group()).map(|n| n.id).collect();
    let texture_count = doc.textures.len() as u32;
    for node in &mut doc.nodes {
        if node.parent.is_some_and(|p| !groups.contains(&p) || p == node.id) {
            node.parent = None;
        }
        if let Some(e) = node.element_mut() {
            if e.twin.is_some_and(|t| !ids.contains(&t)) {
                e.twin = None;
            }
            for face in &mut e.faces {
                if face.texture >= texture_count {
                    face.texture = 0;
                }
            }
        }
    }
    // Циклы родителей (A в B, B в A) разрываются.
    let parents: Vec<(crate::doc::NodeId, Option<crate::doc::NodeId>)> =
        doc.nodes.iter().map(|n| (n.id, n.parent)).collect();
    for (id, _) in &parents {
        let mut current = parents.iter().find(|(n, _)| n == id).and_then(|(_, p)| *p);
        let mut steps = 0;
        while let Some(p) = current {
            if p == *id || steps > parents.len() {
                if let Some(node) = doc.node_mut(*id) {
                    node.parent = None;
                }
                break;
            }
            current = parents.iter().find(|(n, _)| *n == p).and_then(|(_, p)| *p);
            steps += 1;
        }
    }
    let max = doc.nodes.iter().map(|n| n.id.0).max().unwrap_or(0);
    doc.next_id = doc.next_id.max(max + 1);
}

/// Прочитать файл.
pub fn load(path: impl AsRef<Path>) -> Result<Document, ModelError> {
    let path = path.as_ref();
    let text = std::fs::read_to_string(path).map_err(|source| ModelError::Io {
        path: path.display().to_string(),
        source,
    })?;
    from_str(&text).map_err(|e| match e {
        ModelError::Format(reason) => ModelError::Format(format!("{}: {reason}", path.display())),
        other => other,
    })
}

/// Записать файл (через временный рядом: оборванная запись не портит старый файл).
pub fn save(doc: &Document, path: impl AsRef<Path>) -> Result<(), ModelError> {
    let path = path.as_ref();
    let io = |source| ModelError::Io {
        path: path.display().to_string(),
        source,
    };
    if let Some(dir) = path.parent().filter(|d| !d.as_os_str().is_empty()) {
        std::fs::create_dir_all(dir).map_err(io)?;
    }
    let temp = path.with_extension("pmodel.tmp");
    std::fs::write(&temp, to_string(doc)).map_err(io)?;
    std::fs::rename(&temp, path).map_err(io)
}

/// Файл восстановления для документа: во временном каталоге ОС (не в каталоге ассетов,
/// чтобы база ассетов его не подхватила). `path` — файл документа или `None` для нового.
pub fn recovery_path(path: Option<&Path>, name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join("piska-modeler").join("recovery");
    let key = match path {
        Some(p) => {
            // Короткий устойчивый хэш пути (FNV-1a): разные файлы с одним именем не путаются.
            let mut hash: u64 = 0xcbf29ce484222325;
            for b in p.to_string_lossy().bytes() {
                hash = (hash ^ b as u64).wrapping_mul(0x100000001b3);
            }
            format!("{name}-{hash:016x}")
        }
        None => format!("{name}-unsaved"),
    };
    let safe: String = key
        .chars()
        .map(|c| if c.is_alphanumeric() || c == '-' || c == '_' { c } else { '_' })
        .collect();
    dir.join(format!("{safe}.{EXTENSION}"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::doc::{Element, FaceDir};
    use crate::ops;
    use glam::Vec3;

    #[test]
    fn file_round_trip_is_exact() {
        let mut doc = Document::new("crate");
        let group = ops::add_group(&mut doc, "lid", Vec3::new(0.0, 8.0, 0.0), None);
        let id = ops::add_element(&mut doc, "body", Element::cuboid(Vec3::splat(-4.0), Vec3::splat(8.0)), Some(group));
        doc.element_mut(id).unwrap().rotation = Vec3::new(0.0, 22.5, -5.0);
        doc.element_mut(id).unwrap().face_mut(FaceDir::Up).emissive = true;
        ops::mirror_x(&mut doc, &[id]);
        ops::add_texture(&mut doc, "extra", 16, 8);
        doc.textures[0].image.set(3, 4, [1, 2, 3, 4]);
        let text = to_string(&doc);
        assert!(text.starts_with("(\n    format: \"piska-model\",\n    version: 1,"), "{}", &text[..80]);
        let back = from_str(&text).unwrap();
        assert_eq!(back, doc);
        assert_eq!(to_string(&back), text, "saving again gives the same bytes");
        // Запечённый раздел для движка совпадает с запеканием документа (числа — без потерь).
        let baked = read_baked(&text).unwrap();
        assert_eq!(baked, BakedSection::from_baked(&crate::bake(&doc)));
        assert_eq!(baked.indices.len(), 2 * 36);
    }

    #[test]
    fn errors_explain_what_is_wrong() {
        let err = from_str("(format: \"other\", version: 1, model: ())").unwrap_err().to_string();
        assert!(err.contains("not a Piska model"), "{err}");
        let err = from_str("(format: \"piska-model\", version: 99, model: ())").unwrap_err().to_string();
        assert!(err.contains("newer Piska Modeler"), "{err}");
        let err = from_str("(format: \"piska-model\", version: 1, model: (name: 5))").unwrap_err().to_string();
        assert!(err.contains("damaged .pmodel"), "{err}");
    }

    #[test]
    fn broken_links_are_repaired_on_load() {
        let mut doc = Document::new("t");
        let id = ops::add_element(&mut doc, "a", Element::cuboid(Vec3::ZERO, Vec3::ONE), None);
        doc.node_mut(id).unwrap().parent = Some(crate::doc::NodeId(77));
        doc.element_mut(id).unwrap().twin = Some(crate::doc::NodeId(78));
        doc.element_mut(id).unwrap().faces[0].texture = 9;
        doc.next_id = 0;
        let back = from_str(&to_string(&doc)).unwrap();
        let node = back.node(id).unwrap();
        assert_eq!(node.parent, None);
        assert_eq!(node.element().unwrap().twin, None);
        assert_eq!(node.element().unwrap().faces[0].texture, 0);
        assert!(back.next_id > id.0);
    }
}
