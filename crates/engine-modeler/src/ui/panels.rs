//! Правые панели: Element (числовые поля выделенного), Palette (цвета и кисть), Textures.

use egui::{vec2, Color32, Ui};
use engine_i18n::{n_, tr, trf};
use glam::Vec3;

use super::icons::Icon;
use super::style::{self, Palette};
use super::{Command, PaintSettings, PromptKind, ViewSettings};
use crate::doc::{FaceDir, NodeKind, Shape};
use crate::image::{parse_hex, to_hex};
use crate::ops;
use crate::session::Session;

/// Что сделала панель Element.
#[derive(Default)]
pub(super) struct PanelEdit {
    changed: bool,
    pub command: Option<Command>,
}

impl PanelEdit {
    pub fn started_or_changed(&self) -> bool {
        self.changed
    }
}

fn margin(ui: &mut Ui, add: impl FnOnce(&mut Ui)) {
    egui::Frame::none()
        .inner_margin(egui::Margin { left: 10.0, right: 10.0, top: 8.0, bottom: 10.0 })
        .show(ui, |ui| {
            ui.spacing_mut().item_spacing = vec2(4.0, 5.0);
            add(ui)
        });
}

pub(super) fn element_panel(ui: &mut Ui, session: &mut Session, _view: &ViewSettings) -> PanelEdit {
    let p = Palette::from_ctx(ui.ctx());
    let mut out = PanelEdit::default();
    margin(ui, |ui| {
        let Some(id) = session.primary() else {
            ui.label(egui::RichText::new(tr("Nothing selected")).color(p.text_muted));
            ui.label(egui::RichText::new(tr("Click an element in the view or the Outliner")).size(11.5).color(p.text_faint));
            return;
        };
        let Some(node) = session.doc.node(id).cloned() else { return };
        if session.selection.len() > 1 {
            ui.label(egui::RichText::new(trf!("{count} selected — editing `{name}`", count = session.selection.len(), name = node.name)).size(11.5).color(p.text_muted));
        }
        // Имя.
        style::row(ui, "Name", |ui| {
            let key = egui::Id::new(("modeler.name", id.0));
            let mut text: String = ui.data_mut(|d| d.get_temp::<String>(key)).unwrap_or_else(|| node.name.clone());
            let response = super::controls::text_field(ui, &mut text, "", f32::INFINITY);
            if response.has_focus() {
                ui.data_mut(|d| d.insert_temp(key, text.clone()));
            } else {
                ui.data_mut(|d| d.remove::<String>(key));
            }
            if response.lost_focus() && text != node.name {
                session.edit("Rename", |doc| ops::rename(doc, id, &text));
            }
        });

        let begin = |session: &mut Session| {
            if !session.in_progress() {
                session.begin("Edit Properties");
            }
        };

        match &node.kind {
            NodeKind::Group(group) => {
                let mut g = group.clone();
                let mut changed = false;
                style::row(ui, "Origin", |ui| {
                    let mut v = g.origin.to_array();
                    changed |= style::vec3_fields(ui, "Origin", &mut v, 0.1, "").changed;
                    g.origin = Vec3::from_array(v);
                });
                style::row(ui, "Rotation", |ui| {
                    let mut v = g.rotation.to_array();
                    changed |= style::vec3_fields(ui, "Rotation", &mut v, 0.5, "°").changed;
                    g.rotation = Vec3::from_array(v.map(ops::wrap_degrees));
                });
                if changed {
                    begin(session);
                    ops::set_group(&mut session.doc, id, g);
                    out.changed = true;
                }
                ui.label(
                    egui::RichText::new(tr("The group rotates its children around Origin — the joint of a limb."))
                        .size(11.0)
                        .color(p.text_faint),
                );
            }
            NodeKind::Element(element) => {
                let mut e = element.clone();
                let mut changed = false;
                style::row(ui, "Shape", |ui| {
                    let mut shape = e.shape;
                    super::controls::combo(ui, "modeler.shape", &style::shape_name(&shape), 110.0, |ui| {
                            let segments = shape.segments().unwrap_or(8);
                            for option in [
                                Shape::Cuboid,
                                Shape::Plane,
                                Shape::Cylinder { segments },
                                Shape::Cone { segments },
                                Shape::Sphere { segments, rings: (segments / 2).max(2) },
                            ] {
                                if ui.selectable_label(option.name() == shape.name(), style::shape_name(&option)).clicked() {
                                    shape = option;
                                }
                            }
                        });
                    match &mut shape {
                        Shape::Cylinder { segments } | Shape::Cone { segments } => {
                            super::controls::number_field_ex(ui, segments, 3..=64, 0.1, &tr("sides "), "", 0, 72.0);
                        }
                        Shape::Sphere { segments, rings } => {
                            super::controls::number_field_ex(ui, segments, 3..=64, 0.1, &tr("sides "), "", 0, 72.0);
                            super::controls::number_field_ex(ui, rings, 2..=32, 0.1, &tr("rings "), "", 0, 72.0);
                        }
                        _ => {}
                    }
                    // Форму меша сменить нельзя (его вершины пропали бы); в кубы — Convert to Cubes.
                    if shape != e.shape && e.shape != Shape::Mesh && shape != Shape::Mesh {
                        let faces = shape.faces();
                        for dir in FaceDir::ALL {
                            e.face_mut(dir).enabled = faces.contains(&dir);
                        }
                        e.shape = shape;
                        changed = true;
                    }
                });
                let field = |ui: &mut Ui, label: &str, value: &mut Vec3, speed: f64, suffix: &str| {
                    let mut changed = false;
                    style::row(ui, label, |ui| {
                        let mut v = value.to_array();
                        changed = style::vec3_fields(ui, label, &mut v, speed, suffix).changed;
                        *value = Vec3::from_array(v);
                    });
                    changed
                };
                let before_from = e.from;
                changed |= field(ui, n_("Position"), &mut e.from, 0.1, "");
                if e.from != before_from {
                    // Позиция двигает и точку вращения: форма переезжает целиком.
                    e.pivot += e.from - before_from;
                }
                changed |= field(ui, n_("Size"), &mut e.size, 0.1, "");
                e.size = e.size.max(Vec3::ZERO);
                changed |= field(ui, n_("Pivot"), &mut e.pivot, 0.1, "");
                let mut rotation = e.rotation;
                if field(ui, n_("Rotation"), &mut rotation, 0.5, "°") {
                    e.rotation = rotation.to_array().map(ops::wrap_degrees).into();
                    changed = true;
                }
                style::row(ui, "Inflate", |ui| {
                    let r = super::controls::number_field_ex(ui, &mut e.inflate, -8.0..=8.0, 0.05, "", "", 3, 72.0)
                        .on_hover_text(tr("Grows the box on every side; the texture stays the same"));
                    changed |= r.changed();
                });

                // Стороны.
                ui.add_space(super::kit::SP_2);
                style::row(ui, "Faces", |ui| {
                    ui.spacing_mut().item_spacing.x = 2.0;
                    for dir in e.shape.faces() {
                        let face = e.face(*dir);
                        let selected = session.face == Some(*dir);
                        let response = ui
                            .add_enabled_ui(face.enabled || selected, |ui| super::controls::toggle_button(ui, selected, &dir.name()[..1]))
                            .inner
                            .on_hover_text(trf!("{face} face — click to select for UV and colour", face = style::face_name(*dir)));
                        if response.clicked() {
                            session.face = if selected { None } else { Some(*dir) };
                        }
                    }
                });
                let targets: Vec<FaceDir> = match session.face.filter(|f| e.shape.faces().contains(f)) {
                    Some(f) => vec![f],
                    None => e.shape.faces().to_vec(),
                };
                let first = *e.face(targets[0]);
                let scope = if targets.len() == 1 { style::face_name(targets[0]).to_string() } else { tr("All faces").to_string() };
                ui.label(egui::RichText::new(scope).size(11.0).color(p.text_faint));
                style::row(ui, "Visible", |ui| {
                    let mut enabled = first.enabled;
                    if ui.checkbox(&mut enabled, "").changed() {
                        for f in &targets {
                            e.face_mut(*f).enabled = enabled;
                        }
                        changed = true;
                    }
                    ui.add_space(super::kit::SP_4);
                    let mut emissive = first.emissive;
                    if ui.checkbox(&mut emissive, tr("Emissive")).on_hover_text(tr("Unlit in the view; exported as emission")).changed() {
                        for f in &targets {
                            e.face_mut(*f).emissive = emissive;
                        }
                        changed = true;
                    }
                });
                style::row(ui, "Tint", |ui| {
                    let mut color = style::to_color32(first.tint);
                    if egui::widgets::color_picker::color_edit_button_srgba(ui, &mut color, egui::color_picker::Alpha::Opaque).changed() {
                        for f in &targets {
                            e.face_mut(*f).tint = style::from_color32(color);
                        }
                        changed = true;
                    }
                    if first.tint != [255, 255, 255, 255] && super::controls::button(ui, &tr("Reset")).clicked() {
                        for f in &targets {
                            e.face_mut(*f).tint = [255, 255, 255, 255];
                        }
                        changed = true;
                    }
                });
                style::row(ui, "Texture", |ui| {
                    let names: Vec<String> = session.doc.textures.iter().map(|t| t.name.clone()).collect();
                    let mut texture = first.texture as usize;
                    super::controls::combo(ui, "modeler.face_texture", &names.get(texture).cloned().unwrap_or_default(), 120.0, |ui| {
                            for (i, name) in names.iter().enumerate() {
                                ui.selectable_value(&mut texture, i, name);
                            }
                        });
                    if texture as u32 != first.texture {
                        for f in &targets {
                            e.face_mut(*f).texture = texture as u32;
                        }
                        changed = true;
                    }
                });
                style::row(ui, "UV", |ui| {
                    let mut auto = e.box_uv;
                    let twin_driven = e.twin.is_some() && !e.box_uv;
                    let response = ui
                        .add_enabled(!twin_driven, egui::Checkbox::new(&mut auto, tr("Box layout")))
                        .on_hover_text(tr("Unwrap automatically like a paper box; turn off to place faces by hand"));
                    if response.changed() {
                        if auto {
                            out.command = Some(Command::AutoUv);
                        } else {
                            e.box_uv = false;
                            changed = true;
                        }
                    }
                    if twin_driven {
                        ui.label(egui::RichText::new(tr("shares its twin's texture")).size(11.0).color(p.text_faint));
                    }
                });
                if let Some(twin) = e.twin {
                    let name = session.doc.node(twin).map_or("?".to_string(), |n| n.name.clone());
                    style::row(ui, "Twin", |ui| {
                        ui.label(egui::RichText::new(name).color(p.text_muted));
                        if super::controls::button(ui, &tr("Unlink")).clicked() {
                            out.command = Some(Command::UnlinkTwin);
                        }
                    });
                }
                if changed {
                    begin(session);
                    ops::set_element(&mut session.doc, id, e);
                    out.changed = true;
                }
            }
        }
    });
    out
}

pub(super) fn palette_panel(ui: &mut Ui, paint: &mut PaintSettings) {
    let p = Palette::from_ctx(ui.ctx());
    margin(ui, |ui| {
        ui.horizontal(|ui| {
            let primary = style::swatch(ui, egui::Id::new("modeler.palette.primary"), style::to_color32(paint.color), 34.0, true);
            primary.on_hover_text(tr("Brush colour"));
            let secondary = style::swatch(ui, egui::Id::new("modeler.palette.secondary"), style::to_color32(paint.secondary), 22.0, false);
            if secondary.on_hover_text(tr("Second colour: click to swap (X)")).clicked() {
                std::mem::swap(&mut paint.color, &mut paint.secondary);
            }
            ui.add_space(super::kit::SP_3);
            ui.vertical(|ui| {
                let key = egui::Id::new("modeler.palette.hex");
                let mut text: String = ui.data_mut(|d| d.get_temp::<String>(key)).unwrap_or_else(|| to_hex(paint.color));
                let response = super::controls::text_field(ui, &mut text, "", 96.0);
                if response.has_focus() {
                    ui.data_mut(|d| d.insert_temp(key, text.clone()));
                    if let Some(c) = parse_hex(&text) {
                        paint.color = c;
                    }
                } else {
                    ui.data_mut(|d| d.remove::<String>(key));
                }
                let [r, g, b, a] = paint.color;
                ui.label(egui::RichText::new(format!("R {r}  G {g}  B {b}  A {a}")).size(10.5).color(p.text_faint));
            });
        });
        ui.add_space(super::kit::SP_2);
        let mut color = style::to_color32(paint.color);
        if egui::widgets::color_picker::color_picker_color32(ui, &mut color, egui::color_picker::Alpha::OnlyBlend) {
            paint.color = style::from_color32(color);
        }
        if !paint.recent.is_empty() {
            ui.add_space(super::kit::SP_2);
            ui.label(egui::RichText::new(tr("RECENT")).size(10.5).color(p.text_faint));
            ui.horizontal_wrapped(|ui| {
                ui.spacing_mut().item_spacing = vec2(3.0, 3.0);
                for (i, c) in paint.recent.clone().iter().enumerate() {
                    if style::swatch(ui, egui::Id::new(("modeler.recent", i)), style::to_color32(*c), 18.0, *c == paint.color).clicked() {
                        paint.color = *c;
                    }
                }
            });
        }
        ui.add_space(super::kit::SP_2);
        ui.horizontal(|ui| {
            ui.label(egui::RichText::new(tr("PALETTE")).size(10.5).color(p.text_faint));
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if style::icon_button(ui, "modeler.palette.add", Icon::Plus, false, true, "Add the brush colour to the palette").clicked()
                    && !paint.palette.contains(&paint.color)
                {
                    paint.palette.push(paint.color);
                }
            });
        });
        let mut remove = None;
        ui.horizontal_wrapped(|ui| {
            ui.spacing_mut().item_spacing = vec2(3.0, 3.0);
            for (i, c) in paint.palette.iter().enumerate() {
                let response = style::swatch(ui, egui::Id::new(("modeler.palette", i)), style::to_color32(*c), 18.0, *c == paint.color)
                    .on_hover_text(trf!("{hex}  ·  right click removes", hex = to_hex(*c)));
                if response.clicked() {
                    paint.color = *c;
                }
                if response.secondary_clicked() {
                    remove = Some(i);
                }
            }
        });
        if let Some(i) = remove {
            paint.palette.remove(i);
        }
    });
}

/// Что попросила панель Textures.
pub(super) enum TextureAction {
    Command(Command),
    Ask(PromptKind),
}

pub(super) fn textures_panel(ui: &mut Ui, session: &mut Session, current: &mut u32) -> Option<TextureAction> {
    let p = Palette::from_ctx(ui.ctx());
    let mut action = None;
    margin(ui, |ui| {
        for (i, texture) in session.doc.textures.iter().enumerate() {
            let selected = *current == i as u32;
            let text = format!("{}   {}×{}", texture.name, texture.image.width(), texture.image.height());
            let label = egui::RichText::new(text).color(if selected { Color32::WHITE } else { p.text });
            let response = ui.add_sized([ui.available_width(), 22.0], egui::SelectableLabel::new(selected, label));
            let response = match &texture.source {
                Some(source) => response.on_hover_text(trf!("Imported from {source}; reloads when the file changes", source = source)),
                None => response,
            };
            if response.clicked() {
                *current = i as u32;
            }
        }
        ui.horizontal_wrapped(|ui| {
            if super::controls::button(ui, &tr("Add")).clicked() {
                action = Some(TextureAction::Command(Command::AddTexture));
            }
            if super::controls::button(ui, &tr("Resize…")).clicked() {
                action = Some(TextureAction::Ask(PromptKind::ResizeTexture));
            }
            if super::controls::button(ui, &tr("Import PNG…")).clicked() {
                action = Some(TextureAction::Ask(PromptKind::ImportTexture));
            }
            if super::controls::button(ui, &tr("Export PNG…")).clicked() {
                action = Some(TextureAction::Ask(PromptKind::ExportTexture));
            }
        });
    });
    action
}
