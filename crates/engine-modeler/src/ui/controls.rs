//! КОПИЯ `crates/engine-editor/src/controls.rs` движка Piska — не править руками.
//!
//! Обновить: `python scripts/sync_from_engine.py --engine <движок>`.
#![allow(dead_code, unused_imports)]

// Контролы набора (`kit::controls`): единственный способ поставить кнопку, поле, число,
// список, сегменты, строку, карточку в редакторе, хабе и Modeler.
//
// Правила (`docs/design/ui-guidelines.md`, «Один размер контрола»):
// * внешняя высота любого контрола, который стоит в строке с другими, — высота плотности
//   ([`theme::Density`]: [`theme::CONTROL_H`] = 22 в редакторе и Modeler, 28 в хабе и установщике);
//   рамка рисуется ВНУТРИ выделенного прямоугольника (сдвиг на половину толщины), наведение,
//   нажатие и фокус размер не меняют (кольцо фокуса — внутри);
// * ширина кнопки — подпись + поле плотности с каждой стороны, не меньше высоты; кнопка
//   не шире места в строке — подпись обрезается «…» с подсказкой;
// * промежутки — только токены шкалы (`SP_*` и смысловые `ITEM_GAP`, `CARD_PAD`…);
// * строка кнопок не выходит за контейнер — переносится ([`button_row`]).
//
// Проверка исходников (`crates/engine/tests/ui_source_check.rs`) не даёт ставить
// `egui::Button`, `TextEdit`, `ComboBox`, `ui.button(` и `add_space(<число>)` мимо набора.

use egui::text::{LayoutJob, TextFormat, TextWrapping};
use egui::{pos2, vec2, Align, Color32, FontId, Layout, Rect, Response, Sense, Stroke, Ui};

use super::icons::{self, Icon};
use super::kit as theme;
use super::kit::{
    accent, body, mix, BG_ACTIVE, BG_FIELD, BG_HOVER, BG_INSET, CARD_PAD, ERROR, ICON_S,
    ITEM_GAP, LINE, LINE_HOVER, LINE_STRONG, R_M, R_S, SP_1, SP_3, STROKE, TEXT, TEXT_DISABLED, TEXT_MUTED, TEXT_STRONG,
};

/// Вид кнопки.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    /// Обычная.
    Normal,
    /// Главная (акцент). Одна на окно.
    Primary,
    /// Опасное действие.
    Danger,
    /// Без рамки (полосы инструментов, ссылки).
    Ghost,
}

/// Рамка контрола внутри `rect`: обводка не выходит за выделенный прямоугольник.
pub fn paint_frame(painter: &egui::Painter, rect: Rect, rounding: f32, fill: Color32, stroke: Color32) {
    let inset = rect.shrink(STROKE * 0.5);
    painter.rect(inset, (rounding - STROKE * 0.5).max(0.0), fill, Stroke::new(STROKE, stroke));
}

/// Кольцо фокуса — внутри контрола, размер не меняется.
pub fn paint_focus(painter: &egui::Painter, rect: Rect) {
    painter.rect_stroke(rect.shrink(STROKE + 1.0), R_S, Stroke::new(STROKE, accent()));
}

fn one_line(painter: &egui::Painter, text: &str, font: FontId, width: Option<f32>) -> std::sync::Arc<egui::Galley> {
    let mut job = LayoutJob::single_section(text.to_string(), TextFormat { font_id: font, color: Color32::PLACEHOLDER, ..TextFormat::default() });
    if let Some(width) = width {
        job.wrap = TextWrapping { max_width: width.max(8.0), max_rows: 1, break_anywhere: true, overflow_character: Some('…') };
    }
    painter.layout_job(job)
}

/// Естественная ширина кнопки (иконка + подпись + поля) — для раскладки строк заранее.
pub fn button_width(ui: &Ui, icon: Option<Icon>, label: &str) -> f32 {
    let d = theme::density(ui.ctx());
    let text = if label.is_empty() { 0.0 } else { one_line(ui.painter(), label, d.button_font(), None).size().x };
    let icon_w = icon_space(icon, label);
    (text + icon_w + d.pad_x() * 2.0).max(d.control_h()).ceil()
}

fn icon_space(icon: Option<Icon>, label: &str) -> f32 {
    if icon.is_some() {
        ICON_S + if label.is_empty() { 0.0 } else { SP_3 }
    } else {
        0.0
    }
}

/// Цвета кнопки вида `kind` в состоянии: (заливка, рамка, текст). У всех видов, кроме
/// [`Kind::Ghost`], рамка одинаково заметна: у главной и опасной — на тон светлее заливки,
/// чтобы кнопка не выглядела безрамочной и меньше соседней обычной.
pub fn button_colors(kind: Kind, enabled: bool, hovered: bool, pressed: bool) -> (Color32, Color32, Color32) {
    match (kind, enabled) {
        (Kind::Ghost, false) => (Color32::TRANSPARENT, Color32::TRANSPARENT, TEXT_DISABLED),
        (_, false) => (BG_FIELD, LINE, TEXT_DISABLED),
        (Kind::Primary, true) => {
            let base = accent();
            let fill = if pressed { mix(base, Color32::BLACK, 0.15) } else if hovered { mix(base, Color32::WHITE, 0.12) } else { base };
            (fill, mix(fill, Color32::WHITE, 0.28), Color32::WHITE)
        }
        (Kind::Danger, true) => {
            let base = mix(BG_FIELD, ERROR, 0.7);
            let fill = if pressed { mix(base, Color32::BLACK, 0.15) } else if hovered { mix(base, Color32::WHITE, 0.1) } else { base };
            (fill, mix(fill, Color32::WHITE, 0.28), TEXT_STRONG)
        }
        (Kind::Ghost, true) => (
            if pressed { BG_ACTIVE } else if hovered { BG_HOVER } else { Color32::TRANSPARENT },
            Color32::TRANSPARENT,
            if hovered { TEXT_STRONG } else { TEXT },
        ),
        (Kind::Normal, true) => (
            if pressed { BG_ACTIVE } else if hovered { BG_HOVER } else { BG_FIELD },
            if hovered { LINE_HOVER } else { LINE_STRONG },
            TEXT,
        ),
    }
}

/// Кнопка набора: высота [`theme::CONTROL_H`], рамка внутри, размер не зависит от состояния.
pub fn button_ex(ui: &mut Ui, kind: Kind, icon: Option<Icon>, label: &str, min_width: f32) -> Response {
    button_with_id(ui, None, kind, icon, label, min_width)
}

/// То же с заданным `id` (по нему кнопку находят тесты и фокус клавиатуры).
pub fn button_with_id(ui: &mut Ui, id: Option<egui::Id>, kind: Kind, icon: Option<Icon>, label: &str, min_width: f32) -> Response {
    let d = theme::density(ui.ctx());
    let h = d.control_h();
    let natural = button_width(ui, icon, label).max(min_width);
    // В переносящей строке остаток строки мал, но кнопка уйдёт на следующую: мерить по
    // всей ширине строки, а не по остатку.
    let room = if ui.layout().main_wrap() { ui.max_rect().width() } else { ui.available_width() };
    let width = natural.min(room.max(h)).round();
    let response = match id {
        Some(id) => {
            let (rect, _) = ui.allocate_exact_size(vec2(width, h), Sense::hover());
            ui.interact(rect, id, Sense::click())
        }
        None => ui.allocate_exact_size(vec2(width, h), Sense::click()).1,
    };
    let rect = response.rect;
    if !ui.is_rect_visible(rect) {
        return response;
    }
    let enabled = ui.is_enabled();
    let hovered = response.hovered() && enabled;
    let pressed = response.is_pointer_button_down_on() && enabled;
    let (fill, stroke, text) = button_colors(kind, enabled, hovered, pressed);
    let painter = ui.painter();
    if kind == Kind::Ghost {
        if fill != Color32::TRANSPARENT {
            painter.rect_filled(rect, R_S, fill);
        }
    } else {
        paint_frame(painter, rect, R_S, fill, stroke);
    }
    let icon_w = icon_space(icon, label);
    let room = rect.width() - d.pad_x() * 2.0 - icon_w;
    let galley = (!label.is_empty()).then(|| one_line(painter, label, d.button_font(), Some(room)));
    let elided = galley.as_ref().is_some_and(|g| g.elided);
    let content = icon_w + galley.as_ref().map_or(0.0, |g| g.size().x);
    let mut x = (rect.center().x - content * 0.5).round();
    if let Some(icon) = icon {
        let color = if !enabled { TEXT_DISABLED } else if kind == Kind::Normal || kind == Kind::Ghost { TEXT_MUTED } else { text };
        icons::paint(painter, Rect::from_center_size(pos2(x + ICON_S * 0.5, rect.center().y), vec2(ICON_S, ICON_S)), icon, color);
        x += icon_w;
    }
    if let Some(galley) = galley {
        let y = (rect.center().y - galley.size().y * 0.5).round();
        painter.galley(pos2(x, y), galley, text);
    }
    if response.has_focus() {
        paint_focus(painter, rect);
    }
    if elided {
        response.on_hover_text(label.to_string())
    } else {
        response
    }
}

/// Обычная кнопка.
pub fn button(ui: &mut Ui, label: &str) -> Response {
    button_ex(ui, Kind::Normal, None, label, 0.0)
}

/// Главная кнопка (одна на окно).
pub fn primary_button(ui: &mut Ui, label: &str) -> Response {
    button_ex(ui, Kind::Primary, None, label, 0.0)
}

/// Опасное действие.
pub fn danger_button(ui: &mut Ui, label: &str) -> Response {
    button_ex(ui, Kind::Danger, None, label, 0.0)
}

/// Кнопка с иконкой и подписью.
pub fn icon_label_button(ui: &mut Ui, icon: Icon, label: &str) -> Response {
    button_ex(ui, Kind::Normal, Some(icon), label, 0.0)
}

/// Квадратная кнопка-иконка со стороной [`theme::CONTROL_H`] и подсказкой.
pub fn icon_button(ui: &mut Ui, icon: Icon, tooltip: &str) -> Response {
    let h = theme::control_h(ui);
    button_ex(ui, Kind::Ghost, Some(icon), "", h).on_hover_text(tooltip)
}

/// Кнопка-иконка с рамкой (как обычная кнопка) — рядом с полями и кнопками строки.
pub fn icon_button_framed(ui: &mut Ui, icon: Icon, tooltip: &str) -> Response {
    let h = theme::control_h(ui);
    button_ex(ui, Kind::Normal, Some(icon), "", h).on_hover_text(tooltip)
}

/// Кнопка не уже `width` (`f32::INFINITY` — вся строка): «Browse…» рядом с полем,
/// кнопки одной колонки.
pub fn button_sized(ui: &mut Ui, kind: Kind, icon: Option<Icon>, label: &str, width: f32) -> Response {
    let width = if width.is_finite() { width } else { ui.available_width() };
    button_ex(ui, kind, icon, label, width)
}

/// Однострочное поле ввода высотой [`theme::CONTROL_H`] шириной `width` (`f32::INFINITY` — вся
/// строка). Рамка egui рисуется по краю — поэтому поле ставится в прямоугольник, уменьшенный
/// на половину рамки: снаружи оно ровно [`theme::CONTROL_H`].
pub fn text_field(ui: &mut Ui, text: &mut String, hint: &str, width: f32) -> Response {
    text_field_with_id(ui, None, text, hint, width)
}

/// То же с заданным `id` поля.
pub fn text_field_with_id(ui: &mut Ui, id: Option<egui::Id>, text: &mut String, hint: &str, width: f32) -> Response {
    text_field_font(ui, id, text, hint, width, body())
}

/// То же со шрифтом `font` (пути и команды — моноширинным).
pub fn text_field_font(ui: &mut Ui, id: Option<egui::Id>, text: &mut String, hint: &str, width: f32, font: FontId) -> Response {
    let d = theme::density(ui.ctx());
    let h = d.control_h();
    let width = if width.is_finite() { width.min(ui.available_width()) } else { ui.available_width() };
    let (rect, _) = ui.allocate_exact_size(vec2(width.max(h), h), Sense::hover());
    // Рамка egui уже внутри (расширение −½ обводки в теме): поле занимает весь прямоугольник.
    let inner = rect;
    let mut child = ui.new_child(egui::UiBuilder::new().max_rect(inner).layout(Layout::left_to_right(Align::Center)));
    let font_h = ui.fonts(|f| f.row_height(&font));
    let margin_y = ((inner.height() - font_h) * 0.5).max(0.0);
    let mut edit = egui::TextEdit::singleline(text)
        .font(font)
        .margin(egui::Margin::symmetric(d.pad_x() - STROKE * 0.5, margin_y))
        .min_size(inner.size())
        .desired_width(inner.width() - (d.pad_x() - STROKE * 0.5) * 2.0)
        .vertical_align(Align::Center);
    if !hint.is_empty() {
        edit = edit.hint_text(egui::RichText::new(hint).color(TEXT_DISABLED));
    }
    if let Some(id) = id {
        edit = edit.id(id);
    }
    // Ответ — на весь прямоугольник контрола (замеры раскладки видят его внешний размер).
    let mut response = child.add(edit);
    response.rect = rect;
    response
}

/// Поле числа (перетаскиванием или вводом) высотой [`theme::CONTROL_H`].
pub fn number_field<N: egui::emath::Numeric>(ui: &mut Ui, value: &mut N, speed: f64, width: f32) -> Response {
    let h = theme::control_h(ui);
    let (rect, _) = ui.allocate_exact_size(vec2(width.min(ui.available_width()).max(h), h), Sense::hover());
    // Рамка egui уже внутри (расширение −½ обводки в теме): поле занимает весь прямоугольник.
    let inner = rect;
    let mut child = ui.new_child(egui::UiBuilder::new().max_rect(inner).layout(Layout::left_to_right(Align::Center)));
    child.spacing_mut().interact_size.y = inner.height();
    let mut response = child.add_sized(inner.size(), egui::DragValue::new(value).speed(speed).max_decimals(3));
    response.rect = rect;
    response
}

/// Поле числа для нескольких объектов с разными значениями: показывает «—»; потянуть или
/// вписать — задать всем.
pub fn number_field_mixed<N: egui::emath::Numeric>(ui: &mut Ui, value: &mut N, speed: f64, width: f32) -> Response {
    let h = theme::control_h(ui);
    let (rect, _) = ui.allocate_exact_size(vec2(width.min(ui.available_width()).max(h), h), Sense::hover());
    let mut child = ui.new_child(egui::UiBuilder::new().max_rect(rect).layout(Layout::left_to_right(Align::Center)));
    child.spacing_mut().interact_size.y = rect.height();
    let mut response = child.add_sized(rect.size(), egui::DragValue::new(value).speed(speed).custom_formatter(|_, _| "—".to_string()));
    response.rect = rect;
    response
}

/// Поле числа с пределами и единицами («s», « px») высотой [`theme::CONTROL_H`].
pub fn number_field_range<N: egui::emath::Numeric>(
    ui: &mut Ui,
    value: &mut N,
    range: std::ops::RangeInclusive<N>,
    speed: f64,
    suffix: &str,
    width: f32,
) -> Response {
    let h = theme::control_h(ui);
    let (rect, _) = ui.allocate_exact_size(vec2(width.min(ui.available_width()).max(h), h), Sense::hover());
    // Рамка egui уже внутри (расширение −½ обводки в теме): поле занимает весь прямоугольник.
    let inner = rect;
    let mut child = ui.new_child(egui::UiBuilder::new().max_rect(inner).layout(Layout::left_to_right(Align::Center)));
    child.spacing_mut().interact_size.y = inner.height();
    let mut response = child.add_sized(inner.size(), egui::DragValue::new(value).range(range).speed(speed).suffix(suffix).max_decimals(3));
    response.rect = rect;
    response
}

/// Выпадающий список высотой [`theme::CONTROL_H`]: подпись выбранного и пункты в `add`.
pub fn combo<R>(ui: &mut Ui, id: impl std::hash::Hash, selected: &str, width: f32, add: impl FnOnce(&mut Ui) -> R) -> Option<R> {
    let h = theme::control_h(ui);
    let (rect, _) = ui.allocate_exact_size(vec2(width.min(ui.available_width()).max(h), h), Sense::hover());
    // Рамка egui уже внутри (расширение −½ обводки в теме): поле занимает весь прямоугольник.
    let inner = rect;
    let mut child = ui.new_child(egui::UiBuilder::new().max_rect(inner).layout(Layout::left_to_right(Align::Center)));
    child.spacing_mut().interact_size.y = inner.height();
    egui::ComboBox::from_id_salt(id)
        .selected_text(selected)
        .width(inner.width())
        .height(320.0)
        .show_ui(&mut child, add)
        .inner
}

/// Выпадающий список высотой плотности с ответом (для подсказки) — как [`combo`].
/// `selected` может быть цветным (`RichText`): например, «(missing)» предупреждением.
pub fn combo_ex<R>(ui: &mut Ui, id: impl std::hash::Hash, selected: impl Into<egui::WidgetText>, width: f32, add: impl FnOnce(&mut Ui) -> R) -> egui::InnerResponse<Option<R>> {
    let h = theme::control_h(ui);
    let (rect, _) = ui.allocate_exact_size(vec2(width.min(ui.available_width()).max(h), h), Sense::hover());
    let mut child = ui.new_child(egui::UiBuilder::new().max_rect(rect).layout(Layout::left_to_right(Align::Center)));
    child.spacing_mut().interact_size.y = rect.height();
    let mut inner = egui::ComboBox::from_id_salt(id).selected_text(selected).width(rect.width()).height(320.0).show_ui(&mut child, add);
    inner.response.rect = rect;
    inner
}

/// Поле числа с пределами, подписью перед числом («sides 8») и после («°»), высотой плотности.
#[allow(clippy::too_many_arguments)]
pub fn number_field_ex<N: egui::emath::Numeric>(
    ui: &mut Ui,
    value: &mut N,
    range: std::ops::RangeInclusive<N>,
    speed: f64,
    prefix: &str,
    suffix: &str,
    decimals: usize,
    width: f32,
) -> Response {
    let h = theme::control_h(ui);
    let (rect, _) = ui.allocate_exact_size(vec2(width.min(ui.available_width()).max(h), h), Sense::hover());
    let mut child = ui.new_child(egui::UiBuilder::new().max_rect(rect).layout(Layout::left_to_right(Align::Center)));
    child.spacing_mut().interact_size.y = rect.height();
    let mut response = child.add_sized(
        rect.size(),
        egui::DragValue::new(value).range(range).speed(speed).prefix(prefix).suffix(suffix).max_decimals(decimals).update_while_editing(false),
    );
    response.rect = rect;
    response
}

/// Кнопка-переключатель (выбранная — с акцентом): стороны куба, режимы. Высота плотности,
/// не уже высоты.
pub fn toggle_button(ui: &mut Ui, selected: bool, label: &str) -> Response {
    let d = theme::density(ui.ctx());
    let h = d.control_h();
    let galley = one_line(ui.painter(), label, body(), None);
    let width = (galley.size().x + d.pad_x() * 2.0).max(h).round();
    let (rect, response) = ui.allocate_exact_size(vec2(width, h), Sense::click());
    let enabled = ui.is_enabled();
    let hovered = response.hovered() && enabled;
    let (fill, stroke, text) = if selected {
        (theme::accent_soft(), accent(), TEXT_STRONG)
    } else {
        let (f, s, t) = button_colors(Kind::Normal, enabled, hovered, response.is_pointer_button_down_on());
        (f, s, t)
    };
    paint_frame(ui.painter(), rect, R_S, fill, stroke);
    ui.painter().galley((rect.center() - galley.size() * 0.5).round(), galley, text);
    if response.has_focus() {
        paint_focus(ui.painter(), rect);
    }
    response
}

/// Невидимая кнопка размера `size`, открывающая всплывающее меню `add` (содержимое кнопки —
/// значок, плитка — рисует вызывающий). `framed` — рамка обычной кнопки.
pub fn menu_anchor<R>(ui: &mut Ui, size: egui::Vec2, framed: bool, add: impl FnOnce(&mut Ui) -> R) -> egui::InnerResponse<Option<R>> {
    egui::menu::menu_custom_button(ui, egui::Button::new("").frame(framed).min_size(size), add)
}

/// Кнопка с подписью, открывающая всплывающее меню: заливка поля, высота плотности,
/// не уже `min_width`. Подпись — текст или `RichText` (мелкий шрифт полосы, `LayoutJob`).
pub fn menu_button<R>(ui: &mut Ui, label: impl Into<egui::WidgetText>, min_width: f32, add: impl FnOnce(&mut Ui) -> R) -> egui::InnerResponse<Option<R>> {
    let h = theme::control_h(ui);
    egui::menu::menu_custom_button(ui, egui::Button::new(label).fill(BG_FIELD).min_size(vec2(min_width, h)), add)
}

/// Пункт меню: строка во всю ширину меню высотой [`theme::MENU_ROW`], подпись слева,
/// сочетание клавиш справа приглушённым.
pub fn menu_item(ui: &mut Ui, label: &str, shortcut: &str, enabled: bool) -> Response {
    let width = ui.available_width().max(theme::MENU_WIDTH * 0.5);
    let (rect, response) = ui.allocate_exact_size(vec2(width, theme::MENU_ROW), if enabled { Sense::click() } else { Sense::hover() });
    let hovered = response.hovered() && enabled;
    if hovered {
        ui.painter().rect_filled(rect, R_S, BG_HOVER);
    }
    let pad = theme::density(ui.ctx()).pad_x();
    let color = if enabled { TEXT } else { TEXT_DISABLED };
    let keys = (!shortcut.is_empty()).then(|| one_line(ui.painter(), shortcut, body(), None));
    let keys_w = keys.as_ref().map_or(0.0, |g| g.size().x + pad * 2.0);
    let galley = one_line(ui.painter(), label, body(), Some(rect.width() - pad * 2.0 - keys_w));
    ui.painter().galley(pos2(rect.left() + pad, (rect.center().y - galley.size().y * 0.5).round()), galley, color);
    if let Some(keys) = keys {
        ui.painter().galley(pos2(rect.right() - pad - keys.size().x, (rect.center().y - keys.size().y * 0.5).round()), keys, TEXT_DISABLED);
    }
    if response.clicked() {
        ui.close_menu();
    }
    response
}

/// Образец цвета egui рисует обводкой 2 по краю: на время вызова расширение −1, чтобы
/// обводка легла внутрь прямоугольника.
fn inset_color<R>(ui: &mut Ui, add: impl FnOnce(&mut Ui) -> R) -> R {
    ui.scope(|ui| {
        let w = &mut ui.visuals_mut().widgets;
        for v in [&mut w.inactive, &mut w.hovered, &mut w.active, &mut w.open] {
            v.expansion = -1.0;
        }
        add(ui)
    })
    .inner
}

/// Кнопка-образец цвета (sRGB с альфой), высота [`theme::CONTROL_H`].
pub fn color_button(ui: &mut Ui, value: &mut Color32) -> Response {
    inset_color(ui, |ui| ui.color_edit_button_srgba(value))
}

/// Кнопка-образец цвета (линейный RGBA без домножения).
pub fn color_button_rgba(ui: &mut Ui, value: &mut [f32; 4]) -> Response {
    inset_color(ui, |ui| ui.color_edit_button_rgba_unmultiplied(value))
}

/// Флажок с подписью, по центру строки высотой [`theme::CONTROL_H`].
pub fn checkbox(ui: &mut Ui, value: &mut bool, label: &str) -> Response {
    let h = theme::control_h(ui);
    ui.allocate_ui_with_layout(vec2(ui.available_width(), h), Layout::left_to_right(Align::Center), |ui| ui.checkbox(value, label))
        .inner
}

/// Квадрат флажка в `rect` (рамка внутри): для таблиц, где строка сама ловит щелчок.
pub fn paint_check_box(painter: &egui::Painter, rect: Rect, on: bool, live: bool, hovered: bool) {
    let (fill, stroke) = if on {
        (if live { accent() } else { theme::accent_soft() }, if live { mix(accent(), Color32::WHITE, 0.28) } else { LINE_STRONG })
    } else {
        (if hovered { BG_HOVER } else { BG_INSET }, if hovered { accent() } else { LINE_HOVER })
    };
    paint_frame(painter, rect, R_S, fill, stroke);
    if on {
        // Галочка линиями (не иконкой): одинакова в редакторе, хабе и Modeler.
        let r = rect.shrink(SP_1 + 1.0);
        let at = |x: f32, y: f32| pos2(r.left() + r.width() * x, r.top() + r.height() * y);
        let ink = Stroke::new(1.6_f32, if live { Color32::WHITE } else { TEXT_MUTED });
        painter.line_segment([at(0.12, 0.52), at(0.4, 0.8)], ink);
        painter.line_segment([at(0.4, 0.8), at(0.9, 0.2)], ink);
    }
}

/// Флажок, нарисованный набором: квадрат [`theme::ICON`] и подпись, строка высотой плотности,
/// заданный `id` (тесты, фокус). `locked` — отмечен и не снимается. Ответ — на всю строку.
pub fn check_with_id(ui: &mut Ui, id: egui::Id, on: bool, label: &str, enabled: bool, locked: bool) -> Response {
    let h = theme::control_h(ui);
    let galley = ui.painter().layout_no_wrap(label.to_string(), body(), TEXT);
    let box_s = theme::ICON;
    let (rect, _) = ui.allocate_exact_size(vec2(box_s + SP_3 + galley.size().x, h), Sense::hover());
    let response = ui.interact(rect, id, Sense::click());
    let live = enabled && !locked;
    let hovered = response.hovered() && live;
    let box_rect = Rect::from_min_size(pos2(rect.left(), (rect.center().y - box_s * 0.5).round()), vec2(box_s, box_s));
    paint_check_box(ui.painter(), box_rect, on, live, hovered);
    let color = if enabled { TEXT } else { TEXT_DISABLED };
    ui.painter().galley(pos2(box_rect.right() + SP_3, (rect.center().y - galley.size().y * 0.5).round()), galley, color);
    if response.has_focus() {
        paint_focus(ui.painter(), box_rect.expand(SP_1 + 1.0));
    }
    response
}

/// Переключатель-кружок с подписью (один из нескольких), строка высотой плотности.
pub fn radio_with_id(ui: &mut Ui, id: egui::Id, on: bool, label: &str, enabled: bool) -> Response {
    let h = theme::control_h(ui);
    let galley = ui.painter().layout_no_wrap(label.to_string(), body(), TEXT);
    let box_s = theme::ICON;
    let (rect, _) = ui.allocate_exact_size(vec2(box_s + SP_3 + galley.size().x, h), Sense::hover());
    let response = ui.interact(rect, id, Sense::click());
    let hovered = response.hovered() && enabled;
    let center = pos2(rect.left() + box_s * 0.5, rect.center().y.round());
    let r = box_s * 0.5 - STROKE * 0.5;
    ui.painter().circle(center, r, if hovered { BG_HOVER } else { BG_INSET }, Stroke::new(STROKE, if on { accent() } else { LINE_HOVER }));
    if on {
        ui.painter().circle_filled(center, box_s * 0.25, if enabled { accent() } else { TEXT_DISABLED });
    }
    let color = if enabled { TEXT } else { TEXT_DISABLED };
    ui.painter().galley(pos2(rect.left() + box_s + SP_3, (rect.center().y - galley.size().y * 0.5).round()), galley, color);
    response
}



/// Строка контролов: высота [`theme::CONTROL_H`], промежуток [`ITEM_GAP`], всё по центру строки.
pub fn row<R>(ui: &mut Ui, add: impl FnOnce(&mut Ui) -> R) -> R {
    let h = theme::control_h(ui);
    ui.allocate_ui_with_layout(vec2(ui.available_width(), h), Layout::left_to_right(Align::Center), |ui| {
        ui.spacing_mut().item_spacing.x = ITEM_GAP;
        add(ui)
    })
    .inner
}

/// Строка кнопок, которая никогда не выходит за контейнер: не влезает — перенос на
/// следующую строку (правило для карточек, панелей и диалогов).
pub fn button_row<R>(ui: &mut Ui, add: impl FnOnce(&mut Ui) -> R) -> R {
    ui.with_layout(Layout::left_to_right(Align::Center).with_main_wrap(true), |ui| {
        ui.spacing_mut().item_spacing = vec2(ITEM_GAP, ITEM_GAP);
        add(ui)
    })
    .inner
}



/// Карточка: фон, рамка внутри, поле [`CARD_PAD`] со всех сторон.
pub fn card<R>(ui: &mut Ui, add: impl FnOnce(&mut Ui) -> R) -> R {
    egui::Frame::none()
        .fill(theme::BG_RAISED)
        .stroke(Stroke::new(STROKE, LINE))
        .rounding(R_M)
        .inner_margin(egui::Margin::same(CARD_PAD))
        .show(ui, add)
        .inner
}

/// Вдавленный фон (списки, превью) с рамкой внутри.
pub fn inset_frame(painter: &egui::Painter, rect: Rect) {
    paint_frame(painter, rect, R_S, BG_INSET, LINE_STRONG);
}

/// Подпись приглушённым цветом.
pub fn muted(ui: &mut Ui, text: &str) -> Response {
    ui.label(egui::RichText::new(text).color(TEXT_MUTED))
}


