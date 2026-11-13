//! Сеанс правки документа: история Undo / Redo, выделение, «изменён ли», буфер обмена.
//!
//! История хранит снимки документа целиком. Это дёшево: пиксели текстур лежат за `Arc`,
//! и снимок делит их с текущим документом, пока в текстуру не начнут рисовать, —
//! поэтому отмена возвращает документ байт в байт, а штрих кисти стоит одну копию
//! одной текстуры.

use std::sync::Mutex;

use crate::doc::{Document, FaceDir, Node, NodeId};
use crate::image::Image;
use crate::{ops, uv};

/// Сколько шагов помнит история.
pub const HISTORY_LIMIT: usize = 256;

#[derive(Clone, Debug)]
struct Snapshot {
    label: String,
    doc: Document,
    selection: Vec<NodeId>,
}

/// Документ, его история и выделение.
#[derive(Clone, Debug)]
pub struct Session {
    pub doc: Document,
    /// Выделенные узлы; первый — «главный» (его показывает панель Element).
    pub selection: Vec<NodeId>,
    /// Выделенная сторона (для UV-редактора).
    pub face: Option<FaceDir>,
    undo: Vec<Snapshot>,
    redo: Vec<Snapshot>,
    /// Начатая длинная правка (перетаскивание гизмо, штрих кисти).
    pending: Option<Snapshot>,
    /// Документ на момент последнего сохранения.
    saved: Document,
}

impl Session {
    pub fn new(doc: Document) -> Self {
        Self {
            saved: doc.clone(),
            doc,
            selection: Vec::new(),
            face: None,
            undo: Vec::new(),
            redo: Vec::new(),
            pending: None,
        }
    }

    /// Правка одним шагом истории: `f` меняет документ; если он не изменился, шаг не пишется.
    pub fn edit<R>(&mut self, label: &str, f: impl FnOnce(&mut Document) -> R) -> R {
        self.finish();
        let before = self.snapshot(label);
        let result = f(&mut self.doc);
        self.push_if_changed(before);
        result
    }

    /// Начать длинную правку: дальше документ меняется напрямую (каждый кадр
    /// перетаскивания), а в историю попадёт одним шагом при [`commit`](Self::commit).
    pub fn begin(&mut self, label: &str) {
        self.finish();
        self.pending = Some(self.snapshot(label));
    }

    /// Идёт ли длинная правка.
    pub fn in_progress(&self) -> bool {
        self.pending.is_some()
    }

    /// Документ до начала длинной правки (гизмо считает смещение от него).
    pub fn pending_base(&self) -> Option<&Document> {
        self.pending.as_ref().map(|s| &s.doc)
    }

    /// Закончить длинную правку.
    pub fn commit(&mut self) {
        if let Some(before) = self.pending.take() {
            self.push_if_changed(before);
        }
    }

    /// Отменить длинную правку: документ возвращается к её началу.
    pub fn cancel(&mut self) {
        if let Some(before) = self.pending.take() {
            self.doc = before.doc;
            self.selection = before.selection;
        }
    }

    fn finish(&mut self) {
        self.commit();
    }

    fn snapshot(&self, label: &str) -> Snapshot {
        Snapshot {
            label: label.to_string(),
            doc: self.doc.clone(),
            selection: self.selection.clone(),
        }
    }

    fn push_if_changed(&mut self, before: Snapshot) {
        if before.doc == self.doc {
            return;
        }
        self.undo.push(before);
        if self.undo.len() > HISTORY_LIMIT {
            self.undo.remove(0);
        }
        self.redo.clear();
        self.prune_selection();
    }

    pub fn can_undo(&self) -> bool {
        !self.undo.is_empty() || self.pending.is_some()
    }

    pub fn can_redo(&self) -> bool {
        !self.redo.is_empty()
    }

    /// Название шага, который отменит Undo.
    /// Названия шагов, которые отменит Undo, от последнего к первому.
    pub fn undo_labels(&self) -> Vec<&str> {
        self.undo.iter().rev().map(|s| s.label.as_str()).collect()
    }

    pub fn undo_label(&self) -> Option<&str> {
        self.undo.last().map(|s| s.label.as_str())
    }

    pub fn redo_label(&self) -> Option<&str> {
        self.redo.last().map(|s| s.label.as_str())
    }

    pub fn undo(&mut self) -> bool {
        self.finish();
        let Some(previous) = self.undo.pop() else { return false };
        let current = Snapshot {
            label: previous.label.clone(),
            doc: std::mem::replace(&mut self.doc, previous.doc),
            selection: std::mem::replace(&mut self.selection, previous.selection),
        };
        self.redo.push(current);
        self.prune_selection();
        true
    }

    pub fn redo(&mut self) -> bool {
        self.finish();
        let Some(next) = self.redo.pop() else { return false };
        let current = Snapshot {
            label: next.label.clone(),
            doc: std::mem::replace(&mut self.doc, next.doc),
            selection: std::mem::replace(&mut self.selection, next.selection),
        };
        self.undo.push(current);
        self.prune_selection();
        true
    }

    /// Есть ли несохранённые изменения.
    pub fn is_dirty(&self) -> bool {
        self.doc != self.saved
    }

    /// Отметить текущее состояние как сохранённое.
    pub fn mark_saved(&mut self) {
        self.saved = self.doc.clone();
    }

    /// Сколько шагов в истории (для строки состояния и тестов).
    pub fn history_len(&self) -> (usize, usize) {
        (self.undo.len(), self.redo.len())
    }

    fn prune_selection(&mut self) {
        let doc = &self.doc;
        self.selection.retain(|id| doc.node(*id).is_some());
    }

    // ------------------------------------------------------------------
    // Выделение
    // ------------------------------------------------------------------

    pub fn select(&mut self, id: Option<NodeId>) {
        self.selection = id.into_iter().collect();
        self.face = None;
    }

    /// Добавить к выделению или убрать из него (Ctrl + щелчок).
    pub fn toggle_selected(&mut self, id: NodeId) {
        if let Some(i) = self.selection.iter().position(|s| *s == id) {
            self.selection.remove(i);
        } else {
            self.selection.push(id);
        }
    }

    pub fn is_selected(&self, id: NodeId) -> bool {
        self.selection.contains(&id)
    }

    /// Главный выделенный узел.
    pub fn primary(&self) -> Option<NodeId> {
        self.selection.first().copied()
    }

    /// Выделенные элементы (группы — со всем содержимым).
    pub fn selected_elements(&self) -> Vec<NodeId> {
        ops::elements_of(&self.doc, &self.selection)
    }

    // ------------------------------------------------------------------
    // Копирование и вставка
    // ------------------------------------------------------------------

    /// Скопировать выделенное в буфер процесса (вставляется и в другой документ).
    pub fn copy(&self) -> bool {
        let Some(fragment) = Fragment::from_selection(&self.doc, &self.selection) else {
            return false;
        };
        *CLIPBOARD.lock().unwrap_or_else(|p| p.into_inner()) = Some(fragment);
        true
    }

    /// Вставить из буфера; вставленное становится выделением.
    pub fn paste(&mut self) -> bool {
        let fragment = CLIPBOARD.lock().unwrap_or_else(|p| p.into_inner()).clone();
        let Some(fragment) = fragment else { return false };
        let parent = self
            .primary()
            .and_then(|id| self.doc.node(id))
            .and_then(|n| if n.is_group() { Some(n.id) } else { n.parent });
        let created = self.edit("Paste", |doc| fragment.paste_into(doc, parent));
        self.selection = created;
        true
    }
}

static CLIPBOARD: Mutex<Option<Fragment>> = Mutex::new(None);

/// Есть ли что вставить.
pub fn clipboard_has_content() -> bool {
    CLIPBOARD.lock().unwrap_or_else(|p| p.into_inner()).is_some()
}

/// Кусок модели в буфере обмена: узлы и картинки развёрток их элементов.
#[derive(Clone, Debug)]
pub struct Fragment {
    nodes: Vec<Node>,
    /// Для элементов с развёрткой коробкой — их картинка.
    patches: Vec<(NodeId, Image)>,
    density: f32,
}

impl Fragment {
    pub fn from_selection(doc: &Document, selection: &[NodeId]) -> Option<Self> {
        let roots = ops::top_level(doc, selection);
        if roots.is_empty() {
            return None;
        }
        let mut ids = Vec::new();
        for root in &roots {
            ids.push(*root);
            ids.extend(doc.descendants(*root));
        }
        // Порядок — как в документе, корни фрагмента — без родителя.
        let nodes: Vec<Node> = doc
            .nodes
            .iter()
            .filter(|n| ids.contains(&n.id))
            .map(|n| {
                let mut n = n.clone();
                if roots.contains(&n.id) {
                    n.parent = None;
                }
                n
            })
            .collect();
        let patches = nodes
            .iter()
            .filter_map(|n| {
                let e = n.element()?;
                if !e.box_uv {
                    return None;
                }
                let [w, h] = uv::footprint(e, doc.texel_density);
                let texture = doc.textures.get(uv::element_texture(e) as usize)?;
                Some((n.id, texture.image.crop(e.uv_origin[0] as i32, e.uv_origin[1] as i32, w, h)))
            })
            .collect();
        Some(Self {
            nodes,
            patches,
            density: doc.texel_density,
        })
    }

    /// Вставить в документ под `parent`; возвращает новые корни.
    pub fn paste_into(&self, doc: &mut Document, parent: Option<NodeId>) -> Vec<NodeId> {
        let mapping: Vec<(NodeId, NodeId)> = self.nodes.iter().map(|n| (n.id, doc.allocate_id())).collect();
        let map = |id: NodeId| mapping.iter().find(|(o, _)| *o == id).map(|(_, n)| *n);
        let mut roots = Vec::new();
        let texture_count = doc.textures.len() as u32;
        for node in &self.nodes {
            let mut copy = node.clone();
            copy.id = map(node.id).expect("mapped");
            copy.parent = match node.parent {
                Some(p) => map(p),
                None => {
                    roots.push(copy.id);
                    parent
                }
            };
            copy.name = doc.unique_name(&node.name);
            if let Some(e) = copy.element_mut() {
                e.twin = e.twin.and_then(map);
                for face in &mut e.faces {
                    face.texture = face.texture.min(texture_count.saturating_sub(1));
                }
            }
            doc.nodes.push(copy);
        }
        let density_matches = (self.density - doc.texel_density).abs() < 1e-6;
        for (old, patch) in &self.patches {
            let Some(new) = map(*old) else { continue };
            if uv::place(doc, new) {
                let Some(e) = doc.element(new).cloned() else { continue };
                if let Some(tex) = doc.textures.get_mut(uv::element_texture(&e) as usize) {
                    if density_matches {
                        tex.image.blit(patch, 0, 0, e.uv_origin[0] as i32, e.uv_origin[1] as i32, patch.width(), patch.height());
                    }
                }
                if !density_matches {
                    uv::fill_faces(doc, new, ops::BASE_COLOR);
                }
            }
        }
        // Близнецы без развёртки снова берут её у оригинала.
        let ids: Vec<NodeId> = mapping.iter().map(|(_, n)| *n).collect();
        ops::sync_twins(doc, &ids);
        roots
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::doc::Element;
    use glam::Vec3;

    #[test]
    fn undo_redo_and_dirty_flag() {
        let mut s = Session::new(Document::new("t"));
        assert!(!s.is_dirty() && !s.can_undo());
        let id = s.edit("Add", |doc| ops::add_element(doc, "cube", Element::cuboid(Vec3::ZERO, Vec3::ONE * 4.0), None));
        assert!(s.is_dirty());
        s.select(Some(id));
        s.begin("Move");
        ops::move_by(&mut s.doc, &[id], Vec3::X);
        ops::move_by(&mut s.doc, &[id], Vec3::X);
        s.commit();
        assert_eq!(s.history_len(), (2, 0), "a long edit is one step");
        assert_eq!(s.undo_label(), Some("Move"));
        assert!(s.undo());
        assert_eq!(s.doc.element(id).unwrap().from, Vec3::ZERO);
        assert!(s.redo());
        assert_eq!(s.doc.element(id).unwrap().from.x, 2.0);
        s.mark_saved();
        assert!(!s.is_dirty());
        // Правка без изменений не пишется в историю.
        s.edit("Nothing", |_| ());
        assert_eq!(s.history_len(), (2, 0));
        s.undo();
        s.undo();
        assert!(s.doc.nodes.is_empty());
        assert!(s.selection.is_empty(), "the selection of a deleted node is dropped");
    }

    #[test]
    fn copy_paste_between_documents_keeps_painting() {
        let mut a = Session::new(Document::new("a"));
        let id = a.edit("Add", |doc| ops::add_element(doc, "box", Element::cuboid(Vec3::ZERO, Vec3::new(3.0, 3.0, 3.0)), None));
        let up = a.doc.element(id).unwrap().face(FaceDir::Up).uv;
        a.edit("Paint", |doc| doc.textures[0].image.set(up.x as i32 + 1, up.y as i32 + 1, [9, 8, 7, 255]));
        a.select(Some(id));
        assert!(a.copy());

        let mut b = Session::new(Document::new("b"));
        b.edit("Add", |doc| ops::add_element(doc, "other", Element::cuboid(Vec3::ZERO, Vec3::new(8.0, 8.0, 8.0)), None));
        assert!(b.paste());
        let pasted = b.selection[0];
        assert_eq!(b.doc.node(pasted).unwrap().name, "box");
        let up = b.doc.element(pasted).unwrap().face(FaceDir::Up).uv;
        assert_eq!(b.doc.textures[0].image.get(up.x as i32 + 1, up.y as i32 + 1), Some([9, 8, 7, 255]));
        assert!(b.undo());
        assert!(b.doc.node(pasted).is_none());
    }
}
