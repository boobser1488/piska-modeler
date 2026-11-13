//! Outliner: дерево групп и элементов — выбор, переименование, перетаскивание
//! (порядок и вложенность), видимость и замок.

use std::collections::HashSet;

use egui::{pos2, vec2, Color32, Rect, Sense, Stroke, Ui};
use engine_i18n::{tr, trf};

use super::icons::{self, Icon};
use super::style::{self, Palette};
use super::Command;
use crate::doc::{NodeId, NodeKind, Shape};
use crate::ops;
use crate::session::Session;

const ROW: f32 = 22.0;
const INDENT: f32 = 14.0;

pub(super) fn row_id(id: NodeId) -> egui::Id {
    egui::Id::new(("modeler.outliner.row", id.0))
}

#[derive(Default)]
pub(super) struct OutlinerState {
    renaming: Option<(NodeId, String, bool)>,
    collapsed: HashSet<NodeId>,
    dragging: Option<NodeId>,
}

/// Куда бросить перетаскиваемый узел.
#[derive(Clone, Copy, Debug, PartialEq)]
enum Drop {
    Before(NodeId),
    After(NodeId),
    Into(NodeId),
    Root,
}

impl OutlinerState {
    pub fn show(&mut self, ui: &mut Ui, session: &mut Session) -> Option<Command> {
        let p = Palette::from_ctx(ui.ctx());
        let mut command = None;
        if session.doc.nodes.is_empty() {
            ui.add_space(10.0);
            ui.vertical_centered(|ui| {
                ui.label(egui::RichText::new(tr("No elements yet")).color(p.text_muted));
                ui.label(egui::RichText::new(tr("Add a cuboid from the toolbar")).size(11.5).color(p.text_faint));
            });
            ui.add_space(10.0);
            return None;
        }
        // Строки по порядку дерева.
        let mut rows: Vec<(NodeId, usize)> = Vec::new();
        fn walk(session: &Session, collapsed: &HashSet<NodeId>, parent: Option<NodeId>, depth: usize, out: &mut Vec<(NodeId, usize)>) {
            for node in session.doc.children(parent) {
                out.push((node.id, depth));
                if node.is_group() && !collapsed.contains(&node.id) {
                    walk(session, collapsed, Some(node.id), depth + 1, out);
                }
            }
        }
        walk(session, &self.collapsed, None, 0, &mut rows);

        let pointer = ui.input(|i| i.pointer.hover_pos());
        let released = ui.input(|i| i.pointer.any_released());
        let modifiers = ui.input(|i| i.modifiers);
        let mut drop: Option<Drop> = None;
        let mut rects = Vec::new();
        ui.add_space(2.0);
        for (index, (id, depth)) in rows.iter().enumerate() {
            let Some(node) = session.doc.node(*id).cloned() else { continue };
            let width = ui.available_width();
            let (rect, _) = ui.allocate_exact_size(vec2(width, ROW), Sense::hover());
            rects.push((*id, rect));
            let response = ui.interact(rect, row_id(*id), Sense::click_and_drag());
            let selected = session.is_selected(*id);
            let hidden = !session.doc.is_visible(*id);
            let bg = if selected {
                p.accent_soft()
            } else if response.hovered() && self.dragging.is_none() {
                p.hover
            } else if index % 2 == 1 {
                style::mix(p.bg_panel, p.bg_raised, 0.35)
            } else {
                Color32::TRANSPARENT
            };
            ui.painter().rect_filled(rect, 0.0, bg);
            let x = rect.min.x + 8.0 + *depth as f32 * INDENT;
            // Стрелка группы.
            if node.is_group() {
                let arrow = Rect::from_center_size(pos2(x + 4.0, rect.center().y), vec2(8.0, 8.0));
                let open = !self.collapsed.contains(id);
                icons::paint(ui.painter(), arrow, if open { Icon::ChevronDown } else { Icon::ChevronRight }, p.text_muted);
                let toggle = ui.interact(arrow.expand(4.0), egui::Id::new(("modeler.outliner.fold", id.0)), Sense::click());
                if toggle.clicked() {
                    if open {
                        self.collapsed.insert(*id);
                    } else {
                        self.collapsed.remove(id);
                    }
                }
            }
            let icon = match &node.kind {
                NodeKind::Group(_) => Icon::Group,
                NodeKind::Element(e) => match e.shape {
                    Shape::Cuboid => Icon::Cuboid,
                    Shape::Plane => Icon::Plane,
                    Shape::Cylinder { .. } => Icon::Cylinder,
                    Shape::Cone { .. } => Icon::Cone,
                    Shape::Sphere { .. } => Icon::Sphere,
                },
            };
            let text_color = if hidden { p.text_faint } else if selected { Color32::WHITE } else { p.text };
            let icon_rect = Rect::from_center_size(pos2(x + 18.0, rect.center().y), vec2(13.0, 13.0));
            icons::paint(
                ui.painter(),
                icon_rect,
                icon,
                if node.is_group() { style::mix(p.warn, p.text, 0.35) } else { style::mix(p.accent, p.text, 0.4) },
            );
            let name_x = x + 30.0;
            // Переименование на месте.
            let renaming = self.renaming.as_ref().is_some_and(|(r, ..)| r == id);
            if renaming {
                let name_rect = Rect::from_min_max(pos2(name_x, rect.min.y + 1.0), pos2(rect.max.x - 48.0, rect.max.y - 1.0));
                let (_, text, focus) = self.renaming.as_mut().expect("checked");
                let edit = ui.put(name_rect, egui::TextEdit::singleline(text).font(style::body()));
                if *focus {
                    edit.request_focus();
                    *focus = false;
                }
                if edit.lost_focus() {
                    let (_, name, _) = self.renaming.take().expect("checked");
                    if !ui.input(|i| i.key_pressed(egui::Key::Escape)) {
                        session.edit("Rename", |doc| ops::rename(doc, *id, &name));
                    }
                }
            } else {
                let galley = ui.painter().layout_no_wrap(node.name.clone(), style::body(), text_color);
                ui.painter().galley(pos2(name_x, rect.center().y - galley.size().y * 0.5), galley, text_color);
            }
            // Близнец.
            if let Some(twin) = node.element().and_then(|e| e.twin) {
                let link = Rect::from_center_size(pos2(rect.max.x - 56.0, rect.center().y), vec2(12.0, 12.0));
                icons::paint(ui.painter(), link, Icon::Link, p.text_faint);
                let name = session.doc.node(twin).map_or(String::new(), |n| n.name.clone());
                ui.interact(link, egui::Id::new(("modeler.outliner.twin", id.0)), Sense::hover())
                    .on_hover_text(trf!("Mirrored twin of `{name}`: edits are repeated across X = 0", name = name));
            }
            // Замок и глаз: видны всегда, если включены, иначе — при наведении.
            let show_toggles = response.hovered() || !node.visible || node.locked;
            let eye = Rect::from_center_size(pos2(rect.max.x - 14.0, rect.center().y), vec2(14.0, 14.0));
            let lock = Rect::from_center_size(pos2(rect.max.x - 34.0, rect.center().y), vec2(13.0, 13.0));
            let eye_response = ui.interact(eye.expand(3.0), egui::Id::new(("modeler.outliner.eye", id.0)), Sense::click());
            let lock_response = ui.interact(lock.expand(3.0), egui::Id::new(("modeler.outliner.lock", id.0)), Sense::click());
            if show_toggles || eye_response.hovered() || lock_response.hovered() {
                let tint = |on: bool, hovered: bool| if hovered { p.text } else if on { p.text_muted } else { p.text_faint };
                icons::paint(ui.painter(), eye, if node.visible { Icon::Eye } else { Icon::EyeOff }, tint(node.visible, eye_response.hovered()));
                icons::paint(ui.painter(), lock, if node.locked { Icon::Lock } else { Icon::Unlock }, tint(node.locked, lock_response.hovered()));
            }
            if eye_response.on_hover_text(tr("Show / hide in the view (H)")).clicked() {
                session.edit("Visibility", |doc| ops::set_visible(doc, *id, !node.visible));
            }
            if lock_response.on_hover_text(tr("Lock: cannot be picked, moved or painted")).clicked() {
                session.edit("Lock", |doc| ops::set_locked(doc, *id, !node.locked));
            }

            if response.clicked() {
                if modifiers.command || modifiers.shift {
                    session.toggle_selected(*id);
                } else {
                    session.select(Some(*id));
                }
            }
            if response.double_clicked() {
                self.renaming = Some((*id, node.name.clone(), true));
            }
            if response.drag_started() {
                self.dragging = Some(*id);
                if !session.is_selected(*id) {
                    session.select(Some(*id));
                }
            }
            response.context_menu(|ui| {
                if ui.button(tr("Rename")).clicked() {
                    self.renaming = Some((*id, node.name.clone(), true));
                    ui.close_menu();
                }
                if ui.button(tr("Duplicate")).clicked() {
                    if !session.is_selected(*id) {
                        session.select(Some(*id));
                    }
                    command = Some(Command::Duplicate);
                    ui.close_menu();
                }
                if ui.button(tr("Group")).clicked() {
                    if !session.is_selected(*id) {
                        session.select(Some(*id));
                    }
                    command = Some(Command::GroupSelection);
                    ui.close_menu();
                }
                if node.element().is_some_and(|e| e.twin.is_none()) && ui.button(tr("Mirror X")).clicked() {
                    session.select(Some(*id));
                    command = Some(Command::MirrorX);
                    ui.close_menu();
                }
                if node.element().is_some_and(|e| e.twin.is_some()) && ui.button(tr("Unlink Twin")).clicked() {
                    session.select(Some(*id));
                    command = Some(Command::UnlinkTwin);
                    ui.close_menu();
                }
                ui.separator();
                if ui.button(tr("Delete")).clicked() {
                    if !session.is_selected(*id) {
                        session.select(Some(*id));
                    }
                    command = Some(Command::Delete);
                    ui.close_menu();
                }
            });

            // Куда упадёт перетаскиваемый узел.
            if let (Some(dragged), Some(pos)) = (self.dragging, pointer) {
                if rect.contains(pos) && dragged != *id {
                    let t = (pos.y - rect.min.y) / rect.height();
                    drop = Some(if node.is_group() && (0.28..0.72).contains(&t) {
                        Drop::Into(*id)
                    } else if t < 0.5 {
                        Drop::Before(*id)
                    } else {
                        Drop::After(*id)
                    });
                }
            }
        }
        // Пустое место под деревом — бросить в корень.
        let (rest, _) = ui.allocate_exact_size(vec2(ui.available_width(), 24.0), Sense::hover());
        if let (Some(_), Some(pos)) = (self.dragging, pointer) {
            if rest.contains(pos) {
                drop = Some(Drop::Root);
            }
        }
        if let Some(target) = drop {
            let line = Stroke::new(2.0_f32, p.accent);
            let find = |id: NodeId| rects.iter().find(|(r, _)| *r == id).map(|(_, rect)| *rect);
            match target {
                Drop::Before(id) => {
                    if let Some(r) = find(id) {
                        ui.painter().line_segment([r.left_top(), r.right_top()], line);
                    }
                }
                Drop::After(id) => {
                    if let Some(r) = find(id) {
                        ui.painter().line_segment([r.left_bottom(), r.right_bottom()], line);
                    }
                }
                Drop::Into(id) => {
                    if let Some(r) = find(id) {
                        ui.painter().rect_stroke(r.shrink(1.0), 2.0, line);
                    }
                }
                Drop::Root => {
                    ui.painter().line_segment([rest.left_top(), rest.right_top()], line);
                }
            }
        }
        if let Some(dragged) = self.dragging {
            if let Some(pos) = pointer {
                ui.painter().text(
                    pos + vec2(14.0, 4.0),
                    egui::Align2::LEFT_CENTER,
                    session.doc.node(dragged).map_or("", |n| n.name.as_str()),
                    style::small(),
                    p.text,
                );
            }
            if released {
                self.dragging = None;
                if let Some(target) = drop {
                    let moving: Vec<NodeId> = if session.is_selected(dragged) {
                        ops::top_level(&session.doc, &session.selection)
                    } else {
                        vec![dragged]
                    };
                    session.edit("Reorder", |doc| {
                        for id in moving {
                            let (parent, index) = match target {
                                Drop::Root => (None, None),
                                Drop::Into(group) => (Some(group), None),
                                Drop::Before(other) | Drop::After(other) => {
                                    if other == id {
                                        continue;
                                    }
                                    let parent = doc.node(other).and_then(|n| n.parent);
                                    let siblings: Vec<NodeId> =
                                        doc.children(parent).map(|n| n.id).filter(|n| *n != id).collect();
                                    let at = siblings.iter().position(|n| *n == other).unwrap_or(siblings.len());
                                    (parent, Some(if matches!(target, Drop::After(_)) { at + 1 } else { at }))
                                }
                            };
                            ops::reparent(doc, id, parent, index);
                        }
                    });
                }
            }
        }
        command
    }
}
