//! Piska Modeler — простой редактор низкополигональных моделей: кубоиды и простые формы,
//! группы с точками вращения, пиксельная текстура с автоматической развёрткой, рисование
//! прямо по модели. Формат `.pmodel` движок импортирует как меш с текстурой.
//!
//! Библиотека без интерфейса: документ ([`Document`]), операции ([`ops`]), история
//! ([`Session`]), сборка меша ([`geometry`]), развёртка ([`uv`]), рисование ([`paint`]),
//! камера и луч ([`camera`]), программный растеризатор окна ([`raster`]), файл
//! ([`format`]), запекание для движка ([`bake`]), экспорт ([`export`]), импорт опорных
//! мешей ([`import`]), образцы ([`samples`]). Интерфейс на egui — модуль `ui` (фича `ui`).
//!
//! ```
//! use engine_modeler::{ops, Document, Element, Session};
//! use glam::Vec3;
//!
//! let mut session = Session::new(Document::new("box"));
//! let id = session.edit("Add Cuboid", |doc| {
//!     ops::add_element(doc, "body", Element::cuboid(Vec3::splat(-8.0), Vec3::splat(16.0)), None)
//! });
//! session.edit("Grow", |doc| ops::resize_face(doc, id, engine_modeler::FaceDir::Up, 4.0));
//! assert_eq!(session.doc.element(id).unwrap().size.y, 20.0);
//! session.undo();
//! assert_eq!(session.doc.element(id).unwrap().size.y, 16.0);
//! let text = engine_modeler::format::to_string(&session.doc);
//! assert_eq!(engine_modeler::format::from_str(&text).unwrap(), session.doc);
//! ```
#![forbid(unsafe_code)]

pub mod bake;
pub mod camera;
pub mod doc;
pub mod export;
pub mod format;
pub mod geometry;
pub mod image;
pub mod import;
pub mod interchange;
pub mod mesh_edit;
pub mod mesh_ops;
pub mod ops;
pub mod paint;
pub mod raster;
pub mod samples;
pub mod session;
pub mod uv;
pub mod uv_ops;

#[cfg(feature = "ui")]
pub mod ui;

#[cfg(feature = "gpu")]
pub mod gpu;

pub use bake::{bake, BakedModel};
pub use camera::{Camera, Hit, Ray, ViewPreset};
pub use doc::{Document, Element, Face, FaceDir, Group, MeshFace, MeshShape, Node, NodeId, NodeKind, ReferenceMesh, Shape, Texture, UvRect};
pub use geometry::{build_mesh, BuildOptions, BuiltMesh};
pub use image::{Image, Rgba};
pub use session::Session;

/// Ошибки библиотеки: сообщение говорит, что сделать.
#[derive(Debug, thiserror::Error)]
pub enum ModelError {
    #[error("cannot access `{path}`: {source}")]
    Io {
        path: String,
        #[source]
        source: std::io::Error,
    },
    #[error("{0}")]
    Format(String),
}

/// Прочитать `.pmodel` из байтов и запечь для движка (меш в метрах + атлас).
pub fn bake_bytes(bytes: &[u8]) -> Result<BakedModel, ModelError> {
    let text = std::str::from_utf8(bytes).map_err(|_| ModelError::Format("a .pmodel file must be UTF-8 text".to_string()))?;
    Ok(bake(&format::from_str(text)?))
}
